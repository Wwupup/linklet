//! The real channel: ChaCha20-Poly1305, with keys derived by HKDF.
//!
//! Every line of arithmetic here is in a crate that other people have attacked.
//! What this file decides is the part the crates cannot: which key is used for
//! which direction, how a nonce advances, and what is refused. Those are the parts
//! that go wrong in real systems, and they are small enough to read.
//!
//! # The four decisions
//!
//! **Two keys, not one.** HKDF is run twice with different `info` strings, giving
//! one key for host-to-agent and one for agent-to-host. Reusing a single key in
//! both directions means two independent nonce sequences under one key, and the
//! first time a nonce repeats under a key, ChaCha20-Poly1305 stops being
//! confidential and the attacker can forge -- a failure that no test can see from
//! the outside.
//!
//! **One session, one key.** The session identifier is a salt in the derivation,
//! generated fresh for every request. So the counter can start at zero every time:
//! what matters is that a key never sees a repeated nonce, and a fresh key makes
//! the counter's starting point irrelevant.
//!
//! **A counter for the nonce, checked on the way in.** Both sides count messages
//! and a message arriving with the wrong count is refused rather than opened.
//! Without that check an attacker can reorder messages within a session, which
//! authenticates each one and still delivers them in the wrong order.
//!
//! **The tag covers the sequence.** The counter is authenticated as associated
//! data, not just assumed. It costs nothing and it means a captured message cannot
//! be relabelled with a different position.
//!
//! # Forward secrecy, and where it comes from
//!
//! [`Handshake`] adds an X25519 exchange: both sides generate a key pair for this
//! handshake and discard the private half when it is done. The session key is the
//! X25519 result mixed with the shared secret, so an attacker who records the
//! traffic and later learns the secret still needs private keys that no longer
//! exist.
//!
//! The shared secret is still what **authenticates** the exchange. Without it the
//! X25519 result would be anonymous and anyone in the middle could complete a
//! handshake with both ends. Mixing the secret in means the attacker can compute a
//! session and it is not the one either end built -- which a test in
//! `tests/handshake.rs` demonstrates rather than asserts.
//!
//! # What is still not here
//!
//! No protocol version negotiation, no cipher agility, and no defence against an
//! attacker who can *drop* messages -- a session that stops responding is not
//! distinguishable from a dead agent. Those are real gaps and they are listed
//! rather than left to be discovered.

use chacha20poly1305::aead::AeadInOut;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};

use linklet_core::channel::{
    Channel, ChannelError, EphemeralPublic, Handshake, Pending, Role, Sealed, SessionId,
};

/// How long a tag Poly1305 appends.
const TAG_BYTES: usize = 16;

/// The length of the key ChaCha20-Poly1305 takes.
const KEY_BYTES: usize = 32;

/// The direction labels fed to HKDF.
///
/// Distinct strings, and the whole reason two keys exist: the same secret with
/// the same salt and two different `info` values produces two unrelated keys.
const HOST_TO_AGENT: &[u8] = b"linklet v1 host->agent";
const AGENT_TO_HOST: &[u8] = b"linklet v1 agent->host";

/// A session built from key material, a salt, and which end we are.
///
/// Extracted so that the shared-secret path and the handshake path cannot disagree
/// about how a session is built. They differ only in what goes in: the handshake
/// feeds the X25519 result *and* the shared secret, so the keys depend on both.
fn session_from(salt: &[u8], material: &[u8], role: Role) -> Result<Session, ChannelError> {
    let hkdf = Hkdf::<Sha256>::new(Some(salt), material);

    // A failure here means the output length is not a valid HKDF length, which for
    // 32 bytes cannot happen; it is handled rather than unwrapped because a panic
    // in a library is a bug report from a user.
    let derive = |info: &[u8]| -> Result<[u8; KEY_BYTES], ChannelError> {
        let mut key = [0u8; KEY_BYTES];
        hkdf.expand(info, &mut key)
            .map_err(|e| ChannelError::Refused(format!("key derivation: {e}")))?;
        Ok(key)
    };

    // Mirrored, not identical. This is the line a round-trip test caught:
    // assigning the same pair at both ends gave one side a key to seal with and the
    // other a different one to open with, and every message failed.
    let (seal_info, open_info) = match role {
        Role::Host => (HOST_TO_AGENT, AGENT_TO_HOST),
        Role::Agent => (AGENT_TO_HOST, HOST_TO_AGENT),
    };

    Ok(Session {
        seal_key: ChaCha20Poly1305::new((&derive(seal_info)?).into()),
        open_key: ChaCha20Poly1305::new((&derive(open_info)?).into()),
        sent: 0,
        received: 0,
    })
}

/// The salt both ends can compute and neither has to send.
///
/// The initiator's public key then the responder's, **in that order regardless of
/// which end is doing the computing**. Order matters and is the kind of thing that
/// silently halves a system: if each side used its own key first, the two would
/// derive different keys and nothing would open -- which is loud -- but a version
/// that happened to agree on one direction and not the other would be the silent
/// kind.
fn handshake_salt(initiator: &EphemeralPublic, responder: &EphemeralPublic) -> Vec<u8> {
    let mut salt = Vec::with_capacity(EphemeralPublic::BYTES * 2);
    salt.extend_from_slice(initiator.as_bytes());
    salt.extend_from_slice(responder.as_bytes());
    salt
}

/// The only implementation of [`Channel`] and [`Handshake`] in this project.
#[derive(Debug, Clone, Copy, Default)]
pub struct HkdfChannel;

impl Handshake for HkdfChannel {
    fn propose(&self, secret: &[u8]) -> Result<(EphemeralPublic, Box<dyn Pending>), ChannelError> {
        let (private, public) = ephemeral_pair()?;

        let pending = Proposer {
            private,
            public: public.clone(),
            secret: secret.to_vec(),
        };

        Ok((public, Box::new(pending)))
    }

    fn accept(
        &self,
        secret: &[u8],
        peer: &EphemeralPublic,
    ) -> Result<(EphemeralPublic, Box<dyn Sealed>), ChannelError> {
        let (private, public) = ephemeral_pair()?;

        // The initiator's key first, because `Host` is the initiator. The peer's
        // is `peer`, ours is `public`.
        let salt = handshake_salt(peer, &public);
        let shared = exchange(&private, peer)?;

        // The X25519 result *and* the shared secret. The first is what gives
        // forward secrecy; the second is what makes the session one an attacker in
        // the middle cannot produce, because they do not have it.
        let mut material = Vec::with_capacity(shared.len() + secret.len());
        material.extend_from_slice(&shared);
        material.extend_from_slice(secret);

        let session = session_from(&salt, &material, Role::Agent)?;
        Ok((public, Box::new(session)))
    }
}

/// The initiator's half, held between the two messages.
struct Proposer {
    private: StaticSecret,
    public: EphemeralPublic,
    secret: Vec<u8>,
}

impl Pending for Proposer {
    fn finish(self: Box<Self>, peer: &EphemeralPublic) -> Result<Box<dyn Sealed>, ChannelError> {
        // Ours first, because `Host` is the initiator.
        let salt = handshake_salt(&self.public, peer);
        let shared = exchange(&self.private, peer)?;

        let mut material = Vec::with_capacity(shared.len() + self.secret.len());
        material.extend_from_slice(&shared);
        material.extend_from_slice(&self.secret);

        Ok(Box::new(session_from(&salt, &material, Role::Host)?))
    }
}

/// A fresh X25519 key pair, with the private half owned by the caller.
///
/// `StaticSecret` rather than `EphemeralSecret` because this one has to survive
/// across the handshake's two messages -- the initiator cannot finish until the
/// reply arrives. "Ephemeral" here means **discarded when the handshake is done**,
/// which is what forward secrecy needs, and it is a property of how it is used
/// rather than of the type. The value is dropped with the `Proposer`, and nothing
/// writes it anywhere.
fn ephemeral_pair() -> Result<(StaticSecret, EphemeralPublic), ChannelError> {
    let private = StaticSecret::random();
    let public = EphemeralPublic::from_bytes(PublicKey::from(&private).as_bytes().to_vec())?;
    Ok((private, public))
}

/// The X25519 result, with the peer's key as a curve point.
fn exchange(private: &StaticSecret, peer: &EphemeralPublic) -> Result<[u8; 32], ChannelError> {
    let mut bytes = [0u8; EphemeralPublic::BYTES];
    bytes.copy_from_slice(peer.as_bytes());
    let peer = PublicKey::from(bytes);

    // The low-order-point check. X25519 with a peer key that is not a real public
    // key can produce an all-zero result, and a session key derived from a value an
    // attacker can force to zero is a session key the attacker knows. Refusing here
    // is cheaper than reasoning about it later.
    let shared = private.diffie_hellman(&peer);
    if shared.as_bytes().iter().all(|byte| *byte == 0) {
        return Err(ChannelError::NotAuthentic);
    }
    Ok(*shared.as_bytes())
}

impl Channel for HkdfChannel {
    fn open_session(
        &self,
        secret: &[u8],
        session: &SessionId,
        role: Role,
    ) -> Result<Box<dyn Sealed>, ChannelError> {
        // The session identifier is the salt, and the shared secret is the only
        // input. No exchange, and therefore no forward secrecy -- see the trait's
        // documentation for which path a deployment wants.
        Ok(Box::new(session_from(session.as_bytes(), secret, role)?))
    }
}

/// A session identifier from the operating system's random source.
///
/// # Errors
///
/// [`ChannelError::Refused`] when the system cannot produce randomness. Failing
/// loudly is the only safe response: a session identifier that was not random
/// would repeat, and a repeated salt under a reused secret is a repeated key.
pub fn new_session_id() -> Result<SessionId, ChannelError> {
    let mut bytes = [0u8; SessionId::BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|e| ChannelError::Refused(format!("no randomness available: {e}")))?;
    SessionId::from_bytes(bytes.to_vec())
}

/// One session: a key for each direction and a count for each.
struct Session {
    seal_key: ChaCha20Poly1305,
    open_key: ChaCha20Poly1305,
    sent: u64,
    received: u64,
}

impl Session {
    /// The nonce for a message number.
    ///
    /// Four zero bytes then the counter big-endian. ChaCha20-Poly1305 takes a
    /// 96-bit nonce and makes no promise about its structure, so any injective
    /// mapping from counter to nonce would do; this one is a counter in the low 64
    /// bits because that is the part that is easy to read in a debugger.
    fn nonce(count: u64) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&count.to_be_bytes());
        Nonce::from(nonce)
    }
}

impl Sealed for Session {
    fn seal_into(&mut self, plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), ChannelError> {
        let nonce = Self::nonce(self.sent);
        // The counter is authenticated but not transmitted: both sides know it, so
        // sending it would only give an attacker something to change. Putting it in
        // the associated data is what makes a relabelled message fail to open.
        let associated = self.sent.to_be_bytes();

        // In place, which is the point of this method. The plaintext is copied into
        // the caller's buffer and the tag is appended to it, so a chunk of a file is
        // in memory once rather than three times -- and the caller can hand the same
        // buffer back for the next chunk without the allocator being involved.
        out.clear();
        out.extend_from_slice(plaintext);
        self.seal_key
            .encrypt_in_place(&nonce, &associated, out)
            .map_err(|_| ChannelError::Refused("the cipher refused to seal".to_string()))?;

        // Advanced only after the seal succeeded: a failed seal that had already
        // consumed a nonce would leave the two sides counting differently, and every
        // later message would fail to open with no indication of why.
        self.sent += 1;
        Ok(())
    }

    fn open_into(&mut self, ciphertext: &[u8], out: &mut Vec<u8>) -> Result<(), ChannelError> {
        // Refused before the cipher sees it, so that a message which is one byte
        // long reports the authentication problem rather than being handed to a
        // cipher that would report the same thing less clearly.
        if ciphertext.len() < TAG_BYTES {
            return Err(ChannelError::NotAuthentic);
        }

        let nonce = Self::nonce(self.received);
        let associated = self.received.to_be_bytes();

        out.clear();
        out.extend_from_slice(ciphertext);
        self.open_key
            .decrypt_in_place(&nonce, &associated, out)
            // One error for every way this can fail, because the ways are
            // indistinguishable to a caller and telling them apart would tell an
            // attacker which part of a guess was closer.
            .map_err(|_| ChannelError::NotAuthentic)?;

        // `decrypt_in_place` truncates the buffer to the plaintext on success, so
        // `out` holds the message and not the message plus a tag. That is a
        // property of the library rather than of this code, which is the right
        // place for it to live.
        self.received += 1;
        Ok(())
    }
}

/// The length of the tag this channel appends, for a caller sizing a reply.
pub const OVERHEAD_BYTES: usize = TAG_BYTES;
