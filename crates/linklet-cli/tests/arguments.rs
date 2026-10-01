//! Argument parsing, over every command that has flags.
//!
//! # Why this file exists
//!
//! `--agent` was implemented **nine times in three different styles**, and they had drifted:
//! three of them said `--agent needs a host:port` when the value was missing and five said
//! `--agent needs a value`. Nothing caught it, because no test ever ran a command's flag
//! parsing -- `cli.rs` covers `check`, which uses its own parser.
//!
//! So this file is deliberately **one test that walks every command**, rather than one test per
//! command. A per-command test is what let nine copies drift while staying individually green:
//! each was tested against its own behaviour, and nothing compared them. What is asserted here
//! is the property that was missing -- **the commands agree** -- so adding a tenth command means
//! adding its name to these tables and nothing else.
//!
//! # What is not asserted
//!
//! The exact sentence. `a flag needs a value` is pinned because it is now one string in one
//! place, but a test that pinned the whole message would fail every time somebody improved the
//! wording, which is the failure mode this project's `docs/testing.md` warns about. What is
//! pinned is that the flag is named, that the command is named where there is a command, and
//! that every command says the same kind of thing.

use std::process::{Command, Output};

use linklet_core::ExitCode;

/// The tool, run the way a person runs it.
fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_linklet"))
        // No token, so that a command which gets as far as opening a socket fails on that
        // rather than on the token -- this file is about parsing, and the parse happens first.
        .env_remove("LINKLET_TOKEN")
        .args(arguments)
        .output()
        .expect("the tool should run")
}

fn code(output: &Output) -> u8 {
    output
        .status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .expect("the tool exits with a status")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Every command that takes flags, with the flag each one needs a value for.
///
/// **The table is the test.** Adding a command means adding a row, and the row is what makes
/// the new command agree with the other nine rather than becoming a tenth style.
const COMMANDS: &[(&str, &str)] = &[
    ("exec", "--agent"),
    ("ps", "--agent"),
    ("kill", "--agent"),
    ("spawn", "--agent"),
    ("grep", "--agent"),
    ("tail", "--agent"),
    ("ls", "--agent"),
    ("push", "--agent"),
    ("pull", "--agent"),
    ("probe", "--agent"),
];

/// The commands that end in a command tail: everything after their own options goes to the
/// target as one command line.
///
/// **The line the parser has to draw, and why two commands are left out of the unknown-option
/// check.** `linklet exec --agent a:1 myapp.cmd --verbose` has to be possible, so a `--flag`
/// after the command belongs to the command. Every other command is flags only, and an
/// unrecognised one there is the caller's typo and gets refused by name.
const TAIL_COMMANDS: &[&str] = &["exec", "spawn"];

#[test]
fn every_command_that_needs_a_value_says_the_same_thing_when_it_is_missing() {
    // **The drift this file was written to catch.** Nine copies said two different things; the
    // fix is one message in one place, and this is the test that keeps it one message.
    for (command, flag) in COMMANDS {
        let output = run(&[command, flag]);
        let message = stderr(&output);

        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{command} {flag} with no value should be a usage error: {message}"
        );
        assert!(
            message.contains(flag),
            "{command} should name the flag it is missing a value for: {message}"
        );
        assert!(
            message.contains("needs a value"),
            "{command} should say a value was missing, and said: {message}"
        );
        // **And the pointer to the manual, which `push` and `pull` used to be alone in omitting**
        // -- they had their own error printer, which is the same shape of drift as the message
        // itself. A caller who mistyped a flag needs the same thing to read next whatever the
        // command was.
        assert!(
            message.contains("--help"),
            "{command} should point at the usage, and said: {message}"
        );
    }
}

#[test]
fn every_command_that_takes_flags_refuses_an_unknown_one_by_name() {
    // The other half of the same drift: an option a command does not know is a usage error
    // naming the option, not a silent shrug -- except where a command has a tail, which is
    // `exec` and is tested separately below.
    for (command, _) in COMMANDS {
        if TAIL_COMMANDS.contains(command) {
            // These take a command tail, so an unrecognised `--flag` after the command is part
            // of the command. See `a_flag_after_the_command_tail_belongs_to_the_command`.
            continue;
        }

        let output = run(&[command, "--not-a-flag", "x"]);
        let message = stderr(&output);

        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{command} should refuse an unknown option: {message}"
        );
        assert!(
            message.contains("--not-a-flag"),
            "{command} should name the option it does not know: {message}"
        );
    }
}

#[test]
fn every_command_that_needs_an_agent_says_which_flag_is_missing() {
    // A command with no target is a usage error that names what it wanted. Two commands are
    // left out and both for a reason: `exec` also accepts `--agents`, so it has its own test,
    // and `discover` takes no agent at all.
    for (command, _) in COMMANDS {
        if TAIL_COMMANDS.contains(command) {
            // `exec` and `spawn` also accept a list or a command, so neither has a single flag
            // whose absence is the whole story. See the tests below.
            continue;
        }

        let output = run(&[command]);
        let message = stderr(&output);

        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{command} with no target should be a usage error: {message}"
        );
        assert!(
            message.contains("--agent"),
            "{command} should name the flag it needs: {message}"
        );
    }
}

#[test]
fn a_flag_after_the_command_tail_belongs_to_the_command() {
    // **The line the parser has to draw somewhere.** `exec` sends everything after its own
    // options to the target as one command, so a `--flag` there is the command's business and
    // not this tool's. A parser that refused it would make
    // `linklet exec --agent a:1 myapp.cmd --verbose` impossible.
    let output = run(&["exec", "--agent", "127.0.0.1:1", "myapp.cmd", "--verbose"]);

    // Exit 3 is the refusal code: the address parsed, the call could not be made. What matters
    // is that it is not a usage error, which is what a parser that rejected `--verbose` would
    // have produced.
    assert_ne!(
        code(&output),
        ExitCode::USAGE,
        "a flag in the command tail must not be refused as this tool's option: {}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("--verbose"),
        "and it must not be reported as an unknown option: {}",
        stderr(&output)
    );
}

#[test]
fn a_flag_before_the_command_tail_is_still_this_tools_option() {
    // The other side of the same line. `--timeout` before the command is ours and is consumed;
    // it must not reach the target as part of the command, or every remote command would carry
    // this tool's flags.
    let output = run(&[
        "exec",
        "--agent",
        "127.0.0.1:1",
        "--timeout",
        "5",
        "echo",
        "hi",
    ]);

    assert_ne!(
        code(&output),
        ExitCode::USAGE,
        "--timeout is a valid option: {}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("--timeout"),
        "and it must not be refused: {}",
        stderr(&output)
    );
}

#[test]
fn one_agent_and_several_is_refused_rather_than_guessed() {
    // `exec` is the only command with both, and giving both is two intentions. Guessing would
    // run a command somewhere the caller did not name.
    let output = run(&[
        "exec",
        "--agent",
        "127.0.0.1:1",
        "--agents",
        "127.0.0.1:2",
        "echo",
        "x",
    ]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(
        stderr(&output).contains("not both"),
        "the refusal should say why: {}",
        stderr(&output)
    );
}

#[test]
fn a_non_number_where_a_number_belongs_names_the_value_that_arrived() {
    // The one message that is genuinely per-flag, so it is checked on the flags that have it.
    // **What the caller typed is echoed**: "needs a number" without the value sends a reader
    // back to their own command line to work out what was wrong with it.
    let cases: &[(&[&str], &str)] = &[
        (&["ps", "--agent", "a:1", "--max", "lots"], "--max"),
        (
            &["tail", "--agent", "a:1", "--from", "f", "--lines", "many"],
            "--lines",
        ),
        (&["kill", "--agent", "a:1", "--pid", "abc"], "--pid"),
    ];

    for (arguments, flag) in cases {
        let output = run(arguments);
        let message = stderr(&output);

        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{arguments:?} should be a usage error: {message}"
        );
        assert!(
            message.contains(flag),
            "{arguments:?} should name {flag}: {message}"
        );
    }
}

#[test]
fn a_command_with_nothing_to_do_says_what_it_wanted() {
    // The last of the three ways a command can be invoked wrongly, and the one most easily left
    // out of a shared parser: the flags are all fine and the command is still unusable.
    let cases: &[(&[&str], &str)] = &[
        (&["exec", "--agent", "a:1"], "command"),
        (&["push", "--agent", "a:1", "--from", "x"], "--to"),
        (&["pull", "--agent", "a:1", "--to", "x"], "--from"),
        (&["ls", "--agent", "a:1"], "--from"),
        (&["grep", "--agent", "a:1", "--from", "f"], "--pattern"),
        (&["spawn", "--agent", "a:1"], "command"),
    ];

    for (arguments, wanted) in cases {
        let output = run(arguments);
        let message = stderr(&output);

        assert_eq!(
            code(&output),
            ExitCode::USAGE,
            "{arguments:?} should be a usage error: {message}"
        );
        assert!(
            message.contains(wanted),
            "{arguments:?} should say it needs {wanted}: {message}"
        );
    }
}
