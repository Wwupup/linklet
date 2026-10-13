//! Shared validation of replies from actual MCP subprocesses.
use serde_json::Value;
pub fn validate(name: &str, reply: &Value) {
    let definitions = linklet_core::tool_list_json();
    let Some(tool) = definitions
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == name)
    else {
        return;
    };
    let schema = jsonschema::validator_for(&tool["outputSchema"]).expect("schema");
    let facts = &reply["structuredContent"];
    if let Err(error) = schema.validate(facts) {
        panic!(
            "{name}: {error}; result {}",
            facts.to_string().chars().take(4000).collect::<String>()
        );
    }
    assert_eq!(reply["isError"].as_bool(), Some(facts["status"] != "ok"));
}
