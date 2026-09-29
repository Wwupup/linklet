//! The agent's side of the conversation: the handshake, then one request.
//!
//! Frames rather than HTTP, and the argument for that is in `docs/decisions.md`
//! D4: every message was already sealed, so HTTP's methods, paths, headers and
//! status codes were vocabulary nobody read, carried by a parser that had to be
//! right about a grammar nobody used. What is left is a length and a payload, and
//! the four defences that need a socket live in `linklet_adapters`' connection.
//!
//! # What this file decides
//!
//! Which of the two shapes a connection is in, and what to answer when it is
//! neither. Everything specific to a request is in `linklet_core::wire`, everything
//! that runs a command is in `execute`, and everything about the socket is in the
//! connection. That leaves the policy below, which is short enough to read:
//!
//! - **the handshake first, and unsealed**, because there is no session yet
//! - **one sealed request**, and no second one: one transfer per connection is a
//!   rule the transfer will inherit (`docs/transfer.md` T12)
//! - **a refusal, not silence**, wherever the peer has shown it can read one
//!
//! # What it does not do
//!
//! **It does not keep a session between connections.** A session that had to
//! survive would need a table, an eviction policy, and therefore a way to be
//! exhausted -- so a caller that wants to run two commands opens two connections.
//! The cost is a handshake each time, which is a round trip on a LAN.

use std::net::TcpStream;
use std::time::Duration;

use linklet_adapters::{Connection, ConnectionError, HkdfChannel, receive_body};
use linklet_core::auth::{self, Token};
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
use linklet_core::frame::{FrameError, Kind};
use linklet_core::json::{self, Json};
use linklet_core::transfer::{Destination, Manifest};
use linklet_core::wire::{self, Request, WireError};

/// How long the agent waits for the first message of a connection.
///
/// A hello is a public key and arrives at once or not at all. Ten seconds is enough
/// for a slow LAN and short enough that a scanner opening ports and saying nothing
/// costs a thread for ten seconds each.
const HELLO_BUDGET: Duration = Duration::from_secs(10);

/// How long it waits for the sealed request, and for each later message.
///
/// Per read rather than for the whole connection, which is what `docs/framing.md`
/// specifies and what [`Connection`] implements: a peer that stops sending occupies
/// a thread for this long and then loses it.
///
/// **The cost of a per-read budget is stated rather than hidden**: a peer that sends
/// one byte every 29 seconds holds a thread indefinitely, because each read
/// succeeds. `docs/framing.md` lists that as a known gap -- it is not
/// distinguishable from a slow network, and a whole-connection deadline would be the
/// thing that closes it.
const MESSAGE_BUDGET: Duration = Duration::from_secs(30);

/// Serves one connection, and then closes it.
///
/// Errors are answered where the peer has shown it can read an answer, and the
/// connection is closed otherwise. Nothing is logged and dropped: a peer that gets
/// nothing back has to guess whether the agent is slow, dead or refusing, and
/// guessing is what this project exists to remove.
pub fn serve_connection(stream: TcpStream, expected: &Token, root: &Destination) {
    let mut connection = Connection::with_budget(stream, HELLO_BUDGET);

    let hello = match connection.read_frame(Kind::Hello) {
        Ok(body) => body,
        Err(error) => {
            refuse_unreadable(&mut connection, &error);
            return;
        }
    };

    let session = match begin_session(&hello, expected) {
        Ok((reply, session)) => {
            if connection
                .write_frame(Kind::Hello, json::write(&reply).as_bytes())
                .is_err()
            {
                return;
            }
            session
        }
        Err(reason) => {
            // The peer's hello arrived as a frame, so it speaks this protocol and can
            // read a refusal. **This is the one reply that is not sealed, and it
            // cannot be**: the session that would seal it is what failed to exist.
            // Nothing in it is worth protecting -- it is one sentence about a public
            // key that did not parse.
            let refusal = json::write(&wire::reply_refused(&reason));
            let _ = connection.write_frame(Kind::Hello, refusal.as_bytes());
            return;
        }
    };

    connection.set_budget(MESSAGE_BUDGET);
    answer_one(&mut connection, session, root);
}

/// Reads the one sealed request on a connection and answers it.
///
/// One request, because a connection that carried a second would give the reader a
/// reason to hold state across them -- `docs/transfer.md` T12. The connection's own
/// message budget is what enforces it: two messages is the default, and a request that
/// carries a transfer raises it by exactly the number of chunks the manifest declares.
fn answer_one(connection: &mut Connection, mut session: Box<dyn Sealed>, root: &Destination) {
    let Ok(body) = connection.read_frame(Kind::Sealed) else {
        return;
    };

    let mut plaintext = Vec::new();
    if session.open_into(&body, &mut plaintext).is_err() {
        // The session did not open, which means the caller's token was not ours or
        // the bytes were altered. Both get the same sentence.
        //
        // It goes back **unsealed**, and that is deliberate: the session that would
        // seal it is exactly what failed. The worst an attacker who can rewrite
        // traffic gains is turning one message about a failed call into another
        // message about a failed call, because a refusal is never read as a result.
        let refusal = json::write(&wire::reply_refused(auth::unauthorized_reason()));
        let _ = connection.write_frame(Kind::Sealed, refusal.as_bytes());
        return;
    }

    let reply = answer(connection, session.as_mut(), &plaintext, root);

    // Sealed with the same session, so the command's output -- the part a caller most
    // wants kept -- never crosses the network in the clear. One buffer, reused for
    // the seal rather than allocated per message: T10.
    let mut sealed = Vec::new();
    if session
        .seal_into(json::write(&reply).as_bytes(), &mut sealed)
        .is_ok()
    {
        let _ = connection.write_frame(Kind::Sealed, &sealed);
    }
}

/// Answers one request.
///
/// Takes the plaintext rather than a parsed request so that the two ways a message
/// can be unusable -- not JSON, and JSON of the wrong shape -- are refused here, in
/// one place, with the field named.
///
/// It takes the connection and the session too, and only a transfer needs them. The
/// alternative is a second dispatcher for the requests that stream, and a router split
/// in two is a router that eventually disagrees with itself about which request is
/// which.
fn answer(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    plaintext: &[u8],
    root: &Destination,
) -> Json {
    let request = wire::parse_body(plaintext).and_then(|value| wire::request_from_json(&value));

    match request {
        Ok(Request::Identity) => wire::identity_to_json("linklet-agent", env!("CARGO_PKG_VERSION")),
        Ok(Request::Run(run)) => wire::encode_run_reply(&crate::execute::run(&run)),
        Ok(Request::Push(manifest)) => receive(connection, session, &manifest, root),
        // A request the agent could not read is a refusal and not a dropped
        // connection: the caller learns which field was wrong instead of waiting for
        // a reply that is not coming.
        Err(error) => wire::reply_refused(&refusal_text(&error)),
    }
}

/// Receives one pushed file.
///
/// The manifest is checked **in full, before a chunk is read**: the size against the
/// ceiling (T3, and half of T11), the shape of the digest, and the path against the
/// configured root (T1, the most severe item in the document). Only then does the
/// receiving start, and `linklet_adapters`' transfer module is where the rest of the
/// numbered failures are defended -- the `.part`, the running total, the digest
/// comparison, and the rename.
///
/// Every failure is a **refusal** rather than a closed connection: the caller learns
/// which check failed, and by the time this returns a failure the `.part` has been
/// deleted and the real path was never touched.
fn receive(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    manifest: &Manifest,
    root: &Destination,
) -> Json {
    let target = match manifest.check(root) {
        Ok(target) => target,
        Err(error) => return wire::reply_refused(&error.to_string()),
    };

    match receive_body(connection, session, manifest, &target) {
        Ok(outcome) => wire::encode_transfer_reply(&outcome),
        Err(failure) => wire::reply_refused(&failure.to_string()),
    }
}

/// How a wire refusal reads to a person.
///
/// `WireError`'s own `Display` says "bad request: ...", which is HTTP's vocabulary
/// for a thing this protocol does not have. The sentence a caller reads should name
/// the problem and not the transport it arrived on.
fn refusal_text(error: &WireError) -> String {
    match error {
        WireError::BadRequest(problem) => problem.clone(),
    }
}

/// Answers a connection whose first frame could not be read.
///
/// The question is whether the peer is speaking this protocol at all, and the answer
/// is in what failed:
///
/// - **It sent a frame and the kind was wrong, or it sent more than this connection
///   agreed to read.** It speaks frames, so it gets a refusal with the reason in it.
/// - **The first byte was not the magic byte.** This is a service on the wrong port
///   -- an HTTP server, most likely -- or someone with `telnet`. Replying in a
///   language it does not read would be noise on someone else's connection, so the
///   answer is silence and a closed socket.
fn refuse_unreadable(connection: &mut Connection, error: &ConnectionError) {
    let speaks_frames = match error {
        ConnectionError::WrongKind { .. } | ConnectionError::Budget { .. } => true,
        ConnectionError::Frame(FrameError::NotThisProtocol { .. })
        | ConnectionError::Frame(FrameError::Truncated { .. }) => false,
        // Any other framing refusal means a header was read and understood well
        // enough to know what was wrong with it.
        ConnectionError::Frame(_) => true,
        ConnectionError::Timeout { .. }
        | ConnectionError::Ended
        | ConnectionError::Io(_)
        | ConnectionError::Unusable { .. } => false,
    };

    if speaks_frames {
        let refusal = json::write(&wire::reply_refused(&error.to_string()));
        let _ = connection.write_frame(Kind::Hello, refusal.as_bytes());
    }
}

/// Answers a handshake, and returns the reply to send and the session it produced.
///
/// The two sides both generate a key pair for this handshake and **discard the
/// private half when it is done**, which is where forward secrecy comes from: an
/// attacker who records this exchange and later learns the token still needs private
/// keys that no longer exist.
///
/// The token is mixed into the key derivation, so it authenticates the exchange.
/// Without that an attacker who could rewrite traffic would complete a handshake
/// with each side and read everything; with it, the session they compute is not the
/// one either end built. `linklet-adapters/tests/handshake.rs` demonstrates that.
///
/// # Errors
///
/// The reason a refusal would carry: a body that is not a hello, or a public key this
/// build cannot use. Returned as a sentence rather than a type because the caller
/// does one thing with either.
fn begin_session(hello: &[u8], expected: &Token) -> Result<(Json, Box<dyn Sealed>), String> {
    let value = wire::parse_body(hello).map_err(|error| refusal_text(&error))?;
    let peer = wire::handshake_public_from_json(&value).map_err(|error| refusal_text(&error))?;
    let peer = EphemeralPublic::from_bytes(peer).map_err(|error| error.to_string())?;

    let (ours, established) = HkdfChannel
        .accept(expected.expose().as_bytes(), &peer)
        .map_err(|error| error.to_string())?;

    Ok((
        wire::reply_result(wire::handshake_to_json(ours.as_bytes())),
        established,
    ))
}
