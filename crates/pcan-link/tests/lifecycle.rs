//! 背景工作任務與連線釋放生命週期整合測試。

use core::time::Duration;
use std::sync::Arc;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{
    BusState, BusStatus, BusWarnings, CanId, FilterRule, FilterSet, Frame, RxFrame, Timestamp,
    TimestampSource, TransportEvent,
};
use pcan_link::{
    BusEvent, CyclicConfig, Error, Link, LinkState, MatchResult, Matcher, PendingTxPolicy,
    ResponseSpec, SubscribeConfig,
};

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !predicate() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("條件應在期限內成立");
}

fn frame(id: u16, value: u8) -> Frame {
    Frame::new(CanId::standard(id).expect("ID"), &[value]).expect("幀")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recv_panic_closes_link_and_reports_worker_loss() {
    // `build()` 會立即啟動監督任務，而 broadcast 訂閱只收得到訂閱之後發出的
    // 事件。開啟延遲把監督任務停在 `open()` 的 await 點，確保下方的
    // `link.events()` 一定早於 `recv()` panic 觸發的 `WorkerLost` 廣播。
    let (factory, _) = FakeFactory::new(
        FakeTransportBuilder::default()
            .panic_on_recv()
            .open_delay(Duration::from_millis(200)),
    );
    let link = Link::builder(factory)
        .pending_tx_policy(PendingTxPolicy::FailFast)
        .health_check_interval(None)
        .build();
    let mut events = link.events();

    wait_until(|| link.state() == LinkState::Closed).await;
    assert!(matches!(link.wait_connected().await, Err(Error::Closed)));
    assert_eq!(link.state(), LinkState::Closed);
    assert!(matches!(link.try_send(frame(0x123, 1)), Err(Error::Closed)));
    assert!(matches!(
        link.send_await(frame(0x123, 2)).await,
        Err(Error::Closed)
    ));
    tokio::time::timeout(Duration::from_millis(100), link.close())
        .await
        .expect("關閉不得等待失聯工作者");

    let worker = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let BusEvent::WorkerLost { worker } = events.recv().await.expect("工作者事件") {
                break worker;
            }
        }
    })
    .await
    .expect("應收到工作者遺失事件");
    assert_eq!(worker, "supervisor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn supervisor_panic_closes_existing_subscription() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let rejected_id = CanId::standard(0x456).expect("ID");
    let mut subscription = link
        .subscribe(
            SubscribeConfig::new(FilterSet::with(FilterRule::exact(rejected_id))).with_capacity(1),
        )
        .await
        .expect("訂閱");
    let spec = ResponseSpec::new(
        Matcher::Custom(Arc::new(|_: &RxFrame| -> MatchResult {
            panic!("測試用 Matcher panic");
        })),
        Duration::from_secs(1),
    );
    let _pending = link.prepare(&spec).await.expect("註冊交易");

    handle.inject(TransportEvent::Frame(RxFrame::new(
        frame(0x123, 3),
        Timestamp::new(0, TimestampSource::Software),
        false,
    )));
    wait_until(|| link.state() == LinkState::Closed).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("訂閱不得永久等待"),
        None
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_last_link_closes_transport_once() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");

    drop(link);
    wait_until(|| !handle.is_open()).await;
    assert_eq!(handle.close_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_link_stops_cyclic_work_without_busy_loop() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let _id = link
        .schedule_cyclic(CyclicConfig::new(
            frame(0x123, 7),
            Duration::from_millis(10),
        ))
        .expect("週期排程")
        .detach();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_millis(10)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert!(!handle.sent().is_empty());

    drop(link);
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(!handle.is_open());
    let sent_after_close = handle.sent().len();
    let opens_after_close = handle.open_count();
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert_eq!(handle.sent().len(), sent_after_close);
    assert_eq!(handle.open_count(), opens_after_close);
    assert_eq!(handle.close_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamed_status_counts_error_frames_and_overrun_edges() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let warnings = BusWarnings::RX_OVERRUN | BusWarnings::QUEUE_OVERRUN;
    let warning_status = BusStatus::new(BusState::Warning, warnings, None);

    handle.inject(TransportEvent::Status(warning_status));
    wait_until(|| link.stats().rx_error_frames == 1).await;
    handle.inject(TransportEvent::Status(warning_status));
    wait_until(|| link.stats().rx_error_frames == 2).await;
    let snapshot = link.stats();
    assert_eq!(snapshot.rx_hw_overrun, 1);
    assert_eq!(snapshot.rx_queue_overrun, 1);

    handle.inject(TransportEvent::Status(BusStatus::default()));
    wait_until(|| link.stats().rx_error_frames == 3).await;
    handle.inject(TransportEvent::Status(warning_status));
    wait_until(|| link.stats().rx_error_frames == 4).await;
    let snapshot = link.stats();
    assert_eq!(snapshot.rx_hw_overrun, 2);
    assert_eq!(snapshot.rx_queue_overrun, 2);
}

#[tokio::test(start_paused = true)]
async fn polled_health_status_does_not_count_as_error_frame() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .health_check_interval(Some(Duration::from_millis(10)))
        .build();
    link.wait_connected().await.expect("連線");
    handle.set_status(BusStatus::new(
        BusState::Warning,
        BusWarnings::RX_OVERRUN,
        None,
    ));
    tokio::time::advance(Duration::from_millis(10)).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    let snapshot = link.stats();
    assert_eq!(snapshot.rx_error_frames, 0);
    assert_eq!(snapshot.rx_hw_overrun, 1);
}
