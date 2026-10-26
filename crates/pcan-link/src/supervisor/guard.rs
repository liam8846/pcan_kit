//! 背景工作任務的異常結束收斂守衛。

use tokio::sync::{broadcast, mpsc, watch};

use super::SharedRuntime;
use super::machine::LinkState;
use crate::cyclic::CyclicCommand;
use crate::events::BusEvent;
use crate::txqueue::TxGate;

/// 背景工作任務遺失時的收斂嚴重度。
#[derive(Clone, Copy, Debug)]
pub(crate) enum Severity {
    /// 整條連線必須立即收斂至關閉狀態。
    Fatal,
    /// 僅回報工作者遺失，其餘連線功能繼續運作。
    Degraded,
}

/// 確保背景工作任務異常結束時，連線一定收斂到終局狀態。
///
/// 正常結束前呼叫 [`disarm`](Self::disarm)。若因 panic 展開或 future 被丟棄
/// 而略過 `disarm`，`Drop` 會把狀態推到終局，讓所有等待者以
/// `Error::Closed` 返回，而不是永久等待。
#[derive(Debug)]
pub(crate) struct ShutdownGuard {
    state: watch::Sender<LinkState>,
    gate: watch::Sender<TxGate>,
    events: broadcast::Sender<BusEvent>,
    cyclic: mpsc::UnboundedSender<CyclicCommand>,
    worker: &'static str,
    severity: Severity,
    armed: bool,
}

impl ShutdownGuard {
    /// 由共享執行期狀態建立已上鎖的守衛。
    pub(crate) fn new(
        shared: &SharedRuntime,
        cyclic: mpsc::UnboundedSender<CyclicCommand>,
        worker: &'static str,
        severity: Severity,
    ) -> Self {
        Self {
            state: shared.state.clone(),
            gate: shared.gate.clone(),
            events: shared.events.clone(),
            cyclic,
            worker,
            severity,
            armed: true,
        }
    }

    /// 標記工作任務已沿正常路徑完成。
    pub(crate) const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        crate::trace_error!(worker = self.worker, "背景工作任務異常結束");
        let _receivers = self.events.send(BusEvent::WorkerLost {
            worker: self.worker,
        });
        if matches!(self.severity, Severity::Fatal) {
            let _changed = self.gate.send(TxGate::FailAll);
            let _changed = self.state.send(LinkState::Closed);
            let _ignored = self.cyclic.send(CyclicCommand::Close);
        }
    }
}
