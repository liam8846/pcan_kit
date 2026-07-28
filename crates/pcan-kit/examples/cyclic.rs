//! 使用無硬體假傳輸示範週期傳送、原地更新 payload 與 detach。

use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_kit::{CanId, CyclicConfig, Error, Frame, Link};

async fn run() -> Result<(), Error> {
    let id = CanId::standard(0x100)?;
    let frame = Frame::new(id, &[0, 1, 2, 3])?;
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).connect().await?;
    let cyclic = link.schedule_cyclic(CyclicConfig::new(frame, Duration::from_millis(20)))?;
    tokio::time::sleep(Duration::from_millis(55)).await;
    cyclic.set_payload(&[9, 8, 7, 6])?;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let id = cyclic.detach();
    println!(
        "週期項目 {id:?} 已 detach；目前假後端收到 {} 幀",
        handle.sent().len()
    );
    link.close().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("cyclic 範例失敗：{error}");
    }
}
