//! `pcan-kit` 是跨平台、全非同步且具自動重連能力的 CAN 通訊門面。
//!
//! 一般應用只需從本 crate 匯入核心幀、連線監督、訂閱、週期傳送與交易 API。
//! 後端可在執行期由 URI 選擇，且使用列舉靜態分派維持 RX/TX 熱路徑零配置。
//!
//! # 無硬體可執行範例
//!
//! ```
//! # #[cfg(feature = "test-util")]
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use pcan_kit::{CanId, Frame, Link, TransportEvent};
//! use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
//!
//! let id = CanId::standard(0x123)?;
//! let frame = Frame::new(id, &[1, 2, 3])?;
//! let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
//! let link = Link::builder(factory).connect().await?;
//! link.send_await(frame).await?;
//! assert_eq!(handle.sent(), vec![frame]);
//! let _event_type: Option<TransportEvent> = None;
//! link.close().await;
//! # Ok(())
//! # }
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! #   #[cfg(feature = "test-util")]
//! #   run().await?;
//! #   Ok(())
//! # }
//! ```

/// 執行期後端列舉。
pub mod any;
/// URI 解析與連線便捷函式。
pub mod open;

pub use any::{AnyFactory, AnyTransport};
pub use open::{open, parse_uri};
pub use pcan_core::{
    BackendError, Bitrate, BusState, BusStatus, BusWarnings, CanId, Capabilities, ConfigError,
    Error, ErrorCounters, FaultKind, FilterRule, FilterSet, Frame, FrameFlags, FrameKind, IdKind,
    LoadError, Result, RxFrame, Stats, StatsSnapshot, Timestamp, TimestampSource, Transport,
    TransportConfig, TransportEvent, TransportFactory,
};
pub use pcan_link::{
    BackoffPolicy, BusEvent, CollectMode, CyclicConfig, CyclicHandle, CyclicId, CyclicStats,
    FaultCause, Jitter, Link, LinkAction, LinkBuilder, LinkInput, LinkMachine, LinkState,
    MatchResult, Matcher, NoJitter, OverflowPolicy, OverrunPolicy, PendingResponse,
    PendingTxPolicy, PrefixPattern, RejectReason, Repeat, ResponseSpec, SplitMixJitter,
    SubscribeConfig, Subscription, SubscriptionId, TransactionError, TxGate,
};

#[cfg(feature = "basic")]
pub use pcan_basic::{PcanChannel, PcanChannelId, PcanConfig, PcanFactory, RxThreadPolicy};
#[cfg(all(feature = "socketcan", target_os = "linux"))]
pub use pcan_socketcan::{CanSocket, SocketCanConfig, SocketCanFactory};
