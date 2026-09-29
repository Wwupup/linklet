# Review criteria for M1

Written and committed **before** the implementation, so the criteria cannot be
adjusted to fit whatever was written. That is the whole point of putting it
here: a rubric invented after the fact is a description of the code, not a
standard applied to it. It is also how any real review should work -- say what
"good" means first, then find out whether it is there.

What M1 is judged on, in priority order:

> **Applied, M1 complete.** The implementation is in
> `crates/linklet-core/src/target.rs`. The criteria below are kept exactly as
> they were written -- unedited -- because a rubric that gets tidied up after
> the fact stops being evidence that it came first. The one thing this file
> gained afterwards is this paragraph, saying what happened.
>
> Outcome against each heading:
>
> 1. **Passes, for the right reason.** 19 specification tests and 2 architecture
>    guards green. The red state was observed first: 19 failing, all on
>    `unimplemented!` at one line, not on a compile error.
> 2. **Decisions in one place.** The port range check appears once, at the
>    range test near the end of `parse_targets`. Nothing re-derives it.
> 3. **Refuses rather than guesses.** `"a:1,"`, `"a"` and `"a:99999999999999999999"`
>    all refuse; the huge port saturates to `u64::MAX` rather than being
>    reported as unparseable.
> 4. **Readable without the tests.** The eight rules in the doc comment are in
>    the same order as the code, one `if` each.
> 5. **Failure cases checked.** The specification was run red before the code
>    existed, and the whole suite was run after.
>
> One thing this review added to the criteria rather than found satisfied by
> them: rule 6 of the documented order (an empty host, `":80"`) had **no test**
> when the order was first written down, so the documentation was claiming
> behaviour nothing pinned. The test was added, and observed failing, before the
> implementation was written. That is recorded here because it is the failure
> mode this repository exists to avoid, and it happened in this repository.

## 1. Does it pass, and does it pass for the right reason?

```sh
cargo test -p linklet-core
```

Everything in `tests/target_parsing.rs` passes, plus the architecture guards. A
test made to pass by weakening the test is a failure, not a pass: **do not edit
`tests/`** except to add a case whose absence you can explain.

## 2. Are the decisions in one place?

The port range check and the "what counts as a spec" rule should each appear
**once**. Two checks that must agree are a future bug with a delay on it. A
reader should be able to point at one line and say "this is where a port is
allowed to be valid".

## 3. Does it refuse rather than guess?

The three places this is easy to get wrong, and what is expected:

| input | expected | the tempting wrong answer |
|---|---|---|
| `"a:1,"` | `EmptySpec` error | skip the empty entry |
| `"a"` | `PortNotANumber { port: "" }` | assume a default port |
| `"a:99999999999999999999"` | `PortOutOfRange { value: u64::MAX }` | "number too large to parse" |

A default applied here is not a convenience, it is a different language: `"a"`
means something this tool did not say.

## 4. Is it readable without the tests?

Names that state intent, no dead code, no `unwrap()` in `src/`. If a branch
needs a comment, the comment says **why**, not what -- the code already says
what.

## 5. Is there any evidence the author checked the failure cases?

A submission that only ran the happy path is visible: it is the one where
`"a:1,,b:2"` returns `Ok` and nobody noticed. Say which tests were run and what
was expected to fail.

---

## What I will not accept

Not stylistic preferences -- these are the ones that cost something later:

- **The test file modified to make the implementation easier.** The tests are
  the specification.
- **`#[allow(...)]` used to silence rather than to document.** An allow needs a
  scope and a deadline, or it is permanent.
- **A dependency added to `linklet-core`.** `tests/architecture.rs` fails, by
  design; if the need is real, it is M2.
- **Two passes over the input where one will do** -- for example trimming, then
  splitting, then trimming each piece again. Not a correctness bug; it is a
  sign the grammar was not decided before the code was written, and the next
  person will not be able to tell which pass is authoritative.
- **A "clever" one-liner.** The reference solution here is about 30 lines and
  every one of them can be explained. Shorter is not better; *clear* is better,
  and a chain of five iterator adapters that nobody can debug at midnight is
  neither.

## What I will say nothing about

Formatting (`cargo fmt` decides), naming of local variables, and whether the
code looks like mine. There is no style credit available in this review.
