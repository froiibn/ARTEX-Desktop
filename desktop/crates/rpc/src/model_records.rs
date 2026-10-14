//! Provider attempts and usage remain attributable even when a profile fails.
use super::*;
use artex_domain::{Planner, record, text};
use artex_runtime::{ToolCall, Turn};
impl Router {
    pub(super) fn record_model_turn(
        &mut self,
        rid: &str,
        metadata: &Value,
        result: &Result<Turn>,
    ) -> Result<()> {
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        let worker = s
            .get("agents", text(&run.data, "agent_id"))
            .map(|a| a.data["key"].clone())
            .unwrap_or(Value::Null);
        let status = if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let usage = match result {
            Ok(turn) => turn.usage.clone(),
            Err(error) => error.details.get("usage").cloned().unwrap_or(json!({})),
        };
        let mut data = json!({"ts":artex_core::now(),"session_id":rid,"task_id":run.data["task_id"],"worker":worker,
            "model":metadata["model"],"profile_name":metadata["profile_name"],"latency_ms":metadata["latency_ms"],"status":status});
        for (dst, src) in [
            ("input_tokens", "input_tokens"),
            ("output_tokens", "output_tokens"),
            ("cache_read", "cache_read_tokens"),
            ("cache_write", "cache_write_tokens"),
        ] {
            data[dst] = json!(usage[src].as_i64().unwrap_or(0).clamp(0, i32::MAX as i64));
        }
        let mut p = Planner::new(
            &s,
            "run.llm.record",
            &json!({"run_id":rid,"profile_id":metadata["profile_id"]}),
        );
        let mut record = data.clone();
        record["request_body"] = artex_core::body(&metadata["request_body"].to_string());
        record["raw_request"] = record["request_body"].clone();
        match result {
            Ok(turn) => {
                record["response_body"] = artex_core::body(&turn.raw_response.to_string());
                record["raw_response"] = record["response_body"].clone();
            }
            Err(e) => record["error"] = json!(format!("{}: {}", e.code, e.message)),
        }
        if metadata["record_enabled"] == true {
            p.insert("llm_records", "llmrecord", record)?;
        }
        data.as_object_mut().unwrap().remove("session_id");
        data["exploration_id"] = run.data["task_id"]
            .as_str()
            .and_then(|id| s.get("tasks", id))
            .map(|t| t.data["exploration_id"].clone())
            .unwrap_or(Value::Null);
        p.insert("llm_usage", "llmusage", data)?;
        self.internal_commit(p.done(json!({"recorded":true})))?;
        Ok(())
    }
    pub(super) fn record_tool_calls(&mut self, rid: &str, calls: &[ToolCall]) -> Result<()> {
        if calls.is_empty() {
            return Ok(());
        }
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        let agent = record(&s, "agents", text(&run.data, "agent_id"))?;
        let mut p = Planner::new(&s, "run.tool.calls", &json!({"run_id":rid}));
        for call in calls {
            let mut data = json!({"worker":agent.data["key"],"kind":"tool_use","tool":call.name,"tool_use_id":call.id,"summary":call.name,"detail":artex_core::body(&call.input.to_string()),"is_error":false});
            if let Some(conv) = run.data["conversation_id"].as_str() {
                data["conversation_id"] = json!(conv);
                p.insert("conversation_activities", "conversationactivity", data)?;
            } else {
                data["exploration_id"] =
                    record(&s, "tasks", text(&run.data, "task_id"))?.data["exploration_id"].clone();
                data["node_id"] = run.data["intent_id"].clone();
                data["metadata"] = json!({"run_id":rid});
                p.insert("activity", "activity", data)?;
            }
        }
        self.internal_commit(p.done(json!({"recorded":true})))?;
        Ok(())
    }
}
