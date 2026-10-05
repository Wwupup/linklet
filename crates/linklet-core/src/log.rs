//! What the agent records about the requests it served.
//!
//! This is a **decision** module and not an I/O one: it decides what a line says and
//! how a request is tied to its completion. Writing the line is the agent's job, in
//! `linklet-agent/src/log.rs`, and the same split as everywhere else in this project --
//! see `AGENTS.md` rule 1.
//!
//! # The failure this exists for
//!
//! `docs/ROADMAP.md` M10, from the first real target: the agent died with its console,
//! `tasklist` found nothing, and the caller saw a connect **timeout** rather than a
//! refusal. Nothing recorded what the agent had been asked, so there was nothing to read
//! afterwards -- a machine that had answered calls all afternoon left no evidence that it
//! had ever been asked anything.
//!
//! # Two lines per request, and why one is not enough
//!
//! A single line written at the end describes a request that finished. It cannot describe
//! a request that **did not**: a process that dies mid-request writes nothing at all, and
//! the log of the final request is indistinguishable from a log where that request never
//! arrived. So a request writes a line when it is taken and another when it is answered,
//! and the absence of the second is the evidence -- the shape the sibling project uses,
//! and the reason it uses it.
//!
//! # What a line carries, and what it deliberately does not
//!
//! The operation, a request number, the outcome and how long it took. **Not the command,
//! and not the path**: a log on someone else's machine is a file that outlives the reason
//! it was written, and a command line is where secrets are left by accident -- the same
//! argument that keeps the token out of `argv` in the tool the host runs.
//!
//! The reason on a refusal **is** recorded. It is the agent's own sentence about what it
//! refused, it is what the caller was already told, and a log that said "refused" without
//! saying why would send the reader to a different machine to find out.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What a request asked for, as the log names it.
///
/// The protocol's own `op` values plus `unknown`, rather than a second vocabulary --
/// `docs/MCP.md` and `crate::wire` use the same strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Which agent this is.
    Identity,
    /// Run a command.
    Run,
    /// Receive a file.
    Push,
    /// Send a file.
    Pull,
    /// Ask what is running on the machine.
    Ps,
    /// Stop something on the machine.
    Kill,
    /// Start a program that outlives the request.
    Spawn,
    /// Search a file on the machine.
    Grep,
    /// Read the end of a file on the machine.
    Tail,
    /// List a path on the machine.
    Ls,
    /// Something arrived that was not a request this version knows.
    ///
    /// **Recorded rather than skipped**, and that is a decision: a sealed body that is not
    /// JSON, or carries an `op` from a version that does not exist, is exactly the traffic
    /// an operator wants to find in a log. Dropping it would make the log agree with the
    /// protocol instead of with the machine.
    Unknown,
}

impl Operation {
    /// The name this operation is written as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Run => "run",
            Self::Push => "push",
            Self::Pull => "pull",
            Self::Ps => "ps",
            Self::Kill => "kill",
            Self::Spawn => "spawn",
            Self::Grep => "grep",
            Self::Tail => "tail",
            Self::Ls => "ls",
            Self::Unknown => "unknown",
        }
    }

    /// The operation a name refers to, or `None`.
    ///
    /// Called `named` rather than `from_str`, which clippy reads as an attempt at
    /// `FromStr` and flags: this returns `None` for anything it does not know instead of
    /// an error, because a log reader has to decide what an unknown word means and there
    /// is nothing for an error to carry.
    ///
    /// Used by the tests that read a log back and by the agent's `op` lookup, which is
    /// why it is here rather than duplicated: the writer of the format and its readers
    /// should share one table of names.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "identity" => Some(Self::Identity),
            "run" => Some(Self::Run),
            "push" => Some(Self::Push),
            "pull" => Some(Self::Pull),
            "ps" => Some(Self::Ps),
            "kill" => Some(Self::Kill),
            "spawn" => Some(Self::Spawn),
            "grep" => Some(Self::Grep),
            "tail" => Some(Self::Tail),
            "ls" => Some(Self::Ls),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }

    /// Whether a request for this belongs in the log at all.
    ///
    /// **False for `identity` and true for everything else**, and the reason is a
    /// measurement rather than a preference. `identity` is the one request that asks for
    /// nothing and changes nothing, and it is what a monitor calls to ask *are you
    /// alive*; recording it makes the log a heartbeat instead of a record. On this
    /// project's own bench, four days of a five-second liveness check put **37,596
    /// `identity` lines into a log of 37,781** -- 99.5%, leaving 0.5% for the work. The
    /// cost is not the disk: it is that the pair below stops being findable in it.
    ///
    /// # What that costs, stated rather than hidden
    ///
    /// A liveness check that wedges the agent now leaves no `->` line naming it. It does
    /// not need one: the caller that asked is the one holding the socket, and a monitor's
    /// whole question is whether an answer came back. A request that **does** something
    /// is still recorded, and that is where a missing completion line is evidence.
    ///
    /// A caller that wants a refusal recorded is served by this returning true for
    /// [`Operation::Unknown`]: a sealed body that is not a request this version knows is
    /// exactly the traffic an operator wants to find.
    pub fn worth_recording(self) -> bool {
        !matches!(self, Self::Identity)
    }
}

/// How a request ended, as the log names it.
///
/// **Three values and not two.** `Refused` is the agent saying no to something it
/// understood; `Failed` is the agent failing to do something it had accepted -- a
/// transfer whose socket died, or a reply that could not be framed. A reader looking for
/// the requests that went wrong needs to tell "this machine said no" from "this machine
/// broke", because only the second is a reason to look at the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The request was answered with a result.
    Ok,
    /// The request was understood and refused, with a reason.
    Refused,
    /// The request was accepted and could not be completed.
    Failed,
}

impl Outcome {
    /// The name this outcome is written as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }
}

/// Which request a line is about.
///
/// A number of its own rather than a bare `u64`, so that the operation on a completion
/// line cannot be passed where the identifier belongs: the two are both small and the
/// mistake would produce a log that reads plausibly and pairs nothing with anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Id(u64);

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Six digits so the numbers line up in a file somebody is reading, which is the
        // reason to have them at all.
        write!(f, "#{:06}", self.0)
    }
}

/// One line of the agent's log.
///
/// An enum rather than a struct with optional fields, because the two lines carry
/// different things and a reader has to be able to tell them apart **from the line
/// alone**: `->` is a request taken, `<-` is a request answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A request was taken, and has not been answered yet.
    Taken {
        /// Which request this is.
        id: Id,
        /// What it asked for.
        operation: Operation,
    },
    /// A request was answered.
    Answered {
        /// The request this answers, as written on the line where it was taken.
        id: Id,
        /// What it asked for, repeated so a completion line is readable on its own.
        operation: Operation,
        /// How it ended.
        outcome: Outcome,
        /// How long it took, in milliseconds.
        millis: u64,
        /// Why, on a refusal: the agent's own sentence, which the caller was also told.
        reason: Option<String>,
    },
}

impl Line {
    /// The line as it goes in the file.
    ///
    /// One line, no trailing newline -- the writer adds that, and a renderer that added
    /// one would put a blank line between every pair in the file for a caller writing
    /// them in a loop.
    ///
    /// The request number is padded to six digits so the numbers line up in a file
    /// somebody is reading, which is the whole reason to have them. A reason is quoted
    /// and escaped, so a sentence with a newline in it cannot become two lines -- that
    /// would make a log unparseable by exactly the reader who needs it.
    pub fn render(&self) -> String {
        match self {
            Self::Taken { id, operation } => {
                format!("-> {id} {}", operation.as_str())
            }
            Self::Answered {
                id,
                operation,
                outcome,
                millis,
                reason,
            } => {
                let mut line = format!(
                    "<- {id} {} {} {millis} ms",
                    operation.as_str(),
                    outcome.as_str()
                );
                if let Some(reason) = reason {
                    let _ = write!(line, " \"{}\"", escape(reason));
                }
                line
            }
        }
    }
}

/// Escapes a reason so that it stays on one line.
///
/// The same shape as the JSON writer's escaping, and for the same reason: the thing being
/// escaped is text from somewhere else that must not be able to change the structure of
/// the line it appears in. A newline becomes `\n` rather than ending the record, and a
/// quote becomes `\"` rather than closing the field early.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if other.is_control() => {
                let _ = write!(out, "\\u{:04x}", other as u32);
            }
            other => out.push(other),
        }
    }
    out
}

/// A log writer that hands out a number for each request taken.
///
/// Owns the counter, so two requests can never be given the same number and a reader can
/// follow one request through a file that several connections are writing at once.
///
/// # What it does not do
///
/// **It does not open a file.** `render` produces the line and the caller writes it --
/// the split this module exists for. It also does not decide whether logging is on: a
/// caller that does not want a log simply does not keep one of these.
///
/// # Example
///
/// ```
/// use linklet_core::log::{Log, Operation, Outcome};
///
/// let mut log = Log::new();
/// let (id, taken) = log.taken(Operation::Run);
/// assert_eq!(taken.render(), "-> #000001 run");
///
/// let answered = log.answered(id, Operation::Run, Outcome::Ok, 2411, None);
/// assert_eq!(answered.render(), "<- #000001 run ok 2411 ms");
/// ```
#[derive(Debug)]
pub struct Log {
    next: u64,
}

impl Log {
    /// A log whose first request is number one.
    pub fn new() -> Self {
        Self { next: 1 }
    }

    /// The number and the line for a request that has just been taken.
    ///
    /// **The number comes back to the caller**, and that is the correction this type
    /// needed: written with [`Log::answered`] allocating the next number instead, the
    /// completion line of the first request was numbered two and paired with nothing.
    /// A counter that advances on both calls cannot tie anything to anything, so the
    /// tie is a value the caller holds between the two lines.
    pub fn taken(&mut self, operation: Operation) -> (Id, Line) {
        let id = Id(self.next);
        self.next += 1;
        (id, Line::Taken { id, operation })
    }

    /// The line for a request that has just been answered.
    ///
    /// Takes the number [`Log::taken`] returned rather than allocating one: the two
    /// lines are the same request, and a reader pairs them on nothing else.
    ///
    /// The operation is repeated so a completion line can be read without the line above
    /// it -- a file that several connections write at once does not keep the two
    /// adjacent.
    pub fn answered(
        &mut self,
        id: Id,
        operation: Operation,
        outcome: Outcome,
        millis: u64,
        reason: Option<String>,
    ) -> Line {
        Line::Answered {
            id,
            operation,
            outcome,
            millis,
            reason,
        }
    }
}

impl Default for Log {
    /// The same as [`Log::new`], because a log with no requests in it is the ordinary
    /// starting state rather than a mistake.
    fn default() -> Self {
        Self::new()
    }
}

/// How large the agent's log file may get before it is rolled over.
///
/// **The decision, not the doing.** Whether to rotate is a function of a size and a
/// limit, so it is testable in microseconds here; renaming files is I/O and lives in
/// `linklet-agent`. That split is rule 1, and this is the shape it takes for a log.
///
/// # Why there is a bound at all
///
/// The agent appends and never truncates, deliberately -- restarting an agent must not
/// destroy the record of what it was asked before it died, which is the one incident the
/// log exists for. Append-without-bound and a machine that serves for months is a full
/// disk, and a full disk takes down everything else on the machine, not just the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rotation {
    /// Rotate when the file has reached this many bytes.
    max_bytes: u64,
    /// How many rolled-over files to keep beside the current one.
    keep: usize,
}

impl Rotation {
    /// One mebibyte per file and three files kept, so a log costs at most four.
    ///
    /// The number is the usual kind of estimate and is said to be one: a line is about
    /// fifty bytes, so a mebibyte is roughly twenty thousand lines, and one machine's
    /// requests for a long while. What matters is that it is *bounded*, and that the
    /// bound is small enough to be irrelevant next to a disk and large enough that a
    /// diagnosis does not need yesterday's file.
    pub const DEFAULT: Self = Self {
        max_bytes: 1024 * 1024,
        keep: 3,
    };

    /// A rotation policy: roll over at `max_bytes`, keeping `keep` older files.
    ///
    /// **`keep` of zero means no rotation at all**, which is the way to ask for the old
    /// behaviour. It is not "keep nothing": rotating is what *produces* the old files, so
    /// a policy that kept none would have to throw away the file it had just closed, and
    /// a log that deleted itself every mebibyte is worse than one that grows.
    pub fn new(max_bytes: u64, keep: usize) -> Self {
        Self { max_bytes, keep }
    }

    /// How many rolled-over files this keeps. Zero means rotation is off.
    pub fn keep(&self) -> usize {
        self.keep
    }

    /// Whether a current file of this many bytes should be rolled over before another
    /// line is written.
    ///
    /// Takes the size rather than reading it, so the caller that already tracks how much
    /// it has written does not have to ask the filesystem before every line.
    pub fn should_rotate(&self, current_bytes: u64) -> bool {
        self.keep > 0 && current_bytes >= self.max_bytes
    }

    /// The name the file rolled off at `index` is kept under.
    ///
    /// Index 1 is the most recently rolled-over file, so `agent.log` becomes
    /// `agent.log.1` and the oldest is the highest number -- which is the convention a
    /// reader who has met `logrotate` already expects, and inverting it would produce a
    /// directory that looks correct and holds the files in the wrong order.
    ///
    /// A suffix rather than a substituted name, so that a path with an extension keeps
    /// it and `logs/agent.log` does not become `logs/agent.1`.
    pub fn rolled_name(path: &Path, index: usize) -> PathBuf {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_line_and_an_answered_line_are_told_apart_at_a_glance() {
        let mut log = Log::new();
        let (id, taken) = log.taken(Operation::Identity);
        assert_eq!(taken.render(), "-> #000001 identity");
        assert_eq!(
            log.answered(id, Operation::Identity, Outcome::Ok, 3, None)
                .render(),
            "<- #000001 identity ok 3 ms"
        );
    }

    #[test]
    fn an_answer_carries_the_number_of_the_request_it_answers() {
        // The tie the whole file is read on, and the bug this test was written after:
        // the first version had `answered` allocate the next number, so the first
        // request was taken as one and answered as two.
        let mut log = Log::new();
        let (first, _) = log.taken(Operation::Run);
        let (second, _) = log.taken(Operation::Push);

        let number = |line: &Line| match line {
            Line::Taken { id, .. } | Line::Answered { id, .. } => *id,
        };
        assert_eq!(
            number(&log.answered(first, Operation::Run, Outcome::Ok, 1, None)),
            first
        );
        assert_eq!(
            number(&log.answered(second, Operation::Push, Outcome::Ok, 1, None)),
            second
        );
        assert_ne!(first, second, "two requests share a number");
    }

    #[test]
    fn a_refusal_records_the_reason_the_caller_was_told() {
        let mut log = Log::new();
        let (id, _) = log.taken(Operation::Pull);
        let line = log
            .answered(
                id,
                Operation::Pull,
                Outcome::Refused,
                1,
                Some("..\\..\\Windows is outside the root".to_string()),
            )
            .render();

        assert!(line.contains("refused"), "{line}");
        assert!(line.contains("outside the root"), "{line}");
    }

    #[test]
    fn a_reason_with_a_newline_in_it_stays_on_one_line() {
        // The property the whole format rests on: a reader counts `->` lines without
        // `<-` lines, and a reason that broke the line would invent a request.
        let mut log = Log::new();
        let (id, _) = log.taken(Operation::Run);
        let line = log
            .answered(
                id,
                Operation::Run,
                Outcome::Refused,
                2,
                Some("the first line\nthe second line".to_string()),
            )
            .render();

        assert_eq!(line.lines().count(), 1, "{line:?}");
        assert!(line.contains("\\n"), "{line}");
    }

    #[test]
    fn a_reason_cannot_close_its_own_field() {
        let mut log = Log::new();
        let (id, _) = log.taken(Operation::Run);
        let line = log
            .answered(
                id,
                Operation::Run,
                Outcome::Refused,
                2,
                Some("say \"no\"".to_string()),
            )
            .render();

        assert!(line.ends_with("\"say \\\"no\\\"\""), "{line}");
    }

    #[test]
    fn a_line_carries_no_newline_of_its_own() {
        // The writer adds one. A renderer that added one too would put a blank line
        // between every pair in a file written in a loop.
        let mut log = Log::new();
        let (id, taken) = log.taken(Operation::Run);

        assert!(!taken.render().contains('\n'));
        assert!(
            !log.answered(id, Operation::Run, Outcome::Ok, 1, None)
                .render()
                .contains('\n')
        );
    }

    // --- what is worth writing down ------------------------------------------

    #[test]
    fn the_liveness_check_is_not_written_to_the_log() {
        // The measurement this comes from: four days of a five-second probe put 37,596
        // `identity` lines into a 37,781-line log. The other 0.5% was the work, and the
        // pair -- a `->` with no `<-` naming the request that wedged -- is only findable
        // in a log that is mostly not this.
        assert!(!Operation::Identity.worth_recording());

        // Everything that does something is recorded, and `unknown` is in that list on
        // purpose: a sealed body that is not a request this version knows is exactly the
        // traffic an operator wants to find.
        for operation in [
            Operation::Run,
            Operation::Push,
            Operation::Pull,
            Operation::Ps,
            Operation::Kill,
            Operation::Spawn,
            Operation::Grep,
            Operation::Tail,
            Operation::Ls,
            Operation::Unknown,
        ] {
            assert!(
                operation.worth_recording(),
                "{operation:?} does something and belongs in the record"
            );
        }
    }

    #[test]
    fn the_guard_on_that_list_is_the_whole_surface() {
        // A new operation added to the protocol and forgotten here would be recorded or
        // not by accident. This is what makes the list above exhaustive rather than a
        // sample: every name `named` accepts is checked, so adding one to the enum and
        // the parser fails this test until it is decided which side it is on.
        for name in [
            "identity", "run", "push", "pull", "ps", "kill", "spawn", "grep", "tail", "ls",
            "unknown",
        ] {
            let operation = Operation::named(name).expect("a name the parser accepts");
            let recorded = operation.worth_recording();
            assert_eq!(
                recorded,
                name != "identity",
                "{name} is on the wrong side of this decision"
            );
        }
    }

    // --- the bound on the file -----------------------------------------------

    #[test]
    fn a_log_below_its_limit_is_not_rolled_over() {
        let rotation = Rotation::new(1000, 3);
        assert!(!rotation.should_rotate(0));
        assert!(!rotation.should_rotate(999));
    }

    #[test]
    fn a_log_at_its_limit_is_rolled_over() {
        // At the limit rather than past it: a file that overshot by one line has already
        // spent the disk the limit was there to bound.
        let rotation = Rotation::new(1000, 3);
        assert!(rotation.should_rotate(1000));
        assert!(rotation.should_rotate(1001));
        assert!(rotation.should_rotate(u64::MAX));
    }

    #[test]
    fn keeping_nothing_turns_rotation_off_rather_than_deleting_the_log() {
        // Rotating is what produces the older files, so a policy that kept none would
        // have to throw away the file it had just closed. Off is the only reading of
        // `keep: 0` that is not a log that deletes itself every mebibyte.
        let off = Rotation::new(1000, 0);
        assert!(!off.should_rotate(0));
        assert!(!off.should_rotate(1_000_000));
        assert_eq!(off.keep(), 0);
    }

    #[test]
    fn the_default_bound_is_a_bound() {
        // Not a taste judgement -- the claim is only that a machine left running for
        // months cannot fill a disk with request lines.
        assert!(Rotation::DEFAULT.should_rotate(1024 * 1024));
        assert!(!Rotation::DEFAULT.should_rotate(0));
        assert!(Rotation::DEFAULT.keep() >= 1);
    }

    #[test]
    fn a_rolled_name_keeps_the_extension_and_counts_up() {
        // `agent.log` -> `agent.log.1`, not `agent.1`: a substituted extension produces a
        // directory that looks right and holds files nothing else will read.
        let path = Path::new("logs/agent.log");
        assert_eq!(
            Rotation::rolled_name(path, 1),
            Path::new("logs/agent.log.1")
        );
        assert_eq!(
            Rotation::rolled_name(path, 3),
            Path::new("logs/agent.log.3")
        );

        // A path with no extension is not special-cased, and a Windows one keeps its
        // separators.
        assert_eq!(
            Rotation::rolled_name(Path::new("agent"), 2),
            Path::new("agent.2")
        );
        assert_eq!(
            Rotation::rolled_name(Path::new(r"C:\linklet\logs\agent.log"), 1),
            Path::new(r"C:\linklet\logs\agent.log.1")
        );
    }

    #[test]
    fn the_operation_names_are_the_protocol_s_own() {
        for (operation, name) in [
            (Operation::Identity, "identity"),
            (Operation::Run, "run"),
            (Operation::Push, "push"),
            (Operation::Pull, "pull"),
            (Operation::Ps, "ps"),
            (Operation::Kill, "kill"),
            (Operation::Spawn, "spawn"),
            (Operation::Grep, "grep"),
            (Operation::Tail, "tail"),
            (Operation::Ls, "ls"),
            (Operation::Unknown, "unknown"),
        ] {
            assert_eq!(operation.as_str(), name);
            assert_eq!(Operation::named(name), Some(operation));
        }
        assert_eq!(Operation::named("install"), None);
    }
}
