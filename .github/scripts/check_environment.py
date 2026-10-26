"""確認實機驗收環境已有人工審核者；只讀取 API 回應，不修改保護規則。"""

import json
from pathlib import Path
import sys

from verify_release import require_hardware_review


if __name__ == "__main__":
    require_hardware_review(json.loads(Path(sys.argv[1]).read_text(encoding="utf-8")))
    print("實機驗收環境已設定人工審核者")
