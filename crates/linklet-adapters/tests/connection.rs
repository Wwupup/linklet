//! The connection: the four defences that live outside the frame module.
//!
//! `crates/linklet-core/src/frame.rs` defends six of the ten ways a length-prefixed
//! protocol goes wrong, and says so. The other four belong to the connection rather
//! than to a frame, because they need a clock, a socket, or a count that outlives one
//! message -- and `docs/framing.md` is where they are written down. This file is
//! where they are checked:
//!
//! 1. **the read timeout** -- a peer that declares a length and then sends nothing
//! 2. **no pipelining** -- the reader never holds bytes it did not ask for
//! 3. **the magic byte**, which is inline in the frame module and is re-checked here
//!    because this is the layer that reads from a stranger's socket
//! 4. **every write result is checked**
//!
//! and one that is not on that list because it was found while writing this file:
//! **a frame that timed out halfway is not a frame**, so the connection refuses to be
//! read again rather than resynchronising on the bytes that did arrive.
//!
//! Everything here is a real socket on the loopback interface. The frame module's own
//! tests are pure and run in microseconds; these cost a millisecond each and are the
//! only place a timeout can be observed at all.

use std::io::Write;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use linklet_adapters::{Connection, ConnectionError};
use linklet_core::frame::{FrameError, Kind, MAGIC, MAX_PAYLOAD, header};

/// A budget short enough to keep the suite quick, long enough not to misfire.
const BRIEF: Duration = Duration::from_millis(400);

/// A connected pair of sockets, both ends usable.
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("the bound address").port();

    let client = TcpStream::connect(("127.0.0.1", port)).expect("connecting to ourselves");
    let (server, _) = listener.accept().expect("accepting our own connection");
    (client, server)
}

/// Two connections, one on each end of the same socket pair.
fn connections() -> (Connection, Connection) {
    let (client, server) = pair();
    (
        Connection::with_budget(client, BRIEF),
        Connection::with_budget(server, BRIEF),
    )
}

// --- the format, over a socket -------------------------------------------------

#[test]
fn a_frame_crosses_a_real_socket_unchanged() {
    let (mut sender, mut receiver) = connections();

    // A payload that contains the magic byte and a plausible header of its own. A
    // reader that scanned for a boundary instead of trusting the length would end
    // the message in the wrong place here.
    let mut payload = header(Kind::Sealed, 8).expect("a valid header").to_vec();
    payload.extend((0..=255u8).cycle().take(4096));

    sender
        .write_frame(Kind::Sealed, &payload)
        .expect("writing a frame");
    let got = receiver
        .read_frame(Kind::Sealed)
        .expect("reading the frame back");

    assert_eq!(got, payload);
}

#[test]
fn two_frames_in_a_row_are_read_as_two_frames() {
    // The desynchronisation case, from the outside: a reader that held surplus bytes
    // between messages would return a second payload glued to the end of the first.
    let (mut sender, mut receiver) = connections();

    sender.write_frame(Kind::Sealed, b"first").expect("one");
    sender.write_frame(Kind::Sealed, b"second").expect("two");

    assert_eq!(receiver.read_frame(Kind::Sealed).expect("one"), b"first");
    assert_eq!(receiver.read_frame(Kind::Sealed).expect("two"), b"second");
}

// --- defence 1: the read timeout ----------------------------------------------

#[test]
fn a_peer_that_says_nothing_times_out_sooner_than_a_person_gives_up() {
    // Item 1: a peer that connects and sends nothing holds a thread and a
    // connection. Nothing in the frame module can stop that, because it has no
    // clock; this is the layer that does.
    let (client, _server) = pair();
    let mut connection = Connection::with_budget(client, BRIEF);

    let started = Instant::now();
    let error = connection
        .read_frame(Kind::Hello)
        .expect_err("nothing was sent");
    let waited = started.elapsed();

    assert!(
        matches!(error, ConnectionError::Timeout { .. }),
        "expected a timeout, got {error:?}"
    );
    assert!(
        waited < Duration::from_secs(5),
        "it waited {waited:?}, which is not the budget it was given"
    );
    // A timeout says what it knows. It does not claim to know whether the peer is
    // slow, dead, or refusing -- which is the failure this project has already
    // produced once in another shape.
    let text = error.to_string();
    assert!(text.contains("400"), "the budget should be named: {text}");
}

#[test]
fn a_declared_length_that_never_arrives_times_out_rather_than_waiting_forever() {
    // The header arrives, the payload does not: the reader has committed to a
    // length and must still give up in bounded time.
    let (client, mut server) = pair();
    server
        .write_all(&header(Kind::Sealed, 4096).expect("a valid header"))
        .expect("writing a header");
    server.flush().expect("flushing");

    let mut connection = Connection::with_budget(client, BRIEF);
    let error = connection
        .read_frame(Kind::Sealed)
        .expect_err("the payload never comes");

    assert!(
        matches!(error, ConnectionError::Timeout { .. }),
        "expected a timeout, got {error:?}"
    );
}

#[test]
fn a_connection_that_timed_out_mid_frame_is_never_read_again() {
    // Not on the documented list, and found while writing this file. A `read_exact`
    // that times out partway has consumed bytes that are now lost, so the next read
    // would start in the middle of the previous message -- which is exactly the
    // desynchronisation the whole design exists to prevent. Refusing is the only
    // honest answer, and it is louder than the alternative.
    let (client, mut server) = pair();
    server
        .write_frame_raw(&header(Kind::Sealed, 64).expect("a header"), b"half")
        .expect("writing half a frame");

    let mut connection = Connection::with_budget(client, BRIEF);
    assert!(
        matches!(
            connection.read_frame(Kind::Sealed),
            Err(ConnectionError::Timeout { .. })
        ),
        "the first read should time out"
    );

    // The rest arrives after the budget. A reader that resynchronised would treat
    // these bytes as the start of a message.
    server
        .write_frame_raw(&[], b"the-rest-of-the-first-frame")
        .expect("writing the rest");
    let error = connection
        .read_frame(Kind::Sealed)
        .expect_err("a poisoned connection refuses");
    assert!(
        matches!(error, ConnectionError::Unusable { .. }),
        "expected a refusal, got {error:?}"
    );
}

#[test]
fn a_peer_that_hangs_up_is_told_apart_from_a_peer_that_is_silent() {
    // Both are "no frame arrived", and they are different facts: one means the
    // agent died, the other means it is thinking. Reporting a hang-up as a timeout
    // would send a reader looking at the network.
    let (client, server) = pair();
    server
        .shutdown(Shutdown::Both)
        .expect("closing our end of the pair");

    let mut connection = Connection::with_budget(client, BRIEF);
    let error = connection
        .read_frame(Kind::Hello)
        .expect_err("the peer is gone");

    assert!(
        matches!(error, ConnectionError::Ended),
        "expected a hang-up, got {error:?}"
    );
}

#[test]
fn an_abrupt_close_reads_the_same_as_a_clean_one() {
    // Windows sends a reset rather than a FIN when a socket is closed with unread
    // bytes still in its queue, and the reset arrives carrying a **localised**
    // operating-system message. A protocol reader cannot tell the two apart in any way
    // that changes what it does, so both read as "the peer is gone" -- which also means
    // a refusal in another language never reaches a user.
    //
    // The test holds either way: if this machine sends a FIN instead, the assertion is
    // the same one, and that is the point being pinned.
    let (mut client, server) = pair();
    client
        .write_all(b"bytes the peer never reads")
        .expect("writing before the peer closes");
    client.flush().expect("flushing");
    drop(server);
    std::thread::sleep(Duration::from_millis(50));

    let mut connection = Connection::with_budget(client, BRIEF);
    let error = connection
        .read_frame(Kind::Hello)
        .expect_err("the peer is gone");

    assert!(
        matches!(error, ConnectionError::Ended),
        "expected a hang-up, got {error:?}"
    );
    assert!(
        error.to_string().is_ascii(),
        "the refusal must not be the operating system's own words: {error}"
    );
}

// --- defence 3: the magic byte, at the layer that reads a stranger -------------
#[test]
fn a_first_byte_that_is_not_the_magic_is_refused_by_name() {
    // Something else is listening on this port -- an HTTP server, most likely, which
    // is the mistake an operator makes first. The refusal names the byte, so the
    // diagnosis is one line rather than an allocation attempt.
    let (client, mut server) = pair();
    server
        .write_all(b"GET / HTTP/1.1\r\n\r\n")
        .expect("writing another protocol");
    server.flush().expect("flushing");

    let mut connection = Connection::with_budget(client, BRIEF);
    let error = connection
        .read_frame(Kind::Hello)
        .expect_err("that is not this protocol");

    assert_eq!(
        error,
        ConnectionError::Frame(FrameError::NotThisProtocol { found: b'G' })
    );
    let text = error.to_string();
    assert!(
        text.contains("0x47") && text.contains("0x4c"),
        "the message should name both bytes: {text}"
    );
}

#[test]
fn a_hostile_length_is_refused_before_a_single_byte_is_reserved() {
    // Item 3, over a socket: the attacker's number is four gigabytes. This is also
    // the reason `decode_header` returns the length rather than acting on it.
    let (client, mut server) = pair();
    server
        .write_all(&[MAGIC, Kind::Sealed.as_byte(), 0xFF, 0xFF, 0xFF, 0xFF])
        .expect("writing a hostile header");
    server.flush().expect("flushing");

    let mut connection = Connection::with_budget(client, BRIEF);
    assert_eq!(
        connection.read_frame(Kind::Sealed),
        Err(ConnectionError::Frame(FrameError::TooLarge {
            declared: 0xFFFF_FFFF
        }))
    );
}

#[test]
fn the_wrong_kind_is_refused_by_name_and_not_as_a_bad_message() {
    // Item 8: the connection has an order, and a frame arriving out of it means the
    // state machine and the bytes disagree. Saying which kind was expected turns
    // that into a one-line diagnosis.
    let (mut sender, mut receiver) = connections();
    sender
        .write_frame(Kind::Sealed, b"sealed")
        .expect("writing a sealed frame");

    let error = receiver
        .read_frame(Kind::Hello)
        .expect_err("a sealed frame where a hello was expected");
    assert_eq!(
        error,
        ConnectionError::WrongKind {
            expected: Kind::Hello,
            found: Kind::Sealed
        }
    );
    let text = error.to_string();
    assert!(
        text.contains("hello") && text.contains("sealed"),
        "the message should name both kinds: {text}"
    );
}

#[test]
fn the_payload_of_a_refused_frame_is_consumed_so_the_connection_can_close_cleanly() {
    // Not on the documented list, and found by a test that failed once in a handful of
    // runs. Windows resets a socket that is closed with unread bytes in its receive
    // queue, and a reset discards the answer the refusing side just wrote -- so a peer
    // that sent the wrong kind and was told so would sometimes see silence instead,
    // which looks exactly like an agent ignoring the request.
    //
    // The refused payload is therefore read and thrown away, and this shows it: the
    // next read begins at the next frame rather than in the middle of the one that was
    // refused.
    let (mut sender, mut receiver) = connections();
    sender
        .write_frame(Kind::Sealed, b"the payload of the refused frame")
        .expect("the refused frame");
    sender
        .write_frame(Kind::Sealed, b"the one after it")
        .expect("the next frame");

    assert!(matches!(
        receiver.read_frame(Kind::Hello),
        Err(ConnectionError::WrongKind { .. })
    ));
    assert_eq!(
        receiver
            .read_frame(Kind::Sealed)
            .expect("the frame after the refused one"),
        b"the one after it",
        "a reader that had left the refused payload on the socket would find its \
         bytes where this frame's header should be"
    );
}

// --- T11: the message count is bounded by the declared size -------------------

#[test]
fn a_message_past_the_budget_is_refused_rather_than_read() {
    // The replacement for framing item 9, which a transfer does not get to keep: a
    // transfer is a manifest plus N chunks, so the count is no longer a protocol
    // constant. It is bounded instead by the declared size, and this is where that
    // bound is enforced -- which is also what makes a chunk after completion
    // (`docs/transfer.md` T5) a protocol error rather than an extra write.
    let (mut sender, mut receiver) = connections();
    receiver.set_message_limit(2);

    sender.write_frame(Kind::Sealed, b"one").expect("one");
    sender.write_frame(Kind::Sealed, b"two").expect("two");
    sender.write_frame(Kind::Sealed, b"three").expect("three");

    assert_eq!(receiver.read_frame(Kind::Sealed).expect("one"), b"one");
    assert_eq!(receiver.read_frame(Kind::Sealed).expect("two"), b"two");

    let error = receiver
        .read_frame(Kind::Sealed)
        .expect_err("the third is past the budget");
    assert_eq!(error, ConnectionError::Budget { limit: 2 });
    assert!(
        error.to_string().contains('2'),
        "the budget should be named: {error}"
    );
}

#[test]
fn the_budget_can_be_raised_once_the_declared_size_is_known() {
    // The manifest arrives inside the first sealed message, so the count for a
    // transfer cannot be known before it is read. Raising the limit after reading it
    // is the mechanism; leaving it at two is the alternative, and it would refuse
    // every transfer larger than a single chunk.
    let (mut sender, mut receiver) = connections();
    receiver.set_message_limit(1);

    sender.write_frame(Kind::Sealed, b"manifest").expect("one");
    assert_eq!(
        receiver.read_frame(Kind::Sealed).expect("the manifest"),
        b"manifest"
    );

    receiver.set_message_limit(4);
    for expected in [b"chunk".as_slice(), b"chunk", b"chunk"] {
        sender.write_frame(Kind::Sealed, expected).expect("a chunk");
        assert_eq!(
            receiver.read_frame(Kind::Sealed).expect("a chunk"),
            expected
        );
    }

    assert_eq!(receiver.messages_read(), 4);
}

// --- defence 4: every write result is checked --------------------------------

#[test]
fn a_sender_cannot_build_a_frame_the_receiver_would_refuse() {
    // The two ends run the same check, so an over-long frame is refused on the side
    // that would have sent it rather than discovered by the side that cannot read it.
    let (client, _server) = pair();
    let mut connection = Connection::with_budget(client, BRIEF);

    assert_eq!(
        connection.write_frame(Kind::Sealed, &vec![0u8; MAX_PAYLOAD + 1]),
        Err(ConnectionError::Frame(FrameError::TooLarge {
            declared: MAX_PAYLOAD + 1
        }))
    );
}

#[test]
fn writing_to_a_connection_whose_peer_is_gone_is_an_error_and_not_a_silent_success() {
    // Item 10. The failure this catches is a truncated write that nobody noticed,
    // which on the other side is a receiver waiting for the rest of a message the
    // sender believes it sent. The frame is deliberately larger than any socket
    // buffer, so at least one `write_all` has to meet the closed connection.
    let (client, server) = pair();
    server
        .shutdown(Shutdown::Both)
        .expect("closing our end of the pair");
    // Give the close a moment to reach the client, so the write meets a reset
    // socket rather than a buffer that still has room in it.
    std::thread::sleep(Duration::from_millis(50));

    let mut connection = Connection::with_budget(client, BRIEF);
    let payload = vec![0u8; 8 * 1024 * 1024];
    let result = connection.write_frame(Kind::Sealed, &payload);

    assert!(
        result.is_err(),
        "a write to a closed peer reported success, which is the bug item 10 is about"
    );
    assert!(
        !matches!(result, Err(ConnectionError::Frame(_))),
        "the failure should be the socket and not the framing"
    );
}

// --- helpers on a raw socket --------------------------------------------------

/// A raw byte writer, used where a test has to send something the framing layer
/// would refuse to build -- a hostile header, or half a frame.
trait RawFrame {
    fn write_frame_raw(&mut self, head: &[u8], rest: &[u8]) -> std::io::Result<()>;
}

impl RawFrame for TcpStream {
    fn write_frame_raw(&mut self, head: &[u8], rest: &[u8]) -> std::io::Result<()> {
        self.write_all(head)?;
        self.write_all(rest)?;
        self.flush()
    }
}
