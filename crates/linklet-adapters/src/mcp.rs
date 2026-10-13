//! The MCP server: newline-delimited JSON-RPC over stdio.
//!
//! MCP's stdio transport is one JSON message per line, so this file is a loop
//! and a `match`. The interesting parts are all elsewhere: [`serde_json`]
//! reads and writes the messages, and [`linklet_core::tool`] decides what a call
//! means. What is left here is the protocol plumbing, and plumbing is the part
//! that should be boring enough to read in one sitting.
//!
//! # What this deliberately does not do
//!
//! - **No `Content-Length` framing.** That is LSP's transport, not MCP's over
//!   stdio. Reading the wrong protocol produces a server that hangs at the first
//!   message with no error, which is a memorable afternoon.
//! - Only the implemented MCP version is advertised during initialization.
//! - **No batching.** MCP does not use JSON-RPC batches, and accepting one would
//!   mean an array where an object is expected, at a point where a wrong guess
//!   silently ignores half a request.

use std::io::{BufRead, Write};

use linklet_core::{dispatch, tool_list_json};
use serde_json::Value;

/// The MCP version implemented by this server and returned during negotiation.
pub const FALLBACK_PROTOCOL_VERSION: &str = "2025-06-18";

/// The name and version this server reports about itself.
pub const SERVER_NAME: &str = "linklet";
/// The version this server reports about itself.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What to do with one message.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send this reply.
    Reply(Value),
    /// Send nothing. A notification, or a request the client cancelled.
    Ignore,
}

/// The closure a tool call goes through.
///
/// What a tool call goes through.
///
/// Named so the signature in [`handle`] stays readable, and so the integration
/// test can supply the same thing the binary does. It is the core's own trait
/// rather than a closure declared here: this crate decides how to talk to a
/// machine, not what a tool is, and a closure type defined here would put that
/// decision on the wrong side of the line the whole layout rests on.
pub type RunTool<'a> = dyn linklet_core::ToolRunner + 'a;

/// Handles one message and says what to send back.
///
/// Pure: no reading, no writing, no exit. That is what makes the protocol
/// testable without a client -- the integration test drives a real process, but
/// the cases that matter (a notification gets no reply, a bad request gets the
/// right error code) are checked here in microseconds.
pub fn handle(message: &Value, run_tool: &RunTool<'_>) -> Action {
    // A notification has no id, and JSON-RPC forbids replying to one. Getting
    // this wrong earns a client that logs "unexpected response id: null" once
    // per notification.
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return id.map_or(Action::Ignore, |id| {
            Action::Reply(error_reply(&id, -32600, "a request needs a \"method\""))
        });
    };

    let Some(id) = id else {
        // A notification. `notifications/initialized` is the one MCP sends, and
        // the correct answer to all of them is silence.
        return Action::Ignore;
    };

    let params = message.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "initialize" => Action::Reply(reply(
            &id,
            serde_json::json!({
                "protocolVersion": FALLBACK_PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
            }),
        )),
        "tools/list" => Action::Reply(reply(&id, serde_json::json!({"tools": tool_list_json()}))),

        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

            let outcome = match dispatch(name, &arguments, run_tool) {
                Ok(outcome) => outcome,
                Err(error) => {
                    linklet_core::ToolOutcome::failure(linklet_core::result::OperationError::new(
                        linklet_core::result::ErrorCode::InvalidArgument,
                        "validate",
                        error.to_string(),
                    ))
                }
            };
            let outcome = linklet_core::presentation::present(&outcome);
            Action::Reply(reply(
                &id,
                serde_json::json!({
                    "content": [{"type": "text", "text": outcome.text}],
                    "isError": outcome.is_error(),
                    "structuredContent": outcome.structured()
                }),
            ))
        }

        "ping" => Action::Reply(reply(&id, Value::Object(Default::default()))),

        other => Action::Reply(error_reply(
            &id,
            -32601,
            &format!("unsupported method {other:?}"),
        )),
    }
}

fn reply(id: &Value, result: Value) -> Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Reads messages from `input`, writes replies to `output`, until input ends.
///
/// Returns the number of messages handled, which is what a test asserts on
/// rather than reaching into the process.
///
/// # Errors
///
/// Any I/O failure from reading or writing. A malformed *message* is not an
/// error: it produces a JSON-RPC error reply and the loop continues, because a
/// server that dies on one bad line is a server that a stray byte can kill.
pub fn serve<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    run_tool: &RunTool<'_>,
) -> std::io::Result<usize> {
    let mut handled = 0;

    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let action = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message, run_tool),
            // The id is not known because the message did not parse, so the
            // error carries a null id -- which is what JSON-RPC specifies for
            // exactly this case.
            Err(error) => Action::Reply(error_reply(
                &Value::Null,
                -32700,
                &format!("invalid JSON: {error}"),
            )),
        };

        if let Action::Reply(message) = action {
            // One message per line, and the newline is what makes it a message.
            writeln!(output, "{message}")?;
            output.flush()?;
        }

        handled += 1;
    }

    Ok(handled)
}
