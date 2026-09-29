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
//! - **No output cap.** A command that writes a gigabyte writes a gigabyte. The
//!   limit belongs in the wire protocol as a field, so a caller that had its
//!   output cut can tell.

#![forbid(unsafe_code)]

mod execute;
mod server;

use std::net::TcpListener;

use linklet_core::auth::Token;

/// The port the agent listens on when it is not told.
///
/// A constant so that the host and the agent agree by reading one place rather
/// than two that were typed the same way once.
pub const DEFAULT_PORT: u16 = 8787;

const USAGE: &str = "\
linklet-agent -- run a command on this machine when a host asks

usage:
  linklet-agent [--port <port>]

options:
  --port <port>   the port to listen on (default 8787)
  --token <secret>  the shared secret callers must present (or LINKLET_TOKEN)
  -h, --help      print this
";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let mut port = DEFAULT_PORT;
    let mut token: Option<String> = std::env::var("LINKLET_TOKEN").ok();
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
            "--token" => match iterator.next() {
                Some(value) => token = Some(value.clone()),
                None => {
                    eprintln!("linklet-agent: --token needs a value");
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

    // Checked before the port is bound, so that a bad secret is a startup failure
    // rather than a surprise at the first caller. A configuration error should be
    // loud when it is made.
    let Some(token) = token else {
        eprintln!(
            "linklet-agent: no token. Pass --token <secret> or set LINKLET_TOKEN.\\n\\
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

    println!("linklet-agent listening on 0.0.0.0:{bound}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // A thread per connection. The agent answers a handful of calls
                // at a time and each one is a command the caller asked for; a
                // pool would be machinery bought with nothing.
                let token = token.clone();
                std::thread::spawn(move || server::serve_connection(stream, &token));
            }
            // One failed accept is not a reason to stop serving. The listener is
            // still bound and the next caller may be fine.
            Err(error) => eprintln!("linklet-agent: a connection failed: {error}"),
        }
    }
}
