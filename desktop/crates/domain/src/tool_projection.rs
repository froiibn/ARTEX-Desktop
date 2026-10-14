//! Source-compatible tool views; persistent records retain the richer desktop DTOs.
use crate::*;
/// Canonical tool responses retain source batch ordering; typed IDs/null replace legacy IDs/zero.
pub fn mutation_result(name: &str, input: &Value, result: &Value) -> Value {
    match name {
        "create_custom_tool" => {
            return json!(format!("custom tool created: {}", text(result, "key")));
        }
        "update_custom_tool" => {
            return json!(format!("custom tool updated: {}", text(result, "key")));
        }
        "create_mcp" => {
            return json!(format!(
                "mcp created: id={} name={}",
                text(result, "id"),
                text(input, "name")
            ));
        }
        "update_mcp" => return json!(format!("mcp updated: id={}", text(result, "id"))),
        "update_finding_report" => {
            return json!(format!(
                "finding {} report updated ({} chars)",
                text(result, "finding_node_id"),
                text(input, "report").len()
            ));
        }
        "spawn_task" => return json!(format!("task created: {}", text(result, "task_id"))),
        "pause_task" => return json!(format!("task paused: {}", text(result, "task_id"))),
        "goal_met" => return json!("acknowledged: goal marked met"),
        "prove_goal" => {
            return json!(format!(
                "goal {} marked met{}",
                text(result, "goal_id"),
                if result["all_goals_met"] == true {
                    "; all task goals achieved, task completed"
                } else {
                    ""
                }
            ));
        }
        "kill_work" => {
            return json!(format!(
                "Sent termination signal to intent {}",
                text(result, "intent_id")
            ));
        }
        "steer_work" => {
            return json!(format!(
                "Injected corrective instructions into intent {} (effective at the next step)",
                text(input, "intent_id")
            ));
        }
        _ => {}
    }
    if name == "insert_assets" {
        let mut rows = vec![];
        let mut errors = vec![];
        for (index, id) in result["ids"].as_array().into_iter().flatten().enumerate() {
            if id.is_null() {
                let e = &result["errors"][index.to_string()];
                errors.push(json!({"index":index,"error":e["message"].as_str().unwrap_or("Asset insertion failed")}));
            } else {
                rows.push(json!({"index":index,"id":id,"type":input["assets"][index]["type"]}));
            }
        }
        return json!({"results":rows,"errors":errors});
    }
    if name == "report_finding" {
        let mut detail = result.clone();
        detail.as_object_mut().map(|o| o.remove("node_id"));
        return json!(format!(
            "finding recorded: {}\n{}",
            text(result, "finding_node_id"),
            detail
        ));
    }
    if name == "record_finding_retest_result" {
        return json!({"saved":true,"verdict":result["verdict"]});
    }
    let (array, label) = match name {
        "add_intent" => ("intents", "intent created"),
        "record_fact" => ("facts", "fact recorded"),
        "set_goals" => ("goals", "goal added"),
        "add_hint" | "add_task_hint" => ("hints", "hint added"),
        "set_constraints" => ("constraints", "constraint added"),
        _ => return result.clone(),
    };
    if input[array].as_array().is_none_or(|a| a.is_empty()) {
        return json!(format!(
            "{label}: {}",
            result["ids"][0].as_str().unwrap_or("")
        ));
    }
    let mut out = json!({"ids":result["ids"]});
    let errors = result["errors"]
        .as_object()
        .map(|errors| {
            errors
                .iter()
                .map(|(index, e)| {
                    (
                        index.clone(),
                        if e.is_string() {
                            e.clone()
                        } else {
                            e["message"].clone()
                        },
                    )
                })
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    if !errors.is_empty() {
        out["errors"] = json!(errors);
    }
    out
}
use crate::traffic_ui::BlobReader;
use artex_core::Actor;
pub const QUERIES: &[&str] = &[
    "list_facts",
    "list_findings",
    "list_task_findings",
    "list_goals",
    "node_detail",
    "get_task_node_detail",
    "expand_digest",
    "get_worker_output",
    "get_worker_trace",
    "get_task_worker_trace",
    "list_task_worker_traces",
    "search_all_worker_traces",
    "search_task_worker_traces",
    "graph_overview",
    "get_task_graph",
    "list_tasks",
    "list_llm_profiles",
    "list_companies",
    "list_assets",
    "list_untested_assets",
    "get_finding_retest_context",
];
fn ordered<'a>(mut rows: Vec<&'a Record>, desc: bool) -> Vec<&'a Record> {
    rows.sort_by_key(|r| {
        (
            r.header.commit_seq.parse::<u64>().unwrap_or(0),
            r.header.created_at.clone(),
            r.header.id.clone(),
        )
    });
    if desc {
        rows.reverse();
    }
    rows
}
fn plain(v: &Value, reader: &BlobReader) -> Result<Value> {
    if v.is_object() && v.get("text").is_some() && v.get("blob").is_some() {
        return Ok(json!(String::from_utf8_lossy(&traffic_ui::body_bytes(
            v, reader
        )?)));
    }
    match v {
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), plain(v, reader)?)))
                .collect::<Result<_>>()?,
        )),
        Value::Array(a) => Ok(Value::Array(
            a.iter().map(|v| plain(v, reader)).collect::<Result<_>>()?,
        )),
        v => Ok(v.clone()),
    }
}
fn mark(mut d: Value, s: &Snapshot, task: &str, r: &Record) -> Value {
    if let Some(t) = s
        .records("tasks")
        .into_iter()
        .find(|t| t.data["exploration_id"] == r.data["exploration_id"])
    {
        if t.header.id != task {
            d["inherited"] = json!(true);
            d["source_task_id"] = json!(t.header.id);
        }
    }
    d
}
fn compact(r: &Record) -> Value {
    json!({"id":r.header.id,"state":r.data["state"],"summary":r.data["payload"]["summary"]})
}
fn finding(r: &Record, s: &Snapshot, task: &str) -> Value {
    let mut d = compact(r);
    for k in ["severity", "vulnclass"] {
        if !r.data["payload"][k].is_null() && r.data["payload"][k] != "" {
            d[k] = r.data["payload"][k].clone();
        }
    }
    let own = s
        .records("tasks")
        .into_iter()
        .find(|t| t.data["exploration_id"] == r.data["exploration_id"]);
    d["task_id"] = own.map(|t| json!(t.header.id)).unwrap_or(Value::Null);
    if let Some(f) = s
        .records("findings")
        .into_iter()
        .find(|f| f.data["node_id"] == r.header.id)
    {
        d["finding_id"] = json!(f.header.id);
        d["finding_node_id"] = json!(r.header.id);
        d["traffic_count"] = json!(
            s.records("finding_traffic_bindings")
                .iter()
                .filter(|b| b.data["finding_id"] == f.header.id)
                .count()
        );
    }
    if let Some(e) = s
        .records("exploration_edges")
        .into_iter()
        .find(|e| e.data["dst_id"] == r.header.id && e.data["rel"] == "yields")
    {
        d["intent_id"] = e.data["src_id"].clone();
    }
    mark(d, s, task, r)
}
fn full_node(r: &Record, s: &Snapshot, task: &str, reader: &BlobReader) -> Result<Value> {
    let mut d = json!({"id":r.header.id,"kind":r.data["kind"],"payload":plain(&r.data["payload"],reader)?,"priority":r.data["priority"],"state":r.data["state"],"created_at":r.header.created_at});
    for k in ["origin", "owner", "blocked_reason", "delete_reason"] {
        if !r.data[k].is_null() && r.data[k] != "" {
            d[k] = r.data[k].clone();
        }
    }
    let anchors = s
        .records("exploration_anchors")
        .into_iter()
        .filter(|a| a.data["node_id"] == r.header.id)
        .map(|a| a.data["asset_id"].clone())
        .collect::<Vec<_>>();
    if !anchors.is_empty() {
        d["anchors"] = json!(anchors);
    }
    if r.data["kind"] == "finding" {
        let f = finding(r, s, task);
        for k in ["finding_id", "finding_node_id", "traffic_count"] {
            if !f[k].is_null() {
                d[k] = f[k].clone();
            }
        }
    }
    Ok(mark(d, s, task, r))
}
fn activities<'a>(s: &'a Snapshot, task: &str, intent: Option<&str>) -> Vec<&'a Record> {
    let exp = exploration_id(s, task).unwrap_or_default();
    let visible = visible_explorations(s, task);
    ordered(
        s.records("activity")
            .into_iter()
            .filter(|a| {
                if !visible.contains(text(&a.data, "exploration_id"))
                    || !conversation::public_activity_kind(text(&a.data, "kind"))
                    || matches!(text(&a.data, "kind"), "thinking" | "usage")
                    || intent.is_some_and(|i| a.data["node_id"] != i)
                {
                    return false;
                }
                if a.data["exploration_id"] != exp {
                    return a.data["node_id"]
                        .as_str()
                        .and_then(|id| s.get("exploration_nodes", id))
                        .is_some_and(|n| {
                            n.data["kind"] == "intent"
                                && matches!(
                                    text(&n.data, "state"),
                                    "done" | "blocked" | "exhausted" | "stopped"
                                )
                        });
                }
                true
            })
            .collect(),
        false,
    )
}
fn detail(r: &Record, reader: &BlobReader) -> Result<String> {
    let b = transcript_ui::public_body(&r.data["detail"], reader)?;
    Ok(String::from_utf8_lossy(&traffic_ui::body_bytes(&b, reader)?).into_owned())
}
fn step(r: &Record, s: &Snapshot, task: &str, full: bool, reader: &BlobReader) -> Result<Value> {
    let mut d = json!({"step_id":r.header.id,"kind":r.data["kind"],"tool":text(&r.data,"tool"),"is_error":r.data["is_error"].as_bool().unwrap_or(false)});
    if full {
        d["detail"] = json!(detail(r, reader)?);
    } else {
        d["summary"] = json!(
            text(&r.data, "summary")
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(100)
                .collect::<String>()
        );
    }
    Ok(mark(d, s, task, r))
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context, reader: &BlobReader) -> Result<Value> {
    if m == "get_finding_retest_context" {
        let conv = c
            .conversation_id
            .as_deref()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Retest conversation required"))?;
        let rt = s
            .records("finding_retests")
            .into_iter()
            .find(|r| r.data["conversation_id"] == conv)
            .ok_or_else(|| {
                Error::new("PERMISSION_DENIED", "Conversation is not bound to a retest")
            })?;
        let f = record(s, "findings", required(&rt.data, "finding_id")?)?;
        let exp = f.data["task_id"]
            .as_str()
            .and_then(|id| s.get("tasks", id))
            .map(|t| text(&t.data, "exploration_id"))
            .unwrap_or("");
        let constraints = ordered(
            s.records("task_constraints")
                .into_iter()
                .filter(|r| r.data["exploration_id"] == exp)
                .collect(),
            false,
        )
        .iter()
        .map(|r| r.data.clone())
        .collect::<Vec<_>>();
        return Ok(json!({"retest":plain(&rt.data,reader)?,"current_constraints":constraints}));
    }
    if m == "list_companies" {
        let search = text(p, "search").trim().to_lowercase();
        let companies=ordered(s.records("companies"),false).iter().filter(|r|text(&r.data,"name").to_lowercase().contains(&search)).map(|r|json!({"id":r.header.id,"name":r.data["name"],"asset_count":s.records("assets").iter().filter(|a|a.data["company_id"]==r.header.id).count(),"scope":ordered(s.records("company_scope").into_iter().filter(|scope|scope.data["company_id"]==r.header.id).collect(),false).iter().map(|scope|scope.data["raw"].clone()).collect::<Vec<_>>()})).collect::<Vec<_>>();
        return Ok(json!({"count":companies.len(),"companies":companies}));
    }
    if m == "list_assets" {
        let selected = !text(p, "id").is_empty() || !ids(p, "ids").is_empty();
        let mut parameters = p.clone();
        if selected {
            parameters.as_object_mut().unwrap().remove("dsl");
            parameters.as_object_mut().unwrap().remove("type");
            parameters["offset"] = json!(0);
            parameters["limit"] = json!(1000);
        }
        let rows = assets::query(m, &parameters, s, c)?;
        return Ok(
            json!({"count":rows["assets"].as_array().map(Vec::len).unwrap_or(0),"assets":plain(&rows["assets"],reader)?}),
        );
    }
    if m == "list_untested_assets" {
        let task = task_id(p, c)?;
        let (target, tested) = asset_graph_ui::scoped_coverage(s, task)?;
        let rows = ordered(
            s.records("assets")
                .into_iter()
                .filter(|a| {
                    target.contains(&a.header.id)
                        && !tested.contains(&a.header.id)
                        && (text(p, "type").is_empty() || a.data["type"] == p["type"])
                })
                .collect(),
            true,
        );
        let page = p["page"].as_u64().filter(|v| *v > 0).unwrap_or(1);
        let size = p["page_size"].as_u64().filter(|v| *v > 0).unwrap_or(10);
        return Ok(
            json!({"assets":rows.iter().skip(page.saturating_sub(1).saturating_mul(size)as usize).take(size as usize).map(|r|exploration_ui::asset_ref(r)).collect::<Vec<_>>(),"total":rows.len(),"page":page,"page_size":size}),
        );
    }
    if m == "list_llm_profiles" {
        return Ok(
            json!({"profiles":ordered(s.records("llm_profiles"),false).iter().map(|r|json!({"id":r.header.id,"name":r.data["name"],"model":r.data["model"],"format":r.data["format"],"is_active":r.data["is_default"]})).collect::<Vec<_>>()}),
        );
    }
    if m == "list_tasks" {
        let rows = ordered(
            s.records("tasks")
                .into_iter()
                .filter(|t| t.data["deleted_at"].is_null() && t.data["archived_at"].is_null())
                .collect(),
            true,
        );
        return Ok(json!(rows.iter().map(|t|{let created=chrono::DateTime::parse_from_rfc3339(&t.header.created_at).map(|t|t.timestamp()).unwrap_or(0);let end=if t.data["status"]=="running"{chrono::Utc::now().timestamp()}else{s.records("activity").into_iter().filter(|a|a.data["exploration_id"]==t.data["exploration_id"]).filter_map(|a|chrono::DateTime::parse_from_rfc3339(&a.header.created_at).ok().map(|t|t.timestamp())).max().unwrap_or(created)};let profile=t.data["active_llm_profile_id"].as_str().or(t.data["llm_profile_id"].as_str());let mut row=json!({"id":t.header.id,"description":t.data["description"],"goal":t.data["goal"],"status":t.data["status"],"run_seconds":end.saturating_sub(created).max(0),"llm_profile":profile.and_then(|id|s.get("llm_profiles",id)).map(|p|text(&p.data,"name")).unwrap_or("(Activate configuration)")});if !text(&t.data,"parent_ref").is_empty(){row["parent_ref"]=t.data["parent_ref"].clone();}row}).collect::<Vec<_>>()));
    }
    let task = task_id(p, c)?;
    let exp = exploration_id(s, task)?;
    let visible = visible_explorations(s, task);
    match m {
        "list_facts" => {
            let q = text(p, "q").trim().to_lowercase();
            let mut rows = ordered(
                s.records("exploration_nodes")
                    .into_iter()
                    .filter(|n| {
                        n.data["kind"] == "fact"
                            && visible.contains(text(&n.data, "exploration_id"))
                            && text(&n.data["payload"], "summary")
                                .to_lowercase()
                                .contains(&q)
                    })
                    .collect(),
                true,
            );
            let total = rows.len();
            if !text(p, "before").is_empty() {
                let pos = rows
                    .iter()
                    .position(|n| n.header.id == text(p, "before"))
                    .ok_or_else(|| {
                        Error::new("NOT_FOUND", "Fact cursor is outside visible scope")
                    })?;
                rows = rows.into_iter().skip(pos + 1).collect();
            }
            let limit = p["limit"]
                .as_u64()
                .filter(|n| *n > 0)
                .unwrap_or(20)
                .min(100) as usize;
            let more = rows.len() > limit;
            rows.truncate(limit);
            let facts = rows
                .iter()
                .map(|n| {
                    let mut d = mark(compact(n), s, task, n);
                    let summary = text(&n.data["payload"], "summary");
                    if summary.chars().count() > 160 {
                        d["summary"] = json!(format!(
                            "{}…",
                            summary.chars().take(160).collect::<String>()
                        ));
                        d["summary_truncated"] = json!(true);
                    }
                    d
                })
                .collect::<Vec<_>>();
            let mut out = json!({"facts":facts,"total":total,"has_more":more});
            if more {
                out["next_before"] = json!(rows.last().unwrap().header.id);
            }
            Ok(out)
        }
        "graph_overview" | "get_task_graph" => overview(s, task, reader, true),
        "list_findings" | "list_task_findings" => Ok(json!(
            ordered(
                s.records("exploration_nodes")
                    .into_iter()
                    .filter(|n| n.data["kind"] == "finding"
                        && visible.contains(text(&n.data, "exploration_id")))
                    .collect(),
                true
            )
            .iter()
            .take(500)
            .map(|n| finding(n, s, task))
            .collect::<Vec<_>>()
        )),
        "list_goals" => Ok(Value::Array(
            ordered(
                s.records("exploration_nodes")
                    .into_iter()
                    .filter(|n| n.data["kind"] == "goal" && n.data["exploration_id"] == exp)
                    .collect(),
                true,
            )
            .iter()
            .take(100)
            .map(|n| full_node(n, s, task, reader))
            .collect::<Result<_>>()?,
        )),
        "node_detail" | "get_task_node_detail" | "expand_digest" => {
            let id = p["node_id"]
                .as_str()
                .or(p["id"].as_str())
                .or(p["digest_id"].as_str())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Node ID is required"))?;
            let n = visible_node(s, task, id)?;
            if m != "expand_digest" {
                return full_node(n, s, task, reader);
            }
            if n.data["kind"] != "digest" {
                return Err(Error::new("INVALID_PARAMS", "Digest node required"));
            }
            let members = ordered(
                s.records("exploration_edges")
                    .into_iter()
                    .filter(|e| e.data["src_id"] == id && e.data["rel"] == "covers")
                    .filter_map(|e| s.get("exploration_nodes", text(&e.data, "dst_id")))
                    .collect(),
                false,
            );
            let rows = members
                .iter()
                .map(|n| {
                    let mut d = compact(n);
                    if !n.data["payload"]["confidence"].is_null() {
                        d["confidence"] = n.data["payload"]["confidence"].clone();
                    }
                    mark(d, s, task, n)
                })
                .collect::<Vec<_>>();
            Ok(mark(
                json!({"id":id,"state":n.data["state"],"body":plain(&n.data["payload"]["body"],reader)?,"members":rows}),
                s,
                task,
                n,
            ))
        }
        "list_task_worker_traces" => {
            let q = text(p, "q").trim().to_lowercase();
            let rows = ordered(
                s.records("exploration_nodes")
                    .into_iter()
                    .filter(|n| {
                        n.data["kind"] == "intent"
                            && visible.contains(text(&n.data, "exploration_id"))
                            && matches!(
                                text(&n.data, "state"),
                                "running" | "done" | "blocked" | "exhausted" | "stopped"
                            )
                            && (n.data["exploration_id"] == exp || n.data["state"] != "running")
                            && text(&n.data["payload"], "summary")
                                .to_lowercase()
                                .contains(&q)
                    })
                    .collect(),
                true,
            );
            Ok(
                json!({"works":rows.iter().take(p["limit"].as_u64().filter(|n|*n>0).unwrap_or(50)as usize).map(|n|mark(json!({"intent_id":n.header.id,"state":n.data["state"],"summary":n.data["payload"]["summary"]}),s,task,n)).collect::<Vec<_>>()}),
            )
        }
        "get_worker_output" | "get_worker_trace" | "get_task_worker_trace" => {
            let intent = required(p, "intent_id")?;
            let n = visible_node(s, task, intent)?;
            if n.data["kind"] != "intent" {
                return Err(Error::new("INVALID_PARAMS", "Intent node required"));
            }
            if n.data["exploration_id"] != exp
                && !matches!(
                    text(&n.data, "state"),
                    "done" | "blocked" | "exhausted" | "stopped"
                )
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Inherited intent output is private until terminal",
                ));
            }
            let rows = activities(s, task, Some(intent));
            if m == "get_worker_output" {
                let pick = rows
                    .iter()
                    .rev()
                    .find(|a| a.data["kind"] == "result")
                    .or_else(|| rows.iter().rev().find(|a| a.data["kind"] == "text"));
                if let Some(a) = pick {
                    let output = detail(a, reader)?;
                    return Ok(mark(
                        json!({"intent_id":intent,"final_text":if output.is_empty(){text(&a.data,"summary")}else{&output},"summary":a.data["summary"],"is_error":a.data["is_error"].as_bool().unwrap_or(false)}),
                        s,
                        task,
                        n,
                    ));
                }
                return Ok(if n.data["exploration_id"] == exp {
                    json!("The work has no output yet")
                } else {
                    mark(
                        json!({"intent_id":intent,"final_text":"The work has no output yet"}),
                        s,
                        task,
                        n,
                    )
                });
            }
            let mut seen = BTreeSet::new();
            let steps = ids(p, "step_ids")
                .into_iter()
                .filter(|id| seen.insert(id.clone()))
                .collect::<Vec<_>>();
            let selected = steps.iter().take(5).collect::<BTreeSet<_>>();
            let full = !steps.is_empty();
            let q = text(p, "q").trim().to_lowercase();
            if full && !q.is_empty() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "q and step_ids are mutually exclusive",
                ));
            }
            let mut out = vec![];
            for a in rows {
                if full && !selected.contains(&a.header.id) {
                    continue;
                }
                if !q.is_empty()
                    && !text(&a.data, "summary").to_lowercase().contains(&q)
                    && !detail(a, reader)?.to_lowercase().contains(&q)
                {
                    continue;
                }
                out.push(step(a, s, task, full, reader)?);
                if !full
                    && out.len()
                        >= p["limit"]
                            .as_u64()
                            .filter(|n| *n > 0)
                            .unwrap_or(if q.is_empty() { 500 } else { 100 })
                            .min(1000) as usize
                {
                    break;
                }
            }
            let mut result = json!({"intent_id":intent,"steps":out});
            if full {
                result["returned_step_ids"] = json!(steps.iter().take(5).collect::<Vec<_>>());
                if steps.len() > 5 {
                    result["omitted_step_ids"] = json!(&steps[5..]);
                    result["notice"] = json!(
                        "At most five unique steps are returned; request omitted step IDs separately if needed"
                    );
                }
            }
            Ok(mark(result, s, task, n))
        }
        "search_all_worker_traces" | "search_task_worker_traces" => {
            let q = required(p, "q")?.trim();
            if q.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Search query is required"));
            }
            let lower = q.to_lowercase();
            let mut hits = vec![];
            for a in activities(s, task, None) {
                if c.intent_id.as_deref() == a.data["node_id"].as_str()
                    || a.data["node_id"].is_null()
                {
                    continue;
                }
                if !text(&a.data, "summary").to_lowercase().contains(&lower)
                    && !detail(a, reader)?.to_lowercase().contains(&lower)
                {
                    continue;
                }
                let mut row = step(a, s, task, false, reader)?;
                row["intent_id"] = a.data["node_id"].clone();
                row["worker"] = a.data["worker"].clone();
                hits.push(row);
                if hits.len()
                    >= p["limit"]
                        .as_u64()
                        .filter(|n| *n > 0)
                        .unwrap_or(100)
                        .min(1000) as usize
                {
                    break;
                }
            }
            Ok(json!({"query":q,"hits":hits}))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
fn compact_intent(n: &Record, s: &Snapshot) -> Value {
    let mut d = compact(n);
    let mut assets = ids(&n.data["payload"], "asset_ids");
    if assets.is_empty() {
        assets = s
            .records("exploration_anchors")
            .into_iter()
            .filter(|a| a.data["node_id"] == n.header.id)
            .filter_map(|a| a.data["asset_id"].as_str().map(str::to_owned))
            .collect();
    }
    if !assets.is_empty() {
        d["asset_ids"] = json!(assets);
    }
    for (field, rel, side, other) in [
        ("parents", "derived_from", "dst_id", "src_id"),
        ("yields", "yields", "src_id", "dst_id"),
    ] {
        let values = s
            .records("exploration_edges")
            .into_iter()
            .filter(|e| {
                e.data[side] == n.header.id
                    && (e.data["rel"] == rel || field == "parents" && e.data["rel"] == "spawns")
            })
            .map(|e| e.data[other].clone())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            d[field] = json!(values);
        }
    }
    if !n.data["blocked_reason"].is_null() && n.data["blocked_reason"] != "" {
        d["blocked_reason"] = n.data["blocked_reason"].clone();
    }
    d
}
fn overview(s: &Snapshot, task: &str, reader: &BlobReader, related: bool) -> Result<Value> {
    let t = record(s, "tasks", task)?;
    let exp = exploration_id(s, task)?;
    let nodes = ordered(
        s.records("exploration_nodes")
            .into_iter()
            .filter(|n| n.data["exploration_id"] == exp)
            .collect(),
        true,
    );
    let kind = |k: &str| {
        nodes
            .iter()
            .copied()
            .filter(|n| n.data["kind"] == k)
            .collect::<Vec<_>>()
    };
    let mut hot = kind("intent")
        .into_iter()
        .filter(|n| matches!(text(&n.data, "state"), "open" | "running"))
        .map(|n| n.header.id.clone())
        .collect::<BTreeSet<_>>();
    loop {
        let before = hot.len();
        for e in s.records("exploration_edges") {
            if e.data["exploration_id"] != exp {
                continue;
            }
            if hot.contains(text(&e.data, "dst_id"))
                && matches!(text(&e.data, "rel"), "spawns" | "derived_from")
            {
                hot.insert(text(&e.data, "src_id").into());
            }
            if hot.contains(text(&e.data, "src_id")) && e.data["rel"] == "yields" {
                hot.insert(text(&e.data, "dst_id").into());
            }
        }
        if hot.len() == before {
            break;
        }
    }
    let digests = kind("digest")
        .into_iter()
        .filter(|n| n.data["state"] == "active")
        .collect::<Vec<_>>();
    let covered = digests
        .iter()
        .flat_map(|n| {
            s.records("exploration_edges")
                .into_iter()
                .filter(|e| e.data["src_id"] == n.header.id && e.data["rel"] == "covers")
                .map(|e| text(&e.data, "dst_id").to_owned())
        })
        .collect::<BTreeSet<_>>();
    let hidden = |n: &&Record| covered.contains(&n.header.id) && !hot.contains(&n.header.id);
    let all_intents = kind("intent");
    let mut open = all_intents
        .iter()
        .copied()
        .filter(|n| n.data["state"] == "open")
        .collect::<Vec<_>>();
    let frontier = open.len();
    open.sort_by(|a, b| {
        b.data["priority"]
            .as_i64()
            .cmp(&a.data["priority"].as_i64())
            .then(a.header.commit_seq.cmp(&b.header.commit_seq))
    });
    let terminal = all_intents
        .iter()
        .copied()
        .filter(|n| matches!(text(&n.data, "state"), "done" | "blocked" | "exhausted"))
        .collect::<Vec<_>>();
    let facts = kind("fact");
    let findings = kind("finding");
    let from = |id: &str| {
        s.records("exploration_edges")
            .into_iter()
            .find(|e| e.data["dst_id"] == id && e.data["rel"] == "yields")
            .map(|e| e.data["src_id"].clone())
    };
    let recent = |n: &Record| {
        let mut d = compact(n);
        if let Some(i) = from(&n.header.id) {
            d["from_intent"] = i;
        }
        if !n.data["payload"]["confidence"].is_null() {
            d["confidence"] = n.data["payload"]["confidence"].clone();
        }
        d
    };
    let mut out = json!({"task":{"description":t.data["description"],"goal":t.data["goal"]},"goals":kind("goal").iter().take(100).map(|n|json!({"id":n.header.id,"state":n.data["state"],"text":n.data["payload"]["text"]})).collect::<Vec<_>>(),"hints":kind("hint").iter().take(50).map(|n|{let mut d=json!({"id":n.header.id,"state":n.data["state"],"text":n.data["payload"]["text"]});if setting_bool(s,"agent_traffic_binding",false)&&!n.data["payload"]["traffic_refs"].is_null(){d["traffic_refs"]=n.data["payload"]["traffic_refs"].clone();}d}).collect::<Vec<_>>(),"open_intents":open.iter().take(30).map(|n|compact_intent(n,s)).collect::<Vec<_>>(),"running_intents":all_intents.iter().filter(|n|n.data["state"]=="running").map(|n|compact_intent(n,s)).collect::<Vec<_>>(),"done_intents_total":terminal.len(),"frontier_open":frontier,"findings_total":findings.len().min(1000),"facts":facts.len().min(1000),"finding_list":findings.iter().take(10).map(|n|{let mut d=json!({"id":n.header.id,"summary":n.data["payload"]["summary"]});if let Some(i)=from(&n.header.id){d["from_intent"]=i;}d}).collect::<Vec<_>>(),"recent_facts":facts.iter().filter(|n|!hidden(n)).take(20).map(|n|recent(n)).collect::<Vec<_>>(),"recent_done_intents":terminal.iter().filter(|n|!hidden(n)).take(12).map(|n|compact_intent(n,s)).collect::<Vec<_>>(),"related_tasks":[]});
    if !digests.is_empty() {
        let mut sorted = digests;
        sorted.sort_by_key(|n| {
            std::cmp::Reverse(
                s.records("exploration_edges")
                    .into_iter()
                    .filter(|e| e.data["src_id"] == n.header.id && e.data["rel"] == "covers")
                    .filter_map(|e| s.get("exploration_nodes", text(&e.data, "dst_id")))
                    .map(|n| n.header.commit_seq.parse::<u64>().unwrap_or(0))
                    .max()
                    .unwrap_or(0),
            )
        });
        let cap = if related { 15 } else { 6 };
        out["cold_digests"]=Value::Array(sorted.iter().take(cap).map(|n|Ok(json!({"id":n.header.id,"body":plain(&n.data["payload"]["body"],reader)?,"member_count":s.records("exploration_edges").iter().filter(|e|e.data["src_id"]==n.header.id&&e.data["rel"]=="covers").count()}))).collect::<Result<_>>()?);
        if sorted.len() > cap {
            out["cold_digests_more"] = json!(
                sorted[cap..]
                    .iter()
                    .map(|n| n.header.id.clone())
                    .collect::<Vec<_>>()
            );
        }
    }
    let cov = asset_graph_ui::query(
        "ui.taskCoverage",
        &json!({"id":task}),
        s,
        &Context {
            actor: Actor {
                kind: "engine".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: new_id("epoch"),
            },
            task_id: Some(task.into()),
            intent_id: None,
            conversation_id: None,
        },
    )?;
    let (target, _) = asset_graph_ui::scoped_coverage(s, task)?;
    let hosts = context_hosts(s, task, &target);
    out["coverage"] = if t.data["coverage_enabled"] == false {
        json!({"host_count":hosts.len()})
    } else {
        json!({"denominator":cov["denominator"],"tested":cov["tested"],"pct":cov["pct"],"by_type":cov["by_type"],"host_count":hosts.len(),"note":"Asset coverage is a rough reference derived from current and direct source scope and fact anchors"})
    };
    if related {
        let sources = direct_sources(s, task)
            .into_iter()
            .filter(|id| id != task)
            .collect::<Vec<_>>();
        let per_source = (48000 / sources.len().max(1)).min(8000);
        let mut items = vec![];
        for source in sources {
            let mut budget = TextBudget {
                remaining: per_source,
                truncated: false,
            };
            let source_exp = exploration_id(s, &source)?;
            let source_rows = ordered(
                s.records("exploration_nodes")
                    .into_iter()
                    .filter(|n| n.data["exploration_id"] == source_exp)
                    .collect(),
                true,
            );
            let mut item = overview(s, &source, reader, false)?;
            item["source_task_id"] = json!(source);
            item["inherited"] = json!(true);
            item["task"]["status"] = record(s, "tasks", &source)?.data["status"].clone();
            for k in ["description", "goal"] {
                item["task"][k] = json!(budget.take(&item["task"][k], 800));
            }
            let mut stats = serde_json::Map::new();
            for n in &source_rows {
                let k = text(&n.data, "kind");
                let count = stats.get(k).and_then(Value::as_u64).unwrap_or(0);
                stats.insert(k.into(), json!(count + 1));
            }
            item["node_stats"] = json!(stats);
            item["facts"] = json!(
                source_rows
                    .iter()
                    .filter(|n| n.data["kind"] == "fact")
                    .count()
            );
            item["findings"] = json!(
                source_rows
                    .iter()
                    .filter(|n| n.data["kind"] == "finding")
                    .count()
            );
            for (key, kind, cap) in [("goals", "goal", 8), ("hints", "hint", 6)] {
                budget.truncated |= source_rows
                    .iter()
                    .filter(|n| n.data["kind"] == kind)
                    .count()
                    > cap;
                if let Some(a) = item[key].as_array_mut() {
                    a.truncate(cap);
                    for n in a {
                        n["text"] = json!(budget.take(&n["text"], 400));
                        n["inherited"] = json!(true);
                        n["source_task_id"] = json!(source);
                    }
                }
            }
            let fs = source_rows
                .iter()
                .filter(|n| n.data["kind"] == "finding")
                .collect::<Vec<_>>();
            budget.truncated |= fs.len() > 6;
            item["recent_findings"] = json!(
                fs.iter()
                    .take(6)
                    .map(|n| {
                        let mut f = mark(compact(n), s, task, n);
                        for k in ["vulnclass", "severity"] {
                            if !n.data["payload"][k].is_null() {
                                f[k] = n.data["payload"][k].clone();
                            }
                        }
                        f["summary"] = json!(budget.take(&f["summary"], 400));
                        f
                    })
                    .collect::<Vec<_>>()
            );
            budget.truncated |= item["facts"].as_u64().unwrap_or(0) > 12;
            if let Some(a) = item["recent_facts"].as_array_mut() {
                a.truncate(12);
                for n in a {
                    n["summary"] = json!(budget.take(&n["summary"], 400));
                    n["inherited"] = json!(true);
                    n["source_task_id"] = json!(source);
                    if n["from_intent"]
                        .as_str()
                        .and_then(|id| s.get("exploration_nodes", id))
                        .is_some_and(|i| {
                            !matches!(
                                text(&i.data, "state"),
                                "done" | "blocked" | "exhausted" | "stopped"
                            )
                        })
                    {
                        n.as_object_mut().unwrap().remove("from_intent");
                    }
                }
            }
            let covered = source_rows
                .iter()
                .filter(|n| n.data["kind"] == "digest" && n.data["state"] == "active")
                .flat_map(|n| {
                    s.records("exploration_edges")
                        .into_iter()
                        .filter(|e| e.data["src_id"] == n.header.id && e.data["rel"] == "covers")
                        .map(|e| text(&e.data, "dst_id").to_owned())
                })
                .collect::<BTreeSet<_>>();
            item["recent_intent_results"] = json!(
                source_rows
                    .iter()
                    .filter(|n| n.data["kind"] == "intent"
                        && matches!(
                            text(&n.data, "state"),
                            "done" | "blocked" | "exhausted" | "stopped"
                        )
                        && !covered.contains(&n.header.id))
                    .take(8)
                    .map(|n| {
                        let mut d = mark(compact_intent(n, s), s, task, n);
                        d["summary"] = json!(budget.take(&d["summary"], 400));
                        let acts = activities(s, &source, Some(&n.header.id));
                        let recent = acts.iter().rev().take(20).copied().collect::<Vec<_>>();
                        if let Some(a) = recent
                            .iter()
                            .find(|a| a.data["kind"] == "result")
                            .or_else(|| recent.iter().find(|a| a.data["kind"] == "text"))
                        {
                            d["result_summary"] = json!(budget.take(&a.data["summary"], 800));
                        }
                        d
                    })
                    .collect::<Vec<_>>()
            );
            if let Some(digests) = item["cold_digests"].as_array_mut() {
                for d in digests {
                    d["inherited"] = json!(true);
                    d["source_task_id"] = json!(source);
                }
            }
            let scopes = ordered(
                s.records("task_scope")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == source)
                    .collect(),
                false,
            );
            if !scopes.is_empty() {
                budget.truncated |= scopes.len() > 12;
                item["asset_scope_count"] = json!(scopes.len());
                item["asset_scope"]=json!(scopes.iter().take(12).map(|r|{let mut d=json!({"kind":r.data["kind"],"source":budget.take(&r.data["source"],300)});if let Some(k)=["domain","net","value"].into_iter().find(|k|!text(&r.data,k).is_empty()){d["value"]=json!(budget.take(&r.data[k],400));}else if !r.data["company_id"].is_null(){d["company_id"]=r.data["company_id"].clone();}d}).collect::<Vec<_>>());
            }
            item["asset_coverage"] = json!({"denominator":item["coverage"]["denominator"],"tested":item["coverage"]["tested"],"pct":item["coverage"]["pct"],"by_type":item["coverage"]["by_type"]});
            if record(s, "tasks", &source)?.data["coverage_enabled"] == false {
                let source_cov = asset_graph_ui::query(
                    "ui.taskCoverage",
                    &json!({"id":source}),
                    s,
                    &Context {
                        actor: Actor {
                            kind: "engine".into(),
                            agent_id: None,
                            run_id: None,
                            engine_epoch: new_id("epoch"),
                        },
                        task_id: Some(source.clone()),
                        intent_id: None,
                        conversation_id: None,
                    },
                )?;
                for k in ["denominator", "tested", "pct", "by_type"] {
                    item["asset_coverage"][k] = source_cov[k].clone();
                }
            }
            for key in [
                "open_intents",
                "running_intents",
                "related_tasks",
                "done_intents_total",
                "frontier_open",
                "finding_list",
                "recent_done_intents",
                "findings_total",
                "coverage",
            ] {
                item.as_object_mut().unwrap().remove(key);
            }
            if budget.truncated {
                item["summary_truncated"] = json!(true);
            }
            items.push(item);
        }
        out["related_tasks"] = json!(items);
    }

    Ok(out)
}

struct TextBudget {
    remaining: usize,
    truncated: bool,
}
impl TextBudget {
    fn take(&mut self, v: &Value, field_limit: usize) -> String {
        let text = v.as_str().unwrap_or("").trim();
        if text.is_empty() {
            return String::new();
        }
        let limit = self.remaining.min(field_limit);
        if limit == 0 {
            self.truncated = true;
            return String::new();
        }
        let runes = text.chars().collect::<Vec<_>>();
        let out = if runes.len() > limit {
            self.truncated = true;
            format!("{}…", runes[..limit - 1].iter().collect::<String>())
        } else {
            text.into()
        };
        self.remaining -= out.chars().count();
        out
    }
}

fn context_hosts(s: &Snapshot, task: &str, target: &BTreeSet<String>) -> BTreeSet<String> {
    let tasks = direct_sources(s, task);
    let exps = visible_explorations(s, task);
    let anchors = s
        .records("exploration_anchors")
        .into_iter()
        .filter(|a| {
            a.data["node_id"]
                .as_str()
                .and_then(|id| s.get("exploration_nodes", id))
                .is_some_and(|n| exps.contains(text(&n.data, "exploration_id")))
        })
        .map(|a| text(&a.data, "asset_id").to_owned())
        .collect::<BTreeSet<_>>();
    let linked = s
        .records("task_asset_links")
        .into_iter()
        .filter(|l| tasks.contains(text(&l.data, "task_id")))
        .map(|l| text(&l.data, "asset_id").to_owned())
        .collect::<BTreeSet<_>>();
    let mut hosts = BTreeSet::new();
    for a in s.records("assets") {
        if !target.contains(&a.header.id)
            && !anchors.contains(&a.header.id)
            && !linked.contains(&a.header.id)
            && !ids(&a.data, "task_ids").iter().any(|id| tasks.contains(id))
        {
            continue;
        }
        for k in ["domain", "ip"] {
            let h = text(&a.data, k).trim().to_lowercase();
            if !h.is_empty() {
                hosts.insert(h);
            }
        }
        if let Ok(u) = url::Url::parse(text(&a.data, "url")) {
            if let Some(h) = u.host_str() {
                hosts.insert(h.trim_matches(['[', ']']).to_lowercase());
            }
        }
    }
    hosts
}
