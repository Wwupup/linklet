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
linklet-agent.exe --port 8787 --root C:\linklet\transfers
New-NetFirewallRule -DisplayName linklet-agent -Direction Inbound `
    -Protocol TCP -LocalPort 8787 -Action Allow
```

The firewall rule is the step that matters and the reason this layer exists. It is
also the one step that `linklet` deliberately does not do for you: the agent cannot
open a port on a machine it has not been installed on yet.

Three things about that block, all of them learned by doing it on a real machine:

- **The root has to exist.** The agent refuses to start if `--root` is not a directory,
  rather than starting and failing every transfer later with a filesystem error naming a
  path nobody typed. `--root` also defaults to the directory the agent was started in,
  which is why it is worth passing explicitly on a target.
- **A program rule is the one that keeps working.** `-Program <the agent's path>` (any
  port) survives a change of `--port`; a rule for one port does not. The port form above
  is what the first version of this documented, and both are fine.
- **8787 is a common choice and can already be taken.** On the first target this ran
  against, a `lanlink` agent held 8787 and its own program rule, so `linklet-agent` could
  not bind it and exited — while `linklet check` still reported the port live, because
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
