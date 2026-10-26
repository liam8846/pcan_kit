//! 執行純狀態機產生的傳輸、計時與通知動作。

use pcan_core::{FaultKind, FilterSet, Transport, TransportFactory};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::time::Instant;

use super::backoff::SplitMixJitter;
use super::machine::{LinkAction, LinkInput, LinkMachine};
use super::{RuntimeConfig, SharedRuntime, lock};
use crate::events::{BusEvent, FaultCause};
use crate::txqueue::{PendingTxPolicy, TxGate};

pub(super) struct ActionContext<'a, F: TransportFactory> {
    pub(super) factory: &'a F,
    pub(super) transport: &'a mut Option<Arc<F::Transport>>,
    pub(super) transport_watch: &'a watch::Sender<Option<Arc<F::Transport>>>,
    pub(super) machine: &'a mut LinkMachine<SplitMixJitter>,
    pub(super) shared: &'a SharedRuntime,
    pub(super) config: RuntimeConfig,
    pub(super) saved_filter: &'a FilterSet,
    pub(super) backoff_deadline: &'a mut Option<Instant>,
    pub(super) health_deadline: &'a mut Option<Instant>,
    pub(super) stable_deadline: &'a mut Option<Instant>,
    pub(super) fault_cause: FaultCause,
}

pub(super) async fn apply_input<F: TransportFactory>(
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
                            *lock(&context.shared.capabilities) =
                                Some((opened.capabilities(), opened.active_features()));
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
