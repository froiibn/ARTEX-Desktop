//! SQLite acceptance paths use private workspaces and actual OS processes.
use super::*;
use std::time::{Duration, Instant, SystemTime};
fn command(store: &Store, id: &str, name: &str) -> CommandBatch {
    let mut data = artex_core::default_data("companies").unwrap();
    data["name"] = json!(name);
    let expected = store
        .state
        .get("companies", id)
        .map(|r| {
            vec![artex_core::Expected {
                collection: "companies".into(),
                id: id.into(),
                version: r.header.version.clone(),
            }]
        })
        .unwrap_or_default();
    CommandBatch {
        command_id: new_id("command"),
        method: "fixture.company".into(),
        normalized_params: json!({"name":name}),
        expected,
        changes: vec![Change::Put {
            collection: "companies".into(),
            id: id.into(),
            data,
        }],
        events: vec![],
        deliveries: vec![],
        result: json!({"id":id}),
    }
}
fn fingerprints(root: &Path) -> BTreeMap<PathBuf, String> {
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                let relative = entry.path().strip_prefix(root).unwrap().to_owned();
                if relative == Path::new("locks/writer.lock") {
                    continue;
                }
                out.insert(
                    relative,
                    hex::encode(hash(&fs::read(entry.path()).unwrap())),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
#[test]
fn sqlite_is_authoritative_and_cas_receipt_survives_readonly_reopen_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut store = Store::create(&path, "SQLite fixture").unwrap();
    assert!(
        rusqlite::version_number() >= 3_051_003,
        "Bundle must include the WAL-reset fix"
    );
    assert_eq!(store.info()["format_version"], 2);
    assert!(!path.join("store/journal").exists());
    assert!(!path.join("store/snapshots").exists());
    let id = new_id("company");
    let first = command(&store, &id, "before");
    let receipt = store.commit(first.clone()).unwrap();
    let stale = command(&store, &id, "stale");
    let next = command(&store, &id, "after");
    store.commit(next).unwrap();
    assert_eq!(store.commit(stale).unwrap_err().code, "VERSION_CONFLICT");
    assert_eq!(store.commit(first.clone()).unwrap(), receipt);
    let expected = store.snapshot();
    drop(store);
    let owner = fs::read(path.join("locks/writer.lock")).unwrap();
    let before = fingerprints(&path);
    let readonly = Store::open_read_only(&path).unwrap();
    assert!(readonly.read_only());
    assert_eq!(readonly.snapshot(), expected);
    assert_eq!(
        fingerprints(&path),
        before,
        "Read-only open must not create SQLite sidecars"
    );
    drop(readonly);
    assert_eq!(fingerprints(&path), before);
    assert_eq!(fs::read(path.join("locks/writer.lock")).unwrap(), owner);
    let mut writable = Store::open(&path).unwrap();
    assert_eq!(writable.commit(first).unwrap(), receipt);
    assert_eq!(
        &fs::read(path.join(DB_FILE)).unwrap()[..16],
        b"SQLite format 3\0"
    );
}
#[test]
fn transaction_failure_is_atomic_and_confirmed_receipt_recovers_after_ack_loss() {
    for stage in ["after_payload", "after_sync"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "atomic").unwrap();
        let id = new_id("company");
        store.commit(command(&store, &id, "prior")).unwrap();
        let next = command(&store, &id, "next");
        store.fault = Some(stage.into());
        assert_eq!(store.commit(next.clone()).unwrap_err().code, "IO_FAILURE");
        drop(store);
        let mut reopened = Store::open(&path).unwrap();
        let committed = stage == "after_sync";
        assert_eq!(reopened.state.commit_seq, if committed { 2 } else { 1 });
        assert_eq!(
            reopened.state.get("companies", &id).unwrap().data["name"],
            if committed { "next" } else { "prior" }
        );
        assert_eq!(reopened.commit(next).unwrap().commit_seq, "2");
        assert_eq!(reopened.state.records("command_receipts").len(), 2);
    }
}
#[test]
fn legacy_conversion_preserves_original_files_versions_receipts_and_blob_backup_roots() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut old = Store::create_legacy(&path, "legacy").unwrap();
    let id = new_id("company");
    let first = command(&old, &id, "original");
    old.commit(first.clone()).unwrap();
    let orphan = old
        .blob_put(b"old unreferenced evidence", "text/plain")
        .unwrap();
    old.checkpoint().unwrap();
    let state = old.snapshot();
    drop(old);
    let before = fingerprints(&path);
    let readonly = Store::open_read_only(&path).unwrap();
    assert_eq!(readonly.info()["format_version"], 1);
    drop(readonly);
    assert_eq!(fingerprints(&path), before);
    let mut converted = Store::open(&path).unwrap();
    assert_eq!(converted.info()["format_version"], 2);
    assert!(same_records(&converted.snapshot(), &state));
    assert_eq!(converted.commit(first).unwrap().commit_seq, "1");
    for (file, sha) in &before {
        if file != Path::new("project.json") {
            assert_eq!(
                hex::encode(hash(&fs::read(path.join(file)).unwrap())),
                *sha,
                "Legacy source changed: {file:?}"
            );
        }
    }
    assert_eq!(
        hex::encode(hash(
            &fs::read(path.join("backups/pre-sqlite-project.json")).unwrap()
        )),
        before[Path::new("project.json")]
    );
    let report = converted
        .maintenance(GcOptions {
            now: SystemTime::now() + Duration::from_secs(86401),
            ..Default::default()
        })
        .unwrap();
    assert!(
        !report
            .removed_blobs
            .contains(&orphan["sha256"].as_str().unwrap().to_owned())
    );
    drop(converted);
    let reopened = Store::open(&path).unwrap();
    assert!(same_records(&reopened.snapshot(), &state));
    assert_eq!(
        reopened.blob_read(&orphan, 0, 65536).unwrap(),
        b"old unreferenced evidence"
    );
}
#[test]
fn interrupted_migration_publication_resumes_only_for_matching_legacy_generation() {
    for mismatch in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut old = Store::create_legacy(&path, "source").unwrap();
        let id = new_id("company");
        old.commit(command(&old, &id, "original")).unwrap();
        let mut state = old.snapshot();
        if mismatch {
            state
                .collections
                .get_mut("companies")
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .data["name"] = json!("not the source");
        }
        Database::initialize(&path.join(DB_FILE), &state, &old.tombstones, &old.name, &[]).unwrap();
        drop(old);
        let before = fingerprints(&path);
        if mismatch {
            assert_eq!(Store::open(&path).err().unwrap().code, "RECOVERY_REQUIRED");
            assert_eq!(fingerprints(&path), before);
        } else {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.info()["format_version"], 2);
            assert!(same_records(&store.snapshot(), &state));
        }
    }
}
#[test]
fn corrupt_legacy_tail_never_migrates_or_changes_source_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut old = Store::create_legacy(&path, "source").unwrap();
    let id = new_id("company");
    old.commit(command(&old, &id, "original")).unwrap();
    let journal = old.journal_path.clone();
    drop(old);
    OpenOptions::new()
        .append(true)
        .open(journal)
        .unwrap()
        .write_all(b"incomplete")
        .unwrap();
    let before = fingerprints(&path);
    let store = Store::open(&path).unwrap();
    assert!(store.read_only());
    assert_eq!(store.info()["format_version"], 1);
    assert!(!path.join(DB_FILE).exists());
    assert_eq!(fingerprints(&path), before);
}
#[test]
fn sql_foreign_keys_and_tombstones_remain_enforced_after_restore() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut store = Store::create(&path, "FK").unwrap();
    let id = new_id("company");
    store.commit(command(&store, &id, "owner")).unwrap();
    let c = &store.database.as_ref().unwrap().connection;
    assert!(
        c.execute(
            "INSERT INTO relationships VALUES('companies',?1,99999,'companies','company_missing')",
            [&id]
        )
        .is_err()
    );
    let mut deleted = command(&store, &id, "unused");
    deleted.changes = vec![Change::Delete {
        collection: "companies".into(),
        id: id.clone(),
    }];
    store.commit(deleted).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .commit(command(&store, &id, "resurrection"))
            .unwrap_err()
            .code,
        "VERSION_CONFLICT"
    );
}
#[test]
fn checkpoint_digest_matches_durable_sql_metadata_and_pinned_snapshot_survives_writes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut store = Store::create(&path, "checkpoint").unwrap();
    let id = new_id("company");
    store.commit(command(&store, &id, "pin")).unwrap();
    let pinned = store.read_generation();
    let ticket = store.begin_checkpoint().unwrap();
    store.commit(command(&store, &id, "after")).unwrap();
    let result = store.wait_checkpoint().unwrap();
    assert_eq!(ticket.base_seq, "1");
    assert_eq!(result["base_seq"], "1");
    assert_eq!(pinned.get("companies", &id).unwrap().data["name"], "pin");
    let manifest: String = store
        .database
        .as_ref()
        .unwrap()
        .connection
        .query_row(
            "SELECT manifest FROM checkpoints WHERE generation=?1",
            [ticket.generation],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        hex::encode(hash(manifest.as_bytes())),
        result["manifest_sha256"]
    );
    assert_eq!(store.state.commit_seq, 2);
    drop(pinned);
    drop(store);
    assert_eq!(Store::open(&path).unwrap().state.commit_seq, 2);
}
#[test]
fn crash_fixture_child() {
    let Some(root) = std::env::var_os("ARTEX_SQLITE_CRASH_FIXTURE") else {
        return;
    };
    let phase = std::env::var("ARTEX_SQLITE_CRASH_PHASE").unwrap();
    let root = PathBuf::from(root);
    let path = root.join("workspace");
    let mut store = Store::create(&path, "process crash").unwrap();
    let id = new_id("company");
    store.commit(command(&store, &id, "durable")).unwrap();
    if phase == "uncommitted" {
        store
            .database
            .as_ref()
            .unwrap()
            .connection
            .execute_batch(
                "BEGIN IMMEDIATE; UPDATE metadata SET value='999' WHERE key='commit_seq';",
            )
            .unwrap();
    }
    fs::write(root.join("ready"), id).unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn actual_process_termination_recovers_wal_and_readonly_preserves_wal_bytes() {
    for phase in ["committed", "uncommitted"] {
        let temp = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "sqlite::tests::crash_fixture_child",
                "--nocapture",
            ])
            .env("ARTEX_SQLITE_CRASH_FIXTURE", temp.path())
            .env("ARTEX_SQLITE_CRASH_PHASE", phase)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !temp.path().join("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("Crash fixture exited before ready: {status}");
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Crash fixture deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let path = temp.path().join("workspace");
        let before = fingerprints(&path);
        let readonly = Store::open_read_only(&path).unwrap();
        assert_eq!(readonly.state.commit_seq, 1);
        assert_eq!(
            readonly.state.records("companies")[0].data["name"],
            "durable"
        );
        drop(readonly);
        assert_eq!(fingerprints(&path), before);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.state.commit_seq, 1);
        assert_eq!(reopened.state.records("command_receipts").len(), 1);
    }
}

#[test]
fn corrupted_checkpoint_metadata_and_relation_indexes_are_rejected_on_reopen() {
    for kind in ["checkpoint", "relationship"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace");
        let mut store = Store::create(&path, "tamper fixture").unwrap();
        let id = new_id("company");
        store.commit(command(&store, &id, "owner")).unwrap();
        store.checkpoint().unwrap();
        let sql = if kind == "checkpoint" {
            "UPDATE checkpoints SET sha256='invalid'"
        } else {
            // Both endpoints exist: SQLite FK checks alone cannot detect that
            // an extra row is not a relationship declared by the object model.
            "INSERT INTO relationships SELECT collection,id,99999,collection,id FROM objects WHERE collection='companies'"
        };
        store
            .database
            .as_ref()
            .unwrap()
            .connection
            .execute(sql, [])
            .unwrap();
        drop(store);
        assert_eq!(
            Store::open_read_only(&path).err().unwrap().code,
            "CORRUPT_STORE"
        );
    }
}

#[test]
fn legacy_conversion_recreates_missing_empty_work_directories() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let store = Store::create_legacy(&path, "empty directories").unwrap();
    drop(store);
    for directory in ["tmp", "backups", "objects/blobs"] {
        fs::remove_dir(path.join(directory)).unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.info()["format_version"], 2);
    assert!(path.join(DB_FILE).is_file());
    assert!(path.join("backups/pre-sqlite-project.json").is_file());
}

#[test]
fn opaque_original_id_key_swap_rebinds_unchanged_source_relationship() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("workspace");
    let mut store = Store::create(&path, "opaque keys").unwrap();
    let first = new_id("traffic");
    let second = new_id("traffic");
    let body = new_id(artex_core::collection_prefix("exchange_bodies").unwrap());
    let mut a = artex_core::default_data("exchanges").unwrap();
    a["id"] = json!("first/opaque");
    let mut b = a.clone();
    b["id"] = json!("second/opaque");
    let mut source = artex_core::default_data("exchange_bodies").unwrap();
    source["id"] = json!("first/opaque");
    let mut batch = command(&store, &new_id("company"), "fixture");
    batch.changes = vec![
        Change::Put {
            collection: "exchanges".into(),
            id: first.clone(),
            data: a.clone(),
        },
        Change::Put {
            collection: "exchanges".into(),
            id: second.clone(),
            data: b.clone(),
        },
        Change::Put {
            collection: "exchange_bodies".into(),
            id: body.clone(),
            data: source,
        },
    ];
    store.commit(batch).unwrap();
    a["id"] = json!("second/opaque");
    b["id"] = json!("first/opaque");
    let mut swap = command(&store, &new_id("company"), "swap");
    swap.expected = vec![
        artex_core::Expected {
            collection: "exchanges".into(),
            id: first.clone(),
            version: "1".into(),
        },
        artex_core::Expected {
            collection: "exchanges".into(),
            id: second.clone(),
            version: "1".into(),
        },
    ];
    swap.changes = vec![
        Change::Put {
            collection: "exchanges".into(),
            id: first,
            data: a,
        },
        Change::Put {
            collection: "exchanges".into(),
            id: second.clone(),
            data: b,
        },
    ];
    store.commit(swap).unwrap();
    let target:String=store.database.as_ref().unwrap().connection.query_row("SELECT target_id FROM relationships WHERE source_collection='exchange_bodies' AND source_id=?1",[body],|r|r.get(0)).unwrap();
    assert_eq!(target, second);
    drop(store);
    assert_eq!(Store::open_read_only(&path).unwrap().state.commit_seq, 2);
}
