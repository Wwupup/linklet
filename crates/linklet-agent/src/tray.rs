//! Windows tray entry point for the target agent.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod tray_windows;

#[cfg(windows)]
fn main() {
    tray_windows::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("linklet-tray is available on Windows; use linklet-agent on this platform");
    std::process::exit(2);
}
