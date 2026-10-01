//! `ps`, over a real socket, against a real agent on a real machine.
//!
//! What is checked here is the one thing the pure tests cannot: that a listing goes out
//! and comes back, and that the counts and notes survive the trip. The filter and the
//! report are tested in `linklet_core::process`, the parser in `linklet-adapters`, and
//! neither of those can tell whether the two implementations agree about the wire.
//!
//! **The process to look for is this test binary.** That is the trick that makes this
//! layer possible at all: `ps` on the agent is a question about a machine, and the machine
//! running the tests has exactly one process whose name nobody else will be running while
//! the test is. Spawning a marker would also work and would cost a process, a port and a
//! cleanup path -- and it would exercise exactly the same code.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use linklet_client::{AgentAddress, CallError, kill, ps, run};
use linklet_core::auth::Token;
use linklet_core::process::{Filter, Incomplete, Listing, ToKill};
use linklet_core::wire::{KillRequest, RunRequest};

/// The token these tests configure the agent with.
const TEST_TOKEN: &str = "test-token-0123456789";

/// Where the agent binary is, worked out the way `against_agent.rs` does it.
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

    let path = target.join("debug/linklet-agent.exe");
    assert!(
        path.is_file(),
        "the agent binary is not at {}; run `cargo build --workspace` first",
        path.display()
    );
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
        "linklet-ps-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// This test binary's own image name, which is the process `ps` has to find.
fn own_image_name() -> String {
    std::env::current_exe()
        .expect("a running process knows its own path")
        .file_name()
        .expect("and it has a file name")
        .to_string_lossy()
        .into_owned()
}

#[test]
fn a_listing_finds_this_test_process_by_name() {
    // The whole request, over a socket, through the agent's own `tasklist`, and back. A
    // filter that matched nothing here would be a filter that does not work at all.
    let agent = Agent::start();
    let name = own_image_name();

    let listing: Listing = ps(
        &agent.address,
        &Filter {
            name: Some(name.clone()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");

    assert!(
        listing
            .processes
            .iter()
            .any(|process| process.name.eq_ignore_ascii_case(&name)),
        "the process that asked is not in the answer: {listing:#?}"
    );
    assert!(
        listing.total >= listing.count(),
        "the total is how many were examined and cannot be smaller than the answer"
    );
    assert_eq!(
        listing.applied.get_str("name"),
        Some(name.as_str()),
        "the filter has to come back with the answer"
    );
    assert_eq!(
        listing.incomplete(),
        Incomplete::No,
        "a listing that could read its input: {listing:#?}"
    );
}

#[test]
fn a_filter_that_matches_nothing_still_says_what_it_looked_at() {
    // **The property M10 is about, over a socket.** An empty list on its own is the shape
    // that was one step from a wrong conclusion on a real machine; the total and the echoed
    // filter are what make it readable, and both have to survive the trip.
    let agent = Agent::start();

    let listing = ps(
        &agent.address,
        &Filter {
            name: Some("no-such-process-anywhere.exe".to_string()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");

    assert_eq!(listing.count(), 0, "nothing matches that name");
    assert!(
        listing.total > 0,
        "and this machine has processes, which is the fact that matters: {listing:#?}"
    );
    assert_eq!(
        listing.applied.get_str("name"),
        Some("no-such-process-anywhere.exe"),
        "the filter has to come back, or the empty list means nothing"
    );
    assert_eq!(
        listing.incomplete(),
        Incomplete::No,
        "the machine was read completely; the answer is simply empty"
    );
}

#[test]
fn an_empty_filter_answers_with_the_machine_and_a_complete_listing() {
    let agent = Agent::start();
    let listing = ps(&agent.address, &Filter::any()).expect("the agent should answer");

    assert!(listing.total > 0, "a running machine has processes");
    assert!(listing.truncated, "and more of them than one reply carries");
    assert_eq!(listing.count(), linklet_core::process::MAX_LISTED);
    assert!(listing.applied_is_empty());
}

#[test]
fn a_field_this_implementation_cannot_supply_is_reported_and_not_defaulted() {
    // `tasklist` gives no command line, so a caller that filters on one is told rather than
    // handed an empty list. Asking for `cmdline` is the case `docs/ROADMAP.md` M10 records
    // from a real machine, and it is the one where a silent empty answer would be believed.
    let agent = Agent::start();

    let listing = ps(
        &agent.address,
        &Filter {
            cmdline: Some("--port".to_string()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");

    assert_eq!(listing.count(), 0, "no process could be checked");
    assert!(listing.total > 0, "and there were processes to check");
    assert_eq!(
        listing.incomplete(),
        Incomplete::No,
        "the machine was read fine; the field was not available"
    );
    assert!(
        listing.notes.iter().any(|note| note.contains("cmdline")),
        "the note has to name the field that could not be checked: {listing:#?}"
    );
}

#[test]
fn a_ps_call_to_a_machine_with_no_agent_is_a_transport_failure_and_not_an_empty_list() {
    // The distinction the deploy loop turns on: "nothing is running" and "I could not ask"
    // lead to opposite actions, and only one of them is safe.
    let address = AgentAddress::new("127.0.0.1:1")
        .expect("a valid address")
        .with_token(Token::new(TEST_TOKEN).expect("a usable test token"));

    let error = ps(&address, &Filter::any()).expect_err("nothing is listening on port 1");

    assert!(
        matches!(error, CallError::Transport(_) | CallError::NoReply { .. }),
        "{error:?}"
    );
}

// --- and the loop the whole item is for --------------------------------------

/// Starts a process on the agent's machine that lives for about `minutes`, and returns the
/// name to look for it by.
///
/// **Started through `exec` and deliberately not waited for.** `exec` blocks until the
/// command exits, so this runs on its own thread and the test proceeds: what is being set up
/// is a process that outlives the request that started it, which is exactly the shape
/// `spawn` will eventually provide. Until then this is the honest way to produce one, and it
/// is why the `exec` thread is left to finish on its own rather than joined.
fn start_a_marker(agent: &Agent) -> String {
    // Long enough that the test always stops it rather than racing it: the process has to
    // outlive the two calls that look for it and stop it. `ping -n 40` was written first and
    // the race was lost -- the marker exited on its own between the `ps` and the `kill`, and
    // the failure read as the filter not matching.
    let command = "ping -n 600 127.0.0.1";
    let address = agent.address.clone();
    std::thread::spawn(move || {
        let _ = run(
            &address,
            &RunRequest {
                command: command.to_string(),
                timeout_seconds: 60,
            },
        );
    });

    // The image name of what `cmd` starts for that command. `exec` runs through `cmd /C`,
    // so the process that lives is the ping, not the shell.
    "PING.EXE".to_string()
}

#[test]
fn the_deploy_loop_can_be_closed_look_start_and_stop() {
    // **What M10 says cannot be done without a person.** Kill the old build, push, start,
    // confirm it stayed up -- and the two halves that were missing are `ps` and `kill`.
    // This closes the loop with a real process on a real machine: find it, confirm it is
    // running, stop it by pid, and confirm it is gone.
    let agent = Agent::start();
    let marker = start_a_marker(&agent);

    // Look. The wait is for the ping to actually exist; a `ps` immediately after the request
    // can beat the shell that has to start it.
    let mut found = None;
    for _ in 0..40 {
        let listing = ps(
            &agent.address,
            &Filter {
                name: Some(marker.clone()),
                ..Filter::any()
            },
        )
        .expect("the agent should answer");
        if let Some(process) = listing.processes.first() {
            found = Some(process.pid);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let pid = found.unwrap_or_else(|| panic!("{marker} never appeared in a listing"));

    // Stop it, by number -- the one request that is never gated, because a pid is one
    // process and the caller has already said which.
    let report = kill(
        &agent.address,
        &KillRequest {
            to_kill: ToKill::Pid(pid),
            force: false,
            candidates: Filter::any(),
            exclude: None,
        },
    )
    .expect("the agent should answer");

    assert_eq!(report.matched, 1, "{report:#?}");
    assert_eq!(report.killed.len(), 1, "it should be gone: {report:#?}");
    assert_eq!(report.killed[0].pid, pid);
    assert!(report.complete(), "{report:#?}");

    // And confirm it. Looking at the machine again is the only thing that makes the report
    // evidence rather than a claim.
    let after = ps(
        &agent.address,
        &Filter {
            name: Some(marker.clone()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");
    assert!(
        !after.processes.iter().any(|process| process.pid == pid),
        "the process is still there after being killed: {after:#?}"
    );
}

#[test]
fn a_request_that_would_stop_the_agent_is_refused_over_the_socket() {
    // The guard, end to end: refused before `taskkill` runs, and the refusal reaches the
    // caller as a refusal rather than as a transport failure -- which matters here more than
    // anywhere, because the one process that could not answer is the one being asked about.
    let agent = Agent::start();

    let error = kill(
        &agent.address,
        &KillRequest {
            to_kill: ToKill::Matching("linklet-agent".to_string()),
            force: true,
            candidates: Filter::any(),
            exclude: None,
        },
    )
    .expect_err("this would stop the agent");

    let CallError::Refused(reason) = &error else {
        panic!("a refusal, and not a dropped connection: {error:?}");
    };
    assert!(reason.contains("agent"), "{reason}");

    // And the agent is still serving, which is the fact the refusal was protecting.
    assert!(
        ps(&agent.address, &Filter::any()).is_ok(),
        "the agent should still be answering"
    );
}

#[test]
fn a_bulk_match_that_was_not_forced_is_refused_over_the_socket() {
    let agent = Agent::start();

    let error = kill(
        &agent.address,
        &KillRequest {
            to_kill: ToKill::Matching("explorer".to_string()),
            force: false,
            candidates: Filter::any(),
            exclude: None,
        },
    )
    .expect_err("a bulk match without --yes");

    let CallError::Refused(reason) = &error else {
        panic!("a refusal, and not a dropped connection: {error:?}");
    };
    assert!(reason.contains("--yes"), "{reason}");
}

#[test]
fn a_pid_that_is_not_running_is_a_report_of_nothing_and_not_a_failure() {
    // "Make sure it is gone" is an ordinary intent, and a machine where it never existed is
    // that intent already satisfied. What a caller must not be handed either way is a
    // transport failure, which would send it looking at the network.
    let agent = Agent::start();

    let report = kill(
        &agent.address,
        &KillRequest {
            to_kill: ToKill::Pid(4_000_000),
            force: false,
            candidates: Filter::any(),
            exclude: None,
        },
    )
    .expect("the agent should answer");

    assert_eq!(report.matched, 0, "{report:#?}");
    assert!(report.matched_nothing());
    assert!(report.complete(), "nothing was left running");
}
