//! 推送式訂閱路由整合測試。

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{
    CanId, FilterRule, FilterSet, Frame, RxFrame, Timestamp, TimestampSource, TransportEvent,
};
use pcan_link::{Link, OverflowPolicy, SubscribeConfig};
use tokio::sync::mpsc::error::TryRecvError;

fn rx(raw: u16, value: u8, echo: bool) -> RxFrame {
    let id = CanId::standard(raw).expect("有效 ID");
    let frame = Frame::new(id, &[value]).expect("有效幀");
    RxFrame::new(frame, Timestamp::new(0, TimestampSource::Software), echo)
}

async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn filters_before_push_and_keeps_subscribers_independent() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let a = CanId::standard(0x100).expect("ID");
    let b = CanId::standard(0x200).expect("ID");
    let mut first = link
        .subscribe_filter(FilterSet::with(FilterRule::exact(a)))
        .await
        .expect("訂閱");
    let mut second = link
        .subscribe_filter(FilterSet::with(FilterRule::exact(b)))
        .await
        .expect("訂閱");
    handle.inject(TransportEvent::Frame(rx(0x100, 1, false)));
    settle().await;
    assert_eq!(first.try_recv().expect("第一個訂閱收到").frame.id(), a);
    assert!(matches!(second.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test(start_paused = true)]
async fn drop_oldest_is_real_and_echo_is_filtered() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let config = SubscribeConfig::default()
        .with_capacity(1)
        .with_overflow(OverflowPolicy::DropOldest);
    let mut subscription = link.subscribe(config).await.expect("訂閱");
    handle.inject(TransportEvent::Frame(rx(0x100, 9, true)));
    handle.inject(TransportEvent::Frame(rx(0x100, 1, false)));
    handle.inject(TransportEvent::Frame(rx(0x100, 2, false)));
    settle().await;
    assert_eq!(
        subscription.try_recv().expect("保留最新").frame.data(),
        &[2]
    );
    assert_eq!(subscription.dropped(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_one_subscription_does_not_affect_another() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let first = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("訂閱");
    let mut second = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("訂閱");
    drop(first);
    settle().await;
    handle.inject(TransportEvent::Frame(rx(0x321, 3, false)));
    settle().await;
    assert_eq!(second.recv().await.expect("仍可接收").frame.data(), &[3]);
}
