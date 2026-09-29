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
    MAX_DESCRIPTION_CHARS, ToolError, dispatch, tool_list_json, tools, total_description_chars,
};

/// A stand-in for the network: records what it was asked, returns a fixed body.
#[derive(Default)]
struct FakeRun {
    calls: RefCell<Vec<(String, Duration)>>,
}

impl FakeRun {
    fn record(&self, specs: &str, budget: Duration) -> String {
        self.calls.borrow_mut().push((specs.to_string(), budget));
        "live 10.0.0.5:8787 connected\n1 of 1 live".to_string()
    }
}

/// The closure `dispatch` takes, bound to a recorder.
fn runner(fake: &FakeRun) -> impl Fn(&str, Duration) -> String + '_ {
    move |specs, budget| fake.record(specs, budget)
}

// --- the shape of the surface ------------------------------------------------

#[test]
fn there_is_exactly_one_tool() {
    // The count is the assertion. Growing this list is a decision, and the way
    // to make it is to change this number and say in the commit why the new tool
    // earns its place -- which is exactly the conversation that was never had
    // the last time.
    assert_eq!(
        tools().len(),
        1,
        "adding a tool is a decision: change this number and explain in the commit \
         why the new question needs its own tool rather than belonging to this one"
    );
    assert_eq!(tools()[0].name, "check");
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
        let result = dispatch(name, &object! {}, &|_, _| String::new());
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
    let result = dispatch("nope", &object! {}, &|_, _| String::new());
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

    let outcome = dispatch("check", &arguments, &runner(&fake)).expect("a valid call");

    assert!(!outcome.is_error);
    let calls = fake.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "a:1,b:2");
}

#[test]
fn the_timeout_defaults_and_is_passed_through() {
    let fake = FakeRun::default();

    dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &runner(&fake),
    )
    .expect("a valid call");
    dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")], "timeout" => 9i64 },
        &runner(&fake),
    )
    .expect("a valid call");

    let calls = fake.calls.borrow();
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
            &runner(&fake),
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
        fake.calls.borrow().is_empty(),
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
        let result = dispatch("check", &arguments, &runner(&fake));
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
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn an_unknown_argument_is_refused_rather_than_ignored() {
    // The failure this prevents: the caller believes it asked for something, the
    // tool did not, and nothing in the reply says so.
    let fake = FakeRun::default();
    let result = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")], "timout" => 5i64 },
        &runner(&fake),
    );

    assert_eq!(
        result,
        Err(ToolError::UnknownArgument("timout".to_string()))
    );
    assert!(fake.calls.borrow().is_empty());
}

#[test]
fn bad_news_is_not_an_error() {
    // "Three machines are down" is a successful call. Marking it an error would
    // teach the agent to retry a tool that worked.
    let outcome = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &|_, _| "dead a:1 nothing is listening on that port\n0 of 1 live".to_string(),
    )
    .expect("a valid call");

    assert!(!outcome.is_error);
    assert!(outcome.text.contains("0 of 1 live"));
}

#[test]
fn the_reply_is_the_text_the_checker_produced() {
    // The body the agent reads, carried through unchanged. Not JSON: the
    // protocol already wraps the result in one, a second encoding would be a
    // second thing to document, and a model reads a line of text better than an
    // escaped string.
    let produced = "live a:1 connected\n1 of 1 live";
    let outcome = dispatch(
        "check",
        &object! { "targets" => vec![Json::str("a:1")] },
        &|_, _| produced.to_string(),
    )
    .expect("a valid call");

    assert_eq!(outcome.text, produced);
}
