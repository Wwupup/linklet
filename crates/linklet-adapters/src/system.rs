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

/// Running processes whose name matches.
///
/// **One implementation for both platforms, and it moved to get there.** This was
/// `tasklist /FI "IMAGENAME eq ..."` written here, next to the prober -- which meant the
/// `no-process` requirement, the one that makes a testbed worth having on a machine somebody
/// prepared by hand, could only be checked on Windows. The knowledge of how to read a process
/// list belongs with everything else that reads one (`crate::processes`), so this is now a call
/// rather than a program: `tasklist` on Windows, `/proc` on Linux, the same verb.
///
/// The match stays **exact**, which is the semantics the Windows version had and the one the
/// tests pin: `--name` on `ps` is a substring because it is casting a net over what to act on,
/// and this is asking whether one named process is there.
///
/// A failure is reported as `Unknown` with the reason, not as "nothing is running". Those lead
/// to opposite actions -- one says the machine is clean, the other says the machine cannot be
/// seen -- and confusing them is how a test runs against a machine that still has yesterday's
/// process on it.
fn processes_named(name: &str) -> Observation {
    match crate::processes::named(name) {
        Ok(found) => Observation::Processes(found),
        Err(reason) => Observation::Unknown(reason),
    }
}
