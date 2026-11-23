//! Actual NativeApp host-path regressions, as a child of app_workflow_e2e_tests.
//! Uses real NativeApp input/poll/effects and public Service -> NativeGateway.
//! Fixture inputs are isolated files/loopback HTTP. No provider/tool results are
//! injected into App projections, and no OS key store operations are permitted.
use super::Fixture;
use super::transport::{LocalProvider, Probe, Step, factory};
use artex_core::new_id;
#[cfg(windows)]
use artex_core::{Context, Error, Result};
#[cfg(windows)]
use artex_native_core::HostFactory;
use artex_native_core::ServiceConfig;
#[cfg(windows)]
use artex_runtime::HostGateway;
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

// Use the shared, independently checked row-ink/glyph clipping helper so
// font descender padding cannot diverge between provider and host UI evidence.
// A complete galley string is never accepted as viewport visibility evidence.
fn actually_visible_text(output: &egui::FullOutput, area: egui::Rect) -> String {
    super::visible_texts(output, area).join("\n")
}
fn settled_wheel(f: &mut Fixture, events: Vec<egui::Event>) -> egui::FullOutput {
    // Scroll offsets are applied across actual repaint frames. Observing only
    // the input frame then immediately reversing direction can skip the top.
    let mut output = f.frame(events);
    let signature = |out: &egui::FullOutput| {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some((
                    format!("{:?}", t.visual_bounding_rect()),
                    t.galley.text().chars().take(80).collect::<String>(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut same = 0;
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(10));
        let next = f.frame(vec![]);
        if signature(&next) == signature(&output) {
            same += 1
        } else {
            same = 0
        }
        output = next;
        if same >= 2 {
            break;
        }
    }
    output
}
fn assert_drawer_marker(f: &mut Fixture, mut output: egui::FullOutput, marker: &str) {
    // Scroll actual widgets when a correct stored field is initially below the
    // viewport. Never call a full galley or offscreen source field visible.
    let area = f.app.drawer.body_rect.expect("detail drawer must be open");
    if actually_visible_text(&output, area).contains(marker) {
        return;
    }
    output = settled_wheel(
        f,
        vec![
            egui::Event::PointerMoved(area.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0., 10000.),
                modifiers: Default::default(),
            },
        ],
    );
    for step in 0..80 {
        let area = f.app.drawer.body_rect.unwrap();
        if step < 5 {
            println!(
                "HOST_SCROLL {marker} step={step} visible={:?}",
                super::visible_texts(&output, area)
            );
        }
        if actually_visible_text(&output, area).contains(marker) {
            return;
        }
        output = settled_wheel(
            f,
            vec![
                egui::Event::PointerMoved(area.center()),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -area.height() * 0.5),
                    modifiers: Default::default(),
                },
            ],
        );
    }
    let area = f.app.drawer.body_rect.unwrap();
    let shapes=output.shapes.iter().filter_map(|c|match &c.shape {egui::epaint::Shape::Text(t) if t.galley.text().contains(marker)=>Some(json!({"text_head":t.galley.text().chars().take(200).collect::<String>(),"clip":format!("{:?}",c.clip_rect),"bounds":format!("{:?}",t.visual_bounding_rect()),"marker_rows":t.galley.rows.iter().filter(|r|r.text().contains("PTY")||r.text().contains("HEAD")||r.text().contains("SENTINEL")).map(|r|json!({"text":r.text(),"ink":format!("{:?}",r.visuals.mesh_bounds.translate(t.pos.to_vec2()+r.pos.to_vec2()))})).collect::<Vec<_>>()})),_=>None}).collect::<Vec<_>>();
    panic!(
        "Stored field did not become visibly readable by real drawer scrolling: {marker}; area={area:?}, visible={:?}, relevant={shapes:?}",
        super::visible_texts(&output, area)
    );
}
fn clicked_output(f: &mut Fixture, label: &str) -> egui::FullOutput {
    let mut output = f.frame(vec![]);
    let mut found = None;
    // Real wheel input scrolls the actual drawer; this bounded search does not
    // edit ScrollArea state or claim a clipped button was clicked. Half a
    // viewport preserves overlap so a 120px drawer cannot skip a short button.
    for _ in 0..80 {
        found = output.shapes.iter().rev().find_map(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else {
                return None;
            };
            let bounds = text.visual_bounding_rect();
            (text.galley.text() == label && clipped.clip_rect.contains_rect(bounds))
                .then_some(bounds.center())
        });
        if found.is_some() {
            break;
        }
        let area = f.app.drawer.body_rect.expect("source drawer must be open");
        let pointer = area.center();
        output = settled_wheel(
            f,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -area.height() * 0.5),
                    modifiers: Default::default(),
                },
            ],
        );
    }
    let pos = found.unwrap_or_else(|| panic!("Actual visible button missing: {label}"));
    f.frame(vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        },
    ]);
    f.frame(vec![egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Default::default(),
    }])
}
fn wait_completed(f: &mut Fixture, conversation: &str) {
    f.until(|app| {
        let view = app.handle.view().unwrap();
        let runs = view.generation.records("agent_runs");
        assert!(
            !runs
                .iter()
                .any(|r| r.data["conversation_id"] == conversation
                    && matches!(
                        r.data["state"].as_str(),
                        Some("failed" | "recovery_required")
                    )),
            "Actual App run failed: {runs:?}"
        );
        runs.iter()
            .any(|r| r.data["conversation_id"] == conversation && r.data["state"] == "completed")
    });
}
fn configure_conversation(f: &mut Fixture, endpoint: &str, secret: &str, title: &str) -> String {
    let profile=f.setup("ui.saveLLMProfile",json!({"p":{"name":title,"format":"openai","model":"fixture","base_url":endpoint,"api_key":{"secret_ref":secret,"hint":"memory-only fixture"},"streaming":false}}))["id"].as_str().unwrap().to_owned();
    f.setup("ui.setSettings", json!({"patch":{"approval_bypass":true}}));
    f.setup(
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"interactive_shell":true}}),
    );
    f.setup(
        "ui.createConversation",
        json!({"agent_key":"auto","title":title,"llm_profile_id":profile}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn call(id: &str, name: &str, arguments: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})
}
fn chat_tools(calls: Vec<Value>) -> Step {
    Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":null,"tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}).to_string())
}
fn chat_answer(text: &str) -> Step {
    Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":6,"completion_tokens":2}}).to_string())
}
fn assert_offered(request: &Value, names: &[&str]) {
    let tools = request["tools"]
        .as_array()
        .expect("actual provider request must offer tool definitions");
    for name in names {
        let matching = tools
            .iter()
            .filter(|tool| tool["type"] == "function" && tool["function"]["name"] == *name)
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "actual provider request must offer {name} exactly once"
        );
        assert_eq!(
            matching[0]["function"]["parameters"]["type"], "object",
            "offered tool must carry its actual input schema"
        );
    }
}
fn actual_tool_reply(request: &Value, call: &str) -> Value {
    let messages = request["messages"].as_array().unwrap();
    let replies = messages
        .iter()
        .filter(|m| m["role"] == "tool" && m["tool_call_id"] == call)
        .collect::<Vec<_>>();
    assert_eq!(
        replies.len(),
        1,
        "Actual follow-up provider request must have one result for {call}"
    );
    serde_json::from_str(replies[0]["content"].as_str().unwrap()).unwrap_or_else(|e| {
        panic!(
            "Actual tool reply {call} is not JSON ({e}): {}",
            replies[0]["content"]
        )
    })
}
fn displayed_detail_result(f: &Fixture, conversation: &str) -> Value {
    // This read-only seam observes only the real query/Body completion already
    // merged by NativeApp. It never seeds a result or hydrates an app projection.
    let body = f
        .app
        .source_operations
        .query_result_for_app_test("ui.conversationMsgDetail", Some(conversation))
        .expect("actual detail query has completed");
    assert!(body["text"].is_string() && body["blob"].is_null());
    serde_json::from_str(body["text"].as_str().unwrap())
        .expect("actual public tool detail must be original result JSON")
}
fn activity_for_tool(f: &Fixture, conversation: &str, call: &str) -> String {
    let view = f.app.handle.view().unwrap();
    let rows = view
        .generation
        .records("conversation_activities")
        .into_iter()
        .filter(|r| {
            r.data["conversation_id"] == conversation
                && r.data["kind"] == "tool_result"
                && r.data["tool_use_id"] == call
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "Exactly one persisted tool activity must match actual model call"
    );
    rows[0].header.id.clone()
}
fn ledger(f: &Fixture, conversation: &str) -> Vec<(String, Value)> {
    let view = f.app.handle.view().unwrap();
    view.generation
        .records("tool_invocations")
        .into_iter()
        .filter(|r| {
            r.data["conversation_id"] == conversation
                || view
                    .generation
                    .get("agent_runs", r.data["run_id"].as_str().unwrap())
                    .is_some_and(|run| run.data["conversation_id"] == conversation)
        })
        .map(|r| {
            assert_eq!(r.data["state"], "completed");
            assert!(
                r.data["result_ref"]["sha256"]
                    .as_str()
                    .is_some_and(|s| s.len() == 64)
            );
            (r.header.id.clone(), r.data.clone())
        })
        .collect()
}
fn assert_ledger_unchanged(f: &Fixture, records: &[(String, Value)]) {
    let view = f.app.handle.view().unwrap();
    for (id, data) in records {
        assert_eq!(
            view.generation.get("tool_invocations", id).unwrap().data,
            *data
        );
    }
}

// An ordinary GET fixture differs from the JSON POST LocalProvider. It serves
// original input HTML only; it does not synthesize a WebFetch tool result.
struct LocalPage {
    url: String,
    count: Arc<Mutex<usize>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl LocalPage {
    fn recon() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/recon", listener.local_addr().unwrap());
        let count = Arc::new(Mutex::new(0));
        let counted = count.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(90);
            while !stopped.load(Ordering::Acquire) {
                assert!(Instant::now() < deadline, "page fixture exceeded bound");
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(10)))
                            .unwrap();
                        let mut header = vec![];
                        let mut byte = [0];
                        while !header.ends_with(b"\r\n\r\n") {
                            stream.read_exact(&mut byte).unwrap();
                            header.push(byte[0]);
                            assert!(header.len() < 16384);
                        }
                        assert!(
                            String::from_utf8(header)
                                .unwrap()
                                .starts_with("GET /recon HTTP/1.1\r\n")
                        );
                        *counted.lock().unwrap() += 1;
                        let body = format!(
                            "<html><body>PAGE_SENTINEL<script>/*{}*/ const endpoint='/ENDPOINT_SENTINEL';</script><form action='/submit'><input type='hidden' name='csrf' value='TOKEN_SENTINEL'></form><!--COMMENT_SENTINEL--></body></html>",
                            "padding".repeat(9000)
                        );
                        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: Text/HTML\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            count,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for LocalPage {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[test]
fn actual_app_composer_read_offset_recon_mixed_mime_and_shell_list_detail_reopen() {
    let secret = new_id("secret");
    let probe = Probe::default();
    let work = Arc::new(Mutex::new(None::<PathBuf>));
    let opened_work = work.clone();
    let mut f = Fixture::new_with_config(|profile| {
        let base = factory(
            profile,
            BTreeMap::from([(secret.clone(), "local-fixture-placeholder".into())]),
            probe.clone(),
        );
        ServiceConfig {
            host_factory: Some(Arc::new(move |workspace| {
                *opened_work.lock().unwrap() = Some(workspace.work_root.clone());
                base(workspace)
            })),
            wake_ui: None,
        }
    });
    let file_path = Arc::new(Mutex::new(None::<PathBuf>));
    let model_file = file_path.clone();
    let page = LocalPage::recon();
    let endpoint = page.url.clone();
    let provider = LocalProvider::from_handler(move |index, _| match index {
        0 => chat_tools(vec![
            call(
                "actual-read",
                "Read",
                json!({"file_path":model_file.lock().unwrap().as_ref().unwrap(),"offset":2,"limit":1}),
            ),
            call(
                "actual-webfetch",
                "WebFetch",
                json!({"url":endpoint,"extract":true}),
            ),
            json!({"id":"actual-list","type":"function","function":{"name":"shell_list","arguments":""}}),
        ]),
        1 => chat_answer("HOST_PATH_FINAL_SENTINEL"),
        _ => panic!("Unexpected provider request after confirmed tools"),
    });
    let cid = configure_conversation(&mut f, &provider.url, &secret, "Actual host UI path");
    // host::scope_input grants the conversation child, never the execution
    // root as fallback. Derive the real input path from the opened root + real
    // public conversation ID. This does not simulate file staging or uploading.
    let directory = work
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .join("conversations")
        .join(&cid);
    std::fs::create_dir_all(&directory).unwrap();
    let file = directory.join("long-prefix.txt");
    std::fs::write(&file, format!("{}\nREAD_SENTINEL\n", "x".repeat(70000))).unwrap();
    // The agent already owns this scoped CWD; absolute/parent paths remain forbidden.
    *file_path.lock().unwrap() = Some(PathBuf::from("long-prefix.txt"));
    f.type_composer_and_send(&cid, "Use only these isolated local fixture tools.");
    wait_completed(&mut f, &cid);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_offered(&requests[0], &["Read", "WebFetch", "shell_list"]);
    let read = actual_tool_reply(&requests[1], "actual-read");
    assert!(read.to_string().contains("READ_SENTINEL"));
    let web = actual_tool_reply(&requests[1], "actual-webfetch");
    for marker in [
        "PAGE_SENTINEL",
        "ENDPOINT_SENTINEL",
        "TOKEN_SENTINEL",
        "COMMENT_SENTINEL",
    ] {
        assert!(
            web.to_string().contains(marker),
            "compact recon lost {marker}"
        );
    }
    assert!(
        web.to_string().len() < 3000,
        "raw inline filler crowded out compact signals"
    );
    assert_eq!(
        actual_tool_reply(&requests[1], "actual-list")["sessions"],
        json!([])
    );
    assert_eq!(*page.count.lock().unwrap(), 1);
    let attempts = probe.attempts.lock().unwrap().clone();
    assert_eq!(
        attempts.len(),
        3,
        "Count attempts before delegation, including any failures"
    );
    let records = ledger(&f, &cid);
    assert_eq!(records.len(), 3);
    assert!(
        records
            .iter()
            .any(|(_, r)| r["name"] == "shell_list" && r["input"] == json!({}))
    );
    f.assert_main_visible(&cid, "HOST_PATH_FINAL_SENTINEL");
    let details = [
        (
            activity_for_tool(&f, &cid, "actual-read"),
            vec!["READ_SENTINEL"],
            read.clone(),
        ),
        (
            activity_for_tool(&f, &cid, "actual-webfetch"),
            vec![
                "PAGE_SENTINEL",
                "ENDPOINT_SENTINEL",
                "TOKEN_SENTINEL",
                "COMMENT_SENTINEL",
            ],
            web.clone(),
        ),
        (
            activity_for_tool(&f, &cid, "actual-list"),
            vec!["sessions"],
            actual_tool_reply(&requests[1], "actual-list"),
        ),
    ];
    for (seq, markers, expected) in &details {
        for marker in markers {
            let output = f.source_body_detail(&cid, seq);
            assert_eq!(displayed_detail_result(&f, &cid), *expected);
            assert_drawer_marker(&mut f, output, marker);
        }
    }
    f.reopen_same_workspace();
    assert!(
        f.app.handle.view().unwrap().workspace.read_only,
        "reopen helper must use Existing read_only mode"
    );
    f.assert_main_visible(&cid, "HOST_PATH_FINAL_SENTINEL");
    assert_ledger_unchanged(&f, &records);
    for (seq, markers, expected) in details {
        for marker in markers {
            let output = f.source_body_detail(&cid, &seq);
            assert_eq!(displayed_detail_result(&f, &cid), expected);
            assert_drawer_marker(&mut f, output, marker);
        }
    }
    assert_eq!(
        probe.attempts.lock().unwrap().len(),
        3,
        "Readonly reopen replayed host effect"
    );
    f.shutdown();
    assert_eq!(
        provider.finish().len(),
        2,
        "detail UI/read-only reopen/shutdown made extra provider requests"
    );
}

// Preserve the real platform cleanup boundary: Windows Job/ConPTY proves
// terminal containment. An unconfined Unix group is never marked confirmed here.
#[cfg(windows)]
struct PacePtyGateway {
    inner: Arc<dyn HostGateway>,
    observations: Arc<Mutex<Vec<Value>>>,
}
#[cfg(windows)]
impl HostGateway for PacePtyGateway {
    fn request(
        &self,
        workspace: &str,
        ctx: &Context,
        op: &str,
        input: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let opened = op == "tool.execute" && input["name"] == "shell_open";
        let execution_id = input["execution_id"].clone();
        let result = self.inner.request(workspace, ctx, op, input, timeout)?;
        if opened {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                // These are real observer requests on the original run/epoch.
                // Common Probe still records each tool attempt before delegate;
                // the observer flag exists only in our evidence, not product args.
                self.observations
                    .lock()
                    .unwrap()
                    .push(json!({"phase":"before","op":"run.inspect"}));
                let inspected = self.inner.request(
                    workspace,
                    ctx,
                    "run.inspect",
                    json!({}),
                    Duration::from_secs(5),
                )?;
                // A read cursor ahead of current output is correctly rejected.
                // Wait for the matching real execution's retained range before
                // making an observer read; never swallow that error or invent
                // a ready output. The original run/epoch and bound still apply.
                let retained = inspected["executions"].as_array().and_then(|rows| {
                    rows.iter()
                        .find(|r| r["kind"] == "pty" && r["execution_id"] == execution_id)
                });
                let ready = retained.is_some_and(|r| {
                    let first = r["first_cursor"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok());
                    let last = r["last_cursor"]
                        .as_str()
                        .and_then(|s| s.parse::<u64>().ok());
                    matches!((first,last),(Some(first),Some(last)) if first<=65000 && last>=65000)
                });
                if ready {
                    self.observations.lock().unwrap().push(json!({"phase":"before","op":"tool.execute","name":"shell_read","input":{"session_id":result["session_id"],"since_cursor":65000,"max_bytes":65536,"max_lines":10000}}));
                    // The tail is >64KiB away. A non-drain read leaves the next
                    // model read's explicit since0/drain ownership unchanged.
                    let output=self.inner.request(workspace,ctx,"tool.execute",json!({"name":"shell_read","input":{"session_id":result["session_id"],"since_cursor":65000,"max_bytes":65536,"max_lines":10000}}),Duration::from_secs(5))?;
                    self.observations
                        .lock()
                        .unwrap()
                        .push(json!({"phase":"after","inspect":inspected,"output":output}));
                    if inspected["cleanup_confirmed"] == true
                        && output["output"]
                            .as_str()
                            .unwrap_or("")
                            .contains("PTY_TAIL_SENTINEL")
                    {
                        break;
                    }
                } else {
                    self.observations
                        .lock()
                        .unwrap()
                        .push(json!({"phase":"not-ready","inspect":inspected}));
                }
                if Instant::now() >= deadline {
                    return Err(Error::new(
                        "TIMEOUT",
                        "Actual fixture PTY output/cleanup not observed",
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(result)
    }
    fn retire(&self, timeout: Duration) -> Result<()> {
        self.inner.retire(timeout)
    }
    fn retirement_report(&self, workspace: &str, contexts: &[Context]) -> Result<Option<Value>> {
        self.inner.retirement_report(workspace, contexts)
    }
}
#[cfg(windows)]
#[test]
fn actual_app_pty_drain_blob_preview_copy_cleanup_and_reopen() {
    let secret = new_id("secret");
    let probe = Probe::default();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let factory_observations = observations.clone();
    let mut f = Fixture::new_with_config(|profile| {
        let base: HostFactory = factory(
            profile,
            BTreeMap::from([(secret.clone(), "local-fixture-placeholder".into())]),
            probe.clone(),
        );
        ServiceConfig {
            host_factory: Some(Arc::new(move |workspace| {
                Ok(Arc::new(PacePtyGateway {
                    inner: base(workspace)?,
                    observations: factory_observations.clone(),
                }))
            })),
            wake_ui: None,
        }
    });
    let provider = LocalProvider::from_handler(|index, request| match index {
        0 => chat_tools(vec![call(
            "actual-open",
            "shell_open",
            json!({"command":"[Console]::Write('PTY_HEAD_SENTINEL' + ('X' * 70000) + 'PTY_TAIL_SENTINEL')","mode":"hands-free","cols":200}),
        )]),
        1 => {
            let opened = actual_tool_reply(request, "actual-open");
            assert!(artex_core::valid_id(
                opened["session_id"].as_str().unwrap(),
                "shell"
            ));
            chat_tools(vec![call(
                "actual-drain",
                "shell_read",
                json!({"session_id":opened["session_id"],"since_cursor":0,"drain":true,"max_bytes":10,"max_lines":1}),
            )])
        }
        2 => chat_answer("PTY_FINAL_VISIBLE_SENTINEL"),
        _ => panic!("Unexpected provider request would duplicate confirmed host work"),
    });
    let cid = configure_conversation(&mut f, &provider.url, &secret, "Actual PTY UI path");
    f.type_composer_and_send(&cid, "Use only the isolated PTY fixture.");
    wait_completed(&mut f, &cid);
    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert_offered(&requests[0], &["shell_open"]);
    assert_offered(&requests[1], &["shell_read"]);
    let opened = actual_tool_reply(&requests[1], "actual-open");
    // The model request view may lawfully shorten older tool results for
    // context pressure. NativeGateway's original result, never that outbound
    // view, is the full persisted/display/copied output oracle.
    let raw_results = probe
        .results
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["request"]["input"]["name"] == "shell_read"
                && entry["request"]["input"]["input"]["drain"] == true
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(raw_results.len(), 1);
    assert!(
        raw_results[0].get("error").is_none(),
        "actual drain failed: {raw_results:?}"
    );
    let drained = raw_results[0]["ok"].clone();
    let sent_results = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool" && m["tool_call_id"] == "actual-drain")
        .collect::<Vec<_>>();
    assert_eq!(sent_results.len(), 1);
    let sent = sent_results[0]["content"].as_str().unwrap();
    match serde_json::from_str::<Value>(sent) {
        Ok(value) => assert_eq!(value, drained),
        Err(_) => {
            // compression_policy::emergency_tool_view retains head/tail and
            // explicitly marks only the outbound view, leaving durable text.
            assert!(sent.contains("[truncated for context space"));
            assert!(sent.contains("PTY_HEAD_SENTINEL") && sent.contains("PTY_TAIL_SENTINEL"));
        }
    }
    let output = drained["output"].as_str().unwrap();
    assert!(output.len() > 65536);
    assert!(output.contains("PTY_HEAD_SENTINEL") && output.contains("PTY_TAIL_SENTINEL"));
    assert_eq!(
        drained["cursor"]
            .as_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        output.len()
    );
    assert_eq!(drained["omitted"], "0");
    let records = ledger(&f, &cid);
    assert_eq!(records.len(), 2);
    let open_record = &records
        .iter()
        .find(|(_, r)| r["name"] == "shell_open")
        .unwrap()
        .1;
    let read_record = &records
        .iter()
        .find(|(_, r)| r["name"] == "shell_read")
        .unwrap()
        .1;
    assert_ne!(open_record["execution_id"], opened["session_id"]);
    assert_eq!(read_record["input"]["session_id"], opened["session_id"]);
    assert!(
        observations
            .lock()
            .unwrap()
            .iter()
            .any(|o| o["inspect"]["cleanup_confirmed"] == true
                && o["inspect"]["executions"]
                    .as_array()
                    .is_some_and(|rows| rows
                        .iter()
                        .any(|r| r["execution_id"] == open_record["execution_id"])))
    );
    let attempts = probe.attempts.lock().unwrap().clone();
    assert_eq!(
        attempts
            .iter()
            .filter(|a| a["input"]["name"] == "shell_open")
            .count(),
        1
    );
    assert_eq!(
        attempts
            .iter()
            .filter(|a| a["input"]["name"] == "shell_read" && a["input"]["input"]["drain"] == true)
            .count(),
        1
    );
    let open_seq = activity_for_tool(&f, &cid, "actual-open");
    let seq = activity_for_tool(&f, &cid, "actual-drain");
    assert!(
        f.app
            .handle
            .view()
            .unwrap()
            .generation
            .get("conversation_activities", &seq)
            .unwrap()
            .data["detail"]["blob"]
            .is_object(),
        "real 70KiB result must exercise Blob detail loading"
    );
    for reopened in [false, true] {
        if reopened {
            f.reopen_same_workspace();
            assert!(
                f.app.handle.view().unwrap().workspace.read_only,
                "reopen helper must use Existing read_only mode"
            );
            assert_ledger_unchanged(&f, &records);
        }
        f.assert_main_visible(&cid, "PTY_FINAL_VISIBLE_SENTINEL");
        // Display the actual received ShellId through its own persisted open
        // activity, before opening the separate drain Body. A hardcoded prefix
        // or the shell_open input command cannot satisfy this received value.
        let opened_detail = f.source_body_detail(&cid, &open_seq);
        assert_eq!(displayed_detail_result(&f, &cid), opened);
        assert_drawer_marker(
            &mut f,
            opened_detail,
            opened["session_id"].as_str().unwrap(),
        );
        let preview = f.source_body_detail(&cid, &seq);
        assert_drawer_marker(&mut f, preview, "PTY_HEAD_SENTINEL");
        let preview = f.frame(vec![]);
        assert!(
            !actually_visible_text(
                &preview,
                f.app
                    .drawer
                    .body_rect
                    .expect("detail drawer must be visible")
            )
            .contains("PTY_TAIL_SENTINEL"),
            "tail outside 8192-char preview is not falsely called visible"
        );
        let copied = clicked_output(&mut f, "전체 텍스트 복사");
        let full = copied
            .platform_output
            .commands
            .iter()
            .find_map(|c| match c {
                egui::OutputCommand::CopyText(s) => Some(s),
                _ => None,
            })
            .expect("real copy button must emit platform CopyText");
        // Public transcript_ui::body projects the stored message envelope to
        // the original tool_result text. Compare that real public view rather
        // than expecting the private transcript envelope on the UI clipboard.
        let stored: Value = serde_json::from_str(full).unwrap();
        assert!(full.len() > 65536 && full.contains("PTY_TAIL_SENTINEL"));
        assert_eq!(
            stored, drained,
            "actual stored Body full-copy must equal original NativeGateway drain result, including tail"
        );
    }
    assert_eq!(
        probe.attempts.lock().unwrap().len(),
        attempts.len(),
        "readonly reopen or detail UI replayed host effects"
    );
    f.shutdown();
    assert_eq!(
        provider.finish().len(),
        3,
        "detail UI/read-only reopen/shutdown made extra provider requests"
    );
}
