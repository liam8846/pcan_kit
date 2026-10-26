"""建立只包含範例、文件與授權的可重現發行檔，不打包原廠 SDK 或驅動。"""

import argparse
from gzip import GzipFile
from pathlib import Path
import tarfile
import tomllib
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

from verify_release import validate_tag

EXAMPLES = ("monitor", "list_channels", "echo", "cyclic", "uds_request")
DOCUMENTS = (
    "README.md", "CHANGELOG.md", "CONTRIBUTING.md", "LICENSE-MIT", "LICENSE-APACHE",
    "docs/getting-started.md", "docs/concepts.md", "docs/link-runtime.md",
    "docs/testing.md", "docs/ci-cd.md", "docs/acceptance-template.md",
    ".github/assets/banner.svg",
)
TARGETS = ("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc")


def package(root: Path, output: Path, tag: str, target: str) -> Path:
    """先確認每個輸入存在，再以固定檔案順序、時間與權限封裝。"""
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    validate_tag(tag, version)
    if target not in TARGETS:
        raise ValueError("不支援的發行目標")
    windows = target.endswith("windows-msvc")
    executable = ".exe" if windows else ""
    binaries = root / "target" / target / "release" / "examples"
    files = [(binaries / f"{name}{executable}", True) for name in EXAMPLES]
    files += [(root / name, False) for name in DOCUMENTS]
    for path, _ in files:
        if not path.is_file():
            raise FileNotFoundError(path)
    output.mkdir(parents=True, exist_ok=True)
    prefix = f"pcan-kit-{tag}-{target}"
    destination = output / f"{prefix}.{'zip' if windows else 'tar.gz'}"
    # 只建立新成品；失敗時移除不完整檔案，既有成品不覆寫。
    raw = destination.open("xb")
    try:
        with raw:
            if windows:
                with ZipFile(raw, "w", compression=ZIP_DEFLATED) as archive:
                    for path, binary in files:
                        name = path.name if binary else path.relative_to(root).as_posix()
                        info = ZipInfo(f"{prefix}/{name}", (1980, 1, 1, 0, 0, 0))
                        info.create_system = 3
                        info.external_attr = (0o100755 if binary else 0o100644) << 16
                        info.compress_type = ZIP_DEFLATED
                        archive.writestr(info, path.read_bytes())
            else:
                with GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed:
                    with tarfile.open(fileobj=compressed, mode="w") as archive:
                        for path, binary in files:
                            name = path.name if binary else path.relative_to(root).as_posix()
                            info = tarfile.TarInfo(f"{prefix}/{name}")
                            info.size = path.stat().st_size
                            info.mode = 0o755 if binary else 0o644
                            with path.open("rb") as content:
                                archive.addfile(info, content)
    except BaseException:
        destination.unlink(missing_ok=True)
        raise
    return destination


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--output", type=Path, default=Path("dist"))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    args = parser.parse_args()
    print(package(args.root, args.output, args.tag, args.target))
