//! Listing a directory on this machine, and saying when it could not be listed.
//!
//! `linklet_core::listing` decides what a listing *says*; this reads the filesystem. The
//! split is rule 1 of `AGENTS.md`, and it earns its place for the same reason it does in
//! `search.rs` and `processes.rs`: every way this can mislead is a way its **reporting** can
//! mislead, and a report is a table rather than a machine.
//!
//! # The one distinction
//!
//! An empty directory and a directory that is not there produce the same `Vec` and are
//! opposite facts. [`list`] therefore never returns an empty list for a path it could not
//! read: it returns a [`Listing`] whose `found` is false and whose `problem` says why. A
//! caller that confused them would conclude a machine has no logs, which is how a deployment
//! stops looking for them.

use linklet_core::listing::{Entry, Listing, MAX_ENTRIES, could_not_list, empty, sorted};
use linklet_core::transfer::Destination;

/// Lists one path under `root`.
///
/// **A directory and a file are both legitimate questions.** "What is in this directory" and
/// "is this file there, and how big is it" are the two things a caller about to read or pull
/// something needs, and answering the second with "that is not a directory" would make the
/// caller guess a different command to ask it. A file lists as one entry.
///
/// The path is resolved against the agent's transfer root exactly as a pull's is: T1 of
/// `docs/transfer.md` is written about writes, and the same `..\..\Windows\System32\...` that
/// must not be written must not be enumerated either.
///
/// # Errors
///
/// Never returns an error. A path that is not there, or cannot be read, comes back as a
/// listing whose `found` is false -- see the module comment.
pub fn list(root: &Destination, path: &str) -> Listing {
    let target = match root.resolve(path) {
        Ok(target) => target,
        Err(error) => return could_not_list(path, &error.to_string()),
    };

    let metadata = match std::fs::metadata(&target) {
        Ok(metadata) => metadata,
        Err(error) => return could_not_list(path, &format!("cannot read {path}: {error}")),
    };

    if !metadata.is_dir() {
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        return sorted(path, vec![entry_of(&name, &metadata)]);
    }

    let directory = match std::fs::read_dir(&target) {
        Ok(directory) => directory,
        Err(error) => return could_not_list(path, &format!("cannot list {path}: {error}")),
    };

    let mut entries = Vec::new();
    for entry in directory {
        // **One unreadable entry does not fail the listing.** The rest of the directory is
        // still an answer, and refusing the whole thing because one name could not be read
        // would report a directory nobody can see as one that is empty -- which is the
        // confusion this module exists to prevent, arriving from the other direction.
        let Ok(entry) = entry else {
            continue;
        };

        let name = entry.file_name().to_string_lossy().into_owned();
        match entry.metadata() {
            Ok(metadata) => entries.push(entry_of(&name, &metadata)),
            // A name whose metadata cannot be read is still a name, and dropping it would
            // make a directory look smaller than it is.
            Err(_) => entries.push(Entry {
                name,
                dir: entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
                size: None,
                modified: None,
            }),
        }

        // One past the ceiling is enough to know the list was cut short, and it stops a
        // directory with a hundred thousand names being read to the end to say so.
        if entries.len() > MAX_ENTRIES {
            break;
        }
    }

    if entries.is_empty() {
        return empty(path);
    }
    sorted(path, entries)
}

/// One entry, from what the filesystem said about it.
fn entry_of(name: &str, metadata: &std::fs::Metadata) -> Entry {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64);

    Entry {
        name: name.to_string(),
        dir: metadata.is_dir(),
        // `None` for a directory: `Metadata::len` returns something for one on Windows and it
        // is not a number anybody wants. `None` says "not applicable", which is not the same
        // as `Some(0)`, which says "empty file".
        size: (!metadata.is_dir()).then_some(metadata.len()),
        modified,
    }
}
