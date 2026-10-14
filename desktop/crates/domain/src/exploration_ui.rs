use crate::*;
pub const QUERIES: &[&str] = &[
    "ui.frontier",
    "ui.intents",
    "ui.intentsPage",
    "ui.explorationGraph",
    "ui.explorationNodes",
    "ui.activity",
    "ui.activityDetail",
    "ui.activityHistory",
    "ui.mainSessions",
    "ui.taskAssetRefs",
];
pub const COMMANDS: &[&str] = &["ui.newMainSession"];
pub fn resolve_task<'a>(s: &'a Snapshot, p: &Value) -> Result<Option<&'a Record>> {
    let id = text(p, "task");
    if !id.is_empty() && id != "active" {
        let t = record(s, "tasks", id)?;
        if !t.data["deleted_at"].is_null() || !t.data["archived_at"].is_null() {
            return Err(Error::new("NOT_FOUND", "Task is not live"));
        }
        return Ok(Some(t));
    }
    let active = s
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "active_task_id")
        .and_then(|r| r.data["value"].as_str())
        .and_then(|id| s.get("tasks", id))
        .filter(|t| t.data["deleted_at"].is_null() && t.data["archived_at"].is_null());
    Ok(active.or_else(|| {
        s.records("tasks")
            .into_iter()
            .filter(|t| t.data["deleted_at"].is_null() && t.data["archived_at"].is_null())
            .max_by_key(|r| r.header.created_at.clone())
    }))
}
pub fn node(r: &Record, s: &Snapshot, task: &str) -> Value {
    let d = &r.data;
    let mut out = json!({"id":r.header.id,"type":d["kind"],"payload":d["payload"].to_string(),"priority":d["priority"],"state":d["state"],"origin":text(d,"origin"),"ts":d["created_at"]});
    if !text(d, "delete_reason").is_empty() {
        out["delete_reason"] = d["delete_reason"].clone();
    }
    if let Some(owner) = s
        .records("tasks")
        .iter()
        .find(|t| t.data["exploration_id"] == d["exploration_id"] && t.header.id != task)
    {
        out["inherited"] = json!(true);
        out["source_task_id"] = json!(owner.header.id);
    }
    out
}
pub fn edge(r: &Record) -> Value {
    json!({"from":r.data["src_id"],"to":r.data["dst_id"],"rel":r.data["rel"]})
}
pub fn activity(r: &Record, s: &Snapshot, task: &str, detail: bool) -> Value {
    let d = &r.data;
    let mut out = json!({"seq":r.header.id,"worker":text(d,"worker"),"kind":conversation::activity_wire_kind(text(d,"kind")),"ts":d["created_at"],"is_error":d["is_error"],"summary":text(d,"summary")});
    for key in [
        "tool",
        "tool_use_id",
        "main_seg",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ] {
        if !d[key].is_null() && d[key] != "" {
            out[key] = d[key].clone();
        }
    }
    if !d["metadata"]["llm_transition"].is_null() {
        out["metadata"] = json!({"llm_transition":d["metadata"]["llm_transition"]});
    }
    if !d["node_id"].is_null() {
        out["intent_id"] = d["node_id"].clone();
    }
    if detail && !d["detail"].is_null() {
        out["detail"] = d["detail"].clone();
    }
    if let Some(owner) = s
        .records("tasks")
        .iter()
        .find(|t| t.data["exploration_id"] == d["exploration_id"] && t.header.id != task)
    {
        out["inherited"] = json!(true);
        out["source_task_id"] = json!(owner.header.id);
    }
    out
}
pub fn asset_label(a: &Value) -> String {
    let url = text(a, "url");
    if !url.is_empty() {
        let method = text(a, "method");
        return if method.is_empty() {
            url.into()
        } else {
            format!("{method} {url}")
        };
    }
    for key in [
        "domain",
        "ip",
        "app_name",
        "bundle_id",
        "service_name",
        "root_domain",
    ] {
        let v = text(a, key);
        if !v.is_empty() {
            return if matches!(key, "domain" | "ip") && a["port"].as_i64().is_some_and(|p| p > 0) {
                format!("{v}:{}", a["port"])
            } else {
                v.into()
            };
        }
    }
    format!("#{}", text(a, "id"))
}
pub fn asset_ref(a: &Record) -> Value {
    json!({"id":a.header.id,"type":a.data["type"],"label":asset_label(&a.data)})
}
fn ordered<'a>(mut rows: Vec<&'a Record>, ascending: bool) -> Vec<&'a Record> {
    rows.sort_by(|a, b| {
        a.header
            .created_at
            .cmp(&b.header.created_at)
            .then_with(|| {
                a.header
                    .commit_seq
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0))
            })
            .then_with(|| a.header.id.cmp(&b.header.id))
    });
    if !ascending {
        rows.reverse();
    }
    rows
}
pub fn visible_nodes<'a>(s: &'a Snapshot, task: &str, inherit: bool) -> Result<Vec<&'a Record>> {
    let exp = exploration_id(s, task)?;
    let visible = if inherit {
        visible_explorations(s, task)
    } else {
        BTreeSet::from([exp.clone()])
    };
    Ok(ordered(
        s.records("exploration_nodes")
            .into_iter()
            .filter(|n| {
                visible.contains(text(&n.data, "exploration_id"))
                    && (n.data["exploration_id"] == exp
                        || n.data["kind"] != "intent"
                        || matches!(
                            text(&n.data, "state"),
                            "done" | "blocked" | "exhausted" | "stopped"
                        ))
            })
            .collect(),
        true,
    ))
}
fn current_segment(s: &Snapshot, exp: &str) -> i64 {
    s.records("main_sessions")
        .iter()
        .filter(|r| r.data["exploration_id"] == exp)
        .filter_map(|r| r.data["seq"].as_i64())
        .max()
        .unwrap_or(0)
}
fn terminal_history(s: &Snapshot, task: &str, n: &Record) -> bool {
    let inherited =
        n.data["exploration_id"] != record(s, "tasks", task).unwrap().data["exploration_id"];
    !inherited
        || matches!(
            text(&n.data, "state"),
            "done" | "blocked" | "exhausted" | "stopped"
        )
}
fn activity_rows<'a>(s: &'a Snapshot, task: &str, intent: Option<&str>) -> Result<Vec<&'a Record>> {
    let exp = exploration_id(s, task)?;
    let mut exp_for = exp.clone();
    if let Some(id) = intent {
        let n = visible_node(s, task, id)?;
        if n.data["kind"] != "intent" || !terminal_history(s, task, n) {
            return Err(Error::new("NOT_FOUND", "Session not found"));
        }
        exp_for = text(&n.data, "exploration_id").into();
    }
    Ok(ordered(
        s.records("activity")
            .into_iter()
            .filter(|r| {
                r.data["exploration_id"] == exp_for
                    && intent.is_none_or(|id| r.data["node_id"] == id)
                    && (exp_for == exp
                        || !matches!(text(&r.data, "kind"), "thinking" | "usage" | "compression"))
            })
            .collect(),
        true,
    ))
}
fn after_cursor<'a>(
    rows: Vec<&'a Record>,
    s: &Snapshot,
    id: &str,
    after: bool,
) -> Result<Vec<&'a Record>> {
    if id.is_empty() {
        return Ok(rows);
    }
    if let Some(pos) = rows.iter().position(|r| r.header.id == id) {
        return Ok(if after {
            rows.into_iter().skip(pos + 1).collect()
        } else {
            rows.into_iter().take(pos).collect()
        });
    }
    if let Some(replaced) = s.replaced_activity_seq("activity", id) {
        return Ok(rows
            .into_iter()
            .filter(|r| {
                if after {
                    r.header.commit_seq.parse::<u64>().unwrap_or(0) >= replaced
                } else {
                    r.header.commit_seq.parse::<u64>().unwrap_or(0) < replaced
                }
            })
            .collect());
    }
    Err(Error::new("NOT_FOUND", "Cursor no longer visible"))
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    let resolved = resolve_task(s, p)?;
    let task = resolved.map(|r| r.header.id.as_str()).unwrap_or("");
    match m {
        "ui.mainSessions" => {
            let t = resolved.ok_or_else(|| Error::new("NOT_FOUND", "Task not found"))?;
            let exp = text(&t.data, "exploration_id");
            let mut rows = s
                .records("main_sessions")
                .into_iter()
                .filter(|r| r.data["exploration_id"] == exp)
                .collect::<Vec<_>>();
            rows.sort_by_key(|r| std::cmp::Reverse(r.data["seq"].as_i64().unwrap_or(0)));
            Ok(
                json!({"sessions":rows.iter().map(|r|json!({"seq":r.data["seq"],"created_at":r.data["created_at"]})).collect::<Vec<_>>(),"current":current_segment(s,exp)}),
            )
        }
        "ui.frontier"
        | "ui.intents"
        | "ui.intentsPage"
        | "ui.explorationGraph"
        | "ui.explorationNodes" => {
            let inherited = m != "ui.frontier" && m != "ui.explorationNodes";
            let mut rows = if task.is_empty() {
                vec![]
            } else {
                visible_nodes(s, task, inherited)?
            };
            match m {
                "ui.frontier" => {
                    rows.retain(|r| r.data["kind"] == "intent" && r.data["state"] == "open");
                    rows.sort_by(|a, b| {
                        b.data["priority"]
                            .as_i64()
                            .cmp(&a.data["priority"].as_i64())
                            .then_with(|| a.header.created_at.cmp(&b.header.created_at))
                    });
                    rows.truncate(100);
                    Ok(json!(
                        rows.iter().map(|r| node(r, s, task)).collect::<Vec<_>>()
                    ))
                }
                "ui.intents" | "ui.intentsPage" => {
                    rows.retain(|r| r.data["kind"] == "intent");
                    rows.reverse();
                    if m == "ui.intentsPage" && !text(p, "before").is_empty() {
                        let pos = rows
                            .iter()
                            .position(|r| r.header.id == text(p, "before"))
                            .ok_or_else(|| {
                                Error::new("NOT_FOUND", "Intent cursor no longer visible")
                            })?;
                        rows = rows.into_iter().skip(pos + 1).collect();
                    }
                    let limit = p["limit"]
                        .as_u64()
                        .filter(|v| *v > 0)
                        .unwrap_or(300)
                        .min(1000) as usize;
                    let more = rows.len() > limit;
                    rows.truncate(limit);
                    let out = rows.iter().map(|r| node(r, s, task)).collect::<Vec<_>>();
                    Ok(if m == "ui.intents" {
                        json!(out)
                    } else {
                        json!({"items":out,"hasMore":more})
                    })
                }
                "ui.explorationGraph" => {
                    let ids = rows
                        .iter()
                        .map(|r| r.header.id.as_str())
                        .collect::<BTreeSet<_>>();
                    let edges = s
                        .records("exploration_edges")
                        .into_iter()
                        .filter(|e| {
                            ids.contains(text(&e.data, "src_id"))
                                && ids.contains(text(&e.data, "dst_id"))
                        })
                        .map(edge)
                        .collect::<Vec<_>>();
                    Ok(
                        json!({"nodes":rows.iter().map(|r|node(r,s,task)).collect::<Vec<_>>(),"edges":edges}),
                    )
                }
                _ => {
                    let q = &p["query"];
                    let kinds = ids(q, "kinds");
                    let states = ids(q, "states");
                    let term = text(q, "q").to_lowercase();
                    rows.retain(|r| {
                        (kinds.is_empty() || kinds.contains(&text(&r.data, "kind").into()))
                            && (states.is_empty()
                                || states.contains(&text(&r.data, "state").into()))
                            && r.data["payload"].to_string().to_lowercase().contains(&term)
                    });
                    if q["order"] != "asc" {
                        rows.reverse();
                    }
                    let total = rows.len();
                    let page = q["page"].as_u64().unwrap_or(1).max(1);
                    let size = q["size"].as_u64().unwrap_or(20).clamp(1, 200);
                    let rows = rows
                        .into_iter()
                        .skip(((page - 1).saturating_mul(size)) as usize)
                        .take(size as usize)
                        .collect::<Vec<_>>();
                    let on_page = rows
                        .iter()
                        .map(|r| r.header.id.as_str())
                        .collect::<BTreeSet<_>>();
                    let edges = s
                        .records("exploration_edges")
                        .into_iter()
                        .filter(|r| {
                            on_page.contains(text(&r.data, "src_id"))
                                || on_page.contains(text(&r.data, "dst_id"))
                        })
                        .collect::<Vec<_>>();
                    let mut refs = serde_json::Map::new();
                    let mut all = on_page
                        .iter()
                        .map(|v| (*v).to_owned())
                        .collect::<BTreeSet<_>>();
                    for e in &edges {
                        for key in ["src_id", "dst_id"] {
                            let id = text(&e.data, key);
                            all.insert(id.into());
                            if !on_page.contains(id) {
                                if let Some(r) = s.get("exploration_nodes", id) {
                                    refs.insert(id.into(), node(r, s, task));
                                }
                            }
                        }
                    }
                    let mut assets = serde_json::Map::<String, Value>::new();
                    for a in s.records("exploration_anchors") {
                        let id = text(&a.data, "node_id");
                        if all.contains(id) {
                            if let Some(asset) = s.get("assets", text(&a.data, "asset_id")) {
                                assets
                                    .entry(id)
                                    .or_insert_with(|| json!([]))
                                    .as_array_mut()
                                    .unwrap()
                                    .push(asset_ref(asset));
                            }
                        }
                    }
                    Ok(
                        json!({"items":rows.iter().map(|r|node(r,s,task)).collect::<Vec<_>>(),"total":total,"page":page,"size":size,"edges":edges.iter().map(|r|edge(r)).collect::<Vec<_>>(),"refs":refs,"assets":assets}),
                    )
                }
            }
        }
        "ui.activity" => {
            if task.is_empty() {
                return Ok(json!({"items":[],"cursor":null}));
            }
            let opts = &p["opts"];
            let intent = opts["intent"].as_str();
            let rows = activity_rows(s, task, intent)?;
            let rows = after_cursor(rows, s, text(opts, "since"), true)?;
            let limit = opts["limit"].as_u64().unwrap_or(300).clamp(1, 1000) as usize;
            let rows = rows.into_iter().take(limit).collect::<Vec<_>>();
            let cursor = rows
                .last()
                .map(|r| r.header.id.clone())
                .or_else(|| opts["since"].as_str().map(str::to_owned));
            Ok(
                json!({"items":rows.iter().filter(|r|conversation::public_activity_kind(text(&r.data,"kind"))).map(|r|activity(r,s,task,false)).collect::<Vec<_>>(),"cursor":cursor}),
            )
        }
        "ui.activityDetail" => {
            let t = resolved.ok_or_else(|| Error::new("NOT_FOUND", "Task not found"))?;
            let a = record(s, "activity", required(p, "id")?)?;
            if !conversation::public_activity_kind(text(&a.data, "kind")) {
                return Err(Error::new("NOT_FOUND", "Internal activity"));
            }
            if a.data["exploration_id"] != t.data["exploration_id"] {
                let intent = a.data["node_id"].as_str().ok_or_else(|| {
                    Error::new("PERMISSION_DENIED", "Source session is not public")
                })?;
                let rows = activity_rows(s, task, Some(intent))?;
                if !rows.iter().any(|r| r.header.id == a.header.id) {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Activity is outside direct source scope",
                    ));
                }
            }
            Ok(
                json!({"detail":if a.data["detail"].is_null(){artex_core::body("")}else{a.data["detail"].clone()}}),
            )
        }
        "ui.activityHistory" => {
            let t = resolved.ok_or_else(|| Error::new("NOT_FOUND", "Task not found"))?;
            let session = required(p, "session")?;
            let intent = session.strip_prefix("intent:");
            let mut rows = activity_rows(s, task, intent)?;
            if intent.is_none() {
                let filter = if session == "main" || session.starts_with("main:") {
                    let seg = if session == "main" {
                        current_segment(s, text(&t.data, "exploration_id"))
                    } else {
                        session[5..]
                            .parse::<i64>()
                            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid main session"))?
                    };
                    Some(seg)
                } else if session == "plan" {
                    None
                } else {
                    return Err(Error::new("INVALID_PARAMS", "Invalid session"));
                };
                rows.retain(|r| {
                    if let Some(seg) = filter {
                        r.data["worker"] == "mainagent"
                            && r.data["main_seg"].as_i64().unwrap_or(0) == seg
                    } else {
                        r.data["worker"] == "planner" || r.data["worker"] == "goals"
                    }
                });
            }
            let snapshot = activity_rows(s, task, None)?
                .last()
                .map(|r| r.header.id.clone());
            rows = after_cursor(rows, s, text(p, "before"), false)?;
            let limit = p["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize;
            let more = rows.len() > limit;
            let rows = rows
                .into_iter()
                .rev()
                .take(limit)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>();
            let earliest = rows
                .first()
                .map(|r| r.header.id.clone())
                .or_else(|| p["before"].as_str().map(str::to_owned));
            Ok(
                json!({"items":rows.iter().filter(|r|conversation::public_activity_kind(text(&r.data,"kind"))).map(|r|activity(r,s,task,false)).collect::<Vec<_>>(),"snapshotCursor":snapshot,"earliestCursor":earliest,"hasMore":more}),
            )
        }
        "ui.taskAssetRefs" => {
            let task = required(p, "id")?;
            let visible = visible_nodes(s, task, true)?;
            let aid = required(p, "assetId")?;
            record(s, "assets", aid)?;
            let anchored = s
                .records("exploration_anchors")
                .iter()
                .filter(|r| r.data["asset_id"] == aid)
                .map(|r| text(&r.data, "node_id").to_owned())
                .collect::<BTreeSet<_>>();
            let mut out = json!({"intents":[],"facts":[],"findings":[]});
            for n in visible
                .into_iter()
                .filter(|r| anchored.contains(&r.header.id))
            {
                let key = match text(&n.data, "kind") {
                    "intent" => "intents",
                    "fact" => "facts",
                    "finding" => "findings",
                    _ => continue,
                };
                out[key].as_array_mut().unwrap().push(node(n, s, task));
            }
            Ok(out)
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    if m != "ui.newMainSession" {
        return Err(Error::new("METHOD_NOT_FOUND", m));
    }
    let task = resolve_task(s, p)?.ok_or_else(|| Error::new("NOT_FOUND", "Task not found"))?;
    let exp = text(&task.data, "exploration_id");
    let seq = current_segment(s, exp)
        .checked_add(1)
        .ok_or_else(|| Error::new("SIZE_LIMIT", "Session counter exhausted"))?;
    let mut b = Planner::new(s, m, p);
    let stamp = artex_core::now();
    b.insert(
        "main_sessions",
        "mainsession",
        json!({"exploration_id":exp,"seq":seq,"created_at":stamp}),
    )?;
    conversation::push_event(
        &mut b.batch,
        s,
        c,
        "MainSessionCreated",
        "tasks",
        &task.header.id,
    )?;
    Ok(b.done(json!({"seq":seq,"created_at":stamp,"current":seq})))
}
