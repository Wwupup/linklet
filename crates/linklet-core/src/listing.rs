//! What is in a directory, and the counts that make an empty one readable.
//!
//! This module is the **decision**; reading the directory is the adapter's job, which is
//! rule 1 of `AGENTS.md` and is worth the same here as it is in `process.rs` and
//! `search.rs`. `ls` exists because an agent that cannot ask what is in a directory has to
//! pull a file to find out whether it is there -- and a pull that fails is a refusal about a
//! path, not an answer about a directory.
//!
//! # The property it inherits, and the one it adds
//!
//! `docs/ROADMAP.md` M10 says `ls` is "the same shape as `ps`: a list *and* the counts that
//! make an empty one readable". So [`Listing`] carries how many entries there are, whether
//! the list was cut short, and -- the part `ps` does not need -- **whether the directory was
//! there at all**.
//!
//! That last one is the whole reason this is not a `Vec<Entry>`. `ps` cannot be asked about
//! something that does not exist, and `ls` is asked about a path on every call: an empty
//! vector from a directory with nothing in it and an empty vector from a path that is not
//! there are the same `Vec` and **opposite facts**. A caller that cannot tell them apart
//! concludes that a machine has no logs, which is how a deployment stops looking for them.
//!
//! # What it deliberately does not do
//!
//! It does not recurse, and it does not follow a path out of the root. Recursion is a walk
//! with its own ceiling and its own report -- a directory tree is not a list -- and the root
//! is `docs/transfer.md` T1, the same rule a push and a pull obey: the directory a transfer
//! may touch is the directory a listing may name.

/// One thing in a directory.
///
/// `size` is optional because a directory has no size that means anything to a reader --
/// [`std::fs::Metadata::len`] returns something for a directory on Windows and it is not a
/// number anybody wants. `None` says "not applicable", which is different from `Some(0)`,
/// which says "empty file".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name, without the directory part.
    pub name: String,
    /// Whether it is a directory.
    pub dir: bool,
    /// How many bytes, for a file.
    pub size: Option<u64>,
    /// When it was last written, in seconds since the Unix epoch.
    ///
    /// Seconds and not a formatted date: a date is a rendering decision that depends on a
    /// locale and a time zone, and the machine that produces this number is not the machine
    /// that reads it. `docs/ROADMAP.md` M10's encoding lesson is the same argument one layer
    /// down -- do not decide for the reader what they are looking at.
    pub modified: Option<i64>,
}

impl Entry {
    /// A directory.
    pub fn dir(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dir: true,
            size: None,
            modified: None,
        }
    }

    /// A file of `size` bytes.
    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            dir: false,
            size: Some(size),
            modified: None,
        }
    }

    /// The same file, with a modification time.
    pub fn modified_at(mut self, seconds: i64) -> Self {
        self.modified = Some(seconds);
        self
    }
}

/// The most entries one listing reports.
///
/// A ceiling for the same reason every other one here exists: the reply is one frame, and a
/// directory with ten thousand files is a listing nobody reads. The count that was cut is
/// reported rather than dropped -- see [`Listing::total`].
pub const MAX_ENTRIES: usize = 500;

/// What a listing found, and everything a reader needs to judge it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The entries, directories first and then by name.
    pub entries: Vec<Entry>,
    /// How many entries the directory held, before the ceiling.
    pub total: usize,
    /// Whether the list was cut short at [`MAX_ENTRIES`].
    pub truncated: bool,
    /// The path the listing was about, echoed.
    pub path: String,
    /// Whether the directory was there.
    ///
    /// **The field that separates "nothing in it" from "nothing there"**, and the reason
    /// this is a struct rather than a list. `false` means every other field is empty because
    /// nothing was looked at.
    pub found: bool,
    /// Why it could not be listed, when it could not.
    pub problem: Option<String>,
}

/// A listing that never happened.
///
/// **Not an empty directory.** It carries the reason and reports `found: false`, so a caller
/// that would have concluded "there are no logs here" sees a failure to look instead.
pub fn could_not_list(path: &str, reason: &str) -> Listing {
    Listing {
        entries: Vec::new(),
        total: 0,
        truncated: false,
        path: path.to_string(),
        found: false,
        problem: Some(reason.to_string()),
    }
}

/// A directory with nothing in it.
///
/// Written out rather than left to `Default` so that the difference from
/// [`could_not_list`] is visible at both call sites: these two constructors are the two
/// answers this module exists to keep apart.
pub fn empty(path: &str) -> Listing {
    Listing {
        entries: Vec::new(),
        total: 0,
        truncated: false,
        path: path.to_string(),
        found: true,
        problem: None,
    }
}

/// Sorts entries the way a listing should read, and applies the ceiling.
///
/// **Directories first, then by name, ignoring case.** Directories first because that is
/// what a reader navigating a tree is looking for, and by name rather than by the order the
/// operating system returned because that order is not stable between machines and a reply
/// that changes shape between two identical calls is a reply nobody can diff.
///
/// Case-insensitive with a case-sensitive tiebreak, so that `Log` and `log` are adjacent and
/// the order is still total: two names that differ only in case would otherwise be ordered
/// by whatever the sort happened to do.
pub fn sorted(path: &str, mut entries: Vec<Entry>) -> Listing {
    let total = entries.len();

    entries.sort_by(|left, right| {
        right
            .dir
            .cmp(&left.dir)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });

    let truncated = total > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);

    Listing {
        entries,
        total,
        truncated,
        path: path.to_string(),
        found: true,
        problem: None,
    }
}

/// Renders a listing as the lines a person or an agent reads.
///
/// One line per entry, `name` for a file and `name/` for a directory, then its size. The
/// trailing slash rather than a word in front of the name, because it is what a shell
/// completes and what a reader already knows how to read.
///
/// **The first line is the summary and it is always printed**, for the reason
/// `docs/ROADMAP.md` M10 gives: `0 of 0 entries in logs` cannot be read as a directory that
/// is not there, and `could not list logs: ...` cannot be read as an empty one. A bare list
/// of lines can be read as either.
pub fn render(listing: &Listing) -> String {
    if !listing.found {
        return format!(
            "could not list {}: {}",
            listing.path,
            listing.problem.as_deref().unwrap_or("no reason given")
        );
    }

    let mut out = format!(
        "{} of {} entries in {}",
        listing.entries.len(),
        listing.total,
        listing.path
    );
    if listing.truncated {
        out.push_str(", stopped early");
    }

    for entry in &listing.entries {
        let slash = if entry.dir { "/" } else { "" };
        match entry.size {
            Some(size) => out.push_str(&format!("\n{}{slash} {size} bytes", entry.name)),
            None => out.push_str(&format!("\n{}{slash}", entry.name)),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_with_nothing_in_it_is_not_a_directory_that_is_not_there() {
        // **The pair this module exists to keep apart.** Both are empty, both have an empty
        // entry list, and they mean opposite things: one says the machine has no logs, the
        // other says nobody looked. A caller that confuses them stops looking for a file
        // that is there.
        let nothing_in_it = empty("logs");
        assert!(nothing_in_it.found);
        assert!(nothing_in_it.entries.is_empty());
        assert!(nothing_in_it.problem.is_none());

        let not_there = could_not_list("logs", "no such directory");
        assert!(!not_there.found);
        assert!(not_there.entries.is_empty());
        assert_eq!(not_there.problem.as_deref(), Some("no such directory"));

        // And the two renderings cannot be confused for each other.
        assert_eq!(render(&nothing_in_it), "0 of 0 entries in logs");
        assert_eq!(render(&not_there), "could not list logs: no such directory");
    }

    #[test]
    fn directories_come_first_and_then_names_ignore_case() {
        // What a reader navigating a tree expects, and a stable order: the machine's own
        // order differs between filesystems, and a reply whose shape changes between two
        // identical calls is one nobody can diff.
        let listing = sorted(
            "logs",
            vec![
                Entry::file("beta.log", 10),
                Entry::dir("Archive"),
                Entry::file("Alpha.log", 20),
                Entry::dir("zulu"),
            ],
        );

        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["Archive", "zulu", "Alpha.log", "beta.log"]);
    }

    #[test]
    fn the_order_is_total_even_for_names_that_differ_only_in_case() {
        // Without the tiebreak, `Log` and `log` are ordered by whatever the sort happened to
        // do, which is the kind of thing that passes on one machine and fails on the next.
        let listing = sorted("logs", vec![Entry::file("log", 1), Entry::file("Log", 2)]);

        let names: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, ["Log", "log"], "upper case first, deterministically");
    }

    #[test]
    fn a_long_directory_is_cut_short_and_says_how_many_there_were() {
        let many: Vec<Entry> = (0..MAX_ENTRIES + 5)
            .map(|index| Entry::file(format!("file-{index:04}.log"), 1))
            .collect();

        let listing = sorted("logs", many);

        assert_eq!(listing.entries.len(), MAX_ENTRIES);
        assert_eq!(
            listing.total,
            MAX_ENTRIES + 5,
            "the count before the ceiling"
        );
        assert!(listing.truncated);
        assert!(
            render(&listing).contains("stopped early"),
            "the ceiling has to be visible in the first line"
        );
    }

    #[test]
    fn a_directory_is_rendered_with_a_slash_and_no_size() {
        // `size: None` is "not applicable" and not `Some(0)`, which would say "empty file".
        let listing = sorted(
            "logs",
            vec![Entry::dir("archive"), Entry::file("build.log", 1234)],
        );

        assert_eq!(
            render(&listing),
            "2 of 2 entries in logs\narchive/\nbuild.log 1234 bytes"
        );
    }

    #[test]
    fn a_file_inside_the_root_is_listed_as_one_entry() {
        // `ls` on a file rather than a directory is a legitimate question -- "is it there, and
        // how big is it" -- and answering "that is not a directory" would make the caller
        // guess a different command to ask it. It is one entry, and the summary says one.
        let listing = sorted("logs/build.log", vec![Entry::file("build.log", 42)]);

        assert_eq!(
            render(&listing),
            "1 of 1 entries in logs/build.log\nbuild.log 42 bytes"
        );
    }
}
