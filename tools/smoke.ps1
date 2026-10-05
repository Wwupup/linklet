#Requires -Version 7
<#
.SYNOPSIS
    The real-machine smoke test: one target, seven claims, no administration.

.DESCRIPTION
    Everything in tools/verify.ps1 runs on one machine, and that is enough for the
    layers below the network and not enough for a claim about a LAN. This is the
    other layer, and it is deliberately a *script you run* rather than part of the
    gates: it needs a machine, it takes seconds rather than milliseconds, and a gate
    that needs a machine stops being run.

    It takes an address and knows nothing else. That is the whole design. Whether
    the address is a Hyper-V guest, a machine on the desk or a colleague's laptop is
    not this script's business, and coupling it to one of those is how a smoke test
    becomes useless on the day a different one is available.

    **It needs no administrator rights.** The states that would need them -- a
    machine that is powered off, a snapshot restored to nothing -- are produced by
    whoever owns the machine, once, by hand. What this script does is observe.

.PARAMETER Target
    host:port of a machine already running linklet-agent. Required.

.PARAMETER Token
    The shared secret. Defaults to LINKLET_TOKEN. Never printed. Name this or
    TokenFile, not both.

.PARAMETER TokenFile
    A file whose first line is the shared secret. Defaults to LINKLET_TOKEN_FILE.
    This is the parameter to use on a machine deployed the way
    `integrations/README.md` describes, because the secret then never appears in a
    command line or a shell history.

.PARAMETER Linklet
    The binary under test. Defaults to target/debug/linklet.exe.

.PARAMETER KillTimeoutSeconds
    The deadline for the command that must be killed. Three seconds, so the check
    finishes quickly and still outlives nothing.

.EXAMPLE
    pwsh tools/smoke.ps1 -Target 172.18.112.20:8787

.EXAMPLE
    # The same, with the secret in a file rather than in this shell:
    pwsh tools/smoke.ps1 -Target 172.18.112.20:8787 -TokenFile C:\linklet\token.txt

.EXAMPLE
    # The case that needs no second machine at all, and cannot prove the firewall:
    pwsh tools/smoke.ps1 -Target 192.168.3.157:8787
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string] $Target,
    [string] $Token = $env:LINKLET_TOKEN,
    [string] $TokenFile = $env:LINKLET_TOKEN_FILE,
    [string] $Linklet = (Join-Path $PSScriptRoot '..' 'target' 'debug' 'linklet.exe'),
    [int] $KillTimeoutSeconds = 3
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# A port with nothing behind it, for the "the call could not be made" claim. The
# reserved port 1 on loopback, so the result is refused rather than filtered and the
# check does not wait out a timeout.
$DeadAddress = '127.0.0.1:1'

# --- reporting ---------------------------------------------------------------

$script:Results = [System.Collections.Generic.List[object]]::new()

function Report {
    param(
        [string] $Name,
        [bool] $Passed,
        [string] $Observed,
        [string] $Why = ''
    )
    $script:Results.Add([pscustomobject]@{
            Name     = $Name
            Passed   = $Passed
            Observed = $Observed
            Why      = $Why
        })

    $mark = if ($Passed) { 'PASS' } else { 'FAIL' }
    Write-Host ("  {0}  {1}" -f $mark, $Name) -ForegroundColor $(if ($Passed) { 'Green' } else { 'Red' })
    if ($Observed) { Write-Host ("        {0}" -f $Observed) -ForegroundColor DarkGray }
    if (-not $Passed -and $Why) { Write-Host ("        -> {0}" -f $Why) -ForegroundColor Yellow }
}

# --- running the binary under test -------------------------------------------

<#
Runs linklet and returns the exit code, the combined output, and how long it took.

The output is returned verbatim and never paraphrased. On a failure the raw text is
the useful part -- "it failed" is what this project exists to replace -- and a
script that summarised it would be repeating the mistake the tool is about.
#>
function Invoke-Linklet {
    param([string[]] $Arguments)

    $started = Get-Date
    $output = & $Linklet @Arguments 2>&1 | Out-String
    $took = (Get-Date) - $started

    [pscustomobject]@{
        Code   = $LASTEXITCODE
        Output = $output.Trim()
        Took   = $took
    }
}

# --- the claims --------------------------------------------------------------

Write-Host ''
Write-Host "linklet smoke: $Target" -ForegroundColor Cyan
if ($Token -and $TokenFile) {
    Write-Host '  both -Token and -TokenFile were given' -ForegroundColor Red
    Write-Host '  the secret is one thing, so name one of them: the host refuses two sources,' -ForegroundColor Red
    Write-Host '  and every claim below would fail for a reason that is not the target.' -ForegroundColor Red
    exit 2
}
if (-not ($Token -or $TokenFile)) {
    Write-Host '  no token given: neither -Token nor LINKLET_TOKEN, nor -TokenFile nor LINKLET_TOKEN_FILE' -ForegroundColor Red
    Write-Host '  a sealed call cannot be made without one, so nothing below can run.' -ForegroundColor Red
    exit 2
}

# **The token this script was given is the token it uses, from one source.** `-Token`
# was documented and never applied, so a run that passed one presented whatever
# `LINKLET_TOKEN` happened to hold instead -- a documented parameter silently losing
# to the ambient variable, which is the kind of quiet wrong answer this tool exists to
# refuse. The other source is cleared for the same reason: with both set, the host
# presents no token at all and every claim below fails for a reason that has nothing
# to do with the target.
if ($TokenFile) {
    Remove-Item Env:LINKLET_TOKEN -ErrorAction SilentlyContinue
    $env:LINKLET_TOKEN_FILE = $TokenFile
} else {
    $env:LINKLET_TOKEN = $Token
    Remove-Item Env:LINKLET_TOKEN_FILE -ErrorAction SilentlyContinue
}

# The binary has to exist before anything else means something.
if (-not (Test-Path $Linklet)) {
    Write-Host "  no binary at $Linklet" -ForegroundColor Red
    Write-Host '  run: cargo build --workspace' -ForegroundColor Red
    exit 2
}

# 1. Reachability. This is the gate: if the machine is not answering, the rest of
#    the claims are about a machine that is not there.
$reached = Invoke-Linklet @('check', $Target)
$live = $reached.Output -match '^live'
Report -Name 'the target answers on its port' -Passed $live -Observed $reached.Output `
    -Why @'
If this says "no answer", the machine is not unreachable -- its firewall is
probably dropping the connection. Loopback bypasses Windows Firewall, so no test
on one machine can find this, and the symptom looks exactly like a dead host.
On the target, allow the port:
  New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound -Protocol TCP -LocalPort <port> -Action Allow
'@

if (-not $live) {
    Write-Host ''
    Write-Host 'stopped: nothing else can be checked against a target that is not answering.' -ForegroundColor Yellow
    exit 1
}

# 2. A command runs, and its output comes back. The baseline: if this fails, every
#    claim below is measuring something else.
$ran = Invoke-Linklet @('exec', '--agent', $Target, 'echo smoke-marker')
$echoed = $ran.Code -eq 0 -and $ran.Output -match 'smoke-marker'
Report -Name 'a command runs and its output comes back' -Passed $echoed -Observed "exit $($ran.Code) in $([int]$ran.Took.TotalMilliseconds) ms" `
    -Why 'the sealed channel may not have completed: check that both sides hold the same token.'

# 3. The command's own exit code is passed through, so a script can use && on it.
$failed = Invoke-Linklet @('exec', '--agent', $Target, 'exit 7')
$passedThrough = $failed.Code -eq 7
Report -Name "a failing command's own exit code comes back" -Passed $passedThrough -Observed "exit $($failed.Code), expected 7" `
    -Why 'the code should be the command''s, not the tool''s. 3 here means the call was refused, not that the command failed.'

# 4. A call that could not be made is told apart from a command that failed. This
#    is the distinction the whole exit-code scheme exists for.
$refused = Invoke-Linklet @('exec', '--agent', $DeadAddress, 'echo never')
$toldApart = $refused.Code -eq 3
Report -Name 'a call that cannot be made exits 3, not 1' -Passed $toldApart -Observed "exit $($refused.Code), expected 3" `
    -Why 'a script has to tell "the machines are down" from "the tool could not start"; collapsing them loses the information it came for.'

# 5. A wrong token is refused, over a real network, as a refusal and not as a
#    transport error. This is the sealed channel's authentication path. The wrong
#    value is the only source while it is in force: leaving the file named as well
#    would present nothing at all, and the claim would pass for the wrong reason.
$saved = $env:LINKLET_TOKEN
$savedFile = $env:LINKLET_TOKEN_FILE
try {
    $env:LINKLET_TOKEN = 'wrong-token-0123456789'
    Remove-Item Env:LINKLET_TOKEN_FILE -ErrorAction SilentlyContinue
    $bad = Invoke-Linklet @('exec', '--agent', $Target, 'echo never')
}
finally {
    $env:LINKLET_TOKEN = $saved
    # An empty value would be restored as a variable naming no file, which the host
    # reports as an unreadable token file -- so an empty one is removed instead.
    if ($savedFile) { $env:LINKLET_TOKEN_FILE = $savedFile }
    else { Remove-Item Env:LINKLET_TOKEN_FILE -ErrorAction SilentlyContinue }
}

$wrongRefused = $bad.Code -eq 3 -and $bad.Output -notmatch 'could not reach'
Report -Name 'a wrong token is refused, not misreported as unreachable' -Passed $wrongRefused -Observed "exit $($bad.Code)" `
    -Why 'if this says "could not reach", the failure is being reported as a network problem when it is an authentication one.'

# 6. The command tree is killed at its deadline, and the reply still arrives. This
#    is the bug that was found on loopback and the highest-value claim here: on a
#    real network the failure mode is worse, because the client gives up first and
#    reports a transport failure for a command the agent would have described.
$killed = Invoke-Linklet @('exec', '--agent', $Target, '--timeout', "$KillTimeoutSeconds", 'ping', '-n', '30', '127.0.0.1')
$saysKilled = $killed.Output -match 'killed by the deadline'
# The reply has to arrive, and it has to arrive after the deadline rather than
# waiting out the ping. The allowance is generous on purpose: this is a claim about
# the mechanism, not a benchmark.
$arrived = $killed.Took.TotalSeconds -lt ($KillTimeoutSeconds + 10)
Report -Name 'a command past its deadline is killed, and the reply still arrives' `
    -Passed ($saysKilled -and $arrived) `
    -Observed "exit $($killed.Code) after $([int]$killed.Took.TotalSeconds)s" `
    -Why 'Child::kill kills the shell, not the program the shell started. If the reply never arrives, the orphan is holding the pipes again.'

# 7. Nothing survived the kill. The claim above is about the reply; this one is
#    about the machine, and they are different -- a killed shell with a live
#    grandchild passes 6 and fails this.
$left = Invoke-Linklet @('exec', '--agent', $Target, 'tasklist /fi "imagename eq PING.EXE"')
$orphan = $left.Output -match 'PING\.EXE'
Report -Name 'the killed command left nothing running' -Passed (-not $orphan) -Observed $(if ($orphan) { 'a PING.EXE survived' } else { 'nothing matching PING.EXE' }) `
    -Why 'an orphan still holds the executable, which breaks the next push over it.'

# --- the verdict -------------------------------------------------------------

$failedCount = @($script:Results | Where-Object { -not $_.Passed }).Count
Write-Host ''
if ($failedCount -eq 0) {
    Write-Host "all $($script:Results.Count) claims held against $Target" -ForegroundColor Green
    exit 0
}

Write-Host "$failedCount of $($script:Results.Count) claims failed against $Target" -ForegroundColor Red
$script:Results | Where-Object { -not $_.Passed } | ForEach-Object {
    Write-Host ''
    Write-Host "  $($_.Name)" -ForegroundColor Red
    Write-Host "    observed: $($_.Observed)"
    if ($_.Why) { Write-Host "    $($_.Why)" -ForegroundColor Yellow }
}
exit 1
