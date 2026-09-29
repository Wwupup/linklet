//! The output format and the exit codes, pinned.
//!
//! These tests are the contract. An agent on the other side of this tool
//! branches on the exit code and parses the lines, so a change here is a change
//! to a published interface -- which is why it should be hard to make by
//! accident, and require an argument to make on purpose.

use linklet_core::{ExitCode, Host, Port, Report, Status, Target, exit_code_for, render};

fn target(host: &str, port: u16) -> Target {
    Target {
        host: Host::new(host),
        port: Port::new(port),
    }
}

fn report(status: Status, reason: &str) -> Report {
    Report {
        target: target("10.0.0.1", 8787),
        status,
        reason: reason.to_string(),
    }
}

fn alive() -> Report {
    report(Status::Alive, "connected")
}

// --- exit codes --------------------------------------------------------------

#[test]
fn the_codes_are_the_documented_numbers() {
    // Written as an explicit table rather than trusting the constants to stay
    // put: these numbers are the interface, so changing one has to be a
    // deliberate edit to this test as well.
    assert_eq!(ExitCode::SUCCESS, 0);
    assert_eq!(ExitCode::NOT_ALL_ALIVE, 1);
    assert_eq!(ExitCode::USAGE, 2);
    assert_eq!(ExitCode::REFUSED, 3);
}

#[test]
fn a_run_where_everything_is_alive_succeeds() {
    assert_eq!(exit_code_for(&[alive(), alive()]), ExitCode::SUCCESS);
}

#[test]
fn a_run_with_one_dead_target_does_not() {
    let reports = [
        alive(),
        report(Status::Refused, "the machine refused the connection"),
    ];
    assert_eq!(exit_code_for(&reports), ExitCode::NOT_ALL_ALIVE);
}

#[test]
fn an_unreachable_target_alone_is_enough_to_fail_the_run() {
    assert_eq!(
        exit_code_for(&[report(Status::Unreachable, "no answer within 5 s")]),
        ExitCode::NOT_ALL_ALIVE
    );
}

#[test]
fn an_unknown_target_alone_is_enough_to_fail_the_run() {
    // The tempting alternative is to treat "we could not tell" as a pass, which
    // would make a name-resolution failure look like a healthy machine. Not
    // knowing is not the same as being fine.
    assert_eq!(
        exit_code_for(&[report(
            Status::Unknown("cannot resolve".into()),
            "cannot resolve"
        )]),
        ExitCode::NOT_ALL_ALIVE
    );
}

#[test]
fn a_completed_run_of_nothing_is_a_success_at_nothing() {
    // Looks wrong, is deliberate. The refusal for an empty target list happens
    // in `check_targets`, before this function is reachable; letting both cases
    // land here would give one situation two answers.
    assert_eq!(exit_code_for(&[]), ExitCode::SUCCESS);
}

// --- the line format ---------------------------------------------------------

#[test]
fn a_line_is_status_target_reason() {
    let line = render(&report(
        Status::Refused,
        "the machine refused the connection",
    ));
    assert_eq!(
        line,
        "dead 10.0.0.1:8787 the machine refused the connection"
    );
}

#[test]
fn the_status_token_is_the_first_word_and_has_no_space_in_it() {
    // The property an agent's parser depends on: one `split` and the most
    // important field is out. Tested for every status, because the value of the
    // property is that it holds for all of them.
    for status in [
        Status::Alive,
        Status::Refused,
        Status::Unreachable,
        Status::Unknown("x".into()),
    ] {
        let line = render(&report(status, "why"));
        let first = line.split_whitespace().next().expect("a line has a word");
        assert!(
            ["live", "dead", "unknown"].contains(&first),
            "the first word is {first:?}, which is not one of the three tokens"
        );
    }
}

#[test]
fn refused_and_unreachable_both_read_as_dead_but_say_different_things() {
    // The deliberate loss: one bit for the caller, and the distinction kept in
    // the reason where a human and a language model both read it.
    let refused = render(&report(
        Status::Refused,
        "the machine refused the connection",
    ));
    let unreachable = render(&report(Status::Unreachable, "no answer within 5 s"));

    assert!(refused.starts_with("dead "));
    assert!(unreachable.starts_with("dead "));
    assert_ne!(refused, unreachable);
    assert!(refused.contains("refused"));
    assert!(unreachable.contains("no answer"));
}

#[test]
fn a_line_carries_no_line_break_of_its_own() {
    // If a report could contain a newline, one report would become two lines and
    // every caller that counts lines would be quietly wrong. The reasons are
    // built from budgets and adapter messages, so this is checking an assumption
    // that the rest of the format rests on.
    for status in [
        Status::Alive,
        Status::Refused,
        Status::Unreachable,
        Status::Unknown("cannot resolve a:80".into()),
    ] {
        let line = render(&report(status, "the machine refused the connection"));
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
    }
}

#[test]
fn the_line_shows_the_target_exactly_as_it_was_written() {
    // Round-tripping matters: an agent quoting a failure back to a user must
    // quote the address the user typed, not a normalised version of it.
    let odd = Report {
        target: target("BUILD-BOX", 80),
        status: Status::Alive,
        reason: "connected".to_string(),
    };
    assert_eq!(render(&odd), "live BUILD-BOX:80 connected");
}
