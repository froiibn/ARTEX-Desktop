//! Display the source text view while retaining the structured transcript in storage.
use crate::*;
pub fn body(value: &Value, read: &dyn Fn(&Value) -> Result<Vec<u8>>) -> Result<Value> {
    render(value, read, true)
}
pub fn public_body(value: &Value, read: &dyn Fn(&Value) -> Result<Vec<u8>>) -> Result<Value> {
    render(value, read, false)
}
fn render(
    value: &Value,
    read: &dyn Fn(&Value) -> Result<Vec<u8>>,
    include_thinking: bool,
) -> Result<Value> {
    let raw = if let Some(t) = value.as_str().or_else(|| value["text"].as_str()) {
        t.to_owned()
    } else if !value["blob"].is_null() {
        String::from_utf8(read(&value["blob"])?)
            .map_err(|_| Error::new("CORRUPT_STORE", "Transcript is not UTF-8"))?
    } else {
        return Ok(artex_core::body(""));
    };
    let Ok(envelope) = serde_json::from_str::<Value>(&raw) else {
        return Ok(value.clone());
    };
    if let Some(content) = envelope["message"]["content"].as_array() {
        let mut pieces = Vec::new();
        for b in content {
            match text(b, "type") {
                "text" => pieces.push(text(b, "text").to_owned()),
                "thinking" if include_thinking => pieces.push(text(b, "thinking").to_owned()),
                "tool_result" => {
                    pieces.push(if let Some(t) = b["content"].as_str() {
                        t.to_owned()
                    } else if let Some(parts) = b["content"].as_array() {
                        parts
                            .iter()
                            .filter_map(|p| p["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        b["content"].to_string()
                    });
                }
                _ => {}
            }
        }
        return Ok(artex_core::body(&pieces.join("\n")));
    }
    if (envelope["run_id"].is_string() || envelope["stream"]["run_id"].is_string())
        && envelope["text"].is_string()
    {
        return Ok(artex_core::body(
            envelope["display_text"]
                .as_str()
                .unwrap_or_else(|| text(&envelope, "text")),
        ));
    }
    Ok(value.clone())
}
pub fn project(
    method: &str,
    mut result: Value,
    read: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<Value> {
    match method {
        "ui.conversationMsgDetail" => body(&result, read),
        "ui.activityDetail" => {
            result["detail"] = body(&result["detail"], read)?;
            Ok(result)
        }
        "ui.interceptExecution" => {
            if let Some(items) = result["items"].as_array_mut() {
                for item in items {
                    if !item["detail"].is_null() {
                        item["detail"] = body(&item["detail"], read)?;
                    }
                }
            }
            Ok(result)
        }
        _ => Ok(result),
    }
}

/// Promote durable stream metadata from activity bodies without exposing the raw transcript envelope.
pub fn project_with_snapshot(
    method: &str,
    mut result: Value,
    s: &Snapshot,
    read: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<Value> {
    if matches!(
        method,
        "ui.conversationMessages" | "ui.conversationHistory" | "ui.activity" | "ui.activityHistory"
    ) {
        if let Some(items) = result["items"].as_array_mut() {
            for item in items {
                let id = text(item, "seq");
                let collection = if method.contains("conversation") {
                    "conversation_activities"
                } else {
                    "activity"
                };
                if let Some(row) = s.get(collection, id) {
                    let raw = crate::traffic_ui::body_bytes(&row.data["detail"], read)?;
                    if let Ok(envelope) = serde_json::from_slice::<Value>(&raw) {
                        if envelope["stream"].is_object() {
                            item["stream"] = json!({"run_id":envelope["stream"]["run_id"],"turn_id":envelope["stream"]["turn_id"],"pending":envelope["stream"]["pending"].as_bool().unwrap_or(envelope["stream"]["partial"]==true)});
                        }
                    }
                    if !item["detail"].is_null() {
                        item["detail"] = body(&row.data["detail"], read)?;
                    }
                }
            }
        }
    }
    project(method, result, read)
}
