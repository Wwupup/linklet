# Moving a file, and every way that goes wrong

`docs/framing.md` is about a message. This is about a file, which is a different
problem with a different failure list -- and the list is longer, because a transfer
has state a message does not: an offset, a declared size, a destination on someone
else's disk, and a temporary file that must not survive a failure.

Written before the code, for the same reason as the framing document: **"it is only
a stream of chunks" is not an argument**, and this project has already paid twice
for a rule that was satisfied by a rule rather than a reason.

## The framing does not create the debt. The connection protocol does.

Worth separating, because "we chose hand-written framing and now large files are a
problem" would be the wrong conclusion.

`magic | kind | length | payload` is the shape binary protocols use for streamed
data -- TLS records, SSH, gRPC's frames. **It chunks naturally**, each chunk stays
under `MAX_PAYLOAD`, and every one of the fifteen framing tests still applies per
chunk: the header is pinned by value, the length is refused before an allocation,
a payload is never scanned for a boundary.

`MAX_PAYLOAD` is a **per-frame** limit and not a per-file one. That distinction was
right, and it does not need to change.

## The two documented defences that move

Both are in `docs/framing.md`, and changing a written defence is a cost that has to
be paid explicitly rather than discovered later.

**Item 9 -- "a peer sends messages forever" -- does not survive.** A transfer is one
manifest plus N chunks, so the message count is no longer bounded by a protocol
constant. Its replacement is a **declared total size plus a policy ceiling**: the
count is bounded because the size is. That ceiling is now the number that decides
whether `push` is useful at all, which makes it a decision rather than a constant.

**Item 2 -- no pipelining -- survives, and the condition it was written with is
narrower than it needs to be.**

As written it holds only "if chunks are strictly one at a time, so the reader never
holds unread bytes it did not ask for", and it "dies the moment throughput is bought
by writing several chunks before reading anything". The second half does not survive
contact with the implementation, and the reason is worth stating because the
conclusion reached is **stronger** than the one written here.

**The defence is about the reader, and it is a property of the reader, not of the
sender.** The failure it prevents is a message boundary being reinterpreted, and a
boundary can only be reinterpreted by something holding bytes it did not ask for.
`linklet-adapters`' connection holds none: there is no buffered reader anywhere in
it, so every read is `read_exact` for exactly the six-byte header and then exactly the
declared payload. When a frame is finished, the next read begins at the next frame's
magic byte -- which is where the sender put it, however far ahead the sender has run.

So the sender does write ahead: a push fills the socket as fast as the disk and the
network allow, **one whole frame per write**, and the operating system's own flow
control bounds what is in flight. What it never does is write a partial frame and then
something else, and what the reader never does is read a byte it did not ask for.

**The defence therefore holds unconditionally rather than conditionally**, and it is
checked rather than asserted: `crates/linklet-adapters/tests/connection.rs` writes two
frames back to back and requires two back, and requires a connection that failed a
read to refuse to be read again rather than resynchronise on what arrived.

The cost claim above -- "round-trip latency per megabyte on a LAN" -- is wrong too, and
it was wrong when it was written: the design block below has never had an
acknowledgement in it, and without one there is no round trip per chunk. What the
choice actually costs is **one chunk of memory per direction per connection**, and what
it buys is that the desynchronisation defence does not depend on the sender's behaviour
at all.

## The design

```
one connection:
  frame 0      Kind::Hello  -> the handshake, in the clear
  frame 1      Kind::Sealed -> seal(manifest)      { op: "push", path, bytes, sha256 }
  frame 2      Kind::Sealed -> seal(accepted)      { accepted: true, bytes }
  frame 3..k   Kind::Sealed -> seal(chunk)         k = ceil(bytes / CHUNK)
  frame k+1    Kind::Sealed -> seal(result)        { bytes, sha256 }

CHUNK = 1 MiB, well under MAX_PAYLOAD, so a chunk never meets the frame ceiling
```

The `op` field is the one thing here that the prose above does not imply, and it is
there because a manifest and a command arrive on the same connection in the same shape.
It rides *alongside* the manifest's three fields rather than wrapping them, so the
message a receiver reads first **is** the manifest -- which is what the design needs,
since the size in it is what bounds the message count.

The **accepted** frame is T14 below, and it is a separate message rather than a field on
the result for a reason worth stating: the sender needs an answer *before* it sends the
body, and the result can only be computed after the body has arrived. Its shape carries
`accepted` and no `sha256` while a result carries `sha256` and no `accepted`, because two
reply shapes that overlap on `bytes` alone would let a client read an acceptance as a
result.

### The two halves have to overlap

**Frames 3..k are not a batch that one side writes and the other then reads.** The sender
writes a chunk; the receiver reads it, writes it to the `.part` file and updates its digest;
the sender writes the next. A body larger than a socket buffer cannot be done any other way:
the kernel accepts a few hundred kilobytes, the sender blocks on a full send buffer, and a
receiver that has not started reading yet cannot drain it. The two ends therefore **have to be
running at the same time**, which is why `linklet_core::channel::Sealed` requires `Send` and
why `send_body` and `receive_body` are written as functions over a connection rather than as
one call that does both.

**This is not a performance note, it is a liveness one.** It was found by a test that did the
impossible thing -- wrote 3 MiB and then read it, sequentially -- and passed for a long time,
because the socket buffers on the machine it ran on happened to be large enough to hold the
whole body. The identical test failed on a runner where they were not, with
`Timeout { millis: 30000 }`: the pattern is a deadlock when the body is big enough, and a slow
seesaw when it is only nearly big enough. The same test run concurrently takes about a second.

The stale invariant was in the test and not in the protocol, which is the useful part to
remember: **a test that passes because a buffer was large is not testing the design.** See
`crates/linklet-adapters/tests/transfer.rs`, where the sender runs on its own thread.

**The other direction is the same shape with the manifest on the other side**, because
the size has to come from whoever holds the file:

```
one connection:
  frame 0      Kind::Hello  -> the handshake, in the clear
  frame 1      Kind::Sealed -> seal(request)    { op: "pull", path }
  frame 2      Kind::Sealed -> seal(manifest)   { path, bytes, sha256 }
  frame 3..k   Kind::Sealed -> seal(chunk)      k = ceil(bytes / CHUNK)
```

The manifest is a *reply* here and a *request* there, and that is the whole difference
between the two operations from the wire's point of view. Both sides therefore run the
same receiver, and **the same fourteen failures apply in both directions** -- including
T1, which is about to be read as much as about to be written: without the root a pull
would read any file on the machine, which is a different severity of mistake and not a
smaller one.

A pull has no *accepted* frame, and it does not need one: the host is the receiver and it
knows the size and the digest before the body arrives, so its refusal is its own and
already in hand. What it can do is close while the agent is mid-write, and the agent then
reports a broken connection to nobody. That is recorded rather than fixed: the side that
has something to say is the one that refused.

Streaming on both ends: read 1 MiB, seal it, frame it, write it. **The file is never
in memory whole**, on either side.

The receiver:

1. reads the manifest and **refuses before starting** if the declared size is over
   the ceiling
2. validates the destination path (T1, T2 below)
3. opens `<path>.part` and writes each chunk as it arrives
4. **checks after every chunk that the running total has not passed the declared
   size** (T4)
5. on reaching the declared size: flush, hash what was written, compare, and only
   then rename over the real path (T7, T9)
6. on any failure: delete the `.part` and never touch the real path (T6, T8)

## The ways it goes wrong

**T1. The destination path escapes where it is allowed to write.** The remote path
is the caller's. `../../Windows/System32/drivers/etc/hosts` is a file write as
SYSTEM on someone else's machine. **This is the most severe item in the document and
it is not a framing problem, which is why the framing analysis did not contain
it.** *Stopped by validating the path against a configured root*: refuse any
component equal to `..`, refuse an absolute path outside the root, refuse a root
that is not itself absolute. The check is on the resolved path, not the string.

**T2. The destination is a symlink pointing elsewhere.** Writing to a path that is a
link writes somewhere the operator did not intend, and the rename would replace the
link rather than follow it. *Stopped by refusing a destination that exists and is
not a regular file*, checked before the `.part` is created.

**T3. The declared size is enormous.** The receiver allocates nothing from the
declared number, but it does agree to receive that many bytes. *Stopped by the
policy ceiling, checked before any chunk is read.*

**T4. The running total passes the declared size.** A sender that declares 100 bytes
and keeps sending fills the disk. This is the transfer equivalent of framing item 3,
and it is the one most likely to be missed because the declared number *was*
checked. *Stopped by checking `written + chunk.len() <= declared` after every
chunk*, not once at the start.

**T5. A chunk arrives after the declared size is reached.** The transfer is over;
anything further is either a bug or an attempt to extend it. *Stopped by treating a
frame after completion as a protocol error and closing the connection.*

**T6. The transfer ends early.** The sender dies, the network drops, the deadline
passes. *Stopped by never renaming a file that is shorter than declared*, and by
deleting the `.part`. A short file at the real path is worse than no file, because
the next step believes it.

**T7. What arrived is not what was sent.** *Stopped by comparing digests*: the
manifest carries the sender's digest, the receiver hashes what it wrote, and the
comparison happens before the rename. The AEAD already authenticates the bytes, so
this catches the layers above it -- a framing bug, a write that silently short-wrote,
a `.part` that was overwritten by something else.

**T8. The disk fills, or a write fails.** *Stopped by propagating the error, deleting
the `.part`, and answering with the reason.* A write error that is logged and
ignored produces a short file that fails T7 on the sender's side with no explanation
of why.

**T9. A failed transfer leaves a file under the real name.** *Stopped by writing to
`.part` and renaming.* The rename is the only moment the real path changes, and it
happens after every check has passed.

**What this does not cover, and it was measured rather than assumed**: a receiver that is
killed *outright* -- not failed, killed -- cannot delete its own temporary, because the
code that deletes it does not run. A 12 MiB pull killed at 900 ms left a 3 MiB
`<name>.part` on the host and, correctly, no file under the real name. The residue is
harmless rather than dangerous: the next attempt to that destination truncates it with
`File::create` before writing a byte. It is also visible, which is the part to know --
an operator who finds a large `.part` on a target has found the evidence of an
interrupted transfer and not a corrupted one. Nothing sweeps them up on startup, and
that is deliberate: deleting files by a naming convention is a destructive action to take
on someone else's machine on the strength of a convention.

Two transfers to one destination at once therefore share a `.part` name, and each
writes at its own offset. At most one of them can pass its digest check, so the other
fails having written nothing under the real name -- **a failure rather than a
corruption**, which is the property that matters. A per-attempt temporary name would
turn that failure into two successful transfers racing to rename over one another,
which is a worse thing to have on someone else's disk.

**T10. The file exists three times in memory.** `seal(&[u8]) -> Vec<u8>`, then the
framed copy, then the file buffer: a 500 MB file does not fit. **This is a channel
API problem and it has to be fixed before the transfer is written, not after.**
*Stopped by sealing into a caller-provided buffer* so the chunk is read, sealed in
place, and written, with one chunk of memory in play per direction.

**T11. The message-count defence is gone.** See above. *Stopped by the declared size
plus the ceiling*, which bounds the count by bounding the bytes.

**T12. A connection carries a transfer and then something else.** Two operations on
one connection would give the reader a reason to hold state across them. *Stopped by
one transfer per connection*, the same rule the two-message protocol already had.

**T13. A crash between the rename and the reply.** The file landed and the caller was
told the transfer failed. Retrying overwrites it, which is safe, so the honest
resolution is **to declare the operation idempotent** rather than to build a
transaction. That is written down rather than left for a caller to guess.

**T14. The receiver's refusal never reaches the sender.** The sender has the whole file
ready, so it streams the body as soon as the manifest is written. A receiver that decides
at the manifest -- the path is outside its root, the size is over its ceiling -- therefore
writes its refusal while the sender is still sending, and then closes **with the sender's
unread chunks in its receive queue**. Windows resets a socket closed in that state, and a
reset discards what the peer had not read yet: the refusal is destroyed in transit. What
the caller reported was *"the agent closed the connection without answering"* -- true, and
no help at all to whoever pushed a build to the wrong place.

*Stopped by answering the manifest before the body*: the receiver's acceptance is a
message, so the sender has something to wait for and sends nothing it is about to have
refused. That also removes the work a doomed transfer would cost the receiver, which
draining the refused body would have paid instead.

**It was not fully stopped, and the remainder took a flaky test to find.** The rule above
covers a receiver that refuses *after* accepting, and it leaves the other case: a receiver
that refuses **at the manifest** writes its refusal and then drops the socket -- and if the
sender has bytes in flight that the receiver never read, dropping the socket makes Windows
reset it, and a reset discards the refusal the sender had not read yet. The symptom is the
one this entry opens with. It is rare because the sender has to lose the race, and it showed
up as **about one full test suite in three, and never once in isolation**: the suite runs
dozens of agents at once, which is the load that loses it.

*Stopped by closing gently*: the agent now **half-closes and drains** whenever the answer it
just wrote is a refusal -- `shutdown(Write)` sends a FIN, which discards nothing, and then the
sender's remaining bytes are read out so that nothing is unread when the socket goes. The
client side was already right: it waits for the manifest's answer before sending a byte of the
body. See `Connection::shutdown_write`, `Connection::drain`, and `server::settle`.

**This one was found on a real machine, and not one of the loopback tests could lose the
race reliably**: on loopback the sender usually wins and the refusal gets read in time, so
every test passed while a real link failed on every refusal. The test that pins it is
therefore about the order rather than the outcome --
`crates/linklet-client/tests/manifest_refusal.rs` runs a fake agent that refuses and then
looks at the socket, so a sender that streams first is caught deterministically.

## What is deliberately not in this milestone

**Resumption.** An offset, a digest of a prefix, and a `.part` that may be from a
different attempt is where the complexity and the bugs live, and none of it is
needed to move a build onto a machine. It is deferred with the reason, not omitted:
a resume that trusts a `.part` it did not verify is a way to install half of one
build and half of another.

**Multiple files.** One transfer moves one file. A directory is the caller's loop,
so that a failure has an obvious meaning.

**Progress reporting.** The caller learns the outcome, and a transfer that is slow
is indistinguishable from one that is stuck until the deadline. Worth having, not
worth building before the thing it reports on works.

**Reading a file the root does not contain.** The root bounds a pull exactly as it
bounds a push, so collecting a log from outside it is not possible. That is the same
decision as T1 seen from the other side: an agent that could read anywhere would be a
file server for the machine, and the caller can configure the root to include whatever
it needs.

## What this changes in `docs/framing.md`

Its item 9 is no longer a defence of the protocol; it is a defence of *this*
protocol's message count, replaced by T11. Its item 2 now depends on the choice made
above. **Both edits belong in the commit that implements the transfer**, because a
defence list that describes a design nobody is using is worse than no list -- it is
believed.
