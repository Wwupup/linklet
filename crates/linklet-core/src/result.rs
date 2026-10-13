//! Operation facts shared by command-line and MCP presentation.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Whether an operation completed, failed, or has an unknown outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A valid result, including empty queries and negative observations.
    Ok,
    /// A known failure.
    Error,
    /// A side effect may have happened but its outcome was not received.
    Unknown,
}
/// Stable categories, never inferred from human-readable messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Invalid invocation.
    InvalidArgument,
    /// Cannot load or save local configuration.
    LocalConfigError,
    /// No saved target credential.
    PairingRequired,
    /// Authentication was rejected.
    AuthenticationFailed,
    /// Both binaries need the same protocol.
    ProtocolMismatch,
    /// Could not establish a connection.
    ConnectionFailed,
    /// Access denied by the operating system.
    PermissionDenied,
    /// A path or program does not exist.
    NotFound,
    /// A known command exit was unsuccessful.
    CommandFailed,
    /// The time allowance expired.
    DeadlineExceeded,
    /// No trustworthy result after a possible side effect.
    ResultUnknown,
    /// Invalid reply shape or authentication.
    InvalidReply,
    /// Remote request declined.
    Refused,
    /// Another filesystem or process failure.
    IoError,
    /// An invariant or worker failed.
    InternalError,
}
/// Machine-readable failure and its original diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationError {
    /// Stable category.
    pub code: ErrorCode,
    /// Observed phase, for example validate, connect, execute, or await_reply.
    pub phase: String,
    /// Original diagnostic including relevant path or OS details.
    pub message: String,
}
impl OperationError {
    /// Builds a diagnostic without parsing its message.
    pub fn new(code: ErrorCode, phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            phase: phase.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for OperationError {}
/// Facts and the full command-line presentation of an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolOutcome {
    /// Success, failure, or uncertainty after possible execution.
    pub status: Status,
    /// Address, or null for local and multi-target operations.
    pub target: Option<String>,
    /// Operation-specific structured facts.
    pub data: Value,
    /// Failure independent of output completeness.
    pub error: Option<OperationError>,
    /// Limits or incomplete observations that do not fail the operation.
    pub warnings: Vec<String>,
    /// Full CLI text; MCP has a separate bounded presentation.
    #[serde(skip)]
    pub text: String,
}
impl ToolOutcome {
    /// A successful operation with a human-readable presentation.
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            status: Status::Ok,
            target: None,
            data: json!({}),
            error: None,
            warnings: Vec::new(),
            text: text.into(),
        }
    }
    /// An internal failure. Use a specific category at known boundaries.
    pub fn failed(text: impl Into<String>) -> Self {
        Self::failure(OperationError::new(ErrorCode::InternalError, "local", text))
    }
    /// A typed failure, retaining uncertainty separately.
    pub fn failure(error: OperationError) -> Self {
        let mut result = Self::ok(error.message.clone());
        result.fail(error);
        result
    }
    /// Attaches structured facts.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }
    /// Attaches a target.
    pub fn at(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }
    /// True for a known failure or an unknown outcome.
    pub fn is_error(&self) -> bool {
        self.status != Status::Ok
    }
    /// Records a failure while preserving observed data.
    pub fn fail(&mut self, error: OperationError) {
        self.status = if error.code == ErrorCode::ResultUnknown {
            Status::Unknown
        } else {
            Status::Error
        };
        self.error = Some(error);
    }
    /// Structured MCP result, without duplicating the CLI text.
    pub fn structured(&self) -> Value {
        json!({"status":self.status,"target":self.target,"data":self.data,
            "error":self.error,"warnings":self.warnings})
    }
}
/// Common schema with operation-specific data.
pub fn output_schema(data: Value) -> Value {
    json!({"type":"object","additionalProperties":false,
    "required":["status","target","data","error","warnings"],
    "properties":{
        "status":{"type":"string","enum":["ok","error","unknown"]},
        "target":{"type":["string","null"]}, "data":data,
        "error":{"anyOf":[{"type":"null"},{"type":"object","additionalProperties":false,
            "required":["code","phase","message"],"properties":{
                "code":{"type":"string"},"phase":{"type":"string"},"message":{"type":"string"}}}]},
        "warnings":{"type":"array","items":{"type":"string"}}
    }})
}

impl From<String> for OperationError {
    fn from(message: String) -> Self {
        Self::new(ErrorCode::Refused, "validate", message)
    }
}
impl From<&str> for OperationError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

impl From<&String> for OperationError {
    fn from(message: &String) -> Self {
        message.as_str().into()
    }
}
