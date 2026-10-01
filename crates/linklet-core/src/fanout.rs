//! One operation across several machines, and what comes back.
//!
//! This is the second half of `docs/ROADMAP.md` M10's discovery item, and it is the half the
//! item calls a decision rather than a loop: **what and how many results come back from a
//! many-target `exec`**. The concurrency itself is an adapter's job -- it opens sockets -- but
//! every question about the shape of the answer is here, where it is a table.
//!
//! # The four decisions this module makes
//!
//! 1. **Every target gets a line, in the order the caller asked for them.** Not the order they
//!    finished in: a fan-out over four machines that answers in a different order each run is
//!    one nobody can diff, and the caller wrote the list for a reason.
//! 2. **One machine failing does not stop the others.** A run that stopped at the first
//!    refusal would make "which of these ten are up" into a sequence of ten calls, which is
//!    what this exists to replace.
//! 3. **"The machine said no" and "I could not reach the machine" are different answers.**
//!    [`Fate::Refused`] and [`Fate::Unreachable`] send a reader to different places -- the
//!    build, or the network -- and a report that folded them together would be a report
//!    nobody can act on.
//! 4. **The report says how many were examined and whether the run was cut short.** The same
//!    lesson as `ps`, `ls` and discovery, for the fourth time in this milestone: a count of
//!    what was looked at, so that "nothing came back" cannot be read as "there is nothing".
//!
//! # What it does not do
//!
//! It does not decide what to do about a failure, and it does not retry. A retry turns one
//! request into two that ran -- the same argument `linklet-client` makes about itself -- and
//! "should a fan-out give up" is a policy a caller with a reason can apply to the report.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The most targets one fan-out will run over.
///
/// A ceiling for the same reason every other one in this project has one: a list of targets is
/// input, and input that decides how much work happens is input that can decide too much. At
/// the default concurrency this is a few seconds even when every machine is slow.
pub const MAX_TARGETS: usize = 256;

/// The most targets one fan-out works on at once.
///
/// Four, because each one is a socket conversation and a fan-out is a convenience over
/// calling each in turn. A hundred at once would make one run the whole of a small tool's
/// traffic, and the difference between four and a hundred is not a difference anybody asked
/// for.
pub const AT_ONCE: usize = 4;

/// How one target's turn ended.
///
/// The distinction this enum exists for is between the machine and the call: a program that
/// ran and exited 1 is a fact about the build, and an agent that could not be reached is a
/// fact about the network. Folding them into "failed" is what makes an agent retry a machine
/// that already answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// The operation ran to completion on this target.
    Ran,
    /// The target answered and declined: it understood the request and said no.
    Refused,
    /// The target could not be reached, or answered with something unreadable.
    Unreachable,
    /// The work itself did not finish: it panicked, which no caller should ever see.
    ///
    /// **A panic is a result and not a crash.** A fan-out is the one place in this project
    /// where several pieces of work happen at once and one of them could take the process
    /// down; catching it here means the other targets still answer, and the report says which
    /// one went wrong instead of the caller losing all of them.
    Panicked,
}

impl Fate {
    /// Whether this target needs looking at again.
    pub fn is_failure(self) -> bool {
        !matches!(self, Self::Ran)
    }

    /// How a reader says it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::Refused => "refused",
            Self::Unreachable => "unreachable",
            Self::Panicked => "panicked",
        }
    }
}

/// One thing to do, and what to call it in the report.
#[derive(Debug)]
pub struct Task {
    /// The target, as the caller wrote it.
    pub target: String,
    /// What to do with it.
    pub work: Work,
}

/// The work one target's turn consists of.
///
/// A newtype around the closure rather than the bare `Box<dyn FnOnce()>`, and not for tidiness:
/// a `Task` has to be **`Debug`, movable and callable exactly once**. The box is already
/// movable; a boxed closure is not `Debug`, and a `Task` is held in a mutex that a panic is
/// caught around, so a panic message has to be able to print what it was holding. The `Option`
/// is what makes "exactly once" a property of the type: taking the work out leaves `None`, and
/// a fan-out that called one target's work twice would be a fan-out that did something twice on
/// somebody's machine.
pub struct Work(Option<Box<dyn FnOnce() -> Fate + Send>>);

impl std::fmt::Debug for Work {
    /// Names the closure rather than trying to print it, which is all a reader can use.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<work>")
    }
}

impl Work {
    /// Wraps one target's work.
    pub fn new(work: impl FnOnce() -> Fate + Send + 'static) -> Self {
        Self(Some(Box::new(work)))
    }

    /// Takes the work out, so it can be called exactly once.
    fn take(&mut self) -> Option<Box<dyn FnOnce() -> Fate + Send>> {
        self.0.take()
    }
}

impl Task {
    /// A task for one target.
    ///
    /// The work returns a [`Fate`] rather than a result of its own, because a fan-out does not
    /// have to read what came back -- the caller's operation already turned it into one of the
    /// four states, and a generic payload here would be a second way to spell the same answer.
    pub fn new(target: impl Into<String>, work: impl FnOnce() -> Fate + Send + 'static) -> Self {
        Self {
            target: target.into(),
            work: Work::new(work),
        }
    }
}

/// What happened to one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The target, echoed so a report line stands alone.
    pub target: String,
    /// How its turn ended.
    pub fate: Fate,
    /// Why, when there is a why.
    pub reason: Option<String>,
}

/// The answer to a fan-out.
///
/// `total` is the number of targets the caller asked for and `outcomes` is what happened to
/// each of them; they differ only when the run was cut short at [`MAX_TARGETS`], which is the
/// one case where a caller must not read the report as the whole story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// One outcome per target, in the order the targets were given.
    pub outcomes: Vec<Outcome>,
    /// How many targets were asked for.
    pub total: usize,
    /// Whether the list was cut short at [`MAX_TARGETS`].
    pub truncated: bool,
}

impl Report {
    /// How many targets ran to completion.
    pub fn ran(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.fate == Fate::Ran)
            .count()
    }

    /// The targets that need looking at again, in the order they were given.
    pub fn failures(&self) -> Vec<&Outcome> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.fate.is_failure())
            .collect()
    }
    /// Whether every target that was examined ran to completion.
    ///
    /// **`false` for a truncated run**, because a report over the first two hundred and fifty
    /// of three hundred targets has not said anything about the last fifty and must not be
    /// read as if it had.
    pub fn complete(&self) -> bool {
        !self.truncated && self.failures().is_empty()
    }

    /// Whether every target arrived and none of them needs looking at again.
    pub fn all_ran(&self) -> bool {
        !self.outcomes.is_empty() && self.failures().is_empty()
    }
}

/// Runs every task, at most [`AT_ONCE`] at a time, and reports what happened.
///
/// The work is the caller's and so is the number of targets; what this decides is the shape of
/// the answer -- one line per target, in the caller's order, with the machine's refusals told
/// apart from the network's.
///
/// A task that panics does not take the run with it: it comes back as [`Fate::Panicked`] and
/// the other targets are unaffected. That is the one form of failure this module can do
/// something about, and not doing it would make one bad operation enough to lose a whole
/// report.
pub fn run(tasks: Vec<Task>) -> Report {
    let total = tasks.len();
    let truncated = total > MAX_TARGETS;

    let mut tasks = tasks;
    tasks.truncate(MAX_TARGETS);

    let outcomes: Mutex<Vec<Option<Outcome>>> = Mutex::new(vec![None; tasks.len()]);
    let next = AtomicUsize::new(0);

    // `Arc` would be needed to hand the tasks to threads that outlive this call; a scope means
    // they do not, so the tasks can be borrowed and each one is taken by exactly the worker
    // that runs it.
    let tasks: Vec<Mutex<Option<Task>>> = tasks
        .into_iter()
        .map(|task| Mutex::new(Some(task)))
        .collect();

    std::thread::scope(|scope| {
        let workers = AT_ONCE.min(tasks.len().max(1));
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = tasks.get(index) else {
                        return;
                    };
                    // A task is taken out of its slot rather than borrowed from it, because
                    // `FnOnce` cannot be called through a borrow and because a slot that is
                    // empty is a slot no second worker can pick up.
                    let Some(task) = slot.lock().ok().and_then(|mut slot| slot.take()) else {
                        return;
                    };
                    let target = task.target.clone();
                    let mut work = task.work;

                    // The panic is caught and turned into an outcome. `AssertUnwindSafe`
                    // because the closure is the caller's and this cannot promise anything
                    // about what it left behind -- what it promises is that the *report* still
                    // arrives, which is the part the other targets depend on.
                    let fate = match work.take() {
                        Some(work) => match catch_unwind(AssertUnwindSafe(work)) {
                            Ok(fate) => fate,
                            Err(_) => Fate::Panicked,
                        },
                        // Unreachable: a slot is taken exactly once, by the worker that got its
                        // index. Said rather than assumed, because the alternative is a silent
                        // `Ran` for work that never happened.
                        None => Fate::Panicked,
                    };

                    let outcome = Outcome {
                        target,
                        fate,
                        reason: (fate == Fate::Panicked)
                            .then(|| "the operation panicked, which is a bug in it".to_string()),
                    };
                    if let Ok(mut outcomes) = outcomes.lock()
                        && let Some(slot) = outcomes.get_mut(index)
                    {
                        *slot = Some(outcome);
                    }
                }
            });
        }
    });

    let outcomes = outcomes
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();

    Report {
        outcomes,
        total,
        truncated,
    }
}

/// Renders a report as the lines a person or an agent reads.
///
/// **The summary comes first and the failures are named.** A reader's first question about a
/// fan-out is "did they all work", and their second is "which one did not" -- so the summary
/// answers the first and the lines answer the second, and neither is left to be counted by
/// eye. The same shape `check`, `ps`, `ls` and discovery all use, which is the point: one
/// reading of a summary in this tool, not five.
pub fn render(report: &Report) -> String {
    let mut out = format!("{} of {} ran", report.ran(), report.total);
    if report.truncated {
        out.push_str(&format!(", stopped at {MAX_TARGETS} targets"));
    }

    for outcome in &report.outcomes {
        match &outcome.reason {
            Some(reason) => out.push_str(&format!(
                "\n{} {}: {reason}",
                outcome.fate.as_str(),
                outcome.target
            )),
            None => out.push_str(&format!("\n{} {}", outcome.fate.as_str(), outcome.target)),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// A task that records that it ran and answers with a fixed fate.
    fn task(target: &str, fate: Fate, ran: Arc<AtomicUsize>) -> Task {
        Task::new(target, move || {
            ran.fetch_add(1, Ordering::Relaxed);
            fate
        })
    }

    #[test]
    fn every_target_gets_a_line_in_the_order_the_caller_gave_them() {
        // **The order is the caller's and not the finishing order.** A fan-out over four
        // machines that answers in a different order each run is one nobody can diff, and the
        // caller wrote the list for a reason.
        let ran = Arc::new(AtomicUsize::new(0));
        let tasks = vec![
            task("a:1", Fate::Ran, ran.clone()),
            task("b:1", Fate::Ran, ran.clone()),
            task("c:1", Fate::Ran, ran.clone()),
        ];

        let report = run(tasks);

        assert_eq!(ran.load(Ordering::Relaxed), 3, "every task ran");
        assert_eq!(
            report
                .outcomes
                .iter()
                .map(|outcome| outcome.target.as_str())
                .collect::<Vec<_>>(),
            ["a:1", "b:1", "c:1"]
        );
        assert!(report.complete());
        assert_eq!(render(&report), "3 of 3 ran\nran a:1\nran b:1\nran c:1");
    }

    #[test]
    fn a_machine_that_refuses_is_not_a_machine_that_cannot_be_reached() {
        // **The distinction the whole report is arranged around.** "The build failed" and "I
        // could not reach the box" send a reader to different places, and a report that folded
        // them together would send every reader to the same wrong one.
        let ran = Arc::new(AtomicUsize::new(0));
        let tasks = vec![
            Task::new("up:1", {
                let ran = ran.clone();
                move || {
                    ran.fetch_add(1, Ordering::Relaxed);
                    Fate::Ran
                }
            }),
            Task::new("no:1", || Fate::Refused),
            Task::new("gone:1", || Fate::Unreachable),
        ];

        let report = run(tasks);

        assert_eq!(report.ran(), 1);
        assert_eq!(report.failures().len(), 2);
        assert!(!report.complete());
        let rendered = render(&report);
        assert!(rendered.starts_with("1 of 3 ran"), "{rendered}");
        assert!(rendered.contains("refused no:1"), "{rendered}");
        assert!(rendered.contains("unreachable gone:1"), "{rendered}");
    }

    #[test]
    fn one_machine_failing_does_not_stop_the_others() {
        // A run that stopped at the first refusal would turn "which of these ten are up" into
        // ten calls, which is what a fan-out exists to replace.
        let ran = Arc::new(AtomicUsize::new(0));
        let tasks = vec![
            Task::new("first:1", || Fate::Unreachable),
            task("second:1", Fate::Ran, ran.clone()),
            task("third:1", Fate::Ran, ran.clone()),
        ];

        let report = run(tasks);

        assert_eq!(ran.load(Ordering::Relaxed), 2, "the others still ran");
        assert_eq!(report.ran(), 2);
        assert_eq!(report.total, 3);
    }

    #[test]
    fn a_task_that_panics_comes_back_as_a_result_and_the_others_still_answer() {
        // The one failure this module can do something about. A fan-out is the only place in
        // this project where several pieces of work happen at once, and losing a whole report
        // because one operation has a bug would be the worst possible trade.
        let ran = Arc::new(AtomicUsize::new(0));
        let tasks = vec![
            task("before:1", Fate::Ran, ran.clone()),
            Task::new("bad:1", || panic!("this operation has a bug")),
            task("after:1", Fate::Ran, ran.clone()),
        ];

        let report = run(tasks);

        assert_eq!(ran.load(Ordering::Relaxed), 2, "the other two still ran");
        assert_eq!(report.outcomes.len(), 3, "and all three are reported");
        assert_eq!(report.outcomes[1].fate, Fate::Panicked);
        assert!(
            report.outcomes[1]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("bug")),
            "the reason says the operation is at fault: {:#?}",
            report.outcomes[1]
        );
        assert!(!report.complete());
    }

    #[test]
    fn a_list_past_the_ceiling_is_cut_short_and_says_so() {
        // The count of what was examined, for the fourth time in this milestone. `complete()`
        // is false for a truncated run because a report over the first two hundred and fifty
        // of three hundred has said nothing about the last fifty.
        let ran = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<Task> = (0..MAX_TARGETS + 5)
            .map(|index| task(&format!("host-{index}:1"), Fate::Ran, ran.clone()))
            .collect();

        let report = run(tasks);

        assert_eq!(report.total, MAX_TARGETS + 5);
        assert_eq!(report.outcomes.len(), MAX_TARGETS);
        assert!(report.truncated);
        assert!(!report.complete(), "the last five were never looked at");
        assert!(
            render(&report).contains(&format!("stopped at {MAX_TARGETS} targets")),
            "the ceiling has to be visible in the first line"
        );
    }

    #[test]
    fn a_run_of_nothing_is_not_a_run_that_succeeded() {
        // "Every target ran" is true of no targets and it is not an answer -- `all_ran` is the
        // question a caller asks when it wants to know that something came back, and an empty
        // list has not answered it.
        let report = run(Vec::new());

        assert_eq!(report.total, 0);
        assert!(report.outcomes.is_empty());
        assert!(report.complete(), "nothing failed, because nothing ran");
        assert!(
            !report.all_ran(),
            "and that is not the same as everything ran"
        );
        assert_eq!(render(&report), "0 of 0 ran");
    }

    #[test]
    fn the_work_actually_happens_at_the_same_time() {
        // The claim "fan-out" makes. Four tasks that each wait for the others prove it: with
        // concurrency four they finish together, and with concurrency one they would deadlock
        // -- so the barrier is both the proof and the timeout.
        use std::sync::Barrier;

        let barrier = Arc::new(Barrier::new(AT_ONCE));
        let tasks: Vec<Task> = (0..AT_ONCE)
            .map(|index| {
                let barrier = barrier.clone();
                Task::new(format!("host-{index}:1"), move || {
                    // Waiting for the others can only succeed if they are all running.
                    barrier.wait();
                    Fate::Ran
                })
            })
            .collect();

        let report = run(tasks);

        assert_eq!(report.ran(), AT_ONCE, "{report:#?}");
    }
}
