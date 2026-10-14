//! Workspace approval policy; the writer commits settings and pending barriers together.
use crate::*;
use chrono::{DateTime, Utc};
use regex::Regex;
use std::collections::BTreeSet;

pub fn enabled(s: &Snapshot) -> bool {
    setting_bool(s, "approval_bypass", false)
}
pub fn matching_deny(s: &Snapshot, name: &str, input: &Value) -> bool {
    if !policy::enabled_tools(s).iter().any(|tool| tool == name) {
        return false;
    }
    let mut rules: Vec<_> = s
        .records("intercept_rules")
        .into_iter()
        .filter(|r| r.data["enabled"] == true)
        .collect();
    rules.sort_by(|a, b| {
        b.data["priority"]
            .as_i64()
            .cmp(&a.data["priority"].as_i64())
            .then(a.header.created_at.cmp(&b.header.created_at))
            .then(a.header.id.cmp(&b.header.id))
    });
    let serialized = input.to_string();
    rules
        .into_iter()
        .find(|r| {
            let subject = match text(&r.data, "match_target") {
                "tool_name" => name,
                "tool_input" => &serialized,
                _ => return false,
            };
            if r.data["match_type"] == "regex" {
                Regex::new(text(&r.data, "pattern")).is_ok_and(|re| re.is_match(subject))
            } else {
                subject.contains(text(&r.data, "pattern"))
            }
        })
        .is_some_and(|r| r.data["action"] == "deny")
}
pub fn projected<'a>(b: &'a Planner, row: &'a Record) -> &'a Value {
    b.batch
        .changes
        .iter()
        .find_map(|change| match change {
            Change::Put {
                collection,
                id,
                data,
            } if collection == &row.header.collection && id == &row.header.id => Some(data),
            _ => None,
        })
        .unwrap_or(&row.data)
}
pub fn resume(b: &mut Planner, rid: &str, epoch: &str) {
    let Some(run) = b.state.get("agent_runs", rid) else {
        return;
    };
    let data = projected(b, run);
    if data["state"] != "waiting" || data["engine_epoch"] != epoch {
        return;
    }
    if b.state
        .records("user_questions")
        .iter()
        .any(|q| q.data["run_id"] == rid && projected(b, q)["state"] == "pending")
        || b.state
            .records("intercept_pending")
            .iter()
            .any(|p| p.data["audit"]["run_id"] == rid && projected(b, p)["status"] == "pending")
        || b.state.records("conversation_requests").iter().any(|q| {
            matches!(
                projected(b, q)["state"].as_str(),
                Some("open" | "claimed" | "waiting")
            ) && b
                .state
                .get("agent_messages", &q.header.id)
                .is_some_and(|m| m.data["sender"]["run_id"] == rid)
        })
    {
        return;
    }
    let mut data = data.clone();
    data["state"] = json!("running");
    b.put("agent_runs", rid, data);
}
fn active(s: &Snapshot, rid: &str, epoch: &str) -> bool {
    s.get("agent_runs", rid).is_some_and(|r| {
        r.data["engine_epoch"] == epoch
            && matches!(r.data["state"].as_str(), Some("running" | "waiting"))
    })
}
pub fn settle_pending(b: &mut Planner, c: &Context) -> Result<()> {
    let mut owners = BTreeSet::new();
    for row in b.state.records("intercept_pending") {
        if row.data["status"] != "pending" {
            continue;
        }
        let Some(rid) = row.data["audit"]["run_id"].as_str() else {
            continue;
        };
        if !active(b.state, rid, &c.actor.engine_epoch) {
            continue;
        }
        if row.data["audit"]["timeout_enabled"] == true
            && row.data["audit"]["deadline_at"]
                .as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t.with_timezone(&Utc) <= Utc::now())
        {
            continue;
        }
        let deny = matching_deny(
            b.state,
            text(&row.data, "tool_name"),
            &row.data["tool_input"],
        );
        let mut data = row.data.clone();
        data["status"] = json!(if deny { "denied" } else { "allowed" });
        data["decided_at"] = json!(artex_core::now());
        if !data["audit"].is_object() {
            data["audit"] = json!({});
        }
        data["audit"]["effective_action"] = json!(if deny { "deny" } else { "allow" });
        data["audit"]["decision_reason"] = json!(if deny {
            "Explicit intercept deny remains effective"
        } else {
            "Workspace approval bypass settled internal approval"
        });
        data["audit"]["approval_bypass"] = json!(!deny);
        data["audit"]["execution_status"] = json!(if deny {
            "not_executed"
        } else {
            "awaiting_result"
        });
        b.put("intercept_pending", &row.header.id, data);
        owners.insert(rid.to_owned());
    }
    for row in b.state.records("user_questions") {
        if row.data["state"] != "pending"
            || row.data["kind"] == "plan_approval"
            || row.data["payload"]["type"] != "tool_approval"
        {
            continue;
        }
        let Some(rid) = row.data["run_id"].as_str() else {
            continue;
        };
        if !active(b.state, rid, &c.actor.engine_epoch) {
            continue;
        }
        if row.data["deadline_at"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .is_none_or(|t| t.with_timezone(&Utc) <= Utc::now())
        {
            continue;
        }
        let mut data = row.data.clone();
        data["state"] = json!("answered");
        data["answer"] = json!({"approved":true,"approval_bypass":true});
        b.put("user_questions", &row.header.id, data);
        owners.insert(rid.to_owned());
    }
    for rid in owners {
        resume(b, &rid, &c.actor.engine_epoch);
    }
    Ok(())
}
/// A manual grant is bound to the current epoch and the exact normalized call.
pub fn manual_grant(
    s: &Snapshot,
    epoch: &str,
    rid: &str,
    toolid: &str,
    name: &str,
    input: &Value,
) -> bool {
    s.records("intercept_pending").iter().any(|row| {
        let audit = &row.data["audit"];
        row.data["status"] == "allowed"
            && audit["decision_origin"] == "user"
            && audit["decision_epoch"] == epoch
            && audit["run_id"] == rid
            && audit["tool_use_id"] == toolid
            && row.data["tool_name"] == name
            && row.data["tool_input"] == *input
            && row.data["decided_at"]
                .as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .is_some()
    })
}
pub fn mark_manual_decision(
    b: &mut Planner,
    id: &str,
    c: &Context,
    allowed: bool,
    decided_at: &str,
) -> Result<()> {
    let row = record(b.state, "intercept_pending", id)?;
    let mut data = projected(b, row).clone();
    if !data["audit"].is_object() {
        data["audit"] = json!({});
    }
    data["audit"]["decision_origin"] = json!("user");
    data["audit"]["decision_epoch"] = json!(c.actor.engine_epoch);
    data["decided_at"] = json!(decided_at);
    b.put("intercept_pending", id, data.clone());
    if !allowed {
        return Ok(());
    }
    let Some(rid) = data["audit"]["run_id"].as_str() else {
        return Ok(());
    };
    for q in b.state.records("user_questions") {
        if q.data["state"] != "pending"
            || q.data["kind"] == "plan_approval"
            || q.data["payload"]["type"] != "tool_approval"
            || q.data["run_id"] != rid
            || q.data["payload"]["tool_use_id"] != data["audit"]["tool_use_id"]
            || q.data["payload"]["name"] != data["tool_name"]
            || q.data["payload"]["input"] != data["tool_input"]
        {
            continue;
        }
        if q.data["deadline_at"]
            .as_str()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .is_none_or(|t| t.with_timezone(&Utc) <= Utc::now())
        {
            continue;
        }
        let mut answered = q.data.clone();
        answered["state"] = json!("answered");
        answered["answer"] =
            json!({"approved":true,"decision_origin":"user_intercept","approval_id":id});
        b.put("user_questions", &q.header.id, answered);
    }
    Ok(())
}
pub fn annotate(
    mut batch: CommandBatch,
    initial: &str,
    initial_reason: Option<&str>,
    skipped_review: Option<&str>,
) -> CommandBatch {
    for change in &mut batch.changes {
        if let Change::Put {
            collection, data, ..
        } = change
        {
            if collection == "intercept_pending" {
                data["audit"]["approval_bypass"] = json!(true);
                data["audit"]["initial_action"] = json!(initial);
                if let Some(review) = skipped_review {
                    data["audit"]["skipped_review"] = json!(review);
                }
                if let Some(reason) = initial_reason {
                    data["audit"]["initial_reason"] = json!(reason);
                }
                data["audit"]["decision_reason"] =
                    json!("Workspace approval bypass skipped internal review");
            }
        }
    }
    batch
}
