/// Command-line help shared by the two entry points.
pub const USAGE: &str = "\
linklet-agent -- run a command on this machine when a host asks

usage:
  linklet-agent [--port <port>] [--log <file> | --no-log]

options:
  init            rotate saved code and credentials, then start; stop the agent first
  --port <port>   the port to listen on (default 8787)
  --log <file>    where the request log goes (or LINKLET_LOG). Default: logs/agent.log
                  in the directory the agent's own executable is in, rolled over when
                  it reaches a mebibyte with three older files kept
  --no-log        keep no log. A request that never finishes then leaves no evidence
                  behind, which is the one thing the log exists to answer
  --version      print the version
  -h, --help      print this

Startup creates or reuses this account's permanent six-digit
pairing code and displays it. Pair from a controller once; credentials are remembered.

File paths are absolute, or relative to the directory the agent was started in.
Access is limited by this process's operating-system permissions.
";
