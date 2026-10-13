//! Presentation budgets preserve facts and complete query entries.
use linklet_core::{
    ToolOutcome,
    presentation::{CONTENT_BUDGET, present},
};
use serde_json::json;
#[test]
fn batch_streams_share_one_budget_and_keep_their_ends() {
    let records: Vec<_> = (0..4).map(|i| ToolOutcome::ok("full output").at(format!("host:{i}")).with_data(json!({
        "exit_code":0,"stdout":{"text":format!("{}END{i}", "\u{4e2d}".repeat(50000)),"total_bytes":150004,"retained_bytes":150004,"truncated":false,"lossy":false},
        "stderr":{"text":if i==0 {"small"} else {""}}
    })).structured()).collect();
    let input = ToolOutcome::ok("4 of 4 ran").with_data(json!({"results":records,"count":4}));
    let output = present(&input);
    let mut used = output.text.len();
    for (i, record) in output.data["results"]
        .as_array()
        .expect("results")
        .iter()
        .enumerate()
    {
        let data = &record["data"];
        let text = data["stdout"]["text"].as_str().expect("stdout");
        assert!(text.ends_with(&format!("END{i}")));
        assert!(text.len() > 7000, "fair share");
        assert_eq!(data["stdout"]["presentation_truncated"], true);
        assert_eq!(data["stdout"]["total_bytes"], 150004);
        used += text.len() + data["stderr"]["text"].as_str().expect("stderr").len();
    }
    assert!(used <= CONTENT_BUDGET, "used {used}");
    assert!(!output.is_error());
    assert_eq!(
        input.data["results"][0]["data"]["stdout"]["text"]
            .as_str()
            .expect("original")
            .len(),
        150004
    );
}
#[test]
fn queries_keep_whole_entries_and_the_original_counts() {
    let entries: Vec<_> = (0..100)
        .map(|i| json!({"name":format!("{i}{}","x".repeat(1000)),"size":42}))
        .collect();
    let result = present(
        &ToolOutcome::ok("100 entries")
            .with_data(json!({"entries":entries,"count":100,"total":100,"complete":true})),
    );
    assert_eq!(result.data["count"], 100);
    assert_eq!(result.data["presentation_truncated"], true);
    let shown = result.data["entries"].as_array().expect("entries");
    assert!(!shown.is_empty() && shown.len() < 100);
    assert!(
        shown
            .iter()
            .all(|e| e["name"].as_str().expect("name").len() >= 1001)
    );
}

#[test]
fn check_and_discovery_entries_share_the_same_budget() {
    for field in ["results", "networks"] {
        let entries: Vec<_> = (0..100)
            .map(|i| json!({"target":format!("host-{i}"),"detail":"x".repeat(1000)}))
            .collect();
        let shown =
            present(&ToolOutcome::ok("query").with_data(json!({field:entries,"count":100})));
        assert!(shown.data[field].as_array().expect("entries").len() < 100);
        assert_eq!(shown.data["count"], 100);
    }
}
