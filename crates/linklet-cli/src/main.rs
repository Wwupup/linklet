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

mod probe;

use std::env;
use std::process::ExitCode as ProcessExit;
use std::time::Duration;

use linklet_adapters::{SystemProber, TcpProbe, serve};
use linklet_client::{AgentAddress, render_call_error};
use linklet_core::arguments::{Args, Flag, Strictness};
use linklet_core::auth::Token;
use linklet_core::testbed::{self, Testbed};
use linklet_core::wire::{
    self, GrepRequest, KillRequest, LsRequest, RunRequest, SpawnRequest, TailRequest,
};
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
  linklet exec --agents <a,b,c> [options] <command...>
  linklet ps --agent <host:port> [options]
  linklet kill --agent <host:port> (--pid <n> | --name <exact> | --contains <text>) [options]
  linklet spawn --agent <host:port> --output <remote> <command...>
  linklet grep --agent <host:port> --from <remote> --pattern <text> [options]
  linklet tail --agent <host:port> --from <remote> [--lines <n>]
  linklet ls --agent <host:port> --from <remote>
  linklet probe --agent <host:port>
  linklet discover [--port <port>] [--networks] [--targets]
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

about reading a file on a target:
  grep and tail read one file under the agent's transfer root without moving it, which
  is what a two-gigabyte log needs and a pull cannot do. The first line is a summary:
  how many matches, which file, and which encoding the bytes were read as -- because a
  file that could not be read must never look like a file with no matches. Exit 1 means
  the answer is incomplete: the search stopped early, or the file was cut short at the
  byte ceiling.

options for grep:
  --pattern <text>    the text to find, passed as an argument and never through a shell
  -i, --ignore-case   match without regard to case (the default is to care)
  --first, --last     search from the start (default) or the end, which is what the
                      question where is the last ERROR wants
  --max <n>           the most matches to report (default 12)
  --context <n>       lines to show on each side of a match (default 0)

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
    /// A flag that takes a number did not get one.
    ///
    /// **`MissingValue` used to live here too and is gone**, because there is one sentence for a
    /// missing value now and `linklet_core::arguments` owns it. A variant that only wrapped
    /// somebody else's message would be a layer with no decision in it -- which is the argument
    /// this enum's own doc comment made for not having one in the first place.
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
            Self::NotANumber { flag, value } => {
                write!(f, "{flag} needs a number, got {value:?}")
            }
            Self::Usage(message) => write!(f, "{message}"),
        }
    }
}

/// Reads a command's flags, or reports the first thing wrong with them.
///
/// **The bridge between a pure parser and a process.** `linklet_core::arguments` decides what is
/// wrong and says so without knowing about exit codes or streams, which is what makes it
/// testable in microseconds; this turns that into the sentence a caller reads and the code a
/// script branches on, which is the only part that needs to know either.
fn read(arguments: &[String], flags: &[Flag], strictness: Strictness) -> Result<Args, u8> {
    Args::parse(arguments, flags, strictness).map_err(|error| {
        eprintln!("linklet: {error}; see linklet --help");
        ExitCode::USAGE
    })
}

/// Reads a number out of a flag, or says what arrived instead.
///
/// **What the caller typed is echoed**, because "needs a number" without the value sends a
/// reader back to their own command line to work out what was wrong with it. `None` means the
/// flag was not given, which is different from a flag given badly -- the caller decides what an
/// absent flag defaults to.
fn number(args: &Args, flag: &str) -> Result<Option<u64>, u8> {
    let Some(text) = args.value(flag) else {
        return Ok(None);
    };
    match text.parse::<u64>() {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            eprintln!("linklet: {flag} needs a number, got {text:?}");
            Err(ExitCode::USAGE)
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

    if arguments.first().map(String::as_str) == Some("spawn") {
        return run_spawn(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("grep") {
        return run_grep(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("tail") {
        return run_tail(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("ls") {
        return run_ls(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("discover") {
        return run_discover(&arguments[1..]);
    }

    if arguments.first().map(String::as_str) == Some("probe") {
        return run_probe_command(&arguments[1..]);
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
    let flags = [
        Flag::takes_value("--timeout"),
        Flag::takes_value("--max-targets"),
        Flag::switch("-h"),
        Flag::switch("--help"),
    ];
    let args = Args::parse(arguments, &flags, Strictness::AllowWords)
        .map_err(|error| CliError::Usage(error.to_string()))?;

    // `-h` before anything else, including before the verb is checked: a caller who asked for
    // help gets help and not a complaint about a missing command.
    if args.switch("-h") || args.switch("--help") {
        return Ok(None);
    }

    // The words are the verb and its targets, and the verb is checked rather than assumed. It
    // is read out of the words rather than from `arguments[0]` because `linklet --timeout 5
    // check a:1` has to keep working, and it did.
    let mut words = args.words().iter();
    match words.next().map(String::as_str) {
        Some("check") => {}
        _ => {
            return Err(CliError::Usage(
                "no command given; expected `check`".to_string(),
            ));
        }
    }
    let specs: Vec<String> = words.cloned().collect();

    let budget_seconds = match args.value("--timeout") {
        Some(text) => text.parse().map_err(|_| CliError::NotANumber {
            flag: "--timeout".to_string(),
            value: text.to_string(),
        })?,
        None => DEFAULT_BUDGET_SECONDS,
    };
    let max_targets = match args.value("--max-targets") {
        Some(text) => text.parse().map_err(|_| CliError::NotANumber {
            flag: "--max-targets".to_string(),
            value: text.to_string(),
        })?,
        None => MAX_TARGETS,
    };

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

    fn spawn(&self, agent: &str, request: &SpawnRequest) -> ToolOutcome {
        spawn_on(agent, request, token_from_environment().as_ref())
    }

    fn grep(&self, agent: &str, request: &GrepRequest) -> ToolOutcome {
        grep_on(agent, request, token_from_environment().as_ref())
    }

    fn tail(&self, agent: &str, request: &TailRequest) -> ToolOutcome {
        tail_on(agent, request, token_from_environment().as_ref())
    }

    fn ls(&self, agent: &str, request: &LsRequest) -> ToolOutcome {
        ls_on(agent, request, token_from_environment().as_ref())
    }
}

/// Runs a command on one agent and says what happened, before it is rendered.
///
/// # Errors
///
/// The rendered sentence for anything that leaves the command's fate unknown -- an agent that
/// could not be reached, or one that answered with something unreadable. **A command that ran
/// is never an error**, however badly it went: that is the distinction the protocol was built
/// around, and the one a fan-out has to preserve across several machines.
fn exec_at(
    agent: &str,
    command: &str,
    timeout_seconds: u64,
    token: Option<&Token>,
) -> Result<wire::RunOutcome, String> {
    let mut address = AgentAddress::new(agent).map_err(|error| render_call_error(&error))?;
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    let request = RunRequest {
        command: command.to_string(),
        timeout_seconds,
    };

    linklet_client::run(&address, &request).map_err(|error| render_call_error(&error))
}

/// Runs one command across several agents and reports each of them.
///
/// **This is the half of `docs/ROADMAP.md` M10's discovery item that is a decision rather than
/// a loop**, and the decisions are in `linklet_core::fanout`: every target gets a line in the
/// order the caller gave them however they finish, one machine failing does not stop the
/// others, and a machine that refused is told apart from one that could not be reached. What is
/// here is only the part that needs a socket.
///
/// # The work happens once
///
/// A fan-out that ran the command and then ran it again to render the output would be a tool
/// that executes a build twice on four machines, which is worse than any answer it could give.
/// So the closure keeps what it rendered in a slot of its own, and the report and the output
/// are read out of the same run.
///
/// # The exit code
///
/// `check`'s rather than `exec`'s: `0` when every agent the command was sent to ran it, `1`
/// when one of them did not, `3` when the invocation was wrong. **A command that ran and
/// exited non-zero is still `0`** -- the same rule the single-target path follows, because the
/// command's own status is the caller's business and the agent's reachability is this tool's.
fn run_exec_across(
    agents: &[String],
    command: &str,
    timeout_seconds: u64,
    token: Option<&Token>,
) -> u8 {
    use linklet_core::fanout::{Fate, Task};

    if agents.is_empty() {
        eprintln!("linklet: --agents needs at least one host:port");
        return ExitCode::USAGE;
    }
    if agents.len() > linklet_core::fanout::MAX_TARGETS {
        // Refused rather than cut, which is the opposite of the scan's ceiling: a scan's list
        // is generated and a caller cannot know how long it is, and this list was typed.
        eprintln!(
            "linklet: {} agents is more than the {} one run will take",
            agents.len(),
            linklet_core::fanout::MAX_TARGETS
        );
        return ExitCode::USAGE;
    }

    // One slot per agent, holding what that agent's run rendered. Filled by the worker that
    // ran it, read afterwards in the caller's order.
    //
    // An `Arc` because there is one closure per agent and each has to own a handle to the
    // slots rather than borrowing the one the others also need.
    let rendered: std::sync::Arc<std::sync::Mutex<Vec<Option<String>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(vec![None; agents.len()]));

    let tasks: Vec<Task> = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            let agent = agent.clone();
            let command = command.to_string();
            let token = token.cloned();
            let rendered = std::sync::Arc::clone(&rendered);
            Task::new(agent.clone(), move || {
                match exec_at(&agent, &command, timeout_seconds, token.as_ref()) {
                    Ok(outcome) => {
                        if let Ok(mut rendered) = rendered.lock()
                            && let Some(slot) = rendered.get_mut(index)
                        {
                            *slot = Some(wire::render_run(&outcome));
                        }
                        // The command ran. Whether it went well is the command's business and
                        // not this fan-out's, which is the distinction the protocol is built on.
                        Fate::Ran
                    }
                    // The agent answered and said no, which is a fact about the machine.
                    Err(problem) if problem.contains("refused") => Fate::Refused,
                    // Anything else is a machine no answer could be got out of.
                    Err(_) => Fate::Unreachable,
                }
            })
        })
        .collect();

    let report = linklet_core::fanout::run(tasks);

    // The fan-out's summary, then one labelled block per agent. The blocks are the expensive
    // part and they are what a caller actually reads, so they are printed rather than left
    // behind in a struct -- and the summary is first because "did they all work" is the first
    // question.
    println!("{} of {} ran", report.ran(), report.total);
    let rendered = rendered
        .lock()
        .map(|slots| slots.clone())
        .unwrap_or_default();
    for outcome in &report.outcomes {
        match outcome.fate {
            Fate::Ran => {
                println!("ran {}", outcome.target);
                if let Some(text) = agents
                    .iter()
                    .position(|agent| agent == &outcome.target)
                    .and_then(|index| rendered.get(index).cloned().flatten())
                {
                    for line in text.lines() {
                        println!("  {line}");
                    }
                }
            }
            fate => println!("{} {}", fate.as_str(), outcome.target),
        }
    }

    if report.failures().is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::NOT_ALL_ALIVE
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
    match exec_at(agent, command, timeout_seconds, token) {
        Ok(outcome) => ToolOutcome::ok(wire::render_run(&outcome)),
        Err(problem) => ToolOutcome::failed(problem),
    }
}

/// Probes one agent and prints which of the three states it is in.
///
/// A thin command over [`probe::run_probe`], which carries the reasoning and the exit codes. It
/// is its own subcommand rather than a flag on `check` because the two ask different questions
/// and a script has to tell them apart: `check` says whether a socket is there, and this says
/// whether the agent behind it is working.
fn run_probe_command(arguments: &[String]) -> u8 {
    let flags = [Flag::takes_value("--agent")];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: probe needs --agent <host:port>");
        return ExitCode::USAGE;
    };

    probe::run_probe(agent, token_from_environment().as_ref())
}

/// Runs the MCP server on stdio until the client closes it.
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
    // **`AllowWords` because this ends in a command**, and the command may have flags of its
    // own: `linklet exec --agent a:1 myapp.cmd --verbose` has to be possible. Everything in the
    // table is this tool's; everything else is the command's.
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--agents"),
        Flag::takes_value("--timeout"),
    ];
    let args = match read(arguments, &flags, Strictness::AllowWords) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let timeout = match number(&args, "--timeout") {
        Ok(Some(got)) => got,
        Ok(None) => DEFAULT_EXEC_TIMEOUT_SECONDS,
        Err(code) => return code,
    };

    // One target or several, and never both: a caller that gave both has two intentions and
    // guessing between them would run a command somewhere it did not name.
    let single = args.value("--agent");
    let several = args.value("--agents");
    if single.is_some() && several.is_some() {
        eprintln!("linklet: give --agent for one machine or --agents for several, not both");
        return ExitCode::USAGE;
    }

    if several.is_some() {
        if args.words().is_empty() {
            eprintln!("linklet: exec needs a command");
            return ExitCode::USAGE;
        }
        return run_exec_across(
            &args.list("--agents"),
            &args.tail(),
            timeout,
            token_from_environment().as_ref(),
        );
    }

    let Some(agent) = single else {
        eprintln!("linklet: exec needs --agent <host:port>, or --agents for several");
        return ExitCode::USAGE;
    };
    if args.words().is_empty() {
        eprintln!("linklet: exec needs a command");
        return ExitCode::USAGE;
    }

    // Joined rather than taken one word at a time, so that the command is exactly
    // what was typed after the options. A tool that reassembled a command line
    // from pieces would be a second interpretation of the caller's quoting.
    let command = args.tail();

    let outcome = exec_on(agent, &command, timeout, token_from_environment().as_ref());
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
            // The same pointer the shared parser adds, because a caller who mistyped a flag
            // needs the same thing to read next whether the command was `push` or `ps`. It was
            // the only message in the tool without it, and the reason was that transfers had
            // their own printer -- which is the drift this refactor was about.
            eprintln!("linklet: {problem}; see linklet --help");
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
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--from"),
        Flag::takes_value("--to"),
    ];
    // Strict, and this one refuses `--help` too: the parser does not know `-h`, so it is
    // reported as an unknown option, which is a usage error and is what the previous version
    // said in its own words.
    let args =
        Args::parse(arguments, &flags, Strictness::Strict).map_err(|error| error.to_string())?;

    let name = direction.name();
    Ok(TransferOptions {
        agent: args
            .required("--agent")
            .map(str::to_string)
            .map_err(|_| format!("{name} needs --agent <host:port>"))?,
        from: args
            .required("--from")
            .map(str::to_string)
            .map_err(|_| format!("{name} needs --from"))?,
        to: args
            .required("--to")
            .map(str::to_string)
            .map_err(|_| format!("{name} needs --to"))?,
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
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--name"),
        Flag::takes_value("--cmdline"),
        Flag::takes_value("--query"),
        Flag::takes_value("--exclude"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let mut filter = linklet_core::process::Filter::any();
    // Set rather than assigned one by one, so that the flag names live in the table above and
    // nowhere else -- the drift being fixed was nine copies of the same three lines.
    filter.name = args.value("--name").map(str::to_string);
    filter.cmdline = args.value("--cmdline").map(str::to_string);
    filter.query = args.value("--query").map(str::to_string);
    filter.exclude = args.value("--exclude").map(str::to_string);

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: ps needs --agent <host:port>");
        return ExitCode::USAGE;
    };

    let outcome = ps_on(agent, &filter, token_from_environment().as_ref());
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

/// Finds machines on the networks this host is on.
///
/// # What it prints, and why in this order
///
/// The **summary first**, then one line per address that answered. The summary is the part
/// that makes the answer readable and it is not optional: a scan has a ceiling, so "nothing
/// answered" and "I tried a thousand of this network's sixty-five thousand addresses" are
/// different facts, and only one of them means the network is empty. That is the same
/// argument as `ps` and `ls`, and `docs/ROADMAP.md` M10 asks for it here too.
///
/// `--targets` prints the addresses as the one comma-separated spec `check`, `exec` and the
/// rest already take, so that discovery feeds the commands it exists for rather than being a
/// list a person retypes.
///
/// The exit code is `check`'s: `0` when the whole plan was scanned, `1` when it was cut short
/// and the answer is therefore incomplete, `2` for a wrong invocation. **An address that did
/// not answer is not a failure** -- a scan is not a census, and reporting an empty network as
/// a broken one would be the mistake this whole project is arranged against.
fn run_discover(arguments: &[String]) -> u8 {
    // **The agent's own default port, and the same number in both places for the same
    // reason the agent's `DEFAULT_PORT` is a constant**: the question this answers is "which
    // of these machines is running an agent", and a discovery default that disagreed with
    // the agent default would find nothing on a network that is full of them. It is a literal
    // here because `linklet-cli` does not depend on `linklet-agent` -- the host tool has no
    // business linking the target binary -- and `tests/cli.rs` pins the two together.
    let flags = [
        Flag::takes_value("--port"),
        Flag::switch("--networks"),
        Flag::switch("--targets"),
        Flag::switch("-h"),
        Flag::switch("--help"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // The usage, for the two ways of asking for it. `discover` is the one command that answered
    // `-h` before this parser existed, and dropping it would be a feature change dressed up as
    // a refactor -- see the note in `docs/COMMANDS.md`.
    if args.switch("-h") || args.switch("--help") {
        println!("{USAGE}");
        return ExitCode::USAGE;
    }

    // The port is parsed here rather than by the parser, and the parser has no opinion about
    // numbers: `--max needs a number` and `--port needs a port number` are different sentences
    // because the flags are different shapes, and a parser that knew about both would be a
    // parser with a type table in it.
    let mut port = 8787u16;
    if let Some(text) = args.value("--port") {
        match text.parse::<u16>() {
            Ok(value) => port = value,
            Err(_) => {
                eprintln!("linklet: --port needs a port number, got {text:?}");
                return ExitCode::USAGE;
            }
        }
    }

    let networks_only = args.switch("--networks");
    let targets = args.switch("--targets");

    if networks_only {
        return match linklet_adapters::local_interfaces() {
            Ok(interfaces) => {
                for interface in &interfaces {
                    println!("{interface}");
                }
                ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("linklet: {reason}");
                ExitCode::NOT_ALL_ALIVE
            }
        };
    }

    let plan = match linklet_adapters::plan_here() {
        Ok(plan) => plan,
        Err(reason) => {
            // The machine's interfaces could not be read, which is a fact about the call and
            // not about the network: exit 1 would say the network is empty.
            eprintln!("linklet: {reason}");
            return ExitCode::NOT_ALL_ALIVE;
        }
    };

    let result = linklet_adapters::scan(&plan, port);

    if targets {
        // **`host:port` and not a bare address.** Every other command takes a `host:port`, and
        // the port is the one this scan was looking for -- so a list of bare addresses is a
        // list nobody can paste anywhere, which is the opposite of what this flag is for. It
        // printed `192.168.100.2` until a real run fed it to `exec --agents` and got
        // "unreachable" for a machine that was sitting right there.
        println!(
            "{}",
            result
                .found
                .iter()
                .map(|found| format!("{}:{}", found.address, found.port))
                .collect::<Vec<_>>()
                .join(",")
        );
        return if result.truncated {
            ExitCode::NOT_ALL_ALIVE
        } else {
            ExitCode::SUCCESS
        };
    }

    // One summary line, then the answers. The networks and the skipped addresses are named
    // so that a reader can tell which part of the machine the scan came from -- a scan that
    // silently covered one of two adapters is a scan that missed half the answer.
    println!(
        "{} of {} addresses answered on port {port}",
        result.found.len(),
        result.tried
    );
    for found in &result.found {
        println!("{}", found.address);
    }
    if !result.networks.is_empty() {
        println!("networks: {}", result.networks.join(" "));
    }
    for skipped in &result.skipped {
        println!("skipped: {skipped}");
    }
    if result.truncated {
        println!(
            "note: the scan was cut short at its ceiling, so addresses beyond it were not tried"
        );
    }

    if result.truncated {
        ExitCode::NOT_ALL_ALIVE
    } else {
        ExitCode::SUCCESS
    }
}

/// Lists a path on an agent's machine and prints what is there.
///
/// The exit code is `check`'s: `0` when the listing arrived and the whole directory was read,
/// `1` when it arrived and something about it is incomplete -- the path was not there, or the
/// list was cut short -- and `3` when the call could not be made. **A path that is not there
/// is exit 1 and not 3**, because the call *was* made: what went wrong is on the machine, and
/// sending the reader to the network for a directory they typed wrong is the mistake the
/// exit-code scheme exists to prevent.
fn run_ls(arguments: &[String]) -> u8 {
    // **One flag table, one parser.** The reasoning is in `linklet_core::arguments`; what is
    // local is which flags this command has and which of them are required.
    let flags = [Flag::takes_value("--agent"), Flag::takes_value("--from")];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: ls needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    // A path is required rather than defaulted to the root. The root is the agent's own
    // choice and a caller that asked for "everything" would get a listing of a directory it
    // did not name, which is the shape of a guess.
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: ls needs --from <path on the target>");
        return ExitCode::USAGE;
    };

    let outcome = ls_on(
        agent,
        &LsRequest {
            path: path.to_string(),
        },
        token_from_environment().as_ref(),
    );
    println!("{}", outcome.text);

    if outcome.is_error {
        return ExitCode::REFUSED;
    }

    match listing_complete_from_text(&outcome.text) {
        true => ExitCode::SUCCESS,
        false => ExitCode::NOT_ALL_ALIVE,
    }
}

/// Whether a rendered listing is the whole truth about the directory.
///
/// The first line is the summary and it carries both facts: a path that could not be read
/// starts with "could not list", and a cut list says "stopped early". Read out of the text
/// for the same reason `search_complete_from_text` is: the alternative is threading one fact
/// through two parameters.
fn listing_complete_from_text(text: &str) -> bool {
    let summary = text.lines().next().unwrap_or_default();

    !summary.starts_with("could not list") && !summary.contains("stopped early")
}

/// Lists a path on an agent's machine and renders what is there.
///
/// Shared by the command and the tool.
fn ls_on(agent: &str, request: &LsRequest, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::ls(&address, request) {
        Ok(listing) => ToolOutcome::ok(linklet_core::listing::render(&listing)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// Searches a file on an agent's machine and prints what it found.
///
/// The exit code is `check`'s rather than `exec`'s: `0` when the search ran and the whole
/// file was looked at, `1` when it ran and stopped early, `2` for a wrong invocation, and
/// `3` when the call could not be made. **A file that could not be read is exit 1 and not
/// 3**, because the call *was* made: what went wrong is on the machine, and sending the
/// reader to the network for a path they typed wrong is the mistake the whole exit-code
/// scheme exists to prevent.
fn run_grep(arguments: &[String]) -> u8 {
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--from"),
        Flag::takes_value("--pattern"),
        Flag::takes_value("--max"),
        Flag::takes_value("--context"),
        // The short form is a switch like the long one, and both are in the table so that
        // neither is a special case the parser has to be told about.
        Flag::switch("-i"),
        Flag::switch("--ignore-case"),
        Flag::switch("--last"),
        Flag::switch("--first"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // The default is the opposite of the protocol's, and deliberately: at a command line a
    // person searching a log for `ERROR` means the upper-case one, and `-i` is the shorter
    // thing to type for the other reading.
    let case_sensitive = !(args.switch("-i") || args.switch("--ignore-case"));
    let direction = if args.switch("--last") {
        linklet_core::search::Direction::Last
    } else {
        linklet_core::search::Direction::First
    };

    let mut limit = linklet_core::search::Limit::default();
    match number(&args, "--max") {
        Ok(Some(got)) => limit.max_matches = got as usize,
        Ok(None) => {}
        Err(code) => return code,
    }
    match number(&args, "--context") {
        Ok(Some(got)) => limit.context = got as usize,
        Ok(None) => {}
        Err(code) => return code,
    }

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: grep needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: grep needs --from <path on the target>");
        return ExitCode::USAGE;
    };
    let Some(text) = args.value("--pattern") else {
        eprintln!("linklet: grep needs --pattern <text>");
        return ExitCode::USAGE;
    };

    let request = GrepRequest {
        path: path.to_string(),
        pattern: linklet_core::search::Pattern {
            text: text.to_string(),
            case_sensitive,
        },
        direction,
        limit,
    };

    report_search(grep_on(agent, &request, token_from_environment().as_ref()))
}

/// Reads the last lines of a file on an agent's machine and prints them.
///
/// The exit code follows [`run_grep`], and for the same reason.
fn run_tail(arguments: &[String]) -> u8 {
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--from"),
        Flag::takes_value("--lines"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let count = match number(&args, "--lines") {
        Ok(Some(got)) => got as usize,
        Ok(None) => 20,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: tail needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: tail needs --from <path on the target>");
        return ExitCode::USAGE;
    };

    report_search(tail_on(
        agent,
        &TailRequest {
            path: path.to_string(),
            count,
        },
        token_from_environment().as_ref(),
    ))
}

/// Prints a search and returns the exit code that goes with it.
///
/// Shared by the two commands and the two tools, so that none of them can disagree about
/// what a search that did not happen looks like.
///
/// **The text goes to stdout even when the search failed**, because it is the answer: a
/// caller that had to merge two streams to reconstruct one would eventually not bother.
fn report_search(outcome: ToolOutcome) -> u8 {
    println!("{}", outcome.text);

    if outcome.is_error {
        return ExitCode::REFUSED;
    }

    // An incomplete answer is exit 1: the call was made, the machine answered, and the
    // answer is not the whole truth. The `searched` line is what the renderer leads with,
    // so an unreadable file is the first thing a reader sees.
    match search_complete_from_text(&outcome.text) {
        true => ExitCode::SUCCESS,
        false => ExitCode::NOT_ALL_ALIVE,
    }
}

/// Whether a rendered search is the whole truth about the file.
///
/// The first line is the summary and it carries both facts: a failed search starts with
/// "could not search", and a stopped one says "stopped early". Read out of the text for the
/// same reason `kill_complete_from_text` is: the alternative is threading one fact through
/// two parameters.
fn search_complete_from_text(text: &str) -> bool {
    let summary = text.lines().next().unwrap_or_default();

    !summary.starts_with("could not search") && !summary.contains("stopped early")
}

/// Searches a file on an agent's machine and renders what it found.
///
/// Shared by the command and the tool.
fn grep_on(agent: &str, request: &GrepRequest, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::grep(&address, request) {
        Ok(search) => ToolOutcome::ok(linklet_core::search::render(&search)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// Reads the end of a file on an agent's machine and renders it.
fn tail_on(agent: &str, request: &TailRequest, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::tail(&address, request) {
        Ok(search) => ToolOutcome::ok(linklet_core::search::render(&search)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
}

/// Starts a program on an agent's machine that outlives this command.
///
/// The exit code is `exec`'s: `0` when something was started, `3` when the call could not be
/// made. There is no command status to pass through, because **this command does not wait for
/// the program** -- that is the whole of it -- so a program that exits a second later is a
/// program this command correctly reported as started.
///
/// Confirming it stayed up is the caller's next two commands, and they are the ones the
/// deploy loop is made of: `linklet ps` to look, `linklet kill` to stop it again.
fn run_spawn(arguments: &[String]) -> u8 {
    // `AllowWords` because everything after the options is the command to run, which may have
    // flags of its own. See the line `linklet_core::arguments` draws.
    let flags = [Flag::takes_value("--agent"), Flag::takes_value("--output")];
    let args = match read(arguments, &flags, Strictness::AllowWords) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: spawn needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    if args.words().is_empty() {
        eprintln!("linklet: spawn needs a command");
        return ExitCode::USAGE;
    }
    // Required rather than defaulted, and the refusal says why: a program whose output goes
    // nowhere is a program the caller cannot look at afterwards, which is the one thing this
    // feature is for. A default like `linklet-spawn.log` in the agent's working directory
    // would be a file nobody asked for in a place nobody chose.
    let Some(output) = args.value("--output") else {
        eprintln!("linklet: spawn needs --output <path on the target>: the program writes");
        eprintln!("linklet:   its own output there, which is how you read it afterwards");
        return ExitCode::USAGE;
    };

    let request = SpawnRequest {
        // The command is exactly what was typed after the options, joined the way `exec`
        // joins it: a tool that reassembled a command line from pieces would be a second
        // interpretation of the caller's quoting.
        command: args.tail(),
        output: output.to_string(),
    };

    let outcome = spawn_on(agent, &request, token_from_environment().as_ref());
    println!("{}", outcome.text);

    if outcome.is_error {
        return ExitCode::REFUSED;
    }
    ExitCode::SUCCESS
}

/// Starts a program on an agent's machine and renders what was started.
///
/// Shared by the command and the tool, like `ps_on` and `kill_on`.
fn spawn_on(agent: &str, request: &SpawnRequest, token: Option<&Token>) -> ToolOutcome {
    let mut address = match AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failed(render_call_error(&error)),
    };
    if let Some(token) = token {
        address = address.with_token(token.clone());
    }

    match linklet_client::spawn(&address, request) {
        // `started <pid>`, and the word is chosen: "started" is what this side knows, where
        // "running" would be a claim about a moment it has not looked at.
        Ok(report) => ToolOutcome::ok(format!("started {}", report.pid)),
        Err(error) => ToolOutcome::failed(render_call_error(&error)),
    }
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
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--pid"),
        Flag::takes_value("--name"),
        Flag::takes_value("--contains"),
        Flag::takes_value("--exclude"),
        Flag::takes_value("--candidates-name"),
        Flag::takes_value("--candidates-cmdline"),
        Flag::switch("--yes"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    // **Built by naming the flags once, in one place.** The three target flags are read into a
    // small list and then checked, rather than each arm checking the others: the version this
    // replaced had a closure per arm doing the same test three times.
    let mut targets: Vec<linklet_core::process::ToKill> = Vec::new();
    if let Some(text) = args.value("--pid") {
        match text.parse::<u32>() {
            Ok(pid) => targets.push(linklet_core::process::ToKill::Pid(pid)),
            Err(_) => {
                eprintln!("linklet: --pid needs a process identifier, got {text:?}");
                return ExitCode::USAGE;
            }
        }
    }
    if let Some(text) = args.value("--name") {
        targets.push(linklet_core::process::ToKill::Name(text.to_string()));
    }
    if let Some(text) = args.value("--contains") {
        targets.push(linklet_core::process::ToKill::Matching(text.to_string()));
    }

    if targets.len() > 1 {
        eprintln!("linklet: kill takes one of --pid, --name or --contains, not several");
        return ExitCode::USAGE;
    }

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: kill needs --agent <host:port>");
        return ExitCode::USAGE;
    };
    let Some(to_kill) = targets.pop() else {
        eprintln!("linklet: kill needs one of --pid, --name or --contains");
        return ExitCode::USAGE;
    };

    let mut candidates = linklet_core::process::Filter::any();
    candidates.name = args.value("--candidates-name").map(str::to_string);
    candidates.cmdline = args.value("--candidates-cmdline").map(str::to_string);

    let request = KillRequest {
        to_kill,
        force: args.switch("--yes"),
        candidates,
        exclude: args.value("--exclude").map(str::to_string),
    };

    let outcome = kill_on(agent, &request, token_from_environment().as_ref());
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
