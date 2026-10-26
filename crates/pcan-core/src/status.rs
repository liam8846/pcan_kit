bitflags::bitflags! {
    /// 匯流排健康警告旗標。
    ///
    /// 這些狀況本身不一定致命，主要供觀測、統計與事件廣播。
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub struct BusWarnings: u16 {
        /// 錯誤計數上升，對應 PCAN `BUSLIGHT`。
        const BUS_LIGHT = 1 << 0;
        /// 進入錯誤警告區，對應 PCAN `BUSHEAVY` 或 `BUSWARNING`。
        const BUS_HEAVY = 1 << 1;
        /// 控制器進入 error-passive。
        const BUS_PASSIVE = 1 << 2;
        /// 控制器接收溢位，硬體端已丟幀。
        const RX_OVERRUN = 1 << 3;
        /// 驅動接收佇列溢位，軟體端已丟幀。
        const QUEUE_OVERRUN = 1 << 4;
        /// 傳送佇列已滿，屬於背壓訊號。
        const TX_QUEUE_FULL = 1 << 5;
        /// 其他需要注意的狀況，對應 PCAN `CAUTION`。
        const CAUTION = 1 << 6;
        /// 傳送逾時，對應 SocketCAN `CAN_ERR_TX_TIMEOUT`。
        const TX_TIMEOUT = 1 << 7;
        /// 仲裁失敗，對應 SocketCAN `CAN_ERR_LOSTARB`。
        const ARBITRATION_LOST = 1 << 8;
    }
}

/// CAN 匯流排的控制器狀態。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum BusState {
    /// 控制器正常參與通訊。
    #[default]
    Active,
    /// 錯誤計數已進入警告區。
    Warning,
    /// 控制器處於 error-passive。
    ErrorPassive,
    /// 控制器已因錯誤過多離開匯流排。
    BusOff,
    /// 控制器或通道已停止。
    Stopped,
}

/// 後端提供的 CAN 錯誤計數器。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ErrorCounters {
    /// 傳送錯誤計數。
    pub tx: u8,
    /// 接收錯誤計數。
    pub rx: u8,
}

/// 匯流排狀態快照。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct BusStatus {
    /// 控制器目前狀態。
    pub state: BusState,
    /// 當下所有非致命警告。
    pub warnings: BusWarnings,
    /// 後端可提供時的傳送與接收錯誤計數。
    pub error_counters: Option<ErrorCounters>,
}

impl BusStatus {
    /// 建立匯流排狀態快照。
    #[must_use]
    pub const fn new(
        state: BusState,
        warnings: BusWarnings,
        error_counters: Option<ErrorCounters>,
    ) -> Self {
        Self {
            state,
            warnings,
            error_counters,
        }
    }

    /// 判斷匯流排是否完全正常且沒有警告。
    #[must_use]
    pub const fn is_healthy(&self) -> bool {
        matches!(self.state, BusState::Active) && self.warnings.is_empty()
    }
}
