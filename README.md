# linklet

A small, honest tool for driving machines on a LAN, built to be called by an AI
agent rather than by a person reading a manual.

> **Status: M0-M6 done.** Five crates, 259 tests, one command that runs every
> gate. A host can check reachability, run a command on a target through a sealed
> channel, and read what it did. See `docs/ROADMAP.md` for what was parked, and
> `docs/decisions.md` for the choices that are not obvious from the code.

## What it does

```console
$ linklet check 10.0.0.5:8787,10.0.0.6:8787
live 10.0.0.5:8787 connected
dead 10.0.0.6:8787 the machine refused the connection
$ echo $?
1
```

```console
$ export LINKLET_TOKEN=$(some-secret-of-at-least-16-bytes)
$ linklet exec --agent 10.0.0.5:8787 "build.cmd --release"
exit 0
took 2411 ms
stdout:
built 3 targets
$ echo $?
0
```

`check` prints one line per target, in the order the targets were given: `live`,
`dead` or `unknown`, then the target as it was written, then the reason. `dead`
covers two situations and the reason is where they are told apart, because the
difference matters to whoever is reading:

| output | what it means |
|---|---|
| `the machine refused the connection` | the machine is up; nothing is listening on that port |
| `no answer within 5 s` | nothing came back at all -- off, filtered, or slow |

`exec` runs one command on one machine and reports its exit code, how long it
took, and what it printed. It needs an agent on the target and a shared token.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run completed, and either everything is alive or the command exited 0 |
| 1 | the run completed and something is not |
| 2 | the invocation was wrong |
| 3 | the run was refused before anything was looked at |

For `exec`, the command's own exit code is passed through when the command ran,
so `linklet exec ... && next` behaves the way the command would. A call that could
not be made gets 3, which no command can produce.

The distinction between 1 and 3 is the one an agent needs: "the machines are down"
and "the tool could not start" send it to different places, and collapsing them
into one non-zero code loses exactly the information it came for.

## The problem

Driving a remote machine during debugging means a long chain of small,
error-prone steps: work out which machines are reachable, copy a build over, start
it, notice it did not start, look at a log, kill what is left, copy the log back.
Every one of those steps is a place where a guess is made and never checked, and
the failure that results is quiet.

This tool makes each step explicit: it takes a description of targets, returns
typed answers, and refuses to answer a question it did not actually ask the
machine.

## What it does not do

A boundary is part of the design, and an unstated boundary gets crossed by
accident. This tool:

- does not install an agent on a target by itself (the first copy has to be a file
  copy; there is nothing to talk to yet)
- does not keep a database, a service, or a daemon on the host
- does not run on anything but Windows targets, until someone needs otherwise
- does not guess: when it cannot determine something, it returns `unknown` with
  the reason, never a plausible default
- **has no identities.** The token authenticates the channel and says nothing about
  *which* caller it is, so there is no per-caller revocation and no audit trail
- **has no cipher agility.** One curve, one cipher, one key derivation, chosen at
  build time
- **has never run in CI.** `tools/verify.ps1` is the entry point and it has only
  ever been run by hand, because there is no remote

## The one architectural rule

**The core is pure. The edges are thin.**

```
linklet-cli        argv in, text out, process exit code          (thin)
linklet-adapters   the network, the OS, the crypto               (thin)
linklet-core       decisions: parsing, validation, policy        (pure, no I/O)
      ^
linklet-agent      the target side     linklet-client   the host side
```

The rule pays for itself in exactly one way, and it is the one that matters:
**every decision in `core` can be tested in microseconds with no network, no temp
files, and no cleanup.** When a test needs a real machine to run, it stops being
run, and then the behaviour it covered rots.

The rule was originally written as "core depends on no crate", and that was the
wrong wording -- it is satisfied by a rule rather than a reason, so it stayed in
force past the point where its reason applied, and a SHA-256 came to be written by
hand because of it. The rule is now what it always meant: **core depends on no
crate that does I/O**, enforced by a named allowlist in
`crates/linklet-core/tests/architecture.rs` where every entry carries a sentence
saying why. `docs/decisions.md` has the whole of it.

That is also how the cryptography is arranged. `linklet-core` declares what a
sealed conversation is (`src/channel.rs`); `linklet-adapters` implements it with
`chacha20poly1305`, `hkdf`, `sha2` and `x25519-dalek`. The core still cannot open a
socket or a cipher, and the arithmetic is in crates other people have attacked.

### What the channel does

A sealed call is **a handshake and then the message it protects**, on one connection:
the hello, then the command sealed under the session it produced. The handshake cannot
protect itself -- the initiator cannot derive a key until it has the responder's public
key -- so the exchange comes first and the command follows it. The alternative, one
round trip with the command sealed under the token alone, leaves the command readable by
anyone who later learns the token, which is the wrong half to protect.

A transfer is the same beginning and then a stream: a manifest carrying the size and the
digest, then one sealed chunk per mebibyte. `docs/transfer.md` is the design and the
thirteen ways it goes wrong.

Both sides generate an X25519 key pair per handshake and **discard the private
half**, so a session recorded today cannot be read by anyone who learns the token
tomorrow. The token still authenticates the exchange: an attacker who substitutes
their own public key can complete a handshake and still cannot open a byte,
because the session they build is not the one either end built.
`crates/linklet-adapters/tests/handshake.rs` demonstrates that rather than
asserting it.

## Working on it

```sh
pwsh tools/verify.ps1           # fmt, clippy, test, rustdoc, dependency inventory
```

One entry point, because a gate nobody runs is a gate that does not exist. It runs
`cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `cargo doc` with
`-D warnings`, and a dependency inventory that prints which crate depends on what.

Read `AGENTS.md` before your first commit -- it is the rules only, one screen long.
`docs/INDEX.md` is the map of everything else.

### Where things are

| | |
|---|---|
| `crates/linklet-core/` | the decisions. 22 test files, all pure |
| `crates/linklet-adapters/` | sockets and crypto: the only place either happens |
| `crates/linklet-agent/` | the target side: one binary, binds a port, runs commands |
| `crates/linklet-client/` | the host side of the protocol |
| `crates/linklet-cli/` | argv in, text out, exit code |
| `docs/` | see `docs/INDEX.md`, which says what kind of document each one is |
