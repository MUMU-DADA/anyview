# Anyview macOS

Swift + AppKit 原生界面，通过 UniFFI 调用共用 Rust 核心。

## 构建与使用

在项目根目录的 `tools/macos/` 中提供了直接可用的构建与测试脚本（可在终端执行或访达双击）：

| 脚本 | 作用 |
| --- | --- |
| `编译测试.command` | 编译 Rust 核心与 Swift 应用、运行单元测试与应用自检，报告输出至 `reports/macos/<时间>/` |
| `打开Anyview.command` | 启动已编译的 `out/anyview-build/Anyview.app` |
| `打包.command` | 将测试通过的 `Anyview.app` 打包为 `out/macos/Anyview-<版本>.dmg`（本地 ad-hoc 签名；未公证版本在其他 Mac 首次打开时需在「系统设置 › 隐私与安全性」放行） |
| `检查访达缩略图.command` | 检查已安装到 `/Applications/Anyview.app` 的 Quick Look 缩略图与预览扩展状态 |
| `清理.command` | 清理本机编译缓存、临时目录及 Anyview 写入的系统状态 |

### 依赖

- 仅需 macOS 14+ 与系统 Xcode Command Line Tools（无需完整 Xcode 或 Homebrew）。
- Rust 版本固定在根目录 `rust-toolchain.toml`：若本机已安装 `rustup` 会自动使用对应工具链；未安装 `rustup` 时，脚本可下载并校验官方独立安装包至本地构建目录。

## 目录结构

| 路径 | 内容 |
| --- | --- |
| `Sources/Anyview/` | 主应用源码（AppKit / SwiftUI 浮层） |
| `Extensions/Thumbnail/` | Quick Look 缩略图扩展 |
| `Extensions/Preview/` | Quick Look 预览扩展 |
| `Tests/BindingCheck/` | Swift UniFFI 接口集成测试 |
| `Resources/` | `Info.plist`、图标与多语言本地化资源 |
| `../../tools/macos/` | macOS 构建、自检与打包脚本 |
