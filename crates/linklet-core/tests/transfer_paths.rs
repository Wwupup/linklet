//! The specification for the decisions a transfer makes before it touches anything.
//!
//! `docs/transfer.md` T1 is the most severe item in that document: the destination
//! is the caller's, so a path the caller chooses is a path an attacker chooses if
//! anything upstream is confused, and the outcome is an arbitrary file write as
//! SYSTEM on someone else's machine. **It is more severe than anything in the
//! framing list, because a framing bug is a refusal and this is a write.**
//!
//! Every rule in `src/transfer.rs` has a test here: the paths it refuses (T1), the
//! sizes it refuses before a chunk is read (T3), and the arithmetic it keeps while
//! the chunks arrive (T4, T5, T6, T7, T11). The Windows-specific path rules get more
//! than one test each, because they are the ones a reader is most likely to think are
//! already handled by the `..` check.
//!
//! What is *not* here is anything that needs a file or a socket: whether the
//! destination is a symlink, what is on disk, and the rename. Those are
//! `linklet-adapters`' business and are tested against a real filesystem.

use linklet_core::transfer::{
    Destination, MAX_TRANSFER_BYTES, Manifest, ManifestError, PathError, Receiving, Sending,
    TransferError, verify_digest,
};

/// A root these tests can use, absolute on whichever platform they run on.
///
/// The rules below are mostly Windows path rules and they are applied unconditionally by
/// `Destination::resolve` -- refusing a colon, a leading `\\`, a reserved device name and a
/// trailing dot is over-strict on Linux and harmless there, where those are ordinary
/// characters. **What is not inert is the root itself**: on Linux `C:\linklet` is a *relative*
/// path, so every test in this file failed at the fixture rather than at the rule, which is how
/// a suite can look like it covers a platform it has never run on.
#[cfg(windows)]
const ROOT: &str = r"C:\linklet";
#[cfg(not(windows))]
const ROOT: &str = "/linklet";

/// The platform's own separator, for building a path that is more than one component.
///
/// Backslash is a separator to `resolve` on Windows and an ordinary character on Linux, so a
/// test that spells a nested path with one is testing Windows on both -- see the note on
/// `ROOT`. A test that means "a name with directories in it" has to say so in the syntax of the
/// machine it is running on.
#[cfg(windows)]
const SEP: char = '\\';
#[cfg(not(windows))]
const SEP: char = '/';

/// A nested relative path, in this platform's syntax.
fn nested(parts: &[&str]) -> String {
    parts.join(&SEP.to_string())
}

/// A path that tries to leave the root by climbing out of it, in this platform's syntax.
fn escaping() -> String {
    nested(&["..", "..", "etc", "hosts"])
}

/// An absolute path that is not under the root, in this platform's syntax.
#[cfg(windows)]
fn outside_root() -> String {
    r"C:\Windows\System32\drivers\etc\hosts".to_string()
}
#[cfg(not(windows))]
fn outside_root() -> String {
    "/etc/hosts".to_string()
}

/// A directory whose *name* starts with the root's, which is the string-prefix trap.
fn sibling_of_the_root() -> String {
    let root = std::path::Path::new(ROOT);
    let name = format!(
        "{}evil",
        root.file_name().unwrap_or_default().to_string_lossy()
    );
    let parent = root.parent().unwrap_or(std::path::Path::new("/"));
    parent
        .join(name)
        .join("build.exe")
        .to_string_lossy()
        .into_owned()
}

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
    let requested = nested(&["artifacts", "latest", "build.exe"]);
    let resolved = destination().resolve(&requested).expect("a nested name");
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
    let requested = std::path::Path::new(ROOT).join("build.exe");
    let resolved = destination()
        .resolve(&requested.to_string_lossy())
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
    // The headline case from the document. **In this platform's syntax**, because the rule is
    // "a `..` component" and what counts as a component is the separator: on Linux a backslash
    // is an ordinary character in a filename, so `..\..\x` is one harmless name rather than an
    // escape -- and a test that spelled it that way would be asserting a refusal a correct
    // Linux agent has no reason to give.
    let error = refused(&escaping());
    assert!(
        matches!(error, PathError::Parent { .. }),
        "expected a parent refusal, got {error:?}"
    );
    assert!(error.to_string().contains(".."));
}

#[test]
fn a_parent_component_is_refused_anywhere_in_the_path() {
    // Not just at the front. `a/../.. /b` escapes by the same route.
    for requested in [
        nested(&["..", "hosts"]),
        nested(&["a", "..", "b"]),
        nested(&["a", "b", "..", "..", "c"]),
        nested(&["a", "..", "..", "Windows", "hosts"]),
    ] {
        let error = refused(&requested);
        assert!(
            matches!(error, PathError::Parent { .. }),
            "{requested:?} gave {error:?}"
        );
    }
}

#[test]
fn an_absolute_path_outside_the_root_is_refused() {
    let error = refused(&outside_root());
    assert!(
        matches!(error, PathError::OutsideRoot { .. }),
        "expected an outside-root refusal, got {error:?}"
    );
}

#[test]
fn a_root_that_is_a_prefix_of_another_directory_is_not_enough() {
    // The string-prefix mistake: a sibling directory whose *name* starts with the root's name
    // would be accepted by a comparison on the string form. Components are compared for this
    // reason, and this test is the reason that reason is written down.
    let error = refused(&sibling_of_the_root());
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

#[cfg(windows)]
#[test]
fn a_drive_relative_path_is_refused() {
    // `C:build.exe` is not `C:\build.exe`. It is build.exe relative to whatever the
    // current directory happens to be on drive C -- a different file depending on
    // how the process was started, which is exactly the kind of thing a root is
    // supposed to remove.
    //
    // **Windows only**: on Linux a colon is an ordinary character in a filename, so this is a
    // legal name and refusing it would be a rule with no reason behind it. See M11.
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

#[cfg(windows)]
#[test]
fn a_component_ending_in_a_dot_or_a_space_is_refused() {
    // Windows strips them, so `build.exe.` and `build.exe ` and `build.exe` are one
    // file. A check that compared the names literally would pass a name that
    // becomes a different one on disk -- which is how a name-based rule is evaded.
    //
    // **Windows only, and the rule is inert rather than absent on Linux** where a trailing dot
    // or space is an ordinary character in a filename. `Destination::resolve` refuses them
    // there too, because the checks are not platform-conditional -- so this is over-strict on
    // Linux rather than broken, and `docs/ROADMAP.md` M11 records the POSIX policy that would
    // replace it. What is *not* acceptable is the reverse: a test that asserted the refusal on
    // a platform where the name is legal would be asserting a rule nobody wrote.
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

#[cfg(windows)]
#[test]
fn a_reserved_name_in_a_subdirectory_is_refused_too() {
    // The check is per component, not on the whole path. Windows-only for the same reason as
    // the two above: `NUL` is a device there and a file here.
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

// --- what Linux does with the Windows forms ----------------------------------

/// The Windows path forms are **not refused on Linux, and cannot escape either**, and this is
/// the test that says so.
///
/// The rules above were written for a Windows filesystem and they are applied unconditionally
/// -- but three of them are decided by `std` questions that are platform-dependent: whether a
/// path is absolute, and what counts as a separator. On Linux `..\..\etc\hosts` is therefore a
/// single name containing backslashes rather than a climb out of the root, and `C:\Windows\x`
/// is a relative name rather than an absolute path.
///
/// **So the refusal does not happen, and the property that matters still does.** T1 is about
/// writes leaving the directory the operator configured, and nothing here leaves it: every one
/// of these resolves to a path *inside* the root, because a name that is not absolute and has
/// no parent component can only be joined under it. That is a weaker guarantee than the
/// Windows side gives -- a caller is not told its path was odd -- and it is not something to
/// rely on by accident either, which is why `docs/ROADMAP.md` M11 records the POSIX policy that
/// would make these refusals explicit rather than incidental.
#[cfg(not(windows))]
#[test]
fn a_windows_path_form_cannot_leave_the_root_on_linux() {
    let root = std::path::Path::new(ROOT);

    for requested in [
        r"..\..\Windows\System32\drivers\etc\hosts",
        r"..\hosts",
        r"a\..\..\b",
        r"C:\Windows\System32\drivers\etc\hosts",
        r"\\server\share\build.exe",
        r"logs\NUL",
        r"build.exe.",
    ] {
        match destination().resolve(requested) {
            // Refused: the checks that do not depend on the platform caught it.
            Err(_) => {}
            // Not refused, and then it has to be inside the root -- which is the whole of T1.
            Ok(resolved) => assert!(
                resolved.starts_with(root),
                "{requested:?} resolved to {}, which is outside {}",
                resolved.display(),
                root.display()
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
        (refused(&escaping()), ".."),
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
fn a_manifest_that_checks_out_resolves_to_a_path_under_the_root() {
    use linklet_core::transfer::Manifest;

    let manifest = Manifest {
        path: nested(&["artifacts", "build.exe"]),
        bytes: 1024,
        sha256: DIGEST.to_string(),
    };

    let resolved = manifest.check(&destination()).expect("a valid manifest");
    assert_eq!(
        resolved,
        std::path::Path::new(ROOT)
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
        path: escaping(),
        bytes: 1024,
        sha256: DIGEST.to_string(),
    };

    let error = manifest.check(&destination()).expect_err("an escape");
    assert!(
        matches!(error, ManifestError::Path(PathError::Parent { .. })),
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
        path: escaping(),
        bytes: 1,
        sha256: DIGEST.to_string(),
    };

    manifest
        .check_locally()
        .expect("the path is not this check's business");
    assert!(
        matches!(
            manifest.check(&destination()),
            Err(ManifestError::Path(PathError::Parent { .. }))
        ),
        "and the receiver still refuses it"
    );
}

/// One chunk, for the sender tests.
const CHUNK: u64 = linklet_core::transfer::CHUNK_BYTES;
