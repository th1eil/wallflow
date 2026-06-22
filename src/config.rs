use std::path::PathBuf;
use anyhow::Result;
use notify::{Watcher, RecursiveMode, EventKind};
use crate::app_event::AppEvent;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub wallpaper_dir: String,
    pub interval_secs: u64,
    pub mode: String, // "sequential" | "random"
    pub filters: FilterConfig,
    /// 随机壁纸下载 URL，调用需返回 image/* Content-Type 的图片直链
    #[serde(default = "default_random_wallpaper_url")]
    pub random_wallpaper_url: String,
}

fn default_random_wallpaper_url() -> String {
    "https://picsum.photos/1920/1080".to_string()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilterConfig {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub blur: f32,
}

impl Default for Config {
    fn default() -> Self {
        // HOME must be set on macOS; panic with a clear message if it isn't
        let home = std::env::var("HOME").expect("HOME environment variable not set");
        let pictures = PathBuf::from(home).join("Pictures");
        Self {
            wallpaper_dir: pictures.to_string_lossy().to_string(),
            interval_secs: 300,
            mode: "sequential".to_string(),
            filters: FilterConfig::default(),
            random_wallpaper_url: default_random_wallpaper_url(),
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            blur: 0.0,
        }
    }
}

pub fn get_config_path() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    PathBuf::from(home).join(".config/wallflow/config.toml")
}

pub fn load_or_create_config() -> Result<Config> {
    let path = get_config_path();
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let default_config = Config::default();
        let content = toml::to_string_pretty(&default_config)?;
        std::fs::write(&path, content)?;
        return Ok(default_config);
    }

    let content = std::fs::read_to_string(&path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

pub fn watch_config(tx: std::sync::mpsc::Sender<AppEvent>) -> Result<notify::RecommendedWatcher> {
    let config_path = get_config_path();
    let parent_dir = config_path.parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid config path"))?
        .to_path_buf();

    // Create recommended watcher
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        match res {
            Ok(event) => {
                if let EventKind::Modify(_) = event.kind {
                    for path in event.paths {
                        if path.file_name() == Some(std::ffi::OsStr::new("config.toml")) {
                            println!("Config change detected, reloading...");
                            // Small sleep to ensure write finishes before reading
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            if let Ok(new_config) = load_or_create_config() {
                                let _ = tx.send(AppEvent::ConfigReloaded(new_config));
                            }
                        }
                    }
                }
            }
            Err(e) => eprintln!("Watcher error: {:?}", e),
        }
    })?;

    watcher.watch(&parent_dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}
