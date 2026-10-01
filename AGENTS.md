# Rules

Rules only. The reason for each one is in `docs/rationale.md` -- read it when
you are about to change a rule, not before.

## Docs

| file | read it when |
|---|---|
| `README.md` | what the tool is, and what it deliberately is not |
| `docs/ROADMAP.md` | what to build next, and what was parked on purpose |
| `docs/LEARNING.md` | the task loop: red, spec, smallest change, verify |
| `docs/testing.md` | which layer a test belongs in, and what it costs |
| `docs/MCP.md` | the agent-facing surface: what is on it, and what is not |
| `docs/COMMITS.md` | how to write a commit, with worked examples |
| `docs/review-m1.md` | the standard M1 is judged against, published early |
| `docs/rationale.md` | why a rule below exists |
| `docs/decisions.md` | before arguing with a choice that looks wrong -- it may already be recorded as wrong |
| `docs/retrospective.md` | before starting work, so the six failures are not paid for twice |
| `docs/framing.md` | before touching the wire format or a connection |
| `docs/transfer.md` | before moving a file: fourteen failure modes with a defence for each |
| `docs/smoke.md` | before claiming anything works on a real machine |
| `docs/VERSIONING.md` | before changing the wire, or cutting a release |
| `docs/INDEX.md` | the map of this project, for keeping it current |

## 1. Dependency direction

`linklet-core` does no I/O, and depends on no crate that does.

- Enforced by `tests/architecture.rs`, which allows a named list of
  pure-computation crates and refuses everything else. Adding one means writing
  down why it does no I/O.
- The reason, since the wording changed once already: core tests need no network,
  no files and no cleanup, so they run in milliseconds. A crate that computes and
  touches nothing does not threaten that. "Depends on nothing" was satisfied by a
  rule rather than a reason, and it stayed in force past the point where its reason
  applied -- see `docs/decisions.md`.
- Direction is `cli -> adapters -> core`. Never the reverse.
- Something in `core` needs I/O? It belongs in `adapters`, behind a trait
  `core` defines.

## 2. Tests before implementation

Write the test first. Run it. It must fail, and fail for the reason you meant --
not a compile error, not a test that never runs. A test never seen failing is
not evidence of anything.

`cargo test --workspace` is green before a commit.

## 3. Definition of done

All four pass, plus the docs the change makes untrue are updated in the same
commit.

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## 4. Commits

One logical change per commit -- write the revert message, and if it needs the
word "and", it is two commits. `<type>: <what, imperative>`, under 72
characters, then why if it is not obvious. Types: `feat` `fix` `refactor`
`test` `docs` `chore`. English, ASCII. The rest, with worked examples, is in
`docs/COMMITS.md`. Checked by `tests/commit_message.rs`.

## 5. Never commit

Secrets, tokens, logs, build output, release archives. If you reach for
`git add -f`, fix `.gitignore` instead.

## 6. Public API

Doc comment on every public item: what it does, and what it does not where a
caller could guess wrong. No `unwrap()` in library code.

## 7. Language

English everywhere. ASCII in every committed file; non-ASCII test data is
written as escapes.

## 8. This machine

Not a rule -- the facts about this development machine that cost someone time, so
that a fresh reader does not pay for them again. Each one produced a wrong
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
- **The real-machine smoke test needs no administrator rights.**
  `pwsh tools/smoke.ps1 -Target <host:port>` against a machine running
  `linklet-agent` with the port allowed. `docs/smoke.md` is the whole of it.
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
