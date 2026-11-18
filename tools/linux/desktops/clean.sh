#!/bin/bash
# 删掉桌面检查和 Flatpak 打包留下的东西。
#   clean.sh          删掉还在的 anyview-desktops-*、anyview-flatpak-* 容器（正常情况下用完就删了）
#   clean.sh --image  再删两个镜像 localhost/anyview-desktops、localhost/anyview-flatpak（共约 19 GB；下次自动重建）
#   clean.sh --all    再删 out/flatpak（Rust 依赖、编译缓存、安装包）
# 报告目录 reports/linux-desktops/ 在项目里（git 忽略），要删自己删。
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
for c in $(podman ps -a --format '{{.Names}}' | grep -E '^anyview-(desktops|flatpak)-'); do
  podman rm -f "$c" >/dev/null && echo "删掉了容器：$c"
done
if [ "${1:-}" = --image ] || [ "${1:-}" = --all ]; then
  # 先删派生的 anyview-flatpak，再删它的底 anyview-desktops。
  for image in localhost/anyview-flatpak:latest localhost/anyview-desktops:latest; do
    if podman image exists "$image"; then
      podman rmi "$image" >/dev/null && echo "删掉了镜像：$image"
    fi
  done
  podman image prune -f >/dev/null
fi
if [ "${1:-}" = --all ] && [ -d "$REPO/out/flatpak" ]; then
  # Flatpak 编译时容器里写的文件都属于自己（--userns=keep-id）；万一有别的用户号的，用 podman unshare 删。
  rm -rf "$REPO/out/flatpak" 2>/dev/null || podman unshare rm -rf "$REPO/out/flatpak"
  echo "删掉了：out/flatpak"
fi
left="$(podman ps -a --format '{{.Names}}' | grep -E '^anyview-(desktops|flatpak)-')"
echo "剩下的容器：${left:-无}"
for image in localhost/anyview-desktops:latest localhost/anyview-flatpak:latest; do
  if podman image exists "$image"; then echo "镜像留着：$image（clean.sh --image 删掉）"; fi
done
if [ -d "$REPO/out/flatpak" ]; then echo "留着：out/flatpak（$(du -sh "$REPO/out/flatpak" | cut -f1)；clean.sh --all 删掉）"; fi
