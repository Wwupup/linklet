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
///
/// What [`parse_targets`] does enforce is narrow, and deliberately so: a host
/// is a non-empty run of characters with no colon and no whitespace. That is
/// all. `"!!!"` and `"not-a-host..really"` pass it, because this crate has no
/// way to tell a strange name from a clever one -- and refusing a name that a
/// real resolver would have accepted is the more expensive mistake. The check
/// that matters for those is the connection attempt, which produces a failure
/// naming the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Host(String);

impl Host {
    /// Wraps text as a host, without checking it.
    ///
    /// Public mainly so that a *caller writing an expectation* can build one --
    /// a test needs to say "I expect this target", and a type with no
    /// constructor cannot be named in an answer. Nothing is validated here, and
    /// that is not an oversight: [`parse_targets`] is the way in for input,
    /// because it is the piece that knows the grammar. There is very little to
    /// check anyway -- whatever this is handed, the connection attempt is what
    /// decides whether it means anything.
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

/// A TCP port.
///
/// Two guarantees, and it is worth knowing which one comes from where:
///
/// - **The range is the type's.** A `Port` holds a `u16`, so a value out of
///   `0..=65535` cannot be represented. There is no check to get wrong.
/// - **"Someone meant it" is the parser's.** Zero and 65536 are in range for a
///   `u16` and are not ports; [`parse_targets`] is what refuses them.
///
/// The range check therefore happens once, in the parser, and every later layer
/// can take a `Port` and stop asking whether it is valid. What is *not* true is
/// that the type makes an invalid port unrepresentable -- [`Port::new`] is
/// public and will happily hold `0`. The first draft of this comment claimed
/// otherwise, which was a guarantee the type never had. What actually holds is
/// narrower and worth stating precisely: every `Port` that came out of
/// [`parse_targets`] is valid, and that is the only route a caller has for
/// reading input.
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

/// Renders a target as `host:port`, which is how it was written.
///
/// One deliberate imprecision: an IPv6 host comes out unbracketed, so `::1` on
/// port 80 prints as `::1:80`, which reads as a different address. That is the
/// same ambiguity the *input* grammar has and resolves the same way -- the port
/// is after the last colon -- so parsing this output back recovers the target.
/// A bracketed form would be more conventional and would not round-trip through
/// [`parse_targets`], which is the property worth keeping.
impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// Turns the caller's text into targets, or says precisely why it cannot.
///
/// Grammar (the whole of it -- anything not listed is an error, never a guess):
///
/// ```text
/// input   := spec ("," spec)*
/// spec    := host ":" port
/// host    := one or more characters, with no ":" and no whitespace
/// port    := digits, value 1..=65535
/// ```
///
/// Note what the grammar does **not** say: there is no `[":" port]`, so a spec
/// without a port is not a shorter form of a spec, it is not a spec. The
/// default port is a decision that belongs to the caller, not a guess made
/// here -- guessing it would make `"a"` mean something in this tool that the
/// caller never said.
///
/// The grammar is permissive about hosts on purpose: `host` is not a hostname
/// grammar, and `"!!!"` fits it. See [`Host`] for why refusing a strange name
/// would cost more than accepting one.
///
/// Two things the bullets above leave out, both of which are decided rather
/// than accidental:
///
/// - Whitespace around a spec is not part of it, so `" a:1 , b:2 "` is two
///   targets.
/// - A spec that is empty after trimming is an error, not a skipped entry. A
///   trailing comma is a typo, and silently ignoring it teaches the caller that
///   the input does not matter.
/// - The port separator is the **last** `:`, so `::1:80` means host `::1`,
///   port `80`.
///
/// # The order the rules are applied in
///
/// A spec can break more than one rule at once, and then the *error* depends on
/// which rule is checked first. That is not an implementation detail: if it is
/// left unstated, two reasonable implementations disagree, both look right, and
/// whoever wrote the second one spends an afternoon believing they are wrong.
///
/// The order below is the specification. Each step applies to the whole spec
/// (after trimming), never to a fragment:
///
/// 1. the input, trimmed, is not empty -- else [`TargetError::EmptyInput`]
/// 2. the spec, trimmed, is not empty -- else [`TargetError::EmptySpec`]. This
///    is what a trailing comma hits.
/// 3. the spec contains no whitespace -- else
///    [`TargetError::WhitespaceInSpec`]. **Before any splitting**: `"a:1 b:2"`
///    is one spec containing a space, and reporting `PortNotANumber` for it
///    would describe a fragment the caller never wrote as a separate thing.
/// 4. split at the **last** `:`; if there is none, the port is absent --
///    [`TargetError::PortMissing`]
/// 5. the port text is not empty -- else [`TargetError::PortMissing`]
/// 6. the host text is not empty -- else [`TargetError::PortMissing`]. A port
///    with nothing in front of it names a machine that was never written, and
///    there is no better error for "you wrote `:80`".
/// 7. the port text is all ASCII digits -- else
///    [`TargetError::PortNotANumber`]
/// 8. the port value is in `1..=65535`, saturating to `u64::MAX` rather than
///    "too big to parse" -- else [`TargetError::PortOutOfRange`]
///
/// The failing cases in `tests/target_parsing.rs` each break exactly one rule,
/// except where a test says otherwise. That is deliberate: a test that breaks
/// two rules at once is testing the order, not the rule, and should say so.
///
/// # Errors
///
/// Returns the first spec that does not fit the grammar as a [`TargetError`].
/// It does not collect every failure: an agent acting on a list of targets is
/// stopped by the first bad one either way, and a partial success is the worst
/// possible answer -- the caller cannot tell whether the good entries in it
/// were all of them.
pub fn parse_targets(input: &str) -> Result<Vec<Target>, TargetError> {
    // Rule 1, against the whole input rather than against the first spec.
    // These two cases look identical from here and mean different things to the
    // caller: "" is "you asked for nothing", ",," is "you typed something
    // wrong", and an empty first spec would report the second for the first.
    if input.trim().is_empty() {
        return Err(TargetError::EmptyInput);
    }

    let mut targets = Vec::new();

    for raw in input.split(',') {
        let spec = raw.trim();

        // Rule 2.
        if spec.is_empty() {
            return Err(TargetError::EmptySpec);
        }

        // Rule 3, before anything is split. A spec with a space in it is one
        // spec, and the error names all of it: reporting the fragment "1 b"
        // would describe a string the caller never wrote.
        if spec.chars().any(char::is_whitespace) {
            return Err(TargetError::WhitespaceInSpec {
                spec: spec.to_string(),
            });
        }

        // Rule 4. `rsplit_once`, not `split_once`: an IPv6 host is full of
        // colons, and the port is always the part after the last one.
        let (host, port) = match spec.rsplit_once(':') {
            Some((host, port)) => (host, port),
            // No colon at all: the port is absent, which is rule 5's answer too.
            None => (spec, ""),
        };

        // Rules 5 and 6 share a variant because they share a situation: no port
        // was named. What separates them is only which half is empty, and no
        // caller acts differently on that.
        if port.is_empty() || host.is_empty() {
            return Err(TargetError::PortMissing {
                spec: spec.to_string(),
            });
        }

        // Rule 7. Checked before parsing, so that "1 b" cannot arrive as a
        // parse failure and be reported as one.
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(TargetError::PortNotANumber {
                spec: spec.to_string(),
                port: port.to_string(),
            });
        }

        // Rule 8. Parsed into a wider type and saturated rather than rejected:
        // a number too large for u64 is still a number, and the useful sentence
        // to a caller is "that is not a port", not "that is not a number".
        let wide = port
            .parse::<u128>()
            .map(|value| value.min(u128::from(u64::MAX)) as u64)
            .unwrap_or(u64::MAX);

        if wide == 0 || wide > u64::from(u16::MAX) {
            return Err(TargetError::PortOutOfRange {
                spec: spec.to_string(),
                value: wide,
            });
        }

        targets.push(Target {
            host: Host(host.to_string()),
            port: Port(wide as u16),
        });
    }

    Ok(targets)
}
