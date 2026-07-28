use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pcan_core::{Error, FilterSet, RxFrame};
pub use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{Notify, mpsc, oneshot};

/// 訂閱識別碼。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SubscriptionId(pub(crate) u64);

/// 訂閱佇列滿時的處置策略。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub enum OverflowPolicy {
    /// 丟棄最新到達的幀。
    #[default]
    DropNewest,
    /// 丟棄佇列中最舊的幀，保留最新幀。
    DropOldest,
    /// 關閉該訂閱。
    Close,
}

/// 訂閱設定。
///
/// 建立訂閱時會為該訂閱配置一個固定容量佇列；接收分派熱路徑不會配置。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SubscribeConfig {
    /// 識別碼過濾器。
    pub filter: FilterSet,
    /// 佇列容量。
    pub capacity: usize,
    /// 佇列滿時的政策。
    pub overflow: OverflowPolicy,
    /// 是否接收本地回音。
    pub include_echo: bool,
}

impl Default for SubscribeConfig {
    fn default() -> Self {
        Self {
            filter: FilterSet::accept_all(),
            capacity: 256,
            overflow: OverflowPolicy::DropNewest,
            include_echo: false,
        }
    }
}

impl SubscribeConfig {
    /// 以指定過濾器建立設定。
    #[must_use]
    pub fn new(filter: FilterSet) -> Self {
        Self {
            filter,
            ..Self::default()
        }
    }

    /// 設定固定佇列容量。
    #[must_use]
    pub const fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// 設定溢位政策。
    #[must_use]
    pub const fn with_overflow(mut self, overflow: OverflowPolicy) -> Self {
        self.overflow = overflow;
        self
    }

    /// 設定是否包含本地回音。
    #[must_use]
    pub const fn with_echo(mut self, include: bool) -> Self {
        self.include_echo = include;
        self
    }
}

#[derive(Debug)]
struct QueueState {
    frames: VecDeque<RxFrame>,
}

#[derive(Debug)]
struct Queue {
    state: Mutex<QueueState>,
    notify: Notify,
    closed: AtomicBool,
    capacity: usize,
}

fn lock_queue(queue: &Queue) -> MutexGuard<'_, QueueState> {
    match queue.state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                frames: VecDeque::with_capacity(capacity),
            }),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
            capacity,
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn recv(&self) -> Option<RxFrame> {
        loop {
            let notified = self.notify.notified();
            if let Some(frame) = lock_queue(self).frames.pop_front() {
                return Some(frame);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            notified.await;
        }
    }

    fn try_recv(&self) -> Result<RxFrame, TryRecvError> {
        if let Some(frame) = lock_queue(self).frames.pop_front() {
            Ok(frame)
        } else if self.closed.load(Ordering::Acquire) {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }
}

/// 一個推送式訂閱，丟棄時自動退訂。
///
/// 路由器會在推送之前先過濾，因此不相關訂閱不會被喚醒。相較
/// `broadcast<RxFrame>`，這避免每幀喚醒所有消費者，也能為每個訂閱選擇
/// 明確的溢位政策。
#[derive(Debug)]
pub struct Subscription {
    id: SubscriptionId,
    queue: Arc<Queue>,
    dropped: Arc<AtomicU64>,
    control: mpsc::UnboundedSender<RouterCommand>,
}

impl Subscription {
    /// 取得訂閱識別碼。
    #[must_use]
    pub const fn id(&self) -> SubscriptionId {
        self.id
    }

    /// 接收下一個符合過濾條件的幀。
    ///
    /// 此方法取消安全，可用於 `tokio::select!`。
    pub async fn recv(&mut self) -> Option<RxFrame> {
        self.queue.recv().await
    }

    /// 非阻塞嘗試接收。
    ///
    /// # Errors
    ///
    /// 佇列目前為空或訂閱已關閉時回傳對應的 `TryRecvError`。
    pub fn try_recv(&mut self) -> Result<RxFrame, TryRecvError> {
        self.queue.try_recv()
    }

    /// 取得目前累計丟棄數。
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ignored = self.control.send(RouterCommand::Unsubscribe(self.id));
    }
}

#[derive(Debug)]
pub(crate) enum RouterCommand {
    Subscribe {
        config: SubscribeConfig,
        reply: oneshot::Sender<Result<SubscriptionParts, Error>>,
    },
    Unsubscribe(SubscriptionId),
}

#[derive(Debug)]
pub(crate) struct SubscriptionParts {
    id: SubscriptionId,
    queue: Arc<Queue>,
    dropped: Arc<AtomicU64>,
}

impl SubscriptionParts {
    pub(crate) fn into_subscription(
        self,
        control: mpsc::UnboundedSender<RouterCommand>,
    ) -> Subscription {
        Subscription {
            id: self.id,
            queue: self.queue,
            dropped: self.dropped,
            control,
        }
    }
}

#[derive(Debug)]
struct Slot {
    id: SubscriptionId,
    filter: FilterSet,
    queue: Arc<Queue>,
    overflow: OverflowPolicy,
    include_echo: bool,
    dropped: Arc<AtomicU64>,
    last_reported_second: u64,
}

#[derive(Debug)]
pub(crate) struct Router {
    slots: Vec<Slot>,
    next_id: u64,
}

impl Router {
    pub(crate) const fn new() -> Self {
        Self {
            slots: Vec::new(),
            next_id: 1,
        }
    }

    pub(crate) fn handle(&mut self, command: RouterCommand) {
        match command {
            RouterCommand::Subscribe { config, reply } => {
                if config.capacity == 0 {
                    let _ignored = reply.send(Err(Error::Unsupported("訂閱容量必須大於零")));
                    return;
                }
                let id = SubscriptionId(self.next_id);
                self.next_id = self.next_id.saturating_add(1);
                let queue = Arc::new(Queue::new(config.capacity));
                let dropped = Arc::new(AtomicU64::new(0));
                self.slots.push(Slot {
                    id,
                    filter: config.filter,
                    queue: Arc::clone(&queue),
                    overflow: config.overflow,
                    include_echo: config.include_echo,
                    dropped: Arc::clone(&dropped),
                    last_reported_second: 0,
                });
                let _ignored = reply.send(Ok(SubscriptionParts { id, queue, dropped }));
            }
            RouterCommand::Unsubscribe(id) => {
                if let Some(index) = self.slots.iter().position(|slot| slot.id == id) {
                    let slot = self.slots.swap_remove(index);
                    slot.queue.close();
                }
            }
        }
    }

    pub(crate) fn dispatch(
        &mut self,
        frame: RxFrame,
        elapsed_second: u64,
        mut on_drop: impl FnMut(SubscriptionId, u64),
    ) {
        self.slots.retain_mut(|slot| {
            if slot.queue.closed.load(Ordering::Acquire) {
                return false;
            }
            if (frame.is_echo && !slot.include_echo) || !slot.filter.matches(frame.frame.id()) {
                return true;
            }
            let mut queue = lock_queue(&slot.queue);
            if queue.frames.len() < slot.queue.capacity {
                queue.frames.push_back(frame);
                drop(queue);
                slot.queue.notify.notify_one();
                return true;
            }
            match slot.overflow {
                OverflowPolicy::DropNewest => {}
                OverflowPolicy::DropOldest => {
                    let _discarded = queue.frames.pop_front();
                    queue.frames.push_back(frame);
                    drop(queue);
                    slot.queue.notify.notify_one();
                }
                OverflowPolicy::Close => {
                    drop(queue);
                    slot.queue.close();
                    return false;
                }
            }
            let count = slot.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if elapsed_second > slot.last_reported_second {
                slot.last_reported_second = elapsed_second;
                on_drop(slot.id, count);
            }
            true
        });
    }

    pub(crate) fn close_all(&mut self) {
        for slot in self.slots.drain(..) {
            slot.queue.close();
        }
    }
}

impl Drop for Router {
    fn drop(&mut self) {
        // supervisor 因 panic 展開時也必須喚醒所有正在等待的訂閱者。
        self.close_all();
    }
}
