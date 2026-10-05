//! The Linux backend: `/proc`, and `kill`.
//!
//! # Why `/proc` rather than `ps`
//!
//! `ps` is the obvious counterpart of `tasklist`, and it is the wrong choice here for the
//! reason this project has already paid for twice: **its output is written for a person, in
//! the machine's language, in a format that differs between implementations.** `ipconfig` cost
//! a parser that reads addresses instead of labels; `tasklist` cost a notice recognised by a
//! prefix because its sentence is translated. `/proc` is the machine-readable answer those
//! programs are formatting, it is present on every Linux system, it is not localised, and it
//! gives more than `ps` would: the command line, the executable's path, and the parent pid,
//! each from its own file.
//!
//! The cost is that this backend is Linux and not Unix -- `/proc` is a Linux filesystem and
//! macOS has no equivalent -- which is why `processes.rs` gates it on `target_os = "linux"`
//! rather than on `unix`, and names what a new backend would have to provide.
//!
//! # What `None` means here
//!
//! **"The agent was not allowed to look", never "it is empty".** `/proc/<pid>/exe` and
//! `/proc/<pid>/cmdline` are readable for a caller's own processes and refused for other
//! users' without `CAP_SYS_PTRACE`, so a non-elevated agent describes its own processes in
//! full and other users' by name and pid alone. That is the same asymmetry the Windows backend
//! has for a different reason, and it is reported the same way: the field is `None`, and
//! [`linklet_core::process::Filter::unanswerable`] says so rather than answering "no match".

use std::process::Command;
use std::time::{Duration, Instant};

use linklet_core::process::Process;

use super::{MAX_READ, Parsed};

/// The three fields of `/proc/<pid>/stat` this needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stat {
    /// The kernel's name for the process, at most fifteen bytes.
    comm: String,
    /// The parent's pid, which is what the kill guard is built from.
    ppid: u32,
    /// The process group, which decides whether a kill can take the tree.
    pgrp: u32,
    /// The state character: `R`, `S`, `D`, `Z` and so on.
    state: char,
}

impl Stat {
    /// Whether this process is a zombie: finished, and not yet reaped.
    ///
    /// **A zombie is not a running process**, and treating it as one would break the deploy
    /// loop on Linux for a reason Windows does not have. A program started by `spawn` is this
    /// agent's child, and nothing here waits on it, so when it is killed it stays in the
    /// process table as a zombie until the agent exits. Reporting that as "still running" would
    /// make `kill` answer `killed 0 of 1` for a program that is already gone.
    fn is_gone(&self) -> bool {
        self.state == 'Z'
    }
}

/// Every process on this machine, unfiltered and uncapped.
///
/// **The reply has a ceiling and this does not**: see the same function in the Windows backend
/// for the argument, which is the same one. A process past [`linklet_core::process::MAX_LISTED`]
/// is invisible to a capped listing, and a kill that could not see it would report `matched: 0`
/// for something that is running.
///
/// # Errors
///
/// The reason `/proc` could not be read. It is an error rather than an empty list because the
/// two mean opposite things to a caller deciding whether it is safe to overwrite a file -- and
/// on this platform that distinction is worth more than on Windows, because `/proc` is what
/// makes the kill guard possible at all.
pub(super) fn read_all() -> Result<Parsed, String> {
    let entries =
        std::fs::read_dir("/proc").map_err(|error| format!("cannot read /proc: {error}"))?;

    let mut parsed = Parsed::default();

    for entry in entries {
        if parsed.processes.len() >= MAX_READ {
            break;
        }
        // An entry that cannot be read at all is not a process that could not be described:
        // it is an entry this walk has no business with. Counting it would make every listing
        // incomplete.
        let Ok(entry) = entry else {
            continue;
        };

        // **`/proc` is not a list of processes.** It also holds `self`, `meminfo`, `sys` and
        // the rest of the kernel's own interface; a name that is not a number is one of those,
        // and skipping it is the format rather than a failure to read it.
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };

        match describe(pid) {
            Some(process) => parsed.processes.push(process),
            // **`None` has two causes and this tells them apart, which is the difference
            // between a complete answer and a machine that could not be read.**
            //
            // A process that exited between the directory read and the stat is the ordinary
            // race of reading a live process table; a *zombie* is a process that has finished
            // and not been reaped -- which on this platform is every program `spawn` started
            // and `kill` stopped, because nothing here waits on them. Neither is a failure to
            // read anything, and counting them would make a listing intermittently incomplete
            // on a busy machine.
            //
            // `exists` is the right question rather than "is the directory there", because it
            // is the same question `kill` asks: a zombie's directory is present and the
            // process is gone. Asking anything else here would let `ps` and `kill` disagree
            // about whether a process is running, which is exactly the disagreement the deploy
            // loop cannot survive.
            None => {
                if exists(pid) {
                    parsed.unreadable += 1;
                }
            }
        }
    }

    Ok(parsed)
}

/// Whether a process is gone after being asked to stop.
///
/// **`SIGKILL`, and the process group when this pid leads one.** The group is what makes this
/// the tree: `spawn` puts the program it starts in a group of its own, so the shell it runs
/// through and the program itself both go. A pid that does *not* lead a group is killed on its
/// own, and that is deliberate rather than a limitation -- a pid found by `ps` may share a
/// group with the agent, the shell that started it and the whole test harness, and signalling
/// that group would take everything down.
///
/// A failure is `false` and not an error: a pid that is already gone is the outcome the caller
/// wanted, and the answer is checked against the machine rather than against `kill`'s exit
/// code.
pub(super) fn stop(pid: u32) -> bool {
    let target = match stat_of(pid) {
        Some(stat) if stat.pgrp == pid => format!("-{pid}"),
        _ => pid.to_string(),
    };

    // Through `sh`, and through the shell's own `kill` builtin rather than an external one:
    // `sh` is already required for every command this agent runs, so this adds no new
    // requirement, where `/bin/kill` is a package a minimal image may not have. This is the
    // same reasoning as `crate::shell`, which sends the group signal the same way.
    let _ = Command::new("sh")
        .args(["-c", &format!("kill -9 {target}")])
        .output();

    wait_gone(pid)
}

/// Waits, briefly, for a signal that has been sent to take effect.
///
/// **`SIGKILL` is not instantaneous from the sender's side.** `kill` returns as soon as the
/// signal is queued, and the process is gone when the kernel has finished with it. A report
/// that said "killed" before that would be a claim rather than an observation, and this report
/// is what a deploy loop trusts before overwriting a file a live process might be holding.
///
/// Bounded, because a process that will not die must not hang the agent: after two seconds the
/// answer is "still there", which sends the caller to look again rather than to overwrite.
fn wait_gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if !exists(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether a pid is a process that is still running.
///
/// A zombie is not: see [`Stat::is_gone`], which is the whole reason this reads the state
/// rather than checking that the directory exists.
///
/// When the state cannot be read, "still running" is the answer given, and that is the safe
/// direction: reporting a live process as stopped sends the caller to overwrite a file it is
/// holding.
fn exists(pid: u32) -> bool {
    match stat_of(pid) {
        Some(stat) => !stat.is_gone(),
        None => std::path::Path::new(&format!("/proc/{pid}")).exists(),
    }
}

/// The parent of a process, out of its own `stat`.
///
/// `/proc` reports it directly, so unlike the Windows backend this needs no second program.
/// `None` says "not known" rather than guessing a number that might belong to something else.
pub(super) fn parent_pid(pid: u32) -> Option<u32> {
    stat_of(pid).map(|stat| stat.ppid)
}

/// The three fields of `/proc/<pid>/stat`, or `None` when it cannot be read.
fn stat_of(pid: u32) -> Option<Stat> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    // Lossy rather than strict: `comm` is a byte string and a process may put anything in it.
    // A name that is not UTF-8 is a name this can still report approximately, whereas a failure
    // to read would lose the pid, the parent and the state with it.
    parse_stat(&String::from_utf8_lossy(&raw))
}

/// One process, described as far as this agent is allowed to describe it.
///
/// `None` means "there is no process to describe": either it has finished (see below), or its
/// `/proc/<pid>/stat` could not be read. The caller tells those apart with [`exists`], which is
/// the same question `kill` asks.
fn describe(pid: u32) -> Option<Process> {
    let stat = stat_of(pid)?;

    // **A zombie is not listed**, and this is not tidiness. A program `spawn` started is this
    // agent's child and nothing reaps it, so a killed one stays in the table; listing it would
    // have `ps` report the old build as still running, which is the exact conclusion the deploy
    // loop must not reach -- and `kill` would simultaneously report it gone, because `exists`
    // reads the state. One of the two answers would be wrong, and a caller has no way to know
    // which.
    //
    // Windows does not have this state at all, so nothing there is being made inconsistent: a
    // finished process leaves `tasklist` on both platforms now.
    if stat.is_gone() {
        return None;
    }

    // The executable's real path, which is also the best name available: `comm` is capped at
    // fifteen bytes by the kernel, so the name of anything longer is truncated there and a
    // caller filtering by the name it can see in its own file manager would not match.
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();

    let name = exe
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| stat.comm.clone());

    Some(Process {
        pid,
        name,
        path: exe.map(|path| path.to_string_lossy().into_owned()),
        cmdline: read_cmdline(pid),
    })
}

/// A process's command line, or `None` when the agent was not allowed to read it.
///
/// **`Some("")` and `None` are different answers and both are real here.** A kernel thread has
/// an empty `/proc/<pid>/cmdline` and a readable one, which is `Some("")`; another user's
/// process is refused, which is `None`. Folding them together would let a `--cmdline` filter
/// report "nothing matched" for a machine it could not read.
fn read_cmdline(pid: u32) -> Option<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|raw| join_cmdline(&raw))
}

/// The command line from `/proc/<pid>/cmdline`, which is NUL-separated arguments.
///
/// The trailing NUL is a terminator rather than an empty final argument, so it is dropped.
/// That is the whole of the format, and it is why this is a parser rather than a `replace`:
/// an argument may legitimately be empty, and only the *last* empty field is the terminator.
fn join_cmdline(raw: &[u8]) -> String {
    let mut parts: Vec<String> = raw
        .split(|byte| *byte == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();

    // The last field is the terminator rather than an argument, so the empty string it leaves
    // is dropped -- once, and not repeatedly: an argument that is genuinely empty in the middle
    // of the line is a real argument and stays.
    if parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }

    parts.join(" ")
}

/// The fields of `/proc/<pid>/stat`.
///
/// # The shape this has to survive
///
/// ```text
/// 5144 (linklet-agent) S 1 5144 5144 0 -1 4194560 ...
/// ```
///
/// `pid (comm) state ppid pgrp session ...`, and **`comm` is the awkward field**: it is
/// arbitrary bytes chosen by the process, it may contain spaces, and it may contain parentheses
/// -- `(sd-pam)` is a real one on a modern systemd machine. So neither `split_whitespace` nor a
/// search for the first `)` can find the end of it.
///
/// The end is the **last** `)` in the line, which is unambiguous because every field after
/// `comm` is a single character or a number and none of them contains a parenthesis. That is
/// what this reads, and the test below has a `comm` with both a space and a parenthesis in it.
fn parse_stat(text: &str) -> Option<Stat> {
    // The name is between the first `(` and the **last** `)`: see the doc comment for why the
    // last one is the only unambiguous choice.
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = text[open + 1..close].to_string();

    // ` state ppid pgrp ... ` -- everything after the name, and the three fields this needs are
    // the first three of them.
    let mut fields = text[close + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;

    Some(Stat {
        comm,
        ppid,
        pgrp,
        state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real line from a Linux machine, with the fields this reads marked by their positions.
    const REAL_LINE: &str = "5144 (linklet-agent) S 1 5144 5144 0 -1 4194560 1234 0 0 0 5 3 0 0 20 0 8 0 98765 12345678 4321 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn a_real_stat_line_gives_the_name_the_parent_and_the_group() {
        let stat = parse_stat(REAL_LINE).expect("a line the kernel wrote");

        assert_eq!(stat.comm, "linklet-agent");
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.pgrp, 5144);
        assert_eq!(stat.state, 'S');
        assert!(!stat.is_gone());
    }

    #[test]
    fn a_name_with_a_space_and_a_parenthesis_in_it_is_read_whole() {
        // `(sd-pam)` is a real process name on a systemd machine, and it is why neither
        // `split_whitespace` nor the *first* `)` can be used to find the end of the field: the
        // first one ends `(sd-pam` and leaves `)` to be read as the state.
        let line = "900 (sd-pam) S 1 900 900 0 -1 4194304 100 0 0 0 0 0 0 0 20 0 1 0 1000";
        let stat = parse_stat(line).expect("a name with a parenthesis is still a line");

        assert_eq!(stat.comm, "sd-pam");
        assert_eq!(stat.state, 'S');
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.pgrp, 900);

        // And one with a space in it, which is the other shape that breaks a naive split.
        let spaced = "901 (Web Content) R 1 901 901 0 -1 4194304 100 0 0 0 0 0 0 0 20 0 1 0 1000";
        let stat = parse_stat(spaced).expect("a name with a space is still a line");
        assert_eq!(stat.comm, "Web Content");
        assert_eq!(stat.state, 'R');
    }

    #[test]
    fn a_zombie_is_not_a_running_process() {
        // The state Linux has and Windows does not, and the reason this reads the state rather
        // than checking that the directory exists: a program started by `spawn` is this agent's
        // child, nothing waits on it, and a killed one stays in the table until the agent exits.
        // Reporting that as running would make `kill` answer `killed 0 of 1` for a program that
        // is already gone.
        let zombie = "777 (defunct) Z 1 777 777 0 -1 4194304 0 0 0 0 0 0 0 0 20 0 1 0 1000";
        let stat = parse_stat(zombie).expect("a zombie is still a line");

        assert!(stat.is_gone(), "{stat:#?}");
    }

    #[test]
    fn a_line_that_is_not_a_stat_line_is_refused_rather_than_guessed_at() {
        // A process that exits between the directory listing and this read gives an empty or
        // partial file, and inventing a pid, a parent or a state for it would be inventing a
        // process -- which is the one thing this feature must never do.
        for broken in [
            "",
            "not a stat line at all",
            "5144 (linklet-agent) S",
            "5144 (linklet-agent) S not-a-pid 5144",
            "5144 linklet-agent S 1 5144",
        ] {
            assert_eq!(parse_stat(broken), None, "{broken:?}");
        }
    }

    #[test]
    fn a_command_line_is_its_arguments_separated_by_spaces() {
        assert_eq!(
            join_cmdline(b"/usr/bin/linklet-agent\0--port\08787\0--root\0/tmp\0"),
            "/usr/bin/linklet-agent --port 8787 --root /tmp"
        );
    }

    #[test]
    fn the_terminating_nul_is_not_an_empty_argument() {
        // Every field is NUL-terminated, including the last, so the naive split produces one
        // empty argument more than there are arguments. A caller reading the command line back
        // would see a trailing space that no one typed.
        assert_eq!(join_cmdline(b"prog\0arg\0"), "prog arg");
        // And a genuinely empty argument in the middle survives, which is why this is not a
        // `replace` of one NUL into one space followed by a trim.
        assert_eq!(join_cmdline(b"prog\0\0arg\0"), "prog  arg");
    }

    #[test]
    fn an_empty_command_line_is_an_empty_answer_and_not_a_failure() {
        // A kernel thread has a readable and empty `/proc/<pid>/cmdline`. `Some("")` says "it
        // has no command line", `None` says "I was not allowed to read it", and a `--cmdline`
        // filter has to be able to tell those apart.
        assert_eq!(join_cmdline(b""), "");
        assert_eq!(join_cmdline(b"\0"), "");
    }

    #[test]
    fn a_command_line_that_is_not_utf8_is_still_a_command_line() {
        // An argument may be arbitrary bytes. Lossy is the honest reading: the caller gets the
        // command line with the bad bytes marked rather than an empty answer, which is the same
        // choice the agent makes for a command's output.
        let raw = b"prog\0\xd6\xd0\xce\xc4\0";
        let joined = join_cmdline(raw);

        assert!(joined.starts_with("prog "), "{joined:?}");
        assert!(joined.contains('\u{fffd}'), "{joined:?}");
    }
}
