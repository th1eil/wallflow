# WallFlow

**让壁纸、时钟和多屏桌面，在菜单栏里统一管理。**

WallFlow 是一款使用 Rust 与 AppKit 构建的 macOS 原生壁纸应用。支持本地图片、动态壁纸、视频、定时轮播和图像滤镜，也可以为桌面叠加双时区时钟，并适配 MacBook 的刘海区域。

应用常驻菜单栏，不占用 Dock 图标，无 Electron 或 WebView 依赖。

## 功能一览

| 功能 | 说明 |
| --- | --- |
| 本地壁纸库 | 递归扫描文件夹，支持 PNG、JPEG、WebP、HEIC / HEIF |
| 动态与视频壁纸 | 支持 GIF、APNG、MP4、WebM；提供暂停、循环模式与帧率控制，视频兼容性取决于系统解码能力 |
| 壁纸切换 | 支持顺序、随机和手动模式，可设置自动切换间隔，并回看切换历史 |
| 随机壁纸获取 | 支持自定义图片来源；先预览，再选择「仅保存」或「保存并应用」 |
| 多显示器 | 可选择操作目标屏幕，各屏独立维护壁纸、滤镜和播放状态 |
| 图像滤镜 | 亮度、对比度、饱和度、色温、高斯模糊，以及原图、复古、胶片、黑白、低饱和预设 |
| 桌面时钟 | 主副时区、12 / 24 小时制、日期、农历与节假日显示，可调文字颜色、字号、字重和背景样式 |
| 自动对比色 | 根据壁纸背景亮度调整时钟文字，提高可读性 |
| 刘海适配 | 在有刘海的屏幕顶部合成黑色区域，并支持圆角调节 |
| 原生设置 | 分页管理壁纸库、滤镜、时钟、外观和刘海设置；支持配置文件热重载 |

## 开始使用

### 运行应用

需要 **macOS 13.0+**。支持面向 Apple Silicon 和 Intel 构建；本地构建默认生成当前机器架构的程序。

如果已有 `WallFlow.app`，可将它复制到「应用程序」后打开；也可以打开 DMG，将其中的 App 复制出来使用。

从源码启动，需要 Rust 1.84.0+、Cargo 和 Xcode Command Line Tools。在项目根目录执行：

```bash
cargo run --release
```

启动后，从菜单栏的 WallFlow 图标进入操作菜单。

### 设置你的壁纸库

1. 点击「选择壁纸文件夹...」，选取存放图片或视频的目录，子文件夹也会被扫描。
2. 打开「设置」，选择切换模式与轮播间隔。默认目录为 `~/Pictures`，默认每 5 分钟顺序切换。
3. 使用「上一张」「下一张」「随机切换」浏览壁纸；连接多台显示器时，先在显示器子菜单中选择操作目标。
4. 按需开启桌面时钟、调整滤镜与刘海样式，点击「应用」保存设置。

部分设置会实时预览到桌面；设置窗口中的「取消」不写入配置，但不会回滚已经显示的桌面预览。

### 获取随机壁纸

点击「获取随机壁纸...」，下载完成后会先显示图片预览：

| 操作 | 结果 |
| --- | --- |
| 仅保存 | 加入壁纸库，保留当前桌面与选中项 |
| 保存并应用 | 加入壁纸库，并应用到当前选中的显示器 |
| 关闭预览窗口 | 取消本次操作，清理临时图片 |

确认前，图片只保存在临时目录，不会加入壁纸库或参与轮播。下载和保存期间不会重复发起任务；已有预览会重新显示。

默认来源为 `https://picsum.photos/1920/1080`，可在设置中替换为返回有效图片的直链。获取在线壁纸需要网络连接，本地壁纸不需要。

## 配置

首次启动会创建配置文件：

```text
~/.config/wallflow/config.toml
```

日常使用可直接通过设置窗口修改，也可以编辑 TOML 文件；保存后应用会自动重新加载。下面是一份包含常用选项的配置示例，未列出的时钟和刘海选项使用默认值。

```toml
wallpaper_dir = "/Users/yourname/Pictures" # 改为你的壁纸目录
interval_secs = 300                        # 自动切换间隔，单位：秒
mode = "sequential"                       # sequential / random / manual
random_wallpaper_url = "https://picsum.photos/1920/1080"

[filters]
brightness = 0.0  # -1.0 ～ 1.0
contrast = 0.0    # -1.0 ～ 1.0
saturation = 0.0  # -1.0 ～ 1.0
temperature = 0.0 # -1.0 ～ 1.0
blur = 0.0        # 0.0 ～ 20.0

[clock]
enabled = true
primary_timezone = "local"             # 跟随系统，或使用 IANA 时区名
secondary_enabled = true
secondary_timezone = "Europe/London"
use_24_hour = true
show_seconds = false
show_date = true
show_lunar = true
show_holiday = true
text_color = "#FFFFFF"
text_align = "center"                  # center / right
auto_contrast = true

[notch]
enabled = true
corner_radius = 10.0 # 单位：pt；设置界面支持 0 ～ 24
```

`manual` 模式会停止自动轮播。刘海适配仅在带刘海的屏幕上生效，无刘海屏幕和外接显示器会自动跳过。

完整配置字段及默认值见 [src/config.rs](src/config.rs)。

## 构建与打包

在 macOS 上执行：

```bash
# 构建发布版可执行文件
cargo build --release

# 构建 App，并生成 DMG
bash package.sh

# 启动打包后的应用
open target/release/WallFlow.app
```

| 产物 | 用途 |
| --- | --- |
| `target/release/wallflow` | 命令行启动的可执行文件 |
| `target/release/WallFlow.app` | 可直接运行的 macOS 应用 |
| `target/release/WallFlow.dmg` | 用于分发的磁盘映像 |

打包脚本调用系统自带的 `sips`、`iconutil` 和 `hdiutil`，生成图标、应用目录与 `Info.plist`。

脚本不包含分发签名与公证流程。如需本地 ad-hoc 签名，应在签名后重新生成 DMG，确保安装包内包含签名后的 App：

```bash
codesign --force --deep --sign - target/release/WallFlow.app
plutil -lint target/release/WallFlow.app/Contents/Info.plist
codesign -v --deep --strict target/release/WallFlow.app

hdiutil create -fs HFS+ -volname WallFlow \
  -srcfolder target/release/WallFlow.app -format UDZO \
  -ov target/release/WallFlow.dmg
hdiutil verify target/release/WallFlow.dmg
```

Ad-hoc 签名不等同于 Apple 公证。当前图标源文件为低分辨率图片，如需改善大尺寸图标，可替换 `assets/icon.png` 后重新打包。

## 开发说明

### 技术与模块

核心使用 Rust 2021、Tokio 和 objc2；界面使用 AppKit，图像滤镜使用 wgpu / WGSL 并提供 CPU 回退，视频解码使用 AVFoundation，配置使用 Serde / TOML 与 notify。

| 模块 | 职责 |
| --- | --- |
| `main.rs`、`app_event.rs` | 应用状态、事件分发与后台结果处理 |
| `menubar.rs`、`settings.rs`、`settings_ui.rs` | 菜单栏、设置逻辑与原生布局 |
| `library.rs`、`scheduler.rs` | 壁纸扫描、切换历史与轮播调度 |
| `wallpaper.rs`、`random_preview.rs` | 壁纸下载、确认预览、保存与系统壁纸设置 |
| `screen.rs`、`playback.rs`、`dynamic.rs`、`video.rs` | 多屏状态及动态、视频播放 |
| `color.rs`、`shader.wgsl` | 图像滤镜与 GPU / CPU 处理 |
| `clock.rs`、`contrast.rs`、`lunar.rs`、`notch.rs` | 时钟、对比色、农历与刘海合成 |
| `config.rs`、`notification.rs`、`util.rs` | 配置、通知与共享工具 |

表中 Rust 文件均位于 `src/`。开发过程与验证记录见 [开发文档.md](开发文档.md)。

### 测试

```bash
cargo test --offline
```

`--offline` 只限制 Cargo 拉取依赖。测试中的在线壁纸下载仍需联网，视频用例需访问 macOS 解码服务；受限环境可能导致这两类用例失败。手动性能基准默认跳过。

### 资源开销

- 原图在无需滤镜和刘海合成时直接应用，GPU 滤镜管线按需初始化。
- 静态大图处理与首帧亮度采样共用处理名额，连续切换时会跳过过期请求；多屏处理可能依次完成。
- 时钟只更新变化的文字。GIF / APNG 会保留解码帧，长动图、高分辨率图片及高帧率视频仍可能带来较高资源开销。
- 内存与处理耗时受素材、屏幕尺寸和滤镜设置影响，项目不承诺固定内存或功耗上限。
