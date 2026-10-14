//! Real ConPTY/Unix PTY sessions. Ownership is bound to the engine epoch/run.
use crate::CapabilityRegistry;
use artex_core::{Context, Error, Result, new_id};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    fs,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
const RING: usize = 256 * 1024;
struct State {
    bytes: Vec<u8>,
    total: u64,
    cursor: u64,
    parser: vt100::Parser,
    state: String,
    exit: Option<u32>,
    last: Instant,
    ended: Option<Instant>,
    error: Option<String>,
    leader_exited: bool,
    group_empty: bool,
}
struct Session {
    id: String,
    fingerprint: String,
    command: String,
    mode: String,
    run: String,
    epoch: String,
    rows: u16,
    cols: u16,
    prompt: Option<regex::Regex>,
    state: Mutex<State>,
    stop: AtomicBool,
    force: AtomicBool,
    drain_done: AtomicBool,
    writer_done: AtomicBool,
    writer_stop: AtomicBool,
    terminal_closed: AtomicBool,
    pid: u32,
    writes: mpsc::SyncSender<Vec<u8>>,
    output_path: String,
}
#[derive(Default)]
pub struct PtySupervisor {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    events: Arc<Mutex<VecDeque<Value>>>,
}
fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input[key]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", &format!("Missing {key}")))
}
fn integer(v: &Value, default: u64) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(default)
}
fn pattern(v: &Value) -> Result<Option<regex::Regex>> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .map(|s| {
            regex::RegexBuilder::new(s)
                .size_limit(1024 * 1024)
                .build()
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid terminal regex"))
        })
        .transpose()
}
impl PtySupervisor {
    pub fn inspect(&self, run: &str, epoch: &str) -> Vec<Value> {
        self.sessions.lock().unwrap().values().filter(|s|s.run==run&&s.epoch==epoch).map(|s|{
            let st=s.state.lock().unwrap();let drained=s.drain_done.load(Ordering::SeqCst)&&s.writer_done.load(Ordering::SeqCst)&&s.terminal_closed.load(Ordering::SeqCst);
            let known=st.leader_exited&&st.group_empty;let descendants=cfg!(windows)&&known;let confirmed=descendants&&drained;
            json!({"execution_id":s.id,"kind":"pty","executable":"terminal","pid":s.pid,"state":st.state,"exit_code":st.exit,"stop_requested":s.stop.load(Ordering::SeqCst),"force_requested":s.force.load(Ordering::SeqCst),"leader_exited":st.leader_exited,"output_drained":drained,"known_cleanup_confirmed":known,"descendant_cleanup_confirmed":descendants,"cleanup_confirmed":confirmed,"cleanup_state":if confirmed{"confirmed"}else if st.state=="running"{"running"}else{"unconfirmed"},"containment":if cfg!(windows){"windows_job"}else{"unix_process_group_unconfined"},"first_cursor":st.total.saturating_sub(st.bytes.len()as u64).to_string(),"last_cursor":st.total.to_string(),"output_path":s.output_path,"cleanup_error":st.error})
        }).collect()
    }
    pub fn force_stop_run(&self, run: &str, epoch: &str) {
        for s in self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.run == run && s.epoch == epoch)
        {
            s.stop.store(true, Ordering::SeqCst);
            s.force.store(true, Ordering::SeqCst);
        }
    }
    pub fn execute(
        &self,
        registry: &CapabilityRegistry,
        name: &str,
        input: &Value,
        context: &Context,
    ) -> Result<Value> {
        let run = context
            .actor
            .run_id
            .as_deref()
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "PTY requires current agent run"))?;
        let epoch = &context.actor.engine_epoch;
        if name == "shell_open" {
            return self.open(registry, input, run, epoch);
        }
        if name == "shell_list" {
            // Preserve cleanup receipts for the whole workspace epoch. Dropping
            // an unconfirmed receipt would make a later purge look confirmed.
            let sessions = self.sessions.lock().unwrap();
            return Ok(
                json!({"sessions":sessions.values().filter(|s|s.run==run&&s.epoch==*epoch).map(|s|{let st=s.state.lock().unwrap();json!({"session_id":s.id,"command":s.command,"state":st.state,"mode":s.mode,"idle_ms":st.last.elapsed().as_millis(),"output_path":s.output_path})}).collect::<Vec<_>>(),"notifications":self.drain_events(run,epoch)}),
            );
        }
        let id = text(input, "session_id")?;
        let s = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("NOT_FOUND", "PTY session not found"))?;
        if s.run != run || s.epoch != *epoch {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "PTY session ownership mismatch",
            ));
        }
        match name {
            "shell_close" => {
                s.stop.store(true, Ordering::SeqCst);
                let end = Instant::now() + Duration::from_secs(5);
                while s.state.lock().unwrap().state == "running" && Instant::now() < end {
                    thread::sleep(Duration::from_millis(10));
                }
                let st = s.state.lock().unwrap();
                let confirmed = cfg!(windows)
                    && st.leader_exited
                    && st.group_empty
                    && s.drain_done.load(Ordering::SeqCst)
                    && s.writer_done.load(Ordering::SeqCst)
                    && s.terminal_closed.load(Ordering::SeqCst);
                if st.state == "running" {
                    return Err(Error::new("TIMEOUT", "PTY cleanup deadline exceeded"));
                }
                Ok(
                    json!({"closed":s.id,"exit_code":st.exit,"cleanup_confirmed":confirmed,"accepted":true}),
                )
            }
            "shell_read" => {
                if input["view"] == "screen" {
                    let st = s.state.lock().unwrap();
                    let screen = st.parser.screen();
                    let (row, col) = screen.cursor_position();
                    return Ok(
                        json!({"view":"screen","screen":screen.contents().lines().collect::<Vec<_>>(),"rows":s.rows,"cols":s.cols,"cursor":{"row":row,"col":col},"running":st.state=="running"}),
                    );
                }
                let from = input
                    .get("since_cursor")
                    .map(|v| integer(v, 0))
                    .unwrap_or_else(|| s.state.lock().unwrap().cursor);
                let mut result = read(
                    &s,
                    from,
                    integer(&input["max_bytes"], 8192).min(65536) as usize,
                    integer(&input["max_lines"], 200).min(10000) as usize,
                    input["drain"].as_bool().unwrap_or(false),
                )?;
                result["notifications"] = json!(self.drain_events(run, epoch));
                Ok(result)
            }
            "shell_send" => {
                if s.state.lock().unwrap().state != "running" {
                    return Err(Error::new(
                        "VERSION_CONFLICT",
                        "PTY has ended; read remaining output",
                    ));
                }
                let before = s.state.lock().unwrap().total;
                let mut payload = Vec::new();
                if let Some(t) = input["text"].as_str() {
                    payload.extend_from_slice(t.as_bytes());
                }
                if input["submit"].as_bool().unwrap_or(false) {
                    payload.push(b'\r');
                }
                if let Some(keys) = input["keys"].as_array() {
                    for key in keys {
                        payload.extend_from_slice(key_bytes(
                            key.as_str().ok_or_else(|| {
                                Error::new("INVALID_PARAMS", "Named key required")
                            })?,
                        )?);
                    }
                }
                if let Some(bytes) = input["hex"].as_array() {
                    for byte in bytes {
                        let byte = byte
                            .as_str()
                            .ok_or_else(|| Error::new("INVALID_PARAMS", "Hex byte required"))?;
                        payload.push(
                            u8::from_str_radix(byte.trim_start_matches("0x"), 16).map_err(
                                |_| Error::new("INVALID_PARAMS", "Invalid terminal hex byte"),
                            )?,
                        );
                    }
                }
                if let Some(paste) = input["paste"].as_str() {
                    payload.extend_from_slice(b"\x1b[200~");
                    payload.extend_from_slice(paste.as_bytes());
                    payload.extend_from_slice(b"\x1b[201~");
                }
                if payload.len() > 65536 {
                    return Err(Error::new("SIZE_LIMIT", "PTY input exceeds 64 KiB"));
                }
                s.writes
                    .try_send(payload)
                    .map_err(|_| Error::new("BUSY", "PTY input queue unavailable"))?;
                let done = if input["wait"].as_bool().unwrap_or(true) {
                    wait(
                        &s,
                        Duration::from_millis(integer(&input["quiet_ms"], 8000).min(120000)),
                        Duration::ZERO,
                        Duration::from_millis(integer(&input["timeout_ms"], 120000).min(600000)),
                        pattern(&input["prompt_regex"])?
                            .as_ref()
                            .or(s.prompt.as_ref()),
                        before,
                    )
                } else {
                    "sent"
                };
                let mut result = read(&s, before, 32768, 0, false)?;
                result["done_by"] = json!(done);
                Ok(result)
            }
            _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown PTY tool")),
        }
    }
    fn open(
        &self,
        registry: &CapabilityRegistry,
        input: &Value,
        run: &str,
        epoch: &str,
    ) -> Result<Value> {
        let command = text(input, "command")?;
        let mode = input["mode"].as_str().unwrap_or("hands-free");
        if !["hands-free", "interactive", "dispatch", "monitor"].contains(&mode) {
            return Err(Error::new("INVALID_PARAMS", "Unknown terminal mode"));
        }
        let watches = input["watch"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        pattern(v)?
                            .ok_or_else(|| Error::new("INVALID_PARAMS", "Watch pattern required"))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?
            .unwrap_or_default();
        if mode == "monitor" && (watches.is_empty() || watches.len() > 64) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Monitor requires 1..64 patterns",
            ));
        }
        let rows = integer(&input["rows"], 24).clamp(2, 200) as u16;
        let cols = integer(&input["cols"], 80).clamp(10, 500) as u16;
        let prompt = pattern(&input["prompt_regex"])?;
        let mut semantic = input.clone();
        if let Some(map) = semantic.as_object_mut() {
            map.remove("invocation_id");
            map.remove("command_id");
        }
        use sha2::{Digest, Sha256};
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&semantic)?));
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() >= 1024 || sessions.values().filter(|s| session_active(s)).count() >= 64 {
            return Err(Error::new("BUSY", "PTY session capacity reached"));
        }
        let id = input["execution_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| new_id("sess"));
        if let Some(old) = sessions.get(&id) {
            if old.run != run
                || old.epoch != epoch
                || old.command != command
                || old.mode != mode
                || old.rows != rows
                || old.cols != cols
                || old.fingerprint != fingerprint
            {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "PTY execution identifier reused",
                ));
            }
            return Ok(
                json!({"session_id":id,"state":old.state.lock().unwrap().state,"output_path":old.output_path}),
            );
        }
        let cap = text(input, "directory_capability")?;
        let cwd = registry.root(cap)?;
        let dir = registry.resolve(cap, ".artex-host/pty", true)?;
        fs::create_dir_all(&dir).map_err(Error::io)?;
        let filename = format!(
            "{}.log",
            hex::encode(Sha256::digest(format!("{epoch}\0{id}").as_bytes()))
        );
        let output = registry.resolve(cap, &format!(".artex-host/pty/{filename}"), true)?;
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output)
            .map_err(Error::io)?;
        let (reader, writer, process) = backend::spawn(
            command,
            &cwd,
            rows,
            cols,
            &input["native_environment"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
        )?;
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(16);
        let s = Arc::new(Session {
            id: id.clone(),
            fingerprint,
            command: command.into(),
            mode: mode.into(),
            run: run.into(),
            epoch: epoch.into(),
            rows,
            cols,
            prompt,
            state: Mutex::new(State {
                bytes: vec![],
                total: 0,
                cursor: 0,
                parser: vt100::Parser::new(rows, cols, 1000),
                state: "running".into(),
                exit: None,
                last: Instant::now(),
                ended: None,
                error: None,
                leader_exited: false,
                group_empty: false,
            }),
            stop: AtomicBool::new(false),
            force: AtomicBool::new(false),
            drain_done: AtomicBool::new(false),
            writer_done: AtomicBool::new(false),
            writer_stop: AtomicBool::new(false),
            terminal_closed: AtomicBool::new(false),
            pid: process.pid(),
            writes: tx,
            output_path: output
                .strip_prefix(&cwd)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
        });
        sessions.insert(id.clone(), s.clone());
        drop(sessions);
        let ws = s.clone();
        thread::spawn(move || {
            let mut writer = writer;
            loop {
                if ws.writer_stop.load(Ordering::SeqCst)
                    || ws.force.load(Ordering::SeqCst)
                    || ws.state.lock().unwrap().state != "running"
                {
                    break;
                }
                let data = match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(data) => data,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                if writer
                    .write_all(&data)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    ws.stop.store(true, Ordering::SeqCst);
                    break;
                }
            }
            ws.writer_done.store(true, Ordering::SeqCst);
        });
        let rs = s.clone();
        let events = self.events.clone();
        let drain = thread::spawn(move || {
            let mut reader = reader;
            let mut file = file;
            let mut buf = [0u8; 8192];
            let mut hit = vec![false; watches.len()];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let mut st = rs.state.lock().unwrap();
                if st.total + n as u64 > 1024 * 1024 * 1024 || file.write_all(&buf[..n]).is_err() {
                    st.error = Some("PTY output capture failed or exceeds 1 GiB".into());
                    rs.stop.store(true, Ordering::SeqCst);
                    break;
                }
                st.total += n as u64;
                st.last = Instant::now();
                st.bytes.extend_from_slice(&buf[..n]);
                if st.bytes.len() > RING {
                    let remove = st.bytes.len() - RING;
                    st.bytes.drain(..remove);
                }
                st.parser.process(&buf[..n]);
                let text = String::from_utf8_lossy(&st.bytes);
                for (i, watch) in watches.iter().enumerate() {
                    if !hit[i] && watch.is_match(&text) {
                        hit[i] = true;
                        push_event(
                            &events,
                            json!({"kind":"monitor_match","run_id":rs.run,"engine_epoch":rs.epoch,"session_id":rs.id,"pattern":watch.as_str(),"output":text.chars().rev().take(2048).collect::<String>().chars().rev().collect::<String>()}),
                        );
                    }
                }
            }
            if file.sync_all().is_err() {
                rs.state.lock().unwrap().error = Some("PTY output flush failed".into());
            }
            rs.drain_done.store(true, Ordering::SeqCst);
        });
        let worker = s.clone();
        let events = self.events.clone();
        thread::spawn(move || {
            let mut process = process;
            let mut stopping = None;
            let mut force_at = None;
            let mut leader_exited = false;
            let exit = loop {
                if worker.stop.load(Ordering::SeqCst)
                    || worker.state.lock().unwrap().last.elapsed() > Duration::from_secs(1800)
                {
                    if stopping.is_none() {
                        process.request_stop();
                        let _ = worker.writes.try_send(vec![3]);
                        stopping = Some(Instant::now());
                    }
                }
                if worker.force.load(Ordering::SeqCst)
                    || stopping
                        .is_some_and(|at: Instant| at.elapsed() >= Duration::from_millis(500))
                {
                    if force_at.is_none() {
                        process.kill();
                        worker.force.store(true, Ordering::SeqCst);
                        force_at = Some(Instant::now());
                    }
                }
                if force_at.is_some_and(|at: Instant| at.elapsed() >= Duration::from_secs(2)) {
                    break None;
                }
                match process.try_wait() {
                    Ok(Some(code)) => {
                        process.kill_group();
                        leader_exited = true;
                        break Some(code);
                    }
                    Ok(None) => {}
                    Err(_) => {
                        process.kill();
                        break None;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            };
            worker.writer_stop.store(true, Ordering::SeqCst);
            process.kill_group();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !process.known_empty() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            let group_empty = process.known_empty();
            let closed = worker.clone();
            let closer = thread::spawn(move || {
                process.close_terminal();
                closed.terminal_closed.store(true, Ordering::SeqCst);
            });
            while Instant::now() < deadline
                && (!drain.is_finished()
                    || !worker.writer_done.load(Ordering::SeqCst)
                    || !closer.is_finished())
            {
                thread::sleep(Duration::from_millis(10));
            }
            if drain.is_finished() {
                let _ = drain.join();
            }
            if closer.is_finished() {
                let _ = closer.join();
            }
            let mut st = worker.state.lock().unwrap();
            st.exit = exit;
            st.leader_exited = leader_exited;
            st.group_empty = group_empty;
            st.state = if st.error.is_some() {
                "failed"
            } else if worker.stop.load(Ordering::SeqCst) {
                "closed"
            } else {
                "exited"
            }
            .into();
            st.ended = Some(Instant::now());
            push_event(
                &events,
                json!({"kind":"session_finished","run_id":worker.run,"engine_epoch":worker.epoch,"session_id":worker.id,"exit_code":exit}),
            );
        });
        if mode == "interactive" {
            let by = wait(
                &s,
                Duration::from_millis(integer(&input["quiet_ms"], 8000).min(120000)),
                Duration::from_millis(integer(&input["startup_grace_ms"], 15000).min(120000)),
                Duration::from_secs(120),
                s.prompt.as_ref(),
                0,
            );
            let mut result = read(&s, 0, 32768, 0, false)?;
            result["session_id"] = json!(id);
            result["done_by"] = json!(by);
            return Ok(result);
        }
        if mode == "dispatch" {
            let dispatch = s.clone();
            let events = self.events.clone();
            let quiet = Duration::from_millis(integer(&input["quiet_ms"], 8000).min(120000));
            let grace =
                Duration::from_millis(integer(&input["startup_grace_ms"], 15000).min(120000));
            thread::spawn(move || {
                let by = wait(
                    &dispatch,
                    quiet,
                    grace,
                    Duration::from_secs(120),
                    dispatch.prompt.as_ref(),
                    0,
                );
                push_event(
                    &events,
                    json!({"kind":"dispatch_complete","run_id":dispatch.run,"engine_epoch":dispatch.epoch,"session_id":dispatch.id,"done_by":by}),
                );
            });
        }
        Ok(
            json!({"session_id":id,"state":"running","dispatched":mode=="dispatch","monitoring":if mode=="monitor"{input["watch"].as_array().unwrap().len()}else{0},"output_path":s.output_path}),
        )
    }
    pub fn stop_run(&self, run: &str, epoch: &str) {
        for s in self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.run == run && s.epoch == epoch)
        {
            s.stop.store(true, Ordering::SeqCst);
        }
    }
    pub fn stop_epoch(&self, epoch: &str) {
        for s in self
            .sessions
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.epoch == epoch)
        {
            s.stop.store(true, Ordering::SeqCst);
            s.force.store(true, Ordering::SeqCst);
        }
    }
    pub fn epoch_active(&self, epoch: &str) -> bool {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .any(|session| session.epoch == epoch && session_active(session))
    }
    pub fn drain_events(&self, run: &str, epoch: &str) -> Vec<Value> {
        let mut queue = self.events.lock().unwrap();
        let mut out = vec![];
        let mut keep = VecDeque::new();
        while let Some(event) = queue.pop_front() {
            if event["run_id"] == run && event["engine_epoch"] == epoch {
                out.push(event)
            } else {
                keep.push_back(event)
            }
        }
        *queue = keep;
        out
    }
}
impl Drop for PtySupervisor {
    fn drop(&mut self) {
        for s in self.sessions.lock().unwrap().values() {
            s.stop.store(true, Ordering::SeqCst);
            s.force.store(true, Ordering::SeqCst);
        }
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end
            && self
                .sessions
                .lock()
                .unwrap()
                .values()
                .any(|s| session_active(s))
        {
            thread::sleep(Duration::from_millis(20));
        }
    }
}
fn session_active(session: &Session) -> bool {
    let state = session.state.lock().unwrap();
    state.state == "running"
        || !session.drain_done.load(Ordering::SeqCst)
        || !session.writer_done.load(Ordering::SeqCst)
        || !session.terminal_closed.load(Ordering::SeqCst)
        || (cfg!(windows) && !(state.leader_exited && state.group_empty))
}
fn push_event(events: &Mutex<VecDeque<Value>>, event: Value) {
    let mut events = events.lock().unwrap();
    if events.len() >= 128 {
        events.pop_front();
    }
    events.push_back(event);
}
fn wait(
    s: &Session,
    quiet: Duration,
    grace: Duration,
    timeout: Duration,
    prompt: Option<&regex::Regex>,
    from: u64,
) -> &'static str {
    let start = Instant::now();
    loop {
        let st = s.state.lock().unwrap();
        if st.state != "running" {
            return "exit";
        }
        if s.stop.load(Ordering::SeqCst) {
            return "cancelled";
        }
        if prompt.is_some_and(|p| {
            p.is_match(&String::from_utf8_lossy(
                &st.bytes[(from.saturating_sub(st.total - st.bytes.len() as u64) as usize)
                    .min(st.bytes.len())..],
            ))
        }) {
            return "prompt";
        }
        if start.elapsed() >= grace && st.last.elapsed() >= quiet {
            return "quiet";
        }
        if start.elapsed() >= timeout {
            return "timeout";
        }
        drop(st);
        thread::sleep(Duration::from_millis(20));
    }
}
fn read(s: &Session, from: u64, max: usize, lines: usize, drain: bool) -> Result<Value> {
    let mut st = s.state.lock().unwrap();
    let first = st.total - st.bytes.len() as u64;
    if from < first {
        return Err(Error::new(
            "CURSOR_EXPIRED",
            "PTY cursor expired; full output remains on disk",
        ));
    }
    if from > st.total {
        return Err(Error::new("INVALID_PARAMS", "PTY cursor is beyond output"));
    }
    let offset = (from - first) as usize;
    let available = &st.bytes[offset..];
    let n = available.len().min(max);
    let n = if lines > 0 {
        available[..n]
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .nth(lines - 1)
            .map(|(i, _)| i + 1)
            .unwrap_or(n)
    } else {
        n
    };
    let output = String::from_utf8_lossy(&available[..n]).into_owned();
    let cursor = if drain { st.total } else { from + n as u64 };
    let omitted = st.total - cursor;
    st.cursor = cursor;
    Ok(
        json!({"view":"stream","output":output,"cursor":cursor.to_string(),"running":st.state=="running","omitted":omitted.to_string(),"exit_code":st.exit,"output_path":s.output_path,"capture_error":st.error}),
    )
}
fn key_bytes(key: &str) -> Result<&'static [u8]> {
    Ok(match key.to_ascii_lowercase().as_str() {
        "enter" => b"\r",
        "tab" => b"\t",
        "esc" | "escape" => b"\x1b",
        "backspace" => b"\x7f",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        "ctrl+c" => b"\x03",
        "ctrl+d" => b"\x04",
        "ctrl+z" => b"\x1a",
        "ctrl+l" => b"\x0c",
        "ctrl+a" => b"\x01",
        "ctrl+e" => b"\x05",
        "ctrl+u" => b"\x15",
        "ctrl+k" => b"\x0b",
        "ctrl+r" => b"\x12",
        _ => {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Unsupported named terminal key",
            ));
        }
    })
}

#[cfg(unix)]
mod backend {
    use super::*;
    use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
    pub struct Process {
        child: Box<dyn Child + Send + Sync>,
        master: Option<Box<dyn MasterPty + Send>>,
        pid: i32,
        reaped: bool,
    }
    pub fn spawn(
        command: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        environment: &BTreeMap<String, String>,
    ) -> Result<(Box<dyn Read + Send>, Box<dyn Write + Send>, Process)> {
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "PTY unavailable"))?;
        let mut cmd = CommandBuilder::new("bash");
        cmd.args(["-c", command]);
        cmd.cwd(cwd);
        for (k, v) in environment {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "PTY process could not start"))?;
        let pid = child
            .process_id()
            .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "PTY process id missing"))?
            as i32;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|_| Error::new("IO_FAILURE", "PTY reader unavailable"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|_| Error::new("IO_FAILURE", "PTY writer unavailable"))?;
        drop(pair.slave);
        Ok((
            reader,
            writer,
            Process {
                child,
                master: Some(pair.master),
                pid,
                reaped: false,
            },
        ))
    }
    impl Process {
        pub fn pid(&self) -> u32 {
            self.pid as u32
        }
        pub fn request_stop(&self) {
            unsafe {
                if !self.reaped {
                    libc::kill(-self.pid, libc::SIGTERM);
                }
            }
        }
        pub fn known_empty(&self) -> bool {
            unsafe {
                libc::kill(-self.pid, 0) == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            }
        }
        pub fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
            let status = self.child.try_wait()?;
            if status.is_some() {
                self.reaped = true;
            }
            Ok(status.map(|s| s.exit_code()))
        }
        pub fn kill_group(&self) {
            unsafe {
                if !self.reaped {
                    libc::kill(-self.pid, libc::SIGKILL);
                }
            }
        }
        pub fn kill(&mut self) {
            if self.reaped {
                return;
            }
            self.kill_group();
            let _ = self.child.kill();
        }
        pub fn close_terminal(&mut self) {
            self.master.take();
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            self.kill();
        }
    }
}
#[cfg(windows)]
mod backend {
    use super::*;
    use std::{
        ffi::OsStr,
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
    };
    use windows_sys::Win32::{
        Foundation::*,
        System::{Console::*, JobObjects::*, Pipes::*, Threading::*},
    };
    struct Console(isize);
    impl Drop for Console {
        fn drop(&mut self) {
            unsafe {
                ClosePseudoConsole(self.0);
            }
        }
    }
    pub struct Process {
        process: OwnedHandle,
        job: OwnedHandle,
        console: Option<Console>,
    }
    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
        unsafe {
            let mut r = std::ptr::null_mut();
            let mut w = std::ptr::null_mut();
            if CreatePipe(&mut r, &mut w, std::ptr::null(), 0) == 0 {
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            Ok((
                OwnedHandle::from_raw_handle(r),
                OwnedHandle::from_raw_handle(w),
            ))
        }
    }
    pub fn spawn(
        command: &str,
        cwd: &Path,
        rows: u16,
        cols: u16,
        environment: &BTreeMap<String, String>,
    ) -> Result<(Box<dyn Read + Send>, Box<dyn Write + Send>, Process)> {
        // SAFETY: every checked native handle is immediately owned. The child starts
        // suspended, enters its kill-on-close job, and only then resumes.
        unsafe {
            let (input_read, input_write) = pipe()?;
            let (output_read, output_write) = pipe()?;
            let mut pc = 0;
            if CreatePseudoConsole(
                COORD {
                    X: cols as i16,
                    Y: rows as i16,
                },
                input_read.as_raw_handle(),
                output_write.as_raw_handle(),
                0,
                &mut pc,
            ) < 0
            {
                return Err(Error::new(
                    "FEATURE_DISABLED",
                    "Windows ConPTY requires Windows 10 1809 or later",
                ));
            }
            let console = Console(pc);
            drop(input_read);
            drop(output_write);
            let mut size = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
            let mut memory = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
            let attributes = memory.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            if InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) == 0 {
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            struct Attr(LPPROC_THREAD_ATTRIBUTE_LIST);
            impl Drop for Attr {
                fn drop(&mut self) {
                    unsafe {
                        DeleteProcThreadAttributeList(self.0);
                    }
                }
            }
            let _attr = Attr(attributes);
            if UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                pc as *const _,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdOutput = INVALID_HANDLE_VALUE;
            si.StartupInfo.hStdError = INVALID_HANDLE_VALUE;
            si.lpAttributeList = attributes;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let encoded = base64_utf16(command);
            let mut line = wide(OsStr::new(&format!(
                "powershell.exe -NoLogo -NoProfile -EncodedCommand {encoded}"
            )));
            let cwd = wide(cwd.as_os_str());
            let mut inherited: BTreeMap<String, (String, String)> = std::env::vars_os()
                .map(|(k, v)| {
                    let k = k.to_string_lossy().into_owned();
                    (k.to_uppercase(), (k, v.to_string_lossy().into_owned()))
                })
                .collect();
            for (k, v) in environment {
                inherited.insert(k.to_uppercase(), (k.clone(), v.clone()));
            }
            let mut envblock: Vec<u16> = inherited
                .values()
                .flat_map(|(k, v)| {
                    format!("{k}={v}")
                        .encode_utf16()
                        .chain(Some(0))
                        .collect::<Vec<_>>()
                })
                .collect();
            envblock.push(0);
            if CreateProcessW(
                std::ptr::null(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                envblock.as_ptr() as _,
                cwd.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            ) == 0
            {
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            let process = OwnedHandle::from_raw_handle(pi.hProcess);
            let thread = OwnedHandle::from_raw_handle(pi.hThread);
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                TerminateProcess(process.as_raw_handle(), 1);
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            let job = OwnedHandle::from_raw_handle(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) == 0
                || ResumeThread(thread.as_raw_handle()) == u32::MAX
            {
                TerminateProcess(process.as_raw_handle(), 1);
                return Err(Error::io(std::io::Error::last_os_error()));
            }
            drop(thread);
            let reader: fs::File = output_read.into();
            let writer: fs::File = input_write.into();
            Ok((
                Box::new(reader),
                Box::new(writer),
                Process {
                    process,
                    job,
                    console: Some(console),
                },
            ))
        }
    }
    fn base64_utf16(s: &str) -> String {
        const B: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = s
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let a = chunk[0] as u32;
            let b = *chunk.get(1).unwrap_or(&0) as u32;
            let c = *chunk.get(2).unwrap_or(&0) as u32;
            let v = (a << 16) | (b << 8) | c;
            out.push(B[((v >> 18) & 63) as usize] as char);
            out.push(B[((v >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                B[((v >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                B[(v & 63) as usize] as char
            } else {
                '='
            });
        }
        out
    }
    impl Process {
        pub fn pid(&self) -> u32 {
            unsafe { GetProcessId(self.process.as_raw_handle()) }
        }
        pub fn request_stop(&self) {}
        pub fn known_empty(&self) -> bool {
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                QueryInformationJobObject(
                    self.job.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                ) != 0
                    && info.ActiveProcesses == 0
            }
        }
        pub fn try_wait(&mut self) -> std::io::Result<Option<u32>> {
            unsafe {
                match WaitForSingleObject(self.process.as_raw_handle(), 0) {
                    0 => {
                        let mut code = 0;
                        if GetExitCodeProcess(self.process.as_raw_handle(), &mut code) == 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(Some(code))
                    }
                    258 => Ok(None),
                    _ => Err(std::io::Error::last_os_error()),
                }
            }
        }
        pub fn kill_group(&self) {
            unsafe {
                TerminateJobObject(self.job.as_raw_handle(), 1);
            }
        }
        pub fn kill(&mut self) {
            self.kill_group();
        }
        pub fn close_terminal(&mut self) {
            self.console.take();
        }
    }
    impl Drop for Process {
        fn drop(&mut self) {
            self.kill();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::Actor;
    #[test]
    fn forced_terminal_receipt_is_scoped_and_kept_after_terminal_exit() {
        let dir = tempfile::tempdir().unwrap();
        let registry = CapabilityRegistry::default();
        let cap = registry.grant_directory(dir.path()).unwrap();
        let host = PtySupervisor::default();
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
        let command = if cfg!(windows) {
            "Start-Sleep -Seconds 30"
        } else {
            "sleep 30"
        };
        let opened = host
            .execute(
                &registry,
                "shell_open",
                &json!({"directory_capability":cap,"command":command,"mode":"hands-free"}),
                &context,
            )
            .unwrap();
        let run = context.actor.run_id.as_deref().unwrap();
        let epoch = &context.actor.engine_epoch;
        assert!(host.inspect("unrelated", epoch).is_empty());
        assert!(host.inspect(run, "old_epoch").is_empty());
        host.force_stop_run(run, epoch);
        let end = Instant::now() + Duration::from_secs(6);
        let stopped = loop {
            let result = host.inspect(run, epoch);
            if result[0]["state"] != "running" && result[0]["output_drained"] == true {
                break result;
            }
            assert!(Instant::now() < end, "terminal cleanup did not finish");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(stopped[0]["execution_id"], opened["session_id"]);
        assert_eq!(stopped[0]["leader_exited"], true);
        assert_eq!(stopped[0]["cleanup_confirmed"], cfg!(windows));
        assert!(stopped[0].get("command").is_none());
        assert!(stopped[0].get("env").is_none());
        // Listing cannot discard the proof needed by a subsequent deletion.
        host.execute(&registry, "shell_list", &json!({}), &context)
            .unwrap();
        assert_eq!(host.inspect(run, epoch).len(), 1);
    }
    #[test]
    fn actual_terminal_echo_screen_and_owner_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let registry = CapabilityRegistry::default();
        let cap = registry.grant_directory(dir.path()).unwrap();
        let host = PtySupervisor::default();
        let context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some("agent_test".into()),
                run_id: Some("run_test".into()),
                engine_epoch: "epoch_test".into(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let command = if cfg!(windows) {
            "[Console]::WriteLine('PTY_READY'); while ($true) { $v = [Console]::ReadLine(); if ($null -eq $v) { break }; [Console]::WriteLine('ECHO_'+$v) }"
        } else {
            "printf 'PTY_READY\\n'; while read line; do echo ECHO_$line; done"
        };
        let opened=host.execute(&registry,"shell_open",&json!({"directory_capability":cap,"command":command,"mode":"interactive","prompt_regex":"PTY_READY","startup_grace_ms":0,"quiet_ms":200}),&context).unwrap();
        let id = opened["session_id"].as_str().unwrap();
        assert!(
            host.execute(&registry, "shell_list", &json!({}), &context)
                .unwrap()
                .to_string()
                .contains(id)
        );
        let sent=host.execute(&registry,"shell_send",&json!({"session_id":id,"text":"hello","submit":true,"prompt_regex":"ECHO_hello","timeout_ms":5000}),&context).unwrap();
        assert!(
            sent["output"].as_str().unwrap().contains("ECHO_hello"),
            "{sent}"
        );
        let screen = host
            .execute(
                &registry,
                "shell_read",
                &json!({"session_id":id,"view":"screen"}),
                &context,
            )
            .unwrap();
        assert!(screen["screen"].to_string().contains("hello"));
        let mut other = context.clone();
        other.actor.run_id = Some("other".into());
        assert_eq!(
            host.execute(&registry, "shell_close", &json!({"session_id":id}), &other)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        host.stop_run("run_test", "epoch_test");
        let result = host
            .execute(
                &registry,
                "shell_close",
                &json!({"session_id":id}),
                &context,
            )
            .unwrap();
        assert_eq!(result["closed"], id);
    }
}
