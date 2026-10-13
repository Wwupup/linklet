//! Reading a file on this machine: the bytes, the decoding, and where to stop.
//!
//! `linklet_core::search` decides which lines match and what the bytes were; this opens the
//! file, sniffs it, decodes it, and applies the ceiling. The split is rule 1 of `AGENTS.md`,
//! and it is worth more here than usual: a search over a real file has a dozen ways to be
//! wrong that only a machine can show you, and every one of them is a *report* rather than a
//! rule.
//!
//! # Why the ceiling, and where it is enforced
//!
//! `docs/ROADMAP.md` M10 names the two-gigabyte log as the reason this feature exists at
//! all: the answer is in there and moving the whole file to find it is not an answer. So the
//! file is read **up to a ceiling** and the report says the file was cut short -- a search
//! that silently looked at the first megabyte of a large file would be the worst version of
//! this feature, because it would answer "no matches" with confidence.
//!
//! # The fallback that says which rule it used
//!
//! Unicode is decoded first; Windows fallback uses the system ANSI code page,
//! labelled with its actual number. Only captured bytes are sent to the decoder.

use std::path::Path;

use linklet_core::result::{ErrorCode, OperationError};
use linklet_core::search::{
    Direction, Encoding, Limit, Noun, Pattern, Search, lines_of, scan, sniff,
};

/// The most of a file this will read.
///
/// Sixteen mebibytes, which is the frame ceiling for everything else in this protocol. A
/// file past it is searched from its **end** when that is what was asked for, because "the
/// last ERROR" is a question about the tail -- and a file searched from the front is
/// reported as cut short rather than quietly answering about its first sixteen megabytes.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Why a file could not be searched, in the words the caller should read.
///
/// A `Search` with `searched: false` and this in it -- never an empty result, which is
/// `docs/ROADMAP.md` M10's first lesson.
fn unreadable(path: &str, reason: impl Into<OperationError>) -> Search {
    linklet_core::search::could_not_search(path, reason, Encoding::Utf8)
}

/// Searches one file, resolving relative paths against the configured base.
///
/// Absolute paths are used directly; relative paths use the agent startup
/// directory. The operating system enforces read permissions.
///
/// # Errors
///
/// Never returns an error: a file that could not be read comes back as a [`Search`] whose
/// `searched` is false and whose `problem` says why. That is the shape this whole feature is
/// arranged around -- a caller must be able to tell "this log has no errors" from "I could
/// not read this log".
pub fn grep(
    paths: &linklet_core::transfer::Destination,
    path: &str,
    pattern: &Pattern,
    limit: Limit,
    direction: Direction,
) -> Search {
    let target = match paths.resolve(path) {
        Ok(target) => target,
        Err(error) => {
            return unreadable(
                path,
                OperationError::new(ErrorCode::InvalidArgument, "validate", error.to_string()),
            );
        }
    };

    let resolved = target.to_string_lossy();
    let path = resolved.as_ref();
    let with_text = match read_lines(&target, path, direction) {
        Ok(read) => read,
        Err(problem) => return unreadable(path, problem),
    };

    let search = scan(&with_text.lines, pattern, limit, direction);
    finish(search, path, with_text)
}

/// Reads the last `count` lines of an absolute or base-relative file path.
///
/// The same reading as [`grep`] and the same ceiling, with a pattern that matches
/// everything -- because "show me the end of the log" is the other half of "look at the
/// log", and doing it through a pull moves the whole file to answer a question about its
/// last few lines.
pub fn tail(paths: &linklet_core::transfer::Destination, path: &str, count: usize) -> Search {
    let target = match paths.resolve(path) {
        Ok(target) => target,
        Err(error) => {
            return unreadable(
                path,
                OperationError::new(ErrorCode::InvalidArgument, "validate", error.to_string()),
            );
        }
    };

    let resolved = target.to_string_lossy();
    let path = resolved.as_ref();
    let with_text = match read_lines(&target, path, Direction::Last) {
        Ok(read) => read,
        Err(problem) => return unreadable(path, problem),
    };

    // Every line matches, so the search machinery does the reporting: the counts, the
    // truncation, the encoding and the file size are the same questions whatever was asked.
    let mut search = scan(
        &with_text.lines,
        &Pattern::new(""),
        Limit {
            max_matches: count,
            context: 0,
        },
        Direction::Last,
    );
    // **A read is not a search**, and the summary has to say which it was: `more than 2
    // matches` for a request that asked for the last two lines is a small untruth about what
    // the caller asked for, and the kind of thing that makes the rest of the line suspect.
    search.noun = Noun::Lines;

    finish(search, path, with_text)
}

/// A file's lines, and what was learned about its bytes on the way.
struct Read {
    /// The lines, without terminators.
    lines: Vec<String>,
    /// What the bytes were taken to be.
    encoding: Encoding,
    /// The bytes that were read.
    bytes_read: u64,
    /// The file's size, when the machine reported one.
    file_bytes: Option<u64>,
    /// Whether the ceiling cut the reading short.
    cut_short: bool,
}

/// Moves the facts about the reading onto the search that answered.
fn finish(mut search: Search, path: &str, read: Read) -> Search {
    search.path = path.to_string();
    search.encoding = read.encoding;
    search.bytes_read = read.bytes_read;
    search.file_bytes = read.file_bytes;
    // Two ways to be cut short and one field: the ceiling, and the match limit the scan
    // already decided. Either means "there may be more", which is what a reader acts on.
    search.truncated |= read.cut_short;
    search
}

/// Reads a file's lines, sniffing the bytes first.
///
/// # Errors
///
/// A sentence naming what could not be read or what was not a file.
fn read_lines(target: &Path, path: &str, direction: Direction) -> Result<Read, OperationError> {
    let metadata =
        std::fs::metadata(target).map_err(|error| crate::failure::io(error, "read", target))?;
    if !metadata.is_file() {
        // A directory or a device would otherwise be a strange failure from the read rather
        // than a named refusal, which is the same argument as a pull's.
        return Err(OperationError::new(
            ErrorCode::InvalidArgument,
            "read",
            format!("{path} is not a regular file"),
        ));
    }

    let file_bytes = metadata.len();
    let window = read_bytes(target, direction, file_bytes)?;
    let bytes = &window.bytes;
    let cut_short = window.read < file_bytes;
    let encoding = match sniff(&window.prefix) {
        encoding @ (Encoding::Utf8Bom | Encoding::Utf16Le | Encoding::Utf16Be) => encoding,
        _ => {
            let skip = if window.offset > 0 {
                bytes
                    .iter()
                    .take(3)
                    .take_while(|b| **b & 0xc0 == 0x80)
                    .count()
            } else {
                0
            };
            sniff(&bytes[skip..])
        }
    };

    let decoded = crate::decode_text(
        bytes,
        window.offset + bytes.len(),
        encoding,
        crate::TextSource::File,
    );
    if let Some(error) = decoded.decode_error() {
        return Err(OperationError::new(
            ErrorCode::IoError,
            "decode",
            format!("cannot decode {path}: {error}"),
        ));
    }
    let encoding = Encoding::named(decoded.encoding()).ok_or("unknown decoder encoding")?;
    let text = decoded.as_str();
    let text = if window.offset > 0 {
        text.split_once('\n').map_or(text, |(_, rest)| rest)
    } else {
        text
    };

    Ok(Read {
        lines: lines_of(text),
        encoding,
        bytes_read: window.read,
        file_bytes: Some(file_bytes),
        cut_short,
    })
}

/// The bytes to look at, which end of the file they come from, and how many.
///
/// **A file past the ceiling is read from the end when the end is what was asked for.** The
/// alternative -- always the first sixteen megabytes -- answers "the last ERROR" about the
/// beginning of the file, which is not a smaller answer but a wrong one.
struct Window {
    bytes: Vec<u8>,
    prefix: Vec<u8>,
    offset: usize,
    read: u64,
}

fn read_bytes(
    target: &Path,
    direction: Direction,
    file_bytes: u64,
) -> Result<Window, OperationError> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file =
        std::fs::File::open(target).map_err(|e| crate::failure::io(e, "open", target))?;
    let mut prefix = Vec::new();
    (&mut file)
        .take(256)
        .read_to_end(&mut prefix)
        .map_err(|e| crate::failure::io(e, "read", target))?;
    let remaining = MAX_BYTES - prefix.len() as u64;
    let offset = if file_bytes > MAX_BYTES && direction == Direction::Last {
        let start = file_bytes - remaining;
        file.seek(SeekFrom::Start(start))
            .map_err(|e| crate::failure::io(e, "seek", target))?;
        start as usize
    } else {
        0
    };
    let mut bytes = if offset == 0 {
        prefix.clone()
    } else {
        Vec::new()
    };
    file.take(remaining)
        .read_to_end(&mut bytes)
        .map_err(|e| crate::failure::io(e, "read", target))?;
    let read = bytes.len() as u64 + if offset == 0 { 0 } else { prefix.len() as u64 };
    Ok(Window {
        bytes,
        prefix,
        offset,
        read,
    })
}

#[cfg(test)]
mod bounded_tests {
    use super::*;
    #[test]
    fn a_utf16_tail_window_keeps_its_byte_order_and_character_boundary() {
        let path =
            std::env::temp_dir().join(format!("linklet-utf16-window-{}.txt", std::process::id()));
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend([b'x', 0].repeat(MAX_BYTES as usize / 2));
        bytes.extend(
            "\nTAIL\u{4e2d}\u{6587}\n"
                .encode_utf16()
                .flat_map(u16::to_le_bytes),
        );
        std::fs::write(&path, bytes).expect("fixture");
        let last = read_lines(&path, "fixture", Direction::Last).expect("last window");
        std::fs::remove_file(path).expect("cleanup");
        assert_eq!(last.encoding, Encoding::Utf16Le);
        assert!(
            last.lines.iter().any(|line| line == "TAIL\u{4e2d}\u{6587}"),
            "{:?}",
            last.lines
        );
        assert!(last.bytes_read <= MAX_BYTES);
    }

    #[test]
    fn legacy_decoding_never_reads_outside_the_selected_window() {
        let path =
            std::env::temp_dir().join(format!("linklet-decode-window-{}.txt", std::process::id()));
        let mut bytes = vec![b'x'; MAX_BYTES as usize];
        bytes[..6].copy_from_slice(b"\xffHEAD\n");
        bytes.extend_from_slice(b"\nOUTSIDE\xff\n");
        std::fs::write(&path, bytes).expect("fixture");
        let first = read_lines(&path, "fixture", Direction::First).expect("first window");
        let last = read_lines(&path, "fixture", Direction::Last).expect("last window");
        std::fs::remove_file(path).expect("cleanup");
        assert!(!first.lines.iter().any(|line| line.contains("OUTSIDE")));
        assert!(!last.lines.iter().any(|line| line.contains("HEAD")));
        assert!(first.bytes_read <= MAX_BYTES && last.bytes_read <= MAX_BYTES);
    }
}
