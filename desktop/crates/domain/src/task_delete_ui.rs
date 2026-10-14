//! Pure, reference-aware deletion; native file retirement is coordinated by the router.
use crate::*;
use url::Url;
pub const COMMANDS: &[&str] = &["ui.deleteTask"];
fn ownership(s: &Snapshot, a: &Record, task: &str, exp: &str) -> (bool, bool) {
    let candidates = ids(&a.data, "task_ids");
    let mut own = candidates.iter().any(|id| id == task)
        || s.records("task_asset_links")
            .iter()
            .any(|l| l.data["task_id"] == task && l.data["asset_id"] == a.header.id);
    let mut shared = candidates.iter().any(|id| {
        id != task
            && s.get("tasks", id)
                .is_some_and(|t| t.data["deleted_at"].is_null())
    });
    shared |= s.records("task_asset_links").into_iter().any(|link| {
        link.data["asset_id"] == a.header.id
            && text(&link.data, "task_id") != task
            && s.get("tasks", text(&link.data, "task_id"))
                .is_some_and(|t| t.data["deleted_at"].is_null())
    });
    for anchor in s
        .records("exploration_anchors")
        .into_iter()
        .filter(|l| l.data["asset_id"] == a.header.id)
    {
        if let Some(n) = s.get("exploration_nodes", text(&anchor.data, "node_id")) {
            own |= n.data["exploration_id"] == exp;
            shared |= s.records("tasks").iter().any(|t| {
                t.header.id != task
                    && t.data["deleted_at"].is_null()
                    && t.data["exploration_id"] == n.data["exploration_id"]
            });
        }
    }
    (own, shared)
}
pub fn exclusive_hosts(s: &Snapshot, task: &str) -> Result<Vec<String>> {
    let t = record(s, "tasks", task)?;
    let mut candidate = BTreeSet::new();
    let mut protected = BTreeSet::new();
    for a in s.records("assets") {
        let (own, shared) = ownership(s, a, task, text(&t.data, "exploration_id"));
        let mut hosts = vec![
            text(&a.data, "domain").to_lowercase(),
            text(&a.data, "ip").to_lowercase(),
        ];
        if let Ok(url) = Url::parse(text(&a.data, "url")) {
            if let Some(host) = url.host_str() {
                hosts.push(host.trim_matches(['[', ']']).to_lowercase());
            }
        }
        for host in hosts.into_iter().filter(|h| !h.is_empty()) {
            if own {
                candidate.insert(host.clone());
            }
            if shared {
                protected.insert(host);
            }
        }
    }
    Ok(candidate.difference(&protected).cloned().collect())
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    if m != "ui.deleteTask" {
        return Err(Error::new("METHOD_NOT_FOUND", m));
    }
    let task = required(p, "id")?;
    let t = record(s, "tasks", task)?;
    let exp = required(&t.data, "exploration_id")?;
    let options = &p["options"];
    let hosts = exclusive_hosts(s, task)?;
    let mut b = Planner::new(s, m, p);
    let mut result = json!({"deleted":task,"assets_deleted":0,"assets_detached":0,"traffic_deleted":0,"files_deleted":false,"findings_deleted":0,"llm_records_deleted":0});
    for run in s
        .records("agent_runs")
        .into_iter()
        .filter(|r| r.data["task_id"] == task)
    {
        conversation::cancel_run(&mut b, c, &run.header.id)?;
    }
    for side in s
        .records("side_question_sessions")
        .into_iter()
        .filter(|r| r.data["task_id"] == task)
    {
        b.delete("side_question_sessions", &side.header.id);
    }
    if options["delete_assets"] == true {
        for a in s.records("assets") {
            let (own, shared) = ownership(s, a, task, exp);
            if own && !shared {
                b.delete("assets", &a.header.id);
                result["assets_deleted"] = json!(result["assets_deleted"].as_u64().unwrap() + 1);
            } else if ids(&a.data, "task_ids").iter().any(|id| id == task) {
                let mut d = a.data.clone();
                d["task_ids"] = json!(
                    ids(&d, "task_ids")
                        .into_iter()
                        .filter(|id| id != task)
                        .collect::<Vec<_>>()
                );
                b.put("assets", &a.header.id, d);
                result["assets_detached"] = json!(result["assets_detached"].as_u64().unwrap() + 1);
            }
        }
    }
    if options["delete_findings"] == true {
        for finding in s
            .records("findings")
            .into_iter()
            .filter(|f| f.data["task_id"] == task)
        {
            b.delete("findings", &finding.header.id);
            result["findings_deleted"] = json!(result["findings_deleted"].as_u64().unwrap() + 1);
        }
    }
    if options["delete_llm_records"] == true {
        for r in s
            .records("llm_records")
            .into_iter()
            .filter(|r| r.data["task_id"] == task)
        {
            b.delete("llm_records", &r.header.id);
            result["llm_records_deleted"] =
                json!(result["llm_records_deleted"].as_u64().unwrap() + 1);
        }
    }
    if options["delete_traffic"] == true {
        for r in s
            .records("exchanges")
            .into_iter()
            .filter(|r| hosts.iter().any(|h| h == text(&r.data, "host")))
        {
            b.delete("exchanges", &r.header.id);
            result["traffic_deleted"] = json!(result["traffic_deleted"].as_u64().unwrap() + 1);
        }
    }
    b.delete("tasks", task);
    b.delete("explorations", exp);
    conversation::push_event(&mut b.batch, s, c, "TaskDeleted", "tasks", task)?;
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}
