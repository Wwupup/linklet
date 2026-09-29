# Rules for working in this repository

This file is for an AI agent (and for a human who wants the same constraints).
It deliberately contains only rules that are **checkable** or that a reasonable
person would otherwise get wrong. Explanation of *why* each rule exists lives
next to the rule, in one line -- a rule whose reason is not obvious gets
"cleaned up" by the next person, and then it is not a rule any more.

Keep this file short. Every line added here is a line every future session must
read, and a rule that is not enforced will be ignored, which teaches the reader
that the rest can be ignored too.

## 1. Dependency direction

`linklet-core` must not do I/O, know about the OS, or depend on any crate.

- Enforced by the compiler: `linklet-core` has no `[dependencies]` section, and
  the test `architecture::core_has_no_dependencies` fails if one appears.
- The direction is `cli -> adapters -> core`. Never the reverse.
- If you are about to add a dependency to `core`, the answer is almost always
  that the new code belongs in `adapters` behind a trait that `core` defines.

## 2. Tests before implementation

Write the test first and run it. It must fail, and it must fail **for the
reason you intended** -- not because it does not compile for an unrelated
reason, and not because you forgot to call it. A test that has never been
observed failing is not evidence of anything.

`cargo test --workspace` must be green before a commit.

## 3. Definition of done

A change is done when all of these hold:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

...and the documentation that the change makes untrue has been updated in the
same commit. A doc that describes last week's behaviour is worse than no doc,
because it is believed.

The fourth line is not decoration: `cargo doc` treats a broken link between two
documentation comments as a warning and carries on, so a `[`PortMissing`]` that
stopped resolving stays broken and stays invisible.

## 4. Commits

One logical change per commit. Message form:

```
<type>: <what changed, imperative>

<why it changed, if it is not obvious from the what>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`.
If the message needs the word "also", it is two commits.

## 5. Never commit

Secrets, tokens, logs, build output, release archives. See `.gitignore`; if you
find yourself reaching for `git add -f`, stop and fix the ignore rule instead.

## 6. Public API

Every public item has a doc comment saying what it does and, where a caller
could reasonably guess wrong, what it does *not* do. No `unwrap()` in library
code: a returned error is information, a panic is a bug report from a user.

## 7. Language

All identifiers, comments, doc comments, and documentation in English. The
project's upstream (lanlink) is ASCII-only for tooling reasons and this repo
keeps the same rule: no non-ASCII bytes in committed files except where a test
needs them as data, in which case they are written as escapes.
