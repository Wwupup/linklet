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

use linklet_adapters::{SystemProber, TcpProbe, serve};
use linklet_core::testbed::{self, Testbed};
use linklet_core::{
    CheckError, DEFAULT_BUDGET_SECONDS, ExitCode, MAX_TARGETS, Report, Summary, ToolOutcome,
    ToolRunner, check_targets, exit_code_for, parse_targets, render,
};

/// The usage text, printed for `--help` and for a wrong invocation.
///
/// Kept to what a caller needs to act: no feature list, no marketing, no
/// examples that will be out of date within a release.
const USAGE: &str = "\
linklet -- check whether machines on a LAN are listening

usage:
  linklet check [options] <target>[,<target>...]
  linklet testbed check <spec-file> <target>
  linklet mcp

target:
  host:port            for example 10.0.0.5:8787
  several may be given, separated by commas or by spaces

options for check:
  --timeout <seconds>     how long to wait for each target (default 5)
  --max-targets <count>   refuse a run larger than this (default 256)
  -h, --help              print this

about mcp:
  Speaks the Model Context Protocol on stdin and stdout, for an AI agent to
  call. See docs/MCP.md.
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
    // Checked before the option parser, because these take no options and the
    // parser would reject every one a caller might pass. A subcommand is the
    // first argument or it is not a subcommand.
    if arguments.first().map(String::as_str) == Some("mcp") {
        if arguments.len() > 1 {
            eprintln!("linklet: mcp takes no arguments");
            return ExitCode::USAGE;
        }
        return run_mcp();
    }

    if arguments.first().map(String::as_str) == Some("testbed") {
        return run_testbed(&arguments[1..]);
    }

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
    match check_specs(&options.specs, options.budget, options.max_targets) {
        Ok(reports) => {
            for report in &reports {
                println!("{}", render(report));
            }
            exit_code_for(&reports)
        }
        Err(failure) => {
            // Printed with the numbers as the core words them, so there is one
            // place that decides what "900 targets, at most 256" is called.
            eprintln!("linklet: {failure}");
            failure.exit_code()
        }
    }
}

/// The whole of a check, from text to reports.
///
/// Two callers now: the `check` command and the `check` MCP tool. They differ
/// only in how they print and in what they return, so the parsing and the run
/// live here rather than being written twice -- two copies of "what a target is"
/// would eventually disagree.
///
/// # Errors
///
/// [`CliError`] for a spec that does not parse, [`CheckError`] for a run that is
/// refused before it starts. The two are kept apart because the first blames the
/// argument and the second blames the request.
fn check_specs(
    specs: &str,
    budget: Duration,
    max_targets: usize,
) -> Result<Vec<Report>, CheckFailure> {
    let targets = parse_targets(specs).map_err(|error| CheckFailure::BadSpec(error.to_string()))?;
    check_targets(&TcpProbe, &targets, budget, max_targets).map_err(CheckFailure::Refused)
}

/// Why a check did not produce reports.
enum CheckFailure {
    /// A target specification did not parse.
    BadSpec(String),
    /// The run was refused before anything was probed.
    Refused(CheckError),
}

impl CheckFailure {
    /// The exit code this failure produces.
    ///
    /// The whole reason the two variants are kept apart: a bad spec is a bad
    /// invocation, and a refused run is a refused run. An agent branches on that
    /// difference, so it is decided in one place instead of at each call site.
    fn exit_code(&self) -> u8 {
        match self {
            Self::BadSpec(_) => ExitCode::USAGE,
            // Every `CheckError` is a refusal. The match is exhaustive, so a
            // future variant that means something else stops this compiling
            // rather than silently returning the wrong code.
            Self::Refused(
                CheckError::NoTargets | CheckError::TooManyTargets { .. } | CheckError::ZeroLimit,
            ) => ExitCode::REFUSED,
        }
    }
}

impl std::fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSpec(problem) => f.write_str(problem),
            Self::Refused(error) => write!(f, "{error}"),
        }
    }
}

/// The line format the MCP `check` tool replies with.
///
/// Deliberately the same facts as [`render`] and a different layout: the command
/// line prints one line per target because a person reads it, and the tool wants
/// a count at the end because an agent acts on it. Both are pinned by tests, so
/// neither drifts silently.
fn tool_text(reports: &[Report]) -> String {
    let mut out: Vec<String> = reports.iter().map(render).collect();
    let summary = Summary::of(reports);
    out.push(format!("{} of {} live", summary.alive, summary.total()));
    out.join("\n")
}

/// The capabilities, backed by a real machine and a real filesystem.
///
/// The binary's implementation of the core's [`ToolRunner`]. Everything it does
/// is I/O; everything it decides is in `check_specs` or in `linklet_core::testbed`,
/// which is why the tool surface has twenty tests and this has none of its own
/// beyond the sessions in `tests/mcp_session.rs`.
#[derive(Debug, Clone, Copy, Default)]
struct LiveRunner;

impl ToolRunner for LiveRunner {
    fn reachability(&self, targets: &str, budget: Duration) -> ToolOutcome {
        match check_specs(targets, budget, MAX_TARGETS) {
            Ok(reports) => ToolOutcome::ok(tool_text(&reports)),
            // A target that does not parse is a failed call: the agent asked for
            // something this tool cannot interpret, and it has to change the
            // request. Bad news about a machine is the other case entirely.
            Err(CheckFailure::BadSpec(problem)) => {
                ToolOutcome::failed(format!("the targets do not parse: {problem}"))
            }
            Err(CheckFailure::Refused(error)) => {
                ToolOutcome::failed(format!("the run was refused: {error}"))
            }
        }
    }

    fn testbed(&self, spec_path: &str, target: &str) -> ToolOutcome {
        let Ok(text) = std::fs::read_to_string(spec_path) else {
            // The path was already checked to be inside the working tree, so a
            // failure here is a file that is genuinely not there -- which the
            // agent fixes by writing it, not by retrying.
            return ToolOutcome::failed(format!("cannot read {spec_path}"));
        };

        let Ok(testbed) = Testbed::parse(&text) else {
            let problem = Testbed::parse(&text).expect_err("just failed");
            return ToolOutcome::failed(format!("{spec_path}: {problem}"));
        };

        let verdicts = testbed.check(&SystemProber);
        ToolOutcome::ok(testbed::render(&testbed, &verdicts, target))
    }
}

/// Runs the MCP server on stdio until the client closes it.
///
/// Returns the exit code. `serve` reports an I/O failure as an `Err`, and a
/// broken pipe is the normal way this ends -- the client exits and stops reading
/// -- so it is reported on stderr rather than as a crash.
fn run_mcp() -> u8 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match serve(stdin.lock(), stdout.lock(), &LiveRunner) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("linklet: mcp session ended: {error}");
            ExitCode::NOT_ALL_ALIVE
        }
    }
}

/// Whether a machine matches a testbed specification.
///
/// The answer is a decision rather than an opinion, and the exit code carries it
/// so that an agent branches without reading anything: `0` every requirement was
/// observed to hold, `1` at least one did not, `2` the invocation or the
/// specification was wrong, `3` the run was refused.
///
/// The report goes to stdout even when it is bad news: "this machine is not
/// ready" is a result, and an agent that has to merge two streams to reconstruct
/// it will eventually not bother.
fn run_testbed(arguments: &[String]) -> u8 {
    let mut words = arguments.iter();
    let command = words.next().map(String::as_str);

    if command != Some("check") {
        match command {
            None => eprintln!("linklet: usage: linklet testbed check <spec-file> <target>"),
            Some(other) => {
                eprintln!("linklet: unknown testbed command {other:?}; expected check");
            }
        }
        return ExitCode::USAGE;
    }

    let Some(spec_path) = words.next() else {
        eprintln!("linklet: testbed check needs a specification file");
        return ExitCode::USAGE;
    };
    let Some(target) = words.next() else {
        eprintln!("linklet: testbed check needs a target name, even if only a label");
        return ExitCode::USAGE;
    };
    if words.next().is_some() {
        eprintln!("linklet: testbed check takes a specification file and a target, nothing else");
        return ExitCode::USAGE;
    }

    let text = match std::fs::read_to_string(spec_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("linklet: cannot read {spec_path}: {error}");
            return ExitCode::USAGE;
        }
    };

    let testbed = match Testbed::parse(&text) {
        Ok(testbed) => testbed,
        Err(error) => {
            // The specification's own line number, so the fix is a one-line edit
            // rather than a search through the file.
            eprintln!("linklet: {spec_path}: {error}");
            return ExitCode::USAGE;
        }
    };

    let verdicts = testbed.check(&SystemProber);
    println!("{}", testbed::render(&testbed, &verdicts, target));

    if verdicts.iter().all(|verdict| verdict.held) {
        ExitCode::SUCCESS
    } else {
        ExitCode::NOT_ALL_ALIVE
    }
}
