use core::time::Duration;

use pcan_core::FaultKind;

use crate::events::{BusEvent, FaultCause};
use crate::supervisor::backoff::{BackoffPolicy, Jitter, SplitMixJitter};
use crate::txqueue::TxGate;

const ACTION_CAPACITY: usize = 5;

/// 連線狀態。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[non_exhaustive]
pub enum LinkState {
    /// 尚未啟動。
    Disconnected,
    /// 正在開啟傳輸層。
    Connecting,
    /// 已連線且可用。
    Connected,
    /// 退避等待中。
    Backoff {
        /// 已嘗試次數。
        attempt: u32,
    },
    /// 已關閉，不再重連。
    Closed,
}

/// 餵給狀態機的輸入事件。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LinkInput {
    /// 啟動連線。
    Start,
    /// 傳輸層開啟成功。
    OpenSucceeded,
    /// 傳輸層開啟失敗。
    OpenFailed(FaultKind),
    /// 讀寫路徑回報故障。
    TransportFault(FaultKind),
    /// 匯流排進入 Bus-Off。
    BusOff,
    /// 健康檢查逾時或失敗。
    HealthTimeout,
    /// 退避計時到期。
    BackoffElapsed,
    /// 連線已穩定足夠時間。
    StableElapsed,
    /// 使用者要求關閉。
    Close,
}

/// 狀態機要求監督任務執行的動作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LinkAction {
    /// 開啟傳輸層。
    OpenTransport,
    /// 關閉並丟棄當前傳輸層。
    CloseTransport,
    /// 啟動退避計時。
    ArmBackoff(Duration),
    /// 取消退避計時。
    CancelBackoff,
    /// 啟動健康檢查。
    ArmHealthCheck,
    /// 取消健康檢查。
    CancelHealthCheck,
    /// 廣播事件。
    Emit(BusEvent),
    /// 套用傳送閘門。
    ApplyTxPolicy(TxGate),
}

/// 一次狀態轉移的固定容量動作集。
#[derive(Clone, Copy, Debug)]
pub struct ActionSet {
    items: [LinkAction; ACTION_CAPACITY],
    len: u8,
}

impl ActionSet {
    fn new() -> Self {
        Self {
            items: [LinkAction::OpenTransport; ACTION_CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, action: LinkAction) {
        let index = usize::from(self.len);
        if index < ACTION_CAPACITY {
            self.items[index] = action;
            self.len += 1;
        }
    }

    /// 以連續切片查看所有動作。
    #[must_use]
    pub fn as_slice(&self) -> &[LinkAction] {
        &self.items[..usize::from(self.len)]
    }

    /// 依產生順序走訪動作。
    pub fn iter(&self) -> impl Iterator<Item = LinkAction> + '_ {
        self.as_slice().iter().copied()
    }

    /// 判斷是否沒有動作。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl IntoIterator for ActionSet {
    type Item = LinkAction;
    type IntoIter = core::iter::Take<core::array::IntoIter<LinkAction, ACTION_CAPACITY>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(usize::from(self.len))
    }
}

/// 純重連狀態機；不持有時鐘、不做 I/O。
#[derive(Debug)]
pub struct LinkMachine<J: Jitter = SplitMixJitter> {
    state: LinkState,
    attempt: u32,
    policy: BackoffPolicy,
    jitter: J,
    cause: FaultCause,
}

impl LinkMachine<SplitMixJitter> {
    /// 以預設抖動器建立狀態機。
    #[must_use]
    pub fn new(policy: BackoffPolicy) -> Self {
        Self::with_jitter(policy, SplitMixJitter::from_entropy())
    }
}

impl<J: Jitter> LinkMachine<J> {
    /// 以指定抖動器建立狀態機。
    #[must_use]
    pub const fn with_jitter(policy: BackoffPolicy, jitter: J) -> Self {
        Self {
            state: LinkState::Disconnected,
            attempt: 0,
            policy,
            jitter,
            cause: FaultCause::OpenFailed,
        }
    }

    /// 取得目前狀態。
    #[must_use]
    pub const fn state(&self) -> LinkState {
        self.state
    }

    /// 取得目前累計重試次數。
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    pub(crate) const fn policy(&self) -> &BackoffPolicy {
        &self.policy
    }

    fn fail_or_backoff(&mut self, cause: FaultCause) -> ActionSet {
        let next = self.attempt.saturating_add(1);
        let mut actions = ActionSet::new();
        if self
            .policy
            .max_attempts
            .is_some_and(|maximum| next > maximum)
        {
            self.state = LinkState::Closed;
            actions.push(LinkAction::ApplyTxPolicy(TxGate::FailAll));
            actions.push(LinkAction::Emit(BusEvent::Failed));
            return actions;
        }
        self.attempt = next;
        self.cause = cause;
        self.state = LinkState::Backoff { attempt: next };
        let base = self.policy.base_delay(next);
        let delay = self.jitter.perturb(base, self.policy.jitter_ratio);
        actions.push(LinkAction::ArmBackoff(delay));
        actions.push(LinkAction::Emit(BusEvent::Reconnecting {
            attempt: next,
            delay,
            cause,
        }));
        actions
    }

    fn connected_fault(&mut self, cause: FaultCause) -> ActionSet {
        let mut actions = ActionSet::new();
        actions.push(LinkAction::CancelHealthCheck);
        let next = self.attempt.saturating_add(1);
        if self
            .policy
            .max_attempts
            .is_some_and(|maximum| next > maximum)
        {
            self.state = LinkState::Closed;
            actions.push(LinkAction::ApplyTxPolicy(TxGate::FailAll));
            actions.push(LinkAction::CloseTransport);
            actions.push(LinkAction::Emit(BusEvent::Failed));
            return actions;
        }
        self.attempt = next;
        self.cause = cause;
        self.state = LinkState::Backoff { attempt: next };
        let base = self.policy.base_delay(next);
        let delay = self.jitter.perturb(base, self.policy.jitter_ratio);
        actions.push(LinkAction::ApplyTxPolicy(TxGate::Hold));
        actions.push(LinkAction::CloseTransport);
        if cause == FaultCause::BusOff {
            actions.push(LinkAction::Emit(BusEvent::BusOff));
            actions.push(LinkAction::ArmBackoff(delay));
        } else {
            actions.push(LinkAction::Emit(BusEvent::Reconnecting {
                attempt: next,
                delay,
                cause,
            }));
            actions.push(LinkAction::ArmBackoff(delay));
        }
        actions
    }

    /// 餵入事件並回傳固定容量的動作序列。
    pub fn step(&mut self, input: LinkInput) -> ActionSet {
        if self.state == LinkState::Closed {
            return ActionSet::new();
        }
        if input == LinkInput::Close {
            self.state = LinkState::Closed;
            let mut actions = ActionSet::new();
            actions.push(LinkAction::CancelBackoff);
            actions.push(LinkAction::CancelHealthCheck);
            actions.push(LinkAction::ApplyTxPolicy(TxGate::FailAll));
            actions.push(LinkAction::CloseTransport);
            actions.push(LinkAction::Emit(BusEvent::Closed));
            return actions;
        }
        match (self.state, input) {
            (LinkState::Disconnected, LinkInput::Start)
            | (LinkState::Backoff { .. }, LinkInput::BackoffElapsed) => {
                self.state = LinkState::Connecting;
                let mut actions = ActionSet::new();
                actions.push(LinkAction::Emit(BusEvent::Connecting));
                actions.push(LinkAction::OpenTransport);
                actions
            }
            (LinkState::Connecting, LinkInput::OpenSucceeded) => {
                self.state = LinkState::Connected;
                let mut actions = ActionSet::new();
                actions.push(LinkAction::ArmHealthCheck);
                actions.push(LinkAction::Emit(BusEvent::Connected {
                    attempt: self.attempt,
                }));
                actions.push(LinkAction::ApplyTxPolicy(TxGate::Open));
                actions
            }
            (LinkState::Connecting, LinkInput::OpenFailed(FaultKind::Permanent)) => {
                self.state = LinkState::Closed;
                let mut actions = ActionSet::new();
                actions.push(LinkAction::Emit(BusEvent::Failed));
                actions.push(LinkAction::ApplyTxPolicy(TxGate::FailAll));
                actions
            }
            (LinkState::Connecting, LinkInput::OpenFailed(_)) => {
                self.fail_or_backoff(FaultCause::OpenFailed)
            }
            (LinkState::Connected, LinkInput::BusOff) => self.connected_fault(FaultCause::BusOff),
            (LinkState::Connected, LinkInput::TransportFault(FaultKind::Fatal)) => {
                self.connected_fault(FaultCause::ReadFailed)
            }
            (LinkState::Connected, LinkInput::HealthTimeout) => {
                self.connected_fault(FaultCause::HealthCheckTimeout)
            }
            (LinkState::Connected, LinkInput::TransportFault(FaultKind::Permanent)) => {
                self.state = LinkState::Closed;
                let mut actions = ActionSet::new();
                actions.push(LinkAction::CancelHealthCheck);
                actions.push(LinkAction::ApplyTxPolicy(TxGate::FailAll));
                actions.push(LinkAction::CloseTransport);
                actions.push(LinkAction::Emit(BusEvent::Failed));
                actions
            }
            (LinkState::Connected, LinkInput::TransportFault(FaultKind::Recoverable)) => {
                let mut actions = ActionSet::new();
                actions.push(LinkAction::Emit(BusEvent::Warning(
                    pcan_core::BusWarnings::CAUTION,
                )));
                actions
            }
            (LinkState::Connected, LinkInput::TransportFault(FaultKind::Transient)) => {
                ActionSet::new()
            }
            (LinkState::Connected, LinkInput::StableElapsed) => {
                self.attempt = 0;
                ActionSet::new()
            }
            _ => {
                crate::trace_debug!(state = ?self.state, ?input, "忽略不可能的狀態機輸入");
                ActionSet::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use pcan_core::{BusWarnings, FaultKind};

    use super::{ActionSet, LinkAction, LinkInput, LinkMachine, LinkState};
    use crate::events::{BusEvent, FaultCause};
    use crate::supervisor::backoff::{BackoffPolicy, NoJitter};
    use crate::txqueue::TxGate;

    fn machine() -> LinkMachine<NoJitter> {
        let policy = BackoffPolicy {
            jitter_ratio: 0.0,
            ..BackoffPolicy::default()
        };
        LinkMachine::with_jitter(policy, NoJitter)
    }

    fn actions(set: ActionSet) -> Vec<LinkAction> {
        set.into_iter().collect()
    }

    fn connected(machine: &mut LinkMachine<NoJitter>) {
        let _start = machine.step(LinkInput::Start);
        let _opened = machine.step(LinkInput::OpenSucceeded);
    }

    #[test]
    fn disconnected_start_opens() {
        let mut machine = machine();
        assert_eq!(
            actions(machine.step(LinkInput::Start)),
            vec![
                LinkAction::Emit(BusEvent::Connecting),
                LinkAction::OpenTransport
            ]
        );
        assert_eq!(machine.state(), LinkState::Connecting);
    }

    #[test]
    fn connecting_success_opens_gate() {
        let mut machine = machine();
        let _ = machine.step(LinkInput::Start);
        assert_eq!(
            actions(machine.step(LinkInput::OpenSucceeded)),
            vec![
                LinkAction::ArmHealthCheck,
                LinkAction::Emit(BusEvent::Connected { attempt: 0 }),
                LinkAction::ApplyTxPolicy(TxGate::Open)
            ]
        );
        assert_eq!(machine.state(), LinkState::Connected);
    }

    #[test]
    fn permanent_open_failure_closes() {
        let mut machine = machine();
        let _ = machine.step(LinkInput::Start);
        assert_eq!(
            actions(machine.step(LinkInput::OpenFailed(FaultKind::Permanent))),
            vec![
                LinkAction::Emit(BusEvent::Failed),
                LinkAction::ApplyTxPolicy(TxGate::FailAll)
            ]
        );
        assert_eq!(machine.state(), LinkState::Closed);
    }

    #[test]
    fn retryable_open_failures_back_off() {
        for kind in [
            FaultKind::Fatal,
            FaultKind::Transient,
            FaultKind::Recoverable,
        ] {
            let mut machine = machine();
            let _ = machine.step(LinkInput::Start);
            assert_eq!(
                actions(machine.step(LinkInput::OpenFailed(kind))),
                vec![
                    LinkAction::ArmBackoff(Duration::from_millis(100)),
                    LinkAction::Emit(BusEvent::Reconnecting {
                        attempt: 1,
                        delay: Duration::from_millis(100),
                        cause: FaultCause::OpenFailed,
                    })
                ]
            );
            assert_eq!(machine.state(), LinkState::Backoff { attempt: 1 });
        }
    }

    #[test]
    fn bus_off_closes_holds_and_backs_off() {
        let mut machine = machine();
        connected(&mut machine);
        assert_eq!(
            actions(machine.step(LinkInput::BusOff)),
            vec![
                LinkAction::CancelHealthCheck,
                LinkAction::ApplyTxPolicy(TxGate::Hold),
                LinkAction::CloseTransport,
                LinkAction::Emit(BusEvent::BusOff),
                LinkAction::ArmBackoff(Duration::from_millis(100))
            ]
        );
    }

    #[test]
    fn fatal_and_health_faults_reconnect() {
        for (input, cause) in [
            (
                LinkInput::TransportFault(FaultKind::Fatal),
                FaultCause::ReadFailed,
            ),
            (LinkInput::HealthTimeout, FaultCause::HealthCheckTimeout),
        ] {
            let mut machine = machine();
            connected(&mut machine);
            assert_eq!(
                actions(machine.step(input)),
                vec![
                    LinkAction::CancelHealthCheck,
                    LinkAction::ApplyTxPolicy(TxGate::Hold),
                    LinkAction::CloseTransport,
                    LinkAction::Emit(BusEvent::Reconnecting {
                        attempt: 1,
                        delay: Duration::from_millis(100),
                        cause,
                    }),
                    LinkAction::ArmBackoff(Duration::from_millis(100))
                ]
            );
        }
    }

    #[test]
    fn permanent_transport_fault_closes() {
        let mut machine = machine();
        connected(&mut machine);
        assert_eq!(
            actions(machine.step(LinkInput::TransportFault(FaultKind::Permanent))),
            vec![
                LinkAction::CancelHealthCheck,
                LinkAction::ApplyTxPolicy(TxGate::FailAll),
                LinkAction::CloseTransport,
                LinkAction::Emit(BusEvent::Failed)
            ]
        );
    }

    #[test]
    fn recoverable_warns_and_transient_is_ignored() {
        let mut machine = machine();
        connected(&mut machine);
        assert_eq!(
            actions(machine.step(LinkInput::TransportFault(FaultKind::Recoverable))),
            vec![LinkAction::Emit(BusEvent::Warning(BusWarnings::CAUTION))]
        );
        assert!(
            machine
                .step(LinkInput::TransportFault(FaultKind::Transient))
                .is_empty()
        );
        assert_eq!(machine.state(), LinkState::Connected);
    }

    #[test]
    fn backoff_elapsed_reopens() {
        let mut machine = machine();
        let _ = machine.step(LinkInput::Start);
        let _ = machine.step(LinkInput::OpenFailed(FaultKind::Fatal));
        assert_eq!(
            actions(machine.step(LinkInput::BackoffElapsed)),
            vec![
                LinkAction::Emit(BusEvent::Connecting),
                LinkAction::OpenTransport
            ]
        );
        assert_eq!(machine.state(), LinkState::Connecting);
    }

    #[test]
    fn close_from_every_non_closed_state_has_complete_cleanup() {
        let expected = vec![
            LinkAction::CancelBackoff,
            LinkAction::CancelHealthCheck,
            LinkAction::ApplyTxPolicy(TxGate::FailAll),
            LinkAction::CloseTransport,
            LinkAction::Emit(BusEvent::Closed),
        ];
        let mut disconnected = machine();
        assert_eq!(actions(disconnected.step(LinkInput::Close)), expected);

        let mut connecting = machine();
        let _ = connecting.step(LinkInput::Start);
        assert_eq!(actions(connecting.step(LinkInput::Close)), expected);

        let mut connected_machine = machine();
        connected(&mut connected_machine);
        assert_eq!(actions(connected_machine.step(LinkInput::Close)), expected);

        let mut backoff = machine();
        let _ = backoff.step(LinkInput::Start);
        let _ = backoff.step(LinkInput::OpenFailed(FaultKind::Fatal));
        assert_eq!(actions(backoff.step(LinkInput::Close)), expected);
    }

    #[test]
    fn deterministic_backoff_caps_at_thirty_seconds() {
        let mut machine = machine();
        let expected = [
            100, 200, 400, 800, 1_600, 3_200, 6_400, 12_800, 25_600, 30_000, 30_000,
        ];
        let _ = machine.step(LinkInput::Start);
        for delay_ms in expected {
            let current = actions(machine.step(LinkInput::OpenFailed(FaultKind::Fatal)));
            assert!(matches!(
                current.first(),
                Some(LinkAction::ArmBackoff(delay))
                    if *delay == Duration::from_millis(delay_ms)
            ));
            let _ = machine.step(LinkInput::BackoffElapsed);
        }
    }

    #[test]
    fn maximum_attempts_closes_before_fourth_backoff() {
        let policy = BackoffPolicy {
            max_attempts: Some(3),
            jitter_ratio: 0.0,
            ..BackoffPolicy::default()
        };
        let mut machine = LinkMachine::with_jitter(policy, NoJitter);
        let _ = machine.step(LinkInput::Start);
        for _ in 0..3 {
            let _ = machine.step(LinkInput::OpenFailed(FaultKind::Fatal));
            let _ = machine.step(LinkInput::BackoffElapsed);
        }
        assert_eq!(
            actions(machine.step(LinkInput::OpenFailed(FaultKind::Fatal))),
            vec![
                LinkAction::ApplyTxPolicy(TxGate::FailAll),
                LinkAction::Emit(BusEvent::Failed)
            ]
        );
        assert_eq!(machine.state(), LinkState::Closed);
    }

    #[test]
    fn stable_connection_resets_attempt() {
        let mut machine = machine();
        let _ = machine.step(LinkInput::Start);
        let _ = machine.step(LinkInput::OpenFailed(FaultKind::Fatal));
        let _ = machine.step(LinkInput::BackoffElapsed);
        let _ = machine.step(LinkInput::OpenSucceeded);
        assert_eq!(machine.attempt(), 1);
        assert!(machine.step(LinkInput::StableElapsed).is_empty());
        assert_eq!(machine.attempt(), 0);
        let actions = actions(machine.step(LinkInput::TransportFault(FaultKind::Fatal)));
        assert!(matches!(
            actions.last(),
            Some(LinkAction::ArmBackoff(delay))
                if *delay == Duration::from_millis(100)
        ));
    }

    #[test]
    fn closed_absorbs_every_input_and_impossible_inputs_are_empty() {
        let mut impossible = machine();
        assert!(impossible.step(LinkInput::OpenSucceeded).is_empty());
        let _ = impossible.step(LinkInput::Close);
        for input in [
            LinkInput::Start,
            LinkInput::OpenSucceeded,
            LinkInput::OpenFailed(FaultKind::Fatal),
            LinkInput::TransportFault(FaultKind::Fatal),
            LinkInput::BusOff,
            LinkInput::HealthTimeout,
            LinkInput::BackoffElapsed,
            LinkInput::StableElapsed,
            LinkInput::Close,
        ] {
            assert!(impossible.step(input).is_empty());
            assert_eq!(impossible.state(), LinkState::Closed);
        }
    }
}
