//! 使用 `IdAndPrefix` matcher 示範一筆無硬體 UDS 請求—回應交易。

use core::time::Duration;

use pcan_core::testing::{FakeFactory, FakeTransportBuilder};
use pcan_kit::{
    CanId, Error, Frame, Link, Matcher, PrefixPattern, ResponseSpec, RxFrame, Timestamp,
    TimestampSource, TransactionError, TransportEvent,
};

async fn run() -> Result<(), TransactionError> {
    let request_id = CanId::standard(0x7df)
        .map_err(Error::from)
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    let response_id = CanId::standard(0x7e8)
        .map_err(Error::from)
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    let request = Frame::new(request_id, &[0x02, 0x10, 0x01])
        .map_err(Error::from)
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    let response = Frame::new(response_id, &[0x02, 0x50, 0x01])
        .map_err(Error::from)
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    let prefix = PrefixPattern::new(&[0x02, 0x50])
        .map_err(Error::from)
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    let spec = ResponseSpec::new(
        Matcher::IdAndPrefix {
            id: response_id,
            prefix,
        },
        Duration::from_millis(200),
    );
    let (factory, handle) = FakeFactory::new(FakeTransportBuilder::default());
    let link = Link::builder(factory)
        .connect()
        .await
        .map_err(|error| TransactionError::Send(Box::new(error)))?;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.inject(TransportEvent::Frame(RxFrame::new(
            response,
            Timestamp::new(10_000, TimestampSource::Software),
            false,
        )));
    });
    let received = link.request(request, &spec).await?;
    println!("UDS 回應：{received:?}");
    link.close().await;
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("UDS request 範例失敗：{error}");
    }
}
