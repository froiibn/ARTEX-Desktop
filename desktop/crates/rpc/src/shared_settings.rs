//! App-profile configuration is authoritative; workspace rows are FK-safe projections.
//! A durable intent bridges the two journals, so a crash cannot silently discard a
//! confirmed workspace edit or make an older workspace replace the common defaults.
use super::*;
use crate::shared_configuration::SharedConfiguration;
use artex_core::{Change, Record, default_data};
use serde::{Deserialize, Serialize};
use std::io::{Seek, SeekFrom};
use std::path::Path;

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn temporary(store: &Store) -> Result<(Temporary, std::fs::File)> {
    let path = Temporary(store.temporary_path("cfg")?);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path.0)
        .map_err(Error::io)?;
    Ok((path, file))
}

/// Configuration Bodies keep their original verified BlobRef. Streaming between
/// stores avoids turning large prompts into invalid >64KiB inline Bodies.
fn copy_configuration_blobs(
    source: &Store,
    destination: &mut Store,
    configuration: &SharedConfiguration,
) -> Result<()> {
    let mut copied = std::collections::BTreeSet::new();
    for reference in configuration.body_references() {
        let hash = reference["sha256"]
            .as_str()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Configuration BlobRef lacks a hash"))?;
        if !copied.insert(hash.to_owned()) {
            continue;
        }
        let temporary = Temporary(destination.temporary_path("cfg")?);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary.0)
            .map_err(Error::io)?;
        source.copy_blob_to(&reference, &mut file)?;
        file.sync_all().map_err(Error::io)?;
        drop(file);
        let installed = destination.blob_put_file(
            &temporary.0,
            reference["media_type"]
                .as_str()
                .unwrap_or("application/octet-stream"),
        )?;
        if installed != reference {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Configuration blob copy changed its identity",
            ));
        }
    }
    Ok(())
}

const CATALOG_KEY: &str = "native.shared_settings.v1";
const FORMAT_VERSION: u32 = 1;

#[cfg(test)]
#[path = "shared_settings_storage_tests.rs"]
mod storage_tests;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogReference {
    format_version: u32,
    catalog_blob: artex_core::models::BlobRef,
}

/// Old inline v1 catalogs remain readable. New catalogs use verified immutable
/// blobs so large scripts or retained migration originals cannot overflow a
/// single 16MiB journal frame. The descriptor never embeds user script content.
fn read_catalog(store: &Store, value: &str) -> Result<Catalog> {
    let parsed: Value = serde_json::from_str(value)?;
    if parsed.get("catalog_blob").is_none() {
        return Ok(serde_json::from_value(parsed)?);
    }
    let descriptor: CatalogReference = serde_json::from_value(parsed)?;
    if descriptor.format_version != FORMAT_VERSION {
        return Err(Error::new(
            "RECOVERY_REQUIRED",
            "Unsupported common settings descriptor",
        ));
    }
    let (_temporary, mut file) = temporary(store)?;
    store.copy_blob_to(&serde_json::to_value(descriptor.catalog_blob)?, &mut file)?;
    file.seek(SeekFrom::Start(0)).map_err(Error::io)?;
    Ok(serde_json::from_reader(file)?)
}

/// Operational cursors and workspace approval consent are deliberately not preferences.
fn common_key(key: &str) -> bool {
    matches!(
        key,
        "traffic_capture"
            | "agent_traffic_binding"
            | "llm_record"
            | "web_search_enabled"
            | "web_search_backend"
            | "brave_search_api_key"
            | "tavily_search_api_key"
            | "web_search_proxy"
            | "global_proxy"
            | "python_interpreter"
            | "workers"
            | "task_concurrency_enabled"
            | "task_concurrency_limit"
            | "llm_pool_enabled"
            | "llm_pool_bind_fallback"
            | "constraints_inject_planner"
            | "constraints_inject_worker"
            | "noa_compaction"
            | "notify_enabled"
            | "notify_public_base_url"
            | "notify_digest_interval_min"
            | "llm_retry_policy"
            | "intercept_enabled_tools"
            | "llm_judge_enabled"
            | "llm_judge_profile_id"
            | "llm_judge_prompt"
            | "llm_judge_timeout_seconds"
            | "llm_judge_fail_action"
            | "llm_judge_ask_timeout_seconds"
            | "llm_judge_ask_timeout_action"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{OpenMode, OpenOptions};

    fn model_batch(s: &Snapshot, id: &str, model: &str, active: bool) -> CommandBatch {
        let mut p = artex_domain::Planner::new(s, "fixture.model", &json!({}));
        let mut data = default_data("llm_profiles").unwrap();
        data["id"] = json!(id);
        data["name"] = json!(model);
        data["model"] = json!(model);
        data["format"] = json!("openai");
        data["is_default"] = json!(active);
        data["api_key"] = json!({"secret_ref":new_id("secret"),"hint":"configured"});
        p.put("llm_profiles", id, data);
        let mut batch = p.done(json!({}));
        artex_domain::augment_expected(&mut batch, s).unwrap();
        batch.command_id = new_id("command");
        batch
    }
    fn open(router: &mut Router, path: &Path, create: bool) {
        router
            .native_open(&OpenOptions {
                path: path.into(),
                mode: if create {
                    OpenMode::Create {
                        name: "Shared settings fixture".into(),
                    }
                } else {
                    OpenMode::Existing
                },
                read_only: false,
            })
            .unwrap();
    }
    fn arm(router: &mut Router, batch: &CommandBatch) {
        let mut catalog = router.shared_settings.as_ref().unwrap().catalog.clone();
        catalog.pending = Some(Pending {
            root: router.shared_settings_root.clone().unwrap(),
            workspace_id: router.snapshot().unwrap().workspace_id.clone(),
            command_id: batch.command_id.clone(),
        });
        router
            .shared_settings
            .as_mut()
            .unwrap()
            .persist(catalog)
            .unwrap();
    }
    #[test]
    fn common_settings_intent_recovers_both_crash_boundaries_without_plaintext_or_lost_edit() {
        for committed in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let profile = dir.path().join("profile");
            let a = dir.path().join("a");
            let id = new_id("profile");
            let mut router = Router::new(Default::default());
            router.native_share_settings(&profile).unwrap();
            open(&mut router, &a, true);
            let batch = model_batch(&router.snapshot().unwrap(), &id, "fixture-model", true);
            arm(&mut router, &batch);
            if committed {
                router.store.as_mut().unwrap().commit(batch).unwrap();
            }
            // Drop before closing/sealing the common catalog, as at process loss.
            drop(router);
            let mut restored = Router::new(Default::default());
            restored.native_share_settings(&profile).unwrap();
            assert!(
                restored
                    .shared_settings
                    .as_ref()
                    .unwrap()
                    .catalog
                    .pending
                    .is_none()
            );
            open(&mut restored, &dir.path().join("b"), true);
            let s = restored.snapshot().unwrap();
            assert_eq!(s.get("llm_profiles", &id).is_some(), committed);
            if committed {
                assert_eq!(
                    s.get("llm_profiles", &id).unwrap().data["model"],
                    "fixture-model"
                );
            }
            let scope = restored.native_scope().unwrap();
            restored.native_close(&scope).unwrap();
        }
    }
    #[test]
    fn common_settings_import_keeps_default_and_conflicting_original_then_never_resurrects_deleted_models()
     {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let id = new_id("profile");
        let other = new_id("profile");
        for (root, model) in [(&a, "original-A"), (&b, "original-B")] {
            let mut store = Store::create(root, "Legacy fixture").unwrap();
            store
                .commit(model_batch(&store.snapshot(), &id, model, true))
                .unwrap();
        }
        let mut router = Router::new(Default::default());
        router
            .native_share_settings(&dir.path().join("profile"))
            .unwrap();
        open(&mut router, &a, false);
        let scope = router.native_scope().unwrap();
        router.native_close(&scope).unwrap();
        open(&mut router, &b, false);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("llm_profiles", &id)
                .unwrap()
                .data["model"],
            "original-A"
        );
        let catalog = &router.shared_settings.as_ref().unwrap().catalog;
        assert!(catalog.imports.values().flatten().any(|p| {
            p.profiles
                .get(&id)
                .is_some_and(|r| r.data["model"] == "original-B")
        }));
        let mut batch = model_batch(&router.snapshot().unwrap(), &other, "inactive", false);
        router.commit_with_shared_settings(batch.clone()).unwrap();
        let scope = router.native_scope().unwrap();
        router.native_close(&scope).unwrap();
        open(&mut router, &a, false);
        assert!(
            router
                .snapshot()
                .unwrap()
                .get("llm_profiles", &other)
                .is_some()
        );
        let scope = router.native_scope().unwrap();
        router.native_close(&scope).unwrap();
        open(&mut router, &b, false);
        let s = router.snapshot().unwrap();
        batch.command_id = new_id("command");
        batch.method = "fixture.delete".into();
        batch.changes = vec![Change::Delete {
            collection: "llm_profiles".into(),
            id: other.clone(),
        }];
        batch.expected = vec![];
        artex_domain::augment_expected(&mut batch, &s).unwrap();
        router.commit_with_shared_settings(batch).unwrap();
        let scope = router.native_scope().unwrap();
        router.native_close(&scope).unwrap();
        open(&mut router, &a, false);
        assert!(
            router
                .snapshot()
                .unwrap()
                .get("llm_profiles", &other)
                .is_none()
        );
        let seq = router.snapshot().unwrap().commit_seq;
        for _ in 0..2 {
            router.project_shared_settings().unwrap();
        }
        assert_eq!(
            router.snapshot().unwrap().commit_seq,
            seq,
            "Projection must not write on every read/open"
        );
    }
    #[test]
    fn common_settings_profile_has_one_writer_and_malformed_or_missing_recovery_source_is_explicit()
    {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let mut router = Router::new(Default::default());
        router.native_share_settings(&profile).unwrap();
        assert!(
            Router::new(Default::default())
                .native_share_settings(&profile)
                .is_err()
        );
        open(&mut router, &dir.path().join("a"), true);
        let batch = model_batch(
            &router.snapshot().unwrap(),
            &new_id("profile"),
            "fixture",
            true,
        );
        arm(&mut router, &batch);
        router
            .shared_settings
            .as_mut()
            .unwrap()
            .catalog
            .pending
            .as_mut()
            .unwrap()
            .root = dir.path().join("missing");
        let catalog = router.shared_settings.as_ref().unwrap().catalog.clone();
        router
            .shared_settings
            .as_mut()
            .unwrap()
            .persist(catalog)
            .unwrap();
        drop(router);
        assert!(
            Router::new(Default::default())
                .native_share_settings(&profile)
                .is_err()
        );
        let store = Store::open_read_only(&profile.join("shared-settings")).unwrap();
        let row = store
            .snapshot()
            .records("settings")
            .into_iter()
            .find(|r| r.data["key"] == CATALOG_KEY)
            .unwrap()
            .clone();
        let catalog = read_catalog(&store, row.data["value"].as_str().unwrap()).unwrap();
        assert!(
            catalog.pending.is_some(),
            "Unresolved edits must not be silently forgotten"
        );
    }

    #[test]
    fn common_settings_restored_workspace_can_import_new_profile_without_two_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let mut router = Router::new(Default::default());
        router
            .native_share_settings(&dir.path().join("profile"))
            .unwrap();
        open(&mut router, &a, true);
        let original = new_id("profile");
        let batch = model_batch(
            &router.snapshot().unwrap(),
            &original,
            "common default",
            true,
        );
        router.commit_with_shared_settings(batch).unwrap();
        let workspace_id = router.snapshot().unwrap().workspace_id.clone();
        let scope = router.native_scope().unwrap();
        router.native_close(&scope).unwrap();
        let extra = new_id("profile");
        {
            // A restored copy edited with the older app retains its workspace
            // identity, but a previously unknown profile became its local default.
            let mut store = Store::open(&a).unwrap();
            let s = store.snapshot();
            let mut batch = model_batch(&s, &extra, "legacy new default", true);
            let mut data = s.get("llm_profiles", &original).unwrap().data.clone();
            data["is_default"] = json!(false);
            batch.changes.push(Change::Put {
                collection: "llm_profiles".into(),
                id: original.clone(),
                data,
            });
            artex_domain::augment_expected(&mut batch, &s).unwrap();
            store.commit(batch).unwrap();
        }
        open(&mut router, &a, false);
        let s = router.snapshot().unwrap();
        assert_eq!(s.workspace_id, workspace_id);
        assert_eq!(
            s.get("llm_profiles", &original).unwrap().data["is_default"],
            true
        );
        assert_eq!(
            s.get("llm_profiles", &extra).unwrap().data["is_default"],
            false
        );
        assert!(router.shared_settings.as_ref().unwrap().catalog.imports[&workspace_id].len() >= 2);
        artex_core::validate_snapshot(&s).unwrap();
    }

    #[test]
    fn common_settings_does_not_fallback_to_empty_for_unknown_catalog_version_or_plain_search_key()
    {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        {
            let mut shared = SharedSettings::open(&profile).unwrap();
            for value in [json!("plain-placeholder"), json!(3), Value::Null, json!({})] {
                let mut bad_secret = Catalog::default();
                bad_secret
                    .current
                    .settings
                    .insert("brave_search_api_key".into(), value.to_string());
                assert!(shared.persist(bad_secret).is_err());
            }
            let mut valid = Catalog::default();
            valid.current.settings.insert(
                "brave_search_api_key".into(),
                json!({"secret_ref":new_id("secret"),"hint":"configured"}).to_string(),
            );
            shared.persist(valid).unwrap();
            let mut newer = Catalog::default();
            newer.format_version = FORMAT_VERSION + 1;
            shared.persist(newer).unwrap();
        }
        assert_eq!(
            SharedSettings::open(&profile).err().unwrap().code,
            "RECOVERY_REQUIRED"
        );
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    profiles: BTreeMap<String, Record>,
    settings: BTreeMap<String, String>,
    #[serde(default)]
    configuration: Option<SharedConfiguration>,
}
impl Preferences {
    fn from_snapshot(s: &Snapshot) -> Result<Self> {
        Ok(Self {
            profiles: s
                .records("llm_profiles")
                .into_iter()
                .map(|r| (r.header.id.clone(), r.clone()))
                .collect(),
            settings: s
                .records("settings")
                .into_iter()
                .filter_map(|r| {
                    let key = r.data["key"].as_str()?;
                    common_key(key)
                        .then(|| (key.into(), r.data["value"].as_str().unwrap_or("").into()))
                })
                .collect(),
            configuration: Some(SharedConfiguration::from_snapshot(s)?),
        })
    }
    fn validate(&self) -> Result<()> {
        let mut defaults = 0;
        for (id, row) in &self.profiles {
            if id != &row.header.id || row.header.collection != "llm_profiles" {
                return Err(Error::new("CORRUPT_STORE", "Invalid shared model identity"));
            }
            artex_core::validate_record(row)?;
            defaults += usize::from(row.data["is_default"] == true);
        }
        if defaults > 1 || self.settings.keys().any(|k| !common_key(k)) {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Invalid common settings catalog",
            ));
        }
        // Search keys are serialized SecretRefs in the original settings contract.
        for key in ["brave_search_api_key", "tavily_search_api_key"] {
            if let Some(value) = self.settings.get(key).filter(|v| !v.is_empty()) {
                let reference: Value = serde_json::from_str(value)?;
                if reference["secret_ref"].as_str().is_none() {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Search credentials require SecretRef",
                    ));
                }
                artex_core::validate_secret_parameters(
                    "ui.setSettings",
                    &json!({"api_key":reference}),
                )?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    root: PathBuf,
    workspace_id: String,
    command_id: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    format_version: u32,
    current: Preferences,
    deleted_profiles: std::collections::BTreeSet<String>,
    #[serde(default)]
    deleted_configuration: BTreeMap<String, std::collections::BTreeSet<String>>,
    /// Original imports preserve conflicting old values without activating them.
    imports: BTreeMap<String, Vec<Preferences>>,
    pending: Option<Pending>,
    #[serde(default)]
    recent_models_imported: bool,
}
impl Default for Catalog {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            current: Preferences::default(),
            deleted_profiles: Default::default(),
            deleted_configuration: Default::default(),
            imports: Default::default(),
            pending: None,
            recent_models_imported: false,
        }
    }
}
pub(super) struct SharedSettings {
    store: Store,
    catalog: Catalog,
    profile: PathBuf,
}
impl SharedSettings {
    /// A previously configured model may live in another workspace the user
    /// already opened. Import only model rows from that explicit recent list;
    /// never open its runtime, resolve keys, or replace existing common values.
    fn import_recent_models(&mut self, active: &Path) -> Result<()> {
        if self.catalog.recent_models_imported {
            return Ok(());
        }
        let bytes = match std::fs::read(self.profile.join("recent-workspaces.json")) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(Error::io(error)),
        };
        let recent: Vec<PathBuf> = bytes
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()?
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        for path in recent {
            let path = match path.canonicalize() {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(Error::io(error)),
            };
            if path == active || !seen.insert(path.clone()) {
                continue;
            }
            let source = Store::open_read_only(&path)?;
            if source.recovery()["corruption"].is_object() {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "A recent workspace requires recovery before importing its models",
                ));
            }
            let generation = source.read_generation();
            // The reduced immutable snapshot deliberately excludes work, prompts
            // and policies. First-time model recovery must not activate another
            // workspace or silently choose its agent configuration as canonical.
            if let Some(profiles) = generation
                .collections
                .get("llm_profiles")
                .filter(|rows| !rows.is_empty())
            {
                self.import(&Snapshot {
                    workspace_id: generation.workspace_id.clone(),
                    commit_seq: generation.commit_seq,
                    collections: BTreeMap::from([("llm_profiles".into(), profiles.clone())]),
                })?;
            }
        }
        let mut next = self.catalog.clone();
        next.recent_models_imported = true;
        self.persist(next)
    }
    fn open(profile: &Path) -> Result<Self> {
        std::fs::create_dir_all(profile).map_err(Error::io)?;
        let profile = profile.canonicalize().map_err(Error::io)?;
        let root = profile.join("shared-settings");
        let store = if root.exists() {
            Store::open(&root)?
        } else {
            Store::create(&root, "ARTEX shared settings")?
        };
        if store.read_only() {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Common settings require storage recovery",
            ));
        }
        let snapshot = store.snapshot();
        let saved = snapshot
            .records("settings")
            .into_iter()
            .find(|r| r.data["key"] == CATALOG_KEY);
        if saved.is_none() && snapshot.commit_seq != 0 {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Common settings catalog is missing",
            ));
        }
        let catalog: Catalog = saved
            .map(|r| read_catalog(&store, r.data["value"].as_str().unwrap_or("")))
            .transpose()?
            .unwrap_or_default();
        if catalog.format_version != FORMAT_VERSION {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Unsupported common settings format",
            ));
        }
        catalog.current.validate()?;
        for imported in catalog.imports.values().flatten() {
            imported.validate()?;
        }
        Ok(Self {
            store,
            catalog,
            profile,
        })
    }
    fn persist(&mut self, next: Catalog) -> Result<()> {
        next.current.validate()?;
        let (temporary, mut file) = temporary(&self.store)?;
        serde_json::to_writer(&mut file, &next)?;
        file.sync_all().map_err(Error::io)?;
        drop(file);
        let catalog_blob = self.store.blob_put_file(&temporary.0, "application/json")?;
        // Typed refs in the durable receipt keep both the catalog and all current
        // and preserved prompt blobs visible to Store integrity checks and GC.
        // Use normalized_params: large receipt results are themselves externalized
        // into opaque JSON blobs and would hide these roots from the collector.
        let mut references = BTreeMap::new();
        for preferences in std::iter::once(&next.current).chain(next.imports.values().flatten()) {
            if let Some(configuration) = &preferences.configuration {
                for reference in configuration.body_references() {
                    let hash = reference["sha256"]
                        .as_str()
                        .ok_or_else(|| {
                            Error::new("CORRUPT_STORE", "Invalid configuration BlobRef")
                        })?
                        .to_owned();
                    references.insert(hash, reference);
                }
            }
        }
        let s = self.store.snapshot();
        let old = s
            .records("settings")
            .into_iter()
            .find(|r| r.data["key"] == CATALOG_KEY);
        let id = old
            .map(|r| r.header.id.clone())
            .unwrap_or_else(|| new_id("setting"));
        let mut data = default_data("settings")?;
        data["key"] = json!(CATALOG_KEY);
        data["value"] = json!(serde_json::to_string(&CatalogReference {
            format_version: FORMAT_VERSION,
            catalog_blob: serde_json::from_value(catalog_blob.clone())?,
        })?);
        let expected = old
            .map(|r| Expected {
                collection: "settings".into(),
                id: id.clone(),
                version: r.header.version.clone(),
            })
            .into_iter()
            .collect();
        self.store.commit(CommandBatch {
            command_id: new_id("command"),
            method: "native.shared_settings".into(),
            normalized_params: json!({"catalog_blob":catalog_blob,"configuration_blobs":references.into_values().collect::<Vec<_>>()}),
            expected,
            changes: vec![Change::Put {
                collection: "settings".into(),
                id,
                data,
            }],
            events: vec![],
            deliveries: vec![],
            result: json!({"saved":true}),
        })?;
        self.catalog = next;
        Ok(())
    }
    fn finish(&mut self, snapshot: &Snapshot) -> Result<()> {
        let mut next = self.catalog.clone();
        let current = Preferences::from_snapshot(snapshot)?;
        if let (Some(old), Some(current)) = (&next.current.configuration, &current.configuration) {
            let now = current.root_keys();
            for (collection, old_keys) in old.root_keys() {
                next.deleted_configuration
                    .entry(collection.clone())
                    .or_default()
                    .extend(old_keys.into_iter().filter(|key| {
                        !now.get(&collection).is_some_and(|keys| keys.contains(key))
                    }));
            }
            for (collection, keys) in now {
                if let Some(deleted) = next.deleted_configuration.get_mut(&collection) {
                    deleted.retain(|key| !keys.contains(key));
                }
            }
        }
        next.deleted_profiles.extend(
            next.current
                .profiles
                .keys()
                .filter(|id| !current.profiles.contains_key(*id))
                .cloned(),
        );
        next.current = current;
        next.pending = None;
        self.persist(next)
    }
    fn recover(&mut self, active: Option<&Store>) -> Result<()> {
        let Some(pending) = self.catalog.pending.clone() else {
            return Ok(());
        };
        let active = active.filter(|s| s.read_generation().workspace_id == pending.workspace_id);
        let recovered = if active.is_none() {
            Some(Store::open_read_only(&pending.root).map_err(|_| Error::new(
                "RECOVERY_REQUIRED", "Restore the original workspace folder to finish a pending common settings save"))?)
        } else {
            None
        };
        let source = active.or(recovered.as_ref()).unwrap();
        if source.recovery()["corruption"].is_object() {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Common settings edit requires workspace recovery",
            ));
        }
        let s = source.snapshot();
        if s.workspace_id != pending.workspace_id {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Common settings recovery source changed identity",
            ));
        }
        if s.get("command_receipts", &pending.command_id).is_some() {
            if let Some(configuration) = Preferences::from_snapshot(&s)?.configuration {
                copy_configuration_blobs(source, &mut self.store, &configuration)?;
            }
            self.finish(&s)
        } else {
            let mut next = self.catalog.clone();
            next.pending = None;
            self.persist(next)
        }
    }
    fn import(&mut self, s: &Snapshot) -> Result<()> {
        let incoming = Preferences::from_snapshot(s)?;
        incoming.validate()?;
        let mut next = self.catalog.clone();
        let mut changed = false;
        let mut has_default = next
            .current
            .profiles
            .values()
            .any(|r| r.data["is_default"] == true);
        let mut names: std::collections::BTreeSet<String> = next
            .current
            .profiles
            .values()
            .filter_map(|row| row.data["name"].as_str().map(str::to_owned))
            .collect();
        for (id, r) in &incoming.profiles {
            if next.deleted_profiles.contains(id) || next.current.profiles.contains_key(id) {
                continue;
            }
            let mut r = r.clone();
            // Names were unique only within each old workspace. Keep both stable
            // IDs/credentials on import; disambiguate the display name and retain
            // its exact original in the existing import backup below.
            let base = r.data["name"].as_str().unwrap().to_owned();
            let mut name = base.clone();
            let mut suffix = 0u64;
            while names.contains(&name) {
                suffix = suffix.checked_add(1).ok_or_else(|| {
                    Error::new("SIZE_LIMIT", "Imported model name suffix exhausted")
                })?;
                name = format!("{base} ({suffix})");
            }
            names.insert(name.clone());
            r.data["name"] = json!(name);
            if r.data["is_default"] == true {
                r.data["is_default"] = json!(!has_default);
                has_default = true;
            }
            next.current.profiles.insert(id.clone(), r);
            changed = true;
        }
        for (key, value) in &incoming.settings {
            if !next.current.settings.contains_key(key) {
                let value = if key == "llm_judge_profile_id"
                    && !value.is_empty()
                    && !next.current.profiles.contains_key(value)
                {
                    String::new()
                } else {
                    value.clone()
                };
                next.current.settings.insert(key.clone(), value);
                changed = true;
            }
        }
        if let Some(mut configuration) = incoming.configuration.clone().filter(|c| !c.is_empty()) {
            configuration.without_deleted(&next.deleted_configuration);
            configuration.retain_profiles(&next.current.profiles.keys().cloned().collect());
            if let Some(current) = next.current.configuration.as_mut() {
                changed |= current.merge_missing(&configuration);
            } else {
                next.current.configuration = Some(configuration);
                changed = true;
            }
        }
        // A restored/copied workspace can keep its ID but contain new profiles.
        // Import unknown IDs on every open; keep conflicting originals once,
        // and never let an old backup reset the common default or resurrect IDs.
        let conflict = incoming.profiles.iter().any(|(id, r)| {
            next.current
                .profiles
                .get(id)
                .is_some_and(|v| comparable(&v.data) != comparable(&r.data))
        }) || incoming
            .settings
            .iter()
            .any(|(k, v)| next.current.settings.get(k) != Some(v))
            || incoming.configuration != next.current.configuration;
        let prior = next.imports.get(&s.workspace_id);
        if prior.is_none()
            || ((changed || conflict)
                && !prior
                    .is_some_and(|rows| rows.iter().any(|old| same_preferences(old, &incoming))))
        {
            next.imports
                .entry(s.workspace_id.clone())
                .or_default()
                .push(incoming);
            changed = true;
        }
        if changed { self.persist(next) } else { Ok(()) }
    }
}

fn comparable(data: &Value) -> Value {
    let mut data = data.clone();
    if let Some(o) = data.as_object_mut() {
        o.remove("created_at");
        o.remove("updated_at");
    }
    data
}
fn same_preferences(a: &Preferences, b: &Preferences) -> bool {
    a.settings == b.settings
        && a.configuration == b.configuration
        && a.profiles.len() == b.profiles.len()
        && a.profiles.iter().all(|(id, row)| {
            b.profiles
                .get(id)
                .is_some_and(|other| comparable(&row.data) == comparable(&other.data))
        })
}
fn projection(s: &Snapshot, catalog: &Catalog) -> Result<CommandBatch> {
    let mut changes = Vec::new();
    for (id, r) in &catalog.current.profiles {
        if s.get("llm_profiles", id)
            .is_none_or(|old| comparable(&old.data) != comparable(&r.data))
        {
            changes.push(Change::Put {
                collection: "llm_profiles".into(),
                id: id.clone(),
                data: r.data.clone(),
            });
        }
    }
    for r in s.records("llm_profiles") {
        if catalog.deleted_profiles.contains(&r.header.id) {
            changes.push(Change::Delete {
                collection: "llm_profiles".into(),
                id: r.header.id.clone(),
            });
        }
    }
    for (key, value) in &catalog.current.settings {
        let old = s
            .records("settings")
            .into_iter()
            .find(|r| r.data["key"] == *key);
        if old.is_some_and(|r| r.data["value"] == *value) {
            continue;
        }
        let mut data = old
            .map(|r| r.data.clone())
            .unwrap_or(default_data("settings")?);
        data["key"] = json!(key);
        data["value"] = json!(value);
        changes.push(Change::Put {
            collection: "settings".into(),
            id: old
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("setting")),
            data,
        });
    }
    if let Some(configuration) = &catalog.current.configuration {
        changes.extend(configuration.projection(s)?);
    }
    // Deletion uses the same SET NULL/CASCADE rules as an ordinary model removal.
    let changes = artex_core::expand_deletions(s, &changes)?;
    let mut batch = CommandBatch {
        command_id: new_id("command"),
        method: "migration.shared_settings.project".into(),
        normalized_params: json!({}),
        expected: vec![],
        changes,
        events: vec![],
        deliveries: vec![],
        result: json!({}),
    };
    artex_domain::augment_expected(&mut batch, s)?;
    Ok(batch)
}

impl Router {
    /// A readonly projection can reference a profile-owned prompt blob. Only
    /// currently shared typed configuration Bodies authorize this fallback;
    /// missing/corrupt workspace blobs are never replaced by unrelated backups.
    pub(super) fn configuration_body_store(&self, reference: &Value) -> Result<&Store> {
        let workspace = self
            .store
            .as_ref()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace closed"))?;
        if self.shared_settings_view.is_none() {
            return Ok(workspace);
        }
        let eligible = self.shared_settings.as_ref().is_some_and(|shared| {
            shared
                .catalog
                .current
                .configuration
                .as_ref()
                .is_some_and(|config| {
                    config
                        .body_references()
                        .iter()
                        .any(|allowed| allowed == reference)
                })
        });
        if !eligible {
            return Ok(workspace);
        }
        if workspace
            .existing_blob_ref(
                reference["sha256"].as_str().unwrap_or(""),
                reference["media_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream"),
            )?
            .is_some()
        {
            return Ok(workspace);
        }
        if let Some(shared) = self.shared_settings.as_ref() {
            if shared
                .catalog
                .current
                .configuration
                .as_ref()
                .is_some_and(|config| {
                    config
                        .body_references()
                        .iter()
                        .any(|allowed| allowed == reference)
                })
            {
                return Ok(&shared.store);
            }
        }
        Ok(workspace)
    }
    pub(super) fn shared_configuration_initialized(&self) -> bool {
        self.shared_settings
            .as_ref()
            .is_some_and(|s| s.catalog.current.configuration.is_some())
    }
    /// The host validates this profile separately from workspace execution paths.
    pub fn native_shared_profile(&self) -> Option<&Path> {
        self.shared_settings.as_ref().map(|s| s.profile.as_path())
    }
    /// Bind one application profile before opening any workspace. The catalog has
    /// its own OS writer lock and uses the existing journal/CAS recovery format.
    pub fn native_share_settings(&mut self, profile: &Path) -> Result<()> {
        if self.store.is_some() || self.shared_settings.is_some() {
            return Err(Error::new(
                "BUSY",
                "Bind common settings before opening a workspace",
            ));
        }
        let mut settings = SharedSettings::open(profile)?;
        settings.recover(None)?;
        self.shared_settings = Some(settings);
        Ok(())
    }
    pub(super) fn recover_shared_settings(&mut self) -> Result<()> {
        if let Some(settings) = self.shared_settings.as_mut() {
            let active = self.store.as_ref().filter(|_| {
                settings
                    .catalog
                    .pending
                    .as_ref()
                    .is_some_and(|p| self.shared_settings_root.as_ref() == Some(&p.root))
            });
            settings.recover(active)?;
        }
        Ok(())
    }
    pub(super) fn project_shared_settings(&mut self) -> Result<()> {
        self.shared_settings_view = None;
        let Some(settings) = self.shared_settings.as_mut() else {
            return Ok(());
        };
        let store = self
            .store
            .as_mut()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace closed"))?;
        settings.recover(Some(store))?;
        let s = store.snapshot();
        if let Some(configuration) = Preferences::from_snapshot(&s)?.configuration {
            copy_configuration_blobs(store, &mut settings.store, &configuration)?;
        }
        settings.import(&s)?;
        settings.import_recent_models(self.shared_settings_root.as_deref().ok_or_else(
            || {
                Error::new(
                    "RECOVERY_REQUIRED",
                    "Active workspace root is unavailable for model migration",
                )
            },
        )?)?;
        if !store.read_only() {
            if let Some(configuration) = &settings.catalog.current.configuration {
                copy_configuration_blobs(&settings.store, store, configuration)?;
            }
        }
        let batch = projection(&s, &settings.catalog)?;
        if batch.changes.is_empty() {
            return Ok(());
        }
        let metadata = settings
            .catalog
            .current
            .configuration
            .as_ref()
            .map(|configuration| configuration.import_metadata(&s))
            .unwrap_or_default();
        if store.read_only() {
            // A readonly workspace may display common preferences, but its source
            // bytes and commit sequence stay untouched. This view cannot commit.
            let mut view = s;
            // An untouched created Store has sequence zero and no records. The
            // derived overlay needs positive record provenance to remain a valid
            // Snapshot; this virtual read sequence never advances its disk journal.
            view.commit_seq = view.commit_seq.max(1);
            for change in batch.changes {
                let (col, id) = change.key();
                let (col, id) = (col.to_owned(), id.to_owned());
                match change {
                    Change::Delete { .. } => {
                        if let Some(rows) = view.collections.get_mut(&col) {
                            rows.remove(&id);
                        }
                    }
                    Change::Put { data, .. } => {
                        let mut row = view.get(&col, &id).cloned().unwrap_or_else(|| Record {
                            header: artex_core::Header {
                                collection: col.clone(),
                                id: id.clone(),
                                version: "1".into(),
                                commit_seq: view.commit_seq.to_string(),
                                created_at: artex_core::now(),
                                updated_at: artex_core::now(),
                                legacy_key: Value::Null,
                            },
                            data: Value::Null,
                        });
                        row.data = data;
                        if let Some(meta) = metadata.get(&(col.clone(), id.clone())) {
                            row.header.created_at = meta.created_at.clone();
                            row.header.updated_at = meta.updated_at.clone();
                            row.header.legacy_key = meta.legacy_key.clone();
                            for (key, timestamp) in [
                                ("created_at", &meta.created_at),
                                ("updated_at", &meta.updated_at),
                            ] {
                                if row.data.get(key).is_some() {
                                    row.data[key] = json!(timestamp);
                                }
                            }
                        }
                        view.collections.entry(col).or_default().insert(id, row);
                    }
                }
            }
            artex_core::validate_snapshot(&view)?;
            self.shared_settings_view = Some(std::sync::Arc::new(
                artex_store::ReadGeneration::new(std::sync::Arc::new(view)),
            ));
        } else {
            store.commit_import(batch, metadata)?;
        }
        Ok(())
    }
    /// Persist the intent before the workspace journal, then seal the new catalog
    /// before acknowledging success. Retry/restart resolves either crash boundary.
    pub(super) fn commit_with_shared_settings(
        &mut self,
        batch: CommandBatch,
    ) -> Result<artex_core::Receipt> {
        self.recover_shared_settings()?;
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace closed"))?;
        let snapshot = store.read_generation();
        let common = self.shared_settings.is_some()
            && batch.changes.iter().any(|ch| {
                let (col, id) = ch.key();
                col == "llm_profiles"
                    || configuration_change(&snapshot, ch)
                    || (col == "settings"
                        && match ch {
                            Change::Put { data, .. } => {
                                data["key"].as_str().is_some_and(common_key)
                            }
                            Change::Delete { .. } => snapshot
                                .get(col, id)
                                .and_then(|r| r.data["key"].as_str())
                                .is_some_and(common_key),
                        })
            });
        if common {
            let root = self.shared_settings_root.clone().ok_or_else(|| {
                Error::new(
                    "RECOVERY_REQUIRED",
                    "Common settings recovery root unavailable",
                )
            })?;
            let shared = self.shared_settings.as_mut().unwrap();
            let mut next = shared.catalog.clone();
            next.pending = Some(Pending {
                root,
                workspace_id: snapshot.workspace_id.clone(),
                command_id: batch.command_id.clone(),
            });
            shared.persist(next)?;
        }
        let result = self.store.as_mut().unwrap().commit(batch);
        if common {
            self.recover_shared_settings()?;
        }
        result
    }
}

fn configuration_change(snapshot: &Snapshot, change: &Change) -> bool {
    let (collection, id) = change.key();
    if !matches!(
        collection,
        "agents"
            | "agent_prompts"
            | "agent_prompt_vars"
            | "mcp_servers"
            | "agent_visibility"
            | "agent_skill_visibility"
            | "tools"
            | "agent_triggers"
            | "agent_features"
            | "notification_channels"
            | "intercept_rules"
    ) {
        return false;
    }
    match change {
        Change::Delete { .. } => true,
        Change::Put { data, .. } => {
            let strip = |data: &Value| {
                let mut result = comparable(data);
                if collection == "agent_triggers" {
                    if let Some(object) = result.as_object_mut() {
                        object.remove("last_fire");
                    }
                }
                result
            };
            snapshot
                .get(collection, id)
                .is_none_or(|old| strip(&old.data) != strip(data))
        }
    }
}
