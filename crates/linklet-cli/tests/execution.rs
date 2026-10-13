//! CLI and MCP use exact argv, per-call cwd, and the same process outcomes.
#[path = "../../test-support/credentials.rs"]
mod credentials;
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
        args["agent"] = self.address.clone().into();
        let mut c = self
            .cli()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("mcp");
        writeln!(c.stdin.take().expect("stdin"),"{}",json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).expect("request");
        let output = c.wait_with_output().expect("reply");
        serde_json::from_slice::<Value>(&output.stdout).expect("json")["result"].clone()
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
