"""確認正式版本標籤與同一個提交的主分支 CI 結果。"""

import argparse
import json
from pathlib import Path
import re
import tomllib


def validate_tag(tag: str, version: str) -> None:
    """只允許與 workspace 版本一致的正式版本標籤。"""
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag) or tag != f"v{version}":
        raise ValueError("標籤必須為 v主版.次版.修補版，且與 Cargo.toml 版本一致")


def has_successful_ci(pages: list[dict], sha: str) -> bool:
    """只接受同一提交最新一次主分支 CI；重跑中的舊成功紀錄不能代替。"""
    candidates = [
        run
        for page in pages
        for run in page["workflow_runs"]
        if run.get("head_sha") == sha
        and run.get("head_branch") == "master"
        and run.get("event") == "push"
        and run.get("path") == ".github/workflows/ci.yml"
    ]
    if not candidates:
        return False
    latest = max(candidates, key=lambda run: (run.get("id", 0), run.get("run_attempt", 1)))
    return latest.get("status") == "completed" and latest.get("conclusion") == "success"


def require_hardware_review(environment: dict) -> None:
    """新倉庫尚未設定人工審核時停止，避免環境自動建立而直接通過。"""
    if environment.get("name") != "hardware-acceptance" or not any(
        rule.get("type") == "required_reviewers" and rule.get("reviewers")
        for rule in environment.get("protection_rules", [])
    ):
        raise ValueError("須先設定 hardware-acceptance 的 Required reviewers，再由實際驗收者核准")


def release_notes(changelog: str, version: str) -> str:
    """從已審查的變更紀錄擷取指定版本，不依賴舊倉庫 Git 歷史。"""
    heading = re.search(rf"^## \[{re.escape(version)}\].*$", changelog, re.MULTILINE)
    if not heading:
        raise ValueError(f"CHANGELOG.md 缺少版本 {version}")
    remaining = changelog[heading.end():]
    following = re.search(r"^## ", remaining, re.MULTILINE)
    body = remaining[:following.start()] if following else remaining
    if not body.strip():
        raise ValueError("發行說明不得為空")
    return f"# v{version}\n\n{body.strip()}\n"


def main() -> None:
    """讀取版本與 API 回應，不符合發版條件即停止工作流程。"""
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--runs", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--notes-output", type=Path, required=True)
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    validate_tag(args.tag, version)
    require_hardware_review(json.loads(args.environment.read_text(encoding="utf-8")))
    if not has_successful_ci(json.loads(args.runs.read_text(encoding="utf-8")), args.sha):
        raise ValueError("找不到同一提交在 master 的成功 push CI；等待 CI 後再重跑發版")
    args.notes_output.write_text(release_notes(Path("CHANGELOG.md").read_text(encoding="utf-8"), version), encoding="utf-8")
    print(f"發版檢查通過：{args.tag} / {args.sha}")


if __name__ == "__main__":
    main()
