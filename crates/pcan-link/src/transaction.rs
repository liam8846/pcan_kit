use core::fmt;
use core::num::NonZeroUsize;
use core::time::Duration;
use std::sync::Arc;

use pcan_core::{CanId, ConfigError, EXT_FLAG, Error, FrameKind, RxFrame};
use tokio::sync::{mpsc, oneshot};

use crate::control::ControlLease;

const DEFAULT_WINDOW_RESPONSE_LIMIT: NonZeroUsize = NonZeroUsize::new(64).expect("預設上限非零");

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
            Self::Id(id) if frame.frame.id() == *id => MatchResult::Accept,
            Self::IdMask { id, mask }
                if ((frame.frame.id().to_bits() ^ *id) & (*mask & (EXT_FLAG | 0x1fff_ffff)))
                    == 0 =>
            {
                MatchResult::Accept
            }
            Self::IdAndPrefix { id, prefix }
                if frame.frame.id() == *id && prefix.matches(frame.frame.data()) =>
            {
                MatchResult::Accept
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
///
/// 比對器只負責「這一幀屬不屬於本交易」，**不**決定交易何時結束；結束
/// 完全由 [`CollectMode`] 決定。這是刻意的責任分離：同一個比對器能配
/// `First`、`Exactly(n)` 或 `Window(d)` 使用，語意不變。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum MatchResult {
    /// 不屬於此交易。
    NoMatch,
    /// 收下這一幀；是否完成交易由 [`CollectMode`] 決定。
    Accept,
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
    /// 在指定時間窗內收集符合回應，受 [`ResponseSpec::max_responses`] 限制。
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
    /// 時間窗模式的結果上限，預設 64；收到超過上限的回應時回傳
    /// [`TransactionError::ResponseLimitExceeded`]，不回傳不完整的成功結果。
    /// `First` 與 `Exactly` 的上限由收集模式本身決定。
    pub max_responses: NonZeroUsize,
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
            max_responses: DEFAULT_WINDOW_RESPONSE_LIMIT,
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

    /// 設定時間窗模式最多保存的回應數。
    #[must_use]
    pub const fn with_max_responses(mut self, limit: NonZeroUsize) -> Self {
        self.max_responses = limit;
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
    /// 時間窗內的回應數超過結果上限。
    #[error("時間窗回應數超過上限（{limit}）")]
    ResponseLimitExceeded {
        /// 設定的結果上限。
        limit: usize,
    },
    /// 交易生命週期名額已用盡，包含待建立與待清理項目。
    #[error("交易控制名額已用盡（容量 {capacity}）")]
    ControlQueueFull {
        /// 交易類別的生命週期名額。
        capacity: usize,
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
        lease: ControlLease,
        matcher: Matcher,
        capacity: usize,
        spec: ResponseSpec,
        control: mpsc::UnboundedSender<TransactionCommand>,
        reply: oneshot::Sender<Result<PendingResponse, TransactionError>>,
    },
    Deregister {
        id: u64,
        _lease: ControlLease,
    },
}

/// 已註冊的回應等待器。
///
/// 註冊已由路由任務確認，因此使用者在 `prepare()` 返回後才送出請求，可保證
/// 快速回應不會落在「先送出、後註冊」的競態窗口。取消 future 或丟棄此值
/// 會自動註銷，避免交易表永久成長。
#[derive(Debug)]
pub struct PendingResponse {
    lease: ControlLease,
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
            CollectMode::Window(_) => self.spec.max_responses.get(),
        };
        let mut frames = Vec::with_capacity(limit);
        let duration = match mode {
            CollectMode::Window(window) => window.min(timeout),
            _ => timeout,
        };
        let deadline = tokio::time::Instant::now() + duration;
        loop {
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(deadline) => {
                    if matches!(mode, CollectMode::Window(_)) && !frames.is_empty() {
                        return Ok(frames);
                    }
                    return Err(TransactionError::Timeout {
                        timeout,
                        retries: 0,
                    });
                }
                signal = self.recv_signal() => {
                    match signal? {
                        TransactionSignal::Frame(frame) => {
                            if frames.len() == limit {
                                return Err(TransactionError::ResponseLimitExceeded { limit });
                            }
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
    /// 逾時、斷線、述詞拒絕回應或時間窗收集超過
    /// [`ResponseSpec::max_responses`] 時回傳交易錯誤。
    pub async fn wait_many(mut self) -> Result<Vec<RxFrame>, TransactionError> {
        self.wait_attempt().await
    }
}

impl Drop for PendingResponse {
    fn drop(&mut self) {
        if self.registered {
            let _ignored = self.control.send(TransactionCommand::Deregister {
                id: self.id,
                _lease: Arc::clone(&self.lease),
            });
            self.registered = false;
        }
    }
}

#[derive(Debug)]
struct TransactionSlot {
    id: u64,
    matcher: Matcher,
    sender: mpsc::Sender<TransactionSignal>,
    /// 因等待者緩衝已滿而被丟棄的幀數。
    dropped: u64,
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
                lease,
                matcher,
                capacity,
                spec,
                control,
                reply,
            } => {
                if reply.is_closed() {
                    return;
                }
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
                    dropped: 0,
                });
                // 讓確認訊息持有註銷守衛，涵蓋 ack 已送出但尚未取走的取消。
                if reply
                    .send(Ok(PendingResponse {
                        lease,
                        id,
                        receiver,
                        control,
                        spec,
                        registered: true,
                    }))
                    .is_err()
                {
                    self.slots.pop();
                }
            }
            TransactionCommand::Deregister { id, .. } => {
                if let Some(index) = self.slots.iter().position(|slot| slot.id == id) {
                    self.slots.swap_remove(index);
                }
            }
        }
    }

    pub(crate) fn dispatch(&mut self, frame: RxFrame, mut on_drop: impl FnMut(u64)) {
        self.slots.retain_mut(|slot| {
            let signal = match slot.matcher.evaluate(&frame) {
                MatchResult::NoMatch => return true,
                MatchResult::Accept => TransactionSignal::Frame(frame),
                MatchResult::Reject(reason) => TransactionSignal::Rejected(reason),
            };
            match slot.sender.try_send(signal) {
                Ok(()) => !slot.sender.is_closed(),
                // 等待者仍在，只是緩衝已滿。保留登記並丟棄這一幀，讓等待者
                // 取得已收集的幀，而不是被誤判為斷線。
                Err(mpsc::error::TrySendError::Full(_)) => {
                    slot.dropped = slot.dropped.saturating_add(1);
                    if slot.dropped == 1 {
                        on_drop(slot.id);
                    }
                    true
                }
                // 等待者已消失，移除登記。
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
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

#[cfg(test)]
mod tests {
    use super::{Matcher, ResponseSpec, TransactionCommand, TransactionTable};
    use crate::control::ControlBudget;
    use core::time::Duration;
    use pcan_core::CanId;
    use tokio::sync::{mpsc, oneshot};

    #[test]
    fn cancellation_before_and_after_ack_reclaims_transaction_capacity() {
        let mut table = TransactionTable::new(1);
        let budget = ControlBudget::new(1);
        let spec = ResponseSpec::new(
            Matcher::Id(CanId::standard(0x123).expect("有效 ID")),
            Duration::from_secs(1),
        );
        let (control, mut commands) = mpsc::unbounded_channel();
        let (reply, receiver) = oneshot::channel();
        drop(receiver);
        table.handle(TransactionCommand::Register {
            lease: budget.acquire().expect("名額"),
            matcher: spec.matcher.clone(),
            capacity: 1,
            spec: spec.clone(),
            control: control.clone(),
            reply,
        });
        assert_eq!(table.len(), 0);
        assert!(budget.acquire().is_ok(), "名額應完整回收");

        // 尚未被接收的 ack 必須持有註銷守衛，取消不能等到下一個匹配幀。
        for _ in 0..3 {
            let (reply, receiver) = oneshot::channel();
            table.handle(TransactionCommand::Register {
                lease: budget.acquire().expect("名額"),
                matcher: spec.matcher.clone(),
                capacity: 1,
                spec: spec.clone(),
                control: control.clone(),
                reply,
            });
            assert_eq!(table.len(), 1);
            assert!(budget.acquire().is_err(), "未領取 ack 仍占名額");
            drop(receiver);
            assert!(budget.acquire().is_err(), "待清理命令仍占名額");
            table.handle(commands.try_recv().expect("確認訊息應自動註銷"));
            assert_eq!(table.len(), 0);
            assert!(budget.acquire().is_ok(), "名額應完整回收");
        }
    }
}
