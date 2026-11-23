//! Real provider completions must survive the writer, public history query and
//! checkpoint/reopen boundary; decoder-only tests cannot prove this contract.
use super::*;
use artex_runtime::HostGateway;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};

struct Secrets;
impl HostGateway for Secrets {
    fn request(&self, _: &str, _: &Context, op: &str, _: Value, _: Duration) -> Result<Value> {
        match op {
            "secret.resolve" => Ok(json!({"value":"isolated-fixture"})),
            "run.stop" => Ok(json!({"accepted":true})),
            _ => Err(Error::new(
                "EXTERNAL_FAILURE",
                "Unexpected fixture host call",
            )),
        }
    }
}
fn state() -> RunState {
    RunState {
        cancellation: Cancellation::default(),
        in_flight: false,
        turns: 0,
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
struct HttpFixture {
    url: String,
    stop: mpsc::Sender<()>,
    worker: std::thread::JoinHandle<Vec<Value>>,
}
impl HttpFixture {
    fn new(responses: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (stop, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            while rx.try_recv().is_err() {
                let (mut socket, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                // Windows accepted sockets can inherit the listener's mode.
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                    assert!(head.len() < 65536);
                }
                let len = String::from_utf8(head)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; len];
                socket.read_exact(&mut body).unwrap();
                requests.push(serde_json::from_slice::<Value>(&body).unwrap());
                let (kind, response) = responses.get(requests.len() - 1).unwrap_or_else(|| {
                    panic!("Unexpected regeneration request {}", requests.len())
                });
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).unwrap();
            }
            requests
        });
        Self { url, stop, worker }
    }
    fn finish(self) -> Vec<Value> {
        self.stop.send(()).unwrap();
        self.worker.join().unwrap()
    }
}
fn profile(router: &mut Router, run: &str, format: &str, url: &str, streaming: bool) -> String {
    let s = router.snapshot().unwrap();
    let mut p = Planner::new(&s, "fixture.profile", &json!({}));
    let id = p
        .insert(
            "llm_profiles",
            "profile",
            json!({"name":"fixture","model":"fixture","format":format,
        "base_url":url,"is_default":true,"non_streaming":!streaming,"streaming":streaming,
        "max_tokens":1024,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}}),
        )
        .unwrap();
    let aid = text(&s.get("agent_runs", run).unwrap().data, "agent_id");
    let mut agent = s.get("agents", aid).unwrap().data.clone();
    agent["llm_profile_id"] = json!(id);
    agent["max_turns"] = json!(1);
    agent["wrapup_max_turns"] = json!(1);
    p.put("agents", aid, agent);
    router.internal_commit(p.done(json!({}))).unwrap();
    router.attach_host(Arc::new(Secrets));
    router.runtime.runs.insert(run.into(), state());
    router
        .append_transcript(
            run,
            "user",
            "Fixture input",
            &Message {
                role: "user".into(),
                content: vec![json!({"type":"text","text":"Complete the isolated answer"})],
            },
            None,
        )
        .unwrap();
    id
}
fn model_step(router: &mut Router, run: &str) {
    router.start_model(run).unwrap();
    match router
        .runtime
        .rx
        .recv_timeout(Duration::from_secs(20))
        .unwrap()
    {
        Completion::Model {
            run: rid,
            metadata,
            result,
        } => {
            assert_eq!(rid, run);
            router.complete_model(run, &metadata, result).unwrap();
        }
        _ => panic!("Expected actual provider completion"),
    }
}
fn history(router: &mut Router, conversation: &str) -> Value {
    let s = router.snapshot().unwrap();
    let response = router.response(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0",
        "id":new_id("request"),"workspace_id":s.workspace_id,"engine_epoch":router.engine_epoch,
        "method":"ui.conversationMessages","command_id":null,"expected":[],
        "params":{"id":conversation},"timeout_ms":5000}));
    assert!(response["error"].is_null(), "{response}");
    response["result"].clone()
}
fn reopen(router: &mut Router, dir: &std::path::Path) {
    router.store.as_mut().unwrap().checkpoint().unwrap();
    drop(router.store.take());
    router.store = Some(Store::open(&dir.join("store")).unwrap());
}
fn answer(format: &str, text: &str, capped: bool) -> String {
    if format == "anthropic" {
        json!({"content":[{"type":"text","text":text}],"stop_reason":if capped{"max_tokens"}else{"end_turn"},
            "usage":{"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":17,"cache_creation_input_tokens":5}}).to_string()
    } else if format == "openai" {
        json!({"choices":[{"message":{"content":text},"finish_reason":if capped{"length"}else{"stop"}}],
            "usage":{"prompt_tokens":25,"completion_tokens":2}}).to_string()
    } else {
        json!({"status":if capped{"incomplete"}else{"completed"},
            "incomplete_details":if capped{json!({"reason":"max_output_tokens"})}else{Value::Null},
            "output":[{"type":"message","content":[{"type":"output_text","text":text}]}],
            "usage":{"input_tokens":25,"output_tokens":2}})
        .to_string()
    }
}
#[test]
fn output_cap_resumes_confirmed_history_without_spending_another_logical_turn() {
    for format in ["anthropic", "openai-responses", "openai"] {
        let fixture = HttpFixture::new(vec![
            ("application/json", answer(format, "PARTIAL_EVIDENCE", true)),
            ("application/json", answer(format, "FINAL_EVIDENCE", false)),
        ]);
        let (dir, mut router, run, _) = crate::regression::seeded();
        let conv = text(
            &router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data,
            "conversation_id",
        )
        .to_owned();
        profile(&mut router, &run, format, &fixture.url, false);
        model_step(&mut router, &run);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "running"
        );
        assert_eq!(router.runtime.runs[&run].turns, 0);
        model_step(&mut router, &run);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "completed"
        );
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        let second = requests[1].to_string();
        assert!(second.contains("PARTIAL_EVIDENCE") && second.contains("Resume directly"));
        assert!(!second.contains("Turn budget wrap-up"));
        let before = history(&mut router, &conv);
        reopen(&mut router, dir.path());
        assert_eq!(history(&mut router, &conv), before);
        assert!(
            before.to_string().contains("PARTIAL_EVIDENCE")
                && before.to_string().contains("FINAL_EVIDENCE")
        );
        let s = router.snapshot().unwrap();
        let usage = s.records("llm_usage");
        assert_eq!(usage.len(), 2);
        assert!(
            usage
                .iter()
                .all(|u| u.data["input_tokens"] == 25 && u.data["output_tokens"] == 2)
        );
        if format == "anthropic" {
            assert!(
                usage
                    .iter()
                    .all(|u| u.data["cache_read"] == 17 && u.data["cache_write"] == 5)
            );
        }
    }
}
#[test]
fn output_cap_recovery_is_bounded_and_normal_answers_need_one_call() {
    for (capped, count) in [(true, 4), (false, 1)] {
        let responses = (0..count)
            .map(|_| ("application/json", answer("anthropic", "RETAINED", capped)))
            .collect();
        let fixture = HttpFixture::new(responses);
        let (_dir, mut router, run, _) = crate::regression::seeded();
        profile(&mut router, &run, "anthropic", &fixture.url, false);
        for _ in 0..count {
            model_step(&mut router, &run);
        }
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "completed"
        );
        assert_eq!(fixture.finish().len(), count);
        assert_eq!(router.snapshot().unwrap().records("llm_usage").len(), count);
    }
}

#[test]
fn empty_or_thinking_only_capped_turns_reach_the_same_durable_continuation_path() {
    for thinking in [false, true] {
        let mut capped: Value = serde_json::from_str(&answer("anthropic", "", true)).unwrap();
        capped["content"] = if thinking {
            json!([{"type":"thinking","thinking":"OBSERVED_REASONING","signature":"fixture"}])
        } else {
            json!([])
        };
        let fixture = HttpFixture::new(vec![
            ("application/json", capped.to_string()),
            (
                "application/json",
                answer("anthropic", "CONTINUED_ANSWER", false),
            ),
        ]);
        let (dir, mut router, run, _) = crate::regression::seeded();
        let conv = text(
            &router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data,
            "conversation_id",
        )
        .to_owned();
        profile(&mut router, &run, "anthropic", &fixture.url, false);
        model_step(&mut router, &run);
        assert_eq!(router.runtime.runs[&run].output_recoveries, 1);
        model_step(&mut router, &run);
        let requests = fixture.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].to_string().contains("Resume directly"));
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "completed"
        );
        reopen(&mut router, dir.path());
        assert!(
            history(&mut router, &conv)
                .to_string()
                .contains("CONTINUED_ANSWER")
        );
        assert_eq!(router.snapshot().unwrap().records("llm_usage").len(), 2);
    }
}
#[test]
fn named_responses_terminal_without_output_preserves_single_answer_and_usage_after_reopen() {
    for terminal in [
        json!({"status":"completed","output":[],"usage":{"input_tokens":9,"output_tokens":4}}),
        json!({"status":"completed","usage":{"input_tokens":9,"output_tokens":4}}),
    ] {
        let fixture = HttpFixture::new(vec![(
            "text/event-stream",
            format!(
                "event: response.output_text.delta\ndata: {{\"delta\":\"ONCE_ONLY_EVIDENCE\"}}\n\nevent: response.completed\ndata: {{\"response\":{terminal}}}\n\n"
            ),
        )]);
        let (dir, mut router, run, _) = crate::regression::seeded();
        let conv = text(
            &router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data,
            "conversation_id",
        )
        .to_owned();
        profile(&mut router, &run, "openai-responses", &fixture.url, true);
        model_step(&mut router, &run);
        assert_eq!(fixture.finish().len(), 1);
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &run)
                .unwrap()
                .data["state"],
            "completed"
        );
        let before = history(&mut router, &conv);
        assert!(before.to_string().contains("ONCE_ONLY_EVIDENCE"));
        reopen(&mut router, dir.path());
        assert_eq!(history(&mut router, &conv), before);
        let s = router.snapshot().unwrap();
        let usage = s.records("llm_usage");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].data["input_tokens"], 9);
        assert_eq!(usage[0].data["output_tokens"], 4);
    }
}
#[test]
fn own_deadline_does_not_poison_pool_but_real_provider_failures_still_fail_over() {
    for (code, expired, want_health, want_backup) in [
        ("TIMEOUT", true, false, false),
        ("TIMEOUT", false, true, true),
        ("CANCELLED", false, false, false),
        ("EXTERNAL_FAILURE", false, true, true),
    ] {
        let (dir, mut router, run, _) = crate::regression::seeded();
        let s = router.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.pool", &json!({}));
        for key in ["llm_pool_enabled", "llm_pool_bind_fallback"] {
            p.insert("settings", "setting", json!({"key":key,"value":"true"}))
                .unwrap();
        }
        let mut ids = Vec::new();
        for (name, priority) in [("primary", 2), ("backup", 1)] {
            ids.push(
                p.insert(
                    "llm_profiles",
                    "profile",
                    json!({"name":name,"priority":priority,"model":"fixture",
                "is_default":priority==2,"format":"openai","base_url":"http://127.0.0.1:1/v1",
                "api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}}),
                )
                .unwrap(),
            );
        }
        let aid = text(&s.get("agent_runs", &run).unwrap().data, "agent_id");
        let mut agent = s.get("agents", aid).unwrap().data.clone();
        agent["llm_profile_id"] = json!(ids[0]);
        agent["wrapup_max_turns"] = json!(1);
        p.put("agents", aid, agent);
        let mut data = s.get("agent_runs", &run).unwrap().data.clone();
        data["deadline_at"] = json!(
            (Utc::now() + ChronoDuration::seconds(if expired { -1 } else { 120 }))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
        p.put("agent_runs", &run, data);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.attach_host(Arc::new(Secrets));
        let mut running = state();
        running.in_flight = true;
        // A fixture fence prevents the expected backup from issuing real HTTP.
        running.retry_at = Some(Instant::now() + Duration::from_secs(60));
        router.runtime.runs.insert(run.clone(), running);
        let mut error = Error::new(code, "Isolated completion control");
        error.details = json!({"committed":false});
        router.runtime.tx.send(Completion::Model{run:run.clone(),metadata:json!({"profile_id":ids[0],"profile_name":"primary","model":"fixture","request_body":{},"record_enabled":false,"latency_ms":1}),result:Err(error)}).unwrap();
        router.runtime_tick().unwrap();
        let s = router.snapshot().unwrap();
        let health = s.records("llm_profile_health");
        assert_eq!(!health.is_empty(), want_health, "{code}/{expired}");
        if want_health {
            assert_eq!(health[0].data["fails"], 1);
        }
        let children = s
            .records("agent_runs")
            .into_iter()
            .filter(|r| r.data["parent_run_id"] == run)
            .collect::<Vec<_>>();
        assert_eq!(children.len(), usize::from(expired));
        if want_backup {
            assert_eq!(
                router.runtime.runs[&run].profile_override.as_deref(),
                Some(ids[1].as_str())
            );
        } else {
            assert!(
                !s.records("activity")
                    .iter()
                    .any(|a| a.data["kind"] == "llm_switch")
            );
        }
        let count = health.len();
        reopen(&mut router, dir.path());
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("llm_profile_health")
                .len(),
            count
        );
    }
}

#[test]
fn real_http_request_expiring_at_the_run_deadline_settles_without_provider_health_failure() {
    for connect_attempts in [0, -1] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (stop, stopped) = mpsc::channel();
        let server = std::thread::spawn(move || {
            // Retain the accepted socket without a response until the real provider
            // deadline fires; no synthetic Completion or external provider is used.
            let mut sockets = Vec::new();
            while stopped.try_recv().is_err() {
                match listener.accept() {
                    Ok((socket, _)) => sockets.push(socket),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("{e}"),
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            sockets.len()
        });
        let (dir, mut router, run, _) = crate::regression::seeded();
        let profile_id = profile(&mut router, &run, "anthropic", &url, false);
        let s = router.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.real.deadline", &json!({}));
        let mut data = s.get("llm_profiles", &profile_id).unwrap().data.clone();
        data["retry_connect_attempts"] = json!(connect_attempts);
        p.put("llm_profiles", &profile_id, data);
        for key in ["llm_pool_enabled", "llm_pool_bind_fallback"] {
            p.insert("settings", "setting", json!({"key":key,"value":"true"}))
                .unwrap();
        }
        p.insert("llm_profiles", "profile", json!({"name":"backup","model":"fixture",
        "format":"anthropic","base_url":url,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"}})).unwrap();
        let mut data = s.get("agent_runs", &run).unwrap().data.clone();
        // Leave room for durable fixture admission on shared CI disks, so this
        // exercises an accepted HTTP request rather than expiry before connection.
        data["deadline_at"] = json!(
            (Utc::now() + ChronoDuration::seconds(5))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
        p.put("agent_runs", &run, data);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.start_model(&run).unwrap();
        match router
            .runtime
            .rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
        {
            Completion::Model {
                run: completed,
                metadata,
                result,
            } => {
                assert_eq!(completed, run);
                assert_eq!(metadata["profile_id"], profile_id);
                assert_eq!(result.as_ref().unwrap_err().code, "TIMEOUT");
                router.complete_model(&run, &metadata, result).unwrap();
            }
            _ => panic!("Expected actual deadline completion"),
        }
        stop.send(()).unwrap();
        assert_eq!(
            server.join().unwrap(),
            1,
            "Deadline must not select/retry the backup"
        );
        let s = router.snapshot().unwrap();
        assert!(s.records("llm_profile_health").is_empty());
        assert_eq!(s.get("agent_runs", &run).unwrap().data["state"], "failed");
        assert_eq!(s.records("llm_usage").len(), 1);
        assert_eq!(s.records("llm_usage")[0].data["status"], "failed");
        reopen(&mut router, dir.path());
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("llm_profile_health")
                .is_empty()
        );
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("agent_runs")
                .iter()
                .filter(|r| r.data["parent_run_id"] == run)
                .count(),
            1
        );
    }
}
