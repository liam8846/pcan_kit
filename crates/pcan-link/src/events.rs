use core::time::Duration;

use pcan_core::{BusStatus, BusWarnings};

use crate::router::SubscriptionId;

/// 造成連線中斷的原因，供事件與日誌使用。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum FaultCause {
    /// 開啟傳輸層失敗。
    OpenFailed,
    /// 接收路徑失敗。
    ReadFailed,
    /// 傳送路徑失敗。
    WriteFailed,
    /// 匯流排進入 Bus-Off。
    BusOff,
    /// 健康檢查逾時。
    HealthCheckTimeout,
    /// 使用者要求關閉。
    UserRequested,
}

/// 連線層對外廣播的事件。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum BusEvent {
    /// 背景工作任務異常結束（panic 或被強制丟棄）。
    WorkerLost {
        /// 結束的工作任務名稱，供日誌定位。
        worker: &'static str,
    },
    /// 開始開啟傳輸層。
    Connecting,
    /// 已成功連線。
    Connected {
        /// 本輪成功前的重試次數。
        attempt: u32,
    },
    /// 將在退避後重連。
    Reconnecting {
        /// 即將進行的重試次數。
        attempt: u32,
        /// 實際退避延遲。
        delay: Duration,
        /// 造成重連的原因。
        cause: FaultCause,
    },
    /// 匯流排進入 Bus-Off。
    BusOff,
    /// 匯流排狀態改變。
    BusStateChanged(BusStatus),
    /// 收到非致命警告。
    Warning(BusWarnings),
    /// 訂閱因消費過慢丟棄幀。
    RxDropped {
        /// 發生丟棄的訂閱。
        subscription: SubscriptionId,
        /// 此訂閱的累計丟棄數。
        count: u64,
    },
    /// 傳送佇列丟棄待送幀。
    TxDropped {
        /// 本次丟棄數。
        count: u64,
    },
    /// 傳送佇列越過高水位，應用層應主動降速。
    TxQueueHighWater {
        /// 目前 channel 段排隊幀數。
        queued: u32,
        /// 單段容量。
        capacity: u32,
    },
    /// 傳送佇列已回落至低水位。
    TxQueueRecovered {
        /// 目前 channel 段排隊幀數。
        queued: u32,
        /// 單段容量。
        capacity: u32,
    },
    /// 交易逾時後重送。
    TransactionRetried {
        /// 本次重送次數。
        attempt: u8,
    },
    /// 連線已由使用者關閉。
    Closed,
    /// 永久失敗，不再重連。
    Failed,
}

const _: () = assert!(core::mem::size_of::<BusEvent>() <= 40);
