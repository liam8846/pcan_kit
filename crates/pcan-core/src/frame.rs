use core::fmt;
use core::hash::{Hash, Hasher};

use crate::error::{ConfigError, FrameKind};
use crate::id::CanId;

bitflags::bitflags! {
    /// 幀旗標。
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    pub struct FrameFlags: u8 {
        /// 遠端請求幀，僅古典幀合法。
        const RTR = 1 << 0;
        /// CAN FD 格式旗標。
        const FD = 1 << 1;
        /// 位元率切換，必須與 CAN FD 同時使用。
        const BRS = 1 << 2;
        /// 錯誤狀態指示，僅接收方向有意義。
        const ESI = 1 << 3;
    }
}

/// 將 DLC 轉為位元組長度。
///
/// `0..=8` 直接對應，CAN FD 的 `9..=15` 對應
/// `12、16、20、24、32、48、64`；大於 15 的輸入視同 15。
#[must_use]
pub const fn dlc_to_len(dlc: u8) -> u8 {
    match dlc {
        0..=8 => dlc,
        9 => 12,
        10 => 16,
        11 => 20,
        12 => 24,
        13 => 32,
        14 => 48,
        _ => 64,
    }
}

/// 將合法的 CAN FD 位元組長度轉為 DLC。
///
/// 非協定規定的長度會回傳 `None`。
#[must_use]
pub const fn len_to_dlc(len: u8) -> Option<u8> {
    match len {
        0..=8 => Some(len),
        12 => Some(9),
        16 => Some(10),
        20 => Some(11),
        24 => Some(12),
        32 => Some(13),
        48 => Some(14),
        64 => Some(15),
        _ => None,
    }
}

/// 將任意長度向上取整到最近的合法 CAN FD 長度。
///
/// 長度超過 64 時回傳 `None`。
#[must_use]
pub const fn round_up_fd_len(len: u8) -> Option<u8> {
    match len {
        0..=8 => Some(len),
        9..=12 => Some(12),
        13..=16 => Some(16),
        17..=20 => Some(20),
        21..=24 => Some(24),
        25..=32 => Some(32),
        33..=48 => Some(48),
        49..=64 => Some(64),
        _ => None,
    }
}

/// 一個 CAN 幀。
///
/// 此型別固定為 72 位元組且可 `Copy`，熱路徑只搬移堆疊值、不配置記憶體。
/// 所有建構子都會將未使用的資料尾端歸零，但相等與雜湊仍只比較有效欄位，
/// 因而不會讓後端接收緩衝區的尾端內容影響協定語意。
#[repr(C, align(8))]
#[derive(Clone, Copy, Eq)]
pub struct Frame {
    id: CanId,
    len: u8,
    flags: FrameFlags,
    _reserved: [u8; 2],
    data: [u8; 64],
}

impl Frame {
    /// 建立 CAN 2.0 古典資料幀。
    ///
    /// # Errors
    ///
    /// 酬載超過 8 位元組時回傳 [`ConfigError::InvalidPayloadLen`]。
    pub fn new(id: CanId, data: &[u8]) -> Result<Self, ConfigError> {
        if data.len() > 8 {
            return Err(ConfigError::InvalidPayloadLen {
                len: data.len(),
                kind: FrameKind::Classic,
            });
        }
        Self::from_parts(
            id,
            data,
            data.len(),
            FrameFlags::empty(),
            FrameKind::Classic,
        )
    }

    /// 建立 CAN FD 資料幀。
    ///
    /// 若酬載不是合法的 CAN FD 長度，會向上補零至最近的合法長度；例如
    /// 9 位元組會形成長度 12 的幀，新增的 3 位元組皆為零。
    ///
    /// # Errors
    ///
    /// 酬載超過 64 位元組時回傳 [`ConfigError::InvalidPayloadLen`]。
    pub fn new_fd(id: CanId, data: &[u8], brs: bool) -> Result<Self, ConfigError> {
        let byte_len = u8::try_from(data.len())
            .ok()
            .and_then(round_up_fd_len)
            .ok_or(ConfigError::InvalidPayloadLen {
                len: data.len(),
                kind: FrameKind::Fd,
            })?;
        let mut flags = FrameFlags::FD;
        if brs {
            flags |= FrameFlags::BRS;
        }
        Self::from_parts(id, data, usize::from(byte_len), flags, FrameKind::Fd)
    }

    /// 建立 CAN 2.0 遠端請求幀。
    ///
    /// 遠端請求幀不含資料欄位；[`len`](Self::len) 會回傳要求對端回覆的
    /// 資料長度，而 [`data`](Self::data) 永遠為空。
    ///
    /// # Errors
    ///
    /// `dlc` 大於 8 時回傳 [`ConfigError::InvalidPayloadLen`]。
    pub fn remote(id: CanId, dlc: u8) -> Result<Self, ConfigError> {
        if dlc > 8 {
            return Err(ConfigError::InvalidPayloadLen {
                len: usize::from(dlc),
                kind: FrameKind::Remote,
            });
        }
        Self::from_parts(
            id,
            &[],
            usize::from(dlc),
            FrameFlags::RTR,
            FrameKind::Remote,
        )
    }

    fn from_parts(
        id: CanId,
        source: &[u8],
        len: usize,
        flags: FrameFlags,
        kind: FrameKind,
    ) -> Result<Self, ConfigError> {
        if flags.contains(FrameFlags::RTR) && flags.contains(FrameFlags::FD) {
            return Err(ConfigError::InvalidFlags("RTR 與 FD 不可同時設定"));
        }
        if flags.contains(FrameFlags::BRS) && !flags.contains(FrameFlags::FD) {
            return Err(ConfigError::InvalidFlags("BRS 必須搭配 FD"));
        }
        if len > 64 || source.len() > len {
            return Err(ConfigError::InvalidPayloadLen { len, kind });
        }

        let Ok(encoded_len) = u8::try_from(len) else {
            return Err(ConfigError::InvalidPayloadLen { len, kind });
        };
        let mut bytes = [0; 64];
        bytes[..source.len()].copy_from_slice(source);
        Ok(Self {
            id,
            len: encoded_len,
            flags,
            _reserved: [0; 2],
            data: bytes,
        })
    }

    /// 取得幀的 CAN 識別碼。
    #[must_use]
    pub const fn id(&self) -> CanId {
        self.id
    }

    /// 取得包含 FD padding 的有效資料切片。
    ///
    /// 遠端請求幀在協定上沒有資料欄位，因此即使 [`len`](Self::len) 非零，
    /// 此方法仍會回傳空切片。
    #[must_use]
    pub fn data(&self) -> &[u8] {
        let len = if self.is_remote() {
            0
        } else {
            usize::from(self.len)
        };
        &self.data[..len]
    }

    /// 取得有效資料的可變切片，供週期傳送在原地更新。
    ///
    /// 遠端請求幀沒有資料欄位，因此此方法會回傳空切片。
    #[must_use]
    pub fn data_mut(&mut self) -> &mut [u8] {
        let len = if self.is_remote() {
            0
        } else {
            usize::from(self.len)
        };
        &mut self.data[..len]
    }

    /// 取得有效資料長度。
    ///
    /// 對遠端請求幀而言，此值是要求對端回覆的資料長度；幀本身沒有資料欄位，
    /// 因此 [`data`](Self::data) 仍為空。
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// 判斷有效資料長度是否為零。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 取得幀旗標。
    #[must_use]
    pub const fn flags(&self) -> FrameFlags {
        self.flags
    }

    /// 判斷是否為 CAN FD 幀。
    #[must_use]
    pub const fn is_fd(&self) -> bool {
        self.flags.contains(FrameFlags::FD)
    }

    /// 判斷是否為遠端請求幀。
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        self.flags.contains(FrameFlags::RTR)
    }

    /// 判斷是否啟用 CAN FD 位元率切換。
    #[must_use]
    pub const fn is_brs(&self) -> bool {
        self.flags.contains(FrameFlags::BRS)
    }

    /// 取得由有效資料長度反查的 DLC。
    #[must_use]
    pub const fn dlc(&self) -> u8 {
        match len_to_dlc(self.len) {
            Some(value) => value,
            None => 15,
        }
    }
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.len == other.len
            && self.flags == other.flags
            && self.data() == other.data()
    }
}

impl Hash for Frame {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.len.hash(state);
        self.flags.hash(state);
        self.data().hash(state);
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("id", &self.id)
            .field("len", &self.len)
            .field("flags", &self.flags)
            .field("data", &self.data())
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Frame {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut frame = serializer.serialize_struct("Frame", 4)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "id", &self.id)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "len", &self.len)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "flags", &self.flags)?;
        serde::ser::SerializeStruct::serialize_field(&mut frame, "data", self.data())?;
        serde::ser::SerializeStruct::end(frame)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Frame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct FrameRepr {
            id: CanId,
            len: u8,
            flags: FrameFlags,
            data: Vec<u8>,
        }

        let repr = <FrameRepr as serde::Deserialize>::deserialize(deserializer)?;
        let is_remote = repr.flags.contains(FrameFlags::RTR);
        let is_fd = repr.flags.contains(FrameFlags::FD);

        let kind = if is_remote {
            if is_fd {
                return Err(serde::de::Error::custom("RTR 與 FD 不可同時設定"));
            }
            if repr.len > 8 {
                return Err(serde::de::Error::custom("遠端請求幀的資料長度不可超過 8"));
            }
            if !repr.data.is_empty() {
                return Err(serde::de::Error::custom("遠端請求幀不可包含資料欄位"));
            }
            FrameKind::Remote
        } else if is_fd {
            if len_to_dlc(repr.len).is_none() {
                return Err(serde::de::Error::custom("CAN FD 幀長度不是合法的 DLC 長度"));
            }
            if repr.data.len() != usize::from(repr.len) {
                return Err(serde::de::Error::custom(
                    "CAN FD 幀的 len 與 data 長度不一致",
                ));
            }
            FrameKind::Fd
        } else {
            if repr.len > 8 {
                return Err(serde::de::Error::custom("古典 CAN 幀的資料長度不可超過 8"));
            }
            if repr.data.len() != usize::from(repr.len) {
                return Err(serde::de::Error::custom(
                    "古典 CAN 幀的 len 與 data 長度不一致",
                ));
            }
            FrameKind::Classic
        };

        Self::from_parts(repr.id, &repr.data, usize::from(repr.len), repr.flags, kind)
            .map_err(serde::de::Error::custom)
    }
}

/// 幀的時間戳。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Timestamp {
    micros: u64,
    source: TimestampSource,
}

impl Timestamp {
    /// 以自後端基準點起算的微秒數與來源建立時間戳。
    #[must_use]
    pub const fn new(micros: u64, source: TimestampSource) -> Self {
        Self { micros, source }
    }

    /// 取得自後端基準點起算的微秒數。
    #[must_use]
    pub const fn micros(self) -> u64 {
        self.micros
    }

    /// 取得時間戳來源。
    #[must_use]
    pub const fn source(self) -> TimestampSource {
        self.source
    }
}

/// 時間戳來源，用於判斷精度與可比較性。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum TimestampSource {
    /// 由硬體或驅動提供，通常精度最高。
    Hardware,
    /// 由核心提供，例如 `SocketCAN` 的 `SO_TIMESTAMPNS`。
    Kernel,
    /// 由本函式庫在使用者空間記錄，通常精度最低。
    #[default]
    Software,
}

/// 收到的幀，附帶時間戳與方向資訊。
///
/// 時間戳只存在於此觀測單位，而不放在協定單位 [`Frame`] 中；待傳送的幀
/// 尚未發生，並沒有接收時間戳。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct RxFrame {
    /// 收到的協定幀。
    pub frame: Frame,
    /// 後端提供或函式庫記錄的接收時間。
    pub timestamp: Timestamp,
    /// 是否為本地送出後由後端回送的回音。
    pub is_echo: bool,
}

impl RxFrame {
    /// 建立附帶接收中繼資料的幀。
    #[must_use]
    pub const fn new(frame: Frame, timestamp: Timestamp, is_echo: bool) -> Self {
        Self {
            frame,
            timestamp,
            is_echo,
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<Frame>() == 72);
    assert!(core::mem::align_of::<Frame>() == 8);
};

#[cfg(test)]
mod tests {
    use core::mem::{align_of, size_of};

    use super::{Frame, FrameFlags, FrameKind, dlc_to_len, len_to_dlc, round_up_fd_len};
    use crate::id::CanId;

    fn id() -> CanId {
        CanId::standard(0x123).unwrap_or_else(|error| unreachable!("{error}"))
    }

    #[test]
    fn converts_all_dlc_and_lengths() {
        let expected = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];
        for (dlc, len) in (0_u8..=15).zip(expected) {
            assert_eq!(dlc_to_len(dlc), len);
        }
        for len in 0_u8..=64 {
            let expected_dlc = [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64]
                .iter()
                .position(|candidate| *candidate == len)
                .and_then(|index| u8::try_from(index).ok());
            assert_eq!(len_to_dlc(len), expected_dlc);

            let expected_rounded = [0_u8, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64]
                .into_iter()
                .find(|candidate| *candidate >= len);
            assert_eq!(round_up_fd_len(len), expected_rounded);
        }
    }

    #[test]
    fn validates_frame_construction_and_padding() {
        assert!(Frame::new(id(), &[0; 9]).is_err());
        assert!(Frame::new_fd(id(), &[0; 65], false).is_err());

        let fd = Frame::new_fd(id(), &[1; 9], true).unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(fd.len(), 12);
        assert_eq!(&fd.data()[..9], &[1; 9]);
        assert_eq!(&fd.data()[9..12], &[0; 3]);
        assert!(fd.is_fd());
        assert!(fd.is_brs());
        assert_eq!(fd.dlc(), 9);

        let remote = Frame::remote(id(), 8).unwrap_or_else(|error| unreachable!("{error}"));
        assert!(remote.is_remote());
        assert_eq!(remote.flags(), FrameFlags::RTR);
        assert_eq!(remote.len(), 8);
        assert!(remote.data().is_empty());
        let mut remote = remote;
        assert!(remote.data_mut().is_empty());

        assert!(
            Frame::from_parts(
                id(),
                &[],
                0,
                FrameFlags::RTR | FrameFlags::FD,
                FrameKind::Fd,
            )
            .is_err()
        );
    }

    #[test]
    fn has_fixed_layout() {
        assert_eq!(size_of::<Frame>(), 72);
        assert_eq!(align_of::<Frame>(), 8);
    }

    #[test]
    fn equality_ignores_unused_tail() {
        let first = Frame::new(id(), &[1, 2]).unwrap_or_else(|error| unreachable!("{error}"));
        let mut second = first;
        second.data[63] = 0xff;
        assert_eq!(first, second);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trip_uses_only_effective_data() -> Result<(), Box<dyn std::error::Error>> {
        let frame = Frame::new_fd(id(), &[1; 9], true)?;
        let encoded = serde_json::to_string(&frame)?;
        let value: serde_json::Value = serde_json::from_str(&encoded)?;
        let serialized_len = value
            .get("data")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len);
        assert_eq!(serialized_len, Some(12));

        let decoded: Frame = serde_json::from_str(&encoded)?;
        assert_eq!(decoded, frame);
        assert_eq!(decoded.data(), frame.data());
        Ok(())
    }
}
