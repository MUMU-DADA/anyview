#!/bin/bash
# 从源码构建 NSIS 3.11（makensis + Windows stubs/plugins，zlib 许可证）：
#   git clone --branch v311 --depth 1 https://github.com/kichik/nsis /path/to/nsis
#   NSIS_SOURCE=/path/to/nsis bash tools/windows/build-nsis.sh
# 编出来的是在 Linux 上运行的编译器 makensis，加上装进安装程序里的 Windows 外壳（Stubs）和插件（用 mingw 编）。
# 装在用户目录 ~/.local/opt/nsis-3.11，不动系统。要 scons（容器里 pacman -S scons）和 i686 的 mingw-w64。
# 不编帮助文档、不编只能在 Windows 上运行的小工具；不并行编译（它的构建脚本并行时会互相踩工作目录）。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SOURCE="${NSIS_SOURCE:-$REPO/out/nsis-src}"
PREFIX="${NSIS:-$HOME/.local/opt/nsis-3.11}"
WORK="$REPO/target/nsis"
if [[ ! -f "$SOURCE/SConstruct" ]]; then
    echo "未找到 NSIS 源代码目录 $SOURCE：请设置 NSIS_SOURCE 指向 NSIS 3.11 源码目录" >&2
    exit 1
fi
rm -rf "$WORK/src"
mkdir -p "$WORK"
cp -r "$SOURCE" "$WORK/src"
rm -rf "$WORK/src/.git"
cd "$WORK/src"
# 小工具里只要 UIs（安装程序各种界面的模板），其余的是在 Windows 上用的图形工具，不编。
scons -j1 XGCC_W32_PREFIX=i686-w64-mingw32- SKIPDOC=all NSIS_CONFIG_CONST_DATA_PATH=no \
    "SKIPUTILS=Library/RegTool,MakeLangId,Makensisw,NSIS Menu,SubStart,VPatch/Source/GenPat,zip2exe" \
    VERSION=3.11 PREFIX="$WORK/dist" install > "$WORK/build.log" 2>&1 || { tail -30 "$WORK/build.log" >&2; exit 1; }
rm -rf "$PREFIX"
mkdir -p "$(dirname "$PREFIX")"
cp -r "$WORK/dist" "$PREFIX"
# makensis 按环境变量 NSISDIR 找它的外壳、插件、界面模板（build-installer.sh 会设）。
NSISDIR="$PREFIX" "$PREFIX/makensis" -VERSION
echo
echo "装在 $PREFIX"
