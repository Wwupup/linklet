//! Rule 7, checked instead of remembered: ASCII in every committed file.
//!
//! `AGENTS.md` has required this since the first commit -- *"English everywhere. ASCII in every
//! committed file; non-ASCII test data is written as escapes"* -- and nothing enforced it. The
//! exception in that sentence is the reason the rule can be strict: a test that needs bytes which
//! are not ASCII writes them as escapes (`\u{4e2d}`, `0xEF, 0xBB, 0xBF`), so the *source* stays
//! ASCII and the bytes are produced at run time. `crates/linklet-client/tests/search.rs` and the
//! encoding fixtures are the worked examples.
//!
//! # Why this needed a test rather than more care
//!
//! Seven em dashes reached `docs/VERSIONING.md`, one reached `README.md`, and three reached
//! `docs/smoke.md` -- every one of them from the same hand, and none of them noticed, because a
//! prose dash looks like prose. That is the shape of a rule that is written down and not applied:
//! the writer is concentrating on the sentence.
//!
//! # What is checked, and what is not
//!
//! Every file under the repository whose extension says it is text. **Not** their meaning, their
//! language, or whether an em dash would have been better prose -- only whether the bytes are
//! ASCII. A byte above 127 in one of these files came from a keyboard layout, a paste, or a
//! generator, and the fix is the same in all three cases.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root is two levels up from a crate")
}

/// Extensions that mean "this file is text, and a person wrote or generated it".
///
/// `.lock` is included on purpose: a lockfile is committed here (it is the record of the exact
/// versions a build was verified against) and a crate name is ASCII, so a non-ASCII byte in it
/// would be evidence of something nobody intended.
const TEXT: [&str; 10] = [
    "rs", "md", "toml", "yml", "yaml", "ps1", "cmd", "txt", "json", "lock",
];

/// Directories that are build output or history, and hold no committed text.
const SKIP: [&str; 4] = [".git", "target", "dist", "node_modules"];

/// Every text file under `dir`, recursively.
fn text_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            text_files(&path, found);
            continue;
        }

        let text = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| TEXT.contains(&extension));
        if text {
            found.push(path);
        }
    }
}

#[test]
fn every_committed_text_file_is_ascii() {
    let mut files = Vec::new();
    text_files(&root(), &mut files);

    // **The guard on the guard**, in this repository's habit: the assertion below passes
    // trivially if the walker found nothing, and a walker that returns nothing is
    // indistinguishable from a repository with nothing in it.
    assert!(
        files.len() > 50,
        "the walker found only {} text files, so this check proves nothing; the \
         repository has far more than that",
        files.len()
    );

    let mut offenders: Vec<String> = Vec::new();

    for path in &files {
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let count = bytes.iter().filter(|byte| **byte > 127).count();
        if count == 0 {
            continue;
        }

        // The first offender's character is named, because "3 non-ASCII bytes" does not say
        // whether it is an em dash, a smart quote, or a whole word in another script -- and the
        // three have different fixes.
        let text = String::from_utf8_lossy(&bytes);
        let example = text
            .chars()
            .find(|character| !character.is_ascii())
            .map(|character| format!(" (first is U+{:04X})", character as u32))
            .unwrap_or_default();

        offenders.push(format!(
            "{}: {count} byte(s){example}",
            path.strip_prefix(root()).unwrap_or(path).display()
        ));
    }

    assert!(
        offenders.is_empty(),
        "AGENTS.md rule 7 requires ASCII in every committed file, and non-ASCII test \
         data is written as escapes so that it can be. These are not:\n  {}\n\
         Write the character as an escape, or use the `--` this repository uses for a \
         dash everywhere else.",
        offenders.join("\n  ")
    );
}
