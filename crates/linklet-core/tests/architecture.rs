//! Rules that would otherwise be enforced only by remembering them.
//!
//! A rule in a document is a wish. A rule that fails `cargo test` is a rule.
//! This file turns the one architectural rule of this project into the second
//! kind, which is worth the odd bit of text reading.

use std::fs;
use std::path::Path;

/// Reads a crate's manifest, relative to this crate's directory.
///
/// `CARGO_MANIFEST_DIR` is the directory of the crate the test belongs to
/// (`crates/linklet-core`), baked in at compile time -- so this works no matter
/// which directory `cargo test` was invoked from. Hard-coding a relative path
/// would make the test depend on the caller's working directory, which is the
/// kind of test that passes on the author's machine and fails in CI.
fn sibling_manifest(crate_name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/linklet-core always has a parent directory")
        .join(crate_name)
        .join("Cargo.toml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The entries declared under `[dependencies]`, or an empty vector.
///
/// Deliberately a small parser rather than a set of `contains` calls. The
/// distinction it draws is the one that matters: an empty `[dependencies]`
/// table is *not* a dependency, a comment mentioning the word dependencies is
/// *not* a dependency, and a line under `[dev-dependencies]` belongs to a
/// different table -- test-only dependencies are allowed, because they are not
/// linked into the library a caller receives.
///
/// The first version of this test used a substring search and failed on the
/// empty table. That is the lesson recorded here: a test that inspects text
/// has to parse the text, not look at it.
fn declared_dependencies(crate_name: &str) -> Vec<String> {
    let text = sibling_manifest(crate_name);
    let mut in_dependencies = false;
    let mut found = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();

        // A table header switches which table we are in. `[dependencies.foo]`
        // is a dependency spelled the long way, so it counts.
        if line.starts_with('[') {
            in_dependencies =
                line.starts_with("[dependencies]") || line.starts_with("[dependencies.");
            continue;
        }

        if !in_dependencies {
            continue;
        }
        // Comments and blank lines are not entries. Without this, the comment
        // in the manifest that *explains* the empty table would be read as a
        // dependency -- which is exactly the failure mode this test exists to
        // avoid, one level up.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        found.push(line.to_string());
    }

    found
}

/// The architectural rule of this project, checked instead of trusted.
///
/// `linklet-core` must stay free of dependencies, because that is what makes
/// "no I/O in the core" impossible to break by accident rather than merely
/// discouraged. The moment someone adds `tokio` here to "just read one file",
/// the core stops being testable in microseconds and the property is gone --
/// silently, and without any other test failing. So this test fails instead.
///
/// It is also the demonstration of the point: the rule was written down in
/// `AGENTS.md` and `README.md` first, where it is a wish, and it became real
/// when it moved here.
/// The crates `linklet-core` may depend on, and why each one is allowed.
///
/// # Why this replaced a rule that said "nothing"
///
/// The original rule was that `linklet-core` depends on no crate at all, and the
/// reason behind it was never the count: it was that **core does no I/O**. Its
/// tests need no network, no files and no cleanup, so they run in milliseconds and
/// cannot fail for a reason outside the code. That property is worth defending and
/// it is still defended here.
///
/// What was wrong was the wording. "Depends on nothing" is satisfied by a rule and
/// not by a reason, so it stayed in force past the point where its reason applied
/// -- and while the crate registry looked unreachable it was read as a rule about
/// the whole project, which is how a SHA-256 came to be written by hand. Nothing
/// about a pure-computation library threatens the property above.
///
/// So the rule is narrower and explicit: **core may depend on crates that compute
/// and touch nothing.** Every entry is a decision, and adding one means arguing
/// that the crate does no I/O -- not that it would be convenient.
const ALLOWED_DEPENDENCIES: &[(&str, &str)] = &[(
    "serde_json",
    "parses and writes JSON, and does nothing else: no files, no sockets, no \
     clock, no environment. It replaced six hundred lines of hand-written codec \
     that sat on the path reading untrusted input from the network, which is the \
     last place to keep code whose bugs only a fuzzer finds.",
)];

#[test]
fn core_depends_only_on_crates_that_do_no_io() {
    let declared = declared_dependencies("linklet-core");
    let offenders: Vec<&String> = declared
        .iter()
        .filter(|line| {
            !ALLOWED_DEPENDENCIES
                .iter()
                .any(|(name, _)| line.contains(name))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "linklet-core must not depend on anything that does I/O, but it declares \
         {offenders:?}.\n\
         If the new code needs I/O, it belongs in linklet-adapters behind a trait \
         that linklet-core defines -- see AGENTS.md rule 1.\n\
         If it genuinely does no I/O, add it to ALLOWED_DEPENDENCIES above with a \
         sentence saying why, so the next reader sees the argument rather than the \
         exception."
    );
}

/// Guards the guard, for the allowlist.
///
/// A filter whose names all fail to match would pass every dependency through and
/// look green, which is the failure an allowlist is most likely to have. This
/// checks that each entry actually matches something the manifest declares, and
/// that each one carries a reason.
#[test]
fn the_allowlist_actually_matches_a_real_dependency() {
    let declared = declared_dependencies("linklet-core");

    for (name, reason) in ALLOWED_DEPENDENCIES {
        assert!(
            declared.iter().any(|line| line.contains(name)),
            "{name} is on the allowlist but is not declared. An entry that matches \
             nothing hides a typo, and a typo in a name here would let the real \
             crate through unexamined."
        );
        assert!(
            !reason.trim().is_empty(),
            "{name} is allowed with no reason given, which makes it an exception \
             rather than a decision."
        );
    }
}

/// Guards the guard.
///
/// `core_has_no_dependencies` passing is only evidence if the parser behind it
/// can actually see a dependency. A reader that returns nothing is
/// indistinguishable from a manifest with nothing in it, and that is how an
/// architecture test quietly becomes decoration. This checks the reader against
/// a crate that does have dependencies.
#[test]
fn the_reader_can_actually_see_dependencies() {
    let adapters = declared_dependencies("linklet-adapters");

    assert!(
        adapters.iter().any(|line| line.contains("linklet-core")),
        "the dependency reader found nothing in linklet-adapters, which does \
         depend on linklet-core -- so `core_has_no_dependencies` proves nothing \
         until this is fixed. Found: {adapters:?}"
    );
}
