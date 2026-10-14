use crate::*;
pub const QUERIES: &[&str] = &["legacy.getLogs", "legacy.getLogsHistory"];
fn number(p: &Value, key: &str, default: u64) -> u64 {
    p[key]
        .as_u64()
        .or_else(|| p[key].as_str().and_then(|v| v.parse().ok()))
        .unwrap_or(default)
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    if !QUERIES.contains(&m) {
        return Err(Error::new("METHOD_NOT_FOUND", m));
    }
    let q = p.get("query").unwrap_or(p);
    let history = m == "legacy.getLogsHistory";
    let limit = number(q, "limit", if history { 200 } else { 500 }).clamp(1, 500) as usize;
    let mut rows = s.records("server_logs");
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
    if history {
        let before = text(q, "before");
        if !before.is_empty() && before != "0" {
            let index = rows
                .iter()
                .position(|r| r.header.id == before)
                .ok_or_else(|| Error::new("NOT_FOUND", "Log history cursor no longer exists"))?;
            rows.truncate(index);
        }
        let more = rows.len() >= limit;
        let skip = rows.len().saturating_sub(limit);
        let rows = rows.into_iter().skip(skip).collect::<Vec<_>>();
        return Ok(
            json!({"items":rows.iter().map(|r|line(r,true)).collect::<Vec<_>>(),"has_more":more}),
        );
    }
    let since = text(q, "since");
    if since.starts_with("log_") {
        let index = rows
            .iter()
            .position(|r| r.header.id == since)
            .ok_or_else(|| Error::new("NOT_FOUND", "Log cursor no longer exists"))?;
        rows = rows.into_iter().skip(index + 1).collect();
    } else {
        let since = number(q, "since", 0);
        rows.retain(|r| r.header.commit_seq.parse::<u64>().unwrap_or(0) > since);
    }
    rows.truncate(limit);
    let cursor = rows.last().map(|r| r.header.id.clone()).unwrap_or_else(|| {
        if since.is_empty() {
            "0".into()
        } else {
            since.into()
        }
    });
    Ok(json!({"items":rows.iter().map(|r|line(r,false)).collect::<Vec<_>>(),"cursor":cursor}))
}
fn line(r: &Record, history: bool) -> Value {
    json!({"seq":if history{"0"}else{r.header.id.as_str()},"db_id":r.header.id,"ts":r.data["created_at"],"level":text(&r.data,"level"),"tag":text(&r.data,"tag"),"text":text(&r.data,"text")})
}
