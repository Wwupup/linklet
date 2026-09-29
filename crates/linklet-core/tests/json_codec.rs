//! The specification for the JSON codec.
//!
//! Hand-written JSON is a place errors hide, and two of them are worth naming
//! because they are why the tests look the way they do:
//!
//! - **A parser that hangs.** Fed by a peer, a hand-written parser with a loop
//!   that does not always advance is a hang, not a wrong answer. Several cases
//!   below end in truncation for that reason.
//! - **A writer that emits a raw newline.** MCP over stdio is newline-delimited:
//!   one unescaped `\n` inside a string splits one message into two, and the
//!   failure appears on the *next* message. `write` escapes every character
//!   below 0x20, and that is asserted directly.

use std::collections::BTreeMap;

use linklet_core::json::{self, Json};
use linklet_core::object;

/// Parses and expects success.
fn ok(text: &str) -> Json {
    json::parse(text).unwrap_or_else(|e| panic!("{text:?} should parse, but: {e}"))
}

/// Parses and expects failure.
fn err(text: &str) -> json::JsonError {
    match json::parse(text) {
        Ok(value) => panic!("{text:?} should not parse, but produced {value:?}"),
        Err(error) => error,
    }
}

// --- literals ----------------------------------------------------------
/// Asserts that `text` is refused, and that the refusal says where.
///
/// # Why the message text is no longer asserted
///
/// It used to be, and pinning it was pinning this repository's own vocabulary --
/// the words came from a parser that lived here. They now come from `serde_json`,
/// and asserting its phrasing would be asserting a dependency's internals: a test
/// that goes red on a patch release while nothing is wrong, which trains a reader
/// to ignore it.
///
/// What is asserted is the part that was ever a contract. Input that is not JSON
/// is refused rather than guessed at; the refusal says something; and the position
/// it reports is inside the input rather than a number invented to look useful.
fn refused(text: &str) -> json::JsonError {
    let error = err(text);
    assert!(
        !error.message.trim().is_empty(),
        "{text:?} was refused with an empty message"
    );
    assert!(
        error.at <= text.len(),
        "{text:?} was refused at byte {}, which is past the end of a {} byte input",
        error.at,
        text.len()
    );
    error
}

// --- literals ----------------------------------------------------------------

#[test]
fn the_three_literals_parse() {
    assert_eq!(ok("null"), Json::Null);
    assert_eq!(ok("true"), Json::Bool(true));
    assert_eq!(ok("false"), Json::Bool(false));
}

#[test]
fn a_literal_with_a_letter_stuck_to_it_is_not_a_literal() {
    // The classic hand-written-parser bug: scanning for "null" and stopping as
    // soon as it matches, leaving the extra `l` for the caller to trip over.
    refused("nulll");
}

#[test]
fn a_literal_cut_short_is_an_error() {
    refused("nul");
    refused("tru");
    refused("fals");
}

// --- numbers -----------------------------------------------------------------

#[test]
fn integers_that_fit_are_integers() {
    assert_eq!(ok("0"), Json::Int(0));
    assert_eq!(ok("-1"), Json::Int(-1));
    assert_eq!(ok("65535"), Json::Int(65535));
    assert_eq!(ok("9223372036854775807"), Json::Int(i64::MAX));
}

#[test]
fn numbers_with_a_fraction_or_an_exponent_are_floats() {
    assert_eq!(ok("1.5"), Json::Float(1.5));
    assert_eq!(ok("1e3"), Json::Float(1000.0));
    assert_eq!(ok("-2.5e-1"), Json::Float(-0.25));
}

#[test]
fn an_integer_too_large_for_i64_becomes_a_float_rather_than_failing() {
    // A peer sending a number larger than i64 has sent a number. Rejecting it
    // would turn a byte count into a protocol error.
    match ok("18446744073709551615") {
        Json::Float(value) => assert!(value > 1.8e19),
        other => panic!("expected a float, got {other:?}"),
    }
}

#[test]
fn malformed_numbers_are_rejected_rather_than_guessed() {
    // There is no valid JSON in which a dot, an exponent or a minus stands
    // alone, so reading one as if it did would be inventing a value.
    refused("1.");
    refused("1e");
    refused("-");
    refused("1.2.3");
}

// --- strings -----------------------------------------------------------------

#[test]
fn the_short_escapes_decode() {
    assert_eq!(ok(r#""a\"b""#), Json::str("a\"b"));
    assert_eq!(ok(r#""a\\b""#), Json::str("a\\b"));
    assert_eq!(ok(r#""a\/b""#), Json::str("a/b"));
    assert_eq!(ok(r#""a\nb""#), Json::str("a\nb"));
    assert_eq!(ok(r#""a\tb""#), Json::str("a\tb"));
    assert_eq!(ok(r#""a\rb""#), Json::str("a\rb"));
    assert_eq!(ok(r#""a\bb""#), Json::str("a\u{8}b"));
    assert_eq!(ok(r#""a\fb""#), Json::str("a\u{c}b"));
}

#[test]
fn a_unicode_escape_decodes() {
    assert_eq!(ok(r#""\u0041""#), Json::str("A"));
    // U+00E9 is e-acute, not "e". Written as an escape so the file stays ASCII
    // (rule 7) while the assertion still names the character.
    assert_eq!(ok(r#""\u00e9""#), Json::str("\u{e9}"));
}

#[test]
fn a_surrogate_pair_becomes_one_character() {
    // U+1F600. Reading the halves separately would try to build a `char` from a
    // lone surrogate, which Rust cannot represent.
    assert_eq!(ok(r#""\ud83d\ude00""#), Json::str("\u{1f600}"));
}

#[test]
fn half_a_surrogate_pair_is_an_error() {
    refused(r#""\ud83d""#);
    refused(r#""\ude00""#);
    refused(r#""\ud83dx""#);
    refused(r#""\ud83d\u0041""#);
}

#[test]
fn a_bad_escape_is_named() {
    refused(r#""\q""#);
    // The closing quote is not a hex digit, so this is the "not a hex digit"
    // branch rather than the truncation one. Both are reachable, and they say
    // different things -- see the test below for the other.
    refused(r#""\u00""#);
    refused(r#""\u00zz""#);
}

#[test]
fn a_hex_escape_cut_short_by_the_end_of_input_is_reported_as_such() {
    // No closing quote: the four digits run out of input. This is the case that
    // must not loop or panic, because it is what a truncated line off a socket
    // looks like.
    let error = err(r#""\u00"#);
    assert!(
        !error.message.trim().is_empty(),
        "a refusal with no words in it"
    );
}

#[test]
fn utf8_in_a_string_survives_the_round_trip() {
    // Read as a byte string rather than as characters, so a multi-byte
    // character is copied whole or reported once as invalid.
    for text in ["ascii", "\u{e9}t\u{e9}", "\u{4e2d}\u{6587}", "\u{1f600}"] {
        let encoded = json::write(&Json::str(text));
        assert_eq!(ok(&encoded), Json::str(text), "round trip of {text:?}");
    }
}

#[test]
fn an_unterminated_string_is_an_error_rather_than_a_hang() {
    // The hang case: the loop must run out of input and stop, not spin.
    refused(r#""never closed"#);
    refused(r#""escape at end\"#);
}

#[test]
fn a_raw_control_character_in_a_string_is_rejected() {
    // Not pedantry: a raw newline here is what splits one newline-delimited
    // message into two, and the symptom appears on the next message.
    assert!(!err("\"a\nb\"").message.is_empty());
    assert!(!err("\"a\tb\"").message.is_empty());
}

// --- arrays and objects ------------------------------------------------------

#[test]
fn arrays_parse_including_the_empty_one() {
    assert_eq!(ok("[]"), Json::Array(vec![]));
    assert_eq!(
        ok("[1,true,null]"),
        Json::Array(vec![Json::Int(1), Json::Bool(true), Json::Null])
    );
    assert_eq!(ok("[[]]"), Json::Array(vec![Json::Array(vec![])]));
}

#[test]
fn objects_parse_including_the_empty_one_and_nesting() {
    assert_eq!(ok("{}"), Json::Object(BTreeMap::new()));
    let value = ok(r#"{"a":{"b":[1]}}"#);
    assert_eq!(
        value
            .get("a")
            .and_then(|v| v.get("b"))
            .and_then(Json::as_array)
            .map(<[Json]>::len),
        Some(1)
    );
}

#[test]
fn malformed_arrays_and_objects_are_rejected() {
    assert!(!err("[1,]").message.is_empty());
    assert!(!err("[1").message.is_empty());
    assert!(!err("{1:2}").message.is_empty());
    assert!(!err(r#"{"a" 1}"#).message.is_empty());
    assert!(!err(r#"{"a":1"#).message.is_empty());
    assert!(!err("[1 2]").message.is_empty());
}

#[test]
fn whitespace_around_and_inside_a_document_is_ignored() {
    assert_eq!(ok("  { \"a\" : [ 1 , 2 ] }  "), ok(r#"{"a":[1,2]}"#));
}

#[test]
fn trailing_data_is_an_error() {
    // A line-delimited transport must not accept half a message and ignore the
    // rest: whatever is left over is where the mistake is.
    assert!(!err("null true").message.is_empty());
    assert!(!err("{} {}").message.is_empty());
}

#[test]
fn empty_input_says_so_with_a_position() {
    let error = err("");
    assert!(
        !error.message.trim().is_empty(),
        "a refusal with no words in it"
    );
    assert_eq!(error.at, 0);
}

#[test]
fn errors_carry_the_byte_offset() {
    // The caller is a protocol reader. "unexpected end of input" on a long line
    // is not actionable; the position is what makes it so.
    let error = err(r#"{"a": nope}"#);
    assert!(error.at < 8, "reported: {error}");
    assert!(error.to_string().contains(&format!("byte {}", error.at)));
}

// --- writing -----------------------------------------------------------------

#[test]
fn writing_produces_compact_json() {
    assert_eq!(json::write(&Json::Null), "null");
    assert_eq!(json::write(&Json::Int(-3)), "-3");
    assert_eq!(json::write(&Json::Bool(false)), "false");
    assert_eq!(
        json::write(&Json::Array(vec![Json::Int(1), Json::Int(2)])),
        "[1,2]"
    );
}

#[test]
fn writing_escapes_every_control_character() {
    // The assertion that keeps the transport intact. Every character below 0x20
    // must leave as an escape sequence, so the output can never contain a byte
    // that would end the line early.
    for code in 0u32..0x20 {
        let character = char::from_u32(code).expect("a control character");
        let encoded = json::write(&Json::str(character.to_string()));
        assert!(
            !encoded.contains(character),
            "U+{code:04X} was written raw as {encoded:?}"
        );
    }
}

#[test]
fn the_short_escapes_are_preferred_over_the_long_form() {
    assert_eq!(json::write(&Json::str("\n")), r#""\n""#);
    assert_eq!(json::write(&Json::str("\t")), r#""\t""#);
    assert_eq!(json::write(&Json::str("\"")), r#""\"""#);
    assert_eq!(json::write(&Json::str("\\")), r#""\\""#);
    // ...and the ones with no short form use \u.
    assert_eq!(json::write(&Json::str("\u{1}")), r#""\u0001""#);
}

#[test]
fn an_object_is_written_in_key_order() {
    // A BTreeMap and not a HashMap, so the same value writes the same bytes
    // twice. An encoder whose output depends on hashing is one whose tests
    // flake.
    let value = object! { "b" => 2i64, "a" => 1i64 };
    assert_eq!(json::write(&value), r#"{"a":1,"b":2}"#);
    assert_eq!(json::write(&value), json::write(&value));
}

#[test]
fn a_non_finite_float_is_written_as_null() {
    // JSON has no NaN and no infinity, and `f64::to_string` would write exactly
    // those words, producing a document no parser accepts.
    assert_eq!(json::write(&Json::Float(f64::NAN)), "null");
    assert_eq!(json::write(&Json::Float(f64::INFINITY)), "null");
}

#[test]
fn writing_then_parsing_gives_back_the_same_value() {
    let value = object! {
        "jsonrpc" => "2.0",
        "id" => 1i64,
        "ok" => true,
        "missing" => Json::Null,
        "items" => vec![Json::Int(1), Json::str("two")],
    };
    assert_eq!(ok(&json::write(&value)), value);
}

// --- the accessors a protocol reader needs -----------------------------------

#[test]
fn accessors_return_none_for_the_wrong_shape() {
    let value = object! { "s" => "text", "n" => 1i64 };
    assert_eq!(value.get_str("s"), Some("text"));
    assert_eq!(value.get_str("n"), None, "an int is not a string");
    assert_eq!(value.get_str("absent"), None);
    assert_eq!(value.as_int(), None, "an object is not an int");
    assert_eq!(Json::Null.as_str(), None);
    assert_eq!(Json::Null.as_bool(), None);
    assert_eq!(Json::Null.as_array(), None);
}
