//! Profile-owned Skill originals; task execution copies and usage stay local.
//!
//! The profile instance/store lock must serialize this migration with resource
//! editors. Sources are never modified, including when opened read-only.
use artex_core::{Error, Result, new_id};
use artex_host::{CapabilityRegistry, skills};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

const IMPORT_VERSION: u64 = 1;

/// Import each legacy workspace once and return the explicit profile resource
/// root. Existing common Skills win; differing originals are archived losslessly.
/// A completed profile-side marker prevents a deleted common Skill from being
/// resurrected on reopening that workspace. Later legacy additions are not
/// rescanned; all new editors/tools must already target this returned root.
pub fn prepare(profile: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let workspace = workspace_root.canonicalize().map_err(Error::io)?;
    // A misconfigured profile, including an ancestor junction, must not turn a
    // read-only source into the destination before migration has even started.
    if prospective_path(profile)?.starts_with(&workspace) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Application profile cannot be inside the source workspace",
        ));
    }
    fs::create_dir_all(profile).map_err(Error::io)?;
    ordinary_directory(profile)?;
    let profile = profile.canonicalize().map_err(Error::io)?;
    if profile.starts_with(&workspace) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Application profile cannot be inside the source workspace",
        ));
    }
    let shared = directory(&profile, Path::new("shared-resources"), true)?.unwrap();
    directory(&profile, Path::new("shared-resources/skills"), true)?;
    let registry = CapabilityRegistry::default();
    let shared_cap = registry.grant_directory(&shared)?;
    skills::configure_shared_profile_resources(&registry, &shared_cap)?;
    let source_id = skills::content_digest(workspace.as_os_str().as_encoded_bytes());
    let import_relative = Path::new("shared-resource-imports").join(&source_id);
    let imports = directory(&profile, &import_relative, true)?.unwrap();
    let marker = imports.join("completed.json");
    if marker_exists(&marker, &source_id)? {
        return Ok(shared);
    }
    let source = directory(&workspace, Path::new("resources/skills"), false)?;
    let destination_cap = registry.grant_directory(&profile)?;
    let source_cap = registry.grant_directory(&workspace)?;
    let deleted = skills::deleted_skill_names(&registry, &shared_cap)?;
    let mut imported = Vec::new();
    if let Some(source) = source {
        let mut entries = Vec::new();
        for entry in fs::read_dir(source).map_err(Error::io)? {
            let entry = entry.map_err(Error::io)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(Error::io)?;
            reject_link(&meta)?;
            if !meta.is_dir() {
                continue;
            }
            entries.push(entry);
            if entries.len() > skills::SNAPSHOT_ENTRY_LIMIT {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Legacy Skill count exceeds snapshot budget",
                ));
            }
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let skill_name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::new("INVALID_PARAMS", "Legacy Skill names must be UTF-8"))?;
            let source_relative = Path::new("resources/skills").join(&skill_name);
            let tree = skills::snapshot_tree(&registry, &source_cap, utf8(&source_relative)?)?;
            let common_relative = Path::new("shared-resources/skills").join(&skill_name);
            let common = directory(&profile, &common_relative, false)?;
            let disposition = if deleted.contains(&skill_name) {
                // Even a never-before-seen legacy workspace cannot reactivate
                // a globally deleted name. Preserve its bytes outside the catalog.
                let backup_relative = import_relative.join(&skill_name).join(&tree.digest);
                directory(&profile, backup_relative.parent().unwrap(), true)?;
                install(
                    &profile,
                    &imports,
                    &registry,
                    &destination_cap,
                    &backup_relative,
                    &tree,
                )?;
                "deleted_preserved"
            } else if common.is_some() {
                let existing =
                    skills::snapshot_tree(&registry, &destination_cap, utf8(&common_relative)?)?;
                if existing.digest == tree.digest {
                    "identical"
                } else {
                    let backup_relative = import_relative.join(&skill_name).join(&tree.digest);
                    directory(&profile, backup_relative.parent().unwrap(), true)?;
                    install(
                        &profile,
                        &imports,
                        &registry,
                        &destination_cap,
                        &backup_relative,
                        &tree,
                    )?;
                    "conflict_preserved"
                }
            } else {
                install(
                    &profile,
                    &imports,
                    &registry,
                    &destination_cap,
                    &common_relative,
                    &tree,
                )?;
                "imported"
            };
            imported
                .push(json!({"name":skill_name,"sha256":tree.digest,"disposition":disposition}));
        }
    }
    // Publish the marker only after every complete tree is installed/archived.
    // Interrupted imports retry idempotently against the existing tree hashes.
    let bytes = serde_json::to_vec(
        &json!({"format_version":IMPORT_VERSION,"source_path_sha256":source_id,"completed":true,"skills":imported}),
    )?;
    atomic_marker(&imports, &marker, &bytes)?;
    Ok(shared)
}

fn prospective_path(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path).map_err(Error::io)?;
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Profile path must not contain parent traversal",
        ));
    }
    let mut existing = absolute.as_path();
    loop {
        match fs::symlink_metadata(existing) {
            Ok(_) => {
                let root = existing.canonicalize().map_err(Error::io)?;
                return Ok(root.join(
                    absolute
                        .strip_prefix(existing)
                        .map_err(|_| Error::new("INVALID_PARAMS", "Invalid profile path"))?,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing = existing.parent().ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "Profile path has no existing ancestor")
                })?;
            }
            Err(error) => return Err(Error::io(error)),
        }
    }
}

/// The sole resource directory outside a workspace that the native gateway may
/// grant. A profile import archive, sibling, or junction is never callable.
pub(crate) fn validate_root(profile: &Path, resources: &Path) -> Result<()> {
    ordinary_directory(profile)?;
    let profile = profile.canonicalize().map_err(Error::io)?;
    let expected = directory(&profile, Path::new("shared-resources"), false)?
        .ok_or_else(|| Error::new("NOT_FOUND", "Shared resource root is missing"))?;
    if std::path::absolute(resources).map_err(Error::io)? != expected {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Only the exact profile shared resource root is permitted",
        ));
    }
    ordinary_directory(resources)
}

fn marker_exists(path: &Path, source_id: &str) -> Result<bool> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::io(error)),
    };
    reject_link(&meta)?;
    if !meta.is_file() || meta.len() > skills::MATERIALIZED_LIMIT as u64 {
        return Err(Error::new(
            "CORRUPT_STORE",
            "Invalid shared Skill import marker",
        ));
    }
    let value: Value = serde_json::from_slice(&fs::read(path).map_err(Error::io)?)?;
    if value["format_version"] != IMPORT_VERSION
        || value["source_path_sha256"] != source_id
        || value["completed"] != true
        || !value["skills"].is_array()
    {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "Shared Skill import marker requires reconciliation",
        ));
    }
    Ok(true)
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Skill paths must be UTF-8"))
}
fn reject_link(metadata: &fs::Metadata) -> Result<()> {
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if metadata.file_type().is_symlink() || reparse {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Shared resources cannot traverse links or reparse points",
        ));
    }
    Ok(())
}
fn ordinary_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path).map_err(Error::io)?;
    reject_link(&meta)?;
    if !meta.is_dir() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Shared resource directory required",
        ));
    }
    Ok(())
}
fn directory(root: &Path, relative: &Path, create: bool) -> Result<Option<PathBuf>> {
    let mut path = root.to_owned();
    for component in relative.components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Resource path must remain relative",
                ));
            }
        }
        match fs::symlink_metadata(&path) {
            Ok(_) => ordinary_directory(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
                return Ok(None);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(Error::io)?;
                ordinary_directory(&path)?;
            }
            Err(error) => return Err(Error::io(error)),
        }
    }
    if !path.canonicalize().map_err(Error::io)?.starts_with(root) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Resource path escaped its owner",
        ));
    }
    Ok(Some(path))
}

struct Stage {
    root: PathBuf,
    path: PathBuf,
}
impl Drop for Stage {
    fn drop(&mut self) {
        // Only the generated, validated profile child is eligible for cleanup;
        // never follow a replacement junction during recursive removal.
        if self.path.starts_with(&self.root)
            && self.path != self.root
            && fs::symlink_metadata(&self.path).is_ok_and(|m| reject_link(&m).is_ok())
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn install(
    profile: &Path,
    imports: &Path,
    registry: &CapabilityRegistry,
    cap: &str,
    relative: &Path,
    tree: &skills::SkillTree,
) -> Result<()> {
    let destination = profile.join(relative);
    if directory(profile, relative, false)?.is_some() {
        if skills::snapshot_tree(registry, cap, utf8(relative)?)?.digest != tree.digest {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Preserved Skill import has different content",
            ));
        }
        return Ok(());
    }
    let stage = Stage {
        root: imports.to_owned(),
        path: imports.join(new_id("skillstage")),
    };
    fs::create_dir(&stage.path).map_err(Error::io)?;
    for path in &tree.directories {
        directory(&stage.path, Path::new(path), true)?;
    }
    for (path, bytes) in &tree.files {
        let path = stage.path.join(path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(Error::io)?;
        file.write_all(bytes).map_err(Error::io)?;
        file.sync_all().map_err(Error::io)?;
    }
    let stage_relative = stage
        .path
        .strip_prefix(profile)
        .map_err(|_| Error::new("PERMISSION_DENIED", "Invalid import stage owner"))?;
    if skills::snapshot_tree(registry, cap, utf8(stage_relative)?)?.digest != tree.digest {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Skill changed while staging its import",
        ));
    }
    for (path, permissions) in &tree.file_permissions {
        fs::set_permissions(stage.path.join(path), permissions.clone()).map_err(Error::io)?;
    }
    // The application profile lock serializes publishers and native file edits.
    // Rename keeps partially written trees outside the callable Skill catalog.
    directory(profile, relative.parent().unwrap(), false)?
        .ok_or_else(|| Error::new("NOT_FOUND", "Skill destination parent disappeared"))?;
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Skill destination changed during import",
        ));
    }
    fs::rename(&stage.path, &destination).map_err(Error::io)?;
    Ok(())
}
fn atomic_marker(parent: &Path, destination: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = parent.join(new_id("skillmarker"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(Error::io)?;
        file.write_all(bytes).map_err(Error::io)?;
        file.sync_all().map_err(Error::io)?;
        drop(file);
        if fs::symlink_metadata(destination).is_ok() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Skill import marker changed",
            ));
        }
        fs::rename(&temporary, destination).map_err(Error::io)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    const MARKDOWN: &str = "---\nname: inspect\ndescription: Inspect a target\nunknown_field: retain exactly\nmcps: [scanner]\n---\nRead ${SKILL_DIR}/data.bin and run helper.sh.\n";
    fn skill(workspace: &Path, markdown: &str) -> PathBuf {
        let path = workspace.join("resources/skills/inspect");
        fs::create_dir_all(path.join("empty/nested")).unwrap();
        fs::write(path.join("SKILL.md"), markdown).unwrap();
        fs::write(path.join("data.bin"), [0, 255, 1, 128, 0]).unwrap();
        fs::write(path.join("helper.sh"), "#!/bin/sh\nprintf helper").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path.join("helper.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }
    fn tree(root: &Path, relative: &str) -> skills::SkillTree {
        let registry = CapabilityRegistry::default();
        let cap = registry.grant_directory(root).unwrap();
        skills::snapshot_tree(&registry, &cap, relative).unwrap()
    }
    #[test]
    fn shared_resources_import_exact_binary_unknown_yaml_empty_directories_without_source_writes() {
        let source = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        skill(source.path(), MARKDOWN);
        let large = vec![b'x'; 65 * 1024];
        fs::write(
            source.path().join("resources/skills/inspect/large.txt"),
            &large,
        )
        .unwrap();
        let before = tree(source.path(), "resources/skills/inspect");
        let root = prepare(profile.path(), source.path()).unwrap();
        assert_eq!(
            fs::read(root.join("skills/inspect/SKILL.md")).unwrap(),
            MARKDOWN.as_bytes()
        );
        assert_eq!(
            fs::read(root.join("skills/inspect/data.bin")).unwrap(),
            [0, 255, 1, 128, 0]
        );
        assert_eq!(
            fs::read(root.join("skills/inspect/large.txt")).unwrap(),
            large
        );
        assert!(root.join("skills/inspect/empty/nested").is_dir());
        let after = tree(source.path(), "resources/skills/inspect");
        let imported = tree(&root, "skills/inspect");
        assert_eq!(before.digest, after.digest);
        assert_eq!(before.digest, imported.digest);
        assert_eq!(before.files, after.files);
        assert_eq!(before.directories, after.directories);
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join("skills/inspect/helper.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
    }
    #[test]
    fn shared_resources_conflicts_preserve_original_and_completed_import_never_resurrects_deleted_skill()
     {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        skill(a.path(), MARKDOWN);
        let original = MARKDOWN.replace("Inspect a target", "Different original");
        skill(b.path(), &original);
        let root = prepare(profile.path(), a.path()).unwrap();
        let before = tree(b.path(), "resources/skills/inspect");
        assert_eq!(prepare(profile.path(), b.path()).unwrap(), root);
        assert_eq!(
            fs::read_to_string(root.join("skills/inspect/SKILL.md")).unwrap(),
            MARKDOWN
        );
        let namespace = skills::content_digest(
            b.path()
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes(),
        );
        let backup = profile
            .path()
            .join("shared-resource-imports")
            .join(namespace)
            .join("inspect")
            .join(&before.digest);
        assert_eq!(
            fs::read_to_string(backup.join("SKILL.md")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(backup.join("data.bin")).unwrap(),
            [0, 255, 1, 128, 0]
        );
        assert!(backup.join("empty/nested").is_dir());
        assert_eq!(
            tree(b.path(), "resources/skills/inspect").digest,
            before.digest
        );
        fs::remove_dir_all(root.join("skills/inspect")).unwrap();
        let added = a.path().join("resources/skills/new-legacy");
        fs::create_dir_all(&added).unwrap();
        fs::write(added.join("SKILL.md"), MARKDOWN).unwrap();
        prepare(profile.path(), a.path()).unwrap();
        prepare(profile.path(), b.path()).unwrap();
        assert!(!root.join("skills/inspect").exists());
        assert!(!root.join("skills/new-legacy").exists());
        assert_eq!(
            fs::read_to_string(backup.join("SKILL.md")).unwrap(),
            original
        );
    }
    #[test]
    fn shared_resources_global_delete_blocks_unvisited_legacy_import_but_allows_explicit_recreation()
     {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let c = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        skill(a.path(), MARKDOWN);
        skill(
            b.path(),
            &MARKDOWN.replace("Inspect a target", "Unvisited B original"),
        );
        skill(
            c.path(),
            &MARKDOWN.replace("Inspect a target", "Unvisited C original"),
        );
        let root = prepare(profile.path(), a.path()).unwrap();
        let registry = CapabilityRegistry::default();
        let cap = registry.grant_directory(&root).unwrap();
        skills::execute(
            &registry,
            "deleteSkill",
            &json!({"directory_capability":cap,"name":"inspect"}),
        )
        .unwrap();
        prepare(profile.path(), b.path()).unwrap();
        assert!(!root.join("skills/inspect").exists());
        let source = tree(b.path(), "resources/skills/inspect");
        let namespace = skills::content_digest(
            b.path()
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes(),
        );
        let backup = profile
            .path()
            .join("shared-resource-imports")
            .join(namespace)
            .join("inspect")
            .join(source.digest);
        assert!(
            fs::read_to_string(backup.join("SKILL.md"))
                .unwrap()
                .contains("Unvisited B original")
        );
        assert_eq!(
            fs::read(backup.join("data.bin")).unwrap(),
            [0, 255, 1, 128, 0]
        );
        assert!(backup.join("empty/nested").is_dir());
        skills::execute(&registry,"create_skill",&json!({"directory_capability":cap,"name":"inspect","description":"Explicit new common skill","instructions":"Deliberately recreated"})).unwrap();
        prepare(profile.path(), c.path()).unwrap();
        let active = fs::read_to_string(root.join("skills/inspect/SKILL.md")).unwrap();
        assert!(active.contains("Deliberately recreated"));
        assert!(!active.contains("Unvisited C original"));
        assert!(
            skills::deleted_skill_names(&registry, &cap)
                .unwrap()
                .contains("inspect")
        );
    }
    #[test]
    fn shared_resources_readonly_empty_source_is_never_initialized_and_corrupt_marker_is_not_ignored()
     {
        let source = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        prepare(profile.path(), source.path()).unwrap();
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 0);
        let namespace = skills::content_digest(
            source
                .path()
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes(),
        );
        let marker = profile
            .path()
            .join("shared-resource-imports")
            .join(namespace)
            .join("completed.json");
        fs::write(marker, b"{\"format_version\":999}").unwrap();
        assert_eq!(
            prepare(profile.path(), source.path()).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 0);
    }
    #[test]
    fn shared_resources_reject_nested_profile_before_any_source_directory_is_created() {
        let source = tempfile::tempdir().unwrap();
        let nested = source.path().join("profile/does-not-exist");
        assert_eq!(
            prepare(&nested, source.path()).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 0);
        assert_eq!(
            prepare(source.path(), source.path()).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        assert_eq!(fs::read_dir(source.path()).unwrap().count(), 0);
    }
    #[test]
    fn shared_resources_reject_oversized_snapshot_before_publishing_skill_or_completion() {
        let source = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let path = skill(source.path(), MARKDOWN);
        fs::File::create(path.join("oversized.bin"))
            .unwrap()
            .set_len(skills::MATERIALIZED_LIMIT as u64 + 1)
            .unwrap();
        assert_eq!(
            prepare(profile.path(), source.path()).unwrap_err().code,
            "SIZE_LIMIT"
        );
        assert!(
            !profile
                .path()
                .join("shared-resources/skills/inspect")
                .exists()
        );
        let namespace = skills::content_digest(
            source
                .path()
                .canonicalize()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes(),
        );
        assert!(
            !profile
                .path()
                .join("shared-resource-imports")
                .join(namespace)
                .join("completed.json")
                .exists()
        );
        assert_eq!(
            fs::metadata(path.join("oversized.bin")).unwrap().len(),
            skills::MATERIALIZED_LIMIT as u64 + 1
        );
    }
    #[cfg(any(unix, windows))]
    fn link_directory(source: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, source).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let result = std::process::Command::new("cmd.exe")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(cmd_path(source))
                .arg(cmd_path(target))
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "Could not create isolated junction fixture: status {:?}, stderr {}",
                result.status.code(),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    #[cfg(windows)]
    fn cmd_path(path: &Path) -> String {
        // mklink's builtin parser rejects mixed separators that Rust/Win32
        // filesystem APIs accept, so normalize only these fixture arguments.
        let path = path
            .to_str()
            .expect("Fixture paths are UTF-8")
            .replace('/', "\\");
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{unc}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
        }
    }
    #[cfg(any(unix, windows))]
    #[test]
    fn shared_resources_reject_source_tree_and_profile_root_symlink_or_junction_escapes() {
        let source = tempfile::tempdir().unwrap();
        let profile = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = skill(source.path(), MARKDOWN);
        fs::write(outside.path().join("private.bin"), b"must not import").unwrap();
        link_directory(&path.join("escape"), outside.path());
        assert_eq!(
            prepare(profile.path(), source.path()).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        assert!(
            !profile
                .path()
                .join("shared-resources/skills/inspect")
                .exists()
        );
        assert_eq!(
            fs::read(outside.path().join("private.bin")).unwrap(),
            b"must not import"
        );
        let other_profile = tempfile::tempdir().unwrap();
        link_directory(
            &other_profile.path().join("shared-resources"),
            outside.path(),
        );
        assert_eq!(
            prepare(other_profile.path(), source.path())
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            fs::read(outside.path().join("private.bin")).unwrap(),
            b"must not import"
        );
    }
}
