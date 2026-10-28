## 设置窗口
- 原生设置界面由 `src/settings.rs` 管控配置与事件，由 `src/settings_ui.rs` 管控布局和材质；视觉改动应尽量保持这条边界。
- `NSScrollView` 的文档视图使用 flipped 坐标；页面高度由内容栈撑开。分类卡片隐藏时，`NSStackView` 需折叠隐藏视图。
- 窗口级视图用 frame 和 autoresizing；滚动页内部使用 Auto Layout。混用窗口级约束曾导致设置窗口宽度塌缩。
- `iconutil` 在本地受限沙盒内会误报 `Invalid Iconset`；同一完整图标集在沙盒外转换成功。打包脚本应生成标准的 10 个图标尺寸文件。
- 随机壁纸下载用例访问 `picsum.photos`；无网络的测试环境会因 DNS 失败，需在可联网环境复测。
- 原生设置 UI 采用 180pt 侧栏、右侧可滚动内容和固定底栏；长输入框与滑块使用低 hugging 优先级伸缩，最小窗口 760×500pt。
- `NSViewAutoresizingMask` 中 MaxYMargin(32) 才能让底部 y=0 的操作栏贴底；MinYMargin(8) 会在窗口缩小时把它推到可视区外。
- `NSPopUpButton selectItemWithTag:` 返回 BOOL；用 `let _: bool` 接收，否则 objc2 运行时签名检查会 panic。
- 独立错误提示应隐藏其整行容器，并让垂直 `NSStackView` 开启 `setDetachesHiddenViews(true)`，仅隐藏文字仍会留下空白。
- 目视检查设置页可用 `WF_DEBUG_SHOW=1`；加 `WF_DEBUG_COMPACT=1` 可直接以 760×500pt 构造窗口，且调试模式不显示时钟遮挡层。
- 设置页实时预览直接改写运行时滤镜/时钟状态；取消或关闭窗口时需撤销已发出及待发出的预览事件，否则界面值与运行时状态会分离。
- 颜色预设在自定义 HEX 时会在首位插入「自定义」；处理菜单选择应使用 item 的 tag，不能用 indexOfSelectedItem 索引静态预设数组。

## 打包
- `package.sh` 的 Info.plist heredoc 结束符当前在**第 68 行**；`head -66 package.sh | bash` 会切断 heredoc，产出 `Encountered unexpected EOF` 的坏 plist —— `.app` 表面完整、`open` 也不报错，但分发包带病。要跳过 DMG 用 `sed '/^hdiutil create/d' package.sh | bash`。
- `package.sh` 不含签名步骤。分发前补 `codesign --force --deep --sign - target/release/WallFlow.app`，**签名后重建 DMG**，否则 DMG 里是未签名版本。
- 打包后必检两项：`plutil -lint .../Contents/Info.plist` 须输出 OK；`codesign -v --deep --strict` 须退出码 0。

## 壁纸切换与图像管线（2026-10-28，macOS 27）
- **系统壁纸状态存的是文件 URL**：`~/Library/Application Support/com.apple.wallpaper/Store/Index.plist` 里 `SystemDefault/Desktop/.../url/relative` 直接指向我们交给 `setDesktopImageURL` 的文件。所以产物**不能放 `$TMPDIR`**（下次切换/退出即删，重启后桌面指向失效路径），要落 `~/Library/Application Support/WallFlow/cache`；退出时也**不要删**，交给下次启动的 `cleanup_stale_cache()` 收敛（保留最近 4 张，多屏各一张）。
- **`wgpu::Device::create_texture` 超限是 panic，不是 Err**：默认 `max_texture_dimension_2d = 8192`，9000×6000 的相机原图会直接打死工作线程、`process_image` 的 `Err` 分支根本兜不住。进 GPU 前必须用 `FilterPipeline::fits_device()` 自查并回退 CPU。
- **滤镜参数必须带容差**：设置面板滑块会往 config 写 ±0.01 量级残留（-100..100 量纲下约 0.5），`== 0.0` 严格判等会让「肉眼就是原图」的配置永远走完整管线。容差取 0.5，模糊单独 0.05（单位是像素）。
- **降采样：`imageops::thumbnail`(box) 比 `resize`(Triangle) 快约 4 倍**（9000×6000→5120×3413 实测 129ms vs 502ms），缩小场景质量无可见差别；Target 取「屏幕物理像素」，只缩不放。
- **输出改 JPEG(q92)**：全尺寸 PNG 编码是 514ms/88MB，JPEG 是 279ms/6MB，且 macOS 26+ 由系统进程按文件 URL 解码渲染，交给它的文件越小越省。原图真的含透明像素（按像素扫，不只看扩展名）才回落 PNG。
- **`CGDisplayCreateImage` 要预检权限**：自 10.15 废弃且需屏幕录制授权，无权限时每次切换仍会同步跑一遍注定失败的捕获，淡入层是空白。用 `CGPreflightScreenCaptureAccess()` 先判，没有就直接 `set_wallpaper`。
- 基准：`cargo test --release --offline bench_wallpaper_pipeline -- --ignored --nocapture`（取壁纸目录最大一张，打印解码/降采样/滤镜/PNG/JPEG 耗时）。单测 35 项，新增 4 项覆盖容差、降采样边界、alpha 判定、CPU 回退尺寸。

## 刘海屏黑色矩阵（2026-10-28）
- **不要用窗口盖菜单栏**：第一版在 level 25（高于菜单栏 24）建了一条整屏宽的黑窗，把菜单标题与状态图标全挡住了 —— 即使 `ignoresMouseEvents` 让点击能穿透，看不见也没法用。**正确做法是把黑条渲染进壁纸图片**，菜单栏材质会自然采样到黑色，内容照常可见。
- **对齐前提：输出图必须精确等于屏幕物理像素尺寸**。系统按 Cover 语义缩放并居中裁切壁纸，尺寸不一致时，画的 N 像素黑条会被再缩放，黑条高度就偏离刘海高度。用 `notch::cover_to()`（Cover + 居中裁切）而不是 `fit_for_screen()`（只缩不放）。
- **绘制顺序**：解码 → `cover_to` → 亮度采样 → 滤镜 → **最后画黑条** → 编码。黑条必须在滤镜之后，否则会被亮度/色温一起染色，得不到纯黑。
- **圆角是「外翻」不是「内收」**（第一版画反了）：黑条在左右两侧向下多延伸 `radius`，圆心在 `(r, bar_h + r)` 与 `(w - r, bar_h + r)`，圆外的区域才是黑 —— 于是黑色向两侧铺开、下方壁纸的顶角呈圆角收进来。
- 刘海判定仍是 `NSScreen.safeAreaInsets.top > 0`（本机 MBA 13.6"/27.2 实测 38pt，屏 1710×1112pt、backing 2.0 → 黑条 76px、圆角 10pt→20px）。无刘海屏不进合成，零成本。
- 代价：有刘海且开启时，每次换壁纸都要「解码 + cover + 编码」（54MP 源图约 1.2s，后台线程）。这是图像方案的固有成本；关掉开关即可回到零滤镜直设原图的快路径。
- **设置面板的分类是强绑定的固定数组**：`SectionColumn::section()` 每调用一次就**新建一张卡片**，而 `settings_ui::navigation_sidebar()` 的 `[Retained<NSButton>; N]` 与 `SettingsNavPtrs { cards: [_; N] }` 写死数量，`try_into().expect(...)` 会在启动时直接 panic。加分类必须同步改：`settings_ui.rs` 的返回类型 + `std::array::from_fn` 长度 + title/symbol 数组、`settings.rs` 的 `SettingsNavPtrs` 三个数组与 `card_views` 长度。



