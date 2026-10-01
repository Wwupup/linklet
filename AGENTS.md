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
| `docs/machine.md` | when a command fails in a way that looks like the code is wrong |
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
written as escapes. Checked by `tests/ascii_only.rs`, which exists because
seven em dashes reached one document from the same hand without any of them
being noticed.

## 8. This machine

The facts about this development machine that have each cost someone time -- the stale git
proxy that made the registry look unreachable, the sandbox mode that fails before any command
runs, what a command sent to a target does to quote characters -- are in `docs/machine.md`.

They are there and not here because a rule is something to obey and a machine fact is something
to look up: the rules above are short enough to read every time, and the facts in that file are
long enough that keeping them here buried the rules.

**Read it when a command fails in a way that looks like the code is wrong.**
