//! 使用無硬體假傳輸示範收到指定 ID 後原樣回送。

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_kit::{
    CanId, Error, FilterRule, FilterSet, Frame, Link, RxFrame, Timestamp, TimestampSource,
    TransportEvent,
};

async fn run() -> Result<(), Error> {
    let id = CanId::standard(0x321)?;
    let incoming = Frame::new(id, &[1, 2, 3])?;
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory).connect().await?;
    let mut subscription = link
        .subscribe_filter(FilterSet::with(FilterRule::exact(id)))
        .await?;
    handle.inject(TransportEvent::Frame(RxFrame::new(
        incoming,
        Timestamp::new(1, TimestampSource::Software),
        false,
    )));
    if let Some(received) = subscription.recv().await {
        let reply = Frame::new(received.frame.id(), received.frame.data())?;
        link.send_await(reply).await?;
        println!("收到並回送：{reply:?}");
    }
    link.close().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("echo 範例失敗：{error}");
    }
}
