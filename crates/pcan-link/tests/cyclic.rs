//! 單一排程器週期傳送整合測試。

use core::num::NonZeroU32;
use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{CanId, Frame};
use pcan_link::{BusEvent, CyclicConfig, Link, OverrunPolicy, Repeat};

async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

fn frame(value: u8) -> Frame {
    let id = CanId::standard(0x123).expect("ID");
    Frame::new(id, &[value]).expect("幀")
}

#[tokio::test(start_paused = true)]
async fn count_payload_pause_and_raii_stop_work() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let config = CyclicConfig::new(frame(1), Duration::from_millis(10))
        .with_overrun(OverrunPolicy::Burst)
        .with_repeat(Repeat::Count(NonZeroU32::new(3).expect("非零")));
    let cyclic = link.schedule_cyclic(config).expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    cyclic.set_payload(&[2]).expect("更新");
    tokio::time::advance(Duration::from_millis(20)).await;
    settle().await;
    let sent = handle.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].data(), &[1]);
    assert_eq!(sent[1].data(), &[2]);

    let repeating = link
        .schedule_cyclic(CyclicConfig::new(frame(5), Duration::from_millis(10)))
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    repeating.pause().expect("暫停");
    settle().await;
    let before = handle.sent().len();
    tokio::time::advance(Duration::from_millis(50)).await;
    settle().await;
    assert_eq!(handle.sent().len(), before);
    repeating.resume().expect("恢復");
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    assert!(handle.sent().len() > before);
    drop(repeating);
    settle().await;
    let stopped = handle.sent().len();
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(handle.sent().len(), stopped);
}

#[tokio::test(start_paused = true)]
async fn detached_item_keeps_running() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(
            CyclicConfig::new(frame(7), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Burst),
        )
        .expect("排程");
    let _id = cyclic.detach();
    settle().await;
    tokio::time::advance(Duration::from_millis(30)).await;
    settle().await;
    assert!(handle.sent().len() >= 3);
}

#[tokio::test(start_paused = true)]
async fn disconnected_ticks_skip_and_priority_orders_same_tick() {
    let (factory, handle) = FakeFactory::new(
        FakeTransportBuilder::default().open_fails(1, pcan_core::FaultKind::Fatal),
    );
    let mut policy = pcan_link::BackoffPolicy::default();
    policy.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(policy)
        .health_check_interval(None)
        .build();
    settle().await;
    let disconnected = link
        .schedule_cyclic(CyclicConfig::new(frame(9), Duration::from_millis(10)))
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(20)).await;
    settle().await;
    assert!(handle.sent().is_empty());
    assert!(disconnected.stats().skipped >= 1);
    tokio::time::advance(Duration::from_millis(80)).await;
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    assert!(!handle.sent().is_empty());
    drop(disconnected);
    settle().await;
    handle.clear_sent();

    let low = link
        .schedule_cyclic(CyclicConfig::new(frame(2), Duration::from_millis(10)).with_priority(200))
        .expect("排程");
    let high = link
        .schedule_cyclic(CyclicConfig::new(frame(1), Duration::from_millis(10)).with_priority(1))
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    let sent = handle.sent();
    assert_eq!(sent[0].data(), &[1]);
    assert_eq!(sent[1].data(), &[2]);
    drop((low, high));
}

#[tokio::test(start_paused = true)]
async fn skip_and_burst_have_distinct_overrun_behavior() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let skip = link
        .schedule_cyclic(
            CyclicConfig::new(frame(1), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Skip),
        )
        .expect("排程");
    let burst = link
        .schedule_cyclic(
            CyclicConfig::new(frame(2), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Burst),
        )
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(skip.stats().sent, 1);
    assert!(skip.stats().skipped >= 9);
    assert_eq!(burst.stats().sent, 10);
    assert_eq!(handle.sent().len(), 11);
}

#[tokio::test(start_paused = true)]
async fn one_second_burst_preserves_one_hundred_absolute_phase_ticks() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(
            CyclicConfig::new(frame(3), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Burst),
        )
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(cyclic.stats().sent, 100);
    assert_eq!(handle.sent().len(), 100);
}

#[tokio::test(start_paused = true)]
async fn drop_stop_survives_more_than_old_control_capacity() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .tx_queue_capacity(256)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(CyclicConfig::new(frame(4), Duration::from_millis(10)))
        .expect("排程");
    for _ in 0..128 {
        let _result = cyclic.trigger_once();
    }
    drop(cyclic);
    for _ in 0..256 {
        tokio::task::yield_now().await;
    }
    let stopped_at = handle.sent().len();
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(handle.sent().len(), stopped_at);
}

#[tokio::test(start_paused = true)]
async fn full_tx_queue_counts_skip_and_emits_drop_event() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .tx_queue_capacity(1)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let mut events = link.events();
    let cyclic = link
        .schedule_cyclic(
            CyclicConfig::new(frame(6), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Burst),
        )
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert!(cyclic.stats().skipped > 0);
    let mut saw_drop = false;
    while let Ok(event) = events.try_recv() {
        if matches!(event, BusEvent::TxDropped { .. }) {
            saw_drop = true;
            break;
        }
    }
    assert!(saw_drop);
}
