#!/bin/bash
# 生成各平台分发包附带的第三方许可证清单：
#   bash tools/licenses/generate.sh
# 1. cargo-about 0.9.3 列出编进程序的 Rust 库和它们的许可证原文（Windows、macOS 的核心按 anyview-ffi，Linux 按
#    anyview-linux，各按自己的目标平台筛）。cargo-about 从源代码编在 ~/.local/opt/cargo-about：
#      cargo install --locked cargo-about --version 0.9.3 --features cli --root ~/.local/opt/cargo-about
#    用 --frozen 运行：不联网（它只在缺许可证文件时才会去 GitHub 拉，本项目不这样拉代码）。
# 2. generate.py 再加上 C、C++ 写的库（LibRaw、jxrlib、UnRAR、heif.dll 等）和 Windows 随程序带的运行库，
#    写到 crates/anyview-core/licenses/<系统>.txt：核心把它编进程序（帮助 › 开源许可），安装包里另带一份。
# 加、减、升级依赖以后要重新运行：核心的测试按 Cargo.lock 核对清单是不是最新的。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
ABOUT="${CARGO_ABOUT:-$HOME/.local/opt/cargo-about/bin/cargo-about}"
if [[ ! -x "$ABOUT" ]]; then
    echo "没有 cargo-about：cargo install --locked cargo-about --version 0.9.3 --features cli --root ~/.local/opt/cargo-about" >&2
    exit 1
fi
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
about() {
    "$ABOUT" generate --frozen -c tools/licenses/about.toml -m "$1" --target "$2" tools/licenses/about.hbs -o "$TMP/$3.txt"
}
about crates/anyview-ffi/Cargo.toml x86_64-pc-windows-gnu windows
about crates/anyview-ffi/Cargo.toml aarch64-apple-darwin macos
about apps/linux/Cargo.toml x86_64-unknown-linux-gnu linux
python3 -I tools/licenses/generate.py "$TMP" crates/anyview-core/licenses
