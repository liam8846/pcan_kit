use std::fs;
use std::io;
use std::path::Path;

use pcan_core::{BackendError, Error, FaultKind};

const SYS_CLASS_NET: &str = "/sys/class/net";
const ARPHRD_CAN: u32 = 280;
const IFF_UP: u32 = 0x01;
const CANFD_MTU: u32 = 72;

/// 一個 Linux CAN 網路介面的資訊。
#[derive(Clone, Debug)]
pub struct SocketCanInterfaceInfo {
    name: Box<str>,
    index: u32,
    mtu: u32,
    is_up: bool,
}

impl SocketCanInterfaceInfo {
    /// 回傳網路介面名稱。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 回傳核心配置的網路介面索引。
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }

    /// 回傳網路介面的最大傳輸單元位元組數。
    #[must_use]
    pub const fn mtu(&self) -> u32 {
        self.mtu
    }

    /// 回傳網路介面是否已啟用。
    #[must_use]
    pub const fn is_up(&self) -> bool {
        self.is_up
    }

    /// 介面 MTU 是否已設定為 CAN FD 所需的 72 位元組。
    #[must_use]
    pub const fn supports_fd(&self) -> bool {
        self.mtu >= CANFD_MTU
    }
}

fn parse_interface(
    name: &str,
    type_text: &str,
    flags_text: &str,
    mtu_text: &str,
    index_text: &str,
) -> Option<SocketCanInterfaceInfo> {
    let hardware_type = type_text.trim().parse::<u32>().ok()?;
    // 依核心回報的 ARPHRD_CAN 種類辨識，避免漏掉 vxcan，亦不會誤收名稱
    // 恰好以 can 開頭的其他網路介面。
    if hardware_type != ARPHRD_CAN {
        return None;
    }

    let flags_text = flags_text.trim();
    let flags_digits = flags_text
        .strip_prefix("0x")
        .or_else(|| flags_text.strip_prefix("0X"))
        .unwrap_or(flags_text);
    let flags = u32::from_str_radix(flags_digits, 16).ok()?;
    let mtu = mtu_text.trim().parse::<u32>().ok()?;
    let index = index_text.trim().parse::<u32>().ok()?;

    Some(SocketCanInterfaceInfo {
        name: name.into(),
        index,
        mtu,
        is_up: flags & IFF_UP != 0,
    })
}

fn read_interface(path: &Path, name: &str) -> io::Result<Option<SocketCanInterfaceInfo>> {
    let type_text = fs::read_to_string(path.join("type"))?;
    let flags_text = fs::read_to_string(path.join("flags"))?;
    let mtu_text = fs::read_to_string(path.join("mtu"))?;
    let index_text = fs::read_to_string(path.join("ifindex"))?;
    Ok(parse_interface(
        name,
        &type_text,
        &flags_text,
        &mtu_text,
        &index_text,
    ))
}

// `Permanent` 是門面層可略過後端的語意契約，不可任意更動。
fn root_error(source: io::Error) -> Error {
    Error::Io(BackendError::SocketCan {
        op: "列舉 /sys/class/net",
        kind: FaultKind::Permanent,
        source,
    })
}

// `Fatal` 是門面層必須上報非預期錯誤的語意契約，不可任意更動。
fn join_error(source: &tokio::task::JoinError) -> Error {
    Error::Io(BackendError::SocketCan {
        op: "等待 SocketCAN 通道列舉工作",
        kind: FaultKind::Fatal,
        source: io::Error::other(source.to_string()),
    })
}

#[cfg(feature = "tracing")]
fn log_entry_error(error: &io::Error) {
    tracing::debug!(%error, "讀取 SocketCAN 介面目錄項目失敗，略過該筆");
}

#[cfg(not(feature = "tracing"))]
fn log_entry_error(_: &io::Error) {}

#[cfg(feature = "tracing")]
fn log_interface_error(name: &str, error: &io::Error) {
    tracing::debug!(interface = %name, %error, "讀取 SocketCAN 介面資訊失敗，略過該筆");
}

#[cfg(not(feature = "tracing"))]
fn log_interface_error(_: &str, _: &io::Error) {}

fn list_interfaces_blocking() -> Result<Box<[SocketCanInterfaceInfo]>, Error> {
    let entries = fs::read_dir(SYS_CLASS_NET).map_err(root_error)?;
    let mut interfaces = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log_entry_error(&error);
                continue;
            }
        };
        let Ok(name) = entry.file_name().into_string() else {
            #[cfg(feature = "tracing")]
            tracing::debug!("SocketCAN 介面名稱不是 UTF-8，略過該筆");
            continue;
        };

        match read_interface(&entry.path(), &name) {
            Ok(Some(interface)) => interfaces.push(interface),
            Ok(None) => {}
            Err(error) => log_interface_error(&name, &error),
        }
    }

    interfaces.sort_unstable_by(|left, right| left.name.cmp(&right.name));
    Ok(interfaces.into_boxed_slice())
}

/// 列舉本機所有 CAN 類型的網路介面（含 `can`、`vcan`、`vxcan`）。
///
/// 掃描與 sysfs 檔案讀取會在 Tokio 阻塞工作執行緒執行。掃描期間若單一
/// 介面消失或無法讀取，只會略過該筆，不影響其他列舉結果。
///
/// # Errors
///
/// `/sys/class/net` 根目錄無法讀取，或 Tokio 阻塞工作無法完成時回傳錯誤。
pub async fn list_interfaces() -> Result<Box<[SocketCanInterfaceInfo]>, Error> {
    tokio::task::spawn_blocking(list_interfaces_blocking)
        .await
        .map_err(|error| join_error(&error))?
}

#[cfg(test)]
mod tests {
    use super::parse_interface;

    #[test]
    fn rejects_non_can_hardware_type() {
        assert!(parse_interface("eth0", "1", "0x1", "1500", "2").is_none());
    }

    #[test]
    fn parses_prefixed_hex_flags_and_classic_can_mtu() -> Result<(), &'static str> {
        let interface =
            parse_interface("can0", "280", "0x1", "16", "3").ok_or("解析 CAN 介面失敗")?;

        assert_eq!(interface.name(), "can0");
        assert_eq!(interface.index(), 3);
        assert_eq!(interface.mtu(), 16);
        assert!(interface.is_up());
        assert!(!interface.supports_fd());
        Ok(())
    }

    #[test]
    fn parses_unprefixed_hex_flags_and_fd_mtu() -> Result<(), &'static str> {
        let interface =
            parse_interface("vcan0", "280", "10000", "72", "4").ok_or("解析 CAN FD 介面失敗")?;

        assert!(!interface.is_up());
        assert!(interface.supports_fd());
        Ok(())
    }

    #[test]
    fn trims_trailing_newlines_from_every_field() -> Result<(), &'static str> {
        let interface = parse_interface("vxcan0", "280\n", "0X0001\n", "72\n", "5\n")
            .ok_or("解析含換行的 CAN 介面失敗")?;

        assert_eq!(interface.index(), 5);
        assert!(interface.is_up());
        assert!(interface.supports_fd());
        Ok(())
    }

    #[test]
    fn rejects_an_unparseable_field() {
        assert!(parse_interface("can0", "280", "0x1", "無效", "3").is_none());
    }
}
