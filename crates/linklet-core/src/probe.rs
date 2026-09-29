//! Asking whether a machine answers, without being able to open a socket.
//!
//! This module is where the shape of the whole project shows up. Deciding
//! whether a target is reachable is a rule, and rules live in the core -- but
//! reachability can only be observed by connecting, which is I/O, which the
//! core cannot do.
//!
//! The resolution is that **the core states what it needs and the adapter
//! supplies it**. [`Probe`] is declared here, in the pure crate; the
//! implementation that opens a TCP connection lives in `linklet-adapters`. The
//! dependency arrow therefore points `adapters -> core`, which is the only
//! direction that keeps the core testable without a machine.
//!
//! The tempting alternative -- declare the trait in the adapter and have the
//! core depend on it -- reverses the arrow and drags all of `adapters` into
//! every test of a rule. It is worth being precise about why that is worse than
//! it sounds: it is not a matter of taste, it is that the core could then only
//! be tested by linking the thing that does I/O, and a test that needs a
//! network stops being run.

use crate::Target;
use std::time::Duration;

/// What one look at one target observed.
///
/// Deliberately not a single `bool`. "Nothing is listening" and "everything is
/// being dropped" are the same answer to a `bool` and completely different
/// answers to the person trying to work out why a build will not deploy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The connection was accepted: something is listening.
    Answered,
    /// The machine answered and refused. It is up and reachable -- which is
    /// useful news -- and nothing is listening on that port.
    Refused,
    /// Nothing came back before the budget ran out. The machine may be off, the
    /// port may be filtered, or the network may be slow; this probe cannot tell
    /// those apart and does not pretend to.
    TimedOut,
    /// The attempt ended without the machine ever saying anything: the budget
    /// ran out first.
    ///
    /// Kept separate from [`ProbeOutcome::TimedOut`] because they are different
    /// claims. A timeout is something the OS decided; no answer is the absence
    /// of a decision, which is all a probe can honestly report when it stopped
    /// listening before the machine stopped talking. The measured reason this
    /// distinction exists is in `linklet-adapters/src/tcp.rs`: on Windows a
    /// refused connection takes about two seconds to surface, so a short budget
    /// produces a timeout the runtime invented rather than one the machine
    /// reported.
    NoAnswer,
    /// The attempt failed for a reason that is neither of the above, described
    /// in the adapter's own words. This is where name-resolution failures land.
    Error(String),
}

/// Looks at one target, within a time budget, and reports what it saw.
///
/// # What an implementation may and may not do
///
/// - It **should not** wait longer than the budget. The budget is a promise to
///   the caller, and a probe that routinely overruns it makes every deadline
///   above it a suggestion.
///
///   The one thing that outranks the promise is not being wrong. An
///   implementation that cannot reach a correct verdict inside the budget may
///   take longer, and must say so in its documentation: taking longer and being
///   right beats answering quickly with a verdict the machine never reached.
///   `linklet-adapters` does exactly this, with a measured justification.
/// - It **must not** block forever on anything, including name resolution. A
///   hostname that does not resolve can hang for as long as the resolver feels
///   like it; the budget covers that too.
/// - It **may** return [`ProbeOutcome::NoAnswer`] or [`ProbeOutcome::Error`]
///   rather than panicking. There is no panic that carries useful information
///   about a network, and a panic in a library is a bug report from a user.
///
/// The budget is passed in rather than measured by the implementation, which is
/// what keeps the *policy* -- what the budget is, and what a timeout means --
/// in the core instead of scattered through the code that talks to the OS.
pub trait Probe {
    /// Looks at `target`, spending no more than `budget` on it.
    fn probe(&self, target: &Target, budget: Duration) -> ProbeOutcome;
}

/// How many targets one run will look at.
///
/// A refusal rather than a queue: a caller that asks about a thousand machines
/// has made a mistake, and an error naming the limit is more useful than a
/// command that appears to work for four minutes.
pub const MAX_TARGETS: usize = 256;

/// The ceiling a single probe may be given, in seconds.
///
/// Ten seconds is long enough for a slow LAN and short enough that a caller
/// notices. It is a cap rather than a default: a caller may ask for less.
pub const MAX_BUDGET_SECONDS: u64 = 10;

/// The budget a caller should use when it has no reason to want another.
///
/// Exported so that the CLI and the tests agree on one number instead of each
/// writing their own. It is comfortably above the measured floor an adapter may
/// impose on itself (five seconds in `linklet-adapters`), because a default
/// that sits on a threshold is a default that behaves differently on a busy
/// machine.
pub const DEFAULT_BUDGET_SECONDS: u64 = 5;

/// What the caller is told about one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Something accepted the connection.
    Alive,
    /// The machine answered and refused: up, but nothing on that port.
    Refused,
    /// No answer within the budget.
    Unreachable,
    /// The probe could not reach a verdict, for the reason given.
    Unknown(String),
}

/// One target and what was observed about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The target that was looked at, as the caller wrote it.
    pub target: Target,
    /// What the probe observed.
    pub status: Status,
    /// The observation in the adapter's own words.
    ///
    /// Always filled in, including for a success -- "connected" is an
    /// observation too, and a caller that has to special-case the happy path to
    /// find its reason is a caller that will get it wrong.
    pub reason: String,
}

impl Report {
    /// Whether this target is alive.
    pub fn is_alive(&self) -> bool {
        matches!(self.status, Status::Alive)
    }
}

/// How many of each status a run produced. Derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    /// Targets that answered.
    pub alive: usize,
    /// Targets that refused.
    pub refused: usize,
    /// Targets that did not answer.
    pub unreachable: usize,
    /// Targets the probe could not classify.
    pub unknown: usize,
}

impl Summary {
    /// Counts a set of reports.
    pub fn of(reports: &[Report]) -> Self {
        let mut summary = Self::default();
        for report in reports {
            match report.status {
                Status::Alive => summary.alive += 1,
                Status::Refused => summary.refused += 1,
                Status::Unreachable => summary.unreachable += 1,
                Status::Unknown(_) => summary.unknown += 1,
            }
        }
        summary
    }

    /// How many targets were looked at.
    pub fn total(&self) -> usize {
        self.alive + self.refused + self.unreachable + self.unknown
    }
}

/// Why a run was refused before it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// No targets were given.
    NoTargets,
    /// More targets than [`MAX_TARGETS`].
    TooManyTargets {
        /// How many the caller asked about.
        asked: usize,
        /// The limit.
        limit: usize,
    },
    /// The run was allowed zero targets, which can only refuse everything.
    ZeroLimit,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTargets => write!(f, "no targets to check"),
            Self::TooManyTargets { asked, limit } => {
                write!(
                    f,
                    "{asked} targets asked for, at most {limit} are checked in one run"
                )
            }
            Self::ZeroLimit => write!(f, "a limit of zero targets would refuse every run"),
        }
    }
}

impl std::error::Error for CheckError {}

/// Looks at every target, in the order given, and reports what was observed.
///
/// The order of the result matches the order of the input, including when the
/// statuses differ: a caller that has to re-sort the answer to match its
/// question has been handed a worse interface than one that gets them in order.
///
/// The budget applies to **each** target, not to the run. "Ten seconds" is a
/// far more useful promise than "ten seconds in total, divided among however
/// many machines you named", which is a number neither side can predict.
///
/// # Errors
///
/// Refuses the whole run -- before probing anything -- when there is nothing to
/// check, when the limit is zero, or when there are more targets than the
/// limit. Nothing is probed in those cases: a partial run that then fails is
/// indistinguishable, to the caller, from a partial run that succeeded.
pub fn check_targets(
    probe: &dyn Probe,
    targets: &[Target],
    budget: Duration,
    max_targets: usize,
) -> Result<Vec<Report>, CheckError> {
    if max_targets == 0 {
        return Err(CheckError::ZeroLimit);
    }
    if targets.is_empty() {
        return Err(CheckError::NoTargets);
    }
    if targets.len() > max_targets {
        return Err(CheckError::TooManyTargets {
            asked: targets.len(),
            limit: max_targets,
        });
    }

    let mut reports = Vec::with_capacity(targets.len());

    for target in targets {
        let (status, reason) = match probe.probe(target, budget) {
            ProbeOutcome::Answered => (Status::Alive, "connected".to_string()),
            ProbeOutcome::Refused => (
                Status::Refused,
                "the machine refused the connection".to_string(),
            ),
            // Both mean "we could not talk to it", which is the same news to
            // the caller and therefore the same status. They stay separate
            // outcomes because the adapter needs the difference -- see the
            // variant's documentation -- not because the caller does.
            ProbeOutcome::TimedOut => (
                Status::Unreachable,
                "the machine did not answer in time".to_string(),
            ),
            ProbeOutcome::NoAnswer => (
                Status::Unreachable,
                format!("no answer within {} s", budget.as_secs()),
            ),
            ProbeOutcome::Error(message) => (Status::Unknown(message.clone()), message),
        };

        reports.push(Report {
            target: target.clone(),
            status,
            reason,
        });
    }

    Ok(reports)
}
