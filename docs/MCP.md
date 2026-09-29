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

**One tool.**

| tool | question it answers |
|---|---|
| `check` | does each host:port accept a TCP connection |

That is the whole list, and the size is the point. An earlier generation of this
idea grew to seventeen tools and 13,758 characters of description, most of it
spent telling the reader when to use a *different* tool. Nobody decided to build
that; it accumulated one reasonable-looking addition at a time.

The three rules that keep it from happening again are in
`crates/linklet-core/src/tool.rs`, and `crates/linklet-core/tests/tool_surface.rs`
enforces them: the tool count is asserted, no description may name another tool,
and every description has a character budget.

`exec`, `logs` and file transfer are **absent, not stubbed**. They need something
on the far side to talk to, and there is nothing there yet. A tool that answers
"not implemented" is worse than a missing one: the agent has spent a turn on it
and cannot tell a missing feature from a broken one. They arrive with the agent
that serves them.

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
