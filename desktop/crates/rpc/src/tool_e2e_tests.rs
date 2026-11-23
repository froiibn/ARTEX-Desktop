//! Public agent tool calls must preserve their result and effects across reopen.
use super::*;
use artex_domain::Planner;
#[path = "assessment_report_e2e_tests.rs"]
mod assessment_report;

struct Fixture {
    dir: tempfile::TempDir,
    router: Router,
    run: String,
    context: Context,
    task: String,
    company: String,
    asset: String,
    fact: String,
    goal: String,
    intent: String,
    live: String,
    worker: String,
    digest: String,
    finding: String,
    finding_node: String,
    traffic: String,
    hash: Value,
    mcp: String,
}
fn domain(router: &mut Router, method: &str, input: Value, c: &Context) -> Value {
    let plan = artex_domain::plan(method, &input, router.snapshot().unwrap().as_ref(), c).unwrap();
    router.internal_commit(plan).unwrap()
}
fn patch(router: &mut Router, collection: &str, id: &str, patch: Value) {
    let s = router.snapshot().unwrap();
    let old = s.get(collection, id).unwrap();
    let mut data = old.data.clone();
    artex_domain::merge(&mut data, &patch);
    let mut p = Planner::new(&s, "fixture.patch", &json!({}));
    p.expect(old);
    p.put(collection, id, data);
    router.internal_commit(p.done(json!({}))).unwrap();
}
fn fixture() -> Fixture {
    let (dir, mut router, run, mut c) = regression::seeded();
    let mut user = c.clone();
    user.actor.kind = "user".into();
    let task = domain(
        &mut router,
        "task.create",
        json!({"description":"TASK_MARKER","goal":"Fixture goal","seed_first_intent":false}),
        &user,
    )["task_id"]
        .as_str()
        .unwrap()
        .to_owned();
    user.task_id = Some(task.clone());
    let company = domain(
        &mut router,
        "add_company_scope",
        json!({"company":"Fixture Company","scope":"fixture.invalid","reason":"isolated fixture"}),
        &user,
    )["company_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let asset = domain(
        &mut router,
        "insert_assets",
        json!({"assets":[{"type":"service","service_type":"http","url":"https://fixture.invalid/","company_id":company}]}),
        &user,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    domain(
        &mut router,
        "add_task_scope",
        json!({"kind":"root_domain","value":"fixture.invalid","reason":"isolated scope"}),
        &user,
    );
    let fact = domain(
        &mut router,
        "record_fact",
        json!({"summary":"FACT_MARKER","detail":"confirmed fixture evidence"}),
        &user,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let goal = domain(
        &mut router,
        "set_goals",
        json!({"text":"GOAL_MARKER"}),
        &user,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let intent = domain(
        &mut router,
        "add_intent",
        json!({"summary":"TRACE_WORK_MARKER"}),
        &user,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let live = domain(
        &mut router,
        "add_intent",
        json!({"summary":"LIVE_WORK_MARKER"}),
        &user,
    )["ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let finding_result = domain(
        &mut router,
        "report_finding",
        json!({"summary":"FINDING_MARKER","vulnclass":"IDOR","severity":"medium","evidence":"fixture evidence"}),
        &user,
    );
    let finding = finding_result["finding_id"].as_str().unwrap().to_owned();
    let finding_node = finding_result["finding_node_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let blank = router
        .store
        .as_mut()
        .unwrap()
        .blob_put(b"", "application/octet-stream")
        .unwrap();
    let response = router
        .store
        .as_mut()
        .unwrap()
        .blob_put(b"CAPTURED_MARKER", "application/octet-stream")
        .unwrap();
    let head = router
        .store
        .as_mut()
        .unwrap()
        .blob_put(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n",
            "application/octet-stream",
        )
        .unwrap();
    let s = router.snapshot().unwrap();
    let aid = c.actor.agent_id.as_deref().unwrap();
    let cid = c.conversation_id.as_deref().unwrap();
    let exp = s.get("tasks", &task).unwrap().data["exploration_id"].clone();
    let mut p = Planner::new(&s, "fixture.rich", &json!({}));
    let mut rd = s.get("agent_runs", &run).unwrap().data.clone();
    rd["task_id"] = json!(task);
    p.put("agent_runs", &run, rd);
    for tool in artex_tools::list() {
        if tool["family"] == "artex" {
            p.insert(
                "tools",
                "toolconfig",
                json!({"key":tool["name"],"system":true,"enabled":true,"agents":["mainagent"]}),
            )
            .unwrap();
        }
    }
    p.insert(
        "agent_features",
        "agentfeature",
        json!({"agent_id":aid,"questions":true,"plan_mode":true,"memory":true}),
    )
    .unwrap();
    p.insert(
        "settings",
        "setting",
        json!({"key":"agent_traffic_binding","value":"true"}),
    )
    .unwrap();
    p.insert("llm_profiles","profile",json!({"name":"PROFILE_MARKER","model":"fixture","format":"openai","api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}})).unwrap();
    for (id, state) in [(&intent, "done"), (&live, "running")] {
        let mut data = s.get("exploration_nodes", id).unwrap().data.clone();
        data["state"] = json!(state);
        p.put("exploration_nodes", id, data);
    }
    let worker=p.insert("agent_runs","run",json!({"agent_id":aid,"task_id":task,"intent_id":live,"engine_epoch":router.engine_epoch,"state":"running"})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"result","summary":"TRACE_MARKER","detail":artex_core::body("TRACE_MARKER actual result") ,"is_error":false})).unwrap();
    let digest=p.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"digest","state":"active","payload":{"summary":"DIGEST_MARKER","body":artex_core::body("DIGEST_MARKER original context")}})).unwrap();
    p.insert(
        "exploration_edges",
        "edge",
        json!({"exploration_id":exp,"src_id":digest,"dst_id":fact,"rel":"covers"}),
    )
    .unwrap();
    let traffic=p.insert("exchanges",artex_core::collection_prefix("exchanges").unwrap(),json!({"ts":chrono::Utc::now().timestamp().to_string(),"host":"fixture.invalid","url":"https://fixture.invalid/evidence","method":"GET","status":200,"req_len":"0","resp_len":"15","content_type":"text/plain"})).unwrap();
    let mut bodies = artex_core::default_data("exchange_bodies").unwrap();
    artex_domain::merge(
        &mut bodies,
        &json!({"id":traffic,"req_head":artex_core::body("GET /evidence HTTP/1.1\r\n"),"req_body":blank,"resp_head":{"text":null,"blob":head},"resp_body":response}),
    );
    p.put(
        "exchange_bodies",
        &new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),
        bodies,
    );
    p.insert("finding_retests","retest",json!({"finding_id":finding,"conversation_id":cid,"status":"running","snapshot":{"finding":s.get("findings",&finding).unwrap().data,"evidence":[]}})).unwrap();
    p.insert("tools","toolconfig",json!({"key":"fixture_custom","kind":"command","system":false,"enabled":true,"deferred":true,"description":"CUSTOM_MARKER","agents":["mainagent"],"exec":{"command":"echo fixture"},"schema":{"type":"object","properties":{}}})).unwrap();
    let mcp=p.insert("mcp_servers","mcp",json!({"name":"MCP_MARKER","transport":"stdio","command":"fixture-not-launched","enabled":true,"args":[],"env":{}})).unwrap();
    p.insert("memory_records","memory",json!({"agent_id":aid,"name":"MEMORY_MARKER","type":"project","description":"fixture recall","content":artex_core::body("MEMORY_MARKER original memory"),"version":"1"})).unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    c = artex_agent::context_for_run(&router.snapshot().unwrap(), &run, &router.engine_epoch)
        .unwrap();
    Fixture {
        dir,
        router,
        run,
        context: c,
        task,
        company,
        asset,
        fact,
        goal,
        intent,
        live,
        worker,
        digest,
        finding,
        finding_node,
        traffic,
        hash: response["sha256"].clone(),
        mcp,
    }
}
fn request(f: &Fixture, name: &str, input: Value) -> Value {
    let s = f.router.snapshot().unwrap();
    // A real caller supplies versions for its observed objects. Supplying the
    // complete isolated fixture read-set avoids bypassing admission/CAS, while
    // the negative test below proves stale versions cannot write anything.
    let expected=s.collections.values().flat_map(|rows|rows.values()).map(|row|json!({"collection":row.header.collection,"id":row.header.id,"version":row.header.version})).collect::<Vec<_>>();
    json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":"tool.invoke","workspace_id":s.workspace_id,"engine_epoch":f.router.engine_epoch,"command_id":new_id("command"),"expected":expected,"timeout_ms":30000,"params":{"run_id":f.run,"tool_use_id":new_id("tooluse"),"name":name,"input":input,"raw_input":null}})
}
fn isolated_copy(base: &Fixture) -> Fixture {
    fn copy_tree(source: &std::path::Path, target: &std::path::Path) {
        std::fs::create_dir_all(target).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let destination = target.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            assert!(!kind.is_symlink());
            if kind.is_dir() {
                copy_tree(&entry.path(), &destination);
            } else {
                std::fs::copy(entry.path(), destination).unwrap();
            }
        }
    }
    // The baseline is checkpointed and closed before copying. Each case has a
    // distinct native Store lock and files; no effect can leak between tools.
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    copy_tree(&base.dir.path().join("store"), &dir.path().join("store"));
    let mut router = Router::new(BTreeMap::from([("work".into(), work)]));
    router
        .set_execution_capabilities(Some("work".into()), None)
        .unwrap();
    router.engine_epoch = base.router.engine_epoch.clone();
    router.host_epoch = Some(new_id("epoch"));
    router.store = Some(Store::open(&dir.path().join("store")).unwrap());
    Fixture {
        dir,
        router,
        run: base.run.clone(),
        context: base.context.clone(),
        task: base.task.clone(),
        company: base.company.clone(),
        asset: base.asset.clone(),
        fact: base.fact.clone(),
        goal: base.goal.clone(),
        intent: base.intent.clone(),
        live: base.live.clone(),
        worker: base.worker.clone(),
        digest: base.digest.clone(),
        finding: base.finding.clone(),
        finding_node: base.finding_node.clone(),
        traffic: base.traffic.clone(),
        hash: base.hash.clone(),
        mcp: base.mcp.clone(),
    }
}
fn input(f: &Fixture, name: &str) -> Value {
    match name {
        "add_company_scope" => {
            json!({"company":"NEW_COMPANY_MARKER","scope":"new.invalid","reason":"isolated business change"})
        }
        "add_hint" => json!({"text":"NEW_HINT_MARKER"}),
        "add_task_hint" => json!({"task_id":f.task,"text":"NEW_TASK_HINT_MARKER"}),
        "add_intent" => json!({"summary":"NEW_INTENT_MARKER"}),
        "record_fact" => json!({"summary":"NEW_FACT_MARKER","detail":"real fixture fact"}),
        "set_goals" => json!({"text":"NEW_GOAL_MARKER"}),
        "set_constraints" => json!({"text":"NEW_CONSTRAINT_MARKER","type":"deny"}),
        "add_task_scope" => {
            json!({"kind":"subdomain","value":"added.fixture.invalid","reason":"explicit fixture scope"})
        }
        "insert_assets" => {
            json!({"assets":[{"type":"service","service_type":"http","url":"https://new.fixture.invalid/"}]})
        }
        "delete_assets_by_host" => json!({"host":"fixture.invalid"}),
        "list_assets" => json!({"id":f.asset}),
        "create_custom_tool" => {
            json!({"key":"new_fixture","kind":"command","exec":{"command":"echo fixture"},"schema":{"type":"object","properties":{}},"agents":["mainagent"],"description":"NEW_CUSTOM_MARKER"})
        }
        "update_custom_tool" => {
            json!({"key":"fixture_custom","kind":"command","exec":{"command":"echo changed"},"schema":{"type":"object","properties":{}},"agents":["mainagent"],"description":"UPDATED_CUSTOM_MARKER"})
        }
        "create_mcp" => {
            json!({"name":"NEW_MCP_MARKER","transport":"stdio","command":"fixture-not-launched"})
        }
        "update_mcp" => {
            json!({"id":f.mcp,"name":"UPDATED_MCP_MARKER","transport":"stdio","command":"fixture-not-launched"})
        }
        "report_finding" => {
            json!({"summary":"NEW_FINDING_MARKER","vulnclass":"IDOR","severity":"high","evidence":"actual isolated evidence"})
        }
        "update_finding_report" => {
            json!({"finding_node_id":f.finding_node,"report":"UPDATED_REPORT_MARKER","evidence_version":"0"})
        }
        "bind_finding_traffic" => {
            json!({"finding_id":f.finding,"traffic_refs":[{"traffic_id":f.traffic,"role":"proof","note":"TRAFFIC_BIND_MARKER"}]})
        }
        "get_finding_traffic" => json!({"finding_id":f.finding}),
        "record_finding_retest_result" => {
            json!({"verdict":"fixed","summary":"STAGED_RETEST_MARKER","evidence":"confirmed isolated retest evidence"})
        }
        "prove_goal" => {
            json!({"goal_id":f.goal,"evidence_id":f.fact,"reason":"confirmed fixture proof"})
        }
        "goal_met" => json!({"reason":"confirmed fixture goal"}),
        "pause_task" => json!({"task_id":f.task}),
        "spawn_task" => {
            json!({"description":"SPAWN_MARKER","goal":"scope-limited fixture work","seed_first_intent":false})
        }
        "steer_work" => json!({"intent_id":f.live,"message":"STEER_MARKER"}),
        "kill_work" => json!({"intent_id":f.live}),
        "get_task_graph" | "list_task_findings" | "list_task_worker_traces" => {
            json!({"task_id":f.task})
        }
        "get_worker_output" | "get_worker_trace" => json!({"intent_id":f.intent}),
        "get_task_worker_trace" => json!({"task_id":f.task,"intent_id":f.intent}),
        "search_all_worker_traces" => json!({"q":"TRACE_MARKER"}),
        "search_task_worker_traces" => json!({"task_id":f.task,"q":"TRACE_MARKER"}),
        "node_detail" => json!({"id":f.fact}),
        "get_task_node_detail" => json!({"task_id":f.task,"id":f.fact}),
        "expand_digest" => json!({"id":f.digest}),
        "traffic_search" => json!({"host":"fixture.invalid"}),
        "traffic_get" => json!({"id":f.traffic}),
        "traffic_blob" => json!({"hash":f.hash}),
        "AskUserQuestion" => {
            json!({"questions":[{"question":"QUESTION_MARKER?","options":[{"label":"Yes"},{"label":"No"}]}]})
        }
        "ExitPlanMode" => json!({"plan":"PLAN_MARKER"}),
        "RecordMemory" => json!({"name":"NEW_MEMORY_MARKER","content":"NEW_MEMORY_CONTENT"}),
        "RecallMemory" => json!({"query":"MEMORY_MARKER"}),
        "TodoWrite" => json!({"todos":[{"content":"TODO_MARKER","status":"in_progress"}]}),
        "SearchExtraTools" => json!({"query":"select:fixture_custom"}),
        _ => json!({}),
    }
}
fn verify(f: &Fixture, name: &str, output: &str) {
    let s = f.router.snapshot().unwrap();
    let has = |collection: &str, key: &str, value: &str| {
        s.records(collection).iter().any(|r| r.data[key] == value)
    };
    match name {
        "graph_overview" | "get_task_graph" => {
            // Source ToolSet.graphOverviewData is a distilled planner view, not
            // the raw node/edge graph. Populated sections prove the public tool
            // dispatch preserves that projection, also after durable reopen.
            let graph: Value = serde_json::from_str(output).unwrap();
            assert_eq!(graph["task"]["description"], "TASK_MARKER");
            assert_eq!(graph["task"]["goal"], "Fixture goal");
            assert_eq!(
                graph["goals"],
                json!([{"id":f.goal,"state":"open","text":"GOAL_MARKER"}])
            );
            let node_id = |summary: &str| {
                s.records("exploration_nodes")
                    .iter()
                    .find(|r| {
                        r.data["payload"]["summary"] == summary
                            || r.data["payload"]["text"] == summary
                    })
                    .unwrap()
                    .header
                    .id
                    .clone()
            };
            assert_eq!(
                graph["hints"],
                json!([{"id":node_id("OVERVIEW_HINT_MARKER"),"state":"active","text":"OVERVIEW_HINT_MARKER"}])
            );
            let open = graph["open_intents"].as_array().unwrap();
            assert_eq!(open.len(), 1);
            assert_eq!(open[0]["id"], node_id("OVERVIEW_OPEN_MARKER"));
            assert_eq!(open[0]["summary"], "OVERVIEW_OPEN_MARKER");
            assert_eq!(open[0]["state"], "open");
            assert_eq!(graph["running_intents"][0]["id"], f.live);
            assert_eq!(graph["recent_done_intents"][0]["id"], f.intent);
            assert_eq!(graph["frontier_open"], 1);
            assert_eq!(graph["done_intents_total"], 1);
            assert_eq!(graph["findings_total"], 1);
            assert_eq!(
                graph["finding_list"],
                json!([{"id":f.finding_node,"summary":"FINDING_MARKER"}])
            );
            // The task-origin fact and the explicit fixture fact are distinct.
            assert_eq!(graph["facts"], 2);
            assert_eq!(graph["cold_digests"][0]["id"], f.digest);
            let coverage = &graph["coverage"];
            assert_eq!(coverage["denominator"], 2);
            // Source planner.SetOwnerNode(origin) anchors an explicitly inserted
            // service to the origin fact; TaskCoverageWithSources counts every
            // fact anchor, including origin. The derived root domain is only in
            // scope, so one of these two assets is tested (not zero).
            assert_eq!(coverage["tested"], 1);
            assert_eq!(coverage["pct"], 0.5);
            assert_eq!(coverage["host_count"], 1);
            assert_eq!(
                coverage["by_type"],
                json!([{"type":"root_domain","total":1,"tested":0},{"type":"service","total":1,"tested":1}])
            );
            let related = graph["related_tasks"].as_array().unwrap();
            assert_eq!(related.len(), 1);
            let source = s
                .records("tasks")
                .iter()
                .find(|r| r.data["description"] == "OVERVIEW_SOURCE_MARKER")
                .unwrap()
                .header
                .id
                .clone();
            let source_exp = &s.get("tasks", &source).unwrap().data["exploration_id"];
            assert!(
                s.records("exploration_nodes").iter().any(|r| {
                    r.data["exploration_id"] == *source_exp
                        && r.data["kind"] == "intent"
                        && r.data["state"] == "open"
                }),
                "the source must really have live work to prove frontier isolation"
            );
            assert_eq!(related[0]["source_task_id"], source);
            assert_eq!(related[0]["inherited"], true);
            assert_eq!(related[0]["task"]["description"], "OVERVIEW_SOURCE_MARKER");
            assert_eq!(related[0]["task"]["goal"], "Source fixture goal");
            assert!(
                related[0].get("open_intents").is_none(),
                "source intents must not enter the local frontier"
            );
        }
        "add_company_scope" => assert!(has("companies", "name", "NEW_COMPANY_MARKER")),
        "add_hint" | "add_task_hint" | "add_intent" | "record_fact" | "set_goals" => {
            let marker = match name {
                "add_hint" => "NEW_HINT_MARKER",
                "add_task_hint" => "NEW_TASK_HINT_MARKER",
                "add_intent" => "NEW_INTENT_MARKER",
                "record_fact" => "NEW_FACT_MARKER",
                _ => "NEW_GOAL_MARKER",
            };
            assert!(
                s.records("exploration_nodes")
                    .iter()
                    .any(|n| n.data["payload"].to_string().contains(marker)),
                "{name}"
            );
        }
        "set_constraints" => assert!(has("task_constraints", "text", "NEW_CONSTRAINT_MARKER")),
        "add_task_scope" => assert!(
            s.records("task_scope")
                .iter()
                .any(|n| n.data.to_string().contains("added.fixture.invalid"))
        ),
        "insert_assets" => assert!(
            s.records("assets")
                .iter()
                .any(|r| r.data["url"] == "https://new.fixture.invalid")
        ),
        "delete_assets_by_host" => {
            assert!(s.get("assets", &f.asset).is_none());
            assert!(s.get("companies", &f.company).is_some());
        }
        "create_custom_tool" => assert!(has("tools", "key", "new_fixture")),
        "update_custom_tool" => assert!(has("tools", "description", "UPDATED_CUSTOM_MARKER")),
        "create_mcp" => assert!(has("mcp_servers", "name", "NEW_MCP_MARKER")),
        "update_mcp" => assert_eq!(
            s.get("mcp_servers", &f.mcp).unwrap().data["name"],
            "UPDATED_MCP_MARKER"
        ),
        "report_finding" => assert!(has("findings", "summary", "NEW_FINDING_MARKER")),
        "update_finding_report" => assert_eq!(
            s.get("findings", &f.finding).unwrap().data["report"],
            artex_core::body("UPDATED_REPORT_MARKER")
        ),
        "bind_finding_traffic" => {
            assert_eq!(s.records("finding_traffic_bindings").len(), 1);
            assert_eq!(
                s.get("findings", &f.finding).unwrap().data["evidence_version"],
                "1"
            );
        }
        "record_finding_retest_result" => {
            assert!(has("finding_retests", "summary", "STAGED_RETEST_MARKER"));
            assert_eq!(
                s.get("findings", &f.finding).unwrap().data["status"],
                "pending"
            );
            assert!(has("finding_retests", "status", "running"));
        }
        "prove_goal" => {
            let goal = s.get("exploration_nodes", &f.goal).unwrap();
            assert_eq!(goal.data["state"], "met");
            assert_eq!(goal.data["payload"]["proof_node_id"], f.fact);
        }
        "goal_met" => assert_eq!(s.get("tasks", &f.task).unwrap().data["status"], "done"),
        "pause_task" => assert_eq!(s.get("tasks", &f.task).unwrap().data["status"], "paused"),
        "spawn_task" => assert!(has("tasks", "description", "SPAWN_MARKER")),
        "steer_work" => {
            assert!(has("steering_messages", "text", "STEER_MARKER"));
            assert_eq!(
                s.get("agent_runs", &f.worker).unwrap().data["state"],
                "running"
            );
        }
        "kill_work" => {
            assert_eq!(
                s.get("exploration_nodes", &f.live).unwrap().data["state"],
                "stopped"
            );
            assert_eq!(
                s.get("agent_runs", &f.worker).unwrap().data["state"],
                "cancelled"
            );
            assert_eq!(
                s.get("agent_runs", &f.run).unwrap().data["state"],
                "running"
            );
        }
        "EnterPlanMode" => assert_eq!(
            s.get("agent_runs", &f.run).unwrap().data["permission_mode"],
            "plan"
        ),
        "AskUserQuestion" | "ExitPlanMode" => {
            assert!(has("user_questions", "state", "pending"));
            assert_eq!(
                s.get("agent_runs", &f.run).unwrap().data["state"],
                "waiting"
            );
        }
        "RecordMemory" => assert!(has("memory_records", "name", "NEW_MEMORY_MARKER")),
        "TodoWrite" => assert_eq!(
            s.get("agent_runs", &f.run).unwrap().data["todos"][0]["content"],
            "TODO_MARKER"
        ),
        _ => {
            let marker = match name {
                "list_companies" => "Fixture Company",
                "list_assets" | "list_untested_assets" | "traffic_search" => "fixture.invalid",
                "list_tasks" => "TASK_MARKER",
                "list_llm_profiles" => "PROFILE_MARKER",
                "list_facts" | "node_detail" | "get_task_node_detail" => "FACT_MARKER",
                "list_goals" => "GOAL_MARKER",
                "list_findings" | "list_task_findings" | "get_finding_retest_context" => {
                    "FINDING_MARKER"
                }
                "expand_digest" => "DIGEST_MARKER",
                "list_task_worker_traces" => "TRACE_WORK_MARKER",
                "get_worker_output"
                | "get_worker_trace"
                | "get_task_worker_trace"
                | "search_all_worker_traces"
                | "search_task_worker_traces" => "TRACE_MARKER",
                "traffic_get" | "traffic_blob" => "CAPTURED_MARKER",
                "get_finding_traffic" => "TRAFFIC_BIND_MARKER",
                "RecallMemory" => "MEMORY_MARKER",
                "SearchExtraTools" => "fixture_custom",
                _ => panic!("missing business assertion for {name}"),
            };
            assert!(
                output.contains(marker),
                "{name}: expected {marker}, got {output}"
            );
        }
    }
}
// Checkpoint files preserve canonical empty collections; loading creates their
// maps only when a record exists. Compare every record unchanged, independent of
// this representation of an empty set. Business deletion assertions stay exact.
fn logical_records(snapshot: &artex_core::Snapshot) -> Value {
    let mut records = serde_json::to_value(&snapshot.collections).unwrap();
    records
        .as_object_mut()
        .unwrap()
        .retain(|_, map| !map.as_object().unwrap().is_empty());
    records
}
fn same_records(actual: &Value, expected: &Value, label: &str) {
    if actual != expected {
        let a = actual.as_object().unwrap();
        let b = expected.as_object().unwrap();
        let mut differences = Vec::new();
        for name in a
            .keys()
            .chain(b.keys())
            .collect::<std::collections::BTreeSet<_>>()
        {
            if a.get(name) != b.get(name) {
                differences.push(name);
            }
        }
        panic!("{label}: changed collections {differences:?}");
    }
}
#[test]
fn public_agent_domain_tools_preserve_business_results_blobs_and_reopen() {
    let mut names = artex_tools::list()
        .iter()
        .filter(|t| {
            t["family"] == "artex"
                && !matches!(
                    t["name"].as_str(),
                    Some("create_skill" | "update_skill_file")
                )
        })
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(names.len(), 49);
    names.extend(
        [
            "AskUserQuestion",
            "EnterPlanMode",
            "ExitPlanMode",
            "RecordMemory",
            "RecallMemory",
            "TodoWrite",
            "SearchExtraTools",
        ]
        .into_iter()
        .map(str::to_owned),
    );
    let mut base = fixture();
    base.router.store.as_mut().unwrap().checkpoint().unwrap();
    base.router.store.take();
    for name in &names {
        let mut f = isolated_copy(&base);
        if matches!(name.as_str(), "graph_overview" | "get_task_graph") {
            let mut user = f.context.clone();
            user.actor.kind = "user".into();
            domain(
                &mut f.router,
                "add_hint",
                json!({"text":"OVERVIEW_HINT_MARKER"}),
                &user,
            );
            domain(
                &mut f.router,
                "add_intent",
                json!({"summary":"OVERVIEW_OPEN_MARKER"}),
                &user,
            );
            let source = domain(&mut f.router, "task.create", json!({"description":"OVERVIEW_SOURCE_MARKER","goal":"Source fixture goal","seed_first_intent":true}), &user)["task_id"].clone();
            domain(
                &mut f.router,
                "task.sources.replace",
                json!({"id":f.task,"source_task_ids":[source]}),
                &user,
            );
        }
        if name == "get_finding_traffic" {
            let mut c = f.context.clone();
            c.actor.kind = "user".into();
            let p = input(&f, "bind_finding_traffic");
            domain(&mut f.router, "bind_finding_traffic", p, &c);
        }
        if name == "ExitPlanMode" {
            patch(
                &mut f.router,
                "agent_runs",
                &f.run,
                json!({"permission_mode":"plan"}),
            );
        }
        let req = request(&f, name, input(&f, name));
        let response = f.router.response_as(req.clone(), Some(&f.context));
        assert!(response["error"].is_null(), "{name}: {response}");
        let result = &response["result"];
        assert_eq!(result["state"], "completed", "{name}: {response}");
        assert_eq!(result["result"]["is_error"], false, "{name}: {response}");
        assert!(result["result_ref"]["sha256"].is_string(), "{name}");
        let stored = f.router.read_json_blob(&result["result_ref"]).unwrap();
        assert_eq!(stored, result["result"]);
        let output = f.router.tool_result_text(&stored).unwrap();
        verify(&f, name, &output);
        let committed = f.router.snapshot().unwrap();
        let records = logical_records(&committed);
        assert_eq!(committed.records("tool_invocations").len(), 1, "{name}");
        f.router.store.as_mut().unwrap().checkpoint().unwrap();
        f.router.store.take();
        f.router.store = Some(Store::open(&f.dir.path().join("store")).unwrap());
        same_records(
            &logical_records(&f.router.snapshot().unwrap()),
            &records,
            &format!("{name}: durable effects"),
        );
        assert_eq!(
            f.router.read_json_blob(&result["result_ref"]).unwrap(),
            stored,
            "{name}: stored body"
        );
        verify(&f, name, &output);
        let mut retry = req;
        retry["id"] = json!(new_id("request"));
        let replay = f.router.response_as(retry, Some(&f.context));
        if name == "pause_task"
            && f.router
                .snapshot()
                .unwrap()
                .get("agent_runs", &f.run)
                .unwrap()
                .data["state"]
                == "cancelled"
        {
            assert_eq!(
                replay["error"]["data"]["code"], "PERMISSION_DENIED",
                "cancelled actor must not regain authority"
            );
        } else {
            assert!(replay["error"].is_null(), "{name}: replay {replay}");
            assert_eq!(replay["result"], *result, "{name}: command replay");
        }
        same_records(
            &logical_records(&f.router.snapshot().unwrap()),
            &records,
            &format!("{name}: replay cannot repeat effects"),
        );
        eprintln!(
            "PUBLIC_TOOL_E2E_PASS {name}: schema/agent/CAS/dispatch/business/result_blob/checkpoint/reopen/idempotency"
        );
    }
}

#[test]
fn public_tool_stale_cas_and_invalid_evidence_leave_business_and_ledger_unchanged() {
    let mut f = fixture();
    for (name, input, stale, code) in [
        (
            "record_fact",
            json!({"summary":"MUST_NOT_PERSIST"}),
            true,
            "VERSION_CONFLICT",
        ),
        (
            "update_finding_report",
            json!({"finding_node_id":f.finding_node,"report":"MUST_NOT_PERSIST","evidence_version":"99"}),
            false,
            "EVIDENCE_CONFLICT",
        ),
        (
            "create_mcp",
            json!({"name":"MUST_NOT_PERSIST","transport":"stdio","command":"never-execute","env":{"API_TOKEN":"forbidden-literal"}}),
            false,
            "PERMISSION_DENIED",
        ),
    ] {
        let mut req = request(&f, name, input);
        if stale {
            req["expected"][0]["version"] = json!("0");
        }
        let before = f.router.snapshot().unwrap();
        let result = f.router.response_as(req, Some(&f.context));
        assert_eq!(result["error"]["data"]["code"], code, "{name}: {result}");
        let after = f.router.snapshot().unwrap();
        assert_eq!(before.commit_seq, after.commit_seq, "{name}");
        assert_eq!(
            serde_json::to_value(&before.collections).unwrap(),
            serde_json::to_value(&after.collections).unwrap(),
            "{name}: atomic rejection"
        );
    }
}
