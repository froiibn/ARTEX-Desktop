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
        ("max_tokens", "output_token_limit"),
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
