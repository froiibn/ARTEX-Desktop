use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
};
fn prompt() -> Prompt {
    Prompt {
        system: "Korean or English.".into(),
        messages: vec![Message {
            role: "user".into(),
            content: vec![json!({"type":"text","text":"hello"})],
        }],
        tools: vec![],
        session_id: "fixture".into(),
    }
}
fn drain(socket: &mut TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut head = vec![];
    while !head.ends_with(b"\r\n\r\n") {
        let mut one = [0];
        socket.read_exact(&mut one).unwrap();
        head.push(one[0]);
        assert!(head.len() < 65536);
    }
    let len = String::from_utf8(head)
        .unwrap()
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|n| n.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    socket.read_exact(&mut body).unwrap();
}
fn server(responses: Vec<(u16, &'static str, String)>) -> (String, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let count = responses.len();
        for (status, media, body) in responses {
            let (mut socket, _) = listener.accept().unwrap();
            drain(&mut socket);
            write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        count
    });
    (url, handle)
}
fn profile(url: &str) -> Value {
    json!({"format":"openai","model":"local-fixture","base_url":url,"retry_connect_attempts":1,"retry_connect_interval_ms":1,"retry_empty_attempts":1,"retry_empty_interval_ms":1,"retry_stream_attempts":1,"retry_stream_interval_ms":1})
}
fn success() -> String {
    json!({"choices":[{"message":{"content":"실제 응답"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3}}).to_string()
}
#[test]
fn transient_establishment_and_empty_response_retry_actual_http() {
    for first in [
        (
            503,
            "application/json",
            json!({"error":{"message":"temporarily unavailable"}}).to_string(),
        ),
        (
            200,
            "application/json",
            json!({"choices":[{"message":{"content":""},"finish_reason":"stop"}]}).to_string(),
        ),
    ] {
        let (url, server) = server(vec![first, (200, "application/json", success())]);
        let turn = generate(
            &profile(&url),
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(turn.text, "실제 응답");
        assert_eq!(server.join().unwrap(), 2);
        assert_eq!(turn.usage["input_tokens"], 7);
    }
}
#[test]
fn quota_error_is_not_retried_and_credentials_are_redacted() {
    let(url,server)=server(vec![(429,"application/json",json!({"error":{"code":"insufficient_quota","message":"fixture-secret credit balance exhausted"}}).to_string())]);
    let error = generate(
        &profile(&url),
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert!(error.message.contains("insufficient_quota"));
    assert!(!error.message.contains("fixture-secret"));
    assert_eq!(error.details["status"], 429);
    assert_eq!(server.join().unwrap(), 1);
}
#[test]
fn partial_output_prevents_replay_but_preoutput_overload_retries() {
    let partial = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n".to_owned();
    let (url, server) = server(vec![(200, "text/event-stream", partial)]);
    let error = generate(
        &profile(&url),
        "key",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.details["committed"], true);
    assert_eq!(server.join().unwrap(), 1);
    let(url,server)=self::server(vec![(200,"text/event-stream","data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Try later\"}}\n".into()),(200,"application/json",success())]);
    assert!(
        generate(
            &profile(&url),
            "key",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(2)
        )
        .is_ok()
    );
    assert_eq!(server.join().unwrap(), 2);
}
#[test]
fn cancellation_closes_actual_pending_connection_within_one_second() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        drain(&mut socket);
        socket
            .set_read_timeout(Some(Duration::from_millis(25)))
            .unwrap();
        tx.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let mut b = [0];
            match socket.read(&mut b) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(_) => {}
            }
            if Instant::now() > until {
                return false;
            }
        }
    });
    let cancel = Cancellation::default();
    let worker_cancel = cancel.clone();
    let worker = thread::spawn(move || {
        generate(
            &profile(&url),
            "key",
            &prompt(),
            &worker_cancel,
            Duration::from_secs(60),
        )
    });
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    cancel.cancel();
    assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(server.join().unwrap());
}
#[test]
fn retry_disable_defaults_and_model_discovery_actual_http() {
    assert_eq!(
        retry_count(&json!({"retry_connect_attempts":-1}), "connect", 3),
        0
    );
    assert_eq!(
        retry_count(&json!({"retry_connect_attempts":0}), "connect", 3),
        3
    );
    let (url, server) = server(vec![(
        200,
        "application/json",
        json!({"data":[{"id":"one"},{"id":"two"}]}).to_string(),
    )]);
    assert_eq!(
        models(
            &profile(&url),
            "key",
            &Cancellation::default(),
            Duration::from_secs(2)
        )
        .unwrap(),
        vec!["one", "two"]
    );
    assert_eq!(server.join().unwrap(), 1);
}

fn anthropic_completed_sse(stop: &str, blocks: &[Value]) -> String {
    let mut events = format!(
        "data: {}\n\n",
        json!({"type":"message_start","message":{"usage":{"input_tokens":13439,"output_tokens":0}}})
    );
    for (index, block) in blocks.iter().enumerate() {
        events.push_str(&format!(
            "data: {}\n\n",
            json!({"type":"content_block_start","index":index,"content_block":block})
        ));
    }
    events.push_str(&format!(
        "data: {}\n\n",
        json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":0}})
    ));
    events
}
#[test]
fn completed_anthropic_refusal_reports_usage_and_prevents_provider_and_controller_replay() {
    let (url, worker) = server(vec![(
        200,
        "text/event-stream",
        anthropic_completed_sse("refusal", &[]),
    )]);
    let mut p = profile(&url);
    p["format"] = json!("anthropic");
    p["streaming"] = json!(true);
    let error = generate(
        &p,
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.code, "EXTERNAL_FAILURE");
    assert_eq!(
        error.message,
        "LLM provider declined the request (stop_reason=refusal)"
    );
    assert_eq!(error.details["committed"], true);
    assert_eq!(error.details["stop_reason"], "refusal");
    assert_eq!(error.details["completion_class"], "refusal");
    assert_eq!(error.details["usage"]["input_tokens"], 13439);
    assert_eq!(error.details["usage"]["output_tokens"], 0);
    assert_eq!(worker.join().unwrap(), 1);
}
#[test]
fn explicit_refusal_with_thinking_is_terminal_but_normal_thinking_only_keeps_existing_behavior() {
    let thinking =
        json!({"type":"thinking","thinking":"private-thought","signature":"private-signature"});
    for stop in ["refusal", "end_turn"] {
        let (url,worker)=server(vec![(200,"application/json",json!({"content":[thinking],"stop_reason":stop,"usage":{"input_tokens":5,"output_tokens":3}}).to_string())]);
        let mut p = profile(&url);
        p["format"] = json!("anthropic");
        let response = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(2),
        );
        if stop == "refusal" {
            let error = response.unwrap_err();
            assert_eq!(error.details["completion_class"], "refusal");
            assert_eq!(error.details["committed"], true);
            assert_eq!(error.details["content_summary"]["thinking_bytes"], 15);
            assert!(
                !serde_json::to_string(&error)
                    .unwrap()
                    .contains("private-thought")
            );
        } else {
            let turn = response.unwrap();
            assert_eq!(turn.thinking, "private-thought");
            assert!(turn.text.is_empty() && turn.tools.is_empty());
        }
        assert_eq!(worker.join().unwrap(), 1);
    }
}
#[test]
fn deterministic_empty_limits_filters_and_refusal_content_do_not_retry_actual_json() {
    for (stop, class) in [
        ("model_context_window_exceeded", "context_window_limit"),
        ("content_filter", "content_filter"),
    ] {
        let payload = json!({"content":[{"type":"redacted_thinking","data":"fixture-secret opaque private payload","signature":"private-signature"}],"stop_reason":stop,"usage":{"input_tokens":17,"output_tokens":8192}});
        let (url, worker) = server(vec![(200, "application/json", payload.to_string())]);
        let mut p = profile(&url);
        p["format"] = json!("anthropic");
        let error = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert_eq!(error.details["completion_class"], class);
        assert_eq!(error.details["committed"], true);
        assert_eq!(error.details["stop_reason"], stop);
        assert_eq!(error.details["usage"]["output_tokens"], 8192);
        assert_eq!(
            error.details["content_summary"]["class_counts"]["redacted_thinking"],
            1
        );
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("fixture-secret")
        );
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("private-signature")
        );
        assert_eq!(worker.join().unwrap(), 1);
    }
    let (url,worker)=server(vec![(200,"application/json",json!({"choices":[{"message":{"content":null,"refusal":"private refusal explanation fixture-secret"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":0}}).to_string())]);
    let error = generate(
        &profile(&url),
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.details["completion_class"], "refusal");
    assert_eq!(error.details["stop_reason"], "stop");
    assert!(error.message.contains("refusal response content"));
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private refusal explanation")
    );
    assert_eq!(worker.join().unwrap(), 1);
}
#[test]
fn completed_empty_json_and_sse_keep_safe_end_turn_diagnostics_and_unknown_block_counts() {
    let block = json!({"type":"private-provider-type","text":"fixture-secret","url":"https://private.example/credential","signature":"private-signature"});
    for (media,body) in [("application/json",json!({"content":[block],"stop_reason":"end_turn","usage":{"input_tokens":29,"output_tokens":2}}).to_string()),("text/event-stream",anthropic_completed_sse("end_turn",&[block.clone()]))] {
        let (url,worker)=server(vec![(200,media,body)]);
        let mut p=profile(&url);p["format"]=json!("anthropic");p["retry_empty_attempts"]=json!(-1);
        let error=generate(&p,"fixture-secret",&prompt(),&Cancellation::default(),Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.details["committed"],true);
        assert_eq!(error.details["completion_class"],"empty_output");
        assert_eq!(error.details["stop_reason"],"end_turn");
        assert_eq!(error.details["content_summary"]["class_counts"]["other"],1);
        assert_eq!(error.details["usage_reported"],true);
        let safe=serde_json::to_string(&error).unwrap();
        for forbidden in ["fixture-secret","private-provider-type","private.example","private-signature"] {assert!(!safe.contains(forbidden));}
        assert_eq!(worker.join().unwrap(),1);
    }
}
#[test]
fn explicit_refusal_content_array_is_not_mistaken_for_retryable_empty_output() {
    let (url,worker)=server(vec![(200,"application/json",json!({"choices":[{"message":{"content":[{"type":"refusal","refusal":"private refusal explanation"}]},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":0}}).to_string())]);
    let error = generate(
        &profile(&url),
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.details["completion_class"], "refusal");
    assert_eq!(error.details["committed"], true);
    assert_eq!(error.details["usage"]["input_tokens"], 11);
    assert_eq!(
        error.details["content_summary"]["class_counts"]["refusal"],
        1
    );
    assert!(
        !serde_json::to_string(&error)
            .unwrap()
            .contains("private refusal explanation")
    );
    assert_eq!(worker.join().unwrap(), 1);
}

// Count actual HTTP attempts while the caller owns the fixture lifetime. This
// catches regeneration after billed JSON output without relying on sleep timing.
fn counted_server(
    media: &'static str,
    body: String,
) -> (String, mpsc::Sender<()>, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (stop, stopped) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut calls = 0;
        loop {
            if stopped.try_recv().is_ok() {
                return calls;
            }
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_nonblocking(false).unwrap();
                    drain(&mut socket);
                    calls += 1;
                    write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("{e}"),
            }
        }
    });
    (url, stop, worker)
}

#[test]
fn responses_output_cap_preserves_partial_usage_and_true_stop_reason_without_replay() {
    for streaming in [false, true] {
        let raw = json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"message","content":[{"type":"output_text","text":"confirmed partial evidence"}]}],"usage":{"input_tokens":13,"output_tokens":5}});
        let (media, body) = if streaming {
            (
                "text/event-stream",
                format!(
                    "data: {}\n\n",
                    json!({"type":"response.incomplete","response":raw})
                ),
            )
        } else {
            ("application/json", raw.to_string())
        };
        let (url, stop, worker) = counted_server(media, body);
        let mut p = profile(&url);
        p["format"] = json!("openai-responses");
        p["retry_stream_attempts"] = json!(2);
        let result = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(3),
        );
        stop.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        let turn = result.unwrap();
        assert_eq!(turn.text, "confirmed partial evidence");
        assert_eq!(turn.stop_reason, "max_tokens");
        assert_eq!(turn.raw_response["status"], "incomplete");
        assert_eq!(turn.usage["input_tokens"], 13);
        assert_eq!(turn.usage["output_tokens"], 5);
    }
}

#[test]
fn observed_json_failure_is_committed_billed_and_never_replayed_or_executed() {
    for raw in [
        json!({"status":"incomplete","incomplete_details":{"reason":"content_filter"},"output":[{"type":"message","content":[{"type":"output_text","text":"observed draft"}]}],"usage":{"input_tokens":13,"output_tokens":5}}),
        json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[{"type":"function_call","call_id":"partial-call","name":"Bash","arguments":"{\"command\":"}],"usage":{"input_tokens":13,"output_tokens":5}}),
    ] {
        let (url, stop, worker) = counted_server("application/json", raw.to_string());
        let mut p = profile(&url);
        p["format"] = json!("openai-responses");
        p["retry_stream_attempts"] = json!(2);
        let error = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(3),
        )
        .unwrap_err();
        stop.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        assert_eq!(error.details["committed"], true);
        assert_eq!(error.details["usage"]["input_tokens"], 13);
        assert_eq!(error.details["usage"]["output_tokens"], 5);
    }
    let (url,worker)=server(vec![(200,"application/json",json!({"status":"completed","output":[{"type":"function_call","call_id":"whole-call","name":"Bash","arguments":"{\"command\":\"echo fixture\"}"}],"usage":{"input_tokens":3,"output_tokens":2}}).to_string())]);
    let mut p = profile(&url);
    p["format"] = json!("openai-responses");
    let turn = generate(
        &p,
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(turn.tools.len(), 1);
    assert_eq!(turn.tools[0].input["command"], "echo fixture");
    assert_eq!(worker.join().unwrap(), 1);
}

#[test]
fn output_free_json_failure_keeps_usage_but_can_retry_transient_generation_failure() {
    let failed =
        json!({"status":"failed","output":[],"usage":{"input_tokens":13,"output_tokens":0}});
    let error = decode("openai-responses", failed).unwrap_err();
    assert_ne!(error.details["committed"], true);
    assert_eq!(error.details["usage"]["input_tokens"], 13);
    // Status alone does not prove any text/thinking/tool was generated. Preserve
    // the existing transient retry policy when the server reports no output.
    let (url,worker)=server(vec![(200,"application/json",json!({"status":"failed","output":[]}).to_string()),(200,"application/json",json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"recovered answer"}]}],"usage":{"input_tokens":3,"output_tokens":2}}).to_string())]);
    let mut p = profile(&url);
    p["format"] = json!("openai-responses");
    let turn = generate(
        &p,
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(turn.text, "recovered answer");
    assert_eq!(worker.join().unwrap(), 2);
}

#[test]
fn interrupted_visible_text_and_chat_cache_usage_survive_actual_http() {
    for (format, body) in [
        (
            "openai",
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"choices":[{"delta":{"content":"visible partial answer"},"finish_reason":null}]}),
                json!({"choices":[],"usage":{"prompt_tokens":17,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":7}}})
            ),
        ),
        (
            "anthropic",
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type":"message_start","message":{"usage":{"input_tokens":17,"output_tokens":3,"cache_read_input_tokens":7}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"visible partial answer"}})
            ),
        ),
        (
            "openai-responses",
            format!(
                "data: {}\n\ndata: {}\n\n",
                json!({"type":"response.output_text.delta","delta":"visible partial answer"}),
                json!({"type":"response.failed","response":{"status":"failed","usage":{"input_tokens":17,"output_tokens":3,"input_tokens_details":{"cached_tokens":7}}}})
            ),
        ),
    ] {
        let (url, stop, worker) = counted_server("text/event-stream", body);
        let mut p = profile(&url);
        p["format"] = json!(format);
        let error = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(3),
        )
        .unwrap_err();
        stop.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        assert_eq!(error.details["partial_text"], "visible partial answer");
        assert_eq!(error.details["committed"], true);
        assert_eq!(
            error.details["usage"]["input_tokens"],
            if format == "anthropic" { 24 } else { 17 }
        );
        assert_eq!(error.details["usage"]["cache_read_tokens"], 7);
    }
    // JSON and SSE share usage normalization; unknown details stay zero.
    for (details, expected) in [
        (json!({"cached_tokens":7}), 7),
        (json!({}), 0),
        (json!({"cached_tokens":-2}), 0),
    ] {
        let turn=decode("openai",json!({"choices":[{"message":{"content":"complete"},"finish_reason":"stop"}],"usage":{"prompt_tokens":17,"completion_tokens":3,"prompt_tokens_details":details}})).unwrap();
        assert_eq!(turn.usage["cache_read_tokens"], expected);
    }
}

#[test]
fn failed_responses_terminal_only_text_is_preserved_without_duplicating_deltas() {
    for with_delta in [false, true] {
        let mut body = String::new();
        if with_delta {
            body.push_str(&format!(
                "data: {}\n\n",
                json!({"type":"response.output_text.delta","delta":"observed"})
            ));
        }
        body.push_str(&format!("data: {}\n\n",json!({"type":"response.failed","response":{"status":"failed","output":[{"type":"message","content":[{"type":"output_text","text":"observed draft"}]}],"usage":{"input_tokens":17,"output_tokens":3}}})));
        let (url, stop, worker) = counted_server("text/event-stream", body);
        let mut p = profile(&url);
        p["format"] = json!("openai-responses");
        let error = generate(
            &p,
            "fixture-secret",
            &prompt(),
            &Cancellation::default(),
            Duration::from_secs(3),
        )
        .unwrap_err();
        stop.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), 1);
        assert_eq!(error.details["partial_text"], "observed draft");
        assert_eq!(error.details["usage"]["input_tokens"], 17);
        assert_eq!(error.details["committed"], true);
    }
}

#[test]
fn anthropic_cache_usage_counts_total_input_once_for_json_sse_and_interruption() {
    let raw = json!({"input_tokens":3,"output_tokens":2,"cache_read_input_tokens":17,"cache_creation_input_tokens":5});
    for (media,body,ok) in [
        ("application/json",json!({"content":[{"type":"text","text":"cache-total"}],"stop_reason":"end_turn","usage":raw}).to_string(),true),
        ("text/event-stream",format!("data: {}\n\ndata: {}\n\ndata: {}\n\n",json!({"type":"message_start","message":{"usage":raw}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"cache-total"}}),json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}})),true),
        ("text/event-stream",format!("data: {}\n\ndata: {}\n\n",json!({"type":"message_start","message":{"usage":raw}}),json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"cache-total"}})),false),
    ] {
        let (url,stop,worker)=counted_server(media,body);let mut p=profile(&url);p["format"]=json!("anthropic");
        let result=generate(&p,"fixture-secret",&prompt(),&Cancellation::default(),Duration::from_secs(3));
        stop.send(()).unwrap();assert_eq!(worker.join().unwrap(),1);
        let usage=if ok {result.unwrap().usage}else{let error=result.unwrap_err();assert_eq!(error.details["partial_text"],"cache-total");error.details["usage"].clone()};
        assert_eq!(usage,json!({"input_tokens":25,"output_tokens":2,"cache_read_tokens":17,"cache_write_tokens":5}));
    }
    // These provider counters already include cached input; adding it again
    // would make the normalization fix corrupt the other two formats.
    for (format, raw) in [
        (
            "openai",
            json!({"choices":[{"message":{"content":"complete"},"finish_reason":"stop"}],"usage":{"prompt_tokens":25,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":17}}}),
        ),
        (
            "openai-responses",
            json!({"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"complete"}]}],"usage":{"input_tokens":25,"output_tokens":2,"input_tokens_details":{"cached_tokens":17}}}),
        ),
    ] {
        let turn = decode(format, raw).unwrap();
        assert_eq!(turn.usage["input_tokens"], 25);
        assert_eq!(turn.usage["cache_read_tokens"], 17);
    }
}

#[test]
fn responses_named_event_type_fallback_and_authority_use_one_actual_request() {
    for named in [false, true] {
        for conflict in [false, true] {
            if !named && conflict {
                continue;
            }
            let mut delta = json!({"delta":"named 한글🙂"});
            let mut terminal = json!({"response":{"status":"completed","output":[],"usage":{"input_tokens":9,"output_tokens":4}}});
            if !named || conflict {
                delta["type"] = json!(if conflict {
                    "response.failed"
                } else {
                    "response.output_text.delta"
                });
                terminal["type"] = json!(if conflict {
                    "response.failed"
                } else {
                    "response.completed"
                });
            }
            let body = if named {
                format!(
                    "event: response.output_text.delta\r\ndata: {delta}\r\n\r\nevent: response.completed\r\ndata: {terminal}\r\n\r\n"
                )
            } else {
                format!("data: {delta}\n\ndata: {terminal}\n\n")
            };
            let (url, stop, worker) = counted_server("text/event-stream", body);
            let mut p = profile(&url);
            p["format"] = json!("openai-responses");
            let mut observed = String::new();
            let turn = generate_streaming(
                &p,
                "fixture-secret",
                &prompt(),
                &Cancellation::default(),
                Duration::from_secs(3),
                &mut |delta| {
                    if let StreamDelta::Text(piece) = delta {
                        observed.push_str(&piece);
                    }
                    Ok(())
                },
            )
            .unwrap();
            stop.send(()).unwrap();
            assert_eq!(worker.join().unwrap(), 1);
            assert_eq!(turn.text, "named 한글🙂");
            assert_eq!(observed, "named 한글🙂");
            assert_eq!(turn.usage["input_tokens"], 9);
        }
    }
}

#[test]
fn responses_assembled_deltas_keep_text_reasoning_and_verified_tools_without_terminal_output() {
    for output in [Value::Null, json!([])] {
        for args in ["{}", "{\"path\":"] {
            let mut terminal =
                json!({"status":"completed","usage":{"input_tokens":9,"output_tokens":4}});
            if !output.is_null() {
                terminal["output"] = output.clone();
            }
            let body = format!(
                "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\ndata: {}\n\n",
                json!({"type":"response.output_text.delta","output_index":0,"delta":"observed evidence"}),
                json!({"type":"response.reasoning_summary_text.delta","output_index":1,"delta":"private reasoning"}),
                json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"fixture-call","name":"shell_list","arguments":""}}),
                json!({"type":"response.function_call_arguments.delta","output_index":2,"delta":args}),
                json!({"type":"response.completed","response":terminal})
            );
            let (url, stop, worker) = counted_server("text/event-stream", body);
            let mut p = profile(&url);
            p["format"] = json!("openai-responses");
            let mut visible = String::new();
            let result = generate_streaming(
                &p,
                "fixture-secret",
                &prompt(),
                &Cancellation::default(),
                Duration::from_secs(3),
                &mut |delta| {
                    if let StreamDelta::Text(piece) = delta {
                        visible.push_str(&piece);
                    }
                    Ok(())
                },
            );
            stop.send(()).unwrap();
            assert_eq!(worker.join().unwrap(), 1);
            assert_eq!(visible, "observed evidence");
            if args == "{}" {
                let turn = result.unwrap();
                assert_eq!(turn.text, "observed evidence");
                assert_eq!(turn.thinking, "private reasoning");
                assert_eq!(turn.tools.len(), 1);
                assert_eq!(turn.tools[0].input, json!({}));
                assert_eq!(turn.usage["input_tokens"], 9);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.details["committed"], true);
                assert_eq!(error.details["partial_text"], "observed evidence");
                assert_eq!(error.details["usage"]["input_tokens"], 9);
            }
        }
    }
    // A contradictory nonempty terminal snapshot cannot cause regeneration of
    // text already emitted to the caller, even when that snapshot is unusable.
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"type":"response.output_text.delta","delta":"real draft"}),
        json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"unknown-block"}],"usage":{"input_tokens":9,"output_tokens":4}}})
    );
    let (url, stop, worker) = counted_server("text/event-stream", body);
    let mut p = profile(&url);
    p["format"] = json!("openai-responses");
    let error = generate(
        &p,
        "fixture-secret",
        &prompt(),
        &Cancellation::default(),
        Duration::from_secs(3),
    )
    .unwrap_err();
    stop.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), 1);
    assert_eq!(error.details["partial_text"], "real draft");
    assert_eq!(error.details["committed"], true);
}

#[test]
fn completed_zero_argument_tools_accept_blank_json_and_sse_but_never_truncated_objects() {
    let mut request = prompt();
    request.tools = vec![
        json!({"name":"shell_list","description":"fixture sessions","input_schema":{"type":"object","properties":{}}}),
    ];
    for format in ["openai", "openai-responses"] {
        for streaming in [false, true] {
            for args in ["{}", "", "   ", "{\"path\":"] {
                let raw = if format == "openai" {
                    json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"fixture-call","function":{"name":"shell_list","arguments":args}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2}})
                } else {
                    json!({"status":"completed","output":[{"type":"function_call","call_id":"fixture-call","name":"shell_list","arguments":args}],"usage":{"input_tokens":5,"output_tokens":2}})
                };
                let (media, body) = if !streaming {
                    ("application/json", raw.to_string())
                } else if format == "openai" {
                    (
                        "text/event-stream",
                        format!(
                            "data: {}\n\ndata: {}\n\n",
                            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"fixture-call","function":{"name":"shell_list","arguments":args}}]},"finish_reason":"tool_calls"}]}),
                            json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}})
                        ),
                    )
                } else {
                    (
                        "text/event-stream",
                        format!(
                            "data: {}\n\ndata: {}\n\n",
                            json!({"type":"response.output_item.added","item":{"type":"function_call","call_id":"fixture-call","name":"shell_list","arguments":args}}),
                            json!({"type":"response.completed","response":{"status":"completed","output":[],"usage":{"input_tokens":5,"output_tokens":2}}})
                        ),
                    )
                };
                let (url, stop, worker) = counted_server(media, body);
                let mut p = profile(&url);
                p["format"] = json!(format);
                let result = generate(
                    &p,
                    "fixture-secret",
                    &request,
                    &Cancellation::default(),
                    Duration::from_secs(3),
                );
                stop.send(()).unwrap();
                assert_eq!(worker.join().unwrap(), 1);
                if args.starts_with('{') && args != "{}" {
                    let error = result.unwrap_err();
                    assert_eq!(error.details["committed"], true);
                    assert_eq!(error.details["usage"]["input_tokens"], 5);
                } else {
                    let turn = result.unwrap();
                    assert_eq!(turn.tools.len(), 1);
                    assert_eq!(turn.tools[0].name, "shell_list");
                    assert_eq!(turn.tools[0].input, json!({}));
                    assert_eq!(turn.usage["input_tokens"], 5);
                }
            }
        }
    }
}

#[test]
fn valid_empty_or_thinking_only_output_caps_reach_controller_without_provider_replay() {
    for format in ["openai", "anthropic", "openai-responses"] {
        for thinking in [false, true] {
            for streaming in [false, true] {
                let raw = match format {
                    "anthropic" => {
                        json!({"content":if thinking {json!([{"type":"thinking","thinking":"capped reasoning","signature":"fixture"}])}else{json!([])},"stop_reason":"max_tokens","usage":{"input_tokens":5,"output_tokens":1024}})
                    }
                    "openai-responses" => {
                        json!({"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":if thinking {json!([{"type":"reasoning","summary":[{"type":"summary_text","text":"capped reasoning"}]}])}else{json!([])},"usage":{"input_tokens":5,"output_tokens":1024}})
                    }
                    _ => {
                        json!({"choices":[{"message":{"content":"","reasoning_content":if thinking {"capped reasoning"}else{""}},"finish_reason":"length"}],"usage":{"prompt_tokens":5,"completion_tokens":1024}})
                    }
                };
                let (media, body) = if !streaming {
                    ("application/json", raw.to_string())
                } else if format == "anthropic" {
                    let mut body = format!(
                        "data: {}\n\n",
                        json!({"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}})
                    );
                    for (index, block) in raw["content"].as_array().unwrap().iter().enumerate() {
                        body.push_str(&format!("data: {}\n\n",json!({"type":"content_block_start","index":index,"content_block":block})));
                    }
                    body.push_str(&format!("data: {}\n\n",json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":1024}})));
                    ("text/event-stream", body)
                } else if format == "openai-responses" {
                    (
                        "text/event-stream",
                        format!(
                            "data: {}\n\n",
                            json!({"type":"response.incomplete","response":raw})
                        ),
                    )
                } else {
                    (
                        "text/event-stream",
                        format!(
                            "data: {}\n\ndata: {}\n\n",
                            json!({"choices":[{"delta":{"reasoning_content":if thinking {"capped reasoning"}else{""}},"finish_reason":"length"}]}),
                            json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":1024}})
                        ),
                    )
                };
                let (url, stop, worker) = counted_server(media, body);
                let mut p = profile(&url);
                p["format"] = json!(format);
                let turn = generate(
                    &p,
                    "fixture-secret",
                    &prompt(),
                    &Cancellation::default(),
                    Duration::from_secs(3),
                )
                .unwrap();
                stop.send(()).unwrap();
                assert_eq!(worker.join().unwrap(), 1);
                assert!(turn.text.is_empty() && turn.tools.is_empty());
                assert_eq!(
                    turn.stop_reason,
                    if format == "openai" {
                        "length"
                    } else {
                        "max_tokens"
                    }
                );
                assert_eq!(
                    turn.thinking,
                    if thinking { "capped reasoning" } else { "" }
                );
                assert_eq!(turn.usage["input_tokens"], 5);
                assert_eq!(turn.usage["output_tokens"], 1024);
            }
        }
    }
}

#[test]
fn ready_transport_results_cannot_escape_an_expired_owning_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for cancelled in [false, true] {
        let cancel = Cancellation::default();
        if cancelled {
            cancel.cancel();
        }
        let error = runtime
            .block_on(owned(
                &cancel,
                Instant::now() - Duration::from_secs(1),
                std::future::ready(Err::<(), _>("transport timeout")),
            ))
            .unwrap_err();
        assert_eq!(error.code, if cancelled { "CANCELLED" } else { "TIMEOUT" });
    }
    assert_eq!(
        runtime
            .block_on(owned(
                &Cancellation::default(),
                Instant::now() + Duration::from_secs(1),
                std::future::ready("still owned")
            ))
            .unwrap(),
        "still owned"
    );
}
