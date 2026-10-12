# Rules

## Reading

- Before work, read `docs/retrospective.md`. `README.md` defines the tool;
  `docs/ROADMAP.md` defines scope; `docs/INDEX.md` maps the remaining docs.
- Before questioning a design, read `docs/decisions.md`; before changing a rule,
  read `docs/rationale.md`.
- Read the relevant guide before changing its area: `docs/MCP.md` (tools),
  `docs/framing.md` (connections), `docs/transfer.md` (file transfers),
  `docs/VERSIONING.md` (wire changes or releases), `docs/VERIFYING.md` (workflows).
- Read `docs/machine.md` for unexpected command failures and `docs/VERIFYING.md`
  for unexpected verification results. Read `docs/smoke.md` before claiming LAN behavior.

## Work style

- State assumptions and verifiable success criteria before implementation. Surface
  competing interpretations and simpler approaches; clarify uncertainty before proceeding.
  For multi-step work, give a short plan with a check for each step.
- Build only what was requested. No speculative features, single-use abstractions,
  unrequested configuration, or handling of impossible cases. Prefer the smallest solution.
- Every changed line must serve the request. Match existing style; avoid unrelated
  cleanup, refactoring, or formatting. Remove only dead code your change creates;
  report pre-existing dead code instead of deleting it.

## 1. Dependency direction

`cli -> adapters -> core`. Core performs no I/O; mature libraries are welcome.
Prefer established libraries to hand-written general-purpose code. Keep I/O in
adapters behind core traits. Record each core dependency and its pure use in
`crates/linklet-core/tests/architecture.rs`; do not call its I/O APIs from core.

## 2. Tests before implementation

Write and run the test first; observe failure for the intended behavior, not a
compile error or a test that never ran. For refactors, verify before and after.
Use the cheapest layer that observes the behavior; see `docs/testing.md` and
`docs/LEARNING.md`. Make the smallest change, then verify the success criteria.

## 3. Definition of done

All four checks must pass; update docs made untrue by the change in the same commit.
`tools/verify.ps1` is the shared entry point. Its required gates are:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

## 4. Commits

One logical change per commit. If its revert needs "and", split it.
Use `<type>: <imperative summary>`, under 72 characters; explain why when needed.
Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`. See `docs/COMMITS.md`.

## 5. Never commit

No secrets, tokens, logs, build output, or release archives.
Fix `.gitignore` instead of using `git add -f`.

## 6. Public API

Document every public item, including limits callers could misunderstand.
No `unwrap()` in library code.

## 7. Language

Use English and ASCII in committed files and commit messages.
Write non-ASCII test data as escapes.
