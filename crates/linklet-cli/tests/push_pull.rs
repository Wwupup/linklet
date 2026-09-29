//! The transfer commands, run as processes against a real agent.
//!
//! `crates/linklet-client/tests/against_agent.rs` covers the two ends as library calls.
//! What is left for this layer is what only exists once a program is a process, and it is
//! the same list `tests/cli.rs` has: **the exit code, which stream the text went to, and
//! what the line says.** An agent that cannot tell a refused transfer from a broken one
//! will retry the wrong thing, and no library test can see that.
//!
//! The agent is spawned here rather than in `tests/cli.rs` because a transfer needs one;
//! `check` does not.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use linklet_core::ExitCode;

/// The token both ends are configured with.
const TEST_TOKEN: &str = "test-token-0123456789";

/// Where the agent binary is, worked out the way `against_agent.rs` does.
///
/// `CARGO_BIN_EXE_linklet-agent` does not exist in this package: that variable is
/// defined only for a binary of the crate being compiled.
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
        "linklet-cli-transfer-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A running agent, killed and cleaned up when the test ends.
struct Agent {
    child: Child,
    address: String,
    root: PathBuf,
}

impl Agent {
    fn start() -> Self {
        let root = scratch_dir();
        let mut child = Command::new(agent_binary())
            .env("LINKLET_TOKEN", TEST_TOKEN)
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

        // "linklet-agent listening on 0.0.0.0:51234 transfers under C:\somewhere", so the
        // port is the word that starts with the bound address.
        let port = banner
            .split_whitespace()
            .find_map(|word| word.strip_prefix("0.0.0.0:"))
            .and_then(|text| text.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("cannot read a port from {banner:?}"));

        Self {
            child,
            address: format!("127.0.0.1:{port}"),
            root,
        }
    }

    /// A path inside the agent's root, which is the only place a transfer may go.
    fn under_root(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Runs the command line with the token in the environment, as a deployment would.
fn linklet(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_linklet"))
        .env("LINKLET_TOKEN", TEST_TOKEN)
        .args(arguments)
        .output()
        .expect("the binary under test should be runnable")
}

/// The exit code, as a `u8` so it can be compared with `ExitCode` directly.
fn code(output: &Output) -> u8 {
    output
        .status
        .code()
        .expect("the process should exit normally, not be killed by a signal") as u8
}

/// stdout as text.
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("the tool prints UTF-8")
}

/// stderr as text.
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("the tool prints UTF-8")
}

// --- the two commands --------------------------------------------------------

#[test]
fn a_push_takes_a_file_to_the_agent_and_says_what_landed() {
    let agent = Agent::start();
    let source = agent.under_root("source.bin");
    std::fs::write(&source, b"a build artifact").expect("writing the source");

    let output = linklet(&[
        "push",
        "--agent",
        &agent.address,
        "--from",
        source.to_str().expect("a UTF-8 path"),
        "--to",
        "build.exe",
    ]);

    assert_eq!(code(&output), ExitCode::SUCCESS, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("build.exe") && text.contains("16 bytes"),
        "the line should name where it landed and how big it is: {text}"
    );
    assert!(
        text.contains("sha256 "),
        "and the digest, which is the only reason a transfer reports anything: {text}"
    );
    assert!(
        stderr(&output).is_empty(),
        "a success is not an error message"
    );

    assert_eq!(
        std::fs::read(agent.under_root("build.exe")).expect("the file should be there"),
        b"a build artifact"
    );
}

#[test]
fn a_pull_brings_a_file_back_and_says_what_arrived() {
    // The pair to the test above, from the other end: the command that exists for
    // collecting a log or a result. Push and pull share a parser, and this is the test
    // that catches them sharing it *wrongly* -- `--from` and `--to` swap meaning between
    // the two, and a copy-paste would send the file back.
    let agent = Agent::start();
    let remote = agent.under_root("build.log");
    std::fs::write(&remote, b"a log worth collecting").expect("writing the file on the target");

    let destination = agent.under_root("collected.log");
    let output = linklet(&[
        "pull",
        "--agent",
        &agent.address,
        "--from",
        "build.log",
        "--to",
        destination.to_str().expect("a UTF-8 path"),
    ]);

    assert_eq!(code(&output), ExitCode::SUCCESS, "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("collected.log") && text.contains("22 bytes"),
        "the line should name the local destination: {text}"
    );

    assert_eq!(
        std::fs::read(&destination).expect("the file should be here"),
        b"a log worth collecting"
    );
}

#[test]
fn a_refused_transfer_is_exit_three_on_stderr_and_prints_no_result() {
    // The distinction an agent branches on, and the one thing a library test cannot see:
    // "the transfer could not be made" is not "the transfer arrived and was wrong". Exit
    // 3 is a code no command can produce, and the line that would look like a result is
    // not printed at all.
    let agent = Agent::start();
    let source = agent.under_root("source.bin");
    std::fs::write(&source, b"payload").expect("writing the source");

    let output = linklet(&[
        "push",
        "--agent",
        &agent.address,
        "--from",
        source.to_str().expect("a UTF-8 path"),
        // Outside the agent's root, which is T1 and the most severe thing in the document.
        "--to",
        r"..\..\escaped.exe",
    ]);

    assert_eq!(code(&output), ExitCode::REFUSED);
    assert!(
        stdout(&output).is_empty(),
        "a refused transfer must not print something that reads like a result: {}",
        stdout(&output)
    );
    let text = stderr(&output);
    assert!(
        text.contains("refused"),
        "the refusal should say the agent refused it: {text}"
    );
}

#[test]
fn a_transfer_of_a_file_that_is_not_there_is_a_refusal_and_not_a_crash() {
    let agent = Agent::start();
    let missing = agent.under_root("not-here.bin");

    let output = linklet(&[
        "push",
        "--agent",
        &agent.address,
        "--from",
        missing.to_str().expect("a UTF-8 path"),
        "--to",
        "build.exe",
    ]);

    assert_eq!(code(&output), ExitCode::REFUSED);
    assert!(stdout(&output).is_empty());
    assert!(
        stderr(&output).contains("not-here.bin"),
        "the refusal should name the file: {}",
        stderr(&output)
    );
}

// --- a wrong invocation ------------------------------------------------------

#[test]
fn a_transfer_without_a_destination_is_a_usage_error() {
    // Every option is required and none has a default: a transfer with a guessed
    // destination is a file written where nobody asked for it.
    for (label, arguments) in [
        ("no agent", vec!["push", "--from", "a", "--to", "b"]),
        ("no from", vec!["push", "--agent", "a:1", "--to", "b"]),
        ("no to", vec!["push", "--agent", "a:1", "--from", "a"]),
        ("nothing at all", vec!["pull"]),
    ] {
        let output = linklet(&arguments);
        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{label}: {}",
            stderr(&output)
        );
        assert!(
            stdout(&output).is_empty(),
            "{label}: a run that never happened prints nothing"
        );
        assert!(
            !stderr(&output).is_empty(),
            "{label}: a refusal has to say which option is missing"
        );
    }
}

#[test]
fn an_unknown_option_names_itself() {
    let output = linklet(&["push", "--nope", "x"]);
    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(stderr(&output).contains("--nope"), "{}", stderr(&output));
}

#[test]
fn help_lists_both_transfer_commands() {
    // A command nobody can find is a command that does not exist, and the usage text is
    // the only place a person looks.
    let output = linklet(&["--help"]);
    let text = stdout(&output);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(text.contains("linklet push"), "{text}");
    assert!(text.contains("linklet pull"), "{text}");
}
