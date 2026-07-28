use crate::filter::FilterSet;

/// 後端無關的 CAN 位元率設定。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Bitrate {
    /// 古典 CAN 的單一名目位元率。
    Classic {
        /// 名目位元率，單位為 bit/s。
        nominal: u32,
    },
    /// CAN FD 的仲裁段與資料段位元率。
    Fd {
        /// 仲裁段名目位元率，單位為 bit/s。
        nominal: u32,
        /// 資料段位元率，單位為 bit/s。
        data: u32,
    },
}

impl Bitrate {
    /// 常用的古典 CAN 500 kbit/s 設定。
    pub const CLASSIC_500K: Self = Self::Classic { nominal: 500_000 };
    /// 常用的古典 CAN 250 kbit/s 設定。
    pub const CLASSIC_250K: Self = Self::Classic { nominal: 250_000 };
    /// 常用的古典 CAN 1 Mbit/s 設定。
    pub const CLASSIC_1M: Self = Self::Classic { nominal: 1_000_000 };
    /// 常用的 CAN FD 500 kbit/s 仲裁、2 Mbit/s 資料設定。
    pub const FD_500K_2M: Self = Self::Fd {
        nominal: 500_000,
        data: 2_000_000,
    };

    /// 取得仲裁段的名目位元率。
    #[must_use]
    pub const fn nominal(self) -> u32 {
        match self {
            Self::Classic { nominal } | Self::Fd { nominal, .. } => nominal,
        }
    }

    /// 取得 CAN FD 資料段位元率；古典設定回傳 `None`。
    #[must_use]
    pub const fn data(self) -> Option<u32> {
        match self {
            Self::Classic { .. } => None,
            Self::Fd { data, .. } => Some(data),
        }
    }

    /// 判斷是否為 CAN FD 位元率設定。
    #[must_use]
    pub const fn is_fd(self) -> bool {
        matches!(self, Self::Fd { .. })
    }
}

/// 後端無關的完整通道設定。
///
/// 此型別刻意可 [`Clone`] 且保存所有通道選項，因為重連時監督層必須完整重放
/// 設定。只恢復部分選項會讓重連後的通道行為悄悄改變，是通訊函式庫常見且
/// 難以診斷的產品缺陷。
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct TransportConfig {
    /// 通道使用的 CAN 位元率。
    pub bitrate: Bitrate,
    /// 是否啟用唯聽模式；此模式不送出任何位元，包含 ACK。
    pub listen_only: bool,
    /// 是否接收後端提供的錯誤幀。
    pub receive_error_frames: bool,
    /// 是否接收後端提供的狀態幀。
    pub receive_status_frames: bool,
    /// 是否接收自己送出的回音幀。
    pub receive_own_frames: bool,
    /// Bus-Off 後是否要求驅動自動復歸。
    pub bus_off_auto_reset: bool,
    /// 套用於硬體或核心層的識別碼過濾器。
    pub filter: FilterSet,
    /// 後端內部接收佇列容量。
    pub rx_queue_capacity: usize,
}

impl TransportConfig {
    /// 設定位元率。
    #[must_use]
    pub fn with_bitrate(mut self, bitrate: Bitrate) -> Self {
        self.bitrate = bitrate;
        self
    }

    /// 設定唯聽模式。
    #[must_use]
    pub fn with_listen_only(mut self, enabled: bool) -> Self {
        self.listen_only = enabled;
        self
    }

    /// 設定是否接收錯誤幀。
    #[must_use]
    pub fn with_receive_error_frames(mut self, enabled: bool) -> Self {
        self.receive_error_frames = enabled;
        self
    }

    /// 設定是否接收狀態幀。
    #[must_use]
    pub fn with_receive_status_frames(mut self, enabled: bool) -> Self {
        self.receive_status_frames = enabled;
        self
    }

    /// 設定是否接收本地送出幀的回音。
    #[must_use]
    pub fn with_receive_own_frames(mut self, enabled: bool) -> Self {
        self.receive_own_frames = enabled;
        self
    }

    /// 設定 Bus-Off 後是否由驅動自動復歸。
    #[must_use]
    pub fn with_bus_off_auto_reset(mut self, enabled: bool) -> Self {
        self.bus_off_auto_reset = enabled;
        self
    }

    /// 設定硬體或核心層識別碼過濾器。
    #[must_use]
    pub fn with_filter(mut self, filter: FilterSet) -> Self {
        self.filter = filter;
        self
    }

    /// 設定後端內部接收佇列容量。
    #[must_use]
    pub fn with_rx_queue_capacity(mut self, capacity: usize) -> Self {
        self.rx_queue_capacity = capacity;
        self
    }
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            bitrate: Bitrate::CLASSIC_500K,
            listen_only: false,
            receive_error_frames: true,
            receive_status_frames: false,
            receive_own_frames: false,
            bus_off_auto_reset: true,
            filter: FilterSet::accept_all(),
            rx_queue_capacity: 1024,
        }
    }
}
