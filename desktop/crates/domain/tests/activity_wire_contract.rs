use artex_core::{Actor, Change, Context, Snapshot, body, default_data, new_id};
use artex_domain::{conversation, exploration_ui, transcript_ui};
use artex_store::Store;
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

fn put(store: &mut Store, collection: &str, prefix: &str, patch: Value) -> String {
    let id = new_id(prefix);
    let mut data = default_data(collection).unwrap();
    artex_domain::merge(&mut data, &patch);
    if data.get("id").is_some() {
        data["id"] = json!(id);
    }
    let mut batch = artex_domain::empty_batch("test.activity_wire", &json!({}));
    batch.command_id = new_id("command");
    batch.changes.push(Change::Put {
        collection: collection.into(),
        id: id.clone(),
        data,
    });
    store.commit(batch).unwrap();
    id
}

fn item<'a>(response: &'a Value, id: &str) -> &'a Value {
    response["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["seq"] == id)
        .unwrap()
}

#[test]
fn persisted_internal_kinds_validate_all_four_nonempty_activity_list_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), "Activity wire contracts").unwrap();
    let context = context();
    let mut task = artex_domain::plan(
        "task.create",
        &json!({"description":"Activity audit","goal":"Preserve confirmed tool history"}),
        &store.snapshot(),
        &context,
    )
    .unwrap();
    task.command_id = new_id("command");
    let task_id = store.commit(task).unwrap().result["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let exploration =
        store.snapshot().get("tasks", &task_id).unwrap().data["exploration_id"].clone();
    let conversation_id = put(
        &mut store,
        "conversations",
        "conversation",
        json!({"agent_key":"auto","title":"Confirmed tool history"}),
    );
    let run_id = new_id("run");
    let audit = body(&json!({"run_id":run_id,"guard_audit":{"ts":1720000000,"tool":"Read","action":"allow","reason":"Authorized","command":""},"guard_attribution":"ok"}).to_string());
    let guidance = body(&json!({"run_id":run_id,"message":{"role":"user","content":[{"type":"text","text":"Loaded skill instructions"}]},"skill_invocation":{"name":"example","success":true}}).to_string());
    let mut ids = Vec::new();
    for (kind, wire_kind, detail) in [
        ("guard", "text", audit.clone()),
        ("skill_guidance", "user", guidance.clone()),
        (
            "scheduler_retry",
            "user",
            body("Confirmed failure retry notice"),
        ),
        ("user", "user", body("User-authored message")),
        ("text", "text", body("Visible response")),
    ] {
        let patch = json!({"worker":"mainagent","kind":kind,"summary":kind,"detail":detail,"metadata":{"run_id":run_id,"turn_id":"original-turn"}});
        let mut task_patch = patch.clone();
        task_patch["exploration_id"] = exploration.clone();
        let task_activity = put(&mut store, "activity", "activity", task_patch);
        let mut conversation_patch = patch;
        conversation_patch
            .as_object_mut()
            .unwrap()
            .remove("metadata");
        conversation_patch["conversation_id"] = json!(conversation_id);
        let conversation_activity = put(
            &mut store,
            "conversation_activities",
            "conversationactivity",
            conversation_patch,
        );
        ids.push((
            task_activity,
            conversation_activity,
            kind,
            wire_kind,
            detail,
        ));
    }
    // Internal configuration records still advance cursors but remain non-public.
    for kind in ["context_config", "context_nudge", "compression"] {
        put(
            &mut store,
            "activity",
            "activity",
            json!({"exploration_id":exploration,"worker":"mainagent","kind":kind,"summary":"Internal only","detail":body("Private configuration")}),
        );
        put(
            &mut store,
            "conversation_activities",
            "conversationactivity",
            json!({"conversation_id":conversation_id,"worker":"mainagent","kind":kind,"summary":"Internal only","detail":body("Private configuration")}),
        );
    }
    let snapshot = store.snapshot();
    let original: Snapshot = snapshot.clone();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (method, params) in [
        ("ui.conversationMessages", json!({"id":conversation_id})),
        (
            "ui.conversationHistory",
            json!({"id":conversation_id,"history":true}),
        ),
        ("ui.activity", json!({"task":task_id})),
        (
            "ui.activityHistory",
            json!({"task":task_id,"session":"main:0"}),
        ),
    ] {
        let response = if method.starts_with("ui.conversation") {
            conversation::query("conversation.messages", &params, &snapshot, &context).unwrap()
        } else {
            exploration_ui::query(method, &params, &snapshot, &context).unwrap()
        };
        let response =
            transcript_ui::project_with_snapshot(method, response, &snapshot, &|reference| {
                store.blob_read(reference, 0, 65536)
            })
            .unwrap();
        artex_core::validate_contract(&contract["results"][method], &contract, &response)
            .unwrap_or_else(|error| panic!("{method}: {error}"));
        assert_eq!(response["items"].as_array().unwrap().len(), ids.len());
        for (task_activity, conversation_activity, _, wire_kind, _) in &ids {
            let id = if method.starts_with("ui.conversation") {
                conversation_activity
            } else {
                task_activity
            };
            assert_eq!(item(&response, id)["kind"], *wire_kind, "{method}");
        }
    }
    for (task_activity, conversation_activity, kind, _, detail) in &ids {
        let task_row = snapshot.get("activity", task_activity).unwrap();
        let conversation_row = snapshot
            .get("conversation_activities", conversation_activity)
            .unwrap();
        assert_eq!(
            exploration_ui::activity(task_row, &snapshot, &task_id, true)["detail"],
            *detail
        );
        assert_eq!(
            conversation::wire_activity(conversation_row, true)["detail"],
            *detail
        );
        assert_eq!(
            conversation::query(
                "conversation.detail",
                &json!({"id":conversation_id,"seq":conversation_activity}),
                &snapshot,
                &context,
            )
            .unwrap(),
            *detail
        );
        assert_eq!(
            exploration_ui::query(
                "ui.activityDetail",
                &json!({"task":task_id,"id":task_activity}),
                &snapshot,
                &context,
            )
            .unwrap()["detail"],
            *detail
        );
        for row in [task_row, conversation_row] {
            assert_eq!(row.data["kind"], *kind);
            assert_eq!(row.data["detail"], *detail);
        }
        assert_eq!(task_row.data["metadata"]["run_id"], run_id);
        assert_eq!(task_row.data["metadata"]["turn_id"], "original-turn");
        if *kind == "guard" || *kind == "skill_guidance" {
            for row in [task_row, conversation_row] {
                let envelope: Value =
                    serde_json::from_str(row.data["detail"]["text"].as_str().unwrap()).unwrap();
                assert_eq!(envelope["run_id"], run_id);
            }
        }
    }
    assert_eq!(snapshot, original);
    assert_eq!(store.snapshot(), original);
}

#[test]
fn activity_wire_kind_preserves_public_kinds_and_existing_hidden_policy() {
    for kind in [
        "tool_use",
        "tool_result",
        "text",
        "thinking",
        "result",
        "user",
        "intent",
        "round",
        "usage",
        "llm_switch",
        "llm_failover",
        "intercept_request",
    ] {
        assert_eq!(conversation::activity_wire_kind(kind), kind);
        assert!(conversation::public_activity_kind(kind));
    }
    for kind in ["guard", "skill_guidance", "scheduler_retry"] {
        assert!(conversation::public_activity_kind(kind));
    }
    for kind in ["context_config", "context_nudge", "compression"] {
        assert!(!conversation::public_activity_kind(kind));
        assert_eq!(conversation::activity_wire_kind(kind), kind);
    }
}
