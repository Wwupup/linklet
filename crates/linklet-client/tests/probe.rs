//! Asking an agent whether it is working, over a real socket.
//!
//! The distinction this file exists to prove is the one `docs/ROADMAP.md` M10 opened with: a
//! **listening** socket and a **working** agent are not the same thing, and a supervisor built
//! on a bare connect would watch a wedged process forever without restarting it.
//!
//! Two of these four tests need a socket that accepts and says nothing -- which is exactly what
//! a wedged agent looks like from outside -- and neither can be faked with a working agent. So
//! one of them binds a `TcpListener` and never accepts, which is the honest reproduction.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use linklet_client::{AgentAddress, CallError, Liveness, probe};
use linklet_core::auth::Token;

/// The token the tests configure the agent with.
const TEST_TOKEN: &str = "probe-token-0123456789";

/// Where the agent binary is.
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

    let path = target.join("debug/linklet-agent.exe");
    assert!(
        path.is_file(),
        "the agent binary is not at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

/// A directory under the system temporary directory that no other test is using.
fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-probe-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A running agent, killed when the test ends.
struct Agent {
    child: Child,
    address: AgentAddress,
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
            .arg("--root")
            .arg(&root)
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

        let address = AgentAddress::new(format!("127.0.0.1:{port}"))
            .expect("the banner port is a valid address")
            .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));

        Self {
            child,
            address,
            root,
        }
    }

    /// The same address without a token, or with a different one.
    fn address_with(&self, token: Option<&str>) -> AgentAddress {
        let text = self.address.text.clone();
        match token {
            Some(token) => AgentAddress::new(text)
                .expect("the address")
                .with_token(Token::new(token).expect("a usable token")),
            None => AgentAddress::new(text).expect("the address"),
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A port the OS says is free, with nothing listening on it afterwards.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("a bound listener").port();
    drop(listener);
    port
}

#[test]
fn a_working_agent_answers() {
    // The baseline. If this fails, nothing else in the file means anything.
    let agent = Agent::start();

    let liveness = probe(&agent.address, Duration::from_secs(5)).expect("the agent should answer");

    match liveness {
        Liveness::Answered(name) => assert!(
            name.contains("linklet-agent"),
            "the identity should name the agent: {name}"
        ),
        other => panic!("a working agent answers: {other:?}"),
    }
}

#[test]
fn a_port_with_nothing_behind_it_is_not_an_answer() {
    // The case a supervisor restarts. `CallError::Transport` rather than a timeout, because
    // loopback refuses immediately -- what matters is that it is an `Err` at all, since an `Ok`
    // here would have a supervisor leave a dead agent alone forever.
    let address = AgentAddress::new(format!("127.0.0.1:{}", free_port()))
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable token"));

    let outcome = probe(&address, Duration::from_secs(2));

    assert!(
        outcome.is_err(),
        "nothing is listening, so nothing answered: {outcome:?}"
    );
}

#[test]
fn a_socket_that_accepts_and_says_nothing_is_not_a_working_agent() {
    // **The whole reason `probe` exists.** This listener accepts connections into the backlog
    // and never speaks, which is what a wedged agent looks like from outside -- and a bare
    // connect check calls it healthy. A supervisor that used `check` would never restart this.
    //
    // A `TcpListener` that is never `accept`ed is the deterministic version: the kernel
    // completes the handshake on its behalf, so a connect succeeds and nothing else ever will.
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port the OS picks");
    let port = listener.local_addr().expect("a bound listener").port();

    let address = AgentAddress::new(format!("127.0.0.1:{port}"))
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable token"));

    // The connect must succeed, or this test is proving the wrong thing.
    assert!(
        std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}")
                .parse()
                .expect("a socket address"),
            Duration::from_secs(2),
        )
        .is_ok(),
        "the point of this fixture is that a connection IS accepted"
    );

    let started = std::time::Instant::now();
    let outcome = probe(&address, Duration::from_millis(600));

    assert!(
        outcome.is_err(),
        "a socket that never answers is not a working agent: {outcome:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and the probe gives up on its budget rather than hanging: {:?}",
        started.elapsed()
    );
    drop(listener);
}

#[test]
fn a_refusal_is_an_answer_and_not_a_dead_agent() {
    // **The decision that keeps a supervisor from restarting healthy processes.** An agent
    // holding a different secret says no, and saying no proves it is running. `docs/smoke.md`
    // carries the same rule for the exit code.
    let agent = Agent::start();
    let wrong = agent.address_with(Some("a-different-secret-0123456789"));

    let liveness = probe(&wrong, Duration::from_secs(5)).expect("a refusal is an answer");

    match liveness {
        Liveness::Refused(reason) => assert!(
            reason.contains("token"),
            "the refusal should be about the token: {reason}"
        ),
        other => panic!("a wrong token is refused, not answered: {other:?}"),
    }
}

#[test]
fn a_call_with_no_token_at_all_is_a_bad_address_and_not_a_dead_machine() {
    // Which of the four states a missing secret belongs to. It is the script's mistake, and
    // reporting it as "nothing is listening" would have a supervisor starting an agent that is
    // already running.
    let agent = Agent::start();

    let outcome = probe(&agent.address_with(None), Duration::from_secs(2));

    assert!(
        matches!(outcome, Err(CallError::BadAddress(_))),
        "a missing token is the caller's problem: {outcome:?}"
    );
}
