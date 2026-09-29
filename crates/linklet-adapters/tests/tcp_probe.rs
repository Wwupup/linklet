//! The real probe, against real sockets, on this machine.
//!
//! Note what this file can and cannot cover. It can cover "a listener accepts"
//! and "a closed port reports a refusal", because a laptop can produce both --
//! though on Windows the second takes about two seconds to surface, which is why
//! the budgets here are generous. It **cannot** cover a timeout that is a fact
//! about a network without risking a test that takes ten seconds, or that fails
//! on a laptop with an unusual route.
//!
//! That gap is the argument for the rest of the design, stated as a fact rather
//! than an opinion: the behaviour that is awkward to test here -- what a timeout
//! means, what happens to the run when one target fails -- is tested in
//! `linklet-core/tests/probe_check.rs`, with a fake, in milliseconds. What is
//! left here is the small part that genuinely needs a socket.

use std::net::TcpListener;
use std::time::{Duration, Instant};

use linklet_adapters::{MIN_BUDGET, TcpProbe, effective_budget};
use linklet_core::{Host, MAX_BUDGET_SECONDS, Port, Probe, ProbeOutcome, Target};

/// Generous enough to cover the ~2 s a refused connection takes on Windows.
const BUDGET: Duration = Duration::from_secs(6);

/// Builds a target pointing at `127.0.0.1` on the given port.
fn local(port: u16) -> Target {
    Target {
        host: Host::new("127.0.0.1"),
        port: Port::new(port),
    }
}

/// Binds a listener on an arbitrary free port and returns it with that port.
///
/// Port 0 asks the OS for a free port and `local_addr` reports which one it
/// chose, so the test never guesses a port number and never collides with
/// something else on the machine -- the usual cause of a test that passes alone
/// and fails in a suite.
fn listening() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binding an ephemeral port should work");
    let port = listener
        .local_addr()
        .expect("a bound listener has an address")
        .port();
    (listener, port)
}

/// Binds and immediately drops a listener, leaving the port closed.
fn closed_port() -> u16 {
    let (listener, port) = listening();
    drop(listener);
    port
}

#[test]
fn a_listening_port_answers() {
    let (_listener, port) = listening();

    assert_eq!(TcpProbe.probe(&local(port), BUDGET), ProbeOutcome::Answered);
}

#[test]
fn a_listening_port_answers_immediately() {
    // A real listener accepts in microseconds, so the budget must not be slept
    // away. This is the test that catches a probe that waits for its deadline
    // instead of for the answer. Measured at 137 microseconds on the machine
    // this was written on; the bound is loose because CI machines are busy, and
    // the failure it catches is a wait measured in seconds.
    let (_listener, port) = listening();
    let started = Instant::now();

    assert_eq!(TcpProbe.probe(&local(port), BUDGET), ProbeOutcome::Answered);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an answered probe took {:?}, which means it waited for something",
        started.elapsed()
    );
}

#[test]
fn a_port_with_nothing_behind_it_refuses_rather_than_reporting_no_answer() {
    // Binding and immediately dropping the listener leaves the port closed. The
    // OS then refuses connections to it, which is the same thing a target
    // machine does when the program under test is not running -- the case this
    // tool exists to distinguish from "the machine is off".
    //
    // This is the assertion that the budget floor earns its keep. With the
    // budget under two seconds, `connect_timeout` returns a synthetic timeout
    // before the refusal arrives, and this test fails with `NoAnswer` -- which
    // is precisely the wrong advice to give a user whose program simply is not
    // running.
    let port = closed_port();

    assert_eq!(TcpProbe.probe(&local(port), BUDGET), ProbeOutcome::Refused);
}

#[test]
fn a_host_that_cannot_be_resolved_is_reported_with_the_reason() {
    // `.invalid` is reserved by RFC 2606 and can never resolve, which makes it
    // a better choice than a plausible-looking fake name.
    //
    // An empty host is *not* a good choice here, and this was learned the hard
    // way: `("" , 80).to_socket_addrs()` resolves to nine addresses on this
    // machine -- six IPv6 and three IPv4 -- so it is "this machine", not
    // "nonsense". A test built on the wrong assumption about that would have
    // been testing the multi-address path while claiming to test resolution.
    let target = Target {
        host: Host::new("no-such-host.invalid"),
        port: Port::new(80),
    };

    match TcpProbe.probe(&target, BUDGET) {
        // The message is the adapter's, so the assertion is on the shape rather
        // than the wording: it must name what could not be resolved, and it must
        // not be flattened into a timeout or a refusal.
        ProbeOutcome::Error(message) => assert!(
            message.contains("cannot resolve"),
            "the error should say what failed, got: {message}"
        ),
        other => panic!("an unresolvable host is not {other:?}"),
    }
}

/// The measured time a refused loopback connection takes to surface on Windows,
/// from the table in `src/tcp.rs`. Named rather than inlined so that a test
/// comparing against it reads as a comparison against a measurement.
const MEASURED_REFUSAL: Duration = Duration::from_millis(2050);

#[test]
fn the_budget_floor_has_headroom_over_the_measured_refusal_time() {
    // The floor exists because a refused loopback connection on Windows takes
    // about two seconds to surface, and a budget below that produces a synthetic
    // timeout instead of the refusal.
    //
    // The factor matters rather than the exact number: a floor set *at* the
    // measurement loses the race on a busy machine, and losing it turns "the
    // machine is up and nothing is listening" into "the machine did not answer".
    // So the constant is checked against the measurement instead of trusted, and
    // the reason is in the failure message where the person lowering it will
    // read it.
    assert!(
        MIN_BUDGET >= MEASURED_REFUSAL * 2,
        "MIN_BUDGET is {MIN_BUDGET:?}, which is less than twice the {MEASURED_REFUSAL:?} \
         a refused connection takes on Windows -- not enough headroom for a busy machine"
    );
}

#[test]
fn the_floor_wins_over_a_smaller_request_and_the_ceiling_over_a_larger_one() {
    // Tested as a function, not as a stopwatch. The earlier version of this test
    // asserted that a probe with a 100 ms budget took at least MIN_BUDGET,
    // which was wrong twice over: a refusal can surface *before* the floor
    // elapses (that is the whole point of the floor -- the answer arrived inside
    // the window), and timing a real connection to check a constant is slower
    // and flakier than calling the function that computes it.
    assert_eq!(effective_budget(Duration::from_millis(100)), MIN_BUDGET);
    assert_eq!(effective_budget(MIN_BUDGET), MIN_BUDGET);
    assert_eq!(
        effective_budget(Duration::from_secs(3600)),
        Duration::from_secs(MAX_BUDGET_SECONDS)
    );
    // Inside the two ends, a request is honoured exactly.
    assert_eq!(
        effective_budget(Duration::from_secs(7)),
        Duration::from_secs(7)
    );
}

#[test]
fn a_budget_below_the_floor_still_produces_the_refusal_rather_than_silence() {
    // The behavioural half of the test above: this is what the floor buys. With
    // a 100 ms budget and no floor, `connect_timeout` gives up at 100 ms with a
    // synthetic timeout and this assertion sees `NoAnswer` -- the wrong advice
    // for a user whose program simply is not running.
    let port = closed_port();

    assert_eq!(
        TcpProbe.probe(&local(port), Duration::from_millis(100)),
        ProbeOutcome::Refused,
        "a 100 ms budget produced silence instead of the refusal the machine gave"
    );
}
