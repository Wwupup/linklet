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
use linklet_client::{AgentAddress, render_call_error};
use linklet_core::auth::Token;
use linklet_core::testbed::{self, Testbed};
use linklet_core::wire::{self, KillRequest, RunRequest};
use linklet_core::{
    CheckError, DEFAULT_BUDGET_SECONDS, DEFAULT_EXEC_TIMEOUT_SECONDS, ExitCode, MAX_AT_ONCE,
    MAX_TARGETS, Report, Summary, ToolOutcome, ToolRunner, check_targets_concurrent, exit_code_for,
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
  linklet testbed check <spec-file> <target>
  linklet exec --agent <host:port> [options] <command...>
  linklet ps --agent <host:port> [options]
  linklet kill --agent <host:port> (--pid <n> | --name <exact> | --contains <text>) [options]
  linklet push --agent <host:port> --from <local> --to <remote>
  linklet pull --agent <host:port> --from <remote> --to <local>
  linklet mcp

target:
  host:port            for example 10.0.0.5:8787
  several may be given, separated by commas or by spaces

options for check:
  --timeout <seconds>     how long to wait for each target (default 5)
  --max-targets <count>   refuse a run larger than this (default 256)
  -h, --help              print this

about transfers:
  A path under the agent's transfer root -- the directory it was started with, or
  --root on the agent. One file per command, and a directory is your own loop.

options for ps:
  --name <text>       keep processes whose image name contains this
  --cmdline <text>    keep processes whose command line contains this
  --query <text>      keep processes matching this in any readable field
  --exclude <text>    drop processes whose name or command line contains this

about the listing:
  One line per process, `pid name`, then a summary saying how many of how many
  matched and what filter was applied. An empty list is only readable next to that
  summary, which is why it is always printed -- and exit 1 rather than 0 when the
  machine could not be read completely.

options for kill:
  --pid <n>           stop this one process
  --name <text>       stop every process whose image name is exactly this
  --contains <text>   stop every process whose image name contains this
  --yes               required for --name and --contains: they can match several
  --exclude <text>    do not stop processes whose image name contains this
  --candidates-name <text>     only consider processes matching this first
  --candidates-cmdline <text>  only consider processes whose command line matches

about stopping things:
  A pid is never gated, because a number is one process. A name is, because a build
  and its helper often share one. Stopping the agent itself is refused on the target
  and the refusal names the process -- filtering it out here would report success
  while the one process the caller named kept running. Exit 1 means something the
  request matched is still running.

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

    if arguments.first().map(String::as_str) == Some("exec") {
        return run_exec(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("push") {
        return run_transfer(Direction::Push, &arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("pull") {
        return run_transfer(Direction::Pull, &arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("ps") {
        return run_ps(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("kill") {
        return run_kill(&arguments[1..]);
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

    // The concurrent run, not the serial one. Both are tested and both agree;
    // this is the one whose waits overlap, and ten unreachable machines taking
    // one timeout instead of ten is the whole point of the milestone.
    check_targets_concurrent(&TcpProbe, &targets, budget, max_targets, MAX_AT_ONCE)
        .map_err(CheckFailure::Refused)
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
            // rather than silently returning the wrong code -- which is exactly
            // what happened when `ZeroAtOnce` was added, and the reason the
            // pattern is spelled out instead of written as `Refused(_)`.
            Self::Refused(
                CheckError::NoTargets
                | CheckError::TooManyTargets { .. }
                | CheckError::ZeroLimit
                | CheckError::ZeroAtOnce,
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

    fn exec(&self, agent: &str, command: &str, timeout_seconds: u64) -> ToolOutcome {
        // The MCP server reads the token from the environment once at startup;
        // see `run_mcp`. A tool argument would put the secret in the conversation.
        exec_on(
            agent,
            command,
            timeout_seconds,
            token_from_environment().as_ref(),
        )
    }

    fn push(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        transfer_on(
            Direction::Push,
            agent,
            from,
            to,
            token_from_environment().as_ref(),
        )
    }

    fn pull(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        transfer_on(
            Direction::Pull,
            agent,
            from,
            to,
            token_from_environment().as_ref(),
        )
    }

    fn ps(&self, agent: &str, filter: &linklet_core::process::Filter) -> ToolOutcome {
        ps_on(agent, filter, token_from_environment().as_ref())
    }

    fn kill(&self, agent: &str, request: &KillRequest) -> ToolOutcome {
        kill_on(agent, request, token_from_environment().as_ref())
    }
}

/// Runs a command on an agent and renders what it did.
///
/// Shared by the `exec` command and the `exec` tool, so that the two cannot
/// disagree about what a failure looks like. The distinction it keeps intact is
/// the one the protocol was built around: a call that could not be made is
/// `is_error`, and a call that was made and went badly is a result carrying bad
/// news.
fn exec_on(agent: &str, command: &str, timeout_seconds: u64, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    let request = RunRequest {
        command: command.to_string(),
        timeout_seconds,
    };

    match linklet_client::run(&address, &request) {
        Ok(outcome) => ToolOutcome::ok(wire::render_run(&outcome)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
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

/// Runs a command on an agent and prints what it did.
///
/// The exit code is the command's own when there is one, and it is worth being
/// explicit about why: a caller in a shell script wants `linklet exec ... && next`
/// to behave the way the command itself would. Anything that means the command
/// did not run gets [`ExitCode::REFUSED`], a code no command can produce, so a
/// script can tell "it ran and failed" from "it never ran" without reading a word
/// of output.
fn run_exec(arguments: &[String]) -> u8 {
    let mut agent: Option<String> = None;
    let mut timeout = DEFAULT_EXEC_TIMEOUT_SECONDS;
    let mut words: Vec<String> = Vec::new();
    let mut iterator = arguments.iter();

    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--agent" => match iterator.next() {
                Some(value) => agent = Some(value.clone()),
                None => {
                    eprintln!("linklet: --agent needs a host:port");
                    return ExitCode::USAGE;
                }
            },
            "--timeout" => match iterator.next().and_then(|value| value.parse().ok()) {
                Some(value) => timeout = value,
                None => {
                    eprintln!("linklet: --timeout needs a number of seconds");
                    return ExitCode::USAGE;
                }
            },
            other => words.push(other.to_string()),
        }
    }

    let Some(agent) = agent else {
        eprintln!("linklet: exec needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    if words.is_empty() {
        eprintln!("linklet: exec needs a command");
        return ExitCode::USAGE;
    }

    // Joined rather than taken one word at a time, so that the command is exactly
    // what was typed after the options. A tool that reassembled a command line
    // from pieces would be a second interpretation of the caller's quoting.
    let command = words.join(" ");

    let outcome = exec_on(&agent, &command, timeout, token_from_environment().as_ref());
    println!("{}", outcome.text);

    if outcome.is_error {
        // The command never ran, so there is no exit code to pass on, and the
        // refusal code says so without ambiguity.
        ExitCode::REFUSED
    } else {
        exit_code_from_text(&outcome.text)
    }
}

/// Which way a transfer goes.
///
/// An enum rather than two near-identical functions, because the two commands take the
/// same three options and differ in one thing. Writing them out twice is how two
/// commands come to disagree about which of `--from` and `--to` is the local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Local to remote: `--from` is here and `--to` is there.
    Push,
    /// Remote to local: `--from` is there and `--to` is here.
    Pull,
}

impl Direction {
    /// The command's name, for messages.
    fn name(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Pull => "pull",
        }
    }
}

/// Runs one transfer and returns the exit code.
///
/// The exit code follows `check` rather than `exec`: a call that could not be made is a
/// **refusal**, which is exit 3 and goes to stderr, because there is no command whose
/// exit status could be passed through. A pull that arrived and a push that landed are
/// exit 0 with a line of facts on stdout.
fn run_transfer(direction: Direction, arguments: &[String]) -> u8 {
    let options = match parse_transfer(direction, arguments) {
        Ok(options) => options,
        Err(problem) => {
            eprintln!("linklet: {problem}");
            return ExitCode::USAGE;
        }
    };

    let outcome = transfer_on(
        direction,
        &options.agent,
        &options.from,
        &options.to,
        token_from_environment().as_ref(),
    );

    if outcome.is_error {
        eprintln!("linklet: {}", outcome.text);
        return ExitCode::REFUSED;
    }

    println!("{}", outcome.text);
    ExitCode::SUCCESS
}

/// The three options a transfer takes, whichever way it goes.
struct TransferOptions {
    /// The agent's `host:port`.
    agent: String,
    /// Where the file is now.
    from: String,
    /// Where it should end up.
    to: String,
}

/// Turns `argv` into the three options, or says which one is wrong.
///
/// Every flag is required and none has a default: a transfer with a guessed destination
/// is a file written somewhere nobody asked for, and this project would rather refuse.
fn parse_transfer(direction: Direction, arguments: &[String]) -> Result<TransferOptions, String> {
    let mut agent: Option<String> = None;
    let mut from: Option<String> = None;
    let mut to: Option<String> = None;

    let mut words = arguments.iter();
    while let Some(argument) = words.next() {
        let mut take = |flag: &str| -> Result<String, String> {
            words
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match argument.as_str() {
            "--agent" => agent = Some(take("--agent")?),
            "--from" => from = Some(take("--from")?),
            "--to" => to = Some(take("--to")?),
            "-h" | "--help" => {
                return Err("this command takes no --help; see linklet --help".into());
            }
            other => return Err(format!("unknown option {other:?}")),
        }
    }

    let name = direction.name();
    Ok(TransferOptions {
        agent: agent.ok_or_else(|| format!("{name} needs --agent <host:port>"))?,
        from: from.ok_or_else(|| format!("{name} needs --from"))?,
        to: to.ok_or_else(|| format!("{name} needs --to"))?,
    })
}

/// Lists what is running on an agent's machine, and prints what it found.
///
/// The exit code is the shape `check` uses rather than `exec`'s: `0` when a listing arrived
/// and was complete, `1` when it arrived and something about it could not be read, `2` for a
/// wrong invocation, and `3` when the call could not be made. **An incomplete listing is exit
/// 1 and not 3**, because the call *was* made: what went wrong is on the machine, and the
/// distinction is the one the whole exit-code scheme exists for.
fn run_ps(arguments: &[String]) -> u8 {
    let mut agent: Option<String> = None;
    let mut filter = linklet_core::process::Filter::any();
    let mut words = arguments.iter();

    while let Some(argument) = words.next() {
        let mut value = |flag: &str| -> Result<String, u8> {
            words.next().cloned().ok_or_else(|| {
                eprintln!("linklet: {flag} needs a value");
                ExitCode::USAGE
            })
        };
        match argument.as_str() {
            "--agent" => match value("--agent") {
                Ok(got) => agent = Some(got),
                Err(code) => return code,
            },
            "--name" => match value("--name") {
                Ok(got) => filter.name = Some(got),
                Err(code) => return code,
            },
            "--cmdline" => match value("--cmdline") {
                Ok(got) => filter.cmdline = Some(got),
                Err(code) => return code,
            },
            "--query" => match value("--query") {
                Ok(got) => filter.query = Some(got),
                Err(code) => return code,
            },
            "--exclude" => match value("--exclude") {
                Ok(got) => filter.exclude = Some(got),
                Err(code) => return code,
            },
            other => {
                eprintln!("linklet: unknown option {other:?}; see linklet --help");
                return ExitCode::USAGE;
            }
        }
    }

    let Some(agent) = agent else {
        eprintln!("linklet: ps needs --agent <host:port>");
        return ExitCode::USAGE;
    };

    let outcome = ps_on(&agent, &filter, token_from_environment().as_ref());
    println!("{}", outcome.text);

    if outcome.is_error {
        // The call could not be made, so there is no listing and no count to branch on.
        // Printed to stdout by the line above because the same text is the tool's whole
        // answer, and a caller that has to merge two streams to reconstruct one will not.
        return ExitCode::REFUSED;
    }

    match complete_from_text(&outcome.text) {
        true => ExitCode::SUCCESS,
        false => ExitCode::NOT_ALL_ALIVE,
    }
}

/// Whether a rendered listing says the answer is the whole truth.
///
/// A small parse of a format this project owns, the same way `exit_code_from_text` is, and
/// for the same reason: the alternative is threading the [`linklet_core::process::Listing`]
/// through the renderer as well as through the text, which puts one fact in two parameters.
/// The format is pinned by tests in `linklet_core::process`, so a change that breaks this
/// parser breaks those first.
///
/// The markers are the two the renderer writes: a summary that could not read the process
/// list, and a count of lines that could not be read.
fn complete_from_text(text: &str) -> bool {
    let summary = text.lines().next().unwrap_or_default();

    !summary.contains("could not be read")
        && !summary.contains("unreadable")
        && !text.contains("\nnote: ")
}

/// Stops something on an agent's machine.
///
/// # The two things this will not do without being asked twice
///
/// `--name` and `--contains` can match more than one process, so they need `--yes`: a build
/// and its helper often share a name, and killing both on a typo is an act that cannot be
/// undone. `--pid` never needs it, because a number is one process.
///
/// **Stopping the agent itself is refused on the machine, not here**, and the refusal names
/// the process. That is not politeness: if this side filtered it out instead, the caller
/// would read a report saying everything else was killed and never learn that the one
/// process it named is still running.
///
/// The exit code is `check`'s and not `exec`'s -- there is no command whose status could be
/// passed through. `0` when everything the request matched is gone, `1` when something is
/// still there, `3` when the call could not be made or was refused.
fn run_kill(arguments: &[String]) -> u8 {
    let mut agent: Option<String> = None;
    let mut to_kill: Option<linklet_core::process::ToKill> = None;
    let mut force = false;
    let mut exclude: Option<String> = None;
    let mut candidates = linklet_core::process::Filter::any();
    let mut words = arguments.iter();

    while let Some(argument) = words.next() {
        let mut value = |flag: &str| -> Result<String, u8> {
            words.next().cloned().ok_or_else(|| {
                eprintln!("linklet: {flag} needs a value");
                ExitCode::USAGE
            })
        };
        let mut set_target = |target: linklet_core::process::ToKill| -> Result<(), u8> {
            if to_kill.is_some() {
                eprintln!("linklet: kill takes one of --pid, --name or --contains, not several");
                return Err(ExitCode::USAGE);
            }
            to_kill = Some(target);
            Ok(())
        };

        match argument.as_str() {
            "--agent" => match value("--agent") {
                Ok(got) => agent = Some(got),
                Err(code) => return code,
            },
            "--pid" => {
                let Ok(text) = value("--pid") else {
                    return ExitCode::USAGE;
                };
                let Ok(pid) = text.parse::<u32>() else {
                    eprintln!("linklet: --pid needs a process identifier, got {text:?}");
                    return ExitCode::USAGE;
                };
                if let Err(code) = set_target(linklet_core::process::ToKill::Pid(pid)) {
                    return code;
                }
            }
            "--name" => {
                let Ok(text) = value("--name") else {
                    return ExitCode::USAGE;
                };
                if let Err(code) = set_target(linklet_core::process::ToKill::Name(text)) {
                    return code;
                }
            }
            "--contains" => {
                let Ok(text) = value("--contains") else {
                    return ExitCode::USAGE;
                };
                if let Err(code) = set_target(linklet_core::process::ToKill::Matching(text)) {
                    return code;
                }
            }
            "--exclude" => match value("--exclude") {
                Ok(got) => exclude = Some(got),
                Err(code) => return code,
            },
            "--candidates-name" => match value("--candidates-name") {
                Ok(got) => candidates.name = Some(got),
                Err(code) => return code,
            },
            "--candidates-cmdline" => match value("--candidates-cmdline") {
                Ok(got) => candidates.cmdline = Some(got),
                Err(code) => return code,
            },
            "--yes" => force = true,
            other => {
                eprintln!("linklet: unknown option {other:?}; see linklet --help");
                return ExitCode::USAGE;
            }
        }
    }

    let Some(agent) = agent else {
        eprintln!("linklet: kill needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    let Some(to_kill) = to_kill else {
        eprintln!("linklet: kill needs one of --pid, --name or --contains");
        return ExitCode::USAGE;
    };

    let request = KillRequest {
        to_kill,
        force,
        candidates,
        exclude,
    };

    let outcome = kill_on(&agent, &request, token_from_environment().as_ref());
    println!("{}", outcome.text);

    if outcome.is_error {
        return ExitCode::REFUSED;
    }

    // A report that says something is still there is exit 1: the call was made and the
    // machine has not done what was asked. `killed 0 of 1` is read by `complete_from_kill`
    // rather than by counting lines, for the same reason the listing's summary is.
    match kill_complete_from_text(&outcome.text) {
        true => ExitCode::SUCCESS,
        false => ExitCode::NOT_ALL_ALIVE,
    }
}

/// Whether a rendered kill says everything it matched is gone.
///
/// The first line is `killed <n> of <m>`, so a report where the two numbers differ is a
/// report about something still running. Read out of the text rather than out of the
/// [`linklet_core::process::KillReport`], which `kill_on` has already rendered, for the same
/// reason `exit_code_from_text` is: the alternative is threading one fact through two
/// parameters.
fn kill_complete_from_text(text: &str) -> bool {
    let Some(first) = text.lines().next() else {
        return false;
    };
    let Some(rest) = first.strip_prefix("killed ") else {
        return false;
    };
    let Some((killed, matched)) = rest.split_once(" of ") else {
        return false;
    };

    match (
        killed.trim().parse::<usize>(),
        matched.trim().parse::<usize>(),
    ) {
        // Only when both parsed and agree. An unreadable line is not evidence that
        // something is gone, so it is exit 1 -- the same direction every other answer in
        // this tool takes when it cannot tell.
        (Ok(killed), Ok(matched)) => killed == matched,
        _ => false,
    }
}

/// Stops something on an agent's machine and renders what happened.
///
/// Shared by the command and the tool, like `ps_on`, so that the two cannot disagree about
/// what a refusal looks like.
fn kill_on(agent: &str, request: &KillRequest, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::kill(&address, request) {
        Ok(report) => ToolOutcome::ok(linklet_core::process::render_kill(&report)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// Asks an agent what is running and renders what it said.
/// Shared by the command and the tool, like `exec_on` and `transfer_on`, so that the two
/// cannot disagree about what a failure looks like.
fn ps_on(
    agent: &str,
    filter: &linklet_core::process::Filter,
    token: Option<&Token>,
) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::ps(&address, filter) {
        Ok(listing) => ToolOutcome::ok(linklet_core::process::render(&listing)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// Moves one file and renders what happened.
/// Shared by the command and the tool, so that the two cannot disagree about what a
/// failure looks like -- the same shape [`exec_on`] has. The distinction it keeps is the
/// one this project is arranged around: a call that could not be made is `is_error`, and
/// a transfer that arrived with the wrong digest is a result carrying bad news.
fn transfer_on(
    direction: Direction,
    agent: &str,
    from: &str,
    to: &str,
    token: Option<&Token>,
) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    let result = match direction {
        // `from` is the local path on a push, and `to` is the remote one.
        Direction::Push => linklet_client::push(&address, std::path::Path::new(from), to),
        Direction::Pull => linklet_client::pull(&address, from, std::path::Path::new(to)),
    };

    match result {
        Ok(outcome) => ToolOutcome::ok(wire::render_transfer(&outcome, to)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// The token this host presents, from the environment.
///
/// The environment rather than a flag, so that the secret does not appear in a
/// process listing or a shell history. `LINKLET_TOKEN` is the same variable the
/// agent reads, so a bench with both ends on one machine needs it set once.
///
/// An unusable token is reported and treated as absent rather than refused: the
/// caller finds out from the agent refusing the session, which is the same thing
/// that happens when it is wrong, and one message for one problem is better than two.
fn token_from_environment() -> Option<Token> {
    let secret = std::env::var("LINKLET_TOKEN").ok()?;
    match Token::new(secret) {
        Ok(token) => Some(token),
        Err(error) => {
            eprintln!("linklet: LINKLET_TOKEN is unusable: {error}");
            None
        }
    }
}

/// The exit code a rendered run reports, or success when it reports none.
///
/// A small parse of a format this project owns, which is normally a smell -- but
/// the alternative is returning the outcome as well as rendering it, and that
/// would put the same fact in two parameters. The format is pinned by
/// `linklet_core::wire`'s tests, so a change that breaks this parser breaks those
/// first.
fn exit_code_from_text(text: &str) -> u8 {
    let Some(first) = text.lines().next() else {
        return ExitCode::SUCCESS;
    };
    let Some(rest) = first.strip_prefix("exit ") else {
        // "no exit code: <reason>" -- the command was killed, and a killed
        // command's status is a failure by any reading.
        return ExitCode::NOT_ALL_ALIVE;
    };
    // Truncated to the low byte, because a process exit code is a byte: a command
    // that exits 256 is a command that exited 0 on this platform, and pretending
    // otherwise would report a failure the operating system does not.
    rest.trim().parse::<i32>().unwrap_or(1).rem_euclid(256) as u8
}
