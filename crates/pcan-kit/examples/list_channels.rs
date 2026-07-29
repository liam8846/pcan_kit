//! 列出目前可見的 PCAN-Basic 與 Linux `SocketCAN` 通道。

use pcan_kit::{Error, list_channels};

async fn run() -> Result<(), Error> {
    let channels = list_channels().await?;
    if channels.is_empty() {
        println!("找不到 CAN 通道；可能尚未安裝驅動，或目前沒有連接硬體");
        return Ok(());
    }

    for channel in &channels {
        let name = match channel.display_name() {
            "" => "未命名裝置",
            name => name,
        };
        let availability = if channel.is_available() {
            "可開啟"
        } else {
            "目前不可開啟"
        };
        let fd = if channel.supports_fd() {
            "支援 CAN FD"
        } else {
            "不支援 CAN FD"
        };
        println!("{}｜{}｜{}｜{}", channel.uri(), name, availability, fd);
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("無法列舉 CAN 通道：{error}");
    }
}
