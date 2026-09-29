# The framing, and the four defences that live outside it

`crates/linklet-core/src/frame.rs` lists ten ways a length-prefixed protocol goes
wrong and defends against six of them inline. The other four belong to the
connection rather than to a frame, so they live in
`crates/linklet-adapters/src/connection.rs` and are written down here -- a defence
that exists only in someone's head is a defence that gets removed by the next
person tidying up.

## The claim this replaces HTTP on

Worth restating, because everything below assumes it:

> **A bug in the framing causes a refusal, not a forged message.**

Every payload goes straight into an AEAD that authenticates it, so an attacker who
lies about a length gets bytes that will not open rather than a message that is
acted on. That is the difference from HTTP, where a `Content-Length` that
disagrees with a `Transfer-Encoding: chunked` lets an attacker inject a request the
server acts on, because nothing downstream can tell a smuggled request from a real
one.

The failure mode here is **denial of service, not compromise**. That is a real
difference and it is the whole argument for the trade. It is not "this cannot go
wrong", which is why the four below exist.

## 1. The read timeout (item 1)

A peer that declares a large length and then sends nothing holds a thread and a
connection. Nothing inside the frame module can stop that, because it has no clock.

**Every read on a connection has a timeout**, and a timeout is a refusal rather
than a hang. The budget is the caller's: the host allows the command's own deadline
plus a fixed allowance for the reply, and the agent allows a fixed budget for a
request to arrive.

The failure that this replaces is worth naming, because the project has already
produced it once in a different shape: a client that gave up before the agent and
reported a **transport failure** for something the agent would have described
correctly. A timeout that fires says "no reply within N seconds"; it does not
pretend to know why.

## 2. No pipelining (item 2)

The protocol is one request, one reply, in that order, and the reader never has
unread bytes it did not ask for.

This is the defence against desynchronisation, and it is a property of the
connection rather than of a frame: if a reader can be holding surplus bytes when it
finishes a message, then a boundary can be reinterpreted, and that is the smuggling
shape. Refusing to pipeline means the question never arises -- there is no state in
which surplus bytes exist.

A connection carries at most **two** messages: the hello, and one sealed request
that answers it. The count is the defence for item 9, and it is two rather than
"some" because the handshake exists to establish one session for one request.

## 3. The magic byte (item 7)

Already inline, listed here because it is the one an operator meets first: pointing
the host at the wrong port produces "the first byte is 0x47 and this protocol starts
with 0x4c", which names the problem. Without it, three bytes of a foreign protocol
would be read as a length and the error would arrive as an allocation attempt.

## 4. Every write result is checked (item 10)

`write_all`, never `write`, and the error is propagated rather than dropped. A
truncated write that nobody noticed is a receiver waiting for the rest of a message
the sender believes it sent -- and with a read timeout, that becomes a refusal that
looks like a slow peer.

**A write also has a deadline**, which the list above does not mention because it is
not a framing failure: a reply can be as large as a command's output, and a peer
that stops reading fills its own receive window and leaves the sender blocked in
`write_all` for as long as the socket lives. On the agent that is a thread held
permanently by a caller that never reads. The budget is the same one a read gets.
A write that never returns has no result to check, so it is given a bound instead --
and a write that times out partway leaves the connection desynchronised, which is
why the connection is then closed rather than reused.

## 5. A frame that timed out halfway is not a frame

Not on the ten-item list, and found while writing the connection: `read_exact`
consuming part of a payload before its deadline and then failing leaves those bytes
gone. The next read would begin in the middle of the previous message -- which is
exactly the desynchronisation item 2 exists to prevent. So a connection that fails
a read for any reason **refuses to be read again**, and says so, rather than
resynchronising on whatever arrived.

The same applies to a peer that is gone. A clean close and a reset are not told
apart, because the caller does the same thing about either, and on Windows a reset
carries a localised message that would otherwise be the sentence a user reads.

## What is still not defended, and is known

- **An attacker who can drop bytes can stall a connection** until its timeout. That
  is a denial of service and it is not distinguishable from a slow network. It is
  the same gap the channel module records from the other direction.
- **A frame is read into memory in full.** `MAX_PAYLOAD` is 16 MiB and the real
  ceiling is that multiplied by the number of connections. Streaming is what would
  remove the limit, and it is not done.
- **No rate limiting.** A peer that connects, is refused, and reconnects in a loop
  costs the agent a thread each time. Bounded by the operating system's backlog and
  nothing this project controls.
