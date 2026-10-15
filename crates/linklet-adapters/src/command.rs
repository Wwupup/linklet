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
//!
//! `crates/linklet-adapters/tests/console.rs` starts a helper and asks which
//! console it landed on. That check is an adapter test rather than one beside
//! this code, because it starts real processes and `docs/testing.md` keeps that
//! out of the unit layer.

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
