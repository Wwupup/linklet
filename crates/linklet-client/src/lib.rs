//! The host's side of the protocol: ask an agent to run something, and decode
//! what it says.
//!
//! Frames rather than HTTP, for the reason in `docs/decisions.md` D4: the sealing
//! already made HTTP's vocabulary meaningless, and this side reads replies from a
//! program this project also wrote. The socket work is in `linklet_adapters` and the
//! message shapes are in `linklet_core::wire`; what is left here is the order the
//! messages go in and what a failure means.
//!
//! # What it does not do
//!
//! - **No session between calls.** One connection per call, handshake included. It
//!   costs a round trip and it is what keeps the agent stateless.
//! - **No retry.** A caller that wants to try again can call again, and a
//!   transport that retries on its own turns one request into two commands that
//!   ran.
//! - **No identities.** One token, every caller. See `docs/ROADMAP.md` M9.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use linklet_adapters::{
    Connection, ConnectionError, HkdfChannel, TransferFailure, describe as describe_file,
    receive_body, send_body,
};
use linklet_core::auth::Token;
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
use linklet_core::frame::Kind;
use linklet_core::json;
use linklet_core::transfer::TransferError;
use linklet_core::wire::{
    self, GrepRequest, KillRequest, LsRequest, Reply, Request, RunOutcome, RunRequest,
    SpawnRequest, TailRequest, WireError,
};

/// Why a call could not be completed.
///
/// Every variant means the host learned **nothing** about whether the command
/// ran. That is the point of keeping this type apart from [`RunOutcome`] with a
/// non-zero exit code: "the tool could not reach the machine" and "the command
/// ran and failed" send whoever is reading to completely different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The address is not something this can connect to.
    BadAddress(String),
    /// The connection could not be made or was lost.
    Transport(String),
    /// The request went out and no reply came back inside the budget.
    ///
    /// Separate from [`CallError::Transport`] because the two are different facts
    /// and this one must not pretend to know more: nothing is listening, versus the
    /// agent took the request and did not answer. `docs/framing.md` is explicit about
    /// why a timeout says what it knows and no more.
    NoReply {
        /// How long it waited.
        millis: u64,
    },
    /// The reply was not a message this client understands.
    Protocol(String),
    /// The transfer could not happen for a reason on **this** side of the socket.
    ///
    /// Distinct from [`CallError::Transport`] and [`CallError::Protocol`], which are about
    /// the agent, and from [`CallError::Refused`], which is the agent saying no. A local
    /// path was not there, was not a file, was empty, or changed while it was being sent --
    /// and the message is that reason with **no prefix claiming to know where it came
    /// from**. The first real-machine run reported an empty local file as "the agent's
    /// reply was not understood: a transfer of no bytes is not one this protocol sends",
    /// which sends the reader to the network for a mistake in their own tree.
    Local(String),
    /// The agent answered, and the answer was no.
    Refused(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadAddress(problem) => write!(f, "address: {problem}"),
            Self::Transport(problem) => write!(f, "{problem}"),
            Self::NoReply { millis } => write!(f, "no reply within {millis} ms"),
            Self::Protocol(problem) => write!(f, "protocol: {problem}"),
            Self::Local(problem) => write!(f, "{problem}"),
            Self::Refused(problem) => write!(f, "the agent refused: {problem}"),
        }
    }
}

impl std::error::Error for CallError {}

/// How long to wait for a connection.
///
/// Separate from the read budget, because the two fail for different reasons: a
/// connection timeout means the machine or the agent is not there, and a read
/// timeout means the agent took the request and did not come back.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// The allowance for a handshake, and for the reply to anything that is not a
/// command.
///
/// A handshake is a round trip with no work behind it; ten seconds is generous and
/// still bounded, which is the whole point of having a number rather than waiting.
const HANDSHAKE_ALLOWANCE: Duration = Duration::from_secs(10);

/// Extra time on top of the command's own deadline before the host gives up.
///
/// The agent enforces the deadline; this is the allowance for the request to travel
/// and the reply to come back. A host that used exactly the command's deadline would
/// report a timeout for a command the agent was about to kill and describe, which
/// would be a lie about what happened.
const REPLY_ALLOWANCE: Duration = Duration::from_secs(10);

/// The budget for each message of a transfer.
///
/// **Per message, not for the whole transfer.** Four gibibytes is several thousand
/// chunks, so one deadline covering all of them would either be hours long -- and
/// therefore useless -- or would fail a large file on a slow link. What a stalled
/// transfer needs is for *a* chunk to be late, and that is what this bounds.
const TRANSFER_ALLOWANCE: Duration = Duration::from_secs(30);

/// An agent's address, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAddress {
    /// `host:port`, exactly as given.
    pub text: String,
    /// The secret to present.
    ///
    /// Optional in the type and required in practice: the token is what
    /// authenticates the handshake, so a call without one is refused before a socket
    /// is opened. It is optional here because an address is built in places that do
    /// not know about the environment -- the MCP surface reads the token from the
    /// environment once, at startup -- and a type that could not be built without a
    /// secret would have to be given a fake one.
    pub token: Option<Token>,
}

impl AgentAddress {
    /// Wraps text that looks like a `host:port`.
    ///
    /// The check is shape only -- there is a colon and something on each side.
    /// Whether the machine exists is what the connection attempt is for, and a
    /// resolver failure dressed up as a validation error would send the reader
    /// looking at their syntax instead of their network.
    pub fn new(text: impl Into<String>) -> Result<Self, CallError> {
        let text = text.into();
        let (host, port) = text.rsplit_once(':').ok_or_else(|| {
            CallError::BadAddress(format!("{text:?} has no port; expected host:port"))
        })?;

        if host.is_empty() {
            return Err(CallError::BadAddress(format!("{text:?} names no host")));
        }
        if port.parse::<u16>().is_err() {
            return Err(CallError::BadAddress(format!(
                "{text:?} has a port that is not a number"
            )));
        }

        Ok(Self { text, token: None })
    }

    /// The same address, presenting this secret.
    pub fn with_token(mut self, token: Token) -> Self {
        self.token = Some(token);
        self
    }
}

/// Runs a command on an agent and returns what it observed.
///
/// # Two messages on one connection, and why
///
/// A handshake cannot protect itself. The initiator cannot derive a session key
/// until it has the responder's public key, so the exchange has to happen *before*
/// the command can be sealed -- and the command is the part worth protecting. So
/// the handshake goes first, then the sealed request, both on the connection just
/// opened.
///
/// The alternative is one round trip with the request sealed under the token
/// alone, which leaves the command readable by anyone who later learns the token.
/// That is the wrong half to protect.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know whether the command
/// ran. A command that ran and failed comes back as
/// `Ok(RunOutcome { exit_code: Some(1), .. })`, which is the distinction the
/// protocol was built around.
pub fn run(address: &AgentAddress, request: &RunRequest) -> Result<RunOutcome, CallError> {
    // The budget belongs to the command: the agent enforces the deadline and this
    // side waits for it plus the allowance for the reply to travel.
    let budget = Duration::from_secs(request.timeout_seconds) + REPLY_ALLOWANCE;

    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Run(request.clone()),
        budget,
    )?;

    match reply {
        Reply::Result(value) => wire::run_outcome_from_json(&value)
            .map_err(|error| CallError::Protocol(error.to_string())),
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
    }
}

/// Asks an agent which agent it is.
/// Asks an agent which agent it is.
///
/// The cheapest call in the protocol and the one a caller makes first: it needs no
/// arguments, changes nothing, and answers the question "is there an agent here" with
/// a fact rather than an inference from whether a port is open.
///
/// It is sealed and it needs a token, which is worth being explicit about: the
/// expensive part is the handshake, and a plaintext version of this question would be a
/// version an unauthenticated caller could ask.
pub fn identity(address: &AgentAddress) -> Result<String, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Identity,
        HANDSHAKE_ALLOWANCE,
    )?;

    wire::identity_from_json(&reply).map_err(|error| CallError::Protocol(error.to_string()))
}

/// What an agent did when it was asked whether it is working.
///
/// Two arms and not a `Result<String, CallError>`, because **a refusal is an answer**. A
/// supervisor watching an agent needs to know that it is alive, and an agent that says no to a
/// request has demonstrated exactly that. Reporting a refusal as a failure would have a
/// supervisor restarting healthy processes -- and it would do it most often in the one
/// situation where restarting helps least, which is an operator holding the wrong secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// The agent answered with its identity.
    Answered(String),
    /// The agent answered, and the answer was no.
    Refused(String),
}

impl Liveness {
    /// How a monitoring script says it in one word.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Answered(_) => "answered",
            Self::Refused(_) => "refused",
        }
    }

    /// Why, whichever arm this is.
    pub fn detail(&self) -> &str {
        match self {
            Self::Answered(detail) | Self::Refused(detail) => detail,
        }
    }
}

/// Asks an agent whether it is working, by doing the smallest call this protocol has.
///
/// **`identity` rather than a bare connection**, because a socket tells you a process is
/// listening and nothing about whether it is doing anything: a wedged agent keeps its listening
/// socket open, so the kernel accepts connections into the backlog and anything built on a
/// connect check reports it healthy. This completes a handshake and reads a reply, so a wedged
/// agent fails it. `docs/ROADMAP.md` M10 is where that failure is written down -- the caller saw
/// a connect timeout rather than a refusal, because the process behind the port was gone.
///
/// # Errors
///
/// [`CallError`] for the cases where the agent did not speak: no connection, a lost one, a
/// timeout, or a reply that was not a message. **A refusal is not among them** -- see
/// [`Liveness`].
pub fn probe(address: &AgentAddress, budget: Duration) -> Result<Liveness, CallError> {
    let (mut connection, mut session) = begin(address, budget)?;
    match ask(
        &mut connection,
        session.as_mut(),
        &Request::Identity,
        budget,
    )? {
        // The same reader `identity` uses, so what counts as a well-formed identity is decided
        // in one place -- and a refusal never reaches it, because it is matched out first.
        //
        // Two arms and no catch-all: `Reply` has exactly these two shapes, so a third one added
        // later is a compile error here rather than a probe that quietly calls it a failure.
        reply @ Reply::Result(_) => wire::identity_from_json(&reply)
            .map(Liveness::Answered)
            .map_err(|error| CallError::Protocol(error.to_string())),
        Reply::Refused(reason) => Ok(Liveness::Refused(reason)),
    }
}

/// Copies one local file to a path under the agent's transfer root.
///
/// The file is hashed first, and the digest goes in the manifest: the receiving side
/// compares what it wrote against it before the file is renamed into place, so a
/// transfer that did not arrive intact leaves nothing behind under the real name. The
/// digest that comes back is the one the receiver computed, which is the evidence that
/// the comparison happened.
///
/// # One transfer per connection
///
/// The request **is** the manifest, and what follows it is the file. A connection that
/// carried a second request would give the reader a reason to hold state across them,
/// which is the shape `docs/transfer.md` T12 refuses.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know whether the file
/// landed. [`CallError::Refused`] carries the agent's own reason, and it is a refusal
/// rather than a transport failure: the transfer was made and the answer was no.
pub fn push(
    address: &AgentAddress,
    local: &std::path::Path,
    remote: &str,
) -> Result<wire::TransferOutcome, CallError> {
    // Described and checked before a socket is opened: a file that is empty, past the
    // ceiling, or unreadable is a local mistake, and making it look like a network one
    // would send the reader to the wrong machine.
    let manifest = describe_file(local, remote).map_err(transfer_failure)?;

    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    connection.set_budget(TRANSFER_ALLOWANCE);

    send_request(
        &mut connection,
        session.as_mut(),
        &Request::Push(manifest.clone()),
    )?;

    // **The receiver's answer to the manifest, read before the body is sent.** This is T14,
    // and it was found on a real machine rather than reasoned about: the sender has the
    // whole file ready, so without this it streams the body into a receiver that may already
    // have refused -- and the receiver's close, with those unread chunks in its queue, makes
    // Windows reset the connection, which destroys the refusal before this side reads it.
    // What the caller saw was "the agent closed the connection without answering".
    //
    // Two further replies are read on this connection: this one, and the transfer's result.
    // The default budget of two messages is the handshake and one reply, so it is raised here
    // rather than left to coincide.
    connection.set_message_limit(connection.messages_read() + 2);
    match read_reply(&mut connection, session.as_mut(), TRANSFER_ALLOWANCE)? {
        Reply::Refused(reason) => return Err(CallError::Refused(reason)),
        Reply::Result(value) => {
            let reply = Reply::Result(value);
            let accepted = wire::accepted_bytes_from_reply(&reply).map_err(|error| {
                CallError::Protocol(format!("the agent did not accept the manifest: {error}"))
            })?;
            // A receiver that accepted a different number has read a different manifest,
            // and finding that out here beats finding it out from a digest.
            if accepted != manifest.bytes {
                return Err(CallError::Protocol(format!(
                    "the agent accepted {accepted} bytes and {} were declared",
                    manifest.bytes
                )));
            }
        }
    }

    send_body(&mut connection, session.as_mut(), local, &manifest).map_err(transfer_failure)?;

    match read_reply(&mut connection, session.as_mut(), TRANSFER_ALLOWANCE)? {
        Reply::Result(value) => wire::transfer_outcome_from_json(&value)
            .map_err(|error| CallError::Protocol(error.to_string())),
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
    }
}

/// Asks an agent what is running on its machine.
///
/// The filter is applied **on the agent**, not here, and that is the point of it being a
/// request rather than a convention: a machine with hundreds of processes should send the
/// handful that match, and the caller should not have to receive the rest to find them.
///
/// **An empty listing is not an error and is not the same fact as a machine that could not
/// be read.** The answer carries the counts, the filter that was applied, and a note for
/// anything the machine could not supply; [`linklet_core::process::Listing::incomplete`] is
/// where a caller branches on that. This is the difference `docs/ROADMAP.md` M10 records,
/// and the reason the deploy loop can now ask "is the old build still running" before it
/// overwrites the file.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know what is running:
/// an unreachable agent, a reply that could not be read, or an agent that refused.
pub fn ps(
    address: &AgentAddress,
    filter: &linklet_core::process::Filter,
) -> Result<linklet_core::process::Listing, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Ps(filter.clone()),
        HANDSHAKE_ALLOWANCE,
    )?;

    wire::ps_listing_from_reply(&reply).map_err(|error| CallError::Protocol(error.to_string()))
}

/// Lists a path on an agent's machine, without moving anything.
///
/// **The answer distinguishes an empty directory from one that is not there.** Both come
/// back as no entries, and they are opposite facts: the first says the machine has no logs,
/// the second says nobody looked. [`linklet_core::listing::Listing::found`] is where a caller
/// branches, and the counts are beside it -- `docs/ROADMAP.md` M10's requirement that this be
/// "the same shape as `ps`".
///
/// A file lists as one entry, so "is it there, and how big is it" is the same call.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know what is in the directory.
/// **A path that is not there is not an error**: it is a listing whose `found` is false,
/// because that is a fact about the machine rather than about the call.
pub fn ls(
    address: &AgentAddress,
    request: &LsRequest,
) -> Result<linklet_core::listing::Listing, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Ls(request.clone()),
        HANDSHAKE_ALLOWANCE,
    )?;

    match reply {
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
        Reply::Result(_) => wire::ls_listing_from_reply(&reply)
            .map_err(|error| CallError::Protocol(error.to_string())),
    }
}

/// Searches a file on an agent's machine, without moving it.
///
/// **The answer is a search and not a list of lines**, and that is the point of it: it
/// carries how many matches there are, whether the scan stopped early, whether the file was
/// cut short, and which encoding the bytes were taken to be. A caller that received only the
/// matching lines could not tell "this log has no errors" from "that file could not be
/// read" -- `docs/ROADMAP.md` M10's first lesson, and the reason this does not return a
/// `Vec<String>`.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know what is in the file -- an
/// unreachable agent, or a reply that could not be read. **A file that could not be read on
/// the target is not an error**: it comes back as a search whose `searched` is false and
/// whose `problem` says why, because that is a fact about the machine rather than about the
/// call, and folding it into a transport failure would send the reader to the network.
pub fn grep(
    address: &AgentAddress,
    request: &GrepRequest,
) -> Result<linklet_core::search::Search, CallError> {
    read(address, Request::Grep(request.clone()))
}

/// Reads the last lines of a file on an agent's machine, without moving it.
///
/// The same answer shape as [`grep`] and for the same reason. It is the other half of "look
/// at the log": a pull moves the whole file to answer a question about its last few lines.
///
/// # Errors
///
/// As [`grep`].
pub fn tail(
    address: &AgentAddress,
    request: &TailRequest,
) -> Result<linklet_core::search::Search, CallError> {
    read(address, Request::Tail(request.clone()))
}

/// Sends one of the two reading requests and returns the search it answered with.
fn read(
    address: &AgentAddress,
    request: Request,
) -> Result<linklet_core::search::Search, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(&mut connection, session.as_mut(), &request, READ_ALLOWANCE)?;

    match reply {
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
        Reply::Result(_) => {
            wire::search_from_reply(&reply).map_err(|error| CallError::Protocol(error.to_string()))
        }
    }
}

/// How long a read of a file on a target may take.
///
/// Longer than a handshake, because the work behind it is up to sixteen mebibytes read,
/// sniffed, decoded and scanned -- and a file that is not UTF-8 is read a second time
/// through the target's own shell.
const READ_ALLOWANCE: Duration = Duration::from_secs(30);

/// Starts a program on an agent's machine that outlives this call.
///
/// **The difference from [`run`], and the reason both exist**: `run` waits for the command
/// and returns its output, so a program meant to keep running holds the call, the connection
/// and the agent's pipes with it. This starts the program with **its own output file** and
/// returns its pid immediately.
///
/// That pid plus a [`ps`] call is the whole of "did it stay up": the reply deliberately says
/// nothing about health, because at the moment it is sent nothing knows. The deploy loop is
/// those calls in that order -- start, look, and stop if it has to be stopped again.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know whether the program started,
/// including an agent that refused because the output file could not be created.
pub fn spawn(
    address: &AgentAddress,
    request: &SpawnRequest,
) -> Result<wire::SpawnReport, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Spawn(request.clone()),
        HANDSHAKE_ALLOWANCE,
    )?;

    // **The refusal is checked first, and this is not a formality.** An agent that could not
    // create the output file refuses, and reading that refusal as a malformed report reports
    // "the agent's reply was not understood" -- a sentence that sends the reader to look at
    // the protocol instead of at the path they typed. `kill` and `run` both do this; this
    // one was written without it, and the test that spawns onto a directory is what caught
    // it.
    match reply {
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
        Reply::Result(_) => wire::spawn_report_from_reply(&reply)
            .map_err(|error| CallError::Protocol(error.to_string())),
    }
}

/// Stops something on an agent's machine.
///
/// # The two refusals, and why they are refusals rather than reports
///
/// [`linklet_core::process::Refusal::NotForced`] for anything that can match more than one
/// process, and [`linklet_core::process::Refusal::WouldKillItself`] when the request would
/// stop the agent or the process that started it. Both arrive as [`CallError::Refused`] and
/// both mean **nothing was attempted**, which is a different answer from a report showing
/// nothing was killed: the first says the caller should decide again, the second says the
/// machine has nothing to do.
///
/// The guard runs on the agent and not here, because whether it applies depends on what is
/// running there and on which process is answering -- neither of which this side can see.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know what happened, including the
/// two refusals above.
pub fn kill(
    address: &AgentAddress,
    request: &KillRequest,
) -> Result<linklet_core::process::KillReport, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    // A kill lists the machine and then stops what it found, so it is allowed more time than
    // a handshake: `--name` on a busy machine is a `tasklist` plus a `taskkill` per match.
    let reply = ask(
        &mut connection,
        session.as_mut(),
        &Request::Kill(request.clone()),
        KILL_ALLOWANCE,
    )?;

    match reply {
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
        Reply::Result(_) => wire::kill_report_from_reply(&reply)
            .map_err(|error| CallError::Protocol(error.to_string())),
    }
}

/// How long a kill may take.
///
/// Longer than a handshake, because the work behind it is a process list and then one
/// `taskkill` per match -- and each of those waits for the process to be confirmed gone. Ten
/// seconds is generous for a handful of matches and still bounded, which is the point of
/// having a number.
const KILL_ALLOWANCE: Duration = Duration::from_secs(30);

/// Brings one file back from a path under the agent's transfer root.
///
/// The agent describes the file before sending a byte of it: the reply is a manifest,
/// with the size and the digest, and then the chunks. **The host checks that manifest
/// itself** -- the ceiling, and the shape of the digest -- because an agent that lied
/// about a size would otherwise be choosing how much this side agrees to receive, which
/// is the same mistake as trusting a caller's number in the other direction.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know whether the file
/// arrived. [`CallError::Refused`] carries the agent's own reason.
pub fn pull(
    address: &AgentAddress,
    remote: &str,
    local: &std::path::Path,
) -> Result<wire::TransferOutcome, CallError> {
    let (mut connection, mut session) = begin(address, HANDSHAKE_ALLOWANCE)?;
    connection.set_budget(TRANSFER_ALLOWANCE);

    send_request(
        &mut connection,
        session.as_mut(),
        &Request::Pull {
            path: remote.to_string(),
        },
    )?;

    // The manifest first, and it is checked before any chunk is read: the size against
    // this host's ceiling, and the digest's shape. `check_locally` and not `check`,
    // because where the file may be written is this side's own path and has nothing to
    // do with the agent's root.
    let manifest = match read_reply(&mut connection, session.as_mut(), TRANSFER_ALLOWANCE)? {
        Reply::Result(value) => {
            let manifest = wire::manifest_from_json(&value)
                .map_err(|error| CallError::Protocol(error.to_string()))?;
            manifest.check_locally().map_err(|error| {
                CallError::Protocol(format!(
                    "the agent described a transfer it may not send: {error}"
                ))
            })?;
            manifest
        }
        Reply::Refused(reason) => return Err(CallError::Refused(reason)),
    };

    receive_body(&mut connection, session.as_mut(), &manifest, local).map_err(transfer_failure)
}

/// Turns a transfer failure into the caller's failure.
///
/// A refusal by the agent arrives through [`Reply`] and not here. What is left is the
/// question of **whose side the failure is on**, because that is what decides where the
/// person reading it should look next.
fn transfer_failure(failure: TransferFailure) -> CallError {
    match failure {
        TransferFailure::Connection(error) => transport(error),

        // A fact about a file on this side: it was not there, was not a file, was empty,
        // was past the ceiling, or was shorter than the digest that was taken of it. The
        // agent is not in question and the message must not suggest it is.
        TransferFailure::Filesystem { .. }
        | TransferFailure::Unsendable(_)
        | TransferFailure::NotAFile { .. }
        | TransferFailure::Refused(TransferError::Short { .. }) => {
            CallError::Local(failure.to_string())
        }

        // And these are about the other end: a message that would not open, a running total
        // that did not add up, or bytes that do not hash to what it declared.
        TransferFailure::Channel(_) | TransferFailure::Refused(_) => {
            CallError::Protocol(failure.to_string())
        }
    }
}

/// The sentence a caller reads when the two ends speak different protocols.
///
/// **Both numbers and a direction, and the direction is the point.** "Protocol mismatch" leaves
/// a reader to work out which end to upgrade; naming which is older answers that. The message
/// is a `CallError::Protocol` rather than a variant of its own because there is nothing for a
/// caller to *branch* on -- the only sensible responses are to upgrade one end or to use an
/// older host, and both start with reading this.
///
/// `docs/VERSIONING.md` is the policy this implements, including why a mismatch is refused
/// rather than tolerated.
fn protocol_mismatch(agent: i64) -> String {
    let tool = wire::PROTOCOL_VERSION;
    let older = if agent < tool {
        "the agent"
    } else {
        "this tool"
    };
    let newer = if agent < tool {
        "this tool"
    } else {
        "the agent"
    };
    format!(
        "protocol {agent} on the agent and protocol {tool} here; \
         {older} is the older one, so upgrade {newer} or use a matching build \
         (see docs/VERSIONING.md)"
    )
}

/// Opens a connection and completes a handshake on it.
///
/// The initiator's half of the handshake is dropped inside this function, and that
/// is what forward secrecy consists of: the private key for this exchange no longer
/// exists once the session is built.
///
/// # Errors
///
/// [`CallError::BadAddress`] when there is no token to authenticate with, and the
/// variants for every way the connection or the exchange can fail.
fn begin(
    address: &AgentAddress,
    budget: Duration,
) -> Result<(Connection, Box<dyn Sealed>), CallError> {
    let Some(token) = address.token.as_ref() else {
        return Err(CallError::BadAddress(
            "a sealed call needs a token: set LINKLET_TOKEN, or pass --agent with one".to_string(),
        ));
    };

    let mut connection = connect(address, budget)?;

    // Our public key goes out in a hello frame, in the clear. There is nothing to
    // protect in it: it is half of a key pair whose other half never leaves this
    // process, and the token is mixed into the derivation rather than sent.
    let (ours, pending) = HkdfChannel
        .propose(token.expose().as_bytes())
        .map_err(|error| CallError::Protocol(format!("cannot begin a handshake: {error}")))?;
    let hello = json::write(&wire::handshake_to_json(ours.as_bytes()));
    connection
        .write_frame(Kind::Hello, hello.as_bytes())
        .map_err(transport)?;

    let frame = connection.read_frame(Kind::Hello).map_err(transport)?;
    let reply = decode_reply(&frame)?;
    let theirs = match reply {
        Reply::Result(value) => {
            let theirs = wire::handshake_public_from_json(&value)
                .map_err(|error| protocol(error.to_string()))?;

            // **The one place the two ends compare what they speak**, and it is deliberately a
            // check and not a gate. Refusing a mismatch outright would make this build unable to
            // talk to anything older than itself, which is the failure the number was added to
            // prevent -- an old agent that can serve every request a caller makes is a working
            // deployment, not an error. What it refuses is *silence*: without this, the caller
            // learns about the skew from whichever operation happens to need a feature the
            // agent does not have, if it ever asks for one, in the words "not one of ...".
            let peer = wire::handshake_protocol_from_json(&value)
                .map_err(|error| protocol(error.to_string()))?;
            if peer != wire::PROTOCOL_VERSION {
                return Err(CallError::Protocol(protocol_mismatch(peer)));
            }

            theirs
        }
        // The agent answers a hello it could not use with a refusal in the clear,
        // because there is no session yet to seal it with. That is the only reason
        // this arm exists.
        Reply::Refused(reason) => return Err(CallError::Refused(reason)),
    };
    let theirs =
        EphemeralPublic::from_bytes(theirs).map_err(|error| protocol(error.to_string()))?;

    let session = pending
        .finish(&theirs)
        .map_err(|error| CallError::Protocol(format!("the handshake did not complete: {error}")))?;

    Ok((connection, session))
}

/// Sends one request and reads the one reply.
///
/// The budget is applied to the read and not to the connection, so a command's own
/// deadline decides how long the reply is waited for and a handshake does not get
/// the same allowance by accident.
fn ask(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    request: &Request,
    budget: Duration,
) -> Result<Reply, CallError> {
    send_request(connection, session, request)?;
    read_reply(connection, session, budget)
}

/// Writes one sealed request.
fn send_request(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    request: &Request,
) -> Result<(), CallError> {
    let body = json::write(&wire::request_to_json(request));

    let mut sealed = Vec::new();
    session
        .seal_into(body.as_bytes(), &mut sealed)
        .map_err(|error| CallError::Protocol(format!("cannot seal the request: {error}")))?;
    connection
        .write_frame(Kind::Sealed, &sealed)
        .map_err(transport)
}

/// Reads one sealed reply, or a refusal that arrived unsealed.
fn read_reply(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    budget: Duration,
) -> Result<Reply, CallError> {
    connection.set_budget(budget);
    let frame = connection.read_frame(Kind::Sealed).map_err(transport)?;

    let mut plaintext = Vec::new();
    match session.open_into(&frame, &mut plaintext) {
        Ok(()) => decode_reply(&plaintext),
        Err(error) => {
            // The session did not open: the agent's token is not ours, or the bytes
            // were altered. The agent's refusal for exactly this case is sent
            // unsealed, because the session that would seal it is what failed, so it
            // is read here rather than reported as an authentication failure.
            //
            // This cannot manufacture a success: it is consulted only after the
            // session has already failed, and a refusal is never read as a result. The
            // most an attacker who rewrites traffic can do is replace one message
            // about a failed call with another message about a failed call.
            match decode_reply(&frame) {
                Ok(Reply::Refused(reason)) => Ok(Reply::Refused(reason)),
                _ => Err(CallError::Protocol(format!(
                    "the sealed reply did not open: {error}"
                ))),
            }
        }
    }
}

/// Reads one of the protocol's replies out of a frame body.
fn decode_reply(body: &[u8]) -> Result<Reply, CallError> {
    let value = wire::parse_body(body).map_err(|error| protocol(error.to_string()))?;
    wire::reply_from_json(&value).map_err(|error| protocol(error.to_string()))
}

/// Connects, leaving a connection ready for the handshake.
fn connect(address: &AgentAddress, budget: Duration) -> Result<Connection, CallError> {
    let mut addresses = address
        .text
        .to_socket_addrs()
        .map_err(|e| CallError::BadAddress(format!("cannot resolve {}: {e}", address.text)))?;

    let Some(target) = addresses.next() else {
        return Err(CallError::BadAddress(format!(
            "{} resolved to no addresses",
            address.text
        )));
    };

    let stream = TcpStream::connect_timeout(&target, CONNECT_BUDGET)
        .map_err(|e| CallError::Transport(format!("cannot reach {}: {e}", address.text)))?;

    Ok(Connection::with_budget(stream, budget))
}

/// Turns a connection failure into the caller's failure.
///
/// A timeout keeps its own name. It is the one refusal this layer makes that says
/// something the others do not -- that time passed -- and folding it into "could not
/// reach the agent" would claim knowledge about a machine that answered the
/// connection and then went quiet.
fn transport(error: ConnectionError) -> CallError {
    match error {
        ConnectionError::Timeout { millis } => CallError::NoReply { millis },
        ConnectionError::Ended => {
            CallError::Transport("the agent closed the connection without answering".to_string())
        }
        other => CallError::Transport(other.to_string()),
    }
}

/// A protocol failure, in one place so the wording does not drift.
fn protocol(problem: String) -> CallError {
    CallError::Protocol(problem)
}

/// Turns a call failure into the text an agent reads.
///
/// Separate from [`std::fmt::Display`] for [`CallError`] because this is the
/// thing that goes in a tool result: it starts with the fact that decides what
/// the reader does next, and a caller must not have to work out from prose whether
/// the command ran.
pub fn render_call_error(error: &CallError) -> String {
    match error {
        CallError::BadAddress(problem) => format!("bad address: {problem}"),
        CallError::Transport(problem) => format!("could not reach the agent: {problem}"),
        CallError::NoReply { millis } => format!("no reply within {millis} ms"),
        CallError::Protocol(problem) => format!("the agent's reply was not understood: {problem}"),
        // No prefix: the reason already names the file and the problem, and any prefix here
        // would be a claim about where the failure came from -- which is the mistake this
        // variant exists to stop making.
        CallError::Local(problem) => problem.clone(),
        CallError::Refused(problem) => format!("the agent refused the request: {problem}"),
    }
}

/// So that a caller that only needs to know what happened to a reply does not have
/// to import [`WireError`] as well.
impl From<WireError> for CallError {
    fn from(error: WireError) -> Self {
        CallError::Protocol(error.to_string())
    }
}
