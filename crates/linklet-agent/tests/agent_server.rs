//! The agent, driven as a real process over a real socket.
//!
//! Everything the protocol decides is unit-tested in `linklet-core`. What is left
//! here is the part that only exists once there is a server: that a handshake is
//! answered, that a reply is flushed rather than buffered, that a command's output
//! survives the trip, and that the refusals are the ones `wire.rs` says.
//!
//! The frames are built here with the same connection and channel the host uses,
//! which is deliberate: a test that wrote its own framing would be a second reading
//! of the protocol, and would pass while the two disagreed.
//!
//! The agent binds port 0 and prints the port it got, so a test never guesses one
//! and never collides with something else on the machine. The banner is read
//! before the first request, which is also why the banner exists: it is bound
//! before it is printed, so reading it is proof the port is open.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use linklet_adapters::{Connection, ConnectionError, HkdfChannel};
use linklet_core::channel::{EphemeralPublic, Handshake, Sealed};
use linklet_core::frame::Kind;
use linklet_core::json;
use linklet_core::transfer::{CHUNK_BYTES, Manifest};
use linklet_core::wire::{self, Reply, Request, RunOutcome, RunRequest};

/// One chunk, as a `usize`, for slicing a body into messages.
const CHUNK: usize = CHUNK_BYTES as usize;

/// The token these tests configure the agent with.
///
/// Sixteen bytes, so it passes the length check the agent does at startup. A
/// literal is fine here: it is a test secret, on a port the OS chose, in a
/// process that lives for one test.
const TEST_TOKEN: &str = "test-token-0123456789";

/// A token that is well formed and not the one the agent holds.
const WRONG_TOKEN: &str = "wrong-token-0123456789";

/// A digest of the right shape, for the manifests these tests send.
///
/// Not all zeros: digits have no case, so a digest made of them cannot check that a
/// comparison is case-sensitive -- a mistake made once already in `tests/transfer_paths.rs`.
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A budget for the tests' own reads, long enough that a slow machine never
/// misfires and short enough that a broken agent fails the suite quickly.
const TEST_BUDGET: Duration = Duration::from_secs(30);

/// A running agent, killed when the test ends.
struct Agent {
    child: Child,
    port: u16,
    /// The one directory this agent's transfers may touch, removed with the agent.
    root: std::path::PathBuf,
}

impl Agent {
    /// Starts the agent on a port the OS picks, and waits until it is listening.
    fn start() -> Self {
        Self::start_with(&[])
    }

    /// The same, with extra arguments -- so that a test can ask the agent to keep a log.
    fn start_with(extra: &[&str]) -> Self {
        let root = scratch_dir();
        let mut child = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
            // Through the environment rather than --token, which exercises the
            // path a deployment actually uses and keeps a secret out of the
            // process command line.
            .env("LINKLET_TOKEN", TEST_TOKEN)
            .arg("--port")
            .arg("0")
            .arg("--root")
            .arg(&root)
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the agent should start");

        let stdout = child.stdout.take().expect("stdout was piped");
        let port = read_banner(stdout);

        Self { child, port, root }
    }

    /// Opens a connection and completes a handshake on it with this token.
    fn sealed_with(&self, token: &str) -> Conversation {
        let stream =
            TcpStream::connect(("127.0.0.1", self.port)).expect("the agent should be reachable");
        let mut connection = Connection::with_budget(stream, TEST_BUDGET);

        let (ours, pending) = HkdfChannel
            .propose(token.as_bytes())
            .expect("proposing a handshake");
        let hello = json::write(&wire::handshake_to_json(ours.as_bytes()));
        connection
            .write_frame(Kind::Hello, hello.as_bytes())
            .expect("writing the handshake");

        let frame = connection
            .read_frame(Kind::Hello)
            .expect("the agent answers every hello");
        let theirs = match reply(&frame) {
            Reply::Result(value) => value,
            Reply::Refused(reason) => panic!("the handshake was refused: {reason}"),
        };
        let theirs = wire::handshake_public_from_json(&theirs).expect("an ephemeral_public field");
        let theirs = EphemeralPublic::from_bytes(theirs).expect("32 bytes");

        let session = pending.finish(&theirs).expect("finishing the handshake");
        Conversation {
            connection,
            session,
        }
    }

    /// Opens a connection and completes a handshake with the agent's own token.
    fn sealed(&self) -> Conversation {
        self.sealed_with(TEST_TOKEN)
    }

    /// A bare connection, for the tests that send something the protocol refuses.
    fn stream(&self) -> TcpStream {
        TcpStream::connect(("127.0.0.1", self.port)).expect("the agent should be reachable")
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A directory under the system temporary directory that no other test is using.
fn scratch_dir() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-agent-server-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// Reads the banner and returns the port the agent actually bound.
///
/// "linklet-agent listening on 0.0.0.0:51234 transfers under C:\somewhere no log", so the
/// port is the word that starts with the bound address -- not the last thing after a
/// colon, which is wrong the moment a Windows path is on the line. Reading the banner is
/// also the proof that the port is open: it is printed after the bind.
///
/// The reader is leaked on purpose. Dropping it closes the child's stdout, and a process
/// writing to a closed pipe is a process that may exit for a reason unrelated to the test.
fn read_banner(stdout: impl std::io::Read + Send + 'static) -> u16 {
    let mut reader = BufReader::new(stdout);
    let mut banner = String::new();
    reader
        .read_line(&mut banner)
        .expect("the agent prints a banner when it is listening");
    std::mem::forget(reader);

    banner
        .split_whitespace()
        .find_map(|word| word.strip_prefix("0.0.0.0:"))
        .and_then(|text| text.parse().ok())
        .unwrap_or_else(|| panic!("cannot read a port from the banner: {banner:?}"))
}

/// A handshake that is done and a connection that can carry a request.
struct Conversation {
    connection: Connection,
    session: Box<dyn Sealed>,
}

impl Conversation {
    /// Sends one request and returns the reply.
    fn ask(&mut self, request: &Request) -> Reply {
        self.ask_json(&json::write(&wire::request_to_json(request)))
    }

    /// Sends one request written by hand.
    ///
    /// For the shapes [`Request`] cannot express: an operation this version does not
    /// have, and a body that is not a request at all. Going through the type would
    /// make those untestable, and they are exactly the messages an attacker sends.
    fn ask_json(&mut self, body: &str) -> Reply {
        self.send_json(body);
        self.read_one()
    }

    /// Writes one request, and reads nothing.
    ///
    /// Separate from [`Conversation::ask_json`] because the order matters for a
    /// transfer: the sender writes the manifest and then waits for an answer, and a
    /// test that could not stop between those two points could not check that.
    fn send_json(&mut self, body: &str) {
        let sealed = self
            .session
            .seal(body.as_bytes())
            .expect("sealing the request");
        self.connection
            .write_frame(Kind::Sealed, &sealed)
            .expect("writing the request");
    }

    /// Reads one reply, opening it or reading a refusal that came in the clear.
    fn read_one(&mut self) -> Reply {
        let frame = self
            .connection
            .read_frame(Kind::Sealed)
            .expect("the agent answers every request");
        open(&mut self.session, &frame)
    }

    /// Sends a pull request and returns the first reply, leaving the connection
    /// positioned at the manifest or the refusal.
    fn pull(&mut self, path: &str) -> Reply {
        self.ask(&Request::Pull {
            path: path.to_string(),
        })
    }

    /// Sends a push request, waits for the answer to the manifest, and only then sends
    /// the file -- if the manifest was accepted at all.
    ///
    /// **The order is the client's, and it is the point of T14**: a sender that streams
    /// before the receiver has answered puts the file on the wire behind a refusal its own
    /// close will destroy. A helper that sent the body first would hide exactly the failure
    /// this shape exists to prevent, which is what it did until a real machine found it.
    ///
    /// The body is sent from memory rather than through `linklet_adapters`' sender on
    /// purpose: that function reads a file, and this test is about what the *agent* does
    /// with the messages.
    fn push(&mut self, manifest: &Manifest, body: &[u8]) -> Reply {
        self.send_json(&json::write(&wire::request_to_json(&wire::Request::Push(
            manifest.clone(),
        ))));

        // Two replies are read on this connection from here: the answer to the manifest and
        // the transfer's result. The default budget is the handshake and one reply, so the
        // sender raises it -- exactly as `linklet-client` does, for exactly this reason.
        self.connection
            .set_message_limit(self.connection.messages_read() + 2);

        let answer = self.read_one();
        if matches!(answer, Reply::Refused(_)) {
            // Refused at the manifest, so no chunk is sent -- which is the property the
            // caller is asserting by getting this back.
            return answer;
        }
        let accepted = wire::accepted_bytes_from_reply(&answer).expect("an acceptance");
        assert_eq!(
            accepted, manifest.bytes,
            "the agent accepted a different size from the one declared"
        );

        for chunk in body.chunks(CHUNK) {
            let sealed = self.session.seal(chunk).expect("sealing a chunk");
            self.connection
                .write_frame(Kind::Sealed, &sealed)
                .expect("writing a chunk");
        }

        self.read_one()
    }
}

/// Opens a reply frame, falling back to reading it in the clear.
///
/// The fallback is not laxity: the agent's refusal for "this session did not open"
/// is sent unsealed, because the session that would seal it is what failed. A test
/// that did not read it would be asserting on a decryption failure instead of on
/// the agent's answer.
fn open(session: &mut Box<dyn Sealed>, frame: &[u8]) -> Reply {
    let mut plaintext = Vec::new();
    match session.open_into(frame, &mut plaintext) {
        Ok(()) => reply(&plaintext),
        Err(_) => reply(frame),
    }
}

/// One of the protocol's replies, out of a frame body.
fn reply(body: &[u8]) -> Reply {
    let value = wire::parse_body(body).unwrap_or_else(|e| {
        panic!(
            "the reply is not a message this protocol defines: {e} in {:?}",
            String::from_utf8_lossy(body)
        )
    });
    wire::reply_from_json(&value).expect("a reply shape")
}

// --- pulling a file ----------------------------------------------------------

#[test]
fn a_pull_sends_the_manifest_and_then_the_file() {
    // The agent's half of a pull. The manifest must come **first**, because it is what
    // tells the caller how many chunks to expect -- and the caller's message budget is
    // set from it, so a manifest that arrived after the body would leave the body
    // refused.
    let agent = Agent::start();
    let content = b"the bytes of a log file";
    std::fs::write(agent.root.join("build.log"), content).expect("writing a file to pull");

    let mut conversation = agent.sealed();
    let first = conversation.pull(r"build.log");

    let manifest = match first {
        Reply::Result(value) => wire::manifest_from_json(&value).expect("a manifest"),
        Reply::Refused(reason) => panic!("the agent refused the pull: {reason}"),
    };
    assert_eq!(manifest.bytes, content.len() as u64);
    assert_eq!(
        manifest.sha256,
        linklet_adapters::digest_of_file(&agent.root.join("build.log")).expect("a digest")
    );

    // And then the body, which is one chunk for a file this size. The budget has to be
    // raised the way the host raises it -- from the manifest -- because that is T11
    // enforced on the reading side, and a test that skipped the step would be reading in
    // a state no real receiver is ever in.
    conversation
        .connection
        .set_message_limit(conversation.connection.messages_read() + manifest.chunks());
    let frame = conversation
        .connection
        .read_frame(Kind::Sealed)
        .expect("a chunk after the manifest");
    let mut plaintext = Vec::new();
    conversation
        .session
        .open_into(&frame, &mut plaintext)
        .expect("opening the chunk");
    assert_eq!(plaintext, content);
}

#[test]
fn a_pull_of_a_file_that_is_not_there_is_refused_by_name() {
    let agent = Agent::start();
    let reason = refusal(agent.sealed().pull(r"logs\missing.log"));

    assert!(reason.contains("missing.log"), "{reason}");
}

#[test]
fn a_pull_that_escapes_the_root_is_refused_by_name() {
    // The read side of T1, and it is the same rule: the root is what stops a pull from
    // reading the machine rather than the directory the agent was pointed at. A path
    // that must not be written must not be read either.
    let agent = Agent::start();
    let reason = refusal(
        agent
            .sealed()
            .pull(r"..\..\Windows\System32\drivers\etc\hosts"),
    );

    assert!(
        reason.contains("..") || reason.contains("outside"),
        "the refusal should name what it refused: {reason}"
    );
}

#[test]
fn a_pull_of_something_that_is_not_a_file_is_refused_rather_than_opened() {
    // A directory would otherwise surface as an operating-system error from `File::open`
    // rather than as a named refusal, and `docs/transfer.md` T2 is about the same
    // question asked of a write.
    let agent = Agent::start();
    std::fs::create_dir(agent.root.join("a-directory")).expect("a directory to aim at");

    let reason = refusal(agent.sealed().pull("a-directory"));
    assert!(
        reason.contains("regular file"),
        "the refusal should say what it is not: {reason}"
    );
}

/// The reason out of a reply that must be a refusal.
fn refusal(reply: Reply) -> String {
    match reply {
        Reply::Refused(reason) => reason,
        Reply::Result(value) => panic!("expected a refusal and got a result: {value:?}"),
    }
}

/// The outcome out of a reply that must be a result.
fn outcome(reply: Reply) -> RunOutcome {
    match reply {
        Reply::Result(value) => {
            wire::run_outcome_from_json(&value).expect("the result is a run outcome")
        }
        Reply::Refused(reason) => panic!("expected an outcome and got a refusal: {reason}"),
    }
}

/// Runs a command and decodes the outcome it produces.
fn run(agent: &Agent, command: &str) -> RunOutcome {
    run_with(agent, command, 30)
}

/// Runs a command with an explicit deadline.
fn run_with(agent: &Agent, command: &str, timeout_seconds: u64) -> RunOutcome {
    outcome(agent.sealed().ask(&Request::Run(RunRequest {
        command: command.to_string(),
        timeout_seconds,
    })))
}

// --- what the agent recorded about the requests it served --------------------

/// The lines of a log file, or a panic naming the path.
fn logged(path: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    text.lines().map(str::to_string).collect()
}

/// The number out of a log line, which is what ties a `->` to its `<-`.
fn number_of(line: &str) -> String {
    line.split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("a log line names a request: {line:?}"))
        .to_string()
}

#[test]
fn a_request_leaves_a_taken_line_and_an_answered_line() {
    // **The property the whole log exists for.** A line written only when a request
    // finishes cannot describe a request that did not, and a process that dies mid-request
    // writes nothing at all -- so the final request of an agent that died looks exactly
    // like a request that never arrived. Two lines make the absence of the second one the
    // evidence.
    let log_path =
        std::env::temp_dir().join(format!("linklet-agent-log-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&log_path);

    let agent = Agent::start_with(&["--log", &log_path.to_string_lossy()]);
    let outcome = run(&agent, "echo logged");
    assert_eq!(outcome.exit_code, Some(0), "{outcome:#?}");

    let lines = logged(&log_path);
    let _ = std::fs::remove_file(&log_path);

    assert_eq!(lines.len(), 2, "one request, two lines: {lines:#?}");
    assert!(lines[0].starts_with("-> "), "{lines:#?}");
    assert!(lines[0].ends_with(" run"), "{lines:#?}");
    assert!(lines[1].starts_with("<- "), "{lines:#?}");
    assert!(lines[1].contains(" run ok "), "{lines:#?}");
    assert!(lines[1].ends_with(" ms"), "{lines:#?}");

    assert_eq!(
        number_of(&lines[0]),
        number_of(&lines[1]),
        "the two lines have to be the same request: {lines:#?}"
    );
}

#[test]
fn a_request_this_version_cannot_read_is_still_recorded() {
    // The traffic an operator most wants to find: a body that arrived and was not a
    // request. A log that only recorded what it understood would agree with the protocol
    // instead of with the machine.
    let log_path = std::env::temp_dir().join(format!(
        "linklet-agent-log-unknown-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&log_path);

    let agent = Agent::start_with(&["--log", &log_path.to_string_lossy()]);
    let reason = refusal(agent.sealed().ask_json("not a request at all"));
    assert!(!reason.is_empty());

    let lines = logged(&log_path);
    let _ = std::fs::remove_file(&log_path);

    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(lines[0].ends_with(" unknown"), "{lines:#?}");
    assert!(lines[1].contains(" unknown refused "), "{lines:#?}");
    assert!(
        lines[1].contains(&reason),
        "the reason the caller was given belongs in the log too: {lines:#?}"
    );
}

#[test]
fn the_log_says_which_agent_it_is_for_a_request_that_worked() {
    // `identity` is the other operation on the surface, and this is the baseline: a
    // request that asks for nothing and succeeds is still two lines.
    let log_path = std::env::temp_dir().join(format!(
        "linklet-agent-log-identity-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&log_path);

    let agent = Agent::start_with(&["--log", &log_path.to_string_lossy()]);
    let _ = agent.sealed().ask(&Request::Identity);

    let lines = logged(&log_path);
    let _ = std::fs::remove_file(&log_path);

    assert_eq!(lines.len(), 2, "{lines:#?}");
    assert!(lines[0].ends_with(" identity"), "{lines:#?}");
    assert!(lines[1].contains(" identity ok "), "{lines:#?}");
}

#[test]
fn an_agent_without_a_log_writes_no_log() {
    // The default, and it is not a stub: an agent that was not asked to keep a log keeps
    // none. Without this the log could be created by accident and a reader would have to
    // find out which way round the option works.
    let agent = Agent::start();
    let outcome = run(&agent, "echo unlogged");

    assert_eq!(outcome.exit_code, Some(0), "{outcome:#?}");
    assert!(
        !agent.root.join("linklet-agent.log").exists(),
        "an agent with no --log wrote something anyway"
    );
}

#[test]
fn the_agent_refuses_to_start_with_a_log_it_cannot_open() {
    // The same argument as the token and the root: an operator who asked for a log and
    // silently did not get one has a machine whose evidence they believe exists and does
    // not. That is the mistake this feature is a reaction to.
    let directory = scratch_dir();
    let child = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
        .env("LINKLET_TOKEN", TEST_TOKEN)
        .arg("--port")
        .arg("0")
        .arg("--root")
        .arg(&directory)
        // A directory where a file was expected: `RequestLog::open` refuses it by name
        // rather than letting the first write fail.
        .arg("--log")
        .arg(&directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the agent should start");

    let output = child.wait_with_output().expect("the agent should exit");
    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a refusal to start, and the agent is running"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--log") && stderr.contains("directory"),
        "the message should name the option and what was wrong with it: {stderr}"
    );
}

// --- who is there ------------------------------------------------------------

#[test]
fn the_agent_says_which_agent_it_is() {
    let agent = Agent::start();
    let reply = agent.sealed().ask(&Request::Identity);

    let Reply::Result(value) = reply else {
        panic!("the identity call should be answered");
    };
    assert_eq!(value.get_str("name"), Some("linklet-agent"));
    assert!(
        value.get_str("version").is_some(),
        "a caller that has only a name cannot tell an old agent from a new one"
    );
}

// --- running a command -------------------------------------------------------

#[test]
fn a_command_that_succeeds_returns_its_output_and_an_exit_code_of_zero() {
    let agent = Agent::start();
    let outcome = run(&agent, "echo hello");

    assert_eq!(outcome.exit_code, Some(0), "{outcome:#?}");
    assert!(
        outcome.stdout.as_str().contains("hello"),
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
    let reply = agent.sealed().ask(&Request::Run(RunRequest {
        command: "exit 3".to_string(),
        timeout_seconds: 30,
    }));

    let outcome = outcome(reply);
    assert_eq!(outcome.exit_code, Some(3), "{outcome:#?}");
}

#[test]
fn standard_error_survives_the_trip() {
    // A failing build writes to stderr and nothing else, so a transport that
    // loses stderr loses the entire diagnosis.
    let agent = Agent::start();
    let outcome = run(&agent, "echo problem 1>&2");

    assert!(
        outcome.stderr.as_str().contains("problem"),
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
        outcome.stderr.as_str().contains("not recognized")
            || outcome.stdout.as_str().contains("not recognized"),
        "the shell's complaint should reach the caller: {outcome:#?}"
    );
}

#[test]
fn a_caller_supplied_timeout_kills_the_command_and_says_which_failure_it_was() {
    // The three-way distinction, over a real socket: killed by the deadline is a
    // different fact from failing to start, and the reason constant is what the
    // host branches on.
    let agent = Agent::start();
    let outcome = run_with(&agent, "ping -n 30 127.0.0.1", 1);

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

#[test]
fn several_commands_in_a_row_all_answer() {
    // One request per connection, and a thread per connection: the second call is
    // the assertion that the first did not leave the listener in a bad state.
    let agent = Agent::start();
    for i in 0..5 {
        let outcome = run(&agent, &format!("echo round-{i}"));
        assert_eq!(outcome.exit_code, Some(0));
        assert!(
            outcome.stdout.as_str().contains(&format!("round-{i}")),
            "round {i} came back wrong: {outcome:#?}"
        );
    }
}

// --- what is refused before a command runs -----------------------------------

#[test]
fn a_message_that_is_not_json_is_refused_and_no_command_runs() {
    // Sealed, because an unsealed body never opens at all -- which is a different
    // refusal, tested below. This one is about a body that arrived intact and was not
    // a message.
    let agent = Agent::start();
    let reason = refusal(agent.sealed().ask_json("not json at all"));

    assert!(!reason.is_empty(), "a refusal has to say something");
    assert!(
        !reason.contains("exit_code"),
        "a refusal must not carry a result field: {reason}"
    );
}

#[test]
fn an_operation_this_version_does_not_have_is_refused_with_the_ones_it_does() {
    // A version skew is a list rather than a puzzle. The alternative -- an agent that
    // guessed at an unknown op -- turns a broken client into a command that runs.
    let agent = Agent::start();
    let reply = agent.sealed().ask_json(r#"{"op": "install"}"#);

    let reason = refusal(reply);
    assert!(reason.contains("install"), "{reason}");
    assert!(
        reason.contains("identity") && reason.contains("run"),
        "the refusal should list the operations that exist: {reason}"
    );
}

#[test]
fn a_command_field_that_is_missing_is_refused_by_name() {
    let agent = Agent::start();
    let reply = agent
        .sealed()
        .ask_json(r#"{"op": "run", "timeout_seconds": 5}"#);

    let reason = refusal(reply);
    assert!(reason.contains("command"), "{reason}");
}

#[test]
fn a_timeout_beyond_the_protocol_limit_is_refused_rather_than_clamped() {
    // An agent that accepts an unbounded timeout from the network has been handed
    // a way to be occupied forever.
    let agent = Agent::start();
    let reply = agent
        .sealed()
        .ask_json(r#"{"op": "run", "command": "echo x", "timeout_seconds": 100000}"#);

    let reason = refusal(reply);
    assert!(reason.contains("timeout_seconds"), "{reason}");
}

// --- who is allowed to ask ---------------------------------------------------

#[test]
fn a_session_that_did_not_open_is_refused_in_the_clear_and_runs_nothing() {
    // The whole point of the token, over a real socket: someone who can reach the
    // port but does not know the secret gets nothing. The agent's hello is accepted
    // by anyone -- the token is mixed into the key derivation, so the refusal cannot
    // happen until the caller seals something.
    let agent = Agent::start();
    let mut conversation = agent.sealed_with(WRONG_TOKEN);
    let reply = conversation.ask(&Request::Run(RunRequest {
        command: "echo should-not-run".to_string(),
        timeout_seconds: 5,
    }));

    let reason = refusal(reply);
    assert_eq!(
        reason,
        linklet_core::auth::unauthorized_reason(),
        "the refusal should be the agent's one sentence about a token"
    );
    assert!(
        !reason.contains("should-not-run"),
        "no outcome may be produced for a session that did not open: {reason}"
    );
}

#[test]
fn a_wrong_token_is_refused_the_same_way_as_a_missing_one() {
    // Same words, so the reply cannot be used to learn whether a guess was closer than
    // no guess. Which of the two happened is information a caller who has the token
    // does not need and one who does not should not get.
    //
    // "Missing" is not a case the agent can see any more: a caller with no token
    // cannot begin a handshake at all, and that is refused by the *client* before a
    // socket is opened -- see `linklet-client/tests/against_agent.rs`. What is left to
    // check here is that every unusable token gets one sentence.
    let agent = Agent::start();
    let reasons: Vec<String> = ["", "short", WRONG_TOKEN]
        .into_iter()
        .map(|token| {
            refusal(agent.sealed_with(token).ask(&Request::Run(RunRequest {
                command: "echo nope".to_string(),
                timeout_seconds: 5,
            })))
        })
        .collect();

    assert!(
        reasons.windows(2).all(|pair| pair[0] == pair[1]),
        "every unusable token should get the same answer: {reasons:?}"
    );
    assert_eq!(reasons[0], linklet_core::auth::unauthorized_reason());
}

// --- something that is not this protocol at all -------------------------------

#[test]
fn a_service_that_is_not_this_one_gets_silence_and_not_a_frame() {
    // The operator's first mistake: the host pointed at the wrong port. Answering in a
    // language the peer does not read would be noise on someone else's connection --
    // and the refusal that names the byte is on *this* side, where it is useful.
    let agent = Agent::start();
    let mut stream = agent.stream();
    stream
        .write_all(b"GET / HTTP/1.1\r\n\r\n")
        .expect("writing another protocol");
    stream.flush().expect("flushing");

    let mut connection = Connection::with_budget(stream, TEST_BUDGET);
    let error = connection
        .read_frame(Kind::Hello)
        .expect_err("a silent close, not a reply");
    assert!(
        matches!(error, ConnectionError::Ended),
        "expected the agent to close without answering, got {error:?}"
    );
}

#[test]
fn a_frame_of_the_wrong_kind_is_refused_with_the_reason_in_it() {
    // Unlike the case above, this peer *is* speaking frames -- it sent a valid one of
    // the wrong kind -- so it can read a refusal and the refusal names both kinds.
    let agent = Agent::start();
    let mut connection = Connection::with_budget(agent.stream(), TEST_BUDGET);
    connection
        .write_frame(Kind::Sealed, b"sealed before any hello")
        .expect("writing");

    let frame = connection
        .read_frame(Kind::Hello)
        .expect("a refusal rather than silence");
    let reason = refusal(reply(&frame));

    assert!(
        reason.contains("hello") && reason.contains("sealed"),
        "the refusal should name both kinds: {reason}"
    );
}

// --- pushing a file ----------------------------------------------------------

#[test]
fn the_agent_answers_a_push_manifest_before_any_of_the_file_is_sent() {
    // **Found on a real machine and not on loopback**, which is the whole reason this
    // test is written as an order rather than as an outcome.
    //
    // The sender streams the body as soon as it has written the manifest, so a receiver
    // that decides at the manifest has to say so *before* the body arrives. Otherwise it
    // writes the refusal and closes with the sender's unread chunks in its receive queue
    // -- and Windows resets a socket closed in that state, which destroys the refusal the
    // sender had not read yet. What the sender reports instead is "the agent closed the
    // connection without answering": true, and no help at all to whoever pushed a build
    // to the wrong place.
    //
    // On loopback the reset usually loses the race and the refusal arrives; over a real
    // link it lost it every time with a kilobyte of body. So what is checked here is the
    // property that makes it deterministic: **no chunk is sent, and an answer comes back
    // anyway.**
    let agent = Agent::start();
    let mut conversation = agent.sealed();
    // Short, so a regression fails this test in seconds rather than in the default
    // thirty. Nothing about the transfer needs longer.
    conversation.connection.set_budget(Duration::from_secs(5));

    conversation.send_json(&json::write(&wire::request_to_json(&Request::Push(
        Manifest {
            path: r"..\..\escaped.exe".to_string(),
            bytes: 1024,
            sha256: DIGEST.to_string(),
        },
    ))));

    let reply = conversation.read_one();
    let reason = refusal(reply);
    assert!(
        reason.contains("..") || reason.contains("escaped"),
        "the refusal should name what it refused: {reason}"
    );
}

#[test]
fn a_pushed_file_lands_under_the_root_and_the_agent_reports_its_digest() {
    // The agent's half of a push, over a real socket and a real disk: the manifest
    // arrives, the chunks follow, and the answer carries the digest of what is now
    // there. `linklet-client`'s test is the same trip from the other end; this one can
    // look at the file the agent wrote, which is what the client cannot do.
    let agent = Agent::start();
    let content = b"the bytes of a build artifact";
    let digest = linklet_adapters::digest_of_file(&write_source(&agent, content))
        .expect("hashing the source");

    let manifest = Manifest {
        path: "build.exe".to_string(),
        bytes: content.len() as u64,
        sha256: digest.clone(),
    };

    let result = wire::transfer_outcome_from_reply(&agent.sealed().push(&manifest, content))
        .expect("a transfer result");
    assert_eq!(result.bytes, content.len() as u64);
    assert_eq!(result.sha256, digest);

    assert_eq!(
        std::fs::read(agent.root.join("build.exe")).expect("the file should be there"),
        content
    );
    assert!(
        !agent.root.join("build.exe.part").exists(),
        "a completed push leaves no temporary behind"
    );
}

#[test]
fn a_push_whose_path_escapes_the_root_is_refused_before_a_chunk_is_read() {
    // T1 at the agent's own layer: the path is checked against the configured root
    // before the temporary file exists, so a `..` costs nothing and writes nothing.
    let agent = Agent::start();
    let content = b"payload";
    let digest = linklet_adapters::digest_of_file(&write_source(&agent, content))
        .expect("hashing the source");

    let manifest = Manifest {
        path: r"..\..\escaped.exe".to_string(),
        bytes: content.len() as u64,
        sha256: digest,
    };

    let reason = refusal(agent.sealed().push(&manifest, content));
    assert!(
        reason.contains("..") || reason.contains("escaped"),
        "the refusal should name what it refused: {reason}"
    );
    assert!(
        !agent.root.join("escaped.exe").exists(),
        "nothing may be written outside the root"
    );
}

#[test]
fn a_push_of_more_bytes_than_it_declared_is_refused_and_leaves_nothing() {
    // T4 at the agent's layer, and the one most likely to be missed because the
    // declared number *was* checked. The refusal arrives after the first chunk, and the
    // real path is never touched.
    let agent = Agent::start();
    let content = vec![9u8; 4096];
    let digest = linklet_adapters::digest_of_file(&write_source(&agent, &content))
        .expect("hashing the source");

    let manifest = Manifest {
        path: "build.exe".to_string(),
        // Understated on purpose: the sender then keeps sending.
        bytes: 100,
        sha256: digest,
    };

    let reason = refusal(agent.sealed().push(&manifest, &content));
    assert!(
        reason.contains("100"),
        "the refusal should name the declared size: {reason}"
    );
    assert!(!agent.root.join("build.exe").exists());
    assert!(!agent.root.join("build.exe.part").exists());
}

/// Writes the bytes these push tests send, and returns the local path.
///
/// The file is written so that `digest_of_file` can hash it the way the host would --
/// through the same function, so the digest in the manifest is the one the host would
/// have put there rather than one this test invented.
fn write_source(agent: &Agent, content: &[u8]) -> std::path::PathBuf {
    let path = agent.root.join("source.bin");
    std::fs::write(&path, content).expect("writing the source");
    path
}

#[test]
fn the_agent_refuses_to_start_with_a_root_that_is_not_a_directory() {
    // A configuration error should be loud when it is made, which is the same argument the
    // token gets one test above. Without this the agent starts happily and **every**
    // transfer fails later with a filesystem error naming a path nobody typed -- which is
    // what happened on the first real machine this ran against, where `--root` pointed at
    // a directory that had not been created yet.
    //
    // The wait is bounded rather than `output()`, because the failure being checked for is
    // "it kept running": a test that blocked on a process which is serving happily would
    // hang the suite instead of failing it.
    let missing = std::env::temp_dir().join("linklet-root-that-does-not-exist");
    let _ = std::fs::remove_dir_all(&missing);

    let mut child = Command::new(env!("CARGO_BIN_EXE_linklet-agent"))
        .env("LINKLET_TOKEN", TEST_TOKEN)
        .args(["--port", "0"])
        .arg("--root")
        .arg(&missing)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the agent should start");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("waiting for the agent") {
            break Some(status);
        }
        if std::time::Instant::now() > deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("the agent started with a root that does not exist, and is still running");
    };

    assert_eq!(
        status.code(),
        Some(2),
        "expected a refusal to start rather than a running agent"
    );

    let mut stderr = String::new();
    std::io::Read::read_to_string(
        child.stderr.as_mut().expect("stderr was piped"),
        &mut stderr,
    )
    .expect("reading stderr");
    assert!(
        stderr.contains("linklet-root-that-does-not-exist"),
        "the message should name the root: {stderr}"
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
