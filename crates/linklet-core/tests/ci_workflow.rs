//! The CI workflows, checked against the two claims made about them.
//!
//! This repository prefers a rule the compiler enforces to a rule a reviewer remembers, and the
//! same habit applies to its own automation. `tools/verify.ps1` says in its header that *"CI: the
//! workflow calls this exact script"*, and `docs/testing.md` says CI **does not restate the four
//! commands**. Both are claims about the repository that the repository can check.
//!
//! # What the first claim is worth
//!
//! `tools/verify.ps1` exists because the four commands were documented in three files and a commit
//! still went in red -- the rules were written and nothing ran them. A workflow that listed the
//! four commands a second time would rebuild that failure one level up: two lists to keep in step,
//! with the CI copy being the one nobody tries locally. So the test is not "does CI run the
//! gates", it is **"is the script still the only place they are written down"**.
//!
//! # What it deliberately does not check
//!
//! Whether the workflow is *correct* -- whether the runner image exists, whether the action
//! versions are current, whether the YAML is valid. Those need a YAML parser and a network, and a
//! test that half-parses YAML would be the "looks at text instead of parsing it" failure
//! `tests/architecture.rs` already records. What is checked here is the mechanical half: the files
//! are present, they call the script, and they do not duplicate it.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root is two levels up from a crate")
}

/// Every workflow file, by name.
fn workflows() -> Vec<(String, String)> {
    let dir = root().join(".github/workflows");
    let mut found: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", dir.display()))
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let text = fs::read_to_string(entry.path()).expect("a workflow file is text");
            (name, text)
        })
        .collect();

    found.sort();
    found
}

/// A `run:` body that invokes the gate script.
const INVOCATION: &str = "verify.ps1";

#[test]
fn every_workflow_that_runs_the_gates_calls_the_script() {
    let found = workflows();

    assert!(
        !found.is_empty(),
        "no workflow files were found under .github/workflows, so every claim \
         `README.md` and `docs/testing.md` make about CI is unchecked"
    );

    for (name, text) in &found {
        let runs_gates =
            text.contains("cargo ") || text.contains(INVOCATION) || text.contains("verify.ps1");
        if !runs_gates {
            // A workflow that runs nothing is allowed: a release job that only attaches
            // artifacts is a legitimate shape and this test has no opinion about it.
            continue;
        }

        assert!(
            text.contains(INVOCATION),
            "{name} mentions cargo but never calls `{INVOCATION}`, which is the single \
             entry point every gate goes through. Restating the four commands in a \
             workflow is the failure the script was written to prevent. See docs/testing.md."
        );
    }
}

#[test]
fn no_workflow_restates_the_gate_commands_that_the_script_owns() {
    // The four commands, as the script spells them. Each one appearing in a workflow means two
    // lists that can drift, and the drift is invisible until the day they disagree.
    const OWNED_BY_THE_SCRIPT: [&str; 4] = [
        "cargo fmt --all --check",
        "cargo clippy --workspace --all-targets",
        "cargo test --workspace",
        "cargo doc --workspace",
    ];

    for (name, text) in workflows() {
        for command in OWNED_BY_THE_SCRIPT {
            assert!(
                !text.contains(command),
                "{name} restates `{command}`, which `tools/verify.ps1` owns. The workflow \
                 should call the script, not copy it -- one list, one place to add a gate."
            );
        }
    }
}

#[test]
fn the_workflows_that_build_do_not_pin_a_toolchain_of_their_own() {
    // `rust-toolchain.toml` pins 1.95.0 and carries the reason: "it compiles on my machine" is
    // almost always a toolchain difference. A workflow naming a version gives that pin a second
    // place to be wrong, and the workflow's copy is the one that silently wins.
    let pinned =
        fs::read_to_string(root().join("rust-toolchain.toml")).expect("the toolchain file exists");
    let version = pinned
        .lines()
        .find_map(|line| line.trim().strip_prefix("channel = "))
        .map(|value| value.trim().trim_matches('"').to_string())
        .expect("rust-toolchain.toml names a channel");

    for (name, text) in workflows() {
        if !text.contains("cargo ") {
            continue;
        }
        // A workflow may name the version -- it has to, to ask rustup for it -- but if it does,
        // the version it names has to be the pinned one.
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("toolchain:") else {
                continue;
            };
            let named = rest.trim().trim_matches('"');
            assert_eq!(
                named, version,
                "{name} asks for toolchain {named} while rust-toolchain.toml pins {version}. \
                 One of the two is going to win and it will not be the one a reader checks."
            );
        }
    }
}
