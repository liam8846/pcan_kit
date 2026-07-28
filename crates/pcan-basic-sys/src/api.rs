use core::ffi::{c_char, c_void};
use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use libloading::Library;
use pcan_core::LoadError;

use crate::{
    TPCANBaudrate, TPCANBitrateFD, TPCANHandle, TPCANMode, TPCANMsg, TPCANMsgFD, TPCANParameter,
    TPCANStatus, TPCANTimestamp, TPCANTimestampFD, TPCANType,
};

/// `CAN_Initialize` 函式指標。
pub type FnInitialize =
    unsafe extern "system" fn(TPCANHandle, TPCANBaudrate, TPCANType, u32, u16) -> TPCANStatus;
/// `CAN_InitializeFD` 函式指標。
pub type FnInitializeFD = unsafe extern "system" fn(TPCANHandle, TPCANBitrateFD) -> TPCANStatus;
/// `CAN_Uninitialize` 函式指標。
pub type FnUninitialize = unsafe extern "system" fn(TPCANHandle) -> TPCANStatus;
/// `CAN_Reset` 函式指標。
pub type FnReset = unsafe extern "system" fn(TPCANHandle) -> TPCANStatus;
/// `CAN_GetStatus` 函式指標。
pub type FnGetStatus = unsafe extern "system" fn(TPCANHandle) -> TPCANStatus;
/// `CAN_Read` 函式指標。
pub type FnRead =
    unsafe extern "system" fn(TPCANHandle, *mut TPCANMsg, *mut TPCANTimestamp) -> TPCANStatus;
/// `CAN_ReadFD` 函式指標。
pub type FnReadFD =
    unsafe extern "system" fn(TPCANHandle, *mut TPCANMsgFD, *mut TPCANTimestampFD) -> TPCANStatus;
/// `CAN_Write` 函式指標。
pub type FnWrite = unsafe extern "system" fn(TPCANHandle, *mut TPCANMsg) -> TPCANStatus;
/// `CAN_WriteFD` 函式指標。
pub type FnWriteFD = unsafe extern "system" fn(TPCANHandle, *mut TPCANMsgFD) -> TPCANStatus;
/// `CAN_FilterMessages` 函式指標。
pub type FnFilterMessages =
    unsafe extern "system" fn(TPCANHandle, u32, u32, TPCANMode) -> TPCANStatus;
/// `CAN_GetValue` 函式指標。
pub type FnGetValue =
    unsafe extern "system" fn(TPCANHandle, TPCANParameter, *mut c_void, u32) -> TPCANStatus;
/// `CAN_SetValue` 函式指標。
pub type FnSetValue =
    unsafe extern "system" fn(TPCANHandle, TPCANParameter, *mut c_void, u32) -> TPCANStatus;
/// `CAN_GetErrorText` 函式指標。
pub type FnGetErrorText = unsafe extern "system" fn(TPCANStatus, u16, *mut c_char) -> TPCANStatus;
/// `CAN_LookUpChannel` 函式指標。
pub type FnLookUpChannel = unsafe extern "system" fn(*mut c_char, *mut TPCANHandle) -> TPCANStatus;

/// 已載入的 PCAN-Basic 函式庫。
pub struct PcanApi {
    initialize: FnInitialize,
    initialize_fd: Option<FnInitializeFD>,
    uninitialize: FnUninitialize,
    reset: FnReset,
    get_status: FnGetStatus,
    read: FnRead,
    read_fd: Option<FnReadFD>,
    write: FnWrite,
    write_fd: Option<FnWriteFD>,
    filter_messages: FnFilterMessages,
    get_value: FnGetValue,
    set_value: FnSetValue,
    get_error_text: FnGetErrorText,
    look_up_channel: Option<FnLookUpChannel>,
    // 安全關鍵：函式指標只在此 Library 存活時有效。Rust 依欄位宣告順序
    // drop，因此 Library 必須放最後，確保前面所有指標先失效再卸載映像。
    _lib: Library,
}

impl core::fmt::Debug for PcanApi {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PcanApi")
            .field("supports_fd", &self.supports_fd())
            .field("supports_lookup", &self.look_up_channel.is_some())
            .finish_non_exhaustive()
    }
}

impl PcanApi {
    /// 初始化古典 CAN 通道。
    #[must_use]
    pub fn initialize(&self, channel: TPCANHandle, baudrate: TPCANBaudrate) -> TPCANStatus {
        // SAFETY: 函式指標在載入時依正確簽章解析，且 `_lib` 在整個呼叫期間存活。
        unsafe { (self.initialize)(channel, baudrate, 0, 0, 0) }
    }

    /// 初始化 CAN FD 通道；舊版函式庫不支援時回傳 `None`。
    #[must_use]
    pub fn initialize_fd(&self, channel: TPCANHandle, bitrate: &CStr) -> Option<TPCANStatus> {
        self.initialize_fd.map(|function| {
            // SAFETY: 呼叫者提供的字串指標由上層 `CString` 保持到呼叫返回，
            // 函式指標簽章亦已在載入時驗證。
            unsafe { function(channel, bitrate.as_ptr().cast_mut()) }
        })
    }

    /// 解除初始化指定通道。
    #[must_use]
    pub fn uninitialize(&self, channel: TPCANHandle) -> TPCANStatus {
        // SAFETY: 函式指標簽章正確，通道值是純量，且 Library 仍由 self 持有。
        unsafe { (self.uninitialize)(channel) }
    }

    /// 重設指定通道的收送佇列。
    #[must_use]
    pub fn reset(&self, channel: TPCANHandle) -> TPCANStatus {
        // SAFETY: 函式指標簽章正確，且 Library 仍由 self 持有。
        unsafe { (self.reset)(channel) }
    }

    /// 查詢指定通道的原始狀態位元。
    #[must_use]
    pub fn get_status(&self, channel: TPCANHandle) -> TPCANStatus {
        // SAFETY: 函式指標簽章正確，且 Library 仍由 self 持有。
        unsafe { (self.get_status)(channel) }
    }

    /// 嘗試讀取一個古典 CAN 訊息及其硬體時間戳。
    #[must_use]
    pub fn read(&self, channel: TPCANHandle) -> (TPCANStatus, TPCANMsg, TPCANTimestamp) {
        let mut message = TPCANMsg::default();
        let mut timestamp = TPCANTimestamp::default();
        // SAFETY: 兩個輸出指標都指向大小與對齊經編譯期驗證的可寫堆疊值，
        // 且其生命週期涵蓋整個同步 FFI 呼叫。
        let status = unsafe { (self.read)(channel, &raw mut message, &raw mut timestamp) };
        (status, message, timestamp)
    }

    /// 嘗試讀取一個 CAN FD 訊息；舊版函式庫不支援時回傳 `None`。
    #[must_use]
    pub fn read_fd(
        &self,
        channel: TPCANHandle,
    ) -> Option<(TPCANStatus, TPCANMsgFD, TPCANTimestampFD)> {
        self.read_fd.map(|function| {
            let mut message = TPCANMsgFD::default();
            let mut timestamp = 0;
            // SAFETY: 輸出指標指向有效且對齊的堆疊值，生命週期涵蓋同步呼叫。
            let status = unsafe { function(channel, &raw mut message, &raw mut timestamp) };
            (status, message, timestamp)
        })
    }

    /// 送出一個古典 CAN 訊息。
    #[must_use]
    pub fn write(&self, channel: TPCANHandle, message: &TPCANMsg) -> TPCANStatus {
        let mut stack_copy = *message;
        // SAFETY: C 原型雖接收可變指標，語意上不保留指標；傳入的是獨立、
        // 可寫且版面相容的堆疊副本。
        unsafe { (self.write)(channel, &raw mut stack_copy) }
    }

    /// 送出一個 CAN FD 訊息；舊版函式庫不支援時回傳 `None`。
    #[must_use]
    pub fn write_fd(&self, channel: TPCANHandle, message: &TPCANMsgFD) -> Option<TPCANStatus> {
        self.write_fd.map(|function| {
            let mut stack_copy = *message;
            // SAFETY: 傳入可寫、版面相容且只在同步呼叫期間存活的堆疊副本。
            unsafe { function(channel, &raw mut stack_copy) }
        })
    }

    /// 設定單一 PCAN 識別碼區間過濾器。
    #[must_use]
    pub fn filter_messages(
        &self,
        channel: TPCANHandle,
        from: u32,
        to: u32,
        mode: TPCANMode,
    ) -> TPCANStatus {
        // SAFETY: 函式指標簽章正確，所有參數皆為純量且 Library 存活。
        unsafe { (self.filter_messages)(channel, from, to, mode) }
    }

    /// 讀取 `u32` 型 PCAN 參數。
    #[must_use]
    pub fn get_value_u32(
        &self,
        channel: TPCANHandle,
        parameter: TPCANParameter,
    ) -> (TPCANStatus, u32) {
        let mut value = 0_u32;
        // SAFETY: 緩衝指向有效的四位元組可寫值，傳入大小與型別完全一致。
        let status = unsafe {
            (self.get_value)(
                channel,
                parameter,
                (&raw mut value).cast(),
                u32::try_from(core::mem::size_of::<u32>()).unwrap_or(4),
            )
        };
        (status, value)
    }

    /// 讀取 `i32` 型 PCAN 參數，例如 Linux 接收事件 fd。
    #[must_use]
    pub fn get_value_i32(
        &self,
        channel: TPCANHandle,
        parameter: TPCANParameter,
    ) -> (TPCANStatus, i32) {
        let mut value = -1_i32;
        // SAFETY: 緩衝指向有效的四位元組可寫值，傳入大小與型別完全一致。
        let status = unsafe {
            (self.get_value)(
                channel,
                parameter,
                (&raw mut value).cast(),
                u32::try_from(core::mem::size_of::<i32>()).unwrap_or(4),
            )
        };
        (status, value)
    }

    /// 設定 `u32` 型 PCAN 參數。
    #[must_use]
    pub fn set_value_u32(
        &self,
        channel: TPCANHandle,
        parameter: TPCANParameter,
        value: u32,
    ) -> TPCANStatus {
        let mut stack_value = value;
        // SAFETY: 緩衝指向有效的四位元組堆疊值，驅動只在同步呼叫中讀取。
        unsafe {
            (self.set_value)(
                channel,
                parameter,
                (&raw mut stack_value).cast(),
                u32::try_from(core::mem::size_of::<u32>()).unwrap_or(4),
            )
        }
    }

    /// 設定指標寬度的 PCAN 參數，例如 Windows 接收事件 HANDLE。
    #[must_use]
    pub fn set_value_usize(
        &self,
        channel: TPCANHandle,
        parameter: TPCANParameter,
        value: usize,
    ) -> TPCANStatus {
        let mut stack_value = value;
        let size = u32::try_from(core::mem::size_of::<usize>()).unwrap_or(4);
        // SAFETY: 緩衝指向有效、原生指標寬度的堆疊值，驅動只在呼叫中複製值。
        unsafe { (self.set_value)(channel, parameter, (&raw mut stack_value).cast(), size) }
    }

    /// 取得 PCAN 狀態碼的英文診斷文字。
    #[must_use]
    pub fn error_text(&self, code: TPCANStatus) -> Box<str> {
        let mut buffer = [0_i8; 256];
        // SAFETY: 緩衝區可寫且有 256 位元組，符合 PCAN-Basic 文件規定；
        // 語言碼 0x09 指定英文，函式不保留此指標。
        let status = unsafe { (self.get_error_text)(code, 0x09, buffer.as_mut_ptr()) };
        if status != 0 {
            return format!("無法取得錯誤文字（GetErrorText={status:#010x}）").into_boxed_str();
        }
        // SAFETY: 原廠 API 成功時保證寫入 NUL 結尾字串；緩衝預先全為零，
        // 即使驅動寫滿較短內容仍必定有終止符。
        let text = unsafe { CStr::from_ptr(buffer.as_ptr()) };
        text.to_string_lossy().into_owned().into_boxed_str()
    }

    /// 判斷載入的函式庫是否完整提供 CAN FD 初始化、讀取與寫入符號。
    #[must_use]
    pub const fn supports_fd(&self) -> bool {
        self.initialize_fd.is_some() && self.read_fd.is_some() && self.write_fd.is_some()
    }
}

static GLOBAL: OnceLock<Arc<PcanApi>> = OnceLock::new();

unsafe fn required<T: Copy>(
    library: &Library,
    name: &'static [u8],
    label: &'static str,
) -> Result<T, LoadError> {
    // PCANBasic.dll 由 .def 匯出未修飾名稱，x86/x64 與 Linux 相同；只有
    // 呼叫慣例需要 extern "system"，不需要嘗試 `_CAN_Initialize@20`。
    // SAFETY: T 是與該已知 PCAN-Basic 符號完全一致的函式指標型別；
    // 回傳前只複製裸指標，且 PcanApi 會持有 Library 至所有指標失效之後。
    let symbol = unsafe { library.get::<T>(name) }
        .map_err(|_| LoadError::MissingSymbol { symbol: label })?;
    Ok(*symbol)
}

unsafe fn optional<T: Copy>(library: &Library, name: &'static [u8]) -> Option<T> {
    // SAFETY: 同 required；解析失敗代表舊版函式庫不提供此選配能力。
    unsafe { library.get::<T>(name) }.ok().map(|symbol| *symbol)
}

unsafe fn resolve(library: Library) -> Result<PcanApi, LoadError> {
    // SAFETY: 每個型別皆逐一對應原廠 PCANBasic.h 原型，Library 最終移入
    // PcanApi 最後欄位以維持所有複製函式指標的有效期。
    unsafe {
        Ok(PcanApi {
            initialize: required(&library, b"CAN_Initialize\0", "CAN_Initialize")?,
            initialize_fd: optional(&library, b"CAN_InitializeFD\0"),
            uninitialize: required(&library, b"CAN_Uninitialize\0", "CAN_Uninitialize")?,
            reset: required(&library, b"CAN_Reset\0", "CAN_Reset")?,
            get_status: required(&library, b"CAN_GetStatus\0", "CAN_GetStatus")?,
            read: required(&library, b"CAN_Read\0", "CAN_Read")?,
            read_fd: optional(&library, b"CAN_ReadFD\0"),
            write: required(&library, b"CAN_Write\0", "CAN_Write")?,
            write_fd: optional(&library, b"CAN_WriteFD\0"),
            filter_messages: required(&library, b"CAN_FilterMessages\0", "CAN_FilterMessages")?,
            get_value: required(&library, b"CAN_GetValue\0", "CAN_GetValue")?,
            set_value: required(&library, b"CAN_SetValue\0", "CAN_SetValue")?,
            get_error_text: required(&library, b"CAN_GetErrorText\0", "CAN_GetErrorText")?,
            look_up_channel: optional(&library, b"CAN_LookUpChannel\0"),
            _lib: library,
        })
    }
}

#[cfg(unix)]
unsafe fn open_library(path: &Path, _absolute: bool) -> Result<Library, libloading::Error> {
    use libloading::os::unix::{Library as UnixLibrary, RTLD_LOCAL, RTLD_NOW};

    // SAFETY: 路徑來自明確候選；載入原廠函式庫的初始化/終止程式是此
    // FFI crate 的責任。RTLD_NOW 讓未解析符號立即失敗，RTLD_LOCAL 避免污染全域。
    let library = unsafe { UnixLibrary::open(Some(path), RTLD_NOW | RTLD_LOCAL) }?;
    Ok(library.into())
}

#[cfg(windows)]
unsafe fn open_library(path: &Path, absolute: bool) -> Result<Library, libloading::Error> {
    use libloading::os::windows::Library as WindowsLibrary;

    const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;
    const LOAD_LIBRARY_SEARCH_DEFAULT_DIRS: u32 = 0x0000_1000;
    let flags = if absolute {
        LOAD_WITH_ALTERED_SEARCH_PATH
    } else {
        // 排除目前工作目錄，避免 DLL planting；仍保留系統、應用程式及
        // 使用 AddDllDirectory 註冊的標準目錄。
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
    };
    // SAFETY: 路徑與安全搜尋旗標已在上方決定，載入原廠 DLL 是本 crate
    // 的明確職責，且回傳物件負責匹配 FreeLibrary。
    let library = unsafe { WindowsLibrary::load_with_flags(path, flags) }?;
    Ok(library.into())
}

fn load_candidate(path: &Path, absolute: bool) -> Result<PcanApi, LoadError> {
    // SAFETY: open_library 只建立由 RAII 管理的 Library；resolve 會驗證每個
    // 必要符號，失敗時 Library 隨區域值安全卸載。
    let library = unsafe { open_library(path, absolute) }.map_err(|error| LoadError::NotFound {
        tried: vec![path.to_string_lossy().into_owned().into_boxed_str()],
        source: Some(std::io::Error::other(error)),
    })?;
    // SAFETY: 所有符號由 resolve 依官方原型解析並與 Library 綁定生命週期。
    unsafe { resolve(library) }
}

fn candidates() -> Vec<PathBuf> {
    let mut values = Vec::new();
    if let Some(path) = std::env::var_os("PCAN_BASIC_LIB").map(PathBuf::from)
        && path.is_absolute()
    {
        values.push(path);
    }
    #[cfg(windows)]
    {
        values.push(PathBuf::from("PCANBasic.dll"));
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            values.push(
                PathBuf::from(program_files)
                    .join("PEAK-System")
                    .join("PCAN-Basic API")
                    .join("x64")
                    .join("PCANBasic.dll"),
            );
        }
    }
    #[cfg(unix)]
    {
        values.push(PathBuf::from("libpcanbasic.so"));
        values.push(PathBuf::from("libpcanbasic.so.4"));
        values.push(PathBuf::from("/usr/lib/libpcanbasic.so"));
        values.push(PathBuf::from("/usr/local/lib/libpcanbasic.so"));
    }
    values
}

/// 依標準搜尋順序載入 PCAN-Basic 函式庫，並在行程層級快取成功結果。
///
/// 失敗不會被快取；使用者可在行程執行期間安裝驅動或修正環境變數後重試。
///
/// # Errors
///
/// 所有候選皆無法載入時回傳 [`LoadError::NotFound`]；已載入的函式庫缺少
/// 核心符號時回傳 [`LoadError::MissingSymbol`]。
pub fn load() -> Result<Arc<PcanApi>, LoadError> {
    if let Some(api) = GLOBAL.get() {
        return Ok(Arc::clone(api));
    }
    let paths = candidates();
    let mut tried = Vec::with_capacity(paths.len());
    let mut last_source = None;
    for path in paths {
        let absolute = path.is_absolute();
        tried.push(path.to_string_lossy().into_owned().into_boxed_str());
        match load_candidate(&path, absolute) {
            Ok(api) => {
                let api = Arc::new(api);
                let _already_initialized = GLOBAL.set(Arc::clone(&api));
                return Ok(GLOBAL.get().map_or(api, Arc::clone));
            }
            Err(LoadError::NotFound { source, .. }) => last_source = source,
            Err(error) => return Err(error),
        }
    }
    Err(LoadError::NotFound {
        tried,
        source: last_source,
    })
}

/// 從指定絕對路徑載入 PCAN-Basic 函式庫。
///
/// 此函式不使用全域快取，適合需要隔離特定 SDK 版本的程式。
///
/// # Errors
///
/// 路徑不是絕對路徑、檔案無法載入或必要符號缺失時回傳載入錯誤。
pub fn load_from(path: &Path) -> Result<Arc<PcanApi>, LoadError> {
    if !path.is_absolute() {
        return Err(LoadError::NotFound {
            tried: vec![path.to_string_lossy().into_owned().into_boxed_str()],
            source: Some(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PCAN-Basic 函式庫路徑必須是絕對路徑",
            )),
        });
    }
    load_candidate(path, true).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use pcan_core::LoadError;

    #[test]
    fn unavailable_driver_returns_clean_not_found() {
        match super::load() {
            Err(LoadError::NotFound { tried, .. }) => assert!(!tried.is_empty()),
            Err(other) => panic!("未安裝驅動時應回 NotFound，實際：{other:?}"),
            Ok(_) => {
                // 測試主機已安裝 PCAN-Basic，載入成功亦屬合法。
            }
        }
    }
}
