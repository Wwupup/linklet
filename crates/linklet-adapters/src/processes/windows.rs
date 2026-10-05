//! The Windows backend: `tasklist`, `taskkill`, `wmic`.
//!
//! # Why `tasklist`
//!
//! It is present on every Windows machine, it is already what the testbed feature uses to
//! answer "is something still running", and it needs no privileges for the processes the
//! caller owns. The alternative, `Get-CimInstance Win32_Process` through PowerShell, gives
//! command lines for every process on an elevated agent -- a real advantage and a second
//! dependency on a program whose output differs across versions. `tasklist` writes CSV, and
//! CSV is a format this project can pin in a test.

use std::process::Command;

use linklet_core::process::Process;

use super::{MAX_READ, Parsed};

/// Every process on this machine, unfiltered and uncapped.
///
/// **The reply has a ceiling and this does not**, and the distinction is load-bearing.
/// [`linklet_core::process::MAX_LISTED`] exists so that one reply cannot be the thing that
/// fails -- a busy machine has hundreds of processes and the reply is one frame -- but a caller
/// that used the capped listing to decide what to kill would not find a process past the
/// ceiling. The failure that produces is the worst one available here: `matched: 0` for a
/// process that is running, read by a deploy loop as a clean machine.
///
/// This was found by a test rather than by reading: the deploy-loop test looked for a `ping`
/// with `--name` (which found it, and `ps` is capped the same way) and then asked to kill it by
/// pid through an unfiltered listing, where the process was past the ceiling and simply was not
/// there. **A cap on what is reported must not be a cap on what is acted on.**
///
/// Still bounded by [`MAX_READ`], which bounds the reading rather than the answer.
///
/// # Errors
///
/// The reason `tasklist` could not be run. It is an error rather than an empty list because
/// the two mean opposite things to a caller deciding whether it is safe to overwrite a file.
pub(super) fn read_all() -> Result<Parsed, String> {
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .map_err(|error| format!("cannot run tasklist: {error}"))?;

    // `tasklist` exits 0 and writes "no tasks are running" when a filter matched nothing, so
    // the status is not the answer. A failure to run at all is already the `map_err` above.
    Ok(parse_tasklist(&String::from_utf8_lossy(&output.stdout)))
}

/// Whether a process is gone after being asked to stop.
///
/// `taskkill /T /F`: the tree, so that a program's own children go with it -- the same reason
/// `execute.rs` uses it for a deadline. `/F` because a task that ignores a polite close would
/// otherwise be reported as killed while it is still running, and this report is what a deploy
/// loop trusts before overwriting a file.
///
/// A failure is `false` and not an error: `taskkill` exits non-zero when the process is already
/// gone, which is the outcome the caller wanted, so the answer is checked against the machine
/// rather than against the exit code.
pub(super) fn stop(pid: u32) -> bool {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();

    !exists(pid)
}

/// Whether a pid is in the machine's process list.
///
/// **`tasklist` filtered by pid, and not the ordinary listing.** The ordinary one is capped at
/// [`linklet_core::process::MAX_LISTED`] for the reply, so a process just past that ceiling
/// would look absent -- and a kill that reported success on a process which is still running is
/// exactly the answer a deploy loop must not be given. The filtered form returns the one pid or
/// the "no tasks" notice, and the notice is read as "gone" because that is what it says.
fn exists(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output();

    let Ok(output) = output else {
        // The list could not be read, so "gone" cannot be claimed. Reporting the process as
        // still running sends the caller to look again; reporting it as stopped would send the
        // caller to overwrite a file a live process is holding.
        return true;
    };

    !parse_tasklist(&String::from_utf8_lossy(&output.stdout))
        .processes
        .is_empty()
}

/// The parent of a process, out of `wmic`'s own output.
///
/// `tasklist` does not report it. `None` says "not known" rather than guessing a number that
/// might belong to something else -- a wrong pid here would protect an arbitrary process or
/// fail to protect the right one, and both are worse than the narrower guard the caller is told
/// about.
pub(super) fn parent_pid(pid: u32) -> Option<u32> {
    let output = Command::new("wmic")
        .args([
            "process",
            "where",
            &format!("ProcessId={pid}"),
            "get",
            "ParentProcessId",
            "/value",
        ])
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.trim().strip_prefix("ParentProcessId="))
        .and_then(|value| value.trim().parse().ok())
}

/// Reads `tasklist /FO CSV /NH` output.
///
/// # The shapes this has to survive, all of them seen on a real machine
///
/// - `"explorer.exe","5144","Console","3","938,060 K"` -- the ordinary row. **The memory column
///   contains a comma inside a quoted field**, which is the reason this is a CSV parser and not
///   a `split(',')`.
/// - `"System Idle Process","0","Services","0","8 K"` -- a name with spaces.
/// - `INFO: No tasks are running which match the specified criteria.` -- the notice when a
///   filter matched nothing. **It is localised**, so it cannot be recognised by its words; it
///   is recognised by not being a CSV row, and it is not counted as unreadable because it is
///   the answer rather than a failure to read one.
///
/// A line that is neither a row nor that notice is counted as unreadable. Skipping it quietly
/// would let a listing report a clean machine it could not actually read.
fn parse_tasklist(text: &str) -> Parsed {
    let mut parsed = Parsed::default();

    for line in text.lines() {
        if parsed.processes.len() >= MAX_READ {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // **The notice is the answer, not a line that failed to be read.** It goes first,
        // because that is what it is: `tasklist` saying "nothing matched". Counting it as
        // unreadable would turn a complete answer into a machine that could not be read, which
        // is the one confusion this whole feature exists to prevent.
        if is_notice(line) {
            continue;
        }

        match fields(line).as_slice() {
            // Two fields are read: the name and the pid. The session columns are not the same
            // on every Windows version -- a Session Number column appeared in one -- and
            // requiring all five would make this fail on a version it otherwise understands
            // perfectly.
            [name, pid, ..] if is_quoted(line) => match pid.parse::<u32>() {
                Ok(pid) => parsed.processes.push(Process::named(pid, name.clone())),
                Err(_) => parsed.unreadable += 1,
            },
            // Anything else the program wrote that is neither a row nor the notice: a line that
            // could not be read, and counted rather than skipped.
            _ => parsed.unreadable += 1,
        }
    }

    parsed
}

/// Whether a line is `tasklist`'s notice rather than a row or a broken row.
///
/// **`INFO:` and not the sentence after it.** `tasklist` prints the rest of that message through
/// `FormatMessage`, so on this machine it reads `INFO: No tasks are running which match the
/// specified criteria.` and on a German one it reads something else -- while the `INFO:` prefix
/// comes from the program's own format string and does not move. Recognising it by its words
/// would count "nothing matched" as a line that could not be read, which is the opposite of
/// what it says.
fn is_notice(line: &str) -> bool {
    line.starts_with("INFO:")
}

/// Whether a line looks like a CSV row rather than a message.
///
/// `tasklist` quotes every field, so a row starts with a quote and a notice does not. This is
/// what makes the localisation of the notice survivable: nothing here reads its words.
fn is_quoted(line: &str) -> bool {
    line.starts_with('"')
}

/// The CSV fields of one line, with the quotes removed.
///
/// A quoted field may contain a comma -- the memory column does -- and a doubled quote inside a
/// quoted field is one quote. That is the whole of the format this uses: no multi-line fields,
/// because `tasklist` writes one process per line.
fn fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '"' => {
                // A doubled quote inside a quoted field is one literal quote.
                if quoted && characters.peek() == Some(&'"') {
                    current.push('"');
                    let _ = characters.next();
                } else {
                    quoted = !quoted;
                }
            }
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    fields.push(current);

    fields
        .into_iter()
        .map(|field| field.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output from a machine running `zh-CN`: the notice is localised, which is exactly
    /// why nothing here reads it.
    const REAL_ROWS: &str = "\
\"System Idle Process\",\"0\",\"Services\",\"0\",\"8 K\"\r
\"explorer.exe\",\"5144\",\"Console\",\"3\",\"938,060 K\"\r
";

    #[test]
    fn a_real_row_is_read_including_the_comma_inside_a_field() {
        let parsed = parse_tasklist(REAL_ROWS);

        assert_eq!(parsed.unreadable, 0, "{parsed:#?}");
        assert_eq!(parsed.processes.len(), 2);
        assert_eq!(parsed.processes[0].name, "System Idle Process");
        assert_eq!(parsed.processes[0].pid, 0);
        assert_eq!(parsed.processes[1].name, "explorer.exe");
        assert_eq!(parsed.processes[1].pid, 5144);
    }

    #[test]
    fn the_no_tasks_notice_is_not_a_process_and_not_an_unreadable_line() {
        // It is the answer -- "nothing matched" -- and counting it as a line that failed to
        // parse would report a machine that could not be read.
        let parsed =
            parse_tasklist("INFO: No tasks are running which match the specified criteria.\r\n");

        assert!(parsed.processes.is_empty());
        assert_eq!(parsed.unreadable, 0, "{parsed:#?}");
    }

    #[test]
    fn a_line_that_is_neither_a_row_nor_a_notice_is_counted() {
        let parsed = parse_tasklist("\"broken.exe\",\"not-a-pid\",\"Console\",\"1\",\"4 K\"\r\n");

        assert!(parsed.processes.is_empty());
        assert_eq!(parsed.unreadable, 1, "{parsed:#?}");
    }

    #[test]
    fn the_parser_does_not_require_the_columns_one_windows_version_added() {
        // A Session Number column appeared in one version and not another. Reading two fields
        // and ignoring the rest is what keeps this working on both.
        let without_session_number = "\"cmd.exe\",\"1234\",\"Console\",\"2,048 K\"\r\n";
        let parsed = parse_tasklist(without_session_number);

        assert_eq!(parsed.processes.len(), 1, "{parsed:#?}");
        assert_eq!(parsed.processes[0].pid, 1234);
    }

    #[test]
    fn a_process_here_has_no_path_and_no_command_line() {
        // The honest limitation of `tasklist`, stated as a test so that a reader does not add a
        // filter on a field nothing provides and wonder why it matches nothing.
        let parsed = parse_tasklist(REAL_ROWS);
        let process = &parsed.processes[1];

        assert_eq!(process.path, None);
        assert_eq!(process.cmdline, None);
    }
}
