use artex_core::{Actor, Context, new_id};
use artex_host::{ExecutionRequest, HostBridge, HostRuntime, SecretVault};
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
fn context(kind: &str) -> Context {
    Context {
        actor: Actor {
            kind: kind.into(),
            agent_id: if kind == "agent" {
                Some(new_id("agent"))
            } else {
                None
            },
            run_id: if kind == "agent" {
                Some(new_id("run"))
            } else {
                None
            },
            engine_epoch: new_id("epoch"),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    }
}
#[test]
fn actual_mcp_http_sse_and_network_cancellation() {
    let host = Arc::new(HostRuntime::default());
    let dir = tempfile::tempdir().unwrap();
    let ctx = context("agent");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/http_mcp.py");
    let execution = new_id("exec");
    let run = ctx.actor.run_id.as_ref().unwrap();
    host.executions
        .execute(ExecutionRequest {
            execution_id: execution.clone(),
            run_id: run.clone(),
            engine_epoch: ctx.actor.engine_epoch.clone(),
            executable: if cfg!(windows) {
                "python.exe".into()
            } else {
                "python3".into()
            },
            // The fixture must bind numeric loopback without reverse DNS.
            // A resolver stall/failure here must not affect MCP qualification.
            args: vec![
                "-u".into(),"-c".into(),
                "import runpy,socket,sys; socket.getfqdn=lambda *args: (_ for _ in ()).throw(RuntimeError('Loopback fixture must not resolve a host name')); runpy.run_path(sys.argv[1],run_name='__main__')".into(),
                fixture.to_string_lossy().into_owned(),
            ],
            cwd: dir.path().into(),
            timeout_ms: 45000,
        })
        .unwrap();
    // Process/import/readiness budget is independent of the measured network
    // cancellation deadline below. It does not make cancellation more lenient.
    let end = Instant::now() + Duration::from_secs(15);
    let port = loop {
        let s = host
            .executions
            .status(&execution, run, &ctx.actor.engine_epoch)
            .unwrap();
        if let Some(port) = s
            .output
            .lines()
            .find_map(|s| s.trim().parse::<u16>().ok().filter(|p| *p != 0))
        {
            break port;
        }
        assert_eq!(
            s.state, "running",
            "HTTP fixture exited before readiness: {s:?}"
        );
        assert!(Instant::now() < end, "Fixture port missing: {s:?}");
        thread::sleep(Duration::from_millis(10));
    };
    for transport in ["http", "sse"] {
        let input = json!({"server":{"transport":transport,"url":format!("http://127.0.0.1:{port}/{transport}")},"tool_name":"echo","arguments":{"text":format!("real-{transport}")}});
        let r = host.execute_tool("mcp.call", &input, &ctx).unwrap();
        assert_eq!(r["content"][0]["text"], format!("real-{transport}"));
    }
    let runtime = host.clone();
    let context = ctx.clone();
    let worker = thread::spawn(move || {
        runtime.execute_tool("mcp.call",&json!({"server":{"transport":"http","url":format!("http://127.0.0.1:{port}/http")},"tool_name":"echo","arguments":{"delay":true}}),&context)
    });
    let accepted_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = host
            .executions
            .status(&execution, run, &ctx.actor.engine_epoch)
            .unwrap();
        if status.output.lines().any(|line| line == "DELAY_ACCEPTED") {
            break;
        }
        assert!(
            !worker.is_finished(),
            "MCP call returned before the fixture accepted its delayed response"
        );
        assert!(
            Instant::now() < accepted_deadline,
            "HTTP fixture did not accept the delayed MCP request: {status:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let start = Instant::now();
    host.stop_run(run, &ctx.actor.engine_epoch);
    assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn actual_mcp_stdio_roundtrip_and_cancel() {
    let host = Arc::new(HostRuntime::default());
    let dir = tempfile::tempdir().unwrap();
    let cap = host.capabilities.grant_directory(dir.path()).unwrap();
    let context = context("agent");
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_server.py");
    let input = json!({"directory_capability":cap,"server":{"transport":"stdio","command":if cfg!(windows){"python.exe"}else{"python3"},"args":[fixture]},"tool_name":"echo","arguments":{"text":"real-pipe"}});
    let cache = artex_host::mcp::refresh(&host, &input, &context).unwrap();
    assert_eq!(cache[0]["name"], "echo");
    assert_eq!(cache[0]["input_schema"]["type"], "object");
    let result = host.execute_tool("mcp.call", &input, &context).unwrap();
    assert_eq!(result["content"][0]["text"], "real-pipe");
    let mut slow = input.clone();
    slow["arguments"] = json!({"delay":true});
    let runtime = host.clone();
    let ctx = context.clone();
    let handle = thread::spawn(move || runtime.execute_tool("mcp.call", &slow, &ctx));
    thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    host.stop_run(
        context.actor.run_id.as_ref().unwrap(),
        &context.actor.engine_epoch,
    );
    assert_eq!(handle.join().unwrap().unwrap_err().code, "CANCELLED");
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn native_effect_receipt_survives_epoch_and_rejects_changed_input() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let cap = host.capabilities.grant_directory(dir.path()).unwrap();
    let mut ctx = context("user");
    let command = new_id("command");
    let mut input =
        json!({"directory_capability":cap,"command_id":command,"path":"note.txt","content":"one"});
    host.execute_ui("ui.workspaceWrite", &input, &ctx).unwrap();
    fs::write(dir.path().join("note.txt"), "external edit").unwrap();
    ctx.actor.engine_epoch = new_id("epoch");
    assert!(host.execute_ui("ui.workspaceWrite", &input, &ctx).is_ok());
    assert_eq!(
        fs::read_to_string(dir.path().join("note.txt")).unwrap(),
        "external edit"
    );
    input["content"] = json!("changed");
    assert_eq!(
        host.execute_ui("ui.workspaceWrite", &input, &ctx)
            .unwrap_err()
            .code,
        "COMMAND_ID_REUSE"
    );
}
#[test]
fn direct_file_resolution_rejects_raw_paths_and_agent_owner() {
    let host = HostBridge::new(Arc::new(SecretVault::default()));
    let ctx = context("user");
    host.bind(&ctx.actor.engine_epoch, "workspace_fixture")
        .unwrap();
    let token = host
        .runtime
        .files
        .stage(None, "archive.zip", b"fixture", true)
        .unwrap();
    let params = json!({"file_capability":token,"purpose":"archive_import"});
    assert_eq!(
        host.execute_direct("workspace_fixture", &ctx, "file.resolve", params.clone())
            .unwrap()["byte_length"],
        "7"
    );
    assert_eq!(
        host.execute_direct(
            "workspace_fixture",
            &ctx,
            "file.resolve",
            json!({"file_capability":"C:/untrusted.zip","purpose":"archive_import"})
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    let mut agent = ctx.clone();
    agent.actor.kind = "agent".into();
    assert_eq!(
        host.execute_direct("workspace_fixture", &agent, "file.resolve", params)
            .unwrap_err()
            .code,
        "PERMISSION_DENIED"
    );
}
#[test]
fn actual_webfetch_receives_local_http_and_rejects_assets() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).unwrap();
        let body = "<h1>Native fixture</h1><p>Actual HTTP</p>";
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let host = HostRuntime::default();
    let ctx = context("agent");
    let response = host
        .execute_tool(
            "WebFetch",
            &json!({"url":format!("http://{address}/page"),"network_config":{"proxy":""}}),
            &ctx,
        )
        .unwrap();
    assert!(
        response["content"]
            .as_str()
            .unwrap()
            .contains("Actual HTTP")
    );
    assert_eq!(
        host.execute_tool(
            "WebFetch",
            &json!({"url":format!("http://{address}/app.js")}),
            &ctx
        )
        .unwrap_err()
        .code,
        "INVALID_PARAMS"
    );
    worker.join().unwrap();
}
#[test]
fn http_cancellation_closes_pending_headers_and_body() {
    for custom in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut head = vec![];
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                socket.read_exact(&mut b).unwrap();
                head.push(b[0]);
            }
            if custom {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 100\r\nConnection: close\r\n\r\n").unwrap();
            }
            tx.send(()).unwrap();
            let mut b = [0];
            assert!(
                matches!(socket.read(&mut b), Ok(0) | Err(_)),
                "Owned HTTP connection should close on cancellation"
            );
        });
        let host = Arc::new(HostRuntime::default());
        let dir = tempfile::tempdir().unwrap();
        let cap = host.capabilities.grant_directory(dir.path()).unwrap();
        let ctx = context("agent");
        let owned = host.clone();
        let owner = ctx.clone();
        let worker = thread::spawn(move || {
            if custom {
                owned.execute_tool("custom.execute",&json!({"directory_capability":cap,"definition":{"kind":"http","params":{},"exec":{"url":format!("http://{address}/delayed"),"method":"GET"}}}),&owner)
            } else {
                owned.execute_tool(
                    "WebFetch",
                    &json!({"url":format!("http://{address}/delayed")}),
                    &owner,
                )
            }
        });
        rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let start = Instant::now();
        host.stop_run(ctx.actor.run_id.as_ref().unwrap(), &ctx.actor.engine_epoch);
        assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
        assert!(start.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }
}

#[test]
fn actual_sdk_files_processes_monitor_and_sleep() {
    let directory = tempfile::tempdir().unwrap();
    let host = Arc::new(HostRuntime::default());
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let ctx = context("agent");
    host.execute_tool(
        "Write",
        &json!({"directory_capability":cap,"file_path":"note.txt","content":"first\nsecond\n"}),
        &ctx,
    )
    .unwrap();
    host.execute_tool("Edit",&json!({"directory_capability":cap,"file_path":"note.txt","old_string":"first","new_string":"edited"}),&ctx).unwrap();
    host.execute_tool("MultiEdit",&json!({"directory_capability":cap,"file_path":"note.txt","edits":[{"old_string":"edited","new_string":"final"},{"old_string":"second","new_string":"tail"}]}),&ctx).unwrap();
    let read = host
        .execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"note.txt"}),
            &ctx,
        )
        .unwrap();
    assert!(read.to_string().contains("final"));
    assert!(
        host.execute_tool("LS", &json!({"directory_capability":cap,"path":"."}), &ctx)
            .unwrap()
            .to_string()
            .contains("note.txt")
    );
    let completed=host.execute_tool("Bash",&json!({"directory_capability":cap,"command":"echo actual-sdk-command","timeout_ms":10000}),&ctx).unwrap();
    assert_eq!(completed["state"], "completed");
    assert!(
        completed["output"]
            .as_str()
            .unwrap()
            .contains("actual-sdk-command")
    );
    let command = if cfg!(windows) {
        "Write-Output actual-monitor; Start-Sleep -Seconds 30"
    } else {
        "echo actual-monitor; sleep 30"
    };
    let monitor = host
        .execute_tool(
            "Monitor",
            &json!({"directory_capability":cap,"command":command}),
            &ctx,
        )
        .unwrap();
    let id = monitor["task_id"].as_str().unwrap();
    assert!(
        host.execute_tool("TaskList", &json!({}), &ctx)
            .unwrap()
            .to_string()
            .contains(id)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = host
            .execute_tool("TaskOutput", &json!({"task_id":id}), &ctx)
            .unwrap();
        if output["output"]
            .as_str()
            .unwrap()
            .contains("actual-monitor")
        {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    host.execute_tool("TaskStop", &json!({"task_id":id}), &ctx)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while host
        .execute_tool("TaskOutput", &json!({"task_id":id}), &ctx)
        .unwrap()["state"]
        == "running"
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    let runtime = host.clone();
    let sleep_context = ctx.clone();
    let sleeping = thread::spawn(move || {
        runtime.execute_tool("sleep", &json!({"seconds":30}), &sleep_context)
    });
    thread::sleep(Duration::from_millis(60));
    host.stop_run(ctx.actor.run_id.as_ref().unwrap(), &ctx.actor.engine_epoch);
    assert_eq!(sleeping.join().unwrap().unwrap_err().code, "CANCELLED");
}

#[test]
fn actual_search_backends_against_local_http_fixtures() {
    for backend in ["ddgs", "brave-free", "tavily", "deepseek"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8_lossy(&request).to_lowercase();
            assert!(headers.contains("/fixture") || headers.contains("/v1/messages"));
            if matches!(backend, "brave-free" | "deepseek") {
                assert!(headers.contains("fixture-secret"));
            }
            let (mime,body)=match backend {
    "ddgs"=>("text/html",r#"<div class="result"><a class="result__a" href="https://fixture.invalid/result">Local search result</a><span class="result__snippet">Actual fixture snippet</span></div>"#.to_string()),
    "brave-free"=>("application/json",json!({"web":{"results":[{"title":"Local search result","url":"https://fixture.invalid/result","description":"Actual fixture snippet"}]}}).to_string()),
    "tavily"=>("application/json",json!({"results":[{"title":"Local search result","url":"https://fixture.invalid/result","content":"Actual fixture snippet"}]}).to_string()),
    _=>("text/event-stream",format!("data: {}\n\n",json!({"type":"content_block_start","content_block":{"type":"web_search_tool_result","content":[{"type":"web_search_result","title":"Local search result","url":"https://fixture.invalid/result"}]}}))),
   };
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let host = HostRuntime::default();
        let ctx = context("agent");
        let result=host.execute_tool("web_search",&json!({"query":"local fixture","limit":1,"network_config":{"backend":backend,"endpoint":format!("http://{address}/fixture"),"base_url":format!("http://{address}"),"model":"fixture-model","api_key":"fixture-secret","proxy":""}}),&ctx).unwrap();
        assert_eq!(
            result["results"][0]["url"], "https://fixture.invalid/result",
            "{backend}: {result}"
        );
        worker.join().unwrap();
    }
}

#[test]
fn actual_custom_command_python_and_http_execution() {
    let directory = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let cap = host.capabilities.grant_directory(directory.path()).unwrap();
    let ctx = context("agent");
    let command=host.execute_tool("custom.execute",&json!({"directory_capability":cap,"definition":{"kind":"command","exec":{"command":"echo {message}","timeout_ms":10000},"params":{"message":"actual custom shell"}}}),&ctx).unwrap();
    assert_eq!(command["is_error"], false);
    assert!(
        command["output"]
            .as_str()
            .unwrap()
            .contains("actual custom shell")
    );
    let script=host.execute_tool("custom.execute",&json!({"directory_capability":cap,"definition":{"kind":"script","exec":{"code":"import json,sys,os\np=json.load(sys.stdin)\nassert os.environ['TOOL_MESSAGE']==p['message']\nprint(p['message'])","timeout_ms":10000},"params":{"message":"actual custom python"}}}),&ctx).unwrap();
    assert_eq!(script["is_error"], false);
    assert!(
        script["output"]
            .as_str()
            .unwrap()
            .contains("actual custom python")
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        let count = stream.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..count]).contains("/custom?message=fixture"));
        let body = "actual custom HTTP";
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    });
    let http=host.execute_tool("custom.execute",&json!({"directory_capability":cap,"definition":{"kind":"http","exec":{"url":format!("http://{address}/custom?message={{message}}"),"method":"GET","timeout_ms":10000},"params":{"message":"fixture"}}}),&ctx).unwrap();
    assert_eq!(http["is_error"], false);
    assert!(
        http["output"]
            .as_str()
            .unwrap()
            .contains("actual custom HTTP")
    );
    worker.join().unwrap();
}
