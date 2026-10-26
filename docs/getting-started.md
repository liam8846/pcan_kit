# 驅動與介面設定

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

刪除介面前先停止程式，再執行 `sudo ip link del vcan0`。若執行環境的核心沒有 `vcan` 模組，`modprobe: FATAL: Module vcan not found` 是環境限制，不
是本函式庫故障，可改用自建 WSL2 核心、完整 Linux VM 或實體 Linux 主機。

</details>


進階行為見 [Link 執行期](link-runtime.md)，驗證方式見 [測試與驗收](testing.md)。
