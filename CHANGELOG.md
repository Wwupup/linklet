# Changelog

What changed, for someone using this tool rather than building it. Grouped by
milestone, because the milestones are how the work was actually decided.

Not automatically generated from commits. `git log` already has that, and a
generated changelog tells a user which internal function moved. This file exists
to answer one question: **what can I do now that I could not do before?**

## [Unreleased]

### Added

- **A handshake, so a recorded session cannot be read by someone who learns the
  secret later.** Both sides generate an X25519 key pair per handshake and discard
  the private half, so the session key depends on a value that no longer exists
  anywhere. The shared secret still authenticates the exchange: an attacker in the
  middle can complete a handshake with both ends and still cannot open a byte,
  because the session they build is not the one either end built.

### Changed

- **The project may now depend on vetted crates, in the adapters.** The rule that
  kept `linklet-core` dependency-free was being read as a rule about the whole
  project, and it was applied past the point where its reason held: a SHA-256 was
  hand-written while the registry looked unreachable, and was wrong three times out
  of three in its padding. The registry was reachable -- a stale proxy address in a
  git config was the whole problem. `docs/decisions.md` records it.

### Added

- **A shared token is required to run anything.** The agent refuses a request
  without one, with the same 401 and the same words whether the token was absent
  or wrong -- which of the two happened is information a caller with the token
  does not need and one without it should not get. The host reads `LINKLET_TOKEN`
  from the environment rather than a flag, so the secret stays out of process
  listings and shell history, and the agent refuses to start with a token shorter
  than sixteen bytes.

- **`linklet exec --agent <host:port> <command...>`**, and an `exec` MCP tool.
  The exit code is the command's own when it has one, so `linklet exec ... && next`
  behaves the way the command would; a call that could not be made gets the
  refusal code, which no command can produce, so a script can tell "it ran and
  failed" from "it never ran" without reading any output.

- **`linklet-agent`**, the target-side binary. One file, one dependency (the
  shared protocol), and two things it does: say which agent it is, and run a
  command when asked. It is separate from the host tool because it runs on a
  different machine -- the one being debugged -- and because shipping "can check
  a port" and "can run a command" behind the same door is a larger thing to put
  on someone else's box than either alone.
- **The host-agent wire protocol**, in `linklet-core/src/wire.rs`. It is the one
  place that decides what a host asks an agent and what an agent answers, so a
  change to the protocol is a change to one file rather than to two that must
  agree. Both binaries decode the same definitions, so they cannot drift into two
  readings of one message.
- **`linklet-client`**, the host side of the protocol. It shares `wire.rs`
  with the agent, so the two ends of a message are decoded by the same code, and
  its integration tests spawn the real agent and run real commands through it.
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

### Added

- **A handshake, so a recorded session cannot be read by someone who learns the
  secret later.** Both sides generate an X25519 key pair per handshake and discard
  the private half, so the session key depends on a value that no longer exists
  anywhere. The shared secret still authenticates the exchange: an attacker in the
  middle can complete a handshake with both ends and still cannot open a byte,
  because the session they build is not the one either end built.

### Changed

- `dispatch` takes a `ToolRunner` rather than a closure per capability. With one
  tool the closure read fine; with two it would have been two closures and a
  signature that was the least readable thing in the file.
- The MCP session tests set the process's working directory instead of inheriting
  it. `cargo test` starts the binary in the crate directory, and a test that
  assumed the repository root failed with "cannot read target/..." while the file
  was demonstrably there.

### Fixed

- **A command killed by its deadline did not return.** `Child::kill` kills the
  shell, not the program the shell started, so `cmd /C ping ...` left ping
  holding both pipes, the agent's readers never finished, and the caller saw a
  transport failure for a command the agent was about to describe properly. The
  fix kills the process tree by pid. The upstream project's pitfalls file has
  this exact lesson in it, which is where the first cost of learning it was paid.
- **Many targets no longer take one timeout each.** 20 unreachable machines on a
  5 s budget finish in 5.0 s rather than roughly 40 s. The `check` command uses
  the concurrent run; the answers, the order and the refusals are unchanged.
- `docs/testing.md` was named in neither the index nor the routing table, so a
  reader starting from either place could not find it.

### Known gaps

- **The channel is not wired into the agent and client yet.** It exists, it is
  tested, and nothing uses it: the token still travels in cleartext over HTTP.
  Until that is done, this is a tool for a network you control and the auth module
  documentation says so.
- **No cipher agility and no version negotiation.** One cipher, one curve, one
  key derivation, chosen at build time.
- No automated run against a second machine. `docs/testing.md` says what that
  costs and what it does not cover.
- The MCP surface has never been read by a model with no other context. The tests
  check that the descriptions are short; they cannot check that they are
  understandable, and that is the claim the milestone rests on.
- `--at-once` is a constant rather than a flag. Ten unreachable machines and
  sixty behave the same way, so nobody has wanted a different value yet; a flag
  added before anyone asks is how a command line grows arguments nobody uses.

## [0.1.0]

The first version with a shape. Everything below landed between the initial
skeleton and M5.

### Added

- **A handshake, so a recorded session cannot be read by someone who learns the
  secret later.** Both sides generate an X25519 key pair per handshake and discard
  the private half, so the session key depends on a value that no longer exists
  anywhere. The shared secret still authenticates the exchange: an attacker in the
  middle can complete a handshake with both ends and still cannot open a byte,
  because the session they build is not the one either end built.

### Changed

- **The project may now depend on vetted crates, in the adapters.** The rule that
  kept `linklet-core` dependency-free was being read as a rule about the whole
  project, and it was applied past the point where its reason held: a SHA-256 was
  hand-written while the registry looked unreachable, and was wrong three times out
  of three in its padding. The registry was reachable -- a stale proxy address in a
  git config was the whole problem. `docs/decisions.md` records it.

### Added

- **`linklet exec --agent <host:port> <command...>`**, and an `exec` MCP tool.
  The exit code is the command's own when it has one, so `linklet exec ... && next`
  behaves the way the command would; a call that could not be made gets the
  refusal code, which no command can produce, so a script can tell "it ran and
  failed" from "it never ran" without reading any output.

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
