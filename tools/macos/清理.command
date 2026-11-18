#!/bin/bash
# 清理 Anyview 在本机留下的构建缓存与系统运行状态；如 /Applications 已安装则询问是否一并移到废纸篓。
# 删的是 Mac 上的：临时目录里的编译缓存（Rust 工具链、依赖源码、中间文件），以及系统自己记的
# 偏好设置、已存储的窗口状态、缓存、崩溃报告、"打开最近使用"的记录、程序登记（LaunchServices）、
# 访达缩略图和预览扩展的沙盒容器（~/Library/Containers 里的两个）。
# 保留：用户自行安装的 Rust 环境（~/.rustup、~/.cargo；
# 本项目用的 1.99.1 工具链可以用 rustup toolchain uninstall 1.99.1 删掉）。
# 写给 macOS 自带的 bash 3.2：变量一律写成 ${名字}。
set -u

id="local.anyview.Anyview"
root="$(cd "$(dirname "$0")/../.." && pwd)"
library="${HOME}/Library"
temporary="${TMPDIR:-/tmp}"
# 同时清理项目 out/ 与系统临时目录中的构建缓存。
legacy_temp="$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null)"
legacy_temp="${legacy_temp:-${temporary}}"
case "${root}" in
    /Volumes/*) local_out="" ;;
    *) local_out="${root}/out" ;;
esac
removed=0

say() { printf '%s\n' "$*"; }
# 把主目录开头的路径显示成 ~/…（bash 3.2 里 ${变量/#…/\~} 会把反斜杠也留下来）。
short_path() {
    case "$1" in
        "${HOME}"/*) printf '~%s' "${1#${HOME}}" ;;
        *) printf '%s' "$1" ;;
    esac
}
remove() {  # remove <路径>：在就删，说一声
    if [[ -e "$1" || -L "$1" ]]; then
        rm -rf "$1" && say "删掉了：$(short_path "$1")" && removed=$(( removed + 1 ))
    fi
}

if pgrep -f "Anyview.app/Contents/MacOS/[A]nyview" >/dev/null 2>&1; then
    say "Anyview 还开着：先退出它（⌘Q），再双击本文件。"
    printf '按回车键关闭这个窗口…'
    read -r _
    exit 1
fi

say "清理 Anyview 在这台 Mac 上留下的东西…"
lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
# 从 DMG 装进“应用程序”的 Anyview：问一声，同意了才移到废纸篓（废纸篓里还能拿回来）。先取消扩展和程序的登记，
# 免得系统还去用废纸篓里那份的扩展。用系统的"移到废纸篓"接口（同访达），不需要额外的权限。
installed="/Applications/Anyview.app"
if [[ -d "${installed}" ]]; then
    printf '%s' "“应用程序”里装着 Anyview。要一起删掉吗？输入 y 再按回车就移到废纸篓，直接按回车跳过："
    read -r answer
    if [[ "${answer}" == "y" || "${answer}" == "Y" ]]; then
        for appex in "${installed}"/Contents/PlugIns/*.appex; do
            [[ -d "${appex}" ]] && pluginkit -r "${appex}" >/dev/null 2>&1
        done
        [[ -x "${lsregister}" ]] && "${lsregister}" -u "${installed}" >/dev/null 2>&1
        if [[ "$(osascript -l JavaScript -e 'ObjC.import("Foundation"); $.NSFileManager.defaultManager.trashItemAtURLResultingItemURLError($.NSURL.fileURLWithPath("/Applications/Anyview.app"), null, null) ? "ok" : "no"' 2>/dev/null)" == "ok" ]]; then
            say "移到废纸篓了：/Applications/Anyview.app" && removed=$(( removed + 1 ))
        else
            say "没删掉 /Applications/Anyview.app：请在访达里把它拖到废纸篓，再双击一次本文件。"
        fi
    fi
fi
# 临时目录里运行过的副本（自检用的、打开Anyview.command 用的）也有程序登记，先清掉再删目录。
# 访达缩略图扩展：编译测试会临时登记它、测完就取消；中途停掉时可能还登记着，这里一并取消。
dev_builds=("${legacy_temp%/}/anyview-build")
[[ -n "${local_out}" ]] && dev_builds+=("${local_out}/anyview-build")
for copy in "${dev_builds[@]/%//Anyview.app}" "${dev_builds[@]/%//run/Anyview.app}"; do
    if [[ -d "${copy}/Contents/PlugIns/AnyviewThumbnail.appex" ]]; then
        pluginkit -r "${copy}/Contents/PlugIns/AnyviewThumbnail.appex" >/dev/null 2>&1 \
            && say "取消了：缩略图扩展的登记（$(short_path "${copy}")）"
    fi
    if [[ -d "${copy}" && -x "${lsregister}" ]]; then
        "${lsregister}" -u "${copy}" >/dev/null 2>&1 && say "清掉了：程序登记（$(short_path "${copy}")）"
    fi
done
for dev_build in "${dev_builds[@]}"; do
    remove "${dev_build}"
done
[[ -n "${local_out}" ]] && remove "${local_out}/tmp"
for leftover in "${legacy_temp%/}"/anyview-step* "${legacy_temp%/}"/anyview-start* "${legacy_temp%/}"/anyview-api-* \
    "${legacy_temp%/}"/anyview-archives "${legacy_temp%/}"/anyview-ql-check "${legacy_temp%/}"/anyview-test; do
    remove "${leftover}"
done
remove "${library}/Caches/anyview"
remove "${library}/Application Support/Anyview"
if defaults read "${id}" >/dev/null 2>&1; then
    defaults delete "${id}" >/dev/null 2>&1 && say "删掉了：程序的偏好设置（defaults ${id}）" && removed=$(( removed + 1 ))
fi
remove "${library}/Preferences/${id}.plist"
remove "${library}/Saved Application State/${id}.savedState"
remove "${library}/Caches/${id}"
# "打开最近使用"的记录（系统按小写的程序标识存放）。
for list in "${library}/Application Support/com.apple.sharedfilelist/com.apple.LSSharedFileList.ApplicationRecentDocuments"/local.anyview.anyview.sfl*; do
    remove "${list}"
done
for report in "${library}/Logs/DiagnosticReports"/Anyview* "${library}/Logs/DiagnosticReports"/binding-check*; do
    remove "${report}"
done
# 访达缩略图、预览扩展在沙盒里运行，系统第一次运行它们时各建一个容器目录。
# macOS 可能弹窗问“终端”能否访问别的 App 的数据：点“允许”才删得掉；不允许时这里说一声，可以在访达里删。
for container in "${library}/Containers/${id}.Thumbnail" "${library}/Containers/${id}.Preview"; do
    [[ -e "${container}" ]] || continue
    if rm -rf "${container}" 2>/dev/null && [[ ! -e "${container}" ]]; then
        say "删掉了：$(short_path "${container}")" && removed=$(( removed + 1 ))
    else
        say "没删掉：$(short_path "${container}")（系统不让终端删。可以在访达里按 ⇧⌘G 前往 ~/Library/Containers，把“Anyview 缩略图”“Anyview 预览”（英文系统上叫“Anyview Thumbnails”“Anyview Preview”）拖到废纸篓）"
    fi
done
if [[ -d "${root}/out/macos/Anyview.app" && -x "${lsregister}" ]]; then
    "${lsregister}" -u "${root}/out/macos/Anyview.app" >/dev/null 2>&1 && say "清掉了：程序登记（out/macos/Anyview.app）"
fi

if (( removed == 0 )); then
    say "这台 Mac 上没有 Anyview 留下的文件。"
else
    say "好了。"
fi
printf '按回车键关闭这个窗口…'
read -r _
