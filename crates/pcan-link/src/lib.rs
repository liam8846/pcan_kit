//! `pcan_kit` 的自動重連、路由、排程與交易監督層。
//!
//! 每條連線由 supervisor、傳送工作者與週期排程器共同維護。背景工作者以
//! RAII 守衛收斂異常結束；在 panic 採 unwind 或 future 被執行期丟棄時，
//! 連線狀態會推到 `Closed`，訂閱與等待中的操作也會被喚醒。若程式使用
//! `panic = "abort"`，程序會直接終止，Rust 無法執行任何 `Drop` 守衛。
//!
//! 傳送背壓由 bounded channel 與工作者暫存佇列組成兩段，可透過
//! [`Link::tx_queue_depth`] 觀測；[`LinkBuilder::tx_high_water_ratio`] 可設定
//! 尚未塞滿前的主動降速事件。

/// 連線建構器。
pub mod builder;
/// 週期傳送排程。
pub mod cyclic;
/// 對外事件。
pub mod events;
/// 邏輯連線門面。
pub mod link;
/// 推送式接收路由。
pub mod router;
/// 連線監督器。
pub mod supervisor;
/// 請求與回應交易。
pub mod transaction;
/// 傳送佇列、背壓與待送政策。
pub mod txqueue;

pub use builder::LinkBuilder;
pub use cyclic::{CyclicConfig, CyclicHandle, CyclicId, CyclicStats, OverrunPolicy, Repeat};
pub use events::{BusEvent, FaultCause};
pub use link::Link;
pub use pcan_core::{Capabilities, Error, StatsSnapshot};
pub use router::{OverflowPolicy, SubscribeConfig, Subscription, SubscriptionId};
pub use supervisor::backoff::{BackoffPolicy, Jitter, NoJitter, SplitMixJitter};
pub use supervisor::machine::{ActionSet, LinkAction, LinkInput, LinkMachine, LinkState};
pub use transaction::{
    CollectMode, MatchResult, Matcher, PendingResponse, PrefixPattern, RejectReason, ResponseSpec,
    TransactionError,
};
pub use txqueue::{PendingTxPolicy, TxGate, TxQueueDepth};

#[cfg(feature = "tracing")]
macro_rules! trace_debug {
    ($($arg:tt)*) => {
        tracing::debug!($($arg)*)
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! trace_debug {
    ($($arg:tt)*) => {{}};
}

pub(crate) use trace_debug;

#[cfg(feature = "tracing")]
macro_rules! trace_warn {
    ($($arg:tt)*) => {
        tracing::warn!($($arg)*)
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! trace_warn {
    ($($arg:tt)*) => {{}};
}

pub(crate) use trace_warn;

#[cfg(feature = "tracing")]
macro_rules! trace_error {
    ($($arg:tt)*) => {
        tracing::error!($($arg)*)
    };
}

#[cfg(not(feature = "tracing"))]
macro_rules! trace_error {
    ($($arg:tt)*) => {{}};
}

pub(crate) use trace_error;
