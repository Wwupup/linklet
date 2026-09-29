//! What the client does when an agent refuses a manifest before the body.
//!
//! This file exists for one bug, found on a real machine and not on loopback.
//!
//! A push sends the manifest and then the file. The receiving agent decides at the
//! manifest -- the path is outside its root, the size is over its ceiling -- and there is
//! no time for the sender to hear about it if the sender has already started streaming:
//! the receiver writes its refusal and closes **with the sender's unread chunks still in
//! its receive queue**, Windows resets a socket closed in that state, and the reset
//! destroys the refusal the sender had not read yet. What the caller reported was "the
//! agent closed the connection without answering": true, and no help at all to whoever
//! pushed a build to the wrong place.
//!
//! On loopback the sender usually wins that race, which is why every test in
//! `against_agent.rs` passed while the real machine failed on every refusal. So this test
//! does not race: the fake agent here refuses the manifest and then reads nothing, and
//! the body it is offered is far larger than any socket buffer -- so a client that
//! streams before reading cannot complete, and one that reads first never sends a byte.
//!
//! The fake agent is written with the same connection and channel the real one uses, so
//! it cannot drift from the protocol: it is the agent's side of the same code.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use linklet_adapters::{Connection, HkdfChannel};
use linklet_client::{AgentAddress, CallError, push};
use linklet_core::auth::Token;
use linklet_core::channel::{EphemeralPublic, Handshake};
use linklet_core::frame::Kind;
use linklet_core::json;
use linklet_core::wire::{self, Request};

/// The token both ends use.
const TEST_TOKEN: &str = "test-token-0123456789";

/// A budget for the fake agent's own reads: long enough that a slow machine never
/// misfires.
const BUDGET: Duration = Duration::from_secs(30);

/// How long the fake agent waits, after refusing, to see whether a body arrives anyway.
///
/// Long enough that a client which streams before reading has certainly had its bytes
/// accepted by the socket, and it is not a duration the test measures: a client that
/// waits for the answer sends nothing, so this expires on an empty socket.
const PATIENCE: Duration = Duration::from_millis(1000);

/// A directory under the system temporary directory that removes itself.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "linklet-manifest-refusal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Sends a manifest, refuses it, and looks at whether a body arrives anyway.
///
/// The refusal is the whole of the answer, and the connection is then closed exactly as
/// the real agent closes it: without draining. If the client sent the body anyway, those
/// bytes are what makes the close a reset.
fn refuse_the_manifest(listener: TcpListener, reason: &str) -> bool {
    let _ = listener.set_nonblocking(false);
    let (stream, _) = listener.accept().expect("the client should connect");

    // The handshake, in the order the protocol puts it: hello, hello reply.
    let mut connection = Connection::with_budget(stream, BUDGET);
    let frame = connection
        .read_frame(Kind::Hello)
        .expect("the client's hello");
    let hello = wire::parse_body(&frame).expect("a hello body");
    let peer = wire::handshake_public_from_json(&hello).expect("an ephemeral key");
    let peer = EphemeralPublic::from_bytes(peer).expect("32 bytes");

    let (ours, mut session) = HkdfChannel
        .accept(TEST_TOKEN.as_bytes(), &peer)
        .expect("accepting the handshake");
    let reply = json::write(&wire::reply_result(wire::handshake_to_json(
        ours.as_bytes(),
    )));
    connection
        .write_frame(Kind::Hello, reply.as_bytes())
        .expect("the hello reply");

    // The manifest, which is the only thing this fake agent reads on purpose.
    let frame = connection.read_frame(Kind::Sealed).expect("the manifest");
    let mut plaintext = Vec::new();
    session
        .open_into(&frame, &mut plaintext)
        .expect("opening the manifest");
    let request = wire::parse_body(&plaintext).expect("a manifest body");
    match wire::request_from_json(&request) {
        Ok(Request::Push(_)) => {}
        other => panic!("expected a push request and got {other:?}"),
    }

    // The refusal, and then a deliberate look at whether a body arrives anyway. **Not
    // reading it is the point**: this fake agent does not drain, so whatever the client
    // sent is still in the queue when the connection closes -- and that is what makes the
    // close a reset, which is what destroys the refusal on the client's side.
    let refusal = json::write(&wire::reply_refused(reason));
    let mut sealed = Vec::new();
    session
        .seal_into(refusal.as_bytes(), &mut sealed)
        .expect("sealing the refusal");
    connection
        .write_frame(Kind::Sealed, &sealed)
        .expect("writing the refusal");

    // A short budget, because the answer here is "no frame arrived" and waiting thirty
    // seconds for it would make the suite slow for no more certainty. **A frame is the
    // only bad outcome**: a client that read the refusal and then closed cleanly ends this
    // read with Ended, and that is the behaviour being asked for rather than a failure.
    connection.set_budget(PATIENCE);
    let body_arrived_early = connection.read_frame(Kind::Sealed).is_ok();

    // Held open for a moment so the client has every chance to read the refusal, and then
    // dropped -- which is what turns a streamed body into a reset.
    std::thread::sleep(Duration::from_millis(250));
    body_arrived_early
}

/// Runs a push against a fake agent that refuses the manifest with `reason`.
///
/// Returns what the caller was told and whether the body arrived **before** the client
/// could have read the refusal.
fn push_against_a_refusal(reason: &str) -> (Result<wire::TransferOutcome, CallError>, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("the bound address").port();

    let refusal = reason.to_string();
    let agent = std::thread::spawn(move || refuse_the_manifest(listener, &refusal));

    let scratch = Scratch::new();
    let source = scratch.file("source.bin");
    let content: Vec<u8> = (0..8 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    std::fs::write(&source, &content).expect("writing a large local file");

    let address = AgentAddress::new(format!("127.0.0.1:{port}"))
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));

    let result = push(&address, &source, "build.exe");

    let body_arrived_early = agent.join().expect("the fake agent should finish");
    (result, body_arrived_early)
}

#[test]
fn a_manifest_refused_before_the_body_reports_the_reason_and_not_a_broken_connection() {
    // The bug, as one assertion. A refusal is an answer, and the caller has to be able to
    // read it: "the agent refused the request: ... outside the root" sends whoever pushed
    // the file to the path they typed, and "the agent closed the connection without
    // answering" sends them to the network.
    let reason = "the path is outside the root this agent may write in";
    let (result, _) = push_against_a_refusal(reason);
    let error = result.expect_err("the agent refused this");

    match error {
        CallError::Refused(text) => assert!(
            text.contains("outside the root"),
            "the refusal should carry the agent's own words: {text}"
        ),
        other => panic!(
            "a refusal has to arrive as a refusal; got {other:?}. A transport error here \
             means the sender streamed the body and the reset destroyed the answer."
        ),
    }
}

#[test]
fn the_body_does_not_arrive_before_the_manifest_has_been_answered() {
    // The property behind the test above, checked on its own because **the first version
    // of that test passed with the bug present**: on loopback the sender wins the race
    // often enough to see a reasonable message, and only a real link failed every time.
    //
    // Here the fake agent refuses and then looks at the socket with a one-second budget.
    // A client that waits for the answer has sent nothing, so that read times out; a
    // client that streams first has already put eight megabytes in the queue, so it does
    // not. Deterministic in both directions, which is what the outcome test could not be.
    let (_, body_arrived_early) = push_against_a_refusal("refused before the body");

    assert!(
        !body_arrived_early,
        "the sender put the file on the wire before reading the agent's answer to the \
         manifest. That is the bug: the agent has already refused, closes with those \
         bytes unread, and the reset destroys the refusal before the sender reads it."
    );
}
