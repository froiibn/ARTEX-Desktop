use super::*;
use artex_core::{Change, CommandBatch, default_data, new_id};
use artex_store::Store;
use serde_json::json;
use std::{fs, io::Write, path::Path};
fn fixture() -> LegacyExport {
    let mut x = LegacyExport {
        source_instance: "fixture/source".into(),
        format_version: 1,
        ..Default::default()
    };
    x.tables.insert("companies".into(),vec![json!({"id":9007199254740993u64,"name":"한국어 회사","nkey":"한국어 회사","created_at":"2020-01-02T03:04:05Z","updated_at":"2021-01-02T03:04:05Z","extension_public":"preserved"})]);
    x.tables.insert("company_scope".into(),vec![json!({"id":2,"company_id":"9007199254740993","kind":"domain","domain":"example.test","raw":"example.test"})]);
    x
}
#[test]
fn migration_preserves_exact_ids_fk_times_extensions_and_retry_mapping() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("workspace");
    let mut s = Store::create(&p, "migrate").unwrap();
    let source = fixture();
    let report = import_into(
        &mut s,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(report.extensions.len(), 1);
    let snapshot = s.snapshot();
    let company = snapshot.records("companies")[0];
    let id = company.header.id.clone();
    assert_eq!(company.header.legacy_key, "9007199254740993");
    assert_eq!(company.header.created_at, "2020-01-02T03:04:05.000Z");
    assert_eq!(company.header.updated_at, "2021-01-02T03:04:05.000Z");
    assert_eq!(snapshot.records("company_scope")[0].data["company_id"], id);
    drop(s);
    let mut s = Store::open(&p).unwrap();
    import_into(
        &mut s,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    assert_eq!(s.snapshot().records("companies").len(), 1);
    assert_eq!(s.snapshot().records("companies")[0].header.id, id);
    assert_eq!(s.snapshot().records("companies")[0].header.version, "2");
}
#[test]
fn plaintext_credentials_never_reach_journal_and_unavailable_keys_disable_config() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("workspace");
    let mut s = Store::create(&p, "secrets").unwrap();
    let mut source = LegacyExport {
        source_instance: "credential/source".into(),
        format_version: 1,
        ..Default::default()
    };
    source.tables.insert(
        "llm_profiles".into(),
        vec![json!({"id":1,"name":"profile","api_key":"TOP_SECRET_LITERAL"})],
    );
    source.tables.insert("notification_channels".into(),vec![json!({"id":2,"name":"mail","kind":"email","enabled":true,"config":{"password":"TOP_SECRET_LITERAL","host":"smtp.example.test"}})]);
    let report = import_into(
        &mut s,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    assert_eq!(report.pending_secret_refs.len(), 2);
    assert_eq!(
        s.snapshot().records("notification_channels")[0].data["enabled"],
        false
    );
    for file in fs::read_dir(p.join("store/journal")).unwrap() {
        let bytes = fs::read(file.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("TOP_SECRET_LITERAL"));
    }
}
#[test]
fn traffic_sqlite_is_readonly_and_binary_body_roundtrips() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("traffic.sqlite");
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE exchanges(id TEXT PRIMARY KEY,ts INTEGER,host TEXT); CREATE TABLE exchange_bodies(id TEXT PRIMARY KEY,req_head TEXT NOT NULL DEFAULT '',req_body BLOB,req_blob TEXT,resp_head TEXT NOT NULL DEFAULT '',resp_body BLOB,resp_blob TEXT); CREATE TABLE blob_refs(hash TEXT,exchange_id TEXT,PRIMARY KEY(hash,exchange_id)); INSERT INTO exchanges VALUES('opaque',9223372036854775807,'example.test'); INSERT INTO exchange_bodies(id,req_body,req_head,resp_head) VALUES('opaque',X'0001FF','Req','Resp');").unwrap();
    }
    let before = fs::read(&db).unwrap();
    let export = read_traffic_sqlite(&db, t.path(), "traffic/source").unwrap();
    assert_eq!(fs::read(&db).unwrap(), before);
    let mut s = Store::create(&t.path().join("w"), "traffic").unwrap();
    import_into(
        &mut s,
        &export,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    let snap = s.snapshot();
    assert_eq!(
        snap.records("exchanges")[0].data["ts"],
        "9223372036854775807"
    );
    let reference = &snap.records("exchange_bodies")[0].data["req_body"];
    assert_eq!(s.blob_read(reference, 0, 100).unwrap(), vec![0, 1, 255]);
}
fn zip_fixture(path: &Path, entries: Vec<(&str, Vec<u8>)>) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        z.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(&bytes).unwrap();
    }
    z.finish().unwrap();
}
#[test]
fn portable_archive_restores_fk_blob_and_source_is_preserved() {
    let t = tempfile::tempdir().unwrap();
    let mut source = Store::create(&t.path().join("source"), "portable").unwrap();
    import_into(
        &mut source,
        &fixture(),
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    let reference = source
        .blob_put(&[0, 255, 128], "application/octet-stream")
        .unwrap();
    let mut data = default_data("exchange_bodies").unwrap();
    data["id"] = json!("opaque");
    data["req_body"] = reference;
    let mut exchange = default_data("exchanges").unwrap();
    exchange["id"] = json!("opaque");
    source
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "test.body".into(),
            normalized_params: json!({}),
            expected: vec![],
            changes: vec![
                Change::Put {
                    collection: "exchanges".into(),
                    id: new_id(artex_core::collection_prefix("exchanges").unwrap()),
                    data: exchange,
                },
                Change::Put {
                    collection: "exchange_bodies".into(),
                    id: new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),
                    data,
                },
            ],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    let archive = t.path().join("task.zip");
    export_archive(&source, &archive).unwrap();
    let before = fs::read(&archive).unwrap();
    let export = read_archive(&archive, "portable/source").unwrap();
    let dest = t.path().join("restored");
    let report = migrate_new(
        &dest,
        &export,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    assert!(report.complete);
    assert_eq!(fs::read(&archive).unwrap(), before);
    let restored = Store::open(&dest).unwrap();
    assert!(!restored.read_only());
    let snapshot = restored.snapshot();
    assert_eq!(
        snapshot.records("company_scope")[0].data["company_id"],
        snapshot.records("companies")[0].header.id
    );
    assert_eq!(
        restored
            .blob_read(
                &snapshot.records("exchange_bodies")[0].data["req_body"],
                0,
                100
            )
            .unwrap(),
        vec![0, 255, 128]
    );
}
#[test]
fn legacy_archive_versions_streamed_tables_and_traffic_import() {
    let t = tempfile::tempdir().unwrap();
    for version in 1..=3 {
        let path = t.path().join(format!("v{version}.tar.zst"));
        let encoder =
            zstd::stream::write::Encoder::new(fs::File::create(&path).unwrap(), 1).unwrap();
        let mut tar = tar::Builder::new(encoder);
        let manifest = json!({"format_version":version,"tables":{},"streamed_tables":{"companies":"tables/companies.ndjson"},"data_counts":{"companies":1}});
        for(name,bytes)in [("manifest.json",serde_json::to_vec(&manifest).unwrap()),("tables/companies.ndjson",serde_json::to_vec(&fixture().tables["companies"][0]).unwrap()),("traffic/traffic.json",serde_json::to_vec(&json!({"version":1,"exchanges":[{"id":"opaque","ts":1,"req_head":"","resp_head":"","req_body":"AAH/"}],"blobs":[]})).unwrap())]{let mut h=tar::Header::new_gnu();h.set_size(bytes.len() as u64);h.set_mode(0o600);h.set_cksum();tar.append_data(&mut h,name,bytes.as_slice()).unwrap();}
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        let x = read_archive(&path, "archive/source").unwrap();
        assert_eq!(x.tables["companies"].len(), 1);
        assert_eq!(
            x.tables["exchange_bodies"][0]["req_body"]["legacy_bytes"],
            "AAH/"
        );
    }
}
#[test]
fn archive_rejects_traversal_duplicate_case_and_checksum_corruption() {
    let t = tempfile::tempdir().unwrap();
    for (name, entries) in [
        ("traversal", vec![("../outside", vec![])]),
        (
            "duplicate",
            vec![("manifest.json", vec![]), ("MANIFEST.JSON", vec![])],
        ),
    ] {
        let p = t.path().join(name);
        zip_fixture(&p, entries);
        assert_eq!(read_archive(&p, "source").unwrap_err().code, "PARSE_ERROR");
    }
    let p = t.path().join("checksum");
    zip_fixture(&p,vec![("manifest.json",serde_json::to_vec(&json!({"format":"artex-task/1","tables":[{"collection":"companies","path":"companies.ndjson","sha256":"wrong","count":0}],"blobs":[]})).unwrap()),("companies.ndjson",vec![])]);
    assert_eq!(read_archive(&p, "source").unwrap_err().code, "PARSE_ERROR");
}
#[test]
fn new_workspace_is_only_published_after_all_bounded_fk_batches() {
    let t = tempfile::tempdir().unwrap();
    let source = fixture();
    let destination = t.path().join("published");
    let report = migrate_new(
        &destination,
        &source,
        &mut PendingSecrets,
        LegacyOptions {
            max_batch_bytes: 1200,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.command_ids.len(), 2);
    let s = Store::open(&destination).unwrap();
    assert!(!s.read_only());
    assert_eq!(s.snapshot().records("companies").len(), 1);
    assert!(destination.join("backups/migration-report.json").exists());
}
#[test]
fn failed_import_has_no_dangling_committed_objects_or_plaintext() {
    let t = tempfile::tempdir().unwrap();
    let mut s = Store::create(&t.path().join("w"), "failure").unwrap();
    let mut source = fixture();
    source.tables.get_mut("company_scope").unwrap()[0]["company_id"] = json!(999);
    assert!(
        import_into(
            &mut s,
            &source,
            &mut PendingSecrets,
            LegacyOptions::default()
        )
        .is_err()
    );
    assert!(s.snapshot().records("companies").is_empty());
}
#[test]
fn bounded_session_yields_between_batches_and_reuses_durable_ids_after_restart() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("w");
    let mut store = Store::create(&path, "stepped").unwrap();
    let source = fixture();
    let options = LegacyOptions {
        max_batch_bytes: 1200,
        ..Default::default()
    };
    let mut session =
        ImportSession::prepare(&mut store, &source, &mut PendingSecrets, options.clone()).unwrap();
    assert!(!session.step(&mut store).unwrap());
    assert_eq!(store.snapshot().commit_seq, 1);
    let first_id = store.snapshot().records("companies")[0].header.id.clone();
    assert_eq!(store.snapshot().records("company_scope").len(), 0);
    drop(session);
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let mut resumed =
        ImportSession::prepare(&mut store, &source, &mut PendingSecrets, options).unwrap();
    while !resumed.step(&mut store).unwrap() {}
    assert!(resumed.report().complete);
    assert_eq!(store.snapshot().records("companies")[0].header.id, first_id);
    assert_eq!(
        store.snapshot().records("company_scope")[0].data["company_id"],
        first_id
    );
}
#[test]
fn background_preparation_detects_a_writer_change_before_overwriting_a_target() {
    let t = tempfile::tempdir().unwrap();
    let mut store = Store::create(&t.path().join("w"), "CAS").unwrap();
    let source = fixture();
    import_into(
        &mut store,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    let mut view = store.read_view();
    let mut session = ImportSession::prepare(
        &mut view,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    let row = store.snapshot().records("companies")[0].clone();
    let mut data = row.data.clone();
    data["name"] = json!("concurrent update");
    store
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.update".into(),
            normalized_params: json!({}),
            expected: vec![artex_core::Expected {
                collection: "companies".into(),
                id: row.header.id.clone(),
                version: row.header.version,
            }],
            changes: vec![Change::Put {
                collection: "companies".into(),
                id: row.header.id.clone(),
                data,
            }],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    assert_eq!(
        session.step(&mut store).unwrap_err().code,
        "VERSION_CONFLICT"
    );
    assert_eq!(
        store
            .snapshot()
            .get("companies", &row.header.id)
            .unwrap()
            .data["name"],
        "concurrent update"
    );
}
#[test]
fn archive_cancel_does_not_publish_a_destination() {
    let t = tempfile::tempdir().unwrap();
    let mut store = Store::create(&t.path().join("w"), "cancel").unwrap();
    let source = fixture();
    import_into(
        &mut store,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    let tid = new_id("task");
    let eid = new_id(artex_core::collection_prefix("explorations").unwrap());
    let mut task = default_data("tasks").unwrap();
    task["id"] = json!(tid);
    task["exploration_id"] = json!(eid);
    let mut exp = default_data("explorations").unwrap();
    exp["id"] = json!(eid);
    store
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.task".into(),
            normalized_params: json!({}),
            expected: vec![],
            changes: vec![
                Change::Put {
                    collection: "explorations".into(),
                    id: eid,
                    data: exp,
                },
                Change::Put {
                    collection: "tasks".into(),
                    id: tid.clone(),
                    data: task,
                },
            ],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    // Cancellation of a real task export cannot publish a destination or alter
    // the committed graph.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let destination = t.path().join("cancel.zip");
    let before = store.snapshot();
    assert!(export_task_archive_cancellable(&store, &tid, &destination, cancel).is_err());
    assert!(!destination.exists());
    assert_eq!(before.collections, store.snapshot().collections);
}
#[test]
fn restored_secret_refs_are_enabled_only_when_native_vault_confirms_availability() {
    struct Available;
    impl SecretSink for Available {
        fn import_secret(&mut self, _: &str, _: &str) -> artex_core::Result<SecretOutcome> {
            panic!("reference import must not transfer plaintext")
        }
        fn reference_available(&mut self, _: &serde_json::Value) -> artex_core::Result<bool> {
            Ok(true)
        }
    }
    let t = tempfile::tempdir().unwrap();
    let mut source = LegacyExport {
        source_instance: "ref/source".into(),
        format_version: 1,
        ..Default::default()
    };
    source.tables.insert("notification_channels".into(),vec![json!({"id":1,"name":"mail","kind":"email","enabled":true,"config":{"password":{"secret_ref":"secret_00000000000000000000000000000001","hint":"configured"}}})]);
    for available in [true, false] {
        let mut store = Store::create(&t.path().join(available.to_string()), "reference").unwrap();
        let sink: &mut dyn SecretSink = if available {
            &mut Available
        } else {
            &mut PendingSecrets
        };
        let report = import_into(&mut store, &source, sink, LegacyOptions::default()).unwrap();
        assert_eq!(
            store.snapshot().records("notification_channels")[0].data["enabled"],
            available
        );
        assert_eq!(report.pending_secret_refs.is_empty(), available);
    }
}

#[derive(Default)]
struct HeaderVaultFixture {
    values: std::collections::BTreeMap<String, String>,
}
impl SecretSink for HeaderVaultFixture {
    fn import_secret(&mut self, key: &str, plaintext: &str) -> artex_core::Result<SecretOutcome> {
        use sha2::{Digest, Sha256};
        let id = format!(
            "secret_{}",
            &hex::encode(Sha256::digest(key.as_bytes()))[..32]
        );
        self.values.insert(id.clone(), plaintext.into());
        Ok(SecretOutcome {
            reference: json!({"secret_ref":id,"hint":"fixture configured"}),
            available: true,
        })
    }
    fn reference_available(&mut self, value: &serde_json::Value) -> artex_core::Result<bool> {
        Ok(value["secret_ref"]
            .as_str()
            .is_some_and(|id| self.values.contains_key(id)))
    }
}
fn assert_workspace_does_not_contain(path: &Path, needles: &[&str]) {
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            assert_workspace_does_not_contain(&entry.path(), needles);
        } else {
            let bytes = fs::read(entry.path()).unwrap();
            for needle in needles {
                assert!(
                    !bytes
                        .windows(needle.len())
                        .any(|part| part == needle.as_bytes()),
                    "Plain credential persisted in {}",
                    entry.path().display()
                );
            }
        }
    }
}
#[test]
fn legacy_http_custom_headers_are_one_secretref_through_checkpoint_reopen_and_retry() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("workspace");
    let mut store = Store::create(&path, "header migration").unwrap();
    let mut vault = HeaderVaultFixture::default();
    let headers = json!({"Authorization":"Bearer migration-auth-private","Proxy-Authorization":"Basic migration-proxy-private","Cookie":"session=migration-cookie-private","Set-Cookie":"migration-response-private","X-API-Key":"migration-apikey-private","API-Key":"migration-api-private","X-Auth-Token":"migration-token-private","Accept":"application/json","X-Public":"kept public header"});
    let mut source = LegacyExport {
        source_instance: "custom/header/source".into(),
        format_version: 1,
        ..Default::default()
    };
    source.tables.insert("tools".into(),vec![json!({"key":"legacy_http","kind":"http","system":false,"enabled":true,"deferred":true,"agents":["mainagent"],"exec":{"url":"http://localhost/fixture","headers":headers,"method":"GET"},"schema":{"type":"object"}})]);
    source.tables.insert(
        "exchanges".into(),
        vec![json!({"id":"opaque-traffic","host":"fixture.test"})],
    );
    source.tables.insert("exchange_bodies".into(),vec![json!({"id":"opaque-traffic","req_head":"Authorization: opaque traffic evidence is preserved","resp_head":"Set-Cookie: opaque evidence"})]);
    let original = serde_json::to_vec(&source).unwrap();
    let report = import_into(&mut store, &source, &mut vault, LegacyOptions::default()).unwrap();
    assert!(report.complete);
    assert!(report.pending_secret_refs.is_empty());
    let s = store.snapshot();
    let tool = s.records("tools")[0];
    let id = tool.header.id.clone();
    let reference = tool.data["exec"]["headers"].clone();
    assert!(reference["secret_ref"].is_string());
    assert_eq!(tool.data["enabled"], true);
    let secret = reference["secret_ref"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&vault.values[secret]).unwrap(),
        headers
    );
    assert_eq!(
        s.records("exchange_bodies")[0].data["req_head"],
        artex_core::body("Authorization: opaque traffic evidence is preserved")
    );
    store.checkpoint().unwrap();
    store.checkpoint().unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert!(!store.read_only());
    assert_eq!(
        store.snapshot().get("tools", &id).unwrap().data["exec"]["headers"],
        reference
    );
    import_into(&mut store, &source, &mut vault, LegacyOptions::default()).unwrap();
    assert_eq!(store.snapshot().records("tools").len(), 1);
    assert_eq!(vault.values.len(), 1);
    assert_eq!(
        store.snapshot().get("tools", &id).unwrap().data["exec"]["headers"],
        reference
    );
    assert_eq!(
        serde_json::to_vec(&source).unwrap(),
        original,
        "Legacy source is preserved byte-for-byte"
    );
    drop(store); // Release the native Windows byte-range writer lock before scanning.
    assert_workspace_does_not_contain(
        &path,
        &[
            "migration-auth-private",
            "migration-proxy-private",
            "migration-cookie-private",
            "migration-response-private",
            "migration-apikey-private",
            "migration-api-private",
            "migration-token-private",
        ],
    );
}
#[test]
fn http_sse_mcp_headers_and_mixed_portable_refs_normalize_without_plaintext() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("workspace");
    let mut store = Store::create(&path, "mixed headers").unwrap();
    let mut vault = HeaderVaultFixture::default();
    let existing = vault
        .import_secret("existing/auth", "portable-existing-private")
        .unwrap()
        .reference;
    let mut source = LegacyExport {
        source_instance: "mixed/header/source".into(),
        format_version: 1,
        ..Default::default()
    };
    source.tables.insert("tools".into(),vec![json!({"key":"portable_http","enabled":true,"kind":"http","exec":{"headers":{"Authorization":existing,"Cookie":"mixed-cookie-private","X-API-Key":"mixed-apikey-private","Accept":"application/json"}}})]);
    source.tables.insert("mcp_servers".into(),vec![json!({"id":1,"name":"http","transport":"http","url":"http://localhost/fixture","enabled":true,"env":{"Cookie":"mcp-http-private","API-Key":"mcp-key-private","Accept":"application/json"}}),json!({"id":2,"name":"sse","transport":"sse","url":"http://localhost/fixture","enabled":true,"env":{"Authorization":existing,"Set-Cookie":"mcp-sse-private","X-Public":"public"}}),json!({"id":3,"name":"stdio","transport":"stdio","command":"fixture-not-executed","enabled":true,"env":{"PATH":"/usr/bin","Cookie":"ordinary process environment value"}})]);
    let report = import_into(&mut store, &source, &mut vault, LegacyOptions::default()).unwrap();
    assert!(report.pending_secret_refs.is_empty());
    let snapshot = store.snapshot();
    let headers = &snapshot.records("tools")[0].data["exec"]["headers"];
    assert_eq!(headers["Authorization"], existing);
    assert_eq!(headers["Accept"], "application/json");
    for key in ["Cookie", "X-API-Key"] {
        assert!(headers[key]["secret_ref"].is_string());
    }
    let http = snapshot
        .records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == "http")
        .unwrap();
    assert!(http.data["env"]["secret_ref"].is_string());
    let sse = snapshot
        .records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == "sse")
        .unwrap();
    assert_eq!(sse.data["env"]["Authorization"], existing);
    assert!(sse.data["env"]["Set-Cookie"]["secret_ref"].is_string());
    assert_eq!(sse.data["env"]["X-Public"], "public");
    let stdio = snapshot
        .records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == "stdio")
        .unwrap();
    assert_eq!(
        stdio.data["env"]["Cookie"],
        "ordinary process environment value"
    );
    assert_eq!(stdio.data["env"]["PATH"], "/usr/bin");
    store.checkpoint().unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(!store.read_only());
    assert_eq!(store.snapshot().records("tools")[0].data["enabled"], true);
    drop(store);
    assert_workspace_does_not_contain(
        &path,
        &[
            "portable-existing-private",
            "mixed-cookie-private",
            "mixed-apikey-private",
            "mcp-http-private",
            "mcp-key-private",
            "mcp-sse-private",
        ],
    );
}
#[test]
fn pending_legacy_custom_header_secret_disables_tool_and_report_never_contains_plaintext() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("workspace");
    let mut store = Store::create(&path, "pending headers").unwrap();
    let mut source = LegacyExport {
        source_instance: "pending/header/source".into(),
        format_version: 1,
        ..Default::default()
    };
    source.tables.insert("tools".into(),vec![json!({"key":"pending_http","kind":"http","enabled":true,"exec":{"headers":{"Cookie":"pending-cookie-private","X-API-Key":"pending-apikey-private"}}})]);
    let report = import_into(
        &mut store,
        &source,
        &mut PendingSecrets,
        LegacyOptions::default(),
    )
    .unwrap();
    assert_eq!(report.pending_secret_refs.len(), 1);
    assert_eq!(store.snapshot().records("tools")[0].data["enabled"], false);
    assert!(store.snapshot().records("tools")[0].data["exec"]["headers"]["secret_ref"].is_string());
    store.checkpoint().unwrap();
    drop(store);
    assert_workspace_does_not_contain(&path, &["pending-cookie-private", "pending-apikey-private"]);
}
