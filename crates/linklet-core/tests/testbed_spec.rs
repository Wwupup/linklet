//! The specification for testbed requirements.
//!
//! The claim these tests support is the one the whole design rests on: **whether
//! a machine is ready to be tested is a decision, and a decision can be checked
//! without a machine.** A virtual machine, a spare PC and an already-configured
//! target are all judged by the same code below, which is why an agent can
//! direct a test on a machine it did not build.

use std::cell::RefCell;
use std::collections::BTreeMap;

use linklet_core::testbed::{
    Observation, PathKind, Prober, Requirement, SpecError, Testbed, render,
};

/// A machine made of a table, so a scenario is data rather than a setup script.
#[derive(Default)]
struct FakeMachine {
    /// What the prober answers, keyed by the requirement it was asked about.
    answers: BTreeMap<String, Observation>,
    /// Every requirement it was asked, in order.
    asked: RefCell<Vec<String>>,
    /// What it answers for a requirement with no entry.
    fallback: Option<Observation>,
}

impl FakeMachine {
    fn new(answers: Vec<(&str, Observation)>) -> Self {
        Self {
            answers: answers
                .into_iter()
                .map(|(key, value)| (key.to_string(), value))
                .collect(),
            asked: RefCell::new(Vec::new()),
            fallback: None,
        }
    }

    fn with_fallback(mut self, value: Observation) -> Self {
        self.fallback = Some(value);
        self
    }

    fn key(requirement: &Requirement) -> String {
        linklet_core::testbed::describe(requirement)
    }
}

impl Prober for FakeMachine {
    fn observe(&self, requirement: &Requirement) -> Observation {
        let key = Self::key(requirement);
        self.asked.borrow_mut().push(key.clone());
        self.answers
            .get(&key)
            .cloned()
            .or_else(|| self.fallback.clone())
            .unwrap_or_else(|| {
                // A fake that panics hides the failure in the harness; an
                // observation the assertions can see keeps it in the test.
                Observation::Unknown(format!("not scripted: {key}"))
            })
    }
}

/// Parses and expects success.
fn ok(text: &str) -> Testbed {
    Testbed::parse(text).unwrap_or_else(|e| panic!("should parse: {e}\n{text}"))
}

/// Parses and expects failure.
fn err(text: &str) -> SpecError {
    match Testbed::parse(text) {
        Ok(testbed) => panic!("should not parse, produced {testbed:?}"),
        Err(error) => error,
    }
}

// --- the format --------------------------------------------------------------

#[test]
fn a_full_specification_parses_into_its_parts() {
    let testbed = ok("\
        # what a machine must look like before the launch test means anything\n\
        name launch-smoke\n\
        require reachable 127.0.0.1:8765\n\
        require artifact dist\\app.exe present\n\
        require artifact dist\\out dir\n\
        forbid  artifact dist\\app.exe.part present\n\
        require no-process app.exe\n");

    assert_eq!(testbed.name, "launch-smoke");
    assert_eq!(
        testbed.requirements,
        vec![
            Requirement::Reachable("127.0.0.1:8765".to_string()),
            Requirement::Present {
                path: "dist\\app.exe".to_string(),
                kind: PathKind::File
            },
            Requirement::Present {
                path: "dist\\out".to_string(),
                kind: PathKind::Dir
            },
            Requirement::Absent {
                path: "dist\\app.exe.part".to_string()
            },
            Requirement::NoProcess("app.exe".to_string()),
        ]
    );
}

#[test]
fn blank_lines_and_comments_are_ignored_wherever_they_are() {
    let testbed = ok("\n# leading comment\n\nrequire reachable h:1\n\n   # indented\nname t\n");
    assert_eq!(testbed.requirements.len(), 1);
    assert_eq!(testbed.name, "t");
}

#[test]
fn a_name_may_contain_spaces() {
    // The rest of the line, not one word: an agent writing a descriptive name
    // should not have to remove the spaces to make it parse.
    assert_eq!(
        ok("name two words\nrequire reachable h:1\n").name,
        "two words"
    );
}

#[test]
fn an_unknown_directive_is_refused_with_its_line_number() {
    // A specification that silently ignores a line it did not understand checks
    // less than its author believes, and the author is an agent that will not
    // notice.
    let error = err("name t\nrequiers reachable h:1\n");
    assert_eq!(error.line, 2);
    assert!(error.message.contains("requiers"), "{error}");
    assert!(error.to_string().starts_with("line 2:"), "{error}");
}

#[test]
fn a_specification_with_no_name_or_no_requirements_is_refused() {
    assert!(err("require reachable h:1\n").message.contains("no name"));
    assert!(err("name t\n").message.contains("requires nothing"));
}

#[test]
fn a_requirement_missing_its_parts_names_what_is_missing() {
    assert!(
        err("name t\nrequire reachable\n")
            .message
            .contains("host:port")
    );
    assert!(err("name t\nrequire artifact\n").message.contains("path"));
    assert!(
        err("name t\nrequire artifact x\n")
            .message
            .contains("present or dir")
    );
    assert!(
        err("name t\nrequire no-process\n")
            .message
            .contains("process name")
    );
    assert!(
        err("name t\nrequire artifact x somewhere\n")
            .message
            .contains("expected present or dir")
    );
}

#[test]
fn the_two_directives_that_make_no_sense_are_refused_with_a_reason() {
    // Not pedantry: both are expressible, and a reader who writes one has a
    // different idea of what a testbed is than this module does. Saying so is
    // cheaper than either silently ignoring it or failing at check time.
    assert!(
        err("name t\nforbid reachable h:1\n")
            .message
            .contains("must not answer")
    );
    assert!(
        err("name t\nforbid no-process app.exe\n")
            .message
            .contains("must be running")
    );
}

// --- judging facts -----------------------------------------------------------

#[test]
fn a_reachable_target_that_answers_holds() {
    let testbed = ok("name t\nrequire reachable h:1\n");
    let machine = FakeMachine::new(vec![("reachable h:1", Observation::Answered)]);
    let verdicts = testbed.check(&machine);

    assert_eq!(verdicts.len(), 1);
    assert!(verdicts[0].held);
    assert_eq!(verdicts[0].detail, "h:1 answered");
}

#[test]
fn refused_and_no_answer_fail_with_different_words() {
    // The distinction the whole probe design exists for, carried through to what
    // the agent reads: a refusal means the machine is up and a firewall or a
    // missing process is the question, and no answer means something else
    // entirely.
    let testbed = ok("name t\nrequire reachable h:1\n");

    let refused = testbed.check(&FakeMachine::new(vec![(
        "reachable h:1",
        Observation::Refused,
    )]));
    let silent = testbed.check(&FakeMachine::new(vec![(
        "reachable h:1",
        Observation::NoAnswer,
    )]));

    assert!(!refused[0].held);
    assert!(refused[0].detail.contains("nothing is listening"));
    assert!(!silent[0].held);
    assert!(silent[0].detail.contains("did not answer"));
    assert_ne!(refused[0].detail, silent[0].detail);
}

#[test]
fn a_present_artifact_of_the_wrong_kind_names_both_kinds() {
    let testbed = ok("name t\nrequire artifact out dir\n");
    let machine = FakeMachine::new(vec![(
        "artifact out is a directory",
        Observation::WrongKind {
            found: PathKind::File,
        },
    )]);
    let verdicts = testbed.check(&machine);

    assert!(!verdicts[0].held);
    assert!(
        verdicts[0].detail.contains("is a file"),
        "{}",
        verdicts[0].detail
    );
    assert!(
        verdicts[0].detail.contains("a directory was required"),
        "{}",
        verdicts[0].detail
    );
}

#[test]
fn a_leftover_artifact_is_the_failure_this_exists_for() {
    // The check a hand-prepared machine fails, and the reason a testbed needs a
    // specification rather than a person saying "it should be fine". A `.part`
    // file from an interrupted copy changes what a launch test means.
    let testbed = ok("name t\nforbid artifact dist\\app.exe.part present\n");
    let machine = FakeMachine::new(vec![(
        "artifact dist\\app.exe.part is gone",
        Observation::Exists,
    )]);
    let verdicts = testbed.check(&machine);

    assert!(!verdicts[0].held);
    assert!(
        verdicts[0].detail.contains("still there"),
        "{}",
        verdicts[0].detail
    );
    assert!(
        verdicts[0].detail.contains("the test assumes it is not"),
        "{}",
        verdicts[0].detail
    );
}

#[test]
fn a_forbidden_path_that_is_missing_holds() {
    let testbed = ok("name t\nforbid artifact x present\n");
    let machine = FakeMachine::new(vec![("artifact x is gone", Observation::Missing)]);
    assert!(testbed.check(&machine)[0].held);
}

#[test]
fn a_process_that_is_still_running_is_named_in_the_failure() {
    let testbed = ok("name t\nrequire no-process app.exe\n");
    let machine = FakeMachine::new(vec![(
        "no-process app.exe",
        Observation::Processes(vec!["app.exe (pid 42)".to_string()]),
    )]);
    let verdicts = testbed.check(&machine);

    assert!(!verdicts[0].held);
    assert!(
        verdicts[0].detail.contains("pid 42"),
        "{}",
        verdicts[0].detail
    );
}

#[test]
fn an_observation_that_does_not_fit_the_requirement_is_reported_as_not_held() {
    // A prober bug, not a machine fault. It is reported as not-held -- a
    // requirement nobody could check is not one that held -- and the detail says
    // it is the checker's own confusion, so the agent does not go looking at the
    // machine.
    let testbed = ok("name t\nrequire reachable h:1\n");
    let machine = FakeMachine::new(vec![("reachable h:1", Observation::Exists)]);
    let verdicts = testbed.check(&machine);

    assert!(!verdicts[0].held);
    assert!(
        verdicts[0].detail.contains("does not fit"),
        "{}",
        verdicts[0].detail
    );
}

// --- the properties a caller depends on --------------------------------------

#[test]
fn every_requirement_is_checked_even_after_one_fails() {
    // Stopping at the first failure would make an agent fix one thing and run
    // again, which for three broken requirements is three round trips. Round
    // trips are what this project exists to reduce.
    let testbed = ok("\
        name t\n\
        require reachable a:1\n\
        require artifact b present\n\
        require no-process c\n");
    let machine = FakeMachine::new(vec![]).with_fallback(Observation::Missing);
    let verdicts = testbed.check(&machine);

    assert_eq!(verdicts.len(), 3, "all three were checked");
    assert_eq!(
        machine.asked.borrow().len(),
        3,
        "and all three were asked about"
    );
    assert!(verdicts.iter().all(|verdict| !verdict.held));
}

#[test]
fn an_unknown_observation_is_a_failure_and_not_a_pass() {
    // The tempting shortcut is to treat "could not tell" as fine, which would
    // make a prober that cannot see the machine report every testbed as ready.
    let testbed = ok("name t\nrequire reachable h:1\n");
    let machine = FakeMachine::new(vec![(
        "reachable h:1",
        Observation::Unknown("no permission".to_string()),
    )]);
    assert!(!testbed.check(&machine)[0].held);
}

#[test]
fn the_verdicts_are_in_the_order_of_the_specification() {
    let testbed =
        ok("name t\nrequire reachable a:1\nrequire reachable b:2\nrequire reachable c:3\n");
    let machine = FakeMachine::new(vec![]).with_fallback(Observation::Answered);
    let verdicts = testbed.check(&machine);

    let asked = machine.asked.borrow();
    assert_eq!(
        asked.as_slice(),
        &[
            "reachable a:1".to_string(),
            "reachable b:2".to_string(),
            "reachable c:3".to_string()
        ]
    );
    assert_eq!(verdicts.len(), 3);
}

// --- what the agent reads ----------------------------------------------------

#[test]
fn the_report_names_the_testbed_the_target_and_every_requirement() {
    let testbed =
        ok("name launch-smoke\nrequire reachable 127.0.0.1:8765\nforbid artifact tmp\\x present\n");
    let machine = FakeMachine::new(vec![
        ("reachable 127.0.0.1:8765", Observation::Answered),
        ("artifact tmp\\x is gone", Observation::Exists),
    ]);
    let text = render(&testbed, &testbed.check(&machine), "10.0.0.5");

    assert_eq!(
        text,
        "testbed launch-smoke  target 10.0.0.5\n\
         PASS reachable 127.0.0.1:8765: 127.0.0.1:8765 answered\n\
         FAIL artifact tmp\\x is gone: tmp\\x is still there, and the test assumes it is not\n\
         NOT READY 1 of 2 requirements met"
    );
}

#[test]
fn a_ready_machine_says_ready() {
    let testbed = ok("name t\nrequire reachable h:1\n");
    let machine = FakeMachine::new(vec![("reachable h:1", Observation::Answered)]);
    let text = render(&testbed, &testbed.check(&machine), "h");

    assert!(text.ends_with("READY 1 of 1 requirements met"), "{text}");
    assert!(!text.contains("NOT READY"));
}

#[test]
fn every_line_starts_with_a_word_a_parser_can_branch_on() {
    // The property an agent's parser depends on: the first word of a verdict
    // line is PASS or FAIL, and the last line starts with READY or NOT READY.
    let testbed = ok("name t\nrequire reachable a:1\nforbid artifact b present\n");
    let machine = FakeMachine::new(vec![("reachable a:1", Observation::Answered)]);
    let text = render(&testbed, &testbed.check(&machine), "h");

    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("testbed "));
    assert!(lines[1].starts_with("PASS "));
    assert!(lines[2].starts_with("FAIL "));
    assert!(lines[3].starts_with("NOT READY "));
    assert_eq!(
        lines.len(),
        4,
        "one header, one line per requirement, one verdict"
    );
}

#[test]
fn the_report_is_ascii() {
    // Rule 7, applied to the thing an agent reads. A stray non-ASCII character
    // in a protocol-adjacent output is the class of bug that shows up as mangled
    // text on a machine with a different code page.
    let testbed = ok("name t\nrequire reachable a:1\nrequire artifact b\\c present\n");
    let machine = FakeMachine::new(vec![]).with_fallback(Observation::Missing);
    let text = render(&testbed, &testbed.check(&machine), "10.0.0.5");
    assert!(text.is_ascii(), "{text}");
}
