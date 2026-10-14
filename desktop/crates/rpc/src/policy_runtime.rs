//! The writer owns intercept state; model review is a cancellable read-only worker.
use super::*;
use artex_domain::{Planner, record, text};
use artex_runtime::{Message, Prompt, ToolCall, permission::Decision};
use chrono::Utc;
use std::time::Duration;

impl Router {
    pub(super) fn intercept_tool(&mut self, rid: &str, call: &ToolCall) -> Result<Decision> {
        let s = self.snapshot()?;
        let c = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        let batch =
            artex_domain::policy::decision_plan(&s, &c, rid, &call.id, &call.name, &call.input)?;
        let outcome = if batch.changes.is_empty() {
            batch.result
        } else {
            self.internal_commit(batch)?
        };
        if outcome["judge_required"] == true {
            if let Err(e) = self.start_judge(rid, call, &outcome["judge"]) {
                self.complete_judge(rid,call,&json!({"version":"1","tool_name":call.name,"arguments":call.input,"captured_config":outcome["judge"]}),text(&outcome["judge"],"fail_action"),&format!("Judge failed: {}",e.code))?;
                return self.intercept_tool(rid, call);
            }
            return Ok(Decision::Ask);
        }
        let decision = match text(&outcome, "decision") {
            "deny" => Decision::Deny,
            "ask" => Decision::Ask,
            _ => Decision::Allow,
        };
        if decision == Decision::Ask {
            let fresh = self.snapshot()?;
            let run = record(&fresh, "agent_runs", rid)?;
            if run.data["state"] != "waiting" {
                let mut p = Planner::new(&fresh, "policy.wait", &json!({"run_id":rid}));
                let mut data = run.data.clone();
                data["state"] = json!("waiting");
                p.put("agent_runs", rid, data);
                self.internal_commit(p.done(json!({"waiting":true})))?;
            }
        }
        Ok(decision)
    }
    fn start_judge(&mut self, rid: &str, call: &ToolCall, config: &Value) -> Result<()> {
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        let agent = record(&s, "agents", text(&run.data, "agent_id"))?;
        let profile = if let Some(id) = config["profile_id"].as_str() {
            record(&s, "llm_profiles", id)?
        } else {
            artex_domain::conversation::effective_profile(
                &s,
                agent,
                run.data["conversation_id"]
                    .as_str()
                    .and_then(|id| s.get("conversations", id)),
                run.data["task_id"]
                    .as_str()
                    .and_then(|id| s.get("tasks", id)),
            )?
        };
        let profile_id = profile.header.id.clone();
        let mut profile = profile.data.clone();
        profile["streaming"] = json!(false);
        profile["max_tokens"] = json!(2048);
        let mut background = String::new();
        for m in self
            .compressed_messages(rid)?
            .iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            for b in &m.content {
                if b["type"] == "text" {
                    background.push_str(b["text"].as_str().unwrap_or(""));
                    background.push('\n');
                }
            }
        }
        let truncated = background.len() > 32 * 1024;
        if truncated {
            let mut end = 32 * 1024;
            while !background.is_char_boundary(end) {
                end -= 1;
            }
            background.truncate(end);
        }
        let mut input = json!({"version":"1","tool_name":call.name,"arguments":call.input,"correlation":"exact","background":{"source":if run.data["conversation_id"].is_null(){"worker_summary"}else{"user_message"},"text":background,"truncated":truncated},"captured_config":config});
        input["working_directory"] = json!(if let Some(task) = run.data["task_id"].as_str() {
            format!("work/tasks/{task}")
        } else {
            format!("work/conversations/{}", text(&run.data, "conversation_id"))
        });
        if let Some(intent) = run.data["intent_id"]
            .as_str()
            .and_then(|id| s.get("exploration_nodes", id))
        {
            input["worker_intent"] = json!(intent.data["payload"].to_string());
        }
        let mut model_input = input.clone();
        model_input
            .as_object_mut()
            .unwrap()
            .remove("captured_config");
        let prompt = Prompt {
            system: artex_domain::policy::effective_judge_prompt(text(config, "prompt")),
            messages: vec![Message {
                role: "user".into(),
                content: vec![json!({"type":"text","text":serde_json::to_string(&model_input)?})],
            }],
            tools: vec![],
            session_id: format!("{rid}-judge-{}", call.id),
        };
        let mut metadata = json!({"profile_id":profile_id,"profile_name":profile["name"],"model":profile["model"],"record_enabled":artex_domain::setting_bool(&s,"llm_record",false),"purpose":"intercept_judge","request_body":artex_runtime::provider::encode(&profile,&prompt)?.1});
        let gateway = self.gateway.clone().ok_or_else(|| {
            Error::new(
                "FEATURE_DISABLED",
                "Model judge requires native credential access",
            )
        })?;
        let context = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        let workspace = s.workspace_id.clone();
        let timeout = Duration::from_secs(
            config["timeout_seconds"]
                .as_u64()
                .unwrap_or(15)
                .clamp(1, 300),
        );
        let fail = text(config, "fail_action").to_owned();
        let call = call.clone();
        let rid = rid.to_owned();
        let state = self
            .runtime
            .runs
            .get_mut(&rid)
            .ok_or_else(|| Error::new("STALE_EPOCH", "Run controller is inactive"))?;
        state.in_flight = true;
        let cancel = artex_runtime::Cancellation::default();
        let review_id = artex_core::new_id("request");
        self.runtime
            .judge_workers
            .insert(rid.clone(), (review_id.clone(), cancel.clone()));
        let tx = self.runtime.tx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let model_result = (|| {
                let secret = gateway.request(
                    &workspace,
                    &context,
                    "secret.resolve",
                    json!({"secret_ref":profile["api_key"]["secret_ref"]}),
                    timeout,
                )?;
                cancel.check()?;
                artex_runtime::provider::generate(
                    &profile,
                    secret["value"].as_str().unwrap_or(""),
                    &prompt,
                    &cancel,
                    timeout.saturating_sub(started.elapsed()),
                )
            })();
            let result = model_result
                .as_ref()
                .map_err(|e| e.clone())
                .and_then(|turn| parse_judge(&turn.text));
            let (decision, reason) =
                result.unwrap_or_else(|e| (fail, format!("Judge failed: {}", e.code)));
            metadata["latency_ms"] =
                json!(started.elapsed().as_millis().min(i32::MAX as u128) as u64);
            let _ = tx.send(controller::Completion::Judge {
                review_id,
                run: rid,
                call,
                input,
                decision,
                reason,
                metadata,
                model_result,
            });
        });
        Ok(())
    }
    pub(super) fn complete_judge(
        &mut self,
        rid: &str,
        call: &ToolCall,
        input: &Value,
        decision: &str,
        reason: &str,
    ) -> Result<()> {
        if let Some(state) = self.runtime.runs.get_mut(rid) {
            state.in_flight = false;
        }
        let s = self.snapshot()?;
        let c = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        let plan = artex_domain::policy::judge_decision_plan(
            &s,
            &c,
            rid,
            &call.id,
            &call.name,
            &call.input,
            decision,
            reason,
            input,
        )?;
        self.internal_commit(plan)?;
        Ok(())
    }
    pub(super) fn maintain_intercepts(&mut self) -> Result<()> {
        let s = self.snapshot()?;
        let c = Context {
            actor: Actor {
                kind: "engine".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let plan = artex_domain::policy::expire_pending_plan(&s, &c)?;
        if !plan.changes.is_empty() {
            self.internal_commit(plan)?;
        }
        let s = self.snapshot()?;
        if artex_domain::approval_bypass::enabled(&s) {
            let mut b = Planner::new(&s, "policy.bypass", &json!({}));
            artex_domain::approval_bypass::settle_pending(&mut b, &c)?;
            if !b.batch.changes.is_empty() {
                self.internal_commit(b.done(json!({"settled":true})))?;
            }
        }
        let s = self.snapshot()?;
        let mut p = Planner::new(&s, "policy.resume", &json!({}));
        for (rid, state) in &self.runtime.runs {
            let Some(call) = state.pending.front() else {
                continue;
            };
            let Some(_run) = s
                .get("agent_runs", rid)
                .filter(|r| r.data["state"] == "waiting")
            else {
                continue;
            };
            let settled = s.records("intercept_pending").into_iter().any(|r| {
                r.data["audit"]["run_id"] == *rid
                    && r.data["audit"]["tool_use_id"] == call.id
                    && r.data["status"] != "pending"
            });
            if settled {
                artex_domain::approval_bypass::resume(&mut p, rid, &self.engine_epoch);
            }
        }
        if !p.batch.changes.is_empty() {
            self.internal_commit(p.done(json!({"resumed":true})))?;
        }
        Ok(())
    }
    pub(super) fn cancel_approval_judge(&mut self, rid: &str) {
        if let Some((id, cancel)) = self.runtime.judge_workers.remove(rid) {
            cancel.cancel();
            self.runtime.retired_judges.insert(id);
            if let Some(state) = self.runtime.runs.get_mut(rid) {
                state.in_flight = false;
            }
        }
    }
    pub(super) fn approval_committed(&mut self, request: &DispatchRequest) {
        if request.method == "ui.setSettings" && request.params["patch"]["approval_bypass"] == true
        {
            let runs = self
                .runtime
                .judge_workers
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for rid in runs {
                self.cancel_approval_judge(&rid);
                self.runtime.approval_wakes.insert(rid);
            }
            for (rid, state) in &self.runtime.runs {
                if !state.pending.is_empty() {
                    self.runtime.approval_wakes.insert(rid.clone());
                }
            }
        } else if request.method == "ui.interceptDecide" || request.method == "user.answer" {
            if let Ok(s) = self.snapshot() {
                let row = if request.method == "user.answer" {
                    s.get(
                        "user_questions",
                        request.params["question_id"].as_str().unwrap_or(""),
                    )
                } else {
                    s.get(
                        "intercept_pending",
                        request.params["id"].as_str().unwrap_or(""),
                    )
                };
                if let Some(rid) = row.and_then(|r| {
                    if request.method == "user.answer" {
                        r.data["run_id"].as_str()
                    } else {
                        r.data["audit"]["run_id"].as_str()
                    }
                }) {
                    self.runtime.approval_wakes.insert(rid.to_owned());
                }
            }
        }
    }
    pub(super) fn process_approval_wakes(&mut self) -> Result<()> {
        let owners = std::mem::take(&mut self.runtime.approval_wakes);
        if !owners.is_empty() {
            self.account_task_time()?;
        }
        for rid in owners {
            let s = self.snapshot()?;
            let Some(run) = s.get("agent_runs", &rid) else {
                continue;
            };
            if run.data["engine_epoch"] != self.engine_epoch || run.data["state"] != "running" {
                continue;
            }
            if run.data["deadline_at"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t.with_timezone(&Utc) <= Utc::now())
            {
                continue;
            }
            if s.get("agents", text(&run.data, "agent_id"))
                .is_none_or(|a| a.data["enabled"] != true)
            {
                continue;
            }
            if let Some(task) = run.data["task_id"]
                .as_str()
                .and_then(|id| s.get("tasks", id))
            {
                if self.deleting_tasks.contains(&task.header.id)
                    || task.data["paused"] == true
                    || !task.data["deleted_at"].is_null()
                    || !task.data["archived_at"].is_null()
                    || matches!(
                        text(&task.data, "status"),
                        "done" | "failed" | "timeout" | "paused"
                    )
                {
                    continue;
                }
            }
            let Some(state) = self.runtime.runs.get(&rid) else {
                continue;
            };
            if state.in_flight || state.cancellation.cancelled() {
                continue;
            }
            if let Some(call) = state.pending.front().cloned() {
                if let Err(error) = self.next_tool(&rid, call) {
                    self.finish_runtime(
                        &rid,
                        "failed",
                        &format!("{}: {}", error.code, error.message),
                    )?;
                }
            }
        }
        Ok(())
    }
}

fn parse_judge(raw: &str) -> Result<(String, String)> {
    // A model may not smuggle extra keys or an executable instruction into its verdict.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Verdict {
        decision: String,
        comment: String,
    }
    let raw = raw.trim();
    let fence = "\u{0060}\u{0060}\u{0060}";
    let raw = if let Some(inner) = raw.strip_prefix(fence).and_then(|v| v.strip_suffix(fence)) {
        let inner = inner.trim();
        inner
            .strip_prefix("json\n")
            .or_else(|| inner.strip_prefix("json\r\n"))
            .unwrap_or(inner)
            .trim()
    } else {
        raw
    };
    let verdict: Verdict = serde_json::from_str(raw).map_err(|_| {
        Error::new(
            "INVALID_PARAMS",
            "Judge must return a single JSON decision and comment",
        )
    })?;
    if !["allow", "deny", "ask"].contains(&verdict.decision.as_str())
        || verdict.comment.trim().is_empty()
        || verdict.comment.chars().count() > 2400
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Judge decision or comment is invalid",
        ));
    }
    Ok((verdict.decision, verdict.comment))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn judge_verdict_rejects_ambiguous_or_extra_fields() {
        assert_eq!(
            parse_judge(r#"{"decision":"ask","comment":"Need target confirmation"}"#)
                .unwrap()
                .0,
            "ask"
        );
        for bad in [
            r#"{"decision":"allow","decision":"deny","comment":"x"}"#,
            r#"{"decision":"allow","comment":"x","extra":true}"#,
            r#"{"decision":"approve","comment":"x"}"#,
            "allow",
        ] {
            assert!(parse_judge(bad).is_err());
        }
    }
}
