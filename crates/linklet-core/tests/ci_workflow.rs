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

/// One workflow's text, by file name.
fn workflow(name: &str) -> String {
    workflows()
        .into_iter()
        .find(|(file, _)| file == name)
        .unwrap_or_else(|| panic!("{name} should exist under .github/workflows"))
        .1
}

/// A workflow's text **with its comments removed**, which is the part that is behaviour.
///
/// The checks below started out searching the whole file, and the first run of them failed on
/// comments in `release.yml` that *explain* the rules -- one naming `Compress-Archive` while saying
/// why the archive is not built with it, one naming `integrations/README.md`. A workflow's prose is
/// not its commands, and a check that cannot tell them apart is the "looks at text instead of
/// parsing it" failure `tests/architecture.rs` records. Stripping comments is not a real parse; it
/// is enough to make these two checks about what the workflow *does*.
fn commands(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Whether `haystack` names `needle` as a whole path element.
///
/// A plain substring test is wrong here in a way that is easy to miss: `integrations/README.md`
/// contains `README.md`, so a payload check written with `contains` would report a release that
/// carries the README twice and call it a second list. That is a false positive today and a false
/// negative the day somebody writes `docs/LICENSE`.
fn mentions(haystack: &str, needle: &str) -> bool {
    let is_path_char =
        |c: char| c.is_ascii_alphanumeric() || matches!(c, '/' | '\\' | '_' | '-' | '.');

    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let clear_before = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_path_char(c));
        let clear_after = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_path_char(c));
        if clear_before && clear_after {
            return true;
        }
        from = start + 1;
    }
    false
}

/// The files the release archive carries, read out of `tools/make_release.sh`.
///
/// Read from the script rather than written down a second time here, because the check below is
/// that there is **one** list. A test with its own copy would pass while the two copies disagreed,
/// which is the failure this whole file is about.
fn release_payload() -> Vec<String> {
    let script = fs::read_to_string(root().join("tools/make_release.sh"))
        .expect("tools/make_release.sh is where the release is assembled");

    let start = script
        .find("payload=(")
        .expect("the release script names its payload in a list")
        + "payload=(".len();
    let rest = &script[start..];
    let end = rest.find(")\n").expect("the payload list is closed");

    // Split on whitespace rather than on lines: the list is written one item per line, and a
    // rewrite that put two on one line should not quietly change what is checked.
    rest[..end].split_whitespace().map(str::to_string).collect()
}

/// The target triples the release ships, read out of the same script's two variables.
fn release_triples() -> Vec<String> {
    let script = fs::read_to_string(root().join("tools/make_release.sh"))
        .expect("tools/make_release.sh is where the release is assembled");

    ["windows=", "linux="]
        .iter()
        .map(|prefix| {
            let start = script
                .find(prefix)
                .unwrap_or_else(|| panic!("the release script names {prefix}"))
                + prefix.len();
            script[start..]
                .lines()
                .next()
                .expect("a value follows the name")
                .trim()
                .to_string()
        })
        .collect()
}

#[test]
fn the_release_carries_one_archive_for_both_platforms() {
    // **The archive is assembled where the zip tool writes the Unix mode, and this is the check on
    // that.** `Compress-Archive` writes `external_attr = 0` on every entry -- measured, it puts a
    // Linux binary on disk at mode 600 -- so an archive the Windows image made would ship a Linux
    // executable nobody can run. Info-ZIP `zip` writes the mode, and it is on the Ubuntu image.
    let release = workflow("release.yml");
    let release_commands = commands(&release);

    assert!(
        !release_commands.contains("Compress-Archive"),
        "release.yml uses `Compress-Archive`, which writes no Unix mode -- a Linux binary in that \
         archive arrives at mode 600. tools/make_release.sh extracts what it wrote and checks the \
         bit; the header of that script has the measurement."
    );

    let publish = release
        .split("\n  publish:")
        .nth(1)
        .expect("release.yml has a job that assembles the archive");
    assert!(
        publish.contains("ubuntu-latest"),
        "the job that assembles the archive does not run on the image whose `zip` writes the Unix \
         mode, which is the whole reason it is not the Windows job"
    );
}

#[test]
fn the_release_builds_a_binary_for_every_platform_it_ships() {
    // Three places have to agree about which platforms this release is for: the triples the
    // assembling script reads, the matrix the build job runs, and the `bin/` directories a user
    // ends up with. The first two are compared here; the third is checked by the script itself,
    // because it is the one that can look inside the archive it just wrote.
    let release = workflow("release.yml");
    let triples = release_triples();

    assert_eq!(
        triples.len(),
        2,
        "the release script should name exactly the two platforms this release ships, and it named \
         {triples:?}"
    );

    for triple in &triples {
        assert!(
            release.contains(triple),
            "tools/make_release.sh hands over `{triple}` and release.yml never builds it, so the \
             archive would be assembled from a directory that is not there"
        );
    }
}

#[test]
fn the_payload_list_is_the_release_script_s_and_not_the_workflow_s() {
    // The files a release carries that are **not** built used to be listed in the workflow. They
    // are in `tools/make_release.sh` now, for the same reason the four gates are in
    // `tools/verify.ps1`: one list, in a file a person can run, rather than a second one in a file
    // only a runner executes.
    let release = commands(&workflow("release.yml"));
    let payload = release_payload();

    assert!(
        !payload.is_empty(),
        "the payload list in tools/make_release.sh read as empty, so the checks below would pass \
         while checking nothing"
    );

    for item in &payload {
        assert!(
            !mentions(&release, item),
            "release.yml names `{item}`, which tools/make_release.sh is the one place for. Two \
             lists of what a release carries is two lists that can drift, and the release is the \
             worst place to find out which one won."
        );
    }

    assert!(
        payload.iter().any(|item| item == "integrations"),
        "the payload should still carry `integrations` -- the client entry and the skill are half \
         of what a release is for, and they are the half a `bin/`-only package loses silently"
    );
}

#[test]
fn the_readers_that_check_a_workflow_can_actually_see_a_command() {
    // Guards the guards, for the reason `docs/testing.md` gives: a reader that returns nothing is
    // indistinguishable from a workflow with nothing in it, and a `mentions` that always answered
    // no would make the payload check above pass forever.
    let text = "# a comment naming Compress-Archive and integrations/README.md\nrun: echo hi\n";
    assert!(
        commands(text).contains("echo hi"),
        "the comment stripper threw away a command as well"
    );
    assert!(
        !commands(text).contains("Compress-Archive"),
        "the comment stripper left a comment behind, so every check on it is really a check on \
         prose"
    );

    // The payload items are repository-relative paths, so the unit a mention is judged in is the
    // whole path and not a file name at the end of one.
    assert!(
        mentions("cp docs/MCP.md dist/", "docs/MCP.md"),
        "a payload path should count as named"
    );
    assert!(
        mentions("payload: README.md", "README.md"),
        "a payload item at the end of a line should count as named"
    );
    assert!(
        !mentions("integrations/README.md", "README.md"),
        "`integrations/README.md` is not a mention of the payload item `README.md`, and treating it \
         as one is how the payload check reported a false positive the first time it ran"
    );
}

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
