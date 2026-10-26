# 核心概念與設計取捨

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

時間窗收集的最終結果另由 `ResponseSpec::max_responses` 限制，預設 64，可用
`with_max_responses(NonZeroUsize)` 調整。收到超過上限的回應時回傳
`TransactionError::ResponseLimitExceeded`，不會繼續擴張結果容器；接收通道容量
最多為 64，與結果上限分開計算。`First` 與 `Exactly(n)` 則分別以 1 與 n
為上限。截止時間優先於已排隊的回應，持續流量不會延後交易結束。

訂閱與交易的註冊確認訊息會持有自動清理守衛；即使在確認已送出、呼叫端尚未
取得控制代碼時取消，也會退訂或註銷，不需要等下一個符合條件的幀才回收。

`CyclicHandle` 可在不重建排程的情況下原地更新 payload、整個幀或週期，也能
暫停、恢復、單次觸發或 detach。`OverrunPolicy` 與 `with_max_burst` 決定系統
延遲造成落後時要補送幾幀，避免喚醒延遲後一次爆出大量幀。

訂閱、交易與週期排程的清理通道使用 unbounded，讓 `Drop` 能同步送出清理命令。
資源上界由生命週期名額與命令合併共同維持：

`LinkBuilder::control_capacity(n)` 設定每類項目的獨立名額，預設訂閱、交易與
週期項目各 1024。名額涵蓋待建立、使用中與待清理階段，最後一個控制代碼、
背景項目及其命令都釋放後才歸還。清理不另取名額，因此滿載也能退訂、註銷與停止。
零視為 1，超過 Tokio 信號量上限時截至該上限。

| 類別 | 例子 | 上界來源 |
|---|---|---|
| 狀態變更 | `set_payload`、`set_frame`、`set_period`、`pause`／`resume` | 合併為最新值，單一項目最多一個未處理命令 |
| 事件 | `trigger_once` | 計數累積，封頂於 `max_burst`，溢位計入 `CyclicStats::skipped` |
| 新增訂閱／交易 | `subscribe`、`prepare` | 送命令前先取得該類生命週期名額，耗盡立即回報錯誤 |
| 新增週期項目 | `schedule_cyclic` | 生命週期名額，加上獨立的待處理新增上限（`MAX_PENDING_CYCLIC_ADDS`，256） |
| 清理 | `Drop` 送出的退訂、註銷與停止命令 | 沿用原名額，每個控制代碼最多一筆；取消 `stop()` 不重複送出 |

訂閱與週期項目耗盡名額時回 `Error::ControlQueueFull`，交易則回
`TransactionError::ControlQueueFull`；錯誤中的 `capacity` 是該次命中的上限。
交易還受 `max_in_flight_transactions` 的活躍交易數限制，超過時回
`TooManyInFlight`。這些是項目數上限，各訂閱的緩衝與交易的回應數仍由其設定決定。

即使訂閱因溢位關閉、交易已斷線或有限次週期已結束，仍持有的控制代碼也占名額，
應主動釋放。`detach()` 將週期項目交給背景排程，名額保留到項目結束或連線關閉。
取消尚未完成的新增操作後，需等背景任務丟棄該命令，名額才會恢復。

公開 API 範圍與監督器責任分工見[架構設計說明](link-runtime.md)。

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
- **Linux RX 明確持有事件 fd。** SocketCAN 持有自己的 socket；PCAN 複製
  驅動提供的事件 fd 並獨立持有，停止時先反註冊 reactor，再解除通道初始化。
  接收與停止以同步鎖協調，鎖不跨越非同步等待。Tokio 最低版本為 1.54.1。
- **硬體過濾器只在能精確表示時才下推。** PCAN 硬體過濾器只能表示單一連續
  ID 區間，因此只有單規則、非反轉、低位 wildcard 連續的遮罩會下推；其他集
  合會記錄 debug 診斷、開放硬體 filter，並保留 `pcan-link` Router 的完整軟
  體語意——寧可多收也絕不靜默漏幀。
