# Changelog

What changed, for someone using this tool rather than building it. Grouped by
milestone, because the milestones are how the work was actually decided.

Not automatically generated from commits. `git log` already has that, and a
generated changelog tells a user which internal function moved. This file exists
to answer one question: **what can I do now that I could not do before?**

  added before anyone asks is how a command line grows arguments nobody uses.

## [Unreleased]

### Added

- **A secret can be read from a file**, so that a script or a client configuration
  can name the secret instead of holding it. `LINKLET_TOKEN_FILE` on both ends, and
  `--token-file` on the agent. A byte-order mark and the line ending are not part of
  the secret, so a file written by a Windows editor or by `Set-Content -Encoding
  utf8` holds exactly what was typed into it. **A secret is named once**: a file and
  a value together are refused rather than ordered, and on the host a file that was
  named and cannot be read is never answered from the environment -- either would
  authenticate with a secret nobody named while telling the caller its token was
  wrong.

- **The release is one archive**, `linklet-<version>.zip`, with the executables under `bin/`
  and everything needed to install and operate them beside them: the MCP client entry, the
  skill that carries the order the calls go in, and the three documents an operator reads.
  Four loose assets had asked whoever downloaded to work out which of them went where.
  `docs/VERSIONING.md` has the layout and `integrations/README.md` has what the material is.
  **Nothing in it installs an agent on a target** -- that first copy is a file copy, once, by
  hand.

### Fixed

- **The agent's log recorded a never-ending liveness check.** `identity` is the one
  request that asks for nothing, and it is what a monitor calls; writing it down turned
  the record into a heartbeat. On this project's own bench, four days of a five-second
  probe put **37,596 `identity` lines into a 37,781-line file** -- 99.5%, leaving 0.5%
  for the work. The cost was not the disk: the log is read by finding a `->` line with no
  `<-`, and that is not findable in a file that is mostly the agent saying it is fine.
  A liveness check is no longer written down.
- **The log is bounded, and it has a home.** It appends and never truncates, on purpose --
  restarting an agent must not destroy the record of what it was asked before it died --
  and it had no other limit, so a machine left serving for months filled its disk. It now
  rolls over at a mebibyte and keeps three older files, **never between a request and its
  answer**, because a pair split across two files reports a finished request as one that
  never finished. It is also kept **without being asked for**: `logs/agent.log` beside the
  executable, not the working directory, because a scheduled task starts its program with
  the scheduler's directory and the default would have landed in `System32\logs` on
  exactly the unattended machines this exists for. `--log` still chooses a path and
  `--no-log` turns it off.

### Removed

- **`tools/linklet-supervise.ps1`**, which restarted an agent that died or wedged. It was
  right about the decision and wrong about the shape: it was a process standing in for a
  service, so nothing watched *it*, and a dead supervisor stopped restarting a dead agent. It
  was also the last Windows-only moving part of a tool that now runs on two platforms. What is
  left is the platform's own scheduler for survival and **`linklet probe`** for the caller who
  wants to know the difference between dead and wedged -- and the caller is now the thing that
  drives this tool, which for an agent is step 2 of the skill. `docs/smoke.md` has both.
- **`tools/smoke.ps1`**, the real-machine layer. It was a PowerShell program that drove the
  commands any caller can already drive, which made it a second client to keep in step with the
  first and one that could only run on one platform. **The seven claims are the durable thing
  and the program was not**: they are a list in `docs/smoke.md` now, and they are made with
  whatever client is to hand.

### Added

- **The agent runs on Linux.** `crates/linklet-agent/src/shell.rs` is now the only module in
  the project that knows which operating system it is on: it picks `cmd /C` or `sh -c`, and
  `taskkill /T /F` or a process-group `kill -9`, so `exec` and `spawn` and the deadline that
  stops them work on both. It was two files each hardcoding `cmd`, and it turned the whole
  `linklet-agent` suite green on Linux -- 36 tests that had never run there.
  **A Windows host driving a Linux agent, and a Linux host driving a Windows one, are both
  verified between real machines**; so are both transfer directions, digests included. What
  is still Windows-only is `ps`, `kill` and `spawn`, which are `tasklist` and `taskkill` in
  `linklet-adapters`; `docs/ROADMAP.md` M11 is what the rest of it would take, and what
  running the suite on Linux found.
- **`linklet-agent --no-log`**, for an operator who wants no record written. It is the
  opt-out the default above made necessary, and what it costs is stated where it is
  offered: a request that never finishes then leaves no evidence behind.
- **The supervisor wrote a line every five seconds for as long as the machine was up**, on the
  console of whoever was watching the target and in its own log. Measured on this project's
  bench after four days: **18,786 of the 18,815 lines were one sentence** saying the agent was
  fine, and the same probe had put 37,596 `identity` lines into the agent's own 37,781 -- so
  the request log whose whole design is that a *missing* second line names a wedged request had
  0.5% of its content left for the work. A cycle is now written when it changes, or when it has
  stood for five minutes. Nothing is hidden: every death, recovery, kill and start is still
  there. `docs/smoke.md` has the table and the reasoning.

What a release looks like, and what a version number means here, is in
`docs/VERSIONING.md`.

## [0.2.0] -- 2026-10-01

**The version where the tool can do something to a machine.** Up to 0.1.0 it observed:
`check`, `testbed check`, and a sealed `exec`. This one closes the loop it was built for --
put a build on a target, run it, see what it is doing, stop it, read what it wrote, and find
the target in the first place. It is the version the first real machine was driven with, and
everything under **Fixed** here is something that machine found.

The wire protocol changed shape, which is why the minor number moved rather than the patch.

**Released after being driven at `192.168.100.2` with these exact binaries**: the seven claims
of `tools/smoke.ps1`, every command this version added, and the deploy loop end to end. That
step earned its place -- it found a defect in `spawn` that no test had, and the entry under
**Fixed** that describes it was written afterwards.

### Added

- **`linklet push` and `linklet pull`**, and both on the MCP surface. A transfer is
  digest-verified in the direction it travels, refuses a path that leaves the agent's root,
  and leaves no `.part` file behind when it fails. `pull` reads through the same ceiling
  `push` writes through, so neither direction can make an agent allocate without bound.
- **`linklet ps`, `linklet kill` and `linklet spawn`**, and all three as tools. Together they
  are the deploy loop: **is the old build still running, stop it, start the new one.** An
  empty process listing carries the counts that make it readable, `kill` has two refusals it
  can only make because it is an interface rather than a command line, and `spawn` returns a
  pid instead of holding the request open the way `exec` does.
- **`linklet grep`, `linklet tail` and `linklet ls`**, and all three as tools. The reading
  happens on the target and only the answer crosses, because a pull is the wrong tool for a
  two-gigabyte log. A file past the sixteen-mebibyte ceiling is read **from its end** for a
  `last` search, and every search says how many lines it read, whether it stopped early, and
  **which encoding won**. `ls` distinguishes an empty directory from one that is not there,
  which are the same list and opposite facts.
- **`linklet discover`**, so the machines on a network can be found rather than written down.
  It reads this host's interfaces, builds a plan of the addresses it will try, and reports
  both of its ceilings. The local address and the default gateway are listed with the reason
  they were skipped rather than silently dropped.
- **`linklet exec --agents <a,b,c>`**: one command across several machines, one labelled
  result per target in the order they were given, and **a machine that refused told apart from
  one that could not be reached**. A target that panics does not take the report with it.
- **`linklet probe`**, which answers whether an agent is *working* rather than only
  *listening* -- see under **Fixed** for why that is not the same question. Four documented
  exit codes.
- **`tools/linklet-supervise.ps1`**: restarts an agent that died and one that wedged, with
  capped backoff, killing whatever holds the port rather than whatever has the expected name.
- **`linklet-agent --log <file>`** (or `LINKLET_LOG`), which appends two lines per request --
  when it was taken and when it was answered, with the duration. A request that never finishes
  writes only the first line, and that is what names it.
- **A documented way to start an agent so it outlives its console**, in `docs/smoke.md`,
  including why the scheduler has to run a script file rather than a command line.

### Changed

- **The wire protocol is frames rather than hand-written HTTP.** A request line, headers and a
  `Content-Length` became a six-byte header whose failure modes are enumerated in
  `crates/linklet-core/src/frame.rs`. The reading surface facing the network got smaller,
  which was the point.
- **The protocol's distinction is stronger for the loss of the status code.** "A command ran
  and failed" and "a request could not be made" were 200-versus-400; they are now a result and
  a refusal, and a result holding an exit code of 1 cannot be misread as a transport failure
  because it is not one.
- **Every command's flags are read by one parser** (`linklet_core::arguments`). They had been
  parsed per command, nine copies of which had drifted into two different messages for the
  same mistake, with nothing to compare them. The one line that parser has to draw is between
  a flags-only command, where an unrecognised `--flag` is a typo, and a command with a tail,
  where it belongs to the command being run.

### Fixed

- **A reply too large to frame was reported as a network failure.** A command that produced
  20 MB ran to completion on the target and the caller was told the agent "closed the
  connection without answering". The frame's ceiling is 16 MiB and there was no way to say so;
  the agent now refuses by name and quotes both sizes.
- **A command's output that was not UTF-8 was dropped silently.** `tasklist` on a Chinese
  installation returns bytes that are not UTF-8, and the caller was told there was no output.
  Non-UTF-8 output is now returned with a note saying how it was read.
- **A listening socket is not a working agent.** An agent wedged on a lock keeps its listening
  socket open, so a connect check calls it healthy while every real call times out. Measured
  against a socket that accepted and never answered: `check` said `live` and the probe said
  `no reply within 2000 ms`. `linklet probe` completes a handshake and reads a reply instead,
  which is what makes the supervisor worth having.
- **A receiver's refusal never reached the sender.** The sender streamed the body before
  reading the answer, so a receiver that refused wrote its reply and then dropped a socket
  with unread bytes in its queue -- which Windows resets, destroying the reply in transit. The
  manifest is answered before the body is sent, and a refusal is delivered with a half-close
  and a drain.
- **A killed transfer left its `.part` file behind**, so the next attempt at the same path
  found a file nobody wrote and could not say where it came from.
- **`spawn`'s output path never went through the transfer root.** Every other write in this
  protocol is rooted; this one went straight to the filesystem, so a relative path resolved
  against the agent's working directory and a `..` in it was never refused -- it wrote the file
  wherever the agent could. Found by driving this release candidate on a real machine, and by
  tests that had only ever passed an absolute path built from the root.

### Earlier in this version

Landing between 0.1.0 and 0.2.0, in the order they were built. Kept because each one is a
decision a reader may meet again; see the commit that carries it for the argument.


### Added

- **A handshake, so a recorded session cannot be read by someone who learns the
  secret later.** Both sides generate an X25519 key pair per handshake and discard
  the private half, so the session key depends on a value that no longer exists
  anywhere. The shared secret still authenticates the exchange: an attacker in the
  middle can complete a handshake with both ends and still cannot open a byte,
  because the session they build is not the one either end built.

### Added

- **The command and its output cross the network sealed.** A sealed call is two
  messages on one connection: the handshake, then the command sealed under the
  session it produced. The handshake cannot protect itself -- the initiator cannot
  derive a key until it has the responder public key -- so the exchange comes
  first and the command follows it. The alternative, one round trip with the
  command sealed under the token alone, leaves the command readable by anyone who
  later learns the token, which is the wrong half to protect.
- A sealed call needs a token, and the client says so **before opening a socket**
  rather than after a round trip. The token is what authenticates the handshake,
  so without one there is no call to make.

### Added

- **`tools/smoke.ps1`, the real-machine layer.** Seven claims against one target,
  taking an address and knowing nothing about where it came from. It needs no
  administrator rights, deliberately: the states that would need them are produced
  by whoever owns the machine, once, by hand. The first version of this said that
  adding a firewall rule needed host privileges, which was wrong -- the rule lives
  inside the guest, and `linklet exec` is a thing that runs commands inside the
  guest.
- It holds all seven claims against this machine over its LAN address. **It has
  never run against a second machine**, and the claim it exists for -- that the
  firewall blocks the agent port on a real deployment -- is still untested.

### Changed

- **The JSON codec is `serde_json`.** Six hundred and seventy-two lines of
  hand-written parser and writer became three hundred and forty-five: the domain
  enum, and the two conversions between it and `serde_json`. It was correct and it
  was tested, and it was still the wrong call -- it parsed untrusted input from the
  network, which is the last place to keep code whose bugs only a fuzzer finds. All
  call sites are unchanged, and the thirty codec tests now test the conversion.
- **The tests stopped asserting error wording and exact offsets, and that is a
  real reduction in what is checked.** Eleven tests pinned the phrases a parser in
  this repository produced ("unknown escape", "incomplete literal"). Those phrases
  now come from `serde_json`, so pinning them would pin a dependency
  internals -- a test that goes red on a patch release while nothing is wrong, which
  trains a reader to ignore it. What is still asserted is the part that was ever a
  contract: malformed input is refused, the refusal says something, and the position
  it reports lies inside the input and moves when the problem moves. The offset
  convention also differs by one byte -- `serde_json` points into a bad literal
  rather than at its first character -- and that difference is recorded rather than
  smoothed over.
- **Rule 1 was rewritten, not broken.** `linklet-core` may now depend on crates
  that do no I/O. The old wording said "depends on no crate", and that is satisfied
  by a rule rather than a reason, so it stayed in force past the point where its
  reason applied. The gate now takes a named allowlist where each entry carries a
  sentence saying why that crate is pure computation.

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

### Added

- **The command and its output cross the network sealed.** A sealed call is two
  messages on one connection: the handshake, then the command sealed under the
  session it produced. The handshake cannot protect itself -- the initiator cannot
  derive a key until it has the responder public key -- so the exchange comes
  first and the command follows it. The alternative, one round trip with the
  command sealed under the token alone, leaves the command readable by anyone who
  later learns the token, which is the wrong half to protect.
- A sealed call needs a token, and the client says so **before opening a socket**
  rather than after a round trip. The token is what authenticates the handshake,
  so without one there is no call to make.

### Added

- **`tools/smoke.ps1`, the real-machine layer.** Seven claims against one target,
  taking an address and knowing nothing about where it came from. It needs no
  administrator rights, deliberately: the states that would need them are produced
  by whoever owns the machine, once, by hand. The first version of this said that
  adding a firewall rule needed host privileges, which was wrong -- the rule lives
  inside the guest, and `linklet exec` is a thing that runs commands inside the
  guest.
- It holds all seven claims against this machine over its LAN address. **It has
  never run against a second machine**, and the claim it exists for -- that the
  firewall blocks the agent port on a real deployment -- is still untested.

### Changed

- **The JSON codec is `serde_json`.** Six hundred and seventy-two lines of
  hand-written parser and writer became three hundred and forty-five: the domain
  enum, and the two conversions between it and `serde_json`. It was correct and it
  was tested, and it was still the wrong call -- it parsed untrusted input from the
  network, which is the last place to keep code whose bugs only a fuzzer finds. All
  call sites are unchanged, and the thirty codec tests now test the conversion.
- **The tests stopped asserting error wording and exact offsets, and that is a
  real reduction in what is checked.** Eleven tests pinned the phrases a parser in
  this repository produced ("unknown escape", "incomplete literal"). Those phrases
  now come from `serde_json`, so pinning them would pin a dependency
  internals -- a test that goes red on a patch release while nothing is wrong, which
  trains a reader to ignore it. What is still asserted is the part that was ever a
  contract: malformed input is refused, the refusal says something, and the position
  it reports lies inside the input and moves when the problem moves. The offset
  convention also differs by one byte -- `serde_json` points into a bad literal
  rather than at its first character -- and that difference is recorded rather than
  smoothed over.
- **Rule 1 was rewritten, not broken.** `linklet-core` may now depend on crates
  that do no I/O. The old wording said "depends on no crate", and that is satisfied
  by a rule rather than a reason, so it stayed in force past the point where its
  reason applied. The gate now takes a named allowlist where each entry carries a
  sentence saying why that crate is pure computation.

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

- **The handshake is authenticated by a shared secret, not by a certificate.** Whoever holds
  the token can talk to the agent; there is no notion of which caller it is, so there is no
  per-caller revocation and no audit trail. A deployment that needs those needs identities.
- **No cipher agility.** One cipher, one curve, one key derivation, chosen at build time.
- **This version does not negotiate its protocol**, so an old host and a new agent fail with
  an unknown `op` rather than with a version. `docs/VERSIONING.md` records what that costs
  and what the next version has to do about it.
- **The supervisor is a process, not a service.** Nothing watches it, so a dead supervisor
  stops restarting a dead agent. `docs/smoke.md` says so where it documents it.
- No automated run against a second machine, and **no CI at all**: `tools/verify.ps1` is
  run by hand. `docs/testing.md` says what that costs.
- The MCP surface has never been read by a model with no other context. The tests check that
  the descriptions are short; they cannot check that they are understandable, and that is
  the claim the milestone rests on.
- `--at-once` is a constant rather than a flag. Ten unreachable machines and sixty behave the
  same way, so nobody has wanted a different value yet; a flag added before anyone asks is
  how a command line grows arguments nobody uses.

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

### Added

- **The command and its output cross the network sealed.** A sealed call is two
  messages on one connection: the handshake, then the command sealed under the
  session it produced. The handshake cannot protect itself -- the initiator cannot
  derive a key until it has the responder public key -- so the exchange comes
  first and the command follows it. The alternative, one round trip with the
  command sealed under the token alone, leaves the command readable by anyone who
  later learns the token, which is the wrong half to protect.
- A sealed call needs a token, and the client says so **before opening a socket**
  rather than after a round trip. The token is what authenticates the handshake,
  so without one there is no call to make.

### Added

- **`tools/smoke.ps1`, the real-machine layer.** Seven claims against one target,
  taking an address and knowing nothing about where it came from. It needs no
  administrator rights, deliberately: the states that would need them are produced
  by whoever owns the machine, once, by hand. The first version of this said that
  adding a firewall rule needed host privileges, which was wrong -- the rule lives
  inside the guest, and `linklet exec` is a thing that runs commands inside the
  guest.
- It holds all seven claims against this machine over its LAN address. **It has
  never run against a second machine**, and the claim it exists for -- that the
  firewall blocks the agent port on a real deployment -- is still untested.

### Changed

- **The JSON codec is `serde_json`.** Six hundred and seventy-two lines of
  hand-written parser and writer became three hundred and forty-five: the domain
  enum, and the two conversions between it and `serde_json`. It was correct and it
  was tested, and it was still the wrong call -- it parsed untrusted input from the
  network, which is the last place to keep code whose bugs only a fuzzer finds. All
  call sites are unchanged, and the thirty codec tests now test the conversion.
- **The tests stopped asserting error wording and exact offsets, and that is a
  real reduction in what is checked.** Eleven tests pinned the phrases a parser in
  this repository produced ("unknown escape", "incomplete literal"). Those phrases
  now come from `serde_json`, so pinning them would pin a dependency
  internals -- a test that goes red on a patch release while nothing is wrong, which
  trains a reader to ignore it. What is still asserted is the part that was ever a
  contract: malformed input is refused, the refusal says something, and the position
  it reports lies inside the input and moves when the problem moves. The offset
  convention also differs by one byte -- `serde_json` points into a bad literal
  rather than at its first character -- and that difference is recorded rather than
  smoothed over.
- **Rule 1 was rewritten, not broken.** `linklet-core` may now depend on crates
  that do no I/O. The old wording said "depends on no crate", and that is satisfied
  by a rule rather than a reason, so it stayed in force past the point where its
  reason applied. The gate now takes a named allowlist where each entry carries a
  sentence saying why that crate is pure computation.

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
