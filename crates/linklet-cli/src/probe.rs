//! Authenticated usability probe shared by CLI and MCP; no lifecycle actions.
use super::CommandOutcome;
use linklet_client::{CallError, Liveness};
use linklet_core::auth::Token;
use std::time::Duration;
/// Connected peer did not return a valid identity in time.
pub const NO_ANSWER: i32 = 4;
const PROBE_BUDGET: Duration = Duration::from_secs(2);
/// Maps authenticated identity and typed failures to the five probe exit codes.
pub fn run_probe(agent: &str, token: &Token) -> CommandOutcome {
    let result = linklet_client::AgentAddress::new(agent).and_then(|address| {
        linklet_client::probe(&address.with_token(token.clone()), PROBE_BUDGET)
    });
    let error = match result {
        Ok(Liveness::Answered(identity)) => {
            let mut result = CommandOutcome::answered(format!("answered {agent}: {identity}"), 0)
                .facts(agent, identity.to_json());
            result.tool.warnings = identity.warnings;
            return result;
        }
        Ok(Liveness::Refused(error)) => CallError::Refused(error),
        Err(error) => error,
    };
    let code = match &error {
        CallError::BadAddress(_) | CallError::Local(_) | CallError::PairingRequired(_) => 2,
        CallError::Connect(_) => 1,
        CallError::Refused(_) | CallError::VersionMismatch(_) => 3,
        _ => NO_ANSWER,
    };
    let mut result = CommandOutcome::call_failed(agent, &error);
    let prefix = match code {
        1 => "nothing listening",
        3 => "refused",
        _ => "no answer",
    };
    result.tool.text = format!("{prefix} from {agent}: {}", result.tool.text);
    result.code = code;
    result
}
