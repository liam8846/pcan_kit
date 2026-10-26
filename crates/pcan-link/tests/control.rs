//! 控制名額覆蓋待建立、使用中與待清理階段，取消操作不能提前歸還名額。

use core::future::Future;
use core::num::NonZeroU32;
use core::pin::Pin;
use core::task::{Context, Waker};
use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{CanId, Frame, RxFrame, Timestamp, TimestampSource, TransportEvent};
use pcan_link::{
    CyclicConfig, Error, Link, Matcher, OverflowPolicy, Repeat, ResponseSpec, SubscribeConfig,
    TransactionError,
};

fn frame() -> Frame {
    Frame::new(CanId::standard(0x123).expect("ID"), &[1]).expect("幀")
}

fn spec() -> ResponseSpec {
    ResponseSpec::new(Matcher::Id(frame().id()), Duration::from_secs(1))
}

fn assert_pending<F: Future>(future: Pin<&mut F>) {
    assert!(
        future
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_subscriptions_keep_capacity_until_commands_are_drained() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(4)
        .health_check_interval(None)
        .build();
    // 尚未讓監督器執行，模擬背景工作無法消化控制命令。
    let mut requests = Vec::new();
    for _ in 0..4 {
        let mut pending = Box::pin(link.subscribe(SubscribeConfig::default()));
        assert_pending(pending.as_mut());
        requests.push(pending);
    }
    assert!(matches!(
        link.subscribe(SubscribeConfig::default()).await,
        Err(Error::ControlQueueFull { capacity: 4 })
    ));
    drop(requests);
    assert!(matches!(
        link.subscribe(SubscribeConfig::default()).await,
        Err(Error::ControlQueueFull { capacity: 4 })
    ));
    link.wait_connected().await.expect("連線");
    settle().await;
    let subscription = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("取消命令已回收");
    drop(subscription);
    link.close().await;
}

#[tokio::test(start_paused = true)]
async fn cancelled_transactions_keep_capacity_until_commands_are_drained() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(1)
        .health_check_interval(None)
        .build();
    let response = spec();
    let mut pending = Box::pin(link.prepare(&response));
    assert_pending(pending.as_mut());
    assert!(matches!(
        link.prepare(&response).await,
        Err(TransactionError::ControlQueueFull { capacity: 1 })
    ));
    drop(pending);
    assert!(matches!(
        link.prepare(&response).await,
        Err(TransactionError::ControlQueueFull { capacity: 1 })
    ));
    link.wait_connected().await.expect("連線");
    settle().await;
    let response = link.prepare(&response).await.expect("取消命令已回收");
    drop(response);
    link.close().await;
}

#[tokio::test(start_paused = true)]
async fn budgets_are_independent_and_cleanup_does_not_need_free_capacity() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(1)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let subscription = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("訂閱名額");
    let pending = link.prepare(&spec()).await.expect("交易有獨立名額");
    let cyclic = link
        .schedule_cyclic(CyclicConfig::new(frame(), Duration::from_secs(1)))
        .expect("週期有獨立名額");
    assert!(matches!(
        link.subscribe(SubscribeConfig::default()).await,
        Err(Error::ControlQueueFull { capacity: 1 })
    ));
    assert!(matches!(
        link.prepare(&spec()).await,
        Err(TransactionError::ControlQueueFull { capacity: 1 })
    ));
    drop(subscription);
    drop(pending);
    drop(cyclic);
    // Drop 已送清理命令，但背景尚未處理，不能先歸還名額。
    assert!(matches!(
        link.schedule_cyclic(CyclicConfig::new(frame(), Duration::from_secs(1))),
        Err(Error::ControlQueueFull { capacity: 1 })
    ));
    settle().await;
    let _subscription = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("訂閱清理完成");
    let _pending = link.prepare(&spec()).await.expect("交易清理完成");
    let _cyclic = link
        .schedule_cyclic(CyclicConfig::new(frame(), Duration::from_secs(1)))
        .expect("週期清理完成");
    link.close().await;
    assert_eq!(link.in_flight_count(), 0, "關閉應同步清空交易數");
    assert!(matches!(
        link.subscribe(SubscribeConfig::default()).await,
        Err(Error::Closed)
    ));
    assert!(matches!(
        link.prepare(&spec()).await,
        Err(TransactionError::Closed)
    ));
    assert!(matches!(
        link.schedule_cyclic(CyclicConfig::new(frame(), Duration::from_secs(1))),
        Err(Error::Closed)
    ));
}

#[tokio::test(start_paused = true)]
async fn overflow_closed_subscription_still_reserves_its_cleanup_capacity() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(1)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let mut subscription = link
        .subscribe(
            SubscribeConfig::default()
                .with_capacity(1)
                .with_overflow(OverflowPolicy::Close),
        )
        .await
        .expect("訂閱");
    for _ in 0..2 {
        handle.inject(TransportEvent::Frame(RxFrame::new(
            frame(),
            Timestamp::new(0, TimestampSource::Software),
            false,
        )));
    }
    settle().await;
    assert!(subscription.try_recv().is_ok(), "溢位前的幀仍可讀取");
    assert!(
        matches!(
            subscription.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
        ),
        "必須確實觸發溢位關閉"
    );
    assert!(matches!(
        link.subscribe(SubscribeConfig::default()).await,
        Err(Error::ControlQueueFull { capacity: 1 })
    ));
    drop(subscription);
    settle().await;
    let _subscription = link
        .subscribe(SubscribeConfig::default())
        .await
        .expect("清理後可重用");
    link.close().await;
}

#[tokio::test(start_paused = true)]
async fn detached_cyclic_reserves_capacity_until_its_finite_schedule_finishes() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(1)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let config = CyclicConfig::new(frame(), Duration::from_millis(10))
        .with_repeat(Repeat::Count(NonZeroU32::new(1).expect("非零")));
    let _id = link.schedule_cyclic(config).expect("排程").detach();
    settle().await;
    assert!(matches!(
        link.schedule_cyclic(config),
        Err(Error::ControlQueueFull { capacity: 1 })
    ));
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    let cyclic = link.schedule_cyclic(config).expect("結束後回收名額");
    cyclic.stop().await.expect("停止仍可在名額用盡時執行");
    link.close().await;
}

#[tokio::test(start_paused = true)]
async fn finished_cyclic_handle_and_stale_commands_keep_their_capacity() {
    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .control_capacity(1)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let config = CyclicConfig::new(frame(), Duration::from_millis(10))
        .with_repeat(Repeat::Count(NonZeroU32::new(1).expect("非零")));
    let cyclic = link.schedule_cyclic(config).expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;
    assert_eq!(cyclic.stats().sent, 1, "有限次排程已完成");
    assert!(matches!(
        link.schedule_cyclic(config),
        Err(Error::ControlQueueFull { capacity: 1 })
    ));
    cyclic.set_payload(&[2]).expect("舊控制代碼仍可排合併更新");
    drop(cyclic);
    assert!(
        matches!(
            link.schedule_cyclic(config),
            Err(Error::ControlQueueFull { capacity: 1 })
        ),
        "待清理的陳舊更新與停止命令仍占名額"
    );
    settle().await;
    let cyclic = link.schedule_cyclic(config).expect("殘留命令已釋放");
    cyclic.stop().await.expect("停止");
    link.close().await;
}
