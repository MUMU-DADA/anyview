#!/bin/bash
# 编出 Windows 上看 HEIC 用的 heif.dll：libheif 加 libde265（HEVC 解码器），都是 LGPL-3.0，从 third_party 里的
# 源代码交叉编译。libde265 编成静态库、连进 libheif 的 DLL，所以只有一个文件；C++ 运行库、线程库也静态连进去，
# 只依赖 Windows 自带的系统库。输出 out/windows/heif.dll，程序运行时在自己旁边找它，有就用（没有就和原来一样）。
# 用法（需要 CMake、Ninja 与 mingw-w64）：bash tools/windows/build-heif.sh
# 只编解码要用的：别的编解码器、示例、测试、插件加载都关掉；编译时不联网。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
WORK="$REPO/target/heif"
PREFIX="$WORK/prefix"
OUT="$REPO/out/windows"
JOBS="${JOBS:-2}"
mkdir -p "$WORK" "$OUT"

cat > "$WORK/mingw.cmake" <<'TOOLCHAIN'
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
TOOLCHAIN

echo "== libde265（静态库）"
cmake -S "$REPO/third_party/libde265" -B "$WORK/de265" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$WORK/mingw.cmake" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_FIND_ROOT_PATH="$PREFIX" \
    -DBUILD_SHARED_LIBS=OFF -DENABLE_SDL=OFF -DENABLE_DECODER=OFF -DENABLE_ENCODER=OFF
cmake --build "$WORK/de265" -j "$JOBS"
cmake --install "$WORK/de265"

echo "== libheif（DLL）"
cmake -S "$REPO/third_party/libheif" -B "$WORK/heif" -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE="$WORK/mingw.cmake" -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_FIND_ROOT_PATH="$PREFIX" -DCMAKE_PREFIX_PATH="$PREFIX" \
    -DCMAKE_CXX_FLAGS="-DLIBDE265_STATIC_BUILD" \
    -DCMAKE_SHARED_LINKER_FLAGS="-static -static-libgcc -static-libstdc++" \
    -DBUILD_SHARED_LIBS=ON -DENABLE_PLUGIN_LOADING=OFF \
    -DWITH_LIBDE265=ON -DWITH_X265=OFF -DWITH_KVAZAAR=OFF -DWITH_UVG266=OFF -DWITH_VVDEC=OFF -DWITH_VVENC=OFF \
    -DWITH_X264=OFF -DWITH_OpenH264_DECODER=OFF -DWITH_DAV1D=OFF -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF \
    -DWITH_SvtEnc=OFF -DWITH_RAV1E=OFF -DWITH_JPEG_DECODER=OFF -DWITH_JPEG_ENCODER=OFF \
    -DWITH_OpenJPEG_ENCODER=OFF -DWITH_OpenJPEG_DECODER=OFF -DWITH_FFMPEG_DECODER=OFF -DWITH_OPENJPH_ENCODER=OFF \
    -DWITH_UNCOMPRESSED_CODEC=OFF -DWITH_LIBSHARPYUV=OFF -DWITH_EXAMPLES=OFF -DWITH_GDK_PIXBUF=OFF \
    -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF
cmake --build "$WORK/heif" -j "$JOBS"

DLL="$(find "$WORK/heif" -name 'libheif*.dll' | head -1)"
if [[ -z "$DLL" ]]; then
    echo "没有编出 libheif 的 DLL" >&2
    exit 1
fi
x86_64-w64-mingw32-strip --strip-unneeded -o "$OUT/heif.dll" "$DLL"
echo "== 输出"
ls -la "$OUT/heif.dll"
sha256sum "$OUT/heif.dll"
echo "依赖的 DLL（应当只有 Windows 自带的）："
x86_64-w64-mingw32-objdump -p "$OUT/heif.dll" | grep 'DLL Name' | sort -u
echo "导出的函数里有没有要用的几个："
x86_64-w64-mingw32-objdump -p "$OUT/heif.dll" | grep -c -E 'heif_context_alloc|heif_decode_image|heif_image_get_plane_readonly|heif_init'
