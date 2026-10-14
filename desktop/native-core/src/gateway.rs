//! Native host calls share the application process. No engine client or framing.
use artex_core::{Context, Error, Result, valid_id};
use artex_host::{HostBridge, SecretVault};
use artex_rpc::native::NativeWorkspace;
use artex_runtime::HostGateway;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

const MAX_ACTIVE: usize = 32;
const MAX_CONTROLS: usize = 4;
const MAX_OBSERVERS: usize = 4;
#[derive(Clone, Copy)]
enum RequestClass {
    Work,
    Control,
    Observer,
}
impl RequestClass {
    fn for_operation(operation: &str) -> Self {
        match operation {
            "run.stop" | "run.force_stop" | "ui.stop" => Self::Control,
            "run.inspect" | "scope.inspect" | "host.events" => Self::Observer,
            _ => Self::Work,
        }
    }
    fn limit(self) -> usize {
        match self {
            Self::Work => MAX_ACTIVE,
            Self::Control => MAX_CONTROLS,
            Self::Observer => MAX_OBSERVERS,
        }
    }
    fn count(self, admission: &mut Admission) -> &mut usize {
        match self {
            Self::Work => &mut admission.work_active,
            Self::Control => &mut admission.control_active,
            Self::Observer => &mut admission.observer_active,
        }
    }
}
struct Admission {
    closing: bool,
    active: usize,
    work_active: usize,
    control_active: usize,
    observer_active: usize,
    stop_started: bool,
    stop_complete: bool,
    run_active: std::collections::HashMap<String, usize>,
}
impl Admission {
    fn admit(&mut self, class: RequestClass, run: Option<&str>) -> Result<()> {
        if self.closing {
            return Err(Error::new("CANCELLED", "Native workspace is retiring"));
        }
        if *class.count(self) >= class.limit() {
            return Err(Error::new("BUSY", "Native host request lane limit reached"));
        }
        *class.count(self) += 1;
        self.active += 1;
        if let Some(run) = run {
            *self.run_active.entry(run.into()).or_default() += 1;
        }
        Ok(())
    }
}
struct Inner {
    bridge: HostBridge,
    workspace: NativeWorkspace,
    admission: Mutex<Admission>,
    wake: Condvar,
}
/// Create a fresh gateway for every workspace opening, including reopening the
/// same folder. A retired instance can never acquire a different binding.
pub struct NativeGateway {
    inner: Arc<Inner>,
}
impl NativeGateway {
    pub fn open_with_profile(
        workspace: &NativeWorkspace,
        secrets: Arc<SecretVault>,
        profile: &std::path::Path,
    ) -> Result<Self> {
        let gateway = Self::open_inner(workspace, secrets, false)?;
        gateway.inner.bridge.runtime.files.set_effect_profile(
            profile,
            &workspace.scope.workspace_id,
            workspace.read_only.then_some(workspace.root.as_path()),
        )?;
        let registry = artex_host::CapabilityRegistry::default();
        let cap = registry.grant_directory(profile)?;
        let log_root = registry.resolve(
            &cap,
            &format!("native-logs/{}", workspace.scope.workspace_id),
            true,
        )?;
        std::fs::create_dir_all(&log_root).map_err(Error::io)?;
        gateway.inner.bridge.configure_native_logs(&log_root)?;
        Ok(gateway)
    }
    pub fn open(workspace: &NativeWorkspace, secrets: Arc<SecretVault>) -> Result<Self> {
        Self::open_inner(workspace, secrets, true)
    }
    fn open_inner(
        workspace: &NativeWorkspace,
        secrets: Arc<SecretVault>,
        workspace_logs: bool,
    ) -> Result<Self> {
        let root = workspace.root.canonicalize().map_err(Error::io)?;
        let bridge = HostBridge::new(secrets);
        for (token, path) in [
            (&workspace.execution_capability, &workspace.work_root),
            (&workspace.resource_capability, &workspace.resources_root),
        ] {
            if !path.exists() && workspace.read_only {
                continue;
            }
            let canonical = path.canonicalize().map_err(Error::io)?;
            if !canonical.starts_with(&root) || canonical == root {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Native capability must be a workspace child directory",
                ));
            }
            bridge
                .runtime
                .capabilities
                .grant_directory_as(token, path)?;
        }
        bridge.bind(&workspace.scope.engine_epoch, &workspace.scope.workspace_id)?;
        if workspace_logs && !workspace.read_only {
            bridge.configure_native_logs(&workspace.work_root)?;
        }
        Ok(Self {
            inner: Arc::new(Inner {
                bridge,
                workspace: workspace.clone(),
                admission: Mutex::new(Admission {
                    closing: false,
                    active: 0,
                    work_active: 0,
                    control_active: 0,
                    observer_active: 0,
                    stop_started: false,
                    stop_complete: false,
                    run_active: Default::default(),
                }),
                wake: Condvar::new(),
            }),
        })
    }
    pub fn scope(&self) -> &artex_rpc::native::WorkspaceScope {
        &self.inner.workspace.scope
    }
    fn check_context(&self, workspace: &str, context: &Context) -> Result<()> {
        if workspace != self.scope().workspace_id
            || context.actor.engine_epoch != self.scope().engine_epoch
        {
            return Err(Error::new(
                "STALE_EPOCH",
                "Native workspace context is stale",
            ));
        }
        let actor = &context.actor;
        let valid = match actor.kind.as_str() {
            "user" => actor.agent_id.is_none() && actor.run_id.is_none(),
            "agent" => {
                actor
                    .agent_id
                    .as_deref()
                    .is_some_and(|id| valid_id(id, "agent"))
                    && actor
                        .run_id
                        .as_deref()
                        .is_some_and(|id| valid_id(id, "run"))
            }
            _ => false,
        };
        if !valid {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Native ownership must be issued by the kernel",
            ));
        }
        for (id, prefix) in [
            (context.task_id.as_deref(), "task"),
            (context.intent_id.as_deref(), "node"),
            (context.conversation_id.as_deref(), "conversation"),
        ] {
            if id.is_some_and(|id| !valid_id(id, prefix)) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Native context identifier is invalid",
                ));
            }
        }
        Ok(())
    }
    fn begin_stop(&self) -> Result<()> {
        let mut admission = self.inner.admission.lock().unwrap();
        admission.closing = true;
        if admission.stop_started {
            return Ok(());
        }
        admission.stop_started = true;
        drop(admission);
        let inner = self.inner.clone();
        if let Err(error) = thread::Builder::new()
            .name("native-host-retire".into())
            .spawn(move || {
                inner.bridge.invalidate();
                let mut admission = inner.admission.lock().unwrap();
                admission.stop_complete = true;
                inner.wake.notify_all();
            })
        {
            self.inner.admission.lock().unwrap().stop_started = false;
            return Err(Error::io(error));
        }
        Ok(())
    }
    fn cancel_owned(&self, context: &Context, parameters: &Value) {
        if let Some(run) = context.actor.run_id.as_deref() {
            self.inner
                .bridge
                .runtime
                .stop_run(run, &context.actor.engine_epoch);
        } else if let Some(command) = parameters["command_id"].as_str() {
            self.inner
                .bridge
                .runtime
                .stop_run(&format!("ui_{command}"), &context.actor.engine_epoch);
        }
    }
}
struct Permit(Arc<Inner>, Option<String>, RequestClass);
impl Drop for Permit {
    fn drop(&mut self) {
        let mut admission = self.0.admission.lock().unwrap();
        admission.active -= 1;
        *self.2.count(&mut admission) -= 1;
        if let Some(run) = &self.1 {
            if let Some(count) = admission.run_active.get_mut(run) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    admission.run_active.remove(run);
                }
            }
        }
        self.0.wake.notify_all();
    }
}
impl HostGateway for NativeGateway {
    fn request(
        &self,
        workspace: &str,
        context: &Context,
        operation: &str,
        parameters: Value,
        timeout: Duration,
    ) -> Result<Value> {
        self.check_context(workspace, context)?;
        if timeout.is_zero() {
            return Err(Error::new(
                "TIMEOUT",
                "Native operation deadline is exhausted",
            ));
        }
        if self.inner.workspace.read_only && !readonly_operation(operation, &parameters) {
            return Err(Error::new(
                "READ_ONLY",
                "Native effects are disabled for a read-only workspace",
            ));
        }
        if self.inner.workspace.read_only
            && operation == "native.file.save"
            && !self.inner.bridge.runtime.files.has_effect_profile()
        {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "Read-only export requires a configured external app profile",
            ));
        }
        let permit = {
            let mut admission = self.inner.admission.lock().unwrap();
            let class = RequestClass::for_operation(operation);
            let run = match class {
                RequestClass::Work => context.actor.run_id.clone(),
                RequestClass::Control | RequestClass::Observer => None,
            };
            admission.admit(class, run.as_deref())?;
            Permit(self.inner.clone(), run, class)
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let inner = self.inner.clone();
        let owner = context.clone();
        let name = operation.to_string();
        let params = parameters.clone();
        let workspace = workspace.to_string();
        thread::Builder::new()
            .name("native-host-operation".into())
            .spawn(move || {
                let _permit = permit;
                if matches!(
                    name.as_str(),
                    "workspace.quarantine" | "workspace.quarantine_conversation"
                ) {
                    let scope = if name == "workspace.quarantine" {
                        format!("tasks/{}", params["task_id"].as_str().unwrap_or_default())
                    } else {
                        format!(
                            "conversations/{}",
                            params["conversation_id"].as_str().unwrap_or_default()
                        )
                    };
                    let runs = inner
                        .bridge
                        .runtime
                        .scope_runs(&scope, &owner.actor.engine_epoch);
                    let admission = inner.admission.lock().unwrap();
                    if runs
                        .iter()
                        .any(|run| admission.run_active.get(run).copied().unwrap_or(0) > 0)
                    {
                        let _ = tx.send(Err(Error::new(
                            "BUSY",
                            "Scoped native effects are still active; files retained",
                        )));
                        return;
                    }
                }
                let mut result = inner
                    .bridge
                    .execute_direct(&workspace, &owner, &name, params);
                if matches!(name.as_str(), "run.inspect" | "run.stop" | "run.force_stop") {
                    if let (Some(run), Ok(value)) = (owner.actor.run_id.as_ref(), result.as_mut()) {
                        let active = inner
                            .admission
                            .lock()
                            .unwrap()
                            .run_active
                            .get(run)
                            .copied()
                            .unwrap_or(0);
                        value["active_host_calls"] = serde_json::json!(active);
                        value["run_requests_active"] = serde_json::json!(active);
                        value["run_requests_quiesced"] = serde_json::json!(active == 0);
                        value["external_cleanup_confirmed"] = value["cleanup_confirmed"].clone();
                        if active > 0 {
                            value["cleanup_confirmed"] = serde_json::json!(false);
                            value["cleanup_state"] = serde_json::json!("unconfirmed");
                        }
                    }
                }
                if !name.starts_with("logs.") {
                    if let Err(error) = &result {
                        inner.bridge.record_native_log(&format!(
                            "Native operation {name} stopped: {}",
                            error.code
                        ));
                    } else if name == "tool.execute" {
                        inner
                            .bridge
                            .record_native_log("Native agent tool completed");
                    }
                }
                let _ = tx.send(result);
            })
            .map_err(Error::io)?;
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Inspection must never turn an observer deadline into a stop
                // request against the process being observed.
                if !matches!(operation, "run.inspect" | "scope.inspect") {
                    self.cancel_owned(context, &parameters);
                }
                let mut error = Error::new(
                    "TIMEOUT",
                    "Native operation did not confirm completion before its deadline",
                );
                error.recovery_required = !readonly_operation(operation, &parameters);
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let mut error = Error::new(
                    "EXTERNAL_FAILURE",
                    "Native operation worker exited without a confirmed result",
                );
                error.recovery_required = !readonly_operation(operation, &parameters);
                Err(error)
            }
        }
    }
    fn retire(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        self.begin_stop()?;
        let mut admission = self.inner.admission.lock().unwrap();
        loop {
            if admission.active == 0
                && admission.stop_complete
                && !self
                    .inner
                    .bridge
                    .runtime
                    .epoch_active(&self.scope().engine_epoch)
            {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let mut error = Error::new(
                    "RECOVERY_REQUIRED",
                    "Native external work has not confirmed retirement",
                );
                error.recovery_required = true;
                return Err(error);
            }
            admission = self
                .inner
                .wake
                .wait_timeout(admission, remaining.min(Duration::from_millis(20)))
                .unwrap()
                .0;
        }
    }
    fn retirement_report(&self, workspace: &str, contexts: &[Context]) -> Result<Option<Value>> {
        if workspace != self.scope().workspace_id {
            return Err(Error::new(
                "STALE_EPOCH",
                "Retirement report workspace is stale",
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for context in contexts {
            self.check_context(workspace, context)?;
            if context.actor.kind != "agent" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Retirement report requires stored agent run contexts",
                ));
            }
            if !ids.insert(context.actor.run_id.as_deref().unwrap()) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Retirement report requires distinct owned runs",
                ));
            }
        }
        let admission = self.inner.admission.lock().unwrap();
        if !admission.closing
            || !admission.stop_complete
            || admission.active != 0
            || self
                .inner
                .bridge
                .runtime
                .epoch_active(&self.scope().engine_epoch)
        {
            let mut error = Error::new(
                "RECOVERY_REQUIRED",
                "Native retirement is not confirmed; no report can be issued",
            );
            error.recovery_required = true;
            return Err(error);
        }
        let runs = contexts
            .iter()
            .map(|context| {
                let mut proof = self.inner.bridge.runtime.inspect_run(
                    context.actor.run_id.as_deref().unwrap(),
                    &context.actor.engine_epoch,
                );
                proof["external_cleanup_confirmed"] = proof["cleanup_confirmed"].clone();
                proof["active_host_calls"] = serde_json::json!(0);
                proof["run_requests_active"] = serde_json::json!(0);
                proof["run_requests_quiesced"] = serde_json::json!(true);
                // Historical Unix executions retain their unconfirmed descendant
                // proof. Normal retirement is not a stronger containment guarantee.
                proof
            })
            .collect::<Vec<_>>();
        Ok(Some(serde_json::json!({"runs":runs})))
    }
}
impl Drop for NativeGateway {
    fn drop(&mut self) {
        let _ = self.begin_stop();
    }
}
fn readonly_operation(operation: &str, parameters: &Value) -> bool {
    match operation {
        "skill.catalog"
        | "native.file.read"
        | "native.file.stage"
        | "native.file.release"
        | "native.file.save"
        | "native.skill.meta.read"
        | "secret.available"
        | "secret.resolve_user"
        | "secret.resolve"
        | "logs.poll"
        | "logs.ack"
        | "traffic.poll"
        | "traffic.ack"
        | "host.events"
        | "file.resolve"
        | "file.release"
        | "run.stop"
        | "run.inspect"
        | "run.force_stop"
        | "ui.stop" => true,
        "tool.execute" => matches!(
            parameters["name"].as_str(),
            Some("Read" | "LS" | "Glob" | "Grep" | "TaskOutput" | "TaskList")
        ),
        "ui.execute" => matches!(
            parameters["name"]
                .as_str()
                .or_else(|| parameters["method"].as_str())
                .map(|name| name.strip_prefix("ui.").unwrap_or(name)),
            Some(
                "workspaceList"
                    | "workspaceRead"
                    | "workspaceDownload"
                    | "skills"
                    | "skillFiles"
                    | "readSkillFile"
            )
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Actor, new_id};
    use artex_rpc::native::WorkspaceScope;
    fn reserve_test_slot(gateway: &NativeGateway, class: RequestClass) -> Permit {
        gateway
            .inner
            .admission
            .lock()
            .unwrap()
            .admit(class, None)
            .unwrap();
        Permit(gateway.inner.clone(), None, class)
    }
    #[test]
    fn saturated_work_cannot_starve_stop_and_all_reserved_lanes_are_bounded() {
        let (_dir, w, g, c) = fixture();
        let gateway = Arc::new(g);
        // Model 31 other stalled requests with the production admission/RAII
        // code; only one real cancellable Rust sleep worker is needed.
        let held_work = (0..MAX_ACTIVE - 1)
            .map(|_| reserve_test_slot(&gateway, RequestClass::Work))
            .collect::<Vec<_>>();
        let worker_gateway = gateway.clone();
        let owner = c.clone();
        let workspace = w.scope.workspace_id.clone();
        let worker = thread::spawn(move || {
            worker_gateway.request(
                &workspace,
                &owner,
                "tool.execute",
                json!({"name":"sleep","input":{"seconds":10}}),
                Duration::from_secs(15),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if gateway.inner.admission.lock().unwrap().work_active == MAX_ACTIVE {
                break;
            }
            assert!(Instant::now() < deadline, "owned worker was not admitted");
            thread::sleep(Duration::from_millis(1));
        }
        let full = gateway
            .request(
                &w.scope.workspace_id,
                &c,
                "tool.execute",
                json!({"name":"TaskList","input":{}}),
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(full.code, "BUSY");
        let held_control = (0..MAX_CONTROLS)
            .map(|_| reserve_test_slot(&gateway, RequestClass::Control))
            .collect::<Vec<_>>();
        let held_observers = (0..MAX_OBSERVERS)
            .map(|_| reserve_test_slot(&gateway, RequestClass::Observer))
            .collect::<Vec<_>>();
        assert_eq!(gateway.inner.admission.lock().unwrap().active, 40);
        assert_eq!(
            gateway
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "run.stop",
                    json!({}),
                    Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "BUSY"
        );
        assert_eq!(
            gateway
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "run.inspect",
                    json!({}),
                    Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "BUSY"
        );
        drop(held_control);
        // Observer flooding and all 32 work slots still leave the four stop
        // slots available. This cancels the actual owned worker, not a mock.
        let stopped = gateway
            .request(
                &w.scope.workspace_id,
                &c,
                "run.stop",
                json!({}),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(stopped["accepted"], true);
        assert_eq!(stopped["stop_requested"], true);
        let ended = worker.join().unwrap().unwrap_err();
        assert_eq!(ended.code, "CANCELLED");
        drop(held_observers);
        let deadline = Instant::now() + Duration::from_secs(2);
        let inspected = loop {
            let value = gateway
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "run.inspect",
                    json!({}),
                    Duration::from_secs(2),
                )
                .unwrap();
            if value["run_requests_quiesced"] == true {
                break value;
            }
            assert!(
                Instant::now() < deadline,
                "owned worker permit did not retire"
            );
            thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(inspected["active_host_calls"], 0);
        assert_eq!(inspected["cleanup_confirmed"], true);
        drop(held_work);
        // Completion is sent immediately before its RAII permit is dropped.
        let deadline = Instant::now() + Duration::from_secs(2);
        while gateway.inner.admission.lock().unwrap().active != 0 {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
        let counts = gateway.inner.admission.lock().unwrap();
        assert_eq!(
            (
                counts.work_active,
                counts.control_active,
                counts.observer_active
            ),
            (0, 0, 0)
        );
        drop(counts);
        gateway.retire(Duration::from_secs(5)).unwrap();
    }
    #[test]
    fn retirement_report_requires_actual_retirement_and_exact_current_scope() {
        let (_dir, w, g, c) = fixture();
        assert_eq!(
            g.retirement_report(&w.scope.workspace_id, std::slice::from_ref(&c))
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        g.retire(Duration::from_secs(5)).unwrap();
        let report = g
            .retirement_report(&w.scope.workspace_id, std::slice::from_ref(&c))
            .unwrap()
            .unwrap();
        assert_eq!(
            report["runs"][0]["run_id"],
            c.actor.run_id.as_deref().unwrap()
        );
        assert_eq!(report["runs"][0]["engine_epoch"], w.scope.engine_epoch);
        assert_eq!(report["runs"][0]["cleanup_confirmed"], true);
        assert_eq!(report["runs"][0]["active_host_calls"], 0);
        assert_eq!(report["runs"][0]["run_requests_quiesced"], true);
        assert_eq!(
            g.retirement_report(&new_id("workspace"), std::slice::from_ref(&c))
                .unwrap_err()
                .code,
            "STALE_EPOCH"
        );
        let mut old = c.clone();
        old.actor.engine_epoch = new_id("epoch");
        assert_eq!(
            g.retirement_report(&w.scope.workspace_id, &[old])
                .unwrap_err()
                .code,
            "STALE_EPOCH"
        );
        let mut invalid = c.clone();
        invalid.actor.run_id = Some("run_bad".into());
        assert_eq!(
            g.retirement_report(&w.scope.workspace_id, &[invalid])
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            g.retirement_report(&w.scope.workspace_id, &[c.clone(), c])
                .unwrap_err()
                .code,
            "INVALID_PARAMS"
        );
    }
    #[cfg(unix)]
    #[test]
    fn retirement_report_does_not_upgrade_unconfined_unix_execution() {
        let (_dir, w, g, c) = fixture();
        let id = new_id("exec");
        let run = c.actor.run_id.as_deref().unwrap();
        g.inner
            .bridge
            .runtime
            .executions
            .execute(artex_host::ExecutionRequest {
                execution_id: id.clone(),
                run_id: run.into(),
                engine_epoch: w.scope.engine_epoch.clone(),
                executable: "/bin/sh".into(),
                args: vec!["-c".into(), "true".into()],
                cwd: w.work_root.clone(),
                timeout_ms: 1000,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while g
            .inner
            .bridge
            .runtime
            .executions
            .status(&id, run, &w.scope.engine_epoch)
            .unwrap()
            .state
            == "running"
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        g.retire(Duration::from_secs(5)).unwrap();
        let report = g
            .retirement_report(&w.scope.workspace_id, &[c])
            .unwrap()
            .unwrap();
        assert_eq!(report["runs"][0]["run_requests_quiesced"], true);
        assert_eq!(report["runs"][0]["cleanup_confirmed"], false);
        assert_eq!(
            report["runs"][0]["executions"][0]["descendant_cleanup_confirmed"],
            false
        );
    }
    #[test]
    fn monitor_deadline_never_cancels_owned_run_and_workers_block_cleanup() {
        let (_dir, w, g, c) = fixture();
        let gateway = Arc::new(g);
        let worker_gateway = gateway.clone();
        let owner = c.clone();
        let wid = w.scope.workspace_id.clone();
        let worker = thread::spawn(move || {
            worker_gateway.request(
                &wid,
                &owner,
                "tool.execute",
                json!({"name":"sleep","input":{"seconds":2}}),
                Duration::from_secs(5),
            )
        });
        let end = Instant::now() + Duration::from_secs(2);
        while gateway
            .inner
            .admission
            .lock()
            .unwrap()
            .run_active
            .get(c.actor.run_id.as_ref().unwrap())
            .copied()
            .unwrap_or(0)
            == 0
        {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(1));
        }
        // A deliberately exhausted observer budget may race a fast result;
        // either outcome must leave the real run untouched.
        let observed = gateway.request(
            &w.scope.workspace_id,
            &c,
            "run.inspect",
            json!({}),
            Duration::from_nanos(1),
        );
        if let Err(error) = observed {
            assert_eq!(error.code, "TIMEOUT");
        }
        let result = gateway
            .request(
                &w.scope.workspace_id,
                &c,
                "run.inspect",
                json!({}),
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result["stop_requested"], false);
        assert_eq!(result["external_cleanup_confirmed"], true);
        assert_eq!(result["run_requests_quiesced"], false);
        assert_eq!(result["cleanup_confirmed"], false);
        assert!(result["active_host_calls"].as_u64().unwrap() > 0);
        assert!(
            worker.join().unwrap().is_ok(),
            "monitor deadline cancelled the real sleep"
        );
        let result = gateway
            .request(
                &w.scope.workspace_id,
                &c,
                "run.inspect",
                json!({}),
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result["cleanup_confirmed"], true);
        assert_eq!(result["run_requests_quiesced"], true);
        let denied = gateway
            .request(
                &w.scope.workspace_id,
                &c,
                "run.force_stop",
                json!({"pid":123}),
                Duration::from_secs(1),
            )
            .unwrap_err();
        assert_eq!(denied.code, "INVALID_PARAMS");
        let mut wrong = c.clone();
        wrong.actor.engine_epoch = new_id("epoch");
        assert_eq!(
            gateway
                .request(
                    &w.scope.workspace_id,
                    &wrong,
                    "run.inspect",
                    json!({}),
                    Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "STALE_EPOCH"
        );
        gateway.retire(Duration::from_secs(5)).unwrap();
    }
    fn fixture() -> (tempfile::TempDir, NativeWorkspace, NativeGateway, Context) {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let resources = dir.path().join("resources");
        std::fs::create_dir(&work).unwrap();
        std::fs::create_dir(&resources).unwrap();
        let workspace = NativeWorkspace {
            scope: WorkspaceScope {
                workspace_id: new_id("workspace"),
                engine_epoch: new_id("epoch"),
                open_generation: new_id("generation"),
            },
            root: dir.path().to_path_buf(),
            work_root: work,
            resources_root: resources,
            execution_capability: new_id("capability"),
            resource_capability: new_id("capability"),
            read_only: false,
        };
        let gateway = NativeGateway::open(&workspace, Arc::new(SecretVault::default())).unwrap();
        let context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: workspace.scope.engine_epoch.clone(),
            },
            task_id: Some(new_id("task")),
            intent_id: None,
            conversation_id: None,
        };
        (dir, workspace, gateway, context)
    }
    #[test]
    fn scoped_upload_is_readable_by_its_conversation_and_not_another() {
        let (_dir, w, g, mut agent) = fixture();
        agent.task_id = None;
        agent.conversation_id = Some(new_id("conversation"));
        let mut user = agent.clone();
        user.actor.kind = "user".into();
        user.actor.agent_id = None;
        user.actor.run_id = None;
        user.conversation_id = None;
        let selected = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(selected.path(), "SCOPED-ATTACHMENT").unwrap();
        let deadline = Duration::from_secs(10);
        let staged = g
            .request(
                &w.scope.workspace_id,
                &user,
                "native.file.stage",
                json!({"path":selected.path()}),
                deadline,
            )
            .unwrap();
        let upload=g.request(&w.scope.workspace_id,&user,"ui.execute",json!({"name":"chatUpload","input":{"scope":"session","id":agent.conversation_id,"files":[staged["file_capability"]],"directory_capability":w.execution_capability,"command_id":new_id("command")}}),deadline).unwrap();
        let path = upload["attachments"][0]["path"].as_str().unwrap();
        assert!(
            w.work_root
                .join("conversations")
                .join(agent.conversation_id.as_ref().unwrap())
                .join(path)
                .is_file()
        );
        let input = json!({"name":"Read","input":{"file_path":path,"directory_capability":w.execution_capability,"attachment_paths":[path]}});
        let read = g
            .request(
                &w.scope.workspace_id,
                &agent,
                "tool.execute",
                input.clone(),
                deadline,
            )
            .unwrap();
        assert!(
            read["content"]
                .as_str()
                .unwrap()
                .contains("SCOPED-ATTACHMENT")
        );
        agent.conversation_id = Some(new_id("conversation"));
        assert!(
            g.request(
                &w.scope.workspace_id,
                &agent,
                "tool.execute",
                input,
                deadline
            )
            .is_err()
        );
        g.retire(deadline).unwrap();
    }
    #[test]
    fn readonly_generated_download_saves_outside_workspace_with_profile_receipt() {
        let (_dir, mut w, g, mut c) = fixture();
        let deadline = Duration::from_secs(10);
        g.retire(deadline).unwrap();
        std::fs::write(w.work_root.join("download.txt"), "READONLY-EXPORT").unwrap();
        w.read_only = true;
        w.scope.engine_epoch = new_id("epoch");
        w.scope.open_generation = new_id("generation");
        let profile = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let native =
            NativeGateway::open_with_profile(&w, Arc::new(SecretVault::default()), profile.path())
                .unwrap();
        c.actor.kind = "user".into();
        c.actor.agent_id = None;
        c.actor.run_id = None;
        c.task_id = None;
        c.actor.engine_epoch = w.scope.engine_epoch.clone();
        let before = walk_files(&w.work_root);
        let result=native.request(&w.scope.workspace_id,&c,"ui.execute",json!({"name":"ui.workspaceDownload","input":{"directory_capability":w.execution_capability,"path":"download.txt"}}),deadline).unwrap();
        let destination = out.path().join("export.txt");
        let command = new_id("command");
        let params = json!({"file_capability":result["file_capability"],"destination":destination,"command_id":command});
        let saved = native
            .request(
                &w.scope.workspace_id,
                &c,
                "native.file.save",
                params.clone(),
                deadline,
            )
            .unwrap();
        assert_eq!(saved["saved"], true);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "READONLY-EXPORT"
        );
        std::fs::write(&destination, "changed later").unwrap();
        assert_eq!(
            native
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "native.file.save",
                    params,
                    deadline
                )
                .unwrap(),
            saved
        );
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "changed later"
        );
        assert!(native.request(&w.scope.workspace_id,&c,"native.file.save",json!({"file_capability":result["file_capability"],"destination":w.work_root.join("forbidden.txt"),"command_id":new_id("command")}),deadline).is_err());
        assert_eq!(before, walk_files(&w.work_root));
        let logs = native
            .request(
                &w.scope.workspace_id,
                &c,
                "logs.poll",
                json!({"limit":100}),
                deadline,
            )
            .unwrap();
        assert!(!logs["items"].as_array().unwrap().is_empty());
        native.retire(deadline).unwrap();
    }
    fn walk_files(
        root: &std::path::Path,
    ) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        fn visit(
            root: &std::path::Path,
            out: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
        ) {
            for entry in std::fs::read_dir(root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path, out)
                } else {
                    out.insert(path.clone(), std::fs::read(path).unwrap());
                }
            }
        }
        let mut result = Default::default();
        visit(root, &mut result);
        result
    }
    #[test]
    fn native_editor_conflict_and_receipt_replay_preserve_external_changes() {
        let (_dir, w, g, mut c) = fixture();
        c.actor.kind = "user".into();
        c.actor.agent_id = None;
        c.actor.run_id = None;
        c.task_id = None;
        let deadline = Duration::from_secs(10);
        let command = new_id("command");
        let input = json!({"directory_capability":w.execution_capability,"path":"edit.txt","content":"created","create":true,"expected_sha256":null,"command_id":command});
        let saved = g
            .request(
                &w.scope.workspace_id,
                &c,
                "native.file.write",
                input.clone(),
                deadline,
            )
            .unwrap();
        std::fs::write(w.work_root.join("edit.txt"), "external").unwrap();
        assert_eq!(
            g.request(
                &w.scope.workspace_id,
                &c,
                "native.file.write",
                input,
                deadline
            )
            .unwrap(),
            saved
        );
        assert_eq!(
            std::fs::read_to_string(w.work_root.join("edit.txt")).unwrap(),
            "external"
        );
        let stale = json!({"directory_capability":w.execution_capability,"path":"edit.txt","content":"draft","create":false,"expected_sha256":saved["sha256"],"command_id":new_id("command")});
        assert_eq!(
            g.request(
                &w.scope.workspace_id,
                &c,
                "native.file.write",
                stale,
                deadline
            )
            .unwrap_err()
            .code,
            "VERSION_CONFLICT"
        );
        let loaded = g
            .request(
                &w.scope.workspace_id,
                &c,
                "native.file.read",
                json!({"directory_capability":w.execution_capability,"path":"edit.txt"}),
                deadline,
            )
            .unwrap();
        let fresh = json!({"directory_capability":w.execution_capability,"path":"edit.txt","content":"reconciled","create":false,"expected_sha256":loaded["sha256"],"command_id":new_id("command")});
        assert_eq!(
            g.request(
                &w.scope.workspace_id,
                &c,
                "native.file.write",
                fresh,
                deadline
            )
            .unwrap()["saved"],
            true
        );
        g.retire(deadline).unwrap();
    }
    #[test]
    fn direct_file_skill_calls_and_old_epoch_are_fenced() {
        let (_dir, w, g, c) = fixture();
        let deadline = Duration::from_secs(10);
        g.request(&w.scope.workspace_id,&c,"tool.execute",json!({"name":"Write","input":{"directory_capability":w.execution_capability,"file_path":"native.txt","content":"DIRECT-NATIVE"}}),deadline).unwrap();
        let read=g.request(&w.scope.workspace_id,&c,"tool.execute",json!({"name":"Read","input":{"directory_capability":w.execution_capability,"file_path":"native.txt"}}),deadline).unwrap();
        assert!(read["content"].as_str().unwrap().contains("DIRECT-NATIVE"));
        artex_host::skills::execute(&g.inner.bridge.runtime.capabilities,"create_skill",&json!({"directory_capability":w.resource_capability,"name":"approved","description":"native","instructions":"Read ${SKILL_DIR}/sample.txt"})).unwrap();
        artex_host::skills::execute(&g.inner.bridge.runtime.capabilities,"update_skill_file",&json!({"directory_capability":w.resource_capability,"name":"approved","file":"sample.txt","content":"DIRECT-SKILL"})).unwrap();
        let skill=g.request(&w.scope.workspace_id,&c,"tool.execute",json!({"name":"Skill","input":{"directory_capability":w.resource_capability,"execution_directory_capability":w.execution_capability,"name":"approved"}}),deadline).unwrap();
        let path = format!("{}/sample.txt", skill["skill_dir"].as_str().unwrap());
        let read=g.request(&w.scope.workspace_id,&c,"tool.execute",json!({"name":"Read","input":{"directory_capability":w.execution_capability,"file_path":path}}),deadline).unwrap();
        assert!(read["content"].as_str().unwrap().contains("DIRECT-SKILL"));
        assert!(
            g.request(
                &w.scope.workspace_id,
                &c,
                "host.input.begin",
                json!({}),
                deadline
            )
            .is_err()
        );
        let mut old = c.clone();
        old.actor.engine_epoch = new_id("epoch");
        assert_eq!(
            g.request(
                &w.scope.workspace_id,
                &old,
                "tool.execute",
                json!({}),
                deadline
            )
            .unwrap_err()
            .code,
            "STALE_EPOCH"
        );
        g.retire(deadline).unwrap();
        assert_eq!(
            g.request(
                &w.scope.workspace_id,
                &c,
                "host.events",
                json!({}),
                deadline
            )
            .unwrap_err()
            .code,
            "CANCELLED"
        );
        let mut reopened = w.clone();
        reopened.scope.engine_epoch = new_id("epoch");
        reopened.scope.open_generation = new_id("generation");
        let fresh = NativeGateway::open(&reopened, Arc::new(SecretVault::default())).unwrap();
        assert_eq!(
            fresh
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "host.events",
                    json!({}),
                    deadline
                )
                .unwrap_err()
                .code,
            "STALE_EPOCH"
        );
        fresh.retire(deadline).unwrap();
    }
    #[test]
    fn direct_mcp_stdio_is_a_user_tool_process_not_an_engine_process() {
        let (_dir, w, g, mut c) = fixture();
        c.actor.kind = "user".into();
        c.actor.agent_id = None;
        c.actor.run_id = None;
        c.task_id = None;
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/host/tests/fixtures/mcp_server.py");
        let result=g.request(&w.scope.workspace_id,&c,"mcp.call",json!({"command_id":new_id("command"),"directory_capability":w.execution_capability,"server":{"transport":"stdio","command":if cfg!(windows){"python.exe"}else{"python3"},"args":[script]},"tool_name":"echo","arguments":{"text":"DIRECT-MCP"}}),Duration::from_secs(20)).unwrap();
        assert!(result.to_string().contains("DIRECT-MCP"));
        g.retire(Duration::from_secs(5)).unwrap();
    }
    #[test]
    fn read_only_absent_roots_are_not_created() {
        // Begin with genuinely absent roots, not a writable host's populated
        // log directory. The real profile is a separate selected directory.
        let dir = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let w = NativeWorkspace {
            scope: WorkspaceScope {
                workspace_id: new_id("workspace"),
                engine_epoch: new_id("epoch"),
                open_generation: new_id("generation"),
            },
            root: dir.path().into(),
            work_root: dir.path().join("work"),
            resources_root: dir.path().join("resources"),
            execution_capability: new_id("capability"),
            resource_capability: new_id("capability"),
            read_only: true,
        };
        let gateway =
            NativeGateway::open_with_profile(&w, Arc::new(SecretVault::default()), profile.path())
                .unwrap();
        assert!(!dir.path().join("work").exists());
        assert!(!dir.path().join("resources").exists());
        let c = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: w.scope.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        assert_eq!(
            gateway
                .request(
                    &w.scope.workspace_id,
                    &c,
                    "tool.execute",
                    json!({"name":"Bash","input":{}}),
                    Duration::from_secs(1)
                )
                .unwrap_err()
                .code,
            "READ_ONLY"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        gateway.retire(Duration::from_secs(5)).unwrap();
    }
}
