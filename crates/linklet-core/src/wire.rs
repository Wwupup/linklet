//! What the host asks an agent, and what an agent answers.
//!
//! The two halves of this project talk in frames: `crate::frame` decides what a
//! frame is, `linklet-adapters`' connection puts them on a socket, and this module
//! decides what they contain. The agent's server, the host's client and the tests
//! all read these definitions, so a change to the protocol is a change to one file
//! rather than to two that must agree.
//!
//! # Two shapes, and no status code
//!
//! A [`Request`] is what the host asks for; a [`Reply`] is what the agent says back.
//! **A request that could not be answered is a reply like any other**, carrying the
//! reason as a sentence. There is no numeric status and no error shaping to infer:
//! a refusal arrives as a refusal, so a caller never has to decode a transport-level
//! number to find out what happened.
//!
//! That replaced HTTP, and the reason is in `docs/decisions.md` D4. Every message
//! was already sealed, so methods, paths, headers and status codes were vocabulary
//! nobody read, carried by a parser that had to be right about a grammar nobody
//! used.
//!
//! # The distinction this module exists to protect
//!
//! **A command that ran and failed is not a request that failed.** `exit_code: 1`
//! from a program that did its job and reported a problem is a successful call
//! carrying bad news; a command that never started is a call that could not be
//! made. Folding the two into one field is the mistake that makes an agent retry a
//! target that already answered, so they are separate shapes here: a [`Reply`] is
//! either a result or a refusal, and a result that says "the program exited 1" is
//! the first of those.

use crate::json::{self, Json};
use crate::object;
use crate::transfer::Manifest;
use std::collections::BTreeMap;

/// The request that asks which agent is there.
///
/// The cheapest question in the protocol and the one a caller asks first: is there
/// an agent here, and which one. It carries no arguments, so a caller can send it
/// without deciding anything.
const OP_IDENTITY: &str = "identity";

/// The request that runs a command.
const OP_RUN: &str = "run";

/// The request that sends a file to an agent.
const OP_PUSH: &str = "push";

/// Every `op` this version understands, for an error message that lists them.
///
/// A single list rather than a sentence written at each refusal: a caller that sent
/// an `op` this version does not know needs to see the ones it does, and a list that
/// is written twice is a list that disagrees with itself eventually.
const KNOWN_OPS: &str = "identity, run, push";

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

/// Why a message was not the shape this protocol defines.
///
/// One variant, because there is one thing wrong: the bytes arrived and were not a
/// request or a reply. What could not be answered at all is not this -- that is
/// [`Reply::Refused`], and it is a successful piece of protocol carrying bad news.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The body was not the shape this protocol defines.
    BadRequest(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(problem) => write!(f, "bad request: {problem}"),
        }
    }
}

impl std::error::Error for WireError {}

/// A command the host wants run on a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    /// The command line.
    pub command: String,
    /// Seconds the agent may spend before it kills the process.
    pub timeout_seconds: u64,
}

/// What the host asks an agent for.
///
/// The list is short on purpose. Every request is a question someone has, and the
/// three here are "is there an agent", "run this" and "here is a file" -- see
/// `crate::tool` for the same rule applied to the agent-facing surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Which agent this is, and which version.
    Identity,
    /// Run a command.
    Run(RunRequest),
    /// Send a file to the agent.
    ///
    /// **The manifest is the whole of this request**, and what follows it on the
    /// connection is the file, in chunks, with no further request in between. That is
    /// the design `docs/transfer.md` describes: the receiver knows how many chunks are
    /// coming because the size is declared, which is what replaced the message-count
    /// defence a two-message protocol did not need.
    Push(Manifest),
}

/// What the agent answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The request was answered, and this is the answer.
    ///
    /// The payload is free-form JSON because the answer to "which agent is this" and
    /// the answer to "run this" are different shapes, and forcing them into one
    /// would mean a struct with half its fields empty for every caller.
    Result(Json),
    /// The request arrived and could not be answered, and this is why.
    ///
    /// **Not a transport failure.** The message arrived, was understood, and says
    /// no; the caller has a sentence to read instead of a socket error to guess at.
    Refused(String),
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

/// The largest timeout the protocol will carry, in seconds.
///
/// An agent that accepts an unbounded timeout from the network has been handed a
/// way to be occupied forever. Ten minutes is longer than any build step this
/// tool is for and short enough that a mistake ends.
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// A request as JSON.
pub fn request_to_json(request: &Request) -> Json {
    match request {
        Request::Identity => object! { "op" => OP_IDENTITY },
        Request::Run(run) => object! {
            "op" => OP_RUN,
            "command" => run.command,
            "timeout_seconds" => run.timeout_seconds as i64,
        },
        Request::Push(manifest) => {
            let mut entries = manifest_fields(manifest);
            entries.insert("op".to_string(), Json::str(OP_PUSH));
            Json::Object(entries)
        }
    }
}

/// A request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field, because the caller is a program on
/// the other end of a socket and "invalid body" gives whoever is debugging it
/// nothing to look at. An `op` this version does not know is refused with the ones
/// it does, so a version skew is a list rather than a puzzle.
pub fn request_from_json(value: &Json) -> Result<Request, WireError> {
    let op = value.get_str("op").ok_or_else(|| {
        WireError::BadRequest(format!(
            "op: missing or not a string; one of {KNOWN_OPS} is required"
        ))
    })?;

    match op {
        OP_IDENTITY => Ok(Request::Identity),
        // The command's own fields are parsed by the function that owns them, so a
        // run request has one reader rather than two that could disagree.
        OP_RUN => Ok(Request::Run(run_request_from_json(value)?)),
        OP_PUSH => Ok(Request::Push(manifest_from_json(value)?)),
        other => Err(WireError::BadRequest(format!(
            "op: {other:?} is not one of {KNOWN_OPS}"
        ))),
    }
}

/// The three fields of a manifest, as JSON.
///
/// A map rather than a [`Json`] because two messages carry a manifest and only one of
/// them also carries an operation: a push request is the manifest with `op` flattened
/// into it, and the reply to a pull is the manifest alone. Returning the map is what
/// lets those two share one writer of the field names.
///
/// `bytes` is an `i64` because JSON numbers are, and it is safe here rather than by
/// luck: a manifest is refused above four gibibytes before it is ever encoded.
fn manifest_fields(manifest: &Manifest) -> BTreeMap<String, Json> {
    let mut entries = BTreeMap::new();
    entries.insert("path".to_string(), Json::str(&manifest.path));
    entries.insert("bytes".to_string(), Json::Int(manifest.bytes as i64));
    entries.insert("sha256".to_string(), Json::str(&manifest.sha256));
    entries
}

/// A manifest from JSON.
///
/// Ignores every field it does not know, which is what lets a push request carry `op`
/// alongside these three without a second reader for the same shape. **The size and
/// the digest are checked here; the path is not** -- whether a digest is well formed
/// is a fact about this message, and whether a path may be written to is a question
/// for the receiving machine. [`Manifest::check_locally`] is the second half.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field that is wrong or missing.
pub fn manifest_from_json(value: &Json) -> Result<Manifest, WireError> {
    let path = value
        .get_str("path")
        .ok_or_else(|| WireError::BadRequest("path: missing or not a string".to_string()))?
        .to_string();

    let bytes = match value.get("bytes") {
        Some(Json::Int(bytes)) if *bytes > 0 => *bytes as u64,
        Some(Json::Int(bytes)) => {
            return Err(WireError::BadRequest(format!(
                "bytes: {bytes} is not a positive number of bytes"
            )));
        }
        Some(_) => return Err(WireError::BadRequest("bytes: not a number".to_string())),
        None => return Err(WireError::BadRequest("bytes: missing".to_string())),
    };

    let sha256 = value
        .get_str("sha256")
        .ok_or_else(|| WireError::BadRequest("sha256: missing or not a string".to_string()))?
        .to_string();

    Ok(Manifest {
        path,
        bytes,
        sha256,
    })
}

/// What a receiver did with a transfer.
///
/// **The digest it computed**, which is the evidence that T7 passed: a caller told
/// only "done" has no way to know that the file now on the disk is the one that was
/// sent, and the whole point of comparing digests is that somebody else can check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    /// How many bytes were written.
    pub bytes: u64,
    /// The digest of what is now at the destination, lowercase hex.
    pub sha256: String,
}

/// A transfer result as JSON.
pub fn transfer_outcome_to_json(outcome: &TransferOutcome) -> Json {
    object! {
        "bytes" => outcome.bytes as i64,
        "sha256" => outcome.sha256,
    }
}

/// A transfer result from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field that is wrong or missing.
pub fn transfer_outcome_from_json(value: &Json) -> Result<TransferOutcome, WireError> {
    let bytes = match value.get("bytes") {
        Some(Json::Int(bytes)) if *bytes >= 0 => *bytes as u64,
        Some(_) => {
            return Err(WireError::BadRequest(
                "bytes: missing or negative".to_string(),
            ));
        }
        None => return Err(WireError::BadRequest("bytes: missing".to_string())),
    };
    let sha256 = value
        .get_str("sha256")
        .ok_or_else(|| WireError::BadRequest("sha256: missing or not a string".to_string()))?
        .to_string();

    Ok(TransferOutcome { bytes, sha256 })
}

/// The result of a transfer, wrapped as a reply.
pub fn encode_transfer_reply(outcome: &TransferOutcome) -> Json {
    reply_result(transfer_outcome_to_json(outcome))
}

/// The result of a transfer, out of a reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal -- a caller that wants the
/// reason should match on [`Reply`] instead -- or when the result is not a transfer
/// result.
pub fn transfer_outcome_from_reply(reply: &Reply) -> Result<TransferOutcome, WireError> {
    match reply {
        Reply::Result(value) => transfer_outcome_from_json(value),
        Reply::Refused(reason) => Err(WireError::BadRequest(reason.clone())),
    }
}

/// The reply to a pull: the manifest of what is about to arrive.
pub fn manifest_reply(manifest: &Manifest) -> Json {
    reply_result(Json::Object(manifest_fields(manifest)))
}

/// A reply that carries an answer.
pub fn reply_result(result: Json) -> Json {
    object! { "ok" => true, "result" => result }
}

/// A reply that says the request could not be answered.
pub fn reply_refused(reason: &str) -> Json {
    object! { "ok" => false, "error" => reason }
}

/// A reply from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the body is not a reply. The distinction between
/// a result and a refusal is carried by `ok` and not inferred from which fields are
/// present, because two shapes that are told apart by their fields are two shapes
/// that eventually overlap.
pub fn reply_from_json(value: &Json) -> Result<Reply, WireError> {
    let ok = value.get("ok").and_then(Json::as_bool).ok_or_else(|| {
        WireError::BadRequest("a reply needs an ok field with true or false".to_string())
    })?;

    if ok {
        return match value.get("result") {
            Some(result) => Ok(Reply::Result(result.clone())),
            None => Err(WireError::BadRequest(
                "a reply that is ok needs a result".to_string(),
            )),
        };
    }

    match value.get("error") {
        Some(Json::Str(reason)) => Ok(Reply::Refused(reason.clone())),
        Some(_) => Err(WireError::BadRequest(
            "a refusal needs error to be a string".to_string(),
        )),
        None => Err(WireError::BadRequest(
            "a refusal needs an error field".to_string(),
        )),
    }
}

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

/// Encodes an outcome as the result of a run reply.
///
/// The whole of the agent's side of a run, in one call, so that the shape cannot be
/// assembled differently in two places.
pub fn encode_run_reply(outcome: &RunOutcome) -> Json {
    reply_result(run_outcome_to_json(outcome))
}

/// The identity reply, as the agent builds it.
pub fn identity_to_json(name: &str, version: &str) -> Json {
    reply_result(object! { "name" => name, "version" => version })
}

/// The name out of an identity result.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the result is a refusal or holds no name -- the
/// two ways a caller can be handed something that is not an answer.
pub fn identity_from_json(reply: &Reply) -> Result<String, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the identity reply is a refusal".to_string(),
        ));
    };
    value
        .get_str("name")
        .map(str::to_string)
        .ok_or_else(|| WireError::BadRequest("the identity reply has no name".to_string()))
}

/// The outcome out of a run reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal -- a caller that wants the
/// reason should match on [`Reply`] instead -- or when the result is not an outcome.
pub fn run_outcome_from_reply(reply: &Reply) -> Result<RunOutcome, WireError> {
    match reply {
        Reply::Result(value) => run_outcome_from_json(value),
        Reply::Refused(reason) => Err(WireError::BadRequest(reason.clone())),
    }
}

/// Reads one of this module's messages out of a frame body.
///
/// Bytes rather than text, because a frame carries bytes and the conversion to text
/// is a decision with an error case -- a body that is not UTF-8 is not JSON, and
/// saying so here means every caller does not have to.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the body is not UTF-8, or not JSON.
pub fn parse_body(body: &[u8]) -> Result<Json, WireError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| WireError::BadRequest("the message is not UTF-8".to_string()))?;
    json::parse(text).map_err(|error| WireError::BadRequest(error.to_string()))
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
