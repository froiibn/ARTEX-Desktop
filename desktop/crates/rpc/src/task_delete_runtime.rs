//! Native task-file retirement is reversible until the graph deletion commits.
use crate::*;
use artex_domain::{Planner, text};
use std::time::Duration;
impl Router {
    #[cfg(test)]
    pub(super) fn handle_task_delete(&mut self, r: &DispatchRequest, c: &Context) -> Result<Value> {
        self.handle_lifecycle_legacy(r, c)
    }
    pub(super) fn complete_task_delete(
        &mut self,
        r: &DispatchRequest,
        outcome: &Value,
        cancelled: bool,
    ) -> Result<Value> {
        let task = artex_domain::required(&r.params, "id")?.to_owned();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        if outcome["compensated"] == true {
            self.deleting_tasks.remove(&task);
            self.stop_task_after_failed_delete(&task)?;
            let restored = outcome["restored"] == true;
            let mut error = Error::new(
                if restored {
                    outcome["error_code"]
                        .as_str()
                        .unwrap_or("RECOVERY_REQUIRED")
                } else {
                    "RECOVERY_REQUIRED"
                },
                if restored {
                    "Task deletion failed; files restored and task stopped"
                } else {
                    "Task deletion failed and file quarantine needs reconciliation"
                },
            );
            error.recovery_required = !restored;
            return Err(error);
        }
        let result = (|| {
            if cancelled || outcome["cancelled"] == true {
                return Err(Error::new(
                    "CANCELLED",
                    "Task deletion cancelled before commit",
                ));
            }
            let s = self.snapshot()?;
            let mut batch =
                artex_domain::task_delete_ui::plan("ui.deleteTask", &r.params, &s, &context)?;
            batch.result["files_deleted"] = json!(outcome["quarantined"] == true);
            self.commit(r, &mut batch)
        })();
        if let Err(original) = result {
            let input = json!({"task_id":task,"command_id":r.command_id,"directory_capability":self.execution_capability});
            let workspace = self.snapshot()?.workspace_id.clone();
            let gateway = self.gateway.clone();
            let compensation = self.defer_read_worker(r, move |_| {
                let restored = gateway.is_some_and(|g| {
                    g.request(
                        &workspace,
                        &context,
                        "workspace.restore_quarantine",
                        input,
                        Duration::from_secs(15),
                    )
                    .is_ok()
                });
                Ok(json!({"compensated":true,"restored":restored,"error_code":original.code}))
            });
            if compensation.is_err() {
                self.deleting_tasks.remove(&task);
                self.stop_task_after_failed_delete(&task)?;
                let mut error = Error::new(
                    "RECOVERY_REQUIRED",
                    "Task deletion compensation could not be scheduled",
                );
                error.recovery_required = true;
                return Err(error);
            }
            return compensation;
        }
        self.deleting_tasks.remove(&task);
        result
    }
    pub(super) fn task_delete_failed(&mut self, r: &DispatchRequest) -> Result<()> {
        let task = artex_domain::required(&r.params, "id")?.to_owned();
        self.deleting_tasks.remove(&task);
        self.stop_task_after_failed_delete(&task)
    }
    pub(super) fn stop_task_after_failed_delete(&mut self, task: &str) -> Result<()> {
        let s = self.snapshot()?;
        let Some(row) = s.get("tasks", task) else {
            return Ok(());
        };
        let mut b = Planner::new(&s, "task.delete.stopped", &json!({"task_id":task}));
        let mut d = row.data.clone();
        d["paused"] = json!(true);
        d["queued"] = json!(false);
        d["queued_at"] = Value::Null;
        b.put("tasks", task, d);
        let c = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: Some(task.into()),
            intent_id: None,
            conversation_id: None,
        };
        for run in s.records("agent_runs").into_iter().filter(|r| {
            r.data["task_id"] == task
                && matches!(
                    text(&r.data, "state"),
                    "queued" | "running" | "waiting" | "paused"
                )
        }) {
            artex_domain::conversation::cancel_run(&mut b, &c, &run.header.id)?;
        }
        self.internal_commit(b.done(json!({"stopped":true})))?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_runtime::HostGateway;
    use std::sync::Arc;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicBool, Ordering},
    };
    struct NativeFiles {
        live: PathBuf,
        retired: PathBuf,
        quarantined: AtomicBool,
        restoring: AtomicBool,
        release_restore: AtomicBool,
    }
    impl HostGateway for NativeFiles {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            _: Value,
            _: Duration,
        ) -> Result<Value> {
            match operation {
                "workspace.quarantine" => {
                    std::fs::rename(&self.live, &self.retired)?;
                    self.quarantined.store(true, Ordering::SeqCst);
                    Ok(json!({"quarantined":true}))
                }
                "workspace.restore_quarantine" => {
                    self.restoring.store(true, Ordering::SeqCst);
                    let end = Instant::now() + Duration::from_secs(5);
                    while !self.release_restore.load(Ordering::SeqCst) && Instant::now() < end {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    std::fs::rename(&self.retired, &self.live)?;
                    Ok(json!({"restored":true}))
                }
                "ui.stop" | "run.stop" => Ok(json!({"accepted":true})),
                _ => Err(Error::new(
                    "METHOD_NOT_FOUND",
                    "Fixture operation unsupported",
                )),
            }
        }
    }
    fn seeded() -> (
        tempfile::TempDir,
        Router,
        Context,
        DispatchRequest,
        Arc<NativeFiles>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("task-files");
        std::fs::create_dir(&live).unwrap();
        std::fs::write(live.join("evidence.txt"), b"original evidence").unwrap();
        let host = Arc::new(NativeFiles {
            live,
            retired: dir.path().join("retired"),
            quarantined: AtomicBool::new(false),
            restoring: AtomicBool::new(false),
            release_restore: AtomicBool::new(false),
        });
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&dir.path().join("store"), "Deletion fixture").unwrap());
        router.gateway = Some(host.clone());
        router.execution_capability = Some("fixture-cap".into());
        let s = router.snapshot().unwrap();
        let mut p = Planner::new(&s, "test.seed", &json!({}));
        let exploration = p
            .insert(
                "explorations",
                artex_core::collection_prefix("explorations").unwrap(),
                json!({"goal":"Fixture"}),
            )
            .unwrap();
        let task = p
            .insert(
                "tasks",
                "task",
                json!({"name":"Original","exploration_id":exploration,"paused":true}),
            )
            .unwrap();
        router.internal_commit(p.done(json!({}))).unwrap();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: router.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let params = json!({"id":task,"options":{"delete_assets":true,"delete_traffic":true,"delete_files":true,"delete_findings":true,"delete_llm_records":true}});
        let expected = artex_domain::task_delete_ui::plan(
            "ui.deleteTask",
            &params,
            &router.snapshot().unwrap(),
            &context,
        )
        .unwrap()
        .expected;
        let request: DispatchRequest = Request {
            jsonrpc: "2.0".into(),
            protocol: "artex-rpc/1.0".into(),
            id: new_id("request"),
            method: "ui.deleteTask".into(),
            workspace_id: Some(router.snapshot().unwrap().workspace_id.clone()),
            engine_epoch: Some(router.engine_epoch.clone()),
            command_id: Some(new_id("command")),
            expected,
            timeout_ms: 2000,
            params,
        }
        .into();
        (dir, router, context, request, host)
    }
    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < end, "Task deletion fixture timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn successful_native_retirement_and_graph_deletion_share_a_confirmed_receipt() {
        let (_d, mut router, context, request, host) = seeded();
        assert_eq!(
            router.handle_task_delete(&request, &context).unwrap()["__deferred"],
            true
        );
        wait_until(|| {
            router.runtime_tick().unwrap();
            !router
                .deleting_tasks
                .contains(request.params["id"].as_str().unwrap())
        });
        let responses = router.drain_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0]["result"]["files_deleted"], true,
            "{responses:?}"
        );
        assert!(
            router
                .snapshot()
                .unwrap()
                .get("tasks", request.params["id"].as_str().unwrap())
                .is_none()
        );
        assert!(!host.live.exists());
        assert_eq!(
            std::fs::read(host.retired.join("evidence.txt")).unwrap(),
            b"original evidence"
        );
        assert_eq!(
            router.repeated(&request).unwrap().unwrap()["files_deleted"],
            true
        );
    }
    #[test]
    fn cas_failure_restores_asynchronously_and_keeps_task_frozen_until_compensation() {
        let (_d, mut router, context, request, host) = seeded();
        router.handle_task_delete(&request, &context).unwrap();
        wait_until(|| host.quarantined.load(Ordering::SeqCst));
        let s = router.snapshot().unwrap();
        let id = request.params["id"].as_str().unwrap();
        let mut p = Planner::new(&s, "test.concurrent_update", &json!({}));
        let mut data = s.get("tasks", id).unwrap().data.clone();
        data["name"] = json!("Concurrent update");
        p.put("tasks", id, data);
        router.internal_commit(p.done(json!({}))).unwrap();
        let started = Instant::now();
        wait_until(|| {
            router.runtime_tick().unwrap();
            host.restoring.load(Ordering::SeqCst)
        });
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "Writer waited for native compensation"
        );
        assert!(router.deleting_tasks.contains(id));
        assert!(router.drain_responses().is_empty());
        assert_eq!(
            router.snapshot().unwrap().get("tasks", id).unwrap().data["name"],
            "Concurrent update"
        );
        host.release_restore.store(true, Ordering::SeqCst);
        wait_until(|| {
            router.runtime_tick().unwrap();
            !router.deleting_tasks.contains(id)
        });
        let responses = router.drain_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0]["error"]["data"]["code"], "VERSION_CONFLICT",
            "{responses:?}"
        );
        assert!(host.live.join("evidence.txt").is_file());
        assert_eq!(
            router.snapshot().unwrap().get("tasks", id).unwrap().data["name"],
            "Concurrent update"
        );
        assert_eq!(
            router.snapshot().unwrap().get("tasks", id).unwrap().data["paused"],
            true
        );
    }
    #[test]
    fn cancelled_deletion_restores_files_without_late_success_or_graph_removal() {
        let (_d, mut router, context, request, host) = seeded();
        router.handle_task_delete(&request, &context).unwrap();
        wait_until(|| host.quarantined.load(Ordering::SeqCst));
        router.cancel_request(&request.id);
        assert_eq!(router.drain_responses().len(), 1);
        wait_until(|| {
            router.runtime_tick().unwrap();
            host.restoring.load(Ordering::SeqCst)
        });
        let id = request.params["id"].as_str().unwrap();
        assert!(router.deleting_tasks.contains(id));
        host.release_restore.store(true, Ordering::SeqCst);
        wait_until(|| {
            router.runtime_tick().unwrap();
            !router.deleting_tasks.contains(id)
        });
        assert!(router.drain_responses().is_empty());
        assert!(router.snapshot().unwrap().get("tasks", id).is_some());
        assert!(host.live.join("evidence.txt").is_file());
        assert!(router.repeated(&request).unwrap().is_none());
    }
}
