//! Recover model rows from previously selected workspaces without starting their runtime.
use super::*;

#[test]
fn empty_legacy_common_catalog_recovers_recent_models_preserving_keys_defaults_sources_and_deletions()
 {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("previous-model-owner");
    let active = dir.path().join("existing-without-models");
    let profile_root = dir.path().join("profile");
    let previous = Service::spawn(ServiceConfig::default()).unwrap();
    let h = previous.handle();
    open(
        &h,
        &old,
        OpenMode::Create {
            name: "Earlier configured space".into(),
        },
        false,
    );
    let secret = new_id("secret");
    let configured = command(
        &h,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"preserved-model",Some(&secret))}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    command(&h, "ui.activateLLMProfile", json!({"id":configured}));
    let mut other = model_input(None, "second-preserved-model", Some(&new_id("secret")));
    other["name"] = json!("Second earlier model");
    let deleted = command(&h, "ui.saveLLMProfile", json!({"p":other}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    close(&h);
    shutdown(previous);
    let before = fingerprints(&old);
    paused_workspace(&active);
    std::fs::create_dir_all(&profile_root).unwrap();
    std::fs::write(
        profile_root.join("recent-workspaces.json"),
        serde_json::to_vec(&vec![&active, &dir.path().join("removed"), &old, &old]).unwrap(),
    )
    .unwrap();
    // This is the already-shipped empty inline v1 catalog, before the migration
    // marker existed. It must recover on first open after upgrading this build.
    let mut common =
        Store::create(&profile_root.join("shared-settings"), "Earlier app catalog").unwrap();
    let mut row = default_data("settings").unwrap();
    row["key"] = json!("native.shared_settings.v1");
    row["value"]=json!(json!({"format_version":1,"current":{"profiles":{},"settings":{}},"deleted_profiles":[],"imports":{},"pending":null}).to_string());
    common
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.old.catalog".into(),
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
    drop(common);
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile_root).unwrap();
    let h = service.handle();
    open(&h, &active, OpenMode::Existing, false);
    let rows = query(&h, "ui.llmProfiles", json!({}));
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(profile(&rows, &configured)["model"], "preserved-model");
    assert_eq!(profile(&rows, &configured)["is_default"], true);
    assert_eq!(
        h.view()
            .unwrap()
            .generation
            .get("llm_profiles", &configured)
            .unwrap()
            .data["api_key"]["secret_ref"],
        secret
    );
    assert!(
        h.view()
            .unwrap()
            .generation
            .records("agent_runs")
            .is_empty()
    );
    assert_eq!(fingerprints(&old), before);
    command(&h, "ui.deleteLLMProfile", json!({"id":deleted}));
    close(&h);
    shutdown(service);
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile_root).unwrap();
    let h = service.handle();
    open(&h, &active, OpenMode::Existing, false);
    let rows = query(&h, "ui.llmProfiles", json!({}));
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(profile(&rows, &configured)["is_default"], true);
    assert_eq!(fingerprints(&old), before);
    close(&h);
    shutdown(service);
}

#[test]
fn recent_models_keep_an_existing_common_default_and_readonly_destination_bytes_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("profile");
    let old = dir.path().join("old");
    let active = dir.path().join("active");
    let destination = dir.path().join("readonly");
    paused_workspace(&destination);
    let before = fingerprints(&destination);
    let source = Service::spawn(ServiceConfig::default()).unwrap();
    let h = source.handle();
    open(
        &h,
        &old,
        OpenMode::Create {
            name: "Legacy models".into(),
        },
        false,
    );
    let imported = command(
        &h,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"old-default",Some(&new_id("secret")))}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    command(&h, "ui.activateLLMProfile", json!({"id":imported}));
    close(&h);
    shutdown(source);
    // Existing global choice wins over an older source's default flag.
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile_root).unwrap();
    let h = service.handle();
    open(
        &h,
        &active,
        OpenMode::Create {
            name: "Current choice".into(),
        },
        false,
    );
    let keep = command(
        &h,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"current-default",Some(&new_id("secret")))}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    command(&h, "ui.activateLLMProfile", json!({"id":keep}));
    close(&h);
    shutdown(service);
    // Recreate the pre-correction catalog marker state only in this private
    // fixture, while retaining the actual catalog blob and stable profile IDs.
    let mut common = Store::open(&profile_root.join("shared-settings")).unwrap();
    let snapshot = common.snapshot();
    let row = snapshot
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "native.shared_settings.v1")
        .unwrap();
    let mut descriptor: Value = serde_json::from_str(row.data["value"].as_str().unwrap()).unwrap();
    let mut bytes = Vec::new();
    common
        .copy_blob_to(&descriptor["catalog_blob"], &mut bytes)
        .unwrap();
    let mut catalog: Value = serde_json::from_slice(&bytes).unwrap();
    catalog
        .as_object_mut()
        .unwrap()
        .remove("recent_models_imported");
    descriptor["catalog_blob"] = common
        .blob_put(&serde_json::to_vec(&catalog).unwrap(), "application/json")
        .unwrap();
    let mut data = row.data.clone();
    data["value"] = json!(descriptor.to_string());
    common
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.old.marker".into(),
            normalized_params: json!({}),
            expected: vec![Expected {
                collection: "settings".into(),
                id: row.header.id.clone(),
                version: row.header.version.clone(),
            }],
            changes: vec![Change::Put {
                collection: "settings".into(),
                id: row.header.id.clone(),
                data,
            }],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    drop(common);
    std::fs::write(
        profile_root.join("recent-workspaces.json"),
        serde_json::to_vec(&vec![&old]).unwrap(),
    )
    .unwrap();
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile_root).unwrap();
    let h = service.handle();
    open(&h, &destination, OpenMode::Existing, true);
    let rows = query(&h, "ui.llmProfiles", json!({}));
    assert_eq!(profile(&rows, &keep)["is_default"], true);
    assert_eq!(profile(&rows, &imported)["is_default"], false);
    close(&h);
    shutdown(service);
    assert_eq!(fingerprints(&destination), before);
}
