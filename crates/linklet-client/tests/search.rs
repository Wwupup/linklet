//! `grep` and `tail`, over a real socket, against a real agent on a real machine.
//!
//! The pure decisions are tested in `linklet_core::search` and the reading in
//! `linklet-adapters`: the sniff, the pattern, the modes and the counts are tables there.
//! What is left for this layer is what only two processes can show -- that a file on the
//! agent's machine is read, sniffed, decoded and reported, and that the two lessons
//! `docs/ROADMAP.md` M10 copies from the sibling project survive the trip:
//!
//! 1. a failed search does not read as "no matches";
//! 2. the encoding is named, so a reader who is shown nonsense knows which rule produced it.
//!
//! The harness is deliberately a copy of the one in `ps.rs` rather than a shared module: a
//! test binary is a program, integration tests cannot import from one another, and a module
//! shared by two tests is the `tests/common/` arrangement this repository does not have.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use linklet_client::{AgentAddress, grep, ls, tail};
use linklet_core::auth::Token;
use linklet_core::search::{Direction, Encoding, Limit, Pattern};
use linklet_core::wire::{GrepRequest, LsRequest, TailRequest};

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
        "linklet-search-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("a scratch directory");
    path
}

/// A grep request with the defaults a caller would get.
fn grep_for(path: &str, text: &str, direction: Direction) -> GrepRequest {
    GrepRequest {
        path: path.to_string(),
        pattern: Pattern::new(text),
        direction,
        limit: Limit {
            max_matches: 20,
            context: 0,
        },
    }
}

#[test]
fn a_search_finds_a_line_and_reports_what_it_read() {
    // The baseline: a file on the agent's machine is read there and only the matches come
    // back. Everything below is about what happens when that is not the whole story.
    let agent = Agent::start();
    std::fs::write(
        agent.root.join("build.log"),
        "started\nok\nERROR: one\nmore\nERROR: two\ndone\n",
    )
    .expect("a log to search");

    let search = grep(
        &agent.address,
        &grep_for("build.log", "ERROR", Direction::First),
    )
    .expect("the agent should answer");

    assert!(search.searched, "{search:#?}");
    assert_eq!(search.total, Some(2));
    assert_eq!(search.path, "build.log");
    assert_eq!(search.encoding, Encoding::Utf8);
    // The size the machine reported, checked against the file this test wrote rather than
    // against a number copied out of a failing run.
    assert_eq!(
        search.file_bytes,
        Some(
            std::fs::metadata(agent.root.join("build.log"))
                .expect("the log the test wrote")
                .len()
        )
    );
    assert_eq!(
        search
            .lines
            .iter()
            .map(|line| (line.number, line.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(3, "ERROR: one"), (5, "ERROR: two")]
    );
}

#[test]
fn a_file_that_cannot_be_searched_is_not_a_file_with_no_matches() {
    // **The first lesson of `docs/ROADMAP.md` M10, over a socket.** A caller that reads an
    // empty list as "this log has no errors" will conclude a machine is clean on no
    // evidence, and the difference has to survive the trip.
    let agent = Agent::start();

    let search = grep(
        &agent.address,
        &grep_for("missing.log", "ERROR", Direction::First),
    )
    .expect("the agent should answer");

    assert!(!search.searched, "{search:#?}");
    assert_eq!(search.total, None, "nothing was counted");
    assert!(search.lines.is_empty());
    assert!(
        search
            .problem
            .as_deref()
            .is_some_and(|p| p.contains("missing.log")),
        "the reason should name the file: {search:#?}"
    );

    // And the rendering a caller reads leads with the failure rather than with a count.
    let rendered = linklet_core::search::render(&search);
    assert!(rendered.starts_with("could not search"), "{rendered}");
}

#[test]
fn a_path_that_leaves_the_transfer_root_is_refused_like_a_pull() {
    // The root is what makes this a read of a directory rather than a read of the machine,
    // and T1 of `docs/transfer.md` is why: a `..` that must not be written must not be read
    // either. The refusal is a failed search, not a transport error.
    let agent = Agent::start();

    let search = grep(
        &agent.address,
        &grep_for(
            r"..\..\Windows\System32\drivers\etc\hosts",
            "localhost",
            Direction::First,
        ),
    )
    .expect("the agent should answer");

    assert!(!search.searched, "{search:#?}");
    assert!(
        search.problem.as_deref().is_some_and(|p| p.contains("..")),
        "the refusal should name what it refused: {search:#?}"
    );
}

#[test]
fn a_file_that_is_not_utf8_is_decoded_by_a_rule_that_is_named() {
    // **The second lesson, over a socket.** These four bytes are GBK for two CJK characters;
    // they are not UTF-8, and a reader handed them as mojibake without being told has a broken
    // answer that looks like a working one.
    //
    // What is asserted is the label and the round trip, not the characters: the rule differs
    // between machines, and pinning the text would pin the test to this one.
    //
    // **The two platforms apply different rules and both name theirs.** Windows asks the
    // machine for its code page, which is right there because the machine has one and that is
    // what wrote the file. A Linux machine's default encoding is UTF-8, so these bytes are
    // precisely the ones it has no rule for; the rule applied is ISO-8859-1, which is total and
    // reversible -- one character per byte, nothing dropped. The claim that holds on both is
    // the one this test is named for: **the reader is told which rule produced the text, and no
    // byte is lost.**
    let agent = Agent::start();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"first\r\n");
    bytes.extend_from_slice(&[0xd6, 0xd0, 0xce, 0xc4]);
    bytes.extend_from_slice(b"\r\nlast\r\n");
    std::fs::write(agent.root.join("gbk.log"), &bytes).expect("a file of GBK bytes");

    let search = grep(&agent.address, &grep_for("gbk.log", "", Direction::First))
        .expect("the agent should answer");

    assert!(search.searched, "the file was read: {search:#?}");
    assert_eq!(
        search.encoding,
        if cfg!(windows) {
            Encoding::Oem
        } else {
            Encoding::Latin1
        },
        "the label has to say which rule was applied, and it differs by platform: {search:#?}"
    );

    let text: String = search
        .lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains('\u{fffd}'),
        "no byte may be replaced by a mark that means 'this was lost': {text:?}"
    );
    assert!(
        text.contains("last"),
        "and the ASCII around it is intact: {text:?}"
    );
}

#[test]
fn the_last_mode_answers_about_the_end_of_the_file() {
    // The question this exists for: "where is the last ERROR", without moving the file.
    let agent = Agent::start();
    let mut text = String::new();
    for index in 0..50 {
        text.push_str(&format!("line {index}\n"));
    }
    text.push_str("ERROR: the first\n");
    text.push_str("filler\n");
    text.push_str("ERROR: the last\n");
    std::fs::write(agent.root.join("long.log"), &text).expect("a log to search");

    let search = grep(
        &agent.address,
        &grep_for("long.log", "ERROR", Direction::Last),
    )
    .expect("the agent should answer");

    assert_eq!(search.total, Some(2), "both are inside the limit");
    assert_eq!(
        search.lines[1].text, "ERROR: the last",
        "the mode decides which end is read: {search:#?}"
    );
}

#[test]
fn a_file_past_the_ceiling_is_read_from_its_end_and_says_it_was_cut_short() {
    // **Why `grep --last` exists and a pull does not answer it.** The answer is near the end
    // of a large file, so a `last` search takes its window from there. A search from the
    // front would answer about the first sixteen mebibytes -- not a smaller answer, a wrong
    // one -- and either way the truncation has to be visible, because a silent window is how
    // "no matches" becomes a conclusion.
    let agent = Agent::start();
    let path = agent.root.join("big.log");

    // A marker at each end with more than the ceiling between them, written in chunks so the
    // test does not build seventeen megabytes in one allocation.
    let filler = vec![b'x'; 64 * 1024];
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&path).expect("a file to fill");
        file.write_all(b"FRONT-MARKER\n").expect("the front");
        // Two mebibytes more than the adapter's ceiling, in whole lines.
        for _ in 0..(18 * 16) {
            file.write_all(&filler).expect("filler");
            file.write_all(b"\n").expect("a line break");
        }
        file.write_all(b"BACK-MARKER\n").expect("the back");
    }

    let search = grep(
        &agent.address,
        &grep_for("big.log", "MARKER", Direction::Last),
    )
    .expect("the agent should answer");

    assert!(search.searched, "{search:#?}");
    assert!(search.truncated, "the window was the end of a large file");
    assert!(
        search
            .lines
            .iter()
            .any(|line| line.text.contains("BACK-MARKER")),
        "the end of the file should be in the window: {:#?}",
        search.lines
    );
    assert!(
        !search
            .lines
            .iter()
            .any(|line| line.text.contains("FRONT-MARKER")),
        "and the front is outside it, which is what a last search is for"
    );
}

#[test]
fn an_empty_directory_is_not_a_directory_that_is_not_there() {
    // **The distinction `ls` exists for, over a socket.** Both answers hold no entries and
    // they are opposite facts: one says the machine has no logs, the other says nobody
    // looked. A caller that confuses them stops looking for a file that is there.
    let agent = Agent::start();
    std::fs::create_dir(agent.root.join("empty")).expect("an empty directory");

    let present = ls(
        &agent.address,
        &LsRequest {
            path: "empty".to_string(),
        },
    )
    .expect("the agent should answer");

    assert!(present.found, "{present:#?}");
    assert!(present.entries.is_empty());
    assert_eq!(present.total, 0);
    assert!(present.problem.is_none());

    let absent = ls(
        &agent.address,
        &LsRequest {
            path: "not-there".to_string(),
        },
    )
    .expect("the agent should answer");

    assert!(!absent.found, "a directory that is not there: {absent:#?}");
    assert!(absent.entries.is_empty());
    assert!(
        absent
            .problem
            .as_deref()
            .is_some_and(|problem| problem.contains("not-there")),
        "the reason should name the path: {absent:#?}"
    );

    // And the two renderings cannot be confused for each other either.
    assert_eq!(
        linklet_core::listing::render(&present),
        "0 of 0 entries in empty"
    );
    assert!(linklet_core::listing::render(&absent).starts_with("could not list"));
}

#[test]
fn a_listing_names_what_is_there_and_puts_directories_first() {
    let agent = Agent::start();
    std::fs::write(agent.root.join("build.log"), b"12 bytes here").expect("a file");
    std::fs::create_dir(agent.root.join("archive")).expect("a directory");

    let listing = ls(
        &agent.address,
        &LsRequest {
            // `.` and not the empty string: the agent refuses an empty path, which is right
            // -- a request with no path is one that arrived wrong -- and `.` is how a caller
            // says "the root itself".
            path: ".".to_string(),
        },
    )
    .expect("the agent should answer");

    assert!(listing.found, "{listing:#?}");
    let names: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["archive", "build.log"], "directories first");

    let file = listing
        .entries
        .iter()
        .find(|entry| entry.name == "build.log")
        .expect("the file");
    assert_eq!(file.size, Some(13));
    assert!(!file.dir);
    assert!(file.modified.is_some(), "a real file has a real time");

    let directory = listing
        .entries
        .iter()
        .find(|entry| entry.name == "archive")
        .expect("the directory");
    assert!(directory.dir);
    assert_eq!(
        directory.size, None,
        "a directory has no size a reader wants"
    );
}

#[test]
fn a_single_file_lists_as_one_entry() {
    // "Is it there, and how big is it" is a legitimate question, and answering it with "that
    // is not a directory" would make the caller guess a different command to ask it.
    let agent = Agent::start();
    std::fs::write(agent.root.join("build.log"), b"12345").expect("a file");

    let listing = ls(
        &agent.address,
        &LsRequest {
            path: "build.log".to_string(),
        },
    )
    .expect("the agent should answer");

    assert!(listing.found, "{listing:#?}");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].name, "build.log");
    assert_eq!(listing.entries[0].size, Some(5));
}

#[test]
fn a_listing_outside_the_transfer_root_is_refused() {
    // T1 of `docs/transfer.md`: a path that must not be written must not be enumerated
    // either, or a listing becomes a way to map a machine this agent was not given.
    let agent = Agent::start();

    let listing = ls(
        &agent.address,
        &LsRequest {
            path: r"..\..\Windows".to_string(),
        },
    )
    .expect("the agent should answer");

    assert!(!listing.found, "{listing:#?}");
    assert!(
        listing
            .problem
            .as_deref()
            .is_some_and(|problem| problem.contains("..")),
        "the refusal should name what it refused: {listing:#?}"
    );
}

#[test]
fn a_tail_reports_lines_and_not_matches() {
    // `tail` goes through the same machinery as a search -- the counts, the truncation and
    // the encoding are the same questions whatever was asked -- and it must not report
    // itself as a search. `more than 2 matches` for a request that asked for the last two
    // lines is a small untruth about what the caller asked for.
    let agent = Agent::start();
    std::fs::write(agent.root.join("app.log"), "one\ntwo\nthree\nfour\n").expect("a log to read");

    let search = tail(
        &agent.address,
        &TailRequest {
            path: "app.log".to_string(),
            count: 2,
        },
    )
    .expect("the agent should answer");

    assert!(search.searched, "{search:#?}");
    assert_eq!(search.noun, linklet_core::search::Noun::Lines);
    let rendered = linklet_core::search::render(&search);
    assert!(rendered.contains("lines in app.log"), "{rendered}");
    assert!(!rendered.contains("matches"), "{rendered}");
    assert_eq!(
        search
            .lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        vec!["three", "four"],
        "the last two, in the order they appear"
    );
}
