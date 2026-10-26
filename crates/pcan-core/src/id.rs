use core::fmt;

use crate::error::{ConfigError, IdKind};

/// CAN 識別碼中的擴充格式旗標。
pub const EXT_FLAG: u32 = 0x8000_0000;

const STANDARD_MAX: u32 = 0x7ff;
const EXTENDED_MAX: u32 = 0x1fff_ffff;

/// CAN 識別碼。
///
/// 內部以單一 `u32` 表示，bit 31 為擴充旗標。這種表示可直接對應兩種
/// 後端的線上格式，也能讓過濾器用單一位元運算完成比對。
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanId(u32);

impl CanId {
    /// 建立 11-bit 標準識別碼。
    ///
    /// # Errors
    ///
    /// `raw` 大於 `0x7ff` 時回傳 [`ConfigError::IdOutOfRange`]。
    pub const fn standard(raw: u16) -> Result<Self, ConfigError> {
        if raw as u32 <= STANDARD_MAX {
            Ok(Self(raw as u32))
        } else {
            Err(ConfigError::IdOutOfRange {
                value: raw as u32,
                kind: IdKind::Standard,
            })
        }
    }

    /// 建立 29-bit 擴充識別碼。
    ///
    /// # Errors
    ///
    /// `raw` 大於 `0x1fff_ffff` 時回傳 [`ConfigError::IdOutOfRange`]。
    pub const fn extended(raw: u32) -> Result<Self, ConfigError> {
        if raw <= EXTENDED_MAX {
            Ok(Self(raw | EXT_FLAG))
        } else {
            Err(ConfigError::IdOutOfRange {
                value: raw,
                kind: IdKind::Extended,
            })
        }
    }

    /// 取得不含擴充旗標的原始識別碼。
    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0 & EXTENDED_MAX
    }

    /// 判斷是否為 29-bit 擴充識別碼。
    #[must_use]
    pub const fn is_extended(self) -> bool {
        self.0 & EXT_FLAG != 0
    }

    /// 取得包含 bit 31 擴充旗標的後端交換格式。
    ///
    /// 後端 crate 可直接將此值轉成驅動所需的識別碼與格式旗標。
    #[must_use]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    /// 從包含 bit 31 擴充旗標的後端交換格式建立識別碼。
    ///
    /// 未定義的高位元會被清除；標準格式會限制為 11 bit，擴充格式會限制為
    /// 29 bit，確保產生的值維持有效。
    #[must_use]
    pub const fn from_bits(bits: u32) -> Self {
        if bits & EXT_FLAG != 0 {
            Self((bits & EXTENDED_MAX) | EXT_FLAG)
        } else {
            Self(bits & STANDARD_MAX)
        }
    }
}

impl fmt::Debug for CanId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_extended() {
            write!(formatter, "CanId({:#X}, ext)", self.as_raw())
        } else {
            write!(formatter, "CanId({:#X})", self.as_raw())
        }
    }
}

impl fmt::Display for CanId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_extended() {
            write!(formatter, "{:08X}x", self.as_raw())
        } else {
            write!(formatter, "{:03X}", self.as_raw())
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for CanId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u32(self.to_bits())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for CanId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bits = <u32 as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self::from_bits(bits))
    }
}

#[cfg(feature = "embedded-can")]
impl From<CanId> for embedded_can::Id {
    fn from(id: CanId) -> Self {
        if id.is_extended() {
            match embedded_can::ExtendedId::new(id.as_raw()) {
                Some(value) => Self::Extended(value),
                None => Self::Extended(embedded_can::ExtendedId::ZERO),
            }
        } else {
            let raw: u16 = u16::try_from(id.as_raw()).unwrap_or_default();
            match embedded_can::StandardId::new(raw) {
                Some(value) => Self::Standard(value),
                None => Self::Standard(embedded_can::StandardId::ZERO),
            }
        }
    }
}

#[cfg(feature = "embedded-can")]
impl From<embedded_can::Id> for CanId {
    fn from(id: embedded_can::Id) -> Self {
        match id {
            embedded_can::Id::Standard(value) => Self(u32::from(value.as_raw())),
            embedded_can::Id::Extended(value) => Self(value.as_raw() | EXT_FLAG),
        }
    }
}

const _: () = assert!(core::mem::size_of::<CanId>() == 4);

#[cfg(test)]
mod tests {
    use super::{CanId, EXT_FLAG};

    #[test]
    fn validates_boundaries_and_round_trips_bits() {
        let standard = CanId::standard(0x7ff);
        assert!(standard.is_ok());
        assert!(CanId::standard(0x800).is_err());

        let extended = CanId::extended(0x1fff_ffff);
        assert!(extended.is_ok());
        assert!(CanId::extended(0x2000_0000).is_err());

        let standard = standard.unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(standard.as_raw(), 0x7ff);
        assert!(!standard.is_extended());
        assert_eq!(CanId::from_bits(standard.to_bits()), standard);

        let extended = extended.unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(extended.as_raw(), 0x1fff_ffff);
        assert!(extended.is_extended());
        assert_eq!(extended.to_bits(), EXT_FLAG | 0x1fff_ffff);
        assert_eq!(CanId::from_bits(extended.to_bits()), extended);
    }
}
