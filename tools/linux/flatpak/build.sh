#!/bin/bash
# 离线构建 Anyview Flatpak 单文件安装包（输出 out/flatpak/Anyview.flatpak）。
#   1. 按 Cargo.lock 将 Rust 依赖 vendor 到 out/flatpak/vendor。
#   2. 在隔离容器（localhost/anyview-flatpak）中运行 flatpak-builder 离线构建并打包。
# 用法：tools/linux/flatpak/build.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
IMAGE="localhost/anyview-flatpak:latest"
WORK="$REPO/out/flatpak"
APP_ID="local.anyview.Anyview"
STAMP="$(date +%Y-%m-%dT%H-%M-%S)"

"$REPO/tools/linux/flatpak/build-image.sh"
mkdir -p "$WORK"

lock="$(sha256sum "$REPO/Cargo.lock" | cut -d' ' -f1)"
if [ "$(cat "$WORK/vendor.stamp" 2>/dev/null)" != "$lock" ] || [ ! -d "$WORK/vendor" ]; then
  echo "== Rust 依赖 vendor 到 out/flatpak/vendor"
  rm -rf "$WORK/vendor"
  (cd "$REPO" && timeout -k 10 1800 cargo vendor --locked --versioned-dirs out/flatpak/vendor >/dev/null 2>"$WORK/vendor.log")
  echo "$lock" >"$WORK/vendor.stamp"
fi

echo "== flatpak-builder（离线，容器用完即删；最多 60 分钟，第一次约 9 分钟）"
# 中途停下时容器不会自己停，也删掉。
trap 'podman rm -f "anyview-flatpak-build-$STAMP" >/dev/null 2>&1 || true' EXIT
# 限时：卡住时到点结束（--init 让容器里的 1 号进程把“结束”信号转给脚本，-k 30 是还不停就强行结束），容器由上面的 trap 删。
rc=0
timeout -k 30 3600 podman run --rm --init --name "anyview-flatpak-build-$STAMP" --cpus 2 --network none --userns=keep-id \
  --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  -v "$REPO:/src:ro" -v "$WORK:/src/out/flatpak" \
  -e HOME=/tmp/builder-home \
  "$IMAGE" bash -euo pipefail -c "
    mkdir -p /tmp/builder-home
    cd /src/out/flatpak
    flatpak-builder --disable-rofiles-fuse --disable-updates --jobs=2 --force-clean \
      --state-dir=/src/out/flatpak/state --repo=/src/out/flatpak/repo \
      /src/out/flatpak/build /src/apps/linux/flatpak/$APP_ID.yml >/src/out/flatpak/builder.log 2>&1 \
      || { tail -40 /src/out/flatpak/builder.log; exit 1; }
    # 单文件安装包：别的电脑上装时按 --runtime-repo 从 Flathub 装运行时。
    flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
      /src/out/flatpak/repo /src/out/flatpak/Anyview.flatpak $APP_ID
    # 编译目录很大（Rust 的中间文件），装包用不到，删掉。
    rm -rf /src/out/flatpak/build
  " || rc=$?
if [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
  echo "✗ 编译过了 60 分钟还没完，强行结束了（见 $WORK/builder.log）"
fi
[ "$rc" = 0 ] || exit "$rc"
ls -la "$WORK/Anyview.flatpak"
echo "安装包：$WORK/Anyview.flatpak（$(du -h "$WORK/Anyview.flatpak" | cut -f1)）"
