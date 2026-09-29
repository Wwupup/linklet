//! The specification for the MCP tool surface.
//!
//! This file is the *deliverable* of milestone M5, more than the tools are. The
//! project this one grew out of reached seventeen tools and 13,758 characters of
//! description, and it got there one reasonable-looking addition at a time.
//! Nothing in it was ever wrong; it was simply never counted.
//!
//! So the tests here count. A tool added without deciding to add one fails, and
//! the failure message says where the decision belongs.

use std::cell::RefCell;
use std::time::Duration;

use linklet_core::json::Json;
use linklet_core::object;
use linklet_core::{
    MAX_DESCRIPTION_CHARS, ToolError, ToolOutcome, ToolRunner, dispatch, tool_list_json, tools,
    total_description_chars,
};

/// A stand-in for the machine: records what it was asked, returns fixed bodies.
///
/// Implements the same trait the binary does, so every test below exercises the
/// argument handling and the answering without a network or a filesystem. That
/// is the payoff of the trait: two capabilities, no closures to thread through
/// dispatch, and one stub that answers both.
#[derive(Default)]
struct FakeRun {
    reachability_calls: RefCell<Vec<(String, Duration)>>,
    testbed_calls: RefCell<Vec<(String, String)>>,
    exec_calls: RefCell<Vec<(String, String, u64)>>,
    transfer_calls: RefCell<Vec<(String, String, String)>>,
}

impl FakeRun {
    fn testbed_calls(&self) -> Vec<(String, String)> {
        self.testbed_calls.borrow().clone()
    }

    fn exec_calls(&self) -> Vec<(String, String, u64)> {
        self.exec_calls.borrow().clone()
    }

    /// Every transfer call, whichever direction, with the direction not recorded.
    ///
    /// Enough for the tests that check the arguments arrived: a fake that also recorded
    /// the direction would be asserting on a value dispatch does not pass it.
    fn transfer_calls(&self) -> Vec<(String, String, String)> {
        self.transfer_calls.borrow().clone()
    }
}

impl ToolRunner for FakeRun {
    fn reachability(&self, targets: &str, budget: Duration) -> ToolOutcome {
        self.reachability_calls
            .borrow_mut()
            .push((targets.to_string(), budget));
        ToolOutcome::ok("live 10.0.0.5:8787 connected\n1 of 1 live")
    }

    fn testbed(&self, spec_path: &str, target: &str) -> ToolOutcome {
        self.testbed_calls
            .borrow_mut()
            .push((spec_path.to_string(), target.to_string()));
        ToolOutcome::ok("testbed launch-smoke  target box\nREADY 1 of 1 requirements met")
    }

    fn exec(&self, agent: &str, command: &str, timeout_seconds: u64) -> ToolOutcome {
        self.exec_calls.borrow_mut().push((
            agent.to_string(),
            command.to_string(),
            timeout_seconds,
        ));
        ToolOutcome::ok("exit 0\ntook 12 ms\nstdout:\nhello")
    }

    fn push(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        self.transfer_calls.borrow_mut().push((
            agent.to_string(),
            from.to_string(),
            to.to_string(),
        ));
        ToolOutcome::ok("build.exe: 16 bytes, sha256 0123456789abcdef")
    }

    fn pull(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        self.transfer_calls.borrow_mut().push((
            agent.to_string(),
            from.to_string(),
            to.to_string(),
        ));
        ToolOutcome::ok("build.log: 16 bytes, sha256 0123456789abcdef")
    }
}

// --- the shape of the surface ------------------------------------------------

#[test]
fn there_are_exactly_five_tools() {
    // The count is the assertion. Growing this list is a decision, and the way
    // to make it is to change this number and say in the commit why the new tool
    // earns its place -- which is exactly the conversation that was never had
    // the last time.
    //
    // The fifth and sixth additions were `push` and `pull`, and the argument is in
    // `tools()`: an agent that cannot send a file cannot install a build, and one that
    // cannot bring a log back has to ask for it in a command's output instead.
    assert_eq!(
        tools().len(),
        5,
        "adding a tool is a decision: change this number and explain in the commit \
         why the new question needs its own tool rather than belonging to this one"
    );
    // Both names, so a tool cannot be swapped for another without this failing.
    // A count alone would not notice.
    let names: Vec<&str> = tools().iter().map(|tool| tool.name).collect();
    assert_eq!(names, vec!["check", "testbed", "exec", "push", "pull"]);
}

#[test]
fn no_description_refers_to_another_tool() {
    // Rule 2. When a description has to say "use X instead", the list has become
    // a graph the reader traverses and the manual has started to write itself.
    let all = tools();
    let names: Vec<&str> = all.iter().map(|tool| tool.name).collect();

    for tool in &all {
        for other in &names {
            if *other == tool.name {
                continue;
            }
            assert!(
                !tool.description.contains(other),
                "the description of {:?} mentions {other:?}; if the reader needs to \
                 know that, the two tools are one tool",
                tool.name
            );
        }
    }
}

#[test]
fn no_description_carries_a_caveat_or_a_manual() {
    // The words that never appear in a description that fits on one line. Their
    // presence is the symptom this whole milestone exists to prevent.
    for tool in tools() {
        for word in [
            "instead",
            "prefer",
            "or use",
            "note:",
            "however",
            "deprecated",
        ] {
            assert!(
                !tool.description.to_lowercase().contains(word),
                "{:?}'s description contains {word:?}, which is the manual writing itself: {}",
                tool.name,
                tool.description
            );
        }
    }
}

#[test]
fn every_description_fits_its_budget() {
    // The budget itself is asserted where it is declared, by the tests below
    // that compare against it: a budget that cannot be exceeded is not a budget.
    for tool in tools() {
        assert!(
            tool.description.len() <= MAX_DESCRIPTION_CHARS,
            "{:?}'s description is {} characters, over the {MAX_DESCRIPTION_CHARS} budget: {}",
            tool.name,
            tool.description.len(),
            tool.description
        );
    }
}

#[test]
fn the_whole_surface_is_a_tiny_fraction_of_the_one_it_replaces() {
    // The number being guarded, named so that it cannot be quietly forgotten.
    // The reference point had 13,758 characters for seventeen tools, which is
    // 809 characters each.
    const THE_SURFACE_THIS_REPLACES: usize = 13_758;
    let total = total_description_chars();

    assert!(
        total * 20 < THE_SURFACE_THIS_REPLACES,
        "the descriptions total {total} characters; the surface this replaces had \
         {THE_SURFACE_THIS_REPLACES} across seventeen tools"
    );
}

#[test]
fn the_list_and_the_dispatcher_agree_on_the_names() {
    // A tool in the list that dispatch does not know is a bug visible only in a
    // live session, where the agent has already chosen it.
    let listed: Vec<String> = tool_list_json()
        .as_array()
        .expect("tools/list is an array")
        .iter()
        .map(|entry| {
            entry
                .get_str("name")
                .expect("every entry has a name")
                .to_string()
        })
        .collect();

    let known: Vec<String> = tools().iter().map(|tool| tool.name.to_string()).collect();
    assert_eq!(listed, known);

    for name in &listed {
        let result = dispatch(name, &object! {}, &FakeRun::default());
        // Bad arguments rather than "no such tool", which is the distinction
        // being checked: the name is recognised.
        assert!(
            !matches!(result, Err(ToolError::NoSuchTool(_))),
            "{name:?} is listed but the dispatcher does not know it"
        );
    }
}

#[test]
fn each_entry_carries_the_three_fields_mcp_requires() {
    for entry in tool_list_json().as_array().expect("an array") {
        assert!(entry.get_str("name").is_some());
        assert!(entry.get_str("description").is_some());
        let schema = entry.get("inputSchema").expect("a schema");
        assert_eq!(schema.get_str("type"), Some("object"));
        assert!(schema.get("properties").is_some());
    }
}

#[test]
fn the_schema_rejects_arguments_the_tool_does_not_take() {
    // `additionalProperties: false` is a promise to the caller, so it is checked
    // rather than only written down.
    for entry in tool_list_json().as_array().expect("an array") {
        assert_eq!(
            entry
                .get("inputSchema")
                .and_then(|schema| schema.get("additionalProperties"))
                .and_then(Json::as_bool),
            Some(false),
            "a schema that allows extra arguments cannot promise anything"
        );
    }
}

// --- what a call does --------------------------------------------------------

#[test]
fn an_unknown_tool_is_refused_by_name() {
    let result = dispatch("nope", &object! {}, &FakeRun::default());
    assert_eq!(result, Err(ToolError::NoSuchTool("nope".to_string())));
    assert!(
        result
            .expect_err("an unknown tool is an error")
            .to_string()
            .contains("nope")
    );
}

#[test]
fn targets_are_joined_into_the_one_spec_the_parser_takes() {
    let fake = FakeRun::default();
    let arguments = object! { "targets" => vec![Json::str("a:1"), Json::str("b:2")] };

    let outcome = dispatch("check", &arguments, &fake).expect("a valid call");

    assert!(!outcome.is_error);
    let calls = fake.reachability_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "a:1,b:2");
}

#[test]
fn the_timeout_defaults_and_is_passed_through() {
    let fake = FakeRun::default();

    dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &fake,
    )
    .expect("a valid call");
    dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")], "timeout" => 9i64 },
        &fake,
    )
    .expect("a valid call");

    let calls = fake.reachability_calls.borrow();
    assert_eq!(calls[0].1, Duration::from_secs(5), "the default");
    assert_eq!(calls[1].1, Duration::from_secs(9), "the argument");
}

#[test]
fn a_timeout_out_of_range_is_refused_rather_than_clamped() {
    // A caller that asked for 10,000 seconds and silently got 10 has been lied
    // to about what happened.
    let fake = FakeRun::default();
    for value in [0i64, -1, 11, 100_000] {
        let result = dispatch(
            "check",
            &object! { "targets" => vec![Json::str("a:1")], "timeout" => value },
            &fake,
        );
        assert!(
            matches!(
                result,
                Err(ToolError::BadArgument {
                    name: "timeout",
                    ..
                })
            ),
            "{value} should be refused, got {result:?}"
        );
    }
    assert!(
        fake.reachability_calls.borrow().is_empty(),
        "a refused call must not reach the network"
    );
}

#[test]
fn missing_or_misshapen_arguments_are_named() {
    let fake = FakeRun::default();

    // The argument name is asserted as well as the problem: "something is wrong"
    // without saying which field is not actionable for an agent that has to
    // retry.
    let cases: Vec<(Json, &str, &str)> = vec![
        (object! {}, "targets", "missing"),
        (
            object! { "targets" => Json::str("a:1") },
            "targets",
            "expected an array",
        ),
        (
            object! { "targets" => vec![Json::Int(1)] },
            "targets",
            "rather than a string",
        ),
        (
            object! { "targets" => Vec::<Json>::new() },
            "targets",
            "the list is empty",
        ),
        (
            object! { "targets" => vec![Json::str("a:1")], "timeout" => Json::str("5") },
            "timeout",
            "expected an integer",
        ),
    ];

    for (arguments, expected_name, expected_problem) in cases {
        let result = dispatch("check", &arguments, &fake);
        match result {
            Err(ToolError::BadArgument { name, problem }) => {
                assert_eq!(name, expected_name, "for {arguments:?}");
                assert!(
                    problem.contains(expected_problem),
                    "for {arguments:?}, expected {expected_problem:?}, got {problem:?}"
                );
            }
            other => panic!("{arguments:?} should be a bad argument, got {other:?}"),
        }
    }
    assert!(fake.reachability_calls.borrow().is_empty());
}

#[test]
fn an_unknown_argument_is_refused_rather_than_ignored() {
    // The failure this prevents: the caller believes it asked for something, the
    // tool did not, and nothing in the reply says so.
    let fake = FakeRun::default();
    let result = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")], "timout" => 5i64 },
        &fake,
    );

    assert_eq!(
        result,
        Err(ToolError::UnknownArgument("timout".to_string()))
    );
    assert!(fake.reachability_calls.borrow().is_empty());
}

#[test]
fn bad_news_is_not_an_error() {
    // "Three machines are down" is a successful call. Marking it an error would
    // teach the agent to retry a tool that worked.
    //
    // This runner answers with bad news, which is the case the distinction is
    // about: the capability worked and the answer was unwelcome.
    struct Down;
    impl ToolRunner for Down {
        fn reachability(&self, _targets: &str, _budget: Duration) -> ToolOutcome {
            ToolOutcome::ok("dead a:1 nothing is listening on that port\n0 of 1 live")
        }
        fn testbed(&self, _spec_path: &str, _target: &str) -> ToolOutcome {
            ToolOutcome::ok("NOT READY 0 of 1 requirements met")
        }
        fn exec(&self, _agent: &str, _command: &str, _timeout_seconds: u64) -> ToolOutcome {
            ToolOutcome::ok("exit 1\\ntook 3 ms\\nstderr:\\nfailed")
        }
        fn push(&self, _agent: &str, _from: &str, _to: &str) -> ToolOutcome {
            ToolOutcome::failed("cannot read the local file")
        }
        fn pull(&self, _agent: &str, _from: &str, _to: &str) -> ToolOutcome {
            ToolOutcome::failed("the agent refused the request")
        }
    }

    let outcome = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &Down,
    )
    .expect("a valid call");

    assert!(!outcome.is_error, "bad news is not a failed call");
    assert!(outcome.text.contains("0 of 1 live"));
}

// --- the second tool ---------------------------------------------------------

#[test]
fn the_testbed_tool_passes_its_two_arguments_through() {
    let fake = FakeRun::default();
    let arguments =
        object! { "spec" => Json::str("specs/launch.testbed"), "target" => Json::str("box-a") };

    let outcome = dispatch("testbed", &arguments, &fake).expect("a valid call");

    assert!(!outcome.is_error);
    assert_eq!(
        fake.testbed_calls(),
        vec![("specs/launch.testbed".to_string(), "box-a".to_string())]
    );
    assert!(
        fake.reachability_calls.borrow().is_empty(),
        "calling one tool must not run the other"
    );
}

#[test]
fn a_spec_path_outside_the_working_tree_is_refused() {
    // An agent that can read any file on the machine has been handed more than
    // this tool is for. The refusal names the reason rather than failing later
    // with a path error from the filesystem.
    let fake = FakeRun::default();

    for path in [
        "C:\\Windows\\System32\\config\\SAM",
        "/etc/passwd",
        "\\\\server\\share\\spec.testbed",
        "..\\..\\secret.testbed",
        "specs/../../secret.testbed",
    ] {
        let arguments = object! { "spec" => Json::str(path), "target" => Json::str("box") };
        let result = dispatch("testbed", &arguments, &fake);

        assert!(
            matches!(result, Err(ToolError::BadArgument { name: "spec", .. })),
            "{path:?} should be refused, got {result:?}"
        );
    }

    assert!(
        fake.testbed_calls().is_empty(),
        "a refused path must not reach the filesystem"
    );
}

#[test]
fn a_relative_spec_path_is_accepted() {
    let fake = FakeRun::default();
    for path in [
        "specs/a.testbed",
        "a.testbed",
        "specs\\a.testbed",
        "./a.testbed",
    ] {
        let arguments = object! { "spec" => Json::str(path), "target" => Json::str("box") };
        let result = dispatch("testbed", &arguments, &fake);
        assert!(
            result.is_ok(),
            "{path:?} should be accepted, got {result:?}"
        );
    }
}

#[test]
fn the_testbed_tool_needs_both_of_its_arguments() {
    let fake = FakeRun::default();

    let no_spec = object! { "target" => Json::str("box") };
    assert!(matches!(
        dispatch("testbed", &no_spec, &fake),
        Err(ToolError::BadArgument { name: "spec", .. })
    ));

    let no_target = object! { "spec" => Json::str("a.testbed") };
    assert!(matches!(
        dispatch("testbed", &no_target, &fake),
        Err(ToolError::BadArgument { name: "target", .. })
    ));
}

#[test]
fn the_reply_is_the_text_the_runner_produced() {
    // The body the agent reads is the runner's text, carried through unchanged.
    // Not JSON: the protocol already wraps the result in one, a second encoding
    // would be a second thing to document, and a model reads a line of text
    // better than an escaped string.
    let fake = FakeRun::default();
    let outcome = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &fake,
    )
    .expect("a valid call");

    assert_eq!(
        outcome.text, "live 10.0.0.5:8787 connected\n1 of 1 live",
        "the reply should be whatever the runner said, not something dispatch composed"
    );
}

// --- the third tool ----------------------------------------------------------

#[test]
fn the_exec_tool_passes_its_three_arguments_through() {
    let fake = FakeRun::default();
    let arguments = object! {
        "agent" => Json::str("10.0.0.5:8787"),
        "command" => Json::str("build.cmd --release"),
        "timeout" => 120i64,
    };

    let outcome = dispatch("exec", &arguments, &fake).expect("a valid call");

    assert!(!outcome.is_error);
    assert_eq!(
        fake.exec_calls(),
        vec![(
            "10.0.0.5:8787".to_string(),
            "build.cmd --release".to_string(),
            120
        )]
    );
    assert!(
        fake.reachability_calls.borrow().is_empty() && fake.testbed_calls().is_empty(),
        "calling one tool must not run another"
    );
}

#[test]
fn the_exec_timeout_defaults_and_is_refused_outside_the_protocol_range() {
    // The ceiling is the protocol's own constant, checked at the surface so that
    // a caller learns from tools/list rather than from a refusal after a round
    // trip.
    let fake = FakeRun::default();

    dispatch(
        "exec",
        &object! { "agent" => Json::str("a:1"), "command" => Json::str("x") },
        &fake,
    )
    .expect("a valid call");
    assert_eq!(
        fake.exec_calls()[0].2,
        linklet_core::DEFAULT_EXEC_TIMEOUT_SECONDS
    );

    for value in [
        0i64,
        -1,
        (linklet_core::wire::MAX_TIMEOUT_SECONDS + 1) as i64,
    ] {
        let result = dispatch(
            "exec",
            &object! { "agent" => Json::str("a:1"), "command" => Json::str("x"), "timeout" => value },
            &fake,
        );
        assert!(
            matches!(
                result,
                Err(ToolError::BadArgument {
                    name: "timeout",
                    ..
                })
            ),
            "{value} should be refused, got {result:?}"
        );
    }
}

// --- the two transfer tools ---------------------------------------------------

#[test]
fn the_push_tool_passes_its_three_arguments_through() {
    let fake = FakeRun::default();
    let arguments = object! {
        "agent" => Json::str("10.0.0.5:8787"),
        "from" => Json::str("dist/app.exe"),
        "to" => Json::str("app.exe"),
    };

    let outcome = dispatch("push", &arguments, &fake).expect("a valid call");

    assert!(!outcome.is_error);
    assert_eq!(
        fake.transfer_calls(),
        vec![(
            "10.0.0.5:8787".to_string(),
            "dist/app.exe".to_string(),
            "app.exe".to_string()
        )]
    );
    assert!(
        fake.exec_calls().is_empty() && fake.testbed_calls().is_empty(),
        "calling one tool must not run another"
    );
}

#[test]
fn the_pull_tool_passes_its_three_arguments_through() {
    let fake = FakeRun::default();
    let arguments = object! {
        "agent" => Json::str("10.0.0.5:8787"),
        "from" => Json::str("build.log"),
        "to" => Json::str("logs/build.log"),
    };

    dispatch("pull", &arguments, &fake).expect("a valid call");

    assert_eq!(
        fake.transfer_calls(),
        vec![(
            "10.0.0.5:8787".to_string(),
            "build.log".to_string(),
            "logs/build.log".to_string()
        )]
    );
}

#[test]
fn a_transfer_refuses_a_local_path_outside_the_working_tree() {
    // An agent that can name any file on the machine has been handed more than this
    // tool is for, and a transfer is where that becomes two-way: copying
    // `C:\Windows\...` to a target is a read of this machine, and writing over it is a
    // write to it. Both directions are checked, and on the local side of each.
    let fake = FakeRun::default();

    for (tool, local_argument) in [("push", "from"), ("pull", "to")] {
        // The file's own name, rather than the whole path: the refusal quotes the path
        // with `{:?}`, so backslashes are doubled in it and comparing the raw text would
        // be asserting on how a debug format escapes things.
        for (path, name) in [
            (r"C:\Windows\win.ini", "win.ini"),
            ("../../secrets.txt", "secrets.txt"),
            ("/etc/passwd", "passwd"),
        ] {
            let arguments = match tool {
                "push" => object! {
                    "agent" => Json::str("a:1"),
                    "from" => Json::str(path),
                    "to" => Json::str("remote.exe"),
                },
                _ => object! {
                    "agent" => Json::str("a:1"),
                    "from" => Json::str("remote.log"),
                    "to" => Json::str(path),
                },
            };

            let result = dispatch(tool, &arguments, &fake);
            match result {
                Err(ToolError::BadArgument {
                    name: field,
                    problem,
                }) => {
                    assert_eq!(field, local_argument, "{tool} named the wrong argument");
                    assert!(
                        problem.contains(name) && problem.contains("working tree"),
                        "the refusal should name what it refused: {problem}"
                    );
                }
                other => panic!("{tool} accepted {path:?}: {other:?}"),
            }
        }
    }

    assert!(
        fake.transfer_calls().is_empty(),
        "a refused path must not reach the machine"
    );
}

#[test]
fn a_transfer_does_not_check_the_other_side_path() {
    // The remote path belongs to the agent, which owns that root. A second reading of
    // `docs/transfer.md` T1 here would be a second answer to the same question, and two
    // checks that disagree about what is allowed is worse than one that does not run.
    let fake = FakeRun::default();

    // An absolute path on the target is legal if the agent's root contains it, so
    // refusing it here would refuse a request the agent would have served.
    dispatch(
        "push",
        &object! {
            "agent" => Json::str("a:1"),
            "from" => Json::str("app.exe"),
            "to" => Json::str(r"C:\linklet\app.exe"),
        },
        &fake,
    )
    .expect("the target's own root decides this, and it is not this side's to check");

    assert_eq!(fake.transfer_calls().len(), 1);
}

#[test]
fn a_transfer_needs_all_three_arguments() {
    let fake = FakeRun::default();

    for (arguments, expected) in [
        (
            object! { "from" => Json::str("a"), "to" => Json::str("b") },
            "agent",
        ),
        (
            object! { "agent" => Json::str("a:1"), "to" => Json::str("b") },
            "from",
        ),
        (
            object! { "agent" => Json::str("a:1"), "from" => Json::str("a") },
            "to",
        ),
    ] {
        assert!(
            matches!(
                dispatch("push", &arguments, &fake),
                Err(ToolError::BadArgument { name, .. }) if name == expected
            ),
            "expected {expected:?} to be the missing argument for {arguments:?}"
        );
    }
}

#[test]
fn the_exec_tool_needs_an_agent_and_a_non_empty_command() {
    let fake = FakeRun::default();

    assert!(matches!(
        dispatch("exec", &object! { "command" => Json::str("x") }, &fake),
        Err(ToolError::BadArgument { name: "agent", .. })
    ));
    assert!(matches!(
        dispatch("exec", &object! { "agent" => Json::str("a:1") }, &fake),
        Err(ToolError::BadArgument {
            name: "command",
            ..
        })
    ));
    assert!(matches!(
        dispatch(
            "exec",
            &object! { "agent" => Json::str("a:1"), "command" => Json::str("   ") },
            &fake
        ),
        Err(ToolError::BadArgument {
            name: "command",
            ..
        })
    ));
}
