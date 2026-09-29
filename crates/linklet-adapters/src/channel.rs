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
//! # What is still not here
//!
//! No handshake, so no forward secrecy: a session recorded today can be read by
//! anyone who learns the secret later. That is stated in
//! `linklet_core::channel`'s own documentation as well, because a reader who
//! believes otherwise is worse off than one who knows.

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;

use linklet_core::channel::{Channel, ChannelError, Role, Sealed, SessionId};

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

/// The only implementation of [`Channel`] in this project.
#[derive(Debug, Clone, Copy, Default)]
pub struct HkdfChannel;

impl Channel for HkdfChannel {
    fn open_session(
        &self,
        secret: &[u8],
        session: &SessionId,
        role: Role,
    ) -> Result<Box<dyn Sealed>, ChannelError> {
        // `None` for the salt would make HKDF use a zero salt, and the salt is the
        // session identifier -- passing it is the whole point.
        let hkdf = Hkdf::<Sha256>::new(Some(session.as_bytes()), secret);

        // One key per direction. A failure here means the output length is not a
        // valid HKDF length, which for 32 bytes cannot happen; it is handled rather
        // than unwrapped because a panic in a library is a bug report from a user.
        let derive = |info: &[u8]| -> Result<[u8; KEY_BYTES], ChannelError> {
            let mut key = [0u8; KEY_BYTES];
            hkdf.expand(info, &mut key)
                .map_err(|e| ChannelError::Refused(format!("key derivation: {e}")))?;
            Ok(key)
        };

        // Mirrored, not identical. This is the line the round-trip test caught:
        // assigning the same pair at both ends gave the host one key to seal with
        // and the agent a different one to open with, and every message failed.
        let (seal_info, open_info) = match role {
            Role::Host => (HOST_TO_AGENT, AGENT_TO_HOST),
            Role::Agent => (AGENT_TO_HOST, HOST_TO_AGENT),
        };

        Ok(Box::new(Session {
            seal_key: ChaCha20Poly1305::new((&derive(seal_info)?).into()),
            open_key: ChaCha20Poly1305::new((&derive(open_info)?).into()),
            sent: 0,
            received: 0,
        }))
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
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let nonce = Self::nonce(self.sent);
        // The counter is authenticated but not transmitted: both sides know it, so
        // sending it would only give an attacker something to change. Putting it in
        // the associated data is what makes a relabelled message fail to open.
        let associated = self.sent.to_be_bytes();

        let ciphertext = self
            .seal_key
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: &associated,
                },
            )
            .map_err(|_| ChannelError::Refused("the cipher refused to seal".to_string()))?;

        // Advanced only after the seal succeeded: a failed seal that had already
        // consumed a nonce would leave the two sides counting differently, and every
        // later message would fail to open with no indication of why.
        self.sent += 1;
        Ok(ciphertext)
    }

    fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        // Refused before the cipher sees it, so that a message which is one byte
        // long reports the ordering problem rather than an authentication failure
        // that would send a reader looking for the wrong thing.
        if ciphertext.len() < TAG_BYTES {
            return Err(ChannelError::NotAuthentic);
        }

        let nonce = Self::nonce(self.received);
        let associated = self.received.to_be_bytes();

        let plaintext = self
            .open_key
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad: &associated,
                },
            )
            // One error for every way this can fail, because the ways are
            // indistinguishable to a caller and telling them apart would tell an
            // attacker which part of a guess was closer.
            .map_err(|_| ChannelError::NotAuthentic)?;

        self.received += 1;
        Ok(plaintext)
    }
}

/// The length of the tag this channel appends, for a caller sizing a reply.
pub const OVERHEAD_BYTES: usize = TAG_BYTES;
