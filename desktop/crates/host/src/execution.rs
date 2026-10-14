use artex_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
const OUTPUT_LIMIT: usize = 65_536;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecutionRequest {
    pub execution_id: String,
    pub run_id: String,
    pub engine_epoch: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_ms: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct ExecutionStatus {
    pub execution_id: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub output: String,
    pub first_cursor: String,
    pub last_cursor: String,
    pub truncated: bool,
    pub output_path: String,
    pub capture_error: Option<String>,
    pub pid: u32,
    pub stop_requested: bool,
    pub force_requested: bool,
    pub leader_exited: bool,
    pub output_drained: bool,
    pub known_cleanup_confirmed: bool,
    pub descendant_cleanup_confirmed: bool,
    pub cleanup_confirmed: bool,
    pub cleanup_state: String,
    pub containment: String,
    pub executable: String,
    pub kind: String,
}
struct State {
    status: ExecutionStatus,
    bytes: Vec<u8>,
    total: u64,
    file: std::fs::File,
    io_done: Arc<AtomicUsize>,
    io_expected: usize,
}
struct Execution {
    request: ExecutionRequest,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    force: Arc<AtomicBool>,
    input_hash: String,
}
#[derive(Default)]
pub struct ExecutionSupervisor {
    executions: Mutex<HashMap<String, Execution>>,
}
pub(crate) struct ExternalLease {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    force: Arc<AtomicBool>,
}
impl ExternalLease {
    pub(crate) fn started(&self, pid: u32) {
        self.state.lock().unwrap().status.pid = pid;
    }
    pub(crate) fn abort_unstarted(&self) {
        let mut state = self.state.lock().unwrap();
        state.io_expected = 0;
        state.status.state = "failed".into();
        state.status.leader_exited = true;
        state.status.known_cleanup_confirmed = true;
        state.status.descendant_cleanup_confirmed = true;
        refresh_status(&mut state);
    }
    pub(crate) fn force_requested(&self) -> bool {
        self.force.load(Ordering::SeqCst)
    }
    pub(crate) fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
    pub(crate) fn io_counter(&self) -> Arc<AtomicUsize> {
        self.state.lock().unwrap().io_done.clone()
    }
    pub(crate) fn finish(&self, leader_exited: bool, exit_code: Option<i32>, group_empty: bool) {
        let mut state = self.state.lock().unwrap();
        state.status.leader_exited = leader_exited;
        state.status.exit_code = exit_code;
        state.status.known_cleanup_confirmed = leader_exited && group_empty;
        state.status.descendant_cleanup_confirmed = cfg!(windows) && leader_exited && group_empty;
        state.status.state = if self.stopping() {
            "cancelled"
        } else {
            "completed"
        }
        .into();
        refresh_status(&mut state);
    }
}
impl ExecutionSupervisor {
    pub(crate) fn register_external(
        &self,
        request: ExecutionRequest,
        pid: u32,
        kind: &str,
        io_expected: usize,
    ) -> Result<ExternalLease> {
        let mut map = self.executions.lock().unwrap();
        if map.len() >= 1024 {
            return Err(Error::new(
                "BUSY",
                "External process retention limit reached",
            ));
        }
        if map.contains_key(&request.execution_id) {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "External execution id already exists",
            ));
        }
        let state = Arc::new(Mutex::new(State {
            status: ExecutionStatus {
                execution_id: request.execution_id.clone(),
                state: "running".into(),
                exit_code: None,
                output: String::new(),
                first_cursor: "0".into(),
                last_cursor: "0".into(),
                truncated: false,
                output_path: String::new(),
                capture_error: None,
                pid,
                stop_requested: false,
                force_requested: false,
                leader_exited: false,
                output_drained: false,
                known_cleanup_confirmed: false,
                descendant_cleanup_confirmed: false,
                cleanup_confirmed: false,
                cleanup_state: "running".into(),
                containment: if cfg!(windows) {
                    "windows_job"
                } else {
                    "unix_process_group_unconfined"
                }
                .into(),
                executable: request
                    .executable
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                kind: kind.into(),
            },
            bytes: vec![],
            total: 0,
            file: tempfile::tempfile().map_err(Error::io)?,
            io_done: Arc::new(AtomicUsize::new(0)),
            io_expected,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let force = Arc::new(AtomicBool::new(false));
        map.insert(
            request.execution_id.clone(),
            Execution {
                request,
                state: state.clone(),
                stop: stop.clone(),
                force: force.clone(),
                input_hash: String::new(),
            },
        );
        Ok(ExternalLease { state, stop, force })
    }
    pub fn force_stop_run(&self, run: &str, epoch: &str) {
        for execution in self.executions.lock().unwrap().values() {
            if execution.request.run_id == run && execution.request.engine_epoch == epoch {
                execution.stop.store(true, Ordering::SeqCst);
                execution.force.store(true, Ordering::SeqCst);
                let mut state = execution.state.lock().unwrap();
                state.status.stop_requested = true;
                state.status.force_requested = true;
                refresh_status(&mut state);
            }
        }
    }
    /// Engine-only boundary: caller must check current run policy before entering.
    /// Duplicate IDs return existing state; differing parameters are rejected.
    pub fn execute(&self, request: ExecutionRequest) -> Result<ExecutionStatus> {
        self.execute_with_input(request, None, &std::collections::BTreeMap::new())
    }
    pub fn execute_with_input(
        &self,
        request: ExecutionRequest,
        stdin: Option<Vec<u8>>,
        environment: &std::collections::BTreeMap<String, String>,
    ) -> Result<ExecutionStatus> {
        if stdin.as_ref().is_some_and(|s| s.len() > 65536) || environment.len() > 256 {
            return Err(Error::new("SIZE_LIMIT", "Process input budget exceeded"));
        }
        use sha2::{Digest, Sha256};
        let input_hash = hex::encode(Sha256::digest(
            serde_json::to_vec(&(stdin.as_ref(), environment))
                .map_err(|_| Error::new("INVALID_PARAMS", "Cannot encode process input"))?,
        ));
        if request.run_id.is_empty()
            || request.engine_epoch.is_empty()
            || request.timeout_ms == 0
            || request.timeout_ms > 3_600_000
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Run ownership and timeout 1..3600000 required",
            ));
        }
        let output_dir = request.cwd.join(".artex-host").join("outputs");
        for path in [request.cwd.join(".artex-host"), output_dir.clone()] {
            if let Ok(metadata) = std::fs::symlink_metadata(&path) {
                if metadata.file_type().is_symlink()
                    || !path
                        .canonicalize()
                        .map_err(Error::io)?
                        .starts_with(request.cwd.canonicalize().map_err(Error::io)?)
                {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Output directory link escapes execution root",
                    ));
                }
            }
            std::fs::create_dir_all(path).map_err(Error::io)?;
        }
        let output_name = format!(
            "{}.log",
            hex::encode(Sha256::digest(
                format!("{}\0{}", request.engine_epoch, request.execution_id).as_bytes()
            ))
        );
        let output_path = format!(".artex-host/outputs/{output_name}");
        let mut executions = self.executions.lock().unwrap();
        if let Some(e) = executions.get(&request.execution_id) {
            if e.request != request || e.input_hash != input_hash {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Execution id reused with different parameters",
                ));
            }
            return Ok(e.state.lock().unwrap().status.clone());
        }
        if executions.len() >= 1024 {
            return Err(Error::new("BUSY", "Execution retention limit reached"));
        }
        let output_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(output_dir.join(output_name))
            .map_err(Error::io)?;
        let mut command = Command::new(&request.executable);
        command
            .args(&request.args)
            .current_dir(&request.cwd)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.envs(environment);
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
        let mut child = command.spawn().map_err(Error::io)?;
        let group = ProcessGroup::attach(&mut child)?;
        let io_done = Arc::new(AtomicUsize::new(0));
        let input_writer = stdin.and_then(|bytes| {
            child.stdin.take().map(|mut pipe| {
                let done = io_done.clone();
                thread::spawn(move || {
                    let _ = pipe.write_all(&bytes);
                    done.fetch_add(1, Ordering::SeqCst);
                })
            })
        });
        let io_expected = 2 + usize::from(input_writer.is_some());
        let state = Arc::new(Mutex::new(State {
            status: ExecutionStatus {
                execution_id: request.execution_id.clone(),
                state: "running".into(),
                exit_code: None,
                output: String::new(),
                first_cursor: "0".into(),
                last_cursor: "0".into(),
                truncated: false,
                output_path,
                capture_error: None,
                pid: child.id(),
                stop_requested: false,
                force_requested: false,
                leader_exited: false,
                output_drained: false,
                known_cleanup_confirmed: false,
                descendant_cleanup_confirmed: false,
                cleanup_confirmed: false,
                cleanup_state: "running".into(),
                containment: if cfg!(windows) {
                    "windows_job"
                } else {
                    "unix_process_group_unconfined"
                }
                .into(),
                executable: request
                    .executable
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                kind: "process".into(),
            },
            bytes: vec![],
            total: 0,
            file: output_file,
            io_done: io_done.clone(),
            io_expected,
        }));
        let mut drains = vec![];
        for reader in [
            child
                .stdout
                .take()
                .map(|v| Box::new(v) as Box<dyn Read + Send>),
            child
                .stderr
                .take()
                .map(|v| Box::new(v) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let state = state.clone();
            let done = io_done.clone();
            drains.push(thread::spawn(move || {
                drain(reader, state);
                done.fetch_add(1, Ordering::SeqCst);
            }));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let st = state.clone();
        let stopped = stop.clone();
        let force = Arc::new(AtomicBool::new(false));
        let forced = force.clone();
        let timeout = Duration::from_millis(request.timeout_ms);
        thread::spawn(move || {
            let start = Instant::now();
            let mut stopping: Option<(&str, Instant)> = None;
            let mut forced_at: Option<Instant> = None;
            let (terminal, exit_code, leader_exited) = loop {
                let reason = if st.lock().unwrap().status.capture_error.is_some() {
                    Some("failed")
                } else if stopped.load(Ordering::SeqCst) {
                    Some("cancelled")
                } else if start.elapsed() >= timeout {
                    Some("timeout")
                } else {
                    None
                };
                if let Some(reason) = reason {
                    if stopping.is_none() {
                        group.request_stop();
                        stopping = Some((reason, Instant::now()));
                    }
                }
                if forced.load(Ordering::SeqCst)
                    || stopping.is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(500))
                {
                    if forced_at.is_none() {
                        group.kill(&mut child);
                        forced_at = Some(Instant::now());
                        forced.store(true, Ordering::SeqCst);
                    }
                }
                if forced_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(2)) {
                    break (
                        stopping
                            .map(|(reason, _)| reason)
                            .unwrap_or("interrupted")
                            .to_owned(),
                        None,
                        false,
                    );
                }
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        group.mark_leader_reaped();
                        break (
                            stopping
                                .map(|(reason, _)| reason)
                                .unwrap_or(if exit.success() {
                                    "completed"
                                } else {
                                    "failed"
                                })
                                .to_owned(),
                            exit.code(),
                            true,
                        );
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(_) => {
                        group.kill(&mut child);
                        break ("interrupted".into(), None, false);
                    }
                }
            };
            // Job handles remain safe after exit. Unix group numbers do not:
            // after reaping the leader never signal a possibly recycled PGID.
            group.kill(&mut child);
            let end = Instant::now() + Duration::from_secs(2);
            while Instant::now() < end
                && (!group.known_empty() || io_done.load(Ordering::SeqCst) < io_expected)
            {
                thread::sleep(Duration::from_millis(10));
            }
            let group_empty = group.known_empty();
            for drain in drains {
                if drain.is_finished() {
                    let _ = drain.join();
                }
            }
            if let Some(writer) = input_writer {
                if writer.is_finished() {
                    let _ = writer.join();
                }
            }
            let mut s = st.lock().unwrap();
            if s.file.sync_all().is_err() {
                s.status.capture_error = Some("IO_FAILURE".into());
            }
            s.status.state = terminal;
            s.status.exit_code = exit_code;
            s.status.leader_exited = leader_exited;
            s.status.stop_requested = stopped.load(Ordering::SeqCst);
            s.status.force_requested = forced.load(Ordering::SeqCst);
            s.status.known_cleanup_confirmed = leader_exited && group_empty;
            s.status.descendant_cleanup_confirmed = cfg!(windows) && leader_exited && group_empty;
            refresh_status(&mut s);
        });
        let status = state.lock().unwrap().status.clone();
        executions.insert(
            request.execution_id.clone(),
            Execution {
                request,
                state,
                stop,
                force,
                input_hash,
            },
        );
        Ok(status)
    }
    pub fn status(&self, id: &str, run: &str, epoch: &str) -> Result<ExecutionStatus> {
        let map = self.executions.lock().unwrap();
        let e = map
            .get(id)
            .ok_or_else(|| Error::new("NOT_FOUND", "Execution not found"))?;
        check_owner(e, run, epoch)?;
        let mut state = e.state.lock().unwrap();
        refresh_status(&mut state);
        Ok(state.status.clone())
    }
    pub fn output(&self, id: &str, run: &str, epoch: &str, cursor: u64) -> Result<ExecutionStatus> {
        let map = self.executions.lock().unwrap();
        let e = map
            .get(id)
            .ok_or_else(|| Error::new("NOT_FOUND", "Execution not found"))?;
        check_owner(e, run, epoch)?;
        let s = e.state.lock().unwrap();
        let first = s.total - s.bytes.len() as u64;
        if cursor < first || cursor > s.total {
            return Err(Error::new(
                "CURSOR_EXPIRED",
                "Output cursor is outside retained range",
            ));
        }
        let mut out = s.status.clone();
        out.output = String::from_utf8_lossy(&s.bytes[(cursor - first) as usize..]).into_owned();
        Ok(out)
    }
    pub fn output_file_tail(
        &self,
        id: &str,
        run: &str,
        epoch: &str,
        max_bytes: u64,
    ) -> Result<ExecutionStatus> {
        let map = self.executions.lock().unwrap();
        let e = map
            .get(id)
            .ok_or_else(|| Error::new("NOT_FOUND", "Execution not found"))?;
        check_owner(e, run, epoch)?;
        let mut s = e.state.lock().unwrap();
        let start = s.total.saturating_sub(max_bytes.min(65536));
        let total = s.total;
        let mut bytes = vec![0; (total - start) as usize];
        s.file
            .seek(SeekFrom::Start(start))
            .and_then(|_| s.file.read_exact(&mut bytes))
            .and_then(|_| s.file.seek(SeekFrom::End(0)).map(|_| ()))
            .map_err(Error::io)?;
        let mut out = s.status.clone();
        out.output = String::from_utf8_lossy(&bytes).into_owned();
        Ok(out)
    }
    pub fn stop(&self, id: &str, run: &str, epoch: &str) -> Result<ExecutionStatus> {
        let map = self.executions.lock().unwrap();
        let e = map
            .get(id)
            .ok_or_else(|| Error::new("NOT_FOUND", "Execution not found"))?;
        check_owner(e, run, epoch)?;
        e.stop.store(true, Ordering::SeqCst);
        let mut state = e.state.lock().unwrap();
        state.status.stop_requested = true;
        refresh_status(&mut state);
        Ok(state.status.clone())
    }
    pub fn stop_run(&self, run: &str, epoch: &str) {
        for e in self.executions.lock().unwrap().values() {
            if e.request.run_id == run && e.request.engine_epoch == epoch {
                e.stop.store(true, Ordering::SeqCst);
                e.state.lock().unwrap().status.stop_requested = true;
            }
        }
    }
    pub fn stop_epoch(&self, epoch: &str) {
        for e in self.executions.lock().unwrap().values() {
            if e.request.engine_epoch == epoch {
                e.stop.store(true, Ordering::SeqCst);
                e.force.store(true, Ordering::SeqCst);
                e.state.lock().unwrap().status.stop_requested = true;
            }
        }
    }
    pub fn epoch_active(&self, epoch: &str) -> bool {
        self.executions.lock().unwrap().values().any(|execution| {
            execution.request.engine_epoch == epoch && {
                let state = execution.state.lock().unwrap();
                state.status.state == "running"
                    || state.io_done.load(Ordering::SeqCst) < state.io_expected
                    || (cfg!(windows) && !state.status.known_cleanup_confirmed)
            }
        })
    }
    pub fn list(&self, run: &str, epoch: &str) -> Vec<ExecutionStatus> {
        self.executions
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.request.run_id == run && e.request.engine_epoch == epoch)
            .map(|e| {
                let mut state = e.state.lock().unwrap();
                refresh_status(&mut state);
                let mut s = state.status.clone();
                s.output.clear();
                s
            })
            .collect()
    }
}
fn refresh_status(state: &mut State) {
    state.status.output_drained = state.io_done.load(Ordering::SeqCst) >= state.io_expected;
    state.status.cleanup_confirmed = state.status.leader_exited
        && state.status.output_drained
        && state.status.descendant_cleanup_confirmed;
    state.status.cleanup_state = if state.status.cleanup_confirmed {
        "confirmed"
    } else if state.status.state == "running" {
        if state.status.stop_requested {
            "stopping"
        } else {
            "running"
        }
    } else {
        "unconfirmed"
    }
    .into();
}
impl Drop for ExecutionSupervisor {
    fn drop(&mut self) {
        let map = self.executions.lock().unwrap();
        for e in map.values() {
            e.stop.store(true, Ordering::SeqCst);
            e.force.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && map.values().any(|e| {
                let state = e.state.lock().unwrap();
                state.status.state == "running"
                    || state.io_done.load(Ordering::SeqCst) < state.io_expected
            })
        {
            thread::sleep(Duration::from_millis(10));
        }
    }
}
fn check_owner(e: &Execution, run: &str, epoch: &str) -> Result<()> {
    if e.request.run_id != run || e.request.engine_epoch != epoch {
        return Err(Error::new("PERMISSION_DENIED", "Execution owner differs"));
    }
    Ok(())
}
fn drain(mut reader: Box<dyn Read + Send>, state: Arc<Mutex<State>>) {
    let mut buf = [0; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut s = state.lock().unwrap();
                if s.status.capture_error.is_none() {
                    if s.total + n as u64 > 1024 * 1024 * 1024 {
                        s.status.capture_error = Some("SIZE_LIMIT".into());
                    } else if s.file.write_all(&buf[..n]).is_err() {
                        s.status.capture_error = Some("IO_FAILURE".into());
                    }
                }
                s.total += n as u64;
                s.bytes.extend_from_slice(&buf[..n]);
                let excess = s.bytes.len().saturating_sub(OUTPUT_LIMIT);
                if excess > 0 {
                    s.bytes.drain(..excess);
                    s.status.truncated = true;
                }
                s.status.first_cursor = (s.total - s.bytes.len() as u64).to_string();
                s.status.last_cursor = s.total.to_string();
                s.status.output = String::from_utf8_lossy(&s.bytes).into_owned();
            }
        }
    }
}
pub(crate) struct ProcessGroup {
    #[cfg(windows)]
    job: usize,
    #[cfg(unix)]
    pid: i32,
    #[cfg(unix)]
    leader_reaped: AtomicBool,
}
impl ProcessGroup {
    pub(crate) fn mark_leader_reaped(&self) {
        #[cfg(unix)]
        self.leader_reaped.store(true, Ordering::SeqCst);
    }
    pub(crate) fn request_stop(&self) {
        #[cfg(unix)]
        unsafe {
            if !self.leader_reaped.load(Ordering::SeqCst) {
                libc::kill(-self.pid, libc::SIGTERM);
            }
        }
        // No console signal is sent on Windows: attaching to a shared console
        // could interrupt ARTEX or unrelated processes. Force is job-scoped.
    }
    pub(crate) fn known_empty(&self) -> bool {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-self.pid, 0) == -1
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            }
        }
        #[cfg(windows)]
        {
            unsafe {
                use windows_sys::Win32::System::JobObjects::*;
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                QueryInformationJobObject(
                    self.job as _,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as _,
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                ) != 0
                    && info.ActiveProcesses == 0
            }
        }
    }
    pub(crate) fn attach(child: &mut Child) -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                pid: child.id() as i32,
                leader_reaped: AtomicBool::new(false),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            // A suspended child cannot spawn descendants before job assignment.
            // std::process does not expose its primary-thread handle; documented
            // ToolHelp/OpenThread/ResumeThread APIs recover that handle after the
            // suspended process has been assigned to the owned job.
            // SAFETY: native handles are checked, closed exactly once, and structs
            // have the ABI-required sizes. Child remains alive during attachment.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    let _ = child.kill();
                    return Err(Error::io(std::io::Error::last_os_error()));
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as _,
                    std::mem::size_of_val(&info) as u32,
                ) == 0
                    || AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0
                {
                    CloseHandle(job);
                    let _ = child.kill();
                    return Err(Error::io(std::io::Error::last_os_error()));
                }
                if resume_owned_process(child.id()).is_err() {
                    CloseHandle(job);
                    let _ = child.kill();
                    return Err(Error::new("IO_FAILURE", "Cannot resume owned child"));
                }
                Ok(Self { job: job as usize })
            }
        }
    }
    pub(crate) fn kill(&self, child: &mut Child) {
        #[cfg(unix)]
        unsafe {
            if self.leader_reaped.load(Ordering::SeqCst) {
                return;
            }
            libc::kill(-self.pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job as _, 1);
        }
        let _ = child.kill();
    }
}
#[cfg(windows)]
fn resume_owned_process(pid: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::*,
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    // SAFETY: the snapshot and thread handles are checked before use and closed;
    // THREADENTRY32 is zero initialized with its documented size set.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(Error::io(std::io::Error::last_os_error()));
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut found = false;
        let mut next = Thread32First(snapshot, &mut entry);
        while next != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    let previous = ResumeThread(thread);
                    CloseHandle(thread);
                    if previous != u32::MAX {
                        found = true;
                    }
                }
            }
            next = Thread32Next(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        if found {
            Ok(())
        } else {
            Err(Error::new(
                "IO_FAILURE",
                "Suspended child thread could not be resumed",
            ))
        }
    }
}
#[cfg(windows)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn force_stop_is_owned_and_cleanup_is_a_separate_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = ExecutionSupervisor::default();
        let mut first = req(if cfg!(windows) {
            "ping -n 30 127.0.0.1 >NUL"
        } else {
            "sleep 30"
        });
        first.cwd = dir.path().into();
        first.timeout_ms = 30000;
        let mut other = first.clone();
        other.execution_id = "exec_other".into();
        other.run_id = "run_other".into();
        supervisor.execute(first).unwrap();
        supervisor.execute(other).unwrap();
        assert!(supervisor.list("run_test", "wrong_epoch").is_empty());
        supervisor.stop_run("run_test", "epoch");
        let requested = supervisor
            .status("execution_test", "run_test", "epoch")
            .unwrap();
        assert!(requested.stop_requested);
        supervisor.force_stop_run("run_test", "epoch");
        let end = Instant::now() + Duration::from_secs(6);
        let stopped = loop {
            let value = supervisor
                .status("execution_test", "run_test", "epoch")
                .unwrap();
            if value.state != "running" {
                break value;
            }
            assert!(Instant::now() < end, "owned execution did not terminate");
            thread::sleep(Duration::from_millis(10));
        };
        assert!(stopped.leader_exited && stopped.output_drained);
        assert_eq!(stopped.cleanup_confirmed, cfg!(windows));
        if cfg!(windows) {
            assert!(stopped.known_cleanup_confirmed && stopped.descendant_cleanup_confirmed);
        } else {
            assert_eq!(stopped.cleanup_state, "unconfirmed");
        }
        let untouched = supervisor
            .status("exec_other", "run_other", "epoch")
            .unwrap();
        assert_eq!(untouched.state, "running");
        assert!(!untouched.stop_requested);
        supervisor.force_stop_run("run_other", "epoch");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn detached_descendant_drain_is_bounded_and_never_claimed_confined() {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = ExecutionSupervisor::default();
        // The fixture expires itself. Production code never kills this escaped
        // PID: there is no retained native scope proving ownership of it.
        let mut request = req("setsid sh -c 'sleep 8; printf escaped' & exit 0");
        request.cwd = dir.path().into();
        request.timeout_ms = 30000;
        supervisor.execute(request).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        let status = loop {
            let status = supervisor
                .status("execution_test", "run_test", "epoch")
                .unwrap();
            if status.state != "running" {
                break status;
            }
            assert!(
                Instant::now() < end,
                "output drain blocked supervisor indefinitely"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert!(status.leader_exited);
        assert!(!status.output_drained);
        assert!(!status.descendant_cleanup_confirmed && !status.cleanup_confirmed);
        assert!(
            supervisor.epoch_active("epoch"),
            "live drain must retain workspace admission"
        );
    }
    fn req(command: &str) -> ExecutionRequest {
        ExecutionRequest {
            execution_id: "execution_test".into(),
            run_id: "run_test".into(),
            engine_epoch: "epoch".into(),
            executable: if cfg!(windows) {
                "cmd.exe".into()
            } else {
                "/bin/sh".into()
            },
            args: if cfg!(windows) {
                vec!["/C".into(), command.into()]
            } else {
                vec!["-c".into(), command.into()]
            },
            cwd: std::env::current_dir().unwrap(),
            timeout_ms: 1000,
        }
    }
    #[test]
    fn owned_execution_and_dedup() {
        let h = ExecutionSupervisor::default();
        let dir = tempfile::tempdir().unwrap();
        let mut r = req("echo artex");
        r.cwd = dir.path().into();
        h.execute(r.clone()).unwrap();
        h.execute(r).unwrap();
        assert!(h.status("execution_test", "other", "epoch").is_err());
        for _ in 0..150 {
            let s = h.status("execution_test", "run_test", "epoch").unwrap();
            if s.state == "completed" {
                thread::sleep(Duration::from_millis(30));
                assert!(
                    h.output("execution_test", "run_test", "epoch", 0)
                        .unwrap()
                        .output
                        .contains("artex")
                );
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("process did not finish");
    }
    #[test]
    fn timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let h = ExecutionSupervisor::default();
        let mut r = req(if cfg!(windows) {
            "ping -n 10 127.0.0.1 >NUL"
        } else {
            "sleep 10"
        });
        r.timeout_ms = 50;
        r.cwd = dir.path().into();
        h.execute(r).unwrap();
        // Concurrent GUI/compiler fixtures may delay scheduling the watchdog.
        // Assert its terminal outcome within a bounded deadline, not a 200ms race.
        let deadline = Instant::now() + Duration::from_secs(5);
        while h
            .status("execution_test", "run_test", "epoch")
            .unwrap()
            .state
            == "running"
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            h.status("execution_test", "run_test", "epoch")
                .unwrap()
                .state,
            "timeout"
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_job_stops_a_spawned_descendant() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child.pid");
        let script = format!(
            "$child=Start-Process ping.exe -ArgumentList @('-t','127.0.0.1') -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText('{}',[string]$child.Id); Start-Sleep -Seconds 30",
            pid_file.to_string_lossy().replace('\'', "''")
        );
        let h = ExecutionSupervisor::default();
        let mut r = req("");
        r.executable = "powershell.exe".into();
        r.args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ];
        r.timeout_ms = 5000;
        r.cwd = dir.path().into();
        h.execute(r).unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !pid_file.is_file() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        h.stop("execution_test", "run_test", "epoch").unwrap();
        // SAFETY: OpenProcess is used only for the PID emitted by this owned
        // child. The checked handle is closed after the bounded termination wait.
        unsafe {
            use windows_sys::Win32::{
                Foundation::CloseHandle,
                System::Threading::{OpenProcess, WaitForSingleObject},
            };
            let process = OpenProcess(0x0010_0000, 0, pid);
            if !process.is_null() {
                let waited = WaitForSingleObject(process, 2000);
                CloseHandle(process);
                assert_eq!(waited, 0, "Descendant remained alive after stop");
            }
        }
    }
    #[cfg(windows)]
    #[test]
    fn output_ring_is_bounded_and_expired_cursor_fails() {
        let dir = tempfile::tempdir().unwrap();
        let h = ExecutionSupervisor::default();
        let mut r = req("");
        r.executable = "powershell.exe".into();
        r.args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "[Console]::Write(('a' * 100000))".into(),
        ];
        r.timeout_ms = 5000;
        r.cwd = dir.path().into();
        h.execute(r).unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        while h
            .status("execution_test", "run_test", "epoch")
            .unwrap()
            .state
            == "running"
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let s = h.status("execution_test", "run_test", "epoch").unwrap();
        assert_eq!(s.state, "completed");
        assert!(s.truncated);
        assert!(s.output.len() <= OUTPUT_LIMIT);
        assert_eq!(s.last_cursor, "100000");
        assert_eq!(
            std::fs::metadata(dir.path().join(&s.output_path))
                .unwrap()
                .len(),
            100000
        );
        assert_eq!(
            h.output_file_tail("execution_test", "run_test", "epoch", 30000)
                .unwrap()
                .output
                .len(),
            30000
        );
        assert_eq!(
            h.output("execution_test", "run_test", "epoch", 0)
                .unwrap_err()
                .code,
            "CURSOR_EXPIRED"
        );
    }
}
