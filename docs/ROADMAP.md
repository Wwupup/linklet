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

## M0 -- the skeleton (this commit)

- [x] workspace, three crates, the compiler-enforced layer rule
- [x] `.gitignore`, `AGENTS.md`, `rust-toolchain.toml`, this file
- [x] the core types declared, with the tests that define their behaviour
- [ ] everything else: `cargo test` fails on `unimplemented!`, on purpose

## M1 -- the first decision, end to end

The smallest complete thing that does something true.

- [ ] `parse_targets`: the text grammar in `linklet-core/src/target.rs`
- [ ] `cargo test -p linklet-core` green
- [ ] the CLI prints the parsed targets, and exits 2 on a bad spec

*What you learn here:* writing a test before the code, making an invalid state
unrepresentable (`Port` cannot hold 0), and the difference between a validation
error and a guess.

## M2 -- the first I/O, and why it does not leak into the core

- [ ] `linklet-core` defines the trait it needs: "tell me whether this target
      answers, within this deadline"
- [ ] `linklet-adapters` implements it with a real TCP connect and a deadline
- [ ] the core keeps a *policy* on top of it (what counts as reachable, what
      happens on timeout) and that policy is tested with a fake in-memory
      implementation -- no network in the test

*What you learn here:* dependency inversion, and why the trait belongs to the
core rather than to the adapter. This is the single most valuable idea in the
whole roadmap.

## M3 -- a command line an agent can use

- [ ] a real `check` command, output that is stable and parseable
- [ ] exit codes and error messages that an agent can act on without reading
      documentation twice
- [ ] every message the tool prints is either a fact it observed or an error it
      can quote -- no reassurance, no summary, no invented noun

*What you learn here:* the interface is the product, and every sentence printed
is part of it.

## M4 -- one call instead of five

- [ ] concurrency across targets, with per-target timeouts
- [ ] a single invocation that reports on many machines
- [ ] the decision of what to do about partial failure, written down and tested

*What you learn here:* where concurrency belongs (the adapter) and where it
must not live (the decision).

## M5 -- being useful from an agent, and being honest about it

- [ ] the MCP surface, deliberately **few and coarse**: one call per intent,
      not one call per endpoint
- [ ] the whole tool description fits in a paragraph, without caveats
- [ ] a check on the process itself: hand the tool list to a model that has not
      seen this repository and see whether it can pick the right call

*What you learn here:* why a tool description that needs a manual is a symptom
of a bad interface, which is where this project came from.

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
