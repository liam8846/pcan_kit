# 2026-10-26 安全審計紀錄

## 結論與受驗範圍

本紀錄保留首版準備前完成的安全檢查與修復，受驗版本以當次完整提交 SHA 為準。
首版 workspace 版本整理為 `0.1.0`；CI/CD 與版本整理後須重新驗證，未建立發版標籤。

本次針對動態函式庫搜尋、FFI 緩衝與生命週期、SocketCAN 收包邊界、
核心幀反序列化、Link 佇列與交易收集上限，以及驗收／發版工作流程進行原始碼檢查，
並執行相依套件安全公告稽核。這是指定範圍的原始碼與自動檢查，未包含實機滲透測試。

找到兩項可在特定使用情境造成安全影響的問題，以及一項 FFI 防禦性強化，
均已在工作分支修復。正式發版仍受 Windows 原生執行、PEAK 實機驗收、
維護者人工核准與主分支 CI 門檻限制，尚不能宣告可發版。

## 發現與修復

### 中風險：Windows 絕對路徑載入仍搜尋不受信任的相依 DLL

`pcan-basic-sys/src/api.rs` 原先以 `LOAD_WITH_ALTERED_SEARCH_PATH` 載入
絕對路徑的 PCAN-Basic。該旗標仍保留目前目錄與 `PATH` 的相依 DLL 搜尋。
若 PCAN-Basic 的相依 DLL 在較前順位的目錄無法解析，且攻擊者可控制後續搜尋位置，
可能載入攻擊者的 DLL，取得宿主行程權限；無須能改寫原始 PCAN-Basic 檔案。

改為 `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`，
將明確指定的 DLL 所在目錄加到安全搜尋目錄，排除目前目錄與一般 `PATH` 回退。
未指定絕對路徑時維持原有安全搜尋旗標。呼叫者仍須確保明確指定的函式庫、
應用程式目錄與 `AddDllDirectory` 註冊目錄可信任；此修復不驗證檔案簽章。

依據：[Microsoft DLL 搜尋順序](https://learn.microsoft.com/en-us/windows/win32/dlls/dynamic-link-library-search-order)
與 [LoadLibraryExW 旗標](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-loadlibraryexw)。
已通過 Windows 目標交叉編譯，原生搜尋行為仍待驗證。

### 中風險：幀反序列化在檢查長度前配置無上限酬載

啟用 `serde` 時，`pcan-core/src/frame.rs` 原先先反序列化為 `Vec<u8>`，
之後才檢查幀長度。若應用程式將不受信任的序列化資料交給此介面，
攻擊者可用過長陣列造成過量配置及處理，甚至耗盡記憶體；此路徑與原生 CAN 收包不同。

改用固定 64 位元組的堆疊緩衝，不依外部 `size_hint` 配置，
收到第 65 個元素立即回報錯誤。保留既有序列化格式與長度、RTR、FD 旗標檢查。
此上限涵蓋本型別的酬載處理；應用程式仍須限制其整體輸入大小與其他解析資源。

回歸測試涵蓋空酬載、古典 CAN 8 位元組、CAN FD 64 位元組、RTR 往返與
65 位元組拒絕；另以宣告 `usize::MAX` 長度的序列確認僅讀取 65 個元素即停止。

### 防禦性強化：錯誤文字未終止時可能越界讀取

`CAN_GetErrorText` 成功但寫滿 256 位元組且未提供 NUL 時，
原先的 `CStr::from_ptr` 會繼續讀取緩衝之外。這需要原廠介面違反字串契約，
未證實為正常驅動可觸發的漏洞；已載入的惡意原生函式庫本來就具有行程權限。

改為 `CStr::from_bytes_until_nul`，僅在已知緩衝內搜尋，未終止時回傳診斷文字。
Linux C 測試替身實際寫入 256 個非 NUL 字元，確認正常及未終止路徑均可處理。
此測試不代表原廠 DLL、驅動或 PEAK 硬體已驗收。

## 已完成驗證

環境：WSL2 Linux `6.18.54.1-microsoft-standard-WSL2`、Rust `1.98.1`。
工具鏈安裝於獨立的 `/tmp/pcan-audit-rustup`；下列 Cargo 指令均設定
`RUSTUP_HOME=/tmp/pcan-audit-rustup`。預設工具鏈安裝曾因並行初始化衝突失敗，
本表僅記錄改用獨立工具鏈後的成功結果。

| 檢查 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | 通過 |
| `cargo clippy --workspace --all-features --all-targets --locked -- -D warnings` | 通過 |
| `cargo test --workspace --all-features --locked` | 指令通過；119 項有效測試通過，另 24 項 vcan 測試未武裝而提前返回 |
| `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked` | 通過 |
| `cargo deny check` | advisories、bans、licenses、sources 均通過；有未使用授權允許條目的非阻擋警告 |
| `python3 -m unittest discover -s .github/scripts -p 'test_*.py'` | 5 項通過 |
| `python3 .github/scripts/check_cjk_docs.py` | 通過 |
| `cargo check --workspace --all-features --all-targets --locked --target x86_64-pc-windows-msvc` | 交叉編譯檢查通過，未執行 Windows 程式 |
| `git diff --check` | 通過 |

## 待完成驗證與發版門檻

- 遠端 PR CI：以本次完整 PR head SHA 的檢查結果為準。
- vcan：本機只有 `lo` 與 `eth0`，沒有獨立測試介面；未武裝的 24 項測試不算端到端通過。
- Windows 原生：驗證標準安裝及絕對路徑 PCAN-Basic 載入、正常錯誤文字與裝置列舉。
  以具有唯一相依 DLL 名稱的測試 DLL，分別確認相依僅置於目前目錄或一般 `PATH` 時
  載入失敗，而置於指定 DLL 目錄時可載入；確認失敗不觸發不安全搜尋回退。
- PEAK 實機：依[驗收紀錄範本](acceptance-template.md)補上實際 DLL／驅動版本、
  裝置型號、CAN／CAN FD 收發與關閉重開結果，記錄完整受驗 SHA。
- 驗收完成前保持 Draft PR；`hardware-acceptance` 由實際驗收的維護者人工核准。
- 正式版本須依 `CONTRIBUTING.md` 完成修復、版本與文件 PR 的 CI 及必要驗收，
  合併後在相同 SHA 通過 `master` push CI，才能建立與 workspace 版本相符的新正式標籤。
  若另以版本 PR 更新 Cargo manifest，須對新 SHA 補齊相容性驗收。
