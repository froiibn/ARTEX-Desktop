use crate::{CapabilityRegistry, ExecutionRequest, ExecutionSupervisor};
use artex_core::{Context, Error, Result, new_id};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, Read, Write},
    path::Path,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
#[derive(Default)]
pub struct HostRuntime {
    pub capabilities: CapabilityRegistry,
    pub executions: ExecutionSupervisor,
    cancelled: Mutex<HashSet<(String, String)>>,
    closed_epochs: Mutex<HashSet<String>>,
    pub files: crate::native_files::NativeFiles,
    pub(crate) receipt_lock: Mutex<()>,
    pub ptys: crate::pty::PtySupervisor,
    pub traffic: crate::traffic::TrafficManager,
    pub(crate) scoped_runs: Mutex<HashMap<(String, String), HashSet<String>>>,
    pub(crate) quarantined_scopes: Mutex<HashMap<(String, String), String>>,
    pub logs: crate::logs::NativeLogs,
}
impl HostRuntime {
    pub fn scope_runs(&self, scope: &str, epoch: &str) -> Vec<String> {
        self.scoped_runs
            .lock()
            .unwrap()
            .get(&(epoch.into(), scope.into()))
            .map(|runs| runs.iter().cloned().collect())
            .unwrap_or_default()
    }
    pub fn inspect_run(&self, run: &str, epoch: &str) -> Value {
        let mut executions: Vec<Value> = self
            .executions
            .list(run, epoch)
            .into_iter()
            .map(|status| {
                let mut value = serde_json::to_value(status).unwrap_or(Value::Null);
                if let Some(map) = value.as_object_mut() {
                    map.remove("output");
                    let error = map.remove("capture_error").unwrap_or(Value::Null);
                    map.insert("cleanup_error".into(), error);
                }
                value
            })
            .collect();
        executions.extend(self.ptys.inspect(run, epoch));
        executions.sort_by(|a, b| a["execution_id"].as_str().cmp(&b["execution_id"].as_str()));
        let confirmed = executions.iter().all(|e| e["cleanup_confirmed"] == true);
        json!({"run_id":run,"engine_epoch":epoch,"stop_requested":self.cancelled.lock().unwrap().contains(&(run.into(),epoch.into())),"cleanup_confirmed":confirmed,"cleanup_state":if confirmed{"confirmed"}else{"unconfirmed"},"executions":executions})
    }
    pub fn force_stop_run(&self, run: &str, epoch: &str, deadline: Duration) -> Value {
        self.stop_run(run, epoch);
        self.executions.force_stop_run(run, epoch);
        self.ptys.force_stop_run(run, epoch);
        let end = Instant::now() + deadline.min(Duration::from_secs(5));
        loop {
            let mut result = self.inspect_run(run, epoch);
            result["accepted"] = json!(true);
            if result["cleanup_confirmed"] == true || Instant::now() >= end {
                return result;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub(crate) fn epoch_closed(&self, epoch: &str) -> bool {
        self.closed_epochs.lock().unwrap().contains(epoch)
    }
    pub(crate) fn context_cancelled(&self, context: &Context) -> bool {
        self.epoch_closed(&context.actor.engine_epoch)
            || context.actor.run_id.as_ref().is_some_and(|run| {
                self.cancelled
                    .lock()
                    .unwrap()
                    .contains(&(run.clone(), context.actor.engine_epoch.clone()))
            })
    }
    pub fn supported_tools() -> &'static [&'static str] {
        &[
            "Read",
            "Write",
            "Edit",
            "MultiEdit",
            "LS",
            "Bash",
            "TaskOutput",
            "TaskStop",
            "sleep",
            "TaskList",
            "Glob",
            "Grep",
            "Monitor",
            "WebFetch",
            "web_search",
            "Skill",
            "create_skill",
            "update_skill_file",
            "shell_open",
            "shell_send",
            "shell_read",
            "shell_close",
            "shell_list",
        ]
    }
    pub fn stop_run(&self, run: &str, epoch: &str) {
        self.cancelled
            .lock()
            .unwrap()
            .insert((run.into(), epoch.into()));
        self.executions.stop_run(run, epoch);
        self.ptys.stop_run(run, epoch);
    }
    pub fn stop_epoch(&self, epoch: &str) {
        self.closed_epochs.lock().unwrap().insert(epoch.into());
        self.executions.stop_epoch(epoch);
        self.ptys.stop_epoch(epoch);
        self.traffic.stop_epoch(epoch);
    }
    pub fn epoch_active(&self, epoch: &str) -> bool {
        self.executions.epoch_active(epoch) || self.ptys.epoch_active(epoch)
    }
    pub fn execute_tool(&self, name: &str, input: &Value, context: &Context) -> Result<Value> {
        if context.actor.kind != "agent" || context.actor.agent_id.is_none() {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Agent tool ownership required",
            ));
        }
        let run = context
            .actor
            .run_id
            .as_deref()
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Tool requires a current run"))?;
        let epoch = &context.actor.engine_epoch;
        if self.closed_epochs.lock().unwrap().contains(epoch)
            || self
                .cancelled
                .lock()
                .unwrap()
                .contains(&(run.into(), epoch.clone()))
        {
            return Err(Error::new("CANCELLED", "Run has ended"));
        }
        if name == "TaskList" {
            return Ok(json!({"tasks":self.executions.list(run,epoch)}));
        }
        if name.starts_with("shell_") {
            return self.ptys.execute(&self.capabilities, name, input, context);
        }
        if name == "Glob" || name == "Grep" {
            return crate::search::execute(&self.capabilities, name, input);
        }
        if name == "WebFetch" || name == "web_search" {
            return crate::network::execute(self, name, input, context);
        }
        if matches!(name, "Skill" | "create_skill" | "update_skill_file") {
            return crate::skills::execute(&self.capabilities, name, input);
        }
        if name == "custom.execute" {
            return crate::custom::execute(self, &input["definition"], input, context);
        }
        if name == "mcp.call" {
            return crate::mcp::call(self, input, context);
        }
        if name == "TaskOutput" || name == "TaskStop" {
            let id = input.get("task_id").and_then(Value::as_str).unwrap_or("");
            if name == "TaskStop" && id.is_empty() {
                self.executions.stop_run(run, epoch);
                return Ok(json!({"accepted":true}));
            }
            if name == "TaskOutput" && input.get("block").and_then(Value::as_bool).unwrap_or(false)
            {
                let end = Instant::now()
                    + Duration::from_millis(
                        input
                            .get("timeout_ms")
                            .and_then(Value::as_u64)
                            .unwrap_or(30000)
                            .min(3600000),
                    );
                while Instant::now() < end
                    && self.executions.status(id, run, epoch)?.state == "running"
                {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            let status = if name == "TaskStop" {
                self.executions.stop(id, run, epoch)?
            } else {
                let max = input
                    .get("max_bytes")
                    .and_then(Value::as_u64)
                    .unwrap_or(30000)
                    .min(65536);
                self.executions.output_file_tail(id, run, epoch, max)?
            };
            return serde_json::to_value(status)
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "Cannot encode execution"));
        }
        if name == "sleep" {
            let seconds = input
                .get("seconds")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Integer seconds required"))?;
            if seconds == 0 || seconds > 3600 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Sleep range is 1..3600 seconds",
                ));
            }
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end {
                if self.closed_epochs.lock().unwrap().contains(epoch)
                    || self
                        .cancelled
                        .lock()
                        .unwrap()
                        .contains(&(run.into(), epoch.clone()))
                {
                    return Err(Error::new("CANCELLED", "Run stopped during sleep"));
                }
                thread::sleep(Duration::from_millis(20));
            }
            return Ok(json!({"slept_seconds":seconds}));
        }
        let cap = text(input, "directory_capability")?;
        if name == "Bash" || name == "Monitor" {
            // Match norma/tool/task.go::shellCmd on each platform.
            let executable = if cfg!(windows) {
                "powershell.exe"
            } else {
                "bash"
            };
            let command = text(input, "command")?;
            let execution_id = input
                .get("execution_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| new_id("exec"));
            let args = if cfg!(windows) {
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-Command".into(),
                    command.into(),
                ]
            } else {
                vec!["-c".into(), command.into()]
            };
            let timeout = input
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(120000)
                .min(600000);
            let r = ExecutionRequest {
                execution_id: execution_id.clone(),
                run_id: run.into(),
                engine_epoch: epoch.clone(),
                executable: executable.into(),
                args,
                cwd: self.capabilities.root(cap)?,
                // Foreground wait is not the external process lifetime. The run
                // controller supplies its execution budget and stop_run cancels
                // processes when a run ends; foreground expiry auto-backgrounds.
                timeout_ms: input
                    .get("execution_timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(3600000)
                    .min(3600000),
            };
            let environment = input["native_environment"]
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            self.executions.execute_with_input(r, None, &environment)?;
            if name != "Monitor"
                && !input
                    .get("run_in_background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                let start = Instant::now();
                while start.elapsed() < Duration::from_millis(timeout) {
                    let s = self.executions.status(&execution_id, run, epoch)?;
                    if !matches!(s.state.as_str(), "running") {
                        return serde_json::to_value(s).map_err(|_| {
                            Error::new("EXTERNAL_FAILURE", "Execution encoding failed")
                        });
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                let base = command
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("");
                if base == "sleep" {
                    self.executions.stop(&execution_id, run, epoch)?;
                    return Err(Error::new(
                        "TIMEOUT",
                        "Sleep command exceeded foreground wait and was stopped",
                    ));
                }
            }
            let status = self.executions.status(&execution_id, run, epoch)?;
            return Ok(
                json!({"execution_id":execution_id,"task_id":execution_id,"state":status.state,"output":status.output,"output_path":status.output_path,"first_cursor":status.first_cursor,"last_cursor":status.last_cursor,"truncated":status.truncated,"capture_error":status.capture_error}),
            );
        }
        let relative = input
            .get("file_path")
            .or_else(|| input.get("path"))
            .and_then(Value::as_str)
            .unwrap_or(".");
        let path = self
            .capabilities
            .resolve(cap, relative, name != "Read" && name != "LS")?;
        match name {
            "Read" => {
                let offset = input
                    .get("offset")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1)
                    .min(1000000) as usize;
                let limit = input
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(2000)
                    .min(2000) as usize;
                let mut reader = std::io::BufReader::new(fs::File::open(path).map_err(Error::io)?);
                let mut output = String::new();
                for line_number in 1..offset.saturating_add(limit) {
                    let mut bytes = Vec::new();
                    let size = reader
                        .by_ref()
                        .take(65537)
                        .read_until(b'\n', &mut bytes)
                        .map_err(Error::io)?;
                    if size == 0 {
                        break;
                    }
                    if size > 65536 {
                        return Err(Error::new("SIZE_LIMIT", "File line exceeds 64 KiB"));
                    }
                    if bytes.contains(&0) {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Binary file requires blob transfer",
                        ));
                    }
                    let line = std::str::from_utf8(&bytes)
                        .map_err(|_| Error::new("INVALID_PARAMS", "File is not UTF-8 text"))?;
                    if line_number >= offset {
                        output.push_str(&format!(
                            "{}\t{}\n",
                            line_number,
                            line.trim_end_matches(['\n', '\r'])
                        ));
                        if output.len() > 65536 {
                            return Err(Error::new(
                                "SIZE_LIMIT",
                                "Read chunk exceeds 64 KiB; reduce limit",
                            ));
                        }
                    }
                }
                Ok(json!({"content":output,"offset":offset,"limit":limit}))
            }
            "LS" => {
                let mut entries = vec![];
                for entry in fs::read_dir(path).map_err(Error::io)? {
                    let entry = entry.map_err(Error::io)?;
                    if entries.len() >= 10000 {
                        return Err(Error::new("SIZE_LIMIT", "Directory exceeds 10000 entries"));
                    }
                    entries.push(json!({"name":entry.file_name().to_string_lossy(),"directory":entry.file_type().map_err(Error::io)?.is_dir()}));
                }
                entries.sort_by_key(|e| e["name"].as_str().unwrap().to_owned());
                Ok(json!({"entries":entries}))
            }
            "Write" => {
                atomic_write(&path, text(input, "content")?.as_bytes())?;
                Ok(json!({"written":true}))
            }
            "Edit" | "MultiEdit" => {
                if fs::metadata(&path).map_err(Error::io)?.len() > 16 * 1024 * 1024 {
                    return Err(Error::new("SIZE_LIMIT", "Edit supports files up to 16 MiB"));
                }
                let mut content = fs::read_to_string(&path).map_err(Error::io)?;
                let edits = if name == "MultiEdit" {
                    input
                        .get("edits")
                        .and_then(Value::as_array)
                        .ok_or_else(|| Error::new("INVALID_PARAMS", "Edits array required"))?
                        .clone()
                } else {
                    vec![input.clone()]
                };
                for edit in edits {
                    let old = text(&edit, "old_string")?;
                    let new = text(&edit, "new_string")?;
                    let n = content.matches(old).count();
                    if old.is_empty()
                        || n == 0
                        || (!edit
                            .get("replace_all")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                            && n != 1)
                    {
                        return Err(Error::new(
                            "VERSION_CONFLICT",
                            "Edit match absent or ambiguous",
                        ));
                    }
                    content = if edit
                        .get("replace_all")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        content.replace(old, new)
                    } else {
                        content.replacen(old, new, 1)
                    };
                }
                atomic_write(&path, content.as_bytes())?;
                Ok(json!({"edited":true}))
            }
            _ => Err(Error::new(
                "FEATURE_DISABLED",
                "Host tool is not implemented",
            )),
        }
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("INVALID_PARAMS", &format!("Missing {key}")))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "No parent path"))?;
    if !parent.is_dir() {
        return Err(Error::new("NOT_FOUND", "Parent directory does not exist"));
    }
    let temp = parent.join(format!(".{}.tmp", new_id("write")));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(Error::io)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(Error::io)?;
    drop(file);
    install_temp(&temp, path)
}
fn install_temp(temp: &Path, path: &Path) -> Result<()> {
    #[cfg(not(windows))]
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "No parent path"))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            if MoveFileExW(from.as_ptr(), to.as_ptr(), 1 | 8) == 0 {
                let _ = fs::remove_file(temp);
                return Err(Error::io(std::io::Error::last_os_error()));
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(&temp, path).map_err(Error::io)?;
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(Error::io)?;
    }
    Ok(())
}
/// Stream a native selected file without materializing its content in RAM.
pub fn atomic_copy(source: &Path, target: &Path) -> Result<u64> {
    let mut input = fs::File::open(source).map_err(Error::io)?;
    let metadata = input.metadata().map_err(Error::io)?;
    if !metadata.is_file() {
        return Err(Error::new("INVALID_PARAMS", "Regular file required"));
    }
    let length = metadata.len();
    if length > 512 * 1024 * 1024 {
        return Err(Error::new("SIZE_LIMIT", "Native file exceeds 512 MiB"));
    }
    let parent = target
        .parent()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "No parent path"))?;
    let temp = parent.join(format!(".{}.tmp", new_id("copy")));
    let result = (|| {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(Error::io)?;
        let copied = std::io::copy(
            &mut std::io::Read::by_ref(&mut input).take(length + 1),
            &mut output,
        )
        .map_err(Error::io)?;
        if copied != length || input.metadata().map_err(Error::io)?.len() != length {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Selected file changed during copy",
            ));
        }
        output.sync_all().map_err(Error::io)?;
        drop(output);
        install_temp(&temp, target)?;
        Ok(copied)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_file_copy_is_atomic_and_preserves_target_on_limit_failure() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, vec![42; 3 * 1024 * 1024]).unwrap();
        fs::write(&target, b"old").unwrap();
        assert_eq!(atomic_copy(&source, &target).unwrap(), 3 * 1024 * 1024);
        assert_eq!(fs::metadata(&target).unwrap().len(), 3 * 1024 * 1024);
        let file = fs::File::create(&source).unwrap();
        file.set_len(512 * 1024 * 1024 + 1).unwrap();
        assert_eq!(
            atomic_copy(&source, &target).unwrap_err().code,
            "SIZE_LIMIT"
        );
        assert_eq!(fs::metadata(&target).unwrap().len(), 3 * 1024 * 1024);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
    #[test]
    fn multi_edit_failure_leaves_original() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("x"), "one two").unwrap();
        let h = HostRuntime::default();
        let cap = h.capabilities.grant_directory(d.path()).unwrap();
        let ctx = Context {
            actor: artex_core::Actor {
                kind: "agent".into(),
                agent_id: Some("agent_x".into()),
                run_id: Some("run_x".into()),
                engine_epoch: "epoch".into(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        assert!(h.execute_tool("MultiEdit",&json!({"directory_capability":cap,"file_path":"x","edits":[{"old_string":"one","new_string":"changed"},{"old_string":"absent","new_string":"bad"}]}),&ctx).is_err());
        assert_eq!(fs::read_to_string(d.path().join("x")).unwrap(), "one two");
    }
}
