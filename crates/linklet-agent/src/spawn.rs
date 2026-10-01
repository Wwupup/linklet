//! Starting a program that outlives the request that started it.
//!
//! `execute.rs` runs a command and **waits**, which is right for a build step and wrong for
//! a program meant to keep running. The failure mode is the one `docs/ROADMAP.md` M10 opens
//! with and `E:\projects\lanlink` documents as a pitfall: a long-running program started
//! through the blocking path holds the request, the connection **and the agent's pipes**,
//! so the caller waits for something that is never going to exit.
//!
//! # What makes this different from `run`, in one line
//!
//! **The child gets its own output file rather than the agent's pipes.** Everything else
//! follows from that: the agent can answer immediately, the program can outlive the
//! connection, and the caller has somewhere to look afterwards without needing a new way to
//! read a file.
//!
//! # What it does not do
//!
//! - **No supervision.** A program that exits a second later is not restarted and not
//!   reported. The reply says what was started and its pid, and `ps` is how a caller finds
//!   out whether it is still there -- which is what `docs/ROADMAP.md` M10 asks for and not
//!   a lifecycle.
//! - **No new console, so no survival of the agent's console.** The child is in this
//!   process's console and dies with it, exactly as the agent does. Making it otherwise
//!   needs `DETACHED_PROCESS`, which `std` does not expose safely -- the same refusal, for
//!   the same reason, as the agent's own `--detach` in `docs/ROADMAP.md` M10.
//! - **No wait, not even briefly.** An earlier version slept for a moment and checked
//!   whether the process was still alive, so that "it started and died at once" could be
//!   reported as a failure. It was dropped: it delays every spawn by the length of the
//!   sleep, it makes the answer depend on how fast the machine is, and it is a worse version
//!   of the `ps` call the caller is going to make anyway.

use std::fs::OpenOptions;
use std::process::{Command, Stdio};

use linklet_core::wire::SpawnReport;

/// Starts a command and returns what was started, or why it could not be.
///
/// # Errors
///
/// A sentence for the operator, for the two ways this fails before a process exists: the
/// output file cannot be created, or the shell cannot be started. **A failure after the
/// process exists is not an error** -- a program that exits immediately is a program that
/// was started, and answering otherwise would be claiming to know something this function
/// has not waited to see.
pub fn start(command: &str, output: &str) -> Result<SpawnReport, String> {
    // Created or truncated, and both on purpose: the file is this program's output, and a
    // caller that starts the same command twice wants the second run's output rather than
    // the two interleaved. `create_new` would fail on the ordinary second start, and append
    // would make a log that cannot be told from the previous run's.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(output)
        .map_err(|error| format!("cannot write {output}: {error}"))?;

    // `try_clone` rather than handing the same handle to both: the two streams are separate
    // handles on one file, which is what lets them be redirected independently if this ever
    // needs to.
    let errors = file
        .try_clone()
        .map_err(|error| format!("cannot write {output}: {error}"))?;

    let child = Command::new("cmd")
        .args(["/C", command])
        // **The child's own file, not this process's pipes.** This is the whole difference:
        // a program that inherited the agent's stdout would hold it open for as long as it
        // runs, and the agent's thread would wait for a program that never exits -- the trap
        // this module exists to avoid.
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        .spawn()
        .map_err(|error| format!("cannot start {command:?}: {error}"))?;

    // Deliberately not waited for, and the handle dropped: the child is not this process's
    // to reap, and `Child::drop` does not kill it. What is returned is the pid, which is
    // what the caller watches with `ps`.
    Ok(SpawnReport {
        pid: child.id(),
        command: command.to_string(),
    })
}
