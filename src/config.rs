use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use anyhow::Result;
use notify::{Watcher, RecursiveMode, EventKind};
use serde::Deserialize;
use crate::app_event::AppEvent;
use crate::library::SwitchMode;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub wallpaper_dir: String,
    pub interval_secs: u64,
    #[serde(default)]
    pub mode: SwitchMode,
    pub filters: FilterConfig,
    /// 随机壁纸下载 URL，调用需返回 image/* Content-Type 的图片直链
    #[serde(default = "default_random_wallpaper_url")]
    pub random_wallpaper_url: String,
    /// 壁纸中央时钟叠加层
    #[serde(default)]
    pub clock: ClockConfig,
    /// 刘海屏顶部黑色矩阵
    #[serde(default)]
    pub notch: NotchConfig,
}

fn default_random_wallpaper_url() -> String {
    "https://picsum.photos/1920/1080".to_string()
}

/// 屏幕中央时钟叠加层配置（苹果锁屏风格：大号细体时间 + 日期 + 副时区）。
///
/// 所有新增字段都带 `#[serde(default)]`：老版本写出的 config.toml（只有
/// enabled/timezone 那几项）仍能被完整解析，缺省项走默认值，无需迁移。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClockConfig {
    /// 是否显示时钟叠加层
    #[serde(default)]
    pub enabled: bool,
    /// 主时区："local" 或 IANA 时区名（如 "Asia/Shanghai"）
    #[serde(default = "default_primary_timezone")]
    pub primary_timezone: String,
    /// 是否显示副时区
    #[serde(default = "default_true")]
    pub secondary_enabled: bool,
    /// 是否在主时间下方显示日期行
    #[serde(default = "default_true")]
    pub show_date: bool,
    /// 副时区 IANA 名称（如 "Europe/London"）
    #[serde(default = "default_secondary_timezone")]
    pub secondary_timezone: String,
    /// 副时区自定义标签（留空则自动取时区城市名，如 "London"）
    #[serde(default)]
    pub secondary_label: String,
    /// true = 24 小时制；false = 12 小时制（上午/下午）
    #[serde(default = "default_true")]
    pub use_24_hour: bool,
    /// 时间是否带秒（带秒时由既有的 1s 刷新定时器驱动，无需额外定时器）
    #[serde(default)]
    pub show_seconds: bool,
    /// 是否在日期行（星期之后）显示农历
    #[serde(default)]
    pub show_lunar: bool,
    /// 是否在副时区行显示该地区对应的节假日
    #[serde(default)]
    pub show_holiday: bool,

    // ---- 外观 ----
    /// 文字颜色，`#RRGGBB` 或 `#RGB`（大小写不限）
    #[serde(default = "default_text_color")]
    pub text_color: String,
    /// 时间行字号缩放系数，1.0 = 默认大小（见 clock.rs 的基准字号）
    #[serde(default = "default_line_scale")]
    pub time_scale: f32,
    /// 日期行字号缩放系数
    #[serde(default = "default_line_scale")]
    pub date_scale: f32,
    /// 副时区行字号缩放系数
    #[serde(default = "default_line_scale")]
    pub secondary_scale: f32,
    /// 时间行不透明度；日期/副时区按各自的层级比例联动，保持视觉主次
    #[serde(default = "default_text_opacity")]
    pub text_opacity: f32,
    /// 文字投影不透明度，0 = 关闭阴影
    #[serde(default = "default_shadow_opacity")]
    pub shadow_opacity: f32,
    /// 时钟组中心相对屏幕高度的偏移（0.0 = 基准位置 52% 处）
    #[serde(default)]
    pub vertical_offset: f32,
    /// 时间行字重（NSFontWeight 连续值，约 -1.0 极细 … 0.6 极粗）
    #[serde(default = "default_time_weight", deserialize_with = "de_font_weight")]
    pub time_font_weight: f32,
    /// 是否在文字背后显示毛玻璃背景卡片
    #[serde(default)]
    pub frosted_glass: bool,
    /// 毛玻璃背景强度（0.0 几乎透明 … 1.0 全效）
    #[serde(default = "default_frosted_opacity")]
    pub frosted_opacity: f32,
    /// 文字水平对齐方式；同时决定「自动对比色」的亮度采样区域（见 TextAlign）
    #[serde(default = "default_text_align")]
    pub text_align: TextAlign,
    /// 壁纸与文字色对比不足时，自动把文字色换成「保色相的亮度反转色」。
    /// 判定与推色规则见 contrast.rs；默认关闭，避免升级后观感突变。
    #[serde(default)]
    pub auto_contrast: bool,
}

/// 时钟文字的水平对齐方式。
///
/// 用语义化枚举而不是数字：AppKit 的 `NSTextAlignment` 取值随 ABI 变化
/// （Apple Silicon 上是 Center=1/Right=2，Intel 上相反，见 SDK 的
/// `TargetConditionals.h`），硬编码数字会在两种机器上得到相反效果。
/// 数字转换集中在 `clock.rs::ns_text_align()` 一处。
///
/// 它同时决定「自动对比色」往哪里取亮度：
/// - `Right`：只采文字压住的那块矩形（文字在屏幕右侧）
/// - `Center`：采整张壁纸
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlign {
    /// 苹果锁屏风格的水平居中
    Center,
    /// 文字贴屏幕右侧（历史行为，见 clock.rs 的说明）
    Right,
}

fn default_true() -> bool { true }
fn default_primary_timezone() -> String { "local".to_string() }
fn default_secondary_timezone() -> String { "Europe/London".to_string() }
fn default_text_color() -> String { "#FFFFFF".to_string() }
fn default_line_scale() -> f32 { 1.0 }
fn default_text_opacity() -> f32 { 0.95 }
fn default_shadow_opacity() -> f32 { 0.45 }
fn default_time_weight() -> f32 { -0.6 }
fn default_frosted_opacity() -> f32 { 0.5 }

/// 兼容旧版三档字重枚举（`"thin" / "regular" / "bold"`）：字段从枚举改成
/// 连续 NSFontWeight 值后，老 config.toml 里的字符串仍能被解析，映射到
/// 最接近的 NSFontWeight 常量（thin=-0.6 / regular=0.0 / bold=0.3）。
fn de_font_weight<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Compat {
        Number(f32),
        Name(String),
    }
    match Compat::deserialize(deserializer)? {
        Compat::Number(v) => Ok(v),
        Compat::Name(s) => match s.as_str() {
            "thin" => Ok(-0.6),
            "regular" => Ok(0.0),
            "bold" => Ok(0.3),
            other => Err(serde::de::Error::custom(format!(
                "unknown font weight name: {other}"
            ))),
        },
    }
}
/// 默认保持历史行为（文字靠右）。改成 `Center` 会同时把时钟移到屏幕中央，
/// 并把亮度采样切到整张壁纸 —— 属于观感变化，所以默认不动。
fn default_text_align() -> TextAlign { TextAlign::Right }

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            primary_timezone: default_primary_timezone(),
            secondary_enabled: true,
            show_date: true,
            secondary_timezone: default_secondary_timezone(),
            secondary_label: String::new(),
            use_24_hour: true,
            show_seconds: false,
            show_lunar: false,
            show_holiday: false,
            text_color: default_text_color(),
            time_scale: default_line_scale(),
            date_scale: default_line_scale(),
            secondary_scale: default_line_scale(),
            text_opacity: default_text_opacity(),
            shadow_opacity: default_shadow_opacity(),
            vertical_offset: 0.0,
            time_font_weight: default_time_weight(),
            frosted_glass: false,
            frosted_opacity: default_frosted_opacity(),
            text_align: default_text_align(),
            auto_contrast: false,
        }
    }
}

/// 刘海屏顶部黑色矩阵配置。
///
/// 只在「安全区顶部 > 0」的屏幕上生效（有刘海的 MacBook）；外接显示器与无刘海
/// 机型自动跳过，因此默认开启对它们没有副作用。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NotchConfig {
    /// 是否绘制黑色矩阵（默认开启）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 下方壁纸顶部两角的圆角半径（pt）。
    /// 渲染时黑条会在左右两侧向下外翻这段距离，把壁纸的顶角切成圆弧。
    #[serde(default = "default_notch_radius")]
    pub corner_radius: f32,
}

fn default_notch_radius() -> f32 { 10.0 }

impl Default for NotchConfig {
    fn default() -> Self {
        Self { enabled: true, corner_radius: default_notch_radius() }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
            mode: SwitchMode::Sequential,
            filters: FilterConfig::default(),
            random_wallpaper_url: default_random_wallpaper_url(),
            clock: ClockConfig::default(),
            notch: NotchConfig::default(),
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

    let last_reload = Mutex::new(Instant::now() - Duration::from_secs(1));

    // Create recommended watcher
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        match res {
            Ok(event) => {
                if let EventKind::Modify(_) | EventKind::Create(_) = event.kind {
                    for path in event.paths {
                        if path.file_name() == Some(std::ffi::OsStr::new("config.toml")) {
                            // Skip echoes of the app's own writes: the direct
                            // ConfigReloaded already went out at write time.
                            if crate::util::was_recently_self_written() {
                                println!("Ignoring config change written by WallFlow itself.");
                                continue;
                            }
                            let mut last = last_reload.lock().unwrap();
                            if last.elapsed() >= Duration::from_millis(200) {
                                *last = Instant::now();
                                println!("Config change detected, reloading...");
                                if let Ok(new_config) = load_or_create_config() {
                                    crate::util::send_event(&tx, AppEvent::ConfigReloaded(new_config));
                                }
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
