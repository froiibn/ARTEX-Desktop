//! Actual MCP stdio, Streamable HTTP and legacy SSE with bounded framing and
//! cancellation. A cancelled request is never silently retried.
use crate::{
    ExecutionRequest, HostRuntime,
    execution::{ExternalLease, ProcessGroup},
};
use artex_core::{Context, Error, Result};
use futures_util::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::{BufRead, Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
// Keep the existing MCP response-wait budget without assigning that deadline
// to the lifetime of a retained legacy SSE response body.
const RESPONSE_WAIT: Duration = Duration::from_secs(30);
async fn response_deadline<F: std::future::Future>(future: F) -> Result<F::Output> {
    tokio::time::timeout(RESPONSE_WAIT, future)
        .await
        .map_err(|_| Error::new("TIMEOUT", "MCP response deadline exceeded"))
}
#[derive(Clone, PartialEq, Eq, Hash)]
struct SessionKey {
    epoch: String,
    owner: String,
    server: String,
}
struct Slot {
    fingerprint: String,
    retired: AtomicBool,
    client: Mutex<Option<Client>>,
}
/// A run owns its MCP connections, as in the original ToolAugment cleanup.
/// Calls on one connection serialize; unrelated runs/servers never share state.
#[derive(Default)]
pub(crate) struct SessionPool {
    sessions: Mutex<HashMap<SessionKey, Arc<Slot>>>,
}
impl SessionPool {
    pub(crate) fn stop(&self, run: Option<&str>, epoch: &str) {
        let retired = {
            let mut sessions = self.sessions.lock().unwrap();
            let keys: Vec<_> = sessions
                .keys()
                .filter(|key| key.epoch == epoch && run.is_none_or(|run| key.owner == run))
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| sessions.remove(&key))
                .collect::<Vec<_>>()
        };
        for slot in retired {
            slot.retired.store(true, Ordering::SeqCst);
            // The caller marks run cancellation first, so in-flight calls leave
            // their owned I/O before teardown acquires this connection lock.
            slot.client.lock().unwrap().take();
        }
    }
    fn slot(&self, key: SessionKey, fingerprint: String) -> Arc<Slot> {
        let (slot, retired) = {
            let mut sessions = self.sessions.lock().unwrap();
            if let Some(slot) = sessions
                .get(&key)
                .filter(|slot| slot.fingerprint == fingerprint)
            {
                return slot.clone();
            }
            let slot = Arc::new(Slot {
                fingerprint,
                retired: AtomicBool::new(false),
                client: Mutex::new(None),
            });
            let previous = sessions.insert(key, slot.clone());
            if let Some(previous) = &previous {
                previous.retired.store(true, Ordering::SeqCst);
            }
            (slot, previous)
        };
        // A changed approved definition retires its old connection, not just
        // its cache key. Queued calls on that retired connection cannot run.
        if let Some(retired) = retired {
            retired.client.lock().unwrap().take();
        }
        slot
    }
}
enum Transport {
    Stdio(Process),
    Http(Http),
    Sse(Http, reqwest::Url, Events),
}
struct Client {
    transport: Transport,
    runtime: tokio::runtime::Runtime,
    next_id: u64,
}
impl Client {
    fn connect(host: &HostRuntime, input: &Value, context: &Context) -> Result<Self> {
        let server = &input["server"];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::io)?;
        let init = json!({"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"ARTEX Desktop","version":"0.1.0"}}});
        let ready = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let transport = match server["transport"].as_str() {
            Some("stdio") => {
                let cap = input["directory_capability"].as_str().ok_or_else(|| {
                    Error::new("PERMISSION_DENIED", "MCP process capability required")
                })?;
                let process =
                    Process::spawn(server, &host.capabilities.root(cap)?, host, context, input)?;
                process.call(init, host, context)?;
                process.notify(ready)?;
                Transport::Stdio(process)
            }
            Some("http") => {
                let mut http = Http::new(server, input)?;
                runtime.block_on(async {
                    http.post(&init, host, context).await?;
                    http.post(&ready, host, context).await
                })?;
                Transport::Http(http)
            }
            Some("sse") => {
                let http = Http::new(server, input)?;
                let (endpoint, mut events) =
                    runtime.block_on(http.legacy_connect(host, context))?;
                runtime.block_on(async {
                    http.legacy_post(&endpoint, &mut events, &init, host, context)
                        .await?;
                    http.legacy_post(&endpoint, &mut events, &ready, host, context)
                        .await
                })?;
                Transport::Sse(http, endpoint, events)
            }
            _ => return Err(Error::new("INVALID_PARAMS", "Unknown MCP transport")),
        };
        Ok(Self {
            transport,
            runtime,
            next_id: 0,
        })
    }
    fn call(
        &mut self,
        host: &HostRuntime,
        context: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "MCP request id exhausted"))?;
        let frame = json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params});
        match &mut self.transport {
            Transport::Stdio(process) => process.call(frame, host, context),
            Transport::Http(http) => self.runtime.block_on(http.post(&frame, host, context)),
            Transport::Sse(http, endpoint, events) => self
                .runtime
                .block_on(http.legacy_post(endpoint, events, &frame, host, context)),
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // Closing the retained SSE body cancels its stream. Streamable HTTP's
        // DELETE is best effort, bounded by the native process cleanup budget;
        // a response here is never used as external execution cleanup proof.
        if let Transport::Http(http) = &self.transport {
            if http.session.is_some() {
                let _ = self.runtime.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(2),
                        http.request(reqwest::Method::DELETE, &http.url).send(),
                    )
                    .await
                });
            }
        }
    }
}
struct Process {
    child: Child,
    group: ProcessGroup,
    writes: mpsc::SyncSender<Value>,
    reads: mpsc::Receiver<Value>,
    lease: ExternalLease,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Process {
    fn spawn(
        server: &Value,
        cwd: &std::path::Path,
        host: &HostRuntime,
        context: &Context,
        input: &Value,
    ) -> Result<Self> {
        let run = context
            .actor
            .run_id
            .clone()
            .or_else(|| input["command_id"].as_str().map(|id| format!("ui_{id}")))
            .ok_or_else(|| {
                Error::new(
                    "PERMISSION_DENIED",
                    "MCP requires an owned run or UI command",
                )
            })?;
        let command = server["command"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new("INVALID_PARAMS", "MCP command required"))?;
        let mut command = Command::new(command);
        if let Some(args) = server["args"].as_array() {
            for arg in args {
                command.arg(arg.as_str().ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "MCP argument must be a string")
                })?);
            }
        }
        if let Some(env) = server["env"].as_object() {
            for (key, value) in env {
                command.env(
                    key,
                    value.as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Resolved MCP environment required")
                    })?,
                );
            }
        }
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000 | 0x00000004);
        }
        // Reserve ownership before spawning: capacity/ownership failures must
        // not leave an unregistered native process behind.
        let lease = host.executions.register_external(
            ExecutionRequest {
                execution_id: artex_core::new_id("exec"),
                run_id: run,
                engine_epoch: context.actor.engine_epoch.clone(),
                executable: server["command"].as_str().unwrap_or("").into(),
                args: vec![],
                cwd: cwd.into(),
                timeout_ms: 30000,
            },
            0,
            "mcp",
            2,
        )?;
        if host.context_cancelled(context) || lease.stopping() {
            lease.abort_unstarted();
            return Err(Error::new(
                "CANCELLED",
                "MCP run ended before process launch",
            ));
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                lease.abort_unstarted();
                return Err(Error::io(error));
            }
        };
        lease.started(child.id());
        let group = match ProcessGroup::attach(&mut child) {
            Ok(group) => group,
            Err(mut error) => {
                let _ = child.kill();
                let deadline = Instant::now() + Duration::from_secs(2);
                let mut exited = false;
                let mut code = None;
                while Instant::now() < deadline {
                    if let Ok(Some(status)) = child.try_wait() {
                        exited = true;
                        code = status.code();
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                lease.io_counter().store(2, Ordering::SeqCst);
                // Failed containment cannot establish descendant cleanup even
                // when the leader was reaped. Keep that receipt observable.
                lease.finish(exited, code, false);
                error.recovery_required = true;
                return Err(error);
            }
        };
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::sync_channel::<Value>(16);
        let (out_tx, out_rx) = mpsc::sync_channel(16);
        let done = lease.io_counter();
        let writer = thread::spawn(move || {
            while let Ok(value) = rx.recv() {
                let mut bytes = match serde_json::to_vec(&value) {
                    Ok(v) => v,
                    Err(_) => break,
                };
                if bytes.len() > 1_048_576 {
                    break;
                }
                bytes.push(b'\n');
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
            done.fetch_add(1, Ordering::SeqCst);
        });
        let done = lease.io_counter();
        let reader = thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            loop {
                let mut bytes = vec![];
                let n = reader
                    .by_ref()
                    .take(1_048_577)
                    .read_until(b'\n', &mut bytes);
                if !matches!(n,Ok(n)if n>0&&n<=1_048_576) {
                    break;
                }
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    if out_tx.try_send(value).is_err() {
                        break;
                    }
                } else {
                    break;
                }
            }
            done.fetch_add(1, Ordering::SeqCst);
        });
        Ok(Self {
            child,
            group,
            writes: tx,
            reads: out_rx,
            lease,
            workers: vec![writer, reader],
        })
    }
    fn call(&self, value: Value, host: &HostRuntime, context: &Context) -> Result<Value> {
        if host.context_cancelled(context) || self.lease.stopping() || self.lease.force_requested()
        {
            return Err(Error::new("CANCELLED", "MCP run ended"));
        }
        let id = value["id"].clone();
        self.writes
            .try_send(value)
            .map_err(|_| Error::new("BUSY", "MCP writer unavailable"))?;
        let start = Instant::now();
        loop {
            if host.context_cancelled(context)
                || self.lease.stopping()
                || self.lease.force_requested()
            {
                return Err(Error::new("CANCELLED", "MCP run ended"));
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err(Error::new("TIMEOUT", "MCP response deadline exceeded"));
            }
            match self.reads.recv_timeout(Duration::from_millis(50)) {
                Ok(frame) if frame["id"] == id => return response(frame),
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(Error::new("EXTERNAL_FAILURE", "MCP process disconnected")),
            }
        }
    }
    fn notify(&self, value: Value) -> Result<()> {
        self.writes
            .try_send(value)
            .map_err(|_| Error::new("BUSY", "MCP writer unavailable"))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.group.request_stop();
        // Disconnect writes before waiting for its worker; never join a pipe
        // still held open by a detached child beyond the cleanup budget.
        let (empty, _) = mpsc::sync_channel(1);
        let previous = std::mem::replace(&mut self.writes, empty);
        drop(previous);
        let deadline = Instant::now() + Duration::from_secs(2);
        let graceful_until = Instant::now() + Duration::from_millis(500);
        let mut forced = false;
        let mut exited = false;
        let mut code = None;
        while Instant::now() < deadline {
            if !forced && (self.lease.force_requested() || Instant::now() >= graceful_until) {
                self.group.kill(&mut self.child);
                forced = true;
            }
            if !exited {
                if let Ok(Some(status)) = self.child.try_wait() {
                    self.group.mark_leader_reaped();
                    exited = true;
                    code = status.code();
                }
            }
            if exited && self.group.known_empty() && self.workers.iter().all(|w| w.is_finished()) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        for worker in self.workers.drain(..) {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
        self.lease.finish(exited, code, self.group.known_empty());
    }
}
fn response(frame: Value) -> Result<Value> {
    if let Some(error) = frame.get("error") {
        if frame.get("result").is_some()
            || error["code"].as_i64().is_none()
            || !error["message"].is_string()
        {
            return Err(Error::new(
                "EXTERNAL_FAILURE",
                "Malformed MCP error response",
            ));
        }
        let mut result = Error::new("EXTERNAL_FAILURE", "MCP server returned an error");
        // This marker is attached only after matching the request ID and fully
        // decoding an RPC error. Such an error does not poison the transport.
        result.details = json!({"rpc_code":error["code"],"mcp_response_received":true});
        return Err(result);
    }
    frame
        .get("result")
        .cloned()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "MCP result missing"))
}
fn external(e: reqwest::Error) -> Error {
    Error::new(
        if e.is_timeout() {
            "TIMEOUT"
        } else {
            "EXTERNAL_FAILURE"
        },
        "MCP network operation failed",
    )
}
async fn cancellable<F: std::future::Future>(
    host: &HostRuntime,
    context: &Context,
    future: F,
) -> Result<F::Output> {
    tokio::pin!(future);
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {value=&mut future=>{if host.context_cancelled(context){return Err(Error::new("CANCELLED","MCP run ended"));}return Ok(value)},_ =interval.tick()=>{if host.context_cancelled(context){return Err(Error::new("CANCELLED","MCP run ended"));}}}
    }
}
struct Events {
    stream: BoxStream<'static, Result<Vec<u8>>>,
    buffer: Vec<u8>,
    total: usize,
}
impl Events {
    fn new(response: reqwest::Response) -> Self {
        Self {
            stream: response
                .bytes_stream()
                .map(|r| r.map(|b| b.to_vec()).map_err(external))
                .boxed(),
            buffer: vec![],
            total: 0,
        }
    }
    async fn next(&mut self, host: &HostRuntime, context: &Context) -> Result<(String, String)> {
        loop {
            if let Some(end) = self
                .buffer
                .windows(2)
                .position(|b| b == b"\n\n")
                .map(|i| (i, 2))
                .or_else(|| {
                    self.buffer
                        .windows(4)
                        .position(|b| b == b"\r\n\r\n")
                        .map(|i| (i, 4))
                })
            {
                let raw = self.buffer.drain(..end.0 + end.1).collect::<Vec<_>>();
                let text = String::from_utf8(raw)
                    .map_err(|_| Error::new("EXTERNAL_FAILURE", "MCP SSE is not UTF-8"))?;
                let event = text
                    .lines()
                    .find_map(|l| l.strip_prefix("event:"))
                    .unwrap_or("message")
                    .trim()
                    .to_string();
                let data = text
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
                    .collect::<Vec<_>>()
                    .join("\n");
                if data.is_empty() {
                    continue;
                }
                return Ok((event, data));
            }
            let chunk = cancellable(host, context, self.stream.next())
                .await?
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "MCP event stream ended"))??;
            self.total += chunk.len();
            self.buffer.extend(chunk);
            if self.total > 8 * 1024 * 1024 || self.buffer.len() > 1_048_576 {
                return Err(Error::new("SIZE_LIMIT", "MCP event stream budget exceeded"));
            }
        }
    }
    async fn matching(
        &mut self,
        host: &HostRuntime,
        context: &Context,
        id: &Value,
    ) -> Result<Value> {
        // Retained legacy SSE streams span many tool calls. The stream budget
        // bounds one response wait, not the lifetime of an otherwise healthy
        // run session; the independent frame/buffer limit remains in next().
        self.total = 0;
        response_deadline(async {
            loop {
                let (event, data) = self.next(host, context).await?;
                if event != "message" {
                    continue;
                }
                let frame: Value = serde_json::from_str(&data)?;
                if frame["id"] == *id {
                    return response(frame);
                }
            }
        })
        .await?
    }
}
struct Http {
    client: reqwest::Client,
    url: reqwest::Url,
    headers: BTreeMap<String, String>,
    session: Option<String>,
}
impl Http {
    fn new(server: &Value, input: &Value) -> Result<Self> {
        let url = reqwest::Url::parse(
            server["url"]
                .as_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "MCP URL required"))?,
        )
        .map_err(|_| Error::new("INVALID_PARAMS", "Invalid MCP URL"))?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "MCP URL must be HTTP(S) without credentials",
            ));
        }
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(server["insecure"].as_bool().unwrap_or(false));
        if let Some(proxy) = input["network_config"]["proxy"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            builder = builder.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid MCP proxy"))?,
            );
        }
        let client = builder.build().map_err(external)?;
        let mut headers = BTreeMap::new();
        if let Some(env) = server["env"].as_object() {
            if env.len() > 256 {
                return Err(Error::new("SIZE_LIMIT", "MCP header count exceeded"));
            }
            for (key, value) in env {
                headers.insert(
                    key.clone(),
                    value
                        .as_str()
                        .ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "Resolved MCP headers required")
                        })?
                        .to_string(),
                );
            }
        }
        Ok(Self {
            client,
            url,
            headers,
            session: None,
        })
    }
    fn request(&self, method: reqwest::Method, url: &reqwest::Url) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .request(method, url.clone())
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2024-11-05");
        for (key, value) in &self.headers {
            r = r.header(key, value);
        }
        if let Some(session) = &self.session {
            r = r.header("Mcp-Session-Id", session);
        }
        r
    }
    async fn post(
        &mut self,
        value: &Value,
        host: &HostRuntime,
        context: &Context,
    ) -> Result<Value> {
        let response = cancellable(
            host,
            context,
            self.request(reqwest::Method::POST, &self.url)
                .timeout(RESPONSE_WAIT)
                .json(value)
                .send(),
        )
        .await?
        .map_err(external)?;
        if !response.status().is_success() {
            return Err(Error::new("EXTERNAL_FAILURE", "MCP HTTP status failed"));
        }
        if let Some(session) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|v| v.to_str().ok())
        {
            if session.len() > 1024 {
                return Err(Error::new("SIZE_LIMIT", "MCP session id exceeded"));
            }
            self.session = Some(session.into());
        }
        if value.get("id").is_none() {
            return Ok(Value::Null);
        }
        if response
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| s.contains("text/event-stream"))
        {
            return Events::new(response)
                .matching(host, context, &value["id"])
                .await;
        }
        if response.content_length().is_some_and(|n| n > 1_048_576) {
            return Err(Error::new("SIZE_LIMIT", "MCP response exceeded"));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = vec![];
        while let Some(chunk) = cancellable(host, context, stream.next()).await? {
            bytes.extend(chunk.map_err(external)?);
            if bytes.len() > 1_048_576 {
                return Err(Error::new("SIZE_LIMIT", "MCP response exceeded"));
            }
        }
        let frame: Value = serde_json::from_slice(&bytes)?;
        if frame["id"] != value["id"] {
            return Err(Error::new("EXTERNAL_FAILURE", "MCP response id mismatch"));
        }
        crate::mcp::response(frame)
    }
    async fn legacy_connect(
        &self,
        host: &HostRuntime,
        context: &Context,
    ) -> Result<(reqwest::Url, Events)> {
        // The GET stream belongs to the run and may remain idle between calls.
        // Bound handshake/endpoint discovery instead of its whole body lifetime.
        response_deadline(async {
            let response = cancellable(
                host,
                context,
                self.request(reqwest::Method::GET, &self.url).send(),
            )
            .await?
            .map_err(external)?;
            if !response.status().is_success() {
                return Err(Error::new("EXTERNAL_FAILURE", "MCP SSE connection failed"));
            }
            let mut events = Events::new(response);
            loop {
                let (event, data) = events.next(host, context).await?;
                if event == "endpoint" {
                    let endpoint = self
                        .url
                        .join(&data)
                        .map_err(|_| Error::new("EXTERNAL_FAILURE", "Invalid MCP endpoint"))?;
                    if endpoint.origin() != self.url.origin() {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "MCP SSE endpoint changed authority",
                        ));
                    }
                    return Ok((endpoint, events));
                }
            }
        })
        .await?
    }
    async fn legacy_post(
        &self,
        endpoint: &reqwest::Url,
        events: &mut Events,
        frame: &Value,
        host: &HostRuntime,
        context: &Context,
    ) -> Result<Value> {
        let response = cancellable(
            host,
            context,
            self.request(reqwest::Method::POST, endpoint)
                .timeout(RESPONSE_WAIT)
                .json(frame)
                .send(),
        )
        .await?
        .map_err(external)?;
        if !response.status().is_success() {
            return Err(Error::new("EXTERNAL_FAILURE", "MCP SSE POST failed"));
        }
        if frame.get("id").is_some() {
            events.matching(host, context, &frame["id"]).await
        } else {
            Ok(Value::Null)
        }
    }
}
pub fn refresh(host: &HostRuntime, input: &Value, context: &Context) -> Result<Value> {
    let tools = invoke(host, input, context, "tools/list", json!({}))?;
    let list = tools["tools"]
        .as_array()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "MCP tools list missing"))?;
    Ok(json!(list.iter().map(|tool|json!({"name":tool["name"],"description":tool["description"].as_str().unwrap_or(""),"input_schema":tool.get("inputSchema").cloned().unwrap_or_else(||json!({"type":"object"}))})).collect::<Vec<_>>()))
}
pub fn call(host: &HostRuntime, input: &Value, context: &Context) -> Result<Value> {
    invoke(
        host,
        input,
        context,
        "tools/call",
        json!({"name":input["tool_name"],"arguments":input["arguments"]}),
    )
}
fn invoke(
    host: &HostRuntime,
    input: &Value,
    context: &Context,
    method: &str,
    params: Value,
) -> Result<Value> {
    if host.context_cancelled(context) {
        return Err(Error::new("CANCELLED", "MCP epoch/run ended"));
    }
    let owner = context
        .actor
        .run_id
        .clone()
        .or_else(|| input["command_id"].as_str().map(|id| format!("ui_{id}")))
        .ok_or_else(|| {
            Error::new(
                "PERMISSION_DENIED",
                "MCP requires an owned run or UI command",
            )
        })?;
    // UI discovery/test commands have no agent run lifetime to retire a pool
    // entry. Their temporary connection closes when the command completes.
    if context.actor.kind != "agent" || context.actor.run_id.is_none() {
        let mut owned = context.clone();
        owned.actor.run_id = Some(owner);
        if host.context_cancelled(&owned) {
            return Err(Error::new("CANCELLED", "MCP UI command ended"));
        }
        return Client::connect(host, input, &owned)?.call(host, &owned, method, params);
    }
    let server = &input["server"];
    let root = input["directory_capability"]
        .as_str()
        .map(|cap| host.capabilities.root(cap))
        .transpose()?;
    let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(
        server,
        &input["network_config"],
        root,
    ))?));
    let identity = server["id"]
        .as_str()
        .or_else(|| server["name"].as_str())
        .unwrap_or(&fingerprint)
        .to_owned();
    let key = SessionKey {
        epoch: context.actor.engine_epoch.clone(),
        owner,
        server: identity,
    };
    let slot = host.mcp_sessions.slot(key, fingerprint);
    let mut client = slot.client.lock().unwrap();
    if slot.retired.load(Ordering::SeqCst) || host.context_cancelled(context) {
        return Err(Error::new("CANCELLED", "MCP connection was retired"));
    }
    if client.is_none() {
        *client = Some(Client::connect(host, input, context)?);
    }
    let result = client.as_mut().unwrap().call(host, context, method, params);
    // A matched RPC error is a completed response, as in norma Client.wrap;
    // preserve its healthy stateful session. Transport/protocol failures may
    // leave an uncertain request outstanding: discard and never retry here.
    if result
        .as_ref()
        .err()
        .is_some_and(|error| error.details["mcp_response_received"] != true)
    {
        client.take();
    }
    result
}
#[cfg(test)]
mod supervision_tests {
    use super::*;
    use artex_core::{Actor, new_id};
    #[test]
    fn retained_sse_budget_is_per_response_wait_not_session_lifetime() {
        fn events(ids: impl IntoIterator<Item = &'static str>) -> Events {
            let padding = "x".repeat(512 * 1024);
            let chunks = ids
                .into_iter()
                .map(|id| {
                    Ok(format!(
                        "event: message\ndata: {}\n\n",
                        json!({"id":id,"result":{"padding":padding}})
                    )
                    .into_bytes())
                })
                .collect::<Vec<Result<Vec<u8>>>>();
            Events {
                stream: futures_util::stream::iter(chunks).boxed(),
                buffer: vec![],
                total: 0,
            }
        }
        let host = HostRuntime::default();
        let context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        // Each wait is below 8 MiB; their combined bytes exceed that limit.
        let mut retained = events(
            std::iter::repeat_n("other", 8)
                .chain(["first"])
                .chain(std::iter::repeat_n("other", 8))
                .chain(["second"]),
        );
        runtime
            .block_on(retained.matching(&host, &context, &json!("first")))
            .unwrap();
        runtime
            .block_on(retained.matching(&host, &context, &json!("second")))
            .unwrap();
        // An individual wait still cannot consume an unbounded notification
        // stream while waiting for its own response.
        let mut oversized = events(std::iter::repeat_n("other", 17).chain(["target"]));
        assert_eq!(
            runtime
                .block_on(oversized.matching(&host, &context, &json!("target")))
                .unwrap_err()
                .code,
            "SIZE_LIMIT"
        );
    }
    #[test]
    fn stdio_mcp_is_owned_force_stoppable_and_monitor_redacts_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let host = HostRuntime::default();
        let context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let script =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_server.py");
        let server = json!({"command":if cfg!(windows){"python.exe"}else{"python3"},"args":[script],"env":{"SUPERVISION_TEST_SECRET":"NEVER-IN-MONITOR"}});
        let process =
            Process::spawn(&server, directory.path(), &host, &context, &json!({})).unwrap();
        process
            .call(
                json!({"jsonrpc":"2.0","id":"init","method":"initialize"}),
                &host,
                &context,
            )
            .unwrap();
        let run = context.actor.run_id.as_deref().unwrap();
        let epoch = &context.actor.engine_epoch;
        let initial = host.inspect_run(run, epoch);
        assert_eq!(initial["executions"][0]["kind"], "mcp");
        assert!(initial["executions"][0]["pid"].as_u64().unwrap() > 0);
        assert!(!initial.to_string().contains("NEVER-IN-MONITOR"));
        assert!(!initial.to_string().contains("SUPERVISION_TEST_SECRET"));
        let requested = host.force_stop_run(run, epoch, Duration::ZERO);
        assert_eq!(requested["accepted"], true);
        assert_eq!(requested["cleanup_confirmed"], false);
        assert_eq!(
            process
                .call(
                    json!({"jsonrpc":"2.0","id":"next","method":"tools/list"}),
                    &host,
                    &context
                )
                .unwrap_err()
                .code,
            "CANCELLED"
        );
        drop(process);
        let stopped = host.inspect_run(run, epoch);
        assert_eq!(stopped["executions"][0]["leader_exited"], true);
        assert_eq!(stopped["executions"][0]["output_drained"], true);
        assert_eq!(stopped["cleanup_confirmed"], cfg!(windows));
        assert!(
            host.inspect_run("unrelated", epoch)["executions"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn missing_owner_is_rejected_before_process_launch() {
        let host = HostRuntime::default();
        let dir = tempfile::tempdir().unwrap();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let error = Process::spawn(
            &json!({"command":"nonexistent-mcp-fixture"}),
            dir.path(),
            &host,
            &context,
            &json!({}),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "PERMISSION_DENIED");
        assert!(!host.epoch_active(&context.actor.engine_epoch));
    }
}
