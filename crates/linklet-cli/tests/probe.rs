//! The probe as a process, because **the exit codes are the interface**.
//!
//! A supervisor is a script that reads a number and decides whether to kill something, so the
//! numbers are a contract in a way that most output is not: `1` means start it, `4` means kill
//! it and start it, and getting those backwards produces a supervisor that either restarts
//! healthy agents forever or never restarts a dead one. `crates/linklet-client/tests/probe.rs`
//! covers what the probe decides; this covers what a shell is told about it.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// The token the tests configure the agent with.
const TEST_TOKEN: &str = "probe-cli-token-0123456789";

/// The code the probe uses for "something is listening and did not answer".
///
/// Written out rather than imported so that a change to it fails this file, which is the point:
/// a supervisor in the wild has this number in it.
const NO_ANSWER: i32 = 4;

fn agent_binary() -> PathBuf {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate directory has a repository root two levels up")
        .to_path_buf();

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => repository.join("target"),
    };

    let path = target.join(format!(
        "debug/linklet-agent{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        path.is_file(),
        "the agent binary is not at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-probe-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A running agent, killed when the test ends.
struct Agent {
    child: Child,
    port: u16,
    root: PathBuf,
}

impl Agent {
    fn start() -> Self {
        let root = scratch_dir();

        let mut child = Command::new(agent_binary())
            .env("LINKLET_TOKEN", TEST_TOKEN)
            .env_remove("LINKLET_TOKEN_FILE")
            .arg("--port")
            .arg("0")
            .current_dir(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the agent should start");

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut reader = BufReader::new(stdout);
        let mut banner = String::new();
        reader
            .read_line(&mut banner)
            .expect("the agent prints a banner when it is listening");
        std::mem::forget(reader);

        let port = banner
            .split_whitespace()
            .find_map(|word| word.strip_prefix("0.0.0.0:"))
            .and_then(|text| text.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("cannot read a port from {banner:?}"));

        Self { child, port, root }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Runs `linklet probe` with a token in the environment, as a supervisor would.
fn probe(agent: &str, token: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_linklet"))
        .env("LINKLET_TOKEN", token)
        .env_remove("LINKLET_TOKEN_FILE")
        .arg("probe")
        .arg("--agent")
        .arg(agent)
        .output()
        .expect("the tool should run")
}

/// A port with nothing listening on it.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("a bound listener").port();
    drop(listener);
    port
}

#[test]
fn a_working_agent_exits_zero_and_says_it_answered() {
    let agent = Agent::start();

    let output = probe(&format!("127.0.0.1:{}", agent.port), TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("answered"), "{stdout}");
    assert!(stdout.contains("linklet-agent"), "{stdout}");
    assert!(stdout.contains(env!("CARGO_PKG_VERSION")), "{stdout}");
}

#[test]
fn nothing_listening_exits_one_because_a_supervisor_starts_it() {
    let output = probe(&format!("127.0.0.1:{}", free_port()), TEST_TOKEN);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("nothing listening"), "{stdout}");
}

#[test]
fn a_socket_that_never_answers_exits_four_because_a_supervisor_kills_it_first() {
    // **The code that makes a supervisor worth having**, and the one it could not get from
    // `check`: this listener accepts connections and never speaks, so `check` calls it alive and
    // the probe has to call it wedged.
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("a bound listener").port();

    let output = probe(&format!("127.0.0.1:{port}"), TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(NO_ANSWER),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("no answer"), "{stdout}");
    drop(listener);
}

#[test]
fn a_wrong_token_exits_three() {
    // The rule that keeps a supervisor from restarting healthy processes: liveness is not
    // authorization. An operator holding the wrong secret would otherwise have their agent
    // restarted in a loop, which fixes nothing and hides the real problem.
    let agent = Agent::start();

    let output = probe(
        &format!("127.0.0.1:{}", agent.port),
        "a-different-secret-000",
    );

    assert_eq!(
        output.status.code(),
        Some(3),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("refused"), "{stdout}");
}

#[test]
fn a_spec_nobody_can_read_exits_two_and_not_one() {
    // A typo in a supervisor's config must not read as an outage, or the script restarts the
    // agent every time somebody edits it.
    let output = probe("not-an-address", TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn local_token_configuration_errors_exit_two() {
    let output = Command::new(env!("CARGO_BIN_EXE_linklet"))
        .args(["probe", "--agent", "127.0.0.1:1"])
        .env_remove("LINKLET_TOKEN")
        .env("LINKLET_TOKEN_FILE", "missing-token-file-for-probe")
        .output()
        .expect("probe");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot read"));
}

#[test]
fn handshake_version_rejection_and_invalid_reply_have_distinct_codes() {
    use linklet_adapters::Connection;
    use linklet_core::{frame::Kind, wire};
    for (payload, code) in [
        (
            wire::reply_result(
                serde_json::json!({"protocol":1,"ephemeral_public":"00".repeat(32)}),
            )
            .to_string(),
            3,
        ),
        ("not-json".to_string(), 4),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let peer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("connection");
            let mut connection = Connection::with_budget(stream, std::time::Duration::from_secs(3));
            connection.read_frame(Kind::Hello).expect("hello");
            connection
                .write_frame(Kind::Hello, payload.as_bytes())
                .expect("reply");
        });
        let output = probe(&format!("127.0.0.1:{port}"), TEST_TOKEN);
        peer.join().expect("peer finished");
        assert_eq!(output.status.code(), Some(code), "{output:?}");
    }
}
