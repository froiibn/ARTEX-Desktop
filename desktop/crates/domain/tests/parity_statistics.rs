//! Local regression fixtures for Source filtering and cold statistics parity.
use artex_core::{Actor, Context, body, new_id};
use artex_domain::{Planner, plan};
use artex_store::Store;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

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
fn apply(store: &mut Store, method: &str, params: Value, c: &Context) -> Value {
    let mut batch = plan(method, &params, &store.snapshot(), c).unwrap();
    batch.command_id = new_id("command");
    store.commit(batch).unwrap().result
}
fn patch(store: &mut Store, collection: &str, id: &str, changes: Value) {
    let s = store.snapshot();
    let mut p = Planner::new(&s, "test.parity.patch", &json!({}));
    let mut d = s.get(collection, id).unwrap().data.clone();
    artex_domain::merge(&mut d, &changes);
    p.put(collection, id, d);
    let mut b = p.done(json!({}));
    b.command_id = new_id("command");
    store.commit(b).unwrap();
}
fn fixture() -> (tempfile::TempDir, Store, Context, String) {
    let d = tempfile::tempdir().unwrap();
    let mut store = Store::create(&d.path().join("workspace"), "filter fixture").unwrap();
    let mut c = context();
    let task = apply(
        &mut store,
        "task.create",
        json!({"description":"local fixture","goal":"no network"}),
        &c,
    )["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    c.task_id = Some(task);
    let f=apply(&mut store,"report_finding",json!({"vulnclass":"fixture class","severity":"high","summary":"fixture summary","evidence":"short proof"}),&c)["finding_id"].as_str().unwrap().to_owned();
    (d, store, c, f)
}
fn reader(store: &Store, reference: &Value) -> artex_core::Result<Vec<u8>> {
    let mut bytes = vec![];
    store.copy_blob_to(reference, &mut bytes)?;
    Ok(bytes)
}
#[test]
fn blob_search_reaches_late_unicode_in_report_evidence_groups_tree_and_filtered_export() {
    let (_d, mut store, c, f) = fixture();
    let report = format!("{}끝-MixedNeedle", "x".repeat(160000));
    let blob = store.blob_put(report.as_bytes(), "text/plain").unwrap();
    patch(
        &mut store,
        "findings",
        &f,
        json!({"report":{"text":null,"blob":blob}}),
    );
    let s = store.snapshot();
    let read = |r: &Value| reader(&store, r);
    let q = json!({"query":"끝-mixedneedle"});
    let unavailable = |_: &Value| {
        Err(artex_core::Error::new(
            "IO_FAILURE",
            "must not read filtered-out bodies",
        ))
    };
    assert_eq!(
        artex_domain::findings_ui::filtered_with_reader(&s, &json!({}), &unavailable)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        artex_domain::findings_ui::filtered_with_reader(
            &s,
            &json!({"query":"needle","severity":"critical"}),
            &unavailable
        )
        .unwrap()
        .len(),
        0
    );
    for (method, key) in [
        ("ui.findingsPage", "total"),
        ("ui.findingGroups", "finding_total"),
    ] {
        let result =
            artex_domain::findings_ui::query_with_reader(method, &json!({"q":q}), &s, &c, &read)
                .unwrap();
        assert_eq!(result[key], 1);
    }
    let tree = artex_domain::asset_graph_ui::query_with_reader(
        "ui.findingAssetTree",
        &json!({"q":q}),
        &s,
        &read,
    )
    .unwrap();
    assert_eq!(tree["finding_total"], 1);
    let out = artex_domain::report_export::export(
        &json!({"opts":{"format":"json","scope":"filtered","filters":q}}),
        &s,
        &read,
    )
    .unwrap();
    let bytes = STANDARD
        .decode(out["content_base64"]["text"].as_str().unwrap())
        .unwrap();
    let rows: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(rows[0]["id"], f);
    assert!(
        rows[0]["report"]
            .as_str()
            .unwrap()
            .ends_with("끝-MixedNeedle")
    );
    assert_eq!(
        artex_domain::findings_ui::filtered_with_reader(
            &s,
            &json!({"query":"not-in-fixture"}),
            &read
        )
        .unwrap()
        .len(),
        0
    );
    assert_eq!(
        artex_domain::findings_ui::filtered_with_reader(&s, &q, &|_| Err(artex_core::Error::new(
            "IO_FAILURE",
            "fixture read failure"
        )))
        .unwrap_err()
        .code,
        "IO_FAILURE"
    );
    patch(
        &mut store,
        "findings",
        &f,
        json!({"report":body("normal report"),"evidence":{"text":null,"blob":blob}}),
    );
    let read = |r: &Value| reader(&store, r);
    let s = store.snapshot();
    assert_eq!(
        artex_domain::findings_ui::filtered_with_reader(&s, &q, &read)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        artex_domain::findings_ui::filtered(&s, &json!({"query":"normal report"}))
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn command_stats_matches_input_list_for_inline_and_blob_and_propagates_read_failures() {
    let (_d, mut store, c, _f) = fixture();
    let exp = store
        .snapshot()
        .get("tasks", c.task_id.as_ref().unwrap())
        .unwrap()
        .data["exploration_id"]
        .clone();
    let long = format!("{}shared-command-needle", "x".repeat(90000));
    let blob = store.blob_put(long.as_bytes(), "text/plain").unwrap();
    let s = store.snapshot();
    let mut p = Planner::new(&s, "test.commands", &json!({}));
    for (i, detail) in [
        body("echo shared-command-needle"),
        json!({"text":null,"blob":blob}),
    ]
    .into_iter()
    .enumerate()
    {
        p.insert("activity","activity",json!({"exploration_id":exp,"kind":"tool_use","worker":"worker","tool":"Bash","tool_use_id":format!("fixture-use-{i}"),"detail":detail})).unwrap();
    }
    let mut b = p.done(json!({}));
    b.command_id = new_id("command");
    store.commit(b).unwrap();
    let s = store.snapshot();
    let read = |r: &Value| reader(&store, r);
    for needle in ["Bash", "shared-command-needle", "absent-token"] {
        let params = json!({"params":{"q":needle,"task":c.task_id}});
        let stats = artex_domain::auxiliary_ui::query_with_reader(
            "ui.commandStats",
            &params,
            &s,
            &c,
            &read,
        )
        .unwrap();
        let list = artex_domain::llm_ui::query("ui.commands", &params, &s, &c, &read).unwrap();
        let total = stats["stats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["total"].as_u64().unwrap())
            .sum::<u64>();
        assert_eq!(total, list["total"].as_u64().unwrap());
    }
    assert!(
        artex_domain::auxiliary_ui::query_with_reader(
            "ui.commandStats",
            &json!({"params":{"q":"shared-command-needle"}}),
            &s,
            &c,
            &|_| Err(artex_core::Error::new("SIZE_LIMIT", "fixture boundary"))
        )
        .is_err()
    );
}
#[test]
fn source_creation_options_reach_task_scope_runtime_intents_and_rules() {
    let (_d, mut store, c, _f) = fixture();
    let s = store.snapshot();
    let mut p = Planner::new(&s, "test.creation.references", &json!({}));
    let company = p
        .insert("companies", "company", json!({"name":"fixture company"}))
        .unwrap();
    let category = p
        .insert(
            "task_categories",
            "category",
            json!({"name":"fixture category"}),
        )
        .unwrap();
    let mut b = p.done(json!({}));
    b.command_id = new_id("command");
    store.commit(b).unwrap();
    let task=apply(&mut store,"task.create",json!({"description":"advanced fixture","goal":"local only","company_ids":[company],"category_id":category,"timeout_seconds":120,"plan_heartbeat_seconds":900,"coverage_enabled":false,"seed_first_intent":true,"intercept_rules":[{"action":"block","kind":"exact_domain","pattern":"excluded.example","enabled":true,"note":"fixture"}]}),&c)["task_id"].as_str().unwrap().to_owned();
    let s = store.snapshot();
    let row = &s.get("tasks", &task).unwrap().data;
    assert_eq!(row["timeout_seconds"], 120);
    assert_eq!(row["plan_heartbeat_seconds"], 900);
    assert_eq!(row["coverage_enabled"], false);
    assert_eq!(row["category_id"], category);
    assert!(
        s.records("task_scope")
            .iter()
            .any(|r| r.data["task_id"] == task && r.data["company_id"] == company)
    );
    assert!(
        s.records("task_runtime")
            .iter()
            .any(|r| r.data["task_id"] == task && r.data["remaining_timeout_ms"] == "120000")
    );
    assert!(
        s.records("exploration_nodes")
            .iter()
            .any(
                |r| r.data["exploration_id"] == row["exploration_id"] && r.data["kind"] == "intent"
            )
    );
    assert!(
        s.records("task_intercept_rules")
            .iter()
            .any(|r| r.data["task_id"] == task && r.data["pattern"] == "excluded.example")
    );
}

#[test]
fn cold_statistics_count_one_logical_task_despite_registry_aliases_or_nullable_foreign_keys() {
    let task = new_id("task");
    let exp = new_id("exploration");
    let tables = serde_json::from_value(json!({
        "tasks":[{"id":task,"exploration_id":exp}],
        "llm_usage":[
            {"task_id":"legacy-registry-a","exploration_id":exp,"profile_name":"fixture","ts":"2026-10-17T00:00:00Z","input_tokens":2},
            {"task_id":"legacy-registry-b","exploration_id":exp,"profile_name":"fixture","ts":"2026-10-17T00:00:00Z","input_tokens":3},
            {"task_id":task,"profile_name":"direct fixture","ts":"2026-10-17T00:00:00Z","input_tokens":7},
            {"task_id":"unrelated","exploration_id":"other","profile_name":"unrelated","input_tokens":100}
        ],
        "skill_usage":[
            {"task_id":null,"exploration_id":exp,"skill":"fixture","found":true,"agent_key":"worker","ts":"2026-10-17T00:00:00Z"},
            {"task_id":task,"skill":"direct fixture","found":true,"ts":"2026-10-17T00:00:00Z"},
            {"task_id":null,"exploration_id":exp,"skill":"missing fixture","found":false}
        ]
    })).unwrap();
    let before = serde_json::to_value(&tables).unwrap();
    let stats = artex_domain::auxiliary_ui::archive_statistics(&tables, &task).unwrap();
    assert_eq!(
        stats["native_statistics_version"],
        artex_domain::auxiliary_ui::ARCHIVE_STATISTICS_VERSION
    );
    let profiles = stats["token_profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 2);
    for profile in profiles {
        assert_eq!(profile["tasks"], 1);
    }
    let fixture = profiles
        .iter()
        .find(|p| p["profile_name"] == "fixture")
        .unwrap();
    assert_eq!(fixture["calls"], 2);
    assert_eq!(fixture["input_tokens"], 5);
    for skill in stats["skill_stats"].as_array().unwrap() {
        assert_eq!(skill["tasks"], 1);
    }
    assert_eq!(stats["missing_skill_stats"][0]["tasks"], 0);
    // Compact statistics never normalize or rewrite the historical ledger IDs.
    assert_eq!(serde_json::to_value(&tables).unwrap(), before);
}

#[test]
fn archive_wire_projects_all_source_totals_without_mutating_native_dimensions() {
    let (_d, store, c, _f) = fixture();
    let snapshot = store.snapshot();
    let mut archive = artex_core::default_data("task_archives").unwrap();
    archive["id"] = json!(new_id("archive"));
    archive["task_id"] = json!(c.task_id);
    archive["original_status"] = json!("paused");
    archive["aggregate_stats"] = json!({"native_statistics_version":2,"tables":7,"skills":{"fixture":2},"tools":{"Read":3},"finding_stats":{"total":1,"pending":1,"critical":0,"high":1,"medium":0,"low":0,"vulnclasses":["kept"],"tasks":[]},"token_profiles":[{"profile_name":"one","calls":1,"input_tokens":5,"output_tokens":2,"cache_read_tokens":3,"cache_write_tokens":4},{"profile_name":"two","calls":2,"input_tokens":7,"output_tokens":8,"cache_read_tokens":9,"cache_write_tokens":10}],"token_daily":[{"date":"2026-10-17","input_tokens":12}]});
    let before = archive.clone();
    let wire = artex_domain::auxiliary_ui::archive_wire(&archive, &snapshot).unwrap();
    assert_eq!(
        wire["aggregate_stats"]["tokens"],
        json!({"calls":3,"input_tokens":12,"output_tokens":10,"cache_read_tokens":12,"cache_write_tokens":14})
    );
    assert_eq!(wire["aggregate_stats"]["findings"]["high"], 1);
    assert_eq!(wire["aggregate_stats"]["tools"]["Read"], 3);
    assert_eq!(wire["aggregate_stats"]["skills"]["fixture"], 2);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(
        &contract["results"]["ui.restoreTaskArchive"],
        &contract["$defs"],
        &wire,
    )
    .unwrap();
    assert_eq!(archive, before);
    archive["aggregate_stats"] = json!({"token_profiles":[]});
    assert_eq!(artex_domain::auxiliary_ui::archive_wire(&archive,&snapshot).unwrap()["aggregate_stats"]["tokens"]["calls"],0);
    archive["aggregate_stats"] = json!({"tables":7});
    assert_eq!(
        artex_domain::auxiliary_ui::archive_wire(&archive, &snapshot).unwrap()["aggregate_stats"],
        json!({})
    );
    archive["aggregate_stats"] = json!({"token_profiles":[{"calls":-1}]});
    assert_eq!(
        artex_domain::auxiliary_ui::archive_wire(&archive, &snapshot)
            .unwrap_err()
            .code,
        "CORRUPT_STORE"
    );
    archive["aggregate_stats"] = json!({"tools":{"bad":9007199254740992u64}});
    assert_eq!(
        artex_domain::auxiliary_ui::archive_wire(&archive, &snapshot)
            .unwrap_err()
            .code,
        "SIZE_LIMIT"
    );
}
