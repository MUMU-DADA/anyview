#!/bin/bash
# Anyview 一键编译测试（macOS）：在访达里双击本文件（或在终端里 bash 本文件）。
# 依次：准备 Rust → Rust 测试 → 编译 Rust 核心 → 生成 Swift 接口 → 编译 Mac 版 → Swift 接口测试
#      → 生成测试图片 → 组装 Anyview.app → 打开程序自检：空态、浏览顺序、大图、看图操作、悬浮界面、浏览、高品质、色彩、HEIC/AVIF、
#      快速预览、真实图片、性能、文件（窗口会自己打开、切换、关掉，不用动）。
# 真实图片样例（开放许可，可通过 tools/testdata/fetch.py 下载至 tests/fixtures/real）核对 SHA-256 后参与测试。
# 自检报告写入 reports/macos/<时间>/。
#
# Rust：Mac 上装了 rustup 就用它，版本由仓库的 rust-toolchain.toml 固定；没装时用项目里
# out/macos-toolchain/ 的官方安装包（固定版本，每次核对 SHA-256），装进 Mac 的临时目录，只在本脚本里加进 PATH，
# 不写 ~/.cargo、~/.rustup。
# 其余不往 Mac 上装任何东西：
#   - 编译缓存（中间文件等）在 Mac 的临时目录 anyview-build/ 里：系统几天不用会自动清掉，
#     双击 清理.command 可以马上删掉。
#   - 编好的 Anyview.app 位于 out/anyview-build/，可通过 打开Anyview.command 启动。
#   - 编译器和 macOS SDK 用系统里已经装好的 Xcode 命令行工具（苹果的许可不允许放进项目），脚本只用、不改。
#
# 兼容 macOS 自带的 bash 3.2：变量统一使用 ${name} 写法。
set -uo pipefail

RUST_VERSION=1.99.1
RUST_DATE=2026-10-15
RUST_TARGET=aarch64-apple-darwin
# 官方安装包的 SHA-256，与 static.rust-lang.org 的 channel-rust-1.99.1.toml 及各文件旁的 .sha256 一致。
RUSTC_SHA256=23948a0a9c162fd2952defb3fb0462c44e8fcfd1a4ab8a274a1e1bdb2cf810a3
CARGO_SHA256=685e563d94db1f003ffa5f5831439eec02569b9950b69867953c8213269c1eaf
RUST_STD_SHA256=fe79d6ab5a0321de26463999d8c112b8ad3eafbcbcf822d66a2300b45dde1031
MACOS_MIN=14.0  # 最低 macOS 版本

root="$(cd "$(dirname "$0")/../.." && pwd)"
# 构建缓存、输出应用与报告均放在本项目的 out/ 和 reports/macos/ 目录中。
build="${root}/out/anyview-build"
temporary="${root}/out/tmp"
# 程序自己要读、写的测试文件（测试图片、自检报告、复制移动用的文件夹等）放在系统临时目录（/var/folders/…）：
# 放在“文稿”里时，每次新编的程序第一次读都会弹“允许访问文稿文件夹”，没人点就卡住。
system_temp="$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null)"
testdata="${system_temp:-/tmp/}"
testdata="${testdata%/}/anyview-test"
mkdir -p "${build}" "${temporary}" "${testdata}"
export TMPDIR="${temporary}/"
manifest="${root}/Cargo.toml"
downloads="${root}/out/macos-toolchain"
stamp="$(date +%Y-%m-%dT%H-%M-%S)"
results="${root}/reports/macos/${stamp}"
if ! mkdir -p "${results}"; then
    printf '%s\n' "无法建结果目录：${results}" "按回车键关闭这个窗口…"
    read -r _
    exit 1
fi
summary="${results}/summary.txt"

rust_home="${build}/rust-${RUST_VERSION}"
swift_out="${build}/swift"
bindings="${build}/bindings"
app_local="${build}/Anyview.app"
app_share="${root}/out/macos/Anyview.app"
mkdir -p "${build}"
export CARGO_TARGET_DIR="${build}/target"
export MACOSX_DEPLOYMENT_TARGET="${MACOS_MIN}"

# 出错信息同时进 summary.txt；不管在哪一步结束，窗口都停住等回车。
exec 2> >(tee -a "${summary}" >&2)
finished=0
ran_to_end=0
pause() {
    local code=$?
    if [[ "${finished}" == 0 ]]; then
        printf '%s\n' "" "脚本中途停了，上面是出错信息（也存进了 summary.txt）。" | tee -a "${summary}"
    elif [[ "${code}" != 0 && "${ran_to_end}" == 0 ]]; then
        # 某一步出错、后面的检查没法做时（如编译失败）：说清楚是停了，不是卡住。
        printf '%s\n' "" "测试未完成：上述步骤出错，脚本已停止，请查看 summary.txt 与对应日志。" \
            | tee -a "${summary}"
    fi
    # 开发用的这几份程序让系统忘掉（放在 ~/Documents 里，运行过就会被登记，会和装进“应用程序”的那份抢同一个 Bundle ID，
    #  访达缩略图就因此失效过）；以后运行时系统会再登记。
    local copy
    for copy in "${app_local}" "${build}/run/Anyview.app"; do
        [[ -d "${copy}" ]] && "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister" \
            -u "${copy}" >/dev/null 2>&1
    done
    # 系统临时目录里的测试文件跑完就删（不管通过与否、是否中途停下），不留给用户以后清理；
    # 报告已写入 reports/macos/；临时测试图片下次运行时重新生成。
    case "${testdata}" in
        */anyview-test) rm -rf "${testdata}" && printf '\n删掉了临时测试文件：%s\n' "${testdata}" ;;
    esac
    printf '\n测试报告已写入 reports/macos/%s/。\n' "${stamp}"
    # 跑完响一声：跑的时候可以离开，听到声音再回来。
    afplay /System/Library/Sounds/Glass.aiff >/dev/null 2>&1 &
    if [[ -z "${ANYVIEW_NO_PAUSE:-}" ]]; then
        printf '按回车键关闭这个窗口…'
        read -r _
    fi
}
trap pause EXIT

say() { printf '%s\n' "$*" | tee -a "${summary}"; }
# 把主目录开头的路径显示成 ~/…（bash 3.2 里 ${变量/#…/\~} 会把反斜杠也留下来）。
short_path() {
    case "$1" in
        "${HOME}"/*) printf '~%s' "${1#${HOME}}" ;;
        *) printf '%s' "$1" ;;
    esac
}
finish() {
    finished=1
    exit "$1"
}

say "Anyview Mac 版编译测试  ${stamp}"
say "大约要 8–10 分钟：先编译（不占屏幕），后面约 6 分钟程序会自己开窗口、点菜单，这段时间请不要动鼠标和键盘；跑完会响一声。"
say "系统：$(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null)（$(uname -m)）"
memory_gb=$(( $(sysctl -n hw.memsize 2>/dev/null || echo 0) / 1073741824 ))
# 低电量模式会让冷启动变慢：记下来，性能数字对不上时好查。
low_power="$(pmset -g 2>/dev/null | awk '/lowpowermode/ { print $2 }')"
say "芯片：$(sysctl -n machdep.cpu.brand_string 2>/dev/null)，$(sysctl -n hw.ncpu 2>/dev/null) 核，内存 ${memory_gb} GB；低电量模式：${low_power:-?}；开机 $(uptime | sed 's/.*up \([^,]*\),.*/\1/')"
if [[ "$(uname -m)" != "arm64" ]]; then
    say "✗ 目前只支持 Apple 芯片的 Mac。"
    finish 1
fi

# 1. 编译器：系统的 Xcode 命令行工具（不需要完整的 Xcode）。
if ! developer="$(xcode-select -p 2>/dev/null)" || ! xcrun --find swiftc >/dev/null 2>&1 || ! xcrun --find clang >/dev/null 2>&1; then
    say "✗ 没找到 Xcode 命令行工具。请在终端里运行 xcode-select --install，装完以后再双击本文件。"
    finish 1
fi
sdk="$(xcrun --show-sdk-path 2>/dev/null)"
say "Swift：$(xcrun swiftc --version 2>&1 | grep -m 1 'Swift version')"
say "SDK：$(xcrun --show-sdk-version 2>/dev/null)（${developer}）"

cd "${root}" || finish 1

# 各步骤的执行方式：完整输出存进各自的日志，屏幕上只显示进度。
step() {  # step <名字> <日志> <命令…>
    local name="$1" log="${results}/$2"
    shift 2
    say ""
    say "== ${name}"
    local start=${SECONDS}
    # 先写入临时日志文件，结束后合并到报告日志。
    local output
    output="$(mktemp -t anyview-step)"
    "$@" >"${output}" 2>&1 &
    local pid=$!
    local line
    while kill -0 "${pid}" 2>/dev/null; do
        line="$(tail -n 1 "${output}" 2>/dev/null | cut -c1-100)"
        printf '\r\033[K  %s' "${line}"
        sleep 1
    done
    wait "${pid}"
    local status=$?
    cat "${output}" >>"${log}"
    rm -f "${output}"
    local spent=$(( SECONDS - start ))
    printf '\r\033[K'
    if (( status == 0 )); then
        say "  ✓ 完成（${spent} 秒）"
    else
        say "  ✗ 失败（返回 ${status}，${spent} 秒），详情见 $(basename "${log}")"
        tail -n 20 "${log}" | sed 's/^/    /' | tee -a "${summary}"
    fi
    return "${status}"
}

# 2. Rust：Mac 上装了 rustup 就用它，版本固定为 RUST_VERSION（与 rust-toolchain.toml 一致）；
#    未安装 rustup 时自动下载并校验官方安装包至 out/macos-toolchain/，临时加入 PATH。
pinned="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "${root}/rust-toolchain.toml" 2>/dev/null | head -n 1)"
if [[ "${pinned}" != "${RUST_VERSION}" ]]; then
    say "提醒：rust-toolchain.toml 写的是 ${pinned:-（未找到）}，本脚本是 ${RUST_VERSION}，请同步更新。"
fi
rustup_bin=""
for candidate in "${CARGO_HOME:-${HOME}/.cargo}/bin/rustup" "$(command -v rustup 2>/dev/null)"; do
    if [[ -n "${candidate}" && -x "${candidate}" ]]; then
        rustup_bin="${candidate}"
        break
    fi
done

checksum() { shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1; }
prepare() {  # prepare <组件> <sha256>
    local name="$1" want="$2"
    local file="${downloads}/${name}-${RUST_VERSION}-${RUST_TARGET}.tar.xz"
    if [[ "$(checksum "${file}")" == "${want}" ]]; then
        return 0
    fi
    say "下载 ${name} ${RUST_VERSION}…"
    mkdir -p "${downloads}"
    # 先下载到临时文件，校验 SHA-256 通过后再落盘缓存。
    local part="${build}/${name}.tar.xz.part"
    rm -f "${part}"
    if curl -fL --retry 3 -o "${part}" "https://static.rust-lang.org/dist/${RUST_DATE}/${name}-${RUST_VERSION}-${RUST_TARGET}.tar.xz" \
        && [[ "$(checksum "${part}")" == "${want}" ]]; then
        rm -f "${file}"
        cat "${part}" >>"${file}"
        rm -f "${part}"
        if [[ "$(checksum "${file}")" == "${want}" ]]; then
            return 0
        fi
    fi
    rm -f "${part}"
    say "✗ ${name} 下载失败或校验值不对"
    return 1
}
rust_marker="${rust_home}/.anyview-installed"
rust_want="${RUST_VERSION} ${RUSTC_SHA256} ${CARGO_SHA256} ${RUST_STD_SHA256}"
install_pinned_rust() {
    rm -rf "${rust_home}" "${build}/rust-unpack"
    mkdir -p "${build}/rust-unpack" || return 1
    local name
    for name in rustc cargo rust-std; do
        echo "== ${name}"
        tar xJf "${downloads}/${name}-${RUST_VERSION}-${RUST_TARGET}.tar.xz" -C "${build}/rust-unpack" || return 1
        (cd "${build}/rust-unpack/${name}-${RUST_VERSION}-${RUST_TARGET}" \
            && bash install.sh --prefix="${rust_home}" --disable-ldconfig) || return 1
    done
    rm -rf "${build}/rust-unpack"
    printf '%s' "${rust_want}" >"${rust_marker}"
}

if [[ -n "${rustup_bin}" ]]; then
    export PATH="$(dirname "${rustup_bin}"):${PATH}"
    # 用版本正好是 RUST_VERSION 的工具链：先看同名的，再看已装的其他工具链（例如 stable 现在正好是这个版本），
    # 都没有才让 rustup 安装，避免重复下载一份相同的版本（约 450 MB）。
    toolchain=""
    installed="$("${rustup_bin}" toolchain list 2>/dev/null | awk '{print $1}')"
    for candidate in $(printf '%s\n' "${installed}" | grep "^${RUST_VERSION}-") \
                     $(printf '%s\n' "${installed}" | grep -v "^${RUST_VERSION}-"); do
        if "${rustup_bin}" run "${candidate}" rustc --version 2>/dev/null | grep -q "^rustc ${RUST_VERSION} "; then
            toolchain="${candidate}"
            break
        fi
    done
    if [[ -z "${toolchain}" ]]; then
        step "用 rustup 安装 Rust ${RUST_VERSION}（只在第一次需要）" rust-setup.txt \
            "${rustup_bin}" toolchain install "${RUST_VERSION}" --profile minimal --component clippy --component rustfmt \
            || finish 1
        toolchain="${RUST_VERSION}"
    fi
    export RUSTUP_TOOLCHAIN="${toolchain}"
    rust_source="Mac 上的 rustup：$(short_path "${rustup_bin}")，工具链 ${toolchain}"
else
    rust_source="项目里的安装包，装在临时目录"
    export CARGO_HOME="${build}/cargo-home"
    prepare rustc "${RUSTC_SHA256}" || finish 1
    prepare cargo "${CARGO_SHA256}" || finish 1
    prepare rust-std "${RUST_STD_SHA256}" || finish 1
    if [[ "$(cat "${rust_marker}" 2>/dev/null)" != "${rust_want}" || ! -x "${rust_home}/bin/cargo" ]]; then
        step "把 Rust ${RUST_VERSION} 装进临时目录（约 450 MB，只在第一次或被系统清掉以后需要）" rust-setup.txt \
            install_pinned_rust || finish 1
    fi
    export PATH="${rust_home}/bin:${PATH}"
    export RUSTC="${rust_home}/bin/rustc"
fi
if ! rust_version="$(rustc --version 2>&1)" || ! cargo_version="$(cargo --version 2>&1)"; then
    say "✗ Rust 运行不了：${rust_version:-} ${cargo_version:-}"
    finish 1
fi
say "Rust：${rust_version}；${cargo_version}（${rust_source}）"

# 3. 编译、测试、自检的各个步骤。
build_rust_core() {
    cargo build --locked --release --manifest-path "${manifest}" -p anyview-ffi || return 1
    # 记下链接 Rust 静态库还需要的系统库，给 Swift 链接时用。没有重新编译时 rustc 不打印，就沿用上次记下的。
    local note
    note="$(cargo rustc --locked --release --manifest-path "${manifest}" -p anyview-ffi --crate-type staticlib \
        -- --print native-static-libs 2>&1 | sed -n 's/.*native-static-libs: //p' | tail -n 1)"
    if [[ -n "${note}" ]]; then
        printf '%s' "${note}" >"${build}/native-libs.txt"
    fi
    printf '%s\n' "链接时附加的系统库：$(cat "${build}/native-libs.txt" 2>/dev/null)"
    ls -la "${CARGO_TARGET_DIR}/release/libanyview_ffi.a" "${CARGO_TARGET_DIR}/release/libanyview_ffi.dylib"
}

generate_bindings() {
    rm -rf "${bindings}"
    cargo run --locked --release --manifest-path "${manifest}" -p uniffi-bindgen -- \
        generate --library "${CARGO_TARGET_DIR}/release/libanyview_ffi.dylib" --language swift --out-dir "${bindings}" || return 1
    ls -la "${bindings}"
}

# 只用 Xcode 命令行工具里的 swiftc 直接编译，不需要 Xcode 或 SwiftPM。
compile_swift() {
    # swiftc 本来就会链接 libSystem，去掉 -lSystem，免得链接器提示重复。
    local native_libs
    native_libs="$(sed 's/-lSystem//g' "${build}/native-libs.txt" 2>/dev/null)"
    rm -rf "${swift_out}"
    mkdir -p "${swift_out}/modules" || return 1
    echo "== AnyviewCore：UniFFI 生成的 Swift 接口"
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library \
        -Xcc "-fmodule-map-file=${bindings}/AnyviewFFI.modulemap" -I "${bindings}" \
        -module-name AnyviewCore -emit-module -emit-module-path "${swift_out}/modules/AnyviewCore.swiftmodule" \
        -emit-library -static -o "${swift_out}/libAnyviewCore.a" \
        "${bindings}/AnyviewCore.swift" || return 1
    # 应用和接口测试都编译一遍再报告，一次看到全部编译错误。
    local status=0
    echo "== Anyview：应用"
    # native_libs 按空格分成多个参数，有意不加引号。
    # shellcheck disable=SC2086
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library \
        -Xcc "-fmodule-map-file=${bindings}/AnyviewFFI.modulemap" -I "${bindings}" -I "${swift_out}/modules" \
        -module-name Anyview "${root}"/apps/macos/Sources/Anyview/*.swift \
        "${swift_out}/libAnyviewCore.a" "${CARGO_TARGET_DIR}/release/libanyview_ffi.a" ${native_libs} \
        -o "${swift_out}/Anyview" || status=1
    # 两个访达扩展编译不过时不算整个编译失败：程序里先不放它，其他检查照常做，摘要里记一笔（少跑一次）。
    rm -f "${build}/failed-extensions.txt"
    echo "== AnyviewThumbnail：访达缩略图扩展"
    # app extension：入口是 Foundation 的 NSExtensionMain，按扩展的规则编译（-application-extension）。
    # shellcheck disable=SC2086
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library -application-extension \
        -Xcc "-fmodule-map-file=${bindings}/AnyviewFFI.modulemap" -I "${bindings}" -I "${swift_out}/modules" \
        -module-name AnyviewThumbnail "${root}"/apps/macos/Extensions/Thumbnail/*.swift \
        "${swift_out}/libAnyviewCore.a" "${CARGO_TARGET_DIR}/release/libanyview_ffi.a" ${native_libs} \
        -framework QuickLookThumbnailing -Xlinker -e -Xlinker _NSExtensionMain -Xlinker -dead_strip \
        -o "${swift_out}/AnyviewThumbnail" || echo "缩略图扩展（AnyviewThumbnail）" >>"${build}/failed-extensions.txt"
    echo "== AnyviewPreview：访达预览扩展（按空格）"
    # shellcheck disable=SC2086
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library -application-extension \
        -Xcc "-fmodule-map-file=${bindings}/AnyviewFFI.modulemap" -I "${bindings}" -I "${swift_out}/modules" \
        -module-name AnyviewPreview "${root}"/apps/macos/Extensions/Preview/*.swift \
        "${swift_out}/libAnyviewCore.a" "${CARGO_TARGET_DIR}/release/libanyview_ffi.a" ${native_libs} \
        -framework QuickLookUI -Xlinker -e -Xlinker _NSExtensionMain -Xlinker -dead_strip \
        -o "${swift_out}/AnyviewPreview" || echo "预览扩展（AnyviewPreview）" >>"${build}/failed-extensions.txt"
    echo "== TypeSurvey：决定缩略图、预览扩展声明哪些类型"
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library \
        "${root}"/apps/macos/Tools/TypeSurvey/*.swift -o "${swift_out}/type-survey" || status=1
    # 只是 Quick Look 那一步的检查工具：编译不过时那一项少一份记录，不算整个编译失败。
    echo "== QLProbe：用访达同样的接口要缩略图（检查扩展用）"
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library \
        "${root}"/apps/macos/Tools/QLProbe/*.swift -framework QuickLookThumbnailing -o "${swift_out}/ql-probe" \
        || echo "（QLProbe 没编译成功：Quick Look 那一步少一项记录）"
    echo "== BindingCheck：Swift 接口测试"
    # shellcheck disable=SC2086
    xcrun swiftc -O -target "arm64-apple-macos${MACOS_MIN}" -sdk "${sdk}" -parse-as-library \
        -Xcc "-fmodule-map-file=${bindings}/AnyviewFFI.modulemap" -I "${bindings}" -I "${swift_out}/modules" \
        -module-name BindingCheck "${root}"/apps/macos/Tests/BindingCheck/*.swift \
        "${swift_out}/libAnyviewCore.a" "${CARGO_TARGET_DIR}/release/libanyview_ffi.a" ${native_libs} \
        -o "${swift_out}/binding-check" || status=1
    ls -la "${swift_out}"
    return "${status}"
}

# 一个 app extension：可执行文件、Info.plist（版本号、类型调查选出的类型）、带沙盒权限的签名。
# 用 assemble_app 里的 version、build_number。
assemble_extension() {  # assemble_extension <名字> <Info.plist 模板> <声明的类型（JSON 数组）>
    local appex="${app_local}/Contents/PlugIns/$1.appex"
    mkdir -p "${appex}/Contents/MacOS" || return 1
    cp "${swift_out}/$1" "${appex}/Contents/MacOS/$1" || return 1
    sed -e "s/__VERSION__/${version}/g" -e "s/__BUILD__/${build_number}/g" \
        "${root}/apps/macos/Resources/$2" >"${appex}/Contents/Info.plist" || return 1
    plutil -replace NSExtension.NSExtensionAttributes.QLSupportedContentTypes -json "$3" \
        "${appex}/Contents/Info.plist" || return 1
    plutil -lint "${appex}/Contents/Info.plist" || return 1
    # 各语言的名字（“系统设置”的扩展列表里显示）。
    mkdir -p "${appex}/Contents/Resources" || return 1
    ditto "${root}/apps/macos/Resources/Localized/$1" "${appex}/Contents/Resources" || return 1
    codesign --force --sign - --timestamp=none \
        --entitlements "${root}/apps/macos/Resources/Thumbnail.entitlements" "${appex}"
}

assemble_app() {
    local version build_number
    version="$(sed -n 's/^version = "\(.*\)"/\1/p' "${manifest}" | head -n 1)"
    build_number="$(date +%Y%m%d.%H%M%S)"
    rm -rf "${app_local}"
    mkdir -p "${app_local}/Contents/MacOS" "${app_local}/Contents/Resources" || return 1
    cp "${swift_out}/Anyview" "${app_local}/Contents/MacOS/Anyview" || return 1
    sed -e "s/__VERSION__/${version}/g" -e "s/__BUILD__/${build_number}/g" \
        "${root}/apps/macos/Resources/Info.plist" >"${app_local}/Contents/Info.plist" || return 1
    plutil -lint "${app_local}/Contents/Info.plist" || return 1
    # 应用图标：Info.plist 的 CFBundleIconFile 指向它。
    cp "${root}/apps/macos/Resources/AppIcon.icns" "${app_local}/Contents/Resources/AppIcon.icns" || return 1
    # 各语言的文件类型名（访达“种类”一栏，；由核心的测试生成）。
    ditto "${root}/apps/macos/Resources/Localized/Anyview" "${app_local}/Contents/Resources" || return 1
    # 许可证：Anyview 的 MIT 许可证和 macOS 版的第三方软件许可证（程序里的“帮助 › 开源许可”显示同样的内容）。
    cp "${root}/LICENSE" "${app_local}/Contents/Resources/LICENSE.txt" || return 1
    cp "${root}/crates/anyview-core/licenses/macos.txt" "${app_local}/Contents/Resources/THIRD-PARTY-NOTICES.txt" || return 1
    # 访达缩略图、预览扩展：先在这台 Mac 上调查哪些格式系统自己解不了，只声明这些；
    # 扩展必须在沙盒里（Thumbnail.entitlements），先签扩展、再签整个程序。
    # 带上测试图片：系统列着、却解不了其中某张的类型也由扩展做（如 BC7 的 DDS）。
    "${swift_out}/type-survey" "${root}/apps/macos/Resources/Info.plist" \
        "${build}/thumbnail-types.txt" "${build}/thumbnail-survey.txt" "${fixtures}" "${build}/preview-types.txt" || return 1
    local thumbnail_json preview_json
    thumbnail_json="[$(sed -e '/^$/d' -e 's/.*/"&"/' "${build}/thumbnail-types.txt" | paste -sd, -)]"
    preview_json="[$(sed -e '/^$/d' -e 's/.*/"&"/' "${build}/preview-types.txt" | paste -sd, -)]"
    local extension
    for extension in AnyviewThumbnail:ThumbnailInfo.plist AnyviewPreview:PreviewInfo.plist; do
        local types="${thumbnail_json}"
        [[ "${extension%%:*}" == AnyviewPreview ]] && types="${preview_json}"
        if [[ -x "${swift_out}/${extension%%:*}" ]]; then
            assemble_extension "${extension%%:*}" "${extension#*:}" "${types}" || return 1
        else
            echo "（${extension%%:*} 没编译成功：程序里先不放它）"
        fi
    done
    # 签名前去掉所有附加属性（Finder 信息、资源分支等不许出现在签了名的程序里），签完用 --deep 连扩展一起核对。
    xattr -cr "${app_local}"
    codesign --force --sign - --timestamp=none "${app_local}" || return 1
    codesign --verify --deep --strict --verbose=2 "${app_local}" || return 1
    # 仅保留 out/anyview-build/ 下的开发副本；需要安装时通过 打包.command 生成 DMG，
    # 避免多个副本争抢同一 Bundle ID 的 Quick Look 扩展登记。
    # 要装就用 打包.command 做的 DMG；打开Anyview.command 用本项目 out/anyview-build/ 里编好的这份。
    echo "编好了：版本 ${version}，构建 ${build_number}"
    if [[ -d "${app_share}" ]]; then
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister" \
            -u "${app_share}" >/dev/null 2>&1
        rm -rf "${app_share}" && echo "已清理旧的 out/macos/Anyview.app"
    fi
}

# 在临时目录生成本次自检用的合成测试图片。
fixtures="${testdata}/fixtures"
generate_fixtures() {
    cargo run --locked --release --manifest-path "${manifest}" -p anyview-fixtures -- --out "${fixtures}" || return 1
    # HEIC、AVIF（ImageIO）：用系统自带的 sips 从四色块图片生成；系统不能写某种格式时跳过它。
    mkdir -p "${fixtures}/apple"
    local format
    for format in heic avif; do
        if sips -s format "${format}" "${fixtures}/view/quadrants_600x400.png" \
            --out "${fixtures}/apple/quadrants.${format}" >/dev/null 2>&1; then
            echo "生成了 apple/quadrants.${format}"
        else
            rm -f "${fixtures}/apple/quadrants.${format}"
            echo "sips 不能写 ${format}，跳过这种格式的自检"
        fi
    done
}

# 打开程序自检：程序自己打开、切换、截图、写报告，然后退出。报告先写在 Mac 本地，做完再复制进结果的 <名字>/ 目录。
# 运行的是 Mac 本地 out/anyview-build/ 里的 Anyview.app：从网络共享启动时，程序代码要边运行边从网络读，
# 冷启动慢一秒多，测不出真实的速度。
# 注意 bash 3.2 在 set -u 下展开空的 "$@" 会报错，所以用 ${1+"$@"}。
self_test() {  # self_test <名字> [应用参数…]
    local name="$1" waited=0
    shift
    local report_dir="${testdata}/selftest/${name}"
    rm -rf "${report_dir}"
    mkdir -p "${report_dir}" "${results}/${name}"
    # 这一项要用的设置（没有时用默认值）：自检的设置文件就是报告目录里的 settings.toml。
    if [[ -n "${seed_settings:-}" ]]; then
        printf '%s' "${seed_settings}" >>"${report_dir}/settings.toml"
    fi
    open -n -W "${launch_app:-${app_local}}" --args --self-test "${report_dir}" ${1+"$@"} &
    local waiter=$!
    while kill -0 "${waiter}" 2>/dev/null; do
        sleep 1
        waited=$(( waited + 1 ))
        if (( waited > 150 )); then
            echo "150 秒还没退出，结束它"
            pkill -f "Anyview.app/Contents/MacOS/[A]nyview" 2>/dev/null
            sleep 2
            kill "${waiter}" 2>/dev/null
            break
        fi
    done
    wait "${waiter}" 2>/dev/null
    local file
    for file in "${report_dir}"/*; do
        if [[ -f "${file}" ]]; then
            cat "${file}" >>"${results}/${name}/$(basename "${file}")"
        fi
    done
    cat "${report_dir}/selftest.json" 2>/dev/null
    [[ "$(plutil -extract ok raw -o - "${report_dir}/selftest.json" 2>/dev/null)" == "true" ]]
}

json_value() {  # json_value <报告> <键路径>，如 steps.0.name
    plutil -extract "$2" raw -o - "$1" 2>/dev/null
}

# 真实测试图片（可通过 tools/testdata/fetch.py 下载至 tests/fixtures/real，或设置 ANYVIEW_REAL_DIR）。
real_share="${ANYVIEW_REAL_DIR:-${root}/tests/fixtures/real}"
real_manifest="${root}/tools/testdata/real-images.json"
real="${testdata}/real"
sha256_of() {
    shasum -a 256 "$1" 2>/dev/null | cut -d' ' -f1
}
prepare_real() {
    mkdir -p "${real}" || return 1
    local i=0 name sha
    while name="$(plutil -extract "images.${i}.name" raw -o - "${real_manifest}" 2>/dev/null)"; do
        sha="$(plutil -extract "images.${i}.sha256" raw -o - "${real_manifest}" 2>/dev/null)"
        if [[ "$(sha256_of "${real}/${name}")" != "${sha}" ]]; then
            if [[ ! -f "${real_share}/${name}" ]]; then
                echo "✗ 缺少真实测试图片 ${name}（目录：${real_share}）"
                return 1
            fi
            cp "${real_share}/${name}" "${real}/${name}" || return 1
        fi
        if [[ "$(sha256_of "${real}/${name}")" != "${sha}" ]]; then
            echo "✗ ${name} 的 SHA-256 与清单不符"
            rm -f "${real}/${name}"
            return 1
        fi
        echo "✓ ${name}"
        i=$(( i + 1 ))
    done
    if (( i == 0 )); then
        echo "✗ 读不出清单 ${real_manifest}"
        return 1
    fi
    if [[ "$(ls "${fixtures}/many" 2>/dev/null | wc -l | tr -d ' ')" -lt 10000 ]]; then
        echo "生成 1 万个小文件（many/）"
        cargo run --locked --release --manifest-path "${manifest}" -p anyview-fixtures -- \
            --out "${fixtures}" --many 10000 || return 1
    fi
}

# 性能结果的一行：数值保留一位小数，与目标比较（不达标只标出来，不算自检失败）。
perf_misses=0
perf_value_row() {  # perf_value_row <说明> <数值> <目标> <单位>
    local value="$2"
    if [[ -z "${value}" ]]; then
        say "    ? $1：没有数据（目标 ≤ $3 $4）"
        perf_misses=$(( perf_misses + 1 ))
    elif awk -v v="${value}" -v t="$3" 'BEGIN { exit !(v <= t) }'; then
        say "    ✓ $1：$(printf '%.1f' "${value}") $4（目标 ≤ $3 $4）"
    else
        say "    △ $1：$(printf '%.1f' "${value}") $4（目标 ≤ $3 $4，未达标）"
        perf_misses=$(( perf_misses + 1 ))
    fi
}
perf_row() {  # perf_row <说明> <报告> <键路径> <目标> <单位>
    perf_value_row "$1" "$(json_value "$2" "$3")" "$4" "$5"
}
perf_note() {  # perf_note <说明> <报告> <键路径> <单位>：只列出数值
    local value
    value="$(json_value "$2" "$3")"
    say "      $1：$(if [[ -n "${value}" ]]; then printf '%.1f' "${value}"; else printf '?'; fi) $4"
}

# 系统的崩溃报告：这次运行期间新出现的，最后复制进结果。
crash_dir="${HOME}/Library/Logs/DiagnosticReports"
marker="$(mktemp -t anyview-start)"
collect_crashes() {
    local found=0 report
    for report in "${crash_dir}"/Anyview* "${crash_dir}"/binding-check*; do
        [[ -f "${report}" && "${report}" -nt "${marker}" ]] || continue
        cat "${report}" >>"${results}/$(basename "${report}")" && found=1
        say "  崩溃报告：$(basename "${report}")（已复制进结果）"
    done
    rm -f "${marker}"
    return "${found}"
}

failed=0
step "Rust 测试（核心逻辑）" rust-tests.txt cargo test --locked --release --manifest-path "${manifest}" || failed=1
step "编译 Rust 核心" rust-build.txt build_rust_core || finish 1
step "生成 Swift 接口" bindgen.txt generate_bindings || finish 1
step "编译 Mac 版（Swift）" swift-build.txt compile_swift || finish 1
step "生成测试图片" fixtures.txt generate_fixtures || finish 1
step "Swift 接口测试" binding-check.txt "${swift_out}/binding-check" "${fixtures}" || failed=1
step "组装 Anyview.app" app.txt assemble_app || finish 1

# ANYVIEW_BUILD_ONLY=1：仅编译并组装 Anyview.app，跳过界面自检。
if [[ -n "${ANYVIEW_BUILD_ONLY:-}" ]]; then
    say ""
    say "只编译（ANYVIEW_BUILD_ONLY）：程序在 $(short_path "${app_local}")，没有打开程序自检。"
    finish "${failed}"
fi

say ""
say "== 打开程序自检（窗口会自己打开、切换、关掉，不用动）"
step "空态" selftest-empty.txt self_test empty || failed=1
step "浏览：打开 sort/1.png，再下一张、下一张、末张、首张" selftest-nav.txt \
    self_test nav --open "${fixtures}/sort/1.png" --steps next,next,last,first || failed=1
step "大图：打开 1200 万像素 JPEG" selftest-photo.txt \
    self_test photo --open "${fixtures}/basic/photo_4000x3000.jpg" || failed=1
# 看图操作：每一步截图，并按核心算出的位置核对画面颜色（旋转、翻转方向，缩放、平移位置）。
view_steps="rotate_cw,flip_horizontal,rotate_none,zoom_in,zoom_in,pan_down,edge_none,edge_soften,fit_window"
view_steps="${view_steps},next_image,fit_width,pan_down,rotate_ccw,flip_vertical,zoom_out,zoom_original,rotate_180"
step "看图操作：旋转、翻转、缩放、平移、显示方式（逐步截图核对）" selftest-view.txt \
    self_test view --open "${fixtures}/view/quadrants_600x400.png" --steps "${view_steps}" || failed=1
step "悬浮界面：锁定控制栏、图片信息、进度条跳转、透明棋盘格" selftest-overlay.txt \
    self_test overlay --open "${fixtures}/basic/transparent_800x600.png" \
    --steps "toggle_bar_lock,toggle_info,bar_jump:0.5,bar_jump:0.17,toggle_info,toggle_bar_lock" || failed=1
# 浏览、压缩包这两项查的是到达首尾时"询问"的那种做法（默认是直接回到首张）：先把设置写成"询问"。
ask_at_ends="$(printf 'version = 2\n[general]\nat_boundary = "ask"\nat_archive_boundary = "ask"\n_')"
ask_at_ends="${ask_at_ends%_}"
seed_settings="${ask_at_ends}" step "浏览：降序、随机与退回、下一个/上一个文件夹、到达末张时的询问" selftest-browse.txt \
    self_test browse --open "${fixtures}/sort/1.png" \
    --steps "sort_desc,first_image,sort_desc,random_forward,random_back,next_container,prev_container,last_image,next_image,first_image" || failed=1
quality_steps="zoom_in,interp_lanczos,wait_exact,rotate_cw,wait_exact,flip_horizontal,wait_exact"
# 切图前锁定缩放（150%）：新图片保持这个倍率，一定需要精确重绘（不受窗口跟着图片变大小影响）。
quality_steps="${quality_steps},edge_soften_sharpen,wait_exact,invert,wait_exact,gamma,wait_exact,zoom_lock,next_image,wait_exact"
quality_steps="${quality_steps},zoom_lock,invert,gamma,interp_bilinear,edge_soften"
step "高品质：Lanczos 精确重绘、减淡+锐化、反相、Gamma（逐步截图核对）" selftest-quality.txt \
    self_test quality --open "${fixtures}/view/quadrants_600x400.png" --steps "${quality_steps}" || failed=1
step "色彩：带 ICC 的图片按配置文件显示（ColorSync）" selftest-color.txt \
    self_test color --open "${fixtures}/color/linear_gray_128.png" || failed=1
for format in heic avif; do
    if [[ -f "${fixtures}/apple/quadrants.${format}" ]]; then
        step "${format}：用系统的 ImageIO 打开（逐步截图核对）" "selftest-${format}.txt" \
            self_test "${format}" --open "${fixtures}/apple/quadrants.${format}" || failed=1
    fi
done
# 窗口跟着图片：每一步都核对窗口实际大小与程序设定的大小一致。
step "窗口跟着图片：缩放、旋转、最大化后切图、模拟拖动窗口、再切图" selftest-window.txt \
    self_test window --open "${fixtures}/basic/photo_4000x3000.jpg" \
    --steps "zoom_in,rotate_cw,maximize,next_image,maximize,window:700x500,next_image,fullscreen,next_image,fullscreen" \
    || failed=1
step "快速预览：600 万像素 JPEG 先显示内嵌缩略图，再换成清晰图" selftest-preview.txt \
    self_test preview --open "${fixtures}/preview/thumbnail_3000x2000.jpg" || failed=1
# 设置（docs/SETTINGS.md）：逐页截图；改背景色、隐藏控制栏和倍率提示、透明背景、Esc、语言，缓存大小填错时"确定"拦下；
# 生效后核对画面、按键和设置文件；再打开时放弃修改。设置文件放在自检的临时目录里，不碰你自己的设置。
settings_steps="settings_open,settings_page:browse,settings_set:browse.background_color=#336699"
settings_steps="${settings_steps},settings_set:browse.hide_control_bar=true,settings_set:browse.hide_zoom_info=true"
settings_steps="${settings_steps},settings_page:image,settings_set:image.cache_mb=50,settings_set:image.alpha_background=white"
settings_steps="${settings_steps},settings_ok_invalid,settings_set:image.cache_mb=512,settings_page:general"
settings_steps="${settings_steps},settings_set:general.esc_quits=false,settings_page:language,settings_set:language.language=en"
settings_steps="${settings_steps},settings_ok,zoom_out,hover_bottom,key:Escape,settings_open,settings_page:mouse"
settings_steps="${settings_steps},settings_set:mouse.double_click_maximize=false,settings_cancel,settings_open,settings_page:misc"
settings_steps="${settings_steps},settings_page:formats,settings_page:keyboard,settings_page:bookmarks,settings_cancel"
step "设置窗口：逐页截图、改设置、确定生效、填错时拦下、放弃修改" selftest-settings.txt \
    self_test settings --open "${fixtures}/basic/transparent_800x600.png" --steps "${settings_steps}" || failed=1
step "设置：重新启动后设置还在" selftest-settings-reload.txt \
    self_test settings_reload --open "${fixtures}/basic/transparent_800x600.png" \
    --settings "${testdata}/selftest/settings/settings.toml" --steps "settings_open,settings_cancel,zoom_out" || failed=1
# 键盘、鼠标映射：关掉 PageDown 那一行，Q 映射为放大、→ 映射为缩小，滚轮向下放大、右键下一张、单击两侧翻页；
# 确定后逐个按键、点鼠标，核对执行的动作。
mapping_steps="settings_open,settings_page:keyboard,settings_set:keyboard.mappings#0=off"
mapping_steps="${mapping_steps},settings_set:keyboard.mappings#2=on|Q|zoom_in,settings_set:keyboard.mappings#3=on|Right|zoom_out"
mapping_steps="${mapping_steps},settings_page:mouse,settings_set:mouse.wheel_down=on|zoom_in,settings_set:mouse.right=on|next_image"
mapping_steps="${mapping_steps},settings_set:mouse.click_sides=true,settings_ok,key:Q,key:Right,key:PageDown,mouse:wheel_down"
mapping_steps="${mapping_steps},mouse:right,mouse:left:0.1,mouse:double:0.9,mouse:double:0.5"
step "键盘、鼠标映射：设好后逐个按键、点鼠标核对" selftest-mapping.txt \
    self_test mapping --open "${fixtures}/view/quadrants_600x400.png" --steps "${mapping_steps}" || failed=1
# 压缩包：ZIP 里翻页、到末尾询问、下一个压缩包到 7Z（固实）、RAR，再打开 ZIP 回到上次那一页，
# 从压缩包里复制到图片文件夹，清掉记住的位置。解出的临时文件在 Mac 的临时目录里，换压缩包、退出时删掉。
rm -rf "${testdata}/archive-copies"
mkdir -p "${testdata}/archive-copies"
archive_steps="next_image,last_image,next_image,next_container,next_image,next_container"
archive_steps="${archive_steps},open:${fixtures}/archives/a_book.cbz,settings_open,settings_page:misc"
archive_steps="${archive_steps},settings_set:misc.folder1=${testdata}/archive-copies,settings_ok,copy_to_folder1"
archive_steps="${archive_steps},settings_open,settings_page:bookmarks,settings_button:bookmarks.clear_archive:clear,settings_cancel"
seed_settings="${ask_at_ends}" step "压缩包：ZIP、7Z、RAR 里翻页、相邻压缩包、记住位置、复制出来" selftest-archive.txt \
    self_test archive --open "${fixtures}/archives/a_book.cbz" --steps "${archive_steps}" || failed=1
# 选择图片窗口：打开时选中当前图片、缩略图带生成缩略图；单击别的行主窗口不切换；按大小排序后选择不变；
# 隐藏、再显示缩略图导航；确定后切过去；再打开、取消不切换；压缩包里也能用。
picker_steps="picker,picker_select:4,picker_sort:sort_size,picker_hide:on,picker_hide:off,picker_ok"
picker_steps="${picker_steps},picker,picker_select:0,picker_cancel,open:${fixtures}/archives/a_book.cbz,picker,picker_select:2,picker_ok"
step "选择图片窗口：缩略图、列表、排序、隐藏缩略图、确定、取消、压缩包" selftest-picker.txt \
    self_test picker --open "${fixtures}/sort/2.png" --steps "${picker_steps}" || failed=1
# 翻页效果和音效：淡出、滑动、翻页（往前、往后）各停在一半截图，按颜色核对两张叠在一起的画面（停住时翻页用平面的画法，
# 截图画不了三维变换）；最后真的翻一页，等效果做完再核对。自检时翻页音效只记下来，不出声。
effects_steps="settings_open,settings_page:image,settings_set:image.page_effect=fade,settings_page:misc"
effects_steps="${effects_steps},settings_set:misc.page_sound=true,settings_ok,transition_hold:0.5,next_image,transition_hold:off"
effects_steps="${effects_steps},settings_open,settings_page:image,settings_set:image.page_effect=slide,settings_ok"
effects_steps="${effects_steps},transition_hold:0.5,next_image,transition_hold:off,settings_open,settings_page:image"
effects_steps="${effects_steps},settings_set:image.page_effect=page,settings_ok,transition_hold:0.5,prev_image,transition_hold:off,prev_image"
step "翻页效果：淡出、滑动、翻页停在一半核对画面，翻页音效" selftest-effects.txt \
    self_test effects --open "${fixtures}/effects/1_red.png" --steps "${effects_steps}" || failed=1
# 更多格式：formats/（PSD、PSB、DDS、PCX）、modern/（EXR、Radiance HDR、JPEG XL、SVG、AVIF、HEIC，后两种用 ImageIO；
# JPEG 2000、JPEG XR、Clip Studio Paint）、raw/（各家 RAW 的内嵌预览），逐张打开核对画面。动态 AVIF、JPEG XL 动图在播放。
formats_steps="$(printf 'next_image,%.0s' $(seq 15))open:${fixtures}/modern/avif_anim.avif,$(printf 'next_image,%.0s' $(seq 18))"
formats_steps="${formats_steps}open:${fixtures}/raw/a_tiff.nef$(printf ',next_image%.0s' $(seq 8))"
step "更多格式：PSD、DDS、PCX、EXR、Radiance HDR、JPEG XL、SVG、AVIF、HEIC、JPEG 2000、JPEG XR、CLIP、RAW 预览" selftest-formats.txt \
    self_test formats --open "${fixtures}/formats/dds_bc7_dx10.dds" --steps "${formats_steps}" || failed=1
# 生成信息：ComfyUI 的 PNG 上按 P 打开面板，往后翻到 WebP、JPEG（EXIF 里的 A1111 参数），再到没有生成信息的 plain.png。
generation_steps="toggle_info,key:P,generation:state,next_image,generation:state,next_image,generation:state"
generation_steps="${generation_steps},next_image,next_image,next_image,generation:state,generation:close"
step "生成信息：按 P 打开面板，ComfyUI、A1111（PNG、WebP、JPEG）的提示词" selftest-generation.txt \
    self_test generation --open "${fixtures}/generation/comfy.png" --steps "${generation_steps}" || failed=1
# 访达缩略图：先在程序里用核心给几类测试图片生成缩略图（与缩略图扩展调用同一个函数）；
# 再临时登记缩略图扩展，用访达同样的接口（ql-probe）和 qlmanage 给 QOI、CLIP 要缩略图，看系统用不用它（只记录），
# 做完马上取消登记、清掉 Quick Look 的缓存。
step "访达缩略图：核心给测试图片生成缩略图" selftest-thumbs.txt \
    self_test thumbs --open "${fixtures}/zthumbs/quadrants.qoi" \
    --steps "file_thumbnails:${fixtures}/zthumbs,file_thumbnails:${fixtures}/formats,file_thumbnails:${fixtures}/modern,file_thumbnails:${fixtures}/raw" || failed=1
# 系统的命令（pluginkit、qlmanage）有时一直不返回：每个都限时，超时就停掉、记下来。
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
ql_check() {
    local appex="${app_local}/Contents/PlugIns/AnyviewThumbnail.appex" out="${testdata}/ql-out" file since
    local lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
    rm -rf "${out}" "${build}/ql-probe.txt"
    mkdir -p "${out}" || return 1
    since="$(date '+%Y-%m-%d %H:%M:%S')"
    # 扩展本身：签名、权限、链接的库（系统拒绝启动时多半是这里的问题）。
    echo "== 扩展的签名和权限"
    codesign -dv --verbose=2 "${appex}" 2>&1 | grep -E '^(Identifier|Format|Signature|TeamIdentifier|Sealed|Info.plist|Runtime)'
    codesign -d --entitlements - "${appex}" 2>&1 | grep -v '^Executable='
    echo "== 链接的库"
    otool -L "${appex}/Contents/MacOS/AnyviewThumbnail" | tail -n +2
    # 系统（LaunchServices）登记的 Anyview 有哪几份：同一个 Bundle ID 有几份时，系统可能用的是别的那份。
    echo "== 系统登记的 Anyview"
    if [[ -x "${lsregister}" ]]; then
        with_timeout 30 "${lsregister}" -dump 2>/dev/null | grep -iE '^path:.*anyview' | sort -u | head -n 20
        # 取消旧输出目录中 Anyview.app 的 LaunchServices 登记，避免扩展冲突。
        # 这一步只看临时登记的这一份（以后运行或在访达里看到时系统会再登记，不影响使用）。
        if [[ -d "${app_share}" ]]; then
            echo "== 清理旧输出目录的程序登记：$(with_timeout 30 "${lsregister}" -u "${app_share}" 2>&1 && echo 完成)"
        fi
    fi
    echo "== pluginkit -a"
    with_timeout 15 pluginkit -a "${appex}"
    echo "== pluginkit -m（本扩展；+ 是用户打开了，- 是关掉了，空着是默认）"
    with_timeout 15 pluginkit -m -v -i local.anyview.Anyview.Thumbnail
    echo "== 系统里全部的缩略图扩展"
    with_timeout 15 pluginkit -m -v -p com.apple.quicklook.thumbnail
    with_timeout 15 qlmanage -r cache >/dev/null 2>&1
    # 访达用的接口：QOI、CLIP 由扩展生成，PNG 是系统自己做的对照（它也拿不到时说明这个接口在这里用不了）。
    echo "== ql-probe（QLThumbnailGenerator，访达用的接口）"
    # zall/（各种格式放在一起，用户  提议）每个文件都要一次：哪些有缩略图、哪些没有。
    # 系统自己做的、扩展做的都算；raw_ 开头的不是真的 RAW，系统不认也正常。
    with_timeout 400 "${swift_out}/ql-probe" 5 "${fixtures}"/zall/* </dev/null >"${build}/ql-probe.txt" 2>&1
    cat "${build}/ql-probe.txt"
    # qlmanage 交给 quicklookd 做（-x，和访达一样在系统的进程里）；不加 -x 时上次一直不返回。
    for file in "${fixtures}/zthumbs/quadrants.qoi" "${fixtures}/modern/clip_canvas.clip"; do
        echo "== mdls $(basename "${file}")：$(with_timeout 10 mdls -raw -name kMDItemContentType "${file}" 2>&1)"
        echo "== qlmanage -t -x $(basename "${file}")"
        with_timeout 8 qlmanage -t -x -s 256 -o "${out}" "${file}" </dev/null
    done
    echo "== pluginkit -r"
    with_timeout 15 pluginkit -r "${appex}"
    with_timeout 15 qlmanage -r cache >/dev/null 2>&1
    ls -la "${out}"
    # 扩展有没有被启动、为什么失败：这一步开始以来 Quick Look、PlugInKit 的记录（RunningBoard 只要提到 Anyview 的），
    # 以及扩展的崩溃报告。
    # 系统实际启动的是哪一份扩展：“应用程序”里装着 Anyview 时，系统用装好的那份，不用这次编的。
    with_timeout 60 log show --start "${since}" --style compact --predicate 'process == "com.apple.quicklook.ThumbnailsAgent" AND eventMessage CONTAINS "AnyviewThumbnail.appex"' 2>/dev/null \
        | grep -o 'URL: file://[^ ,]*AnyviewThumbnail.appex' | sed 's#URL: file://##' | sort -u >"${build}/ql-served.txt"
    echo "== 系统启动的缩略图扩展：$(tr '\n' ' ' <"${build}/ql-served.txt")"
    echo "== 系统日志（这一步开始以来）"
    with_timeout 60 log show --start "${since}" --style compact --predicate 'process != "Anyview" AND (process IN {"AnyviewThumbnail", "ql-probe", "qlmanage", "quicklookd", "com.apple.quicklook.ThumbnailsAgent", "QuickLookUIService", "pkd"} OR subsystem BEGINSWITH "com.apple.quicklook" OR subsystem == "com.apple.PlugInKit" OR eventMessage CONTAINS[c] "anyview")' 2>&1 | tail -n 300
    echo "== 崩溃报告"
    local crash
    for crash in "${HOME}/Library/Logs/DiagnosticReports"/AnyviewThumbnail*; do
        [[ -f "${crash}" ]] || continue
        echo "-- ${crash}"
        head -n 60 "${crash}"
    done
    return 0
}
step "访达缩略图：临时登记扩展、用 Quick Look 生成缩略图（只记录）" selftest-ql.txt ql_check || true
# 打开方式：每种支持的扩展名，系统（LaunchServices）列出的能打开它的程序里要有这一份 Anyview（只问，不改设置）。
step "打开方式：每种支持的扩展名都能用 Anyview 打开" selftest-openwith.txt \
    self_test openwith --open "${fixtures}/basic/transparent_800x600.png" --steps "open_with" || failed=1
# 背景图片、标题栏隐藏时显示文件名、帮助窗口：设好后窗口四周（和关掉图片后的空态）是背景图片，顶端显示文件名；
# 打开"键盘快捷键""支持的格式"两个帮助窗口；最后关掉背景图片。
background_steps="settings_open,settings_page:browse,settings_set:browse.background_image=true"
background_steps="${background_steps},settings_set:browse.background_image_path=${fixtures}/effects/2_blue.png"
background_steps="${background_steps},settings_set:browse.title_filename=true,settings_ok,wait:600,close_image,help:shortcuts,help:formats"
background_steps="${background_steps},settings_open,settings_page:browse,settings_set:browse.background_image=false,settings_ok"
# 开源许可放在最后，前面各步的序号不变。
background_steps="${background_steps},help:licenses"
step "背景图片、标题栏隐藏时显示文件名、帮助窗口" selftest-background.txt \
    self_test background --open "${fixtures}/sort/1.png" --steps "${background_steps}" || failed=1
# 智能双页阅览：两张竖图一组、横图单独、往回翻、左←右、关掉（每一步按合成的画面核对）。
step "双页阅览：两页一组、横图单独、往回翻、左←右" selftest-pages.txt \
    self_test pages --open "${fixtures}/pages/01.png" --steps "key:7,next_image,next_image,prev_image,prev_image,key:6,key:6" \
    || failed=1
# 文件操作：在临时副本上做。编辑器、桌面图片、打印只记下交出的内容，不真的打开程序、不改你的桌面、不打印。
fileops="${testdata}/fileops"
rm -rf "${fileops}"
mkdir -p "${fileops}/work" "${fileops}/f1" "${fileops}/f2"
cp "${fixtures}/sort/1.png" "${fixtures}/sort/2.png" "${fixtures}/sort/10.png" "${fileops}/work/"
fileops_steps="settings_open,settings_page:misc,settings_set:misc.folder1=${fileops}/f1"
fileops_steps="${fileops_steps},settings_set:misc.folder2=${fileops}/f2,settings_set:misc.editor=/System/Applications/Preview.app"
fileops_steps="${fileops_steps},settings_set:misc.no_screensaver=true,settings_ok,copy_to_folder1,copy_to_folder1"
fileops_steps="${fileops_steps},copy_to_folder2,open_in_editor,set_wallpaper,print,move_to_folder1"
fileops_steps="${fileops_steps},add_file:${fixtures}/sort/中文.png,remove_file:10.png"
step "文件操作：复制、移动、编辑器、桌面图片、打印（只记录）、文件夹变化自动刷新" selftest-fileops.txt \
    self_test fileops --open "${fileops}/work/1.png" --steps "${fileops_steps}" || failed=1
# 动图：GIF 播放、空格暂停、逐帧前后、APNG、GIF 透明处的背景色（文件里的品红，再改成白色）、
# 隐藏动画信息控制。每一步都按屏幕上正在显示的那一帧核对画面。
anim_steps="wait_frames:3,key:Space,wait:300,key:Period,key:Comma,next_image,wait_frames:2,next_image,key:Space"
anim_steps="${anim_steps},settings_open,settings_page:image,settings_set:image.gif_background=white,settings_ok"
anim_steps="${anim_steps},settings_open,settings_page:browse,settings_set:browse.hide_animation_info=true,settings_ok"
step "动图：播放、暂停、逐帧、APNG、GIF 透明背景、隐藏动画信息控制" selftest-anim.txt \
    self_test anim --open "${fixtures}/anim/a_frames.gif" --steps "${anim_steps}" || failed=1
# 窗口：从访达打开时开新窗口、改设置两个窗口都变、Esc 只关第二个窗口；勾上"复用当前窗口"后在原窗口换图；
# 吸附边缘关着时不动、打开后拖到边缘附近贴上去（也用模拟的鼠标拖一次）；显示标题栏后窗口按图片加标题栏；
# 最后一个窗口里按 Esc 退出（自检记下后结束）。
windows_steps="finder_open:${fixtures}/sort/1.png"
windows_steps="${windows_steps},settings_open,settings_page:browse,settings_set:browse.background_color=#336699,settings_ok"
windows_steps="${windows_steps},key_in:1:Escape"
windows_steps="${windows_steps},settings_open,settings_page:general,settings_set:general.single_instance=true,settings_ok"
windows_steps="${windows_steps},finder_open:${fixtures}/sort/2.png,move_window:7:stay"
windows_steps="${windows_steps},settings_open,settings_page:general,settings_set:general.snap_window=true"
windows_steps="${windows_steps},settings_page:browse,settings_set:browse.show_main_window=true,settings_ok"
windows_steps="${windows_steps},next_image,move_window:7:snap,move_window:40:stay,drag_window:9:snap,key:Escape"
step "窗口：从访达打开开新窗口、复用当前窗口、吸附边缘、显示标题栏、Esc" selftest-windows.txt \
    self_test windows --open "${fixtures}/view/quadrants_600x400.png" --steps "${windows_steps}" || failed=1
# 安全上限：翻到解压后 65 MB 的 SVGZ 时先被拦下（不弹对话框，自检记一步），"仍然打开"后显示出来。
step "安全上限：被拦下的文件询问后仍然打开" selftest-limits.txt \
    self_test limits --open "${fixtures}/limits/a.png" --steps "next_image,limit_open" || failed=1
# 窗口看不见、全屏时关窗口：打开“阻止显示器睡眠”后，最小化就不阻止、取回再阻止；
# 第二个窗口全屏时按 Esc 关掉它，菜单栏、程序坞不再自动隐藏。
visible_steps="settings_open,settings_page:misc,settings_set:misc.no_screensaver=true,settings_ok"
visible_steps="${visible_steps},miniaturize,deminiaturize"
visible_steps="${visible_steps},finder_open:${fixtures}/sort/1.png,key_in:1:Enter,key_in:1:Escape"
step "窗口：最小化时不阻止睡眠、全屏时关窗口" selftest-visible.txt \
    self_test visible --open "${fixtures}/view/quadrants_600x400.png" --steps "${visible_steps}" || failed=1
broken_settings="${testdata}/broken-settings"
rm -rf "${broken_settings}"
mkdir -p "${broken_settings}"
printf 'version = 1\n[general\nconfirm_delete = false\n' >"${broken_settings}/settings.toml"
step "设置：设置文件损坏时备份、用默认值并提示" selftest-settings-broken.txt \
    self_test settings_broken --open "${fixtures}/basic/transparent_800x600.png" \
    --settings "${broken_settings}/settings.toml" --steps "settings_open,settings_cancel" || failed=1
real_ready=0
if [[ -d "${real_share}" ]]; then
    if step "准备真实测试图片（复制到本机、核对 SHA-256）" real-images.txt prepare_real; then
        real_ready=1
    else
        failed=1
    fi
else
    say ""
    say "（未找到真实测试图片 ${real_share}，跳过真实图片与性能项；可运行 tools/testdata/fetch.py 获取）"
fi
perf="${testdata}/perf"
if (( real_ready == 1 )); then
    real_steps="zoom_100,zoom_lock,toggle_info,next_image,next_image,next_image,next_image,next_image,next_image,next_image,next_image,next_image"
    step "真实图片：10 张开放许可的照片，逐张以 100% 打开核对" selftest-real.txt \
        self_test real --open "${real}/gif_ozone.gif" --sample-at-100 --steps "${real_steps}" || failed=1
    # 性能：10 份相同的 1200 万像素照片（a–j）、另一个文件夹里的 x，以及 1 万个小文件的文件夹。
    rm -rf "${perf}"
    mkdir -p "${perf}/work" "${perf}/other"
    # 10 份：预读默认往后 5 张，跳到末张时它不在预读范围里，测得的是未缓存切图耗时。
    for name in a b c d e f g h i j; do
        cp "${real}/photo_12mp_iguana.jpg" "${perf}/work/${name}.jpg"
    done
    cp "${real}/photo_12mp_iguana.jpg" "${perf}/other/x.jpg"
    perf_steps="wait:1500,bar_jump:1.0,wait:1500,prev_image,interp_lanczos,wait_exact,interp_bilinear,idle:3000"
    perf_steps="${perf_steps},open:${perf}/other/x.jpg,open:${fixtures}/many"
    step "性能：冷启动、切图、精确重绘、空闲占用、1 万张的文件夹" selftest-perf.txt \
        self_test perf --open "${perf}/work/a.jpg" --sample-at-100 --steps "${perf_steps}" || failed=1
    # 冷启动再测 4 次，共 5 次取中位数：系统建应用的那一步（NSApplication.shared）每次在 100–250 ms 之间跳，
    # 只测一次时数字忽高忽低。
    t1_repeats() {
        local i status=0
        for i in 2 3 4 5; do
            self_test "t1-${i}" --open "${perf}/work/a.jpg" --sample-at-100 || status=1
        done
        return "${status}"
    }
    step "冷启动再测 4 次（取 5 次中位数）" selftest-t1.txt t1_repeats || failed=1
fi

# 文件操作在 Mac 临时目录里的副本上做；移到废纸篓后程序会立刻从废纸篓删掉，不留东西。
files_work="${testdata}/files-work"
rm -rf "${files_work}"
mkdir -p "${files_work}"
cp "${fixtures}/sort/1.png" "${fixtures}/sort/2.png" "${fixtures}/sort/10.png" "${files_work}/"
seed_settings="${ask_at_ends}" step "文件：复制到剪贴板、移到废纸篓（用临时副本）" selftest-files.txt \
    self_test files --open "${files_work}/2.png" \
    --steps "invert,copy_to_clipboard,invert,delete_file,next_image,first_image,delete_file" || failed=1

# 浏览顺序：自然排序应为 1, 2, 10, …, 中文, 1。
expected="1.png 2.png 10.png 中文.png 1.png"
order=""
for i in 0 1 2 3 4; do
    order="${order}$(json_value "${results}/nav/selftest.json" "steps.${i}.name") "
done
order="${order% }"
if [[ "${order}" == "${expected}" ]]; then
    say "  ✓ 浏览顺序：${order}"
else
    say "  ✗ 浏览顺序不对：${order}（应为 ${expected}）"
    failed=1
fi
photo_decode="$(json_value "${results}/photo/selftest.json" steps.0.decodeMs | cut -d. -f1)"
photo_shown="$(json_value "${results}/photo/selftest.json" steps.0.msToImage | cut -d. -f1)"
say "  大图：解码 ${photo_decode:-?} ms，打开到出图 ${photo_shown:-?} ms"
fit_w="$(json_value "${results}/photo/selftest.json" steps.0.fitWindowPt.0 | cut -d. -f1)"
fit_h="$(json_value "${results}/photo/selftest.json" steps.0.fitWindowPt.1 | cut -d. -f1)"
if [[ -n "${fit_w}" ]]; then
    say "  ✓ 窗口按图片调整：4000x3000 的照片打开后窗口为 ${fit_w}x${fit_h} 点"
else
    say "  ✗ 打开图片后窗口没有按图片调整"
    failed=1
fi
view_checked="$(json_value "${results}/view/selftest.json" samplesChecked)"
view_failure="$(json_value "${results}/view/selftest.json" failureText)"
say "  看图操作：画面核对 ${view_checked:-0} 个点${view_failure:+，失败原因：${view_failure}}"
# 进度条跳转：从 7/7 跳到 4/7（plain_640x480.bmp）、再跳到 2/7（palette_320x240.gif）。
jumps="$(json_value "${results}/overlay/selftest.json" steps.3.name) $(json_value "${results}/overlay/selftest.json" steps.4.name)"
if [[ "${jumps}" == "plain_640x480.bmp palette_320x240.gif" ]]; then
    say "  ✓ 悬浮界面：进度条跳转 ${jumps}"
else
    say "  ✗ 进度条跳转不对：${jumps}（应为 plain_640x480.bmp palette_320x240.gif）"
    failed=1
fi
# 浏览：降序后首张是 中文.png，随机后退回，下一个文件夹是 view/，再回到 sort/，末张询问后回到首张。
browse=""
for i in 2 5 6 7 10; do
    browse="${browse}$(json_value "${results}/browse/selftest.json" "steps.${i}.name") "
done
browse="${browse% }"
if [[ "${browse}" == "中文.png 中文.png quadrants_600x400.png 1.png 1.png" ]]; then
    say "  ✓ 浏览：降序、随机退回、相邻文件夹、末张询问都对"
else
    say "  ✗ 浏览结果不对：${browse}"
    failed=1
fi
left="$(ls "${files_work}" 2>/dev/null | tr '\n' ' ')"
if [[ "${left}" == "10.png " ]]; then
    say "  ✓ 文件：删掉了 1.png、2.png，剩下 10.png；废纸篓里没留东西"
else
    say "  ✗ 文件操作不对：剩下 ${left}"
    failed=1
fi
rm -rf "${files_work}"
exact_count="$(json_value "${results}/quality/selftest.json" exactCount)"
exact_worst="$(json_value "${results}/quality/selftest.json" exactWorstMs | cut -d. -f1)"
if [[ "${exact_count}" == "7" ]]; then
    say "  ✓ 高品质：精确重绘 7 次，最慢 ${exact_worst:-?} ms"
else
    say "  ✗ 精确重绘次数不对：${exact_count:-0}（应为 7）"
    failed=1
fi
color_center="$(json_value "${results}/color/selftest.json" steps.0.samples.centerColor)"
color_value=0
if [[ "${color_center}" == \#* ]]; then
    color_value=$(( 16#${color_center:1:2} ))
fi
if (( color_value >= 184 && color_value <= 192 )); then
    say "  ✓ 色彩：灰度 128 按线性 ICC 显示为 ${color_center}（约 188）"
else
    say "  ✗ 色彩不对：${color_center:-?}（灰度 128 按线性 ICC 应约为 #BCBCBC）"
    failed=1
fi
for format in heic avif; do
    if [[ -f "${fixtures}/apple/quadrants.${format}" ]]; then
        shown="$(json_value "${results}/${format}/selftest.json" steps.0.format)"
        say "  ✓ ${format}：ImageIO 解码，格式 ${shown:-?}"
    else
        say "  ${format}：这台 Mac 的 sips 不能写这种格式，没有自检（打开这种文件仍可用）"
    fi
done
# 窗口大小的变化：打开 → 放大（不变）→ 旋转（变竖）→ 最大化 → 切图（保持）→ 还原（回到图片大小）→ 拖动 → 切图（回到图片大小）。
window_sizes=""
drags=""
for i in 0 1 2 3 4 5 6 7 8 9 10; do
    case "$(json_value "${results}/window/selftest.json" "steps.${i}.leftDragMovesWindow")" in
        true) drags="${drags}窗口 " ;;
        false) drags="${drags}平移 " ;;
    esac
    width="$(json_value "${results}/window/selftest.json" "steps.${i}.windowPt.0" | cut -d. -f1)"
    height="$(json_value "${results}/window/selftest.json" "steps.${i}.windowPt.1" | cut -d. -f1)"
    [[ -n "${width}" ]] || break
    window_sizes="${window_sizes}${window_sizes:+ → }$(json_value "${results}/window/selftest.json" "steps.${i}.step") ${width}x${height}"
done
window_failure="$(json_value "${results}/window/selftest.json" failureText)"
if [[ -z "${window_failure}" && -n "${window_sizes}" ]]; then
    say "  ✓ 窗口跟着图片（点）：${window_sizes}"
else
    say "  ✗ 窗口跟着图片：${window_failure:-没有结果}（${window_sizes}）"
    failed=1
fi
# 左键拖动：打开时（图片完整显示）移动窗口，放大后、最大化时平移图片，模拟拖动窗口后又是移动窗口，
# 全屏时平移，退出全屏后又是移动窗口。
if [[ "${drags% }" == "窗口 平移 平移 平移 平移 窗口 窗口 窗口 平移 平移 窗口" ]]; then
    say "  ✓ 左键拖动：图片完整显示时移动窗口，放大后、最大化、全屏时平移图片"
else
    say "  ✗ 左键拖动的判断不对：${drags}"
    failed=1
fi
# 设置：步骤序号见上面的 settings_steps（0 是打开图片）：9 填错后确定，15 确定，16 缩小，17 指针移到底部，
# 18 按 Esc，19 再打开设置，22 放弃修改。
report="${results}/settings/selftest.json"
settings_problems=""
settings_problem() {
    settings_problems="${settings_problems}${settings_problems:+；}$1"
}
[[ "$(json_value "${report}" steps.9.errorPage)" == "image" ]] \
    || settings_problem "缓存大小填 50 时没有拦下（$(json_value "${report}" steps.9.error)）"
[[ "$(json_value "${report}" steps.15.uiSettings.backgroundColor)" == "#336699" ]] || settings_problem "背景色没有生效"
[[ "$(json_value "${report}" steps.15.uiSettings.hideControlBar)" == "true" ]] || settings_problem "不显示控制栏没有生效"
[[ "$(json_value "${report}" steps.15.uiSettings.hideZoomInfo)" == "true" ]] || settings_problem "不显示倍率没有生效"
[[ "$(json_value "${report}" steps.15.uiSettings.alphaBackground)" == "white" ]] || settings_problem "白色透明背景没有生效"
[[ "$(json_value "${report}" steps.15.lang)" == "en" ]] || settings_problem "语言没有换成英文"
settings_text="$(json_value "${report}" steps.15.settingsText)"
for needle in 'background_color = "#336699"' 'esc_quits = false' 'cache_mb = 512' 'language = "en"'; do
    [[ "${settings_text}" == *"${needle}"* ]] || settings_problem "设置文件里没有 ${needle}"
done
corner="$(json_value "${report}" steps.16.cornerColor)"
[[ "${corner}" == "#336699" ]] || settings_problem "缩小后背景色是 ${corner:-?}，应为 #336699"
[[ "$(json_value "${report}" steps.17.overlays.barShown)" == "false" ]] || settings_problem "设置了不显示控制栏，指针移到底部时还是显示了"
escape_action="$(json_value "${report}" steps.18.action)"
[[ -z "${escape_action}" ]] || settings_problem "关掉“按 ESC 退出”后 Esc 还对应 ${escape_action}"
[[ "$(json_value "${report}" steps.19.settingsPages.0)" == "general General" ]] || settings_problem "再打开设置时不是英文"
[[ "$(json_value "${report}" steps.22.dirtyBeforeCancel)" == "true" ]] || settings_problem "放弃修改前草稿没有改动"
reload="${results}/settings_reload/selftest.json"
[[ "$(json_value "${reload}" steps.1.uiSettings.backgroundColor)" == "#336699" && "$(json_value "${reload}" steps.1.lang)" == "en" ]] \
    || settings_problem "重新启动后设置没有保留"
[[ "$(json_value "${reload}" steps.3.cornerColor)" == "#336699" ]] || settings_problem "重新启动后背景色不对"
broken_backup="$(ls "${broken_settings}" 2>/dev/null | grep '^settings-broken-' | head -n 1)"
broken_notice="$(json_value "${results}/settings_broken/selftest.json" steps.0.notice)"
[[ -n "${broken_backup}" && "${broken_notice}" == *settings-broken-* ]] \
    || settings_problem "损坏的设置文件没有备份或没有提示（${broken_backup:-没有备份}；${broken_notice:-没有提示}）"
if [[ -z "${settings_problems}" ]]; then
    say "  ✓ 设置：缓存大小填 50 被拦下；背景色、隐藏控制栏和倍率、白色透明背景、Esc 不退出、英文都生效并写进文件；"
    say "    重新启动后保留；损坏的文件已备份（${broken_backup}）并提示。设置窗口的截图在 settings/step-*-settings.png"
else
    say "  ✗ 设置：${settings_problems}"
    failed=1
fi
rm -rf "${broken_settings}"
# 映射：步骤序号见上面的 mapping_steps（0 是打开图片）。
report="${results}/mapping/selftest.json"
mapping_problems=""
mapping_expect() {  # mapping_expect <步骤> <应有的动作> [应有的图片]
    local action name
    action="$(json_value "${report}" "steps.$1.action")"
    name="$(json_value "${report}" "steps.$1.name")"
    if [[ "${action}" != "$2" || ( -n "${3:-}" && "${name}" != "$3" ) ]]; then
        mapping_problems="${mapping_problems}${mapping_problems:+；}第 $1 步动作 ${action:-无}、图片 ${name:-?}（应为 ${2:-无} ${3:-}）"
    fi
}
mapping_expect 11 zoom_in
mapping_expect 12 zoom_out
mapping_expect 13 ""
mapping_expect 14 zoom_in
mapping_expect 15 next_image tall_400x3000.png
mapping_expect 16 prev_image quadrants_600x400.png
mapping_expect 17 ""
mapping_expect 18 maximize
mapping_rows=""
for i in 0 1 2 3 4; do
    mapping_rows="${mapping_rows} $(json_value "${report}" "steps.5.settings.page.items.0.value.${i}")"
done
[[ "${mapping_rows}" == *"将替代"* || "${mapping_rows}" == *"Replaces"* ]] \
    || mapping_problems="${mapping_problems}${mapping_problems:+；}→ 那一行没有提示将替代下一张"
if [[ -z "${mapping_problems}" ]]; then
    say "  ✓ 映射：Q 放大、→ 缩小（提示将替代下一张）、关掉的 PageDown 不再翻文件夹、滚轮向下放大、右键下一张、"
    say "    单击左侧翻回上一张、两侧双击不最大化、中间双击最大化"
else
    say "  ✗ 映射：${mapping_problems}"
    failed=1
fi
# 压缩包：步骤序号见上面的 archive_steps（0 是打开压缩包）。
report="${results}/archive/selftest.json"
archive_problems=""
archive_note() { archive_problems="${archive_problems}${archive_problems:+；}$1"; }
archive_names=""
for i in 0 1 2 4 5 6 7; do
    archive_names="${archive_names}$(json_value "${report}" "steps.${i}.name")|"
done
[[ "${archive_names}" == "a_book.cbz › 1.png|a_book.cbz › 2.png|a_book.cbz › 3.png|b_book.cb7 › 1.png|b_book.cb7 › 2.png|c_book.cbr › 1.png|a_book.cbz › 3.png|" ]] \
    || archive_note "翻页、换压缩包的顺序不对：${archive_names}"
[[ "$(json_value "${report}" steps.3.boundary)" == "last" ]] || archive_note "ZIP 末尾没有询问"
[[ "$(json_value "${report}" steps.0.deleteEnabled)" == "false" ]] || archive_note "压缩包里的图片能删除"
[[ -f "${testdata}/archive-copies/3.png" ]] || archive_note "没有从压缩包里复制出来"
[[ "$(json_value "${report}" steps.15.archivePositions)" == "0" ]] || archive_note "没有清掉记住的位置"
if [[ -z "${archive_problems}" ]]; then
    say "  ✓ 压缩包：ZIP 里翻页（跳过 __MACOSX 和隐藏文件）、末尾询问、下一个压缩包到 7Z（固实）、RAR，再打开 ZIP 回到上次的第 3 页；"
    say "    压缩包里不能删除；复制出的 3.png 在图片文件夹里；清掉了记住的位置"
else
    say "  ✗ 压缩包：${archive_problems}"
    failed=1
fi
# 背景图片等：步骤序号见上面的 background_steps（0 是打开图片）。
report="${results}/background/selftest.json"
background_problems=""
background_note() { background_problems="${background_problems}${background_problems:+；}$1"; }
[[ "$(json_value "${report}" steps.7.cornerColor)" == "#2828C8" && "$(json_value "${report}" steps.7.background)" == "true" ]] \
    || background_note "设好背景图片后四周不是它（$(json_value "${report}" steps.7.cornerColor)）"
[[ "$(json_value "${report}" steps.7.filenameLabel)" == "1.png" ]] || background_note "标题栏隐藏时没有显示文件名"
[[ "$(json_value "${report}" steps.8.cornerColor)" == "#2828C8" ]] || background_note "空态的背景不是背景图片"
background_rows="$(json_value "${report}" steps.9.help.rows)"
[[ "$(json_value "${report}" steps.9.help.title)" =~ ^(键盘快捷键|Keyboard\ Shortcuts)$ && "${background_rows:-0}" -ge 20 ]] \
    || background_note "键盘快捷键窗口不对"
[[ "$(json_value "${report}" steps.10.help.headings)" == "5" ]] || background_note "支持的格式窗口不对"
[[ "$(json_value "${report}" steps.14.cornerColor)" == "#202020" && "$(json_value "${report}" steps.14.background)" == "false" ]] \
    || background_note "关掉背景图片后四周不是背景色"
licenses_length="$(json_value "${report}" steps.15.help.textLength)"
[[ "$(json_value "${report}" steps.15.help.title)" =~ ^(开源许可|Open\ Source\ Licenses)$ && "${licenses_length:-0}" -ge 100000 \
    && "$(json_value "${report}" steps.15.help.anyviewLicense)" == "true" ]] || background_note "开源许可窗口不对"
if [[ -z "${background_problems}" ]]; then
    say "  ✓ 背景图片：图片四周和空态都是背景图片，关掉后回到背景色；标题栏隐藏时顶端显示文件名；"
    say "    帮助窗口：键盘快捷键 ${background_rows} 行、支持的格式 5 类、开源许可 $(( ${licenses_length:-0} / 1024 )) KB"
else
    say "  ✗ 背景图片等：${background_problems}"
    failed=1
fi
# 翻页效果：步骤序号见上面的 effects_steps（0 是打开图片）。
report="${results}/effects/selftest.json"
effects_problems=""
effects_note() { effects_problems="${effects_problems}${effects_problems:+；}$1"; }
for pair in "8 fade" "15 slide" "22 page"; do
    effects_step="${pair%% *}"
    effects_name="${pair#* }"
    [[ "$(json_value "${report}" "steps.${effects_step}.samples.transition.effect")" == "${effects_name}" \
        && "$(json_value "${report}" "steps.${effects_step}.samples.mismatches")" == "0" ]] \
        || effects_note "${effects_name} 停在一半的画面不对"
done
[[ "$(json_value "${report}" steps.24.name)" == "1_red.png" && -n "$(json_value "${report}" steps.24.samples.checked)" ]] \
    || effects_note "真的翻一页后画面不对"
[[ "$(json_value "${report}" steps.24.sounds.3)" == "NSSound（自检不出声）" && -z "$(json_value "${report}" steps.24.sounds.4)" ]] \
    || effects_note "翻页音效应记下 4 次"
if [[ -z "${effects_problems}" ]]; then
    say "  ✓ 翻页效果：淡出、滑动、翻页停在一半时的颜色都对；真的翻一页后画面核对相符；翻页音效 4 次（自检不出声）"
else
    say "  ✗ 翻页效果：${effects_problems}"
    failed=1
fi
# 更多格式：44 张都要显示出来；几个 RAW 按方向摆正后的尺寸。
report="${results}/formats/selftest.json"
formats_problems=""
formats_list=""
for i in $(seq 0 43); do
    formats_name="$(json_value "${report}" "steps.${i}.name")"
    formats_format="$(json_value "${report}" "steps.${i}.format")"
    formats_size="$(json_value "${report}" "steps.${i}.width")x$(json_value "${report}" "steps.${i}.height")"
    if [[ -z "${formats_format}" ]]; then
        formats_problems="${formats_problems}${formats_problems:+；}第 ${i} 步没有显示出来（${formats_name}）"
    fi
    case "${formats_name}" in
        e_fuji.raf) [[ "${formats_size}" == "720x1080" ]] || formats_problems="${formats_problems}；RAF 方向不对 ${formats_size}" ;;
        g_rotated.dng) [[ "${formats_size}" == "768x1024" ]] || formats_problems="${formats_problems}；DNG 方向不对 ${formats_size}" ;;
    esac
    formats_list="${formats_list} ${formats_format}"
done
if [[ -z "${formats_problems}" ]]; then
    say "  ✓ 更多格式：44 张都显示出来、画面核对相符；格式：$(echo ${formats_list} | tr ' ' '\n' | sort -u | tr '\n' ' ')"
else
    say "  ✗ 更多格式：${formats_problems}"
    failed=1
fi
# 生成信息：面板跟着图片换内容；没有生成信息的 plain.png 上面板显示“没有”、P 不可用；最后关掉；图片信息里有 AI 生成那一行。
report="${results}/generation/selftest.json"
generation_problems=""
check_generation() {  # check_generation <步骤> <生成工具> <提示词>
    local open tool prompt enabled
    open="$(json_value "${report}" "steps.$1.generation.open")"
    tool="$(json_value "${report}" "steps.$1.generation.tool")"
    prompt="$(json_value "${report}" "steps.$1.generation.prompt")"
    enabled="$(json_value "${report}" "steps.$1.generation.enabled")"
    if [[ "${open}" != "true" || "${tool}" != "$2" || "${prompt}" != "$3" || "${enabled}" != "true" ]]; then
        generation_problems="${generation_problems}${generation_problems:+；}第 $1 步（$(json_value "${report}" "steps.$1.name")）：${tool:-没有}「${prompt}」"
    fi
}
check_generation 3 "ComfyUI" "a red fox in snow"
check_generation 5 "Stable Diffusion WebUI" "a cozy cabin, warm light, 雪"
check_generation 7 "Stable Diffusion WebUI" "a cozy cabin, warm light, 雪"
if [[ "$(json_value "${report}" steps.11.generation.open)" != "true" || -n "$(json_value "${report}" steps.11.generation.tool)" \
    || "$(json_value "${report}" steps.11.generation.enabled)" != "false" ]]; then
    generation_problems="${generation_problems}${generation_problems:+；}没有生成信息的 plain.png 上不对"
fi
if [[ "$(json_value "${report}" steps.12.generation.open)" != "false" ]]; then
    generation_problems="${generation_problems}${generation_problems:+；}面板没有关掉"
fi
generation_line=""
for k in 0 1 2 3 4 5 6 7 8; do
    line="$(json_value "${report}" "steps.2.overlays.infoText.${k}")"
    [[ "${line}" == *ComfyUI* ]] && generation_line="${line}"
done
[[ -n "${generation_line}" ]] || generation_problems="${generation_problems}${generation_problems:+；}图片信息里没有 AI 生成那一行"
if [[ -z "${generation_problems}" ]]; then
    say "  ✓ 生成信息：按 P 打开面板，ComfyUI 的 PNG、WebP 和 JPEG 的 EXIF 里的 A1111 参数都显示出来，翻到没有的图片时显示“没有”；图片信息：${generation_line}"
else
    say "  ✗ 生成信息：${generation_problems}"
    failed=1
fi
# 打开方式（Info.plist 的文档类型）：步骤 1 是 open_with。通过与否看程序有没有声明每种扩展名；
# 临时目录中的开发版不一定会进入系统“打开方式”列表，此处仅作记录。
report="${results}/openwith/selftest.json"
openwith_checked="$(json_value "${report}" steps.1.openWith.checked)"
openwith_listed="$(json_value "${report}" steps.1.openWith.listedCount)"
openwith_undeclared="$(plutil -extract steps.1.openWith.notDeclared json -o - "${report}" 2>/dev/null | tr -d '[]"' | tr ',' ' ')"
openwith_missing="$(plutil -extract steps.1.openWith.missing json -o - "${report}" 2>/dev/null | tr -d '[]"' | tr ',' ' ')"
openwith_dynamic="$(plutil -extract steps.1.openWith.dynamic json -o - "${report}" 2>/dev/null | tr -d '[]"' | tr ',' ' ')"
if [[ -z "${openwith_checked}" ]]; then
    say "  ✗ 打开方式：没有记录"
    failed=1
elif [[ -n "${openwith_undeclared// /}" ]]; then
    say "  ✗ 打开方式：程序没有声明能打开这些扩展名：${openwith_undeclared}"
    failed=1
elif [[ "${openwith_listed}" != "0" && -n "${openwith_missing// /}" ]]; then
    say "  ✗ 打开方式：系统列出了 Anyview，但这些扩展名没有：${openwith_missing}"
    failed=1
else
    say "  ✓ 打开方式：${openwith_checked} 种扩展名都声明了（系统里没有类型、按扩展名登记的：${openwith_dynamic:-无}）"
fi
if [[ "${openwith_listed}" == "0" ]]; then
    say "    系统现在没有把这份 Anyview 列进“打开方式”（在临时目录里，系统不列；从 DMG 装进“应用程序”后才列出）"
    say "    系统按 Bundle ID 找到的 Anyview：$(json_value "${report}" steps.1.openWith.systemAppURL)"
else
    say "    系统列出 Anyview 的扩展名：${openwith_listed} 种；列出的是：$(plutil -extract steps.1.openWith.listedCopies json -o - "${report}" 2>/dev/null | tr -d '\n')"
fi
if [[ -s "${build}/failed-extensions.txt" ]]; then
    say "  ✗ 没编译成功：$(tr '\n' ' ' <"${build}/failed-extensions.txt")（错误见 swift-build.txt；程序里先不放，其他检查照常做了）"
    failed=1
fi
# 访达缩略图：步骤 1–4 是四个目录；类型调查的结果；Quick Look 实际生成了几张。
report="${results}/thumbs/selftest.json"
thumbs_total=0
thumbs_made=0
thumbs_failed=""
for i in 1 2 3 4; do
    thumbs_total=$(( thumbs_total + $(json_value "${report}" "steps.${i}.thumbnails.total" || echo 0) ))
    thumbs_made=$(( thumbs_made + $(json_value "${report}" "steps.${i}.thumbnails.made" || echo 0) ))
    thumbs_failed="${thumbs_failed} $(plutil -extract "steps.${i}.thumbnails.failed" json -o - "${report}" 2>/dev/null | tr -d '[]"' | tr ',' ' ')"
done
if [[ "${thumbs_total}" -gt 0 && "${thumbs_made}" == "${thumbs_total}" ]]; then
    say "  ✓ 访达缩略图（核心）：${thumbs_made} 张测试图片都生成了缩略图"
else
    say "  ✗ 访达缩略图（核心）：${thumbs_made}/${thumbs_total}，没生成的：${thumbs_failed}"
    failed=1
fi
if [[ -f "${build}/thumbnail-types.txt" ]]; then
    say "    缩略图扩展声明的类型（这台 Mac 上系统自己解不了的）：$(tr '\n' ' ' <"${build}/thumbnail-types.txt")"
    [[ -f "${build}/preview-types.txt" ]] && say "    预览扩展声明的类型：$(tr '\n' ' ' <"${build}/preview-types.txt")"
    cat "${build}/thumbnail-survey.txt" >>"${results}/thumbnail-survey.txt"
fi
# 程序大小（两个扩展各自带一份核心，看看大了多少）。
size_of() { du -sk "$1" 2>/dev/null | awk '{ printf "%.1f", $1 / 1024 }'; }
say "    程序大小：$(size_of "${app_local}") MB（缩略图扩展 $(size_of "${app_local}/Contents/PlugIns/AnyviewThumbnail.appex") MB、预览扩展 $(size_of "${app_local}/Contents/PlugIns/AnyviewPreview.appex") MB）"
# 访达用的接口（ql-probe）每个文件拿到的缩略图；qlmanage -x 生成了几张。
ql_probe="$(awk -F'\t' '/^[^ ]/ && NF >= 2 { file = $1 }
    /^  缩略图/ { total++; if ($0 ~ /ms：[0-9]+x[0-9]+/) ok++; else { failed = failed sep file; sep = "、" } }
    END { if (total > 0) { printf "%d/%d 个文件有缩略图", ok, total; if (failed != "") printf "，没有的：%s", failed } }' \
    "${build}/ql-probe.txt" 2>/dev/null)"
ql_made="$(ls "${testdata}/ql-out" 2>/dev/null | grep -c '\.png$')"
say "    Quick Look 实测（临时登记扩展，只记录，详情见 selftest-ql.txt）：${ql_probe:-ql-probe 没有结果}"
# Anyview 自己做的那几种：有缩略图时四色块的位置要对。
ql_wrong="$(awk -F'\t' '/^[^ ]/ && NF >= 2 { file = $1 }
    /^  缩略图 [0-9]+ ms：[0-9]+x[0-9]+/ && file ~ /^(quadrants\.(qoi|pam)|clip_canvas\.clip|jxr_still\.jxr|pcx_(24|8)bit\.pcx|psb_rgb8_rle\.psb|b_frames\.apng)$/ && $0 !~ /四色块 对/ { printf "%s%s", sep, file; sep = "、" }' \
    "${build}/ql-probe.txt" 2>/dev/null)"
if [[ -s "${build}/ql-served.txt" ]] && ! grep -q "anyview-build" "${build}/ql-served.txt"; then
    # 系统用的是装好的那份（不是这次编的）：结果只说明装好的版本，不算这次的失败。
    say "    （系统用的是装好的那份扩展：$(tr '\n' ' ' <"${build}/ql-served.txt")；要测这次编的，先用 打包.command 做 DMG 装进去）"
    [[ -n "${ql_wrong}" ]] && say "    装好的那份画得不对：${ql_wrong}"
elif [[ -n "${ql_wrong}" ]]; then
    say "  ✗ Anyview 扩展做的缩略图画得不对（四色块的位置不对）：${ql_wrong}"
    failed=1
fi
say "    qlmanage -x 给 QOI、CLIP 生成了 ${ql_made:-0} 张"
# 选择图片窗口：步骤序号见上面的 picker_steps（0 是打开图片）。
report="${results}/picker/selftest.json"
picker_problems=""
picker_note() { picker_problems="${picker_problems}${picker_problems:+；}$1"; }
[[ "$(json_value "${report}" steps.1.picker.rows)" == "9" && "$(json_value "${report}" steps.1.picker.selected)" == "1" ]] \
    || picker_note "打开时没有选中当前图片"
picker_ready="$(json_value "${report}" steps.1.picker.thumbnailsReady)"
picker_visible="$(json_value "${report}" steps.1.picker.thumbnailsVisible)"
[[ -n "${picker_visible}" && "${picker_visible}" != "0" && "${picker_ready}" == "${picker_visible}" ]] \
    || picker_note "缩略图没有生成：${picker_ready}/${picker_visible}"
[[ "$(json_value "${report}" steps.2.picker.selectedName)" == "sort/B.png" && "$(json_value "${report}" steps.2.index)" == "1" ]] \
    || picker_note "单击后主窗口不该切换"
picker_label="$(json_value "${report}" steps.3.picker.sortLabel)"
[[ ( "${picker_label}" == "按大小排序" || "${picker_label}" == "Sort by Size" ) \
    && "$(json_value "${report}" steps.3.picker.selectedName)" == "sort/B.png" ]] || picker_note "排序后不对：${picker_label}"
[[ "$(json_value "${report}" steps.4.picker.stripVisible)" == "false" && "$(json_value "${report}" steps.4.picker.hideSetting)" == "true" ]] \
    || picker_note "隐藏缩略图导航没有生效"
[[ "$(json_value "${report}" steps.5.picker.stripVisible)" == "true" ]] || picker_note "再显示缩略图导航没有生效"
[[ "$(json_value "${report}" steps.6.name)" == "B.png" && -z "$(json_value "${report}" steps.6.picker.rows)" ]] \
    || picker_note "确定后没有切到 B.png 并关掉窗口"
[[ "$(json_value "${report}" steps.7.picker.selectedName)" == "sort/B.png" ]] || picker_note "再打开时没有选中当前图片"
[[ "$(json_value "${report}" steps.9.name)" == "B.png" && -z "$(json_value "${report}" steps.9.picker.rows)" ]] \
    || picker_note "取消后不该切换"
[[ "$(json_value "${report}" steps.11.picker.rows)" == "3" ]] || picker_note "压缩包里的列表不对"
picker_chosen="$(json_value "${report}" steps.12.picker.selectedName)"
picker_chosen="${picker_chosen##*/}"
[[ "$(json_value "${report}" steps.13.name)" == "a_book.cbz › ${picker_chosen}" ]] || picker_note "压缩包里确定后不对"
if [[ -z "${picker_problems}" ]]; then
    say "  ✓ 选择图片窗口：打开时选中当前图片，缩略图 ${picker_ready} 张已生成；单击只移动选择；按大小排序后选择不变；"
    say "    隐藏、显示缩略图导航；确定切到 B.png；取消不切换；压缩包里切到 ${picker_chosen}"
else
    say "  ✗ 选择图片窗口：${picker_problems}"
    failed=1
fi
# 双页：每一步的名字和合成后的尺寸。
report="${results}/pages/selftest.json"
pages_got=""
for i in 0 1 2 3 4 5 6 7; do
    pages_got="${pages_got}$(json_value "${report}" "steps.${i}.name") $(json_value "${report}" "steps.${i}.width")x$(json_value "${report}" "steps.${i}.height")|"
done
if [[ "${pages_got}" == "01.png 300x400|01.png | 02.png 600x400|03.png 400x300|04.png | 05.png 600x400|03.png 400x300|01.png | 02.png 600x400|01.png | 02.png 600x400|01.png 300x400|" ]]; then
    say "  ✓ 双页：01|02 一组 → 03 横图单独 → 04|05 → 往回 03 → 01|02 → 左←右 → 关掉回到单页（画面逐步核对）"
else
    say "  ✗ 双页：${pages_got}"
    failed=1
fi
# 文件操作。
report="${results}/fileops/selftest.json"
fileops_problems=""
fileops_note() { fileops_problems="${fileops_problems}${fileops_problems:+；}$1"; }
f1="$(cd "${fileops}/f1" && ls | LC_ALL=C sort | tr '\n' '|')"
f2="$(cd "${fileops}/f2" && ls | tr '\n' '|')"
[[ "${f1}" == "1 (2).png|1 (3).png|1.png|" && "${f2}" == "1.png|" ]] || fileops_note "复制、移动的结果不对：f1=${f1} f2=${f2}"
externals="$(json_value "${report}" steps.11.external.what) $(json_value "${report}" steps.12.external.what) $(json_value "${report}" steps.13.external.what)"
[[ "${externals}" == "editor wallpaper print" ]] || fileops_note "编辑器、桌面图片、打印没有交出：${externals}"
[[ "$(json_value "${report}" steps.14.name)" == "2.png" ]] || fileops_note "移动后没有显示下一张"
[[ "$(json_value "${report}" steps.15.total)" == "3" && "$(json_value "${report}" steps.16.total)" == "2" ]] \
    || fileops_note "文件夹变化没有自动刷新"
[[ "$(json_value "${report}" steps.8.preventingSleep)" == "true" ]] || fileops_note "打开设置后没有阻止显示器睡眠"
if [[ -z "${fileops_problems}" ]]; then
    say "  ✓ 文件操作：复制两次（第二次是“1 (2).png”）、复制到图片文件夹 2、移动后显示下一张；编辑器、桌面图片、打印已交出；"
    say "    加一张、删一张后自动刷新；看图时阻止显示器睡眠"
else
    say "  ✗ 文件操作：${fileops_problems}"
    failed=1
fi
# 动图：步骤序号见上面的 anim_steps（0 是打开图片）。
report="${results}/anim/selftest.json"
anim_problems=""
anim_note() { anim_problems="${anim_problems}${anim_problems:+；}$1"; }
paused="$(json_value "${report}" steps.2.animation.index)"
(( $(json_value "${report}" steps.1.framesSeen || echo 0) >= 3 )) 2>/dev/null || anim_note "GIF 没有在播放"
[[ "$(json_value "${report}" steps.2.animation.playing)" == "false" ]] || anim_note "空格没有暂停"
[[ "$(json_value "${report}" steps.3.animation.index)" == "${paused}" ]] || anim_note "暂停后帧还在换"
if [[ -n "${paused}" ]]; then
    [[ "$(json_value "${report}" steps.4.animation.index)" == "$(( (paused + 1) % 4 ))" ]] || anim_note "下一帧不对"
fi
[[ "$(json_value "${report}" steps.5.animation.index)" == "${paused}" ]] || anim_note "上一帧不对"
[[ "$(json_value "${report}" steps.6.name)" == "b_frames.apng" ]] || anim_note "没有切到 APNG"
[[ "$(json_value "${report}" steps.8.name)" == "c_transparent.gif" ]] || anim_note "没有切到透明 GIF"
[[ -z "$(json_value "${report}" steps.17.animationText)" ]] || anim_note "隐藏动画信息控制后还在显示"
if [[ -z "${anim_problems}" ]]; then
    say "  ✓ 动图：GIF 播放、空格暂停（停在第 $(( ${paused:-0} + 1 )) 帧）、逐帧前后、APNG 播放；透明处按文件里的背景色和设置的白色核对画面；"
    say "    动画信息控制显示“$(json_value "${report}" steps.1.animationText)”，设置后隐藏"
else
    say "  ✗ 动图：${anim_problems}"
    failed=1
fi
# 窗口：步骤序号见上面的 windows_steps（0 是打开图片）。
report="${results}/windows/selftest.json"
windows_problems=""
windows_expect() {  # windows_expect <步骤> <键路径> <应有的值> <说明>
    local value
    value="$(json_value "${report}" "steps.$1.$2")"
    if [[ "${value}" != "$3" ]]; then
        windows_problems="${windows_problems}${windows_problems:+；}$4（第 $1 步 ${value:-没有数据}）"
    fi
}
windows_expect 1 how new "从访达打开没有开新窗口"
windows_expect 1 windowCount 2 "从访达打开后不是两个窗口"
windows_expect 5 windows.1.background "#336699" "第二个窗口的背景色没跟着设置变"
windows_expect 6 windowCount 1 "Esc 没有只关掉第二个窗口"
windows_expect 11 how reused "勾上复用后没有在当前窗口里换图"
windows_expect 11 name 2.png "复用的窗口没有显示 2.png"
windows_expect 11 windowCount 1 "复用时又开了窗口"
windows_expect 18 windows.0.titleBarAlwaysShown true "显示标题栏没有生效"
windows_expect 23 quit true "最后一个窗口里按 Esc 没有退出"
title_bar="$(awk -v f="$(json_value "${report}" steps.19.windows.0.framePt.3)" \
    -v c="$(json_value "${report}" steps.19.windows.0.contentPt.1)" 'BEGIN { printf "%.0f", f - c }')"
if (( title_bar < 20 || title_bar > 60 )); then
    windows_problems="${windows_problems}${windows_problems:+；}显示标题栏后窗口没有多出标题栏的高度（${title_bar} 点）"
fi
offset="$(json_value "${report}" steps.1.centerOffsetPt.0),$(json_value "${report}" steps.1.centerOffsetPt.1)"
if [[ -z "${windows_problems}" ]]; then
    say "  ✓ 窗口：从访达打开开了新窗口（中心偏移 ${offset} 点），设置两个窗口都生效，Esc 只关第二个窗口；"
    say "    勾上复用后在原窗口换图；吸附边缘关着时不动、打开后贴边（拖动也贴边）；显示标题栏后多出 ${title_bar} 点；"
    say "    最后一个窗口里 Esc 退出"
else
    say "  ✗ 窗口：${windows_problems}"
    failed=1
fi
# 安全上限：步骤 1 是翻到那个 SVGZ（被拦下），步骤 2 是"仍然打开"。
report="${results}/limits/selftest.json"
if [[ "$(json_value "${report}" steps.1.blocked)" == "true" \
    && "$(json_value "${report}" steps.2.name)" == "b_65mb.svgz" && "$(json_value "${report}" steps.2.width)" == "64" ]]; then
    say "  ✓ 安全上限：解压后 65 MB 的 SVGZ 先被拦下、询问，选“仍然打开”后显示出来（64x48）"
else
    say "  ✗ 安全上限：拦下 $(json_value "${report}" steps.1.blocked)，之后显示 $(json_value "${report}" steps.2.name) $(json_value "${report}" steps.2.width)"
    failed=1
fi
# 窗口看不见、全屏时关窗口：步骤序号见上面的 visible_steps（0 是打开图片）。
report="${results}/visible/selftest.json"
visible_problems=""
visible_note() { visible_problems="${visible_problems}${visible_problems:+；}$1"; }
[[ "$(json_value "${report}" steps.4.preventingSleep)" == "true" ]] || visible_note "打开设置后没有阻止显示器睡眠"
[[ "$(json_value "${report}" steps.5.miniaturized)" == "true" ]] || visible_note "窗口没有最小化"
[[ "$(json_value "${report}" steps.5.preventingSleep)" == "false" ]] || visible_note "最小化后还在阻止显示器睡眠"
[[ "$(json_value "${report}" steps.6.miniaturized)" == "false" ]] || visible_note "窗口没有取回"
[[ "$(json_value "${report}" steps.6.preventingSleep)" == "true" ]] || visible_note "取回窗口后没有再阻止显示器睡眠"
presentation_full="$(json_value "${report}" steps.8.presentation)"
presentation_closed="$(json_value "${report}" steps.9.presentation)"
[[ -n "${presentation_full}" && "${presentation_full}" != "0" ]] || visible_note "第二个窗口没有进入全屏"
[[ "$(json_value "${report}" steps.9.windowCount)" == "1" ]] || visible_note "Esc 没有关掉全屏的窗口"
[[ "${presentation_closed}" == "0" ]] \
    || visible_note "全屏时关掉窗口后菜单栏、程序坞还是自动隐藏（${presentation_closed}）"
if [[ -z "${visible_problems}" ]]; then
    say "  ✓ 窗口看不见时：最小化后不阻止显示器睡眠，取回后再阻止；全屏时关掉窗口，菜单栏、程序坞恢复平常的样子"
else
    say "  ✗ 窗口看不见时、全屏时关窗口：${visible_problems}"
    failed=1
fi
preview_ms="$(json_value "${results}/preview/selftest.json" steps.0.msToPreview)"
if [[ -n "${preview_ms}" ]]; then
    say "  ✓ 快速预览：$(printf '%.1f' "${preview_ms}") ms 出预览，$(json_value "${results}/preview/selftest.json" steps.0.msToImage | cut -d. -f1) ms 换为清晰图"
else
    say "  ✗ 快速预览没有出现"
    failed=1
fi
if (( real_ready == 1 )); then
    report="${results}/real/selftest.json"
    say "  真实图片（100% 时核对 $(json_value "${report}" samplesChecked) 个点）："
    shooting=""
    for i in 0 1 2 3 4 5 6 7 8 9 10 11 12; do
        decode="$(json_value "${report}" "steps.${i}.decodeMs")"
        [[ -n "${decode}" ]] || continue
        proxy=""
        if [[ "$(json_value "${report}" "steps.${i}.hasProxy")" == "true" ]]; then proxy="，代理纹理"; fi
        name="$(json_value "${report}" "steps.${i}.name")"
        say "    ${name}  $(json_value "${report}" "steps.${i}.width")x$(json_value "${report}" "steps.${i}.height")  解码 $(printf '%.0f' "${decode}") ms${proxy}"
        # 拍摄信息：图片信息第 5 行起。
        if [[ "${name}" == "photo_12mp_iguana.jpg" ]]; then
            for k in 4 5 6 7 8; do
                line="$(json_value "${report}" "steps.${i}.overlays.infoText.${k}")"
                [[ -n "${line}" ]] && shooting="${shooting}${shooting:+ | }${line}"
            done
        fi
    done
    if [[ "${shooting}" == *" | "*" | "*" | "* ]]; then
        say "  ✓ 拍摄信息：${shooting}"
    else
        say "  ✗ 拍摄信息不全：${shooting:-没有}"
        failed=1
    fi
    report="${results}/perf/selftest.json"
    say "  性能（本机实测）："
    t1_values="$(json_value "${report}" steps.0.previewMsSinceProcessStart)"
    for i in 2 3 4 5; do
        t1_values="${t1_values} $(json_value "${results}/t1-${i}/selftest.json" steps.0.previewMsSinceProcessStart)"
    done
    t1_median="$(printf '%s\n' ${t1_values} | sort -n | awk 'NF { v[++n] = $1 } END { if (n > 0) print v[int((n + 1) / 2)] }')"
    perf_value_row "冷启动打开 1200 万像素 JPEG（出预览，5 次中位数）" "${t1_median}" 300 ms
    say "      5 次分别是（ms）：$(printf '%s\n' ${t1_values} | awk 'NF { printf "%s%.0f", (n++ ? "、" : ""), $1 }')"
    perf_note "冷启动打开 1200 万像素 JPEG（清晰图，第 1 次）" "${report}" steps.0.msSinceProcessStart ms
    # 冷启动耗时分解：进程启动到 main，以及 main 之后各阶段的时间点（毫秒）。
    timeline=""
    for i in 0 1 2 3 4 5 6 7 8 9 10 11; do
        mark="$(json_value "${report}" "launchTimeline.${i}.name")"
        [[ -n "${mark}" ]] || break
        timeline="${timeline}${timeline:+ → }${mark} $(printf '%.0f' "$(json_value "${report}" "launchTimeline.${i}.ms")")"
    done
    say "      冷启动分解（第 1 次）：进程启动到 main $(printf '%.0f' "$(json_value "${report}" preMainMs)") ms；main 之后（ms）：${timeline}"
    perf_row "运行中打开新图片（出预览）" "${report}" steps.9.msToPreview 100 ms
    perf_note "运行中打开新图片（清晰图）" "${report}" steps.9.msToImage ms
    perf_row "切到已预加载的图片（下一帧）" "${report}" steps.4.msToImage 16.7 ms
    perf_row "切到未缓存的 1200 万像素 JPEG（出预览）" "${report}" steps.2.msToPreview 50 ms
    perf_row "切到未缓存的 1200 万像素 JPEG（清晰图）" "${report}" steps.2.msToImage 150 ms
    perf_row "静止后精确重绘（Lanczos）" "${report}" steps.6.exact.renderMs 150 ms
    perf_row "打开 1 万张的文件夹（扫描 + 第一张）" "${report}" steps.10.msToImage 1000 ms
    perf_row "静止 3 秒的 CPU 占用" "${report}" steps.8.idleCpuPercent 1 %
    perf_note "静止时的内存占用（同活动监视器）" "${report}" steps.8.footprintMb MB
    perf_note "其中图片缓存（上限 1024 MB）" "${report}" steps.8.cacheMb MB
    perf_note "内存峰值（含自检截图等临时内存）" "${report}" steps.8.peakRssMb MB
    if (( perf_misses > 0 )); then
        say "    （${perf_misses} 项性能指标未达参考值或缺少数据，详情见性能日志）"
    fi
    rm -rf "${perf}"
fi
menu_state() {  # menu_state <报告>：模拟按右键后菜单是否弹出
    if [[ "$(json_value "$1" rightClickMenu.opened)" == "true" ]]; then echo "已弹出"; else echo "没弹出"; fi
}
say "  右键菜单：空态时$(menu_state "${results}/empty/selftest.json")，看图时$(menu_state "${results}/view/selftest.json")"

say ""
ran_to_end=1
collect_crashes || failed=1
if (( failed == 0 )); then
    say "✓ 全部通过"
    finish 0
fi
say "✗ 有没通过的，详情见上面"
finish 1
