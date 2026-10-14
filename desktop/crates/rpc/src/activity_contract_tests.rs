//! Real Store/Router coverage for internal activity kinds crossing the frozen
//! source UI boundary. No model, native tool process or network is started.
use super::*;
use artex_domain::Planner;
use artex_runtime::ToolCall;
use native::{NativeDispatch, NativeQuery, NativeTicket};

fn query_both(router: &mut Router, method: &str, params: Value) -> Value {
    let scope = router.native_scope().unwrap();
    let request = json!({
        "jsonrpc":"2.0", "protocol":"artex-rpc/1.0", "id":new_id("request"),
        "method":method, "workspace_id":scope.workspace_id,
        "engine_epoch":scope.engine_epoch, "command_id":null,
        "expected":[], "timeout_ms":5000, "params":params,
    });
    let response = router.response(request);
    assert!(response.get("error").is_none(), "{method}: {response}");
    let value = response["result"].clone();
    artex_core::validate_contract(
        &router.contract["results"][method],
        &router.contract["$defs"],
        &value,
    )
    .unwrap_or_else(|error| panic!("{method} result failed frozen contract: {error}"));
    router.enable_native();
    let dispatch = router
        .native_query(
            &NativeTicket::new(),
            &scope,
            NativeQuery::Feature {
                method: method.into(),
                params,
            },
        )
        .unwrap_or_else(|error| panic!("native {method}: {error}"));
    let NativeDispatch::Complete(reply) = dispatch else {
        panic!("activity queries must complete without an external operation");
    };
    assert_eq!(reply.value, value, "native and RPC projections must agree");
    assert_eq!(
        reply.read_seq.to_string(),
        response["read_seq"].as_str().unwrap()
    );
    value
}

fn append_guards(router: &mut Router, run: &str, pairs: usize) {
    for index in 0..pairs {
        let call = ToolCall {
            id: new_id("tooluse"),
            name: "Bash".into(),
            input: json!({"command":format!("fixture-{index}")}),
        };
        router
            .guard_audit(run, &call, "allow", "Fixture permission audit")
            .unwrap();
        // The audit's durable deduplication path must not produce extra rows.
        router
            .guard_audit(run, &call, "allow", "Fixture permission audit")
            .unwrap();
        router
            .guard_attribution(run, "Bash", "HTTP 403 forbidden fixture", false)
            .unwrap();
    }
}

fn append_skill_guidance(router: &mut Router, run: &str, context: &Context) -> String {
    let snapshot = router.snapshot().unwrap();
    let detail = json!({
        "run_id":run, "turn_id":new_id("turn"),
        "message":{"role":"user","content":[{"type":"text","text":"Approved skill instructions: preserve the source record."}]},
        "skill_invocation":{"name":"fixture-skill","success":true,"invocation_id":new_id("invocation")},
    });
    let batch = artex_domain::conversation::append_activity(
        &snapshot,
        context,
        run,
        "skill_guidance",
        "Skill instructions loaded",
        &artex_core::body(&detail.to_string()),
        None,
    )
    .unwrap();
    let id = batch.result["activity_id"].as_str().unwrap().to_owned();
    router.internal_commit(batch).unwrap();
    id
}

fn assert_page(router: &Router, page: &Value, collection: &str, guard_count: usize, skill: &str) {
    let snapshot = router.snapshot().unwrap();
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), guard_count + 1);
    let mut seen = HashSet::new();
    for item in items {
        let id = item["seq"].as_str().unwrap();
        assert!(seen.insert(id), "duplicate activity in UI projection");
        let stored = snapshot.get(collection, id).unwrap();
        if id == skill {
            assert_eq!(stored.data["kind"], "skill_guidance");
            assert_eq!(item["kind"], "user");
        } else {
            assert_eq!(stored.data["kind"], "guard");
            assert_eq!(item["kind"], "text");
            let detail: Value =
                serde_json::from_str(stored.data["detail"]["text"].as_str().unwrap()).unwrap();
            assert!(detail["guard_audit"].is_object() || detail["guard_attribution"] == "blocked");
        }
        assert_eq!(item["summary"], stored.data["summary"]);
    }
}

fn stored_data(router: &Router, collection: &str) -> BTreeMap<String, Value> {
    router
        .snapshot()
        .unwrap()
        .records(collection)
        .into_iter()
        .map(|row| (row.header.id.clone(), row.data.clone()))
        .collect()
}

#[test]
fn conversation_intercept_execution_validates_dual_namespace_and_preserves_correlated_pair() {
    let (_directory, mut router, run, context) = regression::seeded();
    let conversation = context.conversation_id.clone().unwrap();
    let call = ToolCall {
        id: new_id("tooluse"),
        name: "Bash".into(),
        input: json!({"command":"fixture-only; no process is executed"}),
    };
    let snapshot = router.snapshot().unwrap();
    let decision = artex_domain::policy::judge_decision_plan(
        &snapshot,
        &context,
        &run,
        &call.id,
        &call.name,
        &call.input,
        "allow",
        "Fixture decision, not a model request",
        &json!({}),
    )
    .unwrap();
    let approval = decision.result["pending_id"].as_str().unwrap().to_owned();
    router.internal_commit(decision).unwrap();
    router
        .record_tool_calls(&run, std::slice::from_ref(&call))
        .unwrap();
    router.append_transcript(
        &run, "tool_result", "Fixture result",
        &artex_runtime::Message {
            role: "user".into(),
            content: vec![json!({"type":"tool_result","tool_use_id":call.id,"content":"Correlated conversation fixture result","is_error":false})],
        }, None,
    ).unwrap();

    // A different task deliberately reuses the provider tool ID. Correlation
    // must still select the conversation's pair, not merge global activity.
    let snapshot = router.snapshot().unwrap();
    let batch = artex_domain::plan(
        "task.create",
        &json!({"description":"Unrelated intercept scope"}),
        &snapshot,
        &context,
    )
    .unwrap();
    let task = batch.result["task_id"].as_str().unwrap().to_owned();
    router.internal_commit(batch).unwrap();
    let snapshot = router.snapshot().unwrap();
    let mut planner = Planner::new(&snapshot, "fixture.unrelated_scope", &json!({}));
    let other_conversation = planner
        .insert(
            "conversations",
            "conversation",
            json!({"agent_key":"mainagent","title":"Unrelated conversation"}),
        )
        .unwrap();
    let mut data = snapshot.get("agent_runs", &run).unwrap().data.clone();
    data["conversation_id"] = Value::Null;
    data["task_id"] = json!(task);
    data["id"] = Value::Null;
    let other_run = planner.insert("agent_runs", "run", data).unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    router
        .record_tool_calls(&other_run, std::slice::from_ref(&call))
        .unwrap();
    router.append_transcript(
        &other_run, "tool_result", "Unrelated task result",
        &artex_runtime::Message {
            role:"user".into(),
            content:vec![json!({"type":"tool_result","tool_use_id":call.id,"content":"Unrelated task content must stay out","is_error":false})],
        }, None,
    ).unwrap();
    let snapshot = router.snapshot().unwrap();
    let other_context =
        artex_agent::context_for_run(&snapshot, &other_run, &router.engine_epoch).unwrap();
    let decision = artex_domain::policy::judge_decision_plan(
        &snapshot,
        &other_context,
        &other_run,
        &call.id,
        &call.name,
        &call.input,
        "allow",
        "Task fixture decision, not a model request",
        &json!({}),
    )
    .unwrap();
    let task_approval = decision.result["pending_id"].as_str().unwrap().to_owned();
    router.internal_commit(decision).unwrap();
    let before = stored_data(&router, "conversation_activities");
    let task_before = stored_data(&router, "activity");
    let approval_before = stored_data(&router, "intercept_pending");
    let sequence = router.snapshot().unwrap().commit_seq;
    assert_eq!(before.len(), 2);
    assert_eq!(task_before.len(), 2);
    let result = query_both(
        &mut router,
        "ui.interceptExecution",
        json!({"id":approval,"conversationId":conversation}),
    );
    assert_eq!(result["conversation_id"], conversation);
    assert_eq!(result["task_id"], Value::Null);
    assert_eq!(result["session"], "");
    let items = result["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "tool_use");
    assert_eq!(items[1]["kind"], "tool_result");
    assert_eq!(result["seq"], items[0]["seq"]);
    for item in items {
        let id = item["seq"].as_str().unwrap();
        assert!(artex_core::valid_id(id, "conversationactivity"));
        assert!(!task_before.contains_key(id));
        assert_eq!(item["tool_use_id"], call.id);
        assert_eq!(item["tool"], "Bash");
        let detail = query_both(
            &mut router,
            "ui.conversationMsgDetail",
            json!({"id":conversation,"seq":id}),
        );
        if item["kind"] == "tool_result" {
            assert_eq!(
                detail,
                artex_core::body("Correlated conversation fixture result")
            );
        } else {
            assert_eq!(detail, before[id]["detail"]);
        }
    }
    let task_result = query_both(
        &mut router,
        "ui.interceptExecution",
        json!({"id":task_approval}),
    );
    assert_eq!(task_result["task_id"], task);
    assert_eq!(task_result["conversation_id"], Value::Null);
    assert_eq!(task_result["session"], "main:0");
    assert_eq!(task_result["items"].as_array().unwrap().len(), 2);
    for item in task_result["items"].as_array().unwrap() {
        let id = item["seq"].as_str().unwrap();
        assert!(artex_core::valid_id(id, "activity"));
        assert!(task_before.contains_key(id));
        assert!(!before.contains_key(id));
    }
    let scope = router.native_scope().unwrap();
    let forbidden = json!({"id":approval,"conversationId":other_conversation});
    let response = router.response(json!({
        "jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),
        "method":"ui.interceptExecution","workspace_id":scope.workspace_id,
        "engine_epoch":scope.engine_epoch,"command_id":null,"expected":[],"timeout_ms":5000,"params":forbidden,
    }));
    assert_eq!(response["error"]["data"]["code"], "PERMISSION_DENIED");
    let native = router.native_query(
        &NativeTicket::new(),
        &scope,
        NativeQuery::Feature {
            method: "ui.interceptExecution".into(),
            params: forbidden,
        },
    );
    assert!(matches!(native, Err(error) if error.code == "PERMISSION_DENIED"));
    assert_eq!(stored_data(&router, "conversation_activities"), before);
    assert_eq!(stored_data(&router, "activity"), task_before);
    assert_eq!(stored_data(&router, "intercept_pending"), approval_before);
    assert_eq!(router.snapshot().unwrap().commit_seq, sequence);
}

#[test]
fn conversation_guard_and_skill_guidance_validate_through_rpc_and_native_without_rewriting_storage()
{
    let (_directory, mut router, run, context) = regression::seeded();
    let conversation = context.conversation_id.clone().unwrap();
    append_guards(&mut router, &run, 5);
    let skill = append_skill_guidance(&mut router, &run, &context);
    let before = stored_data(&router, "conversation_activities");
    let sequence = router.snapshot().unwrap().commit_seq;
    for method in ["ui.conversationMessages", "ui.conversationHistory"] {
        let page = query_both(&mut router, method, json!({"id":conversation}));
        assert_page(&router, &page, "conversation_activities", 10, &skill);
        let mut illegal = page.clone();
        illegal["items"][0]["kind"] = json!("guard");
        assert!(
            artex_core::validate_contract(
                &router.contract["results"][method],
                &router.contract["$defs"],
                &illegal,
            )
            .is_err(),
            "frozen contract must still reject raw internal guard kinds"
        );
    }
    for (id, data) in &before {
        let detail = query_both(
            &mut router,
            "ui.conversationMsgDetail",
            json!({"id":conversation,"seq":id}),
        );
        if id == &skill {
            assert_eq!(
                detail,
                artex_core::body("Approved skill instructions: preserve the source record.")
            );
        } else {
            assert_eq!(detail, data["detail"]);
        }
    }
    assert_eq!(stored_data(&router, "conversation_activities"), before);
    assert_eq!(router.snapshot().unwrap().commit_seq, sequence);
}

#[test]
fn task_guard_and_skill_guidance_validate_activity_history_and_details_without_rewriting_storage() {
    let (_directory, mut router, conversation_run, conversation_context) = regression::seeded();
    let snapshot = router.snapshot().unwrap();
    let batch = artex_domain::plan(
        "task.create",
        &json!({"description":"Activity contract regression"}),
        &snapshot,
        &conversation_context,
    )
    .unwrap();
    let task = batch.result["task_id"].as_str().unwrap().to_owned();
    router.internal_commit(batch).unwrap();
    let snapshot = router.snapshot().unwrap();
    let mut planner = Planner::new(&snapshot, "fixture.task_run", &json!({}));
    let mut data = snapshot
        .get("agent_runs", &conversation_run)
        .unwrap()
        .data
        .clone();
    data["conversation_id"] = Value::Null;
    data["task_id"] = json!(task);
    data["id"] = Value::Null;
    let run = planner.insert("agent_runs", "run", data).unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    let context =
        artex_agent::context_for_run(&router.snapshot().unwrap(), &run, &router.engine_epoch)
            .unwrap();
    append_guards(&mut router, &run, 15);
    let skill = append_skill_guidance(&mut router, &run, &context);
    let before = stored_data(&router, "activity");
    let sequence = router.snapshot().unwrap().commit_seq;
    for (method, params) in [
        ("ui.activity", json!({"task":task})),
        ("ui.activityHistory", json!({"task":task,"session":"main"})),
    ] {
        let page = query_both(&mut router, method, params);
        assert_page(&router, &page, "activity", 30, &skill);
    }
    for (id, data) in &before {
        let detail = query_both(
            &mut router,
            "ui.activityDetail",
            json!({"task":task,"id":id}),
        );
        if id == &skill {
            assert_eq!(
                detail["detail"],
                artex_core::body("Approved skill instructions: preserve the source record.")
            );
        } else {
            assert_eq!(detail["detail"], data["detail"]);
        }
    }
    assert_eq!(stored_data(&router, "activity"), before);
    assert_eq!(router.snapshot().unwrap().commit_seq, sequence);
}
