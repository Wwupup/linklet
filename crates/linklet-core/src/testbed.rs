//! Testbed requirements: what a machine must look like before a test means
//! anything, and whether it looks that way.
//!
//! # The problem this solves
//!
//! A test that runs against a machine somebody prepared by hand is not a test.
//! It passes because the machine happened to be in the right state, it cannot be
//! repeated tomorrow, and when it fails nobody can tell whether the code broke
//! or the machine was wrong. Worse for an agent: a human "just knows" the
//! machine is ready, and an agent cannot know anything of the kind.
//!
//! # The shape of the answer
//!
//! **A testbed is not a virtual machine. It is a specification plus a checker.**
//! A VM is one way to satisfy the specification; a spare PC, a cloud instance
//! and an already-configured target are others. What makes a testbed usable by
//! an agent is not how it was built but whether its state can be *decided*
//! without a person looking at it.
//!
//! That distinction matters because it decouples the two halves:
//!
//! - **Producing** a machine is expensive, needs privileges, and is different on
//!   every platform. It is deliberately not in this module.
//! - **Deciding** whether a machine is ready is cheap, needs nothing, and is the
//!   same everywhere. It is all this module does.
//!
//! An agent that can decide readiness can direct a test on any target, including
//! one it did not create. An agent that can only create VMs can test on exactly
//! the platform it was written for.
//!
//! # The rules this enforces
//!
//! 1. **Every requirement is a fact that can be observed.** There is no
//!    "the machine looks fine". A requirement an agent cannot check is a
//!    requirement that will be assumed.
//! 2. **Absence is checked, not assumed.** A test that needs a clean machine is
//!    a test that needs the *absence* of yesterday's process, and absence is the
//!    thing hand-prepared machines get wrong.
//! 3. **A failed check says what was seen.** "Requirement not met" sends the
//!    agent nowhere; "expected a file, the directory holds three other names"
//!    sends it somewhere.

use std::fmt::Write as _;

/// One thing that must be true of a machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requirement {
    /// Something answers at this `host:port`.
    Reachable(String),
    /// A path exists. `kind` is `file` or `dir`.
    Present {
        /// The path, as written in the specification.
        path: String,
        /// `file` or `dir`.
        kind: PathKind,
    },
    /// A path does not exist.
    ///
    /// The check that hand-prepared machines fail. A leftover `.part` file, a
    /// stale output directory or yesterday's log all change what a test means,
    /// and none of them are visible to someone who has not been told to look.
    Absent {
        /// The path, as written in the specification.
        path: String,
    },
    /// No running process matches this name.
    NoProcess(String),
}

/// What a path must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// A file.
    File,
    /// A directory.
    Dir,
}

/// A named set of requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Testbed {
    /// What this testbed is for, for the summary line.
    pub name: String,
    /// What must be true.
    pub requirements: Vec<Requirement>,
}

/// What the checker observed for one requirement.
///
/// Written by the prober, which is the only part that touches a machine. Every
/// variant carries the observation, not a verdict: deciding whether an
/// observation satisfies a requirement is this module's job, and keeping the two
/// apart is what makes the decision testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// A connection was accepted.
    Answered,
    /// Nothing is listening, but the machine answered.
    Refused,
    /// Nothing came back in time.
    NoAnswer,
    /// The path exists.
    Exists,
    /// The path does not exist.
    Missing,
    /// The path exists but is not of the required kind.
    WrongKind {
        /// What it actually is.
        found: PathKind,
    },
    /// The observation could not be made, with the reason.
    ///
    /// Not a failure of the requirement: it is the checker saying it does not
    /// know, which is a third answer and has to be reported as one.
    Unknown(String),
    /// Running processes whose names matched, for a `NoProcess` requirement.
    Processes(Vec<String>),
}

/// Looks at a machine. The only part of this module that is not pure.
pub trait Prober {
    /// Observes one requirement.
    fn observe(&self, requirement: &Requirement) -> Observation;
}

/// What happened to one requirement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The requirement, as written.
    pub requirement: Requirement,
    /// Whether it held.
    pub held: bool,
    /// What was seen, in words an agent can act on.
    pub detail: String,
}

impl Testbed {
    /// Parses a specification.
    ///
    /// The format is line-based, has no quoting and no escaping, and is
    /// deliberately one step above a shell script. An agent writes these, and
    /// the failure mode of a richer format is an agent generating something
    /// almost right: a missing quote, a trailing comma, a comment in the wrong
    /// place. This format has almost nothing to get wrong.
    ///
    /// ```text
    /// # a comment
    /// name tcp-refused
    /// require reachable 127.0.0.1:8765
    /// require artifact dist\app.exe present
    /// require artifact dist\out present
    /// forbid  artifact dist\app.exe.part present
    /// require no-process app.exe
    /// ```
    ///
    /// # Errors
    ///
    /// [`SpecError`] naming the line number, because a specification that
    /// silently ignores a line it did not understand is a specification that
    /// checks less than its author believes.
    pub fn parse(text: &str) -> Result<Self, SpecError> {
        let mut name = None;
        let mut requirements = Vec::new();

        for (index, raw) in text.lines().enumerate() {
            let line_number = index + 1;
            let line = raw.trim();

            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mut words = line.split_whitespace();
            let directive = words.next().unwrap_or_default();

            match directive {
                "name" => {
                    let value = words.collect::<Vec<_>>().join(" ");
                    if value.is_empty() {
                        return Err(SpecError::new(line_number, "name needs a value"));
                    }
                    name = Some(value);
                }
                "require" | "forbid" => {
                    let required = directive == "require";
                    requirements.push(requirement_from(&mut words, required, line_number)?);
                }
                other => {
                    return Err(SpecError::new(
                        line_number,
                        format!("unknown directive {other:?}; expected name, require or forbid"),
                    ));
                }
            }
        }

        let name = name.ok_or_else(|| SpecError::new(0, "the specification has no name"))?;
        if requirements.is_empty() {
            return Err(SpecError::new(
                0,
                "the specification requires nothing, so it can never fail",
            ));
        }

        Ok(Self { name, requirements })
    }

    /// Checks every requirement and reports what held.
    ///
    /// Every requirement is checked even after one fails. Stopping at the first
    /// failure would make an agent fix one thing and run again, which for three
    /// broken requirements is three round trips -- and round trips are the thing
    /// this whole project exists to reduce.
    pub fn check(&self, prober: &dyn Prober) -> Vec<Verdict> {
        self.requirements
            .iter()
            .map(|requirement| {
                let observation = prober.observe(requirement);
                let (held, detail) = judge(requirement, &observation);
                Verdict {
                    requirement: requirement.clone(),
                    held,
                    detail,
                }
            })
            .collect()
    }
}

/// Builds a requirement from the words after `require` or `forbid`.
fn requirement_from<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    required: bool,
    line: usize,
) -> Result<Requirement, SpecError> {
    let kind = words
        .next()
        .ok_or_else(|| SpecError::new(line, "expected reachable, artifact or no-process"))?;

    match (kind, required) {
        ("reachable", true) => {
            let target = words
                .next()
                .ok_or_else(|| SpecError::new(line, "reachable needs a host:port"))?;
            Ok(Requirement::Reachable(target.to_string()))
        }
        // `forbid reachable` is expressible and has no use: a machine that must
        // not be reachable is a machine this tool has nothing to say about.
        ("reachable", false) => Err(SpecError::new(
            line,
            "forbid reachable is not a thing; a target that must not answer is not a testbed",
        )),
        ("artifact", true) => {
            let path = words
                .next()
                .ok_or_else(|| SpecError::new(line, "artifact needs a path"))?;
            let shape = words
                .next()
                .ok_or_else(|| SpecError::new(line, "artifact needs present or dir"))?;
            match shape {
                "present" => Ok(Requirement::Present {
                    path: path.to_string(),
                    kind: PathKind::File,
                }),
                "dir" => Ok(Requirement::Present {
                    path: path.to_string(),
                    kind: PathKind::Dir,
                }),
                other => Err(SpecError::new(
                    line,
                    format!("unknown shape {other:?}; expected present or dir"),
                )),
            }
        }
        // The `present` word is required even for `forbid`, so that reading a
        // specification top to bottom gives the same sentence for both kinds of
        // line. `forbid artifact X present` says what it means.
        ("artifact", false) => {
            let path = words
                .next()
                .ok_or_else(|| SpecError::new(line, "artifact needs a path"))?;
            let shape = words
                .next()
                .ok_or_else(|| SpecError::new(line, "artifact needs present"))?;
            if shape != "present" {
                return Err(SpecError::new(
                    line,
                    format!("unknown shape {shape:?}; a forbidden artifact is written present"),
                ));
            }
            Ok(Requirement::Absent {
                path: path.to_string(),
            })
        }
        ("no-process", true) => {
            let name = words
                .next()
                .ok_or_else(|| SpecError::new(line, "no-process needs a process name"))?;
            Ok(Requirement::NoProcess(name.to_string()))
        }
        ("no-process", false) => Err(SpecError::new(
            line,
            "forbid no-process means a process must be running, which is written \
             as a test's own business, not a testbed requirement",
        )),
        (other, _) => Err(SpecError::new(
            line,
            format!("unknown requirement {other:?}; expected reachable, artifact or no-process"),
        )),
    }
}

/// Decides whether an observation satisfies a requirement, and says what was
/// seen.
///
/// A free function of two arguments, so every case is a line in a table rather
/// than a branch buried in a loop. This is where the "what does it mean" logic
/// lives, and it is why the prober only has to report facts.
fn judge(requirement: &Requirement, observed: &Observation) -> (bool, String) {
    match (requirement, observed) {
        (Requirement::Reachable(target), Observation::Answered) => {
            (true, format!("{target} answered"))
        }
        (Requirement::Reachable(target), Observation::Refused) => (
            false,
            format!("{target} refused the connection: the machine is up, nothing is listening"),
        ),
        (Requirement::Reachable(target), Observation::NoAnswer) => {
            (false, format!("{target} did not answer"))
        }

        (Requirement::Present { path, kind }, Observation::Exists) => {
            (true, format!("{path} is a {kind}"))
        }
        (Requirement::Present { path, .. }, Observation::Missing) => {
            (false, format!("{path} does not exist"))
        }
        (Requirement::Present { path, kind }, Observation::WrongKind { found }) => (
            false,
            format!("{path} is a {found} and a {kind} was required"),
        ),

        (Requirement::Absent { path }, Observation::Missing) => {
            (true, format!("{path} is not there"))
        }
        (Requirement::Absent { path }, Observation::Exists) => (
            false,
            format!("{path} is still there, and the test assumes it is not"),
        ),

        (Requirement::NoProcess(name), Observation::Processes(found)) if found.is_empty() => {
            (true, format!("nothing named {name} is running"))
        }
        (Requirement::NoProcess(name), Observation::Processes(found)) => {
            (false, format!("{name} is still running as {found:?}"))
        }

        // An observation that does not fit the requirement is a bug in the
        // prober, not a failure of the machine. Reported as not-held, because a
        // requirement nobody could check is not a requirement that held -- and
        // the detail says which, so the agent does not go looking at the
        // machine for a fault that is in this code.
        (requirement, observation) => (
            false,
            format!("the prober answered {observation:?}, which does not fit {requirement:?}"),
        ),
    }
}

impl std::fmt::Display for PathKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::File => "file",
            Self::Dir => "directory",
        })
    }
}

/// A specification that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecError {
    /// The 1-based line, or 0 for a problem with the document as a whole.
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

impl SpecError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for SpecError {}

/// Renders a check as the lines an agent reads.
///
/// Fixed shape, one line per requirement, then a verdict. The properties that
/// matter are the ones an agent's parser depends on: the first word of a line is
/// always `PASS`, `FAIL` or the verdict, and the detail after it is a fact rather
/// than a sentence about a fact.
pub fn render(testbed: &Testbed, verdicts: &[Verdict], target: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "testbed {}  target {target}", testbed.name);

    for verdict in verdicts {
        let _ = writeln!(
            out,
            "{} {}: {}",
            if verdict.held { "PASS" } else { "FAIL" },
            describe(&verdict.requirement),
            verdict.detail
        );
    }

    let held = verdicts.iter().filter(|verdict| verdict.held).count();
    let _ = write!(
        out,
        "{} {} of {} requirements met",
        if held == verdicts.len() {
            "READY"
        } else {
            "NOT READY"
        },
        held,
        verdicts.len()
    );
    out
}

/// A requirement as the short phrase a verdict line starts with.
///
/// Kept separate from [`std::fmt::Display`] for [`Requirement`] so that the
/// output format is one function somebody can read, rather than a trait
/// implementation spread over an enum.
pub fn describe(requirement: &Requirement) -> String {
    match requirement {
        Requirement::Reachable(target) => format!("reachable {target}"),
        Requirement::Present { path, kind } => format!("artifact {path} is a {kind}"),
        Requirement::Absent { path } => format!("artifact {path} is gone"),
        Requirement::NoProcess(name) => format!("no-process {name}"),
    }
}
