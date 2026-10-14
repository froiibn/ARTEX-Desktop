#![forbid(unsafe_code)]

use artex_core::{
    Actor, Change, CommandBatch, Context, Error, Expected, Result, Snapshot, new_id, now,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};

fn invalid(message: &str) -> Error {
    Error::new("INVALID_PARAMS", message)
}
fn denied(message: &str) -> Error {
    Error::new("PERMISSION_DENIED", message)
}
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(&format!("Missing string: {key}")))
}
fn batch(method: &str, params: &Value) -> CommandBatch {
    CommandBatch {
        command_id: new_id("command"),
        method: method.into(),
        normalized_params: params.clone(),
        expected: vec![],
        changes: vec![],
        events: vec![],
        deliveries: vec![],
        result: Value::Null,
    }
}
fn put(b: &mut CommandBatch, state: &Snapshot, collection: &str, id: &str, data: Value) {
    if let Some(r) = state.get(collection, id) {
        if !b
            .expected
            .iter()
            .any(|e| e.collection == collection && e.id == id)
        {
            b.expected.push(Expected {
                collection: collection.into(),
                id: id.into(),
                version: r.header.version.clone(),
            });
        }
    }
    if let Some(Change::Put { data: existing, .. }) = b
        .changes
        .iter_mut()
        .find(|c| matches!(c,Change::Put{collection:c,id:i,..} if c==collection && i==id))
    {
        *existing = data;
    } else {
        b.changes.push(Change::Put {
            collection: collection.into(),
            id: id.into(),
            data,
        });
    }
}

/// A sender is derived from the engine's active run, never from the submitted envelope.
pub fn authenticate(state: &Snapshot, context: &Context) -> Result<()> {
    if context.actor.kind != "agent" {
        return Err(denied("An authenticated agent run is required"));
    }
    let aid = context
        .actor
        .agent_id
        .as_deref()
        .ok_or_else(|| denied("Agent identity is missing"))?;
    let rid = context
        .actor
        .run_id
        .as_deref()
        .ok_or_else(|| denied("Run identity is missing"))?;
    let agent = state
        .get("agents", aid)
        .ok_or_else(|| denied("Agent does not exist"))?;
    if agent.data["enabled"] != true {
        return Err(denied("Agent is disabled"));
    }
    let run = state
        .get("agent_runs", rid)
        .ok_or_else(|| denied("Run does not exist"))?;
    if run.data["agent_id"] != aid
        || run.data["engine_epoch"] != context.actor.engine_epoch
        || !matches!(run.data["state"].as_str(), Some("running" | "waiting"))
    {
        return Err(denied("Run identity, epoch, or state is no longer valid"));
    }
    if run.data["task_id"].as_str() != context.task_id.as_deref()
        || run.data["intent_id"].as_str() != context.intent_id.as_deref()
        || run.data["conversation_id"].as_str() != context.conversation_id.as_deref()
    {
        return Err(denied("Run scope cannot be supplied by the caller"));
    }
    Ok(())
}

pub fn context_for_run(state: &Snapshot, run_id: &str, epoch: &str) -> Result<Context> {
    let r = state
        .get("agent_runs", run_id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Run not found"))?;
    let context = Context {
        actor: Actor {
            kind: "agent".into(),
            agent_id: r.data["agent_id"].as_str().map(str::to_owned),
            run_id: Some(run_id.into()),
            engine_epoch: epoch.into(),
        },
        task_id: r.data["task_id"].as_str().map(str::to_owned),
        intent_id: r.data["intent_id"].as_str().map(str::to_owned),
        conversation_id: r.data["conversation_id"].as_str().map(str::to_owned),
    };
    authenticate(state, &context)?;
    Ok(context)
}

/// A fresh immutable snapshot is required for every read; inheritance is exactly one hop.
/// A shared trace is a locator and public preview. Raw provider transcripts,
/// signatures, token accounting and blob capabilities stay behind trace views.
pub fn public_activity_data(record: &artex_core::Record) -> Value {
    let mut value = record.data.clone();
    value["detail"] = artex_core::body(record.data["summary"].as_str().unwrap_or(""));
    if let Some(map) = value.as_object_mut() {
        for key in [
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_write_tokens",
            "raw_message",
            "raw_request",
            "raw_response",
        ] {
            map.remove(key);
        }
    }
    if let Some(map) = value["metadata"].as_object_mut() {
        for key in [
            "raw_message",
            "raw_request",
            "raw_response",
            "usage",
            "thinking",
            "signature",
        ] {
            map.remove(key);
        }
    }
    value
}
pub fn shared_context(state: &Snapshot, task_id: &str) -> Result<Value> {
    let task = state
        .get("tasks", task_id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Task not found"))?;
    if !task.data["deleted_at"].is_null() {
        return Err(Error::new("NOT_FOUND", "Task was deleted"));
    }
    let mut ids = vec![task_id.to_owned()];
    let mut relations = state.records("task_relations");
    relations.sort_by(|a, b| {
        a.header
            .created_at
            .cmp(&b.header.created_at)
            .then(a.header.id.cmp(&b.header.id))
    });
    for rel in relations {
        if rel.data["task_id"] == task_id {
            if let Some(id) = rel.data["source_task_id"].as_str() {
                if let Some(source) = state.get("tasks", id) {
                    if source.data["deleted_at"].is_null()
                        && source.data["archived_at"].is_null()
                        && !ids.iter().any(|s| s == id)
                    {
                        ids.push(id.into());
                    }
                }
            }
        }
    }
    let mut nodes = vec![];
    let mut traces = vec![];
    for (index, tid) in ids.iter().enumerate() {
        let exp = &state.get("tasks", tid).unwrap().data["exploration_id"];
        for n in state.records("exploration_nodes") {
            if &n.data["exploration_id"] == exp {
                let mut v = n.data.clone();
                v["source_task_id"] = json!(tid);
                v["inherited"] = json!(index > 0);
                nodes.push(v);
            }
        }
        for a in state.records("activity") {
            if &a.data["exploration_id"] != exp {
                continue;
            }
            if index > 0 {
                let nid = a.data["node_id"].as_str().unwrap_or("");
                let Some(intent) = state.get("exploration_nodes", nid) else {
                    continue;
                };
                if intent.data["kind"] != "intent"
                    || !matches!(
                        intent.data["state"].as_str(),
                        Some("done" | "blocked" | "exhausted" | "stopped")
                    )
                    || matches!(a.data["kind"].as_str(), Some("thinking" | "usage"))
                {
                    continue;
                }
            }
            if matches!(a.data["kind"].as_str(), Some("thinking" | "usage")) {
                continue;
            }
            let mut v = public_activity_data(a);
            v["source_task_id"] = json!(tid);
            v["inherited"] = json!(index > 0);
            if index > 0 {
                for key in [
                    "input_tokens",
                    "output_tokens",
                    "cache_read_tokens",
                    "cache_write_tokens",
                ] {
                    v.as_object_mut().unwrap().remove(key);
                }
            }
            traces.push(v);
        }
    }
    Ok(
        json!({"read_seq":state.commit_seq.to_string(),"task_ids":ids,"nodes":nodes,"traces":traces}),
    )
}

pub fn plan(
    method: &str,
    params: &Value,
    state: &Snapshot,
    context: &Context,
) -> Result<CommandBatch> {
    match method {
        "agent.send" => send(params, state, context),
        "agent.claim" => claim(params, state, context),
        "agent.acknowledge" => acknowledge(params, state, context),
        "agent.cancel_request" => cancel_request(params, state, context),
        "agent.features.set" => features_set(params, state, context),
        "user.answer" => answer(params, state, context),
        _ => Err(Error::new(
            "METHOD_NOT_FOUND",
            "Agent method not implemented",
        )),
    }
}

pub fn query(method: &str, params: &Value, state: &Snapshot, context: &Context) -> Result<Value> {
    match method {
        "agent.features.get" => {
            let id = field(params, "agent_id")?;
            if state.get("agents", id).is_none() {
                return Err(Error::new("NOT_FOUND", "Agent not found"));
            }
            Ok(state
                .records("agent_features")
                .into_iter()
                .find(|r| r.data["agent_id"] == id)
                .map(|r| r.data.clone())
                .unwrap_or_else(
                    || json!({"agent_id":id,"questions":false,"plan_mode":false,"memory":false}),
                ))
        }
        "agent.inbox" => {
            let aid = field(params, "agent_id")?;
            if context.actor.kind == "agent" {
                authenticate(state, context)?;
                if context.actor.agent_id.as_deref() != Some(aid) {
                    return Err(denied("Inbox belongs to another agent"));
                }
            } else if context.actor.kind != "user" {
                return Err(denied("Inbox access denied"));
            }
            let cursor = if let Some(c) = params["cursor"].as_str() {
                let bytes = URL_SAFE_NO_PAD
                    .decode(c)
                    .map_err(|_| invalid("Invalid inbox cursor"))?;
                let c: Value =
                    serde_json::from_slice(&bytes).map_err(|_| invalid("Invalid inbox cursor"))?;
                if c["workspace"] != state.workspace_id
                    || c["agent_id"] != aid
                    || c["epoch"] != context.actor.engine_epoch
                    || c["read_seq"] != state.commit_seq.to_string()
                {
                    return Err(Error::new(
                        "RESYNC_REQUIRED",
                        "Inbox cursor generation changed",
                    ));
                }
                Some(c)
            } else {
                None
            };
            let mut items = state
                .records("agent_deliveries")
                .into_iter()
                .filter(|r| r.data["recipient_id"] == aid)
                .filter(|r| {
                    cursor.as_ref().is_none_or(|c| {
                        (r.header.created_at.as_str(), r.header.id.as_str())
                            > (
                                c["time"].as_str().unwrap_or(""),
                                c["id"].as_str().unwrap_or(""),
                            )
                    })
                })
                .collect::<Vec<_>>();
            items.sort_by(|a, b| {
                a.header
                    .created_at
                    .cmp(&b.header.created_at)
                    .then(a.header.id.cmp(&b.header.id))
            });
            let limit = params["limit"].as_u64().unwrap_or(100).min(100) as usize;
            let more = items.len() > limit;
            items.truncate(limit.max(1));
            let cursor = if more {
                let last = items.last().unwrap();
                Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"workspace":state.workspace_id,"agent_id":aid,"epoch":context.actor.engine_epoch,"read_seq":state.commit_seq.to_string(),"time":last.header.created_at,"id":last.header.id})).unwrap()))
            } else {
                None
            };
            let values = items
                .iter()
                .filter_map(|d| {
                    state
                        .get("agent_messages", d.data["message_id"].as_str()?)
                        .map(|m| json!({"message":m.data,"delivery":d.data}))
                })
                .collect::<Vec<_>>();
            Ok(json!({"items":values,"read_seq":state.commit_seq.to_string(),"next_cursor":cursor}))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown agent query")),
    }
}

fn send(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    authenticate(state, context)?;
    let mut m = params["message"].clone();
    let kind = field(&m, "kind")?.to_owned();
    let mid = field(&m, "message_id")?.to_owned();
    if m["workspace_id"] != state.workspace_id {
        return Err(denied("Message belongs to another workspace"));
    }
    let actual = json!({"kind":"agent","agent_id":context.actor.agent_id,"run_id":context.actor.run_id,"engine_epoch":context.actor.engine_epoch});
    if m["sender"] != actual {
        return Err(denied("Sender must match the authenticated run"));
    }
    m["sender"] = actual;
    if m["protocol"] != "artex-agent/1.0" {
        return Err(invalid("Unsupported agent protocol"));
    }
    let aid = context.actor.agent_id.as_deref().unwrap();
    let recipients = m["recipients"]
        .as_array()
        .ok_or_else(|| invalid("Recipients must be an array"))?;
    if recipients.is_empty() || recipients.len() > 64 {
        return Err(invalid("Recipients must contain 1..64 agents"));
    }
    let mut distinct = HashSet::new();
    for id in recipients {
        let id = id
            .as_str()
            .ok_or_else(|| invalid("Recipient must be an agent ID"))?;
        if !distinct.insert(id.to_owned()) {
            return Err(invalid("Duplicate recipient"));
        }
        let r = state
            .get("agents", id)
            .ok_or_else(|| Error::new("NOT_FOUND", "Recipient not found"))?;
        if r.data["enabled"] != true {
            return Err(denied("Recipient is disabled"));
        }
        if kind == "request" && id == aid {
            return Err(Error::new("REQUEST_CYCLE", "Self request is forbidden"));
        }
    }
    let channel = field(&m, "channel")?;
    let ws = format!("workspace/{}", state.workspace_id);
    if channel != format!("{ws}/agents")
        && context
            .task_id
            .as_ref()
            .is_none_or(|tid| channel != format!("{ws}/task/{tid}"))
    {
        return Err(denied("Channel is outside the run scope"));
    }
    let topic = field(&m, "topic")?;
    if topic.is_empty()
        || topic.chars().count() > 128
        || m["hop_count"].as_u64().is_none_or(|n| n > 16)
    {
        return Err(invalid("Topic or hop count is invalid"));
    }
    if let Some(text) = m["body"]["text"].as_str() {
        if text.len() > 65536 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Message text must use a blob above 64 KiB",
            ));
        }
    }
    for obj in m["object_refs"]
        .as_array()
        .ok_or_else(|| invalid("object_refs must be an array"))?
    {
        let record = state
            .get(field(obj, "collection")?, field(obj, "id")?)
            .ok_or_else(|| Error::new("NOT_FOUND", "Referenced object not found"))?;
        if obj["version"] != record.header.version {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Object reference version is stale",
            ));
        }
        validate_object_scope(state, context, record)?;
    }
    let mut b = batch("agent.send", params);
    if let Some(refs) = m["object_refs"].as_array() {
        for obj in refs {
            let r = state
                .get(field(obj, "collection")?, field(obj, "id")?)
                .unwrap();
            if !b
                .expected
                .iter()
                .any(|e| e.collection == r.header.collection && e.id == r.header.id)
            {
                b.expected.push(Expected {
                    collection: r.header.collection.clone(),
                    id: r.header.id.clone(),
                    version: r.header.version.clone(),
                });
            }
        }
        if !refs.is_empty() {
            if let Some(tid) = context.task_id.as_deref() {
                for r in state
                    .records("task_relations")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == tid)
                    .chain(
                        artex_domain::direct_sources(state, tid)
                            .iter()
                            .filter_map(|id| state.get("tasks", id))
                            .collect::<Vec<_>>(),
                    )
                {
                    if !b
                        .expected
                        .iter()
                        .any(|e| e.collection == r.header.collection && e.id == r.header.id)
                    {
                        b.expected.push(Expected {
                            collection: r.header.collection.clone(),
                            id: r.header.id.clone(),
                            version: r.header.version.clone(),
                        });
                    }
                }
            }
        }
    }
    for (collection, id) in [
        ("agents", context.actor.agent_id.as_deref().unwrap()),
        ("agent_runs", context.actor.run_id.as_deref().unwrap()),
    ] {
        let record = state.get(collection, id).unwrap();
        if !b
            .expected
            .iter()
            .any(|e| e.collection == collection && e.id == id)
        {
            b.expected.push(Expected {
                collection: collection.into(),
                id: id.into(),
                version: record.header.version.clone(),
            });
        }
    }
    for recipient in recipients {
        let id = recipient.as_str().unwrap();
        let record = state.get("agents", id).unwrap();
        if !b
            .expected
            .iter()
            .any(|e| e.collection == "agents" && e.id == id)
        {
            b.expected.push(Expected {
                collection: "agents".into(),
                id: id.into(),
                version: record.header.version.clone(),
            });
        }
    }
    if let Some(old) = state.get("agent_messages", &mid) {
        if old.data != m {
            return Err(Error::new(
                "COMMAND_ID_REUSE",
                "Message ID already has another payload",
            ));
        }
        if kind == "request" {
            let request = state
                .get("conversation_requests", &mid)
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Request state is missing"))?;
            let mode = params["request_options"]["completion_mode"]
                .as_str()
                .unwrap_or("one");
            let reports = params["request_options"]["expected_reports"]
                .as_array()
                .unwrap_or(recipients);
            if request.data["completion_mode"] != mode
                || request.data["expected_reports"] != json!(reports)
            {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Request options were changed after registration",
                ));
            }
        } else if !params["request_options"].is_null() {
            return Err(invalid("request_options are only valid for a request"));
        }
        let ids = state
            .records("agent_deliveries")
            .into_iter()
            .filter(|r| r.data["message_id"] == mid)
            .map(|r| r.header.id.clone())
            .collect::<Vec<_>>();
        b.result = json!({"message_id":mid,"delivery_ids":ids,"request_id":if kind=="request"{json!(mid)}else{Value::Null}});
        return Ok(b);
    }
    let correlation = m["correlation_id"].as_str().map(str::to_owned);
    if matches!(kind.as_str(), "send" | "request" | "broadcast") {
        if correlation.is_some() || m["final"] != false || m["outcome"] != "none" {
            return Err(invalid("New messages cannot be terminal or correlated"));
        }
    } else if kind == "receipt" {
        let original = state
            .get(
                "agent_messages",
                correlation
                    .as_deref()
                    .ok_or_else(|| invalid("Receipt needs correlation_id"))?,
            )
            .ok_or_else(|| Error::new("NOT_FOUND", "Original message not found"))?;
        if !original.data["recipients"]
            .as_array()
            .is_some_and(|r| r.iter().any(|r| r == aid))
            || m["final"] != false
            || m["outcome"] != "none"
        {
            return Err(denied("Invalid receipt"));
        }
    } else if matches!(kind.as_str(), "reply" | "report") {
        let cid = correlation
            .as_deref()
            .ok_or_else(|| invalid("Reply needs correlation_id"))?;
        let request = state
            .get("conversation_requests", cid)
            .ok_or_else(|| Error::new("NOT_FOUND", "Request not found"))?;
        if !request.data["recipients"]
            .as_array()
            .is_some_and(|r| r.iter().any(|r| r == aid))
            || recipients.len() != 1
            || recipients[0] != request.data["sender_id"]
        {
            return Err(denied("Reply is not from a registered recipient"));
        }
        if m["final"] == true {
            if !matches!(m["outcome"].as_str(), Some("ok" | "error" | "cancelled")) {
                return Err(invalid("Final reply requires a terminal outcome"));
            }
            if let Some(previous) = request.data["terminal_reports"][aid].as_str() {
                let old = state
                    .get("agent_messages", previous)
                    .ok_or_else(|| Error::new("CORRUPT_STORE", "Terminal reply not found"))?;
                if [
                    "body",
                    "outcome",
                    "object_refs",
                    "kind",
                    "recipients",
                    "channel",
                    "topic",
                    "correlation_id",
                    "deadline_at",
                    "hop_count",
                    "final",
                ]
                .iter()
                .any(|key| old.data[*key] != m[*key])
                {
                    return Err(Error::new(
                        "ALREADY_COMPLETED",
                        "Agent already sent a different final reply",
                    ));
                }
                b.result = json!({"message_id":previous,"delivery_ids":[],"request_id":cid});
                return Ok(b);
            }
            if matches!(
                request.data["state"].as_str(),
                Some("open" | "claimed" | "waiting")
            ) {
                let deadline = parse_time(field(&request.data, "deadline_at")?)?;
                if deadline <= Utc::now() {
                    return Err(Error::new("TIMEOUT", "Request deadline elapsed"));
                }
                let mut r = request.data.clone();
                r["terminal_reports"][aid] = json!(mid);
                let complete = r["completion_mode"] == "one"
                    || r["expected_reports"].as_array().unwrap().iter().all(|v| {
                        v.as_str()
                            .is_some_and(|id| r["terminal_reports"].get(id).is_some())
                    });
                if complete {
                    let mut outcome = m["outcome"].as_str().unwrap();
                    if r["completion_mode"] == "all" {
                        for expected in r["expected_reports"].as_array().unwrap() {
                            let recipient = expected.as_str().unwrap();
                            if recipient == aid {
                                continue;
                            }
                            if let Some(old) = r["terminal_reports"][recipient]
                                .as_str()
                                .and_then(|id| state.get("agent_messages", id))
                            {
                                if old.data["outcome"] == "error" {
                                    outcome = "error";
                                    break;
                                }
                                if old.data["outcome"] == "cancelled" && outcome != "error" {
                                    outcome = "cancelled";
                                }
                            }
                        }
                    }
                    r["state"] = json!(match outcome {
                        "ok" => "completed",
                        "error" => "failed",
                        _ => "cancelled",
                    });
                } else {
                    r["state"] = json!("waiting");
                }
                put(&mut b, state, "conversation_requests", cid, r.clone());
                if complete {
                    resume_sender(&mut b, state, cid, &context.actor.engine_epoch);
                }
            }
            for delivery in state.records("agent_deliveries") {
                if delivery.data["message_id"] == cid
                    && delivery.data["recipient_id"] == aid
                    && matches!(
                        delivery.data["state"].as_str(),
                        Some("pending" | "claimed" | "observed" | "interrupted")
                    )
                {
                    let mut data = delivery.data.clone();
                    data["state"] = json!("completed");
                    data["owner"] = json!(context.actor);
                    data["claim_deadline"] = Value::Null;
                    data["completed_at"] = json!(now());
                    put(&mut b, state, "agent_deliveries", &delivery.header.id, data);
                }
            }
            // Late terminal replies remain diagnostics and cannot change a closed request.
        } else if m["outcome"] != "none" {
            return Err(invalid("Progress report cannot have a terminal outcome"));
        }
    } else {
        return Err(invalid("Unknown message kind"));
    }
    if kind == "request" {
        let deadline = parse_time(field(&m, "deadline_at")?)?;
        let remaining = deadline
            .signed_duration_since(Utc::now())
            .num_milliseconds();
        if !(1_000..=3_600_000).contains(&remaining) {
            return Err(invalid(
                "Request deadline must be between one second and one hour",
            ));
        }
        if let Some(run) = context
            .actor
            .run_id
            .as_deref()
            .and_then(|id| state.get("agent_runs", id))
        {
            if let Some(parent) = run.data["deadline_at"].as_str() {
                if deadline > parse_time(parent)? {
                    return Err(invalid("Request deadline exceeds run deadline"));
                }
            }
        }
        for recipient in &distinct {
            if request_path(state, recipient, aid, &mut BTreeSet::new()) {
                return Err(Error::new(
                    "REQUEST_CYCLE",
                    "Request creates a wait-for cycle",
                ));
            }
        }
        let options = &params["request_options"];
        let mode = options["completion_mode"].as_str().unwrap_or("one");
        if !matches!(mode, "one" | "all") {
            return Err(invalid("Invalid completion mode"));
        }
        let reports = options["expected_reports"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| recipients.clone());
        let report_ids = reports
            .iter()
            .map(|v| v.as_str().unwrap_or(""))
            .collect::<HashSet<_>>();
        if reports.is_empty()
            || report_ids.len() != reports.len()
            || report_ids.iter().any(|id| !distinct.contains(*id))
        {
            return Err(invalid(
                "Expected reports must be a unique recipient subset",
            ));
        }
        put(
            &mut b,
            state,
            "conversation_requests",
            &mid,
            json!({"id":mid,"sender_id":aid,"recipients":recipients,"expected_reports":reports,"state":"open","terminal_reports":{},"deadline_at":m["deadline_at"],"completion_mode":mode}),
        );
        let rid = context.actor.run_id.as_deref().unwrap();
        let mut run = state.get("agent_runs", rid).unwrap().data.clone();
        run["state"] = json!("waiting");
        put(&mut b, state, "agent_runs", rid, run);
    } else if !params["request_options"].is_null() {
        return Err(invalid("request_options are only valid for a request"));
    }
    put(&mut b, state, "agent_messages", &mid, m.clone());
    let mut delivery_ids = vec![];
    for recipient in recipients {
        let id = new_id("delivery");
        put(
            &mut b,
            state,
            "agent_deliveries",
            &id,
            json!({"id":id,"event_id":null,"message_id":mid,"recipient_id":recipient,"activation_key":format!("message:{mid}:{}",recipient.as_str().unwrap()),"state":"pending","owner":null,"claim_deadline":null,"attempts":0,"observed_at":null,"completed_at":null}),
        );
        delivery_ids.push(id);
    }
    b.result = json!({"message_id":mid,"delivery_ids":delivery_ids,"request_id":if kind=="request"{json!(mid)}else{Value::Null}});
    event(
        &mut b,
        state,
        context,
        "AgentMessageSent",
        "agent_messages",
        &mid,
        "1",
    )?;
    Ok(b)
}
pub fn validate_object_scope(
    state: &Snapshot,
    context: &Context,
    record: &artex_core::Record,
) -> Result<()> {
    let collection = record.header.collection.as_str();
    if collection == "memory_records" || collection == "agent_features" {
        if record.data["agent_id"].as_str() == context.actor.agent_id.as_deref() {
            return Ok(());
        }
        return Err(denied("Object reference belongs to another agent"));
    }
    if matches!(
        collection,
        "command_receipts"
            | "agent_deliveries"
            | "user_questions"
            | "tool_invocations"
            | "domain_events"
            | "agent_messages"
            | "conversation_requests"
    ) {
        return Err(denied(
            "Private runtime records cannot be shared as object references",
        ));
    }
    let task = context.task_id.as_deref();
    let tasks = task
        .map(|id| artex_domain::direct_sources(state, id))
        .unwrap_or_default();
    let explorations = task
        .map(|id| artex_domain::visible_explorations(state, id))
        .unwrap_or_default();
    let owner = if collection == "tasks" {
        Some(record.header.id.as_str())
    } else {
        record.data["task_id"].as_str()
    };
    if let Some(owner) = owner {
        if !tasks.contains(owner) {
            return Err(denied("Object reference is outside direct shared context"));
        }
        return Ok(());
    }
    let exploration = if collection == "explorations" {
        Some(record.header.id.as_str())
    } else {
        record.data["exploration_id"].as_str()
    };
    if let Some(exploration) = exploration {
        if !explorations.contains(exploration) {
            return Err(denied("Object reference is outside direct shared context"));
        }
        if collection == "activity"
            && task
                .and_then(|tid| state.get("tasks", tid))
                .is_none_or(|t| t.data["exploration_id"] != exploration)
        {
            let intent = record.data["node_id"]
                .as_str()
                .and_then(|id| state.get("exploration_nodes", id));
            if matches!(record.data["kind"].as_str(), Some("thinking" | "usage"))
                || intent.is_none_or(|n| {
                    n.data["kind"] != "intent"
                        || !matches!(
                            n.data["state"].as_str(),
                            Some("done" | "blocked" | "exhausted" | "stopped")
                        )
                })
            {
                return Err(denied(
                    "Inherited execution record is private or not terminal",
                ));
            }
        }
        return Ok(());
    }
    for (key, target) in [
        ("node_id", "exploration_nodes"),
        ("intent_id", "exploration_nodes"),
        ("finding_id", "findings"),
    ] {
        if let Some(id) = record.data[key].as_str() {
            return validate_object_scope(
                state,
                context,
                state.get(target, id).ok_or_else(|| {
                    Error::new("NOT_FOUND", "Referenced object's owner is missing")
                })?,
            );
        }
    }
    if matches!(
        collection,
        "assets"
            | "companies"
            | "company_scope"
            | "agents"
            | "tools"
            | "llm_profiles"
            | "task_categories"
    ) {
        return Ok(());
    }
    Err(denied("Object reference has no readable scope"))
}
fn resume_sender(b: &mut CommandBatch, state: &Snapshot, request_id: &str, epoch: &str) {
    let Some(message) = state.get("agent_messages", request_id) else {
        return;
    };
    let Some(rid) = message.data["sender"]["run_id"].as_str() else {
        return;
    };
    resume_run(b, state, rid, epoch);
}
fn updated<'a>(b: &'a CommandBatch, record: &'a artex_core::Record) -> &'a Value {
    b.changes
        .iter()
        .find_map(|c| match c {
            Change::Put {
                collection,
                id,
                data,
            } if collection == &record.header.collection && id == &record.header.id => Some(data),
            _ => None,
        })
        .unwrap_or(&record.data)
}
fn resume_run(b: &mut CommandBatch, state: &Snapshot, rid: &str, epoch: &str) {
    let Some(run) = state.get("agent_runs", rid) else {
        return;
    };
    let run_data = updated(b, run);
    if run_data["state"] != "waiting" || run_data["engine_epoch"] != epoch {
        return;
    }
    let another = state
        .records("conversation_requests")
        .into_iter()
        .any(|request| {
            matches!(
                updated(b, request)["state"].as_str(),
                Some("open" | "claimed" | "waiting")
            ) && state
                .get("agent_messages", &request.header.id)
                .is_some_and(|m| m.data["sender"]["run_id"] == rid)
        });
    let pending_question = state
        .records("user_questions")
        .into_iter()
        .any(|q| q.data["run_id"] == rid && updated(b, q)["state"] == "pending");
    if !another && !pending_question {
        let mut data = run_data.clone();
        data["state"] = json!("running");
        put(b, state, "agent_runs", rid, data);
    }
}
fn event(
    b: &mut CommandBatch,
    state: &Snapshot,
    context: &Context,
    kind: &str,
    collection: &str,
    id: &str,
    version: &str,
) -> Result<()> {
    let seq = state
        .records("domain_events")
        .into_iter()
        .filter_map(|r| r.data["seq"].as_str().and_then(|s| s.parse::<u64>().ok()))
        .max()
        .unwrap_or(0)
        .checked_add(b.events.len() as u64 + 1)
        .ok_or_else(|| Error::new("SIZE_LIMIT", "Event sequence exhausted"))?;
    let eid = new_id("event");
    let stamp = now();
    b.events.push(artex_core::Record{header:artex_core::Header{id:eid.clone(),collection:"domain_events".into(),version:"1".into(),commit_seq:(state.commit_seq+1).to_string(),created_at:stamp.clone(),updated_at:stamp.clone(),legacy_key:Value::Null},data:json!({"id":eid,"seq":seq.to_string(),"kind":kind,"task_id":context.task_id,"subject_collection":collection,"subject_id":id,"subject_version":version,"actor":context.actor,"created_at":stamp})});
    Ok(())
}
fn request_path(state: &Snapshot, from: &str, target: &str, seen: &mut BTreeSet<String>) -> bool {
    if from == target {
        return true;
    }
    if !seen.insert(from.into()) {
        return false;
    }
    for r in state.records("conversation_requests") {
        if r.data["sender_id"] == from
            && matches!(
                r.data["state"].as_str(),
                Some("open" | "claimed" | "waiting")
            )
            && r.data["deadline_at"]
                .as_str()
                .and_then(|t| parse_time(t).ok())
                .is_some_and(|t| t > Utc::now())
        {
            if r.data["recipients"].as_array().is_some_and(|ids| {
                ids.iter().any(|id| {
                    id.as_str()
                        .is_some_and(|id| request_path(state, id, target, seen))
                })
            }) {
                return true;
            }
        }
    }
    false
}
fn parse_time(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| invalid("Invalid timestamp"))
}

fn claim(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    authenticate(state, context)?;
    let id = field(params, "delivery_id")?;
    let rid = field(params, "run_id")?;
    if context.actor.run_id.as_deref() != Some(rid) {
        return Err(denied("Claim run is not authenticated"));
    }
    let r = state
        .get("agent_deliveries", id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Delivery not found"))?;
    if r.data["recipient_id"].as_str() != context.actor.agent_id.as_deref() {
        return Err(denied("Delivery belongs to another agent"));
    }
    let renewable = r.data["owner"] == serde_json::to_value(&context.actor).unwrap_or(Value::Null);
    let expired = r.data["claim_deadline"]
        .as_str()
        .and_then(|s| parse_time(s).ok())
        .is_some_and(|t| t <= Utc::now());
    if !matches!(r.data["state"].as_str(), Some("pending" | "interrupted"))
        && !(matches!(r.data["state"].as_str(), Some("claimed" | "observed"))
            && (expired || renewable))
    {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Delivery is already claimed or terminal",
        ));
    }
    if let Some(mid) = r.data["message_id"].as_str() {
        if let Some(m) = state.get("agent_messages", mid) {
            if m.data["deadline_at"]
                .as_str()
                .and_then(|s| parse_time(s).ok())
                .is_some_and(|t| t <= Utc::now())
            {
                return Err(Error::new("TIMEOUT", "Message expired"));
            }
        }
    }
    let mut b = batch("agent.claim", params);
    let mut d = r.data.clone();
    d["state"] = if renewable && r.data["state"] == "observed" {
        json!("observed")
    } else {
        json!("claimed")
    };
    d["owner"] = serde_json::to_value(&context.actor).map_err(|_| invalid("Invalid actor"))?;
    d["claim_deadline"] = json!(
        (Utc::now() + Duration::seconds(30)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    if !renewable {
        d["attempts"] = json!(d["attempts"].as_u64().unwrap_or(0) + 1);
    }
    b.result = json!({"claimed":true,"owner":d["owner"],"claim_deadline":d["claim_deadline"]});
    put(&mut b, state, "agent_deliveries", id, d);
    Ok(b)
}
fn acknowledge(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    authenticate(state, context)?;
    let id = field(params, "delivery_id")?;
    let r = state
        .get("agent_deliveries", id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Delivery not found"))?;
    if context.actor.run_id.as_deref() != Some(field(params, "run_id")?)
        || r.data["owner"] != serde_json::to_value(&context.actor).unwrap_or(Value::Null)
        || !matches!(r.data["state"].as_str(), Some("claimed" | "observed"))
    {
        return Err(denied("Claim ownership no longer matches"));
    }
    if r.data["claim_deadline"]
        .as_str()
        .and_then(|s| parse_time(s).ok())
        .is_none_or(|t| t <= Utc::now())
    {
        return Err(Error::new("VERSION_CONFLICT", "Claim lease expired"));
    }
    let action = field(params, "action")?;
    if !matches!(action, "observed" | "completed") {
        return Err(invalid("Invalid acknowledge action"));
    }
    if action == "completed" {
        if let Some(mid) = r.data["message_id"].as_str() {
            if state
                .get("agent_messages", mid)
                .is_some_and(|m| m.data["kind"] == "request")
            {
                return Err(invalid(
                    "A request requires a reply; observation is not completion",
                ));
            }
        }
    }
    let mut b = batch("agent.acknowledge", params);
    let mut d = r.data.clone();
    d["state"] = json!(action);
    d[if action == "completed" {
        "completed_at"
    } else {
        "observed_at"
    }] = json!(now());
    b.result = json!({"state":action});
    put(&mut b, state, "agent_deliveries", id, d);
    Ok(b)
}
fn cancel_request(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    authenticate(state, context)?;
    let id = field(params, "message_id")?;
    let r = state
        .get("conversation_requests", id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Request not found"))?;
    if r.data["sender_id"].as_str() != context.actor.agent_id.as_deref() {
        return Err(denied("Request belongs to another sender"));
    }
    let mut b = batch("agent.cancel_request", params);
    if matches!(
        r.data["state"].as_str(),
        Some("open" | "claimed" | "waiting")
    ) {
        let mut d = r.data.clone();
        d["state"] = json!("cancelled");
        put(&mut b, state, "conversation_requests", id, d);
        for d in state.records("agent_deliveries") {
            if d.data["message_id"] == id
                && matches!(
                    d.data["state"].as_str(),
                    Some("pending" | "claimed" | "observed" | "interrupted")
                )
            {
                let mut value = d.data.clone();
                value["state"] = json!("cancelled");
                value["claim_deadline"] = Value::Null;
                value["completed_at"] = json!(now());
                put(&mut b, state, "agent_deliveries", &d.header.id, value);
            }
        }
        b.result = json!({"state":"cancelled"});
        resume_sender(&mut b, state, id, &context.actor.engine_epoch);
    } else {
        b.result = json!({"state":r.data["state"]});
    }
    Ok(b)
}
fn features_set(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    if context.actor.kind != "user" {
        return Err(denied("Only the user can opt in to agent features"));
    }
    let aid = field(params, "agent_id")?;
    if state.get("agents", aid).is_none() {
        return Err(Error::new("NOT_FOUND", "Agent not found"));
    }
    for key in ["questions", "plan_mode", "memory"] {
        if !params[key].is_boolean() {
            return Err(invalid("Feature flags must be booleans"));
        }
    }
    let id = state
        .records("agent_features")
        .into_iter()
        .find(|r| r.data["agent_id"] == aid)
        .map(|r| r.header.id.clone())
        .unwrap_or_else(|| new_id("agentfeature"));
    let mut b = batch("agent.features.set", params);
    b.result = params.clone();
    put(&mut b, state, "agent_features", &id, params.clone());
    Ok(b)
}
fn answer(params: &Value, state: &Snapshot, context: &Context) -> Result<CommandBatch> {
    if context.actor.kind != "user" {
        return Err(denied("Only the user can answer a question"));
    }
    let id = field(params, "question_id")?;
    let q = state
        .get("user_questions", id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Question not found"))?;
    if q.data["state"] != "pending" || q.data["generation"] != params["generation"] {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Question state or generation changed",
        ));
    }
    if parse_time(field(&q.data, "deadline_at")?)? <= Utc::now() {
        return Err(Error::new("TIMEOUT", "Question expired"));
    }
    let rid = field(&q.data, "run_id")?;
    let run = state
        .get("agent_runs", rid)
        .ok_or_else(|| Error::new("NOT_FOUND", "Question run not found"))?;
    if !matches!(run.data["state"].as_str(), Some("running" | "waiting"))
        || run.data["engine_epoch"] != context.actor.engine_epoch
    {
        return Err(Error::new(
            "STALE_EPOCH",
            "Question belongs to an inactive run",
        ));
    }
    let mut b = batch("user.answer", params);
    let mut d = q.data.clone();
    d["state"] = json!("answered");
    d["answer"] = params["answer"].clone();
    put(&mut b, state, "user_questions", id, d);
    if q.data["kind"] == "plan_approval" {
        let mut r = run.data.clone();
        if params["answer"]["approved"] == true {
            let mode = r["pre_plan_mode"].as_str().unwrap_or("default").to_owned();
            r["permission_mode"] = json!(mode);
            r["approved_target_mode"] = json!(mode);
            r["pre_plan_mode"] = Value::Null;
        }
        put(&mut b, state, "agent_runs", rid, r);
    }
    resume_run(&mut b, state, rid, &context.actor.engine_epoch);
    b.result = json!({"accepted":true,"state":"answered"});
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Header, Record, default_data};
    use std::collections::BTreeMap;
    fn insert(s: &mut Snapshot, c: &str, id: &str, data: Value) {
        let stamp = now();
        s.collections.entry(c.into()).or_default().insert(
            id.into(),
            Record {
                header: Header {
                    id: id.into(),
                    collection: c.into(),
                    version: "1".into(),
                    commit_seq: s.commit_seq.to_string(),
                    created_at: stamp.clone(),
                    updated_at: stamp,
                    legacy_key: Value::Null,
                },
                data,
            },
        );
    }
    fn apply(s: &mut Snapshot, b: CommandBatch) {
        s.commit_seq += 1;
        for ch in b.changes {
            match ch {
                Change::Put {
                    collection,
                    id,
                    data,
                } => insert(s, &collection, &id, data),
                Change::Delete { collection, id } => {
                    s.collections.get_mut(&collection).unwrap().remove(&id);
                }
            }
        }
    }
    fn fixture() -> (Snapshot, Vec<Context>) {
        let mut s = Snapshot {
            workspace_id: new_id("workspace"),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        let epoch = new_id("epoch");
        let mut contexts = vec![];
        for name in ["sender", "first", "second"] {
            let aid = new_id("agent");
            let rid = new_id("run");
            let mut a = default_data("agents").unwrap();
            a["id"] = json!(aid);
            a["key"] = json!(name);
            a["enabled"] = json!(true);
            insert(&mut s, "agents", &aid, a);
            let mut r = default_data("agent_runs").unwrap();
            r["id"] = json!(rid);
            r["agent_id"] = json!(aid);
            r["engine_epoch"] = json!(epoch);
            r["state"] = json!("running");
            insert(&mut s, "agent_runs", &rid, r);
            contexts.push(Context {
                actor: Actor {
                    kind: "agent".into(),
                    agent_id: Some(aid),
                    run_id: Some(rid),
                    engine_epoch: epoch.clone(),
                },
                task_id: None,
                intent_id: None,
                conversation_id: None,
            });
        }
        (s, contexts)
    }
    fn message(
        s: &Snapshot,
        c: &Context,
        recipients: Vec<String>,
        kind: &str,
        correlation: Option<&str>,
        final_reply: bool,
    ) -> Value {
        json!({"message":{"protocol":"artex-agent/1.0","message_id":new_id("message"),"command_id":new_id("command"),"workspace_id":s.workspace_id,"kind":kind,"sender":c.actor,"recipients":recipients,"channel":format!("workspace/{}/agents",s.workspace_id),"topic":"test","correlation_id":correlation,"created_at":now(),"deadline_at":if kind=="request"{json!((Utc::now()+Duration::seconds(120)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true))}else{Value::Null},"body":artex_core::body("test"),"object_refs":[],"final":final_reply,"outcome":if final_reply{"ok"}else{"none"},"hop_count":0},"request_options":null})
    }
    #[test]
    fn all_requires_terminal_reports_and_progress_is_not_completion() {
        let (mut s, c) = fixture();
        let ids = c[1..]
            .iter()
            .map(|c| c.actor.agent_id.clone().unwrap())
            .collect::<Vec<_>>();
        let mut p = message(&s, &c[0], ids.clone(), "request", None, false);
        p["request_options"] = json!({"completion_mode":"all","expected_reports":ids});
        let mid = p["message"]["message_id"].as_str().unwrap().to_owned();
        let b = send(&p, &s, &c[0]).unwrap();
        apply(&mut s, b);
        let target = vec![c[0].actor.agent_id.clone().unwrap()];
        let p = message(&s, &c[1], target.clone(), "report", Some(&mid), false);
        let b = send(&p, &s, &c[1]).unwrap();
        apply(&mut s, b);
        assert_eq!(
            s.get("conversation_requests", &mid).unwrap().data["state"],
            "open"
        );
        let p = message(&s, &c[1], target.clone(), "reply", Some(&mid), true);
        let b = send(&p, &s, &c[1]).unwrap();
        apply(&mut s, b);
        assert_eq!(
            s.get("conversation_requests", &mid).unwrap().data["state"],
            "waiting"
        );
        let p = message(&s, &c[2], target, "reply", Some(&mid), true);
        let b = send(&p, &s, &c[2]).unwrap();
        apply(&mut s, b);
        assert_eq!(
            s.get("conversation_requests", &mid).unwrap().data["state"],
            "completed"
        );
    }
    #[test]
    fn forged_sender_and_wait_cycles_are_rejected() {
        let (mut s, c) = fixture();
        let mut p = message(
            &s,
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        p["message"]["sender"] = json!(c[1].actor);
        assert_eq!(send(&p, &s, &c[0]).unwrap_err().code, "PERMISSION_DENIED");
        p["message"]["sender"] = json!(c[0].actor);
        let b = send(&p, &s, &c[0]).unwrap();
        apply(&mut s, b);
        let p = message(
            &s,
            &c[1],
            vec![c[0].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        assert_eq!(send(&p, &s, &c[1]).unwrap_err().code, "REQUEST_CYCLE");
    }
    #[test]
    fn claim_is_bound_to_owner_and_observation_does_not_complete_request() {
        let (mut s, c) = fixture();
        let p = message(
            &s,
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        let b = send(&p, &s, &c[0]).unwrap();
        let did = b.result["delivery_ids"][0].as_str().unwrap().to_owned();
        apply(&mut s, b);
        let p = json!({"delivery_id":did,"run_id":c[1].actor.run_id});
        let b = claim(&p, &s, &c[1]).unwrap();
        apply(&mut s, b);
        let observed = json!({"delivery_id":did,"run_id":c[1].actor.run_id,"action":"observed"});
        assert!(acknowledge(&observed, &s, &c[1]).is_ok());
        let completed = json!({"delivery_id":did,"run_id":c[1].actor.run_id,"action":"completed"});
        assert_eq!(
            acknowledge(&completed, &s, &c[1]).unwrap_err().code,
            "INVALID_PARAMS"
        );
        assert!(
            claim(
                &json!({"delivery_id":did,"run_id":c[2].actor.run_id}),
                &s,
                &c[2]
            )
            .is_err()
        );
    }
    #[test]
    fn direct_context_never_recurses_and_source_change_is_visible() {
        let (mut s, _) = fixture();
        let mut tids = vec![];
        for _ in 0..3 {
            let tid = new_id("task");
            let exp = new_id("exploration");
            let mut t = default_data("tasks").unwrap();
            t["id"] = json!(tid);
            t["exploration_id"] = json!(exp);
            insert(&mut s, "tasks", &tid, t);
            let nid = new_id("node");
            let mut n = default_data("exploration_nodes").unwrap();
            n["id"] = json!(nid);
            n["exploration_id"] = json!(exp);
            n["kind"] = json!("fact");
            n["state"] = json!("confirmed");
            insert(&mut s, "exploration_nodes", &nid, n);
            tids.push(tid);
        }
        for (a, b) in [(0, 1), (1, 2)] {
            let rid = new_id("relation");
            insert(
                &mut s,
                "task_relations",
                &rid,
                json!({"task_id":tids[a],"source_task_id":tids[b],"created_at":now()}),
            );
        }
        let view = shared_context(&s, &tids[0]).unwrap();
        assert_eq!(view["task_ids"].as_array().unwrap().len(), 2);
        assert_eq!(view["nodes"].as_array().unwrap().len(), 2);
        s.collections
            .get_mut("task_relations")
            .unwrap()
            .retain(|_, r| r.data["task_id"] != tids[0]);
        assert_eq!(
            shared_context(&s, &tids[0]).unwrap()["nodes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn final_reply_consumes_delivery_and_resumes_sender_after_all_waits() {
        let (mut s, c) = fixture();
        let recipient = c[1].actor.agent_id.clone().unwrap();
        let mut mids = vec![];
        for _ in 0..2 {
            let p = message(&s, &c[0], vec![recipient.clone()], "request", None, false);
            mids.push(p["message"]["message_id"].as_str().unwrap().to_owned());
            let b = send(&p, &s, &c[0]).unwrap();
            apply(&mut s, b);
        }
        for (index, mid) in mids.iter().enumerate() {
            let p = message(
                &s,
                &c[1],
                vec![c[0].actor.agent_id.clone().unwrap()],
                "reply",
                Some(mid),
                true,
            );
            let b = send(&p, &s, &c[1]).unwrap();
            apply(&mut s, b);
            assert_eq!(
                s.get("agent_runs", c[0].actor.run_id.as_ref().unwrap())
                    .unwrap()
                    .data["state"],
                if index == 0 { "waiting" } else { "running" }
            );
            assert!(
                s.records("agent_deliveries")
                    .iter()
                    .filter(|r| r.data["message_id"] == mid.as_str())
                    .all(|r| r.data["state"] == "completed" && r.data["claim_deadline"].is_null())
            );
        }
    }
    #[test]
    fn expiration_and_cancellation_are_atomic_and_preserve_other_waits() {
        let (mut s, c) = fixture();
        let p = message(
            &s,
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        let mid = p["message"]["message_id"].as_str().unwrap().to_owned();
        let b = send(&p, &s, &c[0]).unwrap();
        apply(&mut s, b);
        s.collections
            .get_mut("conversation_requests")
            .unwrap()
            .get_mut(&mid)
            .unwrap()
            .data["deadline_at"] = json!(
            (Utc::now() - Duration::seconds(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
        let b = maintenance_plan(&s, &c[0].actor.engine_epoch).unwrap();
        apply(&mut s, b);
        assert_eq!(
            s.get("conversation_requests", &mid).unwrap().data["state"],
            "expired"
        );
        assert!(
            s.records("agent_deliveries")
                .iter()
                .filter(|r| r.data["message_id"] == mid)
                .all(|r| r.data["state"] == "expired")
        );
        assert_eq!(
            s.get("agent_runs", c[0].actor.run_id.as_ref().unwrap())
                .unwrap()
                .data["state"],
            "running"
        );
        assert!(maintenance_plan(&s, &c[0].actor.engine_epoch).is_none());
        let p = message(
            &s,
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        let mid = p["message"]["message_id"].as_str().unwrap().to_owned();
        let b = send(&p, &s, &c[0]).unwrap();
        apply(&mut s, b);
        let b = cancel_request(&json!({"message_id":mid}), &s, &c[0]).unwrap();
        apply(&mut s, b);
        assert_eq!(
            s.get("agent_runs", c[0].actor.run_id.as_ref().unwrap())
                .unwrap()
                .data["state"],
            "running"
        );
    }
    #[test]
    fn protocol_records_and_terminal_status_survive_real_journal_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = artex_store::Store::create(directory.path(), "protocol").unwrap();
        let (mut initial, c) = fixture();
        initial.workspace_id = store.snapshot().workspace_id.clone();
        let mut seed = batch("test.seed", &Value::Null);
        for records in initial.collections.values() {
            for r in records.values() {
                seed.changes.push(Change::Put {
                    collection: r.header.collection.clone(),
                    id: r.header.id.clone(),
                    data: r.data.clone(),
                });
            }
        }
        store.commit(seed).unwrap();
        let p = message(
            &store.snapshot(),
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        let mid = p["message"]["message_id"].as_str().unwrap().to_owned();
        let b = send(&p, &store.snapshot(), &c[0]).unwrap();
        store.commit(b).unwrap();
        let p = message(
            &store.snapshot(),
            &c[1],
            vec![c[0].actor.agent_id.clone().unwrap()],
            "reply",
            Some(&mid),
            true,
        );
        let b = send(&p, &store.snapshot(), &c[1]).unwrap();
        store.commit(b).unwrap();
        assert_eq!(store.snapshot().records("domain_events").len(), 2);
        store.checkpoint().unwrap();
        let expected = serde_json::to_value(store.snapshot()).unwrap();
        drop(store);
        let reopened = artex_store::Store::open(directory.path()).unwrap();
        assert_eq!(serde_json::to_value(reopened.snapshot()).unwrap(), expected);
        assert_eq!(
            reopened
                .snapshot()
                .get("conversation_requests", &mid)
                .unwrap()
                .data["state"],
            "completed"
        );
    }
    #[test]
    fn registered_request_options_and_terminal_evidence_are_immutable() {
        let (mut s, c) = fixture();
        let mut p = message(
            &s,
            &c[0],
            vec![c[1].actor.agent_id.clone().unwrap()],
            "request",
            None,
            false,
        );
        let mid = p["message"]["message_id"].as_str().unwrap().to_owned();
        let b = send(&p, &s, &c[0]).unwrap();
        apply(&mut s, b);
        p["request_options"] =
            json!({"completion_mode":"all","expected_reports":p["message"]["recipients"]});
        assert_eq!(send(&p, &s, &c[0]).unwrap_err().code, "COMMAND_ID_REUSE");
        let p = message(
            &s,
            &c[1],
            vec![c[0].actor.agent_id.clone().unwrap()],
            "reply",
            Some(&mid),
            true,
        );
        let b = send(&p, &s, &c[1]).unwrap();
        apply(&mut s, b);
        let mut duplicate = message(
            &s,
            &c[1],
            vec![c[0].actor.agent_id.clone().unwrap()],
            "reply",
            Some(&mid),
            true,
        );
        assert!(send(&duplicate, &s, &c[1]).is_ok());
        let cid = new_id("company");
        let mut company = default_data("companies").unwrap();
        company["id"] = json!(cid);
        insert(&mut s, "companies", &cid, company);
        duplicate["message"]["object_refs"] =
            json!([{"collection":"companies","id":cid,"version":"1"}]);
        assert_eq!(
            send(&duplicate, &s, &c[1]).unwrap_err().code,
            "ALREADY_COMPLETED"
        );
    }
    #[test]
    fn object_references_use_live_direct_source_scope_and_require_cas() {
        let (mut s, mut contexts) = fixture();
        let mut tids = vec![];
        let mut nids = vec![];
        for _ in 0..3 {
            let tid = new_id("task");
            let exp = new_id("exploration");
            let nid = new_id("node");
            let mut t = default_data("tasks").unwrap();
            t["id"] = json!(tid);
            t["exploration_id"] = json!(exp);
            insert(&mut s, "tasks", &tid, t);
            let mut n = default_data("exploration_nodes").unwrap();
            n["id"] = json!(nid);
            n["exploration_id"] = json!(exp);
            n["kind"] = json!("fact");
            n["state"] = json!("confirmed");
            insert(&mut s, "exploration_nodes", &nid, n);
            tids.push(tid);
            nids.push(nid);
        }
        let rel = new_id("relation");
        insert(
            &mut s,
            "task_relations",
            &rel,
            json!({"task_id":tids[0],"source_task_id":tids[1],"created_at":now()}),
        );
        let fid = new_id("finding");
        let mut finding = default_data("findings").unwrap();
        finding["id"] = json!(fid);
        finding["task_id"] = json!(tids[1]);
        insert(&mut s, "findings", &fid, finding);
        contexts[0].task_id = Some(tids[0].clone());
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut(contexts[0].actor.run_id.as_ref().unwrap())
            .unwrap()
            .data["task_id"] = json!(tids[0]);
        let mut p = message(
            &s,
            &contexts[0],
            vec![contexts[1].actor.agent_id.clone().unwrap()],
            "send",
            None,
            false,
        );
        p["message"]["object_refs"] = json!([{"collection":"findings","id":fid,"version":"1"},{"collection":"exploration_nodes","id":nids[1],"version":"1"}]);
        let b = send(&p, &s, &contexts[0]).unwrap();
        assert!(
            b.expected
                .iter()
                .any(|e| e.collection == "findings" && e.id == fid)
        );
        assert!(
            b.expected
                .iter()
                .any(|e| e.collection == "task_relations" && e.id == rel)
        );
        p["message"]["object_refs"] =
            json!([{"collection":"exploration_nodes","id":nids[2],"version":"1"}]);
        assert_eq!(
            send(&p, &s, &contexts[0]).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        s.collections
            .get_mut("tasks")
            .unwrap()
            .get_mut(&tids[1])
            .unwrap()
            .data["archived_at"] = json!(now());
        p["message"]["object_refs"] = json!([{"collection":"findings","id":fid,"version":"1"}]);
        assert_eq!(
            send(&p, &s, &contexts[0]).unwrap_err().code,
            "PERMISSION_DENIED"
        );
    }
}

/// Recovery never silently resumes an external execution or a run from another epoch.
pub fn recovery_plan(state: &Snapshot, epoch: &str) -> Option<CommandBatch> {
    let mut b = batch("engine.recover", &json!({"engine_epoch":epoch}));
    let timestamp = Utc::now();
    for r in state.records("agent_runs") {
        if matches!(
            r.data["state"].as_str(),
            Some("queued" | "running" | "waiting")
        ) && r.data["engine_epoch"] != epoch
        {
            let mut d = r.data.clone();
            d["state"] = json!("interrupted");
            d["finished_at"] = json!(now());
            put(&mut b, state, "agent_runs", &r.header.id, d);
        }
    }
    for r in state.records("tool_invocations") {
        if matches!(
            r.data["state"].as_str(),
            Some("prepared" | "awaiting_approval" | "running")
        ) {
            let mut d = r.data.clone();
            d["state"] = json!("recovery_required");
            put(&mut b, state, "tool_invocations", &r.header.id, d);
        }
    }
    for r in state.records("user_questions") {
        if r.data["state"] == "pending"
            && state
                .get("agent_runs", r.data["run_id"].as_str().unwrap_or(""))
                .is_some_and(|run| run.data["engine_epoch"] != epoch)
        {
            let mut d = r.data.clone();
            d["state"] = json!("cancelled");
            put(&mut b, state, "user_questions", &r.header.id, d);
        }
    }
    for r in state.records("agent_deliveries") {
        if matches!(r.data["state"].as_str(), Some("claimed" | "observed"))
            && r.data["owner"]["engine_epoch"] != epoch
        {
            let mut d = r.data.clone();
            d["state"] = json!("interrupted");
            d["owner"] = Value::Null;
            d["claim_deadline"] = Value::Null;
            put(&mut b, state, "agent_deliveries", &r.header.id, d);
        }
    }
    for r in state.records("conversation_requests") {
        if matches!(
            r.data["state"].as_str(),
            Some("open" | "claimed" | "waiting")
        ) && r.data["deadline_at"]
            .as_str()
            .and_then(|s| parse_time(s).ok())
            .is_some_and(|t| t <= timestamp)
        {
            let mut d = r.data.clone();
            d["state"] = json!("expired");
            put(&mut b, state, "conversation_requests", &r.header.id, d);
            expire_deliveries(&mut b, state, &r.header.id);
        }
    }
    for r in state.records("task_runtime") {
        if !r.data["last_active_clock"].is_null() {
            let mut d = r.data.clone();
            d["last_active_clock"] = Value::Null;
            d["accounting_state"] = json!("recovery_required");
            put(&mut b, state, "task_runtime", &r.header.id, d);
        }
    }
    for retest in state
        .records("finding_retests")
        .into_iter()
        .filter(|r| matches!(r.data["status"].as_str(), Some("pending" | "running")))
    {
        let mut data = retest.data.clone();
        data["status"] = json!("stopped");
        data["finished_at"] = json!(now());
        data["error"] = json!("Engine restarted; retest interrupted without replay");
        put(&mut b, state, "finding_retests", &retest.header.id, data);
    }
    // Recovery never admits a task before an explicit user resume.
    for task in state
        .records("tasks")
        .into_iter()
        .filter(|t| t.data["status"] == "running" || t.data["status"] == "queued")
    {
        let mut data = task.data.clone();
        data["status"] = json!("paused");
        data["paused"] = json!(true);
        data["queued"] = json!(false);
        put(&mut b, state, "tasks", &task.header.id, data);
        for node in state.records("exploration_nodes").into_iter().filter(|n| {
            n.data["exploration_id"] == task.data["exploration_id"]
                && n.data["kind"] == "intent"
                && matches!(
                    n.data["state"].as_str(),
                    Some("open" | "active" | "running" | "claimed")
                )
        }) {
            let mut data = node.data.clone();
            data["state"] = json!("paused");
            put(&mut b, state, "exploration_nodes", &node.header.id, data);
        }
    }
    if b.changes.is_empty() {
        None
    } else {
        b.result = json!({"recovered":true,"changed_objects":b.changes.len()});
        Some(b)
    }
}

fn expire_deliveries(b: &mut CommandBatch, state: &Snapshot, message_id: &str) {
    for r in state.records("agent_deliveries") {
        if r.data["message_id"] == message_id
            && matches!(
                updated(b, r)["state"].as_str(),
                Some("pending" | "claimed" | "observed" | "interrupted")
            )
        {
            let mut d = updated(b, r).clone();
            d["state"] = json!("expired");
            d["claim_deadline"] = Value::Null;
            d["completed_at"] = json!(now());
            put(b, state, "agent_deliveries", &r.header.id, d);
        }
    }
}

/// The single writer calls this from its idle timer. Expiration is durable even
/// when the UI is idle, and external effects are never restarted by maintenance.
pub fn maintenance_plan(state: &Snapshot, epoch: &str) -> Option<CommandBatch> {
    let mut b = batch("engine.maintain", &json!({"engine_epoch":epoch}));
    let timestamp = Utc::now();
    let mut resumable = BTreeSet::new();
    for r in state.records("conversation_requests") {
        if matches!(
            r.data["state"].as_str(),
            Some("open" | "claimed" | "waiting")
        ) && r.data["deadline_at"]
            .as_str()
            .and_then(|s| parse_time(s).ok())
            .is_some_and(|t| t <= timestamp)
        {
            let mut d = r.data.clone();
            d["state"] = json!("expired");
            put(&mut b, state, "conversation_requests", &r.header.id, d);
            expire_deliveries(&mut b, state, &r.header.id);
            if let Some(rid) = state
                .get("agent_messages", &r.header.id)
                .and_then(|m| m.data["sender"]["run_id"].as_str())
            {
                resumable.insert(rid.to_owned());
            }
        }
    }
    for r in state.records("user_questions") {
        if r.data["state"] == "pending"
            && r.data["deadline_at"]
                .as_str()
                .and_then(|s| parse_time(s).ok())
                .is_some_and(|t| t <= timestamp)
        {
            let mut d = r.data.clone();
            d["state"] = json!("expired");
            put(&mut b, state, "user_questions", &r.header.id, d);
            if let Some(rid) = r.data["run_id"].as_str() {
                resumable.insert(rid.to_owned());
            }
        }
    }
    for r in state.records("agent_deliveries") {
        if matches!(
            updated(&b, r)["state"].as_str(),
            Some("claimed" | "observed")
        ) && r.data["claim_deadline"]
            .as_str()
            .and_then(|s| parse_time(s).ok())
            .is_some_and(|t| t <= timestamp)
        {
            let mut d = r.data.clone();
            d["state"] = json!("interrupted");
            d["owner"] = Value::Null;
            d["claim_deadline"] = Value::Null;
            put(&mut b, state, "agent_deliveries", &r.header.id, d);
        }
    }
    for rid in resumable {
        resume_run(&mut b, state, &rid, epoch);
    }
    if b.changes.is_empty() {
        None
    } else {
        b.result = json!({"changed_objects":b.changes.len()});
        Some(b)
    }
}
