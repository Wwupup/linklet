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

use linklet_client::{AgentAddress, CallError, kill, ps, run, spawn};
use linklet_core::auth::Token;
use linklet_core::process::{Filter, Incomplete, Listing, ToKill};
use linklet_core::wire::{KillRequest, RunRequest, SpawnRequest};

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
/// is a process that outlives the request that started it, which is what `spawn` provides and
/// `exec` cannot. Until the caller has `spawn` in hand this is the honest way to produce one,
/// and the `exec` thread is left to finish on its own rather than joined.
///
/// **`executable` is a parameter because these tests run concurrently.** Two markers of the
/// same name are two processes no test can tell apart: the deploy-loop test looks for
/// `PING.EXE` and a spawn test that started one would make its empty-listing assertion fail
/// intermittently. Each caller passes an executable of its own for that reason.
fn start_a_marker(agent: &Agent, executable: &str) -> String {
    // Long enough that the test always stops it rather than racing it: the process has to
    // outlive the calls that look for it and stop it. `ping -n 40` was written first and the
    // race was lost -- the marker exited on its own between the `ps` and the `kill`, and the
    // failure read as the filter not matching.
    let command = format!("{executable} -n 600 127.0.0.1");
    let address = agent.address.clone();
    std::thread::spawn(move || {
        let _ = run(
            &address,
            &RunRequest {
                command,
                timeout_seconds: 60,
            },
        );
    });

    // The image name `cmd` starts for that command: `exec` runs through `cmd /C`, so the
    // process that lives is the program, not the shell.
    format!("{}.EXE", executable.to_uppercase())
}

#[test]
fn the_deploy_loop_can_be_closed_look_start_and_stop() {
    // **What M10 says cannot be done without a person.** Kill the old build, push, start,
    // confirm it stayed up -- and the two halves that were missing are `ps` and `kill`.
    // This closes the loop with a real process on a real machine: find it, confirm it is
    // running, stop it by pid, and confirm it is gone.
    let agent = Agent::start();
    let marker = start_a_marker(&agent, "ping");

    // Look. The wait is for the ping to actually exist; a `ps` immediately after the request
    // can beat the shell that has to start it. **The baseline is taken before the request**,
    // so what is looked for is a process that appeared, and not one another test is running.
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

// --- and starting the new one ------------------------------------------------

#[test]
fn a_spawn_returns_before_the_program_does_and_the_program_writes_its_own_file() {
    // **The difference from `exec`, and the trap M10 opens with.** `exec` blocking on a
    // long-running program holds the request, the connection and the agent's pipes; this
    // returns a pid at once, and the program it started writes to a file of its own -- which
    // is also how a caller reads it afterwards, with the `pull` that already exists.
    let agent = Agent::start();
    let output = agent.root.join("spawned.log");

    // What was running before, so the program can be identified by being new. The pid the
    // reply carries is the shell's -- `spawn` runs through `cmd`, exactly as `run` does --
    // and the shell is not what the caller wants to watch: it exits as soon as the program
    // is started, while the program keeps running. The pid is still the right handle to stop
    // the tree, which is what `kill` does with it.
    let before: Vec<u32> = ps(
        &agent.address,
        &Filter {
            name: Some("PING.EXE".to_string()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer")
    .processes
    .iter()
    .map(|process| process.pid)
    .collect();

    let started = std::time::Instant::now();
    let report = spawn(
        &agent.address,
        &SpawnRequest {
            // `ping` again, and the two tests cannot be confused for each other because each
            // looks only for pids that were not there before it started. `timeout /T` was
            // tried for a distinct executable and cannot be used: it refuses redirected
            // input, which is how `spawn` runs a command, so it exits at once.
            command: "ping -n 300 127.0.0.1".to_string(),
            output: output.to_string_lossy().into_owned(),
        },
    )
    .expect("the agent should answer");
    assert!(
        report.command.contains("ping"),
        "the command is echoed back: {report:#?}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "spawn waited {:?}, which is `exec` with another name",
        started.elapsed()
    );

    // And it is really running, which is the claim the reply deliberately does not make.
    // This is the `ps` in the deploy loop, and it is a separate call on purpose.
    let mut spawned = None;
    for _ in 0..40 {
        let listing = ps(
            &agent.address,
            &Filter {
                name: Some("PING.EXE".to_string()),
                ..Filter::any()
            },
        )
        .expect("the agent should answer");
        if let Some(process) = listing
            .processes
            .iter()
            .find(|process| !before.contains(&process.pid))
        {
            spawned = Some(process.pid);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let pid = spawned.unwrap_or_else(|| panic!("the spawned program never appeared in a listing"));

    // Stop it, so the test leaves nothing behind -- and by the pid the reply gave, which is
    // the shell: `taskkill /T` takes the program it started with it, which is why that is
    // the pid worth returning.
    let killed = kill(
        &agent.address,
        &KillRequest {
            to_kill: ToKill::Pid(report.pid),
            force: false,
            candidates: Filter::any(),
            exclude: None,
        },
    )
    .expect("the agent should answer");
    assert_eq!(killed.killed.len(), 1, "{killed:#?}");

    // The program itself is gone too, which is the part `/T` is for.
    let after = ps(
        &agent.address,
        &Filter {
            name: Some("PING.EXE".to_string()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");
    assert!(
        !after.processes.iter().any(|process| process.pid == pid),
        "the program outlived the shell that was stopped: {after:#?}"
    );

    // The output file the spawn was told to write. It is inside the agent's root, so the
    // existing `pull` reaches it -- which is why `spawn` needs no new way to read a file.
    //
    // Read after the program was stopped, so the file is complete; `ping` writes its first
    // line immediately, which is what this asserts is there.
    let text = std::fs::read_to_string(&output).unwrap_or_else(|error| {
        panic!("the program's own output file is not there: {error}");
    });
    assert!(
        text.contains("127.0.0.1"),
        "the program's own output should be in its own file: {text:?}"
    );
}

#[test]
fn a_spawn_output_path_is_relative_to_the_transfer_root() {
    // **Every other request resolves its path against the agent's transfer root, and `spawn`
    // did not.** It handed the string straight to `OpenOptions::open`, so a relative path
    // resolved against whatever directory the agent happened to be started in: the write was
    // refused with "Access is denied" from a system directory, or it quietly landed somewhere
    // nobody would look. Found by driving a real release candidate on a real machine, where
    // every form of relative path failed.
    //
    // The failure was invisible to tests because **both spawn tests passed an absolute path**,
    // and `agent.root.join(..)` produces exactly that. A test that only ever exercises the
    // form the caller does not use is not testing the interface.
    let agent = Agent::start();

    // The directory is made first, so that this test is about **where** the path resolves and
    // not about creating directories: `spawn` refuses a path it cannot write, which is a
    // different behaviour with its own test below.
    let nested = agent.root.join("nested");
    std::fs::create_dir(&nested).expect("a directory under the root");

    let report = spawn(
        &agent.address,
        &SpawnRequest {
            // `echo` rather than a long-running program: this test is about where the file
            // goes, and a program that exits at once still exercises the path.
            command: "echo relative".to_string(),
            // A relative path, which is what the protocol says this is and what every caller
            // sends: the tool's own schema calls it "a path on the target".
            output: "nested/spawned-relative.log".to_string(),
        },
    )
    .expect("a relative path under the root is where this belongs");

    assert!(report.pid > 0, "{report:#?}");

    // It is under the root, which is the whole claim.
    let written = nested.join("spawned-relative.log");
    assert!(
        written.exists(),
        "the output should be at {}, which is under the agent's root",
        written.display()
    );
}

#[test]
fn a_spawn_output_that_leaves_the_transfer_root_is_refused_by_name() {
    // The half that matters more than convenience. `spawn` created a file at a path of the
    // caller's choosing with **no `..` guard at all**, because the guard lives in
    // `Destination::resolve` and this path never went through it. Every other write in this
    // protocol is rooted; this one was not, so it could create a file anywhere the agent
    // could -- and it was only luck (the agent's working directory permissions) that made
    // the escape fail rather than succeed.
    let agent = Agent::start();

    let error = spawn(
        &agent.address,
        &SpawnRequest {
            command: "echo escaped".to_string(),
            output: r"..\escaped.log".to_string(),
        },
    )
    .expect_err("a path that leaves the root must not be written");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");
    let text = error.to_string();
    assert!(
        text.contains(".."),
        "the refusal should name what it refused, and said: {text}"
    );

    // And nothing was written outside the root. The root's parent is this test's scratch
    // directory, so a file there is a file that escaped.
    let escaped = agent
        .root
        .parent()
        .expect("the root has a parent")
        .join("escaped.log");
    assert!(
        !escaped.exists(),
        "{} must not exist: the agent wrote outside its root",
        escaped.display()
    );
}

#[test]
fn a_spawn_onto_an_output_file_it_cannot_write_is_refused_and_starts_nothing() {
    // The refusal has to come before the process exists, because a program started with
    // nowhere to write is the one thing this feature exists to prevent.
    let agent = Agent::start();
    let directory = agent.root.join("a-directory");
    std::fs::create_dir(&directory).expect("a directory to aim the output at");

    let error = spawn(
        &agent.address,
        &SpawnRequest {
            // A marker of its own again, so "nothing was started" cannot be about another
            // test's process: nothing in this test runs at all.
            command: "arp -a".to_string(),
            output: directory.to_string_lossy().into_owned(),
        },
    )
    .expect_err("a directory is not a file to write");

    assert!(matches!(error, CallError::Refused(_)), "{error:?}");

    // And nothing was started, which is the half of "refused" that matters: a program
    // started with nowhere to write is the one thing this feature exists to prevent.
    //
    // **Read over the wire and not from the filesystem.** The first version of this checked
    // `directory.read_dir()` -- the *test's* disk -- while the output path is on the
    // *agent's* machine. They are the same machine here, which is exactly why the mistake
    // was invisible, and it would have been wrong the moment they were not.
    let listing = ps(
        &agent.address,
        &Filter {
            name: Some("ARP.EXE".to_string()),
            ..Filter::any()
        },
    )
    .expect("the agent should answer");
    assert!(
        listing.processes.is_empty(),
        "a refused spawn started something: {listing:#?}"
    );
}
