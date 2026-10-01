# The MCP surface

`linklet mcp` speaks the Model Context Protocol on stdin and stdout, so an AI
agent can call this tool directly. There is no configuration and no port: the
client starts the process and talks to it.

```json
{
  "command": "path/to/linklet",
  "args": ["mcp"]
}
```

## What is on the surface, and what is not

**Ten tools.**

| tool | question it answers |
|---|---|
| `check` | does each host:port accept a TCP connection |
| `testbed` | does a machine match a testbed specification file |
| `exec` | run a command on a remote agent and return its output |
| `ps` | what is running on a remote agent's machine |
| `kill` | stop something running on a remote agent's machine |
| `spawn` | start a program on a remote agent's machine, without waiting for it |
| `grep` | find lines matching text in a file on a remote agent's machine |
| `tail` | read the last lines of a file on a remote agent's machine |
| `push` | copy one local file to a remote agent |
| `pull` | copy one file back from a remote agent |

The second one was added late and is the reason the count is asserted rather
than described. `linklet testbed check` shipped as a command first, and for a
while it existed where no agent could reach it: the capability was built and the
surface was never told. An agent cannot ask for what it has not been told about,
so that is a bug in the opposite direction from the usual one -- not a tool with
nothing behind it, but something behind it and no tool.

The sixth is `ps`, and it is the one the deploy loop could not be closed without:
**kill the old build, push, start, confirm it stayed up** needs "is the old build
still running", and `exec` could not answer it. Running `tasklist` through `exec`
returns text an agent has to parse, and -- the part that matters -- it returns
*nothing* when the answer is "nothing matched", with none of the counts that make
an empty answer readable. That is the mistake `docs/ROADMAP.md` M10 records from
a real machine, where `count: 0` was one step from overwriting a running binary.
`ps` is therefore not "exec with a different command"; its **result** is a
listing, and the listing carries what was examined, what filter was applied and
what the machine could not say.

The seventh is `kill`, and it is a tool rather than a command-line `taskkill`
through `exec` for one reason: **a refusal is something an interface has and a
command line does not.** This tool will not stop the agent that is serving it, and
it will not do a bulk match that was not confirmed. Through `exec` both of those
are one careless string away, and the caller learns about it afterwards.

The eighth is `spawn`, and it is not `exec` with a flag. **`exec` waits**, which is
right for a build step and wrong for a program meant to keep running: the request,
the connection and the agent's pipes are all held until the program exits, and a
program that does not exit holds them forever. `spawn` starts the program with its
own output file and answers with a pid. What it deliberately does **not** answer is
whether the program is healthy -- that is `ps`'s question, asked a moment later,
which is the order the deploy loop actually runs in.

The ninth and tenth are `grep` and `tail`, and they are two tools because they are
two questions: *where is the last ERROR* and *what does the end of this log say*.
Folding them would mean a `pattern` argument that is sometimes ignored. What they
share is the reason they exist -- **a pull is digest-verified and root-bounded, and
it is the wrong tool for a two-gigabyte log**, because the answer is inside the file
and the cost is moving it. Neither is `exec` with a `findstr`: that returns text with
no count, no truncation and no encoding, and an empty result from it cannot be told
from a file that could not be read.

**Their answers are searches rather than lists of lines**, and that is the design.
Every reply carries whether the search happened, whether it stopped early, whether
the file was cut short at the byte ceiling, and which encoding the bytes were read
as. A tool that returned only the matching lines would make "this log has no errors"
and "that file could not be opened" the same answer.

**Their pattern is a substring, and the reference implementation's is a ripgrep
pattern.** `ERROR|FATAL` finds nothing here and finds both there, which is a
difference in capability rather than a smaller step, and `docs/ROADMAP.md` M10
records it as the open question it is.

That is also the argument for why these are separate rather than one tool with
several argument sets. `check` needs nothing but an address; `testbed` needs a
file on disk; `exec` needs a command. Folding them together would produce a
description that has to explain each, which is where the manual starts. **`push`
and `pull` are two tools and not one with a direction**, for the same reason: a
single tool would have two mutually exclusive readings of `from` and `to`, and a
description that has to say which one is local this time.

The size is still the point. An earlier generation of this idea grew to
seventeen tools and 13,758 characters of description, most of it spent telling
the reader when to use a *different* tool. Nobody decided to build that; it
accumulated one reasonable-looking addition at a time.

The three rules that keep it from happening again are in
`crates/linklet-core/src/tool.rs`, and `crates/linklet-core/tests/tool_surface.rs`
enforces them: the tool count is asserted, no description may name another tool,
and every description has a character budget. Growing this list means changing
the asserted number and saying in the commit why the new question needs its own
tool.

`logs` is still **absent, not stubbed**. Reading a log is reading a file, so it
is `pull` with a path -- and a tool that answered "not implemented" would be
worse than a missing one: the agent has spent a turn on it and cannot tell a
missing feature from a broken one.

### The `spec` argument is a path, and paths are a boundary

`testbed` reads a file, so it takes a path, so it is the one argument in this
surface that can name something outside the project. It refuses an absolute path
and it refuses `..`. The reason is not that this tool is dangerous; it is that an
agent handed a filesystem-wide read has been given a capability nobody asked for,
and the refusal names itself instead of surfacing later as a path error from the
operating system.

**A transfer has a local half, and that half is bounded the same way.** On `push`
it is `from`; on `pull` it is `to`. Both refuse an absolute path and both refuse
`..`, and the refusal names the argument rather than the machine. The other half
belongs to the agent, which resolves it against its own transfer root: checking it
here as well would be a second answer to the question `docs/transfer.md` T1 asks,
and two checks that disagree about what is allowed are worse than one that does
not run.

## What a call looks like

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call",
 "params":{"name":"check","arguments":{"targets":["10.0.0.5:8787"]}}}
```

and the reply carries plain text:

```
live 10.0.0.5:8787 connected
1 of 1 live
```

Not JSON inside JSON. The protocol already wraps the result, a second encoding
would be a second thing to document, and a line of text is easier to read than an
escaped string.

`isError` is `true` only when the call could not be made. Bad news -- every
machine down -- is a successful call, and marking it an error would teach the
agent to retry a tool that worked.

## The protocol decisions worth knowing

- **Newline-delimited JSON**, one message per line. Not LSP's `Content-Length`
  framing: reading the wrong protocol gives a server that hangs on the first
  message with no error, which is a memorable afternoon.
- **The client's protocol version is echoed, not checked.** Comparing it against
  a constant would break the day a client moves on, and the client is better
  placed to decide whether it can read the replies.
- **A notification gets no reply.** JSON-RPC forbids one, and a server that
  answers them earns a client that logs "unexpected response id: null" once per
  notification.
- **A bad tool call is a result with `isError`, not a protocol error.** The
  request was well formed; the answer is "you called it wrong". A JSON-RPC error
  would tell the agent the transport failed.
- **A malformed line is reported and the loop continues.** A server that dies on
  one bad line is a server a stray byte can kill.
