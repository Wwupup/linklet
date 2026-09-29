//! Where a transfer is allowed to write, and every way a path lies about that.
//!
//! `docs/transfer.md` T1: the destination is the caller's, so a path the caller
//! chooses is a path an attacker chooses if anything upstream is confused. On
//! Windows that is not one check but several, because a path can name a file
//! without saying so.
//!
//! This module is pure: it takes strings and returns a decision. That is what lets
//! every rule below be tested in microseconds and what keeps the filesystem work --
//! does it exist, is it a link, is it a regular file -- in the adapter where it
//! belongs.
//!
//! # The rules, and what each one stops
//!
//! **`..` in any component.** The obvious one. `..\..\Windows\System32\drivers\etc\hosts`
//! is a write as SYSTEM on someone else's machine.
//!
//! **A path that is absolute and outside the root.** An absolute path is not
//! automatically wrong -- the operator may well have configured the root as
//! `C:\linklet` and asked for `C:\linklet\build.exe` -- so this is a prefix check
//! against the root rather than a refusal of absolutes.
//!
//! **A colon anywhere.** This is the Windows rule that is least obvious and most
//! important. `file.txt:evil` is not a file called `file.txt:evil`, it is an
//! **alternate data stream** on `file.txt`: it writes bytes that do not appear in a
//! directory listing and that no ordinary tool will show you. `C:foo` is not a
//! drive -- it is `foo` **relative to whatever the current directory is on drive
//! C**, which is a different file depending on how the process was started. One
//! rule refuses both.
//!
//! **A leading `\\`.** A UNC path is a network share, so it writes to another
//! machine entirely, outside any root.
//!
//! **A component ending in a dot or a space.** Windows strips them, so `build.exe.`
//! and `build.exe ` and `build.exe` are the same file. A check that compares names
//! literally would pass a name that becomes a different one on disk.
//!
//! **A reserved device name.** `NUL`, `CON`, `AUX`, `PRN`, `COM1`..`COM9`,
//! `LPT1`..`LPT9`, with or without an extension and in any case. `NUL` is the one
//! that matters for a transfer: **writing to it succeeds and discards the bytes**,
//! so a push that "verified its digest" would report success and have written
//! nothing.
//!
//! **A path that is empty or contains a NUL byte.** The second cannot reach the
//! filesystem API, but a string that contains one is a sign that something
//! upstream is not doing what it thinks.
//!
//! # What is not here, deliberately
//!
//! **Whether the answer exists, is a link, or is a directory.** Those are questions
//! for the filesystem and they are asked in the adapter, before the temporary file
//! is opened. A validator that guessed at them from the string would be answering
//! a question it cannot see.

use std::path::{Component, Path, PathBuf};

/// The largest transfer this project will agree to receive.
///
/// A policy ceiling rather than a protocol constant -- `docs/transfer.md` T3 and
/// T11 -- so the message count of a transfer is bounded by bounding the bytes. Four
/// gibibytes is chosen to be larger than any build artifact and small enough that
/// refusing one is a decision rather than an accident.
pub const MAX_TRANSFER_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The device names Windows resolves instead of creating a file.
///
/// Checked with and without an extension and in any case, because `NUL` and
/// `nul.txt` are the same thing to the operating system.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Why a destination was refused.
///
/// Every variant carries the thing that caused it. A caller reading this is a
/// person looking at a path that did not work, and "invalid path" would leave them
/// comparing it against a manual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// The caller asked for nothing.
    Empty,
    /// The path holds a NUL byte.
    NulByte,
    /// A component is `..`.
    Parent {
        /// The path as it was given.
        requested: String,
    },
    /// A colon appears, so this is a stream or a drive-relative path.
    Colon {
        /// The path as it was given.
        requested: String,
    },
    /// The path begins with `\\`, which is a share on another machine.
    Network {
        /// The path as it was given.
        requested: String,
    },
    /// A component ends in a dot or a space, which Windows strips.
    TrailingDotOrSpace {
        /// The component that does.
        component: String,
    },
    /// A component names a device rather than a file.
    Reserved {
        /// The component that does.
        component: String,
    },
    /// The path resolves outside the configured root.
    OutsideRoot {
        /// The path as it was given.
        requested: String,
        /// The root it had to stay inside.
        root: String,
    },
    /// The configured root is not usable.
    BadRoot {
        /// Why.
        why: String,
    },
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the path is empty"),
            Self::NulByte => write!(f, "the path contains a NUL byte"),
            Self::Parent { requested } => write!(
                f,
                "{requested:?} contains a .. component, which leaves the directory it is in"
            ),
            Self::Colon { requested } => write!(
                f,
                "{requested:?} contains a colon, so it names an alternate data stream or is \
                 relative to a drive's current directory rather than to the root"
            ),
            Self::Network { requested } => write!(
                f,
                "{requested:?} begins with a double backslash, which is a share on another \
                 machine"
            ),
            Self::TrailingDotOrSpace { component } => write!(
                f,
                "{component:?} ends in a dot or a space, which Windows removes -- so it would \
                 write to a file with a different name"
            ),
            Self::Reserved { component } => write!(
                f,
                "{component:?} is a device, not a file. On Windows the write would succeed and \
                 the bytes would be discarded"
            ),
            Self::OutsideRoot { requested, root } => write!(
                f,
                "{requested:?} resolves outside {root:?}, which is the only directory a \
                 transfer may write in"
            ),
            Self::BadRoot { why } => write!(f, "the configured root is unusable: {why}"),
        }
    }
}

impl std::error::Error for PathError {}

/// The one directory a transfer may write into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    root: PathBuf,
}

impl Destination {
    /// Fixes the root.
    ///
    /// # Errors
    ///
    /// [`PathError::BadRoot`] when the root is empty, relative, or has one of the
    /// shapes this module refuses in a request. A root that is itself relative
    /// would make every decision below depend on the process's working directory,
    /// which is the thing the root exists to remove.
    pub fn new(root: &str) -> Result<Self, PathError> {
        if root.trim().is_empty() {
            return Err(PathError::BadRoot {
                why: "it is empty".to_string(),
            });
        }
        if root.contains('\0') {
            return Err(PathError::BadRoot {
                why: "it contains a NUL byte".to_string(),
            });
        }
        // A root that is not absolute is refused rather than joined to the working
        // directory, because then "inside the root" would mean different things
        // depending on how the process was started.
        if !Path::new(root).is_absolute() {
            return Err(PathError::BadRoot {
                why: format!("{root:?} is not an absolute path"),
            });
        }
        if root.starts_with("\\\\") {
            return Err(PathError::BadRoot {
                why: "it is a network share".to_string(),
            });
        }
        // Exactly one colon, and only as the drive separator. `root[1..]` starts
        // with that colon, so searching it for another one finds it immediately --
        // which is what made this refuse `C:\linklet` on the first run. The search
        // starts after it.
        let after_the_drive = if root.as_bytes().get(1) == Some(&b':') {
            &root[2..]
        } else {
            root
        };
        if after_the_drive.contains(':') {
            return Err(PathError::BadRoot {
                why: "it contains an alternate data stream".to_string(),
            });
        }

        Ok(Self {
            root: PathBuf::from(root),
        })
    }

    /// The root, for a message.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Decides where a requested path actually goes.
    ///
    /// # Errors
    ///
    /// [`PathError`] for every rule in this module's documentation. The returned
    /// path is guaranteed to be inside the root **by the rules below**, not by
    /// whether it happens to exist.
    pub fn resolve(&self, requested: &str) -> Result<PathBuf, PathError> {
        if requested.trim().is_empty() {
            return Err(PathError::Empty);
        }
        if requested.contains('\0') {
            return Err(PathError::NulByte);
        }
        // A share first, because `\\\\?\\C:\\...` contains a colon as well and would
        // otherwise be refused as a stream -- the right refusal for the wrong reason,
        // which is how a message sends a reader to the wrong place.

        // One rule for three problems, and the exemption is the drive separator.
        // `file.txt:stream` is an alternate data stream; `C:foo` is relative to drive
        // C's current directory rather than to the root; and `C:\linklet\build.exe`
        // is an ordinary absolute path whose first colon is the drive. Refusing every
        // colon refused the last one too, which the test caught.
        let after_the_drive = if requested.as_bytes().get(1) == Some(&b':') {
            &requested[2..]
        } else {
            requested
        };
        if !requested.starts_with("\\") && after_the_drive.contains(":") {
            return Err(PathError::Colon {
                requested: requested.to_string(),
            });
        }
        if requested.starts_with("\\\\") || requested.starts_with("//") {
            return Err(PathError::Network {
                requested: requested.to_string(),
            });
        }

        let candidate = Path::new(requested);

        // `C:build.exe` has a prefix and no root, so `is_absolute()` is false and it
        // would be joined to the root as though it were an ordinary relative name. It
        // is not: it means build.exe relative to whatever the current directory is on
        // drive C, which is a different file depending on how the process started.
        {
            let mut parts = candidate.components();
            if matches!(parts.next(), Some(Component::Prefix(_)))
                && !matches!(parts.next(), Some(Component::RootDir))
            {
                return Err(PathError::Colon {
                    requested: requested.to_string(),
                });
            }
        }
        for component in candidate.components() {
            match component {
                Component::ParentDir => {
                    return Err(PathError::Parent {
                        requested: requested.to_string(),
                    });
                }
                Component::Normal(part) => {
                    let part = part.to_string_lossy();
                    if part.ends_with('.') || part.ends_with(' ') {
                        return Err(PathError::TrailingDotOrSpace {
                            component: part.into_owned(),
                        });
                    }
                    // The stem, so that `nul.txt` is caught as well as `nul`.
                    let stem = Path::new(part.as_ref())
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_uppercase())
                        .unwrap_or_default();
                    if RESERVED.contains(&stem.as_str()) {
                        return Err(PathError::Reserved {
                            component: part.into_owned(),
                        });
                    }
                }
                // A prefix is a drive or a share; a root directory is the separator.
                // Both are handled by the absolute-path branch below.
                Component::Prefix(_) | Component::RootDir | Component::CurDir => {}
            }
        }

        let resolved = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        if !within(&self.root, &resolved) {
            return Err(PathError::OutsideRoot {
                requested: requested.to_string(),
                root: self.root.display().to_string(),
            });
        }

        Ok(resolved)
    }
}

/// Whether `candidate` is inside `root`, by components rather than by string.
///
/// Compared component-wise and case-insensitively, because Windows paths are
/// case-insensitive and a prefix test on the string form would accept
/// `C:\linkletevil` for the root `C:\linklet`. That is the mistake this function
/// exists to not make.
fn within(root: &Path, candidate: &Path) -> bool {
    let mut root_parts = root.components().filter_map(plain);
    let mut candidate_parts = candidate.components().filter_map(plain);

    loop {
        match (root_parts.next(), candidate_parts.next()) {
            // The root ran out and the candidate did not: it is inside.
            (None, _) => return true,
            // The candidate ran out first: it is the root's parent or the root.
            (Some(_), None) => return false,
            (Some(want), Some(got)) => {
                if !want.eq_ignore_ascii_case(&got) {
                    return false;
                }
            }
        }
    }
}

/// A component as a comparable string, or `None` for the parts that are not names.
fn plain(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
        // A prefix carries the drive, which has to match for two absolute paths to
        // be in the same tree.
        Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().into_owned()),
        Component::RootDir => Some(String::new()),
        Component::CurDir => None,
        Component::ParentDir => Some("..".to_string()),
    }
}

/// The size of one chunk of a transfer.
///
/// One mebibyte, and the number matters twice. It is well under the frame ceiling so
/// a chunk never meets it, and it is large enough that the per-chunk round trip --
/// which `docs/transfer.md` chose to keep, so that the desynchronisation defence
/// keeps holding -- costs something acceptable on a LAN. A smaller chunk buys
/// latency back; a larger one buys throughput at the cost of holding more memory
/// per connection.
pub const CHUNK_BYTES: u64 = 1024 * 1024;

/// How many chunks a transfer of `bytes` takes.
///
/// `docs/transfer.md` T11: the message count of a transfer is no longer bounded by a
/// protocol constant, so it is bounded by the declared size instead, through this.
///
/// **The arithmetic is a function with tests rather than an expression inside a
/// loop**, because an off-by-one here is either a transfer that never finishes or
/// one that sends a chunk past its declared size -- and the second is T4.
///
/// A zero-byte transfer takes no chunks. It cannot happen, because the manifest
/// refuses an empty file, and it is defined rather than left to fall out of the
/// division.
pub fn chunks_for(bytes: u64, chunk: u64) -> u64 {
    if chunk == 0 {
        return 0;
    }
    bytes.div_ceil(chunk)
}

/// Why a transfer could not be carried through.
///
/// One variant per numbered failure in `docs/transfer.md`, because a caller that has
/// to act differently on "the sender lied" and "the disk filled" needs to be able to
/// tell them apart, and every variant carries the numbers involved: this error is
/// read by whoever is pushing a build, and "too large" does not say whether the
/// sender is broken or the file changed underneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// A chunk would take the running total past the declared size. T4.
    TooMuch {
        /// What the manifest declared.
        declared: u64,
        /// What had already been accepted.
        written: u64,
        /// What this chunk carried.
        chunk: u64,
    },
    /// A chunk arrived after the declared size had been reached. T5.
    PastTheEnd {
        /// What the manifest declared, which is already in hand.
        declared: u64,
    },
    /// The transfer ended before the declared size arrived. T6.
    Short {
        /// What the manifest declared.
        declared: u64,
        /// What actually arrived.
        written: u64,
    },
    /// What arrived is not what was sent. T7.
    Digest {
        /// The digest the sender declared.
        expected: String,
        /// The digest of what was received.
        got: String,
    },
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooMuch {
                declared,
                written,
                chunk,
            } => write!(
                f,
                "{chunk} more bytes would take this transfer to {} of the {declared} it \
                 declared",
                written + chunk
            ),
            Self::PastTheEnd { declared } => write!(
                f,
                "the transfer of {declared} bytes is complete and something followed it"
            ),
            Self::Short { declared, written } => write!(
                f,
                "the transfer ended {written} bytes into the {declared} it declared"
            ),
            Self::Digest { expected, got } => write!(
                f,
                "what arrived hashes to {got}, and the sender declared {expected}"
            ),
        }
    }
}

impl std::error::Error for TransferError {}

/// A transfer being received: the declared size and the running total.
///
/// `docs/transfer.md` T4 is the item most likely to be missed, because the declared
/// number *was* checked -- at the start, before any chunk. This is the check that it
/// is still respected as the chunks arrive, and it is a value rather than an
/// expression inside a loop so that the arithmetic can be tested in microseconds
/// while the loop it belongs to needs a socket.
///
/// It is also where T5 is enforced: a chunk after the declared size has arrived is a
/// protocol error and not merely an overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receiving {
    declared: u64,
    written: u64,
}

impl Receiving {
    /// Starts a transfer of `declared` bytes.
    pub fn new(declared: u64) -> Self {
        Self {
            declared,
            written: 0,
        }
    }

    /// Accounts for one chunk **before it is written**.
    ///
    /// Before, not after: a caller that wrote first and checked afterwards has already
    /// put the bytes on the disk it was trying not to fill.
    ///
    /// # Errors
    ///
    /// [`TransferError::PastTheEnd`] when the declared size has already been reached,
    /// and [`TransferError::TooMuch`] when this chunk would pass it.
    pub fn accept(&mut self, chunk: usize) -> Result<(), TransferError> {
        let chunk = chunk as u64;

        if self.is_complete() {
            return Err(TransferError::PastTheEnd {
                declared: self.declared,
            });
        }
        if self.written + chunk > self.declared {
            return Err(TransferError::TooMuch {
                declared: self.declared,
                written: self.written,
                chunk,
            });
        }

        self.written += chunk;
        Ok(())
    }

    /// How many bytes the manifest declared.
    pub fn declared(&self) -> u64 {
        self.declared
    }

    /// How many bytes have been accepted.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Whether the declared size has arrived, which is the condition for renaming.
    ///
    /// T6: a caller that renamed without asking would put a short file at the real
    /// path, and a short file is worse than no file because the next step believes it.
    pub fn is_complete(&self) -> bool {
        self.written >= self.declared
    }
}

/// A transfer being sent: the declared size and what has been handed over.
///
/// The mirror of [`Receiving`], and it exists for the same reason from the other
/// side: **the sender never offers a chunk it has not declared**. The last chunk is
/// where that goes wrong, because a fixed-size read past the remainder would offer
/// bytes the receiver has already refused to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sending {
    declared: u64,
    sent: u64,
}

impl Sending {
    /// Starts a transfer of `declared` bytes.
    pub fn new(declared: u64) -> Self {
        Self { declared, sent: 0 }
    }

    /// The most the next chunk may carry, or `None` when nothing is owed.
    ///
    /// `None` rather than zero: a caller looping on a length would spin forever on a
    /// zero, and the difference between "send nothing" and "you are finished" is the
    /// difference between a hung push and a completed one.
    pub fn next_chunk(&self) -> Option<usize> {
        if self.is_complete() {
            return None;
        }
        Some((self.declared - self.sent).min(CHUNK_BYTES) as usize)
    }

    /// Accounts for a chunk handed to the connection.
    ///
    /// # Errors
    ///
    /// [`TransferError::TooMuch`] when the chunk is longer than what was declared.
    /// The caller reads a file, and a file can grow between being hashed and being
    /// sent -- so this is a check on the sender's own loop and not on a peer.
    pub fn account(&mut self, chunk: usize) -> Result<(), TransferError> {
        let chunk = chunk as u64;
        if self.sent + chunk > self.declared {
            return Err(TransferError::TooMuch {
                declared: self.declared,
                written: self.sent,
                chunk,
            });
        }
        self.sent += chunk;
        Ok(())
    }

    /// How many bytes have been handed over.
    ///
    /// For the error a sender reports when the file it is reading turns out to be
    /// shorter than the digest it took said it was.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Whether everything declared has been handed over.
    pub fn is_complete(&self) -> bool {
        self.sent >= self.declared
    }
}

/// Compares the digest a sender declared with the one a receiver computed.
///
/// `docs/transfer.md` T7. The AEAD already authenticates the bytes as they cross, so
/// this catches the layers above it: a framing bug, a write that silently
/// short-wrote, and a `.part` that something else overwrote between the write and the
/// check.
///
/// Case-sensitive, and that is a consequence rather than a preference: the manifest
/// requires lowercase hex, so there is one form of a digest in this protocol and the
/// comparison does not have to think about the other.
///
/// # Errors
///
/// [`TransferError::Digest`] with both values. The person reading it needs to see
/// whether the difference looks like a truncation, a different file, or a case
/// difference that should not be possible.
pub fn verify_digest(expected: &str, got: &str) -> Result<(), TransferError> {
    if expected == got {
        return Ok(());
    }
    Err(TransferError::Digest {
        expected: expected.to_string(),
        got: got.to_string(),
    })
}

/// What a transfer says about itself before any of it arrives.
///
/// Checked in full **before the first chunk is read**, which is the defence for T3
/// and half of T11: a receiver that agreed to a size it had not checked would be
/// agreeing to receive it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Where the receiving side should put it, relative to its root.
    pub path: String,
    /// How many bytes are coming.
    pub bytes: u64,
    /// The digest of what was sent, lowercase hex.
    pub sha256: String,
}

/// Why a manifest was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// The declared size is over [`MAX_TRANSFER_BYTES`].
    TooLarge {
        /// What it declared.
        bytes: u64,
    },
    /// The declared size is zero.
    ///
    /// Refused for the same reason a zero-length frame is: nothing this protocol
    /// sends is empty, and accepting it would make an empty file a valid transfer
    /// that a caller might then believe.
    Empty,
    /// The digest is not 64 lowercase hex digits.
    BadDigest {
        /// What it was.
        got: String,
    },
    /// The destination was refused.
    Path(PathError),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                f,
                "a transfer of {bytes} bytes is larger than the {MAX_TRANSFER_BYTES} byte limit"
            ),
            Self::Empty => write!(f, "a transfer of no bytes is not one this protocol sends"),
            Self::BadDigest { got } => write!(
                f,
                "the digest is {got:?}, which is not 64 lowercase hex digits"
            ),
            Self::Path(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<PathError> for ManifestError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl Manifest {
    /// Checks the parts of a manifest that do not depend on the receiving side.
    ///
    /// The sender runs this **before the first chunk**, so a transfer that cannot be
    /// accepted is refused locally rather than after a gigabyte has crossed the
    /// network -- the same argument the framing module makes for refusing an
    /// unacceptable frame at the sender.
    ///
    /// # Errors
    ///
    /// [`ManifestError::Empty`], [`ManifestError::TooLarge`] and
    /// [`ManifestError::BadDigest`]. **Not the path**: the path in a manifest the
    /// sender holds names a place on someone else's machine, and this side cannot
    /// resolve it. Splitting the check is what stops the two ends disagreeing about
    /// who validates what.
    pub fn check_locally(&self) -> Result<(), ManifestError> {
        if self.bytes == 0 {
            return Err(ManifestError::Empty);
        }
        if self.bytes > MAX_TRANSFER_BYTES {
            return Err(ManifestError::TooLarge { bytes: self.bytes });
        }
        if !is_lower_hex_64(&self.sha256) {
            return Err(ManifestError::BadDigest {
                got: self.sha256.clone(),
            });
        }
        Ok(())
    }

    /// Checks a manifest and resolves where it goes.
    ///
    /// Does the whole check in one call on purpose: a caller that could check the
    /// size without checking the path, or the reverse, is a caller that will do one
    /// of them and believe it did both.
    ///
    /// # Errors
    ///
    /// [`ManifestError`] for every rule above.
    pub fn check(&self, destination: &Destination) -> Result<PathBuf, ManifestError> {
        self.check_locally()?;
        Ok(destination.resolve(&self.path)?)
    }

    /// How many chunks this transfer takes.
    pub fn chunks(&self) -> u64 {
        chunks_for(self.bytes, CHUNK_BYTES)
    }
}

/// Whether a string is exactly 64 lowercase hex digits.
///
/// Lowercase only, and that is a decision rather than an oversight: **two digests
/// that differ only in case compare unequal as strings and equal as digests**, so
/// accepting both cases means the comparison has to be case-insensitive and every
/// reader of this code has to know that. Fixing the case fixes the comparison.
fn is_lower_hex_64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
