# linklet-supervise.ps1 -- keep a linklet agent running on this machine.
#
# docs/ROADMAP.md M10 named this as the thing with no counterpart: the agent was started by a
# scheduled task, and a scheduled task does not restart a process that died and cannot tell a
# wedged one from a busy one. This does both.
#
# It runs in the foreground and is meant to be started by the scheduler, which is what keeps it
# alive across a console closing. See docs/smoke.md for the two schtasks lines.
#
# The decision it makes is entirely in `linklet probe`'s exit codes -- see
# crates/linklet-cli/src/probe.rs for why they are those numbers:
#
#     0  the agent answered                          -> leave it alone
#     1  nothing is listening                        -> start it
#     2  the spec could not be read                  -> the config is wrong; stop
#     4  something is listening and did not answer   -> kill it, then start it
#
# Three properties worth stating, because each of them is a way this could be worse than
# nothing:
#
#   * **It never restarts a healthy agent.** A wrong token is exit 0, because an agent that
#     says no has proved it is running. Restarting it would fix nothing and hide the problem.
#   * **A bad config stops the supervisor rather than looping.** Exit 2 means the probe could
#     not be run at all, which is this script's fault and not the agent's; looping on it would
#     fill a log with the same mistake and restart nothing useful.
#   * **It backs off.** An agent that dies instantly on startup -- a port already taken, a root
#     that does not exist -- would otherwise be started thousands of times a minute.
#
# ASCII only, and no quotes anywhere a target's `cmd` might rewrite them: see AGENTS.md
# section 8 for what a command sent to a target does to quote characters.

[CmdletBinding()]
param(
    # The JSON config. Everything else has a default that matches the agent's own.
    [Parameter(Mandatory = $true)]
    [string] $Config,

    # Where the supervisor writes what it did. Defaults to the agent's own log directory.
    [string] $Log = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The probe's codes, named so that the loop below reads as decisions rather than as numbers.
$ANSWERED = 0
$NOTHING_THERE = 1
$BAD_SPEC = 2
$NO_ANSWER = 4

# How long between probes, and how long to wait before starting an agent that just died.
$PROBE_EVERY_SECONDS = 5
$MIN_BACKOFF_SECONDS = 2
$MAX_BACKOFF_SECONDS = 60

function Write-Log {
    param([string] $Message)

    $line = '{0} {1}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Message
    Write-Host $line
    if ($script:LogPath) {
        Add-Content -LiteralPath $script:LogPath -Value $line -Encoding ascii
    }
}

# Runs the probe and returns its exit code, with what it said going to this supervisor's log.
function Get-ProbeCode {
    param([string] $Agent, [string] $Linklet)

    # `2>&1` so a refusal that went to stderr is logged too -- the point of the log is to be
    # able to read afterwards what the probe saw, and half of what it says is on stderr.
    $output = & $Linklet probe --agent $Agent 2>&1
    $code = $LASTEXITCODE
    if ($output) {
        Write-Log ('probe {0} -> {1}: {2}' -f $code, $Agent, ($output -join ' '))
    }
    return $code
}

# Starts the agent and returns the process, or $null when it could not start.
function Start-Agent {
    param([string] $Exe, [string[]] $Arguments, [string] $Log)

    $started = Start-Process -FilePath $Exe -ArgumentList $Arguments -NoNewWindow -PassThru `
        -RedirectStandardOutput $Log -RedirectStandardError ($Log + '.err')
    Write-Log ('started pid {0}: {1}' -f $started.Id, $Exe)
    return $started
}

# Stops whatever is holding the agent's port, and the process tree with it.
#
# **By port and not by name, and that is the correction running it demanded.** The first
# version killed `linklet-agent.exe` by name, which does nothing when the thing holding the
# port is not an agent -- a wedged process, or a stale listener, or an agent under a name this
# script did not guess -- and the supervisor then looped forever reporting a wedged agent it
# could not clear. The port is the one thing that is certainly the agent's, because the probe
# just proved something is on it.
#
# `/T` matters too: a linklet agent can have started a program with `spawn`, and killing only
# the listener leaves that program holding the output file -- the failure `docs/ROADMAP.md`
# records from the deploy loop, one process over.
function Stop-Holder {
    param([System.Diagnostics.Process] $Process, [string] $Exe, [int] $Port)

    if ($Process -and -not $Process.HasExited) {
        Write-Log ('stopping pid {0}' -f $Process.Id)
        Invoke-Taskkill -Arguments @('/PID', "$($Process.Id)", '/T', '/F')
        return
    }

    $holder = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue |
        Select-Object -First 1

    if ($holder) {
        Write-Log ('something is on port {0}; stopping pid {1}' -f $Port, $holder.OwningProcess)
        Invoke-Taskkill -Arguments @('/PID', "$($holder.OwningProcess)", '/T', '/F')
        return
    }

    # Nothing on the port and nothing of ours: there is nothing to stop, and saying so is
    # better than killing by name and hoping.
    Write-Log ('nothing is holding port {0} any more' -f $Port)
}

# Runs taskkill and logs what it said.
#
# **The output is labelled rather than passed through.** `taskkill` answers in the machine's
# OEM code page, so on this project's own bench its Chinese message came out as question marks
# in an ASCII log -- which reads like a corrupt file rather than like a translated message.
# Saying which bytes those are costs one word and stops the next reader guessing.
function Invoke-Taskkill {
    param([string[]] $Arguments)

    $output = & taskkill @Arguments 2>&1
    $code = $LASTEXITCODE
    foreach ($line in @($output)) {
        Write-Log ('taskkill (machine code page): {0}' -f $line)
    }
    Write-Log ('taskkill exit {0}' -f $code)
}

if (-not (Test-Path -LiteralPath $Config)) {
    Write-Host ('linklet-supervise: no config at {0}' -f $Config)
    exit $BAD_SPEC
}

$settings = Get-Content -LiteralPath $Config -Raw | ConvertFrom-Json

foreach ($required in @('Exe', 'Args', 'Agent')) {
    if (-not $settings.PSObject.Properties[$required]) {
        Write-Host ('linklet-supervise: the config has no {0}' -f $required)
        exit $BAD_SPEC
    }
}

$exe = [string] $settings.Exe
$agent = [string] $settings.Agent

# **The port, taken from the address rather than configured separately.** Two places to write
# the same number is two places to disagree, and the disagreement would look like a supervisor
# watching one port while killing whatever holds another.
$Port = 0
$lastColon = $agent.LastIndexOf(':')
if ($lastColon -lt 0 -or -not [int]::TryParse($agent.Substring($lastColon + 1), [ref] $Port)) {
    Write-Host ('linklet-supervise: the agent address {0} has no port' -f $agent)
    exit $BAD_SPEC
}

$linklet = if ($settings.PSObject.Properties['Linklet']) {
    [string] $settings.Linklet
} else {
    # Beside the agent by default, which is how both were deployed.
    Join-Path (Split-Path -Parent $exe) 'linklet.exe'
}
$agentLog = if ($settings.PSObject.Properties['AgentLog']) {
    [string] $settings.AgentLog
} else {
    Join-Path (Split-Path -Parent $exe) 'agent.out'
}

if (-not $Log) {
    $Log = Join-Path (Split-Path -Parent $exe) 'supervisor.log'
}
$script:LogPath = $Log

if (-not (Test-Path -LiteralPath $exe)) {
    Write-Host ('linklet-supervise: no agent at {0}' -f $exe)
    exit $BAD_SPEC
}
if (-not (Test-Path -LiteralPath $linklet)) {
    Write-Host ('linklet-supervise: no linklet tool at {0}' -f $linklet)
    exit $BAD_SPEC
}

Write-Log ('watching {0}, probing every {1}s' -f $agent, $PROBE_EVERY_SECONDS)

$backoff = $MIN_BACKOFF_SECONDS
$child = $null

while ($true) {
    $code = Get-ProbeCode -Agent $agent -Linklet $linklet

    switch ($code) {
        $ANSWERED {
            # Working. Nothing to do, and the backoff resets: the next death is a new event and
            # not a continuation of an old one.
            $backoff = $MIN_BACKOFF_SECONDS
            Start-Sleep -Seconds $PROBE_EVERY_SECONDS
        }
        $NOTHING_THERE {
            Write-Log ('nothing at {0}; starting it' -f $agent)
            $child = Start-Agent -Exe $exe -Arguments $settings.Args -Log $agentLog
            Start-Sleep -Seconds $backoff
            $backoff = [Math]::Min($MAX_BACKOFF_SECONDS, $backoff * 2)
        }
        $NO_ANSWER {
            Write-Log ('{0} accepted a connection and did not answer; killing it' -f $agent)
            Stop-Holder -Process $child -Exe $exe -Port $Port
            $child = $null
            Start-Sleep -Seconds $MIN_BACKOFF_SECONDS
        }
        $BAD_SPEC {
            # The probe could not be run at all. That is this script's config, and looping
            # would fill the log with the same mistake and fix nothing.
            Write-Log 'the probe could not be run; stopping so the mistake is visible'
            exit $BAD_SPEC
        }
        default {
            Write-Log ('unexpected probe code {0}; treating it as nothing there' -f $code)
            Start-Sleep -Seconds $PROBE_EVERY_SECONDS
        }
    }
}
