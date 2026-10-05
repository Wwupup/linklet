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
//! - **No output cap, and a ceiling it does not get to choose.** A command that
//!   writes a gigabyte writes a gigabyte, and the agent holds all of it before it
//!   knows the reply will not fit. What it does about that is refuse by name with
//!   both stream sizes in the reason, rather than closing the connection and leaving
//!   the caller to conclude the network failed -- see `server::run_reply`. Holding
//!   the bytes at all is the part a streaming reply would remove, and that is a
//!   protocol change rather than this one; `docs/ROADMAP.md` M10 records it as the
//!   shape not taken.

#![forbid(unsafe_code)]

mod execute;
mod log;
mod server;
mod spawn;

use std::net::TcpListener;
use std::path::{Path, PathBuf};

use linklet_core::auth::{Token, secret_in_file};
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
/// used for why this is not the working directory, which is what `--root` defaults to.
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
  linklet-agent [--port <port>] [--root <directory>] [--log <file> | --no-log]

options:
  --port <port>   the port to listen on (default 8787)
  --token <secret>  the shared secret callers must present (or LINKLET_TOKEN)
  --token-file <file>  read the secret from the first line of this file (or
                      LINKLET_TOKEN_FILE). A byte-order mark and the line ending
                      are not part of it. Name this or --token, not both -- the
                      two are two answers to one question
  --root <directory>  the only directory a transfer may write in or read from, and it
                      must exist (default: the directory the agent was started in)
  --log <file>    where the request log goes (or LINKLET_LOG). Default: logs/agent.log
                  in the directory the agent's own executable is in, rolled over when
                  it reaches a mebibyte with three older files kept
  --no-log        keep no log. A request that never finishes then leaves no evidence
                  behind, which is the one thing the log exists to answer
  -h, --help      print this
";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let mut port = DEFAULT_PORT;
    let mut root: Option<String> = None;
    let mut token: Option<String> = std::env::var("LINKLET_TOKEN").ok();
    let mut token_file: Option<String> = std::env::var("LINKLET_TOKEN_FILE").ok();
    let mut log_path: Option<String> = std::env::var("LINKLET_LOG").ok();
    let mut no_log = false;
    let mut iterator = arguments.iter();
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
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
            "--root" => match iterator.next() {
                Some(value) => root = Some(value.clone()),
                None => {
                    eprintln!("linklet-agent: --root needs a directory");
                    std::process::exit(2);
                }
            },
            "--token" => match iterator.next() {
                Some(value) => token = Some(value.clone()),
                None => {
                    eprintln!("linklet-agent: --token needs a value");
                    std::process::exit(2);
                }
            },
            "--token-file" => match iterator.next() {
                Some(value) => token_file = Some(value.clone()),
                None => {
                    eprintln!("linklet-agent: --token-file needs a file");
                    std::process::exit(2);
                }
            },
            other => {
                eprintln!("linklet-agent: unexpected argument {other:?}");
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

    // One secret, named once. Two sources are refused rather than ordered: the two
    // are two answers to one question, and whichever lost would be the one the
    // operator believed was in force -- so a caller would be told its token was
    // wrong while the secret it presented was the right one for the file.
    if token.is_some() && token_file.is_some() {
        eprintln!(
            "linklet-agent: a token and a token file were both given; the secret is one \
             thing, so pass --token or --token-file, not both"
        );
        std::process::exit(2);
    }

    // Read here, with the token it stands in for and before the port is bound. A
    // file that was named and cannot be read is refused rather than answered from
    // the environment: falling back would authenticate with a secret nobody named,
    // and the caller would be told its token is wrong.
    let token = match token_file {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(contents) => Some(secret_in_file(&contents).to_string()),
            Err(error) => {
                eprintln!("linklet-agent: cannot read the token file {path}: {error}");
                std::process::exit(2);
            }
        },
        None => token,
    };

    // Checked before the port is bound, so that a bad secret is a startup failure
    // rather than a surprise at the first caller. A configuration error should be
    // loud when it is made.
    let Some(token) = token else {
        eprintln!(
            "linklet-agent: no token. Pass --token <secret>, name a --token-file, or set \
             LINKLET_TOKEN.\\n\\
             Anyone who can reach this port will be able to run commands without one."
        );
        std::process::exit(2);
    };
    let token = match Token::new(token) {
        Ok(token) => token,
        Err(error) => {
            eprintln!("linklet-agent: {error}");
            std::process::exit(2);
        }
    };

    // The one directory a transfer may touch. Checked here, before the port is bound,
    // for the same reason the token is: a configuration error should be loud when it is
    // made rather than at the first caller who tries to push a file.
    //
    // The default is the directory the operator started the agent in, made absolute.
    // That is a decision rather than a convenience: it means the agent can write
    // *somewhere* the moment it runs, and it is somewhere the operator chose by being
    // there. Printing it in the banner is what turns that into something they can see.
    let root = match root {
        Some(text) => text,
        None => match std::env::current_dir() {
            Ok(directory) => directory.to_string_lossy().into_owned(),
            Err(error) => {
                eprintln!("linklet-agent: cannot read the working directory: {error}");
                eprintln!("linklet-agent: pass --root <directory> to say where transfers go");
                std::process::exit(2);
            }
        },
    };
    let root = match Destination::new(&root) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("linklet-agent: --root: {error}");
            std::process::exit(2);
        }
    };

    // And that it is a directory that exists, checked here rather than at the first
    // transfer. An agent that accepted a root which is not there would start happily and
    // then fail **every** transfer with a filesystem error naming a path nobody typed --
    // which is what happened on the first real machine this ran against. A configuration
    // error should be loud when it is made.
    if !root.root().is_dir() {
        eprintln!(
            "linklet-agent: --root {} is not a directory",
            root.root().display()
        );
        eprintln!("linklet-agent: create it, or pass --root with one that exists");
        std::process::exit(2);
    }

    // The log, opened before the port is bound for the same reason as the token and the
    // root: an operator who asked for a log and silently did not get one has a machine
    // whose evidence they believe exists and does not. That is the mistake this feature
    // is a reaction to.
    //
    // **Where it goes by default, and why that is not the working directory.** The
    // default is `logs/agent.log` beside the executable. A working directory would match
    // `--root`'s default and would be easier to test, and it is wrong for the deployment
    // this exists for: a scheduled task starts its program with the scheduler's working
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

    // The root is on the banner because it is the answer to "what can this agent write
    // to", which is the first question a person who is about to push a build should be
    // able to answer without reading a manual. The log is on it for the same reason and
    // one more: a person reading the log later needs to know it is the log this agent is
    // writing, and the file it names is the answer.
    match &log_path {
        Some(path) => println!(
            "linklet-agent listening on 0.0.0.0:{bound} transfers under {} log {path}",
            root.root().display()
        ),
        None => println!(
            "linklet-agent listening on 0.0.0.0:{bound} transfers under {} no log",
            root.root().display()
        ),
    }

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // A thread per connection. The agent answers a handful of calls
                // at a time and each one is a command the caller asked for; a
                // pool would be machinery bought with nothing.
                let token = token.clone();
                let root = root.clone();
                let log = request_log.clone();
                std::thread::spawn(move || server::serve_connection(stream, &token, &root, &log));
            }
            // One failed accept is not a reason to stop serving. The listener is
            // still bound and the next caller may be fine.
            Err(error) => eprintln!("linklet-agent: a connection failed: {error}"),
        }
    }
}
