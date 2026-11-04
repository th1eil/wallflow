/// Metadata about a wallpaper file, displayed as subtitle in the menu.
#[derive(Debug, Clone)]
pub struct WallpaperInfo {
    pub filename: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub format: String,
}

impl WallpaperInfo {
    /// Extract from a loaded image and its source path.
    pub fn from_image(path: &std::path::Path, img: &image::RgbaImage) -> Self {
        let filename = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown")
            .to_string();
        let (width, height) = img.dimensions();
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let format = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        Self { filename, width, height, file_size, format }
    }

    /// Extract minimal info from path only (format + file size, no resolution).
    pub fn from_path(path: &std::path::Path) -> Self {
        let filename = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("unknown")
            .to_string();
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let format = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        Self { filename, width: 0, height: 0, file_size, format }
    }

    /// Format as display string, e.g. "1920×1080 · PNG · 2.3 MB"
    pub fn display_line(&self) -> String {
        let dims = if self.width > 0 && self.height > 0 {
            format!("{}×{} · ", self.width, self.height)
        } else {
            String::new()
        };
        let size_str = if self.file_size >= 1024 * 1024 {
            format!("{:.1} MB", self.file_size as f64 / (1024.0 * 1024.0))
        } else if self.file_size >= 1024 {
            format!("{:.0} KB", self.file_size as f64 / 1024.0)
        } else {
            format!("{} B", self.file_size)
        };
        format!("{} {}· {}", dims, self.format, size_str)
    }
}

/// 区分取消、仅入库、入库并应用，避免把「不应用」误当作取消保存。
#[derive(Debug, Clone, Copy)]
pub enum RandomWallpaperChoice {
    Cancel,
    SaveOnly,
    SaveAndApply,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    Previous,
    Next,
    Random,
    Settings,
    Exit,
    SetWallpaper(std::path::PathBuf),
    ConfigReloaded(crate::config::Config),
    ChooseDirectory,
    /// Async scan completed; wallpapers ready to load into library
    DirectoryScanned {
        dir_path: std::path::PathBuf,
        wallpapers: Vec<std::path::PathBuf>,
        /// 扫描完成后要直接应用的那张图（下载随机壁纸后切到刚保存的文件）。
        /// 只有「刷新列表」是必须的，选中哪张是可选语义 —— 为 None 时按
        /// library 当前模式正常取下一张；为 Some 时用它同步 library 的
        /// current_index/history，避免屏幕内容与选中状态错位。
        select: Option<std::path::PathBuf>,
    },
    /// Apply a named filter preset to the active screen
    SetFilterPreset(crate::color::FilterPreset),
    /// Live filter preview from the settings sliders (not persisted).
    /// Skipped automatically when the active screen shows dynamic content.
    PreviewFilters(crate::color::FilterParams),
    /// 滑块预览的防抖提交：`PreviewFilters` 只登记参数，等 180ms 没有新事件后
    /// 才发这个事件真正重算。带上代数，过期的提交在代数不匹配时被丢弃。
    CommitPreviewFilters(u64),
    /// Live clock-overlay preview from the settings panel (not persisted).
    /// Rebuilds the clock overlay only — the wallpaper, filters, and scheduler
    /// are left untouched, unlike `PreviewFilters`.
    /// 实时时钟预览（不持久化）：只重建时钟叠加层
    PreviewClock(crate::config::ClockConfig),
    /// 刘海黑色矩阵的实时预览（不持久化）：改开关或拖圆角滑块时立即生效，
    /// Apply 才写盘。它画在壁纸图片里，收到后需要重渲染当前壁纸（设置面板
    /// 侧做了 0.4s 防抖，避免拖滑块时反复编码整张图）。
    PreviewNotch(crate::config::NotchConfig),
    /// Select which screen the navigation controls target
    SelectActiveScreen(u32),
    /// Set wallpaper on a specific screen (used by dynamic players)
    SetWallpaperOnScreen {
        screen_number: u32,
        path: std::path::PathBuf,
    },
    /// Direct dynamic frame update (from GIF/APNG/video player) without full reload logic
    SetDynamicFrame {
        screen_number: u32,
        path: std::path::PathBuf,
    },
    /// Toggle pause/resume for the active screen's dynamic player
    TogglePause,
    /// 切换壁纸中央时钟叠加层的显示/隐藏（状态持久化到 config.toml）
    ToggleClock,
    /// 切换「时钟自动对比色」：壁纸与文字色对比不足时自动调整字体颜色
    /// （判定与推色规则见 contrast.rs，状态持久化到 config.toml）
    ToggleAutoContrast,
    /// Change loop mode for the active screen's dynamic player
    SetLoopMode(crate::playback::LoopMode),
    /// Override FPS for the active screen's dynamic player (0 = native)
    SetFps(u32),
    /// Download a random wallpaper from an online source (picsum.photos).
    DownloadRandomWallpaper,
    /// 临时下载完成后交给主线程展示；目录随最后一个持有者释放而清理。
    RandomWallpaperReady {
        temporary_dir: std::sync::Arc<tempfile::TempDir>,
        path: std::path::PathBuf,
    },
    /// 下载或保存失败统一回主线程，确保忙碌状态能复位。
    RandomWallpaperFailed(String),
    /// 原生预览窗口只提交一次选择，不在回调中修改应用状态。
    ConfirmRandomWallpaper(RandomWallpaperChoice),
    RandomWallpaperSaved {
        dir: std::path::PathBuf,
        path: std::path::PathBuf,
        /// 仅保存不能改变当前选择、历史记录或桌面。
        apply: bool,
    },
    /// Open the current wallpaper directory in Finder.
    OpenWallpaperFolder,
    /// The set of connected displays changed (hot-plug, resolution change).
    /// Reconciles per-screen state and rebuilds the display submenu.
    ScreensChanged,
    /// Transient text on the menu status line (None clears it).
    SetStatusLine(Option<String>),
    /// 壁纸在「时钟文字带」上的平均相对亮度采样结果（0..1），由后台采样任务发出。
    /// 时钟据此判断是否需要把字体颜色替换为对比色（规则见 contrast.rs）。
    /// 静态图片、GIF/APNG 首帧、视频首帧都汇总到这一个事件。
    WallpaperLuma {
        /// 静态/首帧采样带请求代数；视频由播放器自行管理生命周期。
        sequence_id: Option<u64>,
        screen_number: u32,
        luma: f64,
    },
    WallpaperProcessed {
        sequence_id: u64,
        luma: f64,
        original_path: std::path::PathBuf,
        filtered_path: std::path::PathBuf,
        screen_number: u32,
        info: WallpaperInfo,
    },
}
