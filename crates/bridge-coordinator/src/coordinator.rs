use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use a2a;
use bridge_core::domain::{InjectRequest, Part, PermitDecision};
use bridge_core::error::BridgeError;
#[cfg(test)]
use bridge_core::ids::OperationId;
use bridge_core::ids::{BatchId, ContextId, TaskId, WorkflowId};
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
    new_detached_task_id, resume_working_tasks, spawn_detached_workflow, DetachedDeps,
    TaskProgressHub,
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
                out.push(char::from(b));
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
        }
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

    /// FIX-3/PFIX-M: a warm single-turn against a context (minted if absent), collected to one TurnOutput.
    /// Context-less local dispatch is a follow-up; this method always uses the warm checkout path.
    pub async fn prompt(&self, p: OpParams) -> Result<TurnOutput, BridgeError> {
        let _deferred_cold_bindings = &self.bindings;
        let cwd = p.validate_cwd(self.allowed_cwd_root.as_ref())?;
        let agent = p
            .agent
            .clone()
            .unwrap_or_else(|| self.registry.default_id());
        let ctx = p.context.clone().unwrap_or_else(|| self.mint_context_id());
        let diagnostic = direct_diagnostic_observer();
        let turn = self
            .session_manager
            .checkout_turn_observed(
                &ctx,
                agent,
                Some(p.agent_override()),
                cwd,
                diagnostic.clone(),
            )
            .await?;
        self.collect_turn_observed(ctx, turn, p.input, diagnostic)
            .await
    }

    /// Continue an EXISTING warm context. Unlike `prompt`, this REUSES the context's stored fingerprint
    /// (agent/config/cwd) instead of re-deriving it from params: the `continue` surface advertises only
    /// `{input, context}`, so omitted agent/cwd/overrides must NOT be read as a config change (which
    /// `checkout_turn` rejects as `ConfigMismatch`). A context that was never minted → `SessionNotFound`.
    pub async fn continue_turn(&self, p: OpParams) -> Result<TurnOutput, BridgeError> {
        let ctx = p
            .context
            .clone()
            .ok_or(BridgeError::InvalidRequest { field: "context" })?;
        let diagnostic = direct_diagnostic_observer();
        let turn = self.session_manager.checkout_existing_turn(&ctx).await?;
        self.collect_turn_observed(ctx, turn, p.input, diagnostic)
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

    async fn collect_turn_observed(
        &self,
        ctx: ContextId,
        turn: crate::session_manager::WarmTurn,
        input: String,
        diagnostic: Arc<dyn DiagnosticObserver>,
    ) -> Result<TurnOutput, BridgeError> {
        let task = self.mint_prompt_task_id();
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
            maybe = events.next() => match maybe {
                    Some(ev) => ev,
                    None => break,
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
            return Err(e.clone());
        }
        cleanup_result?;
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

        Ok(TurnOutput {
            text: out_text,
            stop_reason,
            context: ctx,
        })
    }

    /// Submit a detached workflow run and return its durable task id.
    pub async fn run_workflow(&self, p: OpParams) -> Result<TaskId, BridgeError> {
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

        let task = new_detached_task_id();
        let now = self.clock.now_ms();
        let input = p.input;
        let workflow_spec_json = Some(crate::detached::encode_workflow_spec(&graph));
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
            workflow_spec_json,
            resume_attempts: 0,
            session_cwd: session_cwd.as_ref().map(|c| c.as_str().to_string()),
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        };
        self.task_store.create(&rec).await?;

        let hub = Arc::new(TaskProgressHub::new());
        self.progress_hubs
            .lock()
            .await
            .insert(task.clone(), hub.clone());
        let token = CancellationToken::new();
        self.workflow_cancels
            .lock()
            .await
            .insert(task.clone(), token.clone());
        drop(spawn_detached_workflow(
            &self.detached_deps(),
            task.clone(),
            input,
            graph,
            task.as_str().to_string(),
            token,
            HashMap::new(),
            WorkflowRunContext {
                session_cwd,
                task_id: Some(task.clone()),
                make_rich_sink: None,
                observer: self.observer.clone(),
                ..WorkflowRunContext::default()
            },
            hub,
        ));
        Ok(task)
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
    pub async fn resume(&self) {
        match self.batch_deps() {
            Some(bdeps) => crate::batch::resume_all(&bdeps, self.resume_attempt_cap).await,
            None => resume_working_tasks(&self.detached_deps(), self.resume_attempt_cap).await,
        }
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
        MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore, TurnLogFinalized,
        TurnLogFinished, TurnUsageFinalization,
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
        configured_turns: Arc<StdMutex<Vec<(SessionId, TurnMeta)>>>,
    }

    impl FakeBackend {
        fn new(prompt_gate: Option<Arc<Notify>>) -> Self {
            Self {
                prompt_gate,
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

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.entry.id.clone()]
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

    fn coordinator_fixture(workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>) -> Fixture {
        coordinator_fixture_with_backend(workflows, Arc::new(FakeBackend::new(None)))
    }

    fn coordinator_fixture_with_backend(
        workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>,
        backend: Arc<FakeBackend>,
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
            Arc::new(NoopObserver),
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

        let id = fixture
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

        fixture.coordinator.resume().await;

        let rec = fixture.task_store.get(&id).await.unwrap().unwrap();
        assert_eq!(
            rec.status,
            TaskRecordStatus::Interrupted,
            "coordinator.resume() must interrupt an unresumable working task"
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
