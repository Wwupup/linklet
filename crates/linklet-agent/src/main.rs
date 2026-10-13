//! The target-side agent.
//!
//! It does two things: says which agent it is, and runs a command when asked.
//! Everything it knows about the protocol comes from `linklet_core::wire`, which the
//! host uses too, so the two cannot drift into two readings of the same message.
//!
//! # Why this is a separate binary
//!
//! The host's `linklet` runs on a development machine. This runs on the machine
//! being debugged, which may be a rack, a build box, or the laptop next to it.
//! Making them one binary would mean shipping the whole host tool to every target
//! and would put "can run a command" behind the same door as "can check a port".
//!
//! It is also the reason this crate has two dependencies and not five: the smaller
//! the thing that runs on someone else's machine, the less there is to be wrong with
//! it. `linklet-core` is the protocol and `linklet-adapters` is the socket and the
//! cipher, and neither brings a runtime.
//!
//! # What it does not do
//!
//! - **No identities.** The token authenticates the channel and says nothing about
//!   *which* caller it is, so there is no per-caller revocation and no audit trail.
//!   One secret, every caller. See `docs/ROADMAP.md` M9.
//! - **No session between connections.** A caller that wants to run two commands
//!   opens two connections and does two handshakes. A session that outlived a
//!   connection would need a table and an eviction policy, and an eviction policy is
//!   a way to be exhausted.
//! - **No streaming reply.** Each stream retains its final mebibyte of raw bytes;
//!   protocol 2 reports total size, truncation, encoding, and decoding loss.

#![forbid(unsafe_code)]

mod execute;
mod log;
mod server;
mod shell;
mod spawn;

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use linklet_core::log::Rotation;
use linklet_core::transfer::Destination;

/// The port the agent listens on when it is not told.
///
/// A constant so that the host and the agent agree by reading one place rather
/// than two that were typed the same way once.
pub const DEFAULT_PORT: u16 = 8787;

/// Where the log goes when nobody says.
///
/// `logs/agent.log` beside the agent's own executable -- see the comment where it is
/// used for why this is not the working directory used by relative file paths.
///
/// Falls back to the working directory only if the executable's own path cannot be read,
/// which is the one case where there is nothing better to say, and it is not worth
/// failing a start over because the open below reports a real failure anyway.
fn default_log_path() -> PathBuf {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    beside.join("logs").join("agent.log")
}

const USAGE: &str = "\
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

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let mut port = DEFAULT_PORT;
    let mut log_path: Option<String> = std::env::var("LINKLET_LOG").ok();
    let mut no_log = false;
    let mut reset = false;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "init" if !reset => reset = true,
            "--version" => {
                println!("linklet-agent {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--port" => {
                let Some(value) = iterator.next() else {
                    eprintln!("linklet-agent: --port needs a value");
                    std::process::exit(2);
                };
                match value.parse() {
                    Ok(value) => port = value,
                    Err(_) => {
                        eprintln!("linklet-agent: {value:?} is not a port number");
                        std::process::exit(2);
                    }
                }
            }
            "--log" => match iterator.next() {
                Some(value) => log_path = Some(value.clone()),
                None => {
                    eprintln!("linklet-agent: --log needs a file");
                    std::process::exit(2);
                }
            },
            "--no-log" => no_log = true,
            other => {
                eprintln!("linklet-agent: unexpected argument {other:?}");
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

    let managed = match linklet_adapters::credentials::config_dir()
        .and_then(|path| linklet_adapters::credentials::AgentCredentials::acquire(&path, reset))
    {
        Ok(credentials) => credentials,
        Err(error) => {
            eprintln!("linklet-agent: {error}");
            std::process::exit(2);
        }
    };
    let token = managed.token().clone();
    let pairing = match server::PairingState::new(managed.code()) {
        Ok(state) => std::sync::Arc::new(state),
        Err(error) => {
            eprintln!("linklet-agent: {error}");
            std::process::exit(2);
        }
    };

    // Use the startup directory only as the base for relative paths. Absolute
    // paths may name any location accessible to this process's account.
    let base = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => {
            eprintln!("linklet-agent: cannot read the working directory: {error}");
            std::process::exit(2);
        }
    };
    let paths = match Destination::new(&base.to_string_lossy()) {
        Ok(destination) => destination,
        Err(error) => {
            eprintln!("linklet-agent: working directory: {error}");
            std::process::exit(2);
        }
    };

    // The log, opened before the port is bound for the same reason as the token:
    // an operator who asked for a log and silently did not get one has a machine
    // whose evidence they believe exists and does not. That is the mistake this feature
    // is a reaction to.
    //
    // **Where it goes by default, and why that is not the working directory.** The
    // default is `logs/agent.log` beside the executable. A working directory is wrong
    // for this deployment: a scheduled task starts its program with the scheduler's working
    // directory, not the operator's, so the default would land in `System32\logs` or
    // somewhere equally useless on exactly the machines that run unattended.
    //
    // **A failure to open means two different things, and they are treated differently.**
    // An explicit `--log`, or `LINKLET_LOG`, is a request: if it cannot be honoured the
    // agent refuses to start, because evidence the operator believes exists and does not
    // is the failure this whole feature answers. The *default* location is a convenience
    // nobody asked for, so failing to create it is a warning and the agent serves on --
    // refusing to start because a housekeeping directory was not writable would trade a
    // working machine for a convenience.
    let (request_log, log_path) = if no_log {
        if log_path.is_some() {
            eprintln!("linklet-agent: --log and --no-log were both given; name one of them");
            std::process::exit(2);
        }
        (log::RequestLog::none(), None)
    } else {
        match &log_path {
            Some(path) => match log::RequestLog::open(Path::new(path), Rotation::DEFAULT) {
                Ok(log) => (log, Some(path.clone())),
                Err(reason) => {
                    eprintln!("linklet-agent: --log: {reason}");
                    std::process::exit(2);
                }
            },
            None => {
                let path = default_log_path();
                match log::RequestLog::open(&path, Rotation::DEFAULT) {
                    Ok(log) => (log, Some(path.to_string_lossy().into_owned())),
                    Err(reason) => {
                        eprintln!(
                            "linklet-agent: cannot keep a log at {}: {reason}",
                            path.display()
                        );
                        eprintln!(
                            "linklet-agent: serving without one. Pass --log <file> to choose \
                             where it goes, or --no-log to say this is intended."
                        );
                        (log::RequestLog::none(), None)
                    }
                }
            }
        }
    };

    // Bound before the banner is printed, so that "listening on" is only said
    // once it is true. A message that claims something before trying it is the
    // failure mode this whole project is a reaction to.
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("linklet-agent: cannot listen on port {port}: {error}");
            std::process::exit(1);
        }
    };

    // The port actually bound, not the port requested.
    //
    // Binding to port 0 succeeds and lets the OS choose, so printing the
    // requested number would be a banner that lies -- and it did, until a test
    // read it back and could not connect. On a real port the two numbers are the
    // same and this costs nothing; on 0 it is the difference between an
    // announcement and a guess.
    let bound = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            eprintln!("linklet-agent: listening but cannot read the address back: {error}");
            std::process::exit(1);
        }
    };

    // Name the relative-path base and the log without implying a directory boundary.
    match &log_path {
        Some(path) => println!(
            "linklet-agent listening on 0.0.0.0:{bound} file access as process account; relative paths from {} log {path}",
            paths.base().display()
        ),
        None => println!(
            "linklet-agent listening on 0.0.0.0:{bound} file access as process account; relative paths from {} no log",
            paths.base().display()
        ),
    }

    println!("Pairing code: {}", managed.code());

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // A thread per connection. The agent answers a handful of calls
                // at a time and each one is a command the caller asked for; a
                // pool would be machinery bought with nothing.
                let token = token.clone();
                let paths = paths.clone();
                let log = request_log.clone();
                let pairing = pairing.clone();
                std::thread::spawn(move || {
                    server::serve_connection(stream, &token, &paths, &log, &pairing)
                });
            }
            // One failed accept is not a reason to stop serving. The listener is
            // still bound and the next caller may be fine.
            Err(error) => eprintln!("linklet-agent: a connection failed: {error}"),
        }
    }
}
