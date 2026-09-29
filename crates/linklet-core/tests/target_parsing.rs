//! The specification for `parse_targets`, written before the implementation.
//!
//! It was written first and observed failing first -- 19 tests, all failing on
//! the same `unimplemented!` rather than on a compile error. That order is the
//! point: a specification written after the code describes whatever the code
//! does, including its mistakes.
//!
//! Note what is absent from this file: no network, no files, no temporary
//! directory, no cleanup, no timing. That is what "the core is pure" buys --
//! the whole specification runs in about 20 milliseconds, so there is never a
//! reason not to run it.
//!
//! Failures use `expect` with a message naming the case rather than bare
//! `unwrap`, so a red run says which input broke instead of saying that an
//! `Option` was `None` somewhere. That is why this file needs no
//! `#[allow(clippy::unwrap_used)]`: there is nothing to allow.

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

#[test]
fn a_host_that_is_nonsense_is_still_accepted() {
    // This test asserts that nothing is asserted about hostnames, and that is
    // its purpose. `"!!!"` cannot be a hostname, and passing it through is the
    // intended behaviour: deciding what a hostname looks like is the resolver's
    // job, and this layer has no way to do it. A parser that "helpfully"
    // rejected strange names would also reject real ones it had not thought of,
    // and the failure would land on a user with a name that does resolve.
    //
    // The test is here so that the looseness is a decision rather than an
    // accident: if someone later tightens the host rule, this fails, and they
    // have to argue with a test that says why rather than with a person who
    // remembers.
    assert_eq!(
        parse_targets("!!!:80").expect("the host grammar is permissive on purpose"),
        vec![t("!!!", 80)]
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
fn a_space_after_the_colon_is_still_a_whitespace_error() {
    // This one pins the *order* of the rules, not the rules themselves, and it
    // is the case where two reasonable implementations disagree.
    //
    // "a:1 b:2" breaks two rules at once: it contains a space, and "1 b" is not
    // a number. An implementation that splits on the colon first reports
    // `PortNotANumber { port: "1 b" }`; one that checks whitespace first
    // reports `WhitespaceInSpec`. Both are defensible, so the specification has
    // to pick one -- an unstated rule like this is where two correct-looking
    // programs diverge, and where the second author concludes they are wrong.
    //
    // The whole spec contains whitespace, so the error names the whole spec.
    // Reporting a fragment ("1 b") as the problem would describe a string the
    // caller never wrote as a separate thing.
    assert_eq!(
        err_of("a:1 b:2"),
        TargetError::WhitespaceInSpec {
            spec: "a:1 b:2".to_string()
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
fn a_port_with_no_host_in_front_of_it_is_refused() {
    // ":80" names a machine that was never written. The port is perfectly good
    // and there is nothing to connect to, so this is `PortMissing` -- the same
    // variant as "a:" -- rather than `PortNotANumber`, which would blame the
    // half of the spec that is correct.
    //
    // This test was missing when the rule order was first written down, and the
    // documentation named the case anyway (step 6). A rule in prose with no test
    // under it is the failure this repository is about: the next person reads
    // the sentence, believes it, and finds out otherwise from a user.
    assert_eq!(
        err_of(":80"),
        TargetError::PortMissing {
            spec: ":80".to_string()
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
    //
    // This is the same fact as `"a:"` above -- no port was named -- so it is
    // the same error. It was `PortNotANumber { port: "" }` in the first draft
    // of this specification, which was wrong: an error named "the port is not a
    // number" cannot be the right answer for a spec where no port was written.
    // An error type that needs a comment to explain which input reaches it is
    // the wrong type, and the cost of that is paid by whoever implements the
    // parser -- they write the code that makes the misleading message true.
    assert_eq!(
        err_of("a"),
        TargetError::PortMissing {
            spec: "a".to_string()
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
