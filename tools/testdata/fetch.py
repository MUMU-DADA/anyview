#!/usr/bin/env python3
"""下载真实测试图片并核对 SHA-256（docs/FORMATS.md"测试"）。

图片都来自维基共享资源（Wikimedia Commons），许可证为 CC0 或公有领域，清单见 real-images.json
（标题、来源页、作者、许可证、用途、校验值）。图片不进 Git，下载到 tests/fixtures/real/。
加 --raw 时改按 raw-samples.json 下载 raw.pixls.us 的 CC0 相机原片，放到 tests/fixtures/raw/。

用法：
    python3 -I tools/testdata/fetch.py [--raw] [目标目录]
已下载且校验值正确的文件会跳过；校验值不符时删除并报错。
"""
import hashlib
import json
import os
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = os.path.join(ROOT, "tools", "testdata", "real-images.json")
RAW_MANIFEST = os.path.join(ROOT, "tools", "testdata", "raw-samples.json")
USER_AGENT = "anyview-test-corpus/0.1 (local development of an image viewer)"


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main():
    args = sys.argv[1:]
    raw = "--raw" in args
    args = [a for a in args if a != "--raw"]
    manifest = RAW_MANIFEST if raw else MANIFEST
    default = os.path.join(ROOT, "tests", "fixtures", "raw" if raw else "real")
    target = os.path.abspath(args[0]) if args else default
    os.makedirs(target, exist_ok=True)
    with open(manifest, encoding="utf-8") as f:
        images = json.load(f)["images"]
    failed = 0
    for image in images:
        path = os.path.join(target, image["name"])
        if os.path.exists(path) and sha256(path) == image["sha256"]:
            print(f"已有  {image['name']}")
            continue
        print(f"下载  {image['name']}（{image['bytes'] / 1e6:.1f} MB，{image['license']}）")
        request = urllib.request.Request(image["url"], headers={"User-Agent": USER_AGENT})
        partial = path + ".part"
        with urllib.request.urlopen(request, timeout=120) as response, open(partial, "wb") as out:
            while True:
                block = response.read(1 << 20)
                if not block:
                    break
                out.write(block)
        actual = sha256(partial)
        if actual != image["sha256"]:
            os.remove(partial)
            print(f"  ✗ 校验值不符：{actual}（应为 {image['sha256']}），已删除")
            failed += 1
            continue
        os.replace(partial, path)
    print("全部就绪" if failed == 0 else f"{failed} 个文件没有通过校验")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
