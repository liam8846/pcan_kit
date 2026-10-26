//! 傳送佇列水位與主動背壓事件整合測試。

use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{CanId, FaultKind, Frame};
use pcan_link::{BackoffPolicy, BusEvent, Error, Link};

async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

fn frame(value: u8) -> Frame {
    Frame::new(CanId::standard(0x123).expect("ID"), &[value]).expect("幀")
}

#[tokio::test(start_paused = true)]
async fn depth_high_water_full_counter_and_recovery_are_observable() {
    let (factory, _) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Recoverable));
    let mut backoff = BackoffPolicy::default();
    backoff.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(backoff)
        .tx_queue_capacity(8)
        .tx_high_water_ratio(Some(0.5))
        .max_pending_age(Duration::from_secs(60))
        .health_check_interval(None)
        .build();
    let mut events = link.events();
    settle().await;

    for value in 0..8 {
        loop {
            match link.try_send(frame(value)) {
                Ok(()) => break,
                Err(Error::TxQueueFull { .. }) => settle().await,
                Err(error) => panic!("非預期排入錯誤：{error}"),
            }
        }
        settle().await;
    }
    let staged = link.tx_queue_depth();
    assert_eq!(staged.staged, 8);
    assert_eq!(staged.channel, 0);

    for value in 8..16 {
        link.try_send(frame(value)).expect("填滿 channel 段");
    }
    let full = link.tx_queue_depth();
    assert_eq!(full.channel, 8);
    assert_eq!(full.staged, 8);
    assert_eq!(full.total(), 16);
    assert!((full.utilization() - 1.0).abs() < f32::EPSILON);

    let mut high_water_events = 0;
    while let Ok(event) = events.try_recv() {
        if let BusEvent::TxQueueHighWater { queued, capacity } = event {
            high_water_events += 1;
            assert!(queued > capacity / 2);
            assert_eq!(capacity, 8);
        }
    }
    assert_eq!(high_water_events, 1);
    assert!(matches!(
        link.try_send(frame(16)),
        Err(Error::TxQueueFull { capacity: 8 })
    ));
    assert_eq!(link.stats().tx_queue_full, 1);

    tokio::time::advance(Duration::from_millis(100)).await;
    settle().await;
    assert_eq!(link.tx_queue_depth().total(), 0);
    let mut recovered_events = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, BusEvent::TxQueueRecovered { .. }) {
            recovered_events += 1;
        }
    }
    assert_eq!(recovered_events, 1);
}

#[tokio::test(start_paused = true)]
async fn disabled_high_water_does_not_emit_watermark_events() {
    let (factory, _) =
        FakeFactory::new(FakeTransportBuilder::default().open_fails(1, FaultKind::Recoverable));
    let link = Link::builder(factory)
        .tx_queue_capacity(4)
        .tx_high_water_ratio(None)
        .max_pending_age(Duration::from_secs(60))
        .health_check_interval(None)
        .build();
    let mut events = link.events();
    settle().await;
    for value in 0..4 {
        link.try_send(frame(value)).expect("排入");
    }
    let _full = link.try_send(frame(9));
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(
            event,
            BusEvent::TxQueueHighWater { .. } | BusEvent::TxQueueRecovered { .. }
        ));
    }
}
