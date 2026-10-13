//! The shared secret that decides who may run commands on a target.
//!
//! # The shape of the scheme, and what it is not
//!
//! Pairing generates and delivers the token inside an authenticated encrypted
//! channel. Command connections mix it into session key derivation; only public
//! keys and ciphertext cross the wire, so a captured request is not a credential.
//!
//! What it does not do:
//!
//! - **It says nothing about *which* caller it is.** There is one secret and no
//!   identity behind it, so there is no per-caller revocation and no audit trail.
//! - **It does not stop an attacker who can reach the port from making the agent do
//!   work.** A handshake is accepted before any token is checked -- it has to be,
//!   because the check happens *inside* the derived session -- so a stranger can
//!   occupy a thread for the length of one handshake. See `docs/framing.md` on what
//!   is not defended.
//!
//! This documentation described a bearer token in a plaintext HTTP header until M7
//! replaced HTTP with frames. It had been wrong since M6, which is what a limit
//! written down in one place and changed in another looks like.
//!
/// The smallest token this will accept, in bytes.
///
/// A structural check for malformed saved credentials, not an entropy estimate.
/// Managed pairing generates an independent 256-bit secret encoded as 64 hex
/// characters. The six-digit pairing code is never a command token.
pub const MIN_TOKEN_BYTES: usize = 16;

/// Why a token is not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// Shorter than [`MIN_TOKEN_BYTES`].
    TooShort {
        /// How long it was.
        bytes: usize,
    },
    /// Empty or all whitespace.
    Empty,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { bytes } => write!(
                f,
                "the token is {bytes} bytes; at least {MIN_TOKEN_BYTES} are required"
            ),
            Self::Empty => write!(f, "the token is empty"),
        }
    }
}

impl std::error::Error for TokenError {}

/// A secret that has been checked to be usable.
///
/// The check is length and nothing else: this cannot tell a good secret from a
/// bad one, and pretending otherwise would give false confidence. What it can do
/// is refuse the two cases that are certainly mistakes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Checks a candidate secret.
    ///
    /// # Errors
    ///
    /// [`TokenError`] when the value is empty or shorter than [`MIN_TOKEN_BYTES`].
    pub fn new(secret: impl Into<String>) -> Result<Self, TokenError> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(TokenError::Empty);
        }
        if secret.len() < MIN_TOKEN_BYTES {
            return Err(TokenError::TooShort {
                bytes: secret.len(),
            });
        }
        Ok(Self(secret))
    }

    /// The value, for deriving an authenticated session; never sent on the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The reason an agent gives when a caller's session did not hold a usable token.
///
/// Deliberately says nothing about *why*. Whether the token was absent, wrong, or
/// too short is information a caller who has the token does not need, and one who
/// does not have it should not be given. One sentence for one problem, and the same
/// sentence for every version of it.
pub fn unauthorized_reason() -> &'static str {
    "the token is missing or wrong"
}
