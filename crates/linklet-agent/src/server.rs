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
use std::time::{Duration, Instant};

use linklet_adapters::{
    Connection, ConnectionError, HkdfChannel, describe, receive_body, send_body,
};
use linklet_core::auth::{self, Token};
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
use linklet_core::frame::{FrameError, Kind};
use linklet_core::json::{self, Json};
use linklet_core::log::{Operation, Outcome};
use linklet_core::transfer::{Destination, Manifest};
use linklet_core::wire::{self, Request, WireError};

use crate::log::RequestLog;

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
///
/// `log` records one pair of lines per request, and is a no-op when the operator did not
/// ask for a log. It is threaded through rather than reached for, so that the one place
/// that decides what a line says (`linklet_core::log`) and the one place that writes it
/// (`crate::log`) are the only two things involved.
pub fn serve_connection(stream: TcpStream, expected: &Token, root: &Destination, log: &RequestLog) {
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
    answer_one(&mut connection, session, root, log);
}

/// Reads the one sealed request on a connection and answers it.
///
/// One request, because a connection that carried a second would give the reader a
/// reason to hold state across them -- `docs/transfer.md` T12. The connection's own
/// message budget is what enforces it: two messages is the default, and a request that
/// carries a transfer raises it by exactly the number of chunks the manifest declares.
///
/// **This is where the two log lines are written**, and the order matters more than the
/// wording: the first is written as soon as the request can be named, before a byte of the
/// answer exists, so a request that never finishes leaves a `->` with no `<-` and names
/// itself. The second is written after the reply is on the wire, because "answered" is a
/// fact about the socket rather than about the handler.
fn answer_one(
    connection: &mut Connection,
    mut session: Box<dyn Sealed>,
    root: &Destination,
    log: &RequestLog,
) {
    let Ok(body) = connection.read_frame(Kind::Sealed) else {
        // Nothing was read, so there is nothing to name: no operation arrived.
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
        //
        // Nothing is logged here either, and for the same reason there is nothing to
        // name: a message that will not open is not a request, and guessing an
        // operation from bytes that failed authentication would be inventing evidence.
        let refusal = json::write(&wire::reply_refused(auth::unauthorized_reason()));
        let _ = connection.write_frame(Kind::Sealed, refusal.as_bytes());
        return;
    }

    // A request this version cannot read is still a request that arrived, and the log
    // says `unknown` rather than dropping it -- see `Operation::Unknown`.
    let named = named_operation(&plaintext);
    let id = log.taken(named);
    let started = Instant::now();

    let reply = answer(connection, session.as_mut(), &plaintext, root);

    // Sealed with the same session, so the command's output -- the part a caller most
    // wants kept -- never crosses the network in the clear. One buffer, reused for
    // the seal rather than allocated per message: T10.
    match reply {
        Response::Sealed(reply) => {
            let wrote = write_reply(connection, session.as_mut(), &reply);
            // Three outcomes rather than two. A reply that was refused is the agent saying
            // no to something it understood, which is a fact about the request; a reply
            // that would not go is the agent failing to do something it had accepted,
            // which is a fact about the machine. A reader looking for what went wrong on
            // this target needs to tell those apart.
            let (outcome, reason) = match (wrote, refused(&reply)) {
                (false, _) => (
                    Outcome::Failed,
                    Some("the reply could not be sent".to_string()),
                ),
                (true, true) => (Outcome::Refused, refusal_reason(&reply)),
                (true, false) => (Outcome::Ok, None),
            };
            log.answered(id, named, outcome, elapsed_ms(started), reason);
        }
        // The request answered itself while it was being handled, because what answers
        // it is a stream rather than a message -- a pull sends the manifest and then the
        // file, and there is no third thing to say afterwards. Both of those went out, so
        // this is a request that was answered.
        Response::AlreadySent => {
            log.answered(id, named, Outcome::Ok, elapsed_ms(started), None);
        }
    }
}

/// Whether a reply is the agent saying no.
///
/// The refusal shape is `wire`'s own (`{"ok": false, "error": ...}`) and it is read back
/// here rather than carried alongside the reply, because a second field on every reply
/// would be a second thing to keep in step with the first.
fn refused(reply: &Json) -> bool {
    reply.get("ok").and_then(Json::as_bool) == Some(false)
}

/// The reason out of a refusal, for the log line.
///
/// Absent rather than invented when the field is not there: a refusal with no `error` is a
/// reply this version would not have sent, and a log line is not the place to guess at
/// what it meant.
fn refusal_reason(reply: &Json) -> Option<String> {
    reply.get_str("error").map(str::to_string)
}

/// Which operation a sealed body names, for the log.
///
/// Read out of the JSON rather than out of a parsed [`Request`], because the case that
/// matters most is the request that **could not be parsed**: a body that is not JSON, or
/// carries an `op` this version does not know, is exactly the traffic an operator wants to
/// find in a log, and a name taken from a successful parse would be missing for all of it.
///
/// Deliberately ignores the rest of the message. The command line is the caller's, it can
/// contain anything, and a log on someone else's machine is a file that outlives the
/// reason it was written.
fn named_operation(plaintext: &[u8]) -> Operation {
    wire::parse_body(plaintext)
        .ok()
        .and_then(|value| value.get_str("op").map(str::to_string))
        .and_then(|name| Operation::named(&name))
        .unwrap_or(Operation::Unknown)
}

/// Milliseconds since a request was taken, for the log line.
fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Seals one reply and writes it, saying whether it went.
///
/// One place for the seal-and-write, because three callers do it and each has to get the
/// same two things right: one buffer reused rather than allocated per message (T10), and
/// both results checked (`docs/framing.md` item 4).
fn write_reply(connection: &mut Connection, session: &mut dyn Sealed, reply: &Json) -> bool {
    let mut sealed = Vec::new();
    if session
        .seal_into(json::write(reply).as_bytes(), &mut sealed)
        .is_err()
    {
        return false;
    }
    connection.write_frame(Kind::Sealed, &sealed).is_ok()
}

/// What an answered request leaves to be sent.
///
/// An enum rather than an `Option<Json>` because the two cases are different facts and
/// not presence and absence: a transfer's reply may already be on the wire, and a
/// reader who saw `None` would have to guess whether that meant "nothing to say" or
/// "already said".
enum Response {
    /// Seal this reply and write it.
    Sealed(Json),
    /// Nothing more to send: the request was answered as it was handled.
    AlreadySent,
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
) -> Response {
    let request = wire::parse_body(plaintext).and_then(|value| wire::request_from_json(&value));

    match request {
        Ok(Request::Identity) => Response::Sealed(wire::identity_to_json(
            "linklet-agent",
            env!("CARGO_PKG_VERSION"),
        )),
        Ok(Request::Run(run)) => Response::Sealed(run_reply(&crate::execute::run(&run))),
        Ok(Request::Push(manifest)) => {
            Response::Sealed(receive(connection, session, &manifest, root))
        }
        Ok(Request::Pull { path }) => send(connection, session, &path, root),
        // The one request that looks at the machine rather than at the transfer root, and
        // the only one whose answer is mostly *about the answer*: a listing carries its own
        // counts and notes, so an empty list is readable as "nothing matched" rather than
        // as "nothing is running". `linklet_core::process` decides that; the adapter runs
        // `tasklist` and does not fail, reporting a machine it could not ask as a listing
        // with a note rather than as an error.
        Ok(Request::Ps(filter)) => Response::Sealed(wire::encode_ps_reply(
            &linklet_adapters::list_processes(&filter),
        )),
        // The one request that changes the machine rather than looking at it. The guard runs
        // in the adapter, **before `taskkill` is ever started**: a bulk match that was not
        // forced, and a request that would stop the agent itself, both come back as a
        // refusal with nothing attempted -- which is a different answer from a report saying
        // nothing was killed, and the difference is whether the caller should try again.
        Ok(Request::Kill(kill)) => match linklet_adapters::kill_processes(
            &kill.to_kill,
            kill.force,
            kill.exclude.as_deref(),
            &kill.candidates,
        ) {
            Ok(report) => Response::Sealed(wire::encode_kill_reply(&report)),
            Err(refusal) => Response::Sealed(wire::reply_refused(&refusal.to_string())),
        },
        // A request the agent could not read is a refusal and not a dropped
        // connection: the caller learns which field was wrong instead of waiting for
        // a reply that is not coming.
        Err(error) => Response::Sealed(wire::reply_refused(&refusal_text(&error))),
    }
}

/// The reply to a command that ran, refusing by name when it will not fit.
///
/// **This is the answer where there used to be silence, and `docs/ROADMAP.md` M10 is
/// the whole of it.** A command whose output was past the frame ceiling ran to
/// completion on the target, produced a reply that could not be framed, and got
/// nothing back: the caller was told "the agent closed the connection without
/// answering", which is a sentence about the network and not about the command.
///
/// The decision is made here rather than at the write for a reason worth stating:
/// by the time `write_frame` refuses, the bytes are sealed and gone, and the only
/// thing left to say is "too large" without any of the sizes. Asking first is what
/// lets the refusal name what did not fit.
///
/// Both branches are sealed and framed the same way by the caller, so there is no
/// second send path here -- an agent with two ways to send a reply has a way for
/// them to disagree.
fn run_reply(outcome: &wire::RunOutcome) -> Json {
    let ceiling = wire::reply_ceiling();
    let reply = wire::encode_run_reply(outcome);

    if wire::reply_fits(&reply, ceiling) {
        return reply;
    }

    wire::run_reply_too_large(outcome, ceiling)
}

/// Receives one pushed file.
///
/// The manifest is checked **in full, before a chunk is read**: the size against the
/// ceiling (T3, and half of T11), the shape of the digest, and the path against the
/// configured root (T1, the most severe item in the document). Then the acceptance goes
/// out -- **before the body** -- and only then does the receiving start.
///
/// That order is T14, and it was found on a real machine rather than reasoned about: the
/// sender has the file ready and streams it as soon as the manifest is written, so a
/// refusal that is written while the sender is still sending is a refusal the sender never
/// reads. See [`wire::manifest_accepted`].
///
/// Every failure is a **refusal** rather than a closed connection: the caller learns which
/// check failed, and by the time this returns a failure the `.part` has been deleted and
/// the real path was never touched.
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

    // Answer the manifest before reading a byte of the file. A sender that has not been
    // answered here has sent nothing, which is what stops the refusal below from racing
    // the body -- and what gives the sender a reason to wait.
    if !write_reply(connection, session, &wire::manifest_accepted(manifest)) {
        return wire::reply_refused("the acceptance could not be sent");
    }

    match receive_body(connection, session, manifest, &target) {
        Ok(outcome) => wire::encode_transfer_reply(&outcome),
        Err(failure) => wire::reply_refused(&failure.to_string()),
    }
}

/// Sends one file the caller asked for, and answers as it goes.
///
/// This is the one request that **answers itself**: a pull is a stream, so the manifest
/// goes out first as a reply and the chunks follow it, and there is nothing left for
/// `answer_one` to write when this returns.
///
/// The path is resolved against the root **before anything is opened**, and the root is
/// what makes this a read of a directory rather than a read of the machine: T1 is
/// written about writes, and the same `..\..\Windows\System32\...` that must not be
/// written must not be read either. The file is refused if it is not a regular file,
/// which is the read-side half of T2 -- a directory or a device would otherwise be a
/// strange failure from `File::open` rather than a named refusal.
///
/// A failure *before* the manifest is a refusal, like every other request's. A failure
/// **after** it cannot be: the caller has already been told a size and a digest, and
/// there is no second reply in this protocol. The receiver's own defences are what make
/// that safe -- it never renames a file that is short or whose digest differs, so a
/// transfer that dies halfway leaves nothing behind and the caller's next pull is
/// unremarkable.
fn send(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    path: &str,
    root: &Destination,
) -> Response {
    let target = match root.resolve(path) {
        Ok(target) => target,
        Err(error) => return Response::Sealed(wire::reply_refused(&error.to_string())),
    };

    // Describing the file hashes it, which is the pass that produces the digest the
    // caller will check its copy against. A file that cannot be read, or is not a
    // regular file, is refused by name here rather than after the manifest has promised
    // something.
    let manifest = match describe(&target, path) {
        Ok(manifest) => manifest,
        Err(failure) => return Response::Sealed(wire::reply_refused(&failure.to_string())),
    };

    let mut sealed = Vec::new();
    let reply = json::write(&wire::manifest_reply(&manifest));
    if session.seal_into(reply.as_bytes(), &mut sealed).is_err()
        || connection.write_frame(Kind::Sealed, &sealed).is_err()
    {
        return Response::AlreadySent;
    }

    // The body, and a failure here is silence: the connection closes, the receiver
    // throws its temporary away, and the caller learns that no reply came rather than a
    // reason that would arrive after a promise.
    let _ = send_body(connection, session, &target, &manifest);
    Response::AlreadySent
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
