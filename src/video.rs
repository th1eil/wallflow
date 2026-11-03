use std::path::Path;
use std::sync::mpsc::{self, Sender};
use crate::app_event::AppEvent;
use crate::playback::{LoopMode, PlaybackCommand};
use crate::contrast;
use objc2::msg_send;
use objc2::rc::{autoreleasepool, Allocated, Retained};
use objc2_av_foundation::{AVAsset, AVAssetImageGenerator};
use objc2_core_media::CMTime;
use objc2::runtime::NSObject;
use objc2_foundation::{NSString, NSURL, NSDictionary, NSSize};
use crate::util::find_class;

// ---------------------------------------------------------------------------
// Wrappers to safely move ObjC objects across thread boundaries.
// AVAssetImageGenerator is accessed exclusively by the worker thread.
// ---------------------------------------------------------------------------
// 持有强引用直到 worker 退出；工厂返回的 autoreleased 对象不能裸指针跨线程。
struct SendGenerator(Retained<AVAssetImageGenerator>);
unsafe impl Send for SendGenerator {}

pub struct VideoWallpaperPlayer {
    tx: Sender<AppEvent>,
    command_tx: Option<mpsc::Sender<PlaybackCommand>>,
}

impl VideoWallpaperPlayer {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        Self { tx, command_tx: None }
    }

    /// - `fps`: frame rate to extract at
    /// - `loop_mode`: how to loop (Infinite / PlayOnce / PingPong)
    /// - `screen_number`: which screen to target with SetDynamicFrame events
    /// - `band`: 亮度采样矩形（比例坐标，图像坐标系 y 向下），由
    ///   `ClockManager::sample_band()` 给出 —— 已限制在时钟范围内
    pub fn play(
        &mut self,
        path: &Path,
        fps: u32,
        loop_mode: LoopMode,
        screen_number: u32,
        band: (f64, f64, f64, f64),
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

            let asset: Retained<AVAsset> = msg_send![avurlasset_cls, URLAssetWithURL: &*url, options: None::<&NSDictionary>];
            let duration = get_duration(&*asset);

            if duration <= 0.0 {
                return Err(anyhow::anyhow!("Video has zero or unknown duration"));
            }

            let gen: Retained<AVAssetImageGenerator> = msg_send![generator_cls, assetImageGeneratorWithAsset: &*asset];
            let _: () = msg_send![&*gen, setMaximumSize: NSSize::new(1280.0, 720.0)];

            let send_gen = SendGenerator(gen);

            let (command_tx, command_rx) = mpsc::channel();
            self.command_tx = Some(command_tx);
            let tx = self.tx.clone();

            // Dedicated OS thread — avoids tokio's Send requirement on ObjC pointers.
            std::thread::spawn(move || {
                video_loop(send_gen, tx, command_rx, duration, frame_interval_ms, loop_mode, screen_number, band);
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
    band: (f64, f64, f64, f64),
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
    // 首帧亮度是否已经量过（视频只决策一次，见 Extract frame 处的注释）
    let mut sampled_luma = false;

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

        // 工作线程没有 AppKit 的自动释放池；逐帧清理编码产生的 NSData 等临时对象，
        // 避免内存随着播放时长持续增长。
        autoreleasepool(|_| unsafe {
            if let Some(png_bytes) = grab_frame(&*generator, current_time) {
                let target_path = if use_a { &path_a } else { &path_b };
                if std::fs::write(target_path, &png_bytes).is_ok() {
                    crate::util::send_event(&tx, AppEvent::SetDynamicFrame {
                        screen_number,
                        path: target_path.clone(),
                    });
                }
                use_a = !use_a;

                // 只按第一帧量一次亮度。视频画面一直变，若跟随重采样，时钟文字颜色
                // 会跟着画面频闪 —— 那比「对比度不够」更伤观感，所以与静态/GIF 路径
                // 保持同一套「首帧决策」约定。
                if !sampled_luma {
                    sampled_luma = true;
                    if let Some(luma) = measure_frame_luma(&png_bytes, band) {
                        crate::util::send_event(&tx, AppEvent::WallpaperLuma {
                            sequence_id: None,
                            screen_number,
                            luma,
                        });
                    }
                }
            }
        });

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
            // recv_timeout 会在命令到达时立即唤醒，无需每 50ms 空轮询。
            let poll_timeout = remaining;

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

/// 从 worker 刚生成的 PNG 帧里量一次时钟范围内的平均亮度。
///
/// 复用帧而不是另开一次视频解码：帧是 1280×720 的中间产物，解一次约十几毫秒，
/// 且发生在 worker 线程上，不占用主线程，也不影响出帧节奏。
/// 解码失败（帧数据异常）时返回 None，调用方保持上一次的采样值。
///
/// 注意帧的宽高比（16:9 上限）与屏幕未必一致，`band` 按比例换算时会带来
/// 一点偏差；但采样带紧贴屏幕中心的时钟区域，两种比例下该区域的相对位置
/// 差异很小。
fn measure_frame_luma(png_bytes: &[u8], band: (f64, f64, f64, f64)) -> Option<f64> {
    let img = image::load_from_memory(png_bytes).ok()?.to_rgba8();
    let (x0, y0, x1, y1) = band;
    Some(contrast::sample_region_luma(&img, x0, y0, x1, y1, contrast::SAMPLE_STEP))
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

/// 使用 CMTime 绑定读取时长，避免对 NSValue 发送并不存在的 bytes 消息。
unsafe fn get_duration(asset: &AVAsset) -> f64 {
    let time = asset.duration();
    if time.timescale <= 0 { return 0.0; }
    time.value as f64 / time.timescale as f64
}

/// Extract a frame as PNG bytes at `time_secs` from the generator.
unsafe fn grab_frame(generator: &AVAssetImageGenerator, time_secs: f64) -> Option<Vec<u8>> {
    // 原生绑定正确传递 CMTime 值，并自动释放 copy 方法返回的 CGImage，
    // 包括下面任何提前返回的失败路径。
    let time = CMTime::new((time_secs * 600.0) as i64, 600);
    let cg_image = generator.copyCGImageAtTime_actualTime_error(time, std::ptr::null_mut()).ok()?;

    // 直接从 CGImage 创建 bitmap，省掉原先 NSImage → TIFF → bitmap 的整帧往返。
    let bitmap_cls = find_class("NSBitmapImageRep").ok()?;
    let allocated: Allocated<NSObject> = msg_send![bitmap_cls, alloc];
    let bitmap: Option<Retained<NSObject>> = msg_send![allocated, initWithCGImage: &*cg_image];
    let bitmap = bitmap?;
    let png: *mut NSObject = msg_send![&*bitmap, representationUsingType: crate::util::NS_PNG_FILE_TYPE, properties: None::<&NSDictionary>];
    if png.is_null() { return None; }
    let len: usize = msg_send![&*png, length];
    // NSData.bytes 的 ABI 是 void*，u8* 会被 objc2 编码成 C 字符串并触发校验失败。
    let bytes: *const std::ffi::c_void = msg_send![&*png, bytes];
    if bytes.is_null() { return None; }
    Some(std::slice::from_raw_parts(bytes.cast::<u8>(), len).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 H.264 小样验证原生时间结构、CGImage 所有权及直接 bitmap 编码路径。
    /// 第一帧的 pool 释放后继续取第二帧，确保 generator 不依赖临时对象存活。
    #[test]
    fn native_video_frames_survive_autorelease_pool() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.mp4");
        std::fs::write(&path, include_bytes!("../tests/fixtures/video.mp4")).unwrap();
        let generator = autoreleasepool(|_| unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
            let asset = AVAsset::assetWithURL(&url);
            assert!((get_duration(&asset) - 1.0).abs() < 0.01);
            let class = find_class("AVAssetImageGenerator").unwrap();
            let generator: Retained<AVAssetImageGenerator> =
                msg_send![class, assetImageGeneratorWithAsset: &*asset];
            generator
        });
        for seconds in [0.0, 0.5] {
            let png = autoreleasepool(|_| unsafe { grab_frame(&generator, seconds) })
                .expect("native frame extraction should succeed");
            let frame = image::load_from_memory(&png).unwrap().to_rgb8();
            assert_eq!(frame.dimensions(), (64, 64));
            let pixel = frame.get_pixel(32, 32).0;
            assert!(pixel[0] > 200 && pixel[1] < 20 && pixel[2] < 20,
                "red video frame should keep its color: {:?}", pixel);
        }
    }
}
