use std::path::PathBuf;
use objc2::{msg_send};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSString};
use crate::color::FilterParams;
use crate::dynamic::DynamicWallpaperPlayer;
use crate::video::VideoWallpaperPlayer;

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
