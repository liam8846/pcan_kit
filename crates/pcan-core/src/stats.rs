use core::sync::atomic::{AtomicU64, Ordering};

/// 可安全跨執行緒讀取的累計通訊統計計數器。
#[derive(Debug, Default)]
pub struct Stats {
    /// 已接收的資料幀數。
    pub rx_frames: AtomicU64,
    /// 已成功傳送的資料幀數。
    pub tx_frames: AtomicU64,
    /// 已接收的錯誤幀數。
    pub rx_error_frames: AtomicU64,
    /// 控制器接收溢位次數。
    pub rx_hw_overrun: AtomicU64,
    /// 驅動或函式庫接收佇列溢位次數。
    pub rx_queue_overrun: AtomicU64,
    /// 傳送佇列滿事件次數。
    pub tx_queue_full: AtomicU64,
    /// 因政策或佇列壓力丟棄的傳送幀數。
    pub tx_dropped: AtomicU64,
    /// 成功重連次數。
    pub reconnects: AtomicU64,
    /// Bus-Off 事件次數。
    pub bus_off_events: AtomicU64,
}

/// [`Stats`] 的不可變快照，方便回報與比較。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct StatsSnapshot {
    /// 已接收的資料幀數。
    pub rx_frames: u64,
    /// 已成功傳送的資料幀數。
    pub tx_frames: u64,
    /// 已接收的錯誤幀數。
    pub rx_error_frames: u64,
    /// 控制器接收溢位次數。
    pub rx_hw_overrun: u64,
    /// 驅動或函式庫接收佇列溢位次數。
    pub rx_queue_overrun: u64,
    /// 傳送佇列滿事件次數。
    pub tx_queue_full: u64,
    /// 因政策或佇列壓力丟棄的傳送幀數。
    pub tx_dropped: u64,
    /// 成功重連次數。
    pub reconnects: u64,
    /// Bus-Off 事件次數。
    pub bus_off_events: u64,
}

impl Stats {
    /// 以 Relaxed 讀取建立一致格式的近即時統計快照。
    ///
    /// 各欄位可能來自略微不同的時間點；統計用途不需要跨欄位同步保證。
    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            rx_frames: self.rx_frames.load(Ordering::Relaxed),
            tx_frames: self.tx_frames.load(Ordering::Relaxed),
            rx_error_frames: self.rx_error_frames.load(Ordering::Relaxed),
            rx_hw_overrun: self.rx_hw_overrun.load(Ordering::Relaxed),
            rx_queue_overrun: self.rx_queue_overrun.load(Ordering::Relaxed),
            tx_queue_full: self.tx_queue_full.load(Ordering::Relaxed),
            tx_dropped: self.tx_dropped.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            bus_off_events: self.bus_off_events.load(Ordering::Relaxed),
        }
    }

    /// 將已接收資料幀數遞增一。
    pub fn inc_rx_frames(&self) {
        self.rx_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// 將成功傳送資料幀數遞增一。
    pub fn inc_tx_frames(&self) {
        self.tx_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// 將接收錯誤幀數遞增一。
    pub fn inc_rx_error_frames(&self) {
        self.rx_error_frames.fetch_add(1, Ordering::Relaxed);
    }

    /// 將控制器接收溢位次數遞增一。
    pub fn inc_rx_hw_overrun(&self) {
        self.rx_hw_overrun.fetch_add(1, Ordering::Relaxed);
    }

    /// 將接收佇列溢位次數遞增一。
    pub fn inc_rx_queue_overrun(&self) {
        self.rx_queue_overrun.fetch_add(1, Ordering::Relaxed);
    }

    /// 將傳送佇列滿事件次數遞增一。
    pub fn inc_tx_queue_full(&self) {
        self.tx_queue_full.fetch_add(1, Ordering::Relaxed);
    }

    /// 將傳送丟棄幀數遞增一。
    pub fn inc_tx_dropped(&self) {
        self.tx_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// 將成功重連次數遞增一。
    pub fn inc_reconnects(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    /// 將 Bus-Off 事件次數遞增一。
    pub fn inc_bus_off_events(&self) {
        self.bus_off_events.fetch_add(1, Ordering::Relaxed);
    }
}
