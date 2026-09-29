# Roadmap

Working rules for this list:

- **Each milestone ends in something runnable and green.** No milestone leaves
  the repository in a state where `cargo test --workspace` fails for a reason
  other than "the next milestone is not written yet".
- **Each one is testable without a network and without a second machine**,
  except where it is explicitly about the network -- and then the part that
  decides is still tested without one.
- **The order is a dependency order, not a preference order.** Skipping ahead
  produces exactly the kind of code this project exists to avoid.

## M0 -- the skeleton

- [x] workspace, three crates, the compiler-enforced layer rule
- [x] `.gitignore`, `AGENTS.md`, `rust-toolchain.toml`, this file
- [x] the core types declared, with the tests that define their behaviour
- [x] everything else: `cargo test` failed on `unimplemented!`, on purpose

## M1 -- the first decision, end to end

The smallest complete thing that does something true.

- [x] `parse_targets`: the text grammar in `linklet-core/src/target.rs`
- [x] `cargo test -p linklet-core` green: 19 specification tests, 2 architecture
      guards, 0.10 s warm
- [x] the CLI prints the parsed targets, and exits 2 on a bad spec -- finished
      in M3, where the whole surface was built at once rather than in halves.
      Recorded here rather than deleted, because a checked-off item that was
      quietly dropped is how a roadmap becomes fiction.

*What you learn here:* writing a test before the code, making an invalid state
unrepresentable (`Port` cannot hold 0), and the difference between a validation
error and a guess.

## M2 -- the first I/O, and why it does not leak into the core

- [x] `linklet-core` defines the trait it needs: "tell me whether this target
      answers, within this deadline" -- `Probe`, in `linklet-core/src/probe.rs`
- [x] `linklet-adapters` implements it with a real TCP connect and a deadline
- [x] the core keeps a *policy* on top of it and that policy is tested with a
      fake in-memory implementation -- no network in the test

*What you learn here:* dependency inversion, and why the trait belongs to the
core rather than to the adapter. This is the single most valuable idea in the
whole roadmap.

**What actually happened, which is more useful than the plan was.** Three rounds
of measurement were needed before the adapter could be written correctly, and
none of it was guesswork:

1. On Windows a refused loopback connection takes about 2.05 seconds to surface
   (measured ten times; a listening port answers in 137 microseconds). Under a
   shorter budget `connect_timeout` invents a timeout rather than reporting the
   refusal -- so a program that is simply not running would have been reported as
   a machine that did not answer.
2. An empty host is not nonsense: `("", 80)` resolves to nine local addresses on
   this machine. A test written on the opposite assumption was testing the
   multi-address path while claiming to test resolution failure.
3. Consequently the probe needs two words where one was planned: a timeout is a
   verdict the OS reached, and no answer is the absence of one.

None of that is visible from the plan, and all of it is the kind of thing that
would have shipped as a wrong answer. The tables are in
`linklet-adapters/src/tcp.rs`.

## M3 -- a command line an agent can use

- [x] a real `check` command, output that is stable and parseable
- [x] exit codes and error messages that an agent can act on without reading
      documentation twice
- [x] every message the tool prints is either a fact it observed or an error it
      can quote -- no reassurance, no summary, no invented noun

*What you learn here:* the interface is the product, and every sentence printed
is part of it.

The format and the exit codes live in `linklet-core/src/outcome.rs`, not in
`main`, because they are the published interface: an agent branches on them, so
they should be pinned by tests instead of by a comment in the one file nobody
tests.

## M4 -- one call instead of five

- [x] concurrency across targets, with per-target timeouts
- [x] a single invocation that reports on many machines
- [x] the decision of what to do about partial failure, written down and tested

*What you learn here:* where concurrency belongs (the adapter) and where it
must not live (the decision).

**What actually happened, and what it says about the plan.** The third item
needed no decision at all. Partial failure had already been decided in M2: one
machine being down is an ordinary result carrying bad news, not a failure of the
run. Concurrency did not make it a special case, and the only work it needed was
a test asserting that the concurrent run agrees with the serial one about it.

That is the useful shape of this milestone. Two of the three items were about
*when* the waiting happens; the third was already answered. "We will decide that
later" sometimes means "it was decided already and nobody wrote it down".

The measurement: 20 unreachable targets on a 5 s budget finish in 5.0 s, against
roughly 40 s serial. `docs/testing.md` records which layer each of these tests
belongs in, and why the overlap itself cannot be tested in the core.

## M5 -- being useful from an agent, and being honest about it

- [x] the MCP surface, deliberately **few and coarse**: one call per intent,
      not one call per endpoint
- [x] the whole tool description fits in a paragraph, without caveats
- [x] a check on the process itself: hand the tool list to a model that has not
      seen this repository and see whether it can pick the right call

*What you learn here:* why a tool description that needs a manual is a symptom
of a bad interface, which is where this project came from.

**What the experiment found.** Run and recorded in
`docs/tool-readability.md`, with its method, its result and two flaws in how it
was run. The short version: four of five readers refused to fabricate rather
than guessing an argument, and every refusal named the missing thing. That was
not the property the experiment was designed to test, and it is the more valuable
one -- a plausible guess that looks like progress is how an agent causes damage,
and this surface did not produce one.

The failure is narrower and more interesting than the success. One reader,
asked to confirm two preconditions before a test, reached for `exec` and composed
`dir /b *.exe & tasklist ...`, which answers the question just as well. Nothing in
`testbed`'s description says why it beats typing the equivalent command, and
saying so is a sentence about *when* to use a tool -- which is the kind of
sentence the M5 rules exist to keep out. So the open question is not how to
describe that tool better; it is whether a surface should have fewer tools rather
than better-described ones.

**What actually happened.** The surface is one tool, `check`, with a
sixty-character description. Three rules keep it there, and they are enforced
rather than remembered: `tests/tool_surface.rs` asserts the tool count, forbids a
description from naming another tool, and gives every description a
120-character budget. The reference point being guarded against had 13,758
characters across seventeen tools, and no single step in it was ever wrong.

Three capabilities that were planned here are **absent rather than stubbed**:
`exec`, `logs` and file transfer. All three need something on the far side to
talk to and there is nothing there yet, and rule 3 in `tool.rs` says a tool that
answers "not implemented" is worse than a missing one. They arrive with the agent
that serves them, which is the next thing.

---

## M6 -- who may run commands, and who may read them

**Done, and it was not on this roadmap when the plan was written.** The parked list
said "no security boundary beyond the caller can already reach the machine", and
for a tool whose whole job is running commands on someone else's machine, that was
the wrong call. It was parked, then built. The parked item is struck below rather
than deleted, because a plan that quietly loses an item is a plan nobody can check.

- **A shared token, compared in time that does not depend on how much of it was
  right.** `==` stops at the first differing byte, so the time a reply takes depends
  on how many leading bytes were correct, and that recovers a secret one byte at a
  time. In `crates/linklet-core/src/auth.rs`, with a test that measures rather than
  asserts, and a control that proves the measuring works.
- **A sealed channel with forward secrecy.** X25519, HKDF and ChaCha20-Poly1305,
  with both ephemeral private halves discarded, so a session recorded today cannot
  be read by anyone who learns the token tomorrow.
- **The token is never transmitted.** What crosses the wire is public keys and
  sealed bodies; the token is mixed into the key derivation instead of being sent.
  A captured request is not a credential.
- **Two messages on one connection**: the handshake, then the command sealed under
  the session it produced. The handshake cannot protect itself, so the exchange has
  to come first -- and both on one connection keeps the agent stateless.
- **Rule 1 was rewritten**, from "the core depends on no crate" to "the core depends
  on no crate that does I/O", because the old wording was satisfied by a rule rather
  than a reason and had been read as a rule about the whole project. See
  `docs/decisions.md`.

## M7 -- getting a build onto the machine, and the evidence back

**Done.** `linklet push` puts one file on a target and `linklet pull` brings one
back, over the sealed channel, with the receiving side verifying the digest before
the real path is touched -- so a transfer that did not arrive intact leaves nothing
behind under the name the next step would believe. Both are on the MCP surface too,
because an agent cannot install a build it has no way to send.

**Why this one was necessary rather than valuable**, which is what the plan said
before it was built: the tool could check reachability and run commands, and it
could not put anything on a target -- which is the first step of the workflow it was
written for. A tool that cannot deploy has not yet reached the point where its
security model can be shown to be worth anything.

**What was already in place before any I/O was written**, all in `linklet-core` and
all tested. Written down then because a reader who has to work out what exists will
either rebuild it or build on a different shape; kept now because it is still where
the design lives:

- [x] `src/frame.rs` -- the wire format, with the ten ways a length-prefixed
      protocol goes wrong listed and defended, and a test per failure mode
- [x] `src/transfer.rs` -- T1 path validation against a root (the Windows rules a
      `..` check does not cover), T3/T11: the manifest checked before any chunk is
      read, and the chunk arithmetic as a function with tests. The running total
      (T4), the end-of-transfer and digest checks (T5, T6, T7) arrived with the
      receiving side and are tested in the same place, in microseconds
- [x] `Sealed::seal_into` / `open_into` -- T10, so a chunk of a file is in memory
      once rather than three times

**What it took**, in `adapters`, `agent` and `client`:

- [x] the connection loop reads frames instead of HTTP: a read timeout on every
      read, one chunk at a time so the desynchronisation defence keeps holding, and
      the message count bounded by the declared size
- [x] the agent receives a transfer: manifest, path, `.part`, per-chunk total,
      digest, rename
- [x] the client sends one: stream, `seal_into`, frame
**The agent's hand-written HTTP layer was replaced first, as this section required.**
It is deleted, and `tiny_http` was never needed: the answer turned out to be the
length-prefixed frame that `docs/decisions.md` D4 had already written down as the
design that fits the channel. What replaced it is
`crates/linklet-adapters/src/connection.rs` for the socket and
`crates/linklet-agent/src/server.rs` for the protocol, and D4 records what that cost.

- [x] `linklet push --agent <host:port> --from <local> --to <remote>` copies one
      file to a target over the sealed channel
- [x] `linklet pull --agent <host:port> --from <remote> --to <local>` brings one
      back, for collecting a log or a result
      -- both need `--root` on the agent, which is the one directory a transfer may
      read or write; it defaults to the directory the agent was started in
- [x] **the framed body carries bytes rather than hex.** It was hex because the
      framing layer was written as text, and that doubled every sealed body. Replacing
      HTTP with frames removed the text body entirely, which is stronger than making
      the body binary: there is no longer a body that could be text.
- [x] a size limit, refused clearly rather than truncated
- [x] **the transfer is verified by digest**, compared by the receiving side. A
      half-transferred file left at the destination under its real name is worse
      than a failed transfer, because the next step believes it.
- [x] `push` and `pull` on the MCP surface: an agent cannot install a build it has
      no way to send

**What this milestone does not do, and says so: it does not install anything.** It
moves a file. Whether that file is an agent, a build or a configuration is the
caller's business. "Install automation" stays parked -- the first copy onto a target
is a step a person performs, and performing it by hand once is how they find out
what a deployment actually consists of.

**What it left open, rather than quietly not doing.** A transfer has no progress
reporting: a slow one and a stuck one look the same until the deadline, which is
`docs/transfer.md`'s own parked list. There is no resumption, deliberately, and one
transfer moves one file -- a directory is the caller's loop. And nothing in the
smoke layer covers a transfer: `docs/smoke.md` says which claims it does and does
not make.

## M8 -- did the tool actually do what you asked

**No longer waiting on anything.** M7 landed, so this is the next one and it is not
started. Worth writing down because a sibling project does this better and the gap is
real: every command that reports a *result* should also report whether it was able to
look.

- how many things were examined, and whether it stopped early
- which filters actually applied, echoed back
- a failure to enumerate, never reported as an empty result

`linklet` has one instance of this pattern -- exit 1 against exit 3 -- and it needs
several more before its answers can be trusted on a machine it does not control.

## M9 -- identities, and being able to change the cipher

Parked. The token authenticates the channel and says nothing about *which* caller it
is, so there is no per-caller revocation and no audit trail. There is also one curve,
one cipher and one derivation, chosen at build time. Neither is needed to use this on
a network you control, and both are needed before it is used on one you do not.
## Parked deliberately

Written down so they can be refused on purpose rather than discovered by
accident. None of these is planned:

- a job/session model with a lifecycle (the honest version of this is M4)
- a configuration file (flags until there is a proven need for persistence)
- a daemon or service on the host
~~- encryption, authentication, or any security boundary beyond "the caller can
  already reach the machine"~~ -- **struck at M6.** It was the wrong call, and it
  is left visible because the plan was believed for several milestones while it was
  wrong.
- install automation for the first copy onto a target

## A rule about this file

If a milestone here stops being true -- because the work showed a better order,
or showed the milestone to be unnecessary -- **edit this file in the same
commit that makes it untrue.** A roadmap that describes a plan nobody is
following is worse than no roadmap, because it is believed.
