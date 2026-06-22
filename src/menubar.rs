use std::sync::mpsc::Sender;
use objc2::{define_class, msg_send, ClassType, MainThreadOnly, DefinedClass};
use objc2::rc::{Allocated, Retained};
use objc2_app_kit::{NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSImage};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use crate::app_event::AppEvent;
use crate::app_event::WallpaperInfo;

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
                let _ = sender.send(AppEvent::Previous);
            }
        }

        #[unsafe(method(nextClicked:))]
        unsafe fn next_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::Next);
            }
        }

        #[unsafe(method(randomClicked:))]
        unsafe fn random_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::Random);
            }
        }

        #[unsafe(method(settingsClicked:))]
        unsafe fn settings_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::Settings);
            }
        }

        #[unsafe(method(exitClicked:))]
        unsafe fn exit_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::Exit);
            }
        }

        #[unsafe(method(chooseDirectoryClicked:))]
        unsafe fn choose_directory_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::ChooseDirectory);
            }
        }

        /// Open the current wallpaper directory in Finder.
        #[unsafe(method(openFolderClicked:))]
        unsafe fn open_folder_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::OpenWallpaperFolder);
            }
        }

        /// Download a random wallpaper from online source (picsum.photos).
        #[unsafe(method(downloadRandomClicked:))]
        unsafe fn download_random_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref sender) = *self.ivars().sender.lock().unwrap() {
                let _ = sender.send(AppEvent::DownloadRandomWallpaper);
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
                let preset = match tag_str.as_str() {
                    "original" => crate::color::FilterPreset::Original,
                    "vintage" => crate::color::FilterPreset::Vintage,
                    "film"    => crate::color::FilterPreset::Film,
                    "bw"      => crate::color::FilterPreset::BlackAndWhite,
                    "muted"   => crate::color::FilterPreset::Muted,
                    "japanese" => crate::color::FilterPreset::Japanese,
                    "cyberpunk" => crate::color::FilterPreset::Cyberpunk,
                    "dark"    => crate::color::FilterPreset::Dark,
                    "sunshine" => crate::color::FilterPreset::Sunshine,
                    "forest"  => crate::color::FilterPreset::Forest,
                    _ => return,
                };
                let _ = tx.send(AppEvent::SetFilterPreset(preset));
            }
        }

        /// Screen selection handler: reads tag from the NSMenuItem to set active screen.
        #[unsafe(method(screenSelected:))]
        unsafe fn screen_selected(&self, menu_item: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let tag: isize = msg_send![menu_item, tag];
                let _ = tx.send(AppEvent::SelectActiveScreen(tag as u32));
            }
        }

        /// Toggle pause/resume for the active screen's dynamic player.
        #[unsafe(method(togglePauseClicked:))]
        unsafe fn toggle_pause_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let _ = tx.send(AppEvent::TogglePause);
            }
        }

        /// Set loop mode to Infinite.
        #[unsafe(method(loopInfiniteClicked:))]
        unsafe fn loop_infinite_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let _ = tx.send(AppEvent::SetLoopMode(crate::playback::LoopMode::Infinite));
            }
        }

        /// Set loop mode to PlayOnce.
        #[unsafe(method(loopOnceClicked:))]
        unsafe fn loop_once_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let _ = tx.send(AppEvent::SetLoopMode(crate::playback::LoopMode::PlayOnce));
            }
        }

        /// Set loop mode to PingPong.
        #[unsafe(method(loopPingPongClicked:))]
        unsafe fn loop_pingpong_clicked(&self, _sender: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let _ = tx.send(AppEvent::SetLoopMode(crate::playback::LoopMode::PingPong));
            }
        }

        /// FPS selection: reads tag from NSMenuItem (tag = fps value).
        #[unsafe(method(fpsSelected:))]
        unsafe fn fps_selected(&self, menu_item: *mut NSObject) {
            if let Some(ref tx) = *self.ivars().sender.lock().unwrap() {
                let tag: isize = msg_send![menu_item, tag];
                let _ = tx.send(AppEvent::SetFps(tag as u32));
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
    _status_item: Retained<NSStatusItem>,
    _menu_target: Retained<MenuTarget>,
    _menu: Retained<NSMenu>,
    thumbnail_item: Retained<NSMenuItem>,
    info_item: Retained<NSMenuItem>,
    // Store screen menu items so we can update checkmarks on selection
    screen_items: Vec<Retained<NSMenuItem>>,
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
        
        // Thumbnail Item
        let thumbnail_item = NSMenuItem::new(mtm);
        let screen_items: Vec<Retained<NSMenuItem>>;
        let info_item = NSMenuItem::new(mtm);
        unsafe {
            thumbnail_item.setTitle(&NSString::from_str("No Wallpaper"));
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
            menu.addItem(&prev_item);
            
            // Next
            let next_item = NSMenuItem::new(mtm);
            next_item.setTitle(&NSString::from_str("下一张"));
            next_item.setTarget(Some(&*menu_target));
            next_item.setAction(Some(objc2::sel!(nextClicked:)));
            menu.addItem(&next_item);
            
            // Random
            let random_item = NSMenuItem::new(mtm);
            random_item.setTitle(&NSString::from_str("随机切换"));
            random_item.setTarget(Some(&*menu_target));
            random_item.setAction(Some(objc2::sel!(randomClicked:)));
            menu.addItem(&random_item);

            // --- 显示器 submenu ---
            // Build a list of connected screens; user picks which one nav controls target
            screen_items = {
                let screens = crate::wallpaper::get_screens(mtm);
                if screens.len() > 1 {
                    let mut items = Vec::new();
                    let screen_menu = NSMenu::new(mtm);
                    // Default active screen is the first one; pre-mark it
                    for (i, screen) in screens.iter().enumerate() {
                        let num = crate::screen::screen_number(screen);
                        let res = crate::screen::screen_resolution(screen);
                        let title = format!("显示器 {} ({})", i + 1, res);
                        let item = NSMenuItem::new(mtm);
                        item.setTitle(&NSString::from_str(&title));
                        item.setTarget(Some(&*menu_target));
                        item.setAction(Some(objc2::sel!(screenSelected:)));
                        // Store screen number in the tag (can't use representedObject for u32 easily)
                        let _: () = msg_send![&*item, setTag: num as isize];
                        // First screen is active by default
                        let _: () = msg_send![&*item, setState: (if i == 0 { 1isize } else { 0isize })];
                        screen_menu.addItem(&item);
                        items.push(item);
                    }
                    let screen_menu_item = NSMenuItem::new(mtm);
                    screen_menu_item.setTitle(&NSString::from_str("显示器"));
                    screen_menu_item.setSubmenu(Some(&screen_menu));
                    menu.addItem(&screen_menu_item);
                    items
                } else {
                    Vec::new()
                }
            };

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
                filter_menu.addItem(&item);
            }

            let filter_preset_item = NSMenuItem::new(mtm);
            filter_preset_item.setTitle(&NSString::from_str("滤镜预设"));
            filter_preset_item.setSubmenu(Some(&filter_menu));
            menu.addItem(&filter_preset_item);

            // --- 播放控制 submenu ---
            {
                let ctrl_menu = NSMenu::new(mtm);

                // Pause/Resume
                let pause_item = NSMenuItem::new(mtm);
                pause_item.setTitle(&NSString::from_str("暂停/继续"));
                pause_item.setTarget(Some(&*menu_target));
                pause_item.setAction(Some(objc2::sel!(togglePauseClicked:)));
                ctrl_menu.addItem(&pause_item);

                ctrl_menu.addItem(&NSMenuItem::separatorItem(mtm));

                // Loop mode submenu
                let loop_menu = NSMenu::new(mtm);
                for (title, sel) in [
                    ("无限循环", objc2::sel!(loopInfiniteClicked:)),
                    ("播放一次", objc2::sel!(loopOnceClicked:)),
                    ("乒乓循环", objc2::sel!(loopPingPongClicked:)),
                ] {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(title));
                    item.setTarget(Some(&*menu_target));
                    item.setAction(Some(sel));
                    loop_menu.addItem(&item);
                }
                let loop_item = NSMenuItem::new(mtm);
                loop_item.setTitle(&NSString::from_str("循环模式"));
                loop_item.setSubmenu(Some(&loop_menu));
                ctrl_menu.addItem(&loop_item);

                // FPS submenu
                let fps_menu = NSMenu::new(mtm);
                let fps_values: [(isize, &str); 8] = [
                    (0, "原始"), (1, "1 fps"), (5, "5 fps"), (10, "10 fps"),
                    (15, "15 fps"), (24, "24 fps"), (30, "30 fps"), (60, "60 fps"),
                ];
                for (fps, label) in &fps_values {
                    let item = NSMenuItem::new(mtm);
                    item.setTitle(&NSString::from_str(label));
                    item.setTarget(Some(&*menu_target));
                    item.setAction(Some(objc2::sel!(fpsSelected:)));
                    let _: () = msg_send![&*item, setTag: *fps];
                    fps_menu.addItem(&item);
                }
                let fps_item = NSMenuItem::new(mtm);
                fps_item.setTitle(&NSString::from_str("帧率"));
                fps_item.setSubmenu(Some(&fps_menu));
                ctrl_menu.addItem(&fps_item);

                let ctrl_parent = NSMenuItem::new(mtm);
                ctrl_parent.setTitle(&NSString::from_str("播放控制"));
                ctrl_parent.setSubmenu(Some(&ctrl_menu));
                menu.addItem(&ctrl_parent);
            }

            // Separator
            menu.addItem(&NSMenuItem::separatorItem(mtm));

            // Choose Directory
            let choose_item = NSMenuItem::new(mtm);
            choose_item.setTitle(&NSString::from_str("选择壁纸文件夹..."));
            choose_item.setTarget(Some(&*menu_target));
            choose_item.setAction(Some(objc2::sel!(chooseDirectoryClicked:)));
            menu.addItem(&choose_item);

            // Download Random Wallpaper
            let download_item = NSMenuItem::new(mtm);
            download_item.setTitle(&NSString::from_str("获取随机壁纸..."));
            download_item.setTarget(Some(&*menu_target));
            download_item.setAction(Some(objc2::sel!(downloadRandomClicked:)));
            menu.addItem(&download_item);

            // Open Wallpaper Folder
            let open_item = NSMenuItem::new(mtm);
            open_item.setTitle(&NSString::from_str("打开壁纸文件夹"));
            open_item.setTarget(Some(&*menu_target));
            open_item.setAction(Some(objc2::sel!(openFolderClicked:)));
            menu.addItem(&open_item);
            
            // Settings
            let settings_item = NSMenuItem::new(mtm);
            settings_item.setTitle(&NSString::from_str("设置"));
            settings_item.setTarget(Some(&*menu_target));
            settings_item.setAction(Some(objc2::sel!(settingsClicked:)));
            menu.addItem(&settings_item);
            
            // Exit
            let exit_item = NSMenuItem::new(mtm);
            exit_item.setTitle(&NSString::from_str("退出"));
            exit_item.setTarget(Some(&*menu_target));
            exit_item.setAction(Some(objc2::sel!(exitClicked:)));
            menu.addItem(&exit_item);
            
            status_item.setMenu(Some(&menu));
        }
        
        Self {
            _status_item: status_item,
            _menu_target: menu_target,
            _menu: menu,
            thumbnail_item,
            info_item,
            screen_items,
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
                        // Set size (128x80)
                        let size = objc2_foundation::NSSize::new(128.0, 80.0);
                        let _: () = msg_send![img, setSize: size];
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

    /// Update checkmark state for screen selection menu items.
    /// Called from the event loop when the user selects a different display.
    pub fn set_active_screen(&self, screen_number: u32) {
        for item in &self.screen_items {
            unsafe {
                let tag: isize = msg_send![&**item, tag];
                let state: isize = if tag as u32 == screen_number { 1 } else { 0 };
                let _: () = msg_send![&**item, setState: state];
            }
        }
    }
}
