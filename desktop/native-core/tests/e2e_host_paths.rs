//! Public native commands, real NativeGateway/HostBridge, local provider/HTTP,
//! durable result blobs, history queries and workspace reopen in one test.
use artex_core::{Context, Expected, Result, new_id};
use artex_host::SecretVault;
use artex_native_core::{
    NativeAction, NativeCommand, NativeQuery, NativeReply, NativeTicket, OpenMode, OpenOptions,
    Service, ServiceConfig, ServiceHandle, gateway::NativeGateway,
};
use artex_runtime::HostGateway;
use base64::Engine;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

// Only secret resolution is a memory fixture; all tool work and retirement go
// through the production gateway. No OS credential entries are created.
struct FixtureSecretGateway {
    native: NativeGateway,
    secret: String,
    tool_calls: Arc<Mutex<Vec<String>>>,
    pty_ready: Option<Arc<Mutex<Vec<Value>>>>,
}
impl HostGateway for FixtureSecretGateway {
    fn request(
        &self,
        workspace: &str,
        ctx: &Context,
        op: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if op == "secret.resolve" && params["secret_ref"] == self.secret {
            return Ok(json!({"value":"local-fixture-placeholder"}));
        }
        if op == "tool.execute" {
            self.tool_calls
                .lock()
                .unwrap()
                .push(params["name"].as_str().unwrap().to_owned());
        }
        let opened = op == "tool.execute" && params["name"] == "shell_open";
        let result = self.native.request(workspace, ctx, op, params, timeout)?;
        if opened && let Some(receipts) = &self.pty_ready {
            // Pace only this fixture's provider turn using real host observations.
            // Keep the original shell_open return value, and never forge cleanup.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let inspected = self.native.request(
                    workspace,
                    ctx,
                    "run.inspect",
                    json!({}),
                    Duration::from_secs(5),
                )?;
                let output = self.native.request(workspace, ctx, "tool.execute", json!({"name":"shell_read","input":{"session_id":result["session_id"],"since_cursor":0,"max_bytes":65536,"max_lines":10000}}), Duration::from_secs(5))?;
                if inspected["cleanup_confirmed"] == true
                    && output["output"]
                        .as_str()
                        .unwrap_or("")
                        .contains("PTY_TAIL_SENTINEL")
                {
                    receipts.lock().unwrap().push(inspected);
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(artex_core::Error::new(
                        "TIMEOUT",
                        "Fixture PTY output/cleanup was not observed",
                    ));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
        Ok(result)
    }
    fn retire(&self, timeout: Duration) -> Result<()> {
        self.native.retire(timeout)
    }
    fn retirement_report(&self, workspace: &str, contexts: &[Context]) -> Result<Option<Value>> {
        self.native.retirement_report(workspace, contexts)
    }
}
fn wait(h: &ServiceHandle, ticket: NativeTicket) -> Result<NativeReply> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for done in h.drain_completions() {
            if done.ticket == ticket {
                return done.result;
            }
        }
        assert!(
            Instant::now() < deadline,
            "native command timeout: {:?}",
            h.status()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn action(h: &ServiceHandle, a: NativeAction) -> Result<NativeReply> {
    wait(h, h.try_submit(a)?)
}
fn query(h: &ServiceHandle, q: NativeQuery) -> NativeReply {
    action(
        h,
        NativeAction::Query {
            scope: h.scope().unwrap(),
            query: q,
        },
    )
    .unwrap()
}
fn command(h: &ServiceHandle, method: &str, params: Value) -> NativeReply {
    let prepare = query(
        h,
        NativeQuery::Prepare {
            method: method.into(),
            params: params.clone(),
        },
    );
    let expected: Vec<Expected> =
        serde_json::from_value(prepare.value["expected"].clone()).unwrap();
    action(
        h,
        NativeAction::Command {
            scope: h.scope().unwrap(),
            command: NativeCommand::new(method, params, expected),
        },
    )
    .unwrap()
}
fn request(stream: &mut TcpStream) -> Value {
    // Windows can inherit the listener's nonblocking mode. A partial request
    // header is ordinary transport timing, not a failed provider invocation.
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
    let header = String::from_utf8(header).unwrap();
    let len = header
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap();
    assert!(len < 2 * 1024 * 1024);
    let mut bytes = vec![0; len];
    stream.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn reply(stream: &mut TcpStream, media: &str, body: &str) {
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
}
fn result_text(h: &ServiceHandle, reference: &Value) -> String {
    let reply = query(
        h,
        NativeQuery::Blob {
            sha256: reference["sha256"].as_str().unwrap().into(),
            offset: 0,
            length: 65536,
        },
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(reply.value["data_base64"].as_str().unwrap())
        .unwrap();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    query(
        h,
        NativeQuery::Body {
            body: envelope["content"][0]["text"].clone(),
        },
    )
    .value
    .as_str()
    .unwrap()
    .to_owned()
}
fn history(h: &ServiceHandle, conversation: &str) -> String {
    let rows = query(
        h,
        NativeQuery::Feature {
            method: "ui.conversationMessages".into(),
            params: json!({"id":conversation}),
        },
    )
    .value;
    let mut texts = vec![];
    for row in rows["items"].as_array().unwrap() {
        // The native conversation pane first loads summary rows, then fetches
        // each selected message body through this public detail endpoint.
        let detail = query(
            h,
            NativeQuery::Feature {
                method: "ui.conversationMsgDetail".into(),
                params: json!({"id":conversation,"seq":row["seq"]}),
            },
        )
        .value;
        texts.push(
            query(h, NativeQuery::Body { body: detail })
                .value
                .as_str()
                .unwrap()
                .to_owned(),
        );
        texts.push(row.to_string());
    }
    texts.join("\n")
}

#[test]
fn public_native_conversation_continuation_host_results_persist_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace");
    let profile = dir.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    let secret = new_id("secret");
    let factory_secret = secret.clone();
    let factory_profile = profile.clone();
    let vault = Arc::new(SecretVault::default());
    let tool_calls = Arc::new(Mutex::new(Vec::new()));
    let factory_calls = tool_calls.clone();
    let service = Service::spawn(ServiceConfig {
        host_factory: Some(Arc::new(move |workspace| {
            Ok(Arc::new(FixtureSecretGateway {
                native: NativeGateway::open_with_profile(
                    workspace,
                    vault.clone(),
                    &factory_profile,
                )?,
                secret: factory_secret.clone(),
                tool_calls: factory_calls.clone(),
                pty_ready: None,
            }))
        })),
        wake_ui: None,
    })
    .unwrap();
    let h = service.handle();
    action(
        &h,
        NativeAction::Open(OpenOptions {
            path: path.clone(),
            mode: OpenMode::Create {
                name: "Host E2E fixture".into(),
            },
            read_only: false,
        }),
    )
    .unwrap();
    let page = TcpListener::bind("127.0.0.1:0").unwrap();
    let page_url = format!("http://{}/page", page.local_addr().unwrap());
    page.set_nonblocking(true).unwrap();
    let page_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match page.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(10)))
                        .unwrap();
                    let mut buf = [0; 4096];
                    stream.read(&mut buf).unwrap();
                    let html = format!(
                        "<p>PAGE_SENTINEL</p><script>/*{}*/const p='/api/ENDPOINT_SENTINEL'</script><form method='post' action='/submit'><input type='hidden' name='csrf' value='TOKEN_SENTINEL'></form><!-- COMMENT_SENTINEL -->",
                        "x".repeat(60000)
                    );
                    reply(&mut stream, "Text/HTML; charset=UTF-8", &html);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "page never requested");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    let provider = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", provider.local_addr().unwrap());
    provider.set_nonblocking(true).unwrap();
    let file_path = Arc::new(Mutex::new(String::new()));
    let provider_path = file_path.clone();
    let provider_thread = thread::spawn(move || {
        let mut seen = vec![];
        let deadline = Instant::now() + Duration::from_secs(90);
        while seen.len() < 3 {
            let (mut stream, _) = loop {
                match provider.accept() {
                    Ok(v) => break v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "provider missing request; seen{}",
                            seen.len()
                        );
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            seen.push(request(&mut stream));
            let response = match seen.len() {
                1 => {
                    json!({"choices":[{"message":{"content":"PARTIAL_SENTINEL"},"finish_reason":"length"}],"usage":{"prompt_tokens":5,"completion_tokens":2}})
                }
                2 => {
                    json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"read-offset-fixture","type":"function","function":{"name":"Read","arguments":json!({"file_path":provider_path.lock().unwrap().clone(),"offset":2,"limit":1}).to_string()}},{"id":"webfetch-recon-fixture","type":"function","function":{"name":"WebFetch","arguments":json!({"url":page_url,"extract":true}).to_string()}},{"id":"webfetch-failure-fixture","type":"function","function":{"name":"WebFetch","arguments":"{\"url\":\"file:///fixture-not-http\"}"}},{"id":"shell-list-empty-fixture","type":"function","function":{"name":"shell_list","arguments":""}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":8,"completion_tokens":3}})
                }
                _ => {
                    json!({"choices":[{"message":{"content":"FINAL_SENTINEL"},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":4}})
                }
            };
            reply(&mut stream, "application/json", &response.to_string());
        }
        seen
    });
    command(
        &h,
        "ui.saveLLMProfile",
        json!({"p":{"name":"Local E2E provider","format":"openai","model":"fixture","base_url":endpoint,"api_key":{"secret_ref":secret,"hint":"fixture"},"streaming":false}}),
    );
    let profile_id = h.view().unwrap().generation.records("llm_profiles")[0]
        .header
        .id
        .clone();
    let conversation = command(
        &h,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"Host E2E","llm_profile_id":profile_id}),
    );
    let id = conversation.value["id"].as_str().unwrap().to_owned();
    let picked = profile.join("long.txt");
    std::fs::write(&picked, format!("{}\nREAD_SENTINEL\n", "x".repeat(70000))).unwrap();
    let staged = action(
        &h,
        NativeAction::StageFile {
            scope: h.scope().unwrap(),
            path: picked,
        },
    )
    .unwrap();
    let uploaded = command(
        &h,
        "ui.chatUpload",
        json!({"scope":"session","id":id,"files":[staged.value["file_capability"]]}),
    );
    let attachments = uploaded.value["attachments"].clone();
    *file_path.lock().unwrap() = attachments[0]["path"].as_str().unwrap().to_owned();
    command(
        &h,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    command(
        &h,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"interactive_shell":true}}),
    );
    command(
        &h,
        "ui.sendConversationMessage",
        json!({"id":id,"message":"Use the authorized local fixtures only.","attachments":attachments}),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let view = h.view().unwrap();
        let runs = view.generation.records("agent_runs");
        assert!(
            !runs.iter().any(|r| r.data["conversation_id"] == id
                && matches!(
                    r.data["state"].as_str(),
                    Some("failed" | "recovery_required")
                )),
            "run failed: {:?}",
            runs
        );
        if runs
            .iter()
            .any(|r| r.data["conversation_id"] == id && r.data["state"] == "completed")
        {
            break;
        }
        assert!(Instant::now() < deadline, "run did not finish: {:?}", runs);
        thread::sleep(Duration::from_millis(10));
    }
    page_thread.join().unwrap();
    let requests = provider_thread.join().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1].to_string().contains("PARTIAL_SENTINEL"),
        "continuation omitted committed partial text"
    );
    assert!(requests[2].to_string().contains("READ_SENTINEL"));
    assert!(requests[2].to_string().contains("ENDPOINT_SENTINEL"));
    let invocations = h
        .view()
        .unwrap()
        .generation
        .records("tool_invocations")
        .into_iter()
        .map(|r| (r.header.id.clone(), r.data.clone()))
        .collect::<Vec<_>>();
    assert_eq!(invocations.len(), 4);
    assert_eq!(
        tool_calls
            .lock()
            .unwrap()
            .iter()
            .filter(|name| *name == "shell_list")
            .count(),
        1
    );
    let mut preserved = vec![];
    for (invocation, data) in &invocations {
        let text = result_text(&h, &data["result_ref"]);
        match data["tool_use_id"].as_str().unwrap() {
            "read-offset-fixture" => {
                assert_eq!(data["state"], "completed");
                assert!(text.contains("READ_SENTINEL"));
            }
            "webfetch-recon-fixture" => {
                assert_eq!(data["state"], "completed");
                for sentinel in [
                    "PAGE_SENTINEL",
                    "ENDPOINT_SENTINEL",
                    "TOKEN_SENTINEL",
                    "COMMENT_SENTINEL",
                ] {
                    assert!(text.contains(sentinel), "missing {sentinel}");
                }
                assert!(text.len() < 2000);
            }
            "webfetch-failure-fixture" => {
                assert_eq!(data["state"], "failed");
                assert!(text.contains("INVALID_PARAMS"));
            }
            "shell-list-empty-fixture" => {
                assert_eq!(data["state"], "completed");
                assert!(text.contains("sessions"));
                assert_eq!(
                    data["input"],
                    json!({}),
                    "empty no-argument tool input is normalized before dispatch"
                );
            }
            other => panic!("unexpected tool {other}"),
        }
        preserved.push((invocation.clone(), data["result_ref"].clone(), text));
    }
    let before = history(&h, &id);
    for sentinel in [
        "PARTIAL_SENTINEL",
        "FINAL_SENTINEL",
        "READ_SENTINEL",
        "ENDPOINT_SENTINEL",
        "INVALID_PARAMS",
    ] {
        assert!(before.contains(sentinel), "history missing {sentinel}");
    }
    action(
        &h,
        NativeAction::Command {
            scope: h.scope().unwrap(),
            command: NativeCommand::checkpoint(),
        },
    )
    .unwrap();
    let old_scope = h.scope().unwrap();
    action(
        &h,
        NativeAction::Close {
            scope: old_scope.clone(),
        },
    )
    .unwrap();
    action(
        &h,
        NativeAction::Open(OpenOptions {
            path,
            mode: OpenMode::Existing,
            read_only: true,
        }),
    )
    .unwrap();
    assert_ne!(h.scope().unwrap().engine_epoch, old_scope.engine_epoch);
    for (invocation, reference, text) in preserved {
        let view = h.view().unwrap();
        let record = view
            .generation
            .get("tool_invocations", &invocation)
            .unwrap();
        assert_eq!(record.data["result_ref"], reference);
        assert_eq!(result_text(&h, &reference), text);
    }
    let after = history(&h, &id);
    assert_eq!(
        tool_calls.lock().unwrap().len(),
        4,
        "reopen must not replay host effects"
    );
    for sentinel in [
        "PARTIAL_SENTINEL",
        "FINAL_SENTINEL",
        "READ_SENTINEL",
        "ENDPOINT_SENTINEL",
        "INVALID_PARAMS",
    ] {
        assert!(after.contains(sentinel));
    }
    action(&h, NativeAction::Shutdown).unwrap();
    service.join().unwrap();
}

// Unix process groups intentionally do not prove escaped-descendant containment.
// This close/reopen test requires the Windows Job/ConPTY cleanup proof; it must
// never upgrade the existing macOS/Arch unconfirmed-cleanup boundary.
#[cfg(windows)]
#[test]
fn v09_public_native_pty_returned_session_drains_persists_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workspace");
    let profile = dir.path().join("profile");
    std::fs::create_dir(&profile).unwrap();
    let secret = new_id("secret");
    let factory_secret = secret.clone();
    let factory_profile = profile.clone();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let factory_calls = calls.clone();
    let receipts = Arc::new(Mutex::new(Vec::new()));
    let factory_receipts = receipts.clone();
    let vault = Arc::new(SecretVault::default());
    let service = Service::spawn(ServiceConfig {
        host_factory: Some(Arc::new(move |workspace| {
            Ok(Arc::new(FixtureSecretGateway {
                native: NativeGateway::open_with_profile(
                    workspace,
                    vault.clone(),
                    &factory_profile,
                )?,
                secret: factory_secret.clone(),
                tool_calls: factory_calls.clone(),
                pty_ready: Some(factory_receipts.clone()),
            }))
        })),
        wake_ui: None,
    })
    .unwrap();
    let h = service.handle();
    action(
        &h,
        NativeAction::Open(OpenOptions {
            path: path.clone(),
            mode: OpenMode::Create {
                name: "PTY public-path fixture".into(),
            },
            read_only: false,
        }),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let provider = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(90);
        while requests.len() < 3 {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(value) => break value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "PTY provider missing request");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            let value = request(&mut stream);
            let response = match requests.len() {
                0 => {
                    json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"pty-open-fixture","type":"function","function":{"name":"shell_open","arguments":json!({"command":"[Console]::Write(('X' * 20000) + 'PTY_TAIL_SENTINEL')","mode":"hands-free","cols":200}).to_string()}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":2}})
                }
                1 => {
                    let returned = value["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|m| m["role"] == "tool" && m["tool_call_id"] == "pty-open-fixture")
                        .unwrap();
                    let opened: Value =
                        serde_json::from_str(returned["content"].as_str().unwrap()).unwrap();
                    json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"pty-drain-fixture","type":"function","function":{"name":"shell_read","arguments":json!({"session_id":opened["session_id"],"since_cursor":0,"drain":true,"max_bytes":10,"max_lines":1}).to_string()}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2}})
                }
                _ => {
                    json!({"choices":[{"message":{"content":"PTY_FINAL_SENTINEL"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":2}})
                }
            };
            requests.push(value);
            reply(&mut stream, "application/json", &response.to_string());
        }
        requests
    });
    command(
        &h,
        "ui.saveLLMProfile",
        json!({"p":{"name":"PTY local provider","format":"openai","model":"fixture","base_url":endpoint,"api_key":{"secret_ref":secret,"hint":"fixture"},"streaming":false}}),
    );
    let profile_id = h.view().unwrap().generation.records("llm_profiles")[0]
        .header
        .id
        .clone();
    let created = command(
        &h,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"PTY E2E","llm_profile_id":profile_id}),
    );
    let id = created.value["id"].as_str().unwrap().to_owned();
    command(
        &h,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
    );
    command(
        &h,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"interactive_shell":true}}),
    );
    command(
        &h,
        "ui.sendConversationMessage",
        json!({"id":id,"message":"Use the isolated terminal fixture only.","attachments":[]}),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let view = h.view().unwrap();
        let runs = view.generation.records("agent_runs");
        assert!(
            !runs.iter().any(|r| r.data["conversation_id"] == id
                && matches!(
                    r.data["state"].as_str(),
                    Some("failed" | "recovery_required")
                )),
            "PTY run failed: {runs:?}"
        );
        if runs
            .iter()
            .any(|r| r.data["conversation_id"] == id && r.data["state"] == "completed")
        {
            break;
        }
        assert!(Instant::now() < deadline, "PTY run deadline: {runs:?}");
        thread::sleep(Duration::from_millis(10));
    }
    let requests = provider.join().unwrap();
    assert_eq!(requests.len(), 3);
    let tool_messages = requests[2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["shell_open", "shell_read"],
        "returned session must pass the public next-tool contract: {tool_messages:?}"
    );
    let view = h.view().unwrap();
    let invocations = view.generation.records("tool_invocations");
    assert_eq!(invocations.len(), 2);
    let opened = invocations
        .iter()
        .find(|r| r.data["name"] == "shell_open")
        .unwrap();
    let opened_body: Value =
        serde_json::from_str(&result_text(&h, &opened.data["result_ref"])).unwrap();
    assert!(artex_core::valid_id(
        opened_body["session_id"].as_str().unwrap(),
        "shell"
    ));
    assert!(artex_core::valid_id(
        opened.data["execution_id"].as_str().unwrap(),
        "exec"
    ));
    assert_ne!(opened_body["session_id"], opened.data["execution_id"]);
    let drained = invocations
        .iter()
        .find(|r| r.data["name"] == "shell_read")
        .unwrap();
    assert_eq!(drained.data["state"], "completed");
    assert_eq!(
        drained.data["input"]["session_id"],
        opened_body["session_id"]
    );
    let reference = drained.data["result_ref"].clone();
    let raw = result_text(&h, &reference);
    let output: Value = serde_json::from_str(&raw).unwrap();
    assert!(
        output["output"]
            .as_str()
            .unwrap()
            .contains("PTY_TAIL_SENTINEL")
    );
    assert!(output["output"].as_str().unwrap().len() > 8192);
    assert_eq!(output["omitted"], "0");
    assert_eq!(
        output["cursor"].as_str().unwrap().parse::<usize>().unwrap(),
        output["output"].as_str().unwrap().len()
    );
    assert!(history(&h, &id).contains("PTY_TAIL_SENTINEL"));
    assert_eq!(receipts.lock().unwrap().len(), 1);
    assert_eq!(receipts.lock().unwrap()[0]["cleanup_confirmed"], true);
    assert_eq!(
        receipts.lock().unwrap()[0]["executions"][0]["execution_id"],
        opened.data["execution_id"]
    );
    let invocation = drained.header.id.clone();
    drop(view);
    action(
        &h,
        NativeAction::Command {
            scope: h.scope().unwrap(),
            command: NativeCommand::checkpoint(),
        },
    )
    .unwrap();
    action(
        &h,
        NativeAction::Close {
            scope: h.scope().unwrap(),
        },
    )
    .unwrap();
    action(
        &h,
        NativeAction::Open(OpenOptions {
            path,
            mode: OpenMode::Existing,
            read_only: true,
        }),
    )
    .unwrap();
    assert_eq!(
        h.view()
            .unwrap()
            .generation
            .get("tool_invocations", &invocation)
            .unwrap()
            .data["result_ref"],
        reference
    );
    assert_eq!(result_text(&h, &reference), raw);
    assert!(history(&h, &id).contains("PTY_TAIL_SENTINEL"));
    assert_eq!(*calls.lock().unwrap(), vec!["shell_open", "shell_read"]);
    action(&h, NativeAction::Shutdown).unwrap();
    service.join().unwrap();
}

// Count only message bodies: summary labels can intentionally repeat the same
// words in the native list and are not a second observed provider output.
fn history_bodies(h: &ServiceHandle, conversation: &str) -> String {
    let rows = query(
        h,
        NativeQuery::Feature {
            method: "ui.conversationMessages".into(),
            params: json!({"id":conversation}),
        },
    )
    .value;
    rows["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let detail = query(
                h,
                NativeQuery::Feature {
                    method: "ui.conversationMsgDetail".into(),
                    params: json!({"id":conversation,"seq":row["seq"]}),
                },
            )
            .value;
            query(h, NativeQuery::Body { body: detail })
                .value
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// A Turn-only decoder check cannot prove that named Responses events survive
// public admission, host dispatch, stored tool envelopes, and native detail reads.
// Terminal output intentionally omits the observed call: only SSE deltas supply it.
#[test]
fn responses_named_sse_blank_tool_arguments_reach_host_and_survive_reopen() {
    for (case, terminal_output, arguments, should_fail) in [
        ("missing-output", None, "", false),
        ("empty-output", Some(json!([])), "", false),
        ("malformed-arguments", Some(json!([])), "{\"path\":", true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace");
        let profile = dir.path().join("profile");
        std::fs::create_dir(&profile).unwrap();
        let secret = new_id("secret");
        let factory_secret = secret.clone();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let factory_calls = calls.clone();
        let vault = Arc::new(SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |workspace| {
                Ok(Arc::new(FixtureSecretGateway {
                    native: NativeGateway::open_with_profile(workspace, vault.clone(), &profile)?,
                    secret: factory_secret.clone(),
                    tool_calls: factory_calls.clone(),
                    pty_ready: None,
                }))
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        action(
            &h,
            NativeAction::Open(OpenOptions {
                path: path.clone(),
                mode: OpenMode::Create {
                    name: format!("Responses {case}"),
                },
                read_only: false,
            }),
        )
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop_provider, provider_stopped) = std::sync::mpsc::channel();
        let provider = thread::spawn(move || {
            let mut requests = vec![];
            let deadline = Instant::now() + Duration::from_secs(60);
            'serving: loop {
                let (mut stream, _) = loop {
                    if provider_stopped.try_recv().is_ok() {
                        break 'serving;
                    }
                    match listener.accept() {
                        Ok(value) => break value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "{case}: missing provider request"
                            );
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                let request = request(&mut stream);
                assert_eq!(request["stream"], true);
                let mut terminal =
                    json!({"status":"completed","usage":{"input_tokens":9,"output_tokens":4}});
                if let Some(output) = &terminal_output {
                    terminal["output"] = output.clone();
                }
                // No payload has a `type` discriminator. The real decoder must
                // use SSE event names and assemble the real observed fragments.
                let events = if requests.is_empty() {
                    vec![
                        (
                            "response.output_text.delta",
                            json!({"output_index":0,"delta":"RESPONSES_OBSERVED_SENTINEL"}),
                        ),
                        (
                            "response.output_item.added",
                            json!({"output_index":1,"item":{"type":"function_call","call_id":"responses-shell-list","name":"shell_list","arguments":""}}),
                        ),
                        (
                            "response.function_call_arguments.delta",
                            json!({"output_index":1,"delta":arguments}),
                        ),
                        ("response.completed", json!({"response":terminal})),
                    ]
                } else {
                    vec![
                        (
                            "response.output_text.delta",
                            json!({"output_index":0,"delta":"RESPONSES_FINAL_SENTINEL"}),
                        ),
                        ("response.completed", json!({"response":terminal})),
                    ]
                };
                let body = events
                    .into_iter()
                    .map(|(event, data)| format!("event: {event}\ndata: {data}\n\n"))
                    .collect::<String>();
                reply(&mut stream, "text/event-stream", &body);
                requests.push(request);
            }
            requests
        });
        command(
            &h,
            "ui.saveLLMProfile",
            json!({"p":{"name":format!("Responses {case}"),"format":"openai-responses","model":"fixture","base_url":endpoint,"api_key":{"secret_ref":secret,"hint":"fixture"},"streaming":true}}),
        );
        let profile_id = h.view().unwrap().generation.records("llm_profiles")[0]
            .header
            .id
            .clone();
        let created = command(
            &h,
            "ui.createConversation",
            json!({"agent_key":"auto","title":format!("Responses {case}"),"llm_profile_id":profile_id}),
        );
        let id = created.value["id"].as_str().unwrap().to_owned();
        command(
            &h,
            "ui.setSettings",
            json!({"patch":{"approval_bypass":true}}),
        );
        command(
            &h,
            "ui.saveAgentConfig",
            json!({"key":"auto","patch":{"interactive_shell":true}}),
        );
        command(
            &h,
            "ui.sendConversationMessage",
            json!({"id":id,"message":"Use only the local shell_list fixture.","attachments":[]}),
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let view = h.view().unwrap();
            let runs = view.generation.records("agent_runs");
            if should_fail {
                if runs
                    .iter()
                    .any(|r| r.data["conversation_id"] == id && r.data["state"] == "failed")
                {
                    break;
                }
                assert!(
                    !runs
                        .iter()
                        .any(|r| r.data["conversation_id"] == id && r.data["state"] == "completed"),
                    "malformed arguments must not complete"
                );
            } else {
                assert!(
                    !runs.iter().any(|r| r.data["conversation_id"] == id
                        && matches!(
                            r.data["state"].as_str(),
                            Some("failed" | "recovery_required")
                        )),
                    "{case}: {runs:?}"
                );
                if runs
                    .iter()
                    .any(|r| r.data["conversation_id"] == id && r.data["state"] == "completed")
                {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "{case}: run deadline: {runs:?}");
            thread::sleep(Duration::from_millis(10));
        }
        stop_provider.send(()).unwrap();
        let requests = provider.join().unwrap();
        let view = h.view().unwrap();
        let invocations = view.generation.records("tool_invocations");
        let preserved = if should_fail {
            assert_eq!(
                requests.len(),
                1,
                "observed malformed call must not be regenerated"
            );
            assert!(
                calls.lock().unwrap().is_empty(),
                "invalid arguments reached host effect"
            );
            assert!(
                invocations.is_empty(),
                "invalid call became an executable invocation"
            );
            None
        } else {
            assert_eq!(requests.len(), 2);
            assert_eq!(*calls.lock().unwrap(), vec!["shell_list"]);
            assert_eq!(invocations.len(), 1);
            let invocation = invocations[0];
            assert_eq!(invocation.data["tool_use_id"], "responses-shell-list");
            assert_eq!(invocation.data["name"], "shell_list");
            assert_eq!(invocation.data["input"], json!({}));
            assert_eq!(invocation.data["state"], "completed");
            let result = result_text(&h, &invocation.data["result_ref"]);
            let parsed: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(parsed["sessions"], json!([]));
            // This is the real follow-up provider request, not a fabricated Turn
            // snapshot. It must include both the call and its actual host result.
            let input = requests[1]["input"].as_array().unwrap();
            let outputs = input
                .iter()
                .filter(|v| {
                    v["type"] == "function_call_output" && v["call_id"] == "responses-shell-list"
                })
                .collect::<Vec<_>>();
            assert_eq!(outputs.len(), 1);
            let returned: Value =
                serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
            assert_eq!(returned["sessions"], json!([]));
            Some((
                invocation.header.id.clone(),
                invocation.data["result_ref"].clone(),
                result,
            ))
        };
        drop(view);
        let before = history_bodies(&h, &id);
        assert_eq!(
            before.matches("RESPONSES_OBSERVED_SENTINEL").count(),
            1,
            "observed text duplicated or lost in native detail Body: {before}"
        );
        if !should_fail {
            assert_eq!(before.matches("RESPONSES_FINAL_SENTINEL").count(), 1);
            assert!(
                before.contains("sessions"),
                "real tool body absent from native history"
            );
        }
        action(
            &h,
            NativeAction::Command {
                scope: h.scope().unwrap(),
                command: NativeCommand::checkpoint(),
            },
        )
        .unwrap();
        let old_scope = h.scope().unwrap();
        action(
            &h,
            NativeAction::Close {
                scope: old_scope.clone(),
            },
        )
        .unwrap();
        action(
            &h,
            NativeAction::Open(OpenOptions {
                path,
                mode: OpenMode::Existing,
                read_only: true,
            }),
        )
        .unwrap();
        assert_ne!(h.scope().unwrap().engine_epoch, old_scope.engine_epoch);
        if let Some((invocation, reference, result)) = preserved {
            assert_eq!(
                h.view()
                    .unwrap()
                    .generation
                    .get("tool_invocations", &invocation)
                    .unwrap()
                    .data["result_ref"],
                reference
            );
            assert_eq!(result_text(&h, &reference), result);
        } else {
            assert!(
                h.view()
                    .unwrap()
                    .generation
                    .records("tool_invocations")
                    .is_empty()
            );
        }
        assert_eq!(
            history_bodies(&h, &id),
            before,
            "native visible history changed on read-only reopen"
        );
        assert_eq!(
            calls.lock().unwrap().len(),
            usize::from(!should_fail),
            "reopen replayed a host effect"
        );
        action(&h, NativeAction::Shutdown).unwrap();
        service.join().unwrap();
        println!(
            "Responses Service E2E {case}: provider={} host={} Blob/detail Body/checkpoint/read-only reopen PASS",
            requests.len(),
            calls.lock().unwrap().len()
        );
    }
}
