use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use a2a;
use bridge_core::domain::{AgentOverride, Effort, InjectRequest, Part, PermitDecision};
use bridge_core::error::BridgeError;
#[cfg(test)]
use bridge_core::ids::OperationId;
use bridge_core::ids::{AgentId, BatchId, ContextId, TaskId, WorkflowId};
use bridge_core::orch::{AgentSessionCaps, TerminalUsage, UsageSnapshot};
use bridge_core::permission::{PermKey, PermissionRegistry, PermissionResolution, TurnMeta};
use bridge_core::ports::{
    classify_failure, AgentRegistry, DiagnosticObserver, FailureClass, ObsEvent, Observer,
    PolicyEngine, SessionStore, TurnContext, TurnOutcome, UsageFinalization,
};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{BatchSummary, TaskRecord, TaskRecordStatus, TaskStore};
use bridge_core::translator::{Event, EventKind, TaskOutcome, Translator};
use bridge_workflow::executor::{WorkflowExecutor, WorkflowRunContext};
use bridge_workflow::graph::WorkflowGraph;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::batch::{BatchDeps, BatchParams, BatchRuntime};
use crate::clock::Clock;
use crate::detached::{
    reconcile_pending_terminal_projections, reconcile_terminal_checkpoints, resume_working_tasks,
    spawn_detached_workflow_with_attempt_barriers, workflow_prompt_dispatch_barrier_with_state,
    workflow_terminal_summary_barrier, AttemptTelemetryState, DetachedDeps, TaskProgressHub,
};
use crate::dispatch::{TaskBinding, WarmCompletionExit, WarmCompletionGuard};
use crate::params::{OpParams, PermitParams};
use crate::turn_parts::assemble_turn_parts;

static PROMPT_ID_SEQ: AtomicU64 = AtomicU64::new(1);
const DIRECT_DIAGNOSTIC_CAPACITY: usize = 64;

fn direct_diagnostic_observer() -> Arc<dyn DiagnosticObserver> {
    Arc::new(
        bridge_core::diagnostics::InMemoryDiagnosticObserver::new(DIRECT_DIAGNOSTIC_CAPACITY)
            .expect("direct diagnostic capacity is nonzero"),
    )
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusDto {
    Session(SessionStatusDto),
    Task(TaskStatusDto),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct TraceRefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub journal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<BTreeMap<String, String>>,
}

fn percent_encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b))
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn turn_ref(turn_id: &bridge_core::ids::TurnId) -> String {
    format!("/turns/{}", percent_encode_segment(turn_id.as_str()))
}
fn journal_ref(task_id: &TaskId) -> String {
    format!(
        "/tasks/{}/journal.jsonl",
        percent_encode_segment(task_id.as_str())
    )
}
fn artifact_ref(task_id: &TaskId, node: &bridge_core::ids::NodeId) -> String {
    format!(
        "/tasks/{}/artifacts/{}",
        percent_encode_segment(task_id.as_str()),
        percent_encode_segment(node.as_str())
    )
}

#[derive(serde::Serialize)]
pub struct SessionStatusDto {
    pub state: &'static str,
    pub agent: String,
    pub generation: u64,
    pub idle_age_ms: u128,
    pub capabilities: AgentSessionCaps,
    pub usage: UsageSnapshot,
    pub over_threshold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceRefs>,
}

#[derive(serde::Serialize)]
pub struct TaskStatusDto {
    pub id: TaskId,
    pub workflow: String,
    pub status: &'static str,
    pub result: Option<String>,
    pub error: Option<String>,
    pub updated_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace: Option<TraceRefs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<bridge_core::ids::ExecutionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<bridge_core::ids::AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_ordinal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<bridge_core::ids::AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry_unavailable: Option<bridge_core::workflow_history::LedgerUnavailableReason>,
}

pub struct DirectAttemptHandle {
    barrier: bridge_core::workflow_history::DirectAttemptBarrier,
    pub identity: bridge_core::ids::AttemptIdentity,
    observer: Arc<dyn Observer>,
    workflow: &'static str,
    task_class: &'static str,
    surface: &'static str,
    finished: bool,
}

impl DirectAttemptHandle {
    fn stop_observation(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.observer
            .record_workflow(&bridge_core::ports::WorkflowObsEvent::Stopped {
                task_class: self.task_class,
                surface: self.surface,
            });
    }

    pub async fn mark_prompt_dispatch(&mut self) -> Result<(), BridgeError> {
        match self.barrier.mark_prompt_dispatch().await {
            Ok(()) => Ok(()),
            Err(error) => {
                let reason = error.reason;
                self.observer.record_workflow(
                    &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
                );
                Err(BridgeError::DurableEvidenceUnavailable {
                    reason: reason.as_str(),
                })
            }
        }
    }

    pub async fn finish(
        &mut self,
        outcome: &'static str,
        reason: &'static str,
        degraded: bool,
        cleanup_disposition: &'static str,
    ) -> Result<(), BridgeError> {
        self.finish_with_completeness(outcome, reason, degraded, cleanup_disposition, true)
            .await
    }

    pub async fn finish_with_completeness(
        &mut self,
        outcome: &'static str,
        reason: &'static str,
        degraded: bool,
        cleanup_disposition: &'static str,
        telemetry_complete: bool,
    ) -> Result<(), BridgeError> {
        match self
            .barrier
            .finish(
                outcome,
                reason,
                degraded,
                cleanup_disposition,
                telemetry_complete,
            )
            .await
        {
            Ok((bridge_core::workflow_history::TerminalWrite::Applied, terminal)) => {
                self.finished = true;
                self.observer
                    .record_workflow(&bridge_core::ports::WorkflowObsEvent::Finished {
                        attempt_id: &self.identity.attempt_id,
                        workflow: self.workflow,
                        task_class: self.task_class,
                        surface: self.surface,
                        policy: "r2f0a",
                        outcome: terminal.outcome.as_str(),
                        telemetry_complete: terminal.telemetry_complete,
                        work_seconds: terminal.work_ms as f64 / 1000.0,
                        end_to_end_seconds: terminal.end_to_end_ms as f64 / 1000.0,
                    });
                Ok(())
            }
            Ok((bridge_core::workflow_history::TerminalWrite::Replayed, _)) => {
                self.stop_observation();
                Ok(())
            }
            Ok((bridge_core::workflow_history::TerminalWrite::Conflict, _)) => {
                unreachable!("the shared direct barrier maps conflicts to a typed error")
            }
            Err(error) => {
                if error.reason == bridge_core::workflow_history::LedgerUnavailableReason::Collision
                {
                    // A conflicting durable terminal belongs to the same admitted
                    // attempt but cannot be represented as our Finished sample.
                    // Balance the admission exactly once so the in-flight gauge
                    // cannot remain permanently elevated.
                    self.stop_observation();
                }
                self.observer.record_workflow(
                    &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable {
                        reason: error.reason,
                    },
                );
                Err(BridgeError::DurableEvidenceUnavailable {
                    reason: error.reason.as_str(),
                })
            }
        }
    }
}

impl Drop for DirectAttemptHandle {
    fn drop(&mut self) {
        self.stop_observation();
    }
}

pub type WorkflowHistorySelection = Result<
    Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
    bridge_core::workflow_history::LedgerUnavailableReason,
>;

/// Mandatory coordinator-owned admission for every direct execution surface.
/// Callers may select their durable store before constructing a full coordinator
/// instance, but admission, dispatch uncertainty, terminalization, and
/// conservative drop behavior remain one shared state machine.
#[allow(clippy::too_many_arguments)]
pub async fn admit_direct_attempt_with_history(
    selection: WorkflowHistorySelection,
    observer: Arc<dyn Observer>,
    identity: bridge_core::ids::AttemptIdentity,
    surface: bridge_core::workflow_history::ExecutionSurface,
    workflow: &'static str,
    task_class: &'static str,
    workload_fingerprint: String,
    workload_fingerprint_complete: bool,
    started_ms: i64,
    abort_reason: &'static str,
) -> Result<DirectAttemptHandle, BridgeError> {
    use bridge_core::workflow_history::{DirectAttemptBarrier, ExecutionSurface};

    if identity.ordinal != 0 || identity.parent_attempt_id.is_some() {
        return Err(BridgeError::InvalidRequest {
            field: "attempt linkage",
        });
    }
    if !matches!(
        surface,
        ExecutionSurface::DirectUnary | ExecutionSurface::Mcp | ExecutionSurface::Smoke
    ) {
        return Err(BridgeError::InvalidRequest {
            field: "direct execution surface",
        });
    }
    let store = match selection {
        Ok(store) => store,
        Err(reason) => {
            observer.record_workflow(
                &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
            );
            return Err(BridgeError::DurableEvidenceUnavailable {
                reason: reason.as_str(),
            });
        }
    };
    let task = TaskId::parse(identity.execution_id.as_str().to_owned())?;
    let reservation = bridge_core::workflow_history::AttemptReservation {
        identity: identity.clone(),
        task_id: Some(task),
        workflow: workflow.into(),
        task_class: task_class.into(),
        surface,
        policy: "r2f0a".into(),
        workload_fingerprint,
        started_ms,
        workload_fingerprint_complete,
        prompt_acceptance: "not_dispatched".into(),
        pinned: false,
    };
    let barrier = match DirectAttemptBarrier::admit(store, reservation, abort_reason).await {
        Ok(barrier) => barrier,
        Err(error) => {
            observer.record_workflow(
                &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable {
                    reason: error.reason,
                },
            );
            return Err(BridgeError::DurableEvidenceUnavailable {
                reason: error.reason.as_str(),
            });
        }
    };
    observer.record_workflow(&bridge_core::ports::WorkflowObsEvent::Started {
        task_class,
        surface: surface.as_str(),
    });
    Ok(DirectAttemptHandle {
        barrier,
        identity,
        observer,
        workflow,
        task_class,
        surface: surface.as_str(),
        finished: false,
    })
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct WorkflowLocator {
    pub task_id: TaskId,
    pub execution_id: bridge_core::ids::ExecutionId,
    pub attempt_id: bridge_core::ids::AttemptId,
    pub attempt_ordinal: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<bridge_core::ids::AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry_unavailable: Option<bridge_core::workflow_history::LedgerUnavailableReason>,
}

pub struct TurnOutput {
    pub text: String,
    pub stop_reason: String,
    pub context: ContextId,
}

#[cfg(test)]
#[derive(Default, Clone)]
struct NoopObserver;

#[cfg(test)]
impl Observer for NoopObserver {
    fn record(&self, _e: &ObsEvent<'_>) {}
}

impl From<&crate::session_manager::SessionStatusInfo> for SessionStatusDto {
    fn from(info: &crate::session_manager::SessionStatusInfo) -> Self {
        Self {
            state: info.state,
            agent: info.agent.clone(),
            generation: info.generation,
            idle_age_ms: info.idle_age_ms,
            capabilities: info.capabilities.clone(),
            usage: info.usage.clone(),
            over_threshold: info.over_threshold,
            trace: None,
        }
    }
}

impl From<&TaskRecord> for TaskStatusDto {
    fn from(rec: &TaskRecord) -> Self {
        Self {
            id: rec.id.clone(),
            workflow: rec.workflow.clone(),
            status: rec.status.as_str(),
            result: rec.result.clone(),
            error: rec.error.clone(),
            updated_ms: rec.updated_ms,
            usage: None,
            trace: None,
            execution_id: None,
            attempt_id: None,
            attempt_ordinal: None,
            parent_attempt_id: None,
            telemetry_unavailable: None,
        }
    }
}

/// The stable Rust service API. ONE owner of the orchestration state; A2A/CLI/MCP are thin adapters
/// over it. Concrete struct (one impl, no trait).
pub struct Coordinator {
    pub session_manager: Arc<crate::session_manager::SessionManager>,
    executor: Option<Arc<WorkflowExecutor>>,
    workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>,
    task_store: Arc<dyn TaskStore>,
    session_store: Arc<dyn SessionStore>,
    policy: Arc<dyn PolicyEngine>,
    registry: Arc<dyn AgentRegistry>,
    bindings: Arc<Mutex<HashMap<TaskId, TaskBinding>>>,
    progress_hubs: Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>,
    workflow_cancels: Arc<Mutex<HashMap<TaskId, CancellationToken>>>,
    workflow_runs: Arc<Mutex<HashMap<ContextId, CancellationToken>>>,
    permission_registry: Option<Arc<PermissionRegistry>>,
    clock: Arc<dyn Clock>,
    allowed_cwd_root: Option<SessionCwd>,
    batch: Option<BatchRuntime>,
    observer: Arc<dyn Observer>,
    resume_attempt_cap: u32,
    trace_refs_enabled: bool,
    max_task_turns: usize,
    workflow_history: Option<
        Result<
            Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
            bridge_core::workflow_history::LedgerUnavailableReason,
        >,
    >,
}

pub fn apply_permit(reg: &PermissionRegistry, p: &PermitParams) -> bool {
    if matches!(p.decision, PermitDecision::Escalate { .. }) {
        return false;
    }
    let key = PermKey {
        context_id: p.context.clone(),
        generation: p.generation,
        op: p.op.clone(),
        request_id: p.request_id.clone(),
    };
    reg.resolve(&key, PermissionResolution::Decided(p.decision.clone()))
}

impl Coordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_manager: Arc<crate::session_manager::SessionManager>,
        executor: Option<Arc<WorkflowExecutor>>,
        workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>,
        task_store: Arc<dyn TaskStore>,
        session_store: Arc<dyn SessionStore>,
        policy: Arc<dyn PolicyEngine>,
        registry: Arc<dyn AgentRegistry>,
        clock: Arc<dyn Clock>,
        allowed_cwd_root: Option<SessionCwd>,
        batch: Option<BatchRuntime>,
        observer: Arc<dyn Observer>,
        resume_attempt_cap: u32,
    ) -> Self {
        Self {
            session_manager,
            executor,
            workflows,
            task_store,
            session_store,
            policy,
            registry,
            bindings: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            workflow_runs: Arc::new(Mutex::new(HashMap::new())),
            permission_registry: None,
            clock,
            allowed_cwd_root,
            batch,
            observer,
            resume_attempt_cap,
            trace_refs_enabled: false,
            max_task_turns: 512,
            workflow_history: Some(Err(
                bridge_core::workflow_history::LedgerUnavailableReason::Open,
            )),
        }
    }

    #[must_use]
    pub fn with_workflow_history(
        mut self,
        history: Result<
            Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
            bridge_core::workflow_history::LedgerUnavailableReason,
        >,
    ) -> Self {
        self.workflow_history = Some(history);
        self
    }

    #[must_use]
    pub fn with_trace_refs_config(mut self, enabled: bool, max_task_turns: usize) -> Self {
        self.trace_refs_enabled = enabled;
        self.max_task_turns = max_task_turns;
        self
    }

    #[must_use]
    pub fn with_permission_registry(mut self, reg: Arc<PermissionRegistry>) -> Self {
        self.permission_registry = Some(reg);
        self
    }

    // ---- Shared-state accessors (#10 D2) ----
    // The A2A adapter (`bridge-a2a-inbound`, a SEPARATE crate) adopts these SAME
    // Arc instances so turn-lifecycle STATE has ONE owner. Because the adapter is
    // cross-crate, these must be `pub` (not `pub(crate)`). Each returns a clone of
    // the owned Arc — Arc identity is preserved, so a mutation on either surface is
    // visible to both. Build the Coordinator FIRST, then the adapter adopts from it.

    pub fn task_store(&self) -> Arc<dyn TaskStore> {
        self.task_store.clone()
    }
    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.session_store.clone()
    }
    pub fn registry(&self) -> Arc<dyn AgentRegistry> {
        self.registry.clone()
    }
    pub fn policy(&self) -> Arc<dyn PolicyEngine> {
        self.policy.clone()
    }
    pub fn executor(&self) -> Option<Arc<WorkflowExecutor>> {
        self.executor.clone()
    }
    pub fn workflows(&self) -> Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>> {
        self.workflows.clone()
    }
    pub fn bindings(&self) -> Arc<Mutex<HashMap<TaskId, TaskBinding>>> {
        self.bindings.clone()
    }
    pub fn workflow_cancels(&self) -> Arc<Mutex<HashMap<TaskId, CancellationToken>>> {
        self.workflow_cancels.clone()
    }
    pub fn workflow_runs(&self) -> Arc<Mutex<HashMap<ContextId, CancellationToken>>> {
        self.workflow_runs.clone()
    }
    pub fn progress_hubs(&self) -> Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>> {
        self.progress_hubs.clone()
    }
    pub fn permission_registry(&self) -> Option<Arc<PermissionRegistry>> {
        self.permission_registry.clone()
    }
    pub fn batch(&self) -> Option<BatchRuntime> {
        self.batch.clone()
    }
    pub fn observer(&self) -> Arc<dyn Observer> {
        self.observer.clone()
    }
    pub fn workflow_history(
        &self,
    ) -> Option<
        Result<
            Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
            bridge_core::workflow_history::LedgerUnavailableReason,
        >,
    > {
        self.workflow_history.clone()
    }
    pub fn allowed_cwd_root(&self) -> Option<SessionCwd> {
        self.allowed_cwd_root.clone()
    }

    // ---- By-reference accessors (#10 slice 7) ----
    // The clone accessors above are for adoption/identity; the A2A adapter's handlers
    // read these in-place (e.g. `bindings().lock().await`, `registry().resolve(...)`),
    // where a cloned temporary would be dropped while borrowed. These borrow the owned
    // Arc from the Coordinator the adapter holds behind its own `Arc<Coordinator>`.
    pub fn registry_ref(&self) -> &Arc<dyn AgentRegistry> {
        &self.registry
    }
    pub fn workflow_history_ref(
        &self,
    ) -> &Option<
        Result<
            Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
            bridge_core::workflow_history::LedgerUnavailableReason,
        >,
    > {
        &self.workflow_history
    }
    pub fn policy_ref(&self) -> &Arc<dyn PolicyEngine> {
        &self.policy
    }
    pub fn task_store_ref(&self) -> &Arc<dyn TaskStore> {
        &self.task_store
    }
    pub fn executor_ref(&self) -> &Option<Arc<WorkflowExecutor>> {
        &self.executor
    }
    pub fn workflows_ref(&self) -> &Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>> {
        &self.workflows
    }
    pub fn permission_registry_ref(&self) -> &Option<Arc<PermissionRegistry>> {
        &self.permission_registry
    }
    pub fn batch_ref(&self) -> &Option<BatchRuntime> {
        &self.batch
    }
    pub fn bindings_ref(&self) -> &Arc<Mutex<HashMap<TaskId, TaskBinding>>> {
        &self.bindings
    }
    pub fn workflow_cancels_ref(&self) -> &Arc<Mutex<HashMap<TaskId, CancellationToken>>> {
        &self.workflow_cancels
    }
    pub fn workflow_runs_ref(&self) -> &Arc<Mutex<HashMap<ContextId, CancellationToken>>> {
        &self.workflow_runs
    }
    pub fn progress_hubs_ref(&self) -> &Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>> {
        &self.progress_hubs
    }

    /// Build the detached-workflow dependency view over the Coordinator's owned fields.
    fn detached_deps(&self) -> DetachedDeps {
        DetachedDeps {
            task_store: self.task_store.clone(),
            executor: self.executor.clone(),
            workflows: self.workflows.clone(),
            workflow_cancels: self.workflow_cancels.clone(),
            progress_hubs: self.progress_hubs.clone(),
            clock: self.clock.clone(),
            observer: self.observer.clone(),
            workflow_history: self.workflow_history.clone(),
        }
    }

    pub fn batch_deps(&self) -> Option<BatchDeps> {
        Some(BatchDeps {
            detached: self.detached_deps(),
            runtime: self.batch.clone()?,
            allowed_cwd_root: self.allowed_cwd_root.clone(),
        })
    }

    pub async fn run_batch(&self, p: BatchParams) -> Result<BatchId, BridgeError> {
        let bdeps = self.batch_deps().ok_or(BridgeError::InvalidRequest {
            field: "batch (not configured)",
        })?;
        crate::batch::run_batch(&bdeps, p).await
    }

    pub async fn batch_status(&self, id: &BatchId) -> Result<BatchSummary, BridgeError> {
        let bdeps = self.batch_deps().ok_or(BridgeError::InvalidRequest {
            field: "batch (not configured)",
        })?;
        crate::batch::batch_status(&bdeps, id).await
    }

    pub async fn batch_list(&self, limit: usize) -> Result<Vec<BatchSummary>, BridgeError> {
        let bdeps = self.batch_deps().ok_or(BridgeError::InvalidRequest {
            field: "batch (not configured)",
        })?;
        crate::batch::batch_list(&bdeps, limit).await
    }

    pub async fn cancel_batch(&self, id: &BatchId) -> Result<bool, BridgeError> {
        let bdeps = self.batch_deps().ok_or(BridgeError::InvalidRequest {
            field: "batch (not configured)",
        })?;
        crate::batch::cancel_batch(&bdeps, id).await
    }

    fn mint_context_id(&self) -> ContextId {
        let seq = PROMPT_ID_SEQ.fetch_add(1, Ordering::Relaxed);
        ContextId::parse(format!("ctx-{}-{seq}", self.clock.now_ms()))
            .expect("minted context id is non-empty")
    }

    fn mint_prompt_task_id(&self) -> TaskId {
        let seq = PROMPT_ID_SEQ.fetch_add(1, Ordering::Relaxed);
        TaskId::parse(format!("prompt-{}-{seq}", self.clock.now_ms()))
            .expect("minted task id is non-empty")
    }

    pub fn direct_workload_fingerprint(
        &self,
        agent: &AgentId,
        overrides: Option<&AgentOverride>,
        route_kind: &'static str,
    ) -> (String, bool) {
        // This shape is intentionally request-derived. Reading registry defaults
        // before mandatory attempt admission would let a colliding identity reach
        // registry work. Unknown defaults make the row ineligible for calibration.
        let effort = overrides
            .and_then(|value| value.effort)
            .map(|effort| match effort {
                Effort::Minimal => "minimal",
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High => "high",
                Effort::Xhigh => "xhigh",
                Effort::Max => "max",
            });
        let canonical = serde_json::to_vec(&serde_json::json!({
            "route": route_kind,
            "agent": agent.as_str(),
            "model": overrides.and_then(|value| value.model.as_deref()),
            "effort": effort,
            "mode": overrides.and_then(|value| value.mode.as_deref()),
            "config_known": false,
        }))
        .expect("fixed direct request shape is serializable");
        (
            bridge_core::workflow_history::fingerprint_workload_shape(&canonical),
            false,
        )
    }

    pub async fn admit_direct_attempt(
        &self,
        identity: bridge_core::ids::AttemptIdentity,
        surface: bridge_core::workflow_history::ExecutionSurface,
        task_class: &'static str,
        workload_fingerprint: String,
        workload_fingerprint_complete: bool,
    ) -> Result<DirectAttemptHandle, BridgeError> {
        let selection = self.workflow_history.clone().unwrap_or(Err(
            bridge_core::workflow_history::LedgerUnavailableReason::Open,
        ));
        admit_direct_attempt_with_history(
            selection,
            self.observer.clone(),
            identity,
            surface,
            "direct",
            task_class,
            workload_fingerprint,
            workload_fingerprint_complete,
            self.clock.now_ms(),
            "caller_disconnected",
        )
        .await
    }

    /// Compatibility-only service entry used by embedders with no selected history.
    /// Production MCP/A2A callers must use `prompt_with_identity`.
    #[cfg(test)]
    pub async fn prompt(&self, p: OpParams) -> Result<TurnOutput, BridgeError> {
        if matches!(&self.workflow_history, Some(Ok(_))) {
            return Err(BridgeError::InvalidRequest {
                field: "execution_id/attempt_id",
            });
        }
        self.prompt_inner(p, None).await
    }

    pub async fn prompt_with_identity(
        &self,
        p: OpParams,
        identity: bridge_core::ids::AttemptIdentity,
    ) -> Result<TurnOutput, BridgeError> {
        let _ = p.validate_cwd(self.allowed_cwd_root.as_ref())?;
        let fingerprint_agent = p.agent.clone().unwrap_or_else(|| {
            AgentId::parse("unresolved").expect("fixed unresolved agent id is valid")
        });
        let overrides = p.agent_override();
        let (fingerprint, fingerprint_complete) =
            self.direct_workload_fingerprint(&fingerprint_agent, Some(&overrides), "mcp_prompt");
        let attempt = self
            .admit_direct_attempt(
                identity,
                bridge_core::workflow_history::ExecutionSurface::Mcp,
                "direct",
                fingerprint,
                fingerprint_complete,
            )
            .await?;
        self.prompt_inner(p, Some(attempt)).await
    }

    async fn prompt_inner(
        &self,
        p: OpParams,
        mut attempt: Option<DirectAttemptHandle>,
    ) -> Result<TurnOutput, BridgeError> {
        let _deferred_cold_bindings = &self.bindings;
        let cwd = p.validate_cwd(self.allowed_cwd_root.as_ref())?;
        let agent = p
            .agent
            .clone()
            .unwrap_or_else(|| self.registry.default_id());
        let ctx = p.context.clone().unwrap_or_else(|| self.mint_context_id());
        let diagnostic = direct_diagnostic_observer();
        let turn = match self
            .session_manager
            .checkout_turn_observed(
                &ctx,
                agent,
                Some(p.agent_override()),
                cwd,
                diagnostic.clone(),
            )
            .await
        {
            Ok(turn) => turn,
            Err(error) => {
                if let Some(attempt) = attempt.as_mut() {
                    attempt
                        .finish("failed", "pre_prompt_failure", true, "not_needed")
                        .await?;
                }
                return Err(error);
            }
        };
        self.collect_turn_observed_with_attempt(ctx, turn, p.input, diagnostic, attempt)
            .await
    }

    /// Continue an EXISTING warm context. Unlike `prompt`, this REUSES the context's stored fingerprint
    /// (agent/config/cwd) instead of re-deriving it from params: the `continue` surface advertises only
    /// `{input, context}`, so omitted agent/cwd/overrides must NOT be read as a config change (which
    /// `checkout_turn` rejects as `ConfigMismatch`). A context that was never minted → `SessionNotFound`.
    #[cfg(test)]
    pub async fn continue_turn(&self, p: OpParams) -> Result<TurnOutput, BridgeError> {
        if matches!(&self.workflow_history, Some(Ok(_))) {
            return Err(BridgeError::InvalidRequest {
                field: "execution_id/attempt_id",
            });
        }
        self.continue_turn_inner(p, None).await
    }

    pub async fn continue_turn_with_identity(
        &self,
        p: OpParams,
        identity: bridge_core::ids::AttemptIdentity,
    ) -> Result<TurnOutput, BridgeError> {
        if p.context.is_none() {
            return Err(BridgeError::InvalidRequest { field: "context" });
        }
        let fingerprint_agent =
            AgentId::parse("unresolved").expect("fixed unresolved agent id is valid");
        let (fingerprint, _) =
            self.direct_workload_fingerprint(&fingerprint_agent, None, "mcp_warm_continuation");
        let attempt = self
            .admit_direct_attempt(
                identity,
                bridge_core::workflow_history::ExecutionSurface::Mcp,
                "direct",
                fingerprint,
                false,
            )
            .await?;
        self.continue_turn_inner(p, Some(attempt)).await
    }

    async fn continue_turn_inner(
        &self,
        p: OpParams,
        mut attempt: Option<DirectAttemptHandle>,
    ) -> Result<TurnOutput, BridgeError> {
        let ctx = p
            .context
            .clone()
            .ok_or(BridgeError::InvalidRequest { field: "context" })?;
        let diagnostic = direct_diagnostic_observer();
        let turn = match self.session_manager.checkout_existing_turn(&ctx).await {
            Ok(turn) => turn,
            Err(error) => {
                if let Some(attempt) = attempt.as_mut() {
                    attempt
                        .finish("failed", "pre_prompt_failure", true, "not_needed")
                        .await?;
                }
                return Err(error);
            }
        };
        self.collect_turn_observed_with_attempt(ctx, turn, p.input, diagnostic, attempt)
            .await
    }

    pub async fn inject(&self, req: InjectRequest) -> Result<usize, BridgeError> {
        self.session_manager.inject(req).await
    }

    pub async fn permit(&self, p: PermitParams) -> Result<bool, BridgeError> {
        Ok(self
            .permission_registry
            .as_ref()
            .map(|reg| apply_permit(reg, &p))
            .unwrap_or(false))
    }

    fn new_turn_id() -> bridge_core::ids::TurnId {
        bridge_core::ids::TurnId::parse(format!("turn-{}", a2a::new_task_id()))
            .expect("a2a task id is non-empty")
    }

    fn turn_context_for_warm(
        ctx: &ContextId,
        task: Option<TaskId>,
        turn: &crate::session_manager::WarmTurn,
    ) -> TurnContext {
        TurnContext {
            turn_id: Self::new_turn_id(),
            session_id: ctx.clone(),
            task_id: task,
            workflow: None,
            node: None,
            attempt: 0,
            agent: turn.agent.as_str().to_string(),
            model: turn.model.clone(),
            effort: turn.effort.clone(),
            mode: turn.mode.clone(),
            prompt_id: None,
            traceparent: None,
        }
    }

    /// Drive ONE warm turn to completion and collect it into a `TurnOutput`. Records usage as a side
    /// effect (excluded from output) and settles the handle on EVERY exit: normal and legacy-owner paths
    /// return to Idle, structured failures expire through the exact cleanup claim, and cancellation uses
    /// the drop fallback if the caller disappears mid-drain. The MCP loop is sequential, but Coordinator
    /// is also a general service API, so a canceled caller must never strand `Running`.
    #[cfg(test)]
    async fn collect_turn(
        &self,
        ctx: ContextId,
        turn: crate::session_manager::WarmTurn,
        input: String,
    ) -> Result<TurnOutput, BridgeError> {
        self.collect_turn_observed(
            ctx,
            turn,
            input,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    #[cfg(test)]
    async fn collect_turn_observed(
        &self,
        ctx: ContextId,
        turn: crate::session_manager::WarmTurn,
        input: String,
        diagnostic: Arc<dyn DiagnosticObserver>,
    ) -> Result<TurnOutput, BridgeError> {
        self.collect_turn_observed_with_attempt(ctx, turn, input, diagnostic, None)
            .await
    }

    async fn collect_turn_observed_with_attempt(
        &self,
        ctx: ContextId,
        turn: crate::session_manager::WarmTurn,
        input: String,
        diagnostic: Arc<dyn DiagnosticObserver>,
        mut attempt: Option<DirectAttemptHandle>,
    ) -> Result<TurnOutput, BridgeError> {
        let task = match attempt.as_ref() {
            Some(attempt) => TaskId::parse(attempt.identity.execution_id.as_str().to_string())?,
            None => self.mint_prompt_task_id(),
        };
        let obs_ctx = Self::turn_context_for_warm(&ctx, Some(task.clone()), &turn);
        let started = Instant::now();
        let mut ttft = None;
        let mut last_usage: Option<UsageSnapshot> = None;
        let shared_usage: Arc<std::sync::Mutex<Option<UsageSnapshot>>> =
            Arc::new(std::sync::Mutex::new(None));
        self.observer
            .record(&ObsEvent::TurnStarted { ctx: &obs_ctx });
        let completion = WarmCompletionGuard::finish_owner(
            self.session_manager.clone(),
            ctx.clone(),
            turn.generation,
            turn.op.clone(),
            turn.expiry_intent.clone(),
            diagnostic.clone(),
        );
        let mut finish_guard = TurnFinishGuard {
            observer: self.observer.clone(),
            ctx: obs_ctx.clone(),
            started,
            armed: true,
            usage: shared_usage.clone(),
            completion: Some(completion),
        };

        let parts = assemble_turn_parts(
            turn.seed.as_deref(),
            &turn.injects,
            vec![Part { text: input }],
        );

        turn.backend
            .configure_turn(
                &turn.session,
                TurnMeta {
                    context_id: ctx.clone(),
                    generation: turn.generation.get(),
                    op: turn.op.clone(),
                },
            )
            .await;

        let translator = Translator::new();
        let mut events = translator.run_observed(
            turn.backend.as_ref(),
            self.session_store.as_ref(),
            self.policy.as_ref(),
            &task,
            &turn.session,
            parts,
            diagnostic,
        );
        let mut collected = Vec::new();
        let mut aborted = false;
        let mut prompt_polled = false;
        loop {
            let ev = tokio::select! {
                biased;
                // cancel-tokens F2 (L1 — abort arm FIRST): a force-reset cancelled this turn → stop without
                // polling events (a pre-first-poll abort means `backend.prompt` never runs → no re-mint).
                _ = turn.abort.cancelled() => {
                    finish_guard.observe_exit(WarmCompletionExit::Canceled);
                    aborted = true;
                    break;
                }
                maybe = async {
                    if !prompt_polled {
                        if let Some(attempt) = attempt.as_mut() {
                            attempt.mark_prompt_dispatch().await?;
                        }
                        // The async block continues in this same poll to
                        // events.next(), so this state is never advanced by an
                        // unpolled provider future.
                        prompt_polled = true;
                    }
                    Ok::<_, BridgeError>(events.next().await)
                } => match maybe {
                    Ok(Some(ev)) => ev,
                    Ok(None) => break,
                    Err(error) => {
                        finish_guard.observe_exit(WarmCompletionExit::Error(&error));
                        collected.push(Err(error));
                        break;
                    }
                },
            };
            if ttft.is_none() {
                ttft = Some(started.elapsed());
            }
            match &ev {
                Ok(e) if e.kind() == &EventKind::Usage => {
                    if let Some(snap) = e.usage_snapshot() {
                        last_usage = Some(snap.clone());
                        *shared_usage.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(snap.clone());
                        self.session_manager
                            .record_usage(&ctx, turn.generation, &turn.op, snap.clone())
                            .await;
                    }
                    continue;
                }
                Err(error) => {
                    // Arm expiry synchronously at the error observation site,
                    // before collection/formatting or any cleanup await.
                    finish_guard.observe_exit(WarmCompletionExit::Error(error));
                    collected.push(ev);
                }
                Ok(event)
                    if event.kind() == &EventKind::Terminal
                        && event.outcome() == Some(TaskOutcome::Canceled) =>
                {
                    finish_guard.observe_exit(WarmCompletionExit::Canceled);
                    collected.push(ev);
                }
                _ => collected.push(ev),
            }
        }
        // Drop the translator stream BEFORE finishing (cancels the in-flight backend future on abort).
        drop(events);
        if aborted {
            collected.push(Ok(Event::terminal(TaskOutcome::Canceled)));
        }

        if !aborted && collected.iter().all(Result::is_ok) {
            finish_guard.observe_exit(WarmCompletionExit::Normal);
        }
        // Complete synchronously on the normal/error path. Cancellation before
        // or during this await drops the shared guard/claim and transfers cleanup
        // to the checked unobserved path.
        let cleanup_result = finish_guard.complete().await;
        finish_guard.disarm();
        let cleanup_disposition = if cleanup_result.is_ok() {
            "complete"
        } else {
            "failed"
        };

        if let Some(Err(e)) = collected.iter().find(|r| r.is_err()) {
            let outcome = TurnOutcome::Failed(classify_failure(e));
            self.observer.record(&ObsEvent::TurnFinished {
                ctx: &obs_ctx,
                latency: started.elapsed(),
                ttft,
                outcome: &outcome,
            });
            self.observer.record(&ObsEvent::UsageFinalized {
                ctx: &obs_ctx,
                usage: last_usage.as_ref(),
                fin: UsageFinalization::TurnFinal,
            });
            if let Some(attempt) = attempt.as_mut() {
                if let Err(terminal_error) = attempt
                    .finish("failed", "prompt_failed", true, cleanup_disposition)
                    .await
                {
                    tracing::warn!(error = ?terminal_error, "direct attempt finalization failed after prompt error");
                }
            }
            return Err(e.clone());
        }
        if let Err(error) = cleanup_result {
            if let Some(attempt) = attempt.as_mut() {
                attempt
                    .finish("failed", "cleanup_failed", true, "failed")
                    .await?;
            }
            return Err(error);
        }
        let events: Vec<Event> = collected.into_iter().filter_map(Result::ok).collect();
        let out_text = if let Some(artifact_text) = events
            .iter()
            .rev()
            .find(|e| e.kind() == &EventKind::Artifact)
            .map(|e| e.text().to_string())
        {
            artifact_text
        } else {
            events
                .iter()
                .filter(|e| e.kind() == &EventKind::Status)
                .map(|e| e.text())
                .collect()
        };
        let stop_reason = match events.iter().rev().find_map(|e| e.outcome()) {
            Some(TaskOutcome::Canceled) => "cancelled",
            Some(TaskOutcome::Failed) => "failed",
            Some(TaskOutcome::Completed) | None => "completed",
        }
        .to_string();

        let outcome = events
            .iter()
            .rev()
            .find_map(|e| {
                (e.kind() == &EventKind::Terminal)
                    .then(|| e.outcome())
                    .flatten()
            })
            .map(|outcome| match outcome {
                TaskOutcome::Completed => TurnOutcome::Success,
                TaskOutcome::Failed => TurnOutcome::Failed(FailureClass::Other),
                TaskOutcome::Canceled => TurnOutcome::Canceled,
            })
            .unwrap_or(TurnOutcome::Success);
        self.observer.record(&ObsEvent::TurnFinished {
            ctx: &obs_ctx,
            latency: started.elapsed(),
            ttft,
            outcome: &outcome,
        });
        self.observer.record(&ObsEvent::UsageFinalized {
            ctx: &obs_ctx,
            usage: last_usage.as_ref(),
            fin: UsageFinalization::TurnFinal,
        });

        if let Some(attempt) = attempt.as_mut() {
            let (terminal_outcome, terminal_reason, degraded) = match outcome {
                TurnOutcome::Success => ("completed", "completed", false),
                TurnOutcome::Canceled => ("canceled", "canceled", true),
                TurnOutcome::Failed(_) => ("failed", "prompt_failed", true),
            };
            attempt
                .finish(terminal_outcome, terminal_reason, degraded, "complete")
                .await?;
        }

        Ok(TurnOutput {
            text: out_text,
            stop_reason,
            context: ctx,
        })
    }

    /// Submit a detached workflow run and return its durable task id.
    ///
    /// This compatibility entry point retains the historical start-on-call
    /// behavior: it mints identity internally, and the detached runner may
    /// begin before the awaiting caller observes the returned task id.
    pub async fn run_workflow(&self, p: OpParams) -> Result<TaskId, BridgeError> {
        Ok(self.run_workflow_inner(p, None).await?.task_id)
    }

    /// Submit a detached workflow under an identity already visible to the
    /// caller and return the exact admitted locator.
    ///
    /// ```compile_fail
    /// # use bridge_coordinator::{params::OpParams, Coordinator};
    /// # async fn optional_identity_is_rejected(coordinator: &Coordinator, p: OpParams) {
    /// let _ = coordinator.run_workflow_with_identity(p, None).await;
    /// # }
    /// ```
    pub async fn run_workflow_with_identity(
        &self,
        p: OpParams,
        identity: bridge_core::ids::AttemptIdentity,
    ) -> Result<WorkflowLocator, BridgeError> {
        self.run_workflow_inner(p, Some(identity)).await
    }

    async fn run_workflow_inner(
        &self,
        p: OpParams,
        supplied_identity: Option<bridge_core::ids::AttemptIdentity>,
    ) -> Result<WorkflowLocator, BridgeError> {
        if p.agent.is_some() || p.model.is_some() || p.effort.is_some() || p.mode.is_some() {
            return Err(BridgeError::InvalidRequest {
                field: "agent/model/effort/mode (run_workflow ignores overrides)",
            });
        }
        let wf = p
            .workflow
            .as_deref()
            .ok_or(BridgeError::InvalidRequest { field: "workflow" })?;
        let wf_id = WorkflowId::parse(wf)?;
        let graph = self
            .workflows
            .get(&wf_id)
            .cloned()
            .ok_or(BridgeError::InvalidRequest { field: "workflow" })?;
        let session_cwd = p.validate_cwd(self.allowed_cwd_root.as_ref())?;
        bridge_core::task_spec::validate_input(&p.input)?;

        let identity = match supplied_identity {
            Some(identity) if identity.ordinal == 0 && identity.parent_attempt_id.is_none() => {
                identity
            }
            Some(_) => {
                return Err(BridgeError::InvalidRequest {
                    field: "attempt linkage",
                });
            }
            None => bridge_core::ids::AttemptIdentity::initial()?,
        };
        let task = TaskId::parse(identity.execution_id.as_str().to_string())?;
        let now = self.clock.now_ms();
        let attempt_started = Instant::now();
        let context = p.context.clone();
        let token = CancellationToken::new();
        if let Some(context) = context.as_ref() {
            let mut runs = self.workflow_runs.lock().await;
            if runs.contains_key(context) {
                return Err(BridgeError::HandleBusy);
            }
            // The context points at the detached owner's token before durable
            // admission. SessionCancel during admission therefore cannot fall
            // into a registration gap, and a concurrent same-context submit
            // refuses before task, ledger, session, workflow, or provider effects.
            runs.insert(context.clone(), token.clone());
        }
        let input = p.input;
        let rec = TaskRecord {
            id: task.clone(),
            workflow: wf_id.as_str().to_string(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: now,
            updated_ms: now,
            last_artifact_ms: None,
            input: input.clone(),
            workflow_spec_json: Some(crate::detached::encode_workflow_spec(&graph)),
            resume_attempts: 0,
            session_cwd: session_cwd.as_ref().map(|c| c.as_str().to_string()),
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        };
        // The primary task row, current locator, and global attempt-identity
        // admission are one transaction. A colliding attempt therefore refuses
        // before provider, workflow, or optional-ledger effects.
        let initial_locator = bridge_core::task_store::TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        if let Err(error) = self
            .task_store
            .create_with_attempt_locator(&rec, &initial_locator)
            .await
        {
            if let Some(context) = context.as_ref() {
                self.workflow_runs.lock().await.remove(context);
            }
            return Err(error);
        }

        let (workload_fingerprint, workload_fingerprint_complete) =
            bridge_workflow::graph::workload_fingerprint(&graph, self.registry.as_ref());
        let reservation = bridge_core::workflow_history::AttemptReservation {
            identity: identity.clone(),
            task_id: Some(task.clone()),
            workflow: wf_id.as_str().to_string(),
            task_class: "workflow".into(),
            surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
            policy: "r2f0a".into(),
            workload_fingerprint,
            started_ms: now,
            workload_fingerprint_complete,
            prompt_acceptance: "not_dispatched".into(),
            pinned: false,
        };
        let (history, telemetry_unavailable) = match &self.workflow_history {
            Some(Ok(history)) => match history.reserve(&reservation).await {
                Ok(()) => (Some(history.clone()), None),
                Err(error)
                    if error.reason
                        == bridge_core::workflow_history::LedgerUnavailableReason::Collision =>
                {
                    let _ = self
                        .task_store
                        .mark_attempt_telemetry_unavailable(
                            &task,
                            &identity.attempt_id,
                            error.reason,
                        )
                        .await;
                    let _ = self
                        .task_store
                        .set_terminal(
                            &task,
                            TaskRecordStatus::Interrupted,
                            None,
                            Some("attempt identity collision"),
                            self.clock.now_ms(),
                        )
                        .await;
                    if let Some(context) = context.as_ref() {
                        self.workflow_runs.lock().await.remove(context);
                    }
                    return Err(BridgeError::DurableEvidenceUnavailable {
                        reason: error.reason.as_str(),
                    });
                }
                Err(error) => (None, Some(error.reason)),
            },
            Some(Err(reason)) => (None, Some(*reason)),
            None => (
                None,
                Some(bridge_core::workflow_history::LedgerUnavailableReason::Open),
            ),
        };
        let attempt_telemetry = AttemptTelemetryState::default();
        if let Some(reason) = telemetry_unavailable {
            attempt_telemetry.record(reason);
            self.observer.record_workflow(
                &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
            );
            tracing::warn!(
                reason = reason.as_str(),
                "workflow summary telemetry unavailable"
            );
            if let Err(error) = self
                .task_store
                .mark_attempt_telemetry_unavailable(&task, &identity.attempt_id, reason)
                .await
            {
                tracing::warn!(error = ?error, "workflow telemetry marker persistence failed");
            }
        }
        let locator = WorkflowLocator {
            task_id: task.clone(),
            execution_id: identity.execution_id.clone(),
            attempt_id: identity.attempt_id.clone(),
            attempt_ordinal: identity.ordinal,
            parent_attempt_id: identity.parent_attempt_id.clone(),
            telemetry_unavailable,
        };
        let hub = Arc::new(TaskProgressHub::new());
        self.progress_hubs
            .lock()
            .await
            .insert(task.clone(), hub.clone());
        self.workflow_cancels
            .lock()
            .await
            .insert(task.clone(), token.clone());
        // Preserve CancelTask's pre-admission latch. Once the task row exists,
        // later cancels address `workflow_cancels` directly; this closes the only
        // gap before that registration becomes visible.
        if self
            .session_store
            .cancel_requested(&task)
            .await
            .unwrap_or(false)
        {
            token.cancel();
        }
        let prompt_dispatch = history.as_ref().map(|history| {
            workflow_prompt_dispatch_barrier_with_state(
                history.clone(),
                self.task_store.clone(),
                task.clone(),
                identity.attempt_id.clone(),
                self.observer.clone(),
                attempt_telemetry.clone(),
            )
        });
        let terminal_barrier = workflow_terminal_summary_barrier(
            history,
            attempt_telemetry,
            self.task_store.clone(),
            task.clone(),
            identity.clone(),
            self.observer.clone(),
            wf_id.as_str().to_owned(),
            attempt_started,
            0,
        );
        let runner = spawn_detached_workflow_with_attempt_barriers(
            &self.detached_deps(),
            task.clone(),
            input,
            graph,
            identity.run_id().to_string(),
            token.clone(),
            HashMap::new(),
            WorkflowRunContext {
                session_cwd,
                task_id: Some(task.clone()),
                make_rich_sink: None,
                observer: self.observer.clone(),
                ..WorkflowRunContext::default()
            },
            hub,
            prompt_dispatch,
            Some(terminal_barrier),
        );
        if let Some(context) = context {
            let workflow_runs = self.workflow_runs.clone();
            tokio::spawn(async move {
                let _ = runner.await;
                // No successor can occupy this context before this removal: the
                // current entry remains the admission guard for the runner's
                // entire lifetime.
                workflow_runs.lock().await.remove(&context);
            });
        } else {
            // Dropping a Tokio JoinHandle detaches the durable runner.
            drop(runner);
        }

        Ok(locator)
    }

    /// Return one exact ledger attempt. This is the recovery/status authority for
    /// direct unary and MCP attempts that intentionally have no TaskRecord row.
    pub async fn attempt_status(
        &self,
        attempt: &bridge_core::ids::AttemptId,
    ) -> Result<bridge_core::workflow_history::AttemptRecord, BridgeError> {
        let history = match &self.workflow_history {
            Some(Ok(history)) => history,
            Some(Err(reason)) => {
                return Err(BridgeError::DurableEvidenceUnavailable {
                    reason: reason.as_str(),
                });
            }
            None => {
                return Err(BridgeError::DurableEvidenceUnavailable {
                    reason: bridge_core::workflow_history::LedgerUnavailableReason::Open.as_str(),
                });
            }
        };
        let row = history
            .attempt(attempt)
            .await
            .map_err(|error| BridgeError::DurableEvidenceUnavailable {
                reason: error.reason.as_str(),
            })?
            .ok_or(BridgeError::TaskNotFound)?;
        Ok(bridge_core::workflow_history::compatibility_project_attempt_record(row))
    }

    /// Return status for exactly one warm context or detached task.
    pub async fn status(
        &self,
        ctx: Option<ContextId>,
        task: Option<TaskId>,
    ) -> Result<StatusDto, BridgeError> {
        match (ctx, task) {
            (Some(_), Some(_)) => Err(BridgeError::InvalidRequest {
                field: "context|task_id (exactly one)",
            }),
            (None, None) => Err(BridgeError::InvalidRequest {
                field: "context|task_id (one required)",
            }),
            (Some(c), None) => {
                let info = self
                    .session_manager
                    .status(&c)
                    .await
                    .ok_or(BridgeError::SessionNotFound)?;
                Ok(StatusDto::Session(
                    self.session_status_dto(&c, &info).await?,
                ))
            }
            (None, Some(t)) => {
                let rec = self
                    .task_store
                    .get(&t)
                    .await?
                    .ok_or(BridgeError::TaskNotFound)?;
                Ok(StatusDto::Task(self.task_status_dto(&rec).await?))
            }
        }
    }

    async fn session_status_dto(
        &self,
        ctx: &ContextId,
        info: &crate::session_manager::SessionStatusInfo,
    ) -> Result<SessionStatusDto, BridgeError> {
        let mut dto = SessionStatusDto::from(info);
        if self.trace_refs_enabled {
            if let Some(row) = self.task_store.latest_turn_log_row_for_session(ctx).await? {
                dto.trace = Some(TraceRefs {
                    turn: Some(turn_ref(&row.turn_id)),
                    ..TraceRefs::default()
                });
            }
        }
        Ok(dto)
    }

    async fn task_status_dto(&self, rec: &TaskRecord) -> Result<TaskStatusDto, BridgeError> {
        let mut dto = TaskStatusDto::from(rec);
        if let Some(locator) = self.task_store.get_attempt_locator(&rec.id).await? {
            dto.execution_id = Some(locator.identity.execution_id);
            dto.attempt_id = Some(locator.identity.attempt_id);
            dto.attempt_ordinal = Some(locator.identity.ordinal);
            dto.parent_attempt_id = locator.identity.parent_attempt_id;
            dto.telemetry_unavailable = locator.telemetry_unavailable;
        }

        if let Some(agg) = self.task_store.turn_log_usage_for_task(&rec.id).await? {
            dto.usage = Some(UsageSnapshot {
                used: None,
                size: None,
                cost: agg.cost,
                terminal: Some(TerminalUsage {
                    total_tokens: agg.input_tokens + agg.output_tokens,
                    input_tokens: agg.input_tokens,
                    output_tokens: agg.output_tokens,
                    thought_tokens: agg.thought_tokens,
                    cached_read_tokens: agg.cached_read_tokens,
                    cached_write_tokens: agg.cached_write_tokens,
                }),
                at_ms: if agg.at_ms == 0 {
                    rec.updated_ms
                } else {
                    agg.at_ms
                },
            });
        }

        if self.trace_refs_enabled {
            let turn_rows = self
                .task_store
                .turn_log_rows_for_task(&rec.id, self.max_task_turns)
                .await?;
            let turns = if turn_rows.is_empty() {
                None
            } else {
                Some(turn_rows.iter().map(|row| turn_ref(&row.turn_id)).collect())
            };

            let nodes = self.task_store.node_checkpoint_nodes(&rec.id).await?;
            let artifacts = if nodes.is_empty() {
                None
            } else {
                Some(
                    nodes
                        .iter()
                        .map(|node| (node.as_str().to_string(), artifact_ref(&rec.id, node)))
                        .collect::<BTreeMap<_, _>>(),
                )
            };

            dto.trace = Some(TraceRefs {
                turn: None,
                turns,
                journal: Some(journal_ref(&rec.id)),
                artifacts,
            });
        }

        Ok(dto)
    }

    /// Clear a warm context and its children, rejecting while a workflow run owns the
    /// context. `force = true` aborts an in-flight warm turn (fires its abort token)
    /// instead of rejecting; `false` is the non-force clear (rejects a running turn).
    pub async fn clear(
        &self,
        ctx: ContextId,
        force: bool,
    ) -> Result<crate::session_manager::ResetOutcome, BridgeError> {
        let runs = self.workflow_runs.lock().await;
        if runs.contains_key(&ctx) {
            return Err(BridgeError::HandleBusy);
        }
        let result = self.session_manager.clear_with_children(&ctx, force).await;
        drop(runs);
        result
    }

    /// Cancel a detached task live when possible, then durably flip Working -> Canceled.
    pub async fn cancel_task(&self, id: TaskId) -> Result<bool, BridgeError> {
        if let Some(tok) = self.workflow_cancels.lock().await.get(&id) {
            tok.cancel();
        }
        self.task_store
            .cancel_if_working(&id, self.clock.now_ms())
            .await
    }

    /// Shutdown hook for stdin EOF: cancel live detached work and release all warm sessions.
    pub async fn shutdown(&self) {
        let toks: Vec<(TaskId, CancellationToken)> = self
            .workflow_cancels
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, tok) in toks {
            tok.cancel();
            let _ = self
                .task_store
                .cancel_if_working(&id, self.clock.now_ms())
                .await;
        }
        self.session_manager.release_all().await;
    }

    /// Boot-time detached task resume.
    pub async fn resume(&self) -> Result<(), BridgeError> {
        self.resume_with_cap(self.resume_attempt_cap).await
    }

    /// Run the same attempt-first boot sequence with an explicit resume cap.
    /// Kept public for the legacy inbound test/operator adapter; production serve
    /// uses `resume()` and the configured cap.
    #[doc(hidden)]
    pub async fn resume_with_cap(&self, cap: u32) -> Result<(), BridgeError> {
        // Hidden terminal rows from a prior crash must reconcile before any
        // checkpoint scan or conservative active-summary interruption.
        if !reconcile_pending_terminal_projections(&self.detached_deps()).await {
            tracing::warn!("pending terminal projection reconciliation incomplete; resume refused");
            return Err(BridgeError::StoreFailure);
        }
        if !reconcile_terminal_checkpoints(&self.detached_deps()).await {
            tracing::warn!("terminal-checkpoint attempt reconciliation incomplete; resume refused");
            return Err(BridgeError::StoreFailure);
        }
        if let Some(Ok(history)) = &self.workflow_history {
            let excluded = match self
                .task_store
                .terminal_attempts_with_telemetry_markers()
                .await
            {
                Ok(excluded) => excluded,
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "terminal telemetry-marker scan failed; resume refused"
                    );
                    return Err(BridgeError::StoreFailure);
                }
            };
            if let Err(error) = history
                .interrupt_active_excluding(self.clock.now_ms(), &excluded)
                .await
            {
                self.observer.record_workflow(
                    &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable {
                        reason: error.reason,
                    },
                );
                tracing::warn!(
                    reason = error.reason.as_str(),
                    "workflow history boot reconciliation unavailable; resume refused"
                );
                // Re-running a provider while its prior summary is still active
                // would create two live attempts. Leave primary tasks Working so
                // a later healthy boot can reconcile and resume safely.
                return Err(BridgeError::StoreFailure);
            }
        }
        match self.batch_deps() {
            Some(bdeps) => crate::batch::resume_all(&bdeps, cap).await,
            None => resume_working_tasks(&self.detached_deps(), cap).await,
        }
        Ok(())
    }
}

/// Records telemetry on a dropped coordinator turn, while the nested shared completion guard owns the
/// exact finish/cancel/expire fallback. `collect_turn` settles synchronously on ordinary paths and then
/// disarms this wrapper; cancellation mid-drain drops the nested guard without retaining this telemetry
/// observer in any detached cleanup flight.
struct TurnFinishGuard {
    observer: Arc<dyn Observer>,
    ctx: TurnContext,
    started: Instant,
    armed: bool,
    usage: Arc<std::sync::Mutex<Option<UsageSnapshot>>>,
    completion: Option<WarmCompletionGuard>,
}

impl TurnFinishGuard {
    fn observe_exit(&mut self, exit: WarmCompletionExit<'_>) {
        if let Some(completion) = self.completion.as_mut() {
            completion.observe_exit(exit);
        }
    }

    async fn complete(&mut self) -> Result<(), BridgeError> {
        match self.completion.take() {
            Some(completion) => completion.complete().await,
            None => Ok(()),
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TurnFinishGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let ctx = self.ctx.clone();
        let observer = self.observer.clone();
        let started = self.started;
        observer.record(&ObsEvent::TurnFinished {
            ctx: &ctx,
            latency: started.elapsed(),
            ttft: None,
            outcome: &TurnOutcome::Canceled,
        });
        let usage = self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        observer.record(&ObsEvent::UsageFinalized {
            ctx: &ctx,
            usage: usage.as_ref(),
            fin: UsageFinalization::TurnFinal,
        });
        // Dropping the still-armed shared completion guard starts its exact
        // generation/operation-bound fallback without retaining this telemetry
        // observer in the detached cleanup task.
        drop(self.completion.take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::session_manager::SessionManager;
    use async_trait::async_trait;
    use bridge_core::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition,
    };
    use bridge_core::domain::{
        AgentEntry, AgentKind, Effort, Part, PeerTaskId, PendingRequest, PermissionDecision,
        PermissionRequest, RegistrySnapshot, SessionContext,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, ContextId, NodeId, SessionId};
    use bridge_core::orch::{TerminalUsage, UsageCost, UsageSnapshot};
    use bridge_core::ports::{
        AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, Lease, Resolved,
        TurnContext, TurnOutcome, Update,
    };
    use bridge_core::task_store::{
        MemoryTaskStore, TaskAttemptLocator, TaskRecord, TaskRecordStatus, TaskStore,
        TurnLogFinalized, TurnLogFinished, TurnUsageFinalization,
    };
    use bridge_core::workflow_history::{
        AttemptReservation, AttemptTerminal, CompletedAttempt, LedgerError,
        LedgerUnavailableReason, MemoryWorkflowHistoryStore, TerminalWrite, WorkflowHistoryStore,
    };
    use bridge_workflow::graph::WorkflowNode;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::Notify;

    struct NoopLease;
    impl Lease for NoopLease {}

    struct FakeBackend {
        prompt_gate: Option<Arc<Notify>>,
        prompt_calls: AtomicUsize,
        release_calls: AtomicUsize,
        fail_release: std::sync::atomic::AtomicBool,
        release_gate: Option<Arc<Notify>>,
        configured_turns: Arc<StdMutex<Vec<(SessionId, TurnMeta)>>>,
    }

    impl FakeBackend {
        fn new(prompt_gate: Option<Arc<Notify>>) -> Self {
            Self {
                prompt_gate,
                prompt_calls: AtomicUsize::new(0),
                release_calls: AtomicUsize::new(0),
                fail_release: std::sync::atomic::AtomicBool::new(false),
                release_gate: None,
                configured_turns: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn with_blocked_release(release_gate: Arc<Notify>) -> Self {
            Self {
                prompt_gate: None,
                prompt_calls: AtomicUsize::new(0),
                release_calls: AtomicUsize::new(0),
                fail_release: std::sync::atomic::AtomicBool::new(false),
                release_gate: Some(release_gate),
                configured_turns: Arc::new(StdMutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompt_calls.fetch_add(1, AtomicOrdering::SeqCst);
            if let Some(gate) = &self.prompt_gate {
                gate.notified().await;
            }
            let updates = vec![Ok(Update::Done {
                stop_reason: "end_turn".into(),
            })];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn release_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.release_calls.fetch_add(1, AtomicOrdering::SeqCst);
            if let Some(gate) = &self.release_gate {
                gate.notified().await;
            }
            if self.fail_release.load(AtomicOrdering::SeqCst) {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }

        async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
            self.configured_turns
                .lock()
                .unwrap()
                .push((session.clone(), meta));
        }
    }

    struct FakeRegistry {
        entry: AgentEntry,
        backend: Arc<dyn AgentBackend>,
        resolved: Arc<StdMutex<Vec<AgentId>>>,
    }

    #[async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            self.resolved.lock().unwrap().push(id.clone());
            if *id != self.entry.id {
                return Err(BridgeError::UnknownAgent {
                    id: id.as_str().into(),
                });
            }
            Ok(Resolved {
                entry: Arc::new(self.entry.clone()),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            self.entry.id.clone()
        }
        fn configured_effective(
            &self,
            id: &AgentId,
        ) -> Option<bridge_core::domain::EffectiveConfig> {
            (*id == self.entry.id).then(|| bridge_core::domain::effective_config(&self.entry, None))
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.entry.id.clone()]
        }
    }

    struct NoEffectRegistry {
        default_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AgentRegistry for NoEffectRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("colliding direct admission must not resolve the registry")
        }

        fn default_id(&self) -> AgentId {
            self.default_calls.fetch_add(1, AtomicOrdering::SeqCst);
            AgentId::parse("codex").unwrap()
        }

        fn configured_effective(
            &self,
            _id: &AgentId,
        ) -> Option<bridge_core::domain::EffectiveConfig> {
            None
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![AgentId::parse("codex").unwrap()]
        }
    }

    struct ScriptedBackend {
        text: String,
        usage: Option<UsageSnapshot>,
        prompts: StdMutex<Vec<(SessionId, Vec<Part>)>>,
    }

    impl ScriptedBackend {
        fn new(text: &str) -> Self {
            Self {
                text: text.into(),
                usage: None,
                prompts: StdMutex::new(Vec::new()),
            }
        }

        fn with_usage(text: &str, usage: UsageSnapshot) -> Self {
            Self {
                text: text.into(),
                usage: Some(usage),
                prompts: StdMutex::new(Vec::new()),
            }
        }

        fn prompt_sessions(&self) -> Vec<SessionId> {
            self.prompts
                .lock()
                .unwrap()
                .iter()
                .map(|(session, _)| session.clone())
                .collect()
        }
    }

    #[async_trait]
    impl AgentBackend for ScriptedBackend {
        async fn prompt(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompts.lock().unwrap().push((session.clone(), parts));
            let mut updates = vec![Ok(Update::Text(self.text.clone()))];
            if let Some(usage) = &self.usage {
                updates.push(Ok(Update::Usage(usage.clone())));
            }
            updates.push(Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }));
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// Emits each string as a separate `Update::Text` delta, then Done. This models a
    /// streaming agent; the translator accumulates these deltas into the final Artifact.
    struct DeltaBackend {
        deltas: Vec<String>,
    }

    #[async_trait]
    impl AgentBackend for DeltaBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let mut updates: Vec<Result<Update, BridgeError>> = self
                .deltas
                .iter()
                .map(|d| Ok(Update::Text(d.clone())))
                .collect();
            updates.push(Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }));
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// Emits text deltas without a terminal Done. The translator flushes these as Status
    /// events only, so coordinator text collection must fall back when no Artifact exists.
    struct NoDoneBackend {
        deltas: Vec<String>,
    }

    #[async_trait]
    impl AgentBackend for NoDoneBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates: Vec<Result<Update, BridgeError>> = self
                .deltas
                .iter()
                .map(|d| Ok(Update::Text(d.clone())))
                .collect();
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// Panics if `prompt` is ever called — proves the pre-first-poll abort never reaches `backend.prompt`.
    struct PanicOnPromptBackend;

    #[async_trait]
    impl AgentBackend for PanicOnPromptBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("backend.prompt must not be called when the turn was aborted pre-first-poll");
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ErrorBackend {
        error: BridgeError,
        releases: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AgentBackend for ErrorBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![Err(self.error.clone())])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn release_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.releases.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
    }

    fn structured_agent_failure(class: DiagnosticFailureClass) -> BridgeError {
        BridgeError::agent_failure(
            FailureDiagnostic::build_static_code(
                FailureDiagnosticInput {
                    failed_phase: DiagnosticPhase::PromptStream,
                    last_completed_phase: Some(DiagnosticPhase::PromptStart),
                    class,
                    disposition: FailureDisposition::Fatal,
                    code: "ignored".to_owned(),
                    summary: "bounded test failure".to_owned(),
                    causes: Vec::new(),
                    stderr_observed: false,
                    stderr_line_count: 0,
                    stderr_scope: None,
                    stderr_tail: None,
                    stderr_redaction: None,
                    retry_after_ms: None,
                    reset_at_ms: None,
                    prompt_may_have_been_accepted: true,
                },
                "test.warm.failure",
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
        )
    }

    #[derive(Default)]
    struct FakeSessionStore {
        sessions: StdMutex<HashMap<String, SessionId>>,
        pending: StdMutex<HashMap<String, PendingRequest>>,
        peers: StdMutex<HashMap<String, PeerTaskId>>,
        cancels: StdMutex<std::collections::HashSet<String>>,
        fanouts: StdMutex<std::collections::HashSet<String>>,
    }

    #[async_trait]
    impl SessionStore for FakeSessionStore {
        async fn put(&self, task: &TaskId, session: &SessionId) -> Result<(), BridgeError> {
            self.sessions
                .lock()
                .unwrap()
                .insert(task.as_str().into(), session.clone());
            Ok(())
        }

        async fn session_for(&self, task: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(self.sessions.lock().unwrap().get(task.as_str()).cloned())
        }

        async fn put_pending(
            &self,
            task: &TaskId,
            req: &PendingRequest,
        ) -> Result<(), BridgeError> {
            self.pending
                .lock()
                .unwrap()
                .insert(task.as_str().into(), req.clone());
            Ok(())
        }

        async fn take_pending(&self, task: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
            Ok(self.pending.lock().unwrap().remove(task.as_str()))
        }

        async fn set_peer_task(&self, task: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
            self.peers
                .lock()
                .unwrap()
                .insert(task.as_str().into(), peer.clone());
            Ok(())
        }

        async fn peer_task_for(&self, task: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
            Ok(self.peers.lock().unwrap().get(task.as_str()).cloned())
        }

        async fn request_cancel(&self, task: &TaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().insert(task.as_str().into());
            Ok(())
        }

        async fn cancel_requested(&self, task: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.cancels.lock().unwrap().contains(task.as_str()))
        }

        async fn set_fanout(&self, task: &TaskId) -> Result<(), BridgeError> {
            self.fanouts.lock().unwrap().insert(task.as_str().into());
            Ok(())
        }

        async fn is_fanout(&self, task: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.fanouts.lock().unwrap().contains(task.as_str()))
        }
    }

    #[derive(Default)]
    struct ReconciliationFailureHistory {
        inner: MemoryWorkflowHistoryStore,
    }

    #[async_trait]
    impl WorkflowHistoryStore for ReconciliationFailureHistory {
        async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
            self.inner.reserve(row).await
        }

        async fn mark_prompt_acceptance(
            &self,
            id: &bridge_core::ids::AttemptId,
            acceptance: &str,
        ) -> Result<(), LedgerError> {
            self.inner.mark_prompt_acceptance(id, acceptance).await
        }

        async fn terminalize(
            &self,
            id: &bridge_core::ids::AttemptId,
            terminal: &AttemptTerminal,
        ) -> Result<TerminalWrite, LedgerError> {
            self.inner.terminalize(id, terminal).await
        }
        async fn set_pinned(
            &self,
            id: &bridge_core::ids::AttemptId,
            pinned: bool,
        ) -> Result<bool, LedgerError> {
            self.inner.set_pinned(id, pinned).await
        }

        async fn interrupt_active(&self, _completed_ms: i64) -> Result<u64, LedgerError> {
            Err(LedgerError::new(LedgerUnavailableReason::Io))
        }

        async fn latest_reservation_for_task(
            &self,
            task: &TaskId,
        ) -> Result<Option<AttemptReservation>, LedgerError> {
            self.inner.latest_reservation_for_task(task).await
        }

        async fn completed_between(
            &self,
            start_ms: i64,
            end_ms: i64,
        ) -> Result<Vec<CompletedAttempt>, LedgerError> {
            self.inner.completed_between(start_ms, end_ms).await
        }
    }

    #[derive(Default)]
    struct PromptBarrierFailureHistory {
        inner: MemoryWorkflowHistoryStore,
        fail_terminal: std::sync::atomic::AtomicBool,
        conflict_terminal: std::sync::atomic::AtomicBool,
        commit_then_fail_terminal_once: std::sync::atomic::AtomicBool,
        mark_calls: AtomicUsize,
        terminal_calls: AtomicUsize,
    }

    #[derive(Default)]
    struct WorkflowBalanceObserver {
        started: AtomicUsize,
        finished: AtomicUsize,
        stopped: AtomicUsize,
        unavailable: AtomicUsize,
    }

    impl Observer for WorkflowBalanceObserver {
        fn record(&self, _event: &bridge_core::ports::ObsEvent<'_>) {}

        fn record_workflow(&self, event: &bridge_core::ports::WorkflowObsEvent<'_>) {
            match event {
                bridge_core::ports::WorkflowObsEvent::Started { .. } => {
                    self.started.fetch_add(1, AtomicOrdering::SeqCst);
                }
                bridge_core::ports::WorkflowObsEvent::Finished { .. } => {
                    self.finished.fetch_add(1, AtomicOrdering::SeqCst);
                }
                bridge_core::ports::WorkflowObsEvent::Stopped { .. } => {
                    self.stopped.fetch_add(1, AtomicOrdering::SeqCst);
                }
                bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { .. } => {
                    self.unavailable.fetch_add(1, AtomicOrdering::SeqCst);
                }
            }
        }
    }

    #[async_trait]
    impl WorkflowHistoryStore for PromptBarrierFailureHistory {
        async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
            self.inner.reserve(row).await
        }

        async fn mark_prompt_acceptance(
            &self,
            _id: &bridge_core::ids::AttemptId,
            _acceptance: &str,
        ) -> Result<(), LedgerError> {
            self.mark_calls.fetch_add(1, AtomicOrdering::SeqCst);
            Err(LedgerError::new(LedgerUnavailableReason::Io))
        }

        async fn terminalize(
            &self,
            id: &bridge_core::ids::AttemptId,
            terminal: &AttemptTerminal,
        ) -> Result<TerminalWrite, LedgerError> {
            self.terminal_calls.fetch_add(1, AtomicOrdering::SeqCst);
            if self.conflict_terminal.load(AtomicOrdering::SeqCst) {
                return Ok(TerminalWrite::Conflict);
            }
            if self
                .commit_then_fail_terminal_once
                .swap(false, AtomicOrdering::SeqCst)
            {
                self.inner.terminalize(id, terminal).await?;
                return Err(LedgerError::new(LedgerUnavailableReason::Io));
            }
            if self.fail_terminal.load(AtomicOrdering::SeqCst) {
                Err(LedgerError::new(LedgerUnavailableReason::Io))
            } else {
                self.inner.terminalize(id, terminal).await
            }
        }
        async fn set_pinned(
            &self,
            id: &bridge_core::ids::AttemptId,
            pinned: bool,
        ) -> Result<bool, LedgerError> {
            self.inner.set_pinned(id, pinned).await
        }

        async fn interrupt_active(&self, completed_ms: i64) -> Result<u64, LedgerError> {
            self.inner.interrupt_active(completed_ms).await
        }

        async fn interrupt_active_excluding(
            &self,
            completed_ms: i64,
            excluded: &[bridge_core::ids::AttemptId],
        ) -> Result<u64, LedgerError> {
            self.inner
                .interrupt_active_excluding(completed_ms, excluded)
                .await
        }

        async fn latest_reservation_for_task(
            &self,
            task: &TaskId,
        ) -> Result<Option<AttemptReservation>, LedgerError> {
            self.inner.latest_reservation_for_task(task).await
        }

        async fn completed_between(
            &self,
            start_ms: i64,
            end_ms: i64,
        ) -> Result<Vec<CompletedAttempt>, LedgerError> {
            self.inner.completed_between(start_ms, end_ms).await
        }
    }

    #[tokio::test]
    async fn successful_real_mcp_attempt_row_is_timing_incomplete_and_not_calibration_eligible() {
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let backend = Arc::new(FakeBackend::new(None));
        let observer = Arc::new(WorkflowBalanceObserver::default());
        let fixture = coordinator_fixture_with_backend_and_observer(
            Arc::new(HashMap::new()),
            backend,
            observer,
        );
        let coordinator = fixture
            .coordinator
            .with_workflow_history(Ok(history.clone()));
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let attempt_id = identity.attempt_id.clone();

        coordinator
            .prompt_with_identity(prompt_params("hi"), identity)
            .await
            .unwrap();

        let row = history
            .attempt(&attempt_id)
            .await
            .unwrap()
            .expect("real MCP attempt row exists");
        assert_eq!(
            row.reservation.surface,
            bridge_core::workflow_history::ExecutionSurface::Mcp
        );
        let terminal = row.terminal.expect("real MCP attempt terminalized");
        assert_eq!(terminal.work_ms, 0);
        assert!(terminal.phase_durations.is_empty());
        assert!(!terminal.telemetry_complete);

        let completed = history.completed_between(0, i64::MAX).await.unwrap();
        let report = bridge_core::workflow_history::report(0, i64::MAX, &completed);
        assert_eq!(report.sample_count, 1);
        assert_eq!(report.calibration_sample_count, 0);
        assert_eq!(report.excluded.get("telemetry_incomplete"), Some(&1));
    }

    #[tokio::test]
    async fn public_exact_recovery_projects_legacy_rows_from_custom_history_stores() {
        for surface in [
            bridge_core::workflow_history::ExecutionSurface::DirectUnary,
            bridge_core::workflow_history::ExecutionSurface::Mcp,
        ] {
            let history = Arc::new(MemoryWorkflowHistoryStore::new());
            let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
            let reservation = AttemptReservation {
                identity: identity.clone(),
                task_id: Some(
                    bridge_core::ids::TaskId::parse(identity.execution_id.as_str()).unwrap(),
                ),
                workflow: "direct".into(),
                task_class: "direct".into(),
                surface,
                policy: "r2f0a".into(),
                workload_fingerprint: "legacy-direct".into(),
                started_ms: 1_000,
                workload_fingerprint_complete: true,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            };
            history.reserve(&reservation).await.unwrap();
            let legacy = AttemptTerminal {
                completed_ms: 2_000,
                work_ms: 1_000,
                end_to_end_ms: 1_000,
                queue_ms: 0,
                cancellation_ms: 0,
                cleanup_ms: 0,
                finalization_ms: 0,
                outcome: "completed".into(),
                terminal_reason: "completed".into(),
                producer_terminal: "unknown".into(),
                final_message: "unknown".into(),
                process_liveness: "unknown".into(),
                terminal_evidence_capability: "unsupported".into(),
                terminal_evidence_version: "none".into(),
                terminal_evidence_source: "none".into(),
                terminal_evidence_complete: false,
                degraded: false,
                prompt_acceptance: "not_dispatched".into(),
                cleanup_disposition: "complete".into(),
                node_counts: bridge_core::workflow_history::NodeCounts::default(),
                phase_durations: vec![bridge_core::workflow_history::PhaseDuration {
                    phase: "work".into(),
                    duration_ms: 1_000,
                }],
                telemetry_complete: true,
                monotonic_clock: true,
            };
            history
                .terminalize(&identity.attempt_id, &legacy)
                .await
                .unwrap();
            assert!(
                history
                    .attempt(&identity.attempt_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .terminal
                    .unwrap()
                    .telemetry_complete,
                "the custom store deliberately returns the unprojected legacy row"
            );

            let coordinator = coordinator_fixture(Arc::new(HashMap::new()))
                .coordinator
                .with_workflow_history(Ok(history));
            let recovered = coordinator
                .attempt_status(&identity.attempt_id)
                .await
                .unwrap();
            let terminal = recovered.terminal.unwrap();
            assert_eq!(terminal.end_to_end_ms, 1_000);
            assert_eq!(terminal.work_ms, 0);
            assert!(terminal.phase_durations.is_empty());
            assert!(!terminal.telemetry_complete);
        }
    }

    #[tokio::test]
    async fn actual_prompt_barrier_failure_cleans_up_then_terminalizes_once() {
        for (fail_release, fail_terminal, expected_cleanup) in [
            (false, false, "complete"),
            (true, false, "failed"),
            (false, true, "none"),
        ] {
            let history = Arc::new(PromptBarrierFailureHistory::default());
            history
                .fail_terminal
                .store(fail_terminal, AtomicOrdering::SeqCst);
            let backend = Arc::new(FakeBackend::new(None));
            backend
                .fail_release
                .store(fail_release, AtomicOrdering::SeqCst);
            let observer = Arc::new(WorkflowBalanceObserver::default());
            let fixture = coordinator_fixture_with_backend_and_observer(
                Arc::new(HashMap::new()),
                backend.clone(),
                observer.clone(),
            );
            let coordinator = fixture
                .coordinator
                .with_workflow_history(Ok(history.clone()));
            let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();

            let error = match coordinator
                .prompt_with_identity(prompt_params("hi"), identity)
                .await
            {
                Ok(_) => panic!("prompt barrier failure unexpectedly succeeded"),
                Err(error) => error,
            };
            assert!(matches!(
                error,
                BridgeError::DurableEvidenceUnavailable { reason: "io" }
            ));
            assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
            assert_eq!(backend.release_calls.load(AtomicOrdering::SeqCst), 1);
            assert_eq!(history.mark_calls.load(AtomicOrdering::SeqCst), 1);
            let expected_terminal_calls = if fail_terminal { 2 } else { 1 };
            tokio::time::timeout(Duration::from_secs(1), async {
                while history.terminal_calls.load(AtomicOrdering::SeqCst) < expected_terminal_calls
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("caller Drop must settle its one asynchronous terminal retry");
            assert_eq!(
                history.terminal_calls.load(AtomicOrdering::SeqCst),
                expected_terminal_calls
            );
            assert_eq!(observer.started.load(AtomicOrdering::SeqCst), 1);
            assert_eq!(
                observer.finished.load(AtomicOrdering::SeqCst)
                    + observer.stopped.load(AtomicOrdering::SeqCst),
                1
            );

            let rows = history.completed_between(0, i64::MAX).await.unwrap();
            if fail_terminal {
                assert!(rows.is_empty());
                assert_eq!(observer.stopped.load(AtomicOrdering::SeqCst), 1);
            } else {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].terminal.outcome, "failed");
                assert_eq!(rows[0].terminal.terminal_reason, "prompt_barrier_failed");
                assert_eq!(rows[0].terminal.prompt_acceptance, "unknown");
                assert_eq!(rows[0].terminal.cleanup_disposition, expected_cleanup);
            }
        }
    }

    #[tokio::test]
    async fn canceled_real_mcp_prompt_uses_unknown_cleanup_and_one_drop_terminal() {
        let history = Arc::new(PromptBarrierFailureHistory::default());
        let release_gate = Arc::new(Notify::new());
        let backend = Arc::new(FakeBackend::with_blocked_release(release_gate.clone()));
        let observer = Arc::new(WorkflowBalanceObserver::default());
        let fixture = coordinator_fixture_with_backend_and_observer(
            Arc::new(HashMap::new()),
            backend.clone(),
            observer.clone(),
        );
        let coordinator = Arc::new(
            fixture
                .coordinator
                .with_workflow_history(Ok(history.clone())),
        );
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let attempt_id = identity.attempt_id.clone();

        let caller = tokio::spawn({
            let coordinator = coordinator.clone();
            async move {
                coordinator
                    .prompt_with_identity(prompt_params("hi"), identity)
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while backend.release_calls.load(AtomicOrdering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the real Coordinator caller must enter cleanup after the barrier failure");

        caller.abort();
        let join_error = match caller.await {
            Ok(_) => panic!("aborted Coordinator caller unexpectedly completed"),
            Err(error) => error,
        };
        assert!(join_error.is_cancelled());
        release_gate.notify_one();

        let terminal = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let rows = history.completed_between(0, i64::MAX).await.unwrap();
                if let Some(row) = rows.into_iter().next() {
                    break row.terminal;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("DirectAttemptHandle Drop must asynchronously terminalize the canceled caller");

        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(backend.release_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(history.mark_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(history.terminal_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(terminal.terminal_reason, "prompt_barrier_failed");
        assert_eq!(terminal.prompt_acceptance, "unknown");
        assert_eq!(terminal.cleanup_disposition, "unknown");
        assert!(terminal.degraded);
        assert!(!terminal.telemetry_complete);
        assert_eq!(observer.started.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(observer.finished.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(observer.stopped.load(AtomicOrdering::SeqCst), 1);

        let exact = history.inner.attempt(&attempt_id).await.unwrap().unwrap();
        assert_eq!(exact.terminal, Some(terminal));
    }

    #[tokio::test]
    async fn real_mcp_caller_drop_replays_ambiguous_terminal_exactly_once() {
        let history = Arc::new(PromptBarrierFailureHistory::default());
        history
            .commit_then_fail_terminal_once
            .store(true, AtomicOrdering::SeqCst);
        let backend = Arc::new(FakeBackend::new(None));
        let observer = Arc::new(WorkflowBalanceObserver::default());
        let fixture = coordinator_fixture_with_backend_and_observer(
            Arc::new(HashMap::new()),
            backend.clone(),
            observer.clone(),
        );
        let coordinator = fixture
            .coordinator
            .with_workflow_history(Ok(history.clone()));
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let attempt_id = identity.attempt_id.clone();

        let error = match coordinator
            .prompt_with_identity(prompt_params("hi"), identity)
            .await
        {
            Ok(_) => panic!("prompt barrier failure unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            BridgeError::DurableEvidenceUnavailable { reason: "io" }
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while history.terminal_calls.load(AtomicOrdering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the real caller's Drop path must retry the prepared terminal");

        let rows = history.completed_between(0, i64::MAX).await.unwrap();
        assert_eq!(rows.len(), 1, "the replay cannot create a second summary");
        let terminal = &rows[0].terminal;
        assert_eq!(terminal.terminal_reason, "prompt_barrier_failed");
        assert_eq!(terminal.prompt_acceptance, "unknown");
        assert_eq!(terminal.cleanup_disposition, "complete");
        assert!(terminal.degraded);
        assert!(!terminal.telemetry_complete);
        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(backend.release_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(history.mark_calls.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(history.terminal_calls.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(observer.started.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(observer.finished.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(observer.stopped.load(AtomicOrdering::SeqCst), 1);

        let exact = history.inner.attempt(&attempt_id).await.unwrap().unwrap();
        assert_eq!(exact.terminal.as_ref(), Some(terminal));
    }

    #[tokio::test]
    async fn terminal_summary_conflict_balances_direct_in_flight_once() {
        let history = Arc::new(PromptBarrierFailureHistory::default());
        history
            .conflict_terminal
            .store(true, AtomicOrdering::SeqCst);
        let observer = Arc::new(WorkflowBalanceObserver::default());
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let mut handle = admit_direct_attempt_with_history(
            Ok(history),
            observer.clone(),
            identity,
            bridge_core::workflow_history::ExecutionSurface::DirectUnary,
            "direct",
            "direct",
            bridge_core::workflow_history::fingerprint_workload_shape(b"conflict"),
            true,
            1,
            "caller_aborted",
        )
        .await
        .unwrap();

        assert!(matches!(
            handle
                .finish("failed", "prompt_failed", true, "not_needed")
                .await,
            Err(BridgeError::DurableEvidenceUnavailable {
                reason: "collision"
            })
        ));
        assert_eq!(observer.started.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(observer.finished.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(observer.stopped.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(observer.unavailable.load(AtomicOrdering::SeqCst), 1);

        drop(handle);
        assert_eq!(
            observer.stopped.load(AtomicOrdering::SeqCst),
            1,
            "Drop must not emit a second completion event"
        );
    }

    #[tokio::test]
    async fn shared_direct_admission_records_explicit_smoke_surface() {
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let attempt_id = identity.attempt_id.clone();
        let execution_id = identity.execution_id.clone();
        let mut handle = admit_direct_attempt_with_history(
            Ok(history.clone()),
            Arc::new(NoopObserver),
            identity,
            bridge_core::workflow_history::ExecutionSurface::Smoke,
            "smoke",
            "direct",
            "fixed_pong".into(),
            false,
            1,
            "smoke_aborted",
        )
        .await
        .unwrap();

        handle.mark_prompt_dispatch().await.unwrap();
        handle
            .finish_with_completeness("completed", "completed", false, "complete", false)
            .await
            .unwrap();

        let row = history.attempt(&attempt_id).await.unwrap().unwrap();
        assert_eq!(row.reservation.workflow, "smoke");
        assert_eq!(
            row.reservation.surface,
            bridge_core::workflow_history::ExecutionSurface::Smoke
        );
        assert_eq!(
            row.reservation.task_id.as_ref().map(TaskId::as_str),
            Some(execution_id.as_str())
        );
        let terminal = row.terminal.unwrap();
        assert_eq!(terminal.prompt_acceptance, "dispatch_uncertain");
        assert!(!terminal.telemetry_complete);
    }

    #[tokio::test]
    async fn direct_collision_refuses_before_default_registry_lookup() {
        let default_calls = Arc::new(AtomicUsize::new(0));
        let registry: Arc<dyn AgentRegistry> = Arc::new(NoEffectRegistry {
            default_calls: default_calls.clone(),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let coordinator = coordinator_fixture_with_registry(registry, clock)
            .with_workflow_history(Ok(history.clone()));
        let baseline_calls = default_calls.load(AtomicOrdering::SeqCst);
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task),
                workflow: "direct".into(),
                task_class: "direct".into(),
                surface: bridge_core::workflow_history::ExecutionSurface::Mcp,
                policy: "r2f0a".into(),
                workload_fingerprint: "existing".into(),
                started_ms: 1,
                workload_fingerprint_complete: false,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            })
            .await
            .unwrap();
        let params = OpParams {
            workflow: None,
            skill: None,
            input: "hello".into(),
            context: None,
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: Some("/tmp/repo".into()),
        };

        let error = coordinator
            .prompt_with_identity(params, identity)
            .await
            .err()
            .expect("the duplicate locator must refuse");
        assert!(matches!(
            error,
            BridgeError::DurableEvidenceUnavailable {
                reason: "collision"
            }
        ));
        assert_eq!(
            default_calls.load(AtomicOrdering::SeqCst),
            baseline_calls,
            "identity collision must refuse before the registry default is read"
        );
    }

    struct AllowPolicy;

    impl PolicyEngine for AllowPolicy {
        fn decide(
            &self,
            _req: &PermissionRequest,
            _ctx: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Ok(PermissionDecision::Approve)
        }
    }

    fn entry() -> AgentEntry {
        AgentEntry {
            id: AgentId::parse("codex").unwrap(),
            cmd: Some("codex".into()),
            base_url: None,
            api_key_env: None,
            args: Vec::new(),
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: Some(Effort::High),
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            mcp: Vec::new(),
            mcp_delivery: Default::default(),
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: Vec::new(),
            version: None,
            extensions: Default::default(),
        }
    }

    #[test]
    fn coordinator_constructs_with_full_state() {
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: Arc::new(FakeBackend::new(None)),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let session_manager = Arc::new(SessionManager::new_with_clock(
            registry.clone(),
            Duration::from_secs(60),
            clock.clone(),
        ));
        let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
        let policy: Arc<dyn PolicyEngine> = Arc::new(AllowPolicy);

        let coordinator = Coordinator::new(
            session_manager.clone(),
            None,
            Arc::new(HashMap::new()),
            task_store,
            session_store,
            policy,
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            Arc::new(NoopObserver),
            3,
        );

        assert!(Arc::ptr_eq(&coordinator.session_manager, &session_manager));
    }

    fn workflow(id: &str) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse(id).unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: Vec::new(),
                retry: None,
            }],
            panel: None,
        })
    }

    fn ctx(id: &str) -> ContextId {
        ContextId::parse(id).unwrap()
    }

    fn task(id: &str) -> TaskId {
        TaskId::parse(id).unwrap()
    }

    fn working_record(id: TaskId) -> TaskRecord {
        TaskRecord {
            id,
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 10,
            updated_ms: 10,
            last_artifact_ms: None,
            input: "input".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    struct Fixture {
        coordinator: Coordinator,
        task_store: Arc<MemoryTaskStore>,
    }

    /// Delegating primary store that refuses exactly the first recovered
    /// terminal projection write. This models a summary commit followed by a
    /// one-shot primary transaction failure at the boot boundary.
    struct OneShotPendingTaskStore {
        inner: MemoryTaskStore,
        fail_pending_once: std::sync::atomic::AtomicBool,
        fail_scan_once: std::sync::atomic::AtomicBool,
    }

    impl OneShotPendingTaskStore {
        fn new() -> Self {
            Self {
                inner: MemoryTaskStore::new(),
                fail_pending_once: std::sync::atomic::AtomicBool::new(true),
                fail_scan_once: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn fail_next_scan(&self) {
            self.fail_scan_once
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl TaskStore for OneShotPendingTaskStore {
        async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
            self.inner.create(rec).await
        }

        async fn create_with_attempt_locator(
            &self,
            rec: &TaskRecord,
            locator: &TaskAttemptLocator,
        ) -> Result<(), BridgeError> {
            self.inner.create_with_attempt_locator(rec, locator).await
        }

        async fn set_terminal(
            &self,
            id: &TaskId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            updated_ms: i64,
        ) -> Result<(), BridgeError> {
            self.inner
                .set_terminal(id, status, result, error, updated_ms)
                .await
        }

        async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
            self.inner.get(id).await
        }

        async fn mark_attempt_telemetry_unavailable(
            &self,
            task: &TaskId,
            attempt: &bridge_core::ids::AttemptId,
            reason: LedgerUnavailableReason,
        ) -> Result<(), BridgeError> {
            self.inner
                .mark_attempt_telemetry_unavailable(task, attempt, reason)
                .await
        }

        async fn get_attempt_locator(
            &self,
            task: &TaskId,
        ) -> Result<Option<TaskAttemptLocator>, BridgeError> {
            self.inner.get_attempt_locator(task).await
        }

        async fn terminal_attempts_with_telemetry_markers(
            &self,
        ) -> Result<Vec<bridge_core::ids::AttemptId>, BridgeError> {
            self.inner.terminal_attempts_with_telemetry_markers().await
        }

        async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
            self.inner.list(limit).await
        }

        async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
            self.inner.sweep_interrupted(updated_ms).await
        }

        async fn cancel_if_working(
            &self,
            id: &TaskId,
            updated_ms: i64,
        ) -> Result<bool, BridgeError> {
            self.inner.cancel_if_working(id, updated_ms).await
        }

        async fn put_node_checkpoint(
            &self,
            task: &TaskId,
            node: &NodeId,
            output: &str,
            ok: bool,
            ts: i64,
        ) -> Result<(), BridgeError> {
            self.inner
                .put_node_checkpoint(task, node, output, ok, ts)
                .await
        }

        async fn node_checkpoints(
            &self,
            task: &TaskId,
        ) -> Result<
            Vec<(
                NodeId,
                String,
                bool,
                Option<bridge_core::orch::UsageSnapshot>,
            )>,
            BridgeError,
        > {
            self.inner.node_checkpoints(task).await
        }

        async fn claim_resume_attempt(
            &self,
            task: &TaskId,
            cap: u32,
            now_ms: i64,
        ) -> Result<bridge_core::task_store::ResumeClaim, BridgeError> {
            self.inner.claim_resume_attempt(task, cap, now_ms).await
        }

        async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
            self.inner.working_tasks().await
        }

        async fn record_node_started(
            &self,
            task: &TaskId,
            node: &NodeId,
            operation_id: &bridge_core::ids::OperationId,
            ts: i64,
        ) -> Result<i64, BridgeError> {
            self.inner
                .record_node_started(task, node, operation_id, ts)
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn put_node_checkpoint_sequenced(
            &self,
            task: &TaskId,
            node: &NodeId,
            operation_id: &bridge_core::ids::OperationId,
            output: &str,
            ok: bool,
            ts: i64,
            usage: Option<&bridge_core::orch::UsageSnapshot>,
        ) -> Result<i64, BridgeError> {
            self.inner
                .put_node_checkpoint_sequenced(task, node, operation_id, output, ok, ts, usage)
                .await
        }

        async fn set_terminal_sequenced(
            &self,
            task: &TaskId,
            operation_id: &bridge_core::ids::OperationId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            ts: i64,
        ) -> Result<i64, BridgeError> {
            self.inner
                .set_terminal_sequenced(task, operation_id, status, result, error, ts)
                .await
        }

        #[allow(clippy::too_many_arguments)]
        async fn set_terminal_sequenced_pending(
            &self,
            task: &TaskId,
            operation_id: &bridge_core::ids::OperationId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            ts: i64,
            attempt_id: &bridge_core::ids::AttemptId,
            terminal: &AttemptTerminal,
        ) -> Result<i64, BridgeError> {
            if self
                .fail_pending_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .set_terminal_sequenced_pending(
                    task,
                    operation_id,
                    status,
                    result,
                    error,
                    ts,
                    attempt_id,
                    terminal,
                )
                .await
        }

        async fn pending_terminal_projection(
            &self,
            task: &TaskId,
        ) -> Result<Option<bridge_core::task_store::PendingTerminalProjection>, BridgeError>
        {
            self.inner.pending_terminal_projection(task).await
        }

        async fn pending_terminal_projections(
            &self,
        ) -> Result<Vec<bridge_core::task_store::PendingTerminalProjection>, BridgeError> {
            if self
                .fail_scan_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(BridgeError::StoreFailure);
            }
            self.inner.pending_terminal_projections().await
        }

        async fn mark_terminal_projection_ready(
            &self,
            task: &TaskId,
            attempt_id: &bridge_core::ids::AttemptId,
        ) -> Result<(), BridgeError> {
            self.inner
                .mark_terminal_projection_ready(task, attempt_id)
                .await
        }

        async fn journal_from(
            &self,
            task: &TaskId,
            after_seq: i64,
        ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
            self.inner.journal_from(task, after_seq).await
        }

        async fn progress_snapshot(
            &self,
            task: &TaskId,
        ) -> Result<bridge_core::task_store::TaskProgressSnapshot, BridgeError> {
            self.inner.progress_snapshot(task).await
        }
    }
    fn coordinator_fixture(workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>) -> Fixture {
        coordinator_fixture_with_backend(workflows, Arc::new(FakeBackend::new(None)))
    }

    fn coordinator_fixture_with_backend(
        workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>,
        backend: Arc<FakeBackend>,
    ) -> Fixture {
        coordinator_fixture_with_backend_and_observer(workflows, backend, Arc::new(NoopObserver))
    }

    fn coordinator_fixture_with_backend_and_observer(
        workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>,
        backend: Arc<FakeBackend>,
        observer: Arc<dyn Observer>,
    ) -> Fixture {
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend,
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let session_manager = Arc::new(SessionManager::new_with_clock(
            registry.clone(),
            Duration::from_secs(60),
            clock.clone(),
        ));
        let task_store = Arc::new(MemoryTaskStore::new());
        let task_store_dyn: Arc<dyn TaskStore> = task_store.clone();
        let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
        let policy: Arc<dyn PolicyEngine> = Arc::new(AllowPolicy);
        let executor = Arc::new(WorkflowExecutor::new(registry.clone()));
        let coordinator = Coordinator::new(
            session_manager,
            Some(executor),
            workflows,
            task_store_dyn,
            session_store,
            policy,
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            observer,
            3,
        );
        Fixture {
            coordinator,
            task_store,
        }
    }

    fn workflow_params() -> OpParams {
        OpParams {
            workflow: Some("code-review".into()),
            skill: None,
            input: typed_code_review_input().into(),
            context: None,
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: Some("/tmp/repo".into()),
        }
    }

    fn typed_code_review_input() -> &'static str {
        "---\ntask-type: code-review\n---\n# Review task\n\n## Description\nReview the change.\n\n## Acceptance Criteria\n- Report findings\n"
    }

    fn prompt_params(input: &str) -> OpParams {
        OpParams {
            workflow: None,
            skill: None,
            input: input.into(),
            context: None,
            agent: Some(AgentId::parse("codex").unwrap()),
            model: None,
            effort: None,
            mode: None,
            cwd: Some("/tmp/repo".into()),
        }
    }

    fn coordinator_fixture_with_registry(
        registry: Arc<dyn AgentRegistry>,
        clock: Arc<dyn Clock>,
    ) -> Coordinator {
        let session_manager = Arc::new(SessionManager::new_with_clock(
            registry.clone(),
            Duration::from_secs(60),
            clock.clone(),
        ));
        let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
        let policy: Arc<dyn PolicyEngine> = Arc::new(AllowPolicy);
        Coordinator::new(
            session_manager,
            None,
            Arc::new(HashMap::new()),
            task_store,
            session_store,
            policy,
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            Arc::new(NoopObserver),
            3,
        )
    }

    #[derive(Default)]
    struct ObserverPathBackend {
        prompts: StdMutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait]
    impl AgentBackend for ObserverPathBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("coordinator must use the composite prompt path")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            assert!(observers.rich.is_none());
            self.prompts.lock().unwrap().push(observers.diagnostic);
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                stop_reason: "end_turn".into(),
            })])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ObserverPathRegistry {
        backend: Arc<ObserverPathBackend>,
        resolutions: StdMutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait]
    impl AgentRegistry for ObserverPathRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("coordinator checkout must use observed resolution")
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.resolutions.lock().unwrap().push(observer);
            Ok(Resolved {
                entry: Arc::new({
                    let mut entry = entry();
                    entry.id = id.clone();
                    entry
                }),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.default_id()]
        }
    }

    #[tokio::test]
    async fn prompt_and_continue_thread_one_fresh_operation_observer() {
        let backend = Arc::new(ObserverPathBackend::default());
        let registry = Arc::new(ObserverPathRegistry {
            backend: backend.clone(),
            resolutions: StdMutex::new(Vec::new()),
        });
        let coordinator = coordinator_fixture_with_registry(
            registry.clone(),
            Arc::new(ManualClock::new(1_700_000_000_000)),
        );

        let first = coordinator.prompt(prompt_params("first")).await.unwrap();
        let first_resolution = registry.resolutions.lock().unwrap()[0].clone();
        let first_prompt = backend.prompts.lock().unwrap()[0].clone();
        assert!(
            Arc::ptr_eq(&first_resolution, &first_prompt),
            "prompt must use the checkout observer for collection"
        );

        let mut continuation = prompt_params("second");
        continuation.context = Some(first.context);
        let _ = coordinator.continue_turn(continuation).await.unwrap();

        let resolutions = registry.resolutions.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        assert_eq!(resolutions.len(), 1, "warm continue must not re-resolve");
        assert_eq!(prompts.len(), 2);
        assert!(
            !Arc::ptr_eq(&prompts[0], &prompts[1]),
            "each coordinator operation owns a fresh observer"
        );
    }

    #[tokio::test]
    async fn prompt_warm_returns_text_and_context() {
        let backend = Arc::new(ScriptedBackend::with_usage(
            "backend text",
            UsageSnapshot {
                used: Some(7),
                size: Some(10),
                cost: None,
                terminal: None,
                at_ms: 0,
            },
        ));
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: backend.clone(),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);

        let out = coordinator.prompt(prompt_params("hello")).await.unwrap();

        assert_eq!(out.text, "backend text");
        assert_eq!(out.stop_reason, "completed");
        assert!(!out.context.as_str().is_empty());
        let status = coordinator
            .session_manager
            .status(&out.context)
            .await
            .unwrap();
        assert_eq!(status.usage.used, Some(7));
        assert_eq!(status.usage.at_ms, 1_700_000_000_000);
    }

    fn perm_key(ctx: &str, request_id: &str) -> bridge_core::permission::PermKey {
        bridge_core::permission::PermKey {
            context_id: ContextId::parse(ctx).unwrap(),
            generation: 3,
            op: OperationId::parse("turn-3").unwrap(),
            request_id: request_id.into(),
        }
    }

    fn pending_view(request_id: &str) -> bridge_core::permission::PendingPermissionView {
        bridge_core::permission::PendingPermissionView {
            request_id: request_id.into(),
            tool_call_id: "tool-1".into(),
            generation: 3,
            op: OperationId::parse("turn-3").unwrap(),
            title: "write file".into(),
            options: Vec::new(),
            raw_input: None,
            timeout_ms: 120_000,
        }
    }

    fn permit_params(
        ctx: &str,
        request_id: &str,
        decision: bridge_core::domain::PermitDecision,
    ) -> crate::params::PermitParams {
        crate::params::PermitParams {
            context: ContextId::parse(ctx).unwrap(),
            generation: 3,
            op: OperationId::parse("turn-3").unwrap(),
            request_id: request_id.into(),
            decision,
        }
    }

    #[tokio::test]
    async fn apply_permit_escalate_does_not_resolve() {
        let reg = bridge_core::permission::PermissionRegistry::new();
        let ctx = ContextId::parse("ctx-escalate").unwrap();
        let key = perm_key("ctx-escalate", "req-escalate");
        let (mut rx, _guard) = reg.register(key, pending_view("req-escalate"));

        let resolved = apply_permit(
            &reg,
            &permit_params(
                "ctx-escalate",
                "req-escalate",
                bridge_core::domain::PermitDecision::Escalate {
                    reason: Some("human".into()),
                },
            ),
        );

        assert!(!resolved);
        assert_eq!(reg.pending(&ctx).len(), 1);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn apply_permit_approve_resolves() {
        let reg = bridge_core::permission::PermissionRegistry::new();
        let key = perm_key("ctx-approve", "req-approve");
        let (rx, _guard) = reg.register(key, pending_view("req-approve"));

        let resolved = apply_permit(
            &reg,
            &permit_params(
                "ctx-approve",
                "req-approve",
                bridge_core::domain::PermitDecision::Approve {
                    option_id: Some("approved".into()),
                },
            ),
        );

        assert!(resolved);
        match rx.await.unwrap() {
            bridge_core::permission::PermissionResolution::Decided(
                bridge_core::domain::PermitDecision::Approve { option_id },
            ) => assert_eq!(option_id.as_deref(), Some("approved")),
            other => panic!("unexpected permission resolution: {other:?}"),
        }
    }

    #[test]
    fn apply_permit_unknown_request_false() {
        let reg = bridge_core::permission::PermissionRegistry::new();
        let resolved = apply_permit(
            &reg,
            &permit_params(
                "ctx-missing",
                "missing",
                bridge_core::domain::PermitDecision::Approve { option_id: None },
            ),
        );

        assert!(!resolved);
    }

    #[tokio::test]
    async fn prompt_default_agent_when_unset() {
        let backend = Arc::new(ScriptedBackend::new("default text"));
        let resolved = Arc::new(StdMutex::new(Vec::new()));
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend,
            resolved: resolved.clone(),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);
        let mut p = prompt_params("hello");
        p.agent = None;

        let out = coordinator.prompt(p).await.unwrap();

        assert_eq!(out.text, "default text");
        assert_eq!(
            resolved.lock().unwrap().as_slice(),
            &[AgentId::parse("codex").unwrap()]
        );
    }

    #[tokio::test]
    async fn prompt_returns_full_text_from_streamed_chunks() {
        // s8 T10 live-gate: a delta-streaming agent (Text "OAK","LE","AF") must yield the FULL reply,
        // NOT the last delta. The translator's terminal Artifact carries the full text, which
        // `collect_turn` consumes directly.
        let backend = Arc::new(DeltaBackend {
            deltas: vec!["OAK".into(), "LE".into(), "AF".into()],
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend,
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);

        let out = coordinator.prompt(prompt_params("hi")).await.unwrap();
        assert_eq!(out.text, "OAKLEAF");

        // Same contract at translator chunk-cap scale (max_chunk = 1200): a single
        // large delta spanning multiple Status chunks must still surface as one
        // FULL Artifact text from Coordinator::prompt, not a truncated value.
        let expected_large = "z".repeat(3_001);
        let large_backend = Arc::new(DeltaBackend {
            deltas: vec![expected_large.clone()],
        });
        let large_registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: large_backend,
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let large_coordinator = coordinator_fixture_with_registry(large_registry, clock);

        let out = large_coordinator.prompt(prompt_params("hi")).await.unwrap();
        assert_eq!(out.text, expected_large);
    }

    #[tokio::test]
    async fn prompt_falls_back_to_status_text_when_stream_ends_without_done() {
        let backend = Arc::new(NoDoneBackend {
            deltas: vec!["OAK".into(), "LEAF".into()],
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend,
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);

        let out = coordinator.prompt(prompt_params("hi")).await.unwrap();
        assert_eq!(out.text, "OAKLEAF");
        assert_eq!(out.stop_reason, "completed");
    }

    #[tokio::test]
    async fn continue_reuses_the_same_warm_context() {
        let backend = Arc::new(ScriptedBackend::new("remembered codeword"));
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: backend.clone(),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);

        let first = coordinator.prompt(prompt_params("first")).await.unwrap();
        let mut next = prompt_params("second");
        next.context = Some(first.context.clone());
        let second = coordinator.continue_turn(next).await.unwrap();

        assert_eq!(second.context, first.context);
        assert_eq!(second.text, "remembered codeword");
        let sessions = backend.prompt_sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0], sessions[1]);
    }

    #[tokio::test]
    async fn continue_without_context_is_invalid() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));

        assert!(matches!(
            fixture
                .coordinator
                .continue_turn(prompt_params("hello"))
                .await,
            Err(BridgeError::InvalidRequest { field: "context" })
        ));
    }

    #[tokio::test]
    async fn continue_inherits_stored_cwd_fingerprint() {
        // s8 T10 review BLOCKER: a context minted by `run` WITH a cwd must be continuable with the
        // advertised `{input, context}` shape. `continue` omits cwd/agent/overrides, so it must reuse
        // the context's STORED fingerprint — NOT re-derive (cwd=None) and trip `ConfigMismatch{cwd}`.
        let backend = Arc::new(ScriptedBackend::new("continued"));
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: backend.clone(),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let coordinator = coordinator_fixture_with_registry(registry, clock);

        // `run` with a cwd (prompt_params sets cwd = /tmp/repo, agent = codex).
        let first = coordinator.prompt(prompt_params("first")).await.unwrap();

        // `continue` with ONLY context + input — no cwd, no agent, no overrides.
        let cont = OpParams {
            workflow: None,
            skill: None,
            input: "second".into(),
            context: Some(first.context.clone()),
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
        };
        let second = coordinator.continue_turn(cont).await.unwrap();
        assert_eq!(second.context, first.context);
        assert_eq!(second.text, "continued");
    }

    #[tokio::test]
    async fn continue_unknown_context_is_session_not_found() {
        // `continue` must NOT mint a fresh session for an unknown context (that is `run`'s job).
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let cont = OpParams {
            workflow: None,
            skill: None,
            input: "x".into(),
            context: Some(ctx("ctx-nope")),
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
        };
        assert!(matches!(
            fixture.coordinator.continue_turn(cont).await,
            Err(BridgeError::SessionNotFound)
        ));
    }

    #[tokio::test]
    async fn collect_turn_pre_cancelled_abort_never_prompts() {
        // cancel-tokens F2 / L1: when the abort token is ALREADY cancelled at collect_turn's first poll,
        // the biased select takes the abort arm → events.next() is never polled → backend.prompt never
        // runs (the no-re-mint proof). PanicOnPromptBackend panics if prompt is called; reaching the
        // assertion proves it was not. The turn surfaces as "cancelled".
        let coordinator = coordinator_fixture(Arc::new(HashMap::new())).coordinator;
        let abort = CancellationToken::new();
        abort.cancel();
        let turn = crate::session_manager::WarmTurn {
            backend: Arc::new(PanicOnPromptBackend) as Arc<dyn AgentBackend>,
            session: SessionId::parse("ctx-abort-g1").unwrap(),
            usage_warning: None,
            generation: bridge_core::ids::SessionGeneration::new(1),
            op: OperationId::parse("turn-1").unwrap(),
            expiry_intent: crate::session_manager::WarmExpiryIntent::new(),
            seed: None,
            injects: Vec::new(),
            abort,
            agent: AgentId::parse("codex").unwrap(),
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            mode: Some("default".into()),
        };
        let out = coordinator
            .collect_turn(ctx("ctx-abort"), turn, "hi".into())
            .await
            .unwrap();
        assert_eq!(out.stop_reason, "cancelled");
    }

    #[tokio::test]
    async fn coordinator_expires_every_sampled_structured_warm_failure_but_preserves_legacy_policy()
    {
        for (index, class) in [
            DiagnosticFailureClass::Transport,
            DiagnosticFailureClass::AgentProcess,
            DiagnosticFailureClass::Timeout,
        ]
        .into_iter()
        .enumerate()
        {
            let releases = Arc::new(AtomicUsize::new(0));
            let backend: Arc<dyn AgentBackend> = Arc::new(ErrorBackend {
                error: structured_agent_failure(class),
                releases: releases.clone(),
            });
            let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
                entry: entry(),
                backend,
                resolved: Arc::new(StdMutex::new(Vec::new())),
            });
            let coordinator = coordinator_fixture_with_registry(
                registry,
                Arc::new(ManualClock::new(1_700_000_000_000)),
            );
            let context = ctx(&format!("ctx-structured-{index}"));
            let mut params = prompt_params("fail");
            params.context = Some(context.clone());

            assert!(matches!(
                coordinator.prompt(params).await,
                Err(BridgeError::AgentFailure { .. })
            ));
            assert_eq!(releases.load(AtomicOrdering::SeqCst), 1, "{class:?}");
            assert!(coordinator.session_manager.status(&context).await.is_none());
        }

        let releases = Arc::new(AtomicUsize::new(0));
        let backend: Arc<dyn AgentBackend> = Arc::new(ErrorBackend {
            error: BridgeError::agent_crashed("legacy"),
            releases: releases.clone(),
        });
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend,
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let coordinator = coordinator_fixture_with_registry(
            registry,
            Arc::new(ManualClock::new(1_700_000_000_000)),
        );
        let context = ctx("ctx-legacy-error-policy");
        let mut params = prompt_params("fail");
        params.context = Some(context.clone());
        assert!(matches!(
            coordinator.prompt(params).await,
            Err(BridgeError::AgentCrashed { .. })
        ));
        assert_eq!(releases.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            coordinator
                .session_manager
                .status(&context)
                .await
                .unwrap()
                .state,
            "idle",
            "coordinator legacy errors retain their pre-R2b finish behavior"
        );
    }

    #[cfg(test)]
    mod observability_boundary_tests {
        use super::*;
        use bridge_core::domain::{AgentEntry, AgentKind, Effort, Part};
        use bridge_core::ids::{AgentId, ContextId, SessionId};
        use bridge_core::orch::{TerminalUsage, UsageSnapshot};
        use bridge_core::ports::{
            AgentBackend, AgentRegistry, BackendStream, Lease, ObsEvent, Observer, Resolved,
            TurnContext, TurnOutcome, Update,
        };
        use bridge_core::task_store::{MemoryTaskStore, TaskStore};
        use futures::stream;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Debug)]
        enum RecordedObsEvent {
            Start(TurnContext),
            Finish {
                ctx: TurnContext,
                outcome: TurnOutcome,
            },
            UsageFinalized {
                ctx: TurnContext,
                has_usage: bool,
            },
        }

        #[derive(Default)]
        struct RecordingObserver(Mutex<Vec<RecordedObsEvent>>);

        impl Observer for RecordingObserver {
            fn record(&self, e: &ObsEvent<'_>) {
                let mut g = self.0.lock().unwrap();
                match e {
                    ObsEvent::TurnStarted { ctx } => {
                        g.push(RecordedObsEvent::Start((*ctx).clone()))
                    }
                    ObsEvent::TurnFinished { ctx, outcome, .. } => {
                        g.push(RecordedObsEvent::Finish {
                            ctx: (*ctx).clone(),
                            outcome: (*outcome).clone(),
                        })
                    }
                    ObsEvent::UsageFinalized { ctx, usage, .. } => {
                        g.push(RecordedObsEvent::UsageFinalized {
                            ctx: (*ctx).clone(),
                            has_usage: usage.is_some(),
                        })
                    }
                    _ => {}
                }
            }
        }

        struct NoopLease;
        impl Lease for NoopLease {}

        struct FakeRegistry {
            backend: Arc<dyn AgentBackend>,
        }

        #[async_trait::async_trait]
        impl AgentRegistry for FakeRegistry {
            async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(AgentEntry {
                        id: AgentId::parse("codex").unwrap(),
                        cmd: Some("fake".to_string()),
                        base_url: None,
                        api_key_env: None,
                        args: vec![],
                        kind: AgentKind::Acp,
                        model_provider: None,
                        model: Some("gpt-5.5".to_string()),
                        effort: Some(Effort::High),
                        mode: Some("default".to_string()),
                        cwd: None,
                        session_cwd: None,
                        sandbox: None,
                        watchdog: None,
                        mcp: vec![],
                        mcp_delivery: Default::default(),
                        auth_method: None,
                        pre_authenticated: false,
                        host_fallback_eligible: false,
                        name: None,
                        description: None,
                        tags: vec![],
                        version: None,
                        extensions: Default::default(),
                    }),
                    backend: self.backend.clone(),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }
            async fn apply(
                &self,
                _snapshot: bridge_core::domain::RegistrySnapshot,
            ) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![self.default_id()]
            }
        }

        struct UsageBackend;

        #[async_trait::async_trait]
        impl AgentBackend for UsageBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(stream::iter(vec![
                    Ok(Update::Usage(UsageSnapshot {
                        used: Some(3),
                        size: Some(10),
                        cost: None,
                        terminal: Some(TerminalUsage {
                            total_tokens: 5,
                            input_tokens: 2,
                            output_tokens: 3,
                            thought_tokens: None,
                            cached_read_tokens: None,
                            cached_write_tokens: None,
                        }),
                        at_ms: 0,
                    })),
                    Ok(bridge_core::ports::Update::Text("hello".to_string())),
                    Ok(bridge_core::ports::Update::Done {
                        stop_reason: "end_turn".to_string(),
                    }),
                ])))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        #[tokio::test]
        async fn coordinator_collect_turn_emits_started_finished_and_usage_once() {
            let observer = Arc::new(RecordingObserver::default());
            let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
                backend: Arc::new(UsageBackend),
            });
            let sm = Arc::new(crate::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            ));
            let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
            let session_store: Arc<dyn bridge_core::ports::SessionStore> =
                Arc::new(super::FakeSessionStore::default());
            let coord = Coordinator::new(
                sm,
                None,
                Arc::new(std::collections::HashMap::new()),
                task_store,
                session_store,
                Arc::new(super::AllowPolicy),
                registry,
                Arc::new(crate::clock::SystemClock),
                None,
                None,
                observer.clone(),
                3,
            );

            let out = coord
                .prompt(OpParams {
                    input: "hi".to_string(),
                    context: Some(ContextId::parse("ctx-obs").unwrap()),
                    agent: Some(AgentId::parse("codex").unwrap()),
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    workflow: None,
                    skill: None,
                })
                .await
                .unwrap();

            assert_eq!(out.text, "hello");
            let events = observer.0.lock().unwrap().clone();
            let starts: Vec<TurnContext> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::Start(ctx) => Some(ctx.clone()),
                    _ => None,
                })
                .collect();
            let finishes: Vec<(TurnContext, TurnOutcome)> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::Finish { ctx, outcome } => {
                        Some((ctx.clone(), outcome.clone()))
                    }
                    _ => None,
                })
                .collect();
            let usages: Vec<(TurnContext, bool)> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::UsageFinalized { ctx, has_usage } => {
                        Some((ctx.clone(), *has_usage))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(starts.len(), 1);
            assert_eq!(finishes.len(), 1);
            assert_eq!(usages.len(), 1);
            assert_eq!(starts[0].turn_id, finishes[0].0.turn_id);
            assert_eq!(starts[0].turn_id, usages[0].0.turn_id);
            assert!(usages[0].1);
            let start_idx = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::Start(_)))
                .expect("start event");
            let finish_idx = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::Finish { .. }))
                .expect("finish event");
            assert!(start_idx < finish_idx);
            assert_eq!(starts[0].agent, "codex");
            assert_eq!(starts[0].model.as_deref(), Some("gpt-5.5"));
            assert_eq!(starts[0].effort.as_deref(), Some("high"));
            assert_eq!(starts[0].mode.as_deref(), Some("default"));
            assert_eq!(finishes[0].1, TurnOutcome::Success);
        }

        /// Backend that yields a Usage update then blocks forever (pending), so the
        /// guard fires mid-turn with captured usage.
        struct UsageThenIdleBackend {
            usage: UsageSnapshot,
        }

        #[async_trait::async_trait]
        impl AgentBackend for UsageThenIdleBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                let usage = self.usage.clone();
                // Yield the usage update then block forever so the future must be dropped.
                let once = futures::stream::once(async move { Ok(Update::Usage(usage)) });
                let pending = futures::stream::pending::<Result<Update, BridgeError>>();
                Ok(Box::pin(once.chain(pending)))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct PendingBackend;

        #[async_trait::async_trait]
        impl AgentBackend for PendingBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(futures::stream::pending()))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        fn in_flight(observer: &bridge_observ::PrometheusObserver) -> i64 {
            observer
                .endpoint()
                .render()
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("bridge_turns_in_flight "))
                .expect("in-flight gauge is registered")
                .parse()
                .expect("in-flight gauge is an integer")
        }

        #[tokio::test]
        async fn direct_prompt_metric_matches_running_then_idle_lifecycle() {
            let observer = Arc::new(
                bridge_observ::PrometheusObserver::new(bridge_observ::LabelVocabulary::default())
                    .unwrap(),
            );
            let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
                backend: Arc::new(PendingBackend),
            });
            let sm = Arc::new(crate::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            ));
            let coord = Arc::new(Coordinator::new(
                sm,
                None,
                Arc::new(std::collections::HashMap::new()),
                Arc::new(MemoryTaskStore::new()),
                Arc::new(super::FakeSessionStore::default()),
                Arc::new(super::AllowPolicy),
                registry,
                Arc::new(crate::clock::SystemClock),
                None,
                None,
                observer.clone(),
                3,
            ));
            let context = ContextId::parse("ctx-metric-running").unwrap();
            let params = OpParams {
                input: "hold".into(),
                context: Some(context.clone()),
                agent: Some(AgentId::parse("codex").unwrap()),
                model: None,
                effort: None,
                mode: None,
                cwd: None,
                workflow: None,
                skill: None,
            };

            let running_coord = coord.clone();
            let running = tokio::spawn(async move { running_coord.prompt(params).await });
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if in_flight(&observer) == 1
                        && coord
                            .session_manager
                            .status(&context)
                            .await
                            .is_some_and(|status| status.state == "running")
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("direct turn must become visible as running and in-flight");

            running.abort();
            let _ = running.await;
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if in_flight(&observer) == 0
                        && coord
                            .session_manager
                            .status(&context)
                            .await
                            .is_some_and(|status| status.state == "idle")
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("dropped direct turn must return to idle and leave in-flight accounting");
        }

        #[tokio::test]
        async fn collect_turn_dropped_with_usage_emits_canceled_and_usage_finalized() {
            let usage_snap = UsageSnapshot {
                used: Some(5),
                size: Some(100),
                cost: None,
                terminal: Some(TerminalUsage {
                    total_tokens: 5,
                    input_tokens: 2,
                    output_tokens: 3,
                    thought_tokens: None,
                    cached_read_tokens: None,
                    cached_write_tokens: None,
                }),
                at_ms: 0,
            };
            let observer = Arc::new(RecordingObserver::default());
            let backend = Arc::new(UsageThenIdleBackend {
                usage: usage_snap.clone(),
            });
            let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
                backend: backend as Arc<dyn AgentBackend>,
            });
            let sm = Arc::new(crate::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            ));
            let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
            let session_store: Arc<dyn bridge_core::ports::SessionStore> =
                Arc::new(super::FakeSessionStore::default());
            let coord = Arc::new(Coordinator::new(
                sm,
                None,
                Arc::new(std::collections::HashMap::new()),
                task_store,
                session_store,
                Arc::new(super::AllowPolicy),
                registry,
                Arc::new(crate::clock::SystemClock),
                None,
                None,
                observer.clone(),
                3,
            ));

            let ctx_id = ContextId::parse("ctx-drop-usage").unwrap();
            let turn = coord
                .session_manager
                .checkout_turn(&ctx_id, AgentId::parse("codex").unwrap(), None, None)
                .await
                .unwrap();

            let c2 = coord.clone();
            let handle = tokio::spawn(async move {
                let _ = c2.collect_turn(ctx_id, turn, "hi".into()).await;
            });

            // Wait for TurnStarted + Usage to be processed (usage update is recorded
            // in shared_usage before the translator yields the next event).
            for _ in 0..1000 {
                if observer
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| matches!(e, RecordedObsEvent::Start(_)))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            // Sleep briefly to let the Usage update propagate into shared_usage before abort.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            handle.abort();
            let _ = handle.await;

            // Wait for TurnFinished to appear.
            for _ in 0..1000 {
                if observer
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|e| matches!(e, RecordedObsEvent::Finish { .. }))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }

            let events = observer.0.lock().unwrap().clone();
            let starts = events
                .iter()
                .filter(|e| matches!(e, RecordedObsEvent::Start(_)))
                .count();
            let finishes: Vec<_> = events
                .iter()
                .filter_map(|e| match e {
                    RecordedObsEvent::Finish { outcome, .. } => Some(outcome.clone()),
                    _ => None,
                })
                .collect();
            let usages = events
                .iter()
                .filter(|e| matches!(e, RecordedObsEvent::UsageFinalized { .. }))
                .count();

            assert_eq!(starts, 1, "expected 1 TurnStarted");
            assert_eq!(
                finishes.len(),
                1,
                "expected 1 TurnFinished; got: {events:?}"
            );
            assert_eq!(
                finishes[0],
                TurnOutcome::Canceled,
                "outcome must be Canceled"
            );
            assert_eq!(
                usages, 1,
                "guard must emit UsageFinalized for captured usage; got: {events:?}"
            );

            // Order: TurnFinished before UsageFinalized.
            let finish_pos = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::Finish { .. }))
                .expect("finish event");
            let usage_pos = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::UsageFinalized { .. }))
                .expect("usage event");
            assert!(
                finish_pos < usage_pos,
                "TurnFinished must precede UsageFinalized"
            );
        }

        #[tokio::test]
        async fn turn_finish_drop_guard_without_usage_emits_explicit_no_usage() {
            let observer = Arc::new(RecordingObserver::default());
            let backend = Arc::new(FakeBackend::new(Some(Arc::new(tokio::sync::Notify::new()))));
            let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry { backend });
            let sm = Arc::new(crate::session_manager::SessionManager::new(
                registry.clone(),
                std::time::Duration::from_secs(60),
            ));
            let task_store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
            let session_store: Arc<dyn bridge_core::ports::SessionStore> =
                Arc::new(super::FakeSessionStore::default());
            let coord = Arc::new(Coordinator::new(
                sm,
                None,
                Arc::new(std::collections::HashMap::new()),
                task_store,
                session_store,
                Arc::new(super::AllowPolicy),
                registry,
                Arc::new(crate::clock::SystemClock),
                None,
                None,
                observer.clone(),
                3,
            ));

            let ctx = ContextId::parse("ctx-obs-drop").unwrap();
            let turn = coord
                .session_manager
                .checkout_turn(&ctx, AgentId::parse("codex").unwrap(), None, None)
                .await
                .unwrap();

            let c2 = coord.clone();
            let handle = tokio::spawn(async move {
                let _ = c2.collect_turn(ctx, turn, "hi".into()).await;
            });

            for _ in 0..1000 {
                if observer
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, RecordedObsEvent::Start(_)))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            handle.abort();
            let _ = handle.await;

            for _ in 0..1000 {
                if observer
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, RecordedObsEvent::Finish { .. }))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
            let events = observer.0.lock().unwrap().clone();
            let starts: Vec<TurnContext> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::Start(ctx) => Some(ctx.clone()),
                    _ => None,
                })
                .collect();
            let finishes: Vec<(TurnContext, TurnOutcome)> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::Finish { ctx, outcome } => {
                        Some((ctx.clone(), outcome.clone()))
                    }
                    _ => None,
                })
                .collect();
            let usages: Vec<(TurnContext, bool)> = events
                .iter()
                .filter_map(|event| match event {
                    RecordedObsEvent::UsageFinalized { ctx, has_usage } => {
                        Some((ctx.clone(), *has_usage))
                    }
                    _ => None,
                })
                .collect();

            assert_eq!(starts.len(), 1);
            assert_eq!(finishes.len(), 1);
            assert_eq!(starts[0].turn_id, finishes[0].0.turn_id);
            assert_eq!(finishes[0].1, TurnOutcome::Canceled);
            assert_eq!(usages.len(), 1);
            assert_eq!(starts[0].turn_id, usages[0].0.turn_id);
            assert!(!usages[0].1);
            let start_idx = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::Start(_)))
                .expect("start event");
            let finish_idx = events
                .iter()
                .position(|e| matches!(e, RecordedObsEvent::Finish { .. }))
                .expect("finish event");
            assert!(start_idx < finish_idx);
            assert_eq!(starts[0].agent, "codex");
            assert_eq!(starts[0].model.as_deref(), Some("gpt-5.5"));
            assert_eq!(starts[0].effort.as_deref(), Some("high"));
            assert_eq!(starts[0].mode.as_deref(), Some("default"));
        }
    }

    #[tokio::test]
    async fn collect_turn_configures_turn_meta() {
        let coordinator = coordinator_fixture(Arc::new(HashMap::new())).coordinator;
        let backend = Arc::new(FakeBackend::new(None));
        let session = SessionId::parse("ctx-config-meta-g3").unwrap();
        let op = OperationId::parse("turn-config-meta").unwrap();
        let turn = crate::session_manager::WarmTurn {
            backend: backend.clone() as Arc<dyn AgentBackend>,
            session: session.clone(),
            usage_warning: None,
            generation: bridge_core::ids::SessionGeneration::new(3),
            op: op.clone(),
            expiry_intent: crate::session_manager::WarmExpiryIntent::new(),
            seed: None,
            injects: Vec::new(),
            abort: CancellationToken::new(),
            agent: AgentId::parse("codex").unwrap(),
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            mode: Some("default".into()),
        };

        let out = coordinator
            .collect_turn(ctx("ctx-config-meta"), turn, "hi".into())
            .await
            .unwrap();

        assert_eq!(out.stop_reason, "completed"); // collect_turn maps the backend's end_turn -> completed
        let configured = backend.configured_turns.lock().unwrap();
        assert_eq!(configured.len(), 1);
        assert_eq!(configured[0].0, session);
        assert_eq!(configured[0].1.context_id.as_str(), "ctx-config-meta");
        assert_eq!(configured[0].1.generation, 3);
        assert_eq!(configured[0].1.op, op);
    }

    #[tokio::test]
    async fn dropped_turn_returns_handle_to_idle() {
        // s8 T10 review MAJOR: a turn future dropped mid-drain must return the warm handle to Idle via
        // the drop guard — else the next turn on that context is permanently HandleBusy.
        let gate = Arc::new(Notify::new());
        let fixture = coordinator_fixture_with_backend(
            Arc::new(HashMap::new()),
            Arc::new(FakeBackend::new(Some(gate.clone()))),
        );
        let coord = Arc::new(fixture.coordinator);

        let known = ctx("ctx-drop");
        let mut p = prompt_params("first");
        p.context = Some(known.clone());

        let c2 = coord.clone();
        let handle = tokio::spawn(async move {
            let _ = c2.prompt(p).await;
        });

        // Wait until the turn has checked out (handle exists) and is blocked in the gated backend.
        for _ in 0..1000 {
            if coord.session_manager.status(&known).await.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        // Drop the prompt future mid-drain (the gate is never released).
        handle.abort();

        // The guard's spawned finish_turn returns the handle to Idle: poll until a re-checkout succeeds
        // (a stranded Running handle would stay HandleBusy forever and exhaust the loop).
        let mut released = false;
        for _ in 0..1000 {
            match coord.session_manager.checkout_existing_turn(&known).await {
                Ok(_) => {
                    released = true;
                    break;
                }
                Err(BridgeError::HandleBusy) => {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                Err(other) => panic!("unexpected checkout error: {other:?}"),
            }
        }
        assert!(
            released,
            "warm handle never returned to Idle after the turn future was dropped"
        );
    }

    #[tokio::test]
    async fn run_workflow_creates_durable_task_and_returns_id() {
        let gate = Arc::new(Notify::new());
        let mut workflows = HashMap::new();
        workflows.insert(
            WorkflowId::parse("code-review").unwrap(),
            workflow("code-review"),
        );
        let fixture = coordinator_fixture_with_backend(
            Arc::new(workflows),
            Arc::new(FakeBackend::new(Some(gate))),
        );

        let id: TaskId = fixture
            .coordinator
            .run_workflow(workflow_params())
            .await
            .unwrap();
        let rec = fixture.task_store.get(&id).await.unwrap().unwrap();

        assert_eq!(rec.id, id);
        assert_eq!(rec.workflow, "code-review");
        assert_eq!(rec.status, TaskRecordStatus::Working);
        assert_eq!(rec.input, typed_code_review_input());
        assert_eq!(rec.session_cwd.as_deref(), Some("/tmp/repo"));
        assert!(rec.workflow_spec_json.is_some());
        assert!(
            fixture.task_store.create(&rec).await.is_err(),
            "task creates must be non-clobbering"
        );
    }
    #[tokio::test]
    async fn healthy_workflow_persists_calibration_eligible_measurements() {
        let mut workflows = HashMap::new();
        workflows.insert(
            WorkflowId::parse("code-review").unwrap(),
            workflow("code-review"),
        );
        let Fixture {
            coordinator,
            task_store,
        } = coordinator_fixture(Arc::new(workflows));
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let coordinator = coordinator.with_workflow_history(Ok(history.clone()));

        let task_id: TaskId = coordinator.run_workflow(workflow_params()).await.unwrap();
        let rows = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let rows = history.completed_between(0, i64::MAX).await.unwrap();
                if !rows.is_empty() {
                    break rows;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("workflow summary terminalizes");
        assert_eq!(
            task_store.get(&task_id).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(row.reservation.workload_fingerprint_complete);
        assert!(row.terminal.telemetry_complete);
        assert!(row.terminal.monotonic_clock);
        assert_eq!(row.terminal.cleanup_disposition, "complete");
        assert_eq!(row.terminal.node_counts.completed, 1);
        assert_eq!(
            row.terminal.finalization_ms,
            row.terminal
                .end_to_end_ms
                .saturating_sub(row.terminal.queue_ms)
                .saturating_sub(row.terminal.work_ms)
                .saturating_sub(row.terminal.cleanup_ms)
        );
        assert!(row
            .terminal
            .phase_durations
            .iter()
            .any(|phase| phase.phase == "work"));
        assert!(row
            .terminal
            .phase_durations
            .iter()
            .any(|phase| phase.phase == "finalization"));

        let report = bridge_core::workflow_history::report(0, i64::MAX, &rows);
        assert_eq!(report.calibration_sample_count, 1);
    }

    #[tokio::test]
    async fn unavailable_optional_history_does_not_block_primary_workflow() {
        let mut workflows = HashMap::new();
        workflows.insert(
            WorkflowId::parse("code-review").unwrap(),
            workflow("code-review"),
        );
        let Fixture {
            coordinator,
            task_store,
        } = coordinator_fixture(Arc::new(workflows));
        let coordinator = coordinator.with_workflow_history(Err(LedgerUnavailableReason::Open));

        let supplied_identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let locator = coordinator
            .run_workflow_with_identity(workflow_params(), supplied_identity.clone())
            .await
            .unwrap();
        assert_eq!(
            locator.task_id.as_str(),
            supplied_identity.execution_id.as_str()
        );
        assert_eq!(locator.execution_id, supplied_identity.execution_id);
        assert_eq!(locator.attempt_id, supplied_identity.attempt_id);
        assert_eq!(locator.attempt_ordinal, supplied_identity.ordinal);
        assert_eq!(
            locator.parent_attempt_id,
            supplied_identity.parent_attempt_id
        );
        assert_eq!(
            locator.telemetry_unavailable,
            Some(LedgerUnavailableReason::Open)
        );
        let durable_locator = task_store
            .get_attempt_locator(&locator.task_id)
            .await
            .unwrap()
            .expect("primary task admission persists the locator");
        assert_eq!(durable_locator.identity.execution_id, locator.execution_id);
        assert_eq!(durable_locator.identity.attempt_id, locator.attempt_id);
        assert_eq!(
            durable_locator.telemetry_unavailable,
            Some(LedgerUnavailableReason::Open)
        );

        let terminal = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let record = task_store
                    .get(&locator.task_id)
                    .await
                    .unwrap()
                    .expect("primary task remains queryable");
                if record.status != TaskRecordStatus::Working {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("primary workflow completes without optional history");
        assert_eq!(terminal.status, TaskRecordStatus::Completed);
    }

    #[tokio::test]
    async fn run_workflow_rejects_untyped_input() {
        let mut workflows = HashMap::new();
        workflows.insert(
            WorkflowId::parse("code-review").unwrap(),
            workflow("code-review"),
        );
        let fixture = coordinator_fixture(Arc::new(workflows));
        let mut params = workflow_params();
        params.input = "bare workflow request".into();

        match fixture.coordinator.run_workflow(params).await {
            Err(BridgeError::TaskSpecInvalid { .. }) => {}
            Err(other) => panic!("expected TaskSpecInvalid, got {other:?}"),
            Ok(id) => panic!("expected TaskSpecInvalid, got Ok({id:?})"),
        }
    }

    #[tokio::test]
    async fn resume_reconciles_detached_and_batch_terminal_checkpoints_before_interrupting() {
        for (case, batch_id, checkpoint_ok, expected_status, expected_outcome) in [
            (
                "detached",
                None,
                true,
                TaskRecordStatus::Completed,
                "completed",
            ),
            (
                "batch",
                Some(bridge_core::ids::BatchId::parse("batch-checkpoint").unwrap()),
                false,
                TaskRecordStatus::Failed,
                "failed",
            ),
        ] {
            let graph = workflow("code-review");
            let mut workflows = HashMap::new();
            workflows.insert(graph.id.clone(), graph.clone());
            let backend = Arc::new(FakeBackend::new(None));
            let Fixture {
                coordinator,
                task_store,
            } = coordinator_fixture_with_backend(Arc::new(workflows), backend.clone());
            let history = Arc::new(MemoryWorkflowHistoryStore::new());
            let coordinator = coordinator.with_workflow_history(Ok(history.clone()));
            let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
            let task = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
            let locator = TaskAttemptLocator {
                identity: identity.clone(),
                telemetry_unavailable: None,
            };
            let mut record = working_record(task.clone());
            record.workflow = graph.id.as_str().to_owned();
            record.input = typed_code_review_input().to_owned();
            record.workflow_spec_json = Some(crate::detached::encode_workflow_spec(&graph));
            record.batch_id = batch_id;
            record.item_id = (case == "batch").then(|| "item-0".to_owned());
            task_store
                .create_with_attempt_locator(&record, &locator)
                .await
                .unwrap();
            history
                .reserve(&AttemptReservation {
                    identity: identity.clone(),
                    task_id: Some(task.clone()),
                    workflow: graph.id.as_str().to_owned(),
                    task_class: "workflow".into(),
                    surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                    policy: "r2f0a".into(),
                    workload_fingerprint: bridge_core::workflow_history::fingerprint_workload_shape(
                        b"checkpoint-recovery",
                    ),
                    started_ms: 1,
                    workload_fingerprint_complete: true,
                    prompt_acceptance: "dispatch_uncertain".into(),
                    pinned: false,
                })
                .await
                .unwrap();
            task_store
                .put_node_checkpoint(
                    &task,
                    &NodeId::parse("only").unwrap(),
                    "checkpoint-output",
                    checkpoint_ok,
                    2,
                )
                .await
                .unwrap();

            coordinator.resume().await.unwrap();

            let persisted = task_store.get(&task).await.unwrap().unwrap();
            assert_eq!(persisted.status, expected_status, "{case}");
            assert_eq!(
                persisted.resume_attempts, 0,
                "{case} must not mint a resume"
            );
            assert_eq!(
                task_store.get_attempt_locator(&task).await.unwrap(),
                Some(locator),
                "{case} keeps the original attempt locator"
            );
            assert_eq!(
                backend.prompt_calls.load(AtomicOrdering::SeqCst),
                0,
                "{case} checkpoint recovery cannot prompt"
            );
            let attempt = history
                .attempt(&identity.attempt_id)
                .await
                .unwrap()
                .unwrap();
            let terminal = attempt.terminal.expect("existing attempt terminalized");
            assert_eq!(terminal.outcome, expected_outcome, "{case}");
            assert_eq!(terminal.terminal_reason, "terminal_checkpoint_recovered");
            assert_ne!(terminal.outcome, "interrupted", "{case}");
        }
    }

    #[tokio::test]
    async fn checkpoint_summary_replays_after_one_shot_primary_failure_on_second_boot() {
        let graph = workflow("code-review");
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
        let locator = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        let store = Arc::new(OneShotPendingTaskStore::new());
        let mut record = working_record(task.clone());
        record.workflow = graph.id.as_str().to_owned();
        record.input = typed_code_review_input().to_owned();
        record.workflow_spec_json = Some(crate::detached::encode_workflow_spec(&graph));
        store
            .create_with_attempt_locator(&record, &locator)
            .await
            .unwrap();
        store
            .put_node_checkpoint(
                &task,
                &NodeId::parse("only").unwrap(),
                "stable-checkpoint-output",
                true,
                2,
            )
            .await
            .unwrap();

        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: graph.id.as_str().to_owned(),
                task_class: "workflow".into(),
                surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint: bridge_core::workflow_history::fingerprint_workload_shape(
                    b"checkpoint-primary-one-shot",
                ),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "dispatch_uncertain".into(),
                pinned: false,
            })
            .await
            .unwrap();
        let store_dyn: Arc<dyn TaskStore> = store.clone();
        let history_dyn: Arc<dyn WorkflowHistoryStore> = history.clone();
        let deps = crate::detached::DetachedDeps {
            task_store: store_dyn,
            executor: None,
            workflows: Arc::new(HashMap::new()),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(10)),
            observer: Arc::new(NoopObserver),
            workflow_history: Some(Ok(history_dyn)),
        };

        assert!(
            !crate::detached::reconcile_terminal_checkpoints(&deps).await,
            "the first boot must report the primary write failure"
        );
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );
        assert!(store
            .pending_terminal_projection(&task)
            .await
            .unwrap()
            .is_none());
        let first_terminal = history
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .expect("the first boot committed the recovered summary");

        assert!(crate::detached::reconcile_terminal_checkpoints(&deps).await);
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
        let replayed = history
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .expect("the second boot replays the same summary");
        assert_eq!(replayed, first_terminal);
        assert_eq!(replayed.terminal_reason, "terminal_checkpoint_recovered");
        assert_eq!(replayed.completed_ms, first_terminal.completed_ms);
    }
    #[tokio::test]
    async fn failed_checkpoint_summary_publishes_only_with_exact_primary_marker() {
        let graph = workflow("code-review");
        let mut workflows = HashMap::new();
        workflows.insert(graph.id.clone(), graph.clone());
        let backend = Arc::new(FakeBackend::new(None));
        let Fixture {
            coordinator,
            task_store,
        } = coordinator_fixture_with_backend(Arc::new(workflows), backend.clone());
        let history = Arc::new(PromptBarrierFailureHistory::default());
        history.fail_terminal.store(true, AtomicOrdering::SeqCst);
        let coordinator = coordinator.with_workflow_history(Ok(history.clone()));
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
        let locator = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        let mut record = working_record(task.clone());
        record.workflow = graph.id.as_str().to_owned();
        record.input = typed_code_review_input().to_owned();
        record.workflow_spec_json = Some(crate::detached::encode_workflow_spec(&graph));
        task_store
            .create_with_attempt_locator(&record, &locator)
            .await
            .unwrap();
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: graph.id.as_str().to_owned(),
                task_class: "workflow".into(),
                surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint: bridge_core::workflow_history::fingerprint_workload_shape(
                    b"checkpoint-terminalization-failure",
                ),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "dispatch_uncertain".into(),
                pinned: false,
            })
            .await
            .unwrap();
        task_store
            .put_node_checkpoint(
                &task,
                &NodeId::parse("only").unwrap(),
                "checkpoint-output",
                true,
                2,
            )
            .await
            .unwrap();

        coordinator.resume().await.unwrap();

        let persisted = task_store.get(&task).await.unwrap().unwrap();
        assert_eq!(persisted.status, TaskRecordStatus::Completed);
        assert_eq!(persisted.resume_attempts, 0);
        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
        let marked = task_store
            .get_attempt_locator(&task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(marked.identity, identity);
        assert_eq!(
            marked.telemetry_unavailable,
            Some(LedgerUnavailableReason::Io)
        );
        let attempt = history
            .inner
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            attempt.terminal.is_none(),
            "a bounded exact-attempt marker permits terminal projection without inventing a summary"
        );

        // A later boot does not silently backfill a workflow whose optional
        // summary failed after its exact primary marker became authoritative.
        history.fail_terminal.store(false, AtomicOrdering::SeqCst);
        coordinator.resume().await.unwrap();

        let persisted = task_store.get(&task).await.unwrap().unwrap();
        assert_eq!(persisted.status, TaskRecordStatus::Completed);
        assert_eq!(persisted.resume_attempts, 0);
        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
        let attempt = history
            .inner
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert!(attempt.terminal.is_none());
    }

    #[tokio::test]
    async fn resume_without_terminal_checkpoint_interrupts_then_mints_one_successor() {
        let graph = workflow("code-review");
        let mut workflows = HashMap::new();
        workflows.insert(graph.id.clone(), graph.clone());
        let backend = Arc::new(FakeBackend::new(None));
        let Fixture {
            coordinator,
            task_store,
        } = coordinator_fixture_with_backend(Arc::new(workflows), backend.clone());
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let coordinator = coordinator.with_workflow_history(Ok(history.clone()));
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
        let locator = TaskAttemptLocator {
            identity: identity.clone(),
            telemetry_unavailable: None,
        };
        let mut record = working_record(task.clone());
        record.workflow = graph.id.as_str().to_owned();
        record.input = typed_code_review_input().to_owned();
        record.workflow_spec_json = Some(crate::detached::encode_workflow_spec(&graph));
        task_store
            .create_with_attempt_locator(&record, &locator)
            .await
            .unwrap();
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: graph.id.as_str().to_owned(),
                task_class: "workflow".into(),
                surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint: bridge_core::workflow_history::fingerprint_workload_shape(
                    b"no-terminal-checkpoint",
                ),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "dispatch_uncertain".into(),
                pinned: false,
            })
            .await
            .unwrap();

        coordinator.resume().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if task_store
                    .get(&task)
                    .await
                    .unwrap()
                    .is_some_and(|record| record.status != TaskRecordStatus::Working)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("resumed provider attempt terminalizes");

        let original = history
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .unwrap();
        assert_eq!(original.outcome, "interrupted");
        assert_eq!(original.terminal_reason, "process_restart");
        let current = task_store
            .get_attempt_locator(&task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.identity.ordinal, 1);
        assert_eq!(
            current.identity.parent_attempt_id,
            Some(identity.attempt_id.clone())
        );
        assert_ne!(current.identity.attempt_id, identity.attempt_id);
        assert_eq!(
            task_store
                .get(&task)
                .await
                .unwrap()
                .unwrap()
                .resume_attempts,
            1
        );
        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 1);
    }

    /// #10 slice 4: `coordinator.resume()` is the serve boot-resume entry point that
    /// REPLACES the adapter's `resume_working_tasks`. It must scan the store and act on
    /// each `Working` task. A crashed-mid-run task with no workflow snapshot is
    /// unresumable → the resume scan finalizes it `Interrupted` (deterministic, no graph
    /// execution). This covers the coordinator's resume dispatcher over the (shared) store.
    #[tokio::test]
    async fn resume_interrupts_unresumable_working_task() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("resume-no-snapshot");
        fixture
            .task_store
            .create(&TaskRecord {
                id: id.clone(),
                workflow: "code-review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: String::new(),
                workflow_spec_json: None, // unresumable: no snapshot to reconstruct the graph
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();

        fixture.coordinator.resume().await.unwrap();

        let rec = fixture.task_store.get(&id).await.unwrap().unwrap();
        assert_eq!(
            rec.status,
            TaskRecordStatus::Interrupted,
            "coordinator.resume() must interrupt an unresumable working task"
        );
    }

    #[tokio::test]
    async fn reconciliation_failure_refuses_resume_before_slot_or_prompt() {
        let graph = workflow("code-review");
        let mut workflows = HashMap::new();
        workflows.insert(WorkflowId::parse("code-review").unwrap(), graph.clone());
        let backend = Arc::new(FakeBackend::new(None));
        let Fixture {
            coordinator,
            task_store,
        } = coordinator_fixture_with_backend(Arc::new(workflows), backend.clone());
        let coordinator = coordinator
            .with_workflow_history(Ok(Arc::new(ReconciliationFailureHistory::default())));

        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let id = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
        task_store
            .create(&TaskRecord {
                id: id.clone(),
                workflow: "code-review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: typed_code_review_input().into(),
                workflow_spec_json: Some(crate::detached::encode_workflow_spec(&graph)),
                resume_attempts: 0,
                session_cwd: Some("/tmp/repo".into()),
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();
        let locator = TaskAttemptLocator {
            identity,
            telemetry_unavailable: None,
        };
        task_store.put_attempt_locator(&id, &locator).await.unwrap();

        assert_eq!(coordinator.resume().await, Err(BridgeError::StoreFailure));

        let record = task_store.get(&id).await.unwrap().unwrap();
        assert_eq!(record.status, TaskRecordStatus::Working);
        assert_eq!(record.resume_attempts, 0);
        assert_eq!(
            task_store.get_attempt_locator(&id).await.unwrap(),
            Some(locator)
        );
        assert_eq!(backend.prompt_calls.load(AtomicOrdering::SeqCst), 0);
    }
    #[tokio::test]
    async fn pending_projection_scan_failure_closes_and_retries_serving_gate() {
        let store = Arc::new(OneShotPendingTaskStore::new());
        store.fail_next_scan();
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: Arc::new(FakeBackend::new(None)),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let session_manager = Arc::new(SessionManager::new_with_clock(
            registry.clone(),
            Duration::from_secs(60),
            clock.clone(),
        ));
        let task_store: Arc<dyn TaskStore> = store;
        let coordinator = Coordinator::new(
            session_manager,
            None,
            Arc::new(HashMap::new()),
            task_store,
            Arc::new(FakeSessionStore::default()),
            Arc::new(AllowPolicy),
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            Arc::new(NoopObserver),
            3,
        );

        assert_eq!(coordinator.resume().await, Err(BridgeError::StoreFailure));
        assert_eq!(
            coordinator.resume().await,
            Ok(()),
            "a later healthy boot may retry the serving gate"
        );
    }

    #[tokio::test]
    async fn status_context_xor_task_id() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-status");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        assert!(matches!(
            fixture
                .coordinator
                .status(Some(ctx("ctx-status")), Some(id.clone()))
                .await,
            Err(BridgeError::InvalidRequest { .. })
        ));
        assert!(matches!(
            fixture.coordinator.status(None, None).await,
            Err(BridgeError::InvalidRequest { .. })
        ));

        let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();
        let value = serde_json::to_value(dto).unwrap();
        assert_eq!(value["kind"], "task");
        assert_eq!(value["status"], "working");
    }

    #[test]
    fn trace_refs_skip_absent_fields() {
        let value = serde_json::to_value(TraceRefs::default()).unwrap();
        assert_eq!(value, serde_json::json!({}));
    }

    #[tokio::test]
    async fn task_status_dto_omits_usage_trace_when_none() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-no-rows");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                let value = serde_json::to_value(task).unwrap();
                assert!(value.get("usage").is_none());
                assert!(value.get("trace").is_none());
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    fn dto_turn_ctx(
        turn: &str,
        task: &str,
        completed_ms: i64,
    ) -> (TurnContext, TurnLogFinished, TurnLogFinalized) {
        let ctx = TurnContext {
            turn_id: bridge_core::ids::TurnId::parse(turn).unwrap(),
            session_id: ContextId::parse("ctx-dto").unwrap(),
            task_id: Some(TaskId::parse(task).unwrap()),
            workflow: Some("code-review".into()),
            node: Some("reviewer".into()),
            attempt: 0,
            agent: "codex".into(),
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            mode: None,
            prompt_id: Some("prompt/eval".into()),
            traceparent: None,
        };
        let finished = TurnLogFinished {
            ctx: ctx.clone(),
            started_ms: completed_ms - 10,
            completed_ms,
            latency: Duration::from_millis(10),
            ttft: None,
            outcome: TurnOutcome::Success,
        };
        let usage = TurnLogFinalized {
            ctx: ctx.clone(),
            finalization: TurnUsageFinalization::Usage(UsageSnapshot {
                used: Some(999),
                size: Some(1000),
                cost: Some(UsageCost {
                    amount: 0.50,
                    currency: "USD".into(),
                }),
                terminal: Some(TerminalUsage {
                    total_tokens: 9999,
                    input_tokens: 7,
                    output_tokens: 11,
                    thought_tokens: Some(3),
                    cached_read_tokens: Some(5),
                    cached_write_tokens: None,
                }),
                at_ms: completed_ms,
            }),
        };
        (ctx, finished, usage)
    }

    #[tokio::test]
    async fn task_usage_aggregates_from_turn_log_single_currency() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-usage");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        for (turn, completed_ms) in [("turn-a", 10), ("turn-b", 20)] {
            let (_ctx, finished, usage) = dto_turn_ctx(turn, id.as_str(), completed_ms);
            fixture
                .task_store
                .upsert_turn_finished(&finished)
                .await
                .unwrap();
            fixture
                .task_store
                .finalize_turn_usage(&usage)
                .await
                .unwrap();
        }

        let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                let usage = task.usage.unwrap();
                assert_eq!(usage.used, None);
                assert_eq!(usage.size, None);
                assert_eq!(usage.cost.as_ref().unwrap().currency, "USD");
                assert!((usage.cost.as_ref().unwrap().amount - 1.0).abs() < 0.000_001);
                let terminal = usage.terminal.unwrap();
                assert_eq!(terminal.input_tokens, 14);
                assert_eq!(terminal.output_tokens, 22);
                assert_eq!(terminal.thought_tokens, Some(6));
                assert_eq!(terminal.cached_read_tokens, Some(10));
                assert_eq!(terminal.cached_write_tokens, None);
                assert_eq!(usage.at_ms, 20);
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    #[tokio::test]
    async fn task_usage_omits_cost_for_mixed_currencies() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-mixed");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        let (_ctx, finished, usage) = dto_turn_ctx("turn-usd", id.as_str(), 10);
        fixture
            .task_store
            .upsert_turn_finished(&finished)
            .await
            .unwrap();
        fixture
            .task_store
            .finalize_turn_usage(&usage)
            .await
            .unwrap();

        let (_ctx, finished, mut usage2) = dto_turn_ctx("turn-eur", id.as_str(), 20);
        let TurnUsageFinalization::Usage(snapshot) = &mut usage2.finalization else {
            unreachable!("dto helper always creates usage finalization")
        };
        snapshot.cost = Some(UsageCost {
            amount: 0.25,
            currency: "EUR".into(),
        });
        fixture
            .task_store
            .upsert_turn_finished(&finished)
            .await
            .unwrap();
        fixture
            .task_store
            .finalize_turn_usage(&usage2)
            .await
            .unwrap();

        let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                let usage = task.usage.unwrap();
                assert!(usage.cost.is_none());
                assert_eq!(usage.terminal.unwrap().input_tokens, 14);
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    #[tokio::test]
    async fn task_usage_terminal_total_tokens_is_input_plus_output() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-total");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        let (_ctx, finished, usage) = dto_turn_ctx("turn-total", id.as_str(), 10);
        fixture
            .task_store
            .upsert_turn_finished(&finished)
            .await
            .unwrap();
        fixture
            .task_store
            .finalize_turn_usage(&usage)
            .await
            .unwrap();

        let dto = fixture.coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                let terminal = task.usage.unwrap().terminal.unwrap();
                assert_eq!(terminal.input_tokens, 7);
                assert_eq!(terminal.output_tokens, 11);
                assert_eq!(terminal.total_tokens, 18);
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    #[tokio::test]
    async fn trace_ref_segments_are_percent_encoded() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let coordinator = fixture.coordinator.with_trace_refs_config(true, 4);
        let id = TaskId::parse("task/with?chars").unwrap();
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        let (_ctx, finished, usage) = dto_turn_ctx("turn/with#chars", id.as_str(), 10);
        fixture
            .task_store
            .upsert_turn_finished(&finished)
            .await
            .unwrap();
        fixture
            .task_store
            .finalize_turn_usage(&usage)
            .await
            .unwrap();

        let dto = coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                let trace = task.trace.unwrap();
                assert_eq!(
                    trace.journal.unwrap(),
                    "/tasks/task%2Fwith%3Fchars/journal.jsonl"
                );
                assert_eq!(trace.turns.unwrap(), vec!["/turns/turn%2Fwith%23chars"]);
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    #[tokio::test]
    async fn task_trace_turn_refs_are_capped_but_usage_is_not() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let coordinator = fixture.coordinator.with_trace_refs_config(true, 2);
        let id = task("task-capped");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        for i in 0..3 {
            let (_ctx, finished, usage) = dto_turn_ctx(&format!("turn-{i}"), id.as_str(), 10 + i);
            fixture
                .task_store
                .upsert_turn_finished(&finished)
                .await
                .unwrap();
            fixture
                .task_store
                .finalize_turn_usage(&usage)
                .await
                .unwrap();
        }

        let dto = coordinator.status(None, Some(id)).await.unwrap();

        match dto {
            StatusDto::Task(task) => {
                assert_eq!(task.trace.unwrap().turns.unwrap().len(), 2);
                assert_eq!(task.usage.unwrap().terminal.unwrap().input_tokens, 21);
            }
            StatusDto::Session(_) => panic!("expected task status"),
        }
    }

    #[tokio::test]
    async fn session_status_includes_latest_warm_turn_trace_ref() {
        let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
            entry: entry(),
            backend: Arc::new(FakeBackend::new(None)),
            resolved: Arc::new(StdMutex::new(Vec::new())),
        });
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
        let session_manager = Arc::new(SessionManager::new_with_clock(
            registry.clone(),
            Duration::from_secs(60),
            clock.clone(),
        ));
        let task_store = Arc::new(MemoryTaskStore::new());
        let task_store_dyn: Arc<dyn TaskStore> = task_store.clone();
        let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
        let coordinator = Coordinator::new(
            session_manager.clone(),
            None,
            Arc::new(HashMap::new()),
            task_store_dyn,
            session_store,
            Arc::new(AllowPolicy),
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            Arc::new(NoopObserver),
            3,
        )
        .with_trace_refs_config(true, 4);

        let ctx = ContextId::parse("ctx-warm").unwrap();
        let turn = bridge_core::ids::TurnId::parse("turn-warm-latest").unwrap();
        let turn_ctx = TurnContext {
            turn_id: turn.clone(),
            session_id: ctx.clone(),
            task_id: None,
            workflow: None,
            node: None,
            attempt: 0,
            agent: "codex".into(),
            model: None,
            effort: None,
            mode: None,
            prompt_id: None,
            traceparent: None,
        };
        task_store
            .upsert_turn_finished(&TurnLogFinished {
                ctx: turn_ctx,
                started_ms: 10,
                completed_ms: 20,
                latency: Duration::from_millis(10),
                ttft: None,
                outcome: TurnOutcome::Success,
            })
            .await
            .unwrap();

        let _ = session_manager
            .checkout_turn(&ctx, AgentId::parse("codex").unwrap(), None, None)
            .await
            .unwrap();

        let dto = coordinator.status(Some(ctx), None).await.unwrap();

        match dto {
            StatusDto::Session(session) => {
                assert_eq!(
                    session.trace.unwrap().turn.unwrap(),
                    "/turns/turn-warm-latest"
                );
            }
            StatusDto::Task(_) => panic!("expected session status"),
        }
    }

    #[tokio::test]
    async fn cancel_task_flips_durable_when_working() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-cancel");
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();

        assert!(fixture.coordinator.cancel_task(id.clone()).await.unwrap());
        assert!(!fixture.coordinator.cancel_task(id.clone()).await.unwrap());
        let rec = fixture.task_store.get(&id).await.unwrap().unwrap();
        assert_eq!(rec.status, TaskRecordStatus::Canceled);
    }

    #[tokio::test]
    async fn shutdown_cancels_tokens_and_releases_sessions() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let id = task("task-shutdown");
        let token = CancellationToken::new();
        fixture
            .task_store
            .create(&working_record(id.clone()))
            .await
            .unwrap();
        fixture
            .coordinator
            .workflow_cancels
            .lock()
            .await
            .insert(id.clone(), token.clone());

        let c = ctx("ctx-shutdown");
        let turn = fixture
            .coordinator
            .session_manager
            .checkout_turn(&c, AgentId::parse("codex").unwrap(), None, None)
            .await
            .unwrap();
        fixture
            .coordinator
            .session_manager
            .finish_turn(&c, turn.generation, &turn.op)
            .await;
        assert!(fixture
            .coordinator
            .session_manager
            .status(&c)
            .await
            .is_some());

        fixture.coordinator.shutdown().await;

        assert!(token.is_cancelled());
        assert_eq!(
            fixture.task_store.get(&id).await.unwrap().unwrap().status,
            TaskRecordStatus::Canceled
        );
        assert!(fixture
            .coordinator
            .session_manager
            .status(&c)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn clear_rejects_when_a_run_is_active() {
        let fixture = coordinator_fixture(Arc::new(HashMap::new()));
        let c = ctx("ctx-clear");
        fixture
            .coordinator
            .workflow_runs
            .lock()
            .await
            .insert(c.clone(), CancellationToken::new());

        assert!(matches!(
            fixture.coordinator.clear(c, false).await,
            Err(BridgeError::HandleBusy)
        ));
    }
}
