use artex_core::{Actor, Change, Context, Snapshot, body, default_data, new_id};
use artex_domain::{plan, query};
use artex_store::Store;
use serde_json::{Value, json};

fn ctx() -> Context {
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
fn apply(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = plan(m, &p, &st.snapshot(), c).unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
fn create(st: &mut Store, c: &Context, name: &str, sources: Vec<String>) -> String {
    apply(st,"task.create",json!({"description":name,"goal":"verify behavior","source_task_ids":sources,"plan_heartbeat_seconds":300}),c)["task_id"].as_str().unwrap().into()
}
fn raw_put(st: &mut Store, collection: &str, prefix: &str, patch: Value) -> String {
    let id = new_id(prefix);
    let mut data = default_data(collection).unwrap();
    artex_domain::merge(&mut data, &patch);
    if data.get("id").is_some() {
        data["id"] = json!(id);
    }
    let mut b = artex_domain::empty_batch("test.fixture", &json!({}));
    b.command_id = new_id("command");
    b.changes.push(Change::Put {
        collection: collection.into(),
        id: id.clone(),
        data,
    });
    b.result = json!({});
    st.commit(b).unwrap();
    id
}
fn raw_patch(st: &mut Store, collection: &str, id: &str, patch: Value) {
    let s = st.snapshot();
    let old = s.get(collection, id).unwrap();
    let mut data = old.data.clone();
    artex_domain::merge(&mut data, &patch);
    let mut b = artex_domain::empty_batch("test.fixture.update", &json!({}));
    b.command_id = new_id("command");
    b.expected.push(artex_core::Expected {
        collection: collection.into(),
        id: id.into(),
        version: old.header.version.clone(),
    });
    b.changes.push(Change::Put {
        collection: collection.into(),
        id: id.into(),
        data,
    });
    st.commit(b).unwrap();
}

fn config(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = artex_domain::configuration_ui::plan(m, &p, &st.snapshot(), c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"][m], &contract, &b.result)
        .unwrap_or_else(|e| panic!("{m}: {e}"));
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
#[test]
fn configuration_query_projections_validate_frozen_ui_contracts() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "query projection").unwrap();
    let c = ctx();
    let mut b = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let s = st.snapshot();
    let mid = s.records("mcp_servers")[0].header.id.clone();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.agents", json!({})),
        ("ui.getAgent", json!({"key":"auto"})),
        ("ui.agentPromptVersions", json!({"key":"auto"})),
        ("ui.agentVariables", json!({"key":"auto"})),
        ("ui.getAgentVisibility", json!({"key":"auto"})),
        ("ui.tools", json!({})),
        ("ui.mcpServers", json!({})),
        ("ui.mcpTools", json!({"id":mid})),
        ("ui.resourceVisibility", json!({"kind":"mcp","id":mid})),
        ("ui.skillVisibility", json!({"name":"test"})),
        ("ui.agentTriggers", json!({"key":"auto"})),
        ("ui.llmProfiles", json!({})),
        ("ui.llmRetryPolicy", json!({})),
        ("ui.settings", json!({})),
        (
            "ui.previewAgentPrompt",
            json!({"key":"auto","template":"At {{.Now}} in {{.DataDir}}"}),
        ),
    ] {
        let v = artex_domain::configuration_ui::query(m, &p, &s, &c).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &v)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
    }
}
#[test]
fn task_profiles_company_snapshot_interception_and_admission_are_persisted_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "task options").unwrap();
    let c = ctx();
    let secret = new_id("secret");
    let pid=config(&mut st,"ui.saveLLMProfile",json!({"p":{"name":"P","model":"test","format":"openai","api_key":{"secret_ref":secret,"hint":"test"}}}),&c)["id"].as_str().unwrap().to_owned();
    let co = apply(&mut st, "company.create", json!({"name":"Company"}), &c)["company_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let asset = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"example.com","company_id":co}),
        &c,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let t=apply(&mut st,"task.create",json!({"description":"Task","goal":"Verify","llm_profile_ids":[pid],"company_ids":[co,co],"intercept_rules":[{"enabled":true,"action":"block","kind":"exact_ip","pattern":"127.0.0.1"}]}),&c)["task_id"].as_str().unwrap().to_owned();
    let s = st.snapshot();
    assert_eq!(
        s.get("tasks", &t).unwrap().data["active_llm_profile_id"],
        pid
    );
    assert_eq!(s.records("task_llm_profiles").len(), 1);
    assert_eq!(s.records("task_scope").len(), 1);
    assert_eq!(s.records("task_intercept_rules").len(), 1);
    assert_eq!(s.records("task_asset_links").len(), 1);
    assert_eq!(
        s.get("assets", &asset).unwrap().data["task_ids"],
        json!([t])
    );
    apply(&mut st, "task.resume", json!({"task_id":t}), &c);
    assert_eq!(
        st.snapshot().get("tasks", &t).unwrap().data["status"],
        "running"
    );
    apply(&mut st, "task.pause", json!({"task_id":t}), &c);
    let rt = st.snapshot().records("task_runtime")[0].header.id.clone();
    raw_patch(
        &mut st,
        "task_runtime",
        &rt,
        json!({"accounting_state":"recovery_required"}),
    );
    assert_eq!(
        plan("task.resume", &json!({"task_id":t}), &st.snapshot(), &c)
            .unwrap_err()
            .code,
        "RECOVERY_REQUIRED"
    );
}
fn task_config(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = artex_domain::task_ui::plan(m, &p, &st.snapshot(), c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"][m], &contract, &b.result)
        .unwrap_or_else(|e| panic!("{m}: {e}"));
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
#[test]
fn categories_templates_constraints_and_rules_match_ui_shapes_and_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "metadata").unwrap();
    let c = ctx();
    let first = create(&mut st, &c, "one", vec![]);
    let second = create(&mut st, &c, "two", vec![]);
    let category = task_config(
        &mut st,
        "ui.createTaskCategory",
        json!({"name":"  Red   Team "}),
        &c,
    );
    let cid = category["id"].as_str().unwrap();
    assert_eq!(category["name"], "Red Team");
    let missing = new_id("task");
    let moved = task_config(
        &mut st,
        "ui.updateTasksCategory",
        json!({"taskIds":[first,first,second,missing],"categoryId":cid}),
        &c,
    );
    assert_eq!(moved["items"].as_array().unwrap().len(), 3);
    assert_eq!(moved["category"]["task_count"], 2);
    assert_eq!(moved["items"][2]["ok"], false);
    let template = task_config(
        &mut st,
        "ui.createTaskTemplate",
        json!({"input":{"name":"Preset","description":"Description","goal":"Goal","category_id":cid,"intercept_rules":[{"enabled":true,"kind":"exact_ip","pattern":"127.0.0.1","action":"block"}]}}),
        &c,
    );
    let templateid = template["id"].as_str().unwrap();
    task_config(
        &mut st,
        "ui.updateTaskTemplate",
        json!({"id":templateid,"input":{"category_id":null,"goal":"Revised goal"}}),
        &c,
    );
    let con = task_config(
        &mut st,
        "ui.addConstraint",
        json!({"id":first,"text":"Only passive checks","kind":"deny"}),
        &c,
    );
    let conid = con["id"].as_str().unwrap();
    assert_eq!(
        artex_domain::task_ui::plan(
            "ui.deleteConstraint",
            &json!({"id":second,"constraintId":conid}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
    task_config(
        &mut st,
        "ui.updateConstraint",
        json!({"id":first,"constraintId":conid,"text":"Authorized tests","kind":"allow"}),
        &c,
    );
    let rule = task_config(
        &mut st,
        "ui.createTaskInterceptRule",
        json!({"id":first,"rule":{"enabled":true,"kind":"exact_ip","pattern":"127.0.0.1","action":"block"}}),
        &c,
    );
    let rid = rule["id"].as_str().unwrap();
    task_config(
        &mut st,
        "ui.toggleTaskInterceptRule",
        json!({"id":first,"ruleId":rid,"enabled":false}),
        &c,
    );
    assert_eq!(
        artex_domain::task_ui::plan(
            "ui.deleteTaskInterceptRule",
            &json!({"id":second,"ruleId":rid}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
    task_config(&mut st, "ui.deleteTaskCategory", json!({"id":cid}), &c);
    assert!(st.snapshot().get("tasks", &first).unwrap().data["category_id"].is_null());
    assert!(
        st.snapshot()
            .get("task_templates", templateid)
            .unwrap()
            .data["category_id"]
            .is_null()
    );
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.taskCategories", json!({})),
        ("ui.taskTemplates", json!({})),
        ("ui.taskConstraints", json!({"id":first})),
        ("ui.taskInterceptRules", json!({"id":first})),
        ("ui.assetInterceptRules", json!({})),
    ] {
        let result = artex_domain::task_ui::query(m, &p, &st.snapshot(), &c).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &result)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
    }
}
#[test]
fn defaults_are_atomic_and_preserve_user_configuration_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "defaults").unwrap();
    let c = ctx();
    let mut b = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    assert_eq!(st.snapshot().records("agents").len(), 8);
    assert_eq!(st.snapshot().records("tools").len(), 81);
    for key in [
        "AskUserQuestion",
        "EnterPlanMode",
        "ExitPlanMode",
        "RecallMemory",
        "RecordMemory",
    ] {
        assert_eq!(
            st.snapshot()
                .records("tools")
                .iter()
                .find(|r| r.data["key"] == key)
                .unwrap()
                .data["enabled"],
            false
        );
    }
    let a = artex_domain::conversation::agent_by_key(&st.snapshot(), "auto")
        .unwrap()
        .header
        .id
        .clone();
    config(
        &mut st,
        "ui.saveAgentPrompt",
        json!({"key":"auto","template":"Use {{.Now}} to verify the current time.","note":"edited"}),
        &c,
    );
    config(
        &mut st,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"interactive_shell":false,"max_turns":7}}),
        &c,
    );
    let b = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    assert!(b.changes.is_empty());
    assert!(b.events.is_empty());
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert_eq!(
        st.snapshot().get("agents", &a).unwrap().data["max_turns"],
        7
    );
    let d = artex_domain::configuration_ui::query(
        "ui.getAgent",
        &json!({"key":"auto"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(
        d["prompt"]["text"],
        "Use {{.Now}} to verify the current time."
    );
    assert_eq!(d["versions"].as_array().unwrap().len(), 2);
}
#[test]
fn custom_agent_profile_visibility_and_partial_configuration_have_source_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "config").unwrap();
    let c = ctx();
    let a = config(
        &mut st,
        "ui.createAgent",
        json!({"key":"reviewer","name":"검토","description":"test"}),
        &c,
    );
    let aid = a["id"].as_str().unwrap();
    let secret = new_id("secret");
    let prof = config(
        &mut st,
        "ui.saveLLMProfile",
        json!({"p":{"name":"Test","format":"openai","model":"model","api_key":{"secret_ref":secret,"hint":"…test"},"api_key_hint":"…test","max_tokens":-3,"retry":{"connect":{"attempts":99}}}}),
        &c,
    );
    let pid = prof["id"].as_str().unwrap();
    config(
        &mut st,
        "ui.saveAgentConfig",
        json!({"key":"reviewer","patch":{"llm_profile_id":pid,"max_turns":9,"trigger_merge_mode":"by_task"}}),
        &c,
    );
    config(
        &mut st,
        "ui.saveAgentConfig",
        json!({"key":"reviewer","patch":{"web_search":true}}),
        &c,
    );
    let snap = st.snapshot();
    let data = &snap.get("agents", aid).unwrap().data;
    assert_eq!(data["max_turns"], 9);
    assert_eq!(data["trigger_merge_mode"], "by_task");
    let mcp = config(
        &mut st,
        "ui.saveMcpServer",
        json!({"m":{"name":"browser","transport":"stdio","command":"test","env":{},"args":[]}}),
        &c,
    );
    let mid = mcp["id"].as_str().unwrap();
    config(
        &mut st,
        "ui.setAgentVisibility",
        json!({"key":"reviewer","mcp":[mid],"skill":["review"]}),
        &c,
    );
    config(
        &mut st,
        "ui.setAgentVisibility",
        json!({"key":"reviewer","mcp":[mid],"skill":[]}),
        &c,
    );
    let visible = artex_domain::configuration_ui::query(
        "ui.getAgentVisibility",
        &json!({"key":"reviewer"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(visible, json!({"mcp":[mid],"skill":[]}));
    let profiles =
        artex_domain::configuration_ui::query("ui.llmProfiles", &json!({}), &st.snapshot(), &c)
            .unwrap();
    assert!(profiles[0].get("api_key").is_none());
    assert_eq!(profiles[0]["retry"]["connect"]["attempts"], 20);
    config(
        &mut st,
        "ui.saveLLMProfile",
        json!({"p":{"id":pid,"name":"Edited","format":"openai","model":"next","api_key":""}}),
        &c,
    );
    assert_eq!(
        st.snapshot().get("llm_profiles", pid).unwrap().data["api_key"]["secret_ref"],
        secret
    );
    config(&mut st, "ui.activateLLMProfile", json!({"id":pid}), &c);
    assert_eq!(
        artex_domain::configuration_ui::plan(
            "ui.deleteLLMProfile",
            &json!({"id":pid}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "VERSION_CONFLICT"
    );
    config(&mut st, "ui.deleteAgent", json!({"key":"reviewer"}), &c);
    assert!(st.snapshot().get("agents", aid).is_none());
    assert!(st.snapshot().records("agent_prompts").is_empty());
    assert!(st.snapshot().records("agent_visibility").is_empty());
}

#[test]
fn conversation_admission_transcript_completion_and_cursor_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "conversations").unwrap();
    let c = ctx();
    raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"auto","name":"Auto","enabled":true}),
    );
    raw_put(
        &mut st,
        "llm_profiles",
        "profile",
        json!({"name":"Default","model":"fixture-model","is_default":true,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}}),
    );
    let conv = apply(
        &mut st,
        "conversation.create",
        json!({"agent_key":"auto"}),
        &c,
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = st.snapshot().commit_seq;
    apply(
        &mut st,
        "conversation.send",
        json!({"id":conv,"message":"첫 번째 요청"}),
        &c,
    );
    let s = st.snapshot();
    assert_eq!(s.commit_seq, before + 1);
    let run = s.records("agent_runs")[0].header.id.clone();
    assert_eq!(s.get("agent_runs", &run).unwrap().data["state"], "queued");
    assert_eq!(s.records("conversation_activities").len(), 1);
    assert!(
        s.records("domain_events")
            .iter()
            .any(|event| event.data["kind"] == "RunQueued" && event.data["subject_id"] == run)
    );
    assert_eq!(
        plan(
            "conversation.send",
            &json!({"id":conv,"message":"duplicate while running"}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "BUSY"
    );
    raw_patch(
        &mut st,
        "agent_runs",
        &run,
        json!({"state":"running","started_at":artex_core::now()}),
    );
    let mut b = artex_domain::conversation::append_activity(
        &st.snapshot(),
        &c,
        &run,
        "text",
        "답변 중",
        &body("partial output"),
        None,
    )
    .unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let mut b = artex_domain::conversation::finish_run(
        &st.snapshot(),
        &c,
        &run,
        "completed",
        &body("final output"),
        Some(&json!({"input_tokens":100,"output_tokens":20})),
    )
    .unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let page = query(
        "conversation.messages",
        &json!({"id":conv,"history":true,"limit":2}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["hasMore"], true);
    assert_eq!(page["running"], false);
    let last = page["cursor"].as_str().unwrap().to_owned();
    assert_eq!(
        query(
            "conversation.detail",
            &json!({"id":conv,"seq":last}),
            &st.snapshot(),
            &c
        )
        .unwrap()["text"],
        "final output"
    );
    let count = st.snapshot().records("conversation_activities").len();
    let b = artex_domain::conversation::finish_run(
        &st.snapshot(),
        &c,
        &run,
        "completed",
        &body("duplicate"),
        None,
    )
    .unwrap();
    assert!(b.changes.is_empty());
    assert_eq!(
        st.snapshot().records("conversation_activities").len(),
        count
    );
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert_eq!(
        st.snapshot().get("agent_runs", &run).unwrap().data["state"],
        "completed"
    );
    assert_eq!(
        query(
            "conversation.messages",
            &json!({"id":conv,"since":last}),
            &st.snapshot(),
            &c
        )
        .unwrap()["items"],
        json!([])
    );
}

#[test]
fn conversation_stop_and_batch_delete_are_scoped_and_report_missing_items() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "conversation control").unwrap();
    let c = ctx();
    raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"auto","name":"Auto","enabled":true}),
    );
    raw_put(
        &mut st,
        "llm_profiles",
        "profile",
        json!({"name":"Default","model":"fixture-model","is_default":true,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}}),
    );
    let first = apply(
        &mut st,
        "conversation.create",
        json!({"agent_key":"auto","title":"first"}),
        &c,
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = apply(
        &mut st,
        "conversation.create",
        json!({"agent_key":"auto","title":"second"}),
        &c,
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for id in [&first, &second] {
        apply(
            &mut st,
            "conversation.send",
            json!({"id":id,"message":"run"}),
            &c,
        );
    }
    assert_eq!(
        apply(&mut st, "conversation.stop", json!({"id":first}), &c)["status"],
        "stopping"
    );
    let s = st.snapshot();
    assert!(
        s.records("agent_runs")
            .iter()
            .any(|r| r.data["conversation_id"] == first && r.data["state"] == "cancelled")
    );
    assert!(
        s.records("agent_runs")
            .iter()
            .any(|r| r.data["conversation_id"] == second && r.data["state"] == "queued")
    );
    let missing = new_id("conversation");
    let result = apply(
        &mut st,
        "conversation.delete_batch",
        json!({"ids":[first,first,missing]}),
        &c,
    );
    assert_eq!(result["items"].as_array().unwrap().len(), 2);
    assert_eq!(result["items"][0]["ok"], true);
    assert_eq!(result["items"][1]["ok"], false);
    assert!(
        st.snapshot()
            .records("conversation_activities")
            .iter()
            .all(|row| row.data["conversation_id"] == second)
    );
}

#[test]
fn inherited_worker_output_is_hidden_until_terminal_and_hidden_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "output visibility").unwrap();
    let mut source_context = ctx();
    let source = create(&mut st, &source_context, "source", vec![]);
    let target = create(&mut st, &source_context, "target", vec![source.clone()]);
    source_context.task_id = Some(source);
    let intent = apply(
        &mut st,
        "add_intent",
        json!({"summary":"investigating"}),
        &source_context,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"intermediate result", "intent_id":intent}),
        &source_context,
    )["ids"][0]
        .clone();
    let mut target_context = source_context.clone();
    target_context.task_id = Some(target);
    let parameters = json!({"intent_id":intent});
    assert_eq!(
        query(
            "get_worker_output",
            &parameters,
            &st.snapshot(),
            &source_context
        )
        .unwrap()["results"][0]["id"],
        fact
    );
    for state in [
        "open",
        "running",
        "done",
        "blocked",
        "exhausted",
        "stopped",
        "open",
    ] {
        raw_patch(
            &mut st,
            "exploration_nodes",
            &intent,
            json!({"state":state}),
        );
        let result = query(
            "get_worker_output",
            &parameters,
            &st.snapshot(),
            &target_context,
        );
        if ["done", "blocked", "exhausted", "stopped"].contains(&state) {
            assert_eq!(result.unwrap()["results"][0]["id"], fact, "{state}");
        } else {
            assert_eq!(result.unwrap_err().code, "PERMISSION_DENIED", "{state}");
        }
    }
}

#[test]
fn deleting_finding_removes_canonical_graph_node_and_cascades_with_cas() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "finding deletion").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let intent = apply(
        &mut st,
        "add_intent",
        json!({"summary":"test vulnerability"}),
        &c,
    )["ids"][0]
        .clone();
    let goal = apply(
        &mut st,
        "set_goals",
        json!({"goals":[{"text":"prove vulnerability"}]}),
        &c,
    )["ids"][0]
        .clone();
    let asset = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"ip","ip":"192.0.2.9"}]}),
        &c,
    )["ids"][0]
        .clone();
    let finding = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"injection","severity":"high","summary":"confirmed","intent_id":intent,"asset_ids":[asset]}),
        &c,
    );
    let fid = finding["finding_id"].as_str().unwrap();
    let node = finding["node_id"].as_str().unwrap();
    let s = st.snapshot();
    let exp = s.get("tasks", c.task_id.as_deref().unwrap()).unwrap().data["exploration_id"].clone();
    let anchor = raw_put(
        &mut st,
        "exploration_anchors",
        "anchor",
        json!({"node_id":node,"asset_id":asset}),
    );
    let proof = raw_put(
        &mut st,
        "exploration_edges",
        "edge",
        json!({"exploration_id":exp,"src_id":node,"dst_id":goal,"rel":"proves"}),
    );
    let activity = raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"node_id":node,"kind":"tool_result","detail":body("finding recorded")}),
    );
    let s = st.snapshot();
    let mut batch = plan("finding.delete", &json!({"id":fid}), &s, &c).unwrap();
    for (collection, id) in [
        ("findings", fid),
        ("exploration_nodes", node),
        ("exploration_anchors", &anchor),
        ("exploration_edges", &proof),
        ("activity", &activity),
    ] {
        assert!(
            batch
                .expected
                .iter()
                .any(|e| e.collection == collection && e.id == id),
            "{collection}/{id}"
        );
    }
    batch.command_id = new_id("command");
    st.commit(batch).unwrap();
    let s = st.snapshot();
    assert!(s.get("findings", fid).is_none());
    assert!(s.get("exploration_nodes", node).is_none());
    assert!(s.get("exploration_anchors", &anchor).is_none());
    assert!(
        s.records("exploration_edges")
            .iter()
            .all(|r| r.data["src_id"] != node && r.data["dst_id"] != node)
    );
    assert_eq!(
        s.get("activity", &activity).unwrap().data["node_id"],
        Value::Null
    );
    assert!(
        s.get("exploration_nodes", intent.as_str().unwrap())
            .is_some()
    );
    assert!(s.get("exploration_nodes", goal.as_str().unwrap()).is_some());
}

#[test]
fn staged_company_does_not_steal_assets_from_existing_equal_scope() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "company ordering").unwrap();
    let c = ctx();
    let old = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"Old","scope":"example.com"}),
        &c,
    )["company_id"]
        .clone();
    let asset = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"root_domain","domain":"example.com"}]}),
        &c,
    )["ids"][0]
        .clone();
    assert_eq!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        old
    );
    let new = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"New","scope":"example.com"}),
        &c,
    )["company_id"]
        .clone();
    assert_ne!(old, new);
    assert_eq!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        old
    );
}

#[test]
fn company_delete_atomically_reassigns_derived_and_detached_explicit_assets() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "company deletion").unwrap();
    let c = ctx();
    let old = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"Old","scope":"example.com"}),
        &c,
    )["company_id"]
        .clone();
    let automatic = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"root_domain","domain":"example.com"}]}),
        &c,
    )["ids"][0]
        .clone();
    let detached = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"subdomain","domain":"a.example.com","company_id":old}]}),
        &c,
    )["ids"][0]
        .clone();
    let unmatched = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"ip","ip":"198.51.100.4","company_id":old}]}),
        &c,
    )["ids"][0]
        .clone();
    let other = apply(
        &mut st,
        "company.create",
        json!({"name":"Unrelated explicit owner"}),
        &c,
    )["company_id"]
        .clone();
    let sticky = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"subdomain","domain":"b.example.com","company_id":other}]}),
        &c,
    )["ids"][0]
        .clone();
    let new = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"New","scope":"example.com"}),
        &c,
    )["company_id"]
        .clone();
    let before = st.snapshot();
    assert_eq!(
        before
            .get("assets", automatic.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        old
    );
    let old_scope = before
        .records("company_scope")
        .into_iter()
        .find(|r| r.data["company_id"] == old)
        .unwrap()
        .header
        .id
        .clone();
    let mut batch = plan("company.delete", &json!({"id":old}), &before, &c).unwrap();
    assert!(
        batch
            .expected
            .iter()
            .any(|e| e.collection == "company_scope" && e.id == old_scope)
    );
    assert!(
        batch
            .expected
            .iter()
            .any(|e| e.collection == "companies" && Some(e.id.as_str()) == new.as_str())
    );
    batch.command_id = new_id("command");
    st.commit(batch).unwrap();
    let s = st.snapshot();
    assert_eq!(s.commit_seq, before.commit_seq + 1);
    assert!(s.get("companies", old.as_str().unwrap()).is_none());
    assert!(
        s.records("company_scope")
            .iter()
            .all(|r| r.data["company_id"] != old)
    );
    for id in [&automatic, &detached] {
        let row = &s.get("assets", id.as_str().unwrap()).unwrap().data;
        assert_eq!(row["company_id"], new);
        assert_eq!(row["company_source"], "scope");
    }
    let unmatched = &s.get("assets", unmatched.as_str().unwrap()).unwrap().data;
    assert_eq!(unmatched["company_id"], Value::Null);
    assert_eq!(unmatched["company_source"], "explicit");
    let sticky = &s.get("assets", sticky.as_str().unwrap()).unwrap().data;
    assert_eq!(sticky["company_id"], other);
    assert_eq!(sticky["company_source"], "explicit");
}

#[test]
fn app_icp_insert_update_scope_recompute_and_deleted_explicit_owner_match_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "app ICP").unwrap();
    let c = ctx();
    let first = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"ICP first","scope":"ICP 100"}),
        &c,
    )["company_id"]
        .clone();
    let second = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"ICP second","scope":"ICP 200"}),
        &c,
    )["company_id"]
        .clone();
    let app = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":" IcP \t100 "}]}),
        &c,
    )["ids"][0]
        .clone();
    let s = st.snapshot();
    let row = &s.get("assets", app.as_str().unwrap()).unwrap().data;
    assert_eq!(row["app_icp"], " IcP \t100 ");
    assert_eq!(row["icp"], Value::Null);
    assert_eq!(row["company_id"], first);
    let updated = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":"ICP 200"}]}),
        &c,
    )["ids"][0]
        .clone();
    assert_eq!(updated, app);
    assert_eq!(
        st.snapshot()
            .get("assets", app.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        second
    );
    apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":"ICP unmatched"}]}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("assets", app.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        second
    );
    apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":"ICP 200"}]}),
        &c,
    );
    apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":""}]}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("assets", app.as_str().unwrap())
            .unwrap()
            .data["app_icp"],
        "ICP 200"
    );
    apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","company_id":first}]}),
        &c,
    );
    apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Example app","icp":"ICP 200"}]}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("assets", app.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        first
    );
    apply(&mut st, "company.delete", json!({"id":first}), &c);
    let s = st.snapshot();
    let row = &s.get("assets", app.as_str().unwrap()).unwrap().data;
    assert_eq!(row["company_id"], second);
    assert_eq!(row["company_source"], "scope");
    let late = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"app","app_name":"Late attribution","icp":"ICP 300"}]}),
        &c,
    )["ids"][0]
        .clone();
    assert_eq!(
        st.snapshot()
            .get("assets", late.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        Value::Null
    );
    let third = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"ICP third","scope":"ICP 300"}),
        &c,
    )["company_id"]
        .clone();
    assert_eq!(
        st.snapshot()
            .get("assets", late.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        third
    );
}

#[test]
fn finding_update_mirrors_node_payload_in_the_same_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "finding sync").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let finding = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"injection","severity":"high","summary":"old"}),
        &c,
    );
    let fid = finding["finding_id"].as_str().unwrap();
    let node = finding["node_id"].as_str().unwrap();
    let patch =
        json!({"name":"new name","severity":"low","summary":"new summary","evidence":"new proof"});
    let s = st.snapshot();
    let planned = plan("finding.update", &json!({"id":fid,"patch":patch}), &s, &c).unwrap();
    assert!(
        planned
            .expected
            .iter()
            .any(|e| e.collection == "exploration_nodes" && e.id == node)
    );
    apply(
        &mut st,
        "finding.update",
        json!({"id":fid,"patch":patch}),
        &c,
    );
    let s = st.snapshot();
    let row = &s.get("findings", fid).unwrap().data;
    let payload = &s.get("exploration_nodes", node).unwrap().data["payload"];
    for field in ["name", "severity", "summary"] {
        assert_eq!(row[field], patch[field]);
        assert_eq!(payload[field], patch[field]);
    }
    assert_eq!(row["evidence"]["text"], patch["evidence"]);
    assert_eq!(payload["evidence"], patch["evidence"]);
    assert_eq!(row["evidence_version"], "1");
    let version = s
        .get("exploration_nodes", node)
        .unwrap()
        .header
        .version
        .clone();
    apply(
        &mut st,
        "finding.update",
        json!({"id":fid,"patch":{"status":"confirmed"}}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("exploration_nodes", node)
            .unwrap()
            .header
            .version,
        version
    );
}

#[test]
fn task_creation_is_complete_atomic_and_heartbeat_matches_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "domain").unwrap();
    let c = ctx();
    let id = create(&mut st, &c, "test", vec![]);
    let s = st.snapshot();
    let task = s.get("tasks", &id).unwrap();
    assert_eq!(task.data["plan_heartbeat_seconds"], 600);
    assert_eq!(task.data["status"], "created");
    let exp = task.data["exploration_id"].as_str().unwrap();
    assert_eq!(
        s.records("exploration_nodes")
            .iter()
            .filter(|n| n.data["exploration_id"] == exp)
            .count(),
        2
    );
    assert!(
        s.records("task_runtime")
            .iter()
            .any(|r| r.data["task_id"] == id)
    );
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert!(st.snapshot().get("tasks", &id).is_some());
}

#[test]
fn live_shared_sources_are_one_level_and_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "sharing").unwrap();
    let c = ctx();
    let a = create(&mut st, &c, "a", vec![]);
    let b = create(&mut st, &c, "b", vec![a.clone()]);
    let d = create(&mut st, &c, "c", vec![b.clone()]);
    let mut ca = c.clone();
    ca.task_id = Some(a.clone());
    let fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"A secret observation"}),
        &ca,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let mut cb = c.clone();
    cb.task_id = Some(b.clone());
    let mut cd = c.clone();
    cd.task_id = Some(d);
    assert!(
        query("node_detail", &json!({"id":fact}), &st.snapshot(), &cb).unwrap()["read_only"]
            == true
    );
    assert_eq!(
        query("node_detail", &json!({"id":fact}), &st.snapshot(), &cd)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert_eq!(
        plan(
            "record_fact",
            &json!({"summary":"cannot write source","intent_id":fact}),
            &st.snapshot(),
            &cb
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    let mut data = st.snapshot().get("tasks", &a).unwrap().data.clone();
    data["archived_at"] = json!(artex_core::now());
    let mut batch = artex_domain::empty_batch("test.archive", &json!({}));
    batch.command_id = new_id("command");
    batch.changes.push(Change::Put {
        collection: "tasks".into(),
        id: a,
        data,
    });
    artex_domain::augment_expected(&mut batch, &st.snapshot()).unwrap();
    st.commit(batch).unwrap();
    assert_eq!(
        query("node_detail", &json!({"id":fact}), &st.snapshot(), &cb)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    cd.task_id = None;
}

#[test]
fn assets_merge_natural_keys_and_preserve_exact_domain_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "assets").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "target", vec![]);
    let mut c = c;
    c.task_id = Some(task);
    let a=apply(&mut st,"insert_assets",json!({"assets":[{"type":"subdomain","domain":" Example.COM. ","record_type":"A","record_value":["1.2.3.4"]}]}),&c)["ids"][0].clone();
    let b=apply(&mut st,"insert_assets",json!({"assets":[{"type":"subdomain","domain":"example.com","record_type":"A","record_value":["1.2.3.5"]}]}),&c)["ids"][0].clone();
    assert_eq!(a, b);
    let s = st.snapshot();
    assert_eq!(s.records("assets").len(), 4);
    assert_eq!(
        s.get("assets", a.as_str().unwrap()).unwrap().data["record_value"],
        json!(["1.2.3.4", "1.2.3.5"])
    );
    assert_eq!(s.records("task_asset_links").len(), 4);
    assert_eq!(artex_domain::assets::domain_key("A.."), "a.");
    assert_eq!(
        artex_domain::assets::normalize_url("HTTPS://EXAMPLE.COM/").unwrap(),
        "https://example.com"
    );
    assert!(
        artex_domain::assets::normalize_url("https://example.com/?a=1")
            .unwrap()
            .contains("/?a=1")
    );
}

#[test]
fn finding_report_requires_node_identity_and_evidence_cas() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "finding").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let finding = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"IDOR","severity":"high","summary":"unauthorized object access","evidence":"verified response"}),
        &c,
    );
    let node = finding["node_id"].as_str().unwrap();
    let fid = finding["finding_id"].as_str().unwrap();
    assert_ne!(node, fid);
    assert_eq!(
        plan(
            "update_finding_report",
            &json!({"finding_node_id":fid,"report":"report","evidence_version":"0"}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    apply(
        &mut st,
        "update_finding_report",
        json!({"finding_node_id":node,"report":"Initial report","evidence_version":"0"}),
        &c,
    );
    apply(
        &mut st,
        "finding.update",
        json!({"id":fid,"patch":{"evidence":"new verified response"}}),
        &c,
    );
    assert_eq!(
        plan(
            "update_finding_report",
            &json!({"finding_node_id":node,"report":"stale report","evidence_version":"0"}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "EVIDENCE_CONFLICT"
    );
    let view = query(
        "get_finding_traffic",
        &json!({"finding_id":fid}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(view["report_stale"], true);
    assert_eq!(
        st.snapshot().get("findings", fid).unwrap().data["report"],
        body("Initial report")
    );
}

#[test]
fn retest_is_conversation_bound_and_fixed_disposition_is_atomic() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "retest").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let finding = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"IDOR","severity":"medium","summary":"access","evidence":"proof"}),
        &c,
    );
    let fid = finding["finding_id"].as_str().unwrap();
    let rt = apply(
        &mut st,
        "finding.retest.create",
        json!({"finding_id":fid}),
        &c,
    );
    assert_eq!(
        plan(
            "record_finding_retest_result",
            &json!({"verdict":"fixed","summary":"gone","evidence":"403"}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    let mut rc = c.clone();
    rc.conversation_id = Some(rt["conversation_id"].as_str().unwrap().into());
    apply(
        &mut st,
        "record_finding_retest_result",
        json!({"verdict":"fixed","summary":"blocked now","evidence":"403 on same request"}),
        &rc,
    );
    let s = st.snapshot();
    assert_eq!(s.get("findings", fid).unwrap().data["status"], "fixed");
    let re = s
        .get("finding_retests", rt["retest_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(re.data["verdict"], "fixed");
    assert_eq!(
        s.get("findings", fid).unwrap().header.commit_seq,
        re.header.commit_seq
    );
}

#[test]
fn dsl_evaluates_boolean_precedence_numeric_and_array_fields() {
    use artex_domain::exploration::Dsl;
    let d = json!({"port":443,"technologies":["Nginx"],"domain":"Example.COM"});
    assert!(
        Dsl::parse("(port==80 OR port==443) AND technology=nginx")
            .unwrap()
            .matches(&d)
            .unwrap()
    );
    assert!(
        !Dsl::parse("port<80 OR domain==other")
            .unwrap()
            .matches(&d)
            .unwrap()
    );
    assert!(
        Dsl::parse("domain=example AND port >= 400")
            .unwrap()
            .matches(&d)
            .unwrap()
    );
    assert!(Dsl::parse("type=ip").is_err());
    assert!(Dsl::parse("port>garbage").unwrap().matches(&d).is_err());
}

#[test]
fn retest_cancellation_uses_finished_at_and_stops_owned_session_atomically() {
    for method in ["task.pause", "conversation.stop"] {
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::create(dir.path(), "retest cancellation").unwrap();
        let mut c = ctx();
        let task = create(&mut st, &c, "target", vec![]);
        c.task_id = Some(task.clone());
        let finding = apply(
            &mut st,
            "report_finding",
            json!({"vulnclass":"IDOR","severity":"medium","summary":"target issue","evidence":"proof"}),
            &c,
        );
        let rt = apply(
            &mut st,
            "finding.retest.create",
            json!({"finding_id":finding["finding_id"]}),
            &c,
        );
        let agent = raw_put(
            &mut st,
            "agents",
            "agent",
            json!({"key":"retester","name":"Retester"}),
        );
        let run = raw_put(
            &mut st,
            "agent_runs",
            "run",
            json!({"agent_id":agent,"task_id":task,"conversation_id":rt["conversation_id"],"state":"waiting","engine_epoch":c.actor.engine_epoch}),
        );
        raw_patch(
            &mut st,
            "finding_retests",
            rt["retest_id"].as_str().unwrap(),
            json!({"status":"running"}),
        );
        let question = raw_put(
            &mut st,
            "user_questions",
            "question",
            json!({"run_id":run,"kind":"question","state":"pending","payload":{"questions":[{"question":"Continue?"}]},"generation":"1"}),
        );
        let params = if method == "task.pause" {
            json!({"task_id":task})
        } else {
            json!({"id":rt["conversation_id"]})
        };
        apply(&mut st, method, params, &c);
        let s = st.snapshot();
        let re = s
            .get("finding_retests", rt["retest_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(re.data["status"], "stopped");
        assert!(
            re.data["finished_at"]
                .as_str()
                .is_some_and(|t| !t.is_empty())
        );
        assert!(re.data.get("completed_at").is_none());
        assert_eq!(
            s.get("agent_runs", &run).unwrap().data["state"],
            "cancelled"
        );
        assert_eq!(
            s.get("user_questions", &question).unwrap().data["state"],
            "cancelled"
        );
        assert_eq!(
            re.header.commit_seq,
            s.get("agent_runs", &run).unwrap().header.commit_seq
        );
        st.checkpoint().unwrap();
        drop(st);
        let recovered = Store::open(dir.path()).unwrap().snapshot();
        assert_eq!(
            recovered
                .get("finding_retests", rt["retest_id"].as_str().unwrap())
                .unwrap()
                .data["status"],
            "stopped"
        );
    }
}

#[test]
fn sdk_memory_unicode_names_and_plan_approval_require_user() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "sdk").unwrap();
    let mut c = ctx();
    let agent = raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"mainagent","name":"Main"}),
    );
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"engine_epoch":c.actor.engine_epoch,"state":"running","permission_mode":"default","approved_target_mode":"default","todos":[]}),
    );
    c.actor.kind = "agent".into();
    c.actor.agent_id = Some(agent);
    c.actor.run_id = Some(run.clone());
    let mem = apply(
        &mut st,
        "RecordMemory",
        json!({"name":"한국어 기억 제목","content":"공유 내용"}),
        &c,
    );
    assert!(mem["id"].as_str().unwrap().starts_with("memory_"));
    apply(
        &mut st,
        "TodoWrite",
        json!({"todos":[{"content":"검증","status":"in_progress"}]}),
        &c,
    );
    apply(&mut st, "EnterPlanMode", json!({}), &c);
    let q = apply(&mut st, "ExitPlanMode", json!({"plan":"Do work"}), &c);
    assert_eq!(
        st.snapshot().get("agent_runs", &run).unwrap().data["permission_mode"],
        "plan"
    );
    let response = json!({"question_id":q["question_id"],"generation":q["generation"],"answer":{"approved":true}});
    assert_eq!(
        plan("question.answer", &response, &st.snapshot(), &c)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    let mut user = c.clone();
    user.actor.kind = "user".into();
    apply(&mut st, "question.answer", response, &user);
    assert_eq!(
        st.snapshot().get("agent_runs", &run).unwrap().data["permission_mode"],
        "default"
    );
}

#[test]
fn deferred_discovery_respects_exact_select_binding_enabled_and_mcp_visibility() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "deferred discovery").unwrap();
    let mut c = ctx();
    let agent = raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"worker","name":"Worker"}),
    );
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"state":"running","engine_epoch":c.actor.engine_epoch}),
    );
    c.actor.kind = "agent".into();
    c.actor.agent_id = Some(agent.clone());
    c.actor.run_id = Some(run);
    for (key, enabled, agents, deferred) in [
        ("probe", true, vec!["worker"], true),
        ("probe_extra", true, vec!["worker"], true),
        ("hidden", true, vec!["other"], true),
        ("disabled", false, vec!["worker"], true),
        ("direct", true, vec!["worker"], false),
    ] {
        raw_put(
            &mut st,
            "tools",
            "toolconfig",
            json!({"key":key,"name":key,"enabled":enabled,"system":false,"kind":"command","deferred":deferred,"agents":agents,"description":"endpoint verify","schema":{"type":"object","properties":{}}}),
        );
    }
    let server = raw_put(
        &mut st,
        "mcp_servers",
        "mcp",
        json!({"name":"scanner","enabled":true}),
    );
    raw_put(
        &mut st,
        "mcp_tools_cache",
        "mcptool",
        json!({"server_id":server,"tool_name":"scan","description":"endpoint verify","schema":{"type":"object","properties":{}}}),
    );
    let exact = query(
        "SearchExtraTools",
        &json!({"query":"select:probe"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(exact.as_array().unwrap().len(), 1);
    assert_eq!(exact[0]["name"], "probe");
    let before = query(
        "SearchExtraTools",
        &json!({"query":"endpoint verify"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(before.as_array().unwrap().len(), 2);
    raw_put(
        &mut st,
        "agent_visibility",
        "visibility",
        json!({"agent_id":agent,"resource_kind":"mcp","resource_id":server,"enabled":true}),
    );
    let after = query(
        "SearchExtraTools",
        &json!({"query":"select:probe,mcp__scanner__scan,hidden,disabled,direct"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(after.as_array().unwrap().len(), 2);
    assert_eq!(after[1]["name"], "mcp__scanner__scan");
    raw_patch(&mut st, "mcp_servers", &server, json!({"enabled":false}));
    assert!(
        query(
            "SearchExtraTools",
            &json!({"query":"select:mcp__scanner__scan"}),
            &st.snapshot(),
            &c
        )
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty()
    );
}

#[test]
fn unsupported_never_returns_fake_success() {
    let s = Snapshot {
        workspace_id: new_id("workspace"),
        commit_seq: 0,
        collections: Default::default(),
    };
    assert!(plan("not_existing", &json!({}), &s, &ctx()).is_err());
}

fn bootstrap_commit(st: &mut Store, c: &Context) {
    let mut batch = artex_domain::bootstrap_plan(&st.snapshot(), c).unwrap();
    batch.command_id = new_id("command");
    st.commit(batch).unwrap();
}
fn broken_reporter_default() -> String {
    let repairs: Value =
        serde_json::from_str(include_str!("../resources/builtin-prompt-repairs.v1.json")).unwrap();
    repairs["repairs"][0]["old_template_text"]
        .as_str()
        .unwrap()
        .to_owned()
}
#[test]
fn reporter_fresh_preset_has_full_source_prompt_and_one_time_trigger_policy() {
    use sha2::{Digest, Sha256};
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "full reporter preset").unwrap();
    let c = ctx();
    bootstrap_commit(&mut st, &c);
    let s = st.snapshot();
    let reporter = s
        .records("agents")
        .into_iter()
        .find(|a| a.data["key"] == "reporter")
        .unwrap();
    assert_eq!(reporter.data["builtin"], false);
    assert_eq!(reporter.data["trigger_run_mode"], "parallel");
    assert_eq!(reporter.data["trigger_merge_mode"], "none");
    assert_eq!(reporter.data["trigger_max_parallel"], 5);
    let prompt = s
        .get(
            "agent_prompts",
            reporter.data["current_prompt_id"].as_str().unwrap(),
        )
        .unwrap();
    let full = prompt.data["template_text"]["text"].as_str().unwrap();
    assert_eq!(full.chars().count(), 5195);
    assert_eq!(
        format!("{:x}", Sha256::digest(full.as_bytes())),
        "deda052fd741f38d29a797c138c2b95481d06752384ce9378cefcc4f657b8249"
    );
    assert_eq!(
        Some(full.to_owned()),
        artex_domain::bootstrap::prompt_default("reporter")
    );
    assert!(full.contains("`## PoC`"));
    assert!(full.contains("━━ discipline ━━"));
    let triggers = s
        .records("agent_triggers")
        .into_iter()
        .filter(|t| t.data["agent_key"] == "reporter")
        .collect::<Vec<_>>();
    assert_eq!(triggers.len(), 1);
    assert_eq!(triggers[0].data["tool_names"], "[\"report_finding\"]");
    assert_eq!(triggers[0].data["on_tool_call"], true);
    assert!(
        triggers[0].data["tool_call_message"]
            .as_str()
            .unwrap()
            .contains("evidence_version")
    );
    for key in [
        "update_finding_report",
        "get_task_node_detail",
        "list_task_findings",
        "get_task_worker_trace",
        "list_task_worker_traces",
        "search_task_worker_traces",
        "get_task_graph",
    ] {
        assert!(
            s.records("tools")
                .iter()
                .find(|t| t.data["key"] == key)
                .unwrap()
                .data["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a == "reporter")
        );
    }
    assert!(
        artex_domain::bootstrap_plan(&s, &c)
            .unwrap()
            .changes
            .is_empty()
    );
    st.checkpoint().unwrap();
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert!(
        artex_domain::bootstrap_plan(&st.snapshot(), &c)
            .unwrap()
            .changes
            .is_empty()
    );
}
#[test]
fn reporter_broken_system_default_appends_history_and_repairs_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "repair reporter").unwrap();
    let c = ctx();
    bootstrap_commit(&mut st, &c);
    let reporter = st
        .snapshot()
        .records("agents")
        .into_iter()
        .find(|a| a.data["key"] == "reporter")
        .unwrap()
        .clone();
    let old = reporter.data["current_prompt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let broken = broken_reporter_default();
    raw_patch(
        &mut st,
        "agent_prompts",
        &old,
        json!({"template_text":body(&broken),"note":"Built-in default","updated_by":"system"}),
    );
    raw_put(
        &mut st,
        "agent_prompts",
        "prompt",
        json!({"agent_id":reporter.header.id,"version":4,"template_text":body("Historical user content"),"note":"Past edit","updated_by":"user"}),
    );
    let tool = st
        .snapshot()
        .records("tools")
        .into_iter()
        .find(|t| t.data["key"] == "update_finding_report")
        .unwrap()
        .header
        .id
        .clone();
    raw_patch(&mut st, "tools", &tool, json!({"agents":[]}));
    let stable_tool = st.snapshot().get("tools", &tool).unwrap().clone();
    let before = st.snapshot();
    let repair = artex_domain::bootstrap_plan(&before, &c).unwrap();
    assert!(
        repair
            .expected
            .iter()
            .any(|e| e.collection == "agents" && e.id == reporter.header.id)
    );
    assert!(
        repair
            .expected
            .iter()
            .any(|e| e.collection == "agent_prompts" && e.id == old)
    );
    bootstrap_commit(&mut st, &c);
    let s = st.snapshot();
    let agent = s.get("agents", &reporter.header.id).unwrap();
    let new = agent.data["current_prompt_id"].as_str().unwrap();
    assert_ne!(new, old);
    let p = s.get("agent_prompts", new).unwrap();
    assert_eq!(p.data["version"], 5);
    assert_eq!(
        p.data["template_text"]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        5195
    );
    assert_eq!(
        s.get("agent_prompts", &old).unwrap().data["template_text"],
        body(&broken)
    );
    assert_eq!(
        s.get("tools", &tool).unwrap().header.version,
        stable_tool.header.version
    );
    assert_eq!(s.get("tools", &tool).unwrap().data["agents"], json!([]));
    assert_eq!(agent.header.commit_seq, p.header.commit_seq);
    assert!(
        artex_domain::bootstrap_plan(&s, &c)
            .unwrap()
            .changes
            .is_empty()
    );
    st.checkpoint().unwrap();
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert!(
        artex_domain::bootstrap_plan(&st.snapshot(), &c)
            .unwrap()
            .changes
            .is_empty()
    );
}
#[test]
fn reporter_custom_prompt_occupied_key_and_deliberate_deletion_are_preserved() {
    for (by, note, changed) in [
        ("user", "Built-in default", false),
        ("ui", "", false),
        ("system", "Built-in default", true),
        ("", "", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut st = Store::create(dir.path(), "preserved reporter").unwrap();
        let c = ctx();
        bootstrap_commit(&mut st, &c);
        let a = st
            .snapshot()
            .records("agents")
            .into_iter()
            .find(|a| a.data["key"] == "reporter")
            .unwrap()
            .clone();
        let pid = a.data["current_prompt_id"].as_str().unwrap();
        let text = if changed {
            format!("{}\nUser added instructions", broken_reporter_default())
        } else {
            broken_reporter_default()
        };
        raw_patch(
            &mut st,
            "agent_prompts",
            pid,
            json!({"template_text":body(&text),"note":note,"updated_by":by}),
        );
        assert!(
            artex_domain::bootstrap_plan(&st.snapshot(), &c)
                .unwrap()
                .changes
                .is_empty()
        );
        assert_eq!(
            st.snapshot().get("agents", &a.header.id).unwrap().data["current_prompt_id"],
            pid
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "occupied reporter").unwrap();
    let c = ctx();
    let aid = raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"reporter","name":"My reporter","builtin":false}),
    );
    bootstrap_commit(&mut st, &c);
    let s = st.snapshot();
    assert_eq!(s.get("agents", &aid).unwrap().data["name"], "My reporter");
    assert!(s.get("agents", &aid).unwrap().data["current_prompt_id"].is_null());
    assert!(
        s.records("agent_triggers")
            .iter()
            .all(|t| t.data["agent_key"] != "reporter")
    );
    config(&mut st, "ui.deleteAgent", json!({"key":"reporter"}), &c);
    assert!(st.snapshot().get("agents", &aid).is_none());
    assert!(
        artex_domain::bootstrap_plan(&st.snapshot(), &c)
            .unwrap()
            .changes
            .is_empty()
    );
    st.checkpoint().unwrap();
    drop(st);
    let st = Store::open(dir.path()).unwrap();
    assert!(
        artex_domain::bootstrap_plan(&st.snapshot(), &c)
            .unwrap()
            .changes
            .is_empty()
    );
}

#[test]
fn runtime_prompt_rendering_rejects_fields_outside_the_source_role_struct() {
    use artex_domain::configuration_ui::render_agent_prompt_runtime as render;
    assert_eq!(
        render(
            "worker",
            "{{.Scope}}",
            &json!({"Scope":"not a worker field"})
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        render(
            "reporter",
            "{{.Goal}}",
            &json!({"Goal":"not a custom role field"})
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        render(
            "planner",
            "{{.Undeclared}}",
            &json!({"Undeclared":"must not grant new variables"})
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        render(
            "planner",
            "{{.Goal}}",
            &json!({"Goal":["not a typed string"]})
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
}
#[test]
fn runtime_prompt_rendering_uses_empty_missing_fields_and_never_preview_examples() {
    use artex_domain::configuration_ui::render_agent_prompt_runtime as render;
    assert_eq!(
        render(
            "planner",
            "{{.Goal}}|{{.Scope}}|{{.AssetSummary}}|{{.DataDir}}|{{.Now}}",
            &json!({"Goal":"real task","Now":"real clock"})
        )
        .unwrap(),
        "real task||||real clock"
    );
    assert_eq!(
        render(
            "worker",
            "{{.ProxyAddr}}|{{.WorkerName}}|{{.DataDir}}|{{.Now}}",
            &json!({})
        )
        .unwrap(),
        "|||"
    );
    assert_eq!(
        render(
            "mainagent",
            "{{.Goal}}|{{.FindingsSummary}}",
            &json!({"Goal":null})
        )
        .unwrap(),
        "|"
    );
    assert_eq!(
        render("goals", "{{.EngagementDescription}}", &json!({})).unwrap(),
        ""
    );
    assert_eq!(render("custom","{{.DataDir}}|{{.Now}}",&json!({"DataDir":"actual directory","Scope":"ignored extra caller context","EngagementDescription":"Test example.com"})).unwrap(),"actual directory|");
}

#[test]
fn canonical_mcp_create_update_preserve_secret_refs_and_reject_invalid_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "MCP canonical tools").unwrap();
    let c = ctx();
    let secret = new_id("secret");
    let input = json!({"name":"Scanner","transport":"stdio","command":"scanner","args":["--stdio"],"env":{"API_TOKEN":{"secret_ref":secret,"hint":"token"},"MODE":"safe"}});
    let result = apply(&mut st, "create_mcp", input.clone(), &c);
    let id = result["id"].as_str().unwrap().to_owned();
    let s = st.snapshot();
    let row = s.get("mcp_servers", &id).unwrap();
    assert_eq!(row.data["env"]["API_TOKEN"]["secret_ref"], secret);
    assert_eq!(row.data["args"], json!(["--stdio"]));
    assert_eq!(row.data["enabled"], true);
    assert!(s.records("agent_visibility").is_empty());
    assert_eq!(
        artex_domain::tool_projection::mutation_result("create_mcp", &input, &result),
        json!(format!("mcp created: id={id} name=Scanner"))
    );
    let bad = json!({"name":"Literal","transport":"stdio","command":"scanner","env":{"API_TOKEN":"literal-private-token"}});
    assert_eq!(
        plan("create_mcp", &bad, &s, &c).unwrap_err().code,
        "PERMISSION_DENIED"
    );
    assert_eq!(
        plan(
            "create_mcp",
            &json!({"name":"Bad","transport":"unknown"}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    let update = json!({"id":id,"name":"Scanner HTTP","transport":"http","url":"https://example.com/mcp","env":{"Authorization":{"secret_ref":secret,"hint":"token"}},"enabled":false});
    let updated = apply(&mut st, "update_mcp", update.clone(), &c);
    let s = st.snapshot();
    let row = s.get("mcp_servers", &id).unwrap();
    assert_eq!(row.data["transport"], "http");
    assert_eq!(row.data["url"], "https://example.com/mcp");
    assert!(row.data["command"].is_null());
    assert_eq!(row.data["args"], json!([]));
    assert_eq!(row.data["enabled"], false);
    assert_eq!(row.data["env"]["Authorization"]["secret_ref"], secret);
    assert_eq!(
        artex_domain::tool_projection::mutation_result("update_mcp", &update, &updated),
        json!(format!("mcp updated: id={id}"))
    );
    assert_eq!(
        plan("update_mcp", &json!({"id":id,"url":"not a URL"}), &s, &c)
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        plan(
            "update_mcp",
            &json!({"id":new_id("mcp"),"name":"Missing","transport":"stdio","command":"x"}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
    st.checkpoint().unwrap();
    drop(st);
    assert_eq!(
        Store::open(dir.path())
            .unwrap()
            .snapshot()
            .get("mcp_servers", &id)
            .unwrap()
            .data["env"]["Authorization"]["secret_ref"],
        secret
    );
}

#[test]
fn canonical_kill_and_steer_persist_active_run_state_without_touching_other_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "worker controls").unwrap();
    let c = ctx();
    let source = create(&mut st, &c, "source", vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let mut foreign = c.clone();
    foreign.task_id = Some(source.clone());
    let id = apply(
        &mut st,
        "add_intent",
        json!({"summary":"Current active work"}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let outside = apply(
        &mut st,
        "add_intent",
        json!({"summary":"Source active work"}),
        &foreign,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    raw_patch(
        &mut st,
        "exploration_nodes",
        &id,
        json!({"state":"running"}),
    );
    raw_patch(
        &mut st,
        "exploration_nodes",
        &outside,
        json!({"state":"running"}),
    );
    let agent = raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"worker","name":"Worker"}),
    );
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":task,"intent_id":id,"state":"running","engine_epoch":c.actor.engine_epoch}),
    );
    let other_run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":source,"intent_id":outside,"state":"running","engine_epoch":c.actor.engine_epoch}),
    );
    assert_eq!(
        plan(
            "steer_work",
            &json!({"intent_id":id,"message":"   "}),
            &st.snapshot(),
            &own
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    for tool in ["kill_work", "steer_work"] {
        assert_eq!(
            plan(
                tool,
                &json!({"intent_id":outside,"message":"Do not write source work"}),
                &st.snapshot(),
                &own
            )
            .unwrap_err()
            .code,
            "PERMISSION_DENIED"
        );
    }
    for (ordinal, message) in [
        ("1", "Stop broad scans; verify authorization"),
        ("2", "Use the captured baseline"),
    ] {
        let input = json!({"intent_id":id,"message":message});
        let result = apply(&mut st, "steer_work", input.clone(), &own);
        let s = st.snapshot();
        let row = s
            .get("steering_messages", result["steering_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(row.data["ordinal"], ordinal);
        assert_eq!(row.data["target_run_id"], run);
        assert_eq!(row.data["text"], message);
        assert_eq!(row.data["state"], "pending");
        assert_eq!(s.get("agent_runs", &run).unwrap().data["state"], "running");
        assert!(
            artex_domain::tool_projection::mutation_result("steer_work", &input, &result)
                .as_str()
                .unwrap()
                .contains("effective at the next step")
        );
    }
    let question = raw_put(
        &mut st,
        "user_questions",
        "question",
        json!({"run_id":run,"kind":"question","state":"pending","payload":{"questions":[{"question":"Approve?"}]},"generation":"1"}),
    );
    let input = json!({"intent_id":id});
    let result = apply(&mut st, "kill_work", input.clone(), &own);
    let s = st.snapshot();
    assert_eq!(
        s.get("exploration_nodes", &id).unwrap().data["state"],
        "stopped"
    );
    assert_eq!(
        s.get("agent_runs", &run).unwrap().data["state"],
        "cancelled"
    );
    assert_eq!(
        s.get("user_questions", &question).unwrap().data["state"],
        "cancelled"
    );
    assert_eq!(
        s.get("agent_runs", &other_run).unwrap().data["state"],
        "running"
    );
    assert_eq!(
        s.get("exploration_nodes", &outside).unwrap().data["state"],
        "running"
    );
    assert_eq!(
        artex_domain::tool_projection::mutation_result("kill_work", &input, &result),
        json!(format!("Sent termination signal to intent {id}"))
    );
    assert_eq!(
        plan(
            "steer_work",
            &json!({"intent_id":id,"message":"Too late"}),
            &s,
            &own
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    let fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"not a work handle"}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        plan(
            "kill_work",
            &json!({"intent_id":fact}),
            &st.snapshot(),
            &own
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
}

#[test]
fn ui_mcp_sealed_environment_preflight_commit_query_and_restart_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "sealed MCP UI").unwrap();
    let c = ctx();
    let secret = json!({"secret_ref":new_id("secret"),"hint":"sealed environment"});
    let params = json!({"m":{"name":"Native scanner","transport":"stdio","command":"scanner","args":["--stdio"],"env":secret,"enabled":true,"insecure":false}});
    let before = st.snapshot();
    let preflight =
        artex_domain::configuration_ui::plan("ui.saveMcpServer", &params, &before, &c).unwrap();
    assert_eq!(st.snapshot().commit_seq, before.commit_seq);
    assert!(preflight.expected.is_empty());
    let result = config(&mut st, "ui.saveMcpServer", params.clone(), &c);
    let id = result["id"].as_str().unwrap().to_owned();
    assert_eq!(
        st.snapshot().get("mcp_servers", &id).unwrap().data["env"],
        secret
    );
    let s = st.snapshot();
    let rows = artex_domain::configuration_ui::query("ui.mcpServers", &json!({}), &s, &c).unwrap();
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()["env"],
        secret
    );
    let update = json!({"m":{"id":id,"name":"Native scanner updated","transport":"stdio","command":"scanner","env":secret,"enabled":true}});
    let preflight =
        artex_domain::configuration_ui::plan("ui.saveMcpServer", &update, &s, &c).unwrap();
    assert!(
        preflight
            .expected
            .iter()
            .any(|e| e.collection == "mcp_servers"
                && e.id == id
                && e.version == s.get("mcp_servers", &id).unwrap().header.version)
    );
    config(&mut st, "ui.saveMcpServer", update, &c);
    for env in [
        json!({"secret_ref":"not-an-id","hint":"invalid"}),
        json!({"secret_ref":new_id("secret")}),
        json!({"secret_ref":new_id("secret"),"hint":"invalid","TOKEN":"plaintext-must-not-hide"}),
        json!({"API_TOKEN":"plaintext-must-not-persist"}),
        json!({"MODE":5}),
    ] {
        let bad = json!({"m":{"id":id,"name":"Bad scanner","transport":"stdio","command":"scanner","env":env}});
        assert!(
            artex_domain::configuration_ui::plan("ui.saveMcpServer", &bad, &st.snapshot(), &c)
                .is_err()
        );
    }
    st.checkpoint().unwrap();
    drop(st);
    let recovered = Store::open(dir.path()).unwrap().snapshot();
    assert_eq!(
        recovered.get("mcp_servers", &id).unwrap().data["env"],
        secret
    );
}

#[test]
fn custom_tool_header_credentials_reject_plaintext_journal_and_preserve_valid_references() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "custom headers").unwrap();
    let c = ctx();
    let schema = json!({"type":"object","properties":{"path":{"type":"string"}}});
    let bad = json!({"key":"credential_tool","kind":"http","schema":schema,"exec":{"url":"https://fixture.invalid","headers":{"Authorization":"Bearer must-not-enter-journal"}}});
    let before = st.snapshot().commit_seq;
    let mut batch = plan("create_custom_tool", &bad, &st.snapshot(), &c).unwrap();
    batch.command_id = new_id("command");
    assert_eq!(st.commit(batch).unwrap_err().code, "PERMISSION_DENIED");
    assert_eq!(st.snapshot().commit_seq, before);
    assert!(st.snapshot().records("tools").is_empty());
    let public = json!({"key":"credential_tool","kind":"http","schema":schema,"exec":{"url":"https://fixture.invalid","headers":{"Content-Type":"application/json"}}});
    let result = apply(&mut st, "create_custom_tool", public, &c);
    let id = result["id"].as_str().unwrap().to_owned();
    let secret = json!({"secret_ref":new_id("secret"),"hint":"sealed headers"});
    let sealed = json!({"key":"credential_tool","kind":"http","schema":schema,"exec":{"url":"https://fixture.invalid","headers":secret}});
    apply(&mut st, "update_custom_tool", sealed, &c);
    let stable = st.snapshot().get("tools", &id).unwrap().clone();
    assert_eq!(stable.data["exec"]["headers"], secret);
    let mut batch = plan("update_custom_tool", &bad, &st.snapshot(), &c).unwrap();
    batch.command_id = new_id("command");
    assert_eq!(st.commit(batch).unwrap_err().code, "PERMISSION_DENIED");
    assert_eq!(
        st.snapshot().get("tools", &id).unwrap().header.version,
        stable.header.version
    );
    assert_eq!(
        st.snapshot().get("tools", &id).unwrap().data["exec"]["headers"],
        secret
    );
    let leaf = json!({"key":"credential_tool","kind":"http","schema":schema,"exec":{"url":"https://fixture.invalid","headers":{"Authorization":secret,"Content-Type":"application/json"}}});
    apply(&mut st, "update_custom_tool", leaf, &c);
    st.checkpoint().unwrap();
    drop(st);
    let recovered = Store::open(dir.path()).unwrap().snapshot();
    assert_eq!(
        recovered.get("tools", &id).unwrap().data["exec"]["headers"]["Authorization"],
        secret
    );
}

#[test]
fn canonical_task_profile_and_worker_lists_match_source_nonempty_privacy_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "read tool projections").unwrap();
    let c = ctx();
    let profile = raw_put(
        &mut st,
        "llm_profiles",
        "profile",
        json!({"name":"Research","model":"model-x","format":"openai","is_default":true,"api_key":{"secret_ref":new_id("secret"),"hint":"private-key"},"base_url":"https://private.example/llm"}),
    );
    let source = create(&mut st, &c, "source", vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    let unrelated = create(&mut st, &c, "unrelated", vec![]);
    let deleted = create(&mut st, &c, "deleted", vec![]);
    raw_patch(
        &mut st,
        "tasks",
        &task,
        json!({"llm_profile_id":profile,"parent_ref":"arrangement"}),
    );
    raw_patch(
        &mut st,
        "tasks",
        &deleted,
        json!({"deleted_at":artex_core::now()}),
    );
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let mut intents = vec![];
    for (tid, summary, state) in [
        (&task, "Own work", "running"),
        (&source, "Visible source result", "done"),
        (&source, "Private running source", "running"),
        (&unrelated, "Unrelated result", "done"),
    ] {
        let mut scope = c.clone();
        scope.task_id = Some(tid.clone());
        let id = apply(&mut st, "add_intent", json!({"summary":summary}), &scope)["ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        raw_patch(&mut st, "exploration_nodes", &id, json!({"state":state}));
        intents.push(id);
    }
    let s = st.snapshot();
    let reader = |r: &Value| st.blob_read(r, 0, r["length"].as_str().unwrap().parse().unwrap());
    let profiles =
        artex_domain::tool_projection::query("list_llm_profiles", &json!({}), &s, &own, &reader)
            .unwrap();
    assert_eq!(
        profiles,
        json!({"profiles":[{"id":profile,"name":"Research","model":"model-x","format":"openai","is_active":true}]})
    );
    assert!(!profiles.to_string().contains("private"));
    let tasks =
        artex_domain::tool_projection::query("list_tasks", &json!({}), &s, &own, &reader).unwrap();
    assert_eq!(tasks.as_array().unwrap().len(), 3);
    assert!(
        tasks
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != deleted && r["run_seconds"].as_i64().is_some_and(|n| n >= 0))
    );
    let current = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == task)
        .unwrap();
    assert_eq!(current["llm_profile"], "Research");
    assert_eq!(current["parent_ref"], "arrangement");
    assert_eq!(current.as_object().unwrap().len(), 7);
    assert!(current.get("api_key").is_none());
    let traces = artex_domain::tool_projection::query(
        "list_task_worker_traces",
        &json!({}),
        &s,
        &own,
        &reader,
    )
    .unwrap();
    assert_eq!(traces["works"].as_array().unwrap().len(), 2);
    let inherited = traces["works"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["intent_id"] == intents[1])
        .unwrap();
    assert_eq!(inherited["source_task_id"], source);
    assert_eq!(inherited["inherited"], true);
    assert!(
        traces["works"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["intent_id"] != intents[2]
                && r["intent_id"] != intents[3]
                && r.get("detail").is_none())
    );
    let filtered = artex_domain::tool_projection::query(
        "list_task_worker_traces",
        &json!({"q":"visible source","limit":1}),
        &s,
        &own,
        &reader,
    )
    .unwrap();
    assert_eq!(filtered["works"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["works"][0]["intent_id"], intents[1]);
    assert_eq!(
        artex_domain::tool_projection::query(
            "get_task_worker_trace",
            &json!({"intent_id":intents[3]}),
            &s,
            &own,
            &reader
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
}

#[test]
fn narrow_url_normalization_preserves_go_identity_bytes() {
    for (input, expected) in [
        (
            "HTTPS://EXAMPLE.COM:443/a/../b?z=%2f&a=1#X",
            "https://example.com:443/a/../b?z=%2f&a=1#X",
        ),
        ("http://EXAMPLE.COM:80/", "http://example.com:80"),
        ("http://Example.COM/%2F/", "http://example.com/%2F/"),
        ("http://EXAMPLE.COM/?", "http://example.com/?"),
        ("http://[2001:DB8::1]:80/a", "http://[2001:db8::1]:80/a"),
    ] {
        assert_eq!(
            artex_domain::assets::normalize_url(input).unwrap(),
            expected
        );
    }
}

#[test]
fn inherited_trace_excludes_running_intents_thinking_and_usage() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "traces").unwrap();
    let c = ctx();
    let source = create(&mut st, &c, "source", vec![]);
    let target = create(&mut st, &c, "target", vec![source.clone()]);
    let exp = st.snapshot().get("tasks", &source).unwrap().data["exploration_id"].clone();
    let terminal = raw_put(
        &mut st,
        "exploration_nodes",
        "node",
        json!({"exploration_id":exp,"kind":"intent","state":"done","payload":{"summary":"finished"}}),
    );
    let live = raw_put(
        &mut st,
        "exploration_nodes",
        "node",
        json!({"exploration_id":exp,"kind":"intent","state":"running","payload":{"summary":"running"}}),
    );
    for (node, kind, summary) in [
        (&terminal, "tool_result", "public result"),
        (&terminal, "thinking", "private reasoning"),
        (&terminal, "usage", "secret usage"),
        (&live, "tool_result", "live output"),
    ] {
        raw_put(
            &mut st,
            "activity",
            "activity",
            json!({"exploration_id":exp,"node_id":node,"kind":kind,"summary":summary,"detail":body(summary),"input_tokens":12}),
        );
    }
    let mut c = c;
    c.task_id = Some(target);
    let rows = query("search_all_worker_traces", &json!({}), &st.snapshot(), &c).unwrap();
    assert_eq!(rows["steps"].as_array().unwrap().len(), 1);
    assert_eq!(rows["steps"][0]["summary"], "public result");
    assert!(rows["steps"][0].get("input_tokens").is_none());
}

#[test]
fn command_receipts_without_data_id_are_not_read_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "dependencies").unwrap();
    let c = ctx();
    create(&mut st, &c, "a", vec![]);
    assert!(!st.snapshot().records("command_receipts").is_empty());
    let batch = plan(
        "company.create",
        &json!({"name":"Acme"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert!(
        !batch
            .expected
            .iter()
            .any(|e| e.collection == "command_receipts")
    );
}

#[test]
fn asset_batches_preserve_good_items_and_report_bad_item_indices() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "batch").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let result = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"ip","ip":"192.0.2.10"},{"type":"ip","ip":"host.example"},{"type":"subdomain","domain":"x.example.co.uk","record_type":"A"}]}),
        &c,
    );
    assert!(result["ids"][0].as_str().unwrap().starts_with("asset_"));
    assert_eq!(result["ids"][1], Value::Null);
    assert_eq!(result["errors"]["1"]["code"], "INVALID_PARAMS");
    assert_eq!(st.snapshot().records("assets").len(), 3);
    let row = st
        .snapshot()
        .get("assets", result["ids"][2].as_str().unwrap())
        .unwrap()
        .data
        .clone();
    assert_eq!(row["root_domain"], "example.co.uk");
}

#[test]
fn partial_node_batches_and_last_goal_auto_completion_match_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "goals").unwrap();
    let mut c = ctx();
    let task = create(&mut st, &c, "target", vec![]);
    c.task_id = Some(task.clone());
    let batch = apply(
        &mut st,
        "add_intent",
        json!({"intents":[{"summary":"first valid"},{"summary":""},{"summary":"second valid"}]}),
        &c,
    );
    assert!(batch["ids"][0].is_string());
    assert_eq!(batch["ids"][1], Value::Null);
    assert!(batch["ids"][2].is_string());
    assert_eq!(batch["errors"]["1"]["code"], "INVALID_PARAMS");
    let goals = apply(
        &mut st,
        "set_goals",
        json!({"goals":[{"text":"prove access is blocked"},{"text":"prove logging"}]}),
        &c,
    );
    let fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"verified both"}),
        &c,
    )["ids"][0]
        .clone();
    let result = apply(
        &mut st,
        "prove_goal",
        json!({"goal_id":goals["ids"][0],"evidence_id":fact}),
        &c,
    );
    assert_eq!(result["all_goals_met"], false);
    let result = apply(
        &mut st,
        "prove_goal",
        json!({"goal_id":goals["ids"][1],"evidence_id":fact}),
        &c,
    );
    assert_eq!(result["all_goals_met"], true);
    assert_eq!(
        st.snapshot().get("tasks", &task).unwrap().data["status"],
        "done"
    );
}

#[test]
fn company_natural_key_scope_precedence_and_explicit_owner_are_sticky() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "company").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let company = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"  Acme   Company  ","scope":"example.co.uk\n192.0.2.0/24\ncom\n0.0.0.0/0","reason":"verified ownership"}),
        &c,
    );
    assert_eq!(company["errors"].as_array().unwrap().len(), 2);
    let company2 = apply(
        &mut st,
        "add_company_scope",
        json!({"company":"acme company","scope":"198.51.100.0/24"}),
        &c,
    );
    assert_eq!(company["company_id"], company2["company_id"]);
    let other = apply(
        &mut st,
        "company.create",
        json!({"name":"Other Company"}),
        &c,
    )["company_id"]
        .clone();
    let asset = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"subdomain","domain":"a.example.co.uk","record_type":"A"}]}),
        &c,
    )["ids"][0]
        .clone();
    assert_eq!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        company["company_id"]
    );
    let asset2=apply(&mut st,"insert_assets",json!({"assets":[{"type":"subdomain","domain":"a.example.co.uk","record_type":"A","company_id":other}]}),&c)["ids"][0].clone();
    assert_eq!(asset, asset2);
    apply(
        &mut st,
        "add_company_scope",
        json!({"company":"Another Company","scope":"example.co.uk"}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        other
    );
    assert_eq!(
        plan(
            "company.create",
            &json!({"name":" ACME\tCompany "}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "VERSION_CONFLICT"
    );
}

#[test]
fn immutable_evidence_deduplicates_verified_capture_without_note_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "evidence").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "target", vec![]));
    let blob = st.blob_put(b"verified response", "text/plain").unwrap();
    let traffic = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"ts":"1720000000","host":"example.com","url":"https://example.com/item","method":"GET","status":200,"content_type":"text/plain","req_len":"0","resp_len":"17"}),
    );
    let body_id = new_id("trafficbody");
    let mut data = default_data("exchange_bodies").unwrap();
    artex_domain::merge(
        &mut data,
        &json!({"id":traffic,"req_head":body("GET /item HTTP/1.1"),"resp_head":body("HTTP/1.1 200"),"resp_body":blob}),
    );
    let mut batch = artex_domain::empty_batch("test.capture", &json!({}));
    batch.command_id = new_id("command");
    batch.changes.push(Change::Put {
        collection: "exchange_bodies".into(),
        id: body_id,
        data,
    });
    st.commit(batch).unwrap();
    let finding = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"IDOR","severity":"high","summary":"access","traffic_refs":[{"traffic_id":traffic,"role":"proof","note":"first proof"},{"traffic_id":traffic,"role":"supporting","note":"must not overwrite"}]}),
        &c,
    );
    let fid = finding["finding_id"].as_str().unwrap();
    assert_eq!(finding["evidence_version"], "1");
    apply(
        &mut st,
        "bind_finding_traffic",
        json!({"finding_id":fid,"traffic_refs":[{"traffic_id":traffic,"role":"supporting","note":"changed note"}]}),
        &c,
    );
    let s = st.snapshot();
    assert_eq!(s.records("traffic_evidence_snapshots").len(), 1);
    assert_eq!(s.records("finding_traffic_bindings").len(), 1);
    assert_eq!(
        s.records("finding_traffic_bindings")[0].data["note"],
        "first proof"
    );
    assert_eq!(
        s.get("findings", fid).unwrap().data["evidence_version"],
        "1"
    );
    let no_blob = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"ts":"1720000001","host":"example.com","url":"https://example.com/other","method":"GET","status":200,"req_len":"0","resp_len":"3"}),
    );
    assert!(
        plan(
            "bind_finding_traffic",
            &json!({"finding_id":fid,"traffic_refs":[{"traffic_id":no_blob}]}),
            &st.snapshot(),
            &c
        )
        .is_err()
    );
}

fn task_control(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = artex_domain::task_control_ui::plan(m, &p, &st.snapshot(), c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"][m], &contract, &b.result)
        .unwrap_or_else(|e| panic!("{m}: {e}"));
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
#[test]
fn human_goals_revive_terminal_tasks_and_deletion_does_not_revive() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "goals").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "one", vec![]);
    raw_patch(
        &mut st,
        "tasks",
        &task,
        json!({"status":"done","completed_at":artex_core::now()}),
    );
    let goal = task_control(
        &mut st,
        "ui.addGoal",
        json!({"id":task,"text":"  Verify authorization ","vulnclass":"IDOR"}),
        &c,
    );
    assert_eq!(goal["text"], "Verify authorization");
    assert_eq!(goal["origin"], "human");
    assert_eq!(
        st.snapshot().get("tasks", &task).unwrap().data["status"],
        "running"
    );
    let gid = goal["id"].as_str().unwrap();
    task_control(
        &mut st,
        "ui.updateGoal",
        json!({"id":task,"goalId":gid,"text":"Check revised goal"}),
        &c,
    );
    raw_patch(
        &mut st,
        "tasks",
        &task,
        json!({"status":"done","completed_at":artex_core::now()}),
    );
    task_control(
        &mut st,
        "ui.deleteGoal",
        json!({"id":task,"goalId":gid}),
        &c,
    );
    assert_eq!(
        st.snapshot().get("tasks", &task).unwrap().data["status"],
        "done"
    );
    assert!(st.snapshot().get("exploration_nodes", gid).is_none());
    assert!(st.snapshot().records("activity").iter().any(|a| {
        a.data["summary"]
            .as_str()
            .unwrap_or("")
            .contains("deleted goal")
    }));
}
#[test]
fn intent_cancel_preserves_shared_evidence_and_irreversible_token_usage() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "intent").unwrap();
    let mut c = ctx();
    let task = create(&mut st, &c, "one", vec![]);
    c.task_id = Some(task.clone());
    let first = apply(&mut st, "add_intent", json!({"summary":"First"}), &c)["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let second = apply(&mut st, "add_intent", json!({"summary":"Second"}), &c)["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let exclusive = apply(
        &mut st,
        "record_fact",
        json!({"summary":"Exclusive","intent_id":first}),
        &c,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let shared = apply(
        &mut st,
        "record_fact",
        json!({"summary":"Shared","intent_id":first}),
        &c,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let exp = st.snapshot().get("tasks", &task).unwrap().data["exploration_id"].clone();
    raw_put(
        &mut st,
        "exploration_edges",
        "edge",
        json!({"exploration_id":exp,"src_id":second,"dst_id":shared,"rel":"yields"}),
    );
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"node_id":first,"worker":"worker","kind":"result","summary":"private output","detail":body("sensitive output"),"input_tokens":17}),
    );
    let out = task_control(
        &mut st,
        "ui.controlIntent",
        json!({"taskId":task,"intentId":first,"action":"cancel","reason":"Wrong direction","mode":"hard"}),
        &c,
    );
    assert_eq!(out["deleted"]["intents"], 1);
    assert_eq!(out["deleted"]["facts"], 1);
    let s = st.snapshot();
    assert!(s.get("exploration_nodes", &first).is_none());
    assert!(s.get("exploration_nodes", &exclusive).is_none());
    assert!(s.get("exploration_nodes", &shared).is_some());
    let ledger = s
        .records("activity")
        .into_iter()
        .find(|a| a.data["worker"] == "token-ledger")
        .unwrap();
    assert_eq!(ledger.data["input_tokens"], 17);
    assert!(ledger.data["detail"].is_null());
    assert!(ledger.data["node_id"].is_null());
}
#[test]
fn task_scope_normalization_is_idempotent_and_sources_remain_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "scope").unwrap();
    let c = ctx();
    let source = create(&mut st, &c, "source", vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    let scope = task_control(
        &mut st,
        "ui.addTaskScope",
        json!({"id":source,"kind":"root_domain","value":" API.Example.co.uk:443 ","reason":"authorized"}),
        &c,
    );
    assert_eq!(scope["domain"], "example.co.uk");
    let same = task_control(
        &mut st,
        "ui.addTaskScope",
        json!({"id":source,"kind":"root_domain","value":"example.co.uk"}),
        &c,
    );
    assert_eq!(scope["id"], same["id"]);
    assert_eq!(st.snapshot().records("task_scope").len(), 1);
    let ip = task_control(
        &mut st,
        "ui.addTaskScope",
        json!({"id":task,"kind":"ip","value":"[2001:db8::1]:443"}),
        &c,
    );
    assert_eq!(ip["net"], "2001:db8::1/128");
    let result = artex_domain::task_control_ui::query(
        "ui.taskScope",
        &json!({"id":task}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(result["scope"][0]["task_id"], task);
    assert_eq!(result["scope"][1]["task_id"], source);
    assert_eq!(
        artex_domain::task_control_ui::plan(
            "ui.deleteTaskScope",
            &json!({"id":task,"scopeId":scope["id"]}),
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
}

fn assets_ui(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = artex_domain::assets_ui::plan(m, &p, &st.snapshot(), c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"][m], &contract, &b.result)
        .unwrap_or_else(|e| panic!("{m}: {e}"));
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
#[test]
fn manual_asset_links_keep_global_assets_and_source_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "assets pages").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "one", vec![]);
    let asset = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"example.com"}),
        &c,
    )["ids"][0]
        .clone();
    let p = json!({"taskId":task,"assetIds":[asset,asset],"sourceSummary":"Operator approved"});
    let first = assets_ui(&mut st, "ui.attachTaskAssets", p.clone(), &c);
    assert_eq!(first, json!({"requested":1,"attached":1,"existing":0}));
    let again = assets_ui(&mut st, "ui.attachTaskAssets", p, &c);
    assert_eq!(again["existing"], 1);
    let s = st.snapshot();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.assets", json!({})),
        ("ui.taskAssets", json!({"taskId":task})),
        ("ui.assetCounts", json!({"taskId":task})),
        (
            "ui.searchTaskAssets",
            json!({"taskId":task,"dsl":"domain==example.com"}),
        ),
        ("ui.taskIntentAssets", json!({"taskId":task})),
        ("ui.companies", json!({})),
    ] {
        let out = artex_domain::assets_ui::query(m, &p, &s, &c).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &out)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
    }
    let page =
        artex_domain::assets_ui::query("ui.taskAssets", &json!({"taskId":task}), &s, &c).unwrap();
    assert_eq!(page["assets"][0]["task_source"], "manual");
    assert_eq!(
        page["assets"][0]["task_source_summary"],
        "Operator approved"
    );
    assert_eq!(page["assets"][0]["task_ids"], json!([task]));
    let detached = assets_ui(
        &mut st,
        "ui.detachTaskAsset",
        json!({"taskId":task,"assetId":asset}),
        &c,
    );
    assert_eq!(detached["detached"], 1);
    assert!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .is_some()
    );
    assert!(st.snapshot().records("task_asset_links").is_empty());
}
#[test]
fn company_scope_add_is_partial_replace_is_atomic_and_delete_reattributes() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "company UI").unwrap();
    let c = ctx();
    let first = assets_ui(
        &mut st,
        "ui.createCompany",
        json!({"name":"Old","scope":[{"kind":"domain","value":"example.com"},{"kind":"cidr","value":"10.0.0.0/8"}]}),
        &c,
    );
    assert_eq!(first["scope_added"], 1);
    assert_eq!(first["scope_invalid"], 1);
    assert_eq!(first["scope_errors"].as_array().unwrap().len(), 1);
    let asset = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"example.com"}),
        &c,
    )["ids"][0]
        .clone();
    let second = assets_ui(
        &mut st,
        "ui.createCompany",
        json!({"name":"New","scope":[{"kind":"domain","value":"example.com"}]}),
        &c,
    );
    let before = st.snapshot().commit_seq;
    assert!(
        artex_domain::assets_ui::plan(
            "ui.updateCompanyScope",
            &json!({"id":first["id"],"scope":[{"kind":"ip","value":"bad"}]}),
            &st.snapshot(),
            &c
        )
        .is_err()
    );
    assert_eq!(st.snapshot().commit_seq, before);
    assets_ui(
        &mut st,
        "ui.deleteCompany",
        json!({"id":first["id"],"deleteAssets":false}),
        &c,
    );
    assert_eq!(
        st.snapshot()
            .get("assets", asset.as_str().unwrap())
            .unwrap()
            .data["company_id"],
        second["id"]
    );
    let out =
        artex_domain::assets_ui::query("ui.companies", &json!({}), &st.snapshot(), &c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"]["ui.companies"], &contract, &out).unwrap();
    assert_eq!(out[0]["scope"][0]["domain"], "example.com");
}
#[test]
fn task_scope_registration_invalid_batch_has_no_partial_global_asset_effect() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "register").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "one", vec![]);
    assert!(artex_domain::assets_ui::plan("ui.registerTaskAssetScopes",&json!({"taskId":task,"scope":[{"kind":"domain","value":"example.com"},{"kind":"cidr","value":"10.0.0.0/8"}]}),&st.snapshot(),&c).is_err());
    assert!(st.snapshot().records("assets").is_empty());
    let p = json!({"taskId":task,"scope":[{"kind":"domain","value":"example.com"},{"kind":"ip","value":"192.0.2.1"},{"kind":"keyword","value":" Example company "}]});
    let first = assets_ui(&mut st, "ui.registerTaskAssetScopes", p.clone(), &c);
    assert_eq!(first["assets_linked"], 2);
    assert_eq!(first["scopes_added"], 3);
    let again = assets_ui(&mut st, "ui.registerTaskAssetScopes", p, &c);
    assert_eq!(again["assets_existing"], 2);
    assert_eq!(again["scopes_existing"], 3);
    assert_eq!(st.snapshot().records("assets").len(), 2);
}
#[test]
fn traffic_and_audit_pages_use_capture_bodies_and_actual_guard_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "traffic pages").unwrap();
    let mut c = ctx();
    let task = create(&mut st, &c, "one", vec![]);
    c.task_id = Some(task.clone());
    let exp = st.snapshot().get("tasks", &task).unwrap().data["exploration_id"].clone();
    let req = st
        .blob_put(b"request token", "application/octet-stream")
        .unwrap();
    let resp = st
        .blob_put(b"response payload", "application/octet-stream")
        .unwrap();
    let id = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"ts":"1720000001","host":"example.com","url":"https://example.com/","method":"GET","status":200,"req_len":"13","resp_len":"16"}),
    );
    let mut b = artex_domain::empty_batch("test.capture", &json!({}));
    let mut d = default_data("exchange_bodies").unwrap();
    artex_domain::merge(
        &mut d,
        &json!({"id":id,"req_head":body("GET / HTTP/1.1"),"resp_head":body("HTTP/1.1 200 OK"),"req_body":req,"resp_body":resp}),
    );
    b.changes.push(Change::Put {
        collection: "exchange_bodies".into(),
        id: new_id("trafficbody"),
        data: d,
    });
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"worker":"worker","kind":"tool_use","summary":"a failed tool is not necessarily a denied call","is_error":true}),
    );
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"worker":"worker","kind":"guard","summary":"guard","detail":body(&json!({"guard_audit":{"ts":1720000001,"tool":"Bash","action":"allow","reason":"Approved","command":"printf test"},"guard_attribution":"error"}).to_string())}),
    );
    let s = st.snapshot();
    let read = |reference: &Value| st.blob_read(reference, 0, 65536);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.traffic", json!({"q":"payload"})),
        ("ui.trafficHosts", json!({})),
        ("ui.trafficExchange", json!({"id":id})),
        ("ui.audit", json!({"task":task})),
    ] {
        let out = artex_domain::traffic_ui::query(m, &p, &s, &c, &read).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &out)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
        if m == "ui.traffic" {
            assert_eq!(out["total"], 1);
        }
        if m == "ui.audit" {
            assert_eq!(out["entries"].as_array().unwrap().len(), 1);
            assert_eq!(out["entries"][0]["action"], "allow");
            assert_eq!(out["attributions"]["error"], 1);
        }
    }
}
#[test]
fn coverage_uses_direct_source_fact_anchors_and_tree_deduplicates_ancestor_findings() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "asset graph").unwrap();
    let mut c = ctx();
    let source = create(&mut st, &c, "source", vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    c.task_id = Some(source.clone());
    let root = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"example.com"}),
        &c,
    )["ids"][0]
        .clone();
    let sub = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"subdomain","domain":"api.example.com"}),
        &c,
    )["ids"][0]
        .clone();
    apply(
        &mut st,
        "add_task_scope",
        json!({"entries":[{"kind":"root_domain","value":"example.com"}]}),
        &c,
    );
    apply(
        &mut st,
        "record_fact",
        json!({"summary":"tested subdomain","asset_ids":[sub]}),
        &c,
    );
    let f = apply(
        &mut st,
        "report_finding",
        json!({"vulnclass":"IDOR","severity":"high","summary":"Evidence","asset_ids":[root,sub]}),
        &c,
    );
    assert!(!f["finding_id"].is_null());
    let s = st.snapshot();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.taskCoverage", json!({"id":task})),
        ("ui.taskCoverageGraph", json!({"id":task})),
        ("ui.findingAssetTree", json!({"q":{}})),
    ] {
        let out = artex_domain::asset_graph_ui::query(m, &p, &s, &c).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &out)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
        if m == "ui.taskCoverage" {
            assert_eq!(out["denominator"], 2);
            assert_eq!(out["tested"], 1);
            assert_eq!(out["pct"], 0.5);
        }
        if m == "ui.findingAssetTree" {
            let parent = out["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["asset_id"] == root)
                .unwrap();
            assert_eq!(parent["self"], 1);
            assert_eq!(parent["total"], 1);
            assert_eq!(parent["high"], 1);
        }
    }
    assert_eq!(
        artex_domain::findings_ui::filtered(
            &s,
            &json!({"assetScope":format!("a:{}",root.as_str().unwrap())})
        )
        .unwrap()
        .len(),
        1
    );
}
fn policy_ui(st: &mut Store, m: &str, p: Value, c: &Context) -> Value {
    let mut b = artex_domain::policy_ui::plan(m, &p, &st.snapshot(), c).unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"][m], &contract, &b.result)
        .unwrap_or_else(|e| panic!("{m}: {e}"));
    b.command_id = new_id("command");
    st.commit(b).unwrap().result
}
#[test]
fn intercept_pending_is_idempotent_and_human_decision_wins_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "intercept").unwrap();
    let c = ctx();
    let mut seed = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    seed.command_id = new_id("command");
    st.commit(seed).unwrap();
    let task = create(&mut st, &c, "one", vec![]);
    let agent = st
        .snapshot()
        .records("agents")
        .into_iter()
        .find(|r| r.data["key"] == "worker")
        .unwrap()
        .header
        .id
        .clone();
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":task,"engine_epoch":c.actor.engine_epoch,"state":"running"}),
    );
    let rule = policy_ui(
        &mut st,
        "ui.createInterceptRule",
        json!({"rule":{"name":"Review shell","enabled":true,"priority":10,"match_target":"tool_input","match_type":"regex","pattern":"rm.*file","action":"ask","message":"Confirm deletion","timeout_enabled":true,"timeout_seconds":60,"timeout_action":"deny"}}),
        &c,
    );
    let input = json!({"command":"rm file"});
    let mut batch =
        artex_domain::policy::decision_plan(&st.snapshot(), &c, &run, "call-1", "Bash", &input)
            .unwrap();
    assert_eq!(batch.result["decision"], "ask");
    let id = batch.result["pending_id"].as_str().unwrap().to_owned();
    batch.command_id = new_id("command");
    st.commit(batch).unwrap();
    let again =
        artex_domain::policy::decision_plan(&st.snapshot(), &c, &run, "call-1", "Bash", &input)
            .unwrap();
    assert_eq!(again.result["pending_id"], id);
    assert!(again.changes.is_empty());
    assert_eq!(st.snapshot().records("intercept_pending").len(), 1);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    let s = st.snapshot();
    for (m, p) in [
        ("ui.interceptRules", json!({})),
        ("ui.interceptPending", json!({})),
        ("ui.interceptGetOne", json!({"id":id})),
        ("ui.interceptDetail", json!({"id":id})),
        ("ui.interceptHistoryPage", json!({})),
        ("ui.interceptTaskPage", json!({"taskId":task})),
        ("ui.interceptGetToolConfig", json!({})),
        ("ui.interceptGetJudgeConfig", json!({})),
        ("ui.interceptJudgeUsage", json!({})),
    ] {
        let out = artex_domain::policy_ui::query(m, &p, &s, &c).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &out)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
    }
    let mut data = st
        .snapshot()
        .get("intercept_pending", &id)
        .unwrap()
        .data
        .clone();
    data["audit"]["deadline_at"] = json!("2000-01-01T00:00:00Z");
    raw_patch(
        &mut st,
        "intercept_pending",
        &id,
        json!({"audit":data["audit"]}),
    );
    let mut stale = artex_domain::policy::expire_pending_plan(&st.snapshot(), &c).unwrap();
    stale.command_id = new_id("command");
    policy_ui(
        &mut st,
        "ui.interceptDecide",
        json!({"id":id,"decision":"allowed"}),
        &c,
    );
    assert_eq!(st.commit(stale).unwrap_err().code, "VERSION_CONFLICT");
    assert_eq!(
        st.snapshot().get("intercept_pending", &id).unwrap().data["status"],
        "allowed"
    );
    assert_eq!(
        artex_domain::policy::expire_pending_plan(&st.snapshot(), &c)
            .unwrap()
            .result["resolved"],
        0
    );
    policy_ui(
        &mut st,
        "ui.deleteInterceptRule",
        json!({"id":rule["id"]}),
        &c,
    );
}
#[test]
fn llm_quota_chain_is_monotonic_and_pool_health_resets_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "LLM failure").unwrap();
    let c = ctx();
    let mut seed = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    seed.command_id = new_id("command");
    st.commit(seed).unwrap();
    let a=config(&mut st,"ui.saveLLMProfile",json!({"p":{"name":"first","format":"openai","model":"one","api_key":{"secret_ref":new_id("secret"),"hint":"x"}}}),&c)["id"].as_str().unwrap().to_owned();
    let b=config(&mut st,"ui.saveLLMProfile",json!({"p":{"name":"second","format":"openai","model":"two","api_key":{"secret_ref":new_id("secret"),"hint":"x"}}}),&c)["id"].as_str().unwrap().to_owned();
    config(&mut st, "ui.activateLLMProfile", json!({"id":b}), &c);
    let task = apply(
        &mut st,
        "task.create",
        json!({"description":"Chain","goal":"verify","llm_profile_ids":[a,b]}),
        &c,
    )["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent = st
        .snapshot()
        .records("agents")
        .into_iter()
        .find(|r| r.data["key"] == "worker")
        .unwrap()
        .header
        .id
        .clone();
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":task,"engine_epoch":c.actor.engine_epoch,"state":"running"}),
    );
    let revision = st.snapshot().get("tasks", &task).unwrap().data["llm_chain_revision"].clone();
    let transient=artex_domain::conversation::profile_failure_plan(&st.snapshot(),&c,&run,&json!({"profile_id":a,"chain_revision":revision,"status":429,"message":"rate limit requests per minute","committed":false})).unwrap();
    assert!(transient.changes.is_empty());
    assert_eq!(transient.result["should_retry"], false);
    let mut first=artex_domain::conversation::profile_failure_plan(&st.snapshot(),&c,&run,&json!({"profile_id":a,"chain_revision":revision,"status":402,"message":"LLM provider HTTP 402","committed":false})).unwrap();
    assert_eq!(first.result["next_profile_id"], b);
    assert_eq!(first.result["should_retry"], true);
    first.command_id = new_id("command");
    st.commit(first).unwrap();
    let stale=artex_domain::conversation::profile_failure_plan(&st.snapshot(),&c,&run,&json!({"profile_id":a,"chain_revision":revision,"message":"insufficient_quota","committed":true})).unwrap();
    assert_eq!(stale.result["stale"], true);
    assert_eq!(stale.result["should_retry"], false);
    assert!(stale.changes.is_empty());
    let revision = st.snapshot().get("tasks", &task).unwrap().data["llm_chain_revision"].clone();
    let mut last=artex_domain::conversation::profile_failure_plan(&st.snapshot(),&c,&run,&json!({"profile_id":b,"chain_revision":revision,"message":"payment required","committed":false})).unwrap();
    assert_eq!(last.result["chain_exhausted"], true);
    last.command_id = new_id("command");
    st.commit(last).unwrap();
    let s = st.snapshot();
    assert!(
        artex_domain::conversation::effective_profile(
            &s,
            s.get("agents", &agent).unwrap(),
            None,
            s.get("tasks", &task)
        )
        .is_err()
    );
    let task2 = create(&mut st, &c, "Global pool", vec![]);
    let run2 = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":task2,"engine_epoch":c.actor.engine_epoch,"state":"running"}),
    );
    raw_put(
        &mut st,
        "settings",
        "setting",
        json!({"key":"llm_pool_enabled","value":"true"}),
    );
    raw_put(
        &mut st,
        "settings",
        "setting",
        json!({"key":"llm_retry_policy","value":json!({"breaker":{"attempts":-1,"interval_ms":90000}}).to_string()}),
    );
    let mut failure = artex_domain::conversation::profile_failure_plan(
        &st.snapshot(),
        &c,
        &run2,
        &json!({"profile_id":b,"status":429,"message":"rate limit","committed":false}),
    )
    .unwrap();
    assert_eq!(failure.result["next_profile_id"], a);
    failure.command_id = new_id("command");
    st.commit(failure).unwrap();
    assert_eq!(
        st.snapshot().records("llm_profile_health")[0].data["trips"],
        0
    );
    let mut failure = artex_domain::conversation::profile_failure_plan(
        &st.snapshot(),
        &c,
        &run2,
        &json!({"profile_id":b,"status":401,"message":"invalid key","committed":false}),
    )
    .unwrap();
    failure.command_id = new_id("command");
    st.commit(failure).unwrap();
    assert_eq!(
        st.snapshot().records("llm_profile_health")[0].data["trips"],
        1
    );
    let mut success =
        artex_domain::conversation::profile_success_plan(&st.snapshot(), &c, &b).unwrap();
    success.command_id = new_id("command");
    st.commit(success).unwrap();
    assert!(st.snapshot().records("llm_profile_health").is_empty());
}
#[test]
fn llm_ui_records_and_structured_transcripts_project_source_dtos() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "LLM records").unwrap();
    let c = ctx();
    let mut seed = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    seed.command_id = new_id("command");
    st.commit(seed).unwrap();
    let task = create(&mut st, &c, "one", vec![]);
    let s = st.snapshot();
    let exp = s.get("tasks", &task).unwrap().data["exploration_id"].clone();
    let recordid = raw_put(
        &mut st,
        "llm_records",
        "llmrecord",
        json!({"model":"test","task_id":task,"session_id":"run:s1","profile_name":"Local","worker":"worker","status":"error","error":"limited","latency_ms":4,"input_tokens":7,"output_tokens":2,"cache_read":3,"cache_write":1,"request_body":body("request"),"response_body":body("response")}),
    );
    raw_put(
        &mut st,
        "llm_usage",
        "llmusage",
        json!({"task_id":task,"model":"test","worker":"worker","input_tokens":7,"output_tokens":2,"cache_read":3,"cache_write":1}),
    );
    let useid = raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"worker":"worker","kind":"tool_use","tool":"Read","tool_use_id":"call-1","detail":body("{\"file_path\":\"a\"}")}),
    );
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"worker":"worker","kind":"tool_result","tool":"Read","tool_use_id":"call-1","is_error":true,"detail":body(&json!({"run_id":new_id("run"),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"file not found"}]}}).to_string())}),
    );
    let s = st.snapshot();
    let read = |v: &Value| st.blob_read(v, 0, 65536);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    for (m, p) in [
        ("ui.getLLM", json!({})),
        ("ui.llmPool", json!({})),
        ("ui.taskLLMResolution", json!({"id":task})),
        ("ui.llmRecords", json!({"params":{"session":"S1"}})),
        ("ui.llmRecordDetail", json!({"id":recordid})),
        ("ui.llmTasks", json!({})),
        ("ui.tokensByModel", json!({"task":task})),
        ("ui.commands", json!({"params":{"task":task}})),
    ] {
        let out = artex_domain::llm_ui::query(m, &p, &s, &c, &read).unwrap();
        artex_core::validate_contract(&contract["results"][m], &contract, &out)
            .unwrap_or_else(|e| panic!("{m}: {e}"));
        if m == "ui.tokensByModel" {
            assert_eq!(out["models"][0]["cache_read_tokens"], 3);
        }
        if m == "ui.commands" {
            assert_eq!(out["commands"][0]["id"], useid);
        }
    }
    let payload=body(&json!({"run_id":new_id("run"),"message":{"role":"assistant","content":[{"type":"thinking","thinking":"reason"},{"type":"text","text":"visible answer"}]}}).to_string());
    assert_eq!(
        artex_domain::transcript_ui::body(&payload, &read).unwrap()["text"],
        "reason\nvisible answer"
    );
    let binary = st
        .blob_put(
            payload["text"].as_str().unwrap().as_bytes(),
            "application/json",
        )
        .unwrap();
    let read = |v: &Value| st.blob_read(v, 0, 65536);
    assert_eq!(
        artex_domain::transcript_ui::body(&json!({"text":null,"blob":binary}), &read).unwrap()["text"],
        "reason\nvisible answer"
    );
}
#[test]
fn task_deletion_preserves_shared_assets_historical_usage_and_optional_findings() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "delete task").unwrap();
    let mut c = ctx();
    let first = create(&mut st, &c, "first", vec![]);
    let second = create(&mut st, &c, "second", vec![]);
    c.task_id = Some(first.clone());
    let exclusive = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"private.example.com"}),
        &c,
    )["ids"][0]
        .clone();
    let shared = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"shared.example.org"}),
        &c,
    )["ids"][0]
        .clone();
    assets_ui(
        &mut st,
        "ui.attachTaskAssets",
        json!({"taskId":second,"assetIds":[shared],"sourceSummary":"shared"}),
        &c,
    );
    let finding=apply(&mut st,"report_finding",json!({"vulnclass":"IDOR","severity":"high","summary":"keep historical evidence","asset_ids":[shared]}),&c)["finding_id"].clone();
    raw_put(
        &mut st,
        "llm_usage",
        "llmusage",
        json!({"task_id":first,"model":"test","input_tokens":13}),
    );
    let private_traffic = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"host":"private.example.com","method":"GET","url":"https://private.example.com/","status":200}),
    );
    let shared_traffic = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"host":"shared.example.org","method":"GET","url":"https://shared.example.org/","status":200}),
    );
    raw_patch(
        &mut st,
        "assets",
        shared.as_str().unwrap(),
        json!({"task_ids":[]}),
    );
    let mut b=artex_domain::task_delete_ui::plan("ui.deleteTask",&json!({"id":first,"options":{"delete_assets":true,"delete_traffic":true,"delete_files":false,"delete_findings":false,"delete_llm_records":true}}),&st.snapshot(),&c).unwrap();
    assert_eq!(b.result["assets_deleted"], 1);
    assert_eq!(b.result["assets_detached"], 0);
    assert_eq!(b.result["traffic_deleted"], 1);
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let s = st.snapshot();
    assert!(s.get("tasks", &first).is_none());
    assert!(s.get("assets", exclusive.as_str().unwrap()).is_none());
    assert!(s.get("assets", shared.as_str().unwrap()).is_some());
    assert!(s.get("exchanges", &private_traffic).is_none());
    assert!(s.get("exchanges", &shared_traffic).is_some());
    assert!(s.get("findings", finding.as_str().unwrap()).is_some());
    assert_eq!(s.records("llm_usage")[0].data["input_tokens"], 13);
}
#[test]
fn chat_mention_typed_snapshots_and_log_cursors_are_stable_per_row() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "mention logs").unwrap();
    let c = ctx();
    let asset = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"root_domain","domain":"example.com"}),
        &c,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let s = st.snapshot();
    let page = artex_domain::mentions_ui::query(
        "ui.chatMentions",
        &json!({"kind":"root_domain","query":"example"}),
        &s,
        &c,
    )
    .unwrap();
    assert_eq!(page["items"][0]["id"], asset);
    let mut b = artex_domain::Planner::new(&s, "mention", &json!({}));
    let msg = artex_domain::mentions_ui::compose(
        &mut b,
        &format!("Review @[도메인#{asset} example.com]"),
    )
    .unwrap();
    assert!(msg.contains("Snapshots of records referenced"));
    assert_eq!(b.batch.expected[0].id, asset);
    let mut b = artex_domain::empty_batch("logs", &json!({}));
    b.command_id = new_id("command");
    for n in 0..3 {
        let mut data = default_data("server_logs").unwrap();
        data["text"] = json!(format!("line {n}"));
        b.changes.push(Change::Put {
            collection: "server_logs".into(),
            id: new_id("log"),
            data,
        });
    }
    st.commit(b).unwrap();
    let s = st.snapshot();
    let first =
        artex_domain::logs_ui::query("legacy.getLogs", &json!({"query":{"limit":1}}), &s, &c)
            .unwrap();
    let next = artex_domain::logs_ui::query(
        "legacy.getLogs",
        &json!({"query":{"since":first["cursor"],"limit":2}}),
        &s,
        &c,
    )
    .unwrap();
    assert_eq!(next["items"].as_array().unwrap().len(), 2);
    assert_ne!(next["items"][0]["db_id"], first["items"][0]["db_id"]);
}

#[test]
fn side_questions_freeze_parent_context_dedupe_and_reject_late_cancel_clear_results() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "Side question").unwrap();
    let c = ctx();
    let mut boot = artex_domain::bootstrap_plan(&st.snapshot(), &c).unwrap();
    boot.command_id = new_id("command");
    st.commit(boot).unwrap();
    let profile=config(&mut st,"ui.saveLLMProfile",json!({"p":{"name":"Side","format":"openai","model":"one","api_key":{"secret_ref":new_id("secret"),"hint":"key"},"context_window_k":64}}),&c)["id"].as_str().unwrap().to_owned();
    let agent = st
        .snapshot()
        .records("agents")
        .into_iter()
        .find(|r| r.data["key"] == "mainagent")
        .unwrap()
        .header
        .id
        .clone();
    let conv = apply(
        &mut st,
        "conversation.create",
        json!({"agent_key":"mainagent","title":"Parent"}),
        &c,
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"conversation_id":conv,"engine_epoch":c.actor.engine_epoch,"state":"running"}),
    );
    let s = st.snapshot();
    let mut checkpoint = artex_domain::side_questions::checkpoint_plan(
        &s,
        &c,
        &run,
        1,
        s.get("llm_profiles", &profile).unwrap(),
        &json!({"messages":[{"role":"user","content":"original evidence"}]}),
    )
    .unwrap();
    checkpoint.command_id = new_id("command");
    st.commit(checkpoint).unwrap();
    let routes = artex_core::contracts::read("legacy-routes.v1.json");
    let route = |http: &str| {
        routes["routes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["handler"] == "handleSideQuestions"
                    && r["parent_kind"] == "conversation"
                    && r["legacy_http_method"] == http
            })
            .unwrap()["route_id"]
            .clone()
    };
    let params = json!({"route_id":route("POST"),"path":{"id":conv},"query":{},"body":{"question":"What is known?","client_request_id":"side-test-1"}});
    let mut b = artex_domain::side_questions::plan(
        "legacy.handleSideQuestions",
        &params,
        &st.snapshot(),
        &c,
    )
    .unwrap();
    let id = b.result["id"].as_str().unwrap().to_owned();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let repeated = artex_domain::side_questions::plan(
        "legacy.handleSideQuestions",
        &params,
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert!(repeated.changes.is_empty());
    assert_eq!(repeated.result["id"], id);
    let mut changed = params.clone();
    changed["body"]["question"] = json!("Another question");
    assert_eq!(
        artex_domain::side_questions::plan(
            "legacy.handleSideQuestions",
            &changed,
            &st.snapshot(),
            &c
        )
        .unwrap_err()
        .code,
        "VERSION_CONFLICT"
    );
    let s = st.snapshot();
    let mut checkpoint = artex_domain::side_questions::checkpoint_plan(
        &s,
        &c,
        &run,
        2,
        s.get("llm_profiles", &profile).unwrap(),
        &json!({"messages":[{"role":"user","content":"new evidence"}]}),
    )
    .unwrap();
    checkpoint.command_id = new_id("command");
    st.commit(checkpoint).unwrap();
    let input = artex_domain::side_questions::execution_input(&st.snapshot(), &id).unwrap();
    assert_eq!(
        input["snapshot"]["request"]["messages"][0]["content"],
        "original evidence"
    );
    assert!(input["tools"].as_array().unwrap().is_empty());
    assert_eq!(input["snapshot"]["model"]["window_tokens"], 64000);
    let generation = input["generation"].as_str().unwrap();
    let cancel = json!({"path":{"requestID":id}});
    let mut b =
        artex_domain::side_questions::plan("legacy.cancelSideRequest", &cancel, &st.snapshot(), &c)
            .unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let late = artex_domain::side_questions::update_plan(
        &st.snapshot(),
        &c,
        &id,
        generation,
        &body("late answer"),
        "completed",
        "",
        &json!({}),
        &json!({}),
        None,
    )
    .unwrap();
    assert_eq!(late.result["saved"], false);
    let history = artex_domain::side_questions::query(
        "legacy.handleSideQuestions",
        &json!({"route_id":route("GET"),"path":{"id":conv},"query":{},"body":null}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    assert_eq!(history["items"][0]["status"], "cancelled");
    assert!(
        history["items"][0]["context"]
            .get("frozen_snapshot")
            .is_none()
    );
    let mut b = artex_domain::side_questions::plan(
        "legacy.handleSideQuestions",
        &json!({"route_id":route("DELETE"),"path":{"id":conv},"query":{},"body":null}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    assert!(st.snapshot().get("side_question_requests", &id).is_none());
    assert_eq!(
        artex_domain::side_questions::update_plan(
            &st.snapshot(),
            &c,
            &id,
            generation,
            &body("late"),
            "completed",
            "",
            &json!({}),
            &json!({}),
            None
        )
        .unwrap()
        .result["saved"],
        false
    );
}

#[test]
fn source_asset_derivations_merge_hosts_ips_ports_without_returning_extra_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "Derived assets").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "Derived", vec![]));
    let dns = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"subdomain","domain":"Api.Example.com.","record_type":"A","record_value":["192.0.2.1, 192.0.2.2","invalid" ]}]}),
        &c,
    );
    assert_eq!(dns["ids"].as_array().unwrap().len(), 1);
    let s = st.snapshot();
    assert_eq!(s.records("assets").len(), 4);
    assert_eq!(
        s.get("assets", dns["ids"][0].as_str().unwrap())
            .unwrap()
            .data["ip"],
        "192.0.2.1"
    );
    assert!(
        s.records("assets")
            .iter()
            .filter(|r| r.data["type"] == "ip")
            .all(|r| r.data["bound_domains"] == json!(["api.example.com"]))
    );
    let http = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"service","url":"https://api.example.com/","ip":"192.0.2.1"}]}),
        &c,
    );
    assert_eq!(http["ids"].as_array().unwrap().len(), 1);
    let s = st.snapshot();
    assert_eq!(s.records("assets").len(), 6);
    let ip = s
        .records("assets")
        .into_iter()
        .find(|r| r.data["type"] == "ip" && r.data["ip"] == "192.0.2.1")
        .unwrap();
    assert_eq!(
        ip.data["open_ports"],
        json!([{"port":443,"service":"HTTPS"}])
    );
    let endpoint = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"endpoint","url":"https://api.example.com/items","method":"get","ip":"192.0.2.1"}]}),
        &c,
    );
    assert_eq!(endpoint["ids"].as_array().unwrap().len(), 1);
    assert_eq!(st.snapshot().records("assets").len(), 7);
    let repeated = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"service","url":"https://api.example.com/","ip":"192.0.2.1"}]}),
        &c,
    );
    assert_eq!(repeated["ids"], http["ids"]);
    assert_eq!(st.snapshot().records("assets").len(), 7);
}
#[test]
fn canonical_traffic_search_ranges_and_evidence_export_zip_use_actual_bytes() {
    use base64::Engine;
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "Canonical traffic").unwrap();
    let mut c = ctx();
    c.task_id = Some(create(&mut st, &c, "Traffic report", vec![]));
    let blob = st
        .blob_put(b"verified response bytes", "application/octet-stream")
        .unwrap();
    let traffic = raw_put(
        &mut st,
        "exchanges",
        "traffic",
        json!({"host":"example.com","url":"https://example.com:8443/item","method":"GET","status":200,"ts":"1720000001","req_len":"0","resp_len":"23"}),
    );
    let mut b = artex_domain::empty_batch("fixture.capture", &json!({}));
    let mut d = default_data("exchange_bodies").unwrap();
    artex_domain::merge(
        &mut d,
        &json!({"id":traffic,"req_head":body("GET /item HTTP/1.1"),"resp_head":body("HTTP/1.1 200 OK"),"resp_body":blob}),
    );
    b.changes.push(Change::Put {
        collection: "exchange_bodies".into(),
        id: new_id("trafficbody"),
        data: d,
    });
    b.command_id = new_id("command");
    st.commit(b).unwrap();
    let finding=apply(&mut st,"report_finding",json!({"vulnclass":"IDOR","severity":"high","summary":"Export evidence","evidence":"PoC","traffic_refs":[{"traffic_id":traffic,"role":"proof"}]}),&c)["finding_id"].as_str().unwrap().to_owned();
    let s = st.snapshot();
    let binding = s.records("finding_traffic_bindings")[0].header.id.clone();
    let reader = |v: &Value| st.blob_read(v, 0, 65536);
    let search = artex_domain::traffic_ui::tool_query(
        "traffic_search",
        &json!({"host":"https://example.com:8443/path","body_contains":"response"}),
        &s,
        &c,
        &reader,
    )
    .unwrap();
    assert_eq!(search[0]["id"], traffic);
    assert!(search[0].get("host").is_none());
    assert_eq!(
        artex_domain::traffic_ui::tool_query(
            "traffic_search",
            &json!({"host":"example.com:443","body_contains":"response"}),
            &s,
            &c,
            &reader
        )
        .unwrap(),
        "No matching traffic."
    );
    assert_eq!(
        artex_domain::traffic_ui::tool_query(
            "traffic_search",
            &json!({"host":"example.com","body_contains":"x"}),
            &s,
            &c,
            &reader
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    let part = artex_domain::traffic_ui::tool_query(
        "get_finding_traffic",
        &json!({"finding_id":finding,"binding_id":binding,"offset":9,"length":8}),
        &s,
        &c,
        &reader,
    )
    .unwrap();
    assert_eq!(part["head"], "HTTP/1.1 200 OK");
    assert_eq!(part["body"]["content"], "response");
    assert_eq!(part["body"]["next_offset"], 17);
    let export = |format: &str| {
        let out = artex_domain::report_export::export(
            &json!({"opts":{"scope":"selected","format":format,"ids":[finding]}}),
            &s,
            &reader,
        )
        .unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(out["content_base64"]["text"].as_str().unwrap())
            .unwrap()
    };
    let bytes = export("md-zip");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let prefix = format!("evidence/{finding}/{binding}/");
    let mut evidence = vec![];
    zip.by_name(&format!("{prefix}response.bin"))
        .unwrap()
        .read_to_end(&mut evidence)
        .unwrap();
    assert_eq!(evidence, b"verified response bytes");
    assert!(zip.by_name(&format!("{prefix}request.http")).is_ok());
    assert!(zip.by_name(&format!("{prefix}manifest.json")).is_ok());
    let csv = String::from_utf8(export("csv")).unwrap();
    assert!(csv.contains("Number of traffic evidence"));
    assert!(!csv.contains("verified response bytes"));
    let json: Value = serde_json::from_slice(&export("json")).unwrap();
    assert_eq!(json[0]["evidence"], "PoC");
}

#[test]
fn explicit_task_budget_recovery_requires_user_confirmation_monotonic_time_and_cas() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "budget recovery").unwrap();
    let c = ctx();
    let id = apply(
        &mut st,
        "task.create",
        json!({"description":"recovery","timeout_seconds":60}),
        &c,
    )["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rid = st.snapshot().records("task_runtime")[0].header.id.clone();
    raw_patch(
        &mut st,
        "tasks",
        &id,
        json!({"status":"paused","paused":true}),
    );
    raw_patch(
        &mut st,
        "task_runtime",
        &rid,
        json!({"active_elapsed_ms":"10000","remaining_timeout_ms":"50000","accounting_state":"recovery_required","last_active_clock":"25"}),
    );
    assert_eq!(
        plan("task.resume", &json!({"task_id":id}), &st.snapshot(), &c)
            .unwrap_err()
            .code,
        "RECOVERY_REQUIRED"
    );
    let mut agent = c.clone();
    agent.actor.kind = "agent".into();
    assert_eq!(
        plan(
            "task.recover_budget",
            &json!({"task_id":id,"mode":"use_recorded","elapsed_active_ms":null}),
            &st.snapshot(),
            &agent
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    for elapsed in ["9999", "60001"] {
        assert_eq!(
            plan(
                "task.recover_budget",
                &json!({"task_id":id,"mode":"override","elapsed_active_ms":elapsed}),
                &st.snapshot(),
                &c
            )
            .unwrap_err()
            .code,
            "INVALID_PARAMS"
        );
    }
    let params = json!({"id":id,"mode":"override","elapsedActiveMs":"20000"});
    let mut b =
        artex_domain::task_control_ui::plan("ui.recoverTaskBudget", &params, &st.snapshot(), &c)
            .unwrap();
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(
        &contract["results"]["ui.recoverTaskBudget"],
        &contract,
        &b.result,
    )
    .unwrap();
    assert!(
        b.expected
            .iter()
            .any(|e| e.collection == "tasks" && e.id == id)
    );
    assert!(
        b.expected
            .iter()
            .any(|e| e.collection == "task_runtime" && e.id == rid)
    );
    b.command_id = new_id("command");
    let result = st.commit(b).unwrap().result;
    assert_eq!(result["remaining_timeout_ms"], "40000");
    let s = st.snapshot();
    let runtime = s.get("task_runtime", &rid).unwrap();
    assert!(runtime.data["last_active_clock"].is_null());
    assert_eq!(runtime.data["accounting_state"], "known");
    assert_eq!(s.get("tasks", &id).unwrap().data["status"], "paused");
    assert_eq!(
        plan(
            "task.recover_budget",
            &json!({"task_id":id,"mode":"use_recorded"}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "ALREADY_COMPLETED"
    );
    assert!(plan("task.resume", &json!({"task_id":id}), &s, &c).is_ok());
    raw_patch(
        &mut st,
        "task_runtime",
        &rid,
        json!({"accounting_state":"recovery_required"}),
    );
    let mut stale = plan(
        "task.recover_budget",
        &json!({"task_id":id,"mode":"use_recorded"}),
        &st.snapshot(),
        &c,
    )
    .unwrap();
    stale.command_id = new_id("command");
    raw_patch(
        &mut st,
        "task_runtime",
        &rid,
        json!({"active_elapsed_ms":"21000"}),
    );
    assert_eq!(st.commit(stale).unwrap_err().code, "VERSION_CONFLICT");
    apply(
        &mut st,
        "task.recover_budget",
        json!({"task_id":id,"mode":"override","elapsed_active_ms":"60000"}),
        &c,
    );
    assert_eq!(
        plan("task.resume", &json!({"task_id":id}), &st.snapshot(), &c)
            .unwrap_err()
            .code,
        "CANCELLED"
    );
}

#[test]
fn canonical_tool_nonempty_views_match_source_ids_goals_trace_output_and_overview() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "tool projections").unwrap();
    let c = ctx();
    let source = create(&mut st, &c, "source", vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let mut inherited = c.clone();
    inherited.task_id = Some(source.clone());
    let goal = apply(
        &mut st,
        "set_goals",
        json!({"task_id":task,"goals":[{"text":"Verify authorization"}]}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let iid = apply(
        &mut st,
        "add_intent",
        json!({"task_id":task,"summary":"Inspect login"}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let src_iid = apply(
        &mut st,
        "add_intent",
        json!({"task_id":source,"summary":"Read inherited evidence"}),
        &inherited,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    raw_patch(
        &mut st,
        "exploration_nodes",
        &src_iid,
        json!({"state":"done"}),
    );
    let result = apply(
        &mut st,
        "report_finding",
        json!({"task_id":task,"intent_id":iid,"summary":"Confirmed bypass","vulnclass":"authorization","severity":"high","evidence":"request proof"}),
        &own,
    );
    let fid = result["finding_id"].as_str().unwrap().to_owned();
    let node = result["finding_node_id"].as_str().unwrap().to_owned();
    let exp = st.snapshot().get("tasks", &task).unwrap().data["exploration_id"].clone();
    let source_exp = st.snapshot().get("tasks", &source).unwrap().data["exploration_id"].clone();
    let final_bytes=br#"{"run_id":"test-run","message":{"role":"assistant","content":[{"type":"thinking","thinking":"private embedded reasoning"},{"type":"text","text":"final result from verified blob"}]}}"#;
    let reference = st.blob_put(final_bytes, "text/plain").unwrap();
    let final_id = raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"node_id":iid,"kind":"result","summary":"Summary result","detail":{"text":null,"blob":reference}}),
    );
    let first = raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"node_id":iid,"kind":"tool_result","tool":"Read","summary":"outline\nsecond line","detail":body("Full tool output secret marker")}),
    );
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"node_id":iid,"kind":"thinking","summary":"private thought","detail":body("never expose")}),
    );
    raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":source_exp,"node_id":src_iid,"kind":"result","summary":"Source final","detail":body("source terminal output")}),
    );
    let snapshot = st.snapshot();
    let reader = |r: &Value| st.blob_read(r, 0, r["length"].as_str().unwrap().parse().unwrap());
    let query = |name: &str, p: Value| {
        artex_domain::tool_projection::query(name, &p, &snapshot, &own, &reader).unwrap()
    };
    let finding = query("list_findings", json!({}));
    assert_eq!(finding[0]["id"], node);
    assert_eq!(finding[0]["finding_id"], fid);
    assert_eq!(finding[0]["finding_node_id"], node);
    assert_eq!(finding[0]["intent_id"], iid);
    assert!(finding[0].get("evidence").is_none());
    let goals = query("list_goals", json!({}));
    assert_eq!(goals[0]["id"], goal);
    assert_eq!(goals[0]["payload"]["text"], "Verify authorization");
    let out = query("get_worker_output", json!({"intent_id":iid}));
    assert_eq!(out["final_text"], "final result from verified blob");
    assert!(out.get("results").is_none());
    let trace = query("get_worker_trace", json!({"intent_id":iid}));
    assert_eq!(trace["steps"].as_array().unwrap().len(), 2);
    assert!(
        trace["steps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("detail").is_none())
    );
    assert_eq!(trace["steps"][1]["summary"], "outline");
    let detail = query(
        "get_worker_trace",
        json!({"intent_id":iid,"step_ids":[first,first,final_id]}),
    );
    assert_eq!(detail["returned_step_ids"].as_array().unwrap().len(), 2);
    assert!(
        detail["steps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["detail"] == "Full tool output secret marker")
    );
    let hits = query("search_all_worker_traces", json!({"q":"secret marker"}));
    assert_eq!(hits["hits"][0]["step_id"], first);
    assert_eq!(hits["hits"][0]["intent_id"], iid);
    assert!(hits["hits"][0].get("detail").is_none());
    let inherited_output = query("get_worker_output", json!({"intent_id":src_iid}));
    assert_eq!(inherited_output["source_task_id"], source);
    assert_eq!(inherited_output["final_text"], "source terminal output");
    let graph = query("graph_overview", json!({}));
    assert_eq!(graph["task"]["description"], "current");
    assert_eq!(graph["goals"][0]["text"], "Verify authorization");
    assert_eq!(graph["frontier_open"], 1);
    assert_eq!(graph["finding_list"][0]["from_intent"], iid);
    assert_eq!(graph["findings_total"], 1);
    assert!(graph.get("nodes").is_none());
    assert_eq!(graph["related_tasks"][0]["source_task_id"], source);
    assert_eq!(
        graph["related_tasks"][0]["recent_intent_results"][0]["result_summary"],
        "Source final"
    );
    assert!(graph.get("coverage").is_some());
}

#[test]
fn persisted_stream_blob_promotes_public_marker_and_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "stream projection").unwrap();
    let cid = raw_put(
        &mut st,
        "conversations",
        "conversation",
        json!({"agent_key":"auto","title":"stream"}),
    );
    let run = new_id("run");
    let text = "한국어 streamed fragment ".repeat(1000);
    let envelope = json!({"stream":{"run_id":run,"turn_id":"turn-1","pending":true},"text":text});
    let reference = st
        .blob_put(&serde_json::to_vec(&envelope).unwrap(), "application/json")
        .unwrap();
    let aid = raw_put(
        &mut st,
        "conversation_activities",
        "conversationactivity",
        json!({"conversation_id":cid,"kind":"text","summary":"fragment","detail":{"text":null,"blob":reference}}),
    );
    let s = st.snapshot();
    let c = ctx();
    let result = artex_domain::query("conversation.messages", &json!({"id":cid}), &s, &c).unwrap();
    let reader = |r: &Value| st.blob_read(r, 0, r["length"].as_str().unwrap().parse().unwrap());
    let wire = artex_domain::transcript_ui::project_with_snapshot(
        "ui.conversationMessages",
        result,
        &s,
        &reader,
    )
    .unwrap();
    assert_eq!(wire["items"][0]["stream"], envelope["stream"]);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(
        &contract["results"]["ui.conversationMessages"],
        &contract,
        &wire,
    )
    .unwrap();
    let raw =
        artex_domain::query("conversation.detail", &json!({"id":cid,"seq":aid}), &s, &c).unwrap();
    assert_eq!(
        artex_domain::transcript_ui::project("ui.conversationMsgDetail", raw, &reader).unwrap(),
        body(&text)
    );
}

#[test]
fn primary_asset_registration_auto_scopes_anchors_and_gates_without_derivative_scope_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "source asset registration").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "asset scope", vec![]);
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let intent = apply(
        &mut st,
        "add_intent",
        json!({"summary":"Discover selected host","priority":0}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    own.intent_id = Some(intent.clone());
    let input = json!({"assets":[{"type":"service","url":"https://api.example.com","service_ip":"192.0.2.8","technologies":["nginx"]}]});
    let result = apply(&mut st, "insert_assets", input.clone(), &own);
    let aid = result["ids"][0].as_str().unwrap();
    let s = st.snapshot();
    assert_eq!(s.get("assets", aid).unwrap().data["ip"], "192.0.2.8");
    assert_eq!(
        s.get("assets", aid).unwrap().data["technologies"],
        json!(["nginx"])
    );
    let scopes = s.records("task_scope");
    assert_eq!(scopes.len(), 1);
    assert_eq!(scopes[0].data["kind"], "subdomain");
    assert_eq!(scopes[0].data["domain"], "api.example.com");
    assert_eq!(scopes[0].data["source"], "auto");
    assert!(
        s.records("exploration_anchors")
            .iter()
            .any(|r| r.data["node_id"] == intent && r.data["asset_id"] == aid)
    );
    let link = s
        .records("task_asset_links")
        .into_iter()
        .find(|r| r.data["asset_id"] == aid)
        .unwrap();
    assert_eq!(link.data["source_node_id"], intent);
    let view = artex_domain::tool_projection::query(
        "list_assets",
        &json!({"dsl":"domain=api.example.com"}),
        &s,
        &own,
        &|_| unreachable!(),
    )
    .unwrap();
    assert!(
        view["assets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["id"] == aid)
    );
    assert!(
        !view["assets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["type"] == "root_domain")
    );
    let wire = artex_domain::tool_projection::mutation_result("insert_assets", &input, &result);
    assert_eq!(
        wire,
        json!({"results":[{"index":0,"id":aid,"type":"service"}],"errors":[]})
    );
    let rule = raw_put(
        &mut st,
        "asset_intercept_rules",
        "assetrule",
        json!({"kind":"fuzzy_domain","pattern":"example.com","enabled":true}),
    );
    let out = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"subdomain","domain":"blocked.example.com"},{"type":"ip","ip":"2001:db8::10"}]}),
        &own,
    );
    assert!(out["ids"][0].is_null());
    assert!(
        out["errors"]["0"]["message"]
            .as_str()
            .unwrap()
            .contains("blocked")
    );
    assert!(out["ids"][1].is_string());
    assert_eq!(
        plan(
            "add_intent",
            &json!({"summary":"blocked target","asset_ids":[aid]}),
            &st.snapshot(),
            &own
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    raw_patch(
        &mut st,
        "asset_intercept_rules",
        &rule,
        json!({"enabled":false}),
    );
    raw_put(
        &mut st,
        "task_intercept_rules",
        "taskrule",
        json!({"task_id":task,"action":"allow","kind":"exact_domain","pattern":"api.example.com","enabled":true}),
    );
    assert!(
        plan(
            "add_intent",
            &json!({"summary":"whitelisted target","asset_ids":[aid]}),
            &st.snapshot(),
            &own
        )
        .is_ok()
    );
    let out = apply(
        &mut st,
        "insert_assets",
        json!({"assets":[{"type":"ip","ip":"192.0.2.99"}]}),
        &own,
    );
    assert!(out["ids"][0].is_null());
}

#[test]
fn goal_and_constraint_batches_follow_source_defaults_lineage_and_partial_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "source goal constraints").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "goal", vec![]);
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let goals = json!({"goals":[{"text":"  Verify access  "},{"text":" "}]});
    let result = apply(&mut st, "set_goals", goals.clone(), &own);
    let gid = result["ids"][0].as_str().unwrap();
    assert!(result["ids"][1].is_null());
    let s = st.snapshot();
    let g = s.get("exploration_nodes", gid).unwrap();
    assert_eq!(g.data["priority"], 0);
    assert_eq!(g.data["origin"], "system");
    assert_eq!(g.data["payload"]["text"], "Verify access");
    assert!(
        s.records("exploration_edges")
            .iter()
            .any(|e| e.data["dst_id"] == gid && e.data["rel"] == "spawns")
    );
    let constraints = json!({"constraints":[{"text":"  Passive only  ","type":" ALLOW "},{"text":"invalid","type":"typo"},{"text":"Do not write"}]});
    let result = apply(&mut st, "set_constraints", constraints.clone(), &own);
    assert!(result["ids"][0].is_string());
    assert!(result["ids"][1].is_null());
    assert!(result["ids"][2].is_string());
    let s = st.snapshot();
    assert_eq!(s.records("task_constraints").len(), 2);
    assert_eq!(
        s.get("task_constraints", result["ids"][0].as_str().unwrap())
            .unwrap()
            .data["text"],
        "Passive only"
    );
    let wire =
        artex_domain::tool_projection::mutation_result("set_constraints", &constraints, &result);
    assert_eq!(wire["errors"]["1"], "type must be allow or deny");
    let single = apply(&mut st, "set_goals", json!({"text":"single"}), &own);
    assert_eq!(
        artex_domain::tool_projection::mutation_result(
            "set_goals",
            &json!({"text":"single"}),
            &single
        ),
        format!("goal added: {}", single["ids"][0].as_str().unwrap())
    );
}

#[test]
fn replaced_stream_cursor_returns_same_commit_final_and_internal_context_rows_stay_hidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "stream replacement cursor").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "stream task", vec![]);
    let exp = st.snapshot().get("tasks", &task).unwrap().data["exploration_id"].clone();
    let cid = raw_put(
        &mut st,
        "conversations",
        "conversation",
        json!({"agent_key":"auto","title":"stream"}),
    );
    let ca = raw_put(
        &mut st,
        "conversation_activities",
        "conversationactivity",
        json!({"conversation_id":cid,"kind":"text","summary":"partial"}),
    );
    let aa = raw_put(
        &mut st,
        "activity",
        "activity",
        json!({"exploration_id":exp,"kind":"text","summary":"partial"}),
    );
    let s = st.snapshot();
    let mut b = artex_domain::Planner::new(&s, "test.stream_complete", &json!({}));
    b.delete("conversation_activities", &ca);
    b.delete("activity", &aa);
    let cf = b
        .insert(
            "conversation_activities",
            "conversationactivity",
            json!({"conversation_id":cid,"kind":"text","summary":"final"}),
        )
        .unwrap();
    let af = b
        .insert(
            "activity",
            "activity",
            json!({"exploration_id":exp,"kind":"text","summary":"final"}),
        )
        .unwrap();
    artex_domain::conversation::push_event(
        &mut b.batch,
        &s,
        &c,
        "ActivityReplaced",
        "conversation_activities",
        &ca,
    )
    .unwrap();
    artex_domain::conversation::push_event(
        &mut b.batch,
        &s,
        &c,
        "ActivityReplaced",
        "activity",
        &aa,
    )
    .unwrap();
    let mut batch = b.done(json!({}));
    batch.command_id = new_id("command");
    st.commit(batch).unwrap();
    let hidden = raw_put(
        &mut st,
        "conversation_activities",
        "conversationactivity",
        json!({"conversation_id":cid,"kind":"context_config","summary":"private context configuration"}),
    );
    raw_put(
        &mut st,
        "conversation_activities",
        "conversationactivity",
        json!({"conversation_id":cid,"kind":"scheduler_retry","summary":"retrying"}),
    );
    let s = st.snapshot();
    let conv = query(
        "conversation.messages",
        &json!({"id":cid,"since":ca}),
        &s,
        &c,
    )
    .unwrap();
    assert_eq!(conv["items"][0]["seq"], cf);
    assert_eq!(conv["items"][1]["kind"], "user");
    assert_eq!(conv["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        query(
            "conversation.detail",
            &json!({"id":cid,"seq":hidden}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
    let activity = artex_domain::exploration_ui::query(
        "ui.activity",
        &json!({"task":task,"opts":{"since":aa}}),
        &s,
        &c,
    )
    .unwrap();
    assert_eq!(activity["items"][0]["seq"], af);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(
        &contract["results"]["ui.conversationMessages"],
        &contract,
        &conv,
    )
    .unwrap();
    artex_core::validate_contract(&contract["results"]["ui.activity"], &contract, &activity)
        .unwrap();
    let history = query(
        "conversation.messages",
        &json!({"id":cid,"before":ca,"history":true}),
        &s,
        &c,
    )
    .unwrap();
    assert!(history["items"].as_array().unwrap().is_empty());
    assert_eq!(
        query(
            "conversation.messages",
            &json!({"id":cid,"since":new_id("conversationactivity")}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "NOT_FOUND"
    );
}

#[test]
fn source_graph_summaries_keep_live_lineage_cold_digests_terminal_results_scope_and_text_budgets() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "graph source parity").unwrap();
    let c = ctx();
    let source = create(&mut st, &c, "한".repeat(900).as_str(), vec![]);
    let task = create(&mut st, &c, "current", vec![source.clone()]);
    let mut src = c.clone();
    src.task_id = Some(source.clone());
    let mut cur = c.clone();
    cur.task_id = Some(task.clone());
    let exp = st.snapshot().get("tasks", &source).unwrap().data["exploration_id"].clone();
    let closed = apply(
        &mut st,
        "add_intent",
        json!({"summary":"terminal cold direction"}),
        &src,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"cold proof","intent_id":closed}),
        &src,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    raw_patch(
        &mut st,
        "exploration_nodes",
        &closed,
        json!({"state":"done"}),
    );
    let digest = raw_put(
        &mut st,
        "exploration_nodes",
        "node",
        json!({"exploration_id":exp,"kind":"digest","state":"active","payload":{"body":body("cold source summary")}}),
    );
    for id in [&closed, &fact] {
        raw_put(
            &mut st,
            "exploration_edges",
            "edge",
            json!({"exploration_id":exp,"src_id":digest,"dst_id":id,"rel":"covers"}),
        );
    }
    let live = apply(
        &mut st,
        "add_intent",
        json!({"summary":"live derived direction","parent_ids":[fact]}),
        &src,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let live_fact = apply(
        &mut st,
        "record_fact",
        json!({"summary":"live private lineage","intent_id":live}),
        &src,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    for n in 0..10 {
        apply(
            &mut st,
            "set_goals",
            json!({"text":format!("{n}: {}","목".repeat(450))}),
            &src,
        );
    }
    apply(
        &mut st,
        "add_task_scope",
        json!({"kind":"subdomain","value":"api.example.com","reason":"declared test target"}),
        &src,
    );
    let asset = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"service","url":"https://api.example.com","service_ip":"192.0.2.10"}),
        &src,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let s = st.snapshot();
    let graph = artex_domain::tool_projection::query(
        "graph_overview",
        &json!({}),
        &s,
        &cur,
        &|_| unreachable!(),
    )
    .unwrap();
    let related = &graph["related_tasks"][0];
    assert_eq!(
        related["task"]["description"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        800
    );
    assert_eq!(related["goals"].as_array().unwrap().len(), 8);
    assert_eq!(
        related["goals"][0]["text"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        400
    );
    assert_eq!(related["node_stats"]["goal"], 10);
    assert_eq!(related["asset_scope"][0]["value"], "api.example.com");
    assert_eq!(related["asset_scope_count"], 1);
    assert_eq!(related["summary_truncated"], true);
    assert_eq!(related["cold_digests"][0]["inherited"], true);
    assert!(
        related["recent_intent_results"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        related["recent_facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["id"] == fact)
    );
    let private = related["recent_facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == live_fact)
        .unwrap();
    assert!(private.get("from_intent").is_none());
    assert_eq!(graph["coverage"]["host_count"], 3); // api.example.com, example.com and enrichment IP all attached to direct source.
    let detail = artex_domain::tool_projection::query(
        "expand_digest",
        &json!({"id":digest}),
        &s,
        &cur,
        &|_| unreachable!(),
    )
    .unwrap();
    assert_eq!(detail["members"].as_array().unwrap().len(), 2);
    assert_eq!(detail["body"], "cold source summary");
    assert!(
        detail["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["inherited"] == true)
    );
    assert!(s.get("assets", &asset).is_some());
}

#[test]
fn explicit_resume_retires_interrupted_runs_without_replaying_uncertain_host_effects() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "restart resume").unwrap();
    let c = ctx();
    let task = create(&mut st, &c, "resumable", vec![]);
    let agent = raw_put(
        &mut st,
        "agents",
        "agent",
        json!({"key":"worker","name":"Worker"}),
    );
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let intent = apply(
        &mut st,
        "add_intent",
        json!({"summary":"resume intent"}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    raw_patch(
        &mut st,
        "exploration_nodes",
        &intent,
        json!({"state":"running"}),
    );
    let run = raw_put(
        &mut st,
        "agent_runs",
        "run",
        json!({"agent_id":agent,"task_id":task,"intent_id":intent,"state":"running","engine_epoch":c.actor.engine_epoch}),
    );
    let runtime = st
        .snapshot()
        .records("task_runtime")
        .into_iter()
        .find(|r| r.data["task_id"] == task)
        .unwrap()
        .header
        .id
        .clone();
    raw_patch(
        &mut st,
        "task_runtime",
        &runtime,
        json!({"last_active_clock":"12345","active_elapsed_ms":"1000"}),
    );
    let question = raw_put(
        &mut st,
        "user_questions",
        "question",
        json!({"run_id":run,"kind":"question","state":"pending","payload":{"questions":[{"question":"Continue?"}]},"generation":"1"}),
    );
    apply(&mut st, "task.pause", json!({"task_id":task}), &c);
    assert_eq!(
        st.snapshot().get("user_questions", &question).unwrap().data["state"],
        "cancelled"
    );
    assert!(
        st.snapshot().get("task_runtime", &runtime).unwrap().data["last_active_clock"].is_null()
    );
    assert_eq!(
        st.snapshot().get("task_runtime", &runtime).unwrap().data["active_elapsed_ms"],
        "1000"
    );
    assert_eq!(
        st.snapshot()
            .get("exploration_nodes", &intent)
            .unwrap()
            .data["state"],
        "paused"
    );
    raw_patch(&mut st, "agent_runs", &run, json!({"state":"interrupted"}));
    let invocation = raw_put(
        &mut st,
        "tool_invocations",
        "invocation",
        json!({"run_id":run,"tool_use_id":"prior-effect","name":"Bash","state":"recovery_required","effect":"external","input":{"command":"irreversible prior effect"},"command_id":new_id("command")}),
    );
    apply(&mut st, "task.resume", json!({"task_id":task}), &c);
    let s = st.snapshot();
    assert_eq!(s.get("tasks", &task).unwrap().data["status"], "running");
    assert_eq!(
        s.get("exploration_nodes", &intent).unwrap().data["state"],
        "open"
    );
    assert_eq!(
        s.get("agent_runs", &run).unwrap().data["state"],
        "cancelled"
    );
    assert_eq!(
        s.get("tool_invocations", &invocation).unwrap().data["state"],
        "recovery_required"
    );
    assert!(
        s.records("domain_events")
            .iter()
            .any(|e| e.data["kind"] == "RunCancelled" && e.data["subject_id"] == run)
    );
}

#[test]
fn canonical_spawn_delete_host_custom_config_and_fact_page_match_source_operations() {
    let dir = tempfile::tempdir().unwrap();
    let mut st = Store::create(dir.path(), "source platform tools").unwrap();
    let c = ctx();
    let task=apply(&mut st,"spawn_task",json!({"description":"spawned task","goal":"verify platform behavior","seed_first_intent":true}),&c)["task_id"].as_str().unwrap().to_owned();
    let mut own = c.clone();
    own.task_id = Some(task.clone());
    let s = st.snapshot();
    assert_eq!(s.get("tasks", &task).unwrap().data["status"], "running");
    assert!(
        s.records("domain_events")
            .iter()
            .any(|e| e.data["kind"] == "TaskCreated" && e.data["subject_id"] == task)
    );
    apply(
        &mut st,
        "record_fact",
        json!({"summary":"나".repeat(180)}),
        &own,
    );
    apply(
        &mut st,
        "record_fact",
        json!({"summary":"newest fact"}),
        &own,
    );
    let s = st.snapshot();
    let page = artex_domain::tool_projection::query(
        "list_facts",
        &json!({"limit":1}),
        &s,
        &own,
        &|_| unreachable!(),
    )
    .unwrap();
    assert_eq!(page["facts"][0]["summary"], "newest fact");
    assert_eq!(page["has_more"], true);
    let next = artex_domain::tool_projection::query(
        "list_facts",
        &json!({"limit":1,"before":page["next_before"]}),
        &s,
        &own,
        &|_| unreachable!(),
    )
    .unwrap();
    assert_eq!(
        next["facts"][0]["summary"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        161
    );
    assert_eq!(next["facts"][0]["summary_truncated"], true);
    assert!(next["facts"][0].get("read_only").is_none());
    apply(
        &mut st,
        "asset.upsert",
        json!({"type":"service","url":"https://api.example.com/login"}),
        &own,
    );
    let foreign = apply(
        &mut st,
        "asset.upsert",
        json!({"type":"subdomain","domain":"api.other.org"}),
        &own,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let deleted = apply(
        &mut st,
        "delete_assets_by_host",
        json!({"host":"EXAMPLE.COM."}),
        &c,
    );
    assert_eq!(deleted["deleted"], 3);
    assert_eq!(deleted["deleted_by_type"]["service"], 1);
    assert!(st.snapshot().get("assets", &foreign).is_some());
    let created = apply(
        &mut st,
        "create_custom_tool",
        json!({"key":" custom_test ","kind":"command","exec":{"command":"echo ok"},"description":"old","agents":["mainagent"],"enabled":false,"deferred":true}),
        &c,
    );
    let id = created["id"].as_str().unwrap();
    apply(
        &mut st,
        "update_custom_tool",
        json!({"key":"custom_test","kind":"command","exec":{"command":"echo updated"}}),
        &c,
    );
    let s = st.snapshot();
    let tool = &s.get("tools", id).unwrap().data;
    assert_eq!(tool["enabled"], true);
    assert_eq!(tool["deferred"], false);
    assert_eq!(tool["agents"], json!([]));
    assert_eq!(tool["description"], "");
    assert_eq!(
        plan(
            "create_custom_tool",
            &json!({"key":"bad_http","kind":"http","exec":{"url":"http://localhost"}}),
            &s,
            &c
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
}
