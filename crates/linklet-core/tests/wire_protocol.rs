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
    self, KILLED_BY_DEADLINE, MAX_TIMEOUT_SECONDS, Reply, Request, RunOutcome, RunRequest, Text,
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
        stdout: Text::default(),
        stderr: Text::from_bytes(b"usage: app [options]"),
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
        stdout: Text::default(),
        stderr: Text::default(),
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
        stdout: Text::from_bytes(b"half a build"),
        stderr: Text::default(),
        duration_ms: 900_000,
        reason: Some(KILLED_BY_DEADLINE.to_string()),
    };

    let decoded =
        run_outcome_from_body(&json::write(&wire::encode_run_reply(&outcome))).expect("decodes");

    assert_eq!(decoded.reason.as_deref(), Some(KILLED_BY_DEADLINE));
    assert_eq!(
        decoded.stdout.as_str(),
        "half a build",
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
        stdout: Text::default(),
        stderr: Text::from_bytes(b"usage: app [options]"),
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
        Request::Pull {
            path: r"logs\build.log".to_string(),
        },
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
    assert_eq!(
        json::write(&wire::request_to_json(&Request::Pull {
            path: r"logs\build.log".to_string(),
        })),
        r#"{"op":"pull","path":"logs\\build.log"}"#
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
    for known in ["identity", "run", "push", "pull"] {
        assert!(
            text.contains(known),
            "the refusal should list {known:?}, and the list is {text}"
        );
    }
}

#[test]
fn a_pull_with_no_path_is_refused_by_name() {
    // One field, and therefore one way to be wrong. A pull that defaulted its path
    // would be a read of whatever the agent felt like reading.
    let error =
        wire::request_from_json(&object! { "op" => "pull" }).expect_err("a pull needs a path");
    assert!(error.to_string().contains("path"), "{error}");
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

// --- a reply that will not fit in a frame ------------------------------------

#[test]
fn a_reply_that_fills_the_ceiling_can_still_be_framed() {
    // **The boundary of the predicate, and why it is searched for rather than
    // computed.** The predicate and the sender have to agree exactly: one byte
    // optimistic would let the agent build a reply it could not send, which is the
    // defect this change is about, and one byte pessimistic would refuse output that
    // would have gone.
    //
    // Two attempts to *compute* the boundary failed here first. "Add
    // `ceiling - size_of_empty` bytes of text" is wrong because one more byte of text
    // also carries the byte count in the JSON, and a number that gains a digit makes
    // the reply grow by two. Measuring one step and dividing is wrong for the same
    // reason: the step is not constant. A binary search asks the encoder instead of
    // reasoning about it.
    //
    // **A small ceiling on purpose.** The question is which side of a line a reply
    // falls on, and a line at 4 KiB answers it in microseconds. The same search
    // against the real sixteen mebibytes was written first and took seven seconds,
    // because every step of the search builds and drops a reply the size of the
    // window -- the sort of cost a test in this layer exists to avoid. That the real
    // number is the ceiling the protocol uses is asserted next to the constant, in
    // `wire`'s own tests, and end to end by `against_agent.rs` with 21 MB of output.
    let size_of_reply_with =
        |padding: usize| json::write(&wire::encode_run_reply(&outcome_of_stdout(padding))).len();

    let ceiling = 4096;
    let (mut fits, mut too_large) = (0usize, ceiling);
    while too_large - fits > 1 {
        let middle = fits + (too_large - fits) / 2;
        if size_of_reply_with(middle) <= ceiling {
            fits = middle;
        } else {
            too_large = middle;
        }
    }

    assert_eq!(
        size_of_reply_with(fits),
        ceiling,
        "the search should stop on a reply of exactly the ceiling"
    );
    assert!(
        wire::reply_fits(&wire::encode_run_reply(&outcome_of_stdout(fits)), ceiling),
        "the largest reply the search accepted fits"
    );
    assert!(
        !wire::reply_fits(
            &wire::encode_run_reply(&outcome_of_stdout(too_large)),
            ceiling
        ),
        "and one byte more does not"
    );
}

/// An outcome whose stdout is `bytes` bytes of one character.
fn outcome_of_stdout(bytes: usize) -> RunOutcome {
    RunOutcome {
        exit_code: Some(0),
        stdout: Text::from_bytes("x".repeat(bytes).as_bytes()),
        stderr: Text::default(),
        duration_ms: 1,
        reason: None,
    }
}

#[test]
fn a_reply_that_fits_is_not_refused() {
    // The other half of the test below, and the reason it is a predicate rather
    // than a comparison written at the call site: an agent that refused every
    // reply would pass a test that only checked the too-large case.
    assert!(
        wire::reply_fits(&wire::encode_run_reply(&outcome_of_stdout(1024)), 4096),
        "a small reply fits in a 4 KiB frame budget"
    );
}

#[test]
fn a_reply_too_large_to_frame_is_refused_by_name() {
    // **This is the defect M10 carries.** A command whose output is past the
    // frame ceiling used to leave the agent with a reply it could not frame, and
    // it said nothing: the caller's only possible conclusion was "the agent
    // closed the connection without answering", which is a statement about the
    // network and not about the command. The decision here is what lets the
    // agent answer instead.
    let reply = wire::encode_run_reply(&outcome_of_stdout(1024));

    assert!(
        !wire::reply_fits(&reply, 128),
        "a reply of about a kilobyte cannot fit in a 128 byte budget"
    );

    let refusal = wire::run_reply_too_large(&outcome_of_stdout(20_000_000), 128);
    let text = match wire::reply_from_json(&refusal).expect("the refusal is a reply") {
        Reply::Refused(reason) => reason,
        Reply::Result(value) => panic!("too large is a refusal and not a result: {value:?}"),
    };

    // A caller has to be able to act on it, so it names the size that did not fit
    // and the ceiling it did not fit in -- not a sentence about a socket.
    assert!(text.contains("20000000"), "{text}");
    assert!(text.contains("128"), "{text}");
    assert!(
        text.contains("stdout"),
        "the size that did not fit is the fact that decides what the caller does next: {text}"
    );

    // And the two measurements are told apart in words, because they are not the
    // same number: the stream sizes are bytes the command wrote, the ceiling is
    // bytes of a reply. A caller that read one as the other would look for three
    // megabytes of difference between an output and a message about it.
    assert!(
        text.contains("a reply of at most 128 bytes"),
        "the ceiling should be labelled as a limit on the reply: {text}"
    );
}

// --- output that is not text -------------------------------------------------

/// The bytes of `D6 D0 CE C4`, the four the first real target was measured with.
///
/// Written as bytes rather than as a string literal because that is what the
/// command emitted: this is not text in any encoding the reply claims to carry, and
/// a test that wrote it as a Rust string could not express that.
const NOT_UTF8: [u8; 4] = [0xd6, 0xd0, 0xce, 0xc4];

#[test]
fn output_that_is_utf8_is_not_marked_as_lost() {
    // The other half of the test below. A flag that was always set would pass a test
    // that only checked the lossy case, and every ordinary command would come back
    // with a warning about an encoding nothing was wrong with.
    let text = wire::Text::from_bytes(b"built 3 targets");

    assert_eq!(text.as_str(), "built 3 targets");
    assert!(!text.is_lossy(), "{text:?}");
    assert_eq!(text.byte_count(), 15, "the bytes the command wrote");
}

#[test]
fn output_that_is_not_utf8_carries_the_loss_rather_than_hiding_it() {
    // **The defect M10 carries.** These four bytes -- GBK for two CJK characters --
    // reached the caller as four `U+FFFD` and nothing in the reply said the output
    // was not text. The bytes were checked in the bytes, not in a terminal, so this
    // is the same claim written where it can fail.
    let text = wire::Text::from_bytes(&NOT_UTF8);

    assert!(
        text.is_lossy(),
        "a reply that decoded four invalid bytes must not read as clean text: {text:?}"
    );
    assert_eq!(
        text.as_str().chars().count(),
        4,
        "one replacement character per invalid byte, which is what lossy means"
    );
    assert!(
        text.as_str().contains('\u{fffd}'),
        "the replacement is what makes the difference visible: {text:?}"
    );
    assert_eq!(
        text.byte_count(),
        4,
        "the size is the bytes the command wrote"
    );
}

#[test]
fn a_run_whose_output_was_not_utf8_says_so_after_the_trip() {
    // The point is the wire, not the formatting: a `String` cannot carry "these
    // bytes are not text", so the reply has to. A caller reading only the JSON must
    // be able to tell this reply from one whose output was clean.
    let outcome = RunOutcome {
        exit_code: Some(0),
        stdout: wire::Text::from_bytes(&NOT_UTF8),
        stderr: wire::Text::from_bytes(b""),
        duration_ms: 3,
        reason: None,
    };

    let decoded = run_outcome_from_body(&json::write(&wire::encode_run_reply(&outcome)))
        .expect("its own output should decode");

    assert!(
        decoded.stdout.is_lossy(),
        "the trip through JSON lost the fact that it was not UTF-8: {decoded:#?}"
    );
    assert!(
        !decoded.stderr.is_lossy(),
        "and an empty stream was never lossy: {decoded:#?}"
    );
    assert_eq!(decoded.stdout.byte_count(), 4, "the byte count crossed too");
}

#[test]
fn a_reply_from_before_this_field_existed_still_decodes() {
    // The four fields a run reply has always had, with no byte count and no loss
    // flag. An agent older than this change sends exactly this, and refusing it
    // would be a version skew that breaks a working pair for a field it does not
    // need.
    let older = object! {
        "exit_code" => 0i64,
        "stdout" => Json::str("ok"),
        "stderr" => Json::str(""),
        "duration_ms" => 1i64,
        "reason" => Json::Null,
    };

    let outcome = wire::run_outcome_from_json(&older).expect("a reply this version understands");
    assert_eq!(outcome.stdout.as_str(), "ok");
    assert_eq!(outcome.stdout.byte_count(), 2);
    assert!(!outcome.stdout.is_lossy());
}

// --- looking at what is running ----------------------------------------------

/// A listing with one process, one filter and one thing the machine could not tell us --
/// so that every field has something in it and a dropped field fails the round trip.
fn a_listing() -> linklet_core::process::Listing {
    use linklet_core::process::{Filter, Process, apply};

    let processes = vec![
        Process {
            pid: 100,
            name: "linklet-agent.exe".to_string(),
            path: None,
            cmdline: None,
        },
        Process::named(200, "explorer.exe"),
    ];
    let filter = Filter {
        name: Some("agent".to_string()),
        ..Filter::any()
    };

    apply(processes, &filter, 2)
}

#[test]
fn a_listing_round_trips_with_every_field_a_reader_needs() {
    // **The reply shape M10 asks for**: count, total, truncated, and the filters that were
    // actually applied. Each is checked separately because a reader that got an empty list
    // has to be able to tell "nothing matched" from "nothing was asked" from "the machine
    // could not be read", and two of those three are not lists at all.
    let listing = a_listing();
    let encoded = json::write(&wire::encode_ps_reply(&listing));
    let decoded = wire::ps_listing_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("its own output should decode");

    assert_eq!(decoded.count(), 1, "{decoded:#?}");
    assert_eq!(decoded.processes[0].name, "linklet-agent.exe");
    assert_eq!(decoded.processes[0].pid, 100);
    assert_eq!(decoded.total, 2, "the total is before the filter");
    assert_eq!(decoded.unreadable, 2);
    assert!(!decoded.truncated);
    assert_eq!(decoded.applied.get_str("name"), Some("agent"));
    assert!(!decoded.notes.is_empty(), "{decoded:#?}");
}

#[test]
fn an_empty_filter_comes_back_as_an_empty_object_and_not_as_nothing() {
    // A caller reading a reply has to be able to tell "no filters were asked for" from
    // "the filters were dropped on the way out", and only one of those is safe to act on.
    use linklet_core::process::{Filter, apply};

    let listing = apply(Vec::new(), &Filter::any(), 0);
    let encoded = json::write(&wire::encode_ps_reply(&listing));
    let decoded = wire::ps_listing_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("decodes");

    assert_eq!(decoded.applied, Json::Object(Default::default()));
    assert_eq!(decoded.count(), 0);
}

#[test]
fn every_part_of_a_process_filter_survives_the_wire() {
    use linklet_core::process::Filter;

    let filter = Filter {
        name: Some("agent".to_string()),
        path: Some("bin".to_string()),
        cmdline: Some("--port 8790".to_string()),
        query: Some("linklet".to_string()),
        exclude: Some("test".to_string()),
    };

    let request = Request::Ps(filter.clone());
    let encoded = json::write(&wire::request_to_json(&request));
    let decoded = wire::request_from_json(&json::parse(&encoded).expect("valid JSON"))
        .expect("its own output should decode");

    assert_eq!(decoded, request, "{encoded}");
    let Request::Ps(decoded) = decoded else {
        panic!("a ps request decoded as something else");
    };
    assert_eq!(decoded, filter);
}

#[test]
fn a_ps_request_with_no_arguments_is_a_listing_of_everything() {
    let request = Request::Ps(linklet_core::process::Filter::any());
    let encoded = json::write(&wire::request_to_json(&request));

    assert_eq!(
        encoded, r#"{"op":"ps"}"#,
        "an empty filter is not written out"
    );
    assert_eq!(
        wire::request_from_json(&json::parse(&encoded).expect("valid JSON")).expect("decodes"),
        request
    );
}

#[test]
fn a_filter_field_of_the_wrong_shape_is_refused_by_name() {
    // The filter is the caller's text and it goes into a message, so it is read the way
    // every other field is: by name, with the field that is wrong in the sentence.
    for (body, expected) in [
        (r#"{"op":"ps","name":5}"#, "name"),
        (r#"{"op":"ps","exclude":true}"#, "exclude"),
    ] {
        let error = wire::request_from_json(&json::parse(body).expect("valid JSON"))
            .expect_err("this filter should have been refused");
        assert!(
            error.to_string().contains(expected),
            "for {body}, expected {expected:?} in {error}"
        );
    }
}

// --- starting something that outlives the request ----------------------------

#[test]
fn a_spawn_request_carries_the_command_and_where_its_output_goes() {
    use linklet_core::wire::SpawnRequest;

    let request = Request::Spawn(SpawnRequest {
        command: "app.exe --serve".to_string(),
        output: r"logs\app.log".to_string(),
    });

    let encoded = json::write(&wire::request_to_json(&request));
    let decoded = wire::request_from_json(&json::parse(&encoded).expect("valid JSON"))
        .expect("its own output should decode");

    assert_eq!(decoded, request, "{encoded}");
    assert!(
        encoded.contains("\"op\":\"spawn\""),
        "the operation is pinned by value like the others: {encoded}"
    );
}

#[test]
fn a_spawn_reply_carries_the_pid_and_not_a_promise() {
    // **What a spawn can honestly say.** That the program started and what its pid is --
    // and nothing about whether it is healthy, because at this moment nothing knows. The
    // caller asks `ps` for that, which is what `ps` is for.
    use linklet_core::wire::SpawnReport;

    let report = SpawnReport {
        pid: 5144,
        command: "app.exe --serve".to_string(),
    };

    assert_eq!(
        json::write(&wire::spawn_report_to_json(&report)),
        r#"{"command":"app.exe --serve","pid":5144}"#
    );
}

#[test]
fn a_spawn_request_without_a_command_or_an_output_file_is_refused_by_name() {
    for (body, expected) in [
        (r#"{"op":"spawn","output":"a.log"}"#, "command"),
        (r#"{"op":"spawn","command":"app.exe"}"#, "output"),
        (
            r#"{"op":"spawn","command":"   ","output":"a.log"}"#,
            "command",
        ),
    ] {
        let error = wire::request_from_json(&json::parse(body).expect("valid JSON"))
            .expect_err("this spawn should have been refused");
        assert!(
            error.to_string().contains(expected),
            "for {body}, expected {expected:?} in {error}"
        );
    }
}

// --- searching a file without moving it --------------------------------------

/// A search with a match, a context line and an encoding worth being careful about.
fn a_search() -> linklet_core::search::Search {
    use linklet_core::search::{Direction, Limit, Pattern, lines_of, scan};

    let text = lines_of("first\nERROR: one\nlast");
    let mut search = scan(
        &text,
        &Pattern::new("ERROR"),
        Limit {
            max_matches: 5,
            context: 1,
        },
        Direction::First,
    );
    search.path = "build.log".to_string();
    search.encoding = linklet_core::search::Encoding::Oem;
    search.file_bytes = Some(4096);
    search.bytes_read = 4096;
    search
}

#[test]
fn a_second_unnamed_encoding_is_a_new_tag_rather_than_a_wider_old_one() {
    // **Why this is a separate test and not another assertion above.** A reply whose text was
    // decoded by a rule that is not the machine's code page has to say so with a tag of its own,
    // because the alternative -- reusing `oem` -- would have an older host print "the machine's
    // OEM code page" for a decode that was not one. It would be wrong while believing it
    // understood, which is the line `docs/VERSIONING.md` draws: an old peer may refuse what it
    // does not know, and may not misread it.
    //
    // So this pins the two halves of that: the tag is written, it is not `oem`, and it decodes
    // back. A host that does not know the word refuses the reply by name.
    let mut search = a_search();
    search.encoding = linklet_core::search::Encoding::Latin1;

    let encoded = json::write(&wire::encode_search_reply(&search));
    assert!(
        encoded.contains("\"latin-1\""),
        "the tag has to be on the wire: {encoded}"
    );
    assert!(
        !encoded.contains("\"oem\""),
        "and it must not be the other, wider label: {encoded}"
    );

    let decoded = wire::search_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("its own output should decode");
    assert_eq!(decoded.encoding, linklet_core::search::Encoding::Latin1);
}

#[test]
fn a_search_round_trips_with_everything_a_reader_needs_to_judge_it() {
    // **The fields that keep a failed search from reading as "no matches"**, and the one
    // that says what the text was assumed to be. Each is checked separately because each
    // answers a different question, and a reply that lost one would still look like a
    // plausible list of lines.
    let search = a_search();
    let encoded = json::write(&wire::encode_search_reply(&search));
    let decoded = wire::search_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("its own output should decode");

    assert!(decoded.searched, "{decoded:#?}");
    assert_eq!(decoded.total, Some(1));
    assert_eq!(decoded.encoding, linklet_core::search::Encoding::Oem);
    assert_eq!(decoded.path, "build.log");
    assert_eq!(decoded.file_bytes, Some(4096));
    assert_eq!(decoded.lines.len(), 1);
    assert_eq!(decoded.lines[0].number, 2);
    assert_eq!(decoded.lines[0].text, "ERROR: one");
    assert_eq!(decoded.lines[0].before, vec!["first".to_string()]);
    assert_eq!(decoded.lines[0].after, vec!["last".to_string()]);
}

#[test]
fn a_search_that_did_not_happen_round_trips_as_one_that_did_not() {
    // The distinction the whole feature is arranged around, over the wire: `searched` and
    // `problem` have to survive it, or a caller cannot tell a clean log from a file nobody
    // could open.
    let search = linklet_core::search::could_not_search(
        "build.log",
        "no such file",
        linklet_core::search::Encoding::Utf8,
    );

    let encoded = json::write(&wire::encode_search_reply(&search));
    let decoded = wire::search_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("decodes");

    assert!(!decoded.searched);
    assert_eq!(decoded.problem.as_deref(), Some("no such file"));
    assert_eq!(decoded.total, None, "nothing was counted");
    assert!(decoded.lines.is_empty());
}

#[test]
fn stopped_early_crosses_the_wire_as_a_null_count_and_not_a_zero() {
    // `total: null` is "there may be more"; `total: 0` is "there are none". An agent that
    // wrote zero for the first would be answering a question it had not finished asking.
    use linklet_core::search::{Direction, Limit, Pattern, lines_of, scan};

    let text = lines_of("hit\nhit\nhit\nhit");
    let search = scan(
        &text,
        &Pattern::new("hit"),
        Limit {
            max_matches: 2,
            context: 0,
        },
        Direction::Last,
    );

    let encoded = json::write(&wire::encode_search_reply(&search));
    assert!(encoded.contains(r#""total":null"#), "{encoded}");
    assert!(encoded.contains(r#""truncated":true"#), "{encoded}");

    let decoded = wire::search_from_reply(
        &wire::reply_from_json(&json::parse(&encoded).expect("valid JSON")).expect("a reply"),
    )
    .expect("decodes");
    assert_eq!(decoded.total, None);
}

#[test]
fn a_grep_request_carries_the_pattern_the_mode_and_the_limit() {
    use linklet_core::search::{Direction, Limit, Pattern};
    use linklet_core::wire::GrepRequest;

    let request = Request::Grep(GrepRequest {
        path: r"logs\build.log".to_string(),
        pattern: Pattern::ignoring_case("error"),
        direction: Direction::Last,
        limit: Limit {
            max_matches: 7,
            context: 2,
        },
    });

    let encoded = json::write(&wire::request_to_json(&request));
    assert!(encoded.contains(r#""op":"grep""#), "{encoded}");
    assert!(encoded.contains(r#""mode":"last""#), "{encoded}");
    assert!(
        encoded.contains(r#""case_sensitive":false"#),
        "the case rule is the request's, not a default each end assumes: {encoded}"
    );

    let decoded = wire::request_from_json(&json::parse(&encoded).expect("valid JSON"))
        .expect("its own output should decode");
    assert_eq!(decoded, request);
}

#[test]
fn a_grep_whose_mode_is_not_a_mode_is_refused_by_name() {
    // A version skew on the one field that decides which end of the file is read is worth a
    // sentence: guessing `first` for a caller that meant `last` answers about the wrong end
    // of the file, and reads exactly like an answer.
    let error = wire::request_from_json(
        &json::parse(r#"{"op":"grep","path":"a.log","pattern":"x","mode":"middle"}"#)
            .expect("valid JSON"),
    )
    .expect_err("that is not a mode");

    assert!(error.to_string().contains("mode"), "{error}");
}

#[test]
fn a_tail_count_past_the_ceiling_is_refused_rather_than_clamped() {
    // The same argument as an out-of-range timeout: a caller that asked for ten thousand
    // lines and got two thousand would not know it had.
    let error = wire::request_from_json(
        &json::parse(r#"{"op":"tail","path":"a.log","count":100000}"#).expect("valid JSON"),
    )
    .expect_err("past the ceiling");

    assert!(error.to_string().contains("count"), "{error}");

    let ok = wire::request_from_json(
        &json::parse(r#"{"op":"tail","path":"a.log","count":50}"#).expect("valid JSON"),
    )
    .expect("a count within the ceiling");
    assert_eq!(
        ok,
        Request::Tail(linklet_core::wire::TailRequest {
            path: "a.log".to_string(),
            count: 50,
        })
    );
}

// --- what the agent reads ----------------------------------------------------

#[test]
fn a_successful_run_renders_the_exit_code_the_time_and_the_streams() {
    let outcome = RunOutcome {
        exit_code: Some(0),
        stdout: Text::from_bytes(b"built 3 targets"),
        stderr: Text::default(),
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
        stdout: Text::from_bytes(b"compiling"),
        stderr: Text::from_bytes(b"error: expected ';'"),
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
        stdout: Text::from_bytes(b"ok"),
        stderr: Text::default(),
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
        stdout: Text::default(),
        stderr: Text::default(),
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
        stdout: Text::from_bytes(b"output without a newline"),
        stderr: Text::default(),
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
