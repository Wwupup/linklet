# Decisions, and what they cost

A record of choices that are not obvious from the code, with the reasoning at the
time. The point is that a decision whose reason is not written down gets re-made
by the next person, usually in the other direction.

---

## D1. `linklet-core` has no dependencies. `linklet-adapters` has several.

**Decided at the skeleton, reconfirmed at the channel.**

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
| JSON codec | yes, 660 lines | RFC 8259 is exact and every case is testable against it. **Defensible, but `serde_json` is zero lines and was the wrong call on effort alone.** |
| HTTP (client and server) | yes, ~300 lines each | Only `Content-Length` framing, one request per connection, in a protocol this project owns both ends of. Defensible. |
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

## D3. `Role` and the missing handshake.

Two sides that share a secret derive the same keys and talk. There is **no
handshake**, so **no forward secrecy**: an attacker who records a session and later
learns the secret can read that session.

Fixing it needs an ephemeral key exchange, which is a larger and more delicate
piece of work. It is written here, and in `linklet_core::channel`'s documentation,
because a reader who assumes encryption gives forward secrecy is worse off than
one who knows it does not.
