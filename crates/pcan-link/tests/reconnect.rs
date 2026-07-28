//! 自動重連與關閉整合測試。

use core::future::{Future, pending};
use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransport, FakeTransportBuilder};
use pcan_core::{
    BusState, BusStatus, BusWarnings, CanId, Capabilities, FaultKind, FilterRule, FilterSet, Frame,
    Result, Transport, TransportEvent, TransportFactory,
};
use pcan_link::{BackoffPolicy, BusEvent, Link, LinkState, PendingTxPolicy};

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

struct HangingStatus {
    inner: FakeTransport,
}

impl Transport for HangingStatus {
    fn recv(&self) -> impl Future<Output = Result<TransportEvent>> + Send {
        self.inner.recv()
    }

    fn send(&self, frame: &Frame) -> impl Future<Output = Result<()>> + Send {
        self.inner.send(frame)
    }

    fn status(&self) -> impl Future<Output = Result<BusStatus>> + Send {
        pending()
    }

    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<()>> + Send {
        self.inner.set_filter(filter)
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        self.inner.close()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }
}

struct HangingStatusFactory {
    inner: FakeFactory,
}

#[allow(clippy::manual_async_fn)]
impl TransportFactory for HangingStatusFactory {
    type Transport = HangingStatus;

    fn open(&self) -> impl Future<Output = Result<Self::Transport>> + Send {
        async move { self.inner.open().await.map(|inner| HangingStatus { inner }) }
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn describe(&self) -> &str {
        "fake:hanging-status"
    }
}

#[tokio::test(start_paused = true)]
async fn retries_failed_opens_on_exact_backoff_schedule() {
    let (factory, handle) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(2, FaultKind::Fatal));
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    settle().await;
    assert_eq!(handle.open_count(), 1);

    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(handle.open_count(), 2);
    tokio::time::advance(Duration::from_millis(200)).await;
    settle().await;
    assert_eq!(handle.open_count(), 3);
    assert_eq!(link.state(), LinkState::Connected);
    assert_eq!(handle.last_filter(), Some(FilterSet::accept_all()));
}

#[tokio::test(start_paused = true)]
async fn fatal_rx_fault_reconnects_without_old_closed_error_poisoning_machine() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("初始連線");
    handle.inject_fault(FaultKind::Fatal);
    settle().await;
    assert_eq!(link.state(), LinkState::Backoff { attempt: 1 });
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(handle.open_count(), 2);
    assert_eq!(link.state(), LinkState::Connected);
    assert_eq!(handle.last_filter(), Some(FilterSet::accept_all()));
}

#[tokio::test(start_paused = true)]
async fn updated_hardware_filter_is_replayed_after_reconnect() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let initial = FilterSet::with(FilterRule::exact(CanId::standard(0x100).expect("ID")));
    let updated = FilterSet::with(FilterRule::exact(CanId::standard(0x321).expect("ID")));
    let link = Link::builder(factory)
        .hardware_filter(initial)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    link.set_hardware_filter(updated.clone())
        .await
        .expect("更新過濾器");
    assert_eq!(handle.last_filter(), Some(updated.clone()));

    handle.inject_fault(FaultKind::Fatal);
    settle().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(link.state(), LinkState::Connected);
    assert_eq!(handle.last_filter(), Some(updated));
}

#[tokio::test(start_paused = true)]
async fn open_timeout_enters_backoff_instead_of_stalling() {
    let (factory, handle) =
        FakeFactory::new(FakeTransportBuilder::default().open_delay(Duration::from_secs(10)));
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .open_timeout(Duration::from_millis(5))
        .health_check_interval(None)
        .build();
    settle().await;
    tokio::time::advance(Duration::from_millis(5)).await;
    settle().await;
    assert_eq!(link.state(), LinkState::Backoff { attempt: 1 });
    assert_eq!(handle.open_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn recoverable_does_not_reconnect_and_permanent_closes() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("初始連線");
    handle.inject_fault(FaultKind::Recoverable);
    settle().await;
    assert_eq!(handle.open_count(), 1);
    assert_eq!(link.state(), LinkState::Connected);

    handle.inject_fault(FaultKind::Permanent);
    settle().await;
    assert_eq!(link.state(), LinkState::Closed);
    tokio::time::advance(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(handle.open_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn fail_fast_rejects_during_backoff_and_close_is_idempotent() {
    let (factory, handle) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Fatal));
    let link = Link::builder(factory)
        .pending_tx_policy(PendingTxPolicy::FailFast)
        .health_check_interval(None)
        .build();
    settle().await;
    let id = pcan_core::CanId::standard(0x123).expect("有效 ID");
    let frame = pcan_core::Frame::new(id, &[1]).expect("有效幀");
    assert!(matches!(
        link.send(frame).await,
        Err(pcan_core::Error::Disconnected { .. })
    ));
    tokio::time::advance(Duration::from_millis(150)).await;
    settle().await;
    link.close().await;
    link.close().await;
    assert_eq!(link.state(), LinkState::Closed);
    assert_eq!(handle.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn hold_flushes_fresh_frames_and_expires_old_frames() {
    let (factory, handle) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Fatal));
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .max_pending_age(Duration::from_millis(50))
        .health_check_interval(None)
        .build();
    settle().await;
    let frame = Frame::new(CanId::standard(0x123).expect("ID"), &[1]).expect("幀");
    link.send(frame).await.expect("斷線期間保留");
    tokio::time::advance(Duration::from_millis(60)).await;
    settle().await;
    assert_eq!(link.stats().tx_dropped, 1);
    tokio::time::advance(Duration::from_millis(40)).await;
    settle().await;
    assert!(handle.sent().is_empty());

    let (fresh_factory, fresh_handle) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Fatal));
    let mut fresh_policy = BackoffPolicy::default();
    fresh_policy.jitter_ratio = 0.0;
    let fresh_link = Link::builder(fresh_factory)
        .backoff(fresh_policy)
        .health_check_interval(None)
        .build();
    settle().await;
    let fresh = Frame::new(CanId::standard(0x124).expect("ID"), &[2]).expect("幀");
    fresh_link.send(fresh).await.expect("保留新幀");
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(fresh_handle.sent(), vec![fresh]);
}

#[tokio::test(start_paused = true)]
async fn events_are_ordered_and_connected_watch_is_level_triggered() {
    let (factory, _) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Fatal));
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    let mut events = link.events();
    settle().await;
    assert_eq!(events.try_recv().expect("Connecting"), BusEvent::Connecting);
    assert!(matches!(
        events.try_recv().expect("Reconnecting"),
        BusEvent::Reconnecting { attempt: 1, .. }
    ));
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(events.try_recv().expect("Connecting"), BusEvent::Connecting);
    assert_eq!(
        events.try_recv().expect("Connected"),
        BusEvent::Connected { attempt: 1 }
    );
    link.wait_connected().await.expect("已連線時立即完成");
}

#[tokio::test(start_paused = true)]
async fn bus_off_status_triggers_reconnect() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    handle.inject(TransportEvent::Status(BusStatus::new(
        BusState::BusOff,
        BusWarnings::empty(),
        None,
    )));
    settle().await;
    assert_eq!(link.state(), LinkState::Backoff { attempt: 1 });
    assert_eq!(link.stats().bus_off_events, 1);
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(link.state(), LinkState::Connected);
    assert_eq!(handle.open_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn health_check_timeout_triggers_reconnect() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let mut policy = BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(HangingStatusFactory { inner: factory })
        .backoff(policy)
        .health_check_interval(Some(Duration::from_millis(10)))
        .build();
    link.wait_connected().await.expect("連線");
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    assert_eq!(link.state(), LinkState::Backoff { attempt: 1 });
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(link.state(), LinkState::Connected);
    assert_eq!(handle.open_count(), 2);
}
