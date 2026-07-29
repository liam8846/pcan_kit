//! 以執行期載入 PCAN-Basic 實作的跨平台非同步 CAN 後端。
//!
//! 正常 RX 路徑完全由驅動事件喚醒：Linux 使用 `AsyncFd`，Windows 使用
//! 等待 Win32 Event 的專用執行緒。只有舊版 Linux 驅動不提供接收 fd 時，
//! 才會明確記錄警告並降級到專用輪詢執行緒。
//!
//! PCAN 開啟包含阻塞 FFI 與接收執行緒建立，因此會在 Tokio 阻塞執行緒池
//! 執行。監督層的開啟期限逾時後，已開始的阻塞工作仍會完成並自行清理；
//! 同一工廠的下一次開啟會等待前一次工作及其通道清理結束，避免互相解除
//! 初始化。

mod channel;
/// PCAN 通道與位元率設定。
pub mod config;
/// PCAN C 訊息與核心幀之間的純函式轉換。
pub mod convert;
/// 已連接 PCAN 通道的列舉介面與資訊型別。
pub mod enumerate;
mod rx;

pub use channel::{PcanChannel, PcanFactory};
pub use config::{PcanChannelId, PcanConfig, RxThreadPolicy};
pub use enumerate::{
    PcanChannelCondition, PcanChannelFeatures, PcanChannelInfo, PcanDeviceKind, list_channels,
    list_channels_from,
};
