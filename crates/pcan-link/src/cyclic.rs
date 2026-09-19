use core::cmp::Reverse;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::collections::BinaryHeap;
use std::sync::{Arc, Mutex, MutexGuard};

use pcan_core::{Error, Frame, Stats};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::LinkState;
use crate::events::BusEvent;
use crate::supervisor::guard::ShutdownGuard;
use crate::txqueue::TxItem;

/// 週期傳送識別碼。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct CyclicId(pub(crate) u64);

/// 週期傳送重複次數。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum Repeat {
    /// 持續到明確停止。
    #[default]
    Forever,
    /// 送出指定次數。
    Count(NonZeroU32),
}

/// 排程落後時的處置。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum OverrunPolicy {
    /// 丟棄錯過的 tick 並回到絕對相位。
    #[default]
    Skip,
    /// 依序補送所有錯過的 tick。
    Burst,
}

/// 週期傳送設定。
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CyclicConfig {
    /// 要送出的幀。
    pub frame: Frame,
    /// 週期。
    pub period: Duration,
    /// 首次送出的額外延遲；未指定時等於一個週期。
    pub initial_delay: Option<Duration>,
    /// 重複次數。
    pub repeat: Repeat,
    /// 落後政策。
    pub overrun: OverrunPolicy,
    /// 單一 tick 最多補送幾次；只在 [`OverrunPolicy::Burst`] 下生效。
    ///
    /// 為排程器延遲設下確定性上界。沒有上限時，1 ms 週期的項目在 runtime
    /// 停頓 30 秒後會在單一 tick 裡跑 30000 次補送迴圈，期間 `Stop`、
    /// `Pause`、`SetPayload`、`SetPeriod`、`Trigger` 等控制命令全部無法被
    /// 處理；佇列早已滿時這些補送也只是快速失敗，毫無產出。
    ///
    /// CAN 控制幀通常越舊越沒有價值——停頓五秒後補完過去五百個 heartbeat
    /// 並不是使用者要的行為。超出上限的 tick 計入 [`CyclicStats::skipped`]。
    pub max_burst: NonZeroU32,
    /// 同一 tick 的順序，數值小者優先。
    pub priority: u8,
}

impl CyclicConfig {
    /// [`max_burst`](Self::max_burst) 的預設值。
    pub const DEFAULT_MAX_BURST: NonZeroU32 = NonZeroU32::new(32).unwrap();

    /// 建立永久重複的週期設定。
    #[must_use]
    pub const fn new(frame: Frame, period: Duration) -> Self {
        Self {
            frame,
            period,
            initial_delay: None,
            repeat: Repeat::Forever,
            overrun: OverrunPolicy::Skip,
            max_burst: Self::DEFAULT_MAX_BURST,
            priority: 128,
        }
    }

    /// 設定首次延遲。
    #[must_use]
    pub const fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = Some(delay);
        self
    }

    /// 設定重複次數。
    #[must_use]
    pub const fn with_repeat(mut self, repeat: Repeat) -> Self {
        self.repeat = repeat;
        self
    }

    /// 設定落後政策。
    #[must_use]
    pub const fn with_overrun(mut self, policy: OverrunPolicy) -> Self {
        self.overrun = policy;
        self
    }

    /// 設定單一 tick 的最大補送次數。
    #[must_use]
    pub const fn with_max_burst(mut self, max_burst: NonZeroU32) -> Self {
        self.max_burst = max_burst;
        self
    }

    /// 設定同 tick 優先順序。
    #[must_use]
    pub const fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }
}

/// 週期傳送統計。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct CyclicStats {
    /// 成功交給傳送佇列的次數。
    pub sent: u64,
    /// 因斷線、落後或佇列壓力跳過的次數。
    pub skipped: u64,
    /// 因長度不符而被拒絕的 [`CyclicHandle::set_payload`] 次數。
    ///
    /// 合併更新槽讓長度檢查與套用在同一個鎖內完成，不再有「送出命令後排程
    /// 器才發現幀已變短」的競態；此計數改在呼叫端即時累加，記錄的事件相同
    /// 但更早被偵測到。
    pub stale_payloads: u64,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 週期項目「最新期望狀態」的共享槽。
///
/// [`CyclicHandle::set_payload`] 與 [`CyclicHandle::set_frame`] 是同步函式，
/// 可以被極高速呼叫。若每次呼叫都排一個命令，控制通道就會無限成長——該
/// 通道必須是 unbounded，因為 `Drop` 也要用它送清理命令，而 `Drop` 不能
/// `.await`——排程器還得逐一套用其實早已被覆蓋的中間值。
///
/// 改為在此保存最新值、並以 `queued` 合併喚醒命令之後，一百萬次
/// `set_payload` 最多只留一個未處理命令，排程器也只會看到最後一個值。週期
/// 傳送本來就只關心「下一次要送什麼」，中間值沒有保留價值。
#[derive(Debug)]
pub(crate) struct PendingUpdate {
    frame: Mutex<Frame>,
    queued: AtomicBool,
}

impl PendingUpdate {
    fn new(frame: Frame) -> Self {
        Self {
            frame: Mutex::new(frame),
            queued: AtomicBool::new(false),
        }
    }

    /// 就地套用等長酬載；長度不符時回傳 `false` 且不留下部分更新。
    fn apply_payload(&self, data: &[u8]) -> bool {
        let mut frame = lock(&self.frame);
        if frame.data().len() != data.len() {
            return false;
        }
        frame.data_mut().copy_from_slice(data);
        true
    }

    /// 取得目前期望送出的幀。
    fn frame(&self) -> Frame {
        *lock(&self.frame)
    }
}

#[derive(Debug, Default)]
pub(crate) struct SharedStats {
    sent: AtomicU64,
    skipped: AtomicU64,
    stale_payloads: AtomicU64,
}

/// 週期傳送控制代碼。
///
/// 單一排程 task 以一個 `BinaryHeap` 與一個計時器管理所有項目，可決定同
/// tick 順序並控制抖動，也不會為一百個項目建立一百個 task。建立項目時
/// 允許配置；每次 tick 只操作已配置的 heap 與固定大小幀。
///
/// 丟棄控制代碼會停止項目，避免失控的 CAN 週期訊息干擾匯流排。要讓它在
/// `Link` 存活期間常駐，必須明確呼叫 [`detach`](Self::detach)。
#[must_use = "丟棄 CyclicHandle 會立即停止週期傳送；若要背景常駐請呼叫 detach()"]
#[derive(Debug)]
pub struct CyclicHandle {
    id: CyclicId,
    control: mpsc::UnboundedSender<CyclicCommand>,
    detached: bool,
    pending: Arc<PendingUpdate>,
    stats: Arc<SharedStats>,
}

impl CyclicHandle {
    /// 取得排程識別碼。
    #[must_use]
    pub const fn id(&self) -> CyclicId {
        self.id
    }

    /// 就地更新後續幀的酬載，長度必須與目前幀相同。
    ///
    /// # Errors
    ///
    /// 長度不同或排程器已關閉時回傳錯誤。
    pub fn set_payload(&self, data: &[u8]) -> Result<(), Error> {
        if !self.pending.apply_payload(data) {
            self.stats.stale_payloads.fetch_add(1, Ordering::Relaxed);
            return Err(Error::Unsupported("週期幀新舊酬載長度必須相同"));
        }
        self.wake()
    }

    /// 排一個合併更新命令；已有命令在排隊時不重複送。
    ///
    /// 呼叫端先寫值再設旗標，排程器先清旗標再讀值。這個配對保證不會出現
    /// 「旗標已清、但新值沒有命令護送」的組合：任何在排程器清旗標之後寫入
    /// 的值，其 `swap` 必定看到 `false`，因而會再排到一個命令。
    fn wake(&self) -> Result<(), Error> {
        if self.pending.queued.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        if self
            .control
            .send(CyclicCommand::ApplyUpdate { id: self.id })
            .is_err()
        {
            self.pending.queued.store(false, Ordering::Release);
            return Err(Error::Closed);
        }
        Ok(())
    }

    /// 更新後續送出的完整幀。
    ///
    /// # Errors
    ///
    /// 排程器已關閉時回傳錯誤。
    pub fn set_frame(&self, frame: Frame) -> Result<(), Error> {
        *lock(&self.pending.frame) = frame;
        self.wake()
    }

    /// 更新週期並由目前時間重新排定。
    ///
    /// # Errors
    ///
    /// 週期為零或排程器已關閉時回傳錯誤。
    pub fn set_period(&self, period: Duration) -> Result<(), Error> {
        if period.is_zero() {
            return Err(Error::Unsupported("週期必須大於零"));
        }
        self.control
            .send(CyclicCommand::SetPeriod {
                id: self.id,
                period,
            })
            .map_err(|_| Error::Closed)
    }

    /// 暫停週期項目。
    ///
    /// # Errors
    ///
    /// 排程器已關閉時回傳錯誤。
    pub fn pause(&self) -> Result<(), Error> {
        self.control
            .send(CyclicCommand::Pause(self.id))
            .map_err(|_| Error::Closed)
    }

    /// 恢復週期項目並從目前時間重新定相。
    ///
    /// # Errors
    ///
    /// 排程器已關閉時回傳錯誤。
    pub fn resume(&self) -> Result<(), Error> {
        self.control
            .send(CyclicCommand::Resume(self.id))
            .map_err(|_| Error::Closed)
    }

    /// 立即送出一次且不改變週期相位。
    ///
    /// # Errors
    ///
    /// 排程器已關閉時回傳錯誤。
    pub fn trigger_once(&self) -> Result<(), Error> {
        self.control
            .send(CyclicCommand::Trigger(self.id))
            .map_err(|_| Error::Closed)
    }

    /// 停止並等待排程器確認移除。
    ///
    /// # Errors
    ///
    /// 排程器已關閉或未能確認移除時回傳錯誤。
    pub async fn stop(mut self) -> Result<(), Error> {
        let (sender, receiver) = oneshot::channel();
        self.control
            .send(CyclicCommand::Stop {
                id: self.id,
                reply: Some(sender),
            })
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?;
        self.detached = true;
        Ok(())
    }

    /// 放棄控制權，讓項目在 `Link` 存活期間持續執行。
    #[must_use]
    pub fn detach(mut self) -> CyclicId {
        self.detached = true;
        self.id
    }

    /// 取得此項目的近即時統計。
    #[must_use]
    pub fn stats(&self) -> CyclicStats {
        CyclicStats {
            sent: self.stats.sent.load(Ordering::Relaxed),
            skipped: self.stats.skipped.load(Ordering::Relaxed),
            stale_payloads: self.stats.stale_payloads.load(Ordering::Relaxed),
        }
    }
}

impl Drop for CyclicHandle {
    fn drop(&mut self) {
        if !self.detached {
            let _ignored = self.control.send(CyclicCommand::Stop {
                id: self.id,
                reply: None,
            });
        }
    }
}

#[derive(Debug)]
pub(crate) enum CyclicCommand {
    Add {
        id: CyclicId,
        config: CyclicConfig,
        pending: Arc<PendingUpdate>,
        stats: Arc<SharedStats>,
    },
    /// 合併後的酬載／整幀更新；實際新值由共享槽讀取。
    ApplyUpdate {
        id: CyclicId,
    },
    SetPeriod {
        id: CyclicId,
        period: Duration,
    },
    Pause(CyclicId),
    Resume(CyclicId),
    Trigger(CyclicId),
    Stop {
        id: CyclicId,
        reply: Option<oneshot::Sender<()>>,
    },
    Close,
}

impl CyclicHandle {
    pub(crate) fn create(
        id: CyclicId,
        pending: Arc<PendingUpdate>,
        stats: Arc<SharedStats>,
        control: mpsc::UnboundedSender<CyclicCommand>,
    ) -> Self {
        CyclicHandle {
            id,
            control,
            detached: false,
            pending,
            stats,
        }
    }
}

pub(crate) fn new_shared(frame: Frame) -> (Arc<PendingUpdate>, Arc<SharedStats>) {
    (
        Arc::new(PendingUpdate::new(frame)),
        Arc::new(SharedStats::default()),
    )
}

#[derive(Debug)]
struct Entry {
    id: CyclicId,
    config: CyclicConfig,
    next: Instant,
    paused: bool,
    remaining: Option<u32>,
    generation: u64,
    pending: Arc<PendingUpdate>,
    stats: Arc<SharedStats>,
}

fn find_entry(entries: &mut [Entry], id: CyclicId) -> Option<&mut Entry> {
    entries.iter_mut().find(|entry| entry.id == id)
}

/// 將酬載更新就地套用到週期項目的幀上。
///
/// 併發的 [`CyclicHandle::set_frame`] 可能已經改變幀長度，使先前通過長度檢查的
/// 酬載更新變成陳舊指令。長度不符時會忽略更新並計入
/// [`CyclicStats::stale_payloads`]，而不是截斷或補零送出錯誤的資料；回傳值表示是否
/// 實際套用。
fn enqueue(
    entry: &Entry,
    sender: &mpsc::Sender<TxItem>,
    events: &broadcast::Sender<BusEvent>,
    state: LinkState,
    global_stats: &Stats,
) -> bool {
    if state != LinkState::Connected {
        entry.stats.skipped.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    match sender.try_send(TxItem::fire_and_forget(entry.config.frame)) {
        Ok(()) => {
            entry.stats.sent.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            entry.stats.skipped.fetch_add(1, Ordering::Relaxed);
            global_stats.inc_tx_queue_full();
            global_stats.inc_tx_dropped();
            let _receivers = events.send(BusEvent::TxDropped { count: 1 });
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            entry.stats.skipped.fetch_add(1, Ordering::Relaxed);
            global_stats.inc_tx_dropped();
            let _receivers = events.send(BusEvent::TxDropped { count: 1 });
            false
        }
    }
}

/// 陳舊 heap 節點過多時，由目前有效項目重建整個 heap。
///
/// `SetPeriod` 與 `Resume` 推入新節點卻不移除舊節點，`Stop` 也只從 entry
/// 表移除項目。舊節點要等到原定到期時間才會被 generation 檢查丟棄，週期
/// 很長時（例如 60 秒）可以累積到極大：十萬次 `set_period` 就留下十萬個
/// 節點，整整一分鐘才慢慢清掉。
///
/// 重建保留現有的 generation 設計，只把垃圾清掉：每個未暫停的項目恰好
/// 對應一個節點，暫停中的項目沒有節點（`Resume` 會重新推入）。
fn compact_heap(heap: &mut BinaryHeap<Reverse<(Instant, u8, CyclicId, u64)>>, entries: &[Entry]) {
    if heap.len() <= entries.len() * 4 + 64 {
        return;
    }
    *heap = entries
        .iter()
        .filter(|entry| !entry.paused)
        .map(|entry| {
            Reverse((
                entry.next,
                entry.config.priority,
                entry.id,
                entry.generation,
            ))
        })
        .collect();
}

/// 執行單一計時器的週期排程器。
#[allow(clippy::too_many_lines)]
pub(crate) async fn run_scheduler(
    mut commands: mpsc::UnboundedReceiver<CyclicCommand>,
    control: mpsc::UnboundedSender<CyclicCommand>,
    tx: mpsc::Sender<TxItem>,
    events: broadcast::Sender<BusEvent>,
    state: watch::Receiver<LinkState>,
    global_stats: Arc<Stats>,
    mut shutdown: ShutdownGuard,
) {
    let mut entries = Vec::<Entry>::new();
    let mut heap = BinaryHeap::<Reverse<(Instant, u8, CyclicId, u64)>>::new();
    loop {
        compact_heap(&mut heap, &entries);
        let deadline = heap.peek().map_or_else(
            || Instant::now() + Duration::from_secs(86_400),
            |item| item.0.0,
        );
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    CyclicCommand::Add { id, config, pending, stats } => {
                        if config.period.is_zero() {
                            continue;
                        }
                        let next = Instant::now() + config.initial_delay.unwrap_or(config.period);
                        entries.push(Entry {
                            id,
                            config,
                            next,
                            paused: false,
                            remaining: match config.repeat {
                                Repeat::Forever => None,
                                Repeat::Count(count) => Some(count.get()),
                            },
                            generation: 0,
                            pending: Arc::clone(&pending),
                            stats: Arc::clone(&stats),
                        });
                        heap.push(Reverse((next, config.priority, id, 0)));
                    }
                    CyclicCommand::ApplyUpdate { id } => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            // 先清旗標再讀值，與 CyclicHandle::wake 的「先寫值
                            // 再設旗標」配對，確保清旗標之後寫入的新值一定會
                            // 再排到一個命令。
                            entry.pending.queued.store(false, Ordering::Release);
                            entry.config.frame = entry.pending.frame();
                        }
                    }
                    CyclicCommand::SetPeriod { id, period } => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            entry.config.period = period;
                            entry.next = Instant::now() + period;
                            entry.generation = entry.generation.saturating_add(1);
                            heap.push(Reverse((entry.next, entry.config.priority, id, entry.generation)));
                        }
                    }
                    CyclicCommand::Pause(id) => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            entry.paused = true;
                            entry.generation = entry.generation.saturating_add(1);
                        }
                    }
                    CyclicCommand::Resume(id) => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            entry.paused = false;
                            entry.next = Instant::now() + entry.config.period;
                            entry.generation = entry.generation.saturating_add(1);
                            heap.push(Reverse((entry.next, entry.config.priority, id, entry.generation)));
                        }
                    }
                    CyclicCommand::Trigger(id) => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            let _sent =
                                enqueue(entry, &tx, &events, *state.borrow(), &global_stats);
                        }
                    }
                    CyclicCommand::Stop { id, reply } => {
                        if let Some(index) = entries.iter().position(|entry| entry.id == id) {
                            entries.swap_remove(index);
                        }
                        if let Some(reply) = reply {
                            let _ignored = reply.send(());
                        }
                    }
                    CyclicCommand::Close => break,
                }
            }
            () = tokio::time::sleep_until(deadline) => {
                let now = Instant::now();
                while heap.peek().is_some_and(|item| item.0 .0 <= now) {
                    let Some(Reverse((_, _, id, generation))) = heap.pop() else { break };
                    let Some(index) = entries.iter().position(|entry| entry.id == id) else { continue };
                    let entry = &mut entries[index];
                    if entry.paused || entry.generation != generation {
                        continue;
                    }
                    let late_ticks = u64::try_from(
                        now
                        .duration_since(entry.next)
                        .as_nanos()
                        .checked_div(entry.config.period.as_nanos())
                        .unwrap_or(0),
                    )
                    .unwrap_or(u64::MAX);
                    let wanted = match entry.config.overrun {
                        OverrunPolicy::Skip => 1,
                        OverrunPolicy::Burst => late_ticks.saturating_add(1),
                    };
                    // 補送次數上限讓單一 tick 的工作量有確定性上界，控制命令
                    // 因而不會被長時間停頓後的補送迴圈餓死。
                    let sends = wanted.min(u64::from(entry.config.max_burst.get()));
                    let over_budget = wanted - sends;
                    if over_budget > 0 {
                        entry
                            .stats
                            .skipped
                            .fetch_add(over_budget, Ordering::Relaxed);
                    }
                    if matches!(entry.config.overrun, OverrunPolicy::Skip) && late_ticks > 0 {
                        entry.stats.skipped.fetch_add(late_ticks, Ordering::Relaxed);
                    }
                    let mut completed = false;
                    for _ in 0..sends {
                        if entry.remaining == Some(0) {
                            completed = true;
                            break;
                        }
                        let sent =
                            enqueue(entry, &tx, &events, *state.borrow(), &global_stats);
                        if sent && let Some(remaining) = &mut entry.remaining {
                            *remaining = remaining.saturating_sub(1);
                            completed = *remaining == 0;
                        }
                    }
                    if completed {
                        entries.swap_remove(index);
                        continue;
                    }
                    let advance = late_ticks.saturating_add(1);
                    let periods = u32::try_from(advance).unwrap_or(u32::MAX);
                    let phase_advance = entry.config.period.saturating_mul(periods);
                    entry.next = entry.next.checked_add(phase_advance).unwrap_or_else(|| {
                        crate::trace_warn!(
                            cyclic_id = entry.id.0,
                            "週期相位時間溢位，改由目前時間重新定相"
                        );
                        Instant::now()
                            .checked_add(entry.config.period)
                            .unwrap_or_else(Instant::now)
                    });
                    heap.push(Reverse((entry.next, entry.config.priority, id, entry.generation)));
                }
            }
        }
    }
    drop(control);
    shutdown.disarm();
}

#[cfg(test)]
mod tests {
    use pcan_core::{CanId, Frame};

    use super::PendingUpdate;

    /// 驗證長度相符時會完整更新幀資料。
    #[test]
    fn matching_payload_is_applied() {
        let id = CanId::standard(0x123).expect("ID");
        let pending = PendingUpdate::new(Frame::new(id, &[0; 8]).expect("幀"));

        assert!(pending.apply_payload(&[7; 8]));
        assert_eq!(pending.frame().data(), &[7; 8]);
    }

    /// 驗證長度不符的酬載會被拒絕，且不留下部分更新。
    #[test]
    fn mismatched_payload_is_rejected_without_partial_update() {
        let id = CanId::standard(0x123).expect("ID");
        let pending = PendingUpdate::new(Frame::new(id, &[3; 2]).expect("幀"));

        assert!(!pending.apply_payload(&[7; 8]));
        assert_eq!(pending.frame().data(), &[3; 2]);
    }
}
