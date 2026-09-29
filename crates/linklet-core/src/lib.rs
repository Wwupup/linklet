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

#![forbid(unsafe_code)]

mod error;
mod target;

pub use error::TargetError;
pub use target::{Host, Port, Target, parse_targets};
