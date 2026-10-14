//! Summary generation is cancellable; only the writer publishes verified archives.
use super::*;
use artex_runtime::Turn;
use std::time::Duration;
impl Router {
    pub(super) fn start_compaction(
        &mut self,
        rid: &str,
        profile: &artex_core::Record,
        candidate: compression::AutomaticCompaction,
    ) -> Result<()> {
        let s = self.snapshot()?;
        let context = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        let gateway = self.gateway.clone().ok_or_else(|| {
            Error::new(
                "FEATURE_DISABLED",
                "Native host is required for summary credentials",
            )
        })?;
        let mut configured = profile.data.clone();
        configured["thinking_type"] = json!("disabled");
        configured["max_tokens"] = json!(candidate.max_output_tokens);
        let mut metadata = json!({"profile_id":profile.header.id,"profile_name":configured["name"],"model":configured["model"],"record_enabled":artex_domain::setting_bool(&s,"llm_record",false),"purpose":"context_compaction","request_body":artex_runtime::provider::encode(&configured,&candidate.prompt)?.1});
        let run = s
            .get("agent_runs", rid)
            .ok_or_else(|| Error::new("NOT_FOUND", "Summary run no longer exists"))?;
        let timeout = run.data["deadline_at"]
            .as_str()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
            .map(|v| {
                (v.with_timezone(&chrono::Utc) - chrono::Utc::now())
                    .num_milliseconds()
                    .max(1) as u64
            })
            .unwrap_or(120000)
            .min(120000);
        let state =
            self.runtime.runs.get_mut(rid).ok_or_else(|| {
                Error::new("STALE_EPOCH", "Summary controller is no longer active")
            })?;
        state.in_flight = true;
        let cancel = state.cancellation.clone();
        let tx = self.runtime.tx.clone();
        let workspace = s.workspace_id.clone();
        let run = rid.to_owned();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = (|| {
                let secret = gateway.request(
                    &workspace,
                    &context,
                    "secret.resolve",
                    json!({"secret_ref":configured["api_key"]["secret_ref"]}),
                    Duration::from_secs(10).min(Duration::from_millis(timeout)),
                )?;
                cancel.check()?;
                artex_runtime::provider::generate(
                    &configured,
                    secret["value"].as_str().ok_or_else(|| {
                        Error::new("SECRET_UNAVAILABLE", "Summary credential is unavailable")
                    })?,
                    &candidate.prompt,
                    &cancel,
                    Duration::from_millis(timeout).saturating_sub(started.elapsed()),
                )
            })();
            metadata["latency_ms"] =
                json!(started.elapsed().as_millis().min(i32::MAX as u128) as u64);
            let _ = tx.send(controller::Completion::Compaction {
                run,
                candidate,
                metadata,
                result,
            });
        });
        Ok(())
    }
    pub(super) fn complete_compaction(
        &mut self,
        rid: &str,
        candidate: &compression::AutomaticCompaction,
        metadata: &Value,
        result: Result<Turn>,
    ) -> Result<()> {
        if let Some(state) = self.runtime.runs.get_mut(rid) {
            state.in_flight = false;
        }
        self.record_model_turn(rid, metadata, &result)?;
        let turn = result?;
        if !turn.tools.is_empty() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Summary provider returned tools despite a tool-free request",
            ));
        }
        let s = self.snapshot()?;
        let context = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        let plan = self.automatic_compaction_plan(rid, candidate, &turn.text, &context)?;
        self.internal_commit(plan)?;
        Ok(())
    }
}
