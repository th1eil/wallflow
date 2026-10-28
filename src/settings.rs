use objc2::{define_class, msg_send, ClassType, MainThreadOnly, sel, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{
    NSButton, NSColor, NSPopUpButton, NSScrollView, NSSlider, NSTextField, NSView, NSWindow,
    NSWindowStyleMask, NSBackingStoreType, NSApplication, NSStackView,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use crate::config;

#[derive(Clone, Copy)]
pub struct RawControlPtrs {
    pub window: *mut NSWindow,
    pub dir_label: *mut NSTextField,
    pub interval_input: *mut NSTextField,
    pub interval_hint: *mut NSTextField,
    pub mode_popup: *mut NSPopUpButton,
    pub random_wallpaper_url: *mut NSTextField,
    pub url_hint: *mut NSTextField,
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

    // ---- 时钟设置控件 ----
    pub clock_enabled_btn: *mut NSButton,
    pub clock_preset_popup: *mut NSPopUpButton,
    pub clock_primary_tz: *mut NSPopUpButton,
    pub clock_secondary_tz: *mut NSPopUpButton,
    /// 时间格式（24/12 小时制 × 是否带秒，四项合一）
    pub clock_format_popup: *mut NSPopUpButton,
    /// 字体颜色：预设下拉只是快捷回填，HEX 输入框才是应用时的唯一数据源
    pub clock_color_popup: *mut NSPopUpButton,
    pub clock_color_hex: *mut NSTextField,
    pub clock_color_hint: *mut NSTextField,
    /// 文字水平对齐（居中 / 靠右）。它同时决定自动对比色的采样依据：
    /// 居中 → 整张壁纸，靠右 → 文字所在的矩形。
    pub clock_align_popup: *mut NSPopUpButton,
    /// 「自动对比色」勾选框（壁纸对比不足时自动调整字体颜色）
    pub clock_auto_contrast_btn: *mut NSButton,
    /// 时间行字重（连续 NSFontWeight 滑块）
    pub clock_weight_slider: *mut NSSlider,
    pub clock_weight_val: *mut NSTextField,
    /// 时间/日期/副时区三行各自字号缩放
    pub clock_time_scale_slider: *mut NSSlider,
    pub clock_time_scale_val: *mut NSTextField,
    pub clock_date_scale_slider: *mut NSSlider,
    pub clock_date_scale_val: *mut NSTextField,
    pub clock_sec_scale_slider: *mut NSSlider,
    pub clock_sec_scale_val: *mut NSTextField,
    /// 毛玻璃背景开关 + 强度
    pub clock_frost_btn: *mut NSButton,
    pub clock_frost_opacity_slider: *mut NSSlider,
    pub clock_frost_opacity_val: *mut NSTextField,
    pub clock_opacity_slider: *mut NSSlider,
    pub clock_opacity_val: *mut NSTextField,
    pub clock_shadow_slider: *mut NSSlider,
    pub clock_shadow_val: *mut NSTextField,
    pub clock_offset_slider: *mut NSSlider,
    pub clock_offset_val: *mut NSTextField,
    /// 日期行显示农历开关
    pub clock_lunar_btn: *mut NSButton,
    /// 副时区行显示当地节假日开关
    pub clock_holiday_btn: *mut NSButton,

    // ---- 刘海黑色矩阵 ----
    pub notch_enabled_btn: *mut NSButton,
    pub notch_radius_slider: *mut NSSlider,
    /// 只读数值标签（pt）
    pub notch_radius_val: *mut NSTextField,
}

unsafe impl Send for RawControlPtrs {}
unsafe impl Sync for RawControlPtrs {}

/// 导航只保存视图指针，不持有配置或事件。四张卡片都由页面栈保活，
/// 按钮由固定页头保活；设置窗口销毁时 target 也随之销毁。
#[derive(Clone, Copy)]
struct SettingsNavPtrs {
    scroll: *mut NSScrollView,
    cards: [*mut NSView; 5],
    buttons: [*mut NSButton; 5],
    selections: [*mut NSView; 5],
}

unsafe impl Send for SettingsNavPtrs {}
unsafe impl Sync for SettingsNavPtrs {}

/// Slider tags so value inputs can find their slider back (0..4, blur = 4).
const SLIDER_BRIGHTNESS: isize = 0;
const SLIDER_CONTRAST: isize = 1;
const SLIDER_SATURATION: isize = 2;
const SLIDER_TEMPERATURE: isize = 3;
const SLIDER_BLUR: isize = 4;

/// Delay before a slider rest sends a live preview to the active screen.
const PREVIEW_DEBOUNCE_SECS: f64 = 0.25;
/// 刘海矩阵的预览要重渲染整张壁纸（解码 + 合成 + 编码，0.3～1s），
/// 比时钟预览贵一个数量级，防抖窗口相应放大。
const NOTCH_PREVIEW_DEBOUNCE_SECS: f64 = 0.4;

/// 时钟字体颜色预设（标签, HEX）。下拉选中即把 HEX 回填到输入框，
/// 用户仍可直接改为任意自定义色。
const CLOCK_COLOR_PRESETS: &[(&str, &str)] = &[
    ("白色", "#FFFFFF"),
    ("暖白", "#FFF3E0"),
    ("浅灰", "#D9D9D9"),
    ("中灰", "#8C8C8C"),
    ("深灰", "#3A3A3A"),
    ("黑色", "#000000"),
];

/// 时间格式下拉：索引 → (24 小时制, 是否带秒)
const CLOCK_FORMATS: [(bool, bool); 4] = [
    (true, false),  // 24 小时制
    (false, false), // 12 小时制
    (true, true),   // 24 小时制（含秒）
    (false, true),  // 12 小时制（含秒）
];

/// 常见时区（菜单标签, IANA 名称）。配置文件里仍可填任意 IANA 时区。
const COMMON_TIMEZONES: &[(&str, &str)] = &[
    ("北京 (UTC+8)", "Asia/Shanghai"),
    ("东京 (UTC+9)", "Asia/Tokyo"),
    ("首尔 (UTC+9)", "Asia/Seoul"),
    ("新加坡 (UTC+8)", "Asia/Singapore"),
    ("迪拜 (UTC+4)", "Asia/Dubai"),
    ("伦敦 (UTC+0)", "Europe/London"),
    ("巴黎 (UTC+1)", "Europe/Paris"),
    ("莫斯科 (UTC+3)", "Europe/Moscow"),
    ("悉尼 (UTC+10)", "Australia/Sydney"),
    ("纽约 (UTC-5)", "America/New_York"),
    ("洛杉矶 (UTC-8)", "America/Los_Angeles"),
];

pub struct SettingsTargetIvars {
    pub ptrs: std::sync::Mutex<Option<RawControlPtrs>>,
    nav: std::sync::Mutex<Option<SettingsNavPtrs>>,
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
                nav: std::sync::Mutex::new(None),
                // SAFETY: #[thread_kind = MainThreadOnly] guarantees init is called on the main thread
                mtm: unsafe { MainThreadMarker::new_unchecked() },
                event_tx: std::sync::Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// 分类按钮仅切换卡片的可见性，所有字段实例仍保留在页面栈中。
        /// 因而跨分类修改多个配置项后，按一次「应用」即可一起保存。
        #[unsafe(method(settingsSectionClicked:))]
        unsafe fn settings_section_clicked(&self, sender: *mut NSObject) {
            let index: isize = msg_send![sender, tag];
            if index >= 0 {
                self.select_section(index as usize);
            }
        }

        /// Refresh the five numeric labels from current slider values, then
        /// schedule a debounced live-preview push (cancel any pending one).
        #[unsafe(method(sliderChanged:))]
        unsafe fn slider_changed(&self, _sender: *mut NSObject) {
            self.refresh_value_labels();

            // Trailing-edge debounce: collapse continuous drags into one
            // PreviewFilters event 250ms after the last movement.
            let cls = <SettingsTarget as ClassType>::class();
            let _: () = msg_send![cls,
                cancelPreviousPerformRequestsWithTarget: &*self,
                selector: sel!(sendPreviewFilters:),
                object: std::ptr::null_mut::<NSObject>()
            ];
            let _: () = msg_send![&*self,
                performSelector: sel!(sendPreviewFilters:),
                withObject: std::ptr::null_mut::<NSObject>(),
                afterDelay: PREVIEW_DEBOUNCE_SECS
            ];
        }

        /// (performSelector target) Push the current slider combination as a
        /// live preview onto the active screen without touching config.toml.
        #[unsafe(method(sendPreviewFilters:))]
        unsafe fn send_preview_filters(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let params = read_filter_params(&ptrs);
            if let Some(ref tx) = *self.ivars().event_tx.lock().unwrap() {
                crate::util::send_event(tx, crate::app_event::AppEvent::PreviewFilters(params));
            }
        }

        /// Handle direct numeric input in value fields.
        /// Reads the text field's tag (0-4) to identify which slider to update.
        /// Invalid input turns the field's text red and is ignored until fixed.
        #[unsafe(method(textFieldChanged:))]
        unsafe fn text_field_changed(&self, sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let tag: isize = msg_send![sender, tag];
            let text: Retained<NSString> = msg_send![sender, stringValue];
            let field = sender as *mut NSTextField;

            match text.to_string().parse::<f64>() {
                Ok(value) => {
                    let clamped = match tag {
                        SLIDER_BLUR => value.clamp(0.0, 20.0),
                        _ => value.clamp(-1.0, 1.0),
                    };
                    let slider: *mut NSSlider = match tag {
                        SLIDER_BRIGHTNESS => ptrs.brightness_slider,
                        SLIDER_CONTRAST => ptrs.contrast_slider,
                        SLIDER_SATURATION => ptrs.saturation_slider,
                        SLIDER_TEMPERATURE => ptrs.temperature_slider,
                        SLIDER_BLUR => ptrs.blur_slider,
                        _ => return,
                    };
                    // Back to normal text color once the input is valid again
                    let _: () = msg_send![field, setTextColor: &*NSColor::labelColor()];
                    let _: () = msg_send![slider, setDoubleValue: clamped];
                    self.refresh_value_labels();
                    // Keep input live too: same debounce as dragging
                    let _: () = msg_send![&*self, sliderChanged: std::ptr::null_mut::<NSObject>()];
                }
                Err(_) => {
                    let _: () = msg_send![field, setTextColor: &*NSColor::redColor()];
                }
            }
        }

        /// 时钟外观滑块刷新数值标签，并做防抖实时预览。
        /// 不复用 `sliderChanged:`（那是滤镜通路，会把当前滤镜组合推给活动屏幕
        /// 重渲染壁纸）；这里走独立的 `sendPreviewClock:`，只重建时钟叠加层。
        #[unsafe(method(clockSliderChanged:))]
        unsafe fn clock_slider_changed(&self, _sender: *mut NSObject) {
            self.refresh_clock_value_labels();

            // Trailing-edge debounce：连续拖动聚合成一次 PreviewClock，
            // 取消上一次未执行的调度，避免每 1px 就重建一整套时钟窗口。
            let cls = <SettingsTarget as ClassType>::class();
            let _: () = msg_send![cls,
                cancelPreviousPerformRequestsWithTarget: &*self,
                selector: sel!(sendPreviewClock:),
                object: std::ptr::null_mut::<NSObject>()
            ];
            let _: () = msg_send![&*self,
                performSelector: sel!(sendPreviewClock:),
                withObject: std::ptr::null_mut::<NSObject>(),
                afterDelay: PREVIEW_DEBOUNCE_SECS
            ];
        }

        /// (performSelector target) 把当前面板所有时钟控件组合作为实时预览
        /// 推给时钟叠加层：只重建叠加层、不写盘，Apply 时才真正落盘。
        #[unsafe(method(sendPreviewClock:))]
        unsafe fn send_preview_clock(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let clock = read_clock_config(&ptrs);
            if let Some(ref tx) = *self.ivars().event_tx.lock().unwrap() {
                crate::util::send_event(tx, crate::app_event::AppEvent::PreviewClock(clock));
            }
        }

        /// 刘海矩阵的开关与圆角。
        ///
        /// 它画在壁纸图片里，每次预览都要重渲染整张壁纸，所以必须防抖：
        /// 拖圆角滑块期间只刷新数值标签，停手 0.4s 后才真正渲染一次。
        #[unsafe(method(notchChanged:))]
        unsafe fn notch_changed(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let notch = read_notch_config(&ptrs);
            let text = NSString::from_str(&format!("{:.0} pt", notch.corner_radius));
            let _: () = msg_send![ptrs.notch_radius_val, setStringValue: &*text];

            let cls = <SettingsTarget as ClassType>::class();
            let _: () = msg_send![cls,
                cancelPreviousPerformRequestsWithTarget: &*self,
                selector: sel!(sendPreviewNotch:),
                object: std::ptr::null_mut::<NSObject>()
            ];
            let _: () = msg_send![&*self,
                performSelector: sel!(sendPreviewNotch:),
                withObject: std::ptr::null_mut::<NSObject>(),
                afterDelay: NOTCH_PREVIEW_DEBOUNCE_SECS
            ];
        }

        /// (performSelector target) 防抖后真正下发一次刘海矩阵预览。
        #[unsafe(method(sendPreviewNotch:))]
        unsafe fn send_preview_notch(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let notch = read_notch_config(&ptrs);
            if let Some(ref tx) = *self.ivars().event_tx.lock().unwrap() {
                crate::util::send_event(tx, crate::app_event::AppEvent::PreviewNotch(notch));
            }
        }

        /// 颜色预设下拉 → 回填 HEX 输入框。仅回填，不写配置：Apply 时以 HEX 为准。
        #[unsafe(method(clockColorPresetChanged:))]
        unsafe fn clock_color_preset_changed(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let idx: isize = msg_send![ptrs.clock_color_popup, indexOfSelectedItem];
            if idx < 0 {
                return;
            }
            if let Some((_, hex)) = CLOCK_COLOR_PRESETS.get(idx as usize) {
                let _: () = msg_send![ptrs.clock_color_hex, setStringValue: &*NSString::from_str(hex)];
                // 选预设即视为合法输入，顺手清掉可能残留的报错提示
                set_hint_visible(ptrs.clock_color_hint, false);
            }
        }

        #[unsafe(method(chooseDirClicked:))]
        unsafe fn choose_dir_clicked(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let current_ns: Retained<NSString> = msg_send![ptrs.dir_label, stringValue];
            let current = current_ns.to_string();
            let start_dir = if current.is_empty() { None } else { Some(std::path::PathBuf::from(current)) };

            let mtm = self.ivars().mtm;
            if let Some(path) = crate::util::run_open_directory_panel(mtm, "选择", "选择包含壁纸的文件夹", start_dir.as_deref()) {
                let _: () = msg_send![ptrs.dir_label, setStringValue: &*NSString::from_str(&path.to_string_lossy())];
            }
        }

        #[unsafe(method(cancelClicked:))]
        unsafe fn cancel_clicked(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let _: () = msg_send![ptrs.window, close];
        }

        #[unsafe(method(applyClicked:))]
        unsafe fn apply_clicked(&self, _sender: *mut NSObject) {
            let Some(ptrs) = self.current_ptrs() else { return };
            let dir_ns: Retained<NSString> = msg_send![ptrs.dir_label, stringValue];
            let dir_str = dir_ns.to_string();

            let selected_idx: isize = msg_send![ptrs.mode_popup, indexOfSelectedItem];
            let mode = match selected_idx {
                1 => crate::library::SwitchMode::Random,
                2 => crate::library::SwitchMode::Manual,
                _ => crate::library::SwitchMode::Sequential,
            };

            // --- Validation: interval must be an integer >= 1 ---
            let interval_ns: Retained<NSString> = msg_send![ptrs.interval_input, stringValue];
            match interval_ns.to_string().trim().parse::<u64>() {
                Ok(v) if v >= 1 => {
                    let hint = ptrs.interval_hint;
                    let _: () = msg_send![hint, setHidden: true];
                    let _: () = msg_send![hint, setTextColor: &*NSColor::labelColor()];
                }
                _ => {
                    let hint = ptrs.interval_hint;
                    let _: () = msg_send![hint, setHidden: false];
                    let _: () = msg_send![hint, setTextColor: &*NSColor::redColor()];
                    let _: () = msg_send![ptrs.window, makeFirstResponder: ptrs.interval_input];
                    return; // keep the window open, don't write anything
                }
            }
            let interval_secs: u64 = interval_ns.to_string().trim().parse().unwrap_or(300);

            // --- Soft validation: URL scheme (warn but don't block) ---
            let url_ns: Retained<NSString> = msg_send![ptrs.random_wallpaper_url, stringValue];
            let url_str = url_ns.to_string();
            let url_ok = url_str.is_empty()
                || url_str.starts_with("http://")
                || url_str.starts_with("https://");
            set_hint_visible(ptrs.url_hint, !url_ok);
            if !url_ok {
                let orange = NSColor::systemOrangeColor();
                let _: () = msg_send![ptrs.url_hint, setTextColor: &*orange];
            }

            let brightness: f64 = msg_send![ptrs.brightness_slider, doubleValue];
            let contrast: f64 = msg_send![ptrs.contrast_slider, doubleValue];
            let saturation: f64 = msg_send![ptrs.saturation_slider, doubleValue];
            let temperature: f64 = msg_send![ptrs.temperature_slider, doubleValue];
            let blur: f64 = msg_send![ptrs.blur_slider, doubleValue];

            // ---- 时钟设置：HEX 硬校验先行（预览走宽松回退，写盘前必须拦下坏值）----
            let hex_ns: Retained<NSString> = msg_send![ptrs.clock_color_hex, stringValue];
            match crate::clock::canonical_hex(&hex_ns.to_string()) {
                Some(_) => {
                    set_hint_visible(ptrs.clock_color_hint, false);
                }
                None => {
                    set_hint_visible(ptrs.clock_color_hint, true);
                    let _: () = msg_send![ptrs.clock_color_hint, setTextColor: &*NSColor::redColor()];
                    let _: () = msg_send![ptrs.window, makeFirstResponder: ptrs.clock_color_hex];
                    return; // 保持窗口打开，不写入任何配置
                }
            }

            // 读回完整时钟配置（与实时预览共用 read_clock_config，保证所见即所得）
            let clock = read_clock_config(&ptrs);
            // 刘海矩阵同样与预览共用一份读取
            let notch = read_notch_config(&ptrs);

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
                },
                clock,
                notch,
            };

            // Suppress the watcher echo of our own write (see util)
            crate::util::mark_config_self_write();
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

    unsafe impl NSObjectProtocol for SettingsTarget {}
);

impl SettingsTarget {
    /// 在主线程更新单选态和内容高度。隐藏的 arrangedSubview 由 NSStackView
    /// 从布局中折叠；再将滚动位置归零，避免从长时钟页切回短页时停在空白处。
    fn select_section(&self, index: usize) {
        let Some(nav) = *self.ivars().nav.lock().unwrap() else { return };
        if index >= nav.cards.len() {
            return;
        }
        unsafe {
            for (i, ((card, button), selection)) in nav.cards.iter()
                .zip(nav.buttons.iter()).zip(nav.selections.iter()).enumerate() {
                let _: () = msg_send![*card, setHidden: i != index];
                // 选中背景与按钮分层：导航的点击区不会随背景隐藏而消失。
                let _: () = msg_send![*selection, setHidden: i != index];
                let color = if i == index {
                    NSColor::labelColor()
                } else {
                    NSColor::secondaryLabelColor()
                };
                let _: () = msg_send![*button, setContentTintColor: Some(&*color)];
            }
            let clip: *mut NSObject = msg_send![nav.scroll, contentView];
            let _: () = msg_send![clip, scrollToPoint: NSPoint::new(0.0, 0.0)];
            let _: () = msg_send![nav.scroll, reflectScrolledClipView: clip];
        }
    }

    /// Snapshot the raw control pointers, if wiring is complete.
    fn current_ptrs(&self) -> Option<RawControlPtrs> {
        *self.ivars().ptrs.lock().unwrap()
    }

    /// Copy current slider values into the adjacent numeric labels.
    fn refresh_value_labels(&self) {
        let Some(ptrs) = self.current_ptrs() else { return };
        unsafe {
            let b_val: f64 = msg_send![ptrs.brightness_slider, doubleValue];
            let c_val: f64 = msg_send![ptrs.contrast_slider, doubleValue];
            let s_val: f64 = msg_send![ptrs.saturation_slider, doubleValue];
            let t_val: f64 = msg_send![ptrs.temperature_slider, doubleValue];
            let bl_val: f64 = msg_send![ptrs.blur_slider, doubleValue];

            let pairs = [
                (ptrs.brightness_val_label, format!("{:.2}", b_val)),
                (ptrs.contrast_val_label, format!("{:.2}", c_val)),
                (ptrs.saturation_val_label, format!("{:.2}", s_val)),
                (ptrs.temperature_val_label, format!("{:.2}", t_val)),
                (ptrs.blur_val_label, format!("{:.1}", bl_val)),
            ];
            for (label, text) in pairs {
                let s = NSString::from_str(&text);
                let _: () = msg_send![label, setStringValue: &*s];
            }
        }
    }

    /// 时钟外观滑块的数值标签（只读展示，不接受手工输入）。
    /// 格式带单位感：字号 ×1.00、透明度 0.95、阴影 0.45、位置 +0.00。
    fn refresh_clock_value_labels(&self) {
        let Some(ptrs) = self.current_ptrs() else { return };
        unsafe {
            let weight: f64 = msg_send![ptrs.clock_weight_slider, doubleValue];
            let time_scale: f64 = msg_send![ptrs.clock_time_scale_slider, doubleValue];
            let date_scale: f64 = msg_send![ptrs.clock_date_scale_slider, doubleValue];
            let sec_scale: f64 = msg_send![ptrs.clock_sec_scale_slider, doubleValue];
            let frost: f64 = msg_send![ptrs.clock_frost_opacity_slider, doubleValue];
            let opacity: f64 = msg_send![ptrs.clock_opacity_slider, doubleValue];
            let shadow: f64 = msg_send![ptrs.clock_shadow_slider, doubleValue];
            let offset: f64 = msg_send![ptrs.clock_offset_slider, doubleValue];

            let pairs = [
                (ptrs.clock_weight_val, format!("{:.2}", weight)),
                (ptrs.clock_time_scale_val, format!("×{:.2}", time_scale)),
                (ptrs.clock_date_scale_val, format!("×{:.2}", date_scale)),
                (ptrs.clock_sec_scale_val, format!("×{:.2}", sec_scale)),
                (ptrs.clock_frost_opacity_val, format!("{:.2}", frost)),
                (ptrs.clock_opacity_val, format!("{:.2}", opacity)),
                (ptrs.clock_shadow_val, format!("{:.2}", shadow)),
                (ptrs.clock_offset_val, format!("{:+.2}", offset)),
            ];
            for (label, text) in pairs {
                let s = NSString::from_str(&text);
                let _: () = msg_send![label, setStringValue: &*s];
            }
        }
    }

    /// Set the event sender after construction. Must be called before using the target,
    /// otherwise the Apply button will silently fail to send config changes.
    /// Follows the same two-phase init pattern as TimerTarget.
    pub fn set_event_tx(&self, tx: std::sync::mpsc::Sender<crate::app_event::AppEvent>) {
        *self.ivars().event_tx.lock().unwrap() = Some(tx);
    }
}

fn read_filter_params(ptrs: &RawControlPtrs) -> crate::color::FilterParams {
    unsafe {
        let brightness: f64 = msg_send![ptrs.brightness_slider, doubleValue];
        let contrast: f64 = msg_send![ptrs.contrast_slider, doubleValue];
        let saturation: f64 = msg_send![ptrs.saturation_slider, doubleValue];
        let temperature: f64 = msg_send![ptrs.temperature_slider, doubleValue];
        let blur: f64 = msg_send![ptrs.blur_slider, doubleValue];
        crate::color::FilterParams {
            brightness: (brightness as f32) * 100.0,
            contrast: (contrast as f32) * 100.0,
            saturation: (saturation as f32) * 100.0,
            temperature: (temperature as f32) * 100.0,
            blur: blur as f32,
        }
    }
}

/// 从设置面板控件读回完整的时钟配置。
///
/// 实时预览（`sendPreviewClock:`）与 Apply 共用这一份读取，保证「预览看到的效果」
/// 和「Apply 落地」完全一致。颜色 HEX 采用宽松策略：非法时回退到现有配置值
/// （预览不能因一个坏颜色而中止），写盘前的硬校验仍留在 `apply_clicked` 里。
fn read_clock_config(ptrs: &RawControlPtrs) -> config::ClockConfig {
    unsafe {
        let existing_clock = config::load_or_create_config()
            .map(|c| c.clock)
            .unwrap_or_default();

        let clock_on: isize = msg_send![ptrs.clock_enabled_btn, state];
        let auto_contrast_on: isize = msg_send![ptrs.clock_auto_contrast_btn, state];
        // 对齐方式：索引 0 = 居中，其余（含未选中）都按靠右处理 —— 靠右是历史行为
        let align_idx: isize = msg_send![ptrs.clock_align_popup, indexOfSelectedItem];
        let text_align = if align_idx == 0 {
            config::TextAlign::Center
        } else {
            config::TextAlign::Right
        };
        // 样式预设：tag 0=极简 1=标准 2=双时区；自定义(-1) 保留现有配置
        let preset_item: *mut NSObject = msg_send![ptrs.clock_preset_popup, selectedItem];
        let preset_tag: isize = if preset_item.is_null() {
            2
        } else {
            msg_send![preset_item, tag]
        };
        let (show_date, secondary_enabled) = match preset_tag {
            0 => (false, false),
            1 => (true, false),
            2 => (true, true),
            _ => (existing_clock.show_date, existing_clock.secondary_enabled),
        };

        // 时间格式：24/12 小时制 × 是否带秒，四项合一，读回选中的索引
        let format_idx: isize = msg_send![ptrs.clock_format_popup, indexOfSelectedItem];
        let (use_24_hour, show_seconds) =
            CLOCK_FORMATS.get(format_idx.max(0) as usize).copied().unwrap_or((true, false));

        // 颜色以 HEX 输入框为唯一数据源（预设下拉只是回填）；非法时回退现有值
        let hex_ns: Retained<NSString> = msg_send![ptrs.clock_color_hex, stringValue];
        let text_color = crate::clock::canonical_hex(&hex_ns.to_string())
            .unwrap_or_else(|| existing_clock.text_color.clone());

        let time_font_weight: f64 = msg_send![ptrs.clock_weight_slider, doubleValue];
        let time_scale: f64 = msg_send![ptrs.clock_time_scale_slider, doubleValue];
        let date_scale: f64 = msg_send![ptrs.clock_date_scale_slider, doubleValue];
        let secondary_scale: f64 = msg_send![ptrs.clock_sec_scale_slider, doubleValue];
        let frosted_on: isize = msg_send![ptrs.clock_frost_btn, state];
        let frosted_opacity: f64 = msg_send![ptrs.clock_frost_opacity_slider, doubleValue];
        let text_opacity: f64 = msg_send![ptrs.clock_opacity_slider, doubleValue];
        let shadow_opacity: f64 = msg_send![ptrs.clock_shadow_slider, doubleValue];
        let vertical_offset: f64 = msg_send![ptrs.clock_offset_slider, doubleValue];
        let lunar_on: isize = msg_send![ptrs.clock_lunar_btn, state];
        let holiday_on: isize = msg_send![ptrs.clock_holiday_btn, state];

        config::ClockConfig {
            enabled: clock_on == 1,
            primary_timezone: selected_timezone(ptrs.clock_primary_tz, true),
            secondary_enabled,
            show_date,
            secondary_timezone: selected_timezone(ptrs.clock_secondary_tz, false),
            // 副时区标签不在面板中编辑，保留现有值
            secondary_label: existing_clock.secondary_label,
            use_24_hour,
            show_seconds,
            show_lunar: lunar_on == 1,
            show_holiday: holiday_on == 1,
            text_color,
            time_scale: time_scale as f32,
            date_scale: date_scale as f32,
            secondary_scale: secondary_scale as f32,
            text_opacity: text_opacity as f32,
            shadow_opacity: shadow_opacity as f32,
            vertical_offset: vertical_offset as f32,
            time_font_weight: time_font_weight as f32,
            frosted_glass: frosted_on == 1,
            frosted_opacity: frosted_opacity as f32,
            text_align,
            auto_contrast: auto_contrast_on == 1,
        }
    }
}

/// 从设置面板读回刘海矩阵配置。
///
/// 只由开关与圆角滑块组成，两项都是即时生效的轻量属性，因此预览与 Apply
/// 共用这一份读取，保证「拖滑块看到的」与「写进 config.toml 的」一致。
fn read_notch_config(ptrs: &RawControlPtrs) -> config::NotchConfig {
    unsafe {
        let on: isize = msg_send![ptrs.notch_enabled_btn, state];
        let radius: f64 = msg_send![ptrs.notch_radius_slider, doubleValue];
        config::NotchConfig {
            enabled: on == 1,
            corner_radius: radius as f32,
        }
    }
}

#[allow(dead_code)]
pub struct SettingsPanel {    window: Retained<NSWindow>,
    target: Retained<SettingsTarget>,
}

impl SettingsPanel {
    pub fn new(mtm: MainThreadMarker, event_tx: std::sync::mpsc::Sender<crate::app_event::AppEvent>) -> Self {
        unsafe {
            // 侧栏与内容区共用窗口宽度；表单主控件伸缩，较小屏幕仍可滚动。
            // 紧凑检查从目标尺寸直接构造窗口，避免“先大后小”的测试路径
            // 把初始化和 autoresizing 的问题混在一起。
            let compact = std::env::var_os("WF_DEBUG_SHOW").is_some()
                && std::env::var_os("WF_DEBUG_COMPACT").is_some();
            let initial_size = if compact { NSSize::new(760.0, 500.0) }
                else { NSSize::new(820.0, 550.0) };
            let window_rect = NSRect::new(NSPoint::new(200.0, 200.0), initial_size);
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

            let _: () = msg_send![&*window, setTitle: &*NSString::from_str("WallFlow")];
            let _: () = msg_send![&*window, setReleasedWhenClosed: false];

            let target: Retained<SettingsTarget> = msg_send![SettingsTarget::class(), new];
            target.set_event_tx(event_tx);

            // ---- 视觉骨架：可滚动内容区 + 分区卡片 + 固定底部栏 ----
            // 旧实现把 28 行控件平铺进一个垂直 stack，靠裁剪窗口高度硬塞：
            // 1080p 屏上窗口贴着屏幕底边，底部按钮有被裁风险；所有行等权，
            // 没有层级，打开面板找不到重点。
            let root = NSView::new(mtm);
            let _: () = msg_send![&*window, setContentView: Some(&*root)];
            // contentView 的尺寸必须由窗口单向驱动。
            // NSView::new 的初始 frame 是零，而滚动页内部又是「行宽 = 页宽」
            // 的循环约束；若不及时钉住 frame，窗口在 orderFront 时会拿子视图
            // 的 fitting 宽度反推自身宽度，一路退化到标题栏最小值（实测 50pt）。
            let win_frame: NSRect = msg_send![&*window, frame];
            let mut content_rect: NSRect =
                msg_send![&*window, contentRectForFrameRect: win_frame];
            // contentRectForFrameRect: 返回的是**屏幕坐标系**下的矩形，origin
            // 是窗口在屏幕上的位置。contentView 的坐标系原点在内容区左下角，
            // 直接套用会把整个内容区平移一个屏幕坐标（实测右移 200pt、
            // 左侧留白、右侧被裁）。这里只取尺寸，origin 归零。
            content_rect.origin = NSPoint::new(0.0, 0.0);
            root.setFrame(content_rect);
            // NSViewWidthSizable(2) | NSViewHeightSizable(16)
            let _: () = msg_send![&*root, setAutoresizingMask: 18u64];
            // 关键：setContentView: 会把 contentView 交给 Auto Layout 接管，
            // 此后窗口尺寸由内容的 fitting size 反向驱动。滚动页内部是
            // 「行宽 = 页宽」的循环约束，fitting 宽度解不出来，窗口一路退化
            // 到标题栏最小宽度（实测 50pt）。强制回到 autoresizing 模式，
            // 让窗口尺寸只由 setContentSize 决定。
            let _: () = msg_send![&*root,
                setTranslatesAutoresizingMaskIntoConstraints: true];
            // contentView 必须给出尺寸下限：窗口在 orderFront 时会按
            // contentView 的 fitting 尺寸做一次 size-to-fit，而滚动页的
            // fitting 宽度是 0，结果被标题栏的最小宽度（50pt）兜底。
            // 这两条约束让 content-layout 模式解出 680×560 以上的尺寸。
            root.widthAnchor()
                .constraintGreaterThanOrEqualToConstant(760.0)
                .setActive(true);
            root.heightAnchor()
                .constraintGreaterThanOrEqualToConstant(500.0)
                .setActive(true);

            let (sidebar, tabs, selections) = crate::settings_ui::navigation_sidebar(mtm);
            root.addSubview(&sidebar);
            for button in &tabs {
                let _: () = msg_send![&**button, setTarget: Some(&*target)];
                let _: () = msg_send![&**button, setAction: Some(sel!(settingsSectionClicked:))];
            }

            let (scroll, page) = crate::settings_ui::scroll_page(mtm);
            root.addSubview(&scroll);

            let mut column = crate::settings_ui::SectionColumn::new(page.clone());
            column.section(mtm, "壁纸库", "选择来源并设置自动轮播", "photo.on.rectangle");

            // Directory row: 壁纸目录 [path field][选择...]
            let dir_val = make_input(mtm, "");
            let _: () = msg_send![&*dir_val, setEditable: false];
            let _: () = msg_send![&*dir_val, setBezeled: true];
            let _: () = msg_send![&*dir_val, setDrawsBackground: true];
            let choose_btn = make_button(mtm, "选择...");
            let _: () = msg_send![&*choose_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*choose_btn, setAction: Some(sel!(chooseDirClicked:))];
            let dir_row = row(mtm, "壁纸目录:", vec![
                (&dir_val as &NSView, Some(350.0)),
                (&choose_btn as &NSView, None),
            ]);
            column.add(&dir_row);

            // Mode popup now includes 手动 (previously missing while supported in config)
            let mode_popup: Retained<NSPopUpButton> = {
                let alloc: *mut NSPopUpButton = msg_send![NSPopUpButton::class(), alloc];
                let raw: *mut NSPopUpButton = msg_send![
                    alloc,
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(140.0, 26.0)),
                    pullsDown: false
                ];
                Retained::from_raw(raw).unwrap()
            };
            let _: () = msg_send![&*mode_popup, addItemWithTitle: &*NSString::from_str("顺序切换")];
            let _: () = msg_send![&*mode_popup, addItemWithTitle: &*NSString::from_str("随机切换")];
            let _: () = msg_send![&*mode_popup, addItemWithTitle: &*NSString::from_str("手动")];
            let mode_row = row(mtm, "切换模式:", vec![(&mode_popup as &NSView, None)]);
            column.add(&mode_row);

            // Interval + hard validation hint
            let interval_input = make_input(mtm, "300");
            let interval_hint = make_label(mtm, "间隔需为不小于 1 的整数");
            apply_color(&interval_hint, NSColor::redColor());
            let _: () = msg_send![&*interval_hint, setHidden: true];
            let interval_row = row(mtm, "轮播间隔(秒):", vec![
                (&interval_input as &NSView, Some(80.0)),
                (&interval_hint as &NSView, None),
            ]);
            column.add(&interval_row);

            // Random wallpaper URL + soft warning
            let url_input = make_input(mtm, "");
            let _: () = msg_send![&*url_input, setSelectable: true];
            let url_desc = make_small_label(mtm,
                "图片直链需返回 image/*，可用 ?w=&h= 指定尺寸");
            let url_hint = make_small_label(mtm, "URL 应以 http:// 或 https:// 开头");
            apply_color(&url_hint, NSColor::systemOrangeColor());
            let _: () = msg_send![&*url_hint, setHidden: true];
            let url_row = row(mtm, "随机壁纸 URL:", vec![(&url_input as &NSView, Some(450.0))]);
            column.add(&url_row);
            column.add(&row(mtm, "", vec![(&url_desc as &NSView, None)]));
            let url_hint_row = row(mtm, "", vec![(&url_hint as &NSView, None)]);
            url_hint_row.setHidden(true);
            column.add(&url_hint_row);

            // Sliders — bindings are declared here and assigned by the macro so
            // they stay in scope for the RawControlPtrs wiring below.
            column.section(mtm, "图像滤镜", "微调壁纸的色彩与清晰度", "camera.filters");
            let (b_slider, b_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (c_slider, c_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (s_slider, s_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (t_slider, t_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (bl_slider, bl_val): (Retained<NSSlider>, Retained<NSTextField>);
            macro_rules! slider_row {
                ($name:expr, $slider:ident, $val:ident, $min:expr, $max:expr, $tag:expr) => {{
                    $slider = make_slider(mtm, $min, $max, 0.0);
                    let _: () = msg_send![&*$slider, setTarget: Some(&*target)];
                    let _: () = msg_send![&*$slider, setAction: Some(sel!(sliderChanged:))];
                    $val = make_input(mtm, "");
                    let _: () = msg_send![&*$val, setTag: $tag];
                    let _: () = msg_send![&*$val, setTarget: Some(&*target)];
                    let _: () = msg_send![&*$val, setAction: Some(sel!(textFieldChanged:))];
                    let r = row(
                        mtm,
                        $name,
                        vec![
                            (&$slider as &NSView, Some(330.0)),
                            (&$val as &NSView, Some(64.0)),
                        ],
                    );
                    column.add(&r);
                }};
            }
            slider_row!("亮度:", b_slider, b_val, -1.0, 1.0, SLIDER_BRIGHTNESS);
            slider_row!("对比度:", c_slider, c_val, -1.0, 1.0, SLIDER_CONTRAST);
            slider_row!("饱和度:", s_slider, s_val, -1.0, 1.0, SLIDER_SATURATION);
            slider_row!("色温:", t_slider, t_val, -1.0, 1.0, SLIDER_TEMPERATURE);
            slider_row!("模糊:", bl_slider, bl_val, 0.0, 20.0, SLIDER_BLUR);

            // ---- 时钟设置 ----
            // 分区标题由卡片抬头承担，这里不再单独插一行加粗小字
            column.section(mtm, "桌面时钟", "选择时间、日期与时区信息", "clock");

            let clock_enabled_btn = make_checkbox(mtm, "在壁纸中央显示时钟");
            let clock_row = row(mtm, "时钟开关:", vec![(&clock_enabled_btn as &NSView, None)]);
            column.add(&clock_row);

            // 样式预设：tag 0=极简 1=标准 2=双时区（打开面板时若不匹配任何预设，会临时插入「自定义」tag=-1）
            let preset_popup = make_popup(mtm, &["极简（仅时间）", "标准（时间 + 日期）", "双时区（时间 + 日期 + 世界时钟）"]);
            for tag in 0..3isize {
                let item: *mut NSObject = msg_send![&*preset_popup, itemAtIndex: tag];
                let _: () = msg_send![item, setTag: tag];
            }
            let preset_row = row(mtm, "样式预设:", vec![(&preset_popup as &NSView, None)]);
            column.add(&preset_row);

            let primary_tz_popup = make_tz_popup(mtm, true);
            let primary_tz_row = row(mtm, "主时区:", vec![(&primary_tz_popup as &NSView, None)]);
            column.add(&primary_tz_row);

            let secondary_tz_popup = make_tz_popup(mtm, false);
            let secondary_tz_row = row(mtm, "副时区:", vec![(&secondary_tz_popup as &NSView, None)]);
            column.add(&secondary_tz_row);

            // 时间格式：原来的「24 小时制」勾选框换成四选一（含秒）。勾选框表达不了
            // 「12 小时制」这个正向选择，也装不下秒位，下拉把两个维度一次说清。
            let format_popup = make_popup(mtm, &[
                "24 小时制（17:34）",
                "12 小时制（下午 5:34）",
                "24 小时制（含秒）",
                "12 小时制（含秒）",
            ]);
            for tag in 0..CLOCK_FORMATS.len() as isize {
                let item: *mut NSObject = msg_send![&*format_popup, itemAtIndex: tag];
                let _: () = msg_send![item, setTag: tag];
            }
            let format_row = row(mtm, "时间格式:", vec![(&format_popup as &NSView, Some(230.0))]);
            column.add(&format_row);

            // 外观选项单独成页：基础时钟设置不再被十几项滑块推到屏幕下方。
            column.section(mtm, "外观样式", "调整文字表现和背景效果", "paintbrush.pointed");

            // 字体颜色：预设下拉（快捷回填）+ HEX 输入框（应用时的唯一数据源）
            let color_titles: Vec<&str> = CLOCK_COLOR_PRESETS.iter().map(|(l, _)| *l).collect();
            let color_popup = make_popup(mtm, &color_titles);
            for tag in 0..CLOCK_COLOR_PRESETS.len() as isize {
                let item: *mut NSObject = msg_send![&*color_popup, itemAtIndex: tag];
                let _: () = msg_send![item, setTag: tag];
            }
            let _: () = msg_send![&*color_popup, setTarget: Some(&*target)];
            let _: () = msg_send![&*color_popup, setAction: Some(sel!(clockColorPresetChanged:))];
            let color_hex = make_input(mtm, "#FFFFFF");
            let color_hint = make_small_label(mtm, "颜色格式应为 #RRGGBB 或 #RGB");
            apply_color(&color_hint, NSColor::redColor());
            let _: () = msg_send![&*color_hint, setHidden: true];
            let color_row = row(mtm, "字体颜色:", vec![
                (&color_popup as &NSView, None),
                (&color_hex as &NSView, Some(96.0)),
            ]);
            column.add(&color_row);
            let color_hint_row = row(mtm, "", vec![(&color_hint as &NSView, None)]);
            color_hint_row.setHidden(true);
            column.add(&color_hint_row);

            // 自动对比色：判定与推色规则在 contrast.rs（对比度不足才改，且保色相
            // 只推亮度）。默认关闭，避免升级后观感突变。
            // 上方的对齐方式决定它的采样依据，所以两项挨着放。
            let align_titles = ["居中（依据整张壁纸）", "靠右（依据文字区域）"];
            let align_popup = make_popup(mtm, &align_titles);
            for tag in 0..align_titles.len() as isize {
                let item: *mut NSObject = msg_send![&*align_popup, itemAtIndex: tag];
                let _: () = msg_send![item, setTag: tag];
            }
            let align_row = row(mtm, "文字对齐:", vec![(&align_popup as &NSView, None)]);
            column.add(&align_row);

            let auto_contrast_btn = make_checkbox(mtm, "壁纸对比不足时自动调整字体颜色");
            let auto_contrast_row = row(mtm, "自动对比:", vec![(&auto_contrast_btn as &NSView, None)]);
            column.add(&auto_contrast_row);

            // 农历 / 节假日：开关也走时钟预览通路（与滑块共用 clockSliderChanged:），
            // 点开关立即刷新叠加层，Apply 时才落盘。
            let lunar_btn = make_checkbox(mtm, "日期行显示农历");
            let _: () = msg_send![&*lunar_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*lunar_btn, setAction: Some(sel!(clockSliderChanged:))];
            let lunar_row = row(mtm, "农历:", vec![(&lunar_btn as &NSView, None)]);
            column.add(&lunar_row);

            let holiday_btn = make_checkbox(mtm, "副时区行显示当地节假日");
            let _: () = msg_send![&*holiday_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*holiday_btn, setAction: Some(sel!(clockSliderChanged:))];
            let holiday_row = row(mtm, "节假日:", vec![(&holiday_btn as &NSView, None)]);
            column.add(&holiday_row);

            // 外观滑块逐项成行：宽度变化时滑块保有足够行程，页面高度交给滚动区。
            let (wt_slider, wt_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (ts_slider, ts_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (ds_slider, ds_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (ss_slider, ss_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (fr_slider, fr_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (op_slider, op_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (sh_slider, sh_val): (Retained<NSSlider>, Retained<NSTextField>);
            let (of_slider, of_val): (Retained<NSSlider>, Retained<NSTextField>);
            macro_rules! clock_slider {
                ($slider:ident, $val:ident, $min:expr, $max:expr, $init:expr) => {{
                    $slider = make_slider(mtm, $min, $max, $init);
                    let _: () = msg_send![&*$slider, setTarget: Some(&*target)];
                    let _: () = msg_send![&*$slider, setAction: Some(sel!(clockSliderChanged:))];
                    // 只读数值标签：时钟滑块不开放手工输入，避免与滤镜滑块的
                    // tag 校验通道（textFieldChanged:）混在一起
                    $val = make_label(mtm, "");
                    // 不能写死数值：NSTextAlignment 在 arm64 与 x86_64 上相反，
                    // 旧的 `2i64 // 居中` 在 Apple Silicon 上实际设成了右对齐。
                    let _: () = msg_send![&*$val,
                        setAlignment: crate::settings_ui::TEXT_ALIGN_CENTER];
                }};
            }
            clock_slider!(wt_slider, wt_val, -1.0, 0.6, -0.6);
            clock_slider!(ts_slider, ts_val, 0.5, 2.0, 1.0);
            clock_slider!(ds_slider, ds_val, 0.5, 2.0, 1.0);
            clock_slider!(ss_slider, ss_val, 0.5, 2.0, 1.0);
            clock_slider!(fr_slider, fr_val, 0.0, 1.0, 0.5);
            clock_slider!(op_slider, op_val, 0.0, 1.0, 0.95);
            clock_slider!(sh_slider, sh_val, 0.0, 1.0, 0.45);
            clock_slider!(of_slider, of_val, -0.2, 0.2, 0.0);

            // 毛玻璃开关在前，强度滑块紧跟其后；字重只作用于时间行（日期/副时区
            // 保持固定字重，见 clock.rs）。
            let frost_btn = make_checkbox(mtm, "文字背后显示毛玻璃背景");
            // 开关切换也走时钟预览通路（与滑块共用 clockSliderChanged:）
            let _: () = msg_send![&*frost_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*frost_btn, setAction: Some(sel!(clockSliderChanged:))];
            let frost_row = row(mtm, "毛玻璃:", vec![(&frost_btn as &NSView, None)]);
            column.add(&frost_row);

            for (label, slider, value) in [
                ("字重", &wt_slider as &NSView, &wt_val as &NSView),
                ("毛玻璃强度", &fr_slider as &NSView, &fr_val as &NSView),
                ("时间字号", &ts_slider as &NSView, &ts_val as &NSView),
                ("日期字号", &ds_slider as &NSView, &ds_val as &NSView),
                ("副时区字号", &ss_slider as &NSView, &ss_val as &NSView),
                ("不透明度", &op_slider as &NSView, &op_val as &NSView),
                ("阴影强度", &sh_slider as &NSView, &sh_val as &NSView),
                ("垂直位置", &of_slider as &NSView, &of_val as &NSView),
            ] {
                column.add(&row(mtm, label, vec![(slider, Some(330.0)), (value, Some(52.0))]));
            }

            // ---- 刘海屏黑色矩阵 ----
            // 独立分区：它不属于时钟，也不属于滤镜，是屏幕顶部的装饰层。
            column.section(mtm, "刘海屏", "用黑色矩阵填平刘海缺口", "menubar.rectangle");
            let notch_btn = make_checkbox(mtm, "在壁纸顶部渲染黑色矩阵（高度 = 刘海黑框，不影响菜单栏）");
            let _: () = msg_send![&*notch_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*notch_btn, setAction: Some(sel!(notchChanged:))];
            column.add(&row(mtm, "黑色矩阵:", vec![(&notch_btn as &NSView, None)]));

            // 圆角作用于「下方壁纸的顶角」：黑条在两侧向下外翻，壁纸的上边缘
            // 以圆弧收进来。半径按 pt 存，渲染时换算成物理像素。
            let nr_slider = make_slider(mtm, 0.0, 24.0, 10.0);
            let _: () = msg_send![&*nr_slider, setTarget: Some(&*target)];
            let _: () = msg_send![&*nr_slider, setAction: Some(sel!(notchChanged:))];
            // 只读数值标签，与时钟滑块一致，不开放手工输入
            let nr_val = make_label(mtm, "10 pt");
            let _: () = msg_send![&*nr_val,
                setAlignment: crate::settings_ui::TEXT_ALIGN_CENTER];
            column.add(&row(
                mtm,
                "壁纸顶部圆角:",
                vec![(&nr_slider as &NSView, Some(330.0)), (&nr_val as &NSView, Some(64.0))],
            ));

            // Bottom buttons：固定在底部栏，不随内容滚动
            let cancel_btn = make_button(mtm, "取消");
            let _: () = msg_send![&*cancel_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*cancel_btn, setAction: Some(sel!(cancelClicked:))];
            cancel_btn.setKeyEquivalent(&NSString::from_str("\u{1b}")); // Esc

            let apply_btn = make_button(mtm, "应用");
            let _: () = msg_send![&*apply_btn, setTarget: Some(&*target)];
            let _: () = msg_send![&*apply_btn, setAction: Some(sel!(applyClicked:))];
            // 使用系统强调色绘制主操作，深色模式或用户更换强调色时自动适配。
            apply_btn.setBezelColor(Some(&NSColor::controlAccentColor()));
            // Return key triggers Apply — the classic pulsing default button
            apply_btn.setKeyEquivalent(&NSString::from_str("\r"));

            // 四个内容页始终保留各自控件，导航只改变可见性。
            let cards = column.finish(mtm);
            let card_views: [*mut NSView; 5] = cards.iter()
                .map(|card| Retained::as_ptr(card) as *mut NSView)
                .collect::<Vec<_>>()
                .try_into()
                .expect("设置分类数量应与导航按钮一致");
            *target.ivars().nav.lock().unwrap() = Some(SettingsNavPtrs {
                scroll: Retained::as_ptr(&scroll) as *mut _,
                cards: card_views,
                buttons: tabs.map(|button| Retained::as_ptr(&button) as *mut _),
                selections: selections.map(|view| Retained::as_ptr(&view) as *mut NSView),
            });

            let cancel_view: &NSView = &cancel_btn;
            let apply_view: &NSView = &apply_btn;
            let footer = crate::settings_ui::footer(mtm, &[cancel_view, apply_view]);
            root.addSubview(&footer);

            // 窗口级布局走 frame + autoresizing，不碰 Auto Layout。
            // 只要 contentView 的子视图全部由约束定位，窗口在 orderFront 时
            // 就会按内容的 fitting 尺寸反向调整自身（实测 560 → 50）；
            // 约束只保留在滚动内容内部，那里有 scrollView 给定的确定 frame。
            let content_w = content_rect.size.width;
            let content_h = content_rect.size.height;
            const SIDEBAR_W: f64 = 180.0;
            const FOOTER_H: f64 = 64.0;

            sidebar.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(SIDEBAR_W, content_h),
            ));
            // HeightSizable(16)：侧栏固定宽度，随窗口高度延展。
            let _: () = msg_send![&*sidebar, setAutoresizingMask: 16u64];

            scroll.setFrame(NSRect::new(
                NSPoint::new(SIDEBAR_W, FOOTER_H),
                NSSize::new(content_w - SIDEBAR_W, content_h - FOOTER_H),
            ));
            // WidthSizable(2) | HeightSizable(16)：右侧随窗口两个方向伸缩。
            let _: () = msg_send![&*scroll, setAutoresizingMask: 18u64];

            footer.setFrame(NSRect::new(
                NSPoint::new(SIDEBAR_W, 0.0),
                NSSize::new(content_w - SIDEBAR_W, FOOTER_H),
            ));
            // WidthSizable(2) | MaxYMargin(32)：底边 y=0 固定，窗口缩小时
            // 变化留给上方空白。旧的 MinYMargin(8) 会把底栏推到窗口外。
            let _: () = msg_send![&*footer, setAutoresizingMask: 34u64];

            // 在滚动视图获得实际 frame 后选中首个分类，避免在零尺寸 clip view
            // 上恢复滚动位置，导致首次打开时卡片从中间开始显示。
            target.select_section(0);

            *target.ivars().ptrs.lock().unwrap() = Some(RawControlPtrs {
                window: Retained::as_ptr(&window) as *mut _,
                dir_label: Retained::as_ptr(&dir_val) as *mut _,
                interval_input: Retained::as_ptr(&interval_input) as *mut _,
                interval_hint: Retained::as_ptr(&interval_hint) as *mut _,
                mode_popup: Retained::as_ptr(&mode_popup) as *mut _,
                random_wallpaper_url: Retained::as_ptr(&url_input) as *mut _,
                url_hint: Retained::as_ptr(&url_hint) as *mut _,
                brightness_slider: Retained::as_ptr(&b_slider) as *mut _,
                contrast_slider: Retained::as_ptr(&c_slider) as *mut _,
                saturation_slider: Retained::as_ptr(&s_slider) as *mut _,
                temperature_slider: Retained::as_ptr(&t_slider) as *mut _,
                blur_slider: Retained::as_ptr(&bl_slider) as *mut _,

                brightness_val_label: Retained::as_ptr(&b_val) as *mut _,
                contrast_val_label: Retained::as_ptr(&c_val) as *mut _,
                saturation_val_label: Retained::as_ptr(&s_val) as *mut _,
                temperature_val_label: Retained::as_ptr(&t_val) as *mut _,
                blur_val_label: Retained::as_ptr(&bl_val) as *mut _,

                clock_enabled_btn: Retained::as_ptr(&clock_enabled_btn) as *mut _,
                clock_preset_popup: Retained::as_ptr(&preset_popup) as *mut _,
                clock_primary_tz: Retained::as_ptr(&primary_tz_popup) as *mut _,
                clock_secondary_tz: Retained::as_ptr(&secondary_tz_popup) as *mut _,
                clock_format_popup: Retained::as_ptr(&format_popup) as *mut _,
                clock_color_popup: Retained::as_ptr(&color_popup) as *mut _,
                clock_color_hex: Retained::as_ptr(&color_hex) as *mut _,
                clock_color_hint: Retained::as_ptr(&color_hint) as *mut _,
                clock_align_popup: Retained::as_ptr(&align_popup) as *mut _,
                clock_auto_contrast_btn: Retained::as_ptr(&auto_contrast_btn) as *mut _,
                clock_weight_slider: Retained::as_ptr(&wt_slider) as *mut _,
                clock_weight_val: Retained::as_ptr(&wt_val) as *mut _,
                clock_time_scale_slider: Retained::as_ptr(&ts_slider) as *mut _,
                clock_time_scale_val: Retained::as_ptr(&ts_val) as *mut _,
                clock_date_scale_slider: Retained::as_ptr(&ds_slider) as *mut _,
                clock_date_scale_val: Retained::as_ptr(&ds_val) as *mut _,
                clock_sec_scale_slider: Retained::as_ptr(&ss_slider) as *mut _,
                clock_sec_scale_val: Retained::as_ptr(&ss_val) as *mut _,
                clock_frost_btn: Retained::as_ptr(&frost_btn) as *mut _,
                clock_frost_opacity_slider: Retained::as_ptr(&fr_slider) as *mut _,
                clock_frost_opacity_val: Retained::as_ptr(&fr_val) as *mut _,
                clock_opacity_slider: Retained::as_ptr(&op_slider) as *mut _,
                clock_opacity_val: Retained::as_ptr(&op_val) as *mut _,
                clock_shadow_slider: Retained::as_ptr(&sh_slider) as *mut _,
                clock_shadow_val: Retained::as_ptr(&sh_val) as *mut _,
                clock_offset_slider: Retained::as_ptr(&of_slider) as *mut _,
                clock_offset_val: Retained::as_ptr(&of_val) as *mut _,
                clock_lunar_btn: Retained::as_ptr(&lunar_btn) as *mut _,
                clock_holiday_btn: Retained::as_ptr(&holiday_btn) as *mut _,

                notch_enabled_btn: Retained::as_ptr(&notch_btn) as *mut _,
                notch_radius_slider: Retained::as_ptr(&nr_slider) as *mut _,
                notch_radius_val: Retained::as_ptr(&nr_val) as *mut _,
            });

            window.setContentSize(initial_size);
            window.setContentMinSize(NSSize::new(760.0, 500.0));

            // Center for the very first launch, then attach an autosave name:
            // from then on macOS restores the user-moved frame automatically.
            window.center();
            // v4：布局从「单列平铺」改成「滚动 + 卡片」，窗口高度不再随内容
            // 变化；沿用旧 autosave 名会把用户上次的窄窗口直接搬过来。
            // v5：v4 在宽度塌陷的那次运行里已经把 50pt 宽的 frame 写进了
            // UserDefaults，换名让坏值彻底失效。
            // 收紧默认高度后换新名称，避免上一次 700pt 的测试窗口被自动恢复。
            let autosave_name = NSString::from_str("WallFlowSettings9");
            // setFrameAutosaveName: 返回 BOOL（是否设置成功），不是 void
            let _: bool = msg_send![&*window,
                setFrameAutosaveName: &*autosave_name
            ];

            Self { window, target }
        }
    }

    pub fn show(&self, config: &config::Config) {
        unsafe {
            let Some(ptrs) = self.target.current_ptrs() else { return };
            // Populate fields
            let dir_ns = NSString::from_str(&config.wallpaper_dir);
            let _: () = msg_send![ptrs.dir_label, setStringValue: &*dir_ns];

            let interval_ns = NSString::from_str(&config.interval_secs.to_string());
            let _: () = msg_send![ptrs.interval_input, setStringValue: &*interval_ns];

            let url_ns = NSString::from_str(&config.random_wallpaper_url);
            let _: () = msg_send![ptrs.random_wallpaper_url, setStringValue: &*url_ns];

            // Reset transient hints every open
            let _: () = msg_send![ptrs.interval_hint, setHidden: true];
            set_hint_visible(ptrs.url_hint, false);

            let mode_idx: isize = match config.mode {
                crate::library::SwitchMode::Random => 1,
                crate::library::SwitchMode::Manual => 2,
                _ => 0,
            };
            let _: () = msg_send![ptrs.mode_popup, selectItemAtIndex: mode_idx];

            let _: () = msg_send![ptrs.brightness_slider, setDoubleValue: config.filters.brightness as f64];
            let _: () = msg_send![ptrs.contrast_slider, setDoubleValue: config.filters.contrast as f64];
            let _: () = msg_send![ptrs.saturation_slider, setDoubleValue: config.filters.saturation as f64];
            let _: () = msg_send![ptrs.temperature_slider, setDoubleValue: config.filters.temperature as f64];
            let _: () = msg_send![ptrs.blur_slider, setDoubleValue: config.filters.blur as f64];

            // ---- 时钟控件回填 ----
            let c = &config.clock;
            let _: () = msg_send![ptrs.clock_enabled_btn, setState: (if c.enabled { 1isize } else { 0isize })];
            select_preset(ptrs.clock_preset_popup, c.show_date, c.secondary_enabled);
            select_timezone_in_popup(ptrs.clock_primary_tz, &c.primary_timezone, true);
            select_timezone_in_popup(ptrs.clock_secondary_tz, &c.secondary_timezone, false);
            select_clock_format(ptrs.clock_format_popup, c.use_24_hour, c.show_seconds);
            select_color_preset(ptrs.clock_color_popup, &c.text_color);
            let _: () = msg_send![ptrs.clock_color_hex,
                setStringValue: &*NSString::from_str(&c.text_color)];
            set_hint_visible(ptrs.clock_color_hint, false);
            let _: () = msg_send![ptrs.clock_align_popup, selectItemAtIndex: (
                if c.text_align == config::TextAlign::Center { 0isize } else { 1isize })];
            let _: () = msg_send![ptrs.clock_auto_contrast_btn,
                setState: (if c.auto_contrast { 1isize } else { 0isize })];
            let _: () = msg_send![ptrs.clock_weight_slider, setDoubleValue: c.time_font_weight as f64];
            let _: () = msg_send![ptrs.clock_time_scale_slider, setDoubleValue: c.time_scale as f64];
            let _: () = msg_send![ptrs.clock_date_scale_slider, setDoubleValue: c.date_scale as f64];
            let _: () = msg_send![ptrs.clock_sec_scale_slider, setDoubleValue: c.secondary_scale as f64];
            let _: () = msg_send![ptrs.clock_frost_btn,
                setState: (if c.frosted_glass { 1isize } else { 0isize })];
            let _: () = msg_send![ptrs.clock_frost_opacity_slider, setDoubleValue: c.frosted_opacity as f64];
            let _: () = msg_send![ptrs.clock_opacity_slider, setDoubleValue: c.text_opacity as f64];
            let _: () = msg_send![ptrs.clock_shadow_slider, setDoubleValue: c.shadow_opacity as f64];
            let _: () = msg_send![ptrs.clock_offset_slider, setDoubleValue: c.vertical_offset as f64];
            let _: () = msg_send![ptrs.clock_lunar_btn,
                setState: (if c.show_lunar { 1isize } else { 0isize })];
            let _: () = msg_send![ptrs.clock_holiday_btn,
                setState: (if c.show_holiday { 1isize } else { 0isize })];

            // ---- 刘海矩阵回填 ----
            let n = &config.notch;
            let _: () = msg_send![ptrs.notch_enabled_btn,
                setState: (if n.enabled { 1isize } else { 0isize })];
            let _: () = msg_send![ptrs.notch_radius_slider,
                setDoubleValue: n.corner_radius as f64];
            let _: () = msg_send![ptrs.notch_radius_val,
                setStringValue: &*NSString::from_str(&format!("{:.0} pt", n.corner_radius))];

            // Trigger slider changed to update the value labels
            let _: () = msg_send![&*self.target, sliderChanged: std::ptr::null_mut::<NSObject>()];
            self.target.refresh_clock_value_labels();

            // Show window
            let _: () = msg_send![ptrs.window, makeKeyAndOrderFront: None::<&NSObject>];
            let app = NSApplication::sharedApplication(self.target.ivars().mtm);
            app.activate();
        }
    }
}

// ---------------------------------------------------------------------------
// Small layout helpers (no more hand-placed coordinates)
// ---------------------------------------------------------------------------

/// 回填时间格式下拉：按 (24 小时制, 是否带秒) 反查对应项。
fn select_clock_format(popup: *mut NSPopUpButton, use_24_hour: bool, show_seconds: bool) {
    let idx = CLOCK_FORMATS
        .iter()
        .position(|(h24, sec)| *h24 == use_24_hour && *sec == show_seconds)
        .unwrap_or(0);
    unsafe {
        let _: () = msg_send![popup, selectItemAtIndex: idx as isize];
    }
}

/// 回填颜色预设：命中预设则选中；否则在顶部插入「自定义」（tag = -1）并选中。
/// 这样配置文件里手写的任意颜色不会在面板里被悄悄替换成某个预设色。
fn select_color_preset(popup: *mut NSPopUpButton, hex: &str) {
    unsafe {
        let first: *mut NSObject = msg_send![popup, itemAtIndex: 0isize];
        if !first.is_null() {
            let tag: isize = msg_send![first, tag];
            if tag == -1 {
                let _: () = msg_send![popup, removeItemAtIndex: 0isize];
            }
        }
        let canonical = crate::clock::canonical_hex(hex);
        let hit = canonical.as_deref().and_then(|c| {
            CLOCK_COLOR_PRESETS.iter().position(|(_, preset)| *preset == c)
        });
        match hit {
            Some(i) => {
                let _: () = msg_send![popup, selectItemAtIndex: i as isize];
            }
            None => {
                let _: () = msg_send![popup,
                    insertItemWithTitle: &*NSString::from_str("自定义"),
                    atIndex: 0isize];
                let item: *mut NSObject = msg_send![popup, itemAtIndex: 0isize];
                let _: () = msg_send![item, setTag: -1isize];
                let _: () = msg_send![popup, selectItemAtIndex: 0isize];
            }
        }
    }
}

/// 独立的校验提示必须连同行容器一起隐藏；只隐藏文字仍会在垂直栈里
/// 占据一整行空白，短页面尤其明显。提示的 superview 就是其表单行。
unsafe fn set_hint_visible(hint: *mut NSTextField, visible: bool) {
    let row: *mut NSView = unsafe { msg_send![hint, superview] };
    unsafe {
        let _: () = msg_send![hint, setHidden: !visible];
        if !row.is_null() {
            let _: () = msg_send![row, setHidden: !visible];
        }
    }
}

/// 单行表单：标签列宽与右对齐统一交给视觉层，这里只做参数转发。
/// 保留这个薄封装是为了让所有调用点不必感知布局模块的存在。
fn row(
    mtm: MainThreadMarker,
    label_text: &str,
    items: Vec<(&NSView, Option<f64>)>,
) -> Retained<NSStackView> {
    crate::settings_ui::form_row(mtm, label_text, &items)
}

fn apply_color(label: &NSTextField, color: Retained<NSColor>) {
    unsafe {
        let _: () = msg_send![label, setTextColor: &*color];
    }
}

fn make_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::new(mtm);
    unsafe {
        let text_ns = NSString::from_str(text);
        let _: () = msg_send![&*label, setStringValue: &*text_ns];
        let _: () = msg_send![&*label, setEditable: false];
        let _: () = msg_send![&*label, setBezeled: false];
        let _: () = msg_send![&*label, setDrawsBackground: false];
    }
    label
}

/// 次要说明跟随系统浅色/深色外观，避免固定灰在深色背景上失去对比度。
fn make_small_label(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = make_label(mtm, text);
    unsafe {
        if let Some(font_cls) = objc2::runtime::AnyClass::get(
            std::ffi::CStr::from_bytes_with_nul(b"NSFont\0").unwrap(),
        ) {
            let small: Option<Retained<NSObject>> = msg_send![font_cls, systemFontOfSize: 11.5f64];
            if let Some(f) = small {
                let _: () = msg_send![&*label, setFont: Some(&*f)];
            }
        }
        let _: () = msg_send![&*label, setTextColor: &*NSColor::secondaryLabelColor()];
    }
    label
}

fn make_input(mtm: MainThreadMarker, initial_text: &str) -> Retained<NSTextField> {
    let input = NSTextField::new(mtm);
    unsafe {
        let initial_ns = NSString::from_str(initial_text);
        let _: () = msg_send![&*input, setStringValue: &*initial_ns];
        let _: () = msg_send![&*input, setEditable: true];
        let _: () = msg_send![&*input, setBezeled: true];
    }
    input
}

fn make_slider(mtm: MainThreadMarker, min: f64, max: f64, val: f64) -> Retained<NSSlider> {
    let slider = NSSlider::new(mtm);
    unsafe {
        let _: () = msg_send![&*slider, setMinValue: min];
        let _: () = msg_send![&*slider, setMaxValue: max];
        let _: () = msg_send![&*slider, setDoubleValue: val];
    }
    slider
}

fn make_button(mtm: MainThreadMarker, title: &str) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    unsafe {
        let title_ns = NSString::from_str(title);
        let _: () = msg_send![&*button, setTitle: &*title_ns];
        // NSButtonTypeMomentaryLight == 1: standard bezel push button
        let _: () = msg_send![&*button, setBezelStyle: 1u64];
    }
    button
}

/// 复选框（开关样式）。
fn make_checkbox(mtm: MainThreadMarker, title: &str) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    unsafe {
        // NSButtonTypeSwitch == 3
        let _: () = msg_send![&*button, setButtonType: 3i64];
        let _: () = msg_send![&*button, setTitle: &*NSString::from_str(title)];
    }
    button
}

/// 通用下拉框。
fn make_popup(_mtm: MainThreadMarker, items: &[&str]) -> Retained<NSPopUpButton> {
    unsafe {
        let alloc: *mut NSPopUpButton = msg_send![NSPopUpButton::class(), alloc];
        let raw: *mut NSPopUpButton = msg_send![
            alloc,
            initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(240.0, 26.0)),
            pullsDown: false
        ];
        let popup = Retained::from_raw(raw).unwrap();
        for item in items {
            let _: () = msg_send![&*popup, addItemWithTitle: &*NSString::from_str(item)];
        }
        popup
    }
}

/// 时区下拉框。include_local 时在开头插入「跟随系统」(local)。
fn make_tz_popup(mtm: MainThreadMarker, include_local: bool) -> Retained<NSPopUpButton> {
    let popup = make_popup(mtm, &[]);
    unsafe {
        if include_local {
            let _: () = msg_send![&*popup, addItemWithTitle: &*NSString::from_str("跟随系统")];
        }
        for (label, _) in COMMON_TIMEZONES {
            let _: () = msg_send![&*popup, addItemWithTitle: &*NSString::from_str(label)];
        }
    }
    popup
}

/// 回填时区下拉框：匹配内置项则选中，否则把该 IANA 名称作为自定义项追加并选中，
/// 保证配置文件里手写的任意时区都不会丢。
fn select_timezone_in_popup(popup: *mut NSPopUpButton, tz: &str, include_local: bool) {
    unsafe {
        let base = if include_local { 1 } else { 0 };
        if include_local && (tz.is_empty() || tz == "local") {
            let _: () = msg_send![popup, selectItemAtIndex: 0isize];
            return;
        }
        for (i, (_, name)) in COMMON_TIMEZONES.iter().enumerate() {
            if *name == tz {
                let _: () = msg_send![popup, selectItemAtIndex: (base + i) as isize];
                return;
            }
        }
        // 自定义时区：若之前已添加过自定义项先移除，避免重复
        let count: isize = msg_send![popup, numberOfItems];
        let builtin = base + COMMON_TIMEZONES.len();
        if count as usize > builtin {
            let _: () = msg_send![popup, removeItemAtIndex: builtin as isize];
        }
        let _: () = msg_send![popup, addItemWithTitle: &*NSString::from_str(tz)];
        let _: () = msg_send![popup, selectItemAtIndex: builtin as isize];
    }
}

/// 读取时区下拉框当前选中的 IANA 名称（自定义项的标题即时区名）。
fn selected_timezone(popup: *mut NSPopUpButton, include_local: bool) -> String {
    unsafe {
        let idx: isize = msg_send![popup, indexOfSelectedItem];
        if idx < 0 {
            return if include_local { "local".into() } else { COMMON_TIMEZONES[5].1.into() };
        }
        let idx = idx as usize;
        let base = if include_local { 1 } else { 0 };
        if include_local && idx == 0 {
            return "local".into();
        }
        if idx - base < COMMON_TIMEZONES.len() {
            return COMMON_TIMEZONES[idx - base].1.into();
        }
        // 自定义项：标题就是时区名
        let item: *mut NSObject = msg_send![popup, selectedItem];
        let title: Retained<NSString> = msg_send![item, title];
        title.to_string()
    }
}

/// 回填样式预设：匹配 (show_date, secondary_enabled) 选中对应项；
/// 不匹配则在顶部临时插入「自定义」项（tag = -1）并选中，应用时保留现有配置。
fn select_preset(popup: *mut NSPopUpButton, show_date: bool, secondary_enabled: bool) {
    unsafe {
        // 先移除可能存在的「自定义」项（总是 index 0 且 tag -1）
        let first: *mut NSObject = msg_send![popup, itemAtIndex: 0isize];
        if !first.is_null() {
            let tag: isize = msg_send![first, tag];
            if tag == -1 {
                let _: () = msg_send![popup, removeItemAtIndex: 0isize];
            }
        }
        let tag: isize = match (show_date, secondary_enabled) {
            (false, false) => 0,
            (true, false) => 1,
            (true, true) => 2,
            (false, true) => -1,
        };
        if tag == -1 {
            let _: () = msg_send![popup, insertItemWithTitle: &*NSString::from_str("自定义"), atIndex: 0isize];
            let item: *mut NSObject = msg_send![popup, itemAtIndex: 0isize];
            let _: () = msg_send![item, setTag: -1isize];
            let _: () = msg_send![popup, selectItemAtIndex: 0isize];
        } else {
            // NSPopUpButton 的该选择器返回 BOOL；按 void 接收会在 objc2
            // 的运行时签名检查中 panic，窗口甚至来不及显示。
            let _: bool = msg_send![popup, selectItemWithTag: tag];
        }
    }
}
