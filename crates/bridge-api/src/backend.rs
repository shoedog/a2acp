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
    AgentBackend, BackendObservers, BackendResourceFlightV1, BackendStream, DiagnosticObserver,
    PolicyEngine, RichEventSink, Update, STOP_REASON_CANCELLED,
};
use bridge_core::process::{DurableRemoteRequestFlightV3, RemoteRequestFlightErrorV1};
use bridge_core::provider::ProviderEvidence;
use bridge_core::resource_flight::{
    DedicatedRemoteRequestIdV1, ResourceActionDispositionV1, ResourceActionResultV1,
};
use bridge_core::retained_resource_flight::ResourceFlightOwnerV1;
use futures::StreamExt;
use std::collections::HashMap;
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
        class: DiagnosticFailureClass,
        code: &'static str,
        summary: &'static str,
        cause: Option<String>,
        retry_after_ms: Option<u64>,
        reset_at_ms: Option<i64>,
    ) -> BridgeError {
        let failure = match FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: DiagnosticPhase::PromptStream,
                last_completed_phase: Some(DiagnosticPhase::PromptStart),
                class,
                disposition: FailureDisposition::Fatal,
                code: String::new(),
                summary: summary.to_owned(),
                causes: cause.into_iter().collect(),
                stderr_observed: false,
                stderr_line_count: 0,
                stderr_scope: None,
                stderr_tail: None,
                stderr_redaction: None,
                retry_after_ms,
                reset_at_ms,
                prompt_may_have_been_accepted: true,
            },
            code,
            &self.redactor,
        ) {
            Ok(failure) => failure,
            Err(_) => return BridgeError::InvalidStateTransition,
        };
        let transition = match PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase: DiagnosticPhase::PromptStream,
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
            Err(_) => return BridgeError::InvalidStateTransition,
        };
        let event =
            match bridge_core::diagnostics::DiagnosticEvent::new(transition, Some(failure.clone()))
            {
                Ok(event) => event,
                Err(_) => return BridgeError::InvalidStateTransition,
            };
        match self.observer.record(event).await {
            Ok(()) => BridgeError::agent_failure(failure),
            Err(error) => error,
        }
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

pub trait RemoteRequestIdSource: Send + Sync {
    fn mint(&self) -> Result<DedicatedRemoteRequestIdV1, BridgeError>;
}

struct SystemRemoteRequestIdSource;
impl RemoteRequestIdSource for SystemRemoteRequestIdSource {
    fn mint(&self) -> Result<DedicatedRemoteRequestIdV1, BridgeError> {
        DedicatedRemoteRequestIdV1::mint()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveRequestIdentity {
    Legacy(u64),
    Dedicated(DedicatedRemoteRequestIdV1),
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
    next_turn_epoch: u64,
    current_turn_epoch: Option<u64>,
    cancelled_turn_epoch: Option<u64>,
    next_legacy_request: u64,
    active_request: Option<ActiveRequestSlot>,
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
            next_turn_epoch: 0,
            current_turn_epoch: None,
            cancelled_turn_epoch: None,
            next_legacy_request: 0,
            active_request: None,
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
    flight: Option<DurableRemoteRequestFlightV3>,
    dispatched: bool,
}

impl RequestScope {
    fn begin_dispatch(&mut self) -> Result<(), BridgeError> {
        if let Some(flight) = &mut self.flight {
            flight.begin_dispatch().map_err(request_flight_error)?;
        }
        self.dispatched = true;
        Ok(())
    }

    fn settle(
        mut self,
        disposition: ResourceActionDispositionV1,
    ) -> Result<ResourceActionResultV1, BridgeError> {
        let result = match &mut self.flight {
            Some(flight) => flight.settle(disposition).map_err(request_flight_error)?,
            None => ResourceActionResultV1 {
                disposition,
                duration_ms: 0,
                recovery_owner: None,
                cause: None,
            },
        };
        self.flight = None;
        self.cancel.clear_exact();
        Ok(result)
    }
}

impl Drop for RequestScope {
    fn drop(&mut self) {
        if let Some(flight) = &mut self.flight {
            let disposition = if !self.dispatched {
                ResourceActionDispositionV1::Failed
            } else if *self.cancel_control.borrow() {
                ResourceActionDispositionV1::Partial
            } else {
                ResourceActionDispositionV1::Unknown
            };
            let _ = flight.settle(disposition);
        }
        self.cancel.clear_exact();
    }
}

fn settle_request_scope(
    scope: &mut Option<RequestScope>,
    disposition: ResourceActionDispositionV1,
) -> Result<ResourceActionResultV1, BridgeError> {
    scope
        .take()
        .ok_or(BridgeError::InvalidStateTransition)?
        .settle(disposition)
}

enum PreparedRequest {
    Ready {
        scope: RequestScope,
        cancel_rx: watch::Receiver<bool>,
    },
    TurnCancelled,
}

#[derive(Clone)]
struct RequestAdmission {
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    route: Option<ApiResourceFlightRouteV3>,
    request_ids: Arc<dyn RemoteRequestIdSource>,
}

impl RequestAdmission {
    fn prepare(
        &self,
        session: &SessionId,
        turn_epoch: u64,
    ) -> Result<PreparedRequest, BridgeError> {
        // First reject a cancellation already linearized in the between-round
        // gap. No request identity or flight is minted in that case. Admission
        // is checked again after durable work and before publication to close
        // the race in the opposite direction.
        {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| BridgeError::InvalidStateTransition)?;
            let Some(state) = sessions.get(session) else {
                return Ok(PreparedRequest::TurnCancelled);
            };
            if state.current_turn_epoch != Some(turn_epoch)
                || state.cancelled_turn_epoch == Some(turn_epoch)
            {
                return Ok(PreparedRequest::TurnCancelled);
            }
            if state.active_request.is_some() {
                return Err(BridgeError::InvalidStateTransition);
            }
        }

        let mut flight = match &self.route {
            Some(route) => {
                let request_id = self.request_ids.mint()?;
                let owner = ResourceFlightOwnerV1::new(route.node_id.clone(), session.as_str())
                    .map_err(|error| BridgeError::agent_crashed(error.to_string()))?;
                Some(
                    route
                        .attempt
                        .bind_remote_request(request_id, owner)
                        .map_err(request_flight_error)?,
                )
            }
            None => None,
        };

        let active_conflict = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| BridgeError::InvalidStateTransition)?;
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
                            state.next_legacy_request = state
                                .next_legacy_request
                                .checked_add(1)
                                .ok_or(BridgeError::InvalidStateTransition)?;
                            ActiveRequestIdentity::Legacy(state.next_legacy_request)
                        }
                    };
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
                        identity,
                    };
                    return Ok(PreparedRequest::Ready {
                        scope: RequestScope {
                            cancel,
                            cancel_control,
                            flight,
                            dispatched: false,
                        },
                        cancel_rx,
                    });
                }
            }
        };

        // Publication lost its race with cancellation, forget, or another
        // request. Settle outside the session lock so node aggregation may
        // re-enter unrelated bridge state without deadlocking this session.
        if let Some(flight) = &mut flight {
            flight
                .settle(ResourceActionDispositionV1::Failed)
                .map_err(request_flight_error)?;
        }
        if active_conflict {
            Err(BridgeError::InvalidStateTransition)
        } else {
            Ok(PreparedRequest::TurnCancelled)
        }
    }
}

fn request_flight_error(error: RemoteRequestFlightErrorV1) -> BridgeError {
    BridgeError::agent_crashed(error.to_string())
}

pub struct ApiBackend {
    cfg: ApiConfig,
    client: reqwest::Client,
    policy: Arc<StdMutex<Arc<dyn PolicyEngine>>>,
    sessions: Arc<StdMutex<HashMap<SessionId, SessionState>>>,
    request_ids: Arc<dyn RemoteRequestIdSource>,
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
            request_ids: Arc::new(SystemRemoteRequestIdSource),
        }
    }

    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() {
            *p = policy;
        }
        self
    }

    #[must_use]
    pub fn with_request_id_source(mut self, source: Arc<dyn RemoteRequestIdSource>) -> Self {
        self.request_ids = source;
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
        state.next_turn_epoch = state
            .next_turn_epoch
            .checked_add(1)
            .ok_or(BridgeError::InvalidStateTransition)?;
        let epoch = state.next_turn_epoch;
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
            request_ids: Arc::clone(&self.request_ids),
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

            // This operation-scoped acceptance barrier is crossed immediately
            // before the first HTTP send future is installed. It is deliberately
            // never cleared between tool rounds: once any request may have reached
            // the provider, every later failure is fatal and non-replayable.
            let mut acceptance_barrier_crossed = false;
            for round in 0..max_rounds {
                let PreparedRequest::Ready {
                    scope,
                    mut cancel_rx,
                } = request_admission.prepare(&session, turn_epoch)? else {
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                };
                let mut scope = Some(scope);
                // Durable reservation, owner attachment, identity evidence,
                // intent, and dispatch all precede installation of the POST future.
                scope
                    .as_mut()
                    .ok_or(BridgeError::InvalidStateTransition)?
                    .begin_dispatch()?;
                let req = ChatRequest { model: model.clone(), messages: messages.clone(),
                    tools: vec![crate::tool::tool_def()], stream: do_stream };
                let mut builder = client.post(&url).json(&req);
                if let Some(k) = &api_key { builder = builder.bearer_auth(k); }
                let send = if acceptance_barrier_crossed {
                    builder.send()
                } else {
                    acceptance_barrier_crossed = true;
                    install_first_send(&lifecycle, || builder.send()).await?
                };
                if *cancel_rx.borrow() {
                    settle_request_scope(&mut scope, ResourceActionDispositionV1::Partial)?;
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                }
                tokio::pin!(send);
                let send_result = loop {
                    tokio::select! {
                        biased;
                        changed = cancel_rx.changed() => {
                            if changed.is_ok() && *cancel_rx.borrow() {
                                break None;
                            }
                        }
                        result = &mut send => break Some(result),
                    }
                };
                let Some(send_result) = send_result else {
                    settle_request_scope(&mut scope, ResourceActionDispositionV1::Partial)?;
                    complete_prompt_lifecycle(&lifecycle).await?;
                    yield Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.into(),
                        prefix_attestation: Default::default(),
                    };
                    return;
                };
                let resp = match send_result {
                    Ok(response) => response,
                    Err(error) => {
                        let (class, code, summary) = request_failure(&error, "api.prompt.send");
                        settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                        Err(lifecycle
                            .failure(
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
                        settle_request_scope(&mut scope, ResourceActionDispositionV1::Partial)?;
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
                            settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                            Err(lifecycle
                                .failure(
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
                    settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                    Err(lifecycle
                        .failure(
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
                                settle_request_scope(
                                    &mut scope,
                                    ResourceActionDispositionV1::Partial,
                                )?;
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
                                settle_request_scope(
                                    &mut scope,
                                    ResourceActionDispositionV1::Failed,
                                )?;
                                Err(lifecycle
                                    .failure(
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
                                    settle_request_scope(
                                        &mut scope,
                                        ResourceActionDispositionV1::Failed,
                                    )?;
                                    Err(lifecycle
                                        .failure(
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
                                settle_request_scope(
                                    &mut scope,
                                    ResourceActionDispositionV1::Failed,
                                )?;
                                Err(lifecycle
                                    .failure(
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
                        settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                        Err(lifecycle
                            .failure(
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
                        settle_request_scope(&mut scope, ResourceActionDispositionV1::Partial)?;
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
                            settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                            Err(lifecycle
                                .failure(
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
                            settle_request_scope(&mut scope, ResourceActionDispositionV1::Failed)?;
                            Err(lifecycle
                                .failure(
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
                settle_request_scope(&mut scope, ResourceActionDispositionV1::Complete)?;
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
            if !acceptance_barrier_crossed {
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
        if let Ok(mut map) = self.sessions.lock() {
            if let Some(state) = map.get_mut(session) {
                if let Some(epoch) = state.current_turn_epoch {
                    state.cancelled_turn_epoch = Some(epoch);
                }
                if let Some(active) = &state.active_request {
                    let _ = active.cancel_control.send(true);
                }
            }
            map.remove(session);
        }
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
    use bridge_core::ids::{AttemptId, NodeId, SessionId};
    use bridge_core::ports::{AgentBackend, DiagnosticObserver, PolicyEngine};
    use bridge_core::process::DurableProcessFlightAttemptV3;
    use bridge_core::resource_flight::{
        FileResourceFlightJournal, NodeCleanupAggregationV1, ResourceFlightJournal,
        ResourceFlightJournalEventV1, ResourceFlightKeyV1, ResourceFlightResultPublisher,
    };
    use std::collections::VecDeque;
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

    struct SequenceRequestIds {
        ids: StdMutex<VecDeque<DedicatedRemoteRequestIdV1>>,
        minted: AtomicUsize,
    }

    impl SequenceRequestIds {
        fn new(ids: impl IntoIterator<Item = DedicatedRemoteRequestIdV1>) -> Self {
            Self {
                ids: StdMutex::new(ids.into_iter().collect()),
                minted: AtomicUsize::new(0),
            }
        }
    }

    impl RemoteRequestIdSource for SequenceRequestIds {
        fn mint(&self) -> Result<DedicatedRemoteRequestIdV1, BridgeError> {
            self.minted.fetch_add(1, Ordering::SeqCst);
            self.ids
                .lock()
                .map_err(|_| BridgeError::IdentityUnavailable)?
                .pop_front()
                .ok_or(BridgeError::IdentityUnavailable)
        }
    }

    #[derive(Default)]
    struct RecordingRequestPublisher(StdMutex<Vec<NodeCleanupAggregationV1>>);

    impl ResourceFlightResultPublisher for RecordingRequestPublisher {
        fn publish(&self, aggregation: NodeCleanupAggregationV1) {
            self.0.lock().unwrap().push(aggregation);
        }
    }

    struct ProtectedBackendFixture {
        backend: Arc<ApiBackend>,
        ids: Arc<SequenceRequestIds>,
        journal: Arc<FileResourceFlightJournal>,
        publisher: Arc<RecordingRequestPublisher>,
        _root: tempfile::TempDir,
        journal_root: PathBuf,
    }

    fn protected_backend(
        base_url: String,
        request_ids: Vec<DedicatedRemoteRequestIdV1>,
        journal_cap: usize,
        max_tool_rounds: usize,
        policy: Option<Arc<dyn PolicyEngine>>,
    ) -> ProtectedBackendFixture {
        let root = tempfile::tempdir().unwrap();
        let journal_root = root.path().join("journal");
        std::fs::create_dir(&journal_root).unwrap();
        let journal =
            Arc::new(FileResourceFlightJournal::open(&journal_root, journal_cap).unwrap());
        let publisher = Arc::new(RecordingRequestPublisher::default());
        let publisher_port: Arc<dyn ResourceFlightResultPublisher> = publisher.clone();
        let attempt =
            DurableProcessFlightAttemptV3::new(AttemptId::mint().unwrap(), Arc::clone(&journal))
                .with_result_publisher(publisher_port);
        let ids = Arc::new(SequenceRequestIds::new(request_ids));
        let request_id_source: Arc<dyn RemoteRequestIdSource> = ids.clone();
        let mut cfg = crate::config::ApiConfig::new(base_url);
        cfg.max_tool_rounds = max_tool_rounds;
        cfg.resource_flight_route_v3 = Some(ApiResourceFlightRouteV3::new(
            Arc::new(attempt),
            NodeId::parse("api-node").unwrap(),
        ));
        let backend = ApiBackend::new(cfg).with_request_id_source(request_id_source);
        let backend = match policy {
            Some(policy) => backend.with_policy(policy),
            None => backend,
        };
        ProtectedBackendFixture {
            backend: Arc::new(backend),
            ids,
            journal,
            publisher,
            journal_root,
            _root: root,
        }
    }

    async fn wait_for_active_request(
        backend: &Arc<ApiBackend>,
        session: &SessionId,
        expected: &ActiveRequestIdentity,
    ) -> RequestCancelCapability {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let capability = {
                    let sessions = backend.sessions.lock().unwrap();
                    sessions.get(session).and_then(|state| {
                        state.active_request.as_ref().and_then(|active| {
                            (active.identity == *expected).then(|| RequestCancelCapability {
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

        let id_a = request_id('1');
        let id_b = request_id('2');
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![id_a.clone(), id_b.clone()],
            64,
            4,
            None,
        );
        let session = SessionId::parse("stale-round").unwrap();
        assert_eq!(
            fixture
                .backend
                .attach_resource_flight_owner_v1(&session)
                .unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        let stale_a = wait_for_active_request(
            &fixture.backend,
            &session,
            &ActiveRequestIdentity::Dedicated(id_a.clone()),
        )
        .await;
        let _current_b = wait_for_active_request(
            &fixture.backend,
            &session,
            &ActiveRequestIdentity::Dedicated(id_b.clone()),
        )
        .await;

        assert!(
            !stale_a.cancel_exact(),
            "a retained A sender must refuse successor B"
        );
        assert!(
            !stale_a.clear_exact(),
            "a retained A settlement/drop must not clear successor B"
        );
        assert_eq!(
            exact_request_cancelled(
                &fixture.backend,
                &session,
                &ActiveRequestIdentity::Dedicated(id_b.clone())
            ),
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

        let aggregations = fixture.publisher.0.lock().unwrap().clone();
        assert_eq!(aggregations.len(), 2);
        assert_ne!(
            aggregations[0].resource_flight_id,
            aggregations[1].resource_flight_id
        );
        assert_eq!(aggregations[0].owner, aggregations[1].owner);
        assert_eq!(
            aggregations[0].owner.node_id,
            NodeId::parse("api-node").unwrap()
        );
        assert_eq!(aggregations[0].owner.owner_key, session.as_str());
        assert_eq!(
            aggregations[0].result.disposition,
            ResourceActionDispositionV1::Complete
        );
        assert_eq!(
            aggregations[1].result.disposition,
            ResourceActionDispositionV1::Partial
        );

        for (aggregation, expected) in aggregations.iter().zip([id_a, id_b]) {
            let rows = fixture
                .journal
                .records(&aggregation.resource_flight_id)
                .unwrap();
            assert_eq!(
                rows.iter()
                    .filter(|row| matches!(
                        &row.event,
                        ResourceFlightJournalEventV1::Settled { .. }
                    ))
                    .count(),
                1
            );
            assert!(rows.iter().any(|row| matches!(
                &row.event,
                ResourceFlightJournalEventV1::FlightReserved {
                    key: ResourceFlightKeyV1::DedicatedRemoteRequest { request_id },
                    ..
                } if request_id == &expected
            )));
            assert!(rows.iter().any(|row| matches!(
                &row.event,
                ResourceFlightJournalEventV1::RemoteRequestIdentityCaptured {
                    identity: bridge_core::resource_flight::ResourceIdentityV1::DedicatedRemoteRequest {
                        request_id,
                    },
                    ..
                } if request_id == &expected
            )));
        }
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
            vec![request_id('3'), request_id('4')],
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
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 1);
        let aggregations = fixture.publisher.0.lock().unwrap();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(
            aggregations[0].result.disposition,
            ResourceActionDispositionV1::Complete
        );
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
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('9'), request_id('a')],
            64,
            4,
            None,
        );
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
        assert!(updates.iter().any(Result::is_err));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 2);
        let aggregations = fixture.publisher.0.lock().unwrap();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(
            aggregations[0].result.disposition,
            ResourceActionDispositionV1::Complete
        );
        let moved_journal = FileResourceFlightJournal::open(moved_root, 64).unwrap();
        let rows = moved_journal
            .records(&aggregations[0].resource_flight_id)
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(&row.event, ResourceFlightJournalEventV1::Settled { .. }))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn request_capacity_refusal_precedes_flight_creation_and_post() {
        let server = MockServer::start().await;
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![request_id('5')],
            4,
            4,
            None,
        );
        let session = SessionId::parse("capacity-refusal").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let updates = spawn_drain(Arc::clone(&fixture.backend), session)
            .await
            .unwrap();
        assert!(updates.iter().any(Result::is_err));
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 1);
        assert!(
            fixture.publisher.0.lock().unwrap().is_empty(),
            "capacity is reserved by FlightReserved, so refusal creates no live flight"
        );
    }

    #[tokio::test]
    async fn round_two_identity_collision_does_not_post_or_rewrite_round_one() {
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
        let duplicate = request_id('6');
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![duplicate.clone(), duplicate],
            64,
            4,
            None,
        );
        let session = SessionId::parse("round-two-refusal").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let updates = spawn_drain(Arc::clone(&fixture.backend), session)
            .await
            .unwrap();
        assert!(updates.iter().any(Result::is_err));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 2);
        let aggregations = fixture.publisher.0.lock().unwrap();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(
            aggregations[0].result.disposition,
            ResourceActionDispositionV1::Complete
        );
        let rows = fixture
            .journal
            .records(&aggregations[0].resource_flight_id)
            .unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(&row.event, ResourceFlightJournalEventV1::Settled { .. }))
                .count(),
            1
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
        let id = request_id('7');
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![id.clone()],
            64,
            4,
            None,
        );
        let session = SessionId::parse("consumer-drop").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        wait_for_active_request(
            &fixture.backend,
            &session,
            &ActiveRequestIdentity::Dedicated(id),
        )
        .await;
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
        let aggregations = fixture.publisher.0.lock().unwrap();
        assert_eq!(aggregations.len(), 1);
        assert_eq!(
            aggregations[0].result.disposition,
            ResourceActionDispositionV1::Unknown
        );
    }

    #[tokio::test]
    async fn forget_session_cancels_and_settles_the_exact_request_once() {
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
        let id = request_id('c');
        let fixture = protected_backend(
            format!("{}/v1", server.uri()),
            vec![id.clone()],
            64,
            4,
            None,
        );
        let session = SessionId::parse("forget-exact").unwrap();
        fixture
            .backend
            .attach_resource_flight_owner_v1(&session)
            .unwrap();
        let task = spawn_drain(Arc::clone(&fixture.backend), session.clone());
        wait_for_active_request(
            &fixture.backend,
            &session,
            &ActiveRequestIdentity::Dedicated(id),
        )
        .await;
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if server.received_requests().await.unwrap().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("provider did not receive the request before forget");
        fixture.backend.forget_session(&session).await;

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
            let aggregations = fixture.publisher.0.lock().unwrap();
            assert_eq!(aggregations.len(), 1);
            assert_eq!(
                aggregations[0].result.disposition,
                ResourceActionDispositionV1::Partial
            );
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
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
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 0);
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
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 0);
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
        assert_eq!(fixture.ids.minted.load(Ordering::SeqCst), 0);
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
        let stale =
            wait_for_active_request(&backend, &session, &ActiveRequestIdentity::Legacy(1)).await;
        backend.cancel(&session).await.unwrap();
        let first_updates = first.await.unwrap();
        assert!(matches!(
            first_updates.last(),
            Some(Ok(Update::Done { stop_reason, .. })) if stop_reason == STOP_REASON_CANCELLED
        ));

        let second = spawn_drain(Arc::clone(&backend), session.clone());
        wait_for_active_request(&backend, &session, &ActiveRequestIdentity::Legacy(2)).await;
        assert!(!stale.cancel_exact());
        assert!(!stale.clear_exact());
        assert_eq!(
            exact_request_cancelled(&backend, &session, &ActiveRequestIdentity::Legacy(2)),
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
}
