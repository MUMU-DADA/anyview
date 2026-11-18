#!/bin/bash
# 在 anyview-desktops / anyview-flatpak 容器内运行（由 run.sh 启动，请勿直接在宿主机运行）。
#   inside.sh            （root）准备系统服务；原生模式下像安装包那样把 Anyview（程序、缩略图生成器、图标、图片类型）
#                        放进容器的系统目录；再以普通用户依次跑 kde、gnome
#   inside.sh desktop D  （普通用户）准备一个全新的主目录，在自己的 D-Bus 里起桌面 D，做检查
# MODE=native（默认）：检查编好的 target/release/anyview；MODE=flatpak：给测试用户装上 Anyview.flatpak，
# 用 flatpak run 检查，另加沙盒专有的几项（单实例、壁纸门户、“打开方式”对话框、各种格式）。
# 容器用完即删：这里装的、写的都只在容器里，只有 /out（报告目录）留下。
set -uo pipefail
MODE="${MODE:-native}"
APP_ID="local.anyview.Anyview"

if [ "${1:-}" != desktop ]; then
  if [ "$MODE" = native ]; then
    # 安装包会做的：程序 /usr/bin/anyview，缩略图生成器 /usr/share/thumbnailers/，图标 /usr/share/icons/hicolor/。
    install -m 755 /opt/anyview/anyview /usr/bin/anyview
    install -D -m 644 /opt/anyview/data/thumbnailers/local.anyview.Anyview.thumbnailer \
      /usr/share/thumbnailers/local.anyview.Anyview.thumbnailer
    cp -r /opt/anyview/data/icons/hicolor /usr/share/icons/
    # 自带的图片类型（PAM、CLIP）。
    install -D -m 644 /opt/anyview/data/mime/local.anyview.Anyview.xml /usr/share/mime/packages/local.anyview.Anyview.xml
    update-mime-database /usr/share/mime
    gtk4-update-icon-cache -q -t -f /usr/share/icons/hicolor 2>/dev/null || true
  fi
  DESKTOPS="${DESKTOPS:-kde gnome}"
  for d in $DESKTOPS; do
    install -d -o "$HOST_USER" -m 700 "/home/tester-$d"
  done
  # 系统的账户服务（AccountsService）在容器里一启动就崩溃，而 Flatpak 装、运行程序前都要问它（家长控制）；
  # 去掉它的启动登记（只在这个容器里），Flatpak 找不到这个服务时按“没有家长控制”处理。
  rm -f /usr/share/dbus-1/system-services/org.freedesktop.Accounts.service
  if [ "$MODE" = flatpak ]; then
    # 给两个测试用户装上 Anyview.flatpak（主目录在内存盘上，容器删掉就没了）。
    # 运行时已在镜像的系统级 Flatpak 里；容器不联网，--no-deps 不去 Flathub 核对运行时（安装包里写着从那里装）。
    for d in $DESKTOPS; do
      runuser -u "$HOST_USER" -- env -i PATH=/usr/bin:/bin HOME="/home/tester-$d" bash -c "
        mkdir -p /out/$d
        flatpak install --user --noninteractive --no-deps --bundle /opt/anyview/Anyview.flatpak >/out/$d/flatpak-install.log 2>&1
      " || echo "✗ $d：装不上 Anyview.flatpak（见 $d/flatpak-install.log）"
    done
  fi
  # 桌面要用的系统服务：系统总线（GNOME、KDE 都连）、machine-id。容器里没有 systemd：删掉装包时留下的
  # /run/systemd/seats，GNOME 才不去找 logind（它按这个目录判断有没有 systemd），改用自带的替代实现。
  dbus-uuidgen --ensure
  mkdir -p /run/dbus
  dbus-daemon --system --fork
  rm -rf /run/systemd/seats
  # /out 仅以普通用户身份写入，确保宿主机可直接管理输出文件。
  status=0
  for d in $DESKTOPS; do
    # 每个桌面最多 10 分钟（正常 3～5 分钟）：卡住时到点结束，接着检查下一个桌面。
    timeout -k 10 600 runuser -u "$HOST_USER" -- env -i PATH=/usr/bin:/bin LANG="${LANG:-zh_CN.UTF-8}" MODE="$MODE" \
      bash /harness/inside.sh desktop "$d"
    rc=$?
    if [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
      runuser -u "$HOST_USER" -- bash -c "echo '✗ $d：过了 10 分钟还没检查完，强行结束了（卡在哪一步见 $d/run.log 最后几行）' >>/out/$d/summary.txt"
    fi
    [ "$rc" = 0 ] || status=1
    # 这个桌面留下的程序（桌面、门户、文件管理器等）全部结束，下一个桌面从干净的状态开始。
    pkill -KILL -u "$HOST_USER"
    sleep 1
  done
  exit "$status"
fi

# ---- 普通用户：一个桌面 ----
D="$2"
OUT="/out/$D"
export HOME="/home/tester-$D"
FIRST_PASS=0
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
  FIRST_PASS=1
  mkdir -p "$OUT"
  exec >"$OUT/run.log" 2>&1
fi
export XDG_RUNTIME_DIR="$HOME/.run"
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
# 缩略图缓存目录先建好（真实系统里早就有）：KDE 的 KIO 先查它所在的文件系统，目录不在就不做缩略图。
mkdir -p "$HOME/.cache/thumbnails"
PICS="$HOME/Pictures/测试图片"
mkdir -p "$PICS"
cp -n /fixtures/zall/* "$PICS/"

if [ "$MODE" = flatpak ]; then
  # 系统里的 /etc/profile.d/flatpak.sh 平时做的：让桌面找得到 Flatpak 导出的桌面文件、图标、图片类型。
  export XDG_DATA_DIRS="$HOME/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share"
  # 安装由上面的 root 部分做好了。
  if [ "$FIRST_PASS" = 1 ] && ! flatpak info --user "$APP_ID" >/dev/null 2>&1; then
    echo "✗ $D：装不上 Anyview.flatpak（见 $D/flatpak-install.log）" | tee -a "$OUT/summary.txt"
    exit 1
  fi
  # /out（报告目录）是测试才有的路径，另外放行；图片都在主目录里，用的是安装包自己的权限。
  AV=(flatpak run --filesystem=/out "$APP_ID")
else
  AV=(anyview)
fi

if [ "$FIRST_PASS" = 1 ]; then
  exec dbus-run-session -- bash /harness/inside.sh desktop "$D"
fi

FAILED=0
say() { echo "$*" | tee -a "$OUT/summary.txt"; }
log() { echo "[$(date +%T)] $*"; }
# 每次运行 Anyview 都限时：卡住时到点结束它（这一项没有结果、算失败），后面的检查照常做。
#   av 秒数 参数…      前台运行，等它结束
#   av_bg 秒数 参数…   后台运行，$! 是它的进程号（用 wait_pid 等）
av() {
  local limit="$1" rc
  shift
  timeout -k 5 "$limit" "${AV[@]}" "$@"
  rc=$?
  if [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
    echo "    （超时：Anyview $* 过了 $limit 秒还没结束，结束了它）" >>"$OUT/summary.txt"
    pkill -x anyview
  fi
  return "$rc"
}
av_bg() {
  local limit="$1"
  shift
  timeout -k 5 "$limit" "${AV[@]}" "$@" &
}
# 等后台的一个程序结束，最多 $2 秒（不带参数的 wait 会连桌面一起等，永远等不完）；到点没结束就结束它、返回 1。
wait_pid() {
  local _
  for _ in $(seq 1 $(($2 * 2))); do
    kill -0 "$1" 2>/dev/null || return 0
    sleep 0.5
  done
  kill "$1" 2>/dev/null
  return 1
}
# 自测报告里 Anyview 最近显示过的提示（最后一步的 lastNotice）：提示只显示 2 秒，门户的结果过一会儿才回来，
# 到最后一步截图时提示可能已经没了。
last_notice() {
  python3 -c "import json,sys; s=json.load(open(sys.argv[1])).get('steps', []); print((s[-1].get('lastNotice') if s else '') or '')" "$1" 2>/dev/null
}
# 替用户回答门户还开着的对话框（相当于点“取消”或按 Esc），输出回答了几个。
#   门户后端（KDE、GNOME 各自的 org.freedesktop.impl.portal.desktop.*）弹的：对它的请求调用 Close。
#   GNOME 的询问对话框（“允许 … 吗？”）由 GNOME Shell 自己弹，请求挂在它的连接上（org.gnome.Shell）；GNOME Shell 51
#   的 Close 有错（参数顺序反了：“invocation.get_sender is not a function”），改为像用户那样按 Esc
#   （mutter 的远程桌面接口，press-key-gnome.py）。按完请求不在了才算回答了。
answer_portal_dialogs() {
  local name path n=0
  for name in $(timeout -k 3 20 busctl --user list --no-legend --no-pager 2>/dev/null \
      | awk '($1 ~ /^org\.freedesktop\.impl\.portal\.desktop\./ || $1 == "org.gnome.Shell") && $2 != "-" {print $1}'); do
    for path in $(timeout -k 3 10 busctl --user --timeout=5 tree --list "$name" 2>/dev/null | grep -E '/request/[^/]+/[^/]+$'); do
      if [ "$name" = org.gnome.Shell ]; then
        timeout -k 3 15 python3 /harness/press-key-gnome.py 0xff1b >&2
        sleep 1
        if ! timeout -k 3 10 busctl --user --timeout=5 tree --list "$name" 2>/dev/null | grep -qx "$path"; then
          log "按 Esc 回答了 GNOME Shell 的询问对话框：$path" >&2
          n=$((n + 1))
        fi
      elif timeout -k 3 10 busctl --user --timeout=5 call "$name" "$path" org.freedesktop.impl.portal.Request Close >&2; then
        log "关掉了门户的对话框：$name $path" >&2
        n=$((n + 1))
      fi
    done
  done
  echo "$n"
}

if [ "$D" = kde ]; then
  export XDG_CURRENT_DESKTOP=KDE XDG_SESSION_TYPE=wayland
else
  export XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=gnome
fi
# 先告诉 D-Bus 这是哪个桌面、画面在哪（套接字名先定好），再起桌面：门户等服务由 D-Bus 按需启动，
# 启动时按 XDG_CURRENT_DESKTOP 选后端（KDE、GNOME 各自的），起早了就选错（选成 GTK 的、还找不到画面）。
dbus-update-activation-environment XDG_CURRENT_DESKTOP XDG_SESSION_TYPE HOME XDG_RUNTIME_DIR LANG \
  ${XDG_SESSION_DESKTOP:+XDG_SESSION_DESKTOP} ${XDG_DATA_DIRS:+XDG_DATA_DIRS} WAYLAND_DISPLAY=wayland-av
if [ "$D" = kde ]; then
  export KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1
  # KWin 本身不能带 QT_QPA_PLATFORM=wayland（它会把自己当成客户端去连别的显示服务器，然后崩溃）；桌面起来后再给别的程序设。
  kwin_wayland --virtual --width 1600 --height 900 --no-lockscreen --socket wayland-av >"$OUT/kwin.log" 2>&1 &
  export WAYLAND_DISPLAY=wayland-av
  shot() { timeout -k 3 20 spectacle --background --nonotify --fullscreen --output "$OUT/$1.png" >>"$OUT/screenshot.log" 2>&1; }
  FILES=dolphin
else
  gnome-shell --headless --no-x11 --virtual-monitor 1600x900 --wayland-display wayland-av >"$OUT/gnome-shell.log" 2>&1 &
  export WAYLAND_DISPLAY=wayland-av
  shot() { timeout -k 3 20 python3 /harness/shot-gnome.py "$OUT/$1.png" >>"$OUT/screenshot.log" 2>&1; }
  FILES=nautilus
fi
for _ in $(seq 1 150); do
  [ -S "$XDG_RUNTIME_DIR/wayland-av" ] && break
  sleep 0.2
done
if [ ! -S "$XDG_RUNTIME_DIR/wayland-av" ]; then
  say "✗ $D：桌面没起来（见 $D/*.log）"
  exit 1
fi
sleep 4
log "桌面起来了"
# GNOME 起来时停在“活动”概览里，这时没有窗口有焦点；系统的询问对话框只给有焦点的程序弹（GNOME Shell 的
# accessDialog.js 按 focus_app 核对，没有焦点时直接出错）。像用户点一下桌面那样收起概览，之后开的窗口才拿到焦点。
hide_overview() {
  [ "$D" = gnome ] || return 0
  timeout -k 3 10 gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
    --method org.freedesktop.DBus.Properties.Set org.gnome.Shell OverviewActive '<false>' >/dev/null 2>&1
}
hide_overview
if [ "$D" = kde ]; then
  # 这几个也要等 KWin 起来再设：KWin 带着 KDE_FULL_SESSION、KDE_SESSION_VERSION 启动会崩溃。
  export QT_QPA_PLATFORM=wayland KDE_FULL_SESSION=true KDE_SESSION_VERSION=6
  # KDE 的程序登记（哪种类型用哪个程序打开）要先建好缓存。
  timeout -k 3 60 kbuildsycoca6 --noincremental >"$OUT/kbuildsycoca.log" 2>&1
  # 桌面和面板（壁纸由它管，KDE 的壁纸门户要找它；要有桌面外壳 plasma-desktop）。镜像里有才起（anyview-flatpak 里有）。
  # 它的消息默认写进系统日志（容器里没有），改写到文件。
  if command -v plasmashell >/dev/null; then
    QT_FORCE_STDERR_LOGGING=1 plasmashell --no-respawn >"$OUT/plasmashell.log" 2>&1 &
    sleep 6
    pgrep -x plasmashell >/dev/null && log "KDE 的桌面（plasmashell）起来了" || log "KDE 的桌面（plasmashell）没起来，见 plasmashell.log"
  fi
fi
# 由 D-Bus 启动的程序（文件管理器、门户等）也要知道桌面在哪。
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_TYPE HOME XDG_RUNTIME_DIR LANG \
  ${XDG_DATA_DIRS:+XDG_DATA_DIRS} ${QT_QPA_PLATFORM:+QT_QPA_PLATFORM} ${KDE_FULL_SESSION:+KDE_FULL_SESSION KDE_SESSION_VERSION}
shot 0-desktop

# 1. Anyview 窗口在这个桌面里的样子。
av_bg 30 "$PICS/quadrants.qoi" >"$OUT/anyview-window.log" 2>&1
sleep 6
shot 1-anyview-window
if pgrep -x anyview >/dev/null; then
  say "✓ $D：Anyview 的窗口开出来了（$MODE，截图 1-anyview-window.png）"
else
  say "✗ $D：Anyview 没开出来（$MODE，见 anyview-window.log）"
  FAILED=1
fi
pkill -x anyview; sleep 1

# 2. 文件管理器的缩略图：打开测试图片文件夹，等它做缩略图，再按缩略图缓存（freedesktop 规范，两个桌面相同）逐个核对。
#    Flatpak 版不带缩略图生成器（Flatpak 导出不了），只记录，不算失败。
if [ "$FILES" = dolphin ]; then
  timeout -k 3 60 dolphin --new-window "$PICS" >"$OUT/dolphin.log" 2>&1 &
else
  timeout -k 3 60 nautilus --new-window "$PICS" >"$OUT/nautilus.log" 2>&1 &
fi
sleep 25
shot 2-file-manager
pkill -x "$FILES"; sleep 1
python3 - "$PICS" "$HOME/.cache/thumbnails" "$D" "$FILES" "$MODE" >>"$OUT/summary.txt" <<'PY' || FAILED=1
import hashlib, os, sys
from gi.repository import Gio
pics, cache, desk, fm, mode = sys.argv[1:6]
made, failed, none = [], [], []
for name in sorted(os.listdir(pics)):
    uri = Gio.File.new_for_path(os.path.join(pics, name)).get_uri()
    h = hashlib.md5(uri.encode()).hexdigest() + ".png"
    if any(os.path.exists(os.path.join(cache, s, h)) for s in ("normal", "large", "x-large", "xx-large")):
        made.append(name)
    elif os.path.isdir(os.path.join(cache, "fail")) and any(
            os.path.exists(os.path.join(cache, "fail", d, h)) for d in os.listdir(os.path.join(cache, "fail"))):
        failed.append(name)
    else:
        none.append(name)
total = len(made) + len(failed) + len(none)
mark = "✓" if made else "✗"
note = "（Flatpak 版不带缩略图生成器，只记录）" if mode == "flatpak" else ""
print(f"{mark} {desk}：{fm} 给 {len(made)}/{total} 个文件做了缩略图{note}")
if failed:
    print(f"    做不了（记在 fail 里）：{' '.join(failed)}")
if none:
    print(f"    没有缩略图：{' '.join(none)}")
sys.exit(0 if made else 1)
PY

# 3. 默认程序。原生版：设为默认看图软件，再像双击那样用桌面打开图片（GNOME：gio open；KDE：kioclient exec），
#    应由 Anyview 打开；再恢复原来的默认程序。事先写一个“原来的默认程序”：PNG 用文件管理器（只为核对恢复），JPEG 不设。
#    Flatpak 版：按钮只提示到系统设置里选、不改设置；再像系统设置那样把 Anyview 设为默认，桌面打开图片应由
#    Flatpak 导出的桌面文件启动 Anyview。
mkdir -p "$HOME/.config"
if [ "$D" = kde ]; then
  ORIGINAL="org.kde.dolphin.desktop"
  LIST="$HOME/.config/kde-mimeapps.list"
  open_file() { timeout -k 3 30 kioclient exec "$1" >>"$OUT/open.log" 2>&1 & }
else
  ORIGINAL="org.gnome.Nautilus.desktop"
  LIST="$HOME/.config/mimeapps.list"
  open_file() { timeout -k 3 30 gio open "$1" >>"$OUT/open.log" 2>&1 & }
fi
printf '[Default Applications]\nimage/png=%s;\n' "$ORIGINAL" >"$LIST"
query() { python3 -c "import configparser,sys; c=configparser.ConfigParser(interpolation=None); c.read(sys.argv[1]); print(c.get('Default Applications', sys.argv[2], fallback=''))" "$LIST" "$1"; }
open_both() {  # 像双击那样打开 PNG、JPEG，记下由 Anyview 打开的
  opened=""
  for f in small_64x48.png photo_4000x3000.jpg; do
    open_file "$PICS/$f"
    for _ in $(seq 1 60); do pgrep -x anyview >/dev/null && break; sleep 0.25; done
    sleep 3
    shot "3-open-$f"
    if pgrep -x anyview >/dev/null; then opened="$opened $f"; fi
    pkill -x anyview; pkill -x "$FILES"; sleep 1
  done
}
SET="settings_open,settings_page:formats,settings_button:formats.buttons_linux:set_default,settings_cancel"
RESTORE="settings_open,settings_page:formats,settings_button:formats.buttons_linux:restore_default,settings_cancel"
av 60 --self-test "$OUT/set-default" --open "$PICS/small_64x48.png" --steps "$SET" >"$OUT/set-default.log" 2>&1
[ "$D" = kde ] && timeout -k 3 60 kbuildsycoca6 >>"$OUT/kbuildsycoca.log" 2>&1
log "设为默认后 PNG：$(query image/png)；JPEG：$(query image/jpeg)"
if [ "$MODE" = flatpak ]; then
  report="$(python3 -c "import json,sys; print(' '.join(s.get('report') or '' for s in json.load(open(sys.argv[1])).get('steps', [])))" "$OUT/set-default/selftest.json" 2>/dev/null)"
  if [[ "$report" == *Flatpak* && "$(query image/png)" == "$ORIGINAL;" ]]; then
    say "✓ $D：Flatpak 版按“设为默认看图软件”提示到系统设置里选，没改默认程序"
  else
    say "✗ $D：Flatpak 版“设为默认看图软件”不对：提示“$report”，PNG 现在是 $(query image/png)"
    FAILED=1
  fi
  # 像系统设置那样把 Anyview 设为 PNG、JPEG 的默认程序。
  printf '[Default Applications]\nimage/png=%s.desktop;\nimage/jpeg=%s.desktop;\n' "$APP_ID" "$APP_ID" >"$LIST"
  [ "$D" = kde ] && timeout -k 3 60 kbuildsycoca6 >>"$OUT/kbuildsycoca.log" 2>&1
  open_both
  if [ "$opened" = " small_64x48.png photo_4000x3000.jpg" ]; then
    say "✓ $D：在系统设置里选 Anyview 后，桌面打开 PNG、JPEG 都由 Flatpak 版 Anyview 打开（导出的桌面文件）"
  else
    say "✗ $D：在系统设置里选 Anyview 后，桌面用 Anyview 打开的只有：${opened:-无}（见 3-open-*.png、open.log）"
    FAILED=1
  fi
else
  open_both
  if [ "$opened" = " small_64x48.png photo_4000x3000.jpg" ]; then
    say "✓ $D：设为默认看图软件后，桌面打开 PNG、JPEG 都由 Anyview 打开（PNG 原来是 $ORIGINAL）"
  else
    say "✗ $D：设为默认看图软件后，桌面用 Anyview 打开的只有：${opened:-无}（见 3-open-*.png、open.log）"
    FAILED=1
  fi
  av 60 --self-test "$OUT/restore-default" --open "$PICS/small_64x48.png" --steps "$RESTORE" >"$OUT/restore-default.log" 2>&1
  [ "$D" = kde ] && timeout -k 3 60 kbuildsycoca6 >>"$OUT/kbuildsycoca.log" 2>&1
  log "恢复后 PNG：$(query image/png)；JPEG：$(query image/jpeg)"
  after="$(cat "$LIST")"
  if [[ "$after" == *"image/png=$ORIGINAL"* && "$after" != *local.anyview* ]] \
    && [ ! -e "$HOME/.local/share/applications/local.anyview.Anyview.desktop" ]; then
    say "✓ $D：恢复原来的默认程序后 PNG 回到 $ORIGINAL，Anyview 的桌面文件删掉了"
  else
    say "✗ $D：恢复后不对：$(echo "$after" | tr '\n' ' ')"
    FAILED=1
  fi
fi
cp -r "$HOME/.config" "$OUT/config-after" 2>/dev/null

if [ "$MODE" = flatpak ]; then
  # 4. 各种格式在 Flatpak 里都能打开（运行时里的库，如 HEIC 要的 libheif）：从第一张起翻完测试图片文件夹。
  count="$(ls "$PICS" | wc -l)"
  steps="$(printf 'next_image,%.0s' $(seq 2 "$count"))"
  av 300 --self-test "$OUT/formats" --open "$PICS/$(ls "$PICS" | sort | head -1)" --steps "${steps%,}" >"$OUT/formats.log" 2>&1
  python3 - "$OUT/formats/selftest.json" "$D" "$count" >>"$OUT/summary.txt" <<'PY' || FAILED=1
import json, sys
path, desk, count = sys.argv[1], sys.argv[2], int(sys.argv[3])
try:
    steps = json.load(open(path)).get("steps", [])
except Exception as e:
    print(f"✗ {desk}：Flatpak 版翻测试图片没有结果（{e}）")
    sys.exit(1)
seen = {s.get("name"): s for s in steps if s.get("name")}
bad = sorted(n for n, s in seen.items() if not s.get("format") or s.get("notice"))
formats = sorted({s.get("format") for s in seen.values() if s.get("format")})
mark = "✓" if len(seen) == count and not bad else "✗"
print(f"{mark} {desk}：Flatpak 版翻完 {len(seen)}/{count} 张测试图片，格式：{' '.join(formats)}")
if bad:
    print("    打不开或有提示：" + "；".join(f"{n}（{seen[n].get('notice')}）" for n in bad))
sys.exit(0 if mark == "✓" else 1)
PY

  # 5. 单实例：勾上“不允许多个实例”后再打开一张，应交给开着的窗口、自己马上退出。
  #    Flatpak 每次启动的运行时目录各自独立，套接字要放在同一应用共用的 app/<应用 ID>/ 里才找得到。
  printf 'version = 1\n\n[general]\nsingle_instance = true\n' >"$OUT/single-instance.toml"
  av_bg 40 --settings "$OUT/single-instance.toml" "$PICS/small_64x48.png" >"$OUT/single-1.log" 2>&1
  sleep 6
  started=$(date +%s%N)
  timeout -k 5 20 flatpak run "$APP_ID" --settings "$OUT/single-instance.toml" "$PICS/quadrants.qoi" >"$OUT/single-2.log" 2>&1
  second=$?
  spent=$(( ($(date +%s%N) - started) / 1000000 ))
  sleep 2
  shot 5-single-instance
  windows="$(pgrep -cx anyview)"
  if [ "$second" = 0 ] && [ "$windows" = 1 ]; then
    say "✓ $D：Flatpak 版单实例：第二次打开交给了开着的窗口，自己 ${spent} ms 后退出（截图 5-single-instance.png 应是 quadrants.qoi）"
  else
    say "✗ $D：Flatpak 版单实例不对：第二次返回 $second、用了 ${spent} ms，开着的 Anyview $windows 个"
    FAILED=1
  fi
  pkill -x anyview; sleep 1

  # 6. 设为桌面图片：Flatpak 里用系统的壁纸门户。前后各读一次桌面的壁纸设置，应变了。
  wallpaper_now() {
    if [ "$D" = gnome ]; then
      gsettings get org.gnome.desktop.background picture-uri 2>/dev/null
    else
      grep -h '^Image=' "$HOME/.config/plasma-org.kde.plasma.desktop-appletsrc" 2>/dev/null | tail -1
    fi
  }
  before="$(wallpaper_now)"
  # 第一次：权限库里还没有 Anyview 的记录，门户先弹窗问用户允不允许（不显示预览时），截下这个对话框。
  # 要在 Anyview 退出前替用户回答（这里是取消：KDE 关掉对话框，GNOME 按 Esc）：xdg-desktop-portal 1.22 等回答时一直锁着这个请求，
  # 发请求的程序一退出，它的主循环就去拿这把锁、整个卡住，之后用门户的程序（GTK 程序启动时就要读主题设置）全都卡住。
  ANYVIEW_SELFTEST_RUN_EXTERNAL=wallpaper av_bg 60 --self-test "$OUT/wallpaper-ask" --open "$PICS/photo_4000x3000.jpg" \
    --steps "set_wallpaper,wait:12000" >"$OUT/wallpaper-ask.log" 2>&1
  ask_pid=$!
  sleep 2
  hide_overview
  sleep 3
  shot 6-wallpaper-ask
  closed="$(answer_portal_dialogs)"
  wait_pid "$ask_pid" 30 || pkill -x anyview
  sleep 1
  ask_notice="$(last_notice "$OUT/wallpaper-ask/selftest.json")"
  log "第一次（替用户取消）：关掉的对话框 ${closed:-0} 个，Anyview 的提示「$ask_notice」"
  if [ "${closed:-0}" -ge 1 ] 2>/dev/null; then
    say "✓ $D：Flatpak 版第一次设为桌面图片时，系统弹窗问是否允许（截图 6-wallpaper-ask.png）；测试替用户取消了，Anyview 提示「$ask_notice」"
  else
    say "✗ $D：Flatpak 版第一次设为桌面图片时，没找到系统的询问对话框（见 6-wallpaper-ask.png、wallpaper-ask.log）"
    FAILED=1
  fi
  # 像用户在对话框里点了“允许”那样记进权限库（刚才取消时门户记的是“不允许”，这里改掉），再设一次：这次应直接设好。
  timeout -k 3 20 flatpak permission-set wallpaper wallpaper "$APP_ID" yes >>"$OUT/wallpaper.log" 2>&1
  ANYVIEW_SELFTEST_RUN_EXTERNAL=wallpaper av 60 --self-test "$OUT/wallpaper" --open "$PICS/photo_4000x3000.jpg" \
    --steps "set_wallpaper,wait:5000" >>"$OUT/wallpaper.log" 2>&1
  notice="$(last_notice "$OUT/wallpaper/selftest.json")"
  current="$(wallpaper_now)"
  shot 6-wallpaper
  log "壁纸：提示「$notice」，桌面的壁纸：$before → $current"
  if [[ "$notice" == *已设为桌面图片* || "$notice" == *"desktop picture"* ]] && [ -n "$current" ] && [ "$current" != "$before" ]; then
    say "✓ $D：Flatpak 版设为桌面图片（壁纸门户）：提示「$notice」，桌面壁纸是 $current"
  else
    say "✗ $D：Flatpak 版设为桌面图片不对：提示「$notice」，桌面壁纸：${current:-（读不到）}（见 wallpaper.log）"
    FAILED=1
  fi

  # 7. 用图像编辑器打开：Flatpak 里弹出系统的“打开方式”对话框（截图核对），不报错。
  ANYVIEW_SELFTEST_RUN_EXTERNAL=editor av_bg 60 --self-test "$OUT/editor" --hold-ms 8000 --open "$PICS/photo_4000x3000.jpg" \
    --steps "open_in_editor" >"$OUT/editor.log" 2>&1
  editor_pid=$!
  sleep 5
  shot 7-open-with
  # 截图后替用户取消：核对确实弹出了对话框，也看取消时 Anyview 不报错。
  closed="$(answer_portal_dialogs)"
  wait_pid "$editor_pid" 30 || pkill -x anyview
  notice="$(last_notice "$OUT/editor/selftest.json")"
  if [ "${closed:-0}" -ge 1 ] 2>/dev/null && [[ "$notice" != *失败* && "$notice" != *无法* ]]; then
    say "✓ $D：Flatpak 版用图像编辑器打开：弹出系统的“打开方式”对话框（截图 7-open-with.png），取消时不报错"
  else
    say "✗ $D：Flatpak 版用图像编辑器打开不对：关掉的对话框 ${closed:-0} 个，提示「$notice」（见 7-open-with.png、editor.log）"
    FAILED=1
  fi

  # 8. 文件管理器的缩略图：在 设置 › 格式关联 里按“让文件管理器用 Anyview 做缩略图”（自测的步骤，同用户点按钮），
  #    应在用户目录登记缩略图生成器；清掉缩略图缓存、重开文件管理器，看原本只有 Anyview 能做缩略图的格式现在有没有：
  #    KDE 的 Dolphin 直接运行缩略图生成器，应做出来；GNOME 的 Nautilus 把它关在自己的沙盒里，用不了（只记录）。
  #    最后按“取消登记”，文件应删掉。
  TB="settings_open,settings_page:formats,settings_button:formats.buttons_flatpak_thumbnails"
  REGISTERED="$HOME/.local/share/thumbnailers/$APP_ID.thumbnailer"
  reports_of() {
    python3 -c "import json,sys; print(' '.join(s.get('report') or '' for s in json.load(open(sys.argv[1])).get('steps', [])))" "$1" 2>/dev/null
  }
  av 60 --self-test "$OUT/thumbnailer-register" --open "$PICS/small_64x48.png" \
    --steps "$TB:register_thumbnailer,settings_cancel" >"$OUT/thumbnailer-register.log" 2>&1
  report="$(reports_of "$OUT/thumbnailer-register/selftest.json")"
  cp "$REGISTERED" "$OUT/registered.thumbnailer" 2>/dev/null
  if [[ "$report" == *已登记* || "$report" == *Registered* ]] && grep -q '^X-Anyview-Flatpak=true$' "$REGISTERED" 2>/dev/null \
    && grep -q "^Exec=flatpak run --filesystem=/tmp $APP_ID --thumbnail %i %o %s$" "$REGISTERED" \
    && [ -x "$(sed -n 's/^TryExec=//p' "$REGISTERED")" ]; then
    say "✓ $D：Flatpak 版按“让文件管理器用 Anyview 做缩略图”后，登记了缩略图生成器（提示「$report」，内容见 registered.thumbnailer）"
  else
    say "✗ $D：Flatpak 版登记缩略图生成器不对：提示「$report」，文件 $REGISTERED $( [ -e "$REGISTERED" ] && echo 在 || echo 不在)（见 thumbnailer-register.log）"
    FAILED=1
  fi
  rm -rf "$HOME/.cache/thumbnails" && mkdir -p "$HOME/.cache/thumbnails"
  [ "$D" = kde ] && timeout -k 3 60 kbuildsycoca6 >>"$OUT/kbuildsycoca.log" 2>&1
  if [ "$FILES" = dolphin ]; then
    timeout -k 3 80 dolphin --new-window "$PICS" >"$OUT/dolphin-flatpak-thumbnailer.log" 2>&1 &
  else
    timeout -k 3 80 nautilus --new-window "$PICS" >"$OUT/nautilus-flatpak-thumbnailer.log" 2>&1 &
  fi
  sleep 40
  shot 8-flatpak-thumbnailer
  pkill -x "$FILES"; sleep 1
  python3 - "$PICS" "$HOME/.cache/thumbnails" "$D" >>"$OUT/summary.txt" <<'PY' || FAILED=1
import hashlib, os, sys
from gi.repository import Gio
pics, cache, desk = sys.argv[1:4]
# 系统自己（GNOME 的 glycin、KDE 的插件）一般做不了、要靠 Anyview 的：DDS、JPEG XR、PCX、PSD/PSB、各家 RAW、CLIP、PAM。
ours = (".dds", ".jxr", ".pcx", ".psd", ".psb", ".nef", ".cr2", ".orf", ".rw2", ".raf", ".cr3", ".dng", ".x3f", ".mrw", ".clip", ".pam")
names = sorted(n for n in os.listdir(pics) if n.lower().endswith(ours))
def has(name):
    h = hashlib.md5(Gio.File.new_for_path(os.path.join(pics, name)).get_uri().encode()).hexdigest() + ".png"
    return any(os.path.exists(os.path.join(cache, s, h)) for s in ("normal", "large", "x-large", "xx-large"))
made = [n for n in names if has(n)]
missing = [n for n in names if n not in made]
if desk == "kde":
    # KDE 自己有插件的格式先用自己的（旋转的 DNG 由 KDE 的插件管，原生安装时这个容器里也没做出来），留 2 个余量。
    ok = len(made) >= len(names) - 2
    mark = "✓" if ok else "✗"
    print(f"{mark} {desk}：登记后 Dolphin 给 {len(made)}/{len(names)} 个要靠 Anyview 的文件做了缩略图（截图 8-flatpak-thumbnailer.png）")
else:
    ok = True
    print(f"  {desk}：登记后 Nautilus 给 {len(made)}/{len(names)} 个要靠 Anyview 的文件做了缩略图（GNOME 把缩略图生成器关在自己的沙盒里，用不了，只记录）")
if missing:
    print(f"    没有缩略图：{' '.join(missing)}")
sys.exit(0 if ok else 1)
PY
  av 60 --self-test "$OUT/thumbnailer-unregister" --open "$PICS/small_64x48.png" \
    --steps "$TB:unregister_thumbnailer,settings_cancel" >"$OUT/thumbnailer-unregister.log" 2>&1
  report="$(reports_of "$OUT/thumbnailer-unregister/selftest.json")"
  if [[ "$report" == *已取消登记* || "$report" == *Unregistered* ]] && [ ! -e "$REGISTERED" ]; then
    say "✓ $D：按“取消登记”后缩略图生成器删掉了（提示「$report」）"
  else
    say "✗ $D：取消登记不对：提示「$report」，文件 $REGISTERED $( [ -e "$REGISTERED" ] && echo 还在 || echo 不在)"
    FAILED=1
  fi
fi
exit "$FAILED"
