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
- [ ] a check on the process itself: hand the tool list to a model that has not
      seen this repository and see whether it can pick the right call -- **not
      done, and it is the one that tests the claim.** The other two check the
      surface; this one checks whether the surface works on a reader with no
      other context, which is the only question that matters and the one a test
      cannot answer.

*What you learn here:* why a tool description that needs a manual is a symptom
of a bad interface, which is where this project came from.

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

## Parked deliberately

Written down so they can be refused on purpose rather than discovered by
accident. None of these is planned:

- a job/session model with a lifecycle (the honest version of this is M4)
- a configuration file (flags until there is a proven need for persistence)
- a daemon or service on the host
- encryption, authentication, or any security boundary beyond "the caller can
  already reach the machine"
- install automation for the first copy onto a target

## A rule about this file

If a milestone here stops being true -- because the work showed a better order,
or showed the milestone to be unnecessary -- **edit this file in the same
commit that makes it untrue.** A roadmap that describes a plan nobody is
following is worse than no roadmap, because it is believed.
