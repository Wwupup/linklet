//! The desktop entry point must not allocate a console on Windows.
#[cfg(windows)]
#[test]
fn tray_is_a_gui_executable_and_agent_remains_a_console_executable() {
    for (path, subsystem) in [
        (env!("CARGO_BIN_EXE_linklet-tray"), 2_u16),
        (env!("CARGO_BIN_EXE_linklet-agent"), 3_u16),
    ] {
        let bytes = std::fs::read(path).expect("built executable");
        let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().expect("PE offset")) as usize;
        assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
        let offset = pe + 24 + 68;
        let actual = u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("subsystem"));
        assert_eq!(actual, subsystem, "wrong Windows subsystem for {path}");
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires an interactive Windows desktop with a notification area"]
fn desktop_tray_starts_the_shared_service_and_excludes_a_console_instance() {
    use std::net::{TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    struct Process(Child, std::path::PathBuf);
    impl Drop for Process {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let directory =
        std::env::temp_dir().join(format!("linklet-tray-desktop-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("scratch");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("port");
    let address = reservation.local_addr().expect("address");
    drop(reservation);
    let mut process = Process(
        Command::new(env!("CARGO_BIN_EXE_linklet-tray"))
            .args(["--port", &address.port().to_string(), "--no-log"])
            .env("LOCALAPPDATA", &directory)
            .env_remove("LINKLET_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("tray process"),
        directory.clone(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let stream = loop {
        assert!(
            process.0.try_wait().expect("status").is_none(),
            "tray exited before binding"
        );
        if let Ok(stream) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            break stream;
        }
        assert!(Instant::now() < deadline, "tray did not start its service");
        std::thread::sleep(Duration::from_millis(25));
    };
    let mut connection = linklet_adapters::Connection::with_budget(stream, Duration::from_secs(3));
    connection
        .write_frame(linklet_core::frame::Kind::Hello, b"{}")
        .expect("hello");
    connection
        .read_frame(linklet_core::frame::Kind::Hello)
        .expect("service answers invalid hello");
    let duplicate = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
        .args(["--port", "0", "--no-log"])
        .env("LOCALAPPDATA", &directory)
        .env_remove("LINKLET_LOG")
        .output()
        .expect("console instance");
    assert_eq!(duplicate.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("running"));
}
