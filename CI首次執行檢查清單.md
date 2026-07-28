# CI 首次執行檢查清單

推送提交：`031dc2e`（2026-07-28）
CI 頁面：https://github.com/liam8846/pcan_kit/actions
文件頁面（首次部署後才會有內容）：https://liam8846.github.io/pcan_kit/

本機已把所有能驗的都驗過（見下方「本機已通過」），**剩下三件事只有在真的 runner 上才驗得到**。

---

## 先看 `ci-ok` 這個工作

它是彙總關卡。綠 = 全部過，可以不用往下看了。紅了再依下表判斷。

---

## 預期最可能紅的兩處

### 1. 虛擬 CAN 整合測試（工作名稱標「（觀察中）」）

**這個工作掛了 `continue-on-error: true`，紅了也不會擋住 `ci-ok`。** 這是刻意的，因為它對 runner image 內容有硬依賴。

怎麼判斷：

| 停在哪 | 意義 | 處置 |
|---|---|---|
| `modprobe vcan` 或 `apt-get install linux-modules-extra` | runner 核心版本比 Ubuntu archive 新，找不到對應的模組套件 | **環境問題，不是 bug**。等 runner image 更新，或改用其他方式 |
| `ip link add ... type vcan` 報 `Operation not supported` | 模組其實沒載入成功（`modprobe` 可能回 0 但沒真的載入） | 同上 |
| 介面建立成功、但測試斷言失敗 | **這才是真 bug** | 要修 `crates/pcan-socketcan/` 的程式碼 |

**若連續 5 次都綠**，就可以把 `.github/workflows/ci.yml` 裡 `vcan` 工作的 `continue-on-error: true` 拿掉，並把名稱的「（觀察中）」去掉。

### 2. 文件部署 404

Pages 設定我已確認生效（`build_type: "workflow"`），所以理論上不會。若真的 404，回 Settings → Pages 確認 Source 仍是「GitHub Actions」。**這是設定問題，不是 rustdoc 的 bug**——`cargo doc -D warnings` 在本機已通過。

---

## 本機已通過（這些在 CI 上紅了必定是環境差異，不是程式碼問題）

| 關卡 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | 通過 |
| Clippy `--all-features --all-targets -D warnings` | 通過 |
| Clippy `--no-default-features -D warnings` | 通過 |
| `cargo test --workspace --all-features` | **93 passed** |
| `cargo doc -D warnings` | 通過 |
| `cargo +1.88 check`（MSRV） | 通過 |
| Windows target `check` 與 `clippy` | 通過 |
| `cargo deny --all-features check` | **advisories / bans / licenses / sources 全 ok** |
| `cargo hack --feature-powerset` | **58/58 通過** |
| `check_cjk_docs.py` | 40 檔、1044 行 rustdoc、零問題 |

**注意**：CI 的 Windows 工作是在真的 `windows-latest` runner 上跑 `cargo test`，本機只做過交叉 `check`。若 Windows 測試紅了，那是本機驗不到的真實差異，值得認真看。

---

## 尚未驗證、需要你接硬體才能做的事

**CI 全綠不代表函式庫在真實 CAN 匯流排上能用。** 虛擬 CAN 只涵蓋 SocketCAN 後端的核心 ABI，完全不碰 PCAN-Basic 的 FFI 層（那 49 個 `unsafe` 區塊）。

接上 PEAK 硬體後請依 `README.md` 的「接上硬體後的手動驗證步驟」進行，重點三項：

1. **Windows `PCAN_RECEIVE_EVENT` 的兩條路徑** — pointer-size 與 4-byte fallback。用新舊版 `PCANBasic.dll` 各長時間收包一次，並確認拔除裝置時不會卡在 join。
2. **Linux PEAK receive-event fd 的驅動版本差異** — 高負載連續收包，確認沒有「永久落後一幀」、空閒時沒有週期 wakeup；用舊版驅動確認 `ILLPARAMTYPE` 會印出明確的降級警告。
3. **CAN FD 時序查表** — 目前只涵蓋 80 MHz 時鐘的 12 種組合，其他組合會回 `InvalidBitrate`。可用 `with_raw_fd_bitrate()` 直接給字串繞過。

---

## 一個已知的小瑕疵（不影響功能）

`deny.toml` 的 `licenses.allow` 預先放了 `Unlicense`、`Zlib`、`BSD-2-Clause`、`BSD-3-Clause`，但目前沒有任何相依使用它們，所以每次 `cargo deny` 都會印 `license-not-encountered` 警告（**非阻擋**）。

留著是防禦性的（日後有相依用到不會突然擋住），但也稍微弱化了「任何新增的間接相依都應該被人看見一次」這個原則。想收緊就把沒用到的那幾個拿掉。
