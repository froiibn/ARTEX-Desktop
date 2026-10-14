//! Durable chat lifecycle. The writer commits admission before the controller
//! starts an LLM turn; activities are the common transcript and UI stream.
use crate::*;
use artex_core::body;
use chrono::{Duration, Utc};

pub const QUERIES: &[&str] = &[
    "conversation.list",
    "conversation.messages",
    "conversation.detail",
    "chat.status",
];
pub const COMMANDS: &[&str] = &[
    "conversation.create",
    "conversation.update",
    "conversation.profile",
    "conversation.delete",
    "conversation.delete_batch",
    "conversation.send",
    "conversation.stop",
    "chat.send",
    "chat.stop",
];
fn active(state: &str) -> bool {
    matches!(state, "queued" | "running" | "waiting" | "paused")
}
pub fn agent_by_key<'a>(s: &'a Snapshot, key: &str) -> Result<&'a Record> {
    s.records("agents")
        .into_iter()
        .find(|a| text(&a.data, "key") == key)
        .ok_or_else(|| Error::new("NOT_FOUND", "Agent does not exist"))
}
pub fn effective_profile<'a>(
    s: &'a Snapshot,
    agent: &Record,
    conversation: Option<&Record>,
    task: Option<&Record>,
) -> Result<&'a Record> {
    if let Some(profile) = agent.data["llm_profile_id"]
        .as_str()
        .and_then(|id| s.get("llm_profiles", id))
        .filter(|p| crate::llm_ui::configured(p))
    {
        return Ok(profile);
    }
    let selected = conversation.and_then(|r| r.data["llm_profile_id"].as_str());
    let profile = if let Some(id) = selected {
        record(s, "llm_profiles", id)?
    } else if let Some(task) =
        task.filter(|t| !crate::llm_runtime::chain(s, &t.header.id).is_empty())
    {
        let id = task.data["active_llm_profile_id"]
            .as_str()
            .ok_or_else(|| Error::new("FEATURE_DISABLED", "Task LLM profile chain is exhausted"))?;
        record(s, "llm_profiles", id)?
    } else if let Some(id) = task.and_then(|t| t.data["llm_profile_id"].as_str()) {
        record(s, "llm_profiles", id)?
    } else {
        s.records("llm_profiles")
            .into_iter()
            .find(|p| p.data["is_default"] == true)
            .ok_or_else(|| {
                Error::new(
                    "FEATURE_DISABLED",
                    "Configure an active LLM profile before sending a message",
                )
            })?
    };
    if !crate::llm_ui::configured(profile) {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "The selected LLM profile has no valid model, format or API key",
        ));
    }
    Ok(profile)
}
pub use crate::llm_runtime::{profile_failure_plan, profile_success_plan};
fn busy(s: &Snapshot, conversation: Option<&str>, task: Option<&str>) -> bool {
    s.records("agent_runs").into_iter().any(|r| {
        active(text(&r.data, "state"))
            && match (conversation, task) {
                (Some(id), _) => r.data["conversation_id"] == id,
                (_, Some(id)) => {
                    r.data["task_id"] == id
                        && r.data["intent_id"].is_null()
                        && s.get("agents", text(&r.data, "agent_id"))
                            .is_some_and(|a| a.data["key"] == "mainagent")
                }
                _ => false,
            }
    })
}
pub fn push_event(
    b: &mut CommandBatch,
    s: &Snapshot,
    c: &Context,
    kind: &str,
    collection: &str,
    id: &str,
) -> Result<()> {
    let seq = s
        .records("domain_events")
        .into_iter()
        .filter_map(|r| text(&r.data, "seq").parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .checked_add(b.events.len() as u64 + 1)
        .ok_or_else(|| Error::new("SIZE_LIMIT", "Event sequence exhausted"))?;
    let stamp = artex_core::now();
    let eid = new_id("event");
    let task_id = if collection == "tasks" {
        Some(id.to_owned())
    } else {
        c.task_id
            .clone()
            .or_else(|| {
                b.changes.iter().find_map(|change| match change {
                    Change::Put {
                        collection: object_collection,
                        id: object_id,
                        data,
                    } if object_collection == collection && object_id == id => {
                        data["task_id"].as_str().map(str::to_owned)
                    }
                    _ => None,
                })
            })
            .or_else(|| {
                s.get(collection, id)
                    .and_then(|r| r.data["task_id"].as_str())
                    .map(str::to_owned)
            })
    };
    b.events.push(Record { header: artex_core::Header { id:eid.clone(), collection:"domain_events".into(), version:"1".into(), commit_seq:(s.commit_seq+1).to_string(), created_at:stamp.clone(), updated_at:stamp.clone(), legacy_key:Value::Null },
        data:json!({"id":eid,"seq":seq.to_string(),"kind":kind,"task_id":task_id,"subject_collection":collection,"subject_id":id,"subject_version":s.get(collection,id).map(|r|(r.header.version.parse::<u64>().unwrap_or(0)+1).to_string()).unwrap_or("1".into()),"actor":c.actor,"created_at":stamp}) });
    Ok(())
}
pub fn wire_conversation(r: &Record, s: &Snapshot) -> Value {
    let mut d = r.data.clone();
    d["created_at"] = json!(r.header.created_at);
    d["updated_at"] = json!(r.header.updated_at);
    d["pinned"] = json!(!d["pinned_at"].is_null());
    d["running"] = json!(busy(s, Some(&r.header.id), None));
    if let Some(o) = d.as_object_mut() {
        o.remove("trashed_at");
        o.remove("trash_generation");
        if o["llm_profile_id"].is_null() {
            o.remove("llm_profile_id");
        }
    }
    d
}
fn activity_body(detail: &Value) -> Value {
    if detail.is_null() {
        body("")
    } else if detail.is_string() {
        body(detail.as_str().unwrap())
    } else {
        detail.clone()
    }
}
pub fn public_activity_kind(kind: &str) -> bool {
    !matches!(kind, "context_config" | "context_nudge" | "compression")
}
/// Preserve internal activity provenance in storage while exposing source UI kinds.
pub fn activity_wire_kind(kind: &str) -> &str {
    match kind {
        "guard" => "text",
        "skill_guidance" | "scheduler_retry" => "user",
        kind => kind,
    }
}
pub fn wire_activity(r: &Record, with_detail: bool) -> Value {
    let d = &r.data;
    let mut out = json!({"seq":r.header.id,"worker":text(d,"worker"),"ts":r.header.created_at,"kind":activity_wire_kind(text(d,"kind")),"is_error":d["is_error"],"summary":text(d,"summary")});
    for (stored, wire) in [
        ("node_id", "intent_id"),
        ("tool", "tool"),
        ("tool_use_id", "tool_use_id"),
        ("main_seg", "main_seg"),
        ("input_tokens", "input_tokens"),
        ("output_tokens", "output_tokens"),
        ("cache_read_tokens", "cache_read_tokens"),
        ("cache_write_tokens", "cache_write_tokens"),
    ] {
        if !d[stored].is_null() {
            out[wire] = d[stored].clone();
        }
    }
    if !d["metadata"]["llm_transition"].is_null() {
        out["metadata"] = json!({"llm_transition":d["metadata"]["llm_transition"]});
    }
    if with_detail {
        out["detail"] = activity_body(&d["detail"]);
    }
    out
}
fn title(value: &str, allow_default: bool) -> Result<String> {
    let value = value.trim();
    if value.is_empty() && !allow_default {
        return Err(Error::new("INVALID_PARAMS", "Title cannot be empty"));
    }
    if value.chars().count() > 200 {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Title must not exceed 200 characters",
        ));
    }
    Ok(if value.is_empty() {
        "new conversation".into()
    } else {
        value.into()
    })
}
pub fn cancel_run(b: &mut Planner, c: &Context, id: &str) -> Result<()> {
    let run = record(b.state, "agent_runs", id)?;
    if !active(text(&run.data, "state")) {
        return Ok(());
    }
    let mut d = run.data.clone();
    d["state"] = json!("cancelled");
    d["finished_at"] = json!(artex_core::now());
    b.put("agent_runs", id, d);
    for q in b.state.records("user_questions") {
        if q.data["run_id"] == id && q.data["state"] == "pending" {
            let mut d = q.data.clone();
            d["state"] = json!("cancelled");
            b.put("user_questions", &q.header.id, d);
        }
    }
    for delivery in b.state.records("agent_deliveries") {
        if delivery.data["owner"]["run_id"] == id
            && matches!(
                text(&delivery.data, "state"),
                "pending" | "claimed" | "observed" | "interrupted"
            )
        {
            let mut d = delivery.data.clone();
            d["state"] = json!("cancelled");
            d["claim_deadline"] = Value::Null;
            d["completed_at"] = json!(artex_core::now());
            b.put("agent_deliveries", &delivery.header.id, d);
        }
    }
    for request in b.state.records("conversation_requests") {
        if matches!(text(&request.data, "state"), "open" | "claimed" | "waiting") {
            if b.state
                .get("agent_messages", &request.header.id)
                .is_some_and(|m| m.data["sender"]["run_id"] == id)
            {
                let mut d = request.data.clone();
                d["state"] = json!("cancelled");
                b.put("conversation_requests", &request.header.id, d);
                for delivery in b.state.records("agent_deliveries") {
                    if delivery.data["message_id"] == request.header.id
                        && matches!(
                            text(&delivery.data, "state"),
                            "pending" | "claimed" | "observed" | "interrupted"
                        )
                    {
                        let mut d = delivery.data.clone();
                        d["state"] = json!("cancelled");
                        d["claim_deadline"] = Value::Null;
                        d["completed_at"] = json!(artex_core::now());
                        b.put("agent_deliveries", &delivery.header.id, d);
                    }
                }
            }
        }
    }
    for retest in b.state.records("finding_retests") {
        if retest.data["conversation_id"] == run.data["conversation_id"]
            && !run.data["conversation_id"].is_null()
            && matches!(text(&retest.data, "status"), "pending" | "running")
        {
            let mut d = retest.data.clone();
            d["status"] = json!("stopped");
            d["finished_at"] = json!(artex_core::now());
            b.put("finding_retests", &retest.header.id, d);
        }
    }
    push_event(&mut b.batch, b.state, c, "RunCancelled", "agent_runs", id)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    match m {
        "conversation.create" => {
            let key = required(p, "agent_key")?.trim();
            if key.chars().count() > 120 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Agent key exceeds 120 characters",
                ));
            }
            agent_by_key(s, key)?;
            if let Some(id) = p["llm_profile_id"].as_str() {
                record(s, "llm_profiles", id)?;
            }
            let id=b.insert("conversations","conversation",json!({"agent_key":key,"title":title(text(p,"title"),true)?,"llm_profile_id":p["llm_profile_id"]}))?;
            let mut result = b.data("conversations", &id)?;
            result.as_object_mut().unwrap().remove("trashed_at");
            result.as_object_mut().unwrap().remove("trash_generation");
            result["pinned"] = json!(false);
            result["running"] = json!(false);
            if result["llm_profile_id"].is_null() {
                result.as_object_mut().unwrap().remove("llm_profile_id");
            }
            Ok(b.done(result))
        }
        "conversation.update" | "conversation.profile" => {
            let id = required(p, "id")?;
            let mut d = record(s, "conversations", id)?.data.clone();
            if !d["trashed_at"].is_null() {
                return Err(Error::new(
                    "NOT_FOUND",
                    "Restore the conversation before editing it",
                ));
            }
            if m == "conversation.profile" {
                if let Some(profile) = p["llm_profile_id"].as_str() {
                    record(s, "llm_profiles", profile)?;
                }
                d["llm_profile_id"] = p["llm_profile_id"].clone();
            } else {
                let patch = &p["patch"];
                if patch.get("title").is_none() && patch.get("pinned").is_none() {
                    return Err(Error::new("INVALID_PARAMS", "Provide title or pinned"));
                }
                if let Some(value) = patch["title"].as_str() {
                    d["title"] = json!(title(value, false)?);
                }
                if let Some(pinned) = patch["pinned"].as_bool() {
                    if pinned && d["pinned_at"].is_null() {
                        d["pinned_at"] = json!(artex_core::now());
                    } else if !pinned {
                        d["pinned_at"] = Value::Null;
                    }
                }
            }
            b.put("conversations", id, d.clone());
            d.as_object_mut().unwrap().remove("trashed_at");
            d.as_object_mut().unwrap().remove("trash_generation");
            d["pinned"] = json!(!d["pinned_at"].is_null());
            d["running"] = json!(busy(s, Some(id), None));
            if d["llm_profile_id"].is_null() {
                d.as_object_mut().unwrap().remove("llm_profile_id");
            }
            Ok(b.done(if m == "conversation.profile" {
                json!({"ok":true})
            } else {
                d
            }))
        }
        "conversation.delete" | "conversation.delete_batch" => {
            let mut targets = if m == "conversation.delete" {
                vec![required(p, "id")?.to_owned()]
            } else {
                ids(p, "ids")
            };
            let mut seen = BTreeSet::new();
            targets.retain(|id| seen.insert(id.clone()));
            if targets.is_empty() || targets.len() > 100 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Conversation delete batch requires 1–100 IDs",
                ));
            }
            let mut items = vec![];
            for id in targets {
                if s.get("conversations", &id).is_none() {
                    if m == "conversation.delete" {
                        return Err(Error::new("NOT_FOUND", "Conversation not found"));
                    }
                    items.push(json!({"id":id,"ok":false,"error":"conversation not found"}));
                    continue;
                }
                for run in s.records("agent_runs") {
                    if run.data["conversation_id"] == id {
                        cancel_run(&mut b, c, &run.header.id)?;
                    }
                }
                b.delete("conversations", &id);
                items.push(json!({"id":id,"ok":true}));
            }
            Ok(b.done(if m == "conversation.delete" {
                json!({"ok":true})
            } else {
                json!({"items":items})
            }))
        }
        "conversation.send" | "chat.send" => {
            let conversation = if m == "conversation.send" {
                Some(record(s, "conversations", required(p, "id")?)?)
            } else {
                None
            };
            let task = if m == "chat.send" {
                Some(record(s, "tasks", task_id(p, c)?)?)
            } else {
                None
            };
            if conversation.is_some_and(|row| !row.data["trashed_at"].is_null()) {
                return Err(Error::new(
                    "NOT_FOUND",
                    "Restore the conversation before sending",
                ));
            }
            if task.is_some_and(|t| {
                !t.data["deleted_at"].is_null() || !t.data["archived_at"].is_null()
            }) {
                return Err(Error::new("NOT_FOUND", "Task is deleted or archived"));
            }
            if busy(
                s,
                conversation.map(|r| r.header.id.as_str()),
                task.map(|r| r.header.id.as_str()),
            ) {
                return Err(Error::new(
                    "BUSY",
                    "The session is processing its previous message",
                ));
            }
            let message = text(p, "message").trim();
            let attachments = p["attachments"].as_array().cloned().unwrap_or_default();
            if message.is_empty() && attachments.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Message cannot be empty"));
            }
            let key = conversation
                .map(|r| text(&r.data, "agent_key"))
                .unwrap_or("mainagent");
            let agent = agent_by_key(s, key)?;
            if agent.data["enabled"] != true {
                return Err(Error::new("FEATURE_DISABLED", "Agent is disabled"));
            }
            effective_profile(s, agent, conversation, task)?;
            let rid=b.insert("agent_runs","run",json!({"agent_id":agent.header.id,"task_id":task.map(|r|r.header.id.clone()),"conversation_id":conversation.map(|r|r.header.id.clone()),"engine_epoch":c.actor.engine_epoch,"state":"queued","deadline_at":(Utc::now()+Duration::seconds(agent.data["run_seconds"].as_i64().unwrap_or(1200).max(1))).to_rfc3339_opts(chrono::SecondsFormat::Millis,true)}))?;
            let referenced = mentions_ui::compose(&mut b, message)?;
            let full =
                body(&json!({"text":referenced,"display_text":message,"attachments":attachments,"run_id":rid}).to_string());
            let summary = message
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            let mut activity = json!({"worker":key,"kind":"user","summary":summary,"detail":full});
            if let Some(conversation) = conversation {
                activity["conversation_id"] = json!(conversation.header.id);
                b.insert("conversation_activities", "conversationactivity", activity)?;
                let mut d = conversation.data.clone();
                if matches!(text(&d, "title"), "" | "new conversation") {
                    let value = message
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(40)
                        .collect::<String>();
                    d["title"] = json!(if value.is_empty() {
                        "Attachment message"
                    } else {
                        &value
                    });
                }
                d["updated_at"] = json!(artex_core::now());
                b.put("conversations", &conversation.header.id, d);
            } else if let Some(task) = task {
                activity["exploration_id"] = task.data["exploration_id"].clone();
                activity["metadata"] = json!({"run_id":rid,"turn_id":rid});
                activity["main_seg"] = p["seg"].clone();
                b.insert("activity", "activity", activity)?;
            }
            push_event(&mut b.batch, s, c, "RunQueued", "agent_runs", &rid)?;
            Ok(b.done(if m == "chat.send" {
                json!({"status":"accepted","mode":"llm"})
            } else {
                json!({"status":"accepted"})
            }))
        }
        "conversation.stop" | "chat.stop" => {
            let conv = if m == "conversation.stop" {
                Some(required(p, "id")?)
            } else {
                None
            };
            let task = if m == "chat.stop" {
                Some(task_id(p, c)?)
            } else {
                None
            };
            if let Some(id) = conv {
                record(s, "conversations", id)?;
            }
            if let Some(id) = task {
                record(s, "tasks", id)?;
            }
            let mut stopped = false;
            for run in s.records("agent_runs") {
                if active(text(&run.data, "state"))
                    && ((conv.is_some() && run.data["conversation_id"] == conv.unwrap())
                        || (task.is_some()
                            && run.data["task_id"] == task.unwrap()
                            && run.data["intent_id"].is_null()
                            && s.get("agents", text(&run.data, "agent_id"))
                                .is_some_and(|agent| agent.data["key"] == "mainagent")))
                {
                    cancel_run(&mut b, c, &run.header.id)?;
                    stopped = true;
                }
            }
            Ok(b.done(json!({"status":if stopped{"stopping"}else{"idle"}})))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m {
        "conversation.list" => {
            let mut rows = s.records("conversations");
            rows.retain(|row| row.data["trashed_at"].is_null());
            rows.sort_by(|a, b| {
                b.data["pinned_at"]
                    .as_str()
                    .cmp(&a.data["pinned_at"].as_str())
                    .then(b.header.updated_at.cmp(&a.header.updated_at))
                    .then(b.header.id.cmp(&a.header.id))
            });
            Ok(json!(
                rows.into_iter()
                    .map(|r| wire_conversation(r, s))
                    .collect::<Vec<_>>()
            ))
        }
        "chat.status" => {
            let id = task_id(p, c)?;
            record(s, "tasks", id)?;
            Ok(json!({"running":busy(s,None,Some(id))}))
        }
        "conversation.detail" => {
            let id = required(p, "id")?;
            record(s, "conversations", id)?;
            let row = record(s, "conversation_activities", required(p, "seq")?)?;
            if row.data["conversation_id"] != id {
                return Err(Error::new("NOT_FOUND", "Conversation activity not found"));
            }
            if !public_activity_kind(text(&row.data, "kind")) {
                return Err(Error::new("NOT_FOUND", "Internal conversation activity"));
            }
            Ok(activity_body(&row.data["detail"]))
        }
        "conversation.messages" => {
            let id = required(p, "id")?;
            record(s, "conversations", id)?;
            let mut rows = s
                .records("conversation_activities")
                .into_iter()
                .filter(|r| r.data["conversation_id"] == id)
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                a.header
                    .created_at
                    .cmp(&b.header.created_at)
                    .then(
                        a.header
                            .commit_seq
                            .parse::<u64>()
                            .unwrap_or(0)
                            .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0)),
                    )
                    .then(a.header.id.cmp(&b.header.id))
            });
            let history = p["history"] == true;
            let cursor = text(p, if history { "before" } else { "since" });
            let position = if cursor.is_empty() {
                None
            } else {
                if let Some(position) = rows.iter().position(|r| r.header.id == cursor) {
                    Some(position)
                } else if let Some(replaced) =
                    s.replaced_activity_seq("conversation_activities", cursor)
                {
                    rows.retain(|r| {
                        if history {
                            r.header.commit_seq.parse::<u64>().unwrap_or(0) < replaced
                        } else {
                            r.header.commit_seq.parse::<u64>().unwrap_or(0) >= replaced
                        }
                    });
                    None
                } else {
                    return Err(Error::new("NOT_FOUND", "Conversation cursor not found"));
                }
            };
            let limit = p["limit"]
                .as_u64()
                .unwrap_or(if history { 200 } else { 1000 })
                .clamp(1, 1000) as usize;
            if history {
                if let Some(index) = position {
                    rows.truncate(index);
                }
                let more = rows.len() > limit;
                let skip = rows.len().saturating_sub(limit);
                rows = rows.into_iter().skip(skip).collect();
                let cursor = rows
                    .last()
                    .map(|r| json!(r.header.id))
                    .unwrap_or(Value::Null);
                Ok(
                    json!({"items":rows.iter().filter(|r|public_activity_kind(text(&r.data,"kind"))).map(|r|wire_activity(r,false)).collect::<Vec<_>>(),"cursor":cursor,"running":busy(s,Some(id),None),"hasMore":more}),
                )
            } else {
                if let Some(index) = position {
                    rows = rows.into_iter().skip(index + 1).collect();
                }
                rows.truncate(limit);
                let cursor = rows.last().map(|r| json!(r.header.id)).unwrap_or_else(|| {
                    if cursor.is_empty() {
                        Value::Null
                    } else {
                        json!(cursor)
                    }
                });
                Ok(
                    json!({"items":rows.iter().filter(|r|public_activity_kind(text(&r.data,"kind"))).map(|r|wire_activity(r,false)).collect::<Vec<_>>(),"cursor":cursor,"running":busy(s,Some(id),None)}),
                )
            }
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}

pub fn append_activity(
    s: &Snapshot,
    c: &Context,
    run_id: &str,
    kind: &str,
    summary: &str,
    detail: &Value,
    usage: Option<&Value>,
) -> Result<CommandBatch> {
    append_activity_inner(s, c, run_id, kind, summary, detail, usage, false)
}
fn append_activity_inner(
    s: &Snapshot,
    c: &Context,
    run_id: &str,
    kind: &str,
    summary: &str,
    detail: &Value,
    usage: Option<&Value>,
    finishing: bool,
) -> Result<CommandBatch> {
    let run = record(s, "agent_runs", run_id)?;
    if run.data["engine_epoch"] != c.actor.engine_epoch {
        return Err(Error::new(
            "STALE_EPOCH",
            "Run belongs to another engine epoch",
        ));
    }
    if !matches!(text(&run.data, "state"), "running" | "waiting")
        && !(finishing && active(text(&run.data, "state")))
    {
        return Err(Error::new("CANCELLED", "Run is not active"));
    }
    let mut b = Planner::new(
        s,
        "run.append_activity",
        &json!({"run_id":run_id,"kind":kind,"summary":summary,"detail":detail,"usage":usage}),
    );
    let agent = record(s, "agents", text(&run.data, "agent_id"))?;
    let mut d = json!({"worker":agent.data["key"],"kind":kind,"summary":summary,"detail":activity_body(detail),"is_error":false});
    if let Some(u) = usage {
        for key in [
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
        ] {
            if let Some(v) = u.get(key) {
                d[key] = v.clone();
            }
        }
    }
    let (collection, prefix) = if let Some(id) = run.data["conversation_id"].as_str() {
        d["conversation_id"] = json!(id);
        ("conversation_activities", "conversationactivity")
    } else {
        let task = record(s, "tasks", text(&run.data, "task_id"))?;
        d["exploration_id"] = task.data["exploration_id"].clone();
        d["node_id"] = run.data["intent_id"].clone();
        d["metadata"] = json!({"run_id":run_id,"turn_id":run_id});
        ("activity", "activity")
    };
    let id = b.insert(collection, prefix, d)?;
    push_event(&mut b.batch, s, c, "ActivityAppended", collection, &id)?;
    let mut batch = b.done(json!({"activity_id":id}));
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
pub fn finish_run(
    s: &Snapshot,
    c: &Context,
    run_id: &str,
    status: &str,
    detail: &Value,
    usage: Option<&Value>,
) -> Result<CommandBatch> {
    if !matches!(status, "completed" | "failed" | "cancelled") {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Run finish requires a terminal state",
        ));
    }
    let run = record(s, "agent_runs", run_id)?;
    if !active(text(&run.data, "state")) {
        let mut b = empty_batch("run.finish", &json!({"run_id":run_id,"status":status}));
        b.result = json!({"state":run.data["state"]});
        return Ok(b);
    }
    let mut batch = append_activity_inner(
        s,
        c,
        run_id,
        "result",
        if status == "completed" {
            "Completed"
        } else {
            status
        },
        detail,
        usage,
        true,
    )?;
    if status == "failed" {
        for change in &mut batch.changes {
            if let Change::Put {
                collection, data, ..
            } = change
            {
                if collection == "activity" || collection == "conversation_activities" {
                    data["is_error"] = json!(true);
                }
            }
        }
    }
    let mut b = Planner::new(
        s,
        "run.finish",
        &json!({"run_id":run_id,"status":status,"detail":detail,"usage":usage}),
    );
    b.batch.changes.append(&mut batch.changes);
    b.batch.events.append(&mut batch.events);
    b.batch.expected.append(&mut batch.expected);
    let mut d = run.data.clone();
    d["state"] = json!(status);
    d["finished_at"] = json!(artex_core::now());
    b.put("agent_runs", run_id, d);
    if let Some(intent) = run.data["intent_id"].as_str() {
        let mut n = record(s, "exploration_nodes", intent)?.data.clone();
        n["state"] = json!(match status {
            "completed" => "done",
            "failed" => "blocked",
            _ => "stopped",
        });
        b.put("exploration_nodes", intent, n);
    }
    if let Some(conversation) = run.data["conversation_id"].as_str() {
        let mut d = record(s, "conversations", conversation)?.data.clone();
        d["updated_at"] = json!(artex_core::now());
        b.put("conversations", conversation, d);
    }
    push_event(&mut b.batch, s, c, "RunFinished", "agent_runs", run_id)?;
    let mut batch = b.done(json!({"state":status}));
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
