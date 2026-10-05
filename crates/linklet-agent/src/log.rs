//! The agent's own log file, and the line per request that goes in it.
//!
//! `linklet_core::log` decides **what a line says** and **when the file is too big**;
//! this decides **where it goes and what happens when it cannot**. That split is rule 1
//! of `AGENTS.md`, and the reason is the usual one: the format and the bound are testable
//! in microseconds, and what is left here is a `File` and a `Mutex`.
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
//!
//! # Rotation, and the one thing it must not do
//!
//! The agent appends and never truncates, so the file is bounded by
//! [`Rotation`] instead -- see `linklet_core::log` for the policy. What is decided here
//! is *when it is safe to roll over*, and there is exactly one constraint:
//!
//! > **A rotation must not split a pair.** The log is read by counting `->` lines that
//! > have no `<-`, so a roll-over between a request being taken and being answered would
//! > put its two halves in different files and report a request as unfinished when it
//! > finished. So a rotation is deferred while any request is in flight, and the size
//! > check is made before a `->` line rather than at an arbitrary moment.
//!
//! That deferral cannot grow without bound in practice: the case that keeps a request in
//! flight forever is a request that never finishes, and a request that never finishes is
//! also not writing more lines.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use linklet_core::log::{Id, Log, Operation, Outcome, Rotation};

/// Where the agent's request lines go.
///
/// `Arc<Mutex<..>>` rather than either half on its own: there is a thread per connection,
/// so the number counter has to be shared, and the file handle cannot be written by two
/// threads at once or two lines interleave into one.
#[derive(Clone)]
pub struct RequestLog {
    /// `None` when the agent was told not to log.
    inner: Option<Arc<Mutex<Sink>>>,
}

/// The counter, the file and the bound, behind one lock so that a line is numbered,
/// rolled over if it has to be, and written as one thing.
struct Sink {
    /// The numbering, which is the part `linklet-core` decides.
    log: Log,
    /// The open file, or `None` while a rotation is in progress or after one failed.
    ///
    /// `Option` rather than a `File` because **Windows will not rename a file that is
    /// open**, and rolling over means renaming the file this handle refers to. Closing it
    /// is therefore part of the rotation, and the type says so instead of leaving a
    /// `drop` somewhere a reader has to notice.
    file: Option<File>,
    /// Where the lines go, kept so that a rotation can put a new file in the same place.
    path: PathBuf,
    /// When to roll over, and how many older files to keep.
    rotation: Rotation,
    /// How much has been written to the current file, so the size check is arithmetic
    /// rather than a `stat` before every line.
    written: u64,
    /// Requests taken and not yet answered. A rotation waits for this to be zero.
    in_flight: usize,
}

impl RequestLog {
    /// A log that writes to nothing.
    ///
    /// For an agent started with `--no-log`, and every call below is then a no-op rather
    /// than a failure.
    pub fn none() -> Self {
        Self { inner: None }
    }

    /// Opens `path` and appends to it, creating the file and its directory if they are
    /// not there, and rolling over when the file has reached the rotation's bound.
    ///
    /// **Appends rather than truncates**, so restarting an agent does not destroy the
    /// record of what it was asked before it died -- which is the one incident the log
    /// exists for.
    ///
    /// **The directory is created rather than required.** The default location is a
    /// `logs` directory beside the executable, which is exactly the thing that will not
    /// exist on a fresh deployment, and an agent that refused to start because its own
    /// housekeeping directory was missing would be trading a working machine for a
    /// convenience.
    ///
    /// # Errors
    ///
    /// The reason it could not be opened, as a sentence for the operator. What the agent
    /// does with that is the caller's decision and differs by how the path arrived: an
    /// explicit `--log` that cannot be opened is a refusal to start, and the default
    /// location falling back is a warning. See `main`.
    pub fn open(path: &Path, rotation: Rotation) -> Result<Self, String> {
        if path.is_dir() {
            return Err(format!("{} is a directory", path.display()));
        }

        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }

        let file = open_append(path)?;
        let written = file.metadata().map(|meta| meta.len()).unwrap_or(0);

        Ok(Self {
            inner: Some(Arc::new(Mutex::new(Sink {
                log: Log::new(),
                file: Some(file),
                path: path.to_path_buf(),
                rotation,
                written,
                in_flight: 0,
            }))),
        })
    }

    /// Records that a request has been taken, and returns the number it was given.
    ///
    /// The returned number is what [`RequestLog::answered`] needs; `None` means this
    /// request is not being recorded -- because the agent is not logging, or because the
    /// request is not worth recording. See [`Operation::worth_recording`], which is what
    /// keeps a liveness check from turning the log into a heartbeat.
    ///
    /// **Written before the request is handled**, which is the whole point: a request
    /// that never finishes leaves this line and no other, and that pair -- a `->` with no
    /// `<-` -- is what names the request that wedged or killed the agent.
    pub fn taken(&self, operation: Operation) -> Option<Id> {
        // The policy first, and before the lock: a liveness check is not recorded, is not
        // numbered, and does not count as work in flight. See
        // [`Operation::worth_recording`] for the measurement behind it.
        if !operation.worth_recording() {
            return None;
        }

        let inner = self.inner.as_ref()?;
        let Ok(mut sink) = inner.lock() else {
            // A poisoned lock means another thread panicked while holding it. Recording
            // nothing is the right answer for a log: the alternative is a second panic
            // in a thread that was serving a caller.
            return None;
        };

        sink.rotate_if_due();

        let (id, line) = sink.log.taken(operation);
        sink.append(&line.render());
        sink.in_flight += 1;
        Some(id)
    }

    /// Records that a request has been answered.
    ///
    /// `id` is what [`RequestLog::taken`] returned, and it is what ties the two lines
    /// together. A `None` id means this request was not recorded.
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
        sink.append(&line.render());
        sink.in_flight = sink.in_flight.saturating_sub(1);
    }
}

impl Sink {
    /// Rolls the file over if it has reached the bound and no request is in flight.
    fn rotate_if_due(&mut self) {
        // In flight means a `->` line is in this file and its `<-` line has not been
        // written yet. Rolling over now would put the two in different files and report a
        // request that finished as one that never did -- see the module comment.
        if self.in_flight > 0 {
            return;
        }
        if !self.rotation.should_rotate(self.written) {
            return;
        }

        // Closed first: Windows will not rename a file that is open, and the rename is
        // what makes this a rotation rather than a truncation. The `Option` exists so
        // that closing is a statement rather than a `drop` a reader has to notice.
        self.file = None;

        let keep = self.rotation.keep();
        // Oldest first, so that every file is moved before the name it is about to take
        // is overwritten. The oldest is removed rather than moved: there is nowhere for
        // it to go, and `keep` is the promise about how many are retained.
        let _ = std::fs::remove_file(Rotation::rolled_name(&self.path, keep));
        for index in (1..keep).rev() {
            let from = Rotation::rolled_name(&self.path, index);
            if from.exists() {
                let _ = std::fs::rename(&from, Rotation::rolled_name(&self.path, index + 1));
            }
        }
        let _ = std::fs::rename(&self.path, Rotation::rolled_name(&self.path, 1));

        // A reopen that failed leaves `file` as `None`, and `append` retries on the next
        // line rather than losing the rest of the run.
        self.file = open_append(&self.path).ok();
        self.written = self
            .file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map(|meta| meta.len())
            .unwrap_or(0);
    }

    /// Writes one line, terminated, and ignores a failure.
    ///
    /// The ignoring is stated rather than silent: see the module comment. It is the one
    /// place in the agent where an error is dropped on purpose, and the reason is that
    /// there is nothing a caller could do about it and something they would wrongly do --
    /// a command that ran would be reported as a failure if this were propagated.
    fn append(&mut self, text: &str) {
        // Reopened here and not only inside a rotation, so that a rotation whose reopen
        // failed heals on the next line instead of losing the rest of the run.
        if self.file.is_none() {
            self.file = open_append(&self.path).ok();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if writeln!(file, "{text}").is_err() {
            // A full disk, or a file deleted underneath us. The handle is dropped so the
            // next line tries again, which is the same "retry, and never refuse a command
            // over a log" rule as everywhere else here.
            self.file = None;
            return;
        }
        // `writeln!` writes the bytes it is given, so the newline is one byte on every
        // platform -- there is no CRLF translation to account for.
        self.written += text.len() as u64 + 1;
    }
}

/// Opens `path` for appending, creating it if it is not there.
fn open_append(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use linklet_core::log::Rotation;

    /// A directory under the system temporary directory that no other test is using.
    fn scratch(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);

        let path = std::env::temp_dir().join(format!(
            "linklet-agent-log-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        path
    }

    /// The lines of a file, or an empty list when it is not there.
    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn a_request_that_does_something_is_two_lines() {
        let directory = scratch("pair");
        let path = directory.join("logs/agent.log");
        let log = RequestLog::open(&path, Rotation::DEFAULT).expect("a log");

        let id = log.taken(Operation::Run).expect("run is recorded");
        log.answered(Some(id), Operation::Run, Outcome::Ok, 12, None);

        // The directory in the middle of the path did not exist and was created.
        assert_eq!(
            lines(&path),
            vec![
                "-> #000001 run".to_string(),
                "<- #000001 run ok 12 ms".to_string()
            ]
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_liveness_check_leaves_no_line_at_all() {
        // The whole of the "junk repeated log" fix, on the agent's side of it: a probe
        // every five seconds used to write two lines every five seconds forever.
        let directory = scratch("quiet");
        let path = directory.join("logs/agent.log");
        let log = RequestLog::open(&path, Rotation::DEFAULT).expect("a log");

        for _ in 0..1000 {
            let id = log.taken(Operation::Identity);
            assert!(id.is_none(), "a liveness check is not recorded");
            log.answered(id, Operation::Identity, Outcome::Ok, 0, None);
        }

        assert_eq!(lines(&path), Vec::<String>::new(), "the log stayed empty");

        // And the counter did not run away either: the next real request is number one.
        let id = log.taken(Operation::Ls).expect("ls is recorded");
        log.answered(Some(id), Operation::Ls, Outcome::Ok, 1, None);
        assert_eq!(lines(&path)[0], "-> #000001 ls");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_file_rolls_over_at_the_bound_and_keeps_what_it_was_told_to() {
        let directory = scratch("rotate");
        let path = directory.join("logs/agent.log");
        // Room for roughly one pair per file, and two older files kept.
        let log = RequestLog::open(&path, Rotation::new(60, 2)).expect("a log");

        // Enough requests to roll over several times.
        for _ in 0..12 {
            let id = log.taken(Operation::Run).expect("run is recorded");
            log.answered(Some(id), Operation::Run, Outcome::Ok, 1, None);
        }

        assert!(path.is_file(), "the live file exists");
        assert!(Rotation::rolled_name(&path, 1).is_file(), "one older file");
        assert!(Rotation::rolled_name(&path, 2).is_file(), "two older files");
        assert!(
            !Rotation::rolled_name(&path, 3).exists(),
            "a third older file was kept, and the policy said two"
        );

        // The live file is small, because it is what was rolled over into -- not the
        // whole history, which is the point of a bound.
        let live = std::fs::metadata(&path).expect("the live file").len();
        assert!(
            live < 200,
            "the live file is {live} bytes, which is not bounded"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_newest_rolled_file_holds_the_newer_lines() {
        // The order matters to a reader: `.1` is the most recent roll-over and the highest
        // number is the oldest. Inverting it would produce a directory that looks correct
        // and holds the files backwards.
        let directory = scratch("order");
        let path = directory.join("logs/agent.log");
        let log = RequestLog::open(&path, Rotation::new(50, 3)).expect("a log");

        // The duration is used as a sequence number, so every line says which request it
        // was without the test having to count files.
        for number in 1..=20 {
            let id = log.taken(Operation::Run).expect("run is recorded");
            log.answered(Some(id), Operation::Run, Outcome::Ok, number, None);
        }

        let last_millis = |found: &[String]| -> u64 {
            found
                .last()
                .and_then(|line| line.split_whitespace().rev().nth(1))
                .and_then(|ms| ms.parse().ok())
                .unwrap_or(0)
        };

        let live = last_millis(&lines(&path));
        let one = last_millis(&lines(&Rotation::rolled_name(&path, 1)));
        let two = last_millis(&lines(&Rotation::rolled_name(&path, 2)));

        assert!(live > one, "the live file should hold the newest lines");
        assert!(one > two, ".1 should hold newer lines than .2");

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_rotation_never_splits_a_request_from_its_answer() {
        // The one property rotation must not break. A request is taken, the file is past
        // its bound, and nothing may roll over until that request has been answered --
        // because the two halves in different files would report a finished request as
        // one that never finished, which is the only thing the pair is read for.
        let directory = scratch("pair-safe");
        let path = directory.join("logs/agent.log");
        // A bound of one byte: every file is over it the moment it has a line in it.
        let log = RequestLog::open(&path, Rotation::new(1, 3)).expect("a log");

        let id = log.taken(Operation::Push).expect("push is recorded");
        assert!(
            !Rotation::rolled_name(&path, 1).exists(),
            "a rotation happened while a request was in flight"
        );

        log.answered(Some(id), Operation::Push, Outcome::Ok, 5, None);
        let pair = lines(&path);
        assert_eq!(pair.len(), 2, "the pair is in one file: {pair:#?}");
        assert!(pair[0].starts_with("-> "), "{pair:#?}");
        assert!(pair[1].starts_with("<- "), "{pair:#?}");

        // Only now, with nothing in flight, may the next request roll the file over.
        let _ = log.taken(Operation::Run).expect("run is recorded");
        assert!(
            Rotation::rolled_name(&path, 1).is_file(),
            "the file should have rolled over once nothing was in flight"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}
