//! Windows tray entry point for the target agent.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod tray_windows;

#[cfg(windows)]
fn main() {
    // A GUI-subsystem program has no console, and a helper Windows would give
    // one to gets a visible window with it. This entry point is the only place
    // that is true; the console agent passes its console on to every helper, so
    // that `cmd` and the decoder that reads it agree on the code page.
    linklet_adapters::declare_no_console();
    tray_windows::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("linklet-tray is available on Windows; use linklet-agent on this platform");
    std::process::exit(2);
}
