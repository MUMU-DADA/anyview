#!/usr/bin/env python3
"""合成各平台分发包附带的第三方许可证清单，由 generate.sh 调用：

    generate.py <cargo-about 的输出目录> <写到的目录>

cargo-about 列出的 Rust 库之外，再加上 C、C++ 写的库和 Windows 随程序带的运行库。许可证原文从仓库里的
third_party、cargo 和 NuGet 的缓存、开发容器里 MinGW 的文件读；缺了哪一份就报错停下，不悄悄漏掉。
写出 windows.txt、macos.txt、linux.txt（英文：许可证原文本来就是英文），开头记下 Cargo.lock 的指纹，
核心的测试按它核对清单是不是最新的。
"""
import glob
import os
import re
import sys
import textwrap

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RULE = '-' * 80
BAR = '#' * 80


def clean(text):
    """去掉 BOM、统一换行、去掉行尾空白和首尾空行。"""
    text = text.replace(chr(0xFEFF), '').replace('\r\n', '\n').replace('\r', '\n')
    lines = [line.rstrip() for line in text.split('\n')]
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return '\n'.join(lines)


def read(path):
    path = os.path.expanduser(path)
    if not os.path.isfile(path):
        raise SystemExit(f'找不到许可证文件：{path}')
    with open(path, 'rb') as f:
        data = f.read()
    try:
        return clean(data.decode('utf-8'))
    except UnicodeDecodeError:
        return clean(data.decode('latin-1'))


def newest(pattern):
    """按版本号排在最后的那一个（缓存里可能有几个版本）。"""
    found = glob.glob(os.path.expanduser(pattern))
    if not found:
        raise SystemExit(f'找不到：{pattern}')

    def key(path):
        return [int(p) if p.isdigit() else p for p in re.split(r'(\d+)', path)]

    return sorted(found, key=key)[-1]


def repo(path):
    return os.path.join(REPO, path)


def fingerprint(path):
    """Cargo.lock 的 FNV-1a 64 位指纹（换行统一成 \\n；核心的测试用同样的算法）。"""
    with open(path, 'rb') as f:
        data = f.read().replace(b'\r\n', b'\n')
    h = 0xcbf29ce484222325
    for byte in data:
        h ^= byte
        h = (h * 0x100000001b3) & 0xffffffffffffffff
    return f'{h:016x}'


def define(path, name):
    m = re.search(rf'#define\s+{name}\s+(\d+)', read(path))
    if not m:
        raise SystemExit(f'{path} 里没有 {name}')
    return m.group(1)


def mit_body():
    text = read(repo('LICENSE'))
    start = text.index('Permission is hereby granted')
    return text[start:]


def csproj_version(package):
    text = read(repo('apps/windows/Anyview/Anyview.csproj'))
    m = re.search(rf'<PackageReference Include="{re.escape(package)}" Version="([^"]+)"', text)
    if not m:
        raise SystemExit(f'Anyview.csproj 里没有 {package}')
    return m.group(1)


def component(title, license_name, about, texts=()):
    """一个组件：标题、许可证、说明，后面是许可证原文（可以没有，只给链接）。"""
    # 按 80 列折行（程序里用等宽字体显示，许可证原文也是 80 列左右）。
    parts = [RULE, *textwrap.wrap(title, 80), *textwrap.wrap(f'License: {license_name}', 80)]
    for line in about:
        parts.extend(textwrap.wrap(line, 80))
    for text in texts:
        parts.append('')
        parts.append(text)
    return '\n'.join(parts)


def rust_crates(path):
    """cargo-about 的输出（模板 about.hbs）：每种许可证原文一段，前面是用它的库。"""
    text = clean(open(path, encoding='utf-8').read())
    blocks = []
    crates = set()
    for chunk in text.split('@@LICENSE@@ ')[1:]:
        head, _, rest = chunk.partition('\n')
        used, _, body = rest.partition('\n')
        if not used.startswith('@@USED-BY@@ '):
            raise SystemExit(f'cargo-about 的输出格式不对：{used[:60]}')
        names = []
        for item in used[len('@@USED-BY@@ '):].split(', '):
            item = item.strip()
            if item and item not in names:
                names.append(item)
        crates.update(names)
        used_by = textwrap.wrap('Used by: ' + ', '.join(names), 80, break_on_hyphens=False)
        blocks.append('\n'.join([RULE, head.strip(), *used_by, '', clean(body)]))
    if not blocks:
        raise SystemExit(f'{path} 里没有列出任何库')
    return blocks, len(crates)


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    about_dir, out_dir = sys.argv[1], sys.argv[2]
    os.makedirs(out_dir, exist_ok=True)
    lock = fingerprint(repo('Cargo.lock'))
    mit = mit_body()
    unrar_dir = newest('~/.cargo/registry/src/*/unrar_sys-0.5.8/vendor/unrar')
    unrar_version = f"{define(unrar_dir + '/version.hpp', 'RARVER_MAJOR')}.{define(unrar_dir + '/version.hpp', 'RARVER_MINOR')}"
    libraw_header = repo('third_party/libraw/libraw/libraw_version.h')
    libraw_version = '.'.join(define(libraw_header, f'LIBRAW_{p}_VERSION') for p in ('MAJOR', 'MINOR', 'PATCH'))

    built_in = [
        component(
            f'LibRaw {libraw_version}',
            'CDDL-1.0 (LibRaw may be used under LGPL-2.1 or CDDL-1.0; Anyview uses CDDL-1.0)',
            ['Decodes camera RAW files. https://www.libraw.org',
             'Source code: third_party/libraw in the Anyview source repository.'],
            [read(repo('third_party/libraw/COPYRIGHT')), read(repo('third_party/libraw/LICENSE.CDDL'))]),
        component(
            'jxrlib (JPEG XR reference implementation, 4creators fork)',
            'BSD-2-Clause',
            ['Decodes JPEG XR. https://github.com/4creators/jxrlib',
             'Source code: third_party/jxrlib in the Anyview source repository.'],
            [read(repo('third_party/jxrlib/LICENSE'))]),
        component(
            f'UnRAR {unrar_version}',
            'UnRAR license (freeware)',
            ['Reads RAR archives; included through the unrar and unrar_sys Rust libraries. '
             'It is used only to extract archives. https://www.rarlab.com'],
            [read(unrar_dir + '/license.txt')]),
        component(
            'Rust standard library',
            'MIT (also available under Apache-2.0)',
            ['https://www.rust-lang.org'],
            ['Copyright (c) The Rust Project Contributors\n\n' + mit]),
    ]

    # Windows：heif.dll、MinGW 和 GCC 的运行库、随 Anyview.exe 带的微软组件、生成的 C# 代码、安装程序。
    winappsdk = csproj_version('Microsoft.WindowsAppSDK.WinUI')
    win2d = csproj_version('Microsoft.Graphics.Win2D')
    webview = newest('~/.nuget/packages/microsoft.web.webview2/*/LICENSE.txt')
    dotnet = newest('~/.nuget/packages/microsoft.netcore.app.runtime.nativeaot.win-x64/10.*/LICENSE.TXT')
    heif_version = re.search(r'project\(libheif[^)]*VERSION ([0-9.]+)', read(repo('third_party/libheif/CMakeLists.txt'))).group(1)
    de265 = read(repo('third_party/libde265/CMakeLists.txt'))
    de265_version = re.search(r'project \(libde265[^)]*?VERSION ([0-9.]+)', de265, re.S).group(1)
    heif_copying = read(repo('third_party/libheif/COPYING'))
    lgpl_start = heif_copying.index('GNU LESSER GENERAL PUBLIC LICENSE')
    windows = [
        component(
            f'libheif {heif_version} and libde265 {de265_version} (heif.dll)',
            'LGPL-3.0-or-later',
            ['Decode HEIC images when Windows cannot. https://github.com/strukturag/libheif, '
             'https://github.com/strukturag/libde265',
             'heif.dll is a separate library that Anyview loads only when it is present; you may replace it '
             'with your own build. Source code: third_party/libheif and third_party/libde265 in the Anyview '
             'source repository; build script: tools/windows/build-heif.sh.'],
            [heif_copying[lgpl_start:].strip()]),
        component(
            'MinGW-w64 runtime',
            'see below',
            ['Parts of the MinGW-w64 runtime are linked into anyview_ffi.dll and heif.dll.'],
            [read('/usr/share/licenses/mingw-w64-crt/COPYING.MinGW-w64-runtime.txt')]),
        component(
            'GCC runtime libraries (libgcc, libstdc++)',
            'GPL-3.0-or-later with the GCC Runtime Library Exception',
            ['Linked into anyview_ffi.dll and heif.dll. The exception allows programs under any license to use them.']),
        component(
            f'Windows App SDK {winappsdk} (WinUI 3; the Microsoft.*.dll and other system files next to Anyview.exe)',
            'Microsoft Software License Terms',
            ['https://github.com/microsoft/WindowsAppSDK'],
            [read(newest(f'~/.nuget/packages/microsoft.windowsappsdk.winui/{winappsdk}/license.txt')),
             read(newest(f'~/.nuget/packages/microsoft.windowsappsdk.winui/{winappsdk}/NOTICE.txt'))]),
        component(
            f'Microsoft Edge WebView2 SDK {os.path.basename(os.path.dirname(webview))} '
            '(Microsoft.Web.WebView2.Core.dll, WebView2Loader.dll; part of the Windows App SDK)',
            'BSD-3-Clause (Microsoft)',
            ['https://aka.ms/webview2'],
            [read(webview), read(os.path.join(os.path.dirname(webview), 'NOTICE.txt'))]),
        component(
            f'Win2D {win2d} (Microsoft.Graphics.Canvas.dll)',
            'MIT',
            ['https://github.com/microsoft/Win2D. The source code is under the MIT License; the NuGet package '
             'refers to the Win2D license terms at http://www.microsoft.com/web/webpi/eula/eula_win2d_10012014.htm, '
             'a page that is no longer online.'],
            ['Copyright (c) Microsoft Corporation\n\n' + mit]),
        component(
            # 版本只写到 10.0：Windows 上编译用的 SDK（test.ps1 固定的那个）带的补丁版本和这里缓存的可能差一点，许可证文本相同。
            f'.NET {".".join(os.path.basename(os.path.dirname(dotnet)).split(".")[:2])} runtime (compiled into Anyview.exe with Native AOT)',
            'MIT',
            ['https://dot.net'],
            [read(dotnet), read(os.path.join(os.path.dirname(dotnet), 'THIRD-PARTY-NOTICES.TXT'))]),
        component(
            'Windows SDK projections for C# (Microsoft.Windows.SDK.NET, WinRT.Runtime; compiled into Anyview.exe)',
            'Microsoft Windows SDK license terms',
            ['https://aka.ms/WinSDKLicenseURL']),
        component(
            'uniffi-bindgen-cs 0.11.1 (C# code in Anyview.exe generated from its templates)',
            'MPL-2.0',
            ['https://github.com/NordSecurity/uniffi-bindgen-cs',
             'Source code: third_party/uniffi-bindgen-cs in the Anyview source repository.'],
            [read(repo('third_party/uniffi-bindgen-cs/LICENSE'))]),
        component(
            'NSIS 3.11 (only in the installer, Anyview-<version>-setup.exe)',
            'zlib/libpng license; bzip2 and LZMA parts under their own terms, see below',
            ['https://nsis.sourceforge.io'],
            [read(os.environ.get('NSIS_COPYING', os.path.join(os.environ.get('NSIS_SOURCE', ''), 'COPYING')))]),
    ]

    # macOS：UniFFI 生成的 Swift 代码（它的模板是 MPL-2.0）。
    macos = [
        component(
            'UniFFI 0.32 (Swift code in Anyview generated from its templates)',
            'MPL-2.0',
            ['https://github.com/mozilla/uniffi-rs'],
            [read(repo('third_party/uniffi-bindgen-cs/LICENSE'))]),
    ]

    # Linux：界面用的是系统（或 Flatpak 运行环境）里的库，不在 Anyview 的文件里。
    linux = [
        component(
            'GTK 4, GLib, Pango, Cairo, Graphene and libheif (from your system or the Flatpak runtime)',
            'mostly LGPL-2.1-or-later; libheif LGPL-3.0-or-later',
            ['Anyview for Linux uses these libraries from the system. They are not part of Anyview\'s files '
             'and keep their own licenses. libheif is used only when it is installed.']),
    ]

    platforms = {
        'windows': ('Windows', windows),
        'macos': ('macOS', macos),
        'linux': ('Linux', linux),
    }
    for key, (name, extra) in platforms.items():
        crates, count = rust_crates(os.path.join(about_dir, f'{key}.txt'))
        sections = [
            ('Libraries built into Anyview', built_in),
            (f'{name} components', extra),
            (f'Rust libraries ({count})', crates),
        ]
        header = [
            f'Anyview for {name}: third-party software notices',
            '',
            'Anyview itself is released under the MIT License (see LICENSE). It includes',
            'the software listed below, which comes from other projects and is used under',
            'the license shown with each. Where a license asks for the source code to be',
            'available, it is in the Anyview source repository at the path given.',
            '',
            'Generated by tools/licenses/generate.sh.',
            f'Cargo.lock fingerprint: {lock}',
            '',
            'Contents',
        ]
        for i, (title, _) in enumerate(sections, 1):
            header.append(f'  {i}. {title}')
        body = []
        for i, (title, items) in enumerate(sections, 1):
            body.extend(['', BAR, f'{i}. {title}', BAR, ''])
            body.append('\n\n'.join(items))
        text = '\n'.join(header + body) + '\n'
        path = os.path.join(out_dir, f'{key}.txt')
        with open(path, 'w', encoding='utf-8', newline='\n') as f:
            f.write(text)
        print(f'{path}: {len(text.encode("utf-8")) // 1024} KB, {count} Rust libraries')


if __name__ == '__main__':
    main()
