use core::time::Duration;

/// 本 crate 公開 API 使用的結果型別。
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// 故障類別，決定監督層的處置方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum FaultKind {
    /// 可原地重試，例如傳送佇列暫滿。
    Transient,
    /// 匯流排警告；連線仍可使用，只需觀測與統計。
    Recoverable,
    /// 連線已不可用，需交由重連狀態機重新開啟。
    Fatal,
    /// 設定或呼叫錯誤，重連不會改善。
    Permanent,
}

/// CAN 識別碼種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdKind {
    /// 11-bit 標準識別碼。
    Standard,
    /// 29-bit 擴充識別碼。
    Extended,
}

/// CAN 幀格式種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FrameKind {
    /// CAN 2.0 古典資料幀。
    Classic,
    /// CAN FD 資料幀。
    Fd,
    /// CAN 2.0 遠端請求幀。
    Remote,
}

/// `pcan_kit` 的頂層操作錯誤。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 載入後端動態函式庫失敗。
    #[error("載入後端動態函式庫失敗")]
    Load(#[from] LoadError),

    /// 使用者提供的設定無效。
    #[error("設定無效")]
    Config(#[from] ConfigError),

    /// 開啟指定通道失敗。
    #[error("開啟通道 `{channel}` 失敗")]
    Open {
        /// 嘗試開啟的通道名稱。
        channel: Box<str>,
        /// 後端回報的原始錯誤。
        #[source]
        source: BackendError,
    },

    /// 後端輸入或輸出操作失敗。
    #[error("後端 I/O 失敗")]
    Io(#[from] BackendError),

    /// 匯流排進入 Bus-Off 狀態。
    #[error("匯流排進入 Bus-Off 狀態")]
    BusOff,

    /// 連線已由呼叫端關閉。
    #[error("連線已關閉")]
    Closed,

    /// 連線中斷，監督層正在重連。
    #[error("連線中斷，重連中（第 {attempt} 次嘗試）")]
    Disconnected {
        /// 目前的重連嘗試次數。
        attempt: u32,
    },

    /// 傳送佇列已滿且後端退避重試仍未成功。
    #[error("傳送佇列已滿（容量 {capacity}）")]
    TxQueueFull {
        /// 傳送佇列容量。
        capacity: usize,
    },

    /// 操作未在期限內完成。
    #[error("操作逾時（{}ms）", .timeout.as_millis())]
    Timeout {
        /// 本次操作採用的期限。
        timeout: Duration,
    },

    /// 所選後端不支援要求的能力。
    #[error("此後端不支援：{0}")]
    Unsupported(&'static str),
}

impl Error {
    /// 回傳監督層應採取處置的故障類別。
    #[must_use]
    pub fn fault_kind(&self) -> FaultKind {
        match self {
            Self::Load(_) | Self::Config(_) | Self::Closed | Self::Unsupported(_) => {
                FaultKind::Permanent
            }
            Self::Open { source, .. } | Self::Io(source) => source.fault_kind(),
            Self::BusOff | Self::Disconnected { .. } => FaultKind::Fatal,
            Self::TxQueueFull { .. } | Self::Timeout { .. } => FaultKind::Transient,
        }
    }

    /// 判斷錯誤是否代表連線已不可用、需要重建。
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        self.fault_kind() == FaultKind::Fatal
    }
}

/// 載入執行期後端函式庫時的錯誤。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// 找不到任何候選函式庫。
    #[error("找不到後端函式庫（已嘗試：{tried:?}）；請安裝驅動或設定 PCAN_BASIC_LIB 環境變數")]
    NotFound {
        /// 依序嘗試過的函式庫路徑。
        tried: Vec<Box<str>>,
        /// 作業系統提供的最後一個載入錯誤。
        #[source]
        source: Option<std::io::Error>,
    },

    /// 已載入的函式庫缺少必要符號。
    #[error("函式庫缺少必要符號 `{symbol}`，版本可能過舊")]
    MissingSymbol {
        /// 找不到的必要符號名稱。
        symbol: &'static str,
    },
}

/// 使用者提供的設定或幀內容無效。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// CAN 識別碼超出格式允許的範圍。
    #[error("CAN 識別碼 {value:#x} 超出 {kind:?} 範圍")]
    IdOutOfRange {
        /// 無效的原始識別碼。
        value: u32,
        /// 預期的識別碼種類。
        kind: IdKind,
    },

    /// 酬載長度不符合幀格式。
    #[error("酬載長度 {len} 對 {kind:?} 幀不合法")]
    InvalidPayloadLen {
        /// 使用者提供的位元組長度。
        len: usize,
        /// 預期的幀格式。
        kind: FrameKind,
    },

    /// 位元率參數無法由後端接受。
    #[error("位元率設定無效：{0}")]
    InvalidBitrate(Box<str>),

    /// 通道位址或名稱無法解析。
    #[error("無法解析通道位址 `{0}`")]
    InvalidChannel(Box<str>),

    /// 幀或設定旗標彼此衝突。
    #[error("旗標組合無效：{0}")]
    InvalidFlags(&'static str),
}

/// 後端具體錯誤，保留原始碼與診斷資訊。
///
/// 診斷文字使用不可變的 `Box<str>`，且只在錯誤路徑配置，不影響正常熱路徑。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BackendError {
    /// PCAN-Basic API 回報錯誤狀態碼。
    #[error("PCAN-Basic 錯誤 {code:#010x}（{text}）於 {op}")]
    PcanBasic {
        /// PCAN-Basic 原始狀態碼。
        code: u32,
        /// 驅動提供的人類可讀診斷文字。
        text: Box<str>,
        /// 發生錯誤的靜態操作名稱。
        op: &'static str,
        /// 已正規化的故障類別。
        kind: FaultKind,
    },

    /// `SocketCAN` 系統呼叫失敗。
    #[error("SocketCAN 錯誤於 {op}")]
    SocketCan {
        /// 發生錯誤的靜態操作名稱。
        op: &'static str,
        /// 已正規化的故障類別。
        kind: FaultKind,
        /// 作業系統提供的原始錯誤。
        #[source]
        source: std::io::Error,
    },
}

impl BackendError {
    /// 回傳後端錯誤已正規化的故障類別。
    #[must_use]
    pub const fn fault_kind(&self) -> FaultKind {
        match self {
            Self::PcanBasic { kind, .. } | Self::SocketCan { kind, .. } => *kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::{BackendError, ConfigError, Error, FaultKind, IdKind, LoadError};

    fn backend(kind: FaultKind) -> BackendError {
        BackendError::PcanBasic {
            code: 1,
            text: "測試".into(),
            op: "test",
            kind,
        }
    }

    #[test]
    fn maps_every_top_level_variant() {
        let cases = [
            (
                Error::Load(LoadError::MissingSymbol { symbol: "x" }),
                FaultKind::Permanent,
            ),
            (
                Error::Config(ConfigError::IdOutOfRange {
                    value: 0x800,
                    kind: IdKind::Standard,
                }),
                FaultKind::Permanent,
            ),
            (
                Error::Open {
                    channel: "can0".into(),
                    source: backend(FaultKind::Recoverable),
                },
                FaultKind::Recoverable,
            ),
            (
                Error::Io(backend(FaultKind::Transient)),
                FaultKind::Transient,
            ),
            (Error::BusOff, FaultKind::Fatal),
            (Error::Closed, FaultKind::Permanent),
            (Error::Disconnected { attempt: 2 }, FaultKind::Fatal),
            (Error::TxQueueFull { capacity: 16 }, FaultKind::Transient),
            (
                Error::Timeout {
                    timeout: Duration::from_millis(1),
                },
                FaultKind::Transient,
            ),
            (Error::Unsupported("測試"), FaultKind::Permanent),
        ];

        for (error, expected) in cases {
            assert_eq!(error.fault_kind(), expected);
            assert_eq!(error.is_fatal(), expected == FaultKind::Fatal);
        }
    }
}
