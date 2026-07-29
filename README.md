# pcan_kit

[![CI](https://img.shields.io/github/actions/workflow/status/liam8846/pcan_kit/ci.yml?branch=master&label=CI)](https://github.com/liam8846/pcan_kit/actions/workflows/ci.yml)
[![授權](https://img.shields.io/github/license/liam8846/pcan_kit?label=%E6%8E%88%E6%AC%8A)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-orange?logo=rust)](Cargo.toml)

`pcan_kit` 是以 Rust 2024 編寫的產品級 CAN 通訊函式庫。它同時支援
Windows／Linux 的 PEAK PCAN-Basic 與 Linux SocketCAN，並提供 CAN FD、
自動重連、推送式訂閱、bounded 傳送佇列、週期傳送與請求—回應交易。
`Frame` 與後端 C 結構都使用固定大小堆疊值；正常 RX/TX 熱路徑不配置堆積
記憶體。

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
| `pcan-core` | 72-byte `Frame`、CAN ID、過濾器、狀態、錯誤與零裝箱傳輸 trait |
| `pcan-link` | 指數退避重連、訂閱路由、背壓、週期排程與交易 |
| `pcan-basic-sys` | PCAN-Basic C ABI、狀態位元分類及安全的執行期載入 |
| `pcan-basic` | PCAN-Basic 通道設定、事件驅動 RX 與有限 TX 重試 |
| `pcan-socketcan` | Linux raw CAN socket、核心時間戳與錯誤幀解析 |
| `pcan-kit` | 常用型別重匯出、後端列舉分派、URI 與 examples |

## 安裝與前置需求

本 workspace 使用 Rust edition 2024。函式庫不包含或重新散布 PEAK 的驅動。

### Windows：PCAN-Basic

從 [PEAK-System PCAN-Basic 官方頁面](https://www.peak-system.com/support/software-information/development-kits/pcan-basic/)
安裝 PCAN 驅動與 API。標準安裝後會由 Windows 安全搜尋目錄找到
`PCANBasic.dll`；也可以將環境變數 `PCAN_BASIC_LIB` 設為 DLL 的**絕對路徑**：

```powershell
$env:PCAN_BASIC_LIB = 'C:\Program Files\PEAK-System\PCAN-Basic API\x64\PCANBasic.dll'
```

### Linux：PCAN-Basic

安裝 PEAK 的 chardev 驅動與
[PCAN-Basic for Linux](https://www.peak-system.com/fileadmin/media/linux/can-pcan-basic.php)，
確認動態載入器可找到 `libpcanbasic.so`。也可將 `PCAN_BASIC_LIB` 指到
`.so` 的絕對路徑。

### Linux：SocketCAN

位元率由系統管理，不由函式庫猜測或修改：

```bash
sudo ip link set can0 down
sudo ip link set can0 up type can bitrate 500000
```

CAN FD：

```bash
sudo ip link set can0 down
sudo ip link set can0 up type can bitrate 500000 dbitrate 2000000 fd on
```

## 快速上手

URI 範例：

- `pcan://usb1?bitrate=500k`
- `pcan://usb1?bitrate=500k&dbitrate=2m`
- `pcan://pci2?bitrate=250k&listen_only=true`
- `socketcan://can0`
- `socketcan://can0?fd=true`

以下片段示範開啟、訂閱、傳送、週期傳送與 UDS 風格交易：

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
    println!("交易回應：{:?}", reply);

    if let Some(frame) = subscription.recv().await {
        println!("訂閱收到：{:?}", frame);
    }
    cyclic.stop().await?;
    link.close().await;
    Ok(())
}
```

完整範例位於 `crates/pcan-kit/examples/`：

```bash
cargo run -p pcan-kit --example monitor -- 'socketcan://can0'
cargo run -p pcan-kit --example echo
cargo run -p pcan-kit --example cyclic
cargo run -p pcan-kit --example uds_request
```

後三個範例使用 `FakeTransport`，沒有硬體也能執行。

## Features

| feature | 預設 | 說明 |
|---|---:|---|
| `basic` | 是 | PCAN-Basic 執行期動態載入後端 |
| `socketcan` | 是 | Linux SocketCAN；Windows 相依圖完全不包含此 crate |
| `tracing` | 是 | 連線、降級與無法下推硬體過濾器的診斷紀錄 |
| `serde` | 否 | 核心值型別的 serde 支援 |
| `test-util` | 否 | 匯出無硬體測試用 `FakeTransport` |

CAN FD 刻意不是 Cargo feature。`Frame` 永遠能表示 FD；驅動、核心、通道與
硬體是否實際支援，是執行期性質，應查詢 `Link::capabilities()` 的
`Capabilities::can_fd`／`brs`，而不是編譯兩套協定型別。

部分 PCAN 裝置不支援錯誤幀或狀態幀參數；這些可選參數設定失敗時仍會建立
連線，並由 `Capabilities::error_frames`／`status_frames` 回報執行期協商後
實際可用的能力。明確要求但裝置無法提供時會以 warn 等級記錄；原本即關閉
的能力則只以 debug 等級記錄。

## 錯誤分類與重連

`FaultKind` 決定監督器處置：

| 類別 | 意義 | 行為 |
|---|---|---|
| `Transient` | 短暫背壓，例如 TX queue 滿 | 原地有限重試，不重建通道 |
| `Recoverable` | 匯流排警告但連線仍可用 | 上報事件與統計，繼續收送 |
| `Fatal` | 通道已不可用，例如 Bus-Off、拔除、介面 down | 關閉後退避重連 |
| `Permanent` | 設定、模式或呼叫本身錯誤 | 停止重連並回報失敗 |

預設退避從 100 ms 開始、每次乘二、上限 30 秒，加入 ±25% jitter；成功穩定
60 秒後才把嘗試計數歸零，避免反覆插拔時一直以高頻率重試。

PCAN 開啟所需的阻塞 FFI 與接收執行緒建立會在 Tokio 阻塞執行緒池執行，
因此 `LinkBuilder::open_timeout` 能正常限制監督器的等待時間，也不會凍結
非同步工作執行緒。逾時無法取消已開始的阻塞工作；該工作仍會跑完並自行
清理，而同一工廠的下一次開啟會等待前一次工作連同清理完全結束，避免舊
工作的 `CAN_Uninitialize` 關閉新通道。

`PendingTxPolicy::Hold` 是預設：短暫 USB 重列舉期間保留待送幀，但
`max_pending_age` 預設只允許 1 秒。這同時避免把短暫斷線直接暴露成大量應用
錯誤，也不會在重連後補送危險的陳舊控制命令。安全關鍵命令可改用
`FailFast`；遙測類且允許遺失的資料可用 `DropAll`。

## 傳送背壓與生命週期

傳送路徑有兩段固定上限的佇列：應用程式直接排入的 bounded channel，以及
傳送工作者已取走、等待重連或送上匯流排的暫存段。兩段的單段容量都由
`LinkBuilder::tx_queue_capacity` 設定。`Link::tx_queue_depth()` 會回傳
`TxQueueDepth`：

- `channel` 是 `try_send` 直接面對的排隊量，`utilization()` 可預測何時會
  回傳 `Error::TxQueueFull`。
- `staged` 是工作者已取走但尚未送出的排隊量。
- `total()` 是兩段總積壓，適合觀察端到端延遲壓力。

建構器預設以 `tx_high_water_ratio(Some(0.8))` 啟用主動背壓。channel 段
越過門檻時廣播 `BusEvent::TxQueueHighWater`，跌回門檻減 0.15 時才廣播
`BusEvent::TxQueueRecovered`；這段遲滯可避免門檻附近的事件風暴。傳入
`None` 可停用。`StatsSnapshot::tx_queue_full` 只計算真正因 channel 已滿而
被拒絕的排入次數；後端傳送錯誤則由 `tx_dropped` 表達。

三個背景工作任務都有異常結束守衛。採用預設的 panic unwind 時，工作者
panic 或 future 被執行期丟棄會廣播 `BusEvent::WorkerLost`；致命工作者遺失
還會把連線推到 `LinkState::Closed`，讓狀態等待、訂閱與傳送操作結束而不會
永久等待。`panic = "abort"` 會直接終止整個程序，無法執行 Rust 的 `Drop`
守衛，因而不在這項保證內。

最後一個 `Link` 複本被丟棄時，背景任務會自動關閉且 transport 只關閉一次。
仍建議在正常關機流程明確呼叫 `link.close().await`，如此呼叫端能等待清理
完成，而不是只依賴背景收攤。

接收統計中，`rx_error_frames` 是透過 RX 串流收到的錯誤／狀態幀數，不包含
健康檢查主動輪詢；`rx_hw_overrun` 與 `rx_queue_overrun` 依警告位元上升緣
計數，同一個尚未清除的警告不會被重複高估。

交易等待者使用預先配置的 bounded 緩衝；緩衝滿時只丟棄新抵達的回應幀，
不會把仍存在的等待者誤判為斷線。每筆交易首次丟棄時會廣播一次
`BusEvent::TransactionDropped`，表示這筆交易的收集結果可能不完整，同一
筆交易的後續溢位不會形成事件風暴。

## 主要設計取捨

- `Frame` 固定 72 bytes 且為 `Copy`：一個值即可容納最大 64-byte FD
  payload，接收、傳送與路由只搬堆疊值，不需要 `Vec`、引用生命週期或
  per-frame allocation。
- 執行期後端使用 `AnyTransport` 列舉分派，不使用 `dyn Transport`。現有
  RPITIT trait 可保持 future 在堆疊上；若做 trait object，每次約
  9000 frames/s 的滿載接收都要 `Box<dyn Future>`。
- PCAN RX 使用驅動事件，不以 1 ms Tokio timer 輪詢。Linux 是
  `AsyncFd`，Windows 是等待 Win32 Event 的專用執行緒。只有舊版 Linux
  驅動明確拒絕 `PCAN_RECEIVE_EVENT` fd 時才記錄警告並降級。
- PCAN 硬體過濾器只能表示單一連續 ID 區間。只有單規則、非反轉、低位
  wildcard 連續的遮罩會下推；其他集合會明確記錄 debug 診斷、開放硬體
  filter，保留 `pcan-link` Router 的完整軟體語意。

## 接上硬體後的手動驗證

開發與 CI 環境沒有 PEAK 硬體，因此發行前應逐項完成以下驗證。

### 1. 驅動與函式庫載入

Windows 開啟 PowerShell，Linux 開啟 shell，執行：

```bash
cargo run -p pcan-kit --example monitor -- 'pcan://usb1?bitrate=500k'
```

- 若顯示 `LoadError::NotFound`／「找不到後端函式庫」，檢查驅動安裝、程序
  架構（x64）及 `PCAN_BASIC_LIB` 是否為正確絕對路徑。
- 若載入成功但通道不存在，應得到乾淨的 `Error::Open`，不得 panic。
- Windows 可用 Process Explorer，Linux 可用 `ldd`／`LD_DEBUG=libs` 輔助
  確認實際載入檔案，但不要把不可信目錄加進全域搜尋路徑。

### 2. 兩個 PCAN 通道對接收發

1. 用有正確 120 Ω 終端的 CAN 線連接 USB1 與 USB2，兩端共地。
2. 兩邊都設 500 kbit/s；一端執行 `monitor`：

   ```bash
   cargo run -p pcan-kit --example monitor -- 'pcan://usb1?bitrate=500k'
   ```

3. 另一端用 PCAN-View 或小型測試程式從 USB2 送標準 ID `0x123`、8-byte
   payload，再反向傳送。
4. 核對 ID、標準／擴充格式、RTR、payload、時間戳與回音旗標；連續滿載至少
   十分鐘並確認 `rx_queue_overrun`、`rx_hw_overrun` 不增加。

### 3. 自動重連與設定重放

1. 訂閱 `link.events()` 並先套用非全開過濾器。
2. 穩定收發時拔掉 PCAN-USB；應依序看到 Bus-Off／讀取故障、
   `BusEvent::Reconnecting`，而非 task 靜默停止。
3. 重新插到同一通道；應看到 `Connected`，退避次數與 delay 合理。
4. 從兩個不同 ID 送幀，確認重連後原本的 bitrate、listen-only、錯誤／狀態
   幀、echo、bus-off autoreset 與硬體 filter 都重新套用。可表示的連續區間
   應由硬體擋掉範圍外幀；複雜遮罩則應看到「未下推、軟體過濾」診斷。

### 4. CAN FD 與 BRS

1. 兩端都使用支援 FD 的介面，開啟
   `pcan://usb1?bitrate=500k&dbitrate=2m`。
2. 連線後確認 `Capabilities::can_fd == true` 且 `brs == true`；舊 DLL 不可
   假裝支援。
3. 傳送標準與擴充 ID 的 64-byte BRS 幀，逐 byte 對照接收端，並測試所有
   合法長度：0–8、12、16、20、24、32、48、64。
4. 關閉對端 FD 後再送，確認錯誤能被觀測且不會把 FD 幀誤當古典幀。

### 5. 與外部工具交叉對照

- Linux SocketCAN：

  ```bash
  candump -L can0
  cansend can0 123#1122334455667788
  ```

  FD 可用 `candump can0` 搭配支援 FD 的 `cansend` 語法，核對 BRS 與長度。
- Windows／PCAN-Basic：以 PCAN-View 在相同 nominal/data bitrate 下監看，
  對照方向、時間戳、錯誤狀態與 bus load。

### 6. Linux vcan 無硬體測試

一般 Linux 核心可建立虛擬 CAN：

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
cargo run -p pcan-kit --example monitor -- 'socketcan://vcan0'
cansend vcan0 123#01020304
```

刪除介面前先停止程式，再執行 `sudo ip link del vcan0`。**WSL2 隨附核心通常
沒有 `vcan` 模組**；`modprobe: FATAL: Module vcan not found` 是環境限制，
不是本函式庫故障。可改用自建 WSL2 核心、完整 Linux VM 或實體 Linux 主機。

## 授權

本專案採雙重授權：**MIT OR Apache-2.0**。
