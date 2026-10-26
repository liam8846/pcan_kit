"""依 PR 變更範圍判斷是否需要 Windows／PEAK 實機驗收。"""

from __future__ import annotations

import json
import os
from pathlib import Path, PurePosixPath
import sys


def requires_acceptance(filename: str) -> bool:
    """共用執行邏輯、PCAN 後端與相依設定變更需人工驗收。"""
    path = PurePosixPath(filename)
    if filename in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml"}:
        return True
    if len(path.parts) < 3 or path.parts[0] != "crates":
        return False
    if path.name == "Cargo.toml":
        return True
    return path.parts[1] in {
        "pcan-core", "pcan-link", "pcan-kit", "pcan-basic", "pcan-basic-sys"
    } and path.parts[2] in {"src", "examples"}


def affected_files(pages: list[list[dict]]) -> list[str]:
    """檢查新舊檔名，避免移出受保護目錄的重新命名漏掉驗收。"""
    return sorted({
        filename
        for page in pages
        for entry in page
        for filename in (entry["filename"], entry.get("previous_filename", ""))
        if requires_acceptance(filename)
    })


def main() -> None:
    """輸出工作流程條件與包含受驗 SHA 的摘要。"""
    files = affected_files(json.loads(Path(sys.argv[1]).read_text()))
    required = str(bool(files)).lower()
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"required={required}\n")
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a") as summary:
        summary.write(f"## 驗收範圍\n\n受驗 PR head SHA：`{os.environ['HEAD_SHA']}`\n\n")
        if files:
            summary.write("需完成 Windows 原生執行與 PEAK 實機驗收，將結果記錄在 PR，"
                          "再由維護者核准 `hardware-acceptance` 環境。\n\n")
            summary.writelines(f"- `{filename}`\n" for filename in files)
        else:
            summary.write("本次變更不涉及共用執行邏輯、PCAN 後端或相依設定；"
                          "其餘驗證由 CI 負責。\n")


if __name__ == "__main__":
    main()
