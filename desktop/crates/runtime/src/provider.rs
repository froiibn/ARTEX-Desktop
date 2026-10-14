//! OpenAI chat, Responses and Anthropic HTTP clients. Secrets never enter records.
use crate::{Cancellation, Message, Prompt, ToolCall, Turn};
use artex_core::{Error, Result};
use reqwest::Client;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
pub fn models(
    profile: &Value,
    key: &str,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Vec<String>> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?
        .block_on(models_async(profile, key, cancel, timeout))
}
async fn models_async(
    profile: &Value,
    key: &str,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<Vec<String>> {
    let format = profile["format"].as_str().unwrap_or("openai");
    let base = profile["base_url"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(if format == "anthropic" {
            "https://api.anthropic.com"
        } else {
            "https://api.openai.com/v1"
        })
        .trim_end_matches('/');
    let mut candidates = Vec::new();
    if format == "anthropic" {
        let b = base
            .strip_suffix("/v1/messages")
            .unwrap_or(base)
            .trim_end_matches('/');
        candidates.push((
            format!(
                "{}/models",
                if b.ends_with("/v1") {
                    b.to_owned()
                } else {
                    format!("{b}/v1")
                }
            ),
            true,
        ));
        if let Some(root) = b.strip_suffix("/anthropic") {
            candidates.push((format!("{root}/models"), false));
            candidates.push((format!("{root}/v1/models"), false));
        }
    } else if ["openai", "openai-responses"].contains(&format) {
        let b = base
            .strip_suffix("/chat/completions")
            .or_else(|| base.strip_suffix("/responses"))
            .unwrap_or(base);
        candidates.push((format!("{b}/models"), false));
    } else {
        return Err(Error::new("INVALID_PARAMS", "Unsupported provider format"));
    }
    let started = Instant::now();
    let mut empty = false;
    let mut error = Error::new("EXTERNAL_FAILURE", "Model list unavailable");
    for (url, anthropic) in candidates {
        cancel.check()?;
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err(Error::new("TIMEOUT", "Model discovery deadline reached"));
        }
        let mut builder = Client::builder()
            .no_proxy()
            .timeout(left)
            .connect_timeout(left.min(Duration::from_secs(10)));
        if let Some(proxy) = profile["proxy"].as_str().filter(|s| !s.is_empty()) {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid provider proxy"))?,
            );
        }
        let client = builder
            .build()
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Provider TLS client unavailable"))?;
        let request = client.get(&url);
        let request = if anthropic {
            request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(key)
        };
        let mut response = match owned(cancel, started + timeout, request.send()).await? {
            Ok(r) => r,
            Err(e) => {
                error = Error::new(
                    if e.is_timeout() {
                        "TIMEOUT"
                    } else {
                        "EXTERNAL_FAILURE"
                    },
                    "Provider model request failed",
                );
                continue;
            }
        };
        cancel.check()?;
        if !response.status().is_success() {
            error = Error::new(
                "EXTERNAL_FAILURE",
                &format!(
                    "Provider model request returned HTTP {}",
                    response.status().as_u16()
                ),
            );
            continue;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = owned(cancel, started + timeout, response.chunk())
            .await?
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Provider model response interrupted"))?
        {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Provider model response exceeds 4 MiB",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let parsed: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => {
                error = Error::new("EXTERNAL_FAILURE", "Provider model response is not JSON");
                continue;
            }
        };
        let Some(data) = parsed["data"].as_array() else {
            error = Error::new("EXTERNAL_FAILURE", "Provider model data list missing");
            continue;
        };
        let models = data
            .iter()
            .filter_map(|v| v["id"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !models.is_empty() {
            return Ok(models);
        }
        empty = true;
    }
    if empty { Ok(vec![]) } else { Err(error) }
}
pub fn encode(profile: &Value, prompt: &Prompt) -> Result<(String, Value)> {
    let format = profile["format"].as_str().unwrap_or("openai");
    let model = profile["model"]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "LLM model required"))?;
    let base = profile["base_url"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(if format == "anthropic" {
            "https://api.anthropic.com/v1"
        } else {
            "https://api.openai.com/v1"
        })
        .trim_end_matches('/');
    let mut body = match format {
        "anthropic" => {
            let mut messages = prompt.messages.clone();
            for m in &mut messages {
                m.content.retain(|b| {
                    b["type"] != "thinking"
                        || b["signature"].as_str().is_some_and(|s| !s.is_empty())
                });
            }
            // API requires consecutive roles to be coalesced.
            let mut merged: Vec<Message> = Vec::new();
            for m in messages.drain(..) {
                if let Some(last) = merged.last_mut().filter(|p| p.role == m.role) {
                    last.content.extend(m.content);
                } else {
                    merged.push(m);
                }
            }
            json!({"model":model,"system":prompt.system,"messages":merged,"tools":prompt.tools,"max_tokens":profile["max_tokens"].as_u64().filter(|n| *n>0).unwrap_or(8192),"stream":profile["streaming"] == true})
        }
        "openai" => {
            let mut messages = vec![json!({"role":"system","content":prompt.system})];
            for message in &prompt.messages {
                let mut text = String::new();
                let mut thinking = String::new();
                let mut calls = Vec::new();
                for block in &message.content {
                    match block["type"].as_str() {
                        Some("text") => text.push_str(block["text"].as_str().unwrap_or("")),
                        Some("thinking") => thinking.push_str(block["thinking"].as_str().unwrap_or("")),
                        Some("tool_use") => calls.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":serde_json::to_string(&block["input"])?}})),
                        Some("tool_result") => messages.push(json!({"role":"tool","tool_call_id":block["tool_use_id"],"content":content_text(&block["content"])})),
                        _ => {}
                    }
                }
                if !text.is_empty() || !thinking.is_empty() || !calls.is_empty() {
                    let mut m = json!({"role":message.role,"content":text});
                    if !thinking.is_empty() {
                        m["reasoning_content"] = json!(thinking);
                    }
                    if !calls.is_empty() {
                        m["tool_calls"] = json!(calls);
                    }
                    messages.push(m);
                }
            }
            json!({"model":model,"messages":messages,"tools":prompt.tools.iter().map(|t|json!({"type":"function","function":{"name":t["name"],"description":t["description"],"parameters":t["input_schema"]}})).collect::<Vec<_>>(),"stream":profile["streaming"] == true})
        }
        "openai-responses" => {
            let mut input = Vec::new();
            for m in &prompt.messages {
                let mut text = String::new();
                for b in &m.content {
                    match b["type"].as_str() {
                        Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
                        Some("tool_use") => input.push(json!({"type":"function_call","call_id":b["id"],"name":b["name"],"arguments":serde_json::to_string(&b["input"])?})),
                        Some("tool_result") => input.push(json!({"type":"function_call_output","call_id":b["tool_use_id"],"output":content_text(&b["content"])})),
                        _ => {}
                    }
                }
                if !text.is_empty() {
                    input.push(json!({"role":m.role,"content":text}));
                }
            }
            json!({"model":model,"instructions":prompt.system,"input":input,"tools":prompt.tools.iter().map(|t|json!({"type":"function","name":t["name"],"description":t["description"],"parameters":t["input_schema"]})).collect::<Vec<_>>(),"stream":profile["streaming"] == true,"store":false})
        }
        _ => return Err(Error::new("INVALID_PARAMS", "Unsupported LLM format")),
    };
    let max = profile["max_tokens"].as_u64().filter(|n| *n > 0);
    if format != "anthropic" {
        if let Some(max) = max {
            let field = if format == "openai-responses" {
                "max_output_tokens"
            } else {
                profile["max_tokens_field"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("max_tokens")
            };
            body[field] = json!(max);
        }
    }
    if let Some(effort) = profile["reasoning_effort"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        if format == "openai-responses" {
            body["reasoning"] = json!({"effort":effort});
        } else if format == "openai" {
            body["reasoning_effort"] = json!(effort);
        } else if format == "anthropic" {
            body["output_config"] = json!({"effort":effort});
        }
    }
    if format == "anthropic" && profile["thinking_type"] == "enabled" {
        let max = body["max_tokens"].as_u64().unwrap();
        body["thinking"] =
            json!({"type":"enabled","budget_tokens":(max/2).max(1024).min(max.saturating_sub(1))});
    }
    if profile["thinking_type"] == "disabled" {
        body["thinking"] = json!({"type":"disabled"});
    }
    if format == "openai" && profile["thinking_type"] == "enabled" {
        body["thinking"] = json!({"type":"enabled"});
    }
    let endpoint = match format {
        "anthropic" => "messages",
        "openai-responses" => "responses",
        _ => "chat/completions",
    };
    let url = if base.ends_with(endpoint) {
        base.to_owned()
    } else {
        format!("{base}/{endpoint}")
    };
    Ok((url, body))
}

#[cfg(test)]
#[path = "provider_fixtures.rs"]
mod network_fixtures;
fn content_text(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        s.into()
    } else if let Some(a) = v.as_array() {
        a.iter()
            .map(|b| b["text"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        v.to_string()
    }
}
fn usage(v: &Value) -> Value {
    json!({"input_tokens":v["input_tokens"].as_i64().or_else(||v["prompt_tokens"].as_i64()).unwrap_or(0).clamp(0,i32::MAX as i64),"output_tokens":v["output_tokens"].as_i64().or_else(||v["completion_tokens"].as_i64()).unwrap_or(0).clamp(0,i32::MAX as i64),"cache_read_tokens":v["cache_read_input_tokens"].as_i64().or_else(||v["input_tokens_details"]["cached_tokens"].as_i64()).unwrap_or(0).clamp(0,i32::MAX as i64),"cache_write_tokens":v["cache_creation_input_tokens"].as_i64().unwrap_or(0).clamp(0,i32::MAX as i64)})
}
fn call(id: &Value, name: &Value, input: Value) -> Result<ToolCall> {
    Ok(ToolCall {
        id: id
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Provider returned an empty tool id"))?
            .into(),
        name: name
            .as_str()
            .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Provider returned no tool name"))?
            .into(),
        input,
    })
}
pub fn decode(format: &str, response: Value) -> Result<Turn> {
    let mut turn = Turn {
        raw_response: response.clone(),
        usage: usage(&response["usage"]),
        ..Default::default()
    };
    if response.get("error").is_some_and(|e| !e.is_null()) {
        return Err(provider_error(200, &serde_json::to_vec(&response)?, ""));
    }
    match format {
        "anthropic" => {
            turn.stop_reason = response["stop_reason"].as_str().unwrap_or("").into();
            for b in response["content"]
                .as_array()
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Missing Anthropic content"))?
            {
                match b["type"].as_str() {
                    Some("text") => turn.text.push_str(b["text"].as_str().unwrap_or("")),
                    Some("thinking") => {
                        turn.thinking.push_str(b["thinking"].as_str().unwrap_or(""));
                        turn.thinking_signature = b["signature"].as_str().unwrap_or("").into();
                    }
                    Some("tool_use") => {
                        turn.tools
                            .push(call(&b["id"], &b["name"], b["input"].clone())?)
                    }
                    _ => {}
                }
            }
        }
        "openai-responses" => {
            turn.stop_reason = response["status"].as_str().unwrap_or("").into();
            if ["failed", "incomplete", "cancelled"].contains(&turn.stop_reason.as_str()) {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Responses generation did not complete",
                ));
            }
            for b in response["output"]
                .as_array()
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Missing Responses output"))?
            {
                match b["type"].as_str() {
                    Some("message") => {
                        for c in b["content"].as_array().into_iter().flatten() {
                            if c["type"] == "output_text" {
                                turn.text.push_str(c["text"].as_str().unwrap_or(""));
                            }
                        }
                    }
                    Some("reasoning") => {
                        for c in b["summary"].as_array().into_iter().flatten() {
                            turn.thinking.push_str(c["text"].as_str().unwrap_or(""));
                        }
                    }
                    Some("function_call") => turn.tools.push(call(
                        &b["call_id"],
                        &b["name"],
                        serde_json::from_str(b["arguments"].as_str().unwrap_or("{}"))?,
                    )?),
                    _ => {}
                }
            }
        }
        _ => {
            let c = &response["choices"][0];
            let m = &c["message"];
            if m.is_null() {
                return Err(Error::new("EXTERNAL_FAILURE", "Missing OpenAI message"));
            }
            turn.stop_reason = c["finish_reason"].as_str().unwrap_or("").into();
            turn.text = content_text(&m["content"]);
            if m["content"].is_null() {
                turn.text.clear();
            }
            turn.thinking = m["reasoning_content"].as_str().unwrap_or("").into();
            for b in m["tool_calls"].as_array().into_iter().flatten() {
                turn.tools.push(call(
                    &b["id"],
                    &b["function"]["name"],
                    serde_json::from_str(b["function"]["arguments"].as_str().unwrap_or("{}"))?,
                )?);
            }
        }
    }
    Ok(turn)
}

/// A completed provider response is attributable even when it has no visible
/// assistant text. Keep diagnostics structural: no generated text, signature,
/// arbitrary provider field/type names, URL or request content is copied here.
fn empty_completion_class(turn: &Turn) -> &'static str {
    match turn.stop_reason.as_str() {
        "length" | "max_tokens" => return "output_token_limit",
        "model_context_window_exceeded" => return "context_window_limit",
        "content_filter" => return "content_filter",
        "refusal" | "refused" => return "refusal",
        _ => {}
    }
    let response = &turn.raw_response;
    let explicit_refusal = response["choices"][0]["message"]["refusal"]
        .as_str()
        .is_some_and(|s| !s.is_empty())
        || response["choices"][0]["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["type"] == "refusal")
        || response["content"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| b["type"] == "refusal")
        || response["output"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|b| {
                b["type"] == "refusal"
                    || b["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|c| c["type"] == "refusal")
            });
    if explicit_refusal {
        "refusal"
    } else {
        "empty_output"
    }
}
fn completed_empty_error(format: &str, turn: &Turn) -> Error {
    let class = empty_completion_class(turn);
    let message = match class {
        "output_token_limit" => {
            "LLM reached the output token limit before returning text or a tool call"
        }
        "context_window_limit" => {
            "LLM reached its context window limit before returning text or a tool call"
        }
        "content_filter" => "LLM provider filtered the response (stop_reason=content_filter)",
        "refusal" if matches!(turn.stop_reason.as_str(), "refusal" | "refused") => {
            "LLM provider declined the request (stop_reason=refusal)"
        }
        "refusal" => "LLM provider declined the request (refusal response content)",
        _ => "LLM returned no text or tool call",
    };
    let mut counts = json!({"text":0,"thinking":0,"redacted_thinking":0,"tool_use":0,"message":0,"output_text":0,"reasoning":0,"function_call":0,"refusal":0,"other":0});
    fn count(block: &Value, counts: &mut Value) {
        let key = match block["type"].as_str() {
            Some("text") => "text",
            Some("thinking") => "thinking",
            Some("redacted_thinking") => "redacted_thinking",
            Some("tool_use") => "tool_use",
            Some("message") => "message",
            Some("output_text") => "output_text",
            Some("reasoning") => "reasoning",
            Some("function_call") => "function_call",
            Some("refusal") => "refusal",
            _ => "other",
        };
        counts[key] = json!(counts[key].as_u64().unwrap_or(0) + 1);
        if block["type"] == "message" {
            for child in block["content"].as_array().into_iter().flatten() {
                count(child, counts);
            }
        }
    }
    for block in turn.raw_response["content"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(turn.raw_response["output"].as_array().into_iter().flatten())
        .chain(
            turn.raw_response["choices"][0]["message"]["content"]
                .as_array()
                .into_iter()
                .flatten(),
        )
    {
        count(block, &mut counts);
    }
    let stop = match turn.stop_reason.as_str() {
        "length" => "length",
        "max_tokens" => "max_tokens",
        "model_context_window_exceeded" => "model_context_window_exceeded",
        "content_filter" => "content_filter",
        "refusal" => "refusal",
        "refused" => "refused",
        "end_turn" => "end_turn",
        "stop" => "stop",
        "stop_sequence" => "stop_sequence",
        "tool_use" => "tool_use",
        "tool_calls" => "tool_calls",
        "pause_turn" => "pause_turn",
        "completed" => "completed",
        "" => "absent",
        _ => "unknown",
    };
    let safe_format = match format {
        "anthropic" => "anthropic",
        "openai" => "openai",
        "openai-responses" => "openai-responses",
        _ => "unknown",
    };
    let mut error = Error::new("EXTERNAL_FAILURE", message);
    error.details = json!({"committed":true,"usage":turn.usage,"usage_reported":turn.raw_response["usage"].as_object().is_some_and(|o|!o.is_empty()),"stop_reason":stop,"completion_class":class,"format":safe_format,"content_summary":{"text_bytes":turn.text.len(),"thinking_bytes":turn.thinking.len(),"tool_calls":turn.tools.len(),"class_counts":counts}});
    error
}

#[path = "provider_stream.rs"]
mod streaming;
pub use streaming::StreamDelta;
/// Decode the same incremental state machine used by the HTTP worker.
pub fn decode_sse(format: &str, text: &str) -> Result<Turn> {
    let mut stream = streaming::SseDecoder::new(format);
    stream.feed(text.as_bytes(), &mut |_| Ok(()))?;
    stream.finish(&mut |_| Ok(()))
}
fn retry_count(profile: &Value, layer: &str, default: u64) -> u64 {
    match profile[format!("retry_{layer}_attempts")]
        .as_i64()
        .unwrap_or(0)
    {
        0 => default,
        n if n < 0 => 0,
        n => (n as u64).min(20),
    }
}
async fn owned<F: std::future::Future>(
    cancel: &Cancellation,
    deadline: Instant,
    future: F,
) -> Result<F::Output> {
    tokio::pin!(future);
    let mut timer = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {
            value=&mut future=>{cancel.check()?;return Ok(value)},
            _=timer.tick()=>{cancel.check()?;if Instant::now()>=deadline{return Err(Error::new("TIMEOUT","LLM deadline elapsed"));}}
        }
    }
}
async fn retry_wait(
    profile: &Value,
    layer: &str,
    attempt: u64,
    cap: u64,
    cancel: &Cancellation,
    deadline: Instant,
) -> Result<()> {
    let millis = profile[format!("retry_{layer}_interval_ms")]
        .as_u64()
        .filter(|n| *n > 0)
        .unwrap_or_else(|| 500u64.saturating_mul(1u64 << attempt.min(20)).min(cap));
    owned(
        cancel,
        deadline,
        tokio::time::sleep(Duration::from_millis(millis.min(3_600_000))),
    )
    .await?;
    Ok(())
}
fn provider_error(status: u16, bytes: &[u8], secret: &str) -> Error {
    let parsed = serde_json::from_slice::<Value>(bytes).unwrap_or(Value::Null);
    let detail = &parsed["error"];
    let mut message = format!("LLM provider HTTP {status}");
    for key in ["type", "code", "message"] {
        if let Some(s) = detail[key].as_str().or_else(|| parsed[key].as_str()) {
            let safe = if secret.is_empty() {
                s.to_owned()
            } else {
                s.replace(secret, "[redacted]")
            };
            message.push_str(": ");
            message.extend(safe.chars().take(2048));
        }
    }
    let mut e = Error::new("EXTERNAL_FAILURE", &message);
    e.details = json!({"status":status,"committed":false});
    e
}
fn quota_error(e: &Error) -> bool {
    let s = e.message.to_lowercase();
    if matches!(
        e.details["status"].as_u64(),
        Some(401 | 403 | 500 | 502 | 503 | 504)
    ) {
        return false;
    }
    [
        "insufficient_quota",
        "quota_exceeded",
        "billing_hard_limit",
        "billing_not_active",
        "credit balance",
        "insufficient credit",
        "insufficient balance",
        "payment required",
        "http 402",
    ]
    .iter()
    .any(|x| s.contains(x))
}
pub fn generate(
    profile: &Value,
    secret: &str,
    prompt: &Prompt,
    cancellation: &Cancellation,
    timeout: Duration,
) -> Result<Turn> {
    generate_streaming(profile, secret, prompt, cancellation, timeout, &mut |_| {
        Ok(())
    })
}
/// Callback runs on the provider worker and may apply cancellable bounded backpressure.
/// Tool calls are deliberately absent from progress; only a verified complete Turn owns them.
pub fn generate_streaming(
    profile: &Value,
    secret: &str,
    prompt: &Prompt,
    cancellation: &Cancellation,
    timeout: Duration,
    progress: &mut dyn FnMut(StreamDelta) -> Result<()>,
) -> Result<Turn> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?
        .block_on(generate_async(
            profile,
            secret,
            prompt,
            cancellation,
            timeout,
            progress,
        ))
}
async fn generate_async(
    profile: &Value,
    secret: &str,
    prompt: &Prompt,
    cancellation: &Cancellation,
    timeout: Duration,
    progress: &mut dyn FnMut(StreamDelta) -> Result<()>,
) -> Result<Turn> {
    cancellation.check()?;
    let (url, body) = encode(profile, prompt)?;
    let mut builder = Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout);
    if let Some(proxy) = profile["proxy"].as_str().filter(|s| !s.is_empty()) {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid LLM proxy"))?,
        );
    }
    let client = builder
        .build()
        .map_err(|_| Error::new("EXTERNAL_FAILURE", "Cannot initialize LLM TLS client"))?;
    let format = profile["format"].as_str().unwrap_or("openai");
    let deadline = Instant::now() + timeout;
    let mut connect = 0;
    let mut empty = 0;
    let mut stream = 0;
    loop {
        cancellation.check()?;
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::new("TIMEOUT", "LLM deadline elapsed"));
        }
        let mut r = client.post(&url).json(&body).timeout(left);
        if format == "anthropic" {
            r = r
                .header("x-api-key", secret)
                .header("anthropic-version", "2023-06-01");
        } else if !secret.is_empty() {
            r = r.bearer_auth(secret);
        }
        if let Some(key) = profile["session_header_key"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            r = r.header(key, &prompt.session_id);
        }
        match owned(cancellation, deadline, r.send()).await? {
            Err(e)
                if (e.is_connect() || e.is_timeout())
                    && connect < retry_count(profile, "connect", 3) =>
            {
                retry_wait(profile, "connect", connect, 8000, cancellation, deadline).await?;
                connect += 1;
            }
            Err(_) => {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "LLM transport failed; unconfirmed generation is not replayed",
                ));
            }
            Ok(mut response) => {
                if !response.status().is_success() {
                    let status = response.status().as_u16();
                    let mut bytes = Vec::new();
                    while let Some(chunk) = owned(cancellation, deadline, response.chunk())
                        .await?
                        .map_err(|_| {
                        Error::new("EXTERNAL_FAILURE", "LLM error response interrupted")
                    })? {
                        let end = chunk.len().min(65536usize.saturating_sub(bytes.len()));
                        bytes.extend_from_slice(&chunk[..end]);
                        if bytes.len() >= 65536 {
                            break;
                        }
                    }
                    let e = provider_error(status, &bytes, secret);
                    if matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
                        && !quota_error(&e)
                        && connect < retry_count(profile, "connect", 3)
                    {
                        retry_wait(profile, "connect", connect, 8000, cancellation, deadline)
                            .await?;
                        connect += 1;
                        continue;
                    }
                    return Err(e);
                }
                let sse = response
                    .headers()
                    .get("content-type")
                    .and_then(|h| h.to_str().ok())
                    .is_some_and(|s| s.contains("text/event-stream"));
                let mut bytes = Vec::new();
                let mut decoder = streaming::SseDecoder::new(format);
                let mut failure = None;
                let mut received = 0u64;
                loop {
                    let chunk = match owned(cancellation, deadline, response.chunk()).await {
                        Ok(Ok(Some(chunk))) => chunk,
                        Ok(Ok(None)) => break,
                        Ok(Err(_)) => {
                            failure =
                                Some(Error::new("EXTERNAL_FAILURE", "LLM response interrupted"));
                            break;
                        }
                        Err(e) => {
                            failure = Some(e);
                            break;
                        }
                    };
                    received = received.saturating_add(chunk.len() as u64);
                    if received > MAX_RESPONSE {
                        failure = Some(Error::new("SIZE_LIMIT", "LLM response exceeds 16 MiB"));
                        break;
                    }
                    if sse {
                        if let Err(e) = decoder.feed(&chunk, progress) {
                            failure = Some(e);
                            break;
                        }
                    } else {
                        bytes.extend_from_slice(&chunk);
                    }
                }
                // Even without a final newline, observed output forbids replay.
                let committed = sse && decoder.committed();
                let result = if let Some(e) = failure {
                    if sse {
                        let _ = decoder.flush_pending(progress);
                    }
                    Err(e)
                } else if sse {
                    decoder.finish(progress)
                } else {
                    serde_json::from_slice(&bytes)
                        .map_err(Error::from)
                        .and_then(|v| decode(format, v))
                };
                let committed = committed || (sse && decoder.committed());
                let turn = match result {
                    Ok(turn) => turn,
                    Err(mut e) => {
                        if !secret.is_empty() {
                            e.message = e.message.replace(secret, "[redacted]");
                        }
                        e.details["committed"] = json!(committed);
                        if sse {
                            if let Some(observed) = decoder.observed_usage() {
                                e.details["usage"] = observed;
                            }
                        }
                        let deterministic = [
                            "too long",
                            "context length",
                            "context_length",
                            "maximum context",
                            "http 400",
                            "http 401",
                            "http 403",
                            "http 404",
                            "http 413",
                            "http 422",
                        ]
                        .iter()
                        .any(|s| e.message.to_lowercase().contains(s));
                        if !committed
                            && !deterministic
                            && e.code == "EXTERNAL_FAILURE"
                            && !quota_error(&e)
                            && stream < retry_count(profile, "stream", 2)
                        {
                            retry_wait(profile, "stream", stream, 4000, cancellation, deadline)
                                .await?;
                            stream += 1;
                            continue;
                        }
                        return Err(e);
                    }
                };
                if turn.text.is_empty() && turn.tools.is_empty() {
                    if empty_completion_class(&turn) != "empty_output" {
                        return Err(completed_empty_error(format, &turn));
                    }
                    if !turn.thinking.is_empty() {
                        return Ok(turn);
                    }
                    if empty < retry_count(profile, "empty", 2) {
                        retry_wait(profile, "empty", empty, 8000, cancellation, deadline).await?;
                        empty += 1;
                        continue;
                    }
                    return Err(completed_empty_error(format, &turn));
                }
                return Ok(turn);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_formats_and_streamed_tool_arguments() {
        let p = Prompt {
            system: "한국어".into(),
            messages: vec![Message {
                role: "user".into(),
                content: vec![json!({"type":"text","text":"hello"})],
            }],
            tools: vec![],
            session_id: "run_x".into(),
        };
        for format in ["openai", "anthropic", "openai-responses"] {
            let (url, b) = encode(
                &json!({"format":format,"model":"fixture","base_url":"http://127.0.0.1:1/v1"}),
                &p,
            )
            .unwrap();
            assert!(url.starts_with("http://127.0.0.1:1/v1/"));
            assert_eq!(b["model"], "fixture");
        }
        let text = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"Read\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n",
            "data: [DONE]\n"
        );
        let t = decode_sse("openai", text).unwrap();
        assert_eq!(t.tools[0].input, json!({"path":"a"}));
        assert!(
            decode_sse(
                "openai",
                "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n"
            )
            .is_err()
        );
    }
    #[test]
    fn responses_and_anthropic_preserve_tool_pairing() {
        let t=decode("anthropic",json!({"content":[{"type":"thinking","thinking":"hidden","signature":"sig"},{"type":"tool_use","id":"x","name":"Read","input":{"path":"a"}}],"usage":{"input_tokens":2},"stop_reason":"tool_use"})).unwrap();
        assert_eq!(t.tools[0].id, "x");
        assert_eq!(t.thinking_signature, "sig");
        let t=decode("openai-responses",json!({"status":"completed","output":[{"type":"function_call","call_id":"a","name":"Read","arguments":"{}"}],"usage":{"input_tokens":3}})).unwrap();
        assert_eq!(t.tools[0].id, "a");
    }
}
