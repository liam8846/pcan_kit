# 測試與驗收

## 本機自動檢查

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
cargo clippy --workspace --no-default-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
cargo deny check
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
```

Linux PCAN 生命週期測試需要系統 C 編譯器，測試替身只驗證 FFI 與資源清理行為。
Windows CI 會建置與執行無硬體邏輯，但不宣稱測過原廠 DLL、驅動或 PEAK 裝置。

## 實際 vcan 整合測試

先依[環境設定](getting-started.md)建立獨立介面 `vcan0` 與 `vcan9`，兩者 MTU 設為 72。
下列命令會改變或中斷 `vcan9`，不得指定正在使用的實體介面。

```bash
PCAN_VCAN=vcan0 PCAN_VCAN_REQUIRED=1 \
  cargo test -p pcan-socketcan --features vcan-tests --test vcan --locked -- --test-threads=1 --nocapture
PCAN_VCAN=vcan0 PCAN_VCAN_ADMIN=vcan9 PCAN_VCAN_REQUIRED=1 \
  cargo test -p pcan-kit --features vcan-tests --test vcan_link --locked -- --test-threads=1 --nocapture
PCAN_VCAN_ADMIN=vcan9 PCAN_VCAN_REQUIRED=1 \
  cargo test -p pcan-socketcan --features vcan-tests --test vcan_admin --locked -- --test-threads=1 --nocapture
```

未設定測試介面時，部分測試會提前返回。這不代表端到端驗證通過；CI 以
`PCAN_VCAN_REQUIRED=1` 要求實際介面，缺少介面就失敗，不會把跳過當成成功。

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


依[驗收紀錄範本](acceptance-template.md)記錄環境、結果與完整受驗 SHA。
