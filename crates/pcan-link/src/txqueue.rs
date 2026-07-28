use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::VecDeque;
use std::sync::Arc;

use pcan_core::{Error, FaultKind, Frame, Stats, Transport};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::events::BusEvent;
use crate::supervisor::guard::ShutdownGuard;
use crate::supervisor::{RuntimeConfig, SupervisorCommand};

/// 傳送佇列的即時水位快照。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct TxQueueDepth {
    /// 已排入但尚未被傳送工作者取走的幀數（channel 段）。
    pub channel: usize,
    /// 已取走但尚未送上匯流排的待送幀數（暫存段）。
    pub staged: usize,
    /// 單段容量，即 `LinkBuilder::tx_queue_capacity` 設定值。
    pub capacity: usize,
}

impl TxQueueDepth {
    /// 取得 channel 與暫存兩段合計的積壓幀數。
    #[must_use]
    pub const fn total(&self) -> usize {
        self.channel.saturating_add(self.staged)
    }

    /// 取得 channel 段相對於單段容量的使用比例。
    ///
    /// 這是預測 `Link::try_send` 何時回傳 `Error::TxQueueFull` 的指標，因為
    /// `try_send` 只會直接撞到 channel 段；延遲觀測則應搭配
    /// [`total`](Self::total) 查看兩段總積壓。
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn utilization(&self) -> f32 {
        if self.capacity == 0 {
            0.0
        } else {
            self.channel as f32 / self.capacity as f32
        }
    }
}

/// 斷線期間待送幀的處置策略。
///
/// 預設保留短時間內的幀並以 `max_pending_age` 限制新鮮度：短暫 USB
/// 重列舉不必回報應用錯誤，也不會在重連後補送危險的陳舊控制命令。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum PendingTxPolicy {
    /// 保留佇列，重連後依序送出，逾期者丟棄。
    #[default]
    Hold,
    /// 立即清空，斷線期間的新傳送回報錯誤。
    FailFast,
    /// 清空且不把丟棄回報給等待者。
    DropAll,
}

/// 傳送閘門狀態，由監督狀態機控制。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxGate {
    /// 可送往傳輸層。
    Open,
    /// 暫存至重連或逾期。
    Hold,
    /// 清空所有待送項目。
    FailAll,
}

#[derive(Debug)]
pub(crate) struct TxItem {
    frame: Frame,
    queued_at: Instant,
    completion: Option<oneshot::Sender<Result<(), Error>>>,
}

impl TxItem {
    pub(crate) fn fire_and_forget(frame: Frame) -> Self {
        Self {
            frame,
            queued_at: Instant::now(),
            completion: None,
        }
    }

    pub(crate) fn acknowledged(
        frame: Frame,
        completion: oneshot::Sender<Result<(), Error>>,
    ) -> Self {
        Self {
            frame,
            queued_at: Instant::now(),
            completion: Some(completion),
        }
    }
}

fn expiry_deadline(pending: &VecDeque<TxItem>, max_age: Duration) -> Option<Instant> {
    pending
        .front()
        .and_then(|item| item.queued_at.checked_add(max_age))
}

#[allow(clippy::cast_precision_loss)]
fn publish_depth(
    receiver: &mpsc::Receiver<TxItem>,
    staged: &AtomicUsize,
    pending_len: usize,
    ratio: Option<f32>,
    high_water: &AtomicBool,
    events: &broadcast::Sender<BusEvent>,
) {
    staged.store(pending_len, Ordering::Relaxed);
    let Some(ratio) = ratio else {
        return;
    };
    let capacity = receiver.max_capacity();
    let queued = receiver.len();
    let utilization = queued as f32 / capacity as f32;
    if utilization > ratio
        && high_water
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let _receivers = events.send(BusEvent::TxQueueHighWater {
            queued: u32::try_from(queued).unwrap_or(u32::MAX),
            capacity: u32::try_from(capacity).unwrap_or(u32::MAX),
        });
    } else if utilization <= (ratio - 0.15).max(0.0)
        && high_water
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let _receivers = events.send(BusEvent::TxQueueRecovered {
            queued: u32::try_from(queued).unwrap_or(u32::MAX),
            capacity: u32::try_from(capacity).unwrap_or(u32::MAX),
        });
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn run_tx<T: Transport>(
    mut receiver: mpsc::Receiver<TxItem>,
    mut transport: watch::Receiver<Option<Arc<T>>>,
    mut gate: watch::Receiver<TxGate>,
    supervisor: mpsc::Sender<SupervisorCommand>,
    events: broadcast::Sender<BusEvent>,
    stats: Arc<Stats>,
    staged: Arc<AtomicUsize>,
    high_water: Arc<AtomicBool>,
    config: RuntimeConfig,
    mut shutdown: ShutdownGuard,
) {
    let mut pending = VecDeque::with_capacity(config.tx_capacity);
    loop {
        publish_depth(
            &receiver,
            &staged,
            pending.len(),
            config.tx_high_water_ratio,
            &high_water,
            &events,
        );
        let current_gate = *gate.borrow();
        if current_gate == TxGate::Open {
            let item = pending.pop_front().or_else(|| receiver.try_recv().ok());
            staged.store(pending.len(), Ordering::Relaxed);
            if let Some(mut item) = item {
                if Instant::now().duration_since(item.queued_at) >= config.max_pending_age {
                    stats.inc_tx_dropped();
                    let _receivers = events.send(BusEvent::TxDropped { count: 1 });
                    if let Some(completion) = item.completion.take() {
                        let _ignored = completion.send(Err(Error::Timeout {
                            timeout: config.max_pending_age,
                        }));
                    }
                    continue;
                }
                let current_transport = transport.borrow().clone();
                let Some(current_transport) = current_transport else {
                    pending.push_front(item);
                    if transport.changed().await.is_err() {
                        break;
                    }
                    continue;
                };
                let mut retry = 0;
                loop {
                    match current_transport.send(&item.frame).await {
                        Ok(()) => {
                            stats.inc_tx_frames();
                            if let Some(completion) = item.completion.take() {
                                let _ignored = completion.send(Ok(()));
                            }
                            break;
                        }
                        Err(error)
                            if error.fault_kind() == FaultKind::Transient
                                && retry < config.tx_retry_limit =>
                        {
                            retry += 1;
                            tokio::time::sleep(Duration::from_micros(200)).await;
                        }
                        Err(error) => {
                            let kind = error.fault_kind();
                            if kind == FaultKind::Fatal || kind == FaultKind::Permanent {
                                pending.push_front(item);
                                let _ignored =
                                    supervisor.send(SupervisorCommand::TxFault(kind)).await;
                            } else {
                                stats.inc_tx_dropped();
                                let _receivers = events.send(BusEvent::TxDropped { count: 1 });
                                if let Some(completion) = item.completion.take() {
                                    let _ignored = completion.send(Err(error));
                                }
                            }
                            break;
                        }
                    }
                }
                continue;
            }
        } else if current_gate == TxGate::FailAll {
            let mut count = 0_u64;
            while let Some(mut item) = pending.pop_front().or_else(|| receiver.try_recv().ok()) {
                count = count.saturating_add(1);
                if let Some(completion) = item.completion.take() {
                    let result = match config.pending_policy {
                        PendingTxPolicy::DropAll => Ok(()),
                        _ => Err(Error::Disconnected { attempt: 0 }),
                    };
                    let _ignored = completion.send(result);
                }
            }
            if count > 0 {
                stats.tx_dropped.fetch_add(count, Ordering::Relaxed);
                let _receivers = events.send(BusEvent::TxDropped { count });
            }
            staged.store(pending.len(), Ordering::Relaxed);
        } else {
            while pending.len() < config.tx_capacity {
                match receiver.try_recv() {
                    Ok(item) => pending.push_back(item),
                    Err(_) => break,
                }
            }
            staged.store(pending.len(), Ordering::Relaxed);
            let now = Instant::now();
            let mut count = 0_u64;
            while pending
                .front()
                .is_some_and(|item| now.duration_since(item.queued_at) >= config.max_pending_age)
            {
                if let Some(mut item) = pending.pop_front() {
                    count = count.saturating_add(1);
                    if let Some(completion) = item.completion.take() {
                        let _ignored = completion.send(Err(Error::Timeout {
                            timeout: config.max_pending_age,
                        }));
                    }
                }
            }
            if count > 0 {
                stats.tx_dropped.fetch_add(count, Ordering::Relaxed);
                let _receivers = events.send(BusEvent::TxDropped { count });
            }
            staged.store(pending.len(), Ordering::Relaxed);
        }

        publish_depth(
            &receiver,
            &staged,
            pending.len(),
            config.tx_high_water_ratio,
            &high_water,
            &events,
        );
        let expiry = expiry_deadline(&pending, config.max_pending_age);
        let expires_at = expiry.unwrap_or_else(Instant::now);
        tokio::select! {
            item = receiver.recv(), if pending.len() < config.tx_capacity => {
                let Some(item) = item else { break };
                pending.push_back(item);
            }
            changed = gate.changed() => {
                if changed.is_err() { break; }
            }
            changed = transport.changed() => {
                if changed.is_err() { break; }
            }
            () = tokio::time::sleep_until(expires_at),
                if current_gate == TxGate::Hold && expiry.is_some() => {}
        }
    }
    staged.store(0, Ordering::Relaxed);
    shutdown.disarm();
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicBool, AtomicUsize};

    use pcan_core::{CanId, Frame};
    use tokio::sync::{broadcast, mpsc};

    use super::{TxItem, publish_depth};
    use crate::BusEvent;

    fn item(value: u8) -> TxItem {
        let id = CanId::standard(0x123).expect("ID");
        let frame = Frame::new(id, &[value]).expect("幀");
        TxItem::fire_and_forget(frame)
    }

    #[test]
    fn high_water_hysteresis_suppresses_threshold_event_storms() {
        let (sender, mut receiver) = mpsc::channel(20);
        let (events, mut event_rx) = broadcast::channel(64);
        let staged = AtomicUsize::new(0);
        let high_water = AtomicBool::new(false);
        for value in 0..17 {
            sender.try_send(item(value)).expect("填入高水位");
        }
        publish_depth(&receiver, &staged, 0, Some(0.8), &high_water, &events);

        for value in 0..100 {
            let _item = receiver.try_recv().expect("回落至門檻");
            publish_depth(&receiver, &staged, 0, Some(0.8), &high_water, &events);
            sender.try_send(item(value)).expect("再次越過門檻");
            publish_depth(&receiver, &staged, 0, Some(0.8), &high_water, &events);
        }
        for _ in 0..4 {
            let _item = receiver.try_recv().expect("跌回低水位");
        }
        publish_depth(&receiver, &staged, 0, Some(0.8), &high_water, &events);

        assert!(matches!(
            event_rx.try_recv(),
            Ok(BusEvent::TxQueueHighWater { .. })
        ));
        assert!(matches!(
            event_rx.try_recv(),
            Ok(BusEvent::TxQueueRecovered { .. })
        ));
        assert!(event_rx.try_recv().is_err());
    }
}
