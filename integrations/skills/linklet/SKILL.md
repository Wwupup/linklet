---
name: linklet
description: Use when deploying, starting, inspecting or debugging a program on a Windows or Linux test machine through linklet - putting a build on a target, stopping the previous run, starting something that outlives the call, reading or searching a log on the target, or collecting evidence when a call fails. Requires the linklet MCP server (pair, check, testbed, discover, probe, exec, push, pull, ps, kill, spawn, grep, tail, ls) and a running linklet-agent on each target.
---

# Driving Linklet

The controller runs `linklet mcp`; each target runs an ordinary `linklet-agent`
process. Both binaries need protocol 3. Saved pairing credentials remain valid.
The target's account and current elevation determine access to its filesystem
and processes. Exiting the agent makes it offline; there is no automatic restart.

## Get context once

Use the target address from the user, or discover TCP listeners with `discover`.
Discovery proves reachability only. For a new controller, call `pair` with
`agent` and the six-digit `code` as a string, preserving leading zeros. The
credential is saved automatically. Pair already returns authenticated identity,
account, elevation, shell, startup directory and limits: reuse this context.
For an already paired target, `probe` obtains the same context in one call.
Unknown auxiliary facts are null with warnings; sudo eligibility is not elevation.

Do not request a long-term token or configure the removed LINKLET_TOKEN /
LINKLET_TOKEN_FILE variables. `linklet-agent init` rotates the pairing state
after the old agent stops, so controllers then need to pair again.

## Read structured results

- Branch on `structuredContent.status`, `error.code` and `error.phase`.
  The text is a short summary. Every tool has an outputSchema.
- `ok` is a valid observation. An empty query or unreachable address in a
  discovery scan can be an ok observation. Query counts, scope and completeness
  determine what conclusions the observation supports.
- `error` is a known failure, including a nonzero command exit or timeout.
- `unknown/result_unknown` means a side effect may have happened without a
  valid result arriving. Do not automatically repeat it. Inspect the target
  state or named output file before deciding what to do next.
- Batch exec preserves input order and per-target status; aggregate precedence
  is unknown, then error, then ok. Error and unknown set `isError=true`.

## Execute accurately

Prefer `program` plus literal `args` when starting an executable. Empty
arguments and shell characters are preserved. Omitted args means an empty array.
Use `command` for pipelines, redirection, shell builtins and Windows batch
files. The two modes are exclusive. Windows command mode uses cmd.exe /D /S /C;
Linux uses sh -c. Both close stdin.

Set `cwd` for the call when needed; a relative cwd starts at the agent's
startup directory. A relative program containing path separators starts at that
call's cwd. File tools and `spawn.output` keep their startup-directory base.
Absolute paths make locations explicit. CLI options precede the command or
`--program`; everything after the program is a literal program argument.

Use exec for a finite command. It waits up to its timeout (at most 600 seconds).
It accepts exactly one of agent or agents; batches support 256 targets with
concurrency four. One MCP session otherwise processes calls sequentially.

Use spawn for a background program, providing its output file. It observes for
500 ms by default; observe_ms accepts 0..5000, with zero returning started.
An early exit returns exited and its exit code. running_at_observation means
only that the child was still running at that instant. Read PID kind, cwd, log
path and initial log tail from this reply before making another query. No state
asserts service readiness or the lifetime of every shell descendant.

## Deploy and inspect with the evidence already available

1. If an old instance might prevent replacement, inspect with ps and stop only
   the intended process. ps.name is a substring; kill.name is exact and requires
   confirmed:true. A pid names one process. Check incomplete process snapshots
   and unavailable filter fields before concluding that no instance exists.
2. Push the build. Successful transfers report actual paths, byte counts and
   the compared digest. verified:true needs no extra hash call.
3. Spawn with program/args and an explicit output path. Use the observed exit
   and initial log tail. Check ps later only when continued liveness matters;
   use an application-specific check when readiness matters.
4. Read more log content with tail. The first call reads the last requested
   lines; subsequent calls pass next_cursor with the same target and path.
   Continue while has_more is true. Empty new content is success. Preserve
   original newlines and fragments when concatenating pages. Rotation, shrinking
   or changed boundary samples reset the cursor explicitly; do not claim an
   uninterrupted history after a reset.
5. Use grep for a fixed substring or pull for a complete file copy when needed.
   grep.pattern is not a regular expression; ERROR|FATAL is a literal substring.
   Clean up a program only when its task calls for stopping it.

## Limits and failures

Each command stream keeps at most its final 1 MiB of raw bytes. MCP shares a
32 KiB content budget across command text and query entries. Target truncation,
MCP presentation truncation and decoding loss are separate flags. Discarded
command output is not recoverable; no implicit complete log is saved. Spawn
writes its explicitly named log, and tail cursors never skip undisplayed bytes.

Remote absolute paths, parent components, other Windows drives and ordinary UNC
shares follow OS permissions. There is no --root. Relative local paths for
testbed.spec, push.from and pull.to use the controller's working directory.
Testbed checks that local machine; its target argument is only a label.

A connection failure says that the endpoint was not reached. A connected timeout
does not prove why the target did not answer. Preserve the category, phase and
original reason. Authentication errors may require pairing again; local config
errors require fixing local storage. Do not convert either into a repeated
remote side effect. The agent's rotating diagnostic log can provide further
evidence when accessible; a missing final log line is an observation, not a
proof that the operation did not run.

## Reference

- README.md: CLI commands, startup and limits
- docs/MCP.md: tools, schemas and result meanings
- docs/AI-EVALUATION.md: measured scenarios and limits of the evidence
- docs/smoke.md: separate real-machine and LAN validation
