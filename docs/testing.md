# Test strategy

What kind of test each kind of code gets, and why. The point of writing this
down is that "should this be tested against a real machine?" has an answer that
does not depend on who is asking.

## The four layers

Ordered by what they cost to run, cheapest first. **Most tests belong in the
first layer**, and a test in the wrong layer is worse than a missing one: it
slows every run and covers less than it appears to.

| layer | lives in | touches a machine? | speed | what it is for |
|---|---|---|---|---|
| **Unit** | `src/**/#[cfg(test)]` and `tests/` in the core | no | microseconds | a decision |
| **Contract** | `tests/*_contract.rs`, `tool_surface.rs` | no | microseconds | an interface that other software branches on |
| **Adapter** | `crates/linklet-adapters/tests/` | yes, this machine | seconds | the code that talks to the OS |
| **End to end** | `crates/linklet-cli/tests/` | yes, as a process | seconds | the binary as a user and an agent meet it |

## The rule that decides the layer

> **A test belongs in the cheapest layer that can observe the behaviour.**

And the corollary that does the real work:

> **If a behaviour can only be observed in an expensive layer, that is a design
> problem, not a testing problem.**

That is not a slogan; it is the reason `linklet-core` has no dependencies. The
probe's decision-making -- what counts as reachable, what a timeout means, what
happens to a run when one target fails -- would be in the adapter by default, and
would then need a machine that is off, a machine that refuses, and a machine that
drops packets to test. Declaring `Probe` in the core instead moved seventeen
tests from the adapter layer to the core layer, where they run in under a
millisecond, and `crates/linklet-adapters/tests/tcp_probe.rs` has seven tests
left because seven is all that genuinely needs a socket.

## What each layer may and may not assert

**Unit and contract tests** may assert anything about a decision. They may not
open a socket, read a file, or read a clock.

**Adapter tests** assert what the operating system actually does, which is why
they are the only tests whose comments cite measurements. They may not contain
decisions: if a test in this layer is checking a *rule*, the rule is in the wrong
place.

**End-to-end tests** assert the things that only exist once a program is a
process: the exit code, which stream text went to, and the order of lines. A unit
test cannot catch a report printed to stderr or an exit code dropped on the way
out of `main` -- those are the bugs that break an agent, and they are invisible
to every other layer.

## What a contract test is for, and its one failure mode

The contract layer asserts an interface that **other software branches on**. The
MCP tool surface is one: a description that grew past a length, or a tool that
vanished, breaks a caller that was working.

The failure mode worth naming is a contract asserted **per subject instead of
across subjects**. `crates/linklet-cli/tests/arguments.rs` is the receipt:
`--agent` was parsed nine times and had drifted into two different messages for
the same mistake, and nothing caught it because each copy was only ever tested
against itself. Every one of those nine was green.

So that file walks every command in one test and asserts **the commands agree**.
Adding a tenth command means adding a row to a table, which is the whole
mechanism: a test that named one command would have let this drift happen and
would not notice the next one.

## What is deliberately not tested

- **Nothing is asserted about hostnames.** Deciding what a hostname looks like
  is the resolver's job, and a test that pinned a grammar would reject a real
  name and pass a fake one. See the doc comment on `Host`.
- **No test asserts a wall-clock duration**, except the two in
  `tcp_probe.rs` that exist to catch a probe sleeping away its budget. A timing
  assertion is a test that fails on a busy machine, which makes it a test that
  gets deleted rather than fixed.
- **No test starts a virtual machine.** See below.

## Real machines: the honest position

Everything above runs on one machine, and that machine is the one the developer
is sitting at. That is enough for the adapter layer and not enough for a claim
about a LAN.

A claim about a LAN needs a testbed, and a testbed is **a specification plus a
checker**, not a virtual machine -- see `crates/linklet-core/src/testbed.rs` for
why that distinction is the whole design. The practical consequences:

1. **Readiness is a decision and is tested as one.** `tests/testbed_spec.rs`
   covers every requirement against a machine made of a table. Whether a
   *particular* machine satisfies a specification is `linklet testbed check`,
   which is a command and not a test, because "is this machine ready" is a
   question about the world and its answer changes without the code changing.
2. **A test that needs a machine declares it.** A testbed specification lives in
   `testbeds/*.testbed`, and a test that cannot run without one is marked
   `#[ignore]` with the reason, rather than being skipped silently or failing on
   a laptop.
3. **The gap is stated, not implied.** There is still no *automated* run against a
   second machine -- it needs a machine, so it cannot be a gate. What exists is
   `tools/smoke.ps1`, which is a script you run, and `docs/smoke.md`, which says
   what it claims and what it cannot. `docs/ROADMAP.md` records the automated part
   as open rather than done, because a checked-off item that was quietly dropped is
   how a roadmap becomes fiction.

**A fifth layer, and it is not a gate.** `tools/smoke.ps1` takes an address and makes
seven claims about a machine that exists. It is a script and not part of
`tools/verify.ps1` because it needs a machine: a gate that needs one stops being run,
and then the behaviour it covered rots. `docs/smoke.md` is the whole argument.

## What runs it

**`tools/verify.ps1` is the only definition of done, and CI calls that script rather than
restating its four commands.** `.github/workflows/verify.yml` is the caller: one step, on
Windows, because the tests spawn `tasklist`, `taskkill`, `ipconfig` and `route` and bind
loopback sockets -- a job on another operating system would fail for a reason that has
nothing to do with the change.

The script's own header names the failure this arrangement prevents: the four commands were
documented in three files, a commit went in red anyway, and the rules were fine -- nothing
ran them. A workflow that listed the four commands again would be that same mistake one level
up, with two lists to keep in step and the CI copy being the one nobody tries locally.

**CI does not change what a green run means.** It covers exactly the four gates, on one
machine, the same way a person does. The layer that needs a second machine is still missing,
still open, and still named above.

## Knowing a test is worth its place

Two questions, in order:

1. **Has it been seen failing, for the reason intended?** A test never observed
   red is not evidence: it may be failing to compile, or not running at all.
2. **Can it fail?** A test that cannot fail is a test that costs time on every
   run and buys nothing. Delete it rather than leave it as documentation, because
   a green test that cannot go red is a claim nobody can check.

The repository has the receipt for both. `tests/architecture.rs` failed on an
empty table header rather than the rule it was named after, and a mutation test
of the doc gate could not run because the network was down -- **both looked
exactly like the check working.**
