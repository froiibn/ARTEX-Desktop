//! Task-local native working directories. The root execution capability is
//! never exposed as a fallback to scoped agent file operations.
use crate::{
    HostRuntime,
    tools::{atomic_copy, atomic_write},
};
use artex_core::{Context, Error, Result, valid_id};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
impl HostRuntime {
    pub(crate) fn scope_input(&self, input: &mut Value, context: &Context) -> Result<()> {
        let key = if let Some(id) = context.task_id.as_ref() {
            if !valid_id(id, "task") {
                return Err(Error::new("INVALID_PARAMS", "Typed task scope required"));
            }
            format!("tasks/{id}")
        } else if let Some(id) = context.conversation_id.as_ref() {
            if !valid_id(id, "conversation") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Typed conversation scope required",
                ));
            }
            format!("conversations/{id}")
        } else {
            return Ok(());
        };
        let _scope_guard = self.receipt_lock.lock().unwrap();
        if self
            .quarantined_scopes
            .lock()
            .unwrap()
            .contains_key(&(context.actor.engine_epoch.clone(), key.clone()))
        {
            return Err(Error::new(
                "CANCELLED",
                "Scoped working directory is quarantined",
            ));
        }
        let root = input["directory_capability"]
            .as_str()
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Execution root capability required"))?
            .to_string();
        let dir = self.capabilities.resolve(&root, &key, true)?;
        fs::create_dir_all(&dir).map_err(Error::io)?;
        let digest = hex::encode(Sha256::digest(
            format!("{}\0{root}\0{key}", context.actor.engine_epoch).as_bytes(),
        ));
        let scoped = format!("capability_{}", &digest[..32]);
        self.capabilities.grant_directory_as(&scoped, &dir)?;
        if let Some(run) = context.actor.run_id.as_ref() {
            self.scoped_runs
                .lock()
                .unwrap()
                .entry((context.actor.engine_epoch.clone(), key.clone()))
                .or_default()
                .insert(run.clone());
        }
        if let Some(paths) = input["attachment_paths"].as_array() {
            if paths.len() > 100 {
                return Err(Error::new("SIZE_LIMIT", "Attachment grant count exceeded"));
            }
            for path in paths {
                let relative = path.as_str().ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "Attachment relative path required")
                })?;
                if !relative.starts_with("attachments/") {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Only disclosed legacy attachments may be promoted",
                    ));
                }
                let target = self.capabilities.resolve(&scoped, relative, true)?;
                // Scoped native uploads already live in this run's task/chat
                // CWD. Promote shared legacy/staging paths only when absent.
                if target.is_file() {
                    continue;
                }
                let source = self.capabilities.resolve(&root, relative, false)?;
                if fs::metadata(&source).map_err(Error::io)?.len() > 512 * 1024 * 1024 {
                    return Err(Error::new("SIZE_LIMIT", "Attachment exceeds native budget"));
                }
                fs::create_dir_all(target.parent().unwrap()).map_err(Error::io)?;
                if !target.exists() {
                    atomic_copy(&source, &target)?;
                }
            }
        }
        input["directory_capability"] = json!(scoped);
        Ok(())
    }
    pub(crate) fn restore_quarantine(&self, input: &Value) -> Result<Value> {
        let _guard = self.receipt_lock.lock().unwrap();
        let (task, cap, name) = quarantine_key(input)?;
        let source =
            self.capabilities
                .resolve(cap, &format!(".artex-host/quarantine/{name}"), false)?;
        let target = self
            .capabilities
            .resolve(cap, &format!("tasks/{task}"), true)?;
        let receipt = self.capabilities.resolve(
            cap,
            &format!(".artex-host/quarantine/{name}.state.json"),
            true,
        )?;
        if source.exists() && target.exists() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Task directory changed while quarantined; restore requires reconciliation",
            ));
        }
        fs::create_dir_all(receipt.parent().unwrap()).map_err(Error::io)?;
        // Seal the inverse before restoring: a crashed or retried deletion command
        // must never remove the restored working directory a second time.
        atomic_write(&receipt, b"{\"state\":\"restoring\"}")?;
        if source.exists() {
            fs::create_dir_all(target.parent().unwrap()).map_err(Error::io)?;
            durable_rename(&source, &target)?;
        }
        atomic_write(&receipt, b"{\"state\":\"restored\"}")?;
        self.quarantined_scopes
            .lock()
            .unwrap()
            .retain(|(_, key), owner| key != &format!("tasks/{task}") || owner != &name);
        Ok(json!({"restored":target.exists(),"task_id":task}))
    }
    pub(crate) fn quarantine_task(&self, input: &Value, epoch: &str) -> Result<Value> {
        let _guard = self.receipt_lock.lock().unwrap();
        let (task, cap, name) = quarantine_key(input)?;
        let receipt = self.capabilities.resolve(
            cap,
            &format!(".artex-host/quarantine/{name}.state.json"),
            true,
        )?;
        if receipt.exists() {
            let state: Value = serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
            if matches!(state["state"].as_str(), Some("restoring" | "restored")) {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Deletion command was reversed; explicit new command required",
                ));
            }
        }
        let runs = self
            .scoped_runs
            .lock()
            .unwrap()
            .get(&(epoch.into(), format!("tasks/{task}")))
            .cloned()
            .unwrap_or_default();
        for run in &runs {
            self.stop_run(run, epoch);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while runs
            .iter()
            .any(|run| self.inspect_run(run, epoch)["cleanup_confirmed"] != true)
        {
            if Instant::now() >= deadline {
                return Err(Error::new("TIMEOUT", "Task process cleanup did not finish"));
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.quarantined_scopes
            .lock()
            .unwrap()
            .insert((epoch.into(), format!("tasks/{task}")), name.clone());
        let source = self
            .capabilities
            .resolve(cap, &format!("tasks/{task}"), false)?;
        let directory = self
            .capabilities
            .resolve(cap, ".artex-host/quarantine", true)?;
        fs::create_dir_all(&directory).map_err(Error::io)?;
        let target =
            self.capabilities
                .resolve(cap, &format!(".artex-host/quarantine/{name}"), true)?;
        if target.exists() {
            if source.exists() {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Task deletion command already quarantined a different directory",
                ));
            }
            return Ok(
                json!({"retired":true,"quarantined":true,"task_id":task,"quarantine_id":name,"secure_erase":false}),
            );
        }
        if source.exists() {
            atomic_write(&receipt, b"{\"state\":\"prepared\"}")?;
            durable_rename(&source, &target)?;
            atomic_write(&receipt, b"{\"state\":\"quarantined\"}")?;
            return Ok(
                json!({"retired":true,"quarantined":true,"task_id":task,"quarantine_id":name,"secure_erase":false}),
            );
        }
        fs::create_dir_all(receipt.parent().unwrap()).map_err(Error::io)?;
        atomic_write(&receipt, b"{\"state\":\"quarantined\"}")?;
        Ok(
            json!({"retired":true,"quarantined":false,"task_id":task,"quarantine_id":null,"secure_erase":false}),
        )
    }
    pub(crate) fn quarantine_conversation(&self, input: &Value, epoch: &str) -> Result<Value> {
        let _guard = self.receipt_lock.lock().unwrap();
        let (conversation, generation, cap, name) = conversation_quarantine_key(input)?;
        let key = format!("conversations/{conversation}");
        let receipt = self.capabilities.resolve(
            cap,
            &format!(".artex-host/quarantine/{name}.state.json"),
            true,
        )?;
        if receipt.exists() {
            let state: Value = serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
            if matches!(state["state"].as_str(), Some("restoring" | "restored")) {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Conversation quarantine was reversed; a new command is required",
                ));
            }
            if state["state"] == "committed" {
                let target = self.capabilities.resolve(
                    cap,
                    &format!(".artex-host/quarantine/{name}"),
                    false,
                )?;
                return Ok(
                    json!({"retired":true,"quarantined":target.exists(),"conversation_id":conversation,"trash_generation":generation,"quarantine_id":if target.exists(){Some(name)}else{None},"secure_erase":false}),
                );
            }
        }
        let runs = self.scope_runs(&key, epoch);
        for run in &runs {
            self.stop_run(run, epoch);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while runs
            .iter()
            .any(|run| self.inspect_run(run, epoch)["cleanup_confirmed"] != true)
        {
            if Instant::now() >= deadline {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Conversation descendant cleanup is not confirmed; files retained",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
        let source = self.capabilities.resolve(cap, &key, false)?;
        let target =
            self.capabilities
                .resolve(cap, &format!(".artex-host/quarantine/{name}"), true)?;
        if target.exists() && source.exists() {
            return Err(Error::new(
                "COMMAND_ID_REUSE",
                "Conversation quarantine already owns a different directory",
            ));
        }
        fs::create_dir_all(receipt.parent().unwrap()).map_err(Error::io)?;
        atomic_write(&receipt, b"{\"state\":\"prepared\"}")?;
        self.quarantined_scopes
            .lock()
            .unwrap()
            .insert((epoch.into(), key.clone()), name.clone());
        if source.exists() {
            fs::create_dir_all(target.parent().unwrap()).map_err(Error::io)?;
            atomic_write(&receipt, b"{\"state\":\"prepared\"}")?;
            durable_rename(&source, &target)?;
            atomic_write(&receipt, b"{\"state\":\"quarantined\"}")?;
        }
        atomic_write(&receipt, b"{\"state\":\"quarantined\"}")?;
        Ok(
            json!({"retired":true,"quarantined":target.exists(),"conversation_id":conversation,"trash_generation":generation,"quarantine_id":if target.exists(){Some(name)}else{None},"secure_erase":false}),
        )
    }
    pub(crate) fn complete_conversation_quarantine(&self, input: &Value) -> Result<Value> {
        let _guard = self.receipt_lock.lock().unwrap();
        let (conversation, generation, cap, name) = conversation_quarantine_key(input)?;
        let receipt = self.capabilities.resolve(
            cap,
            &format!(".artex-host/quarantine/{name}.state.json"),
            false,
        )?;
        let state: Value = serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
        if !matches!(state["state"].as_str(), Some("quarantined" | "committed")) {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Quarantine is not ready for graph commit confirmation",
            ));
        }
        atomic_write(&receipt, b"{\"state\":\"committed\"}")?;
        self.quarantined_scopes
            .lock()
            .unwrap()
            .retain(|(_, scope), owner| {
                scope != &format!("conversations/{conversation}") || owner != &name
            });
        Ok(
            json!({"completed":true,"conversation_id":conversation,"trash_generation":generation,"secure_erase":false}),
        )
    }
    pub(crate) fn restore_conversation_quarantine(&self, input: &Value) -> Result<Value> {
        let _guard = self.receipt_lock.lock().unwrap();
        let (conversation, generation, cap, name) = conversation_quarantine_key(input)?;
        let key = format!("conversations/{conversation}");
        if self
            .quarantined_scopes
            .lock()
            .unwrap()
            .iter()
            .any(|((_, scope), owner)| scope == &key && owner != &name)
        {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "A different quarantine generation owns this conversation",
            ));
        }
        let source =
            self.capabilities
                .resolve(cap, &format!(".artex-host/quarantine/{name}"), false)?;
        let target = self.capabilities.resolve(cap, &key, true)?;
        let receipt = self.capabilities.resolve(
            cap,
            &format!(".artex-host/quarantine/{name}.state.json"),
            true,
        )?;
        if source.exists() && target.exists() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Conversation directory changed while quarantined; reconciliation required",
            ));
        }
        // An unknown inverse must not unblock a different deletion generation.
        if !receipt.exists() {
            return Err(Error::new(
                "NOT_FOUND",
                "Conversation quarantine receipt does not exist",
            ));
        }
        let state: Value = serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
        if state["state"] == "committed" {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Committed quarantine cannot be reversed by an old failed command",
            ));
        }
        atomic_write(&receipt, b"{\"state\":\"restoring\"}")?;
        if source.exists() {
            fs::create_dir_all(target.parent().unwrap()).map_err(Error::io)?;
            durable_rename(&source, &target)?;
        }
        atomic_write(&receipt, b"{\"state\":\"restored\"}")?;
        self.quarantined_scopes
            .lock()
            .unwrap()
            .retain(|(_, scope), owner| scope != &key || owner != &name);
        Ok(
            json!({"restored":target.exists(),"conversation_id":conversation,"trash_generation":generation}),
        )
    }
}
fn conversation_quarantine_key(input: &Value) -> Result<(&str, &str, &str, String)> {
    let conversation = input["conversation_id"]
        .as_str()
        .filter(|id| valid_id(id, "conversation"))
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Typed conversation ID required"))?;
    let generation = input["trash_generation"]
        .as_str()
        .filter(|id| valid_id(id, "trash"))
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Typed trash generation required"))?;
    let command = input["command_id"]
        .as_str()
        .filter(|id| valid_id(id, "command"))
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Typed quarantine command required"))?;
    let cap = input["directory_capability"]
        .as_str()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "Execution root capability required"))?;
    let digest = hex::encode(Sha256::digest(
        format!("{generation}\0{command}").as_bytes(),
    ));
    Ok((
        conversation,
        generation,
        cap,
        format!("{conversation}-{}", &digest[..32]),
    ))
}
fn quarantine_key(input: &Value) -> Result<(&str, &str, String)> {
    let task = input["task_id"]
        .as_str()
        .filter(|id| valid_id(id, "task"))
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Typed task id required"))?;
    let command = input["command_id"]
        .as_str()
        .filter(|id| valid_id(id, "command"))
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Native command id required"))?;
    let cap = input["directory_capability"]
        .as_str()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "Execution root capability required"))?;
    Ok((
        task,
        cap,
        format!(
            "{task}-{}",
            &hex::encode(Sha256::digest(command.as_bytes()))[..32]
        ),
    ))
}
fn durable_rename(source: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
        }
        let from = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let to = target
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 8) } == 0 {
            return Err(Error::io(std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, target).map_err(Error::io)?;
        for parent in [source.parent(), target.parent()].into_iter().flatten() {
            fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(Error::io)?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Actor, new_id};
    #[test]
    fn conversation_quarantine_is_reversible_scoped_and_empty_is_retired() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostRuntime::default();
        let cap = host.capabilities.grant_directory(dir.path()).unwrap();
        let epoch = new_id("epoch");
        let conversation = new_id("conversation");
        let other = new_id("conversation");
        for id in [&conversation, &other] {
            fs::create_dir_all(dir.path().join("conversations").join(id)).unwrap();
            fs::write(dir.path().join("conversations").join(id).join("note"), id).unwrap();
        }
        fs::create_dir(dir.path().join("attachments")).unwrap();
        fs::write(dir.path().join("attachments/shared"), "retain").unwrap();
        let params = json!({"conversation_id":conversation,"trash_generation":new_id("trash"),"command_id":new_id("command"),"directory_capability":cap});
        let quarantined = host.quarantine_conversation(&params, &epoch).unwrap();
        assert_eq!(quarantined["retired"], true);
        assert_eq!(quarantined["quarantined"], true);
        assert!(
            !dir.path()
                .join("conversations")
                .join(&conversation)
                .exists()
        );
        assert!(
            dir.path()
                .join("conversations")
                .join(&other)
                .join("note")
                .is_file()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("attachments/shared")).unwrap(),
            "retain"
        );
        assert_eq!(
            host.restore_conversation_quarantine(&params).unwrap()["restored"],
            true
        );
        assert_eq!(
            host.quarantine_conversation(&params, &epoch)
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        let empty = json!({"conversation_id":new_id("conversation"),"trash_generation":new_id("trash"),"command_id":new_id("command"),"directory_capability":cap});
        let result = host.quarantine_conversation(&empty, &epoch).unwrap();
        assert_eq!(result["retired"], true);
        assert_eq!(result["quarantined"], false);
        host.complete_conversation_quarantine(&empty).unwrap();
        assert_eq!(
            host.restore_conversation_quarantine(&empty)
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        let mut bad = empty.clone();
        bad["conversation_id"] = json!("../../attachments");
        assert_eq!(
            host.quarantine_conversation(&bad, &epoch).unwrap_err().code,
            "INVALID_PARAMS"
        );
    }
    #[test]
    fn scoped_files_are_isolated_and_reversal_cannot_redelete() {
        let dir = tempfile::tempdir().unwrap();
        let host = HostRuntime::default();
        let cap = host.capabilities.grant_directory(dir.path()).unwrap();
        let epoch = new_id("epoch");
        let a = new_id("task");
        let b = new_id("task");
        let mut context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: epoch.clone(),
            },
            task_id: Some(a.clone()),
            intent_id: None,
            conversation_id: None,
        };
        let mut input = json!({"directory_capability":cap});
        host.scope_input(&mut input, &context).unwrap();
        host.execute_tool("Write",&json!({"directory_capability":input["directory_capability"],"file_path":"note.txt","content":"task-a"}),&context).unwrap();
        context.task_id = Some(b.clone());
        let mut second = json!({"directory_capability":cap});
        host.scope_input(&mut second, &context).unwrap();
        assert_ne!(
            input["directory_capability"],
            second["directory_capability"]
        );
        assert!(
            !host
                .capabilities
                .resolve(
                    second["directory_capability"].as_str().unwrap(),
                    "note.txt",
                    false
                )
                .unwrap()
                .exists()
        );
        let delete = json!({"task_id":a,"command_id":new_id("command"),"directory_capability":cap});
        assert_eq!(
            host.quarantine_task(&delete, &epoch).unwrap()["quarantined"],
            true
        );
        assert_eq!(
            host.quarantine_task(&delete, &epoch).unwrap()["quarantined"],
            true
        );
        assert_eq!(host.restore_quarantine(&delete).unwrap()["restored"], true);
        assert_eq!(
            host.quarantine_task(&delete, &epoch).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("tasks").join(a).join("note.txt")).unwrap(),
            "task-a"
        );
    }
}
