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

use crate::transfer::Manifest;
use serde_json::Map;
use serde_json::Value;

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

/// The request that stops something on the agent's machine.
const OP_KILL: &str = "kill";

/// The request that starts something that outlives the request.
const OP_SPAWN: &str = "spawn";

/// The request that searches a file on the agent's machine.
const OP_GREP: &str = "grep";

/// The request that reads the end of a file on the agent's machine.
const OP_TAIL: &str = "tail";

/// The request that lists a path on the agent's machine.
const OP_LS: &str = "ls";

/// Every `op` this version understands, for an error message that lists them.
///
/// A single list rather than a sentence written at each refusal: a caller that sent
/// an `op` this version does not know needs to see the ones it does, and a list that
/// is written twice is a list that disagrees with itself eventually.
const KNOWN_OPS: &str = "identity, run, push, pull, ps, kill, spawn, grep, tail, ls";

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
pub fn handshake_to_json(public: &[u8]) -> Value {
    serde_json::json!({ "ephemeral_public": to_hex(public), "protocol": PROTOCOL_VERSION })
}

/// The protocol this build speaks, as a whole number.
///
/// **Raised only when a change makes an old peer answer wrongly rather than not at all.**
/// Adding an operation does not raise it: an old agent refuses an `op` it does not know, by
/// name, which is an answer. Reusing a field, changing what a field means, or changing the
/// framing does raise it -- those are the changes where an old peer would do something wrong
/// while believing it understood.
///
/// **Every build before this field existed is protocol 1**, because the frame protocol and the
/// sealed request/reply shape it is built on were already 1 when they shipped. That is why the
/// first number here is 1 and not 0: a handshake with no `protocol` field is an agent that
/// predates the field, and calling it 0 would put it outside the range of things this scheme
/// can talk about.
///
/// Protocol versions are small positive integers, read with
/// [`serde_json::Value::as_i64`].
///
/// `docs/VERSIONING.md` is the whole policy. `docs/decisions.md` D2 records the cost of the
/// version this replaces, which did not negotiate at all.
pub const PROTOCOL_VERSION: i64 = 3;

/// What a peer speaks when its handshake has no `protocol` field.
///
/// **A separate name from [`PROTOCOL_VERSION`], and the separation is the whole point.** The
/// first version of the reader below returned the *current* version for an absent field, which
/// made the comparison in `linklet_client` a tautology: absence read as "agrees with me", so
/// the one case the field was added for -- an old agent against a new tool -- was the one case
/// that could not be detected. The test that pins that case is what found it.
pub const OLDEST_PROTOCOL: i64 = 1;

/// The protocol number out of a handshake, or [`OLDEST_PROTOCOL`] when the peer does not say.
///
/// **Absence is not a malformed message.** A peer that predates this field sends a perfectly
/// good handshake that happens to be short one key, and refusing the *message* would leave a
/// caller with "bad request" and no idea which end to upgrade. So absence is read as the oldest
/// protocol there is and the two numbers are compared by the caller, which is where the
/// sentence a person reads is built.
///
/// A `protocol` key that is present and is *not* a whole number is a real error, because that
/// is a peer disagreeing about what the field is rather than one that predates it.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the key is present and is not a whole number.
pub fn handshake_protocol_from_json(value: &Value) -> Result<i64, WireError> {
    match value.get("protocol") {
        None => Ok(OLDEST_PROTOCOL),
        Some(field) => field.as_i64().ok_or_else(|| {
            WireError::BadRequest("a handshake protocol must be a number".to_string())
        }),
    }
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
pub fn handshake_public_from_json(value: &Value) -> Result<Vec<u8>, WireError> {
    let text = value
        .get("ephemeral_public")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            WireError::BadRequest("a handshake needs an ephemeral_public hex string".to_string())
        })?;
    from_hex(text)
}

/// Decoded output from one command stream, with raw-byte accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
    value: String,
    byte_count: usize,
    retained_bytes: usize,
    encoding: String,
    lossy: bool,
    decode_error: Option<String>,
}
impl Default for Text {
    fn default() -> Self {
        Self::from_bytes(b"")
    }
}
impl Text {
    /// Decodes UTF-8 bytes, marking invalid bytes as lossy.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self::decoded(
            String::from_utf8_lossy(bytes).into_owned(),
            bytes.len(),
            bytes.len(),
            "utf-8".into(),
            std::str::from_utf8(bytes).is_err(),
            None,
        )
    }
    /// Constructs decoded output. Counts describe raw bytes before decoding;
    /// retained bytes must not exceed total bytes.
    pub fn decoded(
        value: String,
        total_bytes: usize,
        retained_bytes: usize,
        encoding: String,
        lossy: bool,
        decode_error: Option<String>,
    ) -> Self {
        Self {
            value,
            byte_count: total_bytes,
            retained_bytes,
            encoding,
            lossy,
            decode_error,
        }
    }
    /// The retained output as text.
    pub fn as_str(&self) -> &str {
        &self.value
    }
    /// Whether decoding replaced invalid bytes.
    pub fn is_lossy(&self) -> bool {
        self.lossy
    }
    /// Total bytes emitted, including discarded output.
    pub fn byte_count(&self) -> usize {
        self.byte_count
    }
    /// Raw bytes retained before trimming incomplete boundary characters.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    /// Actual encoding used, such as utf-8 or cp936.
    pub fn encoding(&self) -> &str {
        &self.encoding
    }
    /// Whether earlier output was discarded.
    pub fn is_truncated(&self) -> bool {
        self.retained_bytes < self.byte_count
    }
    /// Why a decoder failed, if a lossy fallback was necessary.
    pub fn decode_error(&self) -> Option<&str> {
        self.decode_error.as_deref()
    }
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
fn text_from_json(value: &Value, field: &str) -> Result<Text, WireError> {
    let stream = &value[field];
    let bad = || WireError::BadRequest(format!("{field}: invalid protocol 3 stream metadata"));
    let text = stream["text"].as_str().ok_or_else(bad)?;
    let total = stream["total_bytes"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(bad)?;
    let retained = stream["retained_bytes"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(bad)?;
    let encoding = stream["encoding"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(bad)?;
    let truncated = stream["truncated"].as_bool().ok_or_else(bad)?;
    let lossy = stream["lossy"].as_bool().ok_or_else(bad)?;
    if retained > total || truncated != (retained < total) {
        return Err(bad());
    }
    let decode_error = match stream.get("decode_error") {
        None | Some(Value::Null) => None,
        Some(Value::String(error)) => Some(error.clone()),
        _ => return Err(bad()),
    };
    Ok(Text::decoded(
        text.into(),
        total,
        retained,
        encoding.into(),
        lossy,
        decode_error,
    ))
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
    /// Shell command or direct executable and literal arguments.
    pub command: crate::execution::CommandSpec,
    /// Per-call directory, resolved against the agent startup directory.
    pub cwd: Option<String>,
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
    /// Absolute paths are used directly; relative paths use the agent startup directory.
    Pull {
        /// The file to read, absolute or relative to the agent startup directory.
        path: String,
    },
    /// Ask what is running on the agent's machine.
    ///
    /// The filter is the whole of this request, and an empty one means "everything". What
    /// comes back is a [`crate::process::Listing`], which is a list **and** the counts and
    /// notes that make an empty list readable -- `docs/ROADMAP.md` M10.
    Ps(crate::process::Filter),
    /// Stop something on the agent's machine.
    ///
    /// What to stop, whether the caller meant it, and which candidates to consider. The two
    /// fields that decide whether this is allowed at all are [`KillRequest::force`] and
    /// [`KillRequest::candidates`] -- a bulk match without the first is refused by name, and
    /// the second is how a caller says "only among processes matching this", which is how
    /// one build is stopped without stopping the test harness that shares its name.
    Kill(KillRequest),
    /// Start a program that outlives the request that started it.
    ///
    /// **The difference from [`Request::Run`], and the whole reason this exists**: `run`
    /// waits for the command and holds its output in the reply, so a program that is meant
    /// to keep running holds the request, the connection and the agent's pipes with it.
    /// This starts the program with **its own output file** and answers with its pid, which
    /// is the shape the deploy loop needs -- start the new build, then ask `ps` whether it
    /// is still there. See `docs/ROADMAP.md` M10.
    Spawn(SpawnRequest),
    /// Search a file on the agent's machine.
    ///
    /// **The answer to "look at the log" without moving it**, which is what
    /// `docs/ROADMAP.md` M10 asks for: a pull is digest-verified, and it is
    /// the wrong tool for a two-gigabyte log whose last ERROR is the question.
    Grep(GrepRequest),
    /// Read the end of a file on the agent's machine.
    Tail(TailRequest),
    /// List a path on the agent's machine.
    ///
    /// The one request whose answer is **about a directory**: what is in it, how many entries
    /// there are, and whether it was there at all. `grep`, `tail`, `push` and `pull` all
    /// assume a path exists; this is how a caller finds out without moving something to see.
    Ls(LsRequest),
}

/// A request to list a path on the agent's machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsRequest {
    /// The directory, or a single file, absolute or relative to the agent startup directory.
    pub path: String,
}

/// A request to search a file on the agent's machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepRequest {
    /// The file, absolute or relative to the agent startup directory.
    pub path: String,
    /// What to look for.
    pub pattern: crate::search::Pattern,
    /// Which end of the file to work from.
    pub direction: crate::search::Direction,
    /// How many matches, and how much context.
    pub limit: crate::search::Limit,
}

/// A request to read the end of a file on the agent's machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailRequest {
    /// Resume from a stateless cursor; absent means the last count lines.
    pub cursor: Option<String>,
    /// Internal presentation ceiling in UTF-8 bytes (4..=1 MiB).
    pub content_bytes: usize,
    /// The file, absolute or relative to the agent startup directory.
    pub path: String,
    /// How many lines from the end.
    pub count: usize,
}

/// The most lines a `tail` will return.
///
/// A ceiling for the same reason every other one here exists: the reply is one frame, and a
/// caller asking for a hundred thousand lines of a log would rather be told the limit than
/// have the agent build a reply it cannot send.
pub const MAX_TAIL_LINES: usize = 2000;

/// A request to start a program that keeps running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    /// Observe the child for 0..=5000 milliseconds; default is 500.
    pub observe_ms: u64,
    /// The command line, run through the same shell `run` uses.
    pub command: crate::execution::CommandSpec,
    /// Per-call directory; output paths remain relative to the startup directory.
    pub cwd: Option<String>,
    /// Where the program's output goes, as a path on the **agent's** side.
    ///
    /// **Required, and not a convenience.** A child that inherited the agent's pipes would
    /// keep them open for as long as it runs, which is what makes a long-running program
    /// through `run` the trap `docs/ROADMAP.md` M10 opens with. Its own file is also how the
    /// caller watches it afterwards -- and the reason `spawn` needs no log-reading feature
    /// to be useful.
    pub output: String,
}

/// What [`Request::Spawn`] answers with.
///
/// A pid and the command. **Nothing about health**, because at the moment of the reply
/// nothing knows: the program has started, which is not the same as staying up. That
/// question is `ps`'s, and answering it here with a guess would be the mistake this project
/// exists to remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnReport {
    /// Started, running_at_observation, or exited; none asserts readiness.
    pub state: String,
    /// Actual elapsed observation time in milliseconds.
    pub observed_ms: u64,
    /// Exit code only when an exit was observed.
    pub exit_code: Option<i32>,
    /// Failure observed while starting or watching the process.
    pub error: Option<crate::result::OperationError>,
    /// Bounded combined output tail from the explicitly named log.
    pub tail: Text,
    /// Absolute invocation directory.
    pub cwd: String,
    /// Absolute output file path, resolved against startup directory.
    pub output: String,
    /// Whether PID denotes the shell or the directly started program.
    pub pid_kind: String,
    /// The process identifier of what was started.
    pub pid: u32,
    /// The command line that was started, echoed.
    pub command: String,
}

/// A request to stop something.
///
/// The shape a caller builds, and the shape [`crate::process::plan_kill`] then decides
/// about. The decision is deliberately **not** here: whether a request is safe depends on
/// what is running and on which process is answering it, and neither is known to a type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillRequest {
    /// What to stop.
    pub to_kill: crate::process::ToKill,
    /// Whether the caller meant it. Required for anything that can match more than one
    /// process, and ignored for a pid.
    pub force: bool,
    /// Candidates to consider, before the match. An empty filter means the whole machine.
    ///
    /// This is what makes a bulk kill bounded: `--name app.exe --candidates --cmdline
    /// C:\deploy\app.exe` stops the deployed build and not the copy someone is debugging in
    /// another window.
    pub candidates: crate::process::Filter,
    /// Drop processes whose image name contains this, after the match.
    pub exclude: Option<String>,
}

/// What the agent answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// The request was answered, and this is the answer.
    ///
    /// The payload is free-form JSON because the answer to "which agent is this" and
    /// the answer to "run this" are different shapes, and forcing them into one
    /// would mean a struct with half its fields empty for every caller.
    Result(Value),
    /// The request arrived and could not be answered, and this is why.
    ///
    /// **Not a transport failure.** The message arrived, was understood, and says
    /// no; the caller has a sentence to read instead of a socket error to guess at.
    Refused(crate::result::OperationError),
}

/// What the agent observed.
///
/// Every field is something the agent saw rather than something it concluded.
/// In particular there is no `success` flag: an agent deciding what a caller
/// should think about an exit code is an agent that has to guess, and the caller
/// has the exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    /// Absolute directory used for this invocation.
    pub cwd: String,
    /// Typed failure, including a deadline or unsuccessful exit.
    pub error: Option<crate::result::OperationError>,
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

impl RunOutcome {
    /// The typed command failure, or an unsuccessful exit reported by the program.
    pub fn failure(&self) -> Option<crate::result::OperationError> {
        self.error.clone().or_else(|| {
            (self.exit_code != Some(0)).then(|| {
                crate::result::OperationError::new(
                    crate::result::ErrorCode::CommandFailed,
                    "execute",
                    format!("command exited with {:?}", self.exit_code),
                )
            })
        })
    }
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
pub fn request_to_json(request: &Request) -> Value {
    match request {
        Request::Identity => serde_json::json!({ "op": OP_IDENTITY }),
        Request::Run(run) => {
            let mut value = run_request_to_json(run);
            value["op"] = OP_RUN.into();
            value
        }
        Request::Push(manifest) => {
            let mut entries = manifest_fields(manifest);
            entries.insert("op".to_string(), serde_json::json!(OP_PUSH));
            Value::Object(entries)
        }
        Request::Pull { path } => serde_json::json!({ "op": OP_PULL, "path": path }),
        Request::Ps(filter) => {
            // **An empty filter adds no fields at all**, which is why the pinned bytes of a
            // bare `ps` are `{"op":"ps"}`: absent and empty are the same request, so there
            // is no reason for two spellings of it on the wire.
            let mut entries = match filter_to_json(filter) {
                Value::Object(entries) => entries,
                _ => Map::new(),
            };
            entries.insert("op".to_string(), serde_json::json!(OP_PS));
            Value::Object(entries)
        }
        Request::Kill(kill) => {
            let mut entries = match filter_to_json(&kill.candidates) {
                Value::Object(entries) => entries,
                _ => Map::new(),
            };

            // **`kill` is a tagged field rather than two optional ones.** A request that
            // carried both a pid and a name would need a rule for which wins, and that rule
            // is the kind of thing that is discovered while reading a diff rather than
            // while writing one.
            let (kind, what) = match &kill.to_kill {
                crate::process::ToKill::Pid(pid) => ("pid", serde_json::json!(i64::from(*pid))),
                crate::process::ToKill::Name(name) => ("name", serde_json::json!(name)),
                crate::process::ToKill::Matching(text) => {
                    entries.insert("contains".to_string(), Value::Bool(true));
                    ("name", serde_json::json!(text))
                }
            };
            entries.insert("kill".to_string(), serde_json::json!(kind));
            entries.insert(kind.to_string(), what);
            entries.insert("force".to_string(), Value::Bool(kill.force));
            if let Some(exclude) = &kill.exclude {
                entries.insert("exclude_name".to_string(), serde_json::json!(exclude));
            }
            entries.insert("op".to_string(), serde_json::json!(OP_KILL));
            Value::Object(entries)
        }
        Request::Spawn(spawn) => {
            let mut value = spawn.command.to_json();
            value["op"] = OP_SPAWN.into();
            value["output"] = spawn.output.clone().into();
            value["observe_ms"] = spawn.observe_ms.into();
            if let Some(cwd) = &spawn.cwd {
                value["cwd"] = cwd.clone().into();
            }
            value
        }
        Request::Grep(grep) => serde_json::json!({
            "op": OP_GREP,
            "path": grep.path,
            "pattern": grep.pattern.text,
            "case_sensitive": grep.pattern.case_sensitive,
            "mode": match grep.direction {
                crate::search::Direction::First => "first",
                crate::search::Direction::Last => "last",
            },
            "max_matches": grep.limit.max_matches as i64,
            "context": grep.limit.context as i64,
        }),
        Request::Tail(tail) => serde_json::json!({
            "op": OP_TAIL,
            "cursor":tail.cursor, "content_bytes":tail.content_bytes,
            "path": tail.path,
            "count": tail.count as i64,
        }),
        Request::Ls(list) => serde_json::json!({
            "op": OP_LS,
            "path": list.path,
        }),
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
pub fn request_from_json(value: &Value) -> Result<Request, WireError> {
    let op = value.get("op").and_then(Value::as_str).ok_or_else(|| {
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
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| WireError::BadRequest("path: missing or not a string".to_string()))?
                .to_string(),
        }),
        OP_PS => Ok(Request::Ps(filter_from_json(value)?)),
        OP_KILL => Ok(Request::Kill(kill_request_from_json(value)?)),
        OP_SPAWN => Ok(Request::Spawn(spawn_request_from_json(value)?)),
        OP_GREP => Ok(Request::Grep(grep_request_from_json(value)?)),
        OP_TAIL => Ok(Request::Tail(tail_request_from_json(value)?)),
        OP_LS => Ok(Request::Ls(LsRequest {
            path: required_text(value, "path")?,
        })),
        other => Err(WireError::BadRequest(format!(
            "op: {other:?} is not one of {KNOWN_OPS}"
        ))),
    }
}

/// The three fields of a manifest, as JSON.
///
/// A map rather than a [`Value`] because two messages carry a manifest and only one of
/// them also carries an operation: a push request is the manifest with `op` flattened
/// into it, and the reply to a pull is the manifest alone. Returning the map is what
/// lets those two share one writer of the field names.
///
/// The wire accepts signed counts. A manifest is capped at four gibibytes, well
/// within that range.
fn manifest_fields(manifest: &Manifest) -> Map<String, Value> {
    let mut entries = Map::new();
    entries.insert("path".to_string(), serde_json::json!(&manifest.path));
    entries.insert(
        "bytes".to_string(),
        serde_json::json!(manifest.bytes as i64),
    );
    entries.insert("sha256".to_string(), serde_json::json!(&manifest.sha256));
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
pub fn manifest_from_json(value: &Value) -> Result<Manifest, WireError> {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::BadRequest("path: missing or not a string".to_string()))?
        .to_string();

    let bytes = match value.get("bytes").map(Value::as_i64) {
        Some(Some(bytes)) if bytes > 0 => bytes as u64,
        Some(Some(bytes)) => {
            return Err(WireError::BadRequest(format!(
                "bytes: {bytes} is not a positive number of bytes"
            )));
        }
        Some(_) => return Err(WireError::BadRequest("bytes: not a number".to_string())),
        None => return Err(WireError::BadRequest("bytes: missing".to_string())),
    };

    let sha256 = value
        .get("sha256")
        .and_then(Value::as_str)
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
/// And a value of this type exists only once that comparison **passed** -- a mismatch is
/// a refusal and nothing else -- so the type itself is the statement that the bytes
/// matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    /// Actual destination on the receiving host.
    pub destination: String,
    /// Actual source, filled by the client after verifying the completion.
    pub source: Option<String>,
    /// How many bytes were written.
    pub bytes: u64,
    /// The digest of what is now at the destination, lowercase hex.
    pub sha256: String,
}

/// A transfer result as JSON.
pub fn transfer_outcome_to_json(outcome: &TransferOutcome) -> Value {
    serde_json::json!({
        "destination": outcome.destination,
        "source": outcome.source,
        "bytes": outcome.bytes as i64,
        "sha256": outcome.sha256,
    })
}

/// A transfer result from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field that is wrong or missing.
pub fn transfer_outcome_from_json(value: &Value) -> Result<TransferOutcome, WireError> {
    let bytes = match value.get("bytes").map(Value::as_i64) {
        Some(Some(bytes)) if bytes >= 0 => bytes as u64,
        Some(_) => {
            return Err(WireError::BadRequest(
                "bytes: missing or negative".to_string(),
            ));
        }
        None => return Err(WireError::BadRequest("bytes: missing".to_string())),
    };
    let sha256 = value
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::BadRequest("sha256: missing or not a string".to_string()))?
        .to_string();

    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(WireError::BadRequest(
            "sha256: expected 64 hex digits".into(),
        ));
    }
    let destination = value
        .get("destination")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| WireError::BadRequest("destination: missing".into()))?
        .to_owned();
    let source = match value.get("source") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        _ => {
            return Err(WireError::BadRequest(
                "source: expected string or null".into(),
            ));
        }
    };
    Ok(TransferOutcome {
        bytes,
        sha256,
        destination,
        source,
    })
}

/// The result of a transfer, wrapped as a reply.
pub fn encode_transfer_reply(outcome: &TransferOutcome) -> Value {
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
        Reply::Refused(reason) => Err(WireError::BadRequest(reason.to_string())),
    }
}

/// The reply to a pull: the manifest of what is about to arrive.
///
/// A pull needs this because the size has to come from whoever holds the file, and the
/// receiver needs it before the first chunk so it knows how many to expect. A push does
/// **not** use it -- an accepted push is answered with [`manifest_accepted`], which is a
/// different shape on purpose.
pub fn manifest_reply(manifest: &Manifest) -> Value {
    reply_result(Value::Object(manifest_fields(manifest)))
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
pub fn manifest_accepted(manifest: &Manifest) -> Value {
    reply_result(serde_json::json!({ "accepted": true, "bytes": manifest.bytes as i64 }))
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
    if value.get("accepted").and_then(Value::as_bool) != Some(true) {
        return Err(WireError::BadRequest(
            "the reply is not an acceptance of the manifest".to_string(),
        ));
    }
    match value.get("bytes").map(Value::as_i64) {
        Some(Some(bytes)) if bytes >= 0 => Ok(bytes as u64),
        _ => Err(WireError::BadRequest(
            "an acceptance needs a byte count".to_string(),
        )),
    }
}

/// A reply that carries an answer.
pub fn reply_result(result: Value) -> Value {
    serde_json::json!({ "ok": true, "result": result })
}

/// A reply that says the request could not be answered.
pub fn reply_refused(reason: &str) -> Value {
    reply_failure(&crate::result::OperationError::from(reason))
}

/// A refusal with a stable category and phase.
pub fn reply_failure(error: &crate::result::OperationError) -> Value {
    serde_json::json!({"ok":false,"error":error})
}

/// A reply from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the body is not a reply. The distinction between
/// a result and a refusal is carried by `ok` and not inferred from which fields are
/// present, because two shapes that are told apart by their fields are two shapes
/// that eventually overlap.
pub fn reply_from_json(value: &Value) -> Result<Reply, WireError> {
    let ok = value.get("ok").and_then(Value::as_bool).ok_or_else(|| {
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
        Some(reason @ Value::Object(_)) => serde_json::from_value(reason.clone())
            .map(Reply::Refused)
            .map_err(|e| WireError::BadRequest(format!("invalid refusal: {e}"))),
        Some(_) => Err(WireError::BadRequest(
            "a refusal needs a typed error object".to_string(),
        )),
        None => Err(WireError::BadRequest(
            "a refusal needs an error field".to_string(),
        )),
    }
}

/// The request as JSON.
pub fn run_request_to_json(request: &RunRequest) -> Value {
    let mut value = request.command.to_json();
    value["timeout_seconds"] = request.timeout_seconds.into();
    if let Some(cwd) = &request.cwd {
        value["cwd"] = cwd.clone().into();
    }
    value
}

/// A request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field, because the caller is a program
/// on the other end of a socket and "invalid body" gives whoever is debugging it
/// nothing to look at.
pub fn run_request_from_json(value: &Value) -> Result<RunRequest, WireError> {
    let command = crate::execution::CommandSpec::from_json(value)?;
    let cwd = crate::execution::cwd_from_json(value)?;

    let timeout_seconds = match value.get("timeout_seconds").map(Value::as_i64) {
        None => {
            return Err(WireError::BadRequest(
                "timeout_seconds: missing".to_string(),
            ));
        }
        Some(Some(seconds)) if seconds >= 1 && (seconds as u64) <= MAX_TIMEOUT_SECONDS => {
            seconds as u64
        }
        Some(Some(seconds)) => {
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
        cwd,
        command,
        timeout_seconds,
    })
}

/// Encodes one bounded output stream with byte and encoding metadata.
pub fn text_to_json(text: &Text) -> Value {
    serde_json::json!({
        "text":text.as_str(), "total_bytes":text.byte_count(), "retained_bytes":text.retained_bytes(),
        "encoding":text.encoding(), "truncated":text.is_truncated(), "lossy":text.is_lossy(), "decode_error":text.decode_error()
    })
}

/// The outcome as protocol 3 JSON, including explicit per-stream metadata.
pub fn run_outcome_to_json(outcome: &RunOutcome) -> Value {
    let mut entries = Map::new();
    entries.insert("cwd".into(), outcome.cwd.clone().into());
    entries.insert("error".into(), serde_json::json!(outcome.error));
    entries.insert(
        "exit_code".to_string(),
        match outcome.exit_code {
            Some(code) => serde_json::json!(i64::from(code)),
            None => Value::Null,
        },
    );
    for (field, text) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
        entries.insert(field.to_string(), text_to_json(text));
    }
    entries.insert(
        "duration_ms".to_string(),
        serde_json::json!(outcome.duration_ms as i64),
    );
    entries.insert(
        "reason".to_string(),
        match &outcome.reason {
            Some(reason) => serde_json::json!(reason),
            None => Value::Null,
        },
    );
    Value::Object(entries)
}

/// An outcome from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] for anything that is not the shape above. A missing
/// `exit_code` *field* is an error; a present `null` is a `None`, because those
/// are different messages and only one of them means "killed or never started".
/// Both stream objects must carry the protocol 3 metadata.
pub fn run_outcome_from_json(value: &Value) -> Result<RunOutcome, WireError> {
    let exit_code = match value.get("exit_code") {
        None => {
            return Err(WireError::BadRequest("exit_code: missing".to_string()));
        }
        Some(Value::Null) => None,
        Some(code) => Some(
            code.as_i64()
                .and_then(|c| i32::try_from(c).ok())
                .ok_or_else(|| {
                    WireError::BadRequest("exit_code: outside signed 32-bit range".to_string())
                })?,
        ),
    };

    let stdout = text_from_json(value, "stdout")?;
    let stderr = text_from_json(value, "stderr")?;

    let duration_ms = match value.get("duration_ms").map(Value::as_i64) {
        Some(Some(ms)) if ms >= 0 => ms as u64,
        _ => {
            return Err(WireError::BadRequest(
                "duration_ms: missing or negative".to_string(),
            ));
        }
    };

    let reason = match value.get("reason") {
        None => return Err(WireError::BadRequest("reason: missing".to_string())),
        Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            return Err(WireError::BadRequest("reason: not a string".to_string()));
        }
    };

    Ok(RunOutcome {
        cwd: required_text(value, "cwd")?,
        error: serde_json::from_value(value["error"].clone())
            .map_err(|e| WireError::BadRequest(e.to_string()))?,
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
pub fn encode_run_reply(outcome: &RunOutcome) -> Value {
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
pub fn filter_to_json(filter: &crate::process::Filter) -> Value {
    let mut entries = Map::new();
    for (key, value) in [
        ("name", &filter.name),
        ("path", &filter.path),
        ("cmdline", &filter.cmdline),
        ("query", &filter.query),
        ("exclude", &filter.exclude),
    ] {
        if let Some(value) = value {
            entries.insert(key.to_string(), serde_json::json!(value));
        }
    }
    Value::Object(entries)
}

/// A process filter from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field that is not a string. A field that is simply
/// absent is not an error: it means the caller did not filter on it, which is the ordinary
/// case for four of the five.
pub fn filter_from_json(value: &Value) -> Result<crate::process::Filter, WireError> {
    let text = |field: &str| -> Result<Option<String>, WireError> {
        match value.get(field) {
            Some(Value::String(text)) => Ok(Some(text.clone())),
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
pub fn ps_listing_to_json(listing: &crate::process::Listing) -> Value {
    let processes: Vec<Value> = listing
        .processes
        .iter()
        .map(|process| {
            let mut entries = Map::new();
            entries.insert("pid".to_string(), serde_json::json!(i64::from(process.pid)));
            entries.insert("name".to_string(), serde_json::json!(&process.name));
            // Absent rather than null for a field the machine could not supply: `null`
            // would be a fourth state to interpret, and the reader already has the notes.
            if let Some(path) = &process.path {
                entries.insert("path".to_string(), serde_json::json!(path));
            }
            if let Some(cmdline) = &process.cmdline {
                entries.insert("cmdline".to_string(), serde_json::json!(cmdline));
            }
            Value::Object(entries)
        })
        .collect();

    let notes: Vec<Value> = listing
        .notes
        .iter()
        .map(|value| serde_json::json!(value))
        .collect();
    let complete = match listing.incomplete() {
        crate::process::Incomplete::No => "no",
        crate::process::Incomplete::UnreadableLines => "unreadable-lines",
        crate::process::Incomplete::NotEnumerated => "not-enumerated",
        crate::process::Incomplete::MissingFields => "missing-fields",
    };

    serde_json::json!({
        "problem": listing.problem,
        "missing_fields": listing.missing_fields,
        "processes": Value::Array(processes),
        "count": listing.processes.len() as i64,
        "total": listing.total as i64,
        "unreadable": listing.unreadable as i64,
        "truncated": listing.truncated,
        "complete": complete == "no" && !listing.truncated,
        "incomplete_reason": complete,
        "scope": {"kind":"process_snapshot","filter":listing.applied},
        "applied": listing.applied.clone(),
        "notes": Value::Array(notes),
    })
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
        .and_then(Value::as_array)
        .ok_or_else(|| WireError::BadRequest("processes: missing or not an array".to_string()))?;

    let mut processes = Vec::new();
    for entry in processes_value {
        let pid = match entry.get("pid").map(Value::as_i64) {
            Some(Some(pid)) if pid >= 0 => pid as u32,
            _ => {
                return Err(WireError::BadRequest("a process needs a pid".to_string()));
            }
        };
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| WireError::BadRequest("a process needs a name".to_string()))?
            .to_string();
        processes.push(crate::process::Process {
            pid,
            name,
            path: entry
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string),
            cmdline: entry
                .get("cmdline")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    let number = |field: &str| -> Result<usize, WireError> {
        match value.get(field).map(Value::as_i64) {
            Some(Some(number)) if number >= 0 => Ok(number as usize),
            Some(_) => Err(WireError::BadRequest(format!("{field}: not a count"))),
            None => Err(WireError::BadRequest(format!("{field}: missing"))),
        }
    };

    let notes = value
        .get("notes")
        .and_then(Value::as_array)
        .map(|notes| {
            notes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(crate::process::Listing {
        problem: serde_json::from_value(value["problem"].clone())
            .map_err(|e| WireError::BadRequest(e.to_string()))?,
        missing_fields: serde_json::from_value(value["missing_fields"].clone())
            .map_err(|e| WireError::BadRequest(e.to_string()))?,
        processes,
        total: number("total")?,
        unreadable: number("unreadable")?,
        truncated: value.get("truncated").and_then(Value::as_bool) == Some(true),
        applied: value
            .get("applied")
            .cloned()
            .unwrap_or(Value::Object(Map::new())),
        notes,
    })
}

/// A listing, wrapped as a reply.
pub fn encode_ps_reply(listing: &crate::process::Listing) -> Value {
    reply_result(ps_listing_to_json(listing))
}

/// A kill request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field, for the same reason every other reader here
/// does: a caller that sent a kill it can read a complaint about is a caller that does not
/// have to work out why nothing happened. A request that names **neither** a pid nor a name
/// is refused rather than defaulted -- a kill with no target is not a request to do nothing,
/// it is a message that arrived wrong.
pub fn kill_request_from_json(value: &Value) -> Result<KillRequest, WireError> {
    let kind = value.get("kill").and_then(Value::as_str).ok_or_else(|| {
        WireError::BadRequest("kill: missing; one of \"pid\" or \"name\" is required".to_string())
    })?;

    let to_kill = match kind {
        "pid" => match value.get("pid").map(Value::as_i64) {
            Some(Some(pid)) if pid > 0 => crate::process::ToKill::Pid(pid as u32),
            Some(Some(pid)) => {
                return Err(WireError::BadRequest(format!(
                    "pid: {pid} is not a process identifier"
                )));
            }
            _ => {
                return Err(WireError::BadRequest(
                    "pid: missing or not a number".to_string(),
                ));
            }
        },
        "name" => {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| WireError::BadRequest("name: missing or not a string".to_string()))?
                .to_string();
            if value.get("contains").and_then(Value::as_bool) == Some(true) {
                crate::process::ToKill::Matching(name)
            } else {
                crate::process::ToKill::Name(name)
            }
        }
        other => {
            return Err(WireError::BadRequest(format!(
                "kill: {other:?} is not one of \"pid\" or \"name\""
            )));
        }
    };

    let force = value.get("force").and_then(Value::as_bool) == Some(true);
    let exclude = match value.get("exclude_name") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => {
            return Err(WireError::BadRequest(
                "exclude_name: not a string".to_string(),
            ));
        }
        None => None,
    };

    Ok(KillRequest {
        to_kill,
        force,
        candidates: filter_from_json(value)?,
        exclude,
    })
}

/// A kill report as the result of a reply.
///
/// `matched` is written even when it is zero, because it is the number that keeps an empty
/// list from being read as a clean machine -- the same argument as [`ps_listing_to_json`].
pub fn kill_report_to_json(report: &crate::process::KillReport) -> Value {
    let targets = |targets: &[crate::process::Target]| -> Value {
        Value::Array(
            targets
                .iter()
                .map(|target| {
                    serde_json::json!({
                        "pid": i64::from(target.pid),
                        "name": target.name,
                    })
                })
                .collect(),
        )
    };

    let notes: Vec<Value> = report
        .notes
        .iter()
        .map(|value| serde_json::json!(value))
        .collect();

    serde_json::json!({
        "matched": report.matched as i64,
        "killed": targets(&report.killed),
        "excluded": targets(&report.excluded),
        "failed": targets(&report.failed),
        "complete": report.complete(),
        "notes": Value::Array(notes),
    })
}

/// A kill report out of a reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal -- a caller that wants the reason
/// should match on [`Reply`] instead -- or when the result is not a report. A report missing
/// its counts is not defaulted, for the reason the listing's is not: an agent that did not
/// say how many processes it matched has not answered the question, and filling in zero
/// would report a clean machine on no evidence.
pub fn kill_report_from_reply(reply: &Reply) -> Result<crate::process::KillReport, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the kill request was refused".to_string(),
        ));
    };

    let matched = match value.get("matched").map(Value::as_i64) {
        Some(Some(number)) if number >= 0 => number as usize,
        _ => {
            return Err(WireError::BadRequest(
                "matched: missing or not a count".to_string(),
            ));
        }
    };

    let targets = |field: &str| -> Result<Vec<crate::process::Target>, WireError> {
        let entries = value
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| WireError::BadRequest(format!("{field}: missing or not an array")))?;

        entries
            .iter()
            .map(|entry| {
                let pid = match entry.get("pid").map(Value::as_i64) {
                    Some(Some(pid)) if pid >= 0 => pid as u32,
                    _ => return Err(WireError::BadRequest("a target needs a pid".to_string())),
                };
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| WireError::BadRequest("a target needs a name".to_string()))?
                    .to_string();
                Ok(crate::process::Target { pid, name })
            })
            .collect()
    };

    let notes = value
        .get("notes")
        .and_then(Value::as_array)
        .map(|notes| {
            notes
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    Ok(crate::process::KillReport {
        matched,
        killed: targets("killed")?,
        excluded: targets("excluded")?,
        failed: targets("failed")?,
        notes,
    })
}

/// A kill report, wrapped as a reply.
pub fn encode_kill_reply(report: &crate::process::KillReport) -> Value {
    reply_result(kill_report_to_json(report))
}

/// A spawn request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field. **Both fields are required**, and the output
/// path is not optional for the reason [`SpawnRequest::output`] gives: a child with the
/// agent's pipes keeps them open, which is the trap this request exists to avoid.
pub fn spawn_request_from_json(value: &Value) -> Result<SpawnRequest, WireError> {
    let command = crate::execution::CommandSpec::from_json(value)?;
    let cwd = crate::execution::cwd_from_json(value)?;

    let output = value
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::BadRequest("output: missing or not a string".to_string()))?
        .to_string();
    if output.trim().is_empty() {
        return Err(WireError::BadRequest("output: empty".to_string()));
    }

    let observe_ms = match value.get("observe_ms") {
        None => 500,
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= 5000)
            .ok_or_else(|| WireError::BadRequest("observe_ms: expected 0..=5000".into()))?,
    };
    Ok(SpawnRequest {
        observe_ms,
        command,
        cwd,
        output,
    })
}

/// A spawn report as JSON.
pub fn spawn_report_to_json(report: &SpawnReport) -> Value {
    serde_json::json!({
        "pid": i64::from(report.pid),
        "state":report.state,"observed_ms":report.observed_ms,"exit_code":report.exit_code,"error":report.error,"tail":text_to_json(&report.tail),
        "cwd": report.cwd, "output": report.output, "pid_kind": report.pid_kind,
        "command": report.command,
    })
}

/// A spawn report out of a reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal, or when there is no pid in it --
/// **a spawn that did not report a pid has not answered the question**, and filling in zero
/// would hand the caller a process identifier that belongs to the system idle process.
pub fn spawn_report_from_reply(reply: &Reply) -> Result<SpawnReport, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the spawn request was refused".to_string(),
        ));
    };

    let pid = match value.get("pid").map(Value::as_i64) {
        Some(Some(pid)) if pid > 0 && pid <= i64::from(u32::MAX) => pid as u32,
        _ => {
            return Err(WireError::BadRequest(
                "pid: missing, or not a process identifier".to_string(),
            ));
        }
    };

    // The command is echoed back rather than assumed: a caller that has just started
    // something has to be able to tell which of two spawns this reply is about.
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::BadRequest("command: missing or not a string".to_string()))?
        .to_string();

    let state = required_text(value, "state")?;
    let pid_kind = required_text(value, "pid_kind")?;
    if !matches!(
        state.as_str(),
        "started" | "running_at_observation" | "exited"
    ) || !matches!(pid_kind.as_str(), "program" | "shell")
        || (state != "exited" && !value["exit_code"].is_null())
        || (state == "exited" && value["exit_code"] != 0 && value["error"].is_null())
    {
        return Err(WireError::BadRequest(
            "inconsistent spawn observation".into(),
        ));
    }
    Ok(SpawnReport {
        state,
        observed_ms: value["observed_ms"]
            .as_u64()
            .ok_or_else(|| WireError::BadRequest("observed_ms missing".into()))?,
        exit_code: serde_json::from_value(value["exit_code"].clone())
            .map_err(|e| WireError::BadRequest(e.to_string()))?,
        error: serde_json::from_value(value["error"].clone())
            .map_err(|e| WireError::BadRequest(e.to_string()))?,
        tail: text_from_json(value, "tail")?,
        pid,
        command,
        cwd: required_text(value, "cwd")?,
        output: required_text(value, "output")?,
        pid_kind,
    })
}

/// A spawn report, wrapped as a reply.
pub fn encode_spawn_reply(report: &SpawnReport) -> Value {
    reply_result(spawn_report_to_json(report))
}

/// A grep request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field. **The pattern may be empty** -- that means
/// every line, which is how `tail` is spelled and is a legitimate thing to ask for -- and
/// the path may not, because a search with no file is not a request to do nothing.
pub fn grep_request_from_json(value: &Value) -> Result<GrepRequest, WireError> {
    let path = required_text(value, "path")?;
    let pattern = value
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Absent means case-sensitive, which is what a person searching a log for `ERROR`
    // expects; a caller that wants the other reading says so.
    let case_sensitive = value
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let direction = match value.get("mode").and_then(Value::as_str) {
        Some("last") => crate::search::Direction::Last,
        Some("first") | None => crate::search::Direction::First,
        Some(other) => {
            return Err(WireError::BadRequest(format!(
                "mode: {other:?} is not one of \"first\" or \"last\""
            )));
        }
    };

    let number = |field: &str, default: usize, most: usize| -> Result<usize, WireError> {
        match value.get(field).map(Value::as_i64) {
            Some(Some(number)) if number >= 0 && (number as usize) <= most => Ok(number as usize),
            Some(Some(number)) => Err(WireError::BadRequest(format!(
                "{field}: {number} is outside 0..={most}"
            ))),
            Some(_) => Err(WireError::BadRequest(format!("{field}: not a number"))),
            None => Ok(default),
        }
    };

    Ok(GrepRequest {
        path,
        pattern: crate::search::Pattern {
            text: pattern,
            case_sensitive,
        },
        direction,
        limit: crate::search::Limit {
            max_matches: number("max_matches", 12, MAX_SEARCH_MATCHES)?,
            context: number("context", 0, MAX_SEARCH_CONTEXT)?,
        },
    })
}

/// The most matches one grep will report.
pub const MAX_SEARCH_MATCHES: usize = 200;

/// The most context lines one grep will report on each side of a match.
pub const MAX_SEARCH_CONTEXT: usize = 20;

/// A tail request from JSON.
///
/// # Errors
///
/// [`WireError::BadRequest`] naming the field, and for a count past [`MAX_TAIL_LINES`] --
/// refused rather than clamped, the same way an out-of-range timeout is: a caller that asked
/// for ten thousand lines and got two thousand would not know it had.
pub fn tail_request_from_json(value: &Value) -> Result<TailRequest, WireError> {
    let path = required_text(value, "path")?;

    let count = match value.get("count").map(Value::as_i64) {
        Some(Some(count)) if count > 0 && (count as usize) <= MAX_TAIL_LINES => count as usize,
        Some(Some(count)) => {
            return Err(WireError::BadRequest(format!(
                "count: {count} is outside 1..={MAX_TAIL_LINES}"
            )));
        }
        Some(_) => return Err(WireError::BadRequest("count: not a number".to_string())),
        None => return Err(WireError::BadRequest("count: missing".to_string())),
    };

    let cursor = match value.get("cursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() <= crate::tail::CURSOR_BYTES => Some(s.clone()),
        _ => {
            return Err(WireError::BadRequest(
                "cursor: expected bounded text".into(),
            ));
        }
    };
    let content_bytes = match value.get("content_bytes") {
        None => crate::tail::PAGE_BYTES,
        Some(v) => v
            .as_u64()
            .filter(|n| (4..=crate::tail::PAGE_BYTES as u64).contains(n))
            .ok_or_else(|| WireError::BadRequest("content_bytes outside 4..=1048576".into()))?
            as usize,
    };
    Ok(TailRequest {
        path,
        count,
        cursor,
        content_bytes,
    })
}

/// A required string field, refused by name when it is missing or empty.
fn required_text(value: &Value, field: &'static str) -> Result<String, WireError> {
    let text = value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| WireError::BadRequest(format!("{field}: missing or not a string")))?
        .to_string();
    if text.trim().is_empty() {
        return Err(WireError::BadRequest(format!("{field}: empty")));
    }
    Ok(text)
}

/// A search result as JSON.
///
/// **`searched` and `problem` are always written**, because they are the fields that keep a
/// failed search from reading as "no matches" -- `docs/ROADMAP.md` M10's first lesson, and
/// the reason this is not just a list of lines. `total` is written as `null` when the scan
/// stopped early, which is the difference between "there are none" and "there may be more".
pub fn search_to_json(search: &crate::search::Search) -> Value {
    let lines: Vec<Value> = search
        .lines
        .iter()
        .map(|found| {
            let context = |lines: &[String]| -> Value {
                Value::Array(lines.iter().map(|value| serde_json::json!(value)).collect())
            };
            serde_json::json!({
                "number": found.number as i64,
                "text": found.text,
                "before": context(&found.before),
                "after": context(&found.after),
            })
        })
        .collect();

    serde_json::json!({
        "path": search.path,
        "count": search.lines.len(),
        "complete": search.searched && search.problem.is_none() && !search.truncated,
        "scope": search.scope,
        "lines": Value::Array(lines),
        "total": match search.total {
            Some(total) => serde_json::json!(total as i64),
            None => Value::Null,
        },
        "searched": search.searched,
        "problem": match &search.problem {
            Some(problem) => serde_json::json!(problem),
            None => Value::Null,
        },
        "truncated": search.truncated,
        "bytes_read": search.bytes_read as i64,
        "file_bytes": match search.file_bytes {
            Some(bytes) => serde_json::json!(bytes as i64),
            None => Value::Null,
        },
        "encoding": search.encoding.tag(),
        // What the count counts, so that a `tail` does not report itself as a search:
        // `more than 2 matches` for a request that asked for the last two lines is the kind
        // of small untruth a reader stops trusting the rest of the line over.
        "noun": match search.noun {
            crate::search::Noun::Matches => "matches",
            crate::search::Noun::Lines => "lines",
        },
    })
}

/// A search result out of a reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal, or when the result is not a search.
/// **`searched` is required and not defaulted**: an agent that did not say whether it looked
/// has not answered the question, and defaulting it to `true` would invent a clean log.
pub fn search_from_reply(reply: &Reply) -> Result<crate::search::Search, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest("the search was refused".to_string()));
    };

    let searched = value
        .get("searched")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            WireError::BadRequest("searched: missing, or not true or false".to_string())
        })?;

    let problem = match value.get("problem") {
        Some(Value::Null) | None => None,
        Some(problem @ Value::Object(_)) => Some(
            serde_json::from_value(problem.clone())
                .map_err(|e| WireError::BadRequest(format!("invalid problem: {e}")))?,
        ),
        Some(_) => {
            return Err(WireError::BadRequest(
                "problem: not a typed error".to_string(),
            ));
        }
    };

    let total = match value
        .get("total")
        .filter(|value| !value.is_null())
        .map(Value::as_i64)
    {
        None => None,
        Some(Some(total)) if total >= 0 => Some(total as usize),
        Some(_) => return Err(WireError::BadRequest("total: not a count".to_string())),
    };

    let entries = value
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| WireError::BadRequest("lines: missing or not an array".to_string()))?;

    let context = |entry: &Value, field: &str| -> Vec<String> {
        entry
            .get(field)
            .and_then(Value::as_array)
            .map(|lines| {
                lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    let mut lines = Vec::new();
    for entry in entries {
        let number = match entry.get("number").map(Value::as_i64) {
            Some(Some(number)) if number >= 0 => number as usize,
            _ => {
                return Err(WireError::BadRequest(
                    "a match needs a line number".to_string(),
                ));
            }
        };
        let text = entry
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| WireError::BadRequest("a match needs its text".to_string()))?
            .to_string();
        lines.push(crate::search::Match {
            number,
            text,
            before: context(entry, "before"),
            after: context(entry, "after"),
        });
    }

    let encoding = match value.get("encoding").and_then(Value::as_str) {
        Some(tag) => crate::search::Encoding::named(tag).ok_or_else(|| {
            WireError::BadRequest(format!("encoding: {tag:?} is not one this version knows"))
        })?,
        None => {
            return Err(WireError::BadRequest(
                "encoding: missing, and a reader has to know what the text was taken to be"
                    .to_string(),
            ));
        }
    };

    let number = |field: &str| -> u64 {
        match value.get(field).map(Value::as_i64) {
            Some(Some(number)) if number >= 0 => number as u64,
            _ => 0,
        }
    };

    Ok(crate::search::Search {
        scope: value
            .get("scope")
            .filter(|v| !v.is_null())
            .map(|v| {
                serde_json::from_value(v.clone())
                    .map_err(|e| WireError::BadRequest(format!("scope: {e}")))
            })
            .transpose()?,
        path: value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        lines,
        total,
        searched,
        problem,
        truncated: value.get("truncated").and_then(Value::as_bool) == Some(true),
        bytes_read: number("bytes_read"),
        file_bytes: match value.get("file_bytes").map(Value::as_i64) {
            Some(Some(bytes)) if bytes >= 0 => Some(bytes as u64),
            _ => None,
        },
        encoding,
        // Absent means a search, which is what this reply was before `tail` existed: a
        // reader that has the field and does not know it says "matches" is reading an older
        // answer to the same question.
        noun: match value.get("noun").and_then(Value::as_str) {
            Some("lines") => crate::search::Noun::Lines,
            _ => crate::search::Noun::Matches,
        },
    })
}

/// A search result, wrapped as a reply.
pub fn encode_search_reply(search: &crate::search::Search) -> Value {
    reply_result(search_to_json(search))
}

/// A directory listing as JSON.
///
/// **`found` is always written**, and it is the field that keeps an empty directory from
/// reading as a directory that is not there -- `docs/ROADMAP.md` M10's own requirement for
/// this capability ("a list *and* the counts that make an empty one readable"), and the same
/// argument as `searched` in [`search_to_json`].
pub fn ls_listing_to_json(listing: &crate::listing::Listing) -> Value {
    let entries: Vec<Value> = listing
        .entries
        .iter()
        .map(|entry| {
            let mut fields = Map::new();
            fields.insert("name".to_string(), serde_json::json!(&entry.name));
            fields.insert("dir".to_string(), Value::Bool(entry.dir));
            // Absent rather than null for a directory's size: `null` would be a second way to
            // spell "not applicable", and the reader already has `dir` to branch on.
            if let Some(size) = entry.size {
                fields.insert("size".to_string(), serde_json::json!(size as i64));
            }
            if let Some(modified) = entry.modified {
                fields.insert("modified".to_string(), serde_json::json!(modified));
            }
            Value::Object(fields)
        })
        .collect();

    serde_json::json!({
        "path": listing.path,
        "scope": {"path":listing.path,"recursive":false},
        "complete": listing.found && listing.problem.is_none() && !listing.truncated,
        "entries": Value::Array(entries),
        "count": listing.entries.len() as i64,
        "total": listing.total as i64,
        "truncated": listing.truncated,
        "found": listing.found,
        "problem": match &listing.problem {
            Some(problem) => serde_json::json!(problem),
            None => Value::Null,
        },
    })
}

/// A directory listing out of a reply.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the reply is a refusal, or when the result is not a
/// listing. **`found` is required and not defaulted**: an agent that did not say whether the
/// directory was there has not answered the question, and defaulting it to `true` would
/// invent an empty directory out of a failure to look.
pub fn ls_listing_from_reply(reply: &Reply) -> Result<crate::listing::Listing, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest("the listing was refused".to_string()));
    };

    let found = value
        .get("found")
        .and_then(Value::as_bool)
        .ok_or_else(|| WireError::BadRequest("found: missing, or not true or false".to_string()))?;

    let problem = match value.get("problem") {
        Some(Value::Null) | None => None,
        Some(problem @ Value::Object(_)) => Some(
            serde_json::from_value(problem.clone())
                .map_err(|e| WireError::BadRequest(format!("invalid problem: {e}")))?,
        ),
        Some(_) => {
            return Err(WireError::BadRequest(
                "problem: not a typed error".to_string(),
            ));
        }
    };

    let entries_value = value
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| WireError::BadRequest("entries: missing or not an array".to_string()))?;

    let mut entries = Vec::new();
    for entry in entries_value {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| WireError::BadRequest("an entry needs a name".to_string()))?
            .to_string();
        let dir = entry.get("dir").and_then(Value::as_bool) == Some(true);
        let size = match entry.get("size").map(Value::as_i64) {
            Some(Some(size)) if size >= 0 => Some(size as u64),
            Some(_) => {
                return Err(WireError::BadRequest(
                    "an entry's size is not a count".to_string(),
                ));
            }
            None => None,
        };
        let modified = match entry.get("modified").map(Value::as_i64) {
            Some(Some(seconds)) => Some(seconds),
            _ => None,
        };

        entries.push(crate::listing::Entry {
            name,
            dir,
            size,
            modified,
        });
    }

    let number = |field: &str| -> Result<usize, WireError> {
        match value.get(field).map(Value::as_i64) {
            Some(Some(number)) if number >= 0 => Ok(number as usize),
            Some(_) => Err(WireError::BadRequest(format!("{field}: not a count"))),
            None => Err(WireError::BadRequest(format!("{field}: missing"))),
        }
    };

    Ok(crate::listing::Listing {
        entries,
        total: number("total")?,
        truncated: value.get("truncated").and_then(Value::as_bool) == Some(true),
        path: value
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        found,
        problem,
    })
}

/// A directory listing, wrapped as a reply.
pub fn encode_ls_reply(listing: &crate::listing::Listing) -> Value {
    reply_result(ls_listing_to_json(listing))
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
pub fn reply_fits(reply: &Value, ceiling: usize) -> bool {
    (reply).to_string().len() <= ceiling
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
pub fn run_reply_too_large(outcome: &RunOutcome, ceiling: usize) -> Value {
    reply_failure(&crate::result::OperationError::new(
        crate::result::ErrorCode::ResultUnknown,
        "encode_reply",
        format!(
            "the command's output is too large to return: stdout was {} bytes and stderr {} \
         bytes, and a reply of at most {ceiling} bytes is what this agent can send. Write \
         the output to a file on the target and pull that instead",
            outcome.stdout.byte_count(),
            outcome.stderr.byte_count(),
        ),
    ))
}

/// The identity reply, as the agent builds it.
pub fn identity_to_json(name: &str, version: &str) -> Value {
    reply_result(crate::identity::Identity::named(name, version).to_json())
}

/// The name out of an identity result.
///
/// # Errors
///
/// [`WireError::BadRequest`] when the result is a refusal or holds no name -- the
/// two ways a caller can be handed something that is not an answer.
pub fn identity_from_json(reply: &Reply) -> Result<crate::identity::Identity, WireError> {
    let Reply::Result(value) = reply else {
        return Err(WireError::BadRequest(
            "the identity reply is a refusal".to_string(),
        ));
    };
    let identity: crate::identity::Identity = serde_json::from_value(value.clone())
        .map_err(|e| WireError::BadRequest(format!("invalid identity: {e}")))?;
    if identity.name.is_empty() || identity.version.is_empty() {
        return Err(WireError::BadRequest(
            "identity name and version must be nonempty".into(),
        ));
    }
    Ok(identity)
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
        Reply::Refused(reason) => Err(WireError::BadRequest(reason.to_string())),
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
pub fn parse_body(body: &[u8]) -> Result<Value, WireError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| WireError::BadRequest("the message is not UTF-8".to_string()))?;
    serde_json::from_str::<Value>(text).map_err(|error| WireError::BadRequest(error.to_string()))
}

/// Renders a transfer's outcome as the text a caller reads.
///
/// One line, in the order a person checks them: **where it landed, how big it is, what
/// it hashes to, and that the digest was compared.** The digest is the whole reason a
/// transfer reports anything -- a caller that is told "done" has no way to know the file
/// it now has is the one that was sent -- and the receiving side makes the comparison
/// before the real path is touched, so this line is printed only for a transfer that
/// matched.
///
/// **The trailing word is also an instruction not to repeat the work.** A bare digest
/// reads as an invitation to hash the file again at the other end, which buys a round
/// trip for a comparison that has already been made and raises a false alarm the moment
/// the file has changed in between -- which a log still being written does by definition.
///
/// The destination is passed in rather than read out of the manifest. On a push it is a
/// path on the other machine and on a pull a path on this one, and either way it is what
/// the caller asked for -- the manifest's own `path` field is the sender's answer to a
/// question the caller already answered.
pub fn render_transfer(outcome: &TransferOutcome, destination: &str) -> String {
    format!(
        "{destination}: {} bytes, sha256 {} (verified)",
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

    out.push_str(&format!("cwd {}\n", outcome.cwd));
    for (name, stream) in [("stdout", &outcome.stdout), ("stderr", &outcome.stderr)] {
        render_stream(&mut out, name, stream);
    }

    out.trim_end().to_string()
}

fn render_stream(out: &mut String, name: &str, stream: &Text) {
    if stream.as_str().is_empty() && !stream.is_truncated() && stream.decode_error().is_none() {
        return;
    }
    out.push_str(&format!("{name}:\n"));
    if stream.encoding() != "utf-8" || stream.is_truncated() {
        out.push_str(&format!(
            "  encoding: {}; retained {} of {} bytes\n",
            stream.encoding(),
            stream.retained_bytes(),
            stream.byte_count()
        ));
    }
    if stream.is_truncated() {
        out.push_str("  truncated: showing the output tail; earlier bytes were discarded\n");
    }
    if let Some(error) = stream.decode_error() {
        out.push_str(&format!("  decoding failed: {error}\n"));
    }
    if let Some(note) = stream.loss_note() {
        out.push_str(&format!("  {note}\n"));
    }
    out.push_str(stream.as_str());
    if !stream.as_str().ends_with('\n') {
        out.push('\n');
    }
}

/// Renders startup observations, paths and the captured log tail without claiming readiness.
pub fn render_spawn(report: &SpawnReport) -> String {
    let mut out = format!(
        "{} {} ({}; observed {} ms; exit {:?}; readiness not checked)\ncwd {}\noutput {}\n",
        report.state,
        report.pid,
        report.pid_kind,
        report.observed_ms,
        report.exit_code,
        report.cwd,
        report.output
    );
    if let Some(error) = &report.error {
        out.push_str(&format!(
            "{:?} during {}: {}\n",
            error.code, error.phase, error.message
        ));
    }
    render_stream(&mut out, "log tail", &report.tail);
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
