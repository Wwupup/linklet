//! What the tool says and what it returns, as a contract rather than a habit.
//!
//! Everything here is a decision about the *interface*, and the interface is
//! the product. An agent reads the output and branches on the exit code; if
//! either one drifts, the agent breaks without any code of its own changing.
//! Keeping both here -- pure, tested, with no printing and no process exiting --
//! means the contract can be pinned by tests instead of living in `main` where
//! nothing checks it.

use crate::{Report, Status};

/// The exit codes, fixed.
///
/// Fixed now, before there is a user, because they become a contract the moment
/// anything branches on them. An agent needs to tell "the check ran and
/// something is wrong" apart from "the check never ran", and those must not
/// share a code -- that difference is the whole value of the tool to an agent.
///
/// `u8` rather than `i32` because a process exit code is a byte and
/// `std::process::ExitCode` implements `From<u8>`: the narrower type means the
/// CLI never has to convert, and a code outside `0..=255` is a compile error
/// rather than a number that wraps into someone else's meaning.
pub struct ExitCode;

impl ExitCode {
    /// Everything asked about is alive.
    pub const SUCCESS: u8 = 0;
    /// The check ran, and at least one target is not alive.
    pub const NOT_ALL_ALIVE: u8 = 1;
    /// The invocation was wrong: bad flag, no command, unparseable target.
    pub const USAGE: u8 = 2;
    /// The run was refused before anything was looked at.
    pub const REFUSED: u8 = 3;
}

/// The code for a completed run.
///
/// Takes the resolved reports rather than the `Result`, so that a caller cannot
/// accidentally ask this function about a run that never happened -- that case
/// is [`ExitCode::REFUSED`] and nothing here can report it.
///
/// An empty slice is [`ExitCode::SUCCESS`], which looks wrong and is deliberate:
/// doing nothing was a complete success at doing nothing. The refusal for an
/// empty target list happens earlier, and letting both cases land here would
/// give one situation two answers.
pub fn exit_code_for(reports: &[Report]) -> u8 {
    if reports.iter().all(Report::is_alive) {
        ExitCode::SUCCESS
    } else {
        ExitCode::NOT_ALL_ALIVE
    }
}

/// One report as one line: `status target reason`.
///
/// The format is fixed and boring on purpose. An agent parses this; a human
/// reading it is a bonus. Three properties are load-bearing:
///
/// - **One report, one line.** No wrapping, so a caller can split lines and pair
///   them up without state.
/// - **The status token never contains a space.** It is the first word, so
///   `split_whitespace().next()` is a complete parser for the field that matters
///   most.
/// - **The reason never introduces a newline.** Otherwise one report becomes two
///   lines and every caller that counts lines is quietly wrong.
///
/// The `live`/`dead`/`unknown` vocabulary is narrower than [`Status`] on purpose.
/// `refused` and `unreachable` are a difference the *user* needs and the tool
/// reports in the reason; a caller deciding whether to carry on needs one bit,
/// and three ways to spell "not alive" is three chances to get it wrong. A
/// deliberate loss of information, in the one place where less is more.
pub fn render(report: &Report) -> String {
    let token = match report.status {
        Status::Alive => "live",
        Status::Refused | Status::Unreachable => "dead",
        Status::Unknown(_) => "unknown",
    };

    // A reason is built from a budget, an adapter's error text, or a fixed
    // phrase. None of those contains a newline today. If one ever does, it is a
    // bug in the string, not in this format -- and a caller's line count is
    // already wrong by the time anyone notices.
    debug_assert!(
        !report.reason.contains(['\n', '\r']),
        "a reason must not introduce a line break: {:?}",
        report.reason
    );

    format!("{token} {} {}", report.target, report.reason)
}
