#![cfg(all(target_os = "linux", feature = "vcan-tests"))]
//! Linux vcan 執行期整合測試。
//!
//! 本機使用方式：
//!
//! ```text
//! sudo modprobe vcan
//! sudo ip link add dev vcan0 type vcan
//! sudo ip link set vcan0 up
//! PCAN_VCAN=vcan0 cargo test -p pcan-socketcan --features vcan-tests \
//!   --test vcan -- --test-threads=1 --nocapture
//! ```
//!
//! `PCAN_VCAN` 是執行期武裝旗標與主要介面名稱。未設定時，測試會印出跳過
//! 訊息並提早結束，讓 `cargo test --workspace --all-features` 可在沒有 vcan 的
//! Linux、Windows 或 WSL2 環境安全執行。CI 應另外設定
//! `PCAN_VCAN_REQUIRED=1`；若準備步驟沒有提供 `PCAN_VCAN`，測試會硬失敗，
//! 避免整個 vcan job 靜默跳過。需要第二個端點的測試會在同一介面上建立第二個
//! socket；兩個獨立 vcan netdev 在未設定 gateway 時不會互通，因此不能用另一個
//! vcan 介面作為 peer。
//!
//! 同一條 vcan 上的 socket 會看見相同流量，因此本 test target 必須以
//! `--test-threads=1` 執行。每項測試使用下方集中配置、互不重疊的 CAN ID
//! 區段，避免殘留佇列或日後新增案例時互相誤收。
//!
//! ## vcan 上測不到的東西
//!
//! | 測不到 | 原因 |
//! |---|---|
//! | 真實 BRS / 資料段位元率切換 | vcan 只是把 skb 直接迴送，沒有任何 bit timing。BRS 旗標會原樣往返，但那只證明 ABI 編解碼正確 |
//! | 錯誤幀（`CAN_ERR_*`） | vcan 永遠不產生錯誤幀。`errframe::parse_error_frame` 與 `TransportEvent::Status` 的**端到端**路徑無法覆蓋，只能靠既有的單元測試 |
//! | Bus-Off / Error-Passive / TEC/REC | vcan 不是 `can` netdev，沒有 `can_priv`，`ip link set vcan0 type can ...` 會直接失敗 |
//! | `restart-ms` 自動復歸 | 同上，vcan 沒有 CAN 狀態機 |
//! | 仲裁、ACK、bus load、位元錯誤 | 沒有實體匯流排 |
//! | `listen_only` | 後端本來就回 `Error::Unsupported`，與 vcan 無關 |
//! | 硬體時間戳 | vcan 只有 `SO_TIMESTAMPNS` 的核心軟體時間戳，`TimestampSource::Hardware` 永遠不會出現 |
//! | TX 佇列滿（`ENOBUFS`）與 `FaultKind::Transient` 重試路徑 | vcan 幾乎不產生 ENOBUFS。硬用 `txqueuelen 1` + 巨量 burst 去逼，結果高度不確定。**不要寫成測試**，這條路徑應繼續用 `FakeTransport` 的注入覆蓋 |
//! | 真實位元率設定 | 對 SocketCAN 後端而言 `Bitrate` 只決定要不要 `CAN_RAW_FD_FRAMES` |

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pcan_core::{
    BackendError, Bitrate, CanId, Error, FaultKind, FilterRule, FilterSet, Frame, FrameFlags,
    RxFrame, TimestampSource, Transport, TransportConfig, TransportEvent, TransportFactory,
};
use pcan_socketcan::{CanSocket, SocketCanConfig, SocketCanFactory};

const RECV_TIMEOUT: Duration = Duration::from_secs(2);
const FD_LENGTHS: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

// 每列配置一項測試的 16-ID 區段，索引依本檔測試表的 1..=16 排列。
const TEST_ID_BASES: [u16; 16] = [
    0x110, 0x120, 0x130, 0x140, 0x150, 0x160, 0x170, 0x180, 0x190, 0x1a0, 0x1b0, 0x1c0, 0x1d0,
    0x1e0, 0x1f0, 0x200,
];

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

fn standard(test: usize, offset: u16) -> CanId {
    let raw = TEST_ID_BASES[test - 1]
        .checked_add(offset)
        .expect("測試 CAN ID 區段加總不應溢位");
    CanId::standard(raw).expect("集中配置的標準 CAN ID 必須落在 11-bit 範圍")
}

fn extended(test: usize, offset: u16) -> CanId {
    let raw = 0x10_000_u32 + u32::from(TEST_ID_BASES[test - 1]) + u32::from(offset);
    CanId::extended(raw).expect("集中配置的擴充 CAN ID 必須落在 29-bit 範圍")
}

fn socket_config(interface: &str, fd: bool, receive_own_frames: bool) -> SocketCanConfig {
    let bitrate = if fd {
        Bitrate::FD_500K_2M
    } else {
        Bitrate::CLASSIC_500K
    };
    let mut config = SocketCanConfig::new(interface);
    config.common = TransportConfig::default()
        .with_bitrate(bitrate)
        .with_receive_own_frames(receive_own_frames);
    config
}

async fn open_socket(interface: &str, fd: bool, receive_own_frames: bool) -> CanSocket {
    let factory = SocketCanFactory::new(socket_config(interface, fd, receive_own_frames))
        .expect("有效的 vcan 介面名稱應建立 SocketCAN 工廠");
    factory
        .open()
        .await
        .unwrap_or_else(|error| panic!("開啟測試用 SocketCAN 介面 `{interface}` 失敗：{error:?}"))
}

async fn open_filtered_socket(interface: &str, fd: bool, filter: FilterSet) -> CanSocket {
    let mut config = socket_config(interface, fd, false);
    config.common = config.common.with_filter(filter);
    let factory = SocketCanFactory::new(config).expect("有效的過濾器設定應建立 SocketCAN 工廠");
    factory
        .open()
        .await
        .unwrap_or_else(|error| panic!("開啟已套用過濾器的 `{interface}` 失敗：{error:?}"))
}

async fn recv_frame(socket: &CanSocket) -> RxFrame {
    match tokio::time::timeout(RECV_TIMEOUT, socket.recv()).await {
        Err(error) => panic!("{RECV_TIMEOUT:?} 內未收到任何傳輸事件：{error}"),
        Ok(Err(error)) => panic!("recv 失敗：{error}"),
        Ok(Ok(TransportEvent::Frame(rx))) => rx,
        Ok(Ok(other)) => panic!("vcan 不應產生非資料事件：{other:?}"),
    }
}

/// 證明誘餌被過濾掉：斷言下一個收到的是 sentinel 而非誘餌。
async fn assert_next_is(socket: &CanSocket, expected: CanId) {
    let rx = recv_frame(socket).await;
    assert_eq!(rx.frame.id(), expected, "過濾器沒有擋掉先送出的誘餌幀");
}

async fn send(socket: &CanSocket, frame: &Frame) {
    socket
        .send(frame)
        .await
        .unwrap_or_else(|error| panic!("送出測試幀 {frame:?} 失敗：{error:?}"));
}

#[tokio::test]
async fn classic_frame_round_trips_between_sockets() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, false, false).await;
    let sender = open_socket(&interface, false, false).await;
    let id = standard(1, 0);
    let frame = Frame::new(id, &[0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80])
        .expect("8-byte 古典 CAN 幀應合法");

    send(&sender, &frame).await;
    let rx = recv_frame(&receiver).await;

    assert_eq!(rx.frame.id(), id);
    assert_eq!(rx.frame.data(), frame.data());
    assert_eq!(rx.frame.len(), 8);
    assert!(!rx.frame.is_fd());
    assert!(!rx.is_echo);
}

#[tokio::test]
async fn extended_id_and_remote_frame_round_trip() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, false, false).await;
    let sender = open_socket(&interface, false, false).await;
    let extended_id = extended(2, 0);
    let remote_id = standard(2, 1);
    let extended_frame = Frame::new(extended_id, &[0xa5, 0x5a]).expect("擴充識別碼古典幀應合法");
    let remote_frame = Frame::remote(remote_id, 8).expect("8-byte RTR 請求應合法");

    send(&sender, &extended_frame).await;
    send(&sender, &remote_frame).await;

    let extended_rx = recv_frame(&receiver).await;
    assert_eq!(extended_rx.frame.id(), extended_id);
    assert!(extended_rx.frame.id().is_extended());
    assert_eq!(extended_rx.frame.data(), &[0xa5, 0x5a]);
    assert!(!extended_rx.is_echo);

    let remote_rx = recv_frame(&receiver).await;
    assert_eq!(remote_rx.frame.id(), remote_id);
    assert!(remote_rx.frame.is_remote());
    assert_eq!(remote_rx.frame.len(), 8);
    assert!(remote_rx.frame.data().is_empty());
    assert!(!remote_rx.is_echo);
}

#[tokio::test]
async fn fd_frame_round_trips_with_all_canonical_lengths() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, true, false).await;
    let sender = open_socket(&interface, true, false).await;
    let payload = core::array::from_fn::<_, 64, _>(|index| {
        u8::try_from(index).expect("0..64 索引應可表示為 u8")
    });

    for (offset, length) in FD_LENGTHS.into_iter().enumerate() {
        let id = standard(
            3,
            u16::try_from(offset).expect("FD 長度表索引應可表示為 u16"),
        );
        let frame =
            Frame::new_fd(id, &payload[..length], false).expect("canonical FD 長度必須合法");
        send(&sender, &frame).await;
        let rx = recv_frame(&receiver).await;
        assert_eq!(rx.frame.id(), id, "FD 長度 {length} 的 ID 不符");
        assert_eq!(rx.frame.len(), length, "FD 長度 {length} 往返後改變");
        assert_eq!(
            rx.frame.data(),
            &payload[..length],
            "FD 長度 {length} 的資料不符"
        );
        assert!(rx.frame.is_fd(), "長度 {length} 應保留 FD 格式");
        assert!(!rx.is_echo);
    }
}

#[tokio::test]
async fn classic_and_fd_are_distinguished_on_an_fd_socket() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, true, false).await;
    let sender = open_socket(&interface, true, false).await;
    let classic = Frame::new(standard(4, 0), &[1, 2, 3]).expect("古典幀應合法");
    let fd = Frame::new_fd(standard(4, 1), &[4; 12], false).expect("FD 幀應合法");

    // 即使 socket 已啟用 CAN_RAW_FD_FRAMES，後端仍必須用 send datagram 長度
    // 區分 CAN_MTU 與 CANFD_MTU；不能只看 canfd_frame.flags。
    send(&sender, &classic).await;
    send(&sender, &fd).await;

    let first = recv_frame(&receiver).await;
    let second = recv_frame(&receiver).await;
    assert_eq!(first.frame.id(), classic.id());
    assert!(!first.frame.is_fd());
    assert_eq!(second.frame.id(), fd.id());
    assert!(second.frame.is_fd());
}

#[tokio::test]
async fn kernel_timestamps_are_populated() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, false, false).await;
    let sender = open_socket(&interface, false, false).await;
    let frame = Frame::new(standard(5, 0), &[0x55]).expect("時間戳測試幀應合法");
    let before = epoch_micros();

    send(&sender, &frame).await;
    let rx = recv_frame(&receiver).await;
    let after = epoch_micros();

    assert_eq!(
        rx.timestamp.source(),
        TimestampSource::Kernel,
        "應收到 SO_TIMESTAMPNS 核心時間戳；若在容器中執行，請確認已授予 CAP_NET_RAW；\
         SO_TIMESTAMPNS 設定失敗會降級為軟體時間戳"
    );
    assert!(
        (before.saturating_sub(1_000_000)..=after.saturating_add(1_000_000))
            .contains(&rx.timestamp.micros()),
        "核心時間戳 {} 不接近測試時的 epoch 微秒區間 {before}..={after}",
        rx.timestamp.micros()
    );
}

fn epoch_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
        })
}

#[tokio::test]
async fn brs_and_esi_flags_survive_the_kernel_abi() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, true, false).await;
    let sender = open_socket(&interface, true, false).await;
    let frame = Frame::new_fd(standard(6, 0), &[0x6a; 12], true)
        .and_then(|frame| frame.with_esi(true))
        .expect("BRS + ESI 的 FD 幀應合法");

    // 這只驗證 BRS/ESI 經 Linux CAN ABI 編解碼後仍存在；vcan 沒有 bit
    // timing，完全不驗證真實位元率或資料段位元率切換。
    send(&sender, &frame).await;
    let rx = recv_frame(&receiver).await;

    assert!(rx.frame.is_fd());
    assert!(rx.frame.is_brs());
    assert!(rx.frame.flags().contains(FrameFlags::ESI));
    assert_eq!(rx.frame.data(), frame.data());
}

#[tokio::test]
async fn recv_own_msgs_marks_echo_and_peers_do_not() {
    let Some(interface) = vcan() else {
        return;
    };
    let peer = open_socket(&interface, false, false).await;
    let own = open_socket(&interface, false, true).await;
    let frame = Frame::new(standard(7, 0), &[0x70]).expect("回音測試幀應合法");

    send(&own, &frame).await;
    let own_rx = recv_frame(&own).await;
    let peer_rx = recv_frame(&peer).await;

    assert_eq!(own_rx.frame, frame);
    assert!(own_rx.is_echo, "送出 socket 的 MSG_CONFIRM 應標記為 echo");
    assert_eq!(peer_rx.frame, frame);
    assert!(
        !peer_rx.is_echo,
        "其他 socket 只帶 MSG_DONTROUTE，不應標記為 echo"
    );
}

#[tokio::test]
async fn kernel_filter_applied_at_open_drops_unmatched_frames() {
    let Some(interface) = vcan() else {
        return;
    };
    let sentinel = standard(8, 1);
    let receiver = open_filtered_socket(
        &interface,
        false,
        FilterSet::with(FilterRule::exact(sentinel)),
    )
    .await;
    let sender = open_socket(&interface, false, false).await;
    let bait = Frame::new(standard(8, 0), &[0xb8]).expect("誘餌幀應合法");
    let expected = Frame::new(sentinel, &[0x58]).expect("sentinel 幀應合法");

    send(&sender, &bait).await;
    send(&sender, &expected).await;
    assert_next_is(&receiver, sentinel).await;
}

#[tokio::test]
async fn set_filter_at_runtime_takes_effect() {
    let Some(interface) = vcan() else {
        return;
    };
    let sentinel = standard(9, 1);
    let receiver = open_filtered_socket(&interface, false, FilterSet::reject_all()).await;
    let sender = open_socket(&interface, false, false).await;
    receiver
        .set_filter(&FilterSet::with(FilterRule::exact(sentinel)))
        .await
        .expect("執行期套用精確 SocketCAN 過濾器應成功");
    let bait = Frame::new(standard(9, 0), &[0xb9]).expect("誘餌幀應合法");
    let expected = Frame::new(sentinel, &[0x59]).expect("sentinel 幀應合法");

    send(&sender, &bait).await;
    send(&sender, &expected).await;
    assert_next_is(&receiver, sentinel).await;
}

#[tokio::test]
async fn inverted_filter_excludes_matching_ids() {
    let Some(interface) = vcan() else {
        return;
    };
    let excluded = standard(10, 0);
    let sentinel = standard(10, 1);
    let receiver = open_filtered_socket(
        &interface,
        false,
        FilterSet::with(FilterRule::exact(excluded).inverted()),
    )
    .await;
    let sender = open_socket(&interface, false, false).await;
    let bait = Frame::new(excluded, &[0xba]).expect("反轉過濾誘餌幀應合法");
    let expected = Frame::new(sentinel, &[0x5a]).expect("sentinel 幀應合法");

    send(&sender, &bait).await;
    send(&sender, &expected).await;
    assert_next_is(&receiver, sentinel).await;
}

#[tokio::test]
async fn reject_all_then_accept_all_recovers() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_filtered_socket(&interface, false, FilterSet::reject_all()).await;
    let observer = open_socket(&interface, false, false).await;
    let sender = open_socket(&interface, false, false).await;
    let bait_id = standard(11, 0);
    let bait = Frame::new(bait_id, &[0xbb]).expect("reject_all 誘餌幀應合法");
    send(&sender, &bait).await;
    let observed_bait = recv_frame(&observer).await;
    assert_eq!(
        observed_bait.frame.id(),
        bait_id,
        "accept-all observer 應確認誘餌已完成核心分派"
    );
    receiver
        .set_filter(&FilterSet::accept_all())
        .await
        .expect("reject_all 後恢復 accept_all 應成功");
    let sentinel = standard(11, 1);
    let frame = Frame::new(sentinel, &[0x5b]).expect("sentinel 幀應合法");

    send(&sender, &frame).await;
    assert_next_is(&receiver, sentinel).await;
}

#[tokio::test]
async fn capabilities_reflect_socket_configuration() {
    let Some(interface) = vcan() else {
        return;
    };
    let socket = open_socket(&interface, true, true).await;
    let capabilities = socket.capabilities();

    assert!(capabilities.can_fd);
    assert!(capabilities.brs);
    assert!(capabilities.echo_frames);
    assert!(capabilities.hardware_filter);
    assert!(!capabilities.hardware_timestamps);
}

#[tokio::test]
async fn burst_of_frames_arrives_in_order_without_loss() {
    let Some(interface) = vcan() else {
        return;
    };
    let receiver = open_socket(&interface, false, false).await;
    let sender = open_socket(&interface, false, false).await;
    let id = standard(13, 0);

    let receive_burst = async {
        for expected in 0_u16..200 {
            let rx = recv_frame(&receiver).await;
            assert_eq!(rx.frame.id(), id, "burst 收到非本測試 ID");
            assert_eq!(
                rx.frame.data(),
                expected.to_be_bytes(),
                "burst 在序號 {expected} 發生遺失或亂序"
            );
        }
    };
    let send_burst = async {
        // 先讓接收 future 進入 AsyncFd::readable()，再以小批連續幀反覆觸發
        // readiness edge，才能覆蓋 try_io/EPOLLET 的競態，而非只走 fast path。
        tokio::task::yield_now().await;
        for sequence in 0_u16..200 {
            let frame = Frame::new(id, &sequence.to_be_bytes()).expect("burst 古典幀應合法");
            send(&sender, &frame).await;
            if (sequence + 1) % 16 == 0 {
                tokio::task::yield_now().await;
            }
        }
    };

    let ((), ()) = tokio::join!(receive_burst, send_burst);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_send_and_recv_share_one_socket() {
    const FRAME_COUNT: u16 = 100;

    let Some(interface) = vcan() else {
        return;
    };
    let socket = Arc::new(open_socket(&interface, false, true).await);
    let id = standard(14, 0);
    let receive_socket = Arc::clone(&socket);
    let receiver = tokio::spawn(async move {
        for expected in 0_u16..FRAME_COUNT {
            let rx = recv_frame(&receive_socket).await;
            assert_eq!(rx.frame.id(), id, "並行接收收到非本測試 ID");
            assert_eq!(
                rx.frame.data(),
                expected.to_be_bytes(),
                "並行收送在序號 {expected} 發生遺失或亂序"
            );
            assert!(rx.is_echo, "同一 socket 的回送幀應標記為 echo");
        }
    });
    let send_socket = Arc::clone(&socket);
    let sender = tokio::spawn(async move {
        for sequence in 0_u16..FRAME_COUNT {
            let frame = Frame::new(id, &sequence.to_be_bytes()).expect("並行收送古典幀應合法");
            send(&send_socket, &frame).await;
        }
    });

    sender.await.expect("並行傳送 task 不應 panic");
    receiver.await.expect("並行接收 task 不應 panic");
}

#[tokio::test]
async fn close_is_idempotent_and_reports_closed() {
    let Some(interface) = vcan() else {
        return;
    };
    let socket = open_socket(&interface, false, false).await;
    let frame = Frame::new(standard(15, 0), &[0x15]).expect("關閉測試幀應合法");

    socket.close().await;
    socket.close().await;

    assert!(matches!(socket.recv().await, Err(Error::Closed)));
    assert!(matches!(socket.send(&frame).await, Err(Error::Closed)));
    assert!(matches!(socket.status().await, Err(Error::Closed)));
    assert!(matches!(
        socket.set_filter(&FilterSet::accept_all()).await,
        Err(Error::Closed)
    ));
}

#[tokio::test]
async fn opening_a_missing_interface_reports_open_error() {
    let Some(_interface) = vcan() else {
        return;
    };
    let factory = SocketCanFactory::new(SocketCanConfig::new("vcan-nope"))
        .expect("缺少的介面名稱本身語法仍合法");

    match factory.open().await {
        Err(Error::Open {
            source:
                BackendError::SocketCan {
                    op,
                    kind,
                    source: _,
                },
            ..
        }) => {
            assert_eq!(op, "if_nametoindex");
            assert_eq!(kind, FaultKind::Fatal);
        }
        Err(other) => panic!("缺少介面應回 if_nametoindex Fatal Open，實際：{other:?}"),
        Ok(_) => panic!("不存在的 vcan-nope 不應成功開啟"),
    }
}
