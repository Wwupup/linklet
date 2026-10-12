//! Contracts for authenticated, forward-secret sessions.
//!
//! Adapters generate ephemeral keys and implement these contracts with vetted
//! cryptographic libraries. The core carries protocol values and performs no I/O.
//! Each session accepts messages once, in order; dropped messages can still stop
//! progress. Secret provisioning belongs to [`crate::auth`].

/// Why a message could not be opened.
///
/// Deliberately carries nothing about *which part* went wrong. A channel that said
/// whether the tag was bad, the sequence number was old, or the key was wrong would
/// be telling an attacker which part of their guess was closer, and a caller can do
/// nothing different for any of the three.
///
/// `Clone` but not `Copy`, because [`ChannelError::Refused`] carries an adapter's
/// own words and a `String` cannot be copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    /// The message was not produced by the other side, or was altered.
    NotAuthentic,
    /// A public key was the wrong length.
    BadPublicKey {
        /// How long it was.
        bytes: usize,
    },
    /// The implementation refused, for a reason it can name.
    ///
    /// A separate variant so that an adapter's own failure -- a random source
    /// that will not open, say -- is not reported as a forged message. Those
    /// lead to different actions and one of them is alarming.
    Refused(String),
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The wording is the caller's whole view of the failure, and it is
            // one sentence because there is nothing more to say that would not
            // help an attacker.
            Self::NotAuthentic => write!(f, "the message is not authentic"),
            Self::BadPublicKey { bytes } => write!(
                f,
                "a public key is {bytes} bytes; {} are required",
                EphemeralPublic::BYTES
            ),
            Self::Refused(why) => write!(f, "the channel refused: {why}"),
        }
    }
}

impl std::error::Error for ChannelError {}

/// One direction of a sealed conversation, as the core needs to see it.
///
/// `seal_into` produces bytes for exactly one message and `open_into` accepts
/// exactly one, in order. That is a narrower contract than "encrypt this" and it is
/// deliberate:
/// a channel that accepted messages in any order would need a window and a
/// replayed-message policy, and both are decisions that belong to a caller who
/// knows what it is protecting.
///
/// # Why this requires `Send`
///
/// **A conversation has two ends and they run at the same time.** A transfer larger than a
/// socket buffer cannot be written and then read -- the kernel fills, the sender blocks, and the
/// reader has not started -- so the real client writes while the real agent reads, on different
/// threads. A session that could not cross a thread boundary would make that impossible, and a
/// trait that left it out would be describing something narrower than what the implementations
/// already are: [`crate::channel::Sealed`] has one implementor, and it is two keys and two
/// counters.
///
/// It was found by a test that did the impossible thing -- sending 3 MiB before reading any of
/// it -- and passing, because the socket buffers were large enough. The same test failed on a
/// runner where they were not, with a thirty-second timeout.
pub trait Sealed: Send {
    /// Seals one message into `out`, in order. `out` is cleared first.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Refused`] when the implementation cannot produce a message
    /// at all. It must not be used for "the other side sent something bad" -- that
    /// is [`Sealed::open_into`]'s business.
    fn seal_into(&mut self, plaintext: &[u8], out: &mut Vec<u8>) -> Result<(), ChannelError>;

    /// Opens the next message into `out`, in order. `out` is cleared first.
    ///
    /// # Errors
    ///
    /// [`ChannelError::NotAuthentic`] for bytes that were not produced by the other
    /// side of this session, including replayed or out-of-order messages.
    fn open_into(&mut self, ciphertext: &[u8], out: &mut Vec<u8>) -> Result<(), ChannelError>;

    /// Seals one message and returns it.
    ///
    /// The convenience form, for a caller whose messages are small enough that a
    /// second buffer is not a decision worth making.
    ///
    /// # Errors
    ///
    /// As [`Sealed::seal_into`].
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let mut out = Vec::new();
        self.seal_into(plaintext, &mut out)?;
        Ok(out)
    }

    /// Opens the next message and returns it.
    ///
    /// # Errors
    ///
    /// As [`Sealed::open_into`].
    fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let mut out = Vec::new();
        self.open_into(ciphertext, &mut out)?;
        Ok(out)
    }
}

/// A public key for one handshake, as bytes.
///
/// The core carries it and does not compute with it. What it is -- a curve point,
/// a different length, something else entirely -- is the implementation's
/// business, and a core that knew would be a core that had started implementing
/// the cryptography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EphemeralPublic(Vec<u8>);

impl EphemeralPublic {
    /// How many bytes a public key is.
    ///
    /// Thirty-two, which is X25519's size. Declared here rather than in the
    /// adapter because the *protocol* fixes it: a message carrying a key of
    /// another length cannot be parsed by the other side, so this is a fact about
    /// the wire and not about the curve.
    pub const BYTES: usize = 32;

    /// Wraps bytes that arrived from the other side.
    ///
    /// # Errors
    ///
    /// [`ChannelError::BadPublicKey`] when the length is wrong, which is the only
    /// check the core can make. Whether the bytes are a *valid* public key is a
    /// question for the curve, and an implementation that rejected them here would
    /// be a core that had learned arithmetic.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, ChannelError> {
        let bytes = bytes.into();
        if bytes.len() != Self::BYTES {
            return Err(ChannelError::BadPublicKey { bytes: bytes.len() });
        }
        Ok(Self(bytes))
    }

    /// The bytes, for putting in a header.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The initiator's half of a handshake, waiting for the reply.
///
/// A separate type because the initiator holds a private key it must not use until
/// the peer's public key arrives, and there is no way to express "a session that
/// is not ready" with a single [`Sealed`]. Consuming `self` in
/// [`Pending::finish`] is what stops it being finished twice -- two sessions from
/// one ephemeral key would share a nonce sequence, which is the failure the whole
/// module is arranged to avoid.
pub trait Pending {
    /// Derives the session, given the responder's public key.
    ///
    /// Takes `self` by value: a `Pending` that could be finished twice is a
    /// `Pending` that can produce two sessions from one ephemeral key.
    ///
    /// # Errors
    ///
    /// [`ChannelError::NotAuthentic`] when the peer's key does not produce a
    /// session the shared secret agrees with, and [`ChannelError::Refused`] when
    /// the implementation cannot proceed at all.
    fn finish(self: Box<Self>, peer: &EphemeralPublic) -> Result<Box<dyn Sealed>, ChannelError>;
}

/// Runs a handshake, so that a session recorded today cannot be read by anyone who
/// learns the secret tomorrow.
///
/// # Where forward secrecy comes from
///
/// Both sides generate a key pair for this handshake and **discard the private
/// half when it is done**. The session key is the X25519 result of the initiator's
/// private key against the responder's public key -- the same value the responder
/// computes from its private key and the initiator's public key -- mixed with the
/// shared secret by a key derivation.
///
/// An attacker who records the whole exchange and later learns the shared secret
/// still needs the ephemeral private keys to recompute that result, and those no
/// longer exist in the handshake state. Every session uses this exchange.
///
/// # What the shared secret is still for
///
/// **Authentication.** Without it the exchange is anonymous, and anyone in the
/// middle can complete a handshake with both sides and read everything. Mixing it
/// into the key derivation is what makes the session key depend on a value only
/// the two real ends have, so an attacker who substitutes their own public key
/// produces a session that neither end can open. There is no separate signature
/// because there is nothing to sign with -- the secret *is* the credential.
pub trait Handshake {
    /// Starts a handshake: our public key, and the state to finish with.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Refused`] when the implementation cannot produce a key pair,
    /// which in practice means the system has no randomness.
    fn propose(&self, secret: &[u8]) -> Result<(EphemeralPublic, Box<dyn Pending>), ChannelError>;

    /// Answers a handshake: our public key, and the session, ready to use.
    ///
    /// The responder can finish in one call because it has both public keys --
    /// its own, which it just made, and the initiator's, which it was given. The
    /// initiator cannot, which is why [`Pending`] exists.
    ///
    /// # Errors
    ///
    /// [`ChannelError::NotAuthentic`] when the peer's public key does not produce a
    /// session the shared secret agrees with, and [`ChannelError::Refused`] when
    /// the implementation cannot proceed.
    fn accept(
        &self,
        secret: &[u8],
        peer: &EphemeralPublic,
    ) -> Result<(EphemeralPublic, Box<dyn Sealed>), ChannelError>;
}
