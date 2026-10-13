# This machine

Facts about this development machine that have each cost someone time, so that a fresh reader
does not pay for them again. **Every one of them produced a wrong conclusion before it was
found**, which is why they are written down rather than remembered.

Read this when a command fails in a way that looks like the code is wrong, when driving a real
target, or when the registry or the network seem unreachable. Add to it when a machine fact
costs you time: a fact found twice by hand belongs here.

conclusion before it was found.

- **A stale git proxy made the crate registry look unreachable.** The real proxy
  listens on `7890`; `~/.gitconfig` had `http.proxy = http://127.0.0.1:7892`,
  where nothing listened. Cargo reads git's configuration because of
  `git-fetch-with-cli`, so cargo failed while everything using the Windows system
  proxy worked. The stale entries have been removed; if the registry looks
  unreachable again, check `git config --global --get http.proxy` before
  concluding anything about the network.
- **`cargo search` cannot be used to judge the network.** It does not support the
  `replace-with` registry replacement and fails with a message about sources, not
  about connectivity. `cargo fetch` is the honest test. This mistake cost an hour
  and led to a hand-written SHA-256 -- see `docs/decisions.md` D1.
- **The toolchain is pinned to 1.95.0 MSVC, and the registry is rsproxy.** Adding
  a dependency works; there is no reason to hand-write a library that exists.
- **The real-machine smoke test needs no administrator rights.** The seven claims in
  `docs/smoke.md`, made against a machine running `linklet-agent` with the port
  allowed. There is no script for it any more; the claims are the list.
- **A closed loopback port in this WSL distribution times out instead of being refused.**
  Measured directly, without this tool: bare Python sockets connecting to `127.0.0.1` on
  ports 1, 9 and 54321 each raise `TimeoutError` after the full three seconds, where on
  Windows the same connect is refused at once. So `linklet check 127.0.0.1:1` says
  `no answer within 5 s` here and `refused` there, and **both are correct**: the probe
  reports what the operating system said, and here the operating system said nothing. It
  cost a test assertion (which claimed the word `refused`) and it will cost anyone who
  reads "no answer" as a firewall. `docs/ROADMAP.md` M11 has the run.
- **WSL2 forwards loopback**, so a Linux agent listening on `0.0.0.0:8792` inside the
  distribution is reachable from Windows as `127.0.0.1:8792` as well as on the
  distribution's own address. Verified both ways in M11.
- **The harness sandbox can fail before any command runs, and it looks like a broken
  toolchain.** Under `workspace-write`, every shell call failed with
  `SetNamedSecurityInfoW failed (Win32 5): grantWrite(E:\projects\linklet)` -- that mode
  needs `WRITE_OWNER` on the workspace root, which this account does not have. Nothing was
  wrong with the repository, with cargo, or with the network. What fixes it is the
  session's file policy (`danger-full-access`), so if it comes back, do not go looking for a
  fault in the tree.
- **The LAN target used for the M7 round is 192.168.100.2** (`WinDev2407Eval`, Windows 11
  22621 eval, 3.6 GB), reached through the `lanlink` MCP server. Three things about it cost
  time and will again: **lanlink's own agent already holds 8787**, so `linklet-agent` needs
  another port (8790 was used, with `--root C:\linklet\transfers`); the account is an
  administrator running **unelevated**, so the firewall rule has to be **program-scoped**
  (`-Program <the agent's path>`, which also survives a change of port) because a port rule
  cannot be added and a UAC prompt needs a desktop this link does not have; and a target
  with nothing listening presents as **"no answer"** rather than "refused", which reads
  exactly like a firewall that was never opened. The token is the operator's and is
  deliberately not written down here. `docs/smoke.md` has the rest.
- **`lan_spawn` with a `cmd /c ... > file` redirect captures nothing from the child.** The
  file is created and stays empty; the same command run through `lan_exec` writes its output
  normally. Measured with `hostname`: empty through the spawn call, the hostname through the
  exec call. What does land in the file is the *shell's* own output, so a lone `^C` in a
  redirected log is cmd's echo of a console close and not the program saying anything. Two
  conclusions were drawn wrongly from that empty file before it was checked -- so if a
  detached program's output matters, have the program write its own file. **The M10 round
  confirmed it from the other side**: the bench's `linklet-agent.exe` is still launched
  through that redirect, and `C:\linklet\agent.log` is 0 bytes for an agent that printed a
  banner and has answered every call since. An empty log is evidence about the launcher.
- **Windows commands preserve inner quotes.** The agent now uses
  `cmd.exe /D /S /C` with a raw, outer-quoted command string. Quoted executable
  paths, arguments, pipes, and redirection are supported. The caller still needs
  to preserve its intended string through its local shell. Commands run in the
  agent startup directory, under the agent account.
- **`actionlint` is installed at `%USERPROFILE%\.local\bin\actionlint.exe`** (1.7.12, digest
  checked against the release's `checksums.txt`), and `tools/verify.ps1` runs it when it is on
  `PATH`. Installing it was worth the ten minutes: the first push of the CI workflows came back
  with *"Node.js 20 is deprecated"* for `actions/checkout@v4`, and **no local check would have
  reported it** -- `actionlint` flags `@v3` as too old but not `@v4`, because its staleness
  threshold lags GitHub's. The pins are guarded by `tests/ci_workflow.rs` and reviewed at
  release time; `docs/VERSIONING.md` says how.
- **Nothing in this session can see GitHub's own answers.** There is no `gh`, no `act`, and no
  Docker on this machine, and no MCP server that reaches GitHub. What that means in practice:
  a workflow can be linted and its shell parsed here, and whether the runner accepts it is only
  known after a push. When a workflow fails, the message has to be brought back by hand --
  which is why `docs/VERSIONING.md` lists what to check *before* pushing instead.
- **The current WSL2 Ubuntu 24.04 environment can run the shared verification script.**
  The `ww` account has passwordless sudo. Rust 1.95.0 with rustfmt and Clippy, GCC
  13.3.0, PowerShell 7.6.6, and iproute2 were installed for the 2026-10-13 Linux
  verification. Source `~/.cargo/env` before using Cargo; shell profiles were not
  changed. Run tests as `ww`, with sudo used only for system package installation.
  Set `CARGO_TARGET_DIR` to `~/.cache/linklet-verification/target` to keep Linux
  artifacts on the WSL filesystem and separate from Windows builds. All four
  gates passed through `tools/verify.ps1`; see `docs/VERIFYING.md` for the command.
- **`Compress-Archive` writes no Unix mode, so it cannot build this project's release.** Measured
  on both sides of the same archive: an entry written by `Compress-Archive` comes out of `unzip`
  at mode **600** -- not executable, and not readable by anyone else -- where the same file written
  by Info-ZIP `zip` (3.0, the version on the Ubuntu runner image) comes back **755**, with plain
  files at 644. That is why `release.yml` assembles the archive on Linux, and why
  `tools/make_release.sh` extracts what it wrote and checks the bit instead of trusting the
  staging. It is also the reason this was found rather than shipped: the first archive holding a
  Linux binary would have been made by the Windows archiver.
- **A checkout here has no executable bit on a shell script**, because `git` on Windows does not
  record one -- `git ls-files --eol` reports the mode and `tools/make_release.sh` is mode 100644
  here and 100755 on a Linux clone. CI therefore calls it as `bash tools/make_release.sh`, not
  `./tools/make_release.sh`: calling it by name works on the runner and fails on the machine the
  file was written on.
- **Windows on this machine answers in Chinese, and a test that matches English shell wording
  fails looking like a code fault.** `cmd` reports a missing program in Chinese, in GBK bytes
  that do not even decode as UTF-8, so the wording check in
  `a_command_that_never_started_has_no_exit_code_and_says_so`
  (`crates/linklet-agent/tests/agent_server.rs`) failed while the behaviour was right -- the
  complaint did reach the caller, marked lossy, which is the design. The test now asserts that
  the complaint arrived rather than what it says. Any test that quotes a program's expected
  output will have the same fate on a machine with another display language.
- **`ping` gives a redirected file nothing for about three seconds, and a test that reads it
  sooner reads an empty file.** Measured three ways on this machine: `cmd /C "ping ... > file"`,
  a handle inherited from the parent, and `ping.exe` started directly all leave the file at 0
  bytes until roughly 2.5-3 s after start, and after that it grows a line at a time. So a
  program's redirected output is *eventually* there, not *immediately* there -- and a **forced**
  stop before the first flush discards what the program had buffered, so the file stays empty
  and the tool looks broken. This cost time in the spawn test
  (`crates/linklet-client/tests/ps.rs`), whose file read happened right after a `taskkill /T /F`:
  the tool was correct, the test was reading too early. It now waits for the marker and reads
  the bytes rather than a string, because `ping`'s text here is GBK and `read_to_string` refuses
  it. If a detached program's output matters and it must appear at once, have the program flush
  its own file.
