//! 以執行期載入 PCAN-Basic 實作的跨平台非同步 CAN 後端。
//!
//! 正常 RX 路徑完全由驅動事件喚醒：Linux 使用 `AsyncFd`，Windows 使用
//! 等待 Win32 Event 的專用執行緒。只有舊版 Linux 驅動不提供接收 fd 時，
//! 才會明確記錄警告並降級到專用輪詢執行緒。

mod channel;
/// PCAN 通道與位元率設定。
pub mod config;
/// PCAN C 訊息與核心幀之間的純函式轉換。
pub mod convert;
mod rx;

pub use channel::{PcanChannel, PcanFactory};
pub use config::{PcanChannelId, PcanConfig, RxThreadPolicy};
