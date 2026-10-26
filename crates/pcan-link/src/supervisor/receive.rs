//! 接收分派與狀態觀測；集中管理訂閱、交易及溢位計數。

use core::sync::atomic::Ordering;

use pcan_core::{BusState, BusStatus, BusWarnings, RxFrame};
use tokio::time::Instant;

use super::{SharedRuntime, lock};
use crate::events::BusEvent;
use crate::router::{Router, RouterCommand};
use crate::transaction::{TransactionCommand, TransactionTable};

pub(super) struct RxDispatch {
    router: Router,
    transactions: TransactionTable,
    started_at: Instant,
    pub(super) last_rx: Instant,
    last_warnings: BusWarnings,
}

impl RxDispatch {
    pub(super) fn new(max_in_flight: usize) -> Self {
        let now = Instant::now();
        Self {
            router: Router::new(),
            transactions: TransactionTable::new(max_in_flight),
            started_at: now,
            last_rx: now,
            last_warnings: BusWarnings::empty(),
        }
    }

    pub(super) fn frame(&mut self, shared: &SharedRuntime, frame: RxFrame) {
        self.last_rx = Instant::now();
        shared.stats.inc_rx_frames();
        if let Some(raw) = shared.raw.get() {
            let _receivers = raw.send(frame);
        }
        let elapsed = self
            .last_rx
            .duration_since(self.started_at)
            .as_secs()
            .saturating_add(1);
        self.router.dispatch(frame, elapsed, |subscription, count| {
            let _receivers = shared.events.send(BusEvent::RxDropped {
                subscription,
                count,
            });
        });
        self.transactions.dispatch(frame, |transaction| {
            let _receivers = shared
                .events
                .send(BusEvent::TransactionDropped { transaction });
        });
        self.publish_count(shared);
    }

    /// 接收事件與健康檢查共用同一狀態入口，警告位元只在上升緣計數。
    pub(super) fn status(&mut self, shared: &SharedRuntime, status: BusStatus) -> bool {
        if status.warnings.contains(BusWarnings::RX_OVERRUN)
            && !self.last_warnings.contains(BusWarnings::RX_OVERRUN)
        {
            shared.stats.inc_rx_hw_overrun();
        }
        if status.warnings.contains(BusWarnings::QUEUE_OVERRUN)
            && !self.last_warnings.contains(BusWarnings::QUEUE_OVERRUN)
        {
            shared.stats.inc_rx_queue_overrun();
        }
        self.last_warnings = status.warnings;
        *lock(&shared.bus_status) = status;
        let _receivers = shared.events.send(BusEvent::BusStateChanged(status));
        if !status.warnings.is_empty() {
            let _receivers = shared.events.send(BusEvent::Warning(status.warnings));
        }
        status.state == BusState::BusOff
    }

    pub(super) fn router(&mut self, command: RouterCommand) {
        self.router.handle(command);
    }

    pub(super) fn transaction(&mut self, shared: &SharedRuntime, command: TransactionCommand) {
        self.transactions.handle(command);
        self.publish_count(shared);
    }

    pub(super) fn disconnect(&mut self, shared: &SharedRuntime) {
        self.transactions.disconnect_all();
        self.publish_count(shared);
    }

    pub(super) fn close(&mut self, shared: &SharedRuntime) {
        self.disconnect(shared);
        self.router.close_all();
    }

    fn publish_count(&self, shared: &SharedRuntime) {
        shared
            .in_flight
            .store(self.transactions.len(), Ordering::Release);
    }
}
