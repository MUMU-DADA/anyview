#!/bin/bash
# Linux 无头自测：在无头 sway 中启动应用，依次做三项检查，结果写到 reports/linux/<时间>/：
#   empty  空态：截图并核对背景色 #202020
#   menu   右键菜单：弹出后用合成器截图
#   nav    浏览：打开 sort/1.png，依次 下一张、下一张、末张、首张，核对自然排序的顺序
#   photo  大图：打开 1200 万像素 JPEG，记录打开到出图的耗时
#   view   看图操作：旋转、翻转、缩放、平移、显示方式、像素边缘，逐步截图并核对画面
#   overlay 悬浮界面：锁定控制栏、图片信息、进度条跳转、透明图片的棋盘格
#   browse 浏览：降序、随机与退回、下一个/上一个文件夹、到达末张时的询问
#   files  文件：开着反相复制到剪贴板（剪贴板里应是原图）、删除（移到回收站；回收站放在报告目录里，不留在系统中）
#   default_viewer 设为默认看图软件：写进报告目录里的 mimeapps.list，再恢复原来的默认程序
#   monitor_icc 使用监视器的 ICC 设置：给一个线性 Gamma 的"显示器"配置文件，打开、关掉设置时核对画面颜色
#   thumbnailer 缩略图生成器：anyview --thumbnail 给 QOI、PSB、PCX、CLIP、JPEG XR、RAW、SVGZ 生成 PNG，坏文件报错
#   quality 高品质：Lanczos 精确重绘（旋转、翻转后仍对齐）、减淡+锐化、反相、Gamma
#   color  色彩：带 ICC 的图片转成 sRGB 显示（灰度 128 按线性配置文件应约为 188）
#   preview 快速预览：600 万像素 JPEG 先显示 EXIF 内嵌的缩略图，再换成清晰图
#   window 窗口跟着图片：缩放不改窗口、旋转跟着转、最大化后切图保持、模拟拖动窗口、切图后回到图片大小（只记录）
#   settings 设置窗口（docs/SETTINGS.md）：逐页截图；改背景色、隐藏控制栏和倍率提示、透明背景、Esc、语言，
#          缓存大小填错时"确定"校验失败；生效后核对画面、按键和设置文件；再打开时放弃修改
#   mapping 键盘、鼠标映射：关掉 PageDown 那一行，Q 映射为放大、→ 映射为缩小，滚轮向下放大、右键下一张、
#          单击两侧翻页；确定后逐个按键、点鼠标，核对执行的动作
#   instance 多个实例：没勾"不允许多个实例"时再启动一个 anyview 是另一个窗口（它自己做完自测退出）；
#          勾上后再打开的图片交给这个窗口，新启动的那个马上退出
#   anim   动图：GIF 播放、空格暂停、逐帧前后、APNG、GIF 透明处的背景色（文件里的品红，再改成白色）、
#          隐藏动画信息控制
#   archive 压缩包：ZIP 里翻页、到末尾询问、下一个压缩包到 7Z、RAR，再打开 ZIP 回到上次的那一页，
#          从压缩包里复制到图片文件夹，清掉记住的位置
#   pages  智能双页阅览：两张竖图一组、横图单独、往回翻、左←右、关掉
#   fileops 文件操作：复制（重名加序号）、复制到图片文件夹 2、编辑器、桌面图片、打印（只记录不真的做）、
#          移动到图片文件夹后显示下一张；文件夹里加、删文件后自动刷新
#   picker 选择图片窗口：打开时选中当前图片、缩略图带生成缩略图；单击别的行主窗口不切换；按大小排序后选择不变；
#          隐藏、再显示缩略图导航；确定后切过去；再打开、取消不切换；压缩包里也能用
#   effects 翻页效果和音效：淡出、滑动、翻页（往前、往后）各停在一半截图，按颜色核对两张叠在一起的画面；
#          最后真的翻一页，等效果做完再核对；记下翻页音效（自测时不出声）
#   formats 更多格式：formats/（PSD、PSB、DDS、PCX）、modern/（EXR、Radiance HDR、JPEG XL、SVG、AVIF、HEIC）、raw/（各家 RAW 的内嵌预览）
#          逐张打开核对画面；AVIF、HEIC 要系统里有 libheif
#   background 背景图片：设好后窗口四周（和关掉图片后的空态）是背景图片；帮助窗口（键盘快捷键、支持的格式）；
#          像另一个窗口（进程）那样直接改设置文件，这个窗口跟着变；最后关掉背景图片，四周是新的背景色
#   settings_reload 用上一项写出的设置文件重新启动：设置都还在
#   settings_broken 设置文件损坏：备份旧文件、用默认值、启动后提示一次
#   real   真实图片（tests/fixtures/real，开放许可，用 tools/testdata/fetch.py 下载）：逐张以 100% 打开并核对
#   perf   性能参考测试：用真实高像素照片测冷启动、切图、精确重绘、空闲占用、扫描 1 万个文件
# 用法：
#   bash apps/linux/scripts/selftest.sh [报告目录]
# 环境变量：GSK_RENDERER（默认 cairo，纯软件渲染，结果稳定）、ANYVIEW_PROFILE（默认 release）
set -euo pipefail

REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
STAMP="$(date +%Y-%m-%dT%H-%M-%S)"
OUT="${1:-$REPO/reports/linux/$STAMP}"
PROFILE="${ANYVIEW_PROFILE:-release}"
FIXTURES="$REPO/tests/fixtures/generated"
mkdir -p "$OUT"
cd "$REPO"

if [ "$PROFILE" = "release" ]; then
  cargo build -q --release -p anyview-linux
else
  cargo build -q -p anyview-linux
fi
BIN="$REPO/target/$PROFILE/anyview"
if [ ! -f "$FIXTURES/sort/1.png" ] || [ ! -f "$FIXTURES/basic/photo_4000x3000.jpg" ] \
  || [ ! -f "$FIXTURES/view/tall_400x3000.png" ] || [ ! -f "$FIXTURES/color/linear_gray_128.png" ] \
  || [ ! -f "$FIXTURES/preview/thumbnail_3000x2000.jpg" ] || [ ! -f "$FIXTURES/color/linear_monitor.icc" ] \
  || [ ! -f "$FIXTURES/zthumbs/quadrants.qoi" ] || [ ! -f "$FIXTURES/limits/b_65mb.svgz" ] \
  || [ ! -f "$FIXTURES/modern/hdr_quadrants.hdr" ] || [ ! -f "$FIXTURES/modern/avif_pq.avif" ] \
  || [ ! -f "$FIXTURES/generation/comfy.png" ]; then
  cargo run -q --release -p anyview-fixtures -- --out "$FIXTURES"
fi

RUNTIME="$(mktemp -d /tmp/anyview-xdg.XXXXXX)"
SWAY_PID=""
cleanup() {
  if [ -n "$SWAY_PID" ]; then kill "$SWAY_PID" 2>/dev/null || true; fi
  rm -rf "$RUNTIME"
}
trap cleanup EXIT

env -u WAYLAND_DISPLAY -u DISPLAY \
  XDG_RUNTIME_DIR="$RUNTIME" WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
  sway --config "$REPO/apps/linux/scripts/sway-headless.conf" >"$OUT/sway.log" 2>&1 &
SWAY_PID=$!

SOCK=""
for _ in $(seq 1 50); do
  SOCK="$(cd "$RUNTIME" && ls wayland-* 2>/dev/null | grep -v '\.lock$' | head -n1 || true)"
  [ -n "$SOCK" ] && break
  sleep 0.1
done
if [ -z "$SOCK" ]; then
  echo "sway 没有启动，见 $OUT/sway.log"
  exit 1
fi

export XDG_RUNTIME_DIR="$RUNTIME" WAYLAND_DISPLAY="$SOCK" GDK_BACKEND=wayland
export GSK_RENDERER="${GSK_RENDERER:-cairo}"
unset DISPLAY

FAILED=0
# 每次运行程序都限时：程序自己的自测 120 秒到点就算失败（TIME_LIMIT），但卡在初始化界面之前（如系统的门户卡住）时
# 它的计时还没开始；外面再加 180 秒，到点结束它，这一项算失败，后面照常检查。
LIMIT=180
# 浏览、压缩包、文件这几项查的是到达首尾时"询问"的那种做法（默认是直接回到首张）：先把设置写成"询问"。
ask_at_ends() {  # ask_at_ends <名字>
  mkdir -p "$OUT/$1"
  printf 'version = 2\n[general]\nat_boundary = "ask"\nat_archive_boundary = "ask"\n' >"$OUT/$1/settings.toml"
}
run_check() {  # run_check <名字> <应用参数…>
  local name="$1" rc
  shift
  mkdir -p "$OUT/$name"
  # 脚本开着 set -e：失败的返回值要接住，不然这一项失败时整个脚本就停了，后面的都不查、也不报。
  rc=0
  timeout -k 5 "$LIMIT" "$BIN" --self-test "$OUT/$name" "$@" >"$OUT/$name/app.log" 2>&1 || rc=$?
  if [ "$rc" = 0 ]; then
    echo "✓ $name"
  elif [ "$rc" = 124 ] || [ "$rc" = 137 ]; then
    echo "✗ $name（过了 $LIMIT 秒还没结束，结束了它；见 $OUT/$name/）"
    FAILED=1
  else
    echo "✗ $name（见 $OUT/$name/）"
    FAILED=1
  fi
}

# 额外保持 1.5 秒，用合成器再截一张图，看实际显示（包括窗口外的弹出层）。
run_with_compositor_shot() {  # run_with_compositor_shot <名字> <应用参数…>
  local name="$1"
  shift
  mkdir -p "$OUT/$name"
  timeout -k 5 "$LIMIT" "$BIN" --self-test "$OUT/$name" --hold-ms 1500 "$@" >"$OUT/$name/app.log" 2>&1 &
  local pid=$!
  for _ in $(seq 1 100); do
    [ -f "$OUT/$name/selftest.json" ] && break
    sleep 0.1
  done
  sleep 0.3
  grim "$OUT/$name/compositor.png" 2>>"$OUT/sway.log" || echo "grim 截图失败（见 sway.log）"
  if wait "$pid"; then echo "✓ $name"; else echo "✗ $name（见 $OUT/$name/）"; FAILED=1; fi
}

# 空态：核对背景色。
run_with_compositor_shot empty
# 右键菜单：弹出后截图（compositor.png），菜单内容同时写在 selftest.json 的 contextMenu 中。
run_with_compositor_shot menu --open "$FIXTURES/view/quadrants_600x400.png" --steps context_menu

run_check nav --open "$FIXTURES/sort/1.png" --steps next,next,last,first
run_check photo --open "$FIXTURES/basic/photo_4000x3000.jpg"
VIEW_STEPS="rotate_cw,flip_horizontal,rotate_none,zoom_in,zoom_in,pan_down,edge_none,edge_soften,fit_window"
VIEW_STEPS="$VIEW_STEPS,next_image,fit_width,pan_down,rotate_ccw,flip_vertical,zoom_out,zoom_original,rotate_180"
run_check view --open "$FIXTURES/view/quadrants_600x400.png" --steps "$VIEW_STEPS"
run_check overlay --open "$FIXTURES/basic/transparent_800x600.png" \
  --steps "toggle_bar_lock,toggle_info,bar_jump:0.5,bar_jump:0.17,toggle_info,toggle_bar_lock"
ask_at_ends browse
run_check browse --open "$FIXTURES/sort/1.png" \
  --steps "sort_desc,first_image,sort_desc,random_forward,random_back,next_container,prev_container,last_image,next_image,first_image"

QUALITY_STEPS="zoom_in,interp_lanczos,wait_exact,rotate_cw,wait_exact,flip_horizontal,wait_exact"
# 切图前锁定缩放（150%）：新图片保持这个倍率，一定需要精确重绘（不受窗口跟着图片变大小影响）。
QUALITY_STEPS="$QUALITY_STEPS,edge_soften_sharpen,wait_exact,invert,wait_exact,gamma,wait_exact,zoom_lock,next_image,wait_exact"
QUALITY_STEPS="$QUALITY_STEPS,zoom_lock,invert,gamma,interp_bilinear,edge_soften"
run_check quality --open "$FIXTURES/view/quadrants_600x400.png" --steps "$QUALITY_STEPS"
run_check color --open "$FIXTURES/color/linear_gray_128.png"
run_check preview --open "$FIXTURES/preview/thumbnail_3000x2000.jpg"
run_check window --open "$FIXTURES/basic/photo_4000x3000.jpg" \
  --steps "zoom_in,rotate_cw,maximize,next_image,maximize,window:700x500,next_image,fullscreen,next_image,fullscreen"

SETTINGS_STEPS="settings_open,settings_page:browse,settings_set:browse.background_color=#336699"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_set:browse.hide_control_bar=true,settings_set:browse.hide_zoom_info=true"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_page:image,settings_set:image.cache_mb=50,settings_set:image.alpha_background=white"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_ok_invalid,settings_set:image.cache_mb=512,settings_page:general"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_set:general.esc_quits=false,settings_page:language,settings_set:language.language=en"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_ok,zoom_out,hover_bottom,key:Escape,settings_open,settings_page:mouse"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_set:mouse.double_click_maximize=false,settings_cancel,settings_open,settings_page:misc"
SETTINGS_STEPS="$SETTINGS_STEPS,settings_page:formats,settings_page:keyboard,settings_page:bookmarks,settings_cancel"
# 系统语言设成中文，设置里再改成英文，核对界面换了语言。
LANGUAGE=zh_CN:zh run_check settings --open "$FIXTURES/basic/transparent_800x600.png" --steps "$SETTINGS_STEPS"
LANGUAGE=zh_CN:zh run_check settings_reload --open "$FIXTURES/basic/transparent_800x600.png" \
  --settings "$OUT/settings/settings.toml" --steps "settings_open,settings_cancel,zoom_out"
MAPPING_STEPS="settings_open,settings_page:keyboard,settings_set:keyboard.mappings#0=off"
MAPPING_STEPS="$MAPPING_STEPS,settings_set:keyboard.mappings#2=on|Q|zoom_in,settings_set:keyboard.mappings#3=on|Right|zoom_out"
MAPPING_STEPS="$MAPPING_STEPS,settings_page:mouse,settings_set:mouse.wheel_down=on|zoom_in,settings_set:mouse.right=on|next_image"
MAPPING_STEPS="$MAPPING_STEPS,settings_set:mouse.click_sides=true,settings_ok,key:Q,key:Right,key:PageDown,mouse:wheel_down"
MAPPING_STEPS="$MAPPING_STEPS,mouse:right,mouse:left:0.1,mouse:double:0.9,mouse:double:0.5"
run_check mapping --open "$FIXTURES/view/quadrants_600x400.png" --steps "$MAPPING_STEPS"
INSTANCE_STEPS="spawn_separate:$FIXTURES/sort/1.png,settings_open,settings_page:general"
INSTANCE_STEPS="$INSTANCE_STEPS,settings_set:general.single_instance=true,settings_ok,spawn_open:$FIXTURES/sort/2.png"
run_check instance --open "$FIXTURES/view/quadrants_600x400.png" --steps "$INSTANCE_STEPS"
mkdir -p "$OUT/archive/copies"
ARCHIVE_STEPS="next_image,last_image,next_image,next_container,next_image,next_container"
ARCHIVE_STEPS="$ARCHIVE_STEPS,open:$FIXTURES/archives/a_book.cbz,settings_open,settings_page:misc"
ARCHIVE_STEPS="$ARCHIVE_STEPS,settings_set:misc.folder1=$OUT/archive/copies,settings_ok,copy_to_folder1"
ARCHIVE_STEPS="$ARCHIVE_STEPS,settings_open,settings_page:bookmarks,settings_button:bookmarks.clear_archive:clear,settings_cancel"
ask_at_ends archive
LANGUAGE=zh_CN:zh run_check archive --open "$FIXTURES/archives/a_book.cbz" --steps "$ARCHIVE_STEPS"
# 被安全上限拦下的文件：翻到解压后 65 MB 的 SVGZ 时先问，选"仍然打开"后显示出来。
run_check limits --open "$FIXTURES/limits/a.png" --steps "next_image,limit_open"
PICKER_STEPS="picker,picker_select:4,picker_sort:sort_size,picker_hide:on,picker_hide:off,picker_ok"
PICKER_STEPS="$PICKER_STEPS,picker,picker_select:0,picker_cancel,open:$FIXTURES/archives/a_book.cbz,picker,picker_select:2,picker_ok"
LANGUAGE=zh_CN:zh run_check picker --open "$FIXTURES/sort/2.png" --steps "$PICKER_STEPS"
EFFECTS_STEPS="settings_open,settings_page:image,settings_set:image.page_effect=fade,settings_page:misc"
EFFECTS_STEPS="$EFFECTS_STEPS,settings_set:misc.page_sound=true,settings_ok,transition_hold:0.5,next_image,transition_hold:off"
EFFECTS_STEPS="$EFFECTS_STEPS,settings_open,settings_page:image,settings_set:image.page_effect=slide,settings_ok"
EFFECTS_STEPS="$EFFECTS_STEPS,transition_hold:0.5,next_image,transition_hold:off,settings_open,settings_page:image"
EFFECTS_STEPS="$EFFECTS_STEPS,settings_set:image.page_effect=page,settings_ok,transition_hold:0.5,prev_image,transition_hold:off,prev_image"
LANGUAGE=zh_CN:zh run_check effects --open "$FIXTURES/effects/1_red.png" --steps "$EFFECTS_STEPS"
FORMATS_STEPS="$(printf 'next_image,%.0s' $(seq 15))open:$FIXTURES/modern/avif_anim.avif,$(printf 'next_image,%.0s' $(seq 18))"
FORMATS_STEPS="${FORMATS_STEPS}open:$FIXTURES/raw/a_tiff.nef$(printf ',next_image%.0s' $(seq 8))"
run_check formats --open "$FIXTURES/formats/dds_bc7_dx10.dds" --steps "$FORMATS_STEPS"
# 生成信息：ComfyUI 的 PNG 上按 P 打开面板，往后翻到 WebP、JPEG（EXIF 里的 A1111 参数），再到没有生成信息的 plain.png。
GENERATION_STEPS="toggle_info,key:P,generation:state,next_image,generation:state,next_image,generation:state"
GENERATION_STEPS="$GENERATION_STEPS,next_image,next_image,next_image,generation:state,generation:close"
LANGUAGE=zh_CN:zh run_check generation --open "$FIXTURES/generation/comfy.png" --steps "$GENERATION_STEPS"
BACKGROUND_STEPS="settings_open,settings_page:browse,settings_set:browse.background_image=true"
BACKGROUND_STEPS="$BACKGROUND_STEPS,settings_set:browse.background_image_path=$FIXTURES/effects/2_blue.png,settings_ok,wait:600"
BACKGROUND_STEPS="$BACKGROUND_STEPS,close_image,help:shortcuts,help:formats,edit_settings_file:browse.background_color=\"#336699\""
BACKGROUND_STEPS="$BACKGROUND_STEPS,settings_open,settings_page:browse,settings_set:browse.background_image=false,settings_ok"
# 开源许可放在最后，前面各步的序号不变。
BACKGROUND_STEPS="$BACKGROUND_STEPS,help:licenses"
LANGUAGE=zh_CN:zh run_check background --open "$FIXTURES/sort/1.png" --steps "$BACKGROUND_STEPS"
run_check pages --open "$FIXTURES/pages/01.png" \
  --steps "key:7,next_image,next_image,prev_image,prev_image,key:6,key:6"
mkdir -p "$OUT/fileops/work" "$OUT/fileops/f1" "$OUT/fileops/f2"
cp "$FIXTURES/sort/1.png" "$FIXTURES/sort/2.png" "$FIXTURES/sort/10.png" "$OUT/fileops/work/"
FILEOPS_STEPS="settings_open,settings_page:misc,settings_set:misc.folder1=$OUT/fileops/f1"
FILEOPS_STEPS="$FILEOPS_STEPS,settings_set:misc.folder2=$OUT/fileops/f2,settings_set:misc.editor=/usr/bin/true"
FILEOPS_STEPS="$FILEOPS_STEPS,settings_set:misc.no_screensaver=true,settings_ok,copy_to_folder1,copy_to_folder1"
FILEOPS_STEPS="$FILEOPS_STEPS,copy_to_folder2,open_in_editor,set_wallpaper,print,move_to_folder1"
FILEOPS_STEPS="$FILEOPS_STEPS,add_file:$FIXTURES/sort/中文.png,remove_file:10.png"
LANGUAGE=zh_CN:zh run_check fileops --open "$OUT/fileops/work/1.png" --steps "$FILEOPS_STEPS"
ANIM_STEPS="wait_frames:3,key:Space,wait:300,key:Period,key:Comma,next_image,wait_frames:2,next_image,key:Space"
ANIM_STEPS="$ANIM_STEPS,settings_open,settings_page:image,settings_set:image.gif_background=white,settings_ok"
ANIM_STEPS="$ANIM_STEPS,settings_open,settings_page:browse,settings_set:browse.hide_animation_info=true,settings_ok"
run_check anim --open "$FIXTURES/anim/a_frames.gif" --steps "$ANIM_STEPS"
mkdir -p "$OUT/settings_broken"
printf 'version = 1\n[general\nconfirm_delete = false\n' >"$OUT/settings_broken/settings.toml"
LANGUAGE=zh_CN:zh run_check settings_broken --open "$FIXTURES/basic/transparent_800x600.png" \
  --steps "settings_open,settings_cancel"

REAL="$REPO/tests/fixtures/real"
if [ -f "$REAL/photo_12mp_iguana.jpg" ]; then
  run_check real --open "$REAL/gif_ozone.gif" --sample-at-100 \
    --steps "zoom_100,zoom_lock,toggle_info,next_image,next_image,next_image,next_image,next_image,next_image,next_image,next_image,next_image"
  # 性能：10 份相同的 1200 万像素照片（a–j）、另一个文件夹里的 x，以及 1 万个小文件的文件夹。
  if [ "$(ls "$FIXTURES/many" 2>/dev/null | wc -l)" -lt 10000 ]; then
    cargo run -q --release -p anyview-fixtures -- --out "$FIXTURES" --many 10000 >/dev/null
  fi
  mkdir -p "$OUT/perf/work" "$OUT/perf/other"
  # 10 份：预读默认往后 5 张，跳到末张时它不在预读范围里，测得的是未缓存切图耗时。
  for name in a b c d e f g h i j; do cp "$REAL/photo_12mp_iguana.jpg" "$OUT/perf/work/$name.jpg"; done
  cp "$REAL/photo_12mp_iguana.jpg" "$OUT/perf/other/x.jpg"
  PERF_STEPS="wait:1500,bar_jump:1.0,wait:1500,prev_image,interp_lanczos,wait_exact,interp_bilinear,idle:3000"
  PERF_STEPS="$PERF_STEPS,open:$OUT/perf/other/x.jpg,open:$FIXTURES/many"
  run_check perf --open "$OUT/perf/work/a.jpg" --sample-at-100 --steps "$PERF_STEPS"
else
  echo "（没有真实测试图片，跳过 real、perf：先运行 python3 -I tools/testdata/fetch.py）"
fi

# 文件操作在报告目录里的副本上做；回收站也指向报告目录（XDG_DATA_HOME），不碰系统回收站。
mkdir -p "$OUT/files/work" "$OUT/files/data"
# 删掉 2.png 后在末张再往后：这里也按"询问"查。
ask_at_ends files
cp "$FIXTURES/sort/1.png" "$FIXTURES/sort/2.png" "$FIXTURES/sort/10.png" "$OUT/files/work/"
if XDG_DATA_HOME="$OUT/files/data" timeout -k 5 "$LIMIT" "$BIN" --self-test "$OUT/files" --open "$OUT/files/work/2.png" \
  --steps "invert,copy_to_clipboard,invert,delete_file,next_image,first_image,delete_file" >"$OUT/files/app.log" 2>&1; then
  echo "✓ files"
else
  echo "✗ files（见 $OUT/files/）"
  FAILED=1
fi

# 设为默认看图软件：mimeapps.list、桌面文件、记录都放在报告目录里（XDG 环境变量），不改你的系统设置。
# 事先写一个 mimeapps.list：PNG 原来用 eog，恢复时要换回去；JPEG 原来没有设，恢复时这一项要去掉。
DV="$OUT/default_viewer/xdg"
mkdir -p "$DV/config"
printf '[Default Applications]\nimage/png=org.gnome.eog.desktop;\ntext/plain=org.kde.kate.desktop;\n' >"$DV/config/mimeapps.list"
DV_STEPS="settings_open,settings_page:formats,settings_button:formats.buttons_linux:set_default"
DV_STEPS="$DV_STEPS,settings_button:formats.buttons_linux:restore_default,settings_button:formats.buttons_linux:restore_default,settings_cancel"
XDG_DATA_HOME="$DV/data" XDG_CONFIG_HOME="$DV/config" XDG_STATE_HOME="$DV/state" \
  run_check default_viewer --open "$FIXTURES/sort/1.png" --steps "$DV_STEPS"
python3 - "$OUT/default_viewer/selftest.json" "$DV" "$BIN" <<'PY' || FAILED=1
import json, os, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
dv = [s for s in steps if "defaultViewer" in s]
problems = []
if len(dv) != 3:
    problems.append(f"按钮的记录不是 3 条：{len(dv)}")
else:
    done, back, again = dv
    d = done["defaultViewer"]
    if d["png"] != "local.anyview.Anyview.desktop;org.gnome.eog.desktop;" or d["jpeg"] != "local.anyview.Anyview.desktop;":
        problems.append(f"设为默认后 PNG、JPEG 的默认程序不对：{d['png']}、{d['jpeg']}")
    if d["zip"] is not None:
        problems.append(f"压缩包不应设为默认：{d['zip']}")
    if not d["desktopFile"] or not d["icon"] or not d["record"] or sys.argv[3] not in (d["exec"] or ""):
        problems.append(f"桌面文件或记录不对：{d}")
    b = back["defaultViewer"]
    if b["png"] != "org.gnome.eog.desktop;" or b["jpeg"] is not None or b["desktopFile"] or b["icon"] or b["record"]:
        problems.append(f"恢复后不对：{b}")
    if "没有改过" not in (again.get("report") or "") and "hasn" not in (again.get("report") or ""):
        problems.append(f"再按恢复应提示不用恢复：{again.get('report')}")
text = open(os.path.join(sys.argv[2], "config", "mimeapps.list")).read()
if "org.kde.kate.desktop" not in text or "local.anyview" in text:
    problems.append(f"mimeapps.list 最后不对：{text!r}")
if problems:
    print("✗ 设为默认看图软件：" + "；".join(problems))
    sys.exit(1)
print(f"✓ 设为默认看图软件：{dv[0]['report']}（桌面文件和图标都放好了）；恢复后 PNG 换回 eog、JPEG 去掉、桌面文件和图标删掉；再按一次提示不用恢复")
PY

# 使用监视器的 ICC 设置：用环境变量给一个线性 Gamma 的"显示器"配置文件（真的 X11 桌面从根窗口读，
# Wayland 由桌面负责），打开设置后灰度 128 应转成约 55，关掉后回到 128。
MI_STEPS="settings_open,settings_page:image,settings_set:image.monitor_icc=true,settings_ok,wait:400"
MI_STEPS="$MI_STEPS,settings_open,settings_page:image,settings_set:image.monitor_icc=false,settings_ok,wait:400"
ANYVIEW_TEST_MONITOR_ICC="$FIXTURES/color/linear_monitor.icc" \
  run_check monitor_icc --open "$FIXTURES/color/gray_128.png" --steps "$MI_STEPS"
python3 - "$OUT/monitor_icc/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
centers = [s.get("samples", {}).get("centerColor") for s in steps]
# 第 0 步打开，第 5 步是打开设置后的 wait，第 10 步是关掉后的 wait。
def gray(c):
    return int(c[1:3], 16) if c else -1
picked = [centers[i] if i < len(centers) else None for i in (0, 5, 10)]
before, on, off = (gray(c) for c in picked)
if not (126 <= before <= 130 and 52 <= on <= 58 and 126 <= off <= 130):
    print(f"✗ 监视器 ICC：画面中心应为 128 → 约 55 → 128，实际 {picked}（各步 {centers}）")
    sys.exit(1)
print(f"✓ 监视器 ICC：打开设置后灰度 128 按线性 Gamma 的显示器配置文件显示为 {picked[1]}（约 55），关掉后回到 {picked[2]}")
PY

# 缩略图生成器：anyview --thumbnail 给系统一般不认识的格式生成 PNG（不启动界面）。
mkdir -p "$OUT/thumbnailer"
THUMB_OK=1
for f in zthumbs/quadrants.qoi formats/psb_rgb8_rle.psb formats/pcx_24bit.pcx modern/clip_canvas.clip modern/jxr_still.jxr raw/b_canon.cr2 \
  modern/svgz_quadrants.svgz; do
  if ! timeout -k 5 60 "$BIN" --thumbnail "$FIXTURES/$f" "$OUT/thumbnailer/$(basename "$f").png" 64 >>"$OUT/thumbnailer/log.txt" 2>&1; then
    THUMB_OK=0
  fi
done
if ! timeout -k 5 60 "$BIN" --thumbnail "$FIXTURES/broken/empty.png" "$OUT/thumbnailer/broken.png" 64 >>"$OUT/thumbnailer/log.txt" 2>&1 \
  && [ "$THUMB_OK" = 1 ]; then
  python3 - "$OUT/thumbnailer" <<'PY' || FAILED=1
import os, struct, sys, zlib
d = sys.argv[1]
names = sorted(n for n in os.listdir(d) if n.endswith(".png"))
sizes = []
for n in names:
    data = open(os.path.join(d, n), "rb").read()
    w, h = struct.unpack(">II", data[16:24])
    sizes.append(f"{n[:-4]} {w}x{h}")
    if max(w, h) > 64:
        print(f"✗ 缩略图生成器：{n} 太大 {w}x{h}")
        sys.exit(1)
if len(names) != 7:
    print(f"✗ 缩略图生成器：只生成了 {len(names)} 张：{names}")
    sys.exit(1)
print("✓ 缩略图生成器：7 种格式都生成了 PNG（" + "、".join(sizes) + "），坏文件报错")
PY
else
  echo "✗ 缩略图生成器（见 $OUT/thumbnailer/log.txt）"
  FAILED=1
fi

# 浏览顺序：自然排序应为 1, 2, 10, …, 中文, 1。
python3 - "$OUT/nav/selftest.json" <<'PY' || FAILED=1
import json, sys
report = json.load(open(sys.argv[1]))
names = [s["name"] for s in report.get("steps", [])]
expected = ["1.png", "2.png", "10.png", "中文.png", "1.png"]
if names != expected:
    print(f"✗ 浏览顺序不对：{names}，应为 {expected}")
    sys.exit(1)
print("✓ 浏览顺序：" + " → ".join(names))
PY
python3 - "$OUT/view/selftest.json" <<'PY' || FAILED=1
import json, sys
report = json.load(open(sys.argv[1]))
steps = report.get("steps", [])
checked = sum(s.get("samples", {}).get("checked", 0) for s in steps)
bad = [s["step"] for s in steps if s.get("samples", {}).get("mismatches")]
print(f"  看图操作：{len(steps)} 步，画面核对 {checked} 个点" + (f"，不符：{bad}" if bad else "，全部相符"))
if report.get("failure"):
    print(f"  失败原因：{report['failure']}")
PY
python3 - "$OUT/overlay/selftest.json" <<'PY' || FAILED=1
import json, sys
report = json.load(open(sys.argv[1]))
steps = report.get("steps", [])
names = [s["name"] for s in steps]
expected = ["transparent_800x600.png"] * 3 + ["plain_640x480.bmp"] + ["palette_320x240.gif"] * 3
if names != expected:
    print(f"✗ 进度条跳转的结果不对：{names}")
    sys.exit(1)
info = next((s["overlays"]["infoText"] for s in steps if s.get("overlays", {}).get("infoText")), [])
print("✓ 悬浮界面：控制栏锁定、进度条跳到 4/7 和 2/7、图片信息：" + " | ".join(info))
PY
python3 - "$OUT/browse/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
names = [s["name"] for s in steps]
expected = ["1.png", "1.png", "中文.png", "中文.png", None, "中文.png", "quadrants_600x400.png", "1.png", "中文.png", "中文.png", "1.png"]
ok = len(names) == len(expected) and all(e is None or e == n for e, n in zip(expected, names))
ok = ok and names[4] not in ("中文.png", "1.png") and steps[9].get("boundary") == "last"
if not ok:
    print(f"✗ 浏览结果不对：{names}")
    sys.exit(1)
print("✓ 浏览：降序首张 中文.png，随机到 " + names[4] + " 再退回，下一个文件夹 view/，末张询问后回到首张")
PY
python3 - "$OUT/files/selftest.json" "$OUT/files" <<'PY' || FAILED=1
import json, os, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
names = [s["name"] for s in steps]
work = sorted(os.listdir(os.path.join(sys.argv[2], "work")))
trash = sorted(os.listdir(os.path.join(sys.argv[2], "data", "Trash", "files"))) if os.path.isdir(os.path.join(sys.argv[2], "data", "Trash", "files")) else []
gone = [s.get("deletedGone") for s in steps if s.get("deletedGone") is not None]
ok = names == ["2.png", "2.png", "2.png", "2.png", "10.png", "10.png", "1.png", "10.png"] and work == ["10.png"] and trash == ["1.png", "2.png"] and gone == [True, True]
if not ok:
    print(f"✗ 文件操作不对：{names}，剩下 {work}，回收站 {trash}，{gone}")
    sys.exit(1)
clip = next((s["clipboard"] for s in steps if "clipboard" in s), {})
inverted = clip.get("displayedCenterRGBA") != clip.get("originalCenterRGBA")
if not inverted:
    print(f"✗ 复制前的反相没有生效，核对不了剪贴板里是不是原图：{clip}")
    sys.exit(1)
print("✓ 文件：开着反相复制到剪贴板，剪贴板里是原图（正中 #" + clip["centerRGBA"][:6] + "，画面 #" + clip["displayedCenterRGBA"][:6] + "）、也有文件；两次删除后都显示下一张；回收站里是 " + "、".join(trash))
PY
python3 - "$OUT/quality/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
exact = [s["exact"] for s in steps if "exact" in s]
checked = sum(s.get("samples", {}).get("checked", 0) for s in steps)
if len(exact) != 7:
    print(f"✗ 精确重绘次数不对：{len(exact)}")
    sys.exit(1)
worst = max(e["renderMs"] for e in exact)
print(f"✓ 高品质：精确重绘 {len(exact)} 次（最慢 {worst:.1f} ms），反相、Gamma 后画面核对 {checked} 个点全部相符")
PY
python3 - "$OUT/color/selftest.json" <<'PY' || FAILED=1
import json, sys
step = json.load(open(sys.argv[1]))["steps"][0]
center = step.get("samples", {}).get("centerColor") or "#000000"
value = int(center[1:3], 16)
if not 184 <= value <= 192:
    print(f"✗ 色彩不对：{center}（灰度 128 按线性 ICC 应约为 #BCBCBC）")
    sys.exit(1)
print(f"✓ 色彩：灰度 128 按线性 ICC 显示为 {center}（约 188）")
PY
python3 - "$OUT/preview/selftest.json" <<'PY' || FAILED=1
import json, sys
step = json.load(open(sys.argv[1]))["steps"][0]
if step.get("msToPreview") is None or (step["width"], step["height"]) != (3000, 2000):
    print(f"✗ 快速预览没有出现：{step}")
    sys.exit(1)
print(f"✓ 快速预览：{step['msToPreview']:.1f} ms 出预览，{step['msToImage']:.0f} ms 换为清晰图（3000x2000）")
PY
python3 - "$OUT/window/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
sizes = []
for s in steps:
    w = s.get("windowPx") or [0, 0]
    sizes.append(f"{s['step']} {w[0]}x{w[1]}")
print("  窗口跟着图片（Linux 只记录，无头 sway 不一定照办）：" + " → ".join(sizes))
# 左键拖动：图片完整显示时移动窗口，放大后平移图片。
drags = [s.get("leftDragMovesWindow") for s in steps]
if len(drags) < 2 or drags[0] is not True or drags[1] is not False:
    print(f"✗ 左键拖动的判断不对：{drags}")
    sys.exit(1)
print("✓ 左键拖动：图片完整显示时移动窗口，放大后平移图片（" + " ".join("窗口" if d else "平移" for d in drags) + "）")
PY
python3 - "$OUT/settings/selftest.json" "$OUT/settings_reload/selftest.json" "$OUT/settings_broken" <<'PY' || FAILED=1
import json, os, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
by = lambda name: [s for s in steps if s["step"] == name]
problems = []
invalid = by("settings_ok_invalid")[0]
if invalid.get("applied") or invalid.get("errorPage") != "image" or "100" not in (invalid.get("error") or ""):
    problems.append(f"缓存大小填错时没有拦住：{invalid.get('error')}")
ok = by("settings_ok")[0]
ui = ok.get("uiSettings", {})
if ui.get("backgroundColor") != "#336699" or not ui.get("hideControlBar") or not ui.get("hideZoomInfo") or ui.get("alphaBackground") != "White":
    problems.append(f"生效后的设置不对：{ui}")
if ok.get("lang") != "En":
    problems.append(f"语言没有换成英文：{ok.get('lang')}")
text = ok.get("settingsText") or ""
for needle in ['background_color = "#336699"', "esc_quits = false", "cache_mb = 512", 'language = "en"']:
    if needle not in text:
        problems.append(f"设置文件里没有 {needle}")
zoomed = by("zoom_out")[0]
if zoomed.get("cornerColor") != "#336699":
    problems.append(f"缩小后背景色是 {zoomed.get('cornerColor')}，应为 #336699")
hover = by("hover_bottom")[0]
if hover.get("overlays", {}).get("barShown"):
    problems.append("设置了不显示控制栏，指针移到底部时还是显示了")
escape = by("key:Escape")[0]
if escape.get("action") is not None:
    problems.append(f"关掉“按 ESC 退出”后 Esc 还对应 {escape.get('action')}")
reopened = by("settings_open")[1]
if not any(p.endswith("General") for p in reopened.get("settingsPages", [])):
    problems.append(f"再打开设置时不是英文：{reopened.get('settingsPages')}")
cancel = by("settings_cancel")[0]
if not cancel.get("dirtyBeforeCancel") or cancel.get("open"):
    problems.append(f"放弃修改不对：{cancel}")
reload_steps = json.load(open(sys.argv[2])).get("steps", [])
reload_ui = reload_steps[1].get("uiSettings", {}) if len(reload_steps) > 1 else {}
if reload_ui.get("backgroundColor") != "#336699" or reload_steps[1].get("lang") != "En":
    problems.append(f"重新启动后设置没有保留：{reload_ui}")
if len(reload_steps) > 3 and reload_steps[3].get("cornerColor") != "#336699":
    problems.append(f"重新启动后背景色是 {reload_steps[3].get('cornerColor')}")
broken_dir = sys.argv[3]
broken = json.load(open(os.path.join(broken_dir, "selftest.json"))).get("steps", [])
backups = [f for f in os.listdir(broken_dir) if f.startswith("settings-broken-")]
notice = broken[0].get("notice") if broken else None
if not backups or not notice or "settings-broken-" not in notice:
    problems.append(f"损坏的设置文件没有备份或没有提示：{backups} {notice}")
pages = by("settings_open")[0].get("settingsPages", [])
if not any(p.endswith("一般") for p in pages):
    problems.append(f"第一次打开设置时不是中文（系统语言设成了中文）：{pages}")
shots = sum(1 for s in steps if s.get("settings", {}).get("screenshot"))
if problems:
    for p in problems:
        print(f"✗ 设置：{p}")
    sys.exit(1)
print(f"✓ 设置：{len(pages)} 页（{'、'.join(p.split(' ', 1)[1] for p in pages)}），截图 {shots} 张；"
      "缓存大小填 50 被拦下；背景色、隐藏控制栏和倍率、白色透明背景、Esc 不退出、英文都生效并写进文件；"
      "重新启动后保留；损坏的文件已备份（" + backups[0] + "）并提示")
PY
python3 - "$OUT/mapping/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
expected = [
    (11, "zoom_in", None), (12, "zoom_out", None), (13, None, None), (14, "zoom_in", None),
    (15, "next_image", "tall_400x3000.png"), (16, "prev_image", "quadrants_600x400.png"),
    (17, None, None), (18, "maximize", None),
]
problems = []
for i, action, name in expected:
    s = step(i)
    if s.get("action") != action or (name and s.get("name") != name):
        problems.append(f"第 {i} 步 {s.get('step')}：动作 {s.get('action')}，图片 {s.get('name')}（应为 {action} {name or ''}）")
rows = step(5).get("settings", {}).get("page", {}).get("items", [{}])[0].get("value") or []
if not any("将替代" in r or "Replaces" in r for r in rows):
    problems.append(f"→ 映射为缩小的那一行没有提示将替代下一张：{rows}")
text = step(10).get("settingsText") or ""
if 'key = "Q"' not in text or 'action = "zoom_in"' not in text:
    problems.append("设置文件里没有 Q 的映射")
if problems:
    for p in problems:
        print(f"✗ 映射：{p}")
    sys.exit(1)
print("✓ 映射：Q 放大、→ 缩小（提示将替代下一张）、关掉的 PageDown 不再翻文件夹、滚轮向下放大、右键下一张、"
      "单击左侧翻回上一张、两侧双击不最大化、中间双击最大化；设置文件里有映射")
PY
python3 - "$OUT/anim/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
def anim(i):
    return step(i).get("animation") or {}
problems = []
if step(1).get("framesSeen", 0) < 3 or not anim(1).get("playing"):
    problems.append(f"GIF 没有在播放：{step(1)}")
paused = anim(2).get("index")
if step(2).get("action") != "anim_toggle" or anim(2).get("playing") is not False:
    problems.append("空格没有暂停")
if anim(3).get("index") != paused:
    problems.append(f"暂停后帧还在换：{paused} → {anim(3).get('index')}")
if paused is not None and anim(4).get("index") != (paused + 1) % 4:
    problems.append(f"下一帧不对：{paused} → {anim(4).get('index')}")
if anim(5).get("index") != paused:
    problems.append(f"上一帧不对：{anim(5).get('index')}，应为 {paused}")
if step(6).get("name") != "b_frames.apng" or step(7).get("framesSeen", 0) < 2:
    problems.append("APNG 没有播放")
if step(8).get("name") != "c_transparent.gif" or not step(8).get("samples", {}).get("checked"):
    problems.append("透明 GIF 没有核对画面")
if step(17).get("animationText") is not None or not anim(17):
    problems.append("隐藏动画信息控制后还在显示")
if problems:
    print("✗ 动图：" + "；".join(problems))
    sys.exit(1)
print(f"✓ 动图：GIF 播放、空格暂停（停在第 {paused + 1} 帧）、逐帧前后、APNG 播放；透明处按文件里的背景色（品红）和设置的白色"
      f"核对画面；动画信息控制显示“{step(1).get('animationText')}”，设置后隐藏")
PY
python3 - "$OUT/archive/selftest.json" "$OUT/archive/copies" <<'PY' || FAILED=1
import json, os, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
problems = []
names = [step(i).get("name") for i in (0, 1, 2, 4, 5, 6, 7)]
expected = ["a_book.cbz › 1.png", "a_book.cbz › 2.png", "a_book.cbz › 3.png", "b_book.cb7 › 1.png",
            "b_book.cb7 › 2.png", "c_book.cbr › 1.png", "a_book.cbz › 3.png"]
if names != expected:
    problems.append(f"翻页、换压缩包的顺序不对：{names}")
if step(3).get("boundary") != "last":
    problems.append("ZIP 末尾没有询问")
if step(0).get("deleteEnabled") is not False:
    problems.append("压缩包里的图片能删除")
if not os.path.isfile(os.path.join(sys.argv[2], "3.png")) or "已复制到" not in (step(12).get("notice") or ""):
    problems.append(f"没有从压缩包里复制出来：{step(12).get('notice')}")
if step(11).get("archivePositions", 0) < 3 or step(15).get("archivePositions") != 0:
    problems.append(f"记住的压缩包位置不对：{step(11).get('archivePositions')} → {step(15).get('archivePositions')}")
if problems:
    print("✗ 压缩包：" + "；".join(problems))
    sys.exit(1)
print("✓ 压缩包：ZIP 里翻页（跳过 __MACOSX 和隐藏文件）、末尾询问、下一个压缩包到 7Z（固实）、RAR，"
      "再打开 ZIP 回到上次看的第 3 页；压缩包里不能删除；复制出的 3.png 在图片文件夹里；清掉记住的位置")
PY
python3 - "$OUT/pages/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
got = [(s.get("name"), s.get("width"), s.get("height")) for s in steps]
expected = [("01.png", 300, 400), ("01.png | 02.png", 600, 400), ("03.png", 400, 300), ("04.png | 05.png", 600, 400),
            ("03.png", 400, 300), ("01.png | 02.png", 600, 400), ("01.png | 02.png", 600, 400), ("01.png", 300, 400)]
if got != expected:
    print(f"✗ 双页：{got}")
    sys.exit(1)
print("✓ 双页：01|02 一组 → 03 横图单独 → 04|05 → 往回 03 → 01|02 → 左←右 → 关掉回到单页（画面逐步核对）")
PY
python3 - "$OUT/limits/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
if step(1).get("blocked") is True and step(2).get("name") == "b_65mb.svgz" and step(2).get("width") == 64:
    print("✓ 安全上限：解压后 65 MB 的 SVGZ 先被拦下、询问，选“仍然打开”后显示出来（64x48）")
else:
    print(f"✗ 安全上限：{[(s.get('step'), s.get('name'), s.get('blocked'), s.get('width')) for s in steps]}")
    sys.exit(1)
PY
python3 - "$OUT/fileops/selftest.json" "$OUT/fileops" <<'PY' || FAILED=1
import json, os, sys
report = json.load(open(sys.argv[1]))
steps = report.get("steps", [])
base = sys.argv[2]
def step(i):
    return steps[i] if i < len(steps) else {}
problems = []
f1 = sorted(os.listdir(os.path.join(base, "f1")))
f2 = sorted(os.listdir(os.path.join(base, "f2")))
if f1 != ["1 (2).png", "1 (3).png", "1.png"] or f2 != ["1.png"]:
    problems.append(f"复制、移动的结果不对：f1={f1} f2={f2}")
externals = [(step(i).get("external") or {}).get("what") for i in (11, 12, 13)]
if externals != ["editor", "wallpaper", "print"]:
    problems.append(f"编辑器、桌面图片、打印没有交出：{externals}")
if step(14).get("name") != "2.png":
    problems.append(f"移动后没有显示下一张：{step(14).get('name')}")
if step(15).get("total") != 3 or step(16).get("total") != 2:
    problems.append(f"文件夹变化没有自动刷新：{step(15).get('total')}、{step(16).get('total')}")
if problems:
    print("✗ 文件操作：" + "；".join(problems))
    sys.exit(1)
print(f"✓ 文件操作：复制两次（第二次是“1 (2).png”）、复制到图片文件夹 2、移动后显示下一张；编辑器、桌面图片、打印已交出；"
      f"加一张、删一张后 {step(15).get('ms', 0):.0f} ms、{step(16).get('ms', 0):.0f} ms 内自动刷新")
PY
python3 - "$OUT/background/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
problems = []
if step(6).get("cornerColor") != "#2828C8" or not step(6).get("background"):
    problems.append(f"设好背景图片后四周不是它：{step(6).get('cornerColor')}")
if step(7).get("cornerColor") != "#2828C8":
    problems.append(f"空态的背景不是背景图片：{step(7).get('cornerColor')}")
shortcuts, formats = step(8).get("help") or {}, step(9).get("help") or {}
if shortcuts.get("title") != "键盘快捷键" or (shortcuts.get("rows") or 0) < 20:
    problems.append(f"键盘快捷键窗口不对：{shortcuts}")
if formats.get("title") != "支持的格式" or formats.get("headings") != 5:
    problems.append(f"支持的格式窗口不对：{formats}")
edit = step(10)
if not edit.get("reloaded") or (edit.get("uiSettings") or {}).get("backgroundColor") != "#336699":
    problems.append(f"别的进程改的设置没有跟着变：{edit}")
if step(14).get("cornerColor") != "#336699" or step(14).get("background"):
    problems.append(f"关掉背景图片后四周不是新的背景色：{step(14).get('cornerColor')}")
licenses = step(15).get("help") or {}
if licenses.get("title") != "开源许可" or (licenses.get("textLength") or 0) < 100000 or not licenses.get("anyviewLicense"):
    problems.append(f"开源许可窗口不对：{licenses}")
if problems:
    print("✗ 背景图片等：" + "；".join(problems))
    sys.exit(1)
print(f"✓ 背景图片：图片四周和空态都是背景图片；帮助窗口：键盘快捷键 {shortcuts['rows']} 行、支持的格式 {formats['headings']} 类、"
      f"开源许可 {licenses['textLength'] // 1024} KB；别的窗口改的设置 {edit['reloadedMs']} ms 后跟着变；关掉背景图片后是新的背景色")
PY
python3 - "$OUT/effects/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
problems = []
for i, effect in ((8, "fade"), (15, "slide"), (22, "page")):
    t = step(i).get("samples", {}).get("transition") or {}
    if t.get("effect") != effect or step(i).get("samples", {}).get("mismatches"):
        problems.append(f"{effect} 停在一半的画面不对：{step(i).get('samples')}")
last = step(24)
if last.get("name") != "1_red.png" or not last.get("samples", {}).get("checked"):
    problems.append(f"真的翻一页后画面不对：{last.get('name')} {last.get('samples')}")
sounds = last.get("sounds") or []
if len(sounds) != 4:
    problems.append(f"翻页音效应放 4 次：{sounds}")
if problems:
    print("✗ 翻页效果：" + "；".join(problems))
    sys.exit(1)
points = " ".join(f"{p['got']}" for i in (8, 15, 22) for p in step(i)["samples"]["transition"]["points"])
print(f"✓ 翻页效果：淡出、滑动、翻页停在一半时的颜色都对（{points}）；真的翻一页后画面核对相符；"
      f"翻页音效 4 次（{sounds[0]}）")
PY
python3 - "$OUT/formats/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
rows = [s for s in steps if s.get("format")]
want = {"e_fuji.raf": (720, 1080), "g_rotated.dng": (768, 1024), "i_minolta.mrw": (480, 640), "f_canon.cr3": (1800, 1200)}
problems = [f"{s['name']} 应为 {want[s['name']]}，实际 {s['width']}x{s['height']}"
            for s in rows if s["name"] in want and (s["width"], s["height"]) != want[s["name"]]]
if len(rows) != 44:
    problems.append(f"应打开 44 张，实际 {len(rows)} 张")
# 高动态范围、广色域的 AVIF由核心自己解（按 nclx 换成 sRGB），不交给系统的 libheif：像素格式是核心写的。
pixel = {s["name"]: s.get("pixelFormat") for s in rows}
for name, want in {"avif_pq.avif": "YUV444P10 PQ", "avif_p3.avif": "YUV444P8"}.items():
    if pixel.get(name) != want:
        problems.append(f"{name} 应由核心解（{want}），实际 {pixel.get(name)}")
checked = sum(s.get("samples", {}).get("checked", 0) for s in steps)
if problems:
    print("✗ 更多格式：" + "；".join(problems))
    sys.exit(1)
kinds = sorted({s["format"] for s in rows})
print(f"✓ 更多格式：44 张都显示出来（HDR、P3 的 AVIF 由核心换成 sRGB），画面核对 {checked} 个点；格式：" + "、".join(kinds))
PY
python3 - "$OUT/generation/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def gen(i):
    return (steps[i] if i < len(steps) else {}).get("generation") or {}
problems = []
cabin = "a cozy cabin, warm light, 雪"
want = [(3, "ComfyUI", "a red fox in snow"), (5, "Stable Diffusion WebUI", cabin), (7, "Stable Diffusion WebUI", cabin)]
for i, tool, prompt in want:
    g = gen(i)
    if not g.get("open") or g.get("tool") != tool or g.get("prompt") != prompt or not g.get("enabled"):
        problems.append(f"第 {i} 步（{steps[i].get('name') if i < len(steps) else '?'}）面板不对：{g}")
last = gen(11)
if not last.get("open") or last.get("tool") is not None or last.get("enabled"):
    problems.append(f"没有生成信息的 plain.png：面板应开着、显示“没有”，按 P 不可用：{last}")
if gen(12).get("open"):
    problems.append("面板没有关掉")
info = "\n".join(steps[2].get("overlays", {}).get("infoText", [])) if len(steps) > 2 else ""
if "AI 生成: ComfyUI (按 P 看提示词)" not in info:
    problems.append(f"图片信息里没有 AI 生成那一行：{info!r}")
if problems:
    print("✗ 生成信息：" + "；".join(problems))
    sys.exit(1)
print("✓ 生成信息：按 P 打开面板，ComfyUI 的 PNG、WebP 和 JPEG 的 EXIF 里的 A1111 参数都显示出来，翻到没有生成信息的图片时显示“没有”；图片信息里有 AI 生成那一行")
PY
python3 - "$OUT/picker/selftest.json" <<'PY' || FAILED=1
import json, os, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def step(i):
    return steps[i] if i < len(steps) else {}
def picker(i):
    return step(i).get("picker") or {}
problems = []
first = picker(1)
if first.get("rows") != 9 or first.get("selected") != 1 or first.get("sortLabel") != "按名称排序":
    problems.append(f"打开时不对：{first}")
if not first.get("thumbnailsVisible") or first.get("thumbnailsReady") != first.get("thumbnailsVisible"):
    problems.append(f"缩略图没有生成：{first.get('thumbnailsReady')}/{first.get('thumbnailsVisible')}")
if picker(2).get("selectedName") != "sort/B.png" or step(2).get("index") != 1:
    problems.append(f"单击后主窗口不该切换：{picker(2).get('selectedName')}，主窗口第 {step(2).get('index')} 张")
if picker(3).get("sortLabel") != "按大小排序" or picker(3).get("selectedName") != "sort/B.png":
    problems.append(f"排序后不对：{picker(3).get('sortLabel')} {picker(3).get('selectedName')}")
if picker(4).get("stripVisible") is not False or picker(4).get("hideSetting") is not True:
    problems.append("隐藏缩略图导航没有生效")
if picker(5).get("stripVisible") is not True or picker(5).get("hideSetting") is not False:
    problems.append("再显示缩略图导航没有生效")
if step(6).get("name") != "B.png" or step(6).get("picker") is not None:
    problems.append(f"确定后没有切到 B.png 并关掉窗口：{step(6).get('name')}")
if picker(7).get("selectedName") != "sort/B.png":
    problems.append(f"再打开时没有选中当前图片：{picker(7).get('selectedName')}")
if step(9).get("name") != "B.png" or step(9).get("picker") is not None:
    problems.append(f"取消后不该切换：{step(9).get('name')}")
if picker(11).get("rows") != 3 or not str(picker(11).get("firstName", "")).startswith("a_book.cbz/"):
    problems.append(f"压缩包里的列表不对：{picker(11)}")
chosen = os.path.basename(str(picker(12).get("selectedName", "")))
if step(13).get("name") != f"a_book.cbz › {chosen}":
    problems.append(f"压缩包里确定后不对：{step(13).get('name')}，应为 {chosen}")
if problems:
    print("✗ 选择图片窗口：" + "；".join(problems))
    sys.exit(1)
print(f"✓ 选择图片窗口：打开时选中当前图片，缩略图 {first.get('thumbnailsReady')} 张 {step(1).get('pickerWaitMs', 0)} ms 内生成；"
      "单击只移动选择；按大小排序后选择不变；隐藏、显示缩略图导航；确定切到 B.png；取消不切换；压缩包里切到 " + chosen)
PY
python3 - "$OUT/instance/selftest.json" <<'PY' || FAILED=1
import json, sys
report = json.load(open(sys.argv[1]))
steps = report.get("steps", [])
separate = steps[1] if len(steps) > 1 else {}
forwarded = steps[6] if len(steps) > 6 else {}
children = report.get("children", [])
problems = []
if separate.get("childExit") != 0 or not separate.get("childSelfTestOk"):
    problems.append("没勾时另启动的那个没有自己开窗口做完自测")
if separate.get("name") != "quadrants_600x400.png":
    problems.append(f"没勾时这个窗口的图片变了：{separate.get('name')}")
if forwarded.get("name") != "2.png" or not forwarded.get("forwarded"):
    problems.append(f"勾上后再打开的图片没有交给这个窗口：{forwarded.get('name')}")
if not children or any(c.get("exit") != 0 for c in children):
    problems.append(f"交出图片的进程没有正常退出：{children}")
if problems:
    print("✗ 多个实例：" + "；".join(problems))
    sys.exit(1)
print("✓ 多个实例：没勾“不允许多个实例”时另启动的是另一个窗口；勾上后再打开的 2.png 交给了这个窗口"
      f"（从启动到这个窗口出图 {forwarded.get('msToImage', 0):.0f} ms），新启动的那个马上退出")
PY
if [ -f "$OUT/real/selftest.json" ]; then
python3 - "$OUT/real/selftest.json" <<'PY' || FAILED=1
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
rows = [s for s in steps if "decodeMs" in s]
checked = sum(s.get("samples", {}).get("checked", 0) for s in steps)
print(f"  真实图片（100% 时核对 {checked} 个点）：")
for s in rows:
    extra = "，代理纹理" if s.get("hasProxy") else ""
    print(f"    {s['name']:28} {s['width']}x{s['height']:<6} {s['format']:5} {s['pixelFormat']:7} 解码 {s['decodeMs']:7.0f} ms{extra}")
# 拍摄信息：1200 万像素的相机照片应带相机、镜头、拍摄参数、拍摄时间。
iguana = next((s for s in steps if s.get("name") == "photo_12mp_iguana.jpg" and s.get("overlays", {}).get("infoText")), None)
shooting = iguana["overlays"]["infoText"][4:] if iguana else []
if len(shooting) < 4:
    print(f"✗ 拍摄信息不全：{shooting}")
    sys.exit(1)
print("  ✓ 拍摄信息：" + " | ".join(shooting))
PY
fi
if [ -f "$OUT/perf/selftest.json" ]; then
python3 - "$OUT/perf/selftest.json" <<'PY'
import json, sys
steps = json.load(open(sys.argv[1])).get("steps", [])
def find(step):
    return next((s for s in steps if s["step"] == step or s["step"].startswith(step)), {})
rows = [
    ("冷启动打开 1200 万像素 JPEG（出预览）", find("open").get("previewMsSinceProcessStart"), 300, "ms"),
    ("冷启动打开 1200 万像素 JPEG（清晰图）", find("open").get("msSinceProcessStart"), None, "ms"),
    ("运行中打开新图片（出预览）", find("open:").get("msToPreview"), 100, "ms"),
    ("运行中打开新图片（清晰图）", find("open:").get("msToImage"), None, "ms"),
    ("切到已预加载的图片", find("prev_image").get("msToImage"), 17, "ms"),
    ("切到未缓存的 1200 万像素 JPEG（出预览）", find("bar_jump").get("msToPreview"), 50, "ms"),
    ("切到未缓存的 1200 万像素 JPEG（清晰图）", find("bar_jump").get("msToImage"), 150, "ms"),
    ("静止后精确重绘（Lanczos）", find("wait_exact").get("exact", {}).get("renderMs"), 150, "ms"),
    ("打开 1 万张图片的文件夹（扫描 + 第一张）", [s for s in steps if s["step"].startswith("open:")][-1].get("msToImage") if any(s["step"].startswith("open:") for s in steps) else None, 1000, "ms"),
    ("静止 3 秒的 CPU 占用", find("idle").get("idleCpuPercent"), 1, "%"),
]
print("  性能（参考）：")
for name, value, limit, unit in rows:
    shown = "—" if value is None else f"{value:.1f} {unit}"
    if limit is None:
        print(f"      {name}：{shown}（参考）")
        continue
    mark = "?" if value is None else ("✓" if value <= limit else "△")
    print(f"    {mark} {name}：{shown}（目标 ≤ {limit} {unit}）")
idle = find("idle")
if idle.get("peakRssMb") is not None:
    print(f"    静止时内存：{idle.get('rssMb', 0):.0f} MB，其中图片缓存 {idle.get('cacheMb', 0):.0f} MB（上限 1024 MB）；峰值 {idle['peakRssMb']:.0f} MB（含自测截图）")
PY
fi
python3 - "$OUT/photo/selftest.json" <<'PY' || FAILED=1
import json, sys
step = json.load(open(sys.argv[1]))["steps"][0]
print(f"  大图：{step['width']}x{step['height']} {step['format']}，解码 {step['decodeMs']:.0f} ms，打开到出图 {step['msToImage']:.0f} ms")
PY

mkdir -p "$REPO/reports/linux"
if [ "$OUT" != "$REPO/reports/linux/latest" ]; then
  rm -rf "$REPO/reports/linux/latest"
  cp -r "$OUT" "$REPO/reports/linux/latest"
fi
echo "报告：$OUT"
exit "$FAILED"
