//! The specification for what crosses the wire between a host and an agent.
//!
//! The claim this file exists to protect is in the module documentation of
//! `linklet_core::wire`, and it is the one that decides whether an agent
//! behaves well when something goes wrong:
//!
//! > **A command that ran and failed is not a request that failed.**
//!
//! `exit_code: 1` from a program that did its job is a successful call carrying
//! bad news. A command that never started is a call that could not be made. An
//! agent that folds the two together retries targets that already answered, and
//! reports "the machine is broken" for a program that printed a usage message.
//!
//! Everything else here is shape: JSON is a place where a missing field and a
//! null field look similar in prose and are different in meaning.

use linklet_core::json::{self, Json};
use linklet_core::object;
use linklet_core::wire::{
    self, KILLED_BY_DEADLINE, MAX_TIMEOUT_SECONDS, RunOutcome, RunRequest, WireError,
};

// --- the distinction the module exists for -----------------------------------

#[test]
fn a_command_that_ran_and_failed_is_an_outcome_and_not_an_error() {
    // The whole point, as one test. Exit code 1 reaches the caller as a fact.
    let outcome = RunOutcome {
        exit_code: Some(1),
        stdout: String::new(),
        stderr: "usage: app [options]".to_string(),
        duration_ms: 12,
        reason: None,
    };

    let encoded = wire::encode_run_reply(&outcome);
    let decoded = wire::decode_run_reply(&encoded).expect("its own output should decode");

    assert_eq!(decoded, outcome);
    assert_eq!(decoded.exit_code, Some(1));
    assert!(
        decoded.reason.is_none(),
        "a program that ran has no reason to explain"
    );
}

#[test]
fn a_command_that_never_started_has_no_exit_code_and_says_why() {
    // The other half. No exit code because there was no process, and a reason
    // because otherwise the caller cannot tell this from a process that was
    // killed by its deadline.
    let outcome = RunOutcome {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 0,
        reason: Some("cannot spawn: the file does not exist".to_string()),
    };

    let decoded = wire::decode_run_reply(&wire::encode_run_reply(&outcome)).expect("decodes");

    assert_eq!(decoded, outcome);
    assert_ne!(
        decoded.reason.as_deref(),
        Some(KILLED_BY_DEADLINE),
        "failing to start is not a deadline"
    );
}

#[test]
fn a_deadline_kill_is_a_distinguished_reason_and_not_a_sentence() {
    // The host acts on this: a timeout means the command may have been doing
    // something useful and its output is worth reading, and "failed to start"
    // means nothing happened. Comparing prose would make that a fragile thing to
    // depend on, so the value is a constant.
    let outcome = RunOutcome {
        exit_code: None,
        stdout: "half a build".to_string(),
        stderr: String::new(),
        duration_ms: 900_000,
        reason: Some(KILLED_BY_DEADLINE.to_string()),
    };

    let decoded = wire::decode_run_reply(&wire::encode_run_reply(&outcome)).expect("decodes");

    assert_eq!(decoded.reason.as_deref(), Some(KILLED_BY_DEADLINE));
    assert_eq!(
        decoded.stdout, "half a build",
        "a killed command's output is still worth having"
    );
}

#[test]
fn a_null_exit_code_and_a_missing_one_are_different_messages() {
    // In prose these read the same. They are not: `null` is "the agent looked and
    // there is no code", missing is "this is not the shape I sent".
    let with_null = object! {
        "exit_code" => Json::Null,
        "stdout" => Json::str(""),
        "stderr" => Json::str(""),
        "duration_ms" => 0i64,
        "reason" => Json::str("failed to start"),
    };
    assert_eq!(
        wire::run_outcome_from_json(&with_null)
            .expect("null is a valid exit code")
            .exit_code,
        None
    );

    let without = object! {
        "stdout" => Json::str(""),
        "stderr" => Json::str(""),
        "duration_ms" => 0i64,
        "reason" => Json::Null,
    };
    let error =
        wire::run_outcome_from_json(&without).expect_err("a missing field is not a null one");
    assert!(error.to_string().contains("exit_code"), "{error}");
}

// --- the request -------------------------------------------------------------

#[test]
fn a_request_round_trips() {
    let request = RunRequest {
        command: "build.cmd --release".to_string(),
        timeout_seconds: 900 % MAX_TIMEOUT_SECONDS,
    };
    let encoded = json::write(&wire::run_request_to_json(&request));
    let decoded = wire::run_request_from_json(&json::parse(&encoded).expect("valid JSON"))
        .expect("its own output should decode");
    assert_eq!(decoded, request);
}

#[test]
fn a_request_without_a_usable_command_is_refused_with_the_field() {
    let cases: Vec<(Json, &str)> = vec![
        (object! { "timeout_seconds" => 5i64 }, "command"),
        (
            object! { "command" => Json::Int(1), "timeout_seconds" => 5i64 },
            "command",
        ),
        (
            object! { "command" => Json::str("   "), "timeout_seconds" => 5i64 },
            "empty",
        ),
        (object! { "command" => Json::str("x") }, "timeout_seconds"),
        (
            object! { "command" => Json::str("x"), "timeout_seconds" => Json::str("5") },
            "not a number",
        ),
        (
            object! { "command" => Json::str("x"), "timeout_seconds" => 0i64 },
            "outside",
        ),
    ];

    for (value, expected) in cases {
        let error = wire::run_request_from_json(&value)
            .expect_err("this request should have been refused, and it was accepted");
        assert!(
            error.to_string().contains(expected),
            "for {value:?}, expected {expected:?} in the error, got {error}"
        );
    }
}

#[test]
fn an_insane_timeout_is_refused_rather_than_clamped() {
    // An agent that accepts an unbounded timeout from the network has been handed
    // a way to be occupied forever.
    let too_long = object! { "command" => Json::str("x"), "timeout_seconds" => (MAX_TIMEOUT_SECONDS + 1) as i64 };
    let error = wire::run_request_from_json(&too_long).expect_err("should be refused");
    assert!(
        matches!(&error, WireError::BadRequest(field) if field.contains("timeout_seconds")),
        "{error}"
    );

    let at_the_limit =
        object! { "command" => Json::str("x"), "timeout_seconds" => MAX_TIMEOUT_SECONDS as i64 };
    assert!(
        wire::run_request_from_json(&at_the_limit).is_ok(),
        "the limit itself is allowed"
    );
}

// --- the shape of the body ---------------------------------------------------

#[test]
fn the_error_shape_names_the_problem_and_nothing_else() {
    // Deliberately not a result with an exit code: an error body says the request
    // could not be answered, and the host must not read it looking for output.
    let body = json::write(&wire::wire_error_to_json(&WireError::BadRequest(
        "timeout_seconds: missing".to_string(),
    )));
    let value = json::parse(&body).expect("valid JSON");
    let message = value.get_str("error").expect("an error field");

    assert!(message.contains("timeout_seconds"), "{message}");
    assert!(
        value.get("exit_code").is_none(),
        "an error body must not carry a result field, or the two shapes become one"
    );
}

#[test]
fn a_body_that_is_not_json_is_a_bad_request_and_not_a_panic() {
    let error = wire::decode_run_reply("not json").expect_err("should be refused");
    assert!(matches!(error, WireError::BadRequest(_)));

    // ...and a JSON body of the wrong shape, which is the case a mock server
    // someone wrote by hand produces.
    let error = wire::decode_run_reply(r#"{"exit_code": 0}"#).expect_err("should be refused");
    assert!(error.to_string().contains("stdout"), "{error}");
}

// --- what the agent reads ----------------------------------------------------

#[test]
fn a_successful_run_renders_the_exit_code_the_time_and_the_streams() {
    let outcome = RunOutcome {
        exit_code: Some(0),
        stdout: "built 3 targets".to_string(),
        stderr: String::new(),
        duration_ms: 1234,
        reason: None,
    };

    assert_eq!(
        wire::render_run(&outcome),
        "exit 0\ntook 1234 ms\nstdout:\nbuilt 3 targets"
    );
}

#[test]
fn both_streams_render_when_both_have_something() {
    // A program that writes to both is the normal case for a failing build, and
    // a renderer that shows only one of them loses half the diagnosis.
    let outcome = RunOutcome {
        exit_code: Some(1),
        stdout: "compiling".to_string(),
        stderr: "error: expected ';'".to_string(),
        duration_ms: 40,
        reason: None,
    };

    let text = wire::render_run(&outcome);
    assert!(text.contains("stdout:\ncompiling"), "{text}");
    assert!(text.contains("stderr:\nerror: expected ';'"), "{text}");
}

#[test]
fn an_empty_stream_is_not_rendered_as_an_empty_heading() {
    // "stderr:" followed by nothing is a line that carries no information and
    // costs a reader attention.
    let outcome = RunOutcome {
        exit_code: Some(0),
        stdout: "ok".to_string(),
        stderr: String::new(),
        duration_ms: 1,
        reason: None,
    };

    let text = wire::render_run(&outcome);
    assert!(!text.contains("stderr"), "{text}");
}

#[test]
fn a_killed_run_renders_its_reason_instead_of_an_exit_code() {
    let outcome = RunOutcome {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        duration_ms: 600_000,
        reason: Some(KILLED_BY_DEADLINE.to_string()),
    };

    assert_eq!(
        wire::render_run(&outcome),
        format!("no exit code: {KILLED_BY_DEADLINE}\ntook 600000 ms")
    );
}

#[test]
fn the_rendered_text_carries_no_trailing_blank_line() {
    // The text is embedded in a reply that is already one of several lines, and a
    // double newline in the middle of a protocol body is the kind of thing that
    // looks fine until something parses it.
    let outcome = RunOutcome {
        exit_code: Some(0),
        stdout: "output without a newline".to_string(),
        stderr: String::new(),
        duration_ms: 5,
        reason: None,
    };

    let text = wire::render_run(&outcome);
    assert!(!text.ends_with('\n'), "{text:?}");
    assert!(!text.contains("\n\n"), "{text:?}");
}

#[test]
fn the_paths_are_the_ones_the_protocol_is_documented_with() {
    // Constants, asserted so that a typo is a failing test rather than a host and
    // an agent disagreeing about a slash.
    assert_eq!(wire::RUN_PATH, "/run");
    assert_eq!(wire::IDENTITY_PATH, "/ping");
    assert!(wire::RUN_PATH.starts_with('/') && wire::IDENTITY_PATH.starts_with('/'));
}
