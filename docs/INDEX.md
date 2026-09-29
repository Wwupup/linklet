# Index

A map, not a chapter. One screen. If a line here is wrong, fixing it is part of
the change that made it wrong.

## Where to look

| question | file |
|---|---|
| What is this, and what does it not do? | `README.md` |
| What are the rules? | `AGENTS.md` |
| How do I write a commit? | `docs/COMMITS.md` |
| Why does a rule exist? | `docs/rationale.md` |
| What do I build next? | `docs/ROADMAP.md` |
| How do I do a task? | `docs/LEARNING.md` |
| What kind of test does this get? | `docs/testing.md` |
| How do I call this from an agent? | `docs/MCP.md` |
| How will my work be judged? | `docs/review-m1.md` |
| What is the state of the code? | below |

## What kind of document is it

Not every document here goes out of date the same way, and treating them alike
is how a repository ends up with a folder full of things nobody dares delete.
Four kinds, distinguished by **what keeps them true**:

| kind | kept true by | goes stale when | in this repository |
|---|---|---|---|
| **Spec** | the tests | the tests change | `tests/*.rs` |
| **Rule** | being obeyed | someone changes it on purpose | `AGENTS.md`, `docs/COMMITS.md`, `docs/LEARNING.md`, `docs/testing.md` |
| **Current** | a commit | the code moves | `README.md`, `docs/ROADMAP.md`, this file |
| **Decision** | nothing -- it is a moment | never | `docs/rationale.md`, `docs/review-m1.md` |

The first three kinds belong in the repository, because someone doing next
week's work gets it wrong without them. The fourth is where the trouble starts:
a **decision** records what was true when it was written, so it does not go
stale -- it accumulates. `docs/review-m1.md` is one: the criteria were published
before the implementation and were left unedited on purpose, because a rubric
tidied up afterwards is no longer evidence that it came first.

**Records do not belong here.** A task brief, a progress log, a diff of a review
round -- those are what a pull request description and a commit message are for.
Git already stores them, searchably, attached to the change they describe. The
project this one is modelled on kept 2.28 MB of such files in a working
directory; none of it was tracked, which was the right call made by accident
rather than by rule. A `docs/` that fills up with records is one where the four
kinds above can no longer be told apart, and then nobody trusts any of it.

The test for where something new goes: **will someone doing next week's work do
it wrong without this?** Yes -- a file, in the repository. No -- a commit
message, or nothing.

| file | kind | what it holds |
|---|---|---|
| `AGENTS.md` | rule | the rules, one screen |
| `docs/COMMITS.md` | rule | how to write a commit, with worked examples |
| `README.md` | current | what the tool is and is not |
| `docs/ROADMAP.md` | current | what to build next, and what was parked |
| `docs/LEARNING.md` | rule | the task loop: red, spec, smallest change, verify |
| `docs/testing.md` | rule | which layer a test belongs in, and why |
| `docs/INDEX.md` | current | this file |
| `docs/rationale.md` | decision | why each rule exists, read when changing one |
| `docs/review-m1.md` | decision | the standard M1 was judged against, unedited |

## The code

Direction: `cli -> adapters -> core`. The core is pure; the edges are thin.

| path | state | what it is |
|---|---|---|
| `crates/linklet-core/` | M1-M3 done | decisions; no I/O, no dependencies |
| `crates/linklet-adapters/` | M2 done | sockets: the one place that opens one |
| `crates/linklet-cli/` | M3 done | argv in, text out, exit code |

| file | what it holds |
|---|---|
| `crates/linklet-core/src/lib.rs` | the public surface, and why the traits live here |
| `crates/linklet-core/src/target.rs` | `Host`, `Port`, `Target`, `parse_targets`, and the rule order |
| `crates/linklet-core/src/error.rs` | `TargetError`, one variant per way to fail |
| `crates/linklet-core/src/probe.rs` | the `Probe` trait, `check_targets`, the limits |
| `crates/linklet-core/src/outcome.rs` | the output format and the exit codes: the contract |
| `crates/linklet-adapters/src/tcp.rs` | the real probe, and the Windows measurements behind it |
| `crates/linklet-cli/src/main.rs` | argument parsing and printing, nothing else |
| `crates/linklet-core/tests/target_parsing.rs` | the specification for `parse_targets` |
| `crates/linklet-core/tests/probe_check.rs` | reachability policy, against a fake probe |
| `crates/linklet-core/tests/output_contract.rs` | the line format and exit codes, pinned |
| `crates/linklet-core/tests/architecture.rs` | the layer rule, checked not trusted |
| `crates/linklet-adapters/tests/tcp_probe.rs` | the part that needs a real socket |
| `crates/linklet-cli/tests/cli.rs` | the binary, run as a process |

## Keeping this true

It is an index, so it goes stale silently and a stale index is worse than none:
it sends the reader somewhere confident and wrong. The rule is the same as for
every other document here -- **the commit that changes the code changes this
file.** It should never need its own commit.


