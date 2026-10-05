---
name: linklet
description: Use when deploying, starting, inspecting or debugging a program on a Windows LAN test machine through linklet - putting a build on a target, stopping the previous run, starting something that outlives the call, reading or searching a log on the target, or collecting evidence when a call fails. Requires the linklet MCP server (check, testbed, exec, push, pull, ps, kill, spawn, grep, tail, ls) and a running linklet-agent.exe on each target.
---

# Driving linklet

linklet ships as an MCP server: it gives you the tools. This skill is the procedure,
because the order of the calls matters and the wrong order produces failures that
look like bugs.

There are two sides. The **host** is the machine you are on, where the MCP server
runs. Each **target** is a machine running `linklet-agent.exe`. Every tool but
`check` and `testbed` takes `agent`, an address written `host:port` -- that is the
only way a tool names a machine, and there is no tool that finds one (see below).

## Before anything else

1. **Know the address.** The MCP surface has no discovery tool -- `discover` is a
   command and not a tool -- so the addresses come from the conversation, or from
   `linklet discover --port <port> --targets` run in a shell, which prints the
   `host:port` list every tool takes.
2. **`check` answers reachability, not health.** It opens a connection and closes it,
   so a wedged agent keeps its listening socket and reads as `live` while every real
   call times out. `linklet probe --agent <addr>` completes a handshake and reads a
   reply, and its exit codes are the answer: **0 answered, 1 nothing listening, 2 the
   spec could not be read, 4 something is listening and did not answer.** When a call
   times out, run `probe` before believing anything about the network -- `docs/smoke.md`
   has the measurement where `check` said `live` and `probe` said exit 4.
3. **`testbed` is the one tool that is not about a target.** It checks the machine the
   server runs on against a specification file; its `target` argument is a label in
   the output, not an address to check. Each tool that does act on a target says "on a
   remote agent's machine" in its description and this one does not, which is the
   signal to read before handing it an address.
4. **A refusal is not an empty answer.** `the token is missing or wrong` is about the
   secret, and the server's own stderr names the file it could not read or the two
   sources it was given -- it is a configuration problem on this side, and no amount
   of retrying will change it.

## A round of joint debugging, in order

1. **`check`** -- the target answers, and it is the machine you think it is.
2. **`ps`** with `name` (a **substring**) -- is the program under test still running.
   Read the summary line, never the list alone.
3. **`kill`** it -- by `pid` from that listing, or by `name`, which is **exact** and
   needs `confirmed: true` because it can match several processes. **This step comes
   before the push, not after:** Windows will not let a running executable be
   replaced, so a push over one fails with a sharing violation that reads like a
   permissions problem.
4. **`push`** the new build -- `from` is a local path inside the working tree, `to` is
   under the agent's transfer root. The receiving side verifies the digest before the
   real path is touched, so a truncated copy fails here rather than confusingly later.
5. **`spawn`** it -- `output` is a file on the target for the program's own output,
   and the reply is `started <pid>`. **Never use `exec` for a program that keeps
   running**: `exec` waits, so the request, the connection and the agent's pipes are
   held until it exits, and its timeout is at most 600 seconds -- for a GUI program
   that means a killed process and a failed call.
6. **`ps`** again after a moment -- `started` is not `still running`. If several
   same-named workers exist, filter with `cmdline` rather than `name`.
7. **`grep`** or **`tail`** its log, and do this even if step 6 failed: a program that
   died on startup usually says why in its own log. `mode: "last"` answers "where is
   the last ERROR" directly, and the file does not cross the network to answer it.
8. **`pull`** the whole log when a summary is not enough, or when several machines
   need comparing side by side.
9. **`kill`** it again -- do not leave the program running into the next round.

## Rules that bite when ignored

- **`ps`'s `name` is a substring; `kill`'s `name` is exact.** They are deliberately
  different words on the two tools, and using one to find what the other will stop is
  how a helper process survives a deploy. `kill` also takes `candidates_cmdline` to
  narrow an exact-name match to the worker you mean.
- **An empty list is only readable next to its summary.** `ps` answers `0 of 271
  match`, `ls` answers `0 of 0 entries in <path>`, and `kill` answers `killed 0 of 1`
  with anything it could not stop listed under `failed`. A listing the machine could
  not finish exits 1 and names the field it could not read. **An empty directory and a
  directory that is not there are the same list and opposite facts**, and a match that
  was not killed is not a clean result: read the denominator, and the `failed` line,
  before concluding that anything is gone.
- **`grep`'s `pattern` is a substring, not a pattern.** `ERROR|FATAL` finds nothing
  here. An **empty string matches every line**, which is a valid call and almost
  never the intended one -- `3 matches` for a search meant to narrow is a wrong answer
  that looks like a right one. The first line of the reply says how many lines were
  read, whether it stopped early, and which encoding won; a file that could not be
  read says so and never "no matches".
- **`isError` means the call could not be made, not that the news is bad.** Every
  machine being down is a successful call carrying bad news. Retrying an `isError`
  about the token or about a path is time spent on the wrong problem.
- **Every path the agent resolves goes through its transfer root** -- `push`'s `to`,
  `pull`'s `from`, and `spawn`'s `output` included. An absolute path or a `..` is
  refused by name rather than resolved, and the refusal names the argument.
- **`kill` refuses to stop the agent itself**, and the refusal comes from the target
  rather than being filtered here: a request that named it is a request that must be
  decided again, not a request that silently succeeded on everything else.
- **A command sent to a target cannot carry a quote.** The agent runs commands through
  `cmd /C`, and three layers rewrite quote characters on the way, so a command with
  quotes comes back reading like the program does not exist. What works is a command
  with **no quotes at all**: an absolute path with no space in it, and builtins like
  `certutil` and `type`. `docs/machine.md` has the measured forms that failed.
- **A command runs in the agent's working directory, not yours.** A relative path is
  relative to wherever the agent was started, which is usually why a file "is not
  there" -- pass absolute paths, and remember that a transfer path is resolved against
  the agent's root while a *command's* paths are not.
- **One call at a time.** A single MCP server serialises its tools: an `exec` with a
  600-second timeout holds every other call in this session until it returns.

## When a call fails

1. **Split dead from wedged** with `linklet probe --agent <addr>` (step 2 above).
   Connection refused means the process is gone; a connect that succeeds and then
   times out means the agent is alive and stuck, and something outside it has to clear
   the port.
2. **Read the agent's own log.** With `--log`, the agent appends a pair of lines per
   request -- `-> #000001 run` when it is taken, `<- #000001 run ok 2411 ms` when it is
   answered, with the reason quoted on a refusal. **A `->` with no `<-` is the request
   that wedged it**, and it is the only evidence that names it. If the log is under
   the agent's transfer root, `grep`/`tail` it, or `pull` it.
3. **Report what you observed, not what you conclude.** The raw reply, the summary
   line and the log tail are the useful part; "it failed" is not.

## Run it from the command line when the surface does not have it

Three operations exist on the host binary and not as tools, and two of them are on the
path above: `linklet discover --port <port> --targets` (which machines are there, as
the `host:port` list every tool takes) and `linklet probe --agent <addr>` (is this
agent working, or only listening). The third is `linklet exec --agents a:1,b:2
<command>`, which runs one command across several machines and reports each one in the
order given -- the MCP `exec` takes a single `agent`. `linklet --help` lists the rest
of the command line.

## Reference

- `README.md` -- every command, the output formats, and the exit codes
- `docs/MCP.md` -- the surface: why each tool is separate, and where a caller goes wrong
- `docs/smoke.md` -- the target side: the firewall, starting the agent so it survives
  its console, and the supervisor that restarts a dead or wedged one
- `integrations/README.md` -- how this server and this skill were installed
