//! Reading a command's flags into its options.
//!
//! This is the primitive the command line is built on: **a table of the flags a verb knows, and
//! the arguments after it**. It holds no state and does no I/O, so it is tested in microseconds
//! and its whole behaviour fits in a table -- the same argument `linklet-core::target` makes for
//! itself one module over.
//!
//! # Why this exists, in one number
//!
//! `--agent` was parsed **nine times, in three different styles**, and they had drifted: three
//! said `--agent needs a host:port` when the value was missing and five said `--agent needs a
//! value`. Nine copies stayed green individually because each was only ever tested against
//! itself, so nothing compared them. This module is the single place that decides, and
//! `crates/linklet-cli/tests/arguments.rs` is the test that compares the commands.
//!
//! # The line this draws
//!
//! A command is either **flags only** or **flags and then a tail**:
//!
//! - `linklet ls --agent a:1 --from logs` is flags only. An unrecognised `--flag` is the
//!   caller's typo, and [`Strictness::Strict`] refuses it by name.
//! - `linklet exec --agent a:1 myapp.cmd --verbose` ends in a command, and `--verbose` belongs
//!   to *that* command. [`Strictness::AllowWords`] passes it through.
//!
//! Folding the two together either way breaks something real: strict everywhere makes
//! `exec` unable to run a command that has flags of its own, and lenient everywhere turns a
//! misspelled `--agnet` into a silently ignored option and a command that ran somewhere the
//! caller did not ask for.

use std::collections::BTreeMap;

/// A flag a command knows, and whether it must be followed by a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// Including the leading dashes, as the caller types it.
    pub name: &'static str,
    /// Whether a value must follow it.
    ///
    /// A switch takes no value, so `--force x` has `x` as a word rather than as the switch's
    /// value -- which is why a command that takes only switches cannot also take a tail
    /// without ambiguity, and why none of them do.
    pub value: bool,
}

impl Flag {
    /// A flag that must be followed by a value.
    pub const fn takes_value(name: &'static str) -> Self {
        Self { name, value: true }
    }

    /// A flag that stands alone.
    pub const fn switch(name: &'static str) -> Self {
        Self { name, value: false }
    }
}

/// What to do with an argument that is not one of the declared flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strictness {
    /// Any argument that is not a declared flag is a word.
    ///
    /// For a command with a tail, where the words are the command to run.
    AllowWords,
    /// An argument that looks like a flag must be a declared one.
    ///
    /// For a flags-only command, where anything unrecognised is a typo. An argument that does
    /// **not** look like a flag is still a word, because `linklet testbed check spec.txt
    /// host:1` takes two of them.
    Strict,
}

/// Why arguments could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A flag that takes a value was the last argument.
    ///
    /// Separate from [`ParseError::Unknown`] because the two need different sentences: one
    /// names a flag the command knows and the other a flag it does not, and a caller told
    /// "unknown option --agent" for a missing value would go and look for a typo.
    MissingValue {
        /// The flag whose value never arrived.
        flag: String,
    },
    /// A flag this command does not know.
    Unknown {
        /// As the caller typed it.
        flag: String,
    },
    /// A flag that takes a value was followed by another flag.
    ///
    /// **Refused rather than taken as the value**, and this is the case worth arguing about: a
    /// caller who typed `--agent --timeout 5` has made a mistake, and silently taking
    /// `--timeout` as the agent address produces a connection attempt to a host named
    /// `--timeout`. The value of a flag is a value, not whatever came next.
    NotAValue {
        /// The flag whose value was another flag.
        flag: String,
        /// What followed it.
        found: String,
    },
    /// A flag was given more than once.
    Repeated {
        /// Which one.
        flag: String,
    },
}

impl std::fmt::Display for ParseError {
    /// One sentence, naming what is wrong and what the caller typed.
    ///
    /// **The flag is always named and the command is not**: the caller knows which command they
    /// ran, and every message in this project that named both was longer for no reader's
    /// benefit. The one exception is a sentence a caller cannot act on without knowing where it
    /// came from, which is why `main.rs` prefixes its own.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingValue { flag } => write!(f, "{flag} needs a value"),
            Self::Unknown { flag } => write!(f, "unknown option {flag:?}"),
            Self::NotAValue { flag, found } => {
                write!(f, "{flag} needs a value, and {found:?} is another option")
            }
            Self::Repeated { flag } => write!(f, "{flag} was given more than once"),
        }
    }
}

/// The arguments a command was given, read into its options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    values: BTreeMap<String, String>,
    switches: Vec<String>,
    words: Vec<String>,
}

impl Args {
    /// Reads `arguments` against `flags`.
    ///
    /// # Errors
    ///
    /// [`ParseError`] for every way a command's flags can be wrong. **It stops at the first
    /// one**: a caller that gave several things wrong gets told about the first, which is what
    /// a compiler does and for the same reason -- fixing them one at a time is what actually
    /// happens, and a list of four mistakes is longer to read than the mistakes.
    pub fn parse(
        arguments: &[String],
        flags: &[Flag],
        strictness: Strictness,
    ) -> Result<Self, ParseError> {
        let mut args = Self::default();
        let mut index = 0;

        while index < arguments.len() {
            let argument = &arguments[index];
            index += 1;

            // **`--` ends the options**, and everything after it is a word however it looks.
            // This is the escape hatch `AllowWords` needs to be complete rather than nearly
            // complete: a command whose own first word begins with a dash has no other way to
            // say so, and without this the tool would have a class of commands it cannot run.
            if argument == "--" {
                args.words.extend(arguments[index..].iter().cloned());
                break;
            }

            let Some(flag) = flags.iter().find(|flag| flag.name == argument) else {
                if strictness == Strictness::Strict && looks_like_flag(argument) {
                    return Err(ParseError::Unknown {
                        flag: argument.clone(),
                    });
                }
                args.words.push(argument.clone());
                continue;
            };

            if !flag.value {
                args.switches.push(flag.name.to_string());
                continue;
            }

            let Some(value) = arguments.get(index) else {
                return Err(ParseError::MissingValue {
                    flag: flag.name.to_string(),
                });
            };
            if looks_like_flag(value) {
                return Err(ParseError::NotAValue {
                    flag: flag.name.to_string(),
                    found: value.clone(),
                });
            }
            index += 1;

            if args
                .values
                .insert(flag.name.to_string(), value.clone())
                .is_some()
            {
                return Err(ParseError::Repeated {
                    flag: flag.name.to_string(),
                });
            }
        }

        Ok(args)
    }

    /// The value of a flag that takes one, if it was given.
    ///
    /// By name rather than by destructuring, so that a caller reads `args.value("--agent")`
    /// beside the flag it asked for rather than a positional pair that can be swapped silently.
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.values.get(flag).map(String::as_str)
    }

    /// The value of a flag that takes one, or a sentence naming what is missing.
    ///
    /// # Errors
    ///
    /// The flag's own name, so that a caller can say which command wanted it. This is the third
    /// way a command can be invoked wrongly and the one easiest to leave out of a shared parser:
    /// every flag is well formed and the command is still unusable.
    pub fn required(&self, flag: &str) -> Result<&str, String> {
        self.value(flag)
            .ok_or_else(|| format!("{flag} is required"))
    }

    /// A comma- or space-separated list, in the order it was given.
    ///
    /// **Split here rather than at the call site**, because `--agents a,b` and `--agents a b`
    /// are the same request and a caller should not have to know which the shell left intact.
    /// Empty parts are dropped, so a trailing comma is not a machine named "".
    pub fn list(&self, flag: &str) -> Vec<String> {
        self.value(flag)
            .map(|value| {
                value
                    .split([',', ' '])
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a switch was given.
    pub fn switch(&self, name: &str) -> bool {
        self.switches.iter().any(|switch| switch == name)
    }

    /// The arguments that were not flags: a command's tail, or its positional arguments.
    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// The words joined the way a command line is.
    ///
    /// Joined here rather than by the caller so that `exec` and `spawn` cannot disagree about
    /// it: a tool that reassembled a command line from pieces would be a second interpretation
    /// of the caller's quoting, and two of those is one too many.
    pub fn tail(&self) -> String {
        self.words.join(" ")
    }
}

/// Whether an argument looks like a flag rather than a value.
///
/// One dash and one letter counts (`-i` is a real flag in this tool). A lone `-` does not: it is
/// the conventional name for standard input, and a negative number does not either, so that a
/// value like `-1` can be passed to a flag that wants one.
fn looks_like_flag(argument: &str) -> bool {
    let Some(rest) = argument.strip_prefix('-') else {
        return false;
    };
    if let Some(long) = rest.strip_prefix('-') {
        // `--` alone is the terminator and is handled before this is reached, so anything here
        // with two dashes and at least one character after them is a flag.
        return !long.is_empty();
    }

    // One dash: a flag if it is not a number, so that `-1` stays a value.
    !rest.is_empty() && rest.parse::<i64>().is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    /// The flags most of these tests use, which are the ones `linklet ps` really has.
    const FLAGS: &[Flag] = &[
        Flag::takes_value("--agent"),
        Flag::takes_value("--max"),
        Flag::switch("--force"),
    ];

    fn parse(words: &[&str], strictness: Strictness) -> Result<Args, ParseError> {
        Args::parse(&arguments(words), FLAGS, strictness)
    }

    #[test]
    fn a_flag_and_its_value_become_an_option() {
        let args = parse(&["--agent", "a:1"], Strictness::Strict).expect("well formed");

        assert_eq!(args.value("--agent"), Some("a:1"));
        assert_eq!(args.value("--max"), None);
        assert!(args.words().is_empty());
    }

    #[test]
    fn a_missing_value_names_the_flag_that_is_missing_it() {
        // **The drift this module was written to stop.** Nine copies of this said two different
        // things; there is one sentence now, and it names the flag.
        let error = parse(&["--agent"], Strictness::Strict).expect_err("no value");

        assert_eq!(
            error,
            ParseError::MissingValue {
                flag: "--agent".to_string()
            }
        );
        assert_eq!(error.to_string(), "--agent needs a value");
    }

    #[test]
    fn another_flag_where_a_value_belongs_is_refused_rather_than_taken() {
        // **The case worth arguing about.** Taking the next argument would produce a connection
        // attempt to a host named `--max`, and the caller would be told the machine is
        // unreachable rather than that they forgot a value.
        let error = parse(&["--agent", "--max", "5"], Strictness::Strict).expect_err("no value");

        assert_eq!(
            error,
            ParseError::NotAValue {
                flag: "--agent".to_string(),
                found: "--max".to_string()
            }
        );
    }

    #[test]
    fn a_negative_number_is_a_value_and_not_a_flag() {
        // A value is a value. `-1` is what it looks like, and a parser that treated it as a flag
        // would make it impossible to pass one to a flag that takes a number.
        let args = parse(&["--max", "-1"], Strictness::Strict).expect("a negative value");

        assert_eq!(args.value("--max"), Some("-1"));
        assert!(args.words().is_empty());
    }

    #[test]
    fn a_switch_stands_alone_and_does_not_eat_the_next_word() {
        let args = parse(&["--force", "x"], Strictness::AllowWords).expect("a switch and a word");

        assert!(args.switch("--force"));
        assert_eq!(args.words(), ["x"]);
    }

    #[test]
    fn a_strict_command_refuses_an_option_it_does_not_know() {
        // The typo case: `--agnet` must not become a silently ignored option with a command
        // running somewhere the caller did not ask for.
        let error = parse(&["--agnet", "a:1"], Strictness::Strict).expect_err("unknown");

        assert_eq!(
            error,
            ParseError::Unknown {
                flag: "--agnet".to_string()
            }
        );
    }

    #[test]
    fn a_lenient_command_passes_an_option_through_as_a_word() {
        // **The other side of the line.** `linklet exec --agent a:1 myapp.cmd --verbose` has to
        // be possible -- `--verbose` belongs to the command being run, not to this tool.
        let args = parse(
            &["--agent", "a:1", "myapp.cmd", "--verbose"],
            Strictness::AllowWords,
        )
        .expect("a command with a flag of its own");

        assert_eq!(args.value("--agent"), Some("a:1"));
        assert_eq!(args.words(), ["myapp.cmd", "--verbose"]);
        assert_eq!(args.tail(), "myapp.cmd --verbose");
    }

    #[test]
    fn a_strict_command_still_takes_plain_arguments() {
        // Strict is about flags, not about arguments: `testbed check spec.txt host:1` takes two
        // words and neither is a flag.
        let args = parse(&["spec.txt", "host:1"], Strictness::Strict).expect("two words");

        assert_eq!(args.words(), ["spec.txt", "host:1"]);
    }

    #[test]
    fn two_dashes_end_the_options_and_everything_after_is_a_word() {
        // The escape hatch that makes `AllowWords` complete rather than nearly complete: a
        // command whose first word begins with a dash has no other way to say so.
        let args = parse(
            &["--agent", "a:1", "--", "--not-an-option", "x"],
            Strictness::AllowWords,
        )
        .expect("the terminator");

        assert_eq!(args.words(), ["--not-an-option", "x"]);
    }

    #[test]
    fn the_same_flag_twice_is_an_error_rather_than_a_guess() {
        // Last-wins is a decision nobody made; refusing is one the caller can act on.
        let error =
            parse(&["--agent", "a:1", "--agent", "b:2"], Strictness::Strict).expect_err("repeated");

        assert_eq!(
            error,
            ParseError::Repeated {
                flag: "--agent".to_string()
            }
        );
    }

    #[test]
    fn a_required_flag_that_is_absent_is_named_in_the_sentence() {
        // The third way a command can be invoked wrongly: every flag well formed and the command
        // still unusable.
        let args = parse(&["--max", "5"], Strictness::Strict).expect("well formed");

        let error = args.required("--agent").expect_err("no agent");
        assert_eq!(error, "--agent is required");
    }

    #[test]
    fn a_list_splits_on_commas_and_on_spaces_and_drops_the_empties() {
        // `--agents a,b` and `--agents a b` are the same request, and a trailing comma is a
        // typo rather than a machine named "".
        let args = parse(&["--agent", "a:1,b:2, c:3, "], Strictness::Strict).expect("a list");

        assert_eq!(args.list("--agent"), ["a:1", "b:2", "c:3"]);
        assert!(args.list("--max").is_empty());
    }

    #[test]
    fn the_first_mistake_is_the_one_reported() {
        // A caller that got several things wrong is told about the first, which is what a
        // compiler does and for the same reason: fixing them one at a time is what happens, and
        // a list of four is longer to read than the four.
        let error = parse(&["--agent", "--max", "5", "--nope"], Strictness::Strict);

        assert!(
            matches!(error, Err(ParseError::NotAValue { .. })),
            "{error:?}"
        );
    }
}
