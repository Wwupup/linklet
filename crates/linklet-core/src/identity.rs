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
            limits: json!({}),
            warnings: Vec::new(),
        }
    }
    /// JSON representation, without credentials.
    pub fn to_json(&self) -> Value {
        json!(self)
    }
}
impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}
