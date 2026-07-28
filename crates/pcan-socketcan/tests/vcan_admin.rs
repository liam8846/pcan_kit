#![cfg_attr(
    not(all(target_os = "linux", feature = "vcan-tests")),
    allow(missing_docs)
)]
#![cfg(all(target_os = "linux", feature = "vcan-tests"))]
//! 需要管理員權限的 vcan 整合測試。
//!
//! 這個 test target 只在設定 `PCAN_VCAN_ADMIN` 時執行，並以
//! `sudo -n ip link set <iface> up/down` 操作專屬介面。CI 必須事先允許該
//! `ip` 命令免密碼 sudo。破壞性測試必須使用專屬介面：
//!
//! ```text
//! PCAN_VCAN_ADMIN=vcan9 cargo test -p pcan-socketcan --features vcan-tests \
//!   --test vcan_admin -- --test-threads=1 --nocapture
//! ```

use std::process::Command;

use pcan_core::{BackendError, CanId, Error, FaultKind, Frame, Transport, TransportFactory};
use pcan_socketcan::{CanSocket, SocketCanConfig, SocketCanFactory};

const ADMIN_SEND_ID: u16 = 0x290;

fn vcan_admin() -> Option<String> {
    match std::env::var("PCAN_VCAN_ADMIN") {
        Ok(name) if !name.is_empty() => {
            if matches!(
                std::env::var("PCAN_VCAN"),
                Ok(primary) if !primary.is_empty() && primary == name
            ) {
                panic!(
                    "破壞性測試必須使用專屬獨立介面（例如 vcan9），不可與一般 vcan 測試共用介面"
                );
            }
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
            Err(error) => eprintln!(
                "警告：Drop 無法將管理員 vcan 介面 `{}` 恢復為 up：\
                 無法執行 sudo：{error}",
                self.name
            ),
            Ok(output) if !output.status.success() => eprintln!(
                "警告：Drop 無法將管理員 vcan 介面 `{}` 恢復為 up（status={}）：stderr={}",
                self.name,
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
            Ok(_) => {}
        }
    }
}

fn standard(raw: u16) -> CanId {
    CanId::standard(raw).expect("管理員測試 CAN ID 必須落在 11-bit 範圍")
}

async fn open_socket(interface: &str) -> CanSocket {
    let factory = SocketCanFactory::new(SocketCanConfig::new(interface))
        .expect("有效的管理員 vcan 介面名稱應建立工廠");
    factory.open().await.unwrap_or_else(|error| {
        panic!("開啟管理員測試介面 `{interface}` 的 SocketCAN 失敗：{error:?}")
    })
}

#[tokio::test]
async fn send_on_downed_interface_is_fatal() {
    let Some(interface) = vcan_admin() else {
        return;
    };
    let admin = AdminInterface::arm(interface.clone());
    let socket = open_socket(&interface).await;
    let frame = Frame::new(standard(ADMIN_SEND_ID), &[0x20]).expect("管理員送出測試幀應合法");

    admin.down();
    let result = socket.send(&frame).await;
    admin.up();

    match result {
        Err(Error::Io(BackendError::SocketCan { op, kind, source })) => {
            assert_eq!(op, "send(SocketCAN)");
            assert_eq!(kind, FaultKind::Fatal);
            assert_eq!(
                source.raw_os_error(),
                Some(libc::ENETDOWN),
                "downed vcan 應由核心回報 ENETDOWN"
            );
        }
        Err(other) => panic!("downed vcan 送出應回 ENETDOWN Fatal，實際：{other:?}"),
        Ok(()) => panic!("downed vcan 不應成功送出"),
    }
}
