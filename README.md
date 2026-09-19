# pcan_kit

[![CI](https://img.shields.io/github/actions/workflow/status/liam8846/pcan_kit/ci.yml?branch=master&label=CI&logo=githubactions&logoColor=white)](https://github.com/liam8846/pcan_kit/actions/workflows/ci.yml)
[![API 文件](https://img.shields.io/badge/API%20%E6%96%87%E4%BB%B6-GitHub%20Pages-blue?logo=rust&logoColor=white)](https://liam8846.github.io/pcan_kit/)
[![授權](https://img.shields.io/github/license/liam8846/pcan_kit?label=%E6%8E%88%E6%AC%8A)](#授權)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange?logo=rust&logoColor=white)](#相容性承諾)
[![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%20%7C%20Linux-lightgrey)](#支援矩陣)

> 以 Rust 2024 編寫的產品級 CAN／CAN FD 通訊函式庫：一套 API、兩種後端、
> 全非同步、熱路徑零堆積配置。

`pcan_kit` 讓應用程式用同一組型別操作 **PEAK PCAN-Basic**（Windows／Linux）
與 **Linux SocketCAN**。後端在執行期由 URI 選擇，連線由監督器負責重連與設定
重放，傳送路徑具備明確的有限背壓，並內建週期傳送與請求—回應交易。

---

## 目錄

- [特色](#特色)
- [支援矩陣](#支援矩陣)
- [安裝](#安裝)
- [快速上手](#快速上手)
- [前置設定](#前置設定)
- [範例](#範例)
- [架構](#架構)
- [核心概念](#核心概念)
- [設計取捨](#設計取捨)
- [品質保證](#品質保證)
- [硬體驗收清單](#硬體驗收清單)
- [變更紀錄](#變更紀錄)
- [相容性承諾](#相容性承諾)
- [授權](#授權)

## 特色

- **單一 API、雙後端。** `open("pcan://usb1?bitrate=500k")` 與
  `open("socketcan://can0")` 回傳同一個 `Link` 型別；後端差異由
  `Capabilities` 與 `ActiveFeatures` 誠實回報，而不是靜默模擬。
- **熱路徑零配置。** `Frame` 固定 72 bytes 且為 `Copy`，可容納最大 64-byte
  CAN FD payload。正常 RX／TX 不需要 `Vec`，也不需要 `Box<dyn Future>`。
- **可上線的連線監督。** 指數退避加 jitter 的自動重連、重連後重放完整通道
  設定（bitrate、filter、listen-only、echo、bus-off autoreset），並依
  `FaultKind` 區分「重試」「回報」「重建」「停止」。
- **明確的背壓。** 雙段 bounded 傳送佇列、可查詢的佇列深度、帶遲滯的高水位
  事件，以及斷線期間可選的待送政策（`Hold`／`FailFast`／`DropAll`）。
- **協定層工具。** 週期傳送（可原地更新 payload、限制突發、處理落後）與
  UDS 風格請求—回應交易（prefix matcher、多幀收集、逾時）。
- **編譯期不需要 PEAK SDK。** PCAN-Basic 以 `libloading` 在執行期載入；沒有
  驅動的機器會得到明確的 `LoadError`，而不是連結失敗或 panic。
- **無硬體也能測。** `FakeTransport` 可驅動所有上層邏輯，CI 另以 Linux
  `vcan` 跑端到端整合測試。

## 支援矩陣

| 能力 | `pcan-basic`（Windows／Linux） | `pcan-socketcan`（Linux） |
|---|:--:|:--:|
| CAN 2.0A／2.0B | ✅ | ✅ |
| CAN FD ＋ BRS | ✅ 依裝置與 DLL | ✅ 依核心與介面 |
| 事件驅動 RX | ✅ Win32 Event／`AsyncFd` | ✅ `AsyncFd` |
| 硬體時間戳 | ✅ | ➖ 核心時間戳（`SO_TIMESTAMPNS`） |
| 錯誤幀 | ✅ | ✅ |
| 狀態幀 | ✅ | ➖ 由錯誤幀推導 |
| 本地回音 | ✅ 依 DLL 支援 | ✅ |
| 硬體／核心層過濾 | ✅ 單一連續區間 | ✅ |
| 唯聽模式 | ✅ | ➖ 需由 `ip link` 設定，開啟時明確拒絕 |
| 位元率由函式庫設定 | ✅ | ➖ 由 `ip link` 管理 |

✅ 支援　➖ 不支援或由系統層負責（皆由 API 明確回報，不靜默降級）

## 安裝

本 workspace 使用 Rust edition 2024，MSRV 為 **1.88**。目前尚未發布至
crates.io，請以 git 相依引入並鎖定標籤：

```toml
[dependencies]
pcan-kit = { git = "https://github.com/liam8846/pcan_kit", tag = "v0.2.3" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
```

函式庫**不包含也不重新散布** PEAK 的驅動或 DLL，請見[前置設定](#前置設定)。

### Cargo features

| feature | 預設 | 說明 |
|---|:--:|---|
| `basic` | ✅ | PCAN-Basic 執行期動態載入後端 |
| `socketcan` | ✅ | Linux SocketCAN；Windows 相依圖完全不包含此 crate |
| `tracing` | ✅ | 連線、降級與過濾器未下推等診斷紀錄 |
| `serde` | — | 核心值型別的 `Serialize`／`Deserialize` |
| `test-util` | — | 匯出無硬體測試用 `FakeTransport` |

`pcan-core` 另以預設 feature `embedded-can` 提供 `CanId` 與
`embedded_can::Id` 的雙向轉換。

**CAN FD 刻意不是 Cargo feature。** `Frame` 永遠能表示 FD；驅動、核心、通道
與硬體是否實際支援是執行期性質，應查詢 `Capabilities` 與 `ActiveFeatures`，
而不是編譯兩套協定型別。

## 快速上手

支援的 URI：

| URI | 說明 |
|---|---|
| `pcan://usb1?bitrate=500k` | PCAN-USB 通道 1，500 kbit/s 古典 CAN |
| `pcan://usb1?bitrate=500k&dbitrate=2m` | CAN FD，2 Mbit/s 資料相位 |
| `pcan://pci2?bitrate=250k&listen_only=true` | PCAN-PCI 通道 2，唯聽 |
| `socketcan://can0` | SocketCAN 介面（位元率由 `ip link` 管理） |
| `socketcan://can0?fd=true` | 啟用 CAN FD 幀格式 |

查詢參數僅接受 `bitrate`、`dbitrate`、`listen_only`、`fd`；未知的鍵會直接
回報錯誤，而不是被忽略。

以下片段涵蓋開啟、訂閱、傳送、週期傳送與 UDS 風格交易：

```rust,no_run
use std::time::Duration;
use pcan_kit::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let link = open("socketcan://can0?fd=true").await?;
    link.wait_connected().await?;

    let response_id = CanId::standard(0x7e8)?;
    let mut subscription = link
        .subscribe_filter(FilterSet::with(FilterRule::exact(response_id)))
        .await?;

    let request_id = CanId::standard(0x7df)?;
    let request = Frame::new(request_id, &[0x02, 0x10, 0x01])?;
    link.send_await(request).await?;

    let heartbeat = Frame::new(CanId::standard(0x100)?, &[1, 2, 3, 4])?;
    let cyclic = link.schedule_cyclic(
        CyclicConfig::new(heartbeat, Duration::from_millis(100))
    )?;
    cyclic.set_payload(&[4, 3, 2, 1])?;

    let spec = ResponseSpec::new(
        Matcher::IdAndPrefix {
            id: response_id,
            prefix: PrefixPattern::new(&[0x02, 0x50])?,
        },
        Duration::from_millis(500),
    );
    let reply = link.request(request, &spec).await?;
    println!("交易回應：{reply:?}");

    if let Some(frame) = subscription.recv().await {
        println!("訂閱收到：{frame:?}");
    }
    cyclic.stop().await?;
    link.close().await;
    Ok(())
}
```

`open()` 只建立監督任務並立即返回，不等待實體通道連上；要等待請明確呼叫
`wait_connected()`。需要調整退避、佇列容量、逾時或待送政策時，改用
`Link::builder(factory)` 逐項設定。

### 列舉可用通道

`list_channels()` 會非同步列出所有已編譯後端目前看得到的通道。每筆
`ChannelInfo` 都能提供可直接交給 `open()` 的 URI、人類可讀名稱、目前是否可
開啟，以及硬體是否具備 CAN FD 能力。

```rust,no_run
use pcan_kit::list_channels;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    for channel in &list_channels().await? {
        println!(
            "{}：{}（可用：{}，CAN FD：{}）",
            channel.uri(),
            channel.display_name(),
            channel.is_available(),
            channel.supports_fd(),
        );
    }
    Ok(())
}
```

單一後端不可用會被略過，而不會讓整體列舉失敗：未安裝 PCAN-Basic 驅動時，
Linux 上仍可取得 SocketCAN 介面；所有後端都不可用或沒有硬體時，結果是空切
片。若應用程式直接呼叫較低階的 `pcan_basic::list_channels()`，舊版
PCAN-Basic 不支援 `PCAN_ATTACHED_CHANNELS` 時會明確回傳 `Error::Unsupported`。

## 前置設定

<details>
<summary><b>Windows：PCAN-Basic</b></summary>

從 [PEAK-System PCAN-Basic 官方頁面](https://www.peak-system.com/support/software-information/development-kits/pcan-basic/)
安裝 PCAN 驅動與 API。標準安裝後 Windows 會由安全搜尋目錄找到
`PCANBasic.dll`；也可將環境變數 `PCAN_BASIC_LIB` 設為 DLL 的**絕對路徑**：

```powershell
$env:PCAN_BASIC_LIB = 'C:\Program Files\PEAK-System\PCAN-Basic API\x64\PCANBasic.dll'
```

請確認程序架構（x64）與 DLL 相符。

</details>

<details>
<summary><b>Linux：PCAN-Basic</b></summary>

安裝 PEAK 的 chardev 驅動與
[PCAN-Basic for Linux](https://www.peak-system.com/fileadmin/media/linux/can-pcan-basic.php)，
確認動態載入器能找到 `libpcanbasic.so`；亦可將 `PCAN_BASIC_LIB` 指向 `.so`
的絕對路徑。

</details>

<details>
<summary><b>Linux：SocketCAN</b></summary>

位元率由系統管理，函式庫不猜測也不修改：

```bash
sudo ip link set can0 down
sudo ip link set can0 up type can bitrate 500000
```

CAN FD：

```bash
sudo ip link set can0 down
sudo ip link set can0 up type can bitrate 500000 dbitrate 2000000 fd on
```

</details>

<details>
<summary><b>Linux：無硬體的虛擬 CAN（vcan）</b></summary>

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set vcan0 mtu 72        # 容納 CAN FD 幀
sudo ip link set up vcan0
cargo run -p pcan-kit --example monitor -- 'socketcan://vcan0'
cansend vcan0 123#01020304
```

刪除介面前先停止程式，再執行 `sudo ip link del vcan0`。**WSL2 隨附核心通常
沒有 `vcan` 模組**；`modprobe: FATAL: Module vcan not found` 是環境限制，不
是本函式庫故障，可改用自建 WSL2 核心、完整 Linux VM 或實體 Linux 主機。

</details>

## 範例

| 範例 | 需要硬體 | 內容 |
|---|:--:|---|
| `monitor` | ✅ | 開啟真實後端並監看第一個幀或匯流排事件 |
| `list_channels` | ➖ | 列舉所有後端目前可見的通道 |
| `echo` | ➖ | 依過濾器收到指定 ID 後原樣回送 |
| `cyclic` | ➖ | 週期傳送、原地更新 payload 與 detach |
| `uds_request` | ➖ | `IdAndPrefix` matcher 的請求—回應交易 |

```bash
cargo run -p pcan-kit --example monitor -- 'pcan://usb1?bitrate=500k'
cargo run -p pcan-kit --example monitor -- 'socketcan://can0'
cargo run -p pcan-kit --example list_channels
cargo run -p pcan-kit --example echo
cargo run -p pcan-kit --example cyclic
cargo run -p pcan-kit --example uds_request
```

除 `monitor` 與 `list_channels` 外，其餘範例使用 `FakeTransport`，沒有硬體
也能完整執行。

## 架構

```text
應用程式
   │
pcan-kit                 facade、URI、執行期列舉分派
   ├── pcan-link          重連、路由、TX 佇列、週期與交易
   │      └── pcan-core   Frame、設定、錯誤、Transport trait
   ├── pcan-basic         Windows/Linux PCAN-Basic 後端
   │      └── pcan-basic-sys  固定 ABI FFI、libloading
   └── pcan-socketcan     Linux libc + AsyncFd 後端
```

| crate | 職責 |
|---|---|
| `pcan-core` | 72-byte `Frame`、CAN ID、過濾器、狀態、錯誤與零裝箱 `Transport` trait |
| `pcan-link` | 指數退避重連、訂閱路由、背壓、週期排程與交易 |
| `pcan-basic-sys` | PCAN-Basic C ABI、狀態位元分類與安全的執行期載入 |
| `pcan-basic` | PCAN-Basic 通道設定、事件驅動 RX 與有限 TX 重試 |
| `pcan-socketcan` | Linux raw CAN socket、核心時間戳與錯誤幀解析 |
| `pcan-kit` | 常用型別重匯出、後端列舉分派、URI 與範例 |

只有 `Transport` 實作接觸作業系統與 FFI；重連、佇列、路由、週期與交易等全部
上層邏輯都能以 `FakeTransport` 在無硬體環境完整驅動。

## 核心概念

### 能力資訊的三個層次

三者回答不同問題，刻意不合併成同一個 `bool`：

| 層 | 型別 | 回答的問題 |
|---|---|---|
| 硬體 | `ChannelInfo`（開啟前的列舉 API） | 這個裝置本身支援什麼 |
| 後端 | `Capabilities`（`Link::capabilities()`） | 已開啟的傳輸層做得到什麼 |
| 本次 | `ActiveFeatures`（`Link::active_features()`） | 這次開啟實際啟用了什麼 |

後兩者可以合法地不同：以古典位元率開啟一張支援 FD 的卡片時，
`Capabilities::can_fd` 為 `true` 而 `ActiveFeatures::can_fd` 為 `false`。

### 錯誤分類與重連

`FaultKind` 決定監督器的處置方式：

| 類別 | 意義 | 行為 |
|---|---|---|
| `Transient` | 短暫背壓，例如 TX queue 滿 | 原地有限重試，不重建通道 |
| `Recoverable` | 匯流排警告但連線仍可用 | 上報事件與統計，繼續收送 |
| `Fatal` | 通道已不可用，例如 Bus-Off、拔除、介面 down | 關閉後退避重連 |
| `Permanent` | 設定、模式或呼叫本身錯誤 | 停止重連並回報失敗 |

預設退避從 100 ms 開始、每次乘二、上限 30 秒，並加入 ±25% jitter；成功穩定
60 秒後才把嘗試計數歸零，避免反覆插拔時一直以高頻率重試。

PCAN 開啟所需的阻塞 FFI 與接收執行緒建立會在 Tokio 阻塞執行緒池執行，因此
`LinkBuilder::open_timeout`（預設 5 秒）能正常限制監督器的等待時間，也不會
凍結非同步工作執行緒。逾時無法取消已開始的阻塞工作；該工作仍會跑完並自行
清理，而同一工廠的下一次開啟會等待前一次工作連同清理完全結束，避免舊工作的
`CAN_Uninitialize` 關閉新通道。

斷線期間的待送政策由 `PendingTxPolicy` 決定。預設 `Hold` 搭配
`max_pending_age`（預設 1 秒）：短暫 USB 重列舉期間保留待送幀，但不會在重連
後補送危險的陳舊控制命令。安全關鍵命令可改用 `FailFast`，允許遺失的遙測資料
可用 `DropAll`。

### 傳送背壓與生命週期

傳送路徑有兩段固定上限的佇列：應用程式直接排入的 bounded channel，以及傳送
工作者已取走、等待重連或送上匯流排的暫存段。兩段的單段容量都由
`LinkBuilder::tx_queue_capacity`（預設 256）設定，`Link::tx_queue_depth()`
回傳的 `TxQueueDepth` 可分別觀察：

- `channel`：`try_send` 直接面對的排隊量，`utilization()` 可預測何時會回傳
  `Error::TxQueueFull`。
- `staged`：工作者已取走但尚未送出的排隊量。
- `total()`：兩段總積壓，適合觀察端到端延遲壓力。

建構器預設以 `tx_high_water_ratio(Some(0.8))` 啟用主動背壓。channel 段越過
門檻時廣播 `BusEvent::TxQueueHighWater`，跌回門檻減 0.15 時才廣播
`BusEvent::TxQueueRecovered`——這段遲滯可避免門檻附近的事件風暴；傳入 `None`
可停用。`StatsSnapshot::tx_queue_full` 只計算真正因 channel 已滿而被拒絕的
排入次數，後端傳送錯誤則由 `tx_dropped` 表達。

三個背景工作任務都有異常結束守衛。採用預設的 panic unwind 時，工作者 panic
或 future 被執行期丟棄會廣播 `BusEvent::WorkerLost`；致命工作者遺失還會把連
線推到 `LinkState::Closed`，讓狀態等待、訂閱與傳送操作結束而不會永久等待。
`panic = "abort"` 會直接終止整個程序、無法執行 Rust 的 `Drop` 守衛，因而不
在這項保證內。

最後一個 `Link` 複本被丟棄時，背景任務會自動關閉且 transport 只關閉一次。仍
建議在正常關機流程明確呼叫 `link.close().await`，如此呼叫端能等待清理完成，
而不是只依賴背景收攤。

接收統計中，`rx_error_frames` 是透過 RX 串流收到的錯誤／狀態幀數，不包含健
康檢查主動輪詢；`rx_hw_overrun` 與 `rx_queue_overrun` 依警告位元上升緣計數，
同一個尚未清除的警告不會被重複高估。

### 交易與週期傳送

交易等待者使用預先配置的 bounded 緩衝；緩衝滿時只丟棄新抵達的回應幀，不會
把仍存在的等待者誤判為斷線。每筆交易首次丟棄時廣播一次
`BusEvent::TransactionDropped`，表示該筆交易的收集結果可能不完整，同一筆交
易的後續溢位不會形成事件風暴。同時進行的交易數由
`LinkBuilder::max_in_flight_transactions`（預設 64）限制。

`CyclicHandle` 可在不重建排程的情況下原地更新 payload、整個幀或週期，也能
暫停、恢復、單次觸發或 detach。`OverrunPolicy` 與 `with_max_burst` 決定系統
延遲造成落後時要補送幾幀，避免喚醒延遲後一次爆出大量幀。

控制通道本身必須維持 unbounded——`Drop` 也要用它送停止命令，而 `Drop` 不能
`.await`——因此上界來自命令本身的設計，而不是通道容量。三類控制命令各有不同
的處理方式：

| 類別 | 例子 | 上界來源 |
|---|---|---|
| 狀態變更 | `set_payload`、`set_frame`、`set_period`、`pause`／`resume` | 合併為最新值，單一項目最多一個未處理命令 |
| 事件 | `trigger_once` | 計數累積，封頂於 `max_burst`，溢位計入 `CyclicStats::skipped` |
| 新增項目 | `schedule_cyclic` | 固定名額准入（`MAX_PENDING_CYCLIC_ADDS`，256），用盡時回 `Error::ControlQueueFull` |
| 清理 | `Drop` 送出的停止命令 | 與存活控制代碼一一對應，且不可阻塞 |

名額在排程器取走新增命令時歸還，因此限制的是「未處理命令」而不是「存活項目
數」。訂閱與交易註冊則本來就自限：兩者都要等背景任務回覆，交易另受
`max_in_flight_transactions` 限制。

## 設計取捨

- **`Frame` 固定 72 bytes 且為 `Copy`。** 一個值即可容納最大 64-byte FD
  payload，接收、傳送與路由只搬堆疊值，不需要 `Vec`、引用生命週期或
  per-frame allocation。
- **執行期後端使用 `AnyTransport` 列舉分派，不使用 `dyn Transport`。**
  現有 RPITIT trait 可讓 future 留在堆疊上；若改成 trait object，每次約
  9000 frames/s 的滿載接收都要付一次 `Box<dyn Future>`。
- **PCAN RX 使用驅動事件，不以 1 ms Tokio timer 輪詢。** Linux 是
  `AsyncFd`，Windows 是等待 Win32 Event 的專用執行緒。只有舊版 Linux 驅動
  明確拒絕 `PCAN_RECEIVE_EVENT` fd 時才記錄警告並降級。
- **硬體過濾器只在能精確表示時才下推。** PCAN 硬體過濾器只能表示單一連續
  ID 區間，因此只有單規則、非反轉、低位 wildcard 連續的遮罩會下推；其他集
  合會記錄 debug 診斷、開放硬體 filter，並保留 `pcan-link` Router 的完整軟
  體語意——寧可多收也絕不靜默漏幀。

## 品質保證

CI 在 Ubuntu 與 Windows 上執行下列 job：

| job | 內容 |
|---|---|
| 格式檢查 | `cargo fmt --all -- --check` |
| 靜態分析 | `cargo clippy --workspace --all-features --all-targets -- -D warnings`（Linux ＋ Windows） |
| 建置與測試 | 全功能建置、範例建置、`cargo test`，另驗證 `--no-default-features` |
| 文件建置 | `cargo doc --workspace --all-features`，`RUSTDOCFLAGS=-D warnings` |
| MSRV | 以宣告的最低版本 `cargo check`，並以 `RUSTUP_TOOLCHAIN` 覆寫工具鏈固定檔 |
| 虛擬 CAN | 在 `vcan` 上跑 SocketCAN 傳輸層、Link 端到端與破壞性整合測試 |
| 功能組合冪集 | `cargo hack --feature-powerset check`，驗證 feature 為加性 |
| 供應鏈稽核 | `cargo deny check`，另有每週排程的 RUSTSEC 公告掃描 |

workspace 層級的 lint 設定包含 `clippy::all = deny`、`clippy::pedantic`、
`missing_docs`、`unsafe_op_in_unsafe_fn = deny` 與
`undocumented_unsafe_blocks = deny`：**每個 `unsafe` 區塊都必須寫出安全性理
由**。開發工具鏈由 `rust-toolchain.toml` 固定，與 `Cargo.toml` 宣告的 MSRV
是兩件獨立、分別驗證的事。

本地可用同一組指令重現：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
```

## 硬體驗收清單

開發與 CI 環境沒有 PEAK 硬體，因此發行前應逐項完成下列驗證。

<details>
<summary><b>1. 驅動與函式庫載入</b></summary>

```bash
cargo run -p pcan-kit --example monitor -- 'pcan://usb1?bitrate=500k'
```

- 顯示 `LoadError::NotFound`／「找不到後端函式庫」時，檢查驅動安裝、程序架
  構（x64）及 `PCAN_BASIC_LIB` 是否為正確絕對路徑。
- 載入成功但通道不存在時，應得到乾淨的 `Error::Open`，不得 panic。
- Windows 可用 Process Explorer、Linux 可用 `ldd`／`LD_DEBUG=libs` 確認實際
  載入的檔案，但不要把不可信目錄加進全域搜尋路徑。

</details>

<details>
<summary><b>2. 兩個 PCAN 通道對接收發</b></summary>

1. 用有正確 120 Ω 終端的 CAN 線連接 USB1 與 USB2，兩端共地。
2. 兩邊都設 500 kbit/s，一端執行
   `cargo run -p pcan-kit --example monitor -- 'pcan://usb1?bitrate=500k'`。
3. 另一端用 PCAN-View 或小型測試程式從 USB2 送標準 ID `0x123`、8-byte
   payload，再反向傳送。
4. 核對 ID、標準／擴充格式、RTR、payload、時間戳與回音旗標；連續滿載至少十
   分鐘，並確認 `rx_queue_overrun`、`rx_hw_overrun` 不增加。

</details>

<details>
<summary><b>3. 自動重連與設定重放</b></summary>

1. 訂閱 `link.events()`，並先套用非全開的過濾器。
2. 穩定收發時拔掉 PCAN-USB，應依序看到 Bus-Off／讀取故障與
   `BusEvent::Reconnecting`，而不是 task 靜默停止。
3. 重新插回同一通道，應看到 `Connected`，且退避次數與 delay 合理。
4. 從兩個不同 ID 送幀，確認重連後原本的 bitrate、listen-only、錯誤／狀態
   幀、echo、bus-off autoreset 與硬體 filter 都重新套用。可表示的連續區間應
   由硬體擋掉範圍外幀；複雜遮罩則應看到「未下推、軟體過濾」診斷。

</details>

<details>
<summary><b>4. CAN FD 與 BRS</b></summary>

1. 兩端都使用支援 FD 的介面，開啟 `pcan://usb1?bitrate=500k&dbitrate=2m`。
2. 連線後確認 `Capabilities::can_fd`、`brs` 與 `ActiveFeatures` 一致；舊 DLL
   不可假裝支援。
3. 傳送標準與擴充 ID 的 64-byte BRS 幀，逐 byte 對照接收端，並測試所有合法
   長度：0–8、12、16、20、24、32、48、64。
4. 關閉對端 FD 後再送，確認錯誤能被觀測，且不會把 FD 幀誤當古典幀。

</details>

<details>
<summary><b>5. 與外部工具交叉對照</b></summary>

Linux SocketCAN：

```bash
candump -L can0
cansend can0 123#1122334455667788
```

FD 可用 `candump can0` 搭配支援 FD 的 `cansend` 語法，核對 BRS 與長度。
Windows／PCAN-Basic 則以 PCAN-View 在相同 nominal／data bitrate 下監看，對
照方向、時間戳、錯誤狀態與 bus load。

</details>

## 變更紀錄

完整紀錄見 [CHANGELOG.md](CHANGELOG.md)，各版本的發行說明與執行檔則在
[Releases](https://github.com/liam8846/pcan_kit/releases)。

兩者都由 [git-cliff](https://git-cliff.org) 依 commit 訊息的 conventional
prefix 自動產生（設定在 [`cliff.toml`](cliff.toml)），推送 `v*` 標籤時：

1. 發行工作流程以 `git cliff --current` 產生該版本的分類說明，作為 Release
   內容；
2. 另一個 job 重新產生完整的 `CHANGELOG.md` 並回寫至 `master`。

因此 `CHANGELOG.md` 裡某個版本的區段，是在該版本標籤推出**之後**才被提交
的。要在本機預覽：

```bash
git cliff --output CHANGELOG.md          # 完整檔案
git cliff --unreleased                   # 尚未發行的部分
```

這也是本專案要求 commit 訊息使用 `feat:`／`fix:` 等前綴的原因——changelog 的
品質等同 commit 訊息的品質。不符合格式的 commit（例如 merge commit）會被排除
而不是丟進「其他」分類。

## 相容性承諾

- **MSRV：Rust 1.88。** 提高 MSRV 視為破壞性變更，會伴隨 minor 版本更新，
  並由 CI 的 `msrv` job 獨立驗證。
- **SemVer：** 0.x 期間，破壞性變更會提升 minor 版本。公開列舉與設定結構都
  標記 `#[non_exhaustive]`，新增成員不算破壞性變更。
- **平台：** Windows 與 Linux。SocketCAN 僅限 Linux，在 Windows 上完全不進
  入相依圖。
- **工具鏈固定：** `rust-toolchain.toml` 只影響本工作目錄樹內的建置；下游以
  相依套件形式使用時不會讀到它。

## 授權

本專案採雙重授權，可任選其一：

- MIT 授權（[LICENSE-MIT](LICENSE-MIT)）
- Apache 授權 2.0 版（[LICENSE-APACHE](LICENSE-APACHE)）

除非另有明確聲明，你有意提交並納入本專案的貢獻，均以上述雙重授權提供，不附
加任何額外條款。
