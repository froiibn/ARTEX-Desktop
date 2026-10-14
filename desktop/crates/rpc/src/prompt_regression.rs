use super::*;
use artex_domain::Planner;

fn fixture(role: &str, template: &str) -> (tempfile::TempDir, Router, String) {
    let (dir, mut router, run_id, context) = regression::seeded();
    let snapshot = router.snapshot().unwrap();
    router
        .internal_commit(artex_domain::bootstrap::bootstrap_plan(&snapshot, &context).unwrap())
        .unwrap();
    let task = if matches!(role, "planner" | "worker" | "goals") {
        let snapshot = router.snapshot().unwrap();
        let batch = artex_domain::plan("task.create", &json!({"description":"Prompt regression","goal":"Verify the explicitly authorized test host"}), &snapshot, &context).unwrap();
        let task = batch.result["task_id"].as_str().unwrap().to_owned();
        router.internal_commit(batch).unwrap();
        Some(task)
    } else {
        None
    };
    let snapshot = router.snapshot().unwrap();
    let agent = snapshot
        .records("agents")
        .into_iter()
        .find(|agent| agent.data["key"] == role)
        .unwrap();
    let mut planner = Planner::new(&snapshot, "test.prompt", &json!({}));
    let prompt = planner.insert("agent_prompts", "prompt", json!({"agent_id":agent.header.id,"version":2,"template_text":artex_core::body(template),"note":"User supplied regression override","updated_by":"user"})).unwrap();
    let mut data = agent.data.clone();
    data["current_prompt_id"] = json!(prompt);
    planner.put("agents", &agent.header.id, data);
    let mut run = snapshot.get("agent_runs", &run_id).unwrap().data.clone();
    run["agent_id"] = json!(agent.header.id);
    if let Some(task) = task {
        run["task_id"] = json!(task);
        run["conversation_id"] = Value::Null;
    }
    planner.put("agent_runs", &run_id, run);
    router.internal_commit(planner.done(json!({}))).unwrap();
    (dir, router, run_id)
}

#[test]
fn bad_stored_template_falls_back_without_rewriting_user_history() {
    let (_dir, router, id) = fixture(
        "mainagent",
        "{{.Unknown}} injected example must never be used",
    );
    let snapshot = router.snapshot().unwrap();
    let before = snapshot.commit_seq;
    let prompt = router
        .prompt_for_run(&snapshot, snapshot.get("agent_runs", &id).unwrap())
        .unwrap();
    assert!(prompt.system.contains("Don’t make up findings"));
    assert!(
        prompt
            .system
            .contains("Intermediate artifact output protocol")
    );
    assert!(!prompt.system.contains("injected example"));
    assert_eq!(router.snapshot().unwrap().commit_seq, before);
    let agent = snapshot
        .get(
            "agents",
            snapshot.get("agent_runs", &id).unwrap().data["agent_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    let stored = snapshot
        .get(
            "agent_prompts",
            agent.data["current_prompt_id"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        stored.data["template_text"]["text"],
        "{{.Unknown}} injected example must never be used"
    );
}

#[test]
fn edited_worker_body_keeps_live_constraints_and_actual_native_cwd() {
    let (dir, mut router, id) = fixture("worker", "CUSTOM WORKER BODY {{.DataDir}}");
    let snapshot = router.snapshot().unwrap();
    let task = snapshot.get("agent_runs", &id).unwrap().data["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let exploration = snapshot.get("tasks", &task).unwrap().data["exploration_id"].clone();
    let mut planner = Planner::new(&snapshot, "test.constraints", &json!({}));
    planner.insert("task_constraints", "constraint", json!({"exploration_id":exploration,"kind":"allow","text":"Read only the authorized local fixture"})).unwrap();
    planner.insert("task_constraints", "constraint", json!({"exploration_id":exploration,"kind":"deny","text":"Never modify remote records"})).unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    let snapshot = router.snapshot().unwrap();
    let prompt = router
        .prompt_for_run(&snapshot, snapshot.get("agent_runs", &id).unwrap())
        .unwrap();
    assert!(prompt.system.starts_with("CUSTOM WORKER BODY"));
    assert!(prompt.system.contains("Operational constraints"));
    assert!(
        prompt
            .system
            .contains("Read only the authorized local fixture")
    );
    assert!(prompt.system.contains("Never modify remote records"));
    assert!(
        prompt.system.contains(
            &dir.path()
                .join("work")
                .join("tasks")
                .join(&task)
                .to_string_lossy()
                .to_string()
        )
    );
    assert!(!prompt.system.contains("work/tasks/"));
    let mut planner = Planner::new(&snapshot, "test.constraints-off", &json!({}));
    planner
        .insert(
            "settings",
            "setting",
            json!({"key":"constraints_inject_worker","value":"false"}),
        )
        .unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    let snapshot = router.snapshot().unwrap();
    let prompt = router
        .prompt_for_run(&snapshot, snapshot.get("agent_runs", &id).unwrap())
        .unwrap();
    assert!(!prompt.system.contains("Operational constraints"));
    assert!(prompt.system.contains("CUSTOM WORKER BODY"));
}

#[test]
fn binding_off_removes_recursive_schema_fields_and_preserves_reporter_identity() {
    let (_dir, router, id) = fixture("reporter", "CUSTOM REPORTER BODY");
    let snapshot = router.snapshot().unwrap();
    let prompt = router
        .prompt_for_run(&snapshot, snapshot.get("agent_runs", &id).unwrap())
        .unwrap();
    assert!(prompt.system.starts_with("CUSTOM REPORTER BODY"));
    assert!(prompt.system.contains("evidence_version"));
    assert!(prompt.system.contains("finding_node_id"));
    assert!(!prompt.tools.iter().any(|tool| matches!(
        tool["name"].as_str(),
        Some("bind_finding_traffic" | "traffic_search" | "traffic_get" | "traffic_blob")
    )));
    for tool in prompt.tools.iter().filter(|tool| {
        matches!(
            tool["name"].as_str(),
            Some("report_finding" | "add_hint" | "add_task_hint")
        )
    }) {
        assert!(!tool["input_schema"].to_string().contains("traffic_refs"));
        assert!(
            !tool["input_schema"]
                .to_string()
                .contains("evidence_hint_id")
        );
    }
}

#[test]
fn builtin_tool_user_description_and_schema_are_preserved_in_model_request() {
    let (_dir, mut router, id) = fixture("worker", "CUSTOM BODY");
    let snapshot = router.snapshot().unwrap();
    let tool = snapshot
        .records("tools")
        .into_iter()
        .find(|tool| tool.data["key"] == "record_fact")
        .unwrap();
    let mut planner = Planner::new(&snapshot, "test.tool-metadata", &json!({}));
    let mut data = tool.data.clone();
    data["description"] = json!("USER TOOL DESCRIPTION");
    data["schema"] = json!({"type":"object","properties":{"description":{"type":"string","description":"USER FIELD DESCRIPTION"}}});
    planner.put("tools", &tool.header.id, data);
    router.internal_commit(planner.done(json!({}))).unwrap();
    let snapshot = router.snapshot().unwrap();
    let prompt = router
        .prompt_for_run(&snapshot, snapshot.get("agent_runs", &id).unwrap())
        .unwrap();
    let tool = prompt
        .tools
        .iter()
        .find(|tool| tool["name"] == "record_fact")
        .unwrap();
    assert_eq!(tool["description"], "USER TOOL DESCRIPTION");
    assert_eq!(
        tool["input_schema"]["properties"]["description"]["description"],
        "USER FIELD DESCRIPTION"
    );
}

#[test]
fn saved_scalar_defaults_reach_invocation_validation_and_trusted_native_input() {
    let (_dir, mut router, id) = fixture("worker", "CUSTOM BODY");
    let snapshot = router.snapshot().unwrap();
    let tool = snapshot
        .records("tools")
        .into_iter()
        .find(|tool| tool.data["key"] == "Bash")
        .unwrap();
    let mut planner = Planner::new(&snapshot, "test.native-default", &json!({}));
    let mut data = tool.data.clone();
    data["schema"] = json!({"type":"object","properties":{"command":{"type":"string","default":"printf fixture-default"},"timeout":{"type":"number","default":5}},"required":["command"]});
    planner.put("tools", &tool.header.id, data);
    let mut run = snapshot.get("agent_runs", &id).unwrap().data.clone();
    run["state"] = json!("running");
    planner.put("agent_runs", &id, run);
    router.internal_commit(planner.done(json!({}))).unwrap();
    let snapshot = router.snapshot().unwrap();
    let context = artex_agent::context_for_run(&snapshot, &id, &router.engine_epoch).unwrap();
    let batch = router.invoke_plan(&json!({"run_id":id,"tool_use_id":"default-fixture","name":"Bash","input":{},"raw_input":null}), &context).unwrap();
    assert_eq!(batch.result["state"], "prepared");
    router.internal_commit(batch).unwrap();
    let native = router.trusted_tool_input(&id, "Bash", &json!({})).unwrap();
    assert_eq!(native["command"], "printf fixture-default");
    assert_eq!(native["timeout"], 5);
    let explicit = router
        .trusted_tool_input(
            &id,
            "Bash",
            &json!({"command":"printf explicit","timeout":0}),
        )
        .unwrap();
    assert_eq!(explicit["command"], "printf explicit");
    assert_eq!(explicit["timeout"], 0);
}
