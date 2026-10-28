//! 壁纸亮度采样 + 时钟文字色的自动对比度修正（纯函数，全部可单测）。
//!
//! 要解决的问题：时钟悬浮在壁纸上，当壁纸局部灰度与文字色接近时（暗壁纸配
//! 深灰字、亮壁纸配白字、或是中间调撞色），文字会「糊」进背景。
//!
//! 设计取舍：
//! - 只用 WCAG 相对亮度做量化，不引入感知模型依赖 —— 阈值可解释、可回归测试。
//! - 修正策略是「按通道比例整体推亮度，刚好达标即停」，而不是切黑/白。
//!   通道比例承载着色相与彩度，是用户的审美决策（见 config.clock.text_color），
//!   算法只负责解决可见性，不该顺手把它抹成黑白。
//! - 为什么不用 HSL 保 H/S 的做法：HSL 的 S 是**相对**饱和度，低彩度的高亮度色
//!   会被算成 s = 1.0（暖白 #FFF3E0 就是），一旦压低 L，彩度被剧烈放大 ——
//!   暖白会变成饱和橙，观感跳变。等比缩放通道（压暗）/ 向白插值（提亮）保住的是
//!   彩度观感：暖白压暗后仍是暖灰；而对 #FF8800 这类本来就饱和的颜色，两条路径
//!   的结果几乎一致。
//! - 「刚好达标即停」还有一个好处：中等亮度背景上不会把白字一路压到纯黑，
//!   避免观感突变。
//!
//! 注意：本模块按文字色**不透明**参与判定。实际渲染时时间/日期/副时区各有
//! 0.95/0.85/0.70 的层级系数（clock.rs），再乘用户的 text_opacity。辅助行的
//! 实际对比度会略低于计算值；因此把目标定在 3:1（大字号 AA）而不是更低，
//! 相当于给透明度留了余量。

use image::RgbaImage;

/// 目标对比度（WCAG 大字号 AA）。达到即认定「看得清」，不再动色。
pub const CONTRAST_TARGET: f64 = 3.0;
/// 严重不足阈值：低于它说明文字与背景几乎同色，无论壁纸亮度是否落在极端区，
/// 都必须改色（覆盖「中间调撞色」这类盲区）。
pub const CONTRAST_CRITICAL: f64 = 1.8;
/// 判定「壁纸偏暗 / 偏亮」的边界。只有亮度落在极端区，才允许在对比度「轻度
/// 不足」（CONTRAST_CRITICAL .. CONTRAST_TARGET）时改色 —— 中间调壁纸上的
/// 轻微不足属于正常观感差异，频繁换色比不改更糟。
pub const LUMA_DARK_MAX: f64 = 0.35;
pub const LUMA_LIGHT_MIN: f64 = 0.65;

/// 采样步长（像素）。4K 图上按 4 抽样约 50 万点，几毫秒内完成，精度足够。
pub const SAMPLE_STEP: u32 = 4;

/// 二分迭代次数。12 次把亮度区间收敛到 1/4096，远细于 8bit 色阶。
const BISECT_ITERS: usize = 12;

/// sRGB 分量 → 线性光。WCAG 的对比度是在线性空间算的，
/// 直接拿 sRGB 值做加权会把暗部对比度高估一大截。
pub fn srgb_to_linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 相对亮度（0 = 黑，1 = 白）。分量取 0..1 的 sRGB 值。
pub fn relative_luminance(rgb: (f64, f64, f64)) -> f64 {
    0.2126 * srgb_to_linear(rgb.0)
        + 0.7152 * srgb_to_linear(rgb.1)
        + 0.0722 * srgb_to_linear(rgb.2)
}

/// WCAG 对比度：两个相对亮度之比，范围 1.0（同色）.. 21.0（黑白）。
pub fn contrast_ratio(a: f64, b: f64) -> f64 {
    let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// 当前文字色叠在亮度为 `bg_luma` 的壁纸上，是否需要改色。
///
/// 判据是两级：
/// 1. 对比度达标（≥ 3:1）→ 一律不改，即使壁纸很暗（暗壁纸配白字本来就是最
///    清楚的情况，没有理由动它）。
/// 2. 对比度严重不足（< 1.8:1）→ 一律改，不看壁纸亮度。
/// 3. 轻度不足（1.8 .. 3.0）→ 只有壁纸亮度落在极端区才改，避免中间调上
///    因为一个采样值就换色。
pub fn needs_contrast_fix(text_rgb: (f64, f64, f64), bg_luma: f64) -> bool {
    let cr = contrast_ratio(relative_luminance(text_rgb), bg_luma);
    if cr >= CONTRAST_TARGET {
        return false;
    }
    if cr < CONTRAST_CRITICAL {
        return true;
    }
    bg_luma <= LUMA_DARK_MAX || bg_luma >= LUMA_LIGHT_MIN
}

/// 给出「按通道比例推亮度、刚好达标」的替代文字色；不需要改色时原样返回。
///
/// 方向选择不是简单的「背景亮就压暗」：WCAG 下能否靠**提亮**达标取决于背景
/// 究竟有多暗 —— 提亮需要把前景相对亮度推到 `3*(bg+0.05)-0.05`，只有该值
/// ≤ 1.0（即 bg ≤ 0.3）时才可行，否则压暗是唯一出路。所以分界点取 0.3，
/// 而不是直觉上的 0.5。
pub fn auto_contrast_color(text_rgb: (f64, f64, f64), bg_luma: f64) -> (f64, f64, f64) {
    if !needs_contrast_fix(text_rgb, bg_luma) {
        return text_rgb;
    }

    let lighten_bound = 3.0 * (bg_luma + 0.05) - 0.05;

    if lighten_bound <= 1.0 {
        // 提亮方向：向白插值，取「满足达标前提下最小」的插值量 ——
        // 也就是最接近用户原色的解
        let bound = lighten_bound.clamp(0.0, 1.0);
        let t = smallest_factor_above(0.0, 1.0, |f| mix_to_white(text_rgb, f), bound);
        mix_to_white(text_rgb, t)
    } else {
        // 压暗方向：等比缩放通道，取「满足达标前提下最大」的缩放系数
        let bound = ((bg_luma + 0.05) / CONTRAST_TARGET - 0.05).clamp(0.0, 1.0);
        let k = largest_factor_below(0.0, 1.0, |f| scale_rgb(text_rgb, f), bound);
        scale_rgb(text_rgb, k)
    }
}

/// 整体压暗：三个通道等比缩放。通道比例（色相与彩度的来源）因此保持不变，
/// 这也是它与「调整 HSL 的 L」在低彩度色上产生分歧的地方（见模块头注释）。
fn scale_rgb(rgb: (f64, f64, f64), k: f64) -> (f64, f64, f64) {
    (rgb.0 * k, rgb.1 * k, rgb.2 * k)
}

/// 整体提亮：向白插值。`t = 0` 是原色、`t = 1` 是纯白；
/// 各通道的「与白的距离」按同一比例收缩，色相与彩度观感同样保持。
fn mix_to_white(rgb: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        rgb.0 + (1.0 - rgb.0) * t,
        rgb.1 + (1.0 - rgb.1) * t,
        rgb.2 + (1.0 - rgb.2) * t,
    )
}

/// 采样图片指定区域的平均相对亮度。
///
/// 区域用 0..1 的**比例**坐标（图像坐标系，y 轴向下），这样同一套参数对任意
/// 分辨率的壁纸都成立。按 `step` 像素抽样，避免 4K 图全量遍历。
/// 尺寸非法或区域内取不到像素时返回 0.5（中性值 → 判定必然不触发改色）。
pub fn sample_region_luma(
    img: &RgbaImage,
    x0f: f64,
    y0f: f64,
    x1f: f64,
    y1f: f64,
    step: u32,
) -> f64 {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return 0.5;
    }
    let (x0, x1) = pixel_span(x0f, x1f, w);
    let (y0, y1) = pixel_span(y0f, y1f, h);
    if x1 <= x0 || y1 <= y0 {
        return 0.5;
    }

    let step = step.max(1);
    let mut sum = 0.0;
    let mut count: u64 = 0;
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let p = img.get_pixel(x, y);
            sum += relative_luminance((
                p[0] as f64 / 255.0,
                p[1] as f64 / 255.0,
                p[2] as f64 / 255.0,
            ));
            count += 1;
            x += step;
        }
        y += step;
    }

    if count == 0 {
        0.5
    } else {
        sum / count as f64
    }
}

/// 比例区间 → 像素区间 [lo, hi)，参数顺序颠倒也能得到合法区间。
fn pixel_span(a: f64, b: f64, total: u32) -> (u32, u32) {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let scale = total as f64;
    let lo_px = (lo.clamp(0.0, 1.0) * scale) as u32;
    // 右/下边界向上取整：否则宽 1 像素的窄带会被 round 掉，采不到任何点
    let hi_px = (hi.clamp(0.0, 1.0) * scale).ceil() as u32;
    (lo_px.min(total), hi_px.clamp(lo_px, total))
}

/// 在 [lo, hi] 上二分出**最小**的系数，使其构造出的颜色相对亮度 ≥ `bound`（提亮方向）。
///
/// `f` 是「系数 → 颜色」的映射；相对亮度随系数单调递增是使用前提，
/// `mix_to_white` 与 `scale_rgb` 都满足这个前提。
fn smallest_factor_above<F>(mut lo: f64, mut hi: f64, f: F, bound: f64) -> f64
where
    F: Fn(f64) -> (f64, f64, f64),
{
    for _ in 0..BISECT_ITERS {
        let mid = (lo + hi) * 0.5;
        if relative_luminance(f(mid)) >= bound {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// 在 [lo, hi] 上二分出**最大**的系数，使其构造出的颜色相对亮度 ≤ `bound`（压暗方向）。
fn largest_factor_below<F>(mut lo: f64, mut hi: f64, f: F, bound: f64) -> f64
where
    F: Fn(f64) -> (f64, f64, f64),
{
    for _ in 0..BISECT_ITERS {
        let mid = (lo + hi) * 0.5;
        if relative_luminance(f(mid)) <= bound {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn rgb(hex: u32) -> (f64, f64, f64) {
        (
            ((hex >> 16) & 0xff) as f64 / 255.0,
            ((hex >> 8) & 0xff) as f64 / 255.0,
            (hex & 0xff) as f64 / 255.0,
        )
    }

    fn bytes(c: (f64, f64, f64)) -> [u8; 3] {
        [
            (c.0 * 255.0).round() as u8,
            (c.1 * 255.0).round() as u8,
            (c.2 * 255.0).round() as u8,
        ]
    }

    #[test]
    fn test_srgb_to_linear_endpoints() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-9);
        // 线性段与幂段的分界点两侧必须连续（否则深色区会有台阶）
        let lo = srgb_to_linear(0.04045);
        let hi = srgb_to_linear(0.04045 + 1e-9);
        assert!((hi - lo).abs() < 1e-6);
    }

    #[test]
    fn test_relative_luminance_extremes() {
        assert!(relative_luminance(rgb(0x000000)).abs() < 1e-9);
        assert!((relative_luminance(rgb(0xFFFFFF)) - 1.0).abs() < 1e-9);
        // 绿色权重最高、蓝色最低 —— 白平衡不敏感的场景下这是 WCAG 的定义
        assert!(relative_luminance(rgb(0x00FF00)) > relative_luminance(rgb(0xFF0000)));
        assert!(relative_luminance(rgb(0x0000FF)) < relative_luminance(rgb(0xFF0000)));
    }

    #[test]
    fn test_contrast_ratio_extremes() {
        // 黑白是最极端的对比，WCAG 上限固定 21:1
        assert!((contrast_ratio(1.0, 0.0) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(0.5, 0.5) - 1.0).abs() < 1e-9);
        // 参数顺序无关
        assert_eq!(contrast_ratio(0.2, 0.8), contrast_ratio(0.8, 0.2));
    }

    #[test]
    fn test_needs_contrast_fix_cases() {
        let white = rgb(0xFFFFFF);
        // 白字配黑壁纸：本来就最清楚，绝不能改
        assert!(!needs_contrast_fix(white, 0.0));
        // 白字配白壁纸：严重不足 → 必须改
        assert!(needs_contrast_fix(white, 1.0));
        // 深灰字配中灰壁纸（撞色）：中间调也不该放过
        let mid_gray = rgb(0x8C8C8C);
        assert!(needs_contrast_fix(mid_gray, relative_luminance(mid_gray)));
        // 白字配中间亮度壁纸：对比度约 4.7:1，达标 → 不改
        assert!(!needs_contrast_fix(white, 0.2));
    }

    #[test]
    fn test_auto_contrast_light_wallpaper_darkens() {
        let white = rgb(0xFFFFFF);
        let out = auto_contrast_color(white, 1.0);
        // 白字是灰阶，等比缩放后仍必须是灰阶（三通道相等）
        assert_eq!(out.0, out.1);
        assert_eq!(out.1, out.2);
        let cr = contrast_ratio(relative_luminance(out), 1.0);
        assert!(cr >= CONTRAST_TARGET - 1e-3, "contrast after fix = {}", cr);
        // 「刚好达标即停」：不该一路压到纯黑
        assert!(relative_luminance(out) > 0.05, "over-darkened: {:?}", bytes(out));
    }

    #[test]
    fn test_auto_contrast_dark_wallpaper_lightens() {
        // 深色字配黑壁纸 → 必须提亮才看得见
        let dark = rgb(0x1A1A1A);
        let out = auto_contrast_color(dark, 0.0);
        let cr = contrast_ratio(relative_luminance(out), 0.0);
        assert!(cr >= CONTRAST_TARGET - 1e-3, "contrast after fix = {}", cr);
        assert!(relative_luminance(out) > relative_luminance(dark));
    }

    #[test]
    fn test_auto_contrast_keeps_channel_ratios() {
        // #FF8800 撞上亮壁纸：压暗必须等比 —— g/r 比例承载色相观感，不能漂移
        let orange = rgb(0xFF8800);
        let out = auto_contrast_color(orange, 0.9);
        assert!(out.0 < orange.0, "应当被压暗");
        assert!(
            (out.1 / out.0 - orange.1 / orange.0).abs() < 1e-6,
            "通道比例漂移: {:?} -> {:?}",
            bytes(orange),
            bytes(out)
        );
        assert!(out.2.abs() < 1e-9);
        let cr = contrast_ratio(relative_luminance(out), 0.9);
        assert!(cr >= CONTRAST_TARGET - 1e-3, "contrast after fix = {}", cr);
        // 不能压过头：结果亮度应贴近阈值而不是 0
        assert!(relative_luminance(out) > 0.05, "over-darkened: {:?}", bytes(out));
    }

    #[test]
    fn test_auto_contrast_keeps_low_chroma_low() {
        // 回归测试：暖白 #FFF3E0 撞上纯白壁纸。
        // 若改用「HSL 保 H/S」的方案，这里会得到饱和橙（HSL 的 S 是相对饱和度，
        // 低彩度亮色被算成 s = 1.0，L 一降彩度就被放大）；等比缩放不会。
        let warm_white = rgb(0xFFF3E0);
        let out = auto_contrast_color(warm_white, 1.0);
        let chroma = out.0.max(out.1).max(out.2) - out.0.min(out.1).min(out.2);
        assert!(
            chroma <= 0.10,
            "彩度被放大: {:?} (chroma={:.3})",
            bytes(out),
            chroma
        );
        // 结果应是「更暗的暖白」：r > g > b 的通道顺序保持不变
        assert!(out.0 > out.1 && out.1 > out.2, "通道顺序被破坏: {:?}", bytes(out));
        let cr = contrast_ratio(relative_luminance(out), 1.0);
        assert!(cr >= CONTRAST_TARGET - 1e-3, "contrast after fix = {}", cr);
    }

    #[test]
    fn test_auto_contrast_noop_when_readable() {
        // 白字配纯黑壁纸：可达标，必须原样返回（包括分量完全相等）
        let white = rgb(0xFFFFFF);
        assert_eq!(auto_contrast_color(white, 0.0), white);
        // 亮壁纸上的深色字同样不该被动
        let dark = rgb(0x111111);
        assert_eq!(auto_contrast_color(dark, 0.9), dark);
    }

    #[test]
    fn test_sample_region_luma_uniform_image() {
        let black = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 255]));
        assert!(sample_region_luma(&black, 0.0, 0.0, 1.0, 1.0, SAMPLE_STEP) < 1e-9);
        let white = RgbaImage::from_pixel(64, 64, Rgba([255, 255, 255, 255]));
        assert!((sample_region_luma(&white, 0.0, 0.0, 1.0, 1.0, SAMPLE_STEP) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_sample_region_luma_respects_bounds() {
        // 上半黑、下半白：只采上半区应得到 0，只采下半区应得到 1
        let mut img = RgbaImage::new(32, 32);
        for (_, y, p) in img.enumerate_pixels_mut() {
            let v = if y < 16 { 0 } else { 255 };
            *p = Rgba([v, v, v, 255]);
        }
        assert!(sample_region_luma(&img, 0.0, 0.0, 1.0, 0.5, 1) < 1e-9);
        assert!((sample_region_luma(&img, 0.0, 0.5, 1.0, 1.0, 1) - 1.0).abs() < 1e-9);
        // 整图平均约 0.5（sRGB 中点转线性后也是 0.5）
        let all = sample_region_luma(&img, 0.0, 0.0, 1.0, 1.0, 1);
        assert!((all - 0.5).abs() < 0.02, "got {}", all);
        // 参数顺序颠倒应得到同一结果
        assert_eq!(
            sample_region_luma(&img, 1.0, 1.0, 0.0, 0.0, 4),
            sample_region_luma(&img, 0.0, 0.0, 1.0, 1.0, 4)
        );
    }

    #[test]
    fn test_sample_region_luma_degenerate_inputs() {
        // 空区域 / 空图片都不能 panic，统一回落到中性值 0.5
        let img = RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 255]));
        assert_eq!(sample_region_luma(&img, 0.5, 0.5, 0.5, 0.5, 4), 0.5);
        let zero = RgbaImage::new(0, 0);
        assert_eq!(sample_region_luma(&zero, 0.0, 0.0, 1.0, 1.0, 4), 0.5);
    }
}
