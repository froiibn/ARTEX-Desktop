use crate::*;

fn edge(b: &mut Planner, exp: &str, src: &str, dst: &str, rel: &str) -> Result<()> {
    b.insert(
        "exploration_edges",
        "edge",
        json!({"exploration_id":exp,"src_id":src,"dst_id":dst,"rel":rel}),
    )?;
    Ok(())
}
fn anchors(b: &mut Planner, node: &str, a: &Value) -> Result<()> {
    for id in ids(a, "asset_ids") {
        record(b.state, "assets", &id)?;
        b.insert(
            "exploration_anchors",
            "anchor",
            json!({"node_id":node,"asset_id":id}),
        )?;
    }
    Ok(())
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let task = task_id(p, c)?;
    let exp = exploration_id(s, task)?;
    match m {
        "add_intent" | "record_fact" | "set_goals" | "add_hint" | "add_task_hint" => {
            let (kind, state, array, key) = match m {
                "add_intent" => ("intent", "open", "intents", "summary"),
                "record_fact" => ("fact", "confirmed", "facts", "summary"),
                "set_goals" => ("goal", "open", "goals", "text"),
                _ => ("hint", "active", "hints", "text"),
            };
            let items = p[array]
                .as_array()
                .filter(|a| !a.is_empty())
                .cloned()
                .unwrap_or_else(|| vec![p.clone()]);
            if items.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "empty node batch"));
            }
            let mut out = vec![];
            let mut errors = serde_json::Map::new();
            let batch_mode = p[array].as_array().is_some_and(|a| !a.is_empty());
            for (i, item) in items.into_iter().enumerate() {
                let prior = b.batch.clone();
                let outcome = (|| -> Result<String> {
                    let content = required(&item, key)?.trim();
                    let priority = if matches!(kind, "goal" | "hint") {
                        0
                    } else {
                        item["priority"].as_i64().filter(|n| *n != 0).unwrap_or(5)
                    };
                    if !(0..=10).contains(&priority) {
                        return Err(Error::new("INVALID_PARAMS", "priority must be 0..10"));
                    }
                    let mut payload = item.clone();
                    if kind == "hint" && !ids(&item, "traffic_refs").is_empty() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Traffic refs require typed locator objects",
                        ));
                    }
                    if kind == "hint"
                        && item["traffic_refs"]
                            .as_array()
                            .is_some_and(|a| !a.is_empty())
                        && !setting_bool(s, "agent_traffic_binding", false)
                    {
                        return Err(Error::new(
                            "FEATURE_DISABLED",
                            "Agent traffic binding is disabled",
                        ));
                    }
                    if kind == "hint" {
                        if let Some(refs) = item["traffic_refs"].as_array() {
                            let mut normalized = vec![];
                            let mut seen = BTreeSet::new();
                            for r in refs {
                                let traffic = required(r, "traffic_id")?.trim();
                                let role = if text(r, "role").is_empty() {
                                    "supporting"
                                } else {
                                    text(r, "role")
                                };
                                if !matches!(
                                    role,
                                    "baseline" | "proof" | "verification" | "supporting"
                                ) {
                                    return Err(Error::new(
                                        "INVALID_PARAMS",
                                        "Invalid traffic usage",
                                    ));
                                }
                                if seen.insert(traffic.to_owned()) {
                                    normalized.push(json!({"traffic_id":traffic,"role":role,"note":text(r,"note")}));
                                }
                            }
                            if !normalized.is_empty() {
                                payload["traffic_refs"] = json!(normalized);
                            }
                        }
                    }
                    payload[key] = json!(content);
                    if kind == "intent" {
                        for aid in ids(&item, "asset_ids") {
                            let a = record(s, "assets", &aid)?;
                            assets::check_asset_gate(&mut b, Some(task), &a.data, true)?;
                        }
                    }
                    if let Some(o) = payload.as_object_mut() {
                        for f in [
                            "asset_ids",
                            "parent_ids",
                            "intent_id",
                            "task_id",
                            "priority",
                        ] {
                            o.remove(f);
                        }
                    }
                    let origin = if kind == "hint" {
                        "human"
                    } else if kind == "intent" {
                        "planner"
                    } else {
                        c.actor
                            .agent_id
                            .as_deref()
                            .and_then(|id| s.get("agents", id))
                            .map(|a| text(&a.data, "key"))
                            .filter(|v| !v.is_empty())
                            .unwrap_or("system")
                    };
                    let id=b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":kind,"state":state,"priority":priority,"payload":payload,"owner":c.actor.agent_id,"origin":origin}))?;
                    anchors(&mut b, &id, &item)?;
                    if kind == "goal" {
                        if let Some(origin) = s.records("exploration_nodes").into_iter().find(|n| {
                            n.data["exploration_id"] == exp
                                && n.data["kind"] == "fact"
                                && n.data["state"] == "origin"
                        }) {
                            edge(&mut b, &exp, &origin.header.id, &id, "spawns")?;
                        }
                    }
                    if kind == "intent" {
                        let mut parents = ids(&item, "parent_ids");
                        if parents.is_empty() {
                            if let Some(n) = s.records("exploration_nodes").into_iter().find(|r| {
                                text(&r.data, "exploration_id") == exp
                                    && text(&r.data, "kind") == "fact"
                                    && text(&r.data, "state") == "origin"
                            }) {
                                parents.push(n.header.id.clone());
                            }
                        }
                        for parent in parents {
                            let n = visible_node(s, task, &parent)?;
                            if !matches!(text(&n.data, "kind"), "fact" | "finding") {
                                return Err(Error::new(
                                    "INVALID_PARAMS",
                                    "intent parents must be facts or findings",
                                ));
                            }
                            if text(&n.data, "exploration_id") == exp {
                                edge(&mut b, &exp, &parent, &id, "derived_from")?;
                            } else {
                                let mut d = b.data("exploration_nodes", &id)?;
                                let mut inherited = ids(&d["payload"], "source_parent_ids");
                                inherited.push(parent);
                                d["payload"]["source_parent_ids"] = json!(inherited);
                                b.put("exploration_nodes", &id, d);
                            }
                        }
                    }
                    if kind == "fact" {
                        let intent = item["intent_id"]
                            .as_str()
                            .or(p["intent_id"].as_str())
                            .or(c.intent_id.as_deref());
                        if let Some(intent) = intent {
                            let n = own_node(s, task, intent)?;
                            if text(&n.data, "kind") != "intent" {
                                return Err(Error::new("INVALID_PARAMS", "intent node required"));
                            }
                            edge(&mut b, &exp, intent, &id, "yields")?;
                        }
                    }
                    Ok(id)
                })();
                match outcome {
                    Ok(id) => out.push(json!(id)),
                    Err(e) => {
                        if !batch_mode {
                            return Err(e);
                        }
                        b.batch = prior;
                        out.push(Value::Null);
                        errors.insert(i.to_string(), json!({"code":e.code,"message":e.message}));
                    }
                }
            }
            if out.iter().any(|id| !id.is_null())
                && c.actor
                    .agent_id
                    .as_deref()
                    .and_then(|id| s.get("agents", id))
                    .is_some_and(|a| a.data["key"] == "mainagent")
            {
                task_control_ui::revive(&mut b, task, c)?;
            }
            Ok(b.done(json!({"ids":out,"errors":errors})))
        }
        "set_constraints" => {
            let items = p["constraints"]
                .as_array()
                .filter(|a| !a.is_empty())
                .cloned()
                .unwrap_or_else(|| vec![p.clone()]);
            let mut out = vec![];
            let mut errors = serde_json::Map::new();
            let batch = p["constraints"].as_array().is_some_and(|a| !a.is_empty());
            for (index, item) in items.iter().enumerate() {
                let outcome = (|| -> Result<String> {
                    let txt = required(item, "text")?.trim();
                    let kind = text(item, "type").trim().to_lowercase();
                    let kind = if kind.is_empty() {
                        "deny"
                    } else {
                        kind.as_str()
                    };
                    if !matches!(kind, "allow" | "deny") {
                        return Err(Error::new("INVALID_PARAMS", "type must be allow or deny"));
                    }
                    let origin = c
                        .actor
                        .agent_id
                        .as_deref()
                        .and_then(|id| s.get("agents", id))
                        .map(|a| text(&a.data, "key"))
                        .filter(|v| !v.is_empty())
                        .unwrap_or("system");
                    b.insert(
                        "task_constraints",
                        "constraint",
                        json!({"exploration_id":exp,"kind":kind,"text":txt,"origin":origin}),
                    )
                })();
                match outcome {
                    Ok(id) => out.push(json!(id)),
                    Err(e) => {
                        if !batch {
                            return Err(e);
                        }
                        out.push(Value::Null);
                        errors.insert(index.to_string(), json!(e.message));
                    }
                }
            }
            let mut result = json!({"ids":out});
            if !errors.is_empty() {
                result["errors"] = json!(errors);
            }
            Ok(b.done(result))
        }
        "prove_goal" => {
            let id = required(p, "goal_id")?;
            let proof = required(p, "evidence_id")?;
            let mut d = own_node(s, task, id)?.data.clone();
            let e = visible_node(s, task, proof)?;
            if text(&d, "kind") != "goal"
                || text(&d, "state") != "open"
                || !matches!(text(&e.data, "kind"), "fact" | "finding")
                || text(&e.data, "state") == "dismissed"
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "open goal and confirmed fact/finding proof required",
                ));
            }
            d["state"] = json!("met");
            d["completed_at"] = json!(artex_core::now());
            d["payload"]["proof_reason"] = p["reason"].clone();
            d["payload"]["proof_node_id"] = json!(proof);
            b.put("exploration_nodes", id, d);
            if text(&e.data, "exploration_id") == exp {
                edge(&mut b, &exp, proof, id, "proves")?;
            }
            let goals = s
                .records("exploration_nodes")
                .into_iter()
                .filter(|n| {
                    text(&n.data, "exploration_id") == exp && text(&n.data, "kind") == "goal"
                })
                .collect::<Vec<_>>();
            let all_met = !goals.is_empty()
                && goals
                    .iter()
                    .all(|n| n.header.id == id || text(&n.data, "state") == "met");
            if all_met {
                let mut td = record(s, "tasks", task)?.data.clone();
                td["status"] = json!("done");
                td["completed_at"] = json!(artex_core::now());
                b.put("tasks", task, td);
                let mut ed = record(s, "explorations", &exp)?.data.clone();
                ed["status"] = json!("achieved");
                b.put("explorations", &exp, ed);
            }
            Ok(b.done(json!({"goal_id":id,"state":"met","all_goals_met":all_met})))
        }
        "goal_met" => {
            required(p, "reason")?;
            let mut d = record(s, "tasks", task)?.data.clone();
            d["status"] = json!("done");
            d["completed_at"] = json!(artex_core::now());
            b.put("tasks", task, d);
            let mut e = record(s, "explorations", &exp)?.data.clone();
            e["status"] = json!("achieved");
            b.put("explorations", &exp, e);
            Ok(b.done(json!({"task_id":task,"status":"done"})))
        }
        "kill_work" | "steer_work" => {
            let id = p["intent_id"]
                .as_str()
                .or(p["work_id"].as_str())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "intent_id required"))?;
            let mut d = own_node(s, task, id)?.data.clone();
            if text(&d, "kind") != "intent" {
                return Err(Error::new("INVALID_PARAMS", "intent node required"));
            }
            let active = s
                .records("agent_runs")
                .into_iter()
                .filter(|r| {
                    text(&r.data, "intent_id") == id
                        && matches!(text(&r.data, "state"), "running" | "waiting" | "queued")
                })
                .collect::<Vec<_>>();
            if m == "kill_work" {
                d["state"] = json!("stopped");
                d["completed_at"] = json!(artex_core::now());
                b.put("exploration_nodes", id, d);
                for run in active {
                    conversation::cancel_run(&mut b, c, &run.header.id)?;
                }
                Ok(b.done(json!({"intent_id":id,"state":"stopped"})))
            } else {
                if active.len() != 1 {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "steering requires exactly one live run",
                    ));
                }
                let txt = p["message"]
                    .as_str()
                    .or(p["text"].as_str())
                    .filter(|x| !x.trim().is_empty())
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "steering message required"))?;
                let run = &active[0].header.id;
                let ordinal = s
                    .records("steering_messages")
                    .iter()
                    .filter(|r| text(&r.data, "target_run_id") == run)
                    .filter_map(|r| {
                        r.data["ordinal"]
                            .as_str()
                            .and_then(|x| x.parse::<u64>().ok())
                    })
                    .max()
                    .unwrap_or(0)
                    + 1;
                let sid=b.insert("steering_messages","steering",json!({"task_id":task,"intent_id":id,"target_run_id":run,"text":txt,"ordinal":ordinal.to_string(),"state":"pending"}))?;
                Ok(b.done(json!({"steering_id":sid,"target_run_id":run})))
            }
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    let task = task_id(p, c)?;
    let exp = exploration_id(s, task)?;
    let visible = visible_explorations(s, task);
    match m {
        "graph_overview" | "get_task_graph" => {
            let nodes = s
                .records("exploration_nodes")
                .iter()
                .filter(|r| visible.contains(text(&r.data, "exploration_id")))
                .map(|r| {
                    let mut d = r.data.clone();
                    d["read_only"] = json!(text(&d, "exploration_id") != exp);
                    d
                })
                .collect::<Vec<_>>();
            let edges = s
                .records("exploration_edges")
                .iter()
                .filter(|r| visible.contains(text(&r.data, "exploration_id")))
                .map(|r| r.data.clone())
                .collect::<Vec<_>>();
            Ok(
                json!({"nodes":nodes,"edges":edges,"source_task_ids":direct_sources(s,task).into_iter().filter(|t|t!=task).collect::<Vec<_>>() }),
            )
        }
        "node_detail" | "get_task_node_detail" | "expand_digest" => {
            let id = p["node_id"]
                .as_str()
                .or(p["id"].as_str())
                .or(p["digest_id"].as_str())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "node_id required"))?;
            let r = visible_node(s, task, id)?;
            let mut d = r.data.clone();
            d["read_only"] = json!(text(&d, "exploration_id") != exp);
            if let Some(f) = s
                .records("findings")
                .into_iter()
                .find(|f| text(&f.data, "node_id") == id)
            {
                d["finding_id"] = json!(f.header.id);
            }
            if m == "expand_digest" {
                if text(&d, "kind") != "digest" {
                    return Err(Error::new("INVALID_PARAMS", "digest node required"));
                }
                let members = s
                    .records("exploration_edges")
                    .iter()
                    .filter(|e| text(&e.data, "src_id") == id && text(&e.data, "rel") == "covers")
                    .map(|e| text(&e.data, "dst_id").to_owned())
                    .collect::<Vec<_>>();
                return Ok(
                    json!({"id":id,"state":d["state"],"body":d["payload"]["body"],"members":members.iter().map(|id|visible_node(s,task,id).map(|r|r.data.clone())).collect::<Result<Vec<_>>>()?}),
                );
            }
            Ok(d)
        }
        "list_facts" | "list_goals" => {
            let kind = if m == "list_facts" { "fact" } else { "goal" };
            let q = text(p, "q").to_lowercase();
            let mut rows = s
                .records("exploration_nodes")
                .into_iter()
                .filter(|r| {
                    text(&r.data, "kind") == kind
                        && if kind == "goal" {
                            text(&r.data, "exploration_id") == exp
                        } else {
                            visible.contains(text(&r.data, "exploration_id"))
                        }
                })
                .filter(|r| {
                    text(&r.data["payload"], "summary")
                        .to_lowercase()
                        .contains(&q)
                })
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                b.header
                    .created_at
                    .cmp(&a.header.created_at)
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            let total = rows.len();
            if !text(p, "before").is_empty() {
                let id = text(p, "before");
                let index = rows
                    .iter()
                    .position(|r| r.header.id == id)
                    .ok_or_else(|| Error::new("NOT_FOUND", "fact cursor no longer visible"))?;
                rows = rows.into_iter().skip(index + 1).collect();
            }
            let limit = p["limit"]
                .as_u64()
                .filter(|n| *n > 0)
                .unwrap_or(20)
                .min(100) as usize;
            let has_more = rows.len() > limit;
            rows.truncate(limit);
            let next = if has_more {
                rows.last().map(|r| r.header.id.clone())
            } else {
                None
            };
            let out=rows.iter().map(|r|{let summary=text(&r.data["payload"],"summary");let inherited=text(&r.data,"exploration_id")!=exp;let mut d=json!({"id":r.header.id,"kind":kind,"state":r.data["state"],"summary":summary.chars().take(160).collect::<String>(),"summary_truncated":summary.chars().count()>160,"inherited":inherited,"read_only":inherited});if inherited{d["source_task_id"]=s.records("tasks").into_iter().find(|t|t.data["exploration_id"]==r.data["exploration_id"]).map(|t|json!(t.header.id)).unwrap_or(Value::Null);}d}).collect::<Vec<_>>();
            if kind == "goal" {
                Ok(json!(out))
            } else {
                Ok(json!({"facts":out,"total":total,"has_more":has_more,"next_before":next}))
            }
        }
        "get_worker_output"
        | "get_worker_trace"
        | "get_task_worker_trace"
        | "list_task_worker_traces"
        | "search_all_worker_traces"
        | "search_task_worker_traces" => {
            let intent = text(p, "intent_id");
            if !intent.is_empty() {
                let n = visible_node(s, task, intent)?;
                if text(&n.data, "kind") != "intent" {
                    return Err(Error::new("INVALID_PARAMS", "intent node required"));
                }
                if m == "get_worker_output" {
                    if text(&n.data, "exploration_id") != exp
                        && !matches!(
                            text(&n.data, "state"),
                            "done" | "blocked" | "exhausted" | "stopped"
                        )
                    {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "Inherited worker output is available only for a terminal intent",
                        ));
                    }
                    return Ok(
                        json!({"intent_id":intent,"state":n.data["state"],"results":s.records("exploration_edges").iter().filter(|e|text(&e.data,"src_id")==intent&&text(&e.data,"rel")=="yields").filter_map(|e|s.get("exploration_nodes",text(&e.data,"dst_id"))).map(|r|r.data.clone()).collect::<Vec<_>>() }),
                    );
                }
            }
            let q = text(p, "q").to_lowercase();
            let steps = ids(p, "step_ids");
            let selected = steps.iter().take(5).collect::<BTreeSet<_>>();
            let mut rows = vec![];
            for r in s.records("activity") {
                if !visible.contains(text(&r.data, "exploration_id"))
                    || (!intent.is_empty() && text(&r.data, "node_id") != intent)
                    || (!steps.is_empty() && !selected.contains(&r.header.id))
                {
                    continue;
                }
                let inherited = text(&r.data, "exploration_id") != exp;
                if inherited {
                    if matches!(text(&r.data, "kind"), "thinking" | "usage") {
                        continue;
                    }
                    let node = s.get("exploration_nodes", text(&r.data, "node_id"));
                    if !node.is_some_and(|n| {
                        matches!(
                            text(&n.data, "state"),
                            "done" | "blocked" | "exhausted" | "stopped"
                        )
                    }) {
                        continue;
                    }
                }
                if !r.data.to_string().to_lowercase().contains(&q) {
                    continue;
                }
                let mut d = r.data.clone();
                if inherited {
                    if let Some(o) = d.as_object_mut() {
                        for field in [
                            "input_tokens",
                            "output_tokens",
                            "cache_read_tokens",
                            "cache_write_tokens",
                        ] {
                            o.remove(field);
                        }
                    }
                }
                rows.push(d);
            }
            rows.truncate(p["limit"].as_u64().unwrap_or(100).min(1000) as usize);
            Ok(
                json!({"steps":rows,"omitted_step_ids":steps.into_iter().skip(5).collect::<Vec<_>>() }),
            )
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}

// Source DSL: explicit AND binds tighter than OR, = substring and == exact.
#[derive(Debug)]
pub enum Dsl {
    Leaf(String, String, String),
    And(Box<Dsl>, Box<Dsl>),
    Or(Box<Dsl>, Box<Dsl>),
}
impl Dsl {
    pub fn parse(input: &str) -> Result<Self> {
        let mut toks = vec![];
        let mut current = String::new();
        let mut quote = None;
        for ch in input.chars() {
            if let Some(q) = quote {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                continue;
            }
            if ch.is_whitespace() || matches!(ch, '(' | ')') {
                if !current.is_empty() {
                    toks.push(std::mem::take(&mut current));
                }
                if matches!(ch, '(' | ')') {
                    toks.push(ch.to_string());
                }
            } else {
                current.push(ch);
            }
        }
        if quote.is_some() {
            return Err(Error::new("INVALID_PARAMS", "unterminated DSL quote"));
        }
        if !current.is_empty() {
            toks.push(current);
        }
        let mut pos = 0;
        let result = Self::or(&toks, &mut pos)?;
        if pos != toks.len() {
            return Err(Error::new("INVALID_PARAMS", "unexpected DSL token"));
        }
        Ok(result)
    }
    fn or(t: &[String], p: &mut usize) -> Result<Self> {
        let mut left = Self::and(t, p)?;
        while t.get(*p).is_some_and(|x| x.eq_ignore_ascii_case("OR")) {
            *p += 1;
            left = Self::Or(Box::new(left), Box::new(Self::and(t, p)?));
        }
        Ok(left)
    }
    fn and(t: &[String], p: &mut usize) -> Result<Self> {
        let mut left = Self::atom(t, p)?;
        while t.get(*p).is_some_and(|x| x.eq_ignore_ascii_case("AND")) {
            *p += 1;
            left = Self::And(Box::new(left), Box::new(Self::atom(t, p)?));
        }
        Ok(left)
    }
    fn atom(t: &[String], p: &mut usize) -> Result<Self> {
        let x = t
            .get(*p)
            .ok_or_else(|| Error::new("INVALID_PARAMS", "incomplete DSL"))?;
        *p += 1;
        if x == "(" {
            let n = Self::or(t, p)?;
            if t.get(*p).map(String::as_str) != Some(")") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "missing DSL closing parenthesis",
                ));
            }
            *p += 1;
            return Ok(n);
        }
        if matches!(x.as_str(), ")" | "AND" | "OR") {
            return Err(Error::new("INVALID_PARAMS", "unexpected DSL operator"));
        }
        let mut token = x.clone();
        if t.get(*p)
            .is_some_and(|x| ["=", "==", "!=", ">", ">=", "<", "<="].contains(&x.as_str()))
        {
            token.push_str(&t[*p]);
            *p += 1;
            token.push_str(
                t.get(*p)
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "missing DSL value"))?,
            );
            *p += 1;
        }
        for op in ["==", "!=", ">=", "<=", "=", ">", "<"] {
            if let Some(i) = token.find(op) {
                let field = token[..i].to_lowercase();
                let value = token[i + op.len()..].to_owned();
                if value.is_empty() {
                    return Err(Error::new("INVALID_PARAMS", "empty DSL value"));
                }
                let known = [
                    "domain",
                    "root_domain",
                    "ip",
                    "url",
                    "page_title",
                    "title",
                    "icp",
                    "service_name",
                    "app_name",
                    "bundle_id",
                    "category",
                    "app_icp",
                    "method",
                    "service_type",
                    "record_type",
                    "technology",
                    "technologies",
                    "tech",
                    "port",
                    "status_code",
                    "status",
                    "company_id",
                    "task_id",
                ];
                if !known.contains(&field.as_str()) {
                    return Err(Error::new("INVALID_PARAMS", "unknown DSL field"));
                }
                return Ok(Self::Leaf(field, op.into(), value));
            }
        }
        Ok(Self::Leaf(String::new(), "=".into(), token))
    }
    pub fn matches(&self, d: &Value) -> Result<bool> {
        match self {
            Self::And(a, b) => Ok(a.matches(d)? && b.matches(d)?),
            Self::Or(a, b) => Ok(a.matches(d)? || b.matches(d)?),
            Self::Leaf(f, op, v) => {
                if f.is_empty() {
                    return Ok(d.to_string().to_lowercase().contains(&v.to_lowercase()));
                }
                let key = match f.as_str() {
                    "title" => "page_title",
                    "technology" | "tech" => "technologies",
                    "status" => "status_code",
                    _ => f,
                };
                let value = &d[key];
                if matches!(key, "port" | "status_code") {
                    let want = v.parse::<i64>().map_err(|_| {
                        Error::new("INVALID_PARAMS", "numeric DSL field requires integer")
                    })?;
                    let Some(have) = value.as_i64() else {
                        return Ok(false);
                    };
                    return Ok(match op.as_str() {
                        "=" | "==" => have == want,
                        "!=" => have != want,
                        ">" => have > want,
                        ">=" => have >= want,
                        "<" => have < want,
                        "<=" => have <= want,
                        _ => false,
                    });
                }
                if matches!(op.as_str(), ">" | ">=" | "<" | "<=") {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "ordering operator requires numeric field",
                    ));
                }
                let vals = if let Some(a) = value.as_array() {
                    a.iter().filter_map(Value::as_str).collect::<Vec<_>>()
                } else {
                    vec![value.as_str().unwrap_or("")]
                };
                let hit = vals.iter().any(|a| {
                    if op == "==" {
                        *a == v
                    } else {
                        a.to_lowercase().contains(&v.to_lowercase())
                    }
                });
                Ok(if op == "!=" { !hit } else { hit })
            }
        }
    }
}
