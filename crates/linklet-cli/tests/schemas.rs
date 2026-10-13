//! Validate the actual published schemas, including successful-data requirements.
use serde_json::json;

#[test]
fn every_tool_rejects_a_success_without_its_facts() {
    let definitions = linklet_core::tool_list_json();
    for tool in definitions.as_array().expect("tools") {
        let schema = jsonschema::validator_for(&tool["outputSchema"]).expect("valid schema");
        let empty = json!({"status":"ok","target":null,"data":{},"error":null,"warnings":[]});
        assert!(
            !schema.is_valid(&empty),
            "{} accepted missing successful facts",
            tool["name"]
        );
        let failure = json!({"status":"unknown","target":"host:8787","data":{},"error":{"code":"result_unknown","phase":"await_reply","message":"lost"},"warnings":[]});
        assert!(
            schema.is_valid(&failure),
            "{} rejected a valid failure",
            tool["name"]
        );
    }
}

#[test]
fn execution_schemas_require_one_target_form_and_one_execution_form() {
    let definitions = linklet_core::tool_list_json();
    for name in ["exec", "spawn"] {
        let tool = definitions
            .as_array()
            .expect("tools")
            .iter()
            .find(|t| t["name"] == name)
            .expect("tool");
        let schema = jsonschema::validator_for(&tool["inputSchema"]).expect("schema");
        let mut args = json!({"agent":"host:8787","program":"helper","args":["", "--help"]});
        if name == "spawn" {
            args["output"] = "out.log".into();
        }
        assert!(schema.is_valid(&args), "{args}");
        args["command"] = "helper".into();
        assert!(!schema.is_valid(&args));
        args.as_object_mut().expect("args").remove("command");
        args["agents"] = json!(["other:8787"]);
        assert!(!schema.is_valid(&args));
    }
}
