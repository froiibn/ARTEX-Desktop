//! Native archive workflows. A verified package precedes every hot-data removal.
use crate::*;
use artex_core::Record;
use artex_domain::{Planner, record, text};
use artex_importer::{LegacyOptions, SecretOutcome, SecretSink};
use std::collections::BTreeSet;
use std::{fs, sync::Arc, time::Duration};

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
pub(super) struct PendingArchive {
    archive_id: String,
    operation: String,
    workspace: String,
    epoch: String,
    cancel: Arc<AtomicBool>,
    stage: Option<ArchiveStage>,
}
impl Drop for PendingArchive {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
enum ArchiveStage {
    Worker(mpsc::Receiver<Result<ArchivePrepared>>),
    Restore(artex_importer::ImportSession),
}
enum ArchivePrepared {
    Package(ReadyPackage),
    Restore(artex_importer::ImportSession),
}
struct ReadyPackage {
    package: Value,
    manifest: Value,
    file_size: u64,
    original_size: usize,
    counts: BTreeMap<String, usize>,
    owned_versions: BTreeMap<(String, String), String>,
}
fn task_owned_versions(s: &Snapshot, task_id: &str) -> Result<BTreeMap<(String, String), String>> {
    let task = record(s, "tasks", task_id)?;
    let exp = &task.data["exploration_id"];
    let mut owned = BTreeSet::new();
    for (c, rs) in &s.collections {
        if c == "task_archives" || c == "command_receipts" {
            continue;
        }
        for (id, r) in rs {
            if r.data["task_id"] == task_id
                || r.data["exploration_id"] == *exp
                || c == "tasks" && id == task_id
                || c == "explorations" && r.data["id"] == *exp
            {
                owned.insert((c.clone(), id.clone()));
            }
        }
    }
    loop {
        let n = owned.len();
        for fk in artex_core::foreign_keys() {
            if fk.collection == "task_archives" {
                continue;
            }
            for r in s.records(&fk.collection) {
                if fk.columns.iter().all(|c| !r.data[c].is_null())
                    && s.records(&fk.target).iter().any(|t| {
                        owned.contains(&(fk.target.clone(), t.header.id.clone()))
                            && fk
                                .columns
                                .iter()
                                .zip(&fk.target_columns)
                                .all(|(a, b)| r.data[a] == t.data[b])
                    })
                {
                    owned.insert((fk.collection.clone(), r.header.id.clone()));
                }
            }
        }
        if owned.len() == n {
            break;
        }
    }
    Ok(owned
        .into_iter()
        .map(|(c, id)| {
            let version = s.get(&c, &id).unwrap().header.version.clone();
            ((c, id), version)
        })
        .collect())
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::new("CANCELLED", "Archive operation cancelled"))
    } else {
        Ok(())
    }
}
fn prepare_archive_export(
    mut view: artex_store::StoreReadView,
    task_id: String,
    archive_id: String,
    temporary: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
) -> Result<ArchivePrepared> {
    cancelled(&cancel)?;
    let owned_versions = task_owned_versions(&view.read_generation(), &task_id)?;
    let manifest = artex_importer::export_task_archive_cancellable(
        &view,
        &task_id,
        &temporary,
        cancel.clone(),
    )?;
    cancelled(&cancel)?;
    let source = artex_importer::read_archive(&temporary, &format!("archive-verify:{archive_id}"))?;
    cancelled(&cancel)?;
    let file_size = fs::metadata(&temporary)?.len();
    if file_size > 512 * 1024 * 1024 {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Archive package exceeds importer boundary",
        ));
    }
    let package = view.blob_put_file_cancellable(&temporary, "application/zip", &cancel)?;
    let original_size = serde_json::to_vec(&source.tables)?.len();
    let counts = source
        .tables
        .iter()
        .map(|(c, r)| (c.clone(), r.len()))
        .collect();
    let _ = fs::remove_file(temporary);
    Ok(ArchivePrepared::Package(ReadyPackage {
        package,
        manifest,
        file_size,
        original_size,
        counts,
        owned_versions,
    }))
}
struct VaultSink {
    gateway: Option<Arc<dyn artex_runtime::HostGateway>>,
    workspace: String,
    context: Context,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed() -> (tempfile::TempDir, Router, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&dir.path().join("w"), "archive").unwrap());
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
        let s = router.snapshot().unwrap();
        let batch = artex_domain::tasks::plan(
            "task.create",
            &json!({"name":"preserved task","description":"original","goal":"evidence"}),
            &s,
            &context,
        )
        .unwrap();
        router.internal_commit(batch).unwrap();
        let s = router.snapshot().unwrap();
        let task = s.records("tasks")[0].clone();
        let mut planner = Planner::new(&s, "fixture.paused", &json!({}));
        let mut d = task.data.clone();
        d["paused"] = json!(true);
        planner.put("tasks", &task.header.id, d);
        let node=planner.insert("exploration_nodes","node",json!({"exploration_id":task.data["exploration_id"],"kind":"fact","state":"origin","name":"durable fact","payload":{"text":"saved"}})).unwrap();
        router.internal_commit(planner.done(json!({}))).unwrap();
        (dir, router, task.header.id, node)
    }
    fn drain(router: &mut Router) {
        for _ in 0..1000 {
            router.process_archive_jobs().unwrap();
            let s = router.snapshot().unwrap();
            if router.archive_work.is_none()
                && !s.records("task_archives").iter().any(|r| {
                    matches!(
                        text(&r.data, "state"),
                        "archive_queued"
                            | "archiving"
                            | "restore_queued"
                            | "restoring"
                            | "delete_queued"
                            | "deleting"
                    )
                })
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("archive worker timed out");
    }
    #[test]
    fn archive_verified_package_compacts_and_restores_tombstoned_graph() {
        let (_d, mut router, task, node) = seed();
        let s = router.snapshot().unwrap();
        let c = Context {
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
        let batch = artex_domain::auxiliary_ui::plan("ui.archiveTask", &json!({"id":task}), &s, &c)
            .unwrap();
        let archive = batch.result["id"].as_str().unwrap().to_owned();
        router.internal_commit(batch).unwrap();
        drain(&mut router);
        let s = router.snapshot().unwrap();
        assert_eq!(
            s.get("task_archives", &archive).unwrap().data["state"],
            "ready",
            "{:?}",
            s.get("task_archives", &archive).unwrap().data
        );
        assert!(s.get("exploration_nodes", &node).is_none());
        assert!(!s.get("tasks", &task).unwrap().data["archived_at"].is_null());
        let batch = artex_domain::auxiliary_ui::plan(
            "ui.restoreTaskArchive",
            &json!({"id":archive}),
            &s,
            &c,
        )
        .unwrap();
        router.internal_commit(batch).unwrap();
        drain(&mut router);
        let s = router.snapshot().unwrap();
        assert!(
            s.get("task_archives", &archive).is_none(),
            "{:?}",
            s.get("task_archives", &archive).map(|r| &r.data)
        );
        assert_eq!(
            s.get("tasks", &task).unwrap().data["name"],
            "preserved task"
        );
        assert!(s.get("tasks", &task).unwrap().data["archived_at"].is_null());
        let restored = s.records("exploration_nodes");
        assert_eq!(restored.len(), 3);
        let fact = restored
            .iter()
            .find(|n| n.data["payload"]["text"] == "saved")
            .unwrap();
        assert_ne!(fact.header.id, node);
        assert_eq!(fact.data["payload"]["text"], "saved", "{:?}", fact.data);
    }
    #[test]
    fn restoring_archive_preserves_current_globals_and_does_not_resurrect_deleted_company() {
        let (_d, mut router, task, _node) = seed();
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
        let s = router.snapshot().unwrap();
        let mut planner = Planner::new(&s, "fixture.company", &json!({}));
        let company = planner
            .insert(
                "companies",
                "company",
                json!({"name":"Deleted original company"}),
            )
            .unwrap();
        let category = planner
            .insert(
                "task_categories",
                "category",
                json!({"name":"Original category"}),
            )
            .unwrap();
        let mut data = s.get("tasks", &task).unwrap().data.clone();
        data["company_id"] = json!(company);
        data["category_id"] = json!(category);
        planner.put("tasks", &task, data);
        router.internal_commit(planner.done(json!({}))).unwrap();
        let b = artex_domain::auxiliary_ui::plan(
            "ui.archiveTask",
            &json!({"id":task}),
            &router.snapshot().unwrap(),
            &context,
        )
        .unwrap();
        let archive = b.result["id"].as_str().unwrap().to_owned();
        router.internal_commit(b).unwrap();
        drain(&mut router);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("task_archives", &archive)
                .unwrap()
                .data["state"],
            "ready"
        );
        let s = router.snapshot().unwrap();
        let mut planner = Planner::new(&s, "fixture.globals_changed", &json!({}));
        planner.delete("companies", &company);
        let mut category_data = s.get("task_categories", &category).unwrap().data.clone();
        category_data["name"] = json!("Updated current category");
        planner.put("task_categories", &category, category_data);
        router.internal_commit(planner.done(json!({}))).unwrap();
        let b = artex_domain::auxiliary_ui::plan(
            "ui.restoreTaskArchive",
            &json!({"id":archive}),
            &router.snapshot().unwrap(),
            &context,
        )
        .unwrap();
        router.internal_commit(b).unwrap();
        drain(&mut router);
        let s = router.snapshot().unwrap();
        assert!(
            s.get("task_archives", &archive).is_none(),
            "{:?}",
            s.get("task_archives", &archive).map(|a| &a.data)
        );
        assert!(s.get("companies", &company).is_none());
        assert!(s.records("companies").is_empty());
        assert!(s.get("tasks", &task).unwrap().data["company_id"].is_null());
        assert_eq!(s.get("tasks", &task).unwrap().data["category_id"], category);
        assert_eq!(
            s.get("task_categories", &category).unwrap().data["name"],
            "Updated current category"
        );
    }
    #[test]
    fn missing_package_does_not_remove_stub_or_archive() {
        let (_d, mut router, task, _node) = seed();
        let s = router.snapshot().unwrap();
        let mut planner = Planner::new(&s, "fixture.archive", &json!({}));
        let id=planner.insert("task_archives","archive",json!({"task_id":task,"state":"restore_queued","sha256":"a".repeat(64),"archive_path":format!("blob:{}","a".repeat(64))})).unwrap();
        router.internal_commit(planner.done(json!({}))).unwrap();
        drain(&mut router);
        let s = router.snapshot().unwrap();
        assert_eq!(
            s.get("task_archives", &id).unwrap().data["state"],
            "restore_failed"
        );
        assert!(s.get("tasks", &task).is_some());
    }
    #[test]
    fn compression_yields_to_writer_and_rejects_changed_hot_data() {
        let (_d, mut router, task, node) = seed();
        let s = router.snapshot().unwrap();
        let c = Context {
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
        let b = artex_domain::auxiliary_ui::plan("ui.archiveTask", &json!({"id":task}), &s, &c)
            .unwrap();
        let archive = b.result["id"].as_str().unwrap().to_owned();
        router.internal_commit(b).unwrap();
        router.process_archive_jobs().unwrap();
        assert!(router.archive_work.is_some());
        let s = router.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.concurrent", &json!({}));
        let mut d = record(&s, "exploration_nodes", &node).unwrap().data.clone();
        d["payload"]["text"] = json!("updated while compression runs");
        p.put("exploration_nodes", &node, d);
        router.internal_commit(p.done(json!({}))).unwrap();
        drain(&mut router);
        let s = router.snapshot().unwrap();
        assert_eq!(
            s.get("task_archives", &archive).unwrap().data["state"],
            "archive_failed"
        );
        assert_eq!(
            record(&s, "exploration_nodes", &node).unwrap().data["payload"]["text"],
            "updated while compression runs"
        );
        assert!(record(&s, "tasks", &task).unwrap().data["archived_at"].is_null());
    }
    #[test]
    fn native_gc_rejects_short_grace_and_returns_durable_real_removed_count() {
        let (_d, mut router, _task, _node) = seed();
        let c = Context {
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
        let command = new_id("command");
        let mut r: DispatchRequest = Request {
            jsonrpc: "2.0".into(),
            protocol: "artex-rpc/1.0".into(),
            id: "gc".into(),
            method: "ui.gc".into(),
            workspace_id: Some(router.snapshot().unwrap().workspace_id.clone()),
            engine_epoch: Some(router.engine_epoch.clone()),
            command_id: Some(command),
            expected: vec![],
            timeout_ms: 1000,
            params: json!({"ttl":0}),
        }
        .into();
        assert_eq!(router.handle_gc(&r, &c).unwrap_err().code, "INVALID_PARAMS");
        r.params = json!({});
        let result = router.handle_gc(&r, &c).unwrap();
        assert_eq!(result, json!({"removed":0}));
        assert_eq!(router.handle_gc(&r, &c).unwrap(), result);
    }
}
impl SecretSink for VaultSink {
    fn reference_available(&mut self, reference: &Value) -> Result<bool> {
        let Some(gateway) = &self.gateway else {
            return Ok(false);
        };
        let result = gateway.request(
            &self.workspace,
            &self.context,
            "secret.available",
            json!({"secret_ref":reference["secret_ref"]}),
            Duration::from_secs(5),
        )?;
        Ok(result["available"] == true)
    }
    fn import_secret(&mut self, key: &str, value: &str) -> Result<SecretOutcome> {
        if let Some(gateway) = &self.gateway {
            let result = gateway.request(
                &self.workspace,
                &self.context,
                "secret.import",
                json!({"stable_key":key,"value":value}),
                Duration::from_secs(30),
            )?;
            return Ok(SecretOutcome {
                reference: result["reference"].clone(),
                available: result["available"] == true,
            });
        }
        artex_importer::PendingSecrets.import_secret(key, value)
    }
}
impl Router {
    pub(super) fn handle_archive(&mut self, r: &DispatchRequest, c: &Context) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Archive management requires the native user",
            ));
        }
        if artex_domain::auxiliary_ui::QUERIES.contains(&r.method.as_str()) {
            return artex_domain::auxiliary_ui::query(
                &r.method,
                &r.params,
                self.snapshot()?.as_ref(),
                c,
            );
        }
        self.ensure_writable()?;
        self.require_command(r)?;
        if let Some(result) = self.repeated(r)? {
            return Ok(result);
        }
        let mut batch =
            artex_domain::auxiliary_ui::plan(&r.method, &r.params, self.snapshot()?.as_ref(), c)?;
        self.commit(r, &mut batch)
    }
    pub(super) fn handle_gc(&mut self, r: &DispatchRequest, c: &Context) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Garbage collection requires native user",
            ));
        }
        self.ensure_writable()?;
        self.require_command(r)?;
        if let Some(result) = self.repeated(r)? {
            return Ok(result);
        }
        let grace = artex_domain::auxiliary_ui::gc_grace(&r.params)?;
        let report = self
            .store
            .as_mut()
            .unwrap()
            .maintenance(artex_store::GcOptions {
                orphan_grace: grace,
                ..Default::default()
            })?;
        let mut batch =
            artex_domain::auxiliary_ui::plan("ui.gc", &r.params, self.snapshot()?.as_ref(), c)?;
        batch.result = json!({"removed":report.removed_count});
        self.commit(r, &mut batch)
    }
    pub(super) fn process_archive_jobs(&mut self) -> Result<()> {
        if self.read_only || self.store.is_none() {
            self.archive_work = None;
            return Ok(());
        }
        if let Some(mut work) = self.archive_work.take() {
            if work.workspace != self.snapshot()?.workspace_id || work.epoch != self.engine_epoch {
                return Ok(());
            }
            let result = match work.stage.take().unwrap() {
                ArchiveStage::Worker(receiver) => match receiver.try_recv() {
                    Ok(Ok(ArchivePrepared::Package(package))) => {
                        self.complete_task_package(&work.archive_id, package)
                    }
                    Ok(Ok(ArchivePrepared::Restore(session))) => {
                        work.stage = Some(ArchiveStage::Restore(session));
                        self.archive_work = Some(work);
                        return Ok(());
                    }
                    Ok(Err(error)) => Err(error),
                    Err(mpsc::TryRecvError::Empty) => {
                        work.stage = Some(ArchiveStage::Worker(receiver));
                        self.archive_work = Some(work);
                        return Ok(());
                    }
                    Err(_) => Err(Error::new("EXTERNAL_FAILURE", "Archive worker terminated")),
                },
                ArchiveStage::Restore(mut session) => {
                    match session.step(self.store.as_mut().unwrap()) {
                        Ok(true) => self.finish_task_restore(&work.archive_id),
                        Ok(false) => {
                            work.stage = Some(ArchiveStage::Restore(session));
                            self.archive_work = Some(work);
                            return Ok(());
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            if let Err(error) = result {
                self.fail_archive(&work.archive_id, &work.operation, error)?;
            }
            return Ok(());
        }
        let s = self.snapshot()?;
        let job = s
            .records("task_archives")
            .into_iter()
            .find(|r| {
                matches!(
                    text(&r.data, "state"),
                    "archive_queued"
                        | "archiving"
                        | "restore_queued"
                        | "restoring"
                        | "delete_queued"
                        | "deleting"
                )
            })
            .cloned();
        let Some(job) = job else { return Ok(()) };
        let id = job.header.id.clone();
        let operation = match text(&job.data, "state") {
            "archive_queued" | "archiving" => "archive",
            "restore_queued" | "restoring" => "restore",
            _ => "delete",
        };
        let mut planner = Planner::new(
            &s,
            "archive.start",
            &json!({"archive_id":id,"operation":operation}),
        );
        let mut d = job.data.clone();
        d["state"] = json!("deleting");
        if operation == "archive" {
            d["state"] = json!("archiving");
        }
        if operation == "restore" {
            d["state"] = json!("restoring");
        }
        d["phase"] = json!("verifying");
        d["progress"] = json!(5);
        planner.put("task_archives", &id, d);
        self.internal_commit(planner.done(json!({"started":true})))?;
        if operation == "delete" {
            if let Err(error) = self.delete_task_package(&id) {
                self.fail_archive(&id, operation, error)?;
            }
            return Ok(());
        }
        let store = self.store.as_ref().unwrap();
        let view = store.read_view();
        let temporary = store.temporary_path("zip")?;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let worker_id = id.clone();
        let task_id = text(&job.data, "task_id").to_owned();
        let gateway = self.gateway.clone();
        let epoch = self.engine_epoch.clone();
        let workspace = s.workspace_id.clone();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let secrets = VaultSink {
            gateway,
            workspace: workspace.clone(),
            context,
        };
        let restore = operation == "restore";
        std::thread::spawn(move || {
            let result = if restore {
                prepare_task_restore(view, job, temporary, secrets, worker_cancel.clone())
            } else {
                prepare_archive_export(view, task_id, worker_id, temporary, worker_cancel.clone())
            };
            if !worker_cancel.load(Ordering::Acquire) {
                let _ = tx.send(result);
            }
        });
        self.archive_work = Some(PendingArchive {
            archive_id: id,
            operation: operation.into(),
            workspace,
            epoch,
            cancel,
            stage: Some(ArchiveStage::Worker(rx)),
        });
        Ok(())
    }
    fn fail_archive(&mut self, id: &str, operation: &str, error: Error) -> Result<()> {
        let s = self.snapshot()?;
        if let Some(job) = s.get("task_archives", id) {
            let mut d = job.data.clone();
            d["state"] = json!(format!("{operation}_failed"));
            d["phase"] = json!("failed");
            d["error"] = json!(format!("Archive operation failed ({})", error.code));
            let mut planner = Planner::new(
                &s,
                "archive.failed",
                &json!({"archive_id":id,"code":error.code}),
            );
            planner.put("task_archives", id, d);
            self.internal_commit(planner.done(json!({"failed":true})))?;
        }
        Ok(())
    }
    fn complete_task_package(&mut self, archive_id: &str, ready: ReadyPackage) -> Result<()> {
        let s = self.snapshot()?;
        let archive = record(&s, "task_archives", archive_id)?;
        let task_id = text(&archive.data, "task_id").to_owned();
        if task_owned_versions(&s, &task_id)? != ready.owned_versions {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Task changed while its archive was being verified",
            ));
        }
        if s.records("task_relations").iter().any(|r| {
            r.data["source_task_id"] == task_id
                && s.get("tasks", text(&r.data, "task_id"))
                    .is_some_and(|t| t.data["deleted_at"].is_null())
        }) {
            return Err(Error::new("BUSY", "Live task depends on archive source"));
        }
        let ReadyPackage {
            package,
            manifest,
            file_size,
            original_size,
            counts,
            ..
        } = ready;
        let s = self.snapshot()?;
        let task = record(&s, "tasks", &task_id)?;
        let exploration = text(&task.data, "exploration_id");
        let mut planner = Planner::new(
            &s,
            "archive.complete",
            &json!({"archive_id":archive_id,"package":package}),
        );
        for run in s.records("agent_runs").into_iter().filter(|r| {
            r.data["task_id"] == task_id
                && matches!(
                    text(&r.data, "state"),
                    "queued" | "running" | "waiting" | "paused"
                )
        }) {
            if run.data["state"] != "paused" {
                return Err(Error::new("BUSY", "Archive task still owns an active run"));
            }
            let mut d = run.data.clone();
            d["state"] = json!("interrupted");
            d["finished_at"] = json!(artex_core::now());
            planner.put("agent_runs", &run.header.id, d);
            for invocation in s.records("tool_invocations").into_iter().filter(|i| {
                i.data["run_id"] == run.header.id
                    && matches!(
                        text(&i.data, "state"),
                        "prepared" | "running" | "awaiting_approval"
                    )
            }) {
                let mut d = invocation.data.clone();
                d["state"] = json!("recovery_required");
                planner.put("tool_invocations", &invocation.header.id, d);
            }
        }
        for collection in [
            "intercept_pending",
            "llm_records",
            "llm_usage",
            "skill_usage",
            "tool_usage",
            "side_question_sessions",
            "findings",
            "task_relations",
            "task_asset_links",
            "task_llm_profiles",
            "task_scope",
            "task_constraints",
            "activity",
            "exploration_nodes",
            "traffic_exchanges",
            "traffic_evidence_snapshots",
        ] {
            for row in s.records(collection) {
                if row.data["task_id"] == task_id
                    || row.data["exploration_id"] == exploration
                    || collection == "task_relations" && row.data["source_task_id"] == task_id
                {
                    planner.delete(collection, &row.header.id);
                }
            }
        }
        // Preserve company assets and any asset referenced by another live task.
        for asset in s.records("assets") {
            let memberships = s.records("task_asset_links");
            let target_member = memberships
                .iter()
                .any(|l| l.data["asset_id"] == asset.header.id && l.data["task_id"] == task_id);
            let shared = memberships.iter().any(|l| {
                l.data["asset_id"] == asset.header.id
                    && l.data["task_id"] != task_id
                    && s.get("tasks", text(&l.data, "task_id"))
                        .is_some_and(|t| t.data["deleted_at"].is_null())
            });
            let external_anchor = s.records("exploration_anchors").iter().any(|a| {
                a.data["asset_id"] == asset.header.id
                    && s.get("exploration_nodes", text(&a.data, "node_id"))
                        .is_some_and(|n| n.data["exploration_id"] != exploration)
            });
            if target_member && !shared && !external_anchor && asset.data["company_id"].is_null() {
                planner.delete("assets", &asset.header.id);
            } else if asset.data["task_ids"]
                .as_array()
                .is_some_and(|a| a.iter().any(|t| t == &json!(task_id)))
            {
                let mut d = asset.data.clone();
                d["task_ids"] = json!(
                    d["task_ids"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|t| t.as_str() != Some(&task_id))
                        .cloned()
                        .collect::<Vec<_>>()
                );
                planner.put("assets", &asset.header.id, d);
            }
        }
        let mut task_data = task.data.clone();
        for k in ["name", "description", "goal", "queue_mode"] {
            task_data[k] = json!("");
        }
        for k in [
            "category_id",
            "llm_profile_id",
            "active_llm_profile_id",
            "company_id",
            "parent_ref",
            "pinned_at",
            "first_run_at",
            "deadline_at",
            "queued_at",
        ] {
            task_data[k] = Value::Null;
        }
        task_data["paused"] = json!(true);
        task_data["queued"] = json!(false);
        task_data["timeout_seconds"] = json!(0);
        task_data["archived_at"] = json!(artex_core::now());
        task_data["deleted_at"] = task_data["archived_at"].clone();
        planner.put("tasks", &task_id, task_data);
        let mut exploration_data = record(&s, "explorations", exploration)?.data.clone();
        exploration_data["goal"] = json!("");
        exploration_data["description"] = json!("");
        exploration_data["status"] = json!("open");
        planner.put("explorations", exploration, exploration_data);
        let mut d = record(&s, "task_archives", archive_id)?.data.clone();
        d["state"] = json!("ready");
        d["phase"] = json!("ready");
        d["progress"] = json!(100);
        d["error"] = json!("");
        d["archive_path"] = json!(format!("blob:{}", package["sha256"].as_str().unwrap()));
        d["sha256"] = package["sha256"].clone();
        d["compressed_size"] = json!(file_size.to_string());
        d["original_size"] = json!(original_size.to_string());
        d["archived_at"] = json!(artex_core::now());
        d["data_counts"] = json!(counts);
        d["aggregate_stats"] = json!({"tables":manifest["tables"].as_array().unwrap().len()});
        planner.put("task_archives", archive_id, d);
        self.internal_commit(planner.done(json!({"ready":true})))?;
        Ok(())
    }
    fn finish_task_restore(&mut self, archive_id: &str) -> Result<()> {
        let s = self.snapshot()?;
        let task_id = text(&record(&s, "task_archives", archive_id)?.data, "task_id").to_owned();
        let mut planner = Planner::new(&s, "archive.restored", &json!({"archive_id":archive_id}));
        let mut task = record(&s, "tasks", &task_id)?.data.clone();
        task["archived_at"] = Value::Null;
        task["deleted_at"] = Value::Null;
        planner.put("tasks", &task_id, task);
        planner.delete("task_archives", archive_id);
        self.internal_commit(planner.done(json!({"restored":true})))?;
        Ok(())
    }
    fn delete_task_package(&mut self, archive_id: &str) -> Result<()> {
        let s = self.snapshot()?;
        let archive = record(&s, "task_archives", archive_id)?;
        let task_id = text(&archive.data, "task_id");
        if s.records("task_archives").iter().any(|a| {
            a.header.id != archive_id
                && a.data["source_task_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(task_id)))
        }) {
            return Err(Error::new(
                "BUSY",
                "Another archive depends on this source task",
            ));
        }
        let task = record(&s, "tasks", task_id)?;
        let mut planner = Planner::new(&s, "archive.deleted", &json!({"archive_id":archive_id}));
        planner.delete("task_archives", archive_id);
        planner.delete("tasks", task_id);
        planner.delete("explorations", text(&task.data, "exploration_id"));
        self.internal_commit(planner.done(json!({"deleted":true})))?;
        Ok(())
    }
}

fn prepare_task_restore(
    mut view: artex_store::StoreReadView,
    archive: Record,
    temporary: std::path::PathBuf,
    mut secrets: VaultSink,
    cancel: Arc<AtomicBool>,
) -> Result<ArchivePrepared> {
    cancelled(&cancel)?;
    let s = view.read_generation();
    let archive_id = &archive.header.id;
    let task_id = text(&archive.data, "task_id").to_owned();
    let hash = text(&archive.data, "sha256");
    if text(&archive.data, "archive_path") != format!("blob:{hash}") {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "Archive package is unavailable",
        ));
    }
    let reference = view
        .existing_blob_ref(hash, "application/zip")?
        .ok_or_else(|| Error::new("RECOVERY_REQUIRED", "Archive package missing"))?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    view.copy_blob_to(&reference, &mut file)?;
    file.sync_all()?;
    drop(file);
    cancelled(&cancel)?;
    let mut source = artex_importer::read_archive(
        &temporary,
        &format!("{}:archive:{archive_id}", s.workspace_id),
    )?;
    cancelled(&cancel)?;
    let mut options = LegacyOptions::default();
    for (collection, rows) in &source.tables {
        for row in rows {
            let key = artex_importer::source_primary_key(collection, row)?;
            let existing = row["id"]
                .as_str()
                .and_then(|id| s.get(collection, id))
                .or_else(|| {
                    s.records(collection).into_iter().find(|r| {
                        artex_importer::source_primary_key(collection, &r.data)
                            .ok()
                            .as_ref()
                            == Some(&key)
                            || r.header.legacy_key == key
                            || r.header.legacy_key == json!(key.to_string())
                    })
                });
            if let Some(existing) = existing {
                options.id_bindings.insert(
                    (collection.clone(), key.to_string()),
                    existing.header.id.clone(),
                );
                let owned = row["task_id"] == task_id
                    || collection == "tasks" && row["id"] == task_id
                    || collection == "explorations"
                        && row["id"] == record(&s, "tasks", &task_id)?.data["exploration_id"];
                if !owned {
                    options
                        .keep_existing
                        .insert((collection.clone(), existing.header.id.clone()));
                }
            }
        }
    }
    // Original restore does not recreate deleted global companies, categories
    // or LLM profiles. Nullable historical references are cleared; required
    // membership links to missing globals are excluded. The sealed package
    // retains original provenance even when a global is no longer present.
    let globals = BTreeSet::from(["companies", "task_categories", "llm_profiles"]);
    let mut absent = BTreeMap::<String, BTreeSet<String>>::new();
    for global in &globals {
        if let Some(rows) = source.tables.get_mut(*global) {
            rows.retain(|row| {
                let key = artex_importer::source_primary_key(global, row)
                    .ok()
                    .map(|v| v.to_string());
                let exists = key.as_ref().is_some_and(|key| {
                    options
                        .id_bindings
                        .contains_key(&((*global).to_owned(), key.clone()))
                });
                if !exists {
                    if let Some(key) = key {
                        absent.entry((*global).into()).or_default().insert(key);
                    }
                }
                exists
            });
        }
    }
    for (collection, rows) in &mut source.tables {
        if globals.contains(collection.as_str()) {
            continue;
        }
        let required = artex_core::foreign_keys()
            .iter()
            .filter(|fk| fk.collection == *collection && globals.contains(fk.target.as_str()))
            .collect::<Vec<_>>();
        rows.retain(|row| {
            !required.iter().any(|fk| {
                fk.columns.iter().zip(&fk.target_columns).any(|(field, _)| {
                    let nullable = artex_core::data_model()["tables"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|t| t["name"] == collection.as_str())
                        .unwrap()["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["name"] == field.as_str())
                        .is_some_and(|c| c["nullable"] == true);
                    !nullable
                        && absent
                            .get(&fk.target)
                            .is_some_and(|keys| keys.contains(&row[field].to_string()))
                })
            })
        });
        for row in rows {
            for fk in &required {
                for field in &fk.columns {
                    if absent
                        .get(&fk.target)
                        .is_some_and(|keys| keys.contains(&row[field].to_string()))
                    {
                        row[field] = Value::Null;
                    }
                }
            }
        }
    }
    for task in source
        .tables
        .get_mut("tasks")
        .into_iter()
        .flatten()
        .filter(|t| t["id"] == task_id)
    {
        task["archived_at"] = record(&s, "tasks", &task_id)?.data["archived_at"].clone();
        task["deleted_at"] = record(&s, "tasks", &task_id)?.data["deleted_at"].clone();
        task["paused"] = json!(true);
        task["queued"] = json!(false);
        task["queued_at"] = Value::Null;
    }
    // Approvals are never resurrected as executable authority.
    for pending in source
        .tables
        .get_mut("intercept_pending")
        .into_iter()
        .flatten()
        .filter(|p| p["status"] == "pending")
    {
        pending["status"] = json!("timeout");
        pending["reason"] = json!("Approval timed out during task archiving");
        pending["decided_at"] = json!(artex_core::now());
    }
    let session =
        artex_importer::ImportSession::prepare(&mut view, &source, &mut secrets, options)?;
    cancelled(&cancel)?;
    let _ = fs::remove_file(temporary);
    Ok(ArchivePrepared::Restore(session))
}
