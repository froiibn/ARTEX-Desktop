//! SQLite is the authoritative v2 store. One transaction persists changed
//! objects, foreign-key edges, tombstones, receipt and sequence together. The
//! Rust Snapshot remains the immutable UI/domain projection, not a journal.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
const DB_FILE: &str = "store/workspace.sqlite3";
const SCHEMA_VERSION: i64 = 1;

pub(super) struct Database {
    connection: Connection,
    // Drop the connection before its private read-only recovery replica.
    _replica: Option<tempfile::TempDir>,
}
fn sql_error(error: rusqlite::Error) -> Error {
    let code = match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => "BUSY",
        Some(rusqlite::ErrorCode::ConstraintViolation) => "VERSION_CONFLICT",
        Some(rusqlite::ErrorCode::ReadOnly) => "READ_ONLY",
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            "CORRUPT_STORE"
        }
        _ => "IO_FAILURE",
    };
    // SQLite diagnostics can include raw record values or filenames. Public
    // errors expose the category/extended code only, never credentials/data.
    let mut result = Error::new(code, "SQLite operation failed");
    result.recovery_required = code == "CORRUPT_STORE";
    result.details = json!({"sqlite_code":error.sqlite_error().map(|e|e.extended_code)});
    result
}
fn metadata(c: &Connection, key: &str) -> Result<String> {
    c.query_row("SELECT value FROM metadata WHERE key=?1", [key], |r| {
        r.get(0)
    })
    .map_err(sql_error)
}
fn set_meta(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute("INSERT INTO metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value]).map_err(sql_error)?;
    Ok(())
}
fn insert_record(c: &Connection, record: &Record) -> Result<()> {
    c.execute("INSERT INTO objects(collection,id,version,commit_seq,header,data) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(collection,id) DO UPDATE SET version=excluded.version,commit_seq=excluded.commit_seq,header=excluded.header,data=excluded.data",
        params![record.header.collection,record.header.id,record.header.version,record.header.commit_seq,serde_json::to_string(&record.header)?,serde_json::to_string(&record.data)?]).map_err(sql_error)?;
    Ok(())
}
fn insert_tombstone(c: &Connection, t: &Tombstone) -> Result<()> {
    c.execute(
        "INSERT INTO tombstones(collection,id,record) VALUES(?1,?2,?3)",
        params![t.collection, t.id, serde_json::to_string(t)?],
    )
    .map_err(sql_error)?;
    Ok(())
}
fn edges(c: &Connection, s: &Snapshot, record: &Record) -> Result<()> {
    c.execute(
        "DELETE FROM relationships WHERE source_collection=?1 AND source_id=?2",
        params![record.header.collection, record.header.id],
    )
    .map_err(sql_error)?;
    for (ordinal, fk) in artex_core::foreign_keys()
        .iter()
        .enumerate()
        .filter(|(_, fk)| fk.collection == record.header.collection)
    {
        if fk.columns.iter().any(|field| record.data[field].is_null()) {
            continue;
        }
        let target = s
            .records(&fk.target)
            .into_iter()
            .find(|target| {
                fk.columns
                    .iter()
                    .zip(&fk.target_columns)
                    .all(|(a, b)| record.data[a] == target.data[b])
            })
            .ok_or_else(|| Error::new("VERSION_CONFLICT", "Foreign key target does not exist"))?;
        c.execute("INSERT INTO relationships(source_collection,source_id,relation,target_collection,target_id) VALUES(?1,?2,?3,?4,?5)",params![record.header.collection,record.header.id,ordinal as i64,fk.target,target.header.id]).map_err(sql_error)?;
    }
    Ok(())
}
impl Database {
    fn open(path: &Path, read_only: bool) -> Result<Self> {
        reject_links(path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            reject_links(Path::new(&sidecar))?;
        }
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        let mut replica = None;
        let c = if read_only && fs::metadata(Path::new(&wal)).is_ok_and(|m| m.len() > 0) {
            // SQLite may rebuild the WAL-index after an unclean shutdown. Give
            // the engine a locked, private copy so requested read-only never
            // creates sidecars or recovers/truncates bytes in the source.
            let directory = tempfile::tempdir()?;
            let copy = directory.path().join("snapshot.sqlite3");
            fs::copy(path, &copy)?;
            let mut copy_wal = copy.as_os_str().to_os_string();
            copy_wal.push("-wal");
            fs::copy(Path::new(&wal), Path::new(&copy_wal))?;
            let c = Connection::open_with_flags(
                &copy,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql_error)?;
            c.execute_batch("PRAGMA query_only=ON;")
                .map_err(sql_error)?;
            replica = Some(directory);
            c
        } else if read_only {
            let mut uri = url::Url::from_file_path(fs::canonicalize(path)?).map_err(|_| {
                Error::new(
                    "INVALID_PARAMS",
                    "SQLite path cannot be represented as a file URL",
                )
            })?;
            uri.query_pairs_mut().append_pair("immutable", "1");
            Connection::open_with_flags(
                uri.as_str(),
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_URI
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql_error)?
        } else {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql_error)?
        };
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;")
            .map_err(sql_error)?;
        // No retry sleeps on the app writer. A busy external connection is an
        // explicit error; application commands are already serialized by Store.
        c.busy_timeout(std::time::Duration::ZERO)
            .map_err(sql_error)?;
        let version: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(sql_error)?;
        if version != SCHEMA_VERSION {
            return Err(Error::new(
                "VERSION_MISMATCH",
                "SQLite store schema is unsupported",
            ));
        }
        let check: String = c
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(sql_error)?;
        if check != "ok" {
            return Err(Error::new("CORRUPT_STORE", "SQLite integrity check failed"));
        }
        if c.prepare("PRAGMA foreign_key_check")
            .map_err(sql_error)?
            .exists([])
            .map_err(sql_error)?
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "SQLite foreign key check failed",
            ));
        }
        if !read_only {
            c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
                .map_err(sql_error)?;
        }
        Ok(Self {
            connection: c,
            _replica: replica,
        })
    }
    fn initialize(
        path: &Path,
        state: &Snapshot,
        tombstones: &BTreeMap<(String, String), Tombstone>,
        name: &str,
        legacy_roots: &[String],
    ) -> Result<()> {
        reject_links(path)?;
        // Reserve the path with create_new; never overwrite an unrelated DB.
        write_new_sync(path, &[])?;
        let mut c = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql_error)?;
        c.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;
            CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE objects(collection TEXT NOT NULL,id TEXT NOT NULL,version TEXT NOT NULL,commit_seq TEXT NOT NULL,header TEXT NOT NULL CHECK(json_valid(header)),data TEXT NOT NULL CHECK(json_valid(data)),PRIMARY KEY(collection,id)) WITHOUT ROWID;
            CREATE INDEX objects_sequence ON objects(collection,commit_seq,id);
            CREATE INDEX objects_task ON objects(collection,json_extract(data,'$.task_id'),id);
            CREATE INDEX objects_run ON objects(collection,json_extract(data,'$.run_id'),id);
            CREATE INDEX objects_exploration ON objects(collection,json_extract(data,'$.exploration_id'),id);
            CREATE TABLE relationships(source_collection TEXT NOT NULL,source_id TEXT NOT NULL,relation INTEGER NOT NULL,target_collection TEXT NOT NULL,target_id TEXT NOT NULL,PRIMARY KEY(source_collection,source_id,relation),FOREIGN KEY(source_collection,source_id) REFERENCES objects(collection,id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,FOREIGN KEY(target_collection,target_id) REFERENCES objects(collection,id) DEFERRABLE INITIALLY DEFERRED) WITHOUT ROWID;
            CREATE INDEX relationships_target ON relationships(target_collection,target_id);
            CREATE TABLE tombstones(collection TEXT NOT NULL,id TEXT NOT NULL,record TEXT NOT NULL CHECK(json_valid(record)),PRIMARY KEY(collection,id)) WITHOUT ROWID;
            CREATE TABLE checkpoints(generation TEXT PRIMARY KEY,base_seq TEXT NOT NULL,manifest TEXT NOT NULL,sha256 TEXT NOT NULL) WITHOUT ROWID;
            CREATE TABLE legacy_blob_roots(sha256 TEXT PRIMARY KEY) WITHOUT ROWID;
            PRAGMA user_version=1;").map_err(sql_error)?;
        let tx = c
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        set_meta(&tx, "workspace_id", &state.workspace_id)?;
        set_meta(&tx, "name", name)?;
        set_meta(&tx, "commit_seq", &state.commit_seq.to_string())?;
        set_meta(&tx, "generation", "0")?;
        for records in state.collections.values() {
            for r in records.values() {
                insert_record(&tx, r)?;
            }
        }
        for records in state.collections.values() {
            for r in records.values() {
                edges(&tx, state, r)?;
            }
        }
        for t in tombstones.values() {
            insert_tombstone(&tx, t)?;
        }
        for sha in legacy_roots {
            tx.execute("INSERT INTO legacy_blob_roots VALUES(?1)", [sha])
                .map_err(sql_error)?;
        }
        tx.commit().map_err(sql_error)?;
        c.close().map_err(|(_, e)| sql_error(e))?;
        OpenOptions::new().write(true).open(path)?.sync_all()?;
        sync_directory(path.parent().unwrap())?;
        Ok(())
    }
    fn load(&self, workspace: &str) -> Result<(Snapshot, BTreeMap<(String, String), Tombstone>)> {
        let c = &self.connection;
        if metadata(c, "workspace_id")? != workspace {
            return Err(Error::new(
                "CORRUPT_STORE",
                "SQLite workspace identity mismatch",
            ));
        }
        let mut state = Snapshot {
            workspace_id: workspace.into(),
            commit_seq: parse_u64(&metadata(c, "commit_seq")?)?,
            collections: BTreeMap::new(),
        };
        let mut query=c.prepare("SELECT collection,id,header,data,version,commit_seq FROM objects ORDER BY collection,id").map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let collection: String = row.get(0).map_err(sql_error)?;
            let id: String = row.get(1).map_err(sql_error)?;
            let header: Header =
                serde_json::from_str(&row.get::<_, String>(2).map_err(sql_error)?)?;
            if header.collection != collection
                || header.id != id
                || header.version != row.get::<_, String>(4).map_err(sql_error)?
                || header.commit_seq != row.get::<_, String>(5).map_err(sql_error)?
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "SQLite row/header identity mismatch",
                ));
            }
            let data = serde_json::from_str(&row.get::<_, String>(3).map_err(sql_error)?)?;
            state
                .collections
                .entry(collection)
                .or_default()
                .insert(id, Record { header, data });
        }
        validate_snapshot(&state)?;
        self.validate_relationships(&state)?;
        let mut deleted = BTreeMap::new();
        let mut query = c
            .prepare("SELECT collection,id,record FROM tombstones")
            .map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        while let Some(row) = rows.next().map_err(sql_error)? {
            let t: Tombstone = serde_json::from_str(&row.get::<_, String>(2).map_err(sql_error)?)?;
            if t.collection != row.get::<_, String>(0).map_err(sql_error)?
                || t.id != row.get::<_, String>(1).map_err(sql_error)?
                || state.get(&t.collection, &t.id).is_some()
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "SQLite tombstone identity mismatch",
                ));
            }
            if !valid_id(&t.id, collection_prefix(&t.collection)?)
                || parse_u64(&t.version)? == 0
                || parse_u64(&t.commit_seq)? > state.commit_seq
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "SQLite tombstone version/sequence mismatch",
                ));
            }
            artex_core::models::Timestamp::parse(t.deleted_at.clone())?;
            deleted.insert((t.collection.clone(), t.id.clone()), t);
        }
        self.generations(state.commit_seq)?;
        Ok((state, deleted))
    }
    fn validate_relationships(&self, state: &Snapshot) -> Result<()> {
        let mut expected = BTreeSet::new();
        for (ordinal, fk) in artex_core::foreign_keys().iter().enumerate() {
            for source in state.records(&fk.collection) {
                if fk.columns.iter().any(|c| source.data[c].is_null()) {
                    continue;
                }
                let target = state
                    .records(&fk.target)
                    .into_iter()
                    .find(|r| {
                        fk.columns
                            .iter()
                            .zip(&fk.target_columns)
                            .all(|(a, b)| source.data[a] == r.data[b])
                    })
                    .ok_or_else(|| {
                        Error::new("CORRUPT_STORE", "SQLite relationship target missing")
                    })?;
                expected.insert((
                    fk.collection.clone(),
                    source.header.id.clone(),
                    ordinal as i64,
                    fk.target.clone(),
                    target.header.id.clone(),
                ));
            }
        }
        let mut query=self.connection.prepare("SELECT source_collection,source_id,relation,target_collection,target_id FROM relationships").map_err(sql_error)?;
        let actual: BTreeSet<_> = query
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })
            .map_err(sql_error)?
            .collect::<std::result::Result<_, _>>()
            .map_err(sql_error)?;
        if expected != actual {
            return Err(Error::new(
                "CORRUPT_STORE",
                "SQLite relationship index does not match objects",
            ));
        }
        Ok(())
    }
    fn generations(&self, sequence: u64) -> Result<Vec<String>> {
        let current = metadata(&self.connection, "generation")?;
        let number = parse_u64(&current)?;
        let mut query=self.connection.prepare("SELECT generation,base_seq,manifest,sha256 FROM checkpoints ORDER BY length(generation) DESC,generation DESC").map_err(sql_error)?;
        let mut rows = query.query([]).map_err(sql_error)?;
        let mut generations = Vec::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let generation: String = row.get(0).map_err(sql_error)?;
            let base: String = row.get(1).map_err(sql_error)?;
            let raw: String = row.get(2).map_err(sql_error)?;
            let digest: String = row.get(3).map_err(sql_error)?;
            let manifest: Value = serde_json::from_str(&raw)?;
            if parse_u64(&generation)? == 0
                || parse_u64(&generation)? > number
                || parse_u64(&base)? > sequence
                || manifest["format"] != "artex-sqlite-checkpoint"
                || manifest["format_version"] != 1
                || manifest["generation"] != generation
                || manifest["base_seq"] != base
                || hex::encode(hash(raw.as_bytes())) != digest
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "SQLite checkpoint metadata mismatch",
                ));
            }
            generations.push(generation);
        }
        if number == 0 && generations.is_empty() {
            generations.push(current);
        } else if generations.first() != Some(&current) {
            return Err(Error::new(
                "CORRUPT_STORE",
                "SQLite checkpoint generation missing",
            ));
        }
        Ok(generations)
    }
    pub(super) fn commit(
        &mut self,
        payload: &Payload,
        state: &Snapshot,
        tombstones: &BTreeMap<(String, String), Tombstone>,
        previous: u64,
        fault: Option<&str>,
    ) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if metadata(&tx, "commit_seq")? != previous.to_string() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "SQLite committed sequence changed",
            ));
        }
        inject(fault, "before_append")?;
        let mut changed_target_keys = BTreeSet::new();
        for mutation in &payload.mutations {
            let actual: Option<(String, String)> = tx
                .query_row(
                    "SELECT version,data FROM objects WHERE collection=?1 AND id=?2",
                    params![mutation.collection, mutation.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sql_error)?;
            if actual
                .as_ref()
                .map(|(version, _)| version.as_str())
                .unwrap_or("0")
                != mutation.expected_version
            {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "SQLite record version changed",
                ));
            }
            if let Some((_, raw)) = actual {
                let old: Value = serde_json::from_str(&raw)?;
                for fk in artex_core::foreign_keys()
                    .iter()
                    .filter(|fk| fk.target == mutation.collection)
                {
                    if mutation.record.as_ref().is_none_or(|record| {
                        fk.target_columns
                            .iter()
                            .any(|field| record.data[field] != old[field])
                    }) {
                        changed_target_keys.insert(fk.collection.clone());
                    }
                }
            }
            if mutation.op == "delete" {
                tx.execute(
                    "DELETE FROM objects WHERE collection=?1 AND id=?2",
                    params![mutation.collection, mutation.id],
                )
                .map_err(sql_error)?;
                insert_tombstone(
                    &tx,
                    &tombstones[&(mutation.collection.clone(), mutation.id.clone())],
                )?;
            } else {
                insert_record(&tx, mutation.record.as_ref().unwrap())?;
            }
        }
        for r in payload.events.iter().chain(&payload.deliveries) {
            insert_record(&tx, r)?;
        }
        insert_record(
            &tx,
            state.get("command_receipts", &payload.command_id).unwrap(),
        )?;
        for r in payload
            .mutations
            .iter()
            .filter_map(|m| m.record.as_ref())
            .chain(&payload.events)
            .chain(&payload.deliveries)
        {
            edges(&tx, state, r)?;
        }
        // A source may resolve to another header after a target-key swap. Even
        // a data field named `id` can be an opaque original key (traffic IDs),
        // so compare the actual key values rather than assuming header identity.
        for collection in changed_target_keys {
            for r in state.records(&collection) {
                edges(&tx, state, r)?;
            }
        }
        set_meta(&tx, "commit_seq", &state.commit_seq.to_string())?;
        inject(fault, "after_payload")?;
        tx.commit().map_err(|e| {
            let mut e = sql_error(e);
            e.recovery_required = e.code == "IO_FAILURE";
            e
        })?;
        if let Err(mut e) = inject(fault, "after_sync") {
            e.recovery_required = true;
            return Err(e);
        }
        Ok(())
    }
    pub(super) fn checkpoint(&mut self, sequence: u64) -> Result<(CheckpointTicket, Value)> {
        let generation = parse_u64(&metadata(&self.connection, "generation")?)?
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Generation exhausted"))?;
        let manifest = json!({"format":"artex-sqlite-checkpoint","format_version":1,"generation":generation.to_string(),"base_seq":sequence.to_string(),"created_at":now()});
        let bytes = canonical_bytes(&manifest)?;
        let digest = hex::encode(hash(&bytes));
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        tx.execute(
            "INSERT INTO checkpoints VALUES(?1,?2,?3,?4)",
            params![
                generation.to_string(),
                sequence.to_string(),
                String::from_utf8(bytes).unwrap(),
                digest
            ],
        )
        .map_err(sql_error)?;
        tx.execute("DELETE FROM checkpoints WHERE generation NOT IN (SELECT generation FROM checkpoints ORDER BY length(generation) DESC,generation DESC LIMIT 2)",[]).map_err(sql_error)?;
        set_meta(&tx, "generation", &generation.to_string())?;
        tx.commit().map_err(sql_error)?;
        let (busy, _, _): (i64, i64, i64) = self
            .connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(sql_error)?;
        if busy != 0 {
            return Err(Error::new(
                "BUSY",
                "SQLite checkpoint is blocked by an external reader",
            ));
        }
        Ok((
            CheckpointTicket {
                generation: generation.to_string(),
                base_seq: sequence.to_string(),
            },
            json!({"generation":generation.to_string(),"base_seq":sequence.to_string(),"manifest_sha256":digest}),
        ))
    }
}
impl Store {
    fn sqlite_state(
        path: &Path,
        lock: File,
        state: Snapshot,
        tombstones: BTreeMap<(String, String), Tombstone>,
        name: String,
        read_only: bool,
        database: Database,
    ) -> Result<Self> {
        let generation = parse_u64(&metadata(&database.connection, "generation")?)?;
        let valid_generations = database.generations(state.commit_seq)?;
        let mut store = Self {
            database: Some(database),
            sqlite_checkpoint: None,
            path: path.into(),
            _lock: Arc::new(lock),
            journal: None,
            journal_path: PathBuf::new(),
            state,
            tombstones,
            last_hash: [0; 32],
            generation,
            read_only,
            requested_read_only: read_only,
            corruption: None,
            valid_generations,
            name,
            checkpoint_job: None,
            read_cache: Mutex::new(None),
            read_pins: Mutex::new(Vec::new()),
            pending_import: None,
            #[cfg(test)]
            fault: None,
        };
        if let Err(error) = store.check_blobs(&serde_json::to_value(&store.state)?) {
            store.mark_corruption(json!({"kind":"state_or_blob","error":error.code}));
        }
        Ok(store)
    }
    pub(super) fn create_sqlite(path: &Path, name: &str) -> Result<Self> {
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
        for directory in ["store", "objects/blobs", "backups", "tmp"] {
            fs::create_dir_all(path.join(directory))?;
        }
        let project = json!({"format":"artex-workspace","format_version":2,"storage":"sqlite-v1","workspace_id":new_id("workspace"),"name":name,"created_at":now()});
        let state = Snapshot {
            workspace_id: project["workspace_id"].as_str().unwrap().into(),
            commit_seq: 0,
            collections: BTreeMap::new(),
        };
        Database::initialize(&path.join(DB_FILE), &state, &BTreeMap::new(), name, &[])?;
        // The project file is the final publication boundary; older apps reject
        // v2 instead of accidentally reopening an obsolete v1 journal.
        atomic_new(&path.join("project.json"), &serde_json::to_vec(&project)?)?;
        let database = Database::open(&path.join(DB_FILE), false)?;
        Self::sqlite_state(
            path,
            lock,
            state,
            BTreeMap::new(),
            name.into(),
            false,
            database,
        )
    }
    pub(super) fn open_sqlite(path: &Path, read_only: bool, project: Value) -> Result<Self> {
        if project["format"] != "artex-workspace"
            || project["storage"] != "sqlite-v1"
            || !project["workspace_id"]
                .as_str()
                .is_some_and(|id| valid_id(id, "workspace"))
        {
            return Err(Error::new(
                "VERSION_MISMATCH",
                "SQLite workspace descriptor is unsupported",
            ));
        }
        let lock = lock_workspace_mode(path, read_only)?;
        let database = Database::open(&path.join(DB_FILE), read_only)?;
        let (state, tombstones) = database.load(project["workspace_id"].as_str().unwrap())?;
        let name = metadata(&database.connection, "name")?;
        if project["name"] != name {
            return Err(Error::new(
                "CORRUPT_STORE",
                "SQLite workspace name mismatch",
            ));
        }
        Self::sqlite_state(path, lock, state, tombstones, name, read_only, database)
    }
    pub(super) fn migrate_to_sqlite(&mut self, mut project: Value) -> Result<()> {
        // Empty working directories may have been cleaned from an otherwise
        // valid v1 workspace. Recreate only required v2 directories after the
        // source has been validated and writable migration was requested.
        for name in ["tmp", "backups", "objects/blobs"] {
            let directory = self.path.join(name);
            reject_links(&directory)?;
            fs::create_dir_all(directory)?;
        }
        let destination = self.path.join(DB_FILE);
        reject_links(&destination)?;
        let mut roots = Vec::new();
        for prefix in fs::read_dir(self.path.join("objects/blobs"))? {
            let prefix = prefix?;
            reject_links(&prefix.path())?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(prefix.path())? {
                let file = file?;
                reject_links(&file.path())?;
                let name = file.file_name().to_string_lossy().into_owned();
                if file.file_type()?.is_file()
                    && name.len() == 64
                    && name.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    roots.push(name);
                }
            }
        }
        if !destination.exists() {
            let staging = self
                .path
                .join("tmp")
                .join(format!("{}.sqlite3", new_id("command")));
            Database::initialize(&staging, &self.state, &self.tombstones, &self.name, &roots)?;
            let check = Database::open(&staging, true)?;
            let (state, deleted) = check.load(&self.state.workspace_id)?;
            if !same_records(&state, &self.state)
                || serde_json::to_value(deleted.values().collect::<Vec<_>>())?
                    != serde_json::to_value(self.tombstones.values().collect::<Vec<_>>())?
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "SQLite migration verification failed",
                ));
            }
            drop(check);
            artex_platform::install_new(&staging, &destination)?;
        }
        // A crash may leave a fully installed DB before project publication.
        // Resume only if it matches the still-authoritative legacy generation.
        let check = Database::open(&destination, true)?;
        let (state, deleted) = check.load(&self.state.workspace_id)?;
        if !same_records(&state, &self.state)
            || serde_json::to_value(deleted.values().collect::<Vec<_>>())?
                != serde_json::to_value(self.tombstones.values().collect::<Vec<_>>())?
        {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Existing SQLite migration differs from legacy source",
            ));
        }
        drop(check);
        let backup = self.path.join("backups/pre-sqlite-project.json");
        reject_links(&backup)?;
        if !backup.exists() {
            atomic_new(&backup, &fs::read(self.path.join("project.json"))?)?;
        }
        project["format_version"] = json!(2);
        project["storage"] = json!("sqlite-v1");
        let control = self
            .path
            .join("tmp")
            .join(format!("{}.json", new_id("command")));
        write_new_sync(&control, &serde_json::to_vec(&project)?)?;
        artex_platform::replace_control_file(&control, &self.path.join("project.json"))?;
        self.database = Some(Database::open(&destination, false)?);
        self.journal = None;
        self.journal_path = PathBuf::new();
        self.read_only = false;
        self.requested_read_only = false;
        self.generation = 0;
        self.valid_generations = vec!["0".into()];
        Ok(())
    }
}

// Legacy checkpoints materialize empty collections while SQLite does not store
// empty rows. Compare every identity/header/value without requiring empty maps.
fn same_records(a: &Snapshot, b: &Snapshot) -> bool {
    a.workspace_id == b.workspace_id
        && a.commit_seq == b.commit_seq
        && a.collections.values().map(BTreeMap::len).sum::<usize>()
            == b.collections.values().map(BTreeMap::len).sum::<usize>()
        && a.collections
            .iter()
            .all(|(c, rows)| rows.iter().all(|(id, r)| b.get(c, id) == Some(r)))
}
impl Store {
    pub(super) fn sqlite_maintenance(&mut self, options: GcOptions) -> Result<GcReport> {
        if options.orphan_grace < std::time::Duration::from_secs(86400) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Orphan grace cannot be below 24 hours",
            ));
        }
        if self.sqlite_checkpoint.is_some() {
            return Err(Error::new(
                "BUSY",
                "Checkpoint must finish before maintenance",
            ));
        }
        let mut live = BTreeSet::new();
        maintenance::collect_hashes(&serde_json::to_value(&self.state)?, &mut live);
        let mut pins = self.read_pins.lock().unwrap();
        pins.retain(|p| p.strong_count() > 0);
        for pin in pins.iter().filter_map(|p| p.upgrade()) {
            maintenance::collect_hashes(&serde_json::to_value(&*pin.snapshot)?, &mut live);
        }
        drop(pins);
        let c = &self.database.as_ref().unwrap().connection;
        let mut query = c
            .prepare("SELECT sha256 FROM legacy_blob_roots")
            .map_err(sql_error)?;
        for sha in query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(sql_error)?
        {
            live.insert(sha.map_err(sql_error)?);
        }
        // Preserve every pre-migration blob so the untouched v1 backup remains
        // readable even after SQLite no longer references a historical object.
        let mut report = GcReport {
            retained_generations: self.valid_generations.clone(),
            plan_id: new_id("command"),
            ..Default::default()
        };
        for directory in fs::read_dir(self.path.join("objects/blobs"))? {
            let directory = directory?;
            reject_links(&directory.path())?;
            if !directory.file_type()?.is_dir() {
                continue;
            }
            for file in fs::read_dir(directory.path())? {
                let file = file?;
                reject_links(&file.path())?;
                let sha = file.file_name().to_string_lossy().into_owned();
                if !file.file_type()?.is_file()
                    || sha.len() != 64
                    || !sha.bytes().all(|b| b.is_ascii_hexdigit())
                    || live.contains(&sha)
                {
                    continue;
                }
                if options
                    .now
                    .duration_since(file.metadata()?.modified()?)
                    .unwrap_or_default()
                    < options.orphan_grace
                {
                    continue;
                }
                let reference = json!({"sha256":sha,"length":file.metadata()?.len().to_string(),"media_type":"application/octet-stream"});
                copy_blob_file(&self.path, &reference, &mut std::io::sink())?;
                report.removed_blobs.push(sha);
                if !options.dry_run {
                    match fs::remove_file(file.path()) {
                        Ok(()) => {
                            report.removed_count += 1;
                            sync_directory(file.path().parent().unwrap())?;
                        }
                        Err(error) => report
                            .failures
                            .push(json!({"kind":"blob","error":error.kind().to_string()})),
                    }
                }
            }
        }
        if !options.dry_run {
            atomic_new(
                &self
                    .path
                    .join(format!("backups/gc-{}.json", report.plan_id)),
                &serde_json::to_vec(&report)?,
            )?;
        }
        Ok(report)
    }
    pub(super) fn export_sqlite_recovery_copy(&self, destination: &Path) -> Result<Value> {
        reject_links(destination)?;
        let source = fs::canonicalize(&self.path)?;
        let parent = destination
            .parent()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Recovery parent missing"))?;
        let target = fs::canonicalize(parent)?.join(
            destination
                .file_name()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Recovery filename missing"))?,
        );
        if target.starts_with(&source) {
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
        validate_snapshot(&self.state)?;
        self.check_blobs(&serde_json::to_value(&self.state)?)?;
        let staging = parent.join(format!(".artex-recovery-{}", new_id("command")));
        let mut copy = Store::create(&staging, &format!("{} (recovery copy)", self.name))?;
        let mut live = BTreeSet::new();
        maintenance::collect_hashes(&serde_json::to_value(&self.state)?, &mut live);
        for sha in live {
            let path = self.path.join("objects/blobs").join(&sha[..2]).join(&sha);
            reject_links(&path)?;
            if !path.is_file() {
                continue;
            }
            let reference = json!({"sha256":sha,"length":path.metadata()?.len().to_string(),"media_type":"application/octet-stream"});
            copy_blob_file(&self.path, &reference, &mut std::io::sink())?;
            let imported = copy.blob_put_file(&path, "application/octet-stream")?;
            if imported["sha256"] != sha {
                return Err(Error::new("CORRUPT_STORE", "Recovery blob changed"));
            }
        }
        let mut state = self.state.clone();
        state.workspace_id = copy.state.workspace_id.clone();
        let db = copy.database.as_mut().unwrap();
        let tx = db
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_error)?;
        if metadata(&tx, "commit_seq")? != "0" {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Recovery destination is not empty",
            ));
        }
        for records in state.collections.values() {
            for record in records.values() {
                insert_record(&tx, record)?;
            }
        }
        for records in state.collections.values() {
            for record in records.values() {
                edges(&tx, &state, record)?;
            }
        }
        for tombstone in self.tombstones.values() {
            insert_tombstone(&tx, tombstone)?;
        }
        set_meta(&tx, "commit_seq", &state.commit_seq.to_string())?;
        tx.commit().map_err(sql_error)?;
        copy.state = state;
        copy.tombstones = self.tombstones.clone();
        copy.checkpoint()?;
        let report = json!({"format":"artex-recovery-copy","format_version":1,"source_workspace_id":self.state.workspace_id,"last_valid_seq":self.state.commit_seq.to_string(),"recovery":self.recovery(),"copy_workspace_id":copy.state.workspace_id,"source_unchanged":true});
        atomic_new(
            &staging.join("backups/recovery-report.json"),
            &serde_json::to_vec(&report)?,
        )?;
        drop(copy);
        // Publish only after full SQLite reopen and Blob validation succeeds.
        let verified = Store::open_read_only(&staging)?;
        if verified.read_only() && verified.corruption.is_some() {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Recovery copy verification failed",
            ));
        }
        drop(verified);
        if destination.exists() {
            fs::remove_dir(destination)?;
        }
        artex_platform::install_new(&staging, destination)?;
        Ok(report)
    }
}

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
