//! Native-only lifecycle plans; no undeclared v1 transport methods are exposed.
use crate::*;

pub fn targets(ids: &[String], prefix: &str) -> Result<Vec<String>> {
    if ids.is_empty() || ids.len() > 100 {
        return Err(Error::new("INVALID_PARAMS", "Select 1..100 objects"));
    }
    let mut seen = BTreeSet::new();
    for id in ids {
        if !artex_core::valid_id(id, prefix) || !seen.insert(id.clone()) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Distinct typed object IDs required",
            ));
        }
    }
    Ok(ids.to_vec())
}
pub fn conversation_plan(
    s: &Snapshot,
    c: &Context,
    ids: &[String],
    action: &str,
) -> Result<CommandBatch> {
    let ids = targets(ids, "conversation")?;
    if !matches!(action, "clear" | "trash" | "restore" | "delete") {
        return Err(Error::new("INVALID_PARAMS", "Unknown conversation action"));
    }
    let mut p = Planner::new(
        s,
        "native.lifecycle.conversation",
        &json!({"ids":ids,"action":action}),
    );
    let mut results = vec![];
    for id in &ids {
        let Some(row) = s.get("conversations", id) else {
            if ids.len() == 1 {
                return Err(Error::new("NOT_FOUND", "Conversation does not exist"));
            }
            results.push(json!({"id":id,"ok":false,"error":{"code":"NOT_FOUND","message":"Conversation does not exist"}}));
            continue;
        };
        p.expect(row);
        if action == "restore" && row.data["trashed_at"].is_null() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Conversation is not in trash",
            ));
        }
        if action == "clear" && !row.data["trashed_at"].is_null() {
            return Err(Error::new(
                "NOT_FOUND",
                "Restore the conversation before clearing history",
            ));
        }
        if action != "restore" {
            for run in s
                .records("agent_runs")
                .into_iter()
                .filter(|run| run.data["conversation_id"] == *id)
            {
                p.expect(run);
                conversation::cancel_run(&mut p, c, &run.header.id)?;
            }
        }
        if matches!(action, "clear" | "delete") {
            for activity in s
                .records("conversation_activities")
                .into_iter()
                .filter(|a| a.data["conversation_id"] == *id)
            {
                p.delete("conversation_activities", &activity.header.id);
            }
            let owned_runs = s
                .records("agent_runs")
                .into_iter()
                .filter(|run| run.data["conversation_id"] == *id)
                .map(|run| run.header.id.clone())
                .collect::<BTreeSet<_>>();
            for question in s.records("user_questions").into_iter().filter(|q| {
                q.data["run_id"]
                    .as_str()
                    .is_some_and(|run| owned_runs.contains(run))
            }) {
                p.batch
                    .changes
                    .retain(|ch| ch.key() != ("user_questions", question.header.id.as_str()));
                p.delete("user_questions", &question.header.id);
            }
            for session in s
                .records("side_question_sessions")
                .into_iter()
                .filter(|r| r.data["conversation_id"] == *id)
            {
                p.delete("side_question_sessions", &session.header.id);
            }
            // Tool/LLM audit records are retained. FK SET NULL on permanent
            // deletion preserves provenance without recreating the conversation.
        }
        if action == "delete" {
            p.delete("conversations", id);
        } else {
            let mut data = row.data.clone();
            data["trash_generation"] = json!(new_id("trash"));
            data["trashed_at"] = if action == "trash" {
                json!(artex_core::now())
            } else {
                Value::Null
            };
            p.put("conversations", id, data);
        }
        conversation::push_event(
            &mut p.batch,
            s,
            c,
            "ConversationLifecycleChanged",
            "conversations",
            id,
        )?;
        results.push(json!({"id":id,"ok":true,"deleted":action=="delete","cleared":action=="clear","trashed":action=="trash","restored":action=="restore"}));
    }
    let succeeded = results.iter().filter(|r| r["ok"] == true).count();
    let mut batch = p.done(json!({"accepted":true,"results":results,"succeeded":succeeded,"failed":ids.len()-succeeded,"secure_erase":false}));
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
pub fn stop_plan(s: &Snapshot, c: &Context, run_ids: &[String]) -> Result<CommandBatch> {
    let mut p = Planner::new(s, "native.lifecycle.stop", &json!({"run_ids":run_ids}));
    for id in run_ids {
        p.expect(record(s, "agent_runs", id)?);
        conversation::cancel_run(&mut p, c, id)?;
        if let Some(node) = record(s, "agent_runs", id)?.data["intent_id"]
            .as_str()
            .and_then(|id| s.get("exploration_nodes", id))
        {
            if matches!(
                text(&record(s, "agent_runs", id)?.data, "state"),
                "queued" | "running" | "waiting" | "paused"
            ) && matches!(
                text(&node.data, "state"),
                "open" | "running" | "paused" | "claimed" | "active"
            ) {
                let mut data = node.data.clone();
                data["state"] = json!("stopped");
                data["completed_at"] = json!(artex_core::now());
                p.put("exploration_nodes", &node.header.id, data);
            }
        }
    }
    let mut batch = p.done(json!({"accepted":true}));
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
