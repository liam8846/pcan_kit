use core::future::Future;

use crate::error::Result;
use crate::filter::FilterSet;
use crate::frame::{Frame, RxFrame};
use crate::status::BusStatus;

/// 這個已開啟的後端**具備**哪些能力。
///
/// 本結構只描述「做不做得到」，不描述「這次有沒有開」。例如即使
/// [`TransportConfig::receive_error_frames`](crate::TransportConfig::receive_error_frames)
/// 設為 `false`，只要後端支援錯誤幀，[`error_frames`](Self::error_frames)
/// 仍為 `true`。要知道本次開啟實際啟用了什麼，請查
/// [`ActiveFeatures`]。
///
/// 能力資訊共有三層，用途各不相同：
///
/// | 層 | 型別 | 回答的問題 |
/// |---|---|---|
/// | 硬體 | `ChannelInfo`（列舉 API，開啟前） | 這個裝置本身支援什麼 |
/// | 後端 | [`Capabilities`] | 已開啟的傳輸層做得到什麼 |
/// | 本次 | [`ActiveFeatures`] | 這次開啟實際啟用了什麼 |
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    /// 後端是否具備 CAN FD 能力。
    pub can_fd: bool,
    /// 後端是否具備 CAN FD 位元率切換能力。
    pub brs: bool,
    /// 後端是否具備接收本地送出幀回音的能力。
    pub echo_frames: bool,
    /// 後端是否具備接收錯誤幀的能力。
    pub error_frames: bool,
    /// 後端是否具備接收狀態幀的能力。
    pub status_frames: bool,
    /// 後端是否具備在硬體或作業系統核心層套用過濾器的能力。
    ///
    /// 此欄位只描述後端具備此能力，不保證每個 [`FilterSet`] 都會實際下推。
    /// 個別規則集是否下推取決於後端能否精確表示；無法下推時會退回軟體
    /// 過濾，並以 debug 等級記錄。
    pub hardware_filter: bool,
    /// 是否提供硬體時間戳。
    pub hardware_timestamps: bool,
    /// 是否支援唯聽模式。
    pub listen_only: bool,
}

/// 這次開啟**實際啟用**了哪些功能。
///
/// 與 [`Capabilities`] 成對：前者回答「做不做得到」，本結構回答「這次有沒
/// 有開」。兩者可以合法地不同——以古典位元率開啟一張支援 FD 的卡片時，
/// `Capabilities::can_fd` 為 `true` 而 [`can_fd`](Self::can_fd) 為 `false`。
///
/// 把兩件事擠進同一個 `bool` 會讓跨後端的語意無法對齊，因此明確分開。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct ActiveFeatures {
    /// 本次是否以 CAN FD 模式開啟。
    pub can_fd: bool,
    /// 本次是否可使用位元率切換。
    pub brs: bool,
    /// 本次是否會收到本地送出幀的回音。
    pub echo_frames: bool,
    /// 本次是否會收到錯誤幀。
    pub error_frames: bool,
    /// 本次是否會收到狀態幀。
    pub status_frames: bool,
    /// 本次是否以唯聽模式開啟。
    pub listen_only: bool,
}

/// 傳輸層向上回報的事件。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum TransportEvent {
    /// 收到一個資料幀。
    Frame(RxFrame),
    /// 匯流排狀態發生變化。
    Status(BusStatus),
}

/// 後端傳輸抽象。
///
/// 所有方法取 `&self`，實作需自行處理內部同步，讓單一 `Arc<T>` 可同時由
/// 接收與傳送任務使用。本 trait 是唯一接觸作業系統與 FFI 的介面；重連、
/// 佇列、路由、週期傳送及交易等上層邏輯都能用 `testing::FakeTransport`
/// 在無硬體環境驅動。
///
/// 方法刻意使用 RPITIT（`-> impl Future + Send`），而非 edition 2024 的
/// `async fn`：AFIT 不會自動讓呼叫端取得 `Send` 界限，泛型監督 task 因而
/// 無法可靠地交給 `tokio::spawn`。本 trait 也不使用 `async-trait` 或提供
/// `dyn Transport`，避免每一幀呼叫都產生 `Box<dyn Future>` 的堆積配置；
/// 動態後端選擇應由上層以 enum 靜態分派。
pub trait Transport: Send + Sync + 'static {
    /// 等待並取得下一個傳輸事件。
    ///
    /// 實作必須取消安全；在 `tokio::select!` 中取消此 future 不得遺失已收到的幀。
    fn recv(&self) -> impl Future<Output = Result<TransportEvent>> + Send;

    /// 送出一個幀。
    ///
    /// 佇列滿時後端應先退避重試，超過上限才回傳 [`crate::Error::TxQueueFull`]。
    fn send(&self, frame: &Frame) -> impl Future<Output = Result<()>> + Send;

    /// 回報本次開啟實際啟用了哪些功能。
    ///
    /// 與 [`capabilities`](Self::capabilities) 的差別見 [`ActiveFeatures`]。
    fn active_features(&self) -> ActiveFeatures;

    /// 查詢當前匯流排狀態，供健康檢查使用。
    fn status(&self) -> impl Future<Output = Result<BusStatus>> + Send;

    /// 套用識別碼過濾器；監督層會在重連時重放此設定。
    fn set_filter(&self, filter: &FilterSet) -> impl Future<Output = Result<()>> + Send;

    /// 關閉底層資源。
    ///
    /// 實作必須冪等，重複呼叫不得重複釋放資源或失敗。
    fn close(&self) -> impl Future<Output = ()> + Send;

    /// 回報此後端於執行期實際可用的能力。
    fn capabilities(&self) -> Capabilities;
}

/// 可被監督層建立與重建的傳輸工廠。
///
/// 重連時監督層會再次呼叫 [`open`](Self::open)，取得已套用完整通道設定的新實例。
pub trait TransportFactory: Send + Sync + 'static {
    /// 此工廠產生的具體傳輸型別。
    type Transport: Transport;

    /// 開啟新的傳輸實例，並套用完整通道設定。
    fn open(&self) -> impl Future<Output = Result<Self::Transport>> + Send;

    /// 取得供日誌使用的簡短靜態描述，例如 `pcan:usb1@500k`。
    fn describe(&self) -> &str;
}
