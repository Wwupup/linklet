//! The MCP server: newline-delimited JSON-RPC over stdio.
//!
//! MCP's stdio transport is one JSON message per line, so this file is a loop
//! and a `match`. The interesting parts are all elsewhere: [`linklet_core::json`]
//! reads and writes the messages, and [`linklet_core::tool`] decides what a call
//! means. What is left here is the protocol plumbing, and plumbing is the part
//! that should be boring enough to read in one sitting.
//!
//! # What this deliberately does not do
//!
//! - **No `Content-Length` framing.** That is LSP's transport, not MCP's over
//!   stdio. Reading the wrong protocol produces a server that hangs at the first
//!   message with no error, which is a memorable afternoon.
//! - **No version negotiation logic.** See [`handle`]: the client's version is
//!   echoed back. Checking it would be checking a string against a date, and the
//!   only thing that matters is whether the client can parse the replies, which
//!   the client is better placed to decide.
//! - **No batching.** MCP does not use JSON-RPC batches, and accepting one would
//!   mean an array where an object is expected, at a point where a wrong guess
//!   silently ignores half a request.

use std::io::{BufRead, Write};

use linklet_core::json::{self, Json};
use linklet_core::{ToolError, dispatch, tool_list_json};

/// The protocol version this server replies with when the client did not say.
///
/// Only a fallback. A client that names a version gets its own back, so this
/// constant is not a compatibility claim -- it is what to write when there is
/// nothing to echo.
pub const FALLBACK_PROTOCOL_VERSION: &str = "2025-06-18";

/// The name and version this server reports about itself.
pub const SERVER_NAME: &str = "linklet";
/// The version this server reports about itself.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What to do with one message.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send this reply.
    Reply(Json),
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
pub fn handle(message: &Json, run_tool: &RunTool<'_>) -> Action {
    // A notification has no id, and JSON-RPC forbids replying to one. Getting
    // this wrong earns a client that logs "unexpected response id: null" once
    // per notification.
    let id = message.get("id").cloned();
    let Some(method) = message.get_str("method") else {
        return id.map_or(Action::Ignore, |id| {
            Action::Reply(error_reply(&id, -32600, "a request needs a \"method\""))
        });
    };

    let Some(id) = id else {
        // A notification. `notifications/initialized` is the one MCP sends, and
        // the correct answer to all of them is silence.
        return Action::Ignore;
    };

    let params = message.get("params").cloned().unwrap_or(Json::Null);

    match method {
        "initialize" => Action::Reply(reply(
            &id,
            json::parse(&format!(
                r#"{{
                    "protocolVersion": {},
                    "capabilities": {{"tools": {{"listChanged": false}}}},
                    "serverInfo": {{"name": {}, "version": {}}}
                }}"#,
                json::write(
                    &params
                        .get_str("protocolVersion")
                        .map_or_else(|| Json::str(FALLBACK_PROTOCOL_VERSION), Json::str)
                ),
                json::write(&Json::str(SERVER_NAME)),
                json::write(&Json::str(SERVER_VERSION)),
            ))
            .expect("the template above is a literal and parses"),
        )),

        "tools/list" => Action::Reply(reply(
            &id,
            json::parse(&format!(
                r#"{{"tools": {}}}"#,
                json::write(&tool_list_json())
            ))
            .expect("a literal"),
        )),

        "tools/call" => {
            let name = params.get_str("name").unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| Json::Object(std::collections::BTreeMap::new()));

            match dispatch(name, &arguments, run_tool) {
                Ok(outcome) => Action::Reply(reply(
                    &id,
                    json::parse(&format!(
                        r#"{{"content": [{{"type": "text", "text": {}}}], "isError": {}}}"#,
                        json::write(&Json::str(outcome.text)),
                        outcome.is_error
                    ))
                    .expect("a literal"),
                )),
                // A dispatch failure is a *tool* failure, not a protocol one:
                // the request was well formed and the answer is "you called it
                // wrong". It goes back as a result with `isError`, so the agent
                // sees a message it can act on rather than a transport fault.
                Err(error) => Action::Reply(reply(
                    &id,
                    json::parse(&format!(
                        r#"{{"content": [{{"type": "text", "text": {}}}], "isError": true}}"#,
                        json::write(&Json::str(error_message(&error)))
                    ))
                    .expect("a literal"),
                )),
            }
        }

        "ping" => Action::Reply(reply(&id, Json::Object(Default::default()))),

        other => Action::Reply(error_reply(
            &id,
            -32601,
            &format!("unsupported method {other:?}"),
        )),
    }
}

/// What the agent is told when a call could not be dispatched.
///
/// The message names the field, because an agent retrying has to change
/// something and "invalid arguments" gives it nothing to change.
fn error_message(error: &ToolError) -> String {
    error.to_string()
}

fn reply(id: &Json, result: Json) -> Json {
    Json::Object(
        [
            ("jsonrpc".to_string(), Json::str("2.0")),
            ("id".to_string(), id.clone()),
            ("result".to_string(), result),
        ]
        .into_iter()
        .collect(),
    )
}

fn error_reply(id: &Json, code: i64, message: &str) -> Json {
    Json::Object(
        [
            ("jsonrpc".to_string(), Json::str("2.0")),
            ("id".to_string(), id.clone()),
            (
                "error".to_string(),
                Json::Object(
                    [
                        ("code".to_string(), Json::Int(code)),
                        ("message".to_string(), Json::str(message)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    )
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

        let action = match json::parse(&line) {
            Ok(message) => handle(&message, run_tool),
            // The id is not known because the message did not parse, so the
            // error carries a null id -- which is what JSON-RPC specifies for
            // exactly this case.
            Err(error) => Action::Reply(error_reply(
                &Json::Null,
                -32700,
                &format!("invalid JSON: {error}"),
            )),
        };

        if let Action::Reply(message) = action {
            // One message per line, and the newline is what makes it a message.
            // Writing it separately from the body is why `json::write` promises
            // no trailing newline of its own.
            writeln!(output, "{}", json::write(&message))?;
            output.flush()?;
        }

        handled += 1;
    }

    Ok(handled)
}
