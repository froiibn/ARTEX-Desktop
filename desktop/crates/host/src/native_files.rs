use artex_core::{Error, Result, new_id};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Instant,
};
struct Import {
    _directory: tempfile::TempDir,
    path: PathBuf,
    name: String,
    size: u64,
    complete: bool,
    sha256: Option<String>,
    created: Instant,
}
struct EffectProfile {
    profile_root: PathBuf,
    directory: PathBuf,
    readonly_workspace: Option<PathBuf>,
}
#[derive(Default)]
pub struct NativeFiles {
    imports: Mutex<HashMap<String, Import>>,
    profile: Mutex<Option<EffectProfile>>,
    effects: Mutex<()>,
}
impl NativeFiles {
    pub fn set_effect_profile(
        &self,
        profile: &Path,
        workspace: &str,
        readonly_workspace: Option<&Path>,
    ) -> Result<()> {
        if !artex_core::valid_id(workspace, "workspace") {
            return Err(Error::new("INVALID_PARAMS", "Typed workspace required"));
        }
        let canonical = profile.canonicalize().map_err(Error::io)?;
        let readonly = readonly_workspace
            .map(|root| root.canonicalize().map_err(Error::io))
            .transpose()?;
        if !canonical.is_dir()
            || readonly
                .as_ref()
                .is_some_and(|root| canonical.starts_with(root))
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Export profile must be outside the read-only workspace",
            ));
        }
        let directory = canonical.join("file-save-receipts").join(workspace);
        for path in [canonical.join("file-save-receipts"), directory.clone()] {
            if let Ok(meta) = fs::symlink_metadata(&path) {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = false;
                if !meta.is_dir() || meta.file_type().is_symlink() || reparse {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Export receipt directory must not contain links",
                    ));
                }
            }
        }
        fs::create_dir_all(&directory).map_err(Error::io)?;
        *self.profile.lock().unwrap() = Some(EffectProfile {
            profile_root: canonical,
            directory,
            readonly_workspace: readonly,
        });
        Ok(())
    }
    pub fn has_effect_profile(&self) -> bool {
        self.profile.lock().unwrap().is_some()
    }
    /// A durable user-profile receipt allows export from read-only workspaces.
    /// Results replay without overwriting a later user change at the destination.
    pub fn save_with_receipt(
        &self,
        token: &str,
        destination: &Path,
        command: &str,
    ) -> Result<serde_json::Value> {
        use sha2::{Digest, Sha256};
        if !artex_core::valid_id(command, "command") {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Typed save command ID required",
            ));
        }
        let _effect = self.effects.lock().unwrap();
        let (root, readonly, profile_root) = {
            let profile = self.profile.lock().unwrap();
            let p = profile.as_ref().ok_or_else(|| {
                Error::new(
                    "FEATURE_DISABLED",
                    "Native export profile is not configured",
                )
            })?;
            (
                p.directory.clone(),
                p.readonly_workspace.clone(),
                p.profile_root.clone(),
            )
        };
        if !destination.is_absolute() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Selected destination must be absolute",
            ));
        }
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(token, destination))?));
        let receipt = root.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(command.as_bytes()))
        ));
        if receipt.exists() {
            let meta = fs::symlink_metadata(&receipt).map_err(Error::io)?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 65536 {
                return Err(Error::new("CORRUPT_STORE", "Invalid export receipt"));
            }
            let existing: serde_json::Value =
                serde_json::from_slice(&fs::read(&receipt).map_err(Error::io)?)?;
            if existing["fingerprint"] != fingerprint {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Export command changed input",
                ));
            }
            if existing["state"] == "completed" {
                return Ok(existing["result"].clone());
            }
            if existing["state"] == "failed" {
                return Err(serde_json::from_value(existing["error"].clone())
                    .map_err(|_| Error::new("CORRUPT_STORE", "Invalid export error receipt"))?);
            }
            let mut error = Error::new(
                "RECOVERY_REQUIRED",
                "Export outcome is unknown; inspect destination before retrying",
            );
            error.recovery_required = true;
            return Err(error);
        }
        let normalized =
            destination
                .parent()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Destination parent required"))?
                .canonicalize()
                .map_err(Error::io)?
                .join(destination.file_name().ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "Destination file name required")
                })?);
        if normalized.starts_with(&profile_root) {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Export cannot replace private app profile metadata",
            ));
        }
        if readonly
            .as_ref()
            .is_some_and(|root| normalized.starts_with(root))
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Read-only export cannot write into its source workspace",
            ));
        }
        crate::tools::atomic_write(
            &receipt,
            &serde_json::to_vec(
                &serde_json::json!({"fingerprint":fingerprint,"state":"prepared"}),
            )?,
        )?;
        let result = self.save_path(token, &normalized);
        let record = match &result {
            Ok(value) => {
                serde_json::json!({"fingerprint":fingerprint,"state":"completed","result":value})
            }
            Err(error) => {
                serde_json::json!({"fingerprint":fingerprint,"state":"failed","error":error})
            }
        };
        crate::tools::atomic_write(&receipt, &serde_json::to_vec(&record)?)?;
        result
    }
    /// Called only by the typed native file-picker action. Original files are
    /// never moved; the returned token owns a private immutable snapshot.
    pub fn stage_path(
        &self,
        source: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<serde_json::Value> {
        self.stage_path_named(source, None, cancelled)
    }
    pub fn stage_path_named(
        &self,
        source: &Path,
        display_name: Option<&str>,
        cancelled: impl Fn() -> bool,
    ) -> Result<serde_json::Value> {
        use sha2::{Digest, Sha256};
        if !source.is_absolute() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Selected file path must be absolute",
            ));
        }
        let metadata = fs::symlink_metadata(source).map_err(Error::io)?;
        #[cfg(windows)]
        let reparse = {
            use std::os::windows::fs::MetadataExt;
            metadata.file_attributes() & 0x400 != 0
        };
        #[cfg(not(windows))]
        let reparse = false;
        if !metadata.is_file() || metadata.file_type().is_symlink() || reparse {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Selected source must be a regular file without links",
            ));
        }
        if metadata.len() > 512 * 1024 * 1024 {
            return Err(Error::new("SIZE_LIMIT", "Selected file exceeds 512 MiB"));
        }
        let source_name = source
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Selected file name must be Unicode"))?;
        let name = display_name.unwrap_or(source_name);
        if name.len() > 1024
            || name.is_empty()
            || name.contains(['/', '\\', '\0'])
            || name == "."
            || name == ".."
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Bounded native file display name required",
            ));
        }
        let mut file = fs::File::open(source).map_err(Error::io)?;
        let before = file.metadata().map_err(Error::io)?;
        let token = self.stage(None, name, &[], false)?;
        let result = (|| {
            let mut hash = Sha256::new();
            let mut total = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                if cancelled() {
                    return Err(Error::new("CANCELLED", "File staging retired"));
                }
                let read = file.read(&mut buffer).map_err(Error::io)?;
                if read == 0 {
                    break;
                }
                total += read as u64;
                hash.update(&buffer[..read]);
                self.stage(Some(&token), name, &buffer[..read], false)?;
            }
            let after = file.metadata().map_err(Error::io)?;
            if total != before.len()
                || after.len() != before.len()
                || before.modified().ok() != after.modified().ok()
            {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Selected file changed during staging; select it again",
                ));
            }
            if cancelled() {
                return Err(Error::new("CANCELLED", "File staging retired"));
            }
            self.stage(Some(&token), name, &[], true)?;
            let sha = hex::encode(hash.finalize());
            self.imports
                .lock()
                .unwrap()
                .get_mut(&token)
                .ok_or_else(|| Error::new("CANCELLED", "Staged file retired"))?
                .sha256 = Some(sha.clone());
            Ok(
                serde_json::json!({"file_capability":token,"name":name,"byte_length":total.to_string(),"sha256":sha}),
            )
        })();
        if result.is_err() {
            let _ = self.release(&token);
        }
        result
    }
    pub fn save_path(&self, token: &str, destination: &Path) -> Result<serde_json::Value> {
        if !destination.is_absolute() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Selected destination must be absolute",
            ));
        }
        let parent = destination
            .parent()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Selected destination parent required"))?;
        if !parent.is_dir() {
            return Err(Error::new(
                "NOT_FOUND",
                "Selected destination parent does not exist",
            ));
        }
        if let Ok(meta) = fs::symlink_metadata(destination) {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if !meta.is_file() || meta.file_type().is_symlink() || reparse {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Save destination must be a regular file without links",
                ));
            }
        }
        let (source, name) = self.get(token)?;
        let expected = self
            .imports
            .lock()
            .unwrap()
            .get(token)
            .and_then(|file| file.sha256.clone())
            .ok_or_else(|| {
                Error::new(
                    "PERMISSION_DENIED",
                    "Verified immutable native file capability required",
                )
            })?;
        if file_digest(&source)? != expected {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Staged file changed; create a new verified snapshot",
            ));
        }
        let length = crate::tools::atomic_copy(&source, destination)?;
        if file_digest(destination)? != expected {
            let mut error = Error::new(
                "RECOVERY_REQUIRED",
                "Export destination changed during save; inspect actual contents",
            );
            error.recovery_required = true;
            return Err(error);
        }
        Ok(
            serde_json::json!({"saved":true,"name":name,"byte_length":length.to_string(),"sha256":expected}),
        )
    }
    pub fn clear(&self) {
        self.imports.lock().unwrap().clear();
    }
    pub fn stage(
        &self,
        token: Option<&str>,
        name: &str,
        bytes: &[u8],
        complete: bool,
    ) -> Result<String> {
        if bytes.len() > 65536
            || name.is_empty()
            || name.contains(['/', '\\'])
            || name == "."
            || name == ".."
        {
            return Err(Error::new("INVALID_PARAMS", "Invalid native file chunk"));
        }
        let mut imports = self.imports.lock().unwrap();
        imports.retain(|_, v| v.created.elapsed().as_secs() < 3600);
        let key = token.map(str::to_owned).unwrap_or_else(|| new_id("file"));
        if token.is_none() {
            if imports.len() >= 32 {
                return Err(Error::new("BUSY", "Native file import limit"));
            }
            let directory = tempfile::tempdir().map_err(Error::io)?;
            let path = directory.path().join("data");
            fs::File::create(&path).map_err(Error::io)?;
            imports.insert(
                key.clone(),
                Import {
                    _directory: directory,
                    path,
                    name: name.into(),
                    size: 0,
                    complete: false,
                    sha256: None,
                    created: Instant::now(),
                },
            );
        }
        let total: u64 = imports.values().map(|v| v.size).sum();
        if total + bytes.len() as u64 > 512 * 1024 * 1024 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Native file total storage budget exceeded",
            ));
        }
        let import = imports
            .get_mut(&key)
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Unknown native file capability"))?;
        if import.complete || import.name != name {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Native file import already finalized or renamed",
            ));
        }
        if import.size + bytes.len() as u64 > 512 * 1024 * 1024 {
            return Err(Error::new("SIZE_LIMIT", "Native file exceeds 512 MiB"));
        }
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&import.path)
            .map_err(Error::io)?;
        file.write_all(bytes).map_err(Error::io)?;
        if complete {
            file.sync_all().map_err(Error::io)?;
        }
        import.size += bytes.len() as u64;
        import.complete = complete;
        Ok(key)
    }
    pub fn release(&self, token: &str) -> Result<()> {
        self.imports
            .lock()
            .unwrap()
            .remove(token)
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Unknown native file capability"))?;
        Ok(())
    }
    pub fn get(&self, token: &str) -> Result<(PathBuf, String)> {
        let map = self.imports.lock().unwrap();
        let entry = map.get(token).filter(|v| v.complete).ok_or_else(|| {
            Error::new(
                "PERMISSION_DENIED",
                "Finalized native file capability required",
            )
        })?;
        Ok((entry.path.clone(), entry.name.clone()))
    }
}
fn file_digest(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path).map_err(Error::io)?;
    if !file.metadata().map_err(Error::io)?.is_file() {
        return Err(Error::new("INVALID_PARAMS", "Regular staged file required"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(Error::io)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > 512 * 1024 * 1024 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Digest input exceeds native file limit",
            ));
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_cannot_replace_profile_lock_or_handoff_session() {
        let profile = tempfile::tempdir().unwrap();
        let source = tempfile::NamedTempFile::new().unwrap();
        fs::write(source.path(), "export content").unwrap();
        let owner = crate::ProfileInstanceLock::acquire(profile.path()).unwrap();
        let descriptor = profile.path().join("native-launch/session.info");
        let original = fs::read(&descriptor).unwrap();
        let files = NativeFiles::default();
        files
            .set_effect_profile(profile.path(), &new_id("workspace"), None)
            .unwrap();
        let staged = files.stage_path(source.path(), || false).unwrap();
        let token = staged["file_capability"].as_str().unwrap();
        for destination in [
            profile.path().join("desktop.instance.lock"),
            profile.path().join("native-launch/queue.lock"),
            descriptor.clone(),
        ] {
            assert_eq!(
                files
                    .save_with_receipt(token, &destination, &new_id("command"))
                    .unwrap_err()
                    .code,
                "PERMISSION_DENIED"
            );
        }
        assert_eq!(fs::read(&descriptor).unwrap(), original);
        assert_eq!(
            crate::ProfileInstanceLock::acquire(profile.path())
                .err()
                .unwrap()
                .code,
            "BUSY"
        );
        drop(owner);
        let output = tempfile::tempdir().unwrap();
        assert_eq!(
            files
                .save_with_receipt(token, &output.path().join("report.txt"), &new_id("command"))
                .unwrap()["saved"],
            true
        );
    }
    #[test]
    fn generated_stage_display_name_is_validated_in_staging_scope() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("private-temp");
        fs::write(&source, "generated").unwrap();
        let files = NativeFiles::default();
        let result = files
            .stage_path_named(&source, Some("report.md"), || false)
            .unwrap();
        assert_eq!(result["name"], "report.md");
        assert_eq!(
            files
                .get(result["file_capability"].as_str().unwrap())
                .unwrap()
                .1,
            "report.md"
        );
        assert_eq!(
            files
                .stage_path_named(&source, Some("../outside"), || false)
                .unwrap_err()
                .code,
            "INVALID_PARAMS"
        );
    }
    #[test]
    fn tampered_snapshot_is_rejected_before_destination_changes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        fs::write(&source, "verified").unwrap();
        fs::write(&target, "keep").unwrap();
        let files = NativeFiles::default();
        let staged = files.stage_path(&source, || false).unwrap();
        let token = staged["file_capability"].as_str().unwrap();
        let (path, _) = files.get(token).unwrap();
        fs::write(path, "tampered").unwrap();
        assert_eq!(
            files.save_path(token, &target).unwrap_err().code,
            "VERSION_CONFLICT"
        );
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }
    #[test]
    fn selected_file_snapshot_save_and_release_preserve_original() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("selected.txt");
        let target = dir.path().join("saved.txt");
        fs::write(&source, "selected immutable").unwrap();
        let files = NativeFiles::default();
        let staged = files.stage_path(&source, || false).unwrap();
        let token = staged["file_capability"].as_str().unwrap();
        fs::write(&source, "original later changed").unwrap();
        files.save_path(token, &target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "selected immutable");
        assert_eq!(
            fs::read_to_string(&source).unwrap(),
            "original later changed"
        );
        files.release(token).unwrap();
        assert!(files.get(token).is_err());
        assert!(files.stage_path(&source, || true).is_err());
        assert!(files.imports.lock().unwrap().is_empty());
    }
    #[test]
    fn upload_over_old_limit_and_confirmed_release() {
        let files = NativeFiles::default();
        let token = files.stage(None, "large.bin", b"", false).unwrap();
        let path = {
            let mut map = files.imports.lock().unwrap();
            let entry = map.get_mut(&token).unwrap();
            entry.size = 64 * 1024 * 1024;
            fs::OpenOptions::new()
                .write(true)
                .open(&entry.path)
                .unwrap()
                .set_len(entry.size)
                .unwrap();
            entry.path.clone()
        };
        files
            .stage(Some(&token), "large.bin", &[1; 65536], true)
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), 64 * 1024 * 1024 + 65536);
        files.release(&token).unwrap();
        assert!(!path.exists());
        assert!(files.get(&token).is_err());
    }
}
