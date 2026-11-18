#!/bin/bash
# 使用 NSIS 3.11 构建 Anyview Windows 安装程序：
#   bash tools/windows/build-installer.sh
# 需先在 Windows 上通过 tools\windows\package.ps1 -Action stage 准备好 out/windows/app，
# 并准备好 NSIS（系统安装或通过 tools/windows/build-nsis.sh 编译）。输出 out/windows/Anyview-<版本>-setup.exe。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
APP="$REPO/out/windows/app"
NSIS="${NSIS:-$HOME/.local/opt/nsis-3.11}"
WORK="$REPO/target/installer"
if [[ ! -x "$NSIS/makensis" ]]; then
    echo "没有 NSIS（$NSIS）：先运行 tools/windows/build-nsis.sh" >&2
    exit 1
fi
for f in Anyview.exe anyview_ffi.dll Microsoft.ui.xaml.dll; do
    if [[ ! -f "$APP/$f" ]]; then
        echo "程序文件夹里没有 $f（$APP）：先在 Windows 上运行 编译测试.cmd，再运行 package.ps1 -Action stage" >&2
        exit 1
    fi
done
# 程序是和它编译时的那个核心 DLL 一起测的：核心后来又重编过的话，装进去的就不是测过的那一套了。
if ! cmp -s "$APP/anyview_ffi.dll" "$REPO/out/windows/anyview_ffi.dll"; then
    echo "out/windows/app 里的核心 DLL 和 out/windows/anyview_ffi.dll 不一样：核心重编过，要在 Windows 上重新编译测试、重新 stage" >&2
    exit 1
fi
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -n 1)"
OUT="$REPO/out/windows/Anyview-$VERSION-setup.exe"
rm -rf "$WORK"
mkdir -p "$WORK"
# 许可证：Anyview 的 MIT 许可证和 Windows 版的第三方软件许可证，装在 Anyview.exe 旁边。
# 换成 Windows 的换行、带 BOM 的 UTF-8（哪个记事本都能正常显示）。
python3 -I - "$REPO/LICENSE" "$REPO/crates/anyview-core/licenses/windows.txt" "$WORK" <<'PY'
import os, sys
for src, name in ((sys.argv[1], "LICENSE.txt"), (sys.argv[2], "THIRD-PARTY-NOTICES.txt")):
    with open(src, encoding="utf-8") as f:
        text = f.read().replace("\r\n", "\n").replace("\n", "\r\n")
    with open(os.path.join(sys.argv[3], name), "w", encoding="utf-8-sig", newline="") as f:
        f.write(text)
PY
# 卸载时要删的文件清单：先各个文件，再各级文件夹（深的在前，空了才删得掉）。不含调试符号（不装）。
python3 -I - "$APP" "$WORK/uninstall-files.nsh" <<'PY'
import os, sys
app, out = sys.argv[1], sys.argv[2]
files, dirs = [], []
for base, subdirs, names in os.walk(app):
    rel = os.path.relpath(base, app)
    if rel != ".":
        dirs.append(rel)
    for name in names:
        if not name.lower().endswith(".pdb"):
            files.append(os.path.normpath(os.path.join(rel, name)))
files += ["LICENSE.txt", "THIRD-PARTY-NOTICES.txt"]
quote = lambda path: path.replace("/", "\\").replace("$", "$$")
with open(out, "w", encoding="utf-8-sig", newline="\n") as f:
    for path in sorted(files):
        f.write(f'Delete "$INSTDIR\\{quote(path)}"\n')
    for path in sorted(dirs, key=lambda d: (-d.count(os.sep), d)):
        f.write(f'RMDir "$INSTDIR\\{quote(path)}"\n')
print(f"{len(files)} 个文件，{len(dirs)} 个文件夹")
PY
rm -f "$OUT"
NSISDIR="$NSIS" "$NSIS/makensis" -V2 -INPUTCHARSET UTF8 \
    -DAPP_VERSION="$VERSION" -DAPP_VERSION4="$VERSION.0" -DAPP_DIR="$APP" \
    -DAPP_ICON="$REPO/apps/windows/Anyview/Assets/Anyview.ico" \
    -DWELCOME_BMP="$REPO/tools/windows/installer/welcome.bmp" \
    -DUNINSTALL_LIST="$WORK/uninstall-files.nsh" -DLICENSE_DIR="$WORK" -DOUT_FILE="$OUT" \
    "$REPO/tools/windows/installer/anyview.nsi"
chmod 755 "$OUT"
ls -l "$OUT"
sha256sum "$OUT"
