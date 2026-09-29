//! The wire framing, and every way it could become a source of bugs.
//!
//! This module replaces HTTP. Every message between the host and the agent is
//! sealed, so HTTP's methods, paths, headers and status codes were vocabulary
//! nobody read, carried by a parser that had to be right about a grammar nobody
//! used. What is left is a length and a payload.
//!
//! **That is simpler than HTTP and it is not automatically safe.** Simplicity is
//! not a security argument, so this file starts with the ways a length-prefixed
//! protocol goes wrong and what stops each one. The list is the point of the
//! module; the code below it is small enough to read in one sitting.
//!
//! # Why a framing bug here is not the same as an HTTP smuggling bug
//!
//! This is the structural reason the trade is defensible, and it is worth being
//! precise about rather than assuming.
//!
//! In HTTP, a framing mistake -- a `Content-Length` that disagrees with a
//! `Transfer-Encoding: chunked`, say -- lets an attacker **inject a request the
//! server will act on**, because the server has no way to tell a smuggled request
//! from a real one. The front end and the back end disagree about boundaries and
//! both believe what they parsed.
//!
//! Here, **every payload goes straight into an AEAD that authenticates it.** So a
//! framing mistake cannot produce a message that is acted on: the bytes will not
//! open, and the reader refuses them. An attacker who lies about a length gets a
//! rejected message, not a forged one.
//!
//! The failure mode of a bug in this file is therefore **denial of service, not
//! compromise**. That is a real difference and it is the whole argument for
//! replacing HTTP with something smaller -- but it is not "this cannot go wrong",
//! and the denial-of-service paths are listed below with everything else.
//!
//! # The ways it goes wrong
//!
//! **1. The length lies, and is longer than the data.** The reader waits for bytes
//! that never come, holding a thread and a connection forever. *Stopped by a read
//! timeout on every read*, which lives in the adapter because this module has no
//! clock. See `docs/framing.md` for the budget.
//!
//! **2. The length lies, and is shorter than the data.** The surplus is read as the
//! beginning of the next message. This is framing desynchronisation, the
//! equivalent of HTTP request smuggling, and it is the failure that matters most.
//! *Stopped by refusing to pipeline*: the protocol is one request, one reply, and
//! the reader never has unread bytes it did not ask for. There is no state in
//! which a message boundary can be reinterpreted.
//!
//! **3. The length is enormous.** `Vec::with_capacity(declared)` on a hostile
//! number is an allocation the attacker chose. *Stopped by checking the declared
//! length against [`MAX_PAYLOAD`] **before** anything is allocated* -- see
//! [`decode_header`], which returns the number rather than acting on it, so the
//! caller cannot allocate first and check later by accident.
//!
//! **4. The length is zero.** An empty payload is refused rather than accepted,
//! because nothing this protocol sends is empty and a zero-length message is a
//! sign that something is wrong upstream. Accepting it would also make a
//! zero-filled buffer a valid message, which is a bad property to have.
//!
//! **5. The length is misread, because the two ends disagree about byte order.**
//! *Stopped by fixing big-endian in one place and pinning the exact bytes in a
//! test*, so the wire format is a fact rather than a convention two files share.
//!
//! **6. A partial read is treated as a whole one.** `read` may return fewer bytes
//! than asked for; using it without a loop is the classic version of this bug.
//! *Stopped by the adapter using `read_exact`*, and by this module's API taking a
//! complete header rather than a stream -- there is no way to call it with half a
//! header and have it guess.
//!
//! **7. A message from another protocol, or another version, is parsed as this
//! one.** A port that has the wrong service behind it, or a future version
//! talking to an old agent, would otherwise be read as a plausible length.
//! *Stopped by the magic byte*, which makes "this is not my protocol" a refusal at
//! the first byte instead of a large allocation three bytes later.
//!
//! **8. A message is read where a different kind is expected.** The connection has
//! an order -- hello, then sealed -- and a frame arriving out of that order means
//! the state machine and the bytes disagree. *Stopped by the kind byte being
//! checked by the caller against what it expects*, so the disagreement is named
//! rather than discovered later as an authentication failure.
//!
//! **9. A peer sends messages forever.** Each one is valid, so no single check
//! refuses it, and the thread never ends. *Stopped by a message count per
//! connection*, which the adapter enforces because the count belongs to the
//! connection rather than to a frame.
//!
//! **10. A write is truncated and the sender does not notice.** The receiver waits
//! for the rest of a message the writer believes it sent. *Stopped by checking
//! every write result* in the adapter -- `write_all` rather than `write`, and the
//! error propagated rather than dropped.
//!
//! # What this module deliberately does not do
//!
//! No compression, no chunking, no multiplexing, no negotiation, and no
//! checksum -- the payload is sealed and authenticated by the layer above, so a
//! checksum here would be a second, weaker integrity check on data that is
//! already covered. Adding one would be the kind of redundancy that makes a reader
//! unsure which check is load-bearing.

/// The first byte of every frame.
///
/// `0x4C` is `L`, which is not a security measure -- it is a byte an operator can
/// recognise in a hex dump. What it buys is item 7 above: a service on the wrong
/// port is refused at the first byte instead of being read as a length.
pub const MAGIC: u8 = 0x4C;

/// The bytes before every payload: magic, kind, and a 32-bit length.
pub const HEADER_BYTES: usize = 6;

/// The largest payload this protocol will carry.
///
/// Sixteen mebibytes, matching the agent's body ceiling. The number is not
/// arbitrary: the payload is read into memory in full, so the real limit is this
/// multiplied by the number of connections, and streaming is what would remove it.
/// It is checked *before* any allocation, which is the defence for item 3.
pub const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// What a frame is for.
///
/// Carried on the wire so that a frame arriving at the wrong point in a connection
/// is refused by name rather than surfacing later as an authentication failure --
/// item 8 above. The bytes are part of the format and are pinned by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The first message on a connection: a public key, in the clear.
    Hello,
    /// Every message after it: a sealed body.
    Sealed,
}

impl Kind {
    /// The byte this kind travels as.
    pub fn as_byte(self) -> u8 {
        match self {
            Self::Hello => 1,
            Self::Sealed => 2,
        }
    }

    /// The kind a byte names, or `None`.
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Hello),
            2 => Some(Self::Sealed),
            _ => None,
        }
    }
}

/// Why a frame could not be built or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The first byte is not [`MAGIC`], so this is not this protocol.
    NotThisProtocol {
        /// The byte that was there instead.
        found: u8,
    },
    /// The kind byte names nothing this version knows.
    UnknownKind {
        /// The byte that was there instead.
        found: u8,
    },
    /// The declared length is larger than [`MAX_PAYLOAD`].
    ///
    /// Carries the declared number rather than the limit, because the caller is a
    /// protocol reader and "declared 4294967295" says what happened while "too
    /// large" does not.
    TooLarge {
        /// What the frame claimed.
        declared: usize,
    },
    /// The declared length is zero.
    Empty,
    /// Too few bytes to be a header.
    Truncated {
        /// How many arrived.
        got: usize,
    },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotThisProtocol { found } => write!(
                f,
                "the first byte is {found:#04x} and this protocol starts with {MAGIC:#04x}"
            ),
            Self::UnknownKind { found } => write!(f, "message kind {found} is not one of 1 or 2"),
            Self::TooLarge { declared } => write!(
                f,
                "a payload of {declared} bytes is larger than the {MAX_PAYLOAD} byte limit"
            ),
            Self::Empty => write!(
                f,
                "a message with no payload is not one this protocol sends"
            ),
            Self::Truncated { got } => write!(
                f,
                "{got} bytes is too few for the {HEADER_BYTES} byte header"
            ),
        }
    }
}

impl std::error::Error for FrameError {}

/// Builds the header for a payload of `length` bytes.
///
/// # Errors
///
/// [`FrameError::Empty`] for a zero length and [`FrameError::TooLarge`] beyond the
/// limit. **Refusing here rather than at the reader is the point**: a sender that
/// would produce an unacceptable frame learns so before sending it, instead of the
/// receiver discovering it after a partial transfer.
pub fn header(kind: Kind, length: usize) -> Result<[u8; HEADER_BYTES], FrameError> {
    if length == 0 {
        return Err(FrameError::Empty);
    }
    if length > MAX_PAYLOAD {
        return Err(FrameError::TooLarge { declared: length });
    }

    // Big-endian, written once and never elsewhere: item 5 is about two files
    // disagreeing, and the way to stop that is to have only one file that knows.
    let mut out = [0u8; HEADER_BYTES];
    out[0] = MAGIC;
    out[1] = kind.as_byte();
    out[2..].copy_from_slice(&(length as u32).to_be_bytes());
    Ok(out)
}

/// Reads a header and returns what it declares.
///
/// **Returns the length rather than acting on it.** A caller that allocated from
/// the declared number before this function had a chance to refuse it would have
/// the bug this API is shaped to prevent -- item 3.
///
/// # Errors
///
/// [`FrameError::Truncated`] for too few bytes, [`FrameError::NotThisProtocol`] for
/// a wrong magic byte, [`FrameError::UnknownKind`] for a kind this version does not
/// know, and the same length refusals as [`header`].
pub fn decode_header(bytes: &[u8]) -> Result<(Kind, usize), FrameError> {
    if bytes.len() < HEADER_BYTES {
        return Err(FrameError::Truncated { got: bytes.len() });
    }

    if bytes[0] != MAGIC {
        return Err(FrameError::NotThisProtocol { found: bytes[0] });
    }

    let kind = Kind::from_byte(bytes[1]).ok_or(FrameError::UnknownKind { found: bytes[1] })?;

    // `u32` to `usize` is lossless on every target this project builds for, and the
    // cast is written out rather than left to inference so that the day a 16-bit
    // target appears is the day this line is read.
    let declared = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]) as usize;

    if declared == 0 {
        return Err(FrameError::Empty);
    }
    if declared > MAX_PAYLOAD {
        return Err(FrameError::TooLarge { declared });
    }

    Ok((kind, declared))
}

/// A header followed by its payload, built in one allocation.
///
/// Used by the sending side, which knows the whole message. The two calls it makes
/// are the same checks the reader makes, so a sender cannot produce a frame the
/// receiver would refuse for a length reason.
///
/// # Errors
///
/// As [`header`].
pub fn encode(kind: Kind, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    let head = header(kind, payload.len())?;
    let mut out = Vec::with_capacity(HEADER_BYTES + payload.len());
    out.extend_from_slice(&head);
    out.extend_from_slice(payload);
    Ok(out)
}

/// The whole frame length for a payload, for a sender sizing a buffer.
///
/// # Errors
///
/// As [`header`].
pub fn frame_length(kind: Kind, payload_len: usize) -> Result<usize, FrameError> {
    header(kind, payload_len).map(|_| HEADER_BYTES + payload_len)
}
