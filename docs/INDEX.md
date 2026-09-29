# Index

A map, not a chapter. One screen. If a line here is wrong, fixing it is part of
the change that made it wrong.

## Where to look

| question | file |
|---|---|
| What is this, and what does it not do? | `README.md` |
| What are the rules? | `AGENTS.md` |
| Why does a rule exist? | `docs/rationale.md` |
| What do I build next? | `docs/ROADMAP.md` |
| How do I do a task? | `docs/LEARNING.md` |
| How will my work be judged? | `docs/review-m1.md` |
| What is the state of the code? | below |

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
