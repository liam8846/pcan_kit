//! 控制項目的生命週期准入；清理命令處理完才歸還名額。

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 命令、控制代碼與背景項目共享同一名額，避免提前釋放後堆積清理命令。
pub(crate) type ControlLease = Arc<OwnedSemaphorePermit>;

#[derive(Debug)]
pub(crate) struct ControlBudget {
    slots: Arc<Semaphore>,
    capacity: usize,
}

impl ControlBudget {
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.clamp(1, Semaphore::MAX_PERMITS);
        Self {
            slots: Arc::new(Semaphore::new(capacity)),
            capacity,
        }
    }

    pub(crate) fn acquire(&self) -> Result<ControlLease, usize> {
        Arc::clone(&self.slots)
            .try_acquire_owned()
            .map(Arc::new)
            .map_err(|_| self.capacity)
    }
}
