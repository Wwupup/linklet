//! Looking at a real machine.
//!
//! The only part of the testbed feature that touches anything. Everything it
//! decides -- whether an observation satisfies a requirement -- is in
//! `linklet_core::testbed`, which is why that logic has a table of tests and
//! this file has four.
//!
//! Every observation carries what was *seen*, never a verdict. A prober that
//! decided would be a prober whose decisions could only be tested on a machine
//! with the right things wrong with it.

use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use linklet_core::testbed::{Observation, PathKind, Prober, Requirement};

/// How long to wait for a connection when a requirement asks for one.
///
/// Short, and shorter than the budget a check run uses. This is not a question
/// about whether a service is healthy -- it is "is the port open yet", asked
/// repeatedly by a test driver that is waiting for something to start.
const CONNECT_BUDGET: Duration = Duration::from_secs(3);

/// Looks at the machine this process is running on.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProber;

impl Prober for SystemProber {
    fn observe(&self, requirement: &Requirement) -> Observation {
        match requirement {
            Requirement::Reachable(target) => reachable(target),
            Requirement::Present { path, kind } => match fs::metadata(path) {
                Ok(metadata) => {
                    let found = if metadata.is_dir() {
                        PathKind::Dir
                    } else {
                        PathKind::File
                    };
                    if found == *kind {
                        Observation::Exists
                    } else {
                        Observation::WrongKind { found }
                    }
                }
                Err(_) => Observation::Missing,
            },
            // A path that cannot be inspected is reported as missing here rather
            // than as `Unknown`. The distinction is deliberate in one direction
            // only: "the file is not there" and "the file could not be looked
            // at" lead to the same next action for a testbed, and a `NoProcess`
            // requirement is where not-knowing actually changes what to do.
            Requirement::Absent { path } => {
                if Path::new(path).exists() {
                    Observation::Exists
                } else {
                    Observation::Missing
                }
            }
            Requirement::NoProcess(name) => processes_named(name),
        }
    }
}

/// Whether something accepts a connection at `host:port`.
fn reachable(target: &str) -> Observation {
    let Ok(addresses) = target.to_socket_addrs() else {
        return Observation::Unknown(format!("cannot resolve {target}"));
    };

    let mut last = Observation::NoAnswer;
    let mut tried = false;

    for address in addresses {
        tried = true;
        match TcpStream::connect_timeout(&address, CONNECT_BUDGET) {
            Ok(stream) => {
                drop(stream);
                return Observation::Answered;
            }
            Err(error) => match error.kind() {
                // A refusal is a fact about the port, and trying the next
                // address would not change it.
                std::io::ErrorKind::ConnectionRefused => return Observation::Refused,
                // A synthetic timeout under the budget floor -- see `tcp.rs`.
                // Kept as "no answer" rather than promoted to a verdict.
                std::io::ErrorKind::TimedOut => last = Observation::NoAnswer,
                _ => last = Observation::Unknown(error.to_string()),
            },
        }
    }

    if tried {
        last
    } else {
        Observation::Unknown(format!("{target} resolved to nothing"))
    }
}

/// Running processes whose name matches, on Windows.
///
/// `tasklist` with a filter rather than anything cleverer: it is present on
/// every Windows machine, it needs no privileges for processes owned by the
/// current user, and a testbed is checking for the process *it* started.
///
/// A failure to run the command is reported as `Unknown` with the reason, not as
/// "nothing is running". Those lead to opposite actions -- one says the machine
/// is clean, the other says the machine cannot be seen -- and confusing them is
/// how a test runs against a machine that still has yesterday's process on it.
fn processes_named(name: &str) -> Observation {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("IMAGENAME eq {name}"), "/NH", "/FO", "CSV"])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) => return Observation::Unknown(format!("cannot run tasklist: {error}")),
    };

    // `tasklist` writes its "no tasks are running" notice to stdout with a
    // success code, so the exit status cannot be trusted for the answer. The
    // CSV rows are the answer, and there are none in that case.
    let text = String::from_utf8_lossy(&output.stdout);
    let found: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('"'))
        .filter_map(|line| line.split(',').next())
        .map(|field| field.trim_matches('"').to_string())
        .filter(|field| field.eq_ignore_ascii_case(name))
        .collect();

    Observation::Processes(found)
}
