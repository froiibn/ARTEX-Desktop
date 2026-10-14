//! Side questions own no tool executor and never mutate the parent transcript.
use crate::*;
use sha2::{Digest, Sha256};
pub const QUERIES: &[&str] = &["legacy.handleSideQuestions", "legacy.sideEvents"];
pub const COMMANDS: &[&str] = &["legacy.handleSideQuestions", "legacy.cancelSideRequest"];
fn n(v: &Value, key: &str) -> u64 {
    text(v, key).parse().unwrap_or(0)
}
fn user(c: &Context) -> Result<()> {
    if c.actor.kind == "user" {
        Ok(())
    } else {
        Err(Error::new(
            "PERMISSION_DENIED",
            "Side questions are a user-only service",
        ))
    }
}
pub fn route(p: &Value) -> Result<Value> {
    artex_core::contracts::read("legacy-routes.v1.json")["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["route_id"] == p["route_id"]
                && ["handleSideQuestions", "sideEvents", "cancelSideRequest"]
                    .contains(&text(r, "handler"))
        })
        .cloned()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Invalid side-question route"))
}
pub fn parent(s: &Snapshot, p: &Value, kind: &str) -> Result<Value> {
    let id = required(&p["path"], "id")?;
    if kind == "conversation" {
        let conv = record(s, "conversations", id)?;
        return Ok(
            json!({"session_key":format!("conv-{id}"),"conversation_id":id,"task_id":null,"exploration_id":null,"intent_id":null,"parent_version":conv.header.version}),
        );
    }
    let t = record(s, "tasks", id)?;
    if !t.data["deleted_at"].is_null() || !t.data["archived_at"].is_null() {
        return Err(Error::new("BUSY", "Task has been archived or deleted"));
    }
    let exp = required(&t.data, "exploration_id")?;
    let intent = if kind == "worker" {
        let iid = required(&p["path"], "iid")?;
        let node = record(s, "exploration_nodes", iid)?;
        if node.data["kind"] != "intent" || node.data["exploration_id"] != exp {
            return Err(Error::new("NOT_FOUND", "Worker is outside this task"));
        }
        if node.data["state"] == "stopped" {
            return Err(Error::new("BUSY", "Worker has been deleted"));
        }
        Some(iid)
    } else {
        None
    };
    Ok(
        json!({"session_key":if let Some(iid)=intent{format!("task-{id}-exp-{exp}-worker-{iid}")}else{format!("task-{id}-exp-{exp}-main")},"conversation_id":null,"task_id":id,"exploration_id":exp,"intent_id":intent,"parent_version":t.header.version}),
    )
}
fn session<'a>(s: &'a Snapshot, key: &str) -> Option<&'a Record> {
    s.records("side_question_sessions")
        .into_iter()
        .find(|r| r.data["session_key"] == key)
}
/// Credentials and proxy URLs are excluded from persisted/public model identity.
pub fn model(profile: &Record) -> Value {
    let keys = [
        "format",
        "base_url",
        "model",
        "thinking_type",
        "reasoning_effort",
        "streaming",
        "max_tokens_field",
        "session_header_key",
    ];
    let identity = serde_json::to_vec(
        &keys
            .iter()
            .map(|k| profile.data[*k].clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    json!({"profile_id":profile.header.id,"name":profile.data["name"],"format":profile.data["format"],"model":profile.data["model"],"identity":format!("{:x}",Sha256::digest(identity)),"streaming":profile.data["streaming"],"window_tokens":profile.data["context_window_k"].as_i64().filter(|n|*n>0).unwrap_or(200)*1000})
}
pub fn available<'a>(s: &'a Snapshot, m: &Value) -> Result<&'a Record> {
    let p = record(s, "llm_profiles", required(m, "profile_id")?)?;
    if !llm_ui::configured(p) || model(p)["identity"] != m["identity"] {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "Captured model configuration was removed or changed; run the parent agent to refresh context",
        ));
    }
    Ok(p)
}
pub fn wire(r: &Record) -> Value {
    let d = &r.data;
    let mut context = d["context_info"].clone();
    if let Some(o) = context.as_object_mut() {
        o.remove("admitted_question");
        o.remove("frozen_snapshot");
    }
    let mut out = json!({"id":r.header.id,"ordinal":d["ordinal"],"client_request_id":d["client_id"],"question":d["question"],"answer":d["answer"],"status":d["status"],"model":d["model"],"snapshot_at":d["snapshot_at"],"created_at":d["created_at"],"sequence":d["sequence"],"usage":d["usage"],"context":context});
    if !text(d, "error").is_empty() {
        out["error"] = d["error"].clone();
    }
    out
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    user(c)?;
    if m == "legacy.sideEvents" {
        let id = required(&p["path"], "requestID")?;
        return Ok(wire(record(s, "side_question_requests", id)?));
    }
    let r = route(p)?;
    if r["legacy_http_method"] != "GET" {
        return Err(Error::new(
            "INVALID_PARAMS",
            "This side-question route requires a command",
        ));
    }
    let parent = parent(s, p, required(&r, "parent_kind")?)?;
    let key = required(&parent, "session_key")?;
    let before = p["query"]["before"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| p["query"]["before"].as_u64())
        .unwrap_or(0);
    let all = s
        .records("side_question_requests")
        .into_iter()
        .filter(|r| r.data["session_key"] == key)
        .collect::<Vec<_>>();
    let current = all
        .iter()
        .find(|r| r.data["status"] == "running")
        .map(|r| wire(r))
        .unwrap_or(Value::Null);
    let mut history = all
        .into_iter()
        .filter(|r| {
            r.data["status"] != "running" && (before == 0 || n(&r.data, "ordinal") < before)
        })
        .collect::<Vec<_>>();
    history.sort_by_key(|r| std::cmp::Reverse(n(&r.data, "ordinal")));
    let more = history.len() > 20;
    history.truncate(20);
    let next = if more {
        history.last().map(|r| r.data["ordinal"].clone()).unwrap()
    } else {
        json!("0")
    };
    let snapshot=session(s,key).map(|r|{let snap=&r.data["snapshot"];let status=available(s,&snap["model"]);json!({"captured_at":snap["captured_at"],"model":snap["model"],"available":status.is_ok(),"reason":status.err().map(|e|e.message).unwrap_or_default()})}).unwrap_or(Value::Null);
    Ok(
        json!({"items":history.into_iter().map(wire).collect::<Vec<_>>(),"current":current,"next_cursor":next,"snapshot":snapshot}),
    )
}
/// Publish a frozen provider input before the main completion. Parent run/version ordering survives restarts.
pub fn checkpoint_plan(
    s: &Snapshot,
    c: &Context,
    rid: &str,
    version: u64,
    profile: &Record,
    request: &Value,
) -> Result<CommandBatch> {
    let run = record(s, "agent_runs", rid)?;
    let mut b = Planner::new(
        s,
        "side.checkpoint",
        &json!({"run_id":rid,"version":version.to_string()}),
    );
    let parent = if let Some(id) = run.data["conversation_id"].as_str() {
        parent(s, &json!({"path":{"id":id}}), "conversation")?
    } else {
        parent(
            s,
            &json!({"path":{"id":run.data["task_id"],"iid":run.data["intent_id"]}}),
            if run.data["intent_id"].is_null() {
                "main"
            } else {
                "worker"
            },
        )?
    };
    let key = required(&parent, "session_key")?;
    let previous = session(s, key);
    if let Some(old) = previous {
        let oldrun = s.get("agent_runs", text(&old.data, "run_id"));
        if old.data["run_id"] == rid && n(&old.data, "version") >= version
            || old.data["run_id"] != rid
                && oldrun.is_some_and(|r| {
                    r.header.commit_seq.parse::<u64>().unwrap_or(0)
                        > run.header.commit_seq.parse::<u64>().unwrap_or(0)
                })
        {
            return Ok(b.done(json!({"saved":false})));
        }
    }
    b.expect(run);
    b.expect(profile);
    let mut d = previous
        .map(|r| r.data.clone())
        .unwrap_or(default_data("side_question_sessions")?);
    for key in [
        "session_key",
        "conversation_id",
        "task_id",
        "exploration_id",
        "intent_id",
    ] {
        d[key] = parent[key].clone();
    }
    d["run_id"] = json!(rid);
    d["version"] = json!(version.to_string());
    d["snapshot"] = json!({"parent":parent,"run_id":rid,"version":version.to_string(),"captured_at":artex_core::now(),"model":model(profile),"request":request});
    let id = previous
        .map(|r| r.header.id.clone())
        .unwrap_or_else(|| new_id("sidesession"));
    b.put("side_question_sessions", &id, d);
    let mut out = b.done(json!({"saved":true,"session_id":id}));
    augment_expected(&mut out, s)?;
    let _ = c;
    Ok(out)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    user(c)?;
    let mut b = Planner::new(s, m, p);
    if m == "legacy.cancelSideRequest" {
        let r = record(
            s,
            "side_question_requests",
            required(&p["path"], "requestID")?,
        )?;
        if r.data["status"] == "running" {
            let mut d = r.data.clone();
            d["status"] = json!("cancelled");
            d["error"] = json!("Answers have stopped");
            d["sequence"] = json!((n(&d, "sequence") + 1).to_string());
            b.put("side_question_requests", &r.header.id, d);
            conversation::push_event(
                &mut b.batch,
                s,
                c,
                "SideQuestionUpdated",
                "side_question_requests",
                &r.header.id,
            )?;
        }
        let mut out = b.done(json!({"cancelled":true}));
        augment_expected(&mut out, s)?;
        return Ok(out);
    }
    let route = route(p)?;
    let parent = parent(s, p, required(&route, "parent_kind")?)?;
    let key = required(&parent, "session_key")?;
    if route["legacy_http_method"] == "DELETE" {
        if let Some(row) = session(s, key) {
            let mut d = row.data.clone();
            d["generation"] = json!((n(&d, "generation") + 1).to_string());
            d["memory"] = json!({});
            b.put("side_question_sessions", &row.header.id, d);
            for r in s
                .records("side_question_requests")
                .into_iter()
                .filter(|r| r.data["session_key"] == key)
            {
                b.delete("side_question_requests", &r.header.id);
            }
        }
        let mut out = b.done(json!({"cleared":true}));
        augment_expected(&mut out, s)?;
        return Ok(out);
    }
    if route["legacy_http_method"] != "POST" {
        return Err(Error::new(
            "INVALID_PARAMS",
            "This side-question route is read-only",
        ));
    }
    let input = &p["body"];
    let question = required(input, "question")?.trim();
    let client = required(input, "client_request_id")?;
    if question.chars().count() > 4000
        || serde_json::to_vec(input)?.len() > 32768
        || client.len() > 128
        || !client
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:-".contains(&c))
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Question must be 1–4000 characters with a valid request ID",
        ));
    }
    let session = session(s, key)
        .ok_or_else(|| Error::new("BUSY", "No context snapshot yet; run the parent agent"))?;
    if let Some(r) = s.records("side_question_requests").into_iter().find(|r| {
        r.data["session_key"] == key
            && r.data["client_id"] == client
            && r.data["generation"] == session.data["generation"]
    }) {
        if r.data["question"] != question {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Same request ID cannot be reused for a different question",
            ));
        }
        return Ok(b.done(wire(r)));
    }
    let running = s
        .records("side_question_requests")
        .into_iter()
        .filter(|r| r.data["status"] == "running")
        .collect::<Vec<_>>();
    if running.iter().any(|r| r.data["session_key"] == key) {
        return Err(Error::new(
            "BUSY",
            "This parent already has a side question running",
        ));
    }
    if running.len() >= 4 {
        return Err(Error::new(
            "BUSY",
            "Side-question concurrency limit is four",
        ));
    }
    let profile = available(s, &session.data["snapshot"]["model"])?;
    b.expect(session);
    b.expect(profile);
    let question_input = mentions_ui::compose(&mut b, question)?;
    let counter = s
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "side_question.ordinal");
    let ordinal = s
        .records("side_question_requests")
        .into_iter()
        .map(|r| n(&r.data, "ordinal"))
        .max()
        .unwrap_or(0)
        .max(counter.map(|r| n(&r.data, "value")).unwrap_or(0))
        .checked_add(1)
        .ok_or_else(|| Error::new("SIZE_LIMIT", "Side question ordinal is exhausted"))?;
    let mut counter_data = default_data("settings")?;
    counter_data["key"] = json!("side_question.ordinal");
    counter_data["value"] = json!(ordinal.to_string());
    b.put(
        "settings",
        &counter
            .map(|r| r.header.id.clone())
            .unwrap_or_else(|| new_id("setting")),
        counter_data,
    );
    let id=b.insert("side_question_requests","sidequestion",json!({"session_key":key,"ordinal":ordinal.to_string(),"generation":session.data["generation"],"client_id":client,"question":question,"answer":artex_core::body(""),"status":"running","model":session.data["snapshot"]["model"],"snapshot_at":session.data["snapshot"]["captured_at"],"context_info":{"phase":"preparing","recent_exchanges":0,"history_summarized":false,"snapshot_summarized":false,"admitted_question":question_input,"frozen_snapshot":session.data["snapshot"]}}))?;
    conversation::push_event(
        &mut b.batch,
        s,
        c,
        "SideQuestionCreated",
        "side_question_requests",
        &id,
    )?;
    let staged = Record {
        header: artex_core::Header {
            id: id.clone(),
            collection: "side_question_requests".into(),
            version: "0".into(),
            commit_seq: "0".into(),
            created_at: artex_core::now(),
            updated_at: artex_core::now(),
            legacy_key: Value::Null,
        },
        data: b.data("side_question_requests", &id)?,
    };
    let mut out = b.done(wire(&staged));
    augment_expected(&mut out, s)?;
    Ok(out)
}
/// Guarded persistence rejects completion after cancel/clear/delete/restart.
pub fn update_plan(
    s: &Snapshot,
    c: &Context,
    id: &str,
    generation: &str,
    answer: &Value,
    status: &str,
    error: &str,
    usage: &Value,
    context: &Value,
    memory: Option<&Value>,
) -> Result<CommandBatch> {
    if !["running", "completed", "failed", "cancelled", "interrupted"].contains(&status) {
        return Err(Error::new("INVALID_PARAMS", "Invalid side-question state"));
    }
    let mut b = Planner::new(s, "side.update", &json!({"id":id,"generation":generation}));
    let Some(row) = s.get("side_question_requests", id) else {
        return Ok(b.done(json!({"saved":false})));
    };
    let Some(session) = session(s, text(&row.data, "session_key")) else {
        return Ok(b.done(json!({"saved":false})));
    };
    if row.data["status"] != "running"
        || row.data["generation"] != generation
        || session.data["generation"] != generation
    {
        return Ok(b.done(json!({"saved":false})));
    }
    let mut d = row.data.clone();
    d["answer"] = answer.clone();
    d["status"] = json!(status);
    d["error"] = json!(error);
    d["usage"] = usage.clone();
    d["context_info"] = context.clone();
    d["sequence"] = json!((n(&d, "sequence") + 1).to_string());
    b.put("side_question_requests", id, d);
    b.expect(session);
    if let Some(memory) = memory {
        let mut next = session.data.clone();
        next["memory"] = memory.clone();
        b.put("side_question_sessions", &session.header.id, next);
    }
    conversation::push_event(
        &mut b.batch,
        s,
        c,
        "SideQuestionUpdated",
        "side_question_requests",
        id,
    )?;
    let mut out = b.done(json!({"saved":true}));
    augment_expected(&mut out, s)?;
    Ok(out)
}

/// A dedicated provider worker consumes this immutable payload; tools are never exposed.
pub fn execution_input(s: &Snapshot, id: &str) -> Result<Value> {
    let request = record(s, "side_question_requests", id)?;
    let session = session(s, text(&request.data, "session_key"))
        .ok_or_else(|| Error::new("NOT_FOUND", "Parent side session was cleared"))?;
    if request.data["status"] != "running"
        || request.data["generation"] != session.data["generation"]
    {
        return Err(Error::new("BUSY", "Side request is no longer active"));
    }
    let profile = available(s, &request.data["model"])?;
    // The admitted checkpoint is pinned while the main run continues. Never substitute a newer snapshot.
    let frozen = request.data["context_info"]["frozen_snapshot"]
        .as_object()
        .map(|_| request.data["context_info"]["frozen_snapshot"].clone())
        .unwrap_or_else(|| session.data["snapshot"].clone());
    let mut history = s
        .records("side_question_requests")
        .into_iter()
        .filter(|r| {
            r.data["session_key"] == session.data["session_key"]
                && r.data["generation"] == request.data["generation"]
                && r.data["status"] == "completed"
                && n(&r.data, "ordinal") < n(&request.data, "ordinal")
        })
        .collect::<Vec<_>>();
    history.sort_by_key(|r| n(&r.data, "ordinal"));
    Ok(
        json!({"id":id,"generation":request.data["generation"],"profile_id":profile.header.id,"snapshot":frozen,"question":request.data["context_info"]["admitted_question"].as_str().unwrap_or(text(&request.data,"question")),"history":history.into_iter().map(wire).collect::<Vec<_>>(),"memory":session.data["memory"],"tools":[],"timeout_ms":120000,"max_output_tokens":8192}),
    )
}

pub fn recovery_plan(s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(
        s,
        "side.recovery",
        &json!({"engine_epoch":c.actor.engine_epoch}),
    );
    for r in s
        .records("side_question_requests")
        .into_iter()
        .filter(|r| r.data["status"] == "running")
    {
        let mut d = r.data.clone();
        d["status"] = json!("interrupted");
        d["error"] =
            json!("Engine restarted while the side response was in flight; ask again to continue");
        d["sequence"] = json!((n(&d, "sequence") + 1).to_string());
        b.put("side_question_requests", &r.header.id, d);
        conversation::push_event(
            &mut b.batch,
            s,
            c,
            "SideQuestionUpdated",
            "side_question_requests",
            &r.header.id,
        )?;
    }
    let mut out = b.done(json!({"recovered":true}));
    augment_expected(&mut out, s)?;
    Ok(out)
}
