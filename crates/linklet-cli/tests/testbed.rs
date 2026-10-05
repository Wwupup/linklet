//! `linklet testbed check`, against this machine.
//!
//! # Why this layer
//!
//! Almost all of a testbed is decided in `linklet_core::testbed` and tested there in
//! microseconds: what a specification means, and whether an observation satisfies it. Two
//! requirements leave the core and ask the operating system -- a path, and **a process name** --
//! and it is the process one that this file exists for.
//!
//! **It was `tasklist`, and therefore Windows.** `no-process` is the requirement that makes a
//! testbed worth having on a hand-prepared machine: "a test that needs a clean machine needs the
//! *absence* of yesterday's process", which is the thing a machine somebody prepared by hand gets
//! wrong. A requirement that could only be checked on one platform is not a weaker requirement,
//! it is an unavailable one -- and the whole point of the Linux port is that this tool drives the
//! machines a person actually has.
//!
//! # What is asserted, and what is not
//!
//! The verdict, the exit code and the sentence, because those are the published interface: an
//! agent branches on the code and quotes the sentence. **Not** which program produced the answer.
//! `tasklist` and `/proc` are both allowed to be how the machine knows.

use std::path::PathBuf;
use std::process::Command;

use linklet_core::ExitCode;

/// The name of a process that is certainly running: this test binary.
fn a_running_process() -> String {
    std::env::current_exe()
        .expect("a running process knows its own path")
        .file_name()
        .expect("and it has a file name")
        .to_string_lossy()
        .into_owned()
}

/// A name that is certainly not running.
///
/// Long and specific rather than "nothing" or "app", because the answer has to be a fact about
/// this machine and a short name is a fact about what else happens to be running on it.
const NOT_RUNNING: &str = "linklet-no-such-process-anywhere-at-all";

/// Runs `linklet testbed check` on a specification written for the occasion.
///
/// Returns the exit code, stdout and stderr. The specification goes in the system temporary
/// directory: the CLI takes any path, and a test that wrote into the repository would leave
/// something behind when it failed.
fn check(spec: &str) -> (u8, String, String) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);

    let path = std::env::temp_dir().join(format!(
        "linklet-testbed-{}-{}.testbed",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, spec).expect("writing the spec file");

    let output = Command::new(env!("CARGO_BIN_EXE_linklet"))
        // **The repository root, deliberately.** An `artifact` requirement names a path
        // relative to the process's working directory, and `cargo test` starts this binary in
        // the crate directory rather than at the root -- so without this the path below would
        // mean `crates/linklet-cli/Cargo.toml`, which happens to exist and would make the test
        // pass for a reason it does not state.
        .current_dir(repository_root())
        .args(["testbed", "check", &path.to_string_lossy(), "this-machine"])
        .output()
        .expect("the binary under test should be runnable");

    let _ = std::fs::remove_file(&path);

    let code = output
        .status
        .code()
        .expect("the process should exit normally, not be killed by a signal") as u8;
    (
        code,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_process_that_is_not_running_satisfies_no_process() {
    // The requirement's whole purpose: a test that needs a clean machine is a test that needs
    // the absence of yesterday's process, and this is the check that says so.
    let (code, stdout, stderr) = check(&format!("name clean\nrequire no-process {NOT_RUNNING}\n"));

    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "nothing named {NOT_RUNNING} is running: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains(&format!("nothing named {NOT_RUNNING} is running")),
        "the verdict has to say what it looked for: {stdout:?}"
    );
}

#[test]
fn a_process_that_is_running_fails_no_process_and_names_it() {
    // **The failure the requirement exists to produce**, and it has to be loud: a machine with
    // yesterday's program still on it is the state every one of these tests assumes away.
    let running = a_running_process();
    let (code, stdout, stderr) = check(&format!("name dirty\nrequire no-process {running}\n"));

    assert_eq!(
        code,
        ExitCode::NOT_ALL_ALIVE,
        "this process is running: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("NOT READY"),
        "and the report says so: {stdout:?}"
    );
    assert!(
        stdout.contains(&running),
        "the verdict names the process it found: {stdout:?}"
    );
}

#[test]
fn the_name_match_is_exact_and_not_a_substring() {
    // **The semantics Windows had and Linux has to keep.** `ps`'s `--name` filter is a
    // substring and `kill`'s is exact, deliberately; this one is exact, and the reason is the
    // same as `kill`'s: a requirement that fired on a *different* process than the one named
    // would be a requirement nobody could satisfy by fixing the machine.
    //
    // The name is the running one with its last character removed -- a real name's prefix that
    // no process has -- so a substring match finds this process and an exact one finds nothing.
    let running = a_running_process();
    let prefix = running[..running.len() - 1].to_string();

    let (code, stdout, stderr) = check(&format!("name prefix\nrequire no-process {prefix}\n"));

    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "only {running} is running, and {prefix} is not it: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains(&format!("nothing named {prefix} is running")),
        "{stdout:?}"
    );
}

#[test]
fn a_specification_that_asks_about_processes_and_files_together_is_judged_as_a_whole() {
    // The two portable requirements and the one that used to be Windows-only, in one
    // specification, so that a machine which answers all three is READY -- which is the sentence
    // a caller actually acts on.
    let spec = format!(
        "name mixed\nrequire no-process {NOT_RUNNING}\nrequire artifact Cargo.toml present\n"
    );

    let (code, stdout, stderr) = check(&spec);

    assert_eq!(
        code,
        ExitCode::SUCCESS,
        "both requirements hold here: stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.trim_end().ends_with("READY 2 of 2 requirements met"),
        "the summary counts them: {stdout:?}"
    );
}

/// A path under the repository, so the `artifact` requirement above names something that is
/// there whatever else this machine happens to have.
fn repository_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root is two levels up from a crate")
}
