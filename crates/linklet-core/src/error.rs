//! Why a target specification was rejected.

/// A rejected target specification.
///
/// Each variant carries **the text that was rejected**, not just a description
/// of the problem. That is a deliberate cost: a bare `InvalidInput` forces the
/// caller to guess which of several specs it came from, and an agent reporting
/// a failure to a user needs to quote the exact thing the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The whole input was empty or only whitespace.
    EmptyInput,
    /// One entry of the list was empty, for example `"a:1,,b:2"`.
    EmptySpec,
    /// A spec contained whitespace inside it, for example `"a b:1"`.
    WhitespaceInSpec {
        /// The spec, after trimming.
        spec: String,
    },
    /// A spec named no port at all, with or without a trailing `:`, for example
    /// `"a"` or `"a:"`. Both are the same fact -- there is no port here -- and
    /// reporting them as different errors would make the caller handle one
    /// situation twice.
    PortMissing {
        /// The spec, after trimming.
        spec: String,
    },
    /// The port was not made of digits, for example `"a:http"`.
    PortNotANumber {
        /// The spec, after trimming.
        spec: String,
        /// The text where the digits should have been.
        port: String,
    },
    /// The port was digits but outside `1..=65535`, for example `"a:0"`.
    PortOutOfRange {
        /// The spec, after trimming.
        spec: String,
        /// The number that was out of range.
        value: u64,
    },
}

impl std::fmt::Display for TargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "no targets given"),
            Self::EmptySpec => write!(f, "an entry in the target list is empty"),
            Self::WhitespaceInSpec { spec } => {
                write!(f, "target {spec:?} contains whitespace")
            }
            Self::PortMissing { spec } => {
                write!(f, "target {spec:?} ends with ':' and no port")
            }
            Self::PortNotANumber { spec, port } => {
                write!(f, "target {spec:?} has a non-numeric port {port:?}")
            }
            Self::PortOutOfRange { spec, value } => {
                write!(f, "target {spec:?} has port {value}, outside 1..=65535")
            }
        }
    }
}

impl std::error::Error for TargetError {}
