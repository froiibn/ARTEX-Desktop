//! Pure failure transitions; the controller retries only before any output is committed.
use crate::*;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
pub fn quota_exhausted(message: &str) -> bool {
    let message = message.to_lowercase();
    if Regex::new(r"(?:status(?:\s+code)?|http(?:\s+status)?)\s*[=:]?\s*(?:401|403|5\d\d)\b").unwrap().is_match(&message)||Regex::new(r"(?i)(?:\b(?:rpm|tpm|rpd|qps)\b|quota[_\s-]*metric|rate[_\s-]*limit|too many requests|(?:requests?|tokens?)\s+(?:per|/)\s*(?:second|minute)|(?:per|/)\s*(?:second|minute)\s+(?:requests?|tokens?)|generate[_\s-]*requests[_\s-]*per[_\s-]*(?:minute|second)|tokens?[_\s-]*per[_\s-]*(?:minute|second))").unwrap().is_match(&message){return false;}
    [
        "insufficient_quota",
        "quota_exceeded",
        "quota exceeded",
        "quota exhausted",
        "exceeded your current quota",
        "billing_hard_limit_reached",
        "billing hard limit",
        "billing_not_active",
        "credit balance",
        "insufficient credit",
        "insufficient balance",
        "balance is too low",
        "payment required",
        "status 402",
        "insufficient account balance",
        "insufficient quota",
        "quota has been exhausted",
        "arrears",
        "\u{4f59}\u{989d}\u{4e0d}\u{8db3}",
        "\u{989d}\u{5ea6}\u{4e0d}\u{8db3}",
        "\u{914d}\u{989d}\u{5df2}\u{7528}\u{5c3d}",
        "\u{6b20}\u{8d39}",
        "resource_exhausted",
    ]
    .iter()
    .any(|s| message.contains(s))
}
fn configured(p: &Record) -> bool {
    crate::llm_ui::configured(p)
}
pub fn chain<'a>(s: &'a Snapshot, task: &str) -> Vec<&'a Record> {
    let mut rows = s
        .records("task_llm_profiles")
        .into_iter()
        .filter(|r| r.data["task_id"] == task)
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| r.data["position"].as_i64().unwrap_or(0));
    rows
}
pub fn profile_failure_plan(
    s: &Snapshot,
    c: &Context,
    rid: &str,
    error: &Value,
) -> Result<CommandBatch> {
    let run = record(s, "agent_runs", rid)?;
    let agent = record(s, "agents", required(&run.data, "agent_id")?)?;
    let task = run.data["task_id"]
        .as_str()
        .and_then(|id| s.get("tasks", id));
    let conv = run.data["conversation_id"]
        .as_str()
        .and_then(|id| s.get("conversations", id));
    let current = error["profile_id"]
        .as_str()
        .and_then(|id| s.get("llm_profiles", id))
        .or_else(|| conversation::effective_profile(s, agent, conv, task).ok())
        .ok_or_else(|| Error::new("FEATURE_DISABLED", "Failed profile is unavailable"))?;
    let message = error.as_str().unwrap_or_else(|| text(error, "message"));
    let safe = error["committed"] != true && error["cancelled"] != true;
    let mut b = Planner::new(
        s,
        "profile.failure",
        &json!({"run_id":rid,"profile_id":current.header.id,"error":error}),
    );
    b.expect(run);
    b.expect(current);
    let bound = agent.data["llm_profile_id"]
        .as_str()
        .and_then(|id| s.get("llm_profiles", id))
        .is_some_and(configured)
        || conv.is_some_and(|r| !r.data["llm_profile_id"].is_null());
    if let Some(task) = task.filter(|_| !bound) {
        let taskid = &task.header.id;
        let entries = chain(s, taskid);
        if !entries.is_empty() {
            let revision = text(&task.data, "llm_chain_revision");
            if error["chain_revision"]
                .as_str()
                .is_some_and(|r| r != revision)
            {
                return Ok(b.done(json!({"should_retry":safe&&!task.data["active_llm_profile_id"].is_null(),"profile_changed":true,"next_profile_id":task.data["active_llm_profile_id"],"chain_exhausted":task.data["active_llm_profile_id"].is_null(),"stale":true})));
            }
            let explicit_status = error["status"].as_u64().unwrap_or(0);
            if matches!(explicit_status, 401 | 403 | 500..=599)
                || !(explicit_status == 402 || quota_exhausted(message))
            {
                return Ok(b.done(json!({"should_retry":false,"profile_changed":false,"next_profile_id":null,"chain_exhausted":false,"stale":false})));
            }
            let Some(entry) = entries
                .iter()
                .find(|e| e.data["profile_id"] == current.header.id)
            else {
                return Ok(b.done(json!({"should_retry":false,"profile_changed":false,"next_profile_id":null,"chain_exhausted":false,"stale":false})));
            };
            if task.data["active_llm_profile_id"].is_null() {
                return Ok(b.done(json!({"should_retry":false,"profile_changed":false,"next_profile_id":null,"chain_exhausted":true,"stale":false})));
            }
            let mut data = entry.data.clone();
            if data["status"] != "quota_exhausted" {
                data["status"] = json!("quota_exhausted");
                data["exhausted_at"] = json!(artex_core::now());
                let mut end = message.trim().len().min(1000);
                while !message.trim().is_char_boundary(end) {
                    end -= 1;
                }
                data["last_error"] = json!(&message.trim()[..end]);
                b.put("task_llm_profiles", &entry.header.id, data);
            }
            let active = text(&task.data, "active_llm_profile_id");
            let next = if active != current.header.id {
                Some(active.to_owned())
            } else {
                entries
                    .iter()
                    .find(|e| {
                        e.data["position"].as_i64() > entry.data["position"].as_i64()
                            && e.data["status"] == "ready"
                    })
                    .map(|e| text(&e.data, "profile_id").to_owned())
            };
            let changed = active == current.header.id;
            if changed {
                let mut td = task.data.clone();
                td["llm_profile_id"] = json!(next);
                td["active_llm_profile_id"] = json!(next);
                td["llm_chain_revision"] =
                    json!((revision.parse::<u64>().unwrap_or(0) + 1).to_string());
                b.put("tasks", taskid, td);
                let old = profile_summary(current);
                let new = next
                    .as_deref()
                    .and_then(|id| s.get("llm_profiles", id))
                    .map(profile_summary);
                b.insert("activity","activity",json!({"exploration_id":task.data["exploration_id"],"worker":"system","kind":if next.is_some(){"llm_switch"}else{"llm_failover"},"summary":if next.is_some(){"Quota exhausted; subsequent calls use the next profile"}else{"Task LLM profile chain exhausted"},"detail":artex_core::body(message),"is_error":next.is_none(),"metadata":{"llm_transition":{"mode":if next.is_some(){"automatic"}else{"exhausted"},"reason":message,"previous":old,"next":new}}}))?;
                conversation::push_event(&mut b.batch, s, c, "LLMProfileChanged", "tasks", taskid)?;
            }
            let mut out=b.done(json!({"should_retry":safe&&next.is_some(),"profile_changed":changed,"next_profile_id":next,"chain_exhausted":next.is_none(),"stale":false}));
            augment_expected(&mut out, s)?;
            return Ok(out);
        }
    }
    let pool = setting_bool(s, "llm_pool_enabled", false)
        && (!bound || setting_bool(s, "llm_pool_bind_fallback", false));
    let status = error["status"]
        .as_i64()
        .or_else(|| {
            Regex::new(r"(?i)(?:status(?:\s+code)?|http(?:\s+status)?)\s*[=:]?\s*(\d{3})\b")
                .unwrap()
                .captures(message)
                .and_then(|c| c[1].parse().ok())
        })
        .unwrap_or(0);
    let failover = pool
        && safe
        && error["cancelled"] != true
        && (status == 0 || [401, 402, 403, 404, 408, 429].contains(&status) || status >= 500);
    if !failover {
        return Ok(b.done(json!({"should_retry":false,"profile_changed":false,"next_profile_id":null,"chain_exhausted":false,"stale":false})));
    }
    let health = s
        .records("llm_profile_health")
        .into_iter()
        .find(|h| h.data["profile_id"] == current.header.id);
    let mut h = health
        .map(|r| r.data.clone())
        .unwrap_or(default_data("llm_profile_health")?);
    h["profile_id"] = json!(current.header.id);
    h["fails"] = json!(h["fails"].as_i64().unwrap_or(0) + 1);
    let retry: Value = serde_json::from_str(&configuration_ui::setting(s, "llm_retry_policy"))
        .unwrap_or(json!({}));
    let threshold = match retry["breaker"]["attempts"].as_i64().unwrap_or(0) {
        0 => 3,
        n => n.max(0),
    };
    let trip = [401, 402, 403, 404].contains(&status)
        || threshold > 0 && h["fails"].as_i64().unwrap() >= threshold;
    if trip {
        let trips = h["trips"].as_i64().unwrap_or(0) + 1;
        h["trips"] = json!(trips);
        h["fails"] = json!(0);
        let cooldown = retry["breaker"]["interval_ms"]
            .as_i64()
            .filter(|n| *n > 0)
            .unwrap_or(match trips {
                1 => 60000,
                2 => 300000,
                _ => 1800000,
            });
        h["open_until"] = json!(
            (Utc::now() + Duration::milliseconds(cooldown))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
    }
    h["last_at"] = json!(artex_core::now());
    h["last_error"] = json!(
        message
            .replace('\n', " ")
            .chars()
            .take(300)
            .collect::<String>()
    );
    if let Some(health) = health {
        b.put("llm_profile_health", &health.header.id, h);
    } else {
        b.insert("llm_profile_health", "profilehealth", h)?;
    }
    let mut members = s
        .records("llm_profiles")
        .into_iter()
        .filter(|p| {
            configured(p)
                && p.data["pool_exclude"] != true
                && p.header.id != current.header.id
                && !s.records("llm_profile_health").iter().any(|h| {
                    h.data["profile_id"] == p.header.id
                        && DateTime::parse_from_rfc3339(text(&h.data, "open_until"))
                            .is_ok_and(|t| t > Utc::now())
                })
        })
        .collect::<Vec<_>>();
    members.sort_by(|a, b| {
        b.data["is_default"]
            .as_bool()
            .cmp(&a.data["is_default"].as_bool())
            .then_with(|| {
                b.data["priority"]
                    .as_i64()
                    .cmp(&a.data["priority"].as_i64())
            })
            .then_with(|| a.header.id.cmp(&b.header.id))
    });
    let next = members.first().map(|p| p.header.id.clone());
    let mut out=b.done(json!({"should_retry":next.is_some(),"profile_changed":next.is_some(),"next_profile_id":next,"chain_exhausted":false,"pool_exhausted":next.is_none(),"stale":false}));
    augment_expected(&mut out, s)?;
    Ok(out)
}
pub fn profile_summary(p: &Record) -> Value {
    json!({"id":p.header.id,"name":p.data["name"],"format":p.data["format"],"model":p.data["model"]})
}

pub fn profile_success_plan(s: &Snapshot, c: &Context, profile_id: &str) -> Result<CommandBatch> {
    record(s, "llm_profiles", profile_id)?;
    let mut b = Planner::new(s, "profile.success", &json!({"profile_id":profile_id}));
    for h in s.records("llm_profile_health") {
        if h.data["profile_id"] == profile_id {
            b.delete("llm_profile_health", &h.header.id);
        }
    }
    let _ = c;
    let mut out = b.done(json!({"ok":true}));
    augment_expected(&mut out, s)?;
    Ok(out)
}
