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
| Why is it built this way? | `docs/decisions.md` |
| How does the wire format work? | `docs/framing.md` |
| What does a version number mean, and what is a release? | `docs/VERSIONING.md` |
| Why did this command fail in a way that looks like a bug? | `docs/machine.md` |
| How does a file transfer work? | `docs/transfer.md` |
| What do I build next? | `docs/ROADMAP.md` |
| How do I do a task? | `docs/LEARNING.md` |
| What kind of test does this get? | `docs/testing.md` |
| How do I call this from an agent? | `docs/MCP.md` |
| How do I install it as an MCP server, and what tells an agent the routine? | `integrations/README.md` |
| How will my work be judged? | `docs/review-m1.md` |
| What went wrong on the way here? | `docs/retrospective.md` |
| How do I test against a real machine? | `docs/smoke.md` |
| What is the state of the code? | below |
| Can this run on Linux, and what would it take? | `docs/ROADMAP.md` M11 -- measured, not planned |

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
| `docs/decisions.md` | decision | the choices that are not obvious from the code |
| `docs/framing.md` | rule | the frame format, and the defences that live outside it |
| `docs/transfer.md` | rule | moving a file: the design and its fourteen failure modes |
| `docs/review-m1.md` | decision | the standard M1 was judged against, unedited |
| `docs/retrospective.md` | decision | the failures, ranked, and what the gates could not catch |
| `docs/smoke.md` | rule | the real-machine layer: what it claims, and what it needs |
| `docs/machine.md` | current | what this machine does to commands, and what it cost to find out |

## The experiments

Not documentation. **An experiment is evidence about a decision, kept so it can be re-run, and
it is deliberately not under `docs/`** -- nothing routes a reader here looking for how to do
something, which is what `docs/` is for. `experiments/` holds the method and the raw runs, so a
result whose reasoning lives in somebody's memory cannot be mistaken for one that has none.

| path | what it measured |
|---|---|
| `experiments/m5-tool-readability.md` | whether a reader with no context can pick the right call from the tool descriptions alone -- and the boundary it found, which is that a short description cannot say *when* to prefer a tool |

## The code

Direction: `cli -> adapters -> core`. The core is pure; the edges are thin.

| path | state | what it is |
|---|---|---|
| `crates/linklet-core/` | done | decisions. No I/O; `serde_json` is the only dependency, and it computes |
| `crates/linklet-adapters/` | done | sockets and crypto: the only place either happens |
| `crates/linklet-agent/` | done | the target side: binds a port, runs commands |
| `crates/linklet-client/` | done | the host side of the agent protocol |
| `crates/linklet-cli/` | done | argv in, text out, exit code |

| file | what it holds |
|---|---|
| `crates/linklet-core/src/lib.rs` | the public surface, and why the traits live here |
| `crates/linklet-core/src/target.rs` | `Host`, `Port`, `Target`, `parse_targets`, and the rule order |
| `crates/linklet-core/src/error.rs` | `TargetError`, one variant per way to fail |
| `crates/linklet-core/src/arguments.rs` | reading a command's flags: one table, one parser, and the line between a flag and a command tail |
| `crates/linklet-core/src/probe.rs` | the `Probe` trait, `check_targets`, the limits |
| `crates/linklet-cli/src/probe.rs` | the probe command: whether an agent is working or only listening, and the exit codes a supervisor reads |
| `crates/linklet-core/src/outcome.rs` | the output format and the exit codes: the contract |
| `crates/linklet-core/src/tool.rs` | the MCP tool surface: what may be called, and the rules on it |
| `crates/linklet-core/src/wire.rs` | the host-agent protocol: paths, messages, and the hex for sealed bodies |
| `crates/linklet-core/src/transfer.rs` | T1: whether a path may be written, and the two filesystems' rules for deciding -- including the case rule that was a live escape on Linux |
| `crates/linklet-core/src/auth.rs` | the token, where it may come from, and the constant-time comparison that is the point of it |
| `crates/linklet-core/src/channel.rs` | what a sealed conversation is, and what it is not |
| `crates/linklet-core/src/process.rs` | what is running, what may be stopped, and why an empty list has to say what it looked at |
| `crates/linklet-core/src/search.rs` | what a run of bytes is, which lines match, why a failed search is not an empty one, and the total rule for bytes no machine can name |
| `crates/linklet-core/src/listing.rs` | what is in a directory, and why an empty one is not one that is not there |
| `crates/linklet-core/src/discover.rs` | which networks this host is on, which addresses a scan would try, what it leaves out, and how both platforms spell a network |
| `crates/linklet-core/src/fanout.rs` | one operation across several machines: the order, the fates, and the panics that do not take the report with them |
| `crates/linklet-core/src/log.rs` | the agent's request log: what a line says, the pair that names a request that never finished, and when the file rolls over |
| `crates/linklet-core/src/json.rs` | the domain enum, and the conversions to `serde_json` |
| `crates/linklet-adapters/src/tcp.rs` | the real probe, and the Windows measurements behind it |
| `crates/linklet-adapters/src/connection.rs` | the framed connection: a timeout on every read, no read-ahead, a message budget |
| `crates/linklet-adapters/src/transfer.rs` | moving a file: the `.part`, the running total, the digest, the rename |
| `crates/linklet-adapters/src/channel.rs` | ChaCha20-Poly1305, HKDF, and the X25519 handshake |
| `crates/linklet-adapters/src/discovery.rs` | the scan, the plan, and which networks a machine has -- the platform behind two functions |
| `crates/linklet-adapters/src/discovery/windows.rs` | `ipconfig` and `route print -4` |
| `crates/linklet-adapters/src/discovery/linux.rs` | `ip`, and why this one does not read `/proc` |
| `crates/linklet-adapters/src/listing.rs` | reading a directory on this machine, one unreadable name at a time |
| `crates/linklet-adapters/src/search.rs` | reading a file on this machine: the ceiling, the window, and which rule decoded it |
| `crates/linklet-adapters/src/processes.rs` | the process list and the kill: the orchestration, and the two invariants it holds |
| `crates/linklet-adapters/src/processes/windows.rs` | `tasklist`, `taskkill`, `wmic`, and the CSV parser that survives a localised notice |
| `crates/linklet-adapters/src/processes/linux.rs` | `/proc`, `kill`, and the zombie that is not a running process |
| `crates/linklet-agent/src/shell.rs` | the one module that knows which operating system this is: `cmd /C` or `sh -c` |
| `crates/linklet-adapters/src/mcp.rs` | the MCP server: stdio, newline-delimited JSON-RPC |
| `crates/linklet-agent/src/server.rs` | the handshake, then one request, and when to answer a refusal |
| `crates/linklet-agent/src/log.rs` | the agent's log file, and what happens when it cannot be written |
| `crates/linklet-agent/src/spawn.rs` | starting a program that outlives the request, with its own output file |
| `crates/linklet-agent/src/execute.rs` | running a command, and killing the tree it started |
| `crates/linklet-client/src/lib.rs` | the handshake, then the sealed request |
| `crates/linklet-cli/src/main.rs` | argument parsing and printing, nothing else |
| `crates/linklet-core/tests/architecture.rs` | the layer rule and the dependency allowlist, checked not trusted |
| `crates/linklet-core/tests/auth_secret.rs` | the token rules, and a timing test for the comparison |
| `crates/linklet-adapters/tests/connection.rs` | the four framing defences that need a socket, over real sockets |
| `crates/linklet-adapters/tests/transfer.rs` | a real file over a real socket, and what a failure may leave |
| `crates/linklet-adapters/tests/handshake.rs` | forward secrecy and the man in the middle |
| `crates/linklet-adapters/tests/channel_sealing.rs` | confidentiality, integrity, ordering |
| `crates/linklet-agent/tests/agent_server.rs` | the agent as a process, including who may ask |
| `crates/linklet-client/tests/against_agent.rs` | the client against the real agent binary, transfers included |
| `crates/linklet-client/tests/search.rs` | the search over a socket: the window, the ceiling and the encoding that won |
| `crates/linklet-client/tests/ps.rs` | the listing and the deploy loop over a socket: look, stop, and confirm -- on whichever platform the suite runs on |
| `crates/linklet-cli/tests/cli.rs` | the binary, run as a process |
| `crates/linklet-cli/tests/push_pull.rs` | the transfer commands, as a person and an agent meet them |
| `crates/linklet-cli/tests/token_file.rs` | the secret as a file on both ends: the Windows bytes, the two refusals, and what each end does instead |
| `crates/linklet-cli/tests/testbed.rs` | `testbed check` against this machine: the `no-process` requirement, which used to be Windows-only |
| `crates/linklet-cli/tests/fanout.rs` | one command across several agents: the labels and the exit codes, and a refusal told apart from an unreachable machine |
| `crates/linklet-client/tests/probe.rs` | whether an agent is working or only listening, against a socket that accepts and says nothing |
| `crates/linklet-cli/tests/probe.rs` | the probe's exit codes as a supervisor reads them |
| `crates/linklet-cli/tests/arguments.rs` | every command's flags, asserted as one contract rather than ten behaviours |
| `crates/linklet-core/tests/ci_workflow.rs` | the CI workflow calls the one gate script, on both platforms, instead of restating it |
| `crates/linklet-core/tests/ascii_only.rs` | rule 7, checked instead of remembered |
| `tools/verify.ps1` | the one gate script: fmt, clippy, test, rustdoc, actionlint, inventory |
| `integrations/README.md` | the two pieces an agent installs: the server entry per client, and the token |
| `integrations/dsh/mcp-entry.yml` | the DSH server entry, with the reason for each of its four fields |
| `integrations/skills/linklet/SKILL.md` | the procedure: the order the calls go in, and the rules that bite when ignored |

## Keeping this true

It is an index, so it goes stale silently and a stale index is worse than none:
it sends the reader somewhere confident and wrong. The rule is the same as for
every other document here -- **the commit that changes the code changes this
file.** It should never need its own commit.


