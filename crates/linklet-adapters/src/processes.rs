//! Asking this machine what is running, and stopping what it is told to.
//!
//! The counterpart of `linklet_core::process`, which decides what a listing *says*. What is
//! here is the program that produces one, and the parser that reads it -- two things that can
//! only be wrong about this operating system, and therefore two things that belong in the
//! crate that is allowed to touch one.
//!
//! # The platform decision is in one place, again
//!
//! `crate::shell` is the module that knows which operating system this is for *running a
//! command*; this is the same idea for *listing processes and stopping them*, and it is split
//! the same way for the same reason. The orchestration below -- read everything, filter, plan,
//! report -- is shared and holds the two invariants this feature has; only three things differ
//! by platform:
//!
//! | what | windows | linux |
//! |---|---|---|
//! | enumerate | `tasklist /FO CSV /NH` | `/proc` |
//! | stop a pid | `taskkill /PID <pid> /T /F` | `kill -9`, the process group when the pid leads one |
//! | a pid's parent | `wmic ... get ParentProcessId` | `/proc/<pid>/stat` |
//!
//! `windows` and `linux` are compiled one at a time, and each is a small module that answers
//! those three questions. Everything that could be *decided wrongly* stays here, where it is
//! written once and tested on whichever platform the tests are running on.
//!
//! # Why the two are not a trait
//!
//! M2 put a `Probe` trait in the core and the TCP implementation in `adapters`, and
//! `docs/ROADMAP.md` M11 expected the same move here. It is the wrong shape for this one: a
//! trait with two implementations that cannot both be present in one binary is machinery for
//! choosing something that is not a choice. Which process list a machine has is a fact about
//! the machine, and `cfg` is how this project says a fact -- see `crate::shell`, where the
//! same argument was made for the same reason.
//!
//! # What it does not give
//!
//! On Windows, `tasklist` has no option for a path or a command line: a process is a name and
//! a pid, so both fields are `None` and [`Filter::unanswerable`] reports them rather than
//! returning an empty list. On Linux both are read from `/proc`, and `None` there means what
//! it says: the agent was not allowed to look. That asymmetry is real and is carried to the
//! caller rather than smoothed over -- a `--cmdline` filter works on Linux and is refused by
//! name on Windows.

use linklet_core::process::{
    Filter, KillReport, Listing, Refusal, Seen, Target, ToKill, apply, could_not_enumerate,
    matching, plan_kill,
};

// **`target_os = "linux"` and not `unix`**, because this backend reads `/proc`, which is a
// Linux filesystem and not a Unix one. macOS has no equivalent, so a build there is refused by
// the `compile_error!` below rather than shipped with a backend that cannot read anything.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

// A platform with neither backend would fail to compile below with a message about a missing
// module. This says why, in the words of the thing that is missing.
#[cfg(not(any(windows, target_os = "linux")))]
compile_error!(
    "linklet has a process backend for Windows (tasklist) and for Linux (/proc), and not for \
     this platform: see crates/linklet-adapters/src/processes.rs for the three things one has \
     to provide"
);

/// The most processes either backend will look at.
///
/// `tasklist` on a busy machine reports a few hundred lines, and `/proc` on a busy machine has
/// a few hundred entries; the ceiling is here so that a machine with thousands cannot make the
/// agent read an unbounded amount. It is well above [`linklet_core::process::MAX_LISTED`],
/// which is what the caller sees, so the two are not the same number and the difference is
/// deliberate: this one bounds the *reading*, the other bounds the *reply*.
const MAX_READ: usize = 4096;

/// What one enumeration produced.
///
/// Not a [`Listing`]: a filter has not been applied yet, and the count of entries that could
/// not be read is a fact about the reading rather than about the answer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// The processes the machine described.
    pub(crate) processes: Vec<linklet_core::process::Process>,
    /// Entries that were neither a process nor a machine that said "none".
    pub(crate) unreadable: usize,
}

/// Lists the processes on this machine that match `filter`.
///
/// # Errors
///
/// Never returns an error: a machine that could not be asked comes back as a [`Listing`]
/// whose `notes` say so and whose [`linklet_core::process::Incomplete`] is not `No`. That is
/// deliberate and it is the point of the type -- an empty `Vec` and a failed enumeration are
/// different facts, and only one of them is safe to act on.
pub fn list(filter: &Filter) -> Listing {
    match platform::read_all() {
        Ok(parsed) => apply(parsed.processes, filter, parsed.unreadable),
        Err(reason) => could_not_enumerate(&reason, filter),
    }
}

/// Stops the processes a caller asked to stop.
///
/// # What it refuses, before running anything
///
/// The decision is [`linklet_core::process::plan_kill`] and it is pure: a bulk request that was
/// not forced, a request that would stop the agent itself or the process that started it, and
/// **a request against a machine whose process list could not be read**. All three come back as
/// a [`Refusal`] with nothing attempted, which is a different answer from a report showing
/// nothing was killed.
///
/// The third is the reason this function does not simply pass an empty slice when the read
/// fails: the candidate list is what makes the guard work, so "I could not look" has to reach
/// the decision rather than be flattened into "there is nothing there". See [`Seen`].
///
/// The pids that may not be killed are worked out here because they are facts about this
/// running process: the agent's own, and its parent's -- the shell or scheduler that launched
/// it, whose exit may take the agent with it.
///
/// # Errors
///
/// [`Refusal`] when the request must not be attempted. Everything else comes back as a
/// [`KillReport`], including the processes the machine would not stop: a `taskkill` or a
/// `kill` that failed is a result about the machine and not a failure of the call.
pub fn kill(
    to_kill: &ToKill,
    force: bool,
    exclude: Option<&str>,
    filter: &Filter,
) -> Result<KillReport, Refusal> {
    // **Filtered first, then mapped -- and read from the uncapped list.** The filter is what
    // bounds a bulk match to one build rather than everything with a similar name, and the
    // uncapped read is what stops a process past the reply's ceiling from being invisible to a
    // request that names it. See each backend's `read_all`.
    let (candidates, blind) = match platform::read_all() {
        Ok(parsed) => (
            matching(parsed.processes, filter)
                .iter()
                .map(|process| Target {
                    pid: process.pid,
                    name: process.name.clone(),
                })
                .collect::<Vec<Target>>(),
            None,
        ),
        Err(reason) => (Vec::new(), Some(reason)),
    };

    let seen = match blind {
        Some(reason) => Seen::Blind { reason },
        None => Seen::Listed(&candidates),
    };

    let (protected, note) = protected_pids();

    let planned = plan_kill(to_kill, force, exclude, seen, &protected)?;

    let mut report = KillReport {
        matched: planned.len(),
        killed: Vec::new(),
        excluded: Vec::new(),
        failed: Vec::new(),
        notes: note.into_iter().collect(),
    };

    for target in planned {
        if platform::stop(target.pid) {
            report.killed.push(target);
        } else {
            report.failed.push(target);
        }
    }

    Ok(report)
}

/// The processes whose image name is exactly `name`.
///
/// **The testbed feature's question, and the reason it is answered here rather than beside it.**
/// It used to be `tasklist /FI "IMAGENAME eq ..."` inside `system.rs`, which made the
/// `no-process` requirement -- the one that says "this machine has none of yesterday's program
/// on it", and the requirement that makes a testbed worth having on a hand-prepared machine --
/// **Windows-only by accident of where the code lived.** Everything that knows how to read a
/// machine's process list is here, so this is a third caller of a backend rather than a third
/// backend.
///
/// The match is **exact**, case-insensitively, and deliberately not the substring match
/// [`Filter::name`] does: a `--name` filter is looking for what to act on and casts a wide net,
/// while a requirement that fired on a *different* process than the one named would be one
/// nobody could satisfy by fixing the machine. `kill`'s `name` is exact for the same reason.
///
/// # Errors
///
/// The reason the machine's process list could not be read. **Not an empty list**: a testbed
/// that read "I could not look" as "there is nothing there" would declare a machine clean
/// exactly when it cannot tell.
pub fn named(name: &str) -> Result<Vec<String>, String> {
    let parsed = platform::read_all()?;

    Ok(parsed
        .processes
        .into_iter()
        .map(|process| process.name)
        .filter(|found| found.eq_ignore_ascii_case(name))
        .collect())
}

/// The pids this agent must not stop, and a note when the guard is weaker than it should be.
///
/// The agent's own pid is always protected. Its **parent** is protected too, because the
/// process that started the agent may be a shell whose exit takes the agent with it -- on this
/// bench that is how it was launched, so killing the parent is an indirect way of killing the
/// agent.
///
/// # Why the second half can be missing, and why that is said out loud
///
/// The parent is read from the machine, by `wmic` on Windows and from `/proc` on Linux. **A
/// failure here does not fail the kill** -- the agent's own pid is still guarded, which is the
/// case that matters -- but it does make the guard narrower, and a caller that believes the
/// parent is safe when it is not could kill the agent through it. So the note travels with the
/// report instead of being dropped: the same argument as every other note in this feature.
fn protected_pids() -> (Vec<u32>, Option<String>) {
    let own = std::process::id();
    match platform::parent_pid(own) {
        Some(parent) => (vec![own, parent], None),
        None => (
            vec![own],
            Some(
                "the parent process could not be read, so only the agent itself is guarded: \
                 stopping whatever started it may take the agent down too"
                    .to_string(),
            ),
        ),
    }
}
