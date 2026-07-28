#![cfg(all(target_os = "linux", feature = "vcan-tests"))]

//! 透過 Linux vcan 驗證 `Link` 的端到端整合行為。
//!
//! 執行前需建立 vcan 介面，並以 `PCAN_VCAN` 武裝測試。接收端使用同一介面的
//! 第二個 socket，因為兩個獨立的 vcan netdev 不會自行互通。`PCAN_VCAN_REQUIRED`
//! 適合 CI 使用，可防止介面準備失敗時靜默跳過。
//!
//! 重連測試另以 `PCAN_VCAN_ADMIN` 武裝，並用
//! `sudo -n ip link set <iface> up/down` 操作專屬介面。該介面不得與
//! `PCAN_VCAN` 共用。

use core::time::Duration;
use std::process::Command;

use pcan_kit::{
    BackoffPolicy, CanId, CanSocket, CyclicConfig, FilterRule, FilterSet, Frame, Link, LinkState,
    Matcher, PrefixPattern, ResponseSpec, RxFrame, SocketCanConfig, SocketCanFactory, Subscription,
    TransactionError, Transport, TransportFactory, open,
};

const LINK_END_TO_END_ID: u16 = 0x210;
const LINK_REQUEST_ID: u16 = 0x220;
const LINK_RESPONSE_ID: u16 = 0x221;
const LINK_CYCLIC_ID: u16 = 0x230;
const ADMIN_RECONNECT_BASE: u16 = 0x2a0;

const RECV_TIMEOUT: Duration = Duration::from_secs(2);
const STATE_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_DEADLINE: Duration = Duration::from_secs(3);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// 取得測試用的 vcan 介面名稱；未武裝時回傳 `None` 讓呼叫端提早結束。
fn vcan() -> Option<String> {
    match std::env::var("PCAN_VCAN") {
        Ok(name) if !name.is_empty() => Some(name),
        _ => {
            assert!(
                std::env::var("PCAN_VCAN_REQUIRED").is_err(),
                "PCAN_VCAN_REQUIRED 已設定但 PCAN_VCAN 沒有值：vcan 準備步驟失敗了"
            );
            eprintln!("跳過：未設定 PCAN_VCAN（本機沒有 vcan 介面）");
            None
        }
    }
}

fn vcan_admin() -> Option<String> {
    match std::env::var("PCAN_VCAN_ADMIN") {
        Ok(name) if !name.is_empty() => {
            assert!(
                !matches!(
                    std::env::var("PCAN_VCAN"),
                    Ok(primary) if !primary.is_empty() && primary == name
                ),
                "破壞性測試必須使用專屬獨立介面（例如 vcan9），不可與一般 vcan 測試共用介面"
            );
            Some(name)
        }
        _ => {
            eprintln!("跳過：未設定 PCAN_VCAN_ADMIN（未武裝管理員 vcan 測試）");
            None
        }
    }
}

fn set_interface_state(interface: &str, state: &str) {
    let output = Command::new("sudo")
        .args(["-n", "ip", "link", "set", interface, state])
        .output()
        .unwrap_or_else(|error| {
            panic!("無法執行 `sudo -n ip link set {interface} {state}`：{error}")
        });
    assert!(
        output.status.success(),
        "`sudo -n ip link set {interface} {state}` 失敗（status={}）：stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct AdminInterface {
    name: String,
}

impl AdminInterface {
    fn arm(name: String) -> Self {
        set_interface_state(&name, "up");
        Self { name }
    }

    fn down(&self) {
        set_interface_state(&self.name, "down");
    }

    fn up(&self) {
        set_interface_state(&self.name, "up");
    }
}

impl Drop for AdminInterface {
    fn drop(&mut self) {
        // 測試 panic 時仍盡力恢復介面；正常路徑另外用 up() 檢查回復結果。
        match Command::new("sudo")
            .args(["-n", "ip", "link", "set", &self.name, "up"])
            .output()
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => eprintln!(
                "警告：Drop 恢復管理員 vcan 介面 `{}` 為 up 失敗（status={}）：stderr={}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(error) => eprintln!(
                "警告：Drop 無法執行 sudo 恢復管理員 vcan 介面 `{}` 為 up：{error}",
                self.name
            ),
        }
    }
}

fn standard_id(raw: u16) -> CanId {
    CanId::standard(raw).expect("測試使用的標準 CAN ID 必須有效")
}

fn exact_filter(id: CanId) -> FilterSet {
    FilterSet::with(FilterRule::exact(id))
}

async fn open_link(interface: &str) -> Link {
    let uri = format!("socketcan://{interface}");
    let link = open(&uri)
        .await
        .unwrap_or_else(|error| panic!("建立測試 Link `{uri}` 失敗：{error}"));
    match tokio::time::timeout(TEST_DEADLINE, link.wait_connected()).await {
        Err(elapsed) => panic!("{TEST_DEADLINE:?} 內 Link `{uri}` 未連線：{elapsed}"),
        Ok(Err(error)) => panic!("等待 Link `{uri}` 連線失敗：{error}"),
        Ok(Ok(())) => link,
    }
}

async fn send_until_received(
    sender: &Link,
    frame: Frame,
    subscription: &mut Subscription,
) -> RxFrame {
    let deadline = tokio::time::Instant::now() + TEST_DEADLINE;
    loop {
        sender
            .send_await(frame)
            .await
            .unwrap_or_else(|error| panic!("透過 Link 實際送出測試幀失敗：{error}"));
        match tokio::time::timeout(RETRY_INTERVAL, subscription.recv()).await {
            Ok(Some(rx)) => return rx,
            Ok(None) => panic!("等待測試幀期間訂閱意外關閉"),
            Err(_) if tokio::time::Instant::now() < deadline => {}
            Err(elapsed) => {
                panic!("{TEST_DEADLINE:?} 內未透過 Link 收到測試幀：{elapsed}")
            }
        }
    }
}

async fn open_socket(interface: &str) -> CanSocket {
    let factory = SocketCanFactory::new(SocketCanConfig::new(interface))
        .expect("有效的管理員 vcan 介面名稱應建立工廠");
    factory.open().await.unwrap_or_else(|error| {
        panic!("開啟管理員測試介面 `{interface}` 的 SocketCAN 失敗：{error:?}")
    })
}

async fn wait_until_not_connected(link: &Link) -> LinkState {
    let mut state = link.state_watch();
    match tokio::time::timeout(STATE_TIMEOUT, async {
        loop {
            let current = *state.borrow_and_update();
            if current != LinkState::Connected {
                return current;
            }
            state
                .changed()
                .await
                .expect("Link 狀態 watch 不應在監督器存活時關閉");
        }
    })
    .await
    {
        Ok(state) => state,
        Err(error) => panic!("{STATE_TIMEOUT:?} 內 LinkState 未離開 Connected：{error}"),
    }
}

#[tokio::test]
async fn link_end_to_end_send_and_subscribe() {
    let Some(interface) = vcan() else {
        return;
    };
    let id = standard_id(LINK_END_TO_END_ID);

    let receiver = open_link(&interface).await;
    let sender = open_link(&interface).await;
    let mut subscription = receiver
        .subscribe_filter(exact_filter(id))
        .await
        .expect("建立 Link 接收端訂閱失敗");
    let frame = Frame::new(id, &[0x11, 0x22, 0x33, 0x44]).expect("建立端到端測試幀失敗");

    let rx = send_until_received(&sender, frame, &mut subscription).await;
    assert_eq!(rx.frame, frame, "Link 端到端收到的幀內容不符");
    assert!(!rx.is_echo, "peer Link 收到的幀不應標記為本地回音");

    sender.close().await;
    receiver.close().await;
}

#[tokio::test]
async fn link_request_transaction_over_vcan() {
    let Some(interface) = vcan() else {
        return;
    };
    let request_id = standard_id(LINK_REQUEST_ID);
    let response_id = standard_id(LINK_RESPONSE_ID);
    let request_frame = Frame::new(request_id, &[0x22, 0xf1, 0x90]).expect("建立交易請求幀失敗");
    let response_frame = Frame::new(response_id, &[0x62, 0xf1, 0x90, 0x50, 0x43, 0x41, 0x4e])
        .expect("建立交易回應幀失敗");

    let responder = open_link(&interface).await;
    let requester = open_link(&interface).await;
    let mut requests = responder
        .subscribe_filter(exact_filter(request_id))
        .await
        .expect("建立交易 responder 訂閱失敗");
    let responder_link = responder.clone();
    let responder_task = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + TEST_DEADLINE;
        loop {
            match tokio::time::timeout(RETRY_INTERVAL, requests.recv()).await {
                Ok(Some(rx)) => {
                    assert_eq!(rx.frame, request_frame, "responder 收到的交易請求內容不符");
                    responder_link
                        .send_await(response_frame)
                        .await
                        .unwrap_or_else(|error| panic!("responder 送出交易回應失敗：{error}"));
                }
                Ok(None) => panic!("等待交易請求期間 responder 訂閱意外關閉"),
                Err(_) if tokio::time::Instant::now() < deadline => {}
                Err(_) => return,
            }
        }
    });

    let prefix = PrefixPattern::new(&[0x62, 0xf1, 0x90]).expect("建立交易回應前綴失敗");
    let spec = ResponseSpec::new(
        Matcher::IdAndPrefix {
            id: response_id,
            prefix,
        },
        RETRY_INTERVAL,
    );
    let deadline = tokio::time::Instant::now() + TEST_DEADLINE;
    let response = loop {
        match requester.request(request_frame, &spec).await {
            Ok(rx) => break rx,
            Err(TransactionError::Timeout { .. }) if tokio::time::Instant::now() < deadline => {}
            Err(error) => panic!("Link 交易請求失敗：{error}"),
        }
    };

    assert_eq!(
        response.frame, response_frame,
        "Link 交易收到的回應內容不符"
    );

    responder_task.abort();
    let _cancelled = responder_task.await;
    requester.close().await;
    responder.close().await;
}

#[tokio::test]
async fn link_cyclic_sends_at_the_configured_period() {
    let Some(interface) = vcan() else {
        return;
    };
    let id = standard_id(LINK_CYCLIC_ID);

    let receiver = open_link(&interface).await;
    let sender = open_link(&interface).await;
    let mut subscription = receiver
        .subscribe_filter(exact_filter(id))
        .await
        .expect("建立週期幀接收訂閱失敗");
    let frame = Frame::new(id, &[0xca, 0xfe]).expect("建立週期測試幀失敗");
    let cyclic = sender
        .schedule_cyclic(CyclicConfig::new(frame, Duration::from_millis(20)))
        .expect("註冊 20ms 週期傳送失敗");

    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    let mut frame_count = 0_u32;
    loop {
        match tokio::time::timeout_at(deadline, subscription.recv()).await {
            Ok(Some(rx)) => {
                assert_eq!(rx.frame, frame, "收到的週期幀內容不符");
                frame_count = frame_count.saturating_add(1);
            }
            Ok(None) => panic!("統計週期幀期間訂閱意外關閉"),
            Err(_) => break,
        }
    }

    assert!(
        (10..=20).contains(&frame_count),
        "300ms 內預期收到 10..=20 個 20ms 週期幀，實際收到 {frame_count} 個"
    );
    cyclic.stop().await.expect("停止週期傳送失敗");
    sender.close().await;
    receiver.close().await;
}

#[tokio::test]
async fn link_reconnects_after_interface_returns() {
    let Some(interface) = vcan_admin() else {
        return;
    };
    let admin = AdminInterface::arm(interface.clone());
    let factory = SocketCanFactory::new(SocketCanConfig::new(interface.as_str()))
        .expect("有效的管理員 vcan 介面名稱應建立工廠");
    let mut backoff = BackoffPolicy::default();
    backoff.initial = Duration::from_millis(50);
    backoff.max = Duration::from_millis(100);
    backoff.jitter_ratio = 0.0;
    let link = Link::builder(factory)
        .backoff(backoff)
        .health_check_interval(None)
        .build();
    tokio::time::timeout(STATE_TIMEOUT, link.wait_connected())
        .await
        .expect("初次連線不應超過 5 秒")
        .expect("初次連線管理員 vcan 應成功");

    let bait_id = standard_id(ADMIN_RECONNECT_BASE);
    let sentinel_id = standard_id(ADMIN_RECONNECT_BASE + 1);
    link.set_hardware_filter(FilterSet::with(FilterRule::exact(sentinel_id)))
        .await
        .expect("連線時套用核心精確過濾器應成功");
    let mut subscription = link
        .subscribe_filter(FilterSet::accept_all())
        .await
        .expect("建立重連後驗證訂閱應成功");

    admin.down();
    let probe =
        Frame::new(standard_id(ADMIN_RECONNECT_BASE + 2), &[0x21]).expect("斷線探測幀應合法");
    link.send(probe)
        .await
        .expect("斷線探測幀應先進入 Link bounded 佇列");
    let disconnected_state = wait_until_not_connected(&link).await;
    assert_ne!(disconnected_state, LinkState::Closed);

    admin.up();
    tokio::time::timeout(STATE_TIMEOUT, link.wait_connected())
        .await
        .expect("介面恢復後重連不應超過 5 秒")
        .expect("介面恢復後 Link 應重新連線");

    // 以同一專屬 vcan 上的第二個 socket 送入「誘餌、sentinel」；若監督器
    // 沒有在 open 後重放保存的硬體過濾器，訂閱首先會收到誘餌。
    let sender = open_socket(&interface).await;
    let bait = Frame::new(bait_id, &[0xba]).expect("重連過濾器誘餌幀應合法");
    let sentinel = Frame::new(sentinel_id, &[0x5a]).expect("重連過濾器 sentinel 應合法");
    sender.send(&bait).await.expect("送出重連誘餌幀應成功");
    sender
        .send(&sentinel)
        .await
        .expect("送出重連 sentinel 幀應成功");

    let received = match tokio::time::timeout(RECV_TIMEOUT, subscription.recv()).await {
        Ok(Some(frame)) => frame,
        Ok(None) => panic!("Link 訂閱在等待重連 sentinel 時提前關閉"),
        Err(error) => panic!("{RECV_TIMEOUT:?} 內未收到重連 sentinel：{error}"),
    };
    assert_eq!(
        received.frame.id(),
        sentinel_id,
        "重連後未重放硬體過濾器，先收到被排除的誘餌"
    );
    assert!(matches!(link.state(), LinkState::Connected));

    link.close().await;
    admin.up();
}
