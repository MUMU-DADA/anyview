# Third-Party Notices / 第三方代码说明

本文件记录直接收录进本仓库 `third_party/` 的开源代码来源、版本、许可证与本地改动。
通过包管理器（Cargo、NuGet）引入的依赖由 `Cargo.lock` 与各平台工程文件锁定；各平台分发包附带的完整第三方许可证清单位于 `crates/anyview-core/licenses/<platform>.txt`，由 `tools/licenses/generate.sh` 生成，并在程序内「帮助 › 开源许可」及安装包旁的 `THIRD-PARTY-NOTICES.txt` 中展示。

## LibRaw

- **用途**：RAW 完整解码，由 `crates/anyview-libraw` 从源代码编译进核心，三端共用。
- **位置**：`third_party/libraw/`：上游的 `libraw/`、`internal/`、`src/`（移除 3 个占位文件 `*_ph.cpp` 和 `src/Makefile`），以及 `LICENSE.CDDL`、`LICENSE.LGPL`、`COPYRIGHT`。上游代码无改动；本项目封装位于 `crates/anyview-libraw/src/shim.cpp`。
- **来源**：https://github.com/LibRaw/LibRaw ，commit `2caab57e269cc7fce17c3189e4d4d7e3e5861875`。
- **许可证**：选用 **CDDL-1.0**（文件级开源义务）。其中包含的 dcraw、DCB、FBDD（BSD-3-Clause）、X3F 工具（BSD）及 Adobe DNG SDK 片段（MIT）声明见 `third_party/libraw/COPYRIGHT`。

## jxrlib

- **用途**：JPEG XR 解码，由 `crates/anyview-jxr` 从源代码编译进核心（三端均编入，Windows 上作为系统 WIC 解码器的回退）。
- **位置**：`third_party/jxrlib/`：上游 `common/include/`、`image/sys/`、`image/decode/`、`image/encode/`（剔除 x86 专用 `*_x86.c`）、`jxrgluelib/` 及 `LICENSE`。上游代码无改动；本项目封装位于 `crates/anyview-jxr/src/shim.c` 与 `crates/anyview-jxr/src/compat.h`。
- **来源**：https://github.com/4creators/jxrlib ，commit `1ef17dd8b7b6e958241914c8911d942a43837233`。
- **许可证**：BSD 2-Clause（Copyright © Microsoft Corp.，`third_party/jxrlib/LICENSE`）。

## libheif 与 libde265（Windows 可选 `heif.dll`）

- **用途**：Windows 平台可选的 HEIC/HEIF 解码扩展。两个库合编为独立动态库 `heif.dll`（libde265 静态链接进 libheif），放置于 `anyview_ffi.dll` 同目录，核心在运行时按需动态加载（未附带时继续使用系统 WIC 解码器）。macOS 使用 ImageIO，Linux 动态加载系统安装的 libheif，均不使用本目录源码。
- **位置**：
  - `third_party/libheif/`：上游 `CMakeLists.txt`、`cmake/`、`libheif/`、`gnome/`、`libheif.pc.in`、`COPYING`、`README.md`。
  - `third_party/libde265/`：上游 `CMakeLists.txt`、`cmake/`、`libde265/`、`extra/`、`libde265.pc.in`、`COPYING`、`AUTHORS`、`README.md`、`NEWS`。
  - 两者源码均无改动。
- **来源**：
  - https://github.com/strukturag/libheif ，tag `v1.23.7`，commit `12a259a5882d`。
  - https://github.com/strukturag/libde265 ，tag `v1.1.3`，commit `ba62bf4cfb32`。
- **构建**：通过 `bash tools/windows/build-heif.sh`（需要 CMake、Ninja、MinGW-w64）离线交叉编译，仅启用 libde265 解码器并静态链接 C++ 运行库，输出 `out/windows/heif.dll`。
- **许可证**：**LGPL-3.0-or-later**（`third_party/libheif/COPYING`、`third_party/libde265/COPYING`）。随包分发 `heif.dll` 时需附带 LGPL-3.0 与 GPL-3.0 许可证全文、对应源码与构建脚本，并保持其为可由用户自行替换的独立动态库。
- **专利说明**：HEVC 解码涉及相关专利许可，是否随分发包附带 `heif.dll` 可按发布需求决定；不附带时无需修改代码，Windows 仍可使用系统 HEIF/HEVC 扩展。

## NSIS（Windows 安装程序工具）

- **用途**：构建 Windows 安装程序 `Anyview-<version>-setup.exe`。Anyview 本体不含 NSIS 代码，仅安装包外壳与引导脚本使用 NSIS。
- **位置**：未收入本仓库，可使用系统安装的 `makensis` 或通过 `tools/windows/build-nsis.sh` 从源码自行编译。
- **来源**：https://github.com/kichik/nsis ，tag `v311`（3.11）。
- **许可证**：zlib/libpng 许可证（压缩模块分别遵循 zlib、bzip2、CPL-1.0 及相应安装程序豁免条款）。

## clip_to_psd（格式参考，未复制代码）

- **用途**：`crates/anyview-core/src/clip.rs` 在解析 Clip Studio Paint（`.clip`）文件块与 `CanvasPreview` 表结构时参考了该项目的 `clip_to_psd.py`，代码为本项目独立以 Rust 实现。
- **来源**：https://github.com/dobrokot/clip_to_psd ，commit `e8db68c768f52ec91ba7530820f52537261cbad0`（MIT）。

## psd-tools（格式参考，未复制代码）

- **用途**：`crates/anyview-core/src/psd_layers.rs` 在处理 PSD 矢量蒙版子路径合并与像素蒙版块字段顺序时参考了该项目，代码为本项目独立以 Rust 实现。
- **来源**：https://github.com/psd-tools/psd-tools ，commit `043c28addcfc6f5021e72716d463f9c78abfb43f`（MIT，Copyright (c) 2019 Kota Yamaguchi）。

## uniffi-bindgen-cs

- **用途**：为 Windows C# 界面生成 UniFFI 绑定（由 `tools/windows/build-core.sh` 调用）。
- **位置**：`third_party/uniffi-bindgen-cs/`：上游 `bindgen/` 目录与根目录 `LICENSE`。
- **来源**：https://github.com/NordSecurity/uniffi-bindgen-cs ，commit `7b1aaae16388a8d1e739114a632f357e1926296e`（`0.11.1+v0.31.0`）。
- **许可证**：MPL-2.0（`third_party/uniffi-bindgen-cs/LICENSE`）。
- **本项目改动**：
  1. 适配 UniFFI 0.32.2：更新 `Cargo.toml` 依赖版本；在 `gen_cs/compounds.rs`、`templates/SetTemplate.cs`、`templates/Types.cs` 补齐 Set 集合类型；在 `gen_cs/mod.rs` 支持 `Box<T>`。
  2. 修复枚举字段/类型命名冲突：在 `gen_cs/filters.rs` 与 `templates/EnumTemplate.cs` 检查同一枚举的所有变体，避免生成代码触发 CS8866。
  3. 关闭 `textwrap`、`clap` 非必要默认特性以精简依赖。
  4. 为包含 `IDisposable` 的生成类型添加 `partial` 修饰（`ObjectTemplate.cs`、`EnumTemplate.cs`、`ErrorTemplate.cs`），以满足 Windows Native AOT 下 CsWinRT 的接口表生成要求。
