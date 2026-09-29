//! The connection: a length-prefixed protocol over a socket.
//!
//! `linklet_core::frame` decides what a frame is, and it is pure -- no socket, no
//! clock, no count. This is the layer that turns those bytes into a conversation,
//! and it is the layer where the four framing defences that need a socket live:
//!
//! 1. **A timeout on every read.** A peer that declares a length and then sends
//!    nothing holds a thread and a connection forever, and nothing pure can stop it.
//! 2. **No pipelining.** Every read asks for exactly the bytes it wants and the
//!    reader holds no buffer, so there is never a state in which a message boundary
//!    can be reinterpreted. This is the desynchronisation defence, and it is a
//!    property of this file rather than of the format.
//! 3. **The magic byte is checked here too**, because this is the side that reads
//!    from a stranger's socket.
//! 4. **Every write result is checked** -- `write_all`, and the error propagated
//!    rather than dropped.
//!
//! `docs/framing.md` is where those are written down and why they are not in the
//! frame module.

use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use linklet_core::frame::{self, FrameError, HEADER_BYTES, Kind};

/// Why a connection could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    /// The bytes were not a frame this protocol accepts.
    Frame(FrameError),
    /// The frame was a valid one of the wrong kind.
    WrongKind {
        /// What this point in the conversation requires.
        expected: Kind,
        /// What arrived.
        found: Kind,
    },
    /// No frame arrived within the budget.
    Timeout {
        /// The budget, in milliseconds, because a budget may be under a second.
        millis: u64,
    },
    /// The peer is gone: it closed the connection, or it was reset.
    ///
    /// **The two are not told apart**, and that is a decision rather than an
    /// oversight. A clean close and a reset differ in whether bytes in flight were
    /// lost, which the operating system knows and a protocol reader does not -- and
    /// the thing a caller does about either is the same, because there is no message
    /// and there will not be one. On Windows an abrupt close also carries a localised
    /// message, and reporting that would put a sentence in some other language in
    /// front of a user of a tool that speaks one.
    Ended,
    /// More messages arrived than the declared size allows.
    Budget {
        /// The number this connection agreed to read.
        limit: u64,
    },
    /// The connection cannot be read again.
    Unusable {
        /// Why, in the words of whatever failed first.
        why: String,
    },
    /// The socket failed for a reason of its own.
    Io(String),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(error) => write!(f, "{error}"),
            Self::WrongKind { expected, found } => write!(
                f,
                "expected a {} message and got a {} one",
                kind_name(*expected),
                kind_name(*found)
            ),
            Self::Timeout { millis } => {
                write!(f, "no message arrived within {millis} ms")
            }
            Self::Ended => write!(f, "the other end closed the connection"),
            Self::Budget { limit } => write!(
                f,
                "this connection agreed to {limit} messages and another one arrived"
            ),
            Self::Unusable { why } => {
                write!(f, "this connection cannot be read again: {why}")
            }
            Self::Io(problem) => write!(f, "{problem}"),
        }
    }
}

impl std::error::Error for ConnectionError {}

impl From<FrameError> for ConnectionError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

/// A kind's name as a person would say it.
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Hello => "hello",
        Kind::Sealed => "sealed",
    }
}

/// One framed conversation over one socket.
///
/// # What it guarantees
///
/// - **A timeout on every read.** [`Connection::set_budget`] is applied before each
///   read rather than once at the start, so a caller that shortens the budget for a
///   phase gets the budget it set.
/// - **No read-ahead.** There is no `BufReader` anywhere in this file: each read asks
///   the socket for exactly the bytes it wants. After a frame is returned, the only
///   bytes in flight are ones the peer sent and this side has not asked for, which is
///   the peer's problem rather than a boundary this side could misread.
/// - **A message budget.** [`Connection::set_message_limit`] is the transfer's
///   message-count defence (item 9 in the frame module's list does not survive a
///   transfer): the count is bounded by the declared size instead of by a constant.
pub struct Connection {
    stream: TcpStream,
    budget: Duration,
    limit: u64,
    read: u64,
    unusable: Option<String>,
}

impl Connection {
    /// How long a read waits when the caller does not say.
    ///
    /// Thirty seconds. Long enough for a command that takes its time to reply, short
    /// enough that a peer which sends nothing does not hold a thread for the
    /// afternoon. A caller with a different idea sets its own.
    pub const DEFAULT_BUDGET: Duration = Duration::from_secs(30);

    /// How many messages a connection reads before a transfer raises the limit.
    ///
    /// Two, which is what the handshake protocol already had: the hello, and the one
    /// message that answers it. A transfer is the only thing that needs more, and it
    /// knows how many more only after it has read the manifest.
    const INITIAL_LIMIT: u64 = 2;

    /// Wraps a socket with the default budget.
    pub fn new(stream: TcpStream) -> Self {
        Self::with_budget(stream, Self::DEFAULT_BUDGET)
    }

    /// Wraps a socket with a budget for each read.
    pub fn with_budget(stream: TcpStream, budget: Duration) -> Self {
        Self {
            stream,
            budget,
            limit: Self::INITIAL_LIMIT,
            read: 0,
            unusable: None,
        }
    }

    /// Changes the budget applied to every later read.
    pub fn set_budget(&mut self, budget: Duration) {
        self.budget = budget;
    }

    /// Sets how many messages this connection may read in total.
    ///
    /// The count for a transfer is not a protocol constant -- it is
    /// `chunks_for(bytes)` plus the messages that surround it -- so it is set here
    /// once the declared size is known. See `docs/transfer.md` T11.
    pub fn set_message_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    /// How many messages have been read, which is what the limit is checked against.
    pub fn messages_read(&self) -> u64 {
        self.read
    }

    /// Reads one frame, which must be of `expected` kind.
    ///
    /// # Errors
    ///
    /// [`ConnectionError`] for every refusal this layer makes. A refusal that is a
    /// timeout says only that the budget passed: whether the peer is slow, dead or
    /// refusing is not knowable here, and guessing is the failure this project has
    /// already paid for once.
    pub fn read_frame(&mut self, expected: Kind) -> Result<Vec<u8>, ConnectionError> {
        if let Some(why) = &self.unusable {
            return Err(ConnectionError::Unusable { why: why.clone() });
        }

        if self.read >= self.limit {
            return Err(ConnectionError::Budget { limit: self.limit });
        }

        let mut head = [0u8; HEADER_BYTES];
        self.read_exact(&mut head)?;
        let (kind, declared) = frame::decode_header(&head)?;
        if kind != expected {
            // **The refused frame's payload is still on the socket, and it has to be
            // consumed before this connection can be closed cleanly.** Windows sends a
            // reset for a socket closed with unread bytes in its queue, and a reset can
            // discard the answer this side just wrote -- so a caller that refused a
            // frame and then explained why would sometimes deliver only the refusal's
            // absence. That is a race, it was observed, and it looked exactly like the
            // agent ignoring the request.
            //
            // Bounded by `MAX_PAYLOAD`, which `decode_header` has already checked, and
            // by the read budget on each piece.
            self.discard(declared)?;
            return Err(ConnectionError::WrongKind {
                expected,
                found: kind,
            });
        }

        // Allocated only after `decode_header` has refused anything over the ceiling,
        // which is the ordering item 3 is about.
        let mut payload = vec![0u8; declared];
        self.read_exact(&mut payload)?;

        self.read += 1;
        Ok(payload)
    }

    /// Writes one frame, and flushes it.
    ///
    /// One frame, then it returns. Writing a second before reading anything is what
    /// the protocol refuses to do -- see `docs/transfer.md`, item 2 -- and the shape
    /// of this API is what makes that a choice rather than an accident.
    ///
    /// The write is bounded by the same budget as a read. A reply can be as large as a
    /// command's output, and a peer that stops reading fills its own window and leaves
    /// this side blocked in `write_all` for as long as the socket lives -- which on
    /// the agent is a thread, permanently, for a caller that never reads. Item 4 of
    /// `docs/framing.md` says every write result is checked; a write that never
    /// returns has no result to check, so it gets a deadline as well.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::Frame`] when the payload could not be framed at all, and
    /// [`ConnectionError::Io`] when the socket refused any part of the write.
    pub fn write_frame(&mut self, kind: Kind, payload: &[u8]) -> Result<(), ConnectionError> {
        let head = frame::header(kind, payload.len())?;
        let budget = self.budget;

        // `write_all`, and both results checked. A truncated write that nobody
        // noticed is a receiver waiting for the rest of a message the sender
        // believes it sent.
        if let Err(error) = self
            .stream
            .set_write_timeout(Some(budget))
            .and_then(|()| self.stream.write_all(&head))
            .and_then(|()| self.stream.write_all(payload))
            .and_then(|()| self.stream.flush())
        {
            return Err(self.failed("writing", error));
        }
        Ok(())
    }

    /// Reads and throws away a payload this side is refusing.
    ///
    /// A fixed scratch buffer rather than one allocation the size of the payload: the
    /// number came from a peer, and reserving sixteen mebibytes in order to discard
    /// them is an allocation an attacker chose.
    fn discard(&mut self, mut length: usize) -> Result<(), ConnectionError> {
        let mut scratch = [0u8; 8 * 1024];
        while length > 0 {
            let want = length.min(scratch.len());
            self.read_exact(&mut scratch[..want])?;
            length -= want;
        }
        Ok(())
    }

    /// Reads exactly `buffer.len()` bytes, or says why it could not.
    ///
    /// The budget is applied to the socket **before every read**, not once when the
    /// connection is made. It costs a syscall per frame and it is what makes
    /// [`Connection::set_budget`] mean something for a caller that shortens it for
    /// one phase of a conversation -- and it keeps the invariant in one place instead
    /// of in the constructor and the caller's memory.
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ConnectionError> {
        let budget = self.budget;
        if let Err(error) = self
            .stream
            .set_read_timeout(Some(budget))
            .and_then(|()| self.stream.read_exact(buffer))
        {
            return Err(self.failed("reading", error));
        }
        Ok(())
    }

    /// Turns an I/O failure into a refusal, and closes the connection to further
    /// reads.
    ///
    /// Closing is the part worth explaining. A `read_exact` that fails partway has
    /// consumed bytes that are now lost, so the next read would begin in the middle of
    /// the previous message -- the desynchronisation this whole design exists to
    /// prevent. There is no way to resume, so the connection says so instead of
    /// reading the remainder as though it were a boundary.
    fn failed(&mut self, doing: &str, error: std::io::Error) -> ConnectionError {
        let refusal = match error.kind() {
            // A clean close and an abrupt one mean the same thing to a reader: there is
            // no message and there will not be one. The difference is the operating
            // system's, and on Windows an abrupt close arrives as a reset carrying a
            // **localised** message -- which would otherwise be the sentence a user
            // reads, in a language the rest of this tool does not speak.
            ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::NotConnected => ConnectionError::Ended,
            ErrorKind::WouldBlock | ErrorKind::TimedOut => ConnectionError::Timeout {
                millis: self.budget.as_millis() as u64,
            },
            _ => ConnectionError::Io(format!("{doing}: {error}")),
        };

        self.unusable = Some(refusal.to_string());
        refusal
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("budget_ms", &self.budget.as_millis())
            .field("messages_read", &self.read)
            .field("limit", &self.limit)
            .field("unusable", &self.unusable)
            .finish()
    }
}
