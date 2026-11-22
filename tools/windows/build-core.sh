#!/bin/bash
# 使用 MinGW-w64 将 Rust 核心交叉编译为 Windows DLL（x86_64-pc-windows-gnu）：
#   bash tools/windows/build-core.sh
# 需安装 Rust 1.99.1（rust-toolchain.toml）、x86_64-pc-windows-gnu 目标与 mingw-w64，
# 输出 out/windows/anyview_ffi.dll 与 C# 绑定 out/windows/bindings/。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
export CARGO_TARGET_DIR="$REPO/target/windows-gnu"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
export CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++
export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
# unrar 的源代码按 Windows 的大小写引用 PowrProf.h、Wbemidl.h；mingw 的是小写的。
export CXXFLAGS_x86_64_pc_windows_gnu="-I$REPO/tools/windows/mingw-shim"
CARGO="${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}"
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
# unrar_sys 的构建脚本按"编译它的机器"判断是不是 Windows（cfg!(windows)），在 Linux 上交叉编译时漏掉了
# isnt.cpp 和几个系统库：这里自己补上（同样的编译选项）。
UNRAR="$(ls -d "$CARGO_HOME_DIR"/registry/src/*/unrar_sys-0.5.8 2>/dev/null | head -n 1 || true)"
if [[ -z "$UNRAR" ]]; then
    "$CARGO" fetch --locked
    UNRAR="$(ls -d "$CARGO_HOME_DIR"/registry/src/*/unrar_sys-0.5.8 2>/dev/null | head -n 1 || true)"
fi
if [[ -z "$UNRAR" ]]; then
    CRATE_TAR="$(ls "$CARGO_HOME_DIR"/registry/cache/*/unrar_sys-0.5.8.crate 2>/dev/null | head -n 1 || true)"
    if [[ -n "$CRATE_TAR" ]]; then
        mkdir -p "$CARGO_TARGET_DIR/unrar-src"
        tar -xzf "$CRATE_TAR" -C "$CARGO_TARGET_DIR/unrar-src"
        UNRAR="$CARGO_TARGET_DIR/unrar-src/unrar_sys-0.5.8"
    fi
fi
if [[ -z "$UNRAR" || ! -f "$UNRAR/vendor/unrar/isnt.cpp" ]]; then
    echo "找不到 unrar_sys-0.5.8 源码目录" >&2
    exit 1
fi
SHIM_OBJ="$CARGO_TARGET_DIR/unrar-isnt.o"
mkdir -p "$CARGO_TARGET_DIR"
x86_64-w64-mingw32-g++ -O2 -std=c++14 -w -I"$REPO/tools/windows/mingw-shim" \
    -D_FILE_OFFSET_BITS=64 -D_LARGEFILE_SOURCE -DRAR_SMP -DRARDLL \
    -c "$UNRAR/vendor/unrar/isnt.cpp" -o "$SHIM_OBJ"
# unrar_sys 要 -lpthread，ld 会先找到 mingw 的导入库（运行时要 libwinpthread-1.dll）：放一份静态库冒充它，
# 排在搜索路径最前面（-L 对所有 -l 都起作用），线程库就静态链接进去了。
STATIC_LIBS="$CARGO_TARGET_DIR/static-libs"
mkdir -p "$STATIC_LIBS"
WINPTHREAD="$(x86_64-w64-mingw32-gcc -print-file-name=libwinpthread.a)"
if [[ ! -f "$WINPTHREAD" ]]; then
    for cand in /usr/x86_64-w64-mingw32/lib/libwinpthread.a /usr/lib/gcc/x86_64-w64-mingw32/*-posix/libwinpthread.a; do
        if [[ -f "$cand" ]]; then
            WINPTHREAD="$cand"
            break
        fi
    done
fi
if [[ ! -f "$WINPTHREAD" ]]; then
    echo "找不到 libwinpthread.a（请安装 mingw-w64 并使用 posix 线程版本）" >&2
    exit 1
fi
cp "$WINPTHREAD" "$STATIC_LIBS/libpthread.dll.a"
# 这些链接选项只给最后的 DLL（`cargo rustc` 的 `--` 之后）；补上的目标文件后面要跟它用到的库。
# C++ 运行库、libgcc 静态链接，DLL 只依赖 Windows 自带的系统库。
"$CARGO" rustc --locked --release --target x86_64-pc-windows-gnu -p anyview-ffi --crate-type cdylib -- \
    -C strip=symbols -C link-arg=-L"$STATIC_LIBS" \
    -C link-arg=-static-libgcc -C link-arg=-static-libstdc++ \
    -C link-arg="$SHIM_OBJ" \
    -C link-arg=-Wl,-Bstatic -C link-arg=-lstdc++ -C link-arg=-Wl,-Bdynamic \
    -C link-arg=-ladvapi32 -C link-arg=-lshell32 -C link-arg=-lpowrprof -C link-arg=-lole32 \
    -C link-arg=-loleaut32 -C link-arg=-lkernel32
mkdir -p "$REPO/out/windows"
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/anyview_ffi.dll" "$REPO/out/windows/anyview_ffi.dll"
echo "DLL 依赖的系统库："
x86_64-w64-mingw32-objdump -p "$REPO/out/windows/anyview_ffi.dll" | sed -n 's/^\tDLL Name: /  /p'
ls -l "$REPO/out/windows/anyview_ffi.dll"

# C# 绑定：用 third_party 里的 uniffi-bindgen-cs（在这台机器上编译）从刚编好的 DLL
# 读出接口，生成 out/windows/bindings/anyview_ffi.cs，绑定和 DLL 总是同一次编译的。
# 它用 cargo metadata 找各 crate 的 uniffi.toml，所以在仓库根目录运行。
"$CARGO" build --release --locked -p uniffi-bindgen-cs
rm -rf "$REPO/out/windows/bindings"
mkdir -p "$REPO/out/windows/bindings"
"$CARGO_TARGET_DIR/release/uniffi-bindgen-cs" --library "$REPO/out/windows/anyview_ffi.dll" \
    --out-dir "$REPO/out/windows/bindings" --no-format
ls -l "$REPO/out/windows/bindings"
