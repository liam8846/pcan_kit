use pcan_basic_sys::{
    PCAN_MESSAGE_BRS, PCAN_MESSAGE_ECHO, PCAN_MESSAGE_ERRFRAME, PCAN_MESSAGE_ESI,
    PCAN_MESSAGE_EXTENDED, PCAN_MESSAGE_FD, PCAN_MESSAGE_RTR, PCAN_MESSAGE_STATUS, TPCANMsg,
    TPCANMsgFD, bus_state_of, warnings_of,
};
use pcan_core::{BusStatus, BusWarnings, CanId, ConfigError, Frame, FrameFlags, dlc_to_len};

fn id_from_raw(raw: u32, extended: bool) -> Option<CanId> {
    if extended {
        CanId::extended(raw).ok()
    } else {
        u16::try_from(raw)
            .ok()
            .and_then(|id| CanId::standard(id).ok())
    }
}

fn message_is_metadata(message_type: u8) -> bool {
    message_type & (PCAN_MESSAGE_STATUS | PCAN_MESSAGE_ERRFRAME) != 0
}

/// 將 PCAN-Basic 的 FD 訊息轉為核心幀。
///
/// 回傳 `None` 表示訊息是狀態／錯誤幀或內容不符合 CAN 格式。
#[must_use]
pub fn msg_fd_to_frame(message: &TPCANMsgFD) -> Option<Frame> {
    if message_is_metadata(message.MSGTYPE) {
        return None;
    }
    let id = id_from_raw(message.ID, message.MSGTYPE & PCAN_MESSAGE_EXTENDED != 0)?;
    let len = dlc_to_len(message.DLC);
    let is_fd = message.MSGTYPE & PCAN_MESSAGE_FD != 0;
    let is_rtr = message.MSGTYPE & PCAN_MESSAGE_RTR != 0;
    let frame = if is_rtr {
        if is_fd || len > 8 {
            return None;
        }
        Frame::remote(id, len).ok()?
    } else if is_fd {
        Frame::new_fd(
            id,
            &message.DATA[..usize::from(len)],
            message.MSGTYPE & PCAN_MESSAGE_BRS != 0,
        )
        .ok()?
    } else {
        if len > 8 {
            return None;
        }
        Frame::new(id, &message.DATA[..usize::from(len)]).ok()?
    };
    frame.with_esi(message.MSGTYPE & PCAN_MESSAGE_ESI != 0).ok()
}

/// 將核心幀轉為 PCAN-Basic 的 FD 訊息結構。
#[must_use]
pub fn frame_to_msg_fd(frame: &Frame) -> TPCANMsgFD {
    let mut message = TPCANMsgFD {
        ID: frame.id().as_raw(),
        MSGTYPE: 0,
        DLC: frame.dlc(),
        DATA: [0; 64],
    };
    if frame.id().is_extended() {
        message.MSGTYPE |= PCAN_MESSAGE_EXTENDED;
    }
    if frame.flags().contains(FrameFlags::RTR) {
        message.MSGTYPE |= PCAN_MESSAGE_RTR;
    }
    if frame.flags().contains(FrameFlags::FD) {
        message.MSGTYPE |= PCAN_MESSAGE_FD;
    }
    if frame.flags().contains(FrameFlags::BRS) {
        message.MSGTYPE |= PCAN_MESSAGE_BRS;
    }
    if frame.flags().contains(FrameFlags::ESI) {
        message.MSGTYPE |= PCAN_MESSAGE_ESI;
    }
    message.DATA[..frame.data().len()].copy_from_slice(frame.data());
    message
}

/// 將 PCAN-Basic 古典訊息轉為核心幀。
///
/// 狀態／錯誤幀或無效識別碼與長度會回傳 `None`。
#[must_use]
pub fn msg_to_frame(message: &TPCANMsg) -> Option<Frame> {
    if message_is_metadata(message.MSGTYPE) || message.LEN > 8 {
        return None;
    }
    let id = id_from_raw(message.ID, message.MSGTYPE & PCAN_MESSAGE_EXTENDED != 0)?;
    if message.MSGTYPE & PCAN_MESSAGE_RTR != 0 {
        Frame::remote(id, message.LEN).ok()
    } else {
        Frame::new(id, &message.DATA[..usize::from(message.LEN)]).ok()
    }
}

/// 將核心古典幀轉為 PCAN-Basic 訊息。
///
/// # Errors
///
/// CAN FD 幀不能由古典 API 傳送時回傳設定錯誤。
pub fn frame_to_msg(frame: &Frame) -> Result<TPCANMsg, ConfigError> {
    if frame.is_fd() {
        return Err(ConfigError::InvalidFlags("CAN FD 幀不能使用 CAN_Write"));
    }
    let mut message = TPCANMsg {
        ID: frame.id().as_raw(),
        MSGTYPE: 0,
        LEN: u8::try_from(frame.len())
            .map_err(|_| ConfigError::InvalidFlags("古典 CAN 幀的長度無法表示為 PCAN LEN"))?,
        DATA: [0; 8],
    };
    if frame.id().is_extended() {
        message.MSGTYPE |= PCAN_MESSAGE_EXTENDED;
    }
    if frame.is_remote() {
        message.MSGTYPE |= PCAN_MESSAGE_RTR;
    } else {
        message.DATA[..frame.data().len()].copy_from_slice(frame.data());
    }
    Ok(message)
}

fn embedded_status(message: &TPCANMsgFD) -> u32 {
    u32::from_be_bytes([
        message.DATA[0],
        message.DATA[1],
        message.DATA[2],
        message.DATA[3],
    ])
}

/// 解析 PCAN 狀態幀；資料前四位元組是大端序狀態碼。
#[must_use]
pub fn status_frame_to_status(message: &TPCANMsgFD) -> Option<BusStatus> {
    if message.MSGTYPE & PCAN_MESSAGE_STATUS == 0 {
        return None;
    }
    let status = embedded_status(message);
    Some(BusStatus::new(
        bus_state_of(status),
        warnings_of(status),
        None,
    ))
}

/// 解析 PCAN 錯誤幀內嵌狀態碼為健康警告。
#[must_use]
pub fn error_frame_to_warnings(message: &TPCANMsgFD) -> BusWarnings {
    if message.MSGTYPE & PCAN_MESSAGE_ERRFRAME == 0 {
        BusWarnings::empty()
    } else {
        warnings_of(embedded_status(message))
    }
}

pub(crate) const fn is_echo(message_type: u8) -> bool {
    message_type & PCAN_MESSAGE_ECHO != 0
}

#[cfg(test)]
mod tests {
    use pcan_basic_sys::{PCAN_MESSAGE_EXTENDED, PCAN_MESSAGE_STATUS};
    use pcan_core::{CanId, Frame, FrameFlags};

    use super::{frame_to_msg_fd, msg_fd_to_frame, status_frame_to_status};

    fn ids() -> [CanId; 4] {
        [
            CanId::standard(0).unwrap_or_else(|error| unreachable!("{error}")),
            CanId::standard(0x7ff).unwrap_or_else(|error| unreachable!("{error}")),
            CanId::extended(0).unwrap_or_else(|error| unreachable!("{error}")),
            CanId::extended(0x1fff_ffff).unwrap_or_else(|error| unreachable!("{error}")),
        ]
    }

    #[test]
    fn exhaustively_round_trips_legal_fd_flags_and_lengths() {
        let lengths = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];
        for id in ids() {
            for len in lengths {
                for brs in [false, true] {
                    for esi in [false, true] {
                        let data = [0x5a; 64];
                        let frame = Frame::new_fd(id, &data[..len], brs)
                            .and_then(|frame| frame.with_esi(esi))
                            .unwrap_or_else(|error| unreachable!("{error}"));
                        let decoded = msg_fd_to_frame(&frame_to_msg_fd(&frame));
                        assert_eq!(decoded, Some(frame));
                    }
                }
            }
        }
    }

    #[test]
    fn round_trips_classic_and_remote_boundaries() {
        for id in ids() {
            for len in 0_u8..=8 {
                let data = [0xa5; 8];
                let classic = Frame::new(id, &data[..usize::from(len)])
                    .unwrap_or_else(|error| unreachable!("{error}"));
                assert_eq!(msg_fd_to_frame(&frame_to_msg_fd(&classic)), Some(classic));
                let remote = Frame::remote(id, len).unwrap_or_else(|error| unreachable!("{error}"));
                assert_eq!(msg_fd_to_frame(&frame_to_msg_fd(&remote)), Some(remote));
            }
        }
    }

    #[test]
    fn metadata_frames_do_not_become_data_frames() {
        let mut message = frame_to_msg_fd(
            &Frame::new(
                CanId::standard(1).unwrap_or_else(|error| unreachable!("{error}")),
                &[1],
            )
            .unwrap_or_else(|error| unreachable!("{error}")),
        );
        message.MSGTYPE |= PCAN_MESSAGE_STATUS | PCAN_MESSAGE_EXTENDED;
        message.DATA[..4].copy_from_slice(&0x10_u32.to_be_bytes());
        assert_eq!(msg_fd_to_frame(&message), None);
        assert!(status_frame_to_status(&message).is_some());
        assert!(!FrameFlags::FD.is_empty());
    }
}
