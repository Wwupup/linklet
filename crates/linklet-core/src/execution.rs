//! Shell commands and direct argument vectors, without platform I/O.
use crate::wire::WireError;
use serde_json::{Value, json};

/// Exactly one way to start a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    /// A complete command line interpreted by the target shell.
    Shell(String),
    /// A program and its literal argument vector, without shell interpretation.
    Direct {
        /// Executable name or path; relative paths use the invocation directory.
        program: String,
        /// Literal arguments, including empty strings.
        args: Vec<String>,
    },
}
impl From<String> for CommandSpec {
    fn from(value: String) -> Self {
        Self::Shell(value)
    }
}
impl From<&str> for CommandSpec {
    fn from(value: &str) -> Self {
        Self::Shell(value.into())
    }
}
impl std::fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shell(line) => f.write_str(line),
            Self::Direct { program, args } => write!(f, "{program:?} {args:?}"),
        }
    }
}
impl CommandSpec {
    /// Reads mutually exclusive command/program fields, rejecting arguments on shell mode.
    pub fn from_json(value: &Value) -> Result<Self, WireError> {
        let bad = |message: &str| WireError::BadRequest(message.into());
        if value.get("command").is_some() == value.get("program").is_some() {
            return Err(bad("provide command or program, not both"));
        }
        let text = |name| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
                .map(str::to_string)
                .ok_or_else(|| bad("command/program: expected nonempty text without NUL"))
        };
        if value.get("command").is_some() {
            if value.get("args").is_some() {
                return Err(bad("args requires program"));
            }
            return Ok(Self::Shell(text("command")?));
        }
        let args = match value.get("args") {
            None => Vec::new(),
            Some(Value::Array(args)) => args
                .iter()
                .map(|a| {
                    a.as_str()
                        .filter(|a| !a.contains('\0'))
                        .map(str::to_string)
                        .ok_or_else(|| bad("args: expected strings without NUL"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => return Err(bad("args: expected an array")),
        };
        Ok(Self::Direct {
            program: text("program")?,
            args,
        })
    }
    /// Wire fields; direct arguments are never joined into a command line.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Shell(command) => json!({"command": command}),
            Self::Direct { program, args } => json!({"program":program,"args":args}),
        }
    }
    /// Whether the reported process identifier belongs to the shell or direct program.
    pub fn pid_kind(&self) -> &'static str {
        match self {
            Self::Shell(_) => "shell",
            Self::Direct { .. } => "program",
        }
    }
}
/// Reads an optional per-invocation directory, rejecting empty or NUL paths.
pub fn cwd_from_json(value: &Value) -> Result<Option<String>, WireError> {
    match value.get("cwd") {
        None => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() && !s.contains('\0') => Ok(Some(s.clone())),
        _ => Err(WireError::BadRequest(
            "cwd: expected a nonempty path without NUL".into(),
        )),
    }
}
