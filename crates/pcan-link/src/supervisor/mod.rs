//! 將純狀態機、傳輸層、路由器與時鐘接合的監督任務。

/// 退避策略與抖動器。
pub mod backoff;
/// 背景工作任務的異常結束收斂守衛。
pub(crate) mod guard;
/// 純連線狀態機。
pub mod machine;

use core::future::pending;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::{Arc, Mutex, MutexGuard};

use pcan_core::{
    BusState, BusStatus, BusWarnings, Capabilities, Error, FaultKind, FilterSet, Stats, Transport,
    TransportEvent, TransportFactory,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use self::backoff::{BackoffPolicy, SplitMixJitter};
use self::guard::{Severity, ShutdownGuard};
use self::machine::{LinkAction, LinkInput, LinkMachine, LinkState};
use crate::cyclic::{CyclicCommand, run_scheduler};
use crate::events::{BusEvent, FaultCause};
use crate::router::{Router, RouterCommand};
use crate::transaction::{TransactionCommand, TransactionTable};
use crate::txqueue::{PendingTxPolicy, TxGate, TxItem, run_tx};

#[derive(Debug)]
pub(crate) enum SupervisorCommand {
    Close(oneshot::Sender<()>),
    TxFault(FaultKind),
    SetHardwareFilter {
        filter: FilterSet,
        reply: oneshot::Sender<Result<(), Error>>,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) tx_capacity: usize,
    pub(crate) tx_high_water_ratio: Option<f32>,
    pub(crate) pending_policy: PendingTxPolicy,
    pub(crate) max_pending_age: Duration,
    pub(crate) tx_retry_limit: u32,
    pub(crate) open_timeout: Duration,
    pub(crate) health_interval: Option<Duration>,
    pub(crate) rx_silence_timeout: Option<Duration>,
    pub(crate) max_in_flight: usize,
}

pub(crate) struct RuntimeChannels {
    pub(crate) supervisor: mpsc::Sender<SupervisorCommand>,
    pub(crate) router: mpsc::UnboundedSender<RouterCommand>,
    pub(crate) transaction: mpsc::UnboundedSender<TransactionCommand>,
    pub(crate) cyclic: mpsc::UnboundedSender<CyclicCommand>,
    pub(crate) tx: mpsc::Sender<TxItem>,
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    match value.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct SharedRuntime {
    pub(crate) state: watch::Sender<LinkState>,
    pub(crate) gate: watch::Sender<TxGate>,
    pub(crate) events: broadcast::Sender<BusEvent>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) bus_status: Arc<Mutex<BusStatus>>,
    pub(crate) capabilities: Arc<Mutex<Option<Capabilities>>>,
    pub(crate) in_flight: Arc<AtomicUsize>,
    pub(crate) tx_staged: Arc<AtomicUsize>,
    pub(crate) tx_high_water: Arc<AtomicBool>,
    pub(crate) raw: Arc<std::sync::OnceLock<broadcast::Sender<pcan_core::RxFrame>>>,
}

struct ActionContext<'a, F: TransportFactory> {
    factory: &'a F,
    transport: &'a mut Option<Arc<F::Transport>>,
    transport_watch: &'a watch::Sender<Option<Arc<F::Transport>>>,
    machine: &'a mut LinkMachine<SplitMixJitter>,
    shared: &'a SharedRuntime,
    config: RuntimeConfig,
    saved_filter: &'a FilterSet,
    backoff_deadline: &'a mut Option<Instant>,
    health_deadline: &'a mut Option<Instant>,
    stable_deadline: &'a mut Option<Instant>,
    fault_cause: FaultCause,
}

async fn apply_input<F: TransportFactory>(
    context: &mut ActionContext<'_, F>,
    mut input: LinkInput,
) {
    loop {
        let actions = context.machine.step(input);
        let _changed = context.shared.state.send(context.machine.state());
        let mut followup = None;
        for action in actions {
            match action {
                LinkAction::OpenTransport => {
                    match tokio::time::timeout(context.config.open_timeout, context.factory.open())
                        .await
                    {
                        Ok(Ok(opened)) => {
                            let opened = Arc::new(opened);
                            if let Err(error) = opened.set_filter(context.saved_filter).await {
                                opened.close().await;
                                followup = Some(LinkInput::OpenFailed(error.fault_kind()));
                                continue;
                            }
                            *lock(&context.shared.capabilities) = Some(opened.capabilities());
                            *context.transport = Some(Arc::clone(&opened));
                            let _changed = context.transport_watch.send(Some(opened));
                            followup = Some(LinkInput::OpenSucceeded);
                        }
                        Ok(Err(error)) => {
                            followup = Some(LinkInput::OpenFailed(error.fault_kind()));
                        }
                        Err(_) => {
                            followup = Some(LinkInput::OpenFailed(FaultKind::Fatal));
                        }
                    }
                }
                LinkAction::CloseTransport => {
                    // 先從所有 select 與 TX 觀察端移除，再關閉。舊 transport
                    // 回傳的 Error::Closed 因此不可能重新流入狀態機。
                    let closing = context.transport.take();
                    let _changed = context.transport_watch.send(None);
                    *lock(&context.shared.capabilities) = None;
                    if let Some(closing) = closing {
                        closing.close().await;
                    }
                }
                LinkAction::ArmBackoff(delay) => {
                    *context.backoff_deadline = Some(Instant::now() + delay);
                }
                LinkAction::CancelBackoff => *context.backoff_deadline = None,
                LinkAction::ArmHealthCheck => {
                    *context.health_deadline = context
                        .config
                        .health_interval
                        .map(|value| Instant::now() + value);
                    *context.stable_deadline =
                        Some(Instant::now() + context.machine.policy().reset_after_stable);
                }
                LinkAction::CancelHealthCheck => {
                    *context.health_deadline = None;
                    *context.stable_deadline = None;
                }
                LinkAction::Emit(mut event) => {
                    if let BusEvent::Reconnecting { cause, .. } = &mut event
                        && *cause == FaultCause::ReadFailed
                        && context.fault_cause == FaultCause::WriteFailed
                    {
                        *cause = FaultCause::WriteFailed;
                    }
                    if matches!(event, BusEvent::Connected { attempt } if attempt > 0) {
                        context.shared.stats.inc_reconnects();
                    }
                    let _receivers = context.shared.events.send(event);
                }
                LinkAction::ApplyTxPolicy(gate) => {
                    let actual = if gate == TxGate::Hold
                        && context.config.pending_policy != PendingTxPolicy::Hold
                    {
                        TxGate::FailAll
                    } else {
                        gate
                    };
                    let _changed = context.shared.gate.send(actual);
                }
            }
        }
        let Some(next) = followup else {
            break;
        };
        input = next;
    }
}

/// 依警告位元的上升緣累加接收溢位計數。
///
/// PCAN 的狀態查詢會反覆回報尚未清除的位元，因此只有由未設定轉為設定時
/// 才累加，避免把持續中的同一次溢位嚴重高估。
fn record_overruns(stats: &Stats, previous: BusWarnings, current: BusWarnings) {
    if current.contains(BusWarnings::RX_OVERRUN) && !previous.contains(BusWarnings::RX_OVERRUN) {
        stats.inc_rx_hw_overrun();
    }
    if current.contains(BusWarnings::QUEUE_OVERRUN)
        && !previous.contains(BusWarnings::QUEUE_OVERRUN)
    {
        stats.inc_rx_queue_overrun();
    }
}

/// 建立並啟動所有背景任務。
#[allow(clippy::too_many_lines)]
pub(crate) fn spawn<F: TransportFactory>(
    factory: F,
    policy: BackoffPolicy,
    jitter_seed: Option<u64>,
    initial_filter: FilterSet,
    config: RuntimeConfig,
    shared: SharedRuntime,
) -> RuntimeChannels {
    let (supervisor_tx, mut supervisor_rx) = mpsc::channel(64);
    let (router_tx, mut router_rx) = mpsc::unbounded_channel();
    let (transaction_tx, mut transaction_rx) = mpsc::unbounded_channel();
    let (cyclic_tx, cyclic_rx) = mpsc::unbounded_channel();
    let (tx_tx, tx_rx) = mpsc::channel(config.tx_capacity);
    let (transport_tx, transport_rx) = watch::channel(None);

    let tx_shutdown = ShutdownGuard::new(&shared, cyclic_tx.clone(), "tx", Severity::Fatal);
    tokio::spawn(run_tx(
        tx_rx,
        transport_rx,
        shared.gate.subscribe(),
        supervisor_tx.clone(),
        shared.events.clone(),
        Arc::clone(&shared.stats),
        Arc::clone(&shared.tx_staged),
        Arc::clone(&shared.tx_high_water),
        config,
        tx_shutdown,
    ));
    let scheduler_shutdown =
        ShutdownGuard::new(&shared, cyclic_tx.clone(), "cyclic", Severity::Degraded);
    tokio::spawn(run_scheduler(
        cyclic_rx,
        cyclic_tx.clone(),
        tx_tx.clone(),
        shared.events.clone(),
        shared.state.subscribe(),
        Arc::clone(&shared.stats),
        scheduler_shutdown,
    ));

    let supervisor_sender = supervisor_tx.clone();
    let supervisor_cyclic = cyclic_tx.clone();
    let supervisor_shutdown =
        ShutdownGuard::new(&shared, cyclic_tx.clone(), "supervisor", Severity::Fatal);
    tokio::spawn(async move {
        let mut shutdown = supervisor_shutdown;
        let jitter = jitter_seed.map_or_else(SplitMixJitter::from_entropy, SplitMixJitter::new);
        let mut machine = LinkMachine::with_jitter(policy, jitter);
        let mut saved_filter = initial_filter;
        let mut transport: Option<Arc<F::Transport>> = None;
        let mut router = Router::new();
        let mut transactions = TransactionTable::new(config.max_in_flight);
        let mut backoff_deadline = None;
        let mut health_deadline = None;
        let mut stable_deadline = None;
        let started_at = Instant::now();
        let mut shutdown_state = shared.state.subscribe();

        {
            let mut context = ActionContext {
                factory: &factory,
                transport: &mut transport,
                transport_watch: &transport_tx,
                machine: &mut machine,
                shared: &shared,
                config,
                saved_filter: &saved_filter,
                backoff_deadline: &mut backoff_deadline,
                health_deadline: &mut health_deadline,
                stable_deadline: &mut stable_deadline,
                fault_cause: FaultCause::OpenFailed,
            };
            apply_input(&mut context, LinkInput::Start).await;
        }
        if machine.state() == LinkState::Closed {
            router.close_all();
            transactions.disconnect_all();
            let _ignored = supervisor_cyclic.send(CyclicCommand::Close);
            shutdown.disarm();
            return;
        }
        let mut last_rx = Instant::now();
        let mut last_warnings = BusWarnings::empty();

        loop {
            let backoff_at =
                backoff_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let health_at =
                health_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let stable_at =
                stable_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let silence_at = config.rx_silence_timeout.map_or_else(
                || Instant::now() + Duration::from_secs(86_400),
                |timeout| last_rx + timeout,
            );
            let current_transport = transport.clone();
            let mut input = None;
            let mut cause = FaultCause::ReadFailed;
            tokio::select! {
                result = async {
                    match current_transport {
                        Some(ref active) if machine.state() == LinkState::Connected => active.recv().await,
                        _ => pending().await,
                    }
                } => {
                    match result {
                        Ok(TransportEvent::Frame(frame)) => {
                            last_rx = Instant::now();
                            shared.stats.inc_rx_frames();
                            if let Some(raw) = shared.raw.get() {
                                let _receivers = raw.send(frame);
                            }
                            let elapsed = last_rx
                                .duration_since(started_at)
                                .as_secs()
                                .saturating_add(1);
                            let events = shared.events.clone();
                            router.dispatch(frame, elapsed, |subscription, count| {
                                let _receivers = events.send(BusEvent::RxDropped { subscription, count });
                            });
                            transactions.dispatch(frame, |transaction| {
                                let _receivers =
                                    events.send(BusEvent::TransactionDropped { transaction });
                            });
                            shared.in_flight.store(transactions.len(), Ordering::Release);
                        }
                        Ok(TransportEvent::Status(status)) => {
                            shared.stats.inc_rx_error_frames();
                            record_overruns(&shared.stats, last_warnings, status.warnings);
                            last_warnings = status.warnings;
                            *lock(&shared.bus_status) = status;
                            let _receivers = shared.events.send(BusEvent::BusStateChanged(status));
                            if !status.warnings.is_empty() {
                                let _receivers =
                                    shared.events.send(BusEvent::Warning(status.warnings));
                            }
                            if status.state == BusState::BusOff {
                                input = Some(LinkInput::BusOff);
                                cause = FaultCause::BusOff;
                            }
                        }
                        Err(Error::BusOff) => {
                            input = Some(LinkInput::BusOff);
                            cause = FaultCause::BusOff;
                        }
                        Err(error) => input = Some(LinkInput::TransportFault(error.fault_kind())),
                        Ok(_) => {}
                    }
                }
                () = tokio::time::sleep_until(backoff_at), if backoff_deadline.is_some() => {
                    backoff_deadline = None;
                    input = Some(LinkInput::BackoffElapsed);
                    cause = FaultCause::OpenFailed;
                }
                () = tokio::time::sleep_until(health_at), if health_deadline.is_some() => {
                    health_deadline = config.health_interval.map(|interval| Instant::now() + interval);
                    if let (Some(active), Some(interval)) = (transport.as_ref(), config.health_interval) {
                        if let Ok(Ok(status)) =
                            tokio::time::timeout(interval, active.status()).await
                        {
                            record_overruns(&shared.stats, last_warnings, status.warnings);
                            last_warnings = status.warnings;
                            *lock(&shared.bus_status) = status;
                            let _receivers =
                                shared.events.send(BusEvent::BusStateChanged(status));
                            if !status.warnings.is_empty() {
                                let _receivers =
                                    shared.events.send(BusEvent::Warning(status.warnings));
                            }
                            if status.state == BusState::BusOff {
                                input = Some(LinkInput::BusOff);
                                cause = FaultCause::BusOff;
                            }
                        } else {
                            input = Some(LinkInput::HealthTimeout);
                            cause = FaultCause::HealthCheckTimeout;
                        }
                    }
                }
                () = tokio::time::sleep_until(stable_at), if stable_deadline.is_some() => {
                    stable_deadline = None;
                    input = Some(LinkInput::StableElapsed);
                }
                () = tokio::time::sleep_until(silence_at), if config.rx_silence_timeout.is_some() && machine.state() == LinkState::Connected => {
                    input = Some(LinkInput::HealthTimeout);
                    cause = FaultCause::HealthCheckTimeout;
                }
                command = supervisor_rx.recv() => {
                    match command {
                        Some(SupervisorCommand::Close(reply)) => {
                            let mut context = ActionContext {
                                factory: &factory, transport: &mut transport,
                                transport_watch: &transport_tx, machine: &mut machine,
                                shared: &shared, config, saved_filter: &saved_filter,
                                backoff_deadline: &mut backoff_deadline,
                                health_deadline: &mut health_deadline,
                                stable_deadline: &mut stable_deadline,
                                fault_cause: FaultCause::UserRequested,
                            };
                            apply_input(&mut context, LinkInput::Close).await;
                            transactions.disconnect_all();
                            router.close_all();
                            let _ignored = supervisor_cyclic.send(CyclicCommand::Close);
                            let _ignored = reply.send(());
                            break;
                        }
                        Some(SupervisorCommand::TxFault(kind)) => {
                            input = Some(LinkInput::TransportFault(kind));
                            cause = FaultCause::WriteFailed;
                        }
                        Some(SupervisorCommand::SetHardwareFilter { filter, reply }) => {
                            saved_filter = filter;
                            let result = if let Some(active) = transport.as_ref() {
                                active.set_filter(&saved_filter).await
                            } else {
                                Ok(())
                            };
                            let _ignored = reply.send(result);
                        }
                        None => {
                            input = Some(LinkInput::Close);
                            cause = FaultCause::UserRequested;
                        }
                    }
                }
                command = router_rx.recv() => {
                    if let Some(command) = command {
                        router.handle(command);
                    } else {
                        // 最後一個 Link 與所有訂閱控制端皆已釋放。
                        input = Some(LinkInput::Close);
                        cause = FaultCause::UserRequested;
                    }
                }
                command = transaction_rx.recv() => {
                    if let Some(command) = command {
                        transactions.handle(command);
                        shared.in_flight.store(transactions.len(), Ordering::Release);
                    } else {
                        // 最後一個 Link 與所有交易控制端皆已釋放。
                        input = Some(LinkInput::Close);
                        cause = FaultCause::UserRequested;
                    }
                }
                changed = shutdown_state.changed() => {
                    if changed.is_err()
                        || *shutdown_state.borrow_and_update() == LinkState::Closed
                            && machine.state() != LinkState::Closed
                    {
                        // Fatal 工作者守衛把狀態推到 Closed 後，由 supervisor
                        // 完成需要 await 的 transport 關閉流程。
                        input = Some(LinkInput::Close);
                        cause = FaultCause::UserRequested;
                    }
                }
            }

            if let Some(input) = input {
                if input == LinkInput::BusOff {
                    shared.stats.inc_bus_off_events();
                }
                let state_before_input = machine.state();
                if matches!(
                    input,
                    LinkInput::BusOff
                        | LinkInput::HealthTimeout
                        | LinkInput::TransportFault(FaultKind::Fatal | FaultKind::Permanent)
                ) {
                    transactions.disconnect_all();
                    shared.in_flight.store(0, Ordering::Release);
                }
                let mut context = ActionContext {
                    factory: &factory,
                    transport: &mut transport,
                    transport_watch: &transport_tx,
                    machine: &mut machine,
                    shared: &shared,
                    config,
                    saved_filter: &saved_filter,
                    backoff_deadline: &mut backoff_deadline,
                    health_deadline: &mut health_deadline,
                    stable_deadline: &mut stable_deadline,
                    fault_cause: cause,
                };
                apply_input(&mut context, input).await;
                if state_before_input != LinkState::Connected
                    && machine.state() == LinkState::Connected
                {
                    last_rx = Instant::now();
                }
                if machine.state() == LinkState::Closed {
                    transactions.disconnect_all();
                    router.close_all();
                    let _ignored = supervisor_cyclic.send(CyclicCommand::Close);
                    break;
                }
            }
        }
        shutdown.disarm();
    });

    RuntimeChannels {
        supervisor: supervisor_sender,
        router: router_tx,
        transaction: transaction_tx,
        cyclic: cyclic_tx,
        tx: tx_tx,
    }
}
