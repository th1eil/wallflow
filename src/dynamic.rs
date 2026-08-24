use std::path::Path;
use std::sync::mpsc::Sender;
use tokio::sync::mpsc;
use crate::app_event::AppEvent;
use crate::playback::{LoopMode, PlaybackCommand};
use image::AnimationDecoder;

/// Check if a `.png` file is an animated PNG by scanning for the `acTL` chunk.
/// The acTL chunk must appear before the first IDAT chunk per the APNG spec.
/// Reads only the initial portion of the file (up to 4KB) rather than the entire file.
pub fn is_apng(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    let mut data = [0u8; 4096];
    let Ok(bytes_read) = file.read(&mut data) else { return false };
    if bytes_read < 33 { return false; }
    
    let slice = &data[..bytes_read];
    let mut pos: usize = 8;
    while pos + 12 <= slice.len() {
        let chunk_type = &slice[pos + 4..pos + 8];
        if chunk_type == b"acTL" {
            return true;
        }
        // acTL must appear before IDAT — if we hit IDAT first, it's a static PNG
        if chunk_type == b"IDAT" {
            return false;
        }
        let length = u32::from_be_bytes([slice[pos], slice[pos+1], slice[pos+2], slice[pos+3]]) as usize;
        pos = pos.saturating_add(12 + length); // len(4) + type(4) + data + crc(4)
    }
    false
}

pub struct DynamicWallpaperPlayer {
    tx: Sender<AppEvent>,
    /// Channel to send commands to the running playback task.
    /// Replace the old oneshot cancel channel with a richer mpsc channel.
    command_tx: Option<mpsc::UnboundedSender<PlaybackCommand>>,
}

impl DynamicWallpaperPlayer {
    pub fn new(tx: Sender<AppEvent>) -> Self {
        Self { tx, command_tx: None }
    }

    /// Play a dynamic wallpaper (GIF or APNG) in a background Tokio task.
    ///
    /// - `loop_mode`: how to loop (Infinite / PlayOnce / PingPong)
    /// - `fps_override`: if Some, override per-frame delay; None = use native timing
    /// - `screen_number`: which screen to target with SetWallpaperOnScreen events
    pub fn play(
        &mut self,
        path: &Path,
        loop_mode: LoopMode,
        fps_override: Option<u32>,
        screen_number: u32,
    ) -> anyhow::Result<()> {
        self.stop();

        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let is_apng = ext == "png" && is_apng(path);

        if ext != "gif" && !is_apng {
            return Err(anyhow::anyhow!("Unsupported dynamic format: {}", ext));
        }

        let file = std::fs::File::open(path)?;

        // Helper: extract (image, delay_ms) pairs from an AnimationDecoder
        fn extract_frames(
            frames: Vec<image::Frame>,
            format_name: &str,
        ) -> anyhow::Result<Vec<(image::RgbaImage, u64)>> {
            if frames.is_empty() {
                return Err(anyhow::anyhow!("{} contains no frames", format_name));
            }
            let mut play_frames = Vec::new();
            for frame in frames {
            let (num, denom) = frame.delay().numer_denom_ms();
            let delay_ms = if denom == 0 {
                100
            } else {
                 // numer_denom_ms() returns delay as rational seconds: (100,1000) = 0.1s = 100ms
                // In image 0.24.9 numer_denom_ms returns (u32, u32), so cast to u64
                ((num as u64 * 1000 + denom as u64 / 2) / denom as u64).max(1)
            };
                let image = frame.into_buffer();
                play_frames.push((image, delay_ms));
            }
            Ok(play_frames)
        }

        let play_frames = if is_apng {
            let png_decoder = image::codecs::png::PngDecoder::new(file)?;
            let decoder = png_decoder.apng();
            let frames = decoder.into_frames().collect_frames()?;
            extract_frames(frames, "APNG")?
        } else {
            let decoder = image::codecs::gif::GifDecoder::new(file)?;
            let frames = decoder.into_frames().collect_frames()?;
            extract_frames(frames, "GIF")?
        };

        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        self.command_tx = Some(command_tx);
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let temp_dir = std::env::temp_dir();
            // Per-screen temp files to avoid conflicts when multiple displays
            // play different dynamic wallpapers simultaneously.
            let path_a = temp_dir.join(format!("wallflow_frame_{}_{}_a.png", std::process::id(), screen_number));
            let path_b = temp_dir.join(format!("wallflow_frame_{}_{}_b.png", std::process::id(), screen_number));

            let mut use_a = true;
            let mut idx: usize = 0;
            let mut direction: i32 = 1; // 1 = forward, -1 = backward (PingPong)
            let mut paused = false;
            let mut current_loop = loop_mode;
            let mut frame_delay_ms: Option<u64> = fps_override.map(|fps| {
                if fps == 0 { 100 } else { (1000u64 / fps as u64).max(1) }
            });

            let len = play_frames.len();
            if len == 0 { return; }

            loop {
                // --- Process pending commands first ---
                while let Ok(cmd) = command_rx.try_recv() {
                    match cmd {
                        PlaybackCommand::Pause => paused = true,
                        PlaybackCommand::Resume => paused = false,
                        PlaybackCommand::SetLoopMode(m) => current_loop = m,
                        PlaybackCommand::SetFps(fps) => {
                            frame_delay_ms = if fps == 0 {
                                None // back to native timing
                            } else {
                                Some((1000u64 / fps as u64).max(1))
                            };
                        }
                        PlaybackCommand::Stop => {
                            let _ = std::fs::remove_file(path_a);
                            let _ = std::fs::remove_file(path_b);
                            return;
                        }
                    }
                }

                // --- Pause: sleep and poll for commands ---
                if paused {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                        cmd = command_rx.recv() => {
                            if let Some(cmd) = cmd {
                                match cmd {
                                    PlaybackCommand::Resume => paused = false,
                                    PlaybackCommand::Stop => {
                                        let _ = std::fs::remove_file(path_a);
                                        let _ = std::fs::remove_file(path_b);
                                        return;
                                    }
                                    PlaybackCommand::SetLoopMode(m) => current_loop = m,
                                    PlaybackCommand::SetFps(fps) => {
                                        frame_delay_ms = if fps == 0 { None }
                                        else { Some((1000u64 / fps as u64).max(1)) };
                                    }
                                    _ => {} // Pause while already paused → ignored
                                }
                            } else {
                                // channel closed → stop
                                let _ = std::fs::remove_file(path_a);
                                let _ = std::fs::remove_file(path_b);
                                return;
                            }
                        }
                    }
                    continue;
                }

                // --- Save & send current frame ---
                let (ref img, native_delay) = play_frames[idx];
                let target_path = if use_a { &path_a } else { &path_b };

                if let Err(e) = img.save(target_path) {
                    eprintln!("Failed to save dynamic frame: {:?}", e);
                } else {
                    let _ = tx.send(AppEvent::SetDynamicFrame {
                        screen_number,
                        path: target_path.clone(),
                    });
                }

                use_a = !use_a;

                // --- Advance frame index ---
                match current_loop {
                    LoopMode::Infinite => {
                        idx = (idx + 1) % len;
                    }
                    LoopMode::PlayOnce => {
                        idx += 1;
                        if idx >= len {
                            // Finished one full playthrough — stop
                            break;
                        }
                    }
                    LoopMode::PingPong => {
                        let next = idx as i32 + direction;
                        if next < 0 {
                            direction = 1;
                            idx = 1;
                        } else if next >= len as i32 {
                            direction = -1;
                            idx = len.saturating_sub(2);
                        } else {
                            idx = next as usize;
                        }
                    }
                }

                // --- Wait for next frame time or command ---
                let wait_ms = frame_delay_ms.unwrap_or(native_delay).max(1);

                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => {}
                    cmd = command_rx.recv() => {
                        if let Some(cmd) = cmd {
                            match cmd {
                                PlaybackCommand::Pause => paused = true,
                                PlaybackCommand::Stop => {
                                    let _ = std::fs::remove_file(path_a);
                                    let _ = std::fs::remove_file(path_b);
                                    return;
                                }
                                PlaybackCommand::SetLoopMode(m) => current_loop = m,
                                PlaybackCommand::SetFps(fps) => {
                                    frame_delay_ms = if fps == 0 { None }
                                    else { Some((1000u64 / fps as u64).max(1)) };
                                }
                                _ => {}
                            }
                        } else {
                            // channel closed → stop
                            let _ = std::fs::remove_file(path_a);
                            let _ = std::fs::remove_file(path_b);
                            return;
                        }
                    }
                }
            }

            // Cleanup temp frames on exit
            let _ = std::fs::remove_file(path_a);
            let _ = std::fs::remove_file(path_b);
        });

        Ok(())
    }

    /// Send a playback command to the currently running task.
    pub fn send_command(&self, cmd: PlaybackCommand) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(cmd);
        }
    }

    /// Stop playing the dynamic wallpaper.
    pub fn stop(&self) {
        if let Some(ref tx) = self.command_tx {
            let _ = tx.send(PlaybackCommand::Stop);
        }
    }
}
