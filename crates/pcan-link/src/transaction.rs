use core::fmt;
use core::num::NonZeroUsize;
use core::time::Duration;
use std::sync::Arc;

use pcan_core::{CanId, ConfigError, EXT_FLAG, Error, FrameKind, RxFrame};
use tokio::sync::{mpsc, oneshot};

/// 自訂述詞拒絕回應的原因。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum RejectReason {
    /// 應用協定回報否定狀態碼。
    Code(u8),
    /// 回應內容不符合應用協定。
    InvalidResponse,
}

/// 回應比對條件。
#[derive(Clone)]
#[non_exhaustive]
pub enum Matcher {
    /// 識別碼精確比對。
    Id(CanId),
    /// 識別碼遮罩比對。
    IdMask {
        /// 期望的原始識別碼位元。
        id: u32,
        /// 必須相同的位元遮罩。
        mask: u32,
    },
    /// 識別碼與資料前綴同時比對。
    IdAndPrefix {
        /// 期望識別碼。
        id: CanId,
        /// 資料前綴樣式。
        prefix: PrefixPattern,
    },
    /// 自訂同步述詞。
    ///
    /// 述詞直接在 RX 路由任務執行，必須極快、不得阻塞或 panic。`Arc`
    /// 僅在建立與複製交易規格時配置；逐幀比對不配置。
    Custom(Arc<dyn Fn(&RxFrame) -> MatchResult + Send + Sync>),
}

impl fmt::Debug for Matcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => formatter.debug_tuple("Id").field(id).finish(),
            Self::IdMask { id, mask } => formatter
                .debug_struct("IdMask")
                .field("id", id)
                .field("mask", mask)
                .finish(),
            Self::IdAndPrefix { id, prefix } => formatter
                .debug_struct("IdAndPrefix")
                .field("id", id)
                .field("prefix", prefix)
                .finish(),
            Self::Custom(_) => formatter.write_str("Custom(<fn>)"),
        }
    }
}

impl Matcher {
    pub(crate) fn evaluate(&self, frame: &RxFrame) -> MatchResult {
        match self {
            Self::Id(id) if frame.frame.id() == *id => MatchResult::AcceptAndFinish,
            Self::IdMask { id, mask }
                if ((frame.frame.id().to_bits() ^ *id) & (*mask & (EXT_FLAG | 0x1fff_ffff)))
                    == 0 =>
            {
                MatchResult::AcceptAndFinish
            }
            Self::IdAndPrefix { id, prefix }
                if frame.frame.id() == *id && prefix.matches(frame.frame.data()) =>
            {
                MatchResult::AcceptAndFinish
            }
            Self::Custom(predicate) => predicate(frame),
            _ => MatchResult::NoMatch,
        }
    }
}

/// 最長八位元組的固定大小資料前綴樣式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PrefixPattern {
    bytes: [u8; 8],
    mask: [u8; 8],
    len: u8,
}

impl PrefixPattern {
    /// 建立每個位元都需精確相同的前綴。
    ///
    /// # Errors
    ///
    /// 長度超過八位元組時回傳設定錯誤。
    pub fn new(bytes: &[u8]) -> Result<Self, ConfigError> {
        if bytes.len() > 8 {
            return Err(ConfigError::InvalidPayloadLen {
                len: bytes.len(),
                kind: FrameKind::Classic,
            });
        }
        Self::with_mask(bytes, &[0xff; 8][..bytes.len()])
    }

    /// 建立帶位元遮罩的前綴。
    ///
    /// # Errors
    ///
    /// 兩個切片長度不同或超過八位元組時回傳設定錯誤。
    pub fn with_mask(bytes: &[u8], mask: &[u8]) -> Result<Self, ConfigError> {
        if bytes.len() != mask.len() {
            return Err(ConfigError::InvalidFlags("前綴與遮罩長度必須相同"));
        }
        if bytes.len() > 8 {
            return Err(ConfigError::InvalidPayloadLen {
                len: bytes.len(),
                kind: FrameKind::Classic,
            });
        }
        let mut value = [0; 8];
        let mut value_mask = [0; 8];
        value[..bytes.len()].copy_from_slice(bytes);
        value_mask[..mask.len()].copy_from_slice(mask);
        Ok(Self {
            bytes: value,
            mask: value_mask,
            len: u8::try_from(bytes.len()).map_err(|_| ConfigError::InvalidPayloadLen {
                len: bytes.len(),
                kind: FrameKind::Classic,
            })?,
        })
    }

    /// 判斷資料是否符合前綴。
    #[must_use]
    pub fn matches(&self, data: &[u8]) -> bool {
        let len = usize::from(self.len);
        data.len() >= len
            && data[..len]
                .iter()
                .zip(self.bytes[..len].iter())
                .zip(self.mask[..len].iter())
                .all(|((&actual, &expected), &mask)| ((actual ^ expected) & mask) == 0)
    }
}

/// 自訂述詞的比對結果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum MatchResult {
    /// 不屬於此交易。
    NoMatch,
    /// 收下幀但交易尚未結束。
    Accept,
    /// 收下幀並允許完成交易。
    AcceptAndFinish,
    /// 明確拒絕並以錯誤結束。
    Reject(RejectReason),
}

/// 回應收集模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CollectMode {
    /// 第一個符合回應即完成。
    First,
    /// 收滿指定數量才完成。
    Exactly(NonZeroUsize),
    /// 在指定時間窗內收集所有符合回應。
    Window(Duration),
}

/// 請求與回應規格。
///
/// 建立規格與註冊交易時可配置記憶體；RX 比對熱路徑使用預先建立的 bounded
/// channel，不配置新容器。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ResponseSpec {
    /// 回應比對條件。
    pub matcher: Matcher,
    /// 每次等待的期限。
    pub timeout: Duration,
    /// 逾時後重送次數。
    pub retries: u8,
    /// 回應收集模式。
    pub mode: CollectMode,
}

impl ResponseSpec {
    /// 建立等待第一個回應且不重送的規格。
    #[must_use]
    pub const fn new(matcher: Matcher, timeout: Duration) -> Self {
        Self {
            matcher,
            timeout,
            retries: 0,
            mode: CollectMode::First,
        }
    }

    /// 設定逾時重送次數。
    #[must_use]
    pub const fn with_retries(mut self, retries: u8) -> Self {
        self.retries = retries;
        self
    }

    /// 設定收集模式。
    #[must_use]
    pub const fn with_mode(mut self, mode: CollectMode) -> Self {
        self.mode = mode;
        self
    }
}

/// 請求與回應交易錯誤。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransactionError {
    /// 等待回應超過期限。
    #[error("等待回應逾時（{}ms，已重送 {retries} 次）", .timeout.as_millis())]
    Timeout {
        /// 每次等待期限。
        timeout: Duration,
        /// 實際重送次數。
        retries: u8,
    },
    /// 等待期間連線中斷。
    #[error("等待回應期間連線中斷")]
    Disconnected,
    /// 請求送出失敗。
    #[error("送出請求失敗")]
    Send(#[source] Box<Error>),
    /// 自訂述詞拒絕回應。
    #[error("回應被述詞拒絕：{0:?}")]
    Rejected(RejectReason),
    /// 同時進行的交易數已達上限。
    #[error("同時進行的交易數已達上限（{limit}）")]
    TooManyInFlight {
        /// 設定的上限。
        limit: usize,
    },
    /// 連線已關閉。
    #[error("連線已關閉")]
    Closed,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum TransactionSignal {
    Frame(RxFrame),
    Rejected(RejectReason),
    Disconnected,
}

#[derive(Debug)]
pub(crate) enum TransactionCommand {
    Register {
        matcher: Matcher,
        capacity: usize,
        reply: oneshot::Sender<Result<(u64, mpsc::Receiver<TransactionSignal>), TransactionError>>,
    },
    Deregister(u64),
}

/// 已註冊的回應等待器。
///
/// 註冊已由路由任務確認，因此使用者在 `prepare()` 返回後才送出請求，可保證
/// 快速回應不會落在「先送出、後註冊」的競態窗口。取消 future 或丟棄此值
/// 會自動註銷，避免交易表永久成長。
#[derive(Debug)]
pub struct PendingResponse {
    pub(crate) id: u64,
    pub(crate) receiver: mpsc::Receiver<TransactionSignal>,
    pub(crate) control: mpsc::UnboundedSender<TransactionCommand>,
    pub(crate) spec: ResponseSpec,
    pub(crate) registered: bool,
}

impl PendingResponse {
    async fn recv_signal(&mut self) -> Result<TransactionSignal, TransactionError> {
        self.receiver
            .recv()
            .await
            .ok_or(TransactionError::Disconnected)
    }

    pub(crate) async fn wait_attempt(&mut self) -> Result<Vec<RxFrame>, TransactionError> {
        let mode = self.spec.mode;
        let timeout = self.spec.timeout;
        let limit = match mode {
            CollectMode::First => 1,
            CollectMode::Exactly(count) => count.get(),
            CollectMode::Window(_) => 64,
        };
        let mut frames = Vec::with_capacity(limit);
        let duration = match mode {
            CollectMode::Window(window) => window.min(timeout),
            _ => timeout,
        };
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            tokio::select! {
                signal = self.recv_signal() => {
                    match signal? {
                        TransactionSignal::Frame(frame) => {
                            frames.push(frame);
                            match mode {
                                CollectMode::First => return Ok(frames),
                                CollectMode::Exactly(count)
                                    if frames.len() >= count.get() =>
                                {
                                    return Ok(frames);
                                }
                                CollectMode::Window(_) | CollectMode::Exactly(_) => {}
                            }
                        }
                        TransactionSignal::Rejected(reason) => {
                            return Err(TransactionError::Rejected(reason));
                        }
                        TransactionSignal::Disconnected => {
                            return Err(TransactionError::Disconnected);
                        }
                    }
                },
                () = tokio::time::sleep_until(deadline) => {
                    if matches!(mode, CollectMode::Window(_)) && !frames.is_empty() {
                        return Ok(frames);
                    }
                    return Err(TransactionError::Timeout {
                        timeout,
                        retries: 0,
                    });
                }
            }
        }
    }

    pub(crate) fn clear_buffer(&mut self) {
        while self.receiver.try_recv().is_ok() {}
    }

    /// 等待單一回應。
    ///
    /// # Errors
    ///
    /// 逾時、斷線或述詞拒絕回應時回傳交易錯誤。
    pub async fn wait(mut self) -> Result<RxFrame, TransactionError> {
        let result = self.wait_attempt().await?;
        result
            .into_iter()
            .next()
            .ok_or(TransactionError::Disconnected)
    }

    /// 等待規格指定數量的回應。
    ///
    /// # Errors
    ///
    /// 逾時、斷線或述詞拒絕回應時回傳交易錯誤。
    pub async fn wait_many(mut self) -> Result<Vec<RxFrame>, TransactionError> {
        self.wait_attempt().await
    }
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        if self.registered {
            let _ignored = self.control.send(TransactionCommand::Deregister(self.id));
            self.registered = false;
        }
    }
}

#[derive(Debug)]
struct TransactionSlot {
    id: u64,
    matcher: Matcher,
    sender: mpsc::Sender<TransactionSignal>,
}

#[derive(Debug)]
pub(crate) struct TransactionTable {
    slots: Vec<TransactionSlot>,
    next_id: u64,
    limit: usize,
}

impl TransactionTable {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            slots: Vec::with_capacity(limit),
            next_id: 1,
            limit,
        }
    }

    pub(crate) fn handle(&mut self, command: TransactionCommand) {
        match command {
            TransactionCommand::Register {
                matcher,
                capacity,
                reply,
            } => {
                if self.slots.len() >= self.limit {
                    let _ignored =
                        reply.send(Err(TransactionError::TooManyInFlight { limit: self.limit }));
                    return;
                }
                let id = self.next_id;
                self.next_id = self.next_id.saturating_add(1);
                let (sender, receiver) = mpsc::channel(capacity.max(1));
                self.slots.push(TransactionSlot {
                    id,
                    matcher,
                    sender,
                });
                let _ignored = reply.send(Ok((id, receiver)));
            }
            TransactionCommand::Deregister(id) => {
                if let Some(index) = self.slots.iter().position(|slot| slot.id == id) {
                    self.slots.swap_remove(index);
                }
            }
        }
    }

    pub(crate) fn dispatch(&mut self, frame: RxFrame) {
        self.slots.retain(|slot| {
            let signal = match slot.matcher.evaluate(&frame) {
                MatchResult::NoMatch => return true,
                MatchResult::Accept | MatchResult::AcceptAndFinish => {
                    TransactionSignal::Frame(frame)
                }
                MatchResult::Reject(reason) => TransactionSignal::Rejected(reason),
            };
            slot.sender.try_send(signal).is_ok() && !slot.sender.is_closed()
        });
    }

    pub(crate) fn disconnect_all(&mut self) {
        for slot in self.slots.drain(..) {
            let _ignored = slot.sender.try_send(TransactionSignal::Disconnected);
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}
