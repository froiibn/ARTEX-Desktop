//! Persisted intercept decisions. Controller execution consumes these records;
//! human and timeout decisions race through the same versioned approval object.
use crate::*;
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use sha2::{Digest, Sha256};
pub const DEFAULT_TOOLS: &[&str] = &[
    "Bash",
    "WebFetch",
    "web_search",
    "shell_open",
    "shell_send",
    "Write",
    "Edit",
    "MultiEdit",
];
pub fn enabled_tools(s: &Snapshot) -> Vec<String> {
    if let Some(r) = s
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "intercept_enabled_tools")
    {
        serde_json::from_str(text(&r.data, "value")).unwrap_or_default()
    } else {
        DEFAULT_TOOLS.iter().map(|s| s.to_string()).collect()
    }
}
fn setting(s: &Snapshot, key: &str) -> String {
    configuration_ui::setting(s, key)
}
fn choice(s: &Snapshot, key: &str, default: &str, allowed: &[&str]) -> String {
    let val = setting(s, key);
    if allowed.contains(&val.as_str()) {
        val
    } else {
        default.into()
    }
}
pub fn judge_config(s: &Snapshot) -> Value {
    let defaults: Value = serde_json::from_str(include_str!("policy-prompts.json")).unwrap();
    let mut prompt = setting(s, "llm_judge_prompt");
    if prompt.trim().is_empty() {
        prompt = text(&defaults, "DefaultJudgePrompt").into();
    }
    let positive = |key: &str, default: i64| {
        setting(s, key)
            .parse::<i64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(default)
    };
    let profile = setting(s, "llm_judge_profile_id");
    json!({"enabled":setting_bool(s,"llm_judge_enabled",false),"profile_id":if profile.is_empty(){Value::Null}else{json!(profile)},"prompt":prompt,"timeout_seconds":positive("llm_judge_timeout_seconds",15),"fail_action":choice(s,"llm_judge_fail_action","allow",&["allow","deny","ask"]),"ask_timeout_seconds":positive("llm_judge_ask_timeout_seconds",300),"ask_timeout_action":choice(s,"llm_judge_ask_timeout_action","deny",&["allow","deny"])})
}
pub fn effective_judge_prompt(prompt: &str) -> String {
    let defaults: Value = serde_json::from_str(include_str!("policy-prompts.json")).unwrap();
    let mut out = prompt.to_owned();
    for key in ["JudgeContextBoundary", "JudgeOutputContract"] {
        let section = text(&defaults, key);
        if !out.contains(section) {
            out.push_str("\n\n");
            out.push_str(section);
        }
    }
    out
}
pub fn validate_rule(p: &Value) -> Result<()> {
    required(p, "name")?;
    let target = required(p, "match_target")?;
    if !["tool_name", "tool_input"].contains(&target) {
        return Err(Error::new("INVALID_PARAMS", "Invalid rule match target"));
    }
    let typ = required(p, "match_type")?;
    if !["string", "regex"].contains(&typ) {
        return Err(Error::new("INVALID_PARAMS", "Invalid rule match type"));
    }
    let pattern = required(p, "pattern")?;
    if typ == "regex" {
        Regex::new(pattern).map_err(|e| Error::new("INVALID_PARAMS", &e.to_string()))?;
    }
    if !["allow", "deny", "ask"].contains(&required(p, "action")?) {
        return Err(Error::new("INVALID_PARAMS", "Invalid rule action"));
    }
    Ok(())
}
fn prior<'a>(s: &'a Snapshot, rid: &str, toolid: &str) -> Option<&'a Record> {
    s.records("intercept_pending")
        .into_iter()
        .find(|r| r.data["audit"]["run_id"] == rid && r.data["audit"]["tool_use_id"] == toolid)
}
pub fn saved_decision(r: &Record) -> Value {
    let action = if r.data["status"] == "pending" {
        "ask"
    } else if r.data["status"] == "allowed" || r.data["audit"]["effective_action"] == "allow" {
        "allow"
    } else {
        "deny"
    };
    json!({"decision":action,"pending_id":r.header.id,"reason":r.data["reason"],"judge_required":false})
}
pub fn decision_plan(
    s: &Snapshot,
    c: &Context,
    rid: &str,
    toolid: &str,
    name: &str,
    input: &Value,
) -> Result<CommandBatch> {
    let run = record(s, "agent_runs", rid)?;
    if let Some(r) = prior(s, rid, toolid) {
        if r.data["tool_name"] != name || r.data["tool_input"] != *input {
            return Err(Error::new(
                "COMMAND_ID_REUSE",
                "Intercept input changed for existing tool-use ID",
            ));
        }
        if r.data["status"] == "pending" && crate::approval_bypass::enabled(s) {
            let mut b = Planner::new(
                s,
                "policy.bypass",
                &json!({"run_id":rid,"tool_use_id":toolid}),
            );
            crate::approval_bypass::settle_pending(&mut b, c)?;
            let mut row = r.clone();
            row.data = crate::approval_bypass::projected(&b, r).clone();
            return Ok(b.done(saved_decision(&row)));
        }
        return Ok(Planner::new(
            s,
            "policy.decision",
            &json!({"run_id":rid,"tool_use_id":toolid}),
        )
        .done(saved_decision(r)));
    }
    if run.data["state"] != "running" {
        return Err(Error::new("BUSY", "New tool review requires a running run"));
    }
    if !enabled_tools(s).iter().any(|n| n == name) {
        return Ok(Planner::new(
            s,
            "policy.decision",
            &json!({"run_id":rid,"tool_use_id":toolid}),
        )
        .done(json!({"decision":"allow","pending_id":null,"reason":"","judge_required":false})));
    }
    let mut rules = s
        .records("intercept_rules")
        .into_iter()
        .filter(|r| r.data["enabled"] == true)
        .collect::<Vec<_>>();
    rules.sort_by(|a, b| {
        b.data["priority"]
            .as_i64()
            .cmp(&a.data["priority"].as_i64())
            .then_with(|| a.header.created_at.cmp(&b.header.created_at))
            .then_with(|| a.header.id.cmp(&b.header.id))
    });
    let serialized = input.to_string();
    for rule in rules {
        let subject = match text(&rule.data, "match_target") {
            "tool_name" => name,
            "tool_input" => &serialized,
            _ => continue,
        };
        let pattern = text(&rule.data, "pattern");
        let matches = if rule.data["match_type"] == "regex" {
            Regex::new(pattern).is_ok_and(|re| re.is_match(subject))
        } else {
            subject.contains(pattern)
        };
        if matches {
            let action = text(&rule.data, "action");
            let reason = if text(&rule.data, "message").is_empty() {
                match action {
                    "deny" => format!(
                        "Intercept rule [{}] prohibits this tool",
                        text(&rule.data, "name")
                    ),
                    "ask" => format!(
                        "Intercept rule [{}] requires user approval",
                        text(&rule.data, "name")
                    ),
                    _ => String::new(),
                }
            } else {
                text(&rule.data, "message").into()
            };
            if action == "ask" && crate::approval_bypass::enabled(s) {
                return Ok(crate::approval_bypass::annotate(
                    save_decision(
                        s,
                        c,
                        rid,
                        toolid,
                        name,
                        input,
                        "allow",
                        &format!("Workspace approval bypass skipped intercept approval: {reason}"),
                        Some(rule),
                        false,
                        None,
                    )?,
                    "ask",
                    Some(&reason),
                    None,
                ));
            }
            return save_decision(
                s,
                c,
                rid,
                toolid,
                name,
                input,
                action,
                &reason,
                Some(rule),
                false,
                None,
            );
        }
    }
    let config = judge_config(s);
    if crate::approval_bypass::enabled(s) {
        return Ok(crate::approval_bypass::annotate(
            save_decision(
                s,
                c,
                rid,
                toolid,
                name,
                input,
                "allow",
                "Workspace approval bypass skipped model review",
                None,
                false,
                None,
            )?,
            "allow",
            None,
            (config["enabled"] == true).then_some("model"),
        ));
    }
    let mut b = Planner::new(
        s,
        "policy.decision",
        &json!({"run_id":rid,"tool_use_id":toolid}),
    );
    for r in s.records("settings") {
        if text(&r.data, "key").starts_with("llm_judge_")
            || r.data["key"] == "intercept_enabled_tools"
        {
            b.expect(r);
        }
    }
    let enabled = config["enabled"] == true;
    Ok(b.done(json!({"decision":"allow","pending_id":null,"reason":"","judge_required":enabled,"judge":if enabled{config}else{Value::Null}})))
}
pub fn judge_decision_plan(
    s: &Snapshot,
    c: &Context,
    rid: &str,
    toolid: &str,
    name: &str,
    input: &Value,
    decision: &str,
    reason: &str,
    model_input: &Value,
) -> Result<CommandBatch> {
    if let Some(r) = prior(s, rid, toolid) {
        return Ok(Planner::new(
            s,
            "policy.judge",
            &json!({"run_id":rid,"tool_use_id":toolid}),
        )
        .done(saved_decision(r)));
    }
    let cfg = model_input
        .get("captured_config")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| judge_config(s));
    let action = if ["allow", "deny", "ask"].contains(&decision) {
        decision
    } else {
        text(&cfg, "fail_action")
    };
    save_decision(
        s,
        c,
        rid,
        toolid,
        name,
        input,
        action,
        &format!("[model] {reason}"),
        None,
        true,
        Some(model_input),
    )
}
fn save_decision(
    s: &Snapshot,
    c: &Context,
    rid: &str,
    toolid: &str,
    name: &str,
    input: &Value,
    action: &str,
    reason: &str,
    rule: Option<&Record>,
    model: bool,
    model_input: Option<&Value>,
) -> Result<CommandBatch> {
    let run = record(s, "agent_runs", rid)?;
    let agent = record(s, "agents", required(&run.data, "agent_id")?)?;
    let cfg = model_input
        .and_then(|i| i.get("captured_config"))
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| judge_config(s));
    let timeout = if model {
        cfg["ask_timeout_seconds"].as_i64().unwrap_or(300)
    } else {
        rule.map(|r| r.data["timeout_seconds"].as_i64().unwrap_or(60))
            .unwrap_or(60)
            .max(1)
    };
    let timeout_enabled = model || rule.is_some_and(|r| r.data["timeout_enabled"] == true);
    let timeout_action = if model {
        text(&cfg, "ask_timeout_action")
    } else {
        rule.map(|r| text(&r.data, "timeout_action"))
            .unwrap_or("deny")
    };
    let stamp = artex_core::now();
    let audit = json!({"run_id":rid,"tool_use_id":toolid,"correlation":"exact","input_digest":format!("{:x}",Sha256::digest(serialized(input).as_bytes())),"user_message":"","context":[],"captured_at":stamp,"model_fallback":model,"model_input":model_input,"initial_action":action,"initial_reason":reason,"effective_action":if action=="ask"{""}else{action},"rule_name":rule.map(|r|text(&r.data,"name")).unwrap_or(""),"profile_id":cfg["profile_id"],"execution_status":if action=="allow"{"awaiting_result"}else if action=="deny"{"not_executed"}else{"not_started"},"timeout_enabled":timeout_enabled,"timeout_action":timeout_action,"deadline_at":if timeout_enabled{json!((Utc::now()+Duration::seconds(timeout)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true))}else{Value::Null}});
    let mut b = Planner::new(
        s,
        "policy.decision",
        &json!({"run_id":rid,"tool_use_id":toolid,"tool":name,"input":input,"action":action}),
    );
    b.expect(run);
    if let Some(rule) = rule {
        b.expect(rule);
    }
    let id=b.insert("intercept_pending","approval",json!({"rule_id":rule.map(|r|r.header.id.clone()),"conversation_id":run.data["conversation_id"],"task_id":run.data["task_id"],"agent_name":agent.data["key"],"tool_name":name,"tool_input":input,"status":if action=="ask"{"pending"}else if action=="allow"{"allowed"}else{"denied"},"reason":reason,"decided_at":if action=="ask"{Value::Null}else{json!(stamp)},"decision_source":if model{"model"}else{"rule"},"audit":audit}))?;
    if action == "ask" {
        append_request(&mut b, s, run, name, toolid, &id, input)?;
    }
    conversation::push_event(
        &mut b.batch,
        s,
        c,
        "InterceptDecision",
        "intercept_pending",
        &id,
    )?;
    let mut batch =
        b.done(json!({"decision":action,"pending_id":id,"reason":reason,"judge_required":false}));
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
fn serialized(input: &Value) -> String {
    input.to_string()
}
fn append_request(
    b: &mut Planner,
    s: &Snapshot,
    run: &Record,
    name: &str,
    toolid: &str,
    id: &str,
    input: &Value,
) -> Result<()> {
    let detail = artex_core::body(
        &json!({"run_id":run.header.id,"pending_id":id,"tool":name,"input":input}).to_string(),
    );
    let patch = json!({"worker":s.get("agents",text(&run.data,"agent_id")).map(|a|a.data["key"].clone()).unwrap_or(json!("")),"kind":"intercept_request","tool":name,"tool_use_id":toolid,"summary":format!("Tool {name} requests approval ({id})"),"detail":detail});
    if let Some(conv) = run.data["conversation_id"].as_str() {
        let mut d = patch;
        d["conversation_id"] = json!(conv);
        b.insert("conversation_activities", "conversationactivity", d)?;
    } else if let Some(task) = run.data["task_id"].as_str() {
        let mut d = patch;
        d["exploration_id"] = record(s, "tasks", task)?.data["exploration_id"].clone();
        d["node_id"] = run.data["intent_id"].clone();
        b.insert("activity", "activity", d)?;
    }
    Ok(())
}
pub fn settle(b: &mut Planner, id: &str, status: &str, action: &str, reason: &str) -> Result<()> {
    let mut d = record(b.state, "intercept_pending", id)?.data.clone();
    if d["status"] != "pending" {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Approval is already decided",
        ));
    }
    d["status"] = json!(status);
    d["decided_at"] = json!(artex_core::now());
    let audit = &mut d["audit"];
    if audit.is_null() {
        *audit = json!({});
    }
    audit["effective_action"] = json!(action);
    audit["decision_reason"] = json!(reason);
    if action == "deny" {
        audit["execution_status"] = json!("not_executed");
    } else {
        audit["execution_status"] = json!("awaiting_result");
    }
    b.put("intercept_pending", id, d);
    Ok(())
}
pub fn expire_pending_plan(s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, "policy.expire", &json!({}));
    let mut count = 0;
    for r in s.records("intercept_pending") {
        if r.data["status"] != "pending" {
            continue;
        }
        let audit = &r.data["audit"];
        let cancelled = audit["run_id"]
            .as_str()
            .and_then(|id| s.get("agent_runs", id))
            .is_some_and(|run| {
                matches!(
                    text(&run.data, "state"),
                    "completed" | "failed" | "cancelled" | "interrupted" | "recovery_required"
                )
            });
        let expired = audit["timeout_enabled"] == true
            && DateTime::parse_from_rfc3339(text(audit, "deadline_at"))
                .is_ok_and(|t| t <= Utc::now());
        if cancelled || expired {
            let action = if !cancelled && audit["timeout_action"] == "allow" {
                "allow"
            } else {
                "deny"
            };
            settle(
                &mut b,
                &r.header.id,
                if cancelled { "denied" } else { "timeout" },
                action,
                if cancelled {
                    "Work canceled"
                } else {
                    "Approval timeout; configured policy applied"
                },
            )?;
            conversation::push_event(
                &mut b.batch,
                s,
                c,
                "InterceptDecision",
                "intercept_pending",
                &r.header.id,
            )?;
            count += 1;
        }
    }
    let mut out = b.done(json!({"resolved":count}));
    augment_expected(&mut out, s)?;
    Ok(out)
}
