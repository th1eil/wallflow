use std::sync::mpsc::Sender;
use objc2_foundation::MainThreadMarker;
use crate::app_event::AppEvent;

/// NSPNGFileType constant from NSBitmapImageRep.h
pub const NS_PNG_FILE_TYPE: u64 = 4;

// ---------------------------------------------------------------------------
// Main-loop wake bridge
//
// AppEvents flow through a std::sync::mpsc channel that TimerTarget drains on
// the main run loop. Relying only on the fallback poll timer adds up to one
// tick of latency to every interaction, so main() registers the drain target
// here and every sender wakes the run loop immediately after enqueuing.
// ---------------------------------------------------------------------------

static WAKE_TARGET: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Register the ObjC object implementing `drainEvents:` (the TimerTarget).
/// The pointer stays valid for the whole process lifetime — main() holds it.
pub fn register_wake_target(target: *mut objc2::runtime::NSObject) {
    let _ = WAKE_TARGET.set(target as usize);
}

/// Wake the main run loop so queued events are drained right away.
/// Safe from any thread: `performSelectorOnMainThread:` schedules an
/// asynchronous call (no reentrancy with an in-flight drain).
fn wake_main_loop() {
    if let Some(addr) = WAKE_TARGET.get().copied() {
        let target = addr as *mut objc2::runtime::NSObject;
        unsafe {
            let _: () = objc2::msg_send![target,
                performSelectorOnMainThread: objc2::sel!(drainEvents:),
                withObject: std::ptr::null_mut::<objc2::runtime::NSObject>(),
                waitUntilDone: false
            ];
        }
    }
}

/// Send an event through the app channel and wake the main loop immediately.
/// Use this everywhere instead of a bare `tx.send(...)`.
pub fn send_event(tx: &Sender<AppEvent>, event: AppEvent) {
    let _ = tx.send(event);
    wake_main_loop();
}

/// Show a transient message on the menu status line; auto-clears after `secs`.
/// Safe from any thread.
pub fn flash_status(tx: &Sender<AppEvent>, text: String, secs: u64) {
    send_event(tx, AppEvent::SetStatusLine(Some(text)));
    let tx2 = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
        send_event(&tx2, AppEvent::SetStatusLine(None));
    });
}

// Timestamp (ms since epoch) of the last config.toml write performed by the
// app itself. Lets the hot-reload watcher skip the echo of our own write,
// which used to re-trigger ConfigReloaded and double the work.
static CONFIG_SELF_WRITE_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Call immediately before the app writes config.toml itself.
pub fn mark_config_self_write() {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    CONFIG_SELF_WRITE_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

/// True when a self-write happened within this window — external watcher
/// callbacks arriving this soon are almost certainly our own echo.
pub fn was_recently_self_written() -> bool {
    const ECHO_WINDOW_MS: i64 = 800;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let last = CONFIG_SELF_WRITE_MS.load(std::sync::atomic::Ordering::Relaxed);
    last > 0 && now - last < ECHO_WINDOW_MS
}

/// Present a modal directory chooser. Must be called on the main thread.
/// Returns the selected path, or None if cancelled.
pub fn run_open_directory_panel(
    mtm: MainThreadMarker,
    prompt: &str,
    message: &str,
    start_dir: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    use objc2_app_kit::NSOpenPanel;
    use objc2_foundation::{NSURL, NSString};

    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(true);
    panel.setCanChooseFiles(false);
    panel.setAllowsMultipleSelection(false);
    panel.setPrompt(Some(&NSString::from_str(prompt)));
    panel.setMessage(Some(&NSString::from_str(message)));
    if let Some(dir) = start_dir {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
            &dir.to_string_lossy(),
        ))));
    }

    if panel.runModal() == 1 {
        let url = panel.URL()?;
        let path_ns = url.path()?;
        Some(std::path::PathBuf::from(path_ns.to_string()))
    } else {
        None
    }
}

/// Wraps a temporary file path; deletes the file on Drop unless `keep()` is called.
/// Prevents temp file leaks when spawn_blocking is cancelled mid-execution.
pub struct TempFileGuard(Option<std::path::PathBuf>);

impl TempFileGuard {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }

    /// Consume the guard and return the path without deleting the file.
    pub fn keep(mut self) -> std::path::PathBuf {
        self.0.take().unwrap()
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(ref path) = self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Find an Objective-C class by name. Thread-safe (class objects are global).
pub unsafe fn find_class(name: &str) -> anyhow::Result<&'static objc2::runtime::AnyClass> {
    let cname = std::ffi::CString::new(name).unwrap();
    objc2::runtime::AnyClass::get(cname.as_c_str())
        .ok_or_else(|| anyhow::anyhow!("Objective-C class '{}' not found", name))
}