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

/// The request that brings a file back from an agent.
const OP_PULL: &str = "pull";

/// The request that asks what is running on the agent's machine.
const OP_PS: &str = "ps";

/// Every `op` this version understands, for an error message that lists them.
///
/// A single list rather than a sentence written at each refusal: a caller that sent
/// an `op` this version does not know needs to see the ones it does, and a list that
/// is written twice is a list that disagrees with itself eventually.
const KNOWN_OPS: &str = "identity, run, push, pull, ps";

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

/// What a command wrote to one stream, and whether it was text.
///
/// **A `String` cannot carry "these bytes are not text", and that was the defect.**
/// A command that emitted the four bytes `D6 D0 CE C4` -- GBK for two CJK characters --
/// reached the caller as four `U+FFFD`, and no field in the reply said the output was
/// not text. `docs/ROADMAP.md` M10 is the whole of it: the defect is **not which guess
/// is made but that the guess was silent**, and on the Windows targets this tool is for
/// that is the difference between reading a program's error and reading mojibake.
///
/// So the bytes are decoded here, once, and the fact that something was replaced travels
/// with the text instead of being dropped at the point of decoding. What it deliberately
/// does **not** do is guess a code page: decoding with the machine's OEM code page is
/// what the sibling project does, and it is a real improvement, but it is a decision
/// about which encoding to assume -- and the second defect is that a decision was made
/// silently, not that the wrong one was.
///
/// The byte count is the count the command wrote, which is **not** the length of the
/// text: a replaced byte becomes three bytes of UTF-8, and a multi-byte character that
/// survives keeps its own length in bytes and loses it in characters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Text {
    /// The bytes as text, with anything undecodable replaced.
    value: String,
    /// How many bytes the command wrote.
    byte_count: usize,
    /// Whether any of them had to be replaced.
    lossy: bool,
}

impl Text {
    /// Decodes what a command wrote, recording whether anything was lost.
    ///
    /// Lossy rather than strict, and the same way round as `String::from_utf8_lossy`:
    /// a command that writes one invalid byte must not cost the caller the other ten
    /// thousand that were fine. What was missing before is the second half -- that the
    /// replacement is now *recorded* rather than merely visible to someone who knows
    /// what `U+FFFD` means.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self {
                value: text.to_string(),
                byte_count: bytes.len(),
                lossy: false,
            },
            Err(_) => Self {
                value: String::from_utf8_lossy(bytes).into_owned(),
                byte_count: bytes.len(),
                lossy: true,
            },
        }
    }

    /// The text, with replacements where the bytes were not text.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Whether any byte had to be replaced to make this text.
    ///
    /// The field the defect was about: a caller that reads this can tell an answer from
    /// a guess, and one that does not still gets the text it would have got before.
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }

    /// How many bytes the command wrote.
    ///
    /// The size of the output rather than of the text, which is what a person asking
    /// "how much did it print" means, and what a refusal about a reply too large to
    /// frame has to compare against a ceiling.
    pub fn byte_count(&self) -> usize {
        self.byte_count
    }

    /// Why this text is not the whole answer, or `None` when it is.
    ///
    /// A sentence rather than a flag because it is read by a person looking at a
    /// rendered command, and it names what happened instead of leaving them to work out
    /// what a replacement character means.
    fn loss_note(&self) -> Option<&'static str> {
        self.lossy.then_some(
            "some bytes were not text, and are shown as replacement characters rather than dropped",
        )
    }
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.value)
    }
}

/// Reads a text field and its byte count out of a run outcome.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field when the text is missing or is not a
/// string. **The count and the loss flag are optional on purpose**: an agent older than
/// this change sends neither, and a host that demanded them would refuse a reply it can
/// read perfectly well -- the count falls back to the text's own length, which is exact
/// when the text is not lossy and is the only thing left to say when it is.
fn text_from_json(value: &Json, field: &str) -> Result<Text, WireError> {
    let text = value
        .get_str(field)
        .ok_or_else(|| WireError::BadRequest(format!("{field}: missing or not a string")))?;

    let byte_count = match value.get(&format!("{field}_bytes")) {
        Some(Json::Int(bytes)) if *bytes >= 0 => *bytes as usize,
        Some(_) => {
            return Err(WireError::BadRequest(format!(
                "{field}_bytes: not a count of bytes"
            )));
        }
        None => text.len(),
    };

    let lossy = match value.get(&format!("{field}_not_utf8")) {
        Some(Json::Bool(lossy)) => *lossy,
        Some(_) => {
            return Err(WireError::BadRequest(format!(
                "{field}_not_utf8: not true or false"
            )));
        }
        None => false,
    };

    Ok(Text {
        value: text.to_string(),
        byte_count,
        lossy,
    })
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
    /// Bring a file back from the agent.
    ///
    /// One path and nothing else: what comes back is a manifest and then the file, so
    /// the answer tells the caller the size and the digest before a byte of it arrives.
    /// The path is resolved against the agent's transfer root, which is what stops a
    /// pull from reading the machine rather than the directory it was pointed at.
    Pull {
        /// The file to read, inside the agent's root.
        path: String,
    },
    /// Ask what is running on the agent's machine.
    ///
    /// The filter is the whole of this request, and an empty one means "everything". What
    /// comes back is a [`crate::process::Listing`], which is a list **and** the counts and
    /// notes that make an empty list readable -- `docs/ROADMAP.md` M10.
    Ps(crate::process::Filter),
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
    pub stdout: Text,
    /// What it wrote to standard error.
    pub stderr: Text,
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
        Request::Pull { path } => object! { "op" => OP_PULL, "path" => path },
        Request::Ps(filter) => {
            // **An empty filter adds no fields at all**, which is why the pinned bytes of a
            // bare `ps` are `{"op":"ps"}`: absent and empty are the same request, so there
            // is no reason for two spellings of it on the wire.
            let mut entries = match filter_to_json(filter) {
                Json::Object(entries) => entries,
                _ => BTreeMap::new(),
            };
            entries.insert("op".to_string(), Json::str(OP_PS));
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
        OP_PULL => Ok(Request::Pull {
            path: value
                .get_str("path")
                .ok_or_else(|| WireError::BadRequest("path: missing or not a string".to_string()))?
                .to_string(),
        }),
        OP_PS => Ok(Request::Ps(filter_from_json(value)?)),
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
///
/// A pull needs this because the size has to come from whoever holds the file, and the
/// receiver needs it before the first chunk so it knows how many to expect. A push does
/// **not** use it -- an accepted push is answered with [`manifest_accepted`], which is a
/// different shape on purpose.
pub fn manifest_reply(manifest: &Manifest) -> Json {
    reply_result(Json::Object(manifest_fields(manifest)))
}

/// The receiver's answer to a push's manifest: that it has read it, and what it will take.
///
/// This message exists because of a failure found on a real machine, and `docs/transfer.md`
/// T14 is the whole of it. The sender has the file ready and nothing to stop it streaming,
/// so when the receiver refuses at the manifest the sender is already writing; the receiver
/// then closes with those unread bytes in its queue, Windows resets that connection, and the
/// reset destroys the refusal the sender never got round to reading. The sender reported
/// "the agent closed the connection without answering" -- true, and useless.
///
/// So the receiver answers the manifest before the body, and the sender waits for that
/// answer. The size is echoed back for the sender to check: a receiver that accepted a
/// different number has read a different manifest, and that is worth finding out here
/// rather than from a digest.
///
/// **Deliberately not a [`TransferOutcome`].** An acceptance carries `accepted` and no
/// `sha256`; a result carries `sha256` and no `accepted`. Two reply shapes overlapping on
/// `bytes` alone is how a client comes to read an acceptance as an outcome.
pub fn manifest_accepted(manifest: &Manifest) -> Json {
    reply_result(object! { "accepted" => true, "bytes" => manifest.bytes as i64 })
}

/// The size a receiver said it would take, out of its answer to a manifest.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal, is not an acceptance, or carries
/// no size.
pub fn accepted_bytes_from_reply(reply: &Reply) -> Result<u64, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the manifest was refused".to_string(),
        ));
    };
    if value.get("accepted").and_then(Json::as_bool) != Some(true) {
        return Err(WireError::BadRequest(
            "the reply is not an acceptance of the manifest".to_string(),
        ));
    }
    match value.get("bytes") {
        Some(Json::Int(bytes)) if *bytes >= 0 => Ok(*bytes as u64),
        _ => Err(WireError::BadRequest(
            "an acceptance needs a byte count".to_string(),
        )),
    }
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
///
/// Each stream is written as its text, the number of bytes the command wrote, and
/// whether any of them had to be replaced -- `docs/ROADMAP.md` M10, where the defect is
/// that a guess about encoding was silent. The two extra fields are always written, so a
/// host reading a reply does not have to infer them from the text.
///
/// **They are optional on the reading side**, because an agent older than this change
/// sends only the text. A reply that can be read is not refused for a field that was not
/// invented when it was written.
pub fn run_outcome_to_json(outcome: &RunOutcome) -> Json {
    let mut entries = BTreeMap::new();
    entries.insert(
        "exit_code".to_string(),
        match outcome.exit_code {
            Some(code) => Json::Int(i64::from(code)),
            None => Json::Null,
        },
    );
    for (field, text) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
        entries.insert(field.to_string(), Json::str(text.as_str()));
        entries.insert(
            format!("{field}_bytes"),
            Json::Int(text.byte_count() as i64),
        );
        entries.insert(format!("{field}_not_utf8"), Json::Bool(text.is_lossy()));
    }
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
/// `stdout` and `stderr` carry a byte count and a loss flag that an older agent does
/// not send. A reply that can be read is not refused for a field that did not exist
/// when it was written, so those two default to the text's own length and to "clean".
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

    let stdout = text_from_json(value, "stdout")?;
    let stderr = text_from_json(value, "stderr")?;

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

/// A process filter as JSON, for a `ps` request.
///
/// **The field names are the caller's, and the same names come back in the reply's
/// `applied` object.** That is what makes the echo worth having: a caller can compare what
/// it sent with what was applied without a mapping table in between.
///
/// An empty filter is an empty object rather than `null`: a ps request always carries one,
/// and a shape that was sometimes absent would need two readings of the same message.
pub fn filter_to_json(filter: &crate::process::Filter) -> Json {
    let mut entries = BTreeMap::new();
    for (key, value) in [
        ("name", &filter.name),
        ("path", &filter.path),
        ("cmdline", &filter.cmdline),
        ("query", &filter.query),
        ("exclude", &filter.exclude),
    ] {
        if let Some(value) = value {
            entries.insert(key.to_string(), Json::str(value));
        }
    }
    Json::Object(entries)
}

/// A process filter from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field that is not a string. A field that is simply
/// absent is not an error: it means the caller did not filter on it, which is the ordinary
/// case for four of the five.
pub fn filter_from_json(value: &Json) -> Result<crate::process::Filter, WireError> {
    let text = |field: &str| -> Result<Option<String>, WireError> {
        match value.get(field) {
            Some(Json::Str(text)) => Ok(Some(text.clone())),
            Some(_) => Err(WireError::BadRequest(format!(
                "{field}: not a string, and a filter is text"
            ))),
            None => Ok(None),
        }
    };

    Ok(crate::process::Filter {
        name: text("name")?,
        path: text("path")?,
        cmdline: text("cmdline")?,
        query: text("query")?,
        exclude: text("exclude")?,
    })
}

/// A listing as the result of a `ps` reply.
///
/// Every field is written, including the empty ones, because each one is an answer to a
/// question a reader has to be able to ask: how many matched, how many were looked at,
/// whether the list was cut short, what filter was applied, and what the machine could not
/// say. Leaving one out is how an empty list becomes indistinguishable from a machine that
/// could not be read -- `docs/ROADMAP.md` M10.
pub fn ps_listing_to_json(listing: &crate::process::Listing) -> Json {
    let processes: Vec<Json> = listing
        .processes
        .iter()
        .map(|process| {
            let mut entries = BTreeMap::new();
            entries.insert("pid".to_string(), Json::Int(i64::from(process.pid)));
            entries.insert("name".to_string(), Json::str(&process.name));
            // Absent rather than null for a field the machine could not supply: `null`
            // would be a fourth state to interpret, and the reader already has the notes.
            if let Some(path) = &process.path {
                entries.insert("path".to_string(), Json::str(path));
            }
            if let Some(cmdline) = &process.cmdline {
                entries.insert("cmdline".to_string(), Json::str(cmdline));
            }
            Json::Object(entries)
        })
        .collect();

    let notes: Vec<Json> = listing.notes.iter().map(Json::str).collect();
    let complete = match listing.incomplete() {
        crate::process::Incomplete::No => "no",
        crate::process::Incomplete::UnreadableLines => "unreadable-lines",
        crate::process::Incomplete::NotEnumerated => "not-enumerated",
    };

    object! {
        "processes" => Json::Array(processes),
        "count" => listing.processes.len() as i64,
        "total" => listing.total as i64,
        "unreadable" => listing.unreadable as i64,
        "truncated" => listing.truncated,
        "complete" => complete,
        "applied" => listing.applied.clone(),
        "notes" => Json::Array(notes),
    }
}

/// A listing out of a `ps` reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal -- a caller that wants the reason
/// should match on [`Reply`] instead -- or when the result is not a listing at all. A
/// listing missing its counts is **not** defaulted: an agent that did not say how many
/// processes it looked at has not answered the question, and filling in zero would report
/// a clean machine on no evidence.
pub fn ps_listing_from_reply(reply: &Reply) -> Result<crate::process::Listing, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the ps request was refused".to_string(),
        ));
    };

    let processes_value = value
        .get("processes")
        .and_then(Json::as_array)
        .ok_or_else(|| WireError::BadRequest("processes: missing or not an array".to_string()))?;

    let mut processes = Vec::new();
    for entry in processes_value {
        let pid = match entry.get("pid") {
            Some(Json::Int(pid)) if *pid >= 0 => *pid as u32,
            _ => {
                return Err(WireError::BadRequest("a process needs a pid".to_string()));
            }
        };
        let name = entry
            .get_str("name")
            .ok_or_else(|| WireError::BadRequest("a process needs a name".to_string()))?
            .to_string();
        processes.push(crate::process::Process {
            pid,
            name,
            path: entry.get_str("path").map(str::to_string),
            cmdline: entry.get_str("cmdline").map(str::to_string),
        });
    }

    let number = |field: &str| -> Result<usize, WireError> {
        match value.get(field) {
            Some(Json::Int(number)) if *number >= 0 => Ok(*number as usize),
            Some(_) => Err(WireError::BadRequest(format!("{field}: not a count"))),
            None => Err(WireError::BadRequest(format!("{field}: missing"))),
        }
    };

    let notes = value
        .get("notes")
        .and_then(Json::as_array)
        .map(|notes| {
            notes
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(crate::process::Listing {
        processes,
        total: number("total")?,
        unreadable: number("unreadable")?,
        truncated: value.get("truncated").and_then(Json::as_bool) == Some(true),
        applied: value
            .get("applied")
            .cloned()
            .unwrap_or(Json::Object(BTreeMap::new())),
        notes,
    })
}

/// A listing, wrapped as a reply.
pub fn encode_ps_reply(listing: &crate::process::Listing) -> Json {
    reply_result(ps_listing_to_json(listing))
}

/// Whether a reply fits in the bytes one frame may carry.
///
/// **The ceiling is a parameter and not a constant**, so that this decision can be
/// asked with a number a test chose and not only with the frame module's sixteen
/// mebibytes -- the same reason `Probe` takes a budget. What the agent passes is
/// [`reply_ceiling`], which is the frame's own limit less the room the seal needs.
///
/// The encoded length, rather than the outcome's own fields: what has to fit is
/// the message, and the JSON around the output is part of it. The encoding is
/// deterministic and escapes what has to be escaped, so measuring it is exact --
/// an estimate of it would be a number that is wrong exactly where it matters.
pub fn reply_fits(reply: &Json, ceiling: usize) -> bool {
    json::write(reply).len() <= ceiling
}

/// The largest reply this protocol can send, in the bytes that reply encodes to.
///
/// **The name says reply rather than plaintext on purpose**, because the number is
/// not a limit on a command's output: what has to fit is the reply *around* the
/// output -- `stdout` and `stderr` as JSON, their escapes, and the other three
/// fields. A command may write less than this and still not fit, and that is the
/// case [`run_reply_too_large`] exists to report.
///
/// It is the frame's limit less the channel's tag, which is the only thing sealing
/// adds: the nonce is the message count, which both sides already know, so it is
/// authenticated rather than transmitted. Written as arithmetic rather than as
/// `16 * 1024 * 1024 - 16`, so that moving either number moves this one, and pinned
/// against the frame module's own constants by a test here and against the cipher's
/// by one in `linklet-adapters`.
///
/// The dependency on `crate::frame` is a read of constants and nothing else -- this
/// module still opens no socket and holds no buffer, which is what rule 1 protects.
pub fn reply_ceiling() -> usize {
    // The tag length, named here rather than imported from the adapter:
    // `linklet-core` cannot depend on `linklet-adapters`, because the arrow points
    // the other way. A second copy of a constant is a risk; a second copy with a
    // test that fails when the two differ is the version of that risk this project
    // can afford.
    const CHANNEL_TAG_BYTES: usize = 16;

    crate::frame::MAX_PAYLOAD - CHANNEL_TAG_BYTES
}

/// The refusal for a command whose reply will not fit in one frame.
///
/// **This is the answer where there used to be silence.** The agent took the
/// command's output, built the reply, and found it could not be framed, so it sent
/// nothing and closed -- and a caller cannot tell that from a machine that dropped
/// off the network. `docs/ROADMAP.md` M10 carries the measurement: 20,000,000 bytes
/// of output, the command exiting 0 on the target, the caller told "could not reach
/// the agent".
///
/// The reason names **both stream sizes and the ceiling**, because those are the
/// facts the caller needs to act on: which stream was large, how large, and what the
/// reply would have had to fit in. The sizes carry no thousands separator, so a
/// number in this sentence can be compared with a file's without being parsed out of
/// prose.
///
/// **The stream sizes are bytes the command wrote, and the ceiling is bytes of a
/// reply**, which are different measurements and are labelled as such. A caller that
/// read the ceiling as an output limit would think a 20 MB stdout was over a 16 MB
/// one by three megabytes, when what actually overflowed was the JSON carrying it.
///
/// It is a refusal and not a result on purpose -- the output did not come back, so a
/// caller must not go looking for it in a reply that cannot hold it. And it is a
/// refusal rather than **raising the ceiling or streaming the reply**, which are the
/// other two shapes: a larger number buys the same silence above it, and streaming is
/// a change to the protocol rather than a report about one. That is the choice
/// `docs/ROADMAP.md` M10 records, and this is the smallest shape that ends the
/// silence.
///
/// `ceiling` is the same parameter [`reply_fits`] takes, and it is quoted in the
/// message so the number a caller reads is the number that was applied.
pub fn run_reply_too_large(outcome: &RunOutcome, ceiling: usize) -> Json {
    reply_refused(&format!(
        "the command's output is too large to return: stdout was {} bytes and stderr {} \
         bytes, and a reply of at most {ceiling} bytes is what this agent can send. Write \
         the output to a file on the target and pull that instead",
        outcome.stdout.byte_count(),
        outcome.stderr.byte_count(),
    ))
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

/// Renders a transfer's outcome as the text a caller reads.
///
/// One line, in the order a person checks them: **where it landed, how big it is, and
/// what it hashes to.** The digest is the whole reason a transfer reports anything --
/// a caller that is told "done" has no way to know the file it now has is the one that
/// was sent.
///
/// The destination is passed in rather than read out of the manifest. On a push it is a
/// path on the other machine and on a pull a path on this one, and either way it is what
/// the caller asked for -- the manifest's own `path` field is the sender's answer to a
/// question the caller already answered.
pub fn render_transfer(outcome: &TransferOutcome, destination: &str) -> String {
    format!(
        "{destination}: {} bytes, sha256 {}",
        outcome.bytes, outcome.sha256
    )
}

/// Renders an outcome as the text an agent reads.
///
/// Fixed shape, and the first thing on every line is a fact rather than a
/// sentence about one: the exit code, then the streams. `is_error` is *not*
/// decided here -- a reply that says "the command failed" is the caller's
/// business, and this function has no way to know whether a non-zero exit
/// matters for the command that was run.
///
/// **A stream that was not text says so, under its own heading.** The heading is
/// indented rather than the marker being appended to the body, because the body is a
/// program's output and a sentence glued to the end of it would be indistinguishable
/// from something the program printed. That is the whole point of this line existing:
/// `U+FFFD` in the middle of a build log is not self-explanatory, and a caller who does
/// not know what it means reads mojibake as an encoding problem in the program.
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

    for (name, stream) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
        if stream.as_str().is_empty() {
            continue;
        }
        out.push_str(&format!("{name}:\n"));
        if let Some(note) = stream.loss_note() {
            out.push_str(&format!("  {note}\n"));
        }
        out.push_str(stream.as_str());
        if !stream.as_str().ends_with('\n') {
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{self, Kind};

    /// The whole point of [`reply_ceiling`] is that a reply of exactly that size is
    /// still a frame the sender will produce. The unit tests in `tests/wire_protocol.rs`
    /// check the predicate's boundary and the arithmetic; this is the one claim neither
    /// of them can make, because it is about the frame format rather than about a
    /// reply.
    ///
    /// It lives here, in a module that allocates nothing, rather than in that file:
    /// building a reply of sixteen mebibytes to ask the question took seven seconds
    /// there, and the answer is a fact about two constants.
    #[test]
    fn a_reply_of_exactly_the_ceiling_is_sealed_into_a_frame_that_fits() {
        let sealed = reply_ceiling() + 16;
        assert!(
            frame::header(Kind::Sealed, sealed).is_ok(),
            "a reply the predicate accepts has to be a frame the sender can build"
        );
        assert!(
            frame::header(Kind::Sealed, sealed + 1).is_err(),
            "and one byte past it has to be refused, or the ceiling is not one"
        );
    }

    /// The paragraph above the constant says it is the frame limit less the tag. This
    /// is the other half: the number is not a byte low, which would refuse a reply that
    /// would have gone. A nuisance rather than a defect, and still wrong.
    #[test]
    fn the_ceiling_is_the_frame_limit_and_nothing_more_was_subtracted() {
        assert_eq!(frame::MAX_PAYLOAD - reply_ceiling(), 16);
    }
}
