//! What it means to talk to an agent without the network reading it.
//!
//! # The shape, and why it is a trait rather than an implementation
//!
//! This module says **what** a channel is: something that turns a message into
//! bytes nobody else can read and back again, given a shared secret. It does not
//! say how, and it must not -- implementing cryptography is the one thing this
//! project should never do by hand, and the way to make that impossible is to put
//! the trait here and every implementation somewhere else.
//!
//! That is the same inversion as [`crate::Probe`] and [`crate::testbed::Prober`],
//! applied to the part where hand-rolling is most tempting and most damaging. The
//! core keeps its promise of no dependencies; `linklet-adapters` carries the
//! vetted crate that does the arithmetic.
//!
//! # Why a hand-written implementation was tried and thrown away
//!
//! For completeness, because the reasoning is the useful part: a SHA-256 was
//! written here to avoid a dependency while the crate registry looked
//! unreachable. Padding was wrong three times, the published vectors caught it,
//! and one observation was never explained. The registry was reachable all along
//! -- a stale proxy address in a git config was the whole problem -- and the
//! lesson is not "be more careful". It is that **a hash is a thing with published
//! test vectors and still failed repeatedly**, which is a fair estimate of what
//! would happen to a cipher with none.
//!
//! # What this is not, still
//!
//! - **Forward secrecy is available, and opt-in.** [`Handshake`] runs an X25519
//!   exchange whose private halves are discarded, so a session recorded today
//!   cannot be read by anyone who learns the secret tomorrow. [`Channel`] alone
//!   does **not** have that property: it derives the keys from the shared secret
//!   and nothing else, so it is the wrong path for a deployment and the right one
//!   for a test. Both are here, and which is which is stated on each trait rather
//!   than left to be inferred from the names.
//! - **No protection against replay within a session.** A captured message can be
//!   sent again and will open. The sequence numbers stop a *reordering* from being
//!   accepted; they do not stop a repeat of the exact bytes from being replayed at
//!   the point they were captured. A caller that cares has to carry its own
//!   request identity, and the protocol does not yet.
//! - **Nothing here decides how the secret is chosen or shared.** That is
//!   [`crate::auth`], and its own documentation says the token travels in
//!   cleartext until this module is used -- which is the point of using it.

/// A random value that identifies one session and seeds its keys.
///
/// Opaque to the core: it is bytes to be carried in a header and fed to a key
/// derivation, and giving it methods would invite the core to reason about
/// randomness it cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(Vec<u8>);

impl SessionId {
    /// How many bytes a session identifier carries.
    ///
    /// Thirty-two, which is what a key derivation wants as a salt and is far more
    /// than enough to make a repeat impossible in practice.
    pub const BYTES: usize = 32;

    /// Wraps bytes that came from somewhere else.
    ///
    /// # Errors
    ///
    /// When the length is not [`SessionId::BYTES`]. A short identifier is not a
    /// weak secret, it is a sign that whatever produced it is not producing what
    /// this expects -- and a truncated salt is how two sessions end up with the
    /// same keys.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self, ChannelError> {
        let bytes = bytes.into();
        if bytes.len() != Self::BYTES {
            return Err(ChannelError::BadSessionId { bytes: bytes.len() });
        }
        Ok(Self(bytes))
    }

    /// The bytes, for putting in a header.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

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
    /// The message belongs to a different point in the session.
    OutOfOrder,
    /// A session identifier was the wrong length.
    BadSessionId {
        /// How long it was.
        bytes: usize,
    },
    /// A public key was the wrong length.
    ///
    /// Separate from [`ChannelError::BadSessionId`] because the two arrive from
    /// different headers and a reader fixing one should not be sent to the other.
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
            Self::OutOfOrder => write!(f, "the message is not the next one"),
            Self::BadSessionId { bytes } => write!(
                f,
                "a session identifier is {bytes} bytes; {} are required",
                SessionId::BYTES
            ),
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

/// Which end of a session is being built.
///
/// The parameter that has to exist, and the bug that proved it: the first version
/// derived both directional keys and assigned them the same way at both ends, so
/// the host sealed with one key and the agent tried to open with the other. Every
/// round trip failed.
///
/// That failure was the *safe* direction of the mistake, which is luck rather than
/// design. Had the derivation produced one key and both ends used it for
/// everything, the round trips would all have passed -- and the result would have
/// been a single key with two independent nonce sequences under it, where the first
/// repeat destroys confidentiality and forgery becomes possible. **A test that
/// passes is not evidence that the keys are separate**, so the role is explicit and
/// the two keys are distinct by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The side that opens the conversation.
    Host,
    /// The side that answers.
    Agent,
}

/// One direction of a sealed conversation, as the core needs to see it.
///
/// `seal` produces bytes for exactly one message and `open` accepts exactly one,
/// in order. That is a narrower contract than "encrypt this" and it is deliberate:
/// a channel that accepted messages in any order would need a window and a
/// replayed-message policy, and both are decisions that belong to a caller who
/// knows what it is protecting.
pub trait Sealed {
    /// Seals one message, in order.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Refused`] when the implementation cannot produce a message
    /// at all. It must not be used for "the other side sent something bad" --
    /// that is [`Sealed::open`]'s business.
    fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError>;

    /// Opens the next message, in order.
    ///
    /// # Errors
    ///
    /// [`ChannelError::NotAuthentic`] for bytes that were not produced by the
    /// other side of this session, and [`ChannelError::OutOfOrder`] for a message
    /// that is authentic but is not the one expected next.
    fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, ChannelError>;
}

/// Turns a shared secret and a session identifier into a sealed conversation.
///
/// A trait with one method rather than a free function, because the thing that
/// implements it is the crate that owns the cryptography, and the core's job is to
/// say what it needs. `linklet-adapters` has the only real implementation; tests
/// have a fake that counts calls, which is why the framing and sequencing above
/// can be tested without a cipher.
pub trait Channel {
    /// A conversation keyed by `session`, for the end named by `role`.
    ///
    /// This is the **shared-secret-only** path: two sides that already hold the
    /// same secret derive the same keys with no exchange at all. It is kept
    /// because it is the simplest thing that seals a message and because the tests
    /// above drive it, but it has no forward secrecy -- see [`Handshake`] for the
    /// path a deployment should use.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Refused`] when the implementation cannot set up a session,
    /// and [`ChannelError::NotAuthentic`] when it can but will not.
    fn open_session(
        &self,
        secret: &[u8],
        session: &SessionId,
        role: Role,
    ) -> Result<Box<dyn Sealed>, ChannelError>;
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
/// longer exist anywhere. That is the entire benefit, and it is worth the round
/// trip exactly once: an operator who does not need it can use
/// [`Channel::open_session`] and skip the exchange.
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
