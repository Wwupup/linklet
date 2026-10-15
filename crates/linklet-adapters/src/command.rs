//! Processes whose interaction is through pipes, never a Windows console window.

/// Build a command without allocating a Windows console. GUI programs can still
/// show their own windows. Arguments, stdio, cwd and lifetime remain the caller's.
pub fn background_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let command = std::process::Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        command
    };
    command
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn a_background_console_program_has_no_console_and_keeps_its_output() {
        let script = r#"Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class ConsoleProbe { [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow(); }'; [Console]::Write([ConsoleProbe]::GetConsoleWindow().ToInt64())"#;
        let output = super::background_command("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .output()
            .expect("probe process");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "0",
            "a console was allocated"
        );
    }
}
