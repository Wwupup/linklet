# The real-machine smoke test

Everything in `tools/verify.ps1` runs on one machine. That is enough for the layers
below the network and not enough for a claim about a LAN. This is the other layer:
**seven claims, one target**, and it is a procedure rather than a gate.

**There is no script for it, and that is deliberate.** One used to exist
(`tools/smoke.ps1`), and it was a PowerShell program that drove the commands any caller can
already drive -- so it was a second client to keep in step with the first, and it could only
run on one of the two platforms this tool now works on. **The claims are the durable thing and
the program was not.** Make them with whatever client is to hand: a person at a terminal, or
an agent through the MCP surface.

It knows nothing about where the target came from. A Hyper-V guest, a machine on the desk, a
colleague's laptop, a WSL distribution -- the procedure takes an address. Coupling it to any
one of those is how a smoke test becomes useless on the day a different one is available.

**It needs no administrator rights**, and that is a design decision rather than an accident.
See "what it deliberately does not do" below.

## What it claims, and why each one needs a second machine

| # | the claim | why it cannot be checked on one machine |
|---|---|---|
| 1 | the target answers on its port | **This is where the firewall shows up.** Loopback bypasses Windows Firewall, so no single-machine test can find it, and the symptom -- `no answer` -- looks exactly like a dead host |
| 2 | a command runs and its output comes back | the baseline. If this fails, everything below is measuring something else |
| 3 | a failing command's own exit code comes back | `linklet exec ... && next` only works if the code is the command's |
| 4 | a call that cannot be made exits 3, not 1 | the distinction the exit-code scheme exists for |
| 5 | a wrong token is refused, not misreported as unreachable | the sealed channel's authentication path, over a real network |
| 6 | a command past its deadline is killed and **the reply still arrives** | the kill-tree bug, and on a real network the failure is worse: the client gives up first and reports a transport failure for a command the agent would have described |
| 7 | the killed command left nothing running | claim 6 is about the reply, this is about the machine, and a killed shell with a live grandchild passes 6 and fails this |

**Each claim is made by reading one line and one exit code**, which is what makes a script
unnecessary: claim 1 is `linklet check <target>` exiting 0 with `live`; claim 4 is the same
command against a port nothing is listening on, exiting 3; claim 5 is a deliberate wrong
token, exiting 3 *without* saying `could not reach`; claims 6 and 7 are one `exec` with
`--timeout 3` running a long `ping`, followed by a `ps` that finds no `PING.EXE`. Anything
that fails should be reported with its raw output and what to do about it -- a summary would
repeat the mistake this tool exists to fix.

## What the target needs

Nothing but a running `linklet-agent` and the same token on both sides.

```powershell
# On the target, once:
$env:LINKLET_TOKEN = '<at least sixteen bytes>'
mkdir C:\linklet\transfers
linklet-agent.exe --port 8787 --root C:\linklet\transfers
New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound `
    -Protocol TCP -LocalPort 8787 -Action Allow
```

The firewall rule is the step that matters and the reason this layer exists. It is
also the one step that `linklet` deliberately does not do for you: the agent cannot
open a port on a machine it has not been installed on yet.

**The secret may come from a file instead, and that is the shape to use when a
script or a client configuration would otherwise hold the value.** On the agent:
`--token-file C:\linklet\token.txt`, or `LINKLET_TOKEN_FILE` in the environment.
The host reads the same variable, so one file named on both sides is the whole
configuration. The file's first line is the secret, and a byte-order mark and the
line ending are not part of it -- which matters because `Set-Content` adds both, and
either one left in would derive a different key and arrive as "the token is missing
or wrong" at the first call. **One source, never two**: a token file and a token
together are refused at startup, because the two are two answers to one question and
whichever lost would be the one the operator believed was in force.

### The agent's own log, and where it goes

**The agent keeps a log by default**, at `logs/agent.log` in the directory its own
executable is in. `--log <file>` (or `LINKLET_LOG`) puts it somewhere else, and
`--no-log` turns it off. The default is not the working directory, which is what `--root`
defaults to: a scheduled task starts its program with the *scheduler's* working
directory, so a default that followed it would land in `System32\logs` on exactly the
unattended machines this is for.

The format is a pair, one line per request:

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

**A liveness check is not written down.** `identity` is the one request that asks for
nothing and changes nothing, and it is what a monitor calls to ask *are you alive*;
recording it turns the record into a heartbeat. That is not a theory. This project's own
bench was left running for four days with a five-second probe against it, and the numbers
are the argument for both halves of what changed:

| log | size | lines | the repeated line | everything else |
|---|---|---|---|---|
| `supervisor.log` | 1.6 MB | 18,815 | 18,786 (**99.85%**), one sentence saying the agent was fine | 29 |
| `agent.log` | 907 KB | 37,781 | 37,596 `identity` (**99.5%**) | 185, which is all the work |

The second row is the one that mattered. The log is read by finding a `->` with no `<-`,
and that is not findable inside eighteen thousand lines of the agent saying it is fine.
So the log holds requests that *do* something, and a monitor may ask as often as it likes.

**The file is bounded.** It appends and never truncates -- restarting an agent must not
destroy the record of what it was asked before it died -- so it rolls over instead: at a
mebibyte it becomes `agent.log.1`, the older files shift up, and the fourth is removed.
At most four mebibytes, whatever the machine does. **A roll-over never splits a pair**:
it happens before a `->` line and only when nothing is in flight, because two halves in
different files would report a finished request as one that never finished.

The command line and the file paths are **not** in the log. A log on someone else's
machine outlives the reason it was written, and a command line is where a secret gets
left by accident.

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
"C:\linklet\bin\linklet-agent.exe" --port 8787 --root C:\linklet\transfers > C:\linklet\banner.txt 2>&1
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
**documented way to start the agent**, not a supervisor: `schtasks` will not restart it if
it dies, and it will not notice one that has wedged. See the next section.

Three things about that block, all of them learned by doing it on a real machine:

- **The script's directory must have no space in it.** The command inside is quoted and
  `cmd` handles that; what breaks is anything that goes on to build another command line
  out of it -- the same trap `docs/machine.md` describes for a command sent to a
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

### Keeping it running: the caller does it, not a script

**There was a supervisor, and it was removed.** `tools/linklet-supervise.ps1` watched the
agent with `linklet probe` and restarted it: on death, and on wedged. It was 260 lines of
PowerShell standing in for a service, and it had the fault that shape always has -- **it was
not a service either**, so nothing watched *it*, and a dead supervisor stopped restarting a
dead agent. It was also the last Windows-only moving part of a tool that now works on two
platforms.

**What is left is the platform's own answer plus one command.** For bare survival, a task
that repeats is the scheduler's job rather than a script's:

```powershell
# Re-run the start script every minute; the agent fails to bind if it is already up,
# which is a cheap no-op and the scheduler's normal way to keep something alive.
schtasks /Create /TN linklet-agent /SC MINUTE /MO 1 /TR C:\linklet\start-agent.cmd /F
```

That covers *died*. It does not cover *wedged*, and no scheduler can: a wedged agent keeps
its listening socket open, so anything built on "is the port open" calls it healthy. That
question needs a conversation, which is `linklet probe`:

| code | meaning |
|------|---------|
| 0 | the agent answered |
| 1 | nothing is listening |
| 2 | the spec could not be read |
| 4 | something is listening and did not answer |

**`probe` is not `check`, and the difference is the point.** `check` opens a connection and
closes it; a process wedged on a lock still has its listening socket open, so the kernel
keeps accepting into the backlog and `check` calls it `live`. This was measured against a
real socket that accepted and never answered: **`check` said `live 127.0.0.1:8821
connected` and `probe` said `no answer ... no reply within 2000 ms`, exit 4.** The third
reason the command exists at all is that a supervisor used to read it, and it is now the
caller: **the thing that drives this tool is the thing that notices**, which for an agent is
step 2 of `integrations/skills/linklet/SKILL.md`.

**A wrong token is exit 0, not exit 4.** The question is liveness and not authorization: an
agent that answers "no" has answered, so it is running. A caller that restarted on a wrong
token would restart a healthy process in a loop while hiding the real problem, which is the
operator's secret.

### Three things about starting it by hand

All of them learned by doing it on a real machine:

- **The root has to exist.** The agent refuses to start if `--root` is not a directory,
  rather than starting and failing every transfer later with a filesystem error naming a
  path nobody typed. `--root` also defaults to the directory the agent was started in,
  which is why it is worth passing explicitly on a target.
- **An explicit `--log` is checked the same way, and for the same reason.** An operator who
  asked for a log and silently did not get one has a machine whose evidence they believe
  exists and does not -- which is the mistake this whole feature is a reaction to. A log
  path that is a directory, or cannot be opened, is a refusal to start with exit 2. The
  *default* location failing is only a warning, because nobody asked for it: refusing to
  start over a housekeeping directory would trade a working machine for a convenience.
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

Then make the seven claims against that address.

**On Linux the same procedure works**, with two differences and one thing that does not:
the firewall rule is `ufw`, `firewalld` or nothing, the agent needs no token file
permissions beyond the usual, and `discover` is Windows-only because it parses `ipconfig`
-- see `docs/ROADMAP.md` M11 for what is portable and what is not.

## What it deliberately does not do

**It does not manage virtual machines, and it does not ask for the rights to.**
The states that would need them are produced by whoever owns the machine:

| the state | who produces it |
|---|---|
| a machine that is powered off | you stop it, then make claim 1 and watch it say `no answer` rather than `refused` |
| a machine with nothing on it | you restore the `clean` snapshot |
| a port that is filtered rather than refused | **it can be done from inside** -- see below |

The third one is worth explaining, because the first version of this said the
opposite. Adding a drop rule needs no host privileges: it is a rule *inside* the
guest, and `linklet exec` is a thing that runs commands inside the guest. So:

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
- **A large transfer over a real link.** `push` and `pull` are tested against a real
  filesystem over a real socket -- `crates/linklet-adapters/tests/transfer.rs` for the bytes
  and the `.part`, `crates/linklet-client/tests/against_agent.rs` for the whole chain
  with a real agent -- and the deploy loop has been driven across a real LAN by hand. What
  is not covered is a **large** file crossing a **slow** link, which is a claim about
  timeouts rather than about correctness.
- **Anything about different Windows versions.** One target is one data point.

## Making the claims against one machine

The same procedure against this machine's own LAN address works, and it exercises the real
network stack, the `0.0.0.0` bind and the sealed channel over a non-loopback address.

**It does not exercise the firewall**, and this is the reason the layer cannot be
faked: traffic from a machine to its own address never reaches the network adapter,
so Windows Firewall is not consulted. Claim 1 passes either way, so a run against one
machine must not be read as more than it is.

**"No answer" has a second cause, and it is not the network.** A program-scoped allow
rule stops matching when the process is gone, so an agent that has **died** leaves a port
that is silently dropped rather than refused -- the same symptom as a firewall that was
never opened. That is how it presented on the first real target: the agent had died with
its console, `netstat` showed nothing listening, and `linklet check` said "no answer within
5 s" for a machine that was up and reachable. Look at the process before the firewall:
`tasklist | findstr linklet-agent` first, `netsh advfirewall` second.

**The agent leaves a record, so look at it third.** An agent that died mid-request leaves a
`->` with no `<-`, and that line names the request it died on. An agent that died between
requests leaves pairs, and the last one is simply the last thing it did. It is at
`logs/agent.log` beside the agent's executable unless `--log` said otherwise, and where
that is outside the transfer root, `linklet pull` cannot reach it -- read it on the machine
another way, or start the agent with `--log` inside the root.
