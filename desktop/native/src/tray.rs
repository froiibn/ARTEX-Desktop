//! Win32/AppKit icons stay on the eframe UI thread. Linux uses the GTK-free
//! KSNI worker and verifies an actual StatusNotifier host before hiding a window.
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
}
struct Shared {
    actions: Mutex<VecDeque<TrayAction>>,
    available: AtomicBool,
    error: Mutex<Option<String>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Shared {
    fn action(&self, action: TrayAction) {
        let mut queue = self.actions.lock().unwrap();
        if queue.back() == Some(&action) {
            return;
        }
        if action == TrayAction::Quit {
            queue.clear();
        }
        if queue.len() >= 8 {
            queue.pop_front();
        }
        queue.push_back(action);
        drop(queue);
        (self.wake)();
    }
    fn failed(&self, error: String) {
        let was = self.available.swap(false, Ordering::AcqRel);
        *self.error.lock().unwrap() = Some(error);
        if was {
            self.action(TrayAction::Show)
        }
        (self.wake)();
    }
}
pub struct NativeTray {
    shared: Arc<Shared>,
    started: bool,
    #[cfg(any(windows, target_os = "macos"))]
    icon: Option<tray_icon::TrayIcon>,
    #[cfg(target_os = "linux")]
    stop: Option<std::sync::mpsc::Sender<()>>,
}
impl NativeTray {
    /// Call poll on the eframe UI thread after the native event loop has begun.
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            shared: Arc::new(Shared {
                actions: Mutex::new(VecDeque::new()),
                available: AtomicBool::new(false),
                error: Mutex::new(None),
                wake,
            }),
            started: false,
            #[cfg(any(windows, target_os = "macos"))]
            icon: None,
            #[cfg(target_os = "linux")]
            stop: None,
        }
    }
    pub fn poll(&mut self) -> Vec<TrayAction> {
        if !self.started {
            self.started = true;
            self.start();
        }
        self.shared.actions.lock().unwrap().drain(..).collect()
    }
    pub fn available(&self) -> bool {
        self.shared.available.load(Ordering::Acquire)
    }
    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }
    #[cfg(any(windows, target_os = "macos"))]
    fn start(&mut self) {
        match build(self.shared.clone()) {
            Ok(icon) => {
                self.icon = Some(icon);
                self.shared.available.store(true, Ordering::Release);
            }
            Err(error) => self.shared.failed(error),
        }
    }
    #[cfg(target_os = "linux")]
    fn start(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.stop = Some(tx);
        let shared = self.shared.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("artex-native-tray".into())
            .spawn(move || {
                let result = (|| {
                    let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
                    let watcher = zbus::blocking::Proxy::new(
                        &conn,
                        "org.kde.StatusNotifierWatcher",
                        "/StatusNotifierWatcher",
                        "org.kde.StatusNotifierWatcher",
                    )
                    .map_err(|e| e.to_string())?;
                    let registered: bool = watcher
                        .get_property("IsStatusNotifierHostRegistered")
                        .map_err(|e| e.to_string())?;
                    if !registered {
                        return Err(
                            "StatusNotifier host is unavailable; window close uses normal quit"
                                .into(),
                        );
                    }
                    let _icon = build(shared.clone())?;
                    shared.available.store(true, Ordering::Release);
                    (shared.wake)();
                    loop {
                        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(_) => {}
                        }
                        if !watcher
                            .get_property::<bool>("IsStatusNotifierHostRegistered")
                            .unwrap_or(false)
                        {
                            // Restore before the KSNI destructor waits on its
                            // worker after losing the desktop's D-Bus watcher.
                            shared.failed(
                                "Tray host disappeared; restoring the visible window".into(),
                            );
                            return Err(
                                "Tray host disappeared; restoring the visible window".into()
                            );
                        }
                    }
                    shared.available.store(false, Ordering::Release);
                    Ok(())
                })();
                if let Err(error) = result {
                    shared.failed(error);
                }
            })
        {
            self.shared.failed(error.to_string());
        }
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    fn start(&mut self) {
        self.shared
            .failed("Native tray is unsupported; window close uses normal quit".into());
    }
    #[cfg(test)]
    fn inject(&self, action: TrayAction) {
        self.shared.action(action);
    }
}
#[cfg(target_os = "linux")]
impl Drop for NativeTray {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
fn build(shared: Arc<Shared>) -> Result<tray_icon::TrayIcon, String> {
    use tray_icon::{
        MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem},
    };
    let show = MenuItem::with_id("artex-native-show", "ARTEX 열기 / Show", true, None);
    let quit = MenuItem::with_id("artex-native-quit", "저장 후 종료 / Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &quit])
        .map_err(|e| e.to_string())?;
    let menu_shared = shared.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
        "artex-native-show" => menu_shared.action(TrayAction::Show),
        "artex-native-quit" => menu_shared.action(TrayAction::Quit),
        _ => {}
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if event.id().as_ref() != "artex-native-tray" {
            return;
        }
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        ) {
            shared.action(TrayAction::Show);
        }
    }));
    let artwork = crate::app_icon::tray()?;
    let icon = tray_icon::Icon::from_rgba(artwork.rgba, artwork.width, artwork.height)
        .map_err(|e| e.to_string())?;
    TrayIconBuilder::new()
        .with_id("artex-native-tray")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("ARTEX Native")
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_actions_wake_and_quit_survives_show_flood() {
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = wakes.clone();
        let tray = NativeTray::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        for _ in 0..20 {
            tray.inject(TrayAction::Show)
        }
        tray.inject(TrayAction::Quit);
        let actions: Vec<_> = tray.shared.actions.lock().unwrap().drain(..).collect();
        assert_eq!(actions, vec![TrayAction::Quit]);
        assert!(wakes.load(Ordering::SeqCst) >= 2);
        assert!(!tray.available());
    }
}
