#!/usr/bin/env python3
"""檢查 Rust 文件註解是否含繁體中文。"""

from __future__ import annotations

import re
import sys
from pathlib import Path


# 涵蓋 CJK 統一表意文字、CJK 標點與全形字元。
CJK = re.compile(r"[一-鿿　-〿＀-￯]")
RUSTDOC = re.compile(r"^\s*(?:///|//!)(?: ?)(.*)$")
STANDARD_HEADING = re.compile(r"^#+\s+(?:Errors|Safety|Panics|Examples|Returns)\s*$")
CODE_FENCE = re.compile(r"^\s*```")
PURE_LINK = re.compile(
    r"^\s*(?:"
    r"!?\[[^\]]+\](?:\([^)]*\))?"
    r"|!?\[[^\]]+\]:\s*\S+"
    r"|<(?:(?:https?|file)://|/)[^>]+>"
    r"|(?:https?://|(?:\.\.?/|/)?[\w.-]+/)[^\s]+"
    r"|(?:[A-Za-z_]\w*::)+[A-Za-z_]\w*"
    r")\s*[.,;:]?\s*$"
)
TABLE_SEPARATOR = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?\s*$")

# 只列高頻且幾乎總能改成繁體的字；命中僅警告，避免異體字或專有名詞誤擋 CI。
SIMPLIFIED = {
    "们": "們",
    "该": "該",
    "实": "實",
    "现": "現",
    "问": "問",
    "题": "題",
    "设": "設",
    "备": "備",
    "错": "錯",
    "误": "誤",
    "码": "碼",
    "发": "發",
    "获": "獲",
    "连": "連",
    "传": "傳",
    "输": "輸",
    "状": "狀",
    "态": "態",
    "数": "數",
    "据": "據",
    "类": "類",
    "读": "讀",
    "写": "寫",
    "开": "開",
    "关": "關",
    "闭": "閉",
    "检": "檢",
    "测": "測",
}


def is_exempt(content: str, in_code_block: bool) -> bool:
    """判斷一行文件註解是否屬於允許的非中文內容。"""
    stripped = content.strip()
    return (
        in_code_block
        or not stripped
        or bool(STANDARD_HEADING.fullmatch(stripped))
        or bool(PURE_LINK.fullmatch(stripped))
        or bool(TABLE_SEPARATOR.fullmatch(stripped))
    )


def main() -> int:
    """掃描所有 crate 的 Rust 原始檔並輸出檢查結果。"""
    root = Path(__file__).resolve().parents[2]
    files = sorted((root / "crates").glob("*/src/**/*.rs"))
    failures: list[tuple[Path, int, str]] = []
    warnings: list[tuple[Path, int, str, str]] = []
    rustdoc_lines = 0

    for path in files:
        in_code_block = False
        relative = path.relative_to(root)
        for line_number, raw_line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), start=1
        ):
            match = RUSTDOC.match(raw_line)
            if match is None:
                continue

            rustdoc_lines += 1
            content = match.group(1)
            stripped = content.strip()

            # 圍欄本身與圍欄內的 doctest 都是 Rust／文字範例，不要求中文。
            if CODE_FENCE.match(stripped):
                in_code_block = not in_code_block
                continue

            if is_exempt(content, in_code_block):
                continue

            if not CJK.search(content):
                failures.append((relative, line_number, content))

            found = "".join(dict.fromkeys(char for char in content if char in SIMPLIFIED))
            if found:
                replacements = "、".join(f"{char}→{SIMPLIFIED[char]}" for char in found)
                warnings.append((relative, line_number, content, replacements))

    for path, line_number, content in failures:
        print(f"{path}:{line_number}: {content}")

    for path, line_number, content, replacements in warnings:
        print(
            f"警告：{path}:{line_number}: 疑似含簡體字（{replacements}）：{content}",
            file=sys.stderr,
        )

    if failures:
        print(f"共發現 {len(failures)} 行 rustdoc 未含繁體中文")
        return 1

    print(f"已檢查 {len(files)} 個檔案、{rustdoc_lines} 行 rustdoc，全部含繁體中文")
    if warnings:
        print(f"另有 {len(warnings)} 行疑似含簡體字，請人工確認（不影響檢查結果）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
