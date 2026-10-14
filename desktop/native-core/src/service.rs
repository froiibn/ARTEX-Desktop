use crate::*;
use artex_core::{Actor, Context, Error, Record, Result};
use artex_rpc::{MAX_QUEUE_BYTES, Router};
use artex_runtime::HostGateway;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub type HostFactory = Arc<dyn Fn(&NativeWorkspace) -> Result<Arc<dyn HostGateway>> + Send + Sync>;
#[derive(Clone, Default)]
pub struct ServiceConfig {
    pub host_factory: Option<HostFactory>,
    /// Must only schedule a repaint; it runs on the writer, never on the UI.
    pub wake_ui: Option<Arc<dyn Fn() + Send + Sync>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServicePhase {
    Closed,
    Opening,
    Ready,
    Closing,
    Shutdown,
    RecoveryRequired,
}
#[derive(Clone, Debug)]
pub struct ServiceStatus {
    pub phase: ServicePhase,
    pub scope: Option<WorkspaceScope>,
    pub read_only: bool,
    pub last_error: Option<Error>,
    pub outstanding: usize,
}
#[derive(Clone, Debug)]
pub struct NativeEvent {
    pub scope: WorkspaceScope,
    pub record: Record,
}
#[derive(Clone, Debug, Default)]
pub struct NativeEventBatch {
    pub events: Vec<NativeEvent>,
    /// Overflow discards event deltas, never the latest committed generation.
    pub resync_required: bool,
}
struct Shared {
    phase: ServicePhase,
    view: Option<NativeReadView>,
    error: Option<Error>,
    completions: VecDeque<NativeCompletion>,
    completion_bytes: usize,
    events: VecDeque<NativeEvent>,
    event_bytes: usize,
    event_overflow: bool,
    cancelled: BTreeSet<NativeTicket>,
    active: BTreeSet<NativeTicket>,
    transition: Option<NativeTicket>,
    opening_ticket: Option<NativeTicket>,
    controls: BTreeSet<NativeTicket>,
    suspend_requested: bool,
    read_jobs: BTreeMap<NativeTicket, Arc<ReadJob>>,
}
#[derive(Default)]
struct ReadJob {
    cancel: AtomicBool,
    finished: AtomicBool,
}
impl Default for Shared {
    fn default() -> Self {
        Self {
            phase: ServicePhase::Closed,
            view: None,
            error: None,
            completions: VecDeque::new(),
            completion_bytes: 0,
            events: VecDeque::new(),
            event_bytes: 0,
            event_overflow: false,
            cancelled: BTreeSet::new(),
            active: BTreeSet::new(),
            transition: None,
            opening_ticket: None,
            controls: BTreeSet::new(),
            suspend_requested: false,
            read_jobs: BTreeMap::new(),
        }
    }
}
struct Work {
    ticket: NativeTicket,
    action: NativeAction,
    queued_at: Instant,
    weight: usize,
}
#[derive(Clone)]
pub struct ServiceHandle {
    tx: SyncSender<Work>,
    priority_tx: SyncSender<Work>,
    interaction_tx: SyncSender<Work>,
    shared: Arc<Mutex<Shared>>,
    outstanding: Arc<AtomicUsize>,
    queued_bytes: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    config: ServiceConfig,
}
pub struct Service {
    handle: ServiceHandle,
    worker: Option<JoinHandle<()>>,
}
impl Service {
    pub fn spawn(config: ServiceConfig) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(64);
        let (priority_tx, priority_rx) = mpsc::sync_channel(1);
        let (interaction_tx, interaction_rx) = mpsc::sync_channel(16);
        let handle = ServiceHandle {
            tx,
            priority_tx,
            interaction_tx,
            shared: Arc::new(Mutex::new(Shared::default())),
            outstanding: Arc::new(AtomicUsize::new(0)),
            queued_bytes: Arc::new(AtomicUsize::new(0)),
            stopped: Arc::new(AtomicBool::new(false)),
            config,
        };
        let h = handle.clone();
        let emergency = h.clone();
        let worker = thread::Builder::new()
            .name("artex-native-writer".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    writer(rx, priority_rx, interaction_rx, h)
                }))
                .is_err()
                {
                    let tickets = lock(&emergency.shared)
                        .active
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>();
                    for ticket in tickets {
                        emergency.complete(error_reply(
                            ticket,
                            Error::new(
                                "RECOVERY_REQUIRED",
                                "Native writer stopped unexpectedly; explicit recovery is required",
                            ),
                        ));
                    }
                    let mut shared = lock(&emergency.shared);
                    shared.phase = ServicePhase::RecoveryRequired;
                    shared.view = None;
                    shared.error = Some(Error::new(
                        "RECOVERY_REQUIRED",
                        "Native writer stopped unexpectedly",
                    ));
                    drop(shared);
                    emergency.stopped.store(true, Ordering::Release);
                    emergency.wake();
                }
            })
            .map_err(Error::io)?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }
    pub fn handle(&self) -> ServiceHandle {
        self.handle.clone()
    }
    /// CLI/tests may join after a successful Shutdown completion. GUI callers
    /// wait for that completion through normal polling instead of blocking.
    pub fn join(mut self) -> Result<()> {
        if self.handle.status().phase != ServicePhase::Shutdown {
            return Err(Error::new(
                "BUSY",
                "Complete orderly shutdown before joining the writer",
            ));
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| Error::new("RECOVERY_REQUIRED", "Native writer panicked"))?;
        }
        Ok(())
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.handle.stopped.store(true, Ordering::Release);
    }
}
fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn weight(action: &NativeAction) -> Result<usize> {
    let path = match action {
        NativeAction::Open(options) => Some(options.path.as_path()),
        NativeAction::StageFile { path, .. } => Some(path.as_path()),
        NativeAction::SaveFile { destination, .. } => Some(destination.as_path()),
        _ => None,
    };
    if path.is_some_and(|path| path.as_os_str().len() > 32 * 1024) {
        return Err(Error::new("SIZE_LIMIT", "Native file path exceeds 32 KiB"));
    }
    if matches!(action,NativeAction::FileRead {path,..}|NativeAction::FileWrite {path,..} if path.len()>32*1024)
    {
        return Err(Error::new("SIZE_LIMIT", "Native file path exceeds 32 KiB"));
    }
    let value = match action {
        NativeAction::Query {
            query: NativeQuery::Feature { params, .. } | NativeQuery::Prepare { params, .. },
            ..
        } => Some(params),
        NativeAction::Command { command, .. } => Some(&command.params),
        NativeAction::SkillMetaWrite { meta, .. } => Some(meta),
        NativeAction::StageBody { body, .. } | NativeAction::StageBase64Body { body, .. } => {
            Some(body)
        }
        _ => None,
    };
    // Persistent DTO JSON is data, not an application transport envelope.
    let bytes = value.map(bounded_value_size).transpose()?.unwrap_or(1024);
    let bytes = match action {
        NativeAction::SetSecret { value, field, .. } => bytes
            .saturating_add(value.len())
            .saturating_add(field.len()),
        NativeAction::FileWrite { content, path, .. } => bytes
            .saturating_add(content.len())
            .saturating_add(path.len()),
        NativeAction::Command { command, .. } => command.expected.iter().fold(bytes, |n, row| {
            n.saturating_add(row.collection.len())
                .saturating_add(row.id.len())
                .saturating_add(row.version.len())
                .saturating_add(256)
        }),
        NativeAction::RunControl {
            expected, target, ..
        } => bytes
            .saturating_add(
                expected
                    .iter()
                    .map(|e| e.collection.len() + e.id.len() + e.version.len() + 96)
                    .sum::<usize>(),
            )
            .saturating_add(match target {
                RunControlTarget::Run { run_id } => run_id.len(),
                RunControlTarget::Task { task_id } => task_id.len(),
            }),
        NativeAction::ConversationManage { expected, ids, .. }
        | NativeAction::TaskDelete { expected, ids, .. } => bytes
            .saturating_add(
                expected
                    .iter()
                    .map(|e| e.collection.len() + e.id.len() + e.version.len() + 96)
                    .sum::<usize>(),
            )
            .saturating_add(ids.iter().map(String::len).sum::<usize>()),
        NativeAction::Query {
            query: NativeQuery::Objects(query),
            ..
        } => query
            .ids
            .iter()
            .fold(bytes, |n, id| n.saturating_add(id.len()).saturating_add(16)),
        NativeAction::Query {
            query: NativeQuery::Body { body },
            ..
        } => bounded_value_size(body)?,
        NativeAction::Query {
            query: NativeQuery::DeletionPreview { target, .. },
            ..
        } => bytes.saturating_add(match target {
            DeletionTarget::Conversations { ids } | DeletionTarget::Tasks { ids } => {
                ids.iter().map(String::len).sum::<usize>()
            }
        }),
        NativeAction::FileRead { path, .. } => bytes.saturating_add(path.len()),
        NativeAction::StageFile { path, .. } => bytes.saturating_add(path.as_os_str().len()),
        NativeAction::SaveFile {
            destination, token, ..
        } => bytes
            .saturating_add(destination.as_os_str().len())
            .saturating_add(token.len()),
        NativeAction::ReleaseFile { token, .. } => bytes.saturating_add(token.len()),
        NativeAction::StageBody { name, .. } | NativeAction::StageBase64Body { name, .. } => {
            bytes.saturating_add(name.len())
        }
        NativeAction::SkillMetaRead { name, .. } | NativeAction::SkillMetaWrite { name, .. } => {
            bytes.saturating_add(name.len())
        }
        _ => bytes,
    };
    if bytes > MAX_QUEUE_BYTES {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Native command exceeds the 16 MiB queue budget",
        ));
    }
    Ok(bytes.max(1024))
}

fn interactive(action: &NativeAction) -> bool {
    // Put every command and its prepare stage in the same FIFO. Approval must
    // never overtake a pause/stop already submitted by the user.
    matches!(
        action,
        NativeAction::Command { .. }
            | NativeAction::RunControl { .. }
            | NativeAction::ConversationManage { .. }
            | NativeAction::TaskDelete { .. }
            | NativeAction::Query {
                query: NativeQuery::Prepare { .. },
                ..
            }
    )
}

const INTERACTION_BURST: usize = 8;
fn next_work(
    rx: &Receiver<Work>,
    priority_rx: &Receiver<Work>,
    interaction_rx: &Receiver<Work>,
    burst: &mut usize,
) -> std::result::Result<Work, mpsc::RecvTimeoutError> {
    if let Ok(work) = priority_rx.try_recv() {
        *burst = 0;
        return Ok(work);
    }
    // Interactive preparation/commit can overtake unrelated background reads,
    // but every bounded burst yields a slot to ordinary work when it exists.
    if *burst >= INTERACTION_BURST {
        if let Ok(work) = rx.try_recv() {
            *burst = 0;
            return Ok(work);
        }
    }
    if let Ok(work) = interaction_rx.try_recv() {
        *burst += 1;
        return Ok(work);
    }
    let work = rx.recv_timeout(Duration::from_millis(20));
    if work.is_ok() {
        *burst = 0;
    }
    work
}
fn action_scope(action: &NativeAction) -> Option<&WorkspaceScope> {
    match action {
        NativeAction::Query { scope, .. }
        | NativeAction::RunControl { scope, .. }
        | NativeAction::ConversationManage { scope, .. }
        | NativeAction::TaskDelete { scope, .. }
        | NativeAction::Command { scope, .. }
        | NativeAction::Cancel { scope, .. }
        | NativeAction::Close { scope }
        | NativeAction::SetSecret { scope, .. }
        | NativeAction::DeleteSecret { scope, .. } => Some(scope),
        NativeAction::FileRead { scope, .. } | NativeAction::FileWrite { scope, .. } => Some(scope),
        NativeAction::StageFile { scope, .. }
        | NativeAction::StageBody { scope, .. }
        | NativeAction::StageBase64Body { scope, .. }
        | NativeAction::ReleaseFile { scope, .. }
        | NativeAction::SaveFile { scope, .. } => Some(scope),
        NativeAction::SkillMetaRead { scope, .. } | NativeAction::SkillMetaWrite { scope, .. } => {
            Some(scope)
        }
        _ => None,
    }
}
impl ServiceHandle {
    pub fn try_submit(&self, action: NativeAction) -> Result<NativeTicket> {
        if matches!(
            action,
            NativeAction::Close { .. } | NativeAction::Shutdown | NativeAction::Suspend
        ) {
            return self.submit_control(action);
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::new("CANCELLED", "Native service has stopped"));
        }
        let weight = weight(&action)?;
        let mut shared = lock(&self.shared);
        if let Some(scope) = action_scope(&action) {
            if shared.phase != ServicePhase::Ready {
                return Err(Error::new("BUSY", "Workspace is opening or retiring"));
            }
            if shared.view.as_ref().map(|v| &v.workspace.scope) != Some(scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
        }
        if matches!(action, NativeAction::Open(_)) && shared.phase != ServicePhase::Closed {
            return Err(Error::new(
                "BUSY",
                "Close the current workspace before opening another",
            ));
        }
        if matches!(action, NativeAction::Shutdown)
            && matches!(
                shared.phase,
                ServicePhase::Opening
                    | ServicePhase::Closing
                    | ServicePhase::Shutdown
                    | ServicePhase::RecoveryRequired
            )
        {
            return Err(Error::new(
                "BUSY",
                "Workspace retirement is already in progress",
            ));
        }
        if self.outstanding.load(Ordering::Acquire) >= 64
            || self
                .queued_bytes
                .load(Ordering::Acquire)
                .saturating_add(weight)
                > MAX_QUEUE_BYTES
        {
            return Err(Error::new(
                "BUSY",
                "Native work queue is full; poll completed work before retrying",
            ));
        }
        let previous = shared.phase.clone();
        match action {
            NativeAction::Open(_) => shared.phase = ServicePhase::Opening,
            NativeAction::Close { .. } | NativeAction::Shutdown => {
                shared.phase = ServicePhase::Closing
            }
            _ => {}
        }
        let ticket = NativeTicket::new();
        if matches!(action, NativeAction::Open(_)) {
            shared.opening_ticket = Some(ticket.clone());
            shared.suspend_requested = false;
        }
        if matches!(
            action,
            NativeAction::Open(_) | NativeAction::Close { .. } | NativeAction::Shutdown
        ) {
            shared.transition = Some(ticket.clone());
        }
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        self.queued_bytes.fetch_add(weight, Ordering::AcqRel);
        shared.active.insert(ticket.clone());
        let work = Work {
            ticket: ticket.clone(),
            action,
            queued_at: Instant::now(),
            weight,
        };
        let queue = if interactive(&work.action) {
            &self.interaction_tx
        } else {
            &self.tx
        };
        if let Err(error) = queue.try_send(work) {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            self.queued_bytes.fetch_sub(weight, Ordering::AcqRel);
            shared.active.remove(&ticket);
            shared.phase = previous;
            if shared.transition.as_ref() == Some(&ticket) {
                shared.transition = None;
            }
            if shared.opening_ticket.as_ref() == Some(&ticket) {
                shared.opening_ticket = None;
            }
            return Err(Error::new(
                if matches!(error, TrySendError::Full(_)) {
                    "BUSY"
                } else {
                    "CANCELLED"
                },
                "Native writer is unavailable",
            ));
        }
        Ok(ticket)
    }
    /// One reserved bounded retirement slot remains available when UI data
    /// credits are exhausted. A power callback never takes the writer lock.
    pub fn suspend(&self) -> Result<NativeTicket> {
        self.submit_control(NativeAction::Suspend)
    }
    fn submit_control(&self, action: NativeAction) -> Result<NativeTicket> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::new("CANCELLED", "Native service has stopped"));
        }
        let mut shared = lock(&self.shared);
        if matches!(
            shared.phase,
            ServicePhase::RecoveryRequired | ServicePhase::Shutdown
        ) {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Native service needs explicit recovery",
            ));
        }
        let suspend = matches!(action, NativeAction::Suspend);
        if suspend {
            shared.suspend_requested = true;
            if let Some(opening) = shared.opening_ticket.clone() {
                shared.cancelled.insert(opening);
                shared.view = None;
            }
            if shared.phase == ServicePhase::Closing {
                return shared
                    .transition
                    .clone()
                    .ok_or_else(|| Error::new("BUSY", "Native retirement is in progress"));
            }
        } else if matches!(shared.phase, ServicePhase::Opening | ServicePhase::Closing) {
            return Err(Error::new(
                "BUSY",
                "Native lifecycle transition is in progress",
            ));
        }
        if let NativeAction::Close { scope } = &action {
            if shared.view.as_ref().map(|v| &v.workspace.scope) != Some(scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
        }
        let previous = shared.phase.clone();
        let previous_transition = shared.transition.clone();
        for job in shared.read_jobs.values() {
            job.cancel.store(true, Ordering::Release);
        }
        let ticket = NativeTicket::new();
        shared.phase = ServicePhase::Closing;
        shared.transition = Some(ticket.clone());
        shared.active.insert(ticket.clone());
        shared.controls.insert(ticket.clone());
        self.outstanding.fetch_add(1, Ordering::AcqRel);
        self.queued_bytes.fetch_add(1024, Ordering::AcqRel);
        let work = Work {
            ticket: ticket.clone(),
            action,
            queued_at: Instant::now(),
            weight: 1024,
        };
        if let Err(error) = self.priority_tx.try_send(work) {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            self.queued_bytes.fetch_sub(1024, Ordering::AcqRel);
            shared.active.remove(&ticket);
            shared.controls.remove(&ticket);
            shared.phase = previous;
            shared.transition = previous_transition;
            return Err(Error::new(
                if matches!(error, TrySendError::Full(_)) {
                    "BUSY"
                } else {
                    "CANCELLED"
                },
                "Native retirement worker is unavailable",
            ));
        }
        Ok(ticket)
    }
    /// Out-of-band bounded cancellation does not need a spare command-queue slot.
    pub fn cancel(&self, ticket: &NativeTicket) -> Result<()> {
        let mut shared = lock(&self.shared);
        if shared.controls.contains(ticket) {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "A retirement fence cannot be cancelled",
            ));
        }
        if !shared.active.contains(ticket) {
            return Err(Error::new(
                "NOT_FOUND",
                "Native ticket is no longer pending",
            ));
        }
        shared.cancelled.insert(ticket.clone());
        if let Some(job) = shared.read_jobs.get(ticket) {
            job.cancel.store(true, Ordering::Release);
        }
        Ok(())
    }
    pub fn drain_completions(&self) -> Vec<NativeCompletion> {
        let mut shared = lock(&self.shared);
        let items = shared.completions.drain(..).collect::<Vec<_>>();
        shared.completion_bytes = 0;
        self.outstanding.fetch_sub(items.len(), Ordering::AcqRel);
        items
    }
    pub fn view(&self) -> Option<NativeReadView> {
        lock(&self.shared).view.clone()
    }
    pub fn snapshot(&self) -> Option<Arc<artex_store::ReadGeneration>> {
        self.view().map(|v| v.generation)
    }
    pub fn scope(&self) -> Option<WorkspaceScope> {
        self.view().map(|v| v.workspace.scope)
    }
    pub fn status(&self) -> ServiceStatus {
        let s = lock(&self.shared);
        ServiceStatus {
            phase: s.phase.clone(),
            scope: s.view.as_ref().map(|v| v.workspace.scope.clone()),
            read_only: s.view.as_ref().is_some_and(|v| v.workspace.read_only),
            last_error: s.error.clone(),
            outstanding: self.outstanding.load(Ordering::Acquire),
        }
    }
    pub fn drain_events(&self) -> NativeEventBatch {
        let mut s = lock(&self.shared);
        s.event_bytes = 0;
        NativeEventBatch {
            events: s.events.drain(..).collect(),
            resync_required: std::mem::take(&mut s.event_overflow),
        }
    }
    fn wake(&self) {
        if let Some(wake) = &self.config.wake_ui {
            wake();
        }
    }
    fn complete(&self, mut completion: NativeCompletion) {
        let weight = match &completion.result {
            Ok(reply) => bounded_value_size(&reply.value).map(|size| {
                reply.read_versions.iter().fold(size, |size, expected| {
                    size.saturating_add(256)
                        .saturating_add(expected.collection.len().saturating_mul(6))
                        .saturating_add(expected.id.len().saturating_mul(6))
                        .saturating_add(expected.version.len().saturating_mul(6))
                })
            }),
            Err(error) => bounded_value_size(&error.details).map(|size| {
                size.saturating_add(error.message.len().saturating_mul(6))
                    .saturating_add(error.code.len().saturating_mul(6))
                    .saturating_add(512)
            }),
        };
        let mut shared = lock(&self.shared);
        if shared.active.remove(&completion.ticket) {
            let control = shared.controls.remove(&completion.ticket);
            let was_opening = shared.opening_ticket.as_ref() == Some(&completion.ticket);
            if was_opening {
                shared.opening_ticket = None;
            }
            if was_opening && shared.suspend_requested && completion.result.is_ok() {
                completion.result = Err(Error::new(
                    "CANCELLED",
                    "Workspace opening was fenced by suspension",
                ));
            }
            shared.cancelled.remove(&completion.ticket);
            shared.read_jobs.remove(&completion.ticket);
            let bytes = match weight {
                Ok(size)
                    if shared.completion_bytes.saturating_add(size)
                        <= MAX_QUEUE_BYTES + if control { 16 * 1024 } else { 0 } =>
                {
                    size
                }
                _ => {
                    let mut error = Error::new(
                        "SIZE_LIMIT",
                        "Native result queue exceeds 16 MiB; use paginated queries or the immutable read view",
                    );
                    error.details = json!({"result_not_delivered":true});
                    completion.result = Err(error);
                    1024
                }
            };
            shared.completion_bytes = shared.completion_bytes.saturating_add(bytes);
            shared.completions.push_back(completion);
        }
        drop(shared);
        self.wake();
    }
}

fn bounded_value_size(value: &Value) -> Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 > MAX_QUEUE_BYTES {
                return Err(std::io::Error::other("Native result budget exceeded"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Counter(0);
    serde_json::to_writer(&mut count, value)
        .map_err(|_| Error::new("SIZE_LIMIT", "Native result budget exceeded"))?;
    Ok(count.0.max(1024))
}

fn error_reply(ticket: NativeTicket, error: Error) -> NativeCompletion {
    NativeCompletion {
        ticket,
        result: Err(error),
    }
}
fn reply(router: &Router, value: Value) -> NativeReply {
    NativeReply {
        value,
        scope: router.native_scope(),
        read_seq: router.snapshot().map(|s| s.commit_seq).unwrap_or(0),
        read_versions: vec![],
    }
}
fn user_context(scope: &WorkspaceScope) -> Context {
    Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: scope.engine_epoch.clone(),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    }
}
struct HostReply {
    ticket: NativeTicket,
    scope: WorkspaceScope,
    result: Result<Value>,
}

fn writer(
    rx: Receiver<Work>,
    priority_rx: Receiver<Work>,
    interaction_rx: Receiver<Work>,
    handle: ServiceHandle,
) {
    let mut router = Router::new(BTreeMap::new());
    router.enable_native();
    let mut workspace: Option<NativeWorkspace> = None;
    let mut gateway: Option<Arc<dyn HostGateway>> = None;
    let mut deferred = BTreeSet::new();
    let mut events_after = 0u64;
    let (host_tx, host_rx) = mpsc::sync_channel::<HostReply>(64);
    let mut retirement: Option<Work> = None;
    let mut interaction_burst = 0usize;
    loop {
        if handle.stopped.load(Ordering::Acquire) {
            let _ = wait_read_jobs(&handle);
            let _ = router.native_shutdown();
            if let Some(g) = gateway.take() {
                let _ = g.retire(Duration::from_secs(5));
            }
            break;
        }
        let cancellations = lock(&handle.shared).cancelled.clone();
        for ticket in &cancellations {
            if deferred.contains(ticket) {
                if let Some(scope) = router.native_scope() {
                    let _ = router.native_cancel(&scope, ticket);
                }
            }
        }
        let work = if let Some(work) = retirement.take() {
            // Advance admitted quarantine/compensation without scheduling new
            // work. The phase fence remains closed throughout the retry.
            if let Err(error) = router.native_poll_retirement() {
                lock(&handle.shared).error = Some(error);
            }
            thread::sleep(Duration::from_millis(20));
            Ok(work)
        } else {
            next_work(&rx, &priority_rx, &interaction_rx, &mut interaction_burst)
        };
        match work {
            Ok(mut work) => {
                handle.queued_bytes.fetch_sub(work.weight, Ordering::AcqRel);
                let cancelled = lock(&handle.shared).cancelled.contains(&work.ticket);
                let control = matches!(
                    &work.action,
                    NativeAction::Close { .. } | NativeAction::Shutdown | NativeAction::Suspend
                );
                let overtaken = lock(&handle.shared).phase == ServicePhase::Closing && !control;
                let outcome = if cancelled {
                    Err(Error::new(
                        "CANCELLED",
                        "Native command cancelled before execution",
                    ))
                } else if overtaken {
                    Err(Error::new(
                        "CANCELLED",
                        "Workspace retirement overtook queued work",
                    ))
                } else if matches!(&work.action,NativeAction::Command {command,..} if work.queued_at.elapsed()>=command.timeout)
                {
                    Err(Error::new("TIMEOUT", "Native command expired in the queue"))
                } else {
                    execute(
                        &mut router,
                        &handle,
                        &mut workspace,
                        &mut gateway,
                        &work.ticket,
                        work.action.clone(),
                        &host_tx,
                    )
                };
                match outcome {
                    Ok(NativeDispatch::Deferred) => {
                        deferred.insert(work.ticket);
                    }
                    Ok(NativeDispatch::Complete(value)) => {
                        publish(&router, &handle, &mut workspace, &mut events_after);
                        handle.complete(NativeCompletion {
                            ticket: work.ticket,
                            result: Ok(value),
                        });
                    }
                    Err(mut error) => {
                        if control
                            && error.code == "BUSY"
                            && work.queued_at.elapsed() < Duration::from_secs(5)
                        {
                            work.weight = 0;
                            retirement = Some(work);
                            continue;
                        }
                        if control {
                            error.recovery_required = true;
                            if error.code == "BUSY" {
                                error = Error::new(
                                    "RECOVERY_REQUIRED",
                                    "Admitted file retirement did not complete before shutdown deadline",
                                );
                            }
                            // A failed store retirement still revokes the old
                            // native epoch. Keep its store locked for recovery,
                            // but never leave admitted processes running.
                            if let Some(g) = gateway.take() {
                                if let Err(retire_error) = g.retire(Duration::from_secs(5)) {
                                    error = retire_error;
                                    error.recovery_required = true;
                                }
                            }
                            router.native_detach_host();
                        }
                        let mut shared = lock(&handle.shared);
                        if shared.transition.as_ref() == Some(&work.ticket)
                            && matches!(shared.phase, ServicePhase::Opening | ServicePhase::Closing)
                        {
                            shared.phase = if control {
                                shared.view = None;
                                ServicePhase::RecoveryRequired
                            } else if router.native_scope().is_some() {
                                ServicePhase::Ready
                            } else {
                                ServicePhase::Closed
                            };
                            shared.transition = None;
                        }
                        shared.error = Some(error.clone());
                        drop(shared);
                        handle.complete(error_reply(work.ticket, error));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if matches!(
            lock(&handle.shared).phase,
            ServicePhase::Ready | ServicePhase::Closed
        ) {
            if let Err(error) = router.maintain() {
                lock(&handle.shared).error = Some(error);
                handle.wake();
            }
        }
        for completion in router.drain_native_completions() {
            deferred.remove(&completion.ticket);
            handle.complete(completion);
        }
        for done in host_rx.try_iter() {
            deferred.remove(&done.ticket);
            let result = if router.native_scope().as_ref() != Some(&done.scope) {
                Err(Error::new(
                    "STALE_EPOCH",
                    "Native vault result belongs to a retired workspace",
                ))
            } else {
                done.result.map(|value| reply(&router, value))
            };
            handle.complete(NativeCompletion {
                ticket: done.ticket,
                result,
            });
        }
        publish(&router, &handle, &mut workspace, &mut events_after);
        if router.shutting_down {
            break;
        }
    }
    for work in priority_rx
        .try_iter()
        .chain(interaction_rx.try_iter())
        .chain(rx.try_iter())
    {
        handle.queued_bytes.fetch_sub(work.weight, Ordering::AcqRel);
        handle.complete(error_reply(
            work.ticket,
            Error::new("CANCELLED", "Application shut down before execution"),
        ));
    }
    for ticket in deferred {
        handle.complete(error_reply(
            ticket,
            Error::new("CANCELLED", "Application has shut down"),
        ));
    }
    let mut shared = lock(&handle.shared);
    shared.view = None;
    if shared.phase != ServicePhase::RecoveryRequired {
        shared.phase = ServicePhase::Shutdown;
    }
    drop(shared);
    handle.stopped.store(true, Ordering::Release);
    handle.wake();
}

fn execute(
    router: &mut Router,
    handle: &ServiceHandle,
    workspace: &mut Option<NativeWorkspace>,
    gateway: &mut Option<Arc<dyn HostGateway>>,
    ticket: &NativeTicket,
    action: NativeAction,
    host_tx: &SyncSender<HostReply>,
) -> Result<NativeDispatch> {
    let decode_base64 = matches!(&action, NativeAction::StageBase64Body { .. });
    if router.native_lifecycle_busy()
        && matches!(
            &action,
            NativeAction::FileWrite { .. }
                | NativeAction::SkillMetaWrite { .. }
                | NativeAction::SetSecret { .. }
                | NativeAction::DeleteSecret { .. }
                | NativeAction::SaveFile { .. }
        )
    {
        return Err(Error::new(
            "BUSY",
            "Execution cleanup is in progress; workspace writes are fenced",
        ));
    }
    match action {
        NativeAction::RunControl {
            scope,
            command_id,
            expected,
            target,
            action,
        } => router.native_run_control(ticket, &scope, command_id, expected, target, action),
        NativeAction::ConversationManage {
            scope,
            command_id,
            expected,
            ids,
            action,
        } => router.native_conversation_manage(ticket, &scope, command_id, expected, ids, action),
        NativeAction::TaskDelete {
            scope,
            command_id,
            expected,
            ids,
            options,
        } => router.native_task_delete(ticket, &scope, command_id, expected, ids, options),
        NativeAction::Open(options) => {
            let (opened, result) = match router.native_open(&options) {
                Ok(opened) => opened,
                Err(error) => {
                    abort_open(router, handle)?;
                    return Err(error);
                }
            };
            if lock(&handle.shared).suspend_requested {
                abort_open(router, handle)?;
                return Err(Error::new(
                    "CANCELLED",
                    "Workspace opening was fenced by suspension",
                ));
            }
            if let Some(factory) = &handle.config.host_factory {
                match factory(&opened) {
                    Ok(g) => {
                        router.native_attach_host(g.clone());
                        *gateway = Some(g);
                    }
                    Err(error) => {
                        abort_open(router, handle)?;
                        return Err(error);
                    }
                }
            }
            let view = NativeReadView {
                workspace: opened.clone(),
                generation: router.snapshot()?,
            };
            let mut shared = lock(&handle.shared);
            if shared.suspend_requested
                || shared.cancelled.contains(ticket)
                || shared.phase != ServicePhase::Opening
            {
                drop(shared);
                abort_open(router, handle)?;
                if let Some(g) = gateway.take() {
                    g.retire(Duration::from_secs(5))?;
                }
                router.native_detach_host();
                return Err(Error::new(
                    "CANCELLED",
                    "Workspace opening was retired before publication",
                ));
            }
            *workspace = Some(opened);
            shared.phase = ServicePhase::Ready;
            shared.view = Some(view);
            shared.transition = None;
            shared.error = None;
            drop(shared);
            Ok(NativeDispatch::Complete(result))
        }
        NativeAction::Query { scope, query } => router.native_query(ticket, &scope, query),
        NativeAction::Command { scope, command } => router.native_command(ticket, &scope, &command),
        NativeAction::Cancel {
            scope,
            ticket: target,
        } => Ok(NativeDispatch::Complete(
            router.native_cancel(&scope, &target)?,
        )),
        NativeAction::Close { scope } => {
            wait_read_jobs(handle)?;
            let value = router.native_close(&scope)?;
            if let Some(g) = gateway.take() {
                if let Err(error) = g.retire(Duration::from_secs(5)) {
                    let mut shared = lock(&handle.shared);
                    shared.phase = ServicePhase::RecoveryRequired;
                    shared.view = None;
                    shared.error = Some(error.clone());
                    drop(shared);
                    router.native_detach_host();
                    *workspace = None;
                    return Err(error);
                }
            }
            router.native_detach_host();
            *workspace = None;
            let mut shared = lock(&handle.shared);
            shared.phase = ServicePhase::Closed;
            shared.view = None;
            shared.transition = None;
            shared.events.clear();
            drop(shared);
            Ok(NativeDispatch::Complete(value))
        }
        NativeAction::Shutdown => {
            wait_read_jobs(handle)?;
            let value = router.native_shutdown()?;
            if let Some(g) = gateway.take() {
                if let Err(error) = g.retire(Duration::from_secs(5)) {
                    let mut shared = lock(&handle.shared);
                    shared.phase = ServicePhase::RecoveryRequired;
                    shared.view = None;
                    shared.error = Some(error.clone());
                    drop(shared);
                    router.native_detach_host();
                    *workspace = None;
                    return Err(error);
                }
            }
            router.native_detach_host();
            *workspace = None;
            let mut shared = lock(&handle.shared);
            shared.phase = ServicePhase::Shutdown;
            shared.view = None;
            shared.transition = None;
            shared.events.clear();
            drop(shared);
            Ok(NativeDispatch::Complete(value))
        }
        NativeAction::Suspend => {
            wait_read_jobs(handle)?;
            let value = if let Some(scope) = router.native_scope() {
                router.native_close(&scope)?
            } else {
                reply(router, json!({"closed":true}))
            };
            if let Some(g) = gateway.take() {
                if let Err(error) = g.retire(Duration::from_secs(5)) {
                    let mut shared = lock(&handle.shared);
                    shared.phase = ServicePhase::RecoveryRequired;
                    shared.view = None;
                    shared.error = Some(error.clone());
                    drop(shared);
                    router.native_detach_host();
                    *workspace = None;
                    return Err(error);
                }
            }
            router.native_detach_host();
            *workspace = None;
            let mut shared = lock(&handle.shared);
            shared.phase = ServicePhase::Closed;
            shared.view = None;
            shared.transition = None;
            shared.events.clear();
            shared.event_bytes = 0;
            drop(shared);
            Ok(NativeDispatch::Complete(value))
        }
        NativeAction::SetSecret {
            scope,
            command_id,
            field,
            value,
        } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            if !artex_core::valid_id(&command_id, "command") {
                return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
            }
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native secret vault unavailable"))?
                .clone();
            if router.native_read_only() {
                return Err(Error::new("RECOVERY_REQUIRED", "Workspace is read-only"));
            }
            let tx = host_tx.clone();
            let ticket = ticket.clone();
            thread::Builder::new()
                .name("artex-native-secret".into())
                .spawn(move || {
                    let result = g.request(
                        &scope.workspace_id,
                        &user_context(&scope),
                        "native.secret.set",
                        json!({"command_id":command_id,"field":field,"value":value}),
                        Duration::from_secs(10),
                    );
                    let _ = tx.send(HostReply {
                        ticket,
                        scope,
                        result,
                    });
                })
                .map_err(Error::io)?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::DeleteSecret { scope, reference } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native secret vault unavailable"))?
                .clone();
            if router.native_read_only() {
                return Err(Error::new("RECOVERY_REQUIRED", "Workspace is read-only"));
            }
            let tx = host_tx.clone();
            let ticket = ticket.clone();
            thread::Builder::new()
                .name("artex-native-secret".into())
                .spawn(move || {
                    let result = g.request(
                        &scope.workspace_id,
                        &user_context(&scope),
                        "native.secret.delete",
                        json!({"reference":reference}),
                        Duration::from_secs(10),
                    );
                    let _ = tx.send(HostReply {
                        ticket,
                        scope,
                        result,
                    });
                })
                .map_err(Error::io)?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::FileRead { scope, area, path } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let current = workspace
                .as_ref()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?;
            let cap = match area {
                NativeFileArea::Work => &current.execution_capability,
                NativeFileArea::Resources => &current.resource_capability,
            };
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            let params = json!({"directory_capability":cap,"path":path});
            spawn_host(
                ticket.clone(),
                scope,
                "native.file.read",
                params,
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::FileWrite {
            scope,
            area,
            path,
            content,
            expected_sha256,
            create,
            command_id,
        } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            if router.native_read_only() {
                return Err(Error::new("RECOVERY_REQUIRED", "Workspace is read-only"));
            }
            if !artex_core::valid_id(&command_id, "command") {
                return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
            }
            let current = workspace
                .as_ref()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?;
            let cap = match area {
                NativeFileArea::Work => &current.execution_capability,
                NativeFileArea::Resources => &current.resource_capability,
            };
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            let params = json!({"directory_capability":cap,"path":path,"content":content,"expected_sha256":expected_sha256,"create":create,"command_id":command_id});
            spawn_host(
                ticket.clone(),
                scope,
                "native.file.write",
                params,
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::StageFile { scope, path } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let path = path.to_str().ok_or_else(|| {
                Error::new("INVALID_PARAMS", "Selected file path must be valid UTF-8")
            })?;
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            spawn_host(
                ticket.clone(),
                scope,
                "native.file.stage",
                json!({"path":path}),
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::StageBody { scope, body, name }
        | NativeAction::StageBase64Body { scope, body, name } => {
            if name.is_empty()
                || name.len() > 1024
                || name.contains(['/', '\\', '\0'])
                || matches!(name.as_str(), "." | "..")
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Bounded export basename required",
                ));
            }
            let source = router.native_stage_source(&scope, body)?;
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            let job = Arc::new(ReadJob::default());
            {
                let mut shared = lock(&handle.shared);
                if shared.phase != ServicePhase::Ready || shared.cancelled.contains(ticket) {
                    return Err(Error::new(
                        "CANCELLED",
                        "Export was retired before preparation",
                    ));
                }
                // One export reader bounds verified/decoded intermediate disk
                // space before the host's independent 512 MiB staging budget.
                // Completed readers have already dropped both temporary files
                // and the pinned store lock, even if UI polling is delayed.
                if shared
                    .read_jobs
                    .values()
                    .any(|job| !job.finished.load(Ordering::Acquire))
                {
                    return Err(Error::new(
                        "BUSY",
                        "Another export reader is preparing a file",
                    ));
                }
                shared.read_jobs.insert(ticket.clone(), job.clone());
            }
            if let Err(error) = spawn_stage_body(
                ticket.clone(),
                scope,
                source,
                name,
                decode_base64,
                job,
                g,
                host_tx.clone(),
            ) {
                lock(&handle.shared).read_jobs.remove(ticket);
                return Err(error);
            }
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::ReleaseFile { scope, token } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            spawn_host(
                ticket.clone(),
                scope,
                "native.file.release",
                json!({"file_capability":token}),
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::SaveFile {
            scope,
            token,
            destination,
            command_id,
        } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let destination = destination.to_str().ok_or_else(|| {
                Error::new(
                    "INVALID_PARAMS",
                    "Selected destination path must be valid UTF-8",
                )
            })?;
            if !artex_core::valid_id(&command_id, "command") {
                return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
            }
            let current = workspace
                .as_ref()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?;
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native files unavailable"))?
                .clone();
            let params = json!({"file_capability":token,"destination":destination,"command_id":command_id,"directory_capability":current.execution_capability});
            spawn_host(
                ticket.clone(),
                scope,
                "native.file.save",
                params,
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::SkillMetaRead { scope, name } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            let current = workspace
                .as_ref()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?;
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native skills unavailable"))?
                .clone();
            spawn_host(
                ticket.clone(),
                scope,
                "native.skill.meta.read",
                json!({"directory_capability":current.resource_capability,"name":name}),
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
        NativeAction::SkillMetaWrite {
            scope,
            name,
            meta,
            expected_sha256,
            command_id,
        } => {
            if router.native_scope().as_ref() != Some(&scope) {
                return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
            }
            if router.native_read_only() {
                return Err(Error::new("RECOVERY_REQUIRED", "Workspace is read-only"));
            }
            if !artex_core::valid_id(&command_id, "command") {
                return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
            }
            let current = workspace
                .as_ref()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?;
            let g = gateway
                .as_ref()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native skills unavailable"))?
                .clone();
            spawn_host(
                ticket.clone(),
                scope,
                "native.skill.meta.write",
                json!({"directory_capability":current.resource_capability,"name":name,"meta":meta,"expected_sha256":expected_sha256,"command_id":command_id}),
                g,
                host_tx.clone(),
            )?;
            Ok(NativeDispatch::Deferred)
        }
    }
}
fn spawn_host(
    ticket: NativeTicket,
    scope: WorkspaceScope,
    operation: &'static str,
    params: Value,
    gateway: Arc<dyn HostGateway>,
    tx: SyncSender<HostReply>,
) -> Result<()> {
    thread::Builder::new()
        .name("artex-native-user-operation".into())
        .spawn(move || {
            let result = gateway.request(
                &scope.workspace_id,
                &user_context(&scope),
                operation,
                params,
                Duration::from_secs(30),
            );
            let _ = tx.send(HostReply {
                ticket,
                scope,
                result,
            });
        })
        .map_err(Error::io)?;
    Ok(())
}

fn wait_read_jobs(handle: &ServiceHandle) -> Result<()> {
    let jobs = lock(&handle.shared)
        .read_jobs
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for job in &jobs {
        job.cancel.store(true, Ordering::Release);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while jobs.iter().any(|job| !job.finished.load(Ordering::Acquire)) {
        if Instant::now() >= deadline {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "Export reader has not released its workspace generation",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn spawn_stage_body(
    ticket: NativeTicket,
    scope: WorkspaceScope,
    source: NativeStageSource,
    name: String,
    decode_base64: bool,
    job: Arc<ReadJob>,
    gateway: Arc<dyn HostGateway>,
    tx: SyncSender<HostReply>,
) -> Result<()> {
    thread::Builder::new()
        .name("artex-native-export-reader".into())
        .spawn(move || {
            struct CancelWriter<'a> {
                file: &'a mut std::fs::File,
                cancel: &'a AtomicBool,
            }
            impl std::io::Write for CancelWriter<'_> {
                fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                    if self.cancel.load(Ordering::Acquire) {
                        // write_all retries Interrupted indefinitely; cancellation
                        // must be a terminal writer error instead.
                        return Err(std::io::Error::other("Export cancelled"));
                    }
                    std::io::Write::write(self.file, bytes)
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    std::io::Write::flush(self.file)
                }
            }
            let result = (|| -> Result<Value> {
                if job.cancel.load(Ordering::Acquire) {
                    return Err(Error::new("CANCELLED", "Export cancelled before copying"));
                }
                let mut temporary = tempfile::NamedTempFile::new().map_err(Error::io)?;
                source.copy_to(&mut CancelWriter {
                    file: temporary.as_file_mut(),
                    cancel: &job.cancel,
                })?;
                temporary.as_file().sync_all().map_err(Error::io)?;
                // Verify the complete stored encoded input before interpreting
                // it. Both passes stream bounded chunks; nothing is published
                // if decoding, cancellation or input integrity fails.
                let decoded = if decode_base64 {
                    let mut decoded = tempfile::NamedTempFile::new().map_err(Error::io)?;
                    let input = std::fs::File::open(temporary.path()).map_err(Error::io)?;
                    let mut reader = base64::read::DecoderReader::new(
                        input,
                        &base64::engine::general_purpose::STANDARD,
                    );
                    decode_export(
                        &mut reader,
                        &mut CancelWriter {
                            file: decoded.as_file_mut(),
                            cancel: &job.cancel,
                        },
                        &job.cancel,
                    )?;
                    decoded.as_file().sync_all().map_err(Error::io)?;
                    Some(decoded)
                } else {
                    None
                };
                if job.cancel.load(Ordering::Acquire) {
                    return Err(Error::new(
                        "CANCELLED",
                        "Export cancelled after verification",
                    ));
                }
                let value = gateway.request(
                    &scope.workspace_id,
                    &user_context(&scope),
                    "native.file.stage",
                    json!({"path":decoded.as_ref().unwrap_or(&temporary).path(), "name":name}),
                    Duration::from_secs(30),
                )?;
                if job.cancel.load(Ordering::Acquire) {
                    // A prepared read-only export has no user-visible file effect.
                    // Release its immutable temporary capability before cancellation.
                    let _ = gateway.request(
                        &scope.workspace_id,
                        &user_context(&scope),
                        "native.file.release",
                        json!({"file_capability":value["file_capability"]}),
                        Duration::from_secs(5),
                    );
                    return Err(Error::new("CANCELLED", "Export cancelled before delivery"));
                }
                Ok(value)
            })();
            let result = if job.cancel.load(Ordering::Acquire) && result.is_err() {
                Err(Error::new("CANCELLED", "Export cancelled"))
            } else {
                result
            };
            drop(source); // release native writer lock and generation before retirement ack
            job.finished.store(true, Ordering::Release);
            let _ = tx.send(HostReply {
                ticket,
                scope,
                result,
            });
        })
        .map_err(Error::io)?;
    Ok(())
}

fn decode_export(
    reader: &mut impl std::io::Read,
    writer: &mut impl std::io::Write,
    cancel: &AtomicBool,
) -> Result<u64> {
    let mut buffer = [0u8; 65536];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::new("CANCELLED", "Export decoding cancelled"));
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|_| Error::new("PARSE_ERROR", "Export content is not valid Base64"))?;
        if count == 0 {
            return Ok(total);
        }
        total = total
            .checked_add(count as u64)
            .filter(|total| *total <= 512 * 1024 * 1024)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Decoded export exceeds 512 MiB"))?;
        writer.write_all(&buffer[..count])?;
    }
}

fn abort_open(router: &mut Router, handle: &ServiceHandle) -> Result<()> {
    if let Some(scope) = router.native_scope() {
        if let Err(mut error) = router.native_close(&scope) {
            error.recovery_required = true;
            let mut shared = lock(&handle.shared);
            shared.phase = ServicePhase::RecoveryRequired;
            shared.error = Some(error.clone());
            shared.view = None;
            return Err(error);
        }
    }
    Ok(())
}

fn publish(
    router: &Router,
    handle: &ServiceHandle,
    workspace: &mut Option<NativeWorkspace>,
    after: &mut u64,
) {
    let retiring = {
        let shared = lock(&handle.shared);
        matches!(
            shared.phase,
            ServicePhase::Closing | ServicePhase::RecoveryRequired
        )
    };
    if retiring {
        return;
    }
    let Some(current) = workspace.as_mut() else {
        lock(&handle.shared).view = None;
        *after = 0;
        return;
    };
    let Ok(generation) = router.snapshot() else {
        lock(&handle.shared).view = None;
        return;
    };
    current.read_only = router.native_read_only();
    // Event scans and JSON size accounting may be expensive. Never hold the
    // shared UI mutex while doing either; only publication below is atomic.
    let changed_scope = lock(&handle.shared)
        .view
        .as_ref()
        .is_none_or(|old| old.workspace.scope != current.scope);
    if changed_scope {
        *after = generation.commit_seq;
    }
    let mut batch = Vec::new();
    let mut overflow = false;
    if generation.commit_seq > *after {
        let mut events = generation
            .records("domain_events")
            .into_iter()
            .filter(|r| {
                r.header
                    .commit_seq
                    .parse::<u64>()
                    .is_ok_and(|seq| seq > *after)
            })
            .take(513)
            .collect::<Vec<_>>();
        events.sort_by(|a, b| {
            a.header
                .commit_seq
                .parse::<u64>()
                .unwrap_or(0)
                .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0))
                .then(a.header.id.cmp(&b.header.id))
        });
        if events.len() > 512 {
            overflow = true;
        } else {
            let mut bytes = 0usize;
            for record in events {
                let Ok(weight) = bounded_value_size(&record.data) else {
                    overflow = true;
                    break;
                };
                bytes = bytes.saturating_add(weight).saturating_add(512);
                if bytes > MAX_QUEUE_BYTES {
                    overflow = true;
                    break;
                }
                batch.push((
                    NativeEvent {
                        scope: current.scope.clone(),
                        record: record.clone(),
                    },
                    weight + 512,
                ));
            }
        }
        *after = generation.commit_seq;
    }
    let mut shared = lock(&handle.shared);
    let changed = shared.view.as_ref().is_none_or(|old| {
        old.workspace.scope != current.scope || old.generation.commit_seq != generation.commit_seq
    });
    if changed_scope {
        shared.events.clear();
        shared.event_bytes = 0;
        shared.event_overflow = false;
    }
    for (event, bytes) in batch {
        if shared.events.len() >= 512 || shared.event_bytes.saturating_add(bytes) > MAX_QUEUE_BYTES
        {
            overflow = true;
            break;
        }
        shared.event_bytes += bytes;
        shared.events.push_back(event);
    }
    if overflow {
        shared.events.clear();
        shared.event_bytes = 0;
        shared.event_overflow = true;
    }
    shared.view = Some(NativeReadView {
        workspace: current.clone(),
        generation,
    });
    drop(shared);
    if changed {
        handle.wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Durable creation writes all collection snapshots and their directory
    // entries before publishing Ready. Shared CI disks may spend more than the
    // ordinary completion budget in those fsyncs. This is a fixture setup bound,
    // not an application responsiveness or cancellation allowance.
    const CREATE_SETUP_TIMEOUT: Duration = Duration::from_secs(60);
    const COMPLETION_TIMEOUT: Duration = Duration::from_secs(15);
    #[test]
    fn interaction_lane_precedes_background_reads_but_yields_and_never_overtakes_retirement() {
        let scope = WorkspaceScope {
            workspace_id: "workspace-fixture".into(),
            engine_epoch: "epoch-fixture".into(),
            open_generation: "open-fixture".into(),
        };
        let work = |action| Work {
            ticket: NativeTicket::new(),
            action,
            queued_at: Instant::now(),
            weight: 1024,
        };
        let (normal_tx, normal) = mpsc::sync_channel(32);
        let (control_tx, control) = mpsc::sync_channel(1);
        let (interaction_tx, interaction) = mpsc::sync_channel(16);
        let read = work(NativeAction::Query {
            scope: scope.clone(),
            query: NativeQuery::WorkspaceInfo,
        });
        let read_ticket = read.ticket.clone();
        normal_tx.send(read).unwrap();
        for index in 0..10 {
            let action = NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.interceptDecide".into(),
                    params: json!({"id":format!("approval_{index}"),"decision":"allow"}),
                },
            };
            assert!(interactive(&action));
            interaction_tx.send(work(action)).unwrap();
        }
        let retirement = work(NativeAction::Close {
            scope: scope.clone(),
        });
        let retirement_ticket = retirement.ticket.clone();
        control_tx.send(retirement).unwrap();
        let mut burst = 0;
        assert_eq!(
            next_work(&normal, &control, &interaction, &mut burst)
                .unwrap()
                .ticket,
            retirement_ticket
        );
        for _ in 0..INTERACTION_BURST {
            assert!(interactive(
                &next_work(&normal, &control, &interaction, &mut burst)
                    .unwrap()
                    .action
            ));
        }
        assert_eq!(
            next_work(&normal, &control, &interaction, &mut burst)
                .unwrap()
                .ticket,
            read_ticket,
            "ordinary reads must not starve behind repeated approval actions"
        );
        for _ in 0..2 {
            assert!(interactive(
                &next_work(&normal, &control, &interaction, &mut burst)
                    .unwrap()
                    .action
            ));
        }
        assert!(interactive(&NativeAction::Command {
            scope: scope.clone(),
            command: NativeCommand::new("user.answer", json!({}), vec![])
        }));
        assert!(interactive(&NativeAction::Command {
            scope: scope.clone(),
            command: NativeCommand::new(
                "ui.setSettings",
                json!({"patch":{"approval_bypass":true}}),
                vec![]
            )
        }));
        assert!(interactive(&NativeAction::Command {
            scope: scope.clone(),
            command: NativeCommand::new(
                "ui.setSettings",
                json!({"patch":{"approval_bypass":true,"other":true}}),
                vec![]
            )
        }));
        let stop = work(NativeAction::Command {
            scope: scope.clone(),
            command: NativeCommand::new("ui.controlTask", json!({"act":"pause"}), vec![]),
        });
        let stop_ticket = stop.ticket.clone();
        interaction_tx.send(stop).unwrap();
        let answer = work(NativeAction::Command {
            scope: scope.clone(),
            command: NativeCommand::new("user.answer", json!({}), vec![]),
        });
        let answer_ticket = answer.ticket.clone();
        interaction_tx.send(answer).unwrap();
        assert_eq!(
            next_work(&normal, &control, &interaction, &mut burst)
                .unwrap()
                .ticket,
            stop_ticket
        );
        assert_eq!(
            next_work(&normal, &control, &interaction, &mut burst)
                .unwrap()
                .ticket,
            answer_ticket
        );
    }
    #[test]
    fn real_service_bypass_prepare_commit_is_durable_and_readonly_still_rejects_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace");
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = service.handle();
        let scope = open(
            &handle,
            &path,
            OpenMode::Create {
                name: "Bypass workspace fixture".into(),
            },
            false,
        );
        let params = json!({"patch":{"approval_bypass":true}});
        let prepare = handle
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.setSettings".into(),
                    params: params.clone(),
                },
            })
            .unwrap();
        let prepared = wait(&handle, &prepare).unwrap();
        let expected: Vec<artex_core::Expected> =
            serde_json::from_value(prepared.value["expected"].clone()).unwrap();
        let command = NativeCommand::new("ui.setSettings", params, expected);
        let saved = handle
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: command.clone(),
            })
            .unwrap();
        wait(&handle, &saved).unwrap();
        let settings = handle
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.settings".into(),
                    params: json!({}),
                },
            })
            .unwrap();
        assert_eq!(
            wait(&handle, &settings).unwrap().value["approval_bypass"],
            true
        );
        let replay = handle
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command,
            })
            .unwrap();
        wait(&handle, &replay).unwrap();
        assert_eq!(
            handle
                .snapshot()
                .unwrap()
                .records("settings")
                .into_iter()
                .filter(|r| r.data["key"] == "approval_bypass")
                .count(),
            1
        );
        shutdown(service);

        let readonly = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = readonly.handle();
        let scope = open(&handle, &path, OpenMode::Existing, true);
        let settings = handle
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.settings".into(),
                    params: json!({}),
                },
            })
            .unwrap();
        assert_eq!(
            wait(&handle, &settings).unwrap().value["approval_bypass"],
            true
        );
        let rejected = handle
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: NativeCommand::new(
                    "ui.setSettings",
                    json!({"patch":{"approval_bypass":false}}),
                    vec![],
                ),
            })
            .unwrap();
        // The native command boundary classifies a write on this reopened
        // read-only generation as requiring recovery; it never applies it.
        assert_eq!(
            wait(&handle, &rejected).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        let settings = handle
            .try_submit(NativeAction::Query {
                scope,
                query: NativeQuery::Feature {
                    method: "ui.settings".into(),
                    params: json!({}),
                },
            })
            .unwrap();
        assert_eq!(
            wait(&handle, &settings).unwrap().value["approval_bypass"],
            true,
            "rejected read-only toggle must preserve the stored bypass setting"
        );
        shutdown(readonly);
    }
    #[track_caller]
    fn wait(handle: &ServiceHandle, ticket: &NativeTicket) -> Result<NativeReply> {
        wait_for(handle, ticket, COMPLETION_TIMEOUT, "ordinary completion")
    }
    #[track_caller]
    fn wait_for(
        handle: &ServiceHandle,
        ticket: &NativeTicket,
        timeout: Duration,
        stage: &str,
    ) -> Result<NativeReply> {
        let started = Instant::now();
        let until = started + timeout;
        loop {
            for completion in handle.drain_completions() {
                if &completion.ticket == ticket {
                    if started.elapsed() >= Duration::from_secs(1) {
                        eprintln!(
                            "Fixture {stage} completed in {} ms at {}",
                            started.elapsed().as_millis(),
                            std::panic::Location::caller()
                        );
                    }
                    return completion.result;
                }
            }
            if Instant::now() >= until {
                let status = handle.status();
                panic!(
                    "Native kernel {stage} deadline after {} ms: phase={:?}, outstanding={}, read_only={}, error_code={:?}",
                    started.elapsed().as_millis(),
                    status.phase,
                    status.outstanding,
                    status.read_only,
                    status.last_error.as_ref().map(|error| error.code.as_str())
                );
            }
            thread::sleep(Duration::from_millis(3));
        }
    }
    #[track_caller]
    fn open(
        handle: &ServiceHandle,
        path: &std::path::Path,
        mode: OpenMode,
        read_only: bool,
    ) -> WorkspaceScope {
        let creating = matches!(&mode, OpenMode::Create { .. });
        let ticket = handle
            .try_submit(NativeAction::Open(OpenOptions {
                path: path.to_owned(),
                mode,
                read_only,
            }))
            .unwrap();
        wait_for(
            handle,
            &ticket,
            if creating {
                CREATE_SETUP_TIMEOUT
            } else {
                COMPLETION_TIMEOUT
            },
            if creating {
                "durable create setup"
            } else {
                "reopen"
            },
        )
        .unwrap()
        .scope
        .unwrap()
    }
    fn shutdown(service: Service) {
        let handle = service.handle();
        let ticket = handle.try_submit(NativeAction::Shutdown).unwrap();
        wait(&handle, &ticket).unwrap();
        service.join().unwrap();
    }
    #[test]
    fn real_store_native_commands_preserve_receipts_cas_pins_readonly_and_reopen_epochs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace");
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &path,
            OpenMode::Create {
                name: "Native store".into(),
            },
            false,
        );
        let command = NativeCommand::new(
            "ui.createCompany",
            json!({"name":"Native company","scope":[]}),
            vec![],
        );
        let first = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: command.clone(),
            })
            .unwrap();
        let first = wait(&h, &first).unwrap();
        let id = first.value["id"].as_str().unwrap().to_owned();
        let immutable = h.snapshot().unwrap();
        assert!(immutable.get("companies", &id).is_some());
        let replay = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command,
            })
            .unwrap();
        assert_eq!(wait(&h, &replay).unwrap().value, first.value);
        assert_eq!(h.snapshot().unwrap().records("companies").len(), 1);
        let missing = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: NativeCommand::new("ui.deleteCompany", json!({"id":id}), vec![]),
            })
            .unwrap();
        assert_eq!(wait(&h, &missing).unwrap_err().code, "VERSION_CONFLICT");
        let prepared = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.deleteCompany".into(),
                    params: json!({"id":id}),
                },
            })
            .unwrap();
        let expected =
            serde_json::from_value(wait(&h, &prepared).unwrap().value["expected"].clone()).unwrap();
        let delete = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: NativeCommand::new("ui.deleteCompany", json!({"id":id}), expected),
            })
            .unwrap();
        wait(&h, &delete).unwrap();
        assert!(h.snapshot().unwrap().get("companies", &id).is_none());
        assert!(
            immutable.get("companies", &id).is_some(),
            "Published immutable generation stays pinned"
        );
        let close = h
            .try_submit(NativeAction::Close {
                scope: scope.clone(),
            })
            .unwrap();
        wait(&h, &close).unwrap();
        let readonly = open(&h, &path, OpenMode::Existing, true);
        assert_ne!(scope.engine_epoch, readonly.engine_epoch);
        assert_ne!(scope.open_generation, readonly.open_generation);
        assert_eq!(
            h.try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::WorkspaceInfo
            })
            .unwrap_err()
            .code,
            "STALE_EPOCH"
        );
        let rejected = h
            .try_submit(NativeAction::Command {
                scope: readonly.clone(),
                command: NativeCommand::new(
                    "ui.createCompany",
                    json!({"name":"Must not write","scope":[]}),
                    vec![],
                ),
            })
            .unwrap();
        assert_eq!(wait(&h, &rejected).unwrap_err().code, "RECOVERY_REQUIRED");
        let close = h
            .try_submit(NativeAction::Close { scope: readonly })
            .unwrap();
        wait(&h, &close).unwrap();
        let reopened = open(&h, &path, OpenMode::Existing, false);
        let info = h
            .try_submit(NativeAction::Query {
                scope: reopened,
                query: NativeQuery::WorkspaceInfo,
            })
            .unwrap();
        assert_eq!(wait(&h, &info).unwrap().value["read_only"], false);
        shutdown(service);
    }
    #[test]
    fn completed_unpolled_work_holds_queue_credits_and_cannot_grow_without_bound() {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &dir.path().join("w"),
            OpenMode::Create {
                name: "Bounded".into(),
            },
            false,
        );
        for _ in 0..64 {
            h.try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::WorkspaceInfo,
            })
            .unwrap();
        }
        assert_eq!(
            h.try_submit(NativeAction::Query {
                scope,
                query: NativeQuery::WorkspaceInfo
            })
            .unwrap_err()
            .code,
            "BUSY"
        );
        let until = Instant::now() + Duration::from_secs(15);
        let mut count = 0;
        while count < 64 {
            count += h.drain_completions().len();
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(3));
        }
        assert_eq!(h.status().outstanding, 0);
        shutdown(service);
    }
    #[test]
    fn suspension_retirement_slot_survives_64_unpolled_data_credits() {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &dir.path().join("w"),
            OpenMode::Create {
                name: "Retirement reserve".into(),
            },
            false,
        );
        for _ in 0..64 {
            h.try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::WorkspaceInfo,
            })
            .unwrap();
        }
        let suspend = h.suspend().unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        let mut total = 0;
        let mut retired = false;
        while total < 65 {
            for completion in h.drain_completions() {
                total += 1;
                if completion.ticket == suspend {
                    completion.result.unwrap();
                    retired = true;
                }
            }
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(3));
        }
        assert!(retired);
        assert_eq!(h.status().phase, ServicePhase::Closed);
        assert!(h.view().is_none());
        shutdown(service);
    }
    #[test]
    fn suspension_fences_inflight_open_before_ready_publication_and_requires_explicit_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let factory_returned = Arc::new(AtomicBool::new(false));
        let f_entered = entered.clone();
        let f_release = release.clone();
        let f_returned = factory_returned.clone();
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |_| {
                f_entered.store(true, Ordering::Release);
                let until = Instant::now() + Duration::from_secs(10);
                while !f_release.load(Ordering::Acquire) && Instant::now() < until {
                    thread::sleep(Duration::from_millis(3));
                }
                f_returned.store(true, Ordering::Release);
                Ok(Arc::new(BlockingRead {
                    started: AtomicBool::new(false),
                    release: AtomicBool::new(true),
                }) as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let opening = h
            .try_submit(NativeAction::Open(OpenOptions {
                path: path.clone(),
                mode: OpenMode::Create {
                    name: "Fence fixture".into(),
                },
                read_only: false,
            }))
            .unwrap();
        let setup_started = Instant::now();
        let until = setup_started + CREATE_SETUP_TIMEOUT;
        while !entered.load(Ordering::Acquire) {
            assert!(
                Instant::now() < until,
                "Durable create did not reach the blocked host factory: elapsed_ms={}, phase={:?}",
                setup_started.elapsed().as_millis(),
                h.status().phase
            );
            thread::sleep(Duration::from_millis(3));
        }
        eprintln!(
            "Suspension fixture durable create reached host factory in {} ms",
            setup_started.elapsed().as_millis()
        );
        assert!(
            !factory_returned.load(Ordering::Acquire),
            "The host factory must still be blocked when exercising the opening race"
        );
        let suspend = h.suspend().unwrap();
        assert_eq!(h.status().phase, ServicePhase::Closing);
        assert!(h.view().is_none());
        assert_eq!(
            h.suspend().unwrap(),
            suspend,
            "One retirement ticket is reused"
        );
        assert_eq!(h.cancel(&suspend).unwrap_err().code, "PERMISSION_DENIED");
        assert_eq!(
            h.try_submit(NativeAction::Open(OpenOptions {
                path: path.clone(),
                mode: OpenMode::Existing,
                read_only: false
            }))
            .unwrap_err()
            .code,
            "BUSY"
        );
        // Admission was fenced while the host factory is still deliberately
        // blocked. Measure retirement only after releasing it, without charging
        // initial snapshot fsync time to the cancellation response budget.
        assert!(
            !factory_returned.load(Ordering::Acquire),
            "Suspension admission must return before the blocked host factory finishes"
        );
        release.store(true, Ordering::Release);
        let retirement_started = Instant::now();
        let until = retirement_started + COMPLETION_TIMEOUT;
        let mut results = BTreeMap::new();
        while results.len() < 2 {
            for completion in h.drain_completions() {
                results.insert(completion.ticket, completion.result);
            }
            assert!(
                Instant::now() < until,
                "Suspension completion deadline after host factory release: elapsed_ms={}, phase={:?}, completions={}",
                retirement_started.elapsed().as_millis(),
                h.status().phase,
                results.len()
            );
            thread::sleep(Duration::from_millis(3));
        }
        assert_eq!(
            results.remove(&opening).unwrap().unwrap_err().code,
            "CANCELLED"
        );
        results.remove(&suspend).unwrap().unwrap();
        assert_eq!(h.status().phase, ServicePhase::Closed);
        assert!(h.view().is_none());
        assert!(
            path.join("store/manifests").exists(),
            "Interrupted opening preserved a durable recoverable workspace"
        );
        open(&h, &path, OpenMode::Existing, false);
        shutdown(service);
    }
    struct BlockingRead {
        started: AtomicBool,
        release: AtomicBool,
    }
    impl HostGateway for BlockingRead {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            params: Value,
            _: Duration,
        ) -> Result<Value> {
            if operation == "ui.execute" {
                self.started.store(true, Ordering::Release);
                let end = Instant::now() + Duration::from_secs(5);
                while !self.release.load(Ordering::Acquire) && Instant::now() < end {
                    thread::sleep(Duration::from_millis(2));
                }
                return Ok(
                    json!({"path":params["input"]["path"],"size":4,"binary":false,"content":"late"}),
                );
            }
            if operation == "ui.stop" {
                return Ok(json!({"accepted":true}));
            }
            Err(Error::new(
                "FEATURE_DISABLED",
                "Fixture only implements deferred read",
            ))
        }
    }
    #[test]
    fn deferred_native_read_does_not_block_queries_and_cancellation_is_typed() {
        let dir = tempfile::tempdir().unwrap();
        let host = Arc::new(BlockingRead {
            started: AtomicBool::new(false),
            release: AtomicBool::new(false),
        });
        let factory_host = host.clone();
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |_| {
                Ok(factory_host.clone() as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &dir.path().join("w"),
            OpenMode::Create {
                name: "Deferred".into(),
            },
            false,
        );
        let read = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.workspaceRead".into(),
                    params: json!({"path":"note.txt"}),
                },
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while !host.started.load(Ordering::Acquire) {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(3));
        }
        let info = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::WorkspaceInfo,
            })
            .unwrap();
        assert!(wait(&h, &info).unwrap().scope.is_some());
        h.cancel(&read).unwrap();
        assert_eq!(wait(&h, &read).unwrap_err().code, "CANCELLED");
        host.release.store(true, Ordering::Release);
        shutdown(service);
    }
    #[test]
    #[cfg(unix)]
    fn non_utf8_file_actions_are_request_errors_and_writer_keeps_valid_file_operations_alive() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |workspace| {
                Ok(Arc::new(crate::gateway::NativeGateway::open(
                    workspace,
                    secrets.clone(),
                )?) as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let handle = service.handle();
        let scope = open(
            &handle,
            &dir.path().join("workspace"),
            OpenMode::Create {
                name: "Unix path validation".into(),
            },
            false,
        );
        let invalid_source = dir
            .path()
            .join(OsString::from_vec(b"input-\xff.txt".to_vec()));
        let invalid_destination = dir
            .path()
            .join(OsString::from_vec(b"output-\xfe.txt".to_vec()));
        // APFS rejects non-UTF-8 filenames with EILSEQ (Darwin errno 92).
        // Still submit both invalid native paths: parameter validation must
        // reject them before I/O and leave the writer usable on every Unix OS.
        fn invalid_fixture(path: &std::path::Path, bytes: &[u8]) -> bool {
            match std::fs::write(path, bytes) {
                Ok(()) => true,
                Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) => {
                    false
                }
                Err(error) => panic!("Cannot prepare invalid-path fixture: {error}"),
            }
        }
        let source_exists = invalid_fixture(&invalid_source, b"source preserved");
        let destination_exists = invalid_fixture(&invalid_destination, b"destination preserved");
        let source = dir.path().join("정상 input.txt");
        std::fs::write(&source, b"valid utf8 path bytes").unwrap();
        let invalid = handle
            .try_submit(NativeAction::StageFile {
                scope: scope.clone(),
                path: invalid_source.clone(),
            })
            .unwrap();
        let error = wait(&handle, &invalid).unwrap_err();
        assert_eq!(error.code, "INVALID_PARAMS");
        assert!(!error.recovery_required);
        assert_eq!(handle.status().phase, ServicePhase::Ready);
        if source_exists {
            assert_eq!(std::fs::read(&invalid_source).unwrap(), b"source preserved");
        }
        let stage = handle
            .try_submit(NativeAction::StageFile {
                scope: scope.clone(),
                path: source.clone(),
            })
            .unwrap();
        let descriptor = wait(&handle, &stage).unwrap().value;
        let token = descriptor["file_capability"].as_str().unwrap().to_owned();
        let invalid = handle
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: token.clone(),
                destination: invalid_destination.clone(),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        let error = wait(&handle, &invalid).unwrap_err();
        assert_eq!(error.code, "INVALID_PARAMS");
        assert!(!error.recovery_required);
        assert_eq!(handle.status().phase, ServicePhase::Ready);
        if destination_exists {
            assert_eq!(
                std::fs::read(&invalid_destination).unwrap(),
                b"destination preserved"
            );
        }
        let query = handle
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::WorkspaceInfo,
            })
            .unwrap();
        assert_eq!(wait(&handle, &query).unwrap().scope, Some(scope.clone()));
        let destination = dir.path().join("정상 output.txt");
        let saved = handle
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: token.clone(),
                destination: destination.clone(),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        wait(&handle, &saved).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"valid utf8 path bytes"
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"valid utf8 path bytes");
        let release = handle
            .try_submit(NativeAction::ReleaseFile { scope, token })
            .unwrap();
        wait(&handle, &release).unwrap();
        shutdown(service);
    }
    #[test]
    fn actual_native_gateway_file_cas_returns_direct_results_without_engine_or_frames() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |w| {
                Ok(
                    Arc::new(crate::gateway::NativeGateway::open(w, secrets.clone())?)
                        as Arc<dyn HostGateway>,
                )
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &dir.path().join("w"),
            OpenMode::Create {
                name: "Actual direct files".into(),
            },
            false,
        );
        let command_id = artex_core::new_id("command");
        let write = NativeAction::FileWrite {
            scope: scope.clone(),
            area: NativeFileArea::Work,
            path: "note.txt".into(),
            content: "first bytes".into(),
            expected_sha256: None,
            create: true,
            command_id,
        };
        let ticket = h.try_submit(write.clone()).unwrap();
        let saved = wait(&h, &ticket).unwrap();
        assert_eq!(saved.value["saved"], true);
        assert!(saved.value.get("jsonrpc").is_none());
        let repeated = h.try_submit(write).unwrap();
        assert_eq!(wait(&h, &repeated).unwrap().value, saved.value);
        let read = h
            .try_submit(NativeAction::FileRead {
                scope: scope.clone(),
                area: NativeFileArea::Work,
                path: "note.txt".into(),
            })
            .unwrap();
        let loaded = wait(&h, &read).unwrap();
        assert_eq!(loaded.value["content"], "first bytes");
        let stale = h
            .try_submit(NativeAction::FileWrite {
                scope: scope.clone(),
                area: NativeFileArea::Work,
                path: "note.txt".into(),
                content: "must not overwrite".into(),
                expected_sha256: Some("0".repeat(64)),
                create: false,
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        assert_eq!(wait(&h, &stale).unwrap_err().code, "VERSION_CONFLICT");
        let changed = h
            .try_submit(NativeAction::FileWrite {
                scope: scope.clone(),
                area: NativeFileArea::Work,
                path: "note.txt".into(),
                content: "second bytes".into(),
                expected_sha256: Some(loaded.value["sha256"].as_str().unwrap().into()),
                create: false,
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        wait(&h, &changed).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("w/work/note.txt")).unwrap(),
            "second bytes"
        );
        let escape = h
            .try_submit(NativeAction::FileRead {
                scope,
                area: NativeFileArea::Work,
                path: "../journal".into(),
            })
            .unwrap();
        assert_eq!(wait(&h, &escape).unwrap_err().code, "PERMISSION_DENIED");
        shutdown(service);
    }
    #[test]
    fn schema_declared_70k_skill_body_prepares_saves_replays_and_hydrates_without_wire_upload() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |w| {
                Ok(
                    Arc::new(crate::gateway::NativeGateway::open(w, secrets.clone())?)
                        as Arc<dyn HostGateway>,
                )
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(
            &h,
            &dir.path().join("w"),
            OpenMode::Create {
                name: "Large native Body".into(),
            },
            false,
        );
        let original = "Native standing procedure\n".repeat(3000);
        assert!(original.len() > 65536);
        let params = json!({"s":{"name":"large-body","description":"Large native body fixture","instructions":artex_core::body(&original)}});
        let prepared = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.createSkill".into(),
                    params: params.clone(),
                },
            })
            .unwrap();
        assert!(
            wait(&h, &prepared).unwrap().value["expected"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let command = NativeCommand::new("ui.createSkill", params, vec![]);
        let command_id = command.id.clone();
        let create = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command: command.clone(),
            })
            .unwrap();
        let result = wait(&h, &create).unwrap();
        let replay = h
            .try_submit(NativeAction::Command {
                scope: scope.clone(),
                command,
            })
            .unwrap();
        assert_eq!(wait(&h, &replay).unwrap().value, result.value);
        let view = h.snapshot().unwrap();
        let receipt = view
            .records("command_receipts")
            .into_iter()
            .find(|row| row.data["command_id"] == command_id)
            .unwrap();
        let body = receipt.data["normalized_params"]["s"]["instructions"].clone();
        assert!(body["text"].is_null() && body["blob"].is_object());
        drop(view);
        let hydrate = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Body { body },
            })
            .unwrap();
        assert_eq!(wait(&h, &hydrate).unwrap().value, original);
        let file =
            std::fs::read_to_string(dir.path().join("w/resources/skills/large-body/SKILL.md"))
                .unwrap();
        assert!(file.ends_with(&format!("{original}\n")));
        shutdown(service);
    }
    #[test]
    fn readonly_verified_blob_and_workspace_download_export_via_native_tokens_without_workspace_writes()
     {
        fn contents(root: &std::path::Path) -> BTreeMap<std::path::PathBuf, (usize, String)> {
            use sha2::{Digest, Sha256};
            fn visit(
                root: &std::path::Path,
                directory: &std::path::Path,
                result: &mut BTreeMap<std::path::PathBuf, (usize, String)>,
            ) {
                for entry in std::fs::read_dir(directory).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        visit(root, &path, result);
                    } else {
                        let bytes = std::fs::read(&path).unwrap();
                        result.insert(
                            path.strip_prefix(root).unwrap().to_path_buf(),
                            (bytes.len(), format!("{:x}", Sha256::digest(&bytes))),
                        );
                    }
                }
            }
            let mut result = BTreeMap::new();
            visit(root, root, &mut result);
            result
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let original = (0..131_073usize)
            .map(|n| (n % 256) as u8)
            .collect::<Vec<_>>();
        let mut store = artex_store::Store::create(&path, "Native readonly export").unwrap();
        let blob = store
            .blob_put(&original, "application/octet-stream")
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        std::fs::create_dir_all(path.join("work")).unwrap();
        std::fs::write(path.join("work/note.txt"), "chosen workspace file").unwrap();
        let before = contents(&path);
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let secrets = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |w| {
                Ok(Arc::new(crate::gateway::NativeGateway::open_with_profile(
                    w,
                    secrets.clone(),
                    &profile,
                )?) as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(&h, &path, OpenMode::Existing, true);
        let staged = h
            .try_submit(NativeAction::StageBody {
                scope: scope.clone(),
                body: json!({"text":null,"blob":blob}),
                name: "archive.bin".into(),
            })
            .unwrap();
        let descriptor = wait(&h, &staged).unwrap().value;
        assert_eq!(descriptor["name"], "archive.bin");
        assert_eq!(descriptor["byte_length"], original.len().to_string());
        let destination = dir.path().join("export.bin");
        let action = NativeAction::SaveFile {
            scope: scope.clone(),
            token: descriptor["file_capability"].as_str().unwrap().into(),
            destination: destination.clone(),
            command_id: artex_core::new_id("command"),
        };
        let saved = h.try_submit(action.clone()).unwrap();
        let saved_value = wait(&h, &saved).unwrap().value;
        assert_eq!(std::fs::read(&destination).unwrap(), original);
        let repeated = h.try_submit(action).unwrap();
        assert_eq!(wait(&h, &repeated).unwrap().value, saved_value);
        let download = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.workspaceDownload".into(),
                    params: json!({"path":"note.txt"}),
                },
            })
            .unwrap();
        let descriptor = wait(&h, &download).unwrap().value;
        assert!(descriptor["file_capability"].is_string());
        let save = h
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: descriptor["file_capability"].as_str().unwrap().into(),
                destination: dir.path().join("download.txt"),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        wait(&h, &save).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("download.txt")).unwrap(),
            "chosen workspace file"
        );
        let forbidden = h
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: descriptor["file_capability"].as_str().unwrap().into(),
                destination: path.join("work/forbidden.txt"),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        assert_eq!(wait(&h, &forbidden).unwrap_err().code, "PERMISSION_DENIED");
        shutdown(service);
        assert_eq!(contents(&path), before);
    }
    #[test]
    fn base64_export_streams_verified_binary_and_rejects_invalid_encoding_or_digest_before_staging()
    {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let binary = (0..262_147usize)
            .map(|n| (n % 253) as u8)
            .collect::<Vec<_>>();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&binary);
        let mut store = artex_store::Store::create(&path, "Base64 exports").unwrap();
        let reference = store.blob_put(encoded.as_bytes(), "text/plain").unwrap();
        let corrupt = store.blob_put(b"YWJjZA==", "text/plain").unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let digest = corrupt["sha256"].as_str().unwrap();
        std::fs::write(
            path.join(format!("objects/blobs/{}/{digest}", &digest[..2])),
            b"!WJjZA==",
        )
        .unwrap();
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let secrets = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |w| {
                Ok(Arc::new(crate::gateway::NativeGateway::open_with_profile(
                    w,
                    secrets.clone(),
                    &profile,
                )?) as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(&h, &path, OpenMode::Existing, true);
        let prepare = h
            .try_submit(NativeAction::StageBase64Body {
                scope: scope.clone(),
                body: json!({"text":null,"blob":reference}),
                name: "raw.zip".into(),
            })
            .unwrap();
        let descriptor = wait(&h, &prepare).unwrap().value;
        assert_eq!(descriptor["byte_length"], binary.len().to_string());
        let save = h
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: descriptor["file_capability"].as_str().unwrap().into(),
                destination: dir.path().join("raw.zip"),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        wait(&h, &save).unwrap();
        assert_eq!(std::fs::read(dir.path().join("raw.zip")).unwrap(), binary);
        let invalid = h
            .try_submit(NativeAction::StageBase64Body {
                scope: scope.clone(),
                body: artex_core::body("!not base64"),
                name: "invalid.zip".into(),
            })
            .unwrap();
        assert_eq!(wait(&h, &invalid).unwrap_err().code, "PARSE_ERROR");
        let mismatch = h
            .try_submit(NativeAction::StageBase64Body {
                scope: scope.clone(),
                body: json!({"text":null,"blob":corrupt}),
                name: "corrupt.zip".into(),
            })
            .unwrap();
        assert_eq!(wait(&h, &mismatch).unwrap_err().code, "CORRUPT_STORE");
        assert!(!dir.path().join("invalid.zip").exists());
        assert!(!dir.path().join("corrupt.zip").exists());
        shutdown(service);
    }
    #[test]
    fn readonly_generated_report_and_findings_export_read_and_stage_current_epoch_cache_without_store_writes()
     {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        fn fingerprints(root: &std::path::Path) -> BTreeMap<std::path::PathBuf, (u64, String)> {
            fn visit(
                root: &std::path::Path,
                path: &std::path::Path,
                result: &mut BTreeMap<std::path::PathBuf, (u64, String)>,
            ) {
                for entry in std::fs::read_dir(path).unwrap() {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        visit(root, &path, result);
                    } else {
                        let bytes = std::fs::read(&path).unwrap();
                        result.insert(
                            path.strip_prefix(root).unwrap().to_path_buf(),
                            (bytes.len() as u64, format!("{:x}", Sha256::digest(&bytes))),
                        );
                    }
                }
            }
            let mut result = BTreeMap::new();
            visit(root, root, &mut result);
            result
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let mut store = artex_store::Store::create(&path, "Generated readonly exports").unwrap();
        let exp = artex_core::new_id("exploration");
        let task = artex_core::new_id("task");
        let node = artex_core::new_id("node");
        let finding = artex_core::new_id("finding");
        let text = "Verified evidence for readonly generated body.\n".repeat(2500);
        let reference = store.blob_put(text.as_bytes(), "text/plain").unwrap();
        let body = json!({"text":null,"blob":reference});
        let mut exp_data = artex_core::default_data("explorations").unwrap();
        exp_data["id"] = json!(exp);
        let mut task_data = artex_core::default_data("tasks").unwrap();
        task_data["id"] = json!(task);
        task_data["exploration_id"] = json!(exp);
        task_data["name"] = json!("Export task");
        task_data["description"] = json!("Verify readonly generated reports");
        task_data["goal"] = json!("Export confirmed evidence");
        let mut node_data = artex_core::default_data("exploration_nodes").unwrap();
        node_data["id"] = json!(node);
        node_data["exploration_id"] = json!(exp);
        node_data["kind"] = json!("finding");
        node_data["state"] = json!("confirmed");
        node_data["payload"] = json!({"name":"Large evidence","severity":"high","summary":"Cached readonly report","evidence":body});
        let mut finding_data = artex_core::default_data("findings").unwrap();
        finding_data["id"] = json!(finding);
        finding_data["task_id"] = json!(task);
        finding_data["node_id"] = json!(node);
        finding_data["name"] = json!("Large finding report");
        finding_data["severity"] = json!("high");
        finding_data["status"] = json!("confirmed");
        finding_data["report"] = body;
        store
            .commit(artex_core::CommandBatch {
                command_id: artex_core::new_id("command"),
                method: "fixture.generated_report".into(),
                normalized_params: json!({}),
                expected: vec![],
                changes: vec![
                    artex_core::Change::Put {
                        collection: "explorations".into(),
                        id: exp,
                        data: exp_data,
                    },
                    artex_core::Change::Put {
                        collection: "tasks".into(),
                        id: task.clone(),
                        data: task_data,
                    },
                    artex_core::Change::Put {
                        collection: "exploration_nodes".into(),
                        id: node,
                        data: node_data,
                    },
                    artex_core::Change::Put {
                        collection: "findings".into(),
                        id: finding,
                        data: finding_data,
                    },
                ],
                events: vec![],
                deliveries: vec![],
                result: json!({}),
            })
            .unwrap();
        store.checkpoint().unwrap();
        drop(store);
        let before = fingerprints(&path);
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let vault = Arc::new(artex_host::SecretVault::default());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(Arc::new(move |w| {
                Ok(Arc::new(crate::gateway::NativeGateway::open_with_profile(
                    w,
                    vault.clone(),
                    &profile,
                )?) as Arc<dyn HostGateway>)
            })),
            wake_ui: None,
        })
        .unwrap();
        let h = service.handle();
        let scope = open(&h, &path, OpenMode::Existing, true);
        let report = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.report".into(),
                    params: json!({"task":task}),
                },
            })
            .unwrap();
        let generated = wait(&h, &report).unwrap().value;
        assert!(generated["text"].is_null() && generated["blob"].is_object());
        assert!(
            generated["blob"]["length"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 65536
        );
        let read = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Body {
                    body: generated.clone(),
                },
            })
            .unwrap();
        let report_text = wait(&h, &read).unwrap().value.as_str().unwrap().to_owned();
        assert!(report_text.contains(&text));
        let export = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Feature {
                    method: "ui.exportFindings".into(),
                    params: json!({"opts":{"format":"md-single","scope":"all"}}),
                },
            })
            .unwrap();
        let output = wait(&h, &export).unwrap().value;
        let encoded = output["content_base64"].clone();
        assert!(encoded["text"].is_null() && encoded["blob"].is_object());
        let read = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Body {
                    body: encoded.clone(),
                },
            })
            .unwrap();
        let expected = base64::engine::general_purpose::STANDARD
            .decode(wait(&h, &read).unwrap().value.as_str().unwrap())
            .unwrap();
        assert!(String::from_utf8_lossy(&expected).contains(&text));
        let staged = h
            .try_submit(NativeAction::StageBase64Body {
                scope: scope.clone(),
                body: encoded.clone(),
                name: "findings.md".into(),
            })
            .unwrap();
        let descriptor = wait(&h, &staged).unwrap().value;
        let saved = h
            .try_submit(NativeAction::SaveFile {
                scope: scope.clone(),
                token: descriptor["file_capability"].as_str().unwrap().into(),
                destination: dir.path().join("findings.md"),
                command_id: artex_core::new_id("command"),
            })
            .unwrap();
        wait(&h, &saved).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("findings.md")).unwrap(),
            expected
        );
        let close = h
            .try_submit(NativeAction::Close {
                scope: scope.clone(),
            })
            .unwrap();
        wait(&h, &close).unwrap();
        let reopened = open(&h, &path, OpenMode::Existing, true);
        assert_eq!(
            h.try_submit(NativeAction::StageBody {
                scope,
                body: generated,
                name: "stale.md".into()
            })
            .unwrap_err()
            .code,
            "STALE_EPOCH"
        );
        let expired = h
            .try_submit(NativeAction::Query {
                scope: reopened,
                query: NativeQuery::Body { body: encoded },
            })
            .unwrap();
        assert!(
            wait(&h, &expired).is_err(),
            "Retired ephemeral bytes cannot be reused in a new epoch"
        );
        shutdown(service);
        assert_eq!(fingerprints(&path), before);
    }
    #[test]
    fn export_decoder_is_chunk_bounded_and_cancelled_without_retrying_interrupted_writes() {
        use base64::Engine;
        struct BoundedWriter {
            bytes: Vec<u8>,
            maximum: usize,
        }
        impl std::io::Write for BoundedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.maximum = self.maximum.max(bytes.len());
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let input = vec![0xfcu8; 200_003];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&input);
        let mut decoder = base64::read::DecoderReader::new(
            std::io::Cursor::new(encoded.as_bytes()),
            &base64::engine::general_purpose::STANDARD,
        );
        let mut writer = BoundedWriter {
            bytes: Vec::new(),
            maximum: 0,
        };
        assert_eq!(
            decode_export(&mut decoder, &mut writer, &AtomicBool::new(false)).unwrap(),
            input.len() as u64
        );
        assert_eq!(writer.bytes, input);
        assert!(writer.maximum <= 65536);
        let mut decoder = base64::read::DecoderReader::new(
            std::io::Cursor::new(encoded.as_bytes()),
            &base64::engine::general_purpose::STANDARD,
        );
        let mut untouched = Vec::new();
        assert_eq!(
            decode_export(&mut decoder, &mut untouched, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            "CANCELLED"
        );
        assert!(untouched.is_empty());
    }
}
