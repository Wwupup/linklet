# What this was, and what went wrong

A record of one rebuild, written at the end of it. It is a **decision** document in
the sense `docs/INDEX.md` uses: kept true by nothing, and it does not go stale
because it is a moment rather than a claim about the code.

It exists because the failures are the transferable part. The finished shape of
`linklet` is readable from the code; why it took eight commits to get there, and
which of the wrong turns were avoidable, is not.

---

## What got built

Five crates, 259 tests, one command that runs every gate.

```
linklet-core      decisions. No I/O. serde_json is the only dependency.
linklet-adapters  sockets and crypto: the only place either happens
linklet-agent     the target side
linklet-client    the host side
linklet-cli       argv in, text out, exit code
```

The four things that took real thought, in the order they were needed:

1. **A layer rule that the compiler enforces.** The core cannot open a socket, so
   its tests need no network and run in milliseconds.
2. **A token that is compared in time that does not depend on how much of it was
   right**, because `==` stops at the first wrong byte and that recovers a secret
   one byte at a time.
3. **A handshake with forward secrecy**, so a session recorded today cannot be
   read by anyone who learns the token tomorrow.
4. **A tool surface of three descriptions totalling 179 characters**, tested by
   handing it to readers who had never seen the repository.

---

## The failures, worst first

### 1. A rule became an identity

"`linklet-core` depends on no crate" was the rule. Its **reason** was that the core
does no I/O, so its tests need no network, no files and no cleanup.

Those are not the same statement, and the difference cost more than everything
else combined:

- While the crate registry appeared unreachable, the rule was read as a rule about
  the **whole project**, and a **SHA-256 was written by hand** to avoid a
  dependency. Its padding was wrong three times. The published NIST vectors caught
  it on the first run -- `abc` returned the initial state, meaning the compression
  function never ran. One observation (`Vec::resize(55)` producing `len == 63`) was
  never explained, and the half-working file was deleted rather than debugged.
- The same reading produced a **660-line hand-written JSON codec with 30 tests**.
  It worked. It should still have been `serde_json` from the first line.
- The registry was reachable the entire time.

**The transferable part:** a rule stated as a prohibition is satisfied by
obeying it, so it never gets re-examined against its reason. A rule stated as a
reason gets re-examined the moment the reason stops applying. The fix was not to
try harder; it was to rewrite the rule as what it always meant -- *the core depends
on no crate that does I/O* -- with an allowlist where every entry carries the
argument for it.

### 2. An environment fault read as a design constraint

Cargo could not reach the registry. The conclusion drawn was that the environment
made a dependency impossible, and work was designed around it.

The actual cause was two lines in `~/.gitconfig` pointing git at
`http://127.0.0.1:7892`, where nothing listened. The running proxy was on `7890`.
Cargo reads git's configuration because of `git-fetch-with-cli = true`, so
git's stale address was cargo's address, while everything that used the Windows
system proxy -- the browser, `Invoke-WebRequest`, a direct request to the registry
-- worked fine.

It was made worse by misreading a signal: `cargo search` had failed with a message
about registry replacement that has nothing to do with networks, and that message
was taken as evidence the network was down. `cargo fetch` was working the whole
time.

**The transferable part:** an environment fault and a design constraint look
identical from inside a failing command. One diagnostic that tests the hypothesis
directly -- does a plain HTTPS request to the registry succeed? -- cost nothing and
was not run for several rounds.

### 3. The bug whose lesson had already been read

A command killed by its deadline never returned. `Child::kill()` kills the shell,
not the program the shell started, so `cmd /C ping -n 30` left ping alive holding
both pipes. The readers blocked forever, the agent never replied, and the caller
saw a transport failure for a command the agent was about to describe correctly.

The upstream project this one is modelled on has that exact lesson in its pitfalls
file. It had been read, and cited, and the bug was written anyway.

It also cost a wrong diagnosis first: the client reported a timeout, so the
client's deadline arithmetic was suspected, and the cause was on the other side of
the socket.

**The transferable part:** a pitfall that has been read and not encoded is a
pitfall that will be met again. The note existed; what did not exist was anything
that could fail because of it.

### 4. The assertion pointing the wrong way

Found while fixing 3. The test for a killed command asserted:

```rust
assert!(outcome.duration_ms >= 900);   // a lower bound
```

**A lower bound passes when the process is never killed at all** -- the number is
large for the wrong reason. It was replaced with an upper bound on how long the
reply takes, which is the property that was wanted. The suite went from 29 seconds
to 1.1 as a side effect, because the old test had been waiting out the ping it
failed to kill.

**The transferable part:** an assertion in the wrong direction does not merely fail
to catch a bug; it hides one and then spends time on it every run.

### 5. The mistake that failed safely, by luck

The first version of the sealed channel derived two directional keys and assigned
them the same way at both ends, so one side sealed with a key the other did not
have and **every round trip failed**.

That is the safe direction of that mistake, and it was **luck rather than design**.
Had the derivation produced one key used in both directions, every round trip
would have passed -- and the result would have been two independent nonce sequences
under one key, where the first repeat destroys confidentiality and allows forgery.
It is invisible from outside.

**The transferable part:** a passing test is not evidence that two keys are
separate. `Role` became a parameter so that the mirroring is by construction, and
the reason is written on the type rather than in a comment next to the code.

### 6. The experiment whose isolation was not achieved

The M5 claim -- that three short descriptions are enough for a reader with no other
context -- was tested by handing the tool list to five fresh readers. The method
says a reader gets the tool list and one request, and nothing else.

In practice the readers ran in the working directory **with file access**, and the
one answering request 3 **searched it** for an agent address before refusing. So
the result is evidence about a reader with more context than intended.

The same run had a second flaw: one of five fixtures was typed by hand instead of
generated from the binary, and it was the one that drifted -- a word missing from a
description. Four of five were generated precisely to prevent that.

**The transferable part:** both flaws were recorded in the result rather than
quietly rerun. A method section that describes an isolation the run did not have is
worse than no method section, because it makes the result look stronger than it is.

---

## What the gates caught, and what they could not

The gates caught, repeatedly and without being asked:

- the tool-surface count, in **two** places, when a third tool landed -- once in the
  core and once over a real MCP session
- the dependency rule, the moment `serde_json` was added to the core, which is what
  forced the rule to be rewritten properly instead of quietly
- clippy: `len() > 0` instead of `is_empty`, `% 2 != 0` instead of
  `is_multiple_of`, three unused imports and two dead functions
- rustdoc: a public function whose documentation linked a private one
- a test binary left holding the executable, a stray agent process from an earlier
  end-to-end run
- a mutation that never applied, because PowerShell backticks are escape characters
  -- a no-op mutation proves nothing, and the script now asserts the file changed

**No gate caught any of the six failures above.** Every one of them was a wrong
reasoning step, and the ones that were fixed at all were fixed because the reason
had been written down somewhere a person could read it:

- the tree-kill bug was found by a test, but the *cause* was recognised from a
  note in another project
- the wrong-direction assertion was found by reading it, not by running it
- the key-mirroring bug was found by a round trip, and the *danger* was recognised
  only afterwards
- the rule-versus-reason problem was found by a reader of this project pointing
  out that a dependency was affordable
- the misread network diagnosis was found by the same reader asking why the
  conclusion was reached at all

**The pattern is that the gates were reliable for mechanical errors and useless for
reasoning errors.** What worked on the reasoning errors was writing the reason
down, in the same commit, in a place a reader would be when they needed it.

That is the argument for this directory existing. `docs/rationale.md`,
`docs/decisions.md` and this file are not documentation of the code -- the code is
readable. They are the reasoning that the code cannot carry, and they exist because
in this rebuild the reasoning was the part that failed.

---

## What is still not verified

Listed because a claim nobody tested is worse than an absent one.

- **No CI.** `tools/verify.ps1` is the single entry point and has only ever been run
  by hand. There is no remote to run it from. A green check in a README that does
  not exist would be worse than the honest line in `README.md`.
- **No second machine.** Every test either uses loopback or a real Windows socket.
  `docs/testing.md` says what that costs. Nobody has driven a real target.
- **No identities.** The token authenticates the channel and says nothing about
  which caller it is, so there is no per-caller revocation and no audit trail.
- **No cipher agility.** One curve, one cipher, one derivation, at build time.
- **The M5 experiment had one reader family and five requests.** A good result is
  weak evidence and a bad result is strong evidence; the run produced both, and the
  asymmetry is the useful part.
