//! The specification for where a transfer may write.
//!
//! `docs/transfer.md` T1 is the most severe item in that document: the destination
//! is the caller's, so a path the caller chooses is a path an attacker chooses if
//! anything upstream is confused, and the outcome is an arbitrary file write as
//! SYSTEM on someone else's machine. **It is more severe than anything in the
//! framing list, because a framing bug is a refusal and this is a write.**
//!
//! Every rule in `src/transfer.rs` has a test here. The Windows-specific ones get
//! more than one, because they are the ones a reader is most likely to think are
//! already handled by the `..` check.

use linklet_core::transfer::{Destination, MAX_TRANSFER_BYTES, PathError};

/// A root these tests can use on Windows.
const ROOT: &str = r"C:\linklet";

fn destination() -> Destination {
    Destination::new(ROOT).expect("an absolute root")
}

/// Asserts that a request is refused, and returns the reason for a closer look.
fn refused(requested: &str) -> PathError {
    destination()
        .resolve(requested)
        .expect_err(&format!("{requested:?} should have been refused"))
}

// --- what is allowed ---------------------------------------------------------

#[test]
fn a_plain_relative_path_lands_under_the_root() {
    let resolved = destination().resolve("build.exe").expect("a plain name");
    assert_eq!(resolved, std::path::Path::new(ROOT).join("build.exe"));
}

#[test]
fn a_relative_path_with_directories_lands_under_the_root() {
    // Subdirectories are allowed and are the normal case: a build is put somewhere
    // rather than in the root of whatever the operator configured.
    let resolved = destination()
        .resolve(r"artifacts\latest\build.exe")
        .expect("a nested name");
    assert_eq!(
        resolved,
        std::path::Path::new(ROOT)
            .join("artifacts")
            .join("latest")
            .join("build.exe")
    );
}

#[test]
fn an_absolute_path_inside_the_root_is_allowed() {
    // Refusing every absolute path would be simpler and would break the case an
    // operator meets first: they configured the root, so they know where it is and
    // will type it.
    let resolved = destination()
        .resolve(r"C:\linklet\build.exe")
        .expect("inside the root");
    assert_eq!(resolved, std::path::Path::new(ROOT).join("build.exe"));
}

#[test]
fn the_root_itself_is_case_insensitive() {
    // Windows paths are, so a comparison that is not would refuse a path the
    // operating system would have accepted -- and, worse, the mirror image of that
    // mistake is accepting `C:\linkletevil` for the root `C:\linklet`.
    destination()
        .resolve(r"c:\LINKLET\build.exe")
        .expect("the same directory in another case");
}

// --- T1: the obvious escape --------------------------------------------------

#[test]
fn a_parent_component_is_refused_by_name() {
    // The headline case from the document.
    let error = refused(r"..\..\Windows\System32\drivers\etc\hosts");
    assert!(
        matches!(error, PathError::Parent { .. }),
        "expected a parent refusal, got {error:?}"
    );
    assert!(error.to_string().contains(".."));
}

#[test]
fn a_parent_component_is_refused_anywhere_in_the_path() {
    // Not just at the front. `a\..\..\b` escapes by the same route.
    for requested in [
        r"..\hosts",
        r"a\..\b",
        r"a\b\..\..\c",
        r"a\..\..\Windows\hosts",
        "../hosts",
    ] {
        let error = refused(requested);
        assert!(
            matches!(error, PathError::Parent { .. }),
            "{requested:?} gave {error:?}"
        );
    }
}

#[test]
fn an_absolute_path_outside_the_root_is_refused() {
    let error = refused(r"C:\Windows\System32\drivers\etc\hosts");
    assert!(
        matches!(error, PathError::OutsideRoot { .. }),
        "expected an outside-root refusal, got {error:?}"
    );
}

#[test]
fn a_root_that_is_a_prefix_of_another_directory_is_not_enough() {
    // The string-prefix mistake: `C:\linkletevil` starts with `C:\linklet`, and a
    // comparison on the string form would accept it. Components are compared for
    // this reason, and this test is the reason that reason is written down.
    let error = refused(r"C:\linkletevil\build.exe");
    assert!(
        matches!(error, PathError::OutsideRoot { .. }),
        "expected an outside-root refusal, got {error:?}"
    );
}

// --- T1: the Windows rules a `..` check does not cover ------------------------

#[test]
fn an_alternate_data_stream_is_refused() {
    // `build.exe:evil` is not a file with a colon in its name. It is a stream on
    // build.exe: it holds bytes that no directory listing shows and no ordinary
    // tool will read, which makes it a way to write something the operator cannot
    // see and cannot easily remove.
    let error = refused(r"build.exe:evil");
    assert!(
        matches!(error, PathError::Colon { .. }),
        "expected a colon refusal, got {error:?}"
    );
}

#[test]
fn a_stream_on_an_absolute_path_is_refused_too() {
    refused(r"C:\linklet\build.exe:evil");
}

#[test]
fn a_drive_relative_path_is_refused() {
    // `C:build.exe` is not `C:\build.exe`. It is build.exe relative to whatever the
    // current directory happens to be on drive C -- a different file depending on
    // how the process was started, which is exactly the kind of thing a root is
    // supposed to remove.
    let error = refused("C:build.exe");
    assert!(
        matches!(error, PathError::Colon { .. }),
        "expected a colon refusal, got {error:?}"
    );
}

#[test]
fn a_network_share_is_refused() {
    // A UNC path writes to another machine entirely, outside any root this process
    // configured.
    for requested in [r"\\server\share\build.exe", r"\\?\C:\linklet\build.exe"] {
        let error = refused(requested);
        assert!(
            matches!(error, PathError::Network { .. }),
            "{requested:?} gave {error:?}"
        );
    }
}

#[test]
fn a_component_ending_in_a_dot_or_a_space_is_refused() {
    // Windows strips them, so `build.exe.` and `build.exe ` and `build.exe` are one
    // file. A check that compared the names literally would pass a name that
    // becomes a different one on disk -- which is how a name-based rule is evaded.
    for requested in [r"build.exe.", r"build.exe ", r"a.\b", r"a \b"] {
        let error = refused(requested);
        assert!(
            matches!(error, PathError::TrailingDotOrSpace { .. }),
            "{requested:?} gave {error:?}"
        );
    }
}

#[test]
fn every_reserved_device_name_is_refused_with_and_without_an_extension() {
    // `NUL` is the one that matters: writing to it **succeeds and discards the
    // bytes**, so a push that checked its digest against what it wrote would report
    // success having written nothing.
    let names = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM9", "LPT1", "LPT9", "nul", "Nul", "aux.txt",
        "com1.log", "NUL.dat",
    ];

    for name in names {
        let error = refused(name);
        assert!(
            matches!(error, PathError::Reserved { .. }),
            "{name:?} gave {error:?}"
        );
    }
}

#[test]
fn a_reserved_name_in_a_subdirectory_is_refused_too() {
    // The check is per component, not on the whole path.
    refused(r"logs\NUL");
}

#[test]
fn an_ordinary_name_that_merely_starts_like_a_device_is_allowed() {
    // `console.log` is a file. The rule is the *stem*, and a rule that matched on
    // prefixes would refuse names people actually use.
    destination()
        .resolve("console.log")
        .expect("an ordinary name");
    destination().resolve("com10.txt").expect("not a device");
    destination().resolve("nullify").expect("not a device");
}

// --- the shapes that are not paths at all ------------------------------------

#[test]
fn an_empty_path_is_refused() {
    assert_eq!(refused(""), PathError::Empty);
    assert_eq!(refused("   "), PathError::Empty);
}

#[test]
fn a_path_with_a_nul_byte_is_refused() {
    // It cannot reach the filesystem API, which is the point: a string containing
    // one means something upstream is not doing what it thinks it is.
    assert_eq!(refused("build\0.exe"), PathError::NulByte);
}

// --- the root itself ---------------------------------------------------------

#[test]
fn a_root_that_is_relative_is_refused_at_construction() {
    // Because then "inside the root" would depend on the working directory, which
    // is the thing the root exists to remove.
    for root in ["linklet", r".\linklet", "builds"] {
        let error = Destination::new(root).expect_err("a relative root");
        assert!(
            matches!(error, PathError::BadRoot { .. }),
            "{root:?} gave {error:?}"
        );
    }
}

#[test]
fn an_unusable_root_is_refused_at_construction() {
    for root in [
        "",
        "   ",
        r"\\server\share",
        "C:\\linklet\\build.exe:stream",
    ] {
        assert!(
            Destination::new(root).is_err(),
            "{root:?} should not be a usable root"
        );
    }
}

#[test]
fn every_refusal_quotes_what_caused_it() {
    // A caller reading one of these is a person looking at a path that did not
    // work. "invalid path" would leave them comparing it against a manual.
    let cases: [(PathError, &str); 5] = [
        (refused(r"..\hosts"), ".."),
        (refused("nul"), "device"),
        (refused(r"\\server\share"), "share"),
        (refused("build.exe:evil"), "colon"),
        (refused(""), "empty"),
    ];

    for (error, expected) in cases {
        let text = error.to_string();
        assert!(
            text.to_lowercase().contains(&expected.to_lowercase()),
            "{error:?} rendered as {text:?}, which does not mention {expected:?}"
        );
    }
}

// --- the ceiling, which is the other half of T3 and T11 ----------------------

#[test]
fn the_transfer_ceiling_is_where_the_document_says_it_is() {
    // A policy number rather than a protocol constant, and the number decides
    // whether push is useful at all -- so it is asserted rather than left to be
    // discovered by someone whose build is one byte too large.
    assert_eq!(MAX_TRANSFER_BYTES, 4 * 1024 * 1024 * 1024);
}
