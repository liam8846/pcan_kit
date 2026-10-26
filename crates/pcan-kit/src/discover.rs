use pcan_core::Error;
#[cfg(any(feature = "basic", all(feature = "socketcan", target_os = "linux")))]
use pcan_core::{BackendError, FaultKind};

/// 跨後端的通道列舉結果。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ChannelInfo {
    /// PCAN-Basic 通道。
    #[cfg(feature = "basic")]
    Pcan(pcan_basic::PcanChannelInfo),
    /// Linux `SocketCAN` 網路介面。
    #[cfg(all(feature = "socketcan", target_os = "linux"))]
    SocketCan(pcan_socketcan::SocketCanInterfaceInfo),
    /// 未編譯任何後端時維持型別可用的不可建構變體。
    #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
    Unavailable(core::convert::Infallible),
}

impl ChannelInfo {
    /// 建立可直接餵給 [`crate::open()`] 的 URI，不含位元率查詢參數。
    ///
    /// PCAN-Basic 回報未知通道位址時會產生 `pcan://handle:0xNNN`
    /// 診斷字串；此回退形式目前不受 [`crate::parse_uri`] 接受，只供顯示
    /// 與問題診斷。
    #[must_use]
    pub fn uri(&self) -> Box<str> {
        match self {
            #[cfg(feature = "basic")]
            Self::Pcan(info) => info.channel_id().map_or_else(
                || format!("pcan://handle:{:#X}", info.handle()).into_boxed_str(),
                |channel| format!("pcan://{channel}").into_boxed_str(),
            ),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(info) => format!("socketcan://{}", info.name()).into_boxed_str(),
            #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
            Self::Unavailable(value) => match *value {},
        }
    }

    /// 借用人類可讀的裝置名稱，不進行配置。
    #[must_use]
    pub fn display_name(&self) -> &str {
        match self {
            #[cfg(feature = "basic")]
            Self::Pcan(info) => info.name(),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(info) => info.name(),
            #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
            Self::Unavailable(value) => match *value {},
        }
    }

    /// 判斷通道目前是否可開啟。
    #[must_use]
    pub fn is_available(&self) -> bool {
        match self {
            #[cfg(feature = "basic")]
            Self::Pcan(info) => info.is_available(),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(info) => info.is_up(),
            #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
            Self::Unavailable(value) => match *value {},
        }
    }

    /// 判斷通道是否具備 CAN FD 能力。
    #[must_use]
    pub fn supports_fd(&self) -> bool {
        match self {
            #[cfg(feature = "basic")]
            Self::Pcan(info) => info.supports_fd(),
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            Self::SocketCan(info) => info.supports_fd(),
            #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
            Self::Unavailable(value) => match *value {},
        }
    }
}

#[cfg(feature = "basic")]
fn should_skip_pcan_error(error: &Error) -> bool {
    matches!(error, Error::Load(_) | Error::Unsupported(_))
        || matches!(
            error,
            Error::Io(BackendError::PcanBasic {
                code,
                kind: FaultKind::Fatal,
                ..
            }) if *code != 0
        )
}

/// 列舉已啟用的後端，並套用門面層的後端略過契約。
///
/// PCAN-Basic 載入失敗、不支援列舉或由驅動回報非零狀態碼的致命故障，
/// 以及 `SocketCAN` 回報永久故障時，代表該後端目前不可用，因此只略過
/// 該後端。PCAN-Basic 的永久或暫時故障、阻塞工作的 `join` 失敗、
/// `SocketCAN` 的致命故障，以及其他非預期錯誤仍會向上傳遞。
///
/// # Errors
///
/// 後端回報不符合略過條件的錯誤時回傳錯誤。
#[cfg(any(feature = "basic", all(feature = "socketcan", target_os = "linux")))]
async fn list_enabled_channels() -> Result<Box<[ChannelInfo]>, Error> {
    let mut channels = Vec::new();

    #[cfg(feature = "basic")]
    match pcan_basic::list_channels().await {
        Ok(infos) => channels.extend(infos.into_iter().map(ChannelInfo::Pcan)),
        Err(error) if should_skip_pcan_error(&error) => {
            #[cfg(feature = "tracing")]
            tracing::debug!(%error, "略過不可用的 PCAN-Basic 通道列舉後端");
            #[cfg(not(feature = "tracing"))]
            let _ = error;
        }
        Err(error) => return Err(error),
    }

    #[cfg(all(feature = "socketcan", target_os = "linux"))]
    match pcan_socketcan::list_interfaces().await {
        Ok(infos) => channels.extend(infos.into_iter().map(ChannelInfo::SocketCan)),
        Err(
            error @ Error::Io(BackendError::SocketCan {
                kind: FaultKind::Permanent,
                ..
            }),
        ) => {
            #[cfg(feature = "tracing")]
            tracing::debug!(%error, "略過不可用的 SocketCAN 通道列舉後端");
            #[cfg(not(feature = "tracing"))]
            let _ = error;
        }
        Err(error) => return Err(error),
    }

    Ok(channels.into_boxed_slice())
}

/// 列舉所有已編譯後端目前可見的 CAN 通道。
///
/// 單一後端不可用不會使整體列舉失敗。PCAN-Basic 函式庫載入失敗、版本
/// 不支援列舉或由驅動回報非零狀態碼的致命故障，以及 `SocketCAN` 回報
/// 永久故障時，會略過該後端；沒有任何後端可用時回傳空切片。PCAN-Basic
/// 回報永久或暫時故障、PCAN-Basic 阻塞工作的 `join` 失敗、`SocketCAN`
/// 阻塞工作無法完成所產生的致命故障，以及其他非預期錯誤仍會向上傳遞。
///
/// # Errors
///
/// PCAN-Basic 回報永久或暫時故障、PCAN-Basic 阻塞工作的 `join` 失敗、
/// `SocketCAN` 回報致命故障，或後端列舉遇到其他非預期錯誤時回傳錯誤。
#[cfg_attr(
    not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))),
    allow(clippy::unused_async)
)]
pub async fn list_channels() -> Result<Box<[ChannelInfo]>, Error> {
    #[cfg(any(feature = "basic", all(feature = "socketcan", target_os = "linux")))]
    {
        list_enabled_channels().await
    }
    #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
    {
        Ok(Vec::new().into_boxed_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::list_channels;
    #[cfg(feature = "basic")]
    use super::should_skip_pcan_error;
    #[cfg(feature = "basic")]
    use pcan_core::{BackendError, Error, FaultKind, LoadError};

    #[tokio::test]
    async fn missing_optional_backend_does_not_fail_discovery() {
        assert!(list_channels().await.is_ok());
    }

    #[cfg(feature = "basic")]
    fn pcan_error(code: u32, kind: FaultKind) -> Error {
        Error::Io(BackendError::PcanBasic {
            code,
            text: "測試".into(),
            op: "測試 PCAN 錯誤略過條件",
            kind,
        })
    }

    #[cfg(feature = "basic")]
    #[test]
    fn skips_only_unavailable_pcan_backend_errors() {
        let load = Error::Load(LoadError::MissingSymbol {
            symbol: "CAN_GetValue",
        });
        let unsupported = Error::Unsupported("測試不支援");

        assert!(should_skip_pcan_error(&load));
        assert!(should_skip_pcan_error(&unsupported));
        assert!(should_skip_pcan_error(&pcan_error(
            0x0000_0200,
            FaultKind::Fatal
        )));
        assert!(!should_skip_pcan_error(&pcan_error(0, FaultKind::Fatal)));
        assert!(!should_skip_pcan_error(&pcan_error(
            1,
            FaultKind::Permanent
        )));
        assert!(!should_skip_pcan_error(&pcan_error(
            1,
            FaultKind::Transient
        )));
    }
}
