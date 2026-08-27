mod wallpaper;
mod library;
mod menubar;
mod scheduler;
mod dynamic;
mod color;
mod config;
mod settings;
mod screen;
mod playback;
mod video;
mod app_event;
mod notification;
mod util;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::mpsc::{channel, Receiver, Sender};
use util::TempFileGuard;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly, sel, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::NSObject;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDidChangeScreenParametersNotification,
    NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSObjectProtocol, NSURL, NSString};
use library::WallpaperLibrary;
use app_event::{AppEvent, WallpaperInfo};
use menubar::MenuBarManager;

pub struct AppState {
    // Global — shared across all screens
    pub library: WallpaperLibrary,
    pub mtm: MainThreadMarker,
    pub menu_manager: Option<MenuBarManager>,
    pub filter_pipeline: std::sync::Arc<color::FilterPipeline>,
    pub scheduler: scheduler::WallpaperScheduler,
    pub settings_panel: Option<settings::SettingsPanel>,
    pub loading_sequence: u64,
    pub event_tx: Sender<AppEvent>,
    /// 随机壁纸下载 URL，用户在设置中可自定义
    pub random_wallpaper_url: String,

    // Per-screen state — one entry per connected display
    pub screens: HashMap<u32, screen::ScreenState>,
    /// Currently selected screen for navigation (Prev/Next/Random/filters)
    pub active_screen_number: u32,
}

impl AppState {
    pub fn new(mtm: MainThreadMarker, event_tx: Sender<AppEvent>) -> Self {
        // Build per-screen state from currently connected displays
        let screens_map: HashMap<u32, screen::ScreenState> = wallpaper::get_screens(mtm)
            .iter()
            .map(|s| {
                let num = screen::screen_number(s);
                (num, screen::ScreenState::new(num, event_tx.clone()))
            })
            .collect();

        // Default to the first screen (or 0 if none)
        let active_num = screens_map.keys().next().copied().unwrap_or(0);

        Self {
            library: WallpaperLibrary::new(),
            mtm,
            menu_manager: None,
            filter_pipeline: std::sync::Arc::new(color::FilterPipeline::new()),
            scheduler: scheduler::WallpaperScheduler::new(event_tx.clone()),
            settings_panel: Some(settings::SettingsPanel::new(mtm, event_tx.clone())),
            loading_sequence: 0,
            event_tx: event_tx.clone(),
            random_wallpaper_url: config::Config::default().random_wallpaper_url,
            screens: screens_map,
            active_screen_number: active_num,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Previous => {
                if let Some(path) = self.library.previous() {
                    let sn = self.active_screen_number;
                    self.apply_wallpaper_on_screen(sn, &path);
                }
            }
            AppEvent::Next => {
                if let Some(path) = self.library.next() {
                    let sn = self.active_screen_number;
                    self.apply_wallpaper_on_screen(sn, &path);
                }
            }
            AppEvent::Random => {
                let old_mode = self.library.mode;
                self.library.set_mode(library::SwitchMode::Random);
                if let Some(path) = self.library.next() {
                    let sn = self.active_screen_number;
                    self.apply_wallpaper_on_screen(sn, &path);
                }
                self.library.set_mode(old_mode);
            }
            AppEvent::SelectActiveScreen(n) => {
                println!("Active screen changed to: {}", n);
                self.active_screen_number = n;
                // Update menu checkmark to reflect the newly active screen
                if let Some(ref manager) = self.menu_manager {
                    manager.set_active_screen(n);
                }
            }
            AppEvent::Settings => {
                println!("Settings clicked! Opening native settings panel.");
                if let Ok(current_config) = config::load_or_create_config() {
                    if let Some(ref panel) = self.settings_panel {
                        panel.show(&current_config);
                    }
                }
            }
            AppEvent::Exit => {
                println!("WallFlow shutting down...");
                // Stop all players before exit to avoid dangling ObjC threads
                for (_, screen_state) in &self.screens {
                    screen_state.dynamic_player.stop();
                    screen_state.video_player.stop();
                }
                self.scheduler.stop();
                let app = NSApplication::sharedApplication(self.mtm);
                app.terminate(None);
            }
            AppEvent::SetFilterPreset(preset) => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get_mut(&sn) {
                    screen_state.current_filters = preset.to_params();
                }
                // Mark the chosen preset in the menu
                if let Some(ref manager) = self.menu_manager {
                    manager.set_preset_tag(Some(preset.tag()));
                }
                // Re-apply current wallpaper with the new filter on the active screen
                if let Some(curr_idx) = self.library.current_index {
                    if curr_idx < self.library.wallpapers.len() {
                        let path = self.library.wallpapers[curr_idx].clone();
                        self.apply_wallpaper_on_screen(sn, &path);
                    }
                }
            }
            AppEvent::SetWallpaper(path) => {
                let sn = self.active_screen_number;
                self.apply_wallpaper_on_screen(sn, &path);
            }
            AppEvent::SetWallpaperOnScreen { screen_number, path } => {
                if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                    if let Err(e) = wallpaper::set_wallpaper(self.mtm, &target_screen, &path) {
                        eprintln!("Error setting wallpaper on screen {}: {:?}", screen_number, e);
                    }
                }
            }
            AppEvent::SetDynamicFrame { screen_number, path } => {
                // Direct frame update from dynamic/video player
                if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                    if let Err(e) = wallpaper::set_wallpaper(self.mtm, &target_screen, &path) {
                        eprintln!("Error setting wallpaper frame on screen {}: {:?}", screen_number, e);
                    }
                }
                // Update menu thumbnail
                if let Some(ref manager) = self.menu_manager {
                    let filename = path.file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or("Wallpaper");
                    let info = WallpaperInfo::from_path(&path);
                    manager.update_thumbnail(filename, Some(&path), Some(&info));
                }
            }
            AppEvent::TogglePause => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get_mut(&sn) {
                    // ScreenState owns the toggle; player commands are absolute
                    // Pause/Resume (the tasks treat them as set, not toggle).
                    screen_state.paused = !screen_state.paused;
                    let cmd = if screen_state.paused {
                        playback::PlaybackCommand::Pause
                    } else {
                        playback::PlaybackCommand::Resume
                    };
                    screen_state.dynamic_player.send_command(cmd.clone());
                    screen_state.video_player.send_command(cmd);
                }
                if let Some(ref manager) = self.menu_manager {
                    let paused = self.screens.get(&sn).map(|s| s.paused).unwrap_or(false);
                    manager.set_paused(paused);
                }
            }
            AppEvent::SetLoopMode(mode) => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get_mut(&sn) {
                    screen_state.loop_mode = mode;
                    screen_state.dynamic_player.send_command(playback::PlaybackCommand::SetLoopMode(mode));
                    screen_state.video_player.send_command(playback::PlaybackCommand::SetLoopMode(mode));
                }
                if let Some(ref manager) = self.menu_manager {
                    manager.set_loop_mode(mode);
                }
            }
            AppEvent::SetFps(fps) => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get_mut(&sn) {
                    screen_state.fps_override = fps;
                    screen_state.dynamic_player.send_command(playback::PlaybackCommand::SetFps(fps));
                    screen_state.video_player.send_command(playback::PlaybackCommand::SetFps(fps));
                }
                if let Some(ref manager) = self.menu_manager {
                    manager.set_fps(fps);
                }
            }
            AppEvent::ScreensChanged => {
                self.handle_screens_changed();
            }
            AppEvent::SetStatusLine(text) => {
                if let Some(ref manager) = self.menu_manager {
                    manager.show_status(text.as_deref());
                }
            }
            AppEvent::ConfigReloaded(new_config) => {
                println!("Applying reloaded configuration: {:?}", new_config);

                // 1. Update mode
                self.library.set_mode(new_config.mode);

                // 2.5 Update random wallpaper URL
                self.random_wallpaper_url = new_config.random_wallpaper_url.clone();

                // 2. Update filters on ALL screens
                let new_filters = color::FilterParams {
                    brightness: new_config.filters.brightness * 100.0,
                    contrast: new_config.filters.contrast * 100.0,
                    saturation: new_config.filters.saturation * 100.0,
                    temperature: new_config.filters.temperature * 100.0,
                    blur: new_config.filters.blur,
                };
                for screen_state in self.screens.values_mut() {
                    screen_state.current_filters = new_filters;
                }

                // Checkmark the matching preset, or clear when values are manual
                if let Some(ref manager) = self.menu_manager {
                    manager.set_preset_tag(color::preset_tag_for_params(&new_filters));
                }

                // 3. Update scheduler interval
                self.scheduler.start(new_config.interval_secs);

                // 4. Update wallpaper directory & scan if dir changed
                let dir_changed = self.library.dir_path.as_ref()
                    .map(|d| d.to_string_lossy() != new_config.wallpaper_dir)
                    .unwrap_or(true);

                if dir_changed {
                    let dir = std::path::PathBuf::from(&new_config.wallpaper_dir);
                    if dir.exists() {
                        let tx = self.event_tx.clone();
                        tokio::spawn(async move {
                            let scan_dir = dir.clone();
                            let wallpapers = tokio::task::spawn_blocking(move || {
                                library::collect_wallpapers(&scan_dir)
                            }).await.unwrap_or_default();
                            util::send_event(&tx, AppEvent::DirectoryScanned {
                                dir_path: dir,
                                wallpapers,
                            });
                        });
                    }
                } else {
                    // 5. Re-apply current wallpaper to reflect filter changes immediately
                    if let Some(curr_idx) = self.library.current_index {
                        if curr_idx < self.library.wallpapers.len() {
                            let path = self.library.wallpapers[curr_idx].clone();
                            let sn = self.active_screen_number;
                            self.apply_wallpaper_on_screen(sn, &path);
                        }
                    }
                }
            }
            AppEvent::ChooseDirectory => {
                println!("Opening directory picker...");
                let start_dir = self.library.dir_path.clone();
                if let Some(new_path) = util::run_open_directory_panel(
                    self.mtm,
                    "选择",
                    "选择包含壁纸的文件夹",
                    start_dir.as_deref(),
                ) {
                    println!("User selected wallpaper directory: {}", new_path.display());

                    // Write back to config.toml
                    if let Ok(mut current_config) = config::load_or_create_config() {
                        current_config.wallpaper_dir = new_path.to_string_lossy().to_string();
                        if let Ok(content) = toml::to_string_pretty(&current_config) {
                            let config_path = config::get_config_path();
                            let _ = std::fs::write(config_path, content);
                        }
                    }
                }
            }
            AppEvent::OpenWallpaperFolder => {
                if let Some(ref dir) = self.library.dir_path {
                    println!("Opening wallpaper folder in Finder: {:?}", dir);
                    // NSWorkspace is non-blocking; the old `open` subprocess
                    // waited for Finder to exit before returning.
                    let url = NSURL::fileURLWithPath(&NSString::from_str(
                        &dir.to_string_lossy(),
                    ));
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
            }
            AppEvent::DownloadRandomWallpaper => {
                println!("Downloading random wallpaper from {}...", self.random_wallpaper_url);
                if let Some(ref dir) = self.library.dir_path {
                    let tx = self.event_tx.clone();
                    let dl_dir = dir.clone();
                    let url = self.random_wallpaper_url.clone();
                    // Persistent "in flight" hint; replaced by the outcome below
                    util::send_event(&tx, AppEvent::SetStatusLine(Some("正在获取随机壁纸…".to_string())));
                    tokio::spawn(async move {
                        let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, Vec<PathBuf>, String)> {
                                let path = wallpaper::download_random_wallpaper(&dl_dir, &url)?;
                                let display_name = path.file_name()
                                    .and_then(|f| f.to_str())
                                    .unwrap_or("壁纸")
                                    .to_string();
                                println!("Downloaded: {:?}", path);
                                let wallpapers = library::collect_wallpapers(&dl_dir);
                                Ok((dl_dir, wallpapers, display_name))
                            }).await;

                        match res {
                            Ok(Ok((dir_path, wallpapers, name))) => {
                                util::send_event(&tx, AppEvent::DirectoryScanned {
                                    dir_path,
                                    wallpapers,
                                });
                                util::flash_status(&tx, format!("已保存 {}", name), 5);
                                notification::post("随机壁纸已保存", &name);
                            }
                            Ok(Err(e)) => {
                                eprintln!("Failed to download wallpaper: {:?}", e);
                                util::flash_status(&tx, "随机壁纸下载失败".to_string(), 6);
                                notification::post("随机壁纸下载失败", &format!("{:#}", e));
                            }
                            Err(e) => {
                                eprintln!("Download wallpaper task joined with error: {:?}", e);
                                util::flash_status(&tx, "随机壁纸下载失败".to_string(), 6);
                                notification::post("随机壁纸下载失败", "后台任务异常退出");
                            }
                        }
                    });
                } else {
                    util::flash_status(&self.event_tx, "尚未配置壁纸目录".to_string(), 4);
                }
            }
            AppEvent::WallpaperProcessed { sequence_id, original_path, filtered_path, screen_number, ref info } => {
                if sequence_id == self.loading_sequence {
                    println!("Setting wallpaper on screen {} to: {:?}", screen_number, filtered_path);

                    // Find the target screen and set wallpaper only on it
                    if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                        if let Err(e) = wallpaper::set_wallpaper_with_fade(self.mtm, &target_screen, &filtered_path) {
                            eprintln!("Error setting wallpaper: {:?}", e);
                            util::flash_status(&self.event_tx, "设置壁纸失败".to_string(), 5);
                        }
                    }

                    // Per-screen cleanup: delete old temp file, store new one
                    if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                        if let Some(old_path) = screen_state.last_static_path.take() {
                            let _ = std::fs::remove_file(old_path);
                        }
                        screen_state.last_static_path = Some(filtered_path.clone());
                        screen_state.content_kind = screen::ContentKind::Static;
                        screen_state.paused = false;
                    }

                    // Update menu thumbnail (show last-set image regardless of screen)
                    if let Some(ref manager) = self.menu_manager {
                        let filename = original_path.file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or("Wallpaper");
                        manager.update_thumbnail(filename, Some(&filtered_path), Some(info));
                    }
                    self.sync_menu_state();
                } else {
                    println!("Ignoring obsolete wallpaper process result for sequence {}", sequence_id);
                    let _ = std::fs::remove_file(filtered_path);
                }
            }
            AppEvent::DirectoryScanned { dir_path, wallpapers } => {
                println!("Scanned {} wallpapers from {:?}", wallpapers.len(), dir_path);
                self.library.dir_path = Some(dir_path);
                self.library.wallpapers = wallpapers;
                self.library.current_index = None;
                self.library.history_idx = None;
                if let Some(path) = self.library.next() {
                    let sn = self.active_screen_number;
                    self.apply_wallpaper_on_screen(sn, &path);
                }
            }
        }
    }

    /// Apply a wallpaper to a specific screen, identified by its NSScreenNumber.
    fn apply_wallpaper_on_screen(&mut self, screen_number: u32, path: &std::path::Path) {
        // Stop any active player on this screen before switching
        if let Some(screen_state) = self.screens.get_mut(&screen_number) {
            screen_state.dynamic_player.stop();
            screen_state.video_player.stop();
        }

        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        // Route GIF to per-screen dynamic player
        if ext == "gif" {
            println!("Playing dynamic wallpaper on screen {}: {:?}", screen_number, path);
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                screen_state.content_kind = screen::ContentKind::Dynamic;
                screen_state.paused = false;
                screen_state.loop_mode = playback::LoopMode::Infinite;
                screen_state.fps_override = 0;
                if let Err(e) = screen_state.dynamic_player.play(
                    path,
                    playback::LoopMode::Infinite,
                    None, // native timing
                    screen_number,
                ) {
                    eprintln!("Failed to play GIF wallpaper: {:?}", e);
                }
            }
            self.sync_menu_state();
            return;
        }

        // Route APNG to per-screen dynamic player
        if ext == "png" && dynamic::is_apng(path) {
            println!("Playing APNG dynamic wallpaper on screen {}: {:?}", screen_number, path);
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                screen_state.content_kind = screen::ContentKind::Dynamic;
                screen_state.paused = false;
                screen_state.loop_mode = playback::LoopMode::Infinite;
                screen_state.fps_override = 0;
                if let Err(e) = screen_state.dynamic_player.play(
                    path,
                    playback::LoopMode::Infinite,
                    None,
                    screen_number,
                ) {
                    eprintln!("Failed to play APNG wallpaper: {:?}", e);
                }
            }
            self.sync_menu_state();
            return;
        }

        // Route MP4/WebM to per-screen video player
        if ext == "mp4" || ext == "webm" {
            println!("Playing video wallpaper on screen {}: {:?}", screen_number, path);
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                screen_state.content_kind = screen::ContentKind::Video;
                screen_state.paused = false;
                screen_state.loop_mode = playback::LoopMode::Infinite;
                screen_state.fps_override = 24; // play() default
                if let Err(e) = screen_state.video_player.play(
                    path,
                    24, // default 24 FPS
                    playback::LoopMode::Infinite,
                    screen_number,
                ) {
                    eprintln!("Failed to play video wallpaper: {:?}", e);
                }
            }
            self.sync_menu_state();
            return;
        }

        let filters = self.screens.get(&screen_number)
            .map(|s| s.current_filters)
            .unwrap_or_default();

        // Optimization: if no filters are applied, set the wallpaper directly without GPU/CPU pipeline and temp files
        if filters.is_default() {
            println!("Setting raw wallpaper directly on screen {}: {:?}", screen_number, path);
            if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                if let Err(e) = wallpaper::set_wallpaper_with_fade(self.mtm, &target_screen, path) {
                    eprintln!("Error setting wallpaper: {:?}", e);
                }
            }

            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                if let Some(old_path) = screen_state.last_static_path.take() {
                    let _ = std::fs::remove_file(old_path);
                }
                screen_state.content_kind = screen::ContentKind::Static;
                screen_state.paused = false;
            }

            if let Some(ref manager) = self.menu_manager {
                let filename = path.file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("Wallpaper");
                let info = WallpaperInfo::from_path(path);
                manager.update_thumbnail(filename, Some(path), Some(&info));
            }
            self.sync_menu_state();
            return;
        }

        // Static wallpaper with filters: process with GPU/CPU filter for the target screen
        self.loading_sequence += 1;
        let sequence_id = self.loading_sequence;
        let original_path = path.to_path_buf();
        let filter_pipeline = self.filter_pipeline.clone();
        let event_tx = self.event_tx.clone();

        // Per-screen counter for unique temp filenames
        let static_counter = {
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                screen_state.static_counter += 1;
                screen_state.static_counter
            } else {
                1
            }
        };

        let original_path_for_thread = original_path.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, WallpaperInfo)> {
                // Use wallpaper::decode_image which falls back to macOS NSImage for HEIC/HEIF
                let rgba = wallpaper::decode_image(&original_path_for_thread)?;
                let info = WallpaperInfo::from_image(&original_path_for_thread, &rgba);
                let filtered_img = filter_pipeline.process_image(&rgba, &filters)?;
                let temp_dir = std::env::temp_dir();
                let filtered_path = temp_dir.join(format!(
                    "wallflow_filtered_{}_{}.png",
                    std::process::id(),
                    static_counter
                ));
                filtered_img.save(&filtered_path)?;
                // Wrap in guard so cancellation drops the file; keep() prevents deletion on success
                let guard = TempFileGuard::new(filtered_path);
                Ok((guard.keep(), info))
            }).await;

            match res {
                Ok(Ok((filtered_path, info))) => {
                    util::send_event(&event_tx, AppEvent::WallpaperProcessed {
                        sequence_id,
                        original_path,
                        filtered_path,
                        screen_number,
                        info,
                    });
                }
                Ok(Err(e)) => {
                    eprintln!("Background wallpaper processing error: {:?}", e);
                    util::send_event(&event_tx, AppEvent::SetStatusLine(Some("壁纸处理失败".to_string())));
                }
                Err(e) => {
                    eprintln!("Background wallpaper task joined with error: {:?}", e);
                    util::send_event(&event_tx, AppEvent::SetStatusLine(Some("壁纸处理失败".to_string())));
                }
            }
        });

        self.sync_menu_state();
    }

    /// Push playback-related UI state (pause title, checkmarks, control
    /// enablement) for the active screen into the menu.
    fn sync_menu_state(&self) {
        let Some(ref manager) = self.menu_manager else { return };
        let ss = self.screens.get(&self.active_screen_number);
        let is_playback = matches!(
            ss.map(|s| s.content_kind),
            Some(screen::ContentKind::Dynamic) | Some(screen::ContentKind::Video)
        );
        manager.set_playback_enabled(is_playback);
        manager.set_paused(ss.map(|s| s.paused).unwrap_or(false));
        manager.set_loop_mode(ss.map(|s| s.loop_mode).unwrap_or(playback::LoopMode::Infinite));
        manager.set_fps(ss.map(|s| s.fps_override).unwrap_or(0));
    }

    /// Reconcile per-screen state with the current display list and rebuild
    /// the 显示器 submenu. Fired by NSApplicationDidChangeScreenParameters.
    fn handle_screens_changed(&mut self) {
        let screens = wallpaper::get_screens(self.mtm);
        let mut entries: Vec<(u32, String)> = Vec::with_capacity(screens.len());
        for (i, s) in screens.iter().enumerate() {
            let num = screen::screen_number(s);
            entries.push((num, format!("显示器 {} ({})", i + 1, crate::screen::screen_resolution(s))));
        }
        let current: Vec<u32> = entries.iter().map(|(n, _)| *n).collect();

        // Tear down screens that disappeared
        let stale: Vec<u32> = self.screens
            .keys()
            .filter(|n| !current.contains(n))
            .copied()
            .collect();
        for num in stale {
            if let Some(ss) = self.screens.remove(&num) {
                ss.dynamic_player.stop();
                ss.video_player.stop();
                if let Some(p) = ss.last_static_path {
                    let _ = std::fs::remove_file(p);
                }
            }
        }

        // Create state for newly connected screens
        for num in &current {
            self.screens.entry(*num)
                .or_insert_with(|| screen::ScreenState::new(*num, self.event_tx.clone()));
        }

        // Fall back to a valid screen when the active one disconnected
        if !self.screens.contains_key(&self.active_screen_number) {
            self.active_screen_number = current.first().copied().unwrap_or(0);
        }

        let active = self.active_screen_number;
        if let Some(ref manager) = self.menu_manager {
            manager.rebuild_screen_menu(self.mtm, &entries, active);
            manager.set_active_screen(active);
        }
        self.sync_menu_state();
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        println!("Cleaning up temporary wallpaper files...");
        // Clean per-screen temp files
        for screen_state in self.screens.values() {
            if let Some(ref path) = screen_state.last_static_path {
                let _ = std::fs::remove_file(path);
            }
        }
        // Clean per-screen dynamic/video frame temp files
        let temp_dir = std::env::temp_dir();
        for screen_state in self.screens.values() {
            let sn = screen_state.screen_number;
            let _ = std::fs::remove_file(temp_dir.join(format!("wallflow_frame_{}_{}_a.png", std::process::id(), sn)));
            let _ = std::fs::remove_file(temp_dir.join(format!("wallflow_frame_{}_{}_b.png", std::process::id(), sn)));
        }
    }
}

pub struct TimerTargetIvars {
    state: OnceLock<Arc<Mutex<AppState>>>,
    event_rx: Mutex<Option<Receiver<AppEvent>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TimerTargetIvars]
    pub struct TimerTarget;

    impl TimerTarget {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(TimerTargetIvars {
                state: OnceLock::new(),
                event_rx: Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(timerTick:))]
        unsafe fn timer_tick(&self, _timer: *mut NSObject) {
            self.drain();
        }

        /// Selector used by util::wake_main_loop to drain immediately after a
        /// send from any thread. Invoked asynchronously on the main run loop.
        #[unsafe(method(drainEvents:))]
        unsafe fn drain_events(&self, _sender: *mut NSObject) {
            self.drain();
        }

        /// NSApplicationDidChangeScreenParametersNotification landed:
        /// displays were connected/disconnected or changed resolution.
        #[unsafe(method(screenParamsChanged:))]
        unsafe fn screen_params_changed(&self, _note: *mut NSObject) {
            if let Some(state_arc) = self.ivars().state.get() {
                if let Ok(mut state) = state_arc.lock() {
                    state.handle_event(AppEvent::ScreensChanged);
                }
            }
        }
    }

    unsafe impl NSObjectProtocol for TimerTarget {}
);

impl TimerTarget {
    fn drain(&self) {
        let rx_lock = self.ivars().event_rx.lock().unwrap();
        if let (Some(ref rx), Some(state_arc)) = (&*rx_lock, self.ivars().state.get()) {
            while let Ok(event) = rx.try_recv() {
                if let Ok(mut state) = state_arc.lock() {
                    state.handle_event(event);
                }
            }
        }
    }

    pub fn set_state_and_rx(&self, state: Arc<Mutex<AppState>>, event_rx: Receiver<AppEvent>) {
        let _ = self.ivars().state.set(state);
        *self.ivars().event_rx.lock().unwrap() = Some(event_rx);
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("Must run on main thread");

    // Initialize and enter Tokio runtime context so tokio::spawn/timers function correctly
    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    let _guard = rt.enter();

    let app = NSApplication::sharedApplication(mtm);

    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let (event_tx, event_rx) = channel();
    let state = Arc::new(Mutex::new(AppState::new(mtm, event_tx.clone())));

    // Load or create initial configuration
    let initial_config = match config::load_or_create_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config, using defaults: {:?}", e);
            config::Config::default()
        }
    };

    // Create status bar menu manager
    let menu_manager = MenuBarManager::new(mtm, event_tx.clone());

    // Create periodic timer receiver target
    let timer_target: Retained<TimerTarget> = unsafe { msg_send![TimerTarget::class(), new] };
    timer_target.set_state_and_rx(state.clone(), event_rx);

    // Event senders wake the drain immediately from any thread
    util::register_wake_target(Retained::as_ptr(&timer_target) as *mut NSObject);

    // Track display connect/disconnect/resolution changes
    unsafe {
        let center = NSNotificationCenter::defaultCenter();
        let observer_target: *mut NSObject = Retained::as_ptr(&timer_target) as *mut NSObject;
        center.addObserver_selector_name_object(
            &*observer_target,
            sel!(screenParamsChanged:),
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
        );
    }

    // Initialize state
    {
        let mut locked_state = state.lock().unwrap();
        locked_state.menu_manager = Some(menu_manager);

        // Apply config options
        locked_state.library.set_mode(initial_config.mode);

        let init_filters = color::FilterParams {
            brightness: initial_config.filters.brightness * 100.0,
            contrast: initial_config.filters.contrast * 100.0,
            saturation: initial_config.filters.saturation * 100.0,
            temperature: initial_config.filters.temperature * 100.0,
            blur: initial_config.filters.blur,
        };
        // Apply initial filters to all screens
        for screen_state in locked_state.screens.values_mut() {
            screen_state.current_filters = init_filters;
        }

        // Store random_wallpaper_url from config for DownloadRandomWallpaper event
        locked_state.random_wallpaper_url = initial_config.random_wallpaper_url.clone();

        // Start auto-switch scheduler
        locked_state.scheduler.start(initial_config.interval_secs);

        // Initial menu UI state: display submenu, preset checkmark, playback controls
        {
            let entries: Vec<(u32, String)> = wallpaper::get_screens(mtm)
                .iter()
                .enumerate()
                .map(|(i, s)| (
                    screen::screen_number(s),
                    format!("显示器 {} ({})", i + 1, screen::screen_resolution(s)),
                ))
                .collect();
            if let Some(ref manager) = locked_state.menu_manager {
                manager.rebuild_screen_menu(mtm, &entries, locked_state.active_screen_number);
                manager.set_active_screen(locked_state.active_screen_number);
                let init_filters_menu = color::FilterParams {
                    brightness: initial_config.filters.brightness * 100.0,
                    contrast: initial_config.filters.contrast * 100.0,
                    saturation: initial_config.filters.saturation * 100.0,
                    temperature: initial_config.filters.temperature * 100.0,
                    blur: initial_config.filters.blur,
                };
                manager.set_preset_tag(color::preset_tag_for_params(&init_filters_menu));
            }
            locked_state.sync_menu_state();
        }

        // Scan configured wallpaper directory
        let pictures_dir = std::path::PathBuf::from(&initial_config.wallpaper_dir);
        if pictures_dir.exists() {
            println!("Spawning background scan for: {:?}", pictures_dir);
            let event_tx_clone = event_tx.clone();
            let dir = pictures_dir.clone();
            tokio::spawn(async move {
                let scan_dir = dir.clone();
                let wallpapers = tokio::task::spawn_blocking(move || {
                    library::collect_wallpapers(&scan_dir)
                }).await.unwrap_or_default();
                util::send_event(&event_tx_clone, AppEvent::DirectoryScanned {
                    dir_path: dir,
                    wallpapers,
                });
            });
        }
    }

    // Start config hot-reload watcher and keep it alive in main scope
    let _watcher = match config::watch_config(event_tx) {
        Ok(w) => {
            println!("Configuration file hot-reload watcher active.");
            Some(w)
        }
        Err(e) => {
            eprintln!("Failed to start config hot-reload watcher: {:?}", e);
            None
        }
    };

    // Fallback poll timer on the main run loop. Normal latency is near-zero
    // because senders wake the drain directly (util::send_event); this only
    // covers missed wakes and runs rarely.
    let _timer: Retained<NSObject> = unsafe {
        msg_send![
            objc2_foundation::NSTimer::class(),
            scheduledTimerWithTimeInterval: 1.0f64,
            target: &*timer_target,
            selector: sel!(timerTick:),
            userInfo: None::<&NSObject>,
            repeats: true
        ]
    };

    println!("WallFlow starting event loop...");
    // Provisional notification rights for download results (no prompt)
    notification::request_authorization();
    app.run();
}
