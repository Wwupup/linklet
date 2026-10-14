//! Runtime observations survive the identity protocol without inventing availability.
use linklet_core::wire::{Reply, identity_from_json};
use serde_json::json;

#[test]
fn identity_preserves_verified_runtimes_and_their_observation_time() {
    let runtimes = json!({"observed_at_ms":1234,"entries":[
        {"name":"python","status":"available","program":"C:\\Python\\python.exe","version":"3.13.1"},
        {"name":"powershell","status":"not_found","reason":"not found on PATH"},
        {"name":"pwsh","status":"timed_out","reason":"probe deadline reached"}
    ]});
    let identity = identity_from_json(&Reply::Result(json!({
        "name":"linklet-agent","version":"0.4.1","runtimes":runtimes
    })))
    .expect("valid identity");
    assert_eq!(identity.to_json()["runtimes"], runtimes);
}

#[test]
fn older_identity_leaves_runtime_availability_unknown() {
    let identity = identity_from_json(&Reply::Result(json!({
        "name":"linklet-agent","version":"0.4.1"
    })))
    .expect("older identity");
    assert_eq!(identity.to_json()["runtimes"], serde_json::Value::Null);
}

#[test]
fn available_runtime_requires_a_program_and_version() {
    for entry in [
        json!({"name":"python","status":"available"}),
        json!({"name":"python","status":"available","program":"","version":"3.13"}),
        json!({"name":"python","status":"available","program":"/usr/bin/python3","version":""}),
        json!({"name":"python","status":"installed"}),
    ] {
        assert!(
            identity_from_json(&Reply::Result(json!({
                "name":"linklet-agent","version":"0.4.1",
                "runtimes":{"observed_at_ms":1234,"entries":[entry]}
            })))
            .is_err(),
            "invalid observation accepted: {entry}"
        );
    }
}

#[test]
fn mcp_presentation_keeps_runtime_context_in_readable_text() {
    let mut outcome =
        linklet_core::ToolOutcome::ok("paired\nruntimes: python 3.13 at /usr/bin/python3");
    outcome.data = json!({"runtimes":{"observed_at_ms":1234,"entries":[]}});
    assert!(
        linklet_core::presentation::present(&outcome)
            .text
            .contains("/usr/bin/python3")
    );
}
