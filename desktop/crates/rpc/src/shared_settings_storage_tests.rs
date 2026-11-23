//! Storage boundary tests use unexecuted fixture configuration and no secrets.
use super::*;

#[test]
fn shared_catalog_larger_than_a_journal_frame_reopens_with_all_originals_and_verified_blob_roots() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let mut source =
        Store::create(&dir.path().join("source"), "Large configuration fixture").unwrap();
    // Each script fits an ordinary workspace command. Their current catalog plus
    // preserved import exceeds the Store's existing frame budget without truncation.
    for index in 0..2 {
        let id = new_id("toolconfig");
        let mut data = default_data("tools").unwrap();
        data["key"] = json!(format!("unexecuted_fixture_{index}"));
        data["exec"] = json!({"code":"# fixture only\n".repeat(400_000)});
        source
            .commit(CommandBatch {
                command_id: new_id("command"),
                method: "fixture.config".into(),
                normalized_params: json!({}),
                expected: vec![],
                changes: vec![Change::Put {
                    collection: "tools".into(),
                    id,
                    data,
                }],
                events: vec![],
                deliveries: vec![],
                result: json!({}),
            })
            .unwrap();
    }
    let preferences = Preferences::from_snapshot(&source.snapshot()).unwrap();
    let mut catalog = Catalog::default();
    catalog.current = preferences.clone();
    catalog
        .imports
        .insert(source.snapshot().workspace_id.clone(), vec![preferences]);
    assert!(serde_json::to_vec(&catalog).unwrap().len() > 16_777_216);
    let mut shared = SharedSettings::open(&profile).unwrap();
    shared.persist(catalog).unwrap();
    let snapshot = shared.store.snapshot();
    let row = snapshot
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == CATALOG_KEY)
        .unwrap();
    assert!(row.data["value"].as_str().unwrap().len() < 1024);
    let receipt = snapshot
        .records("command_receipts")
        .into_iter()
        .last()
        .unwrap();
    assert!(receipt.data["normalized_params"]["catalog_blob"]["sha256"].is_string());
    drop(shared);
    let reopened = SharedSettings::open(&profile).unwrap();
    assert_eq!(
        reopened
            .catalog
            .current
            .configuration
            .as_ref()
            .unwrap()
            .tools
            .len(),
        2
    );
    assert_eq!(reopened.catalog.imports.values().next().unwrap().len(), 1);
    assert_eq!(
        reopened.catalog.current.configuration,
        catalog_configuration(&source)
    );
}

fn catalog_configuration(source: &Store) -> Option<SharedConfiguration> {
    Some(SharedConfiguration::from_snapshot(&source.snapshot()).unwrap())
}

#[test]
fn legacy_inline_catalog_upgrades_and_corrupt_catalog_blob_never_falls_back_to_empty() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let mut shared = SharedSettings::open(&profile).unwrap();
    let mut catalog = Catalog::default();
    catalog
        .current
        .settings
        .insert("workers".into(), "3".into());
    let mut row = default_data("settings").unwrap();
    row["key"] = json!(CATALOG_KEY);
    row["value"] = json!(serde_json::to_string(&catalog).unwrap());
    shared
        .store
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.inline".into(),
            normalized_params: json!({}),
            expected: vec![],
            changes: vec![Change::Put {
                collection: "settings".into(),
                id: new_id("setting"),
                data: row,
            }],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    drop(shared);
    let mut upgraded = SharedSettings::open(&profile).unwrap();
    assert_eq!(upgraded.catalog.current.settings["workers"], "3");
    upgraded.persist(upgraded.catalog.clone()).unwrap();
    let snapshot = upgraded.store.snapshot();
    let row = snapshot
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == CATALOG_KEY)
        .unwrap();
    let descriptor: CatalogReference =
        serde_json::from_str(row.data["value"].as_str().unwrap()).unwrap();
    let hash = descriptor.catalog_blob.sha256.as_str();
    let blob = profile
        .join("shared-settings/objects/blobs")
        .join(&hash[..2])
        .join(hash);
    assert!(
        blob.canonicalize()
            .unwrap()
            .starts_with(dir.path().canonicalize().unwrap())
    );
    drop(upgraded);
    std::fs::write(blob, b"corrupt fixture").unwrap();
    assert!(SharedSettings::open(&profile).is_err());
}
