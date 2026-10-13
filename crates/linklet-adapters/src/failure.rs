//! Convert system failures into stable operation diagnostics at the I/O boundary.
use linklet_core::result::{ErrorCode, OperationError};
/// Preserves the original OS diagnostic and the path it applies to.
pub fn io(error: std::io::Error, phase: &str, path: &std::path::Path) -> OperationError {
    let code = match error.kind() {
        std::io::ErrorKind::NotFound => ErrorCode::NotFound,
        std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
        std::io::ErrorKind::InvalidInput => ErrorCode::InvalidArgument,
        std::io::ErrorKind::TimedOut => ErrorCode::DeadlineExceeded,
        _ => ErrorCode::IoError,
    };
    OperationError::new(code, phase, format!("{}: {error}", path.display()))
}
