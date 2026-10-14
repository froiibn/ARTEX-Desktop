use crate::*;
use std::collections::BTreeMap;
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
    for item in array {
        let name = required(item, "name")?;
        let rows = s
            .records("skill_usage")
            .into_iter()
            .filter(|r| r.data["skill"] == name && r.data["found"] == true)
            .collect::<Vec<_>>();
        let mut d = json!({"name":name,"files":item["files"].as_array().cloned().unwrap_or_default(),"calls":rows.len(),"tasks":rows.iter().filter_map(|r|r.data["task_id"].as_str()).collect::<BTreeSet<_>>().len(),"usage_agents":rows.iter().filter_map(|r|r.data["agent_key"].as_str()).filter(|v|!v.is_empty()).collect::<BTreeSet<_>>()});
        for key in ["description", "license", "compatibility", "mcps"] {
            if let Some(value) = item.get(key) {
                if !value.is_null() {
                    d[key] = value.clone();
                }
            }
        }
        if let Some(ts) = rows.iter().map(|r| text(&r.data, "ts")).max() {
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
            let mut groups = BTreeMap::<String, Vec<&Record>>::new();
            for r in s
                .records("skill_usage")
                .into_iter()
                .filter(|r| r.data["found"] == false)
            {
                groups
                    .entry(text(&r.data, "skill").into())
                    .or_default()
                    .push(r);
            }
            let mut rows=groups.into_iter().map(|(skill,rows)|json!({"skill":skill,"calls":rows.len(),"agents":rows.iter().filter_map(|r|r.data["agent_key"].as_str()).filter(|v|!v.is_empty()).collect::<BTreeSet<_>>(),"last_used":rows.iter().map(|r|text(&r.data,"ts")).max()})).collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                b["calls"]
                    .as_u64()
                    .cmp(&a["calls"].as_u64())
                    .then_with(|| a["skill"].as_str().cmp(&b["skill"].as_str()))
            });
            rows.truncate(limit);
            Ok(json!(rows))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
