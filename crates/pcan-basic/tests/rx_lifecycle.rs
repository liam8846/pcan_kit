#![cfg_attr(not(target_os = "linux"), allow(missing_docs))]
#![cfg(target_os = "linux")]
//! 以測試替身驗證接收註冊在通道解除初始化前移除，不需要 PCAN 硬體。

use core::future::Future;
use core::task::{Context, Waker};
use core::time::Duration;
use std::process::Command;

use pcan_basic::{PcanChannelId, PcanConfig, PcanFactory};
use pcan_basic_sys::load_from;
use pcan_core::{Error, Transport, TransportFactory};

#[tokio::test]
async fn close_drop_and_failed_open_remove_registration_before_uninitialize() {
    let directory = std::env::temp_dir().join(format!("pcan-rx-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("建立測試目錄");
    let library = directory.join("libpcanmock.so");
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-Wall", "-Werror", "-o"])
        .arg(&library)
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/pcan_mock.c"
        ))
        .output()
        .expect("Linux 測試需要 C 編譯器");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let api = load_from(&library).expect("載入測試替身");
    assert_eq!(&*api.error_text(0), "mock error");
    assert!(api.error_text(u32::MAX).contains("未提供 NUL 結尾"));
    let mut config = PcanConfig::new(PcanChannelId::Usb(1));
    config.library_path = Some(library);
    let factory = PcanFactory::new(config).expect("工廠");

    for _ in 0..8 {
        let channel = factory.open().await.expect("開啟通道");
        assert!(
            api.get_value_i32(0, 0xe5).1 > 0,
            "測試必須觀察到實際 epoll 註冊"
        );
        let mut receiving = Box::pin(channel.recv());
        assert!(
            receiving
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        channel.close().await;
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(1), receiving).await,
                Ok(Err(Error::Closed))
            ),
            "關閉必須喚醒已等待的接收"
        );
        channel.close().await;
        drop(channel);
        assert_eq!(api.get_value_i32(0, 0xe0).1, 0, "解除初始化時不能保留註冊");
        assert_eq!(api.get_value_i32(0, 0xe3).1, 0, "解除初始化後不能讀取");
    }
    let channel = factory.open().await.expect("再開啟");
    drop(channel);
    assert_eq!(
        api.get_value_i32(0, 0xe2).1,
        9,
        "close 與 Drop 不能重複解除初始化"
    );
    assert_eq!(api.set_value_u32(0, 0xe1, 1), 0);
    assert!(factory.open().await.is_err(), "註冊後套用過濾器失敗");
    assert_eq!(api.get_value_i32(0, 0xe2).1, 10);
    assert_eq!(api.get_value_i32(0, 0xe0).1, 0, "失敗路徑也須先反註冊");
    drop(factory);
    drop(api);
    std::fs::remove_dir_all(directory).expect("清理測試目錄");
}
