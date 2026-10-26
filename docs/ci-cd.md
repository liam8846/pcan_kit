# CI/CD

## 工作流程

| 流程 | 觸發方式 | 職責 |
|---|---|---|
| CI | PR、`master` push、merge queue、手動 | 品質、跨平台、MSRV、功能組合、vcan、供應鏈 |
| 平台與硬體驗收 | PR 建立或更新 | 判斷影響範圍，必要時等待實機驗收者核准 |
| Docs | 成功的主分支 CI、手動 | 建置同一提交的 API 文件並部署 Pages；需明確啟用 |
| Release | `v*` tag push、手動選擇既有 tag | 驗證、建置、人工實機核准、封裝與發布 |
| Security audit | 每週、手動 | 對未變更的程式碼持續檢查 RustSec 公告 |

所有外部 Actions 固定到完整 commit SHA，由 Dependabot 定期提出更新。
共用 Rust 安裝步驟位於 `.github/actions/setup-rust`，預設讀取固定工具鏈；
MSRV 則從 `Cargo.toml` 讀取並明確設定 `RUSTUP_TOOLCHAIN`。

CI 的最終工作必須等每個前置工作成功；失敗、取消或跳過都不能記為「CI 全數通過」。
PR 不寫入共享建置快取，checkout 不保留推送憑證；只有部署與發布工作取得所需寫入權限。

## 新倉庫初次設定

1. 將原始碼放到 `master`，啟用 GitHub Actions，等待首輪 CI。
2. 設定 `master` Ruleset：要求 PR、「CI 全數通過」與「平台與硬體驗收通過」，
   禁止強推與刪除，不設定繞過者。單人維護不必強制另一位 PR 審閱者。
3. 建立 `hardware-acceptance` environment，設定 **Required reviewers** 為實際驗收者，
   並禁止管理員略過審核。單人維護可允許本人核准，但仍須有完整測試紀錄。
   環境的部署政策須允許 PR 驗收與正式版本標籤使用。
4. 設定 `v*` 標籤保護：禁止更新與刪除既有標籤，保留建立新版本的途徑。
5. Actions 的預設工作流程權限設為唯讀，禁止 Actions 核准 PR。
   只允許 `.github/allowed-actions.json` 中列出的固定提交；更新 Action SHA 時，
   同步更新此清單與倉庫允許設定。清單包含 Pages 上傳工具使用的固定巢狀 Action。
6. 若需要公開 API 網站，依下節啟用 Pages；不啟用也不影響 CI 與發版。

PR 驗收與 Release 都會先讀取環境設定。若環境不存在、缺少 required reviewers，
或 API 無法讀取，流程會停止，不會自動建立一個沒有審核者的環境後當成通過。
環境核准只是紀錄人工決定，實際測試內容與受驗 SHA 必須另存驗收紀錄。

## API 文件部署

預設不部署，避免新倉庫尚未開啟 Pages 就出現部署失敗。

1. Settings → Pages → Build and deployment，Source 選擇 **GitHub Actions**。
2. 新增 repository variable：`PUBLISH_DOCS=true`。
3. 推送下一個主分支提交，或重跑既有主分支 push CI。也可手動執行 Docs 的 `master`。

文件建置前會確認同一提交的最新主分支 push CI 已成功。自動觸發只接受本倉庫的
`master` push CI，不執行外部 PR 提供的程式碼。部署使用獨立的 `github-pages`
工作，Rust 建置端不持有 Pages 寫入權限。

部署後首頁會導向 `pcan_kit/index.html`。網站網址由 Pages 設定提供。

## 首版與後續版本

1. 更新 workspace 版本、內部相依、lockfile 與 `CHANGELOG.md` 的版本區段。
2. 完成 PR CI 與必要實機驗收，合併後等相同提交的主分支 push CI 成功。
3. 在已驗證提交建立新的正式標籤；首版為 `v0.1.0`。
4. Release 檢查標籤與版本相符、提交位於 `master`、最新一次同 SHA CI 成功，
   並確認實機驗收環境有人工審核者。
5. Linux 與 Windows 分別建置五個範例，打包 README、CHANGELOG、使用文件、橫幅與兩份授權。
6. 完成實機驗收的維護者核准該發行提交，流程才進入發布。
7. 先建立草稿、上傳兩個平台的檔案與 `SHA256SUMS`，最後轉為正式 Release。

發行檔：

```text
pcan-kit-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
pcan-kit-v0.1.0-x86_64-pc-windows-msvc.zip
SHA256SUMS
```

封裝固定檔案順序、時間、擁有者與權限；在同一封裝環境下，相同輸入產生相同封裝。
這不宣稱不同機器或編譯器一定產生位元組完全相同的二進位檔。

若建置或上傳失敗，可重跑流程，亦可手動選擇相同 tag 執行。只允許續傳相同 SHA
的草稿，已發布版本會拒絕覆寫。標籤不移動；若版本內容需要修改，使用新的版本號。
發行說明從受審查的 CHANGELOG 對應區段擷取，不需要另裝 git-cliff 或回寫主分支。

設計參考：[GitHub Actions 安全使用](https://docs.github.com/en/actions/reference/security/secure-use)、
[環境與審核者 API](https://docs.github.com/en/rest/deployments/environments)、
[GitHub CLI Release](https://cli.github.com/manual/gh_release_create)。
