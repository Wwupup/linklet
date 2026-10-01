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

    /// Runs a command on an agent and reports what the command did.
    ///
    /// Takes the pieces rather than a [`crate::wire::RunRequest`] so that this
    /// crate stays the one deciding what a tool call means. An implementation
    /// builds the request; whether the request is well formed is decided here,
    /// before it ever sees a socket.
    fn exec(&self, agent: &str, command: &str, timeout_seconds: u64) -> ToolOutcome;

    /// Copies one local file to the agent, and reports what landed there.
    ///
    /// `to` is a path on the **agent's** side and is not resolved or checked here: the
    /// agent owns that root, and a second reading of `docs/transfer.md` T1 on this side
    /// would be a second answer to the same question -- which is how two checks come to
    /// disagree about what is allowed.
    fn push(&self, agent: &str, from: &str, to: &str) -> ToolOutcome;

    /// Copies one file back from the agent.
    ///
    /// The mirror of [`ToolRunner::push`], and here it is `to` that is local.
    fn pull(&self, agent: &str, from: &str, to: &str) -> ToolOutcome;

    /// Reports what is running on the agent's machine.
    ///
    /// Takes the filter's fields rather than a [`crate::process::Filter`] for the same
    /// reason [`ToolRunner::exec`] takes a command string: what a tool call means is
    /// decided here, and the implementation only carries it out.
    ///
    /// `name`, `path`, `cmdline`, `query` and `exclude` are the filter, and every one is
    /// optional -- no filter means everything. **The answer is a listing and not a list**:
    /// how many were examined, what filter was applied and what the machine could not
    /// supply are part of it, because an empty list without them is the shape
    /// `docs/ROADMAP.md` M10 records as dangerous.
    fn ps(&self, agent: &str, filter: &crate::process::Filter) -> ToolOutcome;

    /// Stops something on the agent's machine.
    ///
    /// Takes the whole [`crate::wire::KillRequest`] rather than its parts, unlike the two
    /// capabilities above, and the reason is that the parts are not independent: whether a
    /// request is safe depends on what it names, on whether the caller meant it, and on what
    /// it is willing to consider -- and a signature of four loose arguments is one someone
    /// can call with the force flag in the wrong place. The tool schema is where a caller's
    /// arguments are checked; this is where they are carried.
    ///
    /// **A request that must not be attempted is an `Err(Refusal)` on the agent**, and the
    /// implementation reports it as a failed call rather than as a report: "nothing was
    /// attempted" and "nothing was killed" are different answers, and only one of them means
    /// the caller should think again.
    fn kill(&self, agent: &str, request: &crate::wire::KillRequest) -> ToolOutcome;

    /// Starts a program on the agent's machine that outlives this call.
    ///
    /// **The difference from [`ToolRunner::exec`] is the point of the tool**, and the answer
    /// says only what this side knows: a pid. Whether the program is still there a moment
    /// later is [`ToolRunner::ps`]'s question, and a tool that answered both would be
    /// claiming to have looked when it had not.
    fn spawn(&self, agent: &str, request: &crate::wire::SpawnRequest) -> ToolOutcome;
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

/// An optional string argument.
///
/// Absent is `None` and not an error, which is what makes a filter's fields optional: a
/// caller that did not filter on a field did not make a mistake.
///
/// **A value of the wrong shape is an error** rather than a silent `None`: a caller that
/// sent `"name": 5` meant to filter and said it wrong, and dropping the filter would answer
/// a different question than the one asked -- which, for a listing, is the failure this
/// whole feature is arranged against.
///
/// Returns `Result` rather than `Option` for that reason.
fn optional_str(arguments: &Json, name: &'static str) -> Result<Option<String>, ToolError> {
    match arguments.get(name) {
        None => Ok(None),
        Some(Json::Str(text)) => Ok(Some(text.clone()).filter(|text| !text.trim().is_empty())),
        Some(other) => Err(ToolError::BadArgument {
            name,
            problem: format!("expected a string, got {}", type_name(other)),
        }),
    }
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

/// A path that stays inside the working tree, by the name the tool calls it.
///
/// An agent that can name any file on the machine has been handed more than this tool
/// is for. "Read the specification at C:\Windows\..." is not a request worth serving,
/// and neither is "copy C:\Windows\..." to a target or "write this over C:\Windows\...",
/// which are the two directions a transfer adds. Rejecting the two ways out of the tree
/// -- an absolute path and a `..` -- is cheaper than reasoning about what a caller meant,
/// and the refusal names the argument it refused.
fn relative_local_path(arguments: &Json, name: &'static str) -> Result<String, ToolError> {
    let path = required_str(arguments, name)?;

    let is_absolute = path.starts_with('/')
        || path.starts_with('\\')
        || path.chars().nth(1).is_some_and(|c| c == ':');
    let climbs_out = path.split(['/', '\\']).any(|part| part == "..");

    if is_absolute || climbs_out {
        return Err(ToolError::BadArgument {
            name,
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
/// Five, and each has an argument for why it is not part of another. `check` asks
/// whether a port answers, which needs no configuration at all; `testbed` asks whether a
/// machine satisfies a written specification, which needs a file; `exec` runs something.
/// `push` and `pull` are two tools rather than one with a direction, for the reason the
/// first two are: a single tool would have two mutually exclusive argument sets and a
/// description that has to explain which of `from` and `to` is local this time.
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
        Tool {
            name: "exec",
            // The third tool, and the first whose description took thought. It
            // has to say what runs the command, because "run a command" alone
            // would leave a reader unsure whether it runs here or there -- and
            // that is the one thing a caller must know before using it.
            description: "Run a command on a remote linklet agent and return its output.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "command": {
                            "type": "string",
                            "description": "the command line, as it would be typed in cmd.exe"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "seconds the command may run before it is killed",
                            "minimum": 1,
                            "maximum": 600
                        }
                    },
                    "required": ["agent", "command"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "push",
            description: "Copy one local file to a remote linklet agent.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "from": {
                            "type": "string",
                            "description": "path to the local file, inside the working tree"
                        },
                        "to": {
                            "type": "string",
                            "description": "a path on the target, under the agent's transfer root"
                        }
                    },
                    "required": ["agent", "from", "to"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "pull",
            description: "Copy one file from a remote linklet agent to here.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "from": {
                            "type": "string",
                            "description": "a path on the target, under the agent's transfer root"
                        },
                        "to": {
                            "type": "string",
                            "description": "where to put it locally, inside the working tree"
                        }
                    },
                    "required": ["agent", "from", "to"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "ps",
            // Short, and every word is there because a reader would otherwise have to
            // guess at it: *what* is listed (processes), and *where* (the remote agent's
            // machine, not this one). What makes it different from reading a list -- that
            // it says how many it examined, so an empty answer is readable -- is in the
            // result rather than in the description, which is the right place for it:
            // `docs/MCP.md` refuses a description that has to teach a manual.
            description: "List processes on a remote linklet agent's machine.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "name": {
                            "type": "string",
                            "description": "keep processes whose image name contains this"
                        },
                        "cmdline": {
                            "type": "string",
                            "description": "keep processes whose command line contains this"
                        },
                        "query": {
                            "type": "string",
                            "description": "keep processes matching this in any field"
                        },
                        "exclude": {
                            "type": "string",
                            "description": "drop processes whose name or command line contains this"
                        }
                    },
                    "required": ["agent"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "kill",
            // The one description that has to carry a warning, because this is the one tool
            // whose mistake cannot be undone. "Stop processes" says what it does; "on a
            // remote linklet agent's machine" says where, which is the fact a reader must
            // not have to guess before using it.
            description: "Stop processes on a remote linklet agent's machine.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "pid": {
                            "type": "integer",
                            "description": "stop this one process; never needs confirmed"
                        },
                        "name": {
                            "type": "string",
                            "description": "stop processes whose image name is exactly this"
                        },
                        "confirmed": {
                            "type": "boolean",
                            "description": "required for name: it can match more than one process"
                        },
                        "candidates_cmdline": {
                            "type": "string",
                            "description": "only consider processes whose command line contains this"
                        }
                    },
                    "required": ["agent"],
                    "additionalProperties": false
                }"#,
            )
            .expect("the schema above is a literal and parses"),
        },
        Tool {
            name: "spawn",
            // The one description where the *absence* of waiting is the fact a reader needs,
            // and it cannot be said in the description's budget without becoming a manual.
            // So the description says what it does and where, and the word "started" in the
            // result is what tells a caller this is not `exec`: nothing was waited for.
            description: "Start a program on a remote linklet agent's machine, and return at once.",
            input_schema: json::parse(
                r#"{
                    "type": "object",
                    "properties": {
                        "agent": {
                            "type": "string",
                            "description": "the agent's host:port, for example 10.0.0.5:8787"
                        },
                        "command": {
                            "type": "string",
                            "description": "the command line, run through the target's shell"
                        },
                        "output": {
                            "type": "string",
                            "description": "where on the target the program's output goes"
                        }
                    },
                    "required": ["agent", "command", "output"],
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
            let spec = relative_local_path(arguments, "spec")?;
            let target = required_str(arguments, "target")?;
            Ok(runner.testbed(&spec, &target))
        }
        "exec" => {
            reject_unknown(arguments, &["agent", "command", "timeout"])?;
            let agent = required_str(arguments, "agent")?;
            let command = required_str(arguments, "command")?;
            if command.trim().is_empty() {
                return Err(ToolError::BadArgument {
                    name: "command",
                    problem: "empty".to_string(),
                });
            }
            // The same ceiling the protocol enforces, checked here so that a
            // caller learns from the tool surface rather than from a refusal
            // after a round trip. The two constants are one constant; the
            // protocol owns it.
            let timeout = optional_int(
                arguments,
                "timeout",
                DEFAULT_EXEC_TIMEOUT_SECONDS,
                crate::wire::MAX_TIMEOUT_SECONDS,
            )?;
            Ok(runner.exec(&agent, &command, timeout))
        }
        "push" => {
            reject_unknown(arguments, &["agent", "from", "to"])?;
            let agent = required_str(arguments, "agent")?;
            // `from` is the file here, so it is the one bounded by the working tree.
            let from = relative_local_path(arguments, "from")?;
            // `to` is a path on the agent, which owns that root. Deliberately not
            // checked here -- see `ToolRunner::push`.
            let to = required_str(arguments, "to")?;
            Ok(runner.push(&agent, &from, &to))
        }
        "pull" => {
            reject_unknown(arguments, &["agent", "from", "to"])?;
            let agent = required_str(arguments, "agent")?;
            let from = required_str(arguments, "from")?;
            // The mirror of `push`: the local half is `to`, and it is the one that may
            // not leave the tree.
            let to = relative_local_path(arguments, "to")?;
            Ok(runner.pull(&agent, &from, &to))
        }
        "ps" => {
            reject_unknown(arguments, &["agent", "name", "cmdline", "query", "exclude"])?;
            let agent = required_str(arguments, "agent")?;
            // Every filter field is optional and they are the same five the protocol
            // carries. `path` is accepted by the wire and **not** by this tool, because the
            // one implementation of `ps` cannot supply it: a tool argument that is always
            // unanswerable is worse than an absent one, and `docs/MCP.md` says so about
            // tools that exist with nothing behind them.
            let filter = crate::process::Filter {
                name: optional_str(arguments, "name")?,
                path: None,
                cmdline: optional_str(arguments, "cmdline")?,
                query: optional_str(arguments, "query")?,
                exclude: optional_str(arguments, "exclude")?,
            };
            Ok(runner.ps(&agent, &filter))
        }
        "kill" => {
            reject_unknown(
                arguments,
                &["agent", "pid", "name", "confirmed", "candidates_cmdline"],
            )?;
            let agent = required_str(arguments, "agent")?;

            // **A name or a pid, and never both.** The wire refuses a request that names
            // neither, and a request naming both would need a rule for which wins -- which
            // is the kind of rule that is discovered in a diff rather than written in one.
            let by_pid = match arguments.get("pid") {
                Some(Json::Int(pid)) if *pid > 0 => Some(*pid as u32),
                Some(Json::Int(pid)) => {
                    return Err(ToolError::BadArgument {
                        name: "pid",
                        problem: format!("{pid} is not a process identifier"),
                    });
                }
                Some(_) => {
                    return Err(ToolError::BadArgument {
                        name: "pid",
                        problem: "expected a number".to_string(),
                    });
                }
                None => None,
            };

            let to_kill = match (by_pid, optional_str(arguments, "name")?) {
                (Some(pid), None) => crate::process::ToKill::Pid(pid),
                (None, Some(name)) => crate::process::ToKill::Name(name),
                (Some(_), Some(_)) => {
                    return Err(ToolError::BadArgument {
                        name: "name",
                        problem: "give a pid or a name, not both".to_string(),
                    });
                }
                (None, None) => {
                    return Err(ToolError::BadArgument {
                        name: "pid",
                        problem: "one of pid or name is required".to_string(),
                    });
                }
            };

            let request = crate::wire::KillRequest {
                to_kill,
                // The tool's word for it is `confirmed`, and it is a different word on
                // purpose: an agent reading a schema sees "confirmed" and knows it is being
                // asked to mean it, where `force` reads like a technical switch.
                force: arguments.get("confirmed").and_then(Json::as_bool) == Some(true),
                candidates: crate::process::Filter {
                    cmdline: optional_str(arguments, "candidates_cmdline")?,
                    ..crate::process::Filter::any()
                },
                exclude: None,
            };
            Ok(runner.kill(&agent, &request))
        }
        "spawn" => {
            reject_unknown(arguments, &["agent", "command", "output"])?;
            let agent = required_str(arguments, "agent")?;
            let command = required_str(arguments, "command")?;
            if command.trim().is_empty() {
                return Err(ToolError::BadArgument {
                    name: "command",
                    problem: "empty".to_string(),
                });
            }
            // **Required, and the same refusal the command line gives**: a program whose
            // output goes nowhere cannot be looked at afterwards, which is the one thing this
            // capability is for.
            let output = required_str(arguments, "output")?;
            if output.trim().is_empty() {
                return Err(ToolError::BadArgument {
                    name: "output",
                    problem: "empty".to_string(),
                });
            }

            Ok(runner.spawn(&agent, &crate::wire::SpawnRequest { command, output }))
        }
        other => Err(ToolError::NoSuchTool(other.to_string())),
    }
}

/// The timeout an `exec` call gets when the caller does not ask for one.
///
/// Long enough for a build step, short enough that a mistake ends: a caller that
/// wanted longer can say so, and a caller that did not think about it should not
/// get an unbounded wait.
pub const DEFAULT_EXEC_TIMEOUT_SECONDS: u64 = 60;
