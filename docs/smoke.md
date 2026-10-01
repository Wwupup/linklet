# The real-machine smoke test

Everything in `tools/verify.ps1` runs on one machine. That is enough for the layers
below the network and not enough for a claim about a LAN. This is the other layer:
seven claims, one target, and it is a script you run rather than a gate.

```console
$ pwsh tools/smoke.ps1 -Target 172.18.112.20:8787
```

It knows nothing about where the target came from. A Hyper-V guest, a machine on
the desk, a colleague's laptop -- the script takes an address. Coupling it to any
one of those is how a smoke test becomes useless on the day a different one is
available.

**It needs no administrator rights**, and that is a design decision rather than an
accident. See "what it deliberately does not do" below.

## What it claims, and why each one is worth a script

| # | the claim | why it cannot be checked on one machine |
|---|---|---|
| 1 | the target answers on its port | **This is where the firewall shows up.** Loopback bypasses Windows Firewall, so no single-machine test can find it, and the symptom -- `no answer` -- looks exactly like a dead host |
| 2 | a command runs and its output comes back | the baseline. If this fails, everything below is measuring something else |
| 3 | a failing command's own exit code comes back | `linklet exec ... && next` only works if the code is the command's |
| 4 | a call that cannot be made exits 3, not 1 | the distinction the exit-code scheme exists for |
| 5 | a wrong token is refused, not misreported as unreachable | the sealed channel's authentication path, over a real network |
| 6 | a command past its deadline is killed and **the reply still arrives** | the kill-tree bug, and on a real network the failure is worse: the client gives up first and reports a transport failure for a command the agent would have described |
| 7 | the killed command left nothing running | claim 6 is about the reply, this is about the machine, and a killed shell with a live grandchild passes 6 and fails this |

Every failure prints the raw output and a sentence about what to do. A script that
summarised the output would be repeating the mistake this tool exists to fix.

## What the target needs

Nothing but a running `linklet-agent` and the same token on both sides.

```powershell
# On the target, once:
$env:LINKLET_TOKEN = '<at least sixteen bytes>'
mkdir C:\linklet\transfers
linklet-agent.exe --port 8787 --root C:\linklet\transfers --log C:\linklet\agent.log
New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound `
    -Protocol TCP -LocalPort 8787 -Action Allow
```

The firewall rule is the step that matters and the reason this layer exists. It is
also the one step that `linklet` deliberately does not do for you: the agent cannot
open a port on a machine it has not been installed on yet.

### The agent's own log, and why `--log` exists

`--log <file>` appends one line per request to that file, and the format is a pair:

```
-> #000001 run
<- #000001 run ok 2411 ms
```

`->` is a request taken, `<-` is a request answered, the number ties the two, and the
reason on a refusal is quoted at the end of the completion line. **The absence of the
second line is the evidence**: a request that wedged the agent, or that was in flight
when it died, leaves a `->` and nothing else, which is what names it. A log that wrote
one line per request could not do that -- a request that never finished would write
nothing, and would look exactly like a request that never arrived. That is what the
first real target left: an agent that had answered calls all afternoon and not one
record of what it had been asked.

The command line and the file paths are **not** in the log. A log on someone else's
machine outlives the reason it was written, and a command line is where a secret gets
left by accident.

**A shell redirect is not a logging strategy, and this is measured rather than
assumed.** Starting the agent as `cmd /c linklet-agent.exe ... > agent.log` leaves that
file **empty** -- the redirect captures nothing from the child -- so an empty
`agent.log` is evidence about the launcher and not about the agent. `AGENTS.md` section
8 has the measurement and the second time it was confirmed.

### Starting it so it does not die with the console

**A process started from a console dies when that console closes**, and that is the
failure M10 opens with: on the first real target the agent was gone with no process left
to ask, while the port stayed silently dropped rather than refused -- which reads exactly
like a firewall. Starting it from a console is therefore only for a quick check. For a
machine you intend to keep, have the scheduler start it:

```powershell
# On the target, once, in a directory with no space in its name -- see below.
@'
@echo off
set LINKLET_TOKEN=<the secret>
"C:\linklet\bin\linklet-agent.exe" --port 8787 --root C:\linklet\transfers --log C:\linklet\agent.log > C:\linklet\banner.txt 2>&1
'@ | Set-Content C:\linklet\start-agent.cmd -Encoding ascii

schtasks /Create /TN linklet-agent /SC ONCE /ST 00:00 /TR C:\linklet\start-agent.cmd /F
schtasks /Run /TN linklet-agent
```

**Why a script file and not the command line.** `/TR` refuses anything over **261
characters**, and a command that sets the token, names the binary, the root and the log
and redirects two streams is past that on any real path. The script also keeps the
**token out of the scheduler's record**, which is the same argument that keeps it out of
`argv` everywhere else in this project.

**Why this survives, and what it is not.** A task launched this way belongs to the
scheduler, not to your console, so closing the window does not touch it. It is a
**documented way to start the agent**, not a supervisor: `schtasks` will not restart it
if it dies, and it will not notice one that has wedged. `--detach` on the agent was
considered and **refused**: making it real needs Windows' `DETACHED_PROCESS` creation
flag, which `std` does not expose safely, and buying it with `unsafe` or a Win32
dependency in the smallest binary in this repository is a poor trade for something the
scheduler already does. See `docs/ROADMAP.md` M10.

Three things about that block, all of them learned by doing it on a real machine:

- **The script's directory must have no space in it.** The command inside is quoted and
  `cmd` handles that; what breaks is anything that goes on to build another command line
  out of it -- the same trap `AGENTS.md` section 8 describes for a command sent to a
  target. `C:\linklet` is the choice for that reason.
- **`schtasks /ST` wants a time and warns if it is in the past.** The warning is noise
  for a task that is only ever started by hand with `/Run`.
- **The banner goes to its own file because a shell redirect started *through another
  process* captures nothing.** That is measured, and it is why `--log` exists at all: with
  `--log` the agent writes its own evidence, and the redirect only has to survive long
  enough to say whether it started.

To stop it: `schtasks /End /TN linklet-agent` ends the task, and the agent's own process
then has to be killed by name or pid -- `/End` ends the task, not the process the script
started. `schtasks /Delete /TN linklet-agent /F` removes the task itself.

### Supervising it, so a dead or wedged one comes back

**`schtasks` starts an agent and does nothing else with it.** It will not restart a process
that died, and it cannot tell a wedged one from a busy one -- which is the gap
`docs/ROADMAP.md` M10 names as having no counterpart here. `tools/linklet-supervise.ps1` is
that counterpart. It runs in the foreground, is meant to be started by the scheduler, and
watches the agent with `linklet probe`.

**The whole of its decision is `probe`'s four exit codes**, and they exist so a script can act
without reading a word:

| code | meaning | what the supervisor does |
|------|---------|--------------------------|
| 0 | the agent answered | nothing |
| 1 | nothing is listening | start it |
| 2 | the spec could not be read | stop, because the config is wrong |
| 4 | something is listening and did not answer | kill whatever holds the port, then start it |

**`probe` is not `check`, and the difference is the point.** `check` opens a connection and
closes it; a process wedged on a lock still has its listening socket open, so the kernel keeps
accepting into the backlog and `check` calls it `live`. This was measured against a real
socket that accepted and never answered: **`check` said `live 127.0.0.1:8821 connected` and
`probe` said `no answer ... no reply within 2000 ms`, exit 4.** A supervisor built on `check`
would watch that process forever.

**A wrong token is exit 0, not exit 4.** The question is liveness and not authorization: an
agent that answers "no" has answered, so it is running, and a supervisor that restarted it
would restart a healthy process in a loop while hiding the real problem, which is the
operator's secret.

Set it up on the target, next to the agent:

```powershell
@'
{
  "Exe": "C:\\linklet\\bin\\linklet-agent.exe",
  "Args": ["--port", "8790", "--root", "C:\\linklet\\transfers", "--log", "C:\\linklet\\agent.log"],
  "Agent": "127.0.0.1:8790",
  "Linklet": "C:\\linklet\\bin\\linklet.exe"
}
'@ | Set-Content C:\linklet\supervisor.json -Encoding ascii

schtasks /Create /TN linklet-supervisor /SC ONCE /ST 00:00 /TR C:\linklet\start-supervisor.cmd /F
schtasks /Run /TN linklet-supervisor
```

```bat
@echo off
set LINKLET_TOKEN=<the secret>
powershell -NoProfile -ExecutionPolicy Bypass -File C:\linklet\linklet-supervise.ps1 -Config C:\linklet\supervisor.json -Log C:\linklet\supervisor.log
```

Six things about that, five of them measured on a real machine:

- **`powershell`, not `pwsh`.** Windows PowerShell 5.1 is on every Windows machine and
  PowerShell 7 is not -- the bench this was verified on has no `pwsh` at all, and the first
  version of this block used it, so the task started, exited, and left no log and no agent.
  The script is written for 5.1 and runs there.
- **The supervisor must have the token in its environment**, because the probe completes a
  handshake and the handshake needs it. Without one every probe reports "a sealed call needs a
  token" and the supervisor restarts a perfectly healthy agent forever. Its own log shows this
  within one cycle, which is why the log exists.
- **It kills by port, not by name.** The first version killed `linklet-agent.exe` by name,
  which does nothing when the thing holding the port is not an agent -- it then looped forever
  reporting a wedged agent it could not clear. `Get-NetTCPConnection` finds the owner of the
  port the probe just proved is occupied.
- **A wedged agent that is not an agent at all still gets cleared.** Verified by putting a
  plain socket that accepts and never answers on the agent's port: the supervisor killed it and
  replaced it with a real agent.
- **`taskkill` answers in the machine's code page**, so its message is logged with the label
  `(machine code page)` -- on this project's bench it is Chinese and reads as `??` in an ASCII
  log, which looks like corruption rather than like a translated message.
- **Backoff is capped at 60 seconds.** An agent that dies instantly on startup -- a port
  already taken, a root that does not exist -- would otherwise be started thousands of times a
  minute.

**What this still is not.** It is not a service: it is a process the scheduler starts, and
nothing watches *it*. If the supervisor dies, the agent it started keeps running and nothing
restarts the supervisor. That is one level less bad than the gap M10 opened with -- a dead
agent no longer goes unnoticed -- and it is named here rather than implied.

**Both recoveries were verified on the real bench** (`192.168.100.2`, agent on 8790, the
supervisor started by `schtasks`). The supervisor's own log is the evidence, and its times are
the machine's:

*Death.* The agent was killed outright from this side with the supervisor untouched:

```text
04:24:48 probe 0 -> 127.0.0.1:8790: answered 127.0.0.1:8790: linklet-agent
04:24:55 probe 1 -> 127.0.0.1:8790: nothing listening at 127.0.0.1:8790
04:24:55 nothing at 127.0.0.1:8790; starting it
04:24:55 started pid 4044: C:\linklet\bin\linklet-agent.exe
04:24:57 probe 0 -> 127.0.0.1:8790: answered 127.0.0.1:8790: linklet-agent
```

*Wedged.* A plain Python socket that accepted and never answered was put on the agent's port,
so that the thing holding it was **not an agent at all** -- the case the first version of the
script could not clear, because it killed by name:

```text
04:25:31 probe 0 -> 127.0.0.1:8790: answered 127.0.0.1:8790: linklet-agent
04:25:38 probe 4 -> 127.0.0.1:8790: no answer from 127.0.0.1:8790: no reply within 2000 ms
04:25:38 127.0.0.1:8790 accepted a connection and did not answer; killing it
04:25:38 something is on port 8790; stopping pid 11508
04:25:38 taskkill (machine code page): SUCCESS: The process with PID 11508 (child process
         of PID 7504) has been terminated.
04:25:42 probe 1 -> 127.0.0.1:8790: nothing listening at 127.0.0.1:8790
04:25:42 nothing at 127.0.0.1:8790; starting it
04:25:42 started pid 6880: C:\linklet\bin\linklet-agent.exe
```

Across the link, that same wedged socket is what `check` calls healthy: it reported
`live 192.168.100.2:8790 connected` while the probe reported exit 4. After the supervisor
cleared it, `linklet exec --agent 192.168.100.2:8790 "echo recovered"` answered on the first
try.

### Three things about starting it by hand

All of them learned by doing it on a real machine:

- **The root has to exist.** The agent refuses to start if `--root` is not a directory,
  rather than starting and failing every transfer later with a filesystem error naming a
  path nobody typed. `--root` also defaults to the directory the agent was started in,
  which is why it is worth passing explicitly on a target.
- **`--log` is checked the same way, and for the same reason.** An operator who asked for
  a log and silently did not get one has a machine whose evidence they believe exists and
  does not -- which is the mistake this whole feature is a reaction to. A log path that is
  a directory, or cannot be opened, is a refusal to start with exit 2.
- **A program rule is the one that keeps working.** `-Program <the agent's path>` (any
  port) survives a change of `--port`; a rule for one port does not. The port form above
  is what the first version of this documented, and both are fine.
- **8787 is a common choice and can already be taken.** On the first target this ran
  against, a `lanlink` agent held 8787 and its own program rule, so `linklet-agent` could
  not bind it and exited -- while `linklet check` still reported the port live, because
  something was listening. If the first claim passes and the second fails, look at which
  process owns the port before looking at the network.

## Setting up a target once, by hand

Doing this once is enough for every run afterwards, and doing it by hand the first
time is part of the point -- the manual pass is what shows you which steps a
deployment actually has.

1. **Make the machine.** A Hyper-V guest works: generation 2, 4 GB, 40 GB, Windows
   installed. It needs no Hyper-V rights from anyone but you, once.
2. **Give it a stable address.** On the Hyper-V Default Switch the subnet is
   `172.18.112.0/20` and the host is `172.18.112.1`. Set a static address inside
   the guest rather than reading a DHCP lease each time -- and note that the Default
   Switch's subnet can change across host reboots, so check it rather than
   assuming.
3. **Install the agent and allow the port**, as above.
4. **Take a snapshot called `clean`.** Restoring it is how you get "a machine with
   nothing on it", which is the state a bootstrap test starts from.

Then: `pwsh tools/smoke.ps1 -Target <that address>`.

## What it deliberately does not do

**It does not manage virtual machines, and it does not ask for the rights to.**
The states that would need them are produced by whoever owns the machine:

| the state | who produces it |
|---|---|
| a machine that is powered off | you stop it, then run `smoke.ps1` and watch claim 1 say `no answer` rather than `refused` |
| a machine with nothing on it | you restore the `clean` snapshot |
| a port that is filtered rather than refused | **the script can do this itself** -- see below |

The third one is worth explaining, because the first version of this said the
opposite. Adding a drop rule needs no host privileges: it is a rule *inside the
guest*, and `linklet exec` is a thing that runs commands inside the guest. So:

> **Do not add the rule and leave it -- you will lock yourself out of the machine
> you were testing.** Spawn a detached script inside the guest that adds the rule,
> waits, and removes it. The removal has to be guaranteed by the guest and not by
> anything you intend to do afterwards.

The right answer to "how do I get privileges to configure Hyper-V" turned out to be
"you do not need them". Hyper-V Administrators is not meaningfully least-privilege
anyway -- a member can mount another machine's disk and read it as SYSTEM -- so
granting it for convenience would have bought a real capability for a need that
turned out not to exist.

## What it still cannot verify

- **A machine that is powered off**, unless you stop it. Claim 1 covers the
  observation; producing the state is manual by design.
- **Two machines at once.** The concurrency claim (twenty unreachable targets in
  one budget) is tested against fakes in `tests/concurrent_check.rs`. A real test
  would need twenty real addresses, and the claim is about arithmetic rather than
  about the network.
- **A transfer.** `push` and `pull` are tested against a real filesystem over a real
  socket on one machine -- `crates/linklet-adapters/tests/transfer.rs` for the bytes
  and the `.part`, `crates/linklet-client/tests/against_agent.rs` for the whole chain
  with a real agent. What none of that shows is a **large** file crossing a **real
  link**, which is the one thing a second machine would add and the one thing this
  script does not yet claim. It is named here rather than left to be assumed from the
  absence of a failure.
- **Anything about different Windows versions.** One target is one data point.

## Running it against one machine

`pwsh tools/smoke.ps1 -Target <this machine's LAN address>:<port>` works, and it
exercises the real network stack, the `0.0.0.0` bind and the sealed channel over a
non-loopback address.

**It does not exercise the firewall**, and this is the reason the layer cannot be
faked: traffic from a machine to its own address never reaches the network adapter,
so Windows Firewall is not consulted. Claim 1 passes either way. The comment at the
top of the script says so, so that a green run against one machine is not read as
more than it is.

**"No answer" has a second cause, and it is not the network.** A program-scoped allow
rule stops matching when the process is gone, so an agent that has **died** leaves a port
that is silently dropped rather than refused -- the same symptom as a firewall that was
never opened. That is how it presented on the first real target: the agent had died with
its console, `netstat` showed nothing listening, and `linklet check` said "no answer within
5 s" for a machine that was up and reachable. Look at the process before the firewall:
`tasklist | findstr linklet-agent` first, `netsh advfirewall` second.

**The agent now leaves a record, so look at it third.** With `--log`, an agent that died
mid-request leaves a `->` with no `<-`, and that line names the request it died on. An
agent that died between requests leaves pairs, and the last one is simply the last thing it
did. Read it with `linklet pull --agent <address> --from <the log's path> --to <local>` if
it is inside the agent's root, or read it on the machine another way if it is not.
