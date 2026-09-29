//! The target-side agent.
//!
//! One file, no dependencies, and it does two things: says which agent it is, and
//! runs a command when asked. Everything it knows about the protocol comes from
//! `linklet_core::wire`, which the host uses too, so the two cannot drift into
//! two readings of the same message.
//!
//! # Why this is a separate binary
//!
//! The host's `linklet` runs on a development machine. This runs on the machine
//! being debugged, which may be a rack, a build box, or the laptop next to it.
//! Making them one binary would mean shipping the whole host tool to every target
//! and would put "can run a command" behind the same door as "can check a port".
//!
//! It is also the reason the agent has nothing but `linklet-core` in its
//! dependency list: the smaller the thing that runs on someone else's machine,
//! the less there is to be wrong with it.
//!
//! # What it does not do
//!
//! - **No authentication.** The token and the encrypted transport are not built
//!   yet, so this listens on whatever it is told and answers anyone who can reach
//!   the port. That is not a small omission: it belongs next in line, and until
//!   then this is a tool for a network you control.
//! - **No keep-alive, no chunked encoding, one request per connection.** See
//!   `http.rs` for why each of those is a refusal rather than a gap.
//! - **No output cap.** A command that writes a gigabyte writes a gigabyte. The
//!   limit belongs in the wire protocol as a field, so a caller that had its
//!   output cut can tell.

#![forbid(unsafe_code)]

mod execute;
mod http;

use std::net::TcpListener;

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
  -h, --help      print this
";

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let mut port = DEFAULT_PORT;
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
            other => {
                eprintln!("linklet-agent: unexpected argument {other:?}");
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

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
                std::thread::spawn(move || http::serve_connection(stream));
            }
            // One failed accept is not a reason to stop serving. The listener is
            // still bound and the next caller may be fine.
            Err(error) => eprintln!("linklet-agent: a connection failed: {error}"),
        }
    }
}
