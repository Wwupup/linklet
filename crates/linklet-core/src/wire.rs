//! What the host asks an agent, and what an agent answers.
//!
//! The two halves of this project talk over HTTP on a LAN. This module is the
//! only place that decides what those messages are -- the agent's server, the
//! host's client and the tests below all read the same definitions, so a change
//! to the protocol is a change to one file rather than to two that must agree.
//!
//! # The distinction this module exists to protect
//!
//! **A command that ran and failed is not a request that failed.** `exit_code: 1`
//! from a program that did its job and reported a problem is a successful call
//! carrying bad news; a command that never started is a call that could not be
//! made. Folding the two into one error field is the mistake that makes an agent
//! retry a target that already answered, so they are separate types here:
//! [`RunOutcome`] and [`WireError`].
//!
//! # Why hand-written JSON and not a framework
//!
//! The same reason as everywhere else in this project: the core may not link
//! against anything. What it gets in exchange is that every message shape is a
//! value this crate can build and compare in a test, with no server and no
//! socket.

use crate::json::{self, Json};
use crate::object;
use std::collections::BTreeMap;

/// The path the agent answers on for a command.
///
/// A constant rather than a literal at each call site, so the host and the agent
/// cannot drift into two strings that differ by a slash.
pub const RUN_PATH: &str = "/run";

/// The path the agent answers on for its identity.
///
/// The cheapest question in the protocol and the one a caller asks first: is
/// there an agent here, and which one. It is a `GET` with no arguments so that a
/// probe can use it without deciding anything.
pub const IDENTITY_PATH: &str = "/ping";

/// The path the two sides exchange public keys on.
///
/// A separate path rather than a header on `/run`, because it is a different kind
/// of message: it carries no command, needs no token (there is nothing in it but a
/// public key), and **must be answered before anything can be sealed**. Keeping it
/// separate is what lets the agent refuse a `/run` that arrived with no handshake,
/// instead of having to guess whether an absent header was a mistake or an old
/// client.
pub const HELLO_PATH: &str = "/handshake";

/// Encodes bytes as lowercase hexadecimal.
///
/// Hex rather than base64 because a public key is 32 bytes and this is the encoding
/// a reader can check by eye against a captured message. That matters more than the
/// handful of extra characters for a value nobody types by hand.
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Decodes hexadecimal, either case.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the text is not an even number of hex digits.
/// The caller turns that into the message a peer sees, so it says what was wrong
/// with the text and not where it came from.
pub fn from_hex(text: &str) -> Result<Vec<u8>, WireError> {
    if !text.len().is_multiple_of(2) {
        return Err(WireError::BadRequest(format!(
            "a hex value must have an even number of digits, and this has {}",
            text.len()
        )));
    }

    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for pair in bytes.chunks_exact(2) {
        let digit = |byte: u8| -> Result<u8, WireError> {
            match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                b'A'..=b'F' => Ok(byte - b'A' + 10),
                other => Err(WireError::BadRequest(format!(
                    "{:?} is not a hex digit",
                    other as char
                ))),
            }
        };
        out.push((digit(pair[0])? << 4) | digit(pair[1])?);
    }
    Ok(out)
}

/// The one field both handshake messages carry.
///
/// The same shape in both directions, which is not laziness: the two messages are
/// the same thing -- "here is my public key for this handshake" -- and giving them
/// different field names would be a chance for the two sides to disagree about
/// which is which.
pub fn handshake_to_json(public: &[u8]) -> Json {
    object! { "ephemeral_public" => to_hex(public) }
}

/// Reads a handshake message.
///
/// Returns the bytes and not an [`crate::channel::EphemeralPublic`], because
/// whether they are a usable public key is a question for the curve and this crate
/// does not do arithmetic. The length check that belongs to the *wire* -- the field
/// exists and is text -- is made here; the one that belongs to the protocol is made
/// by whoever builds the key.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the field is absent, not text, or not hex.
pub fn handshake_public_from_json(value: &Json) -> Result<Vec<u8>, WireError> {
    let text = value.get_str("ephemeral_public").ok_or_else(|| {
        WireError::BadRequest("a handshake needs an ephemeral_public hex string".to_string())
    })?;
    from_hex(text)
}

/// A command the host wants run on a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    /// The command line.
    pub command: String,
    /// Seconds the agent may spend before it kills the process.
    pub timeout_seconds: u64,
}

/// What the agent observed.
///
/// Every field is something the agent saw rather than something it concluded.
/// In particular there is no `success` flag: an agent deciding what a caller
/// should think about an exit code is an agent that has to guess, and the caller
/// has the exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// The process's exit code, or `None` when it was killed or never ran.
    pub exit_code: Option<i32>,
    /// What it wrote to standard output.
    pub stdout: String,
    /// What it wrote to standard error.
    pub stderr: String,
    /// Milliseconds from spawn to exit.
    pub duration_ms: u64,
    /// Why there is no exit code, when there is not one.
    ///
    /// `None` alongside a present exit code. `Some` alongside an absent one,
    /// except for a process killed by its deadline, where the reason is
    /// [`KILLED_BY_DEADLINE`] -- stated as a constant so the host is not
    /// pattern-matching on prose.
    pub reason: Option<String>,
}

/// The reason recorded for a process the agent stopped at its deadline.
///
/// A distinguished value rather than a sentence, because the host acts on it:
/// "timed out" means the command may still have been doing something useful, and
/// "failed to start" means nothing happened at all.
pub const KILLED_BY_DEADLINE: &str = "killed by the deadline";

/// Why a request could not be answered at all.
///
/// Separate from [`RunOutcome`] on purpose -- see the module documentation. An
/// [`WireError`] means the host learned nothing about the command; an outcome
/// with a non-zero exit code means it learned everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The body was not the shape this protocol defines.
    BadRequest(String),
    /// The path is not one the agent serves.
    NoSuchPath(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(problem) => write!(f, "bad request: {problem}"),
            Self::NoSuchPath(path) => write!(f, "no such path: {path}"),
        }
    }
}

impl std::error::Error for WireError {}

/// The largest timeout the protocol will carry, in seconds.
///
/// An agent that accepts an unbounded timeout from the network has been handed a
/// way to be occupied forever. Ten minutes is longer than any build step this
/// tool is for and short enough that a mistake ends.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// The request as JSON.
pub fn run_request_to_json(request: &RunRequest) -> Json {
    object! {
        "command" => request.command,
        "timeout_seconds" => request.timeout_seconds as i64,
    }
}

/// A request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field, because the caller is a program
/// on the other end of a socket and "invalid body" gives whoever is debugging it
/// nothing to look at.
pub fn run_request_from_json(value: &Json) -> Result<RunRequest, WireError> {
    let command = value
        .get_str("command")
        .ok_or_else(|| WireError::BadRequest("command: missing or not a string".to_string()))?
        .to_string();

    if command.trim().is_empty() {
        return Err(WireError::BadRequest("command: empty".to_string()));
    }

    let timeout_seconds = match value.get("timeout_seconds") {
        None => {
            return Err(WireError::BadRequest(
                "timeout_seconds: missing".to_string(),
            ));
        }
        Some(Json::Int(seconds)) if *seconds >= 1 && (*seconds as u64) <= MAX_TIMEOUT_SECONDS => {
            *seconds as u64
        }
        Some(Json::Int(seconds)) => {
            return Err(WireError::BadRequest(format!(
                "timeout_seconds: {seconds} is outside 1..={MAX_TIMEOUT_SECONDS}"
            )));
        }
        Some(_) => {
            return Err(WireError::BadRequest(
                "timeout_seconds: not a number".to_string(),
            ));
        }
    };

    Ok(RunRequest {
        command,
        timeout_seconds,
    })
}

/// The outcome as JSON.
pub fn run_outcome_to_json(outcome: &RunOutcome) -> Json {
    let mut entries = BTreeMap::new();
    entries.insert(
        "exit_code".to_string(),
        match outcome.exit_code {
            Some(code) => Json::Int(i64::from(code)),
            None => Json::Null,
        },
    );
    entries.insert("stdout".to_string(), Json::str(&outcome.stdout));
    entries.insert("stderr".to_string(), Json::str(&outcome.stderr));
    entries.insert(
        "duration_ms".to_string(),
        Json::Int(outcome.duration_ms as i64),
    );
    entries.insert(
        "reason".to_string(),
        match &outcome.reason {
            Some(reason) => Json::str(reason),
            None => Json::Null,
        },
    );
    Json::Object(entries)
}

/// An outcome from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] for anything that is not the shape above. A missing
/// `exit_code` *field* is an error; a present `null` is a `None`, because those
/// are different messages and only one of them means "killed or never started".
pub fn run_outcome_from_json(value: &Json) -> Result<RunOutcome, WireError> {
    let exit_code = match value.get("exit_code") {
        None => {
            return Err(WireError::BadRequest("exit_code: missing".to_string()));
        }
        Some(Json::Null) => None,
        Some(Json::Int(code)) => Some(*code as i32),
        Some(_) => {
            return Err(WireError::BadRequest("exit_code: not a number".to_string()));
        }
    };

    let text = |field: &str| -> Result<String, WireError> {
        value
            .get_str(field)
            .map(str::to_string)
            .ok_or_else(|| WireError::BadRequest(format!("{field}: missing or not a string")))
    };

    let stdout = text("stdout")?;
    let stderr = text("stderr")?;

    let duration_ms = match value.get("duration_ms") {
        Some(Json::Int(ms)) if *ms >= 0 => *ms as u64,
        _ => {
            return Err(WireError::BadRequest(
                "duration_ms: missing or negative".to_string(),
            ));
        }
    };

    let reason = match value.get("reason") {
        None => return Err(WireError::BadRequest("reason: missing".to_string())),
        Some(Json::Null) => None,
        Some(Json::Str(text)) => Some(text.clone()),
        Some(_) => {
            return Err(WireError::BadRequest("reason: not a string".to_string()));
        }
    };

    Ok(RunOutcome {
        exit_code,
        stdout,
        stderr,
        duration_ms,
        reason,
    })
}

/// The error as JSON, in the shape the agent answers with.
pub fn wire_error_to_json(error: &WireError) -> Json {
    match error {
        WireError::BadRequest(problem) => object! { "error" => problem },
        WireError::NoSuchPath(path) => object! { "error" => format!("no such path: {path}") },
    }
}

/// Renders an outcome as the text an agent reads.
///
/// Fixed shape, and the first thing on every line is a fact rather than a
/// sentence about one: the exit code, then the streams. `is_error` is *not*
/// decided here -- a reply that says "the command failed" is the caller's
/// business, and this function has no way to know whether a non-zero exit
/// matters for the command that was run.
pub fn render_run(outcome: &RunOutcome) -> String {
    let mut out = String::new();

    match outcome.exit_code {
        Some(code) => out.push_str(&format!("exit {code}\n")),
        None => out.push_str(&format!(
            "no exit code: {}\n",
            outcome.reason.as_deref().unwrap_or("unknown")
        )),
    }

    out.push_str(&format!("took {} ms\n", outcome.duration_ms));

    if !outcome.stdout.is_empty() {
        out.push_str("stdout:\n");
        out.push_str(&outcome.stdout);
        if !outcome.stdout.ends_with('\n') {
            out.push('\n');
        }
    }

    if !outcome.stderr.is_empty() {
        out.push_str("stderr:\n");
        out.push_str(&outcome.stderr);
        if !outcome.stderr.ends_with('\n') {
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

/// The whole of a run, decoded from the agent's reply body.
///
/// The host's side of the contract, in one call, so that a test can drive the
/// pair without a socket.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the body is not JSON or not the shape above.
pub fn decode_run_reply(body: &str) -> Result<RunOutcome, WireError> {
    let value = json::parse(body).map_err(|e| WireError::BadRequest(e.to_string()))?;
    run_outcome_from_json(&value)
}

/// Encodes an outcome as the agent's reply body.
pub fn encode_run_reply(outcome: &RunOutcome) -> String {
    json::write(&run_outcome_to_json(outcome))
}
