//! Running a command and looking at what it did.
//!
//! The only part of the agent that starts a process. Everything it decides --
//! what a request means, what an answer looks like -- is in
//! `linklet_core::wire`, which is why that module has fifteen tests and this one
//! has none of its own beyond the fact that the server uses it.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use linklet_core::wire::{KILLED_BY_DEADLINE, RunOutcome, RunRequest};

/// Runs a shell command and reports what happened.
///
/// Spawned through `cmd /C` explicitly rather than with `shell: true`, so that
/// what runs is a decision made here and visible in the code, not a default
/// inherited from a library.
///
/// # What it does not do
///
/// - **No quoting rules of its own.** The command line goes to `cmd.exe` as one
///   argument. Whoever wrote it is responsible for it, in the same way they would
///   be at a prompt -- a tool that "helpfully" re-quoted would be a second
///   interpretation of a string the caller already decided on.
/// - **No output limit of its own, and a limit it does meet.** A command that writes a
///   gigabyte writes a gigabyte, and this function will hold all of it: bounded output is a
///   real need and belongs in the wire protocol as a field, not as a silent truncation
///   here. But the *reply* is one frame, so `MAX_PAYLOAD` (16 MiB) is the real ceiling --
///   and today a command past it is **reported as a dropped connection**, because the
///   sealed reply cannot be framed, the agent sends nothing, and closes. That is a defect
///   and not a design: `docs/ROADMAP.md` M10 carries it, with the measurement (20,000,000
///   bytes of output, the command exiting 0 on the target, the caller told "could not reach
///   the agent").
pub fn run(request: &RunRequest) -> RunOutcome {
    let started = Instant::now();
    let deadline = Duration::from_secs(request.timeout_seconds);

    let mut child = match Command::new("cmd")
        .args(["/C", &request.command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        // No process, so no exit code, and the reason says which failure this is.
        // `exit_code: None` with a reason of "failed to start" is a different
        // fact from `exit_code: None` with a deadline, and the host acts on the
        // difference.
        Err(error) => {
            return RunOutcome {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                duration_ms: elapsed_ms(started),
                reason: Some(format!("cannot spawn: {error}")),
            };
        }
    };

    // Read both pipes on their own threads before waiting.
    //
    // This is the bug this function exists to avoid. Waiting first and reading
    // afterwards deadlocks the moment the command fills a pipe buffer: the child
    // blocks writing, the parent blocks waiting, and a command that works
    // perfectly at a prompt hangs when run through here. It is also
    // intermittently reproducible, which is what makes it worth a comment rather
    // than a fix.
    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");

    let stdout_reader = std::thread::spawn(move || read_all(stdout_pipe));
    let stderr_reader = std::thread::spawn(move || read_all(stderr_pipe));

    // Wait with a deadline, by polling. `Child::wait_timeout` is not in std, and
    // a thread that kills on a timer would need the child handle shared, which
    // means a lock held across a wait -- so polling is the honest simple version.
    // The interval is short enough that a killed command's overhead is invisible
    // next to what it was doing.
    let poll = Duration::from_millis(20);
    let mut waited = Duration::ZERO;
    let mut killed_by_deadline = false;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if waited >= deadline {
                    // Kill the **tree**, not just the shell.
                    //
                    // This was a real bug here, of exactly the kind this
                    // repository keeps finding. `cmd /C ping -n 30 ...` makes ping
                    // a *child* of cmd, and killing cmd leaves ping running --
                    // holding both pipes. The reader threads then never finish,
                    // the agent never replies, and the caller reports a transport
                    // failure for a command the agent was about to describe
                    // properly. The diagnosis took a timeout to even see.
                    //
                    // The lesson is in the upstream project's pitfalls file, which
                    // is where the cost of learning it the first time was paid.
                    kill_tree(child.id());
                    killed_by_deadline = true;
                }
                std::thread::sleep(poll);
                waited += poll;
            }
            // A wait that fails is not a process that exited, and inventing an
            // exit code for it would be the one thing this module must not do.
            Err(error) => {
                return RunOutcome {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    duration_ms: elapsed_ms(started),
                    reason: Some(format!("cannot wait: {error}")),
                };
            }
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let duration_ms = elapsed_ms(started);

    match (status, killed_by_deadline) {
        (Some(status), false) => RunOutcome {
            exit_code: status.code(),
            stdout,
            stderr,
            duration_ms,
            // No code and not killed: the process ended by a signal, and saying
            // "failed to start" would be wrong. `status.code()` is `None` for a
            // signal on Unix and for a terminated process on Windows.
            reason: if status.code().is_none() {
                Some("the process ended without an exit code".to_string())
            } else {
                None
            },
        },
        // Killed, whether or not the kill has landed yet. The output it managed
        // to write before that is still returned -- a killed build's output is
        // exactly what the caller wants to read.
        _ => RunOutcome {
            exit_code: None,
            stdout,
            stderr,
            duration_ms,
            reason: Some(KILLED_BY_DEADLINE.to_string()),
        },
    }
}

/// Kills a process and everything it started.
///
/// `taskkill /T` walks the child list and `/F` does not ask. `Child::kill` alone
/// is not enough on Windows: a shell that has started a program leaves that
/// program running, holding the pipes, and the caller waits for output that will
/// never arrive because nothing is going to write it and nothing has closed it.
///
/// The pid rather than the name, deliberately. Killing by name would match
/// anything else on the machine with the same name, including a process the
/// operator started and would like to keep.
///
/// A failure here is ignored on purpose: it is called from a deadline that has
/// already passed, the caller is going to be told the command was killed either
/// way, and an error path that cannot report anything useful would only obscure
/// that.
fn kill_tree(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Reads a pipe to the end as bytes, then as much text as decodes.
///
/// Lossy rather than strict: a command that writes one invalid byte should not
/// cost the caller the other ten thousand that were fine, and "some bytes were
/// replaced" is visible in the output rather than silent.
fn read_all(mut pipe: impl Read) -> String {
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}
