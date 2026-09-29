//! What a target is, and how one is written.

use crate::TargetError;

/// A host, as the caller wrote it.
///
/// Deliberately **not** resolved to an IP address here, and not validated as a
/// hostname. Reasons, in order of how much they cost to get wrong:
///
/// - Resolution needs DNS, which is I/O, which cannot live in this crate.
/// - Whether a name is valid is a question only the resolver can answer. A
///   regex that "validates" hostnames rejects real ones and accepts fake ones.
/// - An agent often needs to echo back exactly what the user typed. Round
///   tripping through a normalising type loses that.
///
/// So this type carries the text unchanged, and the question "does this name
/// exist" belongs to the adapter that resolves it, at the moment it tries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Host(String);

impl Host {
    /// Wraps text that has already been checked to be a usable host.
    ///
    /// Public mainly so that a *caller writing an expectation* can build one --
    /// a test needs to say "I expect this target", and a type with no
    /// constructor cannot be named in an answer. The checked route in is still
    /// [`parse_targets`]; using this to read input skips the validation, which
    /// is exactly why the parser does not use it on raw input.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The host exactly as it was written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A TCP port that is known to be in `1..=65535`.
///
/// The range check happens once, in [`parse_targets`]. Every later layer can
/// therefore take a `Port` and stop asking whether it is valid -- which is the
/// general shape of a good type: it makes an invalid state unrepresentable
/// rather than making every caller re-check for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Port(u16);

impl Port {
    /// Wraps a port that has already been range-checked.
    ///
    /// The parameter type does most of the work: a `u16` cannot hold 0 or
    /// 65536 by the time it gets here, so the only remaining question is
    /// whether the caller meant a real port. `parse_targets` answers that;
    /// this constructor is for callers building a value they already know.
    pub fn new(value: u16) -> Self {
        Self(value)
    }

    /// The port number.
    pub fn get(self) -> u16 {
        self.0
    }
}

impl std::fmt::Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One machine to talk to, at one port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// The host as written by the caller.
    pub host: Host,
    /// The port to reach it on.
    pub port: Port,
}

/// Turns the caller's text into targets, or says precisely why it cannot.
///
/// Grammar (the whole of it -- anything not listed is an error, never a guess):
///
/// ```text
/// input   := spec ("," spec)*
/// spec    := host [":" port]
/// host    := one or more characters, with no ":" and no whitespace
/// port    := digits, value 1..=65535
/// ```
///
/// - Whitespace around a spec is not part of it, so `" a:1 , b:2 "` is two
///   targets.
/// - A spec that is empty after trimming is an error, not a skipped entry. A
///   trailing comma is a typo, and silently ignoring it teaches the caller
///   that the input does not matter.
/// - The port separator is the **last** `:`, so `::1:80` means host `::1`,
///   port `80`.
/// - A spec with no port at all is also an error. The default port is a
///   decision that belongs to the caller, not a guess made here.
///
/// # Errors
///
/// Returns the first spec that does not fit the grammar as a [`TargetError`].
/// It does not collect every failure: an agent acting on a list of targets is
/// stopped by the first bad one either way, and a partial success is the worst
/// possible answer -- the caller cannot tell whether the good entries in it
/// were all of them.
pub fn parse_targets(_input: &str) -> Result<Vec<Target>, TargetError> {
    // TODO(you): implement this to the grammar above.
    //
    // Before writing a line, answer these three for yourself:
    //   1. Which failure do I hit first if the input is ""? What about ","?
    //   2. Where does the port range check live, and can it be reached twice?
    //   3. The tests in tests/target_parsing.rs are the specification. Read
    //      them before implementing; run `cargo test -p linklet-core` and watch
    //      them fail first.
    unimplemented!("parse_targets is the first thing to build -- see tests/target_parsing.rs")
}
