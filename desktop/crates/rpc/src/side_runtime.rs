//! Tool-free side questions use their admitted checkpoint and own cancellation.
use super::*;
use artex_domain::{record, text};
use artex_runtime::{Cancellation, Message, Prompt, Turn};
use std::{sync::mpsc, time::Duration};
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
    result: Result<Turn>,
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
            self.side.pending.remove(&done.id);
            if done.workspace != self.snapshot()?.workspace_id || done.epoch != self.engine_epoch {
                continue;
            }
            let (answer, status, error, usage) = match done.result {
                Ok(turn) => (
                    artex_core::body(&turn.text),
                    "completed",
                    String::new(),
                    turn.usage,
                ),
                Err(e) => (
                    artex_core::body(""),
                    if e.code == "CANCELLED" {
                        "cancelled"
                    } else {
                        "failed"
                    },
                    format!("{}: {}", e.code, e.message),
                    json!({}),
                ),
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
                None,
            )?;
            if !plan.changes.is_empty() {
                self.internal_commit(plan)?;
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
            let prepared = (|| {
                let input = artex_domain::side_questions::execution_input(&s, &id)?;
                let mut profile = record(&s, "llm_profiles", text(&input, "profile_id"))?
                    .data
                    .clone();
                profile["max_tokens"] = json!(8192);
                let context_bytes = serde_json::to_vec(&input["snapshot"]["request"])?;
                let budget = profile["context_window_k"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .unwrap_or(200)
                    .clamp(32, 1000)
                    * 1000
                    * 3;
                if context_bytes.len() as u64 > budget {
                    return Err(Error::new(
                        "SIZE_LIMIT",
                        "Side-question checkpoint exceeds the captured model context window",
                    ));
                }
                let system = "Answer the user's side question in Korean or English using the read-only parent checkpoint. This is an independent conversation: do not execute tools, alter the parent, or claim new work was performed.";
                let mut messages = vec![Message {
                    role: "user".into(),
                    content: vec![
                        json!({"type":"text","text":format!("Read-only parent checkpoint:\n{}",String::from_utf8(context_bytes).map_err(|_|Error::new("CORRUPT_STORE","Checkpoint is not UTF-8"))?)}),
                    ],
                }];
                for exchange in input["history"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    messages.push(Message {
                        role: "user".into(),
                        content: vec![json!({"type":"text","text":exchange["question"]})],
                    });
                    messages.push(Message {
                        role: "assistant".into(),
                        content: vec![
                            json!({"type":"text","text":self.body_text(&exchange["answer"])?}),
                        ],
                    });
                }
                messages.push(Message {
                    role: "user".into(),
                    content: vec![json!({"type":"text","text":input["question"]})],
                });
                let info = json!({"phase":"answering","recent_exchanges":input["history"].as_array().map(|a|a.len().min(20)).unwrap_or(0),"history_summarized":false,"snapshot_summarized":false});
                Ok((
                    profile,
                    Prompt {
                        system: system.into(),
                        messages,
                        tools: vec![],
                        session_id: id.clone(),
                    },
                    info,
                ))
            })();
            let (profile, prompt, info) = match prepared {
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
                let result = (|| {
                    let secret = gateway.request(
                        &workspace,
                        &context,
                        "secret.resolve_user",
                        json!({"secret_ref":profile["api_key"]["secret_ref"]}),
                        Duration::from_secs(10),
                    )?;
                    cancel.check()?;
                    artex_runtime::provider::generate(
                        &profile,
                        secret["value"].as_str().ok_or_else(|| {
                            Error::new(
                                "SECRET_UNAVAILABLE",
                                "Captured model credential is unavailable",
                            )
                        })?,
                        &prompt,
                        &cancel,
                        Duration::from_secs(120).saturating_sub(started.elapsed()),
                    )
                })();
                let _ = tx.send(Finished {
                    id,
                    workspace,
                    epoch,
                    generation,
                    context: info,
                    result,
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
}
