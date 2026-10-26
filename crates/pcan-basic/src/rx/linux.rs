use std::future::{Future, poll_fn};
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};

use pcan_basic_sys::{
    PCAN_ERROR_ILLPARAMTYPE, PCAN_ERROR_OK, PCAN_RECEIVE_EVENT, PcanApi, TPCANHandle, classify,
};
use pcan_core::{BackendError, Error, FaultKind, TransportEvent};
use tokio::io::unix::AsyncFd;
use tokio::sync::watch;

use super::{ReadOutcome, ThreadRx, backend_error, read_one};
use crate::config::RxThreadPolicy;

#[derive(Debug)]
enum Inner {
    // 每次 poll 只同步持鎖；stop() 取走並反註冊 fd 後才允許解除通道。
    Event(Mutex<Option<AsyncFd<OwnedFd>>>),
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
    shutdown: watch::Sender<bool>,
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
            // SAFETY: F_DUPFD_CLOEXEC 只複製驅動提供的 fd，不取得原始 fd
            // 的所有權。成功後的新 fd 由本來源唯一持有，避免驅動關閉或替換
            // 原始 fd 時破壞 reactor 的生命週期契約。
            let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if duplicate < 0 {
                return Err(Error::Io(BackendError::SocketCan {
                    op: "複製 PCAN 接收事件 fd",
                    kind: FaultKind::Fatal,
                    source: io::Error::last_os_error(),
                }));
            }
            // SAFETY: fcntl 成功傳回新的唯一 fd，由 OwnedFd 配對 close。
            let owned = unsafe { OwnedFd::from_raw_fd(duplicate) };
            // SAFETY: owned 保持同一有效 fd，直到 AsyncFd 先反註冊再關閉。
            // stop() 與同步讀取共用鎖，且在 CAN_Uninitialize 前釋放註冊。
            let afd = unsafe { AsyncFd::register(owned) }.map_err(|source| {
                Error::Io(BackendError::SocketCan {
                    op: "註冊 PCAN 接收事件 fd",
                    kind: FaultKind::Fatal,
                    source: source.into(),
                })
            })?;
            Inner::Event(Mutex::new(Some(afd)))
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
            shutdown: watch::channel(false).0,
        })
    }

    pub(crate) async fn recv(&self) -> Result<TransportEvent, Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let Inner::Event(event) = &self.inner else {
            if let Inner::CompatibilityThread(thread) = &self.inner {
                return thread.recv().await;
            }
            return Err(Error::Closed);
        };
        let mut shutdown = self.shutdown.subscribe();
        let mut changed = std::pin::pin!(shutdown.changed());
        poll_fn(|context| {
            if changed.as_mut().poll(context).is_ready() {
                return Poll::Ready(Err(Error::Closed));
            }
            self.poll_event(event, context)
        })
        .await
    }

    fn poll_event(
        &self,
        event: &Mutex<Option<AsyncFd<OwnedFd>>>,
        context: &mut Context<'_>,
    ) -> Poll<Result<TransportEvent, Error>> {
        let registration = event
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(afd) = registration.as_ref() else {
            return Poll::Ready(Err(Error::Closed));
        };
        loop {
            // 多數情況驅動佇列仍有資料，先讀可避開 epoll 往返。
            match read_one(&self.api, self.handle, self.fd_mode)? {
                ReadOutcome::Event(event) => return Poll::Ready(Ok(event)),
                ReadOutcome::Empty => {}
            }
            let mut guard = ready!(afd.poll_read_ready(context)).map_err(|source| {
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
                Ok(Ok(event)) => return Poll::Ready(Ok(event)),
                Ok(Err(source)) => {
                    if let Some(error) = backend_failure {
                        return Poll::Ready(Err(error));
                    }
                    return Poll::Ready(Err(Error::Io(BackendError::SocketCan {
                        op: "讀取 PCAN 接收事件",
                        kind: FaultKind::Fatal,
                        source,
                    })));
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
        if let Inner::Event(event) = &self.inner {
            // 取得鎖代表所有 CAN_Read 已結束，且後續 poll 只能得到 Closed。
            let mut registration = event
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            drop(registration.take());
        }
        self.shutdown.send_replace(true);
    }
}

impl Drop for RxSource {
    fn drop(&mut self) {
        self.stop();
    }
}
