//! The agent's HTTP, written by hand because the agent must be one file with no
//! dependencies.
//!
//! This is a server for exactly two paths and one method, and it is written as
//! narrowly as that allows. A general-purpose HTTP implementation would be a
//! liability here: every feature it had would be a feature this agent has to be
//! trusted not to misuse.
//!
//! # What it refuses on purpose
//!
//! - **Any method but `POST` on the run path, `GET` on the identity path.** A
//!   server that accepts what it does not document is a server whose surface is
//!   larger than its documentation, which is the failure this whole project is a
//!   reaction to.
//! - **Chunked transfer-encoding.** `Content-Length` or nothing: a request with
//!   no length is refused rather than read until the socket closes, because
//!   "until the socket closes" is how a peer that never closes occupies a thread
//!   forever.
//! - **Keep-alive.** One request per connection. It costs a connection per call
//!   and removes a whole class of framing bug from a tool that makes a handful of
//!   calls at a time.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

use linklet_core::auth::{self, Token};
use linklet_core::json;
use linklet_core::wire::{self, WireError};

/// The largest request body the agent will read.
///
/// A command line and a number. Anything larger is not a request this protocol
/// has a meaning for, and reading it anyway is how a peer gets to decide how much
/// memory the agent spends.
const MAX_BODY: usize = 64 * 1024;

/// One request, as far as the agent needs to understand it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET` or `POST`, as sent.
    pub method: String,
    /// The path, with any query string already removed.
    pub path: String,
    /// The body, or empty.
    pub body: String,
    /// The `Authorization` header, if it was sent.
    pub authorization: Option<String>,
}

/// Why a request could not be read at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The request line or headers were not HTTP this server accepts.
    Malformed(String),
    /// The body was not the declared length, or there was none.
    Body(String),
    /// A read from the socket failed.
    Socket(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(problem) => write!(f, "malformed request: {problem}"),
            Self::Body(problem) => write!(f, "body: {problem}"),
            Self::Socket(problem) => write!(f, "socket: {problem}"),
        }
    }
}

/// Reads one request from a connection.
///
/// # Errors
///
/// [`ReadError`], always with something to look at. The peer is another program
/// and whoever debugs it is reading a log on a machine they may not be sitting
/// at.
pub fn read_request(stream: &TcpStream) -> Result<Request, ReadError> {
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|e| ReadError::Socket(e.to_string()))?;

    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| ReadError::Malformed("no method".to_string()))?
        .to_string();
    let target = parts
        .next()
        .ok_or_else(|| ReadError::Malformed("no path".to_string()))?
        .to_string();

    // The query string is dropped rather than parsed: no path in this protocol
    // takes one, and silently ignoring it is better than pretending to support
    // it. A caller that sends one gets the same answer as without, which is
    // honest, because there is nothing it could have meant.
    let path = target.split('?').next().unwrap_or("").to_string();

    let mut content_length: Option<usize> = None;
    let mut authorization: Option<String> = None;

    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .map_err(|e| ReadError::Socket(e.to_string()))?;
        if read == 0 {
            return Err(ReadError::Malformed(
                "headers ended without a blank line".to_string(),
            ));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }

        let Some((name, value)) = line.split_once(':') else {
            return Err(ReadError::Malformed(format!(
                "header without a colon: {line:?}"
            )));
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();

        match name.as_str() {
            "content-length" => {
                content_length = Some(value.parse().map_err(|_| {
                    ReadError::Malformed(format!("content-length is not a number: {value:?}"))
                })?);
            }
            "authorization" => authorization = Some(value.to_string()),
            // Named rather than ignored, so that a caller using it learns now
            // instead of through a body that never arrives.
            "transfer-encoding" => {
                return Err(ReadError::Malformed(
                    "transfer-encoding is not supported; send content-length".to_string(),
                ));
            }
            _ => {}
        }
    }

    let body = match content_length {
        None | Some(0) => String::new(),
        Some(length) => {
            if length > MAX_BODY {
                return Err(ReadError::Body(format!(
                    "{length} bytes is more than the {MAX_BODY} this agent reads"
                )));
            }
            let mut buffer = vec![0u8; length];
            reader
                .read_exact(&mut buffer)
                .map_err(|e| ReadError::Body(format!("{length} bytes declared, {e}")))?;
            String::from_utf8_lossy(&buffer).into_owned()
        }
    };

    Ok(Request {
        method,
        path,
        body,
        authorization,
    })
}

/// Writes a reply and closes.
///
/// `Connection: close` on every reply, because this server does not keep
/// connections alive and a peer that assumed otherwise would wait for a second
/// response that is never coming.
pub fn write_reply(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// What the agent answers for one request.
///
/// Pure: it takes a parsed request and returns a status and a body. The socket is
/// somebody else's problem, which is what lets every routing and protocol case be
/// tested without binding a port.
pub fn answer(request: &Request, expected: &Token) -> (u16, String) {
    // Authentication first, before the path is even looked at. A request that is
    // not from someone who knows the secret gets the same answer whatever it was
    // asking for, so the reply cannot be used to discover which paths exist.
    let presented = request
        .authorization
        .as_deref()
        .and_then(auth::token_from_header);
    let authorized = presented.is_some_and(|token| auth::token_matches(expected.expose(), token));
    if !authorized {
        return (auth::UNAUTHORIZED, json::write(&auth::unauthorized_body()));
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", path) if path == wire::IDENTITY_PATH => (
            200,
            json::write(&linklet_core::object! {
                "name" => "linklet-agent",
                "version" => env!("CARGO_PKG_VERSION"),
            }),
        ),

        ("POST", path) if path == wire::RUN_PATH => {
            let value = match json::parse(&request.body) {
                Ok(value) => value,
                Err(error) => {
                    return (
                        400,
                        json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                            error.to_string(),
                        ))),
                    );
                }
            };

            match wire::run_request_from_json(&value) {
                Ok(run_request) => {
                    let outcome = crate::execute::run(&run_request);
                    (200, wire::encode_run_reply(&outcome))
                }
                Err(error) => (400, json::write(&wire::wire_error_to_json(&error))),
            }
        }

        // A known path with the wrong method is told apart from an unknown path,
        // because they are different mistakes: one is a client bug and the other
        // is a client talking to the wrong program.
        (_, path) if path == wire::RUN_PATH || path == wire::IDENTITY_PATH => (
            405,
            json::write(&wire::wire_error_to_json(&WireError::BadRequest(format!(
                "{} is not a method this path answers",
                request.method
            )))),
        ),

        (_, path) => (
            404,
            json::write(&wire::wire_error_to_json(&WireError::NoSuchPath(
                path.to_string(),
            ))),
        ),
    }
}

/// Serves one connection: read, answer, close.
///
/// Errors are answered rather than logged and dropped. A peer that gets nothing
/// back has to guess whether the agent is slow, dead, or refusing, and guessing
/// is what this project exists to remove.
pub fn serve_connection(mut stream: TcpStream, expected: &Token) {
    let (status, body) = match read_request(&stream) {
        Ok(request) => answer(&request, expected),
        Err(error) => (
            400,
            json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                error.to_string(),
            ))),
        ),
    };

    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Unknown",
    };
    let _ = write_reply(&mut stream, status, reason, &body);
}
