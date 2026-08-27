use std::cell::RefCell;
use std::sync::mpsc::Sender;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSImage};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use crate::app_event::AppEvent;
use crate::app_event::WallpaperInfo;
use crate::playback::LoopMode;

pub struct MenuTargetIvars {
    sender: std::sync::Mutex<Option<Sender<AppEvent>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuTargetIvars]
    pub struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method_id(init))]
        fn init(this: Allocated<Self>) -> Retained<Self> {
            let this = this.set_ivars(MenuTargetIvars {
                sender: std::sync::Mutex::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        #[unsafe(method(previousClicked:))]
        unsafe fn previous_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::Previous);
            }
        }

        #[unsafe(method(nextClicked:))]
        unsafe fn next_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::Next);
            }
        }

        #[unsafe(method(randomClicked:))]
        unsafe fn random_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::Random);
            }
        }

        #[unsafe(method(settingsClicked:))]
        unsafe fn settings_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::Settings);
            }
        }

        #[unsafe(method(exitClicked:))]
        unsafe fn exit_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::Exit);
            }
        }

        #[unsafe(method(chooseDirectoryClicked:))]
        unsafe fn choose_directory_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::ChooseDirectory);
            }
        }

        /// Open the current wallpaper directory in Finder.
        #[unsafe(method(openFolderClicked:))]
        unsafe fn open_folder_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::OpenWallpaperFolder);
            }
        }

        /// Download a random wallpaper from online source (picsum.photos).
        #[unsafe(method(downloadRandomClicked:))]
        unsafe fn download_random_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(sender, AppEvent::DownloadRandomWallpaper);
            }
        }

        /// Single handler for all filter preset menu items.
        /// Reads the sending NSMenuItem's representedObject (NSString tag)
        /// to determine which preset the user selected.
        #[unsafe(method(presetClicked:))]
        unsafe fn preset_clicked(&self, menu_item: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let tag: objc2::rc::Retained<NSString> = msg_send![menu_item, representedObject];
                let tag_str = tag.to_string();
                if let Some((_, preset)) = crate::color::FilterPreset::PRESETS
                    .iter()
                    .find(|(t, _)| *t == tag_str.as_str())
                {
                    crate::util::send_event(tx, AppEvent::SetFilterPreset(*preset));
                }
            }
        }

        /// Screen selection handler: reads tag from the NSMenuItem to set active screen.
        #[unsafe(method(screenSelected:))]
        unsafe fn screen_selected(&self, menu_item: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let tag: isize = msg_send![menu_item, tag];
                crate::util::send_event(tx, AppEvent::SelectActiveScreen(tag as u32));
            }
        }

        /// Toggle pause/resume for the active screen's dynamic player.
        #[unsafe(method(togglePauseClicked:))]
        unsafe fn toggle_pause_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(tx, AppEvent::TogglePause);
            }
        }

        /// Set loop mode to Infinite.
        #[unsafe(method(loopInfiniteClicked:))]
        unsafe fn loop_infinite_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(tx, AppEvent::SetLoopMode(crate::playback::LoopMode::Infinite));
            }
        }

        /// Set loop mode to PlayOnce.
        #[unsafe(method(loopOnceClicked:))]
        unsafe fn loop_once_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(tx, AppEvent::SetLoopMode(crate::playback::LoopMode::PlayOnce));
            }
        }

        /// Set loop mode to PingPong.
        #[unsafe(method(loopPingPongClicked:))]
        unsafe fn loop_pingpong_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                crate::util::send_event(tx, AppEvent::SetLoopMode(crate::playback::LoopMode::PingPong));
            }
        }

        /// FPS selection: reads tag from NSMenuItem (tag = fps value).
        #[unsafe(method(fpsSelected:))]
        unsafe fn fps_selected(&self, menu_item: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let tag: isize = msg_send![menu_item, tag];
                crate::util::send_event(tx, AppEvent::SetFps(tag as u32));
            }
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    pub fn set_sender(&self, sender: Sender<AppEvent>) {
        *self.ivars().sender.lock().unwrap() = Some(sender);
    }
}

pub struct MenuBarManager {
    _status_item_bar: Retained<NSStatusItem>,
    _menu_target: Retained<MenuTarget>,
    menu: Retained<NSMenu>,
    /// Transient feedback line (下载中/失败原因…), hidden when empty.
    status_item: Retained<NSMenuItem>,
    thumbnail_item: Retained<NSMenuItem>,
    info_item: Retained<NSMenuItem>,
    pause_item: Retained<NSMenuItem>,
    ctrl_parent_item: Retained<NSMenuItem>,
    // Preset items keyed by their represented tag string
    preset_items: Vec<(&'static str, Retained<NSMenuItem>)>,
    loop_items: Vec<(LoopMode, Retained<NSMenuItem>)>,
    fps_items: Vec<(isize, Retained<NSMenuItem>)>,
    // Screen submenu lives longer than construction: rebuilt on display changes
    screens_parent_item: RefCell<Option<Retained<NSMenuItem>>>,
    screen_items: RefCell<Vec<Retained<NSMenuItem>>>,
}

impl MenuBarManager {
    pub fn new(mtm: MainThreadMarker, event_tx: Sender<AppEvent>) -> Self {
        let status_bar = NSStatusBar::systemStatusBar();
        // NSVariableStatusItemLength = -1.0
        let status_item = status_bar.statusItemWithLength(-1.0);

        let menu_target: Retained<MenuTarget> = unsafe { msg_send![MenuTarget::class(), new] };
        menu_target.set_sender(event_tx);

        // Setup status button
        unsafe {
            if let Some(button) = status_item.button(mtm) {
                // Try to load embedded icon
                let icon_bytes = include_bytes!("../assets/icon.png");

                let data: Retained<objc2_foundation::NSData> = msg_send![
                    objc2_foundation::NSData::class(),
                    dataWithBytes: icon_bytes.as_ptr() as *const std::ffi::c_void,
                    length: icon_bytes.len()
                ];

                let image: Option<Retained<NSImage>> = msg_send![
                    msg_send![NSImage::class(), alloc],
                    initWithData: &*data
                ];

                if let Some(ref img) = image {
                    img.setTemplate(true);
                    button.setImage(Some(img));
                } else {
                    button.setTitle(&NSString::from_str("WallFlow"));
                }
            }
        }

        // Create Menu
        let menu = NSMenu::new(mtm);
        // Disable automatic validation: items are enabled/disabled explicitly,
        // otherwise setEnabled(false) gets overridden for target/action items.
        unsafe {
            let _: () = msg_send![&*menu, setAutoenablesItems: false];
        }

        // Thumbnail Item
        let status_line = NSMenuItem::new(mtm);
        let thumbnail_item = NSMenuItem::new(mtm);
        let info_item = NSMenuItem::new(mtm);
        let mut preset_items: Vec<(&'static str, Retained<NSMenuItem>)> = Vec::new();
        let mut loop_items: Vec<(LoopMode, Retained<NSMenuItem>)> = Vec::new();
        let mut fps_items: Vec<(isize, Retained<NSMenuItem>)> = Vec::new();
        let pause_item: Retained<NSMenuItem>;
        let ctrl_parent_item: Retained<NSMenuItem>;
        unsafe {
            status_line.setTitle(&NSString::from_str(""));
            status_line.setEnabled(false);
            status_line.setHidden(true);
            menu.addItem(&status_line);

            thumbnail_item.setTitle(&NSString::from_str("暂无壁纸"));
            thumbnail_item.setEnabled(false);
            menu.addItem(&thumbnail_item);

            // Info line below thumbnail: resolution, format, file size
            info_item.setTitle(&NSString::from_str(""));
            info_item.setEnabled(false);
            menu.addItem(&info_item);

            // Separator
            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Previous
            let prev_item = NSMenuItem::new(mtm);
            prev_item.setTitle(&NSString::from_str("上一张"));
            prev_item.setTarget(Some(&*menu_target));
            prev_item.setAction(Some(objc2::sel!(previousClicked:)));
            prev_item.setEnabled(true);
            menu.addItem(&prev_item);

            // Next
            let next_item = NSMenuItem::new(mtm);
            next_item.setTitle(&NSString::from_str("下一张"));
            next_item.setTarget(Some(&*menu_target));
            next_item.setAction(Some(objc2::sel!(nextClicked:)));
            next_item.setEnabled(true);
            menu.addItem(&next_item);

            // Random
            let random_item = NSMenuItem::new(mtm);
            random_item.setTitle(&NSString::from_str("随机切换"));
            random_item.setTarget(Some(&*menu_target));
            random_item.setAction(Some(objc2::sel!(randomClicked:)));
            random_item.setEnabled(true);
            menu.addItem(&random_item);

            // 显示器 submenu placeholder — populated by rebuild_screen_menu()
            // (also called here right after construction below).

            // --- 滤镜预设 submenu ---
            let filter_menu = NSMenu::new(mtm);
            let preset_names: [(&str, &str); 10] = [
                ("原图",   "original"),
                ("复古",   "vintage"),
                ("胶片",   "film"),
                ("黑白",   "bw"),
                ("低饱和", "muted"),
                ("日系",   "japanese"),
                ("赛博朋克", "cyberpunk"),
                ("暗黑",   "dark"),
                ("暖阳",   "sunshine"),
                ("森林",   "forest"),
            ];
            for (title, tag) in &preset_names {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(title));
                item.setTarget(Some(&*menu_target));
                item.setAction(Some(objc2::sel!(presetClicked:)));
                // Store the preset tag as a string so presetClicked: can read it
                item.setRepresentedObject(Some(&*NSString::from_str(tag)));
                item.setEnabled(true);
                filter_menu.addItem(&item);
                if let Some(static_tag) =
                    crate::color::FilterPreset::PRESETS.iter().find(|(_, p)| p.tag() == *tag)
                {
                    preset_items.push((static_tag.0, item));
                }
            }

            let filter_preset_item = NSMenuItem::new(mtm);
            filter_preset_item.setTitle(&NSString::from_str("滤镜预设"));
            filter_preset_item.setSubmenu(Some(&filter_menu));
            filter_preset_item.setEnabled(true);
            menu.addItem(&filter_preset_item);

            // --- 播放控制 submenu ---
            {
                let ctrl_menu = NSMenu::new(mtm);

                // Pause/Resume
                let pause = NSMenuItem::new(mtm);
                pause.setTitle(&NSString::from_str("暂停"));
                pause.setTarget(Some(&*menu_target));
                pause.setAction(Some(objc2::sel!(togglePauseClicked:)));
                pause.setEnabled(true);
                ctrl_menu.addItem(&pause);

                ctrl_menu.addItem(&NSMenuItem::separatorItem(mtm));

                // Loop mode submenu
                let loop_menu = NSMenu::new(mtm);
                for (title, mode, sel) in [
                    ("无限循环", LoopMode::Infinite, objc2::sel!(loopInfiniteClicked:)),
                    ("播放一次", LoopMode::PlayOnce, objc2::sel!(loopOnceClicked:)),
                    ("乒乓循环", LoopMode::PingPong, objc2::sel!(loopPingPongClicked:)),
                ] {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(title));
                    item.setTarget(Some(&*menu_target));
                    item.setAction(Some(sel));
                    item.setEnabled(true);
                    loop_menu.addItem(&item);
                    loop_items.push((mode, item));
                }
                let loop_item = NSMenuItem::new(mtm);
                loop_item.setTitle(&NSString::from_str("循环模式"));
                loop_item.setSubmenu(Some(&loop_menu));
                loop_item.setEnabled(true);
                ctrl_menu.addItem(&loop_item);

                // FPS submenu
                let fps_menu = NSMenu::new(mtm);
                let fps_values: [(isize, String); 8] = [
                    (0, "原始".to_string()), (1, "1 帧/秒".to_string()), (5, "5 帧/秒".to_string()),
                    (10, "10 帧/秒".to_string()), (15, "15 帧/秒".to_string()), (24, "24 帧/秒".to_string()),
                    (30, "30 帧/秒".to_string()), (60, "60 帧/秒".to_string()),
                ];
                for (fps, label) in &fps_values {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(label));
                    item.setTarget(Some(&*menu_target));
                    item.setAction(Some(objc2::sel!(fpsSelected:)));
                    let _: () = msg_send![&*item, setTag: *fps];
                    item.setEnabled(true);
                    fps_menu.addItem(&item);
                    fps_items.push((*fps, item));
                }
                let fps_item = NSMenuItem::new(mtm);
                fps_item.setTitle(&NSString::from_str("帧率"));
                fps_item.setSubmenu(Some(&fps_menu));
                fps_item.setEnabled(true);
                ctrl_menu.addItem(&fps_item);

                let ctrl_parent = NSMenuItem::new(mtm);
                ctrl_parent.setTitle(&NSString::from_str("播放控制"));
                ctrl_parent.setSubmenu(Some(&ctrl_menu));
                // Playback controls only matter while a GIF/APNG/video plays;
                // AppState enables them once such content is applied.
                ctrl_parent.setEnabled(false);
                menu.addItem(&ctrl_parent);

                pause_item = pause;
                ctrl_parent_item = ctrl_parent;
            }

            // Separator
            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Choose Directory
            let choose_item = NSMenuItem::new(mtm);
            choose_item.setTitle(&NSString::from_str("选择壁纸文件夹..."));
            choose_item.setTarget(Some(&*menu_target));
            choose_item.setAction(Some(objc2::sel!(chooseDirectoryClicked:)));
            choose_item.setEnabled(true);
            menu.addItem(&choose_item);

            // Download Random Wallpaper
            let download_item = NSMenuItem::new(mtm);
            download_item.setTitle(&NSString::from_str("获取随机壁纸..."));
            download_item.setTarget(Some(&*menu_target));
            download_item.setAction(Some(objc2::sel!(downloadRandomClicked:)));
            download_item.setEnabled(true);
            menu.addItem(&download_item);

            // Open Wallpaper Folder
            let open_item = NSMenuItem::new(mtm);
            open_item.setTitle(&NSString::from_str("打开壁纸文件夹"));
            open_item.setTarget(Some(&*menu_target));
            open_item.setAction(Some(objc2::sel!(openFolderClicked:)));
            open_item.setEnabled(true);
            menu.addItem(&open_item);

            // Settings
            let settings_item = NSMenuItem::new(mtm);
            settings_item.setTitle(&NSString::from_str("设置"));
            settings_item.setTarget(Some(&*menu_target));
            settings_item.setAction(Some(objc2::sel!(settingsClicked:)));
            settings_item.setEnabled(true);
            menu.addItem(&settings_item);

            // Exit
            let exit_item = NSMenuItem::new(mtm);
            exit_item.setTitle(&NSString::from_str("退出"));
            exit_item.setTarget(Some(&*menu_target));
            exit_item.setAction(Some(objc2::sel!(exitClicked:)));
            exit_item.setEnabled(true);
            menu.addItem(&exit_item);

            status_item.setMenu(Some(&menu));
        }

        // 显示器 submenu 初始内容在 AppState 初始化时通过
        // rebuild_screen_menu() 填充（main.rs 启动块）。
        Self {
            _status_item_bar: status_item,
            _menu_target: menu_target,
            menu,
            status_item: status_line,
            thumbnail_item,
            info_item,
            pause_item,
            ctrl_parent_item,
            preset_items,
            loop_items,
            fps_items,
            screens_parent_item: RefCell::new(None),
            screen_items: RefCell::new(Vec::new()),
        }
    }

    pub fn update_thumbnail(&self, title: &str, thumbnail_path: Option<&std::path::Path>, info: Option<&WallpaperInfo>) {
        unsafe {
            let title_ns = NSString::from_str(title);
            self.thumbnail_item.setTitle(&title_ns);

            // Update info line (resolution, format, file size)
            if let Some(i) = info {
                let line = i.display_line();
                self.info_item.setTitle(&NSString::from_str(&line));
                self.info_item.setEnabled(true);
            } else {
                self.info_item.setTitle(&NSString::from_str(""));
                self.info_item.setEnabled(false);
            }

            if let Some(path) = thumbnail_path {
                if let Ok(bytes) = std::fs::read(path) {
                    let data: Retained<objc2_foundation::NSData> = msg_send![
                        objc2_foundation::NSData::class(),
                        dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void,
                        length: bytes.len()
                    ];

                    let image: Option<Retained<NSImage>> = msg_send![
                        msg_send![NSImage::class(), alloc],
                        initWithData: &*data
                    ];

                    if let Some(ref img) = image {
                        // Aspect-fit inside the 128×80 box so non-16:10 images
                        // aren't squashed. Falls back to the fixed box when the
                        // dimensions can't be read cheaply from the header.
                        let size = image::image_dimensions(path).ok()
                            .map(|(w, h)| {
                                let w = w.max(1) as f64;
                                let h = h.max(1) as f64;
                                let scale = (128.0 / w).min(80.0 / h);
                                objc2_foundation::NSSize::new(
                                    (w * scale).round().max(1.0),
                                    (h * scale).round().max(1.0),
                                )
                            })
                            .unwrap_or_else(|| objc2_foundation::NSSize::new(128.0, 80.0));
                        img.setSize(size);
                        self.thumbnail_item.setImage(Some(img));
                    } else {
                        self.thumbnail_item.setImage(None);
                    }
                } else {
                    self.thumbnail_item.setImage(None);
                }
            } else {
                self.thumbnail_item.setImage(None);
            }
        }
    }

    /// Show or clear the transient status line above the thumbnail.
    /// The row is hidden while empty so the menu stays compact.
    pub fn show_status(&self, text: Option<&str>) {
        let t = text.unwrap_or("");
        self.status_item.setTitle(&NSString::from_str(t));
        self.status_item.setHidden(t.is_empty());
    }

    /// Update checkmark state for screen selection menu items.
    pub fn set_active_screen(&self, screen_number: u32) {
        for item in self.screen_items.borrow().iter() {
            unsafe {
                let tag: isize = msg_send![&**item, tag];
                let state: isize = if tag as u32 == screen_number { 1 } else { 0 };
                let _: () = msg_send![&**item, setState: state];
            }
        }
    }

    /// Rebuild the 显示器 submenu from the current display list.
    /// Called at construction and again whenever macOS reports that the set of
    /// connected screens changed (hot-plug, resolution change…).
    /// entries: (screen number, display label) pairs.
    pub fn rebuild_screen_menu(&self, mtm: MainThreadMarker, entries: &[(u32, String)], active_number: u32) {
        // Drop the previous submenu entirely
        if let Some(old_parent) = self.screens_parent_item.borrow_mut().take() {
            unsafe {
                let idx: isize = msg_send![&*self.menu, indexOfItem: &*old_parent];
                if idx >= 0 {
                    let _: () = msg_send![&*self.menu, removeItemAtIndex: idx];
                }
            }
        }
        self.screen_items.borrow_mut().clear();

        if entries.len() <= 1 {
            return; // single display needs no picker
        }

        unsafe {
            let screen_menu = NSMenu::new(mtm);
            let mut items = Vec::with_capacity(entries.len());
            for (i, (num, label)) in entries.iter().enumerate() {
                let title = format!("{} ({})", label, i + 1);
                let item = NSMenuItem::new(mtm);
                item.setTitle(&NSString::from_str(&title));
                item.setTarget(Some(&*self._menu_target));
                item.setAction(Some(objc2::sel!(screenSelected:)));
                // Store screen number in the tag (can't use representedObject for u32 easily)
                let _: () = msg_send![&*item, setTag: *num as isize];
                let _: () = msg_send![&*item, setState: (if *num == active_number { 1isize } else { 0isize })];
                item.setEnabled(true);
                screen_menu.addItem(&item);
                items.push(item);
            }
            let screen_menu_item = NSMenuItem::new(mtm);
            screen_menu_item.setTitle(&NSString::from_str("显示器"));
            screen_menu_item.setSubmenu(Some(&screen_menu));
            screen_menu_item.setEnabled(true);
            // Insert before 滤镜预设 (the anchor item that always exists),
            // keeping the original menu layout on every rebuild.
            let count: isize = msg_send![&*self.menu, numberOfItems];
            let mut insert_at = count;
            for i in 0..count {
                let it: *mut NSMenuItem = msg_send![&*self.menu, itemAtIndex: i];
                if !it.is_null() {
                    let title: Retained<NSString> = msg_send![it, title];
                    if title.to_string() == "滤镜预设" {
                        insert_at = i;
                        break;
                    }
                }
            }
            let _: () = msg_send![&*self.menu, insertItemAt: &*screen_menu_item, atIndex: insert_at];

            *self.screens_parent_item.borrow_mut() = Some(screen_menu_item);
            *self.screen_items.borrow_mut() = items;
        }
    }

    /// Reflect the active screen's pause state: 「暂停」 ↔ 「继续」.
    pub fn set_paused(&self, paused: bool) {
        let title = if paused { "继续" } else { "暂停" };
        self.pause_item.setTitle(&NSString::from_str(title));
    }

    /// Enable the whole 播放控制 subtree only while dynamic content shows.
    pub fn set_playback_enabled(&self, enabled: bool) {
        self.ctrl_parent_item.setEnabled(enabled);
    }

    /// Checkmark the currently active filter preset (None clears).
    pub fn set_preset_tag(&self, tag: Option<&'static str>) {
        for (item_tag, item) in &self.preset_items {
            // Tags all point into the same static PRESETS table, so pointer
            // equality identifies the active entry.
            let state = if matches!(tag, Some(t) if std::ptr::eq(t, *item_tag)) { 1isize } else { 0isize };
            unsafe {
                let _: () = msg_send![&**item, setState: state];
            }
        }
    }

    /// Checkmark the active loop mode.
    pub fn set_loop_mode(&self, mode: LoopMode) {
        for (item_mode, item) in &self.loop_items {
            let state = if *item_mode == mode { 1isize } else { 0isize };
            unsafe {
                let _: () = msg_send![&**item, setState: state];
            }
        }
    }

    /// Checkmark the active FPS override (0 = 原始).
    pub fn set_fps(&self, fps: u32) {
        for (item_fps, item) in &self.fps_items {
            let state = if *item_fps == fps as isize { 1isize } else { 0isize };
            unsafe {
                let _: () = msg_send![&**item, setState: state];
            }
        }
    }
}
