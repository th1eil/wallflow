//! 设置面板的视觉层：侧边导航、开放式内容页、滚动容器与弹性表单网格。
//!
//! 为什么单独成文件：`settings.rs` 已经同时承担控件指针、事件通路、配置读写
//! 三层职责，再把布局样式塞进去，「改外观」与「改行为」的 diff 会混在一起，
//! 评审时无法快速判断改动是否触及配置写盘链路。
//!
//! 本模块的边界：
//! - 只提供容器与排版原语，**不持有任何控件指针**，也不发送事件；
//! - 不读取 `Config`，不写盘；
//! - 所有函数都在主线程调用（`MainThreadMarker` 作为准入凭证）。

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyClass;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly};
use objc2_app_kit::{
    NSButton, NSColor, NSImage, NSImageView, NSLayoutAttribute, NSScrollView,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString};

/// 标签列给长字段留足宽度，所有控件从同一条竖线开始。
pub const LABEL_W: f64 = 108.0;

/// `NSTextAlignment` 的取值随 ABI 变化：arm64 上 Right = 2，x86_64 上 Right = 1。
/// 生成绑定里 `NSTextAlignment` 只定义了 Left/Justified/Natural，不能按它的
/// 定义推断数值（详见 `clock.rs` 顶部同名常量）。这里独立定义一份，避免为了
/// 一个常量把 clock 的内部细节暴露成 pub。
pub const TEXT_ALIGN_RIGHT: i64 = if cfg!(target_arch = "x86_64") { 1 } else { 2 };

/// 同上，Center 在 arm64 上是 1，在 x86_64 上是 2。
pub const TEXT_ALIGN_CENTER: i64 = if cfg!(target_arch = "x86_64") { 2 } else { 1 };

/// 字号 + 字重。用 `AnyClass` 取 NSFont，避免为两种字重再开一个 feature。
/// weight 采用 NSFontWeight：0.0 regular / 0.23 medium / 0.3 semibold。
fn apply_font(label: &NSTextField, size: f64, weight: f64) {
    let Some(font_cls) = AnyClass::get(c"NSFont") else { return };
    unsafe {
        let font: Option<Retained<NSObject>> =
            msg_send![font_cls, systemFontOfSize: size, weight: weight];
        if let Some(font) = font {
            let _: () = msg_send![label, setFont: Some(&*font)];
        }
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

/// 四边内边距约束。上下与左右分开取值，卡片和滚动页的呼吸感不同。
fn inset(inner: &NSView, outer: &NSView, h: f64, v: f64) {
    unsafe {
        inner.setTranslatesAutoresizingMaskIntoConstraints(false);
        let edges: [(*mut NSObject, *mut NSObject, f64); 4] = [
            (msg_send![inner, topAnchor], msg_send![outer, topAnchor], v),
            (msg_send![inner, bottomAnchor], msg_send![outer, bottomAnchor], -v),
            (msg_send![inner, leadingAnchor], msg_send![outer, leadingAnchor], h),
            (msg_send![inner, trailingAnchor], msg_send![outer, trailingAnchor], -h),
        ];
        for (a, b, constant) in edges {
            let c: *mut NSObject = msg_send![a, constraintEqualToAnchor: b, constant: constant];
            let _: () = msg_send![c, setActive: true];
        }
    }
}

/// 把子视图加进垂直 stack 并钉成整行宽。容器宽度由外层决定，行只负责撑满。
pub fn fill_row(stack: &NSStackView, view: &NSView) {
    unsafe {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        stack.addArrangedSubview(view);
        let va: *mut NSObject = msg_send![view, widthAnchor];
        let sa: *mut NSObject = msg_send![stack, widthAnchor];
        let c: *mut NSObject = msg_send![va, constraintEqualToAnchor: sa];
        let _: () = msg_send![c, setActive: true];
    }
}

/// 统一表单行：固定宽度的右对齐标签 + 随窗口伸缩的主要控件。
///
/// 标签右对齐是关键：左对齐时长标签会把控件起点推得参差不齐，整个面板看起来
/// 像随手拼的表单；右对齐后所有控件左边缘落在同一条竖线上。
pub fn form_row(
    mtm: MainThreadMarker,
    label_text: &str,
    items: &[(&NSView, Option<f64>)],
) -> Retained<NSStackView> {
    let r = NSStackView::new(mtm);
    r.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    r.setAlignment(NSLayoutAttribute::CenterY);
    r.setSpacing(12.0);

    let label = make_label(mtm, label_text);
    apply_font(&label, 12.5, 0.0);
    unsafe {
        let _: () = msg_send![&*label, setAlignment: TEXT_ALIGN_RIGHT];
        let _: () = msg_send![&*label, setTextColor: &*NSColor::secondaryLabelColor()];
    }
    attach(&r, &label, Some(LABEL_W));

    let mut has_flexible_control = false;
    for (control, width) in items {
        // 旧调用点把 330/350/450pt 当作桌面宽度；这些值现在表示“主输入区”，
        // 不再生成定宽约束。这样路径、URL 和滑块会跟随右侧内容区收放。
        if width.is_some_and(|w| w >= 300.0) {
            control.setTranslatesAutoresizingMaskIntoConstraints(false);
            control.widthAnchor().constraintGreaterThanOrEqualToConstant(110.0).setActive(true);
            unsafe {
                let _: () = msg_send![*control, setContentHuggingPriority: 1.0f32, forOrientation: 0i64];
            }
            r.addArrangedSubview(*control);
            has_flexible_control = true;
        } else {
            attach(&r, control, *width);
        }
    }
    // 下拉框和复选框保持自然尺寸；其余空白交给 spacer，避免控件被拉得过宽。
    if !has_flexible_control {
        let spacer = NSView::new(mtm);
        unsafe {
            spacer.setTranslatesAutoresizingMaskIntoConstraints(false);
            let _: () = msg_send![&*spacer, setContentHuggingPriority: 1.0f32, forOrientation: 0i64];
        }
        r.addArrangedSubview(&spacer);
    }
    r
}

fn attach(stack: &NSStackView, view: &NSView, width: Option<f64>) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    if let Some(w) = width {
        view.widthAnchor().constraintEqualToConstant(w).setActive(true);
    }
    stack.addArrangedSubview(view);
}

/// 内容区保持开放，不再给每个分类重复套圆角卡片。
/// 包装视图仅用于分类切换时整体隐藏，真正的边距由滚动页控制。
fn wrap_section(mtm: MainThreadMarker, inner: &NSStackView) -> Retained<NSView> {
    let section = NSView::new(mtm);
    section.addSubview(inner);
    inset(inner, &section, 0.0, 0.0);
    section
}

/// 增量式分区容器：调用方按原有顺序 `add` 行，遇到新分区时调 `section`，
/// 上一个分区在切换点自动收口成可隐藏的内容页。
///
/// 这样做的原因是 `SettingsPanel::new` 里控件创建与挂载原本是交错的，
/// 如果为了分组把所有控件挂载逻辑整体重排，会连带改到事件绑定与指针赋值，
/// 把一次纯视觉改版的回归面扩大到配置写盘链路。
pub struct SectionColumn {
    page: Retained<NSStackView>,
    /// (标题, 说明, 符号名, 分区内容栈)
    current: Option<(String, String, String, Retained<NSStackView>)>,
    cards: Vec<Retained<NSView>>,
}

impl SectionColumn {
    pub fn new(page: Retained<NSStackView>) -> Self {
        Self { page, current: None, cards: Vec::new() }
    }

    /// 开始一个新分区；上一个分区在这里落成内容页。
    pub fn section(&mut self, mtm: MainThreadMarker, title: &str, subtitle: &str, symbol: &str) {
        self.flush(mtm);
        let inner = NSStackView::new(mtm);
        inner.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        inner.setAlignment(NSLayoutAttribute::Leading);
        inner.setSpacing(12.0);
        // 校验提示的整行隐藏时立即折叠，避免在正常状态留下无意义空白。
        inner.setDetachesHiddenViews(true);
        self.current = Some((title.to_string(), subtitle.to_string(), symbol.to_string(), inner));
    }

    /// 把一行挂到当前分区。分区尚未开始时静默忽略——调用方漏写 section
    /// 只会少一页内容，不会 panic 打断整个面板构建。
    pub fn add(&mut self, view: &NSView) {
        if let Some((_, _, _, inner)) = &self.current {
            fill_row(inner, view);
        }
    }

    /// 收口当前分区并追加到滚动容器。必须在构建结束后手动调用一次。
    pub fn finish(mut self, mtm: MainThreadMarker) -> Vec<Retained<NSView>> {
        self.flush(mtm);
        self.cards
    }

    fn flush(&mut self, mtm: MainThreadMarker) {
        let Some((title, subtitle, symbol, inner)) = self.current.take() else { return };
        let header = section_header(mtm, &title, &subtitle, &symbol);
        // 标题插到内容栈最前面：arrangedSubviews 的顺序决定视觉顺序
        inner.insertArrangedSubview_atIndex(&header, 0);
        // 标题和表单之间多留一段空白，靠排版建立层级而非卡片描边。
        let breathing = NSView::new(mtm);
        breathing.setTranslatesAutoresizingMaskIntoConstraints(false);
        breathing.heightAnchor().constraintEqualToConstant(14.0).setActive(true);
        inner.insertArrangedSubview_atIndex(&breathing, 1);
        let card = wrap_section(mtm, &inner);
        fill_row(&self.page, &card);
        self.cards.push(card);
    }
}

/// 分区标题：以较大的标题承担内容区的视觉重心，图标留在侧边导航。
fn section_header(mtm: MainThreadMarker, title: &str, subtitle: &str, symbol: &str) -> Retained<NSStackView> {
    let _ = symbol;
    let text = NSStackView::new(mtm);
    text.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    text.setAlignment(NSLayoutAttribute::Leading);
    text.setSpacing(6.0);
    let label = make_label(mtm, title);
    apply_font(&label, 28.0, 0.3);
    text.addArrangedSubview(&label);
    let detail = make_label(mtm, subtitle);
    apply_font(&detail, 13.0, 0.0);
    unsafe {
        let _: () = msg_send![&*detail, setTextColor: &*NSColor::secondaryLabelColor()];
    }
    text.addArrangedSubview(&detail);
    text
}

/// 固定侧栏把产品识别和分类导航放在一个区域。选中态用系统 Selection 材质，
/// 颜色会随 macOS 外观变化；按钮本身保持无边框，不再出现一排同质化胶囊。
pub fn navigation_sidebar(
    mtm: MainThreadMarker,
) -> (Retained<NSVisualEffectView>, [Retained<NSButton>; 5], [Retained<NSVisualEffectView>; 5]) {
    let sidebar = NSVisualEffectView::new(mtm);
    sidebar.setMaterial(NSVisualEffectMaterial::Sidebar);
    sidebar.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    sidebar.setState(NSVisualEffectState::Active);

    let content = NSStackView::new(mtm);
    content.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    content.setAlignment(NSLayoutAttribute::Leading);
    content.setSpacing(5.0);

    // 品牌只出现一次，分类页只负责说明当前设置。图标是壁纸堆叠的视觉线索。
    let brand = NSStackView::new(mtm);
    brand.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    brand.setAlignment(NSLayoutAttribute::CenterY);
    brand.setSpacing(9.0);
    let brand_icon = NSImageView::new(mtm);
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("square.stack.3d.up"), None,
    ) {
        brand_icon.setImage(Some(&*image));
        brand_icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
    }
    brand_icon.widthAnchor().constraintEqualToConstant(22.0).setActive(true);
    brand_icon.heightAnchor().constraintEqualToConstant(22.0).setActive(true);
    brand.addArrangedSubview(&brand_icon);
    let brand_name = make_label(mtm, "WallFlow");
    apply_font(&brand_name, 20.0, 0.3);
    brand.addArrangedSubview(&brand_name);
    fill_row(&content, &brand);

    let description = make_label(mtm, "让桌面保持流动");
    apply_font(&description, 11.0, 0.0);
    unsafe {
        let _: () = msg_send![&*description, setTextColor: &*NSColor::secondaryLabelColor()];
    }
    fill_row(&content, &description);
    let gap = NSView::new(mtm);
    gap.heightAnchor().constraintEqualToConstant(28.0).setActive(true);
    fill_row(&content, &gap);

    let mut selections = Vec::new();
    let buttons: [Retained<NSButton>; 5] = std::array::from_fn(|index| {
        let item = NSView::new(mtm);
        item.heightAnchor().constraintEqualToConstant(39.0).setActive(true);
        let selected = NSVisualEffectView::new(mtm);
        selected.setMaterial(NSVisualEffectMaterial::Selection);
        selected.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        selected.setState(NSVisualEffectState::Active);
        selected.setHidden(true);
        unsafe {
            selected.setWantsLayer(true);
            let layer: *mut NSObject = msg_send![&*selected, layer];
            let _: () = msg_send![layer, setCornerRadius: 8.0f64];
            item.addSubview(&selected);
        }
        inset(&selected, &item, 0.0, 0.0);

        let button = NSButton::new(mtm);
        // 索引必须与 settings.rs 里 `column.finish()` 产出的卡片顺序严格一致，
        // 否则导航点到的是另一张卡片（数量不一致会直接 panic，见 try_into）。
        let title = ["壁纸库", "图像滤镜", "桌面时钟", "外观样式", "刘海屏"][index];
        let symbol = [
            "photo.on.rectangle",
            "camera.filters",
            "clock",
            "paintbrush.pointed",
            "menubar.rectangle",
        ][index];
        unsafe {
            let _: () = msg_send![&*button, setTitle: &*NSString::from_str(title)];
            let _: () = msg_send![&*button, setBordered: false];
            let _: () = msg_send![&*button, setAlignment: 0i64];
            let _: () = msg_send![&*button, setImagePosition: 2u64];
            let _: () = msg_send![&*button, setTag: index as isize];
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol), None,
            ) {
                let _: () = msg_send![&*button, setImage: Some(&*image)];
            }
            item.addSubview(&button);
        }
        inset(&button, &item, 11.0, 0.0);
        fill_row(&content, &item);
        selections.push(selected);
        button
    });

    sidebar.addSubview(&content);
    content.setTranslatesAutoresizingMaskIntoConstraints(false);
    content.leadingAnchor().constraintEqualToAnchor_constant(&sidebar.leadingAnchor(), 14.0).setActive(true);
    content.trailingAnchor().constraintEqualToAnchor_constant(&sidebar.trailingAnchor(), -14.0).setActive(true);
    content.topAnchor().constraintEqualToAnchor_constant(&sidebar.topAnchor(), 28.0).setActive(true);

    (sidebar, buttons, selections.try_into().ok().expect("五个侧栏选中背景"))
}

/// 滚动容器。返回 (scrollView, 页面 stack)：调用方把内容行 `fill_row` 进页面
/// stack 即可，高度由内容自然撑开。
///
/// 文档视图必须是 flipped 的：非 flipped 视图的原点在左下角，NSScrollView 初始
/// 会把可视区停在内容**底部**，用户打开面板第一眼看到的是最后一组设置。
pub fn scroll_page(mtm: MainThreadMarker) -> (Retained<NSScrollView>, Retained<NSStackView>) {
    let scroll = NSScrollView::new(mtm);
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(false);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    unsafe {
        // NSBorderType::NoBorder == 0，省一个 feature
        let _: () = msg_send![&*scroll, setBorderType: 0usize];
    }

    let doc = FlippedView::new(mtm);
    let page = NSStackView::new(mtm);
    page.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    page.setAlignment(NSLayoutAttribute::Leading);
    page.setSpacing(0.0);
    // 分类切换时让隐藏内容页退出布局，高度会立即缩到当前页。
    page.setDetachesHiddenViews(true);

    unsafe {
        doc.addSubview(&page);
        page.setTranslatesAutoresizingMaskIntoConstraints(false);
        let _: () = msg_send![&*doc, setTranslatesAutoresizingMaskIntoConstraints: false];

        // page 只钉 top / leading / trailing，**不能钉 bottom**。
        // 四边都钉会形成「页高 = 文档高 = 页高」的循环，Auto Layout 无解，
        // 整条高度链退化到 0（实测 doc=0x0，所有卡片 0 高）。
        let pad_h = 36.0;
        let pad_v = 34.0;
        let page_edges: [(*mut NSObject, *mut NSObject, f64); 3] = [
            (msg_send![&*page, topAnchor], msg_send![&*doc, topAnchor], pad_v),
            (msg_send![&*page, leadingAnchor], msg_send![&*doc, leadingAnchor], pad_h),
            (msg_send![&*page, trailingAnchor], msg_send![&*doc, trailingAnchor], -pad_h),
        ];
        for (a, b, constant) in page_edges {
            let c: *mut NSObject = msg_send![a, constraintEqualToAnchor: b, constant: constant];
            let _: () = msg_send![c, setActive: true];
        }

        // 高度链的根：page 的高度来自 NSStackView 的 intrinsic（内容撑开），
        // 文档高度由它加内边距推出，滚动范围随之确定。
        let doc_h: *mut NSObject = msg_send![&*doc, heightAnchor];
        let page_h: *mut NSObject = msg_send![&*page, heightAnchor];
        let c: *mut NSObject =
            msg_send![doc_h, constraintEqualToAnchor: page_h, constant: pad_v * 2.0];
        let _: () = msg_send![c, setActive: true];
    }

    // 显式取 &NSView：Option<&FlippedView> 不会自动 coerce 成 Option<&NSView>
    let doc_view: &NSView = &doc;
    scroll.setDocumentView(Some(doc_view));

    // 文档视图宽度与位置跟随可视区：否则垂直滚动条出现时内容会被横向裁掉
    unsafe {
        let clip: *mut NSObject = msg_send![&*scroll, contentView];
        let pairs: [(*mut NSObject, *mut NSObject); 3] = [
            (msg_send![&*doc, widthAnchor], msg_send![clip, widthAnchor]),
            (msg_send![&*doc, topAnchor], msg_send![clip, topAnchor]),
            (msg_send![&*doc, leadingAnchor], msg_send![clip, leadingAnchor]),
        ];
        for (a, b) in pairs {
            let c: *mut NSObject = msg_send![a, constraintEqualToAnchor: b];
            let _: () = msg_send![c, setActive: true];
        }
    }

    (scroll, page)
}

/// 底部操作栏保持固定。说明文案放左侧、操作按钮放右侧，应用按钮始终
/// 可见，长设置页也不必滚动到底部才能保存。
pub fn footer(mtm: MainThreadMarker, buttons: &[&NSView]) -> Retained<NSVisualEffectView> {
    let bar = NSVisualEffectView::new(mtm);
    bar.setMaterial(NSVisualEffectMaterial::HeaderView);
    bar.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    bar.setState(NSVisualEffectState::Active);

    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::CenterY);
    row.setSpacing(10.0);
    let hint = make_label(mtm, "更改将在应用后保存");
    apply_font(&hint, 11.0, 0.0);
    unsafe {
        let _: () = msg_send![&*hint, setTextColor: &*NSColor::secondaryLabelColor()];
    }
    row.addArrangedSubview(&hint);
    let spacer = NSView::new(mtm);
    unsafe {
        let _: () = msg_send![&*spacer, setContentHuggingPriority: 1.0f32, forOrientation: 0i64];
    }
    row.addArrangedSubview(&spacer);
    for b in buttons {
        row.addArrangedSubview(b);
    }

    unsafe {
        bar.addSubview(&row);
        let _: () = msg_send![&*row, setTranslatesAutoresizingMaskIntoConstraints: false];
        let edges: [(*mut NSObject, *mut NSObject, f64); 3] = [
            (msg_send![&*row, trailingAnchor], msg_send![&*bar, trailingAnchor], -28.0),
            (msg_send![&*row, leadingAnchor], msg_send![&*bar, leadingAnchor], 28.0),
            (msg_send![&*row, centerYAnchor], msg_send![&*bar, centerYAnchor], 0.0),
        ];
        for (a, b, constant) in edges {
            let c: *mut NSObject = msg_send![a, constraintEqualToAnchor: b, constant: constant];
            let _: () = msg_send![c, setActive: true];
        }
        let h: *mut NSObject = msg_send![&*row, heightAnchor];
        let c: *mut NSObject = msg_send![h, constraintEqualToConstant: 30.0f64];
        let _: () = msg_send![c, setActive: true];
    }

    bar
}

// ---------------------------------------------------------------------------
// flipped 文档视图
// ---------------------------------------------------------------------------

define_class!(
    /// NSScrollView 的文档视图。AppKit 判断滚动坐标系靠 `isFlipped`，
    /// 重写它比在每次 `show()` 里手动 scrollToPoint 更可靠（窗口 resize、
    /// 内容高度变化时都不需要补偿）。
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(_mtm: MainThreadMarker) -> Retained<Self> {
        let alloc: Allocated<Self> = unsafe { msg_send![Self::class(), alloc] };
        let this = alloc.set_ivars(());
        unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(0.0, 0.0),
            )]
        }
    }
}
