//! Validation and loading of shared secrets, and the public refusal message.

use linklet_core::auth::{MIN_TOKEN_BYTES, Token, TokenError, secret_in_file, unauthorized_reason};

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

// --- where a secret comes from, when it is a file ----------------------------

#[test]
fn what_a_windows_editor_adds_to_a_token_file_is_not_part_of_the_secret() {
    // Two things a file acquires without anyone deciding to add them, and both
    // are invisible in every editor that shows the file as one line of text.
    //
    // `Set-Content -Encoding utf8` under Windows PowerShell 5.1 writes a
    // byte-order mark, which is what `smoke.md` tells an operator to use; and
    // every editor ends the last line. What makes these worth a test is that
    // neither fails where it is made. A secret one byte too long derives a
    // different key on this side and the symptom is the agent saying the token is
    // missing or wrong -- which is true about the session and false about the
    // file, and sends the reader to the wrong machine.
    //
    // Compared through `Token::new` rather than as strings: the claim is about
    // the secret the two ends end up deriving, not about the bytes of the file.
    let typed = Token::new("0123456789abcdef").expect("sixteen bytes");

    for contents in [
        "0123456789abcdef\r\n",                // a CRLF file
        "0123456789abcdef\n",                  // and an LF one
        "\u{feff}0123456789abcdef\r\n",        // a BOM from PowerShell 5.1
        "\u{feff}0123456789abcdef",            // a BOM with no line ending
        "0123456789abcdef\r\nsecond line\r\n", // anything after the first line
        "\u{feff}0123456789abcdef\r\nsecond line\r\n",
    ] {
        assert_eq!(
            Token::new(secret_in_file(contents)).expect("the file names a usable secret"),
            typed,
            "{contents:?} should yield the secret that was typed"
        );
    }
}

#[test]
fn a_space_in_a_token_file_is_part_of_the_secret() {
    // No trimming beyond the line ending. A rule that trimmed would accept two
    // files that hold different secrets, and the difference would surface as the
    // same refusal this whole section exists to keep out of the way.
    assert_eq!(secret_in_file(" 0123456789abcdef"), " 0123456789abcdef");
    assert_eq!(secret_in_file("0123456789abcdef "), "0123456789abcdef ");
    assert_eq!(secret_in_file("\t0123456789abcdef"), "\t0123456789abcdef");
}

#[test]
fn an_empty_token_file_yields_nothing_the_length_rule_will_accept() {
    // The file side does not decide what a usable secret is -- `Token::new` does,
    // and this is the check that it is still the only place that decides.
    for contents in ["", "\r\n", "\n", "\u{feff}", "\u{feff}\r\n", "   \r\n"] {
        let candidate = secret_in_file(contents);
        assert!(
            Token::new(candidate).is_err(),
            "{contents:?} yielded {candidate:?}, which the length rule accepted"
        );
    }
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
