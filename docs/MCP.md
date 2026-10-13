# The MCP surface

`linklet mcp` speaks the Model Context Protocol on stdin and stdout, so an AI agent can call
this tool directly. No configuration and no port: the client starts the process and talks to it.

**What an agent reads is the descriptions and schemas in `crates/linklet-core/src/tool.rs`,
not this file.** This is the maintainer's document: what is on the surface, why each tool is
separate, and where a caller can go wrong. `crates/linklet-core/tests/tool_surface.rs` enforces
the rules; this explains them.

## Installing it

Three fields, and they are the same in every client -- only the file they go in differs:

```json
{
  "command": "C:\\linklet\\linklet.exe",
  "args": ["mcp"],
  "env": { "LINKLET_TOKEN_FILE": "C:\\linklet\\token.txt" }
}
```

**`command` names the executable, not a shell command.** An MCP client spawns it without a
shell, so a native `.exe` needs no wrapper -- which is the one thing here that a `.cmd` or a
script cannot do. It is the release's `linklet.exe`; `linklet-agent.exe` is not a client and
belongs on the machines being driven, where no release can put it for you.

Which file the entry goes in is the client's business. `integrations/README.md` has it written
out for the two clients this was set up with, together with the skill that carries **the order
the calls go in**. That order is the half a tool description cannot hold -- see "What has been
measured, and what has not" at the end of this file for the experiment that found the boundary.

## The token, and where it may come from

**The token is read from the environment, or from the file that variable names, and never from
a tool argument** -- an argument would put the secret in the conversation, which is the one
place it outlives the session. In a client configuration, prefer `LINKLET_TOKEN_FILE` over
`LINKLET_TOKEN`: the second writes the secret into a file that gets copied, shared and
committed, and the first writes a path.

The file's first line is the secret, and a byte-order mark and the line ending are not part of
it, so a file written by `Set-Content -Encoding utf8` holds exactly what was typed into it. **A
secret is named once**: a token file and a token together are refused rather than ordered,
because the two are two answers to one question and whichever lost would be the one the
operator believed was in force.

**A wrong token is an authentication refusal.** `probe` reports missing or unreadable
local token configuration separately (exit 2). Other authenticated calls also report
configuration details on stderr. Token configuration does not disable `check`,
`discover`, or `testbed`, which need no secret.

Run `linklet probe --agent <host:port>` before wiring a client up. It completes a handshake and
reads a reply, so it answers whether the token and the agent agree (exit 0) rather than whether
something is listening. `docs/smoke.md` says what the five codes mean and why the difference
from `check` matters.

## The surface

Thirteen tools, one per intent.

| tool | question | required arguments |
|---|---|---|
| `check` | does each `host:port` accept a TCP connection | `targets` |
| `testbed` | does a machine match a specification file | `spec`, `target` |
| `exec` | run and wait on one or several agents | `command`, exactly one of `agent` or `agents` |
| `ps` | what is running on an agent's machine | `agent` |
| `kill` | stop processes on an agent's machine | `agent`, **and `pid` or `name`** |
| `spawn` | start a program on an agent's machine, without waiting | `agent`, `command`, `output` |
| `grep` | find lines in a file on an agent's machine | `agent`, `path`, `pattern` |
| `tail` | read the end of a file on an agent's machine | `agent`, `path` |
| `ls` | list a path on an agent's machine | `agent`, `path` |
| `push` | copy one local file to an agent | `agent`, `from`, `to` |
| `pull` | copy one file back from an agent | `agent`, `from`, `to` |
| `discover` | find TCP listeners on local networks | none; optional `port`, `networks`, `targets` |
| `probe` | authenticate and obtain a valid remote identity | `agent` |

`discover` establishes TCP reachability, not authentication. `probe` succeeds only
with a valid authenticated identity, and includes the remote name and version.

`exec.agents` accepts 1..=256 host:port strings, runs four at once, and returns
results in input order, including duplicate targets. `agent` and `agents` are
mutually exclusive. Command output retains the last 1 MiB per stream and names
truncation, encoding, byte counts, and decoding loss. Protocol 2 requires both
binaries to be upgraded together.

## The two places a caller goes wrong

Everything else on this surface fails safely. These two do not, and both are the kind of mistake
that looks like progress.

**`kill` needs `pid` or `name`.** Neither is marked required in the schema, because JSON Schema
cannot say "exactly one of these" without `oneOf`, which reads worse than the six words the
description spends on it. A call with only `agent` is refused -- it does not kill everything.
`pid` never needs `confirmed`; `name` can match more than one process, so it does. A name match
is **exact**; `ps`'s `name` is a **substring**, and the two are deliberately different words on
the two tools, which is worth knowing before using one to find what the other will stop.

**`grep`'s `pattern` is a substring, not a pattern.** `ERROR|FATAL` finds nothing here and finds
both in a ripgrep-shaped tool. An **empty string matches every line**, which is a valid call and
almost never the intended one: `3 matches` for a search the caller meant to narrow is a wrong
answer that looks like a right one. `docs/ROADMAP.md` M10 records the difference in capability
as open.

## What a call and a reply look like

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call",
 "params":{"name":"check","arguments":{"targets":["10.0.0.5:8787"]}}}
```

```
live 10.0.0.5:8787 connected
1 of 1 live
```

Plain text, not JSON inside JSON: the protocol already wraps the result, a second encoding would
be a second thing to document, and a line of text is easier for a reader than an escaped string.

`exec` sets `isError=true` for nonzero command exits, timeouts, or failed calls.
A batch is an error if any target fails. `probe` is an error unless authentication
and identity validation succeed. Truncation alone does not fail an execution.
An empty query result or an empty discovery scan is not an error.

The same holds inside a result. `ps` on an empty machine answers with the counts that make the
emptiness readable; `ls` distinguishes a directory that is empty from one that is not there;
`grep` says whether it searched and which encoding it read. **A result that could be read two
ways is the failure these tools exist to prevent** -- an agent acting on `count: 0` is one step
from overwriting a running binary, which is where `docs/ROADMAP.md` M10 starts.

**A transfer's result ends with `(verified)`, and that word is there because of what a bare
digest invites.** The receiving side hashes what it wrote and compares it against the manifest
before the real path is touched, so a result line is printed only for a transfer that matched
-- a mismatch is a refusal, and prints no line at all. Without the word the line read as "here
is a digest, check it", which is a round trip for a comparison already made, and a false alarm
the moment the file has changed in between, which a log still being written does by definition.
The tool descriptions carry the same fact before the call rather than after it: both transfers
are *verified by digest*, so a caller that reads the list has no reason to plan the second
check.

## File paths

Local paths -- `testbed.spec`, `push.from` and `pull.to` -- accept absolute paths
and parent components. Relative paths use the MCP server's working directory, just as
they do on the CLI. The server account's filesystem permissions apply.

Remote paths -- `push.to`, `pull.from`, `ls.path`, `grep.path`, `tail.path`
and `spawn.output` -- are absolute or relative to the agent's startup directory.
Parent components, other Windows drives and ordinary UNC shares are allowed. The agent
account's filesystem permissions apply, as they already do to `exec`. The `--root`
option has been removed; setting the startup directory only selects a relative-path base.

Remote file paths still pass through `Destination::resolve` for platform-specific
checks: empty paths, NUL bytes, Windows devices, alternate streams and ambiguous names
are refused. File-type validation, size limits and digest verification are unchanged.
See `docs/transfer.md` T1. Authentication is unchanged; command replies use protocol 2.

## Protocol decisions worth knowing

- **Newline-delimited JSON**, one message per line. Not `Content-Length` framing: reading the
  wrong protocol gives a server that hangs on the first message with no error.
- **The client's protocol version is echoed, not checked.** Comparing it against a constant
  would break the day a client moves on, and the client is better placed to decide whether it
  can read the replies. (This is the MCP protocol version and has nothing to do with
  `docs/VERSIONING.md`, which is the number the agent and the tool exchange.)
- **A notification gets no reply.** JSON-RPC forbids one.
- **A bad tool call is a result with `isError`, not a protocol error.** The request was well
  formed; the answer is "you called it wrong". A JSON-RPC error would say the transport failed.
- **A malformed line is reported and the loop continues.** A server that dies on one bad line is
  a server a stray byte can kill.

## Why each tool is separate, and what keeps it that way

`check` needs nothing but an address; `testbed` needs a file on disk; `exec` needs a command.
Folding them together produces a description that has to explain each, which is where a manual
starts. **`push` and `pull` are two tools rather than one with a direction** for the same
reason: one tool would have two mutually exclusive readings of `from` and `to`.

`spawn` is not `exec` with a flag: **`exec` waits**, so a program meant to keep running holds
the request, the connection and the agent's pipes until it exits. `grep` and `tail` are two
tools because they are two questions -- *where is the last ERROR* and *what does the end of this
log say* -- and folding them means a `pattern` argument that is sometimes ignored.

An earlier generation of this idea grew to seventeen tools and 13,758 characters of description,
most of it spent telling the reader when to use a *different* tool. Nobody decided to build
that; it accumulated one reasonable-looking addition at a time.

Three rules keep it from happening again, all enforced by `tests/tool_surface.rs`: **the tool
count is asserted**, **no description may name another tool**, and **every description has a
character budget**. Growing the surface means changing the asserted number and saying in the
commit why the new question needs its own tool.

`logs` is **absent, not stubbed**. Reading a log is reading a file, so it is `pull` with a path,
and a tool that answered "not implemented" would be worse than a missing one: the agent has spent
a turn and cannot tell a missing feature from a broken one.

## What has been measured, and what has not

The descriptions are short and every one fits its budget. Whether they are **understandable** is
a different claim, and it was tested once: `experiments/m5-tool-readability.md` gave the tool list
to readers with no other context. Four of five refused to fabricate an answer and named the
missing address, command or path instead -- which is the failure mode an agent actually damages
things with.

What that experiment also found is the boundary of the approach: a short description can say what
a tool does and cannot say **when to prefer it** over the obvious alternative, because that is the
sentence the rules above exist to keep out. A reader reached for `exec` with one line of `cmd`
where `testbed` was intended, and that is a reasonable route, not a misreading.
