//! Reading a file that is somewhere else: which lines match, and what the bytes were.
//!
//! This module is the **decision**; opening the file and reading it is the adapter's job.
//! The split is rule 1 of `AGENTS.md`, and it earns its place here for the same reason it
//! does in `process.rs`: every way this feature can mislead is a way its *reporting* can
//! mislead, and a report is testable in microseconds.
//!
//! # The two lessons it exists to keep
//!
//! `docs/ROADMAP.md` M10 names both, from the sibling project that learned them first:
//!
//! 1. **A failed search must not read as "no matches".** A file that could not be read,
//!    a machine whose encoding could not be established, a file past the size this will
//!    look at -- each is a fact about *looking*, and a caller that acts on "no matches"
//!    when the search never happened will conclude a log is clean. [`Search::searched`]
//!    and [`Search::problem`] are what make the difference visible, and neither is ever
//!    left to be inferred from an empty list.
//! 2. **The encoding is sniffed, and the answer says which one won.** Guessing produces
//!    mojibake; guessing *silently* produces mojibake that is believed. [`Encoding`] is
//!    carried out with the text so a reader can see what was assumed and re-read the
//!    bytes themselves if the assumption was wrong.
//!
//! # What it deliberately does not do
//!
//! **No decoding.** The bytes are sniffed, the label is decided, and the text that comes
//! out is what a lossless pass over those bytes produces -- the adapter hands the text in
//! already decoded by whatever it used, and this module decides what to *call* it. That is
//! the same shape as `Text` in `wire.rs`: the decision is here, the bytes are not.

use std::fmt::Write as _;

/// What an encoding is, as far as this can tell.
///
/// A closed set of things a Windows-machine file actually is, and **not a code page
/// number**: the number belongs to the machine that read the file, and putting it on the
/// wire would be publishing a fact about a target nobody asked about. `Oem` carries the
/// number's *absence* rather than a guess, which is the difference between "read as the
/// machine's code page" and "read as code page 936".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Valid UTF-8, with no byte-order mark.
    Utf8,
    /// UTF-8 with a mark at the front.
    Utf8Bom,
    /// UTF-16, little-endian, with a mark.
    Utf16Le,
    /// UTF-16, big-endian, with a mark.
    Utf16Be,
    /// Not text this can name: read as the machine's OEM code page.
    ///
    /// The fallback, and the one case where the label is not a claim about the bytes --
    /// it says which *rule* was applied. A caller that reads the result and finds it
    /// nonsense now knows the rule, which is the whole of what `docs/ROADMAP.md` M10 asks
    /// for here.
    Oem,
}

impl Encoding {
    /// How a person says it.
    ///
    /// The words a reader needs are "which rule was applied", so `Oem` says what it means
    /// rather than naming a code page it does not know.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf8Bom => "utf-8 with a byte-order mark",
            Self::Utf16Le => "utf-16 little-endian",
            Self::Utf16Be => "utf-16 big-endian",
            Self::Oem => "the machine's OEM code page",
        }
    }

    /// The encoding a name refers to, or `None`.
    ///
    /// A reader of the format should be able to check it against the writer without
    /// opening two crates, which is the same argument as `log::Operation::named`.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "utf-8" => Some(Self::Utf8),
            "utf-8-bom" => Some(Self::Utf8Bom),
            "utf-16le" => Some(Self::Utf16Le),
            "utf-16be" => Some(Self::Utf16Be),
            "oem" => Some(Self::Oem),
            _ => None,
        }
    }

    /// The name this is written as on the wire.
    ///
    /// Separate from [`Encoding::as_str`] because the two have different readers: this one
    /// is parsed by a program, and that one is read by a person who has just seen the
    /// output come out wrong.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Utf8 => "utf-8",
            Self::Utf8Bom => "utf-8-bom",
            Self::Utf16Le => "utf-16le",
            Self::Utf16Be => "utf-16be",
            Self::Oem => "oem",
        }
    }
}

/// What a run of bytes is, as far as the bytes themselves can say.
///
/// **The decision is made from the bytes and never from a guess about the machine**, which
/// is what makes it testable without one. A byte-order mark is a fact; a run of bytes that
/// decodes as UTF-8 is a fact; anything else is `Oem`, and calling it that is honest rather
/// than lazy -- the alternative is naming a code page this side cannot know.
///
/// # The order, and why it is this order
///
/// 1. **A byte-order mark wins.** It is the file saying what it is, and a file that says so
///    is not overruled by how its bytes happen to look. UTF-16 text without being told would
///    otherwise decode as a great many NUL bytes and pass for text.
/// 2. **UTF-16 without a mark is inferred from its NUL pattern**, because legacy Windows
///    tools write it and a reader that treats it as bytes produces a wall of `\0`.
/// 3. **Valid UTF-8 is UTF-8.** This is the case a modern tool produces, and the only one
///    where the whole file can be checked rather than sampled.
/// 4. **Everything else is `Oem`**, which is where every legacy single-byte code page ends
///    up -- and they are indistinguishable from each other by inspection alone, which is
///    why the label says "the machine's code page" instead of naming one.
pub fn sniff(bytes: &[u8]) -> Encoding {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Encoding::Utf8Bom;
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return Encoding::Utf16Le;
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return Encoding::Utf16Be;
    }

    if let Some(encoding) = utf16_without_a_mark(bytes) {
        return encoding;
    }

    if std::str::from_utf8(bytes).is_ok() {
        return Encoding::Utf8;
    }

    Encoding::Oem
}

/// Whether a run of bytes looks like UTF-16 that was written without a mark.
///
/// The test is the NUL pattern a Latin-script text file produces: every other byte is zero,
/// on the low side for little-endian and the high side for big-endian. It needs a run long
/// enough to be a pattern rather than a coincidence -- two bytes of text would satisfy it by
/// accident -- and it must be a *majority* rather than all of them, because a non-ASCII
/// character has no zero in it and a file with a few of those is still UTF-16.
fn utf16_without_a_mark(bytes: &[u8]) -> Option<Encoding> {
    /// Long enough that the pattern means something. Eight bytes is four characters of
    /// Latin text, which is longer than any coincidence worth defending against.
    const AT_LEAST: usize = 8;

    if bytes.len() < AT_LEAST {
        return None;
    }

    let pairs = bytes.len() / 2;
    let odd_zeroes = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
    let even_zeroes = bytes.iter().step_by(2).filter(|b| **b == 0).count();

    // A majority, not all: `U+4E2D` in UTF-16LE is `2D 4E`, with no zero in it at all, and a
    // file of CJK text has plenty of those while still being overwhelmingly NUL-patterned in
    // its ASCII parts. Requiring every other byte to be zero would call such a file `Oem`.
    let majority = pairs / 2 + 1;

    if odd_zeroes >= majority && even_zeroes < majority {
        Some(Encoding::Utf16Le)
    } else if even_zeroes >= majority && odd_zeroes < majority {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// What to look for in a line.
///
/// **A fixed substring, and not a regular expression**, which is the smaller honest step:
/// a pattern language needs a parser, a matcher and its own tests, and the question this
/// exists to answer -- "where is the last ERROR" -- is a substring. A regex engine that got
/// a corner wrong would be a search that returned the wrong lines, and the caller has no
/// way to tell that from a log with different lines in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The text to find.
    pub text: String,
    /// Whether case matters.
    pub case_sensitive: bool,
}

impl Pattern {
    /// A case-sensitive pattern.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: true,
        }
    }

    /// The same pattern, ignoring case.
    pub fn ignoring_case(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            case_sensitive: false,
        }
    }

    /// Whether a line contains this.
    ///
    /// An empty pattern matches every line, which is what asking for a file's content with
    /// `grep` means and is not worth a special case.
    pub fn matches(&self, line: &str) -> bool {
        if self.case_sensitive {
            line.contains(&self.text)
        } else {
            line.to_lowercase().contains(&self.text.to_lowercase())
        }
    }
}

/// How many matches to report, and how many lines of context to show around each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    /// The most matches to return. The scan stops once this many are found.
    pub max_matches: usize,
    /// Lines of context to include on each side of a match.
    pub context: usize,
}

impl Default for Limit {
    /// Twelve matches and no context: enough to see a pattern in a log, small enough that a
    /// reply is never the thing that fails.
    fn default() -> Self {
        Self {
            max_matches: 12,
            context: 0,
        }
    }
}

/// What a search found, and everything a reader needs to judge it.
///
/// The fields are separate on purpose, and the pairs that look redundant are not:
///
/// | question | field |
/// |---|---|
/// | did the search happen at all | [`Search::searched`], [`Search::problem`] |
/// | was the whole file looked at | [`Search::truncated`], [`Search::file_bytes`] |
/// | is the count exact or a floor | [`Search::total`] -- `None` means stopped early |
/// | what was the text assumed to be | [`Search::encoding`] |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    /// The matching lines, in the order they were asked for.
    pub lines: Vec<Match>,
    /// How many matches there are, or `None` when the scan stopped at the limit.
    ///
    /// **`None` is the explicit "stopped early"** and it is not the same as `Some(n)` for a
    /// count that happens to be a floor: a caller that reads `None` as zero searches again.
    pub total: Option<usize>,
    /// Whether the search ran. **False means every other field here is empty because
    /// nothing was looked at**, which is the distinction this whole type is arranged around.
    pub searched: bool,
    /// Why the search could not run, when it could not.
    pub problem: Option<String>,
    /// Whether the file was cut short: past the byte ceiling, or the limit reached.
    pub truncated: bool,
    /// The bytes that were read.
    pub bytes_read: u64,
    /// The file's size, when the machine said.
    pub file_bytes: Option<u64>,
    /// What the bytes were taken to be.
    pub encoding: Encoding,
    /// The path the search was about, echoed.
    pub path: String,
    /// What the count counts, as the summary should say it.
    ///
    /// **`tail` is not a search and must not read like one.** It goes through the same
    /// machinery -- the counts, the truncation, the encoding and the ceiling are the same
    /// questions whatever was asked -- and the summary therefore said `more than 2 match(es)`
    /// for a request that asked for the last two lines. A caller reading that has been told
    /// something untrue about what it asked for, which is the kind of small lie this project
    /// exists to remove.
    pub noun: Noun,
}

/// What a search's lines are, for the sentence that reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noun {
    /// Matching lines, from a search.
    Matches,
    /// Lines from the end of a file, from a read.
    Lines,
}

impl Noun {
    /// The word the summary uses, in both numbers.
    ///
    /// Both, because `1 match(es)` is the kind of thing that makes a reader stop trusting
    /// the rest of the line.
    fn word(self, count: usize) -> &'static str {
        match (self, count == 1) {
            (Self::Matches, true) => "match",
            (Self::Matches, false) => "matches",
            (Self::Lines, true) => "line",
            (Self::Lines, false) => "lines",
        }
    }
}

/// One matching line and the lines around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The line number within the file, counting from one.
    pub number: usize,
    /// The line's text, without its terminator.
    pub text: String,
    /// The context lines before it, nearest last.
    pub before: Vec<String>,
    /// The context lines after it, nearest first.
    pub after: Vec<String>,
}

/// A search that never happened.
///
/// **Not an empty search.** It carries the reason and reports `searched: false`, so a caller
/// that would have concluded "this log has no errors" sees a failure to look instead. That
/// is `docs/ROADMAP.md` M10's first lesson in one constructor.
pub fn could_not_search(path: &str, reason: &str, encoding: Encoding) -> Search {
    Search {
        lines: Vec::new(),
        total: None,
        searched: false,
        problem: Some(reason.to_string()),
        truncated: false,
        bytes_read: 0,
        file_bytes: None,
        encoding,
        path: path.to_string(),
        noun: Noun::Matches,
    }
}

/// Which end of the file a search works from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the start, which is what "where does this first appear" wants.
    First,
    /// From the end, which is what "where is the last ERROR" wants.
    ///
    /// The reason this feature exists rather than a pull and a local grep: the answer is
    /// near the end of a file that may be two gigabytes, and reporting the *last* matches
    /// means the scan can stop as soon as it has them.
    Last,
}

/// The result of scanning lines for a pattern.
///
/// Takes the lines rather than a file, so the same function serves `grep` on a file and a
/// pipeline that already has text in hand -- and so that every test of it is a table.
pub fn scan(lines: &[String], pattern: &Pattern, limit: Limit, direction: Direction) -> Search {
    let matched: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| pattern.matches(line))
        .map(|(index, _)| index)
        .collect();

    let total = if matched.len() > limit.max_matches {
        None
    } else {
        Some(matched.len())
    };

    let chosen: Vec<usize> = match direction {
        Direction::First => matched.iter().take(limit.max_matches).copied().collect(),
        // Taken from the end and put back in file order, because a reader reads downwards.
        Direction::Last => {
            let start = matched.len().saturating_sub(limit.max_matches);
            matched[start..].to_vec()
        }
    };

    let lines_out: Vec<Match> = chosen
        .iter()
        .map(|index| Match {
            number: index + 1,
            text: lines[*index].clone(),
            before: context_before(lines, *index, limit.context),
            after: context_after(lines, *index, limit.context),
        })
        .collect();

    Search {
        truncated: total.is_none(),
        lines: lines_out,
        total,
        searched: true,
        problem: None,
        bytes_read: 0,
        file_bytes: None,
        encoding: Encoding::Utf8,
        path: String::new(),
        noun: Noun::Matches,
    }
}

/// The context lines before an index, nearest last.
fn context_before(lines: &[String], index: usize, context: usize) -> Vec<String> {
    let start = index.saturating_sub(context);
    lines[start..index].to_vec()
}

/// The context lines after an index, nearest first.
fn context_after(lines: &[String], index: usize, context: usize) -> Vec<String> {
    let end = (index + 1 + context).min(lines.len());
    lines[index + 1..end].to_vec()
}

/// Splits text into lines, without their terminators.
///
/// Both terminators, because a file from a Windows target may carry either and a reader that
/// left a `\r` on every line would show it as part of the match.
pub fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// Renders a search as the lines a person or an agent reads.
///
/// **The first line is the summary and it says whether the search happened.** Every other
/// shape was tried in the sibling project and abandoned for this one: a list of matches
/// cannot distinguish "nothing matched" from "nothing was read", and only one of those means
/// the log is clean.
///
/// A match is `line: text`, with the line number first so a reader can go straight to it.
/// Context lines are indented, which is what makes them visibly not matches.
pub fn render(search: &Search) -> String {
    if !search.searched {
        return format!(
            "could not search {}: {}",
            search.path,
            search.problem.as_deref().unwrap_or("no reason given")
        );
    }

    let mut out = format!(
        "{} {} in {}, read as {}",
        match search.total {
            Some(total) => total.to_string(),
            None => format!("more than {}", search.lines.len()),
        },
        search.noun.word(search.total.unwrap_or(search.lines.len())),
        search.path,
        search.encoding.as_str(),
    );
    if search.truncated {
        out.push_str(", stopped early");
    }
    if let Some(size) = search.file_bytes {
        let _ = write!(out, ", {size} bytes");
    }

    for found in &search.lines {
        for line in &found.before {
            let _ = write!(out, "\n  {line}");
        }
        let _ = write!(out, "\n{}: {}", found.number, found.text);
        for line in &found.after {
            let _ = write!(out, "\n  {line}");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        lines_of(text)
    }

    // --- what the bytes are --------------------------------------------------

    #[test]
    fn a_byte_order_mark_is_believed_over_how_the_bytes_look() {
        // The file is saying what it is. UTF-16 text without the mark would otherwise be
        // read as a great many NUL bytes and pass for text.
        assert_eq!(sniff(&[0xEF, 0xBB, 0xBF, b'h', b'i']), Encoding::Utf8Bom);
        assert_eq!(sniff(&[0xFF, 0xFE, b'h', 0]), Encoding::Utf16Le);
        assert_eq!(sniff(&[0xFE, 0xFF, 0, b'h']), Encoding::Utf16Be);
    }

    #[test]
    fn utf16_without_a_mark_is_inferred_from_its_nul_pattern() {
        // `hello` in UTF-16LE, and the same bytes read as text would be `h\0e\0l\0l\0o\0`.
        let little_endian: Vec<u8> = "hello".bytes().flat_map(|byte| [byte, 0]).collect();
        assert_eq!(sniff(&little_endian), Encoding::Utf16Le);

        let big_endian: Vec<u8> = "hello".bytes().flat_map(|byte| [0, byte]).collect();
        assert_eq!(sniff(&big_endian), Encoding::Utf16Be);
    }

    #[test]
    fn a_short_run_of_bytes_is_not_called_utf16_on_the_strength_of_two_nuls() {
        // The pattern has to mean something, and two bytes of anything do not.
        assert_eq!(
            sniff(b"h\0"),
            Encoding::Utf8,
            "not long enough to be a pattern"
        );
        assert_eq!(sniff(b""), Encoding::Utf8, "and an empty file is text");
    }

    #[test]
    fn ordinary_text_is_utf8_and_anything_else_is_the_machines_code_page() {
        assert_eq!(sniff("built 3 targets\n".as_bytes()), Encoding::Utf8);

        // The four bytes from the first real target: GBK for two CJK characters, which is
        // not UTF-8 and is not the machine's code page *known* to be anything -- so the
        // label says which rule was applied rather than naming a code page.
        assert_eq!(sniff(&[0xd6, 0xd0, 0xce, 0xc4]), Encoding::Oem);
    }

    #[test]
    fn every_encoding_survives_being_named_and_read_back() {
        // A tag on the wire that a reader cannot look up is a label nobody can act on.
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
            Encoding::Oem,
        ] {
            assert_eq!(Encoding::named(encoding.tag()), Some(encoding));
            assert!(!encoding.as_str().is_empty());
        }
        assert_eq!(Encoding::named("cp936"), None);
    }

    // --- finding a line ------------------------------------------------------

    #[test]
    fn a_pattern_matches_a_substring_and_case_follows_the_request() {
        let text = lines("ok\nerror: one\nERROR: two\ndone");

        let sensitive = scan(
            &text,
            &Pattern::new("ERROR"),
            Limit::default(),
            Direction::First,
        );
        assert_eq!(sensitive.total, Some(1));
        assert_eq!(sensitive.lines[0].number, 3);

        let ignoring = scan(
            &text,
            &Pattern::ignoring_case("error"),
            Limit::default(),
            Direction::First,
        );
        assert_eq!(ignoring.total, Some(2));
        assert_eq!(ignoring.lines[0].number, 2, "in file order");
    }

    #[test]
    fn the_last_mode_returns_the_end_of_the_file_in_file_order() {
        // **The question this exists for**: "where is the last ERROR" in a two-gigabyte log,
        // answered without moving the file. The matches come back in file order because a
        // reader reads downwards, even though the scan looked from the end.
        let text = lines("e1\nx\ne2\nx\ne3\nx\ne4");

        let last = scan(
            &text,
            &Pattern::new("e"),
            Limit {
                max_matches: 2,
                context: 0,
            },
            Direction::Last,
        );

        assert_eq!(
            last.lines.iter().map(|m| m.number).collect::<Vec<_>>(),
            vec![5, 7],
            "the last two, still in the order they appear"
        );
        assert_eq!(last.total, None, "more matched than were reported");
        assert!(last.truncated);
    }

    #[test]
    fn an_exact_count_is_not_the_same_as_stopping_early() {
        // The distinction a caller branches on: `Some(n)` means the whole file was looked
        // at, `None` means look again with a bigger limit. Reading the second as the first
        // is how "there might be more" becomes "there are none".
        let text = lines("hit\nhit\nhit");

        let complete = scan(
            &text,
            &Pattern::new("hit"),
            Limit {
                max_matches: 5,
                context: 0,
            },
            Direction::First,
        );
        assert_eq!(complete.total, Some(3));
        assert!(!complete.truncated);

        let stopped = scan(
            &text,
            &Pattern::new("hit"),
            Limit {
                max_matches: 2,
                context: 0,
            },
            Direction::First,
        );
        assert_eq!(stopped.total, None, "there may be more");
        assert!(stopped.truncated);
        assert_eq!(stopped.lines.len(), 2);
    }

    #[test]
    fn context_lines_are_carried_on_both_sides_of_a_match() {
        let text = lines("a\nb\nERROR\nc\nd");

        let search = scan(
            &text,
            &Pattern::new("ERROR"),
            Limit {
                max_matches: 5,
                context: 1,
            },
            Direction::First,
        );

        assert_eq!(search.lines[0].before, vec!["b".to_string()]);
        assert_eq!(search.lines[0].after, vec!["c".to_string()]);
    }

    #[test]
    fn a_search_that_could_not_run_is_not_a_search_that_found_nothing() {
        // **The first lesson, as one test.** Every field has to agree: nothing was looked
        // at, there is a reason, and no count -- so a caller cannot read `total` as zero
        // and conclude the log is clean.
        let search = could_not_search("build.log", "no such file", Encoding::Utf8);

        assert!(!search.searched);
        assert_eq!(search.total, None);
        assert!(search.lines.is_empty(), "there is nothing to report");
        assert_eq!(search.problem.as_deref(), Some("no such file"));

        let rendered = render(&search);
        assert!(
            rendered.starts_with("could not search build.log"),
            "{rendered}"
        );
        assert!(
            !rendered.contains("0 match"),
            "a failure to look must not read as a count: {rendered}"
        );
    }

    #[test]
    fn a_search_that_did_run_says_the_encoding_it_won_with() {
        // **The second lesson.** A reader who sees mojibake has to be able to find out what
        // was assumed, which is the difference between a guess and a guess that is believed.
        let text = lines("");
        let mut search = scan(
            &text,
            &Pattern::new("x"),
            Limit::default(),
            Direction::First,
        );
        search.path = "old.log".to_string();
        search.encoding = Encoding::Oem;
        search.total = Some(0);

        let rendered = render(&search);
        assert_eq!(
            rendered,
            "0 matches in old.log, read as the machine's OEM code page"
        );
    }

    #[test]
    fn a_match_is_rendered_as_a_line_number_and_its_text() {
        let text = lines("first\nERROR here\nlast");
        let search = scan(
            &text,
            &Pattern::new("ERROR"),
            Limit::default(),
            Direction::First,
        );

        let rendered = render(&search);
        assert!(rendered.contains("2: ERROR here"), "{rendered}");
        assert!(rendered.starts_with("1 match in "), "{rendered}");
    }
}
