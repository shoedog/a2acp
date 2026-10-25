use std::collections::HashMap;
use std::sync::Arc;

use bridge_core::domain::{EffectiveConfig, QueuedInject};
use bridge_core::error::{warm_session_survivability, BridgeError, WarmSessionSurvivability};
use bridge_core::ids::{ContextId, OperationId, SessionGeneration, SessionId, TaskId};
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{AgentBackend, BackendCleanupDispositionV1, DiagnosticObserver, Lease};
use tokio::sync::Mutex;

use crate::session_manager::{ExpiryClaim, SessionManager, WarmExpiryIntent};

/// A task's binding to its resolved registry instance, created on the FIRST local
/// message. Holds the backend driving the task, the effective config applied to its
/// session, and the registry [`Lease`] keeping the slot alive for the task's
/// lifetime. Eviction removes the map's lease reference; active holders retain
/// the slot and configuration stash until their backend work finishes.
pub struct TaskBinding {
    /// Opaque backend session identity owned by this cold binding incarnation.
    pub session: SessionId,
    pub agent: bridge_core::ids::AgentId,
    pub backend: Arc<dyn AgentBackend>,
    /// The effective config applied to the task's session — reused by binding-driven
    /// follow-ups (they prompt the bound backend without recomputing config). Kept on
    /// the binding so the resolved config is available for the task's whole lifetime.
    pub eff: EffectiveConfig,
    /// The registry lease keeping the slot alive for the task. The binding's reference is
    /// dropped when the binding is removed on producer exit; anything else still driving the
    /// bound backend (a follow-up turn, a cancel) co-holds a clone, so the slot's active-task
    /// count only reaches zero once the backend is truly unused.
    pub lease: Arc<dyn Lease>,
}

/// Shared cold-incarnation custody. Eviction stops new follow-ups immediately;
/// the final producer/cancel holder forgets the stash after its backend work ends.
/// Keep the registry lease through forgetting so retirement cannot race cleanup.
pub struct ColdBindingLease {
    backend: Arc<dyn AgentBackend>,
    session: SessionId,
    lease: Arc<dyn Lease>,
}

impl ColdBindingLease {
    pub fn new(backend: Arc<dyn AgentBackend>, session: SessionId, lease: Arc<dyn Lease>) -> Self {
        Self {
            backend,
            session,
            lease,
        }
    }
}

impl Lease for ColdBindingLease {
    fn is_retired(&self) -> bool {
        self.lease.is_retired()
    }
}

// BindingGuard's existing backend slot co-holds the same owner as the turn leases.
// Its terminal forget cannot clear the stash while another holder still needs it.
#[async_trait::async_trait]
impl AgentBackend for ColdBindingLease {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> Result<bridge_core::ports::BackendStream, BridgeError> {
        self.backend.prompt(session, parts).await
    }
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.backend.cancel(session).await
    }
    async fn forget_session(&self, _: &SessionId) {
        // The final Arc drop performs the one underlying forget below.
    }
}

impl Drop for ColdBindingLease {
    fn drop(&mut self) {
        let (backend, session, lease) = (
            self.backend.clone(),
            self.session.clone(),
            self.lease.clone(),
        );
        tokio::spawn(async move {
            backend.forget_session(&session).await;
            drop(lease);
        });
    }
}

/// RAII eviction guard owned by a task's producer. While alive it represents the
/// task's binding; on `Drop` — whether the producer returns cleanly (Done/Failed/
/// Canceled) OR early (client disconnect / error) — it removes the [`TaskBinding`]
/// from the map and relinquishes its shared [`ColdBindingLease`]. The stash is
/// forgotten once all producers and cancels using this incarnation have finished.
/// This is the spec-critical
/// "eviction on EVERY producer exit": a leaked lease keeps a slot un-retirable
/// forever, so the guard must fire on the non-clean paths a manual cleanup might miss.
///
/// `Drop` is synchronous but eviction and final forgetting are async,
/// so it is performed on a spawned task. A follow-up that REUSES an existing binding
/// does NOT own a guard — only the FIRST message's producer evicts.
pub struct BindingGuard {
    pub bindings: Arc<Mutex<HashMap<TaskId, TaskBinding>>>,
    pub task: TaskId,
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
}

impl Drop for BindingGuard {
    fn drop(&mut self) {
        let bindings = self.bindings.clone();
        let task = self.task.clone();
        let session = self.session.clone();
        let backend = self.backend.clone();
        // Note: the spawn-in-Drop pattern means an eviction enqueued during runtime
        // shutdown may not run (the Tokio runtime may be torn down before the task
        // executes), leaving the binding and lease un-evicted. This is acceptable for
        // a single-process bridge that is exiting anyway.
        tokio::spawn(async move {
            // Evict only this incarnation. A delayed older producer must leave a
            // successor's binding and lease intact; its stash has a distinct key.
            let mut bindings = bindings.lock().await;
            if bindings.get(&task).is_some_and(|b| b.session == session) {
                bindings.remove(&task);
            }
            drop(bindings);
            backend.forget_session(&session).await;
        });
    }
}

/// The local backend ready to drive a task, plus its RAII eviction guard. A
/// FIRST-message dispatch returns `Some(guard)` (the producer owns it → evicts the
/// binding/lease/stash on exit); a FOLLOW-UP that reused an existing binding returns
/// `None` (the original producer owns eviction — a follow-up must not evict a still-
/// live binding when its own short-lived call ends).
pub struct LocalDispatch {
    pub usage: Option<bridge_core::translator::PreparedAccountingTurnV1>,
    pub backend: Arc<dyn AgentBackend>,
    /// The session to prompt against — warm `ctx-…` session, or an opaque cold binding key.
    pub session: SessionId,
    /// Warm-session summary seed to prepend to the prompt parts, when present.
    pub seed: Option<String>,
    /// Warm-session context injects to apply to the next prompt parts.
    pub injects: Vec<QueuedInject>,
    /// Warm path only: bridge turn identity stashed into ACP before the next prompt.
    pub turn_meta: Option<TurnMeta>,
    pub guard: Option<BindingGuard>,
    /// Binding path only: a clone of the task binding's lease, held by the producer for the
    /// whole turn so the backend stays leased (never idle-retired) even if the binding is
    /// evicted mid-turn. `None` on the warm path, whose handle holds its own lease.
    pub lease: Option<Arc<dyn Lease>>,
    /// Warm path only: finishes the warm turn (→ Idle) on drop. Mutually exclusive with `guard`.
    pub warm_guard: Option<WarmTurnGuard>,
    /// Per-turn abort token (cancel-tokens F2). For a WARM turn it is the handle's `turn_abort`
    /// (a force-reset cancels it → the producer's biased select aborts before re-minting the released
    /// session). For a cold-bind dispatch it is a fresh, never-cancelled token (no warm handle to race).
    pub abort: tokio_util::sync::CancellationToken,
    /// Per-turn observability context carried to observer sinks.
    pub obs_ctx: bridge_core::ports::TurnContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WarmCompletionAction {
    Finish,
    Cancel,
    Expire,
}

#[derive(Clone, Copy)]
enum LegacyCrashAction {
    Finish,
    Expire,
}

pub enum WarmCompletionExit<'a> {
    Normal,
    Canceled,
    Error(&'a BridgeError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetachedCleanupDisposition {
    Complete,
    Retained,
    Preserved,
    Unknown,
    Failed,
    OwnerHeld,
}

impl DetachedCleanupDisposition {
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Complete => Some("complete"),
            Self::Retained => Some("retained"),
            Self::Preserved => Some("preserved"),
            Self::Unknown => Some("unknown"),
            Self::Failed => Some("failed"),
            Self::OwnerHeld => None,
        }
    }
}

impl From<BackendCleanupDispositionV1> for DetachedCleanupDisposition {
    fn from(value: BackendCleanupDispositionV1) -> Self {
        match value {
            BackendCleanupDispositionV1::Complete => Self::Complete,
            BackendCleanupDispositionV1::Retained => Self::Retained,
            BackendCleanupDispositionV1::Preserved => Self::Preserved,
            BackendCleanupDispositionV1::Unknown => Self::Unknown,
        }
    }
}

/// Observable custody for one exact cleanup flight. Constructing this value
/// transfers the warm claim before durable terminal publication; awaiting it is
/// deliberately separate from response delivery.
pub struct DetachedWarmCleanup {
    task: Option<tokio::task::JoinHandle<Result<BackendCleanupDispositionV1, BridgeError>>>,
    immediate: Option<DetachedCleanupDisposition>,
}

impl DetachedWarmCleanup {
    pub async fn settle(self) -> DetachedCleanupDisposition {
        if let Some(disposition) = self.immediate {
            return disposition;
        }
        match self.task.expect("non-immediate cleanup owns a task").await {
            Ok(Ok(disposition)) => disposition.into(),
            Ok(Err(_)) | Err(_) => DetachedCleanupDisposition::Failed,
        }
    }
}

/// One owner for the current warm turn's terminal state. `observe_exit` is
/// synchronous so a structured failure arms expiry before any formatting,
/// persistence, flush, lock, or other cancellation point.
pub struct WarmCompletionGuard {
    sm: Arc<SessionManager>,
    ctx: ContextId,
    generation: SessionGeneration,
    op: OperationId,
    expiry_intent: WarmExpiryIntent,
    diagnostic: Arc<dyn DiagnosticObserver>,
    canceled_action: WarmCompletionAction,
    legacy_crash_action: LegacyCrashAction,
    action: WarmCompletionAction,
    armed: bool,
}

/// Compatibility name retained for the inbound dispatch DTO.
pub type WarmTurnGuard = WarmCompletionGuard;

impl WarmCompletionGuard {
    pub fn finish_owner(
        sm: Arc<SessionManager>,
        ctx: ContextId,
        generation: SessionGeneration,
        op: OperationId,
        expiry_intent: WarmExpiryIntent,
        diagnostic: Arc<dyn DiagnosticObserver>,
    ) -> Self {
        Self {
            sm,
            ctx,
            generation,
            op,
            expiry_intent,
            diagnostic,
            canceled_action: WarmCompletionAction::Finish,
            legacy_crash_action: LegacyCrashAction::Finish,
            action: WarmCompletionAction::Finish,
            armed: true,
        }
    }

    pub fn workflow_owner(
        sm: Arc<SessionManager>,
        ctx: ContextId,
        generation: SessionGeneration,
        op: OperationId,
        expiry_intent: WarmExpiryIntent,
        diagnostic: Arc<dyn DiagnosticObserver>,
    ) -> Self {
        Self {
            sm,
            ctx,
            generation,
            op,
            expiry_intent,
            diagnostic,
            canceled_action: WarmCompletionAction::Cancel,
            legacy_crash_action: LegacyCrashAction::Expire,
            action: WarmCompletionAction::Cancel,
            armed: true,
        }
    }

    pub fn observe_exit(&mut self, exit: WarmCompletionExit<'_>) {
        let next = match exit {
            WarmCompletionExit::Normal => WarmCompletionAction::Finish,
            WarmCompletionExit::Canceled => self.canceled_action,
            WarmCompletionExit::Error(error)
                if warm_session_survivability(error) == WarmSessionSurvivability::Expire =>
            {
                WarmCompletionAction::Expire
            }
            WarmCompletionExit::Error(BridgeError::AgentCrashed { .. }) => {
                match self.legacy_crash_action {
                    LegacyCrashAction::Finish => WarmCompletionAction::Finish,
                    LegacyCrashAction::Expire => WarmCompletionAction::Expire,
                }
            }
            WarmCompletionExit::Error(_) => WarmCompletionAction::Finish,
        };
        if next == WarmCompletionAction::Expire {
            self.expiry_intent.arm();
        }
        // Once a concrete failure has proved the session unsafe, later channel
        // closure/cancellation bookkeeping cannot make it reusable again.
        if self.action != WarmCompletionAction::Expire {
            self.action = next;
        }
    }

    pub async fn record_usage(&self, usage: bridge_core::orch::UsageSnapshot) {
        self.sm
            .record_usage(&self.ctx, self.generation, &self.op, usage)
            .await;
    }

    pub async fn transfer_cleanup_custody(mut self) -> DetachedWarmCleanup {
        match self.action {
            WarmCompletionAction::Finish => {
                let result = self
                    .sm
                    .finish_owned_turn(&self.ctx, self.generation, &self.op)
                    .await;
                self.armed = false;
                DetachedWarmCleanup {
                    task: None,
                    immediate: Some(
                        result
                            .map(DetachedCleanupDisposition::from)
                            .unwrap_or(DetachedCleanupDisposition::Failed),
                    ),
                }
            }
            WarmCompletionAction::Cancel => {
                let result = self
                    .sm
                    .cancel_turn_current(&self.ctx, self.generation, &self.op)
                    .await;
                self.armed = false;
                DetachedWarmCleanup {
                    task: None,
                    immediate: Some(if result.is_ok() {
                        DetachedCleanupDisposition::Complete
                    } else {
                        DetachedCleanupDisposition::Failed
                    }),
                }
            }
            WarmCompletionAction::Expire => {
                let claim = self
                    .sm
                    .begin_expire_current(&self.ctx, self.generation, &self.op)
                    .await;
                let Some(claim) = claim else {
                    self.armed = false;
                    return DetachedWarmCleanup {
                        task: None,
                        immediate: Some(DetachedCleanupDisposition::OwnerHeld),
                    };
                };
                let flight = claim
                    .into_flight()
                    .expect("a fresh expiry claim owns one cleanup flight");
                self.armed = false;
                let diagnostic = self.diagnostic.clone();
                let task = tokio::spawn(async move {
                    let started = record_cleanup_transition(
                        diagnostic.as_ref(),
                        bridge_core::diagnostics::PhaseStatus::Started,
                        "warm.teardown.release",
                    )
                    .await;
                    let report = ExpiryClaim::join_flight(flight).await;
                    started?;
                    let (status, code) = if report.result.is_ok() {
                        (
                            bridge_core::diagnostics::PhaseStatus::Completed,
                            "warm.teardown.released",
                        )
                    } else {
                        (
                            bridge_core::diagnostics::PhaseStatus::Failed,
                            "warm.teardown.release_failed",
                        )
                    };
                    let observation =
                        record_cleanup_transition(diagnostic.as_ref(), status, code).await;
                    match (report.result, observation) {
                        (Err(primary), _) => Err(primary),
                        (Ok(_), Err(observation)) => Err(observation),
                        (Ok(disposition), Ok(())) => Ok(disposition),
                    }
                });
                DetachedWarmCleanup {
                    task: Some(task),
                    immediate: None,
                }
            }
        }
    }

    pub async fn complete(mut self) -> Result<BackendCleanupDispositionV1, BridgeError> {
        match self.action {
            WarmCompletionAction::Finish => {
                let result = self
                    .sm
                    .finish_owned_turn(&self.ctx, self.generation, &self.op)
                    .await;
                self.armed = false;
                result
            }
            WarmCompletionAction::Cancel => {
                let result = self
                    .sm
                    .cancel_turn_current(&self.ctx, self.generation, &self.op)
                    .await;
                self.armed = false;
                result.map(|()| BackendCleanupDispositionV1::Complete)
            }
            WarmCompletionAction::Expire => {
                let claim = self
                    .sm
                    .begin_expire_current(&self.ctx, self.generation, &self.op)
                    .await;
                let Some(claim) = claim else {
                    self.armed = false;
                    return Ok(BackendCleanupDispositionV1::Complete);
                };
                // From this point the claim/tombstone pair owns fallback. The
                // completion guard must not start a second cleanup flight.
                // Transfer the claim into that observer-free flight before
                // awaiting teardown-start persistence: a pending/canceled
                // observer can detach reporting but cannot suppress cleanup.
                let flight = claim
                    .into_flight()
                    .expect("a fresh expiry claim owns one cleanup flight");
                self.armed = false;
                let started_observation = record_cleanup_transition(
                    self.diagnostic.as_ref(),
                    bridge_core::diagnostics::PhaseStatus::Started,
                    "warm.teardown.release",
                )
                .await;
                let report = ExpiryClaim::join_flight(flight).await;
                // Preserve the existing observation-error precedence, but
                // only after the owned cleanup report has settled.
                started_observation?;
                let terminal = if report.result.is_ok() {
                    (
                        bridge_core::diagnostics::PhaseStatus::Completed,
                        "warm.teardown.released",
                    )
                } else {
                    (
                        bridge_core::diagnostics::PhaseStatus::Failed,
                        "warm.teardown.release_failed",
                    )
                };
                let observation =
                    record_cleanup_transition(self.diagnostic.as_ref(), terminal.0, terminal.1)
                        .await;
                match (report.result, observation) {
                    (Err(primary), _) => Err(primary),
                    (Ok(_), Err(observation)) => Err(observation),
                    (Ok(disposition), Ok(())) => Ok(disposition),
                }
            }
        }
    }
}

async fn record_cleanup_transition(
    observer: &dyn DiagnosticObserver,
    status: bridge_core::diagnostics::PhaseStatus,
    code: &'static str,
) -> Result<(), BridgeError> {
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor,
        PersistedPhaseTransition, PersistedPhaseTransitionInput,
    };

    let transition = PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Teardown,
            status,
            at_ms: diagnostic_timestamp_ms(),
            operation: None,
            code: None,
            auth: None,
        },
        Some(code),
        &DiagnosticRedactor::default(),
    )
    .map_err(|_| BridgeError::InvalidStateTransition)?;
    let event =
        DiagnosticEvent::new(transition, None).map_err(|_| BridgeError::InvalidStateTransition)?;
    observer.record(event).await
}

impl Drop for WarmCompletionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let op = self.op.clone();
        match self.action {
            WarmCompletionAction::Finish => {
                tokio::spawn(async move {
                    sm.finish_turn(&ctx, generation, &op).await;
                });
            }
            WarmCompletionAction::Cancel => {
                tokio::spawn(async move {
                    let _ = sm.cancel_turn_current(&ctx, generation, &op).await;
                });
            }
            WarmCompletionAction::Expire => {
                sm.expire_current_unobserved(ctx, generation, op);
            }
        }
    }
}

/// Governed apply transfers both single-use accounting and exact completion
/// ownership together. R2 route adapters move these into LocalDispatch's seams.
pub struct GovernedDispatch {
    pub turn: crate::session_manager::WarmTurn,
    pub usage: bridge_core::translator::PreparedAccountingTurnV1,
    pub completion: WarmCompletionGuard,
}
