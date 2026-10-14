use crate::*;

fn authorize_finding(s: &Snapshot, c: &Context, f: &Value, write: bool) -> Result<()> {
    if let Some(task) = c.task_id.as_deref() {
        let owner = text(f, "task_id");
        if if write {
            owner != task
        } else {
            !direct_sources(s, task).contains(owner)
        } {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "finding is outside permitted task scope",
            ));
        }
    }
    Ok(())
}
fn bind(b: &mut Planner, fid: &str, refs: &[Value]) -> Result<usize> {
    let mut position = b
        .state
        .records("finding_traffic_bindings")
        .iter()
        .filter(|r| text(&r.data, "finding_id") == fid)
        .filter_map(|r| r.data["position"].as_i64())
        .max()
        .unwrap_or(-1)
        + 1;
    let mut count = 0;
    for tr in refs {
        let traffic = required(tr, "traffic_id")?;
        let ex = record(b.state, "exchanges", traffic)?;
        let bodies = b
            .state
            .records("exchange_bodies")
            .into_iter()
            .find(|r| r.header.id == traffic || text(&r.data, "id") == text(&ex.data, "id"))
            .ok_or_else(|| Error::new("NOT_FOUND", "traffic body references not found"))?;
        let role = if text(tr, "role").is_empty() {
            "supporting"
        } else {
            text(tr, "role")
        };
        if !["baseline", "proof", "verification", "supporting"].contains(&role) {
            return Err(Error::new("INVALID_PARAMS", "invalid evidence role"));
        }
        // Snapshot is immutable, copied from the actual capture rather than guessed traffic IDs.
        for (side, length) in [("req_body", "req_len"), ("resp_body", "resp_len")] {
            let n = text(&ex.data, length)
                .parse::<u64>()
                .map_err(|_| Error::new("INVALID_PARAMS", "capture body length is invalid"))?;
            if n > 0
                && (bodies.data[side]["length"].as_str() != Some(text(&ex.data, length))
                    || bodies.data[side]["sha256"].as_str().is_none())
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "capture body reference and length disagree",
                ));
            }
        }
        let patch = json!({"source_traffic_id":traffic,"captured_at":ex.data["ts"],"url":ex.data["url"],"method":ex.data["method"],"status":ex.data["status"],"content_type":text(&ex.data,"content_type"),"req_head":bodies.data["req_head"],"resp_head":bodies.data["resp_head"],"req_hash":bodies.data["req_body"]["sha256"].as_str().unwrap_or("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),"resp_hash":bodies.data["resp_body"]["sha256"].as_str().unwrap_or("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),"req_len":ex.data["req_len"],"resp_len":ex.data["resp_len"]});
        let same = |d: &Value| patch.as_object().unwrap().iter().all(|(k, v)| d[k] == *v);
        let prior = b
            .state
            .records("traffic_evidence_snapshots")
            .into_iter()
            .find(|r| same(&r.data))
            .map(|r| r.header.id.clone())
            .or_else(|| {
                b.batch.changes.iter().find_map(|ch| match ch {
                    Change::Put {
                        collection,
                        id,
                        data,
                    } if collection == "traffic_evidence_snapshots" && same(data) => {
                        Some(id.clone())
                    }
                    _ => None,
                })
            });
        let snap = if let Some(id) = prior {
            id
        } else {
            b.insert("traffic_evidence_snapshots", "evidence", patch)?
        };
        let already=b.state.records("finding_traffic_bindings").iter().any(|r|text(&r.data,"finding_id")==fid&&text(&r.data,"snapshot_id")==snap)||b.batch.changes.iter().any(|ch|matches!(ch,Change::Put{collection,data,..}if collection=="finding_traffic_bindings"&&text(data,"finding_id")==fid&&text(data,"snapshot_id")==snap));
        if already {
            continue;
        }
        let mut immutable = b.data("traffic_evidence_snapshots", &snap)?;
        if !immutable["unreferenced_at"].is_null() {
            immutable["unreferenced_at"] = Value::Null;
            b.put("traffic_evidence_snapshots", &snap, immutable);
        }
        b.insert("finding_traffic_bindings","binding",json!({"finding_id":fid,"snapshot_id":snap,"role":role,"note":text(tr,"note"),"position":position}))?;
        position += 1;
        count += 1;
    }
    Ok(count)
}
fn retest<'a>(s: &'a Snapshot, c: &Context) -> Result<&'a Record> {
    let conversation = c
        .conversation_id
        .as_deref()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "retest conversation context required"))?;
    s.records("finding_retests")
        .into_iter()
        .find(|r| text(&r.data, "conversation_id") == conversation)
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "conversation is not bound to a retest"))
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    match m {
        "report_finding" => {
            let task = task_id(p, c)?;
            let exp = exploration_id(s, task)?;
            let severity = required(p, "severity")?;
            if !["critical", "high", "medium", "low"].contains(&severity) {
                return Err(Error::new("INVALID_PARAMS", "invalid severity"));
            }
            let summary = required(p, "summary")?;
            let vuln = required(p, "vulnclass")?;
            let intent = p["intent_id"].as_str().or(c.intent_id.as_deref());
            if let Some(i) = intent {
                if text(&own_node(s, task, i)?.data, "kind") != "intent" {
                    return Err(Error::new("INVALID_PARAMS", "intent node required"));
                }
            }
            for a in ids(p, "asset_ids") {
                record(s, "assets", &a)?;
            }
            let node=b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"finding","state":"confirmed","priority":5,"payload":{"vulnclass":vuln,"name":text(p,"name"),"severity":severity,"summary":summary,"evidence":text(p,"evidence")},"owner":c.actor.agent_id}))?;
            if let Some(intent) = intent {
                b.insert(
                    "exploration_edges",
                    "edge",
                    json!({"exploration_id":exp,"src_id":intent,"dst_id":node,"rel":"yields"}),
                )?;
            }
            let fid=b.insert("findings","finding",json!({"task_id":task,"node_id":node,"vulnclass":vuln,"name":text(p,"name"),"severity":severity,"summary":summary,"evidence":artex_core::body(text(p,"evidence")),"worker":c.actor.agent_id.as_deref().unwrap_or(""),"asset_ids":ids(p,"asset_ids"),"status":"pending","report":artex_core::body(""),"evidence_version":"0","report_evidence_version":"0"}))?;
            let mut refs = p["traffic_refs"].as_array().cloned().unwrap_or_default();
            let binding_enabled =
                crate::setting_bool(s, "agent_traffic_binding", false) || c.actor.kind == "user";
            if !binding_enabled {
                refs.clear();
            }
            if let Some(h) = p["evidence_hint_id"].as_str().filter(|_| binding_enabled) {
                let hint = own_node(s, task, h)?;
                if text(&hint.data, "kind") != "hint" {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "evidence_hint_id must be own task hint",
                    ));
                }
                refs.extend(
                    hint.data["payload"]["traffic_refs"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            let count = bind(&mut b, &fid, &refs)?;
            if count > 0 {
                let mut d = b.data("findings", &fid)?;
                d["evidence_version"] = json!("1");
                b.put("findings", &fid, d);
            }
            let mut result = json!({"node_id":node,"finding_node_id":node,"finding_id":fid,"evidence_status":if count>0{"bound"}else{"not_bound"},"evidence_version":if count>0{"1"}else{"0"}});
            let projected = projected(s, &b.batch)?;
            let mut traffic = findings_ui::traffic(&fid, &projected)?;
            for binding in traffic["bindings"].as_array_mut().into_iter().flatten() {
                binding["snapshot"]["req_head"] = artex_core::body("");
                binding["snapshot"]["resp_head"] = artex_core::body("");
            }
            result["traffic"] = traffic;
            if count == 0 {
                result["evidence_note"] = json!(if binding_enabled {
                    "Finding saved without traffic binding; verify captured traffic before binding it, or proceed with recorded command evidence"
                } else {
                    "Finding saved; automatic traffic binding is disabled. Traffic can be associated manually in the findings UI"
                });
            }
            Ok(b.done(result))
        }
        "update_finding_report" => {
            let node = required(p, "finding_node_id")?;
            if !node.starts_with("node_") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "finding_node_id must be NodeId, not FindingId",
                ));
            }
            let f = s
                .records("findings")
                .into_iter()
                .find(|r| text(&r.data, "node_id") == node)
                .ok_or_else(|| Error::new("NOT_FOUND", "finding node has no finding record"))?;
            authorize_finding(s, c, &f.data, true)?;
            let expected = required(p, "evidence_version")?;
            artex_core::parse_u64(expected)?;
            if text(&f.data, "evidence_version") != expected {
                return Err(Error::new(
                    "EVIDENCE_CONFLICT",
                    "evidence changed; reload before updating report",
                ));
            }
            let mut d = f.data.clone();
            d["report"] = artex_core::body(
                p["report"]
                    .as_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "report must be text"))?,
            );
            d["report_evidence_version"] = json!(expected);
            b.put("findings", &f.header.id, d);
            Ok(b.done(json!({"finding_id":f.header.id,"finding_node_id":node,"report_evidence_version":expected})))
        }
        "bind_finding_traffic" => {
            let fid = required(p, "finding_id")?;
            let f = record(s, "findings", fid)?;
            authorize_finding(s, c, &f.data, true)?;
            let refs = p["traffic_refs"]
                .as_array()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "traffic_refs required"))?;
            let count = bind(&mut b, fid, refs)?;
            let mut d = f.data.clone();
            if count > 0 {
                let n = artex_core::parse_u64(text(&d, "evidence_version"))?
                    .checked_add(1)
                    .filter(|x| *x <= i64::MAX as u64)
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "evidence version exhausted"))?;
                d["evidence_version"] = json!(n.to_string());
                b.put("findings", fid, d.clone());
            }
            Ok(b.done(
                json!({"finding_id":fid,"added":count,"evidence_version":d["evidence_version"]}),
            ))
        }
        "finding.update" => {
            let fid = required(p, "id")?;
            let f = record(s, "findings", fid)?;
            authorize_finding(s, c, &f.data, true)?;
            let patch = p["patch"]
                .as_object()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "patch required"))?;
            let mut d = f.data.clone();
            for (k, v) in patch {
                match k.as_str() {
                    "status" => {
                        if ![
                            "pending",
                            "in_progress",
                            "confirmed",
                            "resolved",
                            "fixed",
                            "false_positive",
                            "ignored",
                            "duplicate",
                            "risk_accepted",
                        ]
                        .contains(&v.as_str().unwrap_or(""))
                        {
                            return Err(Error::new("INVALID_PARAMS", "invalid finding status"));
                        }
                        d[k] = v.clone();
                    }
                    "severity" => {
                        if !["critical", "high", "medium", "low"]
                            .contains(&v.as_str().unwrap_or(""))
                        {
                            return Err(Error::new("INVALID_PARAMS", "Invalid severity"));
                        }
                        d[k] = v.clone();
                    }
                    "name" | "vulnclass" | "summary" => {
                        d[k] = json!(
                            v.as_str()
                                .ok_or_else(|| Error::new(
                                    "INVALID_PARAMS",
                                    "Finding text field must be a string"
                                ))?
                                .trim()
                        )
                    }
                    "evidence" => {
                        d[k] = artex_core::body(v.as_str().ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "evidence text required")
                        })?);
                        d["evidence_version"] = json!(
                            (artex_core::parse_u64(text(&d, "evidence_version"))? + 1).to_string()
                        );
                    }
                    _ => return Err(Error::new("INVALID_PARAMS", "field cannot be patched")),
                }
            }
            b.put("findings", fid, d);
            if patch.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "name" | "vulnclass" | "summary" | "severity" | "evidence"
                )
            }) {
                if let Some(node) = f.data["node_id"]
                    .as_str()
                    .and_then(|id| s.get("exploration_nodes", id))
                {
                    let mut node_data = node.data.clone();
                    for field in ["name", "vulnclass", "summary", "severity", "evidence"] {
                        if let Some(value) = patch.get(field) {
                            node_data["payload"][field] = value.clone();
                        }
                    }
                    b.put("exploration_nodes", &node.header.id, node_data);
                }
            }
            Ok(b.done(json!({"id":fid})))
        }
        "finding.delete" => {
            let fid = required(p, "id")?;
            let finding = record(s, "findings", fid)?;
            authorize_finding(s, c, &finding.data, true)?;
            b.delete("findings", fid);
            if let Some(node) = finding.data["node_id"]
                .as_str()
                .and_then(|id| s.get("exploration_nodes", id))
                .filter(|n| text(&n.data, "kind") == "finding")
            {
                // The canonical node owns graph edges and anchors. Its deletion
                // also applies the source schema's activity SET NULL behavior.
                b.delete("exploration_nodes", &node.header.id);
            }
            Ok(b.done(json!({"id":fid})))
        }
        "finding.retest.create" => {
            let fid = required(p, "finding_id")?;
            let f = record(s, "findings", fid)?;
            authorize_finding(s, c, &f.data, false)?;
            if s.records("finding_retests").iter().any(|r| {
                text(&r.data, "finding_id") == fid
                    && matches!(text(&r.data, "status"), "pending" | "running")
            }) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "finding already has an active retest",
                ));
            }
            let conv = b.insert(
                "conversations",
                "conversation",
                json!({"agent_key":"retester","title":format!("Retest: {}",text(&f.data,"name"))}),
            )?;
            let snapshot = json!({"finding":f.data,"evidence":s.records("finding_traffic_bindings").iter().filter(|r|text(&r.data,"finding_id")==fid).map(|r|r.data.clone()).collect::<Vec<_>>()});
            let id=b.insert("finding_retests","retest",json!({"finding_id":fid,"conversation_id":conv,"status":"pending","verdict":"","notes":text(p,"notes"),"snapshot":snapshot,"evidence":artex_core::body("")}))?;
            Ok(b.done(json!({"retest_id":id,"conversation_id":conv})))
        }
        "record_finding_retest_result" => {
            let rt = retest(s, c)?;
            if !matches!(text(&rt.data, "status"), "pending" | "running") {
                return Err(Error::new("INVALID_PARAMS", "retest is terminal"));
            }
            let verdict = required(p, "verdict")?;
            if !["reproduced", "fixed", "inconclusive"].contains(&verdict) {
                return Err(Error::new("INVALID_PARAMS", "invalid verdict"));
            }
            let summary = required(p, "summary")?;
            let evidence = required(p, "evidence")?;
            let mut d = rt.data.clone();
            d["verdict"] = json!(verdict);
            d["summary"] = json!(summary);
            d["evidence"] = artex_core::body(evidence);
            d["status"] = json!("completed");
            d["finished_at"] = json!(artex_core::now());
            b.put("finding_retests", &rt.header.id, d.clone());
            if verdict == "fixed" {
                let fid = text(&d, "finding_id");
                let mut finding = record(s, "findings", fid)?.data.clone();
                finding["status"] = json!("fixed");
                b.put("findings", fid, finding);
            }
            Ok(b.done(json!({"saved":true,"retest_id":rt.header.id,"verdict":verdict})))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m {
        "finding.get" => {
            let f = record(s, "findings", required(p, "id")?)?;
            authorize_finding(s, c, &f.data, false)?;
            Ok(f.data.clone())
        }
        "finding.list" | "list_findings" | "list_task_findings" => {
            let task = p["task_id"].as_str().or(c.task_id.as_deref());
            let allowed = task.map(|t| direct_sources(s, t));
            Ok(json!(
                s.records("findings")
                    .iter()
                    .filter(|r| allowed
                        .as_ref()
                        .is_none_or(|set| set.contains(text(&r.data, "task_id"))))
                    .map(|r| {
                        let mut d = r.data.clone();
                        if let Some(o) = d.as_object_mut() {
                            o.remove("report");
                        }
                        d
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "get_finding_traffic" => {
            let fid = required(p, "finding_id")?;
            let f = record(s, "findings", fid)?;
            authorize_finding(s, c, &f.data, false)?;
            if !text(p, "binding_id").is_empty() {
                return Err(Error::new(
                    "FEATURE_DISABLED",
                    "evidence body byte ranges require blob service",
                ));
            }
            let rows=s.records("finding_traffic_bindings").iter().filter(|r|text(&r.data,"finding_id")==fid).map(|r|json!({"binding":r.data,"snapshot":s.get("traffic_evidence_snapshots",text(&r.data,"snapshot_id")).map(|x|x.data.clone())})).collect::<Vec<_>>();
            Ok(
                json!({"finding_id":fid,"evidence_version":f.data["evidence_version"],"report_evidence_version":f.data["report_evidence_version"],"report_stale":f.data["report_evidence_version"]!=f.data["evidence_version"],"bindings":rows}),
            )
        }
        "get_finding_retest_context" => {
            let rt = retest(s, c)?;
            Ok(
                json!({"retest":rt.data,"finding":record(s,"findings",text(&rt.data,"finding_id"))?.data}),
            )
        }
        "traffic_get" => {
            let id = required(p, "id")?;
            let ex = record(s, "exchanges", id)?;
            let body = s
                .records("exchange_bodies")
                .into_iter()
                .find(|r| text(&r.data, "id") == text(&ex.data, "id"));
            Ok(json!({"exchange":ex.data,"bodies":body.map(|r|r.data.clone())}))
        }
        "traffic_search" => {
            let host = required(p, "host")?;
            let host = url::Url::parse(host)
                .ok()
                .and_then(|u| u.host_str().map(String::from))
                .unwrap_or_else(|| host.into());
            let q = text(p, "contains").to_lowercase();
            if !text(p, "body_contains").is_empty() {
                return Err(Error::new(
                    "FEATURE_DISABLED",
                    "traffic body search requires blob reader",
                ));
            }
            let mut rows = s
                .records("exchanges")
                .iter()
                .filter(|r| {
                    text(&r.data, "host") == host
                        && text(&r.data, "url").to_lowercase().contains(&q)
                })
                .map(|r| r.data.clone())
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                text(b, "ts")
                    .parse::<i64>()
                    .unwrap_or(0)
                    .cmp(&text(a, "ts").parse::<i64>().unwrap_or(0))
            });
            let total = rows.len();
            let limit = p["limit"].as_u64().unwrap_or(3).min(10) as usize;
            let page = p["page"].as_u64().unwrap_or(0) as usize;
            Ok(
                json!({"items":rows.into_iter().skip(page.saturating_mul(limit)).take(limit).collect::<Vec<_>>(),"total":total,"page":page}),
            )
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
