#!/bin/bash
# Anyview macOS 打包脚本：在终端执行或在访达中双击运行。
# 把 编译测试.command 编好、测过的 Anyview.app 做成 DMG 安装包（打开后把 Anyview 拖到“应用程序”），
# 将 编译测试.command 编好并测过的 Anyview.app 打包至 out/macos/Anyview-<版本>.dmg，报告写入 reports/macos/<时间>-dmg/summary.txt。
#
# 现在没有付费的开发者证书（Developer ID），程序只有本机签名（ad-hoc）、没有公证：在这台 Mac 上打开没问题；
# 拷到别的 Mac 上第一次打开时系统会拦一下，要在“系统设置 › 隐私与安全性”里点“仍要打开”。
# 不往 Mac 上装任何东西：只用系统自带的 hdiutil、codesign、ditto、plutil；中间文件在临时目录
# anyview-build/ 里（清理.command 会删掉）。不会自动打开或安装这个 DMG。
#
# 兼容 macOS 自带的 bash 3.2：变量统一使用 ${name} 写法。
set -uo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
# 构建产物和临时文件放在本项目的 out/ 里，报告写入 reports/macos/。
build="${root}/out/anyview-build"
temporary="${root}/out/tmp"
mkdir -p "${build}" "${temporary}"
export TMPDIR="${temporary}/"
stamp="$(date +%Y-%m-%dT%H-%M-%S)"
results="${root}/reports/macos/${stamp}-dmg"
if ! mkdir -p "${results}"; then
    printf '%s\n' "无法建结果目录：${results}" "按回车键关闭这个窗口…"
    read -r _
    exit 1
fi
summary="${results}/summary.txt"
app="${build}/Anyview.app"
staging="${build}/dmg-staging"
mount_point="${build}/dmg-check"

finished=0
pause() {
    local code=$?
    if [[ "${finished}" == 0 ]]; then
        printf '%s\n' "" "脚本中途停了，上面是出错信息（也存进了 summary.txt）。" | tee -a "${summary}"
    elif [[ "${code}" != 0 ]]; then
        printf '%s\n' "" "打包没完成：上面那一步出错，脚本已经停止，请查看 summary.txt 与对应日志。" \
            | tee -a "${summary}"
    fi
    printf '\n打包报告已写入 reports/macos/%s-dmg/。\n' "${stamp}"
    if [[ -z "${ANYVIEW_NO_PAUSE:-}" ]]; then
        printf '按回车键关闭这个窗口…'
        read -r _
    fi
}
trap pause EXIT
say() { printf '%s\n' "$*" | tee -a "${summary}"; }
finish() {
    finished=1
    exit "$1"
}
# 一步：命令的输出先写在临时文件里，再整体追加到结果目录的日志里。
step() {  # step <说明> <日志文件名> <命令…>
    local title="$1" log="$2"
    shift 2
    local local_log="${build}/dmg-step.log"
    : >"${local_log}"
    if "$@" >>"${local_log}" 2>&1; then
        say "✓ ${title}"
        cat "${local_log}" >>"${results}/${log}"
        return 0
    fi
    say "✗ ${title}（见 ${log}）"
    cat "${local_log}" >>"${results}/${log}"
    tail -n 20 "${local_log}" | tee -a "${summary}"
    return 1
}

say "Anyview Mac 版打包  ${stamp}"
say "系统：$(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null)（$(uname -m)）"
if [[ ! -d "${app}" ]]; then
    say "✗ 临时目录里没有编好的 Anyview.app：先双击同一个目录里的 编译测试.command（编译、测试通过后再打包）。"
    finish 1
fi
version="$(plutil -extract CFBundleShortVersionString raw -o - "${app}/Contents/Info.plist" 2>/dev/null)"
build_number="$(plutil -extract CFBundleVersion raw -o - "${app}/Contents/Info.plist" 2>/dev/null)"
if [[ -z "${version}" ]]; then
    say "✗ 读不出 Anyview.app 的版本号（Info.plist）"
    finish 1
fi
say "程序：Anyview ${version}（构建 ${build_number}）"
dmg_name="Anyview-${version}.dmg"
dmg_local="${build}/${dmg_name}"
dmg_share="${root}/out/macos/${dmg_name}"

check_signature() {
    codesign --verify --deep --strict --verbose=2 "${app}"
}
step "程序的签名完整（本机签名）" dmg-codesign.txt check_signature || finish 1

make_staging() {
    rm -rf "${staging}" || return 1
    mkdir -p "${staging}" || return 1
    ditto --norsrc --noextattr --noqtn "${app}" "${staging}/Anyview.app" || return 1
    # 打开 DMG 后把 Anyview 拖到旁边的“应用程序”上就装好了。
    ln -s /Applications "${staging}/Applications"
}
step "准备 DMG 的内容（Anyview.app 和“应用程序”的替身）" dmg-staging.txt make_staging || finish 1

make_dmg() {
    rm -f "${dmg_local}" || return 1
    hdiutil create -volname "Anyview ${version}" -srcfolder "${staging}" -fs HFS+ -format UDZO -ov "${dmg_local}"
}
step "做成压缩的 DMG（hdiutil）" dmg-create.txt make_dmg || finish 1

# 核对：校验和对、能挂上、里面的程序签名完整，挂上看完马上推出。
check_dmg() {
    hdiutil verify "${dmg_local}" || return 1
    rm -rf "${mount_point}" && mkdir -p "${mount_point}" || return 1
    hdiutil attach -readonly -nobrowse -noautoopen -mountpoint "${mount_point}" "${dmg_local}" || return 1
    local status=0
    if [[ ! -d "${mount_point}/Anyview.app" || ! -L "${mount_point}/Applications" ]]; then
        echo "DMG 里没有 Anyview.app 或 Applications 替身"
        status=1
    fi
    codesign --verify --deep --strict --verbose=2 "${mount_point}/Anyview.app" || status=1
    # 里面的文件不能带 Finder 信息、资源分支（签名检查不允许，拷进“应用程序”后系统不用它的扩展）。
    # 系统自动添加的 com.apple.provenance 不影响签名校验，此处忽略。
    if xattr -lr "${mount_point}/Anyview.app" 2>/dev/null | grep -qE ': com\.apple\.(FinderInfo|ResourceFork):'; then
        echo "DMG 里的 Anyview.app 带着 Finder 信息或资源分支："
        xattr -lr "${mount_point}/Anyview.app" | grep -E ': com\.apple\.(FinderInfo|ResourceFork):' | head -n 20
        status=1
    fi
    hdiutil detach "${mount_point}" || hdiutil detach -force "${mount_point}" || status=1
    return "${status}"
}
step "核对 DMG：校验和、挂上后程序和签名都在" dmg-check.txt check_dmg || finish 1

copy_out() {
    mkdir -p "$(dirname "${dmg_share}")" || return 1
    rm -f "${dmg_share}" || return 1
    # 用 ditto 复制，不带附加属性。
    ditto --norsrc --noextattr --noqtn "${dmg_local}" "${dmg_share}"
}
step "放到本项目的 out/macos/${dmg_name}" dmg-copy.txt copy_out || finish 1

# 取消开发构建副本的 LaunchServices 登记，避免与 /Applications/Anyview.app 争抢同一 Bundle ID 的扩展注册。
legacy_build="$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null)"
legacy_build="${legacy_build%/}/anyview-build"
forget_dev_copies() {
    local lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    local copy
    [[ -x "${lsregister}" ]] || return 0
    # 同时清理系统临时目录中可能残留的旧构建副本登记。
    for copy in "${root}/out/macos/Anyview.app" "${app}" "${build}/run/Anyview.app" \
        "${legacy_build}/Anyview.app" "${legacy_build}/run/Anyview.app"; do
        [[ -d "${copy}" ]] || continue
        echo "lsregister -u ${copy}"
        "${lsregister}" -u "${copy}" || true
    done
}
step "让系统忘掉开发用的几份 Anyview（装进“应用程序”的那份才是系统用的）" dmg-forget.txt forget_dev_copies || true

size_mb=$(( $(stat -f %z "${dmg_local}" 2>/dev/null || echo 0) / 1048576 ))
sha="$(shasum -a 256 "${dmg_local}" | cut -d' ' -f1)"
say "  大小 ${size_mb} MB，SHA-256 ${sha}"
say "  只有本机签名、没有公证：在这台 Mac 上直接用；拷到别的 Mac 上第一次打开要在“系统设置 › 隐私与安全性”里点“仍要打开”。"
rm -rf "${staging}" "${mount_point}"
say "全部完成"
finish 0
