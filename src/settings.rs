use objc2::{define_class, msg_send, ClassType, MainThreadOnly, sel, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{
    NSButton, NSPopUpButton, NSSlider, NSTextField, NSView, NSWindow, NSWindowStyleMask, NSBackingStoreType, NSApplication
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString
};
use crate::config;

#[derive(Clone, Copy)]
pub struct RawControlPtrs {
    pub window: *mut NSWindow,
    pub dir_label: *mut NSTextField,
    pub interval_input: *mut NSTextField,
    pub mode_popup: *mut NSPopUpButton,
    pub random_wallpaper_url: *mut NSTextField,
    pub brightness_slider: *mut NSSlider,
    pub contrast_slider: *mut NSSlider,
    pub saturation_slider: *mut NSSlider,
    pub temperature_slider: *mut NSSlider,
    pub blur_slider: *mut NSSlider,
    
    pub brightness_val_label: *mut NSTextField,
    pub contrast_val_label: *mut NSTextField,
    pub saturation_val_label: *mut NSTextField,
    pub temperature_val_label: *mut NSTextField,
    pub blur_val_label: *mut NSTextField,
}

unsafe impl Send for RawControlPtrs {}
unsafe impl Sync for RawControlPtrs {}

pub struct SettingsTargetIvars {
    pub ptrs: std::sync::Mutex<Option<RawControlPtrs>>,
    pub mtm: MainThreadMarker,
    pub event_tx: std::sync::Mutex<Option<std::sync::mpsc::Sender<crate::app_event::AppEvent>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SettingsTargetIvars]
    pub struct SettingsTarget;

    impl SettingsTarget {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(SettingsTargetIvars {
                ptrs: std::sync::Mutex::new(None),
                // SAFETY: #[thread_kind = MainThreadOnly] guarantees init is called on the main thread
                mtm: unsafe { MainThreadMarker::new_unchecked() },
                event_tx: std::sync::Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(sliderChanged:))]
        unsafe fn slider_changed(&self, _sender: *mut NSObject) {
            let ptrs = {
                let lock = self.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                let b_val: f64 = msg_send![ptrs.brightness_slider, doubleValue];
                let c_val: f64 = msg_send![ptrs.contrast_slider, doubleValue];
                let s_val: f64 = msg_send![ptrs.saturation_slider, doubleValue];
                let t_val: f64 = msg_send![ptrs.temperature_slider, doubleValue];
                let bl_val: f64 = msg_send![ptrs.blur_slider, doubleValue];

                let b_str = NSString::from_str(&format!("{:.2}", b_val));
                let c_str = NSString::from_str(&format!("{:.2}", c_val));
                let s_str = NSString::from_str(&format!("{:.2}", s_val));
                let t_str = NSString::from_str(&format!("{:.2}", t_val));
                let bl_str = NSString::from_str(&format!("{:.1}", bl_val));

                let _: () = msg_send![ptrs.brightness_val_label, setStringValue: &*b_str];
                let _: () = msg_send![ptrs.contrast_val_label, setStringValue: &*c_str];
                let _: () = msg_send![ptrs.saturation_val_label, setStringValue: &*s_str];
                let _: () = msg_send![ptrs.temperature_val_label, setStringValue: &*t_str];
                let _: () = msg_send![ptrs.blur_val_label, setStringValue: &*bl_str];
            }
        }

        /// Handle direct numeric input in value fields.
        /// Reads the text field's tag (0-4) to identify which slider to update,
        /// then triggers sliderChanged to sync all displays.
        #[unsafe(method(textFieldChanged:))]
        unsafe fn text_field_changed(&self, sender: *mut NSObject) {
            let ptrs = {
                let lock = self.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                let tag: isize = msg_send![sender, tag];
                let text: Retained<NSString> = msg_send![sender, stringValue];
                if let Ok(value) = text.to_string().parse::<f64>() {
                    let clamped = match tag {
                        4 => value.clamp(0.0, 20.0),  // blur: 0-20
                        _ => value.clamp(-1.0, 1.0),   // others: -1.0 to 1.0
                    };
                    let slider: *mut NSSlider = match tag {
                        0 => ptrs.brightness_slider,
                        1 => ptrs.contrast_slider,
                        2 => ptrs.saturation_slider,
                        3 => ptrs.temperature_slider,
                        4 => ptrs.blur_slider,
                        _ => return,
                    };
                    let _: () = msg_send![slider, setDoubleValue: clamped];
                    // Update labels to reflect the new slider value
                    let _: () = msg_send![&*self, sliderChanged: std::ptr::null_mut::<NSObject>()];
                }
            }
        }

        #[unsafe(method(chooseDirClicked:))]
        unsafe fn choose_dir_clicked(&self, _sender: *mut NSObject) {
            let ptrs = {
                let lock = self.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                let current_ns: Retained<NSString> = msg_send![ptrs.dir_label, stringValue];
                let current = current_ns.to_string();
                let start_dir = if current.is_empty() { None } else { Some(std::path::PathBuf::from(current)) };

                let mtm = self.ivars().mtm;
                if let Some(path) = crate::util::run_open_directory_panel(mtm, "选择", "选择包含壁纸的文件夹", start_dir.as_deref()) {
                    let _: () = msg_send![ptrs.dir_label, setStringValue: &*NSString::from_str(&path.to_string_lossy())];
                }
            }
        }

        #[unsafe(method(cancelClicked:))]
        unsafe fn cancel_clicked(&self, _sender: *mut NSObject) {
            let ptrs = {
                let lock = self.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                let _: () = msg_send![ptrs.window, close];
            }
        }

        #[unsafe(method(applyClicked:))]
        unsafe fn apply_clicked(&self, _sender: *mut NSObject) {
            let ptrs = {
                let lock = self.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                let dir_ns: Retained<NSString> = msg_send![ptrs.dir_label, stringValue];
                let dir_str = dir_ns.to_string();

                let selected_idx: isize = msg_send![ptrs.mode_popup, indexOfSelectedItem];
                let mode = if selected_idx == 1 {
                    crate::library::SwitchMode::Random
                } else {
                    crate::library::SwitchMode::Sequential
                };

                let interval_ns: Retained<NSString> = msg_send![ptrs.interval_input, stringValue];
                let interval_secs: u64 = interval_ns.to_string().parse().unwrap_or(300);

                let url_ns: Retained<NSString> = msg_send![ptrs.random_wallpaper_url, stringValue];
                let url_str = url_ns.to_string();

                let brightness: f64 = msg_send![ptrs.brightness_slider, doubleValue];
                let contrast: f64 = msg_send![ptrs.contrast_slider, doubleValue];
                let saturation: f64 = msg_send![ptrs.saturation_slider, doubleValue];
                let temperature: f64 = msg_send![ptrs.temperature_slider, doubleValue];
                let blur: f64 = msg_send![ptrs.blur_slider, doubleValue];

                let new_config = config::Config {
                    wallpaper_dir: dir_str,
                    interval_secs,
                    mode,
                    random_wallpaper_url: url_str,
                    filters: config::FilterConfig {
                        brightness: brightness as f32,
                        contrast: contrast as f32,
                        saturation: saturation as f32,
                        temperature: temperature as f32,
                        blur: blur as f32,
                    }
                };

                if let Ok(content) = toml::to_string_pretty(&new_config) {
                    let config_path = config::get_config_path();
                    let _ = std::fs::write(config_path, content);
                }

                if let Some(ref tx) = *self.ivars().event_tx.lock().unwrap() {
                    crate::util::send_event(tx, crate::app_event::AppEvent::ConfigReloaded(new_config));
                }

                let _: () = msg_send![ptrs.window, close];
            }
        }
    }

    unsafe impl NSObjectProtocol for SettingsTarget {}
);

impl SettingsTarget {
    /// Set the event sender after construction. Must be called before using the target,
    /// otherwise the Apply button will silently fail to send config changes.
    /// Follows the same two-phase init pattern as TimerTarget.
    pub fn set_event_tx(&self, tx: std::sync::mpsc::Sender<crate::app_event::AppEvent>) {
        *self.ivars().event_tx.lock().unwrap() = Some(tx);
    }
}

#[allow(dead_code)]
pub struct SettingsPanel {
    window: Retained<NSWindow>,
    target: Retained<SettingsTarget>,
}

impl SettingsPanel {
    pub fn new(mtm: MainThreadMarker, event_tx: std::sync::mpsc::Sender<crate::app_event::AppEvent>) -> Self {
        unsafe {
            let window_rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(480.0, 500.0));
            let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
            
            let window: Retained<NSWindow> = {
                let alloc: *mut NSWindow = msg_send![NSWindow::class(), alloc];
                let raw: *mut NSWindow = msg_send![
                    alloc,
                    initWithContentRect: window_rect,
                    styleMask: style,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ];
                Retained::from_raw(raw).unwrap()
            };

            let _: () = msg_send![&*window, setTitle: &*NSString::from_str("WallFlow 设置")];
            let _: () = msg_send![&*window, setReleasedWhenClosed: false];

            let content_view: Retained<NSView> = NSView::new(mtm);
            let _: () = msg_send![&*window, setContentView: Some(&*content_view)];

            // Two-phase init: construct via ObjC new, then set event_tx separately.
            // This pattern matches TimerTarget and avoids ivars being overwritten by init.
            let target: Retained<SettingsTarget> = msg_send![SettingsTarget::class(), new];
            target.set_event_tx(event_tx);

            // Title Label
            let title_label = create_label(mtm, "WallFlow 设置", NSRect::new(NSPoint::new(20.0, 375.0), NSSize::new(200.0, 30.0)));
            if let Some(font_cls) = objc2::runtime::AnyClass::get(std::ffi::CStr::from_bytes_with_nul(b"NSFont\0").unwrap()) {
                let bold_font: Option<Retained<NSObject>> = msg_send![font_cls, boldSystemFontOfSize: 16.0f64];
                if let Some(font) = bold_font {
                    let _: () = msg_send![&*title_label, setFont: Some(&*font)];
                }
            }
            let _: () = msg_send![&*content_view, addSubview: &*title_label];

            // Directory
            let dir_title = create_label(mtm, "壁纸目录:", NSRect::new(NSPoint::new(20.0, 335.0), NSSize::new(80.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*dir_title];

            let dir_val = create_label(mtm, "", NSRect::new(NSPoint::new(110.0, 335.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*dir_val, setBezeled: true];
            let _: () = msg_send![&*dir_val, setDrawsBackground: true];
            let _: () = msg_send![&*content_view, addSubview: &*dir_val];

            let choose_btn = create_button(mtm, "选择...", NSRect::new(NSPoint::new(380.0, 332.0), NSSize::new(80.0, 24.0)));
            let _: () = msg_send![&*choose_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*choose_btn, setAction: Some(sel!(chooseDirClicked:))];
            let _: () = msg_send![&*content_view, addSubview: &*choose_btn];

            // Switch Mode
            let mode_title = create_label(mtm, "切换模式:", NSRect::new(NSPoint::new(20.0, 295.0), NSSize::new(80.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*mode_title];

            let mode_popup: Retained<NSPopUpButton> = {
                let alloc: *mut NSPopUpButton = msg_send![NSPopUpButton::class(), alloc];
                let popup_rect = NSRect::new(NSPoint::new(110.0, 292.0), NSSize::new(130.0, 26.0));
                let raw: *mut NSPopUpButton = msg_send![
                    alloc,
                    initWithFrame: popup_rect,
                    pullsDown: false
                ];
                Retained::from_raw(raw).unwrap()
            };
            let _: () = msg_send![&*mode_popup, addItemWithTitle: &*NSString::from_str("顺序切换")];
            let _: () = msg_send![&*mode_popup, addItemWithTitle: &*NSString::from_str("随机切换")];
            let _: () = msg_send![&*content_view, addSubview: &*mode_popup];

            // Interval input
            let interval_title = create_label(mtm, "轮播间隔(秒):", NSRect::new(NSPoint::new(20.0, 255.0), NSSize::new(100.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*interval_title];

            let interval_input = create_text_input(mtm, "300", NSRect::new(NSPoint::new(110.0, 252.0), NSSize::new(80.0, 22.0)));
            let _: () = msg_send![&*content_view, addSubview: &*interval_input];

            // 随机壁纸 URL
            let url_title = create_label(mtm, "随机壁纸 URL:", NSRect::new(NSPoint::new(20.0, 225.0), NSSize::new(90.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*url_title];

            let url_input = create_text_input(mtm, "", NSRect::new(NSPoint::new(110.0, 222.0), NSSize::new(350.0, 22.0)));
            // 让用户可以选中和复制 URL
            let _: () = msg_send![&*url_input, setSelectable: true];
            let _: () = msg_send![&*content_view, addSubview: &*url_input];

            // URL 调用规则说明
            let url_desc = create_label(mtm, "规则: API 需返回 image/* Content-Type 的图片直链, 支持 ?w=&h= 尺寸参数",
                NSRect::new(NSPoint::new(20.0, 207.0), NSSize::new(440.0, 16.0)));
            if let Some(font_cls) = objc2::runtime::AnyClass::get(std::ffi::CStr::from_bytes_with_nul(b"NSFont\0").unwrap()) {
                let small_font: Option<Retained<NSObject>> = msg_send![font_cls, systemFontOfSize: 10.0f64];
                if let Some(font) = small_font {
                    let _: () = msg_send![&*url_desc, setFont: Some(&*font)];
                }
            }
            if let Some(color_cls) = objc2::runtime::AnyClass::get(std::ffi::CStr::from_bytes_with_nul(b"NSColor\0").unwrap()) {
                let gray_color: Option<Retained<NSObject>> = msg_send![color_cls, grayColor];
                if let Some(color) = gray_color {
                    let _: () = msg_send![&*url_desc, setTextColor: &*color];
                }
            }
            let _: () = msg_send![&*content_view, addSubview: &*url_desc];

            // Brightness Slider
            let b_title = create_label(mtm, "亮度:", NSRect::new(NSPoint::new(20.0, 185.0), NSSize::new(60.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*b_title];
            let b_slider = create_slider(mtm, -1.0, 1.0, 0.0, NSRect::new(NSPoint::new(110.0, 183.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*b_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*b_slider, setAction: Some(sel!(sliderChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*b_slider];
            let b_val_label = create_text_input(mtm, "0.00", NSRect::new(NSPoint::new(380.0, 185.0), NSSize::new(60.0, 22.0)));
            let _: () = msg_send![&*b_val_label, setTag: 0isize];
            let _: () = msg_send![&*b_val_label, setTarget: Some(&*target)];
            let _: () = msg_send![&*b_val_label, setAction: Some(sel!(textFieldChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*b_val_label];

            // Contrast Slider
            let c_title = create_label(mtm, "对比度:", NSRect::new(NSPoint::new(20.0, 150.0), NSSize::new(60.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*c_title];
            let c_slider = create_slider(mtm, -1.0, 1.0, 0.0, NSRect::new(NSPoint::new(110.0, 148.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*c_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*c_slider, setAction: Some(sel!(sliderChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*c_slider];
            let c_val_label = create_text_input(mtm, "0.00", NSRect::new(NSPoint::new(380.0, 150.0), NSSize::new(60.0, 22.0)));
            let _: () = msg_send![&*c_val_label, setTag: 1isize];
            let _: () = msg_send![&*c_val_label, setTarget: Some(&*target)];
            let _: () = msg_send![&*c_val_label, setAction: Some(sel!(textFieldChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*c_val_label];

            // Saturation Slider
            let s_title = create_label(mtm, "饱和度:", NSRect::new(NSPoint::new(20.0, 115.0), NSSize::new(60.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*s_title];
            let s_slider = create_slider(mtm, -1.0, 1.0, 0.0, NSRect::new(NSPoint::new(110.0, 113.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*s_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*s_slider, setAction: Some(sel!(sliderChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*s_slider];
            let s_val_label = create_text_input(mtm, "0.00", NSRect::new(NSPoint::new(380.0, 115.0), NSSize::new(60.0, 22.0)));
            let _: () = msg_send![&*s_val_label, setTag: 2isize];
            let _: () = msg_send![&*s_val_label, setTarget: Some(&*target)];
            let _: () = msg_send![&*s_val_label, setAction: Some(sel!(textFieldChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*s_val_label];

            // Temperature Slider
            let t_title = create_label(mtm, "色温:", NSRect::new(NSPoint::new(20.0, 80.0), NSSize::new(60.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*t_title];
            let t_slider = create_slider(mtm, -1.0, 1.0, 0.0, NSRect::new(NSPoint::new(110.0, 78.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*t_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*t_slider, setAction: Some(sel!(sliderChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*t_slider];
            let t_val_label = create_text_input(mtm, "0.00", NSRect::new(NSPoint::new(380.0, 80.0), NSSize::new(60.0, 22.0)));
            let _: () = msg_send![&*t_val_label, setTag: 3isize];
            let _: () = msg_send![&*t_val_label, setTarget: Some(&*target)];
            let _: () = msg_send![&*t_val_label, setAction: Some(sel!(textFieldChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*t_val_label];

            // Blur Slider
            let bl_title = create_label(mtm, "模糊:", NSRect::new(NSPoint::new(20.0, 45.0), NSSize::new(60.0, 20.0)));
            let _: () = msg_send![&*content_view, addSubview: &*bl_title];
            let bl_slider = create_slider(mtm, 0.0, 20.0, 0.0, NSRect::new(NSPoint::new(110.0, 43.0), NSSize::new(260.0, 20.0)));
            let _: () = msg_send![&*bl_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*bl_slider, setAction: Some(sel!(sliderChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*bl_slider];
            let bl_val_label = create_text_input(mtm, "0.0", NSRect::new(NSPoint::new(380.0, 45.0), NSSize::new(60.0, 22.0)));
            let _: () = msg_send![&*bl_val_label, setTag: 4isize];
            let _: () = msg_send![&*bl_val_label, setTarget: Some(&*target)];
            let _: () = msg_send![&*bl_val_label, setAction: Some(sel!(textFieldChanged:))];
            let _: () = msg_send![&*content_view, addSubview: &*bl_val_label];

            // Bottom Buttons
            let cancel_btn = create_button(mtm, "取消", NSRect::new(NSPoint::new(270.0, 20.0), NSSize::new(90.0, 24.0)));
            let _: () = msg_send![&*cancel_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*cancel_btn, setAction: Some(sel!(cancelClicked:))];
            let _: () = msg_send![&*content_view, addSubview: &*cancel_btn];

            let apply_btn = create_button(mtm, "应用", NSRect::new(NSPoint::new(370.0, 20.0), NSSize::new(90.0, 24.0)));
            let _: () = msg_send![&*apply_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*apply_btn, setAction: Some(sel!(applyClicked:))];
            let _: () = msg_send![&*content_view, addSubview: &*apply_btn];

            // Save raw pointers in target ivars
            *target.ivars().ptrs.lock().unwrap() = Some(RawControlPtrs {
                window: Retained::as_ptr(&window) as *mut _,
                dir_label: Retained::as_ptr(&dir_val) as *mut _,
                interval_input: Retained::as_ptr(&interval_input) as *mut _,
                mode_popup: Retained::as_ptr(&mode_popup) as *mut _,
                random_wallpaper_url: Retained::as_ptr(&url_input) as *mut _,
                brightness_slider: Retained::as_ptr(&b_slider) as *mut _,
                contrast_slider: Retained::as_ptr(&c_slider) as *mut _,
                saturation_slider: Retained::as_ptr(&s_slider) as *mut _,
                temperature_slider: Retained::as_ptr(&t_slider) as *mut _,
                blur_slider: Retained::as_ptr(&bl_slider) as *mut _,

                brightness_val_label: Retained::as_ptr(&b_val_label) as *mut _,
                contrast_val_label: Retained::as_ptr(&c_val_label) as *mut _,
                saturation_val_label: Retained::as_ptr(&s_val_label) as *mut _,
                temperature_val_label: Retained::as_ptr(&t_val_label) as *mut _,
                blur_val_label: Retained::as_ptr(&bl_val_label) as *mut _,
            });

            Self { window, target }
        }
    }

    pub fn show(&self, config: &config::Config) {
        unsafe {
            let ptrs = {
                let lock = self.target.ivars().ptrs.lock().unwrap();
                *lock
            };
            if let Some(ptrs) = ptrs {
                // Populate fields
                let dir_ns = NSString::from_str(&config.wallpaper_dir);
                let _: () = msg_send![ptrs.dir_label, setStringValue: &*dir_ns];

                let interval_ns = NSString::from_str(&config.interval_secs.to_string());
                let _: () = msg_send![ptrs.interval_input, setStringValue: &*interval_ns];

                let url_ns = NSString::from_str(&config.random_wallpaper_url);
                let _: () = msg_send![ptrs.random_wallpaper_url, setStringValue: &*url_ns];

                let mode_idx: isize = match config.mode {
                    crate::library::SwitchMode::Random => 1,
                    _ => 0,
                };
                let _: () = msg_send![ptrs.mode_popup, selectItemAtIndex: mode_idx];

                let _: () = msg_send![ptrs.brightness_slider, setDoubleValue: config.filters.brightness as f64];
                let _: () = msg_send![ptrs.contrast_slider, setDoubleValue: config.filters.contrast as f64];
                let _: () = msg_send![ptrs.saturation_slider, setDoubleValue: config.filters.saturation as f64];
                let _: () = msg_send![ptrs.temperature_slider, setDoubleValue: config.filters.temperature as f64];
                let _: () = msg_send![ptrs.blur_slider, setDoubleValue: config.filters.blur as f64];

                // Trigger slider changed to update the value labels
                let _: () = msg_send![&*self.target, sliderChanged: std::ptr::null_mut::<NSObject>()];

                // Show window
                let _: () = msg_send![ptrs.window, makeKeyAndOrderFront: None::<&NSObject>];
                let app = NSApplication::sharedApplication(self.target.ivars().mtm);
                let _: () = msg_send![&app, activateIgnoringOtherApps: true];
            }
        }
    }
}

// Helpers for creating controls
unsafe fn create_label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::new(mtm);
    let _: () = msg_send![&*label, setFrame: frame];
    let text_ns = NSString::from_str(text);
    let _: () = msg_send![&*label, setStringValue: &*text_ns];
    let _: () = msg_send![&*label, setEditable: false];
    let _: () = msg_send![&*label, setBezeled: false];
    let _: () = msg_send![&*label, setDrawsBackground: false];
    label
}

unsafe fn create_text_input(mtm: MainThreadMarker, initial_text: &str, frame: NSRect) -> Retained<NSTextField> {
    let input = NSTextField::new(mtm);
    let _: () = msg_send![&*input, setFrame: frame];
    let initial_text_ns = NSString::from_str(initial_text);
    let _: () = msg_send![&*input, setStringValue: &*initial_text_ns];
    let _: () = msg_send![&*input, setEditable: true];
    let _: () = msg_send![&*input, setBezeled: true];
    input
}

unsafe fn create_slider(mtm: MainThreadMarker, min: f64, max: f64, val: f64, frame: NSRect) -> Retained<NSSlider> {
    let slider = NSSlider::new(mtm);
    let _: () = msg_send![&*slider, setFrame: frame];
    let _: () = msg_send![&*slider, setMinValue: min];
    let _: () = msg_send![&*slider, setMaxValue: max];
    let _: () = msg_send![&*slider, setDoubleValue: val];
    slider
}

unsafe fn create_button(mtm: MainThreadMarker, title: &str, frame: NSRect) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    let _: () = msg_send![&*button, setFrame: frame];
    let title_ns = NSString::from_str(title);
    let _: () = msg_send![&*button, setTitle: &*title_ns];
    let _: () = msg_send![&*button, setBezelStyle: 1u64];
    button
}
