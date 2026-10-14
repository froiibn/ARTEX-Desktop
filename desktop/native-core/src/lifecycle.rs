//! Window and power lifecycle uses asynchronous writer tickets. No UI joins.
use crate::{
    NativeAction, NativeCompletion, NativeTicket, OpenOptions, ServiceHandle, ServicePhase,
};
use artex_core::{Error, Result};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub enum PowerNotice {
    Suspend {
        ticket: NativeTicket,
        fallback: bool,
    },
    Resume,
    Failed(Error),
}
struct PowerState {
    notices: VecDeque<PowerNotice>,
    clock: Option<(u128, u64)>,
}
/// Own for the native window lifetime. Drain notices before draining service
/// completions so suspend tickets are registered in the UI pending map.
pub struct NativePower {
    monitor: Arc<artex_host::power::PowerMonitor>,
    service: ServiceHandle,
    state: Arc<Mutex<PowerState>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl NativePower {
    pub fn start(service: ServiceHandle, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let state = Arc::new(Mutex::new(PowerState {
            notices: VecDeque::new(),
            clock: artex_host::power::active_time_ms().ok().map(|active| {
                (
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                    active,
                )
            }),
        }));
        let callback_state = state.clone();
        let callback_service = service.clone();
        let callback_wake = wake.clone();
        let monitor =
            artex_host::power::PowerMonitor::start(Arc::new(move |event, budget| match event {
                artex_host::power::PowerEvent::Suspend => Self::suspend(
                    &callback_service,
                    &callback_state,
                    &callback_wake,
                    budget,
                    false,
                ),
                artex_host::power::PowerEvent::Resume => {
                    Self::notice(&callback_state, PowerNotice::Resume);
                    callback_wake();
                }
            }));
        Self {
            monitor,
            service,
            state,
            wake,
        }
    }
    fn notice(state: &Mutex<PowerState>, notice: PowerNotice) {
        let mut state = state.lock().unwrap();
        if state.notices.len() >= 16 {
            state.notices.pop_front();
        }
        state.notices.push_back(notice);
    }
    fn suspend(
        service: &ServiceHandle,
        state: &Mutex<PowerState>,
        wake: &Arc<dyn Fn() + Send + Sync>,
        budget: Duration,
        fallback: bool,
    ) {
        // The priority service transition fences admission before any wait.
        match service.suspend() {
            Ok(ticket) => {
                Self::notice(state, PowerNotice::Suspend { ticket, fallback });
                wake();
                let deadline = Instant::now() + budget;
                while Instant::now() < deadline && service.status().phase == ServicePhase::Closing {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(error) => {
                Self::notice(state, PowerNotice::Failed(error));
                wake();
            }
        }
    }
    pub fn available(&self) -> bool {
        self.monitor.available()
    }
    pub fn registration_error(&self) -> Option<String> {
        self.monitor.error()
    }
    pub fn drain_notices(&self) -> Vec<PowerNotice> {
        self.state.lock().unwrap().notices.drain(..).collect()
    }
    /// A clock-gap fallback remains conservative: retire the workspace and
    /// require explicit reopen. It never claims a pre-suspend checkpoint.
    pub fn tick(&self) {
        let Ok(active) = artex_host::power::active_time_ms() else {
            return;
        };
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let gap = {
            let mut state = self.state.lock().unwrap();
            let gap = state.clock.is_some_and(|(old_wall, old_active)| {
                wall.saturating_sub(old_wall) > active.saturating_sub(old_active) as u128 + 2000
            });
            state.clock = Some((wall, active));
            gap
        };
        if gap
            && matches!(
                self.service.status().phase,
                ServicePhase::Ready | ServicePhase::Opening
            )
        {
            Self::suspend(&self.service, &self.state, &self.wake, Duration::ZERO, true);
        }
    }
    #[cfg(windows)]
    pub fn inject_suspend(&self) -> bool {
        self.monitor.inject_suspend()
    }
}
impl Drop for NativePower {
    fn drop(&mut self) {
        self.monitor.stop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Exit,
    Suspend,
    WorkspaceSwitch,
}
#[derive(Clone, Debug, PartialEq)]
pub enum LifecycleNotice {
    Opened,
    Closed(CloseReason),
    ExitReady,
    Failed(Error),
    Unrelated,
}
enum Pending {
    Open {
        ticket: NativeTicket,
        path: PathBuf,
        read_only: bool,
    },
    Close {
        ticket: NativeTicket,
        reason: CloseReason,
    },
    Shutdown {
        ticket: NativeTicket,
    },
}
struct State {
    pending: Option<Pending>,
    path: Option<PathBuf>,
    read_only: bool,
    resume_required: bool,
    retries: Vec<Instant>,
}
/// Share this controller with platform power notifications; callbacks only
/// submit work. UI completion handling explicitly confirms retirement.
pub struct Lifecycle {
    service: ServiceHandle,
    state: Mutex<State>,
}
impl Lifecycle {
    pub fn new(service: ServiceHandle) -> Self {
        Self {
            service,
            state: Mutex::new(State {
                pending: None,
                path: None,
                read_only: false,
                resume_required: false,
                retries: Vec::new(),
            }),
        }
    }
    pub fn open(&self, options: OpenOptions) -> Result<NativeTicket> {
        let mut state = self.state.lock().unwrap();
        if state.pending.is_some() {
            return Err(Error::new("BUSY", "Native lifecycle work is pending"));
        }
        let path = options.path.clone();
        let read_only = options.read_only;
        let ticket = self.service.try_submit(NativeAction::Open(options))?;
        state.pending = Some(Pending::Open {
            ticket: ticket.clone(),
            path,
            read_only,
        });
        Ok(ticket)
    }
    pub fn reopen(&self) -> Result<NativeTicket> {
        let mut state = self.state.lock().unwrap();
        if state.pending.is_some() {
            return Err(Error::new("BUSY", "Native lifecycle work is pending"));
        }
        let now = Instant::now();
        state
            .retries
            .retain(|time| now.duration_since(*time) < Duration::from_secs(60));
        if state.retries.len() >= 3 {
            return Err(Error::new(
                "BUSY",
                "Explicit reopen limit reached; wait before retrying",
            ));
        }
        let path = state.path.clone().ok_or_else(|| {
            Error::new(
                "WORKSPACE_NOT_OPEN",
                "No previously opened native workspace",
            )
        })?;
        let read_only = state.read_only;
        let options = OpenOptions {
            path: path.clone(),
            mode: crate::OpenMode::Existing,
            read_only,
        };
        let ticket = self.service.try_submit(NativeAction::Open(options))?;
        state.retries.push(now);
        state.pending = Some(Pending::Open {
            ticket: ticket.clone(),
            path,
            read_only,
        });
        Ok(ticket)
    }
    /// The service closes admission synchronously before putting Close on its
    /// queue. No power callback takes the writer or waits for filesystem I/O.
    pub fn close(&self, reason: CloseReason) -> Result<NativeTicket> {
        let mut state = self.state.lock().unwrap();
        if state.pending.is_some() {
            return Err(Error::new("BUSY", "Native lifecycle work is pending"));
        }
        let status = self.service.status();
        let ticket = match status.phase {
            ServicePhase::Ready => self.service.try_submit(NativeAction::Close {
                scope: status
                    .scope
                    .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Native scope disappeared"))?,
            })?,
            ServicePhase::Closed if reason == CloseReason::Exit => {
                let ticket = self.service.try_submit(NativeAction::Shutdown)?;
                state.pending = Some(Pending::Shutdown {
                    ticket: ticket.clone(),
                });
                return Ok(ticket);
            }
            _ => {
                return Err(Error::new(
                    "BUSY",
                    "Native workspace cannot retire in its current phase",
                ));
            }
        };
        state.pending = Some(Pending::Close {
            ticket: ticket.clone(),
            reason,
        });
        Ok(ticket)
    }
    pub fn observe(&self, completion: &NativeCompletion) -> LifecycleNotice {
        let mut state = self.state.lock().unwrap();
        let matches = match state.pending.as_ref() {
            Some(
                Pending::Open { ticket, .. }
                | Pending::Close { ticket, .. }
                | Pending::Shutdown { ticket },
            ) => ticket == &completion.ticket,
            None => false,
        };
        if !matches {
            return LifecycleNotice::Unrelated;
        }
        let pending = state.pending.take().unwrap();
        if let Err(error) = &completion.result {
            return LifecycleNotice::Failed(error.clone());
        }
        match pending {
            Pending::Open {
                path, read_only, ..
            } => {
                state.path = Some(path);
                state.read_only = read_only;
                state.resume_required = false;
                LifecycleNotice::Opened
            }
            Pending::Close {
                reason: CloseReason::Exit,
                ..
            } => match self.service.try_submit(NativeAction::Shutdown) {
                Ok(ticket) => {
                    state.pending = Some(Pending::Shutdown { ticket });
                    LifecycleNotice::Closed(CloseReason::Exit)
                }
                Err(error) => LifecycleNotice::Failed(error),
            },
            Pending::Close { reason, .. } => {
                state.resume_required = reason == CloseReason::Suspend;
                LifecycleNotice::Closed(reason)
            }
            Pending::Shutdown { .. } => LifecycleNotice::ExitReady,
        }
    }
    pub fn recovery_path(&self) -> Option<PathBuf> {
        self.state.lock().unwrap().path.clone()
    }
    pub fn resume_required(&self) -> bool {
        self.state.lock().unwrap().resume_required
    }
    pub fn pending(&self) -> bool {
        self.state.lock().unwrap().pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OpenMode, Service, ServiceConfig};
    use std::thread;
    fn complete(
        service: &ServiceHandle,
        lifecycle: &Lifecycle,
        ticket: &NativeTicket,
    ) -> LifecycleNotice {
        complete_with_budget(service, lifecycle, ticket, Duration::from_secs(10))
    }
    fn complete_with_budget(
        service: &ServiceHandle,
        lifecycle: &Lifecycle,
        ticket: &NativeTicket,
        budget: Duration,
    ) -> LifecycleNotice {
        let started = Instant::now();
        let end = started + budget;
        loop {
            for completion in service.drain_completions() {
                let notice = lifecycle.observe(&completion);
                if &completion.ticket == ticket {
                    return notice;
                }
            }
            assert!(
                Instant::now() < end,
                "Lifecycle ticket {ticket:?} timed out after {:?}; status={:?}",
                started.elapsed(),
                service.status()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn only_confirmed_open_changes_recovery_path_and_suspend_blocks_admission() {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = service.handle();
        let lifecycle = Lifecycle::new(handle.clone());
        let opened = lifecycle
            .open(OpenOptions {
                path: dir.path().join("a"),
                mode: OpenMode::Create {
                    name: "Native lifecycle".into(),
                },
                read_only: false,
            })
            .unwrap();
        assert_eq!(
            complete_with_budget(&handle, &lifecycle, &opened, Duration::from_secs(60)),
            LifecycleNotice::Opened
        );
        let path = lifecycle.recovery_path().unwrap();
        let close = lifecycle.close(CloseReason::Suspend).unwrap();
        assert_eq!(handle.status().phase, ServicePhase::Closing);
        assert!(lifecycle.reopen().is_err());
        assert_eq!(
            complete(&handle, &lifecycle, &close),
            LifecycleNotice::Closed(CloseReason::Suspend)
        );
        assert!(lifecycle.resume_required());
        let bad = lifecycle
            .open(OpenOptions {
                path: dir.path().join("missing"),
                mode: OpenMode::Existing,
                read_only: false,
            })
            .unwrap();
        assert!(matches!(
            complete(&handle, &lifecycle, &bad),
            LifecycleNotice::Failed(_)
        ));
        assert_eq!(lifecycle.recovery_path().unwrap(), path);
        let reopen = lifecycle.reopen().unwrap();
        assert_eq!(
            complete(&handle, &lifecycle, &reopen),
            LifecycleNotice::Opened
        );
        assert!(!lifecycle.resume_required());
        let close = lifecycle.close(CloseReason::WorkspaceSwitch).unwrap();
        complete(&handle, &lifecycle, &close);
        let stop = handle.try_submit(NativeAction::Shutdown).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if handle.drain_completions().iter().any(|c| c.ticket == stop) {
                break;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
        service.join().unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn actual_windows_power_listener_fences_admission_and_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = service.handle();
        let path = dir.path().join("power");
        let ticket = handle
            .try_submit(NativeAction::Open(OpenOptions {
                path: path.clone(),
                mode: OpenMode::Create {
                    name: "Native OS power".into(),
                },
                read_only: false,
            }))
            .unwrap();
        // Durable creation writes all initial collection snapshots. Its setup
        // budget is separate from the registration and suspend bounds below.
        let started = Instant::now();
        let end = started + Duration::from_secs(60);
        loop {
            if let Some(completion) = handle
                .drain_completions()
                .into_iter()
                .find(|c| c.ticket == ticket)
            {
                completion.result.unwrap_or_else(|error| {
                    panic!("Workspace creation failed: {error:?}; status={:?}", handle.status())
                });
                break;
            }
            assert!(
                Instant::now() < end,
                "Workspace creation ticket {ticket:?} timed out after {:?}; status={:?}",
                started.elapsed(),
                handle.status()
            );
            thread::sleep(Duration::from_millis(5));
        }
        let manifests = path.join("store/manifests");
        let before = std::fs::read_dir(&manifests).unwrap().count();
        let power = NativePower::start(handle.clone(), Arc::new(|| {}));
        let end = Instant::now() + Duration::from_secs(5);
        while !power.available() {
            assert!(Instant::now() < end, "{:?}", power.registration_error());
            thread::sleep(Duration::from_millis(5));
        }
        assert!(power.inject_suspend());
        let notices = power.drain_notices();
        let ticket = notices
            .into_iter()
            .find_map(|n| match n {
                PowerNotice::Suspend {
                    ticket,
                    fallback: false,
                } => Some(ticket),
                _ => None,
            })
            .expect("actual power notification must issue retirement ticket");
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(completion) = handle
                .drain_completions()
                .into_iter()
                .find(|c| c.ticket == ticket)
            {
                completion.result.unwrap();
                break;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(handle.status().phase, ServicePhase::Closed);
        assert!(std::fs::read_dir(manifests).unwrap().count() > before);
        drop(power);
        let shutdown = handle.try_submit(NativeAction::Shutdown).unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if handle
                .drain_completions()
                .iter()
                .any(|c| c.ticket == shutdown)
            {
                break;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(5));
        }
        service.join().unwrap();
    }
}
