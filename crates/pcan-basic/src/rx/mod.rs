#[cfg(unix)]
use std::sync::Arc;

use pcan_basic_sys::{PcanApi, StatusOutcome, TPCANHandle, TPCANMsgFD, bus_state_of, classify};
use pcan_core::{
    BackendError, BusState, BusStatus, BusWarnings, Error, FaultKind, RxFrame, Timestamp,
    TimestampSource, TransportEvent,
};

use crate::convert::{
    error_frame_to_warnings, is_echo, msg_fd_to_frame, msg_to_frame, status_frame_to_status,
};

#[cfg_attr(unix, path = "linux.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod platform;
pub(crate) use platform::RxSource;

pub(crate) enum ReadOutcome {
    Event(TransportEvent),
    Empty,
}

pub(crate) fn backend_error(
    api: &PcanApi,
    code: u32,
    operation: &'static str,
    kind: FaultKind,
) -> Error {
    Error::Io(BackendError::PcanBasic {
        code,
        text: api.error_text(code),
        op: operation,
        kind,
    })
}

fn warning_status(warnings: BusWarnings, raw_status: u32) -> TransportEvent {
    TransportEvent::Status(BusStatus::new(bus_state_of(raw_status), warnings, None))
}

fn metadata_event(message: &TPCANMsgFD) -> Option<TransportEvent> {
    if let Some(status) = status_frame_to_status(message) {
        return Some(TransportEvent::Status(status));
    }
    let warnings = error_frame_to_warnings(message);
    if warnings.is_empty() {
        return None;
    }
    let state = if warnings.contains(BusWarnings::BUS_PASSIVE) {
        BusState::ErrorPassive
    } else {
        BusState::Warning
    };
    Some(TransportEvent::Status(BusStatus::new(
        state, warnings, None,
    )))
}

pub(crate) fn read_one(
    api: &PcanApi,
    handle: TPCANHandle,
    fd_mode: bool,
) -> Result<ReadOutcome, Error> {
    if fd_mode {
        let Some((status, message, timestamp)) = api.read_fd(handle) else {
            return Err(Error::Unsupported(
                "載入的 PCAN-Basic 函式庫不提供 CAN_ReadFD",
            ));
        };
        let outcome = classify(status);
        match outcome {
            StatusOutcome::Empty { .. } => return Ok(ReadOutcome::Empty),
            StatusOutcome::TxBusy { warnings } => {
                return Ok(ReadOutcome::Event(warning_status(
                    warnings | BusWarnings::TX_QUEUE_FULL,
                    status,
                )));
            }
            StatusOutcome::Failed { kind, .. } => {
                return Err(backend_error(api, status, "CAN_ReadFD", kind));
            }
            StatusOutcome::Ok { warnings } if !warnings.is_empty() => {
                return Ok(ReadOutcome::Event(warning_status(warnings, status)));
            }
            StatusOutcome::Ok { .. } => {}
            _ => {
                return Err(backend_error(
                    api,
                    status,
                    "CAN_ReadFD",
                    FaultKind::Permanent,
                ));
            }
        }
        if let Some(event) = metadata_event(&message) {
            return Ok(ReadOutcome::Event(event));
        }
        let Some(frame) = msg_fd_to_frame(&message) else {
            return Ok(ReadOutcome::Empty);
        };
        Ok(ReadOutcome::Event(TransportEvent::Frame(RxFrame::new(
            frame,
            Timestamp::new(timestamp, TimestampSource::Hardware),
            is_echo(message.MSGTYPE),
        ))))
    } else {
        let (status, message, timestamp) = api.read(handle);
        let outcome = classify(status);
        match outcome {
            StatusOutcome::Empty { .. } => return Ok(ReadOutcome::Empty),
            StatusOutcome::TxBusy { warnings } => {
                return Ok(ReadOutcome::Event(warning_status(
                    warnings | BusWarnings::TX_QUEUE_FULL,
                    status,
                )));
            }
            StatusOutcome::Failed { kind, .. } => {
                return Err(backend_error(api, status, "CAN_Read", kind));
            }
            StatusOutcome::Ok { warnings } if !warnings.is_empty() => {
                return Ok(ReadOutcome::Event(warning_status(warnings, status)));
            }
            StatusOutcome::Ok { .. } => {}
            _ => {
                return Err(backend_error(api, status, "CAN_Read", FaultKind::Permanent));
            }
        }
        let mut metadata = TPCANMsgFD {
            ID: message.ID,
            MSGTYPE: message.MSGTYPE,
            DLC: message.LEN,
            DATA: [0; 64],
        };
        metadata.DATA[..8].copy_from_slice(&message.DATA);
        if let Some(event) = metadata_event(&metadata) {
            return Ok(ReadOutcome::Event(event));
        }
        let Some(frame) = msg_to_frame(&message) else {
            return Ok(ReadOutcome::Empty);
        };
        Ok(ReadOutcome::Event(TransportEvent::Frame(RxFrame::new(
            frame,
            Timestamp::new(timestamp.to_micros(), TimestampSource::Hardware),
            is_echo(message.MSGTYPE),
        ))))
    }
}

#[cfg(unix)]
pub(crate) struct ThreadRx {
    receiver: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Result<TransportEvent, Error>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    dropped: Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(unix)]
impl core::fmt::Debug for ThreadRx {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ThreadRx")
            .field(
                "dropped",
                &self.dropped.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(unix)]
impl ThreadRx {
    pub(crate) fn new(
        api: Arc<PcanApi>,
        handle: TPCANHandle,
        fd_mode: bool,
        capacity: usize,
        policy: crate::config::RxThreadPolicy,
        poll_interval: Option<core::time::Duration>,
    ) -> Result<Self, Error> {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity.max(1));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let thread_stop = Arc::clone(&stop);
        let thread_dropped = Arc::clone(&dropped);
        let join = std::thread::Builder::new()
            .name(format!("pcan-rx-{handle:04x}"))
            .spawn(move || {
                while !thread_stop.load(std::sync::atomic::Ordering::Acquire) {
                    match read_one(&api, handle, fd_mode) {
                        Ok(ReadOutcome::Event(event)) => {
                            let sent = match policy {
                                crate::config::RxThreadPolicy::Backpressure => {
                                    sender.blocking_send(Ok(event)).is_ok()
                                }
                                crate::config::RxThreadPolicy::DropOnFull => {
                                    match sender.try_send(Ok(event)) {
                                        Ok(()) => true,
                                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                            thread_dropped
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                            true
                                        }
                                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                            false
                                        }
                                    }
                                }
                            };
                            if !sent {
                                break;
                            }
                        }
                        Ok(ReadOutcome::Empty) => {
                            if let Some(interval) = poll_interval {
                                std::thread::sleep(interval);
                            } else {
                                break;
                            }
                        }
                        Err(error) => {
                            let _closed = sender.blocking_send(Err(error)).is_err();
                            break;
                        }
                    }
                }
            })
            .map_err(|source| {
                Error::Io(BackendError::PcanBasic {
                    code: 0,
                    text: source.to_string().into_boxed_str(),
                    op: "建立 PCAN RX 執行緒",
                    kind: FaultKind::Fatal,
                })
            })?;
        Ok(Self {
            receiver: tokio::sync::Mutex::new(receiver),
            stop,
            join: std::sync::Mutex::new(Some(join)),
            dropped,
        })
    }

    pub(crate) async fn recv(&self) -> Result<TransportEvent, Error> {
        let mut receiver = self.receiver.lock().await;
        receiver.recv().await.unwrap_or(Err(Error::Closed))
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut receiver) = self.receiver.try_lock() {
            receiver.close();
        }
        let join = match self.join.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(join) = join {
            let _joined = join.join();
        }
    }
}

#[cfg(unix)]
impl Drop for ThreadRx {
    fn drop(&mut self) {
        self.stop();
    }
}
