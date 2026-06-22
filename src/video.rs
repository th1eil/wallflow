use std::path::Path;
use std::sync::mpsc::{self, Sender};
use crate::app_event::AppEvent;
use crate::playback::{LoopMode, PlaybackCommand};
use objc2::msg_send;
use objc2::runtime::NSObject;
use objc2_foundation::{NSString, NSURL, NSDictionary, NSSize};
use crate::util::find_class;

// ---------------------------------------------------------------------------
// Wrappers to safely move ObjC objects across thread boundaries.
// AVAssetImageGenerator is accessed exclusively by the worker thread.
// ---------------------------------------------------------------------------
struct SendGenerator(*mut NSObject);
unsafe impl Send for SendGenerator {}

pub struct VideoWallpaperPlayer {
    tx: Sender<AppEvent>,
    command_tx: Option<mpsc::Sender<PlaybackCommand>>,
}

impl VideoWallpaperPlayer {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        Self { tx, command_tx: None }
    }

    pub fn play(
        &mut self,
        path: &Path,
        fps: u32,
        loop_mode: LoopMode,
        screen_number: u32,
    ) -> anyhow::Result<()> {
        self.stop();

        let fps = fps.max(1).min(60);
        let frame_interval_ms = 1000u64 / fps as u64;

        let path_str = path.to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid video path"))?;

        unsafe {
            let path_ns = NSString::from_str(path_str);
            let url = NSURL::fileURLWithPath(&path_ns);

            let avurlasset_cls = find_class("AVURLAsset")?;
            let generator_cls = find_class("AVAssetImageGenerator")?;

            let asset: *mut NSObject = msg_send![avurlasset_cls, URLAssetWithURL: &*url, options: None::<&NSDictionary>];
            let duration = get_duration(&*asset);

            if duration <= 0.0 {
                return Err(anyhow::anyhow!("Video has zero or unknown duration"));
            }

            let gen: *mut NSObject = msg_send![generator_cls, assetImageGeneratorWithAsset: &*asset];
            let _: () = msg_send![&*gen, setMaximumSize: NSSize::new(1280.0, 720.0)];

            let send_gen = SendGenerator(gen);

            let (command_tx, command_rx) = mpsc::channel();
            self.command_tx = Some(command_tx);
            let tx = self.tx.clone();

            // Dedicated OS thread — avoids tokio's Send requirement on ObjC pointers.
            std::thread::spawn(move || {
                video_loop(send_gen, tx, command_rx, duration, frame_interval_ms, loop_mode, screen_number);
            });
        }

        Ok(())
    }

    pub fn send_command(&self, cmd: PlaybackCommand) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(cmd);
        }
    }

    pub fn stop(&self) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(PlaybackCommand::Stop);
        }
    }
}

// ---------------------------------------------------------------------------
// Video extraction loop (runs on dedicated std::thread)
// ---------------------------------------------------------------------------

fn video_loop(
    send_gen: SendGenerator,
    tx: Sender<AppEvent>,
    command_rx: mpsc::Receiver<PlaybackCommand>,
    duration_secs: f64,
    frame_interval_ms: u64,
    loop_mode: LoopMode,
    screen_number: u32,
) {
    let generator = send_gen.0;
    let temp_dir = std::env::temp_dir();
    // Per-screen temp files to avoid conflicts when multiple displays
    // play different video wallpapers simultaneously.
    let path_a = temp_dir.join(format!("wallflow_frame_{}_{}_a.png", std::process::id(), screen_number));
    let path_b = temp_dir.join(format!("wallflow_frame_{}_{}_b.png", std::process::id(), screen_number));
    let mut use_a = true;
    let mut current_time: f64 = 0.0;
    let mut paused = false;
    let mut current_loop = loop_mode;
    let mut frame_interval = frame_interval_ms;
    let mut direction: i32 = 1;

    // Use blocking recv with timeout for command polling
    loop {
        // Drain pending commands (non-blocking)
        while let Ok(cmd) = command_rx.try_recv() {
            if apply_video_command(cmd, &mut paused, &mut current_loop, &mut frame_interval) {
                cleanup(&path_a, &path_b);
                return;
            }
        }

        // Pause loop — block on command channel until Resume or Stop
        while paused {
            match command_rx.recv() {
                Ok(cmd) => {
                    if apply_video_command(cmd, &mut paused, &mut current_loop, &mut frame_interval) {
                        cleanup(&path_a, &path_b);
                        return;
                    }
                }
                Err(_) => {
                    cleanup(&path_a, &path_b);
                    return;
                }
            }
        }

        // Extract frame
        unsafe {
            if let Some(png_bytes) = grab_frame(&*generator, current_time) {
                let target_path = if use_a { &path_a } else { &path_b };
                if std::fs::write(target_path, &png_bytes).is_ok() {
                    let _ = tx.send(AppEvent::SetWallpaperOnScreen {
                        screen_number,
                        path: target_path.clone(),
                    });
                }
                use_a = !use_a;
            }
        }

        // Advance time
        let step = frame_interval as f64 / 1000.0;
        match current_loop {
            LoopMode::Infinite => {
                current_time += step;
                if current_time >= duration_secs { current_time = 0.0; }
            }
            LoopMode::PlayOnce => {
                current_time += step;
                if current_time >= duration_secs { break; }
            }
            LoopMode::PingPong => {
                current_time += direction as f64 * step;
                if current_time >= duration_secs {
                    direction = -1;
                    current_time = duration_secs - step;
                } else if current_time <= 0.0 {
                    direction = 1;
                    current_time = step;
                }
            }
        }

        // Sleep with command polling (blocking recv with timeout)
        let sleep_start = std::time::Instant::now();
        let target_sleep = std::time::Duration::from_millis(frame_interval);
        loop {
            let elapsed = sleep_start.elapsed();
            if elapsed >= target_sleep { break; }
            let remaining = target_sleep - elapsed;
            let poll_timeout = remaining.min(std::time::Duration::from_millis(50));

            match command_rx.recv_timeout(poll_timeout) {
                Ok(cmd) => {
                    if apply_video_command(cmd, &mut paused, &mut current_loop, &mut frame_interval) {
                        cleanup(&path_a, &path_b);
                        return;
                    }
                    if paused { break; } // exit sleep loop to enter pause loop
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    cleanup(&path_a, &path_b);
                    return;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Continue polling
                }
            }
        }
    }

    cleanup(&path_a, &path_b);
}

fn cleanup(a: &Path, b: &Path) {
    let _ = std::fs::remove_file(a);
    let _ = std::fs::remove_file(b);
}

fn apply_video_command(
    cmd: PlaybackCommand,
    paused: &mut bool,
    loop_mode: &mut LoopMode,
    frame_interval: &mut u64,
) -> bool {
    match cmd {
        PlaybackCommand::Pause => *paused = true,
        PlaybackCommand::Resume => *paused = false,
        PlaybackCommand::SetLoopMode(m) => *loop_mode = m,
        PlaybackCommand::SetFps(fps) => {
            let f = if fps == 0 { 24 } else { fps.max(1).min(60) };
            *frame_interval = 1000u64 / f as u64;
        }
        PlaybackCommand::Stop => return true,
    }
    false
}

// ---------------------------------------------------------------------------
// ObjC FFI helpers
// ---------------------------------------------------------------------------

/// Read asset duration in seconds via KVC to avoid CMTime struct FFI.
unsafe fn get_duration(asset: &NSObject) -> f64 {
    let key = NSString::from_str("duration");
    let nsvalue: *mut NSObject = msg_send![asset, valueForKey: &*key];
    if nsvalue.is_null() { return 60.0; }

    // NSValue wrapping CMTime {value:i64, timescale:i32, flags:u32, epoch:i64} = 24 bytes
    let raw: *const u8 = msg_send![&*nsvalue, bytes];
    if raw.is_null() { return 60.0; }

    let data = std::slice::from_raw_parts(raw, 24);
    let cm_value = i64::from_ne_bytes(data[0..8].try_into().unwrap());
    let cm_timescale = i32::from_ne_bytes(data[8..12].try_into().unwrap());

    if cm_timescale <= 0 { 60.0 }
    else { cm_value as f64 / cm_timescale as f64 }
}

/// Extract a frame as PNG bytes at `time_secs` from the generator.
unsafe fn grab_frame(generator: &NSObject, time_secs: f64) -> Option<Vec<u8>> {
    // Build CMTime NSValue: {value:i64, timescale:i32, flags:u32, epoch:i64}
    let value = (time_secs * 600.0) as i64;
    let timescale: i32 = 600;
    let mut cm_bytes = [0u8; 24];
    cm_bytes[0..8].copy_from_slice(&value.to_ne_bytes());
    cm_bytes[8..12].copy_from_slice(&timescale.to_ne_bytes());
    cm_bytes[12..16].copy_from_slice(&1i32.to_ne_bytes()); // kCMTimeFlags_Valid

    let type_str = std::ffi::CStr::from_bytes_with_nul(b"{?=qiiq}\0").unwrap();
    let nsvalue_cls = find_class("NSValue").ok()?;
    let cmtime: *mut NSObject = msg_send![nsvalue_cls, valueWithBytes: cm_bytes.as_ptr(), objCType: type_str.as_ptr()];

    // CGImageRef = [generator copyCGImageAtTime:cmtime actualTime:NULL error:NULL]
    let cg_image: *mut NSObject = msg_send![generator, copyCGImageAtTime: &*cmtime, actualTime: std::ptr::null_mut::<NSObject>(), error: std::ptr::null_mut::<NSObject>()];
    if cg_image.is_null() { return None; }

    // Convert CGImage → NSImage → TIFF → BitmapRep → PNG
    let nsimage_cls = find_class("NSImage").ok()?;
    let nsimage: *mut NSObject = msg_send![nsimage_cls, alloc];
    let nsimage: *mut NSObject = msg_send![nsimage, initWithCGImage: cg_image, size: NSSize::new(0.0, 0.0)];
    if nsimage.is_null() { return None; }

    let tiff: *mut NSObject = msg_send![&*nsimage, TIFFRepresentation];
    if tiff.is_null() { return None; }

    let bitmap_cls = find_class("NSBitmapImageRep").ok()?;
    let bitmap: *mut NSObject = msg_send![bitmap_cls, imageRepWithData: &*tiff];
    if bitmap.is_null() { return None; }

    // NSPNGFileType = 4
    let png: *mut NSObject = msg_send![&*bitmap, representationUsingType: 4u64, properties: None::<&NSDictionary>];
    if png.is_null() { return None; }

    let len: usize = msg_send![&*png, length];
    let bytes: *const u8 = msg_send![&*png, bytes];
    if bytes.is_null() { return None; }

    Some(std::slice::from_raw_parts(bytes, len).to_vec())
}
