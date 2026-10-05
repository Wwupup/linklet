//! The CI workflows, checked against the claims made about them.
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
//! # The second claim is about platforms, and it is the one that was added late
//!
//! `docs/testing.md` and `README.md` both say the gates run on **Windows and Linux** -- the adapter
//! layer has a backend per platform, and a job on one of them is a job that cannot see the other.
//! That was not always true: the job was Windows-only while the adapter tests called `tasklist`
//! and `ipconfig` unconditionally, and the reasoning in the workflow said so. It stopped being true
//! at `docs/ROADMAP.md` M11, and nothing failed when the documentation moved ahead of the workflow
//! -- which is what this test is for.
//!
//! # What it deliberately does not check
//!
//! Whether the workflow is *correct* -- whether the runner image exists, whether the action
//! versions are current, whether the YAML is valid. Those need a YAML parser and a network, and a
//! test that half-parses YAML would be the "looks at text instead of parsing it" failure
//! `tests/architecture.rs` already records. What is checked here is the mechanical half: the files
//! are present, they call the script, they do not duplicate it, and they run where the documents
//! say they run.

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

/// The platforms the gates are claimed to run on, in the runner names the workflow uses.
///
/// Two, and not one: the adapter layer has two halves -- `tasklist`, `taskkill`, `ipconfig` and
/// `route` on one side, `/proc` and `ip` on the other -- and a job on a single platform leaves
/// the other half of it unexercised on every push. `docs/testing.md` and `README.md` both say
/// the gates run on Windows and Linux, so this is a claim about the repository that the
/// repository can check.
const RUNNERS: [&str; 2] = ["windows-latest", "ubuntu-latest"];

#[test]
fn the_gates_run_on_every_platform_the_tool_supports() {
    let (name, text) = workflows()
        .into_iter()
        .find(|(name, _)| name == "verify.yml")
        .expect("verify.yml is the workflow every push runs the gates from");

    for runner in RUNNERS {
        assert!(
            text.contains(runner),
            "{name} never names {runner}, so the gates do not run there -- while \
             docs/testing.md and README.md both say they do. A platform dropped from this \
             list is half the adapter layer untested on every push, and nothing else says so."
        );
    }
}

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

#[test]
fn every_action_is_pinned_to_a_version_tag_and_not_a_branch() {
    // **A branch is a moving target, and one was already here.** `dtolnay/rust-toolchain@master`
    // meant the toolchain installer could change under a green build with nothing in this
    // repository having changed -- the opposite of what `rust-toolchain.toml` pins the toolchain
    // for. It has a `v1` tag; both workflows use that now.
    //
    // A major tag is the deliberate choice over a commit SHA. A SHA is the most reproducible and
    // the least maintainable, and the failure that prompted this test is the maintainability
    // half: a SHA nobody bumps stays on a runtime GitHub eventually retires, and a retired
    // runtime arrives as an email rather than as a red build. `docs/VERSIONING.md` says how the
    // pins are reviewed, and why `actions/checkout@v4` was one of them.
    for (name, text) in workflows() {
        for line in text.lines() {
            let line = line.trim();
            let rest = line
                .strip_prefix("- uses:")
                .or_else(|| line.strip_prefix("uses:"))
                .map(str::trim);
            let Some(reference) = rest else {
                continue;
            };

            let Some((action, version)) = reference.trim_matches('"').split_once('@') else {
                panic!(
                    "{name}: `{reference}` names no version at all, so it tracks the default \
                        branch"
                );
            };

            // Forty hex characters is a commit SHA and is allowed. Everything else has to look
            // like a tag, because that is the thing a person can look up.
            let is_sha = version.len() == 40 && version.chars().all(|c| c.is_ascii_hexdigit());
            let is_tag = version
                .strip_prefix('v')
                .and_then(|rest| rest.chars().next())
                .is_some_and(|first| first.is_ascii_digit());
            assert!(
                is_sha || is_tag,
                "{name}: `{action}@{version}` is neither a version tag nor a commit SHA. A \
                 branch moves under a green build; pin `vN` or a SHA."
            );
        }
    }
}
