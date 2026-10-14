//! Approval barriers are tested through real Store plans and application dispatch.
use super::*;
use artex_runtime::HostGateway;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CapturedHost(Mutex<Vec<Value>>);
impl HostGateway for CapturedHost {
    fn request(
        &self,
        _: &str,
        _: &Context,
        operation: &str,
        input: Value,
        _: Duration,
    ) -> Result<Value> {
        if operation == "tool.execute" {
            self.0.lock().unwrap().push(input);
        }
        Err(Error::new(
            "EXTERNAL_FAILURE",
            "Approval fixture deliberately performs no native effect",
        ))
    }
}
fn state(call: ToolCall) -> RunState {
    RunState {
        cancellation: Cancellation::default(),
        in_flight: false,
        turns: 0,
        pending: VecDeque::from([call]),
        wrapup: false,
        profile_override: None,
        failed_profiles: Default::default(),
        idle_nudges: 0,
        intent_failures: 0,
        retry_at: None,
    }
}
fn call(name: &str, id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        input: if name == "Read" {
            json!({"file_path":"fixture.txt"})
        } else {
            json!({"command":"fixture-never-executed"})
        },
    }
}
fn prepared(router: &mut Router, method: &str, params: Value) -> DispatchRequest {
    let base = DispatchRequest {
        id: new_id("request"),
        method: "command.prepare".into(),
        workspace_id: Some(router.snapshot().unwrap().workspace_id.clone()),
        engine_epoch: Some(router.engine_epoch.clone()),
        command_id: None,
        expected: vec![],
        timeout_ms: 5000,
        params: json!({"method":method,"params":params}),
    };
    let result = router.dispatch_request(&base, None).unwrap();
    DispatchRequest {
        id: new_id("request"),
        method: method.into(),
        params,
        expected: serde_json::from_value(result["expected"].clone()).unwrap(),
        command_id: Some(new_id("command")),
        ..base
    }
}
fn command(router: &mut Router, method: &str, params: Value) -> Value {
    let request = prepared(router, method, params);
    router.dispatch_request(&request, None).unwrap()
}
fn seed_policy(router: &mut Router, action: &str) -> String {
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.policy", &json!({}));
    b.insert(
        "settings",
        "setting",
        json!({"key":"intercept_enabled_tools","value":"[\"Read\",\"Bash\"]"}),
    )
    .unwrap();
    let id=b.insert("intercept_rules","interceptrule",json!({"name":"fixture","enabled":true,"priority":100,"match_target":"tool_name","match_type":"string","pattern":"Read","action":action})).unwrap();
    router.internal_commit(b.done(json!({}))).unwrap();
    id
}
fn question(
    b: &mut Planner,
    rid: &str,
    kind: &str,
    typ: &str,
    toolid: &str,
    expired: bool,
) -> String {
    b.insert("user_questions","question",json!({"run_id":rid,"kind":kind,"payload":{"type":typ,"tool_use_id":toolid,"name":"Bash","questions":[{"question":"Fixture question","options":[]}]},"state":"pending","answer":null,"generation":"1","deadline_at":(Utc::now()+if expired{ChronoDuration::seconds(-1)}else{ChronoDuration::hours(1)}).to_rfc3339_opts(chrono::SecondsFormat::Millis,true)})).unwrap()
}
#[test]
fn bypass_settles_only_internal_barriers_preserves_questions_expiry_and_cas() {
    let (dir, mut router, rid, c) = crate::regression::seeded();
    seed_policy(&mut router, "ask");
    let read = call("Read", "pending-intercept");
    assert_eq!(router.intercept_tool(&rid, &read).unwrap(), Decision::Ask);
    let s = router.snapshot().unwrap();
    let approval = s.records("intercept_pending")[0].header.id.clone();
    let obsolete = prepared(
        &mut router,
        "ui.interceptDecide",
        json!({"id":approval,"decision":"denied"}),
    );
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.questions", &json!({}));
    let permission = question(&mut b, &rid, "question", "tool_approval", "p", false);
    let human = question(&mut b, &rid, "question", "question", "human", false);
    let plan = question(
        &mut b,
        &rid,
        "plan_approval",
        "tool_approval",
        "plan",
        false,
    );
    let expired = question(&mut b, &rid, "question", "tool_approval", "expired", true);
    router.internal_commit(b.done(json!({}))).unwrap();
    let changed = command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    assert_eq!(changed["approval_bypass"], true);
    let s = router.snapshot().unwrap();
    assert_eq!(
        s.get("intercept_pending", &approval).unwrap().data["status"],
        "allowed"
    );
    assert_eq!(
        s.get("intercept_pending", &approval).unwrap().data["audit"]["approval_bypass"],
        true
    );
    assert_eq!(
        s.get("user_questions", &permission).unwrap().data["state"],
        "answered"
    );
    for id in [&human, &plan, &expired] {
        assert_eq!(
            s.get("user_questions", id).unwrap().data["state"],
            "pending"
        );
    }
    assert_eq!(s.get("agent_runs", &rid).unwrap().data["state"], "waiting");
    assert_eq!(
        router.dispatch_request(&obsolete, None).unwrap_err().code,
        "VERSION_CONFLICT"
    );
    let s = router.snapshot().unwrap();
    assert_eq!(
        artex_domain::configuration_ui::plan(
            "ui.setSettings",
            &json!({"patch":{"approval_bypass":false}}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    router.store.as_mut().unwrap().checkpoint().unwrap();
    router.store.take();
    let reopened = Store::open(&dir.path().join("store")).unwrap();
    assert!(artex_domain::approval_bypass::enabled(&reopened.snapshot()));
}
#[test]
fn bypass_skips_new_rule_asks_and_model_judge_but_preserves_prior_and_explicit_denials() {
    let (_dir, mut router, rid, c) = crate::regression::seeded();
    let rule = seed_policy(&mut router, "ask");
    let manual = call("Read", "manual-intercept-refusal");
    assert_eq!(router.intercept_tool(&rid, &manual).unwrap(), Decision::Ask);
    let refused = router.snapshot().unwrap().records("intercept_pending")[0]
        .header
        .id
        .clone();
    command(
        &mut router,
        "ui.interceptDecide",
        json!({"id":refused,"decision":"denied"}),
    );
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    assert_eq!(
        router
            .intercept_tool(&rid, &call("Read", "bypass-ask"))
            .unwrap(),
        Decision::Allow
    );
    let s = router.snapshot().unwrap();
    assert!(
        s.records("intercept_pending")
            .iter()
            .any(|p| p.data["audit"]["tool_use_id"] == "bypass-ask"
                && p.data["audit"]["initial_action"] == "ask")
    );
    assert_eq!(
        router.intercept_tool(&rid, &manual).unwrap(),
        Decision::Deny
    );
    let mut b = Planner::new(&s, "test.deny", &json!({}));
    let mut data = s.get("intercept_rules", &rule).unwrap().data.clone();
    data["action"] = json!("deny");
    b.put("intercept_rules", &rule, data);
    b.insert(
        "settings",
        "setting",
        json!({"key":"llm_judge_enabled","value":"true"}),
    )
    .unwrap();
    router.internal_commit(b.done(json!({}))).unwrap();
    let denied = call("Read", "explicit-deny");
    assert_eq!(
        router.intercept_tool(&rid, &denied).unwrap(),
        Decision::Deny
    );
    assert_eq!(
        router.intercept_tool(&rid, &denied).unwrap(),
        Decision::Deny
    );
    assert_eq!(
        router
            .intercept_tool(&rid, &call("Bash", "skip-judge"))
            .unwrap(),
        Decision::Allow
    );
    assert!(router.runtime.judge_workers.is_empty());
    let s = router.snapshot().unwrap();
    let batch = artex_domain::policy::decision_plan(
        &s,
        &c,
        &rid,
        "model-skip",
        "Bash",
        &json!({"command":"fixture"}),
    )
    .unwrap();
    assert_eq!(batch.result["judge_required"], false);
    assert!(batch.changes.iter().any(|change|matches!(change,Change::Put{collection,data,..} if collection=="intercept_pending" && data["audit"]["approval_bypass"]==true)));
}
#[test]
fn bypass_rule_and_model_audits_validate_actual_router_and_native_intercept_detail() {
    use crate::native::{NativeDispatch, NativeQuery, NativeTicket};

    let (_dir, mut router, rid, _) = crate::regression::seeded();
    seed_policy(&mut router, "ask");
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.audit-enabled-judge", &json!({}));
    b.insert(
        "settings",
        "setting",
        json!({"key":"llm_judge_enabled","value":"true"}),
    )
    .unwrap();
    router.internal_commit(b.done(json!({}))).unwrap();
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    for (tool, initial, skipped) in [
        (call("Read", "detail-rule-ask"), "ask", Value::Null),
        (call("Bash", "detail-model-skip"), "allow", json!("model")),
    ] {
        assert_eq!(router.intercept_tool(&rid, &tool).unwrap(), Decision::Allow);
        let s = router.snapshot().unwrap();
        let row = s
            .records("intercept_pending")
            .into_iter()
            .find(|row| row.data["audit"]["tool_use_id"] == tool.id)
            .unwrap();
        assert_eq!(row.data["audit"]["approval_bypass"], true);
        assert_eq!(row.data["audit"]["initial_action"], initial);
        assert_eq!(row.data["audit"]["skipped_review"], skipped);
        assert_eq!(row.data["audit"]["model_fallback"], false);
        let params = json!({"id":row.header.id});
        let scope = router.native_scope().unwrap();
        let request = DispatchRequest {
            id: new_id("request"),
            method: "ui.interceptDetail".into(),
            workspace_id: Some(scope.workspace_id.clone()),
            engine_epoch: Some(scope.engine_epoch.clone()),
            command_id: None,
            expected: vec![],
            timeout_ms: 5000,
            params: params.clone(),
        };
        let detail = router.dispatch_request(&request, None).unwrap();
        artex_core::validate_contract(
            &router.contract["results"]["ui.interceptDetail"],
            &router.contract["$defs"],
            &detail,
        )
        .unwrap();
        // The persisted extensions stay available for diagnostics while the
        // frozen source UI audit projection contains only registered fields.
        assert_eq!(detail["audit"]["initial_action"], initial);
        assert!(detail["audit"].get("approval_bypass").is_none());
        assert!(detail["audit"].get("skipped_review").is_none());
        router.enable_native();
        let native = router
            .native_query(
                &NativeTicket::new(),
                &scope,
                NativeQuery::Feature {
                    method: "ui.interceptDetail".into(),
                    params,
                },
            )
            .unwrap();
        let NativeDispatch::Complete(reply) = native else {
            panic!("Intercept detail must complete without any host effect");
        };
        assert_eq!(reply.value, detail);
        artex_core::validate_contract(
            &router.contract["$defs"]["W467"],
            &router.contract["$defs"],
            &reply.value["audit"],
        )
        .unwrap();
        let mut invalid = reply.value["audit"].clone();
        invalid["initial_action"] = json!("model_review");
        assert!(
            artex_core::validate_contract(
                &router.contract["$defs"]["W467"],
                &router.contract["$defs"],
                &invalid,
            )
            .is_err()
        );
    }
    assert!(router.runtime.judge_workers.is_empty());
}
#[test]
fn bypass_permission_asks_never_override_plan_mode_explicit_deny_or_manual_rejection() {
    let (_dir, mut router, rid, _) = crate::regression::seeded();
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    assert_eq!(
        router
            .approval(&rid, &call("Bash", "bypass-permission"))
            .unwrap(),
        Decision::Allow
    );
    assert!(
        router
            .snapshot()
            .unwrap()
            .records("user_questions")
            .is_empty()
    );
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.plan", &json!({}));
    let mut run = s.get("agent_runs", &rid).unwrap().data.clone();
    run["permission_mode"] = json!("plan");
    b.put("agent_runs", &rid, run);
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(
        router.approval(&rid, &call("Bash", "plan-write")).unwrap(),
        Decision::Deny
    );
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.permission", &json!({}));
    let mut run = s.get("agent_runs", &rid).unwrap().data.clone();
    run["permission_mode"] = json!("default");
    b.put("agent_runs", &rid, run);
    let denied = question(
        &mut b,
        &rid,
        "question",
        "tool_approval",
        "manual-denied",
        false,
    );
    let approved = question(
        &mut b,
        &rid,
        "question",
        "tool_approval",
        "prior-approved",
        false,
    );
    router.internal_commit(b.done(json!({}))).unwrap();
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.answers", &json!({}));
    for (id, allow) in [(&denied, false), (&approved, true)] {
        let mut data = s.get("user_questions", id).unwrap().data.clone();
        data["state"] = json!("answered");
        data["answer"] = json!({"approved":allow});
        b.put("user_questions", id, data);
    }
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(
        router
            .approval(&rid, &call("Bash", "manual-denied"))
            .unwrap(),
        Decision::Deny
    );
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.permissions", &json!({}));
    b.insert(
        "settings",
        "setting",
        json!({"key":"permissions","value":"{\"deny\":[\"Bash\"]}"}),
    )
    .unwrap();
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(
        router
            .approval(&rid, &call("Bash", "prior-approved"))
            .unwrap(),
        Decision::Deny
    );
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":false}}),
    );
    assert!(!artex_domain::approval_bypass::enabled(
        router.snapshot().unwrap().as_ref()
    ));
}
#[test]
fn manual_intercept_and_permission_answers_prepare_commit_and_start_exact_tool_without_scheduler_delay()
 {
    for permission in [false, true] {
        let (_dir, mut router, rid, _) = crate::regression::seeded();
        let host = Arc::new(CapturedHost::default());
        router.attach_host(host.clone());
        let tool = call("Bash", "manual-approved");
        router.runtime.runs.insert(rid.clone(), state(tool.clone()));
        let (method, params) = if permission {
            assert_eq!(router.approval(&rid, &tool).unwrap(), Decision::Ask);
            let s = router.snapshot().unwrap();
            let q = s.records("user_questions")[0];
            (
                "user.answer",
                json!({"question_id":q.header.id,"generation":q.data["generation"],"answer":{"approved":true}}),
            )
        } else {
            let rule = seed_policy(&mut router, "ask");
            let s = router.snapshot().unwrap();
            let mut b = Planner::new(&s, "test.bash-rule", &json!({}));
            let mut data = s.get("intercept_rules", &rule).unwrap().data.clone();
            data["pattern"] = json!("Bash");
            b.put("intercept_rules", &rule, data);
            router.internal_commit(b.done(json!({}))).unwrap();
            assert_eq!(router.intercept_tool(&rid, &tool).unwrap(), Decision::Ask);
            let id = router.snapshot().unwrap().records("intercept_pending")[0]
                .header
                .id
                .clone();
            ("ui.interceptDecide", json!({"id":id,"decision":"allowed"}))
        };
        command(&mut router, method, params);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &rid)
                .unwrap()
                .data["state"],
            "running"
        );
        assert!(router.runtime.approval_wakes.contains(&rid));
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("user_questions")
                .iter()
                .all(|q| q.data["state"] != "pending")
        );
        router.process_approval_wakes().unwrap();
        let s = router.snapshot().unwrap();
        let invocation = s
            .records("tool_invocations")
            .into_iter()
            .find(|i| i.data["tool_use_id"] == tool.id)
            .unwrap();
        assert_eq!(invocation.data["name"], tool.name);
        assert_eq!(invocation.data["state"], "prepared");
        let deadline = Instant::now() + Duration::from_secs(2);
        while host.0.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(host.0.lock().unwrap()[0]["name"], tool.name);
        assert_eq!(host.0.lock().unwrap().len(), 1);
    }
}
#[test]
fn automatic_rule_allow_does_not_grant_permission_and_manual_grants_match_exact_call_epoch_and_deadline()
 {
    let (_dir, mut router, rid, _) = crate::regression::seeded();
    let rule = seed_policy(&mut router, "allow");
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.bash-rule", &json!({}));
    let mut data = s.get("intercept_rules", &rule).unwrap().data.clone();
    data["pattern"] = json!("Bash");
    b.put("intercept_rules", &rule, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    let tool = call("Bash", "auto-allow");
    assert_eq!(router.intercept_tool(&rid, &tool).unwrap(), Decision::Allow);
    assert_eq!(router.approval(&rid, &tool).unwrap(), Decision::Ask);
    let s = router.snapshot().unwrap();
    let approval = s.records("intercept_pending")[0].header.id.clone();
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    // An actual pending intercept correlates with the already pending permission barrier.
    let mut b = Planner::new(&s, "test.pending-intercept", &json!({}));
    let mut data = s.get("intercept_pending", &approval).unwrap().data.clone();
    data["status"] = json!("pending");
    data["audit"]["effective_action"] = json!("");
    b.put("intercept_pending", &approval, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    command(
        &mut router,
        "ui.interceptDecide",
        json!({"id":approval,"decision":"allowed"}),
    );
    let s = router.snapshot().unwrap();
    assert_eq!(s.records("user_questions")[0].data["state"], "answered");
    assert!(artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &new_id("epoch"),
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        "other-call",
        &tool.name,
        &tool.input
    ));
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        "Read",
        &tool.input
    ));
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &json!({"command":"different"})
    ));
    // Approval expiry is a response deadline, not a revocation clock: an
    // accepted decision remains authoritative if tool start happens later.
    let mut b = Planner::new(&s, "test.timely-grant", &json!({}));
    let mut data = s.get("intercept_pending", &approval).unwrap().data.clone();
    data["audit"]["timeout_enabled"] = json!(true);
    data["audit"]["deadline_at"] = json!(
        (Utc::now() - ChronoDuration::seconds(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    data["decided_at"] = json!(
        (Utc::now() - ChronoDuration::seconds(2))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    b.put("intercept_pending", &approval, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    let s = router.snapshot().unwrap();
    assert!(artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    let mut b = Planner::new(&s, "test.committed-grant-after-wait-deadline", &json!({}));
    let mut data = s.get("intercept_pending", &approval).unwrap().data.clone();
    data["audit"]["timeout_enabled"] = json!(true);
    data["decided_at"] = json!(artex_core::now());
    data["audit"]["deadline_at"] = json!(
        (Utc::now() - ChronoDuration::seconds(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    b.put("intercept_pending", &approval, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    assert!(artex_domain::approval_bypass::manual_grant(
        router.snapshot().unwrap().as_ref(),
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.expired-pending", &json!({}));
    let mut data = s.get("intercept_pending", &approval).unwrap().data.clone();
    data["status"] = json!("pending");
    data["decided_at"] = Value::Null;
    b.put("intercept_pending", &approval, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    let params = json!({"id":approval,"decision":"allowed"});
    let s = router.snapshot().unwrap();
    let user = Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: router.engine_epoch.clone(),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    };
    // A deadline alone does not decide the object. Once the timeout wins the
    // same writer/CAS race, a later manual decision cannot reverse it.
    let timeout = artex_domain::policy::expire_pending_plan(&s, &user).unwrap();
    assert!(!timeout.changes.is_empty());
    router.internal_commit(timeout).unwrap();
    let s = router.snapshot().unwrap();
    assert!(!artex_domain::approval_bypass::manual_grant(
        &s,
        &router.engine_epoch,
        &rid,
        &tool.id,
        &tool.name,
        &tool.input
    ));
    assert_eq!(
        artex_domain::policy_ui::plan("ui.interceptDecide", &params, &s, &user)
            .unwrap_err()
            .code,
        "VERSION_CONFLICT"
    );
    let mut b = Planner::new(&s, "test.old-epoch", &json!({}));
    let mut data = s.get("intercept_pending", &approval).unwrap().data.clone();
    data["audit"]["timeout_enabled"] = json!(false);
    data["status"] = json!("pending");
    b.put("intercept_pending", &approval, data);
    let mut run = s.get("agent_runs", &rid).unwrap().data.clone();
    run["engine_epoch"] = json!(new_id("epoch"));
    b.put("agent_runs", &rid, run);
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(
        artex_domain::policy_ui::plan(
            "ui.interceptDecide",
            &params,
            router.snapshot().unwrap().as_ref(),
            &user
        )
        .unwrap_err()
        .code,
        "STALE_EPOCH"
    );
}
#[test]
fn manual_intercept_cannot_auto_answer_expired_mismatched_or_human_questions_or_override_denies() {
    let (_dir, mut router, rid, _) = crate::regression::seeded();
    seed_policy(&mut router, "ask");
    let tool = call("Read", "manual-correlation");
    assert_eq!(router.intercept_tool(&rid, &tool).unwrap(), Decision::Ask);
    let s = router.snapshot().unwrap();
    let approval = s.records("intercept_pending")[0].header.id.clone();
    let mut b = Planner::new(&s, "test.correlated-questions", &json!({}));
    let mismatched = question(&mut b, &rid, "question", "tool_approval", &tool.id, false);
    let expired = question(&mut b, &rid, "question", "tool_approval", &tool.id, true);
    let human = question(&mut b, &rid, "question", "question", &tool.id, false);
    router.internal_commit(b.done(json!({}))).unwrap();
    command(
        &mut router,
        "ui.interceptDecide",
        json!({"id":approval,"decision":"allowed"}),
    );
    let s = router.snapshot().unwrap();
    for id in [&mismatched, &expired, &human] {
        assert_eq!(
            s.get("user_questions", id).unwrap().data["state"],
            "pending"
        );
    }
    assert_eq!(s.get("agent_runs", &rid).unwrap().data["state"], "waiting");
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.manual-deny", &json!({}));
    let mut data = s.get("user_questions", &mismatched).unwrap().data.clone();
    data["state"] = json!("answered");
    data["answer"] = json!({"approved":false});
    b.put("user_questions", &mismatched, data);
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(router.approval(&rid, &tool).unwrap(), Decision::Deny);
    let s = router.snapshot().unwrap();
    let mut b = Planner::new(&s, "test.current-deny", &json!({}));
    b.insert(
        "settings",
        "setting",
        json!({"key":"permissions","value":"{\"deny\":[\"Read\"]}"}),
    )
    .unwrap();
    router.internal_commit(b.done(json!({}))).unwrap();
    assert_eq!(router.approval(&rid, &tool).unwrap(), Decision::Deny);
}
#[test]
fn answered_permission_question_allows_only_its_exact_name_and_input() {
    let (_dir, mut router, rid, _) = crate::regression::seeded();
    let original = call("Bash", "permission-exact-input");
    assert_eq!(router.approval(&rid, &original).unwrap(), Decision::Ask);
    let s = router.snapshot().unwrap();
    let question = s.records("user_questions")[0];
    let params = json!({"question_id":question.header.id,"generation":question.data["generation"],"answer":{"approved":true}});
    command(&mut router, "user.answer", params);
    assert_eq!(router.approval(&rid, &original).unwrap(), Decision::Allow);
    let mut changed = original.clone();
    changed.input = json!({"command":"different-input"});
    assert_eq!(router.approval(&rid, &changed).unwrap(), Decision::Deny);
    changed.name = "Read".into();
    changed.input = json!({"file_path":"fixture.txt"});
    assert_eq!(router.approval(&rid, &changed).unwrap(), Decision::Deny);
    assert_eq!(
        router.snapshot().unwrap().records("user_questions").len(),
        1,
        "Mismatched old approvals must not mint a new grant"
    );
}
#[test]
fn enabling_bypass_cancels_only_judge_and_late_old_review_cannot_override_new_work() {
    let (_dir, mut router, rid, _) = crate::regression::seeded();
    let tool = call("Bash", "judge-call");
    router.runtime.runs.insert(rid.clone(), state(tool.clone()));
    let old_id = new_id("request");
    let old_cancel = Cancellation::default();
    router
        .runtime
        .judge_workers
        .insert(rid.clone(), (old_id.clone(), old_cancel.clone()));
    router.runtime.runs.get_mut(&rid).unwrap().in_flight = true;
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    assert!(old_cancel.cancelled());
    assert!(!router.runtime.runs[&rid].cancellation.cancelled());
    assert!(!router.runtime.runs[&rid].in_flight);
    command(
        &mut router,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":false}}),
    );
    router.runtime.approval_wakes.clear();
    let new_id = new_id("request");
    let new_cancel = Cancellation::default();
    router
        .runtime
        .judge_workers
        .insert(rid.clone(), (new_id.clone(), new_cancel.clone()));
    router.runtime.runs.get_mut(&rid).unwrap().in_flight = true;
    router
        .runtime
        .tx
        .send(Completion::Judge {
            review_id: old_id,
            run: rid.clone(),
            call: tool,
            input: json!({}),
            decision: "deny".into(),
            reason: "Late obsolete judge".into(),
            metadata: json!({}),
            model_result: Err(Error::new("CANCELLED", "Obsolete review")),
        })
        .unwrap();
    router.runtime_tick().unwrap();
    assert!(router.runtime.runs[&rid].in_flight);
    assert_eq!(router.runtime.judge_workers[&rid].0, new_id);
    assert!(!new_cancel.cancelled());
    assert!(
        router
            .snapshot()
            .unwrap()
            .records("intercept_pending")
            .is_empty()
    );
}
#[test]
fn approval_wake_never_starts_native_effect_after_deadline_pause_or_cancellation() {
    for barrier in ["deadline", "paused", "cancelled"] {
        let (_dir, mut router, rid, _) = crate::regression::seeded();
        let host = Arc::new(CapturedHost::default());
        router.attach_host(host.clone());
        seed_policy(&mut router, "ask");
        let tool = call("Read", "approved-but-inactive");
        router.runtime.runs.insert(rid.clone(), state(tool.clone()));
        assert_eq!(router.intercept_tool(&rid, &tool).unwrap(), Decision::Ask);
        let id = router.snapshot().unwrap().records("intercept_pending")[0]
            .header
            .id
            .clone();
        command(
            &mut router,
            "ui.interceptDecide",
            json!({"id":id,"decision":"allowed"}),
        );
        let s = router.snapshot().unwrap();
        let mut b = Planner::new(&s, "test.barrier", &json!({}));
        let mut run = s.get("agent_runs", &rid).unwrap().data.clone();
        match barrier {
            "deadline" => {
                run["deadline_at"] = json!(
                    (Utc::now() - ChronoDuration::seconds(1))
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                )
            }
            "cancelled" => {
                run["state"] = json!("cancelled");
                router.runtime.runs[&rid].cancellation.cancel();
            }
            _ => {
                let exploration = b.insert("explorations", "exploration", json!({})).unwrap();
                let task=b.insert("tasks","task",json!({"name":"Paused fixture","status":"running","paused":true,"exploration_id":exploration})).unwrap();
                run["task_id"] = json!(task);
                run["conversation_id"] = Value::Null;
            }
        }
        b.put("agent_runs", &rid, run);
        router.internal_commit(b.done(json!({}))).unwrap();
        router.process_approval_wakes().unwrap();
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("tool_invocations")
                .is_empty(),
            "Approved {barrier} run must not prepare an effect"
        );
        assert!(
            host.0.lock().unwrap().is_empty(),
            "Approved {barrier} run must not dispatch an effect"
        );
    }
}
