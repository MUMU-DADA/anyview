# 平台适配说明

本文记录 Anyview 在 macOS、Windows 和 Linux 三端的原生控件选择、系统能力集成与构建方式。

## 通用约定

### 原生控件对照

| 界面元素 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| 主窗口 | `NSWindow`（内容延伸至标题栏，支持自动隐藏标题栏） | WinUI 3 `Window`（自定义标题栏） | `GtkApplicationWindow` |
| 画布 | `NSView` + `CALayer` | Win2D（Direct2D） | 自定义 `GtkWidget`（GSK 绘制） |
| 控制栏与箭头 | `NSVisualEffectView`（macOS 26+ 使用 `NSGlassEffectView`） | 亚克力浮层 | `GtkOverlay` + `osd` 样式 |
| 右键菜单 | `NSMenu` | `MenuFlyout` | `GtkPopoverMenu`（嵌套子菜单） |
| 选择图片窗口 | `NSWindow` + 自绘缩略图带 + `NSTableView` | `Window` + `GridView` + `ListView` | `GtkWindow` + Cairo 缩略图带 + `GtkColumnView` |
| 设置窗口 | `NSWindow`（左侧分类列表 + 卡片分组） | `Window`（左侧 `NavigationView`） | `GtkWindow`（`GtkStackSidebar`） |
| 图标 | SF Symbols | Segoe Fluent Icons | 系统符号图标 |
| 对话框 | `NSAlert` | `ContentDialog` | `GtkAlertDialog` |

### 共同规则

- **高分屏缩放**：100% 原始大小表示 1 个图片像素对应 1 个逻辑像素（点），在高分屏上按系统缩放倍率显示。
- **键位与措辞**：快捷键以 [ACTIONS.md](ACTIONS.md) 为准；菜单措辞采用各平台习惯用语（如 macOS 的“在访达中显示”“移到废纸篓”）。
- **平台无关设置项**：不适用的设置项直接隐藏，不灰显占位（详见 [SETTINGS.md](SETTINGS.md)）。

### 系统能力对照

| 功能 | macOS | Windows | Linux |
| --- | --- | --- | --- |
| 移到废纸篓/回收站 | `FileManager.trashItem` | `IFileOperation`（支持撤销） | `GFile.trash` |
| 定位文件 | `NSWorkspace.activateFileViewerSelecting` | `SHOpenFolderAndSelectItems` | D-Bus `org.freedesktop.FileManager1.ShowItems` |
| 剪贴板 | `NSPasteboard` | `Clipboard` | `GdkClipboard` |
| 打印 | `NSPrintOperation` | `PrintManager` | `GtkPrintOperation` |
| 设为壁纸 | `NSWorkspace.setDesktopImageURL` | `IDesktopWallpaper` | Plasma `plasma-apply-wallpaperimage` / GNOME `gsettings` / Flatpak Wallpaper Portal |
| 外部编辑器 | `NSWorkspace.open(_:withApplicationAt:)` | `Process.Start` | 启动配置的外部程序（Flatpak 下走系统“打开方式”门户） |
| 默认看图程序 | `Info.plist` 类型声明 + `NSWorkspace.setDefaultApplication` | 注册表登记 + 系统“默认应用”页 | 用户 `mimeapps.list` + `.desktop` |
| 阻止息屏 | `IOPMAssertion` | `SetThreadExecutionState` | Inhibit Portal |
| 系统解码器 | ImageIO | WIC（及可选 `heif.dll`） | 系统 `libheif`（动态加载） |

---

## macOS

### 系统要求与权限

- 支持 macOS 14 及以上（Apple Silicon `arm64`）。
- 未启用 App Sandbox，以便用户打开单张图片后能够无缝浏览同目录其他图片及相邻文件夹；内置的 Quick Look 缩略图与预览扩展按 macOS 要求在独立沙盒扩展进程中运行。

### 窗口与交互特性

- 单进程多窗口，支持拖放文件/文件夹、访达“打开方式”和系统“打开最近使用”（开启“不保存浏览过的文件夹路径”时不写入）。
- 默认无边框跟图窗口，透明标题栏随控制栏自动隐藏；勾选“显示标题栏”可切换为常规带标题栏窗口；支持窗口边缘吸附。
- 全屏采用即时无动画全屏（`↩` / `⌃⌘F` / 绿色按钮），隐藏菜单栏与程序坞。
- macOS 26 及以上版本悬浮控制栏启用原生 Liquid Glass（`NSGlassEffectView`），并在设置中提供“液面鼓起”“玻璃水珠”“淡色亮底”三种悬停效果。
- 支持完整触控板手势：双指平移、捏合缩放、双指轻扫翻页、双指轻点两下切换 100%/适合窗口、双指旋转；通过 `NSEvent.hasPreciseScrollingDeltas` 区分普通滚轮与触控板。

### 菜单栏

macOS 顶部菜单栏与右键菜单共用核心动作树，快捷键与自定义键盘映射实时同步：

| 菜单 | 主要项目（`——` 为分隔线） |
| --- | --- |
| Anyview | 关于 Anyview —— 设置… `⌘,` —— 服务 —— 隐藏 `⌘H`、隐藏其他 `⌥⌘H`、全部显示 —— 退出 `⌘Q` |
| 文件 | 打开图片… `⌘O`、打开文件夹… `⇧⌘O`、打开最近使用 › —— 选择图片… `G`、在访达中显示 `⌘↩` —— 关闭图片、关闭窗口 `⌘W` —— 移到废纸篓 `⌘⌫`、复制到图片文件夹、复制到图片文件夹 2、移动到图片文件夹、用图像编辑器打开 `⌘E`、设为桌面图片 —— 打印… `⌘P` |
| 编辑 | 拷贝 `⌘C` |
| 显示 | 原始大小 `⌘0`、适合窗口 `⌘9`、适合宽度 `8`、拉伸小图 `Z` —— 智能双页阅览（左→右 `7` / 左←右 `6`）—— 放大 `⌘+`、缩小 `⌘-`、微调 `⌥⌘+`/`⌥⌘-`、100% `*`、锁定缩放 `⌘K` —— 显示图片信息 `⌘I`、锁定控制栏 `` ` `` —— 进入全屏幕 `⌃⌘F` |
| 图像 | 旋转（`⌥↑`/`⌘L`/`⌘R`/`⌥↓`）—— 翻转（`⇧⌘M`/`⇧⌘F`）—— 反相 `⇧⌘I`、Gamma 补偿 `⇧⌘G` —— 像素边缘 ›、插值算法 › |
| 前往 | 下一张 `→`、上一张 `←`、首张 `Home`、末张 `End` —— 跳 10 张、随机翻页 —— 下一个/上一个文件夹或压缩包 `]`/`[` —— 排序方式 › |
| 窗口 | 最小化 `⌘M`、缩放、总在最前 `⇧⌘A` |
| 帮助 | 键盘快捷键、支持的格式、开源许可 |

### 构建与打包

仅依赖 Xcode Command Line Tools 与 Rust 工具链（`rust-toolchain.toml`）：
- `tools/macos/编译测试.command`：编译 Rust 静态库、生成 Swift UniFFI 绑定、编译主程序与两个 Quick Look 扩展并执行绑定测试与自检。
- `tools/macos/打包.command`：将 `Anyview.app` 打包为压缩 DMG（`out/macos/Anyview-<版本>.dmg`）。
- `tools/macos/打开Anyview.command` / `清理.command` / `检查访达缩略图.command`：本地启动、清理与 Quick Look 扩展诊断。

---

## Windows

### 系统要求与架构

- 支持 Windows 10（1809+）与 Windows 11（x64）。
- 界面位于 `apps/windows/Anyview`，基于 C# + WinUI 3 + Win2D，采用 **Native AOT**（`PublishAot`）将应用与 .NET 运行时编译为本机可执行文件，随包附带 Windows App SDK 与 Win2D。
- 共用 Rust 核心编译为 `anyview_ffi.dll`，并通过 `third_party/uniffi-bindgen-cs` 生成 C# UniFFI 绑定。

### 系统集成与分发

- **菜单栏**：默认隐藏；在「设置 › 浏览」取消勾选“不显示菜单栏”后显示顶部菜单栏（与 macOS 菜单树同源，含“打开最近使用”）。
- **资源管理器集成**：安装版支持注册表当前用户级登记（无需管理员权限），由 `Anyview.exe --register` / `--unregister` / `--stop-hosts` 管理，提供资源管理器右键菜单（“用 Anyview 浏览图片”）及 `IThumbnailProvider` 缩略图扩展（支持系统未覆盖的 WebP、PSD、PSB、QOI、TGA、SVG、RAW 等格式）。
- **可选 HEIC 支持**：优先使用系统 WIC（需安装 HEIF/HEVC 扩展）；如与 `anyview_ffi.dll` 同目录放置了由 `tools/windows/build-heif.sh` 构建的 `heif.dll`，核心会自动动态加载作为补充。
- **安装包与免安装版**：支持 NSIS 安装程序（`tools/windows/installer/anyview.nsi`、`tools/windows/build-installer.sh`）与绿色免安装 ZIP（`tools/windows/build-portable.sh`）。
- **缩略图调试日志**：在 `%TEMP%` 下创建空文件 `Anyview-shell.log` 时，缩略图组件会逐条记录宿主进程、格式、尺寸、结果与耗时。

### 构建与测试

- 在 Linux/MinGW-w64 环境执行 `tools/windows/build-core.sh` 编译 `anyview_ffi.dll` 并生成 C# 绑定（可用 `tools/windows/check-binding.sh` 在 Wine 中校验绑定与外壳登记逻辑）。
- 在 Windows（需 .NET 10 SDK 与 Visual Studio C++ 生成工具）运行 `tools/windows/编译测试.cmd` 编译 Native AOT 应用并运行完整自检；通过 `tools/windows/package.ps1` 整理打包目录或验证安装程序。

---

## Linux

### 环境与界面

- 支持 GTK 4.14 及以上，兼容 Wayland 与 X11；界面位于 `apps/linux`，直接链接 `anyview-core`，跟随系统 GTK 主题（不依赖 libadwaita）。
- 多进程窗口模型：开启“不允许多个实例”后，通过本机 Unix 套接字（`$XDG_RUNTIME_DIR/anyview-instance.sock`，Flatpak 下位于 `$XDG_RUNTIME_DIR/app/<ID>/`）将新打开的图片转交现有窗口并传递激活令牌。
- 右键菜单采用 `GtkPopoverMenu` 嵌套子菜单；标题栏与窗口吸附由桌面窗口管理器负责。
- 色彩管理：X11 下读取根窗口 `_ICC_PROFILE` 并在核心转换；Wayland 下交由合成器自行管理。

### 桌面集成与 Flatpak

- **原生安装**：「设置 › 格式关联」可将 Anyview 写入用户 `mimeapps.list` 作为默认看图软件，并记录旧值支持一键恢复。
- **Flatpak**：打包清单位于 `apps/linux/flatpak/local.anyview.Anyview.yml`（GNOME 51 运行时），通过 `tools/linux/flatpak/build.sh` 离线构建单文件 `.flatpak`。由于 Flatpak 沙盒不自动导出文件管理器缩略图生成器，「设置 › 格式关联」提供按钮按需在用户目录登记调用 `flatpak run` 的缩略图生成器（支持 KDE Dolphin 等文件管理器）。
- **测试**：`apps/linux/scripts/selftest.sh` 可在无头 Wayland（sway）环境中驱动完整功能与性能自检；`tools/linux/desktops/` 提供基于容器的 KDE/GNOME 桌面集成测试脚本。
