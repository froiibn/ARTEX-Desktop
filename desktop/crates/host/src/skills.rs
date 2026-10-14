use crate::{CapabilityRegistry, tools::atomic_write};
use artex_core::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};
const MATERIALIZED_LIMIT: usize = 64 * 1024 * 1024;

/// Private catalog used by the engine's admission preflight. No instructions,
/// filesystem paths, credentials or file contents leave this metadata boundary.
pub fn catalog(registry: &CapabilityRegistry, cap: &str) -> Result<Value> {
    let directory = root(registry, cap)?;
    if !directory.exists() {
        return Ok(json!([]));
    }
    let mut entries = Vec::new();
    let mut budget = 0usize;
    for entry in fs::read_dir(directory).map_err(Error::io)? {
        let entry = entry.map_err(Error::io)?;
        if !entry.file_type().map_err(Error::io)?.is_dir() {
            continue;
        }
        let skill = entry.file_name().to_string_lossy().into_owned();
        name(&json!({"name":skill}), "name")?;
        let path = registry.resolve(cap, &format!("skills/{skill}/SKILL.md"), false)?;
        if !path.is_file() {
            continue;
        }
        let (meta, _) = metadata(&read(&path)?)?;
        let item = json!({"name":skill,"description":meta["description"],"mcps":meta["mcps"]});
        budget += serde_json::to_vec(&item)?.len();
        if entries.len() >= 10_000 || budget > 16 * 1024 * 1024 {
            return Err(Error::new("SIZE_LIMIT", "Skill catalog exceeds budget"));
        }
        entries.push(item);
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!(entries))
}

struct SkillTree {
    files: Vec<(String, Vec<u8>)>,
    directories: Vec<String>,
    digest: String,
}
fn snapshot_tree(registry: &CapabilityRegistry, cap: &str, base: &str) -> Result<SkillTree> {
    let directory = registry.resolve(cap, base, false)?;
    if !directory.is_dir() {
        return Err(Error::new("NOT_FOUND", "Skill directory does not exist"));
    }
    let mut tree = SkillTree {
        files: Vec::new(),
        directories: Vec::new(),
        digest: String::new(),
    };
    let mut remaining = MATERIALIZED_LIMIT;
    for entry in walkdir::WalkDir::new(&directory)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry.map_err(|_| Error::new("IO_FAILURE", "Skill traversal failed"))?;
        if entry.file_type().is_symlink() {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Skill snapshots cannot contain links",
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(&directory)
            .map_err(|_| Error::new("PERMISSION_DENIED", "Skill path escaped source"))?
            .to_string_lossy()
            .replace('\\', "/");
        let path = registry.resolve(cap, &format!("{base}/{relative}"), false)?;
        if entry.file_type().is_dir() {
            tree.directories.push(relative);
        } else if entry.file_type().is_file() {
            if fs::metadata(&path).map_err(Error::io)?.len() > remaining as u64 {
                return Err(Error::new("SIZE_LIMIT", "Skill snapshot exceeds 64 MiB"));
            }
            let mut bytes = Vec::new();
            fs::File::open(path)
                .map_err(Error::io)?
                .take(remaining as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(Error::io)?;
            if bytes.len() > remaining {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Skill snapshot grew beyond budget",
                ));
            }
            remaining -= bytes.len();
            tree.files.push((relative, bytes));
        } else {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Skill snapshots require ordinary files and directories",
            ));
        }
        if tree.files.len() + tree.directories.len() > 10_000 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Skill snapshot file count exceeds budget",
            ));
        }
    }
    tree.files.sort_by(|a, b| a.0.cmp(&b.0));
    tree.directories.sort();
    let mut hash = Sha256::new();
    hash.update(b"ARTEX skill snapshot v1\0");
    for dir in &tree.directories {
        hash.update(b"directory\0");
        hash.update((dir.len() as u64).to_le_bytes());
        hash.update(dir.as_bytes());
    }
    for (path, bytes) in &tree.files {
        hash.update(b"file\0");
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    tree.digest = hex::encode(hash.finalize());
    Ok(tree)
}

fn load_scoped(
    registry: &CapabilityRegistry,
    input: &Value,
    resource_cap: &str,
    skill: &str,
) -> Result<Value> {
    let execution_cap = input["execution_directory_capability"]
        .as_str()
        .ok_or_else(|| {
            Error::new(
                "PERMISSION_DENIED",
                "Scoped execution capability required for Skill",
            )
        })?;
    let tree = snapshot_tree(registry, resource_cap, &format!("skills/{skill}"))?;
    let markdown = tree
        .files
        .iter()
        .find(|(path, _)| path == "SKILL.md")
        .ok_or_else(|| Error::new("NOT_FOUND", "Skill instructions are missing"))?;
    if markdown.1.len() > 2 * 1024 * 1024 {
        return Err(Error::new("SIZE_LIMIT", "Skill instructions exceed 2 MiB"));
    }
    let markdown = std::str::from_utf8(&markdown.1)
        .map_err(|_| Error::new("INVALID_PARAMS", "Skill instructions must be UTF-8"))?;
    let (meta, body) = metadata(markdown)?;
    let target_relative = format!(".artex-skills/{skill}/{}", tree.digest);
    let target = registry.resolve(execution_cap, &target_relative, true)?;
    if !target.exists() {
        let stage_relative = format!(
            ".artex-skills/{skill}/.stage-{}",
            uuid::Uuid::new_v4().simple()
        );
        let stage = registry.resolve(execution_cap, &stage_relative, true)?;
        let install = (|| -> Result<()> {
            fs::create_dir_all(&stage).map_err(Error::io)?;
            for dir in &tree.directories {
                let path =
                    registry.resolve(execution_cap, &format!("{stage_relative}/{dir}"), true)?;
                fs::create_dir_all(path).map_err(Error::io)?;
            }
            for (path, bytes) in &tree.files {
                let path =
                    registry.resolve(execution_cap, &format!("{stage_relative}/{path}"), true)?;
                fs::create_dir_all(path.parent().unwrap()).map_err(Error::io)?;
                atomic_write(&path, bytes)?;
            }
            match fs::rename(&stage, &target) {
                Ok(()) => Ok(()),
                Err(_) if target.exists() => Ok(()),
                Err(error) => Err(Error::io(error)),
            }
        })();
        // Re-resolve before deleting the unique staging directory; a changed link
        // is rejected rather than followed during recursive cleanup.
        if let Ok(stage) = registry.resolve(execution_cap, &stage_relative, true) {
            if stage.exists() {
                let _ = fs::remove_dir_all(stage);
            }
        }
        install?;
    }
    if snapshot_tree(registry, execution_cap, &target_relative)?.digest != tree.digest {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Materialized Skill snapshot was modified",
        ));
    }
    let instructions = body.replace("${SKILL_DIR}", &target_relative);
    Ok(
        json!({"name":skill,"instructions":instructions,"args":input["args"],"mcps":meta["mcps"],"skill_dir":target_relative,"content_sha256":tree.digest,"extra":[{"role":"user","content":format!("<skill name=\"{skill}\">\n{instructions}\n{}\n</skill>",input["args"].as_str().unwrap_or(""))}]}),
    )
}
fn name(value: &Value, key: &str) -> Result<String> {
    let s = value[key]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill name required"))?;
    if s.len() > 64
        || !s.as_bytes().first().is_some_and(|b| b.is_ascii_lowercase())
        || !s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Skill name must be lowercase letters, numbers, hyphens",
        ));
    }
    Ok(s.into())
}
fn read(path: &Path) -> Result<String> {
    if fs::metadata(path).map_err(Error::io)?.len() > 2 * 1024 * 1024 {
        return Err(Error::new("SIZE_LIMIT", "Skill file exceeds 2 MiB"));
    }
    fs::read_to_string(path).map_err(Error::io)
}
pub fn root(registry: &CapabilityRegistry, cap: &str) -> Result<std::path::PathBuf> {
    registry.resolve(cap, "skills", true)
}
pub fn metadata(content: &str) -> Result<(Value, String)> {
    let stripped = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"));
    if let Some(front) = stripped {
        let end = front
            .find("\n---")
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill frontmatter terminator missing"))?;
        let mut meta: Value = serde_yaml::from_str(&front[..end])
            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid skill frontmatter"))?;
        if !meta.is_object() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Skill frontmatter must be an object",
            ));
        }
        let references = meta
            .get("mcps")
            .filter(|value| !value.is_null())
            .or_else(|| meta.get("mcp"));
        let mut names = Vec::new();
        let values: Vec<&str> = match references {
            Some(Value::String(value)) => vec![value],
            Some(Value::Array(values)) => values.iter().filter_map(Value::as_str).collect(),
            Some(Value::Null) | None => Vec::new(),
            _ => {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Skill MCP references must be text or an array",
                ));
            }
        };
        for value in values {
            for server in value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if !names.contains(&server) {
                    names.push(server);
                }
            }
        }
        meta["mcps"] = json!(names);
        let body = front[end + 4..]
            .trim_start_matches(['\r', '\n'])
            .to_string();
        Ok((meta, body))
    } else {
        Err(Error::new(
            "INVALID_PARAMS",
            "SKILL.md requires YAML frontmatter",
        ))
    }
}
fn files(directory: &Path) -> Result<Vec<String>> {
    let mut files = vec![];
    for entry in walkdir::WalkDir::new(directory)
        .follow_links(false)
        .min_depth(1)
    {
        let entry = entry.map_err(|_| Error::new("IO_FAILURE", "Skill traversal failed"))?;
        if entry.file_type().is_symlink() {
            continue;
        }
        let mut path = entry
            .path()
            .strip_prefix(directory)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if entry.file_type().is_dir() {
            path.push('/');
        }
        files.push(path);
        if files.len() > 10000 {
            return Err(Error::new("SIZE_LIMIT", "Skill file count exceeded"));
        }
    }
    files.sort();
    Ok(files)
}
pub fn execute(registry: &CapabilityRegistry, method: &str, input: &Value) -> Result<Value> {
    let cap = input["directory_capability"]
        .as_str()
        .ok_or_else(|| Error::new("PERMISSION_DENIED", "Skill capability required"))?;
    if method == "skills" {
        let dir = root(registry, cap)?;
        if !dir.exists() {
            return Ok(json!([]));
        }
        let mut skills = vec![];
        for entry in fs::read_dir(dir).map_err(Error::io)? {
            let entry = entry.map_err(Error::io)?;
            let n = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type().map_err(Error::io)?.is_dir() {
                continue;
            }
            let path = registry.resolve(cap, &format!("skills/{n}/SKILL.md"), false)?;
            if !path.is_file() {
                continue;
            }
            let (meta, _) = metadata(&read(&path)?)?;
            let files = files(path.parent().unwrap())?;
            let mut dto = json!({"name":n,"files":files,"calls":0,"tasks":0,"usage_agents":[]});
            for key in ["description", "license", "compatibility", "mcps"] {
                if let Some(value) = meta.get(key).filter(|v| !v.is_null()) {
                    dto[key] = value.clone();
                }
            }
            skills.push(dto);
            if skills.len() > 10000 {
                return Err(Error::new("SIZE_LIMIT", "Skill count exceeds budget"));
            }
        }
        skills.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
        return Ok(json!(skills));
    }
    let payload = if method == "createSkill" {
        &input["s"]
    } else {
        input
    };
    let n = name(
        payload,
        if method == "createSkillDir" || method == "deleteSkillPath" {
            "skill"
        } else {
            "name"
        },
    )?;
    let dir = registry.resolve(cap, &format!("skills/{n}"), true)?;
    match method {
        "createSkill" | "create_skill" => {
            let description = payload["description"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill description required"))?;
            let mut meta = json!({"name":n,"description":description});
            for k in ["license", "compatibility", "mcps"] {
                if let Some(v) = payload.get(k) {
                    meta[k] = v.clone();
                }
            }
            let front = serde_yaml::to_string(&meta)
                .map_err(|_| Error::new("INVALID_PARAMS", "Cannot encode skill metadata"))?;
            let content = format!(
                "---\n{front}---\n{}\n",
                payload["instructions"].as_str().unwrap_or("")
            );
            let file = registry.resolve(cap, &format!("skills/{n}/SKILL.md"), true)?;
            if file.is_file() {
                if read(&file)? == content {
                    return Ok(json!({"name":n}));
                }
                return Err(Error::new("VERSION_CONFLICT", "Skill already exists"));
            }
            fs::create_dir_all(&dir).map_err(Error::io)?;
            atomic_write(&file, content.as_bytes())?;
            Ok(json!({"name":n}))
        }
        "Skill" => load_scoped(registry, input, cap, &n),
        "updateSkillMeta" => {
            let file = registry.resolve(cap, &format!("skills/{n}/SKILL.md"), true)?;
            let (mut meta, body) = metadata(&read(&file)?)?;
            for key in ["description", "license", "compatibility", "mcps"] {
                if let Some(value) = input["meta"].get(key) {
                    meta[key] = value.clone();
                }
            }
            let front = serde_yaml::to_string(&meta)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid skill metadata"))?;
            atomic_write(&file, format!("---\n{front}---\n{body}").as_bytes())?;
            Ok(json!({"ok":true}))
        }
        "skillFiles" => Ok(json!(files(&dir)?)),
        "deleteSkill" => {
            remove(&dir)?;
            Ok(json!({"deleted":n}))
        }
        _ => {
            let relative = input["file"]
                .as_str()
                .or_else(|| input["path"].as_str())
                .unwrap_or("SKILL.md");
            if Path::new(relative)
                .components()
                .any(|p| !matches!(p, Component::Normal(_) | Component::CurDir))
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Skill path traversal rejected",
                ));
            }
            let file = registry.resolve(cap, &format!("skills/{n}/{relative}"), true)?;
            match method {
                "readSkillFile" => Ok(json!(read(&file)?)),
                "writeSkillFile" | "update_skill_file" => {
                    if !dir.is_dir() {
                        return Err(Error::new("NOT_FOUND", "Skill does not exist"));
                    }
                    fs::create_dir_all(file.parent().unwrap()).map_err(Error::io)?;
                    atomic_write(
                        &file,
                        input["content"]
                            .as_str()
                            .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill content required"))?
                            .as_bytes(),
                    )?;
                    Ok(json!({"ok":true}))
                }
                "createSkillDir" => {
                    fs::create_dir_all(file).map_err(Error::io)?;
                    Ok(json!({"dir":relative}))
                }
                "deleteSkillPath" => {
                    if file == dir {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "Use deleteSkill for skill root",
                        ));
                    }
                    remove(&file)?;
                    Ok(json!({"deleted":relative}))
                }
                _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown skill action")),
            }
        }
    }
}
fn remove(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(Error::io)
    } else {
        fs::remove_file(path).map_err(Error::io)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skill_yaml_and_paths() {
        let d = tempfile::tempdir().unwrap();
        let execution = tempfile::tempdir().unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(d.path()).unwrap();
        let exec_cap = r.grant_directory(execution.path()).unwrap();
        execute(&r,"create_skill",&json!({"directory_capability":cap,"name":"test","description":"x: y\nmultiline","instructions":"Hello ${SKILL_DIR}"})).unwrap();
        let v = execute(
            &r,
            "Skill",
            &json!({"directory_capability":cap,"execution_directory_capability":exec_cap,"name":"test"}),
        )
        .unwrap();
        let relative = v["skill_dir"].as_str().unwrap();
        assert!(relative.starts_with(".artex-skills/test/"));
        assert_eq!(v["instructions"], format!("Hello {relative}\n"));
        assert!(execution.path().join(relative).join("SKILL.md").is_file());
        let list = execute(&r, "skills", &json!({"directory_capability":cap})).unwrap();
        assert_eq!(list[0]["files"], json!(["SKILL.md"]));
        assert_eq!(list[0]["calls"], 0);
        assert!(list[0].get("instructions").is_none());
        execute(
            &r,
            "createSkillDir",
            &json!({"directory_capability":cap,"skill":"test","path":"empty"}),
        )
        .unwrap();
        assert!(
            execute(
                &r,
                "skillFiles",
                &json!({"directory_capability":cap,"name":"test"})
            )
            .unwrap()
            .as_array()
            .unwrap()
            .contains(&json!("empty/"))
        );
        execute(&r,"update_skill_file",&json!({"directory_capability":cap,"name":"test","file":"notes.txt","content":"actual notes"})).unwrap();
        assert_eq!(
            fs::read_to_string(d.path().join("skills/test/notes.txt")).unwrap(),
            "actual notes"
        );
        assert!(execute(&r,"update_skill_file",&json!({"directory_capability":cap,"name":"test","file":"../escape","content":"bad"})).is_err());
    }
    #[test]
    fn materialized_skill_snapshot_is_bounded_immutable_and_catalog_is_metadata_only() {
        let resources = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(resources.path()).unwrap();
        let exec = r.grant_directory(work.path()).unwrap();
        execute(&r,"create_skill",&json!({"directory_capability":cap,"name":"approved","description":"Safe skill","instructions":"Read ${SKILL_DIR}/script.txt","mcps":"one, two"})).unwrap();
        execute(&r,"update_skill_file",&json!({"directory_capability":cap,"name":"approved","file":"script.txt","content":"ACTUAL-SKILL-RESOURCE"})).unwrap();
        let input = json!({"directory_capability":cap,"execution_directory_capability":exec,"name":"approved"});
        let first = execute(&r, "Skill", &input).unwrap();
        let relative = first["skill_dir"].as_str().unwrap();
        assert_eq!(
            fs::read_to_string(work.path().join(relative).join("script.txt")).unwrap(),
            "ACTUAL-SKILL-RESOURCE"
        );
        assert_eq!(first["mcps"], json!(["one", "two"]));
        assert_eq!(
            execute(&r, "Skill", &input).unwrap()["content_sha256"],
            first["content_sha256"]
        );
        let list = catalog(&r, &cap).unwrap();
        assert_eq!(list[0]["mcps"], json!(["one", "two"]));
        assert!(list[0].get("instructions").is_none() && list[0].get("files").is_none());
        fs::write(work.path().join(relative).join("script.txt"), "modified").unwrap();
        assert_eq!(
            execute(&r, "Skill", &input).unwrap_err().code,
            "VERSION_CONFLICT"
        );
        execute(&r,"update_skill_file",&json!({"directory_capability":cap,"name":"approved","file":"script.txt","content":"NEW-SOURCE"})).unwrap();
        let second = execute(&r, "Skill", &input).unwrap();
        assert_ne!(second["content_sha256"], first["content_sha256"]);
        let large = resources.path().join("skills/approved/oversized.bin");
        fs::File::create(&large)
            .unwrap()
            .set_len(MATERIALIZED_LIMIT as u64 + 1)
            .unwrap();
        assert_eq!(execute(&r, "Skill", &input).unwrap_err().code, "SIZE_LIMIT");
    }
    #[test]
    fn legacy_skill_mcp_alias_is_normalized_without_exposing_instructions() {
        let (meta, _) = metadata(
            "---\nname: test\ndescription: test\nmcp: alpha, beta\n---\nSECRET-INSTRUCTIONS",
        )
        .unwrap();
        assert_eq!(meta["mcps"], json!(["alpha", "beta"]));
        let (meta, _) =
            metadata("---\nname: test\ndescription: test\nmcps: [alpha, beta]\n---\nInstructions")
                .unwrap();
        assert_eq!(meta["mcps"], json!(["alpha", "beta"]));
    }
    #[cfg(unix)]
    #[test]
    fn selected_skill_materialization_rejects_symlinks() {
        let resources = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(resources.path()).unwrap();
        let exec = r.grant_directory(work.path()).unwrap();
        execute(&r,"create_skill",&json!({"directory_capability":cap,"name":"test","description":"test","instructions":"test"})).unwrap();
        std::os::unix::fs::symlink(outside.path(), resources.path().join("skills/test/escape"))
            .unwrap();
        assert_eq!(execute(&r,"Skill",&json!({"directory_capability":cap,"execution_directory_capability":exec,"name":"test"})).unwrap_err().code,"PERMISSION_DENIED");
    }
}
