//! 随机壁纸的确认窗口。使用非模态窗口，避免嵌套事件循环重入主状态锁。

use crate::{
    app_event::{AppEvent, RandomWallpaperChoice},
    settings_ui::make_label,
    util,
};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSImage, NSImageScaling, NSImageView, NSView,
    NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::{
    cell::Cell,
    path::PathBuf,
    sync::{mpsc::Sender, Arc},
};

pub struct PreviewTargetIvars {
    tx: Sender<AppEvent>,
    submitted: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = PreviewTargetIvars]
    pub struct PreviewTarget;

    impl PreviewTarget {
        #[unsafe(method(confirm:))]
        fn confirm(&self, _sender: &NSObject) { self.submit(RandomWallpaperChoice::SaveAndApply); }

        #[unsafe(method(saveOnly:))]
        fn save_only(&self, _sender: &NSObject) { self.submit(RandomWallpaperChoice::SaveOnly); }
    }

    unsafe impl NSObjectProtocol for PreviewTarget {}
    unsafe impl NSWindowDelegate for PreviewTarget {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            // 关闭按钮与取消走同一条清理通路；由事件处理统一关闭窗口。
            self.submit(RandomWallpaperChoice::Cancel);
            false
        }
    }
);

impl PreviewTarget {
    fn submit(&self, choice: RandomWallpaperChoice) {
        // 回调只发事件，防止双击和关闭通知重复触发保存。
        if !self.ivars().submitted.replace(true) {
            util::send_event(&self.ivars().tx, AppEvent::ConfirmRandomWallpaper(choice));
        }
    }
}

pub struct RandomPreview {
    window: Retained<NSWindow>,
    // AppKit 的 target/delegate 是弱引用，必须与窗口一起保活。
    _target: Retained<PreviewTarget>,
    pub temporary_dir: Arc<tempfile::TempDir>,
    pub path: PathBuf,
}

impl RandomPreview {
    /// 只有图片可预览时才提供确认入口，解码失败仍由临时目录负责清理。
    pub fn new(
        mtm: MainThreadMarker,
        tx: Sender<AppEvent>,
        temporary_dir: Arc<tempfile::TempDir>,
        path: PathBuf,
    ) -> anyhow::Result<Self> {
        let image = NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&path.to_string_lossy()),
        )
        .ok_or_else(|| anyhow::anyhow!("无法显示壁纸预览"))?;
        let target = PreviewTarget::alloc(mtm).set_ivars(PreviewTargetIvars {
            tx,
            submitted: Cell::new(false),
        });
        let target: Retained<PreviewTarget> = unsafe { msg_send![super(target), init] };
        let frame = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame(0.0, 0.0, 680.0, 500.0),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("随机壁纸预览"));
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        let root = NSView::new(mtm);
        window.setContentView(Some(&root));
        let title = make_label(mtm, "喜欢这张壁纸吗？");
        title.setFrame(frame(24.0, 455.0, 632.0, 24.0));
        root.addSubview(&title);
        let view = NSImageView::new(mtm);
        view.setFrame(frame(24.0, 100.0, 632.0, 345.0));
        view.setImage(Some(&image));
        view.setImageScaling(NSImageScaling::ScaleProportionallyDown);
        root.addSubview(&view);
        let hint = make_label(
            mtm,
            "仅保存到壁纸库，或保存并应用到当前显示器；关闭窗口则取消。",
        );
        hint.setFrame(frame(24.0, 68.0, 632.0, 20.0));
        root.addSubview(&hint);
        // 仅保留两种保存操作；取消交给窗口关闭按钮，Return 沿用保存并应用。
        for (text, x, width, action, key) in [
            ("仅保存", 408.0, 88.0, sel!(saveOnly:), ""),
            ("保存并应用", 508.0, 148.0, sel!(confirm:), "\r"),
        ] {
            let button = NSButton::new(mtm);
            button.setFrame(frame(x, 20.0, width, 32.0));
            button.setTitle(&NSString::from_str(text));
            button.setKeyEquivalent(&NSString::from_str(key));
            unsafe {
                let _: () = msg_send![&*button, setBezelStyle: 1usize];
                button.setTarget(Some(&target));
                button.setAction(Some(action));
            }
            root.addSubview(&button);
        }
        window.center();
        Ok(Self {
            window,
            _target: target,
            temporary_dir,
            path,
        })
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        self.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
}

impl Drop for RandomPreview {
    fn drop(&mut self) {
        // 先断开弱代理，再关闭窗口，避免释放过程中再投递取消事件。
        self.window.setDelegate(None);
        self.window.close();
    }
}
