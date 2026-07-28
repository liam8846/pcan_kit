//! 無需 CAN 硬體即可驅動上層狀態機的測試替身。

use core::future::Future;
use core::time::Duration;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::error::{BackendError, Error, FaultKind, Result};
use crate::filter::FilterSet;
use crate::frame::Frame;
use crate::status::BusStatus;
use crate::transport::{Capabilities, Transport, TransportEvent, TransportFactory};

#[derive(Clone, Copy, Debug)]
enum RecvItem {
    Event(TransportEvent),
    Fault(FaultKind),
}

#[derive(Debug)]
struct FakeState {
    sent: Vec<Frame>,
    open_count: u32,
    close_count: u32,
    is_open: bool,
    status: BusStatus,
    last_filter: Option<FilterSet>,
    open_failures_remaining: u32,
    open_failure_kind: Option<FaultKind>,
    tx_failure_kind: Option<FaultKind>,
    tx_busy_remaining: u32,
    capabilities: Capabilities,
    fault_after: Option<(usize, FaultKind)>,
    fault_after_fired: bool,
    delivered_events: usize,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<FakeState>,
    sender: UnboundedSender<RecvItem>,
    receiver: tokio::sync::Mutex<UnboundedReceiver<RecvItem>>,
}

fn lock_state(shared: &Shared) -> MutexGuard<'_, FakeState> {
    match shared.state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn fake_backend(kind: FaultKind, op: &'static str) -> BackendError {
    BackendError::PcanBasic {
        code: 0,
        text: "FakeTransport 注入故障".into(),
        op,
        kind,
    }
}

fn shared_from_builder(builder: FakeTransportBuilder, is_open: bool) -> (Arc<Shared>, Duration) {
    let (sender, receiver) = unbounded_channel();
    for event in builder.events {
        if sender.send(RecvItem::Event(event)).is_err() {
            break;
        }
    }

    let state = FakeState {
        sent: Vec::new(),
        open_count: 0,
        close_count: 0,
        is_open,
        status: BusStatus::default(),
        last_filter: None,
        open_failures_remaining: builder.open_failures,
        open_failure_kind: builder.open_failure_kind,
        tx_failure_kind: builder.tx_failure_kind,
        tx_busy_remaining: builder.tx_busy_times,
        capabilities: builder.capabilities,
        fault_after: builder.fault_after,
        fault_after_fired: false,
        delivered_events: 0,
    };
    (
        Arc::new(Shared {
            state: Mutex::new(state),
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
        }),
        builder.open_delay,
    )
}

/// 測試用的假傳輸層。
///
/// 此型別實作與真實後端完全相同的 [`Transport`]，讓重連、佇列、路由、
/// 週期傳送與交易邏輯可在無硬體環境下完整測試。
#[derive(Debug)]
pub struct FakeTransport {
    shared: Arc<Shared>,
}

/// [`FakeTransport`] 的建構器。
#[derive(Debug, Default)]
pub struct FakeTransportBuilder {
    open_failures: u32,
    open_failure_kind: Option<FaultKind>,
    open_delay: Duration,
    events: Vec<TransportEvent>,
    fault_after: Option<(usize, FaultKind)>,
    tx_failure_kind: Option<FaultKind>,
    tx_busy_times: u32,
    capabilities: Capabilities,
}

impl FakeTransportBuilder {
    /// 讓前 `times` 次 `open()` 以指定故障類別失敗。
    #[must_use]
    pub fn open_fails(mut self, times: u32, kind: FaultKind) -> Self {
        self.open_failures = times;
        self.open_failure_kind = Some(kind);
        self
    }

    /// 設定每次 `open()` 前的模擬延遲。
    #[must_use]
    pub fn open_delay(mut self, delay: Duration) -> Self {
        self.open_delay = delay;
        self
    }

    /// 預先排入一個要送給上層的事件。
    #[must_use]
    pub fn push_event(mut self, event: TransportEvent) -> Self {
        self.events.push(event);
        self
    }

    /// 在送出第 `n` 個事件之後注入一次接收故障。
    ///
    /// `n` 為零時，第一次 `recv()` 會立即收到故障。
    #[must_use]
    pub fn fault_after(mut self, n: usize, kind: FaultKind) -> Self {
        self.fault_after = Some((n, kind));
        self
    }

    /// 讓每次 `send()` 都以指定故障類別失敗。
    #[must_use]
    pub fn tx_fails_with(mut self, kind: FaultKind) -> Self {
        self.tx_failure_kind = Some(kind);
        self
    }

    /// 讓 `send()` 的前 `n` 次回報傳送佇列已滿。
    #[must_use]
    pub fn tx_busy_times(mut self, n: u32) -> Self {
        self.tx_busy_times = n;
        self
    }

    /// 設定假後端回報的執行期能力。
    #[must_use]
    pub fn capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// 建立已開啟的傳輸實例與其測試遙控器。
    pub fn build(self) -> (FakeTransport, FakeHandle) {
        let (shared, _) = shared_from_builder(self, true);
        (
            FakeTransport {
                shared: Arc::clone(&shared),
            },
            FakeHandle { shared },
        )
    }
}

/// [`FakeTransport`] 的測試遙控器。
///
/// 遙控器可複製並跨 task 使用，也能觀測同一工廠跨多次 `open()` 的累計狀態。
#[derive(Clone, Debug)]
pub struct FakeHandle {
    shared: Arc<Shared>,
}

impl FakeHandle {
    /// 注入一個事件，供下一次 `recv()` 取得。
    pub fn inject(&self, event: TransportEvent) {
        let _closed = self.shared.sender.send(RecvItem::Event(event)).is_err();
    }

    /// 注入一次故障，讓下一次 `recv()` 回傳錯誤。
    pub fn inject_fault(&self, kind: FaultKind) {
        let _closed = self.shared.sender.send(RecvItem::Fault(kind)).is_err();
    }

    /// 取得目前為止成功送出的所有幀。
    #[must_use]
    pub fn sent(&self) -> Vec<Frame> {
        lock_state(&self.shared).sent.clone()
    }

    /// 清空已送出幀的測試紀錄。
    pub fn clear_sent(&self) {
        lock_state(&self.shared).sent.clear();
    }

    /// 取得工廠 `open()` 被呼叫的累計次數。
    #[must_use]
    pub fn open_count(&self) -> u32 {
        lock_state(&self.shared).open_count
    }

    /// 取得 `close()` 被呼叫的累計次數。
    #[must_use]
    pub fn close_count(&self) -> u32 {
        lock_state(&self.shared).close_count
    }

    /// 判斷目前共享的假通道是否處於開啟狀態。
    #[must_use]
    pub fn is_open(&self) -> bool {
        lock_state(&self.shared).is_open
    }

    /// 設定 `status()` 要回報的匯流排狀態。
    pub fn set_status(&self, status: BusStatus) {
        lock_state(&self.shared).status = status;
    }

    /// 取得最後一次 `set_filter()` 套用的過濾器。
    #[must_use]
    pub fn last_filter(&self) -> Option<FilterSet> {
        lock_state(&self.shared).last_filter.clone()
    }
}

#[allow(clippy::manual_async_fn)]
impl Transport for FakeTransport {
    fn recv(&self) -> impl Future<Output = Result<TransportEvent>> + Send {
        async move {
            {
                let mut state = lock_state(&self.shared);
                if !state.is_open {
                    return Err(Error::Closed);
                }
                if let Some((after, kind)) = state.fault_after
                    && !state.fault_after_fired
                    && state.delivered_events >= after
                {
                    state.fault_after_fired = true;
                    return Err(Error::Io(fake_backend(kind, "recv")));
                }
            }

            let item = {
                let mut receiver = self.shared.receiver.lock().await;
                receiver.recv().await
            };
            match item {
                Some(RecvItem::Event(event)) => {
                    lock_state(&self.shared).delivered_events += 1;
                    Ok(event)
                }
                Some(RecvItem::Fault(kind)) => Err(Error::Io(fake_backend(kind, "recv"))),
                None => Err(Error::Closed),
            }
        }
    }

    fn send(&self, frame: &Frame) -> impl Future<Output = Result<()>> + Send {
        let frame = *frame;
        async move {
            let mut state = lock_state(&self.shared);
            if !state.is_open {
                return Err(Error::Closed);
            }
            if state.tx_busy_remaining > 0 {
                state.tx_busy_remaining -= 1;
                return Err(Error::TxQueueFull { capacity: 1 });
            }
            if let Some(kind) = state.tx_failure_kind {
                return Err(Error::Io(fake_backend(kind, "send")));
            }
            state.sent.push(frame);
            Ok(())
        }
    }

    fn status(&self) -> impl Future<Output = Result<BusStatus>> + Send {
        async move {
            let state = lock_state(&self.shared);
            if state.is_open {
                Ok(state.status)
            } else {
                Err(Error::Closed)
            }
        }
    }

    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<()>> + Send {
        let filter = filter.clone();
        async move {
            let mut state = lock_state(&self.shared);
            if !state.is_open {
                return Err(Error::Closed);
            }
            state.last_filter = Some(filter);
            Ok(())
        }
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        async move {
            let mut state = lock_state(&self.shared);
            state.close_count += 1;
            state.is_open = false;
        }
    }

    fn capabilities(&self) -> Capabilities {
        lock_state(&self.shared).capabilities
    }
}

/// 產生 [`FakeTransport`] 的測試工廠。
///
/// 每次開啟都沿用同一份可由 [`FakeHandle`] 觀測的共享狀態，適合驗證重連
/// 次數與設定重放。
#[derive(Debug)]
pub struct FakeFactory {
    shared: Arc<Shared>,
    open_delay: Duration,
}

impl FakeFactory {
    /// 由建構器建立工廠與跨重連共用的測試遙控器。
    pub fn new(builder: FakeTransportBuilder) -> (Self, FakeHandle) {
        let (shared, open_delay) = shared_from_builder(builder, false);
        (
            Self {
                shared: Arc::clone(&shared),
                open_delay,
            },
            FakeHandle { shared },
        )
    }
}

#[allow(clippy::manual_async_fn)]
impl TransportFactory for FakeFactory {
    type Transport = FakeTransport;

    fn open(&self) -> impl Future<Output = Result<Self::Transport>> + Send {
        async move {
            if !self.open_delay.is_zero() {
                tokio::time::sleep(self.open_delay).await;
            }

            let mut state = lock_state(&self.shared);
            state.open_count += 1;
            if state.open_failures_remaining > 0 {
                state.open_failures_remaining -= 1;
                let kind = state.open_failure_kind.unwrap_or(FaultKind::Permanent);
                return Err(Error::Open {
                    channel: "fake".into(),
                    source: fake_backend(kind, "open"),
                });
            }
            state.is_open = true;
            drop(state);
            Ok(FakeTransport {
                shared: Arc::clone(&self.shared),
            })
        }
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn describe(&self) -> &str {
        "fake"
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeFactory, FakeTransportBuilder};
    use crate::error::{FaultKind, Result};
    use crate::frame::{Frame, RxFrame, Timestamp, TimestampSource};
    use crate::id::CanId;
    use crate::transport::{Transport, TransportEvent, TransportFactory};

    fn frame() -> Frame {
        let id = CanId::standard(0x123).unwrap_or_else(|error| unreachable!("{error}"));
        Frame::new(id, &[1, 2, 3]).unwrap_or_else(|error| unreachable!("{error}"))
    }

    fn event() -> TransportEvent {
        TransportEvent::Frame(RxFrame::new(
            frame(),
            Timestamp::new(1, TimestampSource::Software),
            false,
        ))
    }

    #[tokio::test]
    async fn injects_receives_and_records_sent_frames() -> Result<()> {
        let (transport, handle) = FakeTransportBuilder::default().build();
        handle.inject(event());
        assert_eq!(transport.recv().await?, event());

        transport.send(&frame()).await?;
        assert_eq!(handle.sent(), vec![frame()]);
        handle.clear_sent();
        assert!(handle.sent().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn simulates_busy_send_and_idempotent_close() -> Result<()> {
        let (transport, handle) = FakeTransportBuilder::default().tx_busy_times(2).build();
        assert!(matches!(
            transport.send(&frame()).await,
            Err(crate::Error::TxQueueFull { .. })
        ));
        assert!(matches!(
            transport.send(&frame()).await,
            Err(crate::Error::TxQueueFull { .. })
        ));
        transport.send(&frame()).await?;

        transport.close().await;
        transport.close().await;
        assert_eq!(handle.close_count(), 2);
        assert!(!handle.is_open());
        Ok(())
    }

    #[tokio::test]
    async fn factory_counts_and_fails_initial_opens() -> Result<()> {
        let builder = FakeTransportBuilder::default().open_fails(2, FaultKind::Fatal);
        let (factory, handle) = FakeFactory::new(builder);

        let first = factory.open().await;
        assert!(first.is_err());
        let second = factory.open().await;
        assert!(second.is_err());
        let transport = factory.open().await?;

        assert_eq!(handle.open_count(), 3);
        assert!(handle.is_open());
        assert_eq!(factory.describe(), "fake");
        transport.close().await;
        Ok(())
    }
}
