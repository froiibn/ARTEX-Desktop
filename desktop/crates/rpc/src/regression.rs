use super::*;
use artex_domain::Planner;
use artex_runtime::{HostGateway, Message};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
pub(super) fn seeded() -> (tempfile::TempDir, Router, String, Context) {
    let dir = tempfile::tempdir().unwrap();
    let mut router = Router::new(BTreeMap::from([("work".into(), dir.path().join("work"))]));
    std::fs::create_dir(dir.path().join("work")).unwrap();
    router
        .set_execution_capabilities(Some("work".into()), None)
        .unwrap();
    router.host_epoch = Some(new_id("epoch"));
    router.store = Some(Store::create(&dir.path().join("store"), "Regression").unwrap());
    let snapshot = router.snapshot().unwrap();
    let mut p = Planner::new(&snapshot, "test.seed", &json!({}));
    let agent = p
        .insert("agents", "agent", json!({"key":"mainagent","enabled":true}))
        .unwrap();
    let conversation = p
        .insert(
            "conversations",
            "conversation",
            json!({"agent_key":"mainagent","title":"Regression"}),
        )
        .unwrap();
    let run=p.insert("agent_runs","run",json!({"agent_id":agent,"conversation_id":conversation,"engine_epoch":router.engine_epoch,"state":"running"})).unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    let context =
        artex_agent::context_for_run(&router.snapshot().unwrap(), &run, &router.engine_epoch)
            .unwrap();
    (dir, router, run, context)
}
fn wire(router: &Router, method: &str, params: Value, command: Option<String>) -> Value {
    json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":method,"workspace_id":router.snapshot().unwrap().workspace_id,"engine_epoch":router.engine_epoch,"command_id":command,"expected":[],"timeout_ms":5000,"params":params})
}
struct SlowHost {
    release: AtomicBool,
    calls: AtomicUsize,
}
impl HostGateway for SlowHost {
    fn request(
        &self,
        _: &str,
        _: &Context,
        operation: &str,
        _: Value,
        _: Duration,
    ) -> Result<Value> {
        if operation == "ui.stop" {
            return Ok(json!({"accepted":true}));
        }
        if operation == "traffic.configure" {
            return Ok(json!({"enabled":false}));
        }
        if operation == "traffic.poll" {
            return Ok(json!({"exchanges":[]}));
        }
        if operation == "traffic.ack" {
            return Ok(json!({"acknowledged":true}));
        }
        if operation == "logs.poll" {
            return Ok(json!({"items":[],"dropped":"0","capture_error":null}));
        }
        if operation == "logs.ack" {
            return Ok(json!({"acknowledged":0}));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(3);
        while !self.release.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!({"ok":true,"path":"made"}))
    }
}
#[test]
fn settings_value_dto_carries_generation_bound_owner_versions() {
    let (_dir, mut router, _, _) = seeded();
    let snapshot = router.snapshot().unwrap();
    let mut planner = Planner::new(&snapshot, "test.settings", &json!({}));
    let setting = planner
        .insert(
            "settings",
            "setting",
            json!({"key":"traffic_capture","value":"false"}),
        )
        .unwrap();
    let retry = planner
        .insert(
            "settings",
            "setting",
            json!({"key":"llm_retry_policy","value":"{}"}),
        )
        .unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    let request = wire(&router, "ui.settings", json!({}), None);
    let response = router.response(request);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["traffic_capture"], false);
    let expected = response["read_versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == setting)
        .unwrap();
    assert_eq!(expected["collection"], "settings");
    assert_eq!(expected["version"], "1");
    assert_eq!(
        response["read_seq"],
        router.snapshot().unwrap().commit_seq.to_string()
    );
    let request = wire(&router, "ui.llmRetryPolicy", json!({}), None);
    let retry_response = router.response(request);
    assert!(retry_response.get("error").is_none(), "{retry_response}");
    assert!(
        retry_response["read_versions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == retry)
    );
}
#[test]
fn native_operation_does_not_block_ping_and_cancel_preserves_confirmed_receipt() {
    let (_dir, mut router, _run, _context) = seeded();
    let host = Arc::new(SlowHost {
        release: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
    });
    router.attach_host(host.clone());
    let command = new_id("command");
    let request = wire(
        &router,
        "ui.workspaceMkdir",
        json!({"path":"made"}),
        Some(command.clone()),
    );
    assert!(router.response(request.clone()).is_null());
    // The native worker remains blocked on its gate while the writer serves ping.
    // Disk fsync contention in the parallel suite is not an RPC-blocking oracle.
    assert!(!host.release.load(Ordering::SeqCst));
    let ping = wire(&router, "system.ping", json!({}), None);
    assert!(router.response(ping).get("result").is_some());
    let cancel = wire(
        &router,
        "rpc.cancel",
        json!({"request_id":request["id"]}),
        None,
    );
    assert_eq!(router.response(cancel)["result"]["accepted"], true);
    assert_eq!(
        router.drain_responses()[0]["error"]["data"]["code"],
        "RECOVERY_REQUIRED"
    );
    host.release.store(true, Ordering::SeqCst);
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        router.maintain().unwrap();
        let status = wire(
            &router,
            "command.status",
            json!({"command_id":command}),
            None,
        );
        if router.response(status)["result"]["state"] == "committed" {
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(router.drain_responses().is_empty());
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
    assert!(
        router.response(wire(
            &router,
            "ui.workspaceMkdir",
            json!({"path":"made"}),
            Some(command)
        ))["result"]["ok"]
            == true
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn native_query_with_command_id_does_not_write_in_read_only_mode() {
    struct Reader;
    impl HostGateway for Reader {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            _: Value,
            _: Duration,
        ) -> Result<Value> {
            assert_eq!(operation, "ui.execute");
            Ok(json!({"path":"example.txt","size":5,"binary":false,"content":"hello"}))
        }
    }
    let (_dir, mut router, _run, _context) = seeded();
    router.attach_host(Arc::new(Reader));
    router.read_only = true;
    let before = router.snapshot().unwrap().commit_seq;
    let request = wire(
        &router,
        "ui.workspaceRead",
        json!({"path":"example.txt"}),
        Some(new_id("command")),
    );
    assert!(router.response(request).is_null());
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        router.maintain().unwrap();
        let responses = router.drain_responses();
        if !responses.is_empty() {
            assert_eq!(responses[0]["result"]["content"]["text"], "hello");
            break;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(router.snapshot().unwrap().commit_seq, before);
}
#[test]
fn compression_archives_originals_and_reopens_with_stable_references() {
    let (dir, mut router, run, context) = seeded();
    let snapshot = router.snapshot().unwrap();
    let mut settings = Planner::new(&snapshot, "test.noa", &json!({}));
    settings
        .insert(
            "settings",
            "setting",
            json!({"key":"noa_compaction","value":"true"}),
        )
        .unwrap();
    router.internal_commit(settings.done(json!({}))).unwrap();
    router.initialize_context_mode(&run).unwrap();
    for index in 0..12 {
        let message = Message {
            role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
            content: vec![
                json!({"type":"text","text":format!("Original {index}: {}", "x".repeat(3000))}),
            ],
        };
        router
            .append_transcript(&run, "text", "Long original", &message, None)
            .unwrap();
    }
    let bad = json!({"content":[{"startId":"m00001","endId":"m00002","summary":"Too short"}]});
    assert_eq!(
        router.compress_plan(&run, &bad, &context).unwrap_err().code,
        "INVALID_PARAMS"
    );
    let input = json!({"content":[{"startId":"m00001","endId":"m00004","summary":format!("Summary preserves decisions and references: {}","z".repeat(100))}]});
    let plan = router.compress_plan(&run, &input, &context).unwrap();
    let archive = plan.result["blocks"][0]["archive"].clone();
    router.internal_commit(plan).unwrap();
    let bytes = router.read_blob_bytes(&archive, 1024 * 1024).unwrap();
    assert!(String::from_utf8(bytes).unwrap().contains("Original 0"));
    let view = router.compressed_messages(&run).unwrap();
    assert_eq!(view.len(), 9);
    assert!(
        view[0].content[0]["text"]
            .as_str()
            .unwrap()
            .contains("id=\"b1\"")
    );
    assert!(view[1].content.to_string_for_test().contains("m00005"));
    assert!(router.compress_plan(&run, &input, &context).is_err());
    router.store.as_mut().unwrap().checkpoint().unwrap();
    router.store = None;
    router.store = Some(Store::open(&dir.path().join("store")).unwrap());
    assert_eq!(
        serde_json::to_value(router.compressed_messages(&run).unwrap()).unwrap(),
        serde_json::to_value(view).unwrap()
    );
}
trait JsonTest {
    fn to_string_for_test(&self) -> String;
}
impl JsonTest for Vec<Value> {
    fn to_string_for_test(&self) -> String {
        serde_json::to_string(self).unwrap()
    }
}
#[test]
fn large_query_body_is_leased_and_blob_readable() {
    let (_dir, mut router, _run, _context) = seeded();
    let value = router
        .externalize_query(artex_core::body(&"가".repeat(30000)))
        .unwrap();
    assert!(value["text"].is_null());
    let result = router.response(wire(
        &router,
        "blob.read",
        json!({"sha256":value["blob"]["sha256"],"offset":"0","length":65536}),
        None,
    ));
    assert_eq!(result["result"]["length"], "65536");
    assert!(result.get("error").is_none());
}
#[test]
fn read_only_large_body_uses_bounded_ephemeral_lease_without_a_store_write() {
    let (_dir, mut router, _run, _context) = seeded();
    router.read_only = true;
    let before = router.snapshot().unwrap().commit_seq;
    let value = router
        .externalize_query(artex_core::body(&"가".repeat(50000)))
        .unwrap();
    let reference = &value["blob"];
    assert_eq!(reference["length"], "150000");
    let result = router.response(wire(
        &router,
        "blob.read",
        json!({"sha256":reference["sha256"],"offset":"65536","length":65536}),
        None,
    ));
    assert_eq!(result["result"]["length"], "65536");
    assert_eq!(router.snapshot().unwrap().commit_seq, before);
    assert!(
        router
            .store
            .as_ref()
            .unwrap()
            .blob_read(reference, 0, 65536)
            .is_err()
    );
}
#[test]
fn orderly_close_persists_final_clock_pauses_task_and_checkpoints() {
    let (dir, mut router, run, context) = seeded();
    let snapshot = router.snapshot().unwrap();
    let batch = artex_domain::plan(
        "task.create",
        &json!({"description":"Close fixture","timeout_seconds":60}),
        &snapshot,
        &context,
    )
    .unwrap();
    let task = batch.result["task_id"].as_str().unwrap().to_owned();
    router.internal_commit(batch).unwrap();
    let snapshot = router.snapshot().unwrap();
    let runtime = snapshot
        .records("task_runtime")
        .into_iter()
        .find(|r| r.data["task_id"] == task)
        .unwrap();
    let runtime_id = runtime.header.id.clone();
    let mut p = Planner::new(&snapshot, "test.active", &json!({}));
    let mut data = snapshot.get("tasks", &task).unwrap().data.clone();
    data["status"] = json!("running");
    p.put("tasks", &task, data);
    let question=p.insert("user_questions","question",json!({"run_id":run,"kind":"question","payload":{"questions":[{"question":"Fixture?"}]},"state":"pending","generation":"1","deadline_at":artex_core::now()})).unwrap();
    let mut data = runtime.data.clone();
    data["last_active_clock"] = json!(
        artex_platform::active_clock_ms()
            .unwrap()
            .saturating_sub(100)
            .to_string()
    );
    p.put("task_runtime", &runtime_id, data);
    router.internal_commit(p.done(json!({}))).unwrap();
    let response = router.response(wire(&router, "workspace.close", json!({}), None));
    assert_eq!(response["result"]["closed"], true, "{response}");
    assert!(router.store.is_none());
    let store = Store::open(&dir.path().join("store")).unwrap();
    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.get("tasks", &task).unwrap().data["status"],
        "paused"
    );
    let runtime = snapshot.get("task_runtime", &runtime_id).unwrap();
    assert_eq!(runtime.data["accounting_state"], "known");
    assert!(runtime.data["last_active_clock"].is_null());
    assert!(parse_u64(runtime.data["active_elapsed_ms"].as_str().unwrap()).unwrap() >= 100);
    assert_eq!(
        snapshot.get("user_questions", &question).unwrap().data["state"],
        "cancelled"
    );
    assert_eq!(
        snapshot.get("agent_runs", &run).unwrap().data["state"],
        "interrupted"
    );
    assert!(
        !artex_agent::recovery_plan(&snapshot, &new_id("epoch")).is_some_and(|b| b
            .changes
            .iter()
            .any(
                |c| matches!(c,artex_core::Change::Put{collection,..}if collection=="task_runtime")
            ))
    );
}
#[test]
fn corrupt_store_readonly_overrides_cached_router_flag_and_can_close() {
    let (dir, mut router, _run, _context) = seeded();
    let path = dir.path().join("store");
    drop(router.store.take());
    let journal = std::fs::read_dir(path.join("store/journal"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "jnl"))
        .unwrap();
    let mut bytes = std::fs::read(&journal).unwrap();
    bytes[104] ^= 1;
    std::fs::write(journal, bytes).unwrap();
    router.store = Some(Store::open(&path).unwrap());
    assert!(router.store.as_ref().unwrap().read_only());
    assert!(!router.read_only);
    let response = router.response(wire(&router, "workspace.info", json!({}), None));
    assert_eq!(response["result"]["read_only"], true, "{response}");
    router.maintain().unwrap();
    let response = router.response(wire(&router, "workspace.close", json!({}), None));
    assert_eq!(response["result"]["closed"], true, "{response}");
    assert!(router.store.is_none());
    assert!(Store::open(&path).unwrap().read_only());
}
#[test]
fn delayed_native_completion_cannot_commit_after_same_workspace_reopen() {
    let (dir, mut router, _run, _context) = seeded();
    let command = new_id("command");
    let request: Request = serde_json::from_value(wire(
        &router,
        "ui.workspaceMkdir",
        json!({"path":"late"}),
        Some(command.clone()),
    ))
    .unwrap();
    let request = DispatchRequest::from(request);
    let release = Arc::new(AtomicBool::new(false));
    let gate = release.clone();
    router
        .defer_read_worker(&request, move |_| {
            let until = Instant::now() + Duration::from_secs(5);
            while !gate.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(json!({"ok":true,"path":"late"}))
        })
        .unwrap();
    let response = router.response(wire(&router, "workspace.close", json!({}), None));
    assert_eq!(response["result"]["closed"], true, "{response}");
    router.store = Some(Store::open(&dir.path().join("store")).unwrap());
    let before = router.snapshot().unwrap().commit_seq;
    release.store(true, Ordering::SeqCst);
    let until = Instant::now() + Duration::from_secs(2);
    while !router.runtime.ui_generations.is_empty() {
        router.maintain().unwrap();
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(router.snapshot().unwrap().commit_seq, before);
    assert!(
        !router
            .snapshot()
            .unwrap()
            .records("command_receipts")
            .iter()
            .any(|r| r.data["command_id"] == command)
    );
}
