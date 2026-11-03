# WallFlow

macOS 原生壁纸管理应用，Rust 编写，无 Electron/WebView 依赖。

单 `.app` 分发；内存占用取决于壁纸分辨率、动画帧数及滤镜设置。

## 功能

- **静态壁纸** — PNG / JPEG / WebP / HEIC（macOS 原生解码）
- **动态壁纸** — GIF / APNG 逐帧播放、MP4 / WebM 硬件解码
- **多显示器** — 每屏独立设置壁纸、独立滤镜和播放控制
- **GPU 滤镜** — 亮度 / 对比度 / 饱和度 / 色温 / 高斯模糊（wgpu Shader）
- **内置预设** — 原图 / 复古 / 胶片 / 黑白 / 低饱和
- **自动切换** — 定时轮播，顺序 / 随机 / 手动模式
- **热重载** — 配置修改后即时生效，无需重启
- **菜单栏常驻** — 不在 Dock 显示，托盘图标跟随深色/浅色模式

## 系统要求

- macOS 13.0+
- Apple Silicon 或 Intel x86_64

## 快速开始

```bash
# 构建
cargo build --release

# 运行（菜单栏出现图标）
./target/release/wallflow
```

## 打包为 .app

项目附带 `package.sh` 脚本，自动完成构建、.app 封装和 DMG 创建：

```bash
chmod +x package.sh
./package.sh
```

产物：
- `target/release/WallFlow.app` — 可直接运行的 .app 包
- `target/release/WallFlow.dmg` — 可分发的磁盘映像

### 打包脚本做了什么

1. `cargo build --release` 构建可执行文件
2. 创建标准的 macOS .app 目录结构（`Contents/MacOS`、`Contents/Resources`）
3. 用 `sips` + `iconutil` 从 `assets/icon.png` 生成 `.icns` 图标
4. 生成 `Info.plist`（含 `LSUIElement = true`，菜单栏应用）
5. `hdiutil` 创建 DMG

> **注意**：当前图标源文件是 18×18 的小图，放大后模糊。如需清晰图标，请将 `assets/icon.png` 替换为 ≥1024×1024 的 PNG。

## 配置

配置文件：`~/.config/wallflow/config.toml`

```toml
wallpaper_dir = "/Users/xxx/Pictures"   # 壁纸目录
interval_secs = 300                      # 自动切换间隔（秒）
mode = "sequential"                      # sequential | random

[filters]
brightness = 0.0     # -1.0 ~ 1.0
contrast = 0.0       # -1.0 ~ 1.0
saturation = 0.0     # -1.0 ~ 1.0
temperature = 0.0    # -1.0 ~ 1.0
blur = 0.0           # 0.0 ~ 20.0
```

修改后即时生效（`notify` crate 监听文件变化）。

## 项目结构

```
src/
├── main.rs          入口点（启动逻辑）
├── app_event.rs     AppEvent 事件枚举
├── util.rs          共享工具（TempFileGuard、find_class）
├── menubar.rs       菜单栏（NSStatusItem、NSMenu）
├── wallpaper.rs     屏幕检测、壁纸设置、图像解码
├── library.rs       壁纸库扫描、导航、历史记录
├── dynamic.rs       GIF/APNG 播放器
├── video.rs         MP4/WebM 视频播放器（AVFoundation）
├── playback.rs      播放控制类型（LoopMode、PlaybackCommand）
├── scheduler.rs     自动切换调度器
├── screen.rs        多屏状态管理
├── color.rs         GPU 滤镜管线（wgpu）+ CPU 回退
├── config.rs        TOML 配置读写 + 热重载
├── settings.rs      原生设置面板（NSWindow）
├── settings_ui.rs   设置面板的分类导航与布局
└── shader.wgsl      WGSL 着色器（滤镜 + 高斯模糊）
```

## 技术栈

| 组件 | 方案 |
|------|------|
| 语言 | Rust 2021 edition |
| 系统 API | objc2 / objc2-app-kit / objc2-foundation |
| 异步 | tokio |
| 图像解码 | image crate + macOS NSImage（HEIC 回退） |
| GPU 滤镜 | wgpu + WGSL |
| 视频解码 | AVFoundation（FFI） |
| 配置 | serde + toml |
| 热重载 | notify |
| 打包 | 手写 .app 目录 + hdiutil DMG |

## 开发

```bash
# 开发构建（增量编译更快）
cargo build

# 运行
cargo run

# 发布构建
cargo build --release
```

## 资源开销

- GPU 滤镜按首次使用初始化，原图与仅刘海合成无需创建 GPU 管线。
- 静态大图处理与首帧亮度采样依次执行，连续切换时提前跳过过期请求，限制同时存在的大图缓冲。
- 时钟仅刷新变化的文字；视频逐帧释放原生临时对象，并省去 TIFF 中转。
- 多屏同时切换会依次处理；GIF/APNG 仍保留全部解码帧，长动图可能使用较多内存。
- 以上优化已通过发布构建与自动测试；实际内存和芯片占用降幅仍待实机测量，未承诺固定上限。
