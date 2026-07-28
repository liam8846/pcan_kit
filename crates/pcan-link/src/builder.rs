use core::time::Duration;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex, OnceLock};

use pcan_core::{BusStatus, Error, FilterSet, Stats, TransportFactory};
use tokio::sync::{broadcast, watch};

use crate::LinkState;
use crate::link::{Link, LinkInner};
use crate::supervisor::backoff::BackoffPolicy;
use crate::supervisor::{RuntimeConfig, SharedRuntime, spawn};
use crate::txqueue::{PendingTxPolicy, TxGate};

/// `Link` 的產品級執行期建構器。
#[derive(Debug)]
pub struct LinkBuilder<F> {
    factory: F,
    backoff: BackoffPolicy,
    jitter_seed: Option<u64>,
    hardware_filter: FilterSet,
    open_timeout: Duration,
    tx_queue_capacity: usize,
    tx_high_water_ratio: Option<f32>,
    pending_tx_policy: PendingTxPolicy,
    max_pending_age: Duration,
    tx_retry_limit: u32,
    health_check_interval: Option<Duration>,
    rx_silence_timeout: Option<Duration>,
    event_capacity: usize,
    max_in_flight: usize,
}

impl<F: TransportFactory> LinkBuilder<F> {
    pub(crate) fn new(factory: F) -> Self {
        Self {
            factory,
            backoff: BackoffPolicy::default(),
            jitter_seed: None,
            hardware_filter: FilterSet::accept_all(),
            open_timeout: Duration::from_secs(5),
            tx_queue_capacity: 256,
            tx_high_water_ratio: Some(0.8),
            pending_tx_policy: PendingTxPolicy::Hold,
            max_pending_age: Duration::from_secs(1),
            tx_retry_limit: 8,
            health_check_interval: Some(Duration::from_secs(1)),
            rx_silence_timeout: None,
            event_capacity: 64,
            max_in_flight: 64,
        }
    }

    /// 設定重連退避策略。
    #[must_use]
    pub const fn backoff(mut self, policy: BackoffPolicy) -> Self {
        self.backoff = policy;
        self
    }

    /// 設定可重現的抖動種子。
    #[must_use]
    pub const fn jitter_seed(mut self, seed: u64) -> Self {
        self.jitter_seed = Some(seed);
        self
    }

    /// 設定每次開啟後要重放至硬體或核心層的初始過濾器。
    #[must_use]
    pub fn hardware_filter(mut self, filter: FilterSet) -> Self {
        self.hardware_filter = filter;
        self
    }

    /// 設定單次開啟傳輸層的最長等待時間。
    ///
    /// USB 裝置重列舉或驅動異常可能讓 `open()` 長時間無法完成；有限期限可
    /// 讓監督器回到退避重試流程，而不是永久卡在 `Connecting`。預設五秒。
    #[must_use]
    pub const fn open_timeout(mut self, timeout: Duration) -> Self {
        self.open_timeout = timeout;
        self
    }

    /// 設定 bounded 傳送佇列容量。
    #[must_use]
    pub const fn tx_queue_capacity(mut self, capacity: usize) -> Self {
        self.tx_queue_capacity = capacity;
        self
    }

    /// 設定傳送佇列高水位比例，超過時廣播
    /// [`BusEvent::TxQueueHighWater`](crate::BusEvent::TxQueueHighWater)。
    ///
    /// 傳入 `None` 停用。合法範圍為 `0.0..=1.0`，超出範圍會夾到邊界；
    /// `NaN` 會回復預設值 `0.8`。預設為 `Some(0.8)`。
    #[must_use]
    pub fn tx_high_water_ratio(mut self, ratio: Option<f32>) -> Self {
        self.tx_high_water_ratio = ratio.map(|value| {
            if value.is_nan() {
                0.8
            } else {
                value.clamp(0.0, 1.0)
            }
        });
        self
    }

    /// 設定斷線期間的待送政策。
    #[must_use]
    pub const fn pending_tx_policy(mut self, policy: PendingTxPolicy) -> Self {
        self.pending_tx_policy = policy;
        self
    }

    /// 設定待送幀最大年齡。
    #[must_use]
    pub const fn max_pending_age(mut self, age: Duration) -> Self {
        self.max_pending_age = age;
        self
    }

    /// 設定後端暫時錯誤的重試上限。
    #[must_use]
    pub const fn tx_retry_limit(mut self, limit: u32) -> Self {
        self.tx_retry_limit = limit;
        self
    }

    /// 設定健康檢查週期；`None` 表示停用。
    #[must_use]
    pub const fn health_check_interval(mut self, interval: Option<Duration>) -> Self {
        self.health_check_interval = interval;
        self
    }

    /// 設定 RX 靜默逾時。
    ///
    /// 預設為 `None`，因為安靜的 CAN 匯流排完全正常，擅自啟用會造成誤重連。
    #[must_use]
    pub const fn rx_silence_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.rx_silence_timeout = timeout;
        self
    }

    /// 設定事件廣播容量。
    #[must_use]
    pub const fn event_capacity(mut self, capacity: usize) -> Self {
        self.event_capacity = capacity;
        self
    }

    /// 設定同時進行的交易上限。
    #[must_use]
    pub const fn max_in_flight_transactions(mut self, limit: usize) -> Self {
        self.max_in_flight = limit;
        self
    }

    /// 建立並立即啟動第一次連線，不等待成功。
    #[must_use]
    pub fn build(self) -> Link {
        let tx_capacity = self.tx_queue_capacity.max(1);
        let max_in_flight = self.max_in_flight.max(1);
        let event_capacity = self.event_capacity.max(1);
        let (state, _) = watch::channel(LinkState::Disconnected);
        let (gate, _) = watch::channel(TxGate::Hold);
        let (events, _) = broadcast::channel(event_capacity);
        let counters = Arc::new(Stats::default());
        let bus_status = Arc::new(Mutex::new(BusStatus::default()));
        let capabilities = Arc::new(Mutex::new(None));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let tx_staged = Arc::new(AtomicUsize::new(0));
        let tx_high_water = Arc::new(AtomicBool::new(false));
        let raw = Arc::new(OnceLock::new());
        let runtime = RuntimeConfig {
            tx_capacity,
            tx_high_water_ratio: self.tx_high_water_ratio,
            pending_policy: self.pending_tx_policy,
            max_pending_age: self.max_pending_age,
            tx_retry_limit: self.tx_retry_limit,
            open_timeout: self.open_timeout,
            health_interval: self.health_check_interval,
            rx_silence_timeout: self.rx_silence_timeout,
            max_in_flight,
        };
        let shared = SharedRuntime {
            state: state.clone(),
            gate,
            events: events.clone(),
            stats: Arc::clone(&counters),
            bus_status: Arc::clone(&bus_status),
            capabilities: Arc::clone(&capabilities),
            in_flight: Arc::clone(&in_flight),
            tx_staged: Arc::clone(&tx_staged),
            tx_high_water: Arc::clone(&tx_high_water),
            raw: Arc::clone(&raw),
        };
        let channels = spawn(
            self.factory,
            self.backoff,
            self.jitter_seed,
            self.hardware_filter,
            runtime,
            shared,
        );
        Link {
            inner: Arc::new(LinkInner {
                channels,
                state,
                events,
                stats: counters,
                bus_status,
                capabilities,
                in_flight,
                tx_staged,
                tx_high_water,
                raw,
                pending_policy: self.pending_tx_policy,
                tx_capacity,
                tx_high_water_ratio: self.tx_high_water_ratio,
                cyclic_next: AtomicU64::new(1),
            }),
        }
    }

    /// 建立並等待首次連線成功或永久關閉。
    ///
    /// # Errors
    ///
    /// 連線永久失敗時回傳 [`Error::Closed`]。
    pub async fn connect(self) -> Result<Link, Error> {
        let link = self.build();
        link.wait_connected().await?;
        Ok(link)
    }
}
