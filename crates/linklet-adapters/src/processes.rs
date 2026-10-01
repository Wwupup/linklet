//! Asking this machine what is running.
//!
//! The counterpart of `linklet_core::process`, which decides what a listing *says*. What is
//! here is the program that produces one, and the parser that reads it -- two things that
//! can only be wrong about this operating system, and therefore two things that belong in
//! the crate that is allowed to touch one.
//!
//! # Why `tasklist`
//!
//! It is present on every Windows machine, it is already what the testbed feature uses to
//! answer "is something still running", and it needs no privileges for the processes the
//! caller owns. The alternative, `Get-CimInstance Win32_Process` through PowerShell, gives
//! command lines for every process on an elevated agent -- a real advantage and a second
//! dependency on a program whose output differs across versions. `tasklist` writes CSV, and
//! CSV is a format this project can pin in a test.
//!
//! # What it does not give
//!
//! **A path or a command line.** `tasklist` has no option for either; a process is a name
//! and a pid. So every [`Process`] here has `path` and `cmdline` set to `None`, the filter
//! reports those fields as unanswerable when a caller asks for them, and the caller is told
//! rather than handed an empty list. That is `docs/ROADMAP.md` M10's own example, and this
//! is the honest version of it: the fields are absent because they were never obtained.

use std::process::Command;

use linklet_core::process::{Filter, Listing, Process, apply, could_not_enumerate};

/// The most processes this will look at.
///
/// `tasklist` on a busy machine reports a few hundred lines; the ceiling is here so that a
/// machine with thousands cannot make the agent read an unbounded amount. It is well above
/// [`linklet_core::process::MAX_LISTED`], which is what the caller sees, so the two are not
/// the same number and the difference is deliberate: this one bounds the *reading*, the
/// other bounds the *reply*.
const MAX_READ: usize = 4096;

/// Lists the processes on this machine that match `filter`.
///
/// # Errors
///
/// Never returns an error: a machine that could not be asked comes back as a
/// [`Listing`] whose `notes` say so and whose [`linklet_core::process::Incomplete`] is not
/// `No`. That is deliberate and it is the point of the type -- an empty `Vec` and a failed
/// enumeration are different facts, and only one of them is safe to act on.
pub fn list(filter: &Filter) -> Listing {
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) => return could_not_enumerate(&format!("cannot run tasklist: {error}"), filter),
    };

    // `tasklist` exits 0 and writes "no tasks are running" when a filter matched nothing,
    // so the status is not the answer. It refuses to run at all rarely enough that the
    // CSV rows are the honest source; a run that produced neither rows nor that notice
    // would be worth knowing about, and its empty output is already visible as `total: 0`.
    let text = String::from_utf8_lossy(&output.stdout);
    let parsed = parse_tasklist(&text);

    apply(parsed.processes, filter, parsed.unreadable)
}

/// What one run of `tasklist` produced.
///
/// Not a [`Listing`]: a filter has not been applied yet, and the count of lines that could
/// not be read is a fact about the output rather than about the answer.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    /// The processes the output described.
    processes: Vec<Process>,
    /// Lines that were not a process and were not the "no tasks" notice either.
    unreadable: usize,
}

/// Reads `tasklist /FO CSV /NH` output.
///
/// # The shapes this has to survive, all of them seen on a real machine
///
/// - `"explorer.exe","5144","Console","3","938,060 K"` -- the ordinary row. **The memory
///   column contains a comma inside a quoted field**, which is the reason this is a CSV
///   parser and not a `split(',')`.
/// - `"System Idle Process","0","Services","0","8 K"` -- a name with spaces.
/// - `INFO: No tasks are running which match the specified criteria.` -- the notice when a
///   filter matched nothing. **It is localised**, so it cannot be recognised by its words;
///   it is recognised by not being a CSV row, and it is not counted as unreadable because
///   it is the answer rather than a failure to read one.
///
/// A line that is neither a row nor that notice is counted as unreadable. Skipping it
/// quietly would let a listing report a clean machine it could not actually read.
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
        // unreadable would turn a complete answer into a machine that could not be read,
        // which is the one confusion this whole feature exists to prevent.
        if is_notice(line) {
            continue;
        }

        match fields(line).as_slice() {
            // Two fields are read: the name and the pid. The session columns are not the
            // same on every Windows version -- a Session Number column appeared in one --
            // and requiring all five would make this fail on a version it otherwise
            // understands perfectly.
            [name, pid, ..] if is_quoted(line) => match pid.parse::<u32>() {
                Ok(pid) => parsed.processes.push(Process::named(pid, name.clone())),
                Err(_) => parsed.unreadable += 1,
            },
            // Anything else the program wrote that is neither a row nor the notice: a line
            // that could not be read, and counted rather than skipped.
            _ => parsed.unreadable += 1,
        }
    }

    parsed
}

/// Whether a line is `tasklist`'s notice rather than a row or a broken row.
///
/// **`INFO:` and not the sentence after it.** `tasklist` prints the rest of that message
/// through `FormatMessage`, so on this machine it reads `INFO: No tasks are running which
/// match the specified criteria.` and on a German one it reads something else -- while the
/// `INFO:` prefix comes from the program's own format string and does not move. Recognising
/// it by its words would count "nothing matched" as a line that could not be read, which is
/// the opposite of what it says.
fn is_notice(line: &str) -> bool {
    line.starts_with("INFO:")
}

/// Whether a line looks like a CSV row rather than a message.
///
/// `tasklist` quotes every field, so a row starts with a quote and a notice does not. This
/// is what makes the localisation of the notice survivable: nothing here reads its words.
fn is_quoted(line: &str) -> bool {
    line.starts_with('"')
}

/// The CSV fields of one line, with the quotes removed.
///
/// A quoted field may contain a comma -- the memory column does -- and a doubled quote
/// inside a quoted field is one quote. That is the whole of the format this uses: no
/// multi-line fields, because `tasklist` writes one process per line.
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

    /// Real output from this machine, which is `zh-CN`: the notice is localised, which is
    /// exactly why nothing here reads it.
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
        // It is the answer -- "nothing matched" -- and counting it as a line that failed
        // to parse would report a machine that could not be read.
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
        // A Session Number column appeared in one version and not another. Reading two
        // fields and ignoring the rest is what keeps this working on both.
        let without_session_number = "\"cmd.exe\",\"1234\",\"Console\",\"2,048 K\"\r\n";
        let parsed = parse_tasklist(without_session_number);

        assert_eq!(parsed.processes.len(), 1, "{parsed:#?}");
        assert_eq!(parsed.processes[0].pid, 1234);
    }

    #[test]
    fn a_process_here_has_no_path_and_no_command_line() {
        // The honest limitation of `tasklist`, stated as a test so that a reader does not
        // add a filter on a field nothing provides and wonder why it matches nothing.
        let parsed = parse_tasklist(REAL_ROWS);
        let process = &parsed.processes[1];

        assert_eq!(process.path, None);
        assert_eq!(process.cmdline, None);
    }
}
