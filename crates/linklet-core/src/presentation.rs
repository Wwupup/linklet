//! Bounded MCP presentation; raw CLI outcomes remain unchanged.
use crate::ToolOutcome;
use serde_json::{Value, json};
/// Fixed budget for output text and query entries in one MCP result.
pub const CONTENT_BUDGET: usize = 32 * 1024;

fn prefix(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn slots(value: &Value, path: &str, found: &mut Vec<(String, usize, bool)>) {
    if let Some(fields) = value.as_object() {
        if let Some(text) = fields.get("text").and_then(Value::as_str)
            && fields.contains_key("total_bytes")
        {
            found.push((path.into(), text.len(), true));
            return;
        }
        for (key, child) in fields {
            let next = format!("{path}/{key}");
            let query_results = key == "results"
                && child
                    .as_array()
                    .is_some_and(|entries| entries.iter().all(|e| e.get("status").is_none()));
            if query_results
                || matches!(
                    key.as_str(),
                    "lines" | "entries" | "processes" | "listeners" | "networks" | "skipped"
                )
            {
                if let Some(entries) = child.as_array() {
                    found.push((
                        next,
                        entries.iter().map(|e| e.to_string().len()).sum(),
                        false,
                    ));
                }
            } else {
                slots(child, &next, found);
            }
        }
    } else if let Some(entries) = value.as_array() {
        for (index, child) in entries.iter().enumerate() {
            slots(child, &format!("{path}/{index}"), found);
        }
    }
}
/// Fair byte shares, redistributing the unused shares of small or empty outputs.
fn shares(sizes: &[usize], mut budget: usize) -> Vec<usize> {
    let mut result = vec![0; sizes.len()];
    let mut pending: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i] > 0).collect();
    while !pending.is_empty() && budget > 0 {
        let each = (budget / pending.len()).max(1);
        pending.retain(|&i| {
            let granted = each.min(sizes[i] - result[i]).min(budget);
            result[i] += granted;
            budget -= granted;
            result[i] < sizes[i]
        });
    }
    result
}
/// Produces a bounded MCP view without deleting status, target, exit codes or counts.
/// Command text keeps its tail. Query entries are delivered whole. Cursor pages
/// are budgeted before their cursor is issued, so this function never cuts them.
pub fn present(outcome: &ToolOutcome) -> ToolOutcome {
    let mut result = outcome.clone();
    let runtime_context = outcome.data.get("runtimes").is_some_and(Value::is_object);
    result.text = if runtime_context {
        prefix(&outcome.text, 4096)
    } else {
        prefix(outcome.text.lines().next().unwrap_or(""), 1024)
    }
    .into();
    let mut content = Vec::new();
    slots(&result.data, "", &mut content);
    let limits = shares(
        &content.iter().map(|(_, size, _)| *size).collect::<Vec<_>>(),
        CONTENT_BUDGET - result.text.len(),
    );
    let mut shortened = runtime_context && result.text.len() < outcome.text.len();
    for ((path, size, stream), limit) in content.into_iter().zip(limits) {
        if stream {
            if let Some(value) = result.data.pointer_mut(&path) {
                let text = value["text"].as_str().unwrap_or("");
                let mut start = text.len().saturating_sub(limit);
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                let text = text[start..].to_string();
                let omitted = text.len() < size;
                value["text"] = text.into();
                value["presentation_truncated"] = omitted.into();
                shortened |= omitted;
            }
        } else {
            let mut count = 0;
            if let Some(entries) = result.data.pointer_mut(&path).and_then(Value::as_array_mut) {
                let mut remaining = limit;
                for entry in entries.iter() {
                    let bytes = entry.to_string().len();
                    if bytes > remaining {
                        break;
                    }
                    remaining -= bytes;
                    count += 1;
                }
                let omitted = count < entries.len();
                entries.truncate(count);
                shortened |= omitted;
                if let Some((parent, field)) = path.rsplit_once('/')
                    && let Some(data) = result.data.pointer_mut(parent)
                {
                    if data.get("displayed_counts").is_none() {
                        data["displayed_counts"] = json!({});
                    }
                    data["displayed_counts"][field] = count.into();
                    if !matches!(field, "networks" | "skipped") {
                        data["displayed_count"] = count.into();
                    }
                    let previous = data["presentation_truncated"].as_bool().unwrap_or(false);
                    data["presentation_truncated"] = (previous || omitted).into();
                }
            }
        }
    }
    if shortened {
        result.warnings.push("MCP content was shortened to the shared 32 KiB budget. Discarded command output is not recoverable; no implicit log was saved.".into());
    }
    result.data["presentation"] =
        json!({"content_budget_bytes":CONTENT_BUDGET,"truncated":shortened});
    result
}
