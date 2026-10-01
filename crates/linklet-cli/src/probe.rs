//! A probe that answers the question a supervisor has to ask: **is the agent working, or only
//! listening?**
//!
//! `check` opens a TCP connection and closes it, which is the right answer to "is anything
//! there" and the wrong one to "is this agent alive". A process wedged on a lock still has its
//! listening socket open, so the kernel keeps accepting connections into the backlog and
//! `check` reports a healthy machine while every real call times out. That is the failure
//! `docs/ROADMAP.md` M10 opens with -- *"the caller saw a connect timeout rather than a
//! refusal"* -- and a supervisor built on `check` would never restart it.
//!
//! So this does the smallest thing that is still a conversation: connect, complete the
//! handshake, ask for `identity`, and read the answer. **A wedged agent fails it and a working
//! one passes it**, which is the whole of what a supervisor needs to know.
//!
//! # The exit codes are the interface
//!
//! `docs/ROADMAP.md` asks for a probe "whose three exit codes are documented", and they are
//! chosen so a monitoring script can act on them without reading a word. Three of the four are
//! `linklet_core::ExitCode`'s own, so a caller who knows those already knows these:
//!
//! | code | meaning | what a supervisor does |
//! |------|---------|------------------------|
//! | 0 `SUCCESS` | the agent answered | nothing |
//! | 1 `NOT_ALL_ALIVE` | nothing is listening | start it |
//! | 2 `USAGE` | the spec could not be read | fix the script |
//! | 4 `NO_ANSWER` | something is listening and did not answer | kill it, then start it |
//!
//! **`NO_ANSWER` is the one code that is not `ExitCode`'s**, and it is 4 rather than 3 on
//! purpose: 3 is `REFUSED`, which this project uses for "the invocation was wrong before
//! anything was looked at". Reusing it for a wedged agent would make a typo and an outage the
//! same answer.
//!
//! **A wrong token is code 0.** The question is liveness and not authorization: an agent that
//! says no has answered, so it is working, and a supervisor that restarted it would be
//! restarting a healthy process because the operator's secret is wrong. `docs/smoke.md` says so
//! next to the table.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use linklet_client::render_call_error;
use linklet_core::ExitCode;
use linklet_core::auth::Token;

/// The agent accepted the connection and never answered on it.
///
/// The fourth code, and the one `linklet_core::ExitCode` has no name for because nothing else in
/// this tool needs to say it. See the table above for why it is 4 and not 3.
pub const NO_ANSWER: u8 = 4;

/// How long each half of the probe may take.
///
/// Short by design, and measured against what it watches: the bench agent answers a full
/// handshake in about **25 ms**, so two seconds is a hundred times longer than a healthy agent
/// needs. A machine that cannot complete a handshake in two seconds gets restarted, which is
/// the right answer for a service that far past its own normal time.
const PROBE_BUDGET: Duration = Duration::from_secs(2);

/// Probes one agent and returns the exit code.
///
/// See the module docs for the four codes. A spec nobody can parse is `USAGE` rather than
/// "nothing is there", because a typo is not an outage -- and a supervisor that read one as an
/// outage would restart the agent every time somebody edited the script.
pub fn run_probe(agent: &str, token: Option<&Token>) -> u8 {
    let address = match linklet_client::AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => {
            eprintln!("linklet: {}", render_call_error(&error));
            return ExitCode::USAGE;
        }
    };

    // Resolved before anything is opened, so that a name that does not resolve is reported as
    // the script's problem rather than as a machine that is down.
    let socket = match agent.to_socket_addrs().map(|mut found| found.next()) {
        Ok(Some(socket)) => socket,
        Ok(None) => {
            eprintln!("linklet: {agent} resolves to no addresses");
            return ExitCode::USAGE;
        }
        Err(error) => {
            eprintln!("linklet: cannot resolve {agent}: {error}");
            return ExitCode::USAGE;
        }
    };

    // **The first question, kept separate from the second.** Is anything listening at all?
    // Folding the two together would make a machine that is off indistinguishable from a
    // machine that is up and not answering, and those are the two codes a supervisor acts on
    // differently: one gets started and the other gets killed first.
    let connected = TcpStream::connect_timeout(&socket, PROBE_BUDGET);
    if connected.is_err() {
        println!("nothing listening at {agent}");
        return ExitCode::NOT_ALL_ALIVE;
    }
    // Dropped so the agent serves one connection per probe rather than two.
    drop(connected);

    // **The second question: is it working?** The same call a real one makes, so whatever would
    // wedge a real request wedges this -- and it is `identity` because that is the only request
    // this project has with no side effect at all.
    let address = match token {
        Some(token) => address.with_token(token.clone()),
        None => address,
    };

    match linklet_client::probe(&address, PROBE_BUDGET) {
        // Both arms are the agent working: it spoke. Which word it said is in the message for a
        // reader and is not the question being asked -- see the table above, and
        // `linklet_client::Liveness`, which is where that decision is made.
        Ok(liveness) => {
            println!("{} {agent}: {}", liveness.as_str(), liveness.detail());
            ExitCode::SUCCESS
        }
        // The socket accepted and nothing usable came back, which is the wedged case this
        // command exists for. The reason goes to stdout rather than stderr because a supervisor
        // that restarted on this will want it in whatever it logs.
        Err(error) => {
            println!("no answer from {agent}: {}", render_call_error(&error));
            NO_ANSWER
        }
    }
}
