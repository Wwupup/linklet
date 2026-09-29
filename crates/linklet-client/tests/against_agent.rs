//! The client, driven against a real agent.
//!
//! This is where the two halves of M6 meet: a real client process talking over a
//! real socket to a real agent process that really runs a command. Everything
//! below it has been tested against fakes or against one end at a time; this file
//! is the only place the whole thing is exercised together, and it is the test
//! that would have caught a protocol defined twice.
//!
//! The agent is spawned from this crate's test, so `linklet-client` depends on
//! `linklet-agent` being buildable. That is deliberate: a client and a server
//! that cannot be tested against each other are two projects.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use linklet_client::{AgentAddress, CallError, identity, render_call_error, run};
use linklet_core::auth::Token;
use linklet_core::wire::{self, RunRequest};

/// Where the agent binary is.
///
/// `CARGO_BIN_EXE_linklet-agent` does not exist: that variable is defined only
/// for a binary in the package being compiled, and this package has no binary.
/// So the path is worked out instead, and the two ways it can be wrong -- a moved
/// repository and a missing build -- get one message each rather than a spawn
/// failure that reads like a protocol problem.
fn agent_binary() -> PathBuf {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate directory has a repository root two levels up")
        .to_path_buf();

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => repository.join("target"),
    };

    // The profile directory is `debug` for `cargo test`, and `CARGO_BIN_EXE_*`
    // would have handled a release profile. This one does not try: a release
    // build of the tests is not a thing anyone does here, and guessing would
    // trade a clear failure for a confusing one.
    let path = target.join("debug/linklet-agent.exe");
    assert!(
        path.is_file(),
        "the agent binary is not at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

/// The token these tests configure the agent with.
const TEST_TOKEN: &str = "test-token-0123456789";

/// A running agent, killed when the test ends.
struct Agent {
    child: Child,
    address: AgentAddress,
}

impl Agent {
    fn start() -> Self {
        let mut child = Command::new(agent_binary())
            .env("LINKLET_TOKEN", TEST_TOKEN)
            .args(["--port", "0"])
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
        // Kept alive so the child's stdout is not closed under it.
        std::mem::forget(reader);

        let port = banner
            .trim()
            .rsplit(':')
            .next()
            .and_then(|text| text.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("cannot read a port from {banner:?}"));

        let address = AgentAddress::new(format!("127.0.0.1:{port}"))
            .expect("the banner port is a valid address")
            .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));

        Self { child, address }
    }

    fn run(&self, command: &str) -> wire::RunOutcome {
        run(
            &self.address,
            &RunRequest {
                command: command.to_string(),
                timeout_seconds: 30,
            },
        )
        .unwrap_or_else(|e| panic!("the call should have succeeded: {e}"))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- the shape of an address -------------------------------------------------

#[test]
fn an_address_needs_a_host_and_a_number() {
    assert!(AgentAddress::new("127.0.0.1:8787").is_ok());
    assert!(AgentAddress::new("box:1").is_ok());

    let cases = ["127.0.0.1", "", ":8787", "127.0.0.1:", "127.0.0.1:http"];
    for text in cases {
        let error = AgentAddress::new(text).expect_err("should be refused");
        assert!(
            matches!(error, CallError::BadAddress(_)),
            "{text:?} gave {error:?}"
        );
    }
}

// --- the end-to-end trip -----------------------------------------------------

#[test]
fn identity_answers_over_a_real_socket() {
    let agent = Agent::start();
    let name = identity(&agent.address).expect("the agent should answer");
    assert_eq!(name, "linklet-agent");
}

#[test]
fn a_command_runs_and_its_output_comes_back() {
    let agent = Agent::start();
    let outcome = agent.run("echo end-to-end");

    assert_eq!(outcome.exit_code, Some(0), "{outcome:#?}");
    assert!(
        outcome.stdout.contains("end-to-end"),
        "the output should have crossed two processes: {outcome:#?}"
    );
    assert!(
        outcome.duration_ms < 30_000,
        "the reported duration should be a real measurement: {outcome:#?}"
    );
}

#[test]
fn a_command_that_fails_comes_back_as_an_outcome_and_not_an_error() {
    // The distinction the protocol was built around, now measured across the
    // whole chain: `run` returns `Ok`, because the command ran.
    let agent = Agent::start();
    let outcome = agent.run("exit 7");
    assert_eq!(outcome.exit_code, Some(7), "{outcome:#?}");
}

#[test]
fn a_killed_command_says_which_failure_it_was() {
    let agent = Agent::start();
    let outcome = run(
        &agent.address,
        &RunRequest {
            command: "ping -n 30 127.0.0.1".to_string(),
            timeout_seconds: 1,
        },
    )
    .expect("the call itself succeeded");

    assert_eq!(outcome.exit_code, None, "{outcome:#?}");
    assert_eq!(outcome.reason.as_deref(), Some(wire::KILLED_BY_DEADLINE));
}

#[test]
fn an_error_the_agent_reports_does_not_look_like_a_command_result() {
    // The other half of the distinction, from the client's side: a refused
    // request is a `CallError`, so there is no way to read an exit code that was
    // never produced.
    let agent = Agent::start();
    let result = run(
        &agent.address,
        &RunRequest {
            // Beyond the protocol's own limit, so the agent refuses before
            // anything runs.
            command: "echo x".to_string(),
            timeout_seconds: wire::MAX_TIMEOUT_SECONDS + 1,
        },
    );

    let error = result.expect_err("the agent should refuse this");
    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    assert!(
        render_call_error(&error).contains("refused"),
        "the rendered text should say the agent refused: {}",
        render_call_error(&error)
    );
}

// --- what the client says when there is no agent ----------------------------

#[test]
fn no_agent_at_the_address_is_a_transport_error_and_not_a_result() {
    // The case every caller meets first. The rendered text starts with the fact
    // that decides what to do next, because a reader must not have to work out
    // from prose whether the command ran.
    // With a token, so that the call gets as far as the network. Without one it is
    // refused before connecting, which is a different test and a better error.
    let address = AgentAddress::new("127.0.0.1:1")
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));
    let error = run(
        &address,
        &RunRequest {
            command: "echo x".to_string(),
            timeout_seconds: 5,
        },
    )
    .expect_err("nothing is listening on port 1");

    assert!(matches!(error, CallError::Transport(_)), "{error:?}");
    let text = render_call_error(&error);
    assert!(text.starts_with("could not reach the agent"), "{text}");
}

#[test]
fn an_address_that_cannot_be_resolved_is_named() {
    let address = AgentAddress::new("no-such-host.invalid:8787")
        .expect("shape is fine")
        .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));
    let error = run(
        &address,
        &RunRequest {
            command: "echo x".to_string(),
            timeout_seconds: 5,
        },
    )
    .expect_err("that name cannot resolve");

    let text = render_call_error(&error);
    assert!(
        text.contains("no-such-host.invalid"),
        "the text should name what could not be resolved: {text}"
    );
}

#[test]
fn the_client_gives_up_later_than_the_command_deadline() {
    // The agent enforces the deadline and describes what it killed. A client that
    // used exactly the command's deadline would report a transport failure for a
    // command the agent was about to report properly -- a lie about what
    // happened, and the one this allowance exists to prevent.
    let agent = Agent::start();
    let started = std::time::Instant::now();
    let outcome = run(
        &agent.address,
        &RunRequest {
            command: "ping -n 30 127.0.0.1".to_string(),
            timeout_seconds: 1,
        },
    )
    .expect("the call should succeed");

    assert_eq!(outcome.reason.as_deref(), Some(wire::KILLED_BY_DEADLINE));
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "the client waited {:?}, which is past its allowance",
        started.elapsed()
    );
}

#[test]
fn a_host_with_the_wrong_token_is_refused_and_the_command_never_runs() {
    // The end of the chain, from the caller's side: a secret that does not match
    // is a call that could not be made, and the message says the token was missing
    // or wrong rather than leaving the caller to guess.
    let agent = Agent::start();
    let wrong = AgentAddress::new(agent.address.text.clone())
        .expect("the same address")
        .with_token(Token::new("wrong-token-0123456789").expect("a usable test token"));

    let error = run(
        &wrong,
        &RunRequest {
            command: "echo should-not-run".to_string(),
            timeout_seconds: 5,
        },
    )
    .expect_err("the token does not match");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    let text = render_call_error(&error);
    assert!(text.contains("401"), "{text}");
    assert!(text.contains("missing or wrong"), "{text}");
}

#[test]
fn a_host_with_no_token_against_a_secured_agent_is_refused_too() {
    // The other half: an agent configured with a token refuses a caller who has
    // none, which is the case a deployment hits when the environment variable is
    // set on one side only.
    //
    // Where the refusal happens changed when the channel became sealed, and for the
    // better. It used to be the agent answering 401 after a round trip; now the
    // client refuses before it opens a socket, because a sealed call with no token
    // cannot be made at all -- the token is what authenticates the handshake. The
    // caller gets a sentence saying so instead of a status code to interpret.
    let agent = Agent::start();
    let no_token = AgentAddress::new(agent.address.text.clone()).expect("the same address");

    let error = run(
        &no_token,
        &RunRequest {
            command: "echo nope".to_string(),
            timeout_seconds: 5,
        },
    )
    .expect_err("no token");

    assert!(matches!(error, CallError::BadAddress(_)), "{error:?}");
    let text = render_call_error(&error);
    assert!(text.contains("token"), "{text}");
}
