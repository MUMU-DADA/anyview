/*
 * 编译 jxrlib 时每个文件最前面先包含这个头文件（build.rs 用 -include）。不改上游的代码，只补上：
 * - segdec.c 调用的 _byteswap_ulong 在 strcodec.c 里定义，头文件里没有声明（MSVC 自带它）。新的 GCC、Clang
 *   把"没声明就调用"当作错误；
 * - 有的文件用了 wcslen 等标准库函数，却没有包含对应的头文件（MSVC 的头文件把它们带进来了）；
 * - Windows（mingw）：build.rs 去掉 WIN32 这个宏（保留 _WIN32），jxrlib 就和在 Linux、macOS 上一样走跨平台的写法
 *   （它的 WIN32 写法只给 MSVC 用：SAL 标注、fopen_s、"..\x86\x86.h" 等）。另外两处在这里补：
 *   strcodec.c 自己定义的 _byteswap_ulong 和 mingw 的 stdlib.h 里的声明（unsigned long）对不上，改个名字；
 *   ansi.h 按 __LP64__ 定指针宽度的整数类型，64 位 Windows 不是 LP64（long 是 32 位），strcodec.h 的检查会失败：
 *   先在 __LP64__ 下包含一次 ansi.h（它有 #pragma once，之后不再包含），只影响它自己的这几个宏。
 */
#ifndef ANYVIEW_JXR_COMPAT_H
#define ANYVIEW_JXR_COMPAT_H
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include <math.h>
#if defined(_WIN32)
#define _byteswap_ulong anyview_jxr_byteswap_ulong
#endif
unsigned int _byteswap_ulong(unsigned int bits);
#if defined(_WIN64) && !defined(__LP64__)
#define __LP64__ 1
#include "ansi.h"
#undef __LP64__
#endif
#endif
