//! The platform's shell, and how to stop what it started.
//!
//! **This is the only module in the project that knows which operating system it is on**,
//! and it exists so that the rest does not. It was written when the answer to "can this run
//! on Linux" turned out to be "the protocol, the cipher and both transfer directions
//! already do, and `exec` does not, because `cmd` is not there" -- see `docs/ROADMAP.md`
//! M11 for the measurement. Two call sites needed it and the two files each hardcoded
//! `cmd`, which is the shape of a decision that has not been made in one place.
//!
//! # What it deliberately is not
//!
//! **Not a trait, and not a configuration option.** A trait in the core would be the
//! textbook move -- M2 did exactly that for the socket, and `docs/ROADMAP.md` M11 says so --
//! and it would be machinery for two implementations that cannot both be present in one
//! binary. Which shell to use is not a choice the operator makes or a fake a test supplies;
//! it is a fact about the machine the agent is running on, and `cfg` is the way to say a
//! fact.
//!
//! **Not a shell that is chosen for the *command*.** The caller writes the command line and
//! owns its quoting, on both platforms. What changes is only what that line is handed to.

use std::process::{Command, Stdio};

/// A command that runs `line` through this platform's shell.
///
/// Windows gets `cmd /C`, which is what this project has always used and what every
/// documented example is written against. Everything else gets `sh -c`, which is the POSIX
/// answer and the one a caller writing a command line for a Linux target expects.
///
/// **The process is put in its own process group on Unix**, and that is not incidental: it
/// is what makes [`kill_tree`] able to stop the whole tree rather than the shell, which is
/// the same property `taskkill /T` provides on Windows. Without it, a deadline would kill
/// `sh` and leave the program it started holding the pipes -- the exact bug the Windows side
/// of this module's history records.
pub fn command_line(line: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", line]);
        command
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("sh");
        command.args(["-c", line]);
        // 0 means "make this child the leader of a new group whose id is its own pid", so
        // `kill -TERM -<pid>` reaches everything it starts.
        command.process_group(0);
        command
    }
}

/// Kills a process and everything it started.
///
/// The pid rather than a name, deliberately, on both platforms: killing by name would match
/// anything else on the machine with the same name, including a process the operator started
/// and would like to keep.
///
/// A failure is ignored on purpose. It is called from a deadline that has already passed, the
/// caller is going to be told the command was killed either way, and an error path that
/// cannot report anything useful would only obscure that.
pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        // `/T` walks the child list and `/F` does not ask. `Child::kill` alone is not enough:
        // a shell that has started a program leaves that program running, holding the pipes,
        // and the caller waits for output that will never arrive because nothing is going to
        // write it and nothing has closed it.
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(unix)]
    {
        // Through `sh`, and through the shell's own `kill` builtin rather than an external
        // one: `sh` is already required for every command this agent runs, so this adds no
        // new requirement, where `/bin/kill` is a package that a minimal image may not have.
        //
        // A negative pid means the process *group*, which is the group
        // [`command_line`] put the child in. `KILL` and not `TERM` because this runs after a
        // deadline has already passed and there is no second chance to escalate.
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -9 -{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
