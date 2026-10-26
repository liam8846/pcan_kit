use core::fmt;

use crate::{MAX_LENGTH_HARDWARE_NAME, TPCANDevice, TPCANHandle};

/// 古典 CAN 訊息的 C 相容版面。
///
/// 欄位刻意保留 PEAK C 標頭的大寫名稱，方便逐欄核對 ABI。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[allow(non_snake_case)]
pub struct TPCANMsg {
    /// 不含格式旗標的 CAN 識別碼。
    pub ID: u32,
    /// 訊息型別位元欄位。
    pub MSGTYPE: u8,
    /// 有效資料長度。
    pub LEN: u8,
    /// 固定八位元組資料區。
    pub DATA: [u8; 8],
}

/// CAN FD 訊息的 C 相容版面。
///
/// 欄位刻意保留 PEAK C 標頭的大寫名稱，方便逐欄核對 ABI。
///
/// 絕不可使用 `repr(packed)`：原廠 C 結構沒有 packed 屬性，正常 C ABI
/// 會加入尾端兩位元組 padding，使大小為 72。壓成 70 位元組後讓 DLL
/// 寫入 72 位元組會越界並毀損堆疊。
#[repr(C)]
#[derive(Clone, Copy)]
#[allow(non_snake_case)]
pub struct TPCANMsgFD {
    /// 不含格式旗標的 CAN 識別碼。
    pub ID: u32,
    /// 訊息型別位元欄位。
    pub MSGTYPE: u8,
    /// CAN FD DLC，並非位元組長度。
    pub DLC: u8,
    /// 固定六十四位元組資料區。
    pub DATA: [u8; 64],
}

impl Default for TPCANMsgFD {
    fn default() -> Self {
        Self {
            ID: 0,
            MSGTYPE: 0,
            DLC: 0,
            DATA: [0; 64],
        }
    }
}

impl fmt::Debug for TPCANMsgFD {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TPCANMsgFD")
            .field("ID", &self.ID)
            .field("MSGTYPE", &self.MSGTYPE)
            .field("DLC", &self.DLC)
            .finish_non_exhaustive()
    }
}

/// 已連接 PCAN 通道資訊的 C 相容版面。
///
/// 絕不可使用 `repr(packed)`：原廠 C 結構採用一般 C ABI 對齊，裝置名稱
/// 之後具有三位元組 padding。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TPCANChannelInformation {
    /// PCAN 通道控制代碼。
    pub channel_handle: TPCANHandle,
    /// PCAN 硬體種類。
    pub device_type: TPCANDevice,
    /// 裝置上的控制器編號。
    pub controller_number: u8,
    /// 通道硬體能力位元。
    pub device_features: u32,
    /// NUL 結尾的硬體名稱。
    ///
    /// 使用 `u8` 而非 `c_char`，ABI 完全相同，且可避免 `c_char`
    /// 在 ARM 與 x86 平台的有號性差異；這也與 [`TPCANMsg::DATA`] 一致。
    pub device_name: [u8; MAX_LENGTH_HARDWARE_NAME],
    /// 裝置識別碼。
    pub device_id: u32,
    /// 通道目前的占用狀況。
    pub channel_condition: u32,
}

impl Default for TPCANChannelInformation {
    fn default() -> Self {
        Self {
            channel_handle: 0,
            device_type: 0,
            controller_number: 0,
            device_features: 0,
            device_name: [0; MAX_LENGTH_HARDWARE_NAME],
            device_id: 0,
            channel_condition: 0,
        }
    }
}

impl fmt::Debug for TPCANChannelInformation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = core::ffi::CStr::from_bytes_until_nul(&self.device_name)
            .map_or(self.device_name.as_slice(), core::ffi::CStr::to_bytes);
        let name = core::str::from_utf8(bytes).map_or("", |value| value);
        formatter
            .debug_struct("TPCANChannelInformation")
            .field("channel_handle", &self.channel_handle)
            .field("device_type", &self.device_type)
            .field("controller_number", &self.controller_number)
            .field("device_features", &self.device_features)
            .field("device_name", &name)
            .field("device_id", &self.device_id)
            .field("channel_condition", &self.channel_condition)
            .finish()
    }
}

/// 古典 CAN 訊息的硬體時間戳。
///
/// 欄位名稱依原廠 C 標頭保留；結構在兩個支援平台具有相同 ABI。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
#[allow(non_snake_case)]
pub struct TPCANTimestamp {
    /// 低 32 位毫秒數。
    pub millis: u32,
    /// 毫秒計數器的溢位次數。
    pub millis_overflow: u16,
    /// 當前毫秒內的微秒數。
    pub micros: u16,
}

impl TPCANTimestamp {
    /// 將分段時間戳轉為完整微秒數。
    #[must_use]
    pub const fn to_micros(self) -> u64 {
        self.millis_overflow as u64 * (1_u64 << 32) * 1_000
            + self.millis as u64 * 1_000
            + self.micros as u64
    }
}

const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(size_of::<TPCANMsg>() == 16 && align_of::<TPCANMsg>() == 4);
    assert!(offset_of!(TPCANMsg, DATA) == 6);
    assert!(size_of::<TPCANMsgFD>() == 72 && align_of::<TPCANMsgFD>() == 4);
    assert!(offset_of!(TPCANMsgFD, DATA) == 6);
    assert!(size_of::<TPCANTimestamp>() == 8);
    assert!(offset_of!(TPCANTimestamp, micros) == 6);
    assert!(
        size_of::<TPCANChannelInformation>() == 52 && align_of::<TPCANChannelInformation>() == 4
    );
    assert!(offset_of!(TPCANChannelInformation, device_name) == 8);
    assert!(offset_of!(TPCANChannelInformation, device_id) == 44);
    assert!(offset_of!(TPCANChannelInformation, channel_condition) == 48);
};

#[cfg(test)]
mod tests {
    use super::TPCANTimestamp;

    #[test]
    fn timestamp_converts_overflow_and_fraction() {
        let value = TPCANTimestamp {
            millis: 7,
            millis_overflow: 2,
            micros: 321,
        };
        assert_eq!(value.to_micros(), 2 * (1_u64 << 32) * 1_000 + 7_321);
    }
}
