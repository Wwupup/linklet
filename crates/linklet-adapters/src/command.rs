//! Processes whose interaction is through pipes, never a Windows console window.
//!
//! **Suppression is declared by the entry point, not applied to every helper.**
//! A helper started `CREATE_NO_WINDOW` has no console at all, and a console
//! program that would have inherited one loses its output encoding with it:
//! `cmd` takes its own from the attached console, so a windowless `cmd` answers
//! in the machine's OEM code page instead. Measured on a console whose page is
//! UTF-8, `cmd /C cd` for a path holding `\u{4e2d}` returned the OEM bytes
//! `D6 D0` with the flag and the console's UTF-8 bytes without it. On a runner
//! whose OEM page is 437 the same call returned `?`, because that page cannot
//! hold the character at all -- and it passed on a machine whose OEM page is
//! 936, which can. Nothing about a child says which it needs, and redirected
//! stdio is not the same thing as having no console, so the fact belongs to the
//! process the parent is.
//!
//! A process that already has no console -- the tray -- is the case the
//! suppression is for, and there the two agree: measured, `cmd` and `chcp.com`
//! both reported OEM page 936 with no console present, so the agent's decoder
//! and the command it reads still name the same page.

use std::sync::atomic::{AtomicBool, Ordering};

/// Set by an entry point that has no console of its own.
static NO_CONSOLE: AtomicBool = AtomicBool::new(false);

/// Declare that this process has no console, so helpers it starts must not
/// create one.
///
/// The Windows tray is the caller: it is a GUI-subsystem program, and a helper
/// Windows would give a console to would get a visible window with it. Call
/// this before the first helper starts. It is not reversible, because a process
/// cannot acquire a console by asking for one.
///
/// On platforms without a console concept this does nothing.
pub fn declare_no_console() {
    NO_CONSOLE.store(true, Ordering::SeqCst);
}

/// Build a command for a program whose output is read through pipes, without
/// allocating a Windows console window when this process has none of its own.
///
/// Arguments, stdio, cwd and lifetime remain the caller's. A process that does
/// have a console passes it on unchanged, which is what keeps `cmd` reporting in
/// the console's code page. GUI programs can still show their own windows.
pub fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let command = std::process::Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        if NO_CONSOLE.load(Ordering::SeqCst) {
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        command
    };
    command
}

#[cfg(all(test, windows))]
mod tests {
    /// Whether a console program started this way ends up on **this** process's
    /// console, asked from inside it.
    ///
    /// `GetConsoleProcessList` lists the processes attached to the caller's
    /// console, and membership is the observable that separates the two cases
    /// this module is about: an inheriting child is attached to the console the
    /// test process is on, while `CREATE_NO_WINDOW` gives it a console of its
    /// own, where this pid does not appear. Membership rather than a count,
    /// because other tests in the suite start processes on the same console and
    /// a count taken twice is not the same measurement twice.
    ///
    /// An earlier version of this test compared the console's code page and
    /// **passed against the bug**, on a machine whose console and OEM pages are
    /// both 936: a check whose subject and control agree for an unrelated reason
    /// proves nothing. This one was mutation-checked against the old blanket
    /// flag and failed as it should.
    fn joined_this_console(mut command: std::process::Command) -> bool {
        let script = r#"Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class C { [DllImport("kernel32.dll", SetLastError=true)] public static extern uint GetConsoleProcessList(uint[] p, uint n); }'; $b=New-Object uint32[] 64; $n=[C]::GetConsoleProcessList($b,64); [Console]::Write(($b[0..([Math]::Max($n,1)-1)] -join ','))"#;
        let output = command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .expect("probe process");
        assert!(output.status.success(), "{output:?}");
        let mine = std::process::id();
        String::from_utf8_lossy(&output.stdout)
            .split(',')
            .any(|pid| pid.trim().parse::<u32>() == Ok(mine))
    }

    /// **A helper must inherit this process's console, not be given one.**
    ///
    /// A console program takes its output encoding from the console it is
    /// attached to, so a helper started `CREATE_NO_WINDOW` has none to read and
    /// answers in the machine's OEM code page. Measured, that turned a path
    /// holding `\u{4e2d}` into a literal `?` on `windows-latest`, whose OEM page
    /// is 437, while the same call passed where the OEM page is 936.
    ///
    /// The control is the bug itself, run deliberately, so the assertion cannot
    /// pass because both arms measure the same thing.
    ///
    /// The harness without a console is skipped: a plain spawn there would
    /// allocate a window this test must not create, and the two cases would
    /// coincide.
    #[test]
    fn a_helper_inherits_this_process_console_when_it_has_one() {
        use std::os::windows::process::CommandExt;
        if std::fs::File::open("CONOUT$").is_err() {
            eprintln!("no console in this harness; inheritance is untestable here");
            return;
        }
        assert!(
            joined_this_console(std::process::Command::new("powershell.exe")),
            "a plain spawn is not on this console, so the comparison below \
             would not mean inheritance"
        );
        let mut windowless = std::process::Command::new("powershell.exe");
        windowless.creation_flags(0x08000000); // CREATE_NO_WINDOW: the old rule
        assert!(
            !joined_this_console(windowless),
            "the control is broken: a deliberately windowless process is on this \
             console, so the flag would not be observable"
        );
        assert!(
            joined_this_console(super::background_command("powershell.exe")),
            "background_command did not pass this process's console on to its \
             helper, so a console program answers in the machine's OEM code page \
             instead of the console's"
        );
    }
}
