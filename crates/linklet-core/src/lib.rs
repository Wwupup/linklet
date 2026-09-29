//! The decisions, with nothing else attached.
//!
//! Everything in this crate is a pure function of its arguments: the same
//! input produces the same output, nothing is read from the environment,
//! nothing is written, and nothing can fail for a reason that depends on the
//! state of the machine it runs on.
//!
//! That property is the whole point. A decision that can only be tested by
//! touching a real machine is a decision that stops being tested the first
//! time the machine is unavailable, and it is wrong by then.
//!
//! The crate has no dependencies, so this is enforced by the compiler rather
//! than by convention. See `AGENTS.md`, rule 1.
//!
//! # How it asks for what it cannot do
//!
//! Reachability can only be observed by connecting, which is I/O, which cannot
//! happen here. Rather than reaching for it, this crate **states what it
//! needs**: [`Probe`] is declared here and implemented in `linklet-adapters`.
//! The arrow points `adapters -> core`, so a rule can be tested by handing it a
//! fake instead of a machine -- see `tests/probe_check.rs`, which covers
//! timeouts without anything timing out. See `docs/rationale.md` rule 1.

#![forbid(unsafe_code)]

mod error;
pub mod json;
mod outcome;
mod probe;
mod target;
pub mod testbed;
mod tool;

pub use error::TargetError;
pub use outcome::{ExitCode, exit_code_for, render};
pub use probe::{
    CheckError, DEFAULT_BUDGET_SECONDS, MAX_BUDGET_SECONDS, MAX_TARGETS, Probe, ProbeOutcome,
    Report, Status, Summary, check_targets,
};
pub use target::{Host, Port, Target, parse_targets};
pub use tool::{
    MAX_DESCRIPTION_CHARS, Tool, ToolError, ToolOutcome, dispatch, tool_list_json, tools,
    total_description_chars,
};
