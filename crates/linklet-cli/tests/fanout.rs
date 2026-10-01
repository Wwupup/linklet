//! One command across several machines, run as a process.
//!
//! The decisions are a table in `linklet_core::fanout` and the shape of the report is tested
//! there. What is left for this layer is what only a process can show: **which exit code**, and
//! **that one machine being down does not stop the others** -- the second of which cannot be
//! tested anywhere else, because it needs two machines that behave differently.
//!
//! The two machines here are two agents on two ports of this host, one of them told the wrong
//! token. That is deliberate: a live agent and a dead port prove the reachability distinction,
//! and a live agent with the wrong secret proves the other one -- **refused is not
//! unreachable**, and a caller that confused them would go and look at the network for a
//! mistake in its own token.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use linklet_core::ExitCode;

/// The token the tests configure the agents with.
const TEST_TOKEN: &str = "fanout-token-0123456789";

/// Where the agent binary is, worked out the way `push_pull.rs` does it.
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
        "linklet-fanout-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A running agent, killed and cleaned up when the test ends.
///
/// `token` is a parameter rather than a constant so that a test can start one that will refuse
/// the caller -- which is the only way to produce a *refusal* rather than an unreachable host.
struct Agent {
    child: Child,
    address: String,
    root: PathBuf,
}

impl Agent {
    fn start(token: &str) -> Self {
        let root = scratch_dir();
        let mut child = Command::new(agent_binary())
            .env("LINKLET_TOKEN", token)
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

        Self {
            child,
            address: format!("127.0.0.1:{port}"),
            root,
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

/// Runs `linklet exec --agents ...` with the token in the environment, as a person would.
fn exec_across(agents: &[&str], command: &str, token: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_linklet"))
        .env("LINKLET_TOKEN", token)
        .arg("exec")
        .arg("--agents")
        .arg(agents.join(","))
        .arg("--timeout")
        .arg("20")
        .arg(command)
        .output()
        .expect("the tool should run")
}

/// The stdout of a finished run, as lines.
fn lines(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn one_command_runs_on_every_agent_and_each_block_says_which_machine_it_came_from() {
    // The baseline, and the thing a caller actually reads. Two machines, one command, and the
    // output of each labelled with the target it came from -- a fan-out that concatenated the
    // outputs would leave a reader unable to tell which box said what.
    let first = Agent::start(TEST_TOKEN);
    let second = Agent::start(TEST_TOKEN);

    let output = exec_across(
        &[&first.address, &second.address],
        "echo from-the-fan-out",
        TEST_TOKEN,
    );

    assert_eq!(
        output.status.code(),
        Some(i32::from(ExitCode::SUCCESS)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text = lines(&output);
    assert_eq!(text[0], "2 of 2 ran", "{text:#?}");
    assert!(
        text.contains(&format!("ran {}", first.address)),
        "the first target's block is labelled: {text:#?}"
    );
    assert!(
        text.contains(&format!("ran {}", second.address)),
        "and so is the second's: {text:#?}"
    );

    // Both blocks carry the command's own output, which is what makes them worth printing.
    let echoed = text
        .iter()
        .filter(|line| line.contains("from-the-fan-out"))
        .count();
    assert_eq!(echoed, 2, "each machine echoed it: {text:#?}");
}

#[test]
fn a_machine_that_cannot_be_reached_does_not_stop_the_others() {
    // **The property no single-target test can show.** A run that stopped at the first
    // unreachable host would turn "which of these are up" into a sequence of calls, which is
    // what a fan-out exists to replace.
    let live = Agent::start(TEST_TOKEN);
    // Port 1 on loopback has nothing behind it, and the OS says so immediately.
    let dead = "127.0.0.1:1";

    let output = exec_across(&[&live.address, dead], "echo still-ran", TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(i32::from(ExitCode::NOT_ALL_ALIVE)),
        "one of them did not run"
    );

    let text = lines(&output);
    assert_eq!(text[0], "1 of 2 ran", "{text:#?}");
    assert!(
        text.iter().any(|line| line.contains("still-ran")),
        "the reachable machine still ran the command: {text:#?}"
    );
    assert!(
        text.iter()
            .any(|line| line == &format!("unreachable {dead}")),
        "and the unreachable one is named as unreachable: {text:#?}"
    );
}

#[test]
fn a_machine_that_refuses_is_not_a_machine_that_cannot_be_reached() {
    // **The distinction the report exists for.** An agent holding a different token answers,
    // understands, and says no -- and a caller told "unreachable" would go and look at the
    // network for a mistake in its own token. This is the one case that needs a second agent
    // configured with a secret the caller does not have.
    let refusing = Agent::start("a-different-secret-0123456789");

    let output = exec_across(&[&refusing.address], "echo x", TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(i32::from(ExitCode::NOT_ALL_ALIVE)),
        "the command did not run"
    );

    let text = lines(&output);
    assert_eq!(text[0], "0 of 1 ran", "{text:#?}");
    assert!(
        text.iter()
            .any(|line| line == &format!("refused {}", refusing.address)),
        "a refusal is a refusal and not an unreachable machine: {text:#?}"
    );
    assert!(
        !text.iter().any(|line| line.contains("unreachable")),
        "and it must not be reported as one: {text:#?}"
    );
}

#[test]
fn a_command_that_ran_and_failed_is_still_a_run() {
    // The rule the single-target path has kept since M6, kept across several machines: the
    // command's own exit status is the caller's business, and whether the *agents* answered is
    // this tool's. A fan-out that reported exit 7 as a failed fan-out would be telling the
    // caller something about the network that is not true.
    let agent = Agent::start(TEST_TOKEN);

    let output = exec_across(&[&agent.address], "exit 7", TEST_TOKEN);

    assert_eq!(
        output.status.code(),
        Some(i32::from(ExitCode::SUCCESS)),
        "the fan-out succeeded; the command is what failed"
    );
    let text = lines(&output);
    assert_eq!(text[0], "1 of 1 ran", "{text:#?}");
    assert!(
        text.iter().any(|line| line.contains("exit 7")),
        "and the command's own status is in the block it belongs to: {text:#?}"
    );
}

#[test]
fn one_agent_and_several_are_refused_when_both_are_given() {
    // Two intentions in one invocation, and guessing between them would run a command
    // somewhere the caller did not name.
    let output = Command::new(env!("CARGO_BIN_EXE_linklet"))
        .env("LINKLET_TOKEN", TEST_TOKEN)
        .args([
            "exec",
            "--agent",
            "127.0.0.1:1",
            "--agents",
            "127.0.0.1:2",
            "echo",
            "x",
        ])
        .output()
        .expect("the tool should run");

    assert_eq!(output.status.code(), Some(i32::from(ExitCode::USAGE)));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not both"), "{stderr}");
}
