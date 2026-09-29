# Decisions, and what they cost

A record of choices that are not obvious from the code, with the reasoning at the
time. The point is that a decision whose reason is not written down gets re-made
by the next person, usually in the other direction.

---

## D1. `linklet-core` depends on no crate that does I/O.

**Decided at the skeleton, narrowed twice since.**

Originally: `linklet-core` depends on no crate at all. It now allows
`serde_json`, and the rule was rewritten rather than quietly broken.

What the rule was always protecting is that core does no I/O -- its tests need no
network, no files and no cleanup, so they run in milliseconds and cannot fail for a
reason outside the code. A crate that parses JSON and touches nothing does not
threaten that.

What was wrong was the wording. "Depends on nothing" is satisfied by a rule rather
than a reason, so it stayed in force past the point where its reason applied. The
gate now takes a named allowlist where every entry carries a sentence explaining
why that crate does no I/O, and a second test checks the list actually matches the
manifest -- an allowlist that matches nothing looks green and admits everything.

The core is where the decisions live, and a decision that links against a library
is a decision that cannot be tested without that library. `linklet-core` having an
empty `[dependencies]` section is what makes its 200-odd tests run in
milliseconds with no network, no temp files and no cleanup. That property has
paid for itself repeatedly, and it is enforced by the compiler rather than by
anyone remembering.

**But it is a rule about the core, not a rule about the project**, and it was
being read the second way. `linklet-adapters` already depended on `linklet-core`;
adding a cipher there changes nothing about the core's promise. `AGENTS.md` rule 1
had the answer from the beginning -- "something in `core` needs I/O? It belongs in
`adapters`, behind a trait `core` defines" -- and cryptography is the same shape as
I/O.

`linklet_core::channel` declares what a sealed conversation is.
`linklet-adapters/src/channel.rs` implements it with `chacha20poly1305`, `hkdf`
and `sha2`. The core still has no dependencies; the arithmetic is in crates other
people have attacked.

### What the JSON swap cost, stated rather than glossed

Eleven tests used to assert the exact words the old parser produced, and one asserted
an exact byte offset. None of those assertions survive, because the words and the
offset convention are `serde_json`'s now. The tests assert properties instead:
malformed input is refused, the refusal carries words, and the position is inside the
input and moves with the problem.

That is a genuine loss of specificity. It is also the honest boundary: a test that
asserts a dependency's phrasing is a test that breaks when the dependency is
patched, and a suite that cries wolf gets ignored.

### The part of this that went wrong

While the crate registry looked unreachable, a SHA-256 was **written by hand** in
the adapters to keep the project dependency-free. It was wrong:

- the padding length was computed incorrectly **three times**;
- the NIST vectors caught it immediately -- `abc` returned the initial state,
  meaning the compression function was never called at all;
- one observation (`Vec::resize(55)` producing `len == 63`) was never explained,
  and the half-working file was deleted rather than debugged further.

The registry was reachable the whole time. The cause was a **stale proxy address in
a git config** (`127.0.0.1:7892` where the running proxy was on `7890`), and it only
affected tools that read git's configuration -- which cargo does, because of
`git-fetch-with-cli = true`.

**Two lessons, and the second is the one worth keeping.**

1. A hash is a thing with published test vectors and it still failed repeatedly.
   That is a fair estimate of what would happen to a cipher, which has none.
2. **"Zero dependencies" is a means, not an end.** The end is doing the work
   correctly and quickly. That discipline earned its keep for JSON, MCP and HTTP,
   where a specification could be checked against -- and it actively cost value
   for cryptography, where it cannot. The rule was applied past the point where
   its reason held, and the reason was never re-examined because the rule had
   become an identity.

### What still has a hand-written implementation, and why

| component | hand-written | why it is defensible, or not |
|---|---|---|
| JSON codec | **was**, 672 lines; now 345 delegating to `serde_json` | It was correct and tested, and it was still the wrong call: it parsed untrusted input from the network, and a hand-written parser in that position is the classic remote-vulnerability shape. The 345 lines that remain are the domain enum and the two conversions between it and `serde_json`, not a parser. |
| HTTP (client and server) | **deleted at M7** | ~300 lines each, and the argument for keeping them was that only `Content-Length` framing was involved in a protocol this project owns both ends of. It was defensible and it was still not the right call -- see D4, which had already written down what would replace it. |
| Framing (client and server) | yes, ~250 lines including tests | The replacement for HTTP, and a smaller surface: a magic byte, a kind byte, a 32-bit length. `crates/linklet-core/src/frame.rs` lists the ten ways it can go wrong and where each defence lives, six inline and four in the connection. Unlike a request parser, it has no grammar to be right about -- and everything it carries goes into an AEAD that refuses a message it did not authenticate. |
| MCP framing | yes, ~240 lines | Newline-delimited JSON-RPC, no batching, no negotiation. Defensible. |
| SHA-256 | **deleted** | Not defensible. Published vectors and it still failed three times. |
| Channel | uses vetted crates | Not defensible to hand-roll. See above. |

The JSON row is the honest one to leave in: it worked, it is tested, **and it
should still have been `serde_json`**. Being correct is not the same as being the
right use of effort, and 30 tests over a codec nobody asked for is 30 tests that
are not covering the tool.

---

## D2. The channel is a trait in the core, and the keys are per-direction.

**Why a trait:** see D1. The core says *what*; the adapter says *how*; the reason
hand-rolling was tried is removed as a possibility rather than discouraged.

**Why two keys:** HKDF is run twice with different `info` strings, one key per
direction. Reusing one key in both directions means two independent nonce
sequences under one key, and the first repeated nonce destroys confidentiality and
allows forgery. That failure is invisible from outside.

**Why `Role` is a parameter, which it originally was not:** the first version
derived both keys and assigned them the same way at both ends, so the host sealed
with one key and the agent tried to open with the other. Every round trip failed.

That is the *safe* direction of that mistake, and it was luck rather than design.
Had the derivation produced one key used for everything, every round trip would
have passed and the result would have been the silent vulnerability above. **A
passing test is not evidence that the keys are separate**, so the role is explicit
and the mirroring is by construction.

---

## D4. The HTTP layer is hand-written, and the server side should not be.

**Decided, not yet done.** Recorded now because the reasoning is the useful part and
because it was pointed out by the same reader who caught the JSON codec: a second
hand-rolled protocol, in the path that reads untrusted input from the network.

Two hand-written layers exist:

- `crates/linklet-agent/src/http.rs` -- a request parser and reply writer, about 450
  lines, **facing attackers**
- `crates/linklet-client/src/lib.rs` -- a request builder and reply parser, about 200
  lines, talking only to this project's own agent

HTTP parsing is the most attacked surface on the internet, which makes it a worse
place to hand-roll than JSON was -- and it had already produced a bug: the body was
built with `String::from_utf8_lossy`, which silently replaces anything that is not
UTF-8, in the path that handles input from the network.

### The choice, and the constraint that decides half of it

`tiny_http` for the agent and `ureq` for the client, rather than `hyper` and `axum`:
this project is blocking and thread-per-connection, and hyper would bring an async
runtime and change the shape of everything around it for no gain at this size.

**The client half is not settled, and here is why.** The protocol requires two
messages on one connection: the handshake, then the request sealed under the session
it produced. That is what keeps the agent stateless. `ureq` manages a connection
pool and does not promise to put the second request on the same connection -- and a
client library that reassigns it would break the protocol in a way that looks like a
wrong token.

Three ways out, in the order I would take them:

1. **Prove `ureq` reuses the connection** for two sequential requests to one host, and
   if it does, use it.
2. **Keep the client hand-written**, because it parses replies from this project's own
   agent rather than from strangers, and say so instead of implying symmetry.
3. **Give the agent a session table.** Last, because a session that outlives a
   connection needs an eviction policy, and an eviction policy is a way to be
   exhausted.

### The observation worth more than the decision

**The protocol design constrains the library choice, and the two are not independent.**
Wanting to drop a hand-written layer does not settle what replaces it: the
two-message handshake is what makes `ureq` a question rather than an answer.

Which raises the option that is not on the list because it is larger: since every
message is sealed and both ends are this project's, **HTTP is carrying no weight**.
Its methods, paths, headers and status codes are vocabulary nobody reads and surface
nobody needs, and a length-prefixed frame would be about fifty lines instead of four
hundred and fifty. That is the design that fits the channel, and it is written here
rather than done, because replacing a working protocol is work that has to be paid
for by something.

### Done at M7, and the option that was taken

The transfer was the something. A transfer is a manifest plus N chunks on one
connection, and the two-message HTTP shape had no room for it: every chunk would have
been a request, and the reason for a length-prefixed frame -- the message count is
bounded by the declared size rather than by a constant -- only exists once you stop
pretending each message is a request.

So the fourth option was taken, in the form the last paragraph above describes:
`crates/linklet-agent/src/http.rs` is gone, `crates/linklet-adapters/src/connection.rs`
is what replaced it, and `linklet_core::wire` now carries a request and a reply rather
than a method, a path and a status. What that cost and bought, stated rather than
implied:

- **The status code is gone, and the distinction it carried is stronger for it.** The
  protocol's real distinction was never 200-versus-400; it was "a command ran and
  failed" versus "a request could not be made". HTTP said both with a status line and
  a body shape, and the client had to know which. Now a reply is a result or a
  refusal, and a result holding an exit code of 1 cannot be read as a transport
  failure because it is not one.
- **`tiny_http` and `ureq` were never needed**, which retires the whole of the
  `ureq`-connection-pool question above. A protocol this project defines both ends of
  does not need a library to disagree with itself about connection reuse.
- **The reading surface facing attackers got smaller, which was the point.** A
  request line, headers and a `Content-Length` became a six-byte header whose ten
  failure modes are listed in `crates/linklet-core/src/frame.rs` and whose four
  connection-level defences are in `docs/framing.md`. The parser that had to be right
  about a grammar nobody used is gone; what replaced it is not a parser at all.
- **What it cost**: a protocol version that does not negotiate, so an old host and a
  new agent fail with "the first byte is 0x47" or an unknown `op` rather than with a
  406. That is a real cost and it is accepted for a tool with one deployment at a
  time -- `docs/ROADMAP.md` M9 is where version negotiation would go.

---

## D3. `Role` and the missing handshake.

Two sides that share a secret derive the same keys and talk. There is **no
handshake**, so **no forward secrecy**: an attacker who records a session and later
learns the secret can read that session.

Fixing it needs an ephemeral key exchange, which is a larger and more delicate
piece of work. It is written here, and in `linklet_core::channel`'s documentation,
because a reader who assumes encryption gives forward secrecy is worse off than
one who knows it does not.
