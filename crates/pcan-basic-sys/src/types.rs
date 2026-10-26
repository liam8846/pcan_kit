//! 與 PEAK 標頭固定寬度型別一一對應的純量別名。

/// PCAN 通道控制代碼，對應 C `WORD`。
pub type TPCANHandle = u16;
/// PCAN 狀態位元欄位，對應 C `DWORD`。
pub type TPCANStatus = u32;
/// PCAN 裝置種類，對應 C `BYTE`。
pub type TPCANDevice = u8;
/// PCAN 參數識別碼，對應 C `BYTE`。
pub type TPCANParameter = u8;
/// PCAN 訊息型別位元欄位，對應 C `BYTE`。
pub type TPCANMessageType = u8;
/// 非 `PnP` 硬體種類，對應 C `BYTE`。
pub type TPCANType = u8;
/// PCAN 過濾模式，對應 C `BYTE`。
pub type TPCANMode = u8;
/// 古典 CAN BTR0BTR1 位元率，對應 C `WORD`。
pub type TPCANBaudrate = u16;
/// CAN FD 位元率設定字串，對應 C `LPSTR`。
pub type TPCANBitrateFD = *mut core::ffi::c_char;
/// CAN FD 微秒時間戳，對應 C `UINT64`。
pub type TPCANTimestampFD = u64;
