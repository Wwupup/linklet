//! The specification for `parse_targets`, written before the implementation.
//!
//! Run `cargo test -p linklet-core` now: it fails, and it must fail for the
//! reason we intend (`unimplemented!`), not because the tests do not compile.
//!
//! Note what is absent from this file: no network, no files, no temporary
//! directory, no cleanup, no timing. That is what "the core is pure" buys --
//! these tests are a specification that runs in microseconds, so there is
//! never a reason not to run them.

// A panic in a test is not a bug; it *is* the failure signal. So the "no
// unwrap in library code" rule (AGENTS.md rule 6) does not apply in this file.
// The allow is scoped here on purpose: granting it workspace-wide would also
// silence it in `src/`, where a panic is a defect the caller cannot handle.
#![allow(clippy::unwrap_used)]

use linklet_core::{Host, Port, Target, TargetError, parse_targets};

/// Builds the expected target, so a test reads as data rather than noise.
fn t(host: &str, port: u16) -> Target {
    Target {
        host: Host::new(host),
        port: Port::new(port),
    }
}

/// Unwraps the error side, with a message that names the spec under test.
///
/// Every assertion below goes through this instead of `unwrap_err`, so a
/// failure reports "a:0 should have been refused" rather than the message
/// clippy suggests, which says nothing about which case broke. An `expect`
/// message is read at exactly one moment -- when something is already wrong --
/// so it should be written for that reader.
fn err_of(spec: &str) -> TargetError {
    parse_targets(spec).expect_err("this spec should have been refused, but it was accepted")
}

// --- what must be accepted ---------------------------------------------------

#[test]
fn single_target_with_port() {
    assert_eq!(
        parse_targets("192.168.3.5:8787").expect("a plain address:port is the base case"),
        vec![t("192.168.3.5", 8787)]
    );
}

#[test]
fn hostname_is_carried_through_unchanged() {
    // Case is preserved deliberately: "build-box" and "BUILD-BOX" may be the
    // same machine to DNS and are different strings to the user who typed them,
    // and the tool echoes back what it was given.
    assert_eq!(
        parse_targets("BUILD-BOX:80").expect("a hostname is as valid as an address"),
        vec![t("BUILD-BOX", 80)]
    );
}

#[test]
fn several_targets_are_split_on_commas() {
    assert_eq!(
        parse_targets("10.0.0.1:80,10.0.0.2:81").expect("two specs are two targets"),
        vec![t("10.0.0.1", 80), t("10.0.0.2", 81)]
    );
}

#[test]
fn whitespace_around_a_spec_is_not_part_of_it() {
    // Note the tab. Trimming has to handle any whitespace, not just the space
    // character, and this test fails if the implementation only handles " ".
    assert_eq!(
        parse_targets("  a:1 ,\tb:2  ").expect("surrounding whitespace is not part of a spec"),
        vec![t("a", 1), t("b", 2)]
    );
}

#[test]
fn the_port_separator_is_the_last_colon() {
    // An IPv6 address is full of colons, so "the first colon" cannot be right.
    assert_eq!(
        parse_targets("::1:80").expect("the last colon separates host from port"),
        vec![t("::1", 80)]
    );
}

#[test]
fn order_is_preserved_and_duplicates_are_not_deduplicated() {
    // Deduplicating here would look helpful and be wrong: the caller may have
    // its own reason to name a machine twice, and only the caller can know.
    assert_eq!(
        parse_targets("b:2,a:1,b:2").expect("the input order is the output order"),
        vec![t("b", 2), t("a", 1), t("b", 2)]
    );
}

#[test]
fn the_ends_of_the_port_range_are_accepted() {
    assert_eq!(
        parse_targets("h:1").expect("port 1 is the low end of the range"),
        vec![t("h", 1)]
    );
    assert_eq!(
        parse_targets("h:65535").expect("port 65535 is the high end of the range"),
        vec![t("h", 65535)]
    );
}

#[test]
fn a_host_may_contain_a_hyphen_or_a_dot() {
    assert_eq!(
        parse_targets("build-box-01.corp:22").expect("a real hostname is not a syntax error"),
        vec![t("build-box-01.corp", 22)]
    );
}

// --- what must be refused, and how it must be explained ----------------------

#[test]
fn empty_input_is_refused() {
    assert_eq!(err_of(""), TargetError::EmptyInput);
    assert_eq!(err_of("   "), TargetError::EmptyInput);
}

#[test]
fn an_empty_entry_is_refused_rather_than_skipped() {
    // A trailing comma is a typo. Skipping it silently teaches the caller that
    // the input does not matter -- and the next typo is one that hides a host.
    assert_eq!(err_of("a:1,,b:2"), TargetError::EmptySpec);
    assert_eq!(err_of("a:1,"), TargetError::EmptySpec);
}

#[test]
fn whitespace_inside_a_spec_is_refused() {
    // Trimming the ends is forgiving; splitting on the inside is inventing
    // intent. Note that "a:1 b:2" is one spec containing a space, not two.
    assert_eq!(
        err_of("a b:1"),
        TargetError::WhitespaceInSpec {
            spec: "a b:1".to_string()
        }
    );
}

#[test]
fn a_trailing_colon_with_no_port_is_refused() {
    assert_eq!(
        err_of("a:"),
        TargetError::PortMissing {
            spec: "a:".to_string()
        }
    );
}

#[test]
fn a_non_numeric_port_is_refused() {
    assert_eq!(
        err_of("a:http"),
        TargetError::PortNotANumber {
            spec: "a:http".to_string(),
            port: "http".to_string()
        }
    );
}

#[test]
fn a_port_out_of_range_is_refused_and_the_number_is_reported() {
    assert_eq!(
        err_of("a:0"),
        TargetError::PortOutOfRange {
            spec: "a:0".to_string(),
            value: 0
        }
    );
    assert_eq!(
        err_of("a:65536"),
        TargetError::PortOutOfRange {
            spec: "a:65536".to_string(),
            value: 65536
        }
    );
    // Wider than the error type's own number. It must still be reported as out
    // of range rather than as "too big to parse": to the caller, "that port
    // does not exist" is the useful sentence either way. The test pins
    // u64::MAX because that is the widest value the variant can carry, and the
    // implementation has to saturate to reach it.
    assert_eq!(
        err_of("a:99999999999999999999"),
        TargetError::PortOutOfRange {
            spec: "a:99999999999999999999".to_string(),
            value: u64::MAX
        }
    );
}

#[test]
fn a_spec_with_no_port_at_all_is_refused() {
    // The default port is the caller's decision. Guessing it here would make
    // "a" mean something different in this tool than in the caller's head.
    assert_eq!(
        err_of("a"),
        TargetError::PortNotANumber {
            spec: "a".to_string(),
            port: String::new()
        }
    );
}

#[test]
fn the_error_explains_itself_for_a_human() {
    // The message is part of the product: an agent quotes it back to a user,
    // and "invalid input" is not something a user can act on.
    let err = err_of("a:0");
    assert_eq!(
        err.to_string(),
        "target \"a:0\" has port 0, outside 1..=65535"
    );
}
