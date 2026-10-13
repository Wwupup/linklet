//! Operation-specific MCP schemas. These describe facts, not rendered sentences.
use serde_json::{Value, json};

fn object(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .into_iter()
        .flat_map(|p| p.keys().cloned())
        .collect();
    json!({"type":"object","properties":properties,"required":required})
}
fn array(items: Value) -> Value {
    json!({"type":"array","items":items})
}
fn string() -> Value {
    json!({"type":"string"})
}
fn count() -> Value {
    json!({"type":"integer","minimum":0})
}
fn boolean() -> Value {
    json!({"type":"boolean"})
}
fn nullable(value: Value) -> Value {
    json!({"anyOf":[value,{"type":"null"}]})
}
fn strings() -> Value {
    array(string())
}
fn range() -> Value {
    object(json!({"start":count(),"end":count()}))
}
fn exit_code() -> Value {
    nullable(json!({"type":"integer","minimum":i32::MIN,"maximum":i32::MAX}))
}
fn stream() -> Value {
    let mut value = object(
        json!({"text":string(),"total_bytes":count(),"retained_bytes":count(),
        "encoding":string(),"truncated":boolean(),"lossy":boolean(),"decode_error":nullable(string())}),
    );
    value["properties"]["presentation_truncated"] = boolean();
    value
}
fn execution() -> Value {
    object(
        json!({"cwd":string(),"exit_code":exit_code(),"stdout":stream(),"stderr":stream(),
        "duration_ms":count(),"reason":nullable(string()),"error":crate::result::error_schema()}),
    )
}
fn identity() -> Value {
    object(
        json!({"name":string(),"version":string(),"hostname":nullable(string()),
        "os":nullable(string()),"arch":nullable(string()),"account":nullable(string()),
        "elevated":nullable(boolean()),"shell":nullable(string()),"startup_cwd":nullable(string()),
        "observed_at_ms":nullable(count()),"limits":{"type":["object","null"]},"warnings":strings()}),
    )
}

pub(crate) fn tool(name: &str) -> Value {
    let data = match name {
        "pair" | "probe" => identity(),
        "exec" => json!({"anyOf":[execution(),object(json!({"count":count(),"ok":count(),
            "error":count(),"unknown":count(),"results":array(crate::result::output_schema(execution()))}))]}),
        "spawn" => object(
            json!({"pid":count(),"command":string(),"cwd":string(),"output":string(),
            "pid_kind":{"enum":["program","shell"]},"state":{"enum":["started","running_at_observation","exited"]},
            "observed_ms":count(),"exit_code":exit_code(),"error":crate::result::error_schema(),"tail":stream()}),
        ),
        "push" | "pull" => object(json!({"from":string(),"to":string(),"bytes":count(),
            "sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"verified":{"const":true}})),
        "tail" => object(
            json!({"path":string(),"text":string(),"encoding":string(),"lossy":boolean(),
            "range":range(),"next_cursor":string(),"has_more":boolean(),"pending_bytes":count(),
            "file_bytes":count(),"bytes_read":count(),"count":count(),"starts_mid_line":boolean(),
            "ends_mid_line":boolean(),"file_changed":nullable(string()),"continuity":string(),
            "skipped_prefix":boolean(),"warnings":strings(),"complete":boolean()}),
        ),
        "grep" => object(
            json!({"path":string(),"count":count(),"total":nullable(count()),
            "complete":boolean(),"scope":nullable(range()),"searched":boolean(),
            "problem":crate::result::error_schema(),"truncated":boolean(),"bytes_read":count(),
            "file_bytes":nullable(count()),"encoding":string(),"noun":{"const":"matches"},
            "lines":array(object(json!({"number":count(),"text":string(),"before":strings(),"after":strings()})))}),
        ),
        "ls" => {
            let mut entry = object(json!({"name":string(),"dir":boolean()}));
            entry["properties"]["size"] = count();
            entry["properties"]["modified"] = json!({"type":"integer"});
            object(
                json!({"path":string(),"entries":array(entry),"count":count(),"total":count(),
                "truncated":boolean(),"found":boolean(),"problem":crate::result::error_schema(),
                "complete":boolean(),"scope":object(json!({"path":string(),"recursive":{"const":false}}))}),
            )
        }
        "ps" => {
            let mut process = object(json!({"pid":count(),"name":string()}));
            process["properties"]["path"] = string();
            process["properties"]["cmdline"] = string();
            object(
                json!({"processes":array(process),"count":count(),"total":count(),"unreadable":count(),
                "truncated":boolean(),"complete":boolean(),"incomplete_reason":string(),
                "scope":{"type":"object"},"applied":{"type":"object"},"notes":strings(),
                "problem":crate::result::error_schema(),"missing_fields":strings()}),
            )
        }
        "kill" => {
            let targets = array(object(json!({"pid":count(),"name":string()})));
            object(
                json!({"matched":count(),"killed":targets,"excluded":targets,"failed":targets,
                "complete":boolean(),"notes":strings()}),
            )
        }
        "check" => object(
            json!({"count":count(),"complete":boolean(),"authenticated":{"const":false},
            "results":array(object(json!({"target":string(),"reachable":boolean(),"reason":nullable(string())})))}),
        ),
        "testbed" => object(
            json!({"name":string(),"spec":string(),"held":boolean(),"count":count(),
            "results":array(object(json!({"held":boolean(),"detail":string()})))}),
        ),
        "discover" => json!({"anyOf":[
            object(json!({"networks":strings(),"count":count(),"complete":boolean(),"authenticated":{"const":false}})),
            object(json!({"listeners":strings(),"networks":strings(),"skipped":strings(),"count":count(),
                "scanned":count(),"port":{"type":"integer","minimum":1,"maximum":65535},
                "complete":boolean(),"truncated":boolean(),"authenticated":{"const":false}}))]}),
        _ => unreachable!("tool definitions and result schemas are updated together"),
    };
    crate::result::output_schema(data)
}
