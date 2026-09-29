//! The specification for the shared secret.
//!
//! Two kinds of test here, and the second kind is the interesting one.
//!
//! Most of this file checks what [`token_matches`] returns, which any comparison
//! would pass. The test that matters checks **how long it takes**, because the
//! property being protected is not "does it return the right answer" but "does the
//! time it takes reveal how much of the secret was right". A comparison written
//! with `==` passes every functional test in this file and is the vulnerability.
//!
//! A timing test is unusual and worth justifying: it measures a real property, it
//! has been written to fail loudly rather than flakily, and the thing it protects
//! cannot be checked any other way. What it cannot do is prove the code is
//! constant-time -- it can only fail to catch a gross leak. That is stated here
//! rather than implied.

use std::time::Instant;

use linklet_core::auth::{
    MIN_TOKEN_BYTES, TOKEN_HEADER, TOKEN_SCHEME, Token, TokenError, token_from_header,
    token_matches, unauthorized_body,
};

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

// --- reading the header ------------------------------------------------------

#[test]
fn the_header_may_carry_the_scheme_or_not() {
    // The bare form is accepted because the first version of anything is typed by
    // hand at a prompt, and refusing it would make the tool harder to try than to
    // use.
    assert_eq!(token_from_header("Bearer secret"), Some("secret"));
    assert_eq!(token_from_header("  Bearer secret  "), Some("secret"));
    assert_eq!(token_from_header("secret"), Some("secret"));
    assert_eq!(token_from_header("bearer secret"), Some("bearer secret"));
}

#[test]
fn an_empty_header_is_no_token_rather_than_an_empty_one() {
    // The distinction matters: an empty token must not be compared against
    // anything, or a configuration with no token becomes a configuration that
    // accepts nothing in particular.
    //
    // The two `Bearer` cases below are the rule working out: the scheme is
    // `"Bearer "` *with the space*, so a header reading `Bearer` with nothing
    // after it has no prefix to strip and is a bare token whose value happens to
    // be the word. Refusing it would mean guessing at intent, and the client
    // never sends this -- the expectation here was wrong first, not the code.
    assert_eq!(token_from_header(""), None);
    assert_eq!(token_from_header("   "), None);
    // "Bearer" on its own is not the scheme plus nothing -- it has no space, so
    // it is a bare token whose value happens to be the word. Refusing it would
    // mean guessing at intent, and the client never sends this.
    assert_eq!(token_from_header("Bearer"), Some("Bearer"));
    assert_eq!(token_from_header("Bearer   "), Some("Bearer"));
}

#[test]
fn the_scheme_is_the_standard_one() {
    // `Authorization` rather than a made-up name: every proxy, log scrubber and
    // reader already treats that header as a secret. A custom name would be a
    // secret in a place nothing knows to look.
    assert_eq!(TOKEN_HEADER, "authorization");
    assert_eq!(TOKEN_SCHEME, "Bearer ");
}

// --- what the comparison returns ---------------------------------------------

#[test]
fn only_an_exact_match_matches() {
    let expected = "0123456789abcdef";
    assert!(token_matches(expected, expected));

    // Every position, so that a comparison that skipped one cannot pass.
    for index in 0..expected.len() {
        let mut wrong = expected.as_bytes().to_vec();
        wrong[index] = if wrong[index] == b'z' { b'y' } else { b'z' };
        let wrong = String::from_utf8(wrong).expect("still ASCII");
        assert!(
            !token_matches(expected, &wrong),
            "a token differing at byte {index} should not match"
        );
    }
}

#[test]
fn a_prefix_or_a_longer_value_does_not_match() {
    let expected = "0123456789abcdef";
    assert!(!token_matches(expected, "0123456789abcde"));
    assert!(!token_matches(expected, "0123456789abcdefg"));
    assert!(!token_matches(expected, ""));
}

#[test]
fn the_right_prefix_and_nothing_else_does_not_match() {
    // The exact shape a byte-at-a-time attack produces. It must not match, and
    // the timing test below is about the other half of the same attack.
    assert!(!token_matches("0123456789abcdef", "0123456789abcdeX"));
}

// --- the test that is about time ---------------------------------------------

#[test]
fn the_comparison_does_not_take_longer_when_more_of_the_token_is_right() {
    // Why this exists: comparing with `==` stops at the first differing byte, so a
    // wrong first byte costs one step and a right first byte costs two. Measured
    // over enough attempts, that difference recovers the secret one byte at a
    // time. Every functional test above passes with `==` and would not notice.
    //
    // How it is made reliable rather than flaky:
    //
    // - **Many samples, and the minimum.** The minimum is the cleanest estimate of
    //   the cost of the work: a slow sample is another process on the machine, and
    //   noise only ever adds. An average would move with the noise.
    // - **The same length for both cases**, so the length check is not what is
    //   being measured.
    // - **A ratio with a wide margin.** A byte-at-a-time leak on a 32-byte token
    //   is a difference of one iteration out of thirty-two, about 3%; the bound
    //   here is 50%, which a real leak cannot hide under and ordinary noise does
    //   not reach. A tight bound would be a flaky test, and a flaky timing test is
    //   deleted rather than fixed.
    let expected = "0123456789abcdefghijklmnopqrstuv";
    let wrong_at_the_start = format!("z{}", &expected[1..]);
    let wrong_at_the_end = format!("{}z", &expected[..expected.len() - 1]);

    assert_eq!(expected.len(), wrong_at_the_start.len());
    assert_eq!(expected.len(), wrong_at_the_end.len());

    let samples = 20_000;
    let time = |presented: &str| -> std::time::Duration {
        let mut best = std::time::Duration::MAX;
        for _ in 0..samples {
            let started = Instant::now();
            // `black_box` keeps the optimiser from proving the call has no effect
            // and removing it, which would make this measure nothing at all.
            std::hint::black_box(token_matches(
                std::hint::black_box(expected),
                std::hint::black_box(presented),
            ));
            let took = started.elapsed();
            if took < best {
                best = took;
            }
        }
        best
    };

    let early = time(&wrong_at_the_start);
    let late = time(&wrong_at_the_end);

    // Report the measurement either way: a failure that does not say what the
    // numbers were leaves whoever reads it unable to tell a real leak from a
    // noisy machine.
    eprintln!("wrong first byte: {early:?}   wrong last byte: {late:?}");

    let ratio = late.as_secs_f64() / early.as_secs_f64().max(f64::EPSILON);
    assert!(
        ratio < 1.5,
        "a token wrong at the last byte took {ratio:.2}x as long as one wrong at \
         the first ({late:?} against {early:?}). That is the shape of a comparison \
         that stops early, which is how a secret is recovered one byte at a time."
    );
}

#[test]
fn the_measurement_above_can_actually_see_a_difference() {
    // Guards the guard, for the same reason `architecture.rs` does: a timing
    // comparison that cannot detect a difference proves nothing when it detects
    // none. This measures something known to be slower against the same
    // baseline, and fails if the ratio machinery cannot tell them apart.
    //
    // The control is `token_matches` against a string of the same length versus
    // one a byte longer -- the second returns immediately on the length check,
    // which is a large and reliable difference.
    let expected = "0123456789abcdefghijklmnopqrstuv";
    let same_length = "z123456789abcdefghijklmnopqrstuv";
    let different_length = "0123456789abcdefghijklmnopqrstuvwxyz";

    let samples = 20_000;
    let time = |presented: &str| -> std::time::Duration {
        let mut best = std::time::Duration::MAX;
        for _ in 0..samples {
            let started = Instant::now();
            std::hint::black_box(token_matches(
                std::hint::black_box(expected),
                std::hint::black_box(presented),
            ));
            let took = started.elapsed();
            if took < best {
                best = took;
            }
        }
        best
    };

    let full = time(same_length);
    let short_circuit = time(different_length);
    eprintln!("full compare: {full:?}   length short-circuit: {short_circuit:?}");

    assert!(
        short_circuit < full,
        "the length check should be cheaper than comparing every byte \
         ({short_circuit:?} against {full:?}); if this fails the measurement is not \
         measuring, and the test above proves nothing"
    );
}

// --- what a refusal says -----------------------------------------------------

#[test]
fn the_refusal_does_not_say_why() {
    // Whether the token was absent, wrong or too short is information a caller
    // who has the token does not need, and one who does not have it should not be
    // given.
    let body = linklet_core::json::write(&unauthorized_body());
    assert!(body.contains("missing or wrong"), "{body}");
    for hint in ["absent", "empty", "length", "expired"] {
        assert!(
            !body.to_lowercase().contains(hint),
            "the refusal hints at {hint:?}: {body}"
        );
    }
}
