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

use linklet_adapters::HkdfChannel;
use linklet_core::auth::{self, Token};
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
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
pub fn answer(
    request: &Request,
    expected: &Token,
    session: &mut Option<Box<dyn Sealed>>,
) -> (u16, String, bool) {
    // The handshake comes **before** authentication and carries no token, which
    // looks backwards and is not: the message holds nothing but a public key, so
    // there is nothing to protect, and refusing it would mean a caller could not
    // even begin. Everything a caller might want protected is behind the next
    // message, and that one does need the token.
    if request.method == "POST" && request.path == wire::HELLO_PATH {
        return match begin_session(request, expected, session) {
            Ok(reply) => (200, reply, true),
            Err(error) => (400, json::write(&wire::wire_error_to_json(&error)), false),
        };
    }

    // Authentication before the path is even looked at. A request that is not from
    // someone who knows the secret gets the same answer whatever it was asking for,
    // so the reply cannot be used to discover which paths exist.
    let presented = request
        .authorization
        .as_deref()
        .and_then(auth::token_from_header);
    let authorized = presented.is_some_and(|token| auth::token_matches(expected.expose(), token));
    if !authorized {
        return (
            auth::UNAUTHORIZED,
            json::write(&auth::unauthorized_body()),
            false,
        );
    }

    match (request.method.as_str(), request.path.as_str()) {
        ("GET", path) if path == wire::IDENTITY_PATH => (
            200,
            json::write(&linklet_core::object! {
                "name" => "linklet-agent",
                "version" => env!("CARGO_PKG_VERSION"),
            }),
            false,
        ),

        ("POST", path) if path == wire::RUN_PATH => {
            // A command with no handshake on this connection is refused rather
            // than run in the clear. Refusing is the whole point of the
            // exchange -- an agent that fell back to a plaintext path would leave
            // the fallback as the thing an attacker forces.
            let Some(session) = session.as_mut() else {
                return (
                    400,
                    json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                        "a command must be sealed: send POST /handshake on this connection first"
                            .to_string(),
                    ))),
                    false,
                );
            };

            // The body is hex of the sealed JSON, because the framing below this
            // is textual by construction. Doubling the size of a body that is
            // already capped is cheaper than making every path in the HTTP layer
            // carry bytes, and it has the small virtue of being unable to contain
            // a delimiter.
            let Ok(ciphertext) = wire::from_hex(&request.body) else {
                return (401, json::write(&auth::unauthorized_body()), false);
            };

            let Ok(plaintext) = session.open(&ciphertext) else {
                // The same refusal as a bad token, on purpose: whether the secret
                // was wrong or the bytes were altered is not a distinction the
                // caller needs and not one an attacker should be given.
                return (401, json::write(&auth::unauthorized_body()), false);
            };

            // A sealed message that does not decode as UTF-8 is authentic and
            // unusable, which is a different failure from a forged one -- but both
            // end with no command run, so both answer the same way.
            let Ok(text) = String::from_utf8(plaintext) else {
                return (
                    400,
                    json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                        "the sealed request is not UTF-8".to_string(),
                    ))),
                    false,
                );
            };

            let Ok(value) = json::parse(&text) else {
                return (
                    400,
                    json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                        "the sealed request is not JSON".to_string(),
                    ))),
                    false,
                );
            };

            let run_request = match wire::run_request_from_json(&value) {
                Ok(run_request) => run_request,
                Err(error) => {
                    return (400, json::write(&wire::wire_error_to_json(&error)), false);
                }
            };

            let outcome = crate::execute::run(&run_request);
            let reply = wire::encode_run_reply(&outcome);

            // The reply is sealed with the same session. A reply that went back in
            // the clear would leak the command's output, which is the part a
            // caller most wants kept.
            match session.seal(reply.as_bytes()) {
                Ok(sealed) => (200, wire::to_hex(&sealed), false),
                Err(_) => (
                    500,
                    json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                        "the reply could not be sealed".to_string(),
                    ))),
                    false,
                ),
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
            false,
        ),

        (_, path) => (
            404,
            json::write(&wire::wire_error_to_json(&WireError::NoSuchPath(
                path.to_string(),
            ))),
            false,
        ),
    }
}

/// Answers a handshake and leaves the session in `session`.
///
/// The two sides both generate a key pair for this handshake and **discard the
/// private half when it is done**, which is where forward secrecy comes from: an
/// attacker who records this exchange and later learns the token still needs
/// private keys that no longer exist.
///
/// The token is mixed into the key derivation, so it authenticates the exchange.
/// Without that an attacker who could rewrite traffic would complete a handshake
/// with each side and read everything; with it, the session they compute is not the
/// one either end built. `linklet-adapters/tests/handshake.rs` demonstrates that.
fn begin_session(
    request: &Request,
    expected: &Token,
    session: &mut Option<Box<dyn Sealed>>,
) -> Result<String, WireError> {
    let value = json::parse(&request.body)
        .map_err(|error| WireError::BadRequest(format!("the handshake is not JSON: {error}")))?;
    let peer = wire::handshake_public_from_json(&value)?;
    let peer = EphemeralPublic::from_bytes(peer)
        .map_err(|error| WireError::BadRequest(error.to_string()))?;

    let (ours, established) = HkdfChannel
        .accept(expected.expose().as_bytes(), &peer)
        .map_err(|error| WireError::BadRequest(error.to_string()))?;

    // Replacing rather than adding: one connection carries one handshake. A second
    // one would silently orphan the first session and leave the caller sealing with
    // a key the agent no longer holds, which fails in a way that looks like a wrong
    // token.
    *session = Some(established);

    Ok(json::write(&wire::handshake_to_json(ours.as_bytes())))
}

/// Serves one connection: read, answer, and possibly read again.
///
/// **Two messages at most, and the first must be the handshake.** The handshake
/// cannot protect itself -- the initiator cannot derive a key until it has the
/// responder's public key -- so the exchange has to happen before the command can
/// be sealed. Doing both on one connection is what keeps the agent stateless: a
/// session that had to survive between connections would need a table, an eviction
/// policy, and therefore a way to be exhausted.
///
/// Errors are answered rather than logged and dropped. A peer that gets nothing
/// back has to guess whether the agent is slow, dead, or refusing, and guessing is
/// what this project exists to remove.
pub fn serve_connection(mut stream: TcpStream, expected: &Token) {
    let mut session: Option<Box<dyn Sealed>> = None;

    loop {
        let (status, body, keep_going) = match read_request(&stream) {
            Ok(request) => answer(&request, expected, &mut session),
            Err(error) => (
                400,
                json::write(&wire::wire_error_to_json(&WireError::BadRequest(
                    error.to_string(),
                ))),
                false,
            ),
        };

        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            405 => "Method Not Allowed",
            500 => "Internal Server Error",
            _ => "Unknown",
        };
        if write_reply(&mut stream, status, reason, &body).is_err() {
            return;
        }

        // Only a handshake keeps the connection open, and only so that the sealed
        // request can follow on it. Everything else closes, because a connection
        // that stays open is a connection someone has to time out.
        if !keep_going {
            return;
        }
    }
}
