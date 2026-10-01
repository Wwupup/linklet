//! What is running on a machine, and which of it a caller asked to see.
//!
//! This module is the **filter and the report**; running `tasklist` and reading its
//! output is `linklet-adapters`' job, because it starts a program. The split is rule 1 of
//! `AGENTS.md`, and it is worth more here than usual: every way this feature can mislead
//! is a way its *reporting* can mislead, and a report is testable in microseconds.
//!
//! # The property everything here exists for
//!
//! **An empty list must never be mistakable for "nothing is there".** `docs/ROADMAP.md`
//! M10 is the whole of it, and it came from a real machine: a process query returned
//! `count: 0` while the reply also carried the filters it had applied and a note that a
//! non-elevated agent cannot read other users' command lines. Without those two fields the
//! empty result was one step away from the wrong conclusion -- and the wrong conclusion is
//! "the deploy is clean, overwrite the binary", which fails while a process holds the file.
//!
//! Three questions therefore have three separate answers in [`Listing`]:
//!
//! | question | field |
//! |---|---|
//! | what was asked for | [`Listing::applied`] -- echoed, so a filter that was dropped is visible |
//! | is the answer complete | [`Listing::truncated`], [`Listing::unreadable`] |
//! | can this machine answer at all | [`Listing::notes`] -- said, not implied |
//!
//! # What it deliberately does not do
//!
//! It does not kill anything, and it does not decide whether a process is healthy. It also
//! does not invent a process it could not read: a line of `tasklist` output that does not
//! parse is counted in [`Listing::unreadable`] rather than skipped, because a listing that
//! quietly drops what it cannot read is a listing that reports a clean machine.

/// One process, as far as the caller may know about it.
///
/// The optional fields are not laziness: `tasklist` gives a name and a pid to anyone, and
/// the path and the command line only for processes the agent is allowed to look at. **A
/// field that is `None` means "this could not be read", never "this is empty"** -- the
/// distinction [`Listing::notes`] and [`Filter::unanswerable`] are built on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    /// The process identifier.
    pub pid: u32,
    /// The image name, for example `linklet-agent.exe`.
    pub name: String,
    /// The executable's full path, when the agent could read it.
    pub path: Option<String>,
    /// The command line, when the agent could read it.
    pub cmdline: Option<String>,
}

impl Process {
    /// A process with only what `tasklist` gives everyone.
    pub fn named(pid: u32, name: impl Into<String>) -> Self {
        Self {
            pid,
            name: name.into(),
            path: None,
            cmdline: None,
        }
    }

    /// The field a filter name refers to, for the reason `exclude` needs one.
    fn field(&self, field: Field) -> Option<&str> {
        match field {
            Field::Name => Some(&self.name),
            Field::Path => self.path.as_deref(),
            Field::Cmdline => self.cmdline.as_deref(),
        }
    }
}

/// Which of a process's fields a filter can be checked against.
///
/// Public because it is the return type of [`Filter::unanswerable`] and appears in
/// [`Listing::notes`]: a caller has to be able to see *which* field the machine could not
/// describe, rather than only that something could not be checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The image name.
    Name,
    /// The executable's path.
    Path,
    /// The command line.
    Cmdline,
}

impl Field {
    /// How a person says it.
    fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Path => "path",
            Self::Cmdline => "cmdline",
        }
    }
}

/// What the caller asked to see.
///
/// Every field is optional and they are combined with **and**: a process has to satisfy
/// each one that is set. Each string is matched as a case-insensitive substring, because
/// that is what a person types -- `agent` finds `linklet-agent.exe` -- and because an
/// exact-match filter would be a filter nobody uses.
///
/// `exclude` is matched against the name and the command line, and never against the path:
/// an exclude that dropped a process for where its executable lives would be a filter whose
/// effect a reader could not predict from the word they typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Matched against the name only.
    pub name: Option<String>,
    /// Matched against the executable's path.
    pub path: Option<String>,
    /// Matched against the command line.
    pub cmdline: Option<String>,
    /// Matched against name, path and command line together.
    pub query: Option<String>,
    /// Dropped when the name or the command line contains this.
    pub exclude: Option<String>,
}

impl Filter {
    /// A filter that accepts everything.
    pub fn any() -> Self {
        Self::default()
    }

    /// Whether anything was asked for at all.
    pub fn is_empty(&self) -> bool {
        *self == Self::any()
    }

    /// The fields this filter needs that the machine did not provide.
    ///
    /// **This is the difference between a filter and a lie.** A caller that asked for
    /// `cmdline` and got an empty list would conclude that nothing matched, when what
    /// happened is that no process could be described well enough to be checked. The
    /// names come back and [`Listing::notes`] carries the sentence.
    ///
    /// A field counts as unanswerable only when there is at least one process to ask
    /// about: on a machine with nothing running, an unreadable field changes nothing.
    pub fn unanswerable(&self, processes: &[Process]) -> Vec<Field> {
        let wanted = [
            (self.path.as_ref(), Field::Path),
            (self.cmdline.as_ref(), Field::Cmdline),
        ];

        wanted
            .into_iter()
            .filter(|(text, _)| text.is_some())
            .map(|(_, field)| field)
            .filter(|field| {
                processes
                    .iter()
                    .all(|process| process.field(*field).is_none())
            })
            .filter(|_| !processes.is_empty())
            .collect()
    }

    /// Whether one process satisfies every part of this filter.
    ///
    /// A process with no command line **fails** a command-line filter rather than passing
    /// it. Passing would be the unsafe direction: a filter meant to find one build would
    /// quietly accept every process it could not describe.
    fn accepts(&self, process: &Process) -> bool {
        let contains =
            |field: &str, needle: &str| field.to_lowercase().contains(&needle.to_lowercase());

        let named = |needle: &str| {
            process
                .field(Field::Name)
                .is_some_and(|f| contains(f, needle))
        };

        if let Some(name) = &self.name
            && !named(name)
        {
            return false;
        }
        if let Some(path) = &self.path
            && !process
                .field(Field::Path)
                .is_some_and(|f| contains(f, path))
        {
            return false;
        }
        if let Some(cmdline) = &self.cmdline
            && !process
                .field(Field::Cmdline)
                .is_some_and(|f| contains(f, cmdline))
        {
            return false;
        }
        if let Some(query) = &self.query {
            let matches = [Field::Name, Field::Path, Field::Cmdline]
                .into_iter()
                .any(|field| process.field(field).is_some_and(|f| contains(f, query)));
            if !matches {
                return false;
            }
        }
        if let Some(exclude) = &self.exclude {
            let dropped = [Field::Name, Field::Cmdline]
                .into_iter()
                .any(|field| process.field(field).is_some_and(|f| contains(f, exclude)));
            if dropped {
                return false;
            }
        }

        true
    }

    /// The filter as the reply echoes it back.
    ///
    /// The field names are the ones [`Field`] uses, so the echo and the note about an
    /// unanswerable field are written in one vocabulary. **Always an object, empty when
    /// nothing was asked for**, rather than omitted: a caller reading a reply has to be
    /// able to tell "no filters" from "the filters were dropped on the way out", and only
    /// one of those is safe to act on.
    pub fn applied(&self) -> crate::json::Json {
        use crate::json::Json;

        let mut entries = std::collections::BTreeMap::new();
        for (key, value) in [
            ("name", &self.name),
            ("path", &self.path),
            ("cmdline", &self.cmdline),
            ("query", &self.query),
            ("exclude", &self.exclude),
        ] {
            if let Some(value) = value {
                entries.insert(key.to_string(), Json::str(value));
            }
        }
        Json::Object(entries)
    }
}

/// What a listing found, and everything a reader needs to judge it.
///
/// The fields are separate on purpose, and the two counts are not the same number. `total`
/// is how many processes were examined; `count` is how many are in `processes`. A filter
/// that matched nothing and a machine with nothing on it produce different pairs, and so
/// does a listing that hit its ceiling.
///
/// `PartialEq` without `Eq`, because the echoed filter is a [`crate::json::Json`] and a
/// JSON number may be a float.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The processes that matched, in the order the machine reported them.
    pub processes: Vec<Process>,
    /// How many processes were examined, before the filter.
    pub total: usize,
    /// Lines of the machine's output that could not be read as a process.
    pub unreadable: usize,
    /// Whether the filter matched more than [`MAX_LISTED`] and was cut short.
    pub truncated: bool,
    /// The filter, echoed.
    pub applied: crate::json::Json,
    /// What the machine could not tell us, one sentence each. Empty when it could tell us
    /// everything.
    pub notes: Vec<String>,
}

/// Why a listing is not the whole answer, when it is not.
///
/// A reason rather than a bool, and **not a sentence to be read for the answer**: the
/// distinction between "the search ran and found nothing" and "the search could not be
/// run" is the one an agent branches on, and branching on prose is how a tool becomes
/// unusable by the thing it was written for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Incomplete {
    /// Every line of the machine's process list was read.
    No,
    /// Some of that list could not be read, so the total is a floor and not a count.
    UnreadableLines,
    /// The machine's process list could not be obtained at all.
    NotEnumerated,
}

/// How many processes one listing reports.
///
/// A ceiling rather than none, because the reply is one frame and a busy machine has
/// hundreds of processes. Sixty-four is more than a deploy loop needs to look at and small
/// enough that the reply is never the thing that fails -- which is the defect M10 opens
/// with, in a different shape.
pub const MAX_LISTED: usize = 64;

impl Listing {
    /// The number of processes reported.
    pub fn count(&self) -> usize {
        self.processes.len()
    }

    /// Whether the filter that was applied was the empty one.
    ///
    /// Asked of the echo rather than of a [`Filter`], because the echo is what a reader
    /// has: a renderer that re-derived it could describe a filter that was not the one
    /// applied.
    pub fn applied_is_empty(&self) -> bool {
        match &self.applied {
            crate::json::Json::Object(entries) => entries.is_empty(),
            _ => true,
        }
    }

    /// Whether the answer is the whole truth about this machine.
    ///
    /// [`Incomplete::NotEnumerated`] when the process list could not be obtained --
    /// **a caller that treats that as an empty list will deploy over a running binary**,
    /// which is the mistake this whole type is arranged around.
    pub fn incomplete(&self) -> Incomplete {
        if self.total == 0 && self.unreadable > 0 {
            return Incomplete::NotEnumerated;
        }
        if self.unreadable > 0 {
            return Incomplete::UnreadableLines;
        }
        if self.notes.iter().any(|note| note.starts_with("cannot")) {
            return Incomplete::NotEnumerated;
        }
        Incomplete::No
    }
}

/// A listing for a machine that could not be asked.
///
/// **Not an empty list.** It carries the reason and reports [`Incomplete::NotEnumerated`],
/// so the caller that would have overwritten a running binary sees a failure to look
/// rather than a clean machine.
pub fn could_not_enumerate(reason: &str, filter: &Filter) -> Listing {
    Listing {
        processes: Vec::new(),
        total: 0,
        unreadable: 0,
        truncated: false,
        applied: filter.applied(),
        notes: vec![format!("cannot enumerate the process list: {reason}")],
    }
}

/// Applies a filter to what the machine reported.
///
/// `total` is counted **before** the filter and `count` after, so a caller can tell "no
/// process matched this filter" from "this machine has no processes" without reading a
/// sentence.
pub fn apply(processes: Vec<Process>, filter: &Filter, unreadable: usize) -> Listing {
    let total = processes.len();
    let unanswerable = filter.unanswerable(&processes);

    let mut matched: Vec<Process> = processes
        .into_iter()
        .filter(|process| filter.accepts(process))
        .collect();

    let truncated = matched.len() > MAX_LISTED;
    matched.truncate(MAX_LISTED);

    Listing {
        processes: matched,
        total,
        unreadable,
        truncated,
        applied: filter.applied(),
        notes: notes_for(&unanswerable, unreadable),
    }
}

/// The sentences a reader needs to judge a listing.
fn notes_for(unanswerable: &[Field], unreadable: usize) -> Vec<String> {
    let mut notes = Vec::new();

    if !unanswerable.is_empty() {
        let names: Vec<&str> = unanswerable.iter().map(|field| field.as_str()).collect();
        notes.push(format!(
            "no process here could be described well enough to check {}: an agent that is \
             not elevated cannot read another user's command line",
            names.join(" or ")
        ));
    }

    if unreadable > 0 {
        notes.push(format!(
            "{unreadable} line(s) of the machine's process list could not be read and are \
             not in the total"
        ));
    }

    notes
}

/// Renders a listing as the lines a person or an agent reads.
///
/// One line per process, `pid name`, in the order the machine reported them -- the format
/// `check` uses, and for the same reason: a caller branches on the first word, and a line
/// carries no prose it has to read past.
///
/// **The last line is the answer to "could you look", and it is never left out**, not even
/// for a listing with nothing in it. That is the whole of `docs/ROADMAP.md` M10's second
/// lesson applied here: an empty list is only readable next to how many were examined and
/// what was asked for.
///
/// ```
/// use linklet_core::process::{Filter, Process, apply, render};
///
/// let listing = apply(vec![Process::named(100, "linklet-agent.exe")], &Filter {
///     name: Some("agent".to_string()),
///     ..Filter::any()
/// }, 0);
/// assert_eq!(render(&listing), "100 linklet-agent.exe\n1 of 1 match, filter name=agent");
/// ```
pub fn render(listing: &Listing) -> String {
    let mut out = String::new();
    for process in &listing.processes {
        out.push_str(&format!("{} {}\n", process.pid, process.name));
    }

    out.push_str(&format!("{} of {} match", listing.count(), listing.total));

    if !listing.applied_is_empty() {
        out.push_str(&format!(", filter {}", describe_filter(&listing.applied)));
    }
    if listing.truncated {
        out.push_str(&format!(", first {MAX_LISTED} shown"));
    }
    match listing.incomplete() {
        Incomplete::No => {}
        Incomplete::UnreadableLines => {
            out.push_str(&format!(", {} lines unreadable", listing.unreadable));
        }
        Incomplete::NotEnumerated => out.push_str(", the process list could not be read"),
    }

    for note in &listing.notes {
        if !note.is_empty() {
            out.push_str(&format!("\nnote: {note}"));
        }
    }

    out
}

/// The filter as `key=value`, in the order a person wrote the fields.
///
/// Read back out of the JSON the reply already carries rather than from a [`Filter`],
/// because the echo is what the caller has and what a reader should be shown: a renderer
/// that re-derived it could disagree with what was applied.
fn describe_filter(applied: &crate::json::Json) -> String {
    let mut parts = Vec::new();
    for key in ["name", "path", "cmdline", "query", "exclude"] {
        if let Some(value) = applied.get_str(key) {
            parts.push(format!("{key}={value}"));
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two processes, one of which the machine could describe fully.
    fn two_processes() -> Vec<Process> {
        vec![
            Process {
                pid: 100,
                name: "linklet-agent.exe".to_string(),
                path: Some(r"C:\linklet\bin\linklet-agent.exe".to_string()),
                cmdline: Some(r"C:\linklet\bin\linklet-agent.exe --port 8790".to_string()),
            },
            Process::named(200, "explorer.exe"),
        ]
    }

    fn names(listing: &Listing) -> Vec<&str> {
        listing
            .processes
            .iter()
            .map(|process| process.name.as_str())
            .collect()
    }

    #[test]
    fn a_name_filter_matches_a_substring_and_not_a_prefix() {
        let listing = apply(
            two_processes(),
            &Filter {
                name: Some("agent".to_string()),
                ..Filter::any()
            },
            0,
        );

        assert_eq!(names(&listing), ["linklet-agent.exe"]);
        assert_eq!(listing.total, 2, "the total is before the filter");
        assert_eq!(listing.count(), 1);
    }

    #[test]
    fn the_filter_is_echoed_even_when_it_matched_nothing() {
        // The field M10 says made an empty result readable. A reply that omitted the
        // filter would leave "nothing matched" and "nothing was asked" the same shape.
        let listing = apply(
            two_processes(),
            &Filter {
                name: Some("no-such-thing.exe".to_string()),
                ..Filter::any()
            },
            0,
        );

        assert_eq!(listing.count(), 0);
        assert_eq!(listing.total, 2, "there were processes; none matched");
        assert_eq!(
            listing.applied.get_str("name"),
            Some("no-such-thing.exe"),
            "the filter has to come back: {:#?}",
            listing.applied
        );
    }

    #[test]
    fn a_field_the_machine_could_not_read_produces_a_note_and_not_a_silent_empty() {
        // **The defect in M10's own words**: a non-elevated agent cannot read another
        // user's command line, and without a note the empty result is indistinguishable
        // from "nothing there". The filter stays echoed, and the note says why it could
        // not be applied.
        let listing = apply(
            two_processes(),
            &Filter {
                cmdline: Some("--port 8790".to_string()),
                ..Filter::any()
            },
            0,
        );

        // One of the two can be checked and matches; the other cannot be checked and is
        // dropped rather than accepted.
        assert_eq!(names(&listing), ["linklet-agent.exe"]);
        assert!(
            listing.notes.is_empty(),
            "one process could be described, so the field is answerable: {:#?}",
            listing.notes
        );

        // And with nothing describable at all, the note appears.
        let blind = apply(
            vec![Process::named(1, "someone-elses.exe")],
            &Filter {
                cmdline: Some("--port 8790".to_string()),
                ..Filter::any()
            },
            0,
        );
        assert_eq!(blind.count(), 0, "an unreadable field cannot match");
        assert_eq!(blind.total, 1, "and the process is still counted");
        assert_eq!(blind.notes.len(), 1, "{:#?}", blind.notes);
        assert!(blind.notes[0].contains("cmdline"), "{:#?}", blind.notes);
    }

    #[test]
    fn a_listing_that_hit_its_ceiling_says_so() {
        let many: Vec<Process> = (0..MAX_LISTED as u32 + 5)
            .map(|pid| Process::named(pid, "worker.exe"))
            .collect();

        let listing = apply(many, &Filter::any(), 0);

        assert_eq!(listing.count(), MAX_LISTED);
        assert!(listing.truncated, "the ceiling has to be visible");
        assert_eq!(listing.total, MAX_LISTED + 5);
    }

    #[test]
    fn a_line_that_could_not_be_read_is_counted_rather_than_skipped() {
        // A listing that quietly dropped what it could not read would report a clean
        // machine. The count is what makes that impossible.
        let listing = apply(vec![Process::named(1, "worker.exe")], &Filter::any(), 3);

        assert_eq!(listing.total, 1);
        assert_eq!(listing.unreadable, 3);
        assert_eq!(listing.incomplete(), Incomplete::UnreadableLines);
        assert_eq!(listing.notes.len(), 1, "{:#?}", listing.notes);
        assert!(listing.notes[0].contains('3'), "{:#?}", listing.notes);
    }

    #[test]
    fn a_machine_that_could_not_be_asked_is_not_an_empty_machine() {
        // The distinction the deploy loop turns on: "nothing matched" and "the list could
        // not be obtained" lead to opposite actions, and only one of them is safe.
        let listing = could_not_enumerate("cannot run tasklist: not found", &Filter::any());

        assert_eq!(listing.count(), 0, "there is nothing to report");
        assert_eq!(
            listing.incomplete(),
            Incomplete::NotEnumerated,
            "and that is not the same fact as a clean machine"
        );
        assert_eq!(
            listing.applied,
            crate::json::Json::Object(Default::default())
        );
        assert!(!listing.notes.is_empty());
    }

    #[test]
    fn a_machine_with_nothing_on_it_is_complete() {
        // The other side of the test above: a real machine with no processes at all is a
        // complete answer, and a caller must not be left unable to tell them apart.
        let listing = apply(Vec::new(), &Filter::any(), 0);

        assert_eq!(listing.count(), 0);
        assert_eq!(listing.total, 0);
        assert_eq!(listing.incomplete(), Incomplete::No);
        assert!(listing.notes.is_empty());
    }

    #[test]
    fn the_exclude_filter_drops_by_name_and_by_command_line() {
        let listing = apply(
            two_processes(),
            &Filter {
                exclude: Some("explorer".to_string()),
                ..Filter::any()
            },
            0,
        );

        assert_eq!(names(&listing), ["linklet-agent.exe"]);
    }

    #[test]
    fn an_empty_filter_accepts_everything_and_echoes_an_empty_object() {
        let listing = apply(two_processes(), &Filter::any(), 0);

        assert_eq!(listing.count(), 2);
        assert_eq!(
            listing.applied,
            crate::json::Json::Object(Default::default())
        );
    }

    #[test]
    fn the_last_line_says_how_many_were_looked_at_even_when_none_matched() {
        // **The line this whole feature exists for.** "0 of 2 match, filter name=ghost"
        // cannot be read as a clean machine, and the count before the filter is what does
        // the work -- an empty list on its own is the shape M10 records as dangerous.
        let listing = apply(
            two_processes(),
            &Filter {
                name: Some("ghost".to_string()),
                ..Filter::any()
            },
            0,
        );

        assert_eq!(
            render(&listing),
            "0 of 2 match, filter name=ghost",
            "a filter that matched nothing still has to say what it examined"
        );
    }

    #[test]
    fn a_process_line_is_a_pid_then_a_name() {
        // The format `check` uses and for the same reason: the first word is the fact a
        // caller branches on, and there is no prose in front of it.
        let listing = apply(two_processes(), &Filter::any(), 0);
        let first = render(&listing).lines().next().expect("a line").to_string();

        assert_eq!(first, "100 linklet-agent.exe");
    }

    #[test]
    fn a_listing_that_could_not_be_read_says_so_instead_of_ending_at_zero() {
        let listing = could_not_enumerate("cannot run tasklist: not found", &Filter::any());
        let text = render(&listing);
        let summary = text.lines().next().unwrap_or_default();

        assert_eq!(
            summary, "0 of 0 match, the process list could not be read",
            "the first line is the summary and it carries the failure: {text}"
        );
        assert!(
            text.contains("note: cannot enumerate the process list: cannot run tasklist"),
            "and the reason is in the notes: {text}"
        );
    }
}
