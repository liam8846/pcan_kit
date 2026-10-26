use pcan_core::{BusState, BusStatus, BusWarnings, ErrorCounters};

/// 將 `SocketCAN` 錯誤幀解析為匯流排狀態。
///
/// 非錯誤幀會回傳健康的 `Active` 狀態。
#[must_use]
pub fn parse_error_frame(can_id: u32, data: &[u8; 8]) -> BusStatus {
    if can_id & libc::CAN_ERR_FLAG == 0 {
        return BusStatus::default();
    }
    let classes = can_id & libc::CAN_ERR_MASK;
    let mut warnings = BusWarnings::empty();
    if classes & (libc::CAN_ERR_ACK | libc::CAN_ERR_PROT | libc::CAN_ERR_TRX) != 0 {
        warnings |= BusWarnings::BUS_LIGHT;
    }
    if classes & libc::CAN_ERR_LOSTARB != 0 {
        warnings |= BusWarnings::ARBITRATION_LOST;
    }
    if classes & libc::CAN_ERR_TX_TIMEOUT != 0 {
        warnings |= BusWarnings::TX_TIMEOUT;
    }
    let controller = data[1];
    if classes & libc::CAN_ERR_CRTL != 0 {
        let warning_mask =
            u8::try_from(libc::CAN_ERR_CRTL_RX_WARNING | libc::CAN_ERR_CRTL_TX_WARNING)
                .unwrap_or(0);
        let passive_mask =
            u8::try_from(libc::CAN_ERR_CRTL_RX_PASSIVE | libc::CAN_ERR_CRTL_TX_PASSIVE)
                .unwrap_or(0);
        let overflow_mask =
            u8::try_from(libc::CAN_ERR_CRTL_RX_OVERFLOW | libc::CAN_ERR_CRTL_TX_OVERFLOW)
                .unwrap_or(0);
        if controller & warning_mask != 0 {
            warnings |= BusWarnings::BUS_HEAVY;
        }
        if controller & passive_mask != 0 {
            warnings |= BusWarnings::BUS_PASSIVE;
        }
        if controller & overflow_mask != 0 {
            warnings |= BusWarnings::RX_OVERRUN;
        }
    }
    let counters = if classes & libc::CAN_ERR_CNT != 0 {
        Some(ErrorCounters {
            tx: data[6],
            rx: data[7],
        })
    } else {
        None
    };
    let active_mask = u8::try_from(libc::CAN_ERR_CRTL_ACTIVE).unwrap_or(0);
    let explicitly_active = classes & libc::CAN_ERR_RESTARTED != 0
        || (classes & libc::CAN_ERR_CRTL != 0 && controller & active_mask != 0);
    if explicitly_active {
        return BusStatus::new(BusState::Active, BusWarnings::empty(), counters);
    }
    let state = if classes & libc::CAN_ERR_BUSOFF != 0 {
        BusState::BusOff
    } else if warnings.contains(BusWarnings::BUS_PASSIVE) {
        BusState::ErrorPassive
    } else if warnings.intersects(BusWarnings::BUS_HEAVY | BusWarnings::BUS_LIGHT) {
        BusState::Warning
    } else {
        BusState::Active
    };
    BusStatus::new(state, warnings, counters)
}

#[cfg(test)]
mod tests {
    use pcan_core::{BusState, BusWarnings, ErrorCounters};

    use super::parse_error_frame;

    fn parse(class: u32, data: [u8; 8]) -> pcan_core::BusStatus {
        parse_error_frame(libc::CAN_ERR_FLAG | class, &data)
    }

    #[test]
    fn maps_busoff_and_controller_states() {
        assert_eq!(parse(libc::CAN_ERR_BUSOFF, [0; 8]).state, BusState::BusOff);
        for (bits, warning, state) in [
            (
                libc::CAN_ERR_CRTL_RX_WARNING,
                BusWarnings::BUS_HEAVY,
                BusState::Warning,
            ),
            (
                libc::CAN_ERR_CRTL_TX_PASSIVE,
                BusWarnings::BUS_PASSIVE,
                BusState::ErrorPassive,
            ),
            (
                libc::CAN_ERR_CRTL_RX_OVERFLOW,
                BusWarnings::RX_OVERRUN,
                BusState::Active,
            ),
        ] {
            let mut data = [0; 8];
            data[1] = u8::try_from(bits).unwrap_or(0);
            let status = parse(libc::CAN_ERR_CRTL, data);
            assert!(status.warnings.contains(warning));
            assert_eq!(status.state, state);
        }
        let mut active = [0; 8];
        active[1] = u8::try_from(libc::CAN_ERR_CRTL_ACTIVE).unwrap_or(0);
        assert!(parse(libc::CAN_ERR_CRTL, active).is_healthy());
    }

    #[test]
    fn maps_protocol_timeout_arbitration_restart_and_counters() {
        for class in [libc::CAN_ERR_ACK, libc::CAN_ERR_PROT, libc::CAN_ERR_TRX] {
            assert!(
                parse(class, [0; 8])
                    .warnings
                    .contains(BusWarnings::BUS_LIGHT)
            );
        }
        assert!(
            parse(libc::CAN_ERR_LOSTARB, [0; 8])
                .warnings
                .contains(BusWarnings::ARBITRATION_LOST)
        );
        assert!(
            parse(libc::CAN_ERR_TX_TIMEOUT, [0; 8])
                .warnings
                .contains(BusWarnings::TX_TIMEOUT)
        );
        assert_eq!(
            parse(libc::CAN_ERR_RESTARTED, [0; 8]).state,
            BusState::Active
        );
        let mut data = [0; 8];
        data[6] = 11;
        data[7] = 22;
        assert_eq!(
            parse(libc::CAN_ERR_CNT, data).error_counters,
            Some(ErrorCounters { tx: 11, rx: 22 })
        );
    }
}
