//! 刘海屏黑色矩阵（渲染进壁纸图片）。
//!
//! 为什么不做叠加窗口：任何盖在菜单栏之上的窗口都会把菜单标题与状态图标一起
//! 遮住。把黑条画进壁纸本身，菜单栏的材质会自然采样到这块黑色，刘海与顶部
//! 黑边融为一体，而菜单栏内容照常可见。
//!
//! 对齐的关键前提：输出图必须**正好等于屏幕物理像素尺寸**。系统按 Cover 语义
//! 缩放并居中裁切壁纸，只有尺寸完全一致时，我们画的 N 像素黑条才会被 1:1 映射
//! 到屏幕上的 N 个物理像素，黑条高度才等于刘海黑框高度。

use objc2_app_kit::NSScreen;

/// 屏幕顶部需要被黑色矩阵覆盖的几何信息（长度单位 pt）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchGeometry {
    /// 刘海黑框高度（pt），来自 `NSScreen.safeAreaInsets.top`
    pub height_pt: f64,
    /// 逻辑点 → 物理像素的比例（Retina 通常为 2.0）
    pub backing_scale: f64,
}

impl NotchGeometry {
    /// 边框高度对应的物理像素数
    pub fn height_px(&self) -> u32 {
        (self.height_pt * self.backing_scale).round().max(0.0) as u32
    }

    /// 圆角半径（pt）换算成物理像素
    pub fn radius_px(&self, radius_pt: f32) -> u32 {
        (radius_pt as f64 * self.backing_scale).round().max(0.0) as u32
    }
}

/// 该屏的刘海几何；无刘海（`safeAreaInsets.top == 0`）返回 None。
///
/// 这是唯一可靠的运行时判据：外接显示器与无刘海机型该值为 0，因此「默认开启」
/// 对它们没有任何副作用，也不会让无刘海屏多走一遍图像合成。
pub fn geometry(screen: &NSScreen) -> Option<NotchGeometry> {
    let insets = screen.safeAreaInsets();
    if insets.top <= 0.0 {
        return None;
    }
    let frame = screen.frame();
    if frame.size.width <= 0.0 {
        return None;
    }
    // 复用 screen.rs 的换算，避免在这里重复一份 backingScaleFactor 的 msg_send
    let (px_width, _) = crate::screen::screen_pixel_size(screen);
    let backing_scale = px_width as f64 / frame.size.width;
    if !(backing_scale.is_finite() && backing_scale > 0.0) {
        return None;
    }
    Some(NotchGeometry { height_pt: insets.top, backing_scale })
}

/// 按 Cover 语义把图像铺满并居中裁切到精确的目标像素尺寸。
///
/// 与 `wallpaper::fit_for_screen` 的区别：那个「只缩不放」是为了省算力；
/// 这里**必须**得到精确尺寸（必要时放大），否则系统二次缩放会让黑条高度
/// 偏离刘海高度几个像素，视觉上就是一截没盖住的黑边。
pub fn cover_to(img: image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    let (sw, sh) = img.dimensions();
    if width == 0 || height == 0 || sw == 0 || sh == 0 {
        return img;
    }
    if sw == width && sh == height {
        return img;
    }

    let scale = (width as f32 / sw as f32).max(height as f32 / sh as f32);
    let nw = ((sw as f32 * scale).round() as u32).max(width);
    let nh = ((sh as f32 * scale).round() as u32).max(height);
    let scaled = if nw == sw && nh == sh {
        img
    } else {
        image::imageops::thumbnail(&img, nw, nh)
    };

    let (cw, ch) = scaled.dimensions();
    // 居中裁切，和系统 Cover 的对齐口径一致（多出来的部分两侧均分）
    let x = (cw - width) / 2;
    let y = (ch - height) / 2;
    image::imageops::crop_imm(&scaled, x, y, width, height).to_image()
}

/// 在图像顶部画一条纯黑矩阵，并把**下方壁纸的顶角**切成圆角。
///
/// 圆角是「外翻」而不是「内收」：黑条在左右两侧向下多延伸 `radius`，缺口处
/// 用四分之一圆弧过渡，于是黑色从刘海向两侧铺开、壁纸的上边缘呈圆角收进来。
/// （早期版本画反了：把黑条自己的底角削圆，结果壁纸在角上顶到屏幕边缘。）
///
/// 遍历范围只有黑条加圆角那几十行（约 96 行 × 3420 列），代价可忽略；
/// 圆弧边界用 1px 覆盖率做抗锯齿，避免 2 倍屏上出现台阶。
pub fn draw_top_bar(img: &mut image::RgbaImage, bar_height_px: u32, radius_px: u32) {
    let (w, h) = img.dimensions();
    if bar_height_px == 0 || w == 0 || h == 0 {
        return;
    }
    let bar_h = bar_height_px.min(h);
    // 圆角半径不能超过半屏宽，也不能超出图像剩余高度
    let r = (radius_px as f32).min(w as f32 / 2.0).min((h - bar_h) as f32);
    let total_rows = (bar_h as f32 + r).min(h as f32) as u32;

    for y in 0..total_rows {
        let yc = y as f32 + 0.5;
        for x in 0..w {
            let coverage = if y < bar_h {
                // 直边部分：整行纯黑
                1.0
            } else if r <= 0.0 {
                0.0
            } else {
                let xc = x as f32 + 0.5;
                // 到最近一个圆角圆心的水平距离（圆心在 x = r 与 x = w - r 处）
                let dx = if xc < r {
                    r - xc
                } else if xc > w as f32 - r {
                    xc - (w as f32 - r)
                } else {
                    0.0
                };
                if dx <= 0.0 {
                    // 两侧圆心之间：黑条已经结束，这里是壁纸
                    0.0
                } else {
                    // 圆心 y = bar_h + r；到圆心的距离大于半径 = 在圆外 = 黑
                    let dy = (bar_h as f32 + r) - yc;
                    let dist = (dx * dx + dy * dy).sqrt();
                    (dist - r + 0.5).clamp(0.0, 1.0)
                }
            };
            if coverage <= 0.0 {
                continue;
            }
            let px = img.get_pixel_mut(x, y);
            // 向黑色混合；alpha 保持原样，带透明的 PNG 壁纸不会被抠成实心
            for channel in 0..3 {
                px.0[channel] = (px.0[channel] as f32 * (1.0 - coverage)) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无刘海屏必须被判成 None（默认开启对它们零影响）
    #[test]
    fn bar_height_and_radius_follow_backing_scale() {
        let g = NotchGeometry { height_pt: 38.0, backing_scale: 2.0 };
        assert_eq!(g.height_px(), 76);
        assert_eq!(g.radius_px(10.0), 20);
    }

    /// 黑条只覆盖顶部指定行，且中间列是纯黑
    #[test]
    fn draw_top_bar_only_touches_top_rows() {
        let mut img = image::RgbaImage::from_pixel(20, 10, image::Rgba([200, 200, 200, 255]));
        draw_top_bar(&mut img, 4, 0);
        assert_eq!(img.get_pixel(10, 0).0[0], 0, "顶部应为纯黑");
        assert_eq!(img.get_pixel(10, 3).0[0], 0, "黑条最后一行仍为黑");
        assert_eq!(img.get_pixel(10, 4).0[0], 200, "黑条之外不受影响");
    }

    /// 圆角是外翻的：黑条在两侧向下延伸，圆弧把壁纸顶角切成圆角
    #[test]
    fn rounded_corners_flare_downward() {
        let mut img = image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 255, 255, 255]));
        draw_top_bar(&mut img, 10, 8);

        // 屏幕左上角必为黑（黑条本体）
        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        // 黑条下沿之外、贴近屏幕左边缘处仍应是黑：这就是「外翻」
        assert_eq!(img.get_pixel(0, 10).0[0], 0, "外翻区应保留黑色");
        // 圆弧内侧要露出壁纸：壁纸顶角因此呈圆角
        assert_eq!(img.get_pixel(7, 11).0[0], 255, "圆弧内侧应恢复壁纸");
        // 中段黑条以下完全是壁纸
        assert_eq!(img.get_pixel(20, 10).0[0], 255);
        // 圆角区之外（更低的行）不受影响
        assert_eq!(img.get_pixel(0, 19).0[0], 255);
    }

    /// Cover 语义：输出尺寸精确等于目标，且不改变宽高比
    #[test]
    fn cover_to_produces_exact_size() {
        let img = image::RgbaImage::from_pixel(600, 400, image::Rgba([1, 2, 3, 255]));
        let out = cover_to(img, 3420, 2224);
        assert_eq!(out.dimensions(), (3420, 2224));

        let exact = image::RgbaImage::from_pixel(3420, 2224, image::Rgba([1, 2, 3, 255]));
        assert_eq!(cover_to(exact, 3420, 2224).dimensions(), (3420, 2224));
    }
}
