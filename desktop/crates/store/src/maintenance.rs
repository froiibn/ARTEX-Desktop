use super::*;
use std::time::{Duration, SystemTime};
#[derive(Debug, Clone)]
pub struct GcOptions {
    pub now: SystemTime,
    pub orphan_grace: Duration,
    pub dry_run: bool,
}
impl Default for GcOptions {
    fn default() -> Self {
        Self {
            now: SystemTime::now(),
            orphan_grace: Duration::from_secs(86400),
            dry_run: false,
        }
    }
}
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GcReport {
    pub retained_generations: Vec<String>,
    pub removed_generations: Vec<String>,
    pub removed_segments: Vec<String>,
    pub removed_blobs: Vec<String>,
    pub removed_orphans: Vec<String>,
    pub failures: Vec<Value>,
    pub plan_id: String,
    pub removed_count: u64,
}
pub(super) fn collect_hashes(value: &Value, set: &mut BTreeSet<String>) {
    match value {
        Value::String(s)
            if s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
        {
            set.insert(s.clone());
        }
        Value::Array(a) => {
            for v in a {
                collect_hashes(v, set);
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                collect_hashes(v, set);
            }
        }
        _ => {}
    }
}
impl Store {
    pub fn commit_import(
        &mut self,
        batch: CommandBatch,
        metadata: BTreeMap<(String, String), ImportMetadata>,
    ) -> Result<Receipt> {
        if self.pending_import.is_some() {
            return Err(Error::new("BUSY", "Import already active"));
        }
        if !batch.method.starts_with("migration.") {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Import metadata requires a migration command",
            ));
        }
        for ((collection, id), meta) in &metadata {
            if !batch
                .changes
                .iter()
                .any(|c| matches!(c,Change::Put{collection:c,id:i,..}if c==collection&&i==id))
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Import metadata has no matching put",
                ));
            }
            artex_core::models::Timestamp::parse(meta.created_at.clone())?;
            artex_core::models::Timestamp::parse(meta.updated_at.clone())?;
            if !meta.legacy_key.is_null() && !meta.legacy_key.is_string() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Legacy key must be canonical text",
                ));
            }
        }
        self.pending_import = Some(metadata);
        let result = self.commit(batch);
        self.pending_import = None;
        result
    }
    pub fn read_generation(&self) -> Arc<ReadGeneration> {
        let mut cached = self.read_cache.lock().unwrap();
        cached
            .get_or_insert_with(|| {
                let r = Arc::new(ReadGeneration::new(Arc::new(self.state.clone())));
                self.read_pins.lock().unwrap().push(Arc::downgrade(&r));
                r
            })
            .clone()
    }
    pub fn begin_checkpoint(&mut self) -> Result<CheckpointTicket> {
        self.writable()?;
        if self.checkpoint_job.is_some() {
            return Err(Error::new("BUSY", "Checkpoint already running"));
        }
        let mut generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Generation exhausted"))?;
        while self
            .path
            .join(format!("store/snapshots/{generation:020}"))
            .exists()
            || self
                .path
                .join(format!("store/manifests/{generation:020}.json"))
                .exists()
        {
            generation = generation
                .checked_add(1)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Generation exhausted"))?;
        }
        // A segment boundary at N+1 lets future compaction retain a replay chain
        // rooted in the older checkpoint, even while the writer advances beyond N.
        self.rotate_journal()?;
        let state = self.read_generation().snapshot.clone();
        let ticket = CheckpointTicket {
            generation: generation.to_string(),
            base_seq: state.commit_seq.to_string(),
        };
        let path = self.path.clone();
        let tombstones = self.tombstones.clone();
        let last_hash = self.last_hash;
        let fault = self.fault_name();
        let pinned = state.clone();
        let worker = std::thread::Builder::new()
            .name("artex-checkpoint".into())
            .spawn(move || {
                write_checkpoint(&path, &pinned, &tombstones, last_hash, generation, fault)
            })?;
        self.checkpoint_job = Some(CheckpointJob {
            ticket: ticket.clone(),
            _state: state,
            worker,
        });
        self.generation = generation;
        Ok(ticket)
    }
    pub fn poll_checkpoint(&mut self) -> Result<Option<Value>> {
        if self
            .checkpoint_job
            .as_ref()
            .is_some_and(|j| j.worker.is_finished())
        {
            self.wait_checkpoint().map(Some)
        } else {
            Ok(None)
        }
    }
    pub fn checkpoint_status(&self) -> Option<CheckpointTicket> {
        self.checkpoint_job.as_ref().map(|j| j.ticket.clone())
    }
    pub fn wait_checkpoint(&mut self) -> Result<Value> {
        let job = self
            .checkpoint_job
            .take()
            .ok_or_else(|| Error::new("NOT_FOUND", "No checkpoint job"))?;
        let result = job
            .worker
            .join()
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Checkpoint worker terminated"))??;
        self.valid_generations.insert(0, job.ticket.generation);
        Ok(result)
    }
    pub fn maintenance(&mut self, options: GcOptions) -> Result<GcReport> {
        self.writable()?;
        if options.orphan_grace < Duration::from_secs(86400) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Orphan grace cannot be below 24 hours",
            ));
        }
        if self.checkpoint_job.is_some() {
            return Err(Error::new(
                "BUSY",
                "Checkpoint pins must finish before compaction",
            ));
        }
        let mut manifests = list_numbered(&self.path.join("store/manifests"), "json")?;
        manifests.reverse();
        let mut good = Vec::new();
        for (g, p) in &manifests {
            let m: Value = serde_json::from_slice(&fs::read(p)?)?;
            if self.load_manifest(&m).is_ok() {
                good.push((*g, m));
            }
        }
        if good.len() < 2 {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Compaction requires two verified checkpoints",
            ));
        }
        let keep = &good[..2];
        let mut report = GcReport {
            retained_generations: keep.iter().map(|(g, _)| g.to_string()).collect(),
            plan_id: new_id("command"),
            ..Default::default()
        };
        let older = parse_u64(required_string(&keep[1].1, "base_seq")?)?;
        let mut live = BTreeSet::new();
        collect_hashes(&serde_json::to_value(&self.state)?, &mut live);
        for (_, m) in keep {
            let (s, _, _) = self.load_manifest(m)?;
            collect_hashes(&serde_json::to_value(s)?, &mut live);
        }
        // Every live Arc reader pins its old generation even after the writer
        // invalidates the current read cache.
        let mut pins = self.read_pins.lock().unwrap();
        pins.retain(|p| p.strong_count() > 0);
        for pin in pins.iter().filter_map(|p| p.upgrade()) {
            collect_hashes(&serde_json::to_value(&*pin.snapshot)?, &mut live);
        }
        drop(pins);
        let segments = list_numbered(&self.path.join("store/journal"), "jnl")?;
        // Audit even the prefix proposed for removal. Corruption must not be
        // hidden by compaction after another process modifies a closed file.
        let first = segments
            .first()
            .map(|(n, _)| *n)
            .unwrap_or(self.state.commit_seq + 1);
        let mut sequence = first;
        let mut previous = if first == 1 {
            [0; 32]
        } else {
            let manifest = good
                .iter()
                .find(|(_, m)| m["base_seq"] == (first - 1).to_string())
                .ok_or_else(|| {
                    Error::new("RECOVERY_REQUIRED", "Compaction replay anchor missing")
                })?;
            self.load_manifest(&manifest.1)?.2
        };
        for (start, path) in &segments {
            if *start != sequence {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Journal segment gap before compaction",
                ));
            }
            audit_journal(path, &mut sequence, &mut previous)?;
        }
        if sequence != self.state.commit_seq + 1 || previous != self.last_hash {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Journal state differs before compaction",
            ));
        }
        let prune_aligned = segments.iter().any(|(start, _)| *start == older + 1);
        for (i, (start, path)) in segments.iter().enumerate() {
            let end = segments
                .get(i + 1)
                .map(|x| x.0 - 1)
                .unwrap_or(self.state.commit_seq);
            if prune_aligned && end <= older && path != &self.journal_path {
                report
                    .removed_segments
                    .push(path.file_name().unwrap().to_string_lossy().into());
            } else {
                visit_journal(path, |payload| {
                    collect_hashes(payload, &mut live);
                    Ok(())
                })?;
            }
            let _ = start;
        }
        for (g, _) in &manifests {
            if !report.retained_generations.contains(&g.to_string()) {
                report.removed_generations.push(g.to_string());
            }
        }
        for (root, directories) in [
            ("tmp", false),
            ("store/snapshots", true),
            ("backups", false),
        ] {
            for entry in fs::read_dir(self.path.join(root))? {
                let entry = entry?;
                reject_links(&entry.path())?;
                if options
                    .now
                    .duration_since(entry.metadata()?.modified()?)
                    .unwrap_or_default()
                    < options.orphan_grace
                {
                    continue;
                }
                if directories {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.len() != 20
                        || !name.bytes().all(|b| b.is_ascii_digit())
                        || manifests.iter().any(|(g, _)| format!("{g:020}") == name)
                    {
                        continue;
                    }
                    if !entry.file_type()?.is_dir() {
                        continue;
                    }
                } else if !entry.file_type()?.is_file() {
                    continue;
                }
                if root == "backups"
                    && !entry
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".journal-trash")
                {
                    continue;
                }
                report
                    .removed_orphans
                    .push(format!("{root}/{}", entry.file_name().to_string_lossy()));
            }
        }
        {
            for dir in fs::read_dir(self.path.join("objects/blobs"))? {
                let dir = dir?;
                reject_links(&dir.path())?;
                if !dir.file_type()?.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(dir.path())? {
                    let entry = entry?;
                    reject_links(&entry.path())?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if live.contains(&name) || name.len() != 64 {
                        continue;
                    }
                    if options
                        .now
                        .duration_since(entry.metadata()?.modified()?)
                        .unwrap_or_default()
                        >= options.orphan_grace
                    {
                        report.removed_blobs.push(name);
                    }
                }
            }
        }
        let plan = self
            .path
            .join(format!("backups/gc-{}.json", report.plan_id));
        if !options.dry_run {
            atomic_new(&plan, &serde_json::to_vec(&report)?)?;
            for g in &report.removed_generations {
                let manifest = self
                    .path
                    .join(format!("store/manifests/{:020}.json", parse_u64(g)?));
                let trash = self
                    .path
                    .join(format!("backups/gc-{}-manifest-{g}.json", report.plan_id));
                if let Err(e) = artex_platform::install_new(&manifest, &trash) {
                    report.failures.push(
                        json!({"kind":"manifest","generation":g,"error":e.kind().to_string()}),
                    );
                    atomic_new(
                        &self
                            .path
                            .join(format!("backups/gc-{}-result.json", report.plan_id)),
                        &serde_json::to_vec(&report)?,
                    )?;
                    return Ok(report);
                }
                sync_directory(manifest.parent().unwrap())?;
                let dir = self
                    .path
                    .join(format!("store/snapshots/{:020}", parse_u64(g)?));
                let failures_before = report.failures.len();
                remove_regular_tree(&dir, &mut report.failures)?;
                if report.failures.len() == failures_before {
                    report.removed_count += 1;
                }
            }
            for name in &report.removed_segments {
                let p = self.path.join("store/journal").join(name);
                reject_links(&p)?;
                let trash = self.path.join(format!(
                    "backups/gc-{}-{name}.journal-trash",
                    report.plan_id
                ));
                if let Err(e) = artex_platform::install_new(&p, &trash) {
                    report
                        .failures
                        .push(json!({"kind":"journal","file":name,"error":e.kind().to_string()}));
                } else {
                    // The write-through rename prevents a removed old prefix from
                    // reappearing after a Windows crash without its old anchor.
                    sync_directory(p.parent().unwrap())?;
                    if let Err(e) = fs::remove_file(&trash) {
                        report.failures.push(json!({"kind":"journal_trash","file":name,"error":e.kind().to_string()}));
                    } else {
                        report.removed_count += 1;
                    }
                }
            }
            for sha in &report.removed_blobs {
                let p = self.path.join("objects/blobs").join(&sha[..2]).join(sha);
                reject_links(&p)?;
                if digest_file(&p)? != *sha {
                    return Err(Error::new(
                        "CORRUPT_STORE",
                        "Orphan blob hash changed before deletion",
                    ));
                }
                if let Err(e) = fs::remove_file(&p) {
                    report
                        .failures
                        .push(json!({"kind":"blob","sha256":sha,"error":e.kind().to_string()}));
                } else {
                    report.removed_count += 1;
                    sync_directory(p.parent().unwrap())?;
                }
            }
            for relative in &report.removed_orphans {
                safe_relative(relative)?;
                let path = self.path.join(relative);
                reject_links(&path)?;
                let failures_before = report.failures.len();
                if path.is_dir() {
                    remove_regular_tree(&path, &mut report.failures)?;
                } else if let Err(e) = fs::remove_file(&path) {
                    report.failures.push(
                        json!({"kind":"orphan","path":relative,"error":e.kind().to_string()}),
                    );
                }
                if report.failures.len() == failures_before {
                    report.removed_count += 1;
                }
                sync_directory(path.parent().unwrap())?;
            }
            sync_directory(&self.path.join("store/journal"))?;
            atomic_new(
                &self
                    .path
                    .join(format!("backups/gc-{}-result.json", report.plan_id)),
                &serde_json::to_vec(&report)?,
            )?;
            self.valid_generations = report.retained_generations.clone();
        }
        Ok(report)
    }
    pub fn export_recovery_copy(&self, destination: &Path) -> Result<Value> {
        reject_links(destination)?;
        let source_absolute = fs::canonicalize(&self.path)?;
        let destination_absolute =
            fs::canonicalize(destination.parent().ok_or_else(|| {
                Error::new("INVALID_PARAMS", "Recovery destination parent missing")
            })?)?
            .join(destination.file_name().ok_or_else(|| {
                Error::new("INVALID_PARAMS", "Recovery destination filename missing")
            })?);
        if destination_absolute.starts_with(&source_absolute) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Recovery copy must be outside source workspace",
            ));
        }
        if destination.exists() && fs::read_dir(destination)?.next().is_some() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Recovery destination must be empty",
            ));
        }
        let staging = destination
            .parent()
            .unwrap()
            .join(format!(".artex-recovery-{}", artex_core::new_id("cmd")));
        let mut target = Store::create(&staging, &format!("{} (recovery copy)", self.name))?;
        let mut hashes = BTreeSet::new();
        collect_hashes(&serde_json::to_value(&self.state)?, &mut hashes);
        let mut required = BTreeSet::new();
        required_blob_hashes(&serde_json::to_value(&self.state)?, &mut required);
        for sha in hashes {
            let path = self.path.join("objects/blobs").join(&sha[..2]).join(&sha);
            if required.contains(&sha) && !path.is_file() {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Recovery copy requires a missing referenced blob",
                ));
            }
            if path.exists() {
                reject_links(&path)?;
                if digest_file(&path)? != sha {
                    return Err(Error::new(
                        "CORRUPT_STORE",
                        "Recovery references a damaged blob",
                    ));
                }
                let dest = target.path.join("objects/blobs").join(&sha[..2]);
                fs::create_dir_all(&dest)?;
                let dest = dest.join(&sha);
                fs::copy(path, &dest)?;
                if digest_file(&dest)? != sha {
                    return Err(Error::new(
                        "CORRUPT_STORE",
                        "Blob changed during recovery copy",
                    ));
                }
                OpenOptions::new().write(true).open(&dest)?.sync_all()?;
                sync_directory(dest.parent().unwrap())?;
            }
        }
        let copy_id = target.state.workspace_id.clone();
        target.state = self.state.clone();
        target.state.workspace_id = copy_id;
        *target.read_cache.lock().unwrap() = None;
        target.tombstones = self.tombstones.clone();
        target.last_hash = self.last_hash;
        target.journal.take();
        for (_, p) in list_numbered(&target.path.join("store/journal"), "jnl")? {
            fs::remove_file(p)?;
        }
        target.journal_path = PathBuf::new();
        target.rotate_journal()?;
        target.checkpoint()?;
        target.checkpoint()?;
        let report = json!({"format":"artex-recovery-copy","format_version":1,"source_workspace_id":self.state.workspace_id,"last_valid_seq":self.state.commit_seq.to_string(),"recovery":self.recovery(),"copy_workspace_id":target.state.workspace_id,"source_unchanged":true});
        atomic_new(
            &target.path.join("backups/recovery-report.json"),
            &serde_json::to_vec(&report)?,
        )?;
        // Publish only after both complete generations and all referenced blobs
        // are durable. Failure leaves an explicit staging copy, never a usable
        // empty destination masquerading as a completed recovery workspace.
        target.check_blobs(&serde_json::to_value(&target.state)?)?;
        sync_directory(&target.path)?;
        drop(target);
        if destination.exists() {
            fs::remove_dir(destination)?;
        }
        artex_platform::install_new(&staging, destination)?;
        Ok(report)
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        if let Some(job) = self.checkpoint_job.take() {
            let _ = job.worker.join();
        }
    }
}
fn digest_file(path: &Path) -> Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}
fn required_blob_hashes(value: &Value, set: &mut BTreeSet<String>) {
    match value {
        Value::Object(o) => {
            if o.contains_key("sha256") && o.contains_key("length") && o.contains_key("media_type")
            {
                if let Some(h) = o["sha256"].as_str() {
                    set.insert(h.into());
                }
            }
            for v in o.values() {
                required_blob_hashes(v, set);
            }
        }
        Value::Array(a) => {
            for v in a {
                required_blob_hashes(v, set);
            }
        }
        _ => {}
    }
}
fn visit_journal(path: &Path, mut visitor: impl FnMut(&Value) -> Result<()>) -> Result<()> {
    let mut file = File::open(path)?;
    loop {
        let mut header = [0; HEADER_LEN];
        let n = file.read(&mut header[..1])?;
        if n == 0 {
            break;
        }
        file.read_exact(&mut header[1..])?;
        let len = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        if len > MAX_PAYLOAD || &header[..8] != b"ARTEXJ01" {
            return Err(Error::new("CORRUPT_STORE", "Invalid retained journal"));
        }
        let mut bytes = vec![0; len + 8];
        file.read_exact(&mut bytes)?;
        if &bytes[len..] != FOOTER || header[72..104] != hash(&bytes[..len]) {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Retained journal checksum mismatch",
            ));
        }
        let v: Value = serde_json::from_slice(&bytes[..len])?;
        visitor(&v)?;
    }
    Ok(())
}
fn audit_journal(path: &Path, sequence: &mut u64, previous: &mut [u8; 32]) -> Result<()> {
    reject_links(path)?;
    let mut file = File::open(path)?;
    loop {
        let mut header = [0; HEADER_LEN];
        if file.read(&mut header[..1])? == 0 {
            break;
        }
        file.read_exact(&mut header[1..])?;
        let length = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
        if length > MAX_PAYLOAD
            || &header[..8] != b"ARTEXJ01"
            || u16::from_le_bytes(header[8..10].try_into().unwrap()) != 1
            || u16::from_le_bytes(header[10..12].try_into().unwrap()) != 0
            || u64::from_le_bytes(header[16..24].try_into().unwrap()) != *sequence
            || header[40..72] != *previous
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Invalid journal frame before compaction",
            ));
        }
        let mut payload = vec![0; length + 8];
        file.read_exact(&mut payload)?;
        if &payload[length..] != FOOTER || header[72..104] != hash(&payload[..length]) {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Journal digest mismatch before compaction",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(header);
        digest.update(payload);
        *previous = digest.finalize().into();
        *sequence = sequence
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Journal sequence exhausted"))?;
    }
    Ok(())
}
fn remove_regular_tree(dir: &Path, failures: &mut Vec<Value>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    reject_links(dir)?;
    for entry in fs::read_dir(dir)? {
        let e = entry?;
        reject_links(&e.path())?;
        if e.file_type()?.is_dir() {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Unexpected nested snapshot directory",
            ));
        }
        if let Err(err) = fs::remove_file(e.path()) {
            failures.push(json!({"kind":"snapshot","error":err.kind().to_string()}));
        }
    }
    if let Err(e) = fs::remove_dir(dir) {
        failures.push(json!({"kind":"snapshot_directory","error":e.kind().to_string()}));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Expected, default_data};
    fn put(store: &mut Store, id: &str, name: &str) {
        let mut d = default_data("companies").unwrap();
        d["name"] = json!(name);
        d["nkey"] = json!(name);
        let expected = store
            .snapshot()
            .get("companies", id)
            .map(|r| {
                vec![Expected {
                    collection: "companies".into(),
                    id: id.into(),
                    version: r.header.version.clone(),
                }]
            })
            .unwrap_or_default();
        store
            .commit(CommandBatch {
                command_id: new_id("command"),
                method: "fixture.put".into(),
                normalized_params: json!({"name":name}),
                expected,
                changes: vec![Change::Put {
                    collection: "companies".into(),
                    id: id.into(),
                    data: d,
                }],
                events: vec![],
                deliveries: vec![],
                result: json!({}),
            })
            .unwrap();
    }
    #[test]
    fn background_checkpoint_pins_base_while_writer_continues() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("w");
        let mut s = Store::create(&p, "bg").unwrap();
        let id = new_id("company");
        put(&mut s, &id, "before");
        let old = s.read_generation();
        let ticket = s.begin_checkpoint().unwrap();
        put(&mut s, &id, "after");
        assert_eq!(ticket.base_seq, "1");
        assert_eq!(
            old.snapshot.get("companies", &id).unwrap().data["name"],
            "before"
        );
        assert_eq!(s.wait_checkpoint().unwrap()["base_seq"], "1");
        assert_eq!(s.read_generation().snapshot.commit_seq, 2);
        drop(s);
        let reopened = Store::open(&p).unwrap();
        assert!(!reopened.read_only());
        assert_eq!(
            reopened.snapshot().get("companies", &id).unwrap().data["name"],
            "after"
        );
    }
    #[test]
    fn compacted_chain_recovers_from_both_generations_and_retains_receipts() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("w");
        let mut s = Store::create(&p, "gc").unwrap();
        let id = new_id("company");
        for i in 0..5 {
            put(&mut s, &id, &format!("name{i}"));
            s.checkpoint().unwrap();
        }
        let expected = s.snapshot();
        let report = s.maintenance(GcOptions::default()).unwrap();
        assert_eq!(report.retained_generations.len(), 2);
        assert!(!report.removed_segments.is_empty());
        assert!(report.failures.is_empty());
        assert_eq!(s.snapshot().records("command_receipts").len(), 5);
        let latest = parse_u64(&report.retained_generations[0]).unwrap();
        drop(s);
        let reopened = Store::open(&p).unwrap();
        assert!(!reopened.read_only(), "{:?}", reopened.recovery());
        assert_eq!(reopened.snapshot().collections, expected.collections);
        drop(reopened);
        fs::write(p.join(format!("store/manifests/{latest:020}.json")), b"bad").unwrap();
        let recovered = Store::open(&p).unwrap();
        assert!(!recovered.read_only(), "{:?}", recovered.recovery());
        assert_eq!(recovered.snapshot().collections, expected.collections);
    }
    #[test]
    fn gc_preserves_live_reader_blobs_and_observes_24h_grace() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("w");
        let mut s = Store::create(&p, "gc").unwrap();
        let orphan = s.blob_put(b"orphan", "text/plain").unwrap();
        let protected = s.blob_put(b"reader", "text/plain").unwrap();
        let mut pinned = s.snapshot();
        let mut d = default_data("companies").unwrap();
        d["id"] = json!(new_id("http_session"));
        d["request"] = json!({"text":null,"blob":protected});
        let id = new_id("http_session");
        pinned
            .collections
            .entry("http_sessions".into())
            .or_default()
            .insert(
                id.clone(),
                Record {
                    header: Header {
                        id,
                        collection: "http_sessions".into(),
                        version: "1".into(),
                        commit_seq: "1".into(),
                        created_at: now(),
                        updated_at: now(),
                        legacy_key: Value::Null,
                    },
                    data: d,
                },
            );
        let pin = Arc::new(ReadGeneration::new(Arc::new(pinned)));
        s.read_pins.lock().unwrap().push(Arc::downgrade(&pin));
        let young = s
            .maintenance(GcOptions {
                dry_run: true,
                ..Default::default()
            })
            .unwrap();
        assert!(young.removed_blobs.is_empty());
        let future = SystemTime::now() + Duration::from_secs(86401);
        let report = s
            .maintenance(GcOptions {
                now: future,
                ..Default::default()
            })
            .unwrap();
        assert!(
            report
                .removed_blobs
                .contains(&orphan["sha256"].as_str().unwrap().into())
        );
        assert!(
            !report
                .removed_blobs
                .contains(&protected["sha256"].as_str().unwrap().into())
        );
        drop(pin);
        let next = s
            .maintenance(GcOptions {
                now: future,
                ..Default::default()
            })
            .unwrap();
        assert!(
            next.removed_blobs
                .contains(&protected["sha256"].as_str().unwrap().into())
        );
    }
    #[test]
    fn indexed_lookup_and_budgeted_search_use_stable_generation() {
        let t = tempfile::tempdir().unwrap();
        let mut s = Store::create(&t.path().join("w"), "query").unwrap();
        for i in 0..4 {
            put(
                &mut s,
                &format!("company_{i:032x}"),
                &if i == 3 {
                    "needle".into()
                } else {
                    format!("plain{i}")
                },
            );
        }
        let reader = s.read_generation();
        let one = reader.search("companies", "needle", None, 1, 2).unwrap();
        assert_eq!(one.scanned, 2);
        assert!(one.records.is_empty());
        let two = reader
            .search("companies", "needle", one.next_after.as_deref(), 1, 2)
            .unwrap();
        assert_eq!(two.records.len(), 1);
        assert!(two.next_after.is_none());
        let exact = reader
            .lookup("companies", "name", &json!("needle"), None, 20)
            .unwrap();
        assert_eq!(exact.records.len(), 1);
        let typed = artex_core::models::Id::<artex_core::models::Companies>::parse(
            exact.records[0].header.id.clone(),
        )
        .unwrap();
        assert_eq!(reader.typed(&typed).unwrap().data.name, "needle");
        assert!(
            artex_core::models::Id::<artex_core::models::Tasks>::parse(typed.as_str()).is_err()
        );
        assert!(reader.search("companies", "", None, 1001, 1).is_err());
    }
    #[test]
    fn recovery_copy_is_independent_and_never_rolls_back_source() {
        let t = tempfile::tempdir().unwrap();
        let source = t.path().join("w");
        let copy = t.path().join("copy");
        let mut s = Store::create(&source, "recover").unwrap();
        let id = new_id("company");
        put(&mut s, &id, "kept");
        let report = s.export_recovery_copy(&copy).unwrap();
        assert_eq!(report["last_valid_seq"], "1");
        let c = Store::open(&copy).unwrap();
        assert!(!c.read_only(), "{:?}", c.recovery());
        assert_eq!(c.snapshot().collections, s.snapshot().collections);
        assert_ne!(c.snapshot().workspace_id, s.snapshot().workspace_id);
        put(&mut s, &id, "new source");
        assert_eq!(
            c.snapshot().get("companies", &id).unwrap().data["name"],
            "kept"
        );
    }
    #[test]
    fn read_worker_keeps_the_native_writer_lock_until_its_last_view_drops() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("w");
        let store = Store::create(&path, "lock lifetime").unwrap();
        let view = store.read_view();
        drop(store);
        assert!(matches!(Store::open(&path),Err(e)if e.code=="BUSY"));
        assert_eq!(view.read_generation().commit_seq, 0);
        drop(view);
        assert!(Store::open(&path).is_ok());
    }
    #[test]
    fn sealed_file_blob_is_streamed_and_bad_recovery_is_never_published() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("w");
        let mut store = Store::create(&path, "stream").unwrap();
        let source = t.path().join("large.bin");
        let bytes = vec![0xabu8; 192 * 1024 + 7];
        fs::write(&source, &bytes).unwrap();
        let reference = store
            .blob_put_file(&source, "application/octet-stream")
            .unwrap();
        assert_eq!(reference["length"], bytes.len().to_string());
        assert_eq!(
            store.blob_read(&reference, 65536, 65536).unwrap(),
            bytes[65536..131072]
        );
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let id = new_id("company");
        let mut data = artex_core::default_data("companies").unwrap();
        data["id"] = json!(id);
        data["name"] = json!("blob body");
        data["description"] = json!({"text":null,"blob":reference});
        let b = CommandBatch {
            command_id: new_id("command"),
            method: "fixture.put".into(),
            normalized_params: json!({}),
            expected: vec![],
            changes: vec![Change::Put {
                collection: "companies".into(),
                id: id.clone(),
                data,
            }],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        };
        // companies.description is a string in the source model. The blob is
        // instead retained by a command receipt result, which is equally durable.
        let mut b = b;
        b.changes.clear();
        b.result = json!({"body":{"text":null,"blob":reference}});
        store.commit(b).unwrap();
        fs::remove_file(store.blob_path(&reference).unwrap()).unwrap();
        let destination = t.path().join("never-published");
        assert!(store.export_recovery_copy(&destination).is_err());
        assert!(!destination.exists());
        assert_eq!(store.snapshot().commit_seq, 1);
    }
}
