//! The specification for the decisions a transfer makes before it touches anything.
//!
//! `docs/transfer.md` T1 defines filesystem-wide paths under the agent account.
//! Relative paths use the startup directory; no directory containment is imposed.
//!
//! Every rule in `src/transfer.rs` has a test here: the paths it refuses (T1), the
//! sizes it refuses before a chunk is read (T3), and the arithmetic it keeps while
//! the chunks arrive (T4, T5, T6, T7, T11). The Windows-specific path rules get more
//! than one test each to cover ordinary filenames and special objects.
//!
//! What is *not* here is anything that needs a file or a socket: whether the
//! destination is a symlink, what is on disk, and the rename. Those are
//! `linklet-adapters`' business and are tested against a real filesystem.

use linklet_core::transfer::{
    Destination, MAX_TRANSFER_BYTES, Manifest, ManifestError, PathError, Receiving, Rules, Sending,
    TransferError, verify_digest,
};

/// An absolute base directory in the native platform syntax.
#[cfg(windows)]
const BASE: &str = r"C:\linklet";
#[cfg(not(windows))]
const BASE: &str = "/linklet";

/// The same base spelled for the Windows rules, used by every test that names one of them.
///
/// A literal rather than `BASE`, because the Windows rule set has to be exercised *with a
/// Windows base*: `C:\linklet` is not an absolute path on Linux, so the Windows tests built on
/// the platform's own base would fail at the fixture instead of at the rule.
const WINDOWS_BASE: &str = r"C:\linklet";

/// The platform's own separator, for building a path that is more than one component.
///
/// Backslash is a separator to `resolve` on Windows and an ordinary character on Linux, so a
/// test that spells a nested path with one is testing Windows on both -- see the note on
/// `BASE`. A test that means "a name with directories in it" has to say so in the syntax of the
/// machine it is running on.
#[cfg(windows)]
const SEP: char = '\\';
#[cfg(not(windows))]
const SEP: char = '/';

/// A nested relative path, in this platform's syntax.
fn nested(parts: &[&str]) -> String {
    parts.join(&SEP.to_string())
}

/// An absolute path that is not under the base, in this platform's syntax.
#[cfg(windows)]
fn outside_base() -> String {
    r"C:\Windows\System32\drivers\etc\hosts".to_string()
}
#[cfg(not(windows))]
fn outside_base() -> String {
    "/etc/hosts".to_string()
}

/// A destination under the rules of the machine this is running on.
fn destination() -> Destination {
    Destination::new(BASE).expect("an absolute base")
}

/// A destination under the Windows rules, on any machine.
fn windows() -> Destination {
    Destination::with_rules(WINDOWS_BASE, Rules::Windows).expect("an absolute Windows base")
}

/// A destination under the POSIX rules, on any machine.
fn posix() -> Destination {
    Destination::with_rules("/linklet", Rules::Posix).expect("an absolute POSIX base")
}

/// Asserts that a request is refused by the platform's own rules.
fn refused(requested: &str) -> PathError {
    destination()
        .resolve(requested)
        .expect_err(&format!("{requested:?} should have been refused"))
}

// --- what is allowed ---------------------------------------------------------

#[test]
fn a_plain_relative_path_lands_under_the_base() {
    let resolved = destination().resolve("build.exe").expect("a plain name");
    assert_eq!(resolved, std::path::Path::new(BASE).join("build.exe"));
}

#[test]
fn a_relative_path_with_directories_lands_under_the_base() {
    // Subdirectories are allowed and are the normal case: a build is put somewhere
    // relative to the startup directory.
    let requested = nested(&["artifacts", "latest", "build.exe"]);
    let resolved = destination().resolve(&requested).expect("a nested name");
    assert_eq!(
        resolved,
        std::path::Path::new(BASE)
            .join("artifacts")
            .join("latest")
            .join("build.exe")
    );
}

#[test]
fn an_absolute_path_inside_the_base_is_allowed() {
    // Absolute paths are used directly, even when they happen to be under the base.
    let requested = std::path::Path::new(BASE).join("build.exe");
    let resolved = destination()
        .resolve(&requested.to_string_lossy())
        .expect("inside the base");
    assert_eq!(resolved, std::path::Path::new(BASE).join("build.exe"));
}

// --- absolute path spelling --------------------------------------------------

#[test]
fn absolute_paths_preserve_the_callers_case() {
    for requested in [
        "/LINKLET/build.exe",
        "/linklet/build.exe",
        "/linkletEvil/build.exe",
    ] {
        assert_eq!(
            posix().resolve(requested).expect("an absolute path"),
            std::path::PathBuf::from(requested)
        );
    }
}

// --- what POSIX allows that Windows does not --------------------------------

#[test]
fn names_that_are_ordinary_on_posix_are_not_refused() {
    // **The Windows rules are facts about the Windows filesystem, and no others.** A colon is
    // an alternate data stream there and an ordinary character here; `NUL` is a device there and
    // a file here; a trailing dot is stripped there and preserved here. Refusing them on Linux
    // would not protect anything -- it would refuse names this filesystem handles correctly --
    // and a rule kept past the point where its reason applies is the failure this project has
    // already written down twice.
    for requested in ["build.exe:stream", "NUL", "aux.txt", "build.exe.", "a b."] {
        posix().resolve(requested).unwrap_or_else(|error| {
            panic!("{requested:?} is an ordinary POSIX name, got {error:?}")
        });
        // And the other half of the same statement: the Windows rules *do* refuse each of these.
        // Without this the test would pass for a rule set that refuses nothing at all.
        assert!(
            windows().resolve(requested).is_err(),
            "{requested:?} is refused under the Windows rules"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn a_drive_relative_name_is_a_colon_and_a_filename_on_posix() {
    // `C:build.exe` is drive-relative on Windows -- the same file only if the process happened
    // to start in the same directory on drive C -- and a filename here, because a colon is an
    // ordinary character. What matters is the same thing that matters for every other name: it
    // lands under the base.
    //
    // **`cfg(not(windows))` on a test about the POSIX rules, which needs explaining.**
    // [`Rules`] governs the *checks*; which characters separate components is `std::path`'s
    // reading and is the platform's. On a Windows build `C:build.exe` parses as a drive prefix
    // and a name no matter which rules are named, so the question this test asks -- is a colon
    // ordinary? -- cannot be asked there. The colon is still covered on every platform by
    // `names_that_are_ordinary_on_posix_are_not_refused`, whose members parse the same way
    // everywhere.
    let resolved = posix()
        .resolve("C:build.exe")
        .expect("one filename, with a colon in it");

    assert!(
        resolved.starts_with(std::path::Path::new("/linklet")),
        "it has to be inside the base: {}",
        resolved.display()
    );
}

#[cfg(not(windows))]
#[test]
fn a_backslash_is_refused_because_it_means_two_different_things() {
    // Backslashes are ordinary filename characters on POSIX, so a Windows-shaped
    // path would otherwise name an unintended file.
    let error = posix()
        .resolve(r"..\..\etc\hosts")
        .expect_err("a Windows-shaped path, under the POSIX rules");

    assert!(
        matches!(error, PathError::Backslash { .. }),
        "expected a refusal about the separator, got {error:?}"
    );
    let text = error.to_string();
    assert!(text.contains("backslash"), "{text}");
    assert!(
        text.contains("Windows"),
        "the refusal should say why it is not a name here: {text}"
    );
}

// --- paths outside the base --------------------------------------------------

#[test]
fn parent_components_are_resolved_by_the_filesystem_from_the_base() {
    for requested in [nested(&["..", "hosts"]), nested(&["a", "..", "b"])] {
        assert_eq!(
            destination().resolve(&requested).expect("a relative path"),
            std::path::Path::new(BASE).join(&requested)
        );
    }
}

#[test]
fn an_absolute_path_outside_the_base_is_used_directly() {
    let requested = outside_base();
    assert_eq!(
        destination().resolve(&requested).expect("an absolute path"),
        std::path::PathBuf::from(requested)
    );
}

// --- the Windows rules, exercised on every platform ---------------------------
//
// **These run everywhere now.** They used to be either unconditionally applied (so on Linux they
// tested the wiring rather than the rule) or gated behind `#[cfg(windows)]` (so on Linux they ran
// nowhere). Naming the rule set makes each of them a statement about the Windows *rules*, tested
// on whichever machine the suite happens to be running on -- which is what would have caught the
// case-insensitive comparison being wrong on the other one.
//
// **Where the boundary is.** `Rules` decides which *checks* run. How a string splits into
// components, and what `join` does with a relative path, is `std::path`'s business and is
// therefore the platform's: `Path::new(r"a\b").components()` yields two parts on a Windows build
// and one `Normal("a\\b")` on a Linux one, whichever rules are named. So the rules that read
// whole strings are asked on both platforms here, and the three that inspect *components* carry a
// `cfg` -- see the comment above each.
//
// That boundary is worth being precise about, because it is the seam the original bug lived in:
// the module's decisions were a mix of its own checks and `std`'s platform-dependent reading, and
// the half that was `std`'s was the half nobody thought to question. **In production it never
// bites** -- an agent applies `Rules::here()`, so its parsing and its rules belong to the same
// platform -- but it is why one of these tests is gated and the next is not.

/// Asserts that the Windows rules refuse a request, and returns the reason.
fn refused_by_windows(requested: &str) -> PathError {
    windows()
        .resolve(requested)
        .expect_err(&format!("{requested:?} should have been refused"))
}

#[test]
fn an_alternate_data_stream_is_refused() {
    // `build.exe:evil` is not a file with a colon in its name. It is a stream on
    // build.exe: it holds bytes that no directory listing shows and no ordinary
    // tool will read, which makes it a way to write something the operator cannot
    // see and cannot easily remove.
    let error = refused_by_windows(r"build.exe:evil");
    assert!(
        matches!(error, PathError::Colon { .. }),
        "expected a colon refusal, got {error:?}"
    );
}

#[test]
fn a_stream_on_an_absolute_path_is_refused_too() {
    refused_by_windows(r"C:\linklet\build.exe:evil");
}

#[cfg(windows)]
#[test]
fn a_drive_relative_path_is_refused() {
    // `C:build.exe` is not `C:\build.exe`. It is build.exe relative to whatever the
    // current directory happens to be on drive C -- a different file depending on
    // how the process was started, which is exactly the kind of thing a base is
    // supposed to remove.
    //
    // **Gated, and it is the parsing and not the rule.** The check asks whether the first
    // component is a drive prefix, and on a Linux build `C:build.exe` is one `Normal` component
    // with a colon in it -- a character that is ordinary there. A Windows build is the only place
    // this string *is* drive-relative.
    let error = refused_by_windows("C:build.exe");
    assert!(
        matches!(error, PathError::Colon { .. }),
        "expected a colon refusal, got {error:?}"
    );
}

#[test]
fn device_namespaces_are_refused() {
    for requested in [
        r"\\?\C:\linklet\build.exe",
        r"\\.\PhysicalDrive0",
        "//?/C:/build.exe",
    ] {
        assert!(matches!(
            refused_by_windows(requested),
            PathError::DevicePath { .. }
        ));
    }
}

#[cfg(windows)]
#[test]
fn ordinary_network_shares_are_absolute_paths() {
    let requested = r"\\server\share\build.exe";
    assert_eq!(
        windows().resolve(requested).expect("a network file"),
        std::path::PathBuf::from(requested)
    );
    assert!(Destination::new(r"\\server\share").is_ok());
    assert!(matches!(
        windows().resolve(r"\\server\share\file:stream"),
        Err(PathError::Colon { .. })
    ));
}

#[cfg(windows)]
#[test]
fn a_component_ending_in_a_dot_or_a_space_is_refused() {
    // Windows strips them, so `build.exe.` and `build.exe ` and `build.exe` are one
    // file. A check that compared the names literally would pass a name that
    // becomes a different one on disk -- which is how a name-based rule is evaded.
    //
    // **Gated for the parsing**: the check is per component, and `a.\b` is two components only
    // where a backslash separates. The single-component forms (`build.exe.`, `build.exe `) are
    // decided by the same rule anywhere, and `names_that_are_ordinary_on_posix_are_not_refused`
    // asserts the other side of it on both platforms.
    for requested in [r"build.exe.", r"build.exe ", r"a.\b", r"a \b"] {
        let error = refused_by_windows(requested);
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
        let error = refused_by_windows(name);
        assert!(
            matches!(error, PathError::Reserved { .. }),
            "{name:?} gave {error:?}"
        );
    }
}

#[cfg(windows)]
#[test]
fn a_reserved_name_in_a_subdirectory_is_refused_too() {
    // The check is per component, not on the whole path. **Gated for the parsing**, like the two
    // above: on a Linux build `logs\NUL` is one component whose stem is the whole string, so
    // there is no subdirectory to be in. `NUL` on its own is refused on every platform, in
    // `every_reserved_device_name_is_refused_with_and_without_an_extension`.
    refused_by_windows(r"logs\NUL");
}

#[test]
fn an_ordinary_name_that_merely_starts_like_a_device_is_allowed() {
    // `console.log` is a file. The rule is the *stem*, and a rule that matched on
    // prefixes would refuse names people actually use. Under both rule sets: neither
    // filesystem reserves these.
    for rules in [windows(), posix()] {
        for name in ["console.log", "com10.txt", "nullify"] {
            rules
                .resolve(name)
                .unwrap_or_else(|error| panic!("{name:?} is an ordinary name, got {error:?}"));
        }
    }
}

// --- the Windows base, which has the same rules -------------------------------

#[test]
fn a_windows_base_cannot_be_a_device_namespace_or_a_stream() {
    for base in [r"\\?\C:\linklet", r"C:\linklet:stream"] {
        assert!(matches!(
            Destination::with_rules(base, Rules::Windows),
            Err(PathError::BadBase { .. })
        ));
    }
}

// --- the property that has to hold whatever the rules say --------------------

/// A Windows separator is never silently accepted as part of a POSIX filename.
#[test]
fn windows_path_separators_are_refused_under_posix_rules() {
    for requested in [
        r"..\..\Windows\System32\drivers\etc\hosts",
        r"..\hosts",
        r"a\..\..\b",
        r"C:\Windows\System32\drivers\etc\hosts",
        r"\\server\share\build.exe",
        r"logs\NUL",
        // **Not `build.exe.`**, which was in this list and had to come out: it is an ordinary
        // POSIX name -- the trailing dot is a Windows rule and Windows is not here -- so it is
        // *correctly* accepted, and `names_that_are_ordinary_on_posix_are_not_refused` asserts
        // that. What every member of this list has in common is a backslash, which is the one
        // thing that means two different paths on the two platforms.
    ] {
        // The selected rules name the unsupported separator.
        match posix().resolve(requested) {
            Err(PathError::Backslash { .. }) => {}
            Err(other) => panic!("{requested:?} was refused as {other:?}, which is unexpected"),
            Ok(resolved) => panic!(
                "{requested:?} was accepted as {}, which is a file nobody meant to name",
                resolved.display()
            ),
        }
    }
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

// --- the base itself ---------------------------------------------------------

#[test]
fn a_base_that_is_relative_is_refused_at_construction() {
    // The saved base must resolve relative paths independently of a later cwd change.
    for base in ["linklet", r".\linklet", "builds"] {
        let error = Destination::new(base).expect_err("a relative base");
        assert!(
            matches!(error, PathError::BadBase { .. }),
            "{base:?} gave {error:?}"
        );
    }
}

#[test]
fn an_unusable_base_is_refused_at_construction() {
    // The platform-independent shapes, on any rules.
    for base in ["", "   "] {
        assert!(
            Destination::new(base).is_err(),
            "{base:?} should not be a usable base"
        );
    }

    // And the two Windows-only ones, under the rules that own them.
    for base in [r"\\?\C:\linklet", r"C:\linklet\build.exe:stream"] {
        assert!(
            Destination::with_rules(base, Rules::Windows).is_err(),
            "{base:?} should not be a usable Windows base"
        );
    }
}

#[test]
fn the_rules_of_this_machine_are_the_rules_of_its_filesystem() {
    // The one place a platform is asked anything, and the only assertion in this file that is
    // about the machine rather than about a rule. `Rules::here` decides which set
    // `Destination::new` applies, and getting it backwards is the bug this whole arrangement
    // exists to prevent -- so it is pinned rather than assumed.
    let expected = if cfg!(windows) {
        Rules::Windows
    } else {
        Rules::Posix
    };

    assert_eq!(Rules::here(), expected);
    assert_eq!(
        destination().rules(),
        expected,
        "a destination built without naming its rules takes the machine's own"
    );
}

#[test]
fn every_refusal_quotes_what_caused_it() {
    let cases = [
        (windows().resolve("nul").expect_err("a device"), "device"),
        (
            windows().resolve(r"\\?\C:\file").expect_err("a namespace"),
            "namespace",
        ),
        (
            windows().resolve("build.exe:evil").expect_err("a stream"),
            "colon",
        ),
        (destination().resolve("").expect_err("nothing"), "empty"),
    ];
    for (error, expected) in cases {
        assert!(error.to_string().contains(expected), "{error}");
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

// --- T11: the message count, which is now bounded by the size ---------------

#[test]
fn the_chunk_count_is_exact_at_every_boundary() {
    // T11 replaced a protocol constant with arithmetic, and this is where an
    // off-by-one lives: a count that is one short leaves a transfer that never
    // finishes, and one that is one long sends a chunk past the declared size,
    // which is T4.
    use linklet_core::transfer::{CHUNK_BYTES, chunks_for};

    assert_eq!(chunks_for(1, CHUNK_BYTES), 1, "one byte is one chunk");
    assert_eq!(
        chunks_for(CHUNK_BYTES - 1, CHUNK_BYTES),
        1,
        "one short of a chunk"
    );
    assert_eq!(chunks_for(CHUNK_BYTES, CHUNK_BYTES), 1, "exactly one chunk");
    assert_eq!(
        chunks_for(CHUNK_BYTES + 1, CHUNK_BYTES),
        2,
        "one past a chunk"
    );
    assert_eq!(chunks_for(CHUNK_BYTES * 3, CHUNK_BYTES), 3, "exactly three");
    assert_eq!(
        chunks_for(CHUNK_BYTES * 3 + 1, CHUNK_BYTES),
        4,
        "three and a bit"
    );

    // Defined rather than left to fall out of a division by zero.
    assert_eq!(chunks_for(0, CHUNK_BYTES), 0);
    assert_eq!(chunks_for(100, 0), 0);
}

#[test]
fn the_chunk_size_leaves_room_under_the_frame_ceiling() {
    // A chunk that could meet the frame limit would turn a policy decision into a
    // protocol error, and the error would arrive at the far end of a transfer.
    use linklet_core::frame::MAX_PAYLOAD;
    use linklet_core::transfer::CHUNK_BYTES;

    // Sealed, the payload grows by a tag and a nonce's worth of framing, so the
    // margin has to cover that and not just the plaintext.
    assert!(
        (CHUNK_BYTES as usize) + 64 < MAX_PAYLOAD,
        "a chunk plus its overhead must fit in one frame"
    );
}

// --- T3: the manifest, checked before anything is read ----------------------

/// A digest of the right shape, for manifests that are not about the digest.
///
/// It contains letters, and that is not decoration. The first version was sixty-four
/// zeros, which made the "uppercase is refused" case below pass a digest that was
/// legally lowercase -- **digits have no case** -- so the test asserted a refusal of
/// something it had actually accepted.
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn a_manifest_that_checks_out_resolves_to_a_path_under_the_base() {
    use linklet_core::transfer::Manifest;

    let manifest = Manifest {
        path: nested(&["artifacts", "build.exe"]),
        bytes: 1024,
        sha256: DIGEST.to_string(),
    };

    let resolved = manifest.check(&destination()).expect("a valid manifest");
    assert_eq!(
        resolved,
        std::path::Path::new(BASE)
            .join("artifacts")
            .join("build.exe")
    );
}

#[test]
fn a_transfer_over_the_ceiling_is_refused_by_number() {
    // T3. The declared size is checked before a single chunk is read, so a receiver
    // never agrees to receive something it would refuse.
    use linklet_core::transfer::{MAX_TRANSFER_BYTES, Manifest, ManifestError};

    let manifest = Manifest {
        path: "build.exe".to_string(),
        bytes: MAX_TRANSFER_BYTES + 1,
        sha256: DIGEST.to_string(),
    };

    let error = manifest.check(&destination()).expect_err("too large");
    assert_eq!(
        error,
        ManifestError::TooLarge {
            bytes: MAX_TRANSFER_BYTES + 1
        }
    );
    assert!(
        error
            .to_string()
            .contains(&(MAX_TRANSFER_BYTES + 1).to_string())
    );

    // And the ceiling itself is usable: an off-by-one at the boundary is the
    // version of this that ships.
    let at_limit = Manifest {
        path: "build.exe".to_string(),
        bytes: MAX_TRANSFER_BYTES,
        sha256: DIGEST.to_string(),
    };
    assert!(at_limit.check(&destination()).is_ok());
}

#[test]
fn a_transfer_of_nothing_is_refused() {
    use linklet_core::transfer::{Manifest, ManifestError};

    let manifest = Manifest {
        path: "build.exe".to_string(),
        bytes: 0,
        sha256: DIGEST.to_string(),
    };
    assert_eq!(manifest.check(&destination()), Err(ManifestError::Empty));
}

#[test]
fn a_digest_that_is_not_a_digest_is_refused() {
    use linklet_core::transfer::{Manifest, ManifestError};

    // The uppercase case is the one worth stating: two digests differing only in
    // case compare unequal as strings and equal as digests, so only one case is
    // accepted and the comparison never has to think about it.
    let cases = [
        "",
        "abc",
        &DIGEST.to_uppercase(),
        &format!("{DIGEST}0"),
        "zz00000000000000000000000000000000000000000000000000000000000000",
    ];

    for got in cases {
        let manifest = Manifest {
            path: "build.exe".to_string(),
            bytes: 1,
            sha256: got.to_string(),
        };
        assert!(
            matches!(
                manifest.check(&destination()),
                Err(ManifestError::BadDigest { .. })
            ),
            "{got:?} should not be a digest"
        );
    }
}

#[test]
fn a_manifest_checks_the_path_and_not_only_the_size() {
    // One call checks everything, on purpose: a caller that could check the size
    // without the path, or the reverse, is a caller that does one of them.
    use linklet_core::transfer::{Manifest, ManifestError};

    let manifest = Manifest {
        path: "bad\0name".to_string(),
        bytes: 1024,
        sha256: DIGEST.to_string(),
    };

    let error = manifest.check(&destination()).expect_err("a NUL byte");
    assert!(
        matches!(error, ManifestError::Path(PathError::NulByte)),
        "got {error:?}"
    );
}

#[test]
fn a_manifest_reports_which_chunks_it_takes() {
    use linklet_core::transfer::{CHUNK_BYTES, Manifest};

    let manifest = Manifest {
        path: "build.exe".to_string(),
        bytes: CHUNK_BYTES * 2 + 1,
        sha256: DIGEST.to_string(),
    };
    assert_eq!(manifest.chunks(), 3);
}

// --- the running total (T4, T5) ----------------------------------------------

#[test]
fn a_chunk_that_fits_is_accepted_and_the_total_grows() {
    let mut receiving = Receiving::new(10);
    receiving.accept(4).expect("four of ten");
    receiving.accept(3).expect("seven of ten");
    assert_eq!(receiving.written(), 7);
    assert_eq!(receiving.declared(), 10);
    assert!(!receiving.is_complete());
}

#[test]
fn the_boundary_is_exactly_the_declared_size() {
    // One byte less is short, one byte more is too much, exactly equal is finished.
    // This is the whole of T4 and T6 in one test, because an off-by-one at the
    // boundary is the version of this that ships.
    let mut receiving = Receiving::new(3);
    receiving.accept(3).expect("exactly the declared size");
    assert!(receiving.is_complete());

    let mut short = Receiving::new(3);
    short.accept(2).expect("two of three");
    assert!(!short.is_complete());
}

#[test]
fn a_chunk_is_checked_after_the_one_before_it_and_not_only_at_the_start() {
    // T4, and the reason it is stated as "after every chunk": the declared number
    // *was* checked, at the start. A sender that declares 100 and keeps sending is the
    // one this catches, so the refusal has to happen with the total already spent.
    let mut receiving = Receiving::new(10);
    receiving.accept(6).expect("six of ten");
    let error = receiving
        .accept(6)
        .expect_err("twelve is past the ten that was declared");

    assert_eq!(
        error,
        TransferError::TooMuch {
            declared: 10,
            written: 6,
            chunk: 6
        }
    );
    // And the total did not move, so a caller that caught this and carried on would
    // still be refusing the next chunk.
    assert_eq!(receiving.written(), 6);
}

#[test]
fn a_chunk_after_completion_is_told_apart_from_one_that_is_simply_too_big() {
    // T5. The transfer is over, and a frame after completion is a protocol error
    // rather than a chunk that happens to overflow: a reader that treated it as the
    // latter would describe a completed transfer as a failed one.
    let mut receiving = Receiving::new(4);
    receiving.accept(4).expect("all of it");
    assert!(receiving.is_complete());

    let error = receiving
        .accept(1)
        .expect_err("the declared size has been reached");
    assert_eq!(error, TransferError::PastTheEnd { declared: 4 });
}

#[test]
fn the_refusals_say_which_numbers_were_involved() {
    // A transfer's error is read by whoever is pushing a build, and "too large" does
    // not say whether the sender is broken or the file changed underneath it.
    let text = TransferError::TooMuch {
        declared: 10,
        written: 6,
        chunk: 6,
    }
    .to_string();
    for number in ["10", "6"] {
        assert!(text.contains(number), "{text:?} should mention {number}");
    }

    let text = TransferError::Short {
        declared: 100,
        written: 40,
    }
    .to_string();
    assert!(text.contains("100") && text.contains("40"), "{text}");

    let text = TransferError::Digest {
        expected: "aa".repeat(32),
        got: "bb".repeat(32),
    }
    .to_string();
    assert!(text.contains(&"aa".repeat(32)), "{text}");
    assert!(text.contains(&"bb".repeat(32)), "{text}");
}

#[test]
fn a_zero_length_chunk_changes_nothing() {
    // A frame cannot be empty -- the framing refuses it -- so this cannot arrive over
    // a connection. It is defined rather than left to fall out of the comparison,
    // because "it cannot happen" is how an off-by-one ships.
    let mut receiving = Receiving::new(4);
    receiving.accept(0).expect("nothing at all");
    assert_eq!(receiving.written(), 0);
    assert!(!receiving.is_complete());
}

// --- the same arithmetic from the sending side (T4, T11) ----------------------

#[test]
fn a_sender_runs_out_of_chunks_exactly_at_the_declared_size() {
    // T11 on the sending side, and the reason it is here as well as on the receiving
    // side: the message count of a transfer is bounded by the declared size, and one
    // end of that bound is "the sender never offers a chunk it has not declared".
    let mut sending = Sending::new(CHUNK);
    assert_eq!(sending.next_chunk(), Some(CHUNK as usize));
    sending
        .account(CHUNK as usize)
        .expect("the whole first chunk");

    assert_eq!(sending.next_chunk(), None, "the declared size is spent");
    assert!(sending.is_complete());
}

#[test]
fn a_sender_never_offers_more_than_the_declared_remainder() {
    // The last chunk is the one that goes wrong: a fixed-size read past the declared
    // size would offer bytes the receiver has already refused to accept.
    let mut sending = Sending::new(CHUNK + 10);
    assert_eq!(sending.next_chunk(), Some(CHUNK as usize));
    sending.account(CHUNK as usize).expect("a full chunk");

    assert_eq!(sending.next_chunk(), Some(10), "only the remainder is owed");
    sending.account(10).expect("the remainder");
    assert!(sending.is_complete());
}

#[test]
fn a_sender_that_is_handed_more_than_it_offered_refuses() {
    // Belt as well as braces, because the caller reads a file and the file may have
    // grown: a sender that trusted its own read loop would send a chunk the receiver
    // has already been told to refuse.
    let mut sending = Sending::new(5);
    let error = sending
        .account(6)
        .expect_err("six bytes were never declared");
    assert_eq!(
        error,
        TransferError::TooMuch {
            declared: 5,
            written: 0,
            chunk: 6
        }
    );
}

// --- the digest (T7) ---------------------------------------------------------

#[test]
fn a_digest_that_does_not_match_is_refused_and_names_both() {
    // T7. The AEAD already authenticates the bytes, so this catches the layers above
    // it: a framing bug, a write that silently short-wrote, a `.part` that something
    // else overwrote.
    verify_digest(DIGEST, DIGEST).expect("a digest matches itself");

    let error = verify_digest(DIGEST, &"0".repeat(64)).expect_err("these differ");
    assert_eq!(
        error,
        TransferError::Digest {
            expected: DIGEST.to_string(),
            got: "0".repeat(64),
        }
    );
}

#[test]
fn the_digest_comparison_is_case_sensitive() {
    // Both cases cannot be right: two digests differing only in case compare unequal as
    // strings and equal as digests, so accepting both would mean the comparison has to
    // be case-insensitive and every reader has to know it. The manifest requires
    // lowercase, so this is the comparison that matches it.
    assert!(verify_digest(DIGEST, &DIGEST.to_uppercase()).is_err());
}

// --- what a sender can tell about its own manifest (T3, T11) ------------------

#[test]
fn the_sender_refuses_a_transfer_the_receiver_would_refuse() {
    // A sender that would produce an unacceptable transfer learns so locally, instead
    // of after a gigabyte has crossed the network. This is the part of a manifest that
    // does not depend on the receiving side; where it may land is the receiver's
    // question, answered by `check`.
    let cases = [
        (
            Manifest {
                path: "build.exe".to_string(),
                bytes: 0,
                sha256: DIGEST.to_string(),
            },
            "no bytes",
        ),
        (
            Manifest {
                path: "build.exe".to_string(),
                bytes: MAX_TRANSFER_BYTES + 1,
                sha256: DIGEST.to_string(),
            },
            "larger than",
        ),
        (
            Manifest {
                path: "build.exe".to_string(),
                bytes: 1,
                sha256: "nonsense".to_string(),
            },
            "64 lowercase hex",
        ),
    ];

    for (manifest, expected) in cases {
        let error = manifest
            .check_locally()
            .expect_err("this manifest should not be sendable");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?} in {error}"
        );
    }
}

#[test]
fn the_senders_own_check_does_not_look_at_the_path() {
    // Deliberately: the path in a manifest the *sender* holds names a place on someone
    // else's machine, and this side cannot resolve it. Splitting the check is what
    // stops the two ends from disagreeing about who validates what.
    let manifest = Manifest {
        path: "bad\0name".to_string(),
        bytes: 1,
        sha256: DIGEST.to_string(),
    };

    manifest
        .check_locally()
        .expect("the path is not this check's business");
    assert!(
        matches!(
            manifest.check(&destination()),
            Err(ManifestError::Path(PathError::NulByte))
        ),
        "and the receiver still refuses it"
    );
}

/// One chunk, for the sender tests.
const CHUNK: u64 = linklet_core::transfer::CHUNK_BYTES;

#[cfg(windows)]
#[test]
fn an_absolute_path_on_another_drive_is_used_directly() {
    let requested = r"D:\logs\build.log";
    assert_eq!(
        destination().resolve(requested).expect("another drive"),
        std::path::PathBuf::from(requested)
    );
}
