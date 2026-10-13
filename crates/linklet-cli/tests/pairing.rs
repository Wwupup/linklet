//! Pair once, then use credentials across independent CLI and MCP processes.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

struct Fixture {
    root: PathBuf,
    agent: Child,
    address: String,
    code: String,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "linklet-pair-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("scratch");
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repository");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repository.join("target"));
        let mut agent = Command::new(target.join(format!(
            "debug/linklet-agent{}",
            std::env::consts::EXE_SUFFIX
        )))
        .args(["--port", "0", "--no-log"])
        .env("LOCALAPPDATA", root.join("target"))
        .env("XDG_CONFIG_HOME", root.join("target"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start agent");
        let mut reader = BufReader::new(agent.stdout.take().expect("stdout"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("banner");
        let port = line
            .split_whitespace()
            .find_map(|s| s.strip_prefix("0.0.0.0:"))
            .expect("port");
        let address = format!("127.0.0.1:{port}");
        line.clear();
        reader.read_line(&mut line).expect("code");
        let code = line
            .trim()
            .strip_prefix("Pairing code: ")
            .expect("code")
            .to_string();
        Self {
            root,
            agent,
            address,
            code,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_linklet"));
        cmd.env("LOCALAPPDATA", self.root.join("controller"))
            .env("XDG_CONFIG_HOME", self.root.join("controller"));
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("CLI")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.agent.kill();
        let _ = self.agent.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}
#[test]
fn pairing_persists_for_cli_and_mcp_without_a_token_environment() {
    let mut f = Fixture::new();
    let text = success(f.run(&["pair", "--agent", &f.address, "--code", &f.code]));
    assert!(text.contains("paired"));
    assert!(!text.contains(&f.code));
    success(f.run(&["probe", "--agent", &f.address]));
    assert!(
        success(f.run(&["exec", "--agent", &f.address, "echo paired-command"]))
            .contains("paired-command")
    );
    let mut mcp = f
        .command()
        .env("LINKLET_TOKEN", "short")
        .env("LINKLET_TOKEN_FILE", "missing-obsolete-token-file")
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP");
    writeln!(mcp.stdin.as_mut().expect("stdin"), "{}", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"probe","arguments":{"agent":f.address}}})).expect("request");
    let output = mcp.wait_with_output().expect("MCP reply");
    let value: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    assert_eq!(value["result"]["isError"], false);
    let replies = mcp_calls(
        &f,
        "second-controller",
        &[
            call(1, "pair", json!({"agent":f.address,"code":f.code})),
            call(
                2,
                "exec",
                json!({"agents":[f.address,f.address],"command":"echo another-controller"}),
            ),
        ],
    );
    assert_eq!(replies.len(), 2);
    for reply in &replies {
        assert_eq!(reply["result"]["isError"], false, "{reply}");
    }
    let wrong = if f.code == "000000" {
        "000001"
    } else {
        "000000"
    };
    for _ in 0..3 {
        assert_eq!(
            f.run(&["pair", "--agent", &f.address, "--code", wrong])
                .status
                .code(),
            Some(3)
        );
    }
    let throttled = f.run(&["pair", "--agent", &f.address, "--code", &f.code]);
    assert_eq!(throttled.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&throttled.stderr).contains("rate limit"));
    success(f.run(&["probe", "--agent", &f.address]));
    let old_token =
        std::fs::read(f.root.join("target/linklet/agent.json")).expect("saved credentials");
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repository");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository.join("target"));
    let agent_command = || {
        let mut cmd = Command::new(target.join(format!(
            "debug/linklet-agent{}",
            std::env::consts::EXE_SUFFIX
        )));
        cmd.env("LOCALAPPDATA", f.root.join("target"))
            .env("XDG_CONFIG_HOME", f.root.join("target"));
        cmd
    };
    let refused = agent_command()
        .args(["init", "--port", "0", "--no-log"])
        .output()
        .expect("running reset");
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(
        std::fs::read(f.root.join("target/linklet/agent.json")).expect("state unchanged"),
        old_token
    );
    f.agent.kill().expect("stop agent");
    f.agent.wait().expect("wait");
    let port = f.address.rsplit_once(':').expect("port").1;
    f.agent = agent_command()
        .args(["init", "--port", port, "--no-log"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("reset");
    let mut reader = BufReader::new(f.agent.stdout.take().expect("stdout"));
    let mut line = String::new();
    reader.read_line(&mut line).expect("banner");
    assert!(line.contains("listening on"));
    line.clear();
    reader.read_line(&mut line).expect("new code");
    let new_code = line
        .trim()
        .strip_prefix("Pairing code: ")
        .expect("new code");
    assert_ne!(new_code, f.code);
    assert_eq!(
        f.run(&["probe", "--agent", &f.address]).status.code(),
        Some(3),
        "reset revokes saved controller tokens"
    );
    success(f.run(&["pair", "--agent", &f.address, "--code", new_code]));
    success(f.run(&["probe", "--agent", &f.address]));
}

fn mcp_calls(f: &Fixture, controller: &str, messages: &[Value]) -> Vec<Value> {
    let mut child = f
        .command()
        .env("LOCALAPPDATA", f.root.join(controller))
        .env("XDG_CONFIG_HOME", f.root.join(controller))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP");
    for message in messages {
        writeln!(child.stdin.as_mut().expect("stdin"), "{message}").expect("write");
    }
    let output = child.wait_with_output().expect("MCP reply");
    String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|s| serde_json::from_str(s).expect("JSON"))
        .collect()
}
fn call(id: u32, name: &str, args: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}})
}

#[test]
fn one_controller_uses_different_saved_tokens_in_a_batch() {
    let first = Fixture::new();
    let second = Fixture::new();
    for target in [&first, &second] {
        success(first.run(&["pair", "--agent", &target.address, "--code", &target.code]));
    }
    let agents = format!("{},{}", first.address, second.address);
    success(first.run(&["exec", "--agents", &agents, "echo paired-batch"]));
    let replies = mcp_calls(
        &first,
        "controller",
        &[call(
            1,
            "exec",
            json!({"agents":[first.address,second.address],"command":"echo mcp-batch"}),
        )],
    );
    assert_eq!(replies[0]["result"]["isError"], false);
    let output = first
        .command()
        .env("LINKLET_TOKEN", "explicit-wrong-token-1234")
        .args(["probe", "--agent", &first.address])
        .output()
        .expect("obsolete environment");
    assert_eq!(
        output.status.code(),
        Some(0),
        "obsolete token variables cannot override saved credentials"
    );
    let output = first
        .command()
        .env("LINKLET_TOKEN", "short")
        .env("LINKLET_TOKEN_FILE", "missing-obsolete-token-file")
        .args(["probe", "--agent", &first.address])
        .output()
        .expect("obsolete environment");
    assert_eq!(
        output.status.code(),
        Some(0),
        "obsolete token configuration is ignored"
    );
}
