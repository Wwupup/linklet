# Roadmap

Working rules for this list:

- **Each milestone ends in something runnable and green.** No milestone leaves
  the repository in a state where `cargo test --workspace` fails for a reason
  other than "the next milestone is not written yet".
- **Each one is testable without a network and without a second machine**,
  except where it is explicitly about the network -- and then the part that
  decides is still tested without one.
- **The order is a dependency order, not a preference order.** Skipping ahead
  produces exactly the kind of code this project exists to avoid.

## M0 -- the skeleton

- [x] workspace, three crates, the compiler-enforced layer rule
- [x] `.gitignore`, `AGENTS.md`, `rust-toolchain.toml`, this file
- [x] the core types declared, with the tests that define their behaviour
- [x] everything else: `cargo test` failed on `unimplemented!`, on purpose

## M1 -- the first decision, end to end

The smallest complete thing that does something true.

- [x] `parse_targets`: the text grammar in `linklet-core/src/target.rs`
- [x] `cargo test -p linklet-core` green: 19 specification tests, 2 architecture
      guards, 0.10 s warm
- [x] the CLI prints the parsed targets, and exits 2 on a bad spec -- finished
      in M3, where the whole surface was built at once rather than in halves.
      Recorded here rather than deleted, because a checked-off item that was
      quietly dropped is how a roadmap becomes fiction.

*What you learn here:* writing a test before the code, making an invalid state
unrepresentable (`Port` cannot hold 0), and the difference between a validation
error and a guess.

## M2 -- the first I/O, and why it does not leak into the core

- [x] `linklet-core` defines the trait it needs: "tell me whether this target
      answers, within this deadline" -- `Probe`, in `linklet-core/src/probe.rs`
- [x] `linklet-adapters` implements it with a real TCP connect and a deadline
- [x] the core keeps a *policy* on top of it and that policy is tested with a
      fake in-memory implementation -- no network in the test

*What you learn here:* dependency inversion, and why the trait belongs to the
core rather than to the adapter. This is the single most valuable idea in the
whole roadmap.

**What actually happened, which is more useful than the plan was.** Three rounds
of measurement were needed before the adapter could be written correctly, and
none of it was guesswork:

1. On Windows a refused loopback connection takes about 2.05 seconds to surface
   (measured ten times; a listening port answers in 137 microseconds). Under a
   shorter budget `connect_timeout` invents a timeout rather than reporting the
   refusal -- so a program that is simply not running would have been reported as
   a machine that did not answer.
2. An empty host is not nonsense: `("", 80)` resolves to nine local addresses on
   this machine. A test written on the opposite assumption was testing the
   multi-address path while claiming to test resolution failure.
3. Consequently the probe needs two words where one was planned: a timeout is a
   verdict the OS reached, and no answer is the absence of one.

None of that is visible from the plan, and all of it is the kind of thing that
would have shipped as a wrong answer. The tables are in
`linklet-adapters/src/tcp.rs`.

## M3 -- a command line an agent can use

- [x] a real `check` command, output that is stable and parseable
- [x] exit codes and error messages that an agent can act on without reading
      documentation twice
- [x] every message the tool prints is either a fact it observed or an error it
      can quote -- no reassurance, no summary, no invented noun

*What you learn here:* the interface is the product, and every sentence printed
is part of it.

The format and the exit codes live in `linklet-core/src/outcome.rs`, not in
`main`, because they are the published interface: an agent branches on them, so
they should be pinned by tests instead of by a comment in the one file nobody
tests.

## M4 -- one call instead of five

- [x] concurrency across targets, with per-target timeouts
- [x] a single invocation that reports on many machines
- [x] the decision of what to do about partial failure, written down and tested

*What you learn here:* where concurrency belongs (the adapter) and where it
must not live (the decision).

**What actually happened, and what it says about the plan.** The third item
needed no decision at all. Partial failure had already been decided in M2: one
machine being down is an ordinary result carrying bad news, not a failure of the
run. Concurrency did not make it a special case, and the only work it needed was
a test asserting that the concurrent run agrees with the serial one about it.

That is the useful shape of this milestone. Two of the three items were about
*when* the waiting happens; the third was already answered. "We will decide that
later" sometimes means "it was decided already and nobody wrote it down".

The measurement: 20 unreachable targets on a 5 s budget finish in 5.0 s, against
roughly 40 s serial. `docs/testing.md` records which layer each of these tests
belongs in, and why the overlap itself cannot be tested in the core.

## M5 -- being useful from an agent, and being honest about it

- [x] the MCP surface, deliberately **few and coarse**: one call per intent,
      not one call per endpoint
- [x] the whole tool description fits in a paragraph, without caveats
- [x] a check on the process itself: hand the tool list to a model that has not
      seen this repository and see whether it can pick the right call

*What you learn here:* why a tool description that needs a manual is a symptom
of a bad interface, which is where this project came from.

**What the experiment found.** Run and recorded in
`docs/tool-readability.md`, with its method, its result and two flaws in how it
was run. The short version: four of five readers refused to fabricate rather
than guessing an argument, and every refusal named the missing thing. That was
not the property the experiment was designed to test, and it is the more valuable
one -- a plausible guess that looks like progress is how an agent causes damage,
and this surface did not produce one.

The failure is narrower and more interesting than the success. One reader,
asked to confirm two preconditions before a test, reached for `exec` and composed
`dir /b *.exe & tasklist ...`, which answers the question just as well. Nothing in
`testbed`'s description says why it beats typing the equivalent command, and
saying so is a sentence about *when* to use a tool -- which is the kind of
sentence the M5 rules exist to keep out. So the open question is not how to
describe that tool better; it is whether a surface should have fewer tools rather
than better-described ones.

**What actually happened.** The surface is one tool, `check`, with a
sixty-character description. Three rules keep it there, and they are enforced
rather than remembered: `tests/tool_surface.rs` asserts the tool count, forbids a
description from naming another tool, and gives every description a
120-character budget. The reference point being guarded against had 13,758
characters across seventeen tools, and no single step in it was ever wrong.

Three capabilities that were planned here are **absent rather than stubbed**:
`exec`, `logs` and file transfer. All three need something on the far side to
talk to and there is nothing there yet, and rule 3 in `tool.rs` says a tool that
answers "not implemented" is worse than a missing one. They arrive with the agent
that serves them, which is the next thing.

---

## M6 -- who may run commands, and who may read them

**Done, and it was not on this roadmap when the plan was written.** The parked list
said "no security boundary beyond the caller can already reach the machine", and
for a tool whose whole job is running commands on someone else's machine, that was
the wrong call. It was parked, then built. The parked item is struck below rather
than deleted, because a plan that quietly loses an item is a plan nobody can check.

- **A shared token, compared in time that does not depend on how much of it was
  right.** `==` stops at the first differing byte, so the time a reply takes depends
  on how many leading bytes were correct, and that recovers a secret one byte at a
  time. In `crates/linklet-core/src/auth.rs`, with a test that measures rather than
  asserts, and a control that proves the measuring works.
- **A sealed channel with forward secrecy.** X25519, HKDF and ChaCha20-Poly1305,
  with both ephemeral private halves discarded, so a session recorded today cannot
  be read by anyone who learns the token tomorrow.
- **The token is never transmitted.** What crosses the wire is public keys and
  sealed bodies; the token is mixed into the key derivation instead of being sent.
  A captured request is not a credential.
- **Two messages on one connection**: the handshake, then the command sealed under
  the session it produced. The handshake cannot protect itself, so the exchange has
  to come first -- and both on one connection keeps the agent stateless.
- **Rule 1 was rewritten**, from "the core depends on no crate" to "the core depends
  on no crate that does I/O", because the old wording was satisfied by a rule rather
  than a reason and had been read as a rule about the whole project. See
  `docs/decisions.md`.

## M7 -- getting a build onto the machine, and the evidence back

**Done.** `linklet push` puts one file on a target and `linklet pull` brings one
back, over the sealed channel, with the receiving side verifying the digest before
the real path is touched -- so a transfer that did not arrive intact leaves nothing
behind under the name the next step would believe. Both are on the MCP surface too,
because an agent cannot install a build it has no way to send.

**Why this one was necessary rather than valuable**, which is what the plan said
before it was built: the tool could check reachability and run commands, and it
could not put anything on a target -- which is the first step of the workflow it was
written for. A tool that cannot deploy has not yet reached the point where its
security model can be shown to be worth anything.

**What was already in place before any I/O was written**, all in `linklet-core` and
all tested. Written down then because a reader who has to work out what exists will
either rebuild it or build on a different shape; kept now because it is still where
the design lives:

- [x] `src/frame.rs` -- the wire format, with the ten ways a length-prefixed
      protocol goes wrong listed and defended, and a test per failure mode
- [x] `src/transfer.rs` -- T1 path validation against a root (the Windows rules a
      `..` check does not cover), T3/T11: the manifest checked before any chunk is
      read, and the chunk arithmetic as a function with tests. The running total
      (T4), the end-of-transfer and digest checks (T5, T6, T7) arrived with the
      receiving side and are tested in the same place, in microseconds
- [x] `Sealed::seal_into` / `open_into` -- T10, so a chunk of a file is in memory
      once rather than three times

**What it took**, in `adapters`, `agent` and `client`:

- [x] the connection loop reads frames instead of HTTP: a read timeout on every
      read, one chunk at a time so the desynchronisation defence keeps holding, and
      the message count bounded by the declared size
- [x] the agent receives a transfer: manifest, path, `.part`, per-chunk total,
      digest, rename
- [x] the client sends one: stream, `seal_into`, frame
**The agent's hand-written HTTP layer was replaced first, as this section required.**
It is deleted, and `tiny_http` was never needed: the answer turned out to be the
length-prefixed frame that `docs/decisions.md` D4 had already written down as the
design that fits the channel. What replaced it is
`crates/linklet-adapters/src/connection.rs` for the socket and
`crates/linklet-agent/src/server.rs` for the protocol, and D4 records what that cost.

- [x] `linklet push --agent <host:port> --from <local> --to <remote>` copies one
      file to a target over the sealed channel
- [x] `linklet pull --agent <host:port> --from <remote> --to <local>` brings one
      back, for collecting a log or a result
      -- both need `--root` on the agent, which is the one directory a transfer may
      read or write; it defaults to the directory the agent was started in
- [x] **the framed body carries bytes rather than hex.** It was hex because the
      framing layer was written as text, and that doubled every sealed body. Replacing
      HTTP with frames removed the text body entirely, which is stronger than making
      the body binary: there is no longer a body that could be text.
- [x] a size limit, refused clearly rather than truncated
- [x] **the transfer is verified by digest**, compared by the receiving side. A
      half-transferred file left at the destination under its real name is worse
      than a failed transfer, because the next step believes it.
- [x] `push` and `pull` on the MCP surface: an agent cannot install a build it has
      no way to send

**What this milestone does not do, and says so: it does not install anything.** It
moves a file. Whether that file is an agent, a build or a configuration is the
caller's business. "Install automation" stays parked -- the first copy onto a target
is a step a person performs, and performing it by hand once is how they find out
what a deployment actually consists of.

**What it left open, rather than quietly not doing.** A transfer has no progress
reporting: a slow one and a stuck one look the same until the deadline, which is
`docs/transfer.md`'s own parked list. There is no resumption, deliberately, and one
transfer moves one file -- a directory is the caller's loop. And nothing in the
smoke layer covers a transfer: `docs/smoke.md` says which claims it does and does
not make.

## M10 -- the first real target, and everything it found

**The number is out of order on purpose, and this is the next work.** M8 and M9 are already
named in other documents, so they are not renumbered; this section arrived from a machine
rather than from the plan, on the day M7 was called done. Read it before starting either of
those, because the first two items are defects in what M7 already ships -- everything built
on top of them is built on an answer that is wrong in a specific way.

The round that produced it: `linklet-agent` on a Windows 11 eval guest (192.168.100.2), the
seven claims of `tools/smoke.ps1`, then transfers in both directions, a killed transfer, and
every refusal case. Six bugs came out of that and five are fixed in the commits around this
section; what is left is below.

### Two defects, before any new capability

- [x] **A reply too large to frame is refused by name, and not reported as a network
      failure.** A command whose output was 20,000,000 bytes ran to completion on the target
      -- `exit 0`, 20,000,000 bytes measured there -- and the caller was told *"could not
      reach the agent: the agent closed the connection without answering"*. The real ceiling
      is the frame's `MAX_PAYLOAD` of 16 MiB: the sealed reply could not be framed, so the
      agent sent nothing and closed, and a transport error was the only thing the caller
      could conclude. Three shapes were on the table, in increasing order of honesty:

      1. **refuse it by name** -- the agent answers "the output was too large to return",
         which the caller can tell apart from a dropped connection. Smallest change;
      2. **raise the ceiling** -- buys a larger number and the same silence above it;
      3. **stream the reply** -- the only shape that makes "no output limit" true, and it is
         the problem the transfer already solved by chunking. It is also the one that stops
         a single command deciding how much memory the agent spends, which `read_to_end`
         currently lets it.

      **Shape 1 was taken.** `wire::reply_fits` is the decision, with the ceiling as a
      parameter rather than a constant read from the frame module, and `wire::run_reply_too_large`
      is the refusal: both stream sizes and the reply ceiling in one sentence, so a caller
      knows which stream was large and what it would have had to fit in. `server::run_reply`
      asks before it seals, because by the time `write_frame` refuses the bytes are gone and
      there is nothing left to describe.

      **Shapes 2 and 3 are still open on purpose**, and the reason is the shape of the
      failure rather than the number in it: the agent still holds the whole output before it
      knows the reply will not fit, so a command that writes a gigabyte still costs the agent
      a gigabyte. Refusing by name turns that from an unexplained drop into a named fact, and
      it does not make it cheap. That is what option 3 would buy, and it is a protocol change.

      **What it cost to test**, and worth writing down because it will be met again: the
      command that produces the output cannot carry a quote. The agent runs commands through
      `cmd`, and a path that needs quoting does not survive the trip -- a `cmd /C "..."`
      form and a `powershell -Command "..."` form were both measured and both came back as
      the command's own text. The test builds a directory with no space in its name and uses
      `certutil`, which needs no quotes at all.

      The claim itself is corrected in this commit: `execute.rs` said "No output limit. A
      command that writes a gigabyte writes a gigabyte", and that was not true. The limit
      is still there; only the sentence about it changed.
- [x] **A command's output that is not UTF-8 says so, instead of being silently
      discarded.** A command that emitted the four bytes `D6 D0 CE C4` -- GBK for two CJK
      characters -- reached the caller as four `U+FFFD` (`ef bf bd` four times, checked in
      the bytes), and no field in the reply said the output was not text. lanlink decodes
      with the machine's OEM code page, and its `grep` reports which encoding won; on the
      Windows targets this tool is for, that is the difference between reading a program's
      error and reading mojibake. **The defect is not which guess is made but that the guess
      is silent**: a `String` in the wire type cannot carry "these bytes are not text", so
      this was a wire decision and not a formatting one, and the fix had to make the caller
      able to tell.

      **Done, and it is the wire half that changed.** `wire::Text` holds the text, the number
      of bytes the command wrote, and whether any byte had to be replaced; the run outcome's
      `stdout` and `stderr` are `Text` rather than `String`, and the reply carries
      `stdout_bytes`, `stderr_bytes`, `stdout_not_utf8` and `stderr_not_utf8`. A rendered run
      says the loss under the stream's heading rather than after the body, because the body is
      a program's output and a sentence glued to the end of it would look like something the
      program printed.

      **What it does not do is guess a code page**, and that is deliberate. Decoding with the
      OEM code page is what the sibling project does and it is a real improvement -- it turns
      mojibake back into the characters the program meant -- but it is also a second guess,
      and this defect is that a guess was silent rather than that the wrong one was made. The
      two are separable, and the one that does not risk making the output *differently* wrong
      went first. Guessing the code page is open, and it is now a change to how `Text` is
      built rather than to what crosses the wire.

      **An agent older than this change still answers**, which is the other half of a wire
      change: the four fields are written always and read optionally, defaulting to the text's
      own length and to "clean". A host that demanded them would refuse a reply it can read
      perfectly well, and `tests/wire_protocol.rs` pins that case with a reply written the old
      way by hand.

### The capabilities, which is what "operate a machine" means

lanlink is the reference for all of these: `E:\projects\lanlink` on this machine, its
`README.md` is the surface and `docs\pitfalls.md` is what it cost to get there. It is a
debugging tool for the same machines, written by the same hand, and it is ahead of linklet
on everything in this list -- which is why reading it beats designing from scratch.

- [-] **Residency, and an agent that keeps a log.** The agent died with its console during
      this round: `tasklist` found nothing, and the caller saw a connect **timeout** rather
      than a refusal. Nothing brought it back, and nothing recorded what it had been asked --
      the agent prints a banner and answers errors to the client, and keeps no per-request
      record anywhere. lanlink answers this with `supervise.ps1` (restarts on death *and* on
      stuck, with backoff, and a probe whose three exit codes are documented) and with
      `agent.log` recording each request as `->` and `<-` with a duration -- so a `->` with no
      `<-` names the request that wedged it. The parked list refuses "a daemon or service
      **on the host**", which is still right; the *target* side has no such decision written
      down, and this is the first target saying it needs one. Smallest honest shape: one log
      line per request with its outcome and duration, written by the agent to a file of its
      own, plus a documented way to start it that survives its console.

      **The log is done; surviving the console is not, so this item is half-checked.** What
      landed: `linklet-agent --log <file>` (or `LINKLET_LOG`), appending two lines per
      request -- `-> #000001 run` when it is taken and `<- #000001 run ok 2411 ms` when it is
      answered, with the reason quoted on a refusal. The pair is the design and not a
      flourish: a request that never finishes writes only the first line, and that is what
      names it. `linklet_core::log` decides what a line says and is tested in microseconds;
      `linklet-agent/src/log.rs` owns the file and the lock. A log path that cannot be opened
      is a refusal to start, because an operator who asked for a log and silently did not get
      one has evidence they believe exists and does not. The command line and the paths are
      deliberately absent from a line.

      **What is left is the launcher.** The agent still dies with its console, and nothing
      restarts it. **The documented way to start it detached is done**: `docs/smoke.md`
      carries a `schtasks` command and the script file it runs, verified on this machine --
      the task's process kept serving after the session that started it was gone, and its log
      recorded the request. Two things about that were measured rather than assumed:
      `/TR` refuses anything over **261 characters**, which every realistic command line
      exceeds and which is why the task runs a script file; and a script file keeps the
      **token out of the scheduler's record**. `--detach` on the agent was **refused**: it
      needs Windows' `DETACHED_PROCESS` creation flag, `std` does not expose it safely, and
      buying it with `unsafe` or a Win32 dependency inside the smallest binary here is a poor
      trade for what the scheduler already does.
      Still open, and named rather than implied: **nothing supervises the agent.** `schtasks`
      does not restart a process that died and cannot tell a wedged one from a busy one, so
      lanlink's `supervise.ps1` -- restart on death *and* on stuck, with backoff, and a probe
      with three documented exit codes -- has no counterpart here. The parked list still
      refuses a daemon **on the host**; a supervisor **on the target** is a new question and
      has no answer yet.

      **A shell redirect is not a logging strategy**, and that was measured after the fact
      rather than assumed: `hostname > file` launched through the spawn call leaves the file
      **empty**, while the same command through an ordinary exec captures the hostname. The
      near-empty `agent.log` this round was that, not the agent -- so a reader must not read
      an empty log as "it logged nothing". (The one `^C` in it was the shell's own echo.) This
      is the second time on one target that an unexplained observation looked like a fact
      about the agent; both are now written down where the next reader will meet them.
- [x] **`spawn`, `ps`, `kill`.** Today `exec` blocks until the command exits or the deadline
      kills the whole tree, and a child inherits the agent's pipes -- so starting a
      long-running program with it is the exact trap lanlink documents ("never use
      `lan_exec` with `start app.exe`"). There is also no way to ask what is running or to
      stop it, which means the deploy loop (kill the old build, push, start, confirm it
      stayed up) cannot be closed without a person. `ps` needs the fields that make an empty
      result readable: count, total, truncated, and the filters that were actually applied.

      **All three are done**, and this item is checked. What landed:
      `linklet ps --agent <host:port> [--name|--cmdline|--query|--exclude <text>]`,
      `linklet kill --agent <host:port> (--pid <n> | --name <exact> | --contains <text>)`, and
      `linklet spawn --agent <host:port> --output <remote-path> <command...>`, with three new
      requests on the wire and all three as tools on the MCP surface. The shape is the part
      that was designed rather than the code: one line per process, `pid name`, then a
      summary that is **always printed** -- `0 of 271 match, filter name=agent` -- because an
      empty list on its own is what the first real target got one step wrong from. `count`,
      `total`, `truncated`, the filter echoed back, and a note for every field the machine
      could not supply are all in the reply, in the core type
      (`linklet_core::process::Listing`) rather than assembled at the edge.

      **A field the machine cannot answer is reported and never defaulted.** `tasklist`
      gives a name and a pid to anyone and a command line to nobody, so a `--cmdline`
      filter returns no matches *and* a note naming the field, and the exit code is 1: the
      call was made and the answer is incomplete. That is M10's own example, implemented
      rather than quoted.

      **`kill` refuses two things, and refuses them on the target** before `taskkill` runs:
      a bulk match that was not confirmed, and a request that would stop the agent or the
      process that started it. A refusal means **nothing was attempted**, which is a
      different answer from a report saying nothing was killed -- the first says the caller
      should decide again, the second says the machine has nothing to do. It is a refusal
      and not a filter: quietly dropping the agent out of the plan would report success on
      everything else while the one process the caller named kept running. And a name match
      that was deliberately not killed leaves the report **incomplete**, because reading
      `killed: []` as "it was already gone" is how a caller comes to overwrite a file a live
      process is holding.

      **The exit code follows `check` and not `exec`** -- there is no command whose status
      could be passed through, and `1` for an incomplete listing, or for a process that is
      still there, is a fact about the machine rather than about the network, which is what
      the scheme is for.

      **One bug here was found by a test rather than by reading, and it is written down
      where it bit.** The deploy-loop test found its marker with `--name` and then could not
      kill it by pid: `kill` read the ordinary listing, which is capped at `MAX_LISTED` so
      that one reply cannot be the thing that fails, and the process was past the ceiling.
      `matched: 0` for a process that is running is what a deploy loop reads as a clean
      machine. The fix is a pair of functions with the reason on them -- `matching` decides
      with no ceiling, `apply` reports with one. **A cap on what is reported must not be a
      cap on what is acted on.**

      **`spawn` is the third, and it is the one that was missing from the loop.** `exec`
      **waits**, so a program meant to keep running holds the request, the connection and the
      agent's pipes with it -- the trap this item opens with. `spawn` gives the child **its own
      output file** and answers with a pid, which is also how the caller reads the program's
      output afterwards: the `pull` that already existed, because the file is written on the
      target and a path under the agent's root is reachable.

      **What it deliberately does not say is whether the program is healthy.** A reply that
      said "started and running" would be a claim about a moment this side has not looked at,
      so the line is `started 5144` and the `ps` that follows is the question. An earlier
      version slept for a moment and checked, so that a program that died at once could be
      reported as a failure; it was dropped, because it delays every spawn, makes the answer
      depend on how fast the machine is, and is a worse version of the `ps` the caller is
      going to make anyway.

      **One thing `spawn` still cannot do**, stated here rather than left to be discovered:
      the child is in the agent's console and dies with it, exactly as the agent does. Making
      it otherwise needs `DETACHED_PROCESS`, which `std` does not expose safely -- the same
      refusal, for the same reason, as the agent's own `--detach`. A program that must outlive
      the console is started the way `docs/smoke.md` starts the agent: from the scheduler.
- [x] **`ls`, `tail`, `grep`, and an encoding that is reported.** The answer to "look at the
      log" is currently "pull it and grep locally", which is defensible -- the pull is
      digest-verified and root-bounded -- but it is wrong for a two-gigabyte log and it
      cannot answer "where is the last ERROR" without moving the whole file. lanlink's
      `grep` is the reference: the pattern goes in as an argument rather than through a
      shell, `first` and `last` modes, context, and four reporting fields (files searched,
      an exact match count or an explicit "stopped early", truncated, partial). Two of its
      lessons are worth copying verbatim: **a failed search must not read as "no matches"**,
      and the encoding is sniffed with an OEM code page fallback that *says which one won*.

      **All three are done**, and this item is checked. What landed:
      `linklet grep --agent <host:port> --from <remote> --pattern <text> [--last] [-i]
      [--context <n>] [--max <n>]`, `linklet tail --agent <host:port> --from <remote>
      [--lines <n>]`, `linklet ls --agent <host:port> --from <remote>`, the three requests on
      the wire, and all three as tools. The reading is done on the target and only the answer
      crosses: a file past the sixteen-mebibyte ceiling is read **from its end** for a `last`
      search, because the answer to "where is the last ERROR" is near the end of a large file
      and a window taken from the front would answer about the beginning -- not a smaller
      answer, a wrong one.

      **`ls` answers what the others assume.** Before it, finding out what a target held
      meant guessing at names and reading refusals. It takes `ps`'s shape rather than being a
      bare list, because an empty directory and a directory that is not there are the same
      `Vec` and opposite facts -- and a caller that confuses them concludes a machine has no
      logs, which is how a deployment stops looking for them. `found`, the two counts and the
      truncation are all in the reply for that reason, and a file lists as one entry so that
      "is it there, and how big is it" is the same call.

      **The two lessons are the shape of the result, not a note about it.** `searched` and
      `problem` are always present, so a file that could not be read is never an empty list;
      `total` is `null` when the scan stopped at the limit rather than a count, so "there may
      be more" is not "there are none"; and the encoding is carried out with the text and
      printed in the first line, so a reader shown nonsense knows which rule produced it.

      **What it does not do, and this is a real gap rather than a smaller step.** The pattern
      is a **substring**, and lanlink's is a ripgrep pattern: `ERROR|FATAL` finds nothing
      here and finds both there. The choice is defensible -- a pattern language needs a
      parser, a matcher and its own tests, and a matcher that got a corner wrong would return
      the wrong lines with nothing to tell it from a log with different lines in it -- but it
      is a difference in capability and it is written down as one. The open question is
      whether that means a regex engine in the core, which rule 1 forbids as a dependency, or
      a searcher binary on the target whose absence is *reported* rather than read as no
      matches -- which is exactly what the sibling project does with `rg.exe`, and what its
      README warns about.

      **`ls` is the third, and it was the smallest of the three to build and the one that
      changes what a caller can do.** `found` is in the reply and not inferred from an empty
      list, for the reason this whole item exists: the two answers look identical and mean
      opposite things.
- [ ] **Discovery and fan-out.** Every call names one `host:port`; `check` is the only thing
      that takes many targets. lanlink scans the networks it is on, remembers what answered
      for the session, and takes a stable list from the environment -- and then runs the same
      operation across several machines at once. The concurrency belongs where `check`'s
      already is, in the adapter; **what and how many results come back from a many-target
      `exec` or `push` is a decision, not a loop**, and it is the part worth designing.
- [ ] **Jobs -- still parked, and now with a price on it.** A long run cannot be started and
      watched: `exec` is one request with a deadline of at most ten minutes. lanlink has the
      feature and its shape is the evidence that this is a design rather than a patch: six
      states including orphaned, output files, TTLs, cancellation, and adoption after a
      restart. The honest smaller step is `spawn` plus a log file, which covers most of "run
      it and watch it" without a lifecycle. Its parked entry below had a cross-reference to
      M4 that was wrong -- M4 is concurrency across targets -- and that is corrected here.

**What not to copy.** lanlink's surface is seventeen tools; five of them are the job family,
which is one intent, and `docs/tool-readability.md` is the measurement of what that costs.
Its passphrase-derived token is also not needed here: linklet's token is never transmitted,
so it has no carrier to protect, and a derivation would be a security decision with a
minimum length and no rate limiting behind it.

**What M8 gains from this round.** M8's own list -- "how many things were examined, and
whether it stopped early", "which filters actually applied, echoed back", "a failure to
enumerate, never reported as an empty result" -- is exactly what lanlink does everywhere,
and the round produced the example that shows why it matters: a process query returned
`count: 0` and the reply also carried the filters it had applied and a note that a
non-elevated agent cannot read other users' command lines. Without those two fields an
empty result is indistinguishable from "nothing there", and the wrong conclusion was one
step away. linklet has one instance of this pattern today (exit 1 against exit 3) and M8 is
where the rest of them go.

## M8 -- did the tool actually do what you asked

**No longer waiting on anything.** M7 landed, so this is the next one and it is not
started. Worth writing down because a sibling project does this better and the gap is
real: every command that reports a *result* should also report whether it was able to
look.

- how many things were examined, and whether it stopped early
- which filters actually applied, echoed back
- a failure to enumerate, never reported as an empty result

`linklet` has one instance of this pattern -- exit 1 against exit 3 -- and it needs
several more before its answers can be trusted on a machine it does not control.

## M9 -- identities, and being able to change the cipher

Parked. The token authenticates the channel and says nothing about *which* caller it
is, so there is no per-caller revocation and no audit trail. There is also one curve,
one cipher and one derivation, chosen at build time. Neither is needed to use this on
a network you control, and both are needed before it is used on one you do not.
## Parked deliberately

Written down so they can be refused on purpose rather than discovered by
accident. None of these is planned:

- a job/session model with a lifecycle. **The cross-reference here used to say "the honest
  version of this is M4", which was wrong** -- M4 is concurrency across targets, and nothing
  in this plan covers a job's lifetime. It is now in M10 with the price written down: the
  sibling project needed six states, output files, TTLs, cancellation and orphan adoption
  for it, and `spawn` plus a log file is the honest smaller step.
- a configuration file (flags until there is a proven need for persistence)
- a daemon or service **on the host** -- still refused; the host is a client. The *target*
  side is a different question and is now M10: the agent died with its console on the first
  real target and was not brought back.
~~- encryption, authentication, or any security boundary beyond "the caller can
  already reach the machine"~~ -- **struck at M6.** It was the wrong call, and it
  is left visible because the plan was believed for several milestones while it was
  wrong.
- install automation for the first copy onto a target

## A rule about this file

If a milestone here stops being true -- because the work showed a better order,
or showed the milestone to be unnecessary -- **edit this file in the same
commit that makes it untrue.** A roadmap that describes a plan nobody is
following is worse than no roadmap, because it is believed.
