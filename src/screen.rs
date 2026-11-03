use std::path::PathBuf;
use objc2::{msg_send};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSString};
use crate::color::FilterParams;
use crate::dynamic::DynamicWallpaperPlayer;
use crate::playback::LoopMode;
use crate::video::VideoWallpaperPlayer;

/// What kind of wallpaper is currently shown on a screen — drives whether
/// playback controls are enabled and which preview path filters can take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// Nothing applied yet
    None,
    /// Static image (may carry GPU/CPU filters)
    Static,
    /// GIF / APNG via DynamicWallpaperPlayer
    Dynamic,
    /// MP4 / WebM via VideoWallpaperPlayer
    Video,
}

/// Extract the unique per-display identifier from an NSScreen.
/// Uses NSScreenNumber from the device description dictionary — a stable u32
/// that persists across display configuration changes for the same physical monitor.
pub fn screen_number(screen: &NSScreen) -> u32 {
    unsafe {
        // NSDictionary *desc = [screen deviceDescription];
        // NSNumber *num = [desc objectForKey:@"NSScreenNumber"];
        // return [num unsignedIntValue];
        let desc: Retained<NSObject> = msg_send![screen, deviceDescription];
        let key = NSString::from_str("NSScreenNumber");
        let value: Retained<NSObject> = msg_send![&desc, objectForKey: &*key];
        msg_send![&value, unsignedIntValue]
    }
}

/// Get resolution string for a screen (e.g. "2560x1440") for display in menu
pub fn screen_resolution(screen: &NSScreen) -> String {
    let frame = screen.frame();
    let w = frame.size.width as u32;
    let h = frame.size.height as u32;
    format!("{}x{}", w, h)
}

/// 屏幕的物理像素尺寸（逻辑点 × backingScaleFactor）。
///
/// 滤镜前的降采样需要的是物理像素：壁纸最终按物理像素渲染，按逻辑点缩会丢掉
/// Retina 的 2 倍细节；而不缩又会对 6000px 的源图做无用的全量计算。
pub fn screen_pixel_size(screen: &NSScreen) -> (u32, u32) {
    let frame = screen.frame();
    let scale: f64 = unsafe { msg_send![screen, backingScaleFactor] };
    (
        (frame.size.width * scale).round() as u32,
        (frame.size.height * scale).round() as u32,
    )
}

/// Per-screen independent wallpaper state.
/// Each connected display gets its own ScreenState,
/// allowing different wallpapers, filters, and dynamic players per screen.
pub struct ScreenState {
    pub screen_number: u32,
    pub current_filters: FilterParams,
    pub dynamic_player: DynamicWallpaperPlayer,
    pub video_player: VideoWallpaperPlayer,
    pub last_static_path: Option<PathBuf>,
    pub static_counter: u64,
    /// 每屏独立代数：新请求使旧任务失效，其他屏幕的请求不互相取消。
    pub render_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// What's on screen now (Static/Dynamic/Video), so the UI can react.
    pub content_kind: ContentKind,
    /// Main-thread mirror of the players' pause state. Both player tasks treat
    /// PlaybackCommand::Pause/Resume as absolute set, not toggle — so AppState
    /// owns the toggle here and sends the matching explicit command.
    pub paused: bool,
    /// Mirrors playback settings last sent to this screen's players.
    pub loop_mode: LoopMode,
    /// 0 = native timing (menu label 原始).
    pub fps_override: u32,
}

impl ScreenState {
    pub fn new(
        screen_number: u32,
        event_tx: std::sync::mpsc::Sender<crate::app_event::AppEvent>,
    ) -> Self {
        Self {
            screen_number,
            current_filters: FilterParams::default(),
            dynamic_player: DynamicWallpaperPlayer::new(event_tx.clone()),
            video_player: VideoWallpaperPlayer::new(event_tx),
            last_static_path: None,
            static_counter: 0,
            render_generation: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            content_kind: ContentKind::None,
            paused: false,
            loop_mode: LoopMode::Infinite,
            fps_override: 0,
        }
    }
}

/// Helper: find a specific NSScreen by its screen number
pub fn find_screen_by_number(
    mtm: MainThreadMarker,
    target: u32,
) -> Option<Retained<NSScreen>> {
    let screens = crate::wallpaper::get_screens(mtm);
    screens.into_iter().find(|s| screen_number(s) == target)
}
