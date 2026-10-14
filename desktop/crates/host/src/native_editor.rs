//! Optimistic editor revisions. Native saves share the durable effect receipt
//! lock; external programs cannot be forced to participate in that lock.
use crate::CapabilityRegistry;
use artex_core::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
const MAX_EDITOR_BYTES: usize = 2 * 1024 * 1024;

fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", &format!("Missing {key}")))
}
fn conflict() -> Error {
    Error::new(
        "VERSION_CONFLICT",
        "File changed since it was loaded; reload and reconcile the draft",
    )
}
fn safe_path(caps: &CapabilityRegistry, cap: &str, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains('\0') {
        return Err(Error::new("INVALID_PARAMS", "File path required"));
    }
    let mut cursor = caps.root(cap)?;
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part)
                if !part.to_string_lossy().contains(':')
                    && !part.to_string_lossy().eq_ignore_ascii_case(".artex-host") =>
            {
                cursor.push(part)
            }
            _ => {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Editor path is reserved or not relative",
                ));
            }
        }
        if let Ok(meta) = fs::symlink_metadata(&cursor) {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = false;
            if meta.file_type().is_symlink() || reparse {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Editor links and reparse points are forbidden",
                ));
            }
        }
    }
    caps.resolve(cap, relative, true)
}
fn bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = fs::File::open(path).map_err(Error::io)?;
    let metadata = file.metadata().map_err(Error::io)?;
    if !metadata.is_file() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Editor requires a regular file",
        ));
    }
    if metadata.len() > MAX_EDITOR_BYTES as u64 {
        return Ok(None);
    }
    let mut result = Vec::new();
    file.take((MAX_EDITOR_BYTES + 1) as u64)
        .read_to_end(&mut result)
        .map_err(Error::io)?;
    Ok((result.len() <= MAX_EDITOR_BYTES).then_some(result))
}
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn current_hash(path: &Path) -> Result<String> {
    bytes(path)?
        .map(|b| digest(&b))
        .ok_or_else(|| Error::new("SIZE_LIMIT", "File exceeds editor limit"))
}
pub(crate) fn skill_meta(caps: &CapabilityRegistry, method: &str, input: &Value) -> Result<Value> {
    let name = text(input, "name")?;
    if name.is_empty()
        || name.len() > 128
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0', ':'])
    {
        return Err(Error::new("INVALID_PARAMS", "Bounded skill name required"));
    }
    let mut file = json!({"directory_capability":input["directory_capability"],"path":format!("skills/{name}/SKILL.md")});
    let loaded = execute(caps, "nativeFileRead", &file)?;
    if loaded["binary"] == true || loaded["too_large"] == true {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Skill instructions must be bounded UTF-8 text",
        ));
    }
    let (mut meta, body) = crate::skills::metadata(loaded["content"].as_str().unwrap())?;
    if method == "nativeSkillMetaRead" {
        return Ok(json!({"name":name,"meta":meta,"sha256":loaded["sha256"]}));
    }
    if input["expected_sha256"] != loaded["sha256"] {
        return Err(conflict());
    }
    let patch = input["meta"]
        .as_object()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill metadata object required"))?;
    for (key, value) in patch {
        if !["description", "license", "compatibility", "mcps"].contains(&key.as_str()) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Unknown mutable skill metadata field",
            ));
        }
        if key == "mcps" {
            if !value.is_null()
                && !value.is_string()
                && !value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string))
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Skill MCP references must be strings",
                ));
            }
        } else if !value.is_string() && !value.is_null() {
            return Err(Error::new("INVALID_PARAMS", "Skill metadata text required"));
        }
        meta[key] = value.clone();
    }
    let front = serde_yaml::to_string(&meta)
        .map_err(|_| Error::new("INVALID_PARAMS", "Cannot encode skill metadata"))?;
    file["content"] = json!(format!("---\n{front}---\n{body}"));
    file["expected_sha256"] = input["expected_sha256"].clone();
    file["create"] = json!(false);
    let mut saved = execute(caps, "nativeFileWrite", &file)?;
    saved["name"] = json!(name);
    Ok(saved)
}
pub(crate) fn execute(caps: &CapabilityRegistry, method: &str, input: &Value) -> Result<Value> {
    let cap = text(input, "directory_capability")?;
    let relative = text(input, "path")?;
    let path = safe_path(caps, cap, relative)?;
    if method == "nativeFileRead" {
        return match bytes(&path)? {
            None => Ok(
                json!({"path":relative,"content":"","binary":false,"too_large":true,"sha256":null}),
            ),
            Some(bytes) => {
                let sha = digest(&bytes);
                let binary = bytes.contains(&0) || std::str::from_utf8(&bytes).is_err();
                Ok(
                    json!({"path":relative,"content":if binary{String::new()}else{String::from_utf8(bytes).unwrap()},"binary":binary,"too_large":false,"sha256":sha}),
                )
            }
        };
    }
    let content = text(input, "content")?.as_bytes();
    if content.len() > MAX_EDITOR_BYTES || content.contains(&0) {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Editor content must be UTF-8 text without NUL, at most 2 MiB",
        ));
    }
    let create = input["create"]
        .as_bool()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Explicit create flag required"))?;
    let expected = input["expected_sha256"].as_str();
    if create {
        if !input["expected_sha256"].is_null() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "New file must have no previous revision",
            ));
        }
        if path.exists() {
            return Err(conflict());
        }
    } else {
        let expected = expected
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Loaded SHA256 revision required"))?;
        if !path.exists() || current_hash(&path)? != expected.to_ascii_lowercase() {
            return Err(conflict());
        }
    }
    let parent = path
        .parent()
        .filter(|p| p.is_dir())
        .ok_or_else(|| Error::new("NOT_FOUND", "Parent directory does not exist"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(Error::io)?;
    temporary
        .write_all(content)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(Error::io)?;
    // Recheck after preparation, immediately before installation. This is an
    // optimistic revision check, not a filesystem transaction with Bash.
    let checked = safe_path(caps, cap, relative)?;
    if checked != path {
        return Err(conflict());
    }
    if create {
        #[cfg(windows)]
        {
            // WRITE_THROUGH without REPLACE_EXISTING; creation cannot clobber
            // a concurrent creator and confirms durable native installation.
            artex_platform::install_new(temporary.path(), &path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    conflict()
                } else {
                    Error::io(e)
                }
            })?;
        }
        #[cfg(not(windows))]
        temporary.persist_noclobber(&path).map_err(|e| {
            if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                conflict()
            } else {
                Error::io(e.error)
            }
        })?;
        artex_platform::sync_directory(parent).map_err(Error::io)?;
    } else {
        if current_hash(&path)? != expected.unwrap().to_ascii_lowercase() {
            return Err(conflict());
        }
        // Existing platform helper provides write-through rename / directory sync.
        crate::tools::atomic_write(&path, content)?;
    }
    let sha = digest(content);
    if current_hash(&path)? != sha {
        let mut error = Error::new(
            "RECOVERY_REQUIRED",
            "File changed during save; verify actual file contents before retrying",
        );
        error.recovery_required = true;
        return Err(error);
    }
    Ok(json!({"path":relative,"sha256":sha,"saved":true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_metadata_revision_preserves_body_and_rejects_stale_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let caps = CapabilityRegistry::default();
        let cap = caps.grant_directory(dir.path()).unwrap();
        fs::create_dir_all(dir.path().join("skills/sample")).unwrap();
        let path = dir.path().join("skills/sample/SKILL.md");
        fs::write(
            &path,
            "---\nname: sample\ndescription: loaded\nx-custom: keep\n---\nInstructions unchanged",
        )
        .unwrap();
        let loaded = skill_meta(
            &caps,
            "nativeSkillMetaRead",
            &json!({"directory_capability":cap,"name":"sample"}),
        )
        .unwrap();
        let write = json!({"directory_capability":cap,"name":"sample","meta":{"description":"saved"},"expected_sha256":loaded["sha256"]});
        skill_meta(&caps, "nativeSkillMetaWrite", &write).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Instructions unchanged"));
        assert!(content.contains("x-custom: keep"));
        assert!(content.contains("description: saved"));
        assert_eq!(
            skill_meta(&caps, "nativeSkillMetaWrite", &write)
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
    }
    #[test]
    fn loaded_revision_rejects_external_changes_and_create_never_clobbers() {
        let dir = tempfile::tempdir().unwrap();
        let caps = CapabilityRegistry::default();
        let cap = caps.grant_directory(dir.path()).unwrap();
        fs::write(dir.path().join("file.txt"), "loaded").unwrap();
        let loaded = execute(
            &caps,
            "nativeFileRead",
            &json!({"directory_capability":cap,"path":"file.txt"}),
        )
        .unwrap();
        fs::write(dir.path().join("file.txt"), "external").unwrap();
        let write = json!({"directory_capability":cap,"path":"file.txt","content":"draft","expected_sha256":loaded["sha256"],"create":false});
        assert_eq!(
            execute(&caps, "nativeFileWrite", &write).unwrap_err().code,
            "VERSION_CONFLICT"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt")).unwrap(),
            "external"
        );
        let create = json!({"directory_capability":cap,"path":"new.txt","content":"new","expected_sha256":null,"create":true});
        assert_eq!(
            execute(&caps, "nativeFileWrite", &create).unwrap()["saved"],
            true
        );
        assert_eq!(
            execute(&caps, "nativeFileWrite", &create).unwrap_err().code,
            "VERSION_CONFLICT"
        );
        assert!(
            execute(
                &caps,
                "nativeFileRead",
                &json!({"directory_capability":cap,"path":".artex-host/receipts/x"})
            )
            .is_err()
        );
    }
    #[test]
    fn binary_and_oversized_files_are_explicit_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let caps = CapabilityRegistry::default();
        let cap = caps.grant_directory(dir.path()).unwrap();
        fs::write(dir.path().join("binary"), [0, 255]).unwrap();
        let binary = execute(
            &caps,
            "nativeFileRead",
            &json!({"directory_capability":cap,"path":"binary"}),
        )
        .unwrap();
        assert_eq!(binary["binary"], true);
        assert_eq!(binary["content"], "");
        let large = fs::File::create(dir.path().join("large")).unwrap();
        large.set_len((MAX_EDITOR_BYTES + 1) as u64).unwrap();
        let large = execute(
            &caps,
            "nativeFileRead",
            &json!({"directory_capability":cap,"path":"large"}),
        )
        .unwrap();
        assert_eq!(large["too_large"], true);
        assert_eq!(large["sha256"], Value::Null);
    }
}
