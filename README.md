# linklet

A small, honest tool for checking and driving machines on a LAN, built to be
called by an AI agent rather than by a person reading a manual.

> **Status: skeleton.** Nothing is implemented yet. The types in
> `linklet-core` are declared and the tests that define their behaviour are
> written and currently failing. That is the intended starting state; see
> `docs/ROADMAP.md`.

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
- does not guess: when it cannot determine something, it returns "unknown"
  with the reason, never a plausible default

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

This rule is enforced by the compiler, not by review: `linklet-core` has no
dependencies, so it *cannot* open a socket or read a file.

## Working on it

```sh
cargo test --workspace          # the only command you need to start
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Read `AGENTS.md` before your first commit. `docs/ROADMAP.md` says where the
next piece of work is, and `docs/LEARNING.md` says how to do it.
