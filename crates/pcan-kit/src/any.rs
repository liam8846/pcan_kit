use core::future::Future;

use pcan_core::{
    BusStatus, Capabilities, Error, FilterSet, Frame, Transport, TransportEvent, TransportFactory,
};

/// 執行期可選的後端傳輸。
///
/// 此型別使用列舉分派而非 `dyn Transport`。後者會迫使 RPITIT future
/// 裝箱；在 1 Mbit/s 滿載約 9000 幀/秒時，就是每秒約 9000 次堆積配置。
/// 列舉分派沒有該成本，且編譯器仍可內聯各後端。
#[non_exhaustive]
#[derive(Debug)]
pub enum AnyTransport {
    /// PCAN-Basic 通道。
    #[cfg(feature = "basic")]
    Basic(pcan_basic::PcanChannel),
    /// Linux `SocketCAN` raw socket。
    #[cfg(all(feature = "socketcan", target_os = "linux"))]
    SocketCan(pcan_socketcan::CanSocket),
    /// 無硬體測試傳輸。
    #[cfg(feature = "test-util")]
    Fake(pcan_core::testing::FakeTransport),
    /// 沒有啟用任何後端 feature 時維持型別可編譯的不可建構變體。
    #[cfg(not(any(
        feature = "basic",
        all(feature = "socketcan", target_os = "linux"),
        feature = "test-util"
    )))]
    Unavailable(core::convert::Infallible),
}

#[allow(clippy::manual_async_fn)]
impl Transport for AnyTransport {
    fn recv(&self) -> impl Future<Output = Result<TransportEvent, Error>> + Send {
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(transport) => transport.recv().await,
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(transport) => transport.recv().await,
                #[cfg(feature = "test-util")]
                Self::Fake(transport) => transport.recv().await,
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn send(&self, frame: &Frame) -> impl Future<Output = Result<(), Error>> + Send {
        let frame = *frame;
        #[cfg(not(any(
            feature = "basic",
            all(feature = "socketcan", target_os = "linux"),
            feature = "test-util"
        )))]
        let _ = frame;
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(transport) => transport.send(&frame).await,
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(transport) => transport.send(&frame).await,
                #[cfg(feature = "test-util")]
                Self::Fake(transport) => transport.send(&frame).await,
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn status(&self) -> impl Future<Output = Result<BusStatus, Error>> + Send {
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(transport) => transport.status().await,
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(transport) => transport.status().await,
                #[cfg(feature = "test-util")]
                Self::Fake(transport) => transport.status().await,
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<(), Error>> + Send {
        let filter = filter.clone();
        #[cfg(not(any(
            feature = "basic",
            all(feature = "socketcan", target_os = "linux"),
            feature = "test-util"
        )))]
        let _ = &filter;
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(transport) => transport.set_filter(&filter).await,
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(transport) => transport.set_filter(&filter).await,
                #[cfg(feature = "test-util")]
                Self::Fake(transport) => transport.set_filter(&filter).await,
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(transport) => transport.close().await,
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(transport) => transport.close().await,
                #[cfg(feature = "test-util")]
                Self::Fake(transport) => transport.close().await,
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn capabilities(&self) -> Capabilities {
        match self {
            #[cfg(feature = "basic")]
            Self::Basic(transport) => transport.capabilities(),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(transport) => transport.capabilities(),
            #[cfg(feature = "test-util")]
            Self::Fake(transport) => transport.capabilities(),
            #[cfg(not(any(
                feature = "basic",
                all(feature = "socketcan", target_os = "linux"),
                feature = "test-util"
            )))]
            Self::Unavailable(value) => match *value {},
        }
    }
}

/// 對應 [`AnyTransport`] 的執行期後端工廠。
#[non_exhaustive]
#[derive(Debug)]
pub enum AnyFactory {
    /// PCAN-Basic 工廠。
    #[cfg(feature = "basic")]
    Basic(pcan_basic::PcanFactory),
    /// Linux `SocketCAN` 工廠。
    #[cfg(all(feature = "socketcan", target_os = "linux"))]
    SocketCan(pcan_socketcan::SocketCanFactory),
    /// 無硬體測試工廠。
    #[cfg(feature = "test-util")]
    Fake(pcan_core::testing::FakeFactory),
    /// 沒有啟用任何後端 feature 時維持型別可編譯的不可建構變體。
    #[cfg(not(any(
        feature = "basic",
        all(feature = "socketcan", target_os = "linux"),
        feature = "test-util"
    )))]
    Unavailable(core::convert::Infallible),
}

#[allow(clippy::manual_async_fn)]
impl TransportFactory for AnyFactory {
    type Transport = AnyTransport;

    fn open(&self) -> impl Future<Output = Result<Self::Transport, Error>> + Send {
        async move {
            match self {
                #[cfg(feature = "basic")]
                Self::Basic(factory) => factory.open().await.map(AnyTransport::Basic),
                #[cfg(all(feature = "socketcan", target_os = "linux"))]
                Self::SocketCan(factory) => factory.open().await.map(AnyTransport::SocketCan),
                #[cfg(feature = "test-util")]
                Self::Fake(factory) => factory.open().await.map(AnyTransport::Fake),
                #[cfg(not(any(
                    feature = "basic",
                    all(feature = "socketcan", target_os = "linux"),
                    feature = "test-util"
                )))]
                Self::Unavailable(value) => match *value {},
            }
        }
    }

    fn describe(&self) -> &str {
        match self {
            #[cfg(feature = "basic")]
            Self::Basic(factory) => factory.describe(),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(factory) => factory.describe(),
            #[cfg(feature = "test-util")]
            Self::Fake(factory) => factory.describe(),
            #[cfg(not(any(
                feature = "basic",
                all(feature = "socketcan", target_os = "linux"),
                feature = "test-util"
            )))]
            Self::Unavailable(value) => match *value {},
        }
    }
}
