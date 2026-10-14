use artex_core::{Error, Result, new_id};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

#[derive(Default)]
pub struct CapabilityRegistry {
    grants: Mutex<HashMap<String, PathBuf>>,
}
impl CapabilityRegistry {
    pub fn grant_directory(&self, path: &Path) -> Result<String> {
        let id = new_id("capability");
        self.grant_directory_as(&id, path)?;
        Ok(id)
    }
    /// Host-only preallocated token, registered after a workspace is created.
    /// Never exposed as a WebView command or accepted from engine tool input.
    pub fn grant_directory_as(&self, id: &str, path: &Path) -> Result<()> {
        if !artex_core::valid_id(id, "capability") {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid native capability token",
            ));
        }
        let canonical = path.canonicalize().map_err(Error::io)?;
        if !canonical.is_dir() {
            return Err(Error::new("INVALID_PARAMS", "Directory required"));
        }
        let mut grants = self.grants.lock().unwrap();
        if let Some(existing) = grants.get(id) {
            if existing != &canonical {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Capability token already bound",
                ));
            }
            return Ok(());
        }
        grants.insert(id.into(), canonical);
        Ok(())
    }
    pub fn revoke(&self, capability: &str) {
        self.grants.lock().unwrap().remove(capability);
    }
    pub fn root(&self, capability: &str) -> Result<PathBuf> {
        self.grants
            .lock()
            .unwrap()
            .get(capability)
            .cloned()
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Unknown directory capability"))
    }
    /// Reject absolute paths, parent traversal and symlink/reparse escapes. Existing
    /// components are resolved individually; non-existing final files remain rooted.
    /// Capabilities are access policy, not protection against concurrent hostile OS users.
    pub fn resolve(&self, capability: &str, relative: &str, _write: bool) -> Result<PathBuf> {
        let root = self.root(capability)?;
        let mut target = root.clone();
        for c in Path::new(relative).components() {
            match c {
                Component::Normal(part) => {
                    target.push(part);
                    if std::fs::symlink_metadata(&target).is_ok() {
                        target = target.canonicalize().map_err(Error::io)?;
                        if !target.starts_with(&root) {
                            return Err(Error::new("PERMISSION_DENIED", "Path escapes capability"));
                        }
                    }
                }
                Component::CurDir => (),
                _ => {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Absolute and parent paths are forbidden",
                    ));
                }
            }
        }
        if !target.starts_with(root) {
            return Err(Error::new("PERMISSION_DENIED", "Path escapes capability"));
        }
        Ok(target)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn traversal_and_unknown_grants_fail() {
        let dir = tempfile::tempdir().unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(dir.path()).unwrap();
        assert!(
            r.resolve(&cap, "a/b", true)
                .unwrap()
                .starts_with(dir.path().canonicalize().unwrap())
        );
        assert!(r.resolve(&cap, "../outside", false).is_err());
        assert!(
            r.resolve(
                &cap,
                if cfg!(windows) {
                    "C:\\outside"
                } else {
                    "/outside"
                },
                false
            )
            .is_err()
        );
        r.revoke(&cap);
        assert!(r.resolve(&cap, "a", false).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        let r = CapabilityRegistry::default();
        let cap = r.grant_directory(dir.path()).unwrap();
        assert!(r.resolve(&cap, "escape/file", true).is_err());
    }
}
