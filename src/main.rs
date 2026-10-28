mod wallpaper;
mod library;
mod menubar;
mod scheduler;
mod dynamic;
mod color;
mod config;
mod settings;
mod settings_ui;
mod screen;
mod playback;
mod video;
mod app_event;
mod notification;
mod util;
mod clock;
mod contrast;
mod lunar;
mod notch;

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
    /// Last configuration actually applied — lets repeated ConfigReloaded
    /// events (设置面板 Apply 与 watcher 回声之外的重复) act as no-ops.
    pub last_applied: Option<config::Config>,
    /// 壁纸中央时钟叠加层（每块屏幕一个窗口）
    pub clock_manager: clock::ClockManager,
    /// 刘海黑色矩阵配置。它不是叠加层，而是**画进壁纸图片**的装饰：
    /// 任何盖住菜单栏的窗口都会挡住菜单标题与状态图标，画进图片才能两者兼顾。
    pub notch: config::NotchConfig,
    /// 滑块预览的代数：每次 PreviewFilters 自增，延迟提交只在代数仍匹配时生效。
    /// 拖动期间事件按帧涌入，靠它把「一串」收敛成「停手后一次」。
    pub preview_generation: u64,
    /// 最近一次登记、尚未提交的预览参数
    pub preview_pending: Option<color::FilterParams>,

    // Per-screen state — one entry per connected display
    pub screens: HashMap<u32, screen::ScreenState>,
    /// Currently selected screen for navigation (Prev/Next/Random/filters)
    pub active_screen_number: u32,
}

impl AppState {
    pub fn new(mtm: MainThreadMarker, event_tx: Sender<AppEvent>) -> Self {
        // 清掉上一轮遗留的滤镜产物，只留系统当前引用的最新一张。
        // 放在启动阶段做：目录里通常只有 1～3 个文件，成本可忽略。
        wallpaper::cleanup_stale_cache();

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

        let settings_panel = settings::SettingsPanel::new(mtm, event_tx.clone());
        // 调试开关：WF_DEBUG_SHOW=1 时启动即打开设置面板，便于布局回归验证。
        // 正常启动不受影响。
        if std::env::var_os("WF_DEBUG_SHOW").is_some() {
            if let Ok(cfg) = config::load_or_create_config() {
                settings_panel.show(&cfg);
            }
        }

        Self {
            library: WallpaperLibrary::new(),
            mtm,
            menu_manager: None,
            filter_pipeline: std::sync::Arc::new(color::FilterPipeline::new()),
            scheduler: scheduler::WallpaperScheduler::new(event_tx.clone()),
            settings_panel: Some(settings_panel),
            loading_sequence: 0,
            event_tx: event_tx.clone(),
            random_wallpaper_url: config::Config::default().random_wallpaper_url,
            last_applied: None,
            clock_manager: clock::ClockManager::new(config::ClockConfig::default()),
            notch: config::NotchConfig::default(),
            preview_generation: 0,
            preview_pending: None,
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
            AppEvent::PreviewFilters(params) => {
                let sn = self.active_screen_number;
                // Filters only render into static images — a running GIF/APNG/
                // video would just be restarted for nothing, so skip previews.
                if let Some(ss) = self.screens.get(&sn) {
                    if matches!(
                        ss.content_kind,
                        screen::ContentKind::Dynamic | screen::ContentKind::Video
                    ) {
                        return;
                    }
                }
                // 防抖：滑块每次移动都发一个事件，而一次预览是「解码 26MP →
                // 滤镜 → 编码」的完整链路（秒级）。拖动时逐帧跑会把工作线程塞满，
                // 手感变成「拖完还要等」。这里只登记参数并推迟 180ms 提交，
                // 期间再到的事件只推进代数，旧定时器的回调会因代数不匹配自行作废。
                self.preview_generation += 1;
                let generation = self.preview_generation;
                self.preview_pending = Some(params);
                let tx = self.event_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(180)).await;
                    util::send_event(&tx, AppEvent::CommitPreviewFilters(generation));
                });
            }
            AppEvent::CommitPreviewFilters(generation) => {
                // 不是最新一次拖动的回调就丢弃；真正的最新参数由更晚的定时器负责
                if generation != self.preview_generation {
                    return;
                }
                let Some(params) = self.preview_pending.take() else {
                    return;
                };
                let sn = self.active_screen_number;
                if let Some(ss) = self.screens.get_mut(&sn) {
                    ss.current_filters = params;
                }
                if let Some(ref manager) = self.menu_manager {
                    manager.set_preset_tag(color::preset_tag_for_params(&params));
                }
                if let Some(curr_idx) = self.library.current_index {
                    if curr_idx < self.library.wallpapers.len() {
                        let path = self.library.wallpapers[curr_idx].clone();
                        self.apply_wallpaper_on_screen(sn, &path);
                    }
                }
            }
            AppEvent::PreviewClock(clock) => {
                // 只重建时钟叠加层，不碰壁纸/滤镜/调度。apply_config 会比对现有
                // 配置，只有真的变了的项才触发重建，拖不到变化时不产生闪烁。
                self.clock_manager.apply_config(self.mtm, clock);
            }
            AppEvent::PreviewNotch(notch) => {
                // 刘海矩阵画在壁纸里，改参数只能重渲染当前壁纸。
                // 设置面板对这个事件做了 0.4s 防抖，拖圆角滑块只会在停手后渲染一次。
                self.notch = notch;
                let sn = self.active_screen_number;
                if let Some(idx) = self.library.current_index {
                    if idx < self.library.wallpapers.len() {
                        let path = self.library.wallpapers[idx].clone();
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
            AppEvent::ToggleClock => {
                // 与设置面板 Apply / 选目录同一套写法：标记自我写入 → 写盘 → 直接派发
                // ConfigReloaded。这样时钟重建、菜单勾选、以及 last_applied 的同步
                // 全部走同一条配置应用链路，不再有第二份「自己 apply」的分支。
                if let Ok(mut cfg) = config::load_or_create_config() {
                    cfg.clock.enabled = !cfg.clock.enabled;
                    util::mark_config_self_write();
                    if let Ok(content) = toml::to_string_pretty(&cfg) {
                        if std::fs::write(config::get_config_path(), content).is_ok() {
                            util::send_event(&self.event_tx, AppEvent::ConfigReloaded(cfg));
                        }
                    }
                }
            }
            AppEvent::ToggleAutoContrast => {
                // 与 ToggleClock 同一条链路：标记自我写入 → 写盘 → 直接派发
                // ConfigReloaded。这样时钟重建（apply_config 会比对 ClockConfig，
                // auto_contrast 变化即触发重建）、菜单勾选、last_applied 同步
                // 全部走同一套配置应用逻辑，不留第二份「自己 apply」的分支。
                if let Ok(mut cfg) = config::load_or_create_config() {
                    cfg.clock.auto_contrast = !cfg.clock.auto_contrast;
                    util::mark_config_self_write();
                    if let Ok(content) = toml::to_string_pretty(&cfg) {
                        if std::fs::write(config::get_config_path(), content).is_ok() {
                            util::send_event(&self.event_tx, AppEvent::ConfigReloaded(cfg));
                        }
                    }
                }
            }
            AppEvent::SetStatusLine(text) => {
                if let Some(ref manager) = self.menu_manager {
                    manager.show_status(text.as_deref());
                }
            }
            AppEvent::WallpaperLuma { screen_number, luma } => {
                // 采样（解码 + 采样）已经在后台线程做完，这里只做颜色决策与贴色：
                // 「换壁纸 → 自动改对比色」的全部主线程成本就是三次 setTextColor。
                self.clock_manager.set_background_luma(screen_number, luma);
            }
            AppEvent::ConfigReloaded(new_config) => {
                println!("Applying reloaded configuration: {:?}", new_config);

                // Idempotence guard: a config identical to the last applied one
                // (e.g. Apply with untouched values, watcher echo) must not
                // rescan / replay the wallpaper / restart the scheduler.
                if self.last_applied.as_ref() == Some(&new_config) {
                    return;
                }
                let prev = std::mem::replace(&mut self.last_applied, Some(new_config.clone()));

                // 除时钟外的字段是否完全没变。菜单里的「显示时钟」开关走的就是
                // 这条链路（写盘 + ConfigReloaded），若不做区分，下面第 5 步会把
                // 当前壁纸重新渲染并淡入一次 —— 开关时钟不该有视觉闪烁。
                let clock_only = matches!(&prev, Some(p)
                    if p.wallpaper_dir == new_config.wallpaper_dir
                        && p.filters == new_config.filters
                        && p.interval_secs == new_config.interval_secs
                        && p.mode == new_config.mode
                        && p.random_wallpaper_url == new_config.random_wallpaper_url
                        // 刘海矩阵画在壁纸里：参数变了必须重渲染当前壁纸，
                        // 所以它不算「仅时钟变化」，不能走提前返回。
                        && p.notch == new_config.notch);

                // 1. Update mode
                self.library.set_mode(new_config.mode);

                // 2.5 Update random wallpaper URL
                self.random_wallpaper_url = new_config.random_wallpaper_url.clone();

                // 2.6 时钟叠加层：配置变化时重建，菜单勾选状态同步
                // 对齐方式变了 → 采样区域跟着变（靠右看文字矩形、居中看整张图），
                // 旧亮度不再代表新区域，先清掉让它回退到中性值，等下次换壁纸重采。
                let align_changed = prev
                    .as_ref()
                    .map(|p| p.clock.text_align != new_config.clock.text_align)
                    .unwrap_or(false);
                if align_changed {
                    self.clock_manager.clear_background_luma();
                }
                self.clock_manager.apply_config(self.mtm, new_config.clock.clone());
                if let Some(ref manager) = self.menu_manager {
                    manager.set_clock_enabled(new_config.clock.enabled);
                    manager.set_auto_contrast(new_config.clock.auto_contrast);
                }

                // 2.65 刘海黑色矩阵：它不是叠加层，而是画进壁纸的，改参数要重渲染
                // 当前壁纸。这里只同步配置值，真正的重渲染交给下方统一的
                // 「非 clock_only 就重放一次壁纸」路径，避免写第二份应用逻辑。
                self.notch = new_config.notch.clone();

                // 2.7 仅时钟变化：时钟已重建完毕，壁纸/滤镜/调度均未变，
                // 提前返回以免重放一次淡入。
                if clock_only {
                    return;
                }

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

                // 3. Scheduler: Manual mode means "I switch myself" — the old
                // code kept firing Next regardless. Stopped entirely when
                // manual or disabled; restarted only when timing actually changed.
                if new_config.interval_secs == 0
                    || matches!(new_config.mode, library::SwitchMode::Manual)
                {
                    self.scheduler.stop();
                } else {
                    let same_period = matches!(&prev, Some(p)
                        if p.interval_secs == new_config.interval_secs
                            && !matches!(p.mode, library::SwitchMode::Manual));
                    if !same_period {
                        self.scheduler.start(new_config.interval_secs);
                    }
                }

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
                        // mark_config_self_write() 会让 watcher 丢弃这次改动的回声，
                        // 所以必须自己直接派发 ConfigReloaded：否则目录变更永远不生效
                        // （菜单里选完文件夹「没反应」）。与 settings.rs 的 Apply 一致。
                        util::mark_config_self_write();
                        if let Ok(content) = toml::to_string_pretty(&current_config) {
                            let config_path = config::get_config_path();
                            if std::fs::write(&config_path, content).is_ok() {
                                util::send_event(
                                    &self.event_tx,
                                    AppEvent::ConfigReloaded(current_config),
                                );
                            }
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
            // 动态壁纸只按首帧的亮度决策一次（不做周期性重采样：颜色跟随动图
            // 频闪比「不够对比」更伤观感）
            spawn_luma_sampling(
                path.to_path_buf(),
                screen_number,
                self.clock_manager.sample_band(screen_number),
                self.event_tx.clone(),
            );
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
            spawn_luma_sampling(
                path.to_path_buf(),
                screen_number,
                self.clock_manager.sample_band(screen_number),
                self.event_tx.clone(),
            );
            self.sync_menu_state();
            return;
        }

        // Route MP4/WebM to per-screen video player
        if ext == "mp4" || ext == "webm" {
            println!("Playing video wallpaper on screen {}: {:?}", screen_number, path);
            // 视频路径的亮度采样在 worker 线程顺带完成（首帧本来就是它生成的
            // PNG，不需要再解码一次视频），所以这里把采样带一并带进去。
            let band = self.clock_manager.sample_band(screen_number);
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
                    band,
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

        // 刘海矩阵：只有「有刘海的屏幕 + 开关打开」才需要合成。
        // 无刘海屏（safeAreaInsets.top == 0）与关闭状态下不进入合成，
        // 下面那条「零滤镜直设原图」的快路径才得以完整保留。
        let notch_geom = if self.notch.enabled {
            screen::find_screen_by_number(self.mtm, screen_number)
                .and_then(|s| notch::geometry(&s))
        } else {
            None
        };

        // Optimization: if no filters are applied, set the wallpaper directly without GPU/CPU pipeline and temp files
        if filters.is_default() && notch_geom.is_none() {
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
            // 直设路径没有解码过像素（这正是它快的原因），所以亮度采样单开一个
            // 后台任务：解码 → 采样 → 发事件，绝不占用主线程。
            // 采样要把整张图解码一遍（5400 万像素约 0.8s），时钟隐藏或自动对比
            // 关闭时这份数据根本不会被用到，直接跳过。
            if self.clock_manager.wants_luma() {
                spawn_luma_sampling(
                    path.to_path_buf(),
                    screen_number,
                    self.clock_manager.sample_band(screen_number),
                    self.event_tx.clone(),
                );
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
        let band_for_thread = self.clock_manager.sample_band(screen_number);
        // 降采样目标 = 屏幕物理像素。屏幕刚被拔掉时退化为 (0,0)，fit_for_screen
        // 会原样返回，行为等同改动前。
        let target_px = screen::find_screen_by_number(self.mtm, screen_number)
            .map(|s| screen::screen_pixel_size(&s))
            .unwrap_or((0, 0));
        // 闭包里拿不到 self，圆角先取出来
        let notch_radius = self.notch.corner_radius;
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, WallpaperInfo, f64)> {
                // Use wallpaper::decode_image which falls back to macOS NSImage for HEIC/HEIF
                let rgba = wallpaper::decode_image(&original_path_for_thread)?;
                let info = WallpaperInfo::from_image(&original_path_for_thread, &rgba);
                // 透明判定放在缩放之前：重采样会把边缘 alpha 抹开，可能把「本来
                // 全不透明」的图误判成需要 PNG，反而丢回慢路径。
                let keep_alpha = wallpaper::source_may_have_alpha(&original_path_for_thread)
                    && wallpaper::has_transparent_pixels(&rgba);
                // 尺寸归一：
                // - 有刘海：必须 cover 到**精确**的屏幕像素尺寸。系统按 Cover 缩放
                //   居中裁切，只有尺寸一致，画的 76px 黑条才会 1:1 落在刘海高度上。
                // - 无刘海：沿用「只缩不放」的 fit，避免放大损失清晰度。
                let fitted = if notch_geom.is_some() {
                    notch::cover_to(rgba, target_px.0, target_px.1)
                } else {
                    wallpaper::fit_for_screen(rgba, target_px)
                };
                // 亮度采样放在画黑条之前：顶部黑条是装饰，不该参与「自动对比色」判定
                let (bx0, by0, bx1, by1) = band_for_thread;
                let luma = contrast::sample_region_luma(
                    &fitted, bx0, by0, bx1, by1, contrast::SAMPLE_STEP,
                );
                let mut out_img = if filters.is_default() {
                    fitted
                } else {
                    filter_pipeline.process_image(&fitted, &filters)?
                };
                // 黑条必须最后画：先画会被滤镜一起调亮/染色，得到的不是纯黑
                if let Some(geom) = notch_geom {
                    notch::draw_top_bar(
                        &mut out_img,
                        geom.height_px(),
                        geom.radius_px(notch_radius),
                    );
                }
                // 落 Application Support 而不是 $TMPDIR —— 系统壁纸状态保存的是
                // 文件 URL，放临时目录会让退出后的桌面指向已删除文件。
                let stem = format!(
                    "wallflow_filtered_{}_{}",
                    std::process::id(),
                    static_counter
                );
                let filtered_path = wallpaper::write_wallpaper_output(&out_img, &stem, keep_alpha)?;
                // Wrap in guard so cancellation drops the file; keep() prevents deletion on success
                let guard = TempFileGuard::new(filtered_path);
                Ok((guard.keep(), info, luma))
            }).await;

            match res {
                Ok(Ok((filtered_path, info, luma))) => {
                    // 先发亮度再发壁纸：主线程先按新亮度定好文字色，壁纸淡入完成时
                    // 颜色已经就位，不会出现「先看清一下再变灰」的闪变。
                    util::send_event(&event_tx, AppEvent::WallpaperLuma { screen_number, luma });
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
        // 屏幕集合/分辨率变化后，时钟叠加层按新 frame 重建
        self.clock_manager.rebuild(self.mtm);
        // 刘海矩阵画在壁纸里，屏幕几何变了（换屏/改缩放）需要重渲染才能对齐，
        // 这里只做重放当前壁纸；没有壁纸时自然什么也不会发生。
        if self.notch.enabled {
            let sn = self.active_screen_number;
            if let Some(idx) = self.library.current_index {
                if idx < self.library.wallpapers.len() {
                    let path = self.library.wallpapers[idx].clone();
                    self.apply_wallpaper_on_screen(sn, &path);
                }
            }
        }
        self.sync_menu_state();
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        println!("Cleaning up temporary wallpaper files...");
        // 刻意不删 last_static_path：macOS 26 起系统壁纸状态由 Wallpaper Store
        // 托管，保存的是文件 URL，删掉后下次登录/重启桌面会指向不存在的文件。
        // 这类产物统一由下次启动的 wallpaper::cleanup_stale_cache() 收敛（只留最新一张）。
        // Clean per-screen dynamic/video frame temp files
        let temp_dir = std::env::temp_dir();
        for screen_state in self.screens.values() {
            let sn = screen_state.screen_number;
            let _ = std::fs::remove_file(temp_dir.join(format!("wallflow_frame_{}_{}_a.png", std::process::id(), sn)));
            let _ = std::fs::remove_file(temp_dir.join(format!("wallflow_frame_{}_{}_b.png", std::process::id(), sn)));
        }
    }
}

/// 后台解码一张壁纸并测量「时钟范围内」的平均亮度，结果以
/// `AppEvent::WallpaperLuma` 回传主线程，由 `ClockManager` 做颜色决策。
///
/// `band` 由 `ClockManager::sample_band()` 给出：它把采样严格限制在时钟文字
/// 真正压住的那块矩形内（水平靠 AppKit 实测的文本宽度），而不是取整张图或
/// 一个宽泛的中心带。
///
/// 为什么要单独开任务：静态直设路径与 GIF/APNG 路径本来都不解码像素，
/// 而 4K 图解码可能要几百毫秒 —— 放在主线程会顶住壁纸淡入。
/// 视频路径不走这里：它的首帧由 video.rs 的 worker 顺带采样，不必解码两遍。
fn spawn_luma_sampling(
    path: PathBuf,
    screen_number: u32,
    band: (f64, f64, f64, f64),
    event_tx: Sender<AppEvent>,
) {
    tokio::spawn(async move {
        let luma = tokio::task::spawn_blocking(move || -> Option<f64> {
            let img = wallpaper::decode_image(&path).ok()?;
            let (x0, y0, x1, y1) = band;
            Some(contrast::sample_region_luma(&img, x0, y0, x1, y1, contrast::SAMPLE_STEP))
        })
        .await
        .unwrap_or(None);

        // 解码失败（文件损坏、格式不支持）时静默跳过：保留上一个采样值，
        // 不因为一次失败把颜色改掉。注意 GIF/APNG 走 image::open 的默认语义，
        // 取到的就是第一帧。
        if let Some(luma) = luma {
            util::send_event(&event_tx, AppEvent::WallpaperLuma { screen_number, luma });
        }
    });
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
            // 复用这个 1s 重复的 timer 驱动时钟叠加层刷新
            if let Some(state_arc) = self.ivars().state.get() {
                if let Ok(mut state) = state_arc.lock() {
                    state.clock_manager.tick();
                }
            }
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

        // 设置页调试模式只显示设置窗口：全屏时钟叠加层会成为无标题的首个窗口，
        // 挡住自动化工具对设置面板的视觉检查；正常运行仍按原配置显示时钟。
        let mut initial_clock = initial_config.clock.clone();
        if std::env::var_os("WF_DEBUG_SHOW").is_some() {
            initial_clock.enabled = false;
        }
        locked_state.clock_manager.apply_config(mtm, initial_clock);

        // Start auto-switch scheduler; Manual mode means the user switches,
        // so no periodic firing (previously the ticker ignored this).
        locked_state.last_applied = Some(initial_config.clone());
        if initial_config.interval_secs > 0
            && !matches!(initial_config.mode, library::SwitchMode::Manual)
        {
            locked_state.scheduler.start(initial_config.interval_secs);
        }

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
                manager.set_clock_enabled(initial_config.clock.enabled);
                manager.set_auto_contrast(initial_config.clock.auto_contrast);
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
