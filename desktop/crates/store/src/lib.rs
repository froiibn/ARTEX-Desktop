#![forbid(unsafe_code)]

use artex_core::{
    Change, CommandBatch, Error, Header, Receipt, Record, Result, Snapshot, collection_prefix,
    new_id, now, parse_u64, valid_id, validate_record, validate_snapshot,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
mod maintenance;
pub mod query;
pub use maintenance::{GcOptions, GcReport};
pub use query::{QueryIndex, ReadGeneration, SearchPage};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportMetadata {
    pub legacy_key: Value,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointTicket {
    pub generation: String,
    pub base_seq: String,
}
struct CheckpointJob {
    ticket: CheckpointTicket,
    _state: Arc<Snapshot>,
    worker: std::thread::JoinHandle<Result<Value>>,
}

const HEADER_LEN: usize = 104;
const FOOTER: &[u8; 8] = b"ARTEXEND";
const MAX_PAYLOAD: usize = 16_777_216;
const SEGMENT_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tombstone {
    collection: String,
    id: String,
    version: String,
    commit_seq: String,
    deleted_at: String,
    legacy_key: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mutation {
    op: String,
    collection: String,
    id: String,
    expected_version: String,
    new_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<Record>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    format_version: u32,
    committed_at: String,
    command_id: String,
    method: String,
    normalized_params: Value,
    expected: Vec<artex_core::Expected>,
    mutations: Vec<Mutation>,
    events: Vec<Record>,
    deliveries: Vec<Record>,
    result: Value,
}

/// One native OS file lock and one append stream own the workspace writer.
/// Snapshots are deep immutable values; readers can never mutate committed state.
pub struct Store {
    path: PathBuf,
    _lock: Arc<File>,
    journal: Option<File>,
    journal_path: PathBuf,
    state: Snapshot,
    tombstones: BTreeMap<(String, String), Tombstone>,
    last_hash: [u8; 32],
    generation: u64,
    read_only: bool,
    requested_read_only: bool,
    corruption: Option<Value>,
    valid_generations: Vec<String>,
    name: String,
    checkpoint_job: Option<CheckpointJob>,
    read_cache: Mutex<Option<Arc<ReadGeneration>>>,
    read_pins: Mutex<Vec<std::sync::Weak<ReadGeneration>>>,
    pending_import: Option<BTreeMap<(String, String), ImportMetadata>>,
    #[cfg(test)]
    fault: Option<String>,
}
/// A worker may read a pinned generation and immutable blobs, but cannot append
/// journal records. Sharing the exact native lock handle keeps the workspace
/// protected even if its writer closes while a read job is cancelling.
pub struct StoreReadView {
    path: PathBuf,
    _lock: Arc<File>,
    generation: Arc<ReadGeneration>,
    can_publish_blobs: bool,
}
impl StoreReadView {
    pub fn blob_put_file_cancellable(
        &mut self,
        source: &Path,
        media_type: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Value> {
        if !self.can_publish_blobs {
            return Err(Error::new(
                "READ_ONLY",
                "Read-only workspace cannot publish migration blobs",
            ));
        }
        publish_blob_file(&self.path, source, media_type, None, cancel)
    }
    pub fn snapshot(&self) -> Snapshot {
        (*self.generation.snapshot).clone()
    }
    /// Preparation may publish verified immutable blobs. No graph or journal
    /// mutation is possible; abandoned publications age into orphan GC.
    pub fn blob_put(&mut self, bytes: &[u8], media_type: &str) -> Result<Value> {
        if !self.can_publish_blobs {
            return Err(Error::new(
                "READ_ONLY",
                "Read-only workspace cannot publish migration blobs",
            ));
        }
        let sha = hex::encode(hash(bytes));
        let reference =
            json!({"sha256":sha,"length":bytes.len().to_string(),"media_type":media_type});
        let destination = blob_file_path(&self.path, &reference)?;
        let directory = destination.parent().unwrap();
        fs::create_dir_all(directory)?;
        if !destination.exists() {
            let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
            write_new_sync(&temporary, bytes)?;
            if let Err(error) = artex_platform::install_new(&temporary, &destination) {
                if !destination.exists() {
                    return Err(error.into());
                }
                let _ = fs::remove_file(&temporary);
            }
            sync_directory(directory)?;
            sync_directory(directory.parent().unwrap())?;
        }
        copy_blob_file(&self.path, &reference, &mut std::io::sink())?;
        Ok(reference)
    }
    pub fn read_generation(&self) -> Arc<ReadGeneration> {
        self.generation.clone()
    }
    pub fn existing_blob_ref(&self, digest: &str, media_type: &str) -> Result<Option<Value>> {
        existing_blob_reference(&self.path, digest, media_type)
    }
    pub fn copy_blob_to(&self, reference: &Value, writer: &mut impl Write) -> Result<u64> {
        copy_blob_file(&self.path, reference, writer)
    }
}
impl Store {
    pub fn read_view(&self) -> StoreReadView {
        StoreReadView {
            path: self.path.clone(),
            _lock: self._lock.clone(),
            generation: self.read_generation(),
            can_publish_blobs: !self.read_only,
        }
    }
    pub fn temporary_path(&self, extension: &str) -> Result<PathBuf> {
        if extension.is_empty()
            || extension.len() > 12
            || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid temporary file extension",
            ));
        }
        let path = self
            .path
            .join("tmp")
            .join(format!("{}.{}", new_id("command"), extension));
        reject_links(&path)?;
        Ok(path)
    }
    pub fn existing_blob_ref(&self, digest: &str, media_type: &str) -> Result<Option<Value>> {
        existing_blob_reference(&self.path, digest, media_type)
    }
    pub fn create(path: &Path, name: &str) -> Result<Self> {
        if name.trim().is_empty() {
            return Err(Error::new("INVALID_PARAMS", "Workspace name is empty"));
        }
        reject_links(path)?;
        if path.exists() && fs::read_dir(path)?.next().is_some() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Workspace directory is not empty",
            ));
        }
        fs::create_dir_all(path)?;
        let lock = lock_workspace(path)?;
        for dir in [
            "store/manifests",
            "store/snapshots",
            "store/journal",
            "objects/blobs",
            "backups",
            "tmp",
        ] {
            fs::create_dir_all(path.join(dir))?;
        }
        let project = json!({"format":"artex-workspace","format_version":1,"workspace_id":new_id("workspace"),"name":name,"created_at":now()});
        atomic_new(&path.join("project.json"), &serde_json::to_vec(&project)?)?;
        let journal_path = path.join("store/journal/00000000000000000001.jnl");
        atomic_new(&journal_path, &[])?;
        let journal = OpenOptions::new()
            .write(true)
            .append(true)
            .open(&journal_path)?;
        let mut store = Self {
            path: path.into(),
            _lock: Arc::new(lock),
            journal: Some(journal),
            journal_path,
            state: Snapshot {
                workspace_id: project["workspace_id"].as_str().unwrap().into(),
                commit_seq: 0,
                collections: BTreeMap::new(),
            },
            tombstones: BTreeMap::new(),
            last_hash: [0; 32],
            generation: 0,
            read_only: false,
            requested_read_only: false,
            corruption: None,
            valid_generations: Vec::new(),
            name: name.into(),
            checkpoint_job: None,
            read_cache: Mutex::new(None),
            read_pins: Mutex::new(Vec::new()),
            pending_import: None,
            #[cfg(test)]
            fault: None,
        };
        store.checkpoint()?;
        store.checkpoint()?;
        Ok(store)
    }
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_mode(path, false)
    }
    pub fn open_read_only(path: &Path) -> Result<Self> {
        Self::open_with_mode(path, true)
    }
    pub fn open_with_mode(path: &Path, requested_read_only: bool) -> Result<Self> {
        reject_links(path)?;
        if !path.join("project.json").is_file() {
            return Err(Error::new("NOT_FOUND", "Project file is missing"));
        }
        let lock = lock_workspace_mode(path, requested_read_only)?;
        let project: Value = serde_json::from_slice(&fs::read(path.join("project.json"))?)?;
        if project["format"] != "artex-workspace"
            || project["format_version"] != 1
            || !project["workspace_id"]
                .as_str()
                .is_some_and(|v| valid_id(v, "workspace"))
        {
            return Err(Error::new(
                "VERSION_MISMATCH",
                "Workspace project format is unsupported",
            ));
        }
        let workspace_id = project["workspace_id"].as_str().unwrap().to_owned();
        let mut store = Self {
            path: path.into(),
            _lock: Arc::new(lock),
            journal: None,
            journal_path: PathBuf::new(),
            state: Snapshot {
                workspace_id,
                commit_seq: 0,
                collections: BTreeMap::new(),
            },
            tombstones: BTreeMap::new(),
            last_hash: [0; 32],
            generation: 0,
            read_only: false,
            requested_read_only,
            corruption: None,
            valid_generations: Vec::new(),
            name: project["name"]
                .as_str()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Workspace name is missing"))?
                .into(),
            checkpoint_job: None,
            read_cache: Mutex::new(None),
            read_pins: Mutex::new(Vec::new()),
            pending_import: None,
            #[cfg(test)]
            fault: None,
        };
        let mut manifests = list_numbered(&path.join("store/manifests"), "json")?;
        manifests.reverse();
        let mut minimum_recovery_seq = 0;
        for (_, manifest_path) in manifests {
            let bytes = fs::read(&manifest_path)?;
            if let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(s) = manifest["base_seq"]
                    .as_str()
                    .and_then(|s| parse_u64(s).ok())
                {
                    minimum_recovery_seq = minimum_recovery_seq.max(s);
                }
                if let Some(s) = manifest["generation"]
                    .as_str()
                    .and_then(|s| parse_u64(s).ok())
                {
                    store.generation = store.generation.max(s);
                }
                match store.load_manifest(&manifest) {
                    Ok((snapshot, tombstones, hash)) => {
                        store
                            .valid_generations
                            .push(manifest["generation"].as_str().unwrap().into());
                        if store.valid_generations.len() == 1 {
                            store.state = snapshot;
                            store.tombstones = tombstones;
                            store.last_hash = hash;
                        }
                    }
                    Err(e) => {
                        store.corruption = Some(
                            json!({"kind":"checkpoint","file":manifest_path.file_name().unwrap().to_string_lossy(),"error":e.message,"recoverable":true}),
                        )
                    }
                }
            } else {
                store.corruption = Some(
                    json!({"kind":"checkpoint","file":manifest_path.file_name().unwrap().to_string_lossy(),"error":"Invalid manifest JSON","recoverable":true}),
                );
            }
        }
        store.replay()?;
        if store.state.commit_seq < minimum_recovery_seq {
            store.mark_corruption(json!({"kind":"missing_chain","lost_range":{"from":(store.state.commit_seq+1).to_string(),"to":minimum_recovery_seq.to_string()}}));
        }
        if let Err(e) = validate_snapshot(&store.state)
            .and_then(|_| store.check_blobs(&serde_json::to_value(&store.state).unwrap()))
        {
            store.mark_corruption(json!({"kind":"state_or_blob","error":e.to_string()}));
        }
        // Requested read-only is distinct during replay: valid segments must
        // all replay, whereas detected corruption stops at the safe prefix.
        store.read_only |= requested_read_only;
        if !store.read_only {
            if store.journal_path.as_os_str().is_empty() {
                store.rotate_journal()?;
            } else {
                store.journal = Some(OpenOptions::new().append(true).open(&store.journal_path)?);
            }
            // A damaged latest checkpoint is recoverable only when the entire journal restored its sequence.
            if store
                .corruption
                .as_ref()
                .is_some_and(|v| v["recoverable"] == true)
            {
                store.corruption = Some(
                    json!({"kind":"checkpoint_recovered","last_valid_seq":store.state.commit_seq.to_string()}),
                );
            }
        }
        Ok(store)
    }
    pub fn active_read_views_count(&self) -> usize {
        Arc::strong_count(&self._lock).saturating_sub(1)
    }
    pub fn wait_read_views(&self, timeout: std::time::Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        while self.active_read_views_count() != 0 {
            if std::time::Instant::now() >= deadline {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Store readers have not released the native workspace lock",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.clone()
    }
    pub fn read_only(&self) -> bool {
        self.read_only
    }
    pub fn info(&self) -> Value {
        json!({"workspace_id":self.state.workspace_id,"name":self.name,"format_version":1,"commit_seq":self.state.commit_seq.to_string(),
        "read_only":self.read_only,"recovery":self.recovery(),"capabilities":["object_graph","journal_v1","checkpoint_v1","blob_sha256","native_writer_lock"]})
    }
    pub fn recovery(&self) -> Value {
        json!({"mode":if self.read_only {"read_only"}else{"normal"},"valid_generations":self.valid_generations,
        "last_valid_seq":self.state.commit_seq.to_string(),"corruption":self.corruption,"lost_range":self.corruption.as_ref().and_then(|c|c.get("lost_range")).cloned().unwrap_or(Value::Null),"recovery_copy_capability":null})
    }
    fn mark_corruption(&mut self, details: Value) {
        self.read_only = true;
        self.journal = None;
        self.corruption = Some(details);
    }
    fn writable(&self) -> Result<()> {
        if self.read_only {
            let mut e = Error::new(
                "RECOVERY_REQUIRED",
                "Workspace is read-only after corruption or a failed write",
            );
            e.recovery_required = true;
            e.details = self.info();
            Err(e)
        } else {
            Ok(())
        }
    }
    pub fn commit(&mut self, batch: CommandBatch) -> Result<Receipt> {
        self.writable()?;
        if !valid_id(&batch.command_id, "command") || batch.method.trim().is_empty() {
            return Err(Error::new("INVALID_PARAMS", "Invalid command ID or method"));
        }
        // Deduplication deliberately precedes CAS: a retry retains its old expected versions.
        if let Some(record) = self.state.get("command_receipts", &batch.command_id) {
            if record.data["method"] != batch.method
                || record.data["normalized_params"] != batch.normalized_params
                || record.data["expected"] != serde_json::to_value(&batch.expected)?
            {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Command ID was previously used with different inputs",
                ));
            }
            return Ok(Receipt {
                command_id: batch.command_id,
                commit_seq: record.data["commit_seq"].as_str().unwrap().into(),
                result: record.data["result"].clone(),
            });
        }
        artex_core::validate_secret_parameters(&batch.method, &batch.normalized_params)?;
        let mut expected_seen = BTreeSet::new();
        for expected in &batch.expected {
            if !expected_seen.insert((&expected.collection, &expected.id)) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Duplicate expected version entry",
                ));
            }
            let actual = self
                .state
                .get(&expected.collection, &expected.id)
                .map(|r| parse_u64(&r.header.version))
                .transpose()?
                .unwrap_or(0);
            if actual != parse_u64(&expected.version)? {
                let mut e = Error::new("VERSION_CONFLICT", "Object version has changed");
                e.details = json!({"collection":expected.collection,"id":expected.id,"expected":expected.version,"actual":actual.to_string()});
                return Err(e);
            }
        }
        let seq = self
            .state
            .commit_seq
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Commit sequence exhausted"))?;
        let committed_at = now();
        let mut candidate = self.state.clone();
        candidate.commit_seq = seq;
        let mut tombstones = self.tombstones.clone();
        let mut mutations = Vec::new();
        let changes = artex_core::expand_deletions(&self.state, &batch.changes)?;
        for change in changes {
            let (collection, id) = change.key();
            if collection == "command_receipts" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Command receipts are store-owned",
                ));
            }
            if !valid_id(id, collection_prefix(collection)?) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "ID prefix does not match collection",
                ));
            }
            let previous = self.state.get(collection, id);
            if previous.is_some()
                && !batch
                    .expected
                    .iter()
                    .any(|e| e.collection == collection && e.id == id)
            {
                let mut error = Error::new(
                    "VERSION_CONFLICT",
                    "Expected version is required for every existing object, including cascade dependencies",
                );
                error.details = json!({"collection":collection,"id":id,"actual":previous.unwrap().header.version});
                return Err(error);
            }
            let old_version = previous
                .map(|r| parse_u64(&r.header.version))
                .transpose()?
                .unwrap_or(0);
            let new_version = old_version
                .checked_add(1)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Object version exhausted"))?;
            match change {
                Change::Put {
                    collection,
                    id,
                    mut data,
                } => {
                    if tombstones.contains_key(&(collection.clone(), id.clone())) {
                        return Err(Error::new(
                            "VERSION_CONFLICT",
                            "Deleted entity IDs cannot be reused",
                        ));
                    }
                    let object = data.as_object_mut().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Record data must be an object")
                    })?;
                    let typed_primary = artex_core::data_model()["tables"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|t| t["name"] == collection)
                        .is_some_and(|t| {
                            t["columns"].as_array().unwrap().iter().any(|c| {
                                c["name"] == "id"
                                    && c["desktop_schema"]["pattern"]
                                        == format!(
                                            "^{}_[0-9a-f]{{32}}$",
                                            collection_prefix(&collection).unwrap()
                                        )
                            })
                        });
                    if typed_primary {
                        if object
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|v| !v.is_empty() && v != id)
                        {
                            return Err(Error::new(
                                "INVALID_PARAMS",
                                "Record primary ID differs from header",
                            ));
                        }
                        object.insert("id".into(), json!(id));
                    }
                    let imported = self
                        .pending_import
                        .as_ref()
                        .and_then(|m| m.get(&(collection.clone(), id.clone())));
                    if object.contains_key("created_at") {
                        object.insert(
                            "created_at".into(),
                            json!(
                                previous
                                    .map(|r| r.header.created_at.as_str())
                                    .unwrap_or_else(|| imported
                                        .map(|m| m.created_at.as_str())
                                        .unwrap_or(&committed_at))
                            ),
                        );
                    }
                    if object.contains_key("updated_at") {
                        object.insert(
                            "updated_at".into(),
                            json!(
                                imported
                                    .map(|m| m.updated_at.as_str())
                                    .unwrap_or(&committed_at)
                            ),
                        );
                    }
                    let record = Record {
                        header: Header {
                            id: id.clone(),
                            collection: collection.clone(),
                            version: new_version.to_string(),
                            commit_seq: seq.to_string(),
                            created_at: previous
                                .map(|r| r.header.created_at.clone())
                                .unwrap_or_else(|| {
                                    imported
                                        .map(|m| m.created_at.clone())
                                        .unwrap_or_else(|| committed_at.clone())
                                }),
                            updated_at: imported
                                .map(|m| m.updated_at.clone())
                                .unwrap_or_else(|| committed_at.clone()),
                            legacy_key: previous
                                .map(|r| r.header.legacy_key.clone())
                                .unwrap_or_else(|| {
                                    imported
                                        .map(|m| m.legacy_key.clone())
                                        .unwrap_or(Value::Null)
                                }),
                        },
                        data,
                    };
                    validate_record(&record)?;
                    if collection == "traffic_evidence_snapshots"
                        && previous.is_some_and(|r| r.data != record.data)
                    {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "Evidence snapshots are immutable",
                        ));
                    }
                    candidate
                        .collections
                        .entry(collection.clone())
                        .or_default()
                        .insert(id.clone(), record.clone());
                    mutations.push(Mutation {
                        op: "put".into(),
                        collection,
                        id,
                        expected_version: old_version.to_string(),
                        new_version: new_version.to_string(),
                        record: Some(record),
                    });
                }
                Change::Delete { collection, id } => {
                    let old =
                        previous.ok_or_else(|| Error::new("NOT_FOUND", "Entity does not exist"))?;
                    candidate
                        .collections
                        .entry(collection.clone())
                        .or_default()
                        .remove(&id);
                    tombstones.insert(
                        (collection.clone(), id.clone()),
                        Tombstone {
                            collection: collection.clone(),
                            id: id.clone(),
                            version: new_version.to_string(),
                            commit_seq: seq.to_string(),
                            deleted_at: committed_at.clone(),
                            legacy_key: old.header.legacy_key.clone(),
                        },
                    );
                    mutations.push(Mutation {
                        op: "delete".into(),
                        collection,
                        id,
                        expected_version: old_version.to_string(),
                        new_version: new_version.to_string(),
                        record: None,
                    });
                }
            }
        }
        let mut special_keys: BTreeSet<(String, String)> = BTreeSet::new();
        let mut events = batch.events;
        let mut deliveries = batch.deliveries;
        for (required, records) in [
            ("domain_events", &mut events),
            ("agent_deliveries", &mut deliveries),
        ] {
            for record in records {
                if record.header.collection != required
                    || !special_keys.insert((required.into(), record.header.id.clone()))
                    || candidate.get(required, &record.header.id).is_some()
                    || tombstones.contains_key(&(required.into(), record.header.id.clone()))
                {
                    return Err(Error::new(
                        "VERSION_CONFLICT",
                        "Event/delivery is duplicated or has the wrong collection",
                    ));
                }
                record.header.version = "1".into();
                record.header.commit_seq = seq.to_string();
                record.header.created_at = committed_at.clone();
                record.header.updated_at = committed_at.clone();
                validate_record(record)?;
                candidate
                    .collections
                    .entry(required.into())
                    .or_default()
                    .insert(record.header.id.clone(), record.clone());
            }
        }
        let mut result = batch.result;
        if serde_json::to_vec(&result)?.len() > 65536 {
            let reference = self.blob_put(&serde_json::to_vec(&result)?, "application/json")?;
            result = json!({"result_ref":reference});
        }
        let payload = Payload {
            format_version: 1,
            committed_at: committed_at.clone(),
            command_id: batch.command_id.clone(),
            method: batch.method,
            normalized_params: batch.normalized_params,
            expected: batch.expected,
            mutations,
            events,
            deliveries,
            result: result.clone(),
        };
        install_receipt(&mut candidate, &payload, seq)?;
        validate_snapshot(&candidate)?;
        self.check_blobs(&serde_json::to_value(&candidate)?)?;
        let bytes = serde_json::to_vec(&payload)?;
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::new("SIZE_LIMIT", "Journal payload exceeds 16 MiB"));
        }
        let frame = encode_frame(&bytes, seq, self.last_hash, artex_core::random_128())?;
        if self
            .journal
            .as_ref()
            .map(|f| f.metadata().map(|m| m.len()))
            .transpose()?
            .unwrap_or(0)
            >= SEGMENT_LIMIT
        {
            self.rotate_journal()?;
        }
        let fault = self.fault_name();
        let write_result = (|| -> Result<()> {
            inject(fault.as_deref(), "before_append")?;
            let file = self
                .journal
                .as_mut()
                .ok_or_else(|| Error::new("IO_FAILURE", "Journal stream is unavailable"))?;
            if fault.is_none() {
                file.write_all(&frame)?;
            } else {
                file.write_all(&frame[..HEADER_LEN])?;
                inject(fault.as_deref(), "after_header")?;
                file.write_all(&frame[HEADER_LEN..frame.len() - 8])?;
                inject(fault.as_deref(), "after_payload")?;
                file.write_all(&frame[frame.len() - 8..])?;
                inject(fault.as_deref(), "after_footer")?;
            }
            file.sync_all()?;
            inject(fault.as_deref(), "after_sync")?;
            Ok(())
        })();
        if let Err(e) = write_result {
            self.mark_corruption(json!({"kind":"write_failure","error":e.to_string(),"command_id":payload.command_id}));
            return Err(e);
        }
        self.last_hash = hash(&frame);
        self.state = candidate;
        *self.read_cache.lock().unwrap() = None;
        self.tombstones = tombstones;
        if let Err(e) = inject(fault.as_deref(), "after_memory") {
            self.mark_corruption(json!({"kind":"write_failure","error":e.to_string(),"command_id":payload.command_id}));
            return Err(e);
        }
        Ok(Receipt {
            command_id: payload.command_id,
            commit_seq: seq.to_string(),
            result,
        })
    }
    fn rotate_journal(&mut self) -> Result<()> {
        let path = self.path.join(format!(
            "store/journal/{:020}.jnl",
            self.state.commit_seq + 1
        ));
        if path != self.journal_path || !path.exists() {
            atomic_new(&path, &[])?;
        }
        let file = OpenOptions::new().append(true).write(true).open(&path)?;
        self.journal = Some(file);
        self.journal_path = path;
        Ok(())
    }
    pub fn checkpoint(&mut self) -> Result<Value> {
        self.begin_checkpoint()?;
        self.wait_checkpoint()
    }
    fn load_manifest(
        &self,
        manifest: &Value,
    ) -> Result<(Snapshot, BTreeMap<(String, String), Tombstone>, [u8; 32])> {
        if manifest["format"] != "artex-checkpoint"
            || manifest["format_version"] != 1
            || manifest["workspace_id"] != self.state.workspace_id
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Manifest format/workspace mismatch",
            ));
        }
        let mut unsigned = manifest.clone();
        unsigned
            .as_object_mut()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Invalid manifest"))?
            .remove("manifest_sha256");
        if manifest["manifest_sha256"] != hex::encode(hash(&canonical_bytes(&unsigned)?)) {
            return Err(Error::new("CORRUPT_STORE", "Manifest hash mismatch"));
        }
        let generation = parse_u64(required_string(manifest, "generation")?)?;
        let base_seq = parse_u64(required_string(manifest, "base_seq")?)?;
        if parse_u64(required_string(manifest, "journal_start_seq")?)?
            != base_seq
                .checked_add(1)
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Manifest sequence overflow"))?
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Manifest replay sequence mismatch",
            ));
        }
        let mut state = Snapshot {
            workspace_id: self.state.workspace_id.clone(),
            commit_seq: base_seq,
            collections: BTreeMap::new(),
        };
        let mut tombstones = BTreeMap::new();
        let mut seen = BTreeSet::new();
        for entry in manifest["files"]
            .as_array()
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Missing manifest files"))?
        {
            let relative = required_string(entry, "path")?;
            safe_relative(relative)?;
            if !relative.starts_with(&format!("store/snapshots/{generation:020}/"))
                || !seen.insert(relative.to_owned())
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Invalid or duplicate snapshot path",
                ));
            }
            let path = self.path.join(relative);
            reject_links(&path)?;
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 != parse_u64(required_string(entry, "length")?)?
                || hex::encode(hash(&bytes)) != required_string(entry, "sha256")?
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Snapshot file hash or length mismatch",
                ));
            }
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                return Err(Error::new("CORRUPT_STORE", "Snapshot lacks final LF"));
            }
            let mut count = 0;
            for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
                if relative.ends_with("/_tombstones.ndjson") {
                    let t: Tombstone = serde_json::from_slice(line)?;
                    if !valid_id(&t.id, collection_prefix(&t.collection)?)
                        || parse_u64(&t.version)? < 2
                        || parse_u64(&t.commit_seq)? > base_seq
                        || tombstones
                            .insert((t.collection.clone(), t.id.clone()), t)
                            .is_some()
                    {
                        return Err(Error::new(
                            "CORRUPT_STORE",
                            "Invalid or duplicate tombstone",
                        ));
                    }
                } else {
                    let r: Record = serde_json::from_slice(line)?;
                    if path.file_name().unwrap().to_string_lossy()
                        != format!("{}.ndjson", r.header.collection)
                    {
                        return Err(Error::new(
                            "CORRUPT_STORE",
                            "Record stored in incorrect collection file",
                        ));
                    }
                    let records = state
                        .collections
                        .entry(r.header.collection.clone())
                        .or_default();
                    if records.insert(r.header.id.clone(), r).is_some() {
                        return Err(Error::new("CORRUPT_STORE", "Duplicate snapshot record"));
                    }
                }
                count += 1;
            }
            if count != parse_u64(required_string(entry, "record_count")?)? {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Snapshot record count mismatch",
                ));
            }
        }
        for collection in artex_core::registry()["collections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] != "read_projection")
        {
            if !seen.contains(&format!(
                "store/snapshots/{generation:020}/{}.ndjson",
                collection["collection"].as_str().unwrap()
            )) {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Canonical collection snapshot is omitted",
                ));
            }
        }
        if !seen.contains(&format!(
            "store/snapshots/{generation:020}/_tombstones.ndjson"
        )) {
            return Err(Error::new("CORRUPT_STORE", "Tombstone snapshot is omitted"));
        }
        if state.records("command_receipts").len() as u64
            != parse_u64(required_string(manifest, "command_receipt_count")?)?
        {
            return Err(Error::new("CORRUPT_STORE", "Receipt count mismatch"));
        }
        for (collection, id) in tombstones.keys() {
            if state.get(collection, id).is_some() {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Live record reuses tombstoned ID",
                ));
            }
        }
        validate_snapshot(&state)?;
        let digest = hex::decode(required_string(manifest, "base_frame_sha256")?)
            .map_err(|_| Error::new("CORRUPT_STORE", "Invalid base hash"))?;
        let digest = digest
            .try_into()
            .map_err(|_| Error::new("CORRUPT_STORE", "Invalid base hash length"))?;
        Ok((state, tombstones, digest))
    }
    fn replay(&mut self) -> Result<()> {
        let segments = list_numbered(&self.path.join("store/journal"), "jnl")?;
        let snapshot_base = self.state.commit_seq;
        let snapshot_hash = self.last_hash;
        let first_seq = segments
            .first()
            .map(|(seq, _)| *seq)
            .unwrap_or(snapshot_base + 1);
        let mut expected_seq = first_seq;
        let mut previous = [0; 32];
        let mut saw_base = snapshot_base == 0;
        if first_seq > 1 {
            let anchor = first_seq - 1;
            let mut found = None;
            for (_, path) in list_numbered(&self.path.join("store/manifests"), "json")? {
                let m: Value = match serde_json::from_slice(&fs::read(path)?) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if m["base_seq"] == anchor.to_string() {
                    if let Ok((_, _, h)) = self.load_manifest(&m) {
                        found = Some(h);
                        break;
                    }
                }
            }
            if let Some(h) = found {
                previous = h;
                saw_base = anchor == snapshot_base;
            } else {
                self.mark_corruption(
                    json!({"kind":"missing_journal_anchor","first_seq":first_seq.to_string()}),
                );
                return Ok(());
            }
        }
        let total = segments.len();
        for (index, (first_seq, path)) in segments.into_iter().enumerate() {
            if first_seq != expected_seq {
                self.mark_corruption(json!({"kind":"segment_gap","expected":expected_seq.to_string(),"found":first_seq.to_string()}));
                break;
            }
            reject_links(&path)?;
            let mut file = OpenOptions::new().read(true).open(&path)?;
            let length = file.metadata()?.len();
            let mut offset = 0;
            loop {
                if offset == length {
                    break;
                }
                let mut header = [0; HEADER_LEN];
                let available = (length - offset).min(HEADER_LEN as u64) as usize;
                file.read_exact(&mut header[..available])?;
                if available < HEADER_LEN {
                    if index + 1 == total {
                        self.preserve_tail(&path, offset)?;
                    } else {
                        self.mark_corruption(json!({"kind":"incomplete_middle_segment","file":path.file_name().unwrap().to_string_lossy(),"offset":offset}));
                    }
                    break;
                }
                if &header[0..8] != b"ARTEXJ01"
                    || u16::from_le_bytes(header[8..10].try_into().unwrap()) != 1
                    || u16::from_le_bytes(header[10..12].try_into().unwrap()) != 0
                {
                    self.mark_corruption(json!({"kind":"frame_header","file":path.file_name().unwrap().to_string_lossy(),"offset":offset}));
                    break;
                }
                let payload_length =
                    u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
                if payload_length > MAX_PAYLOAD {
                    self.mark_corruption(json!({"kind":"frame_size","offset":offset}));
                    break;
                }
                let frame_length = HEADER_LEN + payload_length + 8;
                if length - offset < frame_length as u64 {
                    if index + 1 == total {
                        self.preserve_tail(&path, offset)?;
                    } else {
                        self.mark_corruption(
                            json!({"kind":"incomplete_middle_segment","offset":offset}),
                        );
                    }
                    break;
                }
                let mut frame = header.to_vec();
                frame.resize(frame_length, 0);
                file.read_exact(&mut frame[HEADER_LEN..])?;
                let seq = u64::from_le_bytes(header[16..24].try_into().unwrap());
                if seq != expected_seq
                    || header[40..72] != previous
                    || header[72..104] != hash(&frame[HEADER_LEN..HEADER_LEN + payload_length])
                    || &frame[HEADER_LEN + payload_length..] != FOOTER
                {
                    self.mark_corruption(json!({"kind":"complete_frame_corruption","file":path.file_name().unwrap().to_string_lossy(),"offset":offset,"expected_seq":expected_seq.to_string()}));
                    break;
                }
                let digest = hash(&frame);
                if seq == snapshot_base {
                    saw_base = true;
                    if digest != snapshot_hash {
                        self.mark_corruption(
                            json!({"kind":"checkpoint_chain_mismatch","seq":seq.to_string()}),
                        );
                        break;
                    }
                }
                if seq > snapshot_base {
                    let payload: Payload = match serde_json::from_slice(
                        &frame[HEADER_LEN..HEADER_LEN + payload_length],
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            self.mark_corruption(
                                json!({"kind":"invalid_payload","error":e.to_string()}),
                            );
                            break;
                        }
                    };
                    if let Err(e) = self.apply_payload(&payload, seq) {
                        self.mark_corruption(json!({"kind":"invalid_mutation","error":e.to_string(),"seq":seq.to_string()}));
                        break;
                    }
                    self.last_hash = digest;
                }
                previous = digest;
                expected_seq = expected_seq
                    .checked_add(1)
                    .ok_or_else(|| Error::new("CORRUPT_STORE", "Sequence overflow"))?;
                offset += frame_length as u64;
            }
            if self.read_only {
                break;
            }
            self.journal_path = path;
        }
        if !saw_base && expected_seq <= snapshot_base {
            self.mark_corruption(
                json!({"kind":"checkpoint_journal_missing","base_seq":snapshot_base.to_string()}),
            );
        }
        Ok(())
    }
    fn preserve_tail(&mut self, path: &Path, offset: u64) -> Result<()> {
        if self.requested_read_only {
            self.corruption = Some(
                json!({"kind":"incomplete_tail_unmodified","file":path.file_name().unwrap().to_string_lossy(),"valid_bytes":offset.to_string()}),
            );
            return Ok(());
        }
        let backup = self.path.join(format!(
            "backups/{}-{}.jnl",
            path.file_stem().unwrap().to_string_lossy(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::copy(path, &backup)?;
        OpenOptions::new().write(true).open(&backup)?.sync_all()?;
        sync_directory(backup.parent().unwrap())?;
        // Original bytes are durable in backup before truncating the incomplete tail.
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(offset)?;
        file.sync_all()?;
        self.corruption = Some(
            json!({"kind":"incomplete_tail_preserved","backup":backup.strip_prefix(&self.path).unwrap().to_string_lossy(),"valid_bytes":offset.to_string()}),
        );
        Ok(())
    }
    fn apply_payload(&mut self, payload: &Payload, seq: u64) -> Result<()> {
        if payload.format_version != 1
            || !valid_id(&payload.command_id, "command")
            || self
                .state
                .get("command_receipts", &payload.command_id)
                .is_some()
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Invalid or duplicate command in journal",
            ));
        }
        artex_core::validate_secret_parameters(&payload.method, &payload.normalized_params)?;
        let mut candidate = self.state.clone();
        candidate.commit_seq = seq;
        let mut tombstones = self.tombstones.clone();
        let mut seen = BTreeSet::new();
        for mutation in &payload.mutations {
            if mutation.collection == "command_receipts"
                || !seen.insert((mutation.collection.clone(), mutation.id.clone()))
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Duplicate/store-owned mutation",
                ));
            }
            let actual = candidate
                .get(&mutation.collection, &mutation.id)
                .map(|r| parse_u64(&r.header.version))
                .transpose()?
                .unwrap_or(0);
            let old = parse_u64(&mutation.expected_version)?;
            let new = parse_u64(&mutation.new_version)?;
            if old != actual || old.checked_add(1) != Some(new) {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Mutation version discontinuity",
                ));
            }
            match mutation.op.as_str() {
                "put" => {
                    let record = mutation
                        .record
                        .clone()
                        .ok_or_else(|| Error::new("CORRUPT_STORE", "Put is missing record"))?;
                    if record.header.collection != mutation.collection
                        || record.header.id != mutation.id
                        || record.header.version != mutation.new_version
                        || parse_u64(&record.header.commit_seq)? != seq
                        || tombstones
                            .contains_key(&(mutation.collection.clone(), mutation.id.clone()))
                    {
                        return Err(Error::new(
                            "CORRUPT_STORE",
                            "Put header mismatch or tombstone reuse",
                        ));
                    }
                    candidate
                        .collections
                        .entry(mutation.collection.clone())
                        .or_default()
                        .insert(mutation.id.clone(), record);
                }
                "delete" => {
                    if mutation.record.is_some() {
                        return Err(Error::new("CORRUPT_STORE", "Delete contains record"));
                    }
                    let old = candidate
                        .collections
                        .entry(mutation.collection.clone())
                        .or_default()
                        .remove(&mutation.id)
                        .ok_or_else(|| Error::new("CORRUPT_STORE", "Deleted entity missing"))?;
                    tombstones.insert(
                        (mutation.collection.clone(), mutation.id.clone()),
                        Tombstone {
                            collection: mutation.collection.clone(),
                            id: mutation.id.clone(),
                            version: new.to_string(),
                            commit_seq: seq.to_string(),
                            deleted_at: payload.committed_at.clone(),
                            legacy_key: old.header.legacy_key,
                        },
                    );
                }
                _ => return Err(Error::new("CORRUPT_STORE", "Unknown mutation operation")),
            }
        }
        for (required, records) in [
            ("domain_events", &payload.events),
            ("agent_deliveries", &payload.deliveries),
        ] {
            for record in records {
                if record.header.collection != required
                    || record.header.version != "1"
                    || parse_u64(&record.header.commit_seq)? != seq
                    || !seen.insert((required.into(), record.header.id.clone()))
                    || candidate.get(required, &record.header.id).is_some()
                    || tombstones.contains_key(&(required.into(), record.header.id.clone()))
                {
                    return Err(Error::new(
                        "CORRUPT_STORE",
                        "Event/delivery mismatch or duplicate",
                    ));
                }
                candidate
                    .collections
                    .entry(required.into())
                    .or_default()
                    .insert(record.header.id.clone(), record.clone());
            }
        }
        install_receipt(&mut candidate, payload, seq)?;
        validate_snapshot(&candidate)?;
        self.state = candidate;
        *self.read_cache.lock().unwrap() = None;
        self.tombstones = tombstones;
        Ok(())
    }
    /// Import a sealed native file with 64KiB working memory. The explicit file
    /// boundary is 8GiB; streaming growth past it is rejected before publication.
    pub fn blob_put_file(&mut self, source: &Path, media_type: &str) -> Result<Value> {
        self.writable()?;
        publish_blob_file(
            &self.path,
            source,
            media_type,
            self.fault_name(),
            &std::sync::atomic::AtomicBool::new(false),
        )
    }
    pub fn blob_put(&mut self, bytes: &[u8], media_type: &str) -> Result<Value> {
        self.writable()?;
        let sha256 = hex::encode(hash(bytes));
        let directory = self.path.join(format!("objects/blobs/{}", &sha256[..2]));
        reject_links(&directory)?;
        fs::create_dir_all(&directory)?;
        let destination = directory.join(&sha256);
        reject_links(&destination)?;
        if destination.exists() {
            let reference =
                json!({"sha256":sha256,"length":bytes.len().to_string(),"media_type":media_type});
            self.verify_blob_file(&reference, &mut File::open(&destination)?)?;
        } else {
            inject(self.fault_name().as_deref(), "blob_before_write")?;
            let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
            write_new_sync(&temporary, bytes)?;
            inject(self.fault_name().as_deref(), "blob_after_tmp_sync")?;
            artex_platform::install_new(&temporary, &destination)?;
            inject(self.fault_name().as_deref(), "blob_after_install")?;
            sync_directory(&directory)?;
            sync_directory(directory.parent().unwrap())?;
        }
        Ok(json!({"sha256":sha256,"length":bytes.len().to_string(),"media_type":media_type}))
    }
    pub fn blob_read(&self, reference: &Value, offset: u64, length: usize) -> Result<Vec<u8>> {
        if length > 65536 {
            return Err(Error::new("SIZE_LIMIT", "Blob read exceeds 64 KiB"));
        }
        let path = self.blob_path(reference)?;
        let mut file = File::open(path)?;
        let size = self.verify_blob_file(reference, &mut file)?;
        if offset > size {
            return Err(Error::new("INVALID_PARAMS", "Blob offset exceeds length"));
        }
        let read_size = (size - offset).min(length as u64) as usize;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; read_size];
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    /// Stream and verify one immutable blob with constant 64KiB memory. The
    /// caller publishes its output only after this integrity check succeeds.
    pub fn copy_blob_to(&self, reference: &Value, writer: &mut impl Write) -> Result<u64> {
        copy_blob_file(&self.path, reference, writer)
    }
    fn blob_path(&self, reference: &Value) -> Result<PathBuf> {
        blob_file_path(&self.path, reference)
    }
    fn verify_blob_file(&self, reference: &Value, file: &mut File) -> Result<u64> {
        let expected = parse_u64(required_string(reference, "length")?)?;
        let size = file.metadata()?.len();
        if expected != size {
            return Err(Error::new("CORRUPT_STORE", "Blob length mismatch"));
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0; 65536];
        file.seek(SeekFrom::Start(0))?;
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        if hex::encode(hasher.finalize()) != required_string(reference, "sha256")? {
            return Err(Error::new("CORRUPT_STORE", "Blob hash mismatch"));
        }
        Ok(size)
    }
    fn check_blobs(&self, value: &Value) -> Result<()> {
        match value {
            Value::Object(map) => {
                if map.len() == 3
                    && map.contains_key("sha256")
                    && map.contains_key("length")
                    && map.contains_key("media_type")
                {
                    let path = self.blob_path(value)?;
                    let mut file = File::open(path)?;
                    self.verify_blob_file(value, &mut file)?;
                }
                for child in map.values() {
                    self.check_blobs(child)?;
                }
            }
            Value::Array(array) => {
                for child in array {
                    self.check_blobs(child)?;
                }
            }
            _ => (),
        };
        Ok(())
    }
    fn fault_name(&self) -> Option<String> {
        #[cfg(test)]
        {
            self.fault.clone()
        }
        #[cfg(not(test))]
        {
            None
        }
    }
}
fn inject(point: Option<&str>, stage: &str) -> Result<()> {
    if point == Some(stage) {
        Err(Error::new(
            "IO_FAILURE",
            &format!("Injected failure at {stage}"),
        ))
    } else {
        Ok(())
    }
}
fn install_receipt(state: &mut Snapshot, payload: &Payload, seq: u64) -> Result<()> {
    let data = json!({"command_id":payload.command_id,"method":payload.method,"normalized_params":payload.normalized_params,"expected":payload.expected,
        "commit_seq":seq.to_string(),"result":payload.result,"result_ref":null});
    let record = Record {
        header: Header {
            id: payload.command_id.clone(),
            collection: "command_receipts".into(),
            version: "1".into(),
            commit_seq: seq.to_string(),
            created_at: payload.committed_at.clone(),
            updated_at: payload.committed_at.clone(),
            legacy_key: Value::Null,
        },
        data,
    };
    validate_record(&record)?;
    state
        .collections
        .entry("command_receipts".into())
        .or_default()
        .insert(payload.command_id.clone(), record);
    Ok(())
}
fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| Error::new("CORRUPT_STORE", &format!("Missing string: {key}")))
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub fn encode_frame(
    payload: &[u8],
    seq: u64,
    previous: [u8; 32],
    transaction: [u8; 16],
) -> Result<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::new("SIZE_LIMIT", "Payload too large"));
    }
    let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len() + 8);
    bytes.extend_from_slice(b"ARTEXJ01");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&seq.to_le_bytes());
    bytes.extend_from_slice(&transaction);
    bytes.extend_from_slice(&previous);
    bytes.extend_from_slice(&hash(payload));
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(FOOTER);
    Ok(bytes)
}
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>> {
    fn ordered(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), ordered(v))).collect();
                json!(sorted)
            }
            Value::Array(array) => json!(array.iter().map(ordered).collect::<Vec<_>>()),
            _ => value.clone(),
        }
    }
    Ok(serde_json::to_vec(&ordered(value))?)
}
fn lock_workspace(path: &Path) -> Result<File> {
    lock_workspace_mode(path, false)
}
fn lock_workspace_mode(path: &Path, read_only: bool) -> Result<File> {
    let lock_path = path.join("locks/writer.lock");
    reject_links(&lock_path)?;
    if read_only {
        // Acquire the same exclusive OS lock without creating directories or
        // replacing its diagnostic owner metadata. The lock must already exist.
        let file = OpenOptions::new().read(true).open(&lock_path)?;
        file.try_lock_exclusive()
            .map_err(|_| Error::new("BUSY", "Another writer holds the native workspace lock"))?;
        return Ok(file);
    }
    fs::create_dir_all(path.join("locks"))?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.try_lock_exclusive()
        .map_err(|_| Error::new("BUSY", "Another writer holds the native workspace lock"))?;
    file.set_len(0)?;
    file.write_all(&serde_json::to_vec(
        &json!({"pid":std::process::id(),"epoch":new_id("epoch"),"opened_at":now()}),
    )?)?;
    file.sync_all()?;
    Ok(file)
}
fn write_new_sync(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_links(path)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn atomic_new(path: &Path, bytes: &[u8]) -> Result<()> {
    reject_links(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "No parent directory"))?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4().simple()));
    write_new_sync(&temporary, bytes)?;
    if path.exists() {
        let _ = fs::remove_file(&temporary);
        return Err(Error::new("VERSION_CONFLICT", "Destination already exists"));
    }
    artex_platform::install_new(&temporary, path)?;
    sync_directory(parent)?;
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    artex_platform::sync_directory(path)?;
    Ok(())
}
fn reject_links(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        // A Windows verbatim prefix alone (\\?\C:) is not a complete path and
        // symlink_metadata returns ERROR_INVALID_FUNCTION. Inspect only rooted paths.
        if matches!(component, Component::Prefix(_)) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() && !trusted_system_storage_alias(&current, &meta)?
                {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Workspace storage path contains a symbolic link",
                    ));
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "Workspace storage path contains a reparse point",
                        ));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
/// macOS exposes its system temporary directories through root-owned aliases.
/// Permit only those exact aliases, without canonicalizing the caller's path:
/// every later component must still pass the ordinary symlink/reparse checks.
#[cfg(target_os = "macos")]
fn trusted_system_storage_alias(path: &Path, metadata: &fs::Metadata) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let target = match path.to_str() {
        Some("/var") => Path::new("/private/var"),
        Some("/tmp") => Path::new("/private/tmp"),
        _ => return Ok(false),
    };
    if !metadata.file_type().is_symlink() || metadata.uid() != 0 {
        return Ok(false);
    }
    let link = fs::read_link(path)?;
    let destination = if link.is_absolute() {
        link
    } else {
        Path::new("/").join(link)
    };
    if destination != target {
        return Ok(false);
    }
    // The alias and its /private parent must not be replaceable by ordinary
    // users. /private/tmp itself is intentionally a root-owned sticky temp dir.
    for parent in [Path::new("/"), Path::new("/private")] {
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Ok(false);
        }
    }
    let meta = fs::symlink_metadata(target)?;
    Ok(meta.is_dir()
        && meta.uid() == 0
        && (meta.mode() & 0o022 == 0
            || (target == Path::new("/private/tmp") && meta.mode() & 0o1000 != 0)))
}
#[cfg(not(target_os = "macos"))]
fn trusted_system_storage_alias(_: &Path, _: &fs::Metadata) -> Result<bool> {
    Ok(false)
}
fn safe_relative(path: &str) -> Result<()> {
    if path.contains('\\')
        || path.contains(':')
        || !Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(Error::new("CORRUPT_STORE", "Unsafe snapshot relative path"));
    }
    Ok(())
}
fn list_numbered(path: &Path, extension: &str) -> Result<Vec<(u64, PathBuf)>> {
    reject_links(path)?;
    let mut paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some(extension) {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| Error::new("CORRUPT_STORE", "Non-UTF8 store file name"))?;
        if stem.len() != 20 || !stem.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Invalid generation/segment file name",
            ));
        }
        let sequence = stem
            .parse::<u64>()
            .map_err(|_| Error::new("CORRUPT_STORE", "File sequence overflow"))?;
        paths.push((sequence, path));
    }
    paths.sort_by_key(|(s, _)| *s);
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Expected, default_data};
    fn batch(method: &str, params: Value, changes: Vec<Change>) -> CommandBatch {
        CommandBatch {
            command_id: new_id("command"),
            method: method.into(),
            normalized_params: params,
            expected: vec![],
            changes,
            events: vec![],
            deliveries: vec![],
            result: json!({"ok":true}),
        }
    }
    fn company(id: &str, name: &str) -> Change {
        let mut data = default_data("companies").unwrap();
        data["name"] = json!(name);
        data["nkey"] = json!(name);
        Change::Put {
            collection: "companies".into(),
            id: id.into(),
            data,
        }
    }
    fn with_cas(store: &Store, mut command: CommandBatch) -> CommandBatch {
        let changes = artex_core::expand_deletions(&store.snapshot(), &command.changes).unwrap();
        for change in changes {
            let (c, id) = change.key();
            if let Some(record) = store.snapshot().get(c, id) {
                if !command
                    .expected
                    .iter()
                    .any(|e| e.collection == c && e.id == id)
                {
                    command.expected.push(Expected {
                        collection: c.into(),
                        id: id.into(),
                        version: record.header.version.clone(),
                    });
                }
            }
        }
        command
    }
    #[test]
    fn frame_matches_frozen_vector() {
        let vector = artex_core::contracts::read("journal-vector.v1.json");
        let raw = hex::decode(vector["frame_hex"].as_str().unwrap()).unwrap();
        let frame = encode_frame(&raw[104..raw.len() - 8], 1, [0; 32], [0x22; 16]).unwrap();
        assert_eq!(raw, frame);
        assert_eq!(
            hex::encode(hash(&frame)),
            vector["frame_sha256"].as_str().unwrap()
        );
    }
    #[test]
    fn canonical_native_path_can_create_and_reopen_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("workspace");
        let store = Store::create(&path, "canonical").unwrap();
        assert!(!store.read_only());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.info()["name"], "canonical");
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_temp_aliases_support_durable_create_and_readonly_reopen() {
        for (alias, physical) in [("/var/tmp", "/private/var/tmp"), ("/tmp", "/private/tmp")] {
            let temp = tempfile::tempdir_in(physical).unwrap();
            let path = Path::new(alias)
                .join(temp.path().file_name().unwrap())
                .join("workspace");
            // Do not canonicalize this fixture: the system alias is the input
            // selected by users and also returned by macOS tempfile defaults.
            let mut store = Store::create(&path, "system alias").unwrap();
            let id = new_id("company");
            let command = batch(
                "company.create",
                json!({"name":"alias"}),
                vec![company(&id, "alias")],
            );
            let receipt = store.commit(command.clone()).unwrap();
            let reference = store
                .blob_put(b"verified through system alias", "text/plain")
                .unwrap();
            store.checkpoint().unwrap();
            drop(store);
            let mut store = Store::open(&path).unwrap();
            assert_eq!(store.commit(command).unwrap(), receipt);
            assert_eq!(
                store.blob_read(&reference, 0, 64).unwrap(),
                b"verified through system alias"
            );
            drop(store);
            let store = Store::open_read_only(&path).unwrap();
            assert!(store.read_only());
            assert_eq!(
                store.snapshot().get("companies", &id).unwrap().data["name"],
                "alias"
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn user_storage_symlinks_remain_rejected_at_ancestor_leaf_and_internal_file() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        let alias = temp.path().join("user-alias");
        symlink(&real, &alias).unwrap();
        assert_eq!(
            Store::create(&alias.join("workspace"), "denied")
                .err()
                .unwrap()
                .code,
            "PERMISSION_DENIED"
        );
        assert!(
            !real.join("workspace").exists(),
            "Reject before creating anything through a user alias"
        );
        let workspace = real.join("workspace");
        drop(Store::create(&workspace, "real").unwrap());
        let leaf = temp.path().join("workspace-link");
        symlink(&workspace, &leaf).unwrap();
        let ancestor_path = alias.join("workspace");
        for path in [&leaf, &ancestor_path] {
            assert_eq!(Store::open(path).err().unwrap().code, "PERMISSION_DENIED");
            assert_eq!(
                Store::open_read_only(path).err().unwrap().code,
                "PERMISSION_DENIED"
            );
        }
        let lock = workspace.join("locks/writer.lock");
        let outside = real.join("outside-writer.lock");
        fs::rename(&lock, &outside).unwrap();
        let original = fs::read(&outside).unwrap();
        symlink(&outside, &lock).unwrap();
        assert_eq!(
            Store::open(&workspace).err().unwrap().code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            Store::open_read_only(&workspace).err().unwrap().code,
            "PERMISSION_DENIED"
        );
        assert_eq!(fs::read(outside).unwrap(), original);
    }
    #[test]
    fn durable_dedup_precedes_cas_and_survives_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "테스트").unwrap();
        let id = new_id("company");
        let mut command = batch(
            "company.create",
            json!({"name":"테스트"}),
            vec![company(&id, "테스트")],
        );
        command.expected.push(Expected {
            collection: "companies".into(),
            id: id.clone(),
            version: "0".into(),
        });
        let receipt = store.commit(command.clone()).unwrap();
        assert_eq!(receipt.commit_seq, "1");
        assert_eq!(store.commit(command.clone()).unwrap(), receipt);
        let mut reuse = command.clone();
        reuse.normalized_params = json!({"name":"변경"});
        assert_eq!(store.commit(reuse).unwrap_err().code, "COMMAND_ID_REUSE");
        store.checkpoint().unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(!store.read_only());
        assert_eq!(store.commit(command).unwrap(), receipt);
        assert_eq!(
            store
                .snapshot()
                .get("companies", &id)
                .unwrap()
                .header
                .version,
            "1"
        );
        assert!(
            Store::open(&path).is_err(),
            "native lock must reject another writer"
        );
    }
    #[test]
    fn snapshot_is_immutable_cas_is_atomic_and_unique_enforced() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::create(&temp.path().join("workspace"), "test").unwrap();
        let id = new_id("company");
        store
            .commit(batch("create", json!({}), vec![company(&id, "a")]))
            .unwrap();
        let old = store.snapshot();
        let mut update = batch("update", json!({}), vec![company(&id, "b")]);
        update.expected.push(Expected {
            collection: "companies".into(),
            id: id.clone(),
            version: "1".into(),
        });
        store.commit(update.clone()).unwrap();
        update.command_id = new_id("command");
        assert_eq!(store.commit(update).unwrap_err().code, "VERSION_CONFLICT");
        assert_eq!(old.get("companies", &id).unwrap().data["name"], "a");
        let other = new_id("company");
        assert_eq!(
            store
                .commit(batch("duplicate", json!({}), vec![company(&other, "b")]))
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
        assert_eq!(store.snapshot().commit_seq, 2);
    }
    #[test]
    fn incomplete_tail_is_preserved_complete_corruption_is_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "test").unwrap();
        let id = new_id("company");
        store
            .commit(batch("create", json!({}), vec![company(&id, "a")]))
            .unwrap();
        let journal = store.journal_path.clone();
        drop(store);
        let original = fs::read(&journal).unwrap();
        let mut file = OpenOptions::new().append(true).open(&journal).unwrap();
        file.write_all(b"ARTEXJ").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let recovered = Store::open(&path).unwrap();
        assert!(!recovered.read_only());
        assert_eq!(recovered.snapshot().commit_seq, 1);
        assert_eq!(fs::read(&journal).unwrap(), original);
        assert_eq!(fs::read_dir(path.join("backups")).unwrap().count(), 1);
        drop(recovered);
        let mut damaged = original;
        damaged[104] ^= 1;
        fs::write(&journal, damaged).unwrap();
        let recovered = Store::open(&path).unwrap();
        assert!(recovered.read_only());
        assert_eq!(recovered.snapshot().commit_seq, 0);
        assert!(recovered.info()["recovery"]["corruption"].is_object());
    }
    #[test]
    fn damaged_latest_generation_recovers_from_older_generation_and_journal() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "test").unwrap();
        let id = new_id("company");
        store
            .commit(batch("create", json!({}), vec![company(&id, "a")]))
            .unwrap();
        store.checkpoint().unwrap();
        let update = with_cas(&store, batch("update", json!({}), vec![company(&id, "b")]));
        store.commit(update).unwrap();
        let checkpoint = store.checkpoint().unwrap();
        drop(store);
        let generation = parse_u64(checkpoint["generation"].as_str().unwrap()).unwrap();
        fs::write(
            path.join(format!("store/snapshots/{generation:020}/companies.ndjson")),
            b"damaged\n",
        )
        .unwrap();
        let store = Store::open(&path).unwrap();
        assert!(!store.read_only());
        assert_eq!(store.snapshot().commit_seq, 2);
        assert_eq!(
            store.snapshot().get("companies", &id).unwrap().data["name"],
            "b"
        );
    }
    #[test]
    fn requested_read_only_replays_all_segments_and_preserves_owner_and_incomplete_tail_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let mut store = Store::create(&path, "Physical read-only").unwrap();
        let first = new_id("company");
        let second = new_id("company");
        store
            .commit(batch("first", json!({}), vec![company(&first, "one")]))
            .unwrap();
        store.rotate_journal().unwrap();
        store
            .commit(batch("second", json!({}), vec![company(&second, "two")]))
            .unwrap();
        let journal = store.journal_path.clone();
        drop(store);
        let mut file = OpenOptions::new().append(true).open(&journal).unwrap();
        file.write_all(b"ARTEXJ").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let before = fs::read(&journal).unwrap();
        let owner = fs::read(path.join("locks/writer.lock")).unwrap();
        let manifests = fs::read_dir(path.join("store/manifests")).unwrap().count();
        let mut read = Store::open_read_only(&path).unwrap();
        assert!(read.read_only());
        assert_eq!(read.snapshot().commit_seq, 2);
        assert!(read.snapshot().get("companies", &first).is_some());
        assert!(read.snapshot().get("companies", &second).is_some());
        assert_eq!(
            read.recovery()["corruption"]["kind"],
            "incomplete_tail_unmodified"
        );
        assert!(read.checkpoint().is_err());
        assert!(read.blob_put(b"forbidden", "text/plain").is_err());
        assert!(Store::open(&path).is_err());
        drop(read);
        assert_eq!(fs::read(&journal).unwrap(), before);
        assert_eq!(fs::read(path.join("locks/writer.lock")).unwrap(), owner);
        assert_eq!(
            fs::read_dir(path.join("store/manifests")).unwrap().count(),
            manifests
        );
        assert_eq!(fs::read_dir(path.join("backups")).unwrap().count(), 0);
        let write = Store::open(&path).unwrap();
        assert!(!write.read_only());
        assert_eq!(write.snapshot().commit_seq, 2);
        assert_eq!(fs::read_dir(path.join("backups")).unwrap().count(), 1);
        assert_eq!(fs::read(&journal).unwrap(), before[..before.len() - 6]);
    }
    #[test]
    fn reader_drain_confirms_os_lock_release_before_writer_retirement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let store = Store::create(&path, "Pinned native reader").unwrap();
        let view = store.read_view();
        assert_eq!(store.active_read_views_count(), 1);
        assert_eq!(
            store
                .wait_read_views(std::time::Duration::ZERO)
                .unwrap_err()
                .code,
            "RECOVERY_REQUIRED"
        );
        let worker = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            drop(view);
        });
        store
            .wait_read_views(std::time::Duration::from_secs(2))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(store.active_read_views_count(), 0);
        drop(store);
        Store::open(&path).unwrap();
    }
    #[test]
    fn damaged_latest_generation_with_missing_chain_does_not_silently_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "test").unwrap();
        let id = new_id("company");
        store
            .commit(batch("create", json!({}), vec![company(&id, "a")]))
            .unwrap();
        let checkpoint = store.checkpoint().unwrap();
        drop(store);
        let generation = parse_u64(checkpoint["generation"].as_str().unwrap()).unwrap();
        fs::write(
            path.join(format!("store/snapshots/{generation:020}/companies.ndjson")),
            b"damaged\n",
        )
        .unwrap();
        fs::remove_file(path.join("store/journal/00000000000000000001.jnl")).unwrap();
        let store = Store::open(&path).unwrap();
        assert!(store.read_only());
        assert_eq!(store.recovery()["lost_range"], json!({"from":"1","to":"1"}));
    }
    #[test]
    fn cascades_set_null_and_tombstones_are_persistent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "test").unwrap();
        let id = new_id("company");
        let scope_id = new_id("companyscope");
        let asset_id = new_id("asset");
        let mut scope = default_data("company_scope").unwrap();
        scope["company_id"] = json!(id);
        scope["kind"] = json!("domain");
        scope["domain"] = json!("example.test");
        let mut asset = default_data("assets").unwrap();
        asset["type"] = json!("root_domain");
        asset["domain"] = json!("example.test");
        asset["company_id"] = json!(id);
        store
            .commit(batch(
                "seed",
                json!({}),
                vec![
                    company(&id, "a"),
                    Change::Put {
                        collection: "company_scope".into(),
                        id: scope_id.clone(),
                        data: scope,
                    },
                    Change::Put {
                        collection: "assets".into(),
                        id: asset_id.clone(),
                        data: asset,
                    },
                ],
            ))
            .unwrap();
        let deletion = with_cas(
            &store,
            batch(
                "delete",
                json!({}),
                vec![Change::Delete {
                    collection: "companies".into(),
                    id: id.clone(),
                }],
            ),
        );
        store.commit(deletion).unwrap();
        assert!(store.snapshot().get("company_scope", &scope_id).is_none());
        assert!(store.snapshot().get("assets", &asset_id).unwrap().data["company_id"].is_null());
        store.checkpoint().unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(!store.read_only());
        assert_eq!(
            store
                .commit(batch("reuse", json!({}), vec![company(&id, "c")]))
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
    }
    #[test]
    fn blob_corruption_forces_read_only_and_no_traversal() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "test").unwrap();
        let reference = store
            .blob_put(b"evidence", "application/octet-stream")
            .unwrap();
        assert_eq!(store.blob_read(&reference, 2, 3).unwrap(), b"ide");
        assert!(store.blob_read(&reference, 0, 65537).is_err());
        assert!(
            store
                .blob_read(
                    &json!({"sha256":"../bad","length":"0","media_type":"x"}),
                    0,
                    1
                )
                .is_err()
        );
        let id = new_id("company");
        let mut command = batch("create", json!({}), vec![company(&id, "a")]);
        command.result = json!({"evidence":reference});
        store.commit(command).unwrap();
        let blob = store.blob_path(&reference).unwrap();
        drop(store);
        fs::write(blob, b"broken").unwrap();
        assert!(Store::open(&path).unwrap().read_only());
    }
    #[test]
    fn workspace_results_match_contract() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::create(&temp.path().join("workspace"), "test").unwrap();
        let rpc = artex_core::contracts::read("rpc.v1.schema.json");
        artex_core::validate_contract(&rpc["results"]["workspace.info"], &rpc, &store.info())
            .unwrap();
        artex_core::validate_contract(
            &rpc["results"]["workspace.recover"],
            &rpc,
            &store.recovery(),
        )
        .unwrap();
    }
    #[test]
    fn existing_objects_require_explicit_cas_including_delete_dependents() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::create(&temp.path().join("workspace"), "test").unwrap();
        let id = new_id("company");
        store
            .commit(batch("create", json!({}), vec![company(&id, "a")]))
            .unwrap();
        assert_eq!(
            store
                .commit(batch(
                    "unversioned_update",
                    json!({}),
                    vec![company(&id, "b")]
                ))
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
        assert_eq!(store.snapshot().commit_seq, 1);
    }
    #[test]
    fn http_asset_unique_key_uses_service_type_and_sql_null_semantics() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::create(&temp.path().join("workspace"), "test").unwrap();
        let http = |url: &str, name: &str| {
            let mut data = default_data("assets").unwrap();
            data["type"] = json!("service");
            data["service_type"] = json!("http");
            data["service_name"] = json!(name);
            data["url"] = json!(url);
            data["port"] = json!(443);
            Change::Put {
                collection: "assets".into(),
                id: new_id("asset"),
                data,
            }
        };
        store
            .commit(batch(
                "first",
                json!({}),
                vec![http("https://example.test", "apache")],
            ))
            .unwrap();
        assert_eq!(
            store
                .commit(batch(
                    "duplicate",
                    json!({}),
                    vec![http("https://example.test", "nginx")]
                ))
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
        store
            .commit(batch(
                "second",
                json!({}),
                vec![http("https://other.test", "apache")],
            ))
            .unwrap();
        // SQL UNIQUE does not merge legacy NULL natural-key components.
        let nullable = || {
            let mut data = default_data("assets").unwrap();
            data["type"] = json!("ip");
            Change::Put {
                collection: "assets".into(),
                id: new_id("asset"),
                data,
            }
        };
        store
            .commit(batch(
                "legacy_nulls",
                json!({}),
                vec![nullable(), nullable()],
            ))
            .unwrap();
    }
    #[test]
    fn commit_fault_boundaries_preserve_confirmed_changes_and_recover_receipts() {
        for point in [
            "before_append",
            "after_header",
            "after_payload",
            "after_footer",
            "after_sync",
            "after_memory",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("workspace");
            let mut store = Store::create(&path, "test").unwrap();
            let prior = new_id("company");
            store
                .commit(batch(
                    "prior",
                    json!({}),
                    vec![company(&prior, "confirmed")],
                ))
                .unwrap();
            let next = new_id("company");
            let command = batch(
                "interrupted",
                json!({}),
                vec![company(&next, "unconfirmed")],
            );
            store.fault = Some(point.into());
            assert_eq!(
                store.commit(command.clone()).unwrap_err().code,
                "IO_FAILURE"
            );
            assert!(store.read_only());
            drop(store);
            let mut store = Store::open(&path).unwrap();
            assert!(!store.read_only(), "{point}");
            assert!(
                store.snapshot().get("companies", &prior).is_some(),
                "{point}"
            );
            if ["after_footer", "after_sync", "after_memory"].contains(&point) {
                assert_eq!(store.snapshot().commit_seq, 2, "{point}");
                let receipt = store.commit(command).unwrap();
                assert_eq!(receipt.commit_seq, "2");
                assert_eq!(store.snapshot().commit_seq, 2);
            } else {
                assert_eq!(store.snapshot().commit_seq, 1, "{point}");
                assert!(store.snapshot().get("companies", &next).is_none());
            }
        }
    }
    #[test]
    fn incomplete_checkpoint_never_replaces_valid_generations() {
        for point in ["checkpoint_after_files", "checkpoint_before_install"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("workspace");
            let mut store = Store::create(&path, "test").unwrap();
            let id = new_id("company");
            store
                .commit(batch("create", json!({}), vec![company(&id, "a")]))
                .unwrap();
            store.fault = Some(point.into());
            assert!(store.checkpoint().is_err());
            drop(store);
            let mut store = Store::open(&path).unwrap();
            assert!(!store.read_only());
            assert_eq!(store.snapshot().commit_seq, 1);
            store.checkpoint().unwrap();
            assert!(store.valid_generations.len() >= 3);
        }
    }
    #[test]
    fn blob_install_failures_leave_no_committed_reference_and_are_retryable() {
        for point in [
            "blob_before_write",
            "blob_after_tmp_sync",
            "blob_after_install",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("workspace");
            let mut store = Store::create(&path, "test").unwrap();
            store.fault = Some(point.into());
            assert_eq!(
                store
                    .blob_put(b"unreferenced evidence", "application/octet-stream")
                    .unwrap_err()
                    .code,
                "IO_FAILURE"
            );
            assert_eq!(store.snapshot().commit_seq, 0);
            drop(store);
            let mut store = Store::open(&path).unwrap();
            assert!(!store.read_only());
            let reference = store
                .blob_put(b"unreferenced evidence", "application/octet-stream")
                .unwrap();
            assert_eq!(
                store.blob_read(&reference, 0, 65536).unwrap(),
                b"unreferenced evidence"
            );
            assert_eq!(store.snapshot().commit_seq, 0);
        }
    }
    #[test]
    fn credentials_never_reach_journal_even_when_record_has_reference() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::create(&temp.path().join("workspace"), "test").unwrap();
        let id = new_id("asset");
        let secret = json!({"secret_ref":new_id("secret"),"hint":"password"});
        let mut data = default_data("assets").unwrap();
        data["type"] = json!("root_domain");
        data["domain"] = json!("example.test");
        data["auth"] = json!([{"username":"admin","password":secret}]);
        let command = batch(
            "asset.upsert",
            json!({"auth":[{"username":"admin","password":"SECRET_PROBE_7"}]}),
            vec![Change::Put {
                collection: "assets".into(),
                id,
                data,
            }],
        );
        assert_eq!(store.commit(command).unwrap_err().code, "PERMISSION_DENIED");
        assert_eq!(store.snapshot().commit_seq, 0);
        assert!(fs::read(&store.journal_path).unwrap().is_empty());
    }
    #[test]
    fn all_54_original_collections_571_fields_round_trip_checkpoint_and_journal() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "모든 자료형").unwrap();
        let mut seed: BTreeMap<String, (String, Value)> = BTreeMap::new();
        for table in artex_core::data_model()["tables"].as_array().unwrap() {
            let collection = table["name"].as_str().unwrap();
            let id = new_id(collection_prefix(collection).unwrap());
            let mut data = default_data(collection).unwrap();
            // Exercise the exact original SQL shape, including old conversations
            // without the optional native trash extension.
            if collection == "conversations" {
                for field in ["trashed_at", "trash_generation"] {
                    assert_eq!(
                        data.as_object_mut().unwrap().remove(field),
                        Some(Value::Null)
                    );
                }
            }
            for column in table["columns"].as_array().unwrap() {
                let field = column["name"].as_str().unwrap();
                if field == "id"
                    && column["desktop_schema"]["pattern"]
                        == format!(
                            "^{}_[0-9a-f]{{32}}$",
                            collection_prefix(collection).unwrap()
                        )
                {
                    data[field] = json!(id);
                }
                if !data[field].is_null() && column["desktop_type"] == "decimal_string" {
                    data[field] = json!("9223372036854775807");
                }
            }
            seed.insert(collection.into(), (id, data));
        }
        // Natural/opaque primary keys are deliberately distinct from typed headers.
        seed.get_mut("side_question_sessions").unwrap().1["session_key"] =
            json!("legacy/session/key");
        seed.get_mut("exchanges").unwrap().1["id"] = json!("legacy-capture-id");
        seed.get_mut("companies").unwrap().1["name"] = json!("회사 A");
        seed.get_mut("companies").unwrap().1["nkey"] = json!("회사 a");
        seed.get_mut("company_scope").unwrap().1["kind"] = json!("domain");
        seed.get_mut("company_scope").unwrap().1["domain"] = json!("example.test");
        seed.get_mut("assets").unwrap().1["type"] = json!("root_domain");
        seed.get_mut("assets").unwrap().1["domain"] = json!("example.test");
        seed.get_mut("exploration_nodes").unwrap().1["kind"] = json!("begin");
        seed.get_mut("exploration_nodes").unwrap().1["state"] = json!("open");
        seed.get_mut("exploration_nodes").unwrap().1["payload"] =
            json!({"한국어":"확장 JSON","nested":[null,true,{"opaque":9223372036854775807i64}]});
        seed.get_mut("tasks").unwrap().1["description"] = json!("작업 설명");
        seed.get_mut("tasks").unwrap().1["goal"] = json!("실행 목적");
        seed.get_mut("findings").unwrap().1["severity"] = json!("high");
        seed.get_mut("task_scope").unwrap().1["kind"] = json!("keyword");
        seed.get_mut("task_scope").unwrap().1["value"] = json!("Artex");
        seed.get_mut("llm_profiles").unwrap().1["api_key"] =
            json!({"secret_ref":new_id("secret"),"hint":"프로필 키"});
        seed.get_mut("assets").unwrap().1["auth"] = json!([]);
        seed.get_mut("agents").unwrap().1["key"] = json!("all54_agent");
        seed.get_mut("llm_records").unwrap().1["task_id"] = json!("opaque/registry/key");
        seed.get_mut("llm_usage").unwrap().1["task_id"] = json!("opaque/registry/key");
        for fk in artex_core::foreign_keys() {
            let target = &seed[&fk.target].1;
            let values: Vec<_> = fk
                .target_columns
                .iter()
                .map(|c| target[c].clone())
                .collect();
            let record = &mut seed.get_mut(&fk.collection).unwrap().1;
            for (field, value) in fk.columns.iter().zip(values) {
                if !record[field].is_null() {
                    record[field] = value;
                }
            }
        }
        seed.get_mut("agent_visibility").unwrap().1["resource_id"] = json!(seed["mcp_servers"].0);
        // Typed provenance without SQL FK still has a syntactic namespace contract.
        for table in artex_core::data_model()["tables"].as_array().unwrap() {
            let collection = table["name"].as_str().unwrap();
            for column in table["columns"].as_array().unwrap() {
                let field = column["name"].as_str().unwrap();
                if seed[collection].1[field].is_null() {
                    continue;
                }
                if let Some(prefix) = column["desktop_schema"]["pattern"]
                    .as_str()
                    .and_then(|p| p.strip_prefix('^'))
                    .and_then(|p| p.strip_suffix("_[0-9a-f]{32}$"))
                {
                    let value = if field == "id" {
                        seed[collection].0.clone()
                    } else {
                        seed.iter()
                            .find(|(c, _)| collection_prefix(c).unwrap() == prefix)
                            .map(|(_, v)| v.0.clone())
                            .unwrap_or_else(|| new_id(prefix))
                    };
                    seed.get_mut(collection).unwrap().1[field] = json!(value);
                }
            }
        }
        seed.get_mut("traffic_evidence_snapshots").unwrap().1["id"] =
            json!(seed["traffic_evidence_snapshots"].0);
        seed.get_mut("finding_traffic_bindings").unwrap().1["snapshot_id"] =
            json!(seed["traffic_evidence_snapshots"].0);
        let source_exp = new_id("exploration");
        let mut source_exp_data = seed["explorations"].1.clone();
        source_exp_data["id"] = json!(source_exp);
        let source_task = new_id("task");
        let mut source_task_data = seed["tasks"].1.clone();
        source_task_data["id"] = json!(source_task);
        source_task_data["exploration_id"] = json!(source_exp);
        seed.get_mut("task_relations").unwrap().1["source_task_id"] = json!(source_task);
        let second_node = new_id("node");
        let mut second_node_data = seed["exploration_nodes"].1.clone();
        second_node_data["id"] = json!(second_node);
        second_node_data["kind"] = json!("fact");
        second_node_data["state"] = json!("origin");
        seed.get_mut("exploration_edges").unwrap().1["dst_id"] = json!(second_node);
        let conversation = seed["conversations"].0.clone();
        let side = &mut seed.get_mut("side_question_sessions").unwrap().1;
        side["conversation_id"] = json!(conversation);
        side["task_id"] = Value::Null;
        side["exploration_id"] = Value::Null;
        side["intent_id"] = Value::Null;
        let mut changes: Vec<_> = seed
            .iter()
            .map(|(collection, (id, data))| Change::Put {
                collection: collection.clone(),
                id: id.clone(),
                data: data.clone(),
            })
            .collect();
        changes.extend([
            Change::Put {
                collection: "explorations".into(),
                id: source_exp,
                data: source_exp_data,
            },
            Change::Put {
                collection: "tasks".into(),
                id: source_task,
                data: source_task_data,
            },
            Change::Put {
                collection: "exploration_nodes".into(),
                id: second_node,
                data: second_node_data,
            },
        ]);
        let command = batch("all54.fixture", json!({"fields":571}), changes);
        store.commit(command).unwrap();
        let expected = store.snapshot();
        let mut fields = 0;
        for (collection, (id, _)) in &seed {
            let record = expected.get(collection, id).unwrap();
            artex_core::models::typed_roundtrip(record).unwrap();
            fields += record.data.as_object().unwrap().len();
        }
        assert_eq!(fields, 571);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(!store.read_only());
        assert_eq!(store.snapshot(), expected, "journal round trip");
        store.checkpoint().unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(!store.read_only());
        // Empty canonical collections are represented explicitly by checkpoints.
        for (collection, records) in &expected.collections {
            assert_eq!(
                store.snapshot().collections[collection],
                *records,
                "{collection}"
            );
        }
    }
}

fn write_checkpoint(
    workspace_path: &Path,
    state: &Snapshot,
    tombstones: &BTreeMap<(String, String), Tombstone>,
    last_hash: [u8; 32],
    generation: u64,
    fault: Option<String>,
) -> Result<Value> {
    let relative = format!("store/snapshots/{generation:020}");
    let directory = workspace_path.join(&relative);
    fs::create_dir(&directory)?;
    let mut files = Vec::new();
    // Include every canonical collection, even empty collections, in every snapshot.
    for collection in artex_core::registry()["collections"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] != "read_projection")
    {
        let name = collection["collection"].as_str().unwrap();
        let records = state.records(name);
        let path = format!("{relative}/{name}.ndjson");
        let (sha, length) = write_ndjson_sync(&workspace_path.join(&path), records.iter())?;
        files.push(json!({"path":path,"sha256":sha,"length":length.to_string(),"record_count":records.len().to_string()}));
    }
    let path = format!("{relative}/_tombstones.ndjson");
    let (sha, length) = write_ndjson_sync(&workspace_path.join(&path), tombstones.values())?;
    files.push(json!({"path":path,"sha256":sha,"length":length.to_string(),"record_count":tombstones.len().to_string()}));
    files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    sync_directory(&directory)?;
    sync_directory(directory.parent().unwrap())?;
    inject(fault.as_deref(), "checkpoint_after_files")?;
    let mut manifest = json!({"format":"artex-checkpoint","format_version":1,"workspace_id":state.workspace_id,"generation":generation.to_string(),
            "base_seq":state.commit_seq.to_string(),"base_frame_sha256":hex::encode(last_hash),"created_at":now(),"files":files,
            "journal_start_seq":state.commit_seq.checked_add(1).ok_or_else(||Error::new("SIZE_LIMIT","Sequence exhausted"))?.to_string(),
            "command_receipt_count":state.records("command_receipts").len().to_string()});
    let manifest_hash = hex::encode(hash(&canonical_bytes(&manifest)?));
    manifest["manifest_sha256"] = json!(manifest_hash);
    inject(fault.as_deref(), "checkpoint_before_install")?;
    atomic_new(
        &workspace_path.join(format!("store/manifests/{generation:020}.json")),
        &canonical_bytes(&manifest)?,
    )?;

    Ok(
        json!({"generation":generation.to_string(),"base_seq":state.commit_seq.to_string(),"manifest_sha256":manifest_hash}),
    )
}

fn write_ndjson_sync<T: Serialize>(
    path: &Path,
    records: impl Iterator<Item = T>,
) -> Result<(String, u64)> {
    struct HashedWriter {
        file: File,
        hash: Sha256,
        length: u64,
    }
    impl Write for HashedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let n = self.file.write(bytes)?;
            self.hash.update(&bytes[..n]);
            self.length += n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let mut writer = HashedWriter {
        file: OpenOptions::new().create_new(true).write(true).open(path)?,
        hash: Sha256::new(),
        length: 0,
    };
    for record in records {
        serde_json::to_writer(&mut writer, &record)?;
        writer.write_all(b"\n")?;
    }
    writer.file.sync_all()?;
    Ok((hex::encode(writer.hash.finalize()), writer.length))
}

fn copy_blob_file(root: &Path, reference: &Value, writer: &mut impl Write) -> Result<u64> {
    let mut file = File::open(blob_file_path(root, reference)?)?;
    let expected = parse_u64(required_string(reference, "length")?)?;
    let mut digest = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Blob length overflow"))?;
        if total > expected {
            return Err(Error::new("CORRUPT_STORE", "Blob exceeds reference length"));
        }
        digest.update(&buffer[..n]);
        writer.write_all(&buffer[..n])?;
    }
    if total != expected || hex::encode(digest.finalize()) != reference["sha256"] {
        return Err(Error::new(
            "CORRUPT_STORE",
            "Blob integrity mismatch during copy",
        ));
    }
    Ok(total)
}

fn blob_file_path(root: &Path, reference: &Value) -> Result<PathBuf> {
    let digest = required_string(reference, "sha256")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::new("INVALID_PARAMS", "Invalid blob hash"));
    }
    parse_u64(required_string(reference, "length")?)?;
    let path = root.join(format!("objects/blobs/{}/{digest}", &digest[..2]));
    reject_links(&path)?;
    Ok(path)
}

fn existing_blob_reference(root: &Path, digest: &str, media_type: &str) -> Result<Option<Value>> {
    let probe = json!({"sha256":digest,"length":"0"});
    let path = blob_file_path(root, &probe)?;
    if !path.exists() {
        return Ok(None);
    }
    let reference = json!({"sha256":digest,"length":File::open(&path)?.metadata()?.len().to_string(),"media_type":media_type});
    copy_blob_file(root, &reference, &mut std::io::sink())?;
    Ok(Some(reference))
}

fn publish_blob_file(
    root: &Path,
    source: &Path,
    media_type: &str,
    fault: Option<String>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Value> {
    const MAX_FILE: u64 = 8 * 1024 * 1024 * 1024;
    reject_links(source)?;
    let mut input = File::open(source)?;
    let initial = input.metadata()?;
    if !initial.is_file() || initial.len() > MAX_FILE {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Blob file must be regular and at most 8GiB",
        ));
    }
    let temporary = root.join("tmp").join(format!("{}.blob", new_id("command")));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        inject(fault.as_deref(), "blob_before_write")?;
        let mut total = 0u64;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(Error::new("CANCELLED", "Blob publication cancelled"));
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Blob file length overflow"))?;
            if total > MAX_FILE {
                return Err(Error::new("SIZE_LIMIT", "Blob file exceeds 8GiB"));
            }
            digest.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
        }
        if total != initial.len() || input.metadata()?.len() != initial.len() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Sealed blob file changed while reading",
            ));
        }
        output.sync_all()?;
        drop(output);
        inject(fault.as_deref(), "blob_after_tmp_sync")?;
        let sha256 = hex::encode(digest.finalize());
        let reference = json!({"sha256":sha256,"length":total.to_string(),"media_type":media_type});
        let directory = root.join(format!("objects/blobs/{}", &sha256[..2]));
        reject_links(&directory)?;
        fs::create_dir_all(&directory)?;
        let destination = directory.join(&sha256);
        reject_links(&destination)?;
        if destination.exists() {
            copy_blob_file(root, &reference, &mut std::io::sink())?;
            fs::remove_file(&temporary)?;
        } else {
            if let Err(error) = artex_platform::install_new(&temporary, &destination) {
                if !destination.exists() {
                    return Err(error.into());
                }
                copy_blob_file(root, &reference, &mut std::io::sink())?;
                let _ = fs::remove_file(&temporary);
            }
            inject(fault.as_deref(), "blob_after_install")?;
            sync_directory(&directory)?;
            sync_directory(directory.parent().unwrap())?;
        }
        Ok(reference)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
