//! LLM configuration, task resolution, and the source recording projections.
use crate::*;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
pub const QUERIES: &[&str] = &[
    "ui.getLLM",
    "ui.llmPool",
    "ui.taskLLMResolution",
    "ui.llmRecords",
    "ui.llmRecordDetail",
    "ui.llmTasks",
    "ui.tokensByModel",
    "ui.commands",
];
pub const COMMANDS: &[&str] = &["ui.setLLM", "ui.resetLLMPool", "ui.llmRecordsDeleteTask"];
pub fn configured(r: &Record) -> bool {
    !r.data["api_key"].is_null()
        && !text(&r.data, "model").trim().is_empty()
        && ["openai", "anthropic", "openai-responses"].contains(&text(&r.data, "format"))
}
fn active(s: &Snapshot) -> Option<&Record> {
    s.records("llm_profiles")
        .into_iter()
        .find(|r| r.data["is_default"] == true)
}
pub fn legacy_config(s: &Snapshot) -> Value {
    let default = json!({});
    let d = active(s).map(|p| &p.data).unwrap_or(&default);
    json!({"configured":active(s).is_some_and(configured),"provider":if text(d,"format").is_empty(){"anthropic"}else{text(d,"format")},"model":text(d,"model"),"base_url":text(d,"base_url"),"proxy":text(d,"proxy"),"key_set":!d["api_key"].is_null(),"rate_per_second":d["rate_per_second"].as_f64().unwrap_or(0.0),"rate_per_minute":d["rate_per_minute"].as_f64().unwrap_or(0.0),"context_window_k":d["context_window_k"].as_i64().unwrap_or(0),"thinking_type":text(d,"thinking_type"),"reasoning_effort":text(d,"reasoning_effort")})
}
pub fn pool_status(s: &Snapshot) -> Value {
    let mut rows = s.records("llm_profiles");
    rows.sort_by(|a, b| {
        b.data["is_default"]
            .as_bool()
            .cmp(&a.data["is_default"].as_bool())
            .then_with(|| {
                b.data["priority"]
                    .as_i64()
                    .cmp(&a.data["priority"].as_i64())
            })
            .then_with(|| a.header.created_at.cmp(&b.header.created_at))
            .then_with(|| a.header.id.cmp(&b.header.id))
    });
    let chain=rows.into_iter().map(|p|{let h=s.records("llm_profile_health").into_iter().find(|h|h.data["profile_id"]==p.header.id);let empty=json!({});let h=h.map(|r|&r.data).unwrap_or(&empty);let remain=DateTime::parse_from_rfc3339(text(h,"open_until")).map(|t|(t.with_timezone(&Utc)-Utc::now()).num_milliseconds()).unwrap_or(0);let fails=h["fails"].as_i64().unwrap_or(0);let mut o=json!({"profile_id":p.header.id,"name":text(&p.data,"name"),"model":text(&p.data,"model"),"format":text(&p.data,"format"),"priority":p.data["priority"].as_i64().unwrap_or(0),"active":p.data["is_default"]==true,"excluded":p.data["pool_exclude"]==true,"state":if remain>0{"tripped"}else if fails>0{"degraded"}else{"ok"},"fails":fails,"trips":h["trips"].as_i64().unwrap_or(0),"cooldown_secs":if remain>0{remain/1000+1}else{0}});for k in["last_error","last_at"]{if !text(h,k).is_empty(){o[k]=h[k].clone();}}o}).collect::<Vec<_>>();
    json!({"enabled":setting_bool(s,"llm_pool_enabled",false),"bind_fallback":setting_bool(s,"llm_pool_bind_fallback",false),"chain":chain})
}
fn resolution(p: Option<&Record>, source: &str, missing: &str) -> Value {
    let mut out = json!({"name":"","format":"","model":"","source":source,"available":false});
    if let Some(p) = p {
        out["profile_id"] = json!(p.header.id);
        for k in ["name", "format", "model"] {
            out[k] = json!(text(&p.data, k));
        }
        out["available"] = json!(configured(p));
        if !configured(p) {
            out["reason"] = json!(if p.data["api_key"].is_null() {
                "LLM configuration has no API key"
            } else {
                "LLM configuration has invalid format or parameters"
            });
        }
    } else {
        out["reason"] = json!(missing);
    }
    out
}
fn task_resolution(s: &Snapshot, t: &Record, key: &str) -> Value {
    let binding = conversation::agent_by_key(s, key)
        .ok()
        .and_then(|a| a.data["llm_profile_id"].as_str())
        .and_then(|id| s.get("llm_profiles", id));
    if let Some(p) = binding.filter(|p| configured(p)) {
        return resolution(Some(p), "agent_binding", "");
    }
    if !llm_runtime::chain(s, &t.header.id).is_empty() {
        let p = t.data["active_llm_profile_id"]
            .as_str()
            .and_then(|id| s.get("llm_profiles", id));
        return resolution(
            p,
            "task_chain",
            if t.data["active_llm_profile_id"].is_null() {
                "Task LLM profile chain is quota exhausted"
            } else {
                "LLM configuration does not exist"
            },
        );
    }
    if let Some(p) = active(s).filter(|p| configured(p)) {
        return resolution(Some(p), "global_profile", "");
    }
    resolution(None, "global", "No available LLM configuration")
}
fn recent(mut rows: Vec<&Record>) -> Vec<&Record> {
    rows.sort_by(|a, b| {
        b.header
            .created_at
            .cmp(&a.header.created_at)
            .then_with(|| {
                b.header
                    .commit_seq
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(&a.header.commit_seq.parse::<u64>().unwrap_or(0))
            })
            .then_with(|| b.header.id.cmp(&a.header.id))
    });
    rows
}
fn wire_record(r: &Record, detail: bool) -> Value {
    let mut o = json!({"id":r.header.id});
    for k in [
        "ts",
        "model",
        "profile_name",
        "session_id",
        "task_id",
        "worker",
        "status",
    ] {
        o[k] = json!(text(&r.data, k));
    }
    for k in [
        "latency_ms",
        "input_tokens",
        "output_tokens",
        "cache_read",
        "cache_write",
    ] {
        o[k] = json!(r.data[k].as_i64().unwrap_or(0));
    }
    if !text(&r.data, "error").is_empty() {
        o["error"] = r.data["error"].clone();
    }
    if detail {
        for k in [
            "request_body",
            "response_body",
            "raw_request",
            "raw_response",
        ] {
            o[k] = if r.data[k].is_object() {
                r.data[k].clone()
            } else {
                artex_core::body(text(&r.data, k))
            };
        }
    }
    o
}
fn page(p: &Value) -> (usize, usize) {
    let page = p["page"].as_u64().unwrap_or(0) as usize;
    let size = p["size"]
        .as_u64()
        .filter(|n| *n > 0)
        .unwrap_or(50)
        .min(10000) as usize;
    (page.saturating_mul(size), size)
}
fn body_text(v: &Value, read: &dyn Fn(&Value) -> Result<Vec<u8>>) -> Result<String> {
    if let Some(t) = v.as_str().or_else(|| v["text"].as_str()) {
        return Ok(t.into());
    }
    if !v["blob"].is_null() {
        return String::from_utf8(read(&v["blob"])?)
            .map_err(|_| Error::new("INVALID_PARAMS", "Activity body is not UTF-8"));
    }
    Ok(String::new())
}
pub fn query(
    m: &str,
    p: &Value,
    s: &Snapshot,
    c: &Context,
    read: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<Value> {
    let _ = c;
    match m {
        "ui.getLLM" => Ok(legacy_config(s)),
        "ui.llmPool" => Ok(pool_status(s)),
        "ui.taskLLMResolution" => {
            let t = record(s, "tasks", required(p, "id")?)?;
            let mut o = json!({});
            for role in ["mainagent", "planner", "worker"] {
                o[role] = task_resolution(s, t, role);
            }
            Ok(o)
        }
        "ui.llmRecordDetail" => Ok(wire_record(
            record(s, "llm_records", required(p, "id")?)?,
            true,
        )),
        "ui.llmRecords" => {
            let q = &p["params"];
            let session = text(q, "session").to_lowercase();
            let rows = recent(
                s.records("llm_records")
                    .into_iter()
                    .filter(|r| {
                        (text(q, "model").is_empty() || r.data["model"] == q["model"])
                            && (text(q, "task").is_empty() || r.data["task_id"] == q["task"])
                            && (session.is_empty()
                                || text(&r.data, "session_id")
                                    .to_lowercase()
                                    .contains(&session))
                    })
                    .collect(),
            );
            let total = rows.len();
            let (offset, size) = page(q);
            Ok(
                json!({"records":rows.into_iter().skip(offset).take(size).map(|r|wire_record(r,false)).collect::<Vec<_>>(),"total":total}),
            )
        }
        "ui.llmTasks" => {
            let mut counts = BTreeMap::<String, (usize, u64)>::new();
            for r in s.records("llm_records") {
                let task = text(&r.data, "task_id");
                if !task.is_empty() {
                    let e = counts.entry(task.into()).or_default();
                    e.0 += 1;
                    e.1 = e.1.max(r.header.commit_seq.parse().unwrap_or(0));
                }
            }
            let mut tasks = counts.into_iter().collect::<Vec<_>>();
            tasks.sort_by(|a, b| b.1.1.cmp(&a.1.1).then_with(|| a.0.cmp(&b.0)));
            Ok(
                json!({"tasks":tasks.into_iter().map(|(id,(count,_))|json!({"task_id":id,"count":count})).collect::<Vec<_>>()}),
            )
        }
        "ui.tokensByModel" => {
            let task = required(p, "task")?;
            let mut models = BTreeMap::<String, Value>::new();
            for r in s
                .records("llm_usage")
                .into_iter()
                .filter(|r| r.data["task_id"] == task)
            {
                let model = text(&r.data, "model").to_owned();
                let e=models.entry(model.clone()).or_insert_with(||json!({"model":model,"calls":0,"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0}));
                e["calls"] = json!(e["calls"].as_i64().unwrap() + 1);
                for (a, b) in [
                    ("input_tokens", "input_tokens"),
                    ("output_tokens", "output_tokens"),
                    ("cache_read", "cache_read_tokens"),
                    ("cache_write", "cache_write_tokens"),
                ] {
                    e[b] = json!(e[b].as_i64().unwrap_or(0) + r.data[a].as_i64().unwrap_or(0));
                }
            }
            let mut rows = models.into_values().collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                b["input_tokens"]
                    .as_i64()
                    .unwrap_or(0)
                    .saturating_add(b["output_tokens"].as_i64().unwrap_or(0))
                    .cmp(
                        &a["input_tokens"]
                            .as_i64()
                            .unwrap_or(0)
                            .saturating_add(a["output_tokens"].as_i64().unwrap_or(0)),
                    )
                    .then_with(|| text(a, "model").cmp(text(b, "model")))
            });
            Ok(json!({"models":rows}))
        }
        "ui.commands" => {
            let q = &p["params"];
            let task = text(q, "task");
            let exp = if task.starts_with("task_") {
                s.get("tasks", task)
                    .map(|t| text(&t.data, "exploration_id"))
                    .unwrap_or(task)
            } else {
                task
            };
            let needle = text(q, "q").to_lowercase();
            let mut rows = Vec::new();
            for call in recent(
                s.records("activity")
                    .into_iter()
                    .filter(|r| {
                        r.data["kind"] == "tool_use"
                            && (exp.is_empty() || r.data["exploration_id"] == exp)
                    })
                    .collect(),
            ) {
                let command = body_text(&call.data["detail"], read)?;
                if !needle.is_empty()
                    && !text(&call.data, "tool").to_lowercase().contains(&needle)
                    && !command.to_lowercase().contains(&needle)
                {
                    continue;
                }
                let results = s
                    .records("activity")
                    .into_iter()
                    .filter(|r| {
                        r.data["kind"] == "tool_result"
                            && r.data["tool_use_id"] == call.data["tool_use_id"]
                    })
                    .collect::<Vec<_>>();
                let results: Vec<Option<&Record>> = if results.is_empty() {
                    vec![None]
                } else {
                    results.into_iter().map(Some).collect()
                };
                for result in results {
                    rows.push(json!({"id":call.header.id,"exploration_id":call.data["exploration_id"],"worker":text(&call.data,"worker"),"tool":text(&call.data,"tool"),"command":artex_core::body(&command),"output":result.map(|r|transcript_ui::body(&r.data["detail"],read)).transpose()?.filter(|v|v.is_object()).unwrap_or_else(||artex_core::body("")),"is_error":result.is_some_and(|r|r.data["is_error"]==true),"created_at":call.data["created_at"]}));
                }
            }
            let total = s
                .records("activity")
                .into_iter()
                .filter(|r| {
                    r.data["kind"] == "tool_use"
                        && (exp.is_empty() || r.data["exploration_id"] == exp)
                })
                .filter(|r| {
                    needle.is_empty()
                        || text(&r.data, "tool").to_lowercase().contains(&needle)
                        || body_text(&r.data["detail"], read)
                            .is_ok_and(|v| v.to_lowercase().contains(&needle))
                })
                .count();
            let (offset, size) = page(q);
            Ok(
                json!({"commands":rows.into_iter().skip(offset).take(size).collect::<Vec<_>>(),"total":total}),
            )
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.llmRecordsDeleteTask" => {
            let task = required(p, "task")?.trim();
            let mut n = 0;
            for r in s.records("llm_records") {
                if r.data["task_id"] == task {
                    b.delete("llm_records", &r.header.id);
                    n += 1;
                }
            }
            json!({"deleted":n})
        }
        "ui.resetLLMPool" => {
            if let Some(id) = p["id"].as_str() {
                record(s, "llm_profiles", id)?;
            }
            for h in s.records("llm_profile_health") {
                if p["id"].as_str().is_none_or(|id| h.data["profile_id"] == id) {
                    b.delete("llm_profile_health", &h.header.id);
                }
            }
            pool_status(&projected(s, &b.batch)?)
        }
        "ui.setLLM" => {
            let model = required(p, "model")?;
            let provider = required(p, "provider")?;
            if !["openai", "anthropic", "openai-responses"].contains(&provider) {
                return Err(Error::new("INVALID_PARAMS", "Unknown LLM format"));
            }
            let old = s
                .records("llm_profiles")
                .into_iter()
                .find(|r| r.data["name"] == "default");
            let mut d = old
                .map(|r| r.data.clone())
                .unwrap_or(default_data("llm_profiles")?);
            d["name"] = json!("default");
            d["model"] = json!(model);
            d["format"] = json!(provider);
            for k in ["base_url", "proxy"] {
                d[k] = json!(text(p, k));
            }
            for k in ["rate_per_second", "rate_per_minute"] {
                d[k] = json!(p[k].as_f64().unwrap_or(0.0));
            }
            d["context_window_k"] = json!(
                p["context_window_k"]
                    .as_i64()
                    .filter(|n| *n > 0)
                    .unwrap_or(200)
                    .min(1000)
            );
            d["thinking_type"] = json!("");
            d["reasoning_effort"] = json!("");
            if p["api_key"].is_object() {
                d["api_key"] = p["api_key"].clone();
            } else if p["api_key"].as_str().is_some_and(|v| !v.is_empty()) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Host must normalize API keys into SecretRef",
                ));
            } else if d["api_key"].is_null() {
                d["api_key"] = active(s)
                    .map(|r| r.data["api_key"].clone())
                    .unwrap_or(Value::Null);
            }
            if d["api_key"].is_null() {
                return Err(Error::new("INVALID_PARAMS", "API key is required"));
            }
            if old.is_none() {
                d["streaming"] = json!(true);
            }
            d["is_default"] = json!(true);
            let id = if let Some(old) = old {
                b.put("llm_profiles", &old.header.id, d);
                old.header.id.clone()
            } else {
                b.insert("llm_profiles", "profile", d)?
            };
            for r in s.records("llm_profiles") {
                if r.header.id != id && r.data["is_default"] == true {
                    let mut d = r.data.clone();
                    d["is_default"] = json!(false);
                    b.put("llm_profiles", &r.header.id, d);
                }
            }
            legacy_config(&projected(s, &b.batch)?)
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if !b.batch.changes.is_empty() {
        let first = b.batch.changes[0].clone();
        let (Change::Put { collection, id, .. } | Change::Delete { collection, id }) = first;
        {
            conversation::push_event(&mut b.batch, s, c, "LLMChanged", &collection, &id)?;
        }
    }
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}
