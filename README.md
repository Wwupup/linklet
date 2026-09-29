# linklet

A small, honest tool for checking machines on a LAN, built to be called by an AI
agent rather than by a person reading a manual.

> **Status: milestone M3.** One command works end to end: it parses a list of
> targets, connects to each, and reports what it observed. Deploying a build,
> reading logs and killing processes are M4 and beyond -- see `docs/ROADMAP.md`.

## What it does

```console
$ linklet check 10.0.0.5:8787,10.0.0.6:8787
live 10.0.0.5:8787 connected
dead 10.0.0.6:8787 the machine refused the connection
$ echo $?
1
```

One line per target, in the order the targets were given. `live`, `dead` or
`unknown`, then the target as it was written, then the reason.

`dead` covers two different situations, and the reason is where they are told
apart, because the difference matters to whoever is reading:

| output | what it means |
|---|---|
| `the machine refused the connection` | the machine is up and reachable; nothing is listening on that port |
| `no answer within 5 s` | nothing came back at all -- off, filtered, or slow |

### Exit codes

| code | meaning |
|---|---|
| 0 | the run completed and everything asked about is alive |
| 1 | the run completed and something is not |
| 2 | the invocation was wrong |
| 3 | the run was refused before anything was looked at |

The distinction between 1 and 3 is the one an agent needs: "the machines are
down" and "the tool could not start" send it to different places, and collapsing
them into one non-zero code loses exactly the information it came for.

## The problem

Driving a remote machine during debugging means a long chain of small,
error-prone steps: work out which machines are reachable, copy a build over,
start it, notice it did not start, look at a log, kill what is left, copy the
log back. Every one of those steps is a place where a guess is made and never
checked, and the failure that results is quiet.

This tool makes each step explicit: it takes a description of targets, returns
typed answers, and refuses to answer a question it did not actually ask the
machine.

## What it does not do

A boundary is part of the design, and an unstated boundary gets crossed by
accident. This tool:

- does not install an agent on a target by itself (the first copy has to be a
  file copy; there is nothing to talk to yet)
- does not keep a database, a service, or a daemon on the host
- does not run on anything but Windows targets, until someone needs otherwise
- does not guess: when it cannot determine something, it returns `unknown` with
  the reason, never a plausible default

## The one architectural rule

**The core is pure. The edges are thin.**

```
linklet-cli        argv in, text out, process exit code          (thin)
      |
linklet-adapters   the network, the OS, the filesystem           (thin)
      |
linklet-core       decisions: parsing, validation, policy        (pure, no I/O)
```

The rule pays for itself in exactly one way, and it is the one that matters:
**every decision in `core` can be tested in microseconds with no network, no
temp files, and no cleanup.** When a test needs a real machine to run, it stops
being run, and then the behaviour it covered rots.

It is worth being concrete about what that buys. The core's 47 behavioural tests
-- parsing, reachability policy, the output format, the exit codes -- run in
**under 0.1 seconds** and cover timeouts without anything timing out. (Two more
tests guard the layer rule itself.) The seven tests that need a real socket live
in `linklet-adapters` and take two seconds, because Windows takes about two
seconds to report a refused connection. Those two seconds are the entire reason
the trait is declared in the core.

This rule is enforced by the compiler, not by review: `linklet-core` has no
dependencies, so it *cannot* open a socket or read a file.

## Working on it

```sh
cargo test --workspace          # the only command you need to start
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Read `AGENTS.md` before your first commit -- it is the rules only, one screen
long. `docs/INDEX.md` is the map of everything else.
