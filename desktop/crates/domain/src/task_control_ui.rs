use crate::*;
use std::collections::BTreeMap;
// Source sendWorkerMessage accepts at most 4000 Unicode scalar values and an
// ASCII correlation ID; these are protocol limits, not a model-specific cap.
const WORKER_MESSAGE_CHAR_LIMIT: usize = 4000;
const WORKER_REQUEST_ID_LIMIT: usize = 128;
pub const QUERIES: &[&str] = &["ui.taskGoals", "ui.taskScope"];
pub const COMMANDS: &[&str] = &[
    "ui.recoverTaskBudget",
    "ui.renameTask",
    "ui.pinTask",
    "ui.addGoal",
    "ui.updateGoal",
    "ui.deleteGoal",
    "ui.controlTasksBatch",
    "ui.controlIntent",
    "ui.rerunIntent",
    "ui.rerunBlocked",
    "ui.sendWorkerMessage",
    "ui.updateTaskLLMProfiles",
    "ui.addTaskScope",
    "ui.deleteTaskScope",
    "ui.setActive",
];
fn live<'a>(s: &'a Snapshot, id: &str) -> Result<&'a Record> {
    let t = record(s, "tasks", id)?;
    if !t.data["deleted_at"].is_null() || !t.data["archived_at"].is_null() {
        return Err(Error::new("NOT_FOUND", "Task is not live"));
    }
    Ok(t)
}
fn stamp(d: &mut Value) {
    let now = artex_core::now();
    if d["created_at"] == "" {
        d["created_at"] = json!(now);
    }
    if d.get("updated_at").is_some() {
        d["updated_at"] = json!(artex_core::now());
    }
}
fn goal(d: &Value) -> Value {
    let mut out =
        json!({"id":d["id"],"text":d["payload"]["text"],"state":d["state"],"ts":d["created_at"]});
    for k in ["vulnclass"] {
        if !text(&d["payload"], k).is_empty() {
            out[k] = d["payload"][k].clone();
        }
    }
    if !text(d, "origin").is_empty() {
        out["origin"] = d["origin"].clone();
    }
    out
}
pub fn scope(d: &Value, s: &Snapshot) -> Value {
    let mut out =
        json!({"id":d["id"],"task_id":d["task_id"],"kind":d["kind"],"source":d["source"]});
    for k in ["company_id", "domain", "net", "value", "reason"] {
        if !d[k].is_null() && d[k] != "" {
            out[k] = d[k].clone();
        }
    }
    if let Some(co) = d["company_id"]
        .as_str()
        .and_then(|id| s.get("companies", id))
    {
        out["company_name"] = co.data["name"].clone();
    }
    out
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    let task = required(p, "id")?;
    let t = live(s, task)?;
    match m {
        "ui.taskGoals" => {
            let mut rows = s
                .records("exploration_nodes")
                .into_iter()
                .filter(|r| {
                    r.data["exploration_id"] == t.data["exploration_id"] && r.data["kind"] == "goal"
                })
                .collect::<Vec<_>>();
            rows.sort_by_key(|r| {
                (
                    r.header.commit_seq.parse::<u64>().unwrap_or(0),
                    r.header.id.clone(),
                )
            });
            Ok(json!({"goals":rows.iter().map(|r|goal(&r.data)).collect::<Vec<_>>()}))
        }
        "ui.taskScope" => {
            let sources = direct_sources(s, task);
            let mut rows = s
                .records("task_scope")
                .into_iter()
                .filter(|r| sources.contains(text(&r.data, "task_id")))
                .collect::<Vec<_>>();
            rows.sort_by_key(|r| {
                (
                    text(&r.data, "task_id") != task,
                    r.header.commit_seq.parse::<u64>().unwrap_or(0),
                    r.header.id.clone(),
                )
            });
            Ok(json!({"scope":rows.iter().map(|r|scope(&r.data,s)).collect::<Vec<_>>()}))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn absorb(b: &mut Planner, other: CommandBatch) {
    for ch in other.changes {
        match ch {
            Change::Put {
                collection,
                id,
                data,
            } => b.put(&collection, &id, data),
            Change::Delete { collection, id } => b.delete(&collection, &id),
        }
    }
    for e in other.expected {
        if b.state.get(&e.collection, &e.id).is_some()
            && !b
                .batch
                .expected
                .iter()
                .any(|x| x.collection == e.collection && x.id == e.id)
        {
            b.batch.expected.push(e)
        }
    }
    let event_start = b
        .state
        .records("domain_events")
        .iter()
        .filter_map(|r| text(&r.data, "seq").parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    for mut event in other.events {
        event.data["seq"] = json!((event_start + b.batch.events.len() as u64 + 1).to_string());
        b.batch.events.push(event);
    }
    b.batch.deliveries.extend(other.deliveries);
}
fn notify(b: &mut Planner, task: &str, summary: &str) -> Result<()> {
    let exp = exploration_id(b.state, task)?;
    let mut d = default_data("activity")?;
    merge(
        &mut d,
        &json!({"exploration_id":exp,"worker":"human","kind":"result","summary":summary}),
    );
    stamp(&mut d);
    b.insert("activity", "activity", d)?;
    Ok(())
}
pub fn revive(b: &mut Planner, task: &str, c: &Context) -> Result<()> {
    let mut t = b.data("tasks", task)?;
    if t["status"] == "running" || t["status"] == "queued" {
        return Ok(());
    }
    t["status"] = json!("paused");
    t["paused"] = json!(true);
    t["completed_at"] = Value::Null;
    t["deadline_at"] = Value::Null;
    // Resume admission consumes a snapshot of the current chain and concurrency policy.
    let mut projected = b.state.clone();
    let mut row = record(b.state, "tasks", task)?.clone();
    row.data = t;
    projected
        .collections
        .get_mut("tasks")
        .unwrap()
        .insert(task.to_owned(), row);
    let mut next = tasks::plan("task.resume", &json!({"task_id":task}), &projected, c)?;
    next.expected
        .retain(|e| e.collection != "tasks" || e.id != task);
    absorb(b, next);
    let exp = exploration_id(b.state, task)?;
    let mut e = b.data("explorations", &exp)?;
    e["status"] = json!("open");
    b.put("explorations", &exp, e);
    Ok(())
}
fn stop_runs(b: &mut Planner, intent: &str, c: &Context) -> Result<()> {
    for r in b.state.records("agent_runs") {
        if r.data["intent_id"] == intent
            && matches!(
                text(&r.data, "state"),
                "queued" | "running" | "waiting" | "paused"
            )
        {
            conversation::cancel_run(b, c, &r.header.id)?;
        }
    }
    for r in b.state.records("side_question_sessions") {
        if r.data["intent_id"] == intent {
            b.delete("side_question_sessions", &r.header.id);
        }
    }
    Ok(())
}
fn hard_delete(b: &mut Planner, task: &str, id: &str) -> Result<Value> {
    let exp = exploration_id(b.state, task)?;
    let nodes = b
        .state
        .records("exploration_nodes")
        .into_iter()
        .filter(|n| n.data["exploration_id"] == exp)
        .map(|r| (r.header.id.clone(), r))
        .collect::<BTreeMap<_, _>>();
    let mut parents = BTreeMap::<String, Vec<String>>::new();
    let mut down = BTreeMap::<String, Vec<String>>::new();
    for r in b.state.records("exploration_edges") {
        if r.data["exploration_id"] != exp {
            continue;
        }
        let src = text(&r.data, "src_id").to_owned();
        let dst = text(&r.data, "dst_id").to_owned();
        parents.entry(dst.clone()).or_default().push(src.clone());
        if matches!(text(&r.data, "rel"), "yields" | "derived_from") {
            down.entry(src).or_default().push(dst);
        }
    }
    let mut deleted = BTreeSet::from([id.to_owned()]);
    loop {
        let mut changed = false;
        for src in deleted.clone() {
            for dst in down.get(&src).into_iter().flatten() {
                let Some(n) = nodes.get(dst) else {
                    continue;
                };
                if deleted.contains(dst)
                    || n.data["kind"] == "goal"
                    || (n.data["kind"] == "fact" && n.data["state"] == "origin")
                {
                    continue;
                }
                if parents
                    .get(dst)
                    .into_iter()
                    .flatten()
                    .all(|p| deleted.contains(p))
                {
                    changed |= deleted.insert(dst.clone());
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = json!({"intents":0,"facts":0,"findings":0,"activities":0});
    for id in &deleted {
        let n = nodes[id];
        let key = match text(&n.data, "kind") {
            "intent" => "intents",
            "fact" => "facts",
            "finding" => "findings",
            _ => "",
        };
        if !key.is_empty() {
            out[key] = json!(out[key].as_u64().unwrap() + 1);
        }
        if n.data["kind"] == "intent" {
            for a in b.state.records("activity") {
                if a.data["exploration_id"] == exp && a.data["node_id"] == *id {
                    // Preserve irreversible usage without exposing deleted worker output.
                    let mut ledger = a.data.clone();
                    ledger["node_id"] = Value::Null;
                    ledger["worker"] = json!("token-ledger");
                    ledger["kind"] = json!("result");
                    ledger["summary"] = json!("Token usage for cancelled intent");
                    ledger["detail"] = Value::Null;
                    ledger["metadata"] = json!({"cancelled_intent_id":id,"token_rollup":true});
                    b.put("activity", &a.header.id, ledger);
                    out["activities"] = json!(out["activities"].as_u64().unwrap() + 1);
                }
            }
            for r in b.state.records("side_question_sessions") {
                if r.data["intent_id"] == *id {
                    b.delete("side_question_sessions", &r.header.id);
                }
            }
        }
        for f in b.state.records("findings") {
            if f.data["node_id"] == *id {
                b.delete("findings", &f.header.id);
            }
        }
        b.delete("exploration_nodes", id);
    }
    Ok(out)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.recoverTaskBudget" => {
            return tasks::plan(
                "task.recover_budget",
                &json!({"task_id":p["id"],"mode":p["mode"],"elapsed_active_ms":p["elapsedActiveMs"]}),
                s,
                c,
            );
        }
        "ui.renameTask" | "ui.pinTask" => {
            let input = if m == "ui.renameTask" {
                json!({"name":p["name"]})
            } else {
                json!({"pinned":p["pinned"]})
            };
            return task_ui::plan("ui.updateTask", &json!({"id":p["id"],"input":input}), s, c);
        }
        "ui.addGoal" | "ui.updateGoal" | "ui.deleteGoal" => {
            let task = required(p, "id")?;
            live(s, task)?;
            let exp = exploration_id(s, task)?;
            let id = if m == "ui.addGoal" {
                let content = required(p, "text")?.trim();
                let mut d = default_data("exploration_nodes")?;
                merge(
                    &mut d,
                    &json!({"exploration_id":exp,"kind":"goal","state":"open","origin":"human","payload":{"text":content}}),
                );
                if !text(p, "vulnclass").is_empty() {
                    d["payload"]["vulnclass"] = json!(text(p, "vulnclass").trim());
                }
                stamp(&mut d);
                let id = b.insert("exploration_nodes", "node", d)?;
                if let Some(origin) = s.records("exploration_nodes").iter().find(|n| {
                    n.data["exploration_id"] == exp
                        && n.data["kind"] == "fact"
                        && n.data["state"] == "origin"
                }) {
                    b.insert("exploration_edges","edge",json!({"exploration_id":exp,"src_id":origin.header.id,"dst_id":id,"rel":"spawns"}))?;
                }
                id
            } else {
                let id = required(p, "goalId")?;
                let n = own_node(s, task, id)?;
                if n.data["kind"] != "goal" {
                    return Err(Error::new("NOT_FOUND", "Goal not found"));
                }
                id.to_owned()
            };
            let old = b.data("exploration_nodes", &id)?;
            if m == "ui.deleteGoal" {
                b.delete("exploration_nodes", &id);
                notify(
                    &mut b,
                    task,
                    &format!("User deleted goal: {}", text(&old["payload"], "text")),
                )?;
                json!({"ok":true})
            } else {
                if m == "ui.updateGoal" {
                    let mut d = old;
                    d["payload"]["text"] = json!(required(p, "text")?.trim());
                    d["payload"]["vulnclass"] = json!(text(p, "vulnclass").trim());
                    b.put("exploration_nodes", &id, d);
                }
                notify(
                    &mut b,
                    task,
                    &format!(
                        "User {} goal: {}",
                        if m == "ui.addGoal" { "added" } else { "edited" },
                        required(p, "text")?.trim()
                    ),
                )?;
                revive(&mut b, task, c)?;
                goal(&b.data("exploration_nodes", &id)?)
            }
        }
        "ui.controlTasksBatch" => {
            let action = required(p, "action")?;
            if !matches!(action, "pause" | "resume") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Action must be pause or resume",
                ));
            }
            let mut seen = BTreeSet::new();
            let values = ids(p, "taskIds")
                .into_iter()
                .map(|v| v.trim().to_owned())
                .filter(|v| seen.insert(v.clone()))
                .collect::<Vec<_>>();
            if values.is_empty() || values.len() > 100 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Batch requires 1 to 100 task IDs",
                ));
            }
            let mut items = vec![];
            for id in values {
                let staged = projected(s, &b.batch)?;
                let attempt = live(&staged, &id).and_then(|_| {
                    tasks::plan(
                        if action == "pause" {
                            "task.pause"
                        } else {
                            "task.resume"
                        },
                        &json!({"task_id":id}),
                        &staged,
                        c,
                    )
                });
                match attempt {
                    Ok(next) => {
                        let data = next
                            .changes
                            .iter()
                            .find_map(|ch| match ch {
                                Change::Put {
                                    collection, data, ..
                                } if collection == "tasks" => Some(data.clone()),
                                _ => None,
                            })
                            .unwrap();
                        items.push(json!({"id":id,"ok":true,"status":data["status"],"queued":data["queued"]}));
                        absorb(&mut b, next);
                    }
                    Err(e) => items.push(json!({"id":id,"ok":false,"error":e.message})),
                }
            }
            json!({"items":items})
        }
        "ui.controlIntent" => {
            let task = required(p, "taskId")?;
            live(s, task)?;
            let id = required(p, "intentId")?;
            let n = own_node(s, task, id)?;
            if n.data["kind"] != "intent" {
                return Err(Error::new("NOT_FOUND", "Intent not found"));
            }
            let action = required(p, "action")?;
            let state = text(&n.data, "state");
            match action {
                "pause" | "resume" => {
                    let expected = if action == "pause" {
                        "running"
                    } else {
                        "paused"
                    };
                    if state != expected {
                        return Err(Error::new("VERSION_CONFLICT", "Intent state changed"));
                    }
                    if action == "pause" {
                        stop_runs(&mut b, id, c)?;
                    }
                    let mut d = n.data.clone();
                    d["state"] = json!(if action == "pause" { "paused" } else { "open" });
                    d["completed_at"] = Value::Null;
                    d["blocked_reason"] = Value::Null;
                    d["content_version"] = json!(d["content_version"].as_i64().unwrap_or(0) + 1);
                    b.put("exploration_nodes", id, d);
                    json!({"id":id,"state":if action=="pause"{"paused"}else{"open"}})
                }
                "cancel" => {
                    if !matches!(state, "running" | "paused" | "open") {
                        return Err(Error::new(
                            "VERSION_CONFLICT",
                            "Only live intents can be cancelled",
                        ));
                    }
                    let reason = required(p, "reason")?.trim();
                    let mode = p["mode"].as_str().unwrap_or("soft");
                    if !matches!(mode, "soft" | "hard") {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Cancel mode must be soft or hard",
                        ));
                    }
                    stop_runs(&mut b, id, c)?;
                    notify(
                        &mut b,
                        task,
                        &format!(
                            "User cancelled intent {}: {}",
                            text(&n.data["payload"], "summary"),
                            reason
                        ),
                    )?;
                    if mode == "hard" {
                        let deleted = hard_delete(&mut b, task, id)?;
                        json!({"id":id,"state":"","deleted":deleted})
                    } else {
                        let mut d = n.data.clone();
                        d["state"] = json!("deleted");
                        d["delete_reason"] = json!(reason);
                        d["blocked_reason"] = Value::Null;
                        d["completed_at"] = json!(artex_core::now());
                        d["content_version"] =
                            json!(d["content_version"].as_i64().unwrap_or(0) + 1);
                        b.put("exploration_nodes", id, d);
                        json!({"id":id,"state":"deleted"})
                    }
                }
                _ => return Err(Error::new("INVALID_PARAMS", "Invalid intent action")),
            }
        }
        "ui.rerunIntent" | "ui.rerunBlocked" => {
            let task = required(p, "taskId")?;
            live(s, task)?;
            let exp = exploration_id(s, task)?;
            let mut count = 0;
            if m == "ui.rerunIntent" {
                let n = own_node(s, task, required(p, "intentId")?)?;
                if n.data["kind"] != "intent" {
                    return Err(Error::new("NOT_FOUND", "Intent not found"));
                }
            }
            for n in s.records("exploration_nodes") {
                if n.data["exploration_id"] != exp || n.data["kind"] != "intent" {
                    continue;
                }
                let eligible = if m == "ui.rerunIntent" {
                    n.header.id == text(p, "intentId")
                        && matches!(text(&n.data, "state"), "blocked" | "exhausted" | "stopped")
                } else {
                    n.data["state"] == "blocked"
                };
                if eligible {
                    let mut d = n.data.clone();
                    d["state"] = json!("open");
                    d["completed_at"] = Value::Null;
                    d["blocked_reason"] = Value::Null;
                    b.put("exploration_nodes", &n.header.id, d);
                    count += 1;
                }
            }
            if count > 0 {
                revive(&mut b, task, c)?;
            }
            json!({"id":task,"reopened":count})
        }
        "ui.sendWorkerMessage" => {
            let task = required(p, "taskId")?;
            let t = live(s, task)?;
            let id = required(p, "intentId")?;
            let n = own_node(s, task, id)?;
            let request = required(p, "requestId")?.trim();
            let message = required(p, "message")?.trim();
            if message.chars().count() > WORKER_MESSAGE_CHAR_LIMIT {
                return Err(Error::new("INVALID_PARAMS", "Message exceeds 4000 characters"));
            }
            if request.is_empty() || request.len() > WORKER_REQUEST_ID_LIMIT || !request.bytes().all(|ch|ch.is_ascii_alphanumeric() || matches!(ch,b'-'|b'_'|b'.'|b':')) {
                return Err(Error::new("INVALID_PARAMS", "requestId must be 1-128 ASCII letters, numbers, -, _, . or :"));
            }
            if t.data["status"] != "running" || t.data["paused"] == true || t.data["queued"] == true {
                return Err(Error::new("VERSION_CONFLICT", "Resume the runnable task before sending a Worker message"));
            }
            if n.data["kind"] != "intent" {
                return Err(Error::new("VERSION_CONFLICT", "Intent node required"));
            }
            // Command receipts cover exact retries; a new command cannot replay
            // an already-admitted human turn with the same intent-scoped ID.
            if s.records("activity").iter().any(|a|a.data["exploration_id"]==t.data["exploration_id"]&&a.data["node_id"]==id&&a.data["metadata"]["worker_message_request_id"]==request) {
                return Err(Error::new("VERSION_CONFLICT", "Worker requestId was already admitted for this intent"));
            }
            b.expect(t);b.expect(n);
            if n.data["state"] == "running" {
                // Retain native live steering as an extension. The original UI
                // pause-then-send path below admits its own dedicated run.
                let next=exploration::plan("steer_work",&json!({"task_id":task,"intent_id":id,"message":message}),s,c)?;
                absorb(&mut b,next);
            } else {
                if n.data["state"] != "paused" {
                    return Err(Error::new("VERSION_CONFLICT", "Pause the intent before sending a Worker message"));
                }
                if s.records("agent_runs").iter().any(|r|r.data["task_id"]==task&&r.data["intent_id"]==id&&matches!(text(&r.data,"state"),"queued"|"running"|"waiting"|"paused"|"interrupted"|"recovery_required"|"cancel_requested")) {
                    return Err(Error::new("BUSY", "Previous intent run has not stopped"));
                }
                let agent=conversation::agent_by_key(s,"worker")?;
                if agent.data["enabled"] != true {return Err(Error::new("FEATURE_DISABLED","Worker is disabled"));}
                let profile=conversation::effective_profile(s,agent,None,Some(t))?;
                b.expect(agent);b.expect(profile);
                let parent=s.records("agent_runs").into_iter().filter(|r|r.data["task_id"]==task&&r.data["intent_id"]==id&&r.data.get("purpose").is_none_or(|purpose|purpose=="normal")&&s.get("agents",text(&r.data,"agent_id")).is_some_and(|a|a.data["key"]=="worker")).max_by_key(|r|r.header.commit_seq.parse::<u64>().unwrap_or(0));
                if let Some(parent)=parent {b.expect(parent);}
                let seconds=agent.data["run_seconds"].as_i64().filter(|n|*n>0).unwrap_or(default_data("agents")?["run_seconds"].as_i64().expect("agent deadline default"));
                let deadline=(chrono::Utc::now()+chrono::Duration::seconds(seconds)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true);
                // Planner::insert deliberately merges only required/default
                // fields. Explicitly retain the optional continuation fields.
                let run=new_id("run");let mut run_data=default_data("agent_runs")?;run_data["id"]=json!(run);merge(&mut run_data,&json!({"agent_id":agent.header.id,"task_id":task,"intent_id":id,"engine_epoch":c.actor.engine_epoch,"state":"queued","deadline_at":deadline}));run_data["purpose"]=json!("normal");run_data["parent_run_id"]=json!(parent.map(|r|r.header.id.as_str()));b.put("agent_runs",&run,run_data);
                let mut node=n.data.clone();node["state"]=json!("running");node["owner"]=json!(agent.header.id);node["completed_at"]=Value::Null;node["blocked_reason"]=Value::Null;node["content_version"]=json!(node["content_version"].as_i64().unwrap_or(0)+1);b.put("exploration_nodes",id,node);
                let referenced=mentions_ui::compose(&mut b,message)?;
                // The durable run and user turn are committed together. Runtime
                // consumes this exact run-scoped packet after queue admission,
                // bypassing frontier capacity without bypassing global fences.
                b.insert("activity","activity",json!({"exploration_id":t.data["exploration_id"],"node_id":id,"worker":"user","kind":"user","summary":message.chars().take(200).collect::<String>(),"detail":artex_core::body(&json!({"text":referenced,"display_text":message,"run_id":run,"request_id":request}).to_string()),"metadata":{"run_id":run,"turn_id":run,"worker_message_request_id":request}}))?;
                conversation::push_event(&mut b.batch,s,c,"RunQueued","agent_runs",&run)?;
            }
            json!({"id":id,"state":"running","accepted":true,"request_id":request})
        }
        "ui.updateTaskLLMProfiles" => {
            let task = required(p, "id")?;
            let t = live(s, task)?;
            let profiles = ids(p, "llmProfileIds");
            let mut seen = BTreeSet::new();
            for id in &profiles {
                if !seen.insert(id) {
                    return Err(Error::new("INVALID_PARAMS", "Duplicate LLM profile"));
                }
                let profile = record(s, "llm_profiles", id)?;
                if profile.data["api_key"].is_null() || text(&profile.data, "model").is_empty() {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "LLM profile is not configured",
                    ));
                }
            }
            let active = p["activeLLMProfileId"]
                .as_str()
                .or_else(|| profiles.first().map(String::as_str));
            if active.is_some_and(|id| !profiles.iter().any(|p| p == id)) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Active profile is outside task chain",
                ));
            }
            for r in s.records("task_llm_profiles") {
                if r.data["task_id"] == task {
                    b.delete("task_llm_profiles", &r.header.id);
                }
            }
            for (i, id) in profiles.iter().enumerate() {
                b.insert(
                    "task_llm_profiles",
                    "profilelink",
                    json!({"task_id":task,"profile_id":id,"position":i,"status":"ready"}),
                )?;
            }
            let mut d = t.data.clone();
            d["llm_profile_id"] = json!(active);
            d["active_llm_profile_id"] = json!(active);
            d["llm_chain_revision"] =
                json!((text(&d, "llm_chain_revision").parse::<u64>().unwrap_or(0) + 1).to_string());
            b.put("tasks", task, d);
            let mut reopened = 0;
            if !matches!(text(&t.data, "status"), "done" | "failed" | "timeout") {
                for n in s.records("exploration_nodes") {
                    if n.data["exploration_id"] == t.data["exploration_id"]
                        && n.data["kind"] == "intent"
                        && n.data["state"] == "blocked"
                        && n.data["blocked_reason"] == "llm_quota_exhausted"
                    {
                        let mut d = n.data.clone();
                        d["state"] = json!("open");
                        d["blocked_reason"] = Value::Null;
                        d["completed_at"] = Value::Null;
                        b.put("exploration_nodes", &n.header.id, d);
                        reopened += 1;
                    }
                }
            }
            json!({"id":task,"llm_profile_ids":profiles,"active_llm_profile_id":active,"llm_failover_state":if profiles.is_empty(){"default"}else{"ready"},"reopened_intents":reopened})
        }
        "ui.addTaskScope" => {
            let task = required(p, "id")?;
            live(s, task)?;
            let (id, _) = assets::add_scope(
                &mut b,
                task,
                required(p, "kind")?,
                required(p, "value")?,
                &p["reason"],
                "manual",
            )?;
            scope(&b.data("task_scope", &id)?, s)
        }
        "ui.deleteTaskScope" => {
            let task = required(p, "id")?;
            live(s, task)?;
            let id = required(p, "scopeId")?;
            let row = record(s, "task_scope", id)?;
            if row.data["task_id"] != task {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Source task scope is read only",
                ));
            }
            b.delete("task_scope", id);
            json!({"ok":true})
        }
        "ui.setActive" => {
            let task = required(p, "id")?;
            live(s, task)?;
            if let Some(r) = s
                .records("settings")
                .into_iter()
                .find(|r| r.data["key"] == "active_task_id")
            {
                let mut d = r.data.clone();
                d["value"] = json!(task);
                b.put("settings", &r.header.id, d);
            } else {
                b.insert(
                    "settings",
                    "setting",
                    json!({"key":"active_task_id","value":task}),
                )?;
            }
            json!({"active":task})
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if let Some(Change::Put { collection, id, .. } | Change::Delete { collection, id }) =
        b.batch.changes.first().cloned()
    {
        conversation::push_event(&mut b.batch, s, c, "TaskChanged", &collection, &id)?;
    }
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}

#[cfg(test)]
mod worker_message_regression {
    use super::*;
    use artex_core::Actor;
    use artex_store::Store;

    fn commit(store:&mut Store,mut batch:CommandBatch)->Value {augment_expected(&mut batch,&store.snapshot()).unwrap();batch.command_id=new_id("command");store.commit(batch).unwrap().result}
    #[test]
    fn paused_worker_message_admits_atomic_scoped_run_turn_with_source_validation_cas_and_no_replay_after_reopen() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("workspace");let mut store=Store::create(&path,"Paused worker message").unwrap();
        let mut c=Context{actor:Actor{kind:"user".into(),agent_id:None,run_id:None,engine_epoch:new_id("epoch")},task_id:None,intent_id:None,conversation_id:None};
        let b=crate::bootstrap_plan(&store.snapshot(),&c).unwrap();commit(&mut store,b);
        let b=crate::plan("task.create",&json!({"description":"Paused message fixture","goal":"Only fixture assets","seed_first_intent":false}),&store.snapshot(),&c).unwrap();let task=commit(&mut store,b)["task_id"].as_str().unwrap().to_owned();c.task_id=Some(task.clone());
        let s=store.snapshot();let agent=s.records("agents").into_iter().find(|a|a.data["key"]=="worker").unwrap().clone();let mut p=Planner::new(&s,"fixture.worker.paused",&json!({}));
        p.insert("llm_profiles","profile",json!({"name":"Fixture","format":"openai","model":"fixture","is_default":true,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}})).unwrap();
        let mut t=s.get("tasks",&task).unwrap().data.clone();t["status"]=json!("running");t["paused"]=json!(false);p.put("tasks",&task,t.clone());
        let node=p.insert("exploration_nodes","node",json!({"exploration_id":t["exploration_id"],"kind":"intent","state":"paused","payload":{"summary":"Paused fixture intent"}})).unwrap();
        let sibling=p.insert("exploration_nodes","node",json!({"exploration_id":t["exploration_id"],"kind":"intent","state":"running","payload":{"summary":"Independent worker remains active"}})).unwrap();
        let unrelated_run=p.insert("agent_runs","run",json!({"agent_id":agent.header.id,"task_id":task,"intent_id":sibling,"engine_epoch":c.actor.engine_epoch,"state":"waiting"})).unwrap();
        p.insert("activity","activity",json!({"exploration_id":t["exploration_id"],"node_id":sibling,"kind":"user","summary":"Other intent's request ID","metadata":{"run_id":unrelated_run,"worker_message_request_id":"safe_request:1"}})).unwrap();
        let foreign_exp=p.insert("explorations","exploration",json!({})).unwrap();p.insert("tasks","task",json!({"description":"Foreign owner","exploration_id":foreign_exp})).unwrap();let foreign_node=p.insert("exploration_nodes","node",json!({"exploration_id":foreign_exp,"kind":"intent","state":"paused"})).unwrap();
        commit(&mut store,p.done(json!({})));
        let mut newest_parent=String::new();
        for _ in 0..2 {let prior=store.snapshot();let mut p=Planner::new(&prior,"fixture.worker.prior_run",&json!({}));newest_parent=p.insert("agent_runs","run",json!({"agent_id":agent.header.id,"task_id":task,"intent_id":node,"engine_epoch":c.actor.engine_epoch,"state":"cancelled","finished_at":artex_core::now()})).unwrap();commit(&mut store,p.done(json!({})));}
        let input=json!({"taskId":task,"intentId":node,"requestId":"  safe_request:1  ","message":"  인간의 추가 지시 🧪  "});let s=store.snapshot();
        for (field,value) in [("message",json!("🧪".repeat(WORKER_MESSAGE_CHAR_LIMIT+1))),("message",json!("  ")),("requestId",json!("bad/request")),("requestId",json!("한글")),("requestId",json!("x".repeat(WORKER_REQUEST_ID_LIMIT+1))),("requestId",json!("  "))] {let mut bad=input.clone();bad[field]=value;assert_eq!(plan("ui.sendWorkerMessage",&bad,&s,&c).unwrap_err().code,"INVALID_PARAMS");}
        for (field,value) in [("paused",json!(true)),("queued",json!(true)),("status",json!("done"))] {let mut guarded=s.clone();guarded.collections.get_mut("tasks").unwrap().get_mut(&task).unwrap().data[field]=value;assert_eq!(plan("ui.sendWorkerMessage",&input,&guarded,&c).unwrap_err().code,"VERSION_CONFLICT");}
        let mut active=s.clone();let mut existing=active.get("agent_runs",&unrelated_run).unwrap().clone();existing.data["intent_id"]=json!(node);active.collections.get_mut("agent_runs").unwrap().insert(unrelated_run.clone(),existing);assert_eq!(plan("ui.sendWorkerMessage",&input,&active,&c).unwrap_err().code,"BUSY");
        let mut foreign=input.clone();foreign["intentId"]=json!(foreign_node);assert_eq!(plan("ui.sendWorkerMessage",&foreign,&s,&c).unwrap_err().code,"PERMISSION_DENIED");
        println!("WORKER_PARENT_RECORDS {}",json!(s.records("agent_runs").iter().map(|r|json!({"id":r.header.id,"task":r.data["task_id"],"intent":r.data["intent_id"],"purpose":r.data.get("purpose"),"purpose_present":r.data.get("purpose").is_some(),"agent":s.get("agents",text(&r.data,"agent_id")).map(|a|a.data["key"].clone())})).collect::<Vec<_>>()));
        let before=chrono::Utc::now();let b=plan("ui.sendWorkerMessage",&input,&s,&c).unwrap();let mut stale_input=input.clone();stale_input["requestId"]=json!("second-request");let mut stale=plan("ui.sendWorkerMessage",&stale_input,&s,&c).unwrap();stale.command_id=new_id("command");let result=commit(&mut store,b);let after=chrono::Utc::now();assert_eq!(result,json!({"id":node,"state":"running","accepted":true,"request_id":"safe_request:1"}));
        let admitted=store.snapshot();let run=admitted.records("agent_runs").into_iter().find(|r|r.data["intent_id"]==node&&r.data["state"]=="queued").unwrap();assert_eq!(run.data["state"],"queued");assert_eq!(run.data["agent_id"],agent.header.id);assert_eq!(run.data["engine_epoch"],c.actor.engine_epoch);assert_eq!(run.data["parent_run_id"],newest_parent);
        let seconds=agent.data["run_seconds"].as_i64().unwrap();let deadline=chrono::DateTime::parse_from_rfc3339(run.data["deadline_at"].as_str().unwrap()).unwrap().with_timezone(&chrono::Utc);assert!(deadline>=before+chrono::Duration::seconds(seconds)-chrono::Duration::milliseconds(1)&&deadline<=after+chrono::Duration::seconds(seconds));
        let turns=admitted.records("activity").into_iter().filter(|a|a.data["node_id"]==node&&a.data["kind"]=="user").collect::<Vec<_>>();assert_eq!(turns.len(),1);let body:Value=serde_json::from_str(turns[0].data["detail"]["text"].as_str().unwrap()).unwrap();assert_eq!(body["text"],"인간의 추가 지시 🧪");assert_eq!(body["display_text"],body["text"]);assert_eq!(body["run_id"],run.header.id);assert_eq!(turns[0].data["metadata"]["worker_message_request_id"],"safe_request:1");
        assert_eq!(admitted.get("exploration_nodes",&node).unwrap().data["owner"],agent.header.id);assert_eq!(admitted.get("agent_runs",&unrelated_run),s.get("agent_runs",&unrelated_run));
        assert_eq!(store.commit(stale).unwrap_err().code,"VERSION_CONFLICT");assert_eq!(store.snapshot().commit_seq,admitted.commit_seq);assert_eq!(plan("ui.sendWorkerMessage",&input,&admitted,&c).unwrap_err().code,"VERSION_CONFLICT");
        store.checkpoint().unwrap();drop(store);let reopened=Store::open_read_only(&path).unwrap();assert_eq!(reopened.snapshot().records("agent_runs").len(),admitted.records("agent_runs").len());assert_eq!(reopened.snapshot().get("agent_runs",&run.header.id),Some(run));assert_eq!(plan("ui.sendWorkerMessage",&input,&reopened.snapshot(),&c).unwrap_err().code,"VERSION_CONFLICT");
    }
}
