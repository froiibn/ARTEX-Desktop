//! Native lifecycle coordination. Accepted cancellation is never cleanup proof.
use crate::native::*;
use crate::*;
use artex_domain::{record, text};
use artex_runtime::{Cancellation, HostGateway};
use std::{sync::Arc, time::Duration};

#[derive(Default)]
pub(super) struct LifecycleState {
    pending: BTreeMap<String, PendingLifecycle>,
    frozen: HashSet<String>,
}
struct PendingLifecycle {
    batch: CommandBatch,
    runs: Vec<String>,
    quarantine: Vec<Value>,
    strict: bool,
    tasks: Vec<String>,
    committed: Option<Value>,
}
fn user_context(epoch: &str) -> Context {
    Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: epoch.into(),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    }
}
fn action_name(action: ConversationAction) -> &'static str {
    match action {
        ConversationAction::ClearHistory => "clear",
        ConversationAction::Trash => "trash",
        ConversationAction::Restore => "restore",
        ConversationAction::DeletePermanently => "delete",
    }
}
fn task_options(options: &TaskDeleteOptions) -> Value {
    json!({"delete_assets":options.delete_assets,"delete_traffic":options.delete_traffic,"delete_files":options.delete_files,"delete_findings":options.delete_findings,"delete_llm_records":options.delete_llm_records})
}
fn quarantine_operation(input: &Value, restore: bool) -> &'static str {
    match (input["conversation_id"].is_string(), restore) {
        (true, false) => "workspace.quarantine_conversation",
        (true, true) => "workspace.restore_conversation_quarantine",
        (false, false) => "workspace.quarantine",
        (false, true) => "workspace.restore_quarantine",
    }
}
fn proof(s: &Snapshot, run: &str) -> Option<Value> {
    let latest = s
        .records("command_receipts")
        .into_iter()
        .filter(|r| r.data["method"] == "native.lifecycle.cleanup")
        .filter_map(|r| {
            r.data["result"]["runs"]
                .as_array()
                .map(|rows| (r.header.commit_seq.parse::<u64>().unwrap_or(0), rows))
        })
        .flat_map(|(seq, rows)| {
            rows.iter()
                .filter(move |v| v["run_id"] == run)
                .map(move |v| (seq, v.clone()))
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, v)| v);
    let record = s.get("agent_runs", run)?;
    if latest.as_ref().is_some_and(|p| {
        p["cleanup_confirmed"] == true && p["engine_epoch"] == record.data["engine_epoch"]
    }) {
        return latest;
    }
    // A failed/completed native run can end before it admits any tool. The
    // durable finish receipt plus the retained invocation audit proves that
    // there was no owned external execution to clean up. A fresh host registry
    // alone is never evidence about an earlier engine lifetime.
    if record.header.legacy_key.is_null()
        && matches!(text(&record.data, "state"), "failed" | "completed")
        && record.data["finished_at"].is_string()
        && !s
            .records("tool_invocations")
            .iter()
            .any(|i| i.data["run_id"] == run)
    {
        let contradictory = s
            .records("command_receipts")
            .iter()
            .filter(|r| r.data["method"] == "native.lifecycle.cleanup")
            .flat_map(|r| r.data["result"]["runs"].as_array().into_iter().flatten())
            .any(|p| {
                p["run_id"] == run
                    && (p["executions"].as_array().is_some_and(|e| !e.is_empty())
                        || p["active_host_calls"].as_u64().is_some_and(|n| n > 0)
                        || p["run_requests_quiesced"] == false)
            });
        if !contradictory {
            if let Some(finish) = s.records("command_receipts").into_iter().find(|r| {
                r.header.legacy_key.is_null()
                    && r.data["method"] == "run.finish"
                    && r.data["normalized_params"]["run_id"] == run
                    && r.data["normalized_params"]["status"] == record.data["state"]
                    && r.data["result"]["state"] == record.data["state"]
                    && r.header.created_at.as_str()
                        >= record.data["finished_at"].as_str().unwrap_or("")
                    && r.header
                        .commit_seq
                        .parse::<u64>()
                        .ok()
                        .zip(record.header.commit_seq.parse::<u64>().ok())
                        .is_some_and(|(finished, current)| finished <= current)
            }) {
                return Some(
                    json!({"run_id":run,"engine_epoch":record.data["engine_epoch"],
                    "cleanup_confirmed":true,"cleanup_state":"confirmed","executions":[],
                    "proof_source":"durable_terminal_without_tool_admission",
                    "terminal_receipt_id":finish.header.id}),
                );
            }
        }
    }
    latest
}
pub(super) fn confirmed(s: &Snapshot, run: &artex_core::Record) -> bool {
    proof(s, &run.header.id).is_some_and(|v| {
        v["cleanup_confirmed"] == true && v["engine_epoch"] == run.data["engine_epoch"]
    })
}
fn combine(out: &mut CommandBatch, next: CommandBatch) {
    for change in next.changes {
        let key = change.key();
        out.changes.retain(|old| old.key() != key);
        out.changes.push(change);
    }
    out.events.extend(next.events);
    out.deliveries.extend(next.deliveries);
    out.expected.extend(next.expected);
}
fn original_expected(s: &Snapshot, batch: &mut CommandBatch) -> Result<()> {
    artex_domain::augment_expected(batch, s)?;
    batch.expected = batch
        .expected
        .iter()
        .filter_map(|e| {
            s.get(&e.collection, &e.id).map(|r| Expected {
                collection: e.collection.clone(),
                id: e.id.clone(),
                version: r.header.version.clone(),
            })
        })
        .collect();
    batch
        .expected
        .sort_by(|a, b| (&a.collection, &a.id).cmp(&(&b.collection, &b.id)));
    batch
        .expected
        .dedup_by(|a, b| a.collection == b.collection && a.id == b.id);
    let high = s
        .records("domain_events")
        .iter()
        .filter_map(|r| text(&r.data, "seq").parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    for (i, event) in batch.events.iter_mut().enumerate() {
        event.data["seq"] = json!(
            high.checked_add(i as u64 + 1)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Event sequence exhausted"))?
                .to_string()
        );
        if let (Some(collection), Some(id)) = (
            event.data["subject_collection"].as_str(),
            event.data["subject_id"].as_str(),
        ) {
            if let Some(row) = s.get(collection, id) {
                event.data["subject_version"] = json!(
                    artex_core::parse_u64(&row.header.version)?
                        .checked_add(1)
                        .ok_or_else(|| Error::new("SIZE_LIMIT", "Subject version exhausted"))?
                        .to_string()
                );
            }
        }
    }
    Ok(())
}
impl Router {
    pub(super) fn persist_native_retirement(&mut self) -> Result<()> {
        if !self.native_mode || self.effective_read_only() {
            return Ok(());
        }
        let Some(gateway) = self.gateway.clone() else {
            return Ok(());
        };
        let s = self.snapshot()?;
        let contexts = s
            .records("agent_runs")
            .into_iter()
            .filter(|run| {
                run.data["engine_epoch"] == self.engine_epoch
                    && run.data["agent_id"]
                        .as_str()
                        .is_some_and(|id| s.get("agents", id).is_some())
            })
            .map(|run| self.stored_run_context(&s, &run.header.id))
            .collect::<Result<Vec<_>>>()?;
        gateway.retire(Duration::from_secs(5))?;
        let Some(report) = gateway.retirement_report(&s.workspace_id, &contexts)? else {
            return Ok(());
        };
        let reported = report["runs"].as_array();
        let rows=contexts.iter().map(|c|{
            let rid=c.actor.run_id.as_deref().unwrap();
            let matches=reported.into_iter().flatten().filter(|v|v["run_id"]==rid&&v["engine_epoch"]==c.actor.engine_epoch).collect::<Vec<_>>();
            let value=matches.first().copied().filter(|_|matches.len()==1);
            let done=value.is_some_and(|v|v["cleanup_confirmed"]==true&&v["run_requests_quiesced"]==true&&v["active_host_calls"].as_u64()==Some(0));
            json!({"run_id":rid,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":done,"cleanup_state":if done{"confirmed"}else{"unknown"},"executions":value.map(|v|v["executions"].clone()).unwrap_or(json!([]))})
        }).collect::<Vec<_>>();
        let mut batch = artex_domain::empty_batch(
            "native.lifecycle.cleanup",
            &json!({"phase":"workspace_retired","engine_epoch":self.engine_epoch}),
        );
        batch.result = json!({"runs":rows,"confirmed_at":artex_core::now()});
        self.internal_commit(batch)?;
        Ok(())
    }
    pub(super) fn lifecycle_has_request(&self, id: &str) -> bool {
        self.lifecycle.pending.contains_key(id)
    }
    pub(super) fn lifecycle_committed(&self, id: &str) -> bool {
        self.lifecycle
            .pending
            .get(id)
            .is_some_and(|p| p.committed.is_some())
    }
    pub fn native_lifecycle_busy(&self) -> bool {
        !self.lifecycle.pending.is_empty()
    }
    pub(super) fn lifecycle_frozen(&self, rid: &str) -> bool {
        self.lifecycle.frozen.contains(rid)
    }
    fn lifecycle_plan(
        &self,
        target: &DeletionTarget,
        options: &DeletionOptions,
    ) -> Result<(CommandBatch, Vec<String>, bool, Vec<Value>)> {
        let s = self.snapshot()?;
        let c = user_context(&self.engine_epoch);
        let mut quarantine = vec![];
        let (mut batch, runs, strict) = match (target, options) {
            (DeletionTarget::Conversations { ids }, DeletionOptions::Conversations { action }) => {
                let batch = artex_domain::native_lifecycle::conversation_plan(
                    &s,
                    &c,
                    ids,
                    action_name(*action),
                )?;
                let successful = batch.result["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|v| v["ok"] == true)
                    .filter_map(|v| v["id"].as_str().map(str::to_owned))
                    .collect::<HashSet<_>>();
                let runs = s
                    .records("agent_runs")
                    .into_iter()
                    .filter(|r| {
                        r.data["conversation_id"]
                            .as_str()
                            .is_some_and(|id| successful.contains(id))
                    })
                    .map(|r| r.header.id.clone())
                    .collect::<Vec<_>>();
                if matches!(
                    action,
                    ConversationAction::ClearHistory | ConversationAction::DeletePermanently
                ) {
                    for id in ids.iter().filter(|id| successful.contains(*id)) {
                        quarantine.push(json!({"conversation_id":id,"trash_generation":new_id("trash"),"delete_contents":true}));
                    }
                }
                (
                    batch,
                    runs,
                    matches!(
                        action,
                        ConversationAction::ClearHistory | ConversationAction::DeletePermanently
                    ),
                )
            }
            (DeletionTarget::Tasks { ids }, DeletionOptions::Tasks { options }) => {
                let ids = artex_domain::native_lifecycle::targets(ids, "task")?;
                let mut combined = artex_domain::empty_batch(
                    "native.lifecycle.tasks",
                    &json!({"ids":ids,"options":task_options(options)}),
                );
                let mut projected = s.snapshot.as_ref().clone();
                let mut results = vec![];
                for id in &ids {
                    let params = json!({"id":id,"options":task_options(options)});
                    match artex_domain::task_delete_ui::plan("ui.deleteTask",&params,&projected,&c) {
                        Ok(next)=>{
                            let mut result=next.result.clone(); result["id"]=json!(id);result["ok"]=json!(true);
                            results.push(result);
                            projected=artex_domain::projected(&projected,&next)?;
                            combine(&mut combined,next);
                            if options.delete_files {quarantine.push(json!({"task_id":id}));}
                        },
                        Err(error)=>results.push(json!({"id":id,"ok":false,"error":{"code":error.code,"message":error.message}})),
                    }
                }
                let succeeded = results.iter().filter(|v| v["ok"] == true).count();
                combined.result = json!({"accepted":true,"results":results,"succeeded":succeeded,"failed":ids.len()-succeeded,"secure_erase":false});
                let successful = combined.result["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|v| v["ok"] == true)
                    .filter_map(|v| v["id"].as_str().map(str::to_owned))
                    .collect::<HashSet<_>>();
                let runs = s
                    .records("agent_runs")
                    .into_iter()
                    .filter(|r| {
                        r.data["task_id"]
                            .as_str()
                            .is_some_and(|id| successful.contains(id))
                    })
                    .map(|r| r.header.id.clone())
                    .collect::<Vec<_>>();
                (combined, runs, true)
            }
            _ => {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Deletion target and options do not match",
                ));
            }
        };
        original_expected(&s, &mut batch)?;
        Ok((batch, runs, strict, quarantine))
    }
    pub(super) fn lifecycle_preview(
        &self,
        target: DeletionTarget,
        options: DeletionOptions,
    ) -> Result<Value> {
        let s = self.snapshot()?;
        let (batch, runs, strict, quarantine) = self.lifecycle_plan(&target, &options)?;
        let collection = match target {
            DeletionTarget::Conversations { .. } => "conversations",
            DeletionTarget::Tasks { .. } => "tasks",
        };
        let ids = match &target {
            DeletionTarget::Conversations { ids } | DeletionTarget::Tasks { ids } => ids,
        };
        let mut counts = BTreeMap::<String, u64>::new();
        for change in artex_core::expand_deletions(&s, &batch.changes)? {
            if let artex_core::Change::Delete { collection, .. } = change {
                *counts.entry(collection).or_default() += 1;
            }
        }
        let requires = runs
            .iter()
            .filter(|rid| s.get("agent_runs", rid).is_some_and(|r| !confirmed(&s, r)))
            .cloned()
            .collect::<Vec<_>>();
        let old_unknown = requires.iter().any(|rid| {
            s.get("agent_runs", rid)
                .is_some_and(|r| r.data["engine_epoch"] != self.engine_epoch)
        });
        let can_apply = !self.effective_read_only()
            && (!strict
                || (!old_unknown
                    && (requires.is_empty() || self.gateway.is_some())
                    && (quarantine.is_empty() || self.gateway.is_some())));
        for collection in ["assets", "exchanges", "llm_records"] {
            if counts.get(collection).copied().unwrap_or(0) > s.records(collection).len() as u64 {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Preview deletion counts exceed the pinned collection",
                ));
            }
        }
        Ok(
            json!({"expected":batch.expected,"targets":ids.iter().filter_map(|id|s.get(collection,id).map(|r|json!({"id":id,"name":r.data["title"].as_str().or(r.data["name"].as_str()).unwrap_or(id)}))).collect::<Vec<_>>(),
            "affected":{"activities":counts.get("conversation_activities").copied().unwrap_or(0)+counts.get("activity").copied().unwrap_or(0),"runs":runs.len(),"questions":counts.get("user_questions").copied().unwrap_or(0),"assets":counts.get("assets").copied().unwrap_or(0),"traffic":counts.get("exchanges").copied().unwrap_or(0),"findings":counts.get("findings").copied().unwrap_or(0),"llm_records":counts.get("llm_records").copied().unwrap_or(0)},
            "owned_attachments":quarantine.iter().map(|v|{let (kind,id)=if let Some(id)=v["task_id"].as_str(){("tasks",id)}else{("conversations",v["conversation_id"].as_str().unwrap())};json!({"owner_id":id,"relative_path":format!("{kind}/{id}")})}).collect::<Vec<_>>(),
            "shared_preserved":{"assets":s.records("assets").len() as u64-counts.get("assets").copied().unwrap_or(0),"traffic":s.records("exchanges").len() as u64-counts.get("exchanges").copied().unwrap_or(0),"llm_records":s.records("llm_records").len() as u64-counts.get("llm_records").copied().unwrap_or(0),"tool_audit":s.records("tool_invocations").len()},
            "requires_stop":requires,"cleanup_confirmed":requires.is_empty(),"can_apply":can_apply,"blocked_reason":if self.effective_read_only(){Some("Workspace is read-only")}else if strict&&old_unknown{Some("An earlier engine lifetime has no durable cleanup confirmation")}else if !can_apply{Some("Native host cleanup is unavailable")}else{None},"secure_erase":false}),
        )
    }
    pub(super) fn lifecycle_trash(&self) -> Result<Value> {
        let s = self.snapshot()?;
        Ok(
            json!({"items":s.records("conversations").into_iter().filter(|r|!r.data["trashed_at"].is_null()).map(|r|json!({"id":r.header.id,"title":r.data["title"],"agent_key":r.data["agent_key"],"trashed_at":r.data["trashed_at"],"trash_generation":r.data["trash_generation"],"expected":[Expected{collection:"conversations".into(),id:r.header.id.clone(),version:r.header.version.clone()}]})).collect::<Vec<_>>()}),
        )
    }
    fn lifecycle_request(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        cid: String,
        expected: Vec<Expected>,
        method: &str,
        params: Value,
    ) -> Result<DispatchRequest> {
        self.check_native_scope(scope)?;
        self.ensure_writable()?;
        if !artex_core::valid_id(&cid, "command") {
            return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
        }
        let schema = self.contract["$defs"]["Expected"].clone();
        for e in &expected {
            self.validate("native:expected", &schema, &serde_json::to_value(e)?)?;
        }
        Ok(DispatchRequest {
            id: ticket.0.clone(),
            method: method.into(),
            workspace_id: Some(scope.workspace_id.clone()),
            engine_epoch: Some(scope.engine_epoch.clone()),
            command_id: Some(cid),
            expected,
            timeout_ms: 30000,
            params,
        })
    }
    fn lifecycle_admit(
        &mut self,
        r: &DispatchRequest,
        batch: CommandBatch,
        runs: Vec<String>,
        strict: bool,
        mut quarantine: Vec<Value>,
        force: bool,
    ) -> Result<Value> {
        if let Some(value) = self.repeated(r)? {
            return Ok(value);
        }
        if self.native_lifecycle_busy() {
            return Err(Error::new(
                "BUSY",
                "Another cleanup operation is in progress",
            ));
        }
        let s = self.snapshot()?;
        for e in &batch.expected {
            if !r.expected.contains(e) {
                let mut error = Error::new(
                    "VERSION_CONFLICT",
                    "Cleanup requires every preview dependency version",
                );
                error.details = json!({"required":e});
                return Err(error);
            }
        }
        for e in &r.expected {
            if s.get(&e.collection, &e.id)
                .is_none_or(|v| v.header.version != e.version)
            {
                return Err(Error::new("VERSION_CONFLICT", "Cleanup preview is stale"));
            }
        }
        let gateway = self.gateway.clone();
        let mut contexts = vec![];
        let mut prior = vec![];
        for rid in &runs {
            let run = record(&s, "agent_runs", rid)?;
            if confirmed(&s, run) {
                prior.push(proof(&s, rid).unwrap());
                continue;
            }
            if run.data["engine_epoch"] != self.engine_epoch {
                prior.push(json!({"run_id":rid,"engine_epoch":run.data["engine_epoch"],"cleanup_confirmed":false,"cleanup_state":"unknown","executions":[]}));
                if strict {
                    return Err(Error::new(
                        "RECOVERY_REQUIRED",
                        "An earlier engine lifetime has no durable cleanup proof",
                    ));
                }
            } else {
                contexts.push(self.stored_run_context(&s, rid)?);
            }
        }
        if strict && (!contexts.is_empty() || !quarantine.is_empty()) && gateway.is_none() {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "Native cleanup confirmation is required",
            ));
        }
        let cap = self.execution_capability.clone();
        for input in &mut quarantine {
            input["directory_capability"] = json!(cap);
            input["command_id"] = json!(r.command_id);
        }
        self.prepare_external_ui(r)?;
        let mut requested = artex_domain::empty_batch(
            "native.lifecycle.cleanup",
            &json!({"operation":r.command_id,"phase":"requested"}),
        );
        requested.result = json!({"runs":runs.iter().filter_map(|rid|s.get("agent_runs",rid).map(|run|json!({"run_id":rid,"engine_epoch":run.data["engine_epoch"],"cleanup_confirmed":false,"cleanup_state":"stopping","executions":[]}))).collect::<Vec<_>>(),"cleanup_state":"stopping","cleanup_confirmed":false});
        self.internal_commit(requested)?;
        for rid in &runs {
            self.lifecycle.frozen.insert(rid.clone());
            if let Some(state) = self.runtime.runs.get(rid) {
                state.cancellation.cancel();
            }
            self.cancel_approval_judge(rid);
        }
        let pending = PendingLifecycle {
            tasks: batch
                .changes
                .iter()
                .filter_map(|ch| match ch {
                    artex_core::Change::Delete { collection, id } if collection == "tasks" => {
                        Some(id.clone())
                    }
                    _ => None,
                })
                .collect(),
            committed: None,
            batch,
            runs: runs.clone(),
            strict,
            quarantine: quarantine.clone(),
        };
        let mut pending = pending;
        if let Some(task) = r.params["task_id"].as_str() {
            if !pending.tasks.iter().any(|v| v == task) {
                pending.tasks.push(task.into());
            }
        }
        self.deleting_tasks.extend(pending.tasks.iter().cloned());
        self.lifecycle.pending.insert(r.id.clone(), pending);
        let w = s.workspace_id.clone();
        let c = user_context(&self.engine_epoch);
        let end = Instant::now() + Duration::from_millis(r.timeout_ms);
        let result = self.defer_read_worker(r, move |cancel| {
            cleanup_worker(
                gateway, &w, &c, contexts, prior, quarantine, strict, force, cancel, end,
            )
        });
        if result.is_err() {
            self.lifecycle_release(&r.id);
        }
        result
    }
    pub fn native_run_control(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        cid: String,
        expected: Vec<Expected>,
        target: RunControlTarget,
        action: RunControlAction,
    ) -> Result<NativeDispatch> {
        let s = self.snapshot()?;
        let (runs, params) = match target {
            RunControlTarget::Run { run_id } => {
                record(&s, "agent_runs", &run_id)?;
                (
                    vec![run_id.clone()],
                    json!({"run_id":run_id,"force":action==RunControlAction::ForceStop}),
                )
            }
            RunControlTarget::Task { task_id } => {
                record(&s, "tasks", &task_id)?;
                (
                    s.records("agent_runs")
                        .into_iter()
                        .filter(|r| r.data["task_id"] == task_id)
                        .map(|r| r.header.id.clone())
                        .collect(),
                    json!({"task_id":task_id,"force":action==RunControlAction::ForceStop}),
                )
            }
        };
        let r = self.lifecycle_request(
            ticket,
            scope,
            cid,
            expected,
            "native.lifecycle.control",
            params,
        )?;
        let mut batch = artex_domain::native_lifecycle::stop_plan(
            &s,
            &user_context(&self.engine_epoch),
            &runs,
        )?;
        batch.result = json!({"accepted":true,"results":runs.iter().map(|id|json!({"id":id,"ok":true})).collect::<Vec<_>>(),"succeeded":runs.len(),"failed":0,"secure_erase":false});
        if let Some(id) = r.params["task_id"].as_str() {
            let row = record(&s, "tasks", id)?;
            let mut data = row.data.clone();
            data["paused"] = json!(true);
            data["queued"] = json!(false);
            if !matches!(text(&data, "status"), "done" | "failed" | "timeout") {
                data["status"] = json!("paused");
            }
            batch.changes.push(artex_core::Change::Put {
                collection: "tasks".into(),
                id: id.into(),
                data,
            });
            for runtime in s
                .records("task_runtime")
                .into_iter()
                .filter(|r| r.data["task_id"] == id)
            {
                let mut data = runtime.data.clone();
                data["last_active_clock"] = Value::Null;
                batch.changes.push(artex_core::Change::Put {
                    collection: "task_runtime".into(),
                    id: runtime.header.id.clone(),
                    data,
                });
            }
            original_expected(&s, &mut batch)?;
        }
        let value = self.lifecycle_admit(
            &r,
            batch,
            runs,
            false,
            vec![],
            action == RunControlAction::ForceStop,
        )?;
        Ok(if value["__deferred"] == true {
            NativeDispatch::Deferred
        } else {
            NativeDispatch::Complete(self.native_reply(&r.method, &r.params, value))
        })
    }
    pub fn native_conversation_manage(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        cid: String,
        expected: Vec<Expected>,
        ids: Vec<String>,
        action: ConversationAction,
    ) -> Result<NativeDispatch> {
        let r = self.lifecycle_request(
            ticket,
            scope,
            cid,
            expected,
            "native.lifecycle.conversation",
            json!({"ids":ids,"action":action_name(action)}),
        )?;
        if let Some(value) = self.repeated(&r)? {
            return Ok(NativeDispatch::Complete(
                self.native_reply(&r.method, &r.params, value),
            ));
        }
        let (batch, runs, strict, quarantine) = self.lifecycle_plan(
            &DeletionTarget::Conversations { ids },
            &DeletionOptions::Conversations { action },
        )?;
        let value = if action == ConversationAction::Restore {
            if let Some(v) = self.repeated(&r)? {
                v
            } else {
                let mut batch = batch;
                self.commit(&r, &mut batch)?
            }
        } else {
            self.lifecycle_admit(&r, batch, runs, strict, quarantine, false)?
        };
        Ok(if value["__deferred"] == true {
            NativeDispatch::Deferred
        } else {
            NativeDispatch::Complete(self.native_reply(&r.method, &r.params, value))
        })
    }
    pub fn native_task_delete(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        cid: String,
        expected: Vec<Expected>,
        ids: Vec<String>,
        options: TaskDeleteOptions,
    ) -> Result<NativeDispatch> {
        let r = self.lifecycle_request(
            ticket,
            scope,
            cid,
            expected,
            "native.lifecycle.tasks",
            json!({"ids":ids,"options":task_options(&options)}),
        )?;
        if let Some(value) = self.repeated(&r)? {
            return Ok(NativeDispatch::Complete(
                self.native_reply(&r.method, &r.params, value),
            ));
        }
        let (batch, runs, strict, quarantine) = self.lifecycle_plan(
            &DeletionTarget::Tasks { ids },
            &DeletionOptions::Tasks { options },
        )?;
        let value = self.lifecycle_admit(&r, batch, runs, strict, quarantine, false)?;
        Ok(if value["__deferred"] == true {
            NativeDispatch::Deferred
        } else {
            NativeDispatch::Complete(self.native_reply(&r.method, &r.params, value))
        })
    }
    fn lifecycle_release(&mut self, id: &str) {
        if let Some(p) = self.lifecycle.pending.remove(id) {
            for task in p.tasks {
                self.deleting_tasks.remove(&task);
            }
            for rid in p.runs {
                self.lifecycle.frozen.remove(&rid);
            }
        }
    }
    pub(super) fn complete_lifecycle(
        &mut self,
        r: &DispatchRequest,
        outcome: Result<Value>,
        cancelled: bool,
    ) -> Result<Value> {
        if r.method == "native.lifecycle.monitor" {
            return outcome;
        }
        let Some(mut pending) = self.lifecycle.pending.remove(&r.id) else {
            return Err(Error::new(
                "CANCELLED",
                "Cleanup operation is no longer pending",
            ));
        };
        let value = match outcome {
            Ok(v) => v,
            Err(error) => {
                for rid in &pending.runs {
                    self.lifecycle.frozen.remove(rid);
                }
                for task in &pending.tasks {
                    self.deleting_tasks.remove(task);
                    self.stop_task_after_failed_delete(task)?;
                }
                self.lifecycle_fail_stop(&pending.runs)?;
                return Err(error);
            }
        };
        if value["finalized"] == true {
            for rid in &pending.runs {
                self.lifecycle.frozen.remove(rid);
                self.finalize_stream(rid);
                if let Some(state) = self.runtime.runs.remove(rid) {
                    state.cancellation.cancel();
                }
            }
            for task in &pending.tasks {
                self.deleting_tasks.remove(task);
            }
            let mut result = pending.committed.take().ok_or_else(|| {
                Error::new(
                    "RECOVERY_REQUIRED",
                    "Committed cleanup result is unavailable",
                )
            })?;
            if r.method.starts_with("native.lifecycle.") {
                result["file_admission_released"] = json!(value["released"] == true);
            }
            return Ok(result);
        }
        if value["compensated"] == true {
            for task in &pending.tasks {
                self.deleting_tasks.remove(task);
                self.stop_task_after_failed_delete(task)?;
            }
            for rid in &pending.runs {
                self.lifecycle.frozen.remove(rid);
            }
            let original =
                serde_json::from_value::<Error>(pending.batch.result["compensation_error"].clone())
                    .unwrap_or_else(|_| {
                        Error::new(
                            "RECOVERY_REQUIRED",
                            "Cleanup compensation result is incomplete",
                        )
                    });
            self.lifecycle_fail_stop(&pending.runs)?;
            return Err(if value["restored"] == true {
                original
            } else {
                Error::new(
                    "RECOVERY_REQUIRED",
                    "Graph remains intact but file restoration is unconfirmed",
                )
            });
        }
        // Persist host proofs before graph changes. An unknown/failed proof remains
        // authoritative across reopening; a new empty registry cannot overwrite it.
        let mut receipt = artex_domain::empty_batch(
            "native.lifecycle.cleanup",
            &json!({"operation":r.command_id}),
        );
        receipt.result = json!({"runs":value["runs"],"cleanup_confirmed":value["cleanup_confirmed"],"confirmed_at":artex_core::now()});
        let proof_commit = self.internal_commit(receipt);
        let result = (|| {
            proof_commit?;
            if cancelled || value["cancelled"] == true {
                return Err(Error::new(
                    "CANCELLED",
                    "Cleanup cancelled before graph commit",
                ));
            }
            if pending.strict && value["cleanup_confirmed"] != true {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Native execution cleanup is unconfirmed; graph remains intact",
                ));
            }
            if r.method.starts_with("native.lifecycle.") {
                pending.batch.result["cleanup_confirmed"] = value["cleanup_confirmed"].clone();
                pending.batch.result["cleanup_state"] =
                    json!(if value["cleanup_confirmed"] == true {
                        "confirmed"
                    } else {
                        "unknown"
                    });
                pending.batch.result["accepted"] = json!(true);
                pending.batch.result["secure_erase"] = json!(false);
            }
            if r.method == "ui.deleteTask" {
                pending.batch.result["files_deleted"] = json!(
                    !pending.quarantine.is_empty()
                        && value["quarantined"]
                            .as_array()
                            .is_some_and(|v| v.iter().any(|item| item["files_present"] == true))
                );
            }
            if let Some(results) = pending
                .batch
                .result
                .get_mut("results")
                .and_then(Value::as_array_mut)
            {
                for item in results {
                    if item["ok"] == true {
                        item["files_deleted"] =
                            json!(value["quarantined"].as_array().is_some_and(|rows| {
                                rows.iter().any(|v| {
                                    v["files_present"] == true
                                        && (v["task_id"] == item["id"]
                                            || v["conversation_id"] == item["id"])
                                })
                            }));
                    }
                }
            }
            self.commit(r, &mut pending.batch)
        })();
        if result.is_ok()
            && pending
                .quarantine
                .iter()
                .any(|v| v["conversation_id"].is_string())
        {
            let gateway = self.gateway.clone();
            let w = self.snapshot()?.workspace_id.clone();
            let c = user_context(&self.engine_epoch);
            let inputs = pending.quarantine.clone();
            pending.committed = Some(result.unwrap());
            self.lifecycle.pending.insert(r.id.clone(), pending);
            return self.defer_read_worker(r, move |_| {
                let mut released = true;
                for input in inputs
                    .into_iter()
                    .filter(|v| v["conversation_id"].is_string())
                {
                    released &= gateway.as_ref().is_some_and(|g| {
                        g.request(
                            &w,
                            &c,
                            "workspace.complete_conversation_quarantine",
                            input,
                            Duration::from_secs(5),
                        )
                        .is_ok()
                    });
                }
                Ok(json!({"finalized":true,"released":released}))
            });
        }
        if result.is_err()
            && value["quarantined"]
                .as_array()
                .is_some_and(|a| !a.is_empty())
        {
            // Compensation must finish before admission resumes. It is another
            // cancellable worker, never a synchronous writer wait.
            let gateway = self.gateway.clone();
            let w = self.snapshot()?.workspace_id.clone();
            let c = user_context(&self.engine_epoch);
            let original = result.err().unwrap();
            let inputs = pending.quarantine.clone();
            pending.batch.result = json!({"compensation_error":original});
            self.lifecycle.pending.insert(r.id.clone(), pending);
            return self.defer_read_worker(r, move |_| {
                let mut restored = true;
                for input in inputs.iter().rev() {
                    restored &= gateway.as_ref().is_some_and(|g| {
                        g.request(
                            &w,
                            &c,
                            quarantine_operation(input, true),
                            input.clone(),
                            Duration::from_secs(15),
                        )
                        .is_ok()
                    });
                }
                Ok(json!({"compensated":true,"restored":restored}))
            });
        }
        for rid in &pending.runs {
            self.lifecycle.frozen.remove(rid);
            if result.is_ok() {
                self.finalize_stream(rid);
                if let Some(state) = self.runtime.runs.remove(rid) {
                    state.cancellation.cancel();
                }
            }
        }
        for task in &pending.tasks {
            self.deleting_tasks.remove(task);
            if result.is_err() {
                self.stop_task_after_failed_delete(task)?;
            }
        }
        if result.is_err() {
            self.lifecycle_fail_stop(&pending.runs)?;
        }
        result
    }
    fn lifecycle_fail_stop(&mut self, runs: &[String]) -> Result<()> {
        let s = self.snapshot()?;
        let ids = runs
            .iter()
            .filter(|id| s.get("agent_runs", id).is_some())
            .cloned()
            .collect::<Vec<_>>();
        let batch =
            artex_domain::native_lifecycle::stop_plan(&s, &user_context(&self.engine_epoch), &ids)?;
        self.internal_commit(batch)?;
        Ok(())
    }
    pub(super) fn handle_lifecycle_legacy(
        &mut self,
        r: &DispatchRequest,
        c: &Context,
    ) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Deletion requires the native user",
            ));
        }
        self.ensure_writable()?;
        self.require_command(r)?;
        if let Some(v) = self.repeated(r)? {
            return Ok(v);
        }
        let s = self.snapshot()?;
        let (mut batch, runs, quarantine) = if r.method == "ui.deleteTask" {
            let id = artex_domain::required(&r.params, "id")?;
            let batch = artex_domain::task_delete_ui::plan(&r.method, &r.params, &s, c)?;
            let runs = s
                .records("agent_runs")
                .into_iter()
                .filter(|v| v.data["task_id"] == id)
                .map(|v| v.header.id.clone())
                .collect();
            let files = if r.params["options"]["delete_files"] == true {
                vec![json!({"task_id":id})]
            } else {
                vec![]
            };
            (batch, runs, files)
        } else {
            let ids = if matches!(
                r.method.as_str(),
                "ui.deleteConversation" | "conversation.delete"
            ) {
                vec![artex_domain::required(&r.params, "id")?.into()]
            } else {
                artex_domain::ids(&r.params, "ids")
            };
            let mut batch = if r.method.starts_with("ui.") {
                facade::plan(&r.method, &r.params, &s, c)?
            } else {
                artex_domain::plan(&r.method, &r.params, &s, c)?
            };
            // Reuse the legacy result shape, with native dependency expansion.
            let result = batch.result.clone();
            let lifecycle =
                artex_domain::native_lifecycle::conversation_plan(&s, c, &ids, "delete")?;
            batch.changes = lifecycle.changes;
            batch.events = lifecycle.events;
            batch.expected = lifecycle.expected;
            batch.result = result;
            let runs = s
                .records("agent_runs")
                .into_iter()
                .filter(|v| {
                    v.data["conversation_id"]
                        .as_str()
                        .is_some_and(|id| ids.iter().any(|i| i == id))
                })
                .map(|v| v.header.id.clone())
                .collect();
            let files = ids
                .iter()
                .map(|id| json!({"conversation_id":id,"trash_generation":new_id("trash")}))
                .collect();
            (batch, runs, files)
        };
        original_expected(&s, &mut batch)?;
        self.lifecycle_admit(r, batch, runs, true, quarantine, false)
    }
    pub(super) fn native_agent_monitor(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        target: MonitorTarget,
    ) -> Result<NativeDispatch> {
        self.check_native_scope(scope)?;
        let s = self.snapshot()?;
        let observed = artex_core::now();
        match &target {
            MonitorTarget::Run { run_id } => {
                record(&s, "agent_runs", run_id)?;
            }
            MonitorTarget::Task { task_id } => {
                record(&s, "tasks", task_id)?;
            }
            MonitorTarget::Conversation { conversation_id } => {
                record(&s, "conversations", conversation_id)?;
            }
            MonitorTarget::All | MonitorTarget::Page { .. } => {}
        }
        let (offset, limit) = match target {
            MonitorTarget::Page { offset, limit } => (offset, limit.clamp(1, 200)),
            _ => (0, 200),
        };
        let mut selected = s
            .records("agent_runs")
            .into_iter()
            .filter(|run| match &target {
                MonitorTarget::All | MonitorTarget::Page { .. } => true,
                MonitorTarget::Run { run_id } => run.header.id == *run_id,
                MonitorTarget::Task { task_id } => run.data["task_id"] == *task_id,
                MonitorTarget::Conversation { conversation_id } => {
                    run.data["conversation_id"] == *conversation_id
                }
            })
            .collect::<Vec<_>>();
        selected.sort_by(|a, b| {
            let active = |run: &artex_core::Record| {
                matches!(
                    text(&run.data, "state"),
                    "queued" | "running" | "waiting" | "paused"
                )
            };
            active(b)
                .cmp(&active(a))
                .then(b.header.updated_at.cmp(&a.header.updated_at))
                .then(b.header.id.cmp(&a.header.id))
        });
        let total = selected.len();
        let has_more = offset.saturating_add(limit) < total;
        let mut task_control_expected = vec![];
        if let MonitorTarget::Task { task_id } = &target {
            let runs = s
                .records("agent_runs")
                .into_iter()
                .filter(|run| run.data["task_id"] == *task_id)
                .map(|run| run.header.id.clone())
                .collect::<Vec<_>>();
            let mut batch = artex_domain::native_lifecycle::stop_plan(
                &s,
                &user_context(&self.engine_epoch),
                &runs,
            )?;
            let task = record(&s, "tasks", task_id)?;
            batch.expected.push(Expected {
                collection: "tasks".into(),
                id: task_id.clone(),
                version: task.header.version.clone(),
            });
            for runtime in s
                .records("task_runtime")
                .into_iter()
                .filter(|r| r.data["task_id"] == *task_id)
            {
                batch.expected.push(Expected {
                    collection: "task_runtime".into(),
                    id: runtime.header.id.clone(),
                    version: runtime.header.version.clone(),
                });
            }
            original_expected(&s, &mut batch)?;
            task_control_expected = batch.expected;
        }
        let mut progress = BTreeMap::<String, String>::new();
        for row in s
            .records("activity")
            .into_iter()
            .chain(s.records("conversation_activities"))
        {
            let detail = row.data["detail"]["text"]
                .as_str()
                .filter(|v| v.len() <= MAX_CHUNK)
                .and_then(|v| serde_json::from_str::<Value>(v).ok());
            let rid = row.data["metadata"]["run_id"]
                .as_str()
                .or_else(|| detail.as_ref().and_then(|v| v["run_id"].as_str()))
                .or_else(|| detail.as_ref().and_then(|v| v["stream"]["run_id"].as_str()));
            if let Some(rid) = rid {
                let stamp = progress.entry(rid.into()).or_default();
                if row.header.created_at > *stamp {
                    *stamp = row.header.created_at.clone();
                }
            }
        }
        let mut rows = vec![];
        let mut contexts = vec![];
        for run in selected.into_iter().skip(offset).take(limit) {
            let rid = &run.header.id;
            let state = text(&run.data, "state");
            let controller = self.runtime.runs.get(rid);
            let wait = s
                .records("user_questions")
                .into_iter()
                .find(|q| q.data["run_id"] == *rid && q.data["state"] == "pending")
                .map(|q| text(&q.data, "kind").to_owned())
                .or_else(|| {
                    s.records("intercept_pending")
                        .into_iter()
                        .any(|p| p.data["audit"]["run_id"] == *rid && p.data["status"] == "pending")
                        .then(|| "intercept_approval".into())
                });
            let stage = if self.lifecycle_frozen(rid) {
                "stopping"
            } else if wait.is_some() {
                "waiting"
            } else if controller.is_some_and(|c| c.in_flight && !c.pending.is_empty()) {
                "tool"
            } else if controller.is_some_and(|c| c.in_flight) {
                "model"
            } else if state == "queued" {
                "queued"
            } else if state == "paused" {
                "paused"
            } else if matches!(state, "running" | "waiting") {
                "controller"
            } else {
                "terminal"
            };
            let last = s
                .records("tool_invocations")
                .into_iter()
                .filter(|r| r.data["run_id"] == *rid)
                .map(|r| r.header.updated_at.clone())
                .chain(
                    s.records("domain_events")
                        .into_iter()
                        .filter(|r| {
                            (r.data["subject_id"] == *rid || r.data["actor"]["run_id"] == *rid)
                                && r.data["kind"] != "RunHeartbeat"
                        })
                        .map(|r| r.header.created_at.clone()),
                )
                .chain(progress.get(rid).cloned())
                .max();
            let old = proof(&s, rid);
            let done = confirmed(&s, run);
            let current = run.data["engine_epoch"] == self.engine_epoch;
            if current && self.gateway.is_some() {
                contexts.push(self.stored_run_context(&s, rid)?);
            }
            let expected = artex_domain::native_lifecycle::stop_plan(
                &s,
                &user_context(&self.engine_epoch),
                std::slice::from_ref(rid),
            )?
            .expected;
            let started = run.data["started_at"]
                .as_str()
                .unwrap_or(&run.header.created_at);
            let ended = run.data["finished_at"].as_str();
            let elapsed = chrono::DateTime::parse_from_rfc3339(started)
                .ok()
                .zip(chrono::DateTime::parse_from_rfc3339(ended.unwrap_or(&observed)).ok())
                .and_then(|(start, end)| {
                    u64::try_from(end.signed_duration_since(start).num_milliseconds()).ok()
                });
            rows.push(json!({"run_id":rid,"agent_id":run.data["agent_id"],"task_id":run.data["task_id"],"conversation_id":run.data["conversation_id"],"state":state,"stage":stage,"wait_reason":wait,"last_progress_at":last,"heartbeat_at":null,"heartbeat_source":"not_recorded","cleanup_state":if done{"confirmed"}else if self.lifecycle_frozen(rid){"stopping"}else{"unknown"},"cleanup_confirmed":done,"executions":old.as_ref().map(|v|v["executions"].clone()).unwrap_or(json!([])),"expected":expected,"can_stop":current&&!self.effective_read_only()&&self.gateway.is_some(),"can_force_stop":current&&!self.effective_read_only()&&self.gateway.is_some()}));
            let row = rows.last_mut().unwrap();
            row["started_at"] = json!(started);
            row["ended_at"] = json!(ended);
            row["end_at"] = json!(ended);
            row["elapsed_ms"] = json!(elapsed);
            row["elapsed_basis"] = json!("wall_time");
        }
        let request = DispatchRequest {
            id: ticket.0.clone(),
            method: "native.lifecycle.monitor".into(),
            workspace_id: Some(scope.workspace_id.clone()),
            engine_epoch: Some(scope.engine_epoch.clone()),
            command_id: None,
            expected: vec![],
            timeout_ms: 5000,
            params: json!({}),
        };
        let gateway = self.gateway.clone();
        let workspace = s.workspace_id.clone();
        self.defer_read_worker(&request, move |cancel| {
            let end=Instant::now()+Duration::from_secs(2);
            for c in contexts {
                if cancel.cancelled() {
                    return Err(Error::new("CANCELLED", "Monitor query cancelled"));
                }
                if Instant::now()>=end {break;}
                if let Some(g) = gateway.as_ref() {
                    if let Ok(value) = g.request(
                        &workspace,
                        &c,
                        "run.inspect",
                        json!({}),
                        end.saturating_duration_since(Instant::now()).min(Duration::from_secs(2)),
                    ) {
                        if value["run_id"] == c.actor.run_id.as_deref().unwrap_or("")
                            && value["engine_epoch"] == c.actor.engine_epoch
                        {
                            if let Some(row) =
                                rows.iter_mut().find(|r| r["run_id"] == value["run_id"])
                            {
                                row["executions"] = value["executions"].clone();
                                row["cleanup_confirmed"] =
                                    json!(value["cleanup_confirmed"] == true);
                                row["cleanup_state"] =
                                    json!(if value["cleanup_confirmed"] == true {
                                        "confirmed"
                                    } else if value["stop_requested"] == true {
                                        "stopping"
                                    } else {
                                        "unknown"
                                    });
                            }
                        }
                    }
                }
            }
            Ok(json!({"runs":rows,"observed_at":observed,"task_control_expected":task_control_expected,"total":total,"has_more":has_more,"offset":offset,"next_offset":if has_more {Some(offset.saturating_add(limit))}else{None}}))
        })?;
        Ok(NativeDispatch::Deferred)
    }
}
fn cleanup_worker(
    gateway: Option<Arc<dyn HostGateway>>,
    workspace: &str,
    user: &Context,
    contexts: Vec<Context>,
    mut proofs: Vec<Value>,
    quarantine: Vec<Value>,
    strict: bool,
    force: bool,
    cancel: Cancellation,
    end: Instant,
) -> Result<Value> {
    let mut issued = Vec::with_capacity(contexts.len());
    for c in contexts {
        let rid = c.actor.run_id.clone().unwrap();
        let result = gateway
            .as_ref()
            .filter(|_| Instant::now() < end && !cancel.cancelled())
            .map(|g| {
                g.request(
                    workspace,
                    &c,
                    if force { "run.force_stop" } else { "run.stop" },
                    json!({"deadline_ms":0}),
                    end.saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(5)),
                )
            });
        let value = match result {
            Some(Ok(value)) => value,
            _ => {
                json!({"run_id":rid,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":false,"cleanup_state":"unknown","executions":[]})
            }
        };
        issued.push((c, value));
    }
    let poll_end = end.min(Instant::now() + Duration::from_secs(3));
    for (c, mut value) in issued {
        let rid = c.actor.run_id.clone().unwrap();
        while value["run_id"] == rid
            && value["engine_epoch"] == c.actor.engine_epoch
            && value["cleanup_confirmed"] != true
            && !cancel.cancelled()
            && Instant::now() < poll_end
            && ((value["external_cleanup_confirmed"] == true
                && value["active_host_calls"].as_u64().is_some_and(|n| n > 0))
                || value["executions"].as_array().is_some_and(|rows| {
                    rows.iter()
                        .any(|e| matches!(e["state"].as_str(), Some("running" | "stopping")))
                }))
        {
            std::thread::sleep(Duration::from_millis(20));
            let Some(g) = gateway.as_ref() else {
                break;
            };
            match g.request(
                workspace,
                &c,
                "run.inspect",
                json!({}),
                poll_end
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(2)),
            ) {
                Ok(next) => value = next,
                Err(_) => break,
            }
        }
        // Exact scope proof; accepted alone does not demonstrate cleanup.
        let valid = value["run_id"] == rid
            && value["engine_epoch"] == c.actor.engine_epoch
            && value["cleanup_confirmed"] == true
            && value["run_requests_quiesced"] == true
            && value["active_host_calls"].as_u64() == Some(0);
        let mut safe = value;
        safe["cleanup_confirmed"] = json!(valid);
        safe["cleanup_state"] = json!(if valid { "confirmed" } else { "unknown" });
        proofs.push(safe);
    }
    let confirmed = proofs.iter().all(|v| v["cleanup_confirmed"] == true);
    let mut done = vec![];
    if (!strict || confirmed) && !cancel.cancelled() && Instant::now() < end {
        for input in quarantine {
            let Some(g) = gateway.as_ref() else {
                break;
            };
            match g.request(
                workspace,
                user,
                quarantine_operation(&input, false),
                input.clone(),
                Duration::from_secs(15),
            ) {
                Ok(value) if value["retired"] == true || value["quarantined"] == true => {
                    let mut input = input;
                    input["files_present"] = json!(value["quarantined"] == true);
                    done.push(input);
                }
                _ => {
                    let mut restored = true;
                    for old in done.iter().rev() {
                        restored &= g
                            .request(
                                workspace,
                                user,
                                quarantine_operation(old, true),
                                old.clone(),
                                Duration::from_secs(15),
                            )
                            .is_ok();
                    }
                    if !restored {
                        return Err(Error::new(
                            "RECOVERY_REQUIRED",
                            "File quarantine compensation is unconfirmed",
                        ));
                    }
                    return Ok(
                        json!({"runs":proofs,"cleanup_confirmed":false,"quarantined":[],"cancelled":cancel.cancelled()}),
                    );
                }
            }
        }
    }
    Ok(
        json!({"runs":proofs,"cleanup_confirmed":confirmed,"quarantined":done,"cancelled":cancel.cancelled()||Instant::now()>=end}),
    )
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct OrderHost(AtomicUsize);
    impl HostGateway for OrderHost {
        fn request(
            &self,
            _: &str,
            c: &Context,
            operation: &str,
            input: Value,
            _: Duration,
        ) -> Result<Value> {
            let done = match operation {
                "run.stop" => {
                    assert_eq!(input["deadline_ms"], 0);
                    self.0.fetch_add(1, Ordering::SeqCst);
                    false
                }
                "run.inspect" => {
                    assert_eq!(
                        self.0.load(Ordering::SeqCst),
                        2,
                        "Every target must receive cancellation before waiting for one target"
                    );
                    true
                }
                _ => panic!("Unexpected fixture operation"),
            };
            Ok(
                json!({"run_id":c.actor.run_id,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":done,"external_cleanup_confirmed":true,"active_host_calls":if done{0}else{1},"run_requests_quiesced":done,"executions":[]}),
            )
        }
    }
    #[test]
    fn batch_stop_issues_every_target_before_quiescence_polling() {
        let epoch = new_id("epoch");
        let contexts = (0..2)
            .map(|_| Context {
                actor: Actor {
                    kind: "agent".into(),
                    agent_id: Some(new_id("agent")),
                    run_id: Some(new_id("run")),
                    engine_epoch: epoch.clone(),
                },
                task_id: None,
                intent_id: None,
                conversation_id: Some(new_id("conversation")),
            })
            .collect();
        let host = Arc::new(OrderHost(AtomicUsize::new(0)));
        let value = cleanup_worker(
            Some(host.clone()),
            &new_id("workspace"),
            &user_context(&epoch),
            contexts,
            vec![],
            vec![],
            true,
            false,
            Cancellation::default(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(value["cleanup_confirmed"], true);
        assert_eq!(value["runs"].as_array().unwrap().len(), 2);
        assert_eq!(host.0.load(Ordering::SeqCst), 2);
    }
}
