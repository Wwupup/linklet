//! The command line, run as a process.
//!
//! These tests launch the real binary. That is slower and less pleasant than
//! calling a function, and it is the point: everything else in this repository
//! tests decisions, and this file tests the three things that only exist once a
//! program is a process -- the exit code, which stream the text went to, and
//! whether the lines are in the order the targets were given.
//!
//! A unit test cannot catch a report printed to stderr, or an exit code
//! discarded on the way out of `main`, and those are exactly the bugs that break
//! an agent using this tool.

use std::net::UdpSocket;
use std::process::{Command, Output};

use linklet_core::ExitCode;

/// Runs the binary with the given arguments.
fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_linklet"))
        .args(arguments)
        .output()
        .expect("the binary under test should be runnable")
}

/// The exit code, as a `u8` so it can be compared with `ExitCode` directly.
fn code(output: &Output) -> u8 {
    output
        .status
        .code()
        .expect("the process should exit normally, not be killed by a signal") as u8
}

/// stdout as text.
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("the tool prints UTF-8")
}

/// stderr as text.
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("the tool prints UTF-8")
}

/// Binds a TCP listener on a free port and keeps it, so the port is *live*.
fn listening() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    (listener, port)
}

/// Returns a port number with nothing listening on TCP.
///
/// A UDP socket is bound and kept. UDP and TCP have separate port spaces, so
/// this reserves the number against any other test without putting a TCP
/// listener on it -- which means a connection is refused at once instead of
/// waiting out the ~2 s Windows takes to surface a refusal.
///
/// The obvious alternative, "bind a TCP listener and drop it", does not hold the
/// number: something else on the machine can take it in between. This does.
fn closed_tcp_port() -> (UdpSocket, u16) {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind udp");
    let port = socket.local_addr().expect("address").port();
    (socket, port)
}

// --- asking for help ---------------------------------------------------------

#[test]
fn help_goes_to_stdout_and_does_not_claim_to_have_checked_anything() {
    let output = run(&["--help"]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(stdout(&output).contains("usage:"));
    assert!(stderr(&output).is_empty(), "help is not an error message");
}

#[test]
fn no_arguments_is_a_usage_error_on_stderr() {
    let output = run(&[]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(
        stdout(&output).is_empty(),
        "a run that never happened prints no report"
    );
    assert!(stderr(&output).contains("no command given"));
}

#[test]
fn an_unknown_option_names_itself() {
    let output = run(&["check", "--nope", "a:1"]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(stderr(&output).contains("--nope"));
}

#[test]
fn a_flag_without_its_value_says_which_flag() {
    let output = run(&["check", "--timeout"]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(stderr(&output).contains("--timeout"));
}

#[test]
fn a_flag_with_a_non_number_says_what_it_got() {
    let output = run(&["check", "--timeout", "soon", "a:1"]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(stderr(&output).contains("soon"));
}

#[test]
fn a_bad_target_is_reported_with_the_core_word_for_it() {
    let output = run(&["check", "a:0"]);

    assert_eq!(code(&output), ExitCode::USAGE);
    assert!(
        stderr(&output).contains("outside 1..=65535"),
        "the message should be the core's, got: {}",
        stderr(&output)
    );
}

// --- checking ----------------------------------------------------------------

#[test]
fn a_live_target_prints_one_line_and_exits_zero() {
    let (_listener, port) = listening();
    let target = format!("127.0.0.1:{port}");

    let output = run(&["check", &target]);

    assert_eq!(code(&output), ExitCode::SUCCESS);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "one target, one line");
    assert_eq!(lines[0], format!("live 127.0.0.1:{port} connected"));
}

#[test]
fn a_refused_target_prints_a_dead_line_and_exits_one() {
    let (_udp, port) = closed_tcp_port();

    let output = run(&["check", &format!("127.0.0.1:{port}")]);

    assert_eq!(code(&output), ExitCode::NOT_ALL_ALIVE);
    let text = stdout(&output);
    assert!(text.starts_with("dead "), "got: {text}");
    assert!(
        text.contains("refused"),
        "a refusal is the useful answer here, and it should survive to the output: {text}"
    );
    // The result is a fact about the world, not a failure of this program, so it
    // belongs on stdout. An agent reading only stderr would otherwise see a
    // silent success.
    assert!(
        stderr(&output).is_empty(),
        "a result is not an error message"
    );
}

#[test]
fn several_targets_come_back_in_the_order_they_were_given() {
    // The bug this catches: a report sorted by status, or grouped by machine.
    // An agent pairing results with its own input list would silently attach
    // each answer to the wrong target.
    let (_listener, live) = listening();
    let (_udp, dead) = closed_tcp_port();

    let output = run(&["check", &format!("127.0.0.1:{dead},127.0.0.1:{live}")]);

    assert_eq!(code(&output), ExitCode::NOT_ALL_ALIVE);
    let text = stdout(&output);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].contains(&dead.to_string()),
        "the first line should be the first target, got: {text}"
    );
    assert!(
        lines[1].contains(&live.to_string()),
        "the second line should be the second target, got: {text}"
    );
}

#[test]
fn a_comma_list_and_a_space_list_produce_identical_output() {
    // Two ways to write the same thing must not be two behaviours. They reach
    // the core as the same text; this is the test that keeps it that way.
    let (_listener, live) = listening();
    let (_udp, dead) = closed_tcp_port();

    let comma = run(&["check", &format!("127.0.0.1:{live},127.0.0.1:{dead}")]);
    let spaces = run(&[
        "check",
        &format!("127.0.0.1:{live}"),
        &format!("127.0.0.1:{dead}"),
    ]);

    assert_eq!(stdout(&comma), stdout(&spaces));
    assert_eq!(code(&comma), code(&spaces));
}

#[test]
fn a_host_that_cannot_be_resolved_is_unknown_and_exits_one() {
    let output = run(&["check", "no-such-host.invalid:80"]);

    assert_eq!(code(&output), ExitCode::NOT_ALL_ALIVE);
    assert!(
        stdout(&output).starts_with("unknown "),
        "got: {}",
        stdout(&output)
    );
}

// --- refused before anything is looked at ------------------------------------

#[test]
fn too_many_targets_is_refused_with_exit_three_and_no_report() {
    let output = run(&["check", "--max-targets", "1", "a:1", "b:2"]);

    assert_eq!(code(&output), ExitCode::REFUSED);
    assert!(
        stdout(&output).is_empty(),
        "a refused run must not print a partial report"
    );
    assert!(stderr(&output).contains("at most 1"));
}

#[test]
fn the_usage_code_is_not_the_refusal_code() {
    // The distinction an agent branches on. If these ever collide, "the tool
    // could not start" becomes indistinguishable from "the machines are down",
    // and an agent will report the wrong thing to a user.
    assert_ne!(ExitCode::USAGE, ExitCode::REFUSED);
    assert_ne!(ExitCode::SUCCESS, ExitCode::NOT_ALL_ALIVE);
}
