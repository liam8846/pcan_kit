//! 請求與回應交易整合測試。

use core::future::Future;
use core::num::NonZeroUsize;
use core::task::{Context, Waker};
use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeHandle, FakeTransportBuilder};
use pcan_core::{CanId, Frame, RxFrame, Timestamp, TimestampSource, TransportEvent};
use pcan_link::{
    BusEvent, CollectMode, Link, Matcher, PrefixPattern, ResponseSpec, TransactionError,
};

fn frame(id: u16, data: &[u8]) -> Frame {
    Frame::new(CanId::standard(id).expect("ID"), data).expect("幀")
}

fn inject(handle: &FakeHandle, id: u16, data: &[u8]) {
    handle.inject(TransportEvent::Frame(RxFrame::new(
        frame(id, data),
        Timestamp::new(0, TimestampSource::Software),
        false,
    )));
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn registration_precedes_send_and_fast_response_is_not_lost() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let observer = handle.clone();
    tokio::spawn(async move {
        loop {
            if !observer.sent().is_empty() {
                inject(&observer, 0x708, &[0x62, 1]);
                break;
            }
            tokio::task::yield_now().await;
        }
    });
    let spec = ResponseSpec::new(
        Matcher::IdAndPrefix {
            id: CanId::standard(0x708).expect("ID"),
            prefix: PrefixPattern::new(&[0x62]).expect("前綴"),
        },
        Duration::from_secs(1),
    );
    let response = link
        .request(frame(0x700, &[0x22, 1]), &spec)
        .await
        .expect("回應");
    assert_eq!(response.frame.data(), &[0x62, 1]);
}

#[tokio::test(start_paused = true)]
async fn exact_collection_and_timeout_work() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::IdMask {
            id: 0x700,
            mask: 0x7f0,
        },
        Duration::from_millis(100),
    )
    .with_mode(CollectMode::Exactly(NonZeroUsize::new(3).expect("非零")))
    .with_max_responses(NonZeroUsize::new(1).expect("非零"));
    let responder = handle.clone();
    tokio::spawn(async move {
        loop {
            if !responder.sent().is_empty() {
                inject(&responder, 0x701, &[1]);
                inject(&responder, 0x702, &[2]);
                inject(&responder, 0x703, &[3]);
                break;
            }
            tokio::task::yield_now().await;
        }
    });
    let responses = link
        .request_many(frame(0x600, &[0]), &spec)
        .await
        .expect("三個回應");
    assert_eq!(responses.len(), 3);

    let timeout_spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_millis(10),
    );
    let request = link.request(frame(0x100, &[0]), &timeout_spec);
    tokio::pin!(request);
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    assert!(matches!(
        request.await,
        Err(TransactionError::Timeout { retries: 0, .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn retries_restart_timeout_and_reuse_registration() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_millis(10),
    )
    .with_retries(2);
    let request_link = link.clone();
    let task = tokio::spawn(async move { request_link.request(frame(0x100, &[1]), &spec).await });
    settle().await;
    for _ in 0..3 {
        tokio::time::advance(Duration::from_millis(10)).await;
        settle().await;
    }
    assert!(matches!(
        task.await.expect("task"),
        Err(TransactionError::Timeout { retries: 2, .. })
    ));
    assert_eq!(handle.sent().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn cancellation_cleans_all_waiters_and_limit_is_enforced() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .health_check_interval(None)
        .max_in_flight_transactions(1)
        .build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x123).expect("ID")),
        Duration::from_secs(1),
    );
    let first = link.prepare(&spec).await.expect("第一個");
    assert!(matches!(
        link.prepare(&spec).await,
        Err(TransactionError::TooManyInFlight { limit: 1 })
    ));
    drop(first);
    settle().await;
    assert_eq!(link.in_flight_count(), 0);
    for _ in 0..100 {
        let pending = link.prepare(&spec).await.expect("建立");
        drop(pending);
    }
    settle().await;
    assert_eq!(link.in_flight_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn fatal_disconnect_wakes_waiter_without_advancing_timeout() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(60),
    );
    let request_link = link.clone();
    let task = tokio::spawn(async move { request_link.request(frame(0x100, &[1]), &spec).await });
    settle().await;
    assert_eq!(handle.sent().len(), 1);
    handle.inject_fault(pcan_core::FaultKind::Fatal);
    settle().await;
    assert!(matches!(
        task.await.expect("task"),
        Err(TransactionError::Disconnected)
    ));
}

#[tokio::test(start_paused = true)]
async fn full_window_buffer_returns_collected_frames_instead_of_disconnect() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(1),
    )
    .with_mode(CollectMode::Window(Duration::from_millis(10)));
    let pending = link.prepare(&spec).await.expect("註冊等待者");

    for value in 0_u8..70 {
        inject(&handle, 0x555, &[value]);
    }
    while link.stats().rx_frames < 70 {
        tokio::task::yield_now().await;
    }
    settle().await;

    let frames = pending.wait_many().await.expect("窗口收集應成功");
    assert_eq!(frames.len(), 64);
}

#[tokio::test(start_paused = true)]
async fn disappeared_waiter_is_removed_after_matching_frame() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(1),
    );
    let pending = link.prepare(&spec).await.expect("註冊等待者");
    assert_eq!(link.in_flight_count(), 1);

    drop(pending);
    inject(&handle, 0x555, &[1]);
    while link.stats().rx_frames < 1 {
        tokio::task::yield_now().await;
    }
    settle().await;

    assert_eq!(link.in_flight_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn transaction_drop_event_is_edge_triggered() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let mut events = link.events();
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(1),
    )
    .with_mode(CollectMode::Window(Duration::from_millis(10)));
    let pending = link.prepare(&spec).await.expect("註冊等待者");

    for value in 0_u8..70 {
        inject(&handle, 0x555, &[value]);
    }
    while link.stats().rx_frames < 70 {
        tokio::task::yield_now().await;
    }
    settle().await;

    let dropped_events = core::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| matches!(event, BusEvent::TransactionDropped { .. }))
        .count();
    assert_eq!(dropped_events, 1);

    let _frames = pending.wait_many().await.expect("窗口收集應成功");
}

#[tokio::test(start_paused = true)]
async fn window_result_limit_applies_even_while_receiver_drains() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let mut events = link.events();
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(1),
    )
    .with_mode(CollectMode::Window(Duration::from_secs(1)));
    assert_eq!(spec.max_responses.get(), 64);
    let spec = spec.with_max_responses(NonZeroUsize::new(3).expect("非零"));
    let pending = link.prepare(&spec).await.expect("註冊");
    let task = tokio::spawn(pending.wait_many());
    for value in 0..3 {
        inject(&handle, 0x555, &[value]);
        settle().await;
        assert!(!task.is_finished(), "恰好達到上限仍應等待時間窗結束");
    }
    inject(&handle, 0x555, &[3]);
    settle().await;
    assert!(matches!(
        task.await.expect("等待工作"),
        Err(TransactionError::ResponseLimitExceeded { limit: 3 })
    ));
    settle().await;
    assert_eq!(link.in_flight_count(), 0);
    assert!(
        !core::iter::from_fn(|| events.try_recv().ok())
            .any(|event| matches!(event, BusEvent::TransactionDropped { .. })),
        "結果超量與接收通道溢位應分別回報"
    );
    link.close().await;
}

#[tokio::test(start_paused = true)]
async fn expired_window_wins_over_queued_responses() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let window = Duration::from_millis(10);
    let spec = ResponseSpec::new(
        Matcher::Id(CanId::standard(0x555).expect("ID")),
        Duration::from_secs(1),
    )
    .with_mode(CollectMode::Window(window));
    let pending = link.prepare(&spec).await.expect("註冊");
    // 手動 poll 讓截止時間確立，再保持 future 未被 poll 到期。
    inject(&handle, 0x555, &[1]);
    settle().await;
    let mut attempt = Box::pin(pending.wait_many());
    assert!(
        attempt
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    tokio::time::advance(window).await;
    inject(&handle, 0x555, &[2]);
    settle().await;
    assert_eq!(attempt.await.expect("時間窗結束").len(), 1);
    link.close().await;
}
