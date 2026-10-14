//! Incremental UTF-8/SSE framing; the network SSE body is never buffered whole.
use super::*;
use std::collections::BTreeMap;
#[derive(Clone, Debug, PartialEq)]
pub enum StreamDelta {
    Text(String),
    Thinking(String),
    Usage(Value),
}
const MAX_EVENT: usize = MAX_RESPONSE as usize;
const DELTA: usize = 16 * 1024;
pub(super) struct SseDecoder {
    format: String,
    line: Vec<u8>,
    data: String,
    open: Value,
    anth: Value,
    blocks: BTreeMap<usize, Value>,
    arguments: BTreeMap<usize, String>,
    completed: Option<Value>,
    committed: bool,
}
impl SseDecoder {
    pub fn new(format: &str) -> Self {
        Self {
            format: format.into(),
            line: vec![],
            data: String::new(),
            open: json!({"choices":[{"message":{"content":"","reasoning_content":"","tool_calls":[]},"finish_reason":null}],"usage":{}}),
            anth: json!({"content":[],"usage":{},"stop_reason":null}),
            blocks: BTreeMap::new(),
            arguments: BTreeMap::new(),
            completed: None,
            committed: false,
        }
    }
    pub fn committed(&self) -> bool {
        self.committed
    }
    /// Only provider-reported counters are returned; unknown partial usage is
    /// absent rather than an invented zero-token completion.
    pub fn observed_usage(&self) -> Option<Value> {
        let raw = match self.format.as_str() {
            "anthropic" => &self.anth["usage"],
            "openai-responses" => &self.completed.as_ref()?["usage"],
            _ => &self.open["usage"],
        };
        raw.as_object()
            .filter(|o| !o.is_empty())
            .map(|_| usage(raw))
    }
    pub fn feed(
        &mut self,
        bytes: &[u8],
        emit: &mut dyn FnMut(StreamDelta) -> Result<()>,
    ) -> Result<()> {
        for &b in bytes {
            if b == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.line(&line, emit)?;
            } else {
                self.line.push(b);
                if self.line.len() > MAX_EVENT {
                    return Err(Error::new("SIZE_LIMIT", "SSE line exceeds 16 MiB"));
                }
            }
        }
        Ok(())
    }
    fn line(&mut self, line: &[u8], emit: &mut dyn FnMut(StreamDelta) -> Result<()>) -> Result<()> {
        let line = std::str::from_utf8(line)
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Invalid SSE UTF-8"))?
            .trim_end_matches('\r');
        if line.is_empty() {
            return self.event(emit);
        }
        if let Some(data) = line.strip_prefix("data:") {
            // Compatible servers sometimes omit the blank separator between JSON events.
            if !self.data.is_empty()
                && (self.data == "[DONE]" || serde_json::from_str::<Value>(&self.data).is_ok())
            {
                self.event(emit)?;
            }
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            if self.data.len() > MAX_EVENT {
                return Err(Error::new("SIZE_LIMIT", "SSE event exceeds 16 MiB"));
            }
        }
        Ok(())
    }
    fn event(&mut self, emit: &mut dyn FnMut(StreamDelta) -> Result<()>) -> Result<()> {
        let data = std::mem::take(&mut self.data);
        if data.is_empty() || data.trim() == "[DONE]" {
            return Ok(());
        }
        let e: Value = serde_json::from_str(&data)
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Malformed SSE event JSON"))?;
        if e["error"].is_object() || e["type"] == "error" {
            return Err(provider_error(200, &serde_json::to_vec(&e)?, ""));
        }
        match self.format.as_str() {
            "openai-responses" => match e["type"].as_str() {
                Some("response.output_text.delta") => {
                    self.progress(false, e["delta"].as_str().unwrap_or(""), emit)?
                }
                Some("response.reasoning_text.delta" | "response.reasoning_summary_text.delta") => {
                    self.progress(true, e["delta"].as_str().unwrap_or(""), emit)?
                }
                Some("response.function_call_arguments.delta") => self.committed = true,
                Some("response.output_item.added") if e["item"]["type"] == "function_call" => {
                    self.committed = true
                }
                Some("response.completed") => {
                    self.committed = true;
                    self.completed = Some(e["response"].clone());
                    emit(StreamDelta::Usage(usage(&e["response"]["usage"])))?;
                }
                Some("response.failed" | "response.incomplete" | "response.cancelled") => {
                    self.committed = true;
                    self.completed = Some(e["response"].clone());
                    return Err(Error::new(
                        "EXTERNAL_FAILURE",
                        "Responses stream did not complete",
                    ));
                }
                _ => {}
            },
            "anthropic" => match e["type"].as_str() {
                Some("message_start") => {
                    self.anth["usage"] = e["message"]["usage"].clone();
                    emit(StreamDelta::Usage(usage(&self.anth["usage"])))?;
                }
                Some("content_block_start") => {
                    let i = index(&e)?;
                    let block = e["content_block"].clone();
                    if block["type"] == "tool_use" {
                        self.committed = true;
                    }
                    if block["type"] == "text" {
                        self.progress(false, block["text"].as_str().unwrap_or(""), emit)?;
                    }
                    if block["type"] == "thinking" {
                        self.progress(true, block["thinking"].as_str().unwrap_or(""), emit)?;
                    }
                    if self.blocks.insert(i, block).is_some() {
                        return Err(Error::new("EXTERNAL_FAILURE", "Repeated SSE block index"));
                    }
                }
                Some("content_block_delta") => {
                    let i = index(&e)?;
                    let d = &e["delta"];
                    let (target, source) = match d["type"].as_str() {
                        Some("thinking_delta") => ("thinking", "thinking"),
                        Some("signature_delta") => ("signature", "signature"),
                        Some("input_json_delta") => ("input", "partial_json"),
                        _ => ("text", "text"),
                    };
                    let piece = d[source].as_str().unwrap_or("");
                    if target == "input" {
                        if !piece.is_empty() {
                            self.committed = true;
                        }
                        self.arguments.entry(i).or_default().push_str(piece);
                    } else {
                        if target != "signature" {
                            self.progress(target == "thinking", piece, emit)?;
                        }
                        let b = self.blocks.get_mut(&i).ok_or_else(|| {
                            Error::new("EXTERNAL_FAILURE", "SSE delta precedes its block")
                        })?;
                        append(&mut b[target], piece);
                    }
                }
                Some("message_delta") => {
                    self.committed = true;
                    self.anth["stop_reason"] = e["delta"]["stop_reason"].clone();
                    if let Some(u) = e["usage"].as_object() {
                        for (k, v) in u {
                            self.anth["usage"][k] = v.clone();
                        }
                    }
                    emit(StreamDelta::Usage(usage(&self.anth["usage"])))?;
                }
                _ => {}
            },
            _ => {
                let c = &e["choices"][0];
                let d = &c["delta"];
                for (field, thinking) in [("content", false), ("reasoning_content", true)] {
                    if let Some(piece) = d[field].as_str() {
                        self.progress(thinking, piece, emit)?;
                        append(&mut self.open["choices"][0]["message"][field], piece);
                    }
                }
                if !c["finish_reason"].is_null() {
                    self.committed = true;
                    self.open["choices"][0]["finish_reason"] = c["finish_reason"].clone();
                }
                if e["usage"].is_object() {
                    self.open["usage"] = e["usage"].clone();
                    emit(StreamDelta::Usage(usage(&e["usage"])))?;
                }
                for b in d["tool_calls"].as_array().into_iter().flatten() {
                    self.committed = true;
                    let i = index(b)?;
                    let target = self.blocks.entry(i).or_insert_with(
                        || json!({"id":"","type":"function","function":{"name":"","arguments":""}}),
                    );
                    if let Some(id) = b["id"].as_str() {
                        target["id"] = json!(id);
                    }
                    for k in ["name", "arguments"] {
                        if let Some(piece) = b["function"][k].as_str() {
                            append(&mut target["function"][k], piece);
                        }
                    }
                }
            }
        }
        Ok(())
    }
    fn progress(
        &mut self,
        thinking: bool,
        piece: &str,
        emit: &mut dyn FnMut(StreamDelta) -> Result<()>,
    ) -> Result<()> {
        if piece.is_empty() {
            return Ok(());
        }
        self.committed = true;
        let mut rest = piece;
        while !rest.is_empty() {
            let mut end = rest.len().min(DELTA);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let part = rest[..end].to_owned();
            emit(if thinking {
                StreamDelta::Thinking(part)
            } else {
                StreamDelta::Text(part)
            })?;
            rest = &rest[end..];
        }
        Ok(())
    }
    pub fn flush_pending(&mut self, emit: &mut dyn FnMut(StreamDelta) -> Result<()>) -> Result<()> {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.line(&line, emit)?;
        }
        self.event(emit)
    }
    pub fn finish(&mut self, emit: &mut dyn FnMut(StreamDelta) -> Result<()>) -> Result<Turn> {
        self.flush_pending(emit)?;
        if self.format == "openai-responses" {
            return decode(
                &self.format,
                self.completed.take().ok_or_else(|| {
                    Error::new(
                        "EXTERNAL_FAILURE",
                        "Missing terminal response; automatic replay is forbidden",
                    )
                })?,
            );
        }
        if self.format == "anthropic" {
            for (i, arg) in std::mem::take(&mut self.arguments) {
                let b = self.blocks.get_mut(&i).ok_or_else(|| {
                    Error::new("EXTERNAL_FAILURE", "Tool arguments have no matching block")
                })?;
                b["input"] = serde_json::from_str(&arg)
                    .map_err(|_| Error::new("EXTERNAL_FAILURE", "Incomplete tool arguments"))?;
            }
            self.anth["content"] = json!(
                std::mem::take(&mut self.blocks)
                    .into_values()
                    .collect::<Vec<_>>()
            );
            if self.anth["stop_reason"].is_null() {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Incomplete Anthropic stream",
                ));
            }
            decode(&self.format, self.anth.clone())
        } else {
            self.open["choices"][0]["message"]["tool_calls"] = json!(
                std::mem::take(&mut self.blocks)
                    .into_values()
                    .collect::<Vec<_>>()
            );
            if self.open["choices"][0]["finish_reason"].is_null() {
                return Err(Error::new("EXTERNAL_FAILURE", "Incomplete OpenAI stream"));
            }
            decode(&self.format, self.open.clone())
        }
    }
}
fn index(e: &Value) -> Result<usize> {
    let i = e["index"].as_u64().unwrap_or(0);
    if i > 1024 {
        return Err(Error::new("SIZE_LIMIT", "SSE block index exceeds 1024"));
    }
    Ok(i as usize)
}

fn append(value: &mut Value, piece: &str) {
    if let Value::String(text) = value {
        text.push_str(piece);
    } else {
        *value = json!(piece);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(value: Value) -> String {
        format!("data: {value}\r\n\r\n")
    }
    #[test]
    fn unicode_chunk_splits_multiline_events_and_three_format_reasoning() {
        let open = event(
            json!({"choices":[{"delta":{"content":"한글🙂","reasoning_content":"생각"}}]}),
        ) + &event(
            json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":2}}),
        );
        let anth = event(json!({"type":"message_start","message":{"usage":{"input_tokens":5}}}))
            + &event(
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            )
            + &event(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"생각"}}),
            )
            + &event(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"signed"}}),
            )
            + &event(
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            )
            + &event(
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"한글🙂"}}),
            )
            + &event(
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
            );
        let responses = event(json!({"type":"response.output_text.delta","delta":"한글🙂"}))
            + &event(json!({"type":"response.reasoning_summary_text.delta","delta":"생각"}))
            + &event(
                json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"한글🙂"}]},{"type":"reasoning","summary":[{"type":"summary_text","text":"생각"}]}],"usage":{"input_tokens":6,"output_tokens":3}}}),
            );
        for (format, bytes) in [
            ("openai", open),
            ("anthropic", anth),
            ("openai-responses", responses),
        ] {
            let mut decoder = SseDecoder::new(format);
            let mut seen = vec![];
            for byte in bytes.as_bytes() {
                decoder
                    .feed(&[*byte], &mut |delta| {
                        seen.push(delta);
                        Ok(())
                    })
                    .unwrap();
            }
            assert!(
                seen.iter()
                    .any(|d| d == &StreamDelta::Text("한글🙂".into()))
            );
            assert!(
                seen.iter()
                    .any(|d| d == &StreamDelta::Thinking("생각".into()))
            );
            let turn = decoder.finish(&mut |_| Ok(())).unwrap();
            assert_eq!(turn.text, "한글🙂");
            assert_eq!(turn.thinking, "생각");
            if format == "anthropic" {
                assert_eq!(turn.thinking_signature, "signed");
                assert_eq!(turn.usage["input_tokens"], 5);
            }
        }
        let multiline = "data: {\"choices\": [\r\ndata: {\"delta\": {\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\r\n\r\n";
        assert_eq!(decode_sse("openai", multiline).unwrap().text, "hello");
    }
    #[test]
    fn tool_arguments_are_never_progress_and_callback_failure_forbids_replay() {
        let mut d = SseDecoder::new("openai");
        let mut seen = vec![];
        let input = event(
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"Read","arguments":"{\"file_path\":"}}]}}]}),
        );
        d.feed(input.as_bytes(), &mut |delta| {
            seen.push(delta);
            Ok(())
        })
        .unwrap();
        assert!(d.committed());
        assert!(seen.is_empty());
        assert!(d.finish(&mut |_| Ok(())).is_err());
        let mut d = SseDecoder::new("openai");
        assert_eq!(
            d.feed(
                event(json!({"choices":[{"delta":{"content":"observed"}}]})).as_bytes(),
                &mut |_| Err(Error::new("CANCELLED", "test"))
            )
            .unwrap_err()
            .code,
            "CANCELLED"
        );
        assert!(d.committed());
    }
    #[test]
    fn interrupted_provider_reports_only_observed_usage_in_all_three_formats() {
        let mut decoder = SseDecoder::new("anthropic");
        assert!(decoder.observed_usage().is_none());
        decoder.feed(event(json!({"type":"message_start","message":{"usage":{"input_tokens":17,"cache_read_input_tokens":3}}})).as_bytes(),&mut |_|Ok(())).unwrap();
        decoder
            .feed(
                event(json!({"type":"message_delta","usage":{"output_tokens":5}})).as_bytes(),
                &mut |_| Ok(()),
            )
            .unwrap();
        assert!(decoder.finish(&mut |_| Ok(())).is_err());
        assert_eq!(decoder.observed_usage().unwrap()["input_tokens"], 17);
        assert_eq!(decoder.observed_usage().unwrap()["output_tokens"], 5);
        assert_eq!(decoder.observed_usage().unwrap()["cache_read_tokens"], 3);
        let mut decoder = SseDecoder::new("openai-responses");
        let error=decoder.feed(event(json!({"type":"response.incomplete","response":{"status":"incomplete","usage":{"input_tokens":9,"output_tokens":2}}})).as_bytes(),&mut |_|Ok(())).unwrap_err();
        assert_eq!(error.code, "EXTERNAL_FAILURE");
        assert_eq!(decoder.observed_usage().unwrap()["output_tokens"], 2);
        let mut decoder = SseDecoder::new("openai");
        decoder
            .feed(
                event(json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}))
                    .as_bytes(),
                &mut |_| Ok(()),
            )
            .unwrap();
        assert_eq!(decoder.observed_usage().unwrap()["input_tokens"], 11);
    }
    #[test]
    fn actual_failed_sse_returns_usage_without_replaying_observed_output() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0];
                socket.read_exact(&mut b).unwrap();
                head.push(b[0]);
                assert!(head.len() < 65536);
            }
            let count = String::from_utf8(head)
                .unwrap()
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap();
            let mut body = vec![0; count];
            socket.read_exact(&mut body).unwrap();
            let body = event(json!({"choices":[{"delta":{"content":"observed partial"}}]}))
                + &event(json!({"choices":[],"usage":{"prompt_tokens":19,"completion_tokens":4}}));
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let prompt = Prompt {
            system: "fixture".into(),
            messages: vec![Message {
                role: "user".into(),
                content: vec![json!({"type":"text","text":"fixture"})],
            }],
            tools: vec![],
            session_id: "fixture".into(),
        };
        let mut observed = Vec::new();
        let error=generate_streaming(&json!({"format":"openai","model":"fixture","base_url":url,"streaming":true,"retry_stream_attempts":2}),"fixture-key",&prompt,&Cancellation::default(),std::time::Duration::from_secs(3),&mut |v|{observed.push(v);Ok(())}).unwrap_err();
        server.join().unwrap();
        assert_eq!(error.details["committed"], true);
        assert_eq!(error.details["usage"]["input_tokens"], 19);
        assert_eq!(error.details["usage"]["output_tokens"], 4);
        assert!(
            observed
                .iter()
                .any(|v| v == &StreamDelta::Text("observed partial".into()))
        );
    }
}
