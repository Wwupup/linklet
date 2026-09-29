//! The specification for `check_targets`, and the demonstration of why the
//! trait lives in the core.
//!
//! Everything in this file runs with no network, no socket, and no waiting.
//! `FakeProbe` is the whole of the outside world as far as these tests are
//! concerned, and it is a dozen lines.
//!
//! That is the payoff of dependency inversion, and it is worth being concrete
//! about it: **these tests cover every status the tool can report, including
//! timeouts, without anything timing out.** A version of this code that opened
//! its own connections could only be tested by finding a machine that is off,
//! one that refuses, and one that drops packets -- and the suite would then take
//! ten seconds per case and fail on a laptop without a network.

use std::cell::RefCell;
use std::time::Duration;

use linklet_core::{
    CheckError, DEFAULT_BUDGET_SECONDS, Host, MAX_TARGETS, Port, Probe, ProbeOutcome, Status,
    Summary, Target, check_targets,
};

/// A probe with no machine behind it: scripted answers, and a record of what it
/// was asked.
///
/// The record matters more than it looks. Without it a test can only say what
/// the core concluded; with it, a test can say that the core asked *once* per
/// target, in order, with the budget it was given. Most of the bugs this kind
/// of function has are in the asking, not in the concluding.
#[derive(Default)]
struct FakeProbe {
    /// One entry per target, in the order the answers are handed out.
    scripted: Vec<ProbeOutcome>,
    /// Every call made: the target and the budget it arrived with.
    calls: RefCell<Vec<(Target, Duration)>>,
}

impl FakeProbe {
    fn new(scripted: Vec<ProbeOutcome>) -> Self {
        Self {
            scripted,
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Probe for FakeProbe {
    fn probe(&self, target: &Target, budget: Duration) -> ProbeOutcome {
        let mut calls = self.calls.borrow_mut();
        let index = calls.len();
        calls.push((target.clone(), budget));

        self.scripted.get(index).cloned().unwrap_or_else(|| {
            // A fake that panics on an unscripted call hides the failure inside
            // the test harness. An outcome the assertions can see keeps the
            // failure where the test can describe it.
            ProbeOutcome::Error(format!("not scripted: call {index} for {target}"))
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

fn time_out() -> ProbeOutcome {
    ProbeOutcome::TimedOut
}

fn no_answer() -> ProbeOutcome {
    ProbeOutcome::NoAnswer
}

const BUDGET: Duration = Duration::from_secs(2);

// --- each outcome becomes one status -----------------------------------------

#[test]
fn an_answered_probe_is_alive() {
    let probe = FakeProbe::new(vec![answer()]);
    let reports =
        check_targets(&probe, &[t("a", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, Status::Alive);
    assert!(reports[0].is_alive());
    assert_eq!(reports[0].reason, "connected");
}

#[test]
fn a_refused_connection_is_not_unreachable() {
    // The single most valuable distinction the tool makes. A `bool` would fold
    // this into "no", and "the machine is up but nothing is listening" sends the
    // caller somewhere completely different from "the machine did not answer".
    let probe = FakeProbe::new(vec![refuse()]);
    let reports =
        check_targets(&probe, &[t("a", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    assert_eq!(reports[0].status, Status::Refused);
    assert!(!reports[0].is_alive());
}

#[test]
fn no_answer_is_unreachable_and_names_the_budget_it_waited() {
    let probe = FakeProbe::new(vec![no_answer()]);
    let reports =
        check_targets(&probe, &[t("a", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    assert_eq!(reports[0].status, Status::Unreachable);
    // The reason is built from the budget, so it cannot disagree with the
    // budget that was actually used.
    assert_eq!(reports[0].reason, "no answer within 2 s");
}

#[test]
fn a_reported_timeout_is_unreachable_too_but_says_something_different() {
    // Two different outcomes, one status. The caller gets the same news --
    // "we could not talk to it" -- but not the same sentence, because the two
    // are different claims: one is a verdict the OS reached, the other is the
    // absence of one. Collapsing them into one outcome would lose that, and
    // collapsing them into one *message* would make the tool say "it timed out"
    // about a probe that never heard anything.
    let probe = FakeProbe::new(vec![time_out()]);
    let reports =
        check_targets(&probe, &[t("a", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    assert_eq!(reports[0].status, Status::Unreachable);
    assert_ne!(reports[0].reason, "no answer within 2 s");
    assert_eq!(reports[0].reason, "the machine did not answer in time");
}

#[test]
fn a_probe_error_is_unknown_and_keeps_the_adapters_words() {
    let probe = FakeProbe::new(vec![ProbeOutcome::Error(
        "name or service not known".to_string(),
    )]);
    let reports =
        check_targets(&probe, &[t("nope", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    assert_eq!(
        reports[0].status,
        Status::Unknown("name or service not known".to_string())
    );
    assert_eq!(reports[0].reason, "name or service not known");
}

// --- the run as a whole ------------------------------------------------------

#[test]
fn the_answer_is_in_the_order_of_the_question() {
    // Three different statuses, deliberately not sorted by status. A caller
    // that has to pair results back up with its own input has been handed a
    // worse interface than one that gets them in order.
    let probe = FakeProbe::new(vec![refuse(), answer(), time_out()]);
    let targets = [t("a", 1), t("b", 2), t("c", 3)];
    let reports =
        check_targets(&probe, &targets, BUDGET, MAX_TARGETS).expect("three targets is a run");

    let statuses: Vec<Status> = reports.iter().map(|r| r.status.clone()).collect();
    assert_eq!(
        statuses,
        vec![Status::Refused, Status::Alive, Status::Unreachable]
    );
    let hosts: Vec<&str> = reports.iter().map(|r| r.target.host.as_str()).collect();
    assert_eq!(hosts, vec!["a", "b", "c"]);
}

#[test]
fn every_target_is_probed_exactly_once() {
    let probe = FakeProbe::new(vec![answer(), answer()]);
    let targets = [t("a", 1), t("b", 2)];
    let _ = check_targets(&probe, &targets, BUDGET, MAX_TARGETS).expect("two targets is a run");

    let calls = probe.calls.borrow();
    assert_eq!(calls.len(), 2, "each target is looked at once, not twice");
    assert_eq!(calls[0].0, targets[0]);
    assert_eq!(calls[1].0, targets[1]);
}

#[test]
fn the_core_passes_the_budget_through_untouched() {
    // This is the test that keeps the core pure, expressed as behaviour.
    //
    // If the core ever measured time itself -- `Instant::now()` before and
    // after, subtracting to find what was left -- this test fails, because the
    // budget would arrive at the probe with something taken off it. That is
    // exactly the change that would put a clock in a crate that is supposed to
    // have no connection to the machine it runs on.
    let probe = FakeProbe::new(vec![answer()]);
    let _ = check_targets(&probe, &[t("a", 80)], BUDGET, MAX_TARGETS).expect("one target is a run");

    let calls = probe.calls.borrow();
    assert_eq!(calls[0].1, BUDGET);
}

#[test]
fn one_target_is_probed_per_scripted_answer() {
    let probe = FakeProbe::new(vec![answer()]);
    let targets = [t("a", 1), t("b", 2)];
    let reports =
        check_targets(&probe, &targets, BUDGET, MAX_TARGETS).expect("two targets is a run");

    // The fake reports "not scripted" for the second call rather than inventing
    // a success. A core that fired the probes out of order would show up here.
    assert_eq!(reports[0].status, Status::Alive);
    assert!(matches!(reports[1].status, Status::Unknown(_)));
}

// --- summaries ---------------------------------------------------------------

#[test]
fn the_summary_counts_each_status() {
    let probe = FakeProbe::new(vec![
        answer(),
        answer(),
        refuse(),
        time_out(),
        ProbeOutcome::Error("x".into()),
    ]);
    let targets = [t("a", 1), t("b", 2), t("c", 3), t("d", 4), t("e", 5)];
    let reports =
        check_targets(&probe, &targets, BUDGET, MAX_TARGETS).expect("five targets is a run");

    let summary = Summary::of(&reports);
    assert_eq!(
        summary,
        Summary {
            alive: 2,
            refused: 1,
            unreachable: 1,
            unknown: 1,
        }
    );
    assert_eq!(summary.total(), 5);
    assert_eq!(summary.total(), reports.len());
}

#[test]
fn an_empty_summary_is_all_zeroes() {
    assert_eq!(Summary::of(&[]), Summary::default());
    assert_eq!(Summary::of(&[]).total(), 0);
}

// --- what is refused before anything is probed --------------------------------

#[test]
fn an_empty_target_list_is_refused() {
    let probe = FakeProbe::new(vec![answer()]);
    assert_eq!(
        check_targets(&probe, &[], BUDGET, MAX_TARGETS),
        Err(CheckError::NoTargets)
    );
    assert!(
        probe.calls.borrow().is_empty(),
        "a refused run must not have probed anything"
    );
}

#[test]
fn more_targets_than_the_limit_is_refused_and_the_numbers_are_reported() {
    let probe = FakeProbe::new(vec![answer(); 3]);
    let targets = [t("a", 1), t("b", 2), t("c", 3)];

    assert_eq!(
        check_targets(&probe, &targets, BUDGET, 2),
        Err(CheckError::TooManyTargets { asked: 3, limit: 2 })
    );
    assert!(
        probe.calls.borrow().is_empty(),
        "the run is refused before any target is looked at, not halfway through"
    );
}

#[test]
fn exactly_the_limit_is_allowed() {
    // The boundary, from the allowed side: an off-by-one that rejects the limit
    // itself would pass every test above.
    let probe = FakeProbe::new(vec![answer(), answer()]);
    let targets = [t("a", 1), t("b", 2)];
    let reports = check_targets(&probe, &targets, BUDGET, 2).expect("exactly the limit is a run");
    assert_eq!(reports.len(), 2);
}

#[test]
fn a_limit_of_zero_is_refused_rather_than_refusing_every_run() {
    let probe = FakeProbe::new(vec![]);
    assert_eq!(
        check_targets(&probe, &[t("a", 1)], BUDGET, 0),
        Err(CheckError::ZeroLimit)
    );
    assert!(probe.calls.borrow().is_empty());
}

// --- the limits themselves ---------------------------------------------------

#[test]
fn the_limits_are_what_they_claim_to_be() {
    // Constants that feed a refusal are part of the interface: a caller reads
    // them to decide whether to batch its work. Changing one is a decision, so
    // it should require changing a test and reading why.
    assert_eq!(MAX_TARGETS, 256);
    assert_eq!(DEFAULT_BUDGET_SECONDS, 5);
    // The default must stay above the floor an adapter may impose on itself
    // (five seconds in `linklet-adapters`). That relation cannot be asserted
    // here -- this crate does not depend on that one, by rule 1 -- so it is
    // written down instead of checked, and this comment is where the next person
    // finds out why the two numbers are related at all.
}

#[test]
fn each_error_explains_itself_for_a_human() {
    assert_eq!(CheckError::NoTargets.to_string(), "no targets to check");
    assert_eq!(
        CheckError::TooManyTargets {
            asked: 900,
            limit: 256
        }
        .to_string(),
        "900 targets asked for, at most 256 are checked in one run"
    );
    assert_eq!(
        CheckError::ZeroLimit.to_string(),
        "a limit of zero targets would refuse every run"
    );
}
