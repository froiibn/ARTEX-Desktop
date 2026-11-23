//! Tool-free side questions use their admitted checkpoint and own cancellation.
use super::*;
use artex_domain::{record, text};
use artex_runtime::{Cancellation, Turn};
use std::{sync::mpsc, time::Duration};
#[path = "side_context.rs"]
mod context;
struct Pending {
    cancel: Cancellation,
    generation: String,
}
struct Finished {
    id: String,
    workspace: String,
    epoch: String,
    generation: String,
    context: Value,
    memory: Value,
    usage: Value,
    // Preparation progress keeps memory durable before the final answer; it
    // shares the same request/generation fence and never releases its worker.
    result: Option<Result<Turn>>,
}
pub(super) struct SideState {
    pending: BTreeMap<String, Pending>,
    tx: mpsc::Sender<Finished>,
    rx: mpsc::Receiver<Finished>,
}
impl Default for SideState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            pending: Default::default(),
            tx,
            rx,
        }
    }
}
impl Drop for SideState {
    fn drop(&mut self) {
        for p in self.pending.values() {
            p.cancel.cancel();
        }
    }
}
impl Router {
    pub(super) fn side_context(&self) -> Context {
        Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        }
    }
    pub(super) fn process_side_questions(&mut self) -> Result<()> {
        if self.store.is_none() || self.read_only {
            self.side = Default::default();
            return Ok(());
        }
        let s = self.snapshot()?;
        for (id, pending) in &self.side.pending {
            if s.get("side_question_requests", id).is_none_or(|r| {
                r.data["status"] != "running" || r.data["generation"] != pending.generation
            }) {
                pending.cancel.cancel();
            }
        }
        while let Ok(done) = self.side.rx.try_recv() {
            let final_update = done.result.is_some();
            if final_update {
                self.side.pending.remove(&done.id);
            }
            if done.workspace != self.snapshot()?.workspace_id || done.epoch != self.engine_epoch {
                continue;
            }
            let (answer, status, error, usage) = match done.result {
                Some(Ok(turn)) => (
                    artex_core::body(&turn.text),
                    "completed",
                    String::new(),
                    turn.usage,
                ),
                Some(Err(e)) => (
                    // Source SideQuestionService returns observed text with an
                    // interrupted answer. The guarded writer below keeps it
                    // failed, and still rejects cancelled/cleared/stale workers.
                    artex_core::body(e.details["partial_text"].as_str().unwrap_or("")),
                    if e.code == "CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    format!("{}: {}", e.code, e.message),
                    done.usage,
                ),
                None => (artex_core::body(""), "running", String::new(), done.usage),
            };
            let plan = artex_domain::side_questions::update_plan(
                self.snapshot()?.as_ref(),
                &self.side_context(),
                &done.id,
                &done.generation,
                &answer,
                status,
                &error,
                &usage,
                &done.context,
                Some(&done.memory),
            )?;
            if !plan.changes.is_empty() {
                self.internal_commit(plan)?;
            }
            if final_update {
                let usage_plan = artex_domain::side_questions::cancelled_usage_plan(
                    self.snapshot()?.as_ref(),
                    &self.side_context(),
                    &done.id,
                    &done.generation,
                    &usage,
                )?;
                if !usage_plan.changes.is_empty() {
                    self.internal_commit(usage_plan)?;
                }
            }
        }
        if !self.deleting_tasks.is_empty() {
            return Ok(());
        }
        let Some(gateway) = self.gateway.clone() else {
            return Ok(());
        };
        let s = self.snapshot()?;
        for request in s
            .records("side_question_requests")
            .into_iter()
            .filter(|r| r.data["status"] == "running")
        {
            if self.side.pending.contains_key(&request.header.id) {
                continue;
            }
            if self.side.pending.len() >= 4 {
                break;
            }
            let id = request.header.id.clone();
            let generation = text(&request.data, "generation").to_owned();
            let prepared: Result<(Value, Value)> = (|| {
                let mut input = artex_domain::side_questions::execution_input(&s, &id)?;
                let profile = record(&s, "llm_profiles", text(&input, "profile_id"))?
                    .data
                    .clone();
                // Resolve immutable bodies on the writer. The worker receives
                // only its admitted data and cannot reach the parent store.
                for exchange in input["history"].as_array_mut().into_iter().flatten() {
                    exchange["answer"] = json!(self.body_text(&exchange["answer"])?);
                }
                Ok((profile, input))
            })();
            let (profile, input) = match prepared {
                Ok(v) => v,
                Err(e) => {
                    let p = artex_domain::side_questions::update_plan(
                        &s,
                        &self.side_context(),
                        &id,
                        &generation,
                        &artex_core::body(""),
                        "failed",
                        &format!("{}: {}", e.code, e.message),
                        &json!({}),
                        &json!({"phase":"failed"}),
                        None,
                    )?;
                    self.internal_commit(p)?;
                    continue;
                }
            };
            let workspace = s.workspace_id.clone();
            let epoch = self.engine_epoch.clone();
            let context = self.side_context();
            let cancel = Cancellation::default();
            self.side.pending.insert(
                id.clone(),
                Pending {
                    cancel: cancel.clone(),
                    generation: generation.clone(),
                },
            );
            let tx = self.side.tx.clone();
            let gateway = gateway.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let timeout = Duration::from_millis(input["timeout_ms"].as_u64().unwrap_or(120000));
                let mut builder = context::SideContext::new(profile.clone(), input);
                let result = (|| {
                    let secret = gateway.request(
                        &workspace,
                        &context,
                        "secret.resolve_user",
                        json!({"secret_ref":profile["api_key"]["secret_ref"]}),
                        Duration::from_secs(10),
                    )?;
                    cancel.check()?;
                    let secret = secret["value"].as_str().ok_or_else(|| {
                        Error::new(
                            "SECRET_UNAVAILABLE",
                            "Captured model credential is unavailable",
                        )
                    })?;
                    builder.respond(
                        &mut |profile, prompt| {
                            cancel.check()?;
                            let remaining = timeout.saturating_sub(started.elapsed());
                            if remaining.is_zero() {
                                return Err(Error::new(
                                    "TIMEOUT",
                                    "Side-question deadline elapsed",
                                ));
                            }
                            artex_runtime::provider::generate(
                                profile, secret, prompt, &cancel, remaining,
                            )
                        },
                        &mut |state| {
                            let _ = tx.send(Finished {
                                id: id.clone(),
                                workspace: workspace.clone(),
                                epoch: epoch.clone(),
                                generation: generation.clone(),
                                context: state.info.clone(),
                                memory: state.memory.clone(),
                                usage: state.usage.clone(),
                                result: None,
                            });
                        },
                    )
                })();
                let _ = tx.send(Finished {
                    id,
                    workspace,
                    epoch,
                    generation,
                    context: builder.info,
                    memory: builder.memory,
                    usage: builder.usage,
                    result: Some(result),
                });
            });
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_runtime::HostGateway;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };
    struct Credentials;
    impl HostGateway for Credentials {
        fn request(
            &self,
            _: &str,
            c: &Context,
            operation: &str,
            _: Value,
            _: Duration,
        ) -> Result<Value> {
            assert_eq!(operation, "secret.resolve_user");
            assert_eq!(c.actor.kind, "user");
            Ok(json!({"value":"fixture-secret"}))
        }
    }
    #[test]
    fn actual_side_provider_uses_admitted_snapshot_and_clear_rejects_late_answer() {
        let (_dir, mut router, rid, parent_context) = super::super::regression::seeded();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let received = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let server_received = received.clone();
        let server_release = release.clone();
        let server = std::thread::spawn(move || {
            let mut requests = vec![];
            let deadline = Instant::now() + Duration::from_secs(10);
            for index in 0..2 {
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(v) => break v,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = vec![];
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let size = String::from_utf8(head)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                let mut data = vec![0; size];
                socket.read_exact(&mut data).unwrap();
                requests.push(serde_json::from_slice::<Value>(&data).unwrap());
                if index == 1 {
                    server_received.store(true, Ordering::SeqCst);
                    while !server_release.load(Ordering::SeqCst) {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                let body=serde_json::to_vec(&json!({"choices":[{"message":{"content":"독립 질문 답변"},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":4}})).unwrap();
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(&body);
            }
            requests
        });
        let s = router.snapshot().unwrap();
        let cid = s.get("agent_runs", &rid).unwrap().data["conversation_id"].clone();
        let mut seed = artex_domain::Planner::new(&s, "side.test.profile", &json!({}));
        let pid=seed.insert("llm_profiles","profile",json!({"name":"Fixture","model":"fixture","format":"openai","base_url":format!("http://{address}"),"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"is_default":true,"streaming":false,"retry_connect_attempts":-1,"retry_empty_attempts":-1,"retry_stream_attempts":-1})).unwrap();
        router.internal_commit(seed.done(json!({}))).unwrap();
        router.attach_host(Arc::new(Credentials));
        let routes = artex_core::contracts::read("legacy-routes.v1.json");
        let route = |method: &str| {
            routes["routes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| {
                    r["handler"] == "handleSideQuestions"
                        && r["parent_kind"] == "conversation"
                        && r["legacy_http_method"] == method
                })
                .unwrap()["route_id"]
                .clone()
        };
        let parameters = |method: &str, client: &str| json!({"route_id":route(method),"path":{"id":cid},"query":{},"body":{"question":"부모 작업은 무엇인가?","client_request_id":client}});
        let s = router.snapshot().unwrap();
        let profile = s.get("llm_profiles", &pid).unwrap();
        let checkpoint = artex_domain::side_questions::checkpoint_plan(
            &s,
            &parent_context,
            &rid,
            1,
            profile,
            &json!({"system":"ORIGINAL FROZEN CONTEXT","messages":[],"tools":[]}),
        )
        .unwrap();
        router.internal_commit(checkpoint).unwrap();
        let user = router.side_context();
        let plan = artex_domain::side_questions::plan(
            "legacy.handleSideQuestions",
            &parameters("POST", "first"),
            router.snapshot().unwrap().as_ref(),
            &user,
        )
        .unwrap();
        let first = plan.result["id"].as_str().unwrap().to_owned();
        router.internal_commit(plan).unwrap();
        let s = router.snapshot().unwrap();
        let checkpoint = artex_domain::side_questions::checkpoint_plan(
            &s,
            &parent_context,
            &rid,
            2,
            s.get("llm_profiles", &pid).unwrap(),
            &json!({"system":"NEW PARENT CONTENT","messages":[],"tools":[]}),
        )
        .unwrap();
        router.internal_commit(checkpoint).unwrap();
        let before = router
            .snapshot()
            .unwrap()
            .records("conversation_activities")
            .len();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            router.process_side_questions().unwrap();
            if router
                .snapshot()
                .unwrap()
                .get("side_question_requests", &first)
                .unwrap()
                .data["status"]
                == "completed"
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("side_question_requests", &first)
                .unwrap()
                .data["answer"]["text"],
            "독립 질문 답변"
        );
        let first_state = router.snapshot().unwrap();
        let first_context = &first_state
            .get("side_question_requests", &first)
            .unwrap()
            .data["context_info"];
        assert_eq!(
            first_context["frozen_snapshot"]["request"]["system"], "ORIGINAL FROZEN CONTEXT",
            "preparation progress must retain the admitted checkpoint even after the parent advances"
        );
        assert_eq!(first_context["admitted_question"], "부모 작업은 무엇인가?");
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("conversation_activities")
                .len(),
            before
        );
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("tool_invocations")
                .is_empty()
        );
        let plan = artex_domain::side_questions::plan(
            "legacy.handleSideQuestions",
            &parameters("POST", "second"),
            router.snapshot().unwrap().as_ref(),
            &user,
        )
        .unwrap();
        router.internal_commit(plan).unwrap();
        while !received.load(Ordering::SeqCst) {
            router.process_side_questions().unwrap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        let clear = artex_domain::side_questions::plan(
            "legacy.handleSideQuestions",
            &parameters("DELETE", ""),
            router.snapshot().unwrap().as_ref(),
            &user,
        )
        .unwrap();
        router.internal_commit(clear).unwrap();
        router.process_side_questions().unwrap();
        release.store(true, Ordering::SeqCst);
        while !router.side.pending.is_empty() {
            router.process_side_questions().unwrap();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("side_question_requests")
                .is_empty()
        );
        let requests = server.join().unwrap();
        assert!(requests[0]["tools"].as_array().unwrap().is_empty());
        let sent = requests[0].to_string();
        assert!(sent.contains("ORIGINAL FROZEN CONTEXT"));
        assert!(!sent.contains("NEW PARENT CONTENT"));
    }

    #[test]
    fn actual_side_history_summary_is_durable_and_reused_after_checkpoint_reopen() {
        let (dir, mut router, rid, parent_context) = super::super::regression::seeded();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            for index in 0..4 {
                let (mut socket, _) = loop {
                    match listener.accept() {
                        Ok(value) => break value,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline);
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("{e}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let size = String::from_utf8(head)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                let mut data = vec![0; size];
                socket.read_exact(&mut data).unwrap();
                requests.push(serde_json::from_slice::<Value>(&data).unwrap());
                let response = if index % 2 == 0 {
                    "Original allowed scope and older constraints preserved."
                } else {
                    "Answer using the frozen context and older scope."
                };
                let body = serde_json::to_vec(&json!({"choices":[{"message":{"content":response},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":4}})).unwrap();
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                socket.write_all(&body).unwrap();
            }
            requests
        });
        let s = router.snapshot().unwrap();
        let cid = s.get("agent_runs", &rid).unwrap().data["conversation_id"].clone();
        let mut seed = artex_domain::Planner::new(&s, "side.history.profile", &json!({}));
        let pid = seed.insert("llm_profiles", "profile", json!({"name":"Fixture","model":"fixture","format":"openai","base_url":format!("http://{address}"),"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"is_default":true,"streaming":false,"retry_connect_attempts":-1,"retry_empty_attempts":-1,"retry_stream_attempts":-1})).unwrap();
        router.internal_commit(seed.done(json!({}))).unwrap();
        let s = router.snapshot().unwrap();
        let plan = artex_domain::side_questions::checkpoint_plan(
            &s,
            &parent_context,
            &rid,
            1,
            s.get("llm_profiles", &pid).unwrap(),
            &json!({"system":"Frozen original scope","messages":[],"tools":[]}),
        )
        .unwrap();
        router.internal_commit(plan).unwrap();
        let s = router.snapshot().unwrap();
        let session = s.records("side_question_sessions")[0];
        let session_id = session.header.id.clone();
        let mut history = artex_domain::Planner::new(&s, "side.history.seed", &json!({}));
        for ordinal in 1..=21 {
            history.insert("side_question_requests", "sidequestion", json!({"session_key":session.data["session_key"],"ordinal":ordinal.to_string(),"generation":session.data["generation"],"client_id":format!("history-{ordinal}"),"question":format!("Earlier question {ordinal}"),"answer":artex_core::body(&format!("Historical answer {ordinal}")),"status":"completed","model":session.data["snapshot"]["model"],"snapshot_at":session.data["snapshot"]["captured_at"]})).unwrap();
        }
        router.internal_commit(history.done(json!({}))).unwrap();
        router.attach_host(Arc::new(Credentials));
        let routes = artex_core::contracts::read("legacy-routes.v1.json");
        let route = routes["routes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| {
                r["handler"] == "handleSideQuestions"
                    && r["parent_kind"] == "conversation"
                    && r["legacy_http_method"] == "POST"
            })
            .unwrap()["route_id"]
            .clone();
        let parent_rows = router
            .snapshot()
            .unwrap()
            .records("conversation_activities")
            .len();
        for index in 0..2 {
            if index == 1 {
                router.store.as_mut().unwrap().checkpoint().unwrap();
                router.store.take();
                router.store = Some(Store::open(&dir.path().join("store")).unwrap());
                assert_eq!(
                    router
                        .snapshot()
                        .unwrap()
                        .get("side_question_sessions", &session_id)
                        .unwrap()
                        .data["memory"]["through"],
                    "1"
                );
            }
            let parameters = json!({"route_id":route,"path":{"id":cid},"query":{},"body":{"question":"Keep earlier constraints?","client_request_id":format!("current-{index}")}});
            let plan = artex_domain::side_questions::plan(
                "legacy.handleSideQuestions",
                &parameters,
                router.snapshot().unwrap().as_ref(),
                &router.side_context(),
            )
            .unwrap();
            let id = plan.result["id"].as_str().unwrap().to_owned();
            router.internal_commit(plan).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                router.process_side_questions().unwrap();
                if router
                    .snapshot()
                    .unwrap()
                    .get("side_question_requests", &id)
                    .unwrap()
                    .data["status"]
                    != "running"
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let s = router.snapshot().unwrap();
            let request = s.get("side_question_requests", &id).unwrap();
            assert_eq!(request.data["status"], "completed", "{}", request.data);
            assert_eq!(request.data["usage"]["input_tokens"], 18);
            assert_eq!(request.data["usage"]["output_tokens"], 8);
            assert_eq!(request.data["context_info"]["history_summarized"], true);
            assert_eq!(
                s.get("side_question_sessions", &session_id).unwrap().data["memory"]["through"],
                (index + 1).to_string()
            );
            assert_eq!(s.records("conversation_activities").len(), parent_rows);
            assert!(s.records("tool_invocations").is_empty());
        }
        let requests = server.join().unwrap();
        assert!(requests[0].to_string().contains("Earlier question 1"));
        assert!(
            requests[2]
                .to_string()
                .contains("Original allowed scope and older constraints preserved.")
        );
        assert!(requests[2].to_string().contains("Earlier question 2"));
        assert!(!requests[2].to_string().contains("Earlier question 1\""));
        assert!(
            requests
                .iter()
                .all(|request| request["tools"].as_array().unwrap().is_empty())
        );
    }

    #[test]
    fn cancelled_side_worker_settles_usage_without_reviving_answer_or_memory() {
        let (_dir, mut router, rid, parent_context) = super::super::regression::seeded();
        let s = router.snapshot().unwrap();
        let mut seed = artex_domain::Planner::new(&s, "side.cancel.fixture", &json!({}));
        let pid=seed.insert("llm_profiles","profile",json!({"name":"fixture","model":"fixture","format":"openai","api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"is_default":true})).unwrap();
        router.internal_commit(seed.done(json!({}))).unwrap();
        let s = router.snapshot().unwrap();
        let checkpoint = artex_domain::side_questions::checkpoint_plan(
            &s,
            &parent_context,
            &rid,
            1,
            s.get("llm_profiles", &pid).unwrap(),
            &json!({"system":"frozen","messages":[],"tools":[]}),
        )
        .unwrap();
        router.internal_commit(checkpoint).unwrap();
        let s = router.snapshot().unwrap();
        let session = s.records("side_question_sessions")[0];
        let generation = text(&session.data, "generation").to_owned();
        let session_id = session.header.id.clone();
        let memory = session.data["memory"].clone();
        let mut seed = artex_domain::Planner::new(&s, "side.cancel.seed", &json!({}));
        let id=seed.insert("side_question_requests","sidequestion",json!({"session_key":session.data["session_key"],"ordinal":"1","generation":generation,"client_id":"fixture","question":"cancelled","answer":artex_core::body("confirmed earlier partial answer"),"status":"cancelled","model":session.data["snapshot"]["model"],"snapshot_at":session.data["snapshot"]["captured_at"]})).unwrap();
        router.internal_commit(seed.done(json!({}))).unwrap();
        // Exercise the actual mailbox consumer: a late model answer must not
        // overwrite cancellation, while its observed usage is still billable.
        router
            .side
            .tx
            .send(Finished {
                id: id.clone(),
                workspace: router.snapshot().unwrap().workspace_id.clone(),
                epoch: router.engine_epoch.clone(),
                generation: generation.clone(),
                context: json!({"phase":"answering"}),
                memory: json!({"history":"must not persist"}),
                usage: json!({"input_tokens":17}),
                result: Some(Ok(Turn {
                    text: "late answer must not persist".into(),
                    usage: json!({"input_tokens":17}),
                    ..Default::default()
                })),
            })
            .unwrap();
        router.process_side_questions().unwrap();
        let s = router.snapshot().unwrap();
        let row = s.get("side_question_requests", &id).unwrap();
        assert_eq!(row.data["status"], "cancelled");
        assert_eq!(
            row.data["answer"]["text"],
            "confirmed earlier partial answer"
        );
        assert_eq!(row.data["usage"]["input_tokens"], 17);
        assert_eq!(
            s.get("side_question_sessions", &session_id).unwrap().data["memory"],
            memory
        );
        let mut interrupted = Error::new("EXTERNAL_FAILURE", "late interrupted response");
        interrupted.details = json!({"committed":true,"partial_text":"late failed draft must not overwrite cancellation","usage":{"input_tokens":23}});
        router
            .side
            .tx
            .send(Finished {
                id: id.clone(),
                workspace: s.workspace_id.clone(),
                epoch: router.engine_epoch.clone(),
                generation: generation.clone(),
                context: json!({}),
                memory: json!({"history":"must not persist"}),
                usage: json!({"input_tokens":23}),
                result: Some(Err(interrupted)),
            })
            .unwrap();
        router.process_side_questions().unwrap();
        let s = router.snapshot().unwrap();
        let row = s.get("side_question_requests", &id).unwrap();
        assert_eq!(row.data["status"], "cancelled");
        assert_eq!(
            row.data["answer"]["text"],
            "confirmed earlier partial answer"
        );
        assert_eq!(row.data["usage"]["input_tokens"], 23);
        assert_eq!(
            s.get("side_question_sessions", &session_id).unwrap().data["memory"],
            memory
        );
        let replay = artex_domain::side_questions::cancelled_usage_plan(
            &s,
            &router.side_context(),
            &id,
            &generation,
            &json!({"input_tokens":3}),
        )
        .unwrap();
        assert!(
            replay.changes.is_empty(),
            "replay cannot lower counters or double-count usage"
        );
        let mut next = artex_domain::Planner::new(&s, "side.cancel.generation", &json!({}));
        let mut data = s
            .get("side_question_sessions", &session_id)
            .unwrap()
            .data
            .clone();
        data["generation"] = json!((generation.parse::<u64>().unwrap() + 1).to_string());
        next.put("side_question_sessions", &session_id, data);
        router.internal_commit(next.done(json!({}))).unwrap();
        let stale = artex_domain::side_questions::cancelled_usage_plan(
            router.snapshot().unwrap().as_ref(),
            &router.side_context(),
            &id,
            &generation,
            &json!({"input_tokens":100}),
        )
        .unwrap();
        assert!(
            stale.changes.is_empty(),
            "clear/generation fence rejects old worker usage"
        );
    }
    #[test]
    fn actual_interrupted_side_answer_is_failed_with_durable_partial_text_and_usage() {
        for interrupted in [true, false] {
            let (dir, mut router, rid, parent_context) = super::super::regression::seeded();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let size = String::from_utf8(head)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap();
                let mut data = vec![0; size];
                socket.read_exact(&mut data).unwrap();
                let mut body = format!(
                    "data: {}\n\ndata: {}\n\n",
                    json!({"choices":[{"delta":{"content":"observed partial evidence"},"finish_reason":null}]}),
                    json!({"choices":[],"usage":{"prompt_tokens":17,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":7}}})
                );
                if !interrupted {
                    body.push_str(&format!(
                        "data: {}\n\n",
                        json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
                    ));
                }
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                serde_json::from_slice::<Value>(&data).unwrap()
            });
            let s = router.snapshot().unwrap();
            let cid = s.get("agent_runs", &rid).unwrap().data["conversation_id"].clone();
            let mut seed = artex_domain::Planner::new(&s, "side.partial.profile", &json!({}));
            let pid=seed.insert("llm_profiles","profile",json!({"name":"Fixture","model":"fixture","format":"openai","base_url":format!("http://{address}"),"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"is_default":true,"streaming":true,"retry_stream_attempts":2,"retry_stream_interval_ms":1})).unwrap();
            router.internal_commit(seed.done(json!({}))).unwrap();
            let s = router.snapshot().unwrap();
            let plan = artex_domain::side_questions::checkpoint_plan(
                &s,
                &parent_context,
                &rid,
                1,
                s.get("llm_profiles", &pid).unwrap(),
                &json!({"system":"frozen","messages":[],"tools":[]}),
            )
            .unwrap();
            router.internal_commit(plan).unwrap();
            router.attach_host(Arc::new(Credentials));
            let routes = artex_core::contracts::read("legacy-routes.v1.json");
            let route = routes["routes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| {
                    r["handler"] == "handleSideQuestions"
                        && r["parent_kind"] == "conversation"
                        && r["legacy_http_method"] == "POST"
                })
                .unwrap()["route_id"]
                .clone();
            let plan=artex_domain::side_questions::plan("legacy.handleSideQuestions",&json!({"route_id":route,"path":{"id":cid},"query":{},"body":{"question":"What evidence is available?","client_request_id":"partial"}}),router.snapshot().unwrap().as_ref(),&router.side_context()).unwrap();
            let id = plan.result["id"].as_str().unwrap().to_owned();
            router.internal_commit(plan).unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                router.process_side_questions().unwrap();
                if router
                    .snapshot()
                    .unwrap()
                    .get("side_question_requests", &id)
                    .unwrap()
                    .data["status"]
                    != "running"
                {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let request = server.join().unwrap();
            assert!(request["tools"].as_array().unwrap().is_empty());
            router.store.as_mut().unwrap().checkpoint().unwrap();
            router.store.take();
            router.store = Some(Store::open(&dir.path().join("store")).unwrap());
            let s = router.snapshot().unwrap();
            let row = s.get("side_question_requests", &id).unwrap();
            assert_eq!(
                row.data["status"],
                if interrupted { "failed" } else { "completed" }
            );
            assert_eq!(row.data["answer"]["text"], "observed partial evidence");
            assert_eq!(row.data["usage"]["input_tokens"], 17);
            assert_eq!(row.data["usage"]["output_tokens"], 3);
            assert_eq!(row.data["usage"]["cache_read_tokens"], 7);
            assert!(s.records("tool_invocations").is_empty());
            if interrupted {
                assert!(
                    row.data["error"]
                        .as_str()
                        .unwrap()
                        .contains("Incomplete OpenAI stream")
                );
            }
        }
    }
}
