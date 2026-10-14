//! In-process boundary between the native kernel and OS operations.
use crate::{HostRuntime, SecretVault};
use artex_core::{Context, Error, Result};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub struct HostBridge {
    pub runtime: HostRuntime,
    pub secrets: Arc<SecretVault>,
    binding: Mutex<Option<(String, String)>>,
}
impl HostBridge {
    pub fn new(secrets: Arc<SecretVault>) -> Self {
        Self {
            runtime: HostRuntime::default(),
            secrets,
            binding: Mutex::new(None),
        }
    }
    pub fn bind(&self, epoch: &str, workspace: &str) -> Result<()> {
        if epoch.is_empty() || workspace.is_empty() {
            return Err(Error::new("INVALID_PARAMS", "Host binding required"));
        }
        let mut binding = self.binding.lock().unwrap();
        match binding.as_ref() {
            Some(old) if old != &(epoch.into(), workspace.into()) => {
                Err(Error::new("VERSION_CONFLICT", "Host binding is immutable"))
            }
            _ => {
                *binding = Some((epoch.into(), workspace.into()));
                if self.runtime.logs.activate().is_err() {
                    self.runtime.logs.capture_failure();
                }
                Ok(())
            }
        }
    }
    /// In-process native boundary. Context and operation come from the kernel,
    /// never from a JSON envelope or UI-supplied actor fields.
    pub fn execute_direct(
        &self,
        workspace: &str,
        context: &Context,
        operation: &str,
        mut parameters: Value,
    ) -> Result<Value> {
        if !parameters.is_object()
            || parameters.get("context").is_some()
            || parameters.get("operation").is_some()
            || parameters.get("host_input").is_some()
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Native operation parameters must not supply ownership or wire transfer fields",
            ));
        }
        if operation.starts_with("host.input.") || operation.starts_with("host.result.") {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "Wire transfers do not exist in the native direct gateway",
            ));
        }
        parameters["operation"] = json!(operation);
        self.execute_common(workspace, context, &parameters)
    }
    fn execute_common(&self, workspace: &str, context: &Context, params: &Value) -> Result<Value> {
        let (epoch, bound_workspace) = self
            .binding
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Host workspace is not bound"))?;
        if workspace != bound_workspace || context.actor.engine_epoch != epoch {
            return Err(Error::new("STALE_EPOCH", "Native context binding mismatch"));
        }
        if matches!(
            params["operation"].as_str(),
            Some(
                "file.resolve"
                    | "native.secret.set"
                    | "native.secret.delete"
                    | "native.file.read"
                    | "native.file.write"
                    | "native.file.stage"
                    | "native.file.release"
                    | "native.file.save"
                    | "native.skill.meta.read"
                    | "native.skill.meta.write"
                    | "file.release"
                    | "secret.import"
                    | "ui.stop"
                    | "notification.send"
                    | "traffic.configure"
                    | "traffic.poll"
                    | "traffic.ack"
                    | "secret.resolve_user"
                    | "mcp.call"
                    | "mcp.refresh"
                    | "workspace.quarantine"
                    | "secret.available"
                    | "logs.poll"
                    | "logs.ack"
                    | "workspace.restore_quarantine"
                    | "workspace.quarantine_conversation"
                    | "workspace.restore_conversation_quarantine"
                    | "workspace.complete_conversation_quarantine"
            )
        ) {
            if context.actor.kind != "user"
                || context.actor.agent_id.is_some()
                || context.actor.run_id.is_some()
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Private archive operation requires user ownership",
                ));
            }
            if matches!(
                params["operation"].as_str(),
                Some("native.skill.meta.read" | "native.skill.meta.write")
            ) {
                let write = params["operation"] == "native.skill.meta.write";
                if write
                    && !params["command_id"]
                        .as_str()
                        .is_some_and(|id| artex_core::valid_id(id, "command"))
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Typed skill metadata command ID required",
                    ));
                }
                let mut input = params.clone();
                input.as_object_mut().unwrap().remove("operation");
                input.as_object_mut().unwrap().remove("context");
                return self.runtime.execute_ui(
                    if write {
                        "nativeSkillMetaWrite"
                    } else {
                        "nativeSkillMetaRead"
                    },
                    &input,
                    context,
                );
            }
            if params["operation"] == "native.file.stage" {
                let source = params["path"]
                    .as_str()
                    .filter(|path| path.len() <= 32768)
                    .ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Bounded selected file path required")
                    })?;
                return self.runtime.files.stage_path_named(
                    std::path::Path::new(source),
                    params["name"].as_str(),
                    || self.runtime.epoch_closed(&context.actor.engine_epoch),
                );
            }
            if params["operation"] == "native.file.save" {
                if !params["command_id"]
                    .as_str()
                    .is_some_and(|id| artex_core::valid_id(id, "command"))
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Typed save command ID required",
                    ));
                }
                if self.runtime.files.has_effect_profile() {
                    return self.runtime.files.save_with_receipt(
                        params["file_capability"]
                            .as_str()
                            .ok_or_else(|| Error::new("INVALID_PARAMS", "File token required"))?,
                        std::path::Path::new(params["destination"].as_str().ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "Selected destination required")
                        })?),
                        params["command_id"].as_str().unwrap(),
                    );
                }
                let mut input = params.clone();
                input.as_object_mut().unwrap().remove("operation");
                input.as_object_mut().unwrap().remove("context");
                return self.runtime.execute_ui("nativeFileSave", &input, context);
            }
            if matches!(
                params["operation"].as_str(),
                Some("native.file.read" | "native.file.write")
            ) {
                let method = if params["operation"] == "native.file.read" {
                    "nativeFileRead"
                } else {
                    "nativeFileWrite"
                };
                let mut input = params.clone();
                input.as_object_mut().unwrap().remove("operation");
                input.as_object_mut().unwrap().remove("context");
                if method == "nativeFileWrite"
                    && !input["command_id"]
                        .as_str()
                        .is_some_and(|id| artex_core::valid_id(id, "command"))
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Typed file command ID required",
                    ));
                }
                return self.runtime.execute_ui(method, &input, context);
            }
            if params["operation"] == "native.secret.set" {
                let command = params["command_id"]
                    .as_str()
                    .filter(|id| artex_core::valid_id(id, "command"))
                    .ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Typed secret command ID required")
                    })?;
                let field = params["field"]
                    .as_str()
                    .filter(|field| {
                        !field.is_empty() && field.len() <= 128 && !field.contains('\0')
                    })
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Bounded secret field required"))?;
                let value = params["value"]
                    .as_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Secret value required"))?;
                return self.secrets.set(command, field, value);
            }
            if params["operation"] == "native.secret.delete" {
                let reference = params["reference"]
                    .as_str()
                    .or_else(|| params["reference"]["secret_ref"].as_str())
                    .filter(|id| artex_core::valid_id(id, "secret"))
                    .ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Typed secret reference required")
                    })?;
                return self.secrets.delete(reference);
            }
            if matches!(
                params["operation"].as_str(),
                Some("file.release" | "native.file.release")
            ) {
                self.runtime
                    .files
                    .release(params["file_capability"].as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Native file capability required")
                    })?)?;
                return Ok(json!({"released":true}));
            }
            if params["operation"] == "logs.poll" {
                return self
                    .runtime
                    .logs
                    .poll(params["limit"].as_u64().unwrap_or(100) as usize);
            }
            if params["operation"] == "logs.ack" {
                let ids = params["ids"]
                    .as_array()
                    .ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Log acknowledgement ids required")
                    })?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| Error::new("INVALID_PARAMS", "Log id required"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                return self.runtime.logs.acknowledge(&ids);
            }
            if params["operation"] == "workspace.restore_quarantine" {
                return self.runtime.restore_quarantine(params);
            }
            if params["operation"] == "workspace.quarantine" {
                return self.runtime.quarantine_task(params, &epoch);
            }
            if params["operation"] == "workspace.quarantine_conversation" {
                return self.runtime.quarantine_conversation(params, &epoch);
            }
            if params["operation"] == "workspace.restore_conversation_quarantine" {
                return self.runtime.restore_conversation_quarantine(params);
            }
            if params["operation"] == "workspace.complete_conversation_quarantine" {
                return self.runtime.complete_conversation_quarantine(params);
            }
            if params["operation"] == "secret.available" {
                let reference = params["secret_ref"]
                    .as_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Secret reference required"))?;
                return Ok(json!({"available":self.secrets.resolve(reference).is_ok()}));
            }
            if params["operation"] == "secret.resolve_user" {
                return Ok(
                    json!({"value":self.secrets.resolve(params["secret_ref"].as_str().ok_or_else(||Error::new("INVALID_PARAMS","Secret reference required"))?)?}),
                );
            }
            if matches!(
                params["operation"].as_str(),
                Some("mcp.call" | "mcp.refresh")
            ) {
                let mut input = params.clone();
                self.resolve_config_secrets(&mut input)?;
                let command = params["command_id"]
                    .as_str()
                    .filter(|s| s.starts_with("command_") && s.len() <= 128)
                    .ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Native MCP command id required")
                    })?;
                let mut owned = context.clone();
                owned.actor.run_id = Some(format!("ui_{command}"));
                return if params["operation"] == "mcp.call" {
                    crate::mcp::call(&self.runtime, &input, &owned)
                } else {
                    crate::mcp::refresh(&self.runtime, &input, &owned)
                };
            }
            match params["operation"].as_str() {
                Some("traffic.configure") => {
                    return self.runtime.traffic.configure(
                        &self.runtime.capabilities,
                        &self.secrets,
                        params,
                        &epoch,
                    );
                }
                Some("traffic.poll") => {
                    return self
                        .runtime
                        .traffic
                        .poll(params["limit"].as_u64().unwrap_or(20) as usize);
                }
                Some("traffic.ack") => {
                    let ids = params["ids"]
                        .as_array()
                        .ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "Capture acknowledgement ids required")
                        })?
                        .iter()
                        .map(|v| {
                            v.as_str().map(str::to_owned).ok_or_else(|| {
                                Error::new("INVALID_PARAMS", "Capture exchange id required")
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    return self.runtime.traffic.acknowledge(&ids);
                }
                _ => {}
            }
            if params["operation"] == "ui.stop" {
                let command = params["command_id"]
                    .as_str()
                    .filter(|s| s.starts_with("command_") && s.len() <= 128)
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Native command id required"))?;
                self.runtime.stop_run(&format!("ui_{command}"), &epoch);
                return Ok(json!({"accepted":true}));
            }
            if params["operation"] == "notification.send" {
                return crate::notifications::send(params, &self.secrets);
            }
            if params["operation"] == "secret.import" {
                let stable = params["stable_key"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Stable secret key required"))?;
                let value = params["value"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 65536)
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Bounded secret value required"))?;
                let reference = self.secrets.set(stable, "archive", value)?;
                return Ok(
                    json!({"available":SecretVault::persistent(&reference),"reference":reference}),
                );
            }
            let purpose = params["purpose"]
                .as_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Archive purpose required"))?;
            if purpose != "archive_import" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Staged upload is only valid for archive import",
                ));
            }
            let (path, name) =
                self.runtime
                    .files
                    .get(params["file_capability"].as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Native file capability required")
                    })?)?;
            let size = std::fs::metadata(&path).map_err(Error::io)?.len();
            return Ok(
                json!({"path":path,"kind":"file","writable":false,"byte_length":size.to_string(),"purpose":purpose,"name":name}),
            );
        }
        if params["operation"] == "ui.execute" {
            let mut input = params["input"].clone();
            if !input.is_object() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Native UI input object required",
                ));
            }
            for key in ["command_id", "execution_id", "invocation_id"] {
                if let Some(value) = params.get(key) {
                    input[key] = value.clone();
                }
            }
            self.resolve_config_secrets(&mut input)?;
            return self.runtime.execute_ui(
                params["name"]
                    .as_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "UI operation required"))?,
                &input,
                &context,
            );
        }
        if context.actor.kind != "agent"
            || context.actor.agent_id.is_none()
            || context.actor.run_id.is_none()
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Current agent run ownership required",
            ));
        }
        match params["operation"].as_str() {
            Some("skill.catalog") => {
                let cap = params["directory_capability"].as_str().ok_or_else(|| {
                    Error::new(
                        "PERMISSION_DENIED",
                        "Resource capability required for Skill catalog",
                    )
                })?;
                crate::skills::catalog(&self.runtime.capabilities, cap)
            }
            Some("tool.execute") => {
                let mut input = params["input"].clone();
                if !input.is_object() {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Host tool input object required",
                    ));
                }
                for key in ["execution_id", "command_id", "invocation_id"] {
                    if let Some(value) = params.get(key) {
                        input[key] = value.clone();
                    }
                }
                self.resolve_config_secrets(&mut input)?;
                let name = params["name"]
                    .as_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Tool name required"))?;
                if name == "Skill" {
                    let mut execution =
                        json!({"directory_capability":input["execution_directory_capability"]});
                    self.runtime.scope_input(&mut execution, &context)?;
                    input["execution_directory_capability"] =
                        execution["directory_capability"].clone();
                }
                if matches!(
                    name,
                    "Read"
                        | "Write"
                        | "Edit"
                        | "MultiEdit"
                        | "LS"
                        | "Glob"
                        | "Grep"
                        | "Bash"
                        | "Monitor"
                        | "shell_open"
                        | "ExecuteExtraTool"
                ) {
                    self.runtime.scope_input(&mut input, &context)?;
                }
                if matches!(
                    name,
                    "Bash" | "Monitor" | "WebFetch" | "shell_open" | "ExecuteExtraTool"
                ) {
                    self.runtime.traffic.apply(&mut input);
                }
                if name == "ExecuteExtraTool" {
                    let resolved = &input["resolved_tool"];
                    if resolved["kind"] == "mcp" {
                        let mut call = input.clone();
                        call["server"] = resolved["server"].clone();
                        call["tool_name"] = resolved["name"].clone();
                        call["arguments"] = input["params"].clone();
                        return crate::mcp::call(&self.runtime, &call, &context);
                    }
                    let mut definition = resolved.clone();
                    definition["params"] = input["params"].clone();
                    return crate::custom::execute(&self.runtime, &definition, &input, &context);
                }
                self.runtime.execute_tool(name, &input, &context)
            }
            Some("run.inspect" | "run.stop" | "run.force_stop") => {
                if ["run_id", "pid", "execution_id", "engine_epoch"]
                    .iter()
                    .any(|key| params.get(key).is_some())
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Run ownership comes only from the trusted context",
                    ));
                }
                let run = context.actor.run_id.as_deref().unwrap();
                if params["operation"] == "run.inspect" {
                    return Ok(self.runtime.inspect_run(run, &epoch));
                }
                if params["operation"] == "run.force_stop" {
                    let milliseconds = params
                        .get("deadline_ms")
                        .map(|v| {
                            v.as_u64()
                                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                                .ok_or_else(|| {
                                    Error::new(
                                        "INVALID_PARAMS",
                                        "Integer force-stop deadline required",
                                    )
                                })
                        })
                        .transpose()?
                        .unwrap_or(2000);
                    if milliseconds > 5000 {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Force-stop deadline exceeds 5000ms",
                        ));
                    }
                    return Ok(self.runtime.force_stop_run(
                        run,
                        &epoch,
                        std::time::Duration::from_millis(milliseconds),
                    ));
                }
                self.runtime.stop_run(run, &epoch);
                let mut result = self.runtime.inspect_run(run, &epoch);
                result["accepted"] = json!(true);
                Ok(result)
            }
            Some("host.events") => Ok(
                json!({"events":self.runtime.ptys.drain_events(context.actor.run_id.as_deref().unwrap(),&epoch)}),
            ),
            Some("secret.resolve") => Ok(
                json!({"value":self.secrets.resolve(params["secret_ref"].as_str().ok_or_else(|| Error::new("INVALID_PARAMS", "Secret reference required"))?)?}),
            ),
            _ => Err(Error::new(
                "METHOD_NOT_FOUND",
                "Unknown private host operation",
            )),
        }
    }
    pub fn invalidate(&self) {
        if let Some((epoch, _)) = self.binding.lock().unwrap().take() {
            self.runtime.stop_epoch(&epoch);
        }
        self.runtime.files.clear();
    }
    pub fn configure_native_logs(&self, root: &std::path::Path) -> Result<()> {
        self.runtime
            .logs
            .set_directory(root.canonicalize().map_err(Error::io)?);
        self.runtime.logs.activate()?;
        self.runtime
            .logs
            .record("Native host connected; tool supervision and durable log ingestion active");
        Ok(())
    }
    pub fn record_native_log(&self, text: &str) {
        self.runtime.logs.record(text);
    }
    fn resolve_config_secrets(&self, input: &mut Value) -> Result<()> {
        fn resolve(v: &mut Value, vault: &SecretVault, depth: usize) -> Result<()> {
            if depth > 32 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Secret configuration nesting exceeded",
                ));
            }
            if let Some(reference) = v.get("secret_ref").and_then(Value::as_str) {
                *v = json!(vault.resolve(reference)?);
                return Ok(());
            }
            match v {
                Value::Object(map) => {
                    for (key, value) in map.iter_mut() {
                        let sealed = value.get("secret_ref").is_some();
                        resolve(value, vault, depth + 1)?;
                        if sealed && matches!(key.as_str(), "env" | "headers") {
                            let raw = value.as_str().ok_or_else(|| {
                                Error::new("CORRUPT_STORE", "Sealed configuration map must be JSON")
                            })?;
                            let parsed: Value = serde_json::from_str(raw).map_err(|_| {
                                Error::new("CORRUPT_STORE", "Invalid sealed configuration JSON")
                            })?;
                            if !parsed.is_object() {
                                return Err(Error::new(
                                    "CORRUPT_STORE",
                                    "Sealed environment or headers must be an object",
                                ));
                            }
                            *value = parsed;
                        }
                    }
                }
                Value::Array(list) => {
                    for value in list {
                        resolve(value, vault, depth + 1)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        for key in [
            "network_config",
            "resolved_tool",
            "server",
            "config",
            "patch",
            "body",
        ] {
            if let Some(value) = input.get_mut(key) {
                resolve(value, &self.secrets, 0)?;
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_binding_and_agent_ownership_are_required() {
        let h = HostBridge::new(Arc::new(SecretVault::default()));
        h.bind("epoch_a", "workspace_a").unwrap();
        let mut context: Context = serde_json::from_value(json!({"actor":{"kind":"agent","agent_id":"agent_a","run_id":"run_a","engine_epoch":"epoch_a"},"task_id":null,"intent_id":null,"conversation_id":null})).unwrap();
        let call = |context: &Context| {
            h.execute_direct(
                "workspace_a",
                context,
                "tool.execute",
                json!({"name":"TaskList","input":{}}),
            )
        };
        assert_eq!(call(&context).unwrap()["tasks"], json!([]));
        context.actor.engine_epoch = "epoch_old".into();
        assert_eq!(call(&context).unwrap_err().code, "STALE_EPOCH");
        context.actor.engine_epoch = "epoch_a".into();
        context.actor.kind = "ui".into();
        assert_eq!(call(&context).unwrap_err().code, "PERMISSION_DENIED");
        assert!(h.bind("epoch_b", "workspace_a").is_err());
    }
    #[test]
    fn approved_skill_resources_match_scoped_read_and_bash_paths() {
        let resources = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let h = HostBridge::new(Arc::new(SecretVault::default()));
        let cap = h
            .runtime
            .capabilities
            .grant_directory(resources.path())
            .unwrap();
        let exec = h.runtime.capabilities.grant_directory(work.path()).unwrap();
        crate::skills::execute(&h.runtime.capabilities,"create_skill",&json!({"directory_capability":cap,"name":"approved","description":"Metadata only","instructions":"Read ${SKILL_DIR}/script.txt","mcp":"one,two"})).unwrap();
        crate::skills::execute(&h.runtime.capabilities,"update_skill_file",&json!({"directory_capability":cap,"name":"approved","file":"script.txt","content":"REAL-SCOPED-SKILL-FILE"})).unwrap();
        let epoch = artex_core::new_id("epoch");
        let workspace = artex_core::new_id("workspace");
        h.bind(&epoch, &workspace).unwrap();
        let context = Context {
            actor: artex_core::Actor {
                kind: "agent".into(),
                agent_id: Some(artex_core::new_id("agent")),
                run_id: Some(artex_core::new_id("run")),
                engine_epoch: epoch.clone(),
            },
            task_id: Some(artex_core::new_id("task")),
            intent_id: None,
            conversation_id: None,
        };
        let call = |operation: &str, params: Value, context: &Context| {
            h.execute_direct(&workspace, context, operation, params)
        };
        let catalog = call(
            "skill.catalog",
            json!({"directory_capability":cap}),
            &context,
        )
        .unwrap();
        assert_eq!(catalog[0]["name"], "approved");
        assert!(catalog[0].get("instructions").is_none());
        let loaded = call(
            "tool.execute", json!({"name":"Skill","input":{"directory_capability":cap,"execution_directory_capability":exec,"name":"approved"}}), &context,
        ).unwrap();
        let directory = loaded["skill_dir"].as_str().unwrap();
        let file = format!("{directory}/script.txt");
        assert!(loaded["instructions"].as_str().unwrap().contains(&file));
        assert!(
            work.path()
                .join("tasks")
                .join(context.task_id.as_ref().unwrap())
                .join(&file)
                .is_file()
        );
        assert!(!work.path().join(&file).exists());
        let read = call(
            "tool.execute",
            json!({"name":"Read","input":{"directory_capability":exec,"file_path":file}}),
            &context,
        )
        .unwrap();
        assert!(
            read["content"]
                .as_str()
                .unwrap()
                .contains("REAL-SCOPED-SKILL-FILE")
        );
        let command = if cfg!(windows) {
            format!("Get-Content -LiteralPath '{file}'")
        } else {
            format!("cat '{file}'")
        };
        let bash = call(
            "tool.execute", json!({"name":"Bash","input":{"directory_capability":exec,"command":command,"timeout_ms":10000}}), &context,
        ).unwrap();
        assert_eq!(bash["state"], "completed");
        assert!(
            bash["output"]
                .as_str()
                .unwrap()
                .contains("REAL-SCOPED-SKILL-FILE")
        );
        let mut forged = context;
        forged.actor.kind = "user".into();
        assert_eq!(
            call(
                "skill.catalog",
                json!({"directory_capability":cap}),
                &forged
            )
            .unwrap_err()
            .code,
            "PERMISSION_DENIED"
        );
    }
}
