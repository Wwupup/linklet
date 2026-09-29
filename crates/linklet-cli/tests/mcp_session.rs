//! The MCP server, driven as a real process over real pipes.
//!
//! The protocol cases that matter are unit-tested in
//! [`linklet_adapters::serve`]'s own module tests, in microseconds. What is left
//! here is the part that only exists once there is a process: that the framing
//! is one message per line in both directions, that a reply is flushed rather
//! than buffered until exit, and that the binary is reachable as `linklet mcp`.
//!
//! A server that buffers its output is a server that looks fine in a unit test
//! and hangs a client forever, which is why this file exists.

use std::io::Write;
use std::process::{Command, Stdio};

use linklet_core::ExitCode;
use linklet_core::json::{self, Json};

/// Sends each message to `linklet mcp` and returns the replies it produced.
///
/// The repository root, from this crate's directory.
///
/// `ancestors()` yields the path itself first, so the root is two steps up from
/// `crates/linklet-cli`.
fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate directory has a repository root two levels up")
        .to_path_buf()
}

/// Sends each message to `linklet mcp` and returns the replies it produced.
///
/// The working directory is set explicitly rather than inherited. `cargo test`
/// starts the binary in the *crate* directory, not the repository root, and a
/// test that assumed otherwise failed with "cannot read target/..." while the
/// file was demonstrably there -- the assertion was right and the assumption was
/// wrong. A tool whose whole job is reading relative paths should be handed a
/// known directory by the test that drives it.
///
/// The whole session is written at once rather than interactively, which is
/// enough to prove the framing: if replies were buffered until exit, they would
/// still arrive, and if the newline framing were wrong the parse below would
/// fail. Flushing is proven by the round trip completing at all.
fn session(messages: &[&str]) -> Vec<Json> {
    session_with_token(messages, None)
}

/// The same, with the host token in the environment.
///
/// One function with a token rather than two that differ in an environment variable: a
/// copy of this that forgot the token would fail with "the agent refused" and send a
/// reader looking at the transfer.
fn session_with_token(messages: &[&str], token: Option<&str>) -> Vec<Json> {
    let root = repo_root();
    assert!(
        root.join("Cargo.toml").is_file(),
        "expected the repository root at {}, and found no Cargo.toml there",
        root.display()
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_linklet"));
    command
        .arg("mcp")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        command.env("LINKLET_TOKEN", token);
    }

    let mut child = command.spawn().expect("the binary under test should start");

    {
        let stdin = child.stdin.as_mut().expect("stdin was piped");
        for message in messages {
            writeln!(stdin, "{message}").expect("writing a request");
        }
    }
    // Dropping stdin closes it, which is what ends the server's loop.

    let output = child.wait_with_output().expect("the server should finish");
    let text = String::from_utf8(output.stdout).expect("replies are UTF-8");

    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            json::parse(line)
                .unwrap_or_else(|e| panic!("a reply was not one JSON line: {e} in {line:?}"))
        })
        .collect()
}

/// A directory under the system temporary directory that removes itself.
///
/// A guard rather than a call at the end of the test, because the end of the test is
/// exactly what does not run when an assertion fails -- and a failing test is when a
/// leaked directory is most likely. The other transfer tests use the same shape; this
/// one leaked a directory until it did.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(name: String) -> Self {
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Where the agent binary is, worked out the way `push_pull.rs` does.
///
/// `CARGO_BIN_EXE_linklet-agent` does not exist in this package: that variable is defined
/// only for a binary of the crate being compiled. The duplication with `push_pull.rs` is
/// the price of two test files that both need an agent, and a shared test crate for
/// thirty lines would be a worse trade.
fn agent_binary() -> std::path::PathBuf {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate directory has a repository root two levels up")
        .to_path_buf();

    let target = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(dir) => std::path::PathBuf::from(dir),
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

/// One request, shaped for readability at the call site.
fn request(id: i64, method: &str, params: &str) -> String {
    if params.is_empty() {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}"}}"#)
    } else {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
    }
}

/// The reply to `id`, or a panic naming what arrived instead.
fn reply_for(replies: &[Json], id: i64) -> Json {
    replies
        .iter()
        .find(|reply| reply.get("id").and_then(Json::as_int) == Some(id))
        .unwrap_or_else(|| panic!("no reply with id {id} in {replies:#?}"))
        .clone()
}

#[test]
fn a_full_session_produces_one_reply_per_request_and_none_for_the_notification() {
    let initialize = request(
        1,
        "initialize",
        r#"{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}"#,
    );
    let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let list = request(2, "tools/list", "");
    let call = request(
        3,
        "tools/call",
        r#"{"name":"check","arguments":{"targets":["127.0.0.1:1"]}}"#,
    );

    let replies = session(&[&initialize, notification, &list, &call]);

    // Four messages in, three replies out. The notification gets nothing, which
    // is the assertion that a strict client would otherwise complain about once
    // per notification.
    assert_eq!(replies.len(), 3, "got {replies:#?}");
    assert!(
        replies
            .iter()
            .all(|reply| reply.get("id") != Some(&Json::Null)),
        "a reply carried a null id, which is what a reply to a notification looks like"
    );
}

#[test]
fn initialize_echoes_the_clients_protocol_version() {
    // Echoed rather than checked. Comparing the client's version against a
    // constant would break the day a client moves on, and the client is better
    // placed to decide whether it can read the replies.
    let initialize = request(
        1,
        "initialize",
        r#"{"protocolVersion":"2029-01-01","capabilities":{}}"#,
    );
    let replies = session(&[&initialize]);

    let result = reply_for(&replies, 1);
    assert_eq!(
        result
            .get("result")
            .and_then(|r| r.get_str("protocolVersion")),
        Some("2029-01-01")
    );
}

#[test]
fn the_tool_list_holds_one_tool_with_a_short_description() {
    let replies = session(&[&request(1, "tools/list", "")]);

    let tools = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Json::as_array)
        .expect("a tools array")
        .to_vec();

    // Five, asserted here as well as in the core. The core test checks the list
    // and the dispatcher agree; this one checks the list survives the wire, which
    // is the part a session can fail at and a unit test cannot.
    assert_eq!(tools.len(), 5, "the surface is five tools: {tools:#?}");
    let names: Vec<Option<&str>> = tools.iter().map(|tool| tool.get_str("name")).collect();
    assert_eq!(
        names,
        vec![
            Some("check"),
            Some("testbed"),
            Some("exec"),
            Some("push"),
            Some("pull")
        ]
    );

    for tool in &tools {
        let description = tool.get_str("description").expect("a description");
        assert!(
            description.len() < 80,
            "{}'s description is {} characters: {description:?}",
            tool.get_str("name").unwrap_or("?"),
            description.len()
        );
    }
}

#[test]
fn the_push_tool_refuses_a_local_path_outside_the_working_tree() {
    // The one decision the transfer tools make in this crate rather than handing to the
    // agent: which half is local, and that a local half may not name the machine. The
    // refusal has to arrive as a tool result rather than a protocol fault, because an
    // agent that gets a transport error retries and one that gets "not that argument"
    // changes it.
    let replies = session(&[&request(
        1,
        "tools/call",
        r#"{"name":"push","arguments":{"agent":"127.0.0.1:1","from":"C:\\Windows\\win.ini","to":"x"}}"#,
    )]);

    let result = reply_for(&replies, 1)
        .get("result")
        .expect("a tool result and not a protocol error")
        .clone();

    assert_eq!(
        result.get("isError").and_then(Json::as_bool),
        Some(true),
        "a refused argument is a failed call: {result:?}"
    );
    let text = result
        .get("content")
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|entry| entry.get_str("text"))
        .expect("the refusal is text");
    assert!(
        text.contains("working tree"),
        "the refusal should say what is wrong: {text}"
    );
}

#[test]
fn the_testbed_tool_answers_from_a_real_file() {
    // The end-to-end path for the second tool: a specification read from disk by
    // the process, judged against this machine, rendered back down the pipe.
    //
    // The file is written under the repository's own `target/`, and `session()`
    // sets the process's working directory to the repository root, so the
    // relative path below names the file the tool will read. Both halves are
    // needed: the tool refuses an absolute path on purpose, and `cargo test`
    // starts the binary in the crate directory unless told otherwise.
    //
    // `target/` is gitignored, so a failure that leaves the file behind leaves
    // nothing tracked. The artifact required is `Cargo.toml`, so the answer on a
    // healthy checkout is READY -- which also makes this catch being run
    // somewhere that is not the repository.
    let repo_root = repo_root();
    assert!(
        repo_root.join("Cargo.toml").is_file(),
        "expected the repository root at {}, and found no Cargo.toml there",
        repo_root.display()
    );

    let spec_path = repo_root.join("target/session-smoke.testbed");
    std::fs::write(
        &spec_path,
        "name session-smoke\nrequire artifact Cargo.toml present\n",
    )
    .expect("writing the spec");

    let call = request(
        1,
        "tools/call",
        r#"{"name":"testbed","arguments":{"spec":"target/session-smoke.testbed","target":"this-machine"}}"#,
    );
    let replies = session(&[&call]);

    let _ = std::fs::remove_file(&spec_path);

    let text = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get_str("text"))
        .expect("a text block")
        .to_string();

    assert!(
        text.starts_with("testbed session-smoke  target this-machine"),
        "got {text:?}"
    );
    assert!(
        text.ends_with("READY 1 of 1 requirements met"),
        "got {text:?}"
    );
}

#[test]
fn a_spec_path_outside_the_tree_is_refused_before_the_filesystem_sees_it() {
    let call = request(
        1,
        "tools/call",
        r#"{"name":"testbed","arguments":{"spec":"C:\\Windows\\win.ini","target":"box"}}"#,
    );
    let replies = session(&[&call]);

    let text = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get_str("text"))
        .expect("a text block")
        .to_string();

    assert!(text.contains("outside the working tree"), "got {text:?}");
}

#[test]
fn a_tool_call_reaches_the_network_and_answers_in_plain_text() {
    let call = request(
        1,
        "tools/call",
        r#"{"name":"check","arguments":{"targets":["127.0.0.1:1"]}}"#,
    );
    let replies = session(&[&call]);

    let result = reply_for(&replies, 1);
    let text = result
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get_str("text"))
        .expect("a text content block");

    // Port 1 on loopback: nothing listens, and the machine says so at once.
    assert!(text.starts_with("dead 127.0.0.1:1 "), "got {text:?}");
    assert!(
        text.contains("refused"),
        "the reason should survive: {text:?}"
    );
    assert!(text.ends_with("0 of 1 live"), "got {text:?}");
    assert_eq!(
        result
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Json::as_bool),
        Some(false),
        "bad news is not a failed call"
    );
}

#[test]
fn a_bad_tool_call_is_a_result_with_is_error_and_not_a_protocol_fault() {
    // The distinction MCP makes: the request was well formed and the answer is
    // "you called it wrong". Replying with a JSON-RPC error instead would tell
    // the agent the transport failed.
    let call = request(1, "tools/call", r#"{"name":"nope","arguments":{}}"#);
    let replies = session(&[&call]);

    let reply = reply_for(&replies, 1);
    assert!(reply.get("error").is_none(), "this is not a protocol error");
    assert_eq!(
        reply
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Json::as_bool),
        Some(true)
    );
}

#[test]
fn a_bad_argument_names_the_field() {
    // An agent retrying has to change something, and "invalid arguments" gives
    // it nothing to change.
    let call = request(
        1,
        "tools/call",
        r#"{"name":"check","arguments":{"targets":["127.0.0.1:1"],"timout":5}}"#,
    );
    let replies = session(&[&call]);

    let text = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get_str("text"))
        .expect("a text block")
        .to_string();

    assert!(text.contains("timout"), "the offending field: {text:?}");
}

#[test]
fn a_line_that_is_not_json_is_reported_and_the_session_continues() {
    // A server that dies on one bad line is a server a stray byte can kill. The
    // second request is the assertion -- it still gets an answer.
    let replies = session(&["not json at all", &request(2, "tools/list", "")]);

    assert_eq!(replies.len(), 2, "got {replies:#?}");
    assert_eq!(
        replies[0]
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Json::as_int),
        Some(-32700)
    );
    assert_eq!(
        replies[0].get("id"),
        Some(&Json::Null),
        "a parse failure has no id to quote"
    );
    assert!(
        replies[1].get("result").is_some(),
        "the session should still work"
    );
}

#[test]
fn an_unknown_method_is_reported_as_unsupported() {
    let replies = session(&[&request(1, "resources/list", "")]);

    assert_eq!(
        reply_for(&replies, 1)
            .get("error")
            .and_then(|e| e.get("code"))
            .and_then(Json::as_int),
        Some(-32601)
    );
}

#[test]
fn blank_lines_are_ignored_rather_than_answered() {
    // A client that sends a trailing newline should not get an error reply for
    // the emptiness.
    let replies = session(&["", "   ", &request(1, "tools/list", "")]);

    assert_eq!(replies.len(), 1, "got {replies:#?}");
}

#[test]
fn the_push_tool_moves_a_real_file_through_a_real_agent() {
    // The end of M7 from the surface an AI agent actually calls: a JSON-RPC request on
    // stdin, a sealed transfer over a socket, and a file on the target. Everything below
    // this is tested somewhere cheaper; what is only observable here is that the whole
    // chain is joined up at all.
    //
    // The local file has to be inside the working tree, because that is what the tool
    // refuses to leave -- and `session()` sets the working directory to the repository
    // root, so `target/...` is where it has to go. `target/` is gitignored, so a failure
    // that leaves it behind leaves nothing tracked.
    let token = "test-token-0123456789";
    let root = Scratch::new(format!("linklet-mcp-transfer-{}", std::process::id()));

    let local = repo_root().join("target/mcp-push-source.bin");
    std::fs::create_dir_all(local.parent().expect("a parent")).expect("target/ exists");
    let content = b"a file that crossed two processes";
    std::fs::write(&local, content).expect("writing the source");

    let mut agent = Command::new(agent_binary())
        .env("LINKLET_TOKEN", token)
        .arg("--port")
        .arg("0")
        .arg("--root")
        .arg(root.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the agent should start");

    let port = {
        use std::io::{BufRead, BufReader};
        let mut banner = String::new();
        BufReader::new(agent.stdout.take().expect("stdout was piped"))
            .read_line(&mut banner)
            .expect("the agent prints a banner");
        banner
            .split_whitespace()
            .find_map(|word| word.strip_prefix("0.0.0.0:"))
            .and_then(|text| text.parse::<u16>().ok())
            .unwrap_or_else(|| panic!("cannot read a port from {banner:?}"))
    };

    let call = request(
        1,
        "tools/call",
        &format!(
            r#"{{"name":"push","arguments":{{"agent":"127.0.0.1:{port}","from":"target/mcp-push-source.bin","to":"landed.bin"}}}}"#
        ),
    );
    let replies = session_with_token(&[&call], Some(token));

    let _ = agent.kill();
    let _ = agent.wait();

    let result = reply_for(&replies, 1)
        .get("result")
        .expect("a tool result")
        .clone();
    assert_eq!(result.get("isError").and_then(Json::as_bool), Some(false));
    let text = result
        .get("content")
        .and_then(Json::as_array)
        .and_then(|content| content.first())
        .and_then(|entry| entry.get_str("text"))
        .expect("the result is text");

    assert_eq!(
        std::fs::read(root.path().join("landed.bin")).expect("the file should have landed"),
        content,
        "the tool said {text:?}"
    );
    assert!(
        text.contains(&format!("{} bytes", content.len())) && text.contains("sha256 "),
        "the tool result should say what landed and what it hashes to: {text}"
    );

    let _ = std::fs::remove_file(&local);
}

#[test]
fn mcp_refuses_to_take_arguments() {
    // `linklet mcp extra` is a caller that has misunderstood the command, and
    // silently ignoring the argument would leave it believing otherwise.
    let output = Command::new(env!("CARGO_BIN_EXE_linklet"))
        .args(["mcp", "extra"])
        .output()
        .expect("the binary should run");

    assert_eq!(output.status.code(), Some(i32::from(ExitCode::USAGE)));
    assert!(String::from_utf8_lossy(&output.stderr).contains("takes no arguments"));
}
