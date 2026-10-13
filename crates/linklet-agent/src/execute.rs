//! Running a command and looking at what it did.
//!
//! The only part of the agent that starts a process. Everything it decides --
//! what a request means, what an answer looks like -- is in
//! `linklet_core::wire`, which is why that module has fifteen tests and this one
//! has none of its own beyond the fact that the server uses it.

use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use linklet_core::wire::{KILLED_BY_DEADLINE, RunOutcome, RunRequest, Text};

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
/// - Each pipe is drained continuously and retains at most one mebibyte of raw
///   tail bytes. Counts and truncation travel with the result under protocol 2.
pub fn run(request: &RunRequest) -> RunOutcome {
    let started = Instant::now();
    let deadline = Duration::from_secs(request.timeout_seconds);

    let failed = |error: linklet_core::result::OperationError, cwd: String| RunOutcome {
        cwd,
        reason: Some(error.message.clone()),
        error: Some(error),
        exit_code: None,
        stdout: Text::default(),
        stderr: Text::default(),
        duration_ms: elapsed_ms(started),
    };
    let (mut command, directory) =
        match crate::shell::invocation(&request.command, request.cwd.as_deref()) {
            Ok(invocation) => invocation,
            Err(error) => return failed(error, request.cwd.clone().unwrap_or_else(|| ".".into())),
        };
    let cwd = directory.to_string_lossy().into_owned();
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return failed(
                linklet_adapters::failure::io(error, "spawn", &directory),
                cwd,
            );
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
                    cwd,
                    error: Some(linklet_core::result::OperationError::new(
                        linklet_core::result::ErrorCode::ResultUnknown,
                        "wait",
                        error.to_string(),
                    )),
                    exit_code: None,
                    stdout: Text::default(),
                    stderr: Text::default(),
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
            cwd,
            error: (!status.success()).then(|| {
                linklet_core::result::OperationError::new(
                    linklet_core::result::ErrorCode::CommandFailed,
                    "execute",
                    format!("command exited with {:?}", status.code()),
                )
            }),
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
            cwd,
            error: Some(linklet_core::result::OperationError::new(
                linklet_core::result::ErrorCode::DeadlineExceeded,
                "execute",
                KILLED_BY_DEADLINE,
            )),
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
/// The decision of *how* belongs to `crate::shell`, which is the one module that knows what
/// it is running on; this is the call site that has a pid and a deadline that has passed.
fn kill_tree(pid: u32) {
    crate::shell::kill_tree(pid);
}

/// Reads a pipe to the end and decodes it, **recording whether anything was lost**.
///
/// Lossy rather than strict: a command that writes one invalid byte should not
/// cost the caller the other ten thousand that were fine. What changed is that the
/// replacement is now reported rather than silent -- `Text` carries the byte count the
/// command wrote and whether any byte had to be replaced, so a caller reading mojibake
/// is told it is mojibake. `docs/ROADMAP.md` M10 is the defect this closes.
fn read_all(mut pipe: impl Read) -> Text {
    let mut capture = linklet_core::output::Capture::default();
    let mut buffer = [0; 8192];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => capture.push(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    linklet_adapters::decode_captured(capture.finish())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn large_output_keeps_the_end_in_bounded_memory() {
        let mut bytes = vec![b'x'; 17 * 1024 * 1024];
        bytes.extend_from_slice(b"END");
        let result = read_all(std::io::Cursor::new(&bytes));
        assert_eq!(result.byte_count(), bytes.len());
        assert!(result.as_str().len() <= 1024 * 1024);
        assert!(result.as_str().ends_with("END"));
    }
    #[cfg(windows)]
    #[test]
    fn both_large_streams_drain_and_return_their_tail() {
        let result = run(&RunRequest {
            cwd: None,
            command: r#"powershell.exe -NoProfile -NonInteractive -Command "$b=[Text.Encoding]::ASCII.GetBytes(('x'*8192));$o=[Console]::OpenStandardOutput();$e=[Console]::OpenStandardError();for($i=0;$i -lt 2200;$i++){$o.Write($b,0,$b.Length);$e.Write($b,0,$b.Length)};$b=[Text.Encoding]::ASCII.GetBytes('END');$o.Write($b,0,3);$e.Write($b,0,3)""#.into(),
            timeout_seconds: 15,
        });
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        for stream in [&result.stdout, &result.stderr] {
            assert_eq!(stream.byte_count(), 2200 * 8192 + 3);
            assert_eq!(stream.retained_bytes(), 1024 * 1024);
            assert!(stream.as_str().ends_with("END"));
            assert!(stream.is_truncated());
        }
        assert!(linklet_core::wire::reply_fits(
            &linklet_core::wire::encode_run_reply(&result),
            linklet_core::wire::reply_ceiling()
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_deadline_stops_large_output_and_keeps_the_partial_result() {
        let result = run(&RunRequest {
            cwd: None,
            command: r#"powershell.exe -NoProfile -NonInteractive -Command "$b=[Text.Encoding]::ASCII.GetBytes(('x'*8192));$o=[Console]::OpenStandardOutput();$e=[Console]::OpenStandardError();while($true){$o.Write($b,0,$b.Length);$e.Write($b,0,$b.Length)}""#.into(),
            timeout_seconds: 1,
        });
        assert_eq!(result.exit_code, None);
        assert_eq!(result.reason.as_deref(), Some(KILLED_BY_DEADLINE));
        assert!(result.stdout.byte_count() > 0);
        assert!(result.stderr.byte_count() > 0);
        assert!(result.stdout.retained_bytes() <= 1024 * 1024);
        assert!(result.stderr.retained_bytes() <= 1024 * 1024);
    }

    #[test]
    fn utf16_output_decodes_before_utf8() {
        let result = read_all(std::io::Cursor::new([0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65]));
        assert_eq!(result.as_str(), "\u{4e2d}\u{6587}");
    }
}

#[cfg(test)]
mod invocation_tests {
    use super::*;
    #[test]
    fn direct_program_uses_its_own_directory_and_closed_stdin() {
        let cwd = std::env::temp_dir().join(format!("linklet cwd \u{4e2d} {}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("directory");
        let request = linklet_core::wire::run_request_from_json(&serde_json::json!({
            "program": if cfg!(windows) {"cmd.exe"} else {"sh"},
            "args": if cfg!(windows) {vec!["/D", "/C", "cd"]} else {vec!["-c", "pwd; read x || exit 0"]},
            "cwd":cwd, "timeout_seconds":2
        })).expect("request");
        let result = run(&request);
        std::fs::remove_dir(&cwd).expect("cleanup");
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert!(
            result
                .stdout
                .as_str()
                .contains(&cwd.to_string_lossy().to_string()),
            "{result:?}"
        );
    }
}
