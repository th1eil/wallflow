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
use std::time::{SystemTime, UNIX_EPOCH};

// CoreGraphics FFI for capturing screen content.
extern "C" {
    fn CGDisplayCreateImage(displayID: u32) -> *mut c_void;
    fn CFRelease(cf: *mut c_void);
    /// macOS 10.15+：查询本进程是否已获得「屏幕录制」授权，不触发任何弹窗。
    fn CGPreflightScreenCaptureAccess() -> bool;
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
    let png: *mut NSObject = msg_send![&*bitmap, representationUsingType: crate::util::NS_PNG_FILE_TYPE, properties: None::<&NSDictionary>];
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
    // 交叉淡入的前提是「先截一张旧桌面当过渡底图」，而整屏截图走的是
    // CGDisplayCreateImage —— 自 macOS 10.15 起废弃，且必须持有「屏幕录制」权限。
    // 无权限时它依旧会被同步调用一次（macOS 26 起 TCC 校验与捕获链路更重），
    // 拿回来的是空图，淡入层等于一张空白遮罩，观感就是「点了菜单要顿一下」。
    // 所以先做无弹窗的权限预检，没有就直接交给系统自带的切换过渡。
    if !unsafe { CGPreflightScreenCaptureAccess() } {
        return set_wallpaper(mtm, screen, path);
    }

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

/// 滤镜产物的落盘目录：`~/Library/Application Support/WallFlow/cache`。
///
/// 不能继续放 `$TMPDIR`：macOS 26 起系统壁纸状态由 Wallpaper Store 统一托管，
/// 里面存的是**文件 URL**（实测 `SystemDefault/Desktop/.../url/relative` 指向
/// 我们生成的临时文件）。放在临时目录时，下次切换或进程退出就会删掉文件，
/// 系统侧只剩一条失效路径；`/var/folders` 本身也会被系统定期清理。
pub fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join("Library/Application Support/WallFlow/cache");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// 启动时清掉上一轮遗留的滤镜产物，只保留最近几张。
///
/// 最近几张里必然包含系统当前引用的文件（上次退出前最后一次设置成功的），
/// 所以不能全删；更早的都是历史产物。
pub fn cleanup_stale_cache() {
    let Some(dir) = cache_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().unwrap_or(UNIX_EPOCH), entry.path()))
        })
        .collect();
    // 新的在前。保留最近 4 张而不是 1 张：多屏环境下每个屏幕各有一张产物，
    // 只留最新会把其它屏幕正在引用的文件删掉，那块屏重启后就回落成默认壁纸。
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(4) {
        let _ = std::fs::remove_file(path);
    }
}

/// 只在格式本身可能带透明通道时才去扫描像素：JPEG 不可能有 alpha，
/// 直接跳过对 2600 万像素的一次全量遍历。
pub fn source_may_have_alpha(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("png") | Some("webp") | Some("gif") | Some("apng")
    )
}

/// 是否存在真正半透明的像素（全 255 的 alpha 等同于不透明）。
pub fn has_transparent_pixels(img: &image::RgbaImage) -> bool {
    img.as_raw().chunks_exact(4).any(|p| p[3] != 255)
}

/// 按「铺满屏幕」语义把图片缩到接近屏幕物理像素尺寸。
///
/// 壁纸源图普遍 5000～6400 px 宽，而屏幕 backing 尺寸约 5120×3328：全尺寸过一遍
/// 滤镜等于白多算 1.5×线性 / 2.6×面积，模糊尤其吃亏（代价按面积平方增长）。
/// 只缩不放 —— 比屏幕小的图保持原样，避免放大损失清晰度。
pub fn fit_for_screen(img: image::RgbaImage, target: (u32, u32)) -> image::RgbaImage {
    let (sw, sh) = img.dimensions();
    let (tw, th) = target;
    if sw == 0 || sh == 0 || tw == 0 || th == 0 {
        return img;
    }
    // 取两个方向的较大者，保证缩小后仍然铺满整屏（与系统 Crop 语义一致）；
    // 取较小者会把画面缩到屏幕之内，系统再放大回去，等于白算一次。
    let scale = (tw as f32 / sw as f32).max(th as f32 / sh as f32);
    if scale >= 1.0 {
        return img;
    }
    let nw = ((sw as f32 * scale).round() as u32).max(1);
    let nh = ((sh as f32 * scale).round() as u32).max(1);
    // 用 box 平均（thumbnail）而不是 resize(Triangle)：同尺寸输出下实测
    // 129ms vs 502ms（9000×6000 → 5120×3413）。缩小时的面积平均本身就是
    // 合格的抗锯齿重采样，壁纸这种照片题材看不出差别。
    image::imageops::thumbnail(&img, nw, nh)
}

/// 把滤镜结果写成系统壁纸可直接读取的文件，返回实际路径。
///
/// 选 JPEG 而不是 PNG 的两个理由：
///   1. 编码成本：2600 万像素的 PNG 走单线程 deflate，是整条切换链路里最长的
///      一段 CPU；JPEG(q92) 快一个数量级，文件也小约 8～10 倍。
///   2. 系统侧成本：macOS 26 起壁纸由系统进程按文件 URL 解码渲染，11 MB / 26MP
///      的 PNG 每次切换都要重新解一遍，这部分时间不在我们的进程里，但同样计入
///      用户感受到的「切换慢」。
/// 原图真的带透明像素时才回落 PNG，避免透明区域被压成黑底。
pub fn write_wallpaper_output(
    img: &image::RgbaImage,
    stem: &str,
    keep_alpha: bool,
) -> Result<PathBuf> {
    let dir = cache_dir().ok_or_else(|| anyhow!("无法创建壁纸缓存目录"))?;

    if keep_alpha {
        let path = dir.join(format!("{stem}.png"));
        img.save(&path)?;
        return Ok(path);
    }

    let path = dir.join(format!("{stem}.jpg"));
    let (w, h) = img.dimensions();
    // 直接摊平成 RGB 三通道，避免 DynamicImage 再拷一份 RGBA（106 MB）
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in img.pixels() {
        rgb.extend_from_slice(&px.0[..3]);
    }

    // 写一半失败时把残留文件删掉：缓存目录里留 0 字节文件会让下次启动的
    // 「保留最新一张」策略误判。
    let result = (|| -> Result<()> {
        use std::io::Write;
        let file = std::fs::File::create(&path)?;
        let mut writer = std::io::BufWriter::new(file);
        {
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, 92);
            encoder.encode(&rgb, w, h, image::ColorType::Rgb8)?;
        }
        writer.flush()?;
        Ok(())
    })();

    if let Err(e) = result {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

static DOWNLOAD_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 确认后才提交到壁纸库；临时文件与目标同盘，提交不会暴露半写入图片或覆盖同名文件。
pub fn save_confirmed_random_wallpaper(source: &Path, dir: &Path) -> Result<PathBuf> {
    let destination = dir.join(source.file_name().ok_or_else(|| anyhow!("壁纸文件名无效"))?);
    let mut output = tempfile::NamedTempFile::new_in(dir)?;
    std::io::copy(&mut std::fs::File::open(source)?, &mut output)?;
    output.persist_noclobber(&destination)?;
    Ok(destination)
}

/// Download a random wallpaper from picsum.photos (free, no API key required).
/// Saves to the given directory and returns the file path.
/// Uses `curl` with failure status code checking and file validation.
pub fn download_random_wallpaper(dir: &Path, url: &str) -> Result<PathBuf> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = DOWNLOAD_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let filename = format!("wallflow_random_{}_{}_{}.jpg", std::process::id(), ts, seq);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 确认只保存到目标库，临时来源的生命周期仍由调用方管理。
    #[test]
    fn confirmed_wallpaper_is_copied_to_library() {
        let source_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("random.jpg");
        std::fs::write(&source, b"complete image bytes").unwrap();
        let saved = save_confirmed_random_wallpaper(&source, library.path()).unwrap();
        assert_eq!(saved, library.path().join("random.jpg"));
        assert_eq!(std::fs::read(&saved).unwrap(), std::fs::read(&source).unwrap());
        assert_eq!(std::fs::read_dir(library.path()).unwrap().count(), 1);
    }

    /// 同名冲突不可覆盖既有壁纸，也不能遗留半成品供轮播扫描。
    #[test]
    fn confirmed_wallpaper_never_overwrites_existing_file() {
        let source_dir = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("random.jpg");
        let existing = library.path().join("random.jpg");
        std::fs::write(&source, b"new image").unwrap();
        std::fs::write(&existing, b"original image").unwrap();
        assert!(save_confirmed_random_wallpaper(&source, library.path()).is_err());
        assert_eq!(std::fs::read(existing).unwrap(), b"original image");
        assert_eq!(std::fs::read_dir(library.path()).unwrap().count(), 1);
    }

    /// 验证随机壁纸下载链路（curl 下载 → 文件非空 → 有效图片）
    #[test]
    fn test_download_random_wallpaper() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let out = download_random_wallpaper(tmp.path(), "https://picsum.photos/1920/1080")
            .expect("download should succeed");
        assert!(out.exists(), "downloaded file should exist");
        let len = std::fs::metadata(&out).expect("metadata").len();
        assert!(len > 0, "downloaded file should not be empty");
        assert!(image::open(&out).is_ok(), "downloaded file should be a valid image");
        println!("Downloaded OK: {} ({} bytes)", out.display(), len);
    }

    /// 验证无效 URL 应返回错误且不残留文件
    #[test]
    fn test_download_invalid_url_fails() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let res = download_random_wallpaper(tmp.path(), "https://picsum.photos/404_not_exist");
        assert!(res.is_err(), "invalid URL should fail");
        assert_eq!(
            std::fs::read_dir(tmp.path()).expect("read dir").count(),
            0,
            "no residual file should remain"
        );
    }

    /// 降采样必须「只缩不放」，且缩小后仍铺满屏幕（与系统 Crop 语义一致）
    #[test]
    fn fit_for_screen_covers_without_upscaling() {
        let img = image::RgbaImage::from_pixel(6000, 4000, image::Rgba([10, 20, 30, 255]));
        let out = fit_for_screen(img, (5120, 3328));
        let (w, h) = out.dimensions();
        assert!(w >= 5120 && h >= 3328, "缩小后仍需铺满屏幕，实际 {}x{}", w, h);
        assert!(w <= 6000 && h <= 4000, "不应放大，实际 {}x{}", w, h);

        let small = image::RgbaImage::from_pixel(800, 600, image::Rgba([0, 0, 0, 255]));
        assert_eq!(fit_for_screen(small, (5120, 3328)).dimensions(), (800, 600));
    }

    /// alpha 判定看实际像素，不看扩展名：全不透明的 PNG 不该被写成慢路径
    #[test]
    fn alpha_detection_ignores_opaque_images() {
        assert!(source_may_have_alpha(Path::new("/tmp/a.png")));
        assert!(!source_may_have_alpha(Path::new("/tmp/a.jpg")));

        let opaque = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        assert!(!has_transparent_pixels(&opaque));

        let mut translucent = opaque.clone();
        translucent.put_pixel(1, 1, image::Rgba([1, 2, 3, 128]));
        assert!(has_transparent_pixels(&translucent));
    }

    /// 切换链路耗时基准（默认不跑，需手动触发）：
    /// `cargo test --release --offline bench_wallpaper_pipeline -- --ignored --nocapture`
    ///
    /// 取壁纸目录里体积最大的一张，分别量「旧路径 = 全尺寸滤镜 + PNG」与
    /// 「新路径 = 缩到屏幕尺寸 + JPEG」的解码、滤镜、编码耗时，用来验证优化
    /// 没有回退。目录可用 WALLFLOW_BENCH_DIR 覆盖。
    #[test]
    #[ignore]
    fn bench_wallpaper_pipeline() {
        use image::GenericImageView;
        use std::time::Instant;

        let dir = std::env::var("WALLFLOW_BENCH_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(home).join("Desktop/jpgs")
            });
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("跳过基准：{} 不存在", dir.display());
            return;
        };
        let Some(src) = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                    Some("jpg") | Some("jpeg") | Some("png")
                )
            })
            .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        else {
            eprintln!("跳过基准：{} 里没有图片", dir.display());
            return;
        };

        let t = Instant::now();
        let img = decode_image(&src).expect("decode");
        let t_decode = t.elapsed();
        let (w, h) = img.dimensions();
        println!(
            "源图 {}×{}（{} MP），解码 {:?}",
            w,
            h,
            (w as u64 * h as u64) / 1_000_000,
            t_decode
        );

        let pipeline = crate::color::FilterPipeline::new();
        let params = crate::color::FilterPreset::Vintage.to_params();

        // 旧路径的编码成本：把整张图按原尺寸写成 PNG（image crate 默认压缩）
        let tmp = tempfile::Builder::new().suffix(".png").tempfile().expect("tempfile");
        let t = Instant::now();
        img.save(tmp.path()).expect("png save");
        let t_png_full = t.elapsed();
        let png_len = std::fs::metadata(tmp.path()).map(|m| m.len()).unwrap_or(0);

        // 新路径：先缩到屏幕尺寸，再滤镜，再写 JPEG
        let t = Instant::now();
        let fitted = fit_for_screen(img, (5120, 3328));
        let t_resize = t.elapsed();

        let t = Instant::now();
        let filtered = pipeline.process_image(&fitted, &params).expect("filter");
        let t_filter = t.elapsed();

        let t = Instant::now();
        let out = write_wallpaper_output(&filtered, "wallflow_bench", false).expect("jpeg save");
        let t_jpeg = t.elapsed();
        let jpeg_len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        // 断言产物能被正常解码：这条路径交给系统的是 JPEG，编码参数写错
        // 会让壁纸直接设不上，属于必须验证的边界。
        let decoded = image::open(&out).expect("JPEG 产物必须可解码");
        assert_eq!(decoded.dimensions(), filtered.dimensions());
        let _ = std::fs::remove_file(&out);

        println!(
            "旧：全尺寸 PNG 编码 {:?}，{} MB",
            t_png_full,
            png_len / 1024 / 1024
        );
        println!(
            "新：降采样 {:?}（{}×{}）+ 滤镜 {:?} + JPEG 编码 {:?}，{:.1} MB",
            t_resize,
            fitted.width(),
            fitted.height(),
            t_filter,
            t_jpeg,
            jpeg_len as f64 / 1024.0 / 1024.0
        );
    }
}
