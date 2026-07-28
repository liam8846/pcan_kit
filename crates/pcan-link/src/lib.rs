//! `pcan_kit` 的自動重連、路由、排程與交易監督層。

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
pub use txqueue::{PendingTxPolicy, TxGate};

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
