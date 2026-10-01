//! The agent's own log file, and the line per request that goes in it.
//!
//! `linklet_core::log` decides **what a line says**; this decides **where it goes and
//! what happens when it cannot**. That split is rule 1 of `AGENTS.md`, and the reason is
//! the usual one: the format is testable in microseconds, and what is left here is a
//! `File` and a `Mutex`.
//!
//! # Why the agent writes its own file
//!
//! **A shell redirect is not a logging strategy, and that was measured rather than
//! assumed.** A program started detached through a `cmd /c ... > file` redirect writes
//! nothing to that file -- the file is created and stays empty -- which is what
//! `C:\linklet\agent.log` on the first real target was: zero bytes, for an agent that had
//! been answering calls all afternoon. `AGENTS.md` section 8 has the measurement.
//!
//! # Why nothing here fails loudly
//!
//! A request that cannot be logged is still answered. The alternative -- refusing a
//! command because a log file could not be written -- trades a working machine for a
//! missing diagnostic, and the caller on the other end of the socket has no way to
//! understand what it did wrong. What the agent does instead is say so once, on standard
//! error, at startup, which is where a person who is starting it is looking.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use linklet_core::log::{Id, Line, Log, Operation, Outcome};

/// Where the agent's request lines go.
///
/// `Arc<Mutex<..>>` rather than either half on its own: there is a thread per connection,
/// so the number counter has to be shared, and the file handle cannot be written by two
/// threads at once or two lines interleave into one.
#[derive(Clone)]
pub struct RequestLog {
    /// `None` when the agent was not told to log, which is the default.
    inner: Option<Arc<Mutex<Sink>>>,
}

/// The counter and the file, behind one lock so a line is numbered and written together.
struct Sink {
    /// The numbering, which is the part `linklet-core` decides.
    log: Log,
    /// The file the lines go to.
    file: File,
}

impl RequestLog {
    /// A log that writes to nothing.
    ///
    /// The default, and it is not a stub: an agent that was not asked to keep a log keeps
    /// none, and every call below is then a no-op rather than a failure.
    pub fn none() -> Self {
        Self { inner: None }
    }

    /// Opens `path` and appends to it, creating it if it is not there.
    ///
    /// **Appends rather than truncates**, so restarting an agent does not destroy the
    /// record of what it was asked before it died -- which is the one incident the log
    /// exists for.
    ///
    /// # Errors
    ///
    /// The reason it could not be opened, as a sentence for the operator. The agent
    /// treats this as a startup failure: an operator who asked for a log and silently did
    /// not get one has a machine whose evidence they believe exists and does not. That is
    /// the mistake this whole feature is a reaction to, and repeating it in the launcher
    /// would be a poor joke.
    pub fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Err(format!("{} is a directory", path.display()));
        }

        // `create(true)` rather than `create_new(true)`: the file is expected on the
        // second start, and a log that refused to open because it already existed would
        // be a log that only ever recorded the first run.
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;

        Ok(Self {
            inner: Some(Arc::new(Mutex::new(Sink {
                log: Log::new(),
                file,
            }))),
        })
    }

    /// Records that a request has been taken, and returns the number it was given.
    ///
    /// The returned number is what [`RequestLog::answered`] needs; `None` means this
    /// agent is not logging, and the caller passes it back unexamined.
    ///
    /// **Written before the request is handled**, which is the whole point: a request
    /// that never finishes leaves this line and no other, and that pair -- a `->` with no
    /// `<-` -- is what names the request that wedged or killed the agent.
    pub fn taken(&self, operation: Operation) -> Option<Id> {
        let inner = self.inner.as_ref()?;
        let Ok(mut sink) = inner.lock() else {
            // A poisoned lock means another thread panicked while holding it. Recording
            // nothing is the right answer for a log: the alternative is a second panic
            // in a thread that was serving a caller.
            return None;
        };

        let (id, line) = sink.log.taken(operation);
        write_line(&mut sink.file, &line);
        Some(id)
    }

    /// Records that a request has been answered.
    ///
    /// `id` is what [`RequestLog::taken`] returned, and it is what ties the two lines
    /// together. A `None` id means the agent is not logging.
    pub fn answered(
        &self,
        id: Option<Id>,
        operation: Operation,
        outcome: Outcome,
        millis: u64,
        reason: Option<String>,
    ) {
        let (Some(id), Some(inner)) = (id, self.inner.as_ref()) else {
            return;
        };
        let Ok(mut sink) = inner.lock() else {
            return;
        };

        let line = sink.log.answered(id, operation, outcome, millis, reason);
        write_line(&mut sink.file, &line);
    }
}

/// Writes one line, terminated, and ignores a failure.
///
/// The ignoring is stated rather than silent: see the module comment. It is the one place
/// in the agent where an error is dropped on purpose, and the reason is that there is
/// nothing a caller could do about it and something they would wrongly do -- a command
/// that ran would be reported as a failure if this were propagated.
fn write_line(file: &mut File, line: &Line) {
    let _ = writeln!(file, "{}", line.render());
}
