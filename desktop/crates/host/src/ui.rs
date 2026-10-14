use crate::{
    HostRuntime,
    tools::{atomic_copy, atomic_write},
};
use artex_core::{Context, Error, Result};
use serde_json::{Value, json};
use std::{fs, io::Read, path::Path};
pub const METHODS: &[&str] = &[
    "nativeFileRead",
    "nativeFileWrite",
    "nativeFileSave",
    "nativeSkillMetaRead",
    "nativeSkillMetaWrite",
    "workspaceList",
    "workspaceRead",
    "workspaceWrite",
    "workspaceMkdir",
    "workspaceDelete",
    "workspaceUpload",
    "workspaceDownload",
    "skills",
    "createSkill",
    "uploadSkill",
    "deleteSkill",
    "updateSkillMeta",
    "createSkillDir",
    "skillFiles",
    "readSkillFile",
    "writeSkillFile",
    "deleteSkillPath",
    "chatUpload",
    "detectPython",
    "testCustomTool",
    "refreshMcpServer",
    "testWebSearch",
];
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", &format!("Missing {key}")))
}
impl HostRuntime {
    pub fn execute_ui(&self, method: &str, input: &Value, context: &Context) -> Result<Value> {
        let method = method.strip_prefix("ui.").unwrap_or(method);
        if context.actor.kind != "user"
            || context.actor.agent_id.is_some()
            || context.actor.run_id.is_some()
            || !METHODS.contains(&method)
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Native UI operation not permitted",
            ));
        }
        if method == "workspaceDownload" {
            return self.execute_ui_inner(method, input, context);
        }
        if !matches!(
            method,
            "workspaceWrite"
                | "nativeFileWrite"
                | "nativeFileSave"
                | "nativeSkillMetaWrite"
                | "workspaceMkdir"
                | "workspaceDelete"
                | "workspaceUpload"
                | "workspaceDownload"
                | "createSkill"
                | "uploadSkill"
                | "deleteSkill"
                | "updateSkillMeta"
                | "createSkillDir"
                | "writeSkillFile"
                | "deleteSkillPath"
                | "chatUpload"
                | "testCustomTool"
        ) {
            return self.execute_ui_inner(method, input, context);
        }
        let _lock = self.receipt_lock.lock().unwrap();
        let command = input["command_id"]
            .as_str()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Native effect command id required"))?;
        let cap = text(input, "directory_capability")?;
        use sha2::{Digest, Sha256};
        if self.epoch_closed(&context.actor.engine_epoch) {
            return Err(Error::new("STALE_EPOCH", "Native epoch ended"));
        }
        let mut semantic = input.clone();
        if let Some(map) = semantic.as_object_mut() {
            for key in ["directory_capability", "invocation_id"] {
                map.remove(key);
            }
        }
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(method, &semantic))?));
        let receipts = self
            .capabilities
            .resolve(cap, ".artex-host/receipts", true)?;
        fs::create_dir_all(&receipts).map_err(Error::io)?;
        let receipt = receipts.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(command.as_bytes()))
        ));
        if receipt.exists() {
            if fs::metadata(&receipt).map_err(Error::io)?.len() > 1_048_576 {
                return Err(Error::new("CORRUPT_STORE", "Oversized native receipt"));
            }
            let existing: Value = serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
            if existing["fingerprint"] != fingerprint {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Native effect command reused with changed parameters",
                ));
            }
            if existing["state"] == "completed" {
                return Ok(existing["result"].clone());
            }
            if existing["state"] == "failed" {
                return Err(serde_json::from_value(existing["error"].clone())
                    .map_err(|_| Error::new("CORRUPT_STORE", "Invalid native effect receipt"))?);
            }
            let mut error = Error::new(
                "RECOVERY_REQUIRED",
                "Native effect outcome is unknown; reconcile before retry",
            );
            error.recovery_required = true;
            return Err(error);
        }
        atomic_write(
            &receipt,
            serde_json::to_vec(&json!({"fingerprint":fingerprint,"state":"prepared"}))?.as_slice(),
        )?;
        let result = self.execute_ui_inner(method, input, context);
        let entry = match &result {
            Ok(value) => json!({"fingerprint":fingerprint,"state":"completed","result":value}),
            Err(error) => json!({"fingerprint":fingerprint,"state":"failed","error":error}),
        };
        atomic_write(&receipt, &serde_json::to_vec(&entry)?)?;
        result
    }
    fn execute_ui_inner(&self, method: &str, input: &Value, context: &Context) -> Result<Value> {
        if context.actor.kind != "user"
            || context.actor.agent_id.is_some()
            || context.actor.run_id.is_some()
            || !METHODS.contains(&method)
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Native UI operation not permitted",
            ));
        }
        if self.epoch_closed(&context.actor.engine_epoch) {
            return Err(Error::new("STALE_EPOCH", "UI native epoch ended"));
        }
        let mut internal_context = context.clone();
        if let Some(command) = input["command_id"].as_str() {
            internal_context.actor.run_id = Some(format!("ui_{command}"));
        }
        let cap = text(input, "directory_capability")?;
        if matches!(method, "nativeSkillMetaRead" | "nativeSkillMetaWrite") {
            return crate::native_editor::skill_meta(&self.capabilities, method, input);
        }
        if method == "nativeFileSave" {
            return self.files.save_path(
                text(input, "file_capability")?,
                Path::new(text(input, "destination")?),
            );
        }
        if matches!(method, "nativeFileRead" | "nativeFileWrite") {
            return crate::native_editor::execute(&self.capabilities, method, input);
        }
        if matches!(
            method,
            "skills"
                | "createSkill"
                | "deleteSkill"
                | "updateSkillMeta"
                | "createSkillDir"
                | "skillFiles"
                | "readSkillFile"
                | "writeSkillFile"
                | "deleteSkillPath"
        ) {
            return crate::skills::execute(&self.capabilities, method, input);
        }
        if method == "uploadSkill" {
            return self.upload_skill(cap, input);
        }
        let relative = input["path"].as_str().unwrap_or(".");
        let path = self.capabilities.resolve(
            cap,
            if relative.is_empty() { "." } else { relative },
            true,
        )?;
        match method {
            "workspaceList" => {
                let mut entries = vec![];
                for e in fs::read_dir(&path).map_err(Error::io)? {
                    let e = e.map_err(Error::io)?;
                    if e.file_name() == ".artex-host" {
                        continue;
                    }
                    let ft = e.file_type().map_err(Error::io)?;
                    if ft.is_symlink() {
                        continue;
                    }
                    let meta = e.metadata().map_err(Error::io)?;
                    let p = Path::new(relative)
                        .join(e.file_name())
                        .to_string_lossy()
                        .replace('\\', "/");
                    entries.push(json!({"name":e.file_name().to_string_lossy(),"path":p,"dir":meta.is_dir(),"size":meta.len(),"mtime":meta.modified().ok().map(|t|t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())}));
                    if entries.len() > 10000 {
                        return Err(Error::new(
                            "SIZE_LIMIT",
                            "Workspace listing budget exceeded",
                        ));
                    }
                }
                entries.sort_by_key(|e| e["name"].as_str().unwrap().to_owned());
                Ok(json!({"path":relative,"entries":entries}))
            }
            "workspaceRead" => {
                let size = fs::metadata(&path).map_err(Error::io)?.len();
                if size > 2 * 1024 * 1024 {
                    return Ok(json!({"path":relative,"size":size,"binary":true,"too_large":true}));
                }
                let bytes = fs::read(&path).map_err(Error::io)?;
                let binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
                Ok(
                    json!({"path":relative,"size":size,"content":if binary{String::new()}else{String::from_utf8(bytes).unwrap()},"binary":binary}),
                )
            }
            "workspaceWrite" => {
                atomic_write(&path, text(input, "content")?.as_bytes())?;
                Ok(json!({"ok":true,"path":relative}))
            }
            "workspaceMkdir" => {
                fs::create_dir_all(path).map_err(Error::io)?;
                Ok(json!({"ok":true,"path":relative}))
            }
            "workspaceDelete" => {
                let root = self.capabilities.root(cap)?;
                if path == root {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Workspace root deletion rejected",
                    ));
                }
                if path.exists() {
                    if path.is_dir() {
                        fs::remove_dir_all(path).map_err(Error::io)?
                    } else {
                        fs::remove_file(path).map_err(Error::io)?
                    }
                }
                Ok(json!({"ok":true}))
            }
            "workspaceUpload" => {
                let dir = self.capabilities.resolve(cap, text(input, "dir")?, true)?;
                let files = input["files"]
                    .as_array()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "File capabilities required"))?;
                for token in files {
                    let (source, name) = self.files.get(token.as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "File capability required")
                    })?)?;
                    let target = self.capabilities.resolve(
                        cap,
                        &dir.join(name)
                            .strip_prefix(self.capabilities.root(cap)?)
                            .map_err(|_| {
                                Error::new("PERMISSION_DENIED", "Upload escaped workspace")
                            })?
                            .to_string_lossy(),
                        true,
                    )?;
                    atomic_copy(&source, &target)?;
                }
                for token in files
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::HashSet<_>>()
                {
                    self.files.release(token)?;
                }
                Ok(json!({"uploaded":files.len()}))
            }
            "workspaceDownload" => self
                .files
                .stage_path(&path, || self.epoch_closed(&context.actor.engine_epoch)),
            "chatUpload" => {
                let scope = text(input, "scope")?;
                let id = text(input, "id")?;
                let base = match scope {
                    "task" if artex_core::valid_id(id, "task") => format!("tasks/{id}"),
                    "session" if artex_core::valid_id(id, "conversation") => {
                        format!("conversations/{id}")
                    }
                    // Pre-conversation uploads stay in a disclosed shared staging
                    // area, then only admitted paths are promoted into run CWD.
                    "staging"
                        if artex_core::valid_id(id, "task")
                            || artex_core::valid_id(id, "conversation") =>
                    {
                        String::new()
                    }
                    _ => {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Upload scope must match its typed task or conversation ID",
                        ));
                    }
                };
                let files = input["files"]
                    .as_array()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "File capabilities required"))?;
                let mut attachments = vec![];
                for token in files {
                    let (source, name) = self.files.get(token.as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "File capability required")
                    })?)?;
                    let directory = self.capabilities.resolve(
                        cap,
                        &if base.is_empty() {
                            "attachments".to_owned()
                        } else {
                            format!("{base}/attachments")
                        },
                        true,
                    )?;
                    fs::create_dir_all(&directory).map_err(Error::io)?;
                    let id = artex_core::new_id("attachment");
                    let filename = format!("{id}-{name}");
                    let target = self.capabilities.resolve(
                        cap,
                        &if base.is_empty() {
                            format!("attachments/{filename}")
                        } else {
                            format!("{base}/attachments/{filename}")
                        },
                        true,
                    )?;
                    atomic_copy(&source, &target)?;
                    attachments.push(json!({"name":name,"path":format!("attachments/{filename}"),"size":fs::metadata(&target).map_err(Error::io)?.len()}));
                }
                for token in files
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<std::collections::HashSet<_>>()
                {
                    self.files.release(token)?;
                }
                Ok(json!({"attachments":attachments}))
            }
            "detectPython" => {
                let cwd = self.capabilities.root(cap)?;
                for exe in if cfg!(windows) {
                    vec!["python.exe", "python3.exe"]
                } else {
                    vec!["python3", "python"]
                } {
                    if let Some(path) = std::env::var_os("PATH").and_then(|paths| {
                        std::env::split_paths(&paths)
                            .map(|p| p.join(exe))
                            .find(|p| p.is_file())
                    }) {
                        return Ok(json!({"python_interpreter":path.to_string_lossy()}));
                    }
                    let _ = &cwd;
                }
                Ok(json!({"python_interpreter":""}))
            }
            "testWebSearch" => {
                let mut config = input
                    .get("network_config")
                    .cloned()
                    .unwrap_or_else(|| input["config"].clone());
                if !config.is_object() {
                    config = json!({});
                }
                let patch = &input["patch"];
                if let Some(backend) = patch["web_search_backend"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    config["backend"] = json!(backend);
                }
                config["proxy"] = json!(patch["web_search_proxy"].as_str().unwrap_or(""));
                let backend = config["backend"].as_str().unwrap_or("ddgs").to_string();
                let key = if backend == "tavily" {
                    "tavily_search_api_key"
                } else {
                    "brave_search_api_key"
                };
                if let Some(value) = patch[key].as_str().filter(|s| !s.is_empty()) {
                    config["api_key"] = json!(value);
                }
                let query = if backend == "deepseek" {
                    "DeepSeek company official website"
                } else {
                    "test"
                };
                match crate::network::execute(
                    self,
                    "web_search",
                    &json!({"query":query,"limit":3,"network_config":config}),
                    &internal_context,
                ) {
                    Ok(value) => {
                        let count = value["results"].as_array().map(Vec::len).unwrap_or(0);
                        if count == 0 {
                            Ok(
                                json!({"ok":false,"error":"Search returned no results","backend":backend}),
                            )
                        } else {
                            Ok(json!({"ok":true,"count":count,"backend":backend}))
                        }
                    }
                    Err(error) => Ok(json!({"ok":false,"error":error.message,"backend":backend})),
                }
            }
            "testCustomTool" => crate::custom::test(self, input, &internal_context),
            "refreshMcpServer" => crate::mcp::refresh(self, input, &internal_context),
            _ => Err(Error::new(
                "FEATURE_DISABLED",
                "Native UI operation unavailable",
            )),
        }
    }
    fn upload_skill(&self, cap: &str, input: &Value) -> Result<Value> {
        let (source, _) = self.files.get(text(input, "file")?)?;
        let file = fs::File::open(source).map_err(Error::io)?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid skill zip"))?;
        if zip.len() > 10000 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Skill archive entries budget exceeded",
            ));
        }
        let mut entries = vec![];
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid archive entry"))?;
            let path = entry
                .enclosed_name()
                .ok_or_else(|| Error::new("PERMISSION_DENIED", "Skill zip traversal rejected"))?;
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Skill archive symlinks rejected",
                ));
            }
            if entry.is_dir() {
                continue;
            }
            total = total
                .checked_add(entry.size())
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Archive size overflow"))?;
            if total > 64 * 1024 * 1024 || entry.size() > 16 * 1024 * 1024 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Skill archive size budget exceeded",
                ));
            }
            let mut bytes = vec![];
            entry
                .by_ref()
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(Error::io)?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(Error::new("SIZE_LIMIT", "Skill archive expansion exceeded"));
            }
            entries.push((path, bytes));
        }
        let (skill_path, skill_bytes) = entries
            .iter()
            .find(|(p, _)| p.file_name().is_some_and(|s| s == "SKILL.md"))
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Archive needs SKILL.md"))?;
        let skill_text = std::str::from_utf8(skill_bytes)
            .map_err(|_| Error::new("INVALID_PARAMS", "SKILL.md must be UTF-8"))?;
        let (meta, _) = crate::skills::metadata(skill_text)?;
        let name = meta["name"]
            .as_str()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill name required"))?;
        if !regex::Regex::new("^[a-z][a-z0-9-]{0,63}$")
            .unwrap()
            .is_match(name)
        {
            return Err(Error::new("INVALID_PARAMS", "Invalid archived skill name"));
        }
        let prefix = skill_path.parent().unwrap().to_path_buf();
        let destination = self
            .capabilities
            .resolve(cap, &format!("skills/{name}"), true)?;
        if destination.exists() && !input["overwrite"].as_bool().unwrap_or(false) {
            return Err(Error::new("VERSION_CONFLICT", "Skill already exists"));
        }
        let staging = tempfile::tempdir_in(self.capabilities.root(cap)?).map_err(Error::io)?;
        let mut count = 0;
        for (path, bytes) in entries {
            let relative = path.strip_prefix(&prefix).map_err(|_| {
                Error::new(
                    "INVALID_PARAMS",
                    "Archive contains files outside skill directory",
                )
            })?;
            if relative.as_os_str().is_empty() {
                continue;
            }
            let target = staging.path().join(relative);
            fs::create_dir_all(target.parent().unwrap()).map_err(Error::io)?;
            atomic_write(&target, &bytes)?;
            count += 1;
        }
        fs::create_dir_all(destination.parent().unwrap()).map_err(Error::io)?;
        let backup =
            destination.with_file_name(format!(".{name}-backup-{}", uuid::Uuid::new_v4().simple()));
        let had = destination.exists();
        if had {
            fs::rename(&destination, &backup).map_err(Error::io)?;
        }
        if let Err(error) = fs::rename(staging.path(), &destination) {
            if had {
                let _ = fs::rename(&backup, &destination);
            }
            return Err(Error::io(error));
        }
        if had {
            fs::remove_dir_all(backup).map_err(Error::io)?;
        }
        self.files.release(text(input, "file")?)?;
        Ok(json!({"name":name,"files":count}))
    }
}
