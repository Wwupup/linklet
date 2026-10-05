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

use linklet_client::{AgentAddress, CallError, identity, pull, push, render_call_error, run};
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
    /// The one directory this agent's transfers may touch.
    ///
    /// Passed with `--root` rather than inferred from the working directory, because a
    /// test that wrote wherever the agent happened to be started would be writing into
    /// the repository.
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
        // Kept alive so the child's stdout is not closed under it.
        std::mem::forget(reader);

        // "linklet-agent listening on 0.0.0.0:51234 transfers under C:\somewhere", so the
        // port is the word that starts with the bound address -- not the last thing after
        // a colon, which the root path made wrong the moment the root was on the banner.
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

    /// Writes a local file this test can push.
    fn local(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, content).expect("writing a local file");
        path
    }

    /// Runs a command on this agent.
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
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A directory under the system temporary directory that no other test is using.
fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-against-agent-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
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
        outcome.stdout.as_str().contains("end-to-end"),
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

// --- a reply the agent could not frame ---------------------------------------

#[test]
fn output_too_large_to_return_is_refused_by_name_and_not_a_dropped_connection() {
    // **The defect M10 carries, over the whole chain.** A command that writes more
    // than the frame ceiling to stdout leaves the agent with a reply it cannot frame.
    // Before this test existed the agent sent nothing and closed, and the caller's
    // only possible reading was "could not reach the agent: the agent closed the
    // connection without answering" -- which sends whoever reads it to the network
    // for a command that ran perfectly.
    //
    // 16,000,000 bytes of zeros are 21,333,338 bytes of base64: past the ceiling by
    // about 4.5 MB, all of it ASCII, so nothing about this case depends on JSON
    // escaping. `certutil` produces them in about a fifth of a second, against the
    // loop a shell would need.
    //
    // **No command below carries a quote, and that is not fastidiousness.** The agent
    // runs commands through `cmd`, and this test's own scratch directory has a space in
    // its name: a path that needs quoting cannot survive the trip, and the failure reads
    // as the command not existing. So the test makes a directory with no space in the
    // name, and a `cmd` builtin runs in it. `certutil` was chosen over a PowerShell
    // one-liner for the same reason: the one-liner needs quotes, and the quoted forms
    // were measured on this machine -- they came back as the command's own text.
    let agent = Agent::start();
    let workspace = agent.root.join("too-large");
    std::fs::create_dir(&workspace).expect("a directory for the command to work in");
    std::fs::write(workspace.join("zeros.bin"), vec![0u8; 16_000_000])
        .expect("a file whose base64 cannot be framed");

    let result = run(
        &agent.address,
        &RunRequest {
            command: format!(
                "cd /d {} && certutil -encode zeros.bin zeros.b64 && type zeros.b64",
                workspace.display()
            ),
            timeout_seconds: 60,
        },
    );

    let error = result.expect_err("the reply cannot be framed, so this cannot be a result");
    let CallError::Refused(reason) = &error else {
        panic!("the caller must be told what happened, and got {error:?}");
    };

    // Named, both of them: the stream that overflowed and the ceiling it did not
    // fit in. A refusal that only said "too large" would leave the caller with no
    // idea whether to narrow the command or to stop trying.
    assert!(reason.contains("stdout"), "{reason}");
    assert!(
        reason.contains(&wire::reply_ceiling().to_string()),
        "the reason should name the ceiling it did not fit in: {reason}"
    );
    assert!(
        render_call_error(&error).contains("refused"),
        "a refusal is not a transport failure: {}",
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

    // **And it is not an agent that refused**, which is the distinction a fan-out branches on.
    //
    // The check for that used to be `text.contains("refused")`, and the line above is why it
    // was wrong: the operating system reports a closed port with that word, so
    // `render_call_error` puts it in a *transport* failure too. A closed port was therefore
    // reported as a machine that had answered and said no -- which passed here and failed on
    // a runner, where that particular sentence arrived.
    assert!(
        !error.was_refused(),
        "a connection the OS refused is not an agent that refused: {text}"
    );
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
    // The end of the chain, from the caller's side: a secret that does not match is a
    // call that could not be made, and the message says the token was missing or wrong
    // rather than leaving the caller to guess.
    //
    // The refusal arrives **unsealed**, because the session that would have sealed it
    // is exactly what failed to exist -- see `linklet-agent`'s server. So this test is
    // also the one that pins the diagnostic: without that path the caller would get
    // "the sealed reply did not open", which is true and useless.
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
    assert!(text.contains("missing or wrong"), "{text}");
    assert!(
        !text.contains("did not open"),
        "a wrong token should be reported as a wrong token and not as a \
         decryption failure: {text}"
    );
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

// --- pushing a file ----------------------------------------------------------

#[test]
fn a_pushed_file_lands_under_the_agents_root() {
    // The whole of M7 in one test: a host process, an agent process, a sealed channel,
    // a file that crossed it, and a digest that says the file on the far side is the one
    // that was sent.
    let agent = Agent::start();
    let source = agent.local("source.bin", b"a build artifact of some length");

    let outcome = push(&agent.address, &source, "build.exe").expect("the push should work");

    let landed = agent.root.join("build.exe");
    assert_eq!(
        std::fs::read(&landed).expect("the file should be there"),
        b"a build artifact of some length"
    );
    assert_eq!(
        outcome.bytes as usize,
        b"a build artifact of some length".len()
    );
    assert_eq!(
        outcome.sha256,
        linklet_adapters::digest_of_file(&source).expect("hashing the source"),
        "the digest that comes back is the receiver's, and it is the sender's too \
         because the bytes are the same"
    );
    assert!(
        !agent.root.join("build.exe.part").exists(),
        "a completed transfer leaves no temporary behind"
    );
}

#[test]
fn a_push_that_escapes_the_agents_root_is_refused_by_the_agent() {
    // T1, end to end, over a real socket: the most severe item in the transfer
    // document. The path is the caller's, so a caller that is confused about who it is
    // talking to would otherwise be writing anywhere on someone else's machine.
    let agent = Agent::start();
    let source = agent.local("source.bin", b"payload");

    let error = push(&agent.address, &source, r"..\..\escaped.exe")
        .expect_err("a path outside the root must be refused");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    let text = render_call_error(&error);
    assert!(
        text.contains("..") || text.contains("escaped"),
        "the refusal should name what it refused: {text}"
    );
    assert!(
        !agent.root.join("escaped.exe").exists(),
        "nothing may be written outside the root"
    );
    assert!(
        !agent.root.join(r"..\..\escaped.exe.part").exists(),
        "and no temporary may be left either"
    );
}

#[test]
fn a_push_of_a_file_that_is_not_there_is_refused_before_a_socket_is_opened() {
    // The failure is local, and reporting it as a network failure would send the reader
    // to look at the wrong machine. The agent is never asked.
    //
    // `CallError::Local` rather than `Protocol`: this used to be a protocol error, which
    // rendered as "the agent's reply was not understood" for a file that was missing on
    // *this* side, before any socket existed.
    let agent = Agent::start();
    let missing = agent.root.join("not-here.bin");

    let error = push(&agent.address, &missing, "build.exe").expect_err("nothing is there");
    assert!(matches!(error, CallError::Local(_)), "{error:?}");
    assert!(
        error.to_string().contains("not-here.bin"),
        "the refusal should name the file: {error}"
    );
    let text = render_call_error(&error);
    assert!(
        !text.contains("the agent"),
        "a local mistake must not be reported as the agent's: {text}"
    );
    assert!(!agent.root.join("build.exe").exists());
}

// --- pulling a file ----------------------------------------------------------

#[test]
fn a_pulled_file_lands_where_the_host_asked_and_matches_its_digest() {
    // The whole of the other half of M7: a file that was on the target, brought back
    // through the sealed channel, and a digest the host computed itself.
    let agent = Agent::start();
    let content = b"a log file worth collecting";
    std::fs::write(agent.root.join("build.log"), content).expect("writing a file on the target");

    let destination = agent.root.join("collected.log");
    let outcome = pull(&agent.address, "build.log", &destination).expect("the pull should work");

    assert_eq!(
        std::fs::read(&destination).expect("the file should be here"),
        content
    );
    assert_eq!(outcome.bytes as usize, content.len());
    assert_eq!(
        outcome.sha256,
        linklet_adapters::digest_of_file(&destination).expect("hashing what arrived"),
        "the digest is of the file this host now has, which is the point of computing it"
    );
    assert!(
        !agent.root.join("collected.log.part").exists(),
        "a completed pull leaves no temporary behind"
    );
}

#[test]
fn a_pulled_file_larger_than_one_chunk_arrives_intact() {
    // The chunk boundary in the pulling direction, which is the one where the *host* is
    // the receiver: the agent sends the file and the message budget comes from the
    // manifest the agent declared.
    let agent = Agent::start();
    let chunk = linklet_core::transfer::CHUNK_BYTES as usize;
    let content: Vec<u8> = (0..chunk + 37).map(|index| (index % 253) as u8).collect();
    std::fs::write(agent.root.join("big.log"), &content).expect("writing a large file");

    let destination = agent.root.join("big-collected.log");
    let outcome = pull(&agent.address, "big.log", &destination).expect("the pull should work");

    assert_eq!(outcome.bytes as usize, content.len());
    assert_eq!(
        std::fs::read(&destination).expect("the file should be here"),
        content
    );
}

#[test]
fn a_pull_of_a_path_outside_the_agents_root_is_refused() {
    // T1 in the reading direction, over a real socket: without the root, a caller could
    // read any file on the target, and the agent would be a file server for the machine.
    let agent = Agent::start();
    let destination = agent.root.join("stolen.txt");

    let error = pull(
        &agent.address,
        r"..\..\Windows\System32\drivers\etc\hosts",
        &destination,
    )
    .expect_err("a path outside the root must be refused");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    assert!(
        !destination.exists(),
        "nothing may be written for a pull that was refused"
    );
}

#[test]
fn a_pull_of_a_file_that_is_not_there_is_refused_and_writes_nothing() {
    let agent = Agent::start();
    let destination = agent.root.join("missing.log");

    let error = pull(&agent.address, "not-here.log", &destination).expect_err("nothing is there");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    assert!(error.to_string().contains("not-here.log"), "{error}");
    assert!(!destination.exists());
}

#[test]
fn a_pull_onto_a_destination_that_is_a_directory_is_refused() {
    // The host's own half of T2. The agent did its job -- it offered a real file -- and
    // the receiving side refuses to put it somewhere that is not a file, which is a
    // failure of *this* side and not of the network.
    let agent = Agent::start();
    std::fs::write(agent.root.join("build.log"), b"content").expect("writing a file");
    let destination = agent.root.join("a-directory");
    std::fs::create_dir(&destination).expect("a directory to aim at");

    let error =
        pull(&agent.address, "build.log", &destination).expect_err("a directory is not a file");

    // `Local`, because it is: the reason names the destination this side chose, and the
    // agent had already offered a perfectly good file.
    assert!(matches!(error, CallError::Local(_)), "{error:?}");
    assert!(
        error.to_string().contains("a-directory"),
        "the reason should name the destination: {error}"
    );
    assert!(
        destination.is_dir(),
        "and the directory is still a directory"
    );
    assert!(!agent.root.join("a-directory.part").exists());
}

#[test]
fn pushing_twice_over_the_same_path_replaces_it() {
    // T13: the operation is declared idempotent rather than transactional, because a
    // crash between the rename and the reply would otherwise need a caller to guess.
    // Retrying overwrites, which is safe.
    let agent = Agent::start();

    let first = agent.local("first.bin", b"the first build");
    push(&agent.address, &first, "build.exe").expect("the first push");
    assert_eq!(
        std::fs::read(agent.root.join("build.exe")).expect("the file"),
        b"the first build"
    );

    let second = agent.local("second.bin", b"the second build, which is longer");
    let outcome = push(&agent.address, &second, "build.exe").expect("the second push");

    assert_eq!(
        std::fs::read(agent.root.join("build.exe")).expect("the file"),
        b"the second build, which is longer"
    );
    assert_eq!(
        outcome.bytes as usize,
        b"the second build, which is longer".len()
    );
    assert!(!agent.root.join("build.exe.part").exists());
}

#[test]
fn a_push_that_the_agent_refuses_says_so_and_is_not_a_transport_failure() {
    // The distinction the whole protocol is arranged around, over the transfer: the
    // file was offered, the agent answered, and the answer was no. A caller must not
    // have to work out from prose whether anything was written.
    let agent = Agent::start();
    let source = agent.local("source.bin", b"payload");

    // An empty file cannot be described, so the local refusal is the one to check here;
    // the *agent's* refusal is the escape test above. What this adds is that a refusal
    // carries the agent's own words rather than being flattened into "the call failed".
    let empty = agent.local("empty.bin", b"");
    let error = push(&agent.address, &empty, "build.exe").expect_err("no bytes to send");
    assert!(
        error.to_string().contains("no bytes"),
        "the refusal should say what is wrong with the transfer: {error}"
    );

    // And a real one: the agent's root, asked to receive a file whose path is a
    // directory that does not exist, refuses rather than failing halfway.
    //
    // **The reason has to survive the trip, and that is not a detail.** This assertion was
    // `matches!(error, CallError::Refused(_))` and passed on loopback while a real machine
    // reported "the agent closed the connection without answering" every time: the sender
    // streamed the body before reading the agent's answer, so the agent's close -- with
    // those bytes unread -- reset the connection and destroyed the refusal. See T14 in
    // `docs/transfer.md` and `tests/manifest_refusal.rs`, which reproduces it
    // deterministically.
    let error = push(&agent.address, &source, r"no\such\directory\build.exe")
        .expect_err("the directory does not exist");
    match &error {
        CallError::Refused(reason) => {
            assert!(
                reason.contains("no\\such\\directory") || reason.contains("cannot find"),
                "the refusal should name the path the agent could not write: {reason}"
            );
        }
        other => panic!("a refusal has to arrive as a refusal, and got {other:?}"),
    }
    assert!(
        !agent
            .root
            .join(r"no\such\directory\build.exe.part")
            .exists(),
        "a refused transfer must not leave a temporary behind"
    );
}
