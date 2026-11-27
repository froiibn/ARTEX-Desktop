//! Exercise the actual writer/model boundaries for the stable-release parity fixes.
use super::*;
use artex_runtime::HostGateway;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};

fn state(turns: u64) -> RunState {
    RunState {
        cancellation: Cancellation::default(),
        in_flight: false,
        turns,
        pending: VecDeque::new(),
        wrapup: false,
        profile_override: None,
        failed_profiles: Default::default(),
        idle_nudges: 0,
        intent_failures: 0,
        output_recoveries: 0,
        retry_at: None,
    }
}

#[test]
fn deadline_request_view_pairs_host_calls_without_fabricating_external_completion() {
    for resolved_before_deadline in [false, true] {
        let (dir, mut router, run, context) = crate::regression::seeded();
        let task = artex_domain::tasks::plan(
            "task.create",
            &json!({"description":"Fixture work","goal":"Preserve confirmed evidence"}),
            &router.snapshot().unwrap(),
            &context,
        )
        .unwrap();
        let task_id = task.result["task_id"].as_str().unwrap().to_owned();
        let exploration = task.result["exploration_id"].as_str().unwrap().to_owned();
        router.internal_commit(task).unwrap();
        let snapshot = router.snapshot().unwrap();
        let mut p = Planner::new(&snapshot, "test.deadline.pending", &json!({}));
        let agent = p
            .insert(
                "agents",
                "agent",
                json!({"key":"worker","enabled":true,"wrapup_max_turns":2}),
            )
            .unwrap();
        let intent = p.insert("exploration_nodes", "node", json!({"exploration_id":exploration,"kind":"intent","state":"running","payload":{"summary":"Existing observation"}})).unwrap();
        p.insert("tools", artex_core::collection_prefix("tools").unwrap(), json!({"key":"fixture_settle_external","system":false,"enabled":true,"deferred":false,"agents":["worker"],"description":"Fixture external command; never executed","kind":"command","schema":{"type":"object","properties":{},"additionalProperties":false},"exec":{"command":"fixture-not-executed"}})).unwrap();
        let mut data = snapshot.get("agent_runs", &run).unwrap().data.clone();
        data["agent_id"] = json!(agent);
        data["conversation_id"] = Value::Null;
        data["task_id"] = json!(task_id);
        data["intent_id"] = json!(intent);
        data["permission_mode"] = json!("bypassPermissions");
        p.put("agent_runs", &run, data);
        let invocation = p.insert("tool_invocations", "invocation", json!({"run_id":run,"tool_use_id":"pending-host-call","name":"Bash","state":"running","command_id":new_id("command")})).unwrap();
        router.internal_commit(p.done(json!({}))).unwrap();
        router.runtime.runs.insert(run.clone(), state(1));
        router.append_transcript(&run, "tool_use", "Pending host call", &Message {role:"assistant".into(),content:vec![json!({"type":"text","text":"Existing observation"}),json!({"type":"tool_use","id":"pending-host-call","name":"Bash","input":{"command":"fixture-not-executed"}})]}, None).unwrap();
        let snapshot = router.snapshot().unwrap();
        let normal = router
            .prompt_for_run(&snapshot, snapshot.get("agent_runs", &run).unwrap())
            .unwrap();
        assert!(
            normal
                .tools
                .iter()
                .any(|tool| tool["name"] == "fixture_settle_external")
        );
        if resolved_before_deadline {
            router
                .complete_host(
                    &run,
                    &invocation,
                    Ok(json!({"state":"completed","exit_code":0,"output":"Actual fixture result"})),
                )
                .unwrap();
        }
        router.begin_timeout_wrapup(&run, false).unwrap();
        // Reopen persisted history: API repair must not turn an unconfirmed
        // host effect into a completed invocation or erase the durable call.
        router.store.take().unwrap().checkpoint().unwrap();
        router.store = Some(artex_store::Store::open(&dir.path().join("store")).unwrap());
        let snapshot = router.snapshot().unwrap();
        let child = snapshot
            .records("agent_runs")
            .into_iter()
            .find(|r| r.data["parent_run_id"] == run)
            .unwrap();
        let child_id = child.header.id.clone();
        let prompt = router.prompt_for_run(&snapshot, child).unwrap();
        assert!(
            !prompt
                .tools
                .iter()
                .any(|tool| tool["name"] == "fixture_settle_external")
        );
        assert_eq!(
            router
                .approval(
                    &child_id,
                    &ToolCall {
                        id: "fixture-denied".into(),
                        name: "fixture_settle_external".into(),
                        input: json!({})
                    }
                )
                .unwrap(),
            Decision::Deny
        );
        assert_eq!(
            snapshot.get("agent_runs", &run).unwrap().data["state"],
            "failed"
        );
        assert_eq!(
            snapshot.get("tool_invocations", &invocation).unwrap().data["state"],
            if resolved_before_deadline {
                "completed"
            } else {
                "running"
            }
        );
        assert!(
            serde_json::to_string(&prompt.messages)
                .unwrap()
                .contains("pending-host-call")
        );
        for format in ["anthropic", "openai", "openai-responses"] {
            let encoded = artex_runtime::provider::encode(&json!({"format":format,"model":"fixture","thinking_type":"disabled","max_tokens":1024}), &prompt).unwrap().1;
            let wire = encoded.to_string();
            assert_eq!(
                wire.contains("pending-host-call"),
                resolved_before_deadline,
                "{format}: {wire}"
            );
            assert_eq!(
                wire.contains("Actual fixture result"),
                resolved_before_deadline
            );
            assert!(wire.contains("Existing observation"));
        }
        if !resolved_before_deadline {
            // A late, actual host completion is still recorded once the parent
            // has ended; request normalization must not preempt that evidence.
            router
                .complete_host(
                    &run,
                    &invocation,
                    Ok(json!({"state":"completed","exit_code":0,"output":"Actual fixture result"})),
                )
                .unwrap();
            let snapshot = router.snapshot().unwrap();
            assert_eq!(
                snapshot.get("tool_invocations", &invocation).unwrap().data["state"],
                "completed"
            );
            let actual = router
                .read_blob_bytes(
                    &snapshot.get("tool_invocations", &invocation).unwrap().data["result_ref"],
                    1024 * 1024,
                )
                .unwrap();
            assert!(
                String::from_utf8(actual)
                    .unwrap()
                    .contains("Actual fixture result")
            );
            assert_eq!(
                snapshot.get("agent_runs", &run).unwrap().data["state"],
                "failed"
            );
            let prompt = router
                .prompt_for_run(&snapshot, snapshot.get("agent_runs", &child_id).unwrap())
                .unwrap();
            let wire = artex_runtime::provider::encode(&json!({"format":"anthropic","model":"fixture","thinking_type":"disabled","max_tokens":1024}), &prompt).unwrap().1;
            // Existing completion policy keeps a terminal parent's transcript
            // frozen. Its real result remains in the invocation Blob, while an
            // unpaired call still cannot leak into a child's API request.
            assert!(!wire.to_string().contains("pending-host-call"));
            assert_eq!(
                prompt
                    .messages
                    .iter()
                    .flat_map(|message| &message.content)
                    .filter(|block| block["type"] == "tool_result"
                        && block["tool_use_id"] == "pending-host-call")
                    .count(),
                0
            );
        }
    }
}

#[test]
fn retest_stages_a_verdict_and_seals_only_the_owning_terminal_outcome() {
    for (terminal, verdict, expected) in [
        ("completed", Some("fixed"), "completed"),
        ("failed", Some("fixed"), "failed"),
        ("cancelled", Some("fixed"), "stopped"),
        ("completed", None, "failed"),
    ] {
        let (_dir, mut router, run, context) = crate::regression::seeded();
        let snapshot = router.snapshot().unwrap();
        let mut p = Planner::new(&snapshot, "test.retest.seed", &json!({}));
        let finding = p
            .insert(
                "findings",
                "finding",
                json!({"name":"fixture","vulnclass":"IDOR","severity":"high","status":"pending"}),
            )
            .unwrap();
        let retest = p.insert("finding_retests", "retest", json!({"finding_id":finding,"conversation_id":context.conversation_id,"status":"running"})).unwrap();
        router.internal_commit(p.done(json!({}))).unwrap();
        if let Some(verdict) = verdict {
            let input = json!({"verdict":verdict,"summary":" verified ","evidence":" original request now denied "});
            let stage = artex_domain::findings::plan(
                "record_finding_retest_result",
                &input,
                &router.snapshot().unwrap(),
                &context,
            )
            .unwrap();
            router.internal_commit(stage).unwrap();
            // Identical tool retries remain valid, but a conflicting conclusion
            // cannot replace the evidence while the retest is still running.
            let snapshot = router.snapshot().unwrap();
            assert!(
                artex_domain::findings::plan(
                    "record_finding_retest_result",
                    &input,
                    &snapshot,
                    &context
                )
                .is_ok()
            );
            let changed =
                json!({"verdict":"reproduced","summary":"different","evidence":"different"});
            assert_eq!(
                artex_domain::findings::plan(
                    "record_finding_retest_result",
                    &changed,
                    &snapshot,
                    &context
                )
                .unwrap_err()
                .code,
                "VERSION_CONFLICT"
            );
            assert_eq!(
                snapshot.get("finding_retests", &retest).unwrap().data["status"],
                "running"
            );
            assert_eq!(
                snapshot.get("findings", &finding).unwrap().data["status"],
                "pending"
            );
        }
        router
            .finish_runtime(&run, terminal, "Fixture terminal outcome")
            .unwrap();
        let snapshot = router.snapshot().unwrap();
        let rt = snapshot.get("finding_retests", &retest).unwrap();
        assert_eq!(rt.data["status"], expected);
        assert_eq!(
            snapshot.get("findings", &finding).unwrap().data["status"],
            if terminal == "completed" && verdict == Some("fixed") {
                "fixed"
            } else {
                "pending"
            }
        );
        assert_eq!(
            rt.header.commit_seq,
            snapshot.get("agent_runs", &run).unwrap().header.commit_seq
        );
        let seq = snapshot.commit_seq;
        router
            .finish_runtime(&run, "completed", "stale completion replay")
            .unwrap();
        assert_eq!(router.snapshot().unwrap().commit_seq, seq);
    }
}

#[test]
fn staged_retest_blob_retries_keep_the_source_limits_and_evidence_identity() {
    let (dir, mut router, _, context) = crate::regression::seeded();
    let snapshot = router.snapshot().unwrap();
    let mut p = Planner::new(&snapshot, "test.retest.blob", &json!({}));
    let finding = p
        .insert(
            "findings",
            "finding",
            json!({"name":"fixture","vulnclass":"IDOR","severity":"high"}),
        )
        .unwrap();
    let retest = p.insert("finding_retests", "retest", json!({"finding_id":finding,"conversation_id":context.conversation_id,"status":"running"})).unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    let input = json!({"verdict":"inconclusive","summary":"Captured evidence","evidence":"証".repeat(22_000)});
    let stage = artex_domain::findings::plan(
        "record_finding_retest_result",
        &input,
        &router.snapshot().unwrap(),
        &context,
    )
    .unwrap();
    router.internal_commit(stage).unwrap();
    router.store.take().unwrap().checkpoint().unwrap();
    router.store = Some(artex_store::Store::open(&dir.path().join("store")).unwrap());
    let snapshot = router.snapshot().unwrap();
    assert!(snapshot.get("finding_retests", &retest).unwrap().data["evidence"]["blob"].is_object());
    assert!(
        artex_domain::findings::plan("record_finding_retest_result", &input, &snapshot, &context)
            .is_ok()
    );
    for (field, bytes) in [("summary", 16_001), ("evidence", 128_001)] {
        let mut oversized = input.clone();
        oversized[field] = json!("x".repeat(bytes));
        assert_eq!(
            artex_domain::findings::plan(
                "record_finding_retest_result",
                &oversized,
                &snapshot,
                &context
            )
            .unwrap_err()
            .code,
            "SIZE_LIMIT"
        );
    }
    for field in ["summary", "evidence"] {
        let mut blank = input.clone();
        blank[field] = json!(" \n\t ");
        assert_eq!(
            artex_domain::findings::plan(
                "record_finding_retest_result",
                &blank,
                &snapshot,
                &context
            )
            .unwrap_err()
            .code,
            "INVALID_PARAMS"
        );
    }
}

#[test]
fn mcp_error_flag_reaches_the_ledger_and_model_tool_result() {
    for is_error in [false, true] {
        let (_dir, mut router, run, _) = crate::regression::seeded();
        let snapshot = router.snapshot().unwrap();
        let mut p = Planner::new(&snapshot, "test.mcp.result", &json!({}));
        let invocation = p.insert("tool_invocations", "invocation", json!({"run_id":run,"tool_use_id":"fixture-mcp","name":"ExecuteExtraTool","state":"running","command_id":new_id("command")})).unwrap();
        router.internal_commit(p.done(json!({}))).unwrap();
        router.runtime.runs.insert(run.clone(), state(0));
        router
            .complete_host(
                &run,
                &invocation,
                Ok(json!({"content":[{"type":"text","text":"fixture result"}],"isError":is_error})),
            )
            .unwrap();
        let snapshot = router.snapshot().unwrap();
        let invocation = snapshot.get("tool_invocations", &invocation).unwrap();
        assert_eq!(
            invocation.data["state"],
            if is_error { "failed" } else { "completed" }
        );
        assert_eq!(
            router
                .read_json_blob(&invocation.data["result_ref"])
                .unwrap()["is_error"],
            is_error
        );
        let transcript = router.context_messages(&run).unwrap();
        assert!(
            transcript
                .iter()
                .flat_map(|m| &m.content)
                .any(|b| b["type"] == "tool_result" && b["is_error"] == is_error)
        );
    }
}

struct SecretOnly;
impl HostGateway for SecretOnly {
    fn request(&self, _: &str, _: &Context, op: &str, _: Value, _: Duration) -> Result<Value> {
        match op {
            "secret.resolve" => Ok(json!({"value":"fixture"})),
            "run.stop" => Ok(json!({"accepted":true})),
            _ => panic!("Unexpected fixture operation {op}"),
        }
    }
}

#[test]
fn unlimited_turns_make_a_real_model_request_past_one_hundred_while_finite_budget_wraps() {
    for (max, turns, expect_wrapup) in [(0, 101, false), (2, 2, true)] {
        let (_dir, mut router, run, _) = crate::regression::seeded();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(20);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < until,
                            "No model request reached the local fixture"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            // Windows can inherit the listener's nonblocking mode. This fixture
            // uses blocking read_exact with a finite socket timeout; otherwise
            // a split/late first header byte races accept and returns WouldBlock.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                stream.read_exact(&mut b).unwrap();
                header.push(b[0]);
            }
            let len: usize = String::from_utf8(header)
                .unwrap()
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; len];
            stream.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            let response=serde_json::to_vec(&json!({"choices":[{"message":{"content":"Fixture complete"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len()).unwrap();
            stream.write_all(&response).unwrap();
            request
        });
        let snapshot = router.snapshot().unwrap();
        let mut p = Planner::new(&snapshot, "test.turn.budget", &json!({}));
        let profile=p.insert("llm_profiles","profile",json!({"name":"fixture","model":"fixture","format":"openai","base_url":format!("http://{address}/v1"),"is_default":true,"non_streaming":true,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}})).unwrap();
        let agent_id = text(&snapshot.get("agent_runs", &run).unwrap().data, "agent_id");
        let mut agent = snapshot.get("agents", agent_id).unwrap().data.clone();
        agent["llm_profile_id"] = json!(profile);
        agent["max_turns"] = json!(max);
        agent["wrapup_max_turns"] = json!(3);
        agent["wrapup_prompt"] = body("FINITE_BUDGET_WRAPUP");
        p.put("agents", agent_id, agent);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.attach_host(Arc::new(SecretOnly));
        router.runtime.runs.insert(run.clone(), state(turns));
        router.start_model(&run).unwrap();
        let completion = router
            .runtime
            .rx
            .recv_timeout(Duration::from_secs(20))
            .unwrap();
        if let Completion::Model {
            run: completed,
            metadata,
            result,
        } = completion
        {
            assert_eq!(completed, run);
            router.complete_model(&run, &metadata, result).unwrap();
        } else {
            panic!("Expected a model completion");
        }
        let request = server.join().unwrap();
        assert_eq!(
            request["messages"]
                .to_string()
                .contains("FINITE_BUDGET_WRAPUP"),
            expect_wrapup
        );
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "completed"
        );
    }
}

#[test]
fn timeout_settlement_records_confirmed_facts_and_keeps_the_intent_exhausted() {
    for (mode, task_timeout) in [
        ("bypassPermissions", false),
        ("plan", false),
        ("bypassPermissions", true),
        ("plan", true),
    ] {
        let (dir, mut router, run, context) = crate::regression::seeded();
        let task = artex_domain::tasks::plan(
            "task.create",
            &json!({"description":"Fixture work","goal":"Verify existing evidence"}),
            &router.snapshot().unwrap(),
            &context,
        )
        .unwrap();
        let task_id = task.result["task_id"].as_str().unwrap().to_owned();
        let exp = task.result["exploration_id"].as_str().unwrap().to_owned();
        router.internal_commit(task).unwrap();
        let snapshot = router.snapshot().unwrap();
        let mut p = Planner::new(&snapshot, "test.deadline", &json!({}));
        let agent = p
            .insert(
                "agents",
                "agent",
                json!({"key":"worker","enabled":true,"wrapup_max_turns":2,"task_timeout_wrapup_max_turns":2}),
            )
            .unwrap();
        let intent=p.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"intent","state":"running","payload":{"summary":"Previously verified work"}})).unwrap();
        let mut data = snapshot.get("agent_runs", &run).unwrap().data.clone();
        data["agent_id"] = json!(agent);
        data["conversation_id"] = Value::Null;
        data["task_id"] = json!(task_id);
        data["intent_id"] = json!(intent);
        data["permission_mode"] = json!(mode);
        p.put("agent_runs", &run, data);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.runtime.runs.insert(run.clone(), state(1));
        router.begin_timeout_wrapup(&run, task_timeout).unwrap();
        // Optional purpose/parent fields must survive a real checkpoint reopen;
        // losing them would silently admit normal tools in the settlement run.
        router.store.take().unwrap().checkpoint().unwrap();
        router.store = Some(artex_store::Store::open(&dir.path().join("store")).unwrap());
        let snapshot = router.snapshot().unwrap();
        let child = snapshot
            .records("agent_runs")
            .into_iter()
            .find(|r| r.data["parent_run_id"] == run)
            .unwrap_or_else(|| {
                panic!(
                    "Missing settlement child: {:?}",
                    snapshot.records("agent_runs")
                )
            });
        let child_id = child.header.id.clone();
        assert_eq!(
            child.data["purpose"],
            if task_timeout {
                "task_timeout_wrapup"
            } else {
                "run_wrapup"
            }
        );
        assert_eq!(child.data["intent_id"], intent);
        assert_eq!(child.data["permission_mode"], mode);
        assert_eq!(
            snapshot.get("exploration_nodes", &intent).unwrap().data["state"],
            "exhausted"
        );
        let prompt = router.prompt_for_run(&snapshot, child).unwrap();
        assert!(
            !prompt
                .tools
                .iter()
                .any(|t| matches!(t["name"].as_str(), Some("Bash" | "WebFetch" | "add_intent")))
        );
        let context =
            artex_agent::context_for_run(&snapshot, &child_id, &router.engine_epoch).unwrap();
        let input = json!({"summary":"Confirmed before deadline","detail":"Existing local evidence","confidence":"observed"});
        let call = ToolCall {
            id: "settled-fact".into(),
            name: "record_fact".into(),
            input: input.clone(),
        };
        if mode == "plan" {
            assert_eq!(router.approval(&child_id, &call).unwrap(), Decision::Deny);
            assert_eq!(
                artex_tools::check_access("record_fact", &input, &snapshot, &context)
                    .unwrap_err()
                    .code,
                "PERMISSION_DENIED"
            );
        } else {
            assert_eq!(router.approval(&child_id, &call).unwrap(), Decision::Allow);
            let dispatch =
                artex_tools::dispatch("record_fact", &input, &snapshot, &context).unwrap();
            if let artex_tools::ToolDispatch::Command(batch) = dispatch {
                router.internal_commit(batch).unwrap();
            } else {
                panic!("Graph fact must be a durable command");
            }
            assert!(
                router
                    .snapshot()
                    .unwrap()
                    .records("exploration_nodes")
                    .iter()
                    .any(|n| n.data["kind"] == "fact"
                        && n.data["payload"]["summary"] == "Confirmed before deadline")
            );
        }
        for name in ["Bash", "ExecuteExtraTool", "WebFetch", "add_intent"] {
            let definition = artex_tools::definition(name).unwrap();
            assert!(!artex_tools::wrapup_tool_allowed(name, definition));
        }
        router
            .finish_runtime(&child_id, "completed", "Settlement finished")
            .unwrap();
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("exploration_nodes", &intent)
                .unwrap()
                .data["state"],
            "exhausted"
        );
    }
}
