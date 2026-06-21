use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bridge_core::domain::Part;
use bridge_core::error::BridgeError;
use bridge_core::ids::{ContextId, OperationId, TaskId, WorkflowId};
use bridge_core::orch::{AgentSessionCaps, UsageSnapshot};
use bridge_core::ports::{AgentRegistry, PolicyEngine, SessionStore};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{TaskRecord, TaskRecordStatus, TaskStore};
use bridge_core::translator::{Event, EventKind, TaskOutcome, Translator};
use bridge_workflow::executor::{WorkflowExecutor, WorkflowRunContext};
use bridge_workflow::graph::WorkflowGraph;
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::clock::Clock;
use crate::detached::{
    new_detached_task_id, resume_working_tasks, spawn_detached_workflow, DetachedDeps,
    TaskProgressHub,
};
use crate::dispatch::TaskBinding;
use crate::params::OpParams;

static PROMPT_ID_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusDto {
    Session(SessionStatusDto),
    Task(TaskStatusDto),
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
}

#[derive(serde::Serialize)]
pub struct TaskStatusDto {
    pub id: TaskId,
    pub workflow: String,
    pub status: &'static str,
    pub result: Option<String>,
    pub error: Option<String>,
    pub updated_ms: i64,
}

pub struct TurnOutput {
    pub text: String,
    pub stop_reason: String,
    pub context: ContextId,
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
    clock: Arc<dyn Clock>,
    allowed_cwd_root: Option<SessionCwd>,
    resume_attempt_cap: u32,
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
            clock,
            allowed_cwd_root,
            resume_attempt_cap,
        }
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
        }
    }

    fn mint_context_id(&self) -> ContextId {
        let seq = PROMPT_ID_SEQ.fetch_add(1, Ordering::Relaxed);
        ContextId::parse(format!("ctx-{}-{seq}", self.clock.now_ms()))
            .expect("minted context id is non-empty")
    }

    fn mint_operation_id(&self) -> OperationId {
        let seq = PROMPT_ID_SEQ.fetch_add(1, Ordering::Relaxed);
        OperationId::parse(format!("op-{}-{seq}", self.clock.now_ms()))
            .expect("minted operation id is non-empty")
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
        let op = self.mint_operation_id();
        let turn = self
            .session_manager
            .checkout_turn(&ctx, agent, Some(p.agent_override()), cwd, op)
            .await?;
        self.collect_turn(ctx, turn, p.input).await
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
        let op = self.mint_operation_id();
        let turn = self
            .session_manager
            .checkout_existing_turn(&ctx, op)
            .await?;
        self.collect_turn(ctx, turn, p.input).await
    }

    /// Drive ONE warm turn to completion and collect it into a `TurnOutput`. Records usage as a side
    /// effect (excluded from output) and returns the handle to Idle on EVERY exit — synchronously on the
    /// normal/error path (so a sequential `continue` observes Idle deterministically), and via the drop
    /// guard if the turn future is cancelled mid-drain (the MCP loop is sequential and never drops
    /// mid-turn, but the Coordinator is a general service API — a cancelled caller must not strand the
    /// handle `Running`; this mirrors the A2A unary path's `WarmTurnGuard`).
    async fn collect_turn(
        &self,
        ctx: ContextId,
        turn: crate::session_manager::WarmTurn,
        input: String,
    ) -> Result<TurnOutput, BridgeError> {
        let mut finish_guard = TurnFinishGuard {
            sm: self.session_manager.clone(),
            ctx: ctx.clone(),
            generation: turn.generation,
            op: turn.op.clone(),
            armed: true,
        };

        let mut parts = Vec::new();
        if let Some(seed) = &turn.seed {
            parts.push(Part {
                text: format!("[Summary of earlier context in this session]\n{seed}"),
            });
        }
        parts.push(Part { text: input });

        let task = self.mint_prompt_task_id();
        let translator = Translator::new();
        let mut events = translator.run(
            turn.backend.as_ref(),
            self.session_store.as_ref(),
            self.policy.as_ref(),
            &task,
            &turn.session,
            parts,
        );
        let mut collected = Vec::new();
        while let Some(ev) = events.next().await {
            match &ev {
                Ok(e) if e.kind() == &EventKind::Usage => {
                    if let Some(snap) = e.usage_snapshot() {
                        self.session_manager
                            .record_usage(&ctx, turn.generation, &turn.op, snap.clone())
                            .await;
                    }
                    continue;
                }
                _ => collected.push(ev),
            }
        }
        drop(events);

        // Finish synchronously on the normal/error path, then disarm so the guard's drop is a no-op
        // (no double finish_turn). If the future was cancelled before reaching here, the still-armed
        // guard fires `finish_turn` on drop.
        self.session_manager
            .finish_turn(&ctx, turn.generation, &turn.op)
            .await;
        finish_guard.disarm();

        if let Some(Err(e)) = collected.iter().find(|r| r.is_err()) {
            return Err(e.clone());
        }
        let events: Vec<Event> = collected.into_iter().filter_map(Result::ok).collect();
        // The full reply is the coalesced Status chunks: the translator OVERWRITES `last_text` per
        // `Update::Text`, so the terminal Artifact event carries only the LAST delta — a truncation on
        // delta-streaming agents (codex-acp streams "OAK","LE","AF" → Artifact="AF"). Joining the Status
        // chunks reconstructs the complete text; fall back to the Artifact for a turn with no streamed
        // text (e.g. a stop_reason-only Done). This is the MCP surface only — the A2A unary path keeps
        // its own (artifact + status_chunks) wire shape unchanged.
        let status_text: String = events
            .iter()
            .filter(|e| e.kind() == &EventKind::Status)
            .map(|e| e.text())
            .collect();
        let out_text = if status_text.is_empty() {
            events
                .iter()
                .rev()
                .find(|e| e.kind() == &EventKind::Artifact)
                .map(|e| e.text().to_string())
                .unwrap_or_default()
        } else {
            status_text
        };
        let stop_reason = match events.iter().rev().find_map(|e| e.outcome()) {
            Some(TaskOutcome::Canceled) => "cancelled",
            Some(TaskOutcome::Failed) => "failed",
            Some(TaskOutcome::Completed) | None => "completed",
        }
        .to_string();

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
            input: input.clone(),
            workflow_spec_json,
            resume_attempts: 0,
            session_cwd: session_cwd.as_ref().map(|c| c.as_str().to_string()),
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
                make_rich_sink: None,
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
                Ok(StatusDto::Session(SessionStatusDto::from(&info)))
            }
            (None, Some(t)) => {
                let rec = self
                    .task_store
                    .get(&t)
                    .await?
                    .ok_or(BridgeError::TaskNotFound)?;
                Ok(StatusDto::Task(TaskStatusDto::from(&rec)))
            }
        }
    }

    /// Clear a warm context and its children, rejecting while a workflow run owns the context.
    pub async fn clear(
        &self,
        ctx: ContextId,
    ) -> Result<crate::session_manager::ResetOutcome, BridgeError> {
        let runs = self.workflow_runs.lock().await;
        if runs.contains_key(&ctx) {
            return Err(BridgeError::HandleBusy);
        }
        let result = self.session_manager.clear_with_children(&ctx, false).await;
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
        resume_working_tasks(&self.detached_deps(), self.resume_attempt_cap).await;
    }
}

/// Returns a warm handle to Idle (via `finish_turn`) if a turn future is dropped before it finishes
/// synchronously. `collect_turn` finishes the turn synchronously on the normal/error path and then
/// `disarm`s this guard, so on those paths the guard's `Drop` is a no-op; it only fires when the turn
/// future is cancelled mid-drain. Mirrors the A2A unary path's `WarmTurnGuard` (the spawn-in-Drop
/// pattern), kept local to the Coordinator because here it's disarmed after a synchronous finish.
struct TurnFinishGuard {
    sm: Arc<crate::session_manager::SessionManager>,
    ctx: ContextId,
    generation: bridge_core::ids::SessionGeneration,
    op: OperationId,
    armed: bool,
}

impl TurnFinishGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TurnFinishGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let op = self.op.clone();
        tokio::spawn(async move {
            sm.finish_turn(&ctx, generation, &op).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use crate::session_manager::SessionManager;
    use async_trait::async_trait;
    use bridge_core::domain::{
        AgentEntry, AgentKind, Effort, Part, PeerTaskId, PendingRequest, PermissionDecision,
        PermissionRequest, RegistrySnapshot, SessionContext,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, ContextId, NodeId, OperationId, SessionId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::{AgentBackend, BackendStream, Lease, Resolved, Update};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus};
    use bridge_workflow::graph::WorkflowNode;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::sync::Notify;

    struct NoopLease;
    impl Lease for NoopLease {}

    struct FakeBackend {
        prompt_gate: Option<Arc<Notify>>,
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

    /// Emits each string as a SEPARATE `Update::Text` delta (then Done) — models a streaming agent
    /// like codex-acp, where the translator's Artifact carries only the last delta.
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
            backend: Arc::new(FakeBackend { prompt_gate: None }),
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
            }],
        })
    }

    fn op(id: &str) -> OperationId {
        OperationId::parse(id).unwrap()
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
            input: "input".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        }
    }

    struct Fixture {
        coordinator: Coordinator,
        task_store: Arc<MemoryTaskStore>,
    }

    fn coordinator_fixture(workflows: Arc<HashMap<WorkflowId, Arc<WorkflowGraph>>>) -> Fixture {
        coordinator_fixture_with_backend(workflows, Arc::new(FakeBackend { prompt_gate: None }))
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
            input: "hello".into(),
            context: None,
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: Some("/tmp/repo".into()),
        }
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
            3,
        )
    }

    #[tokio::test]
    async fn prompt_warm_returns_text_and_context() {
        let backend = Arc::new(ScriptedBackend::with_usage(
            "backend text",
            UsageSnapshot {
                used: Some(7),
                size: Some(10),
                cost: None,
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
    async fn prompt_reconstructs_full_text_from_streamed_chunks() {
        // s8 T10 live-gate: a delta-streaming agent (Text "OAK","LE","AF") must yield the FULL reply,
        // NOT the last delta. The translator's terminal Artifact = last_text = "AF"; the full text lives
        // in the coalesced Status chunks, which `collect_turn` joins.
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
    async fn dropped_turn_returns_handle_to_idle() {
        // s8 T10 review MAJOR: a turn future dropped mid-drain must return the warm handle to Idle via
        // the drop guard — else the next turn on that context is permanently HandleBusy.
        let gate = Arc::new(Notify::new());
        let fixture = coordinator_fixture_with_backend(
            Arc::new(HashMap::new()),
            Arc::new(FakeBackend {
                prompt_gate: Some(gate.clone()),
            }),
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
            match coord
                .session_manager
                .checkout_existing_turn(&known, op("op-recheck"))
                .await
            {
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
            Arc::new(FakeBackend {
                prompt_gate: Some(gate),
            }),
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
        assert_eq!(rec.input, "hello");
        assert_eq!(rec.session_cwd.as_deref(), Some("/tmp/repo"));
        assert!(rec.workflow_spec_json.is_some());
        assert!(
            fixture.task_store.create(&rec).await.is_err(),
            "task creates must be non-clobbering"
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
            .checkout_turn(
                &c,
                AgentId::parse("codex").unwrap(),
                None,
                None,
                op("op-shutdown"),
            )
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
            fixture.coordinator.clear(c).await,
            Err(BridgeError::HandleBusy)
        ));
    }
}
