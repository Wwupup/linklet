//! The specification for looking at many machines at once.
//!
//! The claim these tests exist to hold is narrow and important: **a caller must
//! not be able to tell from the result which function produced it.** Concurrency
//! is a change to when the waiting happens. The moment it starts changing what is
//! reported, it has become a feature with its own bugs rather than a faster way
//! to compute the same thing.
//!
//! So most of this file compares the two run functions against each other rather
//! than either against a hand-written expectation. A test that lists the answers
//! twice would pass while the two drifted.
//!
//! What cannot be tested here is that the waits actually overlap. A fake probe
//! returns immediately, so a concurrent run of ten takes microseconds whether or
//! not any threads were used. That belongs in `linklet-adapters`, with a probe
//! that really sleeps, and it is the one measurement this file cannot make.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use linklet_core::{
    CheckError, Host, MAX_TARGETS, Port, Probe, ProbeOutcome, Report, Status, Target,
    check_targets, check_targets_concurrent,
};

/// A probe with no machine behind it: scripted answers, and a record of the asks.
#[derive(Default)]
struct FakeProbe {
    /// The answer for each target, keyed by host.
    ///
    /// **Keyed by target and not by call order**, which is what this used to be
    /// and was a bug: a concurrent run does not promise to call the probe in the
    /// order the targets were listed -- the field below says so in as many words --
    /// so handing answers out by call index attached them to whichever target
    /// happened to be probed at that position. The test then asserted that each
    /// answer stayed with its own target, which under that arrangement is
    /// unsatisfiable: a correct implementation fails and a shuffling one can pass by
    /// coincidence. It passed for months on scheduling luck.
    scripted: BTreeMap<String, ProbeOutcome>,
    /// Every call made. Order is not asserted on -- the concurrent run cannot
    /// promise one -- but the count is.
    calls: AtomicUsize,
    /// The targets that were asked about, for the set comparison below.
    asked: Mutex<Vec<String>>,
}

impl FakeProbe {
    /// Pairs each target with its answer, by position in the list given.
    fn new(targets: &[Target], scripted: Vec<ProbeOutcome>) -> Self {
        let scripted = targets
            .iter()
            .map(|target| target.to_string())
            .zip(scripted)
            .collect();
        Self {
            scripted,
            calls: AtomicUsize::new(0),
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl Probe for FakeProbe {
    fn probe(&self, target: &Target, _budget: Duration) -> ProbeOutcome {
        // Counted, not indexed by: the count is what the assertions use, and the
        // order the calls arrive in is nobody's business.
        let _index = self.calls.fetch_add(1, Ordering::SeqCst);
        self.asked
            .lock()
            .expect("no panic while holding")
            .push(target.to_string());
        self.scripted
            .get(&target.to_string())
            .cloned()
            .unwrap_or_else(|| {
                // An outcome the assertions can see, rather than a panic that hides
                // the failure inside the harness.
                ProbeOutcome::Error(format!("not scripted: {target}"))
            })
    }
}

fn t(host: &str, port: u16) -> Target {
    Target {
        host: Host::new(host),
        port: Port::new(port),
    }
}

fn answer() -> ProbeOutcome {
    ProbeOutcome::Answered
}

fn refuse() -> ProbeOutcome {
    ProbeOutcome::Refused
}

fn no_answer() -> ProbeOutcome {
    ProbeOutcome::NoAnswer
}

const BUDGET: Duration = Duration::from_secs(2);

/// Runs both ways and asserts they agree, returning the concurrent result.
///
/// The heart of this file. Comparing the two to each other rather than each to a
/// written-out expectation is what makes drift between them a failure instead of
/// two passing tests.
fn both_ways_agree(targets: &[Target], scripted: Vec<ProbeOutcome>, at_once: usize) -> Vec<Report> {
    // A fresh probe for each run: the recorded calls must not be shared, and the
    // scripted answers are keyed by target rather than handed out in call order --
    // see `FakeProbe::scripted` for why that distinction is the whole test.
    let serial = check_targets(
        &FakeProbe::new(targets, scripted.clone()),
        targets,
        BUDGET,
        MAX_TARGETS,
    )
    .expect("a valid serial run");
    let concurrent = check_targets_concurrent(
        &FakeProbe::new(targets, scripted),
        targets,
        BUDGET,
        MAX_TARGETS,
        at_once,
    )
    .expect("a valid concurrent run");

    assert_eq!(
        concurrent, serial,
        "the concurrent run disagreed with the serial one, which means a caller \
         can tell which function produced the answer"
    );
    concurrent
}

// --- the two runs agree ------------------------------------------------------

#[test]
fn one_target_produces_the_same_answer_both_ways() {
    let reports = both_ways_agree(&[t("a", 1)], vec![answer()], 4);
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, Status::Alive);
}

#[test]
fn a_whole_range_of_outcomes_agrees_both_ways() {
    // Every observable outcome in one run, so that a divergence anywhere in the
    // mapping shows up rather than needing a test per status.
    let targets = [
        t("alive", 1),
        t("refused", 2),
        t("silent", 3),
        t("broken", 4),
        t("alive-too", 5),
    ];
    let scripted = vec![
        answer(),
        refuse(),
        no_answer(),
        ProbeOutcome::Error("cannot resolve".to_string()),
        answer(),
    ];

    let reports = both_ways_agree(&targets, scripted, 3);

    let statuses: Vec<Status> = reports.iter().map(|r| r.status.clone()).collect();
    assert_eq!(
        statuses,
        vec![
            Status::Alive,
            Status::Refused,
            Status::Unreachable,
            Status::Unknown("cannot resolve".to_string()),
            Status::Alive,
        ]
    );
}

#[test]
fn the_order_is_the_order_of_the_question_at_any_worker_count() {
    // The single most important property, and the one concurrency is most likely
    // to break: an agent pairing answers back to its own list would attach each
    // answer to the wrong machine, silently. Tested across worker counts because
    // one worker and many are different code paths through the same loop.
    let targets: Vec<Target> = (1..=8).map(|i| t(&format!("host{i}"), i)).collect();
    // Deliberately not in the scripted order: the answer for each index differs,
    // so a shuffled result cannot coincide with the right one.
    let scripted: Vec<ProbeOutcome> = (1..=8)
        .map(|i| if i % 2 == 0 { answer() } else { refuse() })
        .collect();

    for at_once in [1, 2, 3, 8, 100] {
        let reports = both_ways_agree(&targets, scripted.clone(), at_once);
        let hosts: Vec<&str> = reports.iter().map(|r| r.target.host.as_str()).collect();
        assert_eq!(
            hosts,
            vec![
                "host1", "host2", "host3", "host4", "host5", "host6", "host7", "host8"
            ],
            "with {at_once} at once"
        );
        // ...and the answer stayed attached to its own target.
        for (index, report) in reports.iter().enumerate() {
            let expected = if (index + 1) % 2 == 0 {
                Status::Alive
            } else {
                Status::Refused
            };
            assert_eq!(
                report.status, expected,
                "at index {index} with {at_once} at once"
            );
        }
    }
}

#[test]
fn more_workers_than_targets_is_allowed() {
    // The natural off-by-one: asking for 100 workers for 3 targets. Refusing it
    // would be a rule about the machine, not about the work.
    let targets = [t("a", 1), t("b", 2), t("c", 3)];
    let reports = both_ways_agree(&targets, vec![answer(), answer(), answer()], 100);
    assert_eq!(reports.len(), 3);
}

#[test]
fn exactly_one_worker_agreees_with_the_serial_run() {
    // The boundary from the other side: a single worker is the concurrent code
    // doing the serial thing, and it must not differ.
    let targets = [t("a", 1), t("b", 2)];
    both_ways_agree(&targets, vec![refuse(), answer()], 1);
}

// --- every target is asked about once ----------------------------------------

#[test]
fn every_target_is_probed_exactly_once_at_any_worker_count() {
    // A worker loop that takes the next index can hand one out twice if the
    // counter is wrong, and the result would still be the right length. This is
    // the test that would not notice a duplicate by length alone.
    for at_once in [1, 2, 4, 16] {
        for count in [1usize, 2, 5, 17] {
            let targets: Vec<Target> = (0..count).map(|i| t(&format!("h{i}"), 80)).collect();
            let scripted: Vec<ProbeOutcome> = (0..count).map(|_| answer()).collect();
            let probe = FakeProbe::new(&targets, scripted);

            let reports = check_targets_concurrent(&probe, &targets, BUDGET, MAX_TARGETS, at_once)
                .expect("a valid run");

            assert_eq!(reports.len(), count, "at_once={at_once} count={count}");
            assert_eq!(
                probe.calls.load(Ordering::SeqCst),
                count,
                "at_once={at_once} count={count}: a target was asked about more than once, \
                 or one was skipped"
            );

            let mut asked = probe.asked.lock().expect("no panic while holding").clone();
            asked.sort();
            let mut expected: Vec<String> = targets.iter().map(|t| t.to_string()).collect();
            expected.sort();
            assert_eq!(asked, expected, "at_once={at_once} count={count}");
        }
    }
}

// --- what is refused before any thread exists --------------------------------

#[test]
fn the_refusals_are_the_same_as_the_serial_run() {
    // Same three, decided in the same shared function, so this compares the two
    // entry points rather than restating the rules -- if `validate_run` were
    // bypassed by one of them, this fails.
    let probe = FakeProbe::new(&[t("a", 1), t("b", 2)], vec![answer()]);
    let two = [t("a", 1), t("b", 2)];

    assert_eq!(
        check_targets_concurrent(&probe, &[], BUDGET, MAX_TARGETS, 4),
        check_targets(&probe, &[], BUDGET, MAX_TARGETS)
    );
    assert_eq!(
        check_targets_concurrent(&probe, &two, BUDGET, 1, 4),
        check_targets(&probe, &two, BUDGET, 1)
    );
    assert_eq!(
        check_targets_concurrent(&probe, &two, BUDGET, 0, 4),
        check_targets(&probe, &two, BUDGET, 0)
    );
    assert_eq!(
        probe.calls.load(Ordering::SeqCst),
        0,
        "a refused run probes nothing"
    );
}

#[test]
fn zero_at_once_is_refused_rather_than_hanging() {
    // The failure mode this prevents is a run that never finishes, which is worse
    // than an error: nothing to report, nothing to retry, and no exit.
    let probe = FakeProbe::new(&[t("a", 1)], vec![answer()]);
    let result = check_targets_concurrent(&probe, &[t("a", 1)], BUDGET, MAX_TARGETS, 0);

    assert_eq!(result, Err(CheckError::ZeroAtOnce));
    assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_two_limits_are_told_apart() {
    // `max_targets` bounds how much work is asked for, `at_once` how much of the
    // machine is spent doing it. A caller that confused them would have one of
    // the two silently ignored, so the errors say which one failed.
    assert_eq!(
        CheckError::ZeroLimit.to_string(),
        "a limit of zero targets would refuse every run"
    );
    assert_eq!(
        CheckError::ZeroAtOnce.to_string(),
        "checking zero targets at once would never finish"
    );
    assert_ne!(CheckError::ZeroLimit, CheckError::ZeroAtOnce);
}

// --- the same budget, per target ---------------------------------------------

#[test]
fn the_budget_reaches_every_worker_unchanged() {
    // The budget is per target, not per run, and concurrency must not quietly
    // turn it into a total. A probe that recorded the budget it was handed would
    // be the direct test; this is the indirect one, and it is here because the
    // direct one needs a probe that shares state across threads.
    struct Recording {
        budgets: std::sync::Mutex<Vec<Duration>>,
    }
    impl Probe for Recording {
        fn probe(&self, _target: &Target, budget: Duration) -> ProbeOutcome {
            self.budgets
                .lock()
                .expect("no panic while holding")
                .push(budget);
            ProbeOutcome::Answered
        }
    }

    let probe = Recording {
        budgets: std::sync::Mutex::new(Vec::new()),
    };
    let targets: Vec<Target> = (0..8).map(|i| t(&format!("h{i}"), 80)).collect();
    let _ =
        check_targets_concurrent(&probe, &targets, BUDGET, MAX_TARGETS, 4).expect("a valid run");

    let budgets = probe.budgets.lock().expect("no panic while holding");
    assert_eq!(budgets.len(), 8);
    assert!(
        budgets.iter().all(|b| *b == BUDGET),
        "every worker should have been handed the same budget"
    );
}
