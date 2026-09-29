# Read this before changing a rule

The rules are in `AGENTS.md`, one or two lines each. This file holds the *why*
for each of them.

The split is deliberate. `AGENTS.md` is loaded at the start of every session,
so each line in it is paid for every time. The reasoning is needed only when
someone is about to change a rule -- maybe once a month. Mixing the two means
every session pays for an explanation it does not need, and the rules get lost
in the prose, which is how a rule file stops being read and then stops being
followed.

**Nothing here is a rule.** If this file and `AGENTS.md` disagree, `AGENTS.md`
wins and this file is the bug.

## 1. Dependency direction

A rule that depends on someone remembering it is not a rule. Making `core` a
crate with no dependencies turns "do not do I/O in the core" into a compile
error, so it cannot be broken by accident at 2am.

The direction `cli -> adapters -> core` is what keeps decisions testable. Once
`core` can open a socket, its tests need a network, and a test that needs a
network stops being run -- and then the behaviour it covered is wrong without
anyone noticing.

If you want to add a dependency to `core`, the new code almost always belongs
in `adapters`, behind a trait that `core` defines. That inversion is the point
of the whole layout, and it is milestone M2.

## 2. Tests before implementation

A test that has never been seen failing is not evidence of anything. It might
be failing to compile for an unrelated reason, or not running at all, or
asserting the opposite of what its name says. Watching it fail -- for the
reason you intended -- is the only thing that distinguishes a test from a
comment with `assert` in it.

This repository has already been bitten by the quiet version of this: an
architecture test that failed because of an empty table header rather than the
rule it was named after, and a mutation test that couldn't run because the
network was down. **Both looked exactly like the check working.**

## 3. Definition of done

Four checks, all mechanical, all fast. The value is that nobody has to decide
whether a change is finished -- the question has an answer that takes a second
to compute.

The documentation line is there because a document that describes last week's
behaviour is worse than no document: it is believed. It is also the only item
here that a machine cannot check, which is exactly why it is written down.

The doc check specifically is not decoration. `cargo doc` treats a broken link
between two doc comments as a warning and carries on, so a `[PortMissing]` that
stopped resolving stays broken and stays invisible. With the flag, it is an
error and the build fails. That was verified by deliberately breaking a link:
exit code 101, and 0 again after restoring it.

## 4. Commits

One logical change per commit, because a commit is the unit of "undo this" and
of "why did this change". A commit that does three things cannot be reverted
without reverting two things you wanted, and its message has to be vague enough
to cover all three.

"If the message needs the word 'also', it is two commits" is a test, not a
style note -- the word is a reliable symptom. So is the sharper version in
`docs/COMMITS.md`: write the revert message, and if it needs the word "and",
the commit is not atomic.

This rule was in `AGENTS.md` from the first commit and was broken anyway: every
message was written in Chinese while rule 7 said English, and nothing noticed.
That is why the mechanical half now lives in
`crates/linklet-core/tests/commit_message.rs`. A rule nothing enforces is a
wish, and this repository has the receipt.

## 5. Never commit

A committed secret has to be rotated, not deleted: the history keeps it. Fixing
the ignore rule is cheap; rotating a token across machines is not.

Build output and release archives are a function of the source. Storing them
means every clone pays for them forever -- the upstream project this one is
modelled on carries 147 MB of release zips in `dist/`, which is the concrete
cost of not having this rule.

## 6. Public API

A public item with no doc comment is an unexplained decision, and the next
person either guesses or reads the implementation. Both cost more than the
comment.

No `unwrap()` in library code, because the two failure modes are not
equivalent: a returned error is information the caller can act on, and a panic
in a library is a bug report arriving from a user, with no context.

## 7. Language

One language for identifiers and documentation, so that a search finds all of
it. The upstream project (lanlink) is ASCII-only for a tooling reason --
byte-level checks, and a `git` on Windows that rewrites line endings -- and this
repository keeps the same rule so the two can share files and tooling.

Non-ASCII in a test is data, not prose, so it is written as escapes. That is
not a workaround; it makes the test's input visible in a diff.
