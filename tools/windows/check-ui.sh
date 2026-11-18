#!/bin/bash
# 在 Linux 环境下交叉检查 Windows 界面 C# 代码编译：bash tools/windows/check-ui.sh
# 加 --publish 目录：再按发布方式（自包含 + ReadyToRun）输出到那个目录，带上核心的 DLL，给 Wine 冒烟测试用
# （资源索引是空的替身；Wine 里 WinUI 起不来，只能测到 .NET 启动和调用核心）。
# 只查编译错误和警告：Windows 才有的两个工具（合并清单的 mt.exe、生成资源索引的 makepri.exe）在这里不能运行，
# 用替身跳过；真正的编译、测试在 Windows 上由 tools/windows 里的「编译测试.cmd」做。
# 用本机的 .NET SDK（~/.dotnet，与 apps/windows/global.json 同一个功能区），NuGet 包放在 ~/.nuget/packages。
# 编译在 target/windows-ui 里的副本上做，不在 apps/windows 里留下 bin、obj。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$REPO/target/windows-ui"
DOTNET="${DOTNET:-$HOME/.dotnet/dotnet}"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1

rm -rf "$WORK/src"
mkdir -p "$WORK"
cp -r "$REPO/apps/windows" "$WORK/src"
find "$WORK/src" -type d \( -name bin -o -name obj \) -prune -exec rm -rf {} +

# mt.exe 的替身：不合并清单，只把最后一个输入清单复制到 -out: 指定的位置。
cat > "$WORK/fake-mt.sh" <<'SCRIPT'
#!/bin/bash
out=""
last=""
for a in "$@"; do
    case "$a" in
        -out:*) out="${a#-out:}" ;;
        -*) ;;
        *) last="$a" ;;
    esac
done
cp "$last" "$out"
SCRIPT
chmod 755 "$WORK/fake-mt.sh"

# 正式的程序（apps/windows/Anyview）用 out/windows/bindings 里生成的绑定（tools/windows/build-core.sh）。
BINDINGS="$REPO/out/windows/bindings/anyview_ffi.cs"
if [[ ! -f "$BINDINGS" ]]; then
    echo "缺少 $BINDINGS：请先运行 tools/windows/build-core.sh" >&2
    exit 1
fi
APP="${APP:-Anyview}"
cd "$WORK/src/$APP"
"$DOTNET" build -c Release -p:EnableWindowsTargeting=true \
    -p:ManifestTool="$WORK/fake-mt.sh" -p:AppxGeneratePriEnabled=false -p:AnyviewBindings="$BINDINGS" \
    -warnaserror -nowarn:NU1603

if [[ "${1:-}" == "--publish" ]]; then
    OUT="$2"
    # makepri.exe 的替身：空的资源索引（发布时要复制它）。
    PRI="$APP"
    touch "bin/Release/net10.0-windows10.0.19041.0/win-x64/$PRI.pri"
    "$DOTNET" publish -c Release --no-build -p:EnableWindowsTargeting=true \
        -p:ManifestTool="$WORK/fake-mt.sh" -p:AppxGeneratePriEnabled=false -o "$OUT"
    cp "$REPO/out/windows/anyview_ffi.dll" "$OUT/"
fi
