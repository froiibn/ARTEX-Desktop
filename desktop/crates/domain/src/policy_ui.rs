use crate::*;
use chrono::{Duration, Utc};
use std::collections::BTreeMap;
pub const QUERIES: &[&str] = &[
    "ui.interceptRules",
    "ui.interceptPending",
    "ui.interceptGetOne",
    "ui.interceptExecution",
    "ui.interceptDetail",
    "ui.interceptHistory",
    "ui.interceptHistoryPage",
    "ui.interceptTask",
    "ui.interceptTaskPage",
    "ui.interceptGetToolConfig",
    "ui.interceptGetJudgeConfig",
    "ui.interceptJudgeUsage",
];
pub const COMMANDS: &[&str] = &[
    "ui.createInterceptRule",
    "ui.updateInterceptRule",
    "ui.deleteInterceptRule",
    "ui.toggleInterceptRule",
    "ui.interceptDecide",
    "ui.interceptSetToolConfig",
    "ui.interceptSetJudgeConfig",
];
fn source(d: &Value) -> &str {
    if !text(d, "decision_source").is_empty() {
        text(d, "decision_source")
    } else if !d["rule_id"].is_null() {
        "rule"
    } else if text(d, "reason").starts_with("[model]") {
        "model"
    } else {
        "unknown"
    }
}
pub fn pending(d: &Value, s: &Snapshot, detail: bool) -> Value {
    let mut out = json!({"id":d["id"],"agent_name":d["agent_name"],"tool_name":d["tool_name"],"tool_input":d["tool_input"],"status":d["status"],"reason":d["reason"],"created_at":d["created_at"],"decision_source":source(d)});
    for key in ["rule_id", "conversation_id", "task_id", "decided_at"] {
        if !d[key].is_null() {
            out[key] = d[key].clone();
        }
    }
    if detail {
        let conv = d["conversation_id"]
            .as_str()
            .and_then(|id| s.get("conversations", id));
        out["conv_title"] = json!(conv.map(|r| text(&r.data, "title")).unwrap_or(""));
        out["conv_agent_key"] = json!(conv.map(|r| text(&r.data, "agent_key")).unwrap_or(""));
        out["rule_name"] = json!(
            d["rule_id"]
                .as_str()
                .and_then(|id| s.get("intercept_rules", id))
                .map(|r| text(&r.data, "name"))
                .unwrap_or("")
        );
    }
    out
}
fn audit(d: &Value) -> Value {
    if d.is_null() {
        return Value::Null;
    }
    let mut out = json!({});
    for key in [
        "run_id",
        "tool_use_id",
        "correlation",
        "input_digest",
        "user_message",
        "user_truncated",
        "context",
        "context_truncated",
        "captured_at",
        "model_fallback",
        "model_input",
        "model_input_digest",
        "initial_action",
        "initial_reason",
        "effective_action",
        "decision_reason",
        "rule_name",
        "config_digest",
        "profile_id",
        "execution_status",
        "output",
        "output_truncated",
        "execution_ended_at",
    ] {
        if !d[key].is_null()
            && (d[key] != "" || ["input_digest", "user_message", "initial_reason"].contains(&key))
        {
            out[key] = d[key].clone();
        }
    }
    if d["context"].is_null() {
        out["context"] = Value::Null;
    }
    if let Some(model) = out.get_mut("model_input").and_then(Value::as_object_mut) {
        model.remove("captured_config");
    }
    out
}
fn ordered<'a>(mut rows: Vec<&'a Record>) -> Vec<&'a Record> {
    rows.sort_by(|a, b| {
        b.header
            .created_at
            .cmp(&a.header.created_at)
            .then_with(|| b.header.id.cmp(&a.header.id))
    });
    rows
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m {
        "ui.interceptRules" => {
            let mut rows = s.records("intercept_rules");
            rows.sort_by(|a, b| {
                b.data["priority"]
                    .as_i64()
                    .cmp(&a.data["priority"].as_i64())
                    .then_with(|| a.header.created_at.cmp(&b.header.created_at))
                    .then_with(|| a.header.id.cmp(&b.header.id))
            });
            Ok(json!(
                rows.iter().map(|r| r.data.clone()).collect::<Vec<_>>()
            ))
        }
        "ui.interceptGetToolConfig" => Ok(json!({"enabled_tools":policy::enabled_tools(s)})),
        "ui.interceptGetJudgeConfig" => Ok(policy::judge_config(s)),
        "ui.interceptGetOne" | "ui.interceptDetail" => {
            let r = record(s, "intercept_pending", required(p, "id")?)?;
            let mut out = pending(&r.data, s, m == "ui.interceptDetail");
            if m == "ui.interceptDetail" {
                out["audit"] = audit(&r.data["audit"]);
            }
            Ok(out)
        }
        "ui.interceptPending"
        | "ui.interceptHistory"
        | "ui.interceptHistoryPage"
        | "ui.interceptTask"
        | "ui.interceptTaskPage" => {
            let f = &p["filter"];
            let rows = ordered(
                s.records("intercept_pending")
                    .into_iter()
                    .filter(|r| {
                        (m != "ui.interceptPending" || r.data["status"] == "pending")
                            && p["taskId"]
                                .as_str()
                                .is_none_or(|id| r.data["task_id"] == id)
                            && (text(f, "status").is_empty() || r.data["status"] == f["status"])
                            && (text(f, "decision_source").is_empty()
                                || source(&r.data) == text(f, "decision_source"))
                    })
                    .collect(),
            );
            let total = rows.len();
            let page = p["page"].as_u64().unwrap_or(1).max(1);
            let size = p["size"].as_u64().filter(|n| *n > 0).unwrap_or(20).min(100);
            let rows = if m.ends_with("Page") {
                rows.into_iter()
                    .skip((page.saturating_sub(1) * size) as usize)
                    .take(size as usize)
                    .collect()
            } else if m == "ui.interceptHistory" {
                rows.into_iter().take(100).collect()
            } else {
                rows
            };
            let items = rows
                .iter()
                .map(|r| pending(&r.data, s, m != "ui.interceptPending"))
                .collect::<Vec<_>>();
            Ok(if m.ends_with("Page") {
                json!({"items":items,"total":total})
            } else {
                json!(items)
            })
        }
        "ui.interceptExecution" => {
            let approval = record(s, "intercept_pending", required(p, "id")?)?;
            let d = &approval.data;
            let a = &d["audit"];
            let toolid = text(a, "tool_use_id");
            if a["correlation"] != "exact" || toolid.is_empty() {
                return Err(Error::new(
                    "NOT_FOUND",
                    "Exact tool invocation is unavailable",
                ));
            }
            let conv = d["conversation_id"].as_str();
            let task = d["task_id"].as_str();
            let exp = task
                .and_then(|id| s.get("tasks", id))
                .filter(|r| r.data["deleted_at"].is_null() && r.data["archived_at"].is_null())
                .map(|r| text(&r.data, "exploration_id"));
            if conv.is_none() && exp.is_none() {
                return Err(Error::new("NOT_FOUND", "Task or conversation was deleted"));
            }
            if p["conversationId"]
                .as_str()
                .is_some_and(|id| Some(id) != conv)
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Execution belongs to another conversation",
                ));
            }
            let collection = if conv.is_some() {
                "conversation_activities"
            } else {
                "activity"
            };
            let mut rows = s
                .records(collection)
                .into_iter()
                .filter(|r| {
                    r.data["tool_use_id"] == toolid
                        && matches!(text(&r.data, "kind"), "tool_use" | "tool_result")
                        && if let Some(conv) = conv {
                            r.data["conversation_id"] == conv
                        } else {
                            r.data["exploration_id"] == exp.unwrap()
                        }
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                a.header.created_at.cmp(&b.header.created_at).then_with(|| {
                    a.header
                        .commit_seq
                        .parse::<u64>()
                        .unwrap_or(0)
                        .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0))
                })
            });
            if rows.is_empty()
                || rows.len() > 2
                || rows[0].data["kind"] != "tool_use"
                || rows[0].data["tool"] != d["tool_name"]
            {
                return Err(Error::new(
                    "NOT_FOUND",
                    "Unique call/result pair is unavailable",
                ));
            }
            if rows.len() == 2 {
                let first = &rows[0].data;
                let result = &rows[1].data;
                if result["kind"] != "tool_result"
                    || first["worker"] != result["worker"]
                    || first["node_id"] != result["node_id"]
                    || first["main_seg"] != result["main_seg"]
                    || !text(result, "tool").is_empty() && first["tool"] != result["tool"]
                {
                    return Err(Error::new("NOT_FOUND", "Ambiguous execution pair"));
                }
            }
            let call = rows[0];
            let session = if conv.is_some() {
                String::new()
            } else if call.data["worker"] == "mainagent" {
                format!("main:{}", call.data["main_seg"].as_i64().unwrap_or(0))
            } else if call.data["worker"] == "planner" {
                "plan".into()
            } else if let Some(intent) = call.data["node_id"].as_str() {
                format!("intent:{intent}")
            } else {
                return Err(Error::new("NOT_FOUND", "Execution session is unavailable"));
            };
            let items = rows
                .iter()
                .map(|r| {
                    let mut out = exploration_ui::activity(r, s, task.unwrap_or(""), false);
                    if conv.is_some() {
                        out.as_object_mut().unwrap().remove("intent_id");
                    }
                    out
                })
                .collect::<Vec<_>>();
            Ok(
                json!({"conversation_id":conv,"task_id":task,"session":session,"seq":call.header.id,"items":items}),
            )
        }
        "ui.interceptJudgeUsage" => {
            let mut totals = json!({"calls":0,"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"daily":[]});
            let mut daily = BTreeMap::<String, Value>::new();
            let days = p["days"].as_i64().filter(|d| *d > 0).unwrap_or(30);
            let since = (Utc::now() - Duration::days(days))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            for r in s
                .records("llm_usage")
                .into_iter()
                .filter(|r| r.data["worker"] == "judge")
            {
                totals["calls"] = json!(totals["calls"].as_u64().unwrap() + 1);
                for (a, b) in [
                    ("input_tokens", "input_tokens"),
                    ("output_tokens", "output_tokens"),
                    ("cache_read", "cache_read_tokens"),
                    ("cache_write", "cache_write_tokens"),
                ] {
                    totals[b] =
                        json!(totals[b].as_i64().unwrap_or(0) + r.data[a].as_i64().unwrap_or(0));
                }
                let ts = text(&r.data, "ts");
                if ts >= since.as_str() {
                    let day = ts.get(..10).unwrap_or("").to_owned();
                    let d = daily.entry(day.clone()).or_insert_with(
                        || json!({"date":day,"calls":0,"input_tokens":0,"output_tokens":0}),
                    );
                    for key in ["calls", "input_tokens", "output_tokens"] {
                        d[key] = json!(
                            d[key].as_i64().unwrap_or(0)
                                + if key == "calls" {
                                    1
                                } else {
                                    r.data[key].as_i64().unwrap_or(0)
                                }
                        );
                    }
                }
            }
            totals["daily"] = json!(daily.into_values().collect::<Vec<_>>());
            Ok(totals)
        }
        _ => {
            let _ = c;
            Err(Error::new("METHOD_NOT_FOUND", m))
        }
    }
}
fn put_setting(b: &mut Planner, key: &str, value: &str) -> Result<()> {
    if let Some(r) = b
        .state
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == key)
    {
        let mut d = r.data.clone();
        d["value"] = json!(value);
        b.put("settings", &r.header.id, d);
    } else {
        b.insert("settings", "setting", json!({"key":key,"value":value}))?;
    }
    Ok(())
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.createInterceptRule" | "ui.updateInterceptRule" => {
            let rule = &p["rule"];
            policy::validate_rule(rule)?;
            let mut d = default_data("intercept_rules")?;
            for key in [
                "name",
                "enabled",
                "priority",
                "match_target",
                "match_type",
                "pattern",
                "action",
                "message",
                "timeout_enabled",
                "timeout_seconds",
                "timeout_action",
            ] {
                if let Some(value) = rule.get(key) {
                    d[key] = value.clone();
                }
            }
            let id = if m == "ui.createInterceptRule" {
                b.insert("intercept_rules", "interceptrule", d)?
            } else {
                let id = required(p, "id")?;
                let old = record(s, "intercept_rules", id)?;
                d["id"] = json!(id);
                d["created_at"] = old.data["created_at"].clone();
                b.put("intercept_rules", id, d);
                id.into()
            };
            let projected = projected(s, &b.batch)?;
            record(&projected, "intercept_rules", &id)?.data.clone()
        }
        "ui.deleteInterceptRule" => {
            let id = required(p, "id")?;
            record(s, "intercept_rules", id)?;
            b.delete("intercept_rules", id);
            json!({"deleted":id})
        }
        "ui.toggleInterceptRule" => {
            let id = required(p, "id")?;
            let mut d = record(s, "intercept_rules", id)?.data.clone();
            let enabled = p["enabled"]
                .as_bool()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Enabled must be boolean"))?;
            d["enabled"] = json!(enabled);
            b.put("intercept_rules", id, d);
            json!({"ok":true,"enabled":enabled})
        }
        "ui.interceptDecide" => {
            if c.actor.kind != "user" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Only a user may approve execution",
                ));
            }
            let decision = required(p, "decision")?;
            if !["allowed", "denied"].contains(&decision) {
                return Err(Error::new("INVALID_PARAMS", "Invalid approval decision"));
            }
            let decided = Utc::now();
            let id = required(p, "id")?;
            if decision == "allowed" {
                let approval = record(s, "intercept_pending", id)?;
                let audit = &approval.data["audit"];
                if let Some(rid) = audit["run_id"].as_str() {
                    if s.get("agent_runs", rid).is_none_or(|run| {
                        run.data["engine_epoch"] != c.actor.engine_epoch
                            || !matches!(run.data["state"].as_str(), Some("running" | "waiting"))
                    }) {
                        return Err(Error::new(
                            "STALE_EPOCH",
                            "Approval belongs to an inactive run",
                        ));
                    }
                }
            }
            policy::settle(
                &mut b,
                id,
                decision,
                if decision == "allowed" {
                    "allow"
                } else {
                    "deny"
                },
                if decision == "allowed" {
                    "Manually allowed execution"
                } else {
                    "Manual refusal to execute"
                },
            )?;
            crate::approval_bypass::mark_manual_decision(
                &mut b,
                id,
                c,
                decision == "allowed",
                &decided.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            )?;
            if let Some(rid) = record(s, "intercept_pending", id)?.data["audit"]["run_id"].as_str()
            {
                crate::approval_bypass::resume(&mut b, rid, &c.actor.engine_epoch);
            }
            json!({"ok":true})
        }
        "ui.interceptSetToolConfig" => {
            let names = p["enabledTools"]
                .as_array()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Tool names must be an array"))?;
            if names.iter().any(|v| v.as_str().is_none()) {
                return Err(Error::new("INVALID_PARAMS", "Tool names must be strings"));
            }
            put_setting(
                &mut b,
                "intercept_enabled_tools",
                &names
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .collect::<Value>()
                    .to_string(),
            )?;
            Value::Null
        }
        "ui.interceptSetJudgeConfig" => {
            let cfg = &p["cfg"];
            for key in ["fail_action", "ask_timeout_action"] {
                if !if key == "fail_action" {
                    ["allow", "deny", "ask"].contains(&text(cfg, key))
                } else {
                    ["allow", "deny"].contains(&text(cfg, key))
                } {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Invalid judge fallback action",
                    ));
                }
            }
            if let Some(id) = cfg["profile_id"].as_str() {
                record(s, "llm_profiles", id)?;
            }
            for (key, setting) in [
                ("enabled", "llm_judge_enabled"),
                ("profile_id", "llm_judge_profile_id"),
                ("prompt", "llm_judge_prompt"),
                ("timeout_seconds", "llm_judge_timeout_seconds"),
                ("fail_action", "llm_judge_fail_action"),
                ("ask_timeout_seconds", "llm_judge_ask_timeout_seconds"),
                ("ask_timeout_action", "llm_judge_ask_timeout_action"),
            ] {
                let v = &cfg[key];
                let text = if v.is_null() {
                    String::new()
                } else if let Some(v) = v.as_str() {
                    v.into()
                } else {
                    v.to_string()
                };
                put_setting(&mut b, setting, &text)?;
            }
            json!({"ok":true})
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if let Some(Change::Put { collection, id, .. } | Change::Delete { collection, id }) =
        b.batch.changes.first().cloned()
    {
        conversation::push_event(&mut b.batch, s, c, "InterceptChanged", &collection, &id)?;
    }
    let mut batch = b.done(result);
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
