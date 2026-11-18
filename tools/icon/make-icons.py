#!/usr/bin/env python3
"""从 assets/icon/anyview.svg 生成三端应用图标文件。用法：

    python3 -I tools/icon/make-icons.py

需要 rsvg-convert（librsvg）和 Pillow。每个尺寸都从矢量图单独渲染（小尺寸更清楚），生成的文件提交进仓库：

- macOS：apps/macos/Resources/AppIcon.icns（16–1024，按 macOS 的图标网格，四周留透明边）
- Windows：apps/windows/Anyview/Assets/Anyview.ico（16–256，裁掉 macOS 网格的边，图形占满）；
  tools/windows/installer/welcome.bmp（安装程序欢迎页、完成页左边的那一条，164x314）
- Linux：apps/linux/data/icons/hicolor/<尺寸>/apps/local.anyview.Anyview.png 和 scalable 的 SVG（同样裁边）
"""

import io
import re
import subprocess
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SVG = ROOT / "assets" / "icon" / "anyview.svg"
APP_ID = "local.anyview.Anyview"
# Windows、Linux 的图标不要 macOS 网格四周 100 的透明边：取底板（100–924）外再留一点给投影。
CROP_VIEWBOX = "72 72 880 880"


def svg_source(cropped: bool) -> str:
    text = SVG.read_text(encoding="utf-8")
    if not cropped:
        return text
    new, count = re.subn(r'viewBox="[^"]*"', f'viewBox="{CROP_VIEWBOX}"', text, count=1)
    if count != 1:
        sys.exit("SVG 里没有 viewBox")
    return new


def render(size: int, cropped: bool) -> Image.Image:
    png = subprocess.run(
        ["rsvg-convert", "--width", str(size), "--height", str(size), "--format", "png"],
        input=svg_source(cropped).encode("utf-8"),
        capture_output=True,
        check=True,
    ).stdout
    image = Image.open(io.BytesIO(png)).convert("RGBA")
    if image.size != (size, size):
        sys.exit(f"渲染出的尺寸不对：{image.size}，应为 {size}")
    return image


def write_icns() -> Path:
    out = ROOT / "apps" / "macos" / "Resources" / "AppIcon.icns"
    sizes = [1024, 512, 256, 128, 64, 32, 16]
    images = [render(s, cropped=False) for s in sizes]
    images[0].save(out, format="ICNS", append_images=images[1:])
    return out


def write_ico() -> Path:
    out = ROOT / "apps" / "windows" / "Anyview" / "Assets" / "Anyview.ico"
    out.parent.mkdir(parents=True, exist_ok=True)
    sizes = [256, 128, 96, 64, 48, 40, 32, 24, 20, 16]
    images = [render(s, cropped=True) for s in sizes]
    images[0].save(out, format="ICO", sizes=[(s, s) for s in sizes], append_images=images[1:])
    return out


def write_installer_bitmap() -> Path:
    """Windows 安装程序（NSIS）欢迎页、完成页左边的图：浅灰底上放图标。要 24 位的 BMP，没有透明度。"""
    out = ROOT / "tools" / "windows" / "installer" / "welcome.bmp"
    out.parent.mkdir(parents=True, exist_ok=True)
    width, height, icon = 164, 314, 104
    image = Image.new("RGBA", (width, height), (243, 243, 243, 255))
    image.alpha_composite(render(icon, cropped=True), ((width - icon) // 2, 56))
    image.convert("RGB").save(out, format="BMP")
    return out


def write_linux() -> list[Path]:
    base = ROOT / "apps" / "linux" / "data" / "icons" / "hicolor"
    written = []
    for size in [16, 24, 32, 48, 64, 128, 256, 512]:
        out = base / f"{size}x{size}" / "apps" / f"{APP_ID}.png"
        out.parent.mkdir(parents=True, exist_ok=True)
        render(size, cropped=True).save(out, format="PNG", optimize=True)
        written.append(out)
    scalable = base / "scalable" / "apps" / f"{APP_ID}.svg"
    scalable.parent.mkdir(parents=True, exist_ok=True)
    scalable.write_text(svg_source(cropped=True), encoding="utf-8")
    written.append(scalable)
    return written


def main() -> None:
    if not SVG.is_file():
        sys.exit(f"没有 {SVG}")
    outputs = [write_icns(), write_ico(), write_installer_bitmap(), *write_linux()]
    for path in outputs:
        print(f"{path.relative_to(ROOT)}  {path.stat().st_size} 字节")


if __name__ == "__main__":
    main()
