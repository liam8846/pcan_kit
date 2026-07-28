use core::cmp::Reverse;
use core::num::NonZeroU32;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::BinaryHeap;
use std::sync::Arc;

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
    /// 同一 tick 的順序，數值小者優先。
    pub priority: u8,
}

impl CyclicConfig {
    /// 建立永久重複的週期設定。
    #[must_use]
    pub const fn new(frame: Frame, period: Duration) -> Self {
        Self {
            frame,
            period,
            initial_delay: None,
            repeat: Repeat::Forever,
            overrun: OverrunPolicy::Skip,
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
}

#[derive(Debug, Default)]
pub(crate) struct SharedStats {
    sent: AtomicU64,
    skipped: AtomicU64,
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
    payload_len: Arc<AtomicUsize>,
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
        if data.len() != self.payload_len.load(Ordering::Acquire) {
            return Err(Error::Unsupported("週期幀新舊酬載長度必須相同"));
        }
        let mut payload = [0; 64];
        let len = u8::try_from(data.len())
            .ok()
            .filter(|value| usize::from(*value) <= payload.len())
            .ok_or(Error::Unsupported("週期幀酬載長度超過 64"))?;
        payload[..data.len()].copy_from_slice(data);
        self.control
            .send(CyclicCommand::SetPayload {
                id: self.id,
                payload,
                len,
            })
            .map_err(|_| Error::Closed)
    }

    /// 更新後續送出的完整幀。
    ///
    /// # Errors
    ///
    /// 排程器已關閉時回傳錯誤。
    pub fn set_frame(&self, frame: Frame) -> Result<(), Error> {
        self.control
            .send(CyclicCommand::SetFrame { id: self.id, frame })
            .map_err(|_| Error::Closed)?;
        self.payload_len
            .store(frame.data().len(), Ordering::Release);
        Ok(())
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
        payload_len: Arc<AtomicUsize>,
        stats: Arc<SharedStats>,
    },
    SetPayload {
        id: CyclicId,
        payload: [u8; 64],
        len: u8,
    },
    SetFrame {
        id: CyclicId,
        frame: Frame,
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
        payload_len: Arc<AtomicUsize>,
        stats: Arc<SharedStats>,
        control: mpsc::UnboundedSender<CyclicCommand>,
    ) -> Self {
        CyclicHandle {
            id,
            control,
            detached: false,
            payload_len,
            stats,
        }
    }
}

pub(crate) fn new_shared(frame: Frame) -> (Arc<AtomicUsize>, Arc<SharedStats>) {
    (
        Arc::new(AtomicUsize::new(frame.data().len())),
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
    payload_len: Arc<AtomicUsize>,
    stats: Arc<SharedStats>,
}

fn find_entry(entries: &mut [Entry], id: CyclicId) -> Option<&mut Entry> {
    entries.iter_mut().find(|entry| entry.id == id)
}

/// 將酬載就地套用到週期項目的幀上。
///
/// 併發的 [`CyclicHandle::set_frame`] 可能已經改變幀長度，使先前通過長度檢查的
/// 酬載更新變成陳舊指令。此時直接忽略，而不是截斷或補零送出錯誤的資料。
///
/// 回傳是否實際套用，供測試與統計判讀。
fn apply_payload(entry: &mut Entry, payload: &[u8; 64], len: u8) -> bool {
    let data = entry.config.frame.data_mut();
    if data.len() != usize::from(len) {
        return false;
    }
    data.copy_from_slice(&payload[..usize::from(len)]);
    true
}

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
        let deadline = heap.peek().map_or_else(
            || Instant::now() + Duration::from_secs(86_400),
            |item| item.0.0,
        );
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    CyclicCommand::Add { id, config, payload_len, stats } => {
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
                            payload_len: Arc::clone(&payload_len),
                            stats: Arc::clone(&stats),
                        });
                        heap.push(Reverse((next, config.priority, id, 0)));
                    }
                    CyclicCommand::SetPayload { id, payload, len } => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            let _applied = apply_payload(entry, &payload, len);
                        }
                    }
                    CyclicCommand::SetFrame { id, frame } => {
                        if let Some(entry) = find_entry(&mut entries, id) {
                            entry.config.frame = frame;
                            entry.payload_len.store(frame.data().len(), Ordering::Release);
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
                    let sends = match entry.config.overrun {
                        OverrunPolicy::Skip => 1,
                        OverrunPolicy::Burst => late_ticks.saturating_add(1),
                    };
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
    use core::sync::atomic::AtomicUsize;

    use pcan_core::{CanId, Frame};

    use super::{CyclicConfig, CyclicId, Entry, SharedStats, apply_payload};

    /// 以指定幀建立可供酬載套用測試的週期項目。
    fn entry(frame: Frame) -> Entry {
        Entry {
            id: CyclicId(1),
            config: CyclicConfig::new(frame, core::time::Duration::from_millis(10)),
            next: tokio::time::Instant::now(),
            paused: false,
            remaining: None,
            generation: 0,
            payload_len: std::sync::Arc::new(AtomicUsize::new(frame.data().len())),
            stats: std::sync::Arc::new(SharedStats::default()),
        }
    }

    /// 驗證長度相符時會完整更新幀資料。
    #[test]
    fn matching_payload_is_applied() {
        let id = CanId::standard(0x123).expect("ID");
        let mut entry = entry(Frame::new(id, &[0; 8]).expect("幀"));
        let payload = [7; 64];

        assert!(apply_payload(&mut entry, &payload, 8));
        assert_eq!(entry.config.frame.data(), &[7; 8]);
    }

    /// 驗證幀變短後會忽略陳舊的較長酬載。
    #[test]
    fn stale_payload_is_ignored_when_frame_shrinks() {
        let id = CanId::standard(0x123).expect("ID");
        let mut entry = entry(Frame::new(id, &[3; 2]).expect("幀"));
        let payload = [7; 64];

        assert!(!apply_payload(&mut entry, &payload, 8));
        assert_eq!(entry.config.frame.data(), &[3; 2]);
    }

    /// 驗證幀變長後會忽略陳舊的較短酬載。
    #[test]
    fn stale_payload_is_ignored_when_frame_grows() {
        let id = CanId::standard(0x123).expect("ID");
        let mut entry = entry(Frame::new(id, &[3; 8]).expect("幀"));
        let payload = [7; 64];

        assert!(!apply_payload(&mut entry, &payload, 2));
        assert_eq!(entry.config.frame.data(), &[3; 8]);
    }

    /// 驗證遠端幀會忽略非零長度的酬載。
    #[test]
    fn non_empty_payload_is_ignored_for_remote_frame() {
        let id = CanId::standard(0x123).expect("ID");
        let mut entry = entry(Frame::remote(id, 8).expect("遠端幀"));
        let payload = [7; 64];

        assert!(!apply_payload(&mut entry, &payload, 8));
        assert!(entry.config.frame.data().is_empty());
    }

    /// 驗證遠端幀可合法套用零長度酬載。
    #[test]
    fn empty_payload_is_applied_to_remote_frame() {
        let id = CanId::standard(0x123).expect("ID");
        let mut entry = entry(Frame::remote(id, 8).expect("遠端幀"));
        let payload = [7; 64];

        assert!(apply_payload(&mut entry, &payload, 0));
        assert!(entry.config.frame.data().is_empty());
    }
}
