#[cfg(any(feature = "basic", all(feature = "socketcan", target_os = "linux")))]
use pcan_core::{Bitrate, TransportConfig};
use pcan_core::{ConfigError, Error};
use pcan_link::Link;

use crate::AnyFactory;

fn invalid_channel(value: impl Into<Box<str>>) -> Error {
    ConfigError::InvalidChannel(value.into()).into()
}

fn invalid_bitrate(value: impl Into<Box<str>>) -> Error {
    ConfigError::InvalidBitrate(value.into()).into()
}

fn parse_rate(value: &str) -> Result<u32, Error> {
    let lower = value.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(prefix) = lower.strip_suffix('k') {
        (prefix, 1_000_u32)
    } else if let Some(prefix) = lower.strip_suffix('m') {
        (prefix, 1_000_000_u32)
    } else {
        (lower.as_str(), 1_u32)
    };
    let base = digits
        .parse::<u32>()
        .map_err(|_| invalid_bitrate(format!("無法解析位元率 `{value}`")))?;
    base.checked_mul(multiplier)
        .ok_or_else(|| invalid_bitrate(format!("位元率 `{value}` 超出 u32 範圍")))
}

fn parse_bool(key: &str, value: &str) -> Result<bool, Error> {
    match value {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(invalid_channel(format!(
            "查詢參數 `{key}` 需要 true/false，收到 `{value}`"
        ))),
    }
}

#[derive(Default)]
struct Query {
    bitrate: Option<u32>,
    data_bitrate: Option<u32>,
    listen_only: Option<bool>,
    fd: Option<bool>,
}

fn parse_query(value: Option<&str>) -> Result<Query, Error> {
    let mut query = Query::default();
    let Some(value) = value else {
        return Ok(query);
    };
    for item in value.split('&').filter(|item| !item.is_empty()) {
        let (key, raw) = item
            .split_once('=')
            .ok_or_else(|| invalid_channel(format!("查詢參數 `{item}` 缺少 `=`")))?;
        match key {
            "bitrate" => query.bitrate = Some(parse_rate(raw)?),
            "dbitrate" => query.data_bitrate = Some(parse_rate(raw)?),
            "listen_only" => query.listen_only = Some(parse_bool(key, raw)?),
            "fd" => query.fd = Some(parse_bool(key, raw)?),
            _ => {
                return Err(invalid_channel(format!("不支援的 URI 查詢參數 `{key}`")));
            }
        }
    }
    Ok(query)
}

#[cfg(any(feature = "basic", all(feature = "socketcan", target_os = "linux")))]
fn common_config(query: &Query) -> TransportConfig {
    let nominal = query.bitrate.unwrap_or(500_000);
    let wants_fd = query.data_bitrate.is_some() || query.fd == Some(true);
    let bitrate = if wants_fd {
        Bitrate::Fd {
            nominal,
            data: query.data_bitrate.unwrap_or(2_000_000),
        }
    } else {
        Bitrate::Classic { nominal }
    };
    TransportConfig::default()
        .with_bitrate(bitrate)
        .with_listen_only(query.listen_only.unwrap_or(false))
}

/// 從 URI 建立執行期後端工廠。
///
/// 支援 `pcan://usb1?bitrate=500k`、含 `dbitrate=2m` 的 CAN FD、
/// `listen_only=true`，以及 Linux 的 `socketcan://can0?fd=true`。
///
/// # Errors
///
/// URI scheme、通道、布林值或位元率無法解析，以及選定後端未編譯時回傳錯誤。
pub fn parse_uri(uri: &str) -> Result<AnyFactory, Error> {
    let (scheme, remainder) = uri
        .split_once("://")
        .ok_or_else(|| invalid_channel(format!("URI 缺少 `://`：`{uri}`")))?;
    let (target, query_text) = remainder
        .split_once('?')
        .map_or((remainder, None), |(target, query)| (target, Some(query)));
    if target.is_empty() || target.contains('/') {
        return Err(invalid_channel(target));
    }
    let query = parse_query(query_text)?;
    #[cfg(not(any(feature = "basic", all(feature = "socketcan", target_os = "linux"))))]
    let _ = &query;
    match scheme {
        "pcan" => {
            #[cfg(feature = "basic")]
            {
                let channel = pcan_basic::PcanChannelId::parse(target)?;
                let mut config = pcan_basic::PcanConfig::new(channel);
                config.common = common_config(&query);
                pcan_basic::PcanFactory::new(config).map(AnyFactory::Basic)
            }
            #[cfg(not(feature = "basic"))]
            {
                Err(Error::Unsupported("編譯時未啟用 `basic` feature"))
            }
        }
        "socketcan" => {
            #[cfg(all(feature = "socketcan", target_os = "linux"))]
            {
                let mut config = pcan_socketcan::SocketCanConfig::new(target);
                config.common = common_config(&query);
                pcan_socketcan::SocketCanFactory::new(config).map(AnyFactory::SocketCan)
            }
            #[cfg(not(all(feature = "socketcan", target_os = "linux")))]
            {
                Err(Error::Unsupported(
                    "SocketCAN 只在 Linux 且啟用 `socketcan` feature 時可用",
                ))
            }
        }
        _ => Err(invalid_channel(format!("未知的 CAN URI scheme `{scheme}`"))),
    }
}

/// 解析 URI 並建立已啟動監督任務的 [`Link`]。
///
/// 此函式不等待實體通道連上；可用 [`Link::wait_connected`] 明確等待。
///
/// # Errors
///
/// URI 或後端工廠設定無效時回傳錯誤。
pub async fn open(uri: &str) -> Result<Link, Error> {
    let factory = parse_uri(uri)?;
    Ok(Link::builder(factory).build())
}

#[cfg(test)]
mod tests {
    use super::{parse_query, parse_rate};

    #[test]
    fn parses_supported_rate_spellings() {
        assert_eq!(parse_rate("500k").ok(), Some(500_000));
        assert_eq!(parse_rate("500000").ok(), Some(500_000));
        assert_eq!(parse_rate("1m").ok(), Some(1_000_000));
        assert!(parse_rate("fast").is_err());
    }

    #[test]
    fn rejects_unknown_query_keys() {
        assert!(parse_query(Some("speed=500k")).is_err());
    }
}
