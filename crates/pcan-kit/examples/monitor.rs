//! 開啟真實後端並監看第一個 CAN 幀或匯流排事件。

use core::time::Duration;

use pcan_kit::{Error, open};

async fn run() -> Result<(), Error> {
    let uri = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "socketcan://can0".to_owned());
    let link = open(&uri).await?;
    match tokio::time::timeout(Duration::from_secs(3), link.wait_connected()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("連線監督已停止：{error}");
            link.close().await;
            return Ok(());
        }
        Err(_) => {
            eprintln!("三秒內未連線；請確認硬體、驅動與 URI：{uri}");
            link.close().await;
            return Ok(());
        }
    }
    let mut frames = link.subscribe_all_raw();
    let mut events = link.events();
    println!("已連線 {uri}，等待十秒內的第一個幀或匯流排事件…");
    tokio::select! {
        frame = frames.recv() => println!("RX: {frame:?}"),
        event = events.recv() => println!("BUS: {event:?}"),
        () = tokio::time::sleep(Duration::from_secs(10)) => {
            println!("等待期間沒有事件；安靜的 CAN 匯流排是正常狀況");
        }
    }
    link.close().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("monitor 無法啟動：{error}");
        eprintln!("PCAN-Basic 未安裝時請設定 PCAN_BASIC_LIB；SocketCAN 請先啟用介面");
    }
}
