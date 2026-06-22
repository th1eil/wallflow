use std::path::Path;
use anyhow::{anyhow, Result};
use objc2::{msg_send, sel};
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSData, NSDictionary, NSError, NSString, NSURL};
use crate::util::find_class;
use std::ffi::c_void;
use std::path::PathBuf;

// CoreGraphics FFI for capturing screen content.
extern "C" {
    fn CGDisplayCreateImage(displayID: u32) -> *mut c_void;
    fn CFRelease(cf: *mut c_void);
}

/// 获取当前连接的所有屏幕
pub fn get_screens(mtm: MainThreadMarker) -> Vec<Retained<NSScreen>> {
    let screens_array = NSScreen::screens(mtm);
    let count = screens_array.count();
    let mut screens = Vec::with_capacity(count);
    for i in 0..count {
        let screen = screens_array.objectAtIndex(i);
        screens.push(screen);
    }
    screens
}

/// 为指定的屏幕设置壁纸图片
pub fn set_wallpaper(_mtm: MainThreadMarker, screen: &NSScreen, path: &Path) -> Result<()> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid path"))?;
    
    unsafe {
        // 创建 NSURL
        let path_ns = NSString::from_str(path_str);
        let url = NSURL::fileURLWithPath(&path_ns);
        
        // 获取共享工作空间
        let workspace = NSWorkspace::sharedWorkspace();
        
        // 创建空字典作为选项参数
        let options = NSDictionary::new();
        
        // 设置桌面图片
        workspace.setDesktopImageURL_forScreen_options_error(&url, screen, &options)
            .map_err(|err: Retained<NSError>| {
                anyhow!("Failed to set wallpaper: {}", err.localizedDescription())
            })?;
    }
    
    Ok(())
}
/// Decode an image file to RGBA, using macOS NSImage fallback for unsupported formats.
/// - Tries `image::open` first (fastest path for PNG/JPEG/WebP/GIF).
/// - Falls back to macOS native decoder for HEIC/HEIF files.
pub fn decode_image(path: &Path) -> Result<image::RgbaImage> {
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match image::open(path) {
        Ok(img) => Ok(img.to_rgba8()),
        Err(img_err) => {
            if ext == "heic" || ext == "heif" {
                unsafe {
                    decode_image_native(path).map_err(|native_err| {
                        anyhow!("Failed to decode {}: image crate: {}, native: {}",
                            path.display(), img_err, native_err)
                    })
                }
            } else {
                Err(img_err.into())
            }
        }
    }
}

/// macOS native image decoder via NSImage → TIFF → NSBitmapImageRep → PNG → image crate.
/// Required for HEIC/HEIF which the `image` crate doesn't support without a newer Rust toolchain.
unsafe fn decode_image_native(path: &Path) -> Result<image::RgbaImage> {
    let path_str = path.to_str().ok_or_else(|| anyhow!("Invalid UTF-8 path"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path_str));
    let data = NSData::dataWithContentsOfURL(&url);
    let data = match data {
        Some(d) => d,
        None => return Err(anyhow!("NSData failed to read file")),
    };

    // NSImage decodes HEIC natively on macOS
    let img_cls = find_class("NSImage")?;
    let nsimage: *mut NSObject = msg_send![img_cls, alloc];
    let nsimage: *mut NSObject = msg_send![nsimage, initWithData: &*data];
    if nsimage.is_null() {
        return Err(anyhow!("NSImage initWithData returned nil"));
    }

    // TIFF → NSBitmapImageRep → PNG (same pattern as video.rs)
    let tiff: *mut NSObject = msg_send![&*nsimage, TIFFRepresentation];
    if tiff.is_null() {
        return Err(anyhow!("TIFFRepresentation returned nil"));
    }

    let bitmap_cls = find_class("NSBitmapImageRep")?;
    let bitmap: *mut NSObject = msg_send![bitmap_cls, imageRepWithData: &*tiff];
    if bitmap.is_null() {
        return Err(anyhow!("NSBitmapImageRep from TIFF returned nil"));
    }

    // NSPNGFileType = 4
    let png: *mut NSObject = msg_send![&*bitmap, representationUsingType: 4u64, properties: None::<&NSDictionary>];
    if png.is_null() {
        return Err(anyhow!("PNG representation failed"));
    }

    let len: usize = msg_send![&*png, length];
    let bytes: *const u8 = msg_send![&*png, bytes];
    if bytes.is_null() {
        return Err(anyhow!("PNG bytes are null"));
    }

    let slice = std::slice::from_raw_parts(bytes, len);
    let img = image::load_from_memory(slice)?;
    Ok(img.to_rgba8())
}
/// Set wallpaper with a crossfade transition.
/// Captures the current desktop, sets the new wallpaper, then fades from old to new.
/// Must be called on the main thread.
pub fn set_wallpaper_with_fade(mtm: MainThreadMarker, screen: &NSScreen, path: &Path) -> Result<()> {
    unsafe {
        // 1. Capture current screen content using CoreGraphics
        let desc: Retained<NSObject> = msg_send![screen, deviceDescription];
        let key = NSString::from_str("NSScreenNumber");
        let value: Retained<NSObject> = msg_send![&desc, objectForKey: &*key];
        let display_id: u32 = msg_send![&*value, unsignedIntValue];
        let cg_image = CGDisplayCreateImage(display_id);
        let has_screenshot = !cg_image.is_null();

        // 2. Set the new wallpaper via the standard API
        set_wallpaper(mtm, screen, path)?;

        // 3. Crossfade: show old wallpaper as overlay above the new one, then fade out
        if has_screenshot {
            let frame = screen.frame();

            // Create a borderless overlay window
            let win_cls = find_class("NSWindow")?;
            let window: *mut NSObject = msg_send![win_cls, alloc];
            let window: *mut NSObject = msg_send![window,
                initWithContentRect: frame,
                styleMask: 0u64,   // NSWindowStyleMaskBorderless
                backing: 2u64,     // NSBackingStoreBuffered
                defer: false
            ];

            if !window.is_null() {
                // Configure window: very low level (above desktop, below icons), transparent, no mouse events
                let _: () = msg_send![&*window, setLevel: -2000i64];
                let _: () = msg_send![&*window, setIgnoresMouseEvents: true];
                let _: () = msg_send![&*window, setOpaque: false];
                let _: () = msg_send![&*window, setAlphaValue: 1.0];
                let _: () = msg_send![&*window, makeKeyAndOrderFront: None::<&NSObject>];

                // Set layer contents to the captured CGImage (avoids needing NSImageView)
                let content_view: *mut NSObject = msg_send![&*window, contentView];
                let _: () = msg_send![&*content_view, setWantsLayer: true];
                let layer: *mut NSObject = msg_send![&*content_view, layer];
                if !layer.is_null() {
                    let _: () = msg_send![&*layer, setContents: cg_image as *mut NSObject];
                }

                // Animate: set duration on current NSAnimationContext
                if let Ok(ctx_cls) = find_class("NSAnimationContext") {
                    let ctx: *mut NSObject = msg_send![ctx_cls, currentContext];
                    let _: () = msg_send![&*ctx, setDuration: 0.35f64];
                }

                // Fade out via animator proxy
                let animator: *mut NSObject = msg_send![&*window, animator];
                let _: () = msg_send![&*animator, setAlphaValue: 0.0];

                // Close window after animation completes
                let _: () = msg_send![&*window, performSelector: sel!(close), withObject: None::<&NSObject>, afterDelay: 0.4f64];
            }

            CFRelease(cg_image);
        }

        Ok(())
    }
}

/// Download a random wallpaper from picsum.photos (free, no API key required).
/// Saves to the given directory and returns the file path.
/// Uses `curl` which is pre-installed on macOS.
///
/// 修复：添加 `-f` 使 curl 在 HTTP 错误时返回非零退出码；
/// 下载后校验文件是否为有效图片；失败时清理残留文件。
pub fn download_random_wallpaper(dir: &Path, url: &str) -> Result<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let filename = format!("wallflow_random_{}.jpg", ts);
    let output = dir.join(&filename);

    let status = std::process::Command::new("curl")
        // -f: 让 curl 在 HTTP 4xx/5xx 时返回非零退出码
        // -sSL: 静默模式 + 跟随重定向 + 显示错误
        .args(["-fsSL", "-o", &output.to_string_lossy(), url])
        .status()
        .map_err(|e| anyhow!("Failed to run curl: {}", e))?;

    if status.success() {
        // 校验文件大小 > 0
        let len = std::fs::metadata(&output)
            .map_err(|e| anyhow!("Failed to stat downloaded file: {}", e))?
            .len();
        if len == 0 {
            let _ = std::fs::remove_file(&output);
            return Err(anyhow!("Downloaded file is empty"));
        }
        // 校验文件是否为有效图片
        if image::open(&output).is_err() {
            let content = std::fs::read_to_string(&output).unwrap_or_default();
            let _ = std::fs::remove_file(&output);
            return Err(anyhow!(
                "Downloaded file is not a valid image ({} bytes): {}",
                len,
                content.lines().next().unwrap_or("(empty)").trim()
            ));
        }
        Ok(output)
    } else {
        // 失败时清理残留的空文件/无效文件
        let _ = std::fs::remove_file(&output);
        Err(anyhow!("curl download failed (exit code: {:?})", status.code()))
    }
}
