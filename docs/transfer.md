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

**Item 2 -- no pipelining -- survives, conditionally.** It holds if chunks are
strictly one at a time, so the reader never holds unread bytes it did not ask for.
It dies the moment throughput is bought by writing several chunks before reading
anything, because then a reader can be holding surplus bytes when it finishes a
message, and that is the shape that allows a boundary to be reinterpreted.

> **One chunk at a time is the choice, and it is chosen here rather than inherited.**
> The cost is round-trip latency per megabyte on a LAN; the thing bought is that the
> desynchronisation defence keeps holding.

## The design

```
one connection:
  frame 1      Kind::Sealed -> seal(manifest)      { path, bytes, sha256 }
  frame 2..k   Kind::Sealed -> seal(chunk)         k = ceil(bytes / CHUNK)

CHUNK = 1 MiB, well under MAX_PAYLOAD, so a chunk never meets the frame ceiling
```

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

## What this changes in `docs/framing.md`

Its item 9 is no longer a defence of the protocol; it is a defence of *this*
protocol's message count, replaced by T11. Its item 2 now depends on the choice made
above. **Both edits belong in the commit that implements the transfer**, because a
defence list that describes a design nobody is using is worse than no list -- it is
believed.
