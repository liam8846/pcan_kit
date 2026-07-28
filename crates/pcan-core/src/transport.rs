use core::future::Future;

use crate::error::Result;
use crate::filter::FilterSet;
use crate::frame::{Frame, RxFrame};
use crate::status::BusStatus;

/// 後端在執行期實際可用的能力。
///
/// crate 在編譯期永遠能表示 CAN FD 幀；個別裝置是否能使用則由本結構協商。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)]
pub struct Capabilities {
    /// 是否支援 CAN FD。
    pub can_fd: bool,
    /// 是否支援 CAN FD 位元率切換。
    pub brs: bool,
    /// 是否能接收本地送出幀的回音。
    pub echo_frames: bool,
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
