# 參與開發

本專案以 Rust 2024 開發，公開文件、PR 與驗收紀錄使用繁體中文。
提交訊息使用 conventional commits，例如 `fix:`、`feat:`、`docs:`、`chore:`。

## 開發流程

1. 從最新 `master` 建立 `fix/*`、`feat/*`、`docs/*` 或 `chore/*` 工作分支。
   已有相符工作分支時可沿用；不要直接在 `master` 修改。
2. 執行相關檢查，提交並開啟 PR，說明問題、修改後行為與測試結果。
3. 影響 PCAN、共用執行邏輯、相依或工具鏈時，完成必要驗收前保持 Draft。
4. 記錄完整 PR head SHA 與實際驗收結果，由完成驗收的維護者人工核准
   `hardware-acceptance`。自動測試、交叉編譯與測試替身不能代替實機。
5. 確認分支包含最新 `master`，且「CI 全數通過」與「平台與硬體驗收通過」
   兩項必要檢查成功後，再依倉庫保護規則合併。

驗收後若有新提交或更新基底，必須更新受驗 SHA 並重新確認，不能沿用舊版核准。
不覆寫或刪除既有版本標籤，不略過主分支與驗收保護。

## 本機檢查

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
cargo clippy --workspace --no-default-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
cargo deny check
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

完整 vcan 與硬體驗證步驟見[測試與驗收](docs/testing.md)。未設定測試介面時的
提早返回不能記為端到端測試通過。Linux 的 PCAN 生命週期測試需要系統 C 編譯器。

## 必要驗收

| 變更 | 驗證範圍 |
|---|---|
| PCAN 後端、FFI、core／link／kit 執行邏輯及範例 | CI、Windows 原生執行、PEAK 實機 |
| Cargo manifest、lockfile、工具鏈 | CI、Windows／PEAK 相容性 |
| 僅 SocketCAN 原始碼 | CI、實際 vcan；涉及實體匯流排行為時另驗實機 |
| 文件、測試、CI 設定 | 相關自動檢查；實際行為改變時補驗 |

以完整 PR 檔案清單判斷範圍，重新命名同時檢查舊路徑。
實機證據使用[驗收紀錄範本](docs/acceptance-template.md)。核准步驟只記錄人工決定，
本身不執行硬體測試；維護者須確實完成驗收後才核准。

## 發版

首版為 `0.1.0`。版本由 workspace `Cargo.toml` 統一管理，內部相依與 `Cargo.lock`
須同步更新。發行內容預先寫入 `CHANGELOG.md`，不從其他倉庫的歷史產生紀錄。

1. 經 PR 完成版本、文件、CI 與必要驗收，合併後等待同一提交的 `master` push CI 成功。
2. 在該提交建立與 workspace 相符的新標籤，例如 `v0.1.0`，再推送標籤。
3. Release 流程檢查版本、主分支歸屬、最新一次 CI 與人工審核設定，建置兩個平台。
4. 實際驗收的維護者在 `hardware-acceptance` 核准該發行提交。
5. 所有檔案上傳成功後，才將草稿轉為正式 GitHub Release。

只接受 `v主版.次版.修補版`；不發布 `-rc` 等預覽標籤。CI 尚未完成時先等待後重跑，
不要移動標籤。發行失敗可續傳相同提交的草稿；既有正式 Release 不覆寫。
目前不自動發布至 crates.io。

新倉庫需先完成[CI/CD 設定](docs/ci-cd.md)。刪除並重建倉庫不會保留原本的
Rulesets、環境審核者或 Pages 設定；首版也需要如實記錄實機驗收狀態。
