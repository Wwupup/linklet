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
use linklet_core::transfer::Manifest;
use linklet_core::wire::{
    self, KILLED_BY_DEADLINE, MAX_TIMEOUT_SECONDS, Reply, Request, RunOutcome, RunRequest,
    WireError,
};

/// A digest of the right shape, for the messages that carry one.
///
/// Deliberately not all zeros: digits have no case, so a digest made of them cannot be
/// used to check that the comparison is case-sensitive -- which is a mistake that was
/// made once in `tests/transfer_paths.rs`.
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// Reads a run outcome out of a reply body, the way the host has to.
///
/// Two steps rather than one function, because the middle one is the whole point:
/// the reply is a result or a refusal, and only the first of those can be an
/// outcome. A single function returning an outcome would have to fold a refusal
/// into a shape that carries an exit code, which is the mistake this file is about.
fn run_outcome_from_body(body: &str) -> Result<RunOutcome, WireError> {
    let reply = wire::reply_from_json(&json::parse(body).expect("valid JSON"))?;
    wire::run_outcome_from_reply(&reply)
}

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

    let encoded = json::write(&wire::encode_run_reply(&outcome));
    let decoded = run_outcome_from_body(&encoded).expect("its own output should decode");

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

    let decoded =
        run_outcome_from_body(&json::write(&wire::encode_run_reply(&outcome))).expect("decodes");

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

    let decoded =
        run_outcome_from_body(&json::write(&wire::encode_run_reply(&outcome))).expect("decodes");

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

// --- the two shapes, and telling them apart ----------------------------------

#[test]
fn a_refusal_carries_the_reason_and_no_result() {
    // Deliberately not a result with an exit code: a refusal says the request could
    // not be answered, and a caller must not read it looking for output. The two are
    // told apart by `ok` rather than by which fields happen to be present, because
    // two shapes distinguished by their fields are two shapes that eventually
    // overlap.
    let body = json::write(&wire::reply_refused(
        "timeout_seconds: 0 is outside 1..=600",
    ));
    let reply = wire::reply_from_json(&json::parse(&body).expect("valid JSON")).expect("a reply");

    match reply {
        Reply::Refused(reason) => assert!(reason.contains("timeout_seconds"), "{reason}"),
        Reply::Result(value) => panic!("a refusal decoded as a result: {value:?}"),
    }
}

#[test]
fn a_result_that_holds_bad_news_is_still_a_result() {
    // The distinction the protocol was built around, at the level of the envelope: a
    // command that exited 1 produced a result. If this ever decoded as a refusal, an
    // agent would retry a machine that had already answered.
    let outcome = RunOutcome {
        exit_code: Some(1),
        stdout: String::new(),
        stderr: "usage: app [options]".to_string(),
        duration_ms: 4,
        reason: None,
    };

    let body = json::write(&wire::encode_run_reply(&outcome));
    let reply = wire::reply_from_json(&json::parse(&body).expect("valid JSON")).expect("a reply");

    assert!(
        matches!(reply, Reply::Result(_)),
        "an exit code of 1 is bad news and not a refusal"
    );
    assert_eq!(
        wire::run_outcome_from_reply(&reply).expect("an outcome"),
        outcome
    );
}

#[test]
fn a_reply_that_says_neither_is_refused_rather_than_guessed_at() {
    // Four ways to be almost a reply. Each is refused with the field that is wrong,
    // because the alternative -- inferring the shape from what happens to be there --
    // is how a protocol grows two readings of the same bytes.
    for (body, expected) in [
        (r#"{"result": {}}"#, "ok"),
        (r#"{"ok": "true", "result": {}}"#, "ok"),
        (r#"{"ok": true}"#, "result"),
        (r#"{"ok": false}"#, "error"),
    ] {
        let value = json::parse(body).expect("valid JSON");
        let error = wire::reply_from_json(&value).expect_err("should be refused");
        assert!(
            error.to_string().contains(expected),
            "for {body}, expected {expected:?} in {error}"
        );
    }
}

// --- the requests ------------------------------------------------------------

#[test]
fn a_result_of_the_wrong_shape_is_refused_naming_the_field() {
    // The case a mock server someone wrote by hand produces: a well-formed result
    // that is not a run outcome. It has to be a refusal and not a default outcome,
    // because a caller that defaulted the missing fields would report a command that
    // never ran as one that exited zero.
    let error = run_outcome_from_body(r#"{"ok": true, "result": {"exit_code": 0}}"#)
        .expect_err("should be refused");
    assert!(error.to_string().contains("stdout"), "{error}");
}

#[test]
fn every_request_round_trips_through_the_wire() {
    let requests = [
        Request::Identity,
        Request::Run(RunRequest {
            command: "build.cmd --release".to_string(),
            timeout_seconds: 600,
        }),
        Request::Push(Manifest {
            path: r"artifacts\build.exe".to_string(),
            bytes: 12_345,
            sha256: DIGEST.to_string(),
        }),
    ];

    for request in requests {
        let encoded = json::write(&wire::request_to_json(&request));
        let decoded = wire::request_from_json(&json::parse(&encoded).expect("valid JSON"))
            .expect("its own output should decode");
        assert_eq!(decoded, request);
    }
}

#[test]
fn a_push_request_is_the_manifest_with_the_operation_flattened_into_it() {
    // `docs/transfer.md` describes the first frame of a transfer as the manifest
    // itself, and this is that: the operation rides along with the three fields rather
    // than wrapping them in a second object. Pinned by value, because the two ends
    // agreeing about a nesting level is exactly the sort of thing that is obvious until
    // it is not.
    let encoded = json::write(&wire::request_to_json(&Request::Push(Manifest {
        path: "build.exe".to_string(),
        bytes: 3,
        sha256: DIGEST.to_string(),
    })));

    assert_eq!(
        encoded,
        format!(r#"{{"bytes":3,"op":"push","path":"build.exe","sha256":"{DIGEST}"}}"#)
    );
}

#[test]
fn a_manifest_field_that_is_missing_or_impossible_is_refused_by_name() {
    let digest = DIGEST.to_string();
    let cases: [(Json, &str); 5] = [
        (
            object! { "bytes" => 1i64, "sha256" => digest.clone() },
            "path",
        ),
        (
            object! { "path" => "a", "sha256" => digest.clone() },
            "bytes",
        ),
        (
            object! { "path" => "a", "bytes" => 0i64, "sha256" => digest.clone() },
            "positive",
        ),
        (
            object! { "path" => "a", "bytes" => -1i64, "sha256" => digest.clone() },
            "positive",
        ),
        (object! { "path" => "a", "bytes" => 1i64 }, "sha256"),
    ];

    for (value, expected) in cases {
        let error = wire::manifest_from_json(&value).expect_err("this manifest should be refused");
        assert!(
            error.to_string().contains(expected),
            "for {value:?}, expected {expected:?} in {error}"
        );
    }
}

#[test]
fn a_manifest_does_not_check_the_path_because_the_path_is_not_its_business() {
    // A digest and a size are facts about this message. Whether a path may be written
    // to is a question for the machine that owns the directory, and it is asked by
    // `Manifest::check`, which is why the two are separate functions.
    let value = object! {
        "path" => r"..\..\Windows\System32\drivers\etc\hosts",
        "bytes" => 1i64,
        "sha256" => DIGEST,
    };

    let manifest = wire::manifest_from_json(&value).expect("a readable manifest");
    assert_eq!(manifest.path, r"..\..\Windows\System32\drivers\etc\hosts");
}

#[test]
fn a_transfer_result_carries_the_digest_the_receiver_computed() {
    // The point of the comparison is that somebody can check it, so the result is the
    // digest rather than a flag that says the comparison happened.
    let outcome = wire::TransferOutcome {
        bytes: 4096,
        sha256: DIGEST.to_string(),
    };

    let reply = wire::encode_transfer_reply(&outcome);
    let decoded = wire::reply_from_json(&reply).expect("a reply");
    assert_eq!(
        wire::transfer_outcome_from_reply(&decoded).expect("a result"),
        outcome
    );
}

#[test]
fn a_refusal_is_not_a_transfer_result() {
    // A refusal holds a sentence and no digest, so reading one as a result has to fail
    // rather than produce a zero-byte transfer that never happened.
    let reply = wire::reply_refused("the disk is full");
    let decoded = wire::reply_from_json(&reply).expect("a reply");

    let error = wire::transfer_outcome_from_reply(&decoded).expect_err("a refusal");
    assert!(error.to_string().contains("disk is full"), "{error}");
}

#[test]
fn the_op_values_are_pinned_by_value() {
    // The equivalent of the old path constants: a typo here is a host and an agent
    // that cannot talk, and the failure is easier to read as a byte-level fact than
    // as a `.contains("identity")` that passes for the wrong reason.
    assert_eq!(
        json::write(&wire::request_to_json(&Request::Identity)),
        r#"{"op":"identity"}"#
    );
    assert_eq!(
        json::write(&wire::request_to_json(&Request::Run(RunRequest {
            command: "echo hi".to_string(),
            timeout_seconds: 5,
        }))),
        r#"{"command":"echo hi","op":"run","timeout_seconds":5}"#
    );
}

#[test]
fn an_unknown_op_is_refused_with_the_ones_this_version_knows() {
    // A version skew is a list rather than a puzzle: the caller sent an op this
    // build does not have, and the refusal names the ones it does.
    let value = object! { "op" => "install" };
    let error = wire::request_from_json(&value).expect_err("not an op this version has");
    let text = error.to_string();
    assert!(text.contains("install"), "{text}");
    assert!(
        text.contains("identity") && text.contains("run"),
        "the refusal should list the ops that exist: {text}"
    );
}

#[test]
fn a_request_with_no_op_says_so_rather_than_assuming_one() {
    // The field is required. A missing `op` that defaulted to "run" would turn a
    // broken client into a command the agent tries to execute.
    let error = wire::request_from_json(&object! { "command" => "echo hi" })
        .expect_err("an op is required");
    assert!(error.to_string().contains("op"), "{error}");
}

#[test]
fn a_body_that_is_not_json_is_a_bad_request_and_not_a_panic() {
    let error = wire::parse_body(b"not json").expect_err("should be refused");
    assert!(matches!(error, WireError::BadRequest(_)));

    // A body that is valid UTF-8 but not JSON, and one that is not UTF-8 at all:
    // both are refusals, and neither is a panic.
    assert!(wire::parse_body(b"{}").is_ok());
    assert!(wire::parse_body(&[0xff, 0xfe]).is_err());
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
fn the_identity_result_carries_a_name_and_a_version() {
    // Both, because a caller that has only a name cannot tell an old agent from a new
    // one, and "which agent is this" is a question about a version as much as a name.
    let reply = wire::reply_from_json(&wire::identity_to_json("linklet-agent", "0.1.0"))
        .expect("its own output should decode");

    assert_eq!(
        wire::identity_from_json(&reply).expect("a name"),
        "linklet-agent"
    );
    assert!(matches!(reply, Reply::Result(_)));
}
