//! Command-specific help without consuming remote command flags.
use linklet_core::arguments::{Args, Flag, Strictness};

/// Returns the requested command help, stopping before remote command arguments.
pub fn requested(arguments: &[String]) -> Option<String> {
    let command = arguments.first()?.as_str();
    if matches!(command, "-h" | "--help") {
        return Some(super::USAGE.into());
    }
    let detail = match command {
        "check" => {
            "check [--timeout <seconds>] [--max-targets <count>] <host:port,...>\nCheck TCP reachability; this does not authenticate an agent."
        }
        "testbed" => {
            "testbed check <spec-file> <target>\nCheck local requirements from a specification."
        }
        "exec" => {
            "exec (--agent <host:port> | --agents <a,b,...>) [--timeout <1..600>] [--cwd <dir>] (<command...> | --program <exe> [args...])\nOptions end at the command or --program executable. Single-target execution preserves its exit code; a batch returns 0 only if all commands exit 0. Each stream keeps its last 1 MiB, with an explicit truncation notice."
        }
        "spawn" => {
            "spawn --agent <host:port> --output <remote-file> [--cwd <dir>] (<command...> | --program <exe> [args...])\nObserve for 500 ms (use --observe-ms <0..5000>, 0 returns immediately). Early exits report their code; running_at_observation does not mean readiness. Output goes to the named file. No restart or supervision."
        }
        "ps" => {
            "ps --agent <host:port> [--name <text>] [--cmdline <text>] [--query <text>] [--exclude <text>]\nList matching processes."
        }
        "kill" => {
            "kill --agent <host:port> (--pid <n> | --name <exact> | --contains <text>) [--yes] [--exclude <text>] [--candidates-name <text>] [--candidates-cmdline <text>]\nStop matching processes. Name-based selection requires --yes."
        }
        "grep" => {
            "grep --agent <host:port> --from <remote-file> --pattern <text> [-i|--ignore-case] [--first|--last] [--max <n>] [--context <n>]\nSearch a bounded file window with explicit encoding."
        }
        "tail" => {
            "tail --agent <host:port> --from <remote-file> [--lines <n>] [--cursor <opaque>]\nRead the last lines or resume new bytes. Return a bounded page, next_cursor and has_more; preserve newlines and report file changes."
        }
        "ls" => {
            "ls --agent <host:port> --from <remote-directory>\nList files using the agent account permissions."
        }
        "pair" => {
            "pair --agent <host:port> --code <six-digits>\nPair and remember this target for CLI and MCP calls."
        }
        "probe" => {
            "probe --agent <host:port>\nAuthenticate and request identity. Exit: 0 usable, 1 connection failed, 2 arguments/local configuration, 3 authentication/protocol version refused, 4 timeout/invalid reply. No automatic restart."
        }
        "discover" => {
            "discover [--port <1..65535>] [--networks] [--targets]\nScan local networks for TCP reachability, not authentication. --networks lists interfaces; --targets prints host:port entries."
        }
        "push" => {
            "push --agent <host:port> --from <local-file> --to <remote-file>\nTransfer one file and verify its digest."
        }
        "pull" => {
            "pull --agent <host:port> --from <remote-file> --to <local-file>\nTransfer one file and verify its digest."
        }
        "mcp" => {
            "mcp\nServe tools over stdin/stdout. Credentials are remembered per target after pairing."
        }
        _ => return None,
    };
    let asked = if matches!(command, "exec" | "spawn") {
        let mut flags = vec![
            Flag::takes_value("--agent"),
            Flag::takes_value("--cwd"),
            Flag::starts_tail("--program"),
            Flag::switch("-h"),
            Flag::switch("--help"),
        ];
        if command == "exec" {
            flags.extend([
                Flag::takes_value("--agents"),
                Flag::takes_value("--timeout"),
            ]);
        } else {
            flags.push(Flag::takes_value("--output"));
            flags.push(Flag::takes_value("--observe-ms"));
        }
        let args = Args::parse(&arguments[1..], &flags, Strictness::CommandTail).ok()?;
        args.switch("-h") || args.switch("--help")
    } else {
        arguments[1..]
            .iter()
            .take_while(|a| a.as_str() != "--")
            .any(|a| matches!(a.as_str(), "-h" | "--help"))
    };
    asked.then(|| format!("usage: linklet {detail}\n\n-h, --help  Show this help.\n"))
}
