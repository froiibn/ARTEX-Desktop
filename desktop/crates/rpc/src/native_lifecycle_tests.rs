use super::*;
use crate::native::*;
use artex_domain::Planner;
use artex_runtime::HostGateway;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[test]
fn failed_model_only_task_reopens_stops_and_deletes_without_a_fictitious_process_stop() {
    let (d, mut r, run, _conversation, host) = seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.failed.model", &json!({}));
    let exp = p.insert("explorations", "exploration", json!({})).unwrap();
    let task = p.insert("tasks", "task", json!({"name":"Failed model task","exploration_id":exp,"status":"paused","paused":true})).unwrap();
    let mut data = s.get("agent_runs", &run).unwrap().data.clone();
    data["task_id"] = json!(task);
    p.put("agent_runs", &run, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    r.finish_runtime(&run, "failed", "EXTERNAL_FAILURE: LLM provider HTTP 402")
        .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while host.stops.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < until, "Original lifetime cleanup was not issued");
        std::thread::sleep(Duration::from_millis(1));
    }
    let original_stops = host.stops.load(Ordering::SeqCst);
    r.store.take();
    let (w, _) = r
        .native_open(&OpenOptions {
            path: d.path().join("store"),
            mode: OpenMode::Existing,
            read_only: false,
        })
        .unwrap();
    r.attach_host(host.clone());
    // Reproduce the previous UI's repeated, synthetic unknown cleanup receipts.
    for _ in 0..3 {
        let old_epoch =
            r.snapshot().unwrap().get("agent_runs", &run).unwrap().data["engine_epoch"].clone();
        let mut b = artex_domain::empty_batch("native.lifecycle.cleanup", &json!({}));
        b.result = json!({"runs":[{"run_id":run,"engine_epoch":old_epoch,"cleanup_confirmed":false,"cleanup_state":"unknown","executions":[]}]});
        r.internal_commit(b).unwrap();
    }
    let preview = r
        .lifecycle_preview(
            DeletionTarget::Tasks {
                ids: vec![task.clone()],
            },
            DeletionOptions::Tasks {
                options: TaskDeleteOptions::default(),
            },
        )
        .unwrap();
    assert_eq!(preview["cleanup_confirmed"], true);
    assert_eq!(preview["can_apply"], true);
    let ticket = NativeTicket::new();
    let monitor = NativeTicket::new();
    r.native_query(
        &monitor,
        &w.scope,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::Task {
                task_id: task.clone(),
            },
        },
    )
    .unwrap();
    let versions = serde_json::from_value(
        settle(&mut r, &monitor).unwrap().value["task_control_expected"].clone(),
    )
    .unwrap();
    r.native_run_control(
        &ticket,
        &w.scope,
        new_id("command"),
        versions,
        RunControlTarget::Task {
            task_id: task.clone(),
        },
        RunControlAction::Stop,
    )
    .unwrap();
    assert_eq!(
        settle(&mut r, &ticket).unwrap().value["cleanup_confirmed"],
        true
    );
    let preview = r
        .lifecycle_preview(
            DeletionTarget::Tasks {
                ids: vec![task.clone()],
            },
            DeletionOptions::Tasks {
                options: TaskDeleteOptions::default(),
            },
        )
        .unwrap();
    let deletion = NativeTicket::new();
    r.native_task_delete(
        &deletion,
        &w.scope,
        new_id("command"),
        serde_json::from_value(preview["expected"].clone()).unwrap(),
        vec![task.clone()],
        TaskDeleteOptions::default(),
    )
    .unwrap();
    assert_eq!(settle(&mut r, &deletion).unwrap().value["succeeded"], 1);
    assert!(r.snapshot().unwrap().get("tasks", &task).is_none());
    assert_eq!(
        host.stops.load(Ordering::SeqCst),
        original_stops,
        "Never replay effects from the old epoch"
    );
    assert!(
        r.snapshot()
            .unwrap()
            .records("command_receipts")
            .iter()
            .any(
                |v| v.data["result"]["runs"].as_array().is_some_and(|rows| rows
                    .iter()
                    .any(|p| p["proof_source"] == "durable_terminal_without_tool_admission"))
            )
    );
}

#[test]
fn terminal_cleanup_inference_requires_audited_finish_and_no_external_admission_or_contradicting_proof()
 {
    for blocker in [
        "missing_finish",
        "tool",
        "execution",
        "active_call",
        "imported",
    ] {
        let (_d, mut r, run, conversation, _host) = seeded();
        r.finish_runtime(&run, "failed", "EXTERNAL_FAILURE: LLM failed before tools")
            .unwrap();
        let s = r.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.cleanup.blocker", &json!({}));
        match blocker {
            "tool" => {
                p.insert("tool_invocations","invocation",json!({"run_id":run,"tool_use_id":"external-call","name":"Bash","state":"recovery_required"})).unwrap();
            }
            "missing_finish" => {
                let mut incomplete = s.snapshot.as_ref().clone();
                incomplete
                    .collections
                    .get_mut("command_receipts")
                    .unwrap()
                    .retain(|_, receipt| receipt.data["method"] != "run.finish");
                assert!(!crate::native_lifecycle::confirmed(
                    &incomplete,
                    incomplete.get("agent_runs", &run).unwrap()
                ));
                continue;
            }
            "imported" => {
                let mut imported = s.snapshot.as_ref().clone();
                imported
                    .collections
                    .get_mut("agent_runs")
                    .unwrap()
                    .get_mut(&run)
                    .unwrap()
                    .header
                    .legacy_key = json!("legacy-run");
                assert!(!crate::native_lifecycle::confirmed(
                    &imported,
                    imported.get("agent_runs", &run).unwrap()
                ));
                continue;
            }
            _ => {}
        }
        r.internal_commit(p.done(json!({}))).unwrap();
        if matches!(blocker, "execution" | "active_call") {
            let s = r.snapshot().unwrap();
            let mut b = artex_domain::empty_batch("native.lifecycle.cleanup", &json!({}));
            b.result = json!({"runs":[{"run_id":run,"engine_epoch":s.get("agent_runs",&run).unwrap().data["engine_epoch"],"cleanup_confirmed":false,"executions":if blocker=="execution" {json!([{"execution_id":"owned-process","cleanup_confirmed":false}])}else{json!([])},"active_host_calls":if blocker=="active_call"{1}else{0}}]});
            r.internal_commit(b).unwrap();
        }
        let s = r.snapshot().unwrap();
        let mut mutated = s.snapshot.as_ref().clone();
        assert!(
            !crate::native_lifecycle::confirmed(&mutated, mutated.get("agent_runs", &run).unwrap()),
            "{blocker}"
        );
        // Confirm inference remains conservative without consulting a fresh host.
        mutated
            .collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut(&run)
            .unwrap()
            .data["state"] = json!("running");
        assert!(!crate::native_lifecycle::confirmed(
            &mutated,
            mutated.get("agent_runs", &run).unwrap()
        ));
        assert!(s.get("conversations", &conversation).is_some());
    }
}

#[test]
fn opening_paused_and_capacity_queued_tasks_does_not_activate_triggers_or_keep_writing() {
    let dir = tempfile::tempdir().unwrap();
    let options = OpenOptions {
        path: dir.path().join("idle"),
        mode: OpenMode::Create {
            name: "Idle workspace".into(),
        },
        read_only: false,
    };
    let mut r = Router::new(BTreeMap::new());
    r.native_open(&options).unwrap();
    let s = r.snapshot().unwrap();
    let agent = s
        .records("agents")
        .into_iter()
        .find(|a| a.data["key"] == "mainagent")
        .unwrap();
    let mut p = Planner::new(&s, "fixture.idle", &json!({}));
    let exp = p.insert("explorations", "exploration", json!({})).unwrap();
    let task = p
        .insert(
            "tasks",
            "task",
            json!({"name":"Paused","exploration_id":exp,"status":"paused","paused":true}),
        )
        .unwrap();
    let exp2 = p.insert("explorations", "exploration", json!({})).unwrap();
    let queued = p.insert("tasks", "task", json!({"name":"At capacity","exploration_id":exp2,"status":"created","queued":true,"paused":false})).unwrap();
    let run = p.insert("agent_runs", "run", json!({"agent_id":agent.header.id,"task_id":task,"engine_epoch":r.engine_epoch,"state":"paused"})).unwrap();
    p.insert(
        "agent_triggers",
        "trigger",
        json!({"agent_key":"mainagent","enabled":true,"interval_sec":1,"on_task_create":true}),
    )
    .unwrap();
    let c = Context {
        actor: Actor {
            kind: "engine".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: r.engine_epoch.clone(),
        },
        task_id: Some(task.clone()),
        intent_id: None,
        conversation_id: None,
    };
    artex_domain::conversation::push_event(&mut p.batch, &s, &c, "TaskCreated", "tasks", &task)
        .unwrap();
    r.internal_commit(p.done(json!({}))).unwrap();
    // Simulate an interrupted previous lifetime, preserving pending work.
    r.store.take();
    r.native_automation_admitted = true;
    r.native_open(&OpenOptions {
        mode: OpenMode::Existing,
        ..options
    })
    .unwrap();
    assert!(!r.native_automation_admitted);
    let opened = r.snapshot().unwrap();
    assert_eq!(
        opened.get("agent_runs", &run).unwrap().data["state"],
        "interrupted"
    );
    for id in [&task, &queued] {
        let row = opened.get("tasks", id).unwrap();
        assert_eq!(row.data["status"], "paused");
        assert_eq!(row.data["paused"], true);
        assert_eq!(row.data["queued"], false);
    }
    for _ in 0..15 {
        r.maintain().unwrap();
    }
    let after = r.snapshot().unwrap();
    assert_eq!(
        after.commit_seq, opened.commit_seq,
        "Idle observation must settle without ongoing writes"
    );
    assert_eq!(after.records("agent_runs"), opened.records("agent_runs"));
    assert!(r.runtime.runs.is_empty());
    assert!(after.records("agent_deliveries").is_empty());
}

#[test]
fn reopened_paused_task_can_stop_and_delete_using_durable_retirement_proof() {
    let (dir, mut r, run, _conversation, host) = seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.paused", &json!({}));
    let exp = p.insert("explorations", "exploration", json!({})).unwrap();
    let task = p
        .insert(
            "tasks",
            "task",
            json!({"name":"Paused","exploration_id":exp,"status":"paused","paused":true}),
        )
        .unwrap();
    let mut data = s.get("agent_runs", &run).unwrap().data.clone();
    data["task_id"] = json!(task);
    data["state"] = json!("paused");
    p.put("agent_runs", &run, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    let scope = r.native_scope().unwrap();
    r.native_close(&scope).unwrap();
    let (workspace, _) = r
        .native_open(&OpenOptions {
            path: dir.path().join("store"),
            mode: OpenMode::Existing,
            read_only: false,
        })
        .unwrap();
    r.attach_host(host.clone());
    let stops = host.stops.load(Ordering::SeqCst);
    let query = NativeTicket::new();
    r.native_query(
        &query,
        &workspace.scope,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::Task {
                task_id: task.clone(),
            },
        },
    )
    .unwrap();
    let monitor = settle(&mut r, &query).unwrap();
    assert_eq!(
        monitor.value["runs"][0]["can_stop"], false,
        "The old per-run control is unavailable"
    );
    let ticket = NativeTicket::new();
    r.native_run_control(
        &ticket,
        &workspace.scope,
        new_id("command"),
        serde_json::from_value(monitor.value["task_control_expected"].clone()).unwrap(),
        RunControlTarget::Task {
            task_id: task.clone(),
        },
        RunControlAction::Stop,
    )
    .unwrap();
    assert_eq!(
        settle(&mut r, &ticket).unwrap().value["cleanup_confirmed"],
        true
    );
    assert_eq!(
        host.stops.load(Ordering::SeqCst),
        stops,
        "Old external effects must never replay"
    );
    let preview = r
        .lifecycle_preview(
            DeletionTarget::Tasks {
                ids: vec![task.clone()],
            },
            DeletionOptions::Tasks {
                options: TaskDeleteOptions::default(),
            },
        )
        .unwrap();
    assert_eq!(preview["can_apply"], true);
    let deletion = NativeTicket::new();
    r.native_task_delete(
        &deletion,
        &workspace.scope,
        new_id("command"),
        serde_json::from_value(preview["expected"].clone()).unwrap(),
        vec![task.clone()],
        TaskDeleteOptions::default(),
    )
    .unwrap();
    assert_eq!(settle(&mut r, &deletion).unwrap().value["succeeded"], 1);
    assert!(r.snapshot().unwrap().get("tasks", &task).is_none());
}

struct Host {
    confirmed: AtomicBool,
    stops: AtomicUsize,
    inspections: AtomicUsize,
    quiescing: AtomicUsize,
    block_stops: AtomicBool,
    retired_binding: AtomicBool,
    quarantine: AtomicBool,
    restore: AtomicBool,
    release: AtomicBool,
    live: PathBuf,
    retired: PathBuf,
}
impl HostGateway for Host {
    fn request(
        &self,
        _: &str,
        c: &Context,
        operation: &str,
        _: Value,
        _: Duration,
    ) -> Result<Value> {
        match operation {
            "run.stop" | "run.force_stop" | "run.inspect" => {
                if operation != "run.inspect" {
                    self.stops.fetch_add(1, Ordering::SeqCst);
                    let end = Instant::now() + Duration::from_secs(3);
                    while self.block_stops.load(Ordering::SeqCst)
                        && !self.release.load(Ordering::SeqCst)
                        && Instant::now() < end
                    {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                } else {
                    self.inspections.fetch_add(1, Ordering::SeqCst);
                    let _ = self
                        .quiescing
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
                }
                if self.quiescing.load(Ordering::SeqCst) > 0 {
                    return Ok(
                        json!({"accepted":true,"run_id":c.actor.run_id,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":false,"external_cleanup_confirmed":true,"active_host_calls":1,"run_requests_quiesced":false,"executions":[]}),
                    );
                }
                Ok(
                    json!({"accepted":true,"run_id":c.actor.run_id,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":self.confirmed.load(Ordering::SeqCst),"active_host_calls":0,"run_requests_quiesced":true,"executions":[]}),
                )
            }
            "workspace.quarantine_conversation" => {
                if self.live.exists() {
                    std::fs::rename(&self.live, &self.retired)?;
                }
                self.quarantine.store(true, Ordering::SeqCst);
                Ok(json!({"retired":true,"quarantined":self.retired.exists()}))
            }
            "workspace.restore_conversation_quarantine" => {
                self.restore.store(true, Ordering::SeqCst);
                let end = Instant::now() + Duration::from_secs(3);
                while !self.release.load(Ordering::SeqCst) && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(1));
                }
                if self.retired.exists() {
                    std::fs::rename(&self.retired, &self.live)?;
                }
                Ok(json!({"restored":true}))
            }
            "workspace.complete_conversation_quarantine" => Ok(json!({"released":true})),
            "ui.stop" => Ok(json!({"accepted":true})),
            _ => Err(Error::new("METHOD_NOT_FOUND", "Fixture operation")),
        }
    }
    fn retire(&self, _: Duration) -> Result<()> {
        self.retired_binding.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn retirement_report(&self, _: &str, contexts: &[Context]) -> Result<Option<Value>> {
        assert!(self.retired_binding.load(Ordering::SeqCst));
        for c in contexts {
            assert!(
                c.actor
                    .agent_id
                    .as_deref()
                    .is_some_and(|id| artex_core::valid_id(id, "agent"))
            );
        }
        Ok(Some(
            json!({"runs":contexts.iter().map(|c|json!({"run_id":c.actor.run_id,"engine_epoch":c.actor.engine_epoch,"cleanup_confirmed":true,"active_host_calls":0,"run_requests_quiesced":true,"executions":[]})).collect::<Vec<_>>()}),
        ))
    }
}
fn seeded() -> (tempfile::TempDir, Router, String, String, Arc<Host>) {
    let (dir, mut router, run, context) = regression::seeded();
    router.enable_native();
    let conversation = context.conversation_id.unwrap();
    let host = Arc::new(Host {
        confirmed: AtomicBool::new(true),
        stops: AtomicUsize::new(0),
        inspections: AtomicUsize::new(0),
        quiescing: AtomicUsize::new(0),
        block_stops: AtomicBool::new(false),
        retired_binding: AtomicBool::new(false),
        quarantine: AtomicBool::new(false),
        restore: AtomicBool::new(false),
        release: AtomicBool::new(true),
        live: dir.path().join("conversation-files"),
        retired: dir.path().join("retired-files"),
    });
    std::fs::create_dir(&host.live).unwrap();
    std::fs::write(host.live.join("owned.txt"), b"owned").unwrap();
    router.attach_host(host.clone());
    let s = router.snapshot().unwrap();
    let c = artex_agent::context_for_run(&s, &run, &router.engine_epoch).unwrap();
    let batch = artex_domain::conversation::append_activity(
        &s,
        &c,
        &run,
        "user",
        "Fixture history",
        &artex_core::body("original history"),
        None,
    )
    .unwrap();
    router.internal_commit(batch).unwrap();
    (dir, router, run, conversation, host)
}
fn expected(router: &Router, id: &str, action: ConversationAction) -> Vec<Expected> {
    serde_json::from_value(
        router
            .lifecycle_preview(
                DeletionTarget::Conversations {
                    ids: vec![id.into()],
                },
                DeletionOptions::Conversations { action },
            )
            .unwrap()["expected"]
            .clone(),
    )
    .unwrap()
}
fn settle(router: &mut Router, ticket: &NativeTicket) -> Result<NativeReply> {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        router.native_poll_retirement().unwrap();
        for done in router.drain_native_completions() {
            if done.ticket == *ticket {
                return done.result;
            }
        }
        assert!(Instant::now() < end, "Lifecycle completion timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn clear_history_requires_exact_cleanup_proof_and_preserves_conversation_and_audits() {
    let (_d, mut r, run, id, host) = seeded();
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    let result = r
        .native_conversation_manage(
            &ticket,
            &scope,
            new_id("command"),
            expected(&r, &id, ConversationAction::ClearHistory),
            vec![id.clone()],
            ConversationAction::ClearHistory,
        )
        .unwrap();
    assert!(matches!(result, NativeDispatch::Deferred));
    let done = settle(&mut r, &ticket).unwrap();
    assert_eq!(done.value["cleanup_confirmed"], true);
    assert_eq!(done.value["secure_erase"], false);
    let s = r.snapshot().unwrap();
    assert!(s.get("conversations", &id).is_some());
    assert!(s.records("conversation_activities").is_empty());
    assert_eq!(
        s.get("agent_runs", &run).unwrap().data["state"],
        "cancelled"
    );
    assert!(!host.live.exists());
    assert_eq!(
        std::fs::read(host.retired.join("owned.txt")).unwrap(),
        b"owned"
    );
    assert!(
        s.records("command_receipts")
            .iter()
            .any(|v| v.data["method"] == "native.lifecycle.cleanup")
    );
}
#[test]
fn accepted_only_stop_is_not_cleanup_proof_and_old_epoch_unknown_cannot_delete_after_reopen() {
    let (d, mut r, run, id, host) = seeded();
    host.confirmed.store(false, Ordering::SeqCst);
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    r.native_conversation_manage(
        &ticket,
        &scope,
        new_id("command"),
        expected(&r, &id, ConversationAction::DeletePermanently),
        vec![id.clone()],
        ConversationAction::DeletePermanently,
    )
    .unwrap();
    assert_eq!(
        settle(&mut r, &ticket).unwrap_err().code,
        "RECOVERY_REQUIRED"
    );
    assert!(!host.quarantine.load(Ordering::SeqCst));
    assert!(host.live.is_dir());
    assert_eq!(
        r.snapshot().unwrap().get("agent_runs", &run).unwrap().data["state"],
        "cancelled"
    );
    r.store.take();
    r.engine_epoch = new_id("epoch");
    r.store = Some(Store::open(&d.path().join("store")).unwrap());
    host.confirmed.store(true, Ordering::SeqCst);
    let preview = r
        .lifecycle_preview(
            DeletionTarget::Conversations {
                ids: vec![id.clone()],
            },
            DeletionOptions::Conversations {
                action: ConversationAction::DeletePermanently,
            },
        )
        .unwrap();
    assert_eq!(preview["can_apply"], false);
    assert_eq!(preview["cleanup_confirmed"], false);
    let fresh = r.native_scope().unwrap();
    let err = r
        .native_conversation_manage(
            &NativeTicket::new(),
            &fresh,
            new_id("command"),
            serde_json::from_value(preview["expected"].clone()).unwrap(),
            vec![id.clone()],
            ConversationAction::DeletePermanently,
        )
        .err()
        .unwrap();
    assert_eq!(err.code, "RECOVERY_REQUIRED");
    assert_eq!(host.stops.load(Ordering::SeqCst), 1);
    assert!(r.snapshot().unwrap().get("conversations", &id).is_some());
    assert!(
        r.native_conversation_manage(
            &NativeTicket::new(),
            &scope,
            new_id("command"),
            vec![],
            vec![id],
            ConversationAction::Trash
        )
        .is_err()
    );
}
#[test]
fn trash_restore_preserves_history_and_old_worker_cannot_resume_or_recreate_it() {
    let (_d, mut r, run, id, host) = seeded();
    host.confirmed.store(false, Ordering::SeqCst);
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    let cid = new_id("command");
    let versions = expected(&r, &id, ConversationAction::Trash);
    r.native_conversation_manage(
        &ticket,
        &scope,
        cid.clone(),
        versions.clone(),
        vec![id.clone()],
        ConversationAction::Trash,
    )
    .unwrap();
    settle(&mut r, &ticket).unwrap();
    let s = r.snapshot().unwrap();
    let generation = s.get("conversations", &id).unwrap().data["trash_generation"].clone();
    assert!(
        generation
            .as_str()
            .is_some_and(|v| artex_core::valid_id(v, "trash"))
    );
    assert_eq!(
        r.lifecycle_trash().unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        artex_domain::conversation::query(
            "conversation.list",
            &json!({}),
            &s,
            &Context {
                actor: Actor {
                    kind: "user".into(),
                    agent_id: None,
                    run_id: None,
                    engine_epoch: r.engine_epoch.clone()
                },
                task_id: None,
                intent_id: None,
                conversation_id: None
            }
        )
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty()
    );
    let c = Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: r.engine_epoch.clone(),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    };
    assert!(
        artex_domain::conversation::plan(
            "conversation.update",
            &json!({"id":id,"patch":{"title":"Wrong"}}),
            &s,
            &c
        )
        .is_err()
    );
    let restore = NativeTicket::new();
    r.native_conversation_manage(
        &restore,
        &scope,
        new_id("command"),
        expected(&r, &id, ConversationAction::Restore),
        vec![id.clone()],
        ConversationAction::Restore,
    )
    .unwrap();
    let s = r.snapshot().unwrap();
    assert!(s.get("conversations", &id).unwrap().data["trashed_at"].is_null());
    assert_ne!(
        s.get("conversations", &id).unwrap().data["trash_generation"],
        generation
    );
    assert!(!s.records("conversation_activities").is_empty());
    let old = Context {
        actor: Actor {
            kind: "agent".into(),
            agent_id: s.get("agent_runs", &run).unwrap().data["agent_id"]
                .as_str()
                .map(str::to_owned),
            run_id: Some(run.clone()),
            engine_epoch: r.engine_epoch.clone(),
        },
        task_id: None,
        intent_id: None,
        conversation_id: Some(id.clone()),
    };
    assert_eq!(
        artex_domain::conversation::append_activity(
            &s,
            &old,
            &run,
            "text",
            "Late",
            &artex_core::body("late"),
            None
        )
        .unwrap_err()
        .code,
        "CANCELLED"
    );
    assert!(matches!(
        r.native_conversation_manage(
            &NativeTicket::new(),
            &scope,
            cid,
            versions,
            vec![id],
            ConversationAction::Trash
        )
        .unwrap(),
        NativeDispatch::Complete(_)
    ));
}
#[test]
fn staged_history_clear_cas_failure_restores_owned_files_and_preserves_newer_title() {
    let (_d, mut r, _run, id, host) = seeded();
    host.release.store(false, Ordering::SeqCst);
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    r.native_conversation_manage(
        &ticket,
        &scope,
        new_id("command"),
        expected(&r, &id, ConversationAction::ClearHistory),
        vec![id.clone()],
        ConversationAction::ClearHistory,
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while !host.quarantine.load(Ordering::SeqCst) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.concurrent", &json!({}));
    let mut data = s.get("conversations", &id).unwrap().data.clone();
    data["title"] = json!("Newer title");
    p.put("conversations", &id, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    r.native_poll_retirement().unwrap();
    assert!(r.native_lifecycle_busy());
    assert!(r.native_close(&scope).is_err());
    host.release.store(true, Ordering::SeqCst);
    assert_eq!(
        settle(&mut r, &ticket).unwrap_err().code,
        "VERSION_CONFLICT"
    );
    assert!(host.restore.load(Ordering::SeqCst));
    assert_eq!(
        std::fs::read(host.live.join("owned.txt")).unwrap(),
        b"owned"
    );
    assert_eq!(
        r.snapshot()
            .unwrap()
            .get("conversations", &id)
            .unwrap()
            .data["title"],
        "Newer title"
    );
    assert!(
        !r.snapshot()
            .unwrap()
            .records("conversation_activities")
            .is_empty()
    );
}
#[test]
fn cancellation_after_staging_never_late_commits_history_deletion() {
    let (_d, mut r, _run, id, host) = seeded();
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    r.native_conversation_manage(
        &ticket,
        &scope,
        new_id("command"),
        expected(&r, &id, ConversationAction::ClearHistory),
        vec![id.clone()],
        ConversationAction::ClearHistory,
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(3);
    while !host.quarantine.load(Ordering::SeqCst) {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    r.native_cancel(&scope, &ticket).unwrap();
    assert_eq!(settle(&mut r, &ticket).unwrap_err().code, "CANCELLED");
    let end = Instant::now() + Duration::from_secs(3);
    while r.native_lifecycle_busy() {
        r.native_poll_retirement().unwrap();
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(host.live.join("owned.txt").is_file());
    assert!(
        !r.snapshot()
            .unwrap()
            .records("conversation_activities")
            .is_empty()
    );
    assert!(r.drain_native_completions().is_empty());
}

#[test]
fn monitor_observes_real_progress_without_heartbeat_invention_or_stop_effects_and_cancel_is_read_only()
 {
    let (_d, mut r, run, _id, host) = seeded();
    let scope = r.native_scope().unwrap();
    let seq = r.snapshot().unwrap().commit_seq;
    let ticket = NativeTicket::new();
    r.native_query(
        &ticket,
        &scope,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::Run {
                run_id: run.clone(),
            },
        },
    )
    .unwrap();
    let result = settle(&mut r, &ticket).unwrap().value;
    assert_eq!(result["runs"][0]["run_id"], run);
    assert!(result["runs"][0]["last_progress_at"].is_string());
    assert!(result["runs"][0]["heartbeat_at"].is_null());
    assert_eq!(result["runs"][0]["heartbeat_source"], "not_recorded");
    assert_eq!(result["total"], 1);
    assert_eq!(result["has_more"], false);
    assert_eq!(r.snapshot().unwrap().commit_seq, seq);
    assert_eq!(host.stops.load(Ordering::SeqCst), 0);
    let cancel = NativeTicket::new();
    r.native_query(
        &cancel,
        &scope,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::All,
        },
    )
    .unwrap();
    r.native_cancel(&scope, &cancel).unwrap();
    assert_eq!(settle(&mut r, &cancel).unwrap_err().code, "CANCELLED");
    assert_eq!(host.stops.load(Ordering::SeqCst), 0);
    assert_eq!(r.snapshot().unwrap().commit_seq, seq);
}

#[test]
fn task_batch_reports_missing_target_separately_and_preserves_other_task_shared_asset_and_run() {
    let (_d, mut r, run, _conversation, host) = seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.shared", &json!({}));
    let ea = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"A"}),
        )
        .unwrap();
    let eb = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"B"}),
        )
        .unwrap();
    let a = p
        .insert(
            "tasks",
            "task",
            json!({"name":"A","exploration_id":ea,"paused":true}),
        )
        .unwrap();
    let b = p
        .insert(
            "tasks",
            "task",
            json!({"name":"B","exploration_id":eb,"paused":true}),
        )
        .unwrap();
    let asset = p
        .insert(
            "assets",
            "asset",
            json!({"type":"root_domain","domain":"shared.fixture","task_ids":[a,b]}),
        )
        .unwrap();
    r.internal_commit(p.done(json!({}))).unwrap();
    let missing = new_id("task");
    let options = TaskDeleteOptions {
        delete_assets: true,
        ..Default::default()
    };
    let target = DeletionTarget::Tasks {
        ids: vec![a.clone(), missing.clone()],
    };
    let preview = r
        .lifecycle_preview(
            target,
            DeletionOptions::Tasks {
                options: options.clone(),
            },
        )
        .unwrap();
    let versions = serde_json::from_value(preview["expected"].clone()).unwrap();
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    r.native_task_delete(
        &ticket,
        &scope,
        new_id("command"),
        versions,
        vec![a.clone(), missing],
        options,
    )
    .unwrap();
    let result = settle(&mut r, &ticket).unwrap().value;
    assert_eq!(result["succeeded"], 1);
    assert_eq!(result["failed"], 1);
    assert_eq!(result["results"][0]["ok"], true);
    assert_eq!(result["results"][1]["error"]["code"], "NOT_FOUND");
    let s = r.snapshot().unwrap();
    assert!(s.get("tasks", &a).is_none());
    assert!(s.get("tasks", &b).is_some());
    assert!(s.get("assets", &asset).is_some());
    assert_eq!(
        s.get("assets", &asset).unwrap().data["task_ids"],
        json!([b])
    );
    assert_eq!(s.get("agent_runs", &run).unwrap().data["state"], "running");
    assert_eq!(host.stops.load(Ordering::SeqCst), 0);
}

#[test]
fn cleanup_waits_for_actual_request_quiescence_before_file_retirement() {
    let (_d, mut r, _run, id, host) = seeded();
    host.quiescing.store(3, Ordering::SeqCst);
    let scope = r.native_scope().unwrap();
    let ticket = NativeTicket::new();
    r.native_conversation_manage(
        &ticket,
        &scope,
        new_id("command"),
        expected(&r, &id, ConversationAction::ClearHistory),
        vec![id],
        ConversationAction::ClearHistory,
    )
    .unwrap();
    let result = settle(&mut r, &ticket).unwrap();
    assert_eq!(result.value["cleanup_confirmed"], true);
    assert!(host.inspections.load(Ordering::SeqCst) >= 3);
    assert_eq!(host.stops.load(Ordering::SeqCst), 1);
    assert!(host.quarantine.load(Ordering::SeqCst));
}

#[test]
fn task_stop_freezes_its_admission_and_clock_while_another_task_scheduler_continues() {
    let (_d, mut r, run, _conversation, host) = seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.scheduler", &json!({}));
    let ea = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"A"}),
        )
        .unwrap();
    let eb = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"B"}),
        )
        .unwrap();
    let a = p
        .insert(
            "tasks",
            "task",
            json!({"name":"A","exploration_id":ea,"status":"created","queued":true,"paused":false}),
        )
        .unwrap();
    let b = p
        .insert(
            "tasks",
            "task",
            json!({"name":"B","exploration_id":eb,"status":"created","queued":true,"paused":false}),
        )
        .unwrap();
    let mut rd = s.get("agent_runs", &run).unwrap().data.clone();
    rd["conversation_id"] = Value::Null;
    rd["task_id"] = json!(a);
    p.put("agent_runs", &run, rd);
    let runtime=p.insert("task_runtime",artex_core::collection_prefix("task_runtime").unwrap(),json!({"task_id":a,"accounting_state":"known","last_active_clock":artex_platform::active_clock_ms().unwrap().to_string()})).unwrap();
    r.internal_commit(p.done(json!({}))).unwrap();
    let scope = r.native_scope().unwrap();
    let q = NativeTicket::new();
    r.native_query(
        &q,
        &scope,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::Task { task_id: a.clone() },
        },
    )
    .unwrap();
    let versions =
        serde_json::from_value(settle(&mut r, &q).unwrap().value["task_control_expected"].clone())
            .unwrap();
    host.block_stops.store(true, Ordering::SeqCst);
    host.release.store(false, Ordering::SeqCst);
    let ticket = NativeTicket::new();
    r.native_run_control(
        &ticket,
        &scope,
        new_id("command"),
        versions,
        RunControlTarget::Task { task_id: a.clone() },
        RunControlAction::Stop,
    )
    .unwrap();
    assert!(r.deleting_tasks.contains(&a));
    r.runtime_tick().unwrap();
    let s = r.snapshot().unwrap();
    assert_eq!(s.get("tasks", &a).unwrap().data["status"], "created");
    assert_eq!(
        s.records("agent_runs")
            .into_iter()
            .filter(|run| run.data["task_id"] == a)
            .count(),
        1
    );
    assert_eq!(s.get("tasks", &b).unwrap().data["status"], "running");
    assert_eq!(s.get("tasks", &b).unwrap().data["queued"], false);
    assert!(
        s.records("domain_events")
            .into_iter()
            .any(|event| event.data["kind"] == "TaskStarted" && event.data["subject_id"] == b)
    );
    host.release.store(true, Ordering::SeqCst);
    settle(&mut r, &ticket).unwrap();
    let s = r.snapshot().unwrap();
    assert_eq!(s.get("tasks", &a).unwrap().data["status"], "paused");
    assert!(s.get("task_runtime", &runtime).unwrap().data["last_active_clock"].is_null());
    assert!(!r.deleting_tasks.contains(&a));
}

#[test]
fn normal_close_records_real_retirement_proof_but_missing_agent_never_forges_a_context() {
    let (d, mut r, run, id, host) = seeded();
    let scope = r.native_scope().unwrap();
    r.native_close(&scope).unwrap();
    assert!(host.retired_binding.load(Ordering::SeqCst));
    r.engine_epoch = new_id("epoch");
    r.store = Some(Store::open(&d.path().join("store")).unwrap());
    let preview = r
        .lifecycle_preview(
            DeletionTarget::Conversations { ids: vec![id] },
            DeletionOptions::Conversations {
                action: ConversationAction::ClearHistory,
            },
        )
        .unwrap();
    assert_eq!(preview["cleanup_confirmed"], true);
    assert_eq!(preview["can_apply"], true);
    assert_eq!(
        r.snapshot().unwrap().get("agent_runs", &run).unwrap().data["state"],
        "interrupted"
    );
    let (_d, mut r, run, _id, _host) = seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.removed.agent", &json!({}));
    let mut data = s.get("agent_runs", &run).unwrap().data.clone();
    data["agent_id"] = Value::Null;
    p.put("agent_runs", &run, data);
    assert_eq!(
        r.internal_commit(p.done(json!({}))).unwrap_err().code,
        "INVALID_PARAMS"
    );
    let after = r.snapshot().unwrap();
    assert_eq!(after.commit_seq, s.commit_seq);
    assert_eq!(after.get("agent_runs", &run), s.get("agent_runs", &run));
    assert_eq!(
        after.records("command_receipts"),
        s.records("command_receipts")
    );
}

#[test]
fn actual_ui_control_task_queues_at_capacity_with_legal_status_and_admits_when_slot_frees() {
    let (_d, mut r, _run, _conversation, _host) = seeded();
    assert!(!r.native_automation_admitted);
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.capacity", &json!({}));
    p.insert(
        "settings",
        "setting",
        json!({"key":"task_concurrency_enabled","value":"true"}),
    )
    .unwrap();
    p.insert(
        "settings",
        "setting",
        json!({"key":"task_concurrency_limit","value":"1"}),
    )
    .unwrap();
    let ea = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"At capacity"}),
        )
        .unwrap();
    let eb = p
        .insert(
            "explorations",
            artex_core::collection_prefix("explorations").unwrap(),
            json!({"goal":"Waiting"}),
        )
        .unwrap();
    let running=p.insert("tasks","task",json!({"name":"Running","exploration_id":ea,"status":"running","paused":false,"queued":false})).unwrap();
    let waiting = p
        .insert(
            "tasks",
            "task",
            json!({"name":"Waiting","exploration_id":eb,"status":"created","paused":true}),
        )
        .unwrap();
    r.internal_commit(p.done(json!({}))).unwrap();
    let scope = r.native_scope().unwrap();
    let params = json!({"id":waiting,"action":"resume"});
    let prepared = r
        .native_query(
            &NativeTicket::new(),
            &scope,
            NativeQuery::Prepare {
                method: "ui.controlTask".into(),
                params: params.clone(),
            },
        )
        .unwrap();
    let NativeDispatch::Complete(prepared) = prepared else {
        panic!("Pure preparation must finish synchronously");
    };
    let expected = serde_json::from_value(prepared.value["expected"].clone()).unwrap();
    let command = NativeCommand::new("ui.controlTask", params, expected);
    let reply = r
        .native_command(&NativeTicket::new(), &scope, &command)
        .unwrap();
    let NativeDispatch::Complete(reply) = reply else {
        panic!("Pure task control must finish synchronously");
    };
    assert_eq!(reply.value["status"], "created");
    assert_eq!(reply.value["queued"], true);
    let s = r.snapshot().unwrap();
    assert_eq!(s.get("tasks", &waiting).unwrap().data["status"], "created");
    assert_eq!(s.get("tasks", &waiting).unwrap().data["queued"], true);
    assert!(
        r.native_automation_admitted,
        "Only accepted execution admission unlocks automation"
    );
    artex_core::validate_snapshot(&s).unwrap();
    let mut p = Planner::new(&s, "fixture.free.slot", &json!({}));
    let mut data = s.get("tasks", &running).unwrap().data.clone();
    data["status"] = json!("paused");
    data["paused"] = json!(true);
    p.put("tasks", &running, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    r.runtime_tick().unwrap();
    let s = r.snapshot().unwrap();
    assert_eq!(s.get("tasks", &waiting).unwrap().data["status"], "running");
    assert_eq!(s.get("tasks", &waiting).unwrap().data["queued"], false);
    assert!(
        s.records("tasks")
            .into_iter()
            .all(|t| t.data["status"] != "queued")
    );
    artex_core::validate_snapshot(&s).unwrap();
}
