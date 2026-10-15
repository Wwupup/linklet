//! Which console a helper is started on, told from inside it.
//!
//! This is an adapter test rather than a check beside `background_command`
//! because it starts real processes, and `docs/testing.md` keeps that out of the
//! unit layer: the unit tests share a binary with the code-page decoder tests,
//! whose PowerShell subprocesses have a five-second deadline, and three more
//! spawns in that binary pushed them past it on a loaded runner.
//!
//! **What is being pinned.** A console program takes its own output encoding
//! from the console it is attached to. A helper started `CREATE_NO_WINDOW` has
//! no console to read, so it answers in the machine's OEM code page instead:
//! measured, `cmd /C cd` for a path holding `\u{4e2d}` returned the OEM bytes
//! `D6 D0` there and the console's UTF-8 bytes without it. On `windows-latest`,
//! whose OEM page is 437 and cannot represent the character, that came back as a
//! literal `?` -- while the same test passed on a machine whose OEM page is 936.
//! The rule is therefore that `background_command` passes this process's console
//! on, and only a process with no console of its own asks for the window to be
//! suppressed.
//!
//! **The control is the bug itself, run deliberately.** `GetConsoleProcessList`
//! names every process attached to the caller's console: an inheriting child
//! joins the list this process is on, while `CREATE_NO_WINDOW` gives the child a
//! console of its own, where this pid never appears. Membership rather than a
//! count, because other tests start processes on the same console and a count
//! taken twice is not the same measurement twice. An earlier version compared
//! the console's code page instead and passed against the bug here, where the
//! console and OEM pages are both 936.

#![cfg(windows)]

use std::process::Command;

/// Whether a console program started this way ends up on **this** process's
/// console.
fn joined_this_console(mut command: Command) -> bool {
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

#[test]
fn a_helper_inherits_this_process_console_when_it_has_one() {
    use std::os::windows::process::CommandExt;
    if std::fs::File::open("CONOUT$").is_err() {
        eprintln!("no console in this harness; inheritance is untestable here");
        return;
    }
    assert!(
        joined_this_console(std::process::Command::new("powershell.exe")),
        "a plain spawn is not on this console, so the comparison below would not \
         mean inheritance"
    );
    let mut windowless = std::process::Command::new("powershell.exe");
    windowless.creation_flags(0x08000000); // CREATE_NO_WINDOW: the rule that broke
    assert!(
        !joined_this_console(windowless),
        "the control is broken: a deliberately windowless process is on this \
         console, so the flag would not be observable"
    );
    assert!(
        joined_this_console(linklet_adapters::background_command("powershell.exe")),
        "background_command did not pass this process's console on to its helper, \
         so a console program answers in the machine's OEM code page instead of \
         the console's"
    );
}
