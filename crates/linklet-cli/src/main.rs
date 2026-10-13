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

mod help;
mod probe;

use std::env;
use std::time::Duration;

use linklet_adapters::{SystemProber, TcpProbe, serve};
use linklet_client::AgentAddress;
use linklet_core::arguments::{Args, Flag, Strictness};
use linklet_core::auth::Token;
use linklet_core::result::{ErrorCode, OperationError, Status};
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
  linklet pair --agent <host:port> --code <six-digits>
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
  Remote relative paths use the agent startup directory. Absolute paths are used
  directly; access follows the agent account permissions. One file per command,
  and a directory is your own loop.

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
  grep and tail read one file on the agent without moving it, which
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
fn read(arguments: &[String], flags: &[Flag], strictness: Strictness) -> Result<Args, i32> {
    let args = Args::parse(arguments, flags, strictness).map_err(|error| {
        eprintln!("linklet: {error}; see linklet --help");
        i32::from(ExitCode::USAGE)
    })?;
    if strictness == Strictness::Strict && !args.words().is_empty() {
        eprintln!(
            "linklet: unexpected argument {:?}; see linklet --help",
            args.words()[0]
        );
        return Err(i32::from(ExitCode::USAGE));
    }
    Ok(args)
}

/// Reads a number out of a flag, or says what arrived instead.
///
/// **What the caller typed is echoed**, because "needs a number" without the value sends a
/// reader back to their own command line to work out what was wrong with it. `None` means the
/// flag was not given, which is different from a flag given badly -- the caller decides what an
/// absent flag defaults to.
fn number(args: &Args, flag: &str) -> Result<Option<u64>, i32> {
    let Some(text) = args.value(flag) else {
        return Ok(None);
    };
    match text.parse::<u64>() {
        Ok(value) => Ok(Some(value)),
        Err(_) => {
            eprintln!("linklet: {flag} needs a number, got {text:?}");
            Err(i32::from(ExitCode::USAGE))
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

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    // Windows exit statuses are 32 bits. std::process::ExitCode only accepts u8
    // on stable Rust, so exit after dispatch has returned and released its resources.
    std::process::exit(dispatch(&arguments));
}

/// Runs the tool and returns the exit code. Never panics and never exits.
///
/// Kept separate from `main` so that every branch ends in a code the tests can
/// reason about, and so that the only line that touches the process is the one
/// above.
fn dispatch(arguments: &[String]) -> i32 {
    if arguments == ["--version"] {
        println!("linklet {}", env!("CARGO_PKG_VERSION"));
        return i32::from(ExitCode::SUCCESS);
    }
    if let Some(help) = help::requested(arguments) {
        print!("{help}");
        return i32::from(ExitCode::SUCCESS);
    }
    // Checked before the option parser, because these take no options and the
    // parser would reject every one a caller might pass. A subcommand is the
    // first argument or it is not a subcommand.
    if arguments.first().map(String::as_str) == Some("mcp") {
        if arguments.len() > 1 {
            eprintln!("linklet: mcp takes no arguments");
            return i32::from(ExitCode::USAGE);
        }
        return run_mcp();
    }

    if arguments.first().map(String::as_str) == Some("pair") {
        return run_pair(&arguments[1..]);
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
            print!("{USAGE}");
            i32::from(ExitCode::SUCCESS)
        }
        Ok(Some(options)) => run(&options),
        Err(error) => {
            // Errors go to stderr so that a caller piping stdout gets only
            // report lines, and gets no output at all for a run that never
            // happened.
            eprintln!("linklet: {error}");
            i32::from(ExitCode::USAGE)
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
fn run(options: &Options) -> i32 {
    match check_specs(&options.specs, options.budget, options.max_targets) {
        Ok(reports) => {
            for report in &reports {
                println!("{}", render(report));
            }
            i32::from(exit_code_for(&reports))
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
    fn exit_code(&self) -> i32 {
        match self {
            Self::BadSpec(_) => i32::from(ExitCode::USAGE),
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
            ) => i32::from(ExitCode::REFUSED),
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

/// A rendered answer with the CLI status computed from the typed result.
struct CommandOutcome {
    tool: ToolOutcome,
    code: i32,
}

impl CommandOutcome {
    fn answered(text: String, code: i32) -> Self {
        Self {
            tool: ToolOutcome::ok(text),
            code,
        }
    }

    fn failed(text: String) -> Self {
        Self {
            tool: ToolOutcome::failed(text),
            code: i32::from(ExitCode::REFUSED),
        }
    }

    fn call_failed(agent: &str, error: &linklet_client::CallError) -> Self {
        let mut tool = ToolOutcome::failure(error.diagnostic()).at(agent);
        tool.text = linklet_client::render_call_error(error);
        Self {
            tool,
            code: i32::from(ExitCode::REFUSED),
        }
    }

    fn facts(mut self, agent: &str, data: serde_json::Value) -> Self {
        self.tool.target = Some(agent.into());
        if let Some(problem) = data.get("problem").filter(|v| !v.is_null())
            && let Ok(error) = serde_json::from_value::<OperationError>(problem.clone())
        {
            self.tool.fail(error);
        }
        if data.get("complete").and_then(serde_json::Value::as_bool) == Some(false) {
            self.tool
                .warnings
                .push("Observation is incomplete; inspect scope, limits and notes.".into());
        }
        self.tool.data = data;
        self
    }

    fn report(self) -> i32 {
        println!("{}", self.tool.text);
        self.code
    }
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
    fn pair(&self, agent: &str, code: &str) -> ToolOutcome {
        pair_on(agent, code).tool
    }
    fn exec_many(&self, agents: &[String], request: &RunRequest) -> ToolOutcome {
        exec_across(agents, request).tool
    }
    fn discover(&self, port: u16, networks: bool, targets: bool) -> ToolOutcome {
        discover_on(port, networks, targets).tool
    }
    fn probe(&self, agent: &str) -> ToolOutcome {
        probe_on(agent).tool
    }

    fn reachability(&self, targets: &str, budget: Duration) -> ToolOutcome {
        match check_specs(targets, budget, MAX_TARGETS) {
            Ok(reports) => ToolOutcome::ok(tool_text(&reports)).with_data(serde_json::json!({
                "results":reports.iter().map(|r| serde_json::json!({"target":r.target.to_string(),
                    "reachable":r.is_alive(),"reason":r.reason})).collect::<Vec<_>>(),
                "count":reports.len(),"complete":true,"authenticated":false})),
            // A target that does not parse is a failed call: the agent asked for
            // something this tool cannot interpret, and it has to change the
            // request. Bad news about a machine is the other case entirely.
            Err(CheckFailure::BadSpec(problem)) => ToolOutcome::failure(OperationError::new(
                ErrorCode::InvalidArgument,
                "validate",
                format!("the targets do not parse: {problem}"),
            )),
            Err(CheckFailure::Refused(error)) => ToolOutcome::failure(OperationError::new(
                ErrorCode::InvalidArgument,
                "validate",
                format!("the run was refused: {error}"),
            )),
        }
    }

    fn testbed(&self, spec_path: &str, target: &str) -> ToolOutcome {
        let text = match std::fs::read_to_string(spec_path) {
            Ok(text) => text,
            Err(error) => {
                return ToolOutcome::failure(linklet_adapters::failure::io(
                    error,
                    "read_config",
                    std::path::Path::new(spec_path),
                ));
            }
        };

        let Ok(testbed) = Testbed::parse(&text) else {
            let problem = Testbed::parse(&text).expect_err("just failed");
            return ToolOutcome::failure(OperationError::new(
                ErrorCode::LocalConfigError,
                "parse_config",
                format!("{spec_path}: {problem}"),
            ));
        };

        let verdicts = testbed.check(&SystemProber);
        ToolOutcome::ok(testbed::render(&testbed, &verdicts, target)).at(target).with_data(serde_json::json!({
            "name":testbed.name,"spec":spec_path,"held":verdicts.iter().all(|v|v.held),
            "results":verdicts.iter().map(|v|serde_json::json!({"held":v.held,"detail":v.detail})).collect::<Vec<_>>(),
            "count":verdicts.len()}))
    }

    fn exec(&self, agent: &str, request: &RunRequest) -> ToolOutcome {
        // Credentials are loaded per target from the pairing store.
        exec_on(agent, request).tool
    }

    fn push(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        transfer_on(Direction::Push, agent, from, to)
    }

    fn pull(&self, agent: &str, from: &str, to: &str) -> ToolOutcome {
        transfer_on(Direction::Pull, agent, from, to)
    }

    fn ps(&self, agent: &str, filter: &linklet_core::process::Filter) -> ToolOutcome {
        ps_on(agent, filter).tool
    }

    fn kill(&self, agent: &str, request: &KillRequest) -> ToolOutcome {
        kill_on(agent, request).tool
    }

    fn spawn(&self, agent: &str, request: &SpawnRequest) -> ToolOutcome {
        spawn_on(agent, request)
    }

    fn grep(&self, agent: &str, request: &GrepRequest) -> ToolOutcome {
        grep_on(agent, request).tool
    }

    fn tail(&self, agent: &str, request: &TailRequest) -> ToolOutcome {
        tail_on(agent, request).tool
    }

    fn ls(&self, agent: &str, request: &LsRequest) -> ToolOutcome {
        ls_on(agent, request).tool
    }
}

/// Runs a command on one agent and says what happened, before it is rendered.
///
/// # Errors
///
/// Calls one agent, retaining typed failures and the remote command outcome.
///
/// # Errors
/// Returns a call error when no valid command outcome was received. A completed
/// nonzero exit remains a RunOutcome and is classified by command_result.
fn exec_at(
    agent: &str,
    request: &RunRequest,
) -> Result<wire::RunOutcome, linklet_client::CallError> {
    linklet_client::run(&configured_address(agent)?, request)
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
/// A batch exits 0 only when every command exits 0; failures retain their details.
fn exec_across(agents: &[String], request: &RunRequest) -> CommandOutcome {
    use linklet_core::fanout::{Fate, Task};
    if agents.is_empty() || agents.len() > linklet_core::fanout::MAX_TARGETS {
        let mut result =
            CommandOutcome::failed("agents must contain 1..=256 host:port entries".into());
        result.code = i32::from(ExitCode::USAGE);
        return result;
    }
    let results = std::sync::Arc::new(std::sync::Mutex::new(
        (0..agents.len())
            .map(|_| None)
            .collect::<Vec<Option<CommandOutcome>>>(),
    ));
    let tasks = agents
        .iter()
        .enumerate()
        .map(|(index, agent)| {
            let agent = agent.clone();
            let request = request.clone();
            let results = std::sync::Arc::clone(&results);
            Task::new(agent.clone(), move || {
                let (fate, result) = match exec_at(&agent, &request) {
                    Ok(outcome) => (Fate::Ran, command_result(&agent, outcome)),
                    Err(error) => {
                        let fate = if error.was_refused() {
                            Fate::Refused
                        } else {
                            Fate::Unreachable
                        };
                        (fate, CommandOutcome::call_failed(&agent, &error))
                    }
                };
                if let Ok(mut slots) = results.lock() {
                    slots[index] = Some(result);
                }
                fate
            })
        })
        .collect();
    let report = linklet_core::fanout::run(tasks);
    let mut text = format!("{} of {} ran", report.ran(), report.total);
    let mut failed = !report.complete();
    let mut records = Vec::new();
    let mut status = Status::Ok;
    if let Ok(slots) = results.lock() {
        for (index, outcome) in report.outcomes.iter().enumerate() {
            text.push_str(&format!("\n{} {}", outcome.fate.as_str(), outcome.target));
            if let Some(result) = &slots[index] {
                failed |= result.tool.is_error();
                status = status.max(result.tool.status);
                records.push(result.tool.structured());
                for line in result.tool.text.lines() {
                    text.push_str(&format!("\n  {line}"));
                }
            } else {
                failed = true;
                status = Status::Unknown;
                records.push(
                    ToolOutcome::failure(OperationError::new(
                        ErrorCode::ResultUnknown,
                        "worker",
                        "execution did not return a result",
                    ))
                    .at(&outcome.target)
                    .structured(),
                );
                text.push_str("\n  execution did not return a result");
            }
        }
    } else {
        failed = true;
    }
    let mut result = CommandOutcome::answered(text, i32::from(failed));
    if failed {
        result.tool.fail(OperationError::new(
            if status == Status::Unknown {
                ErrorCode::ResultUnknown
            } else {
                ErrorCode::CommandFailed
            },
            "execute",
            "one or more commands failed or have an unknown outcome",
        ));
    }
    result.tool.data = serde_json::json!({"count":agents.len(),"results":records,
        "ok":records.iter().filter(|r|r["status"]=="ok").count(),
        "error":records.iter().filter(|r|r["status"]=="error").count(),
        "unknown":records.iter().filter(|r|r["status"]=="unknown").count()});
    result
}

fn command_result(agent: &str, outcome: wire::RunOutcome) -> CommandOutcome {
    let mut result = CommandOutcome::answered(
        wire::render_run(&outcome),
        outcome
            .exit_code
            .unwrap_or(i32::from(ExitCode::NOT_ALL_ALIVE)),
    );
    result.tool.target = Some(agent.into());
    result.tool.data = wire::run_outcome_to_json(&outcome);
    if let Some(error) = outcome.failure() {
        result.tool.fail(error);
        if result.code == 0 {
            result.code = 1;
        }
    }

    result
}

/// Runs a command on an agent and renders what it did.
///
/// Shared by CLI and MCP; both call failures and unsuccessful commands are errors.
fn exec_on(agent: &str, request: &RunRequest) -> CommandOutcome {
    match exec_at(agent, request) {
        Ok(outcome) => command_result(agent, outcome),
        Err(error) => CommandOutcome::call_failed(agent, &error),
    }
}

/// Probes one agent and reports authenticated usability.
///
/// A thin command over [`probe::run_probe`], which carries the reasoning and the exit codes. It
/// is its own subcommand rather than a flag on `check` because the two ask different questions
/// and a script has to tell them apart: `check` says whether a socket is there, and this says
/// whether the agent behind it is working.
fn run_probe_command(arguments: &[String]) -> i32 {
    let flags = [Flag::takes_value("--agent")];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: probe needs --agent <host:port>");
        return i32::from(ExitCode::USAGE);
    };

    let outcome = probe_on(agent);
    if outcome.code == i32::from(ExitCode::USAGE) {
        eprintln!("linklet: {}", outcome.tool.text);
        outcome.code
    } else {
        outcome.report()
    }
}

/// Runs the MCP server on stdio until the client closes it.
/// Returns the exit code. `serve` reports an I/O failure as an `Err`, and a
/// broken pipe is the normal way this ends -- the client exits and stops reading
/// -- so it is reported on stderr rather than as a crash.
fn run_mcp() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match serve(stdin.lock(), stdout.lock(), &LiveRunner) {
        Ok(_) => i32::from(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("linklet: mcp session ended: {error}");
            i32::from(ExitCode::NOT_ALL_ALIVE)
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
fn run_testbed(arguments: &[String]) -> i32 {
    let mut words = arguments.iter();
    let command = words.next().map(String::as_str);

    if command != Some("check") {
        match command {
            None => eprintln!("linklet: usage: linklet testbed check <spec-file> <target>"),
            Some(other) => {
                eprintln!("linklet: unknown testbed command {other:?}; expected check");
            }
        }
        return i32::from(ExitCode::USAGE);
    }

    let Some(spec_path) = words.next() else {
        eprintln!("linklet: testbed check needs a specification file");
        return i32::from(ExitCode::USAGE);
    };
    let Some(target) = words.next() else {
        eprintln!("linklet: testbed check needs a target name, even if only a label");
        return i32::from(ExitCode::USAGE);
    };
    if words.next().is_some() {
        eprintln!("linklet: testbed check takes a specification file and a target, nothing else");
        return i32::from(ExitCode::USAGE);
    }

    let text = match std::fs::read_to_string(spec_path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("linklet: cannot read {spec_path}: {error}");
            return i32::from(ExitCode::USAGE);
        }
    };

    let testbed = match Testbed::parse(&text) {
        Ok(testbed) => testbed,
        Err(error) => {
            // The specification's own line number, so the fix is a one-line edit
            // rather than a search through the file.
            eprintln!("linklet: {spec_path}: {error}");
            return i32::from(ExitCode::USAGE);
        }
    };

    let verdicts = testbed.check(&SystemProber);
    println!("{}", testbed::render(&testbed, &verdicts, target));

    if verdicts.iter().all(|verdict| verdict.held) {
        i32::from(ExitCode::SUCCESS)
    } else {
        i32::from(ExitCode::NOT_ALL_ALIVE)
    }
}

/// Runs a command on an agent and prints what it did.
///
/// Single-target execution preserves the remote exit code. A call failure returns
/// 3, which can also be a remote exit code; the result text distinguishes them.
/// Batches return 0 only when every command exits 0.
fn execution_arguments(
    args: &Args,
) -> Result<(linklet_core::execution::CommandSpec, Option<String>), i32> {
    let mut value = if let Some(program) = args.value("--program") {
        serde_json::json!({"program":program,"args":args.words()})
    } else {
        serde_json::json!({"command":args.tail()})
    };
    if let Some(cwd) = args.value("--cwd") {
        value["cwd"] = cwd.into();
    }
    let parsed = linklet_core::execution::CommandSpec::from_json(&value)
        .and_then(|command| Ok((command, linklet_core::execution::cwd_from_json(&value)?)));
    parsed.map_err(|error| {
        eprintln!("linklet: {error}");
        i32::from(ExitCode::USAGE)
    })
}
fn run_exec(arguments: &[String]) -> i32 {
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--agents"),
        Flag::takes_value("--timeout"),
        Flag::takes_value("--cwd"),
        Flag::starts_tail("--program"),
    ];
    let args = match read(arguments, &flags, Strictness::CommandTail) {
        Ok(args) => args,
        Err(code) => return code,
    };
    let timeout_seconds = match number(&args, "--timeout") {
        Ok(value) => value.unwrap_or(DEFAULT_EXEC_TIMEOUT_SECONDS),
        Err(code) => return code,
    };
    if !(1..=wire::MAX_TIMEOUT_SECONDS).contains(&timeout_seconds) {
        eprintln!(
            "linklet: --timeout must be in 1..={}",
            wire::MAX_TIMEOUT_SECONDS
        );
        return 2;
    }
    let single = args.value("--agent");
    let several = args.value("--agents");
    if single.is_some() == several.is_some() {
        eprintln!("linklet: exec needs --agent for one machine or --agents for several, not both");
        return 2;
    }
    let (command, cwd) = match execution_arguments(&args) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let request = RunRequest {
        command,
        cwd,
        timeout_seconds,
    };
    if let Some(agent) = single {
        exec_on(agent, &request).report()
    } else {
        exec_across(&args.list("--agents"), &request).report()
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
fn run_transfer(direction: Direction, arguments: &[String]) -> i32 {
    let options = match parse_transfer(direction, arguments) {
        Ok(options) => options,
        Err(problem) => {
            // The same pointer the shared parser adds, because a caller who mistyped a flag
            // needs the same thing to read next whether the command was `push` or `ps`. It was
            // the only message in the tool without it, and the reason was that transfers had
            // their own printer -- which is the drift this refactor was about.
            eprintln!("linklet: {problem}; see linklet --help");
            return i32::from(ExitCode::USAGE);
        }
    };

    let outcome = transfer_on(direction, &options.agent, &options.from, &options.to);

    if outcome.is_error() {
        eprintln!("linklet: {}", outcome.text);
        return i32::from(ExitCode::REFUSED);
    }

    println!("{}", outcome.text);
    i32::from(ExitCode::SUCCESS)
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
    // Help is handled by dispatch; transfer arguments must contain only named options.
    let args =
        Args::parse(arguments, &flags, Strictness::Strict).map_err(|error| error.to_string())?;

    if let Some(word) = args.words().first() {
        return Err(format!("unexpected argument {word:?}"));
    }
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
fn run_ps(arguments: &[String]) -> i32 {
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
        return i32::from(ExitCode::USAGE);
    };

    let outcome = ps_on(agent, &filter);
    outcome.report()
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
fn run_discover(arguments: &[String]) -> i32 {
    let flags = [
        Flag::takes_value("--port"),
        Flag::switch("--networks"),
        Flag::switch("--targets"),
    ];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };
    let port = match args.value("--port").unwrap_or("8787").parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => {
            eprintln!("linklet: --port must be in 1..=65535");
            return i32::from(ExitCode::USAGE);
        }
    };
    discover_on(port, args.switch("--networks"), args.switch("--targets")).report()
}

fn discover_on(port: u16, networks: bool, targets: bool) -> CommandOutcome {
    let failure = |reason: String| {
        let mut result = CommandOutcome::failed(reason);
        result.code = i32::from(ExitCode::NOT_ALL_ALIVE);
        result
    };
    if networks {
        return match linklet_adapters::local_interfaces() {
            Ok(interfaces) => CommandOutcome::answered(
                interfaces
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
                0,
            ).facts("local",serde_json::json!({"networks":interfaces.iter().map(ToString::to_string).collect::<Vec<_>>(),"count":interfaces.len(),"complete":true,"authenticated":false})),
            Err(reason) => failure(reason),
        };
    }
    let plan = match linklet_adapters::plan_here() {
        Ok(plan) => plan,
        Err(reason) => return failure(reason),
    };
    let result = linklet_adapters::scan(&plan, port);
    let text = if targets {
        result
            .found
            .iter()
            .map(|f| format!("{}:{}", f.address, f.port))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        let mut lines = vec![format!(
            "{} of {} addresses answered on port {port}; TCP reachable, authentication not verified",
            result.found.len(),
            result.tried
        )];
        lines.extend(result.found.iter().map(|f| f.address.to_string()));
        if !result.networks.is_empty() {
            lines.push(format!("networks: {}", result.networks.join(" ")));
        }
        lines.extend(result.skipped.iter().map(|s| format!("skipped: {s}")));
        if result.truncated {
            lines.push("note: the scan was cut short at its ceiling, so addresses beyond it were not tried".into());
        }
        lines.join("\n")
    };
    let mut answer = CommandOutcome::answered(text, i32::from(result.truncated));
    answer.tool.data = serde_json::json!({"listeners":result.found.iter().map(|f|format!("{}:{}",f.address,f.port)).collect::<Vec<_>>(),"networks":result.networks,"skipped":result.skipped,"truncated":result.truncated,"complete":!result.truncated && result.skipped.is_empty(),"authenticated":false,"count":result.found.len(),"scanned":result.tried,"port":port});
    answer
}

/// Lists a path on an agent's machine and prints what is there.
///
/// The exit code is `check`'s: `0` when the listing arrived and the whole directory was read,
/// `1` when it arrived and something about it is incomplete -- the path was not there, or the
/// list was cut short -- and `3` when the call could not be made. **A path that is not there
/// is exit 1 and not 3**, because the call *was* made: what went wrong is on the machine, and
/// sending the reader to the network for a directory they typed wrong is the mistake the
/// exit-code scheme exists to prevent.
fn run_ls(arguments: &[String]) -> i32 {
    // **One flag table, one parser.** The reasoning is in `linklet_core::arguments`; what is
    // local is which flags this command has and which of them are required.
    let flags = [Flag::takes_value("--agent"), Flag::takes_value("--from")];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: ls needs --agent <host:port>");
        return i32::from(ExitCode::USAGE);
    };
    // A path is required rather than defaulted to the root. The root is the agent's own
    // choice and a caller that asked for "everything" would get a listing of a directory it
    // did not name, which is the shape of a guess.
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: ls needs --from <path on the target>");
        return i32::from(ExitCode::USAGE);
    };

    let outcome = ls_on(
        agent,
        &LsRequest {
            path: path.to_string(),
        },
    );
    outcome.report()
}

/// Lists a path on an agent's machine and renders what is there.
///
/// Shared by the command and the tool.
fn ls_on(agent: &str, request: &LsRequest) -> CommandOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error),
    };

    match linklet_client::ls(&address, request) {
        Ok(listing) => CommandOutcome::answered(
            linklet_core::listing::render(&listing),
            i32::from(!listing.found || listing.truncated),
        )
        .facts(agent, wire::ls_listing_to_json(&listing)),
        Err(error) => CommandOutcome::call_failed(agent, &error),
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
fn run_grep(arguments: &[String]) -> i32 {
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
        return i32::from(ExitCode::USAGE);
    };
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: grep needs --from <path on the target>");
        return i32::from(ExitCode::USAGE);
    };
    let Some(text) = args.value("--pattern") else {
        eprintln!("linklet: grep needs --pattern <text>");
        return i32::from(ExitCode::USAGE);
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

    grep_on(agent, &request).report()
}

/// Reads the last lines of a file on an agent's machine and prints them.
///
/// The exit code follows [`run_grep`], and for the same reason.
fn run_tail(arguments: &[String]) -> i32 {
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--from"),
        Flag::takes_value("--lines"),
        Flag::takes_value("--cursor"),
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
        return i32::from(ExitCode::USAGE);
    };
    let Some(path) = args.value("--from") else {
        eprintln!("linklet: tail needs --from <path on the target>");
        return i32::from(ExitCode::USAGE);
    };

    tail_on(
        agent,
        &TailRequest {
            cursor: args.value("--cursor").map(str::to_string),
            content_bytes: linklet_core::tail::PAGE_BYTES,
            path: path.to_string(),
            count,
        },
    )
    .report()
}

/// Searches a file on an agent's machine and renders what it found.
///
/// Shared by the command and the tool.
fn grep_on(agent: &str, request: &GrepRequest) -> CommandOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error),
    };

    match linklet_client::grep(&address, request) {
        Ok(search) => CommandOutcome::answered(
            linklet_core::search::render(&search),
            i32::from(!search.searched || search.truncated),
        )
        .facts(agent, wire::search_to_json(&search)),
        Err(error) => CommandOutcome::call_failed(agent, &error),
    }
}

/// Reads the end of a file on an agent's machine and renders it.
fn tail_on(agent: &str, request: &TailRequest) -> CommandOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error),
    };

    match linklet_client::tail(&address, request) {
        Ok(page) => {
            let mut data = serde_json::to_value(&page).expect("page serializes");
            data["complete"] = (!page.has_more && page.pending_bytes == 0).into();
            let mut result = CommandOutcome::answered(
                format!(
                    "{} bytes {}..{} in {}, {}; has_more={}\n{}\nnext_cursor {}",
                    page.range.end - page.range.start,
                    page.range.start,
                    page.range.end,
                    page.path,
                    page.encoding,
                    page.has_more,
                    page.text,
                    page.next_cursor
                ),
                0,
            )
            .facts(agent, data);
            result.tool.warnings = page.warnings;
            result
        }
        Err(error) => CommandOutcome::call_failed(agent, &error),
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
fn run_spawn(arguments: &[String]) -> i32 {
    // CommandTail leaves every argument after the command to the target shell.
    let flags = [
        Flag::takes_value("--agent"),
        Flag::takes_value("--output"),
        Flag::takes_value("--observe-ms"),
        Flag::takes_value("--cwd"),
        Flag::starts_tail("--program"),
    ];
    let args = match read(arguments, &flags, Strictness::CommandTail) {
        Ok(args) => args,
        Err(code) => return code,
    };

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: spawn needs --agent <host:port>");
        return i32::from(ExitCode::USAGE);
    };
    let (command, cwd) = match execution_arguments(&args) {
        Ok(value) => value,
        Err(code) => return code,
    };
    // Required rather than defaulted, and the refusal says why: a program whose output goes
    // nowhere is a program the caller cannot look at afterwards, which is the one thing this
    // feature is for. A default like `linklet-spawn.log` in the agent's working directory
    // would be a file nobody asked for in a place nobody chose.
    let Some(output) = args.value("--output") else {
        eprintln!("linklet: spawn needs --output <path on the target>: the program writes");
        eprintln!("linklet:   its own output there, which is how you read it afterwards");
        return i32::from(ExitCode::USAGE);
    };

    let observe_ms = match number(&args, "--observe-ms") {
        Ok(None) => 500,
        Ok(Some(n)) if n <= 5000 => n,
        _ => {
            eprintln!("linklet: --observe-ms must be 0..=5000");
            return 2;
        }
    };
    let request = SpawnRequest {
        observe_ms,
        // The command is exactly what was typed after the options, joined the way `exec`
        // joins it: a tool that reassembled a command line from pieces would be a second
        // interpretation of the caller's quoting.
        command,
        cwd,
        output: output.to_string(),
    };

    let outcome = spawn_on(agent, &request);
    println!("{}", outcome.text);

    if outcome.is_error() {
        return i32::from(ExitCode::REFUSED);
    }
    i32::from(ExitCode::SUCCESS)
}

/// Starts a program on an agent's machine and renders what was started.
///
/// Shared by the command and the tool, like `ps_on` and `kill_on`.
fn spawn_on(agent: &str, request: &SpawnRequest) -> ToolOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return ToolOutcome::failure(error.diagnostic()).at(agent),
    };

    match linklet_client::spawn(&address, request) {
        // `started <pid>`, and the word is chosen: "started" is what this side knows, where
        // "running" would be a claim about a moment it has not looked at.
        Ok(report) => {
            let mut outcome = ToolOutcome::ok(format!(
                "{} {} ({}; observed {} ms; exit {:?}; readiness not checked)",
                report.state, report.pid, report.pid_kind, report.observed_ms, report.exit_code
            ))
            .at(agent)
            .with_data(wire::spawn_report_to_json(&report));
            if let Some(error) = report.error {
                outcome.fail(error);
            }
            outcome
        }
        Err(error) => CommandOutcome::call_failed(agent, &error).tool,
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
fn run_kill(arguments: &[String]) -> i32 {
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
                return i32::from(ExitCode::USAGE);
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
        return i32::from(ExitCode::USAGE);
    }

    let Some(agent) = args.value("--agent") else {
        eprintln!("linklet: kill needs --agent <host:port>");
        return i32::from(ExitCode::USAGE);
    };
    let Some(to_kill) = targets.pop() else {
        eprintln!("linklet: kill needs one of --pid, --name or --contains");
        return i32::from(ExitCode::USAGE);
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

    let outcome = kill_on(agent, &request);
    outcome.report()
}

/// Stops something on an agent's machine and renders what happened.
///
/// Shared by the command and the tool, like `ps_on`, so that the two cannot disagree about
/// what a refusal looks like.
fn kill_on(agent: &str, request: &KillRequest) -> CommandOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error),
    };

    match linklet_client::kill(&address, request) {
        Ok(report) => {
            let mut result = CommandOutcome::answered(
                linklet_core::process::render_kill(&report),
                i32::from(!report.complete()),
            )
            .facts(agent, wire::kill_report_to_json(&report));
            if !report.complete() {
                result.tool.fail(OperationError::new(
                    ErrorCode::IoError,
                    "terminate",
                    "some processes could not be stopped; inspect per-process results",
                ));
            }
            result
        }
        Err(error) => CommandOutcome::call_failed(agent, &error),
    }
}

/// Asks an agent what is running and renders what it said.
/// Shared by the command and the tool, like `exec_on` and `transfer_on`, so that the two
/// cannot disagree about what a failure looks like.
fn ps_on(agent: &str, filter: &linklet_core::process::Filter) -> CommandOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error),
    };

    match linklet_client::ps(&address, filter) {
        Ok(listing) => CommandOutcome::answered(
            linklet_core::process::render(&listing),
            i32::from(
                listing.truncated || listing.incomplete() != linklet_core::process::Incomplete::No,
            ),
        )
        .facts(agent, wire::ps_listing_to_json(&listing)),
        Err(error) => CommandOutcome::call_failed(agent, &error),
    }
}

/// Moves one file and renders what happened.
/// Shared by the command and the tool, so that the two cannot disagree about what a
/// failure looks like -- the same shape [`exec_on`] has. The distinction it keeps is the
/// one this project is arranged around: a call that could not be made is `is_error`, and
/// a transfer whose bytes did not match the digest is a refusal rather than a quiet
/// success -- so an ok result means the receiving side compared digests and the file
/// under the real name is the one that was sent.
fn transfer_on(direction: Direction, agent: &str, from: &str, to: &str) -> ToolOutcome {
    let address = match configured_address(agent) {
        Ok(address) => address,
        Err(error) => return CommandOutcome::call_failed(agent, &error).tool,
    };

    let result = match direction {
        // `from` is the local path on a push, and `to` is the remote one.
        Direction::Push => linklet_client::push(&address, std::path::Path::new(from), to),
        Direction::Pull => linklet_client::pull(&address, from, std::path::Path::new(to)),
    };

    match result {
        Ok(outcome) => ToolOutcome::ok(wire::render_transfer(&outcome, to)).at(agent).with_data(serde_json::json!({"bytes":outcome.bytes,"sha256":outcome.sha256,"verified":true,"from":outcome.source,"to":outcome.destination})),
        Err(error) => CommandOutcome::call_failed(agent, &error).tool,
    }
}

fn probe_on(agent: &str) -> CommandOutcome {
    match token_for(agent) {
        Ok(token) => probe::run_probe(agent, &token),
        Err(error) => {
            let mut outcome = CommandOutcome::call_failed(agent, &error);
            outcome.code = i32::from(ExitCode::USAGE);
            outcome
        }
    }
}

fn token_for(agent: &str) -> Result<Token, linklet_client::CallError> {
    let directory =
        linklet_adapters::credentials::config_dir().map_err(linklet_client::CallError::Local)?;
    linklet_adapters::credentials::saved_token(&directory, agent).map_err(linklet_client::CallError::Local)?.ok_or_else(|| {
        linklet_client::CallError::PairingRequired(format!(
            "no saved credential for {agent}; run linklet pair --agent {agent} --code <six-digits>"
        ))
    })
}
fn configured_address(agent: &str) -> Result<AgentAddress, linklet_client::CallError> {
    let address = AgentAddress::new(agent)?;
    let token = token_for(agent)?;
    Ok(address.with_token(token))
}
fn pair_on(agent: &str, code: &str) -> CommandOutcome {
    let result = (|| {
        linklet_core::pairing::validate_code(code).map_err(linklet_client::CallError::Local)?;
        let directory = linklet_adapters::credentials::config_dir()
            .map_err(linklet_client::CallError::Local)?;
        let (token, identity) = linklet_client::pair(agent, code)?;
        linklet_adapters::credentials::save_token(&directory, agent, &token)
            .map_err(linklet_client::CallError::Local)?;
        Ok((
            format!("paired with {agent} ({identity}); credential saved for this account"),
            identity,
        ))
    })();
    match result {
        Ok((text, identity)) => {
            let mut result = CommandOutcome::answered(text, 0);
            result.tool.target = Some(agent.into());
            result.tool.data = identity.to_json();
            result.tool.warnings = identity.warnings;
            result
        }
        Err(error) => {
            let code = match &error {
                linklet_client::CallError::Local(_) | linklet_client::CallError::BadAddress(_) => 2,
                linklet_client::CallError::Connect(_) => 1,
                linklet_client::CallError::Refused(_)
                | linklet_client::CallError::VersionMismatch(_) => 3,
                _ => 4,
            };
            CommandOutcome {
                tool: ToolOutcome::failure(error.diagnostic()).at(agent),
                code,
            }
        }
    }
}
fn run_pair(arguments: &[String]) -> i32 {
    let flags = [Flag::takes_value("--agent"), Flag::takes_value("--code")];
    let args = match read(arguments, &flags, Strictness::Strict) {
        Ok(args) => args,
        Err(code) => return code,
    };
    let (Some(agent), Some(code)) = (args.value("--agent"), args.value("--code")) else {
        eprintln!("linklet: pair needs --agent <host:port> and --code <six-digits>");
        return 2;
    };
    let outcome = pair_on(agent, code);
    if outcome.tool.is_error() {
        eprintln!("linklet: {}", outcome.tool.text);
        outcome.code
    } else {
        outcome.report()
    }
}
