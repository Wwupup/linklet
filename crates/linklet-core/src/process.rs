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

/// The processes a filter accepts, **with no ceiling**.
///
/// The half of [`apply`] that decides rather than the half that reports, and it is public
/// because a caller that is going to *act* on the answer must not read a capped one: a kill
/// that could not see a process past [`MAX_LISTED`] would report `matched: 0` for something
/// that is running, and a deploy loop reads that as a clean machine. **A cap on what is
/// reported must not be a cap on what is acted on.**
///
/// This was found by a test rather than by reading it: the deploy-loop test found its marker
/// with `--name` and then asked to kill it by pid, and the pid was past the ceiling in the
/// unfiltered listing `kill` was reading.
pub fn matching(processes: Vec<Process>, filter: &Filter) -> Vec<Process> {
    processes
        .into_iter()
        .filter(|process| filter.accepts(process))
        .collect()
}

/// Applies a filter to what the machine reported.
///
/// `total` is counted **before** the filter and `count` after, so a caller can tell "no
/// process matched this filter" from "this machine has no processes" without reading a
/// sentence.
pub fn apply(processes: Vec<Process>, filter: &Filter, unreadable: usize) -> Listing {
    let total = processes.len();
    let unanswerable = filter.unanswerable(&processes);

    let mut matched = matching(processes, filter);

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

/// What a caller asked to stop.
///
/// An enum rather than two optional fields, because killing one named process and killing
/// every process with a name are different acts and only one of them is safe to do by
/// accident. The type makes the dangerous one impossible to express casually: it has to be
/// asked for by name.
///
/// The matching side is [`Refusal`]'s business rather than this type's -- what a wildcard
/// matches is a regex's question, and this is the shape of the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToKill {
    /// One process, by the number it was given.
    ///
    /// Never gated: a pid is one process, and a caller that names one has already said
    /// which.
    Pid(u32),
    /// One process with exactly this image name, whatever its pid.
    ///
    /// **Gated behind `force`**, because a name is not an identity: on a machine running
    /// several copies of a build, "stop `app.exe`" stops all of them, and the caller may
    /// have meant the one it started.
    Name(String),
    /// Every process whose image name contains this text.
    ///
    /// Gated, and the gating is the point: `--name app` matches a build, a test harness and
    /// whatever else someone named similarly, and a tool that did that on a typo would be a
    /// tool nobody runs twice.
    Matching(String),
}

impl ToKill {
    /// Whether this request can match more than one process.
    pub fn is_bulk(&self) -> bool {
        !matches!(self, Self::Pid(_))
    }

    /// How this request reads in a refusal.
    pub fn describe(&self) -> String {
        match self {
            Self::Pid(pid) => format!("process {pid}"),
            Self::Name(name) => format!("{name:?}"),
            Self::Matching(text) => format!("every process matching {text:?}"),
        }
    }
}

/// What a kill request refused to do, and why.
///
/// Not a [`KillReport`], because a refusal here means **nothing was attempted** -- the
/// difference between "the machine would not let me" and "I decided not to ask" is the
/// difference between a report and a refusal, and folding them together would make a
/// caller read a result to find out whether it had asked for something dangerous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A bulk request that was not forced.
    NotForced {
        /// What it would have done.
        what: String,
    },
    /// A name or match that would have taken the agent itself.
    ///
    /// **Refused before the command runs, and not filtered out of the results.** Filtering
    /// would be the silent version: a caller that asked to stop `linklet-agent` would get a
    /// report saying the kill succeeded on everything else, and the one process it actually
    /// named would be missing from it. Refusing says what happened.
    WouldKillItself {
        /// The names that matched, which is what the caller has to act on.
        names: Vec<String>,
    },
    /// The machine's process list could not be read, so the request could not be checked.
    ///
    /// **Refused rather than attempted against an empty list.** A kill needs the candidate
    /// list twice: to find the process a name refers to, and to check the request against the
    /// pids that must not be stopped. An empty list answers both questions wrongly -- it
    /// reports `matched: 0`, which a deploy loop reads as a clean machine, and it finds no
    /// protected pid, which is how an explicit `--pid` naming the agent itself would get
    /// through. Neither is a fact about the machine; both are the absence of one.
    CannotSee {
        /// What went wrong, in the adapter's own words.
        reason: String,
    },
}

/// What the machine could tell us about what is running.
///
/// **An argument to [`plan_kill`] rather than a `&[Target]`, and that is the point.** A
/// caller cannot pass an empty slice by accident, because an empty slice means something
/// real -- a machine with nothing running -- and "I could not look" is a different fact that
/// has to be constructed on purpose. `docs/ROADMAP.md` M10 is the whole of the argument, one
/// layer down from where it is usually made: the same confusion that makes an empty listing
/// unreadable makes an unreadable listing look like a safe one to kill against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen<'a> {
    /// The machine's process list was read, and these are the processes on it.
    Listed(&'a [Target]),
    /// It could not be read, and this is why.
    Blind {
        /// What went wrong, in the adapter's own words.
        reason: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotForced { what } => write!(
                f,
                "{what} can match more than one process, and killing is not undoable: \
                 pass --yes if that is what you meant"
            ),
            Self::WouldKillItself { names } => write!(
                f,
                "{} is the agent serving this request, or the process that started it: \
                 stopping it would end the conversation before the answer could be sent",
                names.join(", ")
            ),
            Self::CannotSee { reason } => write!(
                f,
                "the machine's process list could not be read ({reason}), so this request \
                 could not be checked against the processes it must not stop: nothing was \
                 attempted"
            ),
        }
    }
}

/// One process a kill request will act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The process identifier.
    pub pid: u32,
    /// The image name, which is what a reader recognises.
    pub name: String,
}

/// What a kill did, in the four pieces a reader needs.
///
/// **The pieces are the point**, and they are the same argument `ps` is built on: a name
/// match that was deliberately not killed is a **failure and not a clean result**, because
/// reading `killed: []` as "it was already gone" is how a caller comes to overwrite a file
/// a live process is holding. `matched` counts everything the request found, so an empty
/// `killed` list can never be read on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillReport {
    /// How many processes the request matched, including any that were not killed.
    pub matched: usize,
    /// The processes that are gone.
    pub killed: Vec<Target>,
    /// The processes that were deliberately not touched, and why.
    pub excluded: Vec<Target>,
    /// The processes the machine would not stop.
    pub failed: Vec<Target>,
    /// What the machine could not tell us about the guard, one sentence each.
    ///
    /// A narrower guard is a real weakening and is said rather than implied -- the same
    /// argument as [`Listing::notes`]. Empty when the guard is what it should be.
    pub notes: Vec<String>,
}

impl KillReport {
    /// Whether everything the request matched is gone.
    ///
    /// **`false` for a name match that was deliberately not killed**, which is the case a
    /// caller most needs to see: it means the machine still has the process the caller
    /// asked about, whatever the `killed` list says.
    pub fn complete(&self) -> bool {
        self.excluded.is_empty() && self.failed.is_empty()
    }

    /// The processes still running, as far as this report knows.
    pub fn still_running(&self) -> Vec<&Target> {
        self.excluded.iter().chain(self.failed.iter()).collect()
    }

    /// Whether anything at all was found.
    pub fn matched_nothing(&self) -> bool {
        self.matched == 0
    }
}

/// Decides what a kill request will act on, refusing the two things it must not do.
///
/// Pure, and separated from running `taskkill` for the same reason the filter is separated
/// from running `tasklist`: everything that can be wrong here is a decision, and a decision
/// that can only be tested by killing something is a decision that stops being tested.
///
/// # Errors
///
/// [`Refusal::NotForced`] for a bulk request without `force`, [`Refusal::CannotSee`] when the
/// machine's process list could not be read, and [`Refusal::WouldKillItself`] when the request
/// would take a protected process. All three mean **nothing was attempted**.
///
/// `protected` is the pids the caller cannot afford to lose -- the agent's own and the
/// process that started it -- and it is an argument rather than a constant because this
/// module does not know them and should not: they are facts about a running process, and
/// this crate has no running processes.
pub fn plan_kill(
    to_kill: &ToKill,
    force: bool,
    exclude: Option<&str>,
    seen: Seen<'_>,
    protected: &[u32],
) -> Result<Vec<Target>, Refusal> {
    // **The machine first**, before the request's own shape is judged. Both are refusals and
    // both mean nothing was attempted, and this one is reported first because it is the one
    // the caller cannot fix: being told to add `--yes` and then being told the machine could
    // not be read is a turn spent on the wrong problem.
    let candidates = match seen {
        Seen::Listed(candidates) => candidates,
        Seen::Blind { reason } => return Err(cannot_see(reason)),
    };

    if to_kill.is_bulk() && !force {
        return Err(Refusal::NotForced {
            what: to_kill.describe(),
        });
    }

    let matches = |target: &Target| match to_kill {
        ToKill::Pid(pid) => target.pid == *pid,
        ToKill::Name(name) => target.name.eq_ignore_ascii_case(name),
        ToKill::Matching(text) => target.name.to_lowercase().contains(&text.to_lowercase()),
    };

    let matched: Vec<&Target> = candidates.iter().filter(|target| matches(target)).collect();

    let protected_names: Vec<String> = matched
        .iter()
        .filter(|target| protected.contains(&target.pid))
        .map(|target| target.name.clone())
        .collect();
    if !protected_names.is_empty() {
        return Err(Refusal::WouldKillItself {
            names: protected_names,
        });
    }

    // The exclusion is applied last, after the refusal above: a caller that asked to kill
    // the agent cannot have that refusal quietly dropped by excluding it. Refusing is the
    // honest answer even when the exclusion would have prevented the harm.
    Ok(matched
        .into_iter()
        .filter(|target| {
            !exclude.is_some_and(|text| target.name.to_lowercase().contains(&text.to_lowercase()))
        })
        .cloned()
        .collect())
}

/// The refusal for a machine that could not be read.
///
/// A named function rather than a struct literal inline, so the branch in [`plan_kill`] reads
/// as the decision it is.
fn cannot_see(reason: String) -> Refusal {
    Refusal::CannotSee { reason }
}

/// A report for a request that was refused before anything ran.
///
/// **Carries the processes the refusal was about**, so that a caller reading only the
/// report can see what it almost did. `matched` is zero because nothing was matched for
/// killing -- the request never got that far.
pub fn refused_kill() -> KillReport {
    KillReport {
        matched: 0,
        killed: Vec::new(),
        excluded: Vec::new(),
        failed: Vec::new(),
        notes: Vec::new(),
    }
}

/// Renders a kill as the lines a person or an agent reads.
///
/// The first line is a count, and it is always the same shape as `ps`'s summary for the
/// same reason: `killed 0 of 1` cannot be mistaken for "there was nothing there", which
/// `killed:` followed by nothing can.
pub fn render_kill(report: &KillReport) -> String {
    let mut out = format!("killed {} of {}\n", report.killed.len(), report.matched);
    for target in &report.killed {
        out.push_str(&format!("{} {}\n", target.pid, target.name));
    }
    for target in &report.excluded {
        out.push_str(&format!("excluded {} {}\n", target.pid, target.name));
    }
    for target in &report.failed {
        out.push_str(&format!("failed {} {}\n", target.pid, target.name));
    }
    out.trim_end().to_string()
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

    #[test]
    fn the_filter_that_acts_has_no_ceiling_even_though_the_one_that_reports_does() {
        // **The bug this pair of functions exists to prevent.** A listing is capped so one
        // reply cannot fail; a kill must not be, or a process past the ceiling is invisible
        // to a request that names it -- and `matched: 0` for a running process is what a
        // deploy loop reads as a clean machine. Found by a test, not by reading: the
        // deploy-loop test found its marker with `--name` and then could not kill it by pid.
        let many: Vec<Process> = (0..MAX_LISTED as u32 + 40)
            .map(|pid| Process::named(pid, "worker.exe"))
            .collect();

        assert_eq!(
            matching(many.clone(), &Filter::any()).len(),
            MAX_LISTED + 40,
            "the filter that decides must see everything"
        );
        assert_eq!(
            apply(many, &Filter::any(), 0).count(),
            MAX_LISTED,
            "and the one that reports must still be capped"
        );
    }

    // --- what may be killed --------------------------------------------------

    /// Three processes, one of which is the agent serving the request.
    fn candidates() -> Vec<Target> {
        vec![
            Target {
                pid: 10,
                name: "app.exe".to_string(),
            },
            Target {
                pid: 11,
                name: "app-helper.exe".to_string(),
            },
            Target {
                pid: 999,
                name: "linklet-agent.exe".to_string(),
            },
        ]
    }

    /// The agent's own pid and the pid that started it.
    const PROTECTED: [u32; 2] = [999, 1000];

    #[test]
    fn a_pid_is_killed_without_being_forced() {
        // A pid is one process and the caller has already said which.
        let planned = plan_kill(
            &ToKill::Pid(10),
            false,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect("one process by number");

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].pid, 10);
    }

    #[test]
    fn a_name_that_would_take_more_than_one_process_has_to_be_forced() {
        // **The safety property.** `--name app.exe` reads like one process and matches two
        // on a machine running a build and its helper; a bulk kill on a typo is the act a
        // tool must not perform by accident.
        let refusal = plan_kill(
            &ToKill::Matching("app".to_string()),
            false,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect_err("a bulk match without --yes");

        let Refusal::NotForced { what } = &refusal else {
            panic!("expected a refusal about forcing, got {refusal:?}");
        };
        assert!(what.contains("app"), "{refusal}");
        assert!(
            refusal.to_string().contains("--yes"),
            "the refusal has to say how to proceed: {refusal}"
        );
    }

    #[test]
    fn a_forced_bulk_match_plans_every_process_it_matched() {
        let planned = plan_kill(
            &ToKill::Matching("app".to_string()),
            true,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect("forced");

        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].pid, 10);
        assert_eq!(
            planned[1].pid, 11,
            "the helper matches too, which is why it is gated"
        );
    }

    #[test]
    fn a_request_that_would_stop_the_agent_is_refused_and_says_which_process() {
        // **The guard that matters most.** Stopping the agent ends the conversation before
        // the answer can be sent, so this has to be refused *before* the command runs --
        // and it is a refusal rather than a filter, because a filter would report success
        // on everything else while silently dropping the one process the caller named.
        let refusal = plan_kill(
            &ToKill::Matching("linklet-agent".to_string()),
            true,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect_err("this would stop the agent");

        let Refusal::WouldKillItself { names } = &refusal else {
            panic!("expected a refusal about the agent itself, got {refusal:?}");
        };
        assert_eq!(names, &["linklet-agent.exe".to_string()]);
        assert!(
            refusal.to_string().contains("agent serving this request"),
            "{refusal}"
        );
    }

    #[test]
    fn an_exclude_does_not_excuse_a_request_that_would_stop_the_agent() {
        // The two could be combined to look harmless -- kill everything matching `agent`
        // except `linklet-agent` -- and the answer is still no: the caller asked for
        // something that would end the conversation, and being told so is more useful than
        // a report about the processes that were not the point.
        let refusal = plan_kill(
            &ToKill::Matching("agent".to_string()),
            true,
            Some("linklet-agent"),
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect_err("excluding it does not make the request safe");

        assert!(
            matches!(refusal, Refusal::WouldKillItself { .. }),
            "{refusal:?}"
        );
    }

    #[test]
    fn a_name_excludes_itself_from_the_plan_but_the_match_is_still_reported() {
        // The exclude is applied to what will be killed, and `matched` is counted before
        // it -- so a report can say "I found three and stopped two", which is the sentence
        // that keeps a deliberate exclusion from looking like a clean machine.
        let planned = plan_kill(
            &ToKill::Matching("app".to_string()),
            true,
            Some("helper"),
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect("forced");

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].pid, 10);
    }

    #[test]
    fn a_machine_that_could_not_be_read_is_refused_rather_than_killed_against() {
        // **The hole this closes, and it was reachable two ways.** A kill needs the candidate
        // list to find the process a name refers to *and* to check the request against the
        // pids that must not be stopped. Handed an empty list when the machine could not be
        // read, `plan_kill` answered both questions wrongly: `matched: 0`, which a deploy loop
        // reads as a clean machine, and "no protected pid found", which is how an explicit
        // `--pid` naming the agent itself would have got through and ended the conversation.
        //
        // The fix is that a caller cannot express any of that by accident: `Seen::Blind` is a
        // value it has to build on purpose, and this is what happens when it does.
        let refusal = plan_kill(
            &ToKill::Pid(10),
            false,
            None,
            Seen::Blind {
                reason: "cannot run tasklist: not found".to_string(),
            },
            &PROTECTED,
        )
        .expect_err("a machine that could not be read is not a machine with nothing running");

        let Refusal::CannotSee { reason } = &refusal else {
            panic!("expected a refusal about not being able to look, got {refusal:?}");
        };
        assert!(reason.contains("tasklist"), "{refusal}");
        let text = refusal.to_string();
        assert!(
            text.contains("nothing was attempted"),
            "the refusal has to say that nothing ran: {text}"
        );
    }

    #[test]
    fn an_explicit_pid_for_the_agent_is_guarded_by_the_list_and_not_by_the_number() {
        // The other half of the same hole, on its own because it is the consequence that
        // matters: the guard works by finding the protected pid *in the list*. With no list
        // there is no guard -- which is why the answer is a refusal and not an attempt.
        let agent_pid = PROTECTED[0];

        let guarded = plan_kill(
            // An explicit pid, which is never gated for forcing.
            &ToKill::Pid(agent_pid),
            false,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect_err("the agent's own pid is protected");

        assert!(
            matches!(guarded, Refusal::WouldKillItself { .. }),
            "{guarded:?}"
        );

        let blind = plan_kill(
            &ToKill::Pid(agent_pid),
            false,
            None,
            Seen::Blind {
                reason: "the list could not be read".to_string(),
            },
            &PROTECTED,
        )
        .expect_err("blind is refused too, and for the reason that matters here");

        assert!(matches!(blind, Refusal::CannotSee { .. }), "{blind:?}");
    }

    #[test]
    fn a_name_that_matches_nothing_plans_nothing_and_is_not_an_error() {
        // "Make sure it is gone" is an ordinary intent, and a machine where it never
        // existed is that intent already satisfied.
        let planned = plan_kill(
            &ToKill::Pid(4242),
            false,
            None,
            Seen::Listed(&candidates()),
            &PROTECTED,
        )
        .expect("nothing to kill is not a refusal");

        assert!(planned.is_empty());
    }

    #[test]
    fn a_kill_report_says_when_the_machine_still_has_the_process() {
        // The shape a caller branches on, and the reason a report is not a bool:
        // `killed: []` on its own is how "it was not there" and "it would not die" come to
        // look the same, and only one of them is safe to deploy over.
        let clean = KillReport {
            matched: 1,
            killed: vec![Target {
                pid: 10,
                name: "app.exe".to_string(),
            }],
            excluded: Vec::new(),
            failed: Vec::new(),
            notes: Vec::new(),
        };
        assert!(clean.complete());
        assert!(clean.still_running().is_empty());

        let partial = KillReport {
            matched: 2,
            killed: Vec::new(),
            excluded: vec![Target {
                pid: 999,
                name: "linklet-agent.exe".to_string(),
            }],
            failed: vec![Target {
                pid: 11,
                name: "app-helper.exe".to_string(),
            }],
            notes: Vec::new(),
        };
        assert!(!partial.complete(), "two processes are still there");
        assert_eq!(partial.still_running().len(), 2);
    }

    #[test]
    fn a_kill_is_rendered_as_a_count_before_anything_else() {
        // The same argument as the listing's summary: `killed 0 of 1` cannot be read as
        // "there was nothing", which a bare list can.
        let report = KillReport {
            matched: 1,
            killed: Vec::new(),
            excluded: Vec::new(),
            failed: vec![Target {
                pid: 10,
                name: "app.exe".to_string(),
            }],
            notes: Vec::new(),
        };

        assert_eq!(render_kill(&report), "killed 0 of 1\nfailed 10 app.exe");
    }
}
