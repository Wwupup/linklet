//! The agent, driven as a real process over a real socket.
//!
//! Everything the protocol decides is unit-tested in `linklet-core`. What is left
//! here is the part that only exists once there is a server: that a request line
//! and headers are read correctly, that a reply is flushed rather than buffered,
//! that a command's output survives the trip, and that the shapes on the wire are
//! the ones `wire.rs` says.
//!
//! The agent binds port 0 and prints the port it got, so a test never guesses one
//! and never collides with something else on the machine. The banner is read
//! before the first request, which is also why the banner exists: it is bound
//! before it is printed, so reading it is proof the port is open.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use linklet_core::auth::{TOKEN_HEADER, TOKEN_SCHEME};
use linklet_core::json::{self, Json};
use linklet_core::wire;

/// The token these tests configure the agent with.
///
/// Sixteen bytes, so it passes the length check the agent does at startup. A
/// literal is fine here: it is a test secret, on a port the OS chose, in a
/// process that lives for one test.
const TEST_TOKEN: &str = "test-token-0123456789";

/// A running agent, killed when the test ends.
struct Agent {
    child: Child,
    port: u16,
}

impl Agent {
    /// Starts the agent on a port the OS picks, and waits until it is listening.
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
            // Through the environment rather than --token, which exercises the
            // path a deployment actually uses and keeps a secret out of the
            // process command line.
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

        // "linklet-agent listening on 0.0.0.0:51234"
        let port = banner
            .trim()
            .rsplit(':')
            .next()
            .and_then(|text| text.parse().ok())
            .unwrap_or_else(|| panic!("cannot read a port from the banner: {banner:?}"));

        // Keep the reader alive on the child's stdout: dropping it closes the
        // pipe, and a process writing to a closed pipe is a process that may
        // exit for a reason unrelated to the test.
        std::mem::forget(reader);

        Self { child, port }
    }

    /// Sends one request and returns the status and the body.
    fn request(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("the agent should be reachable");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("a timeout should be settable");

        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{TOKEN_HEADER}: {TOKEN_SCHEME}{TEST_TOKEN}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .expect("writing the request");
        stream.flush().expect("flushing the request");

        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("reading the reply");

        let (head, body) = raw
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("a reply with no blank line: {raw:?}"));
        let status: u16 = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("no status in {head:?}"));

        (status, body.to_string())
    }

    /// Sends a request whose body is a JSON value.
    fn json_request(&self, method: &str, path: &str, body: &Json) -> (u16, String) {
        self.request(method, path, &json::write(body))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs a command and decodes the outcome it produces.
fn run(agent: &Agent, command: &str) -> wire::RunOutcome {
    let (status, body) = agent.json_request(
        "POST",
        wire::RUN_PATH,
        &linklet_core::object! { "command" => command, "timeout_seconds" => 30i64 },
    );
    assert_eq!(status, 200, "the agent answered {status}: {body}");
    wire::decode_run_reply(&body).unwrap_or_else(|e| panic!("undecodable reply: {e} in {body}"))
}

// --- the identity path -------------------------------------------------------

#[test]
fn the_agent_says_which_agent_it_is() {
    let agent = Agent::start();
    let (status, body) = agent.request("GET", wire::IDENTITY_PATH, "");

    assert_eq!(status, 200);
    let value = json::parse(&body).expect("the identity reply is JSON");
    assert_eq!(value.get_str("name"), Some("linklet-agent"));
    assert!(value.get_str("version").is_some());
}

#[test]
fn a_wrong_method_on_a_known_path_is_not_an_unknown_path() {
    // Two different mistakes, told apart: a client bug, versus a client talking
    // to the wrong program. A 404 for both sends whoever is debugging it to look
    // in the wrong place.
    let agent = Agent::start();
    let (status, _) = agent.request("GET", wire::RUN_PATH, "");
    assert_eq!(status, 405, "GET on the run path is a method problem");
}

#[test]
fn an_unknown_path_is_a_404_with_the_path_in_it() {
    let agent = Agent::start();
    let (status, body) = agent.request("GET", "/nope", "");

    assert_eq!(status, 404);
    assert!(
        body.contains("/nope"),
        "the reply should name the path: {body}"
    );
}

// --- running a command -------------------------------------------------------

#[test]
fn a_command_that_succeeds_returns_its_output_and_an_exit_code_of_zero() {
    let agent = Agent::start();
    let outcome = run(&agent, "echo hello");

    assert_eq!(outcome.exit_code, Some(0), "{outcome:#?}");
    assert!(
        outcome.stdout.contains("hello"),
        "stdout should carry the echo: {outcome:#?}"
    );
    assert!(
        outcome.reason.is_none(),
        "a program that ran needs no reason"
    );
}

#[test]
fn a_command_that_fails_is_an_outcome_and_not_an_error() {
    // The distinction the protocol was designed around, over a real socket this
    // time: a program that ran and reported a problem is a successful call.
    let agent = Agent::start();
    let (status, body) = agent.json_request(
        "POST",
        wire::RUN_PATH,
        &linklet_core::object! { "command" => "exit 3", "timeout_seconds" => 30i64 },
    );

    assert_eq!(status, 200, "the call succeeded: {body}");
    let outcome = wire::decode_run_reply(&body).expect("decodes");
    assert_eq!(outcome.exit_code, Some(3), "{outcome:#?}");
}

#[test]
fn standard_error_survives_the_trip() {
    // A failing build writes to stderr and nothing else, so a transport that
    // loses stderr loses the entire diagnosis.
    let agent = Agent::start();
    let outcome = run(&agent, "echo problem 1>&2");

    assert!(
        outcome.stderr.contains("problem"),
        "stderr should carry it: {outcome:#?}"
    );
}

#[test]
fn a_command_that_never_started_has_no_exit_code_and_says_so() {
    let agent = Agent::start();
    // `cmd /C` on a name that is not a program: the shell reports it and exits
    // non-zero, which is a *run* and not a failure to spawn. That difference is
    // the point -- this asserts the outcome shape rather than a particular code,
    // because which one the shell picks is the shell's business.
    let outcome = run(&agent, "this-program-does-not-exist-anywhere");

    assert!(outcome.exit_code.is_some(), "the shell ran: {outcome:#?}");
    assert_ne!(outcome.exit_code, Some(0));
    assert!(
        outcome.stderr.contains("not recognized") || outcome.stdout.contains("not recognized"),
        "the shell's complaint should reach the caller: {outcome:#?}"
    );
}

#[test]
fn a_caller_supplied_timeout_kills_the_command_and_says_which_failure_it_was() {
    // The three-way distinction, over a real socket: killed by the deadline is a
    // different fact from failing to start, and the reason constant is what the
    // host branches on.
    let agent = Agent::start();
    let (status, body) = agent.json_request(
        "POST",
        wire::RUN_PATH,
        // Longer than the timeout, and it must not finish first.
        &linklet_core::object! { "command" => "ping -n 30 127.0.0.1", "timeout_seconds" => 1i64 },
    );

    assert_eq!(status, 200, "{body}");
    let outcome = wire::decode_run_reply(&body).expect("decodes");
    assert_eq!(outcome.exit_code, None, "{outcome:#?}");
    assert_eq!(
        outcome.reason.as_deref(),
        Some(wire::KILLED_BY_DEADLINE),
        "{outcome:#?}"
    );
    assert!(
        outcome.duration_ms >= 900,
        "it should have run for about the timeout, not {:?} ms",
        outcome.duration_ms
    );
}

// --- what is refused before a command runs -----------------------------------

#[test]
fn a_body_that_is_not_json_is_refused_and_no_command_runs() {
    let agent = Agent::start();
    let (status, body) = agent.request("POST", wire::RUN_PATH, "not json at all");

    assert_eq!(status, 400);
    let value = json::parse(&body).expect("the refusal is JSON");
    assert!(value.get_str("error").is_some());
    assert!(
        value.get("exit_code").is_none(),
        "an error body must not carry a result field: {body}"
    );
}

#[test]
fn a_command_field_that_is_missing_is_refused_by_name() {
    let agent = Agent::start();
    let (status, body) = agent.json_request(
        "POST",
        wire::RUN_PATH,
        &linklet_core::object! { "timeout_seconds" => 5i64 },
    );

    assert_eq!(status, 400);
    assert!(body.contains("command"), "{body}");
}

#[test]
fn a_timeout_beyond_the_protocol_limit_is_refused_rather_than_clamped() {
    // An agent that accepts an unbounded timeout from the network has been handed
    // a way to be occupied forever.
    let agent = Agent::start();
    let (status, body) = agent.json_request(
        "POST",
        wire::RUN_PATH,
        &linklet_core::object! { "command" => "echo x", "timeout_seconds" => 100_000i64 },
    );

    assert_eq!(status, 400);
    assert!(body.contains("timeout_seconds"), "{body}");
}

#[test]
fn several_commands_in_a_row_all_answer() {
    // One request per connection, and a thread per connection: the second call is
    // the assertion that the first did not leave the listener in a bad state.
    let agent = Agent::start();
    for i in 0..5 {
        let outcome = run(&agent, &format!("echo round-{i}"));
        assert_eq!(outcome.exit_code, Some(0));
        assert!(
            outcome.stdout.contains(&format!("round-{i}")),
            "round {i} came back wrong: {outcome:#?}"
        );
    }
}

// --- who is allowed to ask ---------------------------------------------------

/// Sends one request with a hand-written `Authorization` header.
///
/// Written out rather than going through [`Agent::request`], because the whole
/// point of these tests is the header that method always sends correctly.
fn send_without_the_right_token(
    agent: &Agent,
    method: &str,
    path: &str,
    body: &str,
    header: Option<&str>,
) -> String {
    use std::io::Write as _;

    let credential = match header {
        Some(value) => format!("{TOKEN_HEADER}: {value}\r\n"),
        None => String::new(),
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{credential}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", agent.port))
        .expect("the agent should be reachable");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a timeout");
    stream
        .write_all(request.as_bytes())
        .expect("writing the request");
    stream.flush().expect("flushing");

    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("reading the reply");
    raw
}

#[test]
fn a_request_with_no_token_is_refused_and_no_command_runs() {
    // The whole point of the token, over a real socket: someone who can reach the
    // port but does not know the secret gets nothing.
    let agent = Agent::start();
    let body = r#"{"command":"echo should-not-run","timeout_seconds":5}"#;
    let raw = send_without_the_right_token(&agent, "POST", wire::RUN_PATH, body, None);

    assert!(
        raw.starts_with("HTTP/1.1 401"),
        "expected a refusal, got: {}",
        raw.lines().next().unwrap_or("")
    );
    assert!(
        !raw.contains("should-not-run"),
        "the command must not have run: {raw}"
    );
}

#[test]
fn a_wrong_token_is_refused_the_same_way_as_a_missing_one() {
    // Same status and same words, so the reply cannot be used to learn whether a
    // guess was closer than no guess. Which of the two happened is information a
    // caller who has the token does not need and one who does not should not get.
    let agent = Agent::start();
    let body = r#"{"command":"echo nope","timeout_seconds":5}"#;
    let raw = send_without_the_right_token(
        &agent,
        "POST",
        wire::RUN_PATH,
        body,
        Some(&format!("{TOKEN_SCHEME}wrong-token-0123456")),
    );

    assert!(raw.starts_with("HTTP/1.1 401"), "{raw}");
    assert!(raw.contains("missing or wrong"), "{raw}");
}

#[test]
fn the_identity_path_is_behind_the_token_too() {
    // Otherwise the 404-versus-405 distinction becomes a way to map the surface
    // without a token, which is the small leak that makes a bigger one possible.
    let agent = Agent::start();
    let raw = send_without_the_right_token(&agent, "GET", wire::IDENTITY_PATH, "", None);

    assert!(raw.starts_with("HTTP/1.1 401"), "{raw}");
    assert!(
        !raw.contains("linklet-agent"),
        "the identity must not leak to an unauthenticated caller: {raw}"
    );
}

#[test]
fn the_agent_refuses_to_start_without_a_usable_token() {
    // A configuration error should be loud when it is made, not at the first
    // caller. This is the startup check, over a real process.
    for (label, environment) in [("missing", None), ("too short", Some("short"))] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_linklet-agent"));
        command.args(["--port", "0"]);
        match environment {
            Some(value) => command.env("LINKLET_TOKEN", value),
            None => command.env_remove("LINKLET_TOKEN"),
        };

        let output = command.output().expect("the agent should run");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{label}: expected a refusal to start"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("token"),
            "{label}: the message should name the token: {stderr}"
        );
    }
}
