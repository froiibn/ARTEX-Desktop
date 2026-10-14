//! OS power boundary shared by the one-process desktop. No window framework.
//! Callers must fence admission before waiting for the supplied bounded budget.
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
}
pub type PowerCallback = Arc<dyn Fn(PowerEvent, Duration) + Send + Sync>;
/// Monotonic active time excludes suspend on all three supported platforms.
pub fn active_time_ms() -> std::io::Result<u64> {
    artex_platform::active_clock_ms()
}
pub struct PowerMonitor {
    available: AtomicBool,
    stopped: AtomicBool,
    thread: AtomicU32,
    window: AtomicUsize,
    error: Mutex<Option<String>>,
    callback: PowerCallback,
    #[cfg(target_os = "macos")]
    run_loop: AtomicUsize,
    #[cfg(target_os = "linux")]
    connection: Mutex<Option<zbus::blocking::Connection>>,
}
impl PowerMonitor {
    pub fn start(callback: PowerCallback) -> Arc<Self> {
        let monitor = Arc::new(Self {
            available: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            thread: AtomicU32::new(0),
            window: AtomicUsize::new(0),
            error: Mutex::new(None),
            callback,
            #[cfg(target_os = "macos")]
            run_loop: AtomicUsize::new(0),
            #[cfg(target_os = "linux")]
            connection: Mutex::new(None),
        });
        let owner = monitor.clone();
        std::thread::spawn(move || {
            if let Err(error) = native(owner.clone()) {
                *owner.error.lock().unwrap() = Some(error);
            }
            owner.available.store(false, Ordering::Release);
        });
        monitor
    }
    pub fn available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
    fn notify(&self, event: PowerEvent, budget: Duration) {
        if !self.stopped.load(Ordering::Acquire) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.callback)(event, budget)
            }));
        }
    }
    /// Never join the OS event thread on the UI thread.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        #[cfg(windows)]
        unsafe {
            let thread = self.thread.load(Ordering::Acquire);
            if thread != 0 {
                windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    thread,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_QUIT,
                    0,
                    0,
                );
            }
        }
        #[cfg(target_os = "macos")]
        unsafe {
            let ptr = self.run_loop.load(Ordering::Acquire);
            if ptr != 0 {
                mac::CFRunLoopStop(ptr as *const std::ffi::c_void);
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(connection) = self.connection.lock().unwrap().take() {
            std::thread::spawn(move || {
                let _ = connection.close();
            });
        }
    }
    /// Test/qualification hook emits the same event as WM_POWERBROADCAST.
    #[cfg(windows)]
    pub fn inject_suspend(&self) -> bool {
        let window = self.window.load(Ordering::Acquire);
        if window == 0 {
            return false;
        }
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                window as _,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_POWERBROADCAST,
                4,
                0,
            );
        }
        true
    }
}
#[cfg(windows)]
fn native(owner: Arc<PowerMonitor>) -> Result<(), String> {
    win::run(owner)
}
#[cfg(target_os = "macos")]
fn native(owner: Arc<PowerMonitor>) -> Result<(), String> {
    mac::run(owner)
}
#[cfg(target_os = "linux")]
fn native(owner: Arc<PowerMonitor>) -> Result<(), String> {
    let connection = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
    *owner.connection.lock().unwrap() = Some(connection.clone());
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .map_err(|e| e.to_string())?;
    let mut signals = proxy
        .receive_signal("PrepareForSleep")
        .map_err(|e| e.to_string())?;
    fn inhibit(proxy: &zbus::blocking::Proxy<'_>) -> Option<zbus::zvariant::OwnedFd> {
        proxy
            .call(
                "Inhibit",
                &(
                    "sleep",
                    "ARTEX Native",
                    "Checkpoint workspace before sleep",
                    "delay",
                ),
            )
            .ok()
    }
    let mut guard = inhibit(&proxy);
    owner.available.store(guard.is_some(), Ordering::Release);
    for message in &mut signals {
        if owner.stopped.load(Ordering::Acquire) {
            break;
        }
        let sleeping: bool = message.body().deserialize().map_err(|e| e.to_string())?;
        if sleeping {
            owner.notify(PowerEvent::Suspend, Duration::from_secs(4));
            guard.take();
        } else {
            owner.notify(PowerEvent::Resume, Duration::ZERO);
            guard = inhibit(&proxy);
            owner.available.store(guard.is_some(), Ordering::Release);
        }
    }
    Ok(())
}
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn native(_: Arc<PowerMonitor>) -> Result<(), String> {
    Err("OS power adapter unsupported; conservative clock-gap fallback required".into())
}
#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    };
    unsafe extern "system" fn proc(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if message == WM_NCCREATE {
            let create = unsafe { &*(l as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
        }
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const PowerMonitor;
        if message == WM_POWERBROADCAST && !ptr.is_null() {
            let owner = unsafe { &*ptr };
            match w {
                4 => owner.notify(PowerEvent::Suspend, Duration::from_millis(1800)),
                7 | 18 => owner.notify(PowerEvent::Resume, Duration::ZERO),
                _ => {}
            }
            return 1;
        }
        unsafe { DefWindowProcW(hwnd, message, w, l) }
    }
    pub fn run(owner: Arc<PowerMonitor>) -> Result<(), String> {
        let name = format!("ArtexNativePowerListener-{}\0", artex_core::new_id("power"))
            .encode_utf16()
            .collect::<Vec<_>>();
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                Arc::as_ptr(&owner) as _,
            );
            if hwnd.is_null() {
                UnregisterClassW(name.as_ptr(), instance);
                return Err(std::io::Error::last_os_error().to_string());
            }
            owner.thread.store(GetCurrentThreadId(), Ordering::Release);
            owner.window.store(hwnd as usize, Ordering::Release);
            owner.available.store(true, Ordering::Release);
            if !owner.stopped.load(Ordering::Acquire) {
                let mut message = MSG::default();
                while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            owner.window.store(0, Ordering::Release);
            owner.available.store(false, Ordering::Release);
            DestroyWindow(hwnd);
            UnregisterClassW(name.as_ptr(), instance);
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::ffi::c_void;
    type Port = *mut c_void;
    type Loop = *const c_void;
    struct Context {
        host: Arc<PowerMonitor>,
        connection: AtomicU32,
    }
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegisterForSystemPower(
            context: *mut c_void,
            port: *mut Port,
            callback: unsafe extern "C" fn(*mut c_void, u32, u32, *mut c_void),
            notifier: *mut u32,
        ) -> u32;
        fn IONotificationPortGetRunLoopSource(port: Port) -> *const c_void;
        fn IOAllowPowerChange(connection: u32, notification: isize) -> i32;
        fn IODeregisterForSystemPower(notifier: *mut u32) -> i32;
        fn IONotificationPortDestroy(port: Port);
        fn IOServiceClose(connection: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRunLoopGetCurrent() -> Loop;
        fn CFRunLoopAddSource(run_loop: Loop, source: *const c_void, mode: *const c_void);
        fn CFRunLoopRun();
        pub(super) fn CFRunLoopStop(run_loop: Loop);
        static kCFRunLoopDefaultMode: *const c_void;
    }
    unsafe extern "C" fn callback(
        context: *mut c_void,
        _service: u32,
        message: u32,
        arg: *mut c_void,
    ) {
        let context = unsafe { &*(context as *const Context) };
        match message {
            0xe0000270 => unsafe {
                IOAllowPowerChange(context.connection.load(Ordering::SeqCst), arg as isize);
            },
            0xe0000280 => {
                context
                    .host
                    .notify(PowerEvent::Suspend, Duration::from_secs(2));
                unsafe {
                    IOAllowPowerChange(context.connection.load(Ordering::SeqCst), arg as isize);
                }
            }
            0xe0000300 => {
                context.host.notify(PowerEvent::Resume, Duration::ZERO);
            }
            _ => {}
        }
    }
    pub fn run(host: Arc<PowerMonitor>) -> Result<(), String> {
        let mut context = Box::new(Context {
            host: host.clone(),
            connection: AtomicU32::new(0),
        });
        let mut port = std::ptr::null_mut();
        let mut notifier = 0;
        unsafe {
            let connection = IORegisterForSystemPower(
                (&mut *context) as *mut Context as *mut c_void,
                &mut port,
                callback,
                &mut notifier,
            );
            if connection == 0 {
                return Err("IORegisterForSystemPower failed".into());
            }
            context.connection.store(connection, Ordering::SeqCst);
            let loop_ref = CFRunLoopGetCurrent();
            host.run_loop.store(loop_ref as usize, Ordering::SeqCst);
            CFRunLoopAddSource(
                loop_ref,
                IONotificationPortGetRunLoopSource(port),
                kCFRunLoopDefaultMode,
            );
            host.available.store(true, Ordering::SeqCst);
            if !host.stopped.load(Ordering::Acquire) {
                CFRunLoopRun();
            }
            host.run_loop.store(0, Ordering::Release);
            host.available.store(false, Ordering::Release);
            IODeregisterForSystemPower(&mut notifier);
            IONotificationPortDestroy(port);
            IOServiceClose(connection);
        }
        Ok(())
    }
}
