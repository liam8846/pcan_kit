use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use pcan_core::{
    BusStatus, Capabilities, Error, FilterSet, Frame, RxFrame, Stats, StatsSnapshot,
    TransportFactory,
};
use tokio::sync::{broadcast, oneshot, watch};

use crate::LinkState;
use crate::builder::LinkBuilder;
use crate::cyclic::{CyclicCommand, CyclicConfig, CyclicHandle, CyclicId, new_shared};
use crate::events::BusEvent;
use crate::router::{RouterCommand, SubscribeConfig, Subscription};
use crate::supervisor::{RuntimeChannels, SupervisorCommand};
use crate::transaction::{
    CollectMode, PendingResponse, ResponseSpec, TransactionCommand, TransactionError,
};
use crate::txqueue::{PendingTxPolicy, TxItem};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    match value.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct LinkInner {
    pub(crate) channels: RuntimeChannels,
    pub(crate) state: watch::Sender<LinkState>,
    pub(crate) events: broadcast::Sender<BusEvent>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) bus_status: Arc<Mutex<BusStatus>>,
    pub(crate) capabilities: Arc<Mutex<Option<Capabilities>>>,
    pub(crate) in_flight: Arc<AtomicUsize>,
    pub(crate) raw: Arc<OnceLock<broadcast::Sender<RxFrame>>>,
    pub(crate) pending_policy: PendingTxPolicy,
    pub(crate) tx_capacity: usize,
    pub(crate) cyclic_next: AtomicU64,
}

impl core::fmt::Debug for LinkInner {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LinkInner")
            .field("state", &*self.state.borrow())
            .field("pending_policy", &self.pending_policy)
            .field("tx_capacity", &self.tx_capacity)
            .finish_non_exhaustive()
    }
}

/// 一條會自動維持可用性的邏輯 CAN 連線。
///
/// `Link` 可安全複製並跨 task 分享。傳輸故障後會以指數退避重建連線，
/// 工廠在每次 `open()` 套用完整通道設定，監督器再重放保存的過濾器。
#[derive(Clone, Debug)]
pub struct Link {
    pub(crate) inner: Arc<LinkInner>,
}

impl Link {
    /// 建立連線建構器。
    #[must_use]
    pub fn builder<F: TransportFactory>(factory: F) -> LinkBuilder<F> {
        LinkBuilder::new(factory)
    }

    fn disconnected_if_fail_fast(&self) -> Result<(), Error> {
        if self.inner.pending_policy == PendingTxPolicy::FailFast
            && self.state() != LinkState::Connected
        {
            Err(Error::Disconnected { attempt: 0 })
        } else if self.state() == LinkState::Closed {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }

    /// 將幀以 fire-and-forget 方式放入 bounded 佇列。
    ///
    /// 此方法不等待實際送出，佇列滿時立即回報 `TxQueueFull`。
    ///
    /// # Errors
    ///
    /// 佇列已滿、連線已關閉或 `FailFast` 政策拒絕斷線傳送時回傳錯誤。
    pub async fn send(&self, frame: Frame) -> Result<(), Error> {
        core::future::ready(self.try_send(frame)).await
    }

    /// 非阻塞嘗試將幀放入傳送佇列。
    ///
    /// # Errors
    ///
    /// 佇列已滿、連線已關閉或 `FailFast` 政策拒絕斷線傳送時回傳錯誤。
    pub fn try_send(&self, frame: Frame) -> Result<(), Error> {
        self.disconnected_if_fail_fast()?;
        self.inner
            .channels
            .tx
            .try_send(TxItem::fire_and_forget(frame))
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => Error::TxQueueFull {
                    capacity: self.inner.tx_capacity,
                },
                tokio::sync::mpsc::error::TrySendError::Closed(_) => Error::Closed,
            })
    }

    /// 在指定期限內等待傳送佇列取得空位。
    ///
    /// # Errors
    ///
    /// 等待逾時、連線已關閉或 `FailFast` 政策拒絕斷線傳送時回傳錯誤。
    pub async fn send_timeout(&self, frame: Frame, timeout: Duration) -> Result<(), Error> {
        self.disconnected_if_fail_fast()?;
        self.inner
            .channels
            .tx
            .send_timeout(TxItem::fire_and_forget(frame), timeout)
            .await
            .map_err(|error| match error {
                tokio::sync::mpsc::error::SendTimeoutError::Timeout(_) => {
                    Error::Timeout { timeout }
                }
                tokio::sync::mpsc::error::SendTimeoutError::Closed(_) => Error::Closed,
            })
    }

    /// 排入幀並等待後端確認實際送出。
    ///
    /// # Errors
    ///
    /// 排入或後端送出失敗、斷線或待送幀逾期時回傳錯誤。
    pub async fn send_await(&self, frame: Frame) -> Result<(), Error> {
        self.disconnected_if_fail_fast()?;
        let (sender, receiver) = oneshot::channel();
        self.inner
            .channels
            .tx
            .send(TxItem::acknowledged(frame, sender))
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// 建立推送式訂閱。
    ///
    /// # Errors
    ///
    /// 連線已關閉或訂閱容量為零時回傳錯誤。
    pub async fn subscribe(&self, config: SubscribeConfig) -> Result<Subscription, Error> {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .channels
            .router
            .send(RouterCommand::Subscribe {
                config,
                reply: sender,
            })
            .map_err(|_| Error::Closed)?;
        let parts = receiver.await.map_err(|_| Error::Closed)??;
        Ok(parts.into_subscription(self.inner.channels.router.clone()))
    }

    /// 以指定過濾器訂閱，其餘採預設值。
    ///
    /// # Errors
    ///
    /// 連線已關閉時回傳錯誤。
    pub async fn subscribe_filter(&self, filter: FilterSet) -> Result<Subscription, Error> {
        self.subscribe(SubscribeConfig::new(filter)).await
    }

    /// 訂閱所有原始幀，供 trace 或記錄用途。
    ///
    /// 此廣播只在首次呼叫時建立；慢消費者會收到 `Lagged`。一般業務邏輯
    /// 應使用過濾在推送前完成的 [`subscribe`](Self::subscribe)。
    pub fn subscribe_all_raw(&self) -> broadcast::Receiver<RxFrame> {
        self.inner
            .raw
            .get_or_init(|| broadcast::channel(256).0)
            .subscribe()
    }

    /// 取得目前連線狀態。
    #[must_use]
    pub fn state(&self) -> LinkState {
        *self.inner.state.borrow()
    }

    /// 訂閱保有最新值的狀態 watch。
    #[must_use]
    pub fn state_watch(&self) -> watch::Receiver<LinkState> {
        self.inner.state.subscribe()
    }

    /// 訂閱連線事件。
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<BusEvent> {
        self.inner.events.subscribe()
    }

    /// 等待連線就緒。
    ///
    /// 使用 `watch` 是因為它保留當前狀態；即使連線早於呼叫成功也不會漏掉。
    ///
    /// # Errors
    ///
    /// 連線永久失敗或已關閉時回傳錯誤。
    pub async fn wait_connected(&self) -> Result<(), Error> {
        let mut state = self.state_watch();
        loop {
            match *state.borrow_and_update() {
                LinkState::Connected => return Ok(()),
                LinkState::Closed => return Err(Error::Closed),
                _ => {}
            }
            state.changed().await.map_err(|_| Error::Closed)?;
        }
    }

    /// 取得最近一次匯流排狀態。
    #[must_use]
    pub fn bus_status(&self) -> BusStatus {
        *lock(&self.inner.bus_status)
    }

    /// 取得無鎖統計快照。
    #[must_use]
    pub fn stats(&self) -> StatsSnapshot {
        self.inner.stats.snapshot()
    }

    /// 取得當前傳輸能力；未連線時為 `None`。
    #[must_use]
    pub fn capabilities(&self) -> Option<Capabilities> {
        *lock(&self.inner.capabilities)
    }

    /// 更新硬體或核心層過濾器，並保存供後續重連完整重放。
    ///
    /// 未連線時只更新保存值；已連線時會等待目前傳輸層確認套用。
    ///
    /// # Errors
    ///
    /// 監督任務已關閉，或目前傳輸層拒絕過濾器時回傳錯誤。
    pub async fn set_hardware_filter(&self, filter: FilterSet) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.inner
            .channels
            .supervisor
            .send(SupervisorCommand::SetHardwareFilter {
                filter,
                reply: sender,
            })
            .await
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }

    /// 註冊週期傳送項目。
    ///
    /// # Errors
    ///
    /// 週期為零或排程器已關閉時回傳錯誤。
    pub fn schedule_cyclic(&self, config: CyclicConfig) -> Result<CyclicHandle, Error> {
        if config.period.is_zero() {
            return Err(Error::Unsupported("週期必須大於零"));
        }
        let id = CyclicId(self.inner.cyclic_next.fetch_add(1, Ordering::Relaxed));
        let (payload_len, stats) = new_shared(config.frame);
        self.inner
            .channels
            .cyclic
            .send(CyclicCommand::Add {
                id,
                config,
                payload_len: Arc::clone(&payload_len),
                stats: Arc::clone(&stats),
            })
            .map_err(|_| Error::Closed)?;
        Ok(CyclicHandle::create(
            id,
            payload_len,
            stats,
            self.inner.channels.cyclic.clone(),
        ))
    }

    /// 先向 RX 路由註冊並等待 ack，再回傳等待器。
    ///
    /// 這個順序保證快速 ECU 回應不會在等待器完成註冊前抵達。等待器被
    /// `tokio::select!` 取消或直接丟棄時，其 `Drop` guard 會自動註銷。
    ///
    /// # Errors
    ///
    /// 連線已關閉或同時進行的交易達上限時回傳交易錯誤。
    pub async fn prepare(&self, spec: &ResponseSpec) -> Result<PendingResponse, TransactionError> {
        if self.state() == LinkState::Closed {
            return Err(TransactionError::Closed);
        }
        let capacity = match spec.mode {
            CollectMode::First => 1,
            CollectMode::Exactly(count) => count.get(),
            CollectMode::Window(_) => 64,
        };
        let (sender, receiver) = oneshot::channel();
        self.inner
            .channels
            .transaction
            .send(TransactionCommand::Register {
                matcher: spec.matcher.clone(),
                capacity,
                reply: sender,
            })
            .map_err(|_| TransactionError::Closed)?;
        let (id, frames) = receiver.await.map_err(|_| TransactionError::Closed)??;
        Ok(PendingResponse {
            id,
            receiver: frames,
            control: self.inner.channels.transaction.clone(),
            spec: spec.clone(),
            registered: true,
        })
    }

    async fn request_inner(
        &self,
        request: Frame,
        spec: &ResponseSpec,
    ) -> Result<Vec<RxFrame>, TransactionError> {
        let mut pending = self.prepare(spec).await?;
        let mut retry = 0_u8;
        loop {
            self.send_await(request)
                .await
                .map_err(|error| TransactionError::Send(Box::new(error)))?;
            match pending.wait_attempt().await {
                Ok(frames) => return Ok(frames),
                Err(TransactionError::Timeout { .. }) if retry < spec.retries => {
                    retry = retry.saturating_add(1);
                    pending.clear_buffer();
                    let _receivers = self
                        .inner
                        .events
                        .send(BusEvent::TransactionRetried { attempt: retry });
                }
                Err(TransactionError::Timeout { .. }) => {
                    return Err(TransactionError::Timeout {
                        timeout: spec.timeout,
                        retries: retry,
                    });
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// 送出請求並等待第一個符合條件的回應。
    ///
    /// # Errors
    ///
    /// 註冊、傳送、等待或回應比對失敗時回傳交易錯誤。
    pub async fn request(
        &self,
        request: Frame,
        spec: &ResponseSpec,
    ) -> Result<RxFrame, TransactionError> {
        self.request_inner(request, spec)
            .await?
            .into_iter()
            .next()
            .ok_or(TransactionError::Disconnected)
    }

    /// 送出請求並依規格收集多個回應。
    ///
    /// # Errors
    ///
    /// 註冊、傳送、等待或回應比對失敗時回傳交易錯誤。
    pub async fn request_many(
        &self,
        request: Frame,
        spec: &ResponseSpec,
    ) -> Result<Vec<RxFrame>, TransactionError> {
        self.request_inner(request, spec).await
    }

    /// 取得目前已註冊的交易數，主要供壓力與取消清理測試。
    #[doc(hidden)]
    #[must_use]
    pub fn in_flight_count(&self) -> usize {
        self.inner.in_flight.load(Ordering::Acquire)
    }

    /// 主動關閉連線與所有背景排程；重複呼叫安全。
    pub async fn close(&self) {
        if self.state() == LinkState::Closed {
            return;
        }
        let (sender, receiver) = oneshot::channel();
        if self
            .inner
            .channels
            .supervisor
            .send(SupervisorCommand::Close(sender))
            .await
            .is_ok()
        {
            let _ignored = receiver.await;
        }
    }
}
