use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use pcan_basic_sys::{
    PCAN_ERROR_ILLPARAMTYPE, PCAN_ERROR_OK, PCAN_RECEIVE_EVENT, PcanApi, TPCANHandle, classify,
};
use pcan_core::{BackendError, Error, FaultKind, TransportEvent};
use tokio::io::unix::AsyncFd;

use super::{ReadOutcome, ThreadRx, backend_error, read_one};
use crate::config::RxThreadPolicy;

/// 僅借用 PCAN 驅動擁有的 fd，`Drop` 時不關閉它。
///
/// `AsyncFd<T>` 的 Drop 只反註冊 reactor；實際 close 由 `T` 的 Drop 決定。
/// 此 fd 屬於 PEAK 驅動，若改用 `OwnedFd`，通道被 drop 時會誤關裝置 fd，
/// 使後續 `CAN_Read` 全部失敗。
#[derive(Debug)]
struct BorrowedEventFd(RawFd);

impl AsRawFd for BorrowedEventFd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[derive(Debug)]
enum Inner {
    Event(AsyncFd<BorrowedEventFd>),
    CompatibilityThread(ThreadRx),
}

/// Linux PCAN 接收事件來源。
#[derive(Debug)]
pub(crate) struct RxSource {
    inner: Inner,
    api: Arc<PcanApi>,
    handle: TPCANHandle,
    fd_mode: bool,
    closed: AtomicBool,
}

impl RxSource {
    pub(crate) fn new(
        api: Arc<PcanApi>,
        handle: TPCANHandle,
        fd_mode: bool,
        capacity: usize,
        policy: RxThreadPolicy,
    ) -> Result<Self, Error> {
        let (status, fd) = api.get_value_i32(handle, PCAN_RECEIVE_EVENT);
        let inner = if status == PCAN_ERROR_OK && fd >= 0 {
            let afd = AsyncFd::new(BorrowedEventFd(fd)).map_err(|source| {
                Error::Io(BackendError::SocketCan {
                    op: "註冊 PCAN 接收事件 fd",
                    kind: FaultKind::Fatal,
                    source,
                })
            })?;
            Inner::Event(afd)
        } else if status & PCAN_ERROR_ILLPARAMTYPE != 0 {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                status,
                "PCAN 驅動不支援 RECEIVE_EVENT fd，降級為專用 1ms 輪詢執行緒"
            );
            // 這是唯一允許的輪詢回退：舊版 libpcanbasic 或非 PnP 硬體沒有
            // 事件 fd。正常路徑絕不用 sleep 輪詢，避免 0.5–1ms RX 延遲、
            // 每通道 1kHz 無效 FFI 與累積到 QOVERRUN 的排程落後。
            Inner::CompatibilityThread(ThreadRx::new(
                Arc::clone(&api),
                handle,
                fd_mode,
                capacity,
                policy,
                Some(core::time::Duration::from_millis(1)),
            )?)
        } else {
            let kind = match classify(status) {
                pcan_basic_sys::StatusOutcome::Failed { kind, .. } => kind,
                _ => FaultKind::Fatal,
            };
            return Err(backend_error(
                &api,
                status,
                "CAN_GetValue(PCAN_RECEIVE_EVENT)",
                kind,
            ));
        };
        Ok(Self {
            inner,
            api,
            handle,
            fd_mode,
            closed: AtomicBool::new(false),
        })
    }

    pub(crate) async fn recv(&self) -> Result<TransportEvent, Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let Inner::Event(afd) = &self.inner else {
            if let Inner::CompatibilityThread(thread) = &self.inner {
                return thread.recv().await;
            }
            return Err(Error::Closed);
        };
        loop {
            // 多數情況驅動佇列仍有資料，先讀可避開 epoll 往返。
            match read_one(&self.api, self.handle, self.fd_mode)? {
                ReadOutcome::Event(event) => return Ok(event),
                ReadOutcome::Empty => {}
            }
            let mut guard = afd.readable().await.map_err(|source| {
                Error::Io(BackendError::SocketCan {
                    op: "等待 PCAN 接收事件",
                    kind: FaultKind::Fatal,
                    source,
                })
            })?;
            let mut backend_failure = None;
            // 必須使用 try_io，不能在讀空後手動 clear_ready：Linux reactor
            // 使用 EPOLLET。若最後一次讀空與 clear_ready 間恰有新幀到達，
            // 手動清除會抹掉新 edge，永遠等到下一幀才醒。try_io 以 guard
            // 建立時的 ReadyEvent tick 比對，只清除它觀察到的舊就緒。
            match guard.try_io(|_| match read_one(&self.api, self.handle, self.fd_mode) {
                Ok(ReadOutcome::Empty) => Err(io::ErrorKind::WouldBlock.into()),
                Ok(ReadOutcome::Event(event)) => Ok(event),
                Err(error) => {
                    backend_failure = Some(error);
                    Err(io::Error::other("PCAN-Basic 接收失敗"))
                }
            }) {
                Ok(Ok(event)) => return Ok(event),
                Ok(Err(source)) => {
                    if let Some(error) = backend_failure {
                        return Err(error);
                    }
                    return Err(Error::Io(BackendError::SocketCan {
                        op: "讀取 PCAN 接收事件",
                        kind: FaultKind::Fatal,
                        source,
                    }));
                }
                Err(_would_block) => {}
            }
        }
    }

    pub(crate) fn stop(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Inner::CompatibilityThread(thread) = &self.inner {
            thread.stop();
        }
    }
}

impl Drop for RxSource {
    fn drop(&mut self) {
        self.stop();
    }
}
