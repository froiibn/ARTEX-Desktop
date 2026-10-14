use crate::{LegacyExport, sources::traffic_archive};
use artex_core::{Error, Result, data_model};
use artex_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
fn archive_error(message: &str) -> Error {
    Error::new("PARSE_ERROR", message)
}
fn entry_name(name: &str) -> Result<String> {
    if name.is_empty()
        || name.contains('\\')
        || name.contains(':')
        || name.starts_with('/')
        || name
            .split('/')
            .any(|p| p == ".." || p == "." || p.is_empty())
    {
        return Err(archive_error("Unsafe archive entry path"));
    }
    Ok(name.into())
}
fn add_entry(
    files: &mut BTreeMap<String, Vec<u8>>,
    names: &mut BTreeSet<String>,
    name: &str,
    mut reader: impl Read,
    size: u64,
    total: &mut u64,
) -> Result<()> {
    let name = entry_name(name)?;
    if !names.insert(name.to_lowercase()) {
        return Err(archive_error("Duplicate archive entry"));
    }
    *total = total
        .checked_add(size)
        .ok_or_else(|| Error::new("SIZE_LIMIT", "Archive size overflow"))?;
    if *total > MAX_ARCHIVE_BYTES || files.len() >= 100000 {
        return Err(Error::new("SIZE_LIMIT", "Archive exceeds import boundary"));
    }
    let mut bytes = Vec::new();
    reader.by_ref().take(size + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(archive_error("Archive entry size mismatch"));
    }
    files.insert(name, bytes);
    Ok(())
}

/// Accepts the original Go tar.zst versions 1..3 and portable artex-task/1 ZIP.
/// Entries are never extracted into the filesystem.
pub fn read_archive(path: &Path, source_instance: &str) -> Result<LegacyExport> {
    crate::sources::checked_path(path)?;
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || meta.len() > MAX_ARCHIVE_BYTES {
        return Err(Error::new("SIZE_LIMIT", "Archive file boundary exceeded"));
    }
    let mut signature = [0; 4];
    let mut file = fs::File::open(path)?;
    file.read_exact(&mut signature)?;
    let mut files = BTreeMap::new();
    let mut names = BTreeSet::new();
    let mut total = 0;
    if signature[..2] == *b"PK" {
        let mut archive = zip::ZipArchive::new(fs::File::open(path)?)
            .map_err(|_| archive_error("Invalid ZIP archive"))?;
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|_| archive_error("Invalid ZIP entry"))?;
            if entry.is_dir() {
                continue;
            }
            if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
                return Err(archive_error("Archive links are prohibited"));
            }
            let name = entry.name().to_owned();
            let size = entry.size();
            add_entry(&mut files, &mut names, &name, entry, size, &mut total)?;
        }
    } else {
        let decoder = zstd::stream::read::Decoder::new(fs::File::open(path)?)
            .map_err(|_| archive_error("Invalid Zstandard archive"))?;
        let mut archive = tar::Archive::new(decoder);
        for entry in archive
            .entries()
            .map_err(|_| archive_error("Invalid TAR archive"))?
        {
            let entry = entry.map_err(|_| archive_error("Invalid TAR entry"))?;
            if entry.header().entry_type().is_dir() {
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(archive_error(
                    "Archive links and special entries are prohibited",
                ));
            }
            let name = entry
                .path()
                .map_err(|_| archive_error("Invalid TAR path"))?
                .to_str()
                .ok_or_else(|| archive_error("Archive path is not UTF-8"))?
                .to_owned();
            let size = entry.size();
            add_entry(&mut files, &mut names, &name, entry, size, &mut total)?;
        }
    }
    let manifest: Value = serde_json::from_slice(
        files
            .get("manifest.json")
            .ok_or_else(|| archive_error("Archive manifest missing"))?,
    )?;
    if manifest["format"] == "artex-task/1" {
        return portable_import(&manifest, &files, source_instance);
    }
    let version = manifest["format_version"]
        .as_u64()
        .ok_or_else(|| archive_error("Archive format version missing"))?;
    if !(1..=3).contains(&version) {
        return Err(Error::new(
            "VERSION_MISMATCH",
            "Legacy archive version unsupported",
        ));
    }
    let mut out = LegacyExport {
        source_instance: source_instance.into(),
        format_version: version as u32,
        ..Default::default()
    };
    if let Some(tables) = manifest["tables"].as_object() {
        for (name, rows) in tables {
            let rows = if let Some(raw) = rows.as_str() {
                serde_json::from_str::<Value>(raw)?
            } else {
                rows.clone()
            };
            out.tables.insert(
                name.clone(),
                rows.as_array()
                    .ok_or_else(|| archive_error("Archive table is not an array"))?
                    .clone(),
            );
        }
    }
    if let Some(tables) = manifest["streamed_tables"].as_object() {
        for (name, path) in tables {
            if out.tables.contains_key(name) {
                return Err(archive_error("Duplicate manifest table"));
            }
            let path = entry_name(
                path.as_str()
                    .ok_or_else(|| archive_error("Invalid streamed table path"))?,
            )?;
            out.tables.insert(
                name.clone(),
                ndjson(
                    files
                        .get(&path)
                        .ok_or_else(|| archive_error("Streamed table missing"))?,
                )?,
            );
        }
    }
    if let Some(counts) = manifest["data_counts"].as_object() {
        for (name, count) in counts {
            if out
                .tables
                .get(name)
                .is_some_and(|rows| Some(rows.len() as u64) != count.as_u64())
            {
                return Err(archive_error("Archive table count mismatch"));
            }
        }
    }
    if let Some(bytes) = files.get("traffic/traffic.json") {
        traffic_archive(&serde_json::from_slice(bytes)?, &files, &mut out)?;
    }
    Ok(out)
}
fn ndjson(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        rows.push(serde_json::from_slice(line)?);
    }
    Ok(rows)
}
fn portable_import(
    manifest: &Value,
    files: &BTreeMap<String, Vec<u8>>,
    source: &str,
) -> Result<LegacyExport> {
    let mut out = LegacyExport {
        source_instance: source.into(),
        format_version: 1,
        ..Default::default()
    };
    for entry in manifest["tables"]
        .as_array()
        .ok_or_else(|| archive_error("Portable table manifest missing"))?
    {
        let name = entry["collection"]
            .as_str()
            .ok_or_else(|| archive_error("Portable collection missing"))?;
        if !data_model()["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == name)
        {
            return Err(archive_error("Portable collection unsupported"));
        }
        let path = entry_name(
            entry["path"]
                .as_str()
                .ok_or_else(|| archive_error("Portable table path missing"))?,
        )?;
        let bytes = files
            .get(&path)
            .ok_or_else(|| archive_error("Portable table missing"))?;
        if entry["sha256"] != hex::encode(Sha256::digest(bytes)) {
            return Err(archive_error("Portable table checksum mismatch"));
        }
        let mut rows = Vec::new();
        for record in ndjson(bytes)? {
            artex_core::validate_record(&serde_json::from_value(record.clone())?)?;
            let mut data = record["data"].clone();
            let table = data_model()["tables"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .unwrap();
            if table["columns"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == "id" && c["desktop_type"] == "typed_id")
            {
                data["id"] = record["header"]["id"].clone();
            }
            rows.push(data);
        }
        if entry["count"].as_u64() != Some(rows.len() as u64) {
            return Err(archive_error("Portable table count mismatch"));
        }
        if out.tables.insert(name.into(), rows).is_some() {
            return Err(archive_error("Duplicate portable collection"));
        }
    }
    for entry in manifest["blobs"].as_array().unwrap_or(&Vec::new()) {
        let hash = entry["sha256"]
            .as_str()
            .ok_or_else(|| archive_error("Portable blob hash missing"))?;
        let path = entry_name(
            entry["path"]
                .as_str()
                .ok_or_else(|| archive_error("Portable blob path missing"))?,
        )?;
        let bytes = files
            .get(&path)
            .ok_or_else(|| archive_error("Portable blob missing"))?;
        if hex::encode(Sha256::digest(bytes)) != hash
            || entry["length"].as_u64() != Some(bytes.len() as u64)
        {
            return Err(archive_error("Portable blob integrity mismatch"));
        }
        out.blobs.insert(hash.into(), bytes.clone());
    }
    Ok(out)
}

/// Immutable export capability; no writer or mutable object graph is exposed.
pub trait ArchiveSource {
    fn generation(&self) -> std::sync::Arc<artex_store::ReadGeneration>;
    fn existing_blob_ref(&self, digest: &str, media_type: &str) -> Result<Option<Value>>;
    fn copy_blob_to(&self, reference: &Value, writer: &mut impl Write) -> Result<u64>;
}
impl ArchiveSource for Store {
    fn generation(&self) -> std::sync::Arc<artex_store::ReadGeneration> {
        self.read_generation()
    }
    fn existing_blob_ref(&self, d: &str, m: &str) -> Result<Option<Value>> {
        Store::existing_blob_ref(self, d, m)
    }
    fn copy_blob_to(&self, r: &Value, w: &mut impl Write) -> Result<u64> {
        Store::copy_blob_to(self, r, w)
    }
}
impl ArchiveSource for artex_store::StoreReadView {
    fn generation(&self) -> std::sync::Arc<artex_store::ReadGeneration> {
        self.read_generation()
    }
    fn existing_blob_ref(&self, d: &str, m: &str) -> Result<Option<Value>> {
        self.existing_blob_ref(d, m)
    }
    fn copy_blob_to(&self, r: &Value, w: &mut impl Write) -> Result<u64> {
        self.copy_blob_to(r, w)
    }
}
struct CancelWriter<W> {
    inner: W,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
struct ArchiveStaging(std::path::PathBuf);
impl Drop for ArchiveStaging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
impl<W: Write> Write for CancelWriter<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Archive cancelled",
            ));
        }
        self.inner.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
impl<W: std::io::Seek> std::io::Seek for CancelWriter<W> {
    fn seek(&mut self, p: std::io::SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(p)
    }
}
/// Exports frozen original-model collections, excluding local credentials and
/// run receipts. Destination is installed only after ZIP completion and fsync.
pub fn export_archive(store: &impl ArchiveSource, destination: &Path) -> Result<Value> {
    let generation = store.generation();
    export_snapshot(
        store,
        &generation.snapshot,
        destination,
        None,
        Default::default(),
    )
}
/// Task data plus the FK dependency closure, including direct source tasks
/// referenced by task_relations. Unrelated tasks and captures are excluded.
pub fn export_task_archive(
    store: &impl ArchiveSource,
    task_id: &str,
    destination: &Path,
) -> Result<Value> {
    export_task_archive_cancellable(store, task_id, destination, Default::default())
}
pub fn export_task_archive_cancellable(
    store: &impl ArchiveSource,
    task_id: &str,
    destination: &Path,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Value> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::new("CANCELLED", "Archive cancelled"));
    }
    let generation = store.generation();
    let state = &generation.snapshot;
    let task = state
        .get("tasks", task_id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Task archive target missing"))?;
    let exp = task.data["exploration_id"].clone();
    let mut selected = BTreeSet::new();
    let by_id = state
        .collections
        .iter()
        .flat_map(|(c, rs)| rs.keys().map(move |id| (id.clone(), c.clone())))
        .collect::<BTreeMap<_, _>>();
    fn referenced(v: &Value, out: &mut BTreeSet<String>) {
        match v {
            Value::String(s) => {
                out.insert(s.clone());
            }
            Value::Object(o) => {
                for v in o.values() {
                    referenced(v, out);
                }
            }
            Value::Array(a) => {
                for v in a {
                    referenced(v, out);
                }
            }
            _ => {}
        }
    }
    selected.insert(("tasks".to_owned(), task_id.to_owned()));
    for (collection, records) in &state.collections {
        for (id, r) in records {
            if collection != "task_archives"
                && (r.data["task_id"] == task_id
                    || r.data["exploration_id"] == exp
                    || collection == "explorations" && r.data["id"] == exp)
            {
                selected.insert((collection.clone(), id.clone()));
            }
        }
    }
    loop {
        let before = selected.len();
        for (collection, id) in selected.clone() {
            let row = state.get(&collection, &id).unwrap();
            // SQL does not constrain provenance arrays embedded in node payloads.
            // Preserve those referenced nodes/assets as read-only dependencies too.
            let mut references = BTreeSet::new();
            referenced(&row.data, &mut references);
            for target in references {
                if let Some(c) = by_id.get(&target) {
                    if c != "task_archives" {
                        selected.insert((c.clone(), target));
                    }
                }
            }
            if collection == "findings" {
                for asset_id in row.data["asset_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if state.get("assets", asset_id).is_some() {
                        selected.insert(("assets".into(), asset_id.into()));
                    }
                }
            }
            for fk in artex_core::foreign_keys().iter().filter(|fk| {
                fk.collection == collection && fk.columns.iter().all(|c| !row.data[c].is_null())
            }) {
                for target in state.records(&fk.target).iter().filter(|t| {
                    fk.columns
                        .iter()
                        .zip(&fk.target_columns)
                        .all(|(a, b)| row.data[a] == t.data[b])
                }) {
                    selected.insert((fk.target.clone(), target.header.id.clone()));
                }
            } // Finding evidence children and node asset anchors are task-owned even without task_id.
            if ["findings", "exploration_nodes"].contains(&collection.as_str()) {
                for fk in artex_core::foreign_keys().iter().filter(|fk| {
                    fk.target == collection
                        && [
                            "finding_traffic_bindings",
                            "findings_retests",
                            "exploration_anchors",
                        ]
                        .contains(&fk.collection.as_str())
                }) {
                    for child in state.records(&fk.collection).iter().filter(|r| {
                        fk.columns
                            .iter()
                            .zip(&fk.target_columns)
                            .all(|(a, b)| r.data[a] == row.data[b])
                    }) {
                        selected.insert((fk.collection.clone(), child.header.id.clone()));
                    }
                }
            }
        }
        if selected.len() == before {
            break;
        }
    }
    let mut snapshot = artex_core::Snapshot {
        workspace_id: state.workspace_id.clone(),
        commit_seq: state.commit_seq,
        collections: BTreeMap::new(),
    };
    for (c, id) in selected {
        if data_model()["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == c)
        {
            snapshot
                .collections
                .entry(c.clone())
                .or_default()
                .insert(id.clone(), state.get(&c, &id).unwrap().clone());
        }
    }
    export_snapshot(store, &snapshot, destination, Some(task_id), cancel)
}
fn export_snapshot(
    store: &impl ArchiveSource,
    snapshot: &artex_core::Snapshot,
    destination: &Path,
    task_id: Option<&str>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Value> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(Error::new("CANCELLED", "Archive cancelled"));
    }
    crate::sources::checked_path(destination)?;
    if destination.exists() {
        return Err(Error::new("VERSION_CONFLICT", "Archive destination exists"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Archive parent missing"))?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".archive-{}.tmp", artex_core::new_id("cmd")));
    let _staging_cleanup = ArchiveStaging(staging.clone());
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staging)?;
    let mut zip = zip::ZipWriter::new(CancelWriter {
        inner: file,
        cancel: cancel.clone(),
    });
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut tables = Vec::new();
    let mut hashes = BTreeMap::new();
    for table in data_model()["tables"].as_array().unwrap() {
        let name = table["name"].as_str().unwrap();
        let records = snapshot.records(name);
        if records.is_empty() {
            continue;
        }
        let path = format!("tables/{name}.ndjson");
        zip.start_file(&path, options)
            .map_err(|_| archive_error("Archive write failed"))?;
        let mut digest = Sha256::new();
        for record in &records {
            let value = serde_json::to_value(record)?;
            collect_blob(&value, &mut hashes);
            if name == "traffic_evidence_snapshots" {
                for field in ["req_hash", "resp_hash"] {
                    if let Some(hash) = value["data"][field].as_str().filter(|h| !h.is_empty()) {
                        if let Some(reference) =
                            store.existing_blob_ref(hash, "application/octet-stream")?
                        {
                            hashes
                                .insert(hash.into(), reference["length"].as_str().unwrap().into());
                        } else if !(hash
                            == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                            && value["data"][if field == "req_hash" {
                                "req_len"
                            } else {
                                "resp_len"
                            }] == "0")
                        {
                            return Err(Error::new(
                                "RECOVERY_REQUIRED",
                                "Evidence body blob missing",
                            ));
                        }
                    }
                }
            }
            let mut bytes = serde_json::to_vec(&value)?;
            bytes.push(b'\n');
            digest.update(&bytes);
            zip.write_all(&bytes)?;
        }
        tables.push(json!({"collection":name,"path":path,"count":records.len(),"sha256":hex::encode(digest.finalize())}));
    }
    let mut blobs = Vec::new();
    for (hash, length) in hashes {
        let path = format!("blobs/{hash}.bin");
        zip.start_file(&path, options)
            .map_err(|_| archive_error("Archive blob write failed"))?;
        let reference =
            json!({"sha256":hash,"length":length,"media_type":"application/octet-stream"});
        let offset = store.copy_blob_to(&reference, &mut zip)?;
        blobs.push(json!({"sha256":hash,"path":path,"length":offset}));
    }
    let manifest = json!({"format":"artex-task/1","format_version":1,"workspace_id":snapshot.workspace_id,"commit_seq":snapshot.commit_seq.to_string(),"task_id":task_id,"tables":tables,"blobs":blobs});
    zip.start_file("manifest.json", options)
        .map_err(|_| archive_error("Archive manifest write failed"))?;
    serde_json::to_writer(&mut zip, &manifest)?;
    let file = zip
        .finish()
        .map_err(|_| archive_error("Archive completion failed"))?;
    file.inner.sync_all()?;
    drop(file);
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        let _ = fs::remove_file(&staging);
        return Err(Error::new("CANCELLED", "Archive cancelled"));
    }
    artex_platform::install_new(&staging, destination)?;
    Ok(manifest)
}
#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn cancellation_terminates_write_all_instead_of_retrying_interrupted_forever() {
        let mut writer = CancelWriter {
            inner: std::io::Cursor::new(Vec::<u8>::new()),
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        };
        let error = writer.write_all(b"bounded chunk").unwrap_err();
        assert_ne!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(writer.inner.into_inner().is_empty());
    }
}
fn collect_blob(value: &Value, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(o) => {
            if o.contains_key("sha256") && o.contains_key("length") {
                if let (Some(hash), Some(length)) = (o["sha256"].as_str(), o["length"].as_str()) {
                    out.insert(hash.into(), length.into());
                }
            }
            for v in o.values() {
                collect_blob(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_blob(v, out);
            }
        }
        _ => {}
    }
}
