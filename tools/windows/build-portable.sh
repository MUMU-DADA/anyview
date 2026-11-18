#!/bin/bash
# 构建 Anyview Windows 绿色免安装版：
#   bash tools/windows/build-portable.sh
# 基于 tools\windows\package.ps1 -Action stage 整理的 out/windows/app 打包，并附带 portable.txt（使配置保存在程序旁的
# data 文件夹里，不写进系统；文字由核心的测试按 packaging.rs 生成，19 种语言）。
# 结果是 out/windows/Anyview-<版本>-portable.zip，解压出来是一个 Anyview 文件夹。不含调试符号（.pdb）；
# 带着 LICENSE.txt 和 THIRD-PARTY-NOTICES.txt。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
APP="$REPO/out/windows/app"
for f in Anyview.exe anyview_ffi.dll Microsoft.ui.xaml.dll; do
    if [[ ! -f "$APP/$f" ]]; then
        echo "程序文件夹里没有 $f（$APP）：先在 Windows 上运行 编译测试.cmd，再运行 package.ps1 -Action stage" >&2
        exit 1
    fi
done
# 与安装程序相同：程序是和它编译时的那个核心 DLL 一起测的，核心后来又重编过的话就不是测过的那一套了。
if ! cmp -s "$APP/anyview_ffi.dll" "$REPO/out/windows/anyview_ffi.dll"; then
    echo "out/windows/app 里的核心 DLL 和 out/windows/anyview_ffi.dll 不一样：核心重编过，要在 Windows 上重新编译测试、重新 stage" >&2
    exit 1
fi
NOTE="$REPO/tools/windows/portable.txt"
if [[ ! -f "$NOTE" ]]; then
    echo "没有 $NOTE：运行 ANYVIEW_UPDATE_MESSAGES=1 cargo test -p anyview-core 生成" >&2
    exit 1
fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"
OUT="$REPO/out/windows/Anyview-$VERSION-portable.zip"
rm -f "$OUT"
python3 -I - "$APP" "$NOTE" "$OUT" "$REPO/LICENSE" "$REPO/crates/anyview-core/licenses/windows.txt" <<'PY'
import os, sys, zipfile
app, note, out, license_file, notices = sys.argv[1:6]
names = []
for base, dirs, files in os.walk(app):
    dirs.sort()
    for name in sorted(files):
        if not name.lower().endswith(".pdb"):
            names.append(os.path.join(base, name))
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for path in names:
        rel = os.path.relpath(path, app).replace(os.sep, "/")
        z.write(path, f"Anyview/{rel}")
    z.write(note, "Anyview/portable.txt")
    # 许可证：和安装程序一样，Windows 换行、带 BOM。
    for src, name in ((license_file, "LICENSE.txt"), (notices, "THIRD-PARTY-NOTICES.txt")):
        with open(src, encoding="utf-8") as f:
            text = f.read().replace("\r\n", "\n").replace("\n", "\r\n")
        z.writestr(f"Anyview/{name}", ("\ufeff" + text).encode("utf-8"))
print(f"{len(names) + 3} 个文件")
PY
chmod 644 "$OUT"
ls -l "$OUT"
sha256sum "$OUT"
