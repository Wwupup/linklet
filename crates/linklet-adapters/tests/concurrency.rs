//! The one measurement the core cannot make: that the waits actually overlap.
//!
//! `linklet-core/tests/concurrent_check.rs` proves the concurrent run gives the
//! same answers in the same order, and says in its own header that it cannot
//! prove the waits overlap -- a fake probe returns immediately, so a concurrent
//! run of ten takes microseconds whether or not a thread was involved.
//!
//! This file is the other half. It uses a probe that really sleeps and a real
//! clock, which is the only way to tell a speedup from a coincidence.

use std::time::{Duration, Instant};

use linklet_adapters::TcpProbe;
use linklet_core::{
    Host, MAX_TARGETS, Port, Probe, ProbeOutcome, Target, check_targets, check_targets_concurrent,
};

/// A probe that spends a fixed time before answering, like a machine that is not
/// there.
///
/// `Sleep` rather than a real unreachable address, because a test that depends on
/// a network dropping packets is a test that fails on a laptop with a route to
/// it. What is being measured is the loop's structure, and a sleep measures that
/// exactly while being deterministic.
struct Sleepy {
    per_probe: Duration,
}

impl Probe for Sleepy {
    fn probe(&self, _target: &Target, _budget: Duration) -> ProbeOutcome {
        std::thread::sleep(self.per_probe);
        ProbeOutcome::NoAnswer
    }
}

fn targets(count: usize) -> Vec<Target> {
    (1..=count)
        .map(|i| Target {
            host: Host::new(format!("host{i}")),
            port: Port::new(80),
        })
        .collect()
}

#[test]
fn overlapping_makes_a_run_of_many_take_about_as_long_as_a_run_of_one() {
    // The claim, stated as a ratio rather than as a duration so that it holds on
    // a slow machine as well as a fast one.
    //
    // Ten probes of 200 ms each. Serial: about 2 s. One at a time is the
    // definition, so the serial number is also the control -- if the concurrent
    // run were secretly serial too, the two would match and this fails.
    const COUNT: usize = 10;
    const PER_PROBE: Duration = Duration::from_millis(200);

    let probe = Sleepy {
        per_probe: PER_PROBE,
    };
    let list = targets(COUNT);

    let serial_started = Instant::now();
    let serial = check_targets(&probe, &list, Duration::from_secs(5), MAX_TARGETS)
        .expect("a valid serial run");
    let serial_took = serial_started.elapsed();

    let concurrent_started = Instant::now();
    let concurrent =
        check_targets_concurrent(&probe, &list, Duration::from_secs(5), MAX_TARGETS, COUNT)
            .expect("a valid concurrent run");
    let concurrent_took = concurrent_started.elapsed();

    // Same answers, so the speedup is not bought with a different result.
    assert_eq!(concurrent, serial);

    // The control: without it, a "fast" concurrent run could just be a run that
    // did less work.
    assert!(
        serial_took >= PER_PROBE * (COUNT as u32) / 2,
        "the serial run took {serial_took:?}, which is too fast to be a control -- \
         it should be at least half of {} probes of {PER_PROBE:?}",
        COUNT
    );

    // A generous bound: everything at once should be near PER_PROBE, and the
    // assertion is set to catch the failure that matters -- being serial --
    // rather than to measure how fast this machine is.
    assert!(
        concurrent_took < PER_PROBE * 3,
        "the concurrent run of {COUNT} took {concurrent_took:?}, which is not overlapping: \
         {COUNT} probes of {PER_PROBE:?} should finish in roughly one"
    );
}

#[test]
fn the_at_once_limit_actually_limits() {
    // The other property, and the one a "just spawn one thread per target"
    // implementation would silently break. With two at a time, six probes of
    // 100 ms take about three rounds, not one.
    const COUNT: usize = 6;
    const PER_PROBE: Duration = Duration::from_millis(100);
    const AT_ONCE: usize = 2;

    let probe = Sleepy {
        per_probe: PER_PROBE,
    };
    let started = Instant::now();
    let _ = check_targets_concurrent(
        &probe,
        &targets(COUNT),
        Duration::from_secs(5),
        MAX_TARGETS,
        AT_ONCE,
    )
    .expect("a valid run");
    let took = started.elapsed();

    // Three rounds of 100 ms is 300 ms. The bound is loose at the bottom so the
    // test is not a scheduler measurement, and tight at the top so that ignoring
    // the limit -- one round, about 100 ms -- fails.
    assert!(
        took >= PER_PROBE * 2,
        "{COUNT} probes with {AT_ONCE} at once took {took:?}; the limit was not applied"
    );
}

#[test]
fn a_real_probe_answers_the_same_from_either_run() {
    // The end of the chain: not a fake, not a sleeper, the actual TCP probe
    // against actual sockets, through both functions. It is here because every
    // test above uses a stand-in, and the two run functions are only useful if
    // they agree about the real one too.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let closed = {
        // A port with nothing behind it. UDP and TCP have separate port spaces,
        // so binding a UDP socket reserves the number without a listener.
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind udp");
        socket.local_addr().expect("address").port()
    };

    let list = vec![
        Target {
            host: Host::new("127.0.0.1"),
            port: Port::new(port),
        },
        Target {
            host: Host::new("127.0.0.1"),
            port: Port::new(closed),
        },
    ];

    let serial = check_targets(&TcpProbe, &list, Duration::from_secs(6), MAX_TARGETS)
        .expect("a valid serial run");
    let concurrent =
        check_targets_concurrent(&TcpProbe, &list, Duration::from_secs(6), MAX_TARGETS, 8)
            .expect("a valid concurrent run");

    assert_eq!(concurrent, serial);
    assert!(
        concurrent[0].is_alive(),
        "the listener should answer: {concurrent:#?}"
    );
    assert!(
        !concurrent[1].is_alive(),
        "the closed port should not: {concurrent:#?}"
    );
}
