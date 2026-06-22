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
mod util;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Receiver, Sender};
use util::TempFileGuard;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly, sel, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::NSObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSString};
use library::WallpaperLibrary;
use app_event::AppEvent;
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
                // Direct set — used by dynamic players for per-screen frame updates
                if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                    if let Err(e) = wallpaper::set_wallpaper(self.mtm, &target_screen, &path) {
                        eprintln!("Error setting wallpaper on screen {}: {:?}", screen_number, e);
                    }
                }
            }
            AppEvent::TogglePause => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get(&sn) {
                    screen_state.dynamic_player.send_command(playback::PlaybackCommand::Pause);
                    screen_state.video_player.send_command(playback::PlaybackCommand::Pause);
                }
            }
            AppEvent::SetLoopMode(mode) => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get(&sn) {
                    screen_state.dynamic_player.send_command(playback::PlaybackCommand::SetLoopMode(mode));
                    screen_state.video_player.send_command(playback::PlaybackCommand::SetLoopMode(mode));
                }
            }
            AppEvent::SetFps(fps) => {
                let sn = self.active_screen_number;
                if let Some(screen_state) = self.screens.get(&sn) {
                    screen_state.dynamic_player.send_command(playback::PlaybackCommand::SetFps(fps));
                    screen_state.video_player.send_command(playback::PlaybackCommand::SetFps(fps));
                }
            }
            AppEvent::ConfigReloaded(new_config) => {
                println!("Applying reloaded configuration: {:?}", new_config);

                // 1. Update mode
                let mode = match new_config.mode.as_str() {
                    "random" => library::SwitchMode::Random,
                    _ => library::SwitchMode::Sequential,
                };
                self.library.set_mode(mode);

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
                            let _ = tx.send(AppEvent::DirectoryScanned {
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
                unsafe {
                    let name = std::ffi::CStr::from_bytes_with_nul(b"NSOpenPanel\0").unwrap();
                    let panel_class = objc2::runtime::AnyClass::get(name);
                    if let Some(cls) = panel_class {
                        let panel: Retained<NSObject> = msg_send![cls, openPanel];
                        let _: () = msg_send![&panel, setCanChooseDirectories: true];
                        let _: () = msg_send![&panel, setCanChooseFiles: false];
                        let _: () = msg_send![&panel, setAllowsMultipleSelection: false];

                        let response: isize = msg_send![&panel, runModal];
                        if response == 1 {
                            let url: Option<Retained<NSObject>> = msg_send![&panel, URL];
                            if let Some(u) = url {
                                let path_ns: Retained<NSString> = msg_send![&u, path];
                                let new_path = path_ns.to_string();
                                println!("User selected wallpaper directory: {}", new_path);

                                // Write back to config.toml
                                if let Ok(mut current_config) = config::load_or_create_config() {
                                    current_config.wallpaper_dir = new_path;
                                    if let Ok(content) = toml::to_string_pretty(&current_config) {
                                        let config_path = config::get_config_path();
                                        let _ = std::fs::write(config_path, content);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            AppEvent::OpenWallpaperFolder => {
                if let Some(ref dir) = self.library.dir_path {
                    println!("Opening wallpaper folder in Finder: {:?}", dir);
                    let _ = std::process::Command::new("open")
                        .arg(dir)
                        .status();
                }
            }
            AppEvent::DownloadRandomWallpaper => {
                println!("Downloading random wallpaper from {}...", self.random_wallpaper_url);
                if let Some(ref dir) = self.library.dir_path {
                    let tx = self.event_tx.clone();
                    let dl_dir = dir.clone();
                    let url = self.random_wallpaper_url.clone();
                    tokio::spawn(async move {
                        match wallpaper::download_random_wallpaper(&dl_dir, &url) {
                            Ok(path) => {
                                println!("Downloaded: {:?}", path);
                                // Full rescan to pick up the new file
                                let wallpapers = library::collect_wallpapers(&dl_dir);
                                let _ = tx.send(AppEvent::DirectoryScanned {
                                    dir_path: dl_dir,
                                    wallpapers,
                                });
                            }
                            Err(e) => eprintln!("Failed to download wallpaper: {:?}", e),
                        }
                    });
                }
            }
            AppEvent::WallpaperProcessed { sequence_id, original_path, filtered_path, screen_number, ref info } => {
                if sequence_id == self.loading_sequence {
                    println!("Setting wallpaper on screen {} to: {:?}", screen_number, filtered_path);

                    // Find the target screen and set wallpaper only on it
                    if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                        if let Err(e) = wallpaper::set_wallpaper_with_fade(self.mtm, &target_screen, &filtered_path) {
                            eprintln!("Error setting wallpaper: {:?}", e);
                        }
                    }

                    // Per-screen cleanup: delete old temp file, store new one
                    if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                        if let Some(old_path) = screen_state.last_static_path.take() {
                            let _ = std::fs::remove_file(old_path);
                        }
                        screen_state.last_static_path = Some(filtered_path.clone());
                    }

                    // Update menu thumbnail (show last-set image regardless of screen)
                    if let Some(ref manager) = self.menu_manager {
                        let filename = original_path.file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or("Wallpaper");
                        manager.update_thumbnail(filename, Some(&filtered_path), Some(info));
                    }
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
        let is_temp_frame = path.file_name()
            .and_then(|f| f.to_str())
            .map(|s| s.starts_with("wallflow_frame_"))
            .unwrap_or(false);

        if is_temp_frame {
            // Temp frame from dynamic player — set directly on the target screen
            if let Some(target_screen) = screen::find_screen_by_number(self.mtm, screen_number) {
                if let Err(e) = wallpaper::set_wallpaper(self.mtm, &target_screen, path) {
                    eprintln!("Error setting wallpaper: {:?}", e);
                }
            }
            // Update menu thumbnail
            if let Some(ref manager) = self.menu_manager {
                let filename = path.file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or("Wallpaper");
                let info = WallpaperInfo::from_path(path);
                manager.update_thumbnail(filename, Some(path), Some(&info));
            }
            return;
        }

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
                if let Err(e) = screen_state.dynamic_player.play(
                    path,
                    playback::LoopMode::Infinite,
                    None, // native timing
                    screen_number,
                ) {
                    eprintln!("Failed to play GIF wallpaper: {:?}", e);
                }
            }
            return;
        }

        // Route APNG to per-screen dynamic player
        if ext == "png" && dynamic::is_apng(path) {
            println!("Playing APNG dynamic wallpaper on screen {}: {:?}", screen_number, path);
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                if let Err(e) = screen_state.dynamic_player.play(
                    path,
                    playback::LoopMode::Infinite,
                    None,
                    screen_number,
                ) {
                    eprintln!("Failed to play APNG wallpaper: {:?}", e);
                }
            }
            return;
        }

        // Route MP4/WebM to per-screen video player
        if ext == "mp4" || ext == "webm" {
            println!("Playing video wallpaper on screen {}: {:?}", screen_number, path);
            if let Some(screen_state) = self.screens.get_mut(&screen_number) {
                if let Err(e) = screen_state.video_player.play(
                    path,
                    24, // default 24 FPS
                    playback::LoopMode::Infinite,
                    screen_number,
                ) {
                    eprintln!("Failed to play video wallpaper: {:?}", e);
                }
            }
            return;
        }

        // Static wallpaper: process with GPU filter for the target screen
        self.loading_sequence += 1;
        let sequence_id = self.loading_sequence;
        let original_path = path.to_path_buf();
        let filters = self.screens.get(&screen_number)
            .map(|s| s.current_filters)
            .unwrap_or_default();
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
                    let _ = event_tx.send(AppEvent::WallpaperProcessed {
                        sequence_id,
                        original_path,
                        filtered_path,
                        screen_number,
                        info,
                    });
                }
                Ok(Err(e)) => {
                    eprintln!("Background wallpaper processing error: {:?}", e);
                }
                Err(e) => {
                    eprintln!("Background wallpaper task joined with error: {:?}", e);
                }
            }
        });
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
    state: std::sync::Mutex<Option<Arc<Mutex<AppState>>>>,
    event_rx: std::sync::Mutex<Option<Receiver<AppEvent>>>,
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
                state: std::sync::Mutex::new(None),
                event_rx: std::sync::Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(timerTick:))]
        unsafe fn timer_tick(&self, _timer: *mut NSObject) {
            let rx_lock = self.ivars().event_rx.lock().unwrap();
            let state_lock = self.ivars().state.lock().unwrap();
            if let (Some(ref rx), Some(ref state_arc)) = (&*rx_lock, &*state_lock) {
                while let Ok(event) = rx.try_recv() {
                    if let Ok(mut state) = state_arc.lock() {
                        state.handle_event(event);
                    }
                }
            }
        }
    }

    unsafe impl NSObjectProtocol for TimerTarget {}
);

impl TimerTarget {
    pub fn set_state_and_rx(&self, state: Arc<Mutex<AppState>>, event_rx: Receiver<AppEvent>) {
        *self.ivars().state.lock().unwrap() = Some(state);
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

    // Initialize state
    {
        let mut locked_state = state.lock().unwrap();
        locked_state.menu_manager = Some(menu_manager);

        // Apply config options
        let mode = match initial_config.mode.as_str() {
            "random" => library::SwitchMode::Random,
            _ => library::SwitchMode::Sequential,
        };
        locked_state.library.set_mode(mode);

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
                let _ = event_tx_clone.send(AppEvent::DirectoryScanned {
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

    // Schedule 150ms periodic event poll timer on the main run loop
    let _timer: Retained<NSObject> = unsafe {
        msg_send![
            objc2_foundation::NSTimer::class(),
            scheduledTimerWithTimeInterval: 0.150f64,
            target: &*timer_target,
            selector: sel!(timerTick:),
            userInfo: None::<&NSObject>,
            repeats: true
        ]
    };

    println!("WallFlow starting event loop...");
    app.run();
}
use app_event::WallpaperInfo;
use std::path::PathBuf;
