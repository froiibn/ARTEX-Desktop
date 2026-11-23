use crate::*;
pub const QUERIES: &[&str] = &["ui.skillUsage", "ui.missingSkills"];
/// Decorate native filesystem metadata with the persistent successful-call ledger.
pub fn decorate_skills(items: &Value, s: &Snapshot) -> Result<Value> {
    let array = items
        .as_array()
        .or_else(|| items["skills"].as_array())
        .ok_or_else(|| {
            Error::new(
                "INVALID_PARAMS",
                "Native skills result must contain an array",
            )
        })?;
    let mut out = vec![];
    let usage = crate::auxiliary_ui::skill_statistics(s, true)?;
    for item in array {
        let name = required(item, "name")?;
        let row = usage.iter().find(|r| r["skill"] == name);
        let mut d = json!({"name":name,"files":item["files"].as_array().cloned().unwrap_or_default(),"calls":row.map(|r|r["calls"].clone()).unwrap_or(json!(0)),"tasks":row.map(|r|r["tasks"].clone()).unwrap_or(json!(0)),"usage_agents":row.map(|r|r["agents"].clone()).unwrap_or(json!([]))});
        for key in ["description", "license", "compatibility", "mcps"] {
            if let Some(value) = item.get(key) {
                if !value.is_null() {
                    d[key] = value.clone();
                }
            }
        }
        if let Some(ts) = row.and_then(|r| r["last_used"].as_str()) {
            d["last_used"] = json!(ts);
        }
        out.push(d);
    }
    Ok(json!(out))
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    match m {
        "ui.skillUsage" => {
            let name = required(p, "name")?;
            let limit = p["limit"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 200)
                .unwrap_or(50) as usize;
            let mut rows = s
                .records("skill_usage")
                .into_iter()
                .filter(|r| r.data["skill"] == name)
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                text(&b.data, "ts").cmp(text(&a.data, "ts")).then_with(|| {
                    b.header
                        .commit_seq
                        .parse::<u64>()
                        .unwrap_or(0)
                        .cmp(&a.header.commit_seq.parse::<u64>().unwrap_or(0))
                })
            });
            Ok(json!(rows.into_iter().take(limit).map(|r|json!({"ts":r.data["ts"],"agent_key":r.data["agent_key"].as_str().unwrap_or(""),"task_id":r.data["task_id"],"session_id":r.data["session_id"].as_str().unwrap_or(""),"args_len":r.data["args_len"]})).collect::<Vec<_>>()))
        }
        "ui.missingSkills" => {
            let limit = p["limit"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 200)
                .unwrap_or(20) as usize;
            let mut rows = crate::auxiliary_ui::skill_statistics(s, false)?;
            for row in &mut rows {
                row.as_object_mut().unwrap().remove("tasks");
            }
            rows.truncate(limit);
            Ok(json!(rows))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
