# Commits

The rule is one line in `AGENTS.md`. This is how to satisfy it.

## What a commit is for

Not "a save point". Three things read a commit, and each one needs something
different:

| reader | needs |
|---|---|
| `git bisect` | each commit to build and pass its own tests |
| `git revert` | one thing to undo, and nothing else caught in it |
| a person in two years | *why*, because the *what* is in the diff |

A commit that fails its own tests is broken for the first reader. A commit that
changes three things is broken for the second. A commit whose message repeats
the diff is broken for the third.

## The test for "one logical change"

Write the revert message. If it needs the word "and", or it does not make
sense, the commit is not atomic:

```
Revert "feat: add the check command, with the output contract in the core"
```

That reads fine, so it is one change. This does not:

```
Revert "fix: bugs, and also update docs"
```

## The subject line

```
<type>: <what changed, imperative, lowercase, no full stop>
```

- **Imperative**: "add", not "added" or "adds". It completes the sentence
  *"applying this commit will ..."*, which is what `git` itself writes when it
  reverts or cherry-picks.
- **Types**: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`.
- **Under 72 characters**, so it does not wrap in a terminal or a log view.
- **Concrete about the code**, not about the process. `fix: stop the probe
  reporting a refusal as silence` beats `fix: address review feedback`, which
  will mean nothing in a month.

## The body

Optional, and it answers **why**. The diff already says what.

Write it when a reader could reasonably ask "why on earth would you do that?"
For a two-line change with an obvious reason, no body is better than a padded
one.

- Wrap at 72 characters.
- Blank line between subject and body. `git log --oneline` and every tool that
  reads subjects depend on it.
- `git log` is read far more often than it is written, so the effort goes here.

### A worked example

Real, from this repository:

```
feat: let the core declare the probe it needs, and adapters supply it

The core cannot open a socket, so it declares a Probe trait and the TCP
implementation lives in linklet-adapters. The dependency arrow is
adapters -> core, not the reverse. That direction is the whole point: the
core's 17 tests cover timeouts, refusals and resolution failures through a
fake in under a second. The same behaviour tested through real sockets
would need a machine that is off, one that refuses, and one that drops
packets, at ten seconds a case.
```

Note what the body contains and what it does not. It gives the *reason* for the
direction, and the cost of getting it wrong. It does not list the files, or
mention that a test was added -- `git show --stat` says both.

### The same content, written badly

```
feat: M2 dependency inversion

- added probe.rs
- added tcp.rs
- added tests
- fixed some issues
- also cleaned up
```

Every line is either in the diff already, or meaningless. "Fixed some issues"
is a promise the reader cannot check. And if the message needs "also", it was
two commits.

## Emoji, and other decoration

None. `git log` is read by tools as often as by people, and a prefix that
carries no type information competes with the one that does.

## Try it before it is committed

The message can be checked before it exists. `git commit` without `-m` opens an
editor; save the message, read it back as if you were the person who finds it
during a `bisect` in a year, and edit before saving.

`tests/commit_message.rs` does the mechanical half of this -- type, length,
ASCII, a blank line before the body -- and will fail the suite on the worst
offences. It has no opinion about whether the message is any use.
