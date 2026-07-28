use core::sync::atomic::Ordering;
use core::time::Duration;
use std::collections::VecDeque;
use std::sync::Arc;

use pcan_core::{Error, FaultKind, Frame, Stats, Transport};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::events::BusEvent;
use crate::supervisor::{RuntimeConfig, SupervisorCommand};

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

#[allow(clippy::too_many_lines)]
pub(crate) async fn run_tx<T: Transport>(
    mut receiver: mpsc::Receiver<TxItem>,
    mut transport: watch::Receiver<Option<Arc<T>>>,
    mut gate: watch::Receiver<TxGate>,
    supervisor: mpsc::Sender<SupervisorCommand>,
    events: broadcast::Sender<BusEvent>,
    stats: Arc<Stats>,
    config: RuntimeConfig,
) {
    let mut pending = VecDeque::with_capacity(config.tx_capacity);
    loop {
        let current_gate = *gate.borrow();
        if current_gate == TxGate::Open {
            let item = pending.pop_front().or_else(|| receiver.try_recv().ok());
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
                                stats.inc_tx_queue_full();
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
        } else {
            while pending.len() < config.tx_capacity {
                match receiver.try_recv() {
                    Ok(item) => pending.push_back(item),
                    Err(_) => break,
                }
            }
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
        }

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
}
