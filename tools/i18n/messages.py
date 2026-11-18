#!/usr/bin/env python3
"""Anyview 界面文字的翻译文件检查。

总表 crates/anyview-core/locales/messages.toml 由核心的测试生成（键是英文原文，注释是中文原文和用在哪里）；
每种语言一个翻译文件 crates/anyview-core/locales/<代码>.toml，键相同、值是译文。

用法：
  python3 tools/i18n/messages.py check <代码>…   检查翻译文件：缺的、多的、空的、占位符对不上的
  python3 tools/i18n/messages.py todo <代码>      列出还没翻的条目（带中文原文和用在哪里）
  python3 tools/i18n/messages.py stats           各语言翻了多少
"""
import re
import sys
import tomllib
from pathlib import Path

LOCALES = Path(__file__).resolve().parents[2] / "crates/anyview-core/locales"
PLACEHOLDER = re.compile(r"\{[A-Za-z0-9_:]+\}")


def source():
    """英文原文 → (中文原文, 用在哪里)。"""
    text = (LOCALES / "messages.toml").read_text(encoding="utf-8")
    keys = tomllib.loads(text)
    notes = {}
    zh = place = ""
    for line in text.splitlines():
        if line.startswith("# 中文："):
            zh = line[len("# 中文："):]
        elif line.startswith("# 用在："):
            place = line[len("# 用在："):]
        elif line.startswith('"'):
            key = next(iter(tomllib.loads(line)))
            notes[key] = (zh, place)
    return {key: notes.get(key, ("", "")) for key in keys}


def catalog(code):
    path = LOCALES / f"{code}.toml"
    return tomllib.loads(path.read_text(encoding="utf-8"))


def check(code, messages):
    problems = []
    try:
        table = catalog(code)
    except (OSError, tomllib.TOMLDecodeError) as e:
        return [f"{code}：读不了：{e}"]
    missing = [k for k in messages if k not in table]
    if missing:
        problems.append(f"{code}：缺 {len(missing)} 条，例如：{missing[:3]}")
    for key, value in table.items():
        if key not in messages:
            problems.append(f"{code}：多出来的（总表里没有）：{key!r}")
        elif not isinstance(value, str) or not value.strip():
            problems.append(f"{code}：译文是空的：{key!r}")
        elif sorted(PLACEHOLDER.findall(value)) != sorted(PLACEHOLDER.findall(key)):
            problems.append(f"{code}：占位符对不上：{key!r} → {value!r}")
        elif key.count("\n") != value.count("\n"):
            problems.append(f"{code}：换行数不一样：{key!r} → {value!r}")
    return problems


def main(argv):
    if len(argv) < 2 or argv[1] not in ("check", "todo", "stats"):
        print(__doc__)
        return 2
    messages = source()
    if argv[1] == "stats":
        for path in sorted(LOCALES.glob("*.toml")):
            if path.stem == "messages":
                continue
            done = sum(1 for k in catalog(path.stem) if k in messages)
            print(f"{path.stem:8} {done}/{len(messages)}")
        return 0
    if argv[1] == "todo":
        table = catalog(argv[2])
        for key, (zh, place) in messages.items():
            if key not in table:
                print(f"# 中文：{zh}\n# 用在：{place}\n{key!r}\n")
        return 0
    problems = [p for code in argv[2:] for p in check(code, messages)]
    for p in problems:
        print(p)
    print("没有问题" if not problems else f"共 {len(problems)} 处问题")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
