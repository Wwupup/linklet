//! Every document is reachable, and every reference points at something.
//!
//! Two documentation faults have been found by hand in this repository, and a
//! fault found twice by hand is a fault that belongs in a test:
//!
//! 1. **A document nobody can find.** `docs/testing.md` was written, referenced
//!    from the changelog, and listed in neither the index nor the routing table.
//!    The content was fine; the reader never arrived.
//! 2. **A reference to nothing.** The command line said "see docs/MCP.md" before
//!    that file existed, and the changelog named `docs/testing.md` before that
//!    one did.
//!
//! Both are the same failure as a doc comment promising behaviour no test pins:
//! a claim about the repository that the repository does not support. The
//! difference is that these can be checked by a machine, so they are.
//!
//! # What this deliberately does not check
//!
//! Whether a document is *good*, or whether the index describes it accurately.
//! Those are judgements. What is checked is the mechanical half: that a file with
//! content is reachable from the two files a reader starts at, and that a path
//! written down is a path that exists.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root, from this crate's directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root is two levels up from a crate")
}

/// Every `docs/*.md`, by file name.
fn documents() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root().join("docs"))
        .expect("docs/ exists")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The two files a reader is expected to start at.
const ENTRY_POINTS: [&str; 2] = ["AGENTS.md", "docs/INDEX.md"];

#[test]
fn every_document_is_reachable_from_an_entry_point() {
    let mut unreachable: Vec<String> = Vec::new();

    for name in documents() {
        // `INDEX.md` does not need to list itself.
        if name == "INDEX.md" {
            continue;
        }
        // The full `docs/name` form, not the bare file name.
        //
        // The first version accepted either, and a mutation test caught what that
        // cost: deleting the reference from the routing table still passed,
        // because the file name appeared again in the table further down. A check
        // that accepts a mention anywhere is not a check that a reader can find
        // the thing -- and "a mention in a table nobody reads top to bottom" is
        // exactly the state `docs/testing.md` was in.
        let referent = format!("docs/{name}");
        let mentioned = ENTRY_POINTS.iter().any(|entry| {
            fs::read_to_string(root().join(entry))
                .map(|text| text.contains(&referent))
                .unwrap_or(false)
        });
        if !mentioned {
            unreachable.push(referent);
        }
    }

    assert!(
        unreachable.is_empty(),
        "these documents exist and are named in neither {} nor {}:\n  {}\n\
         A document nobody can find is a document nobody reads, and its content \
         being good does not help.",
        ENTRY_POINTS[0],
        ENTRY_POINTS[1],
        unreachable.join("\n  ")
    );
}

#[test]
fn every_path_written_down_in_the_entry_points_exists() {
    // Scoped to the entry points rather than to every document: these are the
    // files a reader follows, and a broken path here sends them nowhere. A path
    // mentioned in passing inside another document is a different, weaker claim.
    let mut missing: Vec<String> = Vec::new();

    for entry in ENTRY_POINTS {
        let text = fs::read_to_string(root().join(entry)).expect("an entry point exists");
        for candidate in references(&text) {
            if !root().join(&candidate).exists() {
                missing.push(format!("{entry} names {candidate}"));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "a document points at something that is not there:\n  {}",
        missing.join("\n  ")
    );
}

/// The repository-relative paths a markdown file refers to.
///
/// Scans the whole text for `docs/...` and `crates/...` runs, rather than only
/// looking inside backtick pairs. The first version split on backticks and took
/// every other piece, which missed a reference written inside a markdown table
/// cell as `[`docs/MCP.md`]` -- there the backticks are nested in brackets, so
/// the pairing is off by one and the reference is invisible. It missed it
/// silently, which is the failure mode this whole file is about.
///
/// Narrow in what it accepts: a path must carry a known extension, so prose that
/// happens to mention a directory is not treated as a reference to open.
fn references(text: &str) -> Vec<String> {
    let mut found = Vec::new();

    let starts = text
        .match_indices("docs/")
        .chain(text.match_indices("crates/"))
        .map(|(start, _)| start);

    for start in starts {
        let candidate: String = text[start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '`' | ')' | ']' | ',' | '|'))
            .collect();
        let candidate = candidate.trim_end_matches('.').to_string();

        let has_a_known_extension = matches!(
            candidate.rsplit('.').next(),
            Some("md" | "rs" | "toml" | "ps1" | "txt" | "json")
        );
        if has_a_known_extension && !candidate.ends_with('/') && !found.contains(&candidate) {
            found.push(candidate);
        }
    }

    found
}

#[test]
fn the_reader_that_checks_references_can_actually_see_one() {
    // Guards the guard, for the same reason `architecture.rs` does: a reader that
    // returns nothing is indistinguishable from a document with no references,
    // and that is how this check quietly becomes decoration.
    let found = references("see `docs/MCP.md` and `crates/linklet-core/src/lib.rs` for more");
    assert_eq!(
        found,
        vec![
            "docs/MCP.md".to_string(),
            "crates/linklet-core/src/lib.rs".to_string()
        ]
    );

    assert!(
        references("no backticked paths here").is_empty(),
        "the reader should find nothing in prose with no references"
    );
}
