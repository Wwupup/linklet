//! Moving a real file over a real socket, and every way it must not leave one.
//!
//! The arithmetic is in `linklet-core` and costs microseconds to check. What is left
//! for this layer is what only a filesystem can answer: whether a `.part` survives a
//! failure, whether a symlink is refused before anything is opened, whether the rename
//! is the only moment the real path changes, and whether a digest taken before a file
//! is sent still matches the file that arrives.
//!
//! Every test works in its own directory under the system temporary directory, named
//! from the process id and a counter so that two tests -- or two runs -- cannot collide.
//! The directory is removed at the end of the test that made it, including when the
//! test fails, because a suite that leaves megabytes behind is a suite people stop
//! running.

use std::fs;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use linklet_adapters::{
    Connection, HkdfChannel, TransferFailure, describe, digest_of_file, part_path, receive_body,
    send_body,
};
use linklet_core::channel::{Sealed, SessionId};
use linklet_core::transfer::Manifest;
use linklet_core::wire::TransferOutcome;

/// A budget for the tests' own reads: generous, because these run on the machine the
/// developer is sitting at and the point is not to measure time.
const BUDGET: Duration = Duration::from_secs(30);

/// A counter so two directories in one process cannot collide.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A directory that cleans itself up.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "linklet-transfer-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
    }

    /// The full path of a file in this directory.
    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Writes a file of `bytes` bytes whose content is a repeating pattern.
    fn write(&self, name: &str, bytes: usize) -> PathBuf {
        let path = self.file(name);
        let content: Vec<u8> = (0..bytes).map(|index| (index % 251) as u8).collect();
        fs::write(&path, content).expect("writing a test file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A connected pair of sockets, both ends usable.
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("the bound address").port();
    let client = TcpStream::connect(("127.0.0.1", port)).expect("connecting to ourselves");
    let (server, _) = listener.accept().expect("accepting our own connection");
    (client, server)
}

/// One end of a transfer: the socket and the session that seals what crosses it.
type End = (Connection, Box<dyn Sealed>);

/// A pair of sessions, one at each end of a socket pair.
///
/// The shared-secret path rather than a handshake, because a handshake is tested where
/// it belongs and the point here is the file.
fn sessions() -> (End, End) {
    let (client, server) = pair();
    let session = SessionId::from_bytes(vec![7u8; SessionId::BYTES]).expect("32 bytes");

    let host = HkdfChannel
        .open_session(
            b"a test secret of some length",
            &session,
            linklet_core::channel::Role::Host,
        )
        .expect("a session");
    let agent = HkdfChannel
        .open_session(
            b"a test secret of some length",
            &session,
            linklet_core::channel::Role::Agent,
        )
        .expect("a session");

    (
        (Connection::with_budget(client, BUDGET), host),
        (Connection::with_budget(server, BUDGET), agent),
    )
}

use linklet_core::channel::Channel;

/// Sends a manifest frame, the way a `push` request does.
///
/// Written here rather than through the client because this layer is testing the
/// transfer and not the protocol: the manifest is a message it is handed, and where the
/// message came from is `linklet_core::wire`'s business.
fn send_manifest(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    manifest: &Manifest,
) -> Result<(), TransferFailure> {
    let body = linklet_core::json::write(&linklet_core::wire::request_to_json(
        &linklet_core::wire::Request::Push(manifest.clone()),
    ));
    let mut sealed = Vec::new();
    session.seal_into(body.as_bytes(), &mut sealed)?;
    connection.write_frame(linklet_core::frame::Kind::Sealed, &sealed)?;
    Ok(())
}

/// Receives a transfer the way the agent does: the manifest off the wire first.
///
/// **Reading the manifest through the connection is part of what is being tested.**
/// `receive_body` sets the message budget from what the connection has already read, so
/// a test that handed it a manifest while leaving that manifest unread on the socket
/// would be in a state the agent is never in and would be counting the messages
/// wrongly. That mistake was made here once, and it looked like a budget bug in the
/// connection.
fn receive(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    destination: &Path,
) -> Result<TransferOutcome, TransferFailure> {
    let frame = connection
        .read_frame(linklet_core::frame::Kind::Sealed)
        .expect("the manifest frame");
    let mut plaintext = Vec::new();
    session
        .open_into(&frame, &mut plaintext)
        .expect("opening the manifest");

    let value = linklet_core::wire::parse_body(&plaintext).expect("the manifest is JSON");
    let manifest = match linklet_core::wire::request_from_json(&value).expect("a request") {
        linklet_core::wire::Request::Push(manifest) => manifest,
        other => panic!("expected a push request and got {other:?}"),
    };

    receive_body(connection, session, &manifest, destination)
}

/// Sends one file and receives it, and returns what the receiver reported.
fn round_trip(
    scratch: &Scratch,
    source_bytes: usize,
    destination_name: &str,
) -> (TransferOutcome, PathBuf, PathBuf) {
    let source = scratch.write("source.bin", source_bytes);
    let destination = scratch.file(destination_name);
    let manifest = describe(&source, destination_name).expect("a describable file");

    let ((mut host, mut host_session), (mut agent, mut agent_session)) = sessions();

    send_manifest(&mut host, host_session.as_mut(), &manifest).expect("writing the manifest");
    send_body(&mut host, host_session.as_mut(), &source, &manifest).expect("sending the body");

    let outcome =
        receive(&mut agent, agent_session.as_mut(), &destination).expect("receiving the transfer");

    (outcome, source, destination)
}

// --- the round trip ----------------------------------------------------------

#[test]
fn a_file_arrives_byte_for_byte_with_the_digest_the_receiver_computed() {
    let scratch = Scratch::new();
    let (outcome, source, destination) = round_trip(&scratch, 4096, "build.exe");

    assert_eq!(
        fs::read(&destination).expect("the received file"),
        fs::read(&source).expect("the sent file")
    );
    assert_eq!(outcome.bytes, 4096);
    assert_eq!(
        outcome.sha256,
        digest_of_file(&source).expect("hashing the source"),
        "the digest that comes back is the one the receiver computed, and it is the \
         sender's because the bytes are the same"
    );
}

#[test]
fn a_file_larger_than_one_chunk_arrives_intact() {
    // The chunk boundary is where an off-by-one turns into a corrupt file rather than a
    // refusal, so the interesting sizes are around it. One byte over is the smallest
    // case that takes two chunks, and it is the last chunk that is short.
    let scratch = Scratch::new();
    let chunk = linklet_core::transfer::CHUNK_BYTES as usize;

    for (label, size) in [
        ("exactly one chunk", chunk),
        ("one byte past a chunk", chunk + 1),
        ("two chunks and a bit", chunk * 2 + 17),
    ] {
        let (outcome, source, destination) =
            round_trip(&scratch, size, &format!("build-{size}.exe"));

        assert_eq!(outcome.bytes as usize, size, "{label}");
        assert_eq!(
            fs::read(&destination).expect("the received file").len(),
            size,
            "{label}"
        );
        assert_eq!(
            fs::read(&destination).expect("the received file"),
            fs::read(&source).expect("the sent file"),
            "{label}"
        );
    }
}

#[test]
fn the_temporary_file_does_not_survive_a_transfer_that_worked() {
    // The `.part` is the whole of what a transfer may leave behind, so a completed
    // transfer must leave none of it.
    let scratch = Scratch::new();
    let (_outcome, _source, destination) = round_trip(&scratch, 512, "build.exe");

    assert!(destination.is_file(), "the file should be there");
    assert!(
        !part_path(&destination).exists(),
        "the temporary should have been renamed away, and {} is still there",
        part_path(&destination).display()
    );
}

// --- T2: the destination is not a file --------------------------------------

#[test]
fn a_destination_that_is_a_directory_is_refused_before_anything_is_written() {
    let scratch = Scratch::new();
    let source = scratch.write("source.bin", 128);
    let destination = scratch.file("a-directory");
    fs::create_dir(&destination).expect("a directory to aim at");

    let manifest = describe(&source, "a-directory").expect("a describable file");
    let ((mut host, mut host_session), (mut agent, mut agent_session)) = sessions();
    send_manifest(&mut host, host_session.as_mut(), &manifest).expect("the manifest");
    send_body(&mut host, host_session.as_mut(), &source, &manifest).expect("the body");

    let failure = receive(&mut agent, agent_session.as_mut(), &destination)
        .expect_err("a directory is not a file");
    assert!(
        matches!(failure, TransferFailure::NotAFile { .. }),
        "got {failure:?}"
    );
    assert!(
        !part_path(&destination).exists(),
        "the refusal must come before the temporary is created"
    );
    assert!(
        destination.is_dir(),
        "and the directory is still a directory"
    );
}

// --- T6, T8, T9: a transfer that did not finish ------------------------------

#[test]
fn a_sender_that_stops_early_leaves_nothing_under_the_real_name() {
    // The failure this exists for: a half-transferred file at the real path is worse
    // than no file at all, because the next step believes it.
    let scratch = Scratch::new();
    let source = scratch.write("source.bin", 4096);
    let destination = scratch.file("build.exe");
    let manifest = describe(&source, "build.exe").expect("a describable file");

    let ((mut host, mut host_session), (mut agent, mut agent_session)) = sessions();
    send_manifest(&mut host, host_session.as_mut(), &manifest).expect("the manifest");
    // Half the file, then the connection goes away: the sender died.
    let mut sealed = Vec::new();
    host_session
        .seal_into(&vec![0u8; 2048], &mut sealed)
        .expect("sealing");
    host.write_frame(linklet_core::frame::Kind::Sealed, &sealed)
        .expect("writing half a file");
    drop(host);

    let failure = receive(&mut agent, agent_session.as_mut(), &destination)
        .expect_err("the rest never comes");
    assert!(
        matches!(failure, TransferFailure::Connection(_)),
        "got {failure:?}"
    );
    assert!(
        !destination.exists(),
        "a short file at the real path is what this test is about"
    );
    assert!(
        !part_path(&destination).exists(),
        "and the temporary must not survive either"
    );
}

#[test]
fn a_sender_that_lies_about_the_size_is_refused_and_leaves_nothing() {
    // T4, over a real socket: the manifest declares 100 bytes and the sender keeps
    // sending. The connection's message budget is what refuses it -- and the real path
    // is never touched.
    let scratch = Scratch::new();
    let source = scratch.write("source.bin", 4096);
    let destination = scratch.file("build.exe");

    // A manifest that understates the file, which is what a lying sender looks like.
    let honest = describe(&source, "build.exe").expect("a describable file");
    let lying = Manifest {
        path: honest.path.clone(),
        bytes: 100,
        sha256: honest.sha256.clone(),
    };

    let ((mut host, mut host_session), (mut agent, mut agent_session)) = sessions();
    send_manifest(&mut host, host_session.as_mut(), &lying).expect("the manifest");
    // One chunk of 4096 bytes against a declared 100.
    let mut sealed = Vec::new();
    host_session
        .seal_into(&fs::read(&source).expect("the source"), &mut sealed)
        .expect("sealing");
    host.write_frame(linklet_core::frame::Kind::Sealed, &sealed)
        .expect("writing a chunk that is too big");

    let failure = receive(&mut agent, agent_session.as_mut(), &destination)
        .expect_err("4096 bytes were declared as 100");
    assert!(
        matches!(failure, TransferFailure::Refused(_)),
        "got {failure:?}"
    );
    assert!(
        !destination.exists(),
        "nothing may land under the real name"
    );
    assert!(!part_path(&destination).exists(), "nor under the temporary");
}

// --- T7: what arrived is not what was sent -----------------------------------

#[test]
fn a_file_that_is_not_what_the_digest_says_is_refused_and_deleted() {
    // The digest is of what is **on the disk**, read back, so a mismatch is caught
    // even though every byte crossed an AEAD that authenticated it. This is the layer
    // above: a framing bug, a short write, or a `.part` something else overwrote.
    let scratch = Scratch::new();
    let source = scratch.write("source.bin", 1024);
    let destination = scratch.file("build.exe");

    let honest = describe(&source, "build.exe").expect("a describable file");
    let mut wrong = honest.clone();
    wrong.sha256 = "0".repeat(64);

    let ((mut host, mut host_session), (mut agent, mut agent_session)) = sessions();
    send_manifest(&mut host, host_session.as_mut(), &wrong).expect("the manifest");
    send_body(&mut host, host_session.as_mut(), &source, &wrong).expect("the body");

    let failure = receive(&mut agent, agent_session.as_mut(), &destination)
        .expect_err("the digest does not match");
    assert!(
        matches!(
            failure,
            TransferFailure::Refused(linklet_core::transfer::TransferError::Digest { .. })
        ),
        "got {failure:?}"
    );
    assert!(
        !destination.exists(),
        "a file whose digest did not match must not be renamed into place"
    );
    assert!(!part_path(&destination).exists());
}

// --- what a sender refuses before starting -----------------------------------

#[test]
fn an_empty_file_cannot_be_described() {
    // The protocol refuses a zero-byte transfer -- nothing it sends is empty -- so the
    // refusal happens here, before a socket is opened, rather than on the other machine.
    let scratch = Scratch::new();
    let empty = scratch.write("empty.bin", 0);

    let failure = describe(&empty, "empty.bin").expect_err("a transfer of no bytes");
    assert!(
        matches!(failure, TransferFailure::Unsendable(_)),
        "got {failure:?}"
    );
    assert!(failure.to_string().contains("no bytes"), "{failure}");
}

#[test]
fn a_file_that_changed_after_it_was_hashed_is_reported_rather_than_timed_out() {
    // A build that is still being written is the realistic version of this, and the
    // alternative is a receiver that waits out its deadline and reports "no message
    // arrived", which names neither the file nor the byte count.
    let scratch = Scratch::new();
    let source = scratch.write("source.bin", 4096);
    let destination = scratch.file("build.exe");
    let manifest = describe(&source, "build.exe").expect("a describable file");

    // Shorter than the digest said: the file was replaced between the two reads.
    fs::write(&source, vec![0u8; 100]).expect("truncating the source");

    let ((mut host, mut host_session), (_agent, _agent_session)) = sessions();
    send_manifest(&mut host, host_session.as_mut(), &manifest).expect("the manifest");

    let failure = send_body(&mut host, host_session.as_mut(), &source, &manifest)
        .expect_err("the file is shorter than it was");
    assert!(
        matches!(
            failure,
            TransferFailure::Refused(linklet_core::transfer::TransferError::Short { .. })
        ),
        "got {failure:?}"
    );
    assert!(failure.to_string().contains("4096"), "{failure}");
    assert!(!destination.exists());
}

#[test]
fn a_source_that_is_not_a_file_is_refused_by_name() {
    let scratch = Scratch::new();
    let directory = scratch.file("a-directory");
    fs::create_dir(&directory).expect("a directory");

    let failure = describe(&directory, "x").expect_err("a directory is not a file");
    assert!(
        matches!(failure, TransferFailure::NotAFile { .. }),
        "got {failure:?}"
    );
}

#[test]
fn a_source_that_does_not_exist_names_the_path() {
    let scratch = Scratch::new();
    let missing = scratch.file("not-here.bin");

    let failure = describe(&missing, "x").expect_err("nothing is there");
    assert!(
        matches!(failure, TransferFailure::Filesystem { .. }),
        "got {failure:?}"
    );
    assert!(
        failure.to_string().contains("not-here.bin"),
        "the refusal should name the file: {failure}"
    );
}

// --- the digest itself -------------------------------------------------------

#[test]
fn the_digest_of_a_file_is_the_one_a_published_vector_gives() {
    // A published vector, because the alternative is a test that agrees with whatever
    // this code happens to do. `abc` is the classic one, and it is the value anyone can
    // check in another tool.
    let scratch = Scratch::new();
    let path = scratch.file("abc.txt");
    fs::write(&path, b"abc").expect("writing abc");

    assert_eq!(
        digest_of_file(&path).expect("a digest"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn the_digest_of_an_empty_file_is_the_empty_vector() {
    // The other published value worth having: it is the one a hand-written hash got
    // wrong three times, and the reason `sha2` is a dependency.
    let scratch = Scratch::new();
    let path = scratch.file("empty.txt");
    fs::write(&path, b"").expect("writing nothing");

    assert_eq!(
        digest_of_file(&path).expect("a digest"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn the_part_path_is_the_real_path_with_a_suffix() {
    // What a failure leaves behind, named in one place so that the thing that writes it
    // and the thing that deletes it cannot disagree.
    assert_eq!(
        part_path(Path::new(r"C:\linklet\build.exe")),
        PathBuf::from(r"C:\linklet\build.exe.part")
    );
}
