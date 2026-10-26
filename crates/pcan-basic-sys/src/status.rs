use pcan_core::{BusState, BusWarnings, FaultKind};

use crate::TPCANStatus;
use crate::consts::{
    PCAN_ERROR_BUSHEAVY, PCAN_ERROR_BUSLIGHT, PCAN_ERROR_BUSOFF, PCAN_ERROR_BUSPASSIVE,
    PCAN_ERROR_CAUTION, PCAN_ERROR_HWINUSE, PCAN_ERROR_ILLCLIENT, PCAN_ERROR_ILLDATA,
    PCAN_ERROR_ILLHW, PCAN_ERROR_ILLMODE, PCAN_ERROR_ILLNET, PCAN_ERROR_ILLOPERATION,
    PCAN_ERROR_ILLPARAMTYPE, PCAN_ERROR_ILLPARAMVAL, PCAN_ERROR_INITIALIZE, PCAN_ERROR_NETINUSE,
    PCAN_ERROR_NODRIVER, PCAN_ERROR_OVERRUN, PCAN_ERROR_QOVERRUN, PCAN_ERROR_QRCVEMPTY,
    PCAN_ERROR_QXMTFULL, PCAN_ERROR_REGTEST, PCAN_ERROR_RESOURCE, PCAN_ERROR_UNKNOWN,
    PCAN_ERROR_XMTFULL,
};

const PERMANENT_MASK: TPCANStatus = PCAN_ERROR_ILLPARAMTYPE
    | PCAN_ERROR_ILLPARAMVAL
    | PCAN_ERROR_ILLDATA
    | PCAN_ERROR_ILLMODE
    | PCAN_ERROR_ILLOPERATION
    | PCAN_ERROR_REGTEST;
const FATAL_MASK: TPCANStatus = PCAN_ERROR_BUSOFF
    | PCAN_ERROR_NODRIVER
    | PCAN_ERROR_ILLHW
    | PCAN_ERROR_ILLNET
    | PCAN_ERROR_ILLCLIENT
    | PCAN_ERROR_RESOURCE
    | PCAN_ERROR_HWINUSE
    | PCAN_ERROR_NETINUSE
    | PCAN_ERROR_INITIALIZE
    | PCAN_ERROR_UNKNOWN;

/// 對 `TPCANStatus` 位元欄位的分類結果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum StatusOutcome {
    /// 操作成功，並保留同時出現的健康警告。
    Ok {
        /// 非致命健康警告。
        warnings: BusWarnings,
    },
    /// 接收佇列空；這是正常狀況，不是錯誤。
    Empty {
        /// 非致命健康警告。
        warnings: BusWarnings,
    },
    /// 傳送佇列滿；這是背壓訊號，不是錯誤。
    TxBusy {
        /// 非致命健康警告。
        warnings: BusWarnings,
    },
    /// 操作失敗。
    Failed {
        /// 原始 PCAN 狀態位元。
        code: TPCANStatus,
        /// 同時出現的非致命健康警告。
        warnings: BusWarnings,
        /// 正規化後的故障類別。
        kind: FaultKind,
    },
}

/// 從狀態位元欄位萃取匯流排健康警告。
#[must_use]
pub const fn warnings_of(status: TPCANStatus) -> BusWarnings {
    let mut bits = 0_u16;
    if status & PCAN_ERROR_BUSLIGHT != 0 {
        bits |= BusWarnings::BUS_LIGHT.bits();
    }
    if status & PCAN_ERROR_BUSHEAVY != 0 {
        bits |= BusWarnings::BUS_HEAVY.bits();
    }
    if status & PCAN_ERROR_BUSPASSIVE != 0 {
        bits |= BusWarnings::BUS_PASSIVE.bits();
    }
    if status & PCAN_ERROR_OVERRUN != 0 {
        bits |= BusWarnings::RX_OVERRUN.bits();
    }
    if status & PCAN_ERROR_QOVERRUN != 0 {
        bits |= BusWarnings::QUEUE_OVERRUN.bits();
    }
    if status & PCAN_ERROR_CAUTION != 0 {
        bits |= BusWarnings::CAUTION.bits();
    }
    BusWarnings::from_bits_retain(bits)
}

/// 從狀態位元欄位推導匯流排狀態。
#[must_use]
pub const fn bus_state_of(status: TPCANStatus) -> BusState {
    if status & PCAN_ERROR_BUSOFF != 0 {
        BusState::BusOff
    } else if status & PCAN_ERROR_BUSPASSIVE != 0 {
        BusState::ErrorPassive
    } else if status & (PCAN_ERROR_BUSHEAVY | PCAN_ERROR_BUSLIGHT) != 0 {
        BusState::Warning
    } else {
        BusState::Active
    }
}

/// 將 PCAN-Basic 的狀態位元欄位依固定優先序分類。
#[must_use]
pub const fn classify(status: TPCANStatus) -> StatusOutcome {
    let warnings = warnings_of(status);
    if status & PERMANENT_MASK != 0 {
        StatusOutcome::Failed {
            code: status,
            warnings,
            kind: FaultKind::Permanent,
        }
    } else if status & FATAL_MASK != 0 {
        StatusOutcome::Failed {
            code: status,
            warnings,
            kind: FaultKind::Fatal,
        }
    } else if status & PCAN_ERROR_QRCVEMPTY != 0 {
        StatusOutcome::Empty { warnings }
    } else if status & (PCAN_ERROR_XMTFULL | PCAN_ERROR_QXMTFULL) != 0 {
        StatusOutcome::TxBusy { warnings }
    } else {
        StatusOutcome::Ok { warnings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::*;

    #[test]
    fn classifies_every_individual_flag() {
        assert!(matches!(
            classify(PCAN_ERROR_OK),
            StatusOutcome::Ok { warnings } if warnings.is_empty()
        ));
        for (flag, warning) in [
            (PCAN_ERROR_BUSLIGHT, BusWarnings::BUS_LIGHT),
            (PCAN_ERROR_BUSHEAVY, BusWarnings::BUS_HEAVY),
            (PCAN_ERROR_BUSPASSIVE, BusWarnings::BUS_PASSIVE),
            (PCAN_ERROR_OVERRUN, BusWarnings::RX_OVERRUN),
            (PCAN_ERROR_QOVERRUN, BusWarnings::QUEUE_OVERRUN),
            (PCAN_ERROR_CAUTION, BusWarnings::CAUTION),
        ] {
            assert_eq!(warnings_of(flag), warning);
            assert!(matches!(classify(flag), StatusOutcome::Ok { .. }));
        }
        assert!(matches!(
            classify(PCAN_ERROR_QRCVEMPTY),
            StatusOutcome::Empty { .. }
        ));
        for flag in [PCAN_ERROR_XMTFULL, PCAN_ERROR_QXMTFULL] {
            assert!(matches!(classify(flag), StatusOutcome::TxBusy { .. }));
        }
        for flag in [
            PCAN_ERROR_BUSOFF,
            PCAN_ERROR_NODRIVER,
            PCAN_ERROR_ILLHW,
            PCAN_ERROR_ILLNET,
            PCAN_ERROR_ILLCLIENT,
            PCAN_ERROR_RESOURCE,
            PCAN_ERROR_HWINUSE,
            PCAN_ERROR_NETINUSE,
            PCAN_ERROR_INITIALIZE,
            PCAN_ERROR_UNKNOWN,
        ] {
            assert!(matches!(
                classify(flag),
                StatusOutcome::Failed {
                    kind: FaultKind::Fatal,
                    ..
                }
            ));
        }
        for flag in [
            PCAN_ERROR_ILLPARAMTYPE,
            PCAN_ERROR_ILLPARAMVAL,
            PCAN_ERROR_ILLDATA,
            PCAN_ERROR_ILLMODE,
            PCAN_ERROR_ILLOPERATION,
            PCAN_ERROR_REGTEST,
        ] {
            assert!(matches!(
                classify(flag),
                StatusOutcome::Failed {
                    kind: FaultKind::Permanent,
                    ..
                }
            ));
        }
    }

    #[test]
    fn preserves_priority_and_combined_warnings() {
        assert!(matches!(
            classify(PCAN_ERROR_BUSOFF | PCAN_ERROR_QRCVEMPTY),
            StatusOutcome::Failed {
                kind: FaultKind::Fatal,
                ..
            }
        ));
        assert!(matches!(
            classify(PCAN_ERROR_BUSLIGHT | PCAN_ERROR_QRCVEMPTY),
            StatusOutcome::Empty {
                warnings: BusWarnings::BUS_LIGHT
            }
        ));
        assert_eq!(
            warnings_of(PCAN_ERROR_QOVERRUN | PCAN_ERROR_OVERRUN),
            BusWarnings::QUEUE_OVERRUN | BusWarnings::RX_OVERRUN
        );
        let combined = PCAN_ERROR_BUSOFF | PCAN_ERROR_BUSPASSIVE | PCAN_ERROR_QOVERRUN;
        assert_eq!(
            warnings_of(combined),
            BusWarnings::BUS_PASSIVE | BusWarnings::QUEUE_OVERRUN
        );
        assert_eq!(bus_state_of(combined), BusState::BusOff);
        assert!(matches!(
            classify(PCAN_ERROR_ILLPARAMVAL | PCAN_ERROR_BUSLIGHT),
            StatusOutcome::Failed {
                warnings: BusWarnings::BUS_LIGHT,
                kind: FaultKind::Permanent,
                ..
            }
        ));
    }

    #[test]
    fn derives_all_bus_states() {
        assert_eq!(bus_state_of(0), BusState::Active);
        assert_eq!(bus_state_of(PCAN_ERROR_BUSLIGHT), BusState::Warning);
        assert_eq!(bus_state_of(PCAN_ERROR_BUSPASSIVE), BusState::ErrorPassive);
        assert_eq!(bus_state_of(PCAN_ERROR_BUSOFF), BusState::BusOff);
    }
}
