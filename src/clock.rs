//! 壁纸中央的时钟叠加层（苹果锁屏风格）。
//!
//! 实现方式：每块屏幕一个透明、无边框、不响应鼠标的 NSWindow，
//! 窗口层级 -2000（与 wallpaper.rs 的淡入淡出叠加层一致：壁纸之上、桌面图标之下），
//! 内部放置三个居中的 NSTextField（大号细体时间 / 日期 / 副时区）。
//! 由 main.rs 中 1s 重复的 NSTimer 驱动 tick() 刷新文本；
//! 时区偏移通过 NSTimeZone 获取（自动处理夏令时），日期换算不引入额外依赖。

use std::collections::HashMap;
use objc2::msg_send;
use objc2::runtime::NSObject;
use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use crate::config::{ClockConfig, TextAlign};
use crate::contrast;
use crate::util::find_class;

/// 三层文字的层级不透明度（时间最实、副时区最淡），再乘用户的 `text_opacity`。
/// 抽成常量是因为「就地改色」也要用同一套系数：两处各写一遍迟早会漂移。
const ALPHA_TIME: f64 = 0.95;
const ALPHA_DATE: f64 = 0.85;
const ALPHA_SECONDARY: f64 = 0.70;

/// AppKit `NSTextAlignment` 的取值随 ABI 变化 —— 这一点极易踩坑：
///
/// ```c
/// // SDK 的 TargetConditionals.h
/// #define TARGET_ABI_USES_IOS_VALUES (!TARGET_CPU_X86_64 || (TARGET_OS_IPHONE && !TARGET_OS_MACCATALYST))
/// // NSText.h
/// #if TARGET_ABI_USES_IOS_VALUES
///   NSTextAlignmentCenter = 1, NSTextAlignmentRight = 2
/// #else
///   NSTextAlignmentRight = 1, NSTextAlignmentCenter = 2
/// #endif
/// ```
///
/// 也就是说 **Apple Silicon 上是 Center=1/Right=2，Intel 上相反**。硬编码 `2`
/// 并在注释里写「Center」，在 arm64 上得到的是右对齐 —— 本项目的时钟就是这样
/// 跑到屏幕右侧去的（`setAlignment: 2` 配 `// NSTextAlignmentCenter` 注释）。
/// 因此这里按架构定常量，绝不写裸数字。
const TEXT_ALIGN_RIGHT_VALUE: i64 = if cfg!(target_arch = "x86_64") { 1 } else { 2 };
const TEXT_ALIGN_CENTER_VALUE: i64 = if cfg!(target_arch = "x86_64") { 2 } else { 1 };

/// 配置里的对齐方式 → AppKit 的枚举值。转换只集中在这一处。
pub fn ns_text_align(align: TextAlign) -> i64 {
    match align {
        TextAlign::Center => TEXT_ALIGN_CENTER_VALUE,
        TextAlign::Right => TEXT_ALIGN_RIGHT_VALUE,
    }
}

/// label 框在屏宽中的起点与宽度（与 `build_overlay` 的 `label_x`/`label_w` 一致）。
const LABEL_X_RATIO: f64 = 0.05;
const LABEL_W_RATIO: f64 = 0.9;

/// 单块屏幕上的时钟叠加层。指针由窗口持有（label 作为 subview 被 contentView 持有），
/// 析构统一走 close_window()，不要做额外 release。
struct ClockOverlay {
    window: *mut NSObject,
    time_label: *mut NSObject,
    date_label: Option<*mut NSObject>,
    secondary_label: Option<*mut NSObject>,
}

pub struct ClockManager {
    pub config: ClockConfig,
    overlays: HashMap<u32, ClockOverlay>,
    /// 每屏壁纸在「时钟文字带」上的平均相对亮度（0..1），由 main.rs 采样后写入。
    /// 没有条目 = 还没采过样，按 0.5 中性值处理 —— 中性值不落在任何极端区，
    /// 因此不会在拿到真实亮度前乱改颜色。
    bg_luma: HashMap<u32, f64>,
    /// 每屏时间行文本宽度占屏宽的比例（AppKit 实测，见 `measure_time_width_ratio`）。
    /// 亮度采样靠它把水平范围收缩到文字真正压住的区域；缺项回退到固定比例。
    time_width: HashMap<u32, f64>,
    /// 每屏时间行文字的水平起点比例（由对齐方式 + 实测宽度推出，见
    /// `time_origin_ratio`）。与 `time_width` 一起定位采样带。
    text_origin: HashMap<u32, f64>,
}

impl ClockManager {
    pub fn new(config: ClockConfig) -> Self {
        Self {
            config,
            overlays: HashMap::new(),
            bg_luma: HashMap::new(),
            time_width: HashMap::new(),
            text_origin: HashMap::new(),
        }
    }

    /// 应用新配置：配置变化时重建叠加层，随后立即刷新一次文本。
    pub fn apply_config(&mut self, mtm: MainThreadMarker, config: ClockConfig) {
        if self.config != config {
            self.config = config;
            self.rebuild(mtm);
        } else {
            self.tick();
        }
    }

    /// 记录某块屏幕壁纸在时钟文字带上的平均相对亮度。
    ///
    /// 这是「换壁纸 → 自动改色」的唯一入口：像素采样已在后台线程完成
    /// （见 main.rs 的采样任务），主线程只做决策与贴色，保持轻量。
    /// 采样结果无论如何都记录下来 —— 这样用户随后打开「自动对比」开关时，
    /// 立刻就有数据可用，不用等到下一次换壁纸。
    pub fn set_background_luma(&mut self, screen_number: u32, luma: f64) {
        let prev = self.bg_luma.insert(screen_number, luma);
        if !self.config.auto_contrast {
            return;
        }
        // 同一张壁纸被重新应用（例如只改了滤镜预设）时亮度几乎不变，跳过贴色，
        // 省掉三次 setTextColor 与一次颜色重算。
        if prev.map(|p| (p - luma).abs() < 1e-6).unwrap_or(false) {
            return;
        }
        let rgb = self.effective_rgb(screen_number);
        if let Some(overlay) = self.overlays.get(&screen_number) {
            set_overlay_text_color(overlay, rgb, &self.config);
            println!(
                "[clock] screen {} bg_luma={:.3} → 文字色 {}",
                screen_number,
                luma,
                format_hex(rgb)
            );
        }
    }

    /// 是否还需要壁纸亮度采样。
    ///
    /// 采样要把整张壁纸解码一遍（5400 万像素的照片约 0.8 秒 CPU），当时钟隐藏
    /// 或自动对比关闭时这份数据永远不会被用到 —— 直接跳过，别为无用数据付解码钱。
    pub fn wants_luma(&self) -> bool {
        self.config.enabled && self.config.auto_contrast
    }

    /// 该屏实际应当使用的文字色：关闭自动对比时就是用户设定色，开启时按背景
    /// 亮度做一次决策（不达标才替换，达标仍用原色）。
    fn effective_rgb(&self, screen_number: u32) -> (f64, f64, f64) {        let base = parse_hex_color(&self.config.text_color).unwrap_or((1.0, 1.0, 1.0));
        if !self.config.auto_contrast {
            return base;
        }
        let bg = self.bg_luma.get(&screen_number).copied().unwrap_or(0.5);
        let rgb = contrast::auto_contrast_color(base, bg);
        if rgb != base {
            println!(
                "[clock] screen {} 对比度不足（bg_luma={:.3}），{} → {}",
                screen_number,
                bg,
                format_hex(base),
                format_hex(rgb)
            );
        }
        rgb
    }

    /// 该屏壁纸亮度采样应使用的矩形，返回比例坐标 `(x0, y0, x1, y1)`
    /// （图像坐标系，y 轴向下）。
    ///
    /// 区域按 `config.clock.text_align` 决定：靠右时只取文字压住的矩形，
    /// 居中时取整张壁纸（见 `sample_region_ratios`）。即使时钟当前未显示也能用 ——
    /// 区域只依赖配置与实测宽度，与窗口是否存在无关。
    pub fn sample_band(&self, screen_number: u32) -> (f64, f64, f64, f64) {
        sample_region_ratios(
            &self.config,
            self.time_width.get(&screen_number).copied(),
            self.text_origin.get(&screen_number).copied(),
        )
    }

    /// 丢弃所有屏幕的背景亮度采样。
    ///
    /// 采样区域变化时（例如用户切换文字对齐方式：靠右 ↔ 居中，两者采的根本
    /// 不是同一块背景）旧亮度不再代表新区域，继续拿它做判定只会得出错误结论。
    /// 清空后判定回退到中性值 0.5（不触发改色），等下一次换壁纸时按新区域
    /// 重新采样 —— 「暂不改色」比「按错误的区域改色」安全得多。
    pub fn clear_background_luma(&mut self) {
        self.bg_luma.clear();
    }

    /// 关闭全部叠加层，并按当前配置为每块屏幕重建（屏幕热插拔/分辨率变化时调用）。
    pub fn rebuild(&mut self, mtm: MainThreadMarker) {
        self.teardown();
        if !self.config.enabled {
            return;
        }
        let screens = crate::wallpaper::get_screens(mtm);
        println!("[clock] rebuild: {} screen(s), config={:?}", screens.len(), self.config);
        for screen in screens {
            let num = crate::screen::screen_number(&screen);
            // 实测时间行宽度：亮度采样要把范围收缩到文字真正压住的那块背景，
            // 固定比例（早先是屏宽 64%）会把两侧无关背景平均进来。测量必须
            // 在主线程做（AppKit 字体测量），而 rebuild 本来就是主线程调用。
            let frame = screen.frame();
            let measured = unsafe {
                measure_time_width_ratio(&self.config, frame.size.width, frame.size.height)
            };
            match measured {
                Some(ratio) => {
                    // 文字起点由「对齐方式 + 实测宽度」推出：采样必须落在文字
                    // 真正所在的位置上（靠右时文字就贴在屏幕右侧）。
                    let origin = time_origin_ratio(ratio, self.config.text_align);
                    self.time_width.insert(num, ratio);
                    self.text_origin.insert(num, origin);
                    println!(
                        "[clock] screen {} 时间行占屏宽 {:.3}，文字起点 {:.3}（对齐 {:?}）→ 采样区域 {:?}",
                        num, ratio, origin, self.config.text_align, self.sample_band(num)
                    );
                }
                // 测量不可用时清掉旧值，让采样回退到保守的固定比例而不是用过期数据
                None => {
                    self.time_width.remove(&num);
                    self.text_origin.remove(&num);
                }
            }
            // 取色放在建窗之前：每块屏的壁纸不同，自己的亮度决定自己的文字色
            let rgb = self.effective_rgb(num);
            match build_overlay(&screen, &self.config, rgb) {
                Some(overlay) => {
                    println!("[clock] overlay built for screen {}", num);
                    self.overlays.insert(num, overlay);
                }
                None => println!("[clock] build_overlay returned None for screen {}", num),
            }
        }
        self.tick();
    }

    fn teardown(&mut self) {
        for (_, overlay) in self.overlays.drain() {
            unsafe {
                let _: () = msg_send![overlay.window, orderOut: None::<&NSObject>];
                let _: () = msg_send![overlay.window, close];
            }
        }
    }

    /// 每秒调用一次：重算主/副时区时间并更新所有屏幕上的标签文本。
    pub fn tick(&mut self) {
        if self.overlays.is_empty() {
            return;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let pri = BrokenTime::from_epoch(now + tz_offset_seconds(&self.config.primary_timezone));
        let time_str = format_time(
            pri.hour,
            pri.minute,
            pri.second,
            self.config.use_24_hour,
            self.config.show_seconds,
        );
        const WEEKDAYS: [&str; 7] = ["日", "一", "二", "三", "四", "五", "六"];
        let mut date_str = format!("{}月{}日 星期{}", pri.month, pri.day, WEEKDAYS[pri.weekday]);
        // 农历：跟随主时区的公历日期（农历不随夏令时/时区偏移变化，只认当地日期）
        if self.config.show_lunar {
            if let Some(ld) = crate::lunar::lunar_from_epoch_days(pri.days) {
                date_str.push_str(" · 农历");
                date_str.push_str(&crate::lunar::lunar_month_day(&ld));
            }
        }

        let secondary_str = if self.config.secondary_enabled {
            let sec = BrokenTime::from_epoch(now + tz_offset_seconds(&self.config.secondary_timezone));
            let label = if self.config.secondary_label.is_empty() {
                city_name_from_tz(&self.config.secondary_timezone)
            } else {
                self.config.secondary_label.clone()
            };
            // 副时区固定不带秒：它是「世界时钟」参照，秒位只会增加噪音
            let mut s = format!("{}  {}", label, format_time(sec.hour, sec.minute, 0, self.config.use_24_hour, false));
            // 副时区与主时区相差一天时给出提示（如中国 vs 英国跨午夜）
            match sec.days - pri.days {
                -1 => s.push_str(" · 昨天"),
                1 => s.push_str(" · 明天"),
                _ => {}
            }
            // 副时区当地节假日（按副时区公历日期判断；中国时区额外含农历节日）
            if self.config.show_holiday {
                if let Some(name) = crate::lunar::holiday_for_tz(
                    &self.config.secondary_timezone,
                    sec.month,
                    sec.day,
                    sec.days,
                ) {
                    s.push_str(" · ");
                    s.push_str(name);
                }
            }
            Some(s)
        } else {
            None
        };

        for overlay in self.overlays.values() {
            set_label_text(overlay.time_label, &time_str);
            if let Some(label) = overlay.date_label {
                set_label_text(label, &date_str);
            }
            if let (Some(label), Some(text)) = (overlay.secondary_label, &secondary_str) {
                set_label_text(label, text);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 界面构建
// ---------------------------------------------------------------------------

/// 为一块屏幕创建时钟叠加层窗口（苹果锁屏风格：超大细体时间居中，下接日期与副时区）。
///
/// `rgb` 是**已经过自动对比度决策**的生效文字色（见 `ClockManager::effective_rgb`）。
/// 布局与取色在这里分开，改色时才能只 `setTextColor` 而不重建窗口。
fn build_overlay(screen: &NSScreen, config: &ClockConfig, rgb: (f64, f64, f64)) -> Option<ClockOverlay> {
    unsafe {
        let frame = screen.frame();
        let w = frame.size.width;
        let h = frame.size.height;

        // 与 wallpaper.rs 淡入淡出叠加层相同的窗口配置：
        // 无边框、透明、忽略鼠标、层级 -2000（壁纸之上、图标之下）
        let win_cls = find_class("NSWindow").ok()?;
        let window: *mut NSObject = msg_send![win_cls, alloc];
        let window: *mut NSObject = msg_send![window,
            initWithContentRect: frame,
            styleMask: 0u64,
            backing: 2u64,
            defer: false
        ];
        if window.is_null() {
            return None;
        }
        let _: () = msg_send![window, setLevel: -2000i64];
        let _: () = msg_send![window, setIgnoresMouseEvents: true];
        let _: () = msg_send![window, setOpaque: false];
        let _: () = msg_send![window, setHasShadow: false];
        // CanJoinAllSpaces(1) | Stationary(16)：出现在所有空间，且不参与 Exposé 重排
        let _: () = msg_send![window, setCollectionBehavior: 17u64];
        // 允许被截屏/录屏捕获（默认创建的窗口 SharingState 为 None，截图里看不到）
        let _: () = msg_send![window, setSharingType: 1u64]; // NSWindowSharingReadOnly
        let clear: *mut NSObject = msg_send![find_class("NSColor").ok()?, clearColor];
        let _: () = msg_send![window, setBackgroundColor: clear];

        let content: *mut NSObject = msg_send![window, contentView];
        if content.is_null() {
            let _: () = msg_send![window, close];
            return None;
        }

        // 字号随屏幕高度缩放，4K 与 1080p 上视觉比例一致；时间/日期/副时区三行
        // 各自一个缩放系数（0.5–2.0×）。下限也要跟着缩放走，否则缩到 0.5× 时
        // 会被绝对下限顶回去，滑块拖到底也没变化。
        let time_scale = config.time_scale.clamp(0.5, 2.0) as f64;
        let date_scale = config.date_scale.clamp(0.5, 2.0) as f64;
        let sec_scale = config.secondary_scale.clamp(0.5, 2.0) as f64;
        let time_size = h * 0.13 * time_scale;
        let date_size = (h * 0.026 * date_scale).max(14.0 * date_scale);
        let sec_size = (h * 0.020 * sec_scale).max(12.0 * sec_scale);
        let time_h = time_size * 1.25;
        let date_h = date_size * 1.7;
        let sec_h = sec_size * 1.7;
        let gap1 = h * 0.012;
        let gap2 = h * 0.008;

        let has_date = config.show_date;
        let has_secondary = config.secondary_enabled;
        let total = time_h
            + if has_date { gap1 + date_h } else { 0.0 }
            + if has_secondary { gap2 + sec_h } else { 0.0 };
        // 组中心放在屏幕纵向 52% 处，比严格居中略高，更符合视觉重心；
        // 用户偏移限制在 ±20% 屏高内，避免把时钟推到屏幕外。
        let center_ratio = 0.52 + config.vertical_offset.clamp(-0.2, 0.2) as f64;
        let top = h * center_ratio + total / 2.0;
        let label_w = w * 0.9;
        let label_x = (w - label_w) / 2.0;

        let time_y = top - time_h;
        let date_y = time_y - gap1 - date_h;
        let sec_y = if has_date { date_y - gap2 - sec_h } else { time_y - gap2 - sec_h };

        // 文字颜色由调用方传入（已做自动对比度决策，非法 HEX 在解析处回退为白色，
        // 设置面板也会在应用前拦下非法值）。
        // 三层文字各有基准 alpha（时间最实、副时区最淡），用户的 text_opacity
        // 作为整体系数乘上去，这样「调淡」不会破坏原有的主次层次。
        let opacity = config.text_opacity.clamp(0.0, 1.0) as f64;
        let shadow = config.shadow_opacity.clamp(0.0, 1.0) as f64;
        // 投影半径跟字号一起缩放，大字号下阴影才不会显得发死（以时间行为基准）
        let shadow_radius = 12.0 * time_scale;

        // 时间行字重是连续 NSFontWeight 值（约 -1.0 极细 … 0.6 极粗）；
        // 日期/副时区保持固定字重（见下方 make_label 调用），保留主次层次。
        let time_weight = config.time_font_weight.clamp(-1.0, 0.6) as f64;

        // 毛玻璃背景：在文字之下垫一块 NSVisualEffectView，按各行最宽文本收缩成
        // 卡片，并随对齐方式贴在对应一侧（靠右贴右、居中居中）。
        if config.frosted_glass {
            let time_w = measure_line_width(&widest_time_text(config), time_size, time_weight);
            let date_w = if has_date {
                measure_line_width("12月31日 星期六", date_size, -0.4)
            } else {
                0.0
            };
            let sec_w = if has_secondary {
                let label = if config.secondary_label.is_empty() {
                    city_name_from_tz(&config.secondary_timezone)
                } else {
                    config.secondary_label.clone()
                };
                measure_line_width(&format!("{}  88:88 · 明天", label), sec_size, 0.0)
            } else {
                0.0
            };
            let text_w = time_w.max(date_w).max(sec_w);
            let pad_h = h * 0.015;
            let pad_v = h * 0.012;
            let card_w = (text_w + pad_h * 2.0).min(w);
            let card_h = total + pad_v * 2.0;
            let card_x = match config.text_align {
                TextAlign::Right => (label_x + label_w - card_w).max(0.0),
                TextAlign::Center => (w - card_w) / 2.0,
            };
            let card_y = (top - total - pad_v).max(0.0);
            add_frosted_backdrop(
                content,
                NSRect::new(NSPoint::new(card_x, card_y), NSSize::new(card_w, card_h)),
                config.frosted_opacity.clamp(0.0, 1.0),
            );
        }

        let align = ns_text_align(config.text_align);
        let time_label = make_label(
            content,
            NSRect::new(NSPoint::new(label_x, time_y), NSSize::new(label_w, time_h)),
            time_size, time_weight,
            rgb, ALPHA_TIME * opacity, shadow, shadow_radius, align,
        )?;
        let date_label = if has_date {
            Some(make_label(
                content,
                NSRect::new(NSPoint::new(label_x, date_y), NSSize::new(label_w, date_h)),
                date_size, -0.4, rgb, ALPHA_DATE * opacity, shadow, shadow_radius, align,
            )?)
        } else {
            None
        };
        let secondary_label = if has_secondary {
            Some(make_label(
                content,
                NSRect::new(NSPoint::new(label_x, sec_y), NSSize::new(label_w, sec_h)),
                sec_size, 0.0, rgb, ALPHA_SECONDARY * opacity, shadow, shadow_radius, align,
            )?)
        } else {
            None
        };

        let _: () = msg_send![window, orderFrontRegardless];

        Some(ClockOverlay { window, time_label, date_label, secondary_label })
    }
}

/// 创建一个半透明文本标签（带柔和投影，保证浅色壁纸上可读）。
/// 颜色由 sRGB 分量构造，`alpha` 已由调用方乘好；`shadow_opacity` 为 0 时不启用
/// 阴影（CALayer 的 shadowOpacity 默认就是 0，跳过即可）。
/// `align` 是 AppKit 的对齐枚举值，**必须由 `ns_text_align()` 转换**（枚举值随
/// ABI 变化，不能硬编码）。
/// 返回的指针由 contentView 持有（addSubview 后 release 掉自己的 +1）。
unsafe fn make_label(
    content: *mut NSObject,
    frame: NSRect,
    font_size: f64,
    font_weight: f64,
    rgb: (f64, f64, f64),
    alpha: f64,
    shadow_opacity: f64,
    shadow_radius: f64,
    align: i64,
) -> Option<*mut NSObject> {
    let tf_cls = find_class("NSTextField").ok()?;
    let label: *mut NSObject = msg_send![tf_cls, alloc];
    let label: *mut NSObject = msg_send![label, initWithFrame: frame];
    if label.is_null() {
        return None;
    }
    let _: () = msg_send![label, setBezeled: false];
    let _: () = msg_send![label, setBordered: false];
    let _: () = msg_send![label, setDrawsBackground: false];
    let _: () = msg_send![label, setEditable: false];
    let _: () = msg_send![label, setSelectable: false];
    let _: () = msg_send![label, setAlignment: align];

    let font: *mut NSObject = msg_send![find_class("NSFont").ok()?,
        systemFontOfSize: font_size, weight: font_weight];
    let _: () = msg_send![label, setFont: font];

    let color_cls = find_class("NSColor").ok()?;
    // 文字色构造集中在 set_label_color，建窗与后续改色共用同一份实现
    if !set_label_color(label, rgb, alpha) {
        return None;
    }

    // 柔和投影：走 CALayer（NSCell 没有 setShadow:，直接调会抛异常）。
    // 背景透明，layer 阴影只作用于文字像素。
    let _: () = msg_send![label, setWantsLayer: true];
    let layer: *mut NSObject = msg_send![label, layer];
    if !layer.is_null() && shadow_opacity > 0.0 {
        let black: *mut NSObject = msg_send![color_cls, blackColor];
        // 用真实的 CGColorRef 类型：debug 构建下 objc2 会校验方法签名的类型编码，
        // 用 *const c_void（'^v'）替代 '^{CGColor=}' 会直接 panic
        let cg_black: *const objc2_core_graphics::CGColor = msg_send![black, CGColor];
        let _: () = msg_send![layer, setShadowColor: cg_black];
        let _: () = msg_send![layer, setShadowOpacity: shadow_opacity as f32];
        let _: () = msg_send![layer, setShadowOffset: NSSize::new(0.0, -2.0)];
        let _: () = msg_send![layer, setShadowRadius: shadow_radius];
    }

    let _: () = msg_send![content, addSubview: label];
    let _: () = msg_send![label, release];
    Some(label)
}

/// 在标签之下垫一块毛玻璃背景卡片（NSVisualEffectView）。
///
/// 材质用 `Sidebar`（随系统明暗外观自适应，最接近 iOS 控制中心那种毛玻璃感）、
/// `BehindWindow` 混合（模糊窗口后面的壁纸）、`Active` 状态（始终呈现激活观感）。
/// `opacity` 是用户可调的强度，映射到视图 `alphaValue`。
/// 视图由 contentView 持有（addSubview 后 release 自己的 +1），无需额外指针。
unsafe fn add_frosted_backdrop(content: *mut NSObject, frame: NSRect, opacity: f32) {
    let Ok(cls) = find_class("NSVisualEffectView") else { return };
    let view: *mut NSObject = msg_send![cls, alloc];
    let view: *mut NSObject = msg_send![view, initWithFrame: frame];
    if view.is_null() {
        return;
    }
    // NSVisualEffectMaterialSidebar = 7；BlendingModeBehindWindow = 0；StateActive = 1
    let _: () = msg_send![view, setMaterial: 7isize];
    let _: () = msg_send![view, setBlendingMode: 0isize];
    let _: () = msg_send![view, setState: 1isize];
    let _: () = msg_send![view, setAlphaValue: opacity as f64];
    let _: () = msg_send![content, addSubview: view];
    let _: () = msg_send![view, release];
}

/// 给标签设置 sRGB 文字色 + 不透明度；返回颜色构造是否成功。
///
/// NSColor 的构造细节只写这一处：建窗（`make_label`）与「自动对比色」的就地改色
/// （`set_overlay_text_color`）共用它，保证两条路径产出的颜色完全一致。
/// 先按 alpha = 1.0 构造、再 `colorWithAlphaComponent:` 统一乘不透明度，
/// 调用方就不必关心「有没有重复相乘」。
unsafe fn set_label_color(label: *mut NSObject, rgb: (f64, f64, f64), alpha: f64) -> bool {
    let Ok(color_cls) = find_class("NSColor") else { return false };
    let base: *mut NSObject = msg_send![color_cls,
        colorWithSRGBRed: rgb.0, green: rgb.1, blue: rgb.2, alpha: 1.0f64];
    if base.is_null() {
        return false;
    }
    let color: *mut NSObject = msg_send![base, colorWithAlphaComponent: alpha];
    let _: () = msg_send![label, setTextColor: color];
    true
}

/// 就地更新一块屏幕上三层文字的颜色（不重建窗口、不动布局）。
///
/// 「换壁纸 → 自动改色」走的就是这条路径：重建窗口会带一次 orderOut/orderFront
/// 闪烁，还要重算整组布局与字号，代价远高于三次 setTextColor。
fn set_overlay_text_color(overlay: &ClockOverlay, rgb: (f64, f64, f64), config: &ClockConfig) {
    let opacity = config.text_opacity.clamp(0.0, 1.0) as f64;
    unsafe {
        set_label_color(overlay.time_label, rgb, ALPHA_TIME * opacity);
        if let Some(label) = overlay.date_label {
            set_label_color(label, rgb, ALPHA_DATE * opacity);
        }
        if let Some(label) = overlay.secondary_label {
            set_label_color(label, rgb, ALPHA_SECONDARY * opacity);
        }
    }
}

/// 颜色 → `#RRGGBB`，只用于日志 —— 自动改色是隐式行为，控制台得能核对决策。
fn format_hex(rgb: (f64, f64, f64)) -> String {
    let q = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    format!("#{:02X}{:02X}{:02X}", q(rgb.0), q(rgb.1), q(rgb.2))
}

/// 时间行文本宽度的回退比例（占屏宽）。测量不可用（或在时钟构建之前）时用它。
///
/// 取 0.25 而不是更宽的值：采样带宁可略窄（最多漏判局部对比不足），
/// 也不要把文字两侧背景平均进来 —— 那是误判的来源，会把整体亮度拉偏。
const FALLBACK_TIME_WIDTH_RATIO: f64 = 0.25;

/// 时间行文字在屏宽中的水平起点比例。
///
/// 由「label 框范围 + 对齐方式 + 实测文字宽度」算出 —— **不能假设文字居中**：
/// 对齐方式是可配的（`config.clock.text_align`），采样带若按错的方式算，就会
/// 整体落到文字旁边的空白上，判定随之失真。
fn time_origin_ratio(text_width_ratio: f64, align: TextAlign) -> f64 {
    let x = match align {
        TextAlign::Right => LABEL_X_RATIO + LABEL_W_RATIO - text_width_ratio,
        TextAlign::Center => LABEL_X_RATIO + (LABEL_W_RATIO - text_width_ratio) / 2.0,
    };
    x.clamp(0.0, 1.0)
}

/// 时钟文字在壁纸图上的采样带，返回 `(x0, y0, x1, y1)` 比例坐标
/// （0..1，图像坐标系即 y 轴向下）。供 main.rs 在解码后的壁纸上量平均亮度。
///
/// 与 `build_overlay` 共用同一套几何系数：整组高度 = 时间行 `0.13 × 1.25 × scale`
/// 加日期行、副时区行与两个间隙，组中心在屏幕纵向 `0.52 + vertical_offset` 处
/// （屏幕坐标原点在左下）。这里翻成图像坐标（`y_img = 1 - y_screen`）。
///
/// 亮度采样区域，返回比例坐标 `(x0, y0, x1, y1)`（0..1，图像坐标系 y 轴向下）。
/// 供 main.rs / video.rs 在解码后的壁纸上量平均亮度。
///
/// **区域随 `config.clock.text_align` 切换**：
/// - `TextAlign::Right`：只覆盖时间行文字实际压住的矩形 —— 垂直取三行 label 框
///   的并集（不外扩），水平由实测文本宽度与对齐方式定出起点。文字在屏幕右侧，
///   采别处只会把无关背景平均进来、稀释判断。
/// - `TextAlign::Center`：**整张壁纸** —— 此时文字压在画面中央，用整图平均亮度判定。
///
/// 后两个参数只在 `Right` 模式用到（由 `ClockManager` 的实测表提供）；
/// 为 None 时回退到 `FALLBACK_TIME_WIDTH_RATIO` 与「居中」，不会 panic。
///
/// 注意：此函数只服务于亮度采样，不参与布局；布局仍以 `build_overlay` 为准，
/// 两边的系数若改动必须同步（单测锁住了当前数值）。
pub fn sample_region_ratios(
    config: &ClockConfig,
    time_width_ratio: Option<f64>,
    time_text_origin: Option<f64>,
) -> (f64, f64, f64, f64) {
    if config.text_align == TextAlign::Center {
        return (0.0, 0.0, 1.0, 1.0);
    }

    let time_scale = config.time_scale.clamp(0.5, 2.0) as f64;
    let date_scale = config.date_scale.clamp(0.5, 2.0) as f64;
    let sec_scale = config.secondary_scale.clamp(0.5, 2.0) as f64;
    let time_h = 0.13 * 1.25 * time_scale;
    let date_h = 0.026 * 1.7 * date_scale;
    let sec_h = 0.020 * 1.7 * sec_scale;
    let gap1 = 0.012;
    let gap2 = 0.008;
    let total = time_h
        + if config.show_date { gap1 + date_h } else { 0.0 }
        + if config.secondary_enabled { gap2 + sec_h } else { 0.0 };

    let center = 0.52 + config.vertical_offset.clamp(-0.2, 0.2) as f64;
    let y_center = 1.0 - center;
    // 三行框的并集：group 高度的一半，不做额外外扩
    let half_h = total / 2.0;
    // 水平宽度：下限 0.02 防止测量异常时退化成取不到像素的窄带
    let width = time_width_ratio.unwrap_or(FALLBACK_TIME_WIDTH_RATIO).clamp(0.02, 1.0);
    // 未测量起点时回退到居中（比硬编码一个右侧值更不容易错）
    let origin = time_text_origin.unwrap_or(0.5 - width / 2.0);
    (
        origin.clamp(0.0, 1.0),
        (y_center - half_h).clamp(0.0, 1.0),
        (origin + width).clamp(0.0, 1.0),
        (y_center + half_h).clamp(0.0, 1.0),
    )
}

/// 时间行的「最宽可能文本」：数字位一律用 8（系统字体里数字字形宽度基本一致，
/// 8 属最宽之列），其余部分（上午/下午前缀、冒号、秒位）按实际格式拼。
///
/// 用它而不是当前时刻的文本，有两个好处：采样带不随时间跳动；量出的宽度不会
/// 小于真正显示的文字宽度，也就不会漏掉文字压住的任何一块背景。
fn widest_time_text(config: &ClockConfig) -> String {
    let mut s = String::new();
    if !config.use_24_hour {
        s.push_str("上午 ");
    }
    s.push_str("88:88");
    if config.show_seconds {
        s.push_str(":88");
    }
    s
}

/// 量出时间行文本占屏宽的比例（0..1）；返回 None 表示测量不可用。
///
/// 为什么必须实测而不是用固定比例：采样带要贴合文字真正压住的那块背景。
/// 早先用「屏宽 18%~82%」的固定宽带，会把文字两侧的背景一并平均进来 ——
/// 那里可能亮暗迥异，结果是本可读的文字被误判成需要改色，或者反过来。
///
/// 必须在主线程调用（AppKit 字体测量）。
unsafe fn measure_time_width_ratio(config: &ClockConfig, screen_w: f64, screen_h: f64) -> Option<f64> {
    if screen_w <= 0.0 || screen_h <= 0.0 {
        return None;
    }
    // 字号公式与 build_overlay 一致（按屏高缩放）
    let time_scale = config.time_scale.clamp(0.5, 2.0) as f64;
    let time_size = screen_h * 0.13 * time_scale;
    let font_cls = find_class("NSFont").ok()?;
    let weight = config.time_font_weight.clamp(-1.0, 0.6) as f64;
    let font: *mut NSObject = msg_send![font_cls,
        systemFontOfSize: time_size, weight: weight];
    if font.is_null() {
        return None;
    }
    let width = measure_text_width(&widest_time_text(config), font);
    if !(width > 0.0) {
        return None;
    }
    Some((width / screen_w).clamp(0.02, 1.0))
}

/// 用 AppKit 量一段文本在指定字体下的排版宽度（点）。
/// 走 `NSStringDrawing` 的 `sizeWithAttributes:`；属性字典只用 NSFont 一项，
/// 键名取 `NSFontAttributeName` 的字面值（AppKit 自 10.0 起未变）。
unsafe fn measure_text_width(text: &str, font: *mut NSObject) -> f64 {
    let Ok(dict_cls) = find_class("NSDictionary") else { return 0.0 };
    let key = NSString::from_str("NSFont");
    let attrs: *mut NSObject = msg_send![dict_cls, dictionaryWithObject: font, forKey: &*key];
    if attrs.is_null() {
        return 0.0;
    }
    let ns_text = NSString::from_str(text);
    let size: NSSize = msg_send![&*ns_text, sizeWithAttributes: attrs];
    size.width
}

/// 量一行文本在指定字号/字重下的排版宽度（点）。毛玻璃卡片靠它收缩到文字
/// 实际压住的范围，而不是铺满整个 label 框。
unsafe fn measure_line_width(text: &str, font_size: f64, weight: f64) -> f64 {
    let Ok(font_cls) = find_class("NSFont") else { return 0.0 };
    let font: *mut NSObject = msg_send![font_cls, systemFontOfSize: font_size, weight: weight];
    if font.is_null() {
        return 0.0;
    }
    measure_text_width(text, font)
}

/// 解析 `#RRGGBB` / `RRGGBB` / `#RGB` 为 0..1 的 sRGB 分量。
/// 返回 None 表示格式非法（设置面板据此标红并拒绝应用）。
pub fn parse_hex_color(text: &str) -> Option<(f64, f64, f64)> {
    let hex = text.trim().trim_start_matches('#');
    let (r, g, b) = match hex.len() {
        // #RGB 简写：每位复制成两位（F → 0xFF）
        3 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            (((n >> 8) & 0xf) * 17, ((n >> 4) & 0xf) * 17, (n & 0xf) * 17)
        }
        6 => {
            let n = u32::from_str_radix(hex, 16).ok()?;
            ((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff)
        }
        _ => return None,
    };
    Some((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
}

/// 归一化为配置里保存的标准写法 `#RRGGBB`（大写）；非法输入返回 None。
pub fn canonical_hex(text: &str) -> Option<String> {
    let (r, g, b) = parse_hex_color(text)?;
    let q = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u32;
    Some(format!("#{:02X}{:02X}{:02X}", q(r), q(g), q(b)))
}

fn set_label_text(label: *mut NSObject, text: &str) {
    unsafe {
        let _: () = msg_send![label, setStringValue: &*NSString::from_str(text)];
    }
}

// ---------------------------------------------------------------------------
// 时区与时间换算
// ---------------------------------------------------------------------------

/// 通过 NSTimeZone 获取时区相对 GMT 的当前偏移（秒），自动包含夏令时。
/// "local"、空串或无法识别的名称都回退到系统本地时区。
fn tz_offset_seconds(name: &str) -> i64 {
    unsafe {
        let Ok(cls) = find_class("NSTimeZone") else { return 0 };
        let tz: *mut NSObject = if name.is_empty() || name == "local" {
            msg_send![cls, localTimeZone]
        } else {
            let tz: *mut NSObject = msg_send![cls, timeZoneWithName: &*NSString::from_str(name)];
            if tz.is_null() {
                msg_send![cls, localTimeZone]
            } else {
                tz
            }
        };
        if tz.is_null() {
            return 0;
        }
        msg_send![tz, secondsFromGMT]
    }
}

/// 从 IANA 时区名提取城市名："Europe/London" → "London"，"America/New_York" → "New York"。
fn city_name_from_tz(tz: &str) -> String {
    tz.rsplit('/')
        .next()
        .unwrap_or(tz)
        .replace('_', " ")
}

struct BrokenTime {
    days: i64, // 自 1970-01-01 的天数（用于跨天比较）
    hour: i64,
    minute: i64,
    second: i64,
    month: u32,
    day: u32,
    weekday: usize, // 0 = 周日
}

impl BrokenTime {
    fn from_epoch(local_secs: i64) -> Self {
        let days = local_secs.div_euclid(86_400);
        let secs = local_secs.rem_euclid(86_400);
        let (_, month, day) = civil_from_days(days);
        // 1970-01-01 是周四（weekday 索引 4）
        let weekday = (days.rem_euclid(7) + 4) as usize % 7;
        Self {
            days,
            hour: secs / 3600,
            minute: (secs % 3600) / 60,
            second: secs % 60,
            month,
            day,
            weekday,
        }
    }
}

/// Howard Hinnant 的 civil_from_days 算法：天数 → (year, month, day)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 时间文案。`show_seconds` 为真时附加秒位（由 1s 刷新定时器驱动）。
fn format_time(hour: i64, minute: i64, second: i64, use_24_hour: bool, show_seconds: bool) -> String {
    let sec = if show_seconds { format!(":{:02}", second) } else { String::new() };
    if use_24_hour {
        format!("{:02}:{:02}{}", hour, minute, sec)
    } else {
        let h12 = match hour % 12 {
            0 => 12,
            h => h,
        };
        let period = if hour < 12 { "上午" } else { "下午" };
        format!("{} {}:{:02}{}", period, h12, minute, sec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_civil_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_792), (2026, 12, 5));
    }

    #[test]
    fn test_break_down_epoch() {
        // 1970-01-01 00:00:00 UTC → 周四
        let t = BrokenTime::from_epoch(0);
        assert_eq!((t.month, t.day, t.weekday), (1, 1, 4));
        // 负时间戳（1969-12-31 23:00，周三）
        let t = BrokenTime::from_epoch(-3600);
        assert_eq!((t.month, t.day, t.weekday, t.hour), (12, 31, 3, 23));
    }

    #[test]
    fn test_format_time() {
        // 24 小时制
        assert_eq!(format_time(9, 5, 0, true, false), "09:05");
        assert_eq!(format_time(23, 59, 0, true, false), "23:59");
        // 12 小时制（上午/下午）
        assert_eq!(format_time(0, 5, 0, false, false), "上午 12:05");
        assert_eq!(format_time(13, 5, 0, false, false), "下午 1:05");
        // 带秒
        assert_eq!(format_time(9, 5, 7, true, true), "09:05:07");
        assert_eq!(format_time(13, 5, 7, false, true), "下午 1:05:07");
    }

    #[test]
    fn test_seconds_from_epoch() {
        assert_eq!(
            {
                let t = BrokenTime::from_epoch(3_661);
                (t.hour, t.minute, t.second)
            },
            (1, 1, 1)
        );
    }

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(parse_hex_color("#FFFFFF"), Some((1.0, 1.0, 1.0)));
        assert_eq!(parse_hex_color("000000"), Some((0.0, 0.0, 0.0)));
        // #RGB 简写按位复制展开
        assert_eq!(parse_hex_color("#fff"), Some((1.0, 1.0, 1.0)));
        // 非法输入一律返回 None，交给设置面板拦下
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#ZZZZZZ"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    #[test]
    fn test_canonical_hex() {
        assert_eq!(canonical_hex("#fff").as_deref(), Some("#FFFFFF"));
        assert_eq!(canonical_hex("  #a1b2c3 ").as_deref(), Some("#A1B2C3"));
        assert_eq!(canonical_hex("nope"), None);
    }

    #[test]
    fn test_city_name_from_tz() {
        assert_eq!(city_name_from_tz("Europe/London"), "London");
        assert_eq!(city_name_from_tz("America/New_York"), "New York");
    }

    #[test]
    fn test_format_hex() {
        assert_eq!(format_hex((1.0, 1.0, 1.0)), "#FFFFFF");
        assert_eq!(format_hex((0.0, 0.0, 0.0)), "#000000");
        assert_eq!(format_hex((1.0, 136.0 / 255.0, 0.0)), "#FF8800");
    }

    #[test]
    fn test_sample_region_right_default() {
        let c = ClockConfig::default();
        assert_eq!(c.text_align, TextAlign::Right, "默认必须保持历史行为");
        let (x0, y0, x1, y1) = sample_region_ratios(&c, None, None);
        // 未测量时水平回退到 FALLBACK_TIME_WIDTH_RATIO (0.25)，起点回退到居中
        assert!((x0 - 0.375).abs() < 1e-9, "x0 = {}", x0);
        assert!((x1 - 0.625).abs() < 1e-9, "x1 = {}", x1);
        // 垂直方向是时钟组框的并集，**不做外扩**：整组约 26% 屏高，中心在
        // 图像坐标 0.48 处
        assert!((y0 - 0.3497).abs() < 5e-3, "y0 = {}", y0);
        assert!((y1 - 0.6104).abs() < 5e-3, "y1 = {}", y1);
        assert!((y1 - y0 - 0.2607).abs() < 5e-3, "组高应贴合时钟，实测 {}", y1 - y0);
        // 采样带必须罩住时钟组中心
        assert!(y0 < 0.48 && y1 > 0.48);
    }

    #[test]
    fn test_sample_region_center_covers_whole_image() {
        let mut c = ClockConfig::default();
        c.text_align = TextAlign::Center;
        // 居中模式：文字压在画面中央，判定依据整张壁纸 —— 实测值不再参与
        assert_eq!(sample_region_ratios(&c, Some(0.2), Some(0.7)), (0.0, 0.0, 1.0, 1.0));
        assert_eq!(sample_region_ratios(&c, None, None), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn test_sample_region_right_follows_measured_width_and_origin() {
        let c = ClockConfig::default();
        // 起点与宽度都由调用方给定，区域必须精确落在这段区间上
        for (width, origin) in [(0.10, 0.85), (0.213, 0.737), (0.45, 0.05)] {
            let (x0, _, x1, _) = sample_region_ratios(&c, Some(width), Some(origin));
            assert!((x0 - origin).abs() < 1e-9, "起点未跟随: {}", x0);
            assert!((x1 - x0 - width).abs() < 1e-9, "宽度未跟随: {}", x1 - x0);
        }
        // 右对齐的典型取值：起点 = 0.95 − 宽度，落在屏幕右侧
        let (x0, _, x1, _) = sample_region_ratios(&c, Some(0.213), Some(0.737));
        assert!(x0 > 0.7 && x1 <= 0.95 + 1e-9, "右对齐文字应落在屏幕右侧");
        // 异常值被夹住：过窄会退化到取不到像素，越界则失去意义
        let (x0, _, x1, _) = sample_region_ratios(&c, Some(0.0), Some(0.5));
        assert!(x1 - x0 >= 0.02 - 1e-9, "宽度下限未生效");
        let (x0, _, x1, _) = sample_region_ratios(&c, Some(0.9), Some(0.5));
        assert!(x0 >= 0.0 && x1 <= 1.0, "上界未生效");
    }

    #[test]
    fn test_time_origin_ratio_per_alignment() {
        // label 框是屏宽的 5%~95%（与 build_overlay 一致）
        let w = 0.213;
        // 靠右：贴 label 框右边缘
        assert!((time_origin_ratio(w, TextAlign::Right) - (0.95 - w)).abs() < 1e-9);
        // 居中：两侧留白相等
        assert!((time_origin_ratio(w, TextAlign::Center) - (0.5 - w / 2.0)).abs() < 1e-9);
        // 极端宽度下仍落在合法区间
        let o = time_origin_ratio(2.0, TextAlign::Right);
        assert!((0.0..=1.0).contains(&o), "越界: {}", o);
    }

    #[test]
    fn test_ns_text_align_locks_abi_values() {
        // 锁住 ABI 假设：Apple Silicon 上 Center=1/Right=2，Intel 上相反。
        // 这个测试的意义是防止有人再把裸数字写回 setAlignment:。
        assert_ne!(
            ns_text_align(TextAlign::Center),
            ns_text_align(TextAlign::Right),
            "Center 与 Right 不能是同一个值"
        );
        if cfg!(target_arch = "x86_64") {
            assert_eq!(ns_text_align(TextAlign::Center), 2);
            assert_eq!(ns_text_align(TextAlign::Right), 1);
        } else {
            assert_eq!(ns_text_align(TextAlign::Center), 1);
            assert_eq!(ns_text_align(TextAlign::Right), 2);
        }
    }

    #[test]
    fn test_sample_region_follows_offset_and_line_count() {
        let base = ClockConfig::default();
        let (_, base_y0, _, base_y1) = sample_region_ratios(&base, None, None);

        // 垂直偏移为正 = 屏幕坐标上移 = 图像坐标 y 减小
        let mut up = base.clone();
        up.vertical_offset = 0.2;
        let (_, y0, _, y1) = sample_region_ratios(&up, None, None);
        assert!(y0 < base_y0 && y1 < base_y1, "offset 未跟随: {} .. {}", y0, y1);

        // 只显示时间行时整组变矮，采样带随之收窄
        let mut minimal = base.clone();
        minimal.show_date = false;
        minimal.secondary_enabled = false;
        let (_, m0, _, m1) = sample_region_ratios(&minimal, None, None);
        assert!(
            m1 - m0 < base_y1 - base_y0,
            "极简样式的采样带应更窄: {} vs {}",
            m1 - m0,
            base_y1 - base_y0
        );

        // 极端偏移也必须落在合法区间内
        let mut extreme = base.clone();
        extreme.vertical_offset = 0.2;
        extreme.time_scale = 2.0;
        extreme.date_scale = 2.0;
        extreme.secondary_scale = 2.0;
        let (_, e0, _, e1) = sample_region_ratios(&extreme, None, None);
        assert!((0.0..=1.0).contains(&e0) && (0.0..=1.0).contains(&e1) && e0 < e1);
    }
}
