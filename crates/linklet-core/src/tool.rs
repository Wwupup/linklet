//! The MCP surface: which tools exist, what they say, and what a call means.
//!
//! This module is the answer to a question the rest of the project is a
//! rehearsal for: **an agent reads this list, and a list it cannot act on from
//! the descriptions alone is a list that needs a manual.** The project this one
//! is modelled on ended up with seventeen tools and 13,758 characters of
//! description, most of it spent explaining when to use a *different* tool.
//! That is the failure mode, and the rules below are drawn directly from it.
//!
//! # The rules
//!
//! 1. **A tool exists because there is a question, not because there is an
//!    endpoint.** One tool per intent. "Get the logs" is an intent; `/tail` and
//!    `/grep` are two endpoints serving it and belong behind one tool.
//! 2. **No tool's description refers to another tool.** The moment one does,
//!    the list has become a graph the reader has to traverse and the manual has
//!    started to write itself. If a description wants to say "use X instead",
//!    the tool it is attached to should not exist.
//! 3. **A tool is a promise that the capability is there.** A tool that answers
//!    "not implemented" is worse than an absent one: the agent has spent a turn
//!    on it and cannot tell a missing feature from a broken one.
//!
//! Rule 3 is why there is one tool here and not five. This build can answer
//! exactly one question -- whether a machine answers on a port. Running a
//! command, reading a log and moving a file all need something on the far side
//! to talk to, and there is nothing there yet. They arrive with the agent that
//! serves them, and `tests/tool_surface.rs` is where that promise is kept: the
//! count is asserted, so adding a tool has to be deliberate.

use std::time::Duration;

use crate::json::{self, Json};

/// One tool, in the shape MCP wants it.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The name the agent calls it by.
    pub name: &'static str,
    /// What it does, and nothing else.
    pub description: &'static str,
    /// The JSON Schema for its arguments.
    pub input_schema: Json,
}

/// What a call produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The reply body.
    pub text: String,
    /// Whether the tool failed to do what it was asked.
    ///
    /// Not the same as the news being bad. "Three machines are down" is a
    /// successful call carrying bad news, and marking it an error would teach
    /// the agent to retry a tool that worked.
    pub is_error: bool,
}

impl ToolOutcome {
    /// A call that did what it was asked.
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }

    /// A call that could not be made, with the reason.
    pub fn failed(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Why a call could not even be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// No tool by that name.
    NoSuchTool(String),
    /// A required argument was absent or the wrong shape.
    BadArgument {
        /// The argument.
        name: &'static str,
        /// What was wrong with it.
        problem: String,
    },
    /// An argument was given that the tool does not take.
    ///
    /// Reported rather than ignored, because an ignored argument is an agent
    /// that believes it asked for something it did not. The commonest case is a
    /// typo in the name of an optional one.
    UnknownArgument(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchTool(name) => write!(f, "no tool called {name:?}"),
            Self::BadArgument { name, problem } => write!(f, "argument {name:?}: {problem}"),
            Self::UnknownArgument(name) => {
                write!(f, "unknown argument {name:?}; run tools/list for the shape")
            }
        }
    }
}

impl std::error::Error for ToolError {}

/// Everything the tools can do.
///
/// A trait rather than more closure parameters. With one tool, [`dispatch`] took
/// one closure and read fine; with two it would take two, and every test would
/// have to supply a stub for the capability it does not exercise. The signature
/// would become the least readable part of this file.
///
/// The same inversion as [`crate::Probe`] and [`crate::testbed::Prober`]: this
/// crate decides *what to ask*, an implementation decides *how to ask a machine*,
/// and `tests/tool_surface.rs` supplies one made of data.
///
/// Returns [`ToolOutcome`] rather than `String` so a capability can report its
/// own failure. A testbed specification that does not exist is a failed call; a
/// machine that is not ready is a successful one, and only the implementation
/// knows which it is holding.
pub trait ToolRunner {
    /// Whether each target accepts a connection.
    fn reachability(&self, targets: &str, budget: Duration) -> ToolOutcome;

    /// Whether a machine matches a testbed specification.
    fn testbed(&self, spec_path: &str, target: &str) -> ToolOutcome;
}

/// The name of a JSON value's type, for an error message.
fn type_name(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Int(_) | Json::Float(_) => "a number",
        Json::Str(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

/// Rejects any argument the tool does not take.
fn reject_unknown(arguments: &Json, allowed: &[&str]) -> Result<(), ToolError> {
    let Json::Object(entries) = arguments else {
        return Ok(());
    };
    for key in entries.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(ToolError::UnknownArgument(key.clone()));
        }
    }
    Ok(())
}

/// A required string argument.
fn required_str(arguments: &Json, name: &'static str) -> Result<String, ToolError> {
    match arguments.get(name) {
        Some(Json::Str(text)) => Ok(text.clone()),
        Some(other) => Err(ToolError::BadArgument {
            name,
            problem: format!("expected a string, got {}", type_name(other)),
        }),
        None => Err(ToolError::BadArgument {
            name,
            problem: "missing".to_string(),
        }),
    }
}

/// A path that stays inside the working tree.
///
/// An agent that can name any file on the machine has been handed more than this
/// tool is for, and "read the specification at C:\Windows\..." is not a request
/// worth serving. Rejecting the two ways out of the tree -- an absolute path and
/// a `..` -- is cheaper than reasoning about what a caller meant, and a refusal
/// names the reason.
fn relative_spec_path(arguments: &Json) -> Result<String, ToolError> {
    let path = required_str(arguments, "spec")?;

    let is_absolute = path.starts_with('/')
        || path.starts_with('\\')
        || path.chars().nth(1).is_some_and(|c| c == ':');
    let climbs_out = path.split(['/', '\\']).any(|part| part == "..");

    if is_absolute || climbs_out {
        return Err(ToolError::BadArgument {
            name: "spec",
            problem: format!("{path:?} is outside the working tree; give a path inside it"),
        });
    }

    Ok(path)
}

/// The target list, as the one spec the parser takes.
///
/// MCP sends an array and [`crate::parse_targets`] takes a comma-separated
/// string, so the two shapes meet here. Joining rather than parsing entry by
/// entry keeps the grammar in one place: a caller that split the list itself
/// would be a second answer to what a target is.
fn targets_from(arguments: &Json) -> Result<String, ToolError> {
    let Some(value) = arguments.get("targets") else {
        return Err(ToolError::BadArgument {
            name: "targets",
            problem: "missing".to_string(),
        });
    };

    let Json::Array(items) = value else {
        return Err(ToolError::BadArgument {
            name: "targets",
            problem: format!("expected an array, got {}", type_name(value)),
        });
    };

    if items.is_empty() {
        return Err(ToolError::BadArgument {
            name: "targets",
            problem: "the list is empty".to_string(),
        });
    }

    let mut specs = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match item {
            Json::Str(text) => specs.push(text.clone()),
            other => {
                return Err(ToolError::BadArgument {
                    name: "targets",
                    problem: format!("entry {index} is {} rather than a string", type_name(other)),
                });
            }
        }
    }

    Ok(specs.join(","))
}

/// An optional integer argument, with a default, a floor and a ceiling.
///
/// Out of range is an error rather than a clamp. A caller that asked for 10,000
/// seconds and silently got 10 has been lied to about what happened, and this
/// project exists to not do that.
fn optional_int(
    arguments: &Json,
    name: &'static str,
    default: u64,
    max: u64,
) -> Result<u64, ToolError> {
    match arguments.get(name) {
        None => Ok(default),
        Some(Json::Int(value)) => {
            if *value < 1 {
                return Err(ToolError::BadArgument {
                    name,
                    problem: format!("{value} is not a positive number of seconds"),
                });
            }
            let value = *value as u64;
            if value > max {
                return Err(ToolError::BadArgument {
                    name,
                    problem: format!("{value} is more than the maximum of {max}"),
                });
            }
            Ok(value)
        }
        Some(other) => Err(ToolError::BadArgument {
            name,
            problem: format!("expected an integer, got {}", type_name(other)),
        }),
    }
}

/// The longest a single description may be.
///
/// A tool whose description needs more than this to be actionable is usually a
/// tool doing more than one thing. Small on purpose: it is a budget, and a
/// budget that cannot be exceeded is not a budget.
pub const MAX_DESCRIPTION_CHARS: usize = 120;

/// The tools this build offers.
///
/// Two. Each has an argument for why it is not part of the other: `check` asks
/// whether a port answers, which needs no configuration at all, and `testbed`
/// asks whether a machine satisfies a written specification, which needs a file.
/// Folding the second into the first would make `check` a tool with two
/// mutually exclusive argument sets and a description that has to explain both.
pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "check",
            description: "Report whether each host:port accepts a TCP connection.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "targets": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "host:port, for example 10.0.0.5:8787"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "seconds to wait for each host",
                            "minimum": 1,
                            "maximum": 10
                        }
                    },
                    "required": ["targets"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "testbed",
            description: "Report whether a machine matches a testbed specification file.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "spec": {
                            "type": "string",
                            "description": "path to a testbed file, inside the working tree"
                        },
                        "target": {
                            "type": "string",
                            "description": "a label for the machine being checked"
                        }
                    },
                    "required": ["spec", "target"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
    ]
}

/// The tool list in the shape `tools/list` replies with.
///
/// Built from [`tools`] rather than written out, so a tool cannot appear in the
/// list with a name the dispatcher has never heard of. That mismatch is the kind
/// of bug that shows up only in a live session.
pub fn tool_list_json() -> Json {
    Json::Array(
        tools()
            .into_iter()
            .map(|tool| {
                Json::Object(
                    [
                        ("name".to_string(), Json::str(tool.name)),
                        ("description".to_string(), Json::str(tool.description)),
                        ("inputSchema".to_string(), tool.input_schema),
                    ]
                    .into_iter()
                    .collect(),
                )
            })
            .collect(),
    )
}

/// The total size of every tool description, in characters.
///
/// Exists so a test can hold a number to something. The failure this project is
/// a reaction to had 13,758 characters of description across seventeen tools,
/// and nobody noticed because nobody was counting.
pub fn total_description_chars() -> usize {
    tools().iter().map(|tool| tool.description.len()).sum()
}

/// Runs a call.
///
/// The closure is the outside world, handed in the same way [`crate::Probe`]
/// is. The decisions -- which tool, whether the arguments fit, how the answer
/// reads -- are testable without a network, and `tests/tool_surface.rs` does
/// exactly that.
///
/// # Errors
///
/// [`ToolError`] for a call that could not be dispatched at all. A tool that ran
/// and had bad news returns `Ok(ToolOutcome)`, because bad news is an answer.
pub fn dispatch(
    name: &str,
    arguments: &Json,
    runner: &dyn ToolRunner,
) -> Result<ToolOutcome, ToolError> {
    match name {
        "check" => {
            reject_unknown(arguments, &["targets", "timeout"])?;
            let specs = targets_from(arguments)?;
            let timeout = optional_int(arguments, "timeout", 5, 10)?;
            Ok(runner.reachability(&specs, Duration::from_secs(timeout)))
        }
        "testbed" => {
            reject_unknown(arguments, &["spec", "target"])?;
            let spec = relative_spec_path(arguments)?;
            let target = required_str(arguments, "target")?;
            Ok(runner.testbed(&spec, &target))
        }
        other => Err(ToolError::NoSuchTool(other.to_string())),
    }
}
