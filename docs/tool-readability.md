# Can a reader who has never seen this use it?

The claim M5 rests on is that the tool descriptions are short **and** sufficient
-- that a reader with no other context can pick the right call. The other two
checks in that milestone count characters and forbid a description from naming
another tool. Neither can tell whether a description is *understandable*, and
that is the claim that matters.

So this is the experiment for it. It is written down because an experiment whose
method lives in someone's memory is an anecdote.

## What is measured

Three questions per request, in order of importance:

1. **Which tool**, or "none of these" when the request is not answerable.
2. **What arguments**, which is where a short description is most likely to fail:
   knowing `exec` exists is not the same as knowing it needs an agent address.
3. **Whether it asked**, when the request was ambiguous. A reader that guesses
   where it should have asked is worse than one that asks too often.

## What is handed to the reader

The `tools/list` reply, verbatim, and one request. Nothing else: no repository,
no documentation, no explanation of what linklet is. That is the whole point --
if the reader needs any of those, the descriptions are not sufficient and the
surface needs a manual, which is the failure this project exists to avoid.

The fixtures are in `tool-readability/`, and they are generated from the binary
rather than typed, so the experiment cannot drift from what the surface actually
says.

## The method

For each request, a reader is asked in a fresh session with no other context:

> You have these tools available. Reply with the single tool call you would
> make, as JSON with a `tool` and an `arguments` field, and nothing else. If no
> tool fits, reply with `{"tool": null, "reason": "..."}`.

Then the answers are compared against the expectations below, and **every
disagreement is recorded, not just the count**. A score with no failures listed
is a number nobody can learn anything from.

## Limits of this experiment, stated up front

These are not disclaimers; they decide how much the result is worth.

- **Same model family.** The readers are the same model that wrote the
  descriptions. It will find its own phrasing obvious in a way another family
  might not, so a good result is weak evidence and a *bad* result is strong
  evidence. The asymmetry is the useful part.
- **Small sample.** Ten requests is enough to find a description that cannot be
  acted on and not enough to rank two workable ones.
- **The rubric is mine.** "Correct" here means "what I would have called", which
  is the same circularity the experiment is trying to escape. Where a request
  admits two answers, the expectation says so and both are accepted.

## The requests

Nothing below names a tool, and each is phrased the way someone would say it out
loud. The expectation is what I would call correct before running the experiment,
written down first so it cannot be adjusted to fit the answers.

| # | the request | expected | why |
|---|---|---|---|
| 1 | "the box at 10.0.0.5 should be serving our debug port, is it up" | `check` with `targets: ["10.0.0.5:8787"]` | One port, one machine, nothing else needed. |
| 2 | "before I run the launch test on this machine I want to know the exe is there and no old copy is still running" | `testbed` with `spec` and `target` | The request is a *set of conditions*, not a command. The conditions live in the file, and the tool's job is to read them. |
| 3 | "run the build script on the target" | `exec` with `agent` and the command | One command, one machine. |
| 4 | "did the build pass" | **ambiguous** -- acceptable answers call `exec` to run it, or ask what to run | There is nothing on a machine that records whether a build passed, so a reader that answers as though there were has invented a tool. |
| 5 | "I want to know if the test box still has yesterday's output directory lying around" | `testbed` with `spec` on that path | A condition about state, which is what a specification is for. |

Requests 1 and 3 are the easy pair: they separate the two tools that both take a
`host:port`. If a reader confuses those, the descriptions of `check` and `exec`
are not doing their job, because the difference between "is it listening" and
"run this" is the one a caller must never get wrong.

Requests 2 and 5 are the pair that matters most: both describe *conditions* rather
than actions, and both need a specification file the reader has never seen. A
reader that answers `exec` is a reader that does not know what `testbed` is for,
whatever the description says.

## The result

Run on 2026-09-29, against the binary built from commit `6d95839`. Fixtures
generated from `linklet mcp` at that commit; readers were five fresh sessions,
each given the tool list and one request.

| # | the request | expected | what came back | verdict |
|---|---|---|---|---|
| 1 | is 10.0.0.5 serving our debug port | `check` | `check` with `targets: ["10.0.0.5:8787"]` | **correct** |
| 2 | exe is there and no old copy is running | `testbed` | `exec` with `dir /b *.exe & tasklist /fi "imagename eq launch-test.exe"` | **disagrees** |
| 3 | run the build script on the target | `exec` | refused: the agent's `host:port` was never given | **correct, and my expectation was wrong** |
| 4 | did the build pass | ambiguous | refused: no tool answers it, and the address and command are both missing | **correct, better than either answer I allowed** |
| 5 | does the test box still have yesterday's output directory | `testbed` | refused: `exec` is the only tool that could, it needs an address nothing supplies, and the path is unspecified | **defensible** |

### The shape of the result

Four of five readers refused to fabricate. Every refusal named the missing thing:
an agent address, a command, a path. Not one of them guessed a value to make a
call possible, and request 1's reader volunteered that its port came from the
schema's example rather than from the request.

That is the property worth having, and it is not the property M5 set out to test.
The milestone asked whether three short descriptions are enough to pick the right
call. What the run shows is that they are enough to **know when a call cannot be
made**, which is the failure mode an agent actually causes damage with -- a
plausible guess that looks like progress.

### Request 2, which is the finding

The reader reached for `exec` and composed `dir /b *.exe & tasklist ...`, which
does answer the question. It is not a misreading; it is a different and reasonable
route. What it shows is that `testbed`'s description says what the tool does and
never says **why it beats typing the equivalent command yourself**. A reader with
no context has no reason to prefer writing a specification file over one line of
`cmd`.

That gap is not fixable by making the description longer, and that is the
interesting part. "Prefer this when the conditions will be checked more than
once" is a sentence about *when* to use a tool, and the M5 rules exist to keep
exactly that kind of sentence out -- because a list of tools that each explain
when to use the others is the manual this project is a reaction to.

So a boundary of the approach has been found: **short descriptions select the
right tool when the request is already an action, and do not sell a tool whose
value is being reusable.** Whether that is a fixable weakness or a real limit is
the next question, and it is a better question than the one this experiment was
designed to answer.

### Request 3, where my expectation was the wrong one

I expected `exec`. The reader refused, because the request never says which
machine. I wrote the expectation while thinking about the tool list and not about
the request, which is the mirror image of the mistake this experiment exists to
catch: I assumed a fact that was not there.

It is recorded as a pass rather than a failure because the reader was right. An
experiment whose expectations are corrected after the fact proves nothing, which
is why the table above keeps the expectation in the column next to what happened
instead of replacing it.

### Two flaws in this run

Both are recorded rather than quietly redone, because both are instructive.

**The readers had more than the tool list.** The method says a reader gets the
tool list and one request, and nothing else. In practice the readers ran in this
workspace with file access, and the one answering request 3 **searched it** --
looking for an agent address and a build script -- before refusing. So the
isolation the method describes was not achieved: this measures a reader with the
tool list *and* a filesystem, which is not the same experiment.

It does not invalidate the refusals, which were about the request rather than the
environment. It does mean the result is evidence about a reader with more context
than intended, and the "nothing else" in the method section is currently a claim
this run does not support.

**One reader saw a different surface.** Request 5's prompt had `exec`'s timeout
description missing the word "seconds": `"the command may run before it is
killed"` where the binary says `"seconds the command may run before it is
killed"`. It did not change that answer -- the refusal was about a missing agent
address -- but one of five readers did not see the same tool list as the other
four.

The flaw matters more than the error: four of the five fixtures are **generated
from `linklet mcp`** precisely so that the experiment cannot drift from what the
surface says, and the fifth input -- the one typed by hand -- is the one that
drifted.

### What follows

The claim survives, narrowed and with a new question attached.

- **Enough:** three descriptions totalling 179 characters select the right call
  when a request is phrased as an action, and produce a refusal naming what is
  missing when it is not. Four of five readers refused rather than guessed.
- **Not enough:** nothing in the surface conveys that a tool is worth using over
  the obvious manual alternative. That information is either impossible to carry
  in 120 characters, or it has to live somewhere a reader will look before
  choosing -- and anything a reader must fetch first is a manual by another name.

The next experiment is not more requests. It is whether a description can carry a
reason to prefer its tool at all, or whether the answer is that this surface
should have fewer tools rather than better-described ones.
