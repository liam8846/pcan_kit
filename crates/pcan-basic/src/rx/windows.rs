use core::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use pcan_basic_sys::{
    PCAN_ERROR_ILLPARAMVAL, PCAN_ERROR_OK, PCAN_RECEIVE_EVENT, PcanApi, TPCANHandle,
};
use pcan_core::{BackendError, Error, FaultKind, TransportEvent};
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, WAIT_FAILED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects,
};

use super::{ReadOutcome, read_one};
use crate::config::RxThreadPolicy;

#[derive(Debug)]
struct OwnedEvent(HANDLE);

// SAFETY: Win32 Event HANDLE 可由任意執行緒等待、設值與關閉；所有權由此
// RAII 型別唯一持有，關閉前 WinRxThread::drop 必定先停止並 join 使用者。
unsafe impl Send for OwnedEvent {}
// SAFETY: SetEvent/WaitForMultipleObjects 對 Event HANDLE 是執行緒安全 API；
// 真正關閉由唯一所有者的 Drop 串行執行。
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    fn create(manual_reset: bool) -> Result<Self, Error> {
        // SAFETY: security attributes 與名稱皆為 null，布林值符合 Win32 ABI；
        // 成功回傳的唯一 HANDLE 交由 OwnedEvent 配對 CloseHandle。
        let handle = unsafe { CreateEventW(ptr::null(), i32::from(manual_reset), 0, ptr::null()) };
        if handle.is_null() {
            // SAFETY: 緊接失敗的 CreateEventW 讀取執行緒區域錯誤碼。
            let code = unsafe { GetLastError() };
            return Err(Error::Io(BackendError::PcanBasic {
                code,
                text: format!("CreateEventW 失敗，Win32 錯誤 {code}").into_boxed_str(),
                op: "CreateEventW",
                kind: FaultKind::Fatal,
            }));
        }
        Ok(Self(handle))
    }

    fn set(&self) {
        // SAFETY: self 持有有效 Event HANDLE；失敗只代表執行緒可能已退出，
        // Drop 仍會 join 並安全收尾。
        let _result = unsafe { SetEvent(self.0) };
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        // SAFETY: HANDLE 由 OwnedEvent 唯一擁有且只在此處關閉一次。
        let _closed = unsafe { CloseHandle(self.0) };
    }
}

struct WinRxThread {
    // 安全關鍵：Drop 先 SetEvent(stop)，再 join，之後欄位才依序釋放兩個
    // Event。PcanChannel 又把整個 RxSource 宣告在 api 前；因此 join 完成前
    // 不可能 CAN_Uninitialize 或 FreeLibrary。反序會讓執行緒在 CAN_Read
    // 執行中卸載 DLL，造成立即當機。
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    stop_event: OwnedEvent,
    rx_event: OwnedEvent,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Result<TransportEvent, Error>>>,
    closed: AtomicBool,
    dropped: Arc<AtomicU64>,
}

impl core::fmt::Debug for WinRxThread {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("WinRxThread")
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl WinRxThread {
    #[allow(clippy::too_many_lines)]
    fn new(
        api: Arc<PcanApi>,
        handle: TPCANHandle,
        fd_mode: bool,
        capacity: usize,
        policy: RxThreadPolicy,
    ) -> Result<Self, Error> {
        let stop_event = OwnedEvent::create(true)?;
        let rx_event = OwnedEvent::create(false)?;
        let rx_value = rx_event.0 as usize;
        let mut status = api.set_value_usize(handle, PCAN_RECEIVE_EVENT, rx_value);
        if status & PCAN_ERROR_ILLPARAMVAL != 0 {
            let mask = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
            let truncated = u32::try_from(rx_value & mask).unwrap_or(0);
            status = api.set_value_u32(handle, PCAN_RECEIVE_EVENT, truncated);
            #[cfg(feature = "tracing")]
            tracing::debug!("PCAN_RECEIVE_EVENT 不接受原生 HANDLE 寬度，已採 4-byte 相容路徑");
        } else {
            #[cfg(feature = "tracing")]
            tracing::debug!("PCAN_RECEIVE_EVENT 採原生 HANDLE 寬度");
        }
        if status != PCAN_ERROR_OK {
            return Err(super::backend_error(
                &api,
                status,
                "CAN_SetValue(PCAN_RECEIVE_EVENT)",
                FaultKind::Fatal,
            ));
        }

        let (sender, receiver) = mpsc::channel(capacity.max(1));
        let thread_stop = stop_event.0 as usize;
        let thread_rx = rx_event.0 as usize;
        let dropped = Arc::new(AtomicU64::new(0));
        let thread_dropped = Arc::clone(&dropped);
        let join = std::thread::Builder::new()
            .name(format!("pcan-rx-{handle:04x}"))
            .spawn(move || {
                let handles = [thread_stop as HANDLE, thread_rx as HANDLE];
                loop {
                    // SAFETY: 兩個 HANDLE 在 WinRxThread::drop 的 join 完成前
                    // 皆保持有效；陣列含恰好兩個元素且不要求等待全部。
                    let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if wait == WAIT_OBJECT_0 {
                        break;
                    }
                    if wait == WAIT_OBJECT_0 + 1 {
                        loop {
                            match read_one(&api, handle, fd_mode) {
                                Ok(ReadOutcome::Event(event)) => {
                                    let keep_running = match policy {
                                        RxThreadPolicy::Backpressure => {
                                            sender.blocking_send(Ok(event)).is_ok()
                                        }
                                        RxThreadPolicy::DropOnFull => {
                                            match sender.try_send(Ok(event)) {
                                                Ok(()) => true,
                                                Err(mpsc::error::TrySendError::Full(_)) => {
                                                    thread_dropped.fetch_add(1, Ordering::Relaxed);
                                                    true
                                                }
                                                Err(mpsc::error::TrySendError::Closed(_)) => false,
                                            }
                                        }
                                    };
                                    if !keep_running {
                                        return;
                                    }
                                }
                                Ok(ReadOutcome::Empty) => break,
                                Err(error) => {
                                    let _closed = sender.blocking_send(Err(error)).is_err();
                                    return;
                                }
                            }
                        }
                        continue;
                    }
                    let code = if wait == WAIT_FAILED {
                        // SAFETY: 緊接失敗的等待呼叫讀取執行緒區域錯誤碼。
                        unsafe { GetLastError() }
                    } else {
                        wait
                    };
                    let error = Error::Io(BackendError::PcanBasic {
                        code,
                        text: format!("WaitForMultipleObjects 失敗，Win32 錯誤 {code}")
                            .into_boxed_str(),
                        op: "WaitForMultipleObjects",
                        kind: FaultKind::Fatal,
                    });
                    let _closed = sender.blocking_send(Err(error)).is_err();
                    break;
                }
            })
            .map_err(|source| {
                Error::Io(BackendError::PcanBasic {
                    code: 0,
                    text: source.to_string().into_boxed_str(),
                    op: "建立 Windows PCAN RX 執行緒",
                    kind: FaultKind::Fatal,
                })
            })?;
        Ok(Self {
            join: std::sync::Mutex::new(Some(join)),
            stop_event,
            rx_event,
            receiver: tokio::sync::Mutex::new(receiver),
            closed: AtomicBool::new(false),
            dropped,
        })
    }

    async fn recv(&self) -> Result<TransportEvent, Error> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let mut receiver = self.receiver.lock().await;
        receiver.recv().await.unwrap_or(Err(Error::Closed))
    }

    fn stop(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut receiver) = self.receiver.try_lock() {
            receiver.close();
        }
        self.stop_event.set();
        let join = match self.join.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(join) = join {
            let _joined = join.join();
        }
        debug_assert!(!self.rx_event.0.is_null());
    }
}

impl Drop for WinRxThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Windows PCAN 接收事件來源。
#[derive(Debug)]
pub(crate) struct RxSource {
    thread: WinRxThread,
}

impl RxSource {
    pub(crate) fn new(
        api: Arc<PcanApi>,
        handle: TPCANHandle,
        fd_mode: bool,
        capacity: usize,
        policy: RxThreadPolicy,
    ) -> Result<Self, Error> {
        Ok(Self {
            thread: WinRxThread::new(api, handle, fd_mode, capacity, policy)?,
        })
    }

    pub(crate) async fn recv(&self) -> Result<TransportEvent, Error> {
        self.thread.recv().await
    }

    pub(crate) fn stop(&self) {
        self.thread.stop();
    }
}
