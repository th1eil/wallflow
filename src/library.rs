use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwitchMode {
    Sequential,
    Random,
    Manual,
}

impl Default for SwitchMode {
    fn default() -> Self {
        Self::Sequential
    }
}

struct Xorshift {
    state: u64,
}

impl Xorshift {
    fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(123456789);
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }
}

pub struct WallpaperLibrary {
    pub dir_path: Option<PathBuf>,
    pub wallpapers: Vec<PathBuf>,
    pub history: VecDeque<PathBuf>, // Max 50 items
    pub history_idx: Option<usize>, // Position when navigating history back/forth
    pub current_index: Option<usize>,
    pub mode: SwitchMode,
    rng: Xorshift,
}

impl WallpaperLibrary {
    pub fn new() -> Self {
        Self {
            dir_path: None,
            wallpapers: Vec::new(),
            history: VecDeque::with_capacity(50),
            history_idx: None,
            current_index: None,
            mode: SwitchMode::Sequential,
            rng: Xorshift::new(),
        }
    }

    /// Recursively scan a directory for supported wallpaper formats
    pub fn scan(&mut self, dir: &Path) -> Result<()> {
        if !dir.is_dir() {
            return Err(anyhow!("Provided path is not a directory: {:?}", dir));
        }

        let wallpapers = collect_wallpapers(dir);
        self.dir_path = Some(dir.to_path_buf());
        self.wallpapers = wallpapers;
        self.current_index = None;
        self.history_idx = None;

        Ok(())
    }

    /// Set current selection mode
    pub fn set_mode(&mut self, mode: SwitchMode) {
        self.mode = mode;
    }

    /// 把指定文件设为当前选中项（同步 `current_index` 与历史指针）。
    ///
    /// 用于「下载随机壁纸后立刻显示刚保存的那张」：只把屏幕切过去而不改
    /// `current_index`，下一次 Next 或滤镜预览会从旧索引继续，屏幕内容与
    /// 「当前项」从此错位。返回 false 表示该文件不在列表里（被移走/删除），
    /// 调用方应退回 `next()` 而不是留空。
    pub fn select_path(&mut self, path: &Path) -> bool {
        let Some(idx) = self.wallpapers.iter().position(|w| w == path) else {
            return false;
        };
        self.current_index = Some(idx);
        self.add_to_history(path.to_path_buf());
        true
    }

    /// Add wallpaper to history (max 50 items)
    pub fn add_to_history(&mut self, path: PathBuf) {
        // Avoid consecutive duplicate entries
        if self.history.back() == Some(&path) {
            return;
        }

        if self.history.len() >= 50 {
            self.history.pop_front();
        }
        self.history.push_back(path);
        // Reset history pointer to the latest element
        self.history_idx = Some(self.history.len() - 1);
    }

    /// Get the next wallpaper according to the current mode
    pub fn next(&mut self) -> Option<PathBuf> {
        if self.wallpapers.is_empty() {
            return None;
        }

        // If we are currently navigating backwards in history, move forward in history first
        if let Some(idx) = self.history_idx {
            if idx + 1 < self.history.len() {
                let next_idx = idx + 1;
                self.history_idx = Some(next_idx);
                let path = self.history[next_idx].clone();
                // Update current index in main wallpapers list if found
                if let Some(pos) = self.wallpapers.iter().position(|p| p == &path) {
                    self.current_index = Some(pos);
                }
                return Some(path);
            }
        }

        // Otherwise, select a new wallpaper based on the mode
        let next_idx = match self.mode {
            SwitchMode::Sequential => {
                if let Some(curr) = self.current_index {
                    (curr + 1) % self.wallpapers.len()
                } else {
                    0
                }
            }
            SwitchMode::Random => {
                let total = self.wallpapers.len();
                if total <= 1 {
                    0
                } else {
                    // Try up to 10 times to pick a different wallpaper than current
                    let mut candidate = (self.rng.next() as usize) % total;
                    let mut tries = 0;
                    while Some(candidate) == self.current_index && tries < 10 {
                        candidate = (self.rng.next() as usize) % total;
                        tries += 1;
                    }
                    if Some(candidate) == self.current_index {
                        (candidate + 1) % total
                    } else {
                        candidate
                    }
                }
            }
            SwitchMode::Manual => {
                return self.current_index.map(|idx| self.wallpapers[idx].clone());
            }
        };

        self.current_index = Some(next_idx);
        let path = self.wallpapers[next_idx].clone();
        self.add_to_history(path.clone());
        Some(path)
    }

    /// Go back to the previous wallpaper in history
    pub fn previous(&mut self) -> Option<PathBuf> {
        if self.history.is_empty() {
            return None;
        }

        let current_idx = match self.history_idx {
            Some(idx) => idx,
            None => self.history.len() - 1,
        };

        if current_idx > 0 {
            let prev_idx = current_idx - 1;
            self.history_idx = Some(prev_idx);
            let path = self.history[prev_idx].clone();

            // Update current index in main list if found
            if let Some(pos) = self.wallpapers.iter().position(|p| p == &path) {
                self.current_index = Some(pos);
            }
            Some(path)
        } else {
            // Already at the beginning of history
            None
        }
    }
}

/// Scan a directory tree for supported wallpaper files.
/// Extracted from `WallpaperLibrary::scan` so it can be called from a background thread
/// without holding the library lock. Results are sent back via event channel.
pub fn collect_wallpapers(dir: &Path) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                wallpapers.extend(collect_wallpapers(&path));
            } else if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if matches!(
                        ext_lower.as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "gif" | "apng" | "mp4" | "webm" | "heic" | "heif"
                    ) {
                        wallpapers.push(path);
                    }
                }
            }
        }
    }
    wallpapers.sort();
    wallpapers
}

#[cfg(test)]
mod tests {
    use super::*;
    // 显式引入：父模块的 `use` 是私有绑定，不依赖 `super::*` 的间接可见性
    use std::path::{Path, PathBuf};

    fn lib_with(paths: &[&str]) -> WallpaperLibrary {
        let mut lib = WallpaperLibrary::new();
        lib.wallpapers = paths.iter().map(PathBuf::from).collect();
        lib
    }

    /// 下载后直接切到指定文件：选中项与历史指针都要对齐，Sequential 的
    /// 下一次 Next 必须从这张继续，而不是回到第一张。
    #[test]
    fn select_path_aligns_current_and_history() {
        let mut lib = lib_with(&["/w/a.jpg", "/w/b.jpg", "/w/c.jpg"]);

        assert!(lib.select_path(Path::new("/w/c.jpg")));
        assert_eq!(lib.current_index, Some(2));
        assert_eq!(lib.history.back(), Some(&PathBuf::from("/w/c.jpg")));
        assert_eq!(lib.history_idx, Some(0));

        assert_eq!(lib.next(), Some(PathBuf::from("/w/a.jpg")));
    }

    /// 指定文件不在列表里时必须是 no-op，调用方据此退回 next()，
    /// 不能让 library 停在「看似选中、实则无效」的索引上。
    #[test]
    fn select_path_missing_is_noop() {
        let mut lib = lib_with(&["/w/a.jpg"]);

        assert!(!lib.select_path(Path::new("/w/missing.jpg")));
        assert_eq!(lib.current_index, None);
        assert!(lib.history.is_empty());
    }

    /// Random 模式下同样要命中指定文件：这是「下载后切到刚保存那张」的
    /// 核心保证 —— 不能被随机抽签覆盖。
    #[test]
    fn select_path_wins_over_random_mode() {
        let mut lib = lib_with(&["/w/a.jpg", "/w/b.jpg"]);
        lib.set_mode(SwitchMode::Random);

        assert!(lib.select_path(Path::new("/w/b.jpg")));
        assert_eq!(lib.current_index, Some(1));
        assert_eq!(lib.history.len(), 1);
    }
}
