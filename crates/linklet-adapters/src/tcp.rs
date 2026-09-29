//! The real probe: a TCP connection attempt with a deadline.
//!
//! This is the only place in the project that opens a socket, and it exists to
//! implement a trait declared somewhere else. It decides nothing: what counts
//! as reachable, how long a budget is, and what a timeout means are all
//! [`linklet_core`]'s answers, not this module's.
//!
//! # One thing this platform does that has to be worked around
//!
//! Measured on Windows, on loopback, ten times in a row:
//!
//! | attempt | result | time |
//! |---|---|---|
//! | `connect_timeout`, budget 200 ms / 500 ms / 1 s / 2 s | `TimedOut`, `raw_os_error` = `None` | exactly the budget |
//! | `connect_timeout`, budget 3 s | `ConnectionRefused`, raw 10061 | about 2.05 s |
//! | plain `connect`, no timeout | `ConnectionRefused`, raw 10061 | about 2.05 s |
//! | a listening port | connected | 137 microseconds |
//!
//! A refused connection on this platform takes about two seconds to surface.
//! Under a shorter budget the call gives up first and reports a *synthesized*
//! timeout with no OS error behind it -- the real reason had not arrived yet.
//! The consequence is not academic: it is the difference between telling a user
//! "the machine is up and nothing is listening" and telling them "the machine
//! did not answer", which send them to completely different places.
//!
//! So the budget has a floor of two seconds (see [`MIN_BUDGET`]). The honest
//! statement of that trade is: **this probe may take two seconds to answer a
//! question the caller asked to have answered in 200 ms, because below two
//! seconds the answer it would give is wrong.** Taking longer and being right
//! beats being quick and misleading.

use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use linklet_core::{MAX_BUDGET_SECONDS, Probe, ProbeOutcome, Target};

/// The shortest budget this adapter will actually use.
///
/// Not a preference: a refused connection takes about 2.05 seconds to surface on
/// Windows (measured ten runs, see the module documentation), and a budget below
/// that produces a synthetic timeout instead of the refusal. This is 2.4 times
/// the measurement -- headroom, because a threshold set *at* a measured value
/// loses the race on a busy machine, and losing it turns "the machine is up and
/// nothing is listening" into "the machine did not answer".
///
/// The cost is real and stated rather than hidden: a caller asking for 200 ms
/// waits up to five seconds. Below about two, the answer would be wrong.
///
/// This is a *budget* floor, not a duration. A refusal that surfaces at 2.04 s
/// still returns at 2.04 s; the floor only guarantees the probe will not give up
/// before the answer could have arrived.
pub const MIN_BUDGET: Duration = Duration::from_secs(5);

/// The real probe over TCP.
///
/// One `connect` per candidate address, each with the full budget. The budget
/// is not divided among addresses, for the same reason it is not divided among
/// targets in the core: "two seconds" is a promise both sides can reason about,
/// and "two seconds shared between however many addresses this name resolves
/// to" is a number nobody can predict.
#[derive(Debug, Clone, Copy, Default)]
pub struct TcpProbe;

impl Probe for TcpProbe {
    fn probe(&self, target: &Target, budget: Duration) -> ProbeOutcome {
        let budget = budget_for(budget);
        let spec = format!("{}:{}", target.host.as_str(), target.port.get());

        let addresses: Vec<SocketAddr> = match spec.to_socket_addrs() {
            Ok(addresses) => addresses.collect(),
            // Resolution failure is not a timeout and not a refusal. It is the
            // one case where the adapter has something specific to say, so it
            // says it rather than flattening it into "no".
            Err(error) => return ProbeOutcome::Error(format!("cannot resolve {spec}: {error}")),
        };

        if addresses.is_empty() {
            // `ToSocketAddrs` produces an empty list for a name that resolves to
            // nothing rather than reporting an error, and reporting that as
            // "no answer" would blame the network for a name problem.
            return ProbeOutcome::Error(format!("{spec} resolved to no addresses"));
        }

        let mut last: Option<ProbeOutcome> = None;

        for address in &addresses {
            match TcpStream::connect_timeout(address, budget) {
                Ok(stream) => {
                    // Closing immediately is deliberate: this probe answers a
                    // question, it does not keep a session.
                    drop(stream);
                    return ProbeOutcome::Answered;
                }
                Err(error) => {
                    let outcome = classify(error);
                    match outcome {
                        // Only these are worth trying the next address for: a
                        // refused address has told us everything about itself,
                        // and a verdict we are unsure of is not a reason to
                        // spend the budget again.
                        ProbeOutcome::TimedOut => last = Some(ProbeOutcome::NoAnswer),
                        ProbeOutcome::NoAnswer => last = Some(ProbeOutcome::NoAnswer),
                        definite => return definite,
                    }
                }
            }
        }

        last.unwrap_or(ProbeOutcome::NoAnswer)
    }
}

/// Applies both ends of the budget: the core's ceiling, and this platform's
/// floor.
///
/// Kept a pure function of one argument, and free of any socket, so the policy
/// can be tested directly rather than inferred from how long a connection took.
/// A timing-based test of a budget is a test that fails on a busy machine; a
/// function call is not.
fn budget_for(requested: Duration) -> Duration {
    requested
        .max(MIN_BUDGET)
        .min(Duration::from_secs(MAX_BUDGET_SECONDS))
}

/// Applies the floor and the ceiling to a requested budget, and returns it.
///
/// Public so that the policy can be tested as a function rather than inferred
/// from how long a connection took: a timing-based test of a budget fails on a
/// busy machine, and a function call does not.
///
/// The returned value is what [`TcpProbe`] would actually spend. Note that it is
/// a budget, not a duration -- a refusal that surfaces before the budget elapses
/// still returns early. See [`MIN_BUDGET`].
pub fn effective_budget(requested: Duration) -> Duration {
    budget_for(requested)
}

/// Maps an OS failure onto what the caller is told.
///
/// The `raw_os_error` check is the part that matters. When `connect_timeout`
/// gives up early it reports `TimedOut` with **no** OS error behind it -- a
/// verdict the runtime invented, not one the machine reached. Only an error
/// that carries a code is a real answer, so only that is allowed to become
/// [`ProbeOutcome::Refused`]. Everything else is downgraded to
/// [`ProbeOutcome::NoAnswer`], which is a weaker claim and a true one.
///
/// Anything unrecognised becomes [`ProbeOutcome::Error`] rather than being
/// rounded to "unreachable": a verdict this code is not sure of should not be
/// dressed up as a fact about the network.
fn classify(error: io::Error) -> ProbeOutcome {
    let code = error.raw_os_error();
    match error.kind() {
        io::ErrorKind::ConnectionRefused if code.is_some() => ProbeOutcome::Refused,
        io::ErrorKind::TimedOut if code.is_some() => ProbeOutcome::TimedOut,
        io::ErrorKind::TimedOut => ProbeOutcome::NoAnswer,
        io::ErrorKind::ConnectionRefused => ProbeOutcome::NoAnswer,
        _ => ProbeOutcome::Error(error.to_string()),
    }
}
