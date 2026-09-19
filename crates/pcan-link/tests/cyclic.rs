//! 單一排程器週期傳送整合測試。

use core::num::NonZeroU32;
use core::time::Duration;
use std::sync::Arc;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_core::{CanId, Frame};
use pcan_link::{
    BusEvent, CyclicConfig, Error, Link, MAX_PENDING_CYCLIC_ADDS, OverrunPolicy, Repeat,
};
use tokio::sync::broadcast;

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
                .with_overrun(OverrunPolicy::Burst)
                // 本測試驗證的是絕對相位補送本身，必須放寬預設的補送上限；
                // 上限行為另由下方的測試覆蓋。
                .with_max_burst(NonZeroU32::new(128).expect("128 不為零")),
        )
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(cyclic.stats().sent, 100);
    assert_eq!(handle.sent().len(), 100);
}

/// 補送上限必須為單一 tick 的工作量設下確定性上界。
///
/// 沒有上限時，停頓一秒的 10 ms 項目會在一次 tick 裡跑滿 100 次補送迴圈，
/// 期間所有控制命令都無法被處理；週期越短情況越誇張。
#[tokio::test(start_paused = true)]
async fn burst_is_bounded_by_max_burst_and_counts_the_rest_as_skipped() {
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(
            CyclicConfig::new(frame(4), Duration::from_millis(10))
                .with_overrun(OverrunPolicy::Burst)
                .with_max_burst(NonZeroU32::new(8).expect("8 不為零")),
        )
        .expect("排程");
    settle().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;

    assert_eq!(cyclic.stats().sent, 8, "補送次數必須被 max_burst 截斷");
    assert_eq!(handle.sent().len(), 8, "超出上限的 tick 不得送上匯流排");
    assert_eq!(
        cyclic.stats().skipped,
        92,
        "被上限擋下的 tick 必須計入 skipped，而不是無聲消失"
    );
}

/// 高速連續更新只需讓排程器看到最後一個值。
///
/// 合併更新槽的可觀察契約：中間值可以被覆蓋，但最後一次呼叫的值必須生效。
#[tokio::test(start_paused = true)]
async fn rapid_payload_updates_coalesce_to_the_latest_value() {
    const UPDATES: u8 = 200;

    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(CyclicConfig::new(frame(0), Duration::from_millis(10)))
        .expect("排程");

    // 不讓排程器插進來：整批更新都在同一個同步區段內完成，確保真的踩到
    // 「排隊中的命令尚未被處理」的合併路徑。
    for value in 1..=UPDATES {
        cyclic.set_payload(&[value]).expect("更新酬載");
    }
    settle().await;
    tokio::time::advance(Duration::from_millis(10)).await;
    settle().await;

    let sent = handle.sent();
    assert_eq!(sent.len(), 1, "一個週期只應送出一幀");
    assert_eq!(sent[0].data(), &[UPDATES], "合併後必須套用最後一次更新的值");
}

/// 未處理的新增命令必須有確定性上界，名額並在排程器取走後歸還。
///
/// 其他控制命令都能合併或計數，新增不行：每一筆都帶著各自的設定與共享槽，
/// 必須原樣送達。因此改以固定名額准入，這是控制平面最後一條可能無限成長的
/// 路徑。
///
/// 本測試在 current-thread runtime 上建立 `Link` 之後完全不 await，排程器
/// task 因而一次都沒有執行，所有新增命令都留在控制通道裡——上界是否成立可以
/// 被確定性地斷言，而不是靠時序碰運氣。
#[tokio::test(start_paused = true)]
async fn pending_cyclic_adds_are_bounded_and_slots_are_returned() {
    let (factory, _handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    let config = CyclicConfig::new(frame(8), Duration::from_secs(3600));

    // 刻意不 await：排程器還沒有機會取走任何一筆新增命令。
    let mut handles = Vec::with_capacity(MAX_PENDING_CYCLIC_ADDS);
    for index in 0..MAX_PENDING_CYCLIC_ADDS {
        match link.schedule_cyclic(config) {
            Ok(handle) => handles.push(handle),
            Err(error) => panic!("第 {index} 筆新增在名額用盡前就失敗：{error:?}"),
        }
    }

    match link.schedule_cyclic(config) {
        Err(Error::ControlQueueFull { capacity }) => {
            assert_eq!(capacity, MAX_PENDING_CYCLIC_ADDS, "錯誤必須帶回實際上界");
        }
        Err(other) => panic!("名額用盡時應回 ControlQueueFull，實際：{other:?}"),
        Ok(_) => panic!("名額已用盡，第 {} 筆不得成功", MAX_PENDING_CYCLIC_ADDS + 1),
    }

    // 讓排程器取走待處理的新增命令，名額隨命令被丟棄而歸還。
    settle().await;
    let recovered = link
        .schedule_cyclic(config)
        .expect("排程器消化之後名額必須重新可用");
    handles.push(recovered);

    // 名額歸還的是「未處理命令」的額度，不是「存活項目」的額度：上面 257 個
    // 項目全部仍在排程器裡。
    assert_eq!(handles.len(), MAX_PENDING_CYCLIC_ADDS + 1);
}

/// 高頻同步控制呼叫必須合併，而不是在控制通道裡無限堆積。
///
/// 控制通道不能改成 bounded：`Drop` 也要用它送停止命令，而 `Drop` 不能
/// `.await`。上界因此只能來自「每個項目最多一個未處理命令」的合併設計本身。
///
/// 本測試在 current-thread runtime 上做二十萬輪、共一百萬次控制呼叫，中途完全
/// 不讓排程器有機會執行。若任何一個控制方法仍是「一次呼叫排一個命令」，佇列
/// 就會累積到一百萬筆；合併之後，最終只剩一個命令、一份最新狀態，以及受
/// `max_burst` 封頂的觸發計數。
#[tokio::test(start_paused = true)]
async fn rapid_control_calls_coalesce_instead_of_growing_the_queue() {
    const ROUNDS: u32 = 200_000;
    let trigger_cap = NonZeroU32::new(8).expect("非零");

    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .tx_queue_capacity(256)
        .health_check_interval(None)
        .build();
    link.wait_connected().await.expect("連線");
    let cyclic = link
        .schedule_cyclic(
            CyclicConfig::new(frame(0), Duration::from_secs(3600)).with_max_burst(trigger_cap),
        )
        .expect("排程");
    settle().await;

    for round in 0..ROUNDS {
        let payload = u8::try_from(round % 251).expect("餘數必定可表示為 u8");
        cyclic.trigger_once().expect("觸發");
        cyclic.set_payload(&[payload]).expect("更新酬載");
        cyclic
            .set_period(Duration::from_secs(3600 + u64::from(round % 97)))
            .expect("改週期");
        cyclic.resume().expect("恢復");
        // 每輪以暫停收尾：合併後的最終狀態是暫停，排程器因而不會在斷言前
        // 因為時間自動前進而多送出週期幀，觸發送出的數量才能精確斷言。
        cyclic.pause().expect("暫停");
    }
    settle().await;

    let sent = handle.sent();
    assert_eq!(
        sent.len(),
        trigger_cap.get() as usize,
        "未處理觸發必須封頂於 max_burst，實際送出 {} 幀",
        sent.len()
    );
    let last_payload = u8::try_from((ROUNDS - 1) % 251).expect("餘數必定可表示為 u8");
    for frame in &sent {
        assert_eq!(
            frame.data(),
            &[last_payload],
            "觸發必須送出合併後的最新酬載"
        );
    }

    let stats = cyclic.stats();
    assert_eq!(stats.sent, u64::from(trigger_cap.get()));
    assert_eq!(
        stats.skipped,
        u64::from(ROUNDS) - u64::from(trigger_cap.get()),
        "超出上限的觸發必須全部計入 skipped，不得靜默消失"
    );

    // 排空之後仍必須正常運作：合併是流量控制，不是一次性的關閉。
    cyclic.resume().expect("恢復");
    cyclic.trigger_once().expect("觸發");
    settle().await;
    assert_eq!(handle.sent().len(), sent.len() + 1);
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
    assert!(link.stats().tx_queue_full > 0);
    let mut saw_drop = false;
    while let Ok(event) = events.try_recv() {
        if matches!(event, BusEvent::TxDropped { .. }) {
            saw_drop = true;
            break;
        }
    }
    assert!(saw_drop);
}

/// 驗證並行更新幀與酬載不會使週期排程器異常結束。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_frame_and_payload_updates_keep_scheduler_alive() {
    const ITERATIONS: usize = 10_000;

    let (factory, _) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).health_check_interval(None).build();
    link.wait_connected().await.expect("連線");
    let mut events = link.events();
    let id = CanId::standard(0x123).expect("ID");
    let cyclic = Arc::new(
        link.schedule_cyclic(CyclicConfig::new(
            Frame::new(id, &[0; 8]).expect("初始幀"),
            Duration::from_secs(60),
        ))
        .expect("排程"),
    );

    let payload_handle = Arc::clone(&cyclic);
    let payload_task = tokio::spawn(async move {
        for index in 0..ITERATIONS {
            let _result = if index % 2 == 0 {
                payload_handle.set_payload(&[1; 2])
            } else {
                payload_handle.set_payload(&[2; 8])
            };
            tokio::task::yield_now().await;
        }
    });
    let frame_handle = Arc::clone(&cyclic);
    let frame_task = tokio::spawn(async move {
        for index in 0..ITERATIONS {
            let next = if index == ITERATIONS / 2 {
                Frame::remote(id, 8).expect("遠端幀")
            } else if index % 2 == 0 {
                Frame::new(id, &[3; 2]).expect("兩位元組幀")
            } else {
                Frame::new(id, &[4; 8]).expect("八位元組幀")
            };
            let _result = frame_handle.set_frame(next);
            tokio::task::yield_now().await;
        }
    });

    payload_task.await.expect("酬載更新 task");
    frame_task.await.expect("幀更新 task");
    Arc::try_unwrap(cyclic)
        .expect("並行 task 應已釋放控制代碼")
        .stop()
        .await
        .expect("排程器應處理完所有更新並確認停止");

    let probe = link
        .schedule_cyclic(CyclicConfig::new(
            Frame::new(id, &[5; 2]).expect("探測幀"),
            Duration::from_secs(60),
        ))
        .expect("排程器應仍可接受新項目");
    probe.stop().await.expect("停止探測項目");

    loop {
        match events.try_recv() {
            Ok(event) => {
                assert!(
                    !matches!(event, BusEvent::WorkerLost { worker: "cyclic" }),
                    "週期排程器不得因並行更新而異常結束"
                );
            }
            // `Lagged` 不可中止排空，否則「斷言事件不存在」會在溢位時靜默放行。
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
        }
    }
}
