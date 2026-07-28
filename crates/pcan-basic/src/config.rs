use core::fmt;
use std::path::PathBuf;

use pcan_basic_sys::{
    PCAN_BAUD_1M, PCAN_BAUD_5K, PCAN_BAUD_10K, PCAN_BAUD_20K, PCAN_BAUD_33K, PCAN_BAUD_47K,
    PCAN_BAUD_50K, PCAN_BAUD_83K, PCAN_BAUD_95K, PCAN_BAUD_100K, PCAN_BAUD_125K, PCAN_BAUD_250K,
    PCAN_BAUD_500K, PCAN_BAUD_800K, TPCANBaudrate, TPCANHandle,
};
use pcan_core::{Bitrate, ConfigError, TransportConfig};

/// PCAN 通道位址。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum PcanChannelId {
    /// USB 通道，索引 1 至 16。
    Usb(u8),
    /// PCI/PCIe 通道，索引 1 至 16。
    Pci(u8),
    /// LAN 通道，索引 1 至 16。
    Lan(u8),
    /// ISA 通道，索引 1 至 8。
    Isa(u8),
    /// PC Card 通道，索引 1 至 2。
    Pcc(u8),
    /// Dongle 通道，目前只有索引 1。
    Dng(u8),
}

fn invalid_channel(value: impl Into<Box<str>>) -> ConfigError {
    ConfigError::InvalidChannel(value.into())
}

impl PcanChannelId {
    /// 轉為 PCAN-Basic 的 `TPCANHandle`。
    ///
    /// # Errors
    ///
    /// 索引超出該硬體種類可用範圍時回傳設定錯誤。
    pub fn to_handle(self) -> Result<TPCANHandle, ConfigError> {
        match self {
            Self::Usb(index) if (1..=8).contains(&index) => Ok(0x50 + u16::from(index)),
            Self::Usb(index) if (9..=16).contains(&index) => Ok(0x500 + u16::from(index)),
            Self::Pci(index) if (1..=8).contains(&index) => Ok(0x40 + u16::from(index)),
            Self::Pci(index) if (9..=16).contains(&index) => Ok(0x400 + u16::from(index)),
            Self::Lan(index) if (1..=16).contains(&index) => Ok(0x800 + u16::from(index)),
            Self::Isa(index) if (1..=8).contains(&index) => Ok(0x20 + u16::from(index)),
            Self::Pcc(index) if (1..=2).contains(&index) => Ok(0x60 + u16::from(index)),
            Self::Dng(1) => Ok(0x31),
            _ => Err(ConfigError::InvalidChannel(
                "PCAN 通道索引超出硬體種類範圍".into(),
            )),
        }
    }

    /// 從字串解析通道，例如 `"usb1"` 或 `"pci3"`。
    ///
    /// # Errors
    ///
    /// 前綴未知、索引不是十進位數字或索引超出範圍時回傳設定錯誤。
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let lower = value.to_ascii_lowercase();
        let split = lower
            .find(|character: char| character.is_ascii_digit())
            .ok_or_else(|| invalid_channel(value))?;
        let (kind, number) = lower.split_at(split);
        let index = number.parse::<u8>().map_err(|_| invalid_channel(value))?;
        let channel = match kind {
            "usb" => Self::Usb(index),
            "pci" => Self::Pci(index),
            "lan" => Self::Lan(index),
            "isa" => Self::Isa(index),
            "pcc" => Self::Pcc(index),
            "dng" => Self::Dng(index),
            _ => return Err(invalid_channel(value)),
        };
        channel.to_handle()?;
        Ok(channel)
    }
}

impl fmt::Display for PcanChannelId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usb(index) => write!(formatter, "usb{index}"),
            Self::Pci(index) => write!(formatter, "pci{index}"),
            Self::Lan(index) => write!(formatter, "lan{index}"),
            Self::Isa(index) => write!(formatter, "isa{index}"),
            Self::Pcc(index) => write!(formatter, "pcc{index}"),
            Self::Dng(index) => write!(formatter, "dng{index}"),
        }
    }
}

/// Windows 或相容性 RX 執行緒佇列滿時的處置策略。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum RxThreadPolicy {
    /// 阻塞回壓到驅動佇列，讓溢位以可觀測的 `QOVERRUN` 出現。
    #[default]
    Backpressure,
    /// 佇列滿時丟棄並計數。
    DropOnFull,
}

/// PCAN 後端專屬設定。
///
/// PCAN-Basic 具備硬體過濾能力，但只有下列過濾器能精確下推：
///
/// - accept-all 會設為 `PCAN_FILTER_OPEN`。
/// - reject-all 會設為 `PCAN_FILTER_CLOSE`。
/// - 單一、非反轉且低位 wildcard 連續的遮罩會轉成單一 `[from, to]` 區間。
///
/// 其餘規則集會把硬體過濾器設為全開，交由上層軟體過濾，並以 debug
/// 等級記錄此次回退。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PcanConfig {
    /// 要開啟的 PCAN 通道。
    pub channel: PcanChannelId,
    /// 位元率、唯聽、錯誤幀與過濾器等共通設定。
    pub common: TransportConfig,
    /// 明確的函式庫絕對路徑；`None` 使用安全標準搜尋順序。
    pub library_path: Option<PathBuf>,
    /// 專用 RX 執行緒的佇列滿政策。
    pub rx_thread_policy: RxThreadPolicy,
    /// 進階使用者提供的原始 PCAN FD 位元率字串。
    pub raw_fd_bitrate: Option<Box<str>>,
}

impl PcanConfig {
    /// 以預設共通設定建立指定通道的 PCAN 設定。
    #[must_use]
    pub fn new(channel: PcanChannelId) -> Self {
        Self {
            channel,
            common: TransportConfig::default(),
            library_path: None,
            rx_thread_policy: RxThreadPolicy::default(),
            raw_fd_bitrate: None,
        }
    }

    /// 覆寫原始 PCAN FD 位元率字串。
    ///
    /// 此字串只在 `common.bitrate` 為 CAN FD 時使用；內容會在開啟前檢查
    /// 不含內嵌 NUL。
    #[must_use]
    pub fn with_raw_fd_bitrate(mut self, bitrate: &str) -> Self {
        self.raw_fd_bitrate = Some(bitrate.into());
        self
    }
}

pub(crate) fn classic_baudrate(bitrate: Bitrate) -> Result<TPCANBaudrate, ConfigError> {
    let Bitrate::Classic { nominal } = bitrate else {
        return Err(ConfigError::InvalidBitrate(
            "CAN FD 位元率不能轉為 BTR0BTR1".into(),
        ));
    };
    match nominal {
        1_000_000 => Ok(PCAN_BAUD_1M),
        800_000 => Ok(PCAN_BAUD_800K),
        500_000 => Ok(PCAN_BAUD_500K),
        250_000 => Ok(PCAN_BAUD_250K),
        125_000 => Ok(PCAN_BAUD_125K),
        100_000 => Ok(PCAN_BAUD_100K),
        95_238 | 95_000 => Ok(PCAN_BAUD_95K),
        83_333 | 83_000 => Ok(PCAN_BAUD_83K),
        50_000 => Ok(PCAN_BAUD_50K),
        47_619 | 47_000 => Ok(PCAN_BAUD_47K),
        33_333 | 33_000 => Ok(PCAN_BAUD_33K),
        20_000 => Ok(PCAN_BAUD_20K),
        10_000 => Ok(PCAN_BAUD_10K),
        5_000 => Ok(PCAN_BAUD_5K),
        _ => Err(ConfigError::InvalidBitrate(
            format!("PCAN-Basic 不提供 {nominal} bit/s 的 BTR0BTR1 查表值").into_boxed_str(),
        )),
    }
}

fn phase(rate: u32) -> Option<(u8, u8, u8, u8)> {
    match rate {
        250_000 => Some((4, 63, 16, 16)),
        500_000 => Some((2, 63, 16, 16)),
        1_000_000 => Some((1, 63, 16, 16)),
        2_000_000 => Some((1, 31, 8, 8)),
        4_000_000 => Some((1, 15, 4, 4)),
        _ => None,
    }
}

pub(crate) fn fd_bitrate_string(
    bitrate: Bitrate,
    raw: Option<&str>,
) -> Result<Box<str>, ConfigError> {
    if let Some(raw) = raw {
        if raw.as_bytes().contains(&0) {
            return Err(ConfigError::InvalidBitrate(
                "原始 PCAN FD 位元率字串不可包含 NUL".into(),
            ));
        }
        return Ok(raw.into());
    }
    let Bitrate::Fd { nominal, data } = bitrate else {
        return Err(ConfigError::InvalidBitrate(
            "古典 CAN 位元率不能建立 FD 設定字串".into(),
        ));
    };
    let nominal_phase = match nominal {
        250_000 | 500_000 | 1_000_000 => phase(nominal),
        _ => None,
    };
    let data_phase = match data {
        500_000 | 1_000_000 | 2_000_000 | 4_000_000 => phase(data),
        _ => None,
    };
    let (Some((nbrp, nt1, nt2, nsjw)), Some((dbrp, dt1, dt2, dsjw))) = (nominal_phase, data_phase)
    else {
        return Err(ConfigError::InvalidBitrate(
            "PCAN FD 查表只支援仲裁段 250k/500k/1m 搭配資料段 500k/1m/2m/4m；可用 raw_fd_bitrate 覆寫"
                .into(),
        ));
    };
    Ok(format!(
        "f_clock_mhz=80, nom_brp={nbrp}, nom_tseg1={nt1}, nom_tseg2={nt2}, nom_sjw={nsjw}, data_brp={dbrp}, data_tseg1={dt1}, data_tseg2={dt2}, data_sjw={dsjw}"
    )
    .into_boxed_str())
}

#[cfg(test)]
mod tests {
    use pcan_core::Bitrate;

    use super::{PcanChannelId, fd_bitrate_string};

    #[test]
    fn channel_boundaries_map_to_documented_handles() {
        assert_eq!(
            PcanChannelId::Usb(1)
                .to_handle()
                .unwrap_or_else(|error| unreachable!("{error}")),
            0x51
        );
        assert_eq!(
            PcanChannelId::Usb(16)
                .to_handle()
                .unwrap_or_else(|error| unreachable!("{error}")),
            0x510
        );
        assert_eq!(
            PcanChannelId::Pci(9)
                .to_handle()
                .unwrap_or_else(|error| unreachable!("{error}")),
            0x409
        );
        assert_eq!(
            PcanChannelId::Lan(16)
                .to_handle()
                .unwrap_or_else(|error| unreachable!("{error}")),
            0x810
        );
        assert!(PcanChannelId::Pcc(3).to_handle().is_err());
        assert_eq!(
            PcanChannelId::parse("USB1").unwrap_or_else(|error| unreachable!("{error}")),
            PcanChannelId::Usb(1)
        );
    }

    #[test]
    fn fd_lookup_covers_full_cross_product() {
        for nominal in [250_000, 500_000, 1_000_000] {
            for data in [500_000, 1_000_000, 2_000_000, 4_000_000] {
                assert!(
                    fd_bitrate_string(Bitrate::Fd { nominal, data }, None).is_ok(),
                    "{nominal}/{data}"
                );
            }
        }
        assert!(
            fd_bitrate_string(
                Bitrate::Fd {
                    nominal: 125_000,
                    data: 2_000_000
                },
                None
            )
            .is_err()
        );
    }
}
