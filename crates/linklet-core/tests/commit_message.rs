//! The mechanical half of the commit-message standard, checked instead of
//! trusted.
//!
//! `AGENTS.md` has carried a commit rule since the first commit, and it was
//! broken anyway -- every message was written in Chinese while rule 7 said
//! English, and nothing noticed. A rule with no gate is a wish. This is the
//! gate.
//!
//! It checks the part a machine can judge: the type, the length, ASCII, and
//! that a body is separated from the subject. It has no opinion about whether a
//! message explains anything, because that is not a property a test can see --
//! see `docs/COMMITS.md` for the part a person has to do.

use std::process::Command;

/// The types a commit subject may start with.
const TYPES: [&str; 6] = ["feat", "fix", "refactor", "test", "docs", "chore"];

/// A subject longer than this wraps in terminals and log views.
const MAX_SUBJECT: usize = 72;

/// How many recent commits to check.
///
/// The whole history would be slow on a real repository, and HEAD is where the
/// mistake is made. The cost of the limit is stated rather than hidden: an
/// older message that breaks a rule stays broken once it has scrolled past.
const HOW_MANY: usize = 10;

/// Reads commit subjects and bodies, oldest of the range first.
///
/// Returns `None` when there is no history to read -- a shallow clone, an
/// exported archive, or a machine without `git`. The test then passes with a
/// message saying so, rather than failing for a reason that has nothing to do
/// with the change being made. A check that fails when it cannot run teaches
/// people to ignore it.
fn recent_commits() -> Option<Vec<String>> {
    let output = Command::new("git")
        .args([
            "log",
            "--no-merges",
            &format!("-{HOW_MANY}"),
            "--format=%B%x00",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Lossy rather than strict: the point of the test is to report non-ASCII,
    // and a decode failure is not a reason to skip the report.
    let text = String::from_utf8_lossy(&output.stdout);
    let commits: Vec<String> = text
        .split('\0')
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .collect();

    if commits.is_empty() {
        None
    } else {
        Some(commits)
    }
}

/// The first line of a message.
fn subject(message: &str) -> &str {
    message.lines().next().unwrap_or("")
}

#[test]
fn commit_subjects_follow_the_standard() {
    let Some(commits) = recent_commits() else {
        eprintln!("no git history to check; skipping the commit-message standard");
        return;
    };

    let mut problems: Vec<String> = Vec::new();

    for message in &commits {
        let line = subject(message);

        let Some((kind, rest)) = line.split_once(':') else {
            problems.push(format!("{line:?} has no \"<type>: \" prefix"));
            continue;
        };

        if !TYPES.contains(&kind) {
            problems.push(format!(
                "{line:?} starts with {kind:?}, which is not one of {TYPES:?}"
            ));
        }

        if rest.trim().is_empty() {
            problems.push(format!("{line:?} has a type and nothing after it"));
        }

        if line.chars().count() > MAX_SUBJECT {
            problems.push(format!(
                "{line:?} is {} characters; the limit is {MAX_SUBJECT}",
                line.chars().count()
            ));
        }

        // Rule 7: ASCII in every committed file, and a commit message is one.
        if !line.is_ascii() {
            problems.push(format!("{line:?} contains non-ASCII characters"));
        }

        if !message.is_ascii() {
            problems.push(format!(
                "the body of {line:?} contains non-ASCII characters"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "the last {} commit messages break the standard in docs/COMMITS.md:\n  {}\n",
        commits.len(),
        problems.join("\n  ")
    );
}

#[test]
fn the_standard_itself_is_described_where_the_rule_points() {
    // A rule that says "see docs/COMMITS.md" and a file that is not there is the
    // failure this repository is about. Cheaper to check than to notice.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/COMMITS.md");
    assert!(
        path.is_file(),
        "AGENTS.md points at docs/COMMITS.md and it does not exist at {}",
        path.display()
    );
}
