//! CLI and MCP use exact argv, per-call cwd, and the same process outcomes.
#[path = "../../test-support/credentials.rs"]
mod credentials;
#[path = "../../test-support/schemas.rs"]
mod schemas;
use credentials::Credentials;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
};
struct Fixture {
    _agent_credentials: Credentials,
    dir: PathBuf,
    agent: Child,
    address: String,
    credentials: Credentials,
    program: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "linklet argv \u{4e2d} {} {}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("directory");
        let program = dir.join(format!("literal arguments{}", std::env::consts::EXE_SUFFIX));
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../test-support/argv_fixture.rs");
        assert!(
            Command::new("rustc")
                .arg(source)
                .arg("-o")
                .arg(&program)
                .status()
                .expect("compile fixture")
                .success()
        );
        let agent_binary = PathBuf::from(env!("CARGO_BIN_EXE_linklet"))
            .with_file_name(format!("linklet-agent{}", std::env::consts::EXE_SUFFIX));
        let agent_credentials = Credentials::new(Some("argv-fixture-token-0123456789"), &[]);
        let mut agent = Command::new(agent_binary)
            .envs(agent_credentials.env())
            .args(["--port", "0", "--no-log"])
            .current_dir(&dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("agent");
        let mut reader = BufReader::new(agent.stdout.take().expect("stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("banner");
        let port = line
            .split_whitespace()
            .find_map(|w| w.strip_prefix("0.0.0.0:"))
            .expect("port");
        let address = format!("127.0.0.1:{port}");
        let credentials = Credentials::new(None, &[(&address, "argv-fixture-token-0123456789")]);
        // Retain the agent configuration until shutdown, independently of the controller.
        Self {
            _agent_credentials: agent_credentials,
            dir,
            agent,
            address,
            credentials,
            program,
        }
    }
    fn cli(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_linklet"));
        c.envs(self.credentials.env());
        c
    }
    fn tool(&self, name: &str, mut args: Value) -> Value {
        if args.get("agents").is_none() {
            args["agent"] = self.address.clone().into();
        }
        let mut c = self
            .cli()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("mcp");
        writeln!(c.stdin.take().expect("stdin"),"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).expect("request");
        let output = c.wait_with_output().expect("reply");
        let result =
            serde_json::from_slice::<Value>(&output.stdout).expect("json")["result"].clone();
        schemas::validate(name, &result);
        result
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.agent.kill();
        let _ = self.agent.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn literal_arguments_cwd_and_remote_help_work_through_cli_and_mcp() {
    let fixture = Fixture::new();
    let cwd = fixture.dir.join("child \u{6587} space");
    std::fs::create_dir(&cwd).expect("cwd");
    let args = [
        "",
        "two words",
        "--help",
        "--agent",
        "\"quoted\"",
        "&|<>$()",
        "\u{4e2d}\u{6587}",
    ];
    let output = fixture
        .cli()
        .args(["exec", "--agent", &fixture.address, "--cwd"])
        .arg(&cwd)
        .arg("--program")
        .arg(&fixture.program)
        .args(args)
        .output()
        .expect("exec");
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).expect("utf8");
    let response = fixture.tool(
        "exec",
        json!({"cwd":cwd,"program":fixture.program,"args":args}),
    );
    assert_eq!(response["isError"], false, "{response}");
    let data = &response["structuredContent"]["data"];
    assert_eq!(data["cwd"], cwd.to_string_lossy().as_ref());
    let remote = data["stdout"]["text"].as_str().expect("output");
    for arg in args {
        let expected = format!("arg:{}:{arg}", arg.len());
        assert!(remote.contains(&expected), "{remote}");
        assert!(text.contains(&expected), "{text}");
    }
    assert!(remote.contains("stdin:0"));
    let bad = fixture.tool(
        "exec",
        json!({"command":"echo wrong","program":fixture.program}),
    );
    assert_eq!(
        bad["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
    let relative = format!(
        "../{}",
        fixture
            .program
            .file_name()
            .expect("filename")
            .to_string_lossy()
    );
    let response = fixture.tool(
        "spawn",
        json!({"cwd":cwd,"program":relative,"args":[],"output":"observed.log"}),
    );
    assert_eq!(response["isError"], false, "{response}");
    assert_eq!(
        response["structuredContent"]["data"]["cwd"],
        cwd.to_string_lossy().as_ref()
    );
    assert_eq!(
        response["structuredContent"]["data"]["output"],
        fixture.dir.join("observed.log").to_string_lossy().as_ref()
    );
}

#[test]
fn spawn_reports_only_what_was_observed() {
    let fixture = Fixture::new();
    for (args, observe, state, code, error) in [
        (vec!["exit", "0"], 500, "exited", Some(0), false),
        (vec!["exit", "7"], 500, "exited", Some(7), true),
        (
            vec!["sleep", "1200"],
            100,
            "running_at_observation",
            None,
            false,
        ),
        (vec!["sleep", "1200"], 0, "started", None, false),
    ] {
        let result=fixture.tool("spawn",json!({"program":fixture.program,"args":args,"output":format!("observe-{observe}-{error}.log"),"observe_ms":observe}));
        assert_eq!(result["isError"], error, "{result}");
        let data = &result["structuredContent"]["data"];
        assert_eq!(data["state"], state, "{result}");
        assert_eq!(data["exit_code"], json!(code));
        assert_eq!(data["pid_kind"], "program");
    }
    let result = fixture.tool(
        "spawn",
        json!({"program":fixture.program,"args":["exit","0"],"output":"default.log"}),
    );
    assert_eq!(result["structuredContent"]["data"]["state"], "exited");
    let result = fixture.tool(
        "spawn",
        json!({"program":fixture.program,"output":"invalid.log","observe_ms":5001}),
    );
    assert_eq!(
        result["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
    std::thread::sleep(std::time::Duration::from_millis(1250));
}

#[test]
fn mcp_tail_pages_do_not_skip_content_to_fit_the_presentation_budget() {
    let fixture = Fixture::new();
    let content = format!("header\r\n{}", "\u{4e2d}\u{1f600}long".repeat(12000));
    std::fs::write(fixture.dir.join("log"), &content).expect("log");
    let mut cursor = None;
    let mut text = String::new();
    let mut calls = 0;
    loop {
        let mut args = json!({"path":"log","lines":20});
        if let Some(c) = cursor {
            args["cursor"] = Value::String(c);
        }
        let result = fixture.tool("tail", args);
        assert_eq!(result["isError"], false, "{result}");
        let data = &result["structuredContent"]["data"];
        let page = data["text"].as_str().expect("text");
        assert!(
            page.len()
                + result["content"][0]["text"]
                    .as_str()
                    .expect("summary")
                    .len()
                <= 32768
        );
        text.push_str(page);
        cursor = Some(data["next_cursor"].as_str().expect("cursor").to_string());
        calls += 1;
        if data["has_more"] == false {
            break;
        }
        assert!(calls < 20, "cursor must make progress");
    }
    assert!(calls > 1);
    assert_eq!(text, content);
    let result = fixture.tool("tail", json!({"path":"log","cursor":cursor}));
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["data"]["text"], "");
}

#[test]
fn concurrent_calls_keep_their_own_directories() {
    let fixture = Fixture::new();
    let left = fixture.dir.join("left");
    let right = fixture.dir.join("right");
    std::fs::create_dir(&left).expect("left");
    std::fs::create_dir(&right).expect("right");
    std::thread::scope(|scope| {
        for cwd in [&left, &right] {
            let f = &fixture;
            scope.spawn(move || {
                let response = f.tool("exec", json!({"cwd":cwd,"program":f.program,"args":[]}));
                assert_eq!(
                    response["structuredContent"]["data"]["cwd"],
                    cwd.to_string_lossy().as_ref()
                );
                assert!(
                    response["structuredContent"]["data"]["stdout"]["text"]
                        .as_str()
                        .expect("stdout")
                        .contains(cwd.to_string_lossy().as_ref())
                );
            });
        }
    });
    let cli = fixture
        .cli()
        .args([
            "spawn",
            "--agent",
            &fixture.address,
            "--output",
            "cli-spawn.log",
            "--cwd",
        ])
        .arg(&left)
        .arg("--program")
        .arg(&fixture.program)
        .args(["--help", "", "&|"])
        .output()
        .expect("spawn");
    assert!(cli.status.success(), "{cli:?}");
    let summary = String::from_utf8(cli.stdout).expect("CLI output");
    assert!(
        summary.contains(left.to_string_lossy().as_ref())
            && summary.contains("cli-spawn.log")
            && summary.contains("stdin:0"),
        "{summary}"
    );
    let log = std::fs::read_to_string(fixture.dir.join("cli-spawn.log")).expect("output");
    assert!(log.contains("arg:6:--help") && log.contains("arg:0:") && log.contains("arg:2:&|"));
}

#[test]
fn real_large_dual_stream_batches_are_bounded_and_timeouts_are_errors() {
    let fixture = Fixture::new();
    let raw = 17 * 1024 * 1024usize;
    let result = fixture.tool("exec",json!({"agents":[fixture.address,fixture.address],"program":fixture.program,"args":["flood",raw.to_string()]}));
    assert_eq!(result["isError"], false);
    let mut content = result["content"][0]["text"]
        .as_str()
        .expect("summary")
        .len();
    for record in result["structuredContent"]["data"]["results"]
        .as_array()
        .expect("batch")
    {
        assert_eq!(record["status"], "ok");
        for (stream, marker) in [("stdout", "OUT-END"), ("stderr", "ERR-END")] {
            let output = &record["data"][stream];
            assert_eq!(output["total_bytes"], raw + marker.len());
            assert_eq!(output["retained_bytes"], 1048576);
            assert_eq!(output["truncated"], true);
            assert_eq!(output["presentation_truncated"], true);
            let text = output["text"].as_str().expect("text");
            assert!(text.ends_with(marker) && text.len() > 7000);
            content += text.len();
        }
    }
    assert!(content <= 32768, "{content}");
    println!(
        "METRIC dual_stream_batch calls=1 result_bytes={} content_bytes={content} wrong_statuses=0",
        result.to_string().len()
    );
    let timeout = fixture.tool(
        "exec",
        json!({"program":fixture.program,"args":["sleep","3000"],"timeout":1}),
    );
    assert_eq!(
        timeout["structuredContent"]["error"]["code"],
        "deadline_exceeded"
    );
    assert_eq!(timeout["isError"], true);
}

#[cfg(windows)]
#[test]
fn direct_windows_batches_are_rejected_before_execution() {
    let fixture = Fixture::new();
    let batch = fixture.dir.join("must not run.cmd");
    std::fs::write(&batch, "@echo wrong>should-not-exist").expect("batch");
    for tool in ["exec", "spawn"] {
        let mut args = json!({"program":batch});
        if tool == "spawn" {
            args["output"] = "rejected.log".into();
        }
        let result = fixture.tool(tool, args);
        assert_eq!(
            result["structuredContent"]["error"]["code"],
            "invalid_argument"
        );
    }
    assert!(!fixture.dir.join("should-not-exist").exists());
}

#[test]
fn fixed_context_start_and_log_scenario_records_calls_and_sizes() {
    let fixture = Fixture::new();
    let probe = fixture.tool("probe", json!({}));
    assert_eq!(probe["isError"], false);
    let identity = &probe["structuredContent"]["data"];
    assert!(identity["observed_at_ms"].is_number());
    let start = fixture.tool(
        "spawn",
        json!({"program":fixture.program,"output":"metrics.log"}),
    );
    assert_eq!(start["structuredContent"]["data"]["state"], "exited");
    let first = fixture.tool("tail", json!({"path":"metrics.log"}));
    let cursor = &first["structuredContent"]["data"]["next_cursor"];
    let next = fixture.tool("tail", json!({"path":"metrics.log","cursor":cursor}));
    assert_eq!(next["structuredContent"]["data"]["text"], "");
    let ps = fixture.tool(
        "ps",
        json!({"name":"linklet-impossible-process-fixture-73cc"}),
    );
    assert_eq!(ps["structuredContent"]["data"]["count"], 0);
    let kill = fixture.tool(
        "kill",
        json!({"name":"linklet-impossible-process-fixture-73cc","confirmed":true}),
    );
    assert_eq!(kill["structuredContent"]["data"]["matched"], 0);
    for (name, result) in [
        ("probe", probe),
        ("spawn", start),
        ("tail_first", first),
        ("tail_empty", next),
        ("ps_empty", ps),
        ("kill_empty", kill),
    ] {
        assert_eq!(result["isError"], false);
        println!(
            "METRIC {name} calls=1 result_bytes={} wrong_statuses=0",
            result.to_string().len()
        );
    }
    println!(
        "METRIC tool_definitions count=14 bytes={}",
        linklet_core::tool_list_json().to_string().len()
    );
}

#[test]
fn batch_unknown_outcome_takes_priority_over_known_command_failure() {
    use linklet_adapters::{Connection, HkdfChannel};
    use linklet_core::{
        channel::{EphemeralPublic, Handshake},
        frame::Kind,
        wire,
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address").to_string();
    let peer = std::thread::spawn(move || {
        let (socket, _) = listener.accept().expect("accept");
        let mut c = Connection::with_budget(socket, std::time::Duration::from_secs(5));
        let hello = wire::parse_body(&c.read_frame(Kind::Hello).expect("hello")).expect("json");
        let public =
            EphemeralPublic::from_bytes(wire::handshake_public_from_json(&hello).expect("key"))
                .expect("key");
        let (ours, mut session) = HkdfChannel
            .accept(b"vanishing-batch-token-012345", &public)
            .expect("handshake");
        c.write_frame(
            Kind::Hello,
            wire::reply_result(wire::handshake_to_json(ours.as_bytes()))
                .to_string()
                .as_bytes(),
        )
        .expect("reply");
        let sealed = c.read_frame(Kind::Sealed).expect("request");
        let mut request = Vec::new();
        session
            .open_into(&sealed, &mut request)
            .expect("authenticated");
        assert!(matches!(
            wire::request_from_json(&wire::parse_body(&request).expect("json")),
            Ok(wire::Request::Run(_))
        ));
    });
    let mut fixture = Fixture::new();
    fixture.credentials = Credentials::new(
        None,
        &[
            (&fixture.address, "argv-fixture-token-0123456789"),
            (&address, "vanishing-batch-token-012345"),
        ],
    );
    let reply = fixture.tool(
        "exec",
        json!({"agents":[fixture.address,address],"program":fixture.program,"args":["exit","7"]}),
    );
    peer.join().expect("peer");
    let result = &reply["structuredContent"];
    assert_eq!(result["status"], "unknown");
    assert_eq!(result["data"]["results"][0]["status"], "error");
    assert_eq!(result["data"]["results"][0]["data"]["exit_code"], 7);
    assert_eq!(result["data"]["results"][1]["status"], "unknown");
    assert_eq!(result["data"]["unknown"], 1);
}

#[test]
fn tail_cursor_is_bound_to_the_controller_target() {
    let fixture = Fixture::new();
    std::fs::write(fixture.dir.join("bound.log"), "first\n").expect("log");
    let first = fixture.tool("tail", json!({"path":"bound.log"}));
    let mut cursor: Value = serde_json::from_str(
        first["structuredContent"]["data"]["next_cursor"]
            .as_str()
            .expect("cursor"),
    )
    .expect("facts");
    cursor["target"] = "different:8787".into();
    let rejected = fixture.tool(
        "tail",
        json!({"path":"bound.log","cursor":cursor.to_string()}),
    );
    assert_eq!(
        rejected["structuredContent"]["error"]["code"],
        "invalid_argument"
    );
}
