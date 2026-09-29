//! The command line: read `argv`, call the core, print, set an exit code.
//!
//! The rule for this crate is that it contains no decisions. Every `if` below is
//! about argument shape, printing, or which exit code to use -- and the exit
//! codes themselves are [`linklet_core::ExitCode`], not numbers written here.
//! What the tool *says* is [`linklet_core::render`], and what it *returns* is
//! [`linklet_core::exit_code_for`]; both are tested in the core, where they can
//! be checked without running a process.
//!
//! # Exit codes
//!
//! | code | meaning |
//! |------|---------|
//! | 0    | the run completed and everything asked about is alive |
//! | 1    | the run completed and something is not |
//! | 2    | the invocation was wrong |
//! | 3    | the run was refused before anything was looked at |
//!
//! # Output
//!
//! One line per target, `status target reason`, in the order the targets were
//! given. The status tokens are `live`, `dead` and `unknown`. Written to stdout,
//! including for a target that is not alive -- a result is not an error
//! condition, and an agent that has to merge two streams to reconstruct the
//! report will eventually not bother.

use std::env;
use std::process::ExitCode as ProcessExit;
use std::time::Duration;

use linklet_adapters::TcpProbe;
use linklet_core::{
    CheckError, DEFAULT_BUDGET_SECONDS, ExitCode, MAX_TARGETS, check_targets, exit_code_for,
    parse_targets, render,
};

/// The usage text, printed for `--help` and for a wrong invocation.
///
/// Kept to what a caller needs to act: no feature list, no marketing, no
/// examples that will be out of date within a release.
const USAGE: &str = "\
linklet -- check whether machines on a LAN are listening

usage:
  linklet check [options] <target>[,<target>...]

target:
  host:port            for example 10.0.0.5:8787
  several may be given, separated by commas or by spaces

options:
  --timeout <seconds>     how long to wait for each target (default 5)
  --max-targets <count>   refuse a run larger than this (default 256)
  -h, --help              print this
";

/// Why the invocation was wrong.
///
/// A separate type from [`CheckError`] because the two are exit code 2 and exit
/// code 3: the invocation being wrong is a different event from a run being
/// refused, and an agent branches on that difference.
///
/// A target that does not parse is *not* in here. It is exit code 2 as well, but
/// it is reported by `run` straight from the core's own error, and adding a
/// variant that only wraps somebody else's message would be a layer with no
/// decision in it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CliError {
    /// A flag that takes a value did not get one.
    MissingValue {
        /// The flag.
        flag: String,
    },
    /// A flag that takes a number did not get one.
    NotANumber {
        /// The flag.
        flag: String,
        /// What was given.
        value: String,
    },
    /// Arguments that made no sense for the command given.
    Usage(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingValue { flag } => write!(f, "{flag} needs a value"),
            Self::NotANumber { flag, value } => {
                write!(f, "{flag} needs a number, got {value:?}")
            }
            Self::Usage(message) => write!(f, "{message}"),
        }
    }
}

/// The options a run was invoked with.
struct Options {
    /// The text of every target argument, joined with commas.
    specs: String,
    /// How long to wait for each target.
    budget: Duration,
    /// How many targets one run will accept.
    max_targets: usize,
}

fn main() -> ProcessExit {
    let arguments: Vec<String> = env::args().skip(1).collect();

    // The code the core computed, turned into the process's own type here and
    // nowhere else. Returning it through `ProcessExit` rather than calling
    // `std::process::exit` keeps every path in this file a normal return, which
    // is what lets the compiler check that `main` always ends.
    ProcessExit::from(dispatch(&arguments))
}

/// Runs the tool and returns the exit code. Never panics and never exits.
///
/// Kept separate from `main` so that every branch ends in a code the tests can
/// reason about, and so that the only line that touches the process is the one
/// above.
fn dispatch(arguments: &[String]) -> u8 {
    match parse_arguments(arguments) {
        Ok(None) => {
            // `--help`: asked for, so it is not an error, but it is also not a
            // run. Exit 0 would be a lie about having checked something, and 2
            // would be a lie about the invocation being wrong; the honest answer
            // is the usage code, and the help is on stdout where a caller asked
            // for it.
            print!("{USAGE}");
            ExitCode::USAGE
        }
        Ok(Some(options)) => run(&options),
        Err(error) => {
            // Errors go to stderr so that a caller piping stdout gets only
            // report lines, and gets no output at all for a run that never
            // happened.
            eprintln!("linklet: {error}");
            ExitCode::USAGE
        }
    }
}

/// Turns `argv` into options, or says why it cannot.
///
/// `Ok(None)` means help was asked for. The distinction between "asked for help"
/// and "made a mistake" exists so the two can print to different streams.
fn parse_arguments(arguments: &[String]) -> Result<Option<Options>, CliError> {
    let mut specs: Vec<String> = Vec::new();
    let mut budget_seconds = DEFAULT_BUDGET_SECONDS;
    let mut max_targets = MAX_TARGETS;
    let mut command_seen = false;
    let mut iterator = arguments.iter();

    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(None),
            "--timeout" => {
                let value = iterator.next().ok_or_else(|| CliError::MissingValue {
                    flag: "--timeout".to_string(),
                })?;
                budget_seconds = value.parse().map_err(|_| CliError::NotANumber {
                    flag: "--timeout".to_string(),
                    value: value.clone(),
                })?;
            }
            "--max-targets" => {
                let value = iterator.next().ok_or_else(|| CliError::MissingValue {
                    flag: "--max-targets".to_string(),
                })?;
                max_targets = value.parse().map_err(|_| CliError::NotANumber {
                    flag: "--max-targets".to_string(),
                    value: value.clone(),
                })?;
            }
            "check" if !command_seen => command_seen = true,
            other if other.starts_with('-') => {
                return Err(CliError::Usage(format!("unknown option {other:?}")));
            }
            other => specs.push(other.to_string()),
        }
    }

    if !command_seen {
        return Err(CliError::Usage(
            "no command given; expected `check`".to_string(),
        ));
    }

    if specs.is_empty() {
        return Err(CliError::Usage("no targets given".to_string()));
    }

    Ok(Some(Options {
        // Joined rather than parsed one at a time, so that `check a:1 b:2` and
        // `check a:1,b:2` reach the core as the same text and cannot behave
        // differently. The grammar in one place, not two.
        specs: specs.join(","),
        budget: Duration::from_secs(budget_seconds),
        max_targets,
    }))
}

/// Parses the targets, checks them, prints the report, and returns the code.
fn run(options: &Options) -> u8 {
    let targets = match parse_targets(&options.specs) {
        Ok(targets) => targets,
        Err(error) => {
            eprintln!("linklet: {error}");
            return ExitCode::USAGE;
        }
    };

    // Unused in this build on purpose: the concurrency that would earn it is
    // milestone M4. Named here rather than left out so that the parameter list
    // does not have to change when it arrives.
    let probe = TcpProbe;

    match check_targets(&probe, &targets, options.budget, options.max_targets) {
        Ok(reports) => {
            for report in &reports {
                println!("{}", render(report));
            }
            exit_code_for(&reports)
        }
        Err(error) => {
            // Refusals are printed with their numbers as the core words them, so
            // there is one place that decides what "900 targets, at most 256" is
            // called.
            eprintln!("linklet: {error}");
            refused_code(&error)
        }
    }
}

/// The exit code for a refused run.
///
/// Every [`CheckError`] is a refusal, so this is a constant with an exhaustive
/// match over it: if a future variant means something else, this stops compiling
/// instead of silently returning the wrong code.
fn refused_code(error: &CheckError) -> u8 {
    match error {
        CheckError::NoTargets | CheckError::TooManyTargets { .. } | CheckError::ZeroLimit => {
            ExitCode::REFUSED
        }
    }
}
