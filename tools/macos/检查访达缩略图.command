#!/bin/bash
# 检查访达的缩略图、按空格的预览有没有用上 Anyview 的扩展。只看、只问系统要缩略图，
# 不登记、不改任何设置，不开窗口、不占屏幕，大约 1 分钟：
# 1. 装在“应用程序”里的 Anyview：版本、两个扩展在不在、签名、隔离标记；系统登记了哪几份 Anyview，
#    两个扩展登记了没有、是不是打开着（pluginkit）；
# 2. 最近一小时系统日志里 Anyview 扩展的记录（检查访达是否启动扩展以及是否存在沙盒拒绝）；
# 3. 若已安装程序的签名检查不通过（例如文件携带扩展属性），确认后清除扩展属性并重新登记两个扩展；
# 4. 通过 QLThumbnailGenerator（2x 缩放，同 Retina 屏）请求各格式缩略图，对比原目录与临时副本：
#    拷到本机临时目录的副本各一次，看四色块的位置对不对；
#    Anyview 的扩展登记着、系统却还把它的类型交给自己的图片扩展时，结束一直开着的缩略图服务（它记着以前的选择，
#    普通信号不理、launchctl 被系统完整性保护拦下），系统用到时自动再启动，再要一次。
# 结果写到 reports/macos/<时间>-ql/。用完删掉本机的临时副本、清掉 Quick Look 的缓存。
# 兼容 macOS 自带的 bash 3.2：变量统一使用 ${name} 写法。
set -u

root="$(cd "$(dirname "$0")/../.." && pwd)"
# 构建产物与报告位于本项目 out/ 与 reports/macos/ 目录。
build="${root}/out/anyview-build"
temporary="${root}/out/tmp"
mkdir -p "${build}" "${temporary}"
export TMPDIR="${temporary}/"
stamp="$(date +%Y-%m-%dT%H-%M-%S)"
results="${root}/reports/macos/${stamp}-ql"
if ! mkdir -p "${results}"; then
    printf '%s\n' "无法建结果目录：${results}" "按回车键关闭这个窗口…"
    read -r _
    exit 1
fi
summary="${results}/summary.txt"
work="${temporary}/anyview-ql-check"
installed="/Applications/Anyview.app"
# 测试图片目录（可通过 ANYVIEW_ZALL_DIR 覆盖，或先用 anyview-fixtures 生成至 tests/fixtures/generated/zall）。
zall="${ANYVIEW_ZALL_DIR:-${root}/tests/fixtures/generated/zall}"
lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
# 覆盖常见非系统原生格式，并附上系统原生 PNG 作为对照。
names="quadrants.qoi clip_canvas.clip jxr_still.jxr pcx_24bit.pcx quadrants.pam psb_rgb8_rle.psb svgz_quadrants.svgz svg_quadrants.svg b_frames.apng avif_anim.avif small_64x48.png"

say() { printf '%s\n' "$*" | tee -a "${summary}"; }
finish() {
    rm -rf "${work}"
    qlmanage -r cache >/dev/null 2>&1
    printf '\n检查报告已写入 reports/macos/%s-ql/。\n' "${stamp}"
    afplay /System/Library/Sounds/Glass.aiff >/dev/null 2>&1 &
    printf '按回车键关闭这个窗口…'
    read -r _
    exit "$1"
}
# 系统的命令有时一直不返回：每个都限时，超时就停掉、记下来。
with_timeout() {  # with_timeout <秒> <命令…>
    local limit="$1" ticks=0 pid
    shift
    "$@" &
    pid=$!
    while kill -0 "${pid}" 2>/dev/null; do
        if (( ticks >= limit * 10 )); then
            kill "${pid}" 2>/dev/null
            sleep 0.5
            kill -9 "${pid}" 2>/dev/null
            echo "（超过 ${limit} 秒还没结束，停掉了：$*）"
            return 124
        fi
        sleep 0.1
        ticks=$(( ticks + 1 ))
    done
    wait "${pid}"
}
# ql-probe 的结果 → "N/M 个有缩略图，没有的：…"
count_probe() {
    awk -F'\t' '/^[^ ]/ && NF >= 2 { file = $1 }
        /^  缩略图/ { total++; if ($0 ~ /ms：[0-9]+x[0-9]+/) ok++; else { failed = failed sep file; sep = "、" } }
        END { if (total > 0) { printf "%d/%d 个有缩略图", ok, total; if (failed != "") printf "，没有的：%s", failed } else printf "没有结果" }' "$1"
}

say "Anyview 访达缩略图检查  ${stamp}"
say "系统：$(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null)"
if [[ ! -d "${installed}" ]]; then
    say "✗ “应用程序”里没有 Anyview：先把 DMG 里的 Anyview 拖进“应用程序”、打开一次，再双击本文件。"
    finish 1
fi
if [[ ! -d "${zall}" ]]; then
    say "✗ 未找到测试图片目录 ${zall}（请先运行 cargo run -p anyview-fixtures -- --out tests/fixtures/generated）"
    finish 1
fi
rm -rf "${work}"
mkdir -p "${work}/zall" || finish 1

# 1. 装好的程序和系统里的登记。
plist="${installed}/Contents/Info.plist"
say "装好的程序：版本 $(plutil -extract CFBundleShortVersionString raw -o - "${plist}" 2>/dev/null)，构建 $(plutil -extract CFBundleVersion raw -o - "${plist}" 2>/dev/null)"
say "  里面的扩展：$(ls "${installed}/Contents/PlugIns" 2>/dev/null | tr '\n' ' ')"
say "  隔离标记（从网上下载的才有）：$(xattr -p com.apple.quarantine "${installed}" 2>/dev/null || echo 没有)"
signature_ok=1
if codesign --verify --deep --strict "${installed}" >"${work}/codesign.txt" 2>&1; then
    say "  签名：完整"
else
    signature_ok=0
    say "  签名：有问题（$(head -n 2 "${work}/codesign.txt" | tr '\n' ' ')）"
fi
# 带附加属性的文件（Finder 信息、资源分支等，签名检查不允许）。
xattr -lr "${installed}" >"${work}/xattr.txt" 2>&1
head -n 400 "${work}/xattr.txt" >>"${results}/xattr.txt"
say "  带附加属性的地方：$(grep -c ': com\.apple\.' "${work}/xattr.txt") 处（$(grep -o ': com\.apple\.[A-Za-z.]*' "${work}/xattr.txt" | sort | uniq -c | tr -s ' ' | tr '\n' ' ')）"
{
    echo "== 系统登记的 Anyview（lsregister）"
    with_timeout 30 "${lsregister}" -dump 2>/dev/null | grep -iE '^path:.*anyview' | sort -u
    for id in local.anyview.Anyview.Thumbnail local.anyview.Anyview.Preview; do
        echo "== pluginkit ${id}（+ 打开了，- 关掉了，空着是默认）"
        with_timeout 15 pluginkit -m -v -i "${id}"
    done
    echo "== 文件类型（样例原件）"
    for name in ${names}; do
        echo "${name}：$(with_timeout 10 mdls -raw -name kMDItemContentType "${zall}/${name}" 2>&1)"
    done
} >"${work}/state.txt" 2>&1
cat "${work}/state.txt" >>"${results}/state.txt"
# lsregister 里程序那一行是"path:   /Applications/Anyview.app (0x1234)"，扩展的路径在 .app/Contents/PlugIns 下面。
apps="$(grep -iE '^path:.*Anyview\.app \(' "${work}/state.txt" | sed -e 's/^path: *//' -e 's/ (0x[0-9a-f]*)$//' | sort -u)"
say "  系统登记的 Anyview：$(printf '%s' "${apps}" | grep -c .) 份（$(printf '%s' "${apps}" | tr '\n' ' ')）"
plugin_state() {  # plugin_state <Bundle ID>：pluginkit 列出的那一行（election 标记、版本、路径），没有就说没有
    local line
    line="$(grep -F "$1(" "${work}/state.txt" | head -n 1 | sed 's/^ *//')"
    printf '%s' "${line:-没有登记}" | cut -c1-200
}
say "  缩略图扩展：$(plugin_state local.anyview.Anyview.Thumbnail)"
say "  预览扩展：$(plugin_state local.anyview.Anyview.Preview)"

# 2. 最近一小时的日志：访达给你看的文件夹做缩略图、预览时，有没有启动 Anyview 的扩展，沙盒有没有拒绝。
say ""
say "== 最近一小时系统日志里 Anyview 扩展的记录（详情见 log-hour.txt）"
with_timeout 120 log show --last 1h --style compact \
    --predicate 'process BEGINSWITH "AnyviewThumbnail" OR process BEGINSWITH "AnyviewPreview" OR eventMessage CONTAINS[c] "local.anyview" OR (process == "kernel" AND eventMessage CONTAINS[c] "anyview")' \
    >"${work}/log-hour.txt" 2>&1
tail -n 2000 "${work}/log-hour.txt" >>"${results}/log-hour.txt"
# 沙盒拒绝只数我们两个扩展自己的（系统日志写作 “AnyviewThumbnail(进程号) deny(1) file-read-data …”）：只找 deny 会把
# 内存管理的 “Denying dirty-tracking opt-in”、Spotlight 读打包时挂上的 “Anyview 0.0.1” 磁盘映像被拒也算进来。
say "  缩略图扩展的记录 $(grep -c 'AnyviewThumbnail\[' "${work}/log-hour.txt") 条，预览扩展的记录 $(grep -c 'AnyviewPreview\[' "${work}/log-hour.txt") 条，沙盒拒绝 $(grep -c -E 'Anyview(Thumbnail|Preview)\([0-9]+\) deny\(' "${work}/log-hour.txt") 条"

# 3. 修：签名检查不过时问一声。
if [[ "${signature_ok}" == 0 ]]; then
    say ""
    printf '%s' "装好的 Anyview 签名检查不过，系统因此不用它的扩展。要修吗？（去掉文件上的附加属性、让系统重新登记；输入 y 再按回车，直接按回车跳过）："
    read -r answer
    if [[ "${answer}" == "y" || "${answer}" == "Y" ]]; then
        say "== 修"
        xattr -cr "${installed}"
        if codesign --verify --deep --strict "${installed}" >"${work}/codesign2.txt" 2>&1; then
            say "  去掉附加属性以后签名：完整"
        else
            say "  去掉附加属性以后签名还是不对：$(head -n 2 "${work}/codesign2.txt" | tr '\n' ' ')"
        fi
        # 取消旧构建副本和废纸篓副本的登记，避免与已安装版本争抢 Bundle ID。
        for old in "${root}/out/macos/Anyview.app" "${HOME}/.Trash/Anyview.app" \
            "${build}/Anyview.app" "${build}/run/Anyview.app"; do
            "${lsregister}" -u "${old}" >/dev/null 2>&1 && say "  让系统忘掉：${old}"
        done
        "${lsregister}" -f "${installed}" >/dev/null 2>&1 && say "  重新登记：${installed}"
        for appex in "${installed}"/Contents/PlugIns/*.appex; do
            with_timeout 15 pluginkit -a "${appex}" >/dev/null 2>&1
        done
        sleep 2
        say "  扩展的登记：缩略图 $(with_timeout 15 pluginkit -m -i local.anyview.Anyview.Thumbnail 2>&1 | grep -c 'local.anyview') 个、预览 $(with_timeout 15 pluginkit -m -i local.anyview.Anyview.Preview 2>&1 | grep -c 'local.anyview') 个"
    fi
fi

# 4. 通过 QLThumbnailGenerator 请求缩略图（分别测试原目录与临时目录副本）。
say ""
say "== 用访达同样的接口要缩略图（每个最多等 5 秒）"
if ! xcrun swiftc -O -parse-as-library "${root}/apps/macos/Tools/QLProbe/QLProbe.swift" \
    -framework QuickLookThumbnailing -o "${work}/ql-probe" >"${work}/build.txt" 2>&1; then
    say "✗ 检查工具编译不过（见 build.txt）"
    cat "${work}/build.txt" >>"${results}/build.txt"
    finish 1
fi
share_files=()
for name in ${names}; do
    cp "${zall}/${name}" "${work}/zall/" 2>/dev/null
    share_files+=("${zall}/${name}")
done
# 让 Quick Look 重新读一遍有哪些扩展（不加 cache 的 -r），再清掉缩略图缓存。
qlmanage -r >/dev/null 2>&1
qlmanage -r cache >/dev/null 2>&1
sleep 2
since="$(date '+%Y-%m-%d %H:%M:%S')"
with_timeout 120 "${work}/ql-probe" 5 "${share_files[@]}" </dev/null >"${work}/probe-share.txt" 2>&1
with_timeout 120 "${work}/ql-probe" 5 "${work}"/zall/* </dev/null >"${work}/probe-local.txt" 2>&1
cat "${work}/probe-share.txt" >>"${results}/probe-share.txt"
cat "${work}/probe-local.txt" >>"${results}/probe-local.txt"
say "  样例目录原件：$(count_probe "${work}/probe-share.txt")"
say "  拷到本机的副本：$(count_probe "${work}/probe-local.txt")"
with_timeout 90 log show --start "${since}" --style compact \
    --predicate 'process BEGINSWITH "Anyview" OR process == "ql-probe" OR process == "com.apple.quicklook.ThumbnailsAgent" OR eventMessage CONTAINS[c] "local.anyview" OR (process == "kernel" AND eventMessage CONTAINS[c] "anyview")' \
    >"${work}/log-probe.txt" 2>&1
tail -n 1500 "${work}/log-probe.txt" >>"${results}/log-probe.txt"
say "  这一步里缩略图扩展的记录 $(grep -c 'AnyviewThumbnail\[' "${work}/log-probe.txt") 条，沙盒拒绝 $(grep -c -E 'Anyview(Thumbnail|Preview)\([0-9]+\) deny\(' "${work}/log-probe.txt") 条"
# 交给了系统自己的图片缩略图扩展、它又解不了的类型（说明系统没把这些类型交给 Anyview 的扩展）。
handed="$(grep -o 'TypeIdentifierHint:[a-z.-]*' "${work}/log-probe.txt" | sed 's/TypeIdentifierHint://' | sort -u | tr '\n' ' ')"
say "  交给系统的图片扩展、它解不了的类型：${handed:-没有}"
# Anyview 的缩略图扩展登记着、系统却还把它的类型交给自己的图片扩展：缩略图服务（ThumbnailsAgent，一直开着）
# 还记着以前的选择。重启它（系统用到时会自动再启动），再要一次。
if [[ "${handed}" == *local.anyview* ]] && pluginkit -m -i local.anyview.Anyview.Thumbnail 2>/dev/null | grep -q local.anyview; then
    say ""
    say "== 缩略图服务还记着以前的选择：重启它（系统用到时会自动再启动），再要一次"
    {
        echo "== 重启前：$(pgrep -fl com.apple.quicklook.ThumbnailsAgent | tr '\n' ' ')"
        pkill -f com.apple.quicklook.ThumbnailsAgent; echo "pkill 返回 $?"
        sleep 1
        echo "== pkill 以后：$(pgrep -fl com.apple.quicklook.ThumbnailsAgent | tr '\n' ' ')"
        # 普通的结束信号它不理（手上有事务时会拖着），launchctl kickstart 又被系统完整性保护拦下：
        # 直接结束它（同 killall -9 Dock），它是按需启动的服务，下次要缩略图时系统自动再启动。
        if pgrep -f com.apple.quicklook.ThumbnailsAgent >/dev/null; then
            pkill -9 -f com.apple.quicklook.ThumbnailsAgent; echo "pkill -9 返回 $?"
            sleep 1
            echo "== pkill -9 以后：$(pgrep -fl com.apple.quicklook.ThumbnailsAgent | tr '\n' ' ')"
        fi
        launchctl print "gui/$(id -u)/com.apple.quicklook.ThumbnailsAgent" 2>&1 | head -n 12
    } >"${work}/restart.txt" 2>&1
    cat "${work}/restart.txt" >>"${results}/restart.txt"
    say "  $(grep -E '^== (重启前|pkill 以后|pkill -9 以后)' "${work}/restart.txt" | tr '\n' ' ' | cut -c1-300)"
    sleep 2
    qlmanage -r cache >/dev/null 2>&1
    since="$(date '+%Y-%m-%d %H:%M:%S')"
    with_timeout 120 "${work}/ql-probe" 5 "${share_files[@]}" </dev/null >"${work}/probe-share2.txt" 2>&1
    with_timeout 120 "${work}/ql-probe" 5 "${work}"/zall/* </dev/null >"${work}/probe-local2.txt" 2>&1
    cat "${work}/probe-share2.txt" >>"${results}/probe-share2.txt"
    cat "${work}/probe-local2.txt" >>"${results}/probe-local2.txt"
    say "  样例目录原件：$(count_probe "${work}/probe-share2.txt")"
    say "  拷到本机的副本：$(count_probe "${work}/probe-local2.txt")"
    with_timeout 90 log show --start "${since}" --style compact \
        --predicate 'process BEGINSWITH "Anyview" OR process == "ql-probe" OR process == "com.apple.quicklook.ThumbnailsAgent" OR eventMessage CONTAINS[c] "local.anyview" OR (process == "kernel" AND eventMessage CONTAINS[c] "anyview")' \
        >"${work}/log-probe2.txt" 2>&1
    tail -n 1500 "${work}/log-probe2.txt" >>"${results}/log-probe2.txt"
    say "  缩略图扩展的记录 $(grep -c 'AnyviewThumbnail\[' "${work}/log-probe2.txt") 条；还交给系统图片扩展的类型：$(grep -o 'TypeIdentifierHint:[a-z.-]*' "${work}/log-probe2.txt" | sed 's/TypeIdentifierHint://' | sort -u | tr '\n' ' ')"
    say "  （访达里已经打开的窗口要关掉再打开，缩略图才会重新做）"
fi
# 按空格的预览不在这里查：用 qlmanage -p -o 不开窗口地要预览时，扩展做的预览它接不住、自己崩溃，系统会弹出
# “qlmanage 意外退出”，打扰正在用电脑的人。预览用最近一小时的日志看（上面“预览扩展的记录”）。

say ""
say "完成"
finish 0
