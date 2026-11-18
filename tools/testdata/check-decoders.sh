#!/bin/bash
# 用外部公开测试集核对自研/集成解码器（PSD、DDS、PCX、RAW）：
#   bash tools/testdata/check-decoders.sh
# 可选外部仓库（设置 TESTDATA_CACHE 指向存放目录）：
#   git clone --depth 1 https://github.com/psd-tools/psd-tools   "$TESTDATA_CACHE/github.com/psd-tools/psd-tools"
#   git clone --depth 1 https://github.com/python-pillow/Pillow  "$TESTDATA_CACHE/github.com/python-pillow/Pillow"
#   python3 -I tools/testdata/fetch.py --raw
# 输出位于 target/decode-check/<组>/：report.tsv、sheet-N.png、diff-N.png。
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
CACHE="${TESTDATA_CACHE:-$REPO/tests/fixtures/external}"
OUT="$REPO/target/decode-check"
cd "$REPO"
cargo build -q --release --locked -p anyview-fixtures --bin decode-check
run() {
    local name="$1" source="$2"
    if [[ ! -e "$source" ]]; then
        echo "跳过 $name：没有 $source（见脚本开头的说明）"
        return
    fi
    rm -rf "${OUT:?}/$name"
    echo "== $name"
    "$REPO/target/release/decode-check" --out "$OUT/$name" "$source"
}
run psd-tools "$CACHE/github.com/psd-tools/psd-tools/tests"
run pillow "$CACHE/github.com/python-pillow/Pillow/Tests/images"
run raw "$REPO/tests/fixtures/raw"
