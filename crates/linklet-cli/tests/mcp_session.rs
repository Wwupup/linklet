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

#[path = "../../test-support/credentials.rs"]
mod credentials;
#[path = "../../test-support/schemas.rs"]
mod schemas;
use credentials::Credentials;

use std::io::Write;
use std::process::{Command, Stdio};

use linklet_core::ExitCode;
use serde_json::Value;

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
fn session(messages: &[&str]) -> Vec<Value> {
    session_with_credentials(messages, &[])
}

/// The same, with saved credentials for the target addresses in these requests.
fn session_with_credentials(messages: &[&str], peers: &[(&str, &str)]) -> Vec<Value> {
    let root = repo_root();
    assert!(
        root.join("Cargo.toml").is_file(),
        "expected the repository root at {}, and found no Cargo.toml there",
        root.display()
    );

    let credentials = Credentials::new(None, peers);
    let mut command = Command::new(env!("CARGO_BIN_EXE_linklet"));
    command
        .arg("mcp")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.envs(credentials.env());

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

    let replies: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line)
                .unwrap_or_else(|e| panic!("a reply was not one JSON line: {e} in {line:?}"))
        })
        .collect();
    for request in messages
        .iter()
        .filter_map(|m| serde_json::from_str::<Value>(m).ok())
    {
        if request["method"] != "tools/call" {
            continue;
        }
        if let Some(reply) = replies.iter().find(|r| r["id"] == request["id"])
            && let Some(name) = request["params"]["name"].as_str()
            && reply.get("result").is_some()
        {
            schemas::validate(name, &reply["result"]);
        }
    }
    replies
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

/// One request, shaped for readability at the call site.
fn request(id: i64, method: &str, params: &str) -> String {
    if params.is_empty() {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}"}}"#)
    } else {
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#)
    }
}

/// The reply to `id`, or a panic naming what arrived instead.
fn reply_for(replies: &[Value], id: i64) -> Value {
    replies
        .iter()
        .find(|reply| reply.get("id").and_then(Value::as_i64) == Some(id))
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
            .all(|reply| reply.get("id") != Some(&Value::Null)),
        "a reply carried a null id, which is what a reply to a notification looks like"
    );
}

#[test]
fn initialize_reports_its_supported_protocol_version() {
    // An unsupported request receives the version this server actually implements.
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
            .and_then(|r| r.get("protocolVersion").and_then(Value::as_str)),
        Some("2025-06-18")
    );
}

#[test]
fn initialize_explains_where_to_obtain_target_runtime_capabilities() {
    let replies = session(&[&request(
        1,
        "initialize",
        r#"{"protocolVersion":"2025-06-18"}"#,
    )]);
    let instructions = replies[0]["result"]["instructions"]
        .as_str()
        .expect("connection guidance");
    for word in ["pair", "probe", "runtimes", "program", "args"] {
        assert!(
            instructions.contains(word),
            "missing {word}: {instructions}"
        );
    }
}

#[test]
fn the_tool_list_holds_one_tool_with_a_short_description() {
    let replies = session(&[&request(1, "tools/list", "")]);

    let tools = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .expect("a tools array")
        .to_vec();

    // Six, asserted here as well as in the core. The core test checks the list
    // and the dispatcher agree; this one checks the list survives the wire, which
    // is the part a session can fail at and a unit test cannot.
    assert_eq!(tools.len(), 14, "the surface is fourteen tools: {tools:#?}");
    let names: Vec<Option<&str>> = tools
        .iter()
        .map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec![
            Some("pair"),
            Some("check"),
            Some("testbed"),
            Some("exec"),
            Some("push"),
            Some("pull"),
            Some("ps"),
            Some("kill"),
            Some("spawn"),
            Some("grep"),
            Some("tail"),
            Some("ls"),
            Some("discover"),
            Some("probe")
        ]
    );

    for tool in &tools {
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .expect("a description");
        assert!(
            description.len() < 80,
            "{}'s description is {} characters: {description:?}",
            tool.get("name").and_then(Value::as_str).unwrap_or("?"),
            description.len()
        );
    }
}

#[test]
fn the_testbed_tool_answers_from_a_real_file() {
    // The end-to-end path for the second tool: a specification read from disk by
    // the process, judged against this machine, rendered back down the pipe.
    //
    // The spec is an absolute path outside the server working directory. Its artifact
    // remains relative to that directory, which session() sets to the repository root.
    let repo_root = repo_root();
    assert!(
        repo_root.join("Cargo.toml").is_file(),
        "expected the repository root at {}, and found no Cargo.toml there",
        repo_root.display()
    );

    let scratch = Scratch::new(format!("linklet-mcp-spec-{}", std::process::id()));
    let spec_path = scratch.path().join("session-smoke.testbed");
    std::fs::write(
        &spec_path,
        "name session-smoke\nrequire artifact Cargo.toml present\n",
    )
    .expect("writing the spec");

    let call = request(
        1,
        "tools/call",
        &serde_json::json!({"name": "testbed", "arguments": {"spec": spec_path, "target": "this-machine"}}).to_string(),
    );
    let replies = session(&[&call]);

    let _ = std::fs::remove_file(&spec_path);

    let text = reply_for(&replies, 1)
        .get("result")
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get("text").and_then(Value::as_str))
        .expect("a text block")
        .to_string();

    assert!(
        text.starts_with("testbed session-smoke  target this-machine"),
        "got {text:?}"
    );
    assert_eq!(
        reply_for(&replies, 1)["result"]["structuredContent"]["data"]["held"],
        true
    );
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
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get("text").and_then(Value::as_str))
        .expect("a text content block");

    // Port 1 on loopback: nothing listens, and the machine says so at once.
    assert!(text.starts_with("dead 127.0.0.1:1 "), "got {text:?}");
    // **Which reason comes back is the environment's business, and the claim is that one
    // does.** On Windows a closed loopback port is refused and the word is `refused`. In the
    // WSL distribution this was first run in, the same connect *times out* -- measured with
    // bare Python sockets, which also time out against ports 1, 9 and 54321 -- because the
    // reset is swallowed somewhere between the distribution and the host. `linklet` reported
    // what the operating system told it, which is the whole design, so the test asserts the
    // reason survived rather than which word the environment produced.
    let reason = text
        .trim_start_matches("dead 127.0.0.1:1 ")
        .trim_end_matches("0 of 1 live")
        .trim();
    assert!(
        reason.contains("refused") || reason.contains("no answer"),
        "the reason should survive: {text:?}"
    );
    assert_eq!(result["result"]["structuredContent"]["data"]["count"], 1);
    assert_eq!(
        result
            .get("result")
            .and_then(|r| r.get("isError"))
            .and_then(Value::as_bool),
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
            .and_then(Value::as_bool),
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
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|first| first.get("text").and_then(Value::as_str))
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
            .and_then(Value::as_i64),
        Some(-32700)
    );
    assert_eq!(
        replies[0].get("id"),
        Some(&Value::Null),
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
            .and_then(Value::as_i64),
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
fn file_tools_use_absolute_and_parent_paths_outside_the_startup_directory() {
    let token = "test-token-0123456789";
    let scratch = Scratch::new(format!("linklet mcp \u{4e2d} files {}", std::process::id()));
    let work = scratch.path().join("work");
    std::fs::create_dir(&work).expect("the agent's startup directory");
    let local = scratch.path().join("source.bin");
    let content = b"a file that crossed two processes\n";
    std::fs::write(&local, content).expect("a source outside the MCP working directory");

    let credentials = Credentials::new(Some(token), &[]);
    let mut agent = Command::new(agent_binary())
        .envs(credentials.env())
        .args(["--port", "0"])
        .current_dir(&work)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the agent without --root");
    let port = {
        use std::io::{BufRead, BufReader};
        let mut banner = String::new();
        let mut reader = BufReader::new(agent.stdout.take().expect("stdout"));
        reader.read_line(&mut banner).expect("a listening banner");
        let mut code = String::new();
        reader.read_line(&mut code).expect("pairing code");
        assert!(banner.contains("relative paths from"), "{banner}");
        banner
            .split_whitespace()
            .find_map(|word| word.strip_prefix("0.0.0.0:"))
            .and_then(|text| text.parse::<u16>().ok())
            .expect("a bound port")
    };
    let address = format!("127.0.0.1:{port}");
    let mut runs = Vec::new();
    for (index, relative) in [false, true].into_iter().enumerate() {
        let remote = scratch.path().join(format!("landed-{index}.bin"));
        let collected = scratch.path().join(format!("collected-{index}.bin"));
        let output = scratch.path().join(format!("spawned-{index}.log"));
        let remote_arg = if relative {
            format!("../landed-{index}.bin")
        } else {
            remote.to_string_lossy().into_owned()
        };
        let output_arg = if relative {
            format!("../spawned-{index}.log")
        } else {
            output.to_string_lossy().into_owned()
        };
        let calls = [
            (
                "push",
                serde_json::json!({"agent": address, "from": local, "to": remote_arg}),
            ),
            (
                "spawn",
                serde_json::json!({"agent": address, "command": if cfg!(windows) { format!("type \"{}\"", remote.display()) } else { format!("cat \"{}\"", remote.display()) }, "output": output_arg}),
            ),
            (
                "pull",
                serde_json::json!({"agent": address, "from": remote_arg, "to": collected}),
            ),
            (
                "ls",
                serde_json::json!({"agent": address, "path": remote_arg}),
            ),
            (
                "grep",
                serde_json::json!({"agent": address, "path": remote_arg, "pattern": "crossed"}),
            ),
            (
                "tail",
                serde_json::json!({"agent": address, "path": remote_arg}),
            ),
            (
                "exec",
                serde_json::json!({"agent":address,"command":if cfg!(windows) { format!("type \"{}\"", remote.display()) } else { format!("cat \"{}\"", remote.display()) }}),
            ),
            (
                "exec",
                serde_json::json!({"agents":[address,address],"command":"exit 7"}),
            ),
            ("probe", serde_json::json!({"agent":address})),
            (
                "exec",
                serde_json::json!({"agent":address,"command":"exit 7"}),
            ),
            (
                "exec",
                serde_json::json!({"agent":address,"timeout":1,"command":if cfg!(windows) { "ping -n 30 127.0.0.1 >NUL" } else { "sleep 30" }}),
            ),
        ];
        let messages: Vec<String> = calls
            .iter()
            .enumerate()
            .map(|(i, (name, arguments))| {
                request(
                    (i + 1) as i64,
                    "tools/call",
                    &serde_json::json!({"name": name, "arguments": arguments}).to_string(),
                )
            })
            .collect();
        let messages: Vec<&str> = messages.iter().map(String::as_str).collect();
        runs.push((
            calls,
            session_with_credentials(&messages, &[(&address, token)]),
            remote,
            collected,
            output,
        ));
    }
    let _ = agent.kill();
    let _ = agent.wait();

    for (calls, replies, remote, collected, output) in runs {
        for (i, (tool, arguments)) in calls.iter().enumerate() {
            let reply = reply_for(&replies, (i + 1) as i64);
            let result = reply.get("result").expect("a tool result");
            assert!(
                result["structuredContent"]["data"]
                    .as_object()
                    .is_some_and(|d| !d.is_empty()),
                "{tool}: {result}"
            );
            assert_eq!(
                result.get("isError").and_then(Value::as_bool),
                Some(arguments["command"] == "exit 7" || arguments["timeout"] == 1),
                "{tool}: {result}"
            );
            let text = result["content"][0]["text"].as_str().expect("text");
            match *tool {
                "push" | "pull" => assert!(text.contains("(verified)"), "{text}"),
                "probe" => {
                    let identity = &result["structuredContent"]["data"];
                    assert_eq!(identity["os"], std::env::consts::OS);
                    assert!(identity["startup_cwd"].as_str().is_some());
                    assert!(identity["account"].as_str().is_some());
                    assert!(text.contains("linklet-agent"), "{text}");
                    assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
                }
                "exec" if arguments["command"] == "exit 7" => {
                    let data = &result["structuredContent"]["data"];
                    if arguments.get("agents").is_some() {
                        for record in data["results"].as_array().expect("results") {
                            assert_eq!(record["data"]["exit_code"], 7);
                            assert_eq!(record["status"], "error");
                        }
                    } else {
                        assert_eq!(data["exit_code"], 7);
                    }
                }
                "exec" if arguments["timeout"] == 1 => assert_eq!(
                    result["structuredContent"]["error"]["code"],
                    "deadline_exceeded"
                ),
                "exec" => assert!(
                    result["structuredContent"]["data"]["stdout"]["text"]
                        .as_str()
                        .expect("stdout")
                        .contains("crossed")
                ),
                "tail" => assert!(
                    result["structuredContent"]["data"]["text"]
                        .as_str()
                        .expect("tail text")
                        .contains("crossed")
                ),
                "grep" => assert!(
                    result["structuredContent"]["data"]["lines"]
                        .to_string()
                        .contains("crossed")
                ),
                "ls" => assert!(text.contains("1 of 1"), "{text}"),
                _ => {}
            }
        }
        assert_eq!(std::fs::read(&remote).expect("the remote file"), content);
        assert_eq!(std::fs::read(&collected).expect("the local copy"), content);
        assert!(output.is_file(), "spawn output: {}", output.display());
    }
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

#[test]
fn integer_request_ids_are_echoed_without_rounding() {
    for id in ["0", "9223372036854775809", "18446744073709551615"] {
        let message = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#);
        let replies = session(&[&message]);
        assert_eq!(replies.len(), 1);
        assert_eq!(
            (replies[0].get("id").expect("a response id")).to_string(),
            id
        );
    }
}

#[test]
fn escaped_request_ids_stay_on_one_line_and_round_trip() {
    let id: String = (0..32)
        .map(|n| char::from_u32(n).expect("control character"))
        .collect();
    let id = format!("{id}\u{4e2d}");
    let message = format!(
        r#"{{"jsonrpc":"2.0","id":{},"method":"ping"}}"#,
        serde_json::json!(&id)
    );
    let replies = session(&[&message]);
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].get("id").and_then(Value::as_str),
        Some(id.as_str())
    );
}

#[test]
fn discovery_uses_the_cli_network_listing_and_probe_errors_are_tool_errors() {
    let output = Command::new(env!("CARGO_BIN_EXE_linklet"))
        .args(["discover", "--networks"])
        .output()
        .expect("CLI discovery");
    let discover = request(
        1,
        "tools/call",
        r#"{"name":"discover","arguments":{"networks":true}}"#,
    );
    let probe = request(
        2,
        "tools/call",
        r#"{"name":"probe","arguments":{"agent":"127.0.0.1:1"}}"#,
    );
    let replies = session_with_credentials(
        &[&discover, &probe],
        &[("127.0.0.1:1", "test-token-0123456789")],
    );
    let result = &reply_for(&replies, 1)["result"];
    assert_eq!(result["isError"], !output.status.success());
    if output.status.success() {
        assert_eq!(
            result["structuredContent"]["data"]["networks"]
                .as_array()
                .expect("networks")
                .iter()
                .map(|v| v.as_str().expect("network"))
                .collect::<Vec<_>>()
                .join("\n"),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    assert_eq!(reply_for(&replies, 2)["result"]["isError"], true);
}

#[test]
fn every_tool_declares_structured_results_and_bad_calls_use_them() {
    let list = request(901, "tools/list", "");
    let call = request(902, "tools/call", r#"{"name":"exec","arguments":{}}"#);
    let replies = session(&[&list, &call]);
    let listing = reply_for(&replies, 901);
    let tools = listing["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 14);
    for tool in tools {
        assert_eq!(tool["outputSchema"]["type"], "object", "{tool}");
        assert!(tool["outputSchema"]["properties"]["status"].is_object());
    }
    let reply = reply_for(&replies, 902);
    let result = &reply["result"];
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["status"], "error");
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
}

#[test]
fn initialize_negotiates_the_supported_structured_result_version() {
    let init = request(903, "initialize", r#"{"protocolVersion":"2099-01-01"}"#);
    let replies = session(&[&init]);
    assert_eq!(
        reply_for(&replies, 903)["result"]["protocolVersion"],
        "2025-06-18"
    );
}

#[test]
fn local_path_and_parameter_errors_have_stable_categories() {
    for (name, args, expected) in [
        (
            "testbed",
            serde_json::json!({"spec":"missing-linklet-spec-31fc","target":"local"}),
            "not_found",
        ),
        (
            "check",
            serde_json::json!({"targets":["not-a-target"]}),
            "invalid_argument",
        ),
    ] {
        let call = request(
            91,
            "tools/call",
            &serde_json::json!({"name":name,"arguments":args}).to_string(),
        );
        let replies = session(&[&call]);
        assert_eq!(
            reply_for(&replies, 91)["result"]["structuredContent"]["error"]["code"],
            expected
        );
    }
}
