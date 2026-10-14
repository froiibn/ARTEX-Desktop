//! Source-specific archive queues, notification configuration and statistics.
use crate::*;
use artex_core::collection_prefix;
use chrono::{Duration, Utc};
use std::collections::{BTreeMap, BTreeSet};
pub const QUERIES: &[&str] = &[
    "ui.taskArchives",
    "ui.taskArchive",
    "ui.stats",
    "ui.usageStats",
    "ui.findingStats",
    "ui.tokenStats",
    "ui.tokenDaily",
    "ui.commandStats",
    "ui.conversationTokens",
    "ui.notifyMeta",
    "ui.notifyChannels",
    "ui.notifyDeliveries",
];
pub const COMMANDS: &[&str] = &[
    "ui.archiveTask",
    "ui.archiveTasks",
    "ui.restoreTaskArchive",
    "ui.restoreTaskArchives",
    "ui.deleteTaskArchive",
    "ui.deleteTaskArchives",
    "ui.notifyCreateChannel",
    "ui.notifyUpdateChannel",
    "ui.notifyDeleteChannel",
    "ui.notifyRetryDelivery",
    "ui.notifyTestChannel",
    "ui.gc",
];
pub fn secret_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "email" => &["password"],
        "telegram" => &["bot_token"],
        "dingtalk" | "feishu" => &["webhook", "secret"],
        "wecom" => &["webhook"],
        "webhook" => &["url", "headers"],
        _ => &[],
    }
}
/// Append finding notification events in the same durable transaction as the
/// finding change. The immutable event snapshot survives finding deletion.
pub fn append_finding_events(s: &Snapshot, b: &mut CommandBatch) -> Result<()> {
    let changes = b.changes.clone();
    for change in changes {
        let artex_core::Change::Put {
            collection,
            id,
            data,
        } = change
        else {
            continue;
        };
        if collection != "findings" {
            continue;
        }
        let prior = s.get("findings", &id);
        let kind = if prior.is_none() {
            "finding_created"
        } else if prior.is_some_and(|r| r.data["status"] != data["status"]) {
            "finding_status_changed"
        } else {
            continue;
        };
        let event_id = new_id(collection_prefix("notification_events")?);
        let mut event = default_data("notification_events")?;
        let task = data["task_id"].as_str().and_then(|t| s.get("tasks", t));
        let assets = ids(&data, "asset_ids")
            .into_iter()
            .filter_map(|a| s.get("assets", &a).map(|r| r.data["host"].clone()))
            .collect::<Vec<_>>();
        let snapshot = json!({"kind":kind,"finding_id":id,"task_id":data["task_id"],"task_name":task.map(|t|t.data["name"].clone()).unwrap_or(Value::Null),"name":data["name"],"vulnclass":data["vulnclass"],"severity":data["severity"],"summary":data["summary"],"asset_ids":data["asset_ids"],"assets":assets,"from_status":prior.map(|r|r.data["status"].clone()).unwrap_or(Value::Null),"to_status":data["status"],"created_at":artex_core::now()});
        event["id"] = json!(event_id);
        event["kind"] = json!(kind);
        event["finding_id"] = json!(id);
        event["snapshot"] = snapshot;
        event["fanned_out"] = json!(false);
        b.changes.push(artex_core::Change::Put {
            collection: "notification_events".into(),
            id: event_id,
            data: event,
        });
    }
    Ok(())
}
fn kind_rate(kind: &str) -> Option<i64> {
    match kind {
        "email" => Some(60),
        "feishu" => Some(100),
        "telegram" | "dingtalk" | "wecom" => Some(20),
        "webhook" => Some(0),
        _ => None,
    }
}
fn setting<'a>(s: &'a Snapshot, key: &str, default: &'a str) -> &'a str {
    s.records("settings")
        .into_iter()
        .find(|r| r.data["key"] == key)
        .and_then(|r| r.data["value"].as_str())
        .unwrap_or(default)
}
fn count(v: u64) -> Result<Value> {
    if v > 9007199254740991 {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Legacy statistics number exceeds exact UI boundary",
        ));
    }
    Ok(json!(v))
}
fn page(input: &Value, size_name: &str, default_size: u64) -> Result<(u64, u64)> {
    let page = input["page"].as_u64().unwrap_or(0);
    let size = input[size_name].as_u64().unwrap_or(default_size);
    if size == 0 || size > 1000 || page.checked_mul(size).is_none() {
        return Err(Error::new("INVALID_PARAMS", "Invalid page boundary"));
    }
    Ok((page, size))
}
pub fn archive_wire(d: &Value, s: &Snapshot) -> Result<Value> {
    let mut out = json!({});
    for key in [
        "id",
        "task_id",
        "state",
        "phase",
        "progress",
        "error",
        "warnings",
        "format_version",
        "sha256",
        "original_size",
        "compressed_size",
        "task_name",
        "task_description",
        "task_goal",
        "original_status",
        "source_task_ids",
        "data_counts",
        "aggregate_stats",
        "requested_at",
        "created_at",
        "updated_at",
    ] {
        out[key] = d[key].clone();
    }
    out["remaining_timeout_seconds"] = count(
        text(d, "remaining_timeout_seconds")
            .parse()
            .map_err(|_| Error::new("CORRUPT_STORE", "Archive remaining time invalid"))?,
    )?;
    if !d["archived_at"].is_null() {
        out["archived_at"] = d["archived_at"].clone();
    }
    if !text(d, "category_name_snapshot").is_empty() {
        out["category_name"] = d["category_name_snapshot"].clone();
    }
    if let Some(category) = s.records("task_categories").into_iter().find(|r| {
        r.header.legacy_key == d["category_id_snapshot"] && !d["category_id_snapshot"].is_null()
    }) {
        out["category_id"] = json!(category.header.id);
    }
    Ok(out)
}
fn channel_wire(d: &Value) -> Value {
    let mut out = d.clone();
    let keys = secret_keys(text(d, "kind"));
    out["secret_keys"] = json!(keys);
    for key in keys {
        if !out["config"][*key].is_null() && out["config"][*key].as_str() != Some("") {
            out["config"][*key] = json!("__masked__");
        }
    }
    out
}
fn delivery_wire(r: &Record, s: &Snapshot) -> Result<Value> {
    let d = &r.data;
    let channel = record(s, "notification_channels", required(d, "channel_id")?)?;
    let event = record(s, "notification_events", required(d, "event_id")?)?;
    let snap = &event.data["snapshot"];
    let mut out = json!({"id":r.header.id,"finding_id":snap["finding_id"].as_str().unwrap_or(text(&event.data,"finding_id")),"event_kind":event.data["kind"],"channel_id":channel.header.id,"channel_name":channel.data["name"],"channel_kind":channel.data["kind"],"state":d["state"],"attempts":d["attempts"],"last_error":d["last_error"],"created_at":d["created_at"],"next_attempt_at":d["next_attempt_at"],"title":snap["name"].as_str().filter(|s|!s.is_empty()).unwrap_or(text(snap,"vulnclass")),"severity":snap["severity"].as_str().unwrap_or("")});
    if !d["batch_id"].is_null() {
        out["batch_id"] = d["batch_id"].clone();
    }
    if !d["sent_at"].is_null() {
        out["sent_at"] = d["sent_at"].clone();
    }
    Ok(out)
}
fn active_task<'a>(p: &Value, s: &'a Snapshot) -> Result<Option<&'a Record>> {
    let requested = text(p, "task");
    if !requested.is_empty() && requested != "active" {
        return Ok(Some(record(s, "tasks", requested)?));
    }
    Ok(s.records("tasks")
        .into_iter()
        .filter(|r| {
            r.data["archived_at"].is_null()
                && r.data["deleted_at"].is_null()
                && r.data["state"] == "running"
        })
        .max_by_key(|r| r.header.updated_at.clone()))
}
#[derive(Default, Clone)]
struct Tokens([u64; 4]);
impl Tokens {
    fn add(&mut self, data: &Value, ledger: bool) -> Result<()> {
        for (i, key) in if ledger {
            ["input_tokens", "output_tokens", "cache_read", "cache_write"]
        } else {
            [
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
            ]
        }
        .iter()
        .enumerate()
        {
            self.0[i] = self.0[i]
                .checked_add(data[*key].as_u64().unwrap_or(0))
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Token aggregate overflow"))?;
        }
        Ok(())
    }
    fn wire(&self) -> Result<Value> {
        Ok(
            json!({"input_tokens":count(self.0[0])?,"output_tokens":count(self.0[1])?,"cache_read_tokens":count(self.0[2])?,"cache_write_tokens":count(self.0[3])?}),
        )
    }
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _c: &Context) -> Result<Value> {
    match m {
        "ui.taskArchive" => archive_wire(&record(s, "task_archives", required(p, "id")?)?.data, s),
        "ui.taskArchives" => {
            let input = &p["input"];
            let (page, size) = page(input, "size", 30)?;
            let q = text(input, "q").to_lowercase();
            let state = text(input, "state");
            let mut rows = s
                .records("task_archives")
                .into_iter()
                .filter(|r| {
                    (state.is_empty() || r.data["state"] == state)
                        && (q.is_empty()
                            || ["task_name", "task_description", "task_goal"]
                                .iter()
                                .any(|k| text(&r.data, k).to_lowercase().contains(&q)))
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                b.header
                    .created_at
                    .cmp(&a.header.created_at)
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            let total = rows.len();
            let items = rows
                .into_iter()
                .skip((page * size) as usize)
                .take(size as usize)
                .map(|r| archive_wire(&r.data, s))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"items":items,"total":total,"page":page,"size":size}))
        }
        "ui.conversationTokens" => {
            let mut out = Vec::new();
            for conversation in s.records("conversations") {
                let mut tokens = Tokens::default();
                for activity in s
                    .records("conversation_activities")
                    .into_iter()
                    .filter(|a| {
                        a.data["conversation_id"] == conversation.header.id
                            && a.data["kind"] == "result"
                    })
                {
                    tokens.add(&activity.data, false)?;
                }
                let mut row = tokens.wire()?;
                row["llm_profile_id"] = conversation.data["llm_profile_id"].clone();
                row["created_at"] = conversation.data["created_at"].clone();
                out.push(row);
            }
            Ok(json!(out))
        }
        "ui.notifyChannels" => Ok(json!(
            s.records("notification_channels")
                .into_iter()
                .map(|r| channel_wire(&r.data))
                .collect::<Vec<_>>()
        )),
        "ui.notifyMeta" => {
            let channels = s.records("notification_channels");
            let deliveries = s.records("notification_deliveries");
            let today = Utc::now().format("%Y-%m-%d").to_string();
            let pending = deliveries
                .iter()
                .filter(|r| matches!(text(&r.data, "state"), "pending" | "sending"))
                .collect::<Vec<_>>();
            let oldest = pending
                .iter()
                .filter_map(|r| {
                    chrono::DateTime::parse_from_rfc3339(text(&r.data, "created_at")).ok()
                })
                .min();
            let kinds=["dingtalk","email","feishu","telegram","webhook","wecom"].map(|k|json!({"kind":k,"default_rate_per_min":kind_rate(k).unwrap(),"secret_keys":secret_keys(k)}));
            Ok(
                json!({"kinds":kinds,"enabled":setting(s,"notify_enabled","true")=="true","public_base_url":setting(s,"notify_public_base_url",""),"digest_interval_min":setting(s,"notify_digest_interval_min",""),"defaults":{"digest_interval_min":30},"stats":{"channels":channels.len(),"channels_on":channels.iter().filter(|r|r.data["enabled"]==true).count(),"pending":pending.len(),"failed":deliveries.iter().filter(|r|r.data["state"]=="failed").count(),"sent_today":deliveries.iter().filter(|r|r.data["state"]=="sent"&&text(&r.data,"sent_at").starts_with(&today)).count(),"backlog_age_ms":oldest.map(|t|Utc::now().signed_duration_since(t).num_milliseconds().max(0)).unwrap_or(0)}}),
            )
        }
        "ui.notifyDeliveries" => {
            let q = &p["q"];
            let (page, size) = page(q, "pageSize", 30)?;
            let rows = s
                .records("notification_deliveries")
                .into_iter()
                .filter(|r| {
                    (q["channelId"].is_null() || r.data["channel_id"] == q["channelId"])
                        && (text(q, "state").is_empty() || r.data["state"] == q["state"])
                })
                .collect::<Vec<_>>();
            let total = rows.len();
            let mut rows = rows;
            rows.sort_by(|a, b| {
                b.header
                    .created_at
                    .cmp(&a.header.created_at)
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            let deliveries = rows
                .into_iter()
                .skip((page * size) as usize)
                .take(size as usize)
                .map(|r| delivery_wire(r, s))
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"deliveries":deliveries,"total":total,"page":page,"page_size":size}))
        }
        "ui.findingStats" => {
            let rows = s.records("findings");
            let mut result = json!({"total":rows.len(),"pending":rows.iter().filter(|r|r.data["status"]=="pending").count(),"vulnclasses":rows.iter().map(|r|text(&r.data,"vulnclass").to_owned()).collect::<BTreeSet<_>>(),"tasks":[]});
            for severity in ["critical", "high", "medium", "low"] {
                result[severity] = json!(
                    rows.iter()
                        .filter(|r| r.data["severity"] == severity)
                        .count()
                );
            }
            let mut tasks = BTreeMap::<String, u64>::new();
            for row in &rows {
                if let Some(id) = row.data["task_id"].as_str() {
                    *tasks.entry(id.into()).or_default() += 1;
                }
            }
            result["tasks"]=json!(tasks.into_iter().filter_map(|(id,n)|s.get("tasks",&id).map(|task|json!({"id":id,"name":task.data["name"],"description":task.data["description"],"count":n}))).collect::<Vec<_>>());
            Ok(result)
        }
        "ui.stats" => {
            let task = active_task(p, s)?;
            let mut result = json!({"assets":s.records("assets").len(),"engine_mode":"idle","llm_configured":s.records("llm_profiles").iter().any(|r|r.data["pool_exclude"]!=true),"roe_enabled":setting(s,"roe.enabled","false")=="true","findings_confirmed":s.records("findings").iter().filter(|r|r.data["status"]=="confirmed").count()});
            if let Some(task) = task {
                let exp = &task.data["exploration_id"];
                let running = s
                    .records("agent_runs")
                    .iter()
                    .any(|r| r.data["task_id"] == task.header.id && r.data["state"] == "running");
                let mode = if task.data["paused"] == true {
                    "paused"
                } else if running {
                    "exploring"
                } else {
                    "idle"
                };
                result["engine_mode"] = json!(mode);
                result["active_task"] = json!({"id":task.header.id,"name":task.data["name"],"description":task.data["description"],"goal":task.data["goal"],"status":task.data["status"],"paused":task.data["paused"],"queued":task.data["queued"],"created_at":task.data["created_at"],"engine_mode":mode,"goals_total":s.records("exploration_nodes").iter().filter(|r|r.data["exploration_id"]==*exp&&r.data["kind"]=="goal").count(),"goals_met":s.records("exploration_nodes").iter().filter(|r|r.data["exploration_id"]==*exp&&r.data["kind"]=="goal"&&r.data["state"]=="met").count()});
            }
            Ok(result)
        }
        "ui.tokenStats" => {
            let task = active_task(p, s)?;
            let mut workers = BTreeMap::<String, Tokens>::new();
            let mut sessions = BTreeMap::<String, Tokens>::new();
            let mut total = Tokens::default();
            if let Some(task) = task {
                for r in s.records("activity").into_iter().filter(|r| {
                    r.data["exploration_id"] == task.data["exploration_id"]
                        && r.data["kind"] == "result"
                }) {
                    let worker = text(&r.data, "worker");
                    workers
                        .entry(worker.into())
                        .or_default()
                        .add(&r.data, false)?;
                    total.add(&r.data, false)?;
                    let session = if worker == "mainagent" {
                        Some(format!("main:{}", r.data["main_seg"].as_i64().unwrap_or(0)))
                    } else if worker == "planner" {
                        Some("plan".into())
                    } else {
                        r.data["node_id"]
                            .as_str()
                            .and_then(|id| s.get("exploration_nodes", id))
                            .filter(|n| n.data["kind"] == "intent")
                            .map(|n| format!("intent:{}", n.header.id))
                    };
                    if let Some(session) = session {
                        sessions.entry(session).or_default().add(&r.data, false)?;
                    }
                }
            }
            let workers = workers
                .into_iter()
                .map(|(worker, t)| {
                    let mut v = t.wire()?;
                    v["worker"] = json!(worker);
                    Ok(v)
                })
                .collect::<Result<Vec<_>>>()?;
            let sessions = sessions
                .into_iter()
                .map(|(session, t)| {
                    let mut v = t.wire()?;
                    v["session"] = json!(session);
                    Ok(v)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"workers":workers,"sessions":sessions,"total":total.wire()?}))
        }
        "ui.tokenDaily" => {
            let days = p["days"].as_u64().unwrap_or(30).clamp(1, 3650);
            let cutoff = (Utc::now() - Duration::days(days as i64 - 1))
                .format("%Y-%m-%d")
                .to_string();
            let mut buckets = BTreeMap::<String, Tokens>::new();
            for r in s
                .records("activity")
                .into_iter()
                .filter(|r| r.data["kind"] == "result")
            {
                let date = text(&r.data, "created_at").get(..10).unwrap_or("");
                if date >= cutoff.as_str() {
                    buckets
                        .entry(date.into())
                        .or_default()
                        .add(&r.data, false)?;
                }
            }
            Ok(json!(
                buckets
                    .into_iter()
                    .map(|(date, t)| {
                        let mut v = t.wire()?;
                        v["date"] = json!(date);
                        Ok(v)
                    })
                    .collect::<Result<Vec<_>>>()?
            ))
        }
        "ui.usageStats" => {
            let days = p["days"].as_u64().unwrap_or(365).clamp(1, 3650);
            let cutoff = (Utc::now() - Duration::days(days as i64 - 1))
                .format("%Y-%m-%d")
                .to_string();
            let mut totals = BTreeMap::<String, (u64, BTreeSet<String>, Tokens)>::new();
            let mut daily = BTreeMap::<(String, String), Tokens>::new();
            for row in s.records("llm_usage") {
                let d = &row.data;
                let profile = text(d, "profile_name");
                let entry = totals.entry(profile.into()).or_default();
                entry.0 += 1;
                if let Some(task) = d["task_id"].as_str().filter(|t| !t.is_empty()) {
                    entry.1.insert(task.into());
                }
                entry.2.add(d, true)?;
                let date = text(d, "ts").get(..10).unwrap_or("");
                if date >= cutoff.as_str() {
                    daily
                        .entry((profile.into(), date.into()))
                        .or_default()
                        .add(d, true)?;
                }
            }
            let by_profile = totals
                .into_iter()
                .map(|(profile, (calls, tasks, tokens))| {
                    let mut v = tokens.wire()?;
                    v["profile_name"] = json!(profile);
                    v["calls"] = count(calls)?;
                    v["tasks"] = count(tasks.len() as u64)?;
                    Ok(v)
                })
                .collect::<Result<Vec<_>>>()?;
            let daily = daily
                .into_iter()
                .map(|((profile, date), t)| {
                    let mut v = t.wire()?;
                    v.as_object_mut().unwrap().remove("cache_write_tokens");
                    v["profile_name"] = json!(profile);
                    v["date"] = json!(date);
                    Ok(v)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"by_profile":by_profile,"daily":daily}))
        }
        "ui.commandStats" => {
            let p = &p["params"];
            let task = text(p, "task");
            let exp = if task.is_empty() {
                None
            } else {
                Some(record(s, "tasks", task)?.data["exploration_id"].clone())
            };
            let q = text(p, "q").to_lowercase();
            let mut totals = BTreeMap::<String, (u64, u64)>::new();
            for r in s.records("activity").into_iter().filter(|r| {
                r.data["kind"] == "tool_use"
                    && exp
                        .as_ref()
                        .is_none_or(|exp| r.data["exploration_id"] == *exp)
            }) {
                if !q.is_empty()
                    && !text(&r.data, "tool").to_lowercase().contains(&q)
                    && !text(&r.data, "detail").to_lowercase().contains(&q)
                {
                    continue;
                }
                let tool = text(&r.data, "tool");
                let tool = if tool.is_empty() { "-" } else { tool };
                let error = s.records("activity").iter().any(|result| {
                    result.data["kind"] == "tool_result"
                        && result.data["tool_use_id"] == r.data["tool_use_id"]
                        && result.data["exploration_id"] == r.data["exploration_id"]
                        && result.data["is_error"] == true
                });
                let n = totals.entry(tool.into()).or_default();
                n.0 += 1;
                n.1 += u64::from(error);
            }
            let mut stats = totals
                .into_iter()
                .map(|(tool, (total, errors))| json!({"tool":tool,"total":total,"errors":errors}))
                .collect::<Vec<_>>();
            stats.sort_by(|a, b| {
                b["total"]
                    .as_u64()
                    .cmp(&a["total"].as_u64())
                    .then_with(|| a["tool"].as_str().cmp(&b["tool"].as_str()))
            });
            Ok(json!({"stats":stats}))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown auxiliary query")),
    }
}

fn merge_config(kind: &str, stored: &Value, incoming: &Value) -> Result<Value> {
    let mut config = stored.clone();
    let keys = secret_keys(kind);
    let destinations: &[&str] = match kind {
        "email" => &["host", "port", "tls"],
        "telegram" => &["base_url"],
        "webhook" => &["url"],
        "feishu" | "dingtalk" | "wecom" => &["webhook"],
        _ => &[],
    };
    let changed = destinations.iter().any(|key| {
        incoming.get(*key).is_some_and(|v| {
            v != &stored[*key] && !v.as_str().is_some_and(|s| s.starts_with("__masked__"))
        })
    });
    if changed {
        for key in keys {
            if incoming.get(*key).is_none_or(|v| {
                v.as_str().is_some_and(|s| s.starts_with("__masked__")) || v == &stored[*key]
            }) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Changed notification destination requires explicit replacement or clearing of every credential",
                ));
            }
        }
    }
    let Some(object) = incoming.as_object() else {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Notification config must be an object",
        ));
    };
    for (key, value) in object {
        if keys.contains(&key.as_str())
            && value.as_str().is_some_and(|s| s.starts_with("__masked__"))
        {
            if stored[key].is_null() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Masked credential has no stored value",
                ));
            }
            continue;
        }
        if keys.contains(&key.as_str())
            && value.is_object()
            && value.to_string().contains("__masked__")
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Nested masked credential is invalid",
            ));
        }
        config[key] = value.clone();
    }
    Ok(config)
}
pub fn gc_grace(p: &Value) -> Result<std::time::Duration> {
    let ttl = p
        .get("ttl")
        .map(|v| {
            v.as_f64()
                .filter(|n| n.is_finite() && *n >= 86400.0 && *n <= 31536000.0 && n.fract() == 0.0)
                .ok_or_else(|| {
                    Error::new(
                        "INVALID_PARAMS",
                        "GC ttl must be an integer of 86400..31536000 seconds",
                    )
                })
        })
        .transpose()?
        .unwrap_or(86400.0);
    Ok(std::time::Duration::from_secs(ttl as u64))
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, _c: &Context) -> Result<CommandBatch> {
    let mut planner = Planner::new(s, m, p);
    let result = match m {
        "ui.notifyCreateChannel" | "ui.notifyUpdateChannel" => {
            let payload = &p["payload"];
            let (id, mut d) = if m == "ui.notifyCreateChannel" {
                (
                    new_id(collection_prefix("notification_channels")?),
                    default_data("notification_channels")?,
                )
            } else {
                let id = required(p, "id")?.to_owned();
                (
                    id.clone(),
                    record(s, "notification_channels", &id)?.data.clone(),
                )
            };
            let old_kind = text(&d, "kind").to_owned();
            for key in ["name", "kind", "enabled", "mode", "filter", "rate_per_min"] {
                if let Some(v) = payload.get(key) {
                    d[key] = v.clone();
                }
            }
            d["id"] = json!(id);
            let kind = required(&d, "kind")?.to_owned();
            let default_rate = kind_rate(&kind)
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Unknown notification provider"))?;
            if m == "ui.notifyCreateChannel" && payload["rate_per_min"].is_null() {
                d["rate_per_min"] = json!(default_rate);
            }
            if !matches!(text(&d, "mode"), "realtime" | "digest") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Notification mode must be realtime or digest",
                ));
            }
            if d["rate_per_min"].as_i64().is_none_or(|n| n < 0) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Notification rate cannot be negative",
                ));
            }
            if !["", "low", "medium", "high", "critical"]
                .contains(&text(&d["filter"], "min_severity"))
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Invalid minimum notification severity",
                ));
            }
            if let Some(config) = payload.get("config") {
                let stored = if old_kind == kind {
                    d["config"].clone()
                } else {
                    json!({})
                };
                d["config"] = merge_config(&kind, &stored, config)?;
            } else if old_kind != kind {
                d["config"] = json!({});
            }
            artex_core::validate_known_credentials("notification_channels", &d)?;
            planner.put("notification_channels", &id, d);
            json!({"id":id})
        }
        "ui.notifyDeleteChannel" => {
            let id = required(p, "id")?;
            record(s, "notification_channels", id)?;
            planner.delete("notification_channels", id);
            json!({"ok":true})
        }
        "ui.gc" => {
            gc_grace(p)?;
            json!({"removed":0})
        }
        "ui.notifyTestChannel" => {
            record(s, "notification_channels", required(p, "id")?)?;
            json!({"ok":true,"latency_ms":0})
        }
        "ui.notifyRetryDelivery" => {
            let id = required(p, "id")?;
            let mut d = record(s, "notification_deliveries", id)?.data.clone();
            if !matches!(text(&d, "state"), "failed" | "skipped") {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Only failed or skipped notification can be retried",
                ));
            }
            d["state"] = json!("pending");
            d["next_attempt_at"] = json!(artex_core::now());
            d["last_error"] = json!("");
            d["attempts"] = json!(0);
            planner.put("notification_deliveries", id, d);
            json!({"ok":true})
        }
        "ui.archiveTask"
        | "ui.archiveTasks"
        | "ui.restoreTaskArchive"
        | "ui.restoreTaskArchives"
        | "ui.deleteTaskArchive"
        | "ui.deleteTaskArchives" => {
            let batch = m.ends_with("Archives") || m == "ui.archiveTasks";
            let targets = if batch {
                ids(
                    p,
                    if m == "ui.archiveTasks" {
                        "taskIds"
                    } else {
                        "archiveIds"
                    },
                )
            } else {
                vec![required(p, "id")?.into()]
            };
            if targets.is_empty() || targets.len() > 100 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Archive batch must contain 1..100 IDs",
                ));
            }
            let mut out = Vec::new();
            let mut single = None;
            for target in targets {
                let result = queue_archive(m, &target, &mut planner);
                match result {
                    Ok(d) => {
                        let wire = archive_wire(&d, s)?;
                        out.push(json!({"id":target,"archive_id":d["id"],"ok":true,"queued":true}));
                        single = Some(wire);
                    }
                    Err(e) if batch => {
                        out.push(json!({"id":target,"ok":false,"queued":false,"error":e.message}))
                    }
                    Err(e) => return Err(e),
                }
            }
            if batch {
                json!({"items":out})
            } else {
                single.unwrap()
            }
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", "Unknown auxiliary command")),
    };
    let mut batch = planner.done(result);
    crate::augment_expected(&mut batch, s)?;
    Ok(batch)
}
fn queue_archive(m: &str, target: &str, p: &mut Planner<'_>) -> Result<Value> {
    let s = p.state;
    if m == "ui.archiveTask" || m == "ui.archiveTasks" {
        let task = record(s, "tasks", target)?;
        if task.data["queued"] == true
            || task.data["paused"] != true
                && !matches!(text(&task.data, "status"), "done" | "failed" | "timeout")
        {
            return Err(Error::new(
                "BUSY",
                "Archive requires a terminal or paused task outside the queue",
            ));
        }
        for relation in s
            .records("task_relations")
            .into_iter()
            .filter(|r| r.data["source_task_id"] == target)
        {
            let child = text(&relation.data, "task_id");
            if s.get("tasks", child)
                .is_some_and(|t| t.data["deleted_at"].is_null())
            {
                return Err(Error::new(
                    "BUSY",
                    "A live task still directly references this source task",
                ));
            }
        }
        let existing = s
            .records("task_archives")
            .into_iter()
            .find(|r| r.data["task_id"] == target);
        if let Some(existing) = existing {
            if !matches!(
                text(&existing.data, "state"),
                "archive_failed" | "archive_queued"
            ) {
                return Err(Error::new("BUSY", "Task already has an archive workflow"));
            }
        }
        let id = existing
            .map(|r| r.header.id.clone())
            .unwrap_or_else(|| new_id("archive"));
        let mut d = existing
            .map(|r| r.data.clone())
            .unwrap_or(default_data("task_archives")?);
        d["id"] = json!(id);
        d["task_id"] = json!(target);
        d["state"] = json!("archive_queued");
        d["phase"] = json!("queued");
        d["progress"] = json!(0);
        d["error"] = json!("");
        d["format_version"] = json!(1);
        for (key, source) in [
            ("task_name", "name"),
            ("task_description", "description"),
            ("task_goal", "goal"),
            ("original_status", "status"),
        ] {
            d[key] = task.data[source].clone();
        }
        d["source_task_ids"] = json!(
            direct_sources(s, target)
                .into_iter()
                .filter(|id| id != target)
                .collect::<Vec<_>>()
        );
        d["requested_at"] = json!(artex_core::now());
        if let Some(category) = task.data["category_id"]
            .as_str()
            .and_then(|id| s.get("task_categories", id))
        {
            d["category_name_snapshot"] = category.data["name"].clone();
            if category
                .header
                .legacy_key
                .as_str()
                .is_some_and(|s| s.parse::<u64>().is_ok())
            {
                d["category_id_snapshot"] = category.header.legacy_key.clone();
            }
        }
        if let Some(runtime) = s
            .records("task_runtime")
            .into_iter()
            .find(|r| r.data["task_id"] == target)
        {
            d["remaining_timeout_seconds"] = json!(
                text(&runtime.data, "remaining_timeout_ms")
                    .parse::<u64>()
                    .unwrap_or(0)
                    .div_ceil(1000)
                    .to_string()
            );
        }
        p.put("task_archives", &id, d.clone());
        Ok(d)
    } else {
        let mut d = record(s, "task_archives", target)?.data.clone();
        let restore = m.starts_with("ui.restore");
        let allowed = if restore {
            matches!(text(&d, "state"), "ready" | "restore_failed")
        } else {
            matches!(
                text(&d, "state"),
                "ready" | "archive_failed" | "restore_failed" | "delete_failed"
            )
        };
        if !allowed {
            return Err(Error::new(
                "BUSY",
                "Archive workflow cannot transition from current state",
            ));
        }
        d["state"] = json!(if restore {
            "restore_queued"
        } else {
            "delete_queued"
        });
        d["phase"] = json!("queued");
        d["progress"] = json!(0);
        d["error"] = json!("");
        p.put("task_archives", target, d.clone());
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_store::Store;
    fn stamped(mut b: CommandBatch) -> CommandBatch {
        b.command_id = new_id("command");
        b
    }
    fn ctx() -> Context {
        Context {
            actor: artex_core::Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        }
    }
    fn check(method: &str, result: &Value) {
        let rpc = artex_core::contracts::read("rpc.v1.schema.json");
        artex_core::validate_contract(&rpc["results"][method], &rpc["$defs"], result).unwrap();
    }
    #[test]
    fn empty_auxiliary_queries_match_all_source_dtos() {
        let s = Snapshot {
            workspace_id: new_id("workspace"),
            commit_seq: 0,
            collections: BTreeMap::new(),
        };
        for method in QUERIES.iter().filter(|m| **m != "ui.taskArchive") {
            check(method, &query(method, &json!({}), &s, &ctx()).unwrap());
        }
    }
    #[test]
    fn notification_secret_mask_patch_and_destination_checks() {
        let t = tempfile::tempdir().unwrap();
        let mut store = Store::create(&t.path().join("w"), "notify").unwrap();
        let c = ctx();
        let p = json!({"payload":{"name":"smtp","kind":"email","config":{"host":"smtp.example.test","password":{"secret_ref":new_id("secret"),"hint":"vault"}}}});
        let b = plan("ui.notifyCreateChannel", &p, &store.snapshot(), &c).unwrap();
        check("ui.notifyCreateChannel", &b.result);
        let id = b.result["id"].as_str().unwrap().to_owned();
        store.commit(stamped(b)).unwrap();
        let result = query("ui.notifyChannels", &json!({}), &store.snapshot(), &c).unwrap();
        check("ui.notifyChannels", &result);
        assert_eq!(result[0]["config"]["password"], "__masked__");
        let saved = store
            .snapshot()
            .get("notification_channels", &id)
            .unwrap()
            .data["config"]["password"]
            .clone();
        let b=plan("ui.notifyUpdateChannel",&json!({"id":id,"payload":{"config":{"password":"__masked__","host":"smtp.example.test"}}}),&store.snapshot(),&c).unwrap();
        store.commit(stamped(b)).unwrap();
        assert_eq!(
            store
                .snapshot()
                .get("notification_channels", &id)
                .unwrap()
                .data["config"]["password"],
            saved
        );
        assert_eq!(
            plan(
                "ui.notifyUpdateChannel",
                &json!({"id":id,"payload":{"config":{"host":"other.example.test"}}}),
                &store.snapshot(),
                &c
            )
            .unwrap_err()
            .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            plan(
                "ui.notifyUpdateChannel",
                &json!({"id":id,"payload":{"config":{"password":"plain"}}}),
                &store.snapshot(),
                &c
            )
            .unwrap_err()
            .code,
            "PERMISSION_DENIED"
        );
    }
    #[test]
    fn source_token_sessions_exclude_tool_results_and_preserve_main_segments() {
        let t = tempfile::tempdir().unwrap();
        let mut store = Store::create(&t.path().join("w"), "stats").unwrap();
        let c = ctx();
        let snapshot = store.snapshot();
        let mut planner = Planner::new(&snapshot, "fixture.seed", &json!({}));
        let exp = planner
            .insert("explorations", "exploration", json!({"goal":"g"}))
            .unwrap();
        let task=planner.insert("tasks","task",json!({"exploration_id":exp,"name":"task","description":"d","goal":"g","status":"running"})).unwrap();
        for (kind, worker, segment, n) in [
            ("result", "mainagent", 1, 5),
            ("result", "mainagent", 2, 9),
            ("result", "planner", 0, 7),
            ("tool_result", "mainagent", 1, 999),
        ] {
            planner.insert("activity",collection_prefix("activity").unwrap(),json!({"exploration_id":exp,"kind":kind,"worker":worker,"main_seg":segment,"input_tokens":n})).unwrap();
        }
        store.commit(stamped(planner.done(json!({})))).unwrap();
        let out = query(
            "ui.tokenStats",
            &json!({"task":task}),
            &store.snapshot(),
            &c,
        )
        .unwrap();
        check("ui.tokenStats", &out);
        assert_eq!(out["total"]["input_tokens"], 21);
        assert_eq!(out["sessions"].as_array().unwrap().len(), 3);
        assert_eq!(out["sessions"][0]["session"], "main:1");
    }
}
