//! Bounded worker progress becomes display-only activity on the single writer.
//! Partial output never has a `message` field and cannot become replayable tools.
use super::*;
use artex_domain::{Planner, record, text};
use artex_runtime::{Cancellation, provider::StreamDelta};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
const CHUNK: usize = 16 * 1024;
const QUEUE: usize = 64;
const ROW_LIMIT: usize = 4096;
#[derive(Default)]
pub(super) struct StreamState {
    runs: BTreeMap<String, Pending>,
    budget: Arc<AtomicUsize>,
}
struct Pending {
    workspace: String,
    epoch: String,
    task: Option<String>,
    turn: String,
    receiver: mpsc::Receiver<Packet>,
    cancel: Cancellation,
    rows: Vec<(String, String)>,
    total: usize,
}

struct Reservation {
    budget: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
struct Packet {
    delta: StreamDelta,
    _reservation: Reservation,
}
pub(super) struct StreamSink {
    sender: mpsc::SyncSender<Packet>,
    cancel: Cancellation,
    budget: Arc<AtomicUsize>,
}
impl StreamSink {
    pub(super) fn emit(&self, delta: StreamDelta) -> Result<()> {
        let bytes = match &delta {
            StreamDelta::Text(s) | StreamDelta::Thinking(s) => s.len(),
            StreamDelta::Usage(v) => serde_json::to_vec(v)?.len(),
        };
        if bytes > CHUNK {
            return Err(Error::new("SIZE_LIMIT", "Stream progress exceeds 16 KiB"));
        }
        let started = Instant::now();
        let allocated = bytes + 256;
        loop {
            self.cancel.check()?;
            let used = self.budget.load(Ordering::Acquire);
            if used + allocated <= MAX_QUEUE_BYTES
                && self
                    .budget
                    .compare_exchange_weak(
                        used,
                        used + allocated,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err(Error::new("BUSY", "Aggregate stream queue exceeds 16 MiB"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut packet = Packet {
            delta,
            _reservation: Reservation {
                budget: self.budget.clone(),
                bytes: allocated,
            },
        };
        loop {
            self.cancel.check()?;
            match self.sender.try_send(packet) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return Err(Error::new("CANCELLED", "Stream writer was closed"));
                }
                Err(mpsc::TrySendError::Full(value)) => packet = value,
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err(Error::new(
                    "BUSY",
                    "Stream writer backpressure exceeded 5 seconds",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Router {
    pub(super) fn begin_stream(&mut self, rid: &str) -> Result<StreamSink> {
        if self.stream_state.runs.contains_key(rid) {
            return Err(Error::new("BUSY", "Model stream is already active"));
        }
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        if run.data["engine_epoch"] != self.engine_epoch {
            return Err(Error::new(
                "STALE_EPOCH",
                "Stream belongs to another engine epoch",
            ));
        }
        let cancel = self
            .runtime
            .runs
            .get(rid)
            .ok_or_else(|| Error::new("CANCELLED", "Runtime run was removed"))?
            .cancellation
            .clone();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        self.stream_state.runs.insert(
            rid.into(),
            Pending {
                workspace: s.workspace_id.clone(),
                epoch: self.engine_epoch.clone(),
                task: run.data["task_id"].as_str().map(str::to_owned),
                turn: new_id("turn"),
                receiver,
                cancel: cancel.clone(),
                rows: vec![],
                total: 0,
            },
        );
        Ok(StreamSink {
            sender,
            cancel,
            budget: self.stream_state.budget.clone(),
        })
    }
    pub(super) fn stream_marker(&self, rid: &str) -> Option<Value> {
        self.stream_state
            .runs
            .get(rid)
            .map(|p| json!({"run_id":rid,"turn_id":p.turn,"pending":false}))
    }
    pub(super) fn process_streams(&mut self) -> Result<()> {
        if self.store.is_none() || self.read_only {
            self.stream_state = Default::default();
            return Ok(());
        }
        let ids = self.stream_state.runs.keys().cloned().collect::<Vec<_>>();
        for rid in ids {
            let frozen = self.stream_state.runs[&rid]
                .task
                .as_ref()
                .is_some_and(|id| self.deleting_tasks.contains(id));
            if frozen || self.lifecycle_frozen(&rid) {
                continue;
            }
            let mut pending = self.stream_state.runs.remove(&rid).unwrap();
            let result = self.drain_stream(&rid, &mut pending);
            self.stream_state.runs.insert(rid, pending);
            result?;
        }
        Ok(())
    }
    fn drain_stream(&mut self, rid: &str, p: &mut Pending) -> Result<()> {
        let s = self.snapshot()?;
        if p.workspace != s.workspace_id || p.epoch != self.engine_epoch {
            p.cancel.cancel();
            return Ok(());
        }
        let Some(run) = s.get("agent_runs", rid) else {
            p.cancel.cancel();
            return Ok(());
        };
        if !matches!(text(&run.data, "state"), "running" | "waiting") {
            return Ok(());
        }
        let mut groups: Vec<StreamDelta> = vec![];
        for _ in 0..QUEUE {
            let Ok(packet) = p.receiver.try_recv() else {
                break;
            };
            let Packet {
                delta,
                _reservation,
            } = packet;
            let merged = match (groups.last_mut(), &delta) {
                (Some(StreamDelta::Text(last)), StreamDelta::Text(next))
                | (Some(StreamDelta::Thinking(last)), StreamDelta::Thinking(next))
                    if last.len() + next.len() <= CHUNK =>
                {
                    last.push_str(next);
                    true
                }
                // Usage is cumulative within this provider turn; a later value replaces it.
                (Some(StreamDelta::Usage(last)), StreamDelta::Usage(next)) => {
                    *last = next.clone();
                    true
                }
                _ => false,
            };
            if !merged {
                groups.push(delta);
            }
        }
        for delta in groups {
            if p.rows.len() >= ROW_LIMIT {
                p.cancel.cancel();
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Stream exceeds 4096 activity fragments",
                ));
            }
            let (kind, value, usage) = match delta {
                StreamDelta::Text(v) => ("text", v, None),
                StreamDelta::Thinking(v) => ("thinking", v, None),
                StreamDelta::Usage(v) => ("usage", String::new(), Some(v)),
            };
            p.total = p.total.saturating_add(value.len());
            if p.total > 16 * 1024 * 1024 {
                p.cancel.cancel();
                return Err(Error::new("SIZE_LIMIT", "Stream text exceeds 16 MiB"));
            }
            let marker = json!({"run_id":rid,"turn_id":p.turn,"pending":true});
            let detail = artex_core::body(
                &json!({"stream":marker,"text":value,"usage_progress":usage}).to_string(),
            );
            let summary = value
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            let s = self.snapshot()?;
            let c = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
            let batch = artex_domain::conversation::append_activity(
                &s, &c, rid, kind, &summary, &detail, None,
            )?;
            let id = batch.result["activity_id"]
                .as_str()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Stream activity identity missing"))?
                .to_owned();
            let collection = if c.conversation_id.is_some() {
                "conversation_activities"
            } else {
                "activity"
            };
            self.internal_commit(batch)?;
            p.rows.push((collection.into(), id));
        }
        Ok(())
    }
    /// Called immediately before the verified assistant batch commits. The same
    /// transaction removes partial display rows and publishes the legal transcript.
    pub(super) fn decorate_stream_batch(
        &mut self,
        rid: &str,
        batch: &mut CommandBatch,
    ) -> Result<()> {
        let Some(p) = self.stream_state.runs.get(rid) else {
            return Ok(());
        };
        let s = self.snapshot()?;
        if p.workspace != s.workspace_id || p.epoch != self.engine_epoch {
            return Err(Error::new("STALE_EPOCH", "Stream changed workspace"));
        }
        let context = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
        for (collection, id) in &p.rows {
            if let Some(row) = s.get(collection, id) {
                artex_domain::conversation::push_event(
                    batch,
                    &s,
                    &context,
                    "ActivityReplaced",
                    &collection,
                    &id,
                )?;
                batch.changes.push(artex_core::Change::Delete {
                    collection: collection.clone(),
                    id: id.clone(),
                });
                batch.expected.push(Expected {
                    collection: collection.clone(),
                    id: id.clone(),
                    version: row.header.version.clone(),
                });
            }
        }
        // Cumulative partial usage never contributes another completed turn total.
        Ok(())
    }
    /// Drop the worker queue only after the assistant replacement CAS is durable.
    pub(super) fn finalize_stream(&mut self, rid: &str) {
        self.stream_state.runs.remove(rid);
    }
    pub(super) fn finish_stream(&mut self, rid: &str, success: bool) -> Result<()> {
        if success {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Successful stream must be finished atomically with its transcript",
            ));
        }
        let Some(mut pending) = self.stream_state.runs.remove(rid) else {
            return Ok(());
        };
        if self.store.is_none() || self.read_only {
            pending.cancel.cancel();
            return Ok(());
        }
        if pending.workspace != self.snapshot()?.workspace_id || pending.epoch != self.engine_epoch
        {
            pending.cancel.cancel();
            return Ok(());
        }
        let result = (|| {
            // Do not turn cancelled or overflowing progress into replayable context.
            if let Err(error) = self.drain_stream(rid, &mut pending) {
                if !matches!(
                    error.code.as_str(),
                    "SIZE_LIMIT" | "CANCELLED" | "PERMISSION_DENIED"
                ) {
                    return Err(error);
                }
            }
            let batch = self.stream_failure_batch(rid, &pending.turn, &pending.rows)?;
            if !batch.changes.is_empty() {
                self.internal_commit(batch)?;
            }
            Ok(())
        })();
        if result.is_err() {
            // A failed storage transition must leave the retry/recovery locator alive.
            self.stream_state.runs.insert(rid.into(), pending);
        }
        result
    }
    /// Recover display-only fragments after opening a workspace. This never
    /// reconstructs provider messages or tool calls, including on a same-epoch reopen.
    pub(super) fn recover_streams(&mut self) -> Result<()> {
        if self.store.is_none()
            || self.read_only
            || self.store.as_ref().is_some_and(Store::read_only)
        {
            return Ok(());
        }
        let s = self.snapshot()?;
        let mut groups: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
        for collection in ["activity", "conversation_activities"] {
            for row in s.records(collection) {
                let raw = self.body_text(&row.data["detail"])?;
                let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                    continue;
                };
                if v["stream"]["pending"] != true || !v["message"].is_null() {
                    continue;
                }
                if let (Some(run), Some(turn)) = (
                    v["stream"]["run_id"].as_str(),
                    v["stream"]["turn_id"].as_str(),
                ) {
                    groups
                        .entry((run.into(), turn.into()))
                        .or_default()
                        .push((collection.into(), row.header.id.clone()));
                }
            }
        }
        for ((run, turn), mut rows) in groups {
            rows.sort_by_key(|(collection, id)| {
                let r = s.get(collection, id).unwrap();
                (
                    r.header.commit_seq.parse::<u64>().unwrap_or(0),
                    r.header.created_at.clone(),
                    id.clone(),
                )
            });
            let batch = self.stream_failure_batch(&run, &turn, &rows)?;
            if !batch.changes.is_empty() {
                self.internal_commit(batch)?;
            }
        }
        Ok(())
    }
    fn stream_failure_batch(
        &self,
        rid: &str,
        turn: &str,
        rows: &[(String, String)],
    ) -> Result<CommandBatch> {
        let s = self.snapshot()?;
        let mut plan = Planner::new(
            &s,
            "run.stream.partial_failure",
            &json!({"run_id":rid,"turn_id":turn}),
        );
        let context = Context {
            actor: Actor {
                kind: "engine".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: s
                .get("agent_runs", rid)
                .and_then(|r| r.data["task_id"].as_str())
                .map(str::to_owned),
            intent_id: None,
            conversation_id: None,
        };
        // Keep thinking in its private kind, rather than mixing it into public text.
        let mut collected: BTreeMap<(String, String), (Value, String)> = BTreeMap::new();
        for (collection, id) in rows {
            let Some(row) = s.get(collection, id) else {
                continue;
            };
            let raw = self.body_text(&row.data["detail"])?;
            let Ok(v) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if v["stream"]["run_id"] != rid
                || v["stream"]["turn_id"] != turn
                || v["stream"]["pending"] != true
                || !v["message"].is_null()
            {
                continue;
            }
            let key = (collection.clone(), text(&row.data, "kind").to_owned());
            let entry = collected
                .entry(key)
                .or_insert_with(|| (row.data.clone(), String::new()));
            entry.1.push_str(v["text"].as_str().unwrap_or(""));
            plan.delete(collection, id);
            artex_domain::conversation::push_event(
                &mut plan.batch,
                &s,
                &context,
                "ActivityReplaced",
                collection,
                id,
            )?;
        }
        for ((collection, kind), (mut data, received)) in collected {
            data["summary"] = json!(received.chars().take(200).collect::<String>());
            data["detail"]=artex_core::body(&json!({"stream":{"run_id":rid,"turn_id":turn,"pending":false},"text":received,"diagnostic":true}).to_string());
            data["is_error"] = json!(true);
            for field in [
                "input_tokens",
                "output_tokens",
                "cache_read_tokens",
                "cache_write_tokens",
            ] {
                data[field] = Value::Null;
            }
            data["kind"] = json!(kind);
            let prefix = if collection == "activity" {
                "activity"
            } else {
                "conversationactivity"
            };
            let id = plan.insert(&collection, prefix, data)?;
            artex_domain::conversation::push_event(
                &mut plan.batch,
                &s,
                &context,
                "ActivityAppended",
                &collection,
                &id,
            )?;
        }
        Ok(plan.done(json!({"partial_retained":true})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_runtime::HostGateway;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        path::Path,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct Host {
        effects: AtomicUsize,
    }
    impl HostGateway for Host {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            _: Value,
            _: Duration,
        ) -> Result<Value> {
            match operation {
                "secret.resolve" => Ok(json!({"value":"local-stream-test-key"})),
                "run.stop" => Ok(json!({"accepted":true})),
                "host.events" => Ok(json!({"events":[]})),
                "traffic.configure" => Ok(json!({"enabled":false})),
                "traffic.poll" => Ok(json!({"exchanges":[]})),
                "traffic.ack" => Ok(json!({"acknowledged":true})),
                "logs.poll" => Ok(json!({"items":[],"dropped":"0","capture_error":null})),
                "logs.ack" => Ok(json!({"acknowledged":0})),
                "tool.execute" => {
                    self.effects.fetch_add(1, Ordering::SeqCst);
                    Err(Error::new(
                        "EXTERNAL_FAILURE",
                        "Unexpected unconfirmed tool effect",
                    ))
                }
                _ => panic!("Unexpected native operation {operation}"),
            }
        }
    }
    fn request(socket: &mut TcpStream) {
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut head = vec![];
        while !head.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            socket.read_exact(&mut b).unwrap();
            head.push(b[0]);
            assert!(head.len() < 65536);
        }
        let n = String::from_utf8(head)
            .unwrap()
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap();
        let mut body = vec![0; n];
        socket.read_exact(&mut body).unwrap();
    }
    fn event(v: Value) -> String {
        format!("data: {v}\n\n")
    }
    fn chunk(socket: &mut TcpStream, value: &str) {
        write!(socket, "{:x}\r\n{value}\r\n", value.len()).unwrap();
        socket.flush().unwrap();
    }
    fn query(router: &mut Router, cid: &str, since: Option<&str>) -> Value {
        let mut params = json!({"id":cid});
        if let Some(id) = since {
            params["since"] = json!(id);
        }
        router.response(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":"ui.conversationMessages","workspace_id":router.snapshot().unwrap().workspace_id,"engine_epoch":router.engine_epoch,"command_id":null,"expected":[],"timeout_ms":5000,"params":params}))
    }
    fn configured(url: &str) -> (tempfile::TempDir, Router, String, String, Arc<Host>) {
        let (d, mut router, rid, _) = crate::regression::seeded();
        let s = router.snapshot().unwrap();
        let run = record(&s, "agent_runs", &rid).unwrap();
        let cid = text(&run.data, "conversation_id").to_owned();
        let mut p = Planner::new(&s, "test.streaming", &json!({}));
        let pid=p.insert("llm_profiles","profile",json!({"name":"localhost fixture","format":"openai","model":"fixture","base_url":url,"streaming":true,"is_default":true,"api_key":{"secret_ref":new_id("secret"),"hint":"fixture"},"retry_stream_attempts":2})).unwrap();
        let aid = text(&run.data, "agent_id");
        let mut agent = record(&s, "agents", aid).unwrap().data.clone();
        agent["llm_profile_id"] = json!(pid);
        p.put("agents", aid, agent);
        let mut data = run.data.clone();
        data["state"] = json!("queued");
        p.put("agent_runs", &rid, data);
        router.internal_commit(p.done(json!({}))).unwrap();
        let host = Arc::new(Host {
            effects: AtomicUsize::new(0),
        });
        router.attach_host(host.clone());
        (d, router, rid, cid, host)
    }
    #[test]
    fn delayed_sse_is_visible_before_completion_and_final_replaces_partial_cursor_after_restart() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (gate_tx, gate_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            chunk(
                &mut socket,
                &event(json!({"choices":[{"delta":{"reasoning_content":"생각 중"}}]})),
            );
            chunk(
                &mut socket,
                &event(json!({"choices":[{"delta":{"content":"완료 전 표시"}}]})),
            );
            gate_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            chunk(
                &mut socket,
                &event(json!({"choices":[{"delta":{"content":" · 최종 응답"}}]})),
            );
            chunk(
                &mut socket,
                &event(
                    json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":9,"completion_tokens":4}}),
                ),
            );
            chunk(&mut socket, "data: [DONE]\n\n");
            socket.write_all(b"0\r\n\r\n").unwrap();
        });
        let (dir, mut router, rid, cid, host) = configured(&url);
        let deadline = Instant::now() + Duration::from_secs(10);
        let cursor = loop {
            router.maintain().unwrap();
            let result = query(&mut router, &cid, None);
            assert!(result["error"].is_null(), "{result}");
            if let Some(row) = result["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["stream"]["pending"] == true && r["kind"] == "text")
            {
                assert_eq!(row["summary"], "완료 전 표시");
                break row["seq"].as_str().unwrap().to_owned();
            }
            assert!(Instant::now() < deadline, "No live stream activity");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &rid)
                .unwrap()
                .data["state"],
            "running"
        );
        assert!(
            router.context_messages(&rid).unwrap().is_empty(),
            "Partial stream must not enter the raw transcript"
        );
        gate_tx.send(()).unwrap();
        loop {
            router.maintain().unwrap();
            if router
                .snapshot()
                .unwrap()
                .get("agent_runs", &rid)
                .unwrap()
                .data["state"]
                == "completed"
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        server.join().unwrap();
        let result = query(&mut router, &cid, Some(&cursor));
        assert!(result["error"].is_null(), "{result}");
        let items = result["result"]["items"].as_array().unwrap();
        let final_row = items
            .iter()
            .find(|r| r["stream"]["pending"] == false)
            .unwrap();
        assert!(items.iter().all(|r| r["stream"]["pending"] != true));
        assert!(final_row["summary"].as_str().unwrap().contains("최종 응답"));
        let messages = router.context_messages(&rid).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content[0]["thinking"], "생각 중");
        assert_eq!(messages[0].content[1]["text"], "완료 전 표시 · 최종 응답");
        assert_eq!(
            router.snapshot().unwrap().records("llm_usage")[0].data["output_tokens"],
            4
        );
        assert_eq!(host.effects.load(Ordering::SeqCst), 0);
        router.store.as_mut().unwrap().checkpoint().unwrap();
        router.store = None;
        router.store = Some(Store::open(&dir.path().join("store")).unwrap());
        assert!(query(&mut router, &cid, Some(&cursor))["error"].is_null());
    }
    #[test]
    fn broken_stream_keeps_partial_diagnostics_without_replay_or_tool_effect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (gate_tx, gate_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            request(&mut socket);
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            chunk(
                &mut socket,
                &event(
                    json!({"choices":[{"delta":{"content":"부분 응답","tool_calls":[{"index":0,"id":"partial-tool","function":{"name":"Read","arguments":"{\"file_path\":\"never.txt\"}"}}]}}]}),
                ),
            );
            gate_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            drop(socket);
            listener.set_nonblocking(true).unwrap();
            let until = Instant::now() + Duration::from_millis(500);
            let mut replays = 0;
            while Instant::now() < until {
                if listener.accept().is_ok() {
                    replays += 1;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            replays
        });
        let (_dir, mut router, rid, cid, host) = configured(&url);
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            router.maintain().unwrap();
            let v = query(&mut router, &cid, None);
            assert!(v["error"].is_null(), "{v}");
            if v["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["stream"]["pending"] == true)
            {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        gate_tx.send(()).unwrap();
        loop {
            router.maintain().unwrap();
            if router
                .snapshot()
                .unwrap()
                .get("agent_runs", &rid)
                .unwrap()
                .data["state"]
                == "failed"
            {
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.join().unwrap(), 0);
        assert_eq!(host.effects.load(Ordering::SeqCst), 0);
        assert!(
            router
                .snapshot()
                .unwrap()
                .records("tool_invocations")
                .is_empty()
        );
        assert!(router.context_messages(&rid).unwrap().is_empty());
        let result = query(&mut router, &cid, None);
        assert!(
            result["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["stream"]["pending"] == false && r["is_error"] == true)
        );
    }
    fn fixture_progress(router: &mut Router, run: &str, chunks: usize) -> (String, String) {
        let snapshot = router.snapshot().unwrap();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let turn = new_id("turn");
        let cancel = Cancellation::default();
        let sink = StreamSink {
            sender,
            cancel: cancel.clone(),
            budget: router.stream_state.budget.clone(),
        };
        router.stream_state.runs.insert(
            run.into(),
            Pending {
                workspace: snapshot.workspace_id.clone(),
                epoch: router.engine_epoch.clone(),
                task: None,
                turn: turn.clone(),
                receiver,
                cancel,
                rows: vec![],
                total: 0,
            },
        );
        for _ in 0..chunks {
            sink.emit(StreamDelta::Text("observed-only ".repeat(1000)))
                .unwrap();
        }
        router.process_streams().unwrap();
        let pending = &router.stream_state.runs[run];
        (pending.rows.last().unwrap().1.clone(), turn)
    }
    struct BlockBlobs {
        path: PathBuf,
        backup: PathBuf,
    }
    impl BlockBlobs {
        fn new(dir: &Path) -> Self {
            let path = dir.join("store/objects/blobs");
            let backup = dir.join("blob-directory-backup");
            std::fs::rename(&path, &backup).unwrap();
            std::fs::write(&path, b"fixture blocks blob publication").unwrap();
            Self { path, backup }
        }
    }
    impl Drop for BlockBlobs {
        fn drop(&mut self) {
            std::fs::remove_file(&self.path).unwrap();
            std::fs::rename(&self.backup, &self.path).unwrap();
        }
    }
    #[test]
    fn failed_assistant_blob_publish_preserves_locator_and_terminal_cursor() {
        let (dir, mut router, rid, context) = crate::regression::seeded();
        let cid = context.conversation_id.as_deref().unwrap();
        let (cursor, _) = fixture_progress(&mut router, &rid, 1);
        let blocked = BlockBlobs::new(dir.path());
        let message = artex_runtime::Message {
            role: "assistant".into(),
            content: vec![json!({"type":"text","text":"final ".repeat(13000)})],
        };
        let error = router
            .append_transcript(&rid, "result", "final", &message, None)
            .unwrap_err();
        assert_eq!(error.code, "IO_FAILURE");
        assert!(
            router.stream_state.runs.contains_key(&rid),
            "Precommit error must not discard the pending locator"
        );
        assert!(!router.store.as_ref().unwrap().read_only());
        assert!(router.context_messages(&rid).unwrap().is_empty());
        drop(blocked);
        router.finish_stream(&rid, false).unwrap();
        assert!(!router.stream_state.runs.contains_key(&rid));
        let response = query(&mut router, cid, Some(&cursor));
        assert!(response["error"].is_null(), "{response}");
        assert!(
            response["result"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["stream"]["pending"] == false && r["is_error"] == true)
        );
        assert!(router.context_messages(&rid).unwrap().is_empty());
    }
    #[test]
    fn failed_terminal_commit_retries_and_reopen_recovers_display_without_replay() {
        let (dir, mut router, rid, context) = crate::regression::seeded();
        let cid = context.conversation_id.as_deref().unwrap();
        let (cursor, _) = fixture_progress(&mut router, &rid, 8);
        let blocked = BlockBlobs::new(dir.path());
        assert_eq!(
            router.finish_stream(&rid, false).unwrap_err().code,
            "IO_FAILURE"
        );
        assert!(router.stream_state.runs.contains_key(&rid));
        assert!(
            router
                .snapshot()
                .unwrap()
                .get("conversation_activities", &cursor)
                .is_some()
        );
        drop(blocked);
        // Drop the process-local locator to model restart after a failed transition.
        router.stream_state = Default::default();
        router.store = None;
        router.store = Some(Store::open(&dir.path().join("store")).unwrap());
        router.recover_streams().unwrap();
        let seq = router.snapshot().unwrap().commit_seq;
        router.recover_streams().unwrap();
        assert_eq!(
            router.snapshot().unwrap().commit_seq,
            seq,
            "Recovery must be idempotent"
        );
        let response = query(&mut router, cid, Some(&cursor));
        assert!(response["error"].is_null(), "{response}");
        let terminal = response["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["stream"]["pending"] == false)
            .unwrap();
        assert_eq!(terminal["is_error"], true);
        let row = router.snapshot().unwrap();
        let terminal = row
            .get("conversation_activities", terminal["seq"].as_str().unwrap())
            .unwrap();
        let diagnostic: Value =
            serde_json::from_str(&router.body_text(&terminal.data["detail"]).unwrap()).unwrap();
        assert_eq!(
            diagnostic["text"].as_str().unwrap(),
            "observed-only ".repeat(1000 * 8)
        );
        assert!(diagnostic["message"].is_null());
        assert!(router.context_messages(&rid).unwrap().is_empty());
        assert!(row.records("tool_invocations").is_empty());
    }
    #[test]
    fn disabled_agent_terminalizes_confirmed_fragments_without_poisoning_writer() {
        let (_dir, mut router, rid, context) = crate::regression::seeded();
        let s = router.snapshot().unwrap();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let cancel = Cancellation::default();
        let turn = new_id("turn");
        let sink = StreamSink {
            sender,
            cancel: cancel.clone(),
            budget: router.stream_state.budget.clone(),
        };
        router.stream_state.runs.insert(
            rid.clone(),
            Pending {
                workspace: s.workspace_id.clone(),
                epoch: router.engine_epoch.clone(),
                task: None,
                turn,
                receiver,
                cancel,
                rows: vec![],
                total: 0,
            },
        );
        sink.emit(StreamDelta::Text("confirmed before revocation".into()))
            .unwrap();
        router.process_streams().unwrap();
        sink.emit(StreamDelta::Text("queued after revocation".into()))
            .unwrap();
        let s = router.snapshot().unwrap();
        let aid = context.actor.agent_id.as_deref().unwrap();
        let mut agent = record(&s, "agents", aid).unwrap().data.clone();
        agent["enabled"] = json!(false);
        let mut plan = Planner::new(&s, "test.disable", &json!({}));
        plan.put("agents", aid, agent);
        router.internal_commit(plan.done(json!({}))).unwrap();
        router.runtime_tick().unwrap();
        router.runtime_tick().unwrap();
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .get("agent_runs", &rid)
                .unwrap()
                .data["state"],
            "cancelled"
        );
        assert!(!router.stream_state.runs.contains_key(&rid));
        let response = query(
            &mut router,
            context.conversation_id.as_deref().unwrap(),
            None,
        );
        assert!(response["error"].is_null(), "{response}");
        assert!(response["result"]["items"].as_array().unwrap().iter().any(
            |r| r["stream"]["pending"] == false
                && r["is_error"] == true
                && r["summary"] == "confirmed before revocation"
        ));
        assert!(router.context_messages(&rid).unwrap().is_empty());
    }
    #[test]
    fn bounded_channel_cancels_and_stale_or_frozen_context_cannot_append() {
        let (_dir, mut router, rid, _) = crate::regression::seeded();
        let s = router.snapshot().unwrap();
        let (sender, receiver) = mpsc::sync_channel(QUEUE);
        let cancel = Cancellation::default();
        let budget = Arc::new(AtomicUsize::new(0));
        let sink = StreamSink {
            sender,
            cancel: cancel.clone(),
            budget: budget.clone(),
        };
        for _ in 0..QUEUE {
            sink.emit(StreamDelta::Text("x".into())).unwrap();
        }
        let worker = std::thread::spawn(move || sink.emit(StreamDelta::Text("blocked".into())));
        std::thread::sleep(Duration::from_millis(10));
        cancel.cancel();
        assert_eq!(worker.join().unwrap().unwrap_err().code, "CANCELLED");
        assert_eq!(budget.load(Ordering::Acquire), QUEUE * (1 + 256));
        let mut pending = Pending {
            workspace: s.workspace_id.clone(),
            epoch: new_id("epoch"),
            task: None,
            turn: new_id("turn"),
            receiver,
            cancel: Cancellation::default(),
            rows: vec![],
            total: 0,
        };
        let before = s.commit_seq;
        router.drain_stream(&rid, &mut pending).unwrap();
        assert_eq!(router.snapshot().unwrap().commit_seq, before);
        assert!(pending.cancel.cancelled());
        pending.epoch = router.engine_epoch.clone();
        pending.task = Some("frozen-task".into());
        router.deleting_tasks.insert("frozen-task".into());
        router.stream_state.runs.insert(rid.clone(), pending);
        router.process_streams().unwrap();
        assert_eq!(router.snapshot().unwrap().commit_seq, before);
        router.stream_state = Default::default();
        assert_eq!(
            budget.load(Ordering::Acquire),
            0,
            "Dropping a stale queue must release its full byte reservation"
        );
    }
}
