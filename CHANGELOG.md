# Changelog

What changed, for someone using this tool rather than building it. Grouped by
milestone, because the milestones are how the work was actually decided.

Not automatically generated from commits. `git log` already has that, and a
generated changelog tells a user which internal function moved. This file exists
to answer one question: **what can I do now that I could not do before?**

## [Unreleased]

### Added

- **The `testbed` MCP tool.** `linklet testbed check` shipped as a command and
  was, for a while, a capability no agent could reach: the surface was never told
  about it. An agent cannot ask for what it has not been told about.
- **`LICENSE`.** `Cargo.toml` has claimed MIT since the first commit; a
  declaration is not a file, and the two disagreeing is worse than neither.
- `tools/verify.ps1` -- the four gates in one command, and the entry point every
  caller refactors through: a person, a hook, or CI. It exists because a commit
  went in red while those four commands were documented in three separate files.
  The rules were written; nothing ran them.
- A test that every document is reachable from `AGENTS.md` or `docs/INDEX.md`,
  and that every `docs/...` path either of them names exists. Two such faults
  were found by hand before this test existed, and a fault found twice by hand
  belongs in a test.

### Changed

- `dispatch` takes a `ToolRunner` rather than a closure per capability. With one
  tool the closure read fine; with two it would have been two closures and a
  signature that was the least readable thing in the file.
- The MCP session tests set the process's working directory instead of inheriting
  it. `cargo test` starts the binary in the crate directory, and a test that
  assumed the repository root failed with "cannot read target/..." while the file
  was demonstrably there.

### Fixed

- `docs/testing.md` was named in neither the index nor the routing table, so a
  reader starting from either place could not find it.

### Known gaps

- **M4, concurrency**, is still not implemented: targets are checked one after
  another, so ten unreachable machines take ten timeouts. Deferred in two
  consecutive milestones, which is where a deferral starts becoming permanent.
- No automated run against a second machine. `docs/testing.md` says what that
  costs and what it does not cover.
- The MCP surface has never been read by a model with no other context. The tests
  check that the description is short; they cannot check that it is
  understandable, and that is the claim the milestone rests on.

## [0.1.0]

The first version with a shape. Everything below landed between the initial
skeleton and M5.

### Added

- `linklet check <target>[,<target>...]` -- reports whether each `host:port`
  accepts a TCP connection, one line per target, with the reason. Exit codes:
  `0` all alive, `1` something is not, `2` bad invocation, `3` the run was
  refused. **`1` and `3` are deliberately different**, because "the machines are
  down" and "the tool could not start" send a caller to different places.
- `linklet mcp` -- speaks the Model Context Protocol on stdin and stdout, so an
  agent can call the tool with no configuration. **One tool on the surface**,
  with a sixty-character description, and tests that keep it that way.
- `linklet testbed check <spec-file> <target>` -- decides whether a machine
  matches a specification before a test is run against it. Exit codes: `0` every
  requirement held, `1` one did not, `2` the specification or the invocation was
  wrong.
- A written specification format for testbed requirements: `require reachable`,
  `require artifact`, `forbid artifact`, `require no-process`. Line-based, no
  quoting, no escaping, because an agent writes these.
- `--timeout <seconds>` and `--max-targets <count>` on `check`.

### Deliberately not here

- **Running commands, reading logs, moving files.** There is no agent on the far
  side to talk to. These are absent rather than stubbed: a tool that answers
  "not implemented" is worse than a missing one, because the caller has spent a
  turn on it and cannot tell a missing feature from a broken one.
- **Anything that modifies the machine it checks.** `check` and `testbed check`
  observe. Nothing in this version changes anything anywhere.
- **A configuration file.** Flags until there is a proven need for persistence.

### Known gaps

- No automated run against a second machine. `docs/testing.md` says what that
  costs and what it does not cover; `docs/ROADMAP.md` records it as open.
- Concurrency across targets is not implemented: targets are checked one after
  another, so a run of ten unreachable machines takes ten timeouts.
