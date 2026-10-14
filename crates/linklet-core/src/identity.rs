//! Authenticated, timestamped facts about the target process.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
/// Target identity and the process context observed for a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Agent product name.
    pub name: String,
    /// Binary version.
    pub version: String,
    /// Operating-system hostname, if observed.
    #[serde(default)]
    pub hostname: Option<String>,
    /// Operating system of the agent binary.
    #[serde(default)]
    pub os: Option<String>,
    /// Architecture of the agent binary.
    #[serde(default)]
    pub arch: Option<String>,
    /// Actual account, not an environment-variable claim.
    #[serde(default)]
    pub account: Option<String>,
    /// Whether the current process is elevated; null when not observed.
    #[serde(default)]
    pub elevated: Option<bool>,
    /// Shell used for command strings.
    #[serde(default)]
    pub shell: Option<String>,
    /// Base directory for relative file paths.
    #[serde(default)]
    pub startup_cwd: Option<String>,
    /// UTC Unix timestamp in milliseconds at observation.
    #[serde(default)]
    pub observed_at_ms: Option<u64>,
    /// Startup runtime snapshot; null for peers that did not report one.
    #[serde(default)]
    pub runtimes: Option<RuntimeSnapshot>,
    /// Published capability ceilings.
    #[serde(default)]
    pub limits: Value,
    /// Missing or incomplete context diagnostics.
    #[serde(default)]
    pub warnings: Vec<String>,
}
impl Identity {
    /// Identity with auxiliary fields explicitly unknown.
    pub fn named(name: &str, version: &str) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            hostname: None,
            os: None,
            arch: None,
            account: None,
            elevated: None,
            shell: None,
            startup_cwd: None,
            observed_at_ms: None,
            runtimes: None,
            limits: json!({}),
            warnings: Vec::new(),
        }
    }
    /// JSON representation, without credentials.
    pub fn to_json(&self) -> Value {
        json!(self)
    }
    /// Readable runtime context for clients that display only tool text.
    /// Starts with a newline, or is empty when the peer did not report runtimes.
    pub fn runtime_summary(&self) -> String {
        let Some(snapshot) = &self.runtimes else {
            return String::new();
        };
        let mut text = format!(
            "\nruntimes (startup snapshot at {} Unix ms; restart agent to refresh):",
            snapshot.observed_at_ms
        );
        for runtime in &snapshot.entries {
            let detail = match &runtime.result {
                RuntimeResult::Available { program, version } => {
                    format!("available {version}; program={program}")
                }
                RuntimeResult::NotFound { reason } => format!("not_found: {reason}"),
                RuntimeResult::Failed { reason } => format!("failed: {reason}"),
                RuntimeResult::TimedOut { reason } => format!("timed_out: {reason}"),
            };
            text.push_str(&format!("\n{}: {detail}", runtime.name.as_str()));
        }
        text
    }
}

/// Bounded runtime observations made in the agent's startup environment.
/// Restart the agent to refresh; these facts do not promise future availability,
/// third-party packages, or permission to perform a particular operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    /// UTC Unix milliseconds when probing began.
    pub observed_at_ms: u64,
    /// One observation per runtime family, not an inventory of installed versions.
    pub entries: Vec<Runtime>,
}

/// An interpreter family checked by Linklet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeName {
    /// Python 3, using python3 or python on PATH.
    Python,
    /// Windows PowerShell, using powershell.exe on PATH.
    Powershell,
    /// PowerShell, using pwsh on PATH.
    Pwsh,
}

impl RuntimeName {
    /// Stable name used in runtime results and human-readable summaries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Powershell => "powershell",
            Self::Pwsh => "pwsh",
        }
    }
}

/// One runtime family's observed execution capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runtime {
    /// Interpreter family.
    pub name: RuntimeName,
    /// Execution result, flattened into the runtime object on the wire.
    #[serde(flatten)]
    pub result: RuntimeResult,
}

/// Result of running a small noninteractive program in a candidate interpreter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeResult {
    /// Code ran successfully and returned a path and version.
    Available {
        /// Absolute interpreter path usable with exec/spawn program and literal args.
        program: String,
        /// Version reported by the running interpreter.
        version: String,
    },
    /// No candidate was found in the documented PATH search; not a machine inventory.
    NotFound {
        /// Search scope that found no candidate.
        reason: String,
    },
    /// A candidate existed but could not start or complete a valid probe.
    Failed {
        /// Launch, exit, output, or validation diagnostic.
        reason: String,
    },
    /// The family's probe budget expired; availability remains unknown.
    TimedOut {
        /// Which bounded observation could not complete.
        reason: String,
    },
}
impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}
