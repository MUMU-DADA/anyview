#!/bin/bash
# 在 Linux + Wine 环境下编译并验证 Windows C# UniFFI 绑定：bash tools/windows/check-binding.sh
# 用 out/windows/ 里的核心 DLL 和生成的绑定（都由 tools/windows/build-core.sh 生成），把 C# 接口测试
# apps/windows/binding-check 编译成自包含的 Windows 程序，在 Wine 里运行（不需要 WinUI，Wine 能跑）。
# 测试图片用 anyview-fixtures 生成的 tests/fixtures/generated（Linux 自测会生成）。
# 编译在 target/windows-binding 里的副本上做，不在 apps/windows 里留下 bin、obj。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$REPO/target/windows-binding"
DOTNET="${DOTNET:-$HOME/.dotnet/dotnet}"
WINE="${WINE:-$HOME/.local/opt/wine/usr/bin/wine}"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1

for f in "$REPO/out/windows/anyview_ffi.dll" "$REPO/out/windows/bindings/anyview_ffi.cs"; do
    if [[ ! -f "$f" ]]; then
        echo "缺少 $f：先在 anyview-dev 容器里运行 tools/windows/build-core.sh" >&2
        exit 1
    fi
done
FIXTURES="$REPO/tests/fixtures/generated"
if [[ ! -d "$FIXTURES/sort" ]]; then
    echo "缺少测试图片 $FIXTURES：先运行 cargo run --release -p anyview-fixtures -- --out tests/fixtures/generated" >&2
    exit 1
fi

rm -rf "$WORK/src" "$WORK/app"
mkdir -p "$WORK"
cp -r "$REPO/apps/windows" "$WORK/src"
find "$WORK/src" -type d \( -name bin -o -name obj \) -prune -exec rm -rf {} +
cd "$WORK/src/binding-check"
"$DOTNET" publish -c Release -o "$WORK/app" -p:AnyviewBindings="$REPO/out/windows/bindings/anyview_ffi.cs"
cp "$REPO/out/windows/anyview_ffi.dll" "$WORK/app/"
# HEIC 用的 heif.dll（tools/windows/build-heif.sh 编出来的）：有就带上，核心在自己旁边找它。
if [[ -f "$REPO/out/windows/heif.dll" && "${NO_HEIF:-}" != 1 ]]; then
    cp "$REPO/out/windows/heif.dll" "$WORK/app/"
fi

# Wine：.NET 程序要用 Wine 自带的 mscoree，WINEDLLOVERRIDES 里不能关掉它。
export WINEPREFIX="$WORK/wine" WINEDEBUG=-all WINEDLLOVERRIDES="mshtml="
# 登记到注册表、撤销在 Wine 自己的注册表里查，不会碰到谁的系统。
export ANYVIEW_CHECK_REGISTRY=1
to_win() { local p="$1"; echo "Z:${p//\//\\}"; }
echo "在 Wine 里运行 C# 接口测试："
# 有 RAW 样片（tools/testdata/fetch.py --raw）时再试 LibRaw：这个柯达文件里没有 JPEG 预览。
RAW_SAMPLE="$REPO/tests/fixtures/raw/Kodak_DC120_8bit_4_3_.kdc"
if [[ -f "$RAW_SAMPLE" ]]; then
    "$WINE" "$(to_win "$WORK/app/BindingCheck.exe")" "$(to_win "$FIXTURES")" "$(to_win "$RAW_SAMPLE")"
else
    "$WINE" "$(to_win "$WORK/app/BindingCheck.exe")" "$(to_win "$FIXTURES")"
fi
