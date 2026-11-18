#!/bin/bash
# 打开编好的 Mac 版 Anyview（由 编译测试.command 编在本项目的 out/anyview-build/Anyview.app）。
# 复制一份到旁边的 run/ 再打开，保持原始构建产物不变。
# 副本和编译缓存放在一起，双击 清理.command 可以删掉。
# 写给 macOS 自带的 bash 3.2：变量一律写成 ${名字}。
set -u

# 从本地构建目录 out/anyview-build/ 启动已编译的 Anyview.app。
root="$(cd "$(dirname "$0")/../.." && pwd)"
app="${root}/out/anyview-build/Anyview.app"
if [[ ! -d "${app}" ]]; then
    printf '%s\n' "这台 Mac 上还没有编好的 Anyview.app：先双击同一个目录里的 编译测试.command。" "按回车键关闭这个窗口…"
    read -r _
    exit 1
fi
local_app="${root}/out/anyview-build/run/Anyview.app"
version_of() {  # version_of <app>：构建编号（每次编译都不同）
    plutil -extract CFBundleVersion raw -o - "$1/Contents/Info.plist" 2>/dev/null
}
if [[ ! -d "${local_app}" || "$(version_of "${local_app}")" != "$(version_of "${app}")" ]]; then
    rm -rf "${local_app}"
    mkdir -p "$(dirname "${local_app}")"
    if ! ditto --norsrc --noextattr --noqtn "${app}" "${local_app}"; then
        printf '%s\n' "复制失败，直接打开编好的那份。"
        open "${app}"
        exit 0
    fi
fi
open "${local_app}"
