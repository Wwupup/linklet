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

        Ok(Self { text })
    }
}

/// Runs a command on an agent and returns what it observed.
///
/// # Errors
///
/// [`CallError`] for anything that means the host does not know whether the
/// command ran. A command that ran and failed comes back as
/// `Ok(RunOutcome { exit_code: Some(1), .. })`.
pub fn run(address: &AgentAddress, request: &RunRequest) -> Result<RunOutcome, CallError> {
    let body = json::write(&wire::run_request_to_json(request));
    let reply = post(address, wire::RUN_PATH, &body, request.timeout_seconds)?;
    wire::decode_run_reply(&reply).map_err(|e| CallError::Protocol(e.to_string()))
}

/// Asks an agent which agent it is.
///
/// The cheapest call in the protocol and the one a caller makes first: it needs
/// no arguments, changes nothing, and answers the question "is there an agent
/// here" with a fact rather than an inference from whether a port is open.
pub fn identity(address: &AgentAddress) -> Result<String, CallError> {
    let reply = get(address, wire::IDENTITY_PATH)?;
    let value = json::parse(&reply).map_err(|e| CallError::Protocol(e.to_string()))?;
    value
        .get_str("name")
        .map(str::to_string)
        .ok_or_else(|| CallError::Protocol("the identity reply has no name".to_string()))
}

/// One `POST`, and the body that came back.
fn post(
    address: &AgentAddress,
    path: &str,
    body: &str,
    command_timeout_seconds: u64,
) -> Result<String, CallError> {
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        address.text,
        body.len()
    );

    let read_budget = Duration::from_secs(command_timeout_seconds) + REPLY_ALLOWANCE;
    exchange(address, &request, read_budget)
}

/// One `GET`, and the body that came back.
fn get(address: &AgentAddress, path: &str) -> Result<String, CallError> {
    let request = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {}\r\n\
         Connection: close\r\n\
         \r\n",
        address.text
    );
    exchange(address, &request, REPLY_ALLOWANCE)
}

/// Sends one request and returns the body of the reply.
fn exchange(
    address: &AgentAddress,
    request: &str,
    read_budget: Duration,
) -> Result<String, CallError> {
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

    let mut stream = TcpStream::connect_timeout(&target, CONNECT_BUDGET)
        .map_err(|e| CallError::Transport(format!("cannot reach {}: {e}", address.text)))?;
    stream
        .set_read_timeout(Some(read_budget))
        .map_err(|e| CallError::Transport(e.to_string()))?;

    stream
        .write_all(request.as_bytes())
        .map_err(|e| CallError::Transport(format!("cannot send the request: {e}")))?;
    stream
        .flush()
        .map_err(|e| CallError::Transport(format!("cannot send the request: {e}")))?;

    read_reply(stream)
}

/// Reads a reply: status line, headers, then exactly the declared body length.
///
/// Reads the body by length rather than to the end of the stream. The difference
/// matters when a peer keeps the connection open: reading to the end would block
/// until the peer decides to close, and with no read timeout that is a hang. The
/// length is there in the reply; using it is why the client does not need to
/// trust the server to hang up.
fn read_reply(stream: TcpStream) -> Result<String, CallError> {
    let mut reader = BufReader::new(stream);

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
    let body = String::from_utf8_lossy(&body).into_owned();

    if status == 200 {
        return Ok(body);
    }

    // Anything else is the agent saying it could not answer. The body carries
    // the reason, and pulling it out here means the caller does not have to know
    // the error shape to report it -- but it is not decoded further, because an
    // error body is a message and the caller quotes it rather than acting on its
    // fields.
    let reason = json::parse(&body)
        .ok()
        .and_then(|value| value.get_str("error").map(str::to_string))
        .unwrap_or(body);

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
