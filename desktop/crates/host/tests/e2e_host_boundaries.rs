//! Source-derived regressions enter the real host tool boundary; external
//! fixtures never touch an application profile, user workspace or live site.
use artex_core::{Actor, Context, new_id};
use artex_host::HostRuntime;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
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
fn fetch(host: &HostRuntime, ctx: &Context, media: &str, body: &str, extract: bool) -> Value {
    fetch_result(host, ctx, media, body, extract).unwrap()
}
fn fetch_result(
    host: &HostRuntime,
    ctx: &Context,
    media: &str,
    body: &str,
    extract: bool,
) -> artex_core::Result<Value> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let media = media.to_owned();
    let body = body.to_owned();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        stream.read(&mut buf).unwrap();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let result = host.execute_tool(
        "WebFetch",
        &json!({"url":format!("http://{addr}/page"),"extract":extract}),
        ctx,
    );
    server.join().unwrap();
    result
}

#[test]
fn v09_actual_terminal_drain_delivers_all_retained_bytes_and_reaps() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let ctx = context();
    let cap = host.capabilities.grant_directory(dir.path()).unwrap();
    let command = if cfg!(windows) {
        "[Console]::Write(('X' * 20000) + 'TAIL_SENTINEL')"
    } else {
        "head -c 20000 /dev/zero | tr '\\0' X; printf TAIL_SENTINEL"
    };
    let opened = host
        .execute_tool(
            "shell_open",
            &json!({"directory_capability":cap,"command":command,"mode":"hands-free","cols":200}),
            &ctx,
        )
        .unwrap();
    let id = opened["session_id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let full = loop {
        let read = host
            .execute_tool(
                "shell_read",
                &json!({"session_id":id,"since_cursor":"0","max_bytes":65536,"max_lines":10000}),
                &ctx,
            )
            .unwrap();
        let inspected = host.inspect_run(
            ctx.actor.run_id.as_deref().unwrap(),
            &ctx.actor.engine_epoch,
        );
        if read["running"] == false
            && read["output"].as_str().unwrap().contains("TAIL_SENTINEL")
            && inspected["executions"][0]["output_drained"] == true
        {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "fixture output/cleanup never completed: {inspected}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let full_text = full["output"].as_str().unwrap();
    assert!(full_text.len() > 8192);
    let partial = host
        .execute_tool(
            "shell_read",
            &json!({"session_id":id,"since_cursor":"0","max_bytes":10,"max_lines":1}),
            &ctx,
        )
        .unwrap();
    assert_eq!(partial["cursor"], "10");
    let continued = host
        .execute_tool(
            "shell_read",
            &json!({"session_id":id,"max_bytes":65536,"max_lines":10000}),
            &ctx,
        )
        .unwrap();
    assert_eq!(
        format!(
            "{}{}",
            partial["output"].as_str().unwrap(),
            continued["output"].as_str().unwrap()
        ),
        full_text
    );
    for capped in [false, true] {
        let mut input = json!({"session_id":id,"since_cursor":"0","drain":true});
        if capped {
            input["max_bytes"] = json!(10);
            input["max_lines"] = json!(1);
        }
        let drained = host.execute_tool("shell_read", &input, &ctx).unwrap();
        assert_eq!(drained["output"], full["output"]);
        assert_eq!(drained["cursor"], full["cursor"]);
        assert_eq!(drained["omitted"], "0");
        assert_eq!(
            host.execute_tool("shell_read", &json!({"session_id":id}), &ctx)
                .unwrap()["output"],
            ""
        );
    }
    let mut stranger = ctx.clone();
    stranger.actor.run_id = Some(new_id("run"));
    assert_eq!(
        host.execute_tool(
            "shell_read",
            &json!({"session_id":id,"drain":true}),
            &stranger
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    let inspected = host.inspect_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert_eq!(inspected["executions"][0]["output_drained"], true);
    assert_eq!(inspected["cleanup_confirmed"], cfg!(windows));
    assert_eq!(
        host.execute_tool("shell_read", &json!({"session_id":id}), &ctx)
            .unwrap_err()
            .code,
        "CANCELLED"
    );
}

#[test]
fn v10_recon_compacts_large_scripts_and_preserves_grouped_security_signals() {
    let host = HostRuntime::default();
    let ctx = context();
    for padding in [String::new(), "x".repeat(60000)] {
        let html = format!(
            "<p>VISIBLE</p><script>/*{padding}*/const p='/api/SENTINEL'; const q='/api/SENTINEL'; const c='<!--NOT_A_COMMENT-->';</script><script src='/app.js'></script><script src='/app.js'></script><form method='post' action='/submit'><input type='hidden' name='csrf' value='TOKEN'><select name='choice'><option>one</option></select><textarea name='notes'></textarea></form><meta property='app:mode' content='safe mode'><link href='/style.css'><!-- COMMENT_SENTINEL -->"
        );
        let result = fetch(&host, &ctx, "text/html", &html, true);
        let content = result["content"].as_str().unwrap();
        for expected in [
            "=== RECON ===",
            "## COMMENTS",
            "COMMENT_SENTINEL",
            "## SCRIPTS (src)",
            "## INLINE ENDPOINTS",
            "/api/SENTINEL",
            "POST /submit → csrf(hidden=TOKEN) choice(select) notes(textarea)",
            "app:mode=safe mode",
            "/style.css",
        ] {
            assert!(content.contains(expected), "missing {expected}: {content}");
        }
        assert_eq!(content.matches("/api/SENTINEL").count(), 1);
        assert_eq!(content.matches("/app.js").count(), 1);
        assert!(!content.contains("NOT_A_COMMENT"));
        assert!(!content.contains("<script"));
        assert!(
            content.len() < 2000,
            "script padding spent the recon budget"
        );
    }
    let no_extract = fetch(
        &host,
        &ctx,
        "text/html",
        "<p>VISIBLE</p><script>const p='/api/SENTINEL'</script>",
        false,
    );
    assert_eq!(no_extract["content"], "VISIBLE");
    let no_recon = fetch(&host, &ctx, "text/html", "<p>VISIBLE</p>", true);
    assert_eq!(no_recon["content"], "VISIBLE");
    let unicode = fetch(
        &host,
        &ctx,
        "text/html",
        &format!(
            "<!--{}--><meta name='mode' content='{}'>",
            "한".repeat(200),
            "界".repeat(80)
        ),
        true,
    );
    assert!(unicode["content"].as_str().unwrap().contains('…'));
    assert!(unicode["content"].as_str().unwrap().len() < 1000);
}

#[test]
fn v11_read_reaches_offsets_after_long_lines_with_bounded_utf8_previews() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostRuntime::default();
    let ctx = context();
    let cap = host.capabilities.grant_directory(dir.path()).unwrap();
    for prefix in [
        "short".to_owned(),
        "x".repeat(70000),
        format!("{}{}", "x".repeat(8191), "한".repeat(30000)),
    ] {
        std::fs::write(
            dir.path().join("long.txt"),
            format!("{prefix}\nTAIL_SENTINEL\n"),
        )
        .unwrap();
        let result = host
            .execute_tool(
                "Read",
                &json!({"directory_capability":cap,"file_path":"long.txt","offset":2,"limit":1}),
                &ctx,
            )
            .unwrap();
        assert_eq!(result["content"], "2\tTAIL_SENTINEL\n");
        let first = host
            .execute_tool(
                "Read",
                &json!({"directory_capability":cap,"file_path":"long.txt","limit":1}),
                &ctx,
            )
            .unwrap();
        assert_eq!(
            first["content"]
                .as_str()
                .unwrap()
                .contains("[line truncated]"),
            prefix.len() > 2000
        );
        assert!(first["content"].as_str().unwrap().len() < 2100);
    }
    std::fs::write(
        dir.path().join("unicode.txt"),
        format!("{}한\n", "x".repeat(1999)),
    )
    .unwrap();
    let unicode = host
        .execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"unicode.txt","limit":1}),
            &ctx,
        )
        .unwrap();
    assert!(!unicode["content"].as_str().unwrap().contains('�'));
    for (file, body) in [
        ("binary", vec![0; 70000]),
        ("invalid", vec![0xff]),
        ("incomplete", vec![0xe2]),
    ] {
        std::fs::write(dir.path().join(file), body).unwrap();
        assert_eq!(
            host.execute_tool(
                "Read",
                &json!({"directory_capability":cap,"file_path":file}),
                &ctx
            )
            .unwrap_err()
            .code,
            "INVALID_PARAMS"
        );
    }
    std::fs::write(dir.path().join("empty"), "").unwrap();
    assert_eq!(
        host.execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"empty"}),
            &ctx
        )
        .unwrap()["content"],
        ""
    );
    assert_eq!(
        host.execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"long.txt","offset":500,"limit":1}),
            &ctx
        )
        .unwrap()["content"],
        ""
    );
    assert_eq!(
        host.execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"../outside"}),
            &ctx
        )
        .unwrap_err()
        .code,
        "PERMISSION_DENIED"
    );
    host.stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
    );
    assert_eq!(
        host.execute_tool(
            "Read",
            &json!({"directory_capability":cap,"file_path":"long.txt"}),
            &ctx
        )
        .unwrap_err()
        .code,
        "CANCELLED"
    );
}

#[test]
fn v14_mixed_case_html_media_uses_markdown_and_recon() {
    let host = HostRuntime::default();
    let ctx = context();
    let body = "<p>VISIBLE</p><script>const p='/api/SENTINEL'</script><!-- COMMENT_SENTINEL -->";
    for media in [
        "text/html",
        "Text/HTML; charset=UTF-8",
        "APPLICATION/XHTML+XML",
    ] {
        assert_eq!(fetch(&host, &ctx, media, body, false)["content"], "VISIBLE");
        let content = fetch(&host, &ctx, media, body, true);
        assert!(
            content["content"]
                .as_str()
                .unwrap()
                .contains("/api/SENTINEL")
        );
        assert!(
            content["content"]
                .as_str()
                .unwrap()
                .contains("COMMENT_SENTINEL")
        );
        assert_eq!(content["media_type"], media);
    }
    assert_eq!(
        fetch(&host, &ctx, "application/json", "{\"ok\":true}", true)["content"],
        "{\"ok\":true}"
    );
    assert_eq!(
        fetch_result(&host, &ctx, "APPLICATION/JAVASCRIPT", "static script", true)
            .unwrap_err()
            .code,
        "INVALID_PARAMS"
    );
}

#[test]
fn v15_shell_session_and_execution_ids_keep_retry_scope_and_cleanup_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let host = std::sync::Arc::new(HostRuntime::default());
    let ctx = context();
    let cap = host.capabilities.grant_directory(dir.path()).unwrap();
    let execution = new_id("exec");
    let command = if cfg!(windows) {
        "Start-Sleep -Seconds 30"
    } else {
        "sleep 30"
    };
    let input = json!({"directory_capability":cap,"execution_id":execution,"command":command,"mode":"hands-free"});
    let opened = host.execute_tool("shell_open", &input, &ctx).unwrap();
    let retries = (0..2)
        .map(|_| {
            let host = host.clone();
            let input = input.clone();
            let ctx = ctx.clone();
            thread::spawn(move || host.execute_tool("shell_open", &input, &ctx))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    let mut conflict = input.clone();
    conflict["command"] = json!("echo must-not-spawn");
    let changed = host.execute_tool("shell_open", &conflict, &ctx);
    let mut another = ctx.clone();
    another.actor.run_id = Some(new_id("run"));
    let wrong_run = host.execute_tool("shell_open", &input, &another);
    let mut another_epoch = ctx.clone();
    another_epoch.actor.engine_epoch = new_id("epoch");
    let wrong_epoch = host.execute_tool("shell_open", &input, &another_epoch);
    host.capabilities.revoke(&cap);
    let revoked = host.execute_tool("shell_open", &input, &ctx);
    host.capabilities
        .grant_directory_as(&cap, outside.path())
        .unwrap();
    let rebound = host.execute_tool("shell_open", &input, &ctx);
    let listed = host.execute_tool("shell_list", &json!({}), &ctx).unwrap();
    // Always stop the real fixture before checking ID/conflict expectations.
    let stopped = host.force_stop_run(
        ctx.actor.run_id.as_deref().unwrap(),
        &ctx.actor.engine_epoch,
        Duration::from_secs(5),
    );
    let session = opened["session_id"].as_str().unwrap();
    assert!(artex_core::valid_id(session, "shell"));
    assert_ne!(session, execution);
    for retry in retries {
        let retried = retry.unwrap();
        assert_eq!(retried["session_id"], opened["session_id"]);
        assert_eq!(retried["output_path"], opened["output_path"]);
    }
    for denied in [changed, wrong_run, wrong_epoch, rebound] {
        assert_eq!(denied.unwrap_err().code, "VERSION_CONFLICT");
    }
    assert_eq!(revoked.unwrap_err().code, "PERMISSION_DENIED");
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["session_id"], opened["session_id"]);
    assert_eq!(stopped["executions"].as_array().unwrap().len(), 1);
    assert_eq!(stopped["executions"][0]["execution_id"], execution);
    assert_eq!(stopped["executions"][0]["output_drained"], true);
    assert_eq!(stopped["cleanup_confirmed"], cfg!(windows));
    assert_eq!(
        std::fs::read_dir(dir.path().join(".artex-host/pty"))
            .unwrap()
            .count(),
        1
    );
    assert!(!outside.path().join(".artex-host").exists());
}
