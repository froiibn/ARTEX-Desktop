use crate::*;
use std::collections::BTreeMap;
pub const QUERIES: &[&str] = &[
    "ui.findings",
    "ui.findingsPage",
    "ui.findingGroups",
    "ui.getFinding",
    "ui.findingTraffic",
    "ui.findingLineage",
    "ui.findingRetests",
    "ui.activeFindingRetests",
];
pub const COMMANDS: &[&str] = &[
    "ui.setFindingStatus",
    "ui.setFindingSeverity",
    "ui.updateFinding",
    "ui.deleteFinding",
    "ui.bindFindingTraffic",
    "ui.editFindingTraffic",
    "ui.removeFindingTraffic",
    "ui.orderFindingTraffic",
    "ui.startFindingRetest",
    "ui.deepenFinding",
];
pub fn finding(d: &Value, s: &Snapshot, detail: bool, context: Option<&str>) -> Value {
    let fid = text(d, "id");
    let bindings = s
        .records("finding_traffic_bindings")
        .iter()
        .filter(|r| r.data["finding_id"] == fid)
        .count();
    let mut out = json!({"id":fid,"finding_id":fid,"vulnclass":d["vulnclass"],"severity":d["severity"],"status":d["status"],"summary":d["summary"],"evidence":d["evidence"],"ts":d["created_at"],"traffic_count":bindings,"evidence_version":d["evidence_version"],"report_evidence_version":d["report_evidence_version"],"report_stale":d["report"]!=artex_core::body("")&&d["report_evidence_version"]!=d["evidence_version"]});
    for key in ["name", "task_id", "param_id"] {
        if !d[key].is_null() && d[key] != "" {
            out[key] = d[key].clone();
        }
    }
    if detail && d["report"] != artex_core::body("") {
        out["report"] = d["report"].clone();
    }
    if let Some(t) = d["task_id"].as_str().and_then(|id| s.get("tasks", id)) {
        out["task_description"] = t.data["description"].clone();
        if context.is_some_and(|id| id != t.header.id) {
            out["inherited"] = json!(true);
            out["source_task_id"] = json!(t.header.id);
        }
    }
    let assets = ids(d, "asset_ids")
        .iter()
        .filter_map(|id| s.get("assets", id))
        .map(exploration_ui::asset_ref)
        .collect::<Vec<_>>();
    if !assets.is_empty() {
        out["assets"] = json!(assets);
    }
    out
}
pub fn binding(d: &Value, s: &Snapshot) -> Result<Value> {
    let snapshot = record(s, "traffic_evidence_snapshots", required(d, "snapshot_id")?)?;
    let mut snap = snapshot.data.clone();
    for k in ["unreferenced_at", "created_at"] {
        snap.as_object_mut().unwrap().remove(k);
    }
    let mut out = d.clone();
    out["snapshot"] = snap;
    Ok(out)
}
pub fn traffic(fid: &str, s: &Snapshot) -> Result<Value> {
    let f = record(s, "findings", fid)?;
    let mut rows = s
        .records("finding_traffic_bindings")
        .into_iter()
        .filter(|r| r.data["finding_id"] == fid)
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| {
        (
            r.data["position"].as_i64().unwrap_or(0),
            r.header.id.clone(),
        )
    });
    Ok(
        json!({"finding_id":fid,"version":f.data["evidence_version"],"report_version":f.data["report_evidence_version"],"bindings":rows.iter().map(|r|binding(&r.data,s)).collect::<Result<Vec<_>>>()?}),
    )
}
fn scoped(s: &Snapshot, p: &Value, c: &Context, write: bool) -> Result<Context> {
    let f = record(s, "findings", required(p, "id")?)?;
    let mut context = c.clone();
    let id = p["contextTask"]
        .as_str()
        .or(p["contextTaskId"].as_str())
        .or(c.task_id.as_deref());
    if let Some(task) = id {
        if if write {
            f.data["task_id"] != task
        } else {
            !direct_sources(s, task).contains(text(&f.data, "task_id"))
        } {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Inherited finding is read only or outside direct scope",
            ));
        }
        context.task_id = Some(task.into());
    }
    Ok(context)
}
pub fn severity(s: &str) -> u8 {
    match s {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}
pub fn filtered<'a>(s: &'a Snapshot, q: &Value) -> Result<Vec<&'a Record>> {
    let query = text(q, "query").trim().to_lowercase();
    let asset_scope = text(q, "assetScope");
    let scope_ids = if asset_scope.is_empty() || asset_scope == "__none__" {
        BTreeSet::new()
    } else {
        asset_graph_ui::scope_asset_ids(s, asset_scope)
    };
    let mut rows = vec![];
    for f in s.records("findings") {
        let d = &f.data;
        if ["severity", "status", "vulnclass"]
            .iter()
            .any(|key| !matches!(text(q, key), "" | "all") && d[*key] != q[*key])
        {
            continue;
        }
        let task = text(q, "task");
        if task == "__unassigned__" {
            if d["task_id"]
                .as_str()
                .and_then(|id| s.get("tasks", id))
                .is_some()
            {
                continue;
            }
        } else if !matches!(task, "" | "all") && d["task_id"] != task {
            continue;
        }
        if !query.is_empty()
            && !["name", "vulnclass", "summary", "evidence", "report"]
                .iter()
                .any(|k| {
                    d[*k]
                        .as_str()
                        .or(d[*k]["text"].as_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                })
        {
            continue;
        }
        if !asset_scope.is_empty() {
            let aids = ids(d, "asset_ids");
            if asset_scope == "__none__" {
                if aids.iter().any(|id| s.get("assets", id).is_some()) {
                    continue;
                }
            } else {
                if !aids.iter().any(|id| scope_ids.contains(id)) {
                    continue;
                }
            }
        }
        rows.push(f);
    }
    rows.sort_by(|a, b| {
        let weight = if q["sort"] == "severity" {
            severity(text(&b.data, "severity")).cmp(&severity(text(&a.data, "severity")))
        } else {
            std::cmp::Ordering::Equal
        };
        weight
            .then_with(|| b.header.created_at.cmp(&a.header.created_at))
            .then_with(|| b.header.id.cmp(&a.header.id))
    });
    Ok(rows)
}
fn retest(d: &Value) -> Value {
    let mut d = d.clone();
    d.as_object_mut().unwrap().remove("snapshot");
    d
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m{
"ui.getFinding"=>{let context=scoped(s,p,c,false)?;let f=record(s,"findings",required(p,"id")?)?;Ok(finding(&f.data,s,true,context.task_id.as_deref()))},
"ui.findingTraffic"=>{scoped(s,p,c,false)?;traffic(required(p,"id")?,s)},
"ui.findings"=>{let task=text(p,"task");let allowed=if !task.is_empty()&&task!="all"{Some(direct_sources(s,task))}else{None};let mut rows=s.records("findings").into_iter().filter(|r|allowed.as_ref().is_none_or(|set|set.contains(text(&r.data,"task_id")))).collect::<Vec<_>>();rows.sort_by(|a,b|b.header.created_at.cmp(&a.header.created_at));rows.truncate(500);Ok(json!(rows.iter().map(|r|finding(&r.data,s,false,if task.is_empty(){None}else{Some(task)})).collect::<Vec<_>>()))},
"ui.findingsPage"=>{let q=&p["q"];let rows=filtered(s,q)?;let total=rows.len();let page=q["page"].as_u64().unwrap_or(1).max(1);let size=q["pageSize"].as_u64().unwrap_or(20).clamp(1,100);Ok(json!({"items":rows.into_iter().skip((page-1).saturating_mul(size)as usize).take(size as usize).map(|r|finding(&r.data,s,false,None)).collect::<Vec<_>>(),"total":total,"page":page,"page_size":size}))},
"ui.findingGroups"=>{let q=&p["q"];let rows=filtered(s,q)?;let finding_total=rows.len();let mut groups=BTreeMap::<Option<String>,Vec<&Record>>::new();for r in rows{let owner=r.data["task_id"].as_str().filter(|id|s.get("tasks",id).is_some()).map(str::to_owned);groups.entry(owner).or_default().push(r);}let mut rows=groups.into_iter().map(|(id,findings)|{let t=id.as_deref().and_then(|id|s.get("tasks",id));let mut out=json!({"task_id":id,"task_name":t.map(|r|text(&r.data,"name")).unwrap_or(""),"task_description":t.map(|r|text(&r.data,"description")).unwrap_or(""),"task_status":t.map(|r|text(&r.data,"status")).unwrap_or(""),"count":findings.len(),"last_found_at":findings.iter().map(|r|r.header.created_at.clone()).max().unwrap()});for severity in ["critical","high","medium","low"]{out[severity]=json!(findings.iter().filter(|f|f.data["severity"]==severity).count());}out}).collect::<Vec<_>>();rows.sort_by(|a,b|{let rank=|v:&Value|["critical","high","medium","low"].iter().position(|k|v[*k].as_u64().unwrap_or(0)>0).unwrap_or(4);(if q["sort"]=="severity"{rank(a).cmp(&rank(b))}else{std::cmp::Ordering::Equal}).then_with(||text(b,"last_found_at").cmp(text(a,"last_found_at"))).then_with(||text(b,"task_id").cmp(text(a,"task_id")))});let total=rows.len();let page=q["page"].as_u64().unwrap_or(1).max(1);let size=q["pageSize"].as_u64().unwrap_or(10).clamp(1,100);Ok(json!({"items":rows.into_iter().skip((page-1).saturating_mul(size)as usize).take(size as usize).collect::<Vec<_>>(),"total":total,"finding_total":finding_total,"page":page,"page_size":size}))},
"ui.findingLineage"=>{let f=record(s,"findings",required(p,"id")?)?;let Some(node)=f.data["node_id"].as_str().and_then(|id|s.get("exploration_nodes",id))else{return Ok(json!({"nodes":[],"edges":[]}));};let mut visited=BTreeSet::from([node.header.id.clone()]);loop{let mut added=false;for e in s.records("exploration_edges"){if e.data["exploration_id"]==node.data["exploration_id"]&&visited.contains(text(&e.data,"dst_id")){added|=visited.insert(text(&e.data,"src_id").into());}}if !added{break;}}let owner=text(&f.data,"task_id");Ok(json!({"nodes":visited.iter().filter_map(|id|s.get("exploration_nodes",id)).map(|r|exploration_ui::node(r,s,owner)).collect::<Vec<_>>(),"edges":s.records("exploration_edges").iter().filter(|r|visited.contains(text(&r.data,"src_id"))&&visited.contains(text(&r.data,"dst_id"))).map(|r|exploration_ui::edge(r)).collect::<Vec<_>>()}))},
"ui.findingRetests"=>{let id=required(p,"id")?;record(s,"findings",id)?;let mut rows=s.records("finding_retests").into_iter().filter(|r|r.data["finding_id"]==id).collect::<Vec<_>>();rows.sort_by(|a,b|b.header.created_at.cmp(&a.header.created_at));Ok(json!(rows.iter().map(|r|retest(&r.data)).collect::<Vec<_>>()))},
"ui.activeFindingRetests"=>Ok(json!(s.records("finding_retests").iter().filter(|r|matches!(text(&r.data,"status"),"pending"|"running")).map(|r|json!({"id":r.header.id,"finding_id":r.data["finding_id"],"conversation_id":r.data["conversation_id"],"status":r.data["status"]})).collect::<Vec<_>>())),
_=>Err(Error::new("METHOD_NOT_FOUND",m))}
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let fid = required(p, "id")?;
    let context = scoped(s, p, c, m != "ui.startFindingRetest")?;
    let f = record(s, "findings", fid)?;
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.setFindingStatus" | "ui.setFindingSeverity" | "ui.updateFinding" => {
            let patch = match m {
                "ui.setFindingStatus" => json!({"status":p["status"]}),
                "ui.setFindingSeverity" => json!({"severity":p["severity"]}),
                _ => p["fields"].clone(),
            };
            if patch.as_object().is_none_or(|o| o.is_empty()) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Finding patch cannot be empty",
                ));
            }
            let next = findings::plan(
                "finding.update",
                &json!({"id":fid,"patch":patch}),
                s,
                &context,
            )?;
            task_control_ui::absorb(&mut b, next);
            let state = projected(s, &b.batch)?;
            finding(&record(&state, "findings", fid)?.data, &state, true, None)
        }
        "ui.deleteFinding" => {
            let next = findings::plan("finding.delete", &json!({"id":fid}), s, &context)?;
            task_control_ui::absorb(&mut b, next);
            json!({"deleted":true,"id":fid})
        }
        "ui.bindFindingTraffic" => {
            let next = findings::plan(
                "bind_finding_traffic",
                &json!({"finding_id":fid,"traffic_refs":p["traffic_refs"]}),
                s,
                &context,
            )?;
            task_control_ui::absorb(&mut b, next);
            traffic(fid, &projected(s, &b.batch)?)?
        }
        "ui.editFindingTraffic" | "ui.removeFindingTraffic" | "ui.orderFindingTraffic" => {
            let version = required(p, "version")?;
            if f.data["evidence_version"] != version {
                return Err(Error::new(
                    "EVIDENCE_CONFLICT",
                    "Evidence changed; refresh before editing",
                ));
            }
            let mut fd = f.data.clone();
            let next = version
                .parse::<u64>()
                .ok()
                .and_then(|v| v.checked_add(1))
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Evidence version exhausted"))?;
            fd["evidence_version"] = json!(next.to_string());
            if m == "ui.orderFindingTraffic" {
                let supplied = ids(p, "binding_ids");
                let actual = s
                    .records("finding_traffic_bindings")
                    .iter()
                    .filter(|r| r.data["finding_id"] == fid)
                    .map(|r| r.header.id.clone())
                    .collect::<BTreeSet<_>>();
                let supplied_set = supplied.iter().cloned().collect::<BTreeSet<_>>();
                if supplied.len() != actual.len() || supplied_set != actual {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Order must contain every binding exactly once",
                    ));
                }
                for (i, id) in supplied.iter().enumerate() {
                    let mut d = record(s, "finding_traffic_bindings", id)?.data.clone();
                    d["position"] = json!(i);
                    b.put("finding_traffic_bindings", id, d);
                }
            } else {
                let id = required(p, "bindingId")?;
                let row = record(s, "finding_traffic_bindings", id)?;
                if row.data["finding_id"] != fid {
                    return Err(Error::new("NOT_FOUND", "Evidence binding not found"));
                }
                if m == "ui.removeFindingTraffic" {
                    b.delete("finding_traffic_bindings", id);
                    let snapshot_id = required(&row.data, "snapshot_id")?;
                    if !s
                        .records("finding_traffic_bindings")
                        .iter()
                        .any(|r| r.header.id != id && r.data["snapshot_id"] == snapshot_id)
                    {
                        let mut snap = record(s, "traffic_evidence_snapshots", snapshot_id)?
                            .data
                            .clone();
                        snap["unreferenced_at"] = json!(artex_core::now());
                        b.put("traffic_evidence_snapshots", snapshot_id, snap);
                    }
                } else {
                    let fields = &p["fields"];
                    let role = required(fields, "role")?;
                    if !["baseline", "proof", "verification", "supporting"].contains(&role) {
                        return Err(Error::new("INVALID_PARAMS", "Invalid evidence role"));
                    }
                    let mut d = row.data.clone();
                    d["role"] = json!(role);
                    d["note"] = json!(text(fields, "note"));
                    b.put("finding_traffic_bindings", id, d);
                }
            }
            b.put("findings", fid, fd);
            traffic(fid, &projected(s, &b.batch)?)?
        }
        "ui.startFindingRetest" => {
            let notes = text(p, "notes").trim();
            if notes.chars().count() > 4000 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Retest notes exceed 4000 characters",
                ));
            }
            let agent = conversation::agent_by_key(s, "retester")?;
            if agent.data["enabled"] != true {
                return Err(Error::new("FEATURE_DISABLED", "Retest agent is disabled"));
            }
            for key in ["get_finding_retest_context", "record_finding_retest_result"] {
                if !s.records("tools").iter().any(|r| {
                    r.data["key"] == key
                        && r.data["enabled"] == true
                        && ids(&r.data, "agents").iter().any(|a| a == "retester")
                }) {
                    return Err(Error::new(
                        "FEATURE_DISABLED",
                        "Retest tools must be enabled and bound",
                    ));
                }
            }
            conversation::effective_profile(s, agent, None, None)?;
            if let Some(existing) = s.records("finding_retests").into_iter().find(|r| {
                r.data["finding_id"] == fid
                    && matches!(text(&r.data, "status"), "pending" | "running")
            }) {
                json!({"retest":retest(&existing.data),"created":false})
            } else {
                let next = findings::plan(
                    "finding.retest.create",
                    &json!({"finding_id":fid,"notes":notes}),
                    s,
                    &context,
                )?;
                let rid = next.result["retest_id"].as_str().unwrap().to_owned();
                let conv = next.result["conversation_id"].as_str().unwrap().to_owned();
                task_control_ui::absorb(&mut b, next);
                let mut rd = b.data("finding_retests", &rid)?;
                rd["snapshot"]["assets"] = json!(
                    ids(&f.data, "asset_ids")
                        .iter()
                        .filter_map(|id| s.get("assets", id))
                        .map(|r| r.data.clone())
                        .collect::<Vec<_>>()
                );
                rd["snapshot"]["constraints"] = json!(
                    f.data["task_id"]
                        .as_str()
                        .and_then(|id| s.get("tasks", id))
                        .map(|task| s
                            .records("task_constraints")
                            .iter()
                            .filter(|r| r.data["exploration_id"] == task.data["exploration_id"])
                            .map(|r| r.data.clone())
                            .collect::<Vec<_>>())
                        .unwrap_or_default()
                );
                b.put("finding_retests", &rid, rd);
                let state = projected(s, &b.batch)?;
                let message = format!(
                    "Retest finding {fid}. First call get_finding_retest_context to read the original evidence and constraints for this session, perform targeted verification, then call record_finding_retest_result to save your conclusion.\n\nAdditional instructions:\n{notes}"
                );
                let send = conversation::plan(
                    "conversation.send",
                    &json!({"id":conv,"message":message}),
                    &state,
                    c,
                )?;
                task_control_ui::absorb(&mut b, send);
                let state = projected(s, &b.batch)?;
                json!({"retest":retest(&record(&state,"finding_retests",&rid)?.data),"created":true})
            }
        }
        "ui.deepenFinding" => {
            let task = required(&f.data, "task_id")?;
            let node = required(&f.data, "node_id")?;
            own_node(s, task, node)?;
            let description = required(p, "description")?.trim();
            if description.chars().count() > 4000 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Description exceeds 4000 characters",
                ));
            }
            let exp = exploration_id(s, task)?;
            let id=b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"intent","state":"open","origin":"human","priority":10,"payload":{"summary":description}}))?;
            b.insert(
                "exploration_edges",
                "edge",
                json!({"exploration_id":exp,"src_id":node,"dst_id":id,"rel":"derived_from"}),
            )?;
            for a in s.records("exploration_anchors") {
                if a.data["node_id"] == node {
                    b.insert(
                        "exploration_anchors",
                        "anchor",
                        json!({"node_id":id,"asset_id":a.data["asset_id"]}),
                    )?;
                }
            }
            b.insert("activity","activity",json!({"exploration_id":exp,"node_id":id,"worker":"system","kind":"text","summary":"User submitted a finding follow-up intent","detail":artex_core::body(description)}))?;
            task_control_ui::revive(&mut b, task, c)?;
            json!({"task_id":task,"intent_id":id,"state":"open","queued":b.data("tasks",task)?["queued"]})
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    conversation::push_event(&mut b.batch, s, c, "FindingChanged", "findings", fid)?;
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}
