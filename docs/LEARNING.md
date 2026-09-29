# How to work on this repository

Short on purpose. This is the procedure, not the theory -- the theory is in the
comments next to the code it explains.

Take one task. A task is one test that does not pass yet.

## The loop

1. **Watch it fail first.**
   `cargo test -p linklet-core --test target_parsing single_target_with_port`
   It has to fail, and it has to fail because the behaviour is missing. A test
   that has never been seen failing is not evidence of anything -- it might be
   failing to compile, or never running at all.

2. **Read the tests as the specification, not the doc comment.**
   `crates/linklet-core/tests/target_parsing.rs` is the truth of what the
   function must do. Where the two disagree, the test wins and the comment is
   the bug.

3. **Make the smallest change that passes one test.**
   Then run the whole suite. Guessing at four tests at once and running them
   together means a failure tells you *that* something is wrong, not *which
   assumption* was wrong.

4. **Ask whether the test was worth it.**
   If a test cannot fail, delete it: it costs time on every run and buys
   nothing. If it failed for a reason you did not intend, you have learned
   something -- fix the test before the code.

5. **Only then, verify the whole thing.**

   ```sh
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
   ```

   All four, every time. They take under a second on a warm cache, so there is
   no reason to skip one, and every reason not to: a check that is allowed to
   fail once is allowed to fail forever.

6. **Commit one logical change.** See `AGENTS.md` rule 4.

## When you are stuck

In rough order of how often each one helps:

- **Write the test for the case you believe cannot happen.** That is usually
  where the bug is, and writing it down forces you to say what you actually
  expect instead of assuming you know.
- **Write the assertion before the type.** Decide what a caller should be able
  to say, then design the type that lets them say it. (`Port::new(0)` is not
  rejected by a check; it is rejected by `u16` not being able to hold a
  negative number, and by the range check happening once, in one place.)
- **Read the test file top to bottom as prose.** The test names are the
  specification, in order.
- **Ask why the rule exists before working around it.** Every rule in
  `AGENTS.md` has its reason written next to it. If the reason turns out not to
  apply, change the rule -- but only after saying out loud which reason stopped
  applying. A rule that gets worked around silently stops being a rule.

## The one thing not to do

Do not add a dependency to `linklet-core` to solve a problem. It will not
build -- `tests/architecture.rs` fails, on purpose. If the code genuinely needs
I/O, the trait belongs in `linklet-core` and the implementation belongs in
`linklet-adapters`, and that is milestone M2.
