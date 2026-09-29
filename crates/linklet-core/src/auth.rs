//! The shared secret that decides who may run commands on a target.
//!
//! # The shape of the scheme, and what it is not
//!
//! A bearer token in a header. Whoever can read one request can replay it, and
//! whoever can read the network can read the token -- because this is plaintext
//! HTTP. **That is a real limit and it is written down rather than implied**: the
//! token stops a program that can reach the port but does not know the secret, and
//! it stops nothing else. Encryption belongs next and is a separate change.
//!
//! Being honest about that matters more than the scheme. A tool whose author
//! believes a token is encryption will be used on a network where the difference
//! decides whether someone else can run commands on the target, and the belief is
//! the vulnerability.
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

/// The header the token travels in.
///
/// `Authorization` rather than a made-up name, because every proxy, log scrubber
/// and reader already treats that header as a secret and handles it accordingly.
/// A custom name would be a secret in a place nothing knows to look.
pub const TOKEN_HEADER: &str = "authorization";

/// The scheme prefix, so the header reads as a standard bearer credential.
pub const TOKEN_SCHEME: &str = "Bearer ";

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

/// Pulls a token out of an `Authorization` header value.
///
/// Accepts `Bearer <token>` and a bare token. The bare form is accepted because
/// the first version of anything is typed by hand at a prompt, and refusing it
/// would make the tool harder to try than to use -- but the prefixed form is what
/// the client sends, because a header that reads as the standard one gets treated
/// as a secret by everything in the path.
pub fn token_from_header(value: &str) -> Option<&str> {
    let value = value.trim();
    let token = value.strip_prefix(TOKEN_SCHEME).unwrap_or(value).trim();
    if token.is_empty() { None } else { Some(token) }
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

/// Renders a 401 body.
///
/// Deliberately says nothing about *why*. Whether the token was absent, wrong, or
/// too short is information a caller who has the token does not need, and one who
/// does not have it should not be given.
pub fn unauthorized_body() -> linklet_core_json::Json {
    linklet_core_json::Json::Object(
        [(
            "error".to_string(),
            linklet_core_json::Json::str("the token is missing or wrong"),
        )]
        .into_iter()
        .collect(),
    )
}

/// The status an agent answers when the token does not match.
pub const UNAUTHORIZED: u16 = 401;

/// A private alias so this module reads without a long path at every use.
use crate::json as linklet_core_json;
