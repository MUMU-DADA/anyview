#!/bin/bash
# 建 Flatpak 打包、测试用的镜像 localhost/anyview-flatpak：在 anyview-desktops（Arch + KDE + GNOME，
# 见 tools/linux/desktops/）上加装 flatpak、flatpak-builder，并从 Flathub 装好 GNOME 运行时、SDK 和 Rust 扩展。
# 均安装在构建镜像内，不修改宿主机 Flatpak 环境。镜像可复用，清理见 clean.sh --image。
# 用 podman run 装好再 podman commit：Flatpak 装东西时要用 bubblewrap，podman build 里起不来。
#
# 用法：tools/linux/flatpak/build-image.sh        镜像已存在时仅补装缺失包
#                     tools/linux/flatpak/build-image.sh --new  重新建（更新运行时）
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
BASE="localhost/anyview-desktops:latest"
IMAGE="localhost/anyview-flatpak:latest"
MANIFEST="$REPO/apps/linux/flatpak/local.anyview.Anyview.yml"
GNOME="$(sed -n "s/^runtime-version: *'\{0,1\}\([0-9.]*\)'\{0,1\} *$/\1/p" "$MANIFEST")"
[ -n "$GNOME" ] || { echo "打包清单里没有 runtime-version"; exit 2; }
# 系统包：flatpak、flatpak-builder 打包用；桌面检查还要 KDE 的门户（真实的 KDE Plasma 用它）和 KDE 的桌面
# （plasmashell 和它要加载的桌面外壳 plasma-desktop：壁纸由它管，KDE 的壁纸门户要找它）。
PACKAGES="flatpak flatpak-builder xdg-desktop-portal-kde plasma-workspace plasma-desktop"
NAME="anyview-flatpak-setup-$$"
trap 'podman rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
RUN=(podman run --name "$NAME" --cpus 2 --security-opt seccomp=unconfined --security-opt systempaths=unconfined)

if podman image exists "$IMAGE" && [ "${1:-}" != --new ]; then
  # 后来加进 PACKAGES 的包：在现有镜像上补装（先按镜像里的软件源列表装；源里已没有这个版本时再整体更新），不重下运行时。
  missing="$(timeout -k 5 60 podman run --rm --network none "$IMAGE" pacman -T $PACKAGES || true)"
  if [ -z "$missing" ]; then
    echo "镜像已有：$IMAGE（要更新运行时用 --new）"
    exit 0
  fi
  echo "== 镜像里缺 $(echo $missing)，补装"
  timeout -k 10 1200 "${RUN[@]}" "$IMAGE" bash -euo pipefail -c "
    pacman -S --noconfirm --needed $(echo $missing) || pacman -Syu --noconfirm --needed $(echo $missing)
    rm -rf /var/cache/pacman/pkg/*
  "
  podman commit -q "$NAME" "$IMAGE" >/dev/null
  podman image prune -f >/dev/null
  echo "补装好了：$IMAGE（$(podman image inspect "$IMAGE" --format '{{.Size}}' | numfmt --to=iec)）"
  exit 0
fi
if ! podman image exists "$BASE"; then
  echo "== 先建 $BASE"
  podman build --cpu-period 100000 --cpu-quota 200000 -t "$BASE" \
    -f "$REPO/tools/linux/desktops/Containerfile" "$REPO/tools/linux/desktops"
fi

echo "== 在 $BASE 上装 $PACKAGES 和 GNOME $GNOME 的运行时、SDK、Rust 扩展（第一次约 1.5 GB）"
timeout -k 10 3600 "${RUN[@]}" -e GNOME="$GNOME" -e PACKAGES="$PACKAGES" "$BASE" bash -euo pipefail -c '
    pacman -Syu --noconfirm --needed $PACKAGES
    rm -rf /var/cache/pacman/pkg/*
    flatpak remote-add --system --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
    flatpak install --system -y --noninteractive flathub "org.gnome.Platform//$GNOME" "org.gnome.Sdk//$GNOME"
    # Rust 扩展的分支要与 SDK 底下的 freedesktop 版本相同（GNOME 的 SDK 元数据里写着）。
    ext="$(flatpak info --system --show-metadata "org.gnome.Sdk//$GNOME" \
      | sed -n "/^\[Extension org.freedesktop.Sdk.Extension\]/,/^\[/s/^version *= *//p" | head -1)"
    echo "Rust 扩展分支：$ext"
    [ -n "$ext" ] || { echo "SDK 的元数据里没有 freedesktop 扩展的版本"; exit 1; }
    flatpak install --system -y --noninteractive flathub "org.freedesktop.Sdk.Extension.rust-stable//$ext"
  '
# 装好就存成镜像；下面只是看看装了什么，出错也不影响。
podman commit -q "$NAME" "$IMAGE" >/dev/null
echo "镜像建好了：$IMAGE（$(podman image inspect "$IMAGE" --format '{{.Size}}' | numfmt --to=iec)）"
timeout -k 5 120 podman run --rm --network none --security-opt seccomp=unconfined --security-opt systempaths=unconfined "$IMAGE" bash -c "
  flatpak list --system --columns=ref,active,size
  flatpak run --command=sh org.gnome.Sdk//$GNOME -c '/usr/lib/sdk/rust-stable/bin/rustc --version' 2>/dev/null
" || true
