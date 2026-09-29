//! The host's side of the protocol: ask an agent to run something, and decode
//! what it says.
//!
//! Hand-written HTTP for the same reason the agent's server is: no dependencies,
//! and a general-purpose client would be a liability for a tool that makes a
//! handful of calls to a program it also wrote.
//!
//! # What it does not do
//!
//! - **No keep-alive.** One connection per call. It costs a handshake and
//!   removes a class of framing bug from a tool whose calls are seconds apart.
//! - **No redirects, no cookies, no proxies, no TLS.** Every one of those is a
//!   feature with a failure mode, and none of them is needed to talk to an agent
//!   on a LAN. Encryption is a real gap and is named as one in `docs/MCP.md`
//!   rather than half-built here.
//! - **No retry.** A caller that wants to try again can call again, and a
//!   transport that retries on its own turns one request into two commands that
//!   ran.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use linklet_adapters::HkdfChannel;
use linklet_core::auth::{TOKEN_HEADER, TOKEN_SCHEME, Token};
use linklet_core::channel::{EphemeralPublic, Handshake};
use linklet_core::json;
use linklet_core::wire::{self, RunOutcome, RunRequest};

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
    /// The reply was not HTTP this client accepts.
    Protocol(String),
    /// The agent answered with an error body.
    Refused(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadAddress(problem) => write!(f, "address: {problem}"),
            Self::Transport(problem) => write!(f, "{problem}"),
            Self::Protocol(problem) => write!(f, "protocol: {problem}"),
            Self::Refused(problem) => write!(f, "the agent refused: {problem}"),
        }
    }
}

impl std::error::Error for CallError {}

/// How long to wait for a connection, and then for the reply.
///
/// Two separate waits, because they fail for different reasons: a connection
/// timeout means the machine or the agent is not there, and a read timeout means
/// the agent took the request and did not come back. One number for both would
/// merge two diagnoses.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// Extra time on top of the command's own deadline before the host gives up.
///
/// The agent enforces the deadline; this is the allowance for the request to
/// travel and the reply to come back. A host that used exactly the command's
/// deadline would report a transport failure for a command the agent was about to
/// kill and describe, which would be a lie about what happened.
const REPLY_ALLOWANCE: Duration = Duration::from_secs(10);

/// An agent's address, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAddress {
    /// `host:port`, exactly as given.
    pub text: String,
    /// The secret to present, if the agent has been configured with one.
    ///
    /// Optional because an agent on a bench with no token is a real way to work,
    /// and making it required would mean inventing a token for the case where
    /// nobody cares. What is not optional is saying so: an agent the caller cannot
    /// authenticate against answers 401, and the message says the token was
    /// missing or wrong rather than leaving the caller to guess.
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
    // The token is the handshake's authentication. Without one there is no way to
    // tell the agent from someone sitting in front of it, so a sealed call is
    // refused rather than done unauthenticated -- an anonymous handshake would be
    // worse than none, because it looks like protection.
    let Some(token) = address.token.as_ref() else {
        return Err(CallError::BadAddress(
            "a sealed call needs a token: set LINKLET_TOKEN, or pass --agent with one".to_string(),
        ));
    };

    let read_budget = Duration::from_secs(request.timeout_seconds) + REPLY_ALLOWANCE;
    let mut reader = open(address, read_budget)?;

    // 1. The handshake. Our public key goes out; theirs comes back.
    let (ours, pending) = HkdfChannel
        .propose(token.expose().as_bytes())
        .map_err(|e| CallError::Protocol(format!("cannot begin a handshake: {e}")))?;
    let hello_body = json::write(&wire::handshake_to_json(ours.as_bytes()));
    let hello = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {length}\r\n\
         Connection: close\r\n\
         \r\n\
         {hello_body}",
        path = wire::HELLO_PATH,
        host = address.text,
        length = hello_body.len(),
    );

    // A handshake reply is JSON, so this is one of the two places a reply is text.
    let hello_reply = String::from_utf8(round_trip(&mut reader, &hello, REPLY_ALLOWANCE)?)
        .map_err(|e| CallError::Protocol(format!("the handshake reply is not text: {e}")))?;
    let hello_value = json::parse(&hello_reply).map_err(|e| CallError::Protocol(e.to_string()))?;
    let theirs = wire::handshake_public_from_json(&hello_value)
        .map_err(|e| CallError::Protocol(e.to_string()))?;
    let theirs =
        EphemeralPublic::from_bytes(theirs).map_err(|e| CallError::Protocol(e.to_string()))?;

    // The initiator's half. This is where the private key for this handshake stops
    // being needed, and it is dropped with `pending` -- which is what forward
    // secrecy consists of.
    let mut session = pending
        .finish(&theirs)
        .map_err(|e| CallError::Protocol(format!("the handshake did not complete: {e}")))?;

    // 2. The command, sealed under the session the handshake produced.
    let body = json::write(&wire::run_request_to_json(request));
    let sealed = session
        .seal(body.as_bytes())
        .map_err(|e| CallError::Protocol(format!("cannot seal the request: {e}")))?;
    // The head is text and the body is the sealed bytes, written after it. The two
    // cannot be one string any more, and that is the point: a body that can only be
    // text has to be hex to carry ciphertext, which doubles every transfer.
    let head = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         {credential}\
         Content-Type: application/octet-stream\r\n\
         Content-Length: {length}\r\n\
         Connection: close\r\n\
         \r\n",
        path = wire::RUN_PATH,
        host = address.text,
        credential = authorization_line(address),
        length = sealed.len(),
    );
    reader
        .get_mut()
        .write_all(head.as_bytes())
        .and_then(|()| reader.get_mut().write_all(&sealed))
        .and_then(|()| reader.get_mut().flush())
        .map_err(|e| CallError::Transport(format!("cannot send the request: {e}")))?;
    // The request was written by hand above, so this reads its reply rather than
    // sending one. `read_reply` raises on any status but 200, carrying the agent
    // own words, so a body that arrives here is a result and not a refusal.
    reader
        .get_ref()
        .set_read_timeout(Some(read_budget))
        .map_err(|e| CallError::Transport(e.to_string()))?;
    let sealed_reply = read_reply(&mut reader)?;

    let plaintext = session
        .open(&sealed_reply)
        .map_err(|e| CallError::Protocol(format!("the sealed reply did not open: {e}")))?;
    let text = String::from_utf8(plaintext)
        .map_err(|e| CallError::Protocol(format!("the sealed reply is not text: {e}")))?;

    wire::decode_run_reply(&text).map_err(|e| CallError::Protocol(e.to_string()))
}

/// Asks an agent which agent it is.
///
/// The cheapest call in the protocol and the one a caller makes first: it needs
/// no arguments, changes nothing, and answers the question "is there an agent
/// here" with a fact rather than an inference from whether a port is open.
pub fn identity(address: &AgentAddress) -> Result<String, CallError> {
    // `get` turns the bytes into text, because every reply on that path is JSON. The
    // conversion lives there rather than here so that it happens once for every GET.
    let reply = get(address, wire::IDENTITY_PATH)?;
    let value = json::parse(&reply).map_err(|e| CallError::Protocol(e.to_string()))?;
    value
        .get_str("name")
        .map(str::to_string)
        .ok_or_else(|| CallError::Protocol("the identity reply has no name".to_string()))
}

/// The credential line, or nothing when there is no token.
///
/// Built in one place so that the two request builders cannot disagree about the
/// header name or the scheme -- a client that sent a bare token on one path and a
/// `Bearer` one on the other would work until an agent stopped accepting one of
/// them.
fn authorization_line(address: &AgentAddress) -> String {
    match &address.token {
        // A raw string, so that `\r\n` here is the escape the format machinery
        // understands rather than two literal backslashes. Written the other way
        // this produced a header glued to the next one, which the agent read as
        // no credential at all and refused with a 401 -- a wrong answer that
        // looked exactly like a wrong token.
        Some(token) => format!("{TOKEN_HEADER}: {TOKEN_SCHEME}{}\r\n", token.expose()),
        None => String::new(),
    }
}

/// One `GET`, and the body that came back.
fn get(address: &AgentAddress, path: &str) -> Result<String, CallError> {
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {}\r\n\
         {}\
         Connection: close\r\n\
         \r\n",
        address.text,
        authorization_line(address)
    );
    let bytes = exchange(address, &request, REPLY_ALLOWANCE)?;
    String::from_utf8(bytes).map_err(|e| CallError::Protocol(format!("the reply is not text: {e}")))
}

/// Sends one request and returns the body of the reply.
fn exchange(
    address: &AgentAddress,
    request: &str,
    read_budget: Duration,
) -> Result<Vec<u8>, CallError> {
    let mut reader = open(address, read_budget)?;
    round_trip(&mut reader, request, read_budget)
}

/// Connects, leaving a reader that more than one message can be sent on.
///
/// Split out from [`exchange`] because a sealed call needs **two** messages on one
/// connection: the handshake, and then the command. The reader has to survive
/// between them -- a fresh `BufReader` for the second message would lose whatever
/// the first read ahead into its buffer, which is the kind of bug that works on a
/// fast network and fails on a slow one.
fn open(address: &AgentAddress, read_budget: Duration) -> Result<BufReader<TcpStream>, CallError> {
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
    stream
        .set_read_timeout(Some(read_budget))
        .map_err(|e| CallError::Transport(e.to_string()))?;

    Ok(BufReader::new(stream))
}

/// Sends one request on an open connection and reads its reply.
fn round_trip(
    reader: &mut BufReader<TcpStream>,
    request: &str,
    read_budget: Duration,
) -> Result<Vec<u8>, CallError> {
    reader
        .get_ref()
        .set_read_timeout(Some(read_budget))
        .map_err(|e| CallError::Transport(e.to_string()))?;

    reader
        .get_mut()
        .write_all(request.as_bytes())
        .map_err(|e| CallError::Transport(format!("cannot send the request: {e}")))?;
    reader
        .get_mut()
        .flush()
        .map_err(|e| CallError::Transport(format!("cannot send the request: {e}")))?;

    read_reply(reader)
}

/// Reads a reply: status line, headers, then exactly the declared body length.
///
/// Reads the body by length rather than to the end of the stream. The difference
/// matters when a peer keeps the connection open: reading to the end would block
/// until the peer decides to close, and with no read timeout that is a hang. The
/// length is there in the reply; using it is why the client does not need to
/// trust the server to hang up.
fn read_reply(reader: &mut BufReader<TcpStream>) -> Result<Vec<u8>, CallError> {
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| CallError::Transport(format!("no reply: {e}")))?;
    if status_line.trim().is_empty() {
        return Err(CallError::Transport(
            "the agent closed the connection without answering".to_string(),
        ));
    }

    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| CallError::Protocol(format!("no status in {status_line:?}")))?;

    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| CallError::Transport(format!("headers: {e}")))?;
        if read == 0 {
            return Err(CallError::Protocol(
                "the reply ended inside its headers".to_string(),
            ));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().ok();
        }
    }

    let length = content_length.ok_or_else(|| {
        // Named rather than read-until-close: a reply without a length is one
        // this client cannot read without trusting the peer to hang up, and it
        // says so instead of blocking.
        CallError::Protocol("the reply declares no content-length".to_string())
    })?;

    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .map_err(|e| CallError::Transport(format!("the reply body was cut short: {e}")))?;
    // Bytes. This used to be `from_utf8_lossy`, which replaces anything that is not
    // UTF-8 with U+FFFD -- a silent modification of a reply, in the place where a
    // silent modification is least acceptable. A sealed reply is not text at all, so
    // the lossy conversion would have corrupted exactly the bodies that matter.

    if status == 200 {
        return Ok(body);
    }

    // Anything else is the agent saying it could not answer. The body carries
    // the reason, and pulling it out here means the caller does not have to know
    // the error shape to report it -- but it is not decoded further, because an
    // error body is a message and the caller quotes it rather than acting on its
    // fields.
    let reason = std::str::from_utf8(&body)
        .ok()
        .and_then(|text| json::parse(text).ok())
        .and_then(|value| value.get_str("error").map(str::to_string))
        // A refusal the agent did not phrase as JSON is still a refusal, and the
        // caller wants to read it. Lossy here is deliberate: this is a message for a
        // person, and the alternative is discarding it for being badly encoded.
        .unwrap_or_else(|| String::from_utf8_lossy(&body).into_owned());

    Err(CallError::Refused(format!("HTTP {status}: {reason}")))
}

/// Turns a call failure into the text an agent reads.
///
/// Separate from [`std::fmt::Display`] for [`CallError`] because this is the
/// thing that goes in a tool result: it starts with "could not reach", which is
/// the fact that decides what the reader does next. A caller must not have to
/// work out from prose whether the command ran.
pub fn render_call_error(error: &CallError) -> String {
    match error {
        CallError::BadAddress(problem) => format!("bad address: {problem}"),
        CallError::Transport(problem) => format!("could not reach the agent: {problem}"),
        CallError::Protocol(problem) => format!("the agent's reply was not understood: {problem}"),
        CallError::Refused(problem) => format!("the agent refused the request: {problem}"),
    }
}
// A body that reached here is a result: `read_reply` raises on any status
// other than 200, carrying the agent's own words. So this used to decide
// "is it hex" to tell a sealed result from a refusal, which was a guess about
// the shape of a body when the status line already said which it was.
