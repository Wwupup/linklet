# Rules

Rules only. The reason for each one is in `docs/rationale.md` -- read it when
you are about to change a rule, not before.

## Docs

| file | read it when |
|---|---|
| `README.md` | what the tool is, and what it deliberately is not |
| `docs/ROADMAP.md` | what to build next, and what was parked on purpose |
| `docs/LEARNING.md` | the task loop: red, spec, smallest change, verify |
| `docs/review-m1.md` | the standard M1 is judged against, published early |
| `docs/rationale.md` | why a rule below exists |
| `docs/INDEX.md` | the map of this project, for keeping it current |

## 1. Dependency direction

`linklet-core` does no I/O and depends on no crate.

- The compiler enforces it: `core` has no `[dependencies]`, and
  `tests/architecture.rs` fails if one appears.
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
