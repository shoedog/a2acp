//! ApiBackend — the non-process OpenAI-compatible AgentBackend.
use crate::config::{ApiConfig, ApiResourceFlightRouteV3};
use crate::provider::{classify_http_error, MAX_ERROR_BODY_BYTES};
use crate::wire::{ChatRequest, Message, SseAccumulator, ToolCall};
use bridge_core::attempt_activity::{
    ActivityReason, AttemptPhase, AttemptRecorder, NoopAttemptRecorder,
};
use bridge_core::catalog::is_blocked_model_id;
use bridge_core::diagnostics::{
    diagnostic_timestamp_ms, DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor,
    FailureDiagnostic, FailureDiagnosticInput, FailureDisposition, PersistedPhaseTransition,
    PersistedPhaseTransitionInput, PhaseStatus,
};
use bridge_core::domain::{
    Part, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{BoundMcpDeliveryPayloadV1, BoundSessionSpecV1};
use bridge_core::ids::SessionId;
use bridge_core::orch::OrchEventKind;
use bridge_core::ports::{
    AgentBackend, BackendCleanupDispositionV1, BackendObservers, BackendResourceFlightV1,
    BackendStream, DiagnosticObserver, PolicyEngine, RichEventSink, Update, STOP_REASON_CANCELLED,
};
use bridge_core::provider::ProviderEvidence;
use bridge_core::remote_request_flight::{OwnedRemoteRequestV1, RemoteRequestFlightRefusalV1};
use bridge_core::resource_flight::{
    DedicatedRemoteRequestIdV1, ResourceActionDispositionV1, ResourceActionResultV1,
};
use bridge_core::retained_resource_flight::ResourceFlightOwnerV1;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::watch;

#[derive(Clone)]
struct ApiLifecycle {
    observer: Arc<dyn DiagnosticObserver>,
    redactor: DiagnosticRedactor,
}

impl ApiLifecycle {
    fn new(observer: Arc<dyn DiagnosticObserver>, api_key: Option<&str>) -> Self {
        Self {
            observer,
            redactor: DiagnosticRedactor::new(api_key),
        }
    }

    async fn record(&self, phase: DiagnosticPhase, status: PhaseStatus) -> Result<(), BridgeError> {
        let transition = PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            None,
            &self.redactor,
        )
        .map_err(|_| BridgeError::InvalidStateTransition)?;
        let event = bridge_core::diagnostics::DiagnosticEvent::new(transition, None)
            .map_err(|_| BridgeError::InvalidStateTransition)?;
        self.observer.record(event).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn failure(
        &self,
        failed_phase: DiagnosticPhase,
        last_completed_phase: Option<DiagnosticPhase>,
        class: DiagnosticFailureClass,
        code: &'static str,
        summary: &'static str,
        causes: Vec<String>,
        retry_after_ms: Option<u64>,
        reset_at_ms: Option<i64>,
        prompt_may_have_been_accepted: bool,
    ) -> (BridgeError, bool) {
        let failure = match FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase,
                last_completed_phase,
                class,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: summary.to_owned(),
                causes,
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms,
                reset_at_ms,
                prompt_may_have_been_accepted,
            },
            code,
            &self.redactor,
        ) {
            Ok(failure) => failure,
            Err(_) => return (BridgeError::InvalidStateTransition, false),
        };
        let transition = match PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase: failed_phase,
                status: PhaseStatus::Failed,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            Some(code),
            &self.redactor,
        ) {
            Ok(transition) => transition,
            Err(_) => return (BridgeError::InvalidStateTransition, false),
        };
        let event =
            match bridge_core::diagnostics::DiagnosticEvent::new(transition, Some(failure.clone()))
            {
                Ok(event) => event,
                Err(_) => return (BridgeError::InvalidStateTransition, false),
            };
        match self.observer.record(event).await {
            Ok(()) => (BridgeError::agent_failure(failure), true),
            Err(error) => (error, false),
        }
    }

    async fn request_flight_failure_recorded(
        &self,
        error: ApiRequestFlightErrorV1,
        prompt_may_have_been_accepted: bool,
    ) -> (BridgeError, bool) {
        let (failed_phase, last_completed_phase) = if prompt_may_have_been_accepted {
            (
                DiagnosticPhase::PromptStream,
                Some(DiagnosticPhase::PromptStart),
            )
        } else {
            (DiagnosticPhase::PromptStart, None)
        };
        self.failure(
            failed_phase,
            last_completed_phase,
            DiagnosticFailureClass::Persistence,
            "api.prompt.request_flight",
            "Durable remote request custody failed",
            vec![error.to_string()],
            None,
            None,
            prompt_may_have_been_accepted,
        )
        .await
    }

    async fn request_flight_failure(
        &self,
        error: ApiRequestFlightErrorV1,
        prompt_may_have_been_accepted: bool,
    ) -> BridgeError {
        self.request_flight_failure_recorded(error, prompt_may_have_been_accepted)
            .await
            .0
    }
}

struct BoundedErrorBody {
    bytes: Vec<u8>,
    oversized: bool,
}

async fn read_bounded_error_body(
    response: reqwest::Response,
) -> Result<BoundedErrorBody, reqwest::Error> {
    let mut chunks = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk?;
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            bytes.extend_from_slice(&chunk[..remaining]);
            return Ok(BoundedErrorBody {
                bytes,
                oversized: true,
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(BoundedErrorBody {
        bytes,
        oversized: false,
    })
}

fn request_failure(
    error: &reqwest::Error,
    transport_code: &'static str,
) -> (DiagnosticFailureClass, &'static str, &'static str) {
    if error.is_timeout() {
        (
            DiagnosticFailureClass::Timeout,
            "api.prompt.timeout",
            "Upstream API request timed out",
        )
    } else {
        (
            DiagnosticFailureClass::Transport,
            transport_code,
            "Upstream API transport failed",
        )
    }
}

async fn complete_prompt_lifecycle(lifecycle: &ApiLifecycle) -> Result<(), BridgeError> {
    lifecycle
        .record(DiagnosticPhase::PromptStream, PhaseStatus::Completed)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptFinish, PhaseStatus::Started)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptFinish, PhaseStatus::Completed)
        .await
}

/// Install the first request future before publishing the post-barrier phase
/// transitions. The returned future has not been polled yet.
async fn install_first_send<F>(
    lifecycle: &ApiLifecycle,
    install: impl FnOnce() -> F,
) -> Result<F, BridgeError> {
    let send = install();
    lifecycle
        .record(DiagnosticPhase::PromptStart, PhaseStatus::Completed)
        .await?;
    lifecycle
        .record(DiagnosticPhase::PromptStream, PhaseStatus::Started)
        .await?;
    Ok(send)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ApiRequestFlightErrorV1 {
    Admission(String),
    Driver(RemoteRequestFlightRefusalV1),
}

impl std::fmt::Display for ApiRequestFlightErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(detail) => {
                write!(
                    formatter,
                    "remote request flight admission refused: {detail}"
                )
            }
            Self::Driver(error) => error.fmt(formatter),
        }
    }
}

impl From<RemoteRequestFlightRefusalV1> for ApiRequestFlightErrorV1 {
    fn from(error: RemoteRequestFlightRefusalV1) -> Self {
        Self::Driver(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveRequestIdentity {
    Legacy(u64),
    Dedicated(DedicatedRemoteRequestIdV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApiRequestCleanupStateV1 {
    AdmissionPendingLegacy,
    AdmissionPendingV3,
    ActiveLegacy,
    ActiveV3,
    DropOwned,
    Terminal,
    SettlementRefused,
    TimedOut,
}

struct ApiRequestCleanupInnerV1 {
    state: ApiRequestCleanupStateV1,
    v3: bool,
    admission_started: bool,
    identity: Option<ActiveRequestIdentity>,
    accepted: bool,
    overlapped_cleanup: bool,
    terminal: Option<(ResourceActionDispositionV1, bool)>,
    diagnostic: Option<(ApiLifecycle, ApiRequestFlightErrorV1, bool)>,
    retained_late_flight: Option<OwnedRemoteRequestV1>,
}

/// Turn-keyed custody retained independently of the removable session slot.
struct ApiRequestCleanupCustodianV1 {
    turn_authority: u64,
    session: SessionId,
    deadline: tokio::time::Instant,
    inner: StdMutex<ApiRequestCleanupInnerV1>,
    changed: watch::Sender<u64>,
    live_waiters: AtomicUsize,
    /// Test-only ordering gate between the pre-settlement snapshot and the
    /// durable settlement, so deadline-crossing schedules are deterministic.
    #[cfg(test)]
    settle_drop_gate: StdMutex<Option<Box<dyn FnOnce() + Send + Sync>>>,
}

impl ApiRequestCleanupCustodianV1 {
    fn new(
        turn_authority: u64,
        session: SessionId,
        v3: bool,
        timeout: std::time::Duration,
    ) -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            turn_authority,
            session,
            deadline: tokio::time::Instant::now() + timeout,
            inner: StdMutex::new(ApiRequestCleanupInnerV1 {
                state: if v3 {
                    ApiRequestCleanupStateV1::AdmissionPendingV3
                } else {
                    ApiRequestCleanupStateV1::AdmissionPendingLegacy
                },
                v3,
                admission_started: false,
                identity: None,
                accepted: false,
                overlapped_cleanup: false,
                terminal: None,
                diagnostic: None,
                retained_late_flight: None,
            }),
            changed,
            live_waiters: AtomicUsize::new(0),
            #[cfg(test)]
            settle_drop_gate: StdMutex::new(None),
        })
    }

    fn notify(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn begin_admission(&self) -> Result<(), ApiRequestFlightErrorV1> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ApiRequestFlightErrorV1::Admission("cleanup cell poisoned".into()))?;
        if inner.state == ApiRequestCleanupStateV1::Terminal
            && inner
                .terminal
                .as_ref()
                .is_some_and(|(result, _)| *result == ResourceActionDispositionV1::Complete)
        {
            inner.state = if inner.v3 {
                ApiRequestCleanupStateV1::AdmissionPendingV3
            } else {
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
            };
            inner.admission_started = false;
            inner.identity = None;
            inner.accepted = false;
            inner.overlapped_cleanup = false;
            inner.terminal = None;
        }
        if !matches!(
            inner.state,
            ApiRequestCleanupStateV1::AdmissionPendingLegacy
                | ApiRequestCleanupStateV1::AdmissionPendingV3
        ) {
            return Err(ApiRequestFlightErrorV1::Admission(
                "cleanup cell is not admitting".into(),
            ));
        }
        inner.admission_started = true;
        Ok(())
    }

    fn bind(&self, identity: &ActiveRequestIdentity) -> Result<(), ApiRequestFlightErrorV1> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ApiRequestFlightErrorV1::Admission("cleanup cell poisoned".into()))?;
        if !inner.admission_started
            || !matches!(
                inner.state,
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
                    | ApiRequestCleanupStateV1::AdmissionPendingV3
            )
        {
            return Err(ApiRequestFlightErrorV1::Admission(
                "cleanup cell bind refused".into(),
            ));
        }
        inner.identity = Some(identity.clone());
        inner.state = if inner.v3 {
            ApiRequestCleanupStateV1::ActiveV3
        } else {
            ApiRequestCleanupStateV1::ActiveLegacy
        };
        drop(inner);
        self.notify();
        Ok(())
    }

    fn mark_accepted(&self, identity: &ActiveRequestIdentity) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.identity.as_ref() == Some(identity) {
                inner.accepted = true;
            }
        }
    }

    fn finish_pending(&self, result: ResourceActionDispositionV1, acknowledged: bool) {
        if let Ok(mut inner) = self.inner.lock() {
            if matches!(
                inner.state,
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
                    | ApiRequestCleanupStateV1::AdmissionPendingV3
                    | ApiRequestCleanupStateV1::DropOwned
            ) {
                inner.state = ApiRequestCleanupStateV1::Terminal;
                inner.terminal = Some((result, acknowledged));
                drop(inner);
                self.notify();
            }
        }
    }

    fn finish(
        &self,
        identity: &ActiveRequestIdentity,
        result: ResourceActionDispositionV1,
        acknowledged: bool,
    ) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.identity.as_ref() != Some(identity) {
            return false;
        }
        if inner.state != ApiRequestCleanupStateV1::TimedOut {
            inner.state = ApiRequestCleanupStateV1::Terminal;
        }
        inner.terminal = Some((result, acknowledged));
        drop(inner);
        self.notify();
        true
    }

    fn refuse(
        &self,
        identity: Option<&ActiveRequestIdentity>,
        error: ApiRequestFlightErrorV1,
        lifecycle: Option<ApiLifecycle>,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            if identity.is_some() && inner.identity.as_ref() != identity {
                return;
            }
            let accepted = inner.accepted;
            inner.state = ApiRequestCleanupStateV1::SettlementRefused;
            inner.diagnostic = lifecycle.map(|lifecycle| (lifecycle, error, accepted));
            drop(inner);
            self.notify();
        }
    }

    fn begin_cleanup(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if matches!(
                inner.state,
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
                    | ApiRequestCleanupStateV1::AdmissionPendingV3
            ) && !inner.admission_started
            {
                inner.state = ApiRequestCleanupStateV1::Terminal;
                inner.terminal = Some((ResourceActionDispositionV1::Complete, true));
            } else if matches!(
                inner.state,
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
                    | ApiRequestCleanupStateV1::AdmissionPendingV3
                    | ApiRequestCleanupStateV1::ActiveLegacy
                    | ApiRequestCleanupStateV1::ActiveV3
            ) {
                inner.overlapped_cleanup = true;
                inner.state = ApiRequestCleanupStateV1::DropOwned;
            }
            drop(inner);
            self.notify();
        }
    }

    fn settle_drop(
        &self,
        identity: &ActiveRequestIdentity,
        flight: Option<OwnedRemoteRequestV1>,
        disposition: ResourceActionDispositionV1,
        lifecycle: Option<ApiLifecycle>,
        accepted: bool,
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let timed_out = inner.state == ApiRequestCleanupStateV1::TimedOut;
        if inner.identity.as_ref() != Some(identity)
            || matches!(
                inner.state,
                ApiRequestCleanupStateV1::Terminal | ApiRequestCleanupStateV1::SettlementRefused
            )
        {
            return;
        }
        if !timed_out {
            inner.state = ApiRequestCleanupStateV1::DropOwned;
        }
        inner.accepted |= accepted;
        inner.overlapped_cleanup = true;
        drop(inner);
        self.notify();
        #[cfg(test)]
        if let Some(gate) = self
            .settle_drop_gate
            .lock()
            .ok()
            .and_then(|mut gate| gate.take())
        {
            gate();
        }
        let settle = || match &flight {
            Some(flight) => flight
                .settle(ResourceActionResultV1 {
                    disposition: disposition.clone(),
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                })
                .map(|outcome| (outcome.result().clone(), true))
                .map_err(ApiRequestFlightErrorV1::from),
            None => Ok((
                ResourceActionResultV1 {
                    disposition: disposition.clone(),
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                },
                false,
            )),
        };
        let result = settle().or_else(|error| {
            if !timed_out && tokio::time::Instant::now() < self.deadline {
                settle()
            } else {
                Err(error)
            }
        });
        // Branch on the CURRENT state under one lock acquisition: observation
        // may have expired while the settlement above was in flight, and a
        // stale pre-settlement snapshot must never route around the absorbing
        // TimedOut — a timed-out cleanup records evidence but never upgrades.
        if let Ok(mut inner) = self.inner.lock() {
            if inner.identity.as_ref() == Some(identity) {
                if inner.state == ApiRequestCleanupStateV1::TimedOut {
                    match result {
                        Ok((result, acknowledged)) => {
                            inner.terminal = Some((result.disposition, acknowledged));
                        }
                        Err(error) => {
                            inner.diagnostic =
                                lifecycle.map(|lifecycle| (lifecycle, error, inner.accepted));
                            inner.retained_late_flight = flight;
                        }
                    }
                } else {
                    match result {
                        Ok((result, acknowledged)) => {
                            inner.state = ApiRequestCleanupStateV1::Terminal;
                            inner.terminal = Some((result.disposition, acknowledged));
                        }
                        Err(error) => {
                            inner.state = ApiRequestCleanupStateV1::SettlementRefused;
                            inner.diagnostic =
                                lifecycle.map(|lifecycle| (lifecycle, error, inner.accepted));
                        }
                    }
                }
            }
        }
        self.notify();
    }

    fn projection(&self) -> Option<BackendCleanupDispositionV1> {
        let inner = self.inner.lock().ok()?;
        match inner.state {
            ApiRequestCleanupStateV1::SettlementRefused | ApiRequestCleanupStateV1::TimedOut => {
                Some(BackendCleanupDispositionV1::Unknown)
            }
            ApiRequestCleanupStateV1::Terminal => {
                let (result, acknowledged) = inner.terminal.as_ref()?;
                Some(
                    if *result == ResourceActionDispositionV1::Complete
                        && (!inner.v3 || *acknowledged)
                        && (inner.v3 || !inner.overlapped_cleanup)
                    {
                        BackendCleanupDispositionV1::Complete
                    } else {
                        BackendCleanupDispositionV1::Unknown
                    },
                )
            }
            _ => None,
        }
    }

    fn reclaimable(&self) -> bool {
        self.projection() == Some(BackendCleanupDispositionV1::Complete)
    }

    async fn observe(&self) -> BackendCleanupDispositionV1 {
        struct Waiter<'a>(&'a AtomicUsize);
        impl Drop for Waiter<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.live_waiters.fetch_add(1, Ordering::AcqRel);
        let _waiter = Waiter(&self.live_waiters);
        let mut changed = self.changed.subscribe();
        loop {
            if let Some(result) = self.projection() {
                let diagnostic = self
                    .inner
                    .lock()
                    .ok()
                    .and_then(|inner| inner.diagnostic.clone());
                if let Some((lifecycle, error, accepted)) = diagnostic {
                    if tokio::time::Instant::now() < self.deadline {
                        let recording = tokio::time::timeout_at(
                            self.deadline,
                            lifecycle.request_flight_failure_recorded(error.clone(), accepted),
                        )
                        .await;
                        if matches!(recording, Ok((_, true))) {
                            if let Ok(mut inner) = self.inner.lock() {
                                let is_same = inner.diagnostic.as_ref().is_some_and(
                                    |(pending_lifecycle, pending_error, pending_accepted)| {
                                        Arc::ptr_eq(
                                            &pending_lifecycle.observer,
                                            &lifecycle.observer,
                                        ) && pending_error == &error
                                            && *pending_accepted == accepted
                                    },
                                );
                                if is_same {
                                    inner.diagnostic = None;
                                }
                            }
                        }
                    }
                }
                return result;
            }
            if tokio::time::timeout_at(self.deadline, changed.changed())
                .await
                .is_err()
            {
                if let Ok(mut inner) = self.inner.lock() {
                    if !matches!(
                        inner.state,
                        ApiRequestCleanupStateV1::Terminal
                            | ApiRequestCleanupStateV1::SettlementRefused
                    ) {
                        inner.state = ApiRequestCleanupStateV1::TimedOut;
                    }
                }
                return BackendCleanupDispositionV1::Unknown;
            }
        }
    }
}

struct ActiveRequestSlot {
    turn_epoch: u64,
    identity: ActiveRequestIdentity,
    cancel_control: watch::Sender<bool>,
}

/// Per-session model plus two deliberately separate cancellation scopes.
/// `cancelled_turn_epoch` closes the gap between tool rounds. The active slot
/// owns one request-local sender, guarded by the exact request identity.
struct SessionState {
    model: SessionModelState,
    current_turn_epoch: Option<u64>,
    cancelled_turn_epoch: Option<u64>,
    next_legacy_request: u64,
    active_request: Option<ActiveRequestSlot>,
    cleanup_cell: Option<Arc<ApiRequestCleanupCustodianV1>>,
    request_flight_owner_attached: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SessionModelState {
    Unconfigured,
    ExplicitNone,
    ExplicitSome(String),
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            model: SessionModelState::Unconfigured,
            current_turn_epoch: None,
            cancelled_turn_epoch: None,
            next_legacy_request: 0,
            active_request: None,
            cleanup_cell: None,
            request_flight_owner_attached: false,
        }
    }
}

struct TurnScope {
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    session: SessionId,
    epoch: u64,
}

impl Drop for TurnScope {
    fn drop(&mut self) {
        let Ok(mut sessions) = self.sessions.lock() else {
            return;
        };
        let Some(state) = sessions.get_mut(&self.session) else {
            return;
        };
        if state.current_turn_epoch == Some(self.epoch) {
            state.current_turn_epoch = None;
            state.cancelled_turn_epoch = None;
        }
    }
}

#[derive(Clone)]
struct RequestCancelCapability {
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    session: SessionId,
    turn_epoch: u64,
    identity: ActiveRequestIdentity,
}

impl RequestCancelCapability {
    /// The identity comparison is the load-bearing stale-round fence.
    fn cancel_exact(&self) -> bool {
        let Ok(sessions) = self.sessions.lock() else {
            return false;
        };
        let Some(active) = sessions
            .get(&self.session)
            .and_then(|state| state.active_request.as_ref())
        else {
            return false;
        };
        if active.turn_epoch != self.turn_epoch || active.identity != self.identity {
            return false;
        }
        let already_cancelled = *active.cancel_control.borrow();
        if already_cancelled {
            return false;
        }
        let _ = active.cancel_control.send(true);
        true
    }

    fn clear_exact(&self) -> bool {
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        let Some(state) = sessions.get_mut(&self.session) else {
            return false;
        };
        let matches = state.active_request.as_ref().is_some_and(|active| {
            active.turn_epoch == self.turn_epoch && active.identity == self.identity
        });
        if matches {
            state.active_request = None;
        }
        matches
    }
}

struct RequestScope {
    cancel: RequestCancelCapability,
    cancel_control: watch::Sender<bool>,
    cleanup: Arc<ApiRequestCleanupCustodianV1>,
    identity: ActiveRequestIdentity,
    flight: Option<OwnedRemoteRequestV1>,
    lifecycle: Option<ApiLifecycle>,
    accepted: Arc<std::sync::atomic::AtomicBool>,
    settled: bool,
}
struct RequestAcceptanceMarker {
    cleanup: Arc<ApiRequestCleanupCustodianV1>,
    identity: ActiveRequestIdentity,
    request_accepted: Arc<std::sync::atomic::AtomicBool>,
    turn_accepted: Arc<std::sync::atomic::AtomicBool>,
}

impl RequestAcceptanceMarker {
    fn mark(&self) {
        self.turn_accepted.store(true, Ordering::Release);
        self.request_accepted.store(true, Ordering::Release);
        self.cleanup.mark_accepted(&self.identity);
    }
}

#[cfg(test)]
struct RequestSendPollBarrierForTest {
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

#[cfg(test)]
impl RequestSendPollBarrierForTest {
    fn install() -> (Arc<Self>, RequestSendPollBarrierGuardForTest) {
        let barrier = Arc::new(Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        });
        REQUEST_SEND_POLL_BARRIER_FOR_TEST.with(|slot| {
            assert!(slot.borrow_mut().replace(Arc::clone(&barrier)).is_none());
        });
        (barrier, RequestSendPollBarrierGuardForTest)
    }

    async fn wait_until_entered(&self) {
        self.entered.notified().await;
    }

    fn release(&self) {
        self.release.add_permits(1);
    }
}

#[cfg(test)]
struct RequestSendPollBarrierGuardForTest;

#[cfg(test)]
impl Drop for RequestSendPollBarrierGuardForTest {
    fn drop(&mut self) {
        REQUEST_SEND_POLL_BARRIER_FOR_TEST.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

#[cfg(test)]
std::thread_local! {
    static REQUEST_SEND_POLL_BARRIER_FOR_TEST:
        std::cell::RefCell<Option<Arc<RequestSendPollBarrierForTest>>> =
            const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
async fn wait_for_request_send_poll_for_test() {
    let barrier = REQUEST_SEND_POLL_BARRIER_FOR_TEST.with(|slot| slot.borrow().clone());
    if let Some(barrier) = barrier {
        barrier.entered.notify_one();
        barrier
            .release
            .acquire()
            .await
            .expect("test poll barrier remains live")
            .forget();
    }
}

async fn drive_provider_send<F>(
    request: Option<&OwnedRemoteRequestV1>,
    send: F,
    accepted: RequestAcceptanceMarker,
) -> Result<F::Output, ApiRequestFlightErrorV1>
where
    F: std::future::Future,
{
    match request {
        Some(request) => request
            .arm_provider_send(async move {
                #[cfg(test)]
                wait_for_request_send_poll_for_test().await;
                accepted.mark();
                send.await
            })
            .await
            .map_err(ApiRequestFlightErrorV1::from),
        None => {
            accepted.mark();
            Ok(send.await)
        }
    }
}
impl RequestScope {
    fn attach_lifecycle(&mut self, lifecycle: ApiLifecycle, accepted: bool) {
        self.lifecycle = Some(lifecycle);
        if accepted {
            // The turn-wide acceptance barrier is diagnostic custody on the
            // cleanup cell only. The request-local bit is set solely by the
            // first-poll acceptance marker: a successor round's request must
            // never inherit acceptance before its own send is polled.
            self.cleanup.mark_accepted(&self.identity);
        }
    }

    fn acceptance_marker(
        &self,
        turn_accepted: Arc<std::sync::atomic::AtomicBool>,
    ) -> RequestAcceptanceMarker {
        RequestAcceptanceMarker {
            cleanup: Arc::clone(&self.cleanup),
            identity: self.identity.clone(),
            request_accepted: Arc::clone(&self.accepted),
            turn_accepted,
        }
    }

    fn acceptance_keyed_disposition(
        &self,
        accepted_disposition: ResourceActionDispositionV1,
    ) -> ResourceActionDispositionV1 {
        if self.accepted.load(Ordering::Acquire) {
            accepted_disposition
        } else {
            ResourceActionDispositionV1::Failed
        }
    }

    fn begin_dispatch(&mut self) -> Result<(), ApiRequestFlightErrorV1> {
        if let Some(flight) = &self.flight {
            flight.journal_intent()?;
            flight.authorize_dispatch()?;
        }
        Ok(())
    }

    fn settle(
        mut self,
        disposition: ResourceActionDispositionV1,
    ) -> Result<ResourceActionResultV1, ApiRequestFlightErrorV1> {
        let disposition = self.acceptance_keyed_disposition(disposition);
        let (result, acknowledged) = match self.flight.take() {
            Some(flight) => {
                let outcome = flight.settle(ResourceActionResultV1 {
                    disposition,
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                })?;
                (outcome.result().clone(), true)
            }
            None => (
                ResourceActionResultV1 {
                    disposition,
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                },
                false,
            ),
        };
        if !self
            .cleanup
            .finish(&self.identity, result.disposition.clone(), acknowledged)
        {
            return Err(ApiRequestFlightErrorV1::Admission(
                "cleanup cell rejected exact terminal publication".into(),
            ));
        }
        self.cancel.clear_exact();
        self.settled = true;
        Ok(result)
    }
}

impl Drop for RequestScope {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let accepted_disposition = if *self.cancel_control.borrow() {
            ResourceActionDispositionV1::Partial
        } else {
            ResourceActionDispositionV1::Unknown
        };
        let disposition = self.acceptance_keyed_disposition(accepted_disposition);
        // Transfer every piece of local cleanup authority before attempting a
        // fallible settlement. The custodian, not this scope, owns the result.
        self.cleanup.settle_drop(
            &self.identity,
            self.flight.take(),
            disposition,
            self.lifecycle.take(),
            self.accepted.load(Ordering::Acquire),
        );
        self.cancel.clear_exact();
        self.settled = true;
    }
}

fn settle_request_scope(
    scope: &mut Option<RequestScope>,
    disposition: ResourceActionDispositionV1,
) -> Result<ResourceActionResultV1, ApiRequestFlightErrorV1> {
    scope
        .take()
        .ok_or_else(|| {
            ApiRequestFlightErrorV1::Admission("request scope was already settled".into())
        })?
        .settle(disposition)
}

async fn settle_request_scope_or_fail(
    lifecycle: &ApiLifecycle,
    scope: &mut Option<RequestScope>,
    disposition: ResourceActionDispositionV1,
    prompt_may_have_been_accepted: bool,
) -> Result<ResourceActionResultV1, BridgeError> {
    match settle_request_scope(scope, disposition) {
        Ok(result) => Ok(result),
        Err(error) => Err(lifecycle
            .request_flight_failure(error, prompt_may_have_been_accepted)
            .await),
    }
}

#[allow(clippy::too_many_arguments)]
async fn provider_failure_after_settlement(
    lifecycle: &ApiLifecycle,
    scope: &mut Option<RequestScope>,
    class: DiagnosticFailureClass,
    code: &'static str,
    summary: &'static str,
    cause: Option<String>,
    retry_after_ms: Option<u64>,
    reset_at_ms: Option<i64>,
) -> BridgeError {
    let mut causes: Vec<String> = cause.into_iter().collect();
    if let Err(error) = settle_request_scope(scope, ResourceActionDispositionV1::Failed) {
        causes.push(format!("durable request settlement refused: {error}"));
    }
    lifecycle
        .failure(
            DiagnosticPhase::PromptStream,
            Some(DiagnosticPhase::PromptStart),
            class,
            code,
            summary,
            causes,
            retry_after_ms,
            reset_at_ms,
            true,
        )
        .await
        .0
}

enum PreparedRequest {
    Ready {
        scope: Box<RequestScope>,
        cancel_rx: watch::Receiver<bool>,
    },
    TurnCancelled,
}

#[derive(Clone)]
struct RequestAdmission {
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    route: Option<ApiResourceFlightRouteV3>,
}

impl RequestAdmission {
    fn prepare(
        &self,
        session: &SessionId,
        turn_epoch: u64,
    ) -> Result<PreparedRequest, ApiRequestFlightErrorV1> {
        // First reject a cancellation already linearized in the between-round
        // gap. No request identity or flight is minted in that case. Admission
        // is checked again after durable work and before publication to close
        // the race in the opposite direction.
        let cleanup = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| ApiRequestFlightErrorV1::Admission("session state poisoned".into()))?;
            let Some(state) = sessions.get(session) else {
                return Ok(PreparedRequest::TurnCancelled);
            };
            if state.current_turn_epoch != Some(turn_epoch)
                || state.cancelled_turn_epoch == Some(turn_epoch)
            {
                return Ok(PreparedRequest::TurnCancelled);
            }
            if state.active_request.is_some() {
                return Err(ApiRequestFlightErrorV1::Admission(
                    "another request is active".into(),
                ));
            }
            let cleanup = state
                .cleanup_cell
                .as_ref()
                .filter(|cell| cell.turn_authority == turn_epoch)
                .cloned()
                .ok_or_else(|| {
                    ApiRequestFlightErrorV1::Admission(
                        "turn cleanup authority is unavailable".into(),
                    )
                })?;
            cleanup.begin_admission()?;
            cleanup
        };

        let flight: Option<OwnedRemoteRequestV1> = (|| {
            Ok::<_, ApiRequestFlightErrorV1>(match &self.route {
                Some(route) => {
                    let owner = ResourceFlightOwnerV1::new(route.node_id.clone(), session.as_str())
                        .map_err(|error| ApiRequestFlightErrorV1::Admission(error.to_string()))?;
                    Some(route.attempt.admit(owner)?)
                }
                None => None,
            })
        })()
        .inspect_err(|error| cleanup.refuse(None, error.clone(), None))?;

        let active_conflict = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| ApiRequestFlightErrorV1::Admission("session state poisoned".into()))?;
            match sessions.get_mut(session) {
                None => false,
                Some(state)
                    if state.current_turn_epoch != Some(turn_epoch)
                        || state.cancelled_turn_epoch == Some(turn_epoch) =>
                {
                    false
                }
                Some(state) if state.active_request.is_some() => true,
                Some(state) => {
                    let identity = match &flight {
                        Some(flight) => {
                            ActiveRequestIdentity::Dedicated(flight.request_id().clone())
                        }
                        None => {
                            state.next_legacy_request =
                                state.next_legacy_request.checked_add(1).ok_or_else(|| {
                                    ApiRequestFlightErrorV1::Admission(
                                        "legacy request authority exhausted".into(),
                                    )
                                })?;
                            ActiveRequestIdentity::Legacy(state.next_legacy_request)
                        }
                    };
                    cleanup.bind(&identity)?;
                    let (cancel_control, cancel_rx) = watch::channel(false);
                    state.active_request = Some(ActiveRequestSlot {
                        turn_epoch,
                        identity: identity.clone(),
                        cancel_control: cancel_control.clone(),
                    });
                    let cancel = RequestCancelCapability {
                        sessions: Arc::clone(&self.sessions),
                        session: session.clone(),
                        turn_epoch,
                        identity: identity.clone(),
                    };
                    return Ok(PreparedRequest::Ready {
                        scope: Box::new(RequestScope {
                            cancel,
                            cancel_control,
                            cleanup: Arc::clone(&cleanup),
                            identity,
                            flight,
                            lifecycle: None,
                            accepted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                            settled: false,
                        }),
                        cancel_rx,
                    });
                }
            }
        };

        // Publication lost its race with cancellation, forget, or another
        // request. Settle outside the session lock so node aggregation may
        // re-enter unrelated bridge state without deadlocking this session.
        let result = match &flight {
            Some(flight) => flight
                .settle(ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Failed,
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                })
                .map(|outcome| (outcome.result().clone(), true))
                .map_err(ApiRequestFlightErrorV1::from),
            None => Ok((
                ResourceActionResultV1 {
                    disposition: ResourceActionDispositionV1::Complete,
                    duration_ms: 0,
                    recovery_owner: None,
                    cause: None,
                },
                false,
            )),
        };
        match result {
            Ok((result, acknowledged)) => cleanup.finish_pending(result.disposition, acknowledged),
            Err(error) => cleanup.refuse(None, error, None),
        }
        if active_conflict {
            Err(ApiRequestFlightErrorV1::Admission(
                "another request won publication".into(),
            ))
        } else {
            Ok(PreparedRequest::TurnCancelled)
        }
    }
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>,
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    cleanup_cells: Arc<StdMutex<HashMap<u64, Arc<ApiRequestCleanupCustodianV1>>>>,
    next_turn_authority: AtomicU64,
}

/// Default policy: approve everything (mirrors AcpBackend's default auto-approver).
struct AutoApprove;
impl PolicyEngine for AutoApprove {
    fn decide(
        &self,
        _: &PermissionRequest,
        _: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

impl ApiBackend {
    pub fn new(cfg: ApiConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.request_timeout)
            .build()
            .expect("reqwest client builds");
        Self {
            cfg,
            client,
            policy: Arc::new(StdMutex::new(Arc::new(AutoApprove) as Arc<dyn PolicyEngine>)),
            sessions: Arc::new(StdMutex::new(HashMap::new())),
            cleanup_cells: Arc::new(StdMutex::new(HashMap::new())),
            next_turn_authority: AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() {
            *p = policy;
        }
        self
    }

    /// Test/inspection helper: the stashed effective model for a session.
    pub fn session_model(&self, s: &SessionId) -> Option<String> {
        match self.sessions.lock().ok()?.get(s)?.model.clone() {
            SessionModelState::ExplicitSome(model) => Some(model),
            SessionModelState::Unconfigured | SessionModelState::ExplicitNone => None,
        }
    }

    fn begin_turn(&self, session: &SessionId) -> Result<TurnScope, BridgeError> {
        let mut map = self
            .sessions
            .lock()
            .map_err(|_| BridgeError::ResourceFlightUnsupported)?;
        let state = map.entry(session.clone()).or_default();
        if self.cfg.resource_flight_route_v3.is_some() && !state.request_flight_owner_attached {
            return Err(BridgeError::ResourceFlightUnsupported);
        }
        if state.current_turn_epoch.is_some() || state.active_request.is_some() {
            return Err(BridgeError::InvalidStateTransition);
        }
        let prior = self
            .next_turn_authority
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| BridgeError::InvalidStateTransition)?;
        let epoch = prior
            .checked_add(1)
            .ok_or(BridgeError::InvalidStateTransition)?;
        let cleanup_cell = ApiRequestCleanupCustodianV1::new(
            epoch,
            session.clone(),
            self.cfg.resource_flight_route_v3.is_some(),
            self.cfg.request_timeout,
        );
        let mut cleanup_cells = self
            .cleanup_cells
            .lock()
            .map_err(|_| BridgeError::InvalidStateTransition)?;
        cleanup_cells.retain(|_, cell| cell.session != *session || !cell.reclaimable());
        cleanup_cells.insert(epoch, Arc::clone(&cleanup_cell));
        state.cleanup_cell = Some(cleanup_cell);
        state.current_turn_epoch = Some(epoch);
        state.cancelled_turn_epoch = None;
        Ok(TurnScope {
            sessions: Arc::clone(&self.sessions),
            session: session.clone(),
            epoch,
        })
    }

    fn request_admission(&self) -> RequestAdmission {
        RequestAdmission {
            sessions: Arc::clone(&self.sessions),
            route: self.cfg.resource_flight_route_v3.clone(),
        }
    }

    fn resolve_api_key(&self) -> Option<String> {
        self.cfg
            .api_key_env
            .as_ref()
            .and_then(|var| std::env::var(var).ok())
    }
    fn resolve_model(&self, s: &SessionId) -> Option<String> {
        match self
            .sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(s).map(|state| state.model.clone()))
        {
            Some(SessionModelState::ExplicitNone) => None,
            Some(SessionModelState::ExplicitSome(model)) => Some(model),
            Some(SessionModelState::Unconfigured) | None => self.cfg.model.clone(),
        }
    }

    fn reject_blocked_model(model: Option<&str>) -> Result<(), BridgeError> {
        if let Some(model) = model.filter(|model| is_blocked_model_id(model)) {
            return Err(BridgeError::config_invalid(format!(
                "api model={model} is blocked by this bridge"
            )));
        }
        Ok(())
    }

    async fn cleanup_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        // Snapshot exact authorities while holding the session lock. A same-ID
        // successor can begin immediately after removal, but is not in this
        // cleanup's closed authority set and therefore cannot be touched by it.
        let cells = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| BridgeError::InvalidStateTransition)?;
            let cleanup_cells = self
                .cleanup_cells
                .lock()
                .map_err(|_| BridgeError::InvalidStateTransition)?;
            if let Some(state) = sessions.get_mut(session) {
                if let Some(epoch) = state.current_turn_epoch {
                    state.cancelled_turn_epoch = Some(epoch);
                }
                if let Some(active) = &state.active_request {
                    let _ = active.cancel_control.send(true);
                }
            }
            let cells = cleanup_cells
                .values()
                .filter(|cell| cell.session == *session)
                .cloned()
                .collect::<Vec<_>>();
            for cell in &cells {
                cell.begin_cleanup();
            }
            sessions.remove(session);
            cells
        };

        let mut disposition = BackendCleanupDispositionV1::Complete;
        for cell in &cells {
            if cell.observe().await == BackendCleanupDispositionV1::Unknown {
                disposition = BackendCleanupDispositionV1::Unknown;
            }
        }

        let mut cleanup_cells = self
            .cleanup_cells
            .lock()
            .map_err(|_| BridgeError::InvalidStateTransition)?;
        for cell in cells.iter().filter(|cell| cell.reclaimable()) {
            let authority = cell.turn_authority;
            if cleanup_cells
                .get(&authority)
                .is_some_and(|registered| Arc::ptr_eq(registered, cell))
            {
                cleanup_cells.remove(&authority);
            }
        }
        Ok(disposition)
    }
}

fn record_text_activity(recorder: &Arc<dyn AttemptRecorder>, high_water: &mut u64, text: &str) {
    if text.is_empty() {
        return;
    }
    let delta = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
    // Overflow is sticky and explicit: the local counter still saturates, but
    // the attempt tally is marked incomplete instead of silently absorbing
    // every later genuine increment at the saturated high water.
    *high_water = match high_water.checked_add(delta) {
        Some(next) => next,
        None => {
            recorder.mark_overflowed();
            u64::MAX
        }
    };
    let _ = recorder.record(
        AttemptPhase::Provider,
        ActivityReason::MessageDelta,
        *high_water,
    );
}

impl ApiBackend {
    async fn prompt_inner(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        rich_sink: Option<Arc<dyn RichEventSink>>,
        activity_recorder: Arc<dyn AttemptRecorder>,
        diagnostic_observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendStream, BridgeError> {
        let url = format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        );
        let model = self.resolve_model(session);
        Self::reject_blocked_model(model.as_deref())?;
        let api_key = self.resolve_api_key();
        let lifecycle = ApiLifecycle::new(diagnostic_observer, api_key.as_deref());
        let do_stream = self.cfg.stream;
        let client = self.client.clone();
        let policy = self.policy.clone();
        let max_rounds = self.cfg.max_tool_rounds;

        let turn_scope = self.begin_turn(session)?;
        let turn_epoch = turn_scope.epoch;
        let request_admission = self.request_admission();
        let session = session.clone();

        let mut messages: Vec<Message> = vec![Message::user(
            parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )];

        let stream = async_stream::try_stream! {
            let _turn_scope = turn_scope;
            let mut message_char_high_water = 0u64;
            lifecycle
                .record(DiagnosticPhase::PromptStart, PhaseStatus::Started)
                .await?;

            // Task D crosses this operation-scoped barrier only after durable
            // ProviderSendArmed and immediately before the actual send future's
            // first poll. It remains sticky across every later tool round.
            let acceptance_barrier_crossed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            for round in 0..max_rounds {
                let prepared = match request_admission.prepare(&session, turn_epoch) {
                    Ok(prepared) => prepared,
                    Err(error) => Err(lifecycle
                        .request_flight_failure(error, acceptance_barrier_crossed.load(Ordering::Acquire))
                        .await)?,
                };
                let PreparedRequest::Ready {
                    mut scope,
                    mut cancel_rx,
                } = prepared else {
                    if !acceptance_barrier_crossed.load(Ordering::Acquire) {
                        lifecycle
                            .record(DiagnosticPhase::PromptStart, PhaseStatus::Completed)
                            .await?;
                        lifecycle
                            .record(DiagnosticPhase::PromptStream, PhaseStatus::Started)
                            .await?;
                    }
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                };
                scope.attach_lifecycle(lifecycle.clone(), acceptance_barrier_crossed.load(Ordering::Acquire));
                let mut scope = Some(*scope);
                // Durable reservation, owner attachment, identity evidence,
                // intent, and dispatch all precede installation of the POST future.
                if let Err(error) = scope
                    .as_mut()
                    .ok_or_else(|| {
                        ApiRequestFlightErrorV1::Admission(
                            "request scope disappeared before dispatch".into(),
                        )
                    })
                    .and_then(RequestScope::begin_dispatch)
                {
                    Err(lifecycle
                        .request_flight_failure(error, acceptance_barrier_crossed.load(Ordering::Acquire))
                        .await)?;
                }
                if *cancel_rx.borrow() {
                    settle_request_scope_or_fail(
                        &lifecycle,
                        &mut scope,
                        ResourceActionDispositionV1::Partial,
                        acceptance_barrier_crossed.load(Ordering::Acquire),
                    )
                    .await?;
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                }
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: do_stream };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let send_result = {
                    let scope_ref = scope.as_ref().expect("request scope exists");
                    let accepted = scope_ref.acceptance_marker(Arc::clone(
                        &acceptance_barrier_crossed,
                    ));
                    let send = drive_provider_send(
                        scope_ref.flight.as_ref(),
                        builder.send(),
                        accepted,
                    );
                    let send = if acceptance_barrier_crossed.load(Ordering::Acquire) {
                        send
                    } else {
                        install_first_send(&lifecycle, || send).await?
                    };
                    tokio::pin!(send);
                    loop {
                        tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    break None;
                                }
                            }
                            result = &mut send => break Some(result),
                        }
                    }
                };
                let Some(send_result) = send_result else {
                    settle_request_scope_or_fail(
                        &lifecycle,
                        &mut scope,
                        ResourceActionDispositionV1::Partial,
                        acceptance_barrier_crossed.load(Ordering::Acquire),
                    )
                    .await?;
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                };
                let send_result = match send_result {
                    Ok(result) => result,
                    Err(error) => Err(lifecycle
                        .request_flight_failure(
                            error,
                            acceptance_barrier_crossed.load(Ordering::Acquire),
                        )
                        .await)?,
                };
                let resp = match send_result {
                    Ok(response) => response,
                    Err(error) => {
                        let (class, code, summary) = request_failure(&error, "api.prompt.send");
                        Err(provider_failure_after_settlement(
                                &lifecycle,
                                &mut scope,
                                class,
                                code,
                                summary,
                                Some(error.to_string()),
                                None,
                                None,
                            )
                            .await)?
                    }
                };
                if !resp.status().is_success() {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let body = read_bounded_error_body(resp);
                    tokio::pin!(body);
                    let body_result = loop {
                        tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    break None;
                                }
                            }
                            result = &mut body => break Some(result),
                        }
                    };
                    let Some(body_result) = body_result else {
                        settle_request_scope_or_fail(
                            &lifecycle,
                            &mut scope,
                            ResourceActionDispositionV1::Partial,
                            true,
                        )
                        .await?;
                        complete_prompt_lifecycle(&lifecycle).await?;
                        yield Update::Done {
                            stop_reason: STOP_REASON_CANCELLED.into(),
                            prefix_attestation: Default::default(),
                        };
                        return;
                    };
                    let body = match body_result {
                        Ok(body) => body,
                        Err(error) => {
                            let (class, code, summary) =
                                request_failure(&error, "api.prompt.error_body_read");
                            Err(provider_failure_after_settlement(
                                    &lifecycle,
                                    &mut scope,
                                    class,
                                    code,
                                    summary,
                                    Some(error.to_string()),
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    let ProviderEvidence {
                        class,
                        code,
                        retry_after_ms,
                        reset_at_ms,
                    } = classify_http_error(
                        status,
                        &body.bytes,
                        body.oversized,
                        &headers,
                        diagnostic_timestamp_ms(),
                    );
                    Err(provider_failure_after_settlement(
                            &lifecycle,
                            &mut scope,
                            class,
                            code,
                            "Upstream API rejected the prompt",
                            Some(format!("upstream HTTP status {}", status.as_u16())),
                            retry_after_ms,
                            reset_at_ms,
                        )
                        .await)?;
                    unreachable!("non-success response always terminates the prompt stream");
                }

                let parsed = if do_stream {
                    let mut acc = SseAccumulator::default();
                    let mut bytes = resp.bytes_stream();
                    let mut buf = String::new();
                    'read: loop {
                        let chunk = tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    None
                                } else {
                                    continue 'read;
                                }
                            }
                            maybe = bytes.next() => maybe,
                        };
                        let Some(chunk) = chunk else {
                            if *cancel_rx.borrow() {
                                settle_request_scope_or_fail(
                                    &lifecycle,
                                    &mut scope,
                                    ResourceActionDispositionV1::Partial,
                                    true,
                                )
                                .await?;
                                complete_prompt_lifecycle(&lifecycle).await?;
                                yield Update::Done {
                                    stop_reason: STOP_REASON_CANCELLED.into(),
                                    prefix_attestation: Default::default(),
                                };
                                return;
                            }
                            break 'read;
                        };
                        let chunk = match chunk {
                            Ok(chunk) => chunk,
                            Err(error) => {
                                let (class, code, summary) =
                                    request_failure(&error, "api.prompt.sse_read");
                                Err(provider_failure_after_settlement(
                                        &lifecycle,
                                        &mut scope,
                                        class,
                                        code,
                                        summary,
                                        Some(error.to_string()),
                                        None,
                                        None,
                                    )
                                    .await)?
                            }
                        };
                        buf.push_str(&String::from_utf8_lossy(&chunk));
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            match acc.push_sse_line(&line) {
                                Ok(Some(text)) => {
                                    record_text_activity(&activity_recorder, &mut message_char_high_water, &text);
                                    yield Update::Text(text);
                                }
                                Ok(None) => {}
                                Err(_) => {
                                    Err(provider_failure_after_settlement(
                                            &lifecycle,
                                            &mut scope,
                                            DiagnosticFailureClass::Protocol,
                                            "api.prompt.sse_frame",
                                            "Upstream API returned a malformed SSE frame",
                                            None,
                                            None,
                                            None,
                                        )
                                        .await)?;
                                }
                            }
                            if acc.is_done() { break 'read; }
                        }
                    }
                    // Flush a trailing line that arrived without a newline at EOF — but
                    // ONLY if no terminal was seen (otherwise `buf` is post-[DONE] noise,
                    // and a chunk-split partial "[DON" would falsely FrameError).
                    if !acc.is_done() && !buf.trim().is_empty() {
                        match acc.push_sse_line(&buf) {
                            Ok(Some(text)) => {
                                record_text_activity(&activity_recorder, &mut message_char_high_water, &text);
                                yield Update::Text(text);
                            }
                            Ok(None) => {}
                            Err(_) => {
                                Err(provider_failure_after_settlement(
                                        &lifecycle,
                                        &mut scope,
                                        DiagnosticFailureClass::Protocol,
                                        "api.prompt.sse_frame",
                                        "Upstream API returned a malformed SSE frame",
                                        None,
                                        None,
                                        None,
                                    )
                                    .await)?;
                            }
                        }
                    }
                    if !acc.is_done() {
                        Err(provider_failure_after_settlement(
                                &lifecycle,
                                &mut scope,
                                DiagnosticFailureClass::Protocol,
                                "api.prompt.sse_incomplete",
                                "Upstream API ended SSE before terminal evidence",
                                None,
                                None,
                                None,
                            )
                            .await)?;
                    }
                    acc.finish()
                } else {
                    let body = resp.text();
                    tokio::pin!(body);
                    let body_result = loop {
                        tokio::select! {
                            biased;
                            changed = cancel_rx.changed() => {
                                if changed.is_ok() && *cancel_rx.borrow() {
                                    break None;
                                }
                            }
                            result = &mut body => break Some(result),
                        }
                    };
                    let Some(body_result) = body_result else {
                        settle_request_scope_or_fail(
                            &lifecycle,
                            &mut scope,
                            ResourceActionDispositionV1::Partial,
                            true,
                        )
                        .await?;
                        complete_prompt_lifecycle(&lifecycle).await?;
                        yield Update::Done {
                            stop_reason: STOP_REASON_CANCELLED.into(),
                            prefix_attestation: Default::default(),
                        };
                        return;
                    };
                    let body = match body_result {
                        Ok(body) => body,
                        Err(error) => {
                            let (class, code, summary) =
                                request_failure(&error, "api.prompt.body_read");
                            Err(provider_failure_after_settlement(
                                    &lifecycle,
                                    &mut scope,
                                    class,
                                    code,
                                    summary,
                                    Some(error.to_string()),
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    let p = match crate::wire::parse_nonstream(&body) {
                        Ok(parsed) => parsed,
                        Err(_) => {
                            Err(provider_failure_after_settlement(
                                    &lifecycle,
                                    &mut scope,
                                    DiagnosticFailureClass::Protocol,
                                    "api.prompt.body_parse",
                                    "Upstream API returned a malformed response body",
                                    None,
                                    None,
                                    None,
                                )
                                .await)?
                        }
                    };
                    if !p.text.is_empty() {
                        record_text_activity(&activity_recorder, &mut message_char_high_water, &p.text);
                        yield Update::Text(p.text.clone());
                    }
                    p
                };
                settle_request_scope_or_fail(
                    &lifecycle,
                    &mut scope,
                    ResourceActionDispositionV1::Complete,
                    true,
                )
                .await?;
                if parsed.tool_calls.is_empty() {
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done { stop_reason: "stop".into() , prefix_attestation: Default::default()}; return;
                }
                // Rich observers need to know that the provider requested a tool so callers such as
                // `smoke` can fail closed. Keep the event metadata-only: provider-controlled ids,
                // names, arguments, locations, and content never cross this diagnostic boundary.
                if let Some(sink) = &rich_sink {
                    for (index, _) in parsed.tool_calls.iter().enumerate() {
                        sink.record(OrchEventKind::ToolCall {
                            tool_call_id: format!("api-tool-{round}-{index}"),
                            title: "API tool request".into(),
                            kind: "other".into(),
                            status: "requested".into(),
                            locations: Vec::new(),
                            content: None,
                        });
                    }
                }
                // Tool round: decide each call SILENTLY via the injected policy.
                // NO Update::Permission is yielded — the backend is the sole authority.
                messages.push(Message::assistant_tool_calls(parsed.tool_calls.clone()));
                for tc in &parsed.tool_calls {
                    let result = decide_tool(&policy, tc);
                    messages.push(Message::tool_result(tc.id.clone(), result));
                }
                // continue → re-POST with the appended tool results.
            }
            if !acceptance_barrier_crossed.load(Ordering::Acquire) {
                // Preserve the legacy `max_tool_rounds = 0` terminal shape. No
                // provider request exists in this degenerate configuration.
                lifecycle
                    .record(DiagnosticPhase::PromptStart, PhaseStatus::Completed)
                    .await?;
                lifecycle
                    .record(DiagnosticPhase::PromptStream, PhaseStatus::Started)
                    .await?;
            }
            complete_prompt_lifecycle(&lifecycle).await?;
            yield Update::Done { stop_reason: "max_tool_rounds".into() , prefix_attestation: Default::default()};
        };
        Ok(Box::pin(stream))
    }
}

#[async_trait::async_trait]
impl AgentBackend for ApiBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            None,
            Arc::new(NoopAttemptRecorder),
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            Some(sink),
            Arc::new(NoopAttemptRecorder),
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            observers.rich,
            observers.activity,
            observers.diagnostic,
        )
        .await
    }

    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        Ok(if self.cfg.resource_flight_route_v3.is_some() {
            BackendResourceFlightV1::ProtectedV3
        } else {
            BackendResourceFlightV1::LegacyV2
        })
    }

    fn attach_resource_flight_owner_v1(
        &self,
        session: &SessionId,
    ) -> Result<BackendResourceFlightV1, BridgeError> {
        if self.cfg.resource_flight_route_v3.is_none() {
            return Err(BridgeError::ResourceFlightUnsupported);
        }
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| BridgeError::ResourceFlightUnsupported)?;
        sessions
            .entry(session.clone())
            .or_default()
            .request_flight_owner_attached = true;
        drop(sessions);
        // Load-bearing re-read: a failed/missing attachment cannot become a
        // successful public exposure through a stale local mode assumption.
        self.resource_flight_v1()
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        let exact = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| BridgeError::InvalidStateTransition)?;
            let Some(state) = sessions.get_mut(session) else {
                return Ok(());
            };
            let Some(turn_epoch) = state.current_turn_epoch else {
                return Ok(());
            };
            state.cancelled_turn_epoch = Some(turn_epoch);
            state
                .active_request
                .as_ref()
                .map(|active| RequestCancelCapability {
                    sessions: Arc::clone(&self.sessions),
                    session: session.clone(),
                    turn_epoch,
                    identity: active.identity.clone(),
                })
        };
        // If the captured request settled and a successor published between the
        // two lock acquisitions, exact identity comparison refuses the stale send.
        // The turn epoch remains cancelled, so the successor cannot be POSTed.
        if let Some(exact) = exact {
            exact.cancel_exact();
        }
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        Self::reject_blocked_model(spec.config.model.as_deref())?;
        let mut map = self.sessions.lock().expect("sessions lock");
        map.entry(session.clone()).or_default().model = match &spec.config.model {
            Some(model) => SessionModelState::ExplicitSome(model.clone()),
            None => SessionModelState::ExplicitNone,
        };
        Ok(())
    }

    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        let frozen = spec.provider_effect.frozen();
        let cwd = spec
            .session
            .cwd
            .as_ref()
            .ok_or(BridgeError::ConfigMismatch {
                field: "bound_session_cwd",
            })?;
        let empty_acp_delivery = matches!(
            spec.provider_effect.delivery().payload(),
            BoundMcpDeliveryPayloadV1::Acp(servers) if servers.is_empty()
        );
        if cwd != &frozen.effect.effective_session_cwd
            || cwd != frozen.checkout.effective_cwd()
            || frozen.effect.mcp_delivery_digest != *spec.provider_effect.delivery().digest()
            || !empty_acp_delivery
        {
            return Err(BridgeError::ConfigMismatch {
                field: "bound_provider_effect",
            });
        }
        self.configure_session(session, &spec.session).await?;
        Ok(())
    }

    async fn forget_session(&self, session: &SessionId) {
        let _ = self.cleanup_session_checked(session).await;
    }

    async fn forget_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_checked(session).await
    }

    async fn forget_session_observed(
        &self,
        session: &SessionId,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_checked(session).await
    }

    async fn release_session(&self, session: &SessionId) {
        let _ = self.cleanup_session_checked(session).await;
    }

    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_checked(session).await
    }

    async fn release_session_observed(
        &self,
        session: &SessionId,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        self.cleanup_session_checked(session).await
    }
}

/// Silent permission decision for one tool call → the `content` of its tool-result
/// message. Approve runs the stub tool; Deny/abstain feed a refusal string.
fn decide_tool(policy: &Arc<StdMutex<Arc<dyn PolicyEngine>>>, tc: &ToolCall) -> String {
    let req = PermissionRequest::with_id(tc.id.clone(), /*interactive=*/ false);
    let decision = policy.lock().ok().map(|p| p.decide(&req, &SessionContext));
    match decision {
        Some(Ok(PermissionDecision::Approve)) => {
            if tc.function.name == crate::tool::TOOL_NAME { crate::tool::run_tool() }
            else { format!("unknown tool: {}", tc.function.name) }
        }
        Some(Err(BridgeError::PermissionDenied)) => "permission denied: tool not executed".into(),
        _ /* abstain / poisoned */ => "permission unavailable: tool not executed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{
        EffectiveConfig, PermissionDecision, PermissionRequest, SessionContext, SessionSpec,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::fs_custody::{
        required_object_identity_v2, BirthTimeV1, ChildNameV2, JournalRootBindingV2,
        JournalRootCustodyV2, ObjectIdentityV2,
    };
    use bridge_core::ids::{AttemptIdentity, NodeId, SessionId};
    use bridge_core::ports::{AgentBackend, DiagnosticObserver, PolicyEngine};
    use bridge_core::remote_request_flight::{
        RemoteRequestDeliveryIdV1, RemoteRequestDriverV1, RemoteRequestJournalV1,
        RemoteRequestResultPublisherV1, RemoteRequestTerminalPublicationV1,
    };
    use std::fs;
    use std::os::unix::fs::MetadataExt as _;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn text_activity_high_water_saturates_and_empty_text_is_neutral() {
        let recorder: Arc<dyn AttemptRecorder> = Arc::new(NoopAttemptRecorder);
        let mut high_water = u64::MAX - 1;
        record_text_activity(&recorder, &mut high_water, "abc");
        assert_eq!(high_water, u64::MAX);

        record_text_activity(&recorder, &mut high_water, "");
        assert_eq!(high_water, u64::MAX);
    }

    #[test]
    fn r2f0b_api_text_counter_overflow_is_sticky_explicit() {
        let recorder: Arc<dyn AttemptRecorder> =
            Arc::new(bridge_core::attempt_activity::SharedAttemptRecorder::new(
                bridge_core::attempt_activity::SystemMonotonicClock::start(),
            ));
        let mut high_water = u64::MAX - 1;
        record_text_activity(&recorder, &mut high_water, "abc");
        assert_eq!(high_water, u64::MAX, "the local counter still saturates");
        let tally = recorder.tally().expect("tally");
        assert!(
            tally.overflowed,
            "an API text-counter overflow must become sticky explicit incompleteness"
        );
    }

    #[test]
    fn r2f0b_api_bounded_text_counter_never_flags_overflow() {
        let recorder: Arc<dyn AttemptRecorder> =
            Arc::new(bridge_core::attempt_activity::SharedAttemptRecorder::new(
                bridge_core::attempt_activity::SystemMonotonicClock::start(),
            ));
        let mut high_water = 0_u64;
        record_text_activity(&recorder, &mut high_water, "abc");
        record_text_activity(&recorder, &mut high_water, "de");
        let tally = recorder.tally().expect("tally");
        assert!(!tally.overflowed);
        assert_eq!(tally.meaningful_progress, 2, "real growth stays progress");
        assert_eq!(tally.max_advance, 5);
    }

    struct InstallOrderObserver {
        installed: Arc<AtomicBool>,
        saw_prompt_start_completed: AtomicBool,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for InstallOrderObserver {
        async fn record(
            &self,
            event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            if event.transition().phase() == DiagnosticPhase::PromptStart
                && event.transition().status() == PhaseStatus::Completed
            {
                assert!(
                    self.installed.load(Ordering::SeqCst),
                    "prompt_start completed before the first send future was installed"
                );
                self.saw_prompt_start_completed
                    .store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    enum DiagnosticRecordOutcome {
        Accept,
        Reject,
        Stall,
    }

    struct DiagnosticOutcomeObserver {
        outcome: DiagnosticRecordOutcome,
        calls: AtomicUsize,
    }

    impl DiagnosticOutcomeObserver {
        fn new(outcome: DiagnosticRecordOutcome) -> Self {
            Self {
                outcome,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for DiagnosticOutcomeObserver {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.outcome {
                DiagnosticRecordOutcome::Accept => Ok(()),
                DiagnosticRecordOutcome::Reject => Err(BridgeError::InvalidStateTransition),
                DiagnosticRecordOutcome::Stall => std::future::pending().await,
            }
        }
    }

    struct DenyAll;
    impl PolicyEngine for DenyAll {
        fn decide(
            &self,
            _: &PermissionRequest,
            _: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    #[tokio::test]
    async fn first_send_is_installed_before_post_barrier_transitions() {
        let installed = Arc::new(AtomicBool::new(false));
        let observer = Arc::new(InstallOrderObserver {
            installed: Arc::clone(&installed),
            saw_prompt_start_completed: AtomicBool::new(false),
        });
        let lifecycle = ApiLifecycle::new(observer.clone(), None);
        lifecycle
            .record(DiagnosticPhase::PromptStart, PhaseStatus::Started)
            .await
            .unwrap();

        let send = install_first_send(&lifecycle, || {
            installed.store(true, Ordering::SeqCst);
            std::future::ready(())
        })
        .await
        .unwrap();
        send.await;

        assert!(observer.saw_prompt_start_completed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancel_before_first_stream_poll_closes_the_lifecycle() {
        let backend = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let session = SessionId::parse("cancel-before-first-poll").unwrap();
        let observer =
            Arc::new(bridge_core::diagnostics::InMemoryDiagnosticObserver::new(16).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &session,
                vec![Part { text: "hi".into() }],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
            .unwrap();

        backend.cancel(&session).await.unwrap();
        let mut updates = Vec::new();
        while let Some(update) = stream.next().await {
            updates.push(update.unwrap());
        }

        assert!(matches!(
            updates.last(),
            Some(Update::Done { stop_reason, .. }) if stop_reason == STOP_REASON_CANCELLED
        ));
        let transitions: Vec<_> = observer
            .snapshot()
            .await
            .into_iter()
            .map(|event| (event.transition().phase(), event.transition().status()))
            .collect();
        assert_eq!(
            transitions,
            vec![
                (DiagnosticPhase::PromptStart, PhaseStatus::Started),
                (DiagnosticPhase::PromptStart, PhaseStatus::Completed),
                (DiagnosticPhase::PromptStream, PhaseStatus::Started),
                (DiagnosticPhase::PromptStream, PhaseStatus::Completed),
                (DiagnosticPhase::PromptFinish, PhaseStatus::Started),
                (DiagnosticPhase::PromptFinish, PhaseStatus::Completed),
            ]
        );
    }

    #[tokio::test]
    async fn forgotten_session_authority_cannot_alias_a_recreated_session() {
        for identity in [
            ActiveRequestIdentity::Legacy(0),
            ActiveRequestIdentity::Dedicated(request_id('a')),
        ] {
            let backend = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
            let session = SessionId::parse("forget-recreate-aba").unwrap();
            let old_turn = backend.begin_turn(&session).unwrap();
            let old_epoch = old_turn.epoch;
            let old_cleanup = backend
                .sessions
                .lock()
                .unwrap()
                .get(&session)
                .unwrap()
                .cleanup_cell
                .clone()
                .unwrap();
            let stale = RequestCancelCapability {
                sessions: Arc::clone(&backend.sessions),
                session: session.clone(),
                turn_epoch: old_epoch,
                identity: identity.clone(),
            };

            backend.forget_session(&session).await;
            let new_turn = backend.begin_turn(&session).unwrap();
            let new_epoch = new_turn.epoch;
            let (new_cancel, new_rx) = watch::channel(false);
            backend
                .sessions
                .lock()
                .unwrap()
                .get_mut(&session)
                .unwrap()
                .active_request = Some(ActiveRequestSlot {
                turn_epoch: new_epoch,
                identity: identity.clone(),
                cancel_control: new_cancel,
            });

            let new_cleanup = backend
                .sessions
                .lock()
                .unwrap()
                .get(&session)
                .unwrap()
                .cleanup_cell
                .clone()
                .unwrap();
            assert_ne!(old_epoch, new_epoch, "turn authority must never be reused");
            assert!(!Arc::ptr_eq(&old_cleanup, &new_cleanup));
            assert!(!old_cleanup.finish(&identity, ResourceActionDispositionV1::Complete, true,));
            old_cleanup.begin_cleanup();
            assert_eq!(
                new_cleanup.inner.lock().unwrap().state,
                ApiRequestCleanupStateV1::AdmissionPendingLegacy
            );
            assert!(
                !stale.cancel_exact(),
                "stale cancel must refuse successor B"
            );
            assert!(!stale.clear_exact(), "stale clear must refuse successor B");
            drop(old_turn);

            let sessions = backend.sessions.lock().unwrap();
            let successor = sessions.get(&session).unwrap();
            assert_eq!(successor.current_turn_epoch, Some(new_epoch));
            assert_eq!(
                successor.active_request.as_ref().map(|slot| &slot.identity),
                Some(&identity)
            );
            assert!(!*new_rx.borrow(), "successor B must remain uncancelled");
            drop(sessions);
            drop(new_turn);
        }
    }

    #[tokio::test]
    async fn configure_session_stashes_model_and_object_safe() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(
            &s,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("haiku".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(be.session_model(&s).as_deref(), Some("haiku"));
        be.forget_session(&s).await;
        assert!(be.session_model(&s).is_none());
        let _obj: Arc<dyn AgentBackend> = Arc::new(ApiBackend::new(crate::config::ApiConfig::new(
            "http://127.0.0.1:1",
        )));
    }

    #[tokio::test]
    async fn explicit_none_session_model_suppresses_the_spawn_default() {
        let mut config = crate::config::ApiConfig::new("http://127.0.0.1:1");
        config.model = Some("model-m".into());
        let backend = ApiBackend::new(config);
        let session = SessionId::parse("explicit-none").unwrap();

        backend
            .configure_session(
                &session,
                &SessionSpec::from_config(EffectiveConfig {
                    model: None,
                    ..Default::default()
                }),
            )
            .await
            .unwrap();

        assert_eq!(backend.resolve_model(&session), None);
    }

    #[tokio::test]
    async fn configure_session_rejects_blocked_fable_family_model() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let s = SessionId::parse("s1").unwrap();
        let err = be
            .configure_session(
                &s,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("claude-fable-5.1[1m]".into()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("api model=claude-fable-5.1[1m] is blocked by this bridge"),
            "{err}"
        );
        assert!(be.session_model(&s).is_none());
    }

    #[tokio::test]
    async fn prompt_rejects_static_blocked_fable_family_model_before_http() {
        let mut cfg = crate::config::ApiConfig::new("http://127.0.0.1:1");
        cfg.model = Some("claude-fable-5.1[1m]".into());
        let be = ApiBackend::new(cfg);
        let s = SessionId::parse("s1").unwrap();
        match be.prompt(&s, vec![Part { text: "hi".into() }]).await {
            Err(err) => assert!(
                err.to_string()
                    .contains("api model=claude-fable-5.1[1m] is blocked by this bridge"),
                "{err}"
            ),
            Ok(_) => panic!("blocked API model must fail before creating a stream"),
        }
    }

    fn request_id(digit: char) -> DedicatedRemoteRequestIdV1 {
        DedicatedRemoteRequestIdV1::parse(format!(
            "{}{}",
            DedicatedRemoteRequestIdV1::PREFIX,
            digit.to_string().repeat(64)
        ))
        .unwrap()
    }

    type RequestJournalRoute = (PathBuf, PathBuf, JournalRootBindingV2);

    fn object(path: &std::path::Path) -> ObjectIdentityV2 {
        let metadata = fs::metadata(path).unwrap();
        required_object_identity_v2(
            metadata.dev(),
            metadata.ino(),
            BirthTimeV1::from_metadata(&metadata),
            "API request fixture",
        )
        .unwrap()
    }

    fn request_journal_route(base: &std::path::Path) -> RequestJournalRoute {
        let anchor = base.join("anchor");
        let parent = anchor.join("parent");
        let root = parent.join("requests");
        let lock = parent.join("operation.lock");
        fs::create_dir_all(&root).unwrap();
        fs::write(&lock, b"").unwrap();
        let binding = JournalRootBindingV2::new(
            object(&anchor),
            ChildNameV2::from_bytes(b"parent").unwrap(),
            object(&parent),
            ChildNameV2::from_bytes(b"requests").unwrap(),
            object(&root),
            ChildNameV2::from_bytes(b"operation.lock").unwrap(),
            object(&lock),
        )
        .unwrap();
        (anchor, root, binding)
    }

    fn request_custody(route: &RequestJournalRoute) -> JournalRootCustodyV2 {
        JournalRootCustodyV2::open(&route.0, &route.2, "API request journal").unwrap()
    }

    fn request_driver(
        route: &RequestJournalRoute,
        capacity: usize,
        publisher: Arc<dyn RemoteRequestResultPublisherV1>,
    ) -> Arc<RemoteRequestDriverV1> {
        let attempt = AttemptIdentity::initial().unwrap();
        RemoteRequestJournalV1::initialize(request_custody(route), attempt.clone()).unwrap();
        Arc::new(
            RemoteRequestDriverV1::open_recovered(
                request_custody(route),
                attempt,
                capacity,
                publisher,
            )
            .unwrap(),
        )
    }

    fn checkpoint_next_ordinal(root: &std::path::Path) -> u64 {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("remote-request-checkpoint.json")).unwrap())
                .unwrap();
        value["next_ordinal"].as_u64().unwrap()
    }

    #[derive(Default)]
    struct RecordingRequestPublisher(StdMutex<Vec<RemoteRequestTerminalPublicationV1>>);

    impl RemoteRequestResultPublisherV1 for RecordingRequestPublisher {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            self.0.lock().unwrap().push(publication.clone());
            Ok(publication.delivery_id().clone())
        }
    }

    struct BlockingRequestPublisher {
        arrived: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    }

    impl RemoteRequestResultPublisherV1 for BlockingRequestPublisher {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            self.arrived.wait();
            self.release.wait();
            Ok(publication.delivery_id().clone())
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum TerminalPublisherFault {
        Refuse,
        Mismatch,
    }

    struct FaultingRequestPublisher {
        fault: TerminalPublisherFault,
        calls: AtomicUsize,
    }

    impl RemoteRequestResultPublisherV1 for FaultingRequestPublisher {
        fn publish_idempotent(
            &self,
            publication: &RemoteRequestTerminalPublicationV1,
        ) -> Result<RemoteRequestDeliveryIdV1, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.fault {
                TerminalPublisherFault::Refuse => Err("injected terminal refusal".into()),
                TerminalPublisherFault::Mismatch => {
                    let mut receipt = serde_json::to_value(publication.delivery_id())
                        .map_err(|error| error.to_string())?;
                    let ordinal = receipt["ordinal"]
                        .as_u64()
                        .ok_or_else(|| "delivery ordinal missing".to_string())?;
                    receipt["ordinal"] = serde_json::Value::from(ordinal.saturating_add(1));
                    serde_json::from_value(receipt).map_err(|error| error.to_string())
                }
            }
        }
    }

    struct ProtectedBackendFixture {
        backend: Arc<ApiBackend>,
        publisher: Arc<RecordingRequestPublisher>,
        _root: tempfile::TempDir,
        journal_root: PathBuf,
    }

    fn protected_backend(
        base_url: String,
        _request_ids: Vec<DedicatedRemoteRequestIdV1>,
        journal_cap: usize,
        max_tool_rounds: usize,
        policy: Option<Arc<dyn PolicyEngine>>,
    ) -> ProtectedBackendFixture {
        let root = tempfile::tempdir().unwrap();
        let route = request_journal_route(root.path());
        let journal_root = route.1.clone();
        let publisher = Arc::new(RecordingRequestPublisher::default());
        let publisher_port: Arc<dyn RemoteRequestResultPublisherV1> = publisher.clone();
        let driver = request_driver(&route, journal_cap, publisher_port);
        let mut cfg = crate::config::ApiConfig::new(base_url);
        cfg.max_tool_rounds = max_tool_rounds;
        cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
            driver,
            NodeId::parse("api-node").unwrap(),
        ));
        let backend = ApiBackend::new(cfg);
        let backend = match policy {
            Some(policy) => backend.with_policy(policy),
            None => backend,
        };
        ProtectedBackendFixture {
            backend: Arc::new(backend),
            publisher,
            journal_root,
            _root: root,
        }
    }

    async fn task_f_exit_after_dispatch_before_first_send_poll(cancelled: bool) {
        let server = MockServer::start().await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let session = SessionId::parse(if cancelled {
            "task-f-unpolled-cancel"
        } else {
            "task-f-unpolled-drop"
        })
        .unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let turn = fixture.backend.begin_turn(&session).unwrap();
        let PreparedRequest::Ready {
            mut scope,
            cancel_rx,
        } = fixture
            .backend
            .request_admission()
            .prepare(&session, turn.epoch)
            .unwrap()
        else {
            panic!("Task F request must be admitted");
        };
        scope.begin_dispatch().unwrap();

        let turn_accepted = Arc::new(AtomicBool::new(false));
        let accepted = scope.acceptance_marker(Arc::clone(&turn_accepted));
        let send = drive_provider_send(
            scope.flight.as_ref(),
            fixture
                .backend
                .client
                .post(format!("{}/v1/chat/completions", server.uri()))
                .send(),
            accepted,
        );
        if cancelled {
            fixture.backend.cancel(&session).await.unwrap();
            assert!(*cancel_rx.borrow(), "the exact request must observe cancel");
        }

        drop(send);
        assert!(!turn_accepted.load(Ordering::Acquire));
        assert!(!scope.accepted.load(Ordering::Acquire));
        drop(scope);
        drop(turn);

        assert!(server.received_requests().await.unwrap().is_empty());
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Failed
        );
        assert!(!publications[0].prompt_may_have_been_accepted());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn task_f_reqwest_send_poll_barrier_distinguishes_unpolled_and_accepted_cancellation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        for (session_name, release_send, disposition, accepted) in [
            (
                "task-f-poll-barrier-before",
                false,
                ResourceActionDispositionV1::Failed,
                false,
            ),
            (
                "task-f-poll-barrier-after",
                true,
                ResourceActionDispositionV1::Partial,
                true,
            ),
        ] {
            let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
            let session = SessionId::parse(session_name).unwrap();
            fixture
                .backend
                .attach_resource_flight_owner_v1(&session)
                .unwrap();
            let turn = fixture.backend.begin_turn(&session).unwrap();
            let PreparedRequest::Ready {
                mut scope,
                cancel_rx,
            } = fixture
                .backend
                .request_admission()
                .prepare(&session, turn.epoch)
                .unwrap()
            else {
                panic!("Task F barrier request must be admitted");
            };
            scope.begin_dispatch().unwrap();
            let turn_accepted = Arc::new(AtomicBool::new(false));
            let (barrier, _reset) = RequestSendPollBarrierForTest::install();
            {
                let send = drive_provider_send(
                    scope.flight.as_ref(),
                    fixture
                        .backend
                        .client
                        .post(format!("{}/v1/chat/completions", server.uri()))
                        .send(),
                    scope.acceptance_marker(Arc::clone(&turn_accepted)),
                );
                tokio::pin!(send);
                tokio::select! {
                    _ = barrier.wait_until_entered() => {}
                    _ = &mut send => panic!("request completed before the poll barrier"),
                }
                if release_send {
                    barrier.release();
                    tokio::select! {
                        _ = wait_for_provider_requests(&server, 1) => {}
                        _ = &mut send => panic!("request completed before its first poll was observed"),
                    }
                }
                fixture.backend.cancel(&session).await.unwrap();
                assert!(*cancel_rx.borrow(), "the exact request must observe cancel");
            }
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                if release_send { 1 } else { 0 }
            );
            assert_eq!(
                turn_accepted.load(Ordering::Acquire),
                accepted,
                "only a released barrier may cross acceptance"
            );
            drop(scope);
            drop(turn);
            let publications = fixture.publisher.0.lock().unwrap();
            assert_eq!(publications.len(), 1);
            assert_eq!(publications[0].result().disposition, disposition);
            assert_eq!(publications[0].prompt_may_have_been_accepted(), accepted);
        }
    }

    #[tokio::test]
    async fn task_f_cancel_after_dispatch_before_first_send_poll_is_failed_unaccepted() {
        task_f_exit_after_dispatch_before_first_send_poll(true).await;
    }

    #[tokio::test]
    async fn task_f_drop_after_dispatch_before_first_send_poll_is_failed_unaccepted() {
        task_f_exit_after_dispatch_before_first_send_poll(false).await;
    }

    async fn task_f_successor_round_exit_before_first_send_poll(cancelled: bool) {
        let server = MockServer::start().await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let session = SessionId::parse(if cancelled {
            "task-f-successor-cancel"
        } else {
            "task-f-successor-drop"
        })
        .unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let turn = fixture.backend.begin_turn(&session).unwrap();
        let PreparedRequest::Ready {
            mut scope,
            cancel_rx,
        } = fixture
            .backend
            .request_admission()
            .prepare(&session, turn.epoch)
            .unwrap()
        else {
            panic!("Task F successor request must be admitted");
        };
        // A successor tool-call round attaches its lifecycle with the
        // turn-wide acceptance barrier already crossed — the exact
        // production round-loop call shape.
        let observer: Arc<dyn DiagnosticObserver> =
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default());
        scope.attach_lifecycle(ApiLifecycle::new(observer, None), true);
        scope.begin_dispatch().unwrap();

        let turn_accepted = Arc::new(AtomicBool::new(true));
        let accepted = scope.acceptance_marker(Arc::clone(&turn_accepted));
        let send = drive_provider_send(
            scope.flight.as_ref(),
            fixture
                .backend
                .client
                .post(format!("{}/v1/chat/completions", server.uri()))
                .send(),
            accepted,
        );
        if cancelled {
            fixture.backend.cancel(&session).await.unwrap();
            assert!(*cancel_rx.borrow(), "the exact request must observe cancel");
        }

        drop(send);
        assert!(
            !scope.accepted.load(Ordering::Acquire),
            "the successor request bit must stay request-local until its own first poll"
        );
        let cell = Arc::clone(&scope.cleanup);
        drop(scope);
        drop(turn);

        assert!(server.received_requests().await.unwrap().is_empty());
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Failed,
            "an unpolled successor exit must persist Failed, never Partial/Unknown"
        );
        assert!(!publications[0].prompt_may_have_been_accepted());
        // The sticky turn acceptance is preserved as cleanup diagnostic
        // custody even though the request row stayed unaccepted.
        assert!(cell.inner.lock().unwrap().accepted);
    }

    #[tokio::test]
    async fn task_f_successor_cancel_before_first_send_poll_is_failed_unaccepted() {
        task_f_successor_round_exit_before_first_send_poll(true).await;
    }

    #[tokio::test]
    async fn task_f_successor_drop_before_first_send_poll_is_failed_unaccepted() {
        task_f_successor_round_exit_before_first_send_poll(false).await;
    }

    #[tokio::test]
    async fn task_f_second_round_cancel_after_send_observed_stays_partial_accepted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(tool_call_sse()),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('a'), request_id('b')],
            64,
            4,
            None,
        );
        let session = SessionId::parse("task-f-round-two-accepted-cancel").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if server.received_requests().await.unwrap().len() >= 2 {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("round two must reach the provider");
        fixture.backend.cancel(&session).await.unwrap();

        let updates = task.await.unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 2);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Complete
        );
        assert!(publications[0].prompt_may_have_been_accepted());
        assert_eq!(
            publications[1].result().disposition,
            ResourceActionDispositionV1::Partial,
            "an accepted in-flight cancellation must stay Partial"
        );
        assert!(publications[1].prompt_may_have_been_accepted());
    }

    fn cleanup_request_flight(
        base: &std::path::Path,
        session: &SessionId,
    ) -> (ActiveRequestIdentity, OwnedRemoteRequestV1, PathBuf) {
        let route = request_journal_route(base);
        let publisher: Arc<dyn RemoteRequestResultPublisherV1> =
            Arc::new(RecordingRequestPublisher::default());
        let driver = request_driver(&route, 64, publisher);
        let owner = ResourceFlightOwnerV1::new(
            NodeId::parse("task-e-cleanup-node").unwrap(),
            session.as_str(),
        )
        .unwrap();
        let flight = driver.admit(owner).unwrap();
        let identity = ActiveRequestIdentity::Dedicated(flight.request_id().clone());
        flight.journal_intent().unwrap();
        flight.authorize_dispatch().unwrap();
        futures::executor::block_on(flight.arm_provider_send(std::future::ready(()))).unwrap();
        (identity, flight, route.1)
    }

    fn refused_cleanup_cell(
        session: &SessionId,
        timeout: Duration,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Arc<ApiRequestCleanupCustodianV1> {
        let identity = ActiveRequestIdentity::Dedicated(request_id('d'));
        let cell = ApiRequestCleanupCustodianV1::new(91, session.clone(), true, timeout);
        cell.begin_admission().unwrap();
        cell.bind(&identity).unwrap();
        cell.mark_accepted(&identity);
        cell.refuse(
            Some(&identity),
            ApiRequestFlightErrorV1::Admission("injected settlement refusal".into()),
            Some(ApiLifecycle::new(observer, None)),
        );
        cell
    }

    async fn wait_for_active_request(
        backend: &Arc<ApiBackend>,
        session: &SessionId,
        prior: Option<&ActiveRequestIdentity>,
    ) -> RequestCancelCapability {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let capability = {
                    let sessions = backend.sessions.lock().unwrap();
                    sessions.get(session).and_then(|state| {
                        state.active_request.as_ref().and_then(|active| {
                            (prior != Some(&active.identity)).then(|| RequestCancelCapability {
                                sessions: Arc::clone(&backend.sessions),
                                session: session.clone(),
                                turn_epoch: active.turn_epoch,
                                identity: active.identity.clone(),
                            })
                        })
                    })
                };
                if let Some(capability) = capability {
                    return capability;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request slot was not published")
    }

    fn exact_request_cancelled(
        backend: &ApiBackend,
        session: &SessionId,
        expected: &ActiveRequestIdentity,
    ) -> Option<bool> {
        let sessions = backend.sessions.lock().unwrap();
        let active = sessions.get(session)?.active_request.as_ref()?;
        if &active.identity != expected {
            return None;
        }
        let cancelled = *active.cancel_control.borrow();
        Some(cancelled)
    }

    fn tool_call_sse() -> &'static str {
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"
    }

    fn stop_sse() -> &'static str {
        "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
    }

    fn spawn_drain(
        backend: Arc<ApiBackend>,
        session: SessionId,
    ) -> tokio::task::JoinHandle<Vec<Result<Update, BridgeError>>> {
        tokio::spawn(async move {
            let mut stream = backend
                .prompt(&session, vec![Part { text: "hi".into() }])
                .await
                .unwrap();
            let mut updates = Vec::new();
            while let Some(update) = stream.next().await {
                updates.push(update);
            }
            updates
        })
    }

    async fn wait_for_provider_requests(server: &MockServer, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if server.received_requests().await.unwrap().len() == expected {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider request count was not reached");
    }

    #[tokio::test]
    async fn stale_round_one_cancel_cannot_cancel_round_two() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("\"role\":\"tool\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(tool_call_sse())
                    .set_delay(Duration::from_millis(100)),
            )
            .mount(&server)
            .await;

        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let session = SessionId::parse("stale-round").unwrap();
        assert_eq!(
            fixture
                .backend
                .attach_resource_flight_owner_v1(&session)
                .unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        let stale_a = wait_for_active_request(&fixture.backend, &session, None).await;
        let id_a = stale_a.identity.clone();
        let current_b = wait_for_active_request(&fixture.backend, &session, Some(&id_a)).await;
        let id_b = current_b.identity.clone();

        assert!(
            !stale_a.cancel_exact(),
            "a retained A sender must refuse successor B"
        );
        assert!(
            !stale_a.clear_exact(),
            "a retained A settlement/drop must not clear successor B"
        );
        assert_eq!(
            exact_request_cancelled(&fixture.backend, &session, &id_b),
            Some(false),
            "the mutation-sensitive positive state proves B remained live"
        );

        fixture.backend.cancel(&session).await.unwrap();
        fixture.backend.cancel(&session).await.unwrap();
        let updates = task.await.unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        let publications = fixture.publisher.0.lock().unwrap().clone();
        assert_eq!(publications.len(), 2);
        assert_ne!(publications[0].delivery_id(), publications[1].delivery_id());
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Complete
        );
        assert_eq!(
            publications[1].result().disposition,
            ResourceActionDispositionV1::Partial
        );
        assert!(publications
            .iter()
            .all(RemoteRequestTerminalPublicationV1::prompt_may_have_been_accepted));
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 2);
        assert_eq!(
            fixture
                .backend
                .forget_session_checked(&session)
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
    }

    struct BetweenRoundsPolicy {
        arrived: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    }

    impl PolicyEngine for BetweenRoundsPolicy {
        fn decide(
            &self,
            _: &PermissionRequest,
            _: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            self.arrived.wait();
            self.release.wait();
            Ok(PermissionDecision::Approve)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_between_round_terminal_and_successor_publication_prevents_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(tool_call_sse()),
            )
            .mount(&server)
            .await;
        let policy = Arc::new(BetweenRoundsPolicy {
            arrived: Arc::new(std::sync::Barrier::new(2)),
            release: Arc::new(std::sync::Barrier::new(2)),
        });
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![],
            64,
            4,
            Some(policy.clone()),
        );
        let session = SessionId::parse("between-rounds").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());

        let arrived = Arc::clone(&policy.arrived);
        tokio::task::spawn_blocking(move || arrived.wait())
            .await
            .unwrap();
        fixture.backend.cancel(&session).await.unwrap();
        let release = Arc::clone(&policy.release);
        tokio::task::spawn_blocking(move || release.wait())
            .await
            .unwrap();

        let updates = task.await.unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 1);
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Complete
        );
        assert!(publications[0].prompt_may_have_been_accepted());
    }

    struct BreakJournalBetweenRounds {
        journal_root: PathBuf,
        moved_root: PathBuf,
    }

    impl PolicyEngine for BreakJournalBetweenRounds {
        fn decide(
            &self,
            _: &PermissionRequest,
            _: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            std::fs::rename(&self.journal_root, &self.moved_root)
                .map_err(|error| BridgeError::agent_crashed(error.to_string()))?;
            Ok(PermissionDecision::Approve)
        }
    }

    #[tokio::test]
    async fn round_two_journal_failure_refuses_before_post_and_preserves_round_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(tool_call_sse()),
            )
            .mount(&server)
            .await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let moved_root = fixture._root.path().join("journal-disabled");
        *fixture.backend.policy.lock().unwrap() = Arc::new(BreakJournalBetweenRounds {
            journal_root: fixture.journal_root.clone(),
            moved_root: moved_root.clone(),
        });
        let session = SessionId::parse("round-two-journal").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();

        let updates = spawn_drain(Arc::clone(&fixture.backend), session)
            .await
            .unwrap();
        let diagnostic = updates
            .iter()
            .find_map(|update| match update {
                Err(BridgeError::AgentFailure { diagnostic }) => Some(diagnostic.as_ref()),
                _ => None,
            })
            .expect("round-two custody failure must be structured");
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Persistence);
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(diagnostic.prompt_may_have_been_accepted());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert!(!fixture.journal_root.exists());
        assert_eq!(checkpoint_next_ordinal(&moved_root), 1);
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Complete
        );
        assert!(publications[0].prompt_may_have_been_accepted());
    }

    #[tokio::test]
    async fn request_capacity_refusal_precedes_flight_creation_and_post() {
        let server = MockServer::start().await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 5, 4, None);
        let owner = ResourceFlightOwnerV1::new(
            NodeId::parse("capacity-prefill").unwrap(),
            "capacity-prefill",
        )
        .unwrap();
        let _held = fixture
            .backend
            .cfg
            .resource_flight_route_v3
            .as_ref()
            .unwrap()
            .attempt
            .admit(owner)
            .unwrap();
        let session = SessionId::parse("capacity-refusal").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let updates = spawn_drain(Arc::clone(&fixture.backend), session)
            .await
            .unwrap();
        let diagnostic = updates
            .iter()
            .find_map(|update| match update {
                Err(BridgeError::AgentFailure { diagnostic }) => Some(diagnostic.as_ref()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("pre-send custody refusal must be structured: {updates:?}"));
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Persistence);
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(!diagnostic.prompt_may_have_been_accepted());
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 1);
        assert!(
            fixture.publisher.0.lock().unwrap().is_empty(),
            "capacity is reserved by FlightReserved, so refusal creates no live flight"
        );
    }

    #[tokio::test]
    async fn consumer_drop_settles_and_clears_only_the_exact_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let session = SessionId::parse("consumer-drop").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        wait_for_active_request(&fixture.backend, &session, None).await;
        task.abort();
        let _ = task.await;

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if fixture.publisher.0.lock().unwrap().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(fixture
            .backend
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .is_some_and(
                |state| state.active_request.is_none() && state.current_turn_epoch.is_none()
            ));
        let publications = fixture.publisher.0.lock().unwrap();
        assert_eq!(publications.len(), 1);
        assert_eq!(
            publications[0].result().disposition,
            ResourceActionDispositionV1::Unknown
        );
    }

    #[tokio::test]
    async fn task_e_active_legacy_checked_cleanup_is_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let backend = Arc::new(ApiBackend::new(crate::config::ApiConfig::new(format!(
            "{}/v1",
            server.uri()
        ))));
        let session = SessionId::parse("task-e-active-legacy").unwrap();
        let task = spawn_drain(Arc::clone(&backend), session.clone());
        wait_for_active_request(&backend, &session, None).await;

        assert_eq!(
            backend.forget_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        assert!(matches!(
            task.await.unwrap().last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));
    }

    #[tokio::test]
    async fn checked_forget_and_release_join_the_exact_request_winner() {
        for operation in ["forget", "release"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/chat/completions"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(stop_sse())
                        .set_delay(Duration::from_secs(5)),
                )
                .mount(&server)
                .await;
            let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
            let session = SessionId::parse(format!("{operation}-exact")).unwrap();
            fixture
                .backend
                .attach_resource_flight_owner_v1(&session)
                .unwrap();
            let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
            wait_for_active_request(&fixture.backend, &session, None).await;
            wait_for_provider_requests(&server, 1).await;
            let cleanup = if operation == "forget" {
                fixture.backend.forget_session_checked(&session).await
            } else {
                fixture.backend.release_session_checked(&session).await
            }
            .unwrap();

            assert_eq!(cleanup, BackendCleanupDispositionV1::Unknown);
            let updates = task.await.unwrap();
            assert!(matches!(
                updates.last(),
                Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
            ));
            assert!(!fixture
                .backend
                .sessions
                .lock()
                .unwrap()
                .contains_key(&session));
            {
                let publications = fixture.publisher.0.lock().unwrap();
                assert_eq!(publications.len(), 1);
                assert_eq!(
                    publications[0].result().disposition,
                    ResourceActionDispositionV1::Partial
                );
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn settlement_refusal_does_not_mask_the_provider_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string("data: not-json\n\n")
                    .set_delay(Duration::from_millis(200)),
            )
            .mount(&server)
            .await;
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('f')],
            64,
            4,
            None,
        );
        let session = SessionId::parse("provider-plus-settlement").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session);
        wait_for_provider_requests(&server, 1).await;
        std::fs::rename(
            &fixture.journal_root,
            fixture._root.path().join("journal-refusing-terminal"),
        )
        .unwrap();

        let updates = task.await.unwrap();
        let diagnostic = updates
            .iter()
            .find_map(|update| match update {
                Err(BridgeError::AgentFailure { diagnostic }) => Some(diagnostic.as_ref()),
                _ => None,
            })
            .expect("provider failure must remain structured");
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Protocol);
        assert!(diagnostic.prompt_may_have_been_accepted());
        assert!(diagnostic
            .causes()
            .iter()
            .any(|cause| cause.contains("durable request settlement refused")));
    }

    #[tokio::test]
    async fn checked_cleanup_projects_terminal_refusal_as_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_secs(5)),
            )
            .mount(&server)
            .await;
        let fixture = protected_backend(format!("{}/v1", server.uri()), vec![], 64, 4, None);
        let session = SessionId::parse("cleanup-terminal-refusal").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        wait_for_active_request(&fixture.backend, &session, None).await;
        wait_for_provider_requests(&server, 1).await;
        std::fs::rename(
            &fixture.journal_root,
            fixture._root.path().join("journal-refusing-cleanup"),
        )
        .unwrap();

        assert_eq!(
            fixture
                .backend
                .forget_session_checked(&session)
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        let updates = task.await.unwrap();
        assert!(updates.iter().any(|update| matches!(
            update,
            Err(BridgeError::AgentFailure { diagnostic })
                if diagnostic.prompt_may_have_been_accepted()
        )));
        assert_eq!(
            fixture
                .backend
                .release_session_checked(&session)
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Unknown,
            "terminal-refusal debt must outlive the removed session slot"
        );
    }

    #[tokio::test]
    async fn task_f_refusing_and_mismatched_publishers_fail_prompt_and_cleanup_unknown() {
        for (fault, session_name) in [
            (TerminalPublisherFault::Refuse, "task-f-publisher-refusal"),
            (
                TerminalPublisherFault::Mismatch,
                "task-f-publisher-mismatch",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/chat/completions"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(stop_sse()),
                )
                .mount(&server)
                .await;
            let root = tempfile::tempdir().unwrap();
            let route = request_journal_route(root.path());
            let publisher = Arc::new(FaultingRequestPublisher {
                fault,
                calls: AtomicUsize::new(0),
            });
            let publisher_port: Arc<dyn RemoteRequestResultPublisherV1> = publisher.clone();
            let driver = request_driver(&route, 64, publisher_port);
            let mut cfg = crate::config::ApiConfig::new(format!("{}/v1", server.uri()));
            cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
                driver,
                NodeId::parse("api-node").unwrap(),
            ));
            let backend = Arc::new(ApiBackend::new(cfg));
            let session = SessionId::parse(session_name).unwrap();
            backend.attach_resource_flight_owner_v1(&session).unwrap();

            let updates = spawn_drain(Arc::clone(&backend), session.clone())
                .await
                .unwrap();
            assert!(
                updates
                    .iter()
                    .any(|update| matches!(update, Err(BridgeError::AgentFailure { .. }))),
                "{fault:?}: terminal publisher fault must fail the prompt: {updates:?}"
            );
            assert_eq!(publisher.calls.load(Ordering::SeqCst), 1, "{fault:?}");
            assert_eq!(
                backend.forget_session_checked(&session).await.unwrap(),
                BackendCleanupDispositionV1::Unknown,
                "{fault:?}: publication debt must not project a false Complete cleanup"
            );
        }
    }

    #[tokio::test]
    async fn poisoned_request_owner_attachment_and_admission_refuse() {
        let fixture = protected_backend(
            "http://127.0.0.1:1/v1".into(),
            vec![request_id('b')],
            64,
            4,
            None,
        );
        let sessions = Arc::clone(&fixture.backend.sessions);
        assert!(std::thread::spawn(move || {
            let _guard = sessions.lock().unwrap();
            panic!("poison request-flight attachment state");
        })
        .join()
        .is_err());
        let session = SessionId::parse("poisoned-attachment").unwrap();
        assert!(matches!(
            fixture.backend.attach_resource_flight_owner_v1(&session),
            Err(BridgeError::ResourceFlightUnsupported)
        ));
        assert!(matches!(
            fixture
                .backend
                .prompt(&session, vec![Part { text: "hi".into() }])
                .await,
            Err(BridgeError::ResourceFlightUnsupported)
        ));
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 0);
    }

    #[tokio::test]
    async fn zero_rounds_mints_no_request_flight_and_missing_attachment_refuses() {
        let server = MockServer::start().await;
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('8')],
            64,
            0,
            None,
        );
        let session = SessionId::parse("zero-rounds").unwrap();
        assert_eq!(
            fixture.backend.resource_flight_v1().unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        assert!(matches!(
            fixture
                .backend
                .prompt(&session, vec![Part { text: "hi".into() }])
                .await,
            Err(BridgeError::ResourceFlightUnsupported)
        ));
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 0);
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let updates = spawn_drain(Arc::clone(&fixture.backend), session)
            .await
            .unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == "max_tool_rounds"
        ));
        assert_eq!(checkpoint_next_ordinal(&fixture.journal_root), 0);
        assert!(fixture.publisher.0.lock().unwrap().is_empty());
        assert!(server.received_requests().await.unwrap().is_empty());

        let v2 = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        assert_eq!(
            v2.resource_flight_v1().unwrap(),
            BackendResourceFlightV1::LegacyV2
        );
        assert!(matches!(
            v2.attach_resource_flight_owner_v1(&SessionId::parse("v2").unwrap()),
            Err(BridgeError::ResourceFlightUnsupported)
        ));
    }

    #[tokio::test]
    async fn fresh_turn_is_live_and_stale_prior_turn_control_cannot_affect_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse())
                    .set_delay(Duration::from_millis(250)),
            )
            .mount(&server)
            .await;
        let backend = Arc::new(ApiBackend::new(crate::config::ApiConfig::new(format!(
            "{}/v1",
            server.uri()
        ))));
        let session = SessionId::parse("fresh-after-cancel").unwrap();

        let first = spawn_drain(Arc::clone(&backend), session.clone());
        let stale = wait_for_active_request(&backend, &session, None).await;
        backend.cancel(&session).await.unwrap();
        let first_updates = first.await.unwrap();
        assert!(matches!(
            first_updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));

        let second = spawn_drain(Arc::clone(&backend), session.clone());
        let current = wait_for_active_request(&backend, &session, Some(&stale.identity)).await;
        assert!(!stale.cancel_exact());
        assert!(!stale.clear_exact());
        assert_eq!(
            exact_request_cancelled(&backend, &session, &current.identity),
            Some(false)
        );
        let second_updates = second.await.unwrap();
        assert!(matches!(
            second_updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == "stop"
        ));
    }

    #[tokio::test]
    async fn with_policy_swaps_engine() {
        let be = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"))
            .with_policy(Arc::new(DenyAll));
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&be);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_e_late_complete_cannot_overwrite_timed_out_through_public_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse()),
            )
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let route = request_journal_route(root.path());
        let arrived = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let publisher: Arc<dyn RemoteRequestResultPublisherV1> =
            Arc::new(BlockingRequestPublisher {
                arrived: Arc::clone(&arrived),
                release: Arc::clone(&release),
            });
        let driver = request_driver(&route, 64, publisher);
        let mut cfg = crate::config::ApiConfig::new(format!("{}/v1", server.uri()));
        // The barriers, not this clock, make the crossing deterministic: the
        // publisher stays stalled until explicitly released, so the cleanup
        // deadline always expires mid-settlement. The bound only needs to be
        // large enough that the HTTP round never times out under full-suite
        // parallel load (200ms was load-marginal and failed whole-workspace
        // runs on unmodified accepted heads).
        cfg.request_timeout = Duration::from_secs(2);
        cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
            driver,
            NodeId::parse("api-node").unwrap(),
        ));
        let backend = Arc::new(ApiBackend::new(cfg));
        let session = SessionId::parse("task-e-public-late-complete").unwrap();
        backend.attach_resource_flight_owner_v1(&session).unwrap();
        let task = spawn_drain(Arc::clone(&backend), session.clone());

        let arrived_gate = Arc::clone(&arrived);
        tokio::task::spawn_blocking(move || arrived_gate.wait())
            .await
            .unwrap();
        let cell = backend
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .unwrap()
            .cleanup_cell
            .clone()
            .unwrap();
        let cleanup = backend.forget_session_checked(&session).await.unwrap();
        let live_waiters = cell.live_waiters.load(Ordering::Acquire);
        let release_gate = Arc::clone(&release);
        tokio::task::spawn_blocking(move || release_gate.wait())
            .await
            .unwrap();
        let updates = task.await.unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == "stop"
        ));
        assert_eq!(cleanup, BackendCleanupDispositionV1::Unknown);
        assert_eq!(live_waiters, 0);
        let inner = cell.inner.lock().unwrap();
        assert_eq!(
            inner.state,
            ApiRequestCleanupStateV1::TimedOut,
            "late Complete evidence must not erase timeout debt"
        );
        assert_eq!(
            inner.terminal,
            Some((ResourceActionDispositionV1::Complete, true))
        );
        drop(inner);
        assert!(!cell.reclaimable());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_e_timed_out_cleanup_recreation_keeps_successor_and_aggregates_old_unknown() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse()),
            )
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let route = request_journal_route(root.path());
        let arrived = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let publisher: Arc<dyn RemoteRequestResultPublisherV1> =
            Arc::new(BlockingRequestPublisher {
                arrived: Arc::clone(&arrived),
                release: Arc::clone(&release),
            });
        let driver = request_driver(&route, 64, publisher);
        let mut cfg = crate::config::ApiConfig::new(format!("{}/v1", server.uri()));
        cfg.request_timeout = Duration::from_secs(2);
        cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
            driver,
            NodeId::parse("api-node").unwrap(),
        ));
        let backend = Arc::new(ApiBackend::new(cfg));
        let session = SessionId::parse("task-e-recreate-after-timeout").unwrap();
        backend.attach_resource_flight_owner_v1(&session).unwrap();
        let first = spawn_drain(Arc::clone(&backend), session.clone());

        let arrived_gate = Arc::clone(&arrived);
        tokio::task::spawn_blocking(move || arrived_gate.wait())
            .await
            .unwrap();
        let old_cell = backend
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .unwrap()
            .cleanup_cell
            .clone()
            .unwrap();
        assert_eq!(
            backend.forget_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        assert_eq!(
            old_cell.inner.lock().unwrap().state,
            ApiRequestCleanupStateV1::TimedOut
        );

        backend.attach_resource_flight_owner_v1(&session).unwrap();
        let successor_turn = backend.begin_turn(&session).unwrap();
        let successor_cell = backend
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .unwrap()
            .cleanup_cell
            .clone()
            .unwrap();
        assert!(!Arc::ptr_eq(&old_cell, &successor_cell));

        let release_gate = Arc::clone(&release);
        tokio::task::spawn_blocking(move || release_gate.wait())
            .await
            .unwrap();
        let first_updates = first.await.unwrap();
        assert!(matches!(
            first_updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == "stop"
        ));
        let live_successor = backend
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .and_then(|state| state.cleanup_cell.clone())
            .expect("late old publisher must not remove the recreated session");
        assert!(Arc::ptr_eq(&live_successor, &successor_cell));
        assert_eq!(
            backend.forget_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Unknown,
            "the later cleanup must retain the old timeout debt"
        );
        drop(successor_turn);
    }

    #[tokio::test]
    async fn task_e_exact_echo_projects_complete_acknowledgement() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(stop_sse()),
            )
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let route = request_journal_route(root.path());
        let publisher = Arc::new(RecordingRequestPublisher::default());
        let publisher_port: Arc<dyn RemoteRequestResultPublisherV1> = publisher.clone();
        let driver = request_driver(&route, 64, publisher_port);
        let mut cfg = crate::config::ApiConfig::new(format!("{}/v1", server.uri()));
        cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
            driver,
            NodeId::parse("api-node").unwrap(),
        ));
        let backend = Arc::new(ApiBackend::new(cfg));
        let session = SessionId::parse("task-e-exact-publication").unwrap();
        backend.attach_resource_flight_owner_v1(&session).unwrap();

        let updates = spawn_drain(Arc::clone(&backend), session.clone())
            .await
            .unwrap();
        assert!(matches!(
            updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == "stop"
        ));
        assert_eq!(publisher.0.lock().unwrap().len(), 1);
        assert_eq!(
            backend.release_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Complete
        );
    }

    #[tokio::test]
    async fn task_e_exact_projection_and_deadline_leave_no_waiter() {
        let session = SessionId::parse("task-e-projection").unwrap();
        for v3 in [false, true] {
            let cell =
                ApiRequestCleanupCustodianV1::new(1, session.clone(), v3, Duration::from_secs(1));
            cell.begin_cleanup();
            assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Complete);
        }

        let pending = ApiRequestCleanupCustodianV1::new(2, session.clone(), true, Duration::ZERO);
        pending.begin_admission().unwrap();
        pending.begin_cleanup();
        assert_eq!(
            pending.observe().await,
            BackendCleanupDispositionV1::Unknown
        );
        assert_eq!(pending.live_waiters.load(Ordering::Acquire), 0);
        assert_eq!(
            pending.inner.lock().unwrap().state,
            ApiRequestCleanupStateV1::TimedOut
        );

        for (disposition, acknowledged, expected) in [
            (
                ResourceActionDispositionV1::Complete,
                true,
                BackendCleanupDispositionV1::Complete,
            ),
            (
                ResourceActionDispositionV1::Complete,
                false,
                BackendCleanupDispositionV1::Unknown,
            ),
            (
                ResourceActionDispositionV1::Partial,
                true,
                BackendCleanupDispositionV1::Unknown,
            ),
            (
                ResourceActionDispositionV1::Failed,
                true,
                BackendCleanupDispositionV1::Unknown,
            ),
            (
                ResourceActionDispositionV1::Unknown,
                true,
                BackendCleanupDispositionV1::Unknown,
            ),
        ] {
            let cell =
                ApiRequestCleanupCustodianV1::new(3, session.clone(), true, Duration::from_secs(1));
            let identity = ActiveRequestIdentity::Dedicated(request_id('e'));
            cell.begin_admission().unwrap();
            cell.bind(&identity).unwrap();
            assert!(cell.finish(&identity, disposition, acknowledged));
            cell.begin_cleanup();
            assert_eq!(cell.observe().await, expected);
        }

        let legacy = ApiRequestCleanupCustodianV1::new(4, session, false, Duration::from_secs(1));
        let identity = ActiveRequestIdentity::Legacy(1);
        legacy.begin_admission().unwrap();
        legacy.bind(&identity).unwrap();
        legacy.begin_cleanup();
        assert!(legacy.finish(&identity, ResourceActionDispositionV1::Complete, true));
        assert_eq!(legacy.observe().await, BackendCleanupDispositionV1::Unknown);
    }

    #[tokio::test]
    async fn task_e_admission_reset_state_table_is_closed_over_terminal_outcomes() {
        let session = SessionId::parse("task-e-admission-reset").unwrap();
        let complete =
            ApiRequestCleanupCustodianV1::new(1, session.clone(), true, Duration::from_secs(1));
        complete.finish_pending(ResourceActionDispositionV1::Complete, false);
        complete
            .begin_admission()
            .expect("Complete must re-admit even without acknowledgement");

        for disposition in [
            ResourceActionDispositionV1::Partial,
            ResourceActionDispositionV1::Failed,
            ResourceActionDispositionV1::Unknown,
        ] {
            let cell =
                ApiRequestCleanupCustodianV1::new(2, session.clone(), true, Duration::from_secs(1));
            cell.finish_pending(disposition.clone(), true);
            assert!(
                cell.begin_admission().is_err(),
                "{disposition:?} must not re-admit"
            );
        }

        let refused =
            ApiRequestCleanupCustodianV1::new(3, session.clone(), true, Duration::from_secs(1));
        refused.refuse(
            None,
            ApiRequestFlightErrorV1::Admission("injected refusal".into()),
            None,
        );
        assert!(
            refused.begin_admission().is_err(),
            "SettlementRefused must not re-admit"
        );

        let timed_out = ApiRequestCleanupCustodianV1::new(4, session, true, Duration::ZERO);
        timed_out.begin_admission().unwrap();
        timed_out.begin_cleanup();
        assert_eq!(
            timed_out.observe().await,
            BackendCleanupDispositionV1::Unknown
        );
        assert!(
            timed_out.begin_admission().is_err(),
            "TimedOut must not re-admit"
        );
    }

    #[tokio::test]
    async fn task_e_all_cleanup_surfaces_share_authority_and_completed_work_does_not_taint() {
        let backend = ApiBackend::new(crate::config::ApiConfig::new("http://127.0.0.1:1"));
        let session = SessionId::parse("task-e-surfaces").unwrap();
        let observer: Arc<dyn DiagnosticObserver> =
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default());
        assert_eq!(
            backend.forget_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Complete
        );
        let first = backend.begin_turn(&session).unwrap();
        assert_eq!(
            backend
                .forget_session_observed(&session, Arc::clone(&observer))
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Complete
        );
        let second = backend.begin_turn(&session).unwrap();
        assert_ne!(first.epoch, second.epoch);
        assert_eq!(
            backend.release_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Complete
        );
        assert_eq!(
            backend
                .release_session_observed(&session, observer)
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Complete
        );
        backend.forget_session(&session).await;
        backend.release_session(&session).await;
    }

    #[tokio::test]
    async fn task_e_drop_refusal_retains_acceptance_aware_diagnostic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('8')],
            64,
            4,
            None,
        );
        let session = SessionId::parse("task-e-drop-diagnostic").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let observer =
            Arc::new(bridge_core::diagnostics::InMemoryDiagnosticObserver::new(16).unwrap());
        let mut stream = fixture
            .backend
            .prompt_with_observers(
                &session,
                vec![Part { text: "hi".into() }],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), stream.next())
                .await
                .is_err()
        );
        std::fs::rename(
            &fixture.journal_root,
            fixture._root.path().join("journal-refusing-drop"),
        )
        .unwrap();
        drop(stream);
        assert_eq!(
            fixture
                .backend
                .forget_session_checked(&session)
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        assert!(observer.snapshot().await.iter().any(|event| event
            .failure()
            .is_some_and(|failure| failure.prompt_may_have_been_accepted())));
    }

    #[tokio::test]
    async fn task_e_expired_observation_retains_accepted_refusal_diagnostic() {
        let session = SessionId::parse("task-e-expired-diagnostic").unwrap();
        let observer = Arc::new(DiagnosticOutcomeObserver::new(
            DiagnosticRecordOutcome::Accept,
        ));
        let cell = refused_cleanup_cell(&session, Duration::ZERO, observer.clone());

        assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);
        assert_eq!(observer.calls.load(Ordering::SeqCst), 0);
        assert!(cell
            .inner
            .lock()
            .unwrap()
            .diagnostic
            .as_ref()
            .is_some_and(|(_, _, accepted)| *accepted));
        assert_eq!(cell.live_waiters.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn task_e_rejected_or_timed_out_diagnostic_recording_retains_custody() {
        let session = SessionId::parse("task-e-observer-refusal").unwrap();
        for outcome in [
            DiagnosticRecordOutcome::Reject,
            DiagnosticRecordOutcome::Stall,
        ] {
            let observer = Arc::new(DiagnosticOutcomeObserver::new(outcome));
            let cell = refused_cleanup_cell(&session, Duration::from_millis(20), observer.clone());

            assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);
            assert_eq!(observer.calls.load(Ordering::SeqCst), 1);
            assert!(cell.inner.lock().unwrap().diagnostic.is_some());
            assert_eq!(cell.live_waiters.load(Ordering::Acquire), 0);
        }
    }

    #[tokio::test]
    async fn task_e_timeout_then_drop_records_success_without_upgrading_unknown() {
        let root = tempfile::tempdir().unwrap();
        let session = SessionId::parse("task-e-timeout-drop-success").unwrap();
        let (identity, flight, _journal_root) = cleanup_request_flight(root.path(), &session);
        let cell = ApiRequestCleanupCustodianV1::new(92, session, true, Duration::ZERO);
        cell.begin_admission().unwrap();
        cell.bind(&identity).unwrap();
        cell.begin_cleanup();
        assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);

        cell.settle_drop(
            &identity,
            Some(flight),
            ResourceActionDispositionV1::Complete,
            None,
            true,
        );

        let inner = cell.inner.lock().unwrap();
        assert_eq!(inner.state, ApiRequestCleanupStateV1::TimedOut);
        assert!(inner.accepted);
        assert_eq!(
            inner.terminal,
            Some((ResourceActionDispositionV1::Complete, true))
        );
        drop(inner);
        assert_eq!(
            cell.projection(),
            Some(BackendCleanupDispositionV1::Unknown)
        );
    }

    #[tokio::test]
    async fn task_e_timeout_then_drop_records_refusal_without_upgrading_unknown() {
        let root = tempfile::tempdir().unwrap();
        let moved_root = root.path().join("journal-moved");
        let session = SessionId::parse("task-e-timeout-drop-refusal").unwrap();
        let (identity, flight, journal_root) = cleanup_request_flight(root.path(), &session);
        let observer = Arc::new(DiagnosticOutcomeObserver::new(
            DiagnosticRecordOutcome::Accept,
        ));
        let lifecycle = ApiLifecycle::new(observer, None);
        let cell = ApiRequestCleanupCustodianV1::new(93, session, true, Duration::ZERO);
        cell.begin_admission().unwrap();
        cell.bind(&identity).unwrap();
        cell.begin_cleanup();
        assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);
        std::fs::rename(&journal_root, moved_root).unwrap();

        cell.settle_drop(
            &identity,
            Some(flight),
            ResourceActionDispositionV1::Unknown,
            Some(lifecycle),
            true,
        );

        let inner = cell.inner.lock().unwrap();
        assert_eq!(inner.state, ApiRequestCleanupStateV1::TimedOut);
        assert!(inner.accepted);
        assert!(inner.terminal.is_none());
        assert!(inner
            .diagnostic
            .as_ref()
            .is_some_and(|(_, _, accepted)| *accepted));
        drop(inner);
        assert_eq!(
            cell.projection(),
            Some(BackendCleanupDispositionV1::Unknown)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_e_settlement_crossing_expiry_cannot_upgrade_timed_out() {
        let root = tempfile::tempdir().unwrap();
        let session = SessionId::parse("task-e-crossing-success").unwrap();
        let (identity, flight, _journal_root) = cleanup_request_flight(root.path(), &session);
        let cell = ApiRequestCleanupCustodianV1::new(94, session, true, Duration::from_millis(200));
        cell.begin_admission().unwrap();
        cell.bind(&identity).unwrap();
        cell.begin_cleanup();

        let arrived = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        {
            let arrived = Arc::clone(&arrived);
            let release = Arc::clone(&release);
            *cell.settle_drop_gate.lock().unwrap() = Some(Box::new(move || {
                arrived.wait();
                release.wait();
            }));
        }
        let settler = {
            let cell = Arc::clone(&cell);
            let identity = identity.clone();
            std::thread::spawn(move || {
                cell.settle_drop(
                    &identity,
                    Some(flight),
                    ResourceActionDispositionV1::Complete,
                    None,
                    true,
                );
            })
        };
        let arrived_gate = Arc::clone(&arrived);
        tokio::task::spawn_blocking(move || arrived_gate.wait())
            .await
            .unwrap();
        // The settlement is stalled between its pre-settlement snapshot and
        // the durable settle; observation now expires and takes TimedOut.
        assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);
        assert_eq!(
            cell.inner.lock().unwrap().state,
            ApiRequestCleanupStateV1::TimedOut
        );
        let release_gate = Arc::clone(&release);
        tokio::task::spawn_blocking(move || release_gate.wait())
            .await
            .unwrap();
        settler.join().unwrap();

        let inner = cell.inner.lock().unwrap();
        assert_eq!(
            inner.state,
            ApiRequestCleanupStateV1::TimedOut,
            "a settlement that began before expiry must not overwrite TimedOut"
        );
        assert_eq!(
            inner.terminal,
            Some((ResourceActionDispositionV1::Complete, true))
        );
        drop(inner);
        assert_eq!(
            cell.projection(),
            Some(BackendCleanupDispositionV1::Unknown)
        );
        assert!(!cell.reclaimable());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_e_settlement_refusal_crossing_expiry_retains_flight_custody() {
        let root = tempfile::tempdir().unwrap();
        let moved_root = root.path().join("journal-moved");
        let session = SessionId::parse("task-e-crossing-refusal").unwrap();
        let (identity, flight, journal_root) = cleanup_request_flight(root.path(), &session);
        let observer = Arc::new(DiagnosticOutcomeObserver::new(
            DiagnosticRecordOutcome::Accept,
        ));
        let lifecycle = ApiLifecycle::new(observer, None);
        let cell = ApiRequestCleanupCustodianV1::new(95, session, true, Duration::from_millis(200));
        cell.begin_admission().unwrap();
        cell.bind(&identity).unwrap();
        cell.mark_accepted(&identity);
        cell.begin_cleanup();

        let arrived = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        {
            let arrived = Arc::clone(&arrived);
            let release = Arc::clone(&release);
            *cell.settle_drop_gate.lock().unwrap() = Some(Box::new(move || {
                arrived.wait();
                release.wait();
            }));
        }
        let settler = {
            let cell = Arc::clone(&cell);
            let identity = identity.clone();
            std::thread::spawn(move || {
                cell.settle_drop(
                    &identity,
                    Some(flight),
                    ResourceActionDispositionV1::Unknown,
                    Some(lifecycle),
                    true,
                );
            })
        };
        let arrived_gate = Arc::clone(&arrived);
        tokio::task::spawn_blocking(move || arrived_gate.wait())
            .await
            .unwrap();
        assert_eq!(cell.observe().await, BackendCleanupDispositionV1::Unknown);
        assert_eq!(
            cell.inner.lock().unwrap().state,
            ApiRequestCleanupStateV1::TimedOut
        );
        // The settlement that is about to resume must refuse: its journal
        // root disappears while it is still stalled pre-settle.
        std::fs::rename(&journal_root, &moved_root).unwrap();
        let release_gate = Arc::clone(&release);
        tokio::task::spawn_blocking(move || release_gate.wait())
            .await
            .unwrap();
        settler.join().unwrap();

        let inner = cell.inner.lock().unwrap();
        assert_eq!(
            inner.state,
            ApiRequestCleanupStateV1::TimedOut,
            "a refusal that crossed expiry must not overwrite TimedOut"
        );
        assert!(inner.terminal.is_none());
        assert!(inner
            .diagnostic
            .as_ref()
            .is_some_and(|(_, _, accepted)| *accepted));
        assert!(
            inner.retained_late_flight.is_some(),
            "the refused late flight must stay in the custodian so its drop never retries"
        );
        drop(inner);
        assert_eq!(
            cell.projection(),
            Some(BackendCleanupDispositionV1::Unknown)
        );
    }

    #[test]
    fn task_e_cleanup_cell_has_the_exact_closed_state_set() {
        use ApiRequestCleanupStateV1::*;
        let states = [
            AdmissionPendingLegacy,
            AdmissionPendingV3,
            ActiveLegacy,
            ActiveV3,
            DropOwned,
            Terminal,
            SettlementRefused,
            TimedOut,
        ];
        assert_eq!(states.len(), 8);
    }
}
