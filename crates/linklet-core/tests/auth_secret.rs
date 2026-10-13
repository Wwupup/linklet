//! Validation and loading of shared secrets, and the public refusal message.

use linklet_core::auth::{MIN_TOKEN_BYTES, Token, TokenError, unauthorized_reason};

// --- what a token may be -----------------------------------------------------

#[test]
fn a_token_has_to_look_like_someone_chose_it() {
    // Not "is it a good secret" -- this cannot tell. It refuses the two cases
    // that are certainly mistakes, and it does so at startup rather than at the
    // first connection, because a configuration error should be loud when it is
    // made.
    assert!(Token::new("a".repeat(MIN_TOKEN_BYTES)).is_ok());
    assert!(Token::new("a longer secret than the minimum").is_ok());

    assert_eq!(Token::new(""), Err(TokenError::Empty));
    assert_eq!(Token::new("   "), Err(TokenError::Empty));
    assert_eq!(Token::new("short"), Err(TokenError::TooShort { bytes: 5 }));
}

#[test]
fn the_short_token_error_says_how_short_and_what_is_required() {
    let error = Token::new("short").expect_err("five bytes is too few");
    let text = error.to_string();
    assert!(text.contains('5'), "{text}");
    assert!(text.contains(&MIN_TOKEN_BYTES.to_string()), "{text}");
}

#[test]
fn a_token_exposes_its_value_and_nothing_else() {
    // `expose` rather than `Display`: a secret that formats itself is a secret
    // that ends up in a log the first time somebody writes `{:?}` on a struct
    // holding one. The debug impl on `Token` prints the value because tests need
    // it; the deliberate name is what a reader sees at every call site.
    let token = Token::new("0123456789abcdef").expect("sixteen bytes");
    assert_eq!(token.expose(), "0123456789abcdef");
}

// --- what a refusal says -----------------------------------------------------

#[test]
fn the_refusal_does_not_say_why() {
    // Whether the token was absent, wrong or too short is information a caller
    // who has the token does not need, and one who does not have it should not be
    // given. One sentence, and the same sentence for every version of the problem.
    let reason = unauthorized_reason();
    assert!(reason.contains("missing or wrong"), "{reason}");
    for hint in ["absent", "empty", "length", "expired", "16", "bytes"] {
        assert!(
            !reason.to_lowercase().contains(hint),
            "the refusal hints at {hint:?}: {reason}"
        );
    }
}
