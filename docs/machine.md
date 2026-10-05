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
- **A command sent to a target cannot carry a quote.** The agent runs commands through
  `cmd /C`, and three layers each rewrite quote characters on the way: PowerShell's
  native-command quoting (which turns `"` into `\"`), Windows argument quoting, and `cmd`'s
  own rule about a first character that is a quote. The result reads like the program does
  not exist: `'\"powershell -NoProfile -C \"\"...\"\"\"' is not recognized as an internal or
  external command`. Measured forms that failed: `cmd /S /C "..."`, `powershell -Command
  "..."`, and either of those passed through PowerShell. What works is a command with **no
  quotes at all** -- an absolute path with no space in it, and builtins like `certutil` and
  `type`. `C:\linklet\transfers` was chosen for exactly that reason, and on this machine
  `%TEMP%` is `C:\Users\wuwei\AppData\Local\Temp`, which has no space either. `linklet exec`
  also runs the command in the *agent's* working directory, not in the caller's: a relative
  path is relative to wherever the agent was started.
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
- **WSL has no `pwsh`, and getting one there is not practical from here.** `tools/verify.ps1` is
  what CI runs on both platforms, so the obvious way to check the Linux leg locally is to run it
  in the WSL distribution -- and Ubuntu 24.04 there has no PowerShell, `sudo` asks for a password,
  and the released tarball is about 75 MB arriving at **10-20 KB/s, measured on both the WSL path
  and the Windows one** (the proxy on `7890` above was not listening when this was tried). The
  fallback is the one M11 used, and it is what the Linux leg was checked with: **the four commands
  run directly in WSL, in the same order**, which is every gate the script runs there except
  `actionlint`.
