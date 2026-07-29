use core::fmt;
use core::str;
use std::ffi::CStr;
use std::path::{Path, PathBuf};

use pcan_basic_sys::{
    FEATURE_DELAY_CAPABLE, FEATURE_FD_CAPABLE, FEATURE_IO_CAPABLE, PCAN_ATTACHED_CHANNELS_COUNT,
    PCAN_CHANNEL_AVAILABLE, PCAN_CHANNEL_OCCUPIED, PCAN_CHANNEL_PCANVIEW, PCAN_CHANNEL_UNAVAILABLE,
    PCAN_DNG, PCAN_ERROR_ILLPARAMTYPE, PCAN_ERROR_ILLPARAMVAL, PCAN_ISA, PCAN_LAN, PCAN_NONEBUS,
    PCAN_PCC, PCAN_PCI, PCAN_PEAKCAN, PCAN_USB, PCAN_VIRTUAL, PcanApi, StatusOutcome,
    TPCANChannelInformation, TPCANHandle, classify, load, load_from,
};
use pcan_core::{BackendError, Error, FaultKind};

use crate::channel::{backend_error, open_task_error};
use crate::config::PcanChannelId;

const MAX_CHANNEL_COUNT: u32 = 1_024;
const ENUMERATION_ATTEMPTS: usize = 3;

/// PCAN 硬體種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum PcanDeviceKind {
    /// PEAK-CAN 硬體。
    PeakCan,
    /// ISA 介面卡。
    Isa,
    /// Dongle 介面。
    Dongle,
    /// PCI 或 `PCIe` 介面卡。
    Pci,
    /// USB 介面。
    Usb,
    /// PC Card 介面。
    PcCard,
    /// 虛擬 PCAN 介面。
    Virtual,
    /// LAN 介面。
    Lan,
    /// 驅動回報了本版尚未認識的硬體種類碼。
    Unknown(u8),
}

/// PCAN 通道目前的占用狀況。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum PcanChannelCondition {
    /// 通道目前無法使用。
    Unavailable,
    /// 通道可供連線。
    Available,
    /// 通道已由其他程式占用。
    Occupied,
    /// 通道可供連線，且已由 PCAN-View 占用。
    AvailableAndOccupied,
    /// 驅動回報了本版尚未認識的占用狀況碼。
    Unknown(u32),
}

bitflags::bitflags! {
    /// 驅動回報的通道硬體能力。
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
    pub struct PcanChannelFeatures: u32 {
        /// 支援 CAN FD。
        const FD_CAPABLE = FEATURE_FD_CAPABLE;
        /// 支援幀間延遲設定。
        const DELAY_CAPABLE = FEATURE_DELAY_CAPABLE;
        /// 支援數位或類比 I/O。
        const IO_CAPABLE = FEATURE_IO_CAPABLE;
    }
}

/// 一筆已連接 PCAN 通道的資訊。
///
/// 固定大小 `Copy` 值，列舉時不對每一筆做堆積配置。
#[derive(Clone, Copy)]
pub struct PcanChannelInfo {
    handle: TPCANHandle,
    channel_id: Option<PcanChannelId>,
    device: PcanDeviceKind,
    controller_number: u8,
    features: PcanChannelFeatures,
    device_id: u32,
    condition: PcanChannelCondition,
    name: [u8; 32],
    name_len: u8,
}

impl PcanChannelInfo {
    /// 回傳 PCAN-Basic 通道控制代碼。
    #[must_use]
    pub const fn handle(&self) -> TPCANHandle {
        self.handle
    }

    /// 回傳可辨識的 PCAN 通道位址。
    #[must_use]
    pub const fn channel_id(&self) -> Option<PcanChannelId> {
        self.channel_id
    }

    /// 回傳 PCAN 硬體種類。
    #[must_use]
    pub const fn device(&self) -> PcanDeviceKind {
        self.device
    }

    /// 回傳裝置上的控制器編號。
    #[must_use]
    pub const fn controller_number(&self) -> u8 {
        self.controller_number
    }

    /// 回傳通道硬體能力。
    #[must_use]
    pub const fn features(&self) -> PcanChannelFeatures {
        self.features
    }

    /// 回傳驅動提供的裝置識別碼。
    #[must_use]
    pub const fn device_id(&self) -> u32 {
        self.device_id
    }

    /// 回傳通道目前的占用狀況。
    #[must_use]
    pub const fn condition(&self) -> PcanChannelCondition {
        self.condition
    }

    /// 借用內嵌緩衝中的硬體名稱，不會進行配置。
    #[must_use]
    pub fn name(&self) -> &str {
        let length = usize::from(self.name_len);
        str::from_utf8(&self.name[..length]).map_or("", core::convert::identity)
    }

    /// 判斷通道目前是否可供連線。
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(
            self.condition,
            PcanChannelCondition::Available | PcanChannelCondition::AvailableAndOccupied
        )
    }

    /// 判斷通道是否支援 CAN FD。
    #[must_use]
    pub const fn supports_fd(&self) -> bool {
        self.features.contains(PcanChannelFeatures::FD_CAPABLE)
    }
}

impl fmt::Debug for PcanChannelInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PcanChannelInfo")
            .field("handle", &self.handle)
            .field("channel_id", &self.channel_id)
            .field("device", &self.device)
            .field("controller_number", &self.controller_number)
            .field("features", &self.features)
            .field("device_id", &self.device_id)
            .field("condition", &self.condition)
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

fn device_kind(raw: u8) -> PcanDeviceKind {
    match raw {
        PCAN_PEAKCAN => PcanDeviceKind::PeakCan,
        PCAN_ISA => PcanDeviceKind::Isa,
        PCAN_DNG => PcanDeviceKind::Dongle,
        PCAN_PCI => PcanDeviceKind::Pci,
        PCAN_USB => PcanDeviceKind::Usb,
        PCAN_PCC => PcanDeviceKind::PcCard,
        PCAN_VIRTUAL => PcanDeviceKind::Virtual,
        PCAN_LAN => PcanDeviceKind::Lan,
        value => PcanDeviceKind::Unknown(value),
    }
}

fn channel_condition(raw: u32) -> PcanChannelCondition {
    match raw {
        PCAN_CHANNEL_UNAVAILABLE => PcanChannelCondition::Unavailable,
        PCAN_CHANNEL_AVAILABLE => PcanChannelCondition::Available,
        PCAN_CHANNEL_OCCUPIED => PcanChannelCondition::Occupied,
        PCAN_CHANNEL_PCANVIEW => PcanChannelCondition::AvailableAndOccupied,
        value => PcanChannelCondition::Unknown(value),
    }
}

fn info_from_raw(raw: &TPCANChannelInformation) -> PcanChannelInfo {
    let bytes =
        CStr::from_bytes_until_nul(&raw.device_name).map_or(&raw.device_name[..32], CStr::to_bytes);
    let valid_name = match str::from_utf8(bytes) {
        Ok(name) => name.as_bytes(),
        Err(error) => &bytes[..error.valid_up_to()],
    };
    let mut name = [0_u8; 32];
    name[..valid_name.len()].copy_from_slice(valid_name);

    PcanChannelInfo {
        handle: raw.channel_handle,
        channel_id: PcanChannelId::from_handle(raw.channel_handle),
        device: device_kind(raw.device_type),
        controller_number: raw.controller_number,
        features: PcanChannelFeatures::from_bits_retain(raw.device_features),
        device_id: raw.device_id,
        condition: channel_condition(raw.channel_condition),
        name,
        name_len: u8::try_from(valid_name.len()).map_or(0, |length| length),
    }
}

fn check_count_status(api: &PcanApi, status: u32) -> Result<(), Error> {
    if status & (PCAN_ERROR_ILLPARAMTYPE | PCAN_ERROR_ILLPARAMVAL) != 0 {
        return Err(Error::Unsupported(
            "載入的 PCAN-Basic 版本不提供通道列舉（PCAN_ATTACHED_CHANNELS）",
        ));
    }
    match classify(status) {
        StatusOutcome::Ok { .. } => Ok(()),
        _ => Err(Error::Io(backend_error(
            api,
            status,
            "CAN_GetValue(PCAN_ATTACHED_CHANNELS_COUNT)",
            FaultKind::Permanent,
        ))),
    }
}

fn unreasonable_count_error(count: u32) -> Error {
    Error::Io(BackendError::PcanBasic {
        code: 0,
        text: format!("驅動回報不合理的 PCAN 通道數量：{count}").into_boxed_str(),
        op: "CAN_GetValue(PCAN_ATTACHED_CHANNELS_COUNT)",
        kind: FaultKind::Permanent,
    })
}

fn hardware_changed_error(status: u32) -> Error {
    Error::Io(BackendError::PcanBasic {
        code: status,
        text: "列舉期間硬體變動，重試三次後仍無法取得一致的通道資訊".into(),
        op: "CAN_GetValue(PCAN_ATTACHED_CHANNELS)",
        kind: FaultKind::Transient,
    })
}

fn list_blocking(api: &PcanApi) -> Result<Box<[PcanChannelInfo]>, Error> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let (count_status, count) = api.get_value_u32(PCAN_NONEBUS, PCAN_ATTACHED_CHANNELS_COUNT);
        check_count_status(api, count_status)?;
        if count == 0 {
            return Ok(Vec::new().into_boxed_slice());
        }
        if count > MAX_CHANNEL_COUNT {
            return Err(unreasonable_count_error(count));
        }
        let count = usize::try_from(count).map_err(|_| unreasonable_count_error(count))?;
        let mut buffer = vec![TPCANChannelInformation::default(); count];
        let status = api.attached_channels(&mut buffer);
        if status & PCAN_ERROR_ILLPARAMVAL != 0 {
            // 熱插拔可能讓讀取數量與抓取陣列之間的裝置數量改變；重新讀取
            // 數量並配置緩衝，避免把短暫競態誤報為永久故障。
            if attempt < ENUMERATION_ATTEMPTS {
                continue;
            }
            break Err(hardware_changed_error(status));
        }
        match classify(status) {
            StatusOutcome::Ok { .. } => {
                return Ok(buffer
                    .iter()
                    .map(info_from_raw)
                    .collect::<Vec<_>>()
                    .into_boxed_slice());
            }
            _ => {
                return Err(Error::Io(backend_error(
                    api,
                    status,
                    "CAN_GetValue(PCAN_ATTACHED_CHANNELS)",
                    FaultKind::Permanent,
                )));
            }
        }
    }
}

/// 列舉目前連接到本機的所有 PCAN 通道。
///
/// # Errors
///
/// 函式庫無法載入、驅動不支援通道列舉、驅動回報錯誤，或阻塞工作
/// 無法完成時回傳錯誤。
pub async fn list_channels() -> Result<Box<[PcanChannelInfo]>, Error> {
    tokio::task::spawn_blocking(|| {
        let api = load()?;
        list_blocking(&api)
    })
    .await
    .map_err(|source| open_task_error(source.to_string(), "等待 PCAN 通道列舉工作"))?
}

/// 以指定絕對路徑的 PCAN-Basic 函式庫列舉通道。
///
/// # Errors
///
/// 路徑不是絕對路徑、函式庫無法載入、驅動不支援通道列舉、驅動回報
/// 錯誤，或阻塞工作無法完成時回傳錯誤。
pub async fn list_channels_from(library_path: &Path) -> Result<Box<[PcanChannelInfo]>, Error> {
    let library_path = PathBuf::from(library_path);
    tokio::task::spawn_blocking(move || {
        let api = load_from(&library_path)?;
        list_blocking(&api)
    })
    .await
    .map_err(|source| open_task_error(source.to_string(), "等待 PCAN 通道列舉工作"))?
}

#[cfg(test)]
mod tests {
    use pcan_basic_sys::{
        FEATURE_DELAY_CAPABLE, FEATURE_FD_CAPABLE, FEATURE_IO_CAPABLE, PCAN_CHANNEL_AVAILABLE,
        PCAN_CHANNEL_OCCUPIED, PCAN_CHANNEL_PCANVIEW, PCAN_CHANNEL_UNAVAILABLE, PCAN_DNG, PCAN_ISA,
        PCAN_LAN, PCAN_PCC, PCAN_PCI, PCAN_PEAKCAN, PCAN_USB, PCAN_VIRTUAL,
        TPCANChannelInformation,
    };

    use super::{PcanChannelCondition, PcanChannelFeatures, PcanDeviceKind, info_from_raw};
    use crate::PcanChannelId;

    fn raw_with_name(name: &[u8]) -> TPCANChannelInformation {
        let mut raw = TPCANChannelInformation::default();
        raw.device_name[..name.len()].copy_from_slice(name);
        raw
    }

    #[test]
    fn parses_nul_terminated_name() {
        let info = info_from_raw(&raw_with_name(b"PCAN-USB\0unused"));
        assert_eq!(info.name(), "PCAN-USB");
    }

    #[test]
    fn caps_full_name_at_thirty_two_bytes() {
        let raw = raw_with_name(b"abcdefghijklmnopqrstuvwxyz1234567");
        let info = info_from_raw(&raw);
        assert_eq!(info.name(), "abcdefghijklmnopqrstuvwxyz123456");
    }

    #[test]
    fn invalid_utf8_becomes_empty_name() {
        let info = info_from_raw(&raw_with_name(&[0xff, 0]));
        assert_eq!(info.name(), "");
    }

    #[test]
    fn preserves_valid_prefix_when_full_name_splits_utf8_character() {
        let mut name = [b'a'; 33];
        name[30..].copy_from_slice(&[0xf0, 0x9f, 0x98]);
        let info = info_from_raw(&raw_with_name(&name));
        assert_eq!(info.name(), "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn maps_raw_fields_and_preserves_unknown_bits() {
        for (raw, wanted) in [
            (PCAN_PEAKCAN, PcanDeviceKind::PeakCan),
            (PCAN_ISA, PcanDeviceKind::Isa),
            (PCAN_DNG, PcanDeviceKind::Dongle),
            (PCAN_PCI, PcanDeviceKind::Pci),
            (PCAN_USB, PcanDeviceKind::Usb),
            (PCAN_PCC, PcanDeviceKind::PcCard),
            (PCAN_VIRTUAL, PcanDeviceKind::Virtual),
            (PCAN_LAN, PcanDeviceKind::Lan),
            (0xfe, PcanDeviceKind::Unknown(0xfe)),
        ] {
            let value = TPCANChannelInformation {
                device_type: raw,
                ..TPCANChannelInformation::default()
            };
            assert_eq!(info_from_raw(&value).device(), wanted);
        }

        for (raw, wanted) in [
            (PCAN_CHANNEL_UNAVAILABLE, PcanChannelCondition::Unavailable),
            (PCAN_CHANNEL_AVAILABLE, PcanChannelCondition::Available),
            (PCAN_CHANNEL_OCCUPIED, PcanChannelCondition::Occupied),
            (
                PCAN_CHANNEL_PCANVIEW,
                PcanChannelCondition::AvailableAndOccupied,
            ),
            (9, PcanChannelCondition::Unknown(9)),
        ] {
            let value = TPCANChannelInformation {
                channel_condition: raw,
                ..TPCANChannelInformation::default()
            };
            assert_eq!(info_from_raw(&value).condition(), wanted);
        }

        let raw = TPCANChannelInformation {
            channel_handle: 0x51,
            controller_number: 3,
            device_features: FEATURE_FD_CAPABLE | FEATURE_DELAY_CAPABLE | FEATURE_IO_CAPABLE | 0x80,
            device_id: 42,
            channel_condition: PCAN_CHANNEL_AVAILABLE,
            ..TPCANChannelInformation::default()
        };
        let info = info_from_raw(&raw);
        assert_eq!(info.channel_id(), Some(PcanChannelId::Usb(1)));
        assert_eq!(info.controller_number(), 3);
        assert_eq!(info.device_id(), 42);
        assert!(info.is_available());
        assert!(info.supports_fd());
        assert!(info.features().contains(
            PcanChannelFeatures::FD_CAPABLE
                | PcanChannelFeatures::DELAY_CAPABLE
                | PcanChannelFeatures::IO_CAPABLE
        ));
        assert_eq!(info.features().bits() & 0x80, 0x80);
    }
}
