//! Authenticated usability probe shared by CLI and MCP; no process lifecycle actions.
use super::CommandOutcome;
use linklet_client::{CallError, Liveness, render_call_error};
use linklet_core::auth::Token;
use std::time::Duration;

/// A connected peer did not return a valid identity in time.
pub const NO_ANSWER: i32 = 4;
const PROBE_BUDGET: Duration = Duration::from_secs(2);

/// Verifies a valid authenticated identity and maps failures to the five probe codes.
pub fn run_probe(agent: &str, token: &Token) -> CommandOutcome {
    let address = match linklet_client::AgentAddress::new(agent) {
        Ok(address) => address,
        Err(error) => return outcome(render_call_error(&error), 2),
    };
    let address = address.with_token(token.clone());
    match linklet_client::probe(&address, PROBE_BUDGET) {
        Ok(Liveness::Answered(identity)) => outcome(format!("answered {agent}: {identity}"), 0),
        Ok(Liveness::Refused(reason)) => outcome(format!("refused {agent}: {reason}"), 3),
        Err(error) => {
            let code = match &error {
                CallError::BadAddress(_) | CallError::Local(_) => 2,
                CallError::Connect(_) => 1,
                CallError::Refused(_) | CallError::VersionMismatch(_) => 3,
                _ => NO_ANSWER,
            };
            let prefix = if code == 1 {
                "nothing listening"
            } else {
                "no answer"
            };
            outcome(
                format!("{prefix} from {agent}: {}", render_call_error(&error)),
                code,
            )
        }
    }
}
fn outcome(text: String, code: i32) -> CommandOutcome {
    let mut result = CommandOutcome::answered(text, code);
    result.tool.is_error = code != 0;
    result
}
