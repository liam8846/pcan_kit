use core::future::Future;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::ffi::CString;
use std::sync::Arc;

use pcan_basic_sys::{
    PCAN_ALLOW_ECHO_FRAMES, PCAN_ALLOW_ERROR_FRAMES, PCAN_ALLOW_STATUS_FRAMES,
    PCAN_BUSOFF_AUTORESET, PCAN_FILTER_CLOSE, PCAN_FILTER_OPEN, PCAN_LISTEN_ONLY,
    PCAN_MESSAGE_FILTER, PCAN_MODE_EXTENDED, PCAN_MODE_STANDARD, PCAN_PARAMETER_OFF,
    PCAN_PARAMETER_ON, PcanApi, StatusOutcome, TPCANHandle, bus_state_of, classify, load,
    load_from, warnings_of,
};
use pcan_core::{
    BackendError, Bitrate, BusStatus, Capabilities, Error, FaultKind, FilterSet, Frame, Transport,
    TransportEvent, TransportFactory,
};
use tokio::sync::{Mutex, Semaphore};

use crate::config::{PcanConfig, classic_baudrate, fd_bitrate_string};
use crate::convert::{frame_to_msg, frame_to_msg_fd};
use crate::rx::RxSource;

pub(crate) fn backend_error(
    api: &PcanApi,
    code: u32,
    operation: &'static str,
    fallback: FaultKind,
) -> BackendError {
    let kind = match classify(code) {
        StatusOutcome::Failed { kind, .. } => kind,
        StatusOutcome::TxBusy { .. } => FaultKind::Transient,
        _ => fallback,
    };
    BackendError::PcanBasic {
        code,
        text: api.error_text(code),
        op: operation,
        kind,
    }
}

fn required_status(api: &PcanApi, status: u32, operation: &'static str) -> Result<(), Error> {
    match classify(status) {
        StatusOutcome::Ok { .. } => Ok(()),
        StatusOutcome::Empty { .. } | StatusOutcome::TxBusy { .. } => Err(Error::Io(
            backend_error(api, status, operation, FaultKind::Transient),
        )),
        StatusOutcome::Failed { .. } => Err(Error::Io(backend_error(
            api,
            status,
            operation,
            FaultKind::Fatal,
        ))),
        _ => Err(Error::Io(backend_error(
            api,
            status,
            operation,
            FaultKind::Permanent,
        ))),
    }
}

fn cleanup(api: &PcanApi, handle: TPCANHandle) {
    let status = api.uninitialize(handle);
    if status != 0 {
        #[cfg(feature = "tracing")]
        tracing::warn!(status, handle, "清理失敗通道時 CAN_Uninitialize 回報錯誤");
    }
}

/// 將 PCAN 開啟工作本身的執行期故障轉為既有後端錯誤。
pub(crate) fn open_task_error(text: impl Into<Box<str>>, operation: &'static str) -> Error {
    Error::Io(BackendError::PcanBasic {
        code: 0,
        text: text.into(),
        op: operation,
        kind: FaultKind::Fatal,
    })
}

fn filter_range(filter: &FilterSet) -> Option<(u32, u32, u8)> {
    let [rule] = filter.rules() else {
        return None;
    };
    let (id, mask, inverted) = rule.parts();
    if inverted || mask & 0x8000_0000 == 0 {
        return None;
    }
    let extended = id & 0x8000_0000 != 0;
    let maximum = if extended { 0x1fff_ffff } else { 0x7ff };
    let numeric_mask = mask & maximum;
    let wildcard = (!numeric_mask) & maximum;
    // 只有低位連續 wildcard（2^n - 1）才形成單一連續區間。
    if wildcard & wildcard.wrapping_add(1) != 0 {
        return None;
    }
    let from = id & numeric_mask & maximum;
    let to = from | wildcard;
    Some((
        from,
        to,
        if extended {
            PCAN_MODE_EXTENDED
        } else {
            PCAN_MODE_STANDARD
        },
    ))
}

fn apply_filter(api: &PcanApi, handle: TPCANHandle, filter: &FilterSet) -> Result<(), Error> {
    if filter.is_accept_all() {
        return required_status(
            api,
            api.set_value_u32(handle, PCAN_MESSAGE_FILTER, PCAN_FILTER_OPEN),
            "CAN_SetValue(PCAN_MESSAGE_FILTER=OPEN)",
        );
    }
    if let [rule] = filter.rules() {
        let (_, mask, inverted) = rule.parts();
        if inverted && mask == 0 {
            return required_status(
                api,
                api.set_value_u32(handle, PCAN_MESSAGE_FILTER, PCAN_FILTER_CLOSE),
                "CAN_SetValue(PCAN_MESSAGE_FILTER=CLOSE)",
            );
        }
    }
    if let Some((from, to, mode)) = filter_range(filter) {
        required_status(
            api,
            api.set_value_u32(handle, PCAN_MESSAGE_FILTER, PCAN_FILTER_CLOSE),
            "CAN_SetValue(PCAN_MESSAGE_FILTER=CLOSE)",
        )?;
        return required_status(
            api,
            api.filter_messages(handle, from, to, mode),
            "CAN_FilterMessages",
        );
    }

    // PCAN 硬體只表示單一連續區間；遮罩、多規則或反轉集合不能精確
    // 下推。改為全開，保留上層 Router 的完整軟體語意，且明確記錄降級。
    #[cfg(feature = "tracing")]
    tracing::debug!("PCAN 過濾器無法表示為單一連續區間，未下推到硬體並改由軟體過濾");
    required_status(
        api,
        api.set_value_u32(handle, PCAN_MESSAGE_FILTER, PCAN_FILTER_OPEN),
        "CAN_SetValue(PCAN_MESSAGE_FILTER=OPEN)",
    )
}

/// 一個已開啟的 PCAN 通道。
pub struct PcanChannel {
    // 安全關鍵：RX 必須在 api 前宣告，確保專用執行緒先 stop + join，
    // 才允許通道解除初始化與 PcanApi 最後一個 Arc 卸載 Library。
    rx: RxSource,
    handle: TPCANHandle,
    caps: Capabilities,
    fd_mode: bool,
    tx_lock: Mutex<()>,
    closed: AtomicBool,
    api: Arc<PcanApi>,
}

impl core::fmt::Debug for PcanChannel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PcanChannel")
            .field("handle", &self.handle)
            .field("caps", &self.caps)
            .field("fd_mode", &self.fd_mode)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl PcanChannel {
    fn close_sync(&self) {
        if self
            .closed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.rx.stop();
        let status = self.api.uninitialize(self.handle);
        if status != 0 {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                status,
                handle = self.handle,
                "關閉 PCAN 通道時 CAN_Uninitialize 回報錯誤"
            );
        }
    }
}

impl Drop for PcanChannel {
    fn drop(&mut self) {
        self.close_sync();
    }
}

#[allow(clippy::manual_async_fn)]
impl Transport for PcanChannel {
    fn recv(&self) -> impl Future<Output = Result<TransportEvent, Error>> + Send {
        async move {
            if self.closed.load(Ordering::Acquire) {
                Err(Error::Closed)
            } else {
                self.rx.recv().await
            }
        }
    }

    fn send(&self, frame: &Frame) -> impl Future<Output = Result<(), Error>> + Send {
        let frame = *frame;
        async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            if frame.is_fd() && !self.fd_mode {
                return Err(Error::Unsupported("古典 PCAN 通道不能傳送 CAN FD 幀"));
            }
            let _guard = self.tx_lock.lock().await;
            for attempt in 0..=8 {
                let status = if self.fd_mode {
                    let message = frame_to_msg_fd(&frame);
                    self.api
                        .write_fd(self.handle, &message)
                        .ok_or(Error::Unsupported("PCAN-Basic 不提供 CAN_WriteFD"))?
                } else {
                    let message = frame_to_msg(&frame)?;
                    self.api.write(self.handle, &message)
                };
                match classify(status) {
                    StatusOutcome::Ok { .. } => return Ok(()),
                    StatusOutcome::TxBusy { .. } if attempt < 8 => {
                        tokio::time::sleep(Duration::from_micros(200)).await;
                    }
                    StatusOutcome::TxBusy { .. } => {
                        return Err(Error::TxQueueFull { capacity: 0 });
                    }
                    StatusOutcome::Empty { .. } => {
                        return Err(Error::Io(backend_error(
                            &self.api,
                            status,
                            "CAN_Write",
                            FaultKind::Transient,
                        )));
                    }
                    StatusOutcome::Failed { kind, .. } => {
                        return Err(Error::Io(backend_error(
                            &self.api,
                            status,
                            if self.fd_mode {
                                "CAN_WriteFD"
                            } else {
                                "CAN_Write"
                            },
                            kind,
                        )));
                    }
                    _ => {
                        return Err(Error::Io(backend_error(
                            &self.api,
                            status,
                            "CAN_Write",
                            FaultKind::Permanent,
                        )));
                    }
                }
            }
            Err(Error::TxQueueFull { capacity: 0 })
        }
    }

    fn status(&self) -> impl Future<Output = Result<BusStatus, Error>> + Send {
        async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            let status = self.api.get_status(self.handle);
            match classify(status) {
                StatusOutcome::Failed { .. }
                    if bus_state_of(status) == pcan_core::BusState::BusOff =>
                {
                    Ok(BusStatus::new(
                        pcan_core::BusState::BusOff,
                        warnings_of(status),
                        None,
                    ))
                }
                StatusOutcome::Failed { kind, .. } => Err(Error::Io(backend_error(
                    &self.api,
                    status,
                    "CAN_GetStatus",
                    kind,
                ))),
                _ => Ok(BusStatus::new(
                    bus_state_of(status),
                    warnings_of(status),
                    None,
                )),
            }
        }
    }

    /// 套用識別碼過濾器。
    ///
    /// accept-all 會下推為 `PCAN_FILTER_OPEN`，reject-all 會下推為
    /// `PCAN_FILTER_CLOSE`；單一、非反轉且低位 wildcard 連續的遮罩會下推
    /// 為單一 `[from, to]` 區間。其餘規則集會將硬體設為全開，退回上層
    /// 軟體過濾，並以 debug 等級記錄。
    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<(), Error>> + Send {
        let filter = filter.clone();
        async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            apply_filter(&self.api, self.handle, &filter)
        }
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        async move {
            self.close_sync();
        }
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }
}

/// 依設定開啟 [`PcanChannel`] 的工廠。
#[derive(Clone)]
pub struct PcanFactory {
    config: PcanConfig,
    api: Arc<PcanApi>,
    /// 以 `Arc<str>` 保存，使 `open()` 為 `spawn_blocking` 複製工廠時不需重新配置。
    describe: Arc<str>,
    /// 序列化開啟嘗試，避免逾時遺棄的開啟與後續重試互相破壞。
    open_gate: Arc<Semaphore>,
}

impl core::fmt::Debug for PcanFactory {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PcanFactory")
            .field("config", &self.config)
            .field("api", &self.api)
            .field("describe", &self.describe)
            .field("open_gate", &self.open_gate)
            .finish()
    }
}

impl PcanFactory {
    /// 建立工廠並立即載入 PCAN-Basic 函式庫。
    ///
    /// # Errors
    ///
    /// 通道、位元率或函式庫設定無效，以及函式庫無法載入時回傳錯誤。
    pub fn new(config: PcanConfig) -> Result<Self, Error> {
        config.channel.to_handle()?;
        match config.common.bitrate {
            Bitrate::Classic { .. } => {
                let _validated = classic_baudrate(config.common.bitrate)?;
            }
            Bitrate::Fd { .. } => {
                let _validated =
                    fd_bitrate_string(config.common.bitrate, config.raw_fd_bitrate.as_deref())?;
            }
            _ => {
                return Err(
                    pcan_core::ConfigError::InvalidBitrate("未知的位元率種類".into()).into(),
                );
            }
        }
        let api = match config.library_path.as_deref() {
            Some(path) => load_from(path)?,
            None => load()?,
        };
        let describe: Arc<str> = format!(
            "pcan:{}@{}",
            config.channel,
            config.common.bitrate.nominal()
        )
        .into();
        Ok(Self {
            config,
            api,
            describe,
            open_gate: Arc::new(Semaphore::new(1)),
        })
    }

    #[allow(clippy::too_many_lines)]
    fn open_channel(&self) -> Result<PcanChannel, Error> {
        let handle = self.config.channel.to_handle()?;
        let fd_mode = self.config.common.bitrate.is_fd();
        if fd_mode && !self.api.supports_fd() {
            return Err(Error::Unsupported(
                "載入的 PCAN-Basic 版本不提供完整 CAN FD API",
            ));
        }
        let initialize_status = match self.config.common.bitrate {
            Bitrate::Classic { .. } => self
                .api
                .initialize(handle, classic_baudrate(self.config.common.bitrate)?),
            Bitrate::Fd { .. } => {
                let bitrate = fd_bitrate_string(
                    self.config.common.bitrate,
                    self.config.raw_fd_bitrate.as_deref(),
                )?;
                let c_string = CString::new(bitrate.as_ref()).map_err(|_| {
                    pcan_core::ConfigError::InvalidBitrate("PCAN FD 位元率字串含有內嵌 NUL".into())
                })?;
                self.api
                    .initialize_fd(handle, c_string.as_c_str())
                    .ok_or(Error::Unsupported("PCAN-Basic 不提供 CAN_InitializeFD"))?
            }
            _ => {
                return Err(
                    pcan_core::ConfigError::InvalidBitrate("未知的位元率種類".into()).into(),
                );
            }
        };
        if let Err(error) = required_status(
            &self.api,
            initialize_status,
            if fd_mode {
                "CAN_InitializeFD"
            } else {
                "CAN_Initialize"
            },
        ) {
            cleanup(&self.api, handle);
            let source = match error {
                Error::Io(source) => source,
                _ => backend_error(
                    &self.api,
                    initialize_status,
                    "CAN_Initialize",
                    FaultKind::Fatal,
                ),
            };
            return Err(Error::Open {
                channel: self.config.channel.to_string().into_boxed_str(),
                source,
            });
        }

        let on_off = |enabled: bool| {
            if enabled {
                PCAN_PARAMETER_ON
            } else {
                PCAN_PARAMETER_OFF
            }
        };
        for (parameter, enabled, operation) in [
            (
                PCAN_LISTEN_ONLY,
                self.config.common.listen_only,
                "設定 PCAN_LISTEN_ONLY",
            ),
            (
                PCAN_ALLOW_ERROR_FRAMES,
                self.config.common.receive_error_frames,
                "設定 PCAN_ALLOW_ERROR_FRAMES",
            ),
            (
                PCAN_ALLOW_STATUS_FRAMES,
                self.config.common.receive_status_frames,
                "設定 PCAN_ALLOW_STATUS_FRAMES",
            ),
        ] {
            let status = self.api.set_value_u32(handle, parameter, on_off(enabled));
            if let Err(error) = required_status(&self.api, status, operation) {
                cleanup(&self.api, handle);
                return Err(error);
            }
        }

        let echo_status = self.api.set_value_u32(
            handle,
            PCAN_ALLOW_ECHO_FRAMES,
            on_off(self.config.common.receive_own_frames),
        );
        let echo_frames = matches!(classify(echo_status), StatusOutcome::Ok { .. });
        if !echo_frames {
            #[cfg(feature = "tracing")]
            tracing::debug!(
                echo_status,
                "PCAN-Basic 不支援 ALLOW_ECHO_FRAMES，能力回報已降級"
            );
        }

        let auto_reset = self.api.set_value_u32(
            handle,
            PCAN_BUSOFF_AUTORESET,
            on_off(self.config.common.bus_off_auto_reset),
        );
        if let Err(error) = required_status(&self.api, auto_reset, "設定 PCAN_BUSOFF_AUTORESET") {
            cleanup(&self.api, handle);
            return Err(error);
        }

        let rx = match RxSource::new(
            Arc::clone(&self.api),
            handle,
            fd_mode,
            self.config.common.rx_queue_capacity,
            self.config.rx_thread_policy,
        ) {
            Ok(source) => source,
            Err(error) => {
                cleanup(&self.api, handle);
                return Err(error);
            }
        };
        if let Err(error) = apply_filter(&self.api, handle, &self.config.common.filter) {
            rx.stop();
            cleanup(&self.api, handle);
            return Err(error);
        }
        let mut caps = Capabilities::default();
        caps.can_fd = fd_mode;
        caps.brs = fd_mode;
        caps.echo_frames = echo_frames;
        caps.hardware_filter = true;
        caps.hardware_timestamps = true;
        caps.listen_only = true;
        Ok(PcanChannel {
            rx,
            handle,
            caps,
            fd_mode,
            tx_lock: Mutex::new(()),
            closed: AtomicBool::new(false),
            api: Arc::clone(&self.api),
        })
    }
}

impl TransportFactory for PcanFactory {
    type Transport = PcanChannel;

    fn open(&self) -> impl Future<Output = Result<Self::Transport, Error>> + Send {
        let factory = self.clone();
        async move {
            let permit = Arc::clone(&factory.open_gate)
                .acquire_owned()
                .await
                .map_err(|_| open_task_error("PCAN 開啟閘門已關閉", "等待 PCAN 開啟閘門"))?;

            // 阻塞 FFI 與 RX 執行緒建立不得佔用非同步執行期工作執行緒。
            // 結果排在 permit 前方，使逾時後由 runtime 丟棄工作輸出時，先
            // 關閉成功開啟的通道，再允許下一次開啟嘗試進入。
            match tokio::task::spawn_blocking(move || (factory.open_channel(), permit)).await {
                Ok((result, _permit)) => result,
                Err(source) => Err(open_task_error(
                    source.to_string(),
                    "等待 PCAN 阻塞開啟工作",
                )),
            }
        }
    }

    fn describe(&self) -> &str {
        &self.describe
    }
}

#[cfg(test)]
mod tests {
    use pcan_core::{CanId, FilterRule, FilterSet};

    use super::{PcanFactory, filter_range};

    #[test]
    fn factory_is_clone_send_and_sync() {
        fn assert_traits<T: Clone + Send + Sync>() {}

        assert_traits::<PcanFactory>();
    }

    #[test]
    fn only_contiguous_single_ranges_are_pushed_down() {
        let id = CanId::standard(0x120).unwrap_or_else(|error| unreachable!("{error}"));
        let contiguous = FilterSet::with(FilterRule::mask(id, 0x8000_07f0));
        assert_eq!(filter_range(&contiguous), Some((0x120, 0x12f, 0)));

        let sparse = FilterSet::with(FilterRule::mask(id, 0x8000_0555));
        assert_eq!(filter_range(&sparse), None);

        let mut multiple = contiguous.clone();
        multiple.push(FilterRule::exact(id));
        assert_eq!(filter_range(&multiple), None);
    }
}
