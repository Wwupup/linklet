//! The shared secret that decides who may run commands on a target.
//!
//! # The shape of the scheme, and what it is not
//!
//! **The token is never transmitted.** It is mixed into the key derivation of a
//! sealed session, so what crosses the wire is public keys and ciphertext, and a
//! captured request is not a credential. That is the part worth being precise about,
//! because it is the part a reader is most likely to assume wrongly in either
//! direction.
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
//! # Why the comparison is written by hand
//!
//! [`token_matches`] does not use `==`. Comparing two strings with `==` in Rust
//! stops at the first differing byte, so the time it takes depends on how many
//! leading bytes were right. An attacker who can time the reply can therefore
//! recover a secret one byte at a time: a wrong first byte returns in one step, a
//! right first byte takes two, and the difference is measurable over enough
//! attempts.
//!
//! That is not a theoretical concern for a LAN tool with a reply path of a few
//! hundred microseconds. It is also the kind of bug that never shows up in a
//! functional test, which is why the test for it measures something.

/// The smallest token this will accept, in bytes.
///
/// A short token is not a weak secret because of the length of the string; it is
/// weak because whoever chose it did not think about the question. Sixteen bytes
/// is the smallest thing that looks like it was chosen on purpose, and the agent
/// refuses anything shorter at startup rather than at the first connection --
/// a configuration error should be loud when it is made.
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
    /// [`TokenError`] when the value could not have been chosen deliberately.
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

    /// The value, for putting in a header.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Whether a presented token is the expected one, in time that does not depend on
/// how much of it was right.
///
/// The loop runs over every byte of both inputs regardless of where they differ,
/// and accumulates the difference instead of returning early. The `u8` accumulator
/// cannot overflow into a false match: it is a bitwise OR of XOR results, so it is
/// zero exactly when every byte matched.
///
/// The length check does leak the expected length, which is not a secret worth
/// protecting -- the token's length is visible from any single request if the
/// attacker can see one, and hiding it would mean comparing against a padded
/// buffer, which trades a real subtlety for a theoretical one.
pub fn token_matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();

    if expected.len() != presented.len() {
        return false;
    }

    let mut difference: u8 = 0;
    for (a, b) in expected.iter().zip(presented.iter()) {
        difference |= a ^ b;
    }
    difference == 0
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
