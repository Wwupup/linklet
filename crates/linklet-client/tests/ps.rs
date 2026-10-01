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

use linklet_client::{AgentAddress, CallError, ps};
use linklet_core::auth::Token;
use linklet_core::process::{Filter, Incomplete, Listing};

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
