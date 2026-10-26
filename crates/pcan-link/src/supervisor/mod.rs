//! 將純狀態機、傳輸層、路由器與時鐘接合的監督任務。

mod actions;
/// 退避策略與抖動器。
pub(crate) mod backoff;
/// 背景工作任務的異常結束收斂守衛。
pub(crate) mod guard;
/// 純連線狀態機。
pub(crate) mod machine;
mod receive;

use core::future::pending;
use core::sync::atomic::{AtomicBool, AtomicUsize};
use core::time::Duration;
use std::sync::{Arc, Mutex, MutexGuard};

use pcan_core::{
    ActiveFeatures, BusStatus, Capabilities, Error, FaultKind, FilterSet, Stats, Transport,
    TransportEvent, TransportFactory,
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;

use self::actions::{ActionContext, apply_input};
use self::backoff::{BackoffPolicy, SplitMixJitter};
use self::guard::{Severity, ShutdownGuard};
use self::machine::{LinkInput, LinkMachine, LinkState};
use self::receive::RxDispatch;
use crate::cyclic::{CyclicCommand, run_scheduler};
use crate::events::{BusEvent, FaultCause};
use crate::router::RouterCommand;
use crate::transaction::TransactionCommand;
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
    /// 已開啟傳輸層的後端能力與本次啟用功能。
    ///
    /// 兩者放在同一個鎖裡一起換，讀者不會看到「能力已更新、啟用狀態還是
    /// 舊的」這種撕裂組合。
    pub(crate) capabilities: Arc<Mutex<Option<(Capabilities, ActiveFeatures)>>>,
    pub(crate) in_flight: Arc<AtomicUsize>,
    pub(crate) tx_staged: Arc<AtomicUsize>,
    pub(crate) tx_high_water: Arc<AtomicBool>,
    pub(crate) raw: Arc<std::sync::OnceLock<broadcast::Sender<pcan_core::RxFrame>>>,
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
        let mut receive = RxDispatch::new(config.max_in_flight);
        let mut backoff_deadline = None;
        let mut health_deadline = None;
        let mut stable_deadline = None;
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
            receive.close(&shared);
            let _ignored = supervisor_cyclic.send(CyclicCommand::Close);
            shutdown.disarm();
            return;
        }
        // 初次開啟可能耗時；RX 靜默期限應從連線建立完成後起算。
        receive.last_rx = Instant::now();

        loop {
            let backoff_at =
                backoff_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let health_at =
                health_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let stable_at =
                stable_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
            let silence_at = config.rx_silence_timeout.map_or_else(
                || Instant::now() + Duration::from_secs(86_400),
                |timeout| receive.last_rx + timeout,
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
                            receive.frame(&shared, frame);
                        }
                        Ok(TransportEvent::Status(status)) => {
                            shared.stats.inc_rx_error_frames();
                            if receive.status(&shared, status) {
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
                            if receive.status(&shared, status) {
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
                            receive.close(&shared);
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
                        receive.router(command);
                    } else {
                        // 最後一個 Link 與所有訂閱控制端皆已釋放。
                        input = Some(LinkInput::Close);
                        cause = FaultCause::UserRequested;
                    }
                }
                command = transaction_rx.recv() => {
                    if let Some(command) = command {
                        receive.transaction(&shared, command);
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
                    receive.disconnect(&shared);
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
                    receive.last_rx = Instant::now();
                }
                if machine.state() == LinkState::Closed {
                    receive.close(&shared);
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
