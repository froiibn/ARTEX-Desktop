//! Fixed local fixtures exercise source behavior without sites, credentials or
//! a running application. The OS still owns all fixture subprocess lifetimes.
use artex_core::{Actor, Context, new_id};
use artex_host::{HostBridge, HostRuntime, SecretVault};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn context() -> Context {
    Context {
        actor: Actor {
            kind: "agent".into(),
            agent_id: Some(new_id("agent")),
            run_id: Some(new_id("run")),
            engine_epoch: new_id("epoch"),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    }
}
fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_sessions.py")
}
fn python() -> &'static str {
    if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    }
}
fn stdio(cap: &str) -> Value {
    json!({"directory_capability":cap,"server":{"name":"fixture","transport":"stdio","command":python(),"args":[fixture()],"env":{"SESSION_LABEL":"initial"}},"tool_name":"counter","arguments":{}})
}
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < end, "fixture did not reach terminal state");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn mcp_stdio_reuses_run_session_serializes_and_retires_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let host = Arc::new(HostRuntime::default());
    let ctx = context();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let input = stdio(&cap);
    assert_eq!(
        artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
        1
    );
    assert_eq!(
        artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
        2
    );
    let calls: Vec<_> = (0..2)
        .map(|_| {
            let h = host.clone();
            let i = input.clone();
            let c = ctx.clone();
            thread::spawn(move || {
                artex_host::mcp::call(&h, &i, &c).unwrap()["counter"]
                    .as_u64()
                    .unwrap()
            })
        })
        .collect();
    let mut counters: Vec<_> = calls.into_iter().map(|t| t.join().unwrap()).collect();
    counters.sort();
    assert_eq!(counters, vec![3, 4]);
    let mut other = ctx.clone();
    other.actor.run_id = Some(new_id("run"));
    assert_eq!(
        artex_host::mcp::call(&host, &input, &other).unwrap()["counter"],
        1
    );
    let mut epoch = ctx.clone();
    epoch.actor.engine_epoch = new_id("epoch");
    assert_eq!(
        artex_host::mcp::call(&host, &input, &epoch).unwrap()["counter"],
        1
    );
    let mut changed = input.clone();
    changed["server"]["env"]["SESSION_LABEL"] = json!("changed");
    let result = artex_host::mcp::call(&host, &changed, &ctx).unwrap();
    assert_eq!(result["counter"], 1);
    assert_eq!(result["label"], "changed");
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert_eq!(
        artex_host::mcp::call(&host, &changed, &ctx)
            .unwrap_err()
            .code,
        "CANCELLED"
    );
    let inspected = host.inspect_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert!(
        inspected["executions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["leader_exited"] == true && e["output_drained"] == true)
    );
    assert_eq!(inspected["cleanup_confirmed"], cfg!(windows));
    host.stop_epoch(&other.actor.engine_epoch);
    host.stop_epoch(&epoch.actor.engine_epoch);
}
#[test]
fn retained_mcp_inflight_cancellation_does_not_retry_or_leave_leader() {
    let directory = tempfile::tempdir().unwrap();
    let host = Arc::new(HostRuntime::default());
    let ctx = context();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let mut input = stdio(&cap);
    artex_host::mcp::call(&host, &input, &ctx).unwrap();
    input["arguments"] = json!({"delay":true});
    let h = host.clone();
    let c = ctx.clone();
    let worker = thread::spawn(move || artex_host::mcp::call(&h, &input, &c));
    thread::sleep(Duration::from_millis(100));
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
    let status = host.inspect_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert_eq!(
        status["executions"].as_array().unwrap().len(),
        1,
        "cancellation must not replay the request"
    );
    assert_eq!(status["executions"][0]["leader_exited"], true);
    assert_eq!(status["executions"][0]["output_drained"], true);
}
#[test]
fn ui_mcp_commands_release_their_temporary_process_after_each_call() {
    let directory = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let mut ctx = context();
    ctx.actor.kind = "user".into();
    ctx.actor.agent_id = None;
    ctx.actor.run_id = None;
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let mut input = stdio(&cap);
    let command = new_id("command");
    input["command_id"] = json!(command);
    // HostBridge also attaches this UI owner; it must not become an agent pool.
    ctx.actor.run_id = Some(format!("ui_{command}"));
    for _ in 0..2 {
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            1
        );
    }
    let status = host.inspect_run(&format!("ui_{command}"), &ctx.actor.engine_epoch);
    assert_eq!(status["executions"].as_array().unwrap().len(), 2);
    assert!(
        status["executions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["leader_exited"] == true && e["output_drained"] == true)
    );
    assert_eq!(status["cleanup_confirmed"], cfg!(windows));
}
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn remote_fixture() -> (Server, String) {
    let mut command = Command::new(python());
    command
        .arg(fixture())
        .arg("http")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut server = Server(command.spawn().unwrap());
    let mut port = String::new();
    BufReader::new(server.0.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    (server, format!("http://127.0.0.1:{}/rpc", port.trim()))
}
#[test]
fn legacy_sse_keeps_state_after_real_idle_beyond_response_deadline() {
    let (_server, url) = remote_fixture();
    let host = HostRuntime::default();
    let ctx = context();
    let input = |transport: &str| json!({"server":{"name":transport,"transport":transport,"url":url},"tool_name":"counter","arguments":{}});
    for transport in ["sse", "http"] {
        assert_eq!(
            artex_host::mcp::call(&host, &input(transport), &ctx).unwrap()["counter"],
            1
        );
    }
    // A response wait is bounded at 30 s; an idle retained GET is not a wait
    // for an outstanding call and must not inherit that lifetime deadline.
    thread::sleep(Duration::from_secs(32));
    for transport in ["sse", "http"] {
        assert_eq!(
            artex_host::mcp::call(&host, &input(transport), &ctx).unwrap()["counter"],
            2
        );
    }
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
}
#[test]
fn legacy_sse_missing_call_response_still_has_finite_deadline() {
    let (_server, url) = remote_fixture();
    let host = HostRuntime::default();
    let ctx = context();
    let mut input = json!({"server":{"name":"sse","transport":"sse","url":url},"tool_name":"counter","arguments":{}});
    assert_eq!(
        artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
        1
    );
    input["arguments"] = json!({"no_response":true});
    let started = Instant::now();
    assert_eq!(
        artex_host::mcp::call(&host, &input, &ctx).unwrap_err().code,
        "TIMEOUT"
    );
    assert!(started.elapsed() < Duration::from_secs(35));
    input["arguments"] = json!({});
    assert_eq!(
        artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
        1,
        "uncertain timed-out call must retire its connection"
    );
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
}
#[test]
fn webfetch_compacts_layout_before_budget_and_marks_real_utf8_truncation() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let evidence = "IMPORTANT_VISIBLE_EVIDENCE";
    let pages = vec![
        format!("<html><body><p>{evidence}</p></body></html>"),
        format!("<html><body>{}<p>{evidence}</p></body></html>", "<div></div>".repeat(13_000)),
        "<div>alpha \t  beta <a href='/target?q=1'>target</a></div><pre><code>  indented\tcode\n    next line  </code></pre>".into(),
        format!("<div>{}</div>", "한".repeat(30_000)),
    ];
    let server = thread::spawn(move || {
        for body in pages {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let host = HostRuntime::default();
    let ctx = context();
    for case in 0..4 {
        let result = host
            .execute_tool(
                "WebFetch",
                &json!({"url":format!("http://{address}/page")}),
                &ctx,
            )
            .unwrap();
        let content = result["content"].as_str().unwrap();
        if case <= 1 {
            assert_eq!(content, evidence);
        } else if case == 2 {
            assert!(
                content.contains("alpha beta [target](/target?q=1)"),
                "{content}"
            );
            assert!(
                content.contains("  indented\tcode\n    next line  "),
                "{content}"
            );
        } else {
            assert!(content.len() <= 50_000);
            assert!(content.ends_with("... [truncated]"));
            assert!(content.contains('한'));
        }
    }
    server.join().unwrap();
}
#[test]
fn rpc_errors_keep_session_but_malformed_responses_retire_without_retry() {
    let directory = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let ctx = context();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let (_server, url) = remote_fixture();
    for transport in ["stdio", "http", "sse"] {
        let mut input = if transport == "stdio" {
            stdio(&cap)
        } else {
            json!({"server":{"name":transport,"transport":transport,"url":url},"tool_name":"counter","arguments":{}})
        };
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            1
        );
        input["arguments"] = json!({"rpc_error":true});
        let error = artex_host::mcp::call(&host, &input, &ctx).unwrap_err();
        assert_eq!(error.details["rpc_code"], -32602);
        input["arguments"] = json!({});
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            2
        );
        input["arguments"] = json!({"malformed":true});
        assert!(artex_host::mcp::call(&host, &input, &ctx).is_err());
        input["arguments"] = json!({});
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            1
        );
        if transport == "stdio" {
            input["arguments"] = json!({"disconnect":true});
            assert!(artex_host::mcp::call(&host, &input, &ctx).is_err());
            input["arguments"] = json!({});
            assert_eq!(
                artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
                1
            );
            assert_eq!(
                host.inspect_run(
                    ctx.actor.run_id.as_deref().unwrap(),
                    &ctx.actor.engine_epoch
                )["executions"]
                    .as_array()
                    .unwrap()
                    .len(),
                3,
                "errors must not automatically replay an action"
            );
        }
    }
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
}
#[test]
fn retained_http_and_sse_inflight_calls_cancel_without_retry() {
    for transport in ["http", "sse"] {
        let (_server, url) = remote_fixture();
        let host = Arc::new(HostRuntime::default());
        let ctx = context();
        let mut input = json!({"server":{"name":"fixture","transport":transport,"url":url},"tool_name":"counter","arguments":{}});
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            1
        );
        input["arguments"] = json!({"delay":true});
        let h = host.clone();
        let c = ctx.clone();
        let request = input.clone();
        let worker = thread::spawn(move || artex_host::mcp::call(&h, &request, &c));
        thread::sleep(Duration::from_millis(100));
        host.stop_run(
            ctx.actor.run_id.as_deref().unwrap(),
            &ctx.actor.engine_epoch,
        );
        assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap_err().code,
            "CANCELLED"
        );
        // Cancelling the connection is not proof that a remote tool undid its
        // effect. These checks only establish local cancellation/no new request.
    }
}
#[test]
fn http_and_sse_retain_protocol_session_between_calls() {
    for transport in ["http", "sse"] {
        let (_server, url) = remote_fixture();
        let host = HostRuntime::default();
        let ctx = context();
        let input = json!({"server":{"name":"remote-fixture","transport":transport,"url":url},"tool_name":"counter","arguments":{}});
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            1
        );
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap()["counter"],
            2
        );
        host.stop_run(
            ctx.actor.run_id.as_deref().unwrap(),
            &ctx.actor.engine_epoch,
        );
        assert_eq!(
            artex_host::mcp::call(&host, &input, &ctx).unwrap_err().code,
            "CANCELLED"
        );
    }
}
#[test]
fn write_creates_nested_parents_and_keeps_capability_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let ctx = context();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let input = json!({"directory_capability":cap,"file_path":"reports/new/report.md","content":"evidence"});
    host.execute_tool("Write", &input, &ctx).unwrap();
    assert_eq!(
        std::fs::read_to_string(directory.path().join("reports/new/report.md")).unwrap(),
        "evidence"
    );
    host.execute_tool("Write", &input, &ctx).unwrap();
    for path in [
        "../outside/report.md",
        if cfg!(windows) {
            "C:\\outside\\report.md"
        } else {
            "/outside/report.md"
        },
    ] {
        let mut bad = input.clone();
        bad["file_path"] = json!(path);
        assert_eq!(
            host.execute_tool("Write", &bad, &ctx).unwrap_err().code,
            "PERMISSION_DENIED"
        );
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), directory.path().join("linked")).unwrap();
        let mut bad = input;
        bad["file_path"] = json!("linked/new/report.md");
        assert_eq!(
            host.execute_tool("Write", &bad, &ctx).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        assert!(!outside.path().join("new").exists());
    }
}
#[cfg(windows)]
#[test]
fn nested_write_rejects_existing_junction_escape() {
    use std::os::windows::process::CommandExt;
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let link = directory.path().join("linked");
    let output = Command::new("cmd.exe")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Could not create isolated junction fixture"
    );
    let host = HostRuntime::default();
    let ctx = context();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let result = host.execute_tool("Write", &json!({"directory_capability":cap,"file_path":"linked/new/report.md","content":"must not escape"}), &ctx);
    // Remove only this junction, never recurse through its target.
    std::fs::remove_dir(&link).unwrap();
    assert_eq!(result.unwrap_err().code, "PERMISSION_DENIED");
    assert!(!outside.path().join("new").exists());
}
#[test]
fn webfetch_keeps_error_page_body_status_fingerprints_and_visible_links() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        for code in [403, 200] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            stream.read(&mut request).unwrap();
            let body = "<html><body><div>div evidence <span>span evidence</span></div><p>paragraph <a href='/target'>target</a></p><a href='/standalone'>standalone</a><script>hidden script text</script></body></html>";
            write!(stream,"HTTP/1.1 {code} Fixture\r\nContent-Type: text/html\r\nServer: fixture-server\r\nX-Powered-By: fixture-runtime\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    let host = HostRuntime::default();
    let ctx = context();
    for code in [403, 200] {
        let result = host
            .execute_tool(
                "WebFetch",
                &json!({"url":format!("http://{address}/page")}),
                &ctx,
            )
            .unwrap();
        assert_eq!(result["status"], code);
        assert_eq!(result["server"], "fixture-server");
        assert_eq!(result["x_powered_by"], "fixture-runtime");
        let content = result["content"].as_str().unwrap();
        for evidence in [
            "div evidence",
            "span evidence",
            "[target](/target)",
            "[standalone](/standalone)",
        ] {
            assert!(content.contains(evidence), "{content}");
        }
        assert!(!content.contains("hidden script text"));
    }
    worker.join().unwrap();
    assert_eq!(
        host.execute_tool("WebFetch", &json!({"url":"file:///not-http"}), &ctx)
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
}
#[test]
fn background_bash_and_monitor_emit_once_with_scope_and_foreground_control() {
    let directory = tempfile::tempdir().unwrap();
    let bridge = HostBridge::new(Arc::new(SecretVault::default()));
    let ctx = context();
    let workspace = new_id("workspace");
    bridge.bind(&ctx.actor.engine_epoch, &workspace).unwrap();
    let host = &bridge.runtime;
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    for (name, code) in [("Bash", 0), ("Monitor", 7)] {
        let command = if cfg!(windows) {
            format!("Write-Output PRIVATE_OUTPUT; exit {code}")
        } else {
            format!("printf PRIVATE_OUTPUT; exit {code}")
        };
        let value=host.execute_tool(name,&json!({"directory_capability":cap,"command":command,"run_in_background":true,"description":"fixture completion"}),&ctx).unwrap();
        let id = value["task_id"].as_str().unwrap();
        wait(|| {
            host.executions
                .status(
                    id,
                    ctx.actor.run_id.as_deref().unwrap(),
                    &ctx.actor.engine_epoch,
                )
                .unwrap()
                .state
                != "running"
        });
    }
    let foreground = if cfg!(windows) {
        "Write-Output foreground"
    } else {
        "printf foreground"
    };
    host.execute_tool(
        "Bash",
        &json!({"directory_capability":cap,"command":foreground}),
        &ctx,
    )
    .unwrap();
    assert!(
        host.executions
            .drain_events("another-run", &ctx.actor.engine_epoch)
            .is_empty()
    );
    assert!(
        host.executions
            .drain_events(ctx.actor.run_id.as_deref().unwrap(), "another-epoch")
            .is_empty()
    );
    let result = bridge
        .execute_direct(&workspace, &ctx, "host.events", json!({}))
        .unwrap();
    let events = result["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().any(|e| e["status"] == "completed"));
    assert!(events.iter().any(|e| e["status"] == "failed"));
    assert!(!result.to_string().contains("PRIVATE_OUTPUT"));
    assert!(
        bridge
            .execute_direct(&workspace, &ctx, "host.events", json!({}))
            .unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    host.stop_epoch(&ctx.actor.engine_epoch);
}
