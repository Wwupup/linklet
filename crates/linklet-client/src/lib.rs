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
    Connection, ConnectionError, HkdfChannel, TransferFailure, describe as describe_file, send_body,
};
use linklet_core::auth::Token;
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
use linklet_core::frame::Kind;
use linklet_core::json;
use linklet_core::wire::{self, Reply, Request, RunOutcome, RunRequest, WireError};

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
    send_body(&mut connection, session.as_mut(), local, &manifest).map_err(transfer_failure)?;

    match read_reply(&mut connection, session.as_mut(), TRANSFER_ALLOWANCE)? {
        Reply::Result(value) => wire::transfer_outcome_from_json(&value)
            .map_err(|error| CallError::Protocol(error.to_string())),
        Reply::Refused(reason) => Err(CallError::Refused(reason)),
    }
}

/// Turns a transfer failure into the caller's failure.
///
/// A refusal by the agent arrives through [`Reply`] and not here; this is for the
/// failures that happen on this side of the socket -- an unreadable file, a file that
/// changed while it was being sent -- and for the ones that mean the connection went
/// away mid-transfer. Those are [`CallError::Transport`] or [`CallError::Protocol`],
/// because what they have in common is that the host does not know whether the file
/// landed.
fn transfer_failure(failure: TransferFailure) -> CallError {
    match failure {
        TransferFailure::Connection(error) => transport(error),
        TransferFailure::Filesystem { .. }
        | TransferFailure::Unsendable(_)
        | TransferFailure::Refused(_)
        | TransferFailure::Channel(_)
        | TransferFailure::NotAFile { .. } => CallError::Protocol(failure.to_string()),
    }
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
            wire::handshake_public_from_json(&value).map_err(|error| protocol(error.to_string()))?
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
