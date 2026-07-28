//! `pcan_kit` 的核心值型別、錯誤分類與傳輸抽象。
//!
//! 本 crate 需要 Rust 標準函式庫（`std`），不支援 `no_std` 環境。
//!
//! [`Frame`] 以固定 72 位元組的 `Copy` 值表示 CAN 幀，讓接收、傳送與分派
//! 熱路徑不需要堆積配置。後端只需實作 [`Transport`]；重連、排程與交易等上層
//! 邏輯便能共用同一個抽象，也能由 `test-util` 功能提供的測試替身驅動。

/// 後端無關的通道設定。
pub mod config;
/// 公開錯誤型別與故障分類。
pub mod error;
/// CAN 識別碼過濾規則。
pub mod filter;
/// CAN 幀、時間戳與 DLC 轉換。
pub mod frame;
/// CAN 識別碼值型別。
pub mod id;
/// 跨執行緒通訊統計。
pub mod stats;
/// 匯流排健康狀態。
pub mod status;
/// 無硬體測試用的傳輸替身。
#[cfg(feature = "test-util")]
pub mod testing;
/// 傳輸後端與工廠抽象。
pub mod transport;

pub use config::{Bitrate, TransportConfig};
pub use error::{
    BackendError, ConfigError, Error, FaultKind, FrameKind, IdKind, LoadError, Result,
};
pub use filter::{FilterRule, FilterSet};
pub use frame::{
    Frame, FrameFlags, RxFrame, Timestamp, TimestampSource, dlc_to_len, len_to_dlc, round_up_fd_len,
};
pub use id::{CanId, EXT_FLAG};
pub use stats::{Stats, StatsSnapshot};
pub use status::{BusState, BusStatus, BusWarnings, ErrorCounters};
pub use transport::{Capabilities, Transport, TransportEvent, TransportFactory};
