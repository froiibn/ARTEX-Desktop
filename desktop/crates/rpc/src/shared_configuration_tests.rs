use super::*;
use artex_core::{Actor, CommandBatch, Context, body};
use artex_store::Store;

fn context() -> Context {
    Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: new_id("epoch"),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    }
}

fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(
        &dir.path().join("workspace"),
        "Portable configuration fixture",
    )
    .unwrap();
    let mut batch = artex_domain::bootstrap::bootstrap_plan(&store.snapshot(), &context()).unwrap();
    batch.command_id = new_id("command");
    store.commit(batch).unwrap();
    (dir, store)
}

fn commit(store: &mut Store, changes: Vec<Change>) {
    commit_metadata(store, changes, BTreeMap::new());
}

fn commit_metadata(
    store: &mut Store,
    changes: Vec<Change>,
    metadata: BTreeMap<(String, String), artex_store::ImportMetadata>,
) {
    if changes.is_empty() {
        return;
    }
    let s = store.snapshot();
    let mut batch = CommandBatch {
        command_id: new_id("command"),
        method: "migration.fixture_shared_configuration".into(),
        normalized_params: json!({}),
        expected: vec![],
        changes,
        events: vec![],
        deliveries: vec![],
        result: json!({}),
    };
    artex_domain::augment_expected(&mut batch, &s).unwrap();
    store.commit_import(batch, metadata).unwrap();
}

fn project(store: &mut Store, config: &SharedConfiguration) {
    let changes = config.projection(&store.snapshot()).unwrap();
    let metadata = config.import_metadata(&store.snapshot());
    commit_metadata(store, changes, metadata);
}

fn agent(s: &Snapshot, key: &str) -> Record {
    s.records("agents")
        .into_iter()
        .find(|r| r.data["key"] == key)
        .unwrap()
        .clone()
}

fn add(store: &mut Store, col: &str, prefix: &str, patch: Value) -> String {
    let id = new_id(prefix);
    let mut data = default_data(col).unwrap();
    overlay(&mut data, &patch).unwrap();
    if data.get("id").is_some() {
        data["id"] = json!(id);
    }
    commit(
        store,
        vec![Change::Put {
            collection: col.into(),
            id: id.clone(),
            data,
        }],
    );
    id
}

#[test]
fn projection_keeps_destination_ids_appends_prompt_and_does_not_import_runtime() {
    let (_a_dir, a) = fixture();
    let (_b_dir, mut b) = fixture();
    let source = a.snapshot();
    let dest = b.snapshot();
    let source_agent = agent(&source, "mainagent");
    let dest_agent = agent(&dest, "mainagent");
    assert_ne!(source_agent.header.id, dest_agent.header.id);
    let original_prompt = dest_agent.data["current_prompt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut config = SharedConfiguration::from_snapshot(&source).unwrap();
    let canonical = config.agents.get_mut("mainagent").unwrap();
    canonical.data["max_turns"] = json!(41);
    canonical.prompt = Some(
        json!({"template_text":body("Portable {{.Goal}} prompt"),"note":"Synthetic edit","updated_by":"user"}),
    );
    canonical.features = Some(json!({"questions":true,"plan_mode":true,"memory":true}));
    canonical.variables.insert(
        "FixtureVariable".into(),
        json!({"description":"User example","example":"not live task data","source":"runtime"}),
    );
    let changes = config.projection(&dest).unwrap();
    assert!(changes.iter().all(|c| !matches!(
        c.key().0,
        "agent_runs" | "memory_records" | "llm_profile_health" | "tasks"
    )));
    commit(&mut b, changes);
    let next = b.snapshot();
    for old_tool in dest.records("tools") {
        assert!(next.get("tools", &old_tool.header.id).is_some());
    }
    let next_agent = agent(&next, "mainagent");
    assert_eq!(next_agent.header.id, dest_agent.header.id);
    assert_eq!(next_agent.data["max_turns"], 41);
    assert!(next.get("agent_prompts", &original_prompt).is_some());
    let current = next
        .get(
            "agent_prompts",
            next_agent.data["current_prompt_id"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(current.data["version"], 2);
    assert_eq!(current.data["agent_id"], dest_agent.header.id);
    assert_eq!(
        current.data["template_text"],
        body("Portable {{.Goal}} prompt")
    );
    assert_eq!(SharedConfiguration::from_snapshot(&next).unwrap(), config);
    assert!(config.projection(&next).unwrap().is_empty());
    // Reopening validates persisted FK/schema state and leaves the portable view equal.
    b.checkpoint().unwrap();
    let root = _b_dir.path().join("workspace");
    drop(b);
    let reopened = Store::open(&root).unwrap();
    assert_eq!(
        SharedConfiguration::from_snapshot(&reopened.snapshot()).unwrap(),
        config
    );
    // Source prompt history and IDs are untouched by another workspace projection.
    assert_eq!(a.snapshot(), source);
}

#[test]
fn exact_mcp_denials_rebind_local_ids_and_changed_server_invalidates_only_its_cache() {
    let (_dir, mut source) = fixture();
    let a = agent(&source.snapshot(), "auto");
    let server = source.snapshot().records("mcp_servers")[0].clone();
    add(
        &mut source,
        "agent_visibility",
        "visibility",
        json!({"agent_id":a.header.id,"resource_kind":"mcp","resource_id":server.header.id,"mcp_tool_name":"","enabled":true}),
    );
    add(
        &mut source,
        "agent_visibility",
        "visibility",
        json!({"agent_id":a.header.id,"resource_kind":"mcp","resource_id":server.header.id,"mcp_tool_name":"dangerous","enabled":false}),
    );
    add(
        &mut source,
        "agent_skill_visibility",
        "skillvisibility",
        json!({"agent_id":a.header.id,"skill_name":"restricted","enabled":false}),
    );
    let config = SharedConfiguration::from_snapshot(&source.snapshot()).unwrap();
    let (_dir2, mut target) = fixture();
    let local = target.snapshot().records("mcp_servers")[0].clone();
    assert_ne!(local.header.id, server.header.id);
    project(&mut target, &config);
    let snapshot = target.snapshot();
    let local_agent = agent(&snapshot, "auto");
    let denied = snapshot
        .records("agent_visibility")
        .into_iter()
        .find(|r| r.data["mcp_tool_name"] == "dangerous")
        .unwrap();
    assert_eq!(denied.data["enabled"], false);
    assert_eq!(denied.data["resource_id"], local.header.id);
    assert_eq!(denied.data["agent_id"], local_agent.header.id);
    let cache = add(
        &mut target,
        "mcp_tools_cache",
        "mcptool",
        json!({"server_id":local.header.id,"tool_name":"old","schema":{},"description":"Old server"}),
    );
    let other = add(
        &mut target,
        "mcp_servers",
        "mcp",
        json!({"name":"other-fixture","transport":"stdio","command":"fixture","args":[],"env":{},"enabled":false}),
    );
    let other_cache = add(
        &mut target,
        "mcp_tools_cache",
        "mcptool",
        json!({"server_id":other,"tool_name":"unrelated","schema":{}}),
    );
    let mut newer = SharedConfiguration::from_snapshot(&target.snapshot()).unwrap();
    newer.mcps.get_mut("browser").unwrap()["command"] = json!("changed-fixture-command");
    project(&mut target, &newer);
    assert!(target.snapshot().get("mcp_tools_cache", &cache).is_none());
    assert!(
        target
            .snapshot()
            .get("mcp_tools_cache", &other_cache)
            .is_some()
    );
    assert!(newer.projection(&target.snapshot()).unwrap().is_empty());
}

#[test]
fn triggers_map_one_to_one_preserve_local_timer_and_ignore_timer_only_changes() {
    let (_a, mut source) = fixture();
    // Identical independent trigger configurations are legal and retain multiplicity.
    let trigger = source.snapshot().records("agent_triggers")[0].clone();
    add(
        &mut source,
        "agent_triggers",
        "trigger",
        fields(&trigger.data, TRIGGER_FIELDS).unwrap(),
    );
    let config = SharedConfiguration::from_snapshot(&source.snapshot()).unwrap();
    let (_b, mut dest) = fixture();
    let existing = dest.snapshot().records("agent_triggers")[0].clone();
    let mut timer = existing.data.clone();
    timer["last_fire"] = json!("2026-10-20T03:00:00.000Z");
    commit(
        &mut dest,
        vec![Change::Put {
            collection: "agent_triggers".into(),
            id: existing.header.id.clone(),
            data: timer,
        }],
    );
    project(&mut dest, &config);
    let snap = dest.snapshot();
    assert_eq!(snap.records("agent_triggers").len(), 2);
    assert_eq!(
        snap.get("agent_triggers", &existing.header.id)
            .unwrap()
            .data["last_fire"],
        "2026-10-20T03:00:00.000Z"
    );
    assert_eq!(SharedConfiguration::from_snapshot(&snap).unwrap(), config);
    assert!(config.projection(&snap).unwrap().is_empty());
    let mut changed = snap
        .get("agent_triggers", &existing.header.id)
        .unwrap()
        .data
        .clone();
    changed["last_fire"] = json!("2026-10-20T04:00:00.000Z");
    commit(
        &mut dest,
        vec![Change::Put {
            collection: "agent_triggers".into(),
            id: existing.header.id.clone(),
            data: changed,
        }],
    );
    assert_eq!(
        SharedConfiguration::from_snapshot(&dest.snapshot()).unwrap(),
        config
    );
}

#[test]
fn deleting_configuration_cleans_only_configuration_and_tombstones_stop_old_imports() {
    let (_dir, mut store) = fixture();
    let id = add(
        &mut store,
        "agents",
        "agent",
        json!({"key":"custom_fixture","name":"Custom fixture","role":"assistant","enabled":true,"builtin":false}),
    );
    add(
        &mut store,
        "agent_prompt_vars",
        "promptvar",
        json!({"agent_id":id,"var_name":"x","source":"runtime"}),
    );
    let original = SharedConfiguration::from_snapshot(&store.snapshot()).unwrap();
    let mut desired = original.clone();
    let deleted = BTreeMap::from([("agents".into(), BTreeSet::from(["custom_fixture".into()]))]);
    desired.without_deleted(&deleted);
    // Historical records are extension-owned and have no agent CASCADE FK.
    let mut snapshot = store.snapshot();
    snapshot
        .collections
        .entry("agent_runs".into())
        .or_default()
        .insert(
            "history".into(),
            Record {
                header: artex_core::Header {
                    id: "history".into(),
                    collection: "agent_runs".into(),
                    version: "1".into(),
                    commit_seq: "1".into(),
                    created_at: artex_core::now(),
                    updated_at: artex_core::now(),
                    legacy_key: Value::Null,
                },
                data: json!({"agent_id":id,"state":"done"}),
            },
        );
    let changes = desired.projection(&snapshot).unwrap();
    assert!(
        changes.iter().any(
            |c| matches!(c, Change::Delete{collection,id:i} if collection=="agents" && i==&id)
        )
    );
    assert!(changes.iter().all(|c| c.key().0 != "agent_runs"));
    project(&mut store, &desired);
    assert!(store.snapshot().get("agents", &id).is_none());
    assert!(
        store
            .snapshot()
            .records("agent_prompt_vars")
            .iter()
            .all(|r| r.data["agent_id"] != id)
    );
    let mut legacy = original;
    legacy.without_deleted(&deleted);
    assert!(!desired.merge_missing(&legacy));
}

#[test]
fn missing_current_prompt_errors_but_deleted_mcp_orphan_is_not_reimported() {
    let (_dir, store) = fixture();
    let mut snapshot = store.snapshot();
    let a = agent(&snapshot, "auto");
    let pid = a.data["current_prompt_id"].as_str().unwrap();
    snapshot
        .collections
        .get_mut("agent_prompts")
        .unwrap()
        .remove(pid);
    assert_eq!(
        SharedConfiguration::from_snapshot(&snapshot)
            .err()
            .unwrap()
            .code,
        "CORRUPT_STORE"
    );
    let mut snapshot = store.snapshot();
    let server = snapshot.records("mcp_servers")[0].header.id.clone();
    let id = new_id("visibility");
    snapshot.collections.entry("agent_visibility".into()).or_default().insert(id.clone(), Record {
        header: artex_core::Header { id,collection:"agent_visibility".into(),version:"1".into(),commit_seq:"1".into(),created_at:artex_core::now(),updated_at:artex_core::now(),legacy_key:Value::Null },
        data:json!({"agent_id":a.header.id,"resource_kind":"mcp","resource_id":server,"mcp_tool_name":"","enabled":true,"created_at":artex_core::now()}),
    });
    snapshot
        .collections
        .get_mut("mcp_servers")
        .unwrap()
        .remove(&server);
    let config = SharedConfiguration::from_snapshot(&snapshot).unwrap();
    assert!(config.agents["auto"].visibility.is_empty());
    assert!(
        config
            .projection(&snapshot)
            .unwrap()
            .iter()
            .any(|c| matches!(c, Change::Delete{collection,..} if collection=="agent_visibility"))
    );
}

#[test]
fn merge_keeps_canonical_config_and_body_references_use_only_typed_paths() {
    let (_a, a) = fixture();
    let (_b, b) = fixture();
    let mut canonical = SharedConfiguration::from_snapshot(&a.snapshot()).unwrap();
    let mut incoming = SharedConfiguration::from_snapshot(&b.snapshot()).unwrap();
    canonical.agents.get_mut("auto").unwrap().data["enabled"] = json!(false);
    assert!(!canonical.merge_missing(&incoming));
    assert_eq!(canonical.agents["auto"].data["enabled"], false);
    let reference = json!({"sha256":"0".repeat(64),"length":"80000","media_type":"text/plain"});
    let body = json!({"text":null,"blob":reference});
    incoming.agents.get_mut("auto").unwrap().data["wrapup_prompt"] = body.clone();
    incoming
        .agents
        .get_mut("auto")
        .unwrap()
        .prompt
        .as_mut()
        .unwrap()["template_text"] = body;
    incoming.tools.values_mut().next().unwrap()["exec"] =
        json!({"blob":{"this":"is opaque execution JSON"}});
    assert_eq!(incoming.body_references(), vec![reference]);
    let profile = new_id("profile");
    incoming.agents.get_mut("auto").unwrap().data["llm_profile_id"] = json!(profile);
    incoming.retain_profiles(&BTreeSet::new());
    assert!(incoming.agents["auto"].data["llm_profile_id"].is_null());
}

#[test]
fn public_mcp_update_invalidates_cached_schema_in_same_cas_batch() {
    let (_dir, mut store) = fixture();
    let server = store.snapshot().records("mcp_servers")[0].clone();
    let cache = add(
        &mut store,
        "mcp_tools_cache",
        "mcptool",
        json!({"server_id":server.header.id,"tool_name":"old","schema":{}}),
    );
    let snap = store.snapshot();
    let mut p = server.data.clone();
    p["command"] = json!("new-fixture-binary");
    let mut batch = artex_domain::configuration::plan("update_mcp", &p, &snap, &context()).unwrap();
    batch.command_id = new_id("command");
    assert!(batch.changes.iter().any(|c| matches!(c, Change::Delete{collection,id} if collection=="mcp_tools_cache" && id==&cache)));
    assert!(
        batch
            .expected
            .iter()
            .any(|e| e.collection == "mcp_tools_cache" && e.id == cache)
    );
    store.commit(batch).unwrap();
    assert!(store.snapshot().get("mcp_tools_cache", &cache).is_none());
}

#[test]
fn large_current_prompt_keeps_verified_blob_reference_and_local_history() {
    let (_a, mut source) = fixture();
    let (_b, mut destination) = fixture();
    let content = "LARGE-CONFIGURATION-PROMPT\n".repeat(crate::MAX_CHUNK);
    let reference = source
        .blob_put(content.as_bytes(), "text/plain; charset=utf-8")
        .unwrap();
    let s = source.snapshot();
    let current = agent(&s, "auto");
    let prompt_id = current.data["current_prompt_id"].as_str().unwrap();
    let mut prompt = s.get("agent_prompts", prompt_id).unwrap().data.clone();
    prompt["template_text"] = json!({"text":null,"blob":reference});
    commit(
        &mut source,
        vec![Change::Put {
            collection: "agent_prompts".into(),
            id: prompt_id.into(),
            data: prompt,
        }],
    );
    let config = SharedConfiguration::from_snapshot(&source.snapshot()).unwrap();
    let encoded = serde_json::to_vec(&config).unwrap();
    let decoded: SharedConfiguration = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, config);
    assert_eq!(decoded.body_references(), vec![reference.clone()]);
    // Blob ownership is the catalog owner's explicit responsibility. Install the
    // fixture bytes in the destination before committing the pure projection.
    assert_eq!(
        destination
            .blob_put(content.as_bytes(), "text/plain; charset=utf-8")
            .unwrap(),
        reference
    );
    let before = destination.snapshot().records("agent_prompts").len();
    project(&mut destination, &decoded);
    let next = destination.snapshot();
    assert_eq!(next.records("agent_prompts").len(), before + 1);
    assert_eq!(SharedConfiguration::from_snapshot(&next).unwrap(), decoded);
    assert!(decoded.projection(&next).unwrap().is_empty());
}

#[test]
fn notification_configuration_preserves_ids_secrets_and_local_delivery_ownership() {
    let (_a, mut source) = fixture();
    let secret = json!({"secret_ref":new_id("secret"),"hint":"synthetic fixture"});
    let channel = add(
        &mut source,
        "notification_channels",
        "notifychannel",
        json!({
            "name":"Synthetic webhook","kind":"webhook","enabled":false,
            "config":{"url":secret},"mode":"digest","filter":{"min_severity":"high"},"rate_per_min":7,
        }),
    );
    let source_event = add(
        &mut source,
        "notification_events",
        "notifyevent",
        json!({
            "kind":"finding_created","finding_id":new_id("finding"),"snapshot":{"site":"source.fixture.test"},"fanned_out":true,
        }),
    );
    add(
        &mut source,
        "notification_deliveries",
        "notifydelivery",
        json!({
            "event_id":source_event,"channel_id":channel,"state":"sent","attempts":2,
        }),
    );
    let config = SharedConfiguration::from_snapshot(&source.snapshot()).unwrap();
    assert!(config.root_keys()["notification_channels"].contains(&channel));
    let mut old_encoding = serde_json::to_value(&config).unwrap();
    old_encoding
        .as_object_mut()
        .unwrap()
        .remove("notification_channels");
    let old: SharedConfiguration = serde_json::from_value(old_encoding).unwrap();
    assert!(
        old.notification_channels.is_empty(),
        "Old catalogs remain readable"
    );
    let (_b, mut destination) = fixture();
    project(&mut destination, &config);
    let snap = destination.snapshot();
    assert_eq!(
        snap.get("notification_channels", &channel).unwrap().data["config"]["url"],
        secret
    );
    assert!(snap.records("notification_events").is_empty());
    assert!(snap.records("notification_deliveries").is_empty());
    let event = add(
        &mut destination,
        "notification_events",
        "notifyevent",
        json!({
            "kind":"finding_created","finding_id":new_id("finding"),"snapshot":{"site":"local.fixture.test"},"fanned_out":true,
        }),
    );
    let delivery = add(
        &mut destination,
        "notification_deliveries",
        "notifydelivery",
        json!({
            "event_id":event,"channel_id":channel,"state":"sent","attempts":3,
        }),
    );
    let local_delivery = destination
        .snapshot()
        .get("notification_deliveries", &delivery)
        .unwrap()
        .clone();
    let mut changed = config.clone();
    changed.notification_channels.get_mut(&channel).unwrap()["rate_per_min"] = json!(13);
    let changes = changed.projection(&destination.snapshot()).unwrap();
    assert!(
        changes
            .iter()
            .all(|c| !matches!(c.key().0, "notification_events" | "notification_deliveries"))
    );
    commit(&mut destination, changes);
    assert_eq!(
        destination
            .snapshot()
            .get("notification_deliveries", &delivery),
        Some(&local_delivery)
    );
    assert!(
        changed
            .projection(&destination.snapshot())
            .unwrap()
            .is_empty()
    );
    let mut imported = config;
    assert!(!changed.merge_missing(&imported));
    assert_eq!(changed.notification_channels[&channel]["rate_per_min"], 13);
    let deleted = BTreeMap::from([(
        "notification_channels".into(),
        BTreeSet::from([channel.clone()]),
    )]);
    changed.without_deleted(&deleted);
    imported.without_deleted(&deleted);
    assert!(!changed.merge_missing(&imported));
    let snap = destination.snapshot();
    let changes = changed.projection(&snap).unwrap();
    assert!(changes.iter().any(|c| matches!(c,Change::Delete{collection,id} if collection=="notification_channels" && id==&channel)));
    // Preserve the original explicit channel-deletion CASCADE semantics. The
    // immutable finding notification event itself must survive that deletion.
    let expanded = artex_core::expand_deletions(&snap, &changes).unwrap();
    assert!(expanded.iter().any(|c| matches!(c,Change::Delete{collection,id} if collection=="notification_deliveries" && id==&delivery)));
    commit(&mut destination, expanded);
    assert!(
        destination
            .snapshot()
            .get("notification_channels", &channel)
            .is_none()
    );
    assert!(
        destination
            .snapshot()
            .get("notification_deliveries", &delivery)
            .is_none()
    );
    assert!(
        destination
            .snapshot()
            .get("notification_events", &event)
            .is_some()
    );
    assert!(
        source
            .snapshot()
            .get("notification_events", &source_event)
            .is_some()
    );
}

#[test]
fn global_tool_rule_import_keeps_equal_priority_policy_order_and_local_consent() {
    let (_a, mut source) = fixture();
    let mut ids = [new_id("interceptrule"), new_id("interceptrule")];
    ids.sort();
    // Deliberately reverse the ID tie-breaker. Losing source creation times would
    // change a deny to allow when both rules arrive in one destination commit.
    let older = &ids[1];
    let newer = &ids[0];
    let mut changes = Vec::new();
    let mut metadata = BTreeMap::new();
    for (id, action, created) in [
        (older, "deny", "2026-10-19T01:00:00.000Z"),
        (newer, "allow", "2026-10-19T02:00:00.000Z"),
    ] {
        let mut data = default_data("intercept_rules").unwrap();
        overlay(
            &mut data,
            &json!({"id":id,"name":action,"enabled":true,"priority":5,
            "match_target":"tool_name","match_type":"string","pattern":"Bash","action":action}),
        )
        .unwrap();
        changes.push(Change::Put {
            collection: "intercept_rules".into(),
            id: id.clone(),
            data,
        });
        metadata.insert(
            ("intercept_rules".into(), id.clone()),
            artex_store::ImportMetadata {
                legacy_key: Value::Null,
                created_at: created.into(),
                updated_at: created.into(),
            },
        );
    }
    commit_metadata(&mut source, changes, metadata);
    assert!(artex_domain::approval_bypass::matching_deny(
        &source.snapshot(),
        "Bash",
        &json!({})
    ));
    let config = SharedConfiguration::from_snapshot(&source.snapshot()).unwrap();
    let (_b, mut destination) = fixture();
    let bypass = add(
        &mut destination,
        "settings",
        "setting",
        json!({"key":"approval_bypass","value":"true"}),
    );
    project(&mut destination, &config);
    let snap = destination.snapshot();
    assert!(artex_domain::approval_bypass::matching_deny(
        &snap,
        "Bash",
        &json!({})
    ));
    assert_eq!(
        snap.get("intercept_rules", older)
            .unwrap()
            .header
            .created_at,
        "2026-10-19T01:00:00.000Z"
    );
    assert_eq!(
        snap.get("intercept_rules", newer)
            .unwrap()
            .header
            .created_at,
        "2026-10-19T02:00:00.000Z"
    );
    assert_eq!(snap.get("settings", &bypass).unwrap().data["value"], "true");
    assert_eq!(SharedConfiguration::from_snapshot(&snap).unwrap(), config);
    assert!(config.projection(&snap).unwrap().is_empty());
    assert!(config.import_metadata(&snap).is_empty());
    let mut old_encoding = serde_json::to_value(&config).unwrap();
    old_encoding
        .as_object_mut()
        .unwrap()
        .remove("intercept_rules");
    let old: SharedConfiguration = serde_json::from_value(old_encoding).unwrap();
    assert!(old.intercept_rules.is_empty());
    let mut changed = config.clone();
    let deleted = BTreeMap::from([("intercept_rules".into(), BTreeSet::from([older.clone()]))]);
    changed.without_deleted(&deleted);
    let mut legacy = config;
    legacy.without_deleted(&deleted);
    assert!(!changed.merge_missing(&legacy));
    assert!(changed.root_keys()["intercept_rules"].contains(newer));
    project(&mut destination, &changed);
    assert!(!artex_domain::approval_bypass::matching_deny(
        &destination.snapshot(),
        "Bash",
        &json!({})
    ));
    assert_eq!(
        destination
            .snapshot()
            .get("settings", &bypass)
            .unwrap()
            .data["value"],
        "true"
    );
}
