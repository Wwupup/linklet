//! Moving a file: the manifest, the chunks, and the temporary that must not survive.
//!
//! `docs/transfer.md` is the design and the list of thirteen ways it goes wrong. The
//! arithmetic is in `linklet_core::transfer`, where it costs microseconds to test; the
//! paths and sizes a manifest may name are checked before anything is opened, also in
//! the core. What is left for this file is the part that needs a disk: reading a file,
//! writing a `.part`, hashing what was written, and the rename.
//!
//! # The order, and the failure each step stops
//!
//! A receiver:
//!
//! 1. **T2** -- a destination that exists and is not a regular file is refused, before
//!    the temporary is created
//! 2. **T11, T5** -- the connection is told how many messages this transfer has, so a
//!    chunk after the declared size is refused by the connection
//! 3. **T4** -- each chunk is accounted for *before* it is written, because a caller
//!    that wrote first has already filled the disk it was trying not to fill
//! 4. **T6** -- the loop ends only when the declared size has arrived, so a short
//!    transfer never reaches the rename
//! 5. **T7** -- the `.part` is flushed and read back, and its digest compared with the
//!    one the sender declared
//! 6. **T9** -- and only then is it renamed over the real path
//! 7. **T8** -- anything that failed deletes the `.part`, and the real path was never
//!    touched
//!
//! A sender reads the file, seals each chunk in place, and frames it. It refuses a
//! transfer the receiver would refuse *before* the first chunk, and it notices a file
//! that turned out to be shorter than the digest it took said it was -- which is the
//! difference between "the sender lied" and "the build was still being written".

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use linklet_core::channel::{ChannelError, Sealed};
use linklet_core::frame::Kind;
use linklet_core::transfer::{
    Manifest, ManifestError, Receiving, Sending, TransferError, verify_digest,
};
use linklet_core::wire::{self, TransferOutcome};

use crate::connection::{Connection, ConnectionError};

/// Why a transfer could not be carried through.
///
/// Deliberately not one variant: a caller has to act differently on "the sender lied
/// about the size" and "the disk is full", and folding them into one message is how a
/// person ends up reading a log to find out which machine to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferFailure {
    /// The sender refused its own transfer before starting it.
    Unsendable(ManifestError),
    /// The arithmetic refused it: too much, past the end, short, or a digest that does
    /// not match.
    ///
    /// [`TransferError::Digest`] is the important one -- T7 -- and it is the reason a
    /// caller is told the digest rather than a flag.
    Refused(TransferError),
    /// The connection failed.
    Connection(ConnectionError),
    /// A message did not open.
    Channel(ChannelError),
    /// The filesystem refused, and this is the path it refused.
    Filesystem {
        /// The path involved.
        path: String,
        /// What the operating system said.
        problem: String,
    },
    /// The path exists and is not a regular file. T2.
    NotAFile {
        /// The path that is not a file.
        path: String,
    },
}

impl std::fmt::Display for TransferFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsendable(error) => write!(f, "this transfer cannot be sent: {error}"),
            Self::Refused(error) => write!(f, "{error}"),
            Self::Connection(error) => write!(f, "{error}"),
            Self::Channel(error) => write!(f, "a message did not open: {error}"),
            Self::Filesystem { path, problem } => write!(f, "{path}: {problem}"),
            Self::NotAFile { path } => write!(
                f,
                "{path} exists and is not a regular file, so writing to it would not \
                 write the file that was asked for"
            ),
        }
    }
}

impl std::error::Error for TransferFailure {}

impl From<TransferError> for TransferFailure {
    fn from(error: TransferError) -> Self {
        Self::Refused(error)
    }
}

impl From<ConnectionError> for TransferFailure {
    fn from(error: ConnectionError) -> Self {
        Self::Connection(error)
    }
}

impl From<ChannelError> for TransferFailure {
    fn from(error: ChannelError) -> Self {
        Self::Channel(error)
    }
}

/// Where a transfer writes while it is in progress.
///
/// `<path>.part`, as `docs/transfer.md` specifies. The real path is only ever changed
/// by a rename, and only after every check has passed -- so a `.part` is the whole of
/// what a failed transfer can leave behind, and it is deleted on the way out.
///
/// Two transfers to one destination at once share this name and each writes at its own
/// offset, so at most one of them can pass its digest check. The other fails having
/// written nothing under the real name, which is a failure rather than a corruption.
pub fn part_path(target: &Path) -> PathBuf {
    let mut text = target.as_os_str().to_os_string();
    text.push(".part");
    PathBuf::from(text)
}

/// The lowercase hex SHA-256 of a file, streamed rather than read whole.
///
/// Streamed because the file may be gigabytes and this may be running on the machine
/// being deployed to. The buffer is 64 KiB, which is small enough to be nothing and
/// large enough that the syscall overhead disappears.
///
/// # Errors
///
/// [`TransferFailure::Filesystem`] naming the path.
pub fn digest_of_file(path: &Path) -> Result<String, TransferFailure> {
    let mut file = File::open(path).map_err(|error| filesystem(path, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| filesystem(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(wire::to_hex(hasher.finalize().as_slice()))
}

/// Describes a local file, for the manifest of a transfer that is about to start.
///
/// The digest is taken **before anything is sent**, because the manifest has to carry
/// it: the receiver compares what it wrote against this, and a digest computed
/// afterwards would be comparing the transfer with itself.
///
/// The result is checked with [`Manifest::check_locally`], so a transfer the receiver
/// would refuse -- empty, past the ceiling, or a digest that is not one -- is refused
/// here, before a chunk crosses the network. Where the file may land is the receiver's
/// question and is not asked here: this side cannot resolve a path on another machine.
///
/// # Errors
///
/// [`TransferFailure::Unsendable`] for a transfer that could not be accepted, and
/// [`TransferFailure::Filesystem`] when the file cannot be read.
pub fn describe(local: &Path, remote_path: &str) -> Result<Manifest, TransferFailure> {
    let metadata = fs::metadata(local).map_err(|error| filesystem(local, error))?;
    if !metadata.is_file() {
        return Err(TransferFailure::NotAFile {
            path: local.display().to_string(),
        });
    }

    let manifest = Manifest {
        path: remote_path.to_string(),
        bytes: metadata.len(),
        sha256: digest_of_file(local)?,
    };
    manifest
        .check_locally()
        .map_err(TransferFailure::Unsendable)?;

    Ok(manifest)
}

/// Sends the body of a transfer: `manifest.bytes` bytes, one chunk at a time.
///
/// The manifest has already gone out, because it is a different message on each side
/// of the protocol -- the request on a push, the first reply on a pull. This is the
/// loop they share.
///
/// **One chunk in flight at a time.** Nothing is written until the previous chunk has
/// been framed and handed to the socket, and nothing is read in between, which is the
/// choice `docs/transfer.md` makes explicitly: it costs a round trip per mebibyte and
/// it is what keeps the desynchronisation defence holding. Each chunk is sealed into a
/// reused buffer rather than a fresh one, so a file is in memory once and not three
/// times -- T10.
///
/// # Errors
///
/// [`TransferFailure::Filesystem`] when the file cannot be read,
/// [`TransferFailure::Refused`] with [`TransferError::Short`] when it turns out to be
/// shorter than the manifest declared, and the connection's own failures.
pub fn send_body(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    local: &Path,
    manifest: &Manifest,
) -> Result<(), TransferFailure> {
    let mut file = File::open(local).map_err(|error| filesystem(local, error))?;
    let mut sending = Sending::new(manifest.bytes);
    let mut chunk: Vec<u8> = Vec::new();
    let mut sealed: Vec<u8> = Vec::new();

    while let Some(length) = sending.next_chunk() {
        chunk.clear();
        chunk.resize(length, 0);

        // A short read means the file changed between being hashed and being sent --
        // a build still being written, most likely. Reporting it here names the file
        // and the byte count; the receiver's alternative is a deadline, which names
        // neither.
        file.read_exact(&mut chunk).map_err(|error| {
            if error.kind() == ErrorKind::UnexpectedEof {
                TransferFailure::Refused(TransferError::Short {
                    declared: manifest.bytes,
                    written: sending.sent(),
                })
            } else {
                filesystem(local, error)
            }
        })?;

        sending.account(chunk.len())?;

        session.seal_into(&chunk, &mut sealed)?;
        connection.write_frame(Kind::Sealed, &sealed)?;
    }

    Ok(())
}

/// Receives the body of a transfer and puts it at `destination`.
///
/// The caller has already read and checked the manifest -- [`Manifest::check`] for the
/// size, the digest and the path, or [`Manifest::check_locally`] plus its own path
/// handling on a host. What this does is decide where the bytes go while they arrive,
/// and refuse to change the real path unless all of them did.
///
/// # Errors
///
/// [`TransferFailure`] for every refusal in this module's documentation. **The real
/// path is not touched on any of them**, and the `.part` does not survive.
pub fn receive_body(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    manifest: &Manifest,
    destination: &Path,
) -> Result<TransferOutcome, TransferFailure> {
    // T2, and before the temporary exists: a destination that is present and is not a
    // regular file is a directory, a symlink or a device. Writing through a symlink
    // writes where the operator did not intend, and the rename would replace the link
    // rather than follow it.
    if let Ok(metadata) = fs::symlink_metadata(destination)
        && !metadata.is_file()
    {
        return Err(TransferFailure::NotAFile {
            path: destination.display().to_string(),
        });
    }

    // T11 and T5. The message count of a transfer is not a protocol constant, so it is
    // bounded by the declared size -- and the bound is enforced by the connection, which
    // is what makes a chunk after the declared size a protocol error rather than a
    // longer file.
    connection.set_message_limit(connection.messages_read() + manifest.chunks());

    let temporary = part_path(destination);
    match copy_into(connection, session, manifest, &temporary) {
        Ok(outcome) => {
            // T9. The rename is the only moment the real path changes, and it is after
            // the digest has been compared and everything else has passed.
            fs::rename(&temporary, destination).map_err(|error| filesystem(destination, error))?;
            Ok(outcome)
        }
        Err(failure) => {
            // T8 and T9: the temporary does not survive, and the real path was never
            // touched. A stale `.part` would also be a file the next attempt has to
            // reason about.
            let _ = fs::remove_file(&temporary);
            Err(failure)
        }
    }
}

/// Writes the chunks into `temporary` and checks what landed there.
fn copy_into(
    connection: &mut Connection,
    session: &mut dyn Sealed,
    manifest: &Manifest,
    temporary: &Path,
) -> Result<TransferOutcome, TransferFailure> {
    let mut file = File::create(temporary).map_err(|error| filesystem(temporary, error))?;
    let mut receiving = Receiving::new(manifest.bytes);
    let mut chunk: Vec<u8> = Vec::new();

    // The loop ends when the declared size has arrived, which is T6's defence rather
    // than a check after it: a transfer that stops early cannot reach the rename,
    // because it cannot leave this loop.
    while !receiving.is_complete() {
        let frame = connection.read_frame(Kind::Sealed)?;
        session.open_into(&frame, &mut chunk)?;

        // T4, and **before** the write: a caller that wrote first has already filled the
        // disk it was trying not to fill.
        receiving.accept(chunk.len())?;
        file.write_all(&chunk)
            .map_err(|error| filesystem(temporary, error))?;
    }

    // T8. A write error that was logged and ignored produces a short file that fails
    // T7 on the sender's side with no explanation of why, so the flush is checked too.
    file.flush().map_err(|error| filesystem(temporary, error))?;
    drop(file);

    // T7, and this is the strongest reading of it: the digest is of what is **on the
    // disk**, read back, and not of what was written in memory. That is what catches a
    // write that silently short-wrote, or a `.part` that something else overwrote
    // between the write and the check.
    let got = digest_of_file(temporary)?;
    verify_digest(&manifest.sha256, &got)?;

    Ok(TransferOutcome {
        bytes: receiving.written(),
        sha256: got,
    })
}

/// A filesystem failure, with the path it was about.
///
/// The path is carried because the reader is looking at a machine they may not be
/// sitting at, and "access is denied" without a name is a message that starts a search
/// rather than ending one.
fn filesystem(path: &Path, error: std::io::Error) -> TransferFailure {
    TransferFailure::Filesystem {
        path: path.display().to_string(),
        problem: error.to_string(),
    }
}
