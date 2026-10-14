//! Real Store acceptance fixtures. Native dispatch capture deliberately returns
//! an error; it verifies preparation and never pretends a native effect succeeded.
use super::*;
use artex_domain::{Planner, record};
use artex_runtime::{HostGateway, ToolCall};
use std::{
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

fn invoke(router: &mut Router, run: &str, context: &Context, name: &str, input: Value) -> Value {
    let batch=router.invoke_plan(&json!({"run_id":run,"tool_use_id":new_id("tooluse"),"name":name,"input":input,"raw_input":null}),context).unwrap();
    router.internal_commit(batch).unwrap()
}
fn traffic(router: &mut Router, bytes: &[u8], evidence: bool) -> Value {
    let reference = router
        .store
        .as_mut()
        .unwrap()
        .blob_put(bytes, "application/octet-stream")
        .unwrap();
    let s = router.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.traffic", &json!({}));
    if evidence {
        p.insert("traffic_evidence_snapshots","evidence",json!({"source_traffic_id":"source","resp_hash":reference["sha256"],"resp_len":reference["length"],"content_type":"application/octet-stream"})).unwrap();
    } else {
        let id=p.insert("exchanges",artex_core::collection_prefix("exchanges").unwrap(),json!({"host":"fixture.test","url":"https://fixture.test/","method":"GET","status":200,"resp_len":bytes.len().to_string()})).unwrap();
        let mut body = artex_core::default_data("exchange_bodies").unwrap();
        body["id"] = json!(id);
        body["resp_body"] = reference.clone();
        p.put(
            "exchange_bodies",
            &new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),
            body,
        );
    }
    router.internal_commit(p.done(json!({}))).unwrap();
    reference
}
fn bind_traffic(router: &mut Router) {
    let s = router.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.binding", &json!({}));
    p.insert(
        "tools",
        artex_core::collection_prefix("tools").unwrap(),
        json!({"key":"traffic_blob","system":true,"enabled":true,"agents":["mainagent"]}),
    )
    .unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
}
#[test]
fn traffic_blob_actual_tool_uses_decimal_offset_8k_pages_and_source_text_result() {
    let (_dir, mut r, run, c) = regression::seeded();
    bind_traffic(&mut r);
    let bytes = [vec![b'A'; 8192], vec![b'B'; 8192], b"tail".to_vec()].concat();
    let reference = traffic(&mut r, &bytes, false);
    let first = invoke(
        &mut r,
        &run,
        &c,
        "traffic_blob",
        json!({"hash":reference["sha256"]}),
    );
    let first = r.tool_result_text(&first["result"]).unwrap();
    assert_eq!(
        first,
        format!(
            "[offset=0 bytes=8192 total_length=16388]\n{}",
            "A".repeat(8192)
        )
    );
    let second = invoke(
        &mut r,
        &run,
        &c,
        "traffic_blob",
        json!({"hash":reference["sha256"],"offset":"8192","length":9000}),
    );
    assert_eq!(
        r.tool_result_text(&second["result"]).unwrap(),
        format!(
            "[offset=8192 bytes=8192 total_length=16388]\n{}",
            "B".repeat(8192)
        )
    );
    let last = invoke(
        &mut r,
        &run,
        &c,
        "traffic_blob",
        json!({"hash":reference["sha256"],"offset":"16384","length":0}),
    );
    assert_eq!(
        r.tool_result_text(&last["result"]).unwrap(),
        "[offset=16384 bytes=4 total_length=16388]\ntail"
    );
    let eof = invoke(
        &mut r,
        &run,
        &c,
        "traffic_blob",
        json!({"hash":reference["sha256"],"offset":"16388"}),
    );
    assert_eq!(
        r.tool_result_text(&eof["result"]).unwrap(),
        "offset 16388 exceeds total length 16388 bytes"
    );
    assert_eq!(r.snapshot().unwrap().records("tool_invocations").len(), 4);
}
#[test]
fn traffic_blob_utf8_binary_and_legacy_negative_offset_are_bounded() {
    let (_dir, mut r, _, _) = regression::seeded();
    let reference = traffic(
        &mut r,
        &[vec![b'a'; 8191], "한글🙂".as_bytes().to_vec()].concat(),
        false,
    );
    let cut = r
        .traffic_blob(&json!({"hash":reference["sha256"],"length":8192}))
        .unwrap();
    assert_eq!(cut["length"], 8192);
    assert_eq!(cut["binary"], false);
    assert_eq!(cut["content"], "a".repeat(8191));
    let korean = r
        .traffic_blob(&json!({"hash":reference["sha256"],"offset":"8191"}))
        .unwrap();
    assert_eq!(korean["content"], "한글🙂");
    let binary = traffic(&mut r, &[vec![0; 600], vec![0xff; 100]].concat(), false);
    let page = r
        .traffic_blob(&json!({"hash":binary["sha256"],"offset":-99}))
        .unwrap();
    assert_eq!(page["length"], 700);
    assert_eq!(page["binary"], true);
    assert_eq!(page["offset"], -99);
    assert_eq!(
        page["content"],
        format!(
            "Binary content; first 512 bytes (hex): {}",
            "00".repeat(512)
        )
    );
    assert!(
        r.traffic_blob_text(&json!({"hash":binary["sha256"],"offset":-99}))
            .unwrap()
            .starts_with("[offset=-99 bytes=700 total_length=700]")
    );
}
#[test]
fn traffic_blob_requires_a_recorded_hash_and_reads_evidence_reference_without_exchange() {
    let (_dir, mut r, _, _) = regression::seeded();
    let reference = traffic(&mut r, b"immutable evidence", true);
    assert_eq!(
        r.traffic_blob(&json!({"hash":reference["sha256"],"offset":"10"}))
            .unwrap()["content"],
        "evidence"
    );
    let orphan = r
        .store
        .as_mut()
        .unwrap()
        .blob_put(b"private orphan", "text/plain")
        .unwrap();
    assert_eq!(
        r.traffic_blob(&json!({"hash":orphan["sha256"]}))
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
    assert_eq!(
        r.traffic_blob(&json!({"hash":"../../invalid"}))
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        r.traffic_blob(&json!({"hash":reference["sha256"],"offset":"18446744073709551616"}))
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
    assert_eq!(
        r.traffic_blob(&json!({"hash":reference["sha256"],"offset":"18446744073709551615"}))
            .unwrap()["length"],
        0
    );
}
fn custom(router: &mut Router) -> (String, Value) {
    let s = router.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.deferred", &json!({}));
    let secret = json!({"secret_ref":new_id("secret"),"hint":"fixture key"});
    let id=p.insert("tools",artex_core::collection_prefix("tools").unwrap(),json!({"key":"fixture_deferred","system":false,"enabled":true,"deferred":true,"agents":["mainagent"],"description":"fixture deferred command","kind":"command","schema":{"type":"object","properties":{"term":{"type":"string"}},"required":["term"],"additionalProperties":false},"exec":{"command":"fixture-not-executed {term}","env":{"API_KEY":secret,"LANG":"ko_KR.UTF-8"}}})).unwrap();
    p.insert(
        "settings",
        "setting",
        json!({"key":"python_interpreter","value":"python-fixture"}),
    )
    .unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    (id, secret)
}
fn discover(router: &mut Router, run: &str, c: &Context, name: &str) -> Value {
    invoke(
        router,
        run,
        c,
        "SearchExtraTools",
        json!({"query":format!("select:{name}")}),
    )
}
#[test]
fn execute_extra_requires_real_completed_same_run_discovery_and_preserves_secretref() {
    let (_dir, mut r, run, c) = regression::seeded();
    let (id, secret) = custom(&mut r);
    let input = json!({"tool_name":"fixture_deferred","params":{"term":"safe argument"}});
    assert_eq!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &input)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    let receipt = discover(&mut r, &run, &c, "fixture_deferred");
    assert_eq!(receipt["state"], "completed");
    let snapshot = r.snapshot().unwrap();
    let mut plan = Planner::new(&snapshot, "fixture.another_run", &json!({}));
    let another = plan
        .insert(
            "agent_runs",
            "run",
            record(&snapshot, "agent_runs", &run).unwrap().data.clone(),
        )
        .unwrap();
    r.internal_commit(plan.done(json!({}))).unwrap();
    assert_eq!(
        r.trusted_tool_input(&another, "ExecuteExtraTool", &input)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED",
        "Discovery of the same agent's other run grants no authority"
    );
    let resolved = r
        .trusted_tool_input(&run, "ExecuteExtraTool", &input)
        .unwrap();
    assert_eq!(resolved["params"], input["params"]);
    assert_eq!(resolved["resolved_tool"]["exec"]["env"]["API_KEY"], secret);
    assert_eq!(resolved["python_interpreter"], "python-fixture");
    let mut forged = input.clone();
    forged["resolved_tool"] = json!({"kind":"command","exec":{"command":"forged-not-authorized"}});
    assert_eq!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &forged)
            .unwrap()["resolved_tool"],
        resolved["resolved_tool"],
        "Execution configuration must come from the current trusted Store definition"
    );
    let batch=r.invoke_plan(&json!({"run_id":run,"tool_use_id":"extra-prepared","name":"ExecuteExtraTool","input":input,"raw_input":null}),&c).unwrap();
    let result = r.internal_commit(batch).unwrap();
    assert_eq!(result["state"], "prepared");
    assert!(result["result_ref"].is_null());
    let s = r.snapshot().unwrap();
    assert_eq!(record(&s, "tools", &id).unwrap().header.version, "1");
    let ledger = record(
        &s,
        "tool_invocations",
        result["invocation_id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(ledger.data["state"], "prepared");
    assert!(ledger.data["result_ref"].is_null());
    let mut p = Planner::new(&s, "fixture.discovery_not_confirmed", &json!({}));
    let invocation = record(
        &s,
        "tool_invocations",
        receipt["invocation_id"].as_str().unwrap(),
    )
    .unwrap();
    let mut data = invocation.data.clone();
    data["state"] = json!("failed");
    p.put("tool_invocations", &invocation.header.id, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    assert_eq!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &input)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}
#[test]
fn execute_extra_rechecks_current_schema_version_binding_and_enabled_after_discovery() {
    let (_dir, mut r, run, c) = regression::seeded();
    let (id, _) = custom(&mut r);
    discover(&mut r, &run, &c, "fixture_deferred");
    let old = json!({"tool_name":"fixture_deferred","params":{"term":"old value"}});
    assert!(r.trusted_tool_input(&run, "ExecuteExtraTool", &old).is_ok());
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.schema_revision", &json!({}));
    let mut data = record(&s, "tools", &id).unwrap().data.clone();
    data["schema"] = json!({"type":"object","properties":{"count":{"type":"integer"}},"required":["count"],"additionalProperties":false});
    p.put("tools", &id, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    assert_eq!(
        r.snapshot()
            .unwrap()
            .get("tools", &id)
            .unwrap()
            .header
            .version,
        "2"
    );
    assert_eq!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &old)
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
    let current = json!({"tool_name":"fixture_deferred","params":{"count":2}});
    assert!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &current)
            .is_ok()
    );
    for patch in [
        json!({"enabled":false}),
        json!({"enabled":true,"agents":["worker"]}),
        json!({"agents":["mainagent"],"deferred":false}),
    ] {
        let s = r.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.revoke", &json!({}));
        let mut data = record(&s, "tools", &id).unwrap().data.clone();
        for (k, v) in patch.as_object().unwrap() {
            data[k] = v.clone();
        }
        p.put("tools", &id, data);
        r.internal_commit(p.done(json!({}))).unwrap();
        assert_eq!(
            r.trusted_tool_input(&run, "ExecuteExtraTool", &current)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
    }
}
#[test]
fn execute_extra_mcp_rechecks_live_visibility_cache_schema_and_secretref() {
    let (_dir, mut r, run, c) = regression::seeded();
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.mcp", &json!({}));
    let secret = json!({"secret_ref":new_id("secret"),"hint":"fixture MCP token"});
    let sid=p.insert("mcp_servers","mcp",json!({"name":"fixture","transport":"stdio","command":"fixture-not-executed","env":{"ACCESS_TOKEN":secret},"enabled":true})).unwrap();
    let cache=p.insert("mcp_tools_cache","mcptool",json!({"server_id":sid,"tool_name":"lookup","schema":{"type":"object","properties":{"q":{"type":"string"}},"required":["q"],"additionalProperties":false}})).unwrap();
    let visibility=p.insert("agent_visibility","visibility",json!({"agent_id":c.actor.agent_id,"resource_kind":"mcp","resource_id":sid,"enabled":true})).unwrap();
    r.internal_commit(p.done(json!({}))).unwrap();
    discover(&mut r, &run, &c, "mcp__fixture__lookup");
    let input = json!({"tool_name":"mcp__fixture__lookup","params":{"q":"fixture"}});
    let resolved = r
        .trusted_tool_input(&run, "ExecuteExtraTool", &input)
        .unwrap();
    assert_eq!(resolved["resolved_tool"]["kind"], "mcp");
    assert_eq!(resolved["resolved_tool"]["name"], "lookup");
    assert_eq!(
        resolved["resolved_tool"]["server"]["env"]["ACCESS_TOKEN"],
        secret
    );
    assert_eq!(
        r.trusted_tool_input(
            &run,
            "ExecuteExtraTool",
            &json!({"tool_name":"mcp__fixture__lookup","params":{"unexpected":1}})
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.visibility_revoked", &json!({}));
    let mut v = record(&s, "agent_visibility", &visibility)
        .unwrap()
        .data
        .clone();
    v["enabled"] = json!(false);
    p.put("agent_visibility", &visibility, v);
    r.internal_commit(p.done(json!({}))).unwrap();
    assert_eq!(
        r.trusted_tool_input(&run, "ExecuteExtraTool", &input)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
    assert!(
        r.snapshot()
            .unwrap()
            .get("mcp_tools_cache", &cache)
            .is_some()
    );
}
struct CaptureHost {
    tx: Mutex<mpsc::Sender<(Context, Value)>>,
    secret_gate: Mutex<mpsc::Receiver<()>>,
}
impl HostGateway for CaptureHost {
    fn request(
        &self,
        _: &str,
        c: &Context,
        operation: &str,
        input: Value,
        _: Duration,
    ) -> Result<Value> {
        match operation {
            "secret.resolve" => {
                let _ = self
                    .secret_gate
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
                Err(Error::new(
                    "CANCELLED",
                    "Fixture does not generate a model response",
                ))
            }
            "tool.execute" => {
                self.tx.lock().unwrap().send((c.clone(), input)).unwrap();
                Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Payload capture only; no native execution claimed",
                ))
            }
            "host.events" => Ok(json!({"events":[]})),
            "run.stop" => Ok(json!({"accepted":true})),
            _ => Err(Error::new("FEATURE_DISABLED", "Unused fixture boundary")),
        }
    }
}
#[test]
fn execute_extra_actual_controller_dispatch_carries_current_config_and_prepared_identity() {
    let (_dir, mut r, run, c) = regression::seeded();
    let (_, secret) = custom(&mut r);
    discover(&mut r, &run, &c, "fixture_deferred");
    let s = r.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.runtime", &json!({}));
    let pid=p.insert("llm_profiles","profile",json!({"name":"fixture","format":"openai","base_url":"http://127.0.0.1:1/v1","model":"fixture","api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"is_default":true})).unwrap();
    let aid = c.actor.agent_id.as_deref().unwrap();
    let mut agent = record(&s, "agents", aid).unwrap().data.clone();
    agent["llm_profile_id"] = json!(pid);
    p.put("agents", aid, agent);
    let mut data = record(&s, "agent_runs", &run).unwrap().data.clone();
    data["state"] = json!("queued");
    data["permission_mode"] = json!("bypassPermissions");
    p.put("agent_runs", &run, data);
    r.internal_commit(p.done(json!({}))).unwrap();
    let (tx, rx) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    r.attach_host(Arc::new(CaptureHost {
        tx: Mutex::new(tx),
        secret_gate: Mutex::new(gate),
    }));
    r.runtime_tick().unwrap();
    let call = ToolCall {
        id: "native-dispatch-fixture".into(),
        name: "ExecuteExtraTool".into(),
        input: json!({"tool_name":"fixture_deferred","params":{"term":"literal fixture"}}),
    };
    let state = r
        .runtime
        .runs
        .get_mut(&run)
        .expect("Queued admission creates real runtime state");
    state.in_flight = false;
    state.pending.push_back(call);
    r.runtime_tick().unwrap();
    let (context, payload) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(release);
    assert_eq!(context.actor.run_id.as_deref(), Some(run.as_str()));
    assert_eq!(payload["name"], "ExecuteExtraTool");
    assert_eq!(payload["input"]["tool_name"], "fixture_deferred");
    assert_eq!(
        payload["input"]["params"],
        json!({"term":"literal fixture"})
    );
    assert_eq!(
        payload["input"]["resolved_tool"]["exec"]["env"]["API_KEY"],
        secret
    );
    assert_eq!(payload["input"]["directory_capability"], "work");
    assert_eq!(payload["input"]["attachment_paths"], json!([]));
    assert_eq!(payload["input"]["python_interpreter"], "python-fixture");
    let s = r.snapshot().unwrap();
    let invocation = record(
        &s,
        "tool_invocations",
        payload["invocation_id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(invocation.data["state"], "prepared");
    assert_eq!(invocation.data["execution_id"], payload["execution_id"]);
    assert_eq!(
        payload["command_id"],
        format!(
            "command_{}",
            invocation.header.id.strip_prefix("invocation_").unwrap()
        )
    );
    assert!(invocation.data["result_ref"].is_null());
}
