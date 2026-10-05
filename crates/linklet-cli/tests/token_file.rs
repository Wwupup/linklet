//! The shared secret, when both ends read it from a file.
//!
//! # Why this is a layer and not a unit test
//!
//! What a token *file* contains is decided in `linklet-core` (`secret_in_file`) and
//! unit-tested there in microseconds. What only exists once these are processes is
//! everything else: that the agent resolves the path it was given, that the host
//! presents the same secret, that a Windows-written file works at both ends, and
//! **what each end does when the file or the environment is wrong**. Those are
//! exit codes and sentences, and no library test can see either.
//!
//! # The two rules this file pins, and why they are not the same rule
//!
//! The **agent** refuses to start when its secret is wrong, because a service that
//! starts misconfigured is a service whose first caller finds out. The **host**
//! reports and goes ahead without a token, because `check` and `testbed` need no
//! secret at all -- and a token setting that is wrong must not take away a
//! capability that never used it.
//!
//! What both ends share is that a secret is **named once**: a file and a value
//! together are two answers to one question, and they are refused rather than
//! ordered, because whichever lost would be the one the operator believed was in
//! force. The file is written here with a byte-order mark and a CRLF, exactly as
//! `Set-Content -Encoding utf8` writes one, so that the strip is exercised across
//! the process boundary and not only against a literal in a unit test.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use linklet_core::ExitCode;

/// The secret both ends are configured with.
const TEST_TOKEN: &str = "test-token-0123456789";

/// Where the agent binary is, worked out the way `push_pull.rs` does.
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

/// A directory under the system temporary directory that no other test is using.
fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-cli-token-file-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A token file written the way a Windows editor or `Set-Content -Encoding utf8`
/// writes one: a UTF-8 byte-order mark, the secret, and a CRLF.
///
/// The bytes are escapes so that the source stays ASCII (rule 7).
fn write_token_file(directory: &Path, secret: &str) -> PathBuf {
    let path = directory.join("token.txt");
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes.extend_from_slice(secret.as_bytes());
    bytes.extend_from_slice(b"\r\n");
    std::fs::write(&path, bytes).expect("a token file");
    path
}

/// A running agent, killed and cleaned up when the test ends.
struct Agent {
    child: Child,
    address: String,
    directory: PathBuf,
}

impl Agent {
    /// Starts an agent whose secret comes from `file`, on a port the OS picks.
    fn reading(file: &Path) -> Self {
        let directory = scratch_dir();
        let mut child = Command::new(agent_binary())
            // Removed rather than left to the machine: the claim is about the file
            // being the source, and a developer with either variable exported would
            // otherwise be testing a different rule than the one named here.
            .env_remove("LINKLET_TOKEN")
            .env_remove("LINKLET_TOKEN_FILE")
            .arg("--token-file")
            .arg(file)
            .arg("--port")
            .arg("0")
            .arg("--root")
            .arg(&directory)
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

        // "linklet-agent listening on 0.0.0.0:51234 ...", so the port is the word
        // that starts with the bound address -- not the last thing after a colon,
        // which is wrong the moment a Windows path is on the line.
        let port = banner
            .split_whitespace()
            .find_map(|word| word.strip_prefix("0.0.0.0:"))
            .and_then(|text| text.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("cannot read a port from {banner:?}"));

        Self {
            child,
            address: format!("127.0.0.1:{port}"),
            directory,
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Runs the command line with one token source named and nothing else in the way.
fn linklet(arguments: &[&str], file: Option<&Path>, secret: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_linklet"));
    command
        .env_remove("LINKLET_TOKEN")
        .env_remove("LINKLET_TOKEN_FILE");
    if let Some(path) = file {
        command.env("LINKLET_TOKEN_FILE", path);
    }
    if let Some(secret) = secret {
        command.env("LINKLET_TOKEN", secret);
    }
    command
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

#[test]
fn both_ends_read_the_same_secret_from_the_same_file() {
    // The deployment this exists for: one file on each machine, an ACL on it, and
    // neither a script nor a client configuration holding the value. The file
    // carries a BOM and a CRLF, so an implementation that took the whole file as
    // the secret would fail here with "the token is missing or wrong" while both
    // ends looked correctly configured.
    let directory = scratch_dir();
    let file = write_token_file(&directory, TEST_TOKEN);

    let agent = Agent::reading(&file);
    let output = linklet(
        &["exec", "--agent", &agent.address, "echo token-from-a-file"],
        Some(&file),
        None,
    );

    assert_eq!(
        code(&output),
        0,
        "the host should authenticate with the file: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("exit 0"), "{stdout}");
    assert!(stdout.contains("token-from-a-file"), "{stdout}");

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_file_and_a_secret_together_are_reported_and_present_nothing() {
    // Two answers to one question. Ordered silently, the answer that lost is the
    // one the operator believed was in force, and the symptom is a caller being
    // told its token is wrong when the secret it offered was the right one for
    // one of the two sources.
    let directory = scratch_dir();
    let file = write_token_file(&directory, TEST_TOKEN);

    let agent = Agent::reading(&file);
    let output = linklet(
        &["exec", "--agent", &agent.address, "echo should-not-run"],
        Some(&file),
        Some(TEST_TOKEN),
    );

    assert_eq!(
        code(&output),
        ExitCode::REFUSED,
        "no secret should have been presented, so the call cannot be made"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LINKLET_TOKEN_FILE") && stderr.contains("LINKLET_TOKEN"),
        "the message should name both sources: {stderr}"
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("should-not-run"),
        "the command must not have run"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_named_token_file_that_cannot_be_read_is_reported_by_its_path() {
    // The file is the source when it is named, so an unreadable one leaves the
    // host with no secret rather than with the environment's -- silently using a
    // different secret than the one named is the quiet wrong answer this project
    // is arranged against.
    let directory = scratch_dir();
    let missing = directory.join("linklet-no-such-token-file.txt");

    let output = linklet(&["probe", "--agent", "127.0.0.1:1"], Some(&missing), None);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("linklet-no-such-token-file.txt"),
        "the message should name the file: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_broken_token_setting_does_not_take_away_a_capability_that_never_used_it() {
    // `check` needs no secret, and this is the reason the host reports a token
    // problem instead of refusing the run: an environment with both variables set
    // is a configuration mistake, and it must not turn a working reachability check
    // into an exit 3 that sends the reader to the network.
    //
    // `check` does not consult the token source at all -- it never has -- so the
    // claim here is that the misconfiguration cannot reach it: the run happens, the
    // target is reported, and this is a dead target rather than an unmakeable call.
    let directory = scratch_dir();
    let file = write_token_file(&directory, TEST_TOKEN);

    let output = linklet(&["check", "127.0.0.1:1"], Some(&file), Some(TEST_TOKEN));

    assert_eq!(
        code(&output),
        ExitCode::NOT_ALL_ALIVE,
        "the check ran and found the target dead: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("127.0.0.1:1"),
        "the check still answered"
    );

    let _ = std::fs::remove_dir_all(&directory);
}
