# The MCP surface

`linklet mcp` speaks the Model Context Protocol on stdin and stdout, so an AI agent can call
this tool directly. No configuration and no port: the client starts the process and talks to it.

```json
{ "command": "path/to/linklet", "args": ["mcp"] }
```

**What an agent reads is the descriptions and schemas in `crates/linklet-core/src/tool.rs`,
not this file.** This is the maintainer's document: what is on the surface, why each tool is
separate, and where a caller can go wrong. `crates/linklet-core/tests/tool_surface.rs` enforces
the rules; this explains them.

## The surface

Eleven tools, one per intent.

| tool | question | required arguments |
|---|---|---|
| `check` | does each `host:port` accept a TCP connection | `targets` |
| `testbed` | does a machine match a specification file | `spec`, `target` |
| `exec` | run a command on an agent's machine and wait for it | `agent`, `command` |
| `ps` | what is running on an agent's machine | `agent` |
| `kill` | stop processes on an agent's machine | `agent`, **and `pid` or `name`** |
| `spawn` | start a program on an agent's machine, without waiting | `agent`, `command`, `output` |
| `grep` | find lines in a file on an agent's machine | `agent`, `path`, `pattern` |
| `tail` | read the end of a file on an agent's machine | `agent`, `path` |
| `ls` | list a path on an agent's machine | `agent`, `path` |
| `push` | copy one local file to an agent | `agent`, `from`, `to` |
| `pull` | copy one file back from an agent | `agent`, `from`, `to` |

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

**`isError` is true only when the call could not be made.** Every machine being down is a
successful call carrying bad news, and marking it an error would teach an agent to retry a tool
that worked. This is the distinction the whole project is arranged around: *it ran and failed*
is not *it never ran*.

The same holds inside a result. `ps` on an empty machine answers with the counts that make the
emptiness readable; `ls` distinguishes a directory that is empty from one that is not there;
`grep` says whether it searched and which encoding it read. **A result that could be read two
ways is the failure these tools exist to prevent** -- an agent acting on `count: 0` is one step
from overwriting a running binary, which is where `docs/ROADMAP.md` M10 starts.

## Paths are a boundary

Three arguments name something on **this** machine and refuse an absolute path or `..`:
`testbed`'s `spec`, and the local half of a transfer -- `from` on `push`, `to` on `pull`. An
agent handed a filesystem-wide read has a capability nobody asked for, and the refusal names the
argument rather than surfacing later as an operating-system error.

The other half of a transfer belongs to the agent, which resolves it against its own transfer
root. It is **not** checked here: checking it twice would be two answers to the question
`docs/transfer.md` T1 asks, and two checks that disagree are worse than one that does not run.

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
