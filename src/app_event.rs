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
    },
    /// Apply a named filter preset to the active screen
    SetFilterPreset(crate::color::FilterPreset),
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
    /// Change loop mode for the active screen's dynamic player
    SetLoopMode(crate::playback::LoopMode),
    /// Override FPS for the active screen's dynamic player (0 = native)
    SetFps(u32),
    /// Download a random wallpaper from an online source (picsum.photos).
    DownloadRandomWallpaper,
    /// Open the current wallpaper directory in Finder.
    OpenWallpaperFolder,
    /// The set of connected displays changed (hot-plug, resolution change).
    /// Reconciles per-screen state and rebuilds the display submenu.
    ScreensChanged,
    WallpaperProcessed {
        sequence_id: u64,
        original_path: std::path::PathBuf,
        filtered_path: std::path::PathBuf,
        screen_number: u32,
        info: WallpaperInfo,
    },
}
