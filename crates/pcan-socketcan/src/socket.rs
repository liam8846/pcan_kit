use core::ffi::c_void;
use core::future::Future;
use core::mem::{MaybeUninit, size_of, zeroed};
use core::ptr;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use pcan_core::{
    BackendError, Bitrate, BusStatus, CanId, Capabilities, ConfigError, Error, FaultKind,
    FilterSet, Frame, FrameFlags, RxFrame, Timestamp, TimestampSource, Transport, TransportConfig,
    TransportEvent, TransportFactory, len_to_dlc,
};
use tokio::io::unix::AsyncFd;

use crate::errframe::parse_error_frame;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn socket_error(operation: &'static str, kind: FaultKind, source: io::Error) -> Error {
    Error::Io(BackendError::SocketCan {
        op: operation,
        kind,
        source,
    })
}

fn errno_kind(error: &io::Error) -> FaultKind {
    match error.raw_os_error() {
        Some(libc::EAGAIN | libc::ENOBUFS) => FaultKind::Transient,
        _ => FaultKind::Fatal,
    }
}

/// Linux `SocketCAN` 後端設定。
///
/// `common.bitrate` 不會設定核心介面的實際位元率；它只決定是否要求
/// `CAN_RAW_FD_FRAMES`。位元率必須由系統管理層先以 `ip link set ...`
/// 設定，函式庫既無法可靠推知控制器時鐘，也不會猜測現有介面設定。
/// 同理，Bus-Off 自動復歸由介面的 `restart-ms` 管理；SocketCAN 沒有
/// PCAN 式獨立狀態幀，狀態變化來自啟用的核心錯誤幀。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SocketCanConfig {
    /// Linux 網路介面名稱，例如 `can0` 或 `vcan0`。
    pub interface: Box<str>,
    /// 共通通訊設定；位元率只用於決定是否要求 CAN FD。
    pub common: TransportConfig,
}

impl SocketCanConfig {
    /// 建立指定介面的預設 `SocketCAN` 設定。
    #[must_use]
    pub fn new(interface: impl Into<Box<str>>) -> Self {
        Self {
            interface: interface.into(),
            common: TransportConfig::default(),
        }
    }
}

fn set_socket_option<T>(
    fd: RawFd,
    level: libc::c_int,
    option: libc::c_int,
    value: &T,
) -> io::Result<()> {
    let length = libc::socklen_t::try_from(size_of::<T>())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket option 過大"))?;
    // SAFETY: value 指向至少 length 位元組的已初始化 T；setsockopt 只在
    // 同步呼叫期間讀取緩衝，不保留指標。
    let result = unsafe {
        libc::setsockopt(
            fd,
            level,
            option,
            ptr::from_ref(value).cast::<c_void>(),
            length,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn kernel_filters(filter: &FilterSet) -> Vec<libc::can_filter> {
    if filter.is_accept_all() {
        return vec![libc::can_filter {
            can_id: 0,
            can_mask: 0,
        }];
    }
    filter
        .rules()
        .iter()
        .map(|rule| {
            let (id, mask, inverted) = rule.parts();
            libc::can_filter {
                can_id: id | if inverted { libc::CAN_INV_FILTER } else { 0 },
                can_mask: mask,
            }
        })
        .collect()
}

fn apply_filters(fd: RawFd, filter: &FilterSet) -> io::Result<()> {
    let filters = kernel_filters(filter);
    let bytes = filters
        .len()
        .checked_mul(size_of::<libc::can_filter>())
        .and_then(|value| libc::socklen_t::try_from(value).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "CAN filter 過多"))?;
    // SAFETY: filters 在呼叫期間保持有效，bytes 精確等於連續陣列大小；
    // 核心只複製內容，不保留使用者空間指標。
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_CAN_RAW,
            libc::CAN_RAW_FILTER,
            filters.as_ptr().cast::<c_void>(),
            bytes,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn can_id(frame: &Frame) -> u32 {
    frame.id().as_raw()
        | if frame.id().is_extended() {
            libc::CAN_EFF_FLAG
        } else {
            0
        }
        | if frame.is_remote() {
            libc::CAN_RTR_FLAG
        } else {
            0
        }
}

fn decode_id(raw: u32) -> Option<CanId> {
    if raw & libc::CAN_EFF_FLAG != 0 {
        CanId::extended(raw & libc::CAN_EFF_MASK).ok()
    } else {
        u16::try_from(raw & libc::CAN_SFF_MASK)
            .ok()
            .and_then(|id| CanId::standard(id).ok())
    }
}

fn classic_to_frame(raw: &libc::can_frame) -> Option<Frame> {
    let id = decode_id(raw.can_id)?;
    let len = raw.can_dlc;
    if len > 8 {
        return None;
    }
    if raw.can_id & libc::CAN_RTR_FLAG != 0 {
        Frame::remote(id, len).ok()
    } else {
        Frame::new(id, &raw.data[..usize::from(len)]).ok()
    }
}

fn fd_to_frame(raw: &libc::canfd_frame) -> Option<Frame> {
    let id = decode_id(raw.can_id)?;
    // `canfd_frame.len` 是位元組長度（0..=64），與 PCAN FD DLC 不同；
    // `can_frame.can_dlc` 在新核心則是 0..=8 長度。兩條轉換不可共用。
    len_to_dlc(raw.len)?;
    Frame::new_fd(
        id,
        &raw.data[..usize::from(raw.len)],
        raw.flags & u8::try_from(libc::CANFD_BRS).unwrap_or(0) != 0,
    )
    .ok()?
    .with_esi(raw.flags & u8::try_from(libc::CANFD_ESI).unwrap_or(0) != 0)
    .ok()
}

fn software_timestamp() -> Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
        });
    Timestamp::new(micros, TimestampSource::Software)
}

const CONTROL_BUFFER_CAPACITY: usize = 64;

#[repr(C)]
struct ControlBuffer {
    _alignment: [libc::cmsghdr; 0],
    bytes: [u8; CONTROL_BUFFER_CAPACITY],
}

impl ControlBuffer {
    const fn new() -> Self {
        Self {
            _alignment: [],
            bytes: [0; CONTROL_BUFFER_CAPACITY],
        }
    }

    fn as_mut_ptr(&mut self) -> *mut c_void {
        self.bytes.as_mut_ptr().cast()
    }

    const fn len(&self) -> usize {
        self.bytes.len()
    }
}

const _: () = {
    assert!(core::mem::align_of::<ControlBuffer>() >= core::mem::align_of::<libc::cmsghdr>());
    assert!(core::mem::offset_of!(ControlBuffer, bytes) == 0);
    assert!(core::mem::size_of::<ControlBuffer>() >= CONTROL_BUFFER_CAPACITY);
};

fn timestamp_control_len() -> Option<usize> {
    let value_size = u32::try_from(core::mem::size_of::<libc::timespec>()).ok()?;
    // SAFETY: CMSG_LEN 僅依傳入的 timespec 大小計算控制訊息所需長度，
    // 不解參考指標或存取外部記憶體。
    usize::try_from(unsafe { libc::CMSG_LEN(value_size) }).ok()
}

fn timestamp_from_control(
    data: &[u8],
    cmsg_level: libc::c_int,
    cmsg_type: libc::c_int,
    cmsg_len: usize,
) -> Option<Timestamp> {
    if cmsg_level != libc::SOL_SOCKET || cmsg_type != libc::SCM_TIMESTAMPNS {
        return None;
    }
    let value_size = core::mem::size_of::<libc::timespec>();
    let required_len = timestamp_control_len()?;
    if cmsg_len < required_len || data.len() < value_size {
        return None;
    }
    // SAFETY: 已確認 level/type 為 SCM_TIMESTAMPNS、cmsg_len 與資料切片
    // 足以容納 timespec；read_unaligned 不要求來源指標符合 timespec 對齊。
    let value = unsafe { ptr::read_unaligned(data.as_ptr().cast::<libc::timespec>()) };
    let seconds = u64::try_from(value.tv_sec).ok()?;
    let nanos = u64::try_from(value.tv_nsec).ok()?;
    Some(Timestamp::new(
        seconds.saturating_mul(1_000_000) + nanos / 1_000,
        TimestampSource::Kernel,
    ))
}

fn control_timestamp(header: &libc::msghdr) -> Option<Timestamp> {
    // SAFETY: header 由成功 recvmsg 填入，msg_control 指向 ControlBuffer；
    // 該型別明確提供至少 cmsghdr 的對齊，CMSG_FIRSTHDR 只在
    // msg_controllen 範圍內計算第一個控制訊息位置。
    let mut current = unsafe { libc::CMSG_FIRSTHDR(header) };
    while !current.is_null() {
        // SAFETY: current 由 CMSG_FIRSTHDR/NXTHDR 產生且仍位於對齊至
        // cmsghdr 的 ControlBuffer 內，可安全解參考為 cmsghdr。
        let control = unsafe { &*current };
        if control.cmsg_level == libc::SOL_SOCKET && control.cmsg_type == libc::SCM_TIMESTAMPNS {
            let value_size = core::mem::size_of::<libc::timespec>();
            let required_len = timestamp_control_len()?;
            // Linux 的 cmsg_len 目前即為 usize；保留顯式 checked conversion，
            // 使此 ABI 邊界不依賴 libc 欄位型別永遠不變。
            #[allow(clippy::useless_conversion)]
            let cmsg_len = usize::try_from(control.cmsg_len).ok();
            if let Some(cmsg_len) = cmsg_len
                && cmsg_len >= required_len
            {
                // SAFETY: current 位於 recvmsg 驗證過的控制訊息鏈中，且上方已確認
                // cmsg_len 足以容納完整 timespec，因此 CMSG_DATA 後至少有 value_size bytes。
                let data = unsafe {
                    core::slice::from_raw_parts(libc::CMSG_DATA(current).cast(), value_size)
                };
                if let Some(timestamp) =
                    timestamp_from_control(data, control.cmsg_level, control.cmsg_type, cmsg_len)
                {
                    return Some(timestamp);
                }
            }
        }
        // SAFETY: header 與 current 同屬成功 recvmsg 產生的控制訊息鏈；
        // libc 會檢查下一個項目不超出 msg_controllen。
        current = unsafe { libc::CMSG_NXTHDR(header, current) };
    }
    None
}

/// 一個已開啟的 Linux `SocketCAN` raw socket。
pub struct CanSocket {
    io: AsyncFd<OwnedFd>,
    caps: Capabilities,
    kernel_timestamps: bool,
    closed: AtomicBool,
    status: Mutex<BusStatus>,
}

impl core::fmt::Debug for CanSocket {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("CanSocket")
            .field("fd", &self.io.get_ref().as_raw_fd())
            .field("caps", &self.caps)
            .field("kernel_timestamps", &self.kernel_timestamps)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl CanSocket {
    #[allow(clippy::too_many_lines)]
    fn open(config: &SocketCanConfig) -> Result<Self, Error> {
        if config.common.listen_only {
            return Err(Error::Unsupported(
                "SocketCAN 唯聽模式必須由 ip link 在介面層設定",
            ));
        }
        if config.common.receive_status_frames {
            #[cfg(feature = "tracing")]
            tracing::debug!("SocketCAN 沒有獨立狀態幀；狀態將由核心錯誤幀推導");
        }
        if config.common.bus_off_auto_reset {
            #[cfg(feature = "tracing")]
            tracing::debug!("SocketCAN Bus-Off 自動復歸由介面 restart-ms 管理，不由 socket 設定");
        }
        let name = CString::new(config.interface.as_ref())
            .map_err(|_| ConfigError::InvalidChannel("SocketCAN 介面名稱含有 NUL".into()))?;
        // SAFETY: socket 參數是 Linux CAN raw ABI 常數；成功 fd 立即移交
        // OwnedFd，所有錯誤路徑由 RAII 關閉。
        let raw_fd = unsafe {
            libc::socket(
                libc::PF_CAN,
                libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                libc::CAN_RAW,
            )
        };
        if raw_fd < 0 {
            return Err(socket_error(
                "socket(PF_CAN)",
                FaultKind::Fatal,
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: raw_fd 是剛由 socket 成功建立、尚未交給其他所有者的 fd。
        let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        // SAFETY: CString 保證 NUL 結尾且指標在呼叫期間有效。
        let interface_index = unsafe { libc::if_nametoindex(name.as_ptr()) };
        if interface_index == 0 {
            let source = io::Error::last_os_error();
            return Err(Error::Open {
                channel: config.interface.clone(),
                source: BackendError::SocketCan {
                    op: "if_nametoindex",
                    kind: FaultKind::Fatal,
                    source,
                },
            });
        }
        let one = 1_i32;
        let fd_enabled =
            set_socket_option(raw_fd, libc::SOL_CAN_RAW, libc::CAN_RAW_FD_FRAMES, &one).is_ok();
        if config.common.bitrate.is_fd() && !fd_enabled {
            return Err(Error::Unsupported(
                "核心或 SocketCAN 介面不支援 CAN_RAW_FD_FRAMES",
            ));
        }
        if !fd_enabled {
            #[cfg(feature = "tracing")]
            tracing::warn!("SocketCAN 不支援 CAN FD，能力已降級為古典 CAN");
        }
        let error_mask: libc::can_err_mask_t = if config.common.receive_error_frames {
            libc::CAN_ERR_MASK
        } else {
            0
        };
        set_socket_option(
            raw_fd,
            libc::SOL_CAN_RAW,
            libc::CAN_RAW_ERR_FILTER,
            &error_mask,
        )
        .map_err(|source| {
            socket_error("setsockopt(CAN_RAW_ERR_FILTER)", FaultKind::Fatal, source)
        })?;
        let own = i32::from(config.common.receive_own_frames);
        set_socket_option(raw_fd, libc::SOL_CAN_RAW, libc::CAN_RAW_RECV_OWN_MSGS, &own).map_err(
            |source| {
                socket_error(
                    "setsockopt(CAN_RAW_RECV_OWN_MSGS)",
                    FaultKind::Fatal,
                    source,
                )
            },
        )?;
        apply_filters(raw_fd, &config.common.filter).map_err(|source| {
            socket_error("setsockopt(CAN_RAW_FILTER)", FaultKind::Fatal, source)
        })?;
        let requested_bytes = config
            .common
            .rx_queue_capacity
            .saturating_mul(libc::CANFD_MTU);
        let buffer_size = i32::try_from(requested_bytes).unwrap_or(i32::MAX);
        set_socket_option(raw_fd, libc::SOL_SOCKET, libc::SO_RCVBUF, &buffer_size)
            .map_err(|source| socket_error("setsockopt(SO_RCVBUF)", FaultKind::Fatal, source))?;
        set_socket_option(raw_fd, libc::SOL_SOCKET, libc::SO_SNDBUF, &buffer_size)
            .map_err(|source| socket_error("setsockopt(SO_SNDBUF)", FaultKind::Fatal, source))?;
        let kernel_timestamps =
            set_socket_option(raw_fd, libc::SOL_SOCKET, libc::SO_TIMESTAMPNS, &one).is_ok();
        if !kernel_timestamps {
            #[cfg(feature = "tracing")]
            tracing::warn!(
                "SO_TIMESTAMPNS 無法啟用，SocketCAN 時間戳明確降級為使用者空間 Software"
            );
        }
        // SAFETY: sockaddr_can 是純 C POD；全零是有效初始狀態，隨後設定
        // family 與介面索引，匿名 union 保持零值。
        let mut address: libc::sockaddr_can = unsafe { zeroed() };
        address.can_family = libc::sa_family_t::try_from(libc::AF_CAN).unwrap_or(0);
        address.can_ifindex = i32::try_from(interface_index).unwrap_or(i32::MAX);
        let address_length = libc::socklen_t::try_from(size_of::<libc::sockaddr_can>())
            .map_err(|_| Error::Unsupported("sockaddr_can 大小無法表示"))?;
        // SAFETY: address 指向已初始化 sockaddr_can，長度與型別精確一致。
        let bound = unsafe {
            libc::bind(
                raw_fd,
                ptr::from_ref(&address).cast::<libc::sockaddr>(),
                address_length,
            )
        };
        if bound != 0 {
            return Err(Error::Open {
                channel: config.interface.clone(),
                source: BackendError::SocketCan {
                    op: "bind(AF_CAN)",
                    kind: FaultKind::Fatal,
                    source: io::Error::last_os_error(),
                },
            });
        }
        let io = AsyncFd::new(owned)
            .map_err(|source| socket_error("AsyncFd::new(SocketCAN)", FaultKind::Fatal, source))?;
        let mut caps = Capabilities::default();
        caps.can_fd = fd_enabled;
        caps.brs = fd_enabled;
        caps.echo_frames = config.common.receive_own_frames;
        caps.hardware_filter = true;
        caps.hardware_timestamps = false;
        caps.listen_only = false;
        Ok(Self {
            io,
            caps,
            kernel_timestamps,
            closed: AtomicBool::new(false),
            status: Mutex::new(BusStatus::default()),
        })
    }

    fn try_recv_one(&self) -> Result<Option<TransportEvent>, Error> {
        let mut raw = MaybeUninit::<libc::canfd_frame>::uninit();
        let mut iovec = libc::iovec {
            iov_base: raw.as_mut_ptr().cast::<c_void>(),
            iov_len: libc::CANFD_MTU,
        };
        let mut control = ControlBuffer::new();
        // SAFETY: msghdr 全零是 POSIX 規定的有效空初始值，隨後填入唯一
        // iovec 與固定堆疊控制緩衝；ControlBuffer 明確保證 cmsghdr 對齊。
        let mut header: libc::msghdr = unsafe { zeroed() };
        header.msg_iov = &raw mut iovec;
        header.msg_iovlen = 1;
        header.msg_control = control.as_mut_ptr();
        header.msg_controllen = control.len();
        // SAFETY: header 描述的資料與控制緩衝皆有效、可寫且在同步呼叫
        // 期間存活；fd 為 nonblocking SocketCAN raw socket。
        let received = unsafe { libc::recvmsg(self.io.get_ref().as_raw_fd(), &raw mut header, 0) };
        if received < 0 {
            let source = io::Error::last_os_error();
            if source.kind() == io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(socket_error(
                "recvmsg(SocketCAN)",
                errno_kind(&source),
                source,
            ));
        }
        let length = usize::try_from(received).unwrap_or(0);
        let timestamp = if self.kernel_timestamps {
            control_timestamp(&header).unwrap_or_else(software_timestamp)
        } else {
            software_timestamp()
        };
        let is_echo = header.msg_flags & libc::MSG_CONFIRM != 0;
        if length == libc::CAN_MTU {
            // SAFETY: recvmsg 已初始化前 CAN_MTU 位元組；buffer 對齊至少等於
            // canfd_frame（8），也符合 can_frame，故可只讀完整 classical 結構。
            let classical = unsafe { ptr::read(raw.as_ptr().cast::<libc::can_frame>()) };
            if classical.can_id & libc::CAN_ERR_FLAG != 0 {
                let status = parse_error_frame(classical.can_id, &classical.data);
                *lock(&self.status) = status;
                return Ok(Some(TransportEvent::Status(status)));
            }
            let frame = classic_to_frame(&classical).ok_or_else(|| {
                socket_error(
                    "解析 can_frame",
                    FaultKind::Permanent,
                    io::Error::new(io::ErrorKind::InvalidData, "無效的 classical CAN 幀"),
                )
            })?;
            return Ok(Some(TransportEvent::Frame(RxFrame::new(
                frame, timestamp, is_echo,
            ))));
        }
        if length == libc::CANFD_MTU {
            // SAFETY: recvmsg 已寫滿 CANFD_MTU，整個 canfd_frame 均已初始化。
            let fd = unsafe { raw.assume_init() };
            let frame = fd_to_frame(&fd).ok_or_else(|| {
                socket_error(
                    "解析 canfd_frame",
                    FaultKind::Permanent,
                    io::Error::new(io::ErrorKind::InvalidData, "無效的 CAN FD 幀"),
                )
            })?;
            return Ok(Some(TransportEvent::Frame(RxFrame::new(
                frame, timestamp, is_echo,
            ))));
        }
        Err(socket_error(
            "recvmsg(SocketCAN) 長度",
            FaultKind::Permanent,
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("核心回傳非 CAN_MTU/CANFD_MTU 長度 {length}"),
            ),
        ))
    }

    fn try_send_one(&self, frame: &Frame) -> io::Result<()> {
        let (pointer, length);
        let mut classical = MaybeUninit::<libc::can_frame>::uninit();
        let mut fd = MaybeUninit::<libc::canfd_frame>::uninit();
        if frame.is_fd() {
            // SAFETY: canfd_frame 是純 C POD；全零是有效保留欄位狀態。
            let mut value: libc::canfd_frame = unsafe { zeroed() };
            value.can_id = can_id(frame);
            value.len = u8::try_from(frame.len()).unwrap_or(64);
            value.flags = u8::try_from(libc::CANFD_FDF).unwrap_or(0);
            if frame.flags().contains(FrameFlags::BRS) {
                value.flags |= u8::try_from(libc::CANFD_BRS).unwrap_or(0);
            }
            if frame.flags().contains(FrameFlags::ESI) {
                value.flags |= u8::try_from(libc::CANFD_ESI).unwrap_or(0);
            }
            value.data[..frame.data().len()].copy_from_slice(frame.data());
            fd.write(value);
            pointer = fd.as_ptr().cast::<c_void>();
            length = libc::CANFD_MTU;
        } else {
            // SAFETY: can_frame 是純 C POD；全零是有效保留欄位狀態。
            let mut value: libc::can_frame = unsafe { zeroed() };
            value.can_id = can_id(frame);
            value.can_dlc = u8::try_from(frame.len()).unwrap_or(8);
            value.data[..frame.data().len()].copy_from_slice(frame.data());
            classical.write(value);
            pointer = classical.as_ptr().cast::<c_void>();
            length = libc::CAN_MTU;
        }
        // 啟用 CAN_RAW_FD_FRAMES 後仍必須靠 send 長度區分格式：古典幀用
        // CAN_MTU、FD 幀用 CANFD_MTU；核心不只看 flags。
        // SAFETY: pointer 指向上方仍存活且已完整初始化的對應結構，length
        // 與實際結構完全一致；send 不保留緩衝指標。
        let sent = unsafe { libc::send(self.io.get_ref().as_raw_fd(), pointer, length, 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if usize::try_from(sent).unwrap_or(0) != length {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "SocketCAN 未完整送出單一 datagram",
            ));
        }
        Ok(())
    }
}

#[allow(clippy::manual_async_fn)]
impl Transport for CanSocket {
    fn recv(&self) -> impl Future<Output = Result<TransportEvent, Error>> + Send {
        async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            loop {
                if let Some(event) = self.try_recv_one()? {
                    return Ok(event);
                }
                let mut guard = self.io.readable().await.map_err(|source| {
                    socket_error("AsyncFd::readable(SocketCAN)", FaultKind::Fatal, source)
                })?;
                let mut backend_failure = None;
                // 與 PCAN Linux 路徑相同，必須由 try_io 依 ReadyEvent tick
                // 清除 EPOLLET 就緒；讀空後手動 clear_ready 會抹掉競態期間
                // 新到達的 edge，造成永久落後一幀。
                match guard.try_io(|_| match self.try_recv_one() {
                    Ok(Some(event)) => Ok(event),
                    Ok(None) => Err(io::ErrorKind::WouldBlock.into()),
                    Err(error) => {
                        backend_failure = Some(error);
                        Err(io::Error::other("SocketCAN 接收失敗"))
                    }
                }) {
                    Ok(Ok(event)) => return Ok(event),
                    Ok(Err(source)) => {
                        return Err(backend_failure.unwrap_or_else(|| {
                            socket_error("recvmsg(SocketCAN)", FaultKind::Fatal, source)
                        }));
                    }
                    Err(_would_block) => {}
                }
            }
        }
    }

    fn send(&self, frame: &Frame) -> impl Future<Output = Result<(), Error>> + Send {
        let frame = *frame;
        async move {
            if self.closed.load(Ordering::Acquire) {
                return Err(Error::Closed);
            }
            if frame.is_fd() && !self.caps.can_fd {
                return Err(Error::Unsupported("此 SocketCAN socket 未啟用 CAN FD"));
            }
            loop {
                match self.try_send_one(&frame) {
                    Ok(()) => return Ok(()),
                    Err(source)
                        if matches!(source.raw_os_error(), Some(libc::EAGAIN | libc::ENOBUFS)) => {}
                    Err(source) => {
                        return Err(socket_error("send(SocketCAN)", errno_kind(&source), source));
                    }
                }
                let mut guard = self.io.writable().await.map_err(|source| {
                    socket_error("AsyncFd::writable(SocketCAN)", FaultKind::Fatal, source)
                })?;
                match guard.try_io(|_| match self.try_send_one(&frame) {
                    Ok(()) => Ok(()),
                    Err(source)
                        if matches!(source.raw_os_error(), Some(libc::EAGAIN | libc::ENOBUFS)) =>
                    {
                        Err(io::ErrorKind::WouldBlock.into())
                    }
                    Err(source) => Err(source),
                }) {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(source)) => {
                        return Err(socket_error("send(SocketCAN)", errno_kind(&source), source));
                    }
                    Err(_would_block) => {}
                }
            }
        }
    }

    fn status(&self) -> impl Future<Output = Result<BusStatus, Error>> + Send {
        let status = *lock(&self.status);
        core::future::ready(if self.closed.load(Ordering::Acquire) {
            Err(Error::Closed)
        } else {
            Ok(status)
        })
    }

    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<(), Error>> + Send {
        let result = if self.closed.load(Ordering::Acquire) {
            Err(Error::Closed)
        } else {
            apply_filters(self.io.get_ref().as_raw_fd(), filter).map_err(|source| {
                socket_error("setsockopt(CAN_RAW_FILTER)", FaultKind::Permanent, source)
            })
        };
        core::future::ready(result)
    }

    fn close(&self) -> impl Future<Output = ()> + Send {
        self.closed.store(true, Ordering::Release);
        core::future::ready(())
    }

    fn capabilities(&self) -> Capabilities {
        self.caps
    }
}

/// 依設定建立 [`CanSocket`] 的可重連工廠。
#[derive(Clone, Debug)]
pub struct SocketCanFactory {
    config: SocketCanConfig,
    describe: String,
}

impl SocketCanFactory {
    /// 建立 `SocketCAN` 工廠；實際 socket 會在每次 `open()` 建立。
    ///
    /// # Errors
    ///
    /// 介面名稱為空或含 NUL 時回傳設定錯誤。
    pub fn new(config: SocketCanConfig) -> Result<Self, Error> {
        if config.interface.is_empty() || config.interface.as_bytes().contains(&0) {
            return Err(ConfigError::InvalidChannel(config.interface.clone()).into());
        }
        let mode = match config.common.bitrate {
            Bitrate::Classic { .. } => "classic",
            Bitrate::Fd { .. } => "fd",
            _ => "unknown",
        };
        let describe = format!("socketcan:{}:{mode}", config.interface);
        Ok(Self { config, describe })
    }
}

#[allow(clippy::manual_async_fn)]
impl TransportFactory for SocketCanFactory {
    type Transport = CanSocket;

    fn open(&self) -> impl Future<Output = Result<Self::Transport, Error>> + Send {
        async move {
            // 這裡只有微秒級 socket 系統呼叫；保持惰性即可，送入阻塞池反而
            // 會增加排程與跨執行緒成本。
            CanSocket::open(&self.config)
        }
    }

    fn describe(&self) -> &str {
        &self.describe
    }
}

#[cfg(test)]
mod tests {
    use pcan_core::{CanId, FaultKind, Frame, TimestampSource};

    use super::{
        ControlBuffer, classic_to_frame, errno_kind, fd_to_frame, timestamp_control_len,
        timestamp_from_control,
    };

    fn cmsg_len() -> usize {
        timestamp_control_len().unwrap_or_else(|| unreachable!("timespec 大小應可轉為 cmsg 長度"))
    }

    #[test]
    fn distinguishes_classic_and_fd_length_semantics() {
        // SAFETY: libc CAN 結構是純 C POD，全零為有效空幀。
        let mut classical: libc::can_frame = unsafe { core::mem::zeroed() };
        classical.can_id = 0x123;
        classical.can_dlc = 8;
        assert_eq!(
            classic_to_frame(&classical).map(|frame| frame.len()),
            Some(8)
        );

        // SAFETY: libc CAN FD 結構是純 C POD，全零為有效空幀。
        let mut fd: libc::canfd_frame = unsafe { core::mem::zeroed() };
        fd.can_id = libc::CAN_EFF_FLAG | 0x123;
        fd.len = 12;
        fd.flags = u8::try_from(libc::CANFD_FDF | libc::CANFD_BRS).unwrap_or(0);
        let decoded = fd_to_frame(&fd).unwrap_or_else(|| unreachable!("有效 FD 幀"));
        let expected = Frame::new_fd(
            CanId::extended(0x123).unwrap_or_else(|error| unreachable!("{error}")),
            &[0; 12],
            true,
        )
        .unwrap_or_else(|error| unreachable!("{error}"));
        assert_eq!(decoded, expected);
    }

    #[test]
    fn classifies_socket_backpressure_and_device_loss() {
        for code in [libc::EAGAIN, libc::ENOBUFS] {
            assert_eq!(
                errno_kind(&std::io::Error::from_raw_os_error(code)),
                FaultKind::Transient
            );
        }
        for code in [libc::ENETDOWN, libc::ENODEV] {
            assert_eq!(
                errno_kind(&std::io::Error::from_raw_os_error(code)),
                FaultKind::Fatal
            );
        }
    }

    #[test]
    fn aligns_control_buffer_for_cmsghdr() {
        let mut control = ControlBuffer::new();
        assert_eq!(
            control.as_mut_ptr().addr() % core::mem::align_of::<libc::cmsghdr>(),
            0
        );
        assert!(control.len() >= 64);
    }

    #[test]
    fn parses_valid_kernel_timestamp_control_message() {
        let value = libc::timespec {
            tv_sec: 42,
            tv_nsec: 123_456_789,
        };
        let mut data = [0_u8; core::mem::size_of::<libc::timespec>()];
        // SAFETY: data 足以容納完整 timespec；write_unaligned 不要求其位址對齊。
        unsafe { core::ptr::write_unaligned(data.as_mut_ptr().cast(), value) };
        let timestamp =
            timestamp_from_control(&data, libc::SOL_SOCKET, libc::SCM_TIMESTAMPNS, cmsg_len())
                .unwrap_or_else(|| unreachable!("有效的 SCM_TIMESTAMPNS 應產生時間戳"));

        assert_eq!(timestamp.micros(), 42_123_456);
        assert_eq!(timestamp.source(), TimestampSource::Kernel);
    }

    #[test]
    fn skips_timestamp_control_message_with_short_length() {
        let data = [0_u8; core::mem::size_of::<libc::timespec>()];
        let short_len = cmsg_len().saturating_sub(1);

        assert_eq!(
            timestamp_from_control(&data, libc::SOL_SOCKET, libc::SCM_TIMESTAMPNS, short_len,),
            None
        );
    }

    #[test]
    fn ignores_non_timestamp_control_message() {
        let data = [0_u8; core::mem::size_of::<libc::timespec>()];
        assert_eq!(
            timestamp_from_control(&data, libc::SOL_SOCKET, libc::SCM_RIGHTS, cmsg_len()),
            None
        );
    }
}
