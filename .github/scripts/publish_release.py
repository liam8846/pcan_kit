"""完整上傳後才公開 Release；只續傳相同提交的草稿，不覆寫正式版本。"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib

from verify_release import validate_tag


def publish(directory: Path, tag: str, sha: str, version: str) -> None:
    """先檢查本機成品，再透過 GitHub CLI 建立／續傳草稿並發布。"""
    validate_tag(tag, version)
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("發行提交必須為完整 SHA")
    archives = [
        directory / f"pcan-kit-{tag}-x86_64-unknown-linux-gnu.tar.gz",
        directory / f"pcan-kit-{tag}-x86_64-pc-windows-msvc.zip",
    ]
    notes = directory / "release-notes.md"
    for path in [*archives, notes]:
        if not path.is_file():
            raise FileNotFoundError(path)
    if not notes.read_text(encoding="utf-8").strip():
        raise ValueError("發行說明不得為空")
    checksums = directory / "SHA256SUMS"
    checksums.write_text("".join(
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in archives
    ), encoding="utf-8")
    existing = subprocess.run(
        ["gh", "release", "view", tag, "--json", "isDraft,targetCommitish"],
        capture_output=True, text=True, check=False,
    )
    if existing.returncode == 0:
        release = json.loads(existing.stdout)
        if not release.get("isDraft") or release.get("targetCommitish") != sha:
            raise ValueError("只可續傳相同提交的草稿；既有正式 Release 不可覆寫")
    else:
        # 若查詢失敗是網路／權限問題而非不存在，create 仍須成功才會續傳。
        # 不帶 --clobber 的建立動作不會改動已存在的版本。
        subprocess.run([
            "gh", "release", "create", tag, "--verify-tag", "--draft", "--target", sha,
            "--title", tag, "--notes-file", str(notes),
        ], check=True)
    subprocess.run([
        "gh", "release", "upload", tag, *(str(path) for path in archives), str(checksums), "--clobber",
    ], check=True)
    subprocess.run([
        "gh", "release", "edit", tag, "--draft=false", "--latest", "--notes-file", str(notes),
    ], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--sha", required=True)
    args = parser.parse_args()
    version = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    publish(args.directory, args.tag, args.sha, version)
