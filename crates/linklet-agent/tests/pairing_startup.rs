//! Automatic credentials persist across process restarts.
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn first_start_generates_a_code_and_restart_keeps_it() {
    let home = std::env::temp_dir().join(format!("linklet-pair-start-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("scratch directory");
    let start = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
            .args(["--port", "0", "--no-log"])
            .env_remove("LINKLET_TOKEN")
            .env_remove("LINKLET_TOKEN_FILE")
            .env("LOCALAPPDATA", &home)
            .env("XDG_CONFIG_HOME", &home)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start");
        let mut reader = BufReader::new(child.stdout.take().expect("stdout"));
        let mut banner = String::new();
        reader.read_line(&mut banner).expect("banner");
        let mut code = String::new();
        reader.read_line(&mut code).expect("code");
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            banner.contains("listening on"),
            "expected automatic startup: {banner}"
        );
        let code = code
            .trim()
            .strip_prefix("Pairing code: ")
            .expect("pairing code")
            .to_string();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        code
    };
    let first = start();
    assert_eq!(first, start());
    std::fs::remove_dir_all(home).expect("cleanup");
}
