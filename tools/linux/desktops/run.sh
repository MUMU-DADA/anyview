#!/bin/bash
# Anyview 在真实桌面里的检查：在隔离的容器里先后起 KDE Plasma（kwin --virtual）、GNOME（gnome-shell --headless）
# 的虚拟桌面，检查文件管理器的缩略图（Dolphin、Nautilus）、设为默认看图软件后在桌面上打开图片、恢复原来的默认程序，
# 并截下 Anyview 窗口在各桌面里的样子。结果写到 reports/linux-desktops/<时间>/。
#
# 容器保持隔离：不挂载宿主机主目录、/run/user（D-Bus）与 /tmp，不联网；仅只读挂载程序、
# 测试图片、本目录，可写的只有报告目录。容器用完即删（--rm），镜像 localhost/anyview-desktops 留着下次用（clean.sh 可删）。
# GNOME 用 bubblewrap 隔离缩略图生成器，要在容器里再建命名空间、挂 /proc：所以不限 seccomp、不遮 /proc 下的系统路径
# （容器仍以无特权运行）。KWin 6 合成需要 GPU 渲染节点，因此仅传入 /dev/dri/renderD128。
# 测试用的主目录放在内存盘（tmpfs）上：KDE 的 KIO 不认识容器的 overlay 文件系统，就不做缩略图（真实电脑上不会这样）。
# 运行前后核对宿主机桌面相关配置未被改动。
#
# 用法：
#   tools/linux/desktops/run.sh [报告目录]                 检查编好的 target/release/anyview（selftest.sh 会编）
#   tools/linux/desktops/run.sh --flatpak [报告目录]       检查 out/flatpak/Anyview.flatpak（tools/linux/flatpak/build.sh 编），
#                                                           用镜像 localhost/anyview-flatpak（里面有 GNOME 运行时）
# 都要有测试图片 tests/fixtures/generated。
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
# 只检查一个桌面时：DESKTOPS=gnome tools/linux/desktops/run.sh …（默认 KDE、GNOME 都检查）。
DESKTOPS="${DESKTOPS:-kde gnome}"
MODE=native
if [ "${1:-}" = --flatpak ]; then
  MODE=flatpak
  shift
fi
STAMP="$(date +%Y-%m-%dT%H-%M-%S)"
FIXTURES="$REPO/tests/fixtures/generated"
NAME="anyview-desktops-$STAMP"
[ -d "$FIXTURES/zall" ] || { echo "没有测试图片 $FIXTURES/zall"; exit 2; }
if [ "$MODE" = flatpak ]; then
  IMAGE="localhost/anyview-flatpak:latest"
  OUT="${1:-$REPO/reports/linux-desktops/$STAMP-flatpak}"
  BUNDLE="$REPO/out/flatpak/Anyview.flatpak"
  [ -f "$BUNDLE" ] || { echo "没有 $BUNDLE：先编（tools/linux/flatpak/build.sh）"; exit 2; }
  "$REPO/tools/linux/flatpak/build-image.sh"
  APP_MOUNTS=(-v "$BUNDLE:/opt/anyview/Anyview.flatpak:ro" -v "$REPO/apps/linux/data:/opt/anyview/data:ro")
else
  IMAGE="localhost/anyview-desktops:latest"
  OUT="${1:-$REPO/reports/linux-desktops/$STAMP}"
  BIN="$REPO/target/release/anyview"
  [ -x "$BIN" ] || { echo "未找到 $BIN：请先构建（cargo build --release -p anyview-linux 或运行 apps/linux/scripts/selftest.sh）"; exit 2; }
  APP_MOUNTS=(-v "$BIN:/opt/anyview/anyview:ro" -v "$REPO/apps/linux/data:/opt/anyview/data:ro")
  if ! podman image exists "$IMAGE"; then
    echo "== 建镜像 $IMAGE（第一次要下载 KDE、GNOME，几分钟）"
    podman build --cpu-period 100000 --cpu-quota 200000 -t "$IMAGE" -f "$REPO/tools/linux/desktops/Containerfile" \
      "$REPO/tools/linux/desktops"
  fi
fi
mkdir -p "$OUT"

# 记录宿主机相关桌面配置，校验测试前后一致。
host_state() {
  local h="$HOME"
  find "$h/.config/mimeapps.list" "$h/.config/kde-mimeapps.list" "$h/.config/gnome-mimeapps.list" \
    "$h/.local/share/applications" "$h/.local/share/thumbnailers" "$h/.local/share/icons/hicolor" \
    "$h/.cache/thumbnails" "$h/.config/anyview" "$h/.local/state/anyview" \
    -maxdepth 3 -printf '%p %s %T@\n' 2>/dev/null | sort || true
}
host_state >"$OUT/host-before.txt"

LIMIT=1320
echo "== 容器 $NAME：KDE、GNOME 的虚拟桌面（$MODE；原生约 3 分钟，flatpak 约 8 分钟，最多 $((LIMIT / 60)) 分钟）"
status=0
# 中途停下（Ctrl-C、被结束）时容器不会自己停，也删掉。
trap 'podman rm -f "$NAME" >/dev/null 2>&1 || true' EXIT
# 限时三层：容器里每次运行 Anyview、截图等都限时，每个桌面最多 10 分钟（inside.sh），整个容器最多 22 分钟（这里）。
# --init：容器里的 1 号进程用 podman 自带的小程序，它把“结束”信号转给脚本。脚本自己当 1 号进程时会忽略这个信号，
# 避免脚本作为 PID 1 忽略 TERM 信号导致超时后挂起；-k 30 在 30 秒后强制结束。
timeout -k 30 "$LIMIT" podman run --rm --init --name "$NAME" --cpus 2 --network none --userns=keep-id --user 0 \
  --security-opt seccomp=unconfined --security-opt systempaths=unconfined \
  --device /dev/dri/renderD128 --tmpfs /home:rw,exec,mode=755 \
  "${APP_MOUNTS[@]}" \
  -v "$FIXTURES:/fixtures:ro" \
  -v "$REPO/tools/linux/desktops:/harness:ro" \
  -v "$OUT:/out" \
  -e HOST_USER="$(id -un)" -e MODE="$MODE" -e DESKTOPS="$DESKTOPS" \
  "$IMAGE" bash /harness/inside.sh || status=$?
# 到点强行结束时 --rm 来不及删容器，这里删。
podman rm -f "$NAME" >/dev/null 2>&1 || true
# 仅检查单个桌面时跳过缺失摘要，确保继续校验宿主机状态。
for d in $DESKTOPS; do cat "$OUT/$d/summary.txt" 2>/dev/null; done >"$OUT/summary.txt" || true
if [ "$status" = 124 ] || [ "$status" = 137 ]; then
  echo "✗ 整个检查过了 $((LIMIT / 60)) 分钟还没完，强行结束了（容器已删；卡在哪一步见 kde/、gnome/ 的 run.log）" >>"$OUT/summary.txt"
fi
cat "$OUT/summary.txt"
# 检查报告目录中是否存在非当前 UID 拥有的文件。
foreign="$(find "$OUT" ! -uid "$(id -u)" | head -5)"
if [ -n "$foreign" ]; then
  echo "✗ 报告目录里有不属于你的文件（用 podman unshare rm 删）：$foreign" | tee -a "$OUT/summary.txt"
  status=1
fi

host_state >"$OUT/host-after.txt"
if cmp -s "$OUT/host-before.txt" "$OUT/host-after.txt"; then
  echo "✓ 宿主机：默认程序、桌面文件、缩略图缓存、Anyview 设置均未变化" | tee -a "$OUT/summary.txt"
else
  echo "✗ 宿主机状态有变化（见 host-before.txt、host-after.txt）：" | tee -a "$OUT/summary.txt"
  diff "$OUT/host-before.txt" "$OUT/host-after.txt" | head -20 | tee -a "$OUT/summary.txt"
  status=1
fi
if podman container exists "$NAME"; then
  echo "✗ 容器没删掉：$NAME" | tee -a "$OUT/summary.txt"
  status=1
fi
echo "报告：$OUT"
exit "$status"
