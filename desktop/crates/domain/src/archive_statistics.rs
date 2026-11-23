//! Cold statistics retain the Source totals while task rows live in a package.
use super::*;

/// Version 2 counts the one logical archived task, rather than ledger registry keys.
pub const ARCHIVE_STATISTICS_VERSION: u64 = 2;

pub fn archived_records(s: &Snapshot) -> Vec<&Record> {
    // Match Source ArchivedAggregateStats: queue state alone does not make data cold.
    s.records("task_archives")
        .into_iter()
        .filter(|a| {
            s.get("tasks", text(&a.data, "task_id"))
                .is_some_and(|t| !t.data["archived_at"].is_null())
        })
        .collect()
}
fn add_number(target: &mut Value, incoming: &Value, key: &str) -> Result<()> {
    let value = |row: &Value| match row.get(key) {
        None | Some(Value::Null) => Ok(0), // Source archive fields are additive.
        Some(v) => v.as_u64().ok_or_else(|| {
            Error::new(
                "CORRUPT_STORE",
                "Archived statistic is not a nonnegative integer",
            )
        }),
    };
    let a = value(target)?;
    let b = value(incoming)?;
    target[key] = count(
        a.checked_add(b)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Archive statistic overflow"))?,
    )?;
    Ok(())
}
/// Project the Source archive DTO without changing rich native statistics.
/// Native profile/day/class dimensions stay in the package and query view;
/// Source's public summary admits only numeric tokens, skills, tools and findings.
pub(super) fn public_summary(stats: &Value) -> Result<Value> {
    if !stats.is_object() {
        return Err(Error::new(
            "CORRUPT_STORE",
            "Archived statistics must be an object",
        ));
    }
    let mut out = json!({});
    for key in ["tokens", "skills", "tools", "findings"] {
        if let Some(value) = stats.get(key) {
            let values = value.as_object().ok_or_else(|| {
                Error::new(
                    "CORRUPT_STORE",
                    "Archived summary must contain numeric objects",
                )
            })?;
            let mut validated = json!({});
            for (field, value) in values {
                validated[field] = count(value.as_u64().ok_or_else(|| {
                    Error::new("CORRUPT_STORE", "Archived summary count is invalid")
                })?)?;
            }
            out[key] = validated;
        }
    }
    if out.get("tokens").is_none() {
        if let Some(profiles) = stats.get("token_profiles") {
            let profiles = profiles.as_array().ok_or_else(|| {
                Error::new("CORRUPT_STORE", "Archived token profiles must be an array")
            })?;
            let mut tokens = json!({"calls":0,"input_tokens":0,"output_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0});
            for profile in profiles {
                for field in [
                    "calls",
                    "input_tokens",
                    "output_tokens",
                    "cache_read_tokens",
                    "cache_write_tokens",
                ] {
                    add_number(&mut tokens, profile, field)?;
                }
            }
            out["tokens"] = tokens;
        }
    }
    if out.get("findings").is_none() {
        if let Some(findings) = stats.get("finding_stats") {
            if !findings.is_object() {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Archived finding statistics must be an object",
                ));
            }
            let mut counts = json!({});
            for field in ["total", "pending", "critical", "high", "medium", "low"] {
                add_number(&mut counts, findings, field)?;
            }
            out["findings"] = counts;
        }
    }
    Ok(out)
}
fn finding_stats(rows: &[&Value]) -> Result<Value> {
    let mut out = json!({"total":count(rows.len() as u64)?,"pending":count(rows.iter().filter(|r| r["status"]=="pending").count() as u64)?,"vulnclasses":rows.iter().filter_map(|r|r["vulnclass"].as_str()).filter(|v|!v.is_empty()).collect::<BTreeSet<_>>(),"tasks":[]});
    for severity in ["critical", "high", "medium", "low"] {
        out[severity] = count(rows.iter().filter(|r| r["severity"] == severity).count() as u64)?;
    }
    Ok(out)
}
fn usage(
    rows: &[&Value],
    since: Option<&chrono::DateTime<Utc>>,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let mut profiles = BTreeMap::<String, (u64, BTreeSet<String>, Tokens)>::new();
    let mut days = BTreeMap::<(String, String), Tokens>::new();
    for row in rows {
        let name = text(row, "profile_name");
        let entry = profiles.entry(name.into()).or_default();
        entry.0 = entry
            .0
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Archive call count overflow"))?;
        if !row["task_id"].is_null() && row["task_id"] != "" {
            entry.1.insert(row["task_id"].to_string());
        }
        entry.2.add(row, true)?;
        // Source UsageDaily filters live timestamps before grouping. A day-only
        // cutoff would include calls before the requested rolling window.
        if since.map_or(Ok(true), |cutoff| {
            statistics_at_or_after(&row["ts"], cutoff)
        })? {
            let date = text(row, "ts").get(..10).unwrap_or("");
            days.entry((name.into(), date.into()))
                .or_default()
                .add(row, true)?;
        }
    }
    let profiles = profiles
        .into_iter()
        .map(|(profile_name, (calls, tasks, tokens))| {
            let mut row = tokens.wire()?;
            row["profile_name"] = json!(profile_name);
            row["calls"] = count(calls)?;
            row["tasks"] = count(tasks.len() as u64)?;
            Ok(row)
        })
        .collect::<Result<Vec<_>>>()?;
    let days = days
        .into_iter()
        .map(|((profile_name, date), tokens)| {
            let mut row = tokens.wire()?;
            row.as_object_mut().unwrap().remove("cache_write_tokens");
            row["profile_name"] = json!(profile_name);
            row["date"] = json!(date);
            Ok(row)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((profiles, days))
}
/// Compute compact statistics from the verified package's task-owned rows.
/// A package can include shared dependencies, which must not be counted twice.
pub fn archive_statistics(tables: &BTreeMap<String, Vec<Value>>, task_id: &str) -> Result<Value> {
    let exploration = tables
        .get("tasks")
        .into_iter()
        .flatten()
        .find(|t| t["id"] == task_id)
        .map(|t| &t["exploration_id"]);
    let owned = |collection: &str| {
        tables
            .get(collection)
            .into_iter()
            .flatten()
            // Source usage/task archive ownership also follows exploration_id.
            // Legacy registry strings remain opaque; do not rewrite them to IDs.
            .filter(|r| {
                r["task_id"] == task_id
                    || collection != "findings"
                        && exploration
                            .is_some_and(|exp| !exp.is_null() && r["exploration_id"] == *exp)
            })
            .collect::<Vec<_>>()
    };
    let findings = finding_stats(&owned("findings"))?;
    let (mut profiles, daily) = usage(&owned("llm_usage"), None)?;
    // Source taskArchiveAggregates assigns `1 tasks` per nonempty profile/skill
    // group. Registry aliases and nullable task FKs still belong to this one
    // archive through exploration_id; their stored identifiers stay untouched.
    for profile in &mut profiles {
        profile["tasks"] = json!(1);
    }
    let calls = |collection: &str, field: &str| -> Result<Value> {
        let mut counts = BTreeMap::<String, u64>::new();
        for row in owned(collection)
            .into_iter()
            .filter(|r| collection != "skill_usage" || r["found"] == true)
        {
            let n = counts.entry(text(row, field).into()).or_default();
            *n = n
                .checked_add(1)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Archive usage count overflow"))?;
        }
        Ok(json!(counts))
    };
    let skill_rows = owned("skill_usage");
    let mut skills = skill_rows_summary(&skill_rows, true)?;
    for skill in &mut skills {
        skill["tasks"] = json!(1);
    }
    Ok(
        json!({"native_statistics_version":ARCHIVE_STATISTICS_VERSION,"finding_stats":findings,"token_profiles":profiles,"token_daily":daily,"skills":calls("skill_usage","skill")?,"tools":calls("tool_usage","tool_key")?,"skill_stats":skills,"missing_skill_stats":skill_rows_summary(&skill_rows,false)?}),
    )
}
fn hot_usage<'a>(s: &'a Snapshot, collection: &str, archives: &[&Record]) -> Vec<&'a Value> {
    let tasks = archives
        .iter()
        .map(|a| text(&a.data, "task_id"))
        .collect::<BTreeSet<_>>();
    let explorations = tasks
        .iter()
        .filter_map(|id| s.get("tasks", id))
        .filter_map(|t| t.data["exploration_id"].as_str())
        .collect::<BTreeSet<_>>();
    s.records(collection)
        .into_iter()
        .filter(|r| {
            !tasks.contains(text(&r.data, "task_id"))
                && !explorations.contains(text(&r.data, "exploration_id"))
        })
        .map(|r| &r.data)
        .collect()
}
fn skill_rows_summary(rows: &[&Value], found: bool) -> Result<Vec<Value>> {
    let mut groups = BTreeMap::<String, Vec<&Value>>::new();
    for row in rows.iter().copied().filter(|r| r["found"] == found) {
        groups
            .entry(text(row, "skill").into())
            .or_default()
            .push(row);
    }
    groups.into_iter().map(|(skill,rows)| Ok(json!({"skill":skill,"calls":count(rows.len() as u64)?,"tasks":if found {rows.iter().filter_map(|r|r["task_id"].as_str()).filter(|v|!v.is_empty()).collect::<BTreeSet<_>>().len()} else {0},"agents":rows.iter().filter_map(|r|r["agent_key"].as_str()).filter(|v|!v.is_empty()).collect::<BTreeSet<_>>(),"last_used":rows.iter().map(|r|text(r,"ts")).filter(|v|!v.is_empty()).max()}))).collect()
}
/// Source catalog call totals include immutable cold-task ledgers.
pub fn tool_usage_counts(s: &Snapshot) -> Result<BTreeMap<String, u64>> {
    let archives = archived_records(s);
    let mut counts = BTreeMap::<String, u64>::new();
    for row in hot_usage(s, "tool_usage", &archives) {
        *counts.entry(text(row, "tool_key").into()).or_default() += 1;
    }
    for archive in archives {
        for (key, calls) in aggregate(archive, "tools")?
            .as_object()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Archived tool counts are not an object"))?
        {
            let incoming = calls
                .as_u64()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Archived tool count is invalid"))?;
            let n = counts.entry(key.clone()).or_default();
            *n = n
                .checked_add(incoming)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Tool usage overflow"))?;
            count(*n)?;
        }
    }
    Ok(counts)
}
/// Merge successful or missing skill summaries before applying UI pagination.
pub fn skill_statistics(s: &Snapshot, found: bool) -> Result<Vec<Value>> {
    let archives = archived_records(s);
    let mut groups = skill_rows_summary(&hot_usage(s, "skill_usage", &archives), found)?
        .into_iter()
        .map(|r| (text(&r, "skill").to_owned(), r))
        .collect::<BTreeMap<_, _>>();
    for archive in archives {
        let stats = &archive.data["aggregate_stats"];
        let field = if found {
            "skill_stats"
        } else {
            "missing_skill_stats"
        };
        let legacy;
        let cold = if let Some(array) = stats[field].as_array() {
            array.as_slice()
        } else if found {
            // Source format v1 initially kept successful call counts only.
            legacy=stats["skills"].as_object().into_iter().flatten().map(|(skill,calls)|json!({"skill":skill,"calls":calls,"tasks":1,"agents":[],"last_used":null})).collect::<Vec<_>>();
            legacy.as_slice()
        } else {
            &[]
        };
        for row in cold {
            let key = text(row, "skill").to_owned();
            let current = groups.entry(key.clone()).or_insert_with(
                || json!({"skill":key,"calls":0,"tasks":0,"agents":[],"last_used":null}),
            );
            add_number(current, row, "calls")?;
            add_number(current, row, "tasks")?;
            let agents = current["agents"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(row["agents"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            current["agents"] = json!(agents);
            if text(row, "last_used") > text(current, "last_used") {
                current["last_used"] = row["last_used"].clone();
            }
        }
    }
    let mut rows = groups.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b["calls"]
            .as_u64()
            .cmp(&a["calls"].as_u64())
            .then_with(|| text(a, "skill").cmp(text(b, "skill")))
    });
    Ok(rows)
}
fn aggregate<'a>(row: &'a Record, field: &str) -> Result<&'a Value> {
    row.data["aggregate_stats"].get(field).ok_or_else(|| {
        Error::new(
            "RECOVERY_REQUIRED",
            "Archived statistics require verified package recovery",
        )
    })
}
pub(super) fn global_finding_stats(s: &Snapshot) -> Result<Value> {
    let archives = archived_records(s);
    let cold_tasks = archives
        .iter()
        .map(|a| text(&a.data, "task_id"))
        .collect::<BTreeSet<_>>();
    let live = s
        .records("findings")
        .into_iter()
        .filter(|r| !cold_tasks.contains(text(&r.data, "task_id")))
        .collect::<Vec<_>>();
    let mut out = finding_stats(&live.iter().map(|r| &r.data).collect::<Vec<_>>())?;
    let mut classes = out["vulnclasses"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    for archive in archives {
        let cold = aggregate(archive, "finding_stats")?;
        for key in ["total", "pending", "critical", "high", "medium", "low"] {
            add_number(&mut out, cold, key)?;
        }
        for name in cold["vulnclasses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !name.is_empty() {
                classes.insert(name.into());
            }
        }
    }
    out["vulnclasses"] = json!(classes);
    let mut tasks = BTreeMap::<String, u64>::new();
    for row in live {
        if let Some(id) = row.data["task_id"].as_str() {
            *tasks.entry(id.into()).or_default() += 1;
        }
    }
    out["tasks"] = json!(tasks.into_iter().filter_map(|(id,n)|s.get("tasks",&id).map(|task|json!({"id":id,"name":task.data["name"],"description":task.data["description"],"count":n}))).collect::<Vec<_>>());
    Ok(out)
}
pub(super) fn global_usage_stats(s: &Snapshot, p: &Value) -> Result<Value> {
    let cutoff = statistics_cutoff(p, 365)?;
    let archives = archived_records(s);
    let cold_tasks = archives
        .iter()
        .map(|a| text(&a.data, "task_id"))
        .collect::<BTreeSet<_>>();
    let cold_explorations = cold_tasks
        .iter()
        .filter_map(|id| s.get("tasks", id))
        .filter_map(|t| t.data["exploration_id"].as_str())
        .collect::<BTreeSet<_>>();
    let live = s
        .records("llm_usage")
        .into_iter()
        .filter(|r| {
            !cold_tasks.contains(text(&r.data, "task_id"))
                && !cold_explorations.contains(text(&r.data, "exploration_id"))
        })
        .map(|r| &r.data)
        .collect::<Vec<_>>();
    let (profiles, daily) = usage(&live, Some(&cutoff))?;
    let mut profiles = profiles
        .into_iter()
        .map(|r| (text(&r, "profile_name").to_owned(), r))
        .collect::<BTreeMap<_, _>>();
    let mut daily = daily
        .into_iter()
        .map(|r| {
            (
                (
                    text(&r, "profile_name").to_owned(),
                    text(&r, "date").to_owned(),
                ),
                r,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for archive in archives {
        for row in aggregate(archive, "token_profiles")?
            .as_array()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Archived profiles are not an array"))?
        {
            let key = text(row, "profile_name").to_owned();
            if let Some(current) = profiles.get_mut(&key) {
                for field in [
                    "calls",
                    "tasks",
                    "input_tokens",
                    "output_tokens",
                    "cache_read_tokens",
                    "cache_write_tokens",
                ] {
                    add_number(current, row, field)?;
                }
            } else {
                profiles.insert(key, row.clone());
            }
        }
        for row in aggregate(archive, "token_daily")?
            .as_array()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Archived days are not an array"))?
        {
            let key = (
                text(row, "profile_name").to_owned(),
                text(row, "date").to_owned(),
            );
            if let Some(current) = daily.get_mut(&key) {
                for field in ["input_tokens", "output_tokens", "cache_read_tokens"] {
                    add_number(current, row, field)?;
                }
            } else {
                daily.insert(key, row.clone());
            }
        }
    }
    // Source cold packages retain daily buckets, so their oldest day is merged
    // by UTC date while live rows above retain the precise timestamp boundary.
    let cutoff = cutoff.format("%Y-%m-%d").to_string();
    Ok(
        json!({"by_profile":profiles.into_values().collect::<Vec<_>>(),"daily":daily.into_values().filter(|r|text(r,"date")>=cutoff.as_str()).collect::<Vec<_>>()}),
    )
}
