//! WorkflowExecutor — runs a validated DAG over the registry. Each node: configure_session
//! → prompt → concatenate Update::Text into the node output. Cancel via token.
use crate::fanout::{
    classify_offline_barrier_error_v1, FanOutControllerV1, PolicyActionV1,
    PolicyTriggerBarrierResultV1, ReadyNodeTerminalV1,
};
use crate::graph::{WorkflowGraph, WorkflowNode};
use crate::run_spec::WorkflowRunSpecV1;
use crate::template::render;
use bridge_core::attestation::{
    append_prompt_contract, prefix_attestation_request_for_capability, HarvestSanitizationMode,
    PrefixAttestationCapability, PrefixAttestationStatus,
};
use bridge_core::domain::{
    effective_config, AgentEntry, AgentOverride, EffectiveConfig, Part, SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    freeze_provider_attempt_v1, BoundProviderEffectV1, BoundSessionSpecV1,
    FrozenNodeExecutionIdentityV1, FrozenProviderLogicalSessionV1, LedgerAdmissionV1,
    NodeCleanupDispositionV1, NodeCleanupV1, NodePrimaryDispositionV1, NodeTerminalV1,
    PolicyNodeRefV1, ProviderEffectKeyV1, ProviderFreezeInputV1, Sha256HexV1,
    EXECUTION_POLICY_SCHEMA_V1,
};
use bridge_core::harvest::{
    commit_harvested_completion, CompletionBodyOrigin, HarvestAuditStore, NoopHarvestAuditStore,
};
use bridge_core::ids::{ContextId, NodeId, OperationId, SessionId};
use bridge_core::orch::UsageSnapshot;
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{
    classify_failure, AgentBackend, AgentRegistry, BackendObservers, BoundEntryUseV1,
    DiagnosticObserver, DiagnosticObserverFactory, FailureClass, ObsEvent, Observer, Resolved,
    RichEventSinkFactory, TurnContext, TurnOutcome, Update, UsageFinalization,
    STOP_REASON_CANCELLED,
};
use bridge_core::SessionCwd;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Per-request context forwarded opaquely through the executor to each node's
/// `configure_session` call. The scheduler/topo logic MUST NOT read this — it
/// is only consumed at the `SessionSpec` build site in `run_node`.
#[derive(Clone)]
pub struct WorkflowRunContext {
    pub session_cwd: Option<SessionCwd>,
    pub make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>,
    pub observer: Arc<dyn Observer>,
    pub parent_traceparent: Option<bridge_core::ports::TraceParent>,
    pub task_id: Option<bridge_core::ids::TaskId>,
    pub prompt_id: Option<String>,
    pub harvest_audit_store: Arc<dyn HarvestAuditStore>,
}

impl Default for WorkflowRunContext {
    fn default() -> Self {
        Self {
            session_cwd: None,
            make_rich_sink: None,
            observer: Arc::new(bridge_observ::NoopObserver),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        }
    }
}

/// An optional, fail-open telemetry barrier polled immediately before the
/// provider prompt future. Callers absorb persistence failures inside the
/// callback so workflow telemetry can never cancel the primary workload.
pub type PromptDispatchBarrier =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Exact canonical checkpoint payload selected by the fan-out controller.
/// The callback must resolve one closed barrier result before policy action.
#[derive(Clone, Debug, PartialEq)]
pub struct PolicyTriggerCheckpointV1 {
    pub node: NodeId,
    pub output: String,
    pub ok: bool,
    pub usage: Option<UsageSnapshot>,
    pub terminal_json: String,
    pub policy_trigger_json: String,
}

pub type PolicyTriggerBarrier = Arc<
    dyn Fn(
            PolicyTriggerCheckpointV1,
        ) -> Pin<Box<dyn Future<Output = PolicyTriggerBarrierResultV1> + Send>>
        + Send
        + Sync,
>;

async fn reach_policy_trigger_barrier_v1(
    admission: &LedgerAdmissionV1,
    barrier: Option<&PolicyTriggerBarrier>,
    checkpoint: PolicyTriggerCheckpointV1,
) -> PolicyTriggerBarrierResultV1 {
    match admission {
        LedgerAdmissionV1::HistoryLedgerUnavailable { reason } => {
            classify_offline_barrier_error_v1((*reason).into())
        }
        LedgerAdmissionV1::DurablePrimaryTaskStore => {
            let Some(barrier) = barrier else {
                return PolicyTriggerBarrierResultV1::PrimaryFailed;
            };
            match barrier(checkpoint).await {
                PolicyTriggerBarrierResultV1::ServedPrimaryCommitted => {
                    PolicyTriggerBarrierResultV1::ServedPrimaryCommitted
                }
                PolicyTriggerBarrierResultV1::PrimaryFailed
                | PolicyTriggerBarrierResultV1::OfflineHistoryCommitted
                | PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { .. } => {
                    PolicyTriggerBarrierResultV1::PrimaryFailed
                }
            }
        }
        LedgerAdmissionV1::HistoryLedgerAdmitted { .. } => {
            let Some(barrier) = barrier else {
                return PolicyTriggerBarrierResultV1::PrimaryFailed;
            };
            match barrier(checkpoint).await {
                PolicyTriggerBarrierResultV1::OfflineHistoryCommitted => {
                    PolicyTriggerBarrierResultV1::OfflineHistoryCommitted
                }
                PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { reason }
                    if reason
                        != bridge_core::workflow_history::LedgerUnavailableReason::Collision =>
                {
                    PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { reason }
                }
                PolicyTriggerBarrierResultV1::PrimaryFailed
                | PolicyTriggerBarrierResultV1::ServedPrimaryCommitted
                | PolicyTriggerBarrierResultV1::OfflineTelemetryUnavailable { .. } => {
                    PolicyTriggerBarrierResultV1::PrimaryFailed
                }
            }
        }
    }
}

fn canonical_terminal_json_v1(terminal: &NodeTerminalV1) -> Result<String, BridgeError> {
    String::from_utf8(
        terminal
            .encode_canonical()
            .map_err(|_| BridgeError::InvalidStateTransition)?,
    )
    .map_err(|_| BridgeError::InvalidStateTransition)
}

fn canonical_trigger_json_v1(
    trigger: &bridge_core::execution_policy::PolicyTriggerV1,
) -> Result<String, BridgeError> {
    String::from_utf8(
        trigger
            .encode_canonical()
            .map_err(|_| BridgeError::InvalidStateTransition)?,
    )
    .map_err(|_| BridgeError::InvalidStateTransition)
}

/// Additive diagnostic-authority wrapper for workflow execution. Keeping the
/// factory out of [`WorkflowRunContext`] preserves source compatibility for
/// downstream exhaustive struct literals while making durable authority an
/// explicit choice at the executor entrypoint.
#[derive(Clone)]
pub struct WorkflowDiagnosticContext {
    request: WorkflowRunContext,
    factory: Arc<dyn DiagnosticObserverFactory>,
    prompt_dispatch: Option<PromptDispatchBarrier>,
    policy_trigger: Option<PolicyTriggerBarrier>,
    frozen_authority: Option<FrozenWorkflowAuthority>,
}

#[derive(Clone)]
struct FrozenWorkflowAuthority {
    run_spec: Arc<WorkflowRunSpecV1>,
    provider_effect_key: Option<Arc<ProviderEffectKeyV1>>,
}

impl FrozenWorkflowAuthority {
    fn node_identity(
        &self,
        node: &WorkflowNode,
    ) -> Result<&FrozenNodeExecutionIdentityV1, BridgeError> {
        let node_digest = Sha256HexV1::digest(node.id.as_str().as_bytes());
        let identity = self
            .run_spec
            .node_execution_identities
            .iter()
            .find(|identity| identity.node.id_sha256 == node_digest)
            .ok_or(BridgeError::ConfigMismatch {
                field: "node_execution_identity",
            })?;
        if identity.selection.agent != node.agent {
            return Err(BridgeError::ConfigMismatch {
                field: "provider_selection",
            });
        }
        Ok(identity)
    }
}

/// True when at least one node explicitly renders the run-workflow input variable.
pub fn graph_consumes_input(graph: &WorkflowGraph) -> bool {
    graph
        .nodes
        .iter()
        .any(|node| node.prompt_template.contains("{{input}}"))
}

/// Error text for the local CLI guard that prevents a non-empty `--input` brief
/// from being silently ignored by a workflow whose prompts never reference it.
pub fn input_consumption_error(graph: &WorkflowGraph, input: &str) -> Option<String> {
    if input.is_empty() || graph_consumes_input(graph) {
        None
    } else {
        Some(format!(
            "workflow {:?} has no node prompt containing {{{{input}}}}, so the supplied --input would be ignored",
            graph.id.as_str()
        ))
    }
}

impl WorkflowDiagnosticContext {
    pub fn new(request: WorkflowRunContext, factory: Arc<dyn DiagnosticObserverFactory>) -> Self {
        Self {
            request,
            factory,
            prompt_dispatch: None,
            policy_trigger: None,
            frozen_authority: None,
        }
    }

    pub fn with_prompt_dispatch_barrier(mut self, barrier: PromptDispatchBarrier) -> Self {
        self.prompt_dispatch = Some(barrier);
        self
    }

    pub fn with_policy_trigger_barrier(mut self, barrier: PolicyTriggerBarrier) -> Self {
        self.policy_trigger = Some(barrier);
        self
    }

    /// Bind this invocation to one validated V2 run specification and its separately held
    /// provider-effect commitment key. The key is never persisted by the workflow layer.
    pub fn with_frozen_run_spec(
        mut self,
        run_spec: Arc<WorkflowRunSpecV1>,
        provider_effect_key: Option<Arc<ProviderEffectKeyV1>>,
    ) -> Result<Self, BridgeError> {
        run_spec
            .validate()
            .map_err(|error| BridgeError::ConfigInvalid {
                reason: format!("invalid frozen workflow run specification: {error}"),
            })?;
        if self.request.session_cwd != run_spec.requested_session_cwd {
            return Err(BridgeError::ConfigMismatch {
                field: "requested_session_cwd",
            });
        }
        for identity in &run_spec.node_execution_identities {
            for attempt in &identity.provider_attempts {
                if let Some(expected) = &attempt.effect.secret_commitment_key_id {
                    let Some(actual) = provider_effect_key.as_ref().map(|key| key.key_id()) else {
                        return Err(BridgeError::ConfigInvalid {
                            reason: "frozen provider effect requires its commitment key".into(),
                        });
                    };
                    if &actual != expected {
                        return Err(BridgeError::ConfigMismatch {
                            field: "provider_effect_key_id",
                        });
                    }
                }
            }
        }
        self.frozen_authority = Some(FrozenWorkflowAuthority {
            run_spec,
            provider_effect_key,
        });
        Ok(self)
    }

    pub fn in_memory(request: WorkflowRunContext) -> Self {
        Self::new(
            request,
            Arc::new(
                bridge_core::diagnostics::InMemoryDiagnosticObserverFactory::new(64)
                    .expect("workflow diagnostic capacity is nonzero"),
            ),
        )
    }

    fn into_parts(
        self,
    ) -> (
        WorkflowRunContext,
        Arc<dyn DiagnosticObserverFactory>,
        Option<PromptDispatchBarrier>,
        Option<PolicyTriggerBarrier>,
        Option<FrozenWorkflowAuthority>,
    ) {
        (
            self.request,
            self.factory,
            self.prompt_dispatch,
            self.policy_trigger,
            self.frozen_authority,
        )
    }
}

pub enum NodeTurnExit {
    Normal,
    Canceled,
    Error(BridgeError),
}

#[async_trait::async_trait]
pub trait NodeTurnCleanup: Send {
    /// Synchronously arm the terminal action before any later cancellation
    /// point (for example, flushing a rich-event sink). Compatibility
    /// implementations have no pre-settlement state and may ignore this.
    fn arm_exit(&mut self, _exit: &NodeTurnExit) {}

    /// Invoked once after prompt+drain on the node's exit branch. Each impl closes over what it owns
    /// (cold: backend+session for forget; warm: SessionManager+child+gen+op for finish/cancel/expire).
    async fn on_exit(self: Box<Self>, exit: NodeTurnExit);

    /// Result-bearing operation-owned cleanup. Compatibility implementations
    /// delegate to the legacy method; warm/observed owners override this so
    /// teardown settles before node-terminal observability.
    async fn on_exit_observed(
        self: Box<Self>,
        exit: NodeTurnExit,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.on_exit(exit).await;
        Ok(())
    }
}

pub struct NodeTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub seed: Option<String>, // warm-only; prepended to the node prompt parts (Slice-4 seed)
    pub cleanup: Box<dyn NodeTurnCleanup>,
}

#[async_trait::async_trait]
pub trait WorkflowNodeDispatcher: Send + Sync {
    async fn checkout(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError>;

    async fn checkout_observed(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<NodeTurn, BridgeError> {
        self.checkout(wf_id, node, run_id, ctx).await
    }

    /// Checkout a warm node turn with executor-selected config overrides.
    /// Dispatchers that configure model/effort/mode during checkout must forward
    /// these overrides; the default preserves legacy dispatchers by dropping them.
    async fn checkout_observed_with_overrides(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        _overrides: Option<AgentOverride>,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<NodeTurn, BridgeError> {
        self.checkout_observed(wf_id, node, run_id, ctx, observer)
            .await
    }
}

/// Uniform future type used in the per-run `FuturesUnordered` pool.
/// Each fan-out node is boxed to this type so `FuturesUnordered` can hold
/// futures of different async-block monomorphisations in one collection.
type NodeFut<'a> = std::pin::Pin<
    Box<dyn futures::Future<Output = (NodeId, Result<NodeRunOutput, BridgeError>)> + Send + 'a>,
>;

#[derive(Clone)]
struct NodeHarvestMeta {
    context: TurnContext,
    mode: HarvestSanitizationMode,
    capability: PrefixAttestationCapability,
    producer_id: String,
    status: PrefixAttestationStatus,
    origin: CompletionBodyOrigin,
}

struct NodeRunOutput {
    text: String,
    ok: bool,
    usage: Option<UsageSnapshot>,
    disposition: NodeDisposition,
    harvest: NodeHarvestMeta,
}

#[allow(clippy::too_many_arguments)]
fn node_harvest_meta_from_context(
    context: TurnContext,
    node: &WorkflowNode,
    capability: PrefixAttestationCapability,
    status: PrefixAttestationStatus,
    origin: CompletionBodyOrigin,
) -> NodeHarvestMeta {
    NodeHarvestMeta {
        context,
        mode: node.harvest_sanitization.unwrap_or_default(),
        capability,
        producer_id: node.agent.as_str().to_string(),
        status,
        origin,
    }
}

#[allow(clippy::too_many_arguments)]
fn node_harvest_meta(
    wf_id: &str,
    node: &WorkflowNode,
    run_id: &str,
    ctx: &WorkflowRunContext,
    attempt: u32,
    capability: PrefixAttestationCapability,
    status: PrefixAttestationStatus,
    origin: CompletionBodyOrigin,
) -> Result<NodeHarvestMeta, BridgeError> {
    Ok(node_harvest_meta_from_context(
        node_turn_context(wf_id, node, run_id, ctx, None, None, None, attempt)?,
        node,
        capability,
        status,
        origin,
    ))
}

#[allow(clippy::too_many_arguments)]
fn node_run_output(
    wf_id: &str,
    node: &WorkflowNode,
    run_id: &str,
    ctx: &WorkflowRunContext,
    text: String,
    ok: bool,
    usage: Option<UsageSnapshot>,
    disposition: NodeDisposition,
    origin: CompletionBodyOrigin,
) -> Result<NodeRunOutput, BridgeError> {
    Ok(NodeRunOutput {
        text,
        ok,
        usage,
        disposition,
        // attempt=0 denotes a pre-attempt synthetic exit, not a real
        // backend prompt attempt.
        harvest: node_harvest_meta(
            wf_id,
            node,
            run_id,
            ctx,
            0,
            PrefixAttestationCapability::default(),
            PrefixAttestationStatus::default(),
            origin,
        )?,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeDisposition {
    Completed,
    Failed,
    Canceled,
}

impl NodeDisposition {
    fn from_turn(outcome: &TurnOutcome) -> Self {
        match outcome {
            TurnOutcome::Success => Self::Completed,
            TurnOutcome::Failed(_) => Self::Failed,
            TurnOutcome::Canceled => Self::Canceled,
        }
    }

    fn workflow_outcome(self) -> WorkflowOutcome {
        match self {
            Self::Completed => WorkflowOutcome::Completed,
            Self::Failed => WorkflowOutcome::Failed,
            Self::Canceled => WorkflowOutcome::Canceled,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowCleanupDisposition {
    Complete,
    Failed,
    NotNeeded,
}

impl WorkflowCleanupDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::NotNeeded => "not_needed",
        }
    }
}

#[derive(Default)]
struct WorkflowCleanupTracker {
    state: std::sync::Mutex<WorkflowCleanupState>,
}

#[derive(Default)]
struct WorkflowCleanupState {
    observed: u32,
    failed: bool,
    intervals: Vec<(std::time::Instant, std::time::Instant)>,
}

impl WorkflowCleanupTracker {
    fn record(&self, started: std::time::Instant, finished: std::time::Instant, succeeded: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.observed = state.observed.saturating_add(1);
        state.failed |= !succeeded;
        state.intervals.push((started, finished));
    }

    fn observation(&self) -> (WorkflowCleanupDisposition, u64) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let disposition = if state.failed {
            WorkflowCleanupDisposition::Failed
        } else if state.observed == 0 {
            WorkflowCleanupDisposition::NotNeeded
        } else {
            WorkflowCleanupDisposition::Complete
        };
        let mut intervals = state.intervals.clone();
        intervals.sort_unstable_by_key(|(started, _)| *started);
        let mut total = std::time::Duration::ZERO;
        if let Some((mut current_start, mut current_end)) = intervals.first().copied() {
            for (started, finished) in intervals.into_iter().skip(1) {
                if started <= current_end {
                    current_end = current_end.max(finished);
                } else {
                    total = total.saturating_add(current_end.duration_since(current_start));
                    current_start = started;
                    current_end = finished;
                }
            }
            total = total.saturating_add(current_end.duration_since(current_start));
        }
        (
            disposition,
            u64::try_from(total.as_millis()).unwrap_or(u64::MAX),
        )
    }
}

async fn cleanup_warm_turn(
    cleanup: Box<dyn NodeTurnCleanup>,
    exit: NodeTurnExit,
    observer: Arc<dyn DiagnosticObserver>,
    tracker: &WorkflowCleanupTracker,
) -> Result<(), BridgeError> {
    let started = std::time::Instant::now();
    let result = cleanup.on_exit_observed(exit, observer).await;
    tracker.record(started, std::time::Instant::now(), result.is_ok());
    result
}

const WORKFLOW_PREFLIGHT_PROMPT: &str = "Reply with exactly PONG and nothing else.";
const PREFLIGHT_OBSERVER_ATTEMPT_BASE: u32 = 10_000;

fn is_exact_preflight_pong(text: &str) -> bool {
    text == "PONG"
}

type PreflightCacheEntry = Arc<tokio::sync::OnceCell<Result<PreflightDecision, PreflightFailure>>>;
type PreflightCache = Arc<tokio::sync::Mutex<HashMap<String, PreflightCacheEntry>>>;

#[derive(Clone)]
enum PreflightSource<'a> {
    Legacy(Arc<AgentEntry>),
    Bound {
        authority: &'a FrozenWorkflowAuthority,
        identity: &'a FrozenNodeExecutionIdentityV1,
    },
}

impl PreflightSource<'_> {
    fn agent(&self) -> &bridge_core::ids::AgentId {
        match self {
            Self::Legacy(entry) => &entry.id,
            Self::Bound { identity, .. } => &identity.selection.agent,
        }
    }

    fn preflight(&self) -> bool {
        match self {
            Self::Legacy(entry) => entry.preflight,
            Self::Bound { identity, .. } => identity.selection.preflight,
        }
    }

    fn primary_model(&self) -> Option<String> {
        match self {
            Self::Legacy(entry) => entry.model.clone(),
            Self::Bound { identity, .. } => identity.selection.effective_model.clone(),
        }
    }

    fn candidates(&self) -> Vec<Option<String>> {
        match self {
            Self::Legacy(entry) => WorkflowExecutor::preflight_candidates(entry),
            Self::Bound { identity, .. } => identity.selection.candidates(),
        }
    }

    fn cache_key(&self) -> String {
        match self {
            Self::Legacy(entry) => entry.id.as_str().to_string(),
            Self::Bound { identity, .. } => {
                let mut effect_bytes = Vec::new();
                for attempt in &identity.provider_attempts {
                    if matches!(
                        attempt.logical_session,
                        FrozenProviderLogicalSessionV1::Preflight { .. }
                    ) {
                        effect_bytes
                            .extend_from_slice(attempt.effect.effect_digest.as_str().as_bytes());
                    }
                }
                let effect_set = Sha256HexV1::digest(&effect_bytes);
                format!(
                    "v2:{}:{}:{}",
                    identity.selection.agent.as_str(),
                    identity.selection.selection_digest.as_str(),
                    effect_set.as_str()
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
struct PreflightDecision {
    selected_model: Option<String>,
    substituted_from: Option<String>,
    candidate_ordinal: u16,
}

#[derive(Clone, Debug)]
enum PreflightFailure {
    Canceled,
    Hard {
        message: String,
        failure_class: FailureClass,
        retain_in_run_cache: bool,
    },
}

impl PreflightFailure {
    fn retain_in_run_cache(&self) -> bool {
        matches!(
            self,
            Self::Hard {
                retain_in_run_cache: true,
                ..
            }
        )
    }
}

#[derive(Clone, Debug)]
struct AttemptSummary {
    attempt: u32,
    model: Option<String>,
    duration: std::time::Duration,
    usage: Option<UsageSnapshot>,
    reason: String,
}

fn model_label(model: Option<&str>) -> String {
    model.unwrap_or("<default>").to_string()
}

fn usage_label(usage: Option<&UsageSnapshot>) -> String {
    match usage {
        Some(usage) => format!(
            "used={:?}, size={:?}, total_tokens={:?}",
            usage.used,
            usage.size,
            usage
                .terminal
                .as_ref()
                .map(|terminal| terminal.total_tokens)
        ),
        None => "usage=n/a".to_string(),
    }
}

fn format_attempt_summaries(attempts: &[AttemptSummary]) -> String {
    attempts
        .iter()
        .map(|attempt| {
            format!(
                "attempt {} model={} duration_ms={} {} reason={}",
                attempt.attempt,
                model_label(attempt.model.as_deref()),
                attempt.duration.as_millis(),
                usage_label(attempt.usage.as_ref()),
                attempt.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn stream_dropped_error() -> BridgeError {
    BridgeError::agent_crashed("backend stream ended before terminal Done")
}

async fn record_synthetic_failure(
    observer: &Arc<dyn DiagnosticObserver>,
    phase: bridge_core::diagnostics::DiagnosticPhase,
    code: &'static str,
    class: bridge_core::diagnostics::DiagnosticFailureClass,
    summary: impl Into<String>,
    causes: Vec<String>,
) {
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition, PersistedPhaseTransition,
        PersistedPhaseTransitionInput, PhaseStatus,
    };

    let redactor = DiagnosticRedactor::default();
    let started = PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase,
            status: PhaseStatus::Started,
            at_ms: diagnostic_timestamp_ms(),
            operation: None,
            code: None,
            auth: None,
        },
        Some(code),
        &redactor,
    )
    .and_then(|transition| DiagnosticEvent::new(transition, None));
    if let Ok(event) = started {
        let _ = observer.record(event).await;
    }

    let failure = FailureDiagnostic::build_static_code(
        FailureDiagnosticInput {
            failed_phase: phase,
            last_completed_phase: match phase {
                bridge_core::diagnostics::DiagnosticPhase::PromptFinish => {
                    Some(bridge_core::diagnostics::DiagnosticPhase::PromptStream)
                }
                bridge_core::diagnostics::DiagnosticPhase::PromptStream => {
                    Some(bridge_core::diagnostics::DiagnosticPhase::PromptStart)
                }
                _ => None,
            },
            class,
            disposition: FailureDisposition::Fatal,
            code: code.to_string(),
            summary: summary.into(),
            causes,
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            prompt_may_have_been_accepted: matches!(
                phase,
                bridge_core::diagnostics::DiagnosticPhase::PromptStart
                    | bridge_core::diagnostics::DiagnosticPhase::PromptStream
                    | bridge_core::diagnostics::DiagnosticPhase::PromptFinish
                    | bridge_core::diagnostics::DiagnosticPhase::Teardown
            ),
        },
        code,
        &redactor,
    )
    .and_then(|failure| {
        PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status: PhaseStatus::Failed,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            Some(code),
            &redactor,
        )
        .and_then(|transition| DiagnosticEvent::new(transition, Some(failure)))
    });
    if let Ok(event) = failure {
        let _ = observer.record(event).await;
    }
}
#[derive(Clone, Copy)]
enum ColdCleanupAction {
    Forget,
    Release,
}

async fn cleanup_cold_session(
    backend: &Arc<dyn AgentBackend>,
    session: &SessionId,
    observer: &Arc<dyn DiagnosticObserver>,
    action: ColdCleanupAction,
    tracker: &WorkflowCleanupTracker,
) -> Result<(), BridgeError> {
    let started = std::time::Instant::now();
    let result = match action {
        ColdCleanupAction::Forget => {
            backend
                .forget_session_observed(session, observer.clone())
                .await
        }
        ColdCleanupAction::Release => {
            backend
                .release_session_observed(session, observer.clone())
                .await
        }
    };
    tracker.record(started, std::time::Instant::now(), result.is_ok());
    result
}

async fn cancel_and_forget_preflight_session(
    backend: &Arc<dyn AgentBackend>,
    session: &SessionId,
    observer: &Arc<dyn DiagnosticObserver>,
    tracker: &WorkflowCleanupTracker,
) {
    let _ = backend.cancel_observed(session, observer.clone()).await;
    let _ = cleanup_cold_session(
        backend,
        session,
        observer,
        ColdCleanupAction::Forget,
        tracker,
    )
    .await;
}

enum RenderInput {
    Freeform(String),
    Spec(bridge_core::task_spec::TaskSpec),
    Invalid(String),
}

fn parse_for_render(raw: &str) -> RenderInput {
    use bridge_core::task_spec::{parse, validate, TaskSpecError};

    match parse(raw) {
        Ok(spec) => match validate(&spec) {
            Ok(()) => RenderInput::Spec(spec),
            Err(e) => RenderInput::Invalid(e.to_string()),
        },
        Err(TaskSpecError::NoTaskType) => RenderInput::Freeform(raw.to_string()),
        Err(e) => RenderInput::Invalid(e.to_string()),
    }
}

fn render_vars_for_input(input: &str) -> Result<Vec<(String, String)>, String> {
    match parse_for_render(input) {
        RenderInput::Freeform(raw) => Ok(vec![("input".to_string(), raw)]),
        RenderInput::Spec(spec) => {
            let mut vars = vec![(
                "input".to_string(),
                bridge_core::task_spec::body(&spec).to_string(),
            )];
            let mut task_vars = Vec::new();

            if let Some(schema) = bridge_core::task_spec::schema(&spec.task_type) {
                for section in schema.sections {
                    task_vars.push((
                        format!(
                            "task.{}",
                            bridge_core::task_spec::normalize_field_name(section.name)
                        ),
                        String::new(),
                    ));
                }
            }

            for (name, value) in bridge_core::task_spec::fields(&spec) {
                task_vars.push((format!("task.{name}"), value));
            }

            vars.extend(task_vars);
            Ok(vars)
        }
        RenderInput::Invalid(msg) => Err(msg),
    }
}

/// Render the reserved `{{workflow.costs}}` synth var: a markdown table of each
/// input source's captured usage. Per-field `n/a` when absent.
/// `windowFraction = used/size` as a raw fraction.
pub(crate) fn render_costs_table(rows: &[(String, Option<UsageSnapshot>)]) -> String {
    let mut table = String::from(
        "| source | used | size | windowFraction | cost |\n| --- | --- | --- | --- | --- |\n",
    );
    for (source, usage) in rows {
        let (used, size, window_fraction, cost) = match usage {
            Some(usage) => {
                let used = usage
                    .used
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into());
                let size = usage
                    .size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into());
                let window_fraction = match (usage.used, usage.size) {
                    (Some(used), Some(size)) if size > 0 => {
                        format!("{:.4}", used as f64 / size as f64)
                    }
                    _ => "n/a".into(),
                };
                let cost = usage
                    .cost
                    .as_ref()
                    .map(|cost| format!("{} {}", cost.amount, cost.currency))
                    .unwrap_or_else(|| "n/a".into());
                (used, size, window_fraction, cost)
            }
            None => ("n/a".into(), "n/a".into(), "n/a".into(), "n/a".into()),
        };
        table.push_str(&format!(
            "| {source} | {used} | {size} | {window_fraction} | {cost} |\n"
        ));
    }
    table
}

pub(crate) fn render_weights(panel: &Option<crate::graph::PanelConfig>) -> String {
    match panel {
        Some(panel) if !panel.weights.is_empty() => {
            let mut rendered = String::new();
            for (key, value) in &panel.weights {
                rendered.push_str(&format!("- {key}: {value}\n"));
            }
            rendered
        }
        _ => "(no weights configured)".to_string(),
    }
}

pub struct WorkflowExecutor {
    registry: Arc<dyn AgentRegistry>,
}

struct FrozenBoundEntryUse {
    bound: BoundEntryUseV1,
    provider_effect: Arc<BoundProviderEffectV1>,
    config: EffectiveConfig,
}

struct FrozenAttemptUse {
    frozen: FrozenBoundEntryUse,
    backend: Arc<dyn AgentBackend>,
}

enum WorkflowAttemptUse {
    Legacy {
        resolved: Resolved,
        config: EffectiveConfig,
    },
    Bound(FrozenAttemptUse),
}

impl WorkflowAttemptUse {
    fn backend(&self) -> &Arc<dyn AgentBackend> {
        match self {
            Self::Legacy { resolved, .. } => &resolved.backend,
            Self::Bound(bound) => &bound.backend,
        }
    }

    fn config(&self) -> &EffectiveConfig {
        match self {
            Self::Legacy { config, .. } => config,
            Self::Bound(bound) => &bound.frozen.config,
        }
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        legacy_cwd: Option<SessionCwd>,
    ) -> Result<(), BridgeError> {
        match self {
            Self::Legacy { resolved, config } => {
                resolved
                    .backend
                    .configure_session(
                        session,
                        &SessionSpec {
                            config: config.clone(),
                            cwd: legacy_cwd,
                        },
                    )
                    .await
            }
            Self::Bound(bound) => {
                bound
                    .backend
                    .configure_bound_session(
                        session,
                        &BoundSessionSpecV1::new(
                            bound.frozen.config.clone(),
                            bound.frozen.provider_effect.clone(),
                        ),
                    )
                    .await
            }
        }
    }

    fn into_retry_invalidation(self, agent: &bridge_core::ids::AgentId) -> RetryInvalidation {
        match self {
            Self::Legacy { .. } => RetryInvalidation::Legacy(agent.clone()),
            Self::Bound(bound) => bound.frozen.into_retry_invalidation(),
        }
    }
}

enum RetryInvalidation {
    Legacy(bridge_core::ids::AgentId),
    Bound {
        bound: BoundEntryUseV1,
        effect_digest: Sha256HexV1,
    },
}

impl FrozenBoundEntryUse {
    fn into_retry_invalidation(self) -> RetryInvalidation {
        RetryInvalidation::Bound {
            effect_digest: self.provider_effect.frozen().effect.effect_digest.clone(),
            bound: self.bound,
        }
    }
}

impl RetryInvalidation {
    async fn apply(self, registry: &dyn AgentRegistry) {
        match self {
            Self::Legacy(agent) => registry.invalidate(&agent).await,
            Self::Bound {
                bound,
                effect_digest,
            } => registry.invalidate_bound(&bound, &effect_digest).await,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutcome {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    NodeStarted {
        node: NodeId,
    },
    NodeFinished {
        node: NodeId,
        ok: bool,
        output: String,
        usage: Option<bridge_core::orch::UsageSnapshot>,
        terminal_json: Option<String>,
        policy_trigger_json: Option<String>,
        policy_trigger_barrier_result: Option<PolicyTriggerBarrierResultV1>,
    },
    CleanupObserved {
        disposition: WorkflowCleanupDisposition,
        duration_ms: u64,
    },
    Terminal {
        outcome: WorkflowOutcome,
        output: String,
    },
}

pub type WorkflowStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<WorkflowEvent, BridgeError>> + Send>>;

#[allow(clippy::too_many_arguments)]
fn node_turn_context(
    wf_id: &str,
    node: &WorkflowNode,
    run_id: &str,
    ctx: &WorkflowRunContext,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    attempt: u32,
) -> Result<TurnContext, BridgeError> {
    Ok(TurnContext {
        turn_id: bridge_core::attestation::generate_turn_id()?,
        session_id: bridge_core::ids::ContextId::parse(run_id).unwrap_or_else(|_| {
            bridge_core::ids::ContextId::parse("workflow-fallback").expect("fallback is valid")
        }),
        task_id: ctx.task_id.clone(),
        workflow: Some(wf_id.to_string()),
        node: Some(node.id.as_str().to_string()),
        attempt,
        agent: node.agent.as_str().to_string(),
        model,
        effort,
        mode,
        prompt_id: ctx.prompt_id.clone(),
        traceparent: ctx.parent_traceparent.clone(),
    })
}

impl WorkflowExecutor {
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self {
        Self { registry }
    }

    fn bind_frozen_entry(
        &self,
        authority: &FrozenWorkflowAuthority,
        node: &WorkflowNode,
        logical_session: FrozenProviderLogicalSessionV1,
    ) -> Result<FrozenBoundEntryUse, BridgeError> {
        let identity = authority.node_identity(node)?;
        let persisted = identity
            .provider_attempts
            .iter()
            .find(|attempt| attempt.logical_session == logical_session)
            .ok_or_else(|| BridgeError::ConfigInvalid {
                reason: "frozen provider logical session is outside the admitted matrix".into(),
            })?;
        let bound = self
            .registry
            .bind_entry_use(&node.agent)
            .ok_or(BridgeError::BindUnsupported)?;
        let reconstructed = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
            entry: &bound.entry,
            overrides: None,
            node: identity.node.clone(),
            logical_session,
            checkout: persisted.checkout.clone(),
            provider_effect_key: authority.provider_effect_key.as_deref(),
        })
        .map_err(|error| BridgeError::ConfigInvalid {
            reason: format!("frozen provider attempt cannot be reconstructed: {error}"),
        })?;
        if reconstructed.selection != identity.selection {
            return Err(BridgeError::ConfigMismatch {
                field: "provider_selection",
            });
        }
        if reconstructed.frozen != *persisted {
            return Err(BridgeError::ConfigMismatch {
                field: "provider_effect",
            });
        }
        let candidate_ordinal = match logical_session {
            FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal }
            | FrozenProviderLogicalSessionV1::Execute { candidate_ordinal } => candidate_ordinal,
        };
        let selected_model = identity
            .selection
            .candidates()
            .get(usize::from(candidate_ordinal))
            .cloned()
            .ok_or_else(|| BridgeError::ConfigInvalid {
                reason: "frozen provider candidate is outside the admitted selection".into(),
            })?;
        Ok(FrozenBoundEntryUse {
            bound,
            provider_effect: Arc::new(reconstructed.bound),
            config: EffectiveConfig {
                model: selected_model,
                effort: identity.selection.effective_effort,
                mode: identity.selection.effective_mode.clone(),
            },
        })
    }

    async fn resolve_frozen_entry(
        &self,
        frozen: FrozenBoundEntryUse,
        diagnostic: Arc<dyn DiagnosticObserver>,
    ) -> Result<FrozenAttemptUse, (BridgeError, FrozenBoundEntryUse)> {
        match self
            .registry
            .resolve_bound(&frozen.bound, &frozen.provider_effect, diagnostic)
            .await
        {
            Ok(backend) => Ok(FrozenAttemptUse { frozen, backend }),
            Err(error) => Err((error, frozen)),
        }
    }

    /// Compute the configured workload partition without resolving or spawning
    /// any agent backend.
    pub fn workload_fingerprint(&self, graph: &WorkflowGraph) -> (String, bool) {
        crate::graph::workload_fingerprint(graph, self.registry.as_ref())
    }

    fn preflight_candidates(entry: &AgentEntry) -> Vec<Option<String>> {
        std::iter::once(entry.model.clone())
            .chain(entry.fallback_models.iter().cloned().map(Some))
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_agent_preflight(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: &Option<PromptDispatchBarrier>,
        cancel: &CancellationToken,
        entry: Arc<AgentEntry>,
        cache: &PreflightCache,
        cleanup_tracker: &WorkflowCleanupTracker,
    ) -> Result<PreflightDecision, PreflightFailure> {
        self.ensure_preflight(
            wf_id,
            node,
            run_id,
            ctx,
            diagnostic_factory,
            prompt_dispatch,
            cancel,
            PreflightSource::Legacy(entry),
            cache,
            cleanup_tracker,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_frozen_preflight(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: &Option<PromptDispatchBarrier>,
        cancel: &CancellationToken,
        authority: &FrozenWorkflowAuthority,
        cache: &PreflightCache,
        cleanup_tracker: &WorkflowCleanupTracker,
    ) -> Result<PreflightDecision, PreflightFailure> {
        let identity = authority
            .node_identity(node)
            .map_err(|error| PreflightFailure::Hard {
                message: format!("frozen preflight identity mismatch: {error:?}"),
                failure_class: classify_failure(&error),
                retain_in_run_cache: false,
            })?;
        self.ensure_preflight(
            wf_id,
            node,
            run_id,
            ctx,
            diagnostic_factory,
            prompt_dispatch,
            cancel,
            PreflightSource::Bound {
                authority,
                identity,
            },
            cache,
            cleanup_tracker,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn ensure_preflight(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: &Option<PromptDispatchBarrier>,
        cancel: &CancellationToken,
        source: PreflightSource<'_>,
        cache: &PreflightCache,
        cleanup_tracker: &WorkflowCleanupTracker,
    ) -> Result<PreflightDecision, PreflightFailure> {
        if !source.preflight() {
            return Ok(PreflightDecision {
                selected_model: source.primary_model(),
                substituted_from: None,
                candidate_ordinal: 0,
            });
        }

        let cache_key = source.cache_key();
        let cell = {
            let mut cache = cache.lock().await;
            cache
                .entry(cache_key.clone())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        // The cell single-flights concurrent first misses. Only successful
        // decisions remain in the run cache. Cancellation and failures proved
        // pre-acceptance are evicted below so they cannot poison later nodes;
        // accepted or indeterminate prompt failures remain cached so another
        // node in this run cannot replay possibly accepted work.
        let result = cell
            .get_or_init(|| async {
                self.run_agent_preflight_uncached(
                    wf_id,
                    node,
                    run_id,
                    ctx,
                    diagnostic_factory,
                    prompt_dispatch,
                    cancel,
                    source.clone(),
                    cleanup_tracker,
                )
                .await
            })
            .await
            .clone();
        if result.is_err()
            && !result
                .as_ref()
                .is_err_and(PreflightFailure::retain_in_run_cache)
        {
            let mut cache = cache.lock().await;
            if cache
                .get(&cache_key)
                .is_some_and(|cached| Arc::ptr_eq(cached, &cell))
            {
                cache.remove(&cache_key);
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_preflight_uncached(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: &Option<PromptDispatchBarrier>,
        cancel: &CancellationToken,
        source: PreflightSource<'_>,
        cleanup_tracker: &WorkflowCleanupTracker,
    ) -> Result<PreflightDecision, PreflightFailure> {
        let candidates = source.candidates();
        let agent = source.agent().clone();
        let primary_model = source.primary_model();
        let mut attempts = Vec::new();
        for (idx, model) in candidates.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(PreflightFailure::Canceled);
            }
            let attempt_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
            let diagnostic = diagnostic_factory.make(
                &node.id,
                PREFLIGHT_OBSERVER_ATTEMPT_BASE.saturating_add(attempt_no),
            );
            let started = std::time::Instant::now();
            let mut usage: Option<UsageSnapshot> = None;
            let mut reason: Option<String> = None;

            let acquired: Result<WorkflowAttemptUse, (BridgeError, RetryInvalidation)> =
                match &source {
                    PreflightSource::Legacy(_) => {
                        let resolved_result = tokio::select! {
                            biased;
                            _ = cancel.cancelled() => return Err(PreflightFailure::Canceled),
                            result = self.registry.resolve_observed(&agent, diagnostic.clone()) => result,
                        };
                        resolved_result
                            .map(|resolved| {
                                let mut config = effective_config(&resolved.entry, None);
                                config.model = model.clone();
                                WorkflowAttemptUse::Legacy { resolved, config }
                            })
                            .map_err(|error| (error, RetryInvalidation::Legacy(agent.clone())))
                    }
                    PreflightSource::Bound { authority, .. } => {
                        let candidate_ordinal =
                            u16::try_from(idx).map_err(|_| PreflightFailure::Hard {
                                message: "frozen preflight candidate ordinal overflow".into(),
                                failure_class: FailureClass::Other,
                                retain_in_run_cache: false,
                            })?;
                        let frozen = self
                            .bind_frozen_entry(
                                authority,
                                node,
                                FrozenProviderLogicalSessionV1::Preflight { candidate_ordinal },
                            )
                            .map_err(|error| PreflightFailure::Hard {
                                message: format!(
                                    "frozen preflight binding refused before effects: {error:?}"
                                ),
                                failure_class: classify_failure(&error),
                                retain_in_run_cache: false,
                            })?;
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => return Err(PreflightFailure::Canceled),
                            result = self.resolve_frozen_entry(frozen, diagnostic.clone()) => {
                                result
                                    .map(WorkflowAttemptUse::Bound)
                                    .map_err(|(error, frozen)| {
                                        (error, frozen.into_retry_invalidation())
                                    })
                            }
                        }
                    }
                };
            let attempt_use = match acquired {
                Ok(attempt_use) => attempt_use,
                Err((error, invalidation)) => {
                    attempts.push(AttemptSummary {
                        attempt: attempt_no,
                        model: model.clone(),
                        duration: started.elapsed(),
                        usage: None,
                        reason: format!("resolve error: {error:?}"),
                    });
                    if idx + 1 < candidates.len() {
                        invalidation.apply(self.registry.as_ref()).await;
                        tracing::warn!(
                            agent = agent.as_str(),
                            model = %model_label(model.as_deref()),
                            error = ?error,
                            "workflow preflight fallback after resolve error"
                        );
                        continue;
                    }
                    break;
                }
            };
            if cancel.is_cancelled() {
                return Err(PreflightFailure::Canceled);
            }

            let session = match SessionId::parse(format!(
                "workflow-preflight-{wf_id}-{}-{run_id}-{attempt_no}",
                agent.as_str()
            )) {
                Ok(session) => session,
                Err(error) => {
                    return Err(PreflightFailure::Hard {
                        message: format!("preflight failed to build session id: {error:?}"),
                        failure_class: classify_failure(&error),
                        retain_in_run_cache: false,
                    });
                }
            };

            let configure_result = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = cleanup_cold_session(
                        attempt_use.backend(),
                        &session,
                        &diagnostic,
                        ColdCleanupAction::Forget,
                        cleanup_tracker,
                    ).await;
                    return Err(PreflightFailure::Canceled);
                }
                result = attempt_use.configure_session(&session, ctx.session_cwd.clone()) => result,
            };
            if let Err(error) = configure_result {
                let _ = cleanup_cold_session(
                    attempt_use.backend(),
                    &session,
                    &diagnostic,
                    ColdCleanupAction::Forget,
                    cleanup_tracker,
                )
                .await;
                attempts.push(AttemptSummary {
                    attempt: attempt_no,
                    model: model.clone(),
                    duration: started.elapsed(),
                    usage: None,
                    reason: format!("configure error: {error:?}"),
                });
                if idx + 1 < candidates.len() {
                    attempt_use
                        .into_retry_invalidation(&agent)
                        .apply(self.registry.as_ref())
                        .await;
                    tracing::warn!(
                        agent = agent.as_str(),
                        model = %model_label(model.as_deref()),
                        error = ?error,
                        "workflow preflight fallback after configure error"
                    );
                    continue;
                }
                break;
            }
            if cancel.is_cancelled() {
                let _ = cleanup_cold_session(
                    attempt_use.backend(),
                    &session,
                    &diagnostic,
                    ColdCleanupAction::Forget,
                    cleanup_tracker,
                )
                .await;
                return Err(PreflightFailure::Canceled);
            }

            let preflight_turn_id =
                bridge_core::attestation::generate_turn_id().map_err(|error| {
                    PreflightFailure::Hard {
                        message: format!(
                            "preflight failed to mint turn evidence correlation: {error:?}"
                        ),
                        failure_class: classify_failure(&error),
                        retain_in_run_cache: false,
                    }
                })?;
            let preflight_context =
                ContextId::parse(session.as_str().to_string()).map_err(|error| {
                    PreflightFailure::Hard {
                        message: format!("preflight failed to bind context: {error:?}"),
                        failure_class: classify_failure(&error),
                        retain_in_run_cache: false,
                    }
                })?;
            let preflight_op = OperationId::parse(format!(
                "workflow-preflight-{}-{attempt_no}",
                node.id.as_str()
            ))
            .map_err(|error| PreflightFailure::Hard {
                message: format!("preflight failed to bind operation: {error:?}"),
                failure_class: classify_failure(&error),
                retain_in_run_cache: false,
            })?;
            attempt_use
                .backend()
                .configure_turn(
                    &session,
                    TurnMeta {
                        context_id: preflight_context,
                        generation: u64::from(attempt_no),
                        op: preflight_op,
                        turn_id: preflight_turn_id.clone(),
                        requested_mode: HarvestSanitizationMode::Off,
                        prefix_attestation_request:
                            bridge_core::attestation::PrefixAttestationRequest::Disabled,
                    },
                )
                .await;
            let rich_sink = ctx
                .make_rich_sink
                .as_ref()
                .map(|factory| factory.make(&node.id));
            let activity = rich_sink
                .as_ref()
                .and_then(|sink| sink.attempt_recorder())
                .unwrap_or_else(|| Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder));
            let stream = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    cancel_and_forget_preflight_session(
                        attempt_use.backend(),
                        &session,
                        &diagnostic,
                        cleanup_tracker,
                    ).await;
                    return Err(PreflightFailure::Canceled);
                }
                result = async {
                    if let Some(barrier) = prompt_dispatch {
                        barrier().await;
                    }
                    // Registration happens only after the durable prompt
                    // barrier is polled. Preflight is a genuine provider turn,
                    // so accepted or indeterminate work must participate in
                    // the same bounded attempt evidence population as nodes.
                    let terminal_evidence = match rich_sink.as_ref() {
                        Some(sink) => sink
                            .terminal_evidence_for_turn(
                                attempt_use.backend().terminal_evidence_capability(),
                                u64::from(attempt_no),
                                session.as_str(),
                                preflight_turn_id.as_str(),
                            )?
                            .unwrap_or_else(|| {
                                Arc::new(
                                    bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                                )
                            }),
                        None => Arc::new(
                            bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                        ),
                    };
                    attempt_use.backend().prompt_with_observers(
                        &session,
                        vec![Part {
                            text: WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                        }],
                        BackendObservers::new(diagnostic.clone(), rich_sink.clone())
                            .with_attempt_telemetry(activity.clone(), terminal_evidence),
                    ).await
                } => result,
            };
            if cancel.is_cancelled() {
                cancel_and_forget_preflight_session(
                    attempt_use.backend(),
                    &session,
                    &diagnostic,
                    cleanup_tracker,
                )
                .await;
                return Err(PreflightFailure::Canceled);
            }
            let mut stream = match stream {
                Ok(stream) => stream,
                Err(error) => {
                    let may_have_been_accepted = match &error {
                        BridgeError::AgentFailure { diagnostic } => {
                            diagnostic.prompt_may_have_been_accepted()
                        }
                        _ => true,
                    };
                    if may_have_been_accepted {
                        cancel_and_forget_preflight_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            cleanup_tracker,
                        )
                        .await;
                    } else {
                        let _ = cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await;
                    }
                    attempts.push(AttemptSummary {
                        attempt: attempt_no,
                        model: model.clone(),
                        duration: started.elapsed(),
                        usage: None,
                        reason: format!("prompt error: {error:?}"),
                    });
                    if may_have_been_accepted {
                        return Err(PreflightFailure::Hard {
                            message: format!(
                                "preflight stopped for agent {} because prompt acceptance could not be excluded: {}",
                                agent.as_str(),
                                format_attempt_summaries(&attempts)
                            ),
                            failure_class: classify_failure(&error),
                            retain_in_run_cache: true,
                        });
                    }
                    if idx + 1 < candidates.len() {
                        attempt_use
                            .into_retry_invalidation(&agent)
                            .apply(self.registry.as_ref())
                            .await;
                        tracing::warn!(
                            agent = agent.as_str(),
                            model = %model_label(model.as_deref()),
                            error = ?error,
                            "workflow preflight fallback after prompt error"
                        );
                        continue;
                    }
                    break;
                }
            };

            let mut text = String::new();
            let mut saw_done = false;
            let mut replay_barrier = false;
            loop {
                tokio::select! {
                    biased;
                    item = stream.next() => match item {
                        Some(Ok(Update::Text(chunk) | Update::FinalAnswer(chunk))) =>
                            text.push_str(&chunk),
                        Some(Ok(Update::Usage(mut next_usage))) => {
                            if let Some(previous) = &usage {
                                next_usage.merge_missing_from(previous);
                            }
                            usage = Some(next_usage);
                        }
                        Some(Ok(Update::Permission(_))) => {}
                        Some(Ok(Update::Done { stop_reason, .. })) => {
                            saw_done = true;
                            if stop_reason != STOP_REASON_CANCELLED {
                                break;
                            }
                            reason = Some("preflight canceled by agent".to_string());
                            break;
                        }
                        Some(Err(error)) => {
                            reason = Some(format!("stream error: {error:?}"));
                            replay_barrier = true;
                            break;
                        }
                        None => {
                            reason = Some("stream ended before terminal Done".to_string());
                            replay_barrier = true;
                            break;
                        }
                    },
                    _ = cancel.cancelled() => {
                        cancel_and_forget_preflight_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            cleanup_tracker,
                        ).await;
                        return Err(PreflightFailure::Canceled);
                    }
                }
            }

            let cleanup_error = if replay_barrier {
                cancel_and_forget_preflight_session(
                    attempt_use.backend(),
                    &session,
                    &diagnostic,
                    cleanup_tracker,
                )
                .await;
                None
            } else {
                cleanup_cold_session(
                    attempt_use.backend(),
                    &session,
                    &diagnostic,
                    ColdCleanupAction::Forget,
                    cleanup_tracker,
                )
                .await
                .err()
            };

            if saw_done
                && reason.is_none()
                && is_exact_preflight_pong(&text)
                && cleanup_error.is_none()
            {
                let substituted_from = if model != &primary_model {
                    Some(model_label(primary_model.as_deref()))
                } else {
                    None
                };
                if let Some(from) = &substituted_from {
                    let to = model_label(model.as_deref());
                    tracing::warn!(
                        agent = agent.as_str(),
                        from = %from,
                        to = %to,
                        "workflow preflight selected fallback model"
                    );
                    if let Some(factory) = &ctx.make_rich_sink {
                        let sink = factory.make(&node.id);
                        sink.record(bridge_core::orch::OrchEventKind::Progress {
                            progress: bridge_core::orch::ProgressPayload::legacy(format!(
                                "workflow preflight selected fallback model for agent {}: {from} -> {to}",
                                agent.as_str()
                            )),
                        });
                        let _ = sink.flush().await;
                    }
                }
                let decision = PreflightDecision {
                    selected_model: model.clone(),
                    substituted_from,
                    candidate_ordinal: u16::try_from(idx).map_err(|_| PreflightFailure::Hard {
                        message: "preflight candidate ordinal overflow".into(),
                        failure_class: FailureClass::Other,
                        retain_in_run_cache: false,
                    })?,
                };
                return Ok(decision);
            }

            let reason = match cleanup_error {
                Some(error) => format!("cleanup error: {error:?}"),
                None => reason.unwrap_or_else(|| {
                    if !saw_done {
                        "stream ended before terminal Done".to_string()
                    } else if text.trim().is_empty() {
                        "empty final".to_string()
                    } else {
                        format!("unexpected smoke response: {text:?}")
                    }
                }),
            };
            attempts.push(AttemptSummary {
                attempt: attempt_no,
                model: model.clone(),
                duration: started.elapsed(),
                usage: usage.clone(),
                reason: reason.clone(),
            });
            return Err(PreflightFailure::Hard {
                message: format!(
                    "preflight stopped for agent {} because the prompt was accepted and must not be replayed: {}",
                    agent.as_str(),
                    format_attempt_summaries(&attempts)
                ),
                failure_class: FailureClass::Other,
                retain_in_run_cache: true,
            });
        }

        Err(PreflightFailure::Hard {
            message: format!(
                "preflight exhausted for agent {} after {} model(s): {}",
                agent.as_str(),
                attempts.len(),
                format_attempt_summaries(&attempts)
            ),
            failure_class: FailureClass::Other,
            retain_in_run_cache: false,
        })
    }

    /// Run one node: render its prompt from `vars`, resolve+configure+prompt+drain, forget.
    /// Returns (text, ok, usage, disposition). On any failure returns the error marker + ok=false.
    #[allow(clippy::too_many_arguments)]
    async fn run_node(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        vars: &HashMap<&str, &str>,
        run_id: &str,
        cancel: &CancellationToken,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: &Option<PromptDispatchBarrier>,
        cleanup_tracker: &WorkflowCleanupTracker,
        dispatcher: Option<&Arc<dyn WorkflowNodeDispatcher>>,
        preflight_cache: &PreflightCache,
        frozen_authority: Option<&FrozenWorkflowAuthority>,
    ) -> Result<NodeRunOutput, BridgeError> {
        if cancel.is_cancelled() {
            return node_run_output(
                wf_id,
                node,
                run_id,
                ctx,
                format!("[node {} canceled]", node.id.as_str()),
                false,
                None,
                NodeDisposition::Canceled,
                CompletionBodyOrigin::BridgeSyntheticCancellation,
            );
        }
        if let Some(d) = dispatcher {
            if frozen_authority.is_some() {
                return Err(BridgeError::BindUnsupported);
            }
            let rendered = render(&node.prompt_template, vars);
            let node_obs_ctx = node_turn_context(wf_id, node, run_id, ctx, None, None, None, 0)?;
            ctx.observer
                .record(&ObsEvent::NodeStarted { ctx: &node_obs_ctx });
            let entry_snapshot = self.registry.entry_snapshot(&node.agent);
            let (checkout_overrides, obs_model, obs_effort, obs_mode) =
                if let Some(entry) = entry_snapshot.as_ref().filter(|entry| entry.preflight) {
                    let preflight_decision = match self
                        .ensure_agent_preflight(
                            wf_id,
                            node,
                            run_id,
                            ctx,
                            diagnostic_factory,
                            prompt_dispatch,
                            cancel,
                            entry.clone(),
                            preflight_cache,
                            cleanup_tracker,
                        )
                        .await
                    {
                        Ok(decision) => decision,
                        Err(PreflightFailure::Canceled) => {
                            let outcome = TurnOutcome::Canceled;
                            ctx.observer.record(&ObsEvent::NodeFinished {
                                ctx: &node_obs_ctx,
                                outcome: &outcome,
                            });
                            return node_run_output(
                                wf_id,
                                node,
                                run_id,
                                ctx,
                                format!("[node {} canceled]", node.id.as_str()),
                                false,
                                None,
                                NodeDisposition::Canceled,
                                CompletionBodyOrigin::BridgeSyntheticCancellation,
                            );
                        }
                        Err(PreflightFailure::Hard {
                            message,
                            failure_class,
                            ..
                        }) => {
                            let outcome = TurnOutcome::Failed(failure_class);
                            ctx.observer.record(&ObsEvent::NodeFinished {
                                ctx: &node_obs_ctx,
                                outcome: &outcome,
                            });
                            return node_run_output(
                                wf_id,
                                node,
                                run_id,
                                ctx,
                                format!("[node {} failed: {message}]", node.id.as_str()),
                                false,
                                None,
                                NodeDisposition::Failed,
                                CompletionBodyOrigin::BridgeSyntheticStreamError,
                            );
                        }
                    };
                    let mut eff = effective_config(entry, None);
                    eff.model = preflight_decision.selected_model.clone();
                    let checkout_overrides =
                        preflight_decision
                            .substituted_from
                            .as_ref()
                            .map(|_| AgentOverride {
                                model: preflight_decision.selected_model.clone(),
                                effort: None,
                                mode: None,
                            });
                    (
                        checkout_overrides,
                        eff.model.clone(),
                        eff.effort
                            .as_ref()
                            .map(|e| format!("{e:?}").to_ascii_lowercase()),
                        eff.mode.clone(),
                    )
                } else {
                    (None, None, None, None)
                };
            let attempt = 1_u32;
            // A successful prompt-open makes this path deliberately single-attempt:
            // terminal failures must leave the accepted prompt unreplayed.
            {
                let diagnostic = diagnostic_factory.make(&node.id, attempt);
                let obs_ctx = node_turn_context(
                    wf_id,
                    node,
                    run_id,
                    ctx,
                    obs_model.clone(),
                    obs_effort.clone(),
                    obs_mode.clone(),
                    attempt,
                )?;
                let mut turn = match d
                    .checkout_observed_with_overrides(
                        wf_id,
                        node,
                        run_id,
                        ctx,
                        checkout_overrides.clone(),
                        diagnostic.clone(),
                    )
                    .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        let fail_out = TurnOutcome::Failed(classify_failure(&e));
                        ctx.observer.record(&ObsEvent::NodeFinished {
                            ctx: &node_obs_ctx,
                            outcome: &fail_out,
                        });
                        return node_run_output(
                            wf_id,
                            node,
                            run_id,
                            ctx,
                            format!("[node {} failed: {:?}]", node.id.as_str(), e),
                            false,
                            None,
                            NodeDisposition::Failed,
                            CompletionBodyOrigin::BridgeSyntheticStreamError,
                        );
                    }
                };
                if cancel.is_cancelled() {
                    let cleanup = cleanup_warm_turn(
                        turn.cleanup,
                        NodeTurnExit::Normal,
                        diagnostic.clone(),
                        cleanup_tracker,
                    )
                    .await;
                    let (text, outcome) = match cleanup {
                        Ok(()) => (
                            format!("[node {} canceled]", node.id.as_str()),
                            TurnOutcome::Canceled,
                        ),
                        Err(error) => (
                            format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error),
                            TurnOutcome::Failed(classify_failure(&error)),
                        ),
                    };
                    ctx.observer.record(&ObsEvent::NodeFinished {
                        ctx: &node_obs_ctx,
                        outcome: &outcome,
                    });
                    let disposition = NodeDisposition::from_turn(&outcome);
                    let origin = if matches!(disposition, NodeDisposition::Canceled) {
                        CompletionBodyOrigin::BridgeSyntheticCancellation
                    } else {
                        CompletionBodyOrigin::BridgeSyntheticStreamError
                    };
                    return node_run_output(
                        wf_id,
                        node,
                        run_id,
                        ctx,
                        text,
                        false,
                        None,
                        disposition,
                        origin,
                    );
                }

                let prefix_capability = turn.backend.prefix_attestation_capability();
                let node_harvest_mode = node.harvest_sanitization.unwrap_or_default();
                let prefix_attestation_request = match prefix_attestation_request_for_capability(
                    node_harvest_mode,
                    &prefix_capability,
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        let cleanup = cleanup_warm_turn(
                            turn.cleanup,
                            NodeTurnExit::Normal,
                            diagnostic.clone(),
                            cleanup_tracker,
                        )
                        .await;
                        let text = match cleanup {
                            Ok(()) => {
                                format!("[node {} failed: {:?}]", node.id.as_str(), error)
                            }
                            Err(cleanup_error) => format!(
                                "[node {} cleanup failed after prefix setup error: {:?}]",
                                node.id.as_str(),
                                cleanup_error
                            ),
                        };
                        let outcome = TurnOutcome::Failed(classify_failure(&error));
                        ctx.observer.record(&ObsEvent::NodeFinished {
                            ctx: &node_obs_ctx,
                            outcome: &outcome,
                        });
                        return node_run_output(
                            wf_id,
                            node,
                            run_id,
                            ctx,
                            text,
                            false,
                            None,
                            NodeDisposition::Failed,
                            CompletionBodyOrigin::BridgeSyntheticStreamError,
                        );
                    }
                };
                turn.backend
                    .configure_turn(
                        &turn.session,
                        TurnMeta {
                            context_id: obs_ctx.session_id.clone(),
                            // NodeTurn carries no session-manager generation/op;
                            // mirror the cold-path convention: attempt-scoped
                            // generation and a synthesized workflow operation id.
                            generation: u64::from(attempt),
                            op: OperationId::parse(format!(
                                "workflow-{}-{attempt}",
                                node.id.as_str()
                            ))
                            .expect("workflow operation id is nonempty"),
                            turn_id: obs_ctx.turn_id.clone(),
                            requested_mode: node_harvest_mode,
                            prefix_attestation_request: prefix_attestation_request.clone(),
                        },
                    )
                    .await;
                let rendered_with_contract = append_prompt_contract(
                    rendered.clone(),
                    &prefix_capability,
                    &prefix_attestation_request,
                );
                let mut parts = vec![Part {
                    text: rendered_with_contract,
                }];
                if let Some(seed) = turn.seed {
                    parts.insert(
                        0,
                        Part {
                            text: format!("[Summary of earlier context in this session]\n{seed}"),
                        },
                    );
                }

                ctx.observer
                    .record(&ObsEvent::TurnStarted { ctx: &obs_ctx });
                let turn_start = std::time::Instant::now();
                let mut ttft_val: Option<std::time::Duration> = None;
                let rich_sink = ctx
                    .make_rich_sink
                    .as_ref()
                    .map(|factory| factory.make(&node.id));
                let activity = rich_sink
                    .as_ref()
                    .and_then(|sink| sink.attempt_recorder())
                    .unwrap_or_else(|| {
                        Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder)
                    });
                let (mut stream, terminal_evidence) = tokio::select! {
                    biased;
                    // Prompt-open is the ownership boundary. If the backend result
                    // and workflow cancellation are simultaneously ready, observe
                    // the concrete backend result first so a structured failure
                    // can arm warm-session expiry. A pending prompt still yields
                    // immediately to cancellation.
                    s = async {
                        if let Some(barrier) = prompt_dispatch {
                            barrier().await;
                        }
                        let terminal_evidence = match rich_sink.as_ref() {
                            Some(sink) => sink
                                .terminal_evidence_for_turn(
                                    turn.backend.terminal_evidence_capability(),
                                    u64::from(attempt),
                                    turn.session.as_str(),
                                    obs_ctx.turn_id.as_str(),
                                )?
                                .unwrap_or_else(|| {
                                    Arc::new(
                                        bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                                    )
                                }),
                            None => Arc::new(
                                bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                            ),
                        };
                        let stream = turn.backend.prompt_with_observers(
                            &turn.session,
                            parts,
                            BackendObservers::new(diagnostic.clone(), rich_sink.clone())
                                .with_attempt_telemetry(activity.clone(), terminal_evidence.clone()),
                        ).await?;
                        Ok::<_, BridgeError>((stream, terminal_evidence))
                    } => match s {
                        Ok(s) => s,
                        Err(e) => {
                            turn
                                .cleanup
                                .arm_exit(&NodeTurnExit::Error(e.clone()));
                            if let Some(sink) = &rich_sink {
                                if let Err(flush_error) = sink.flush().await {
                                    eprintln!(
                                        "rich sink flush failed after warm prompt error for node {}: {:?}",
                                        node.id.as_str(),
                                        flush_error
                                    );
                                }
                            }
                            let text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                            let fail_out = TurnOutcome::Failed(classify_failure(&e));
                            let _ = cleanup_warm_turn(
                                turn.cleanup,
                                NodeTurnExit::Error(e),
                                diagnostic.clone(),
                                cleanup_tracker,
                            )
                            .await;
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: &obs_ctx,
                                latency: turn_start.elapsed(),
                                ttft: None,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: &obs_ctx,
                                usage: None,
                                fin: UsageFinalization::TurnFinal,
                            });
                            ctx.observer.record(&ObsEvent::NodeFinished {
                                ctx: &node_obs_ctx,
                                outcome: &fail_out,
                            });
                            return node_run_output(
                                wf_id,
                                node,
                                run_id,
                                ctx,
                                text,
                                false,
                                None,
                                NodeDisposition::Failed,
                                CompletionBodyOrigin::BridgeSyntheticStreamError,
                            );
                        }
                    },
                    _ = cancel.cancelled() => {
                        turn.cleanup.arm_exit(&NodeTurnExit::Canceled);
                        if let Some(sink) = &rich_sink {
                            if let Err(flush_error) = sink.flush().await {
                                eprintln!(
                                    "rich sink flush failed after warm prompt-open cancellation for node {}: {:?}",
                                    node.id.as_str(),
                                    flush_error
                                );
                            }
                        }
                        let cleanup = cleanup_warm_turn(
                            turn.cleanup,
                            NodeTurnExit::Canceled,
                            diagnostic.clone(),
                            cleanup_tracker,
                        )
                        .await;
                        let (text, outcome) = match cleanup {
                            Ok(()) => (
                                format!("[node {} canceled]", node.id.as_str()),
                                TurnOutcome::Canceled,
                            ),
                            Err(error) => (
                                format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error),
                                TurnOutcome::Failed(classify_failure(&error)),
                            ),
                        };
                        ctx.observer.record(&ObsEvent::TurnFinished {
                            ctx: &obs_ctx,
                            latency: turn_start.elapsed(),
                            ttft: None,
                            outcome: &outcome,
                        });
                        ctx.observer.record(&ObsEvent::UsageFinalized {
                            ctx: &obs_ctx,
                            usage: None,
                            fin: UsageFinalization::TurnFinal,
                        });
                        ctx.observer.record(&ObsEvent::NodeFinished {
                            ctx: &node_obs_ctx,
                            outcome: &outcome,
                        });
                        let disposition = NodeDisposition::from_turn(&outcome);
                        let origin = if matches!(disposition, NodeDisposition::Canceled) {
                            CompletionBodyOrigin::BridgeSyntheticCancellation
                        } else {
                            CompletionBodyOrigin::BridgeSyntheticStreamError
                        };
                        return node_run_output(
                            wf_id,
                            node,
                            run_id,
                            ctx,
                            text,
                            false,
                            None,
                            disposition,
                            origin,
                        );
                    }
                };
                let mut text = String::new();
                let mut ok = true;
                let mut saw_done = false;
                let mut done_stop_cancelled = false;
                let mut prefix_attestation_status = PrefixAttestationStatus::default();
                let mut last_usage: Option<UsageSnapshot> = None;
                let mut exit = loop {
                    tokio::select! {
                        biased;
                        // Prompt ownership already exists here. If a backend item
                        // and workflow cancellation are simultaneously ready, the
                        // concrete backend result wins so a queued AgentFailure can
                        // arm warm-session expiry. A pending stream still yields
                        // immediately to cancellation.
                        item = stream.next() => match item {
                            Some(Ok(Update::Text(t))) => {
                                if ttft_val.is_none() && !t.is_empty() {
                                    ttft_val = Some(turn_start.elapsed());
                                }
                                text.push_str(&t);
                                if cancel.is_cancelled() {
                                    ok = false;
                                    text = format!("[node {} canceled]", node.id.as_str());
                                    break NodeTurnExit::Canceled;
                                }
                            }
                            Some(Ok(Update::FinalAnswer(t))) => {
                                if !t.is_empty() {
                                    terminal_evidence.record_deliverable_final();
                                }
                                if ttft_val.is_none() && !t.is_empty() {
                                    ttft_val = Some(turn_start.elapsed());
                                }
                                text.push_str(&t);
                                if cancel.is_cancelled() {
                                    ok = false;
                                    text = format!("[node {} canceled]", node.id.as_str());
                                    break NodeTurnExit::Canceled;
                                }
                            }
                            Some(Ok(Update::Permission(_))) => {
                                if cancel.is_cancelled() {
                                    ok = false;
                                    text = format!("[node {} canceled]", node.id.as_str());
                                    break NodeTurnExit::Canceled;
                                }
                            }
                            Some(Ok(Update::Usage(mut u))) => {
                                if let Some(previous) = &last_usage {
                                    u.merge_missing_from(previous);
                                }
                                last_usage = Some(u);
                                if cancel.is_cancelled() {
                                    ok = false;
                                    text = format!("[node {} canceled]", node.id.as_str());
                                    break NodeTurnExit::Canceled;
                                }
                            }
                            Some(Ok(Update::Done { stop_reason, prefix_attestation })) => {
                                saw_done = true;
                                prefix_attestation_status = prefix_attestation;
                                if stop_reason == STOP_REASON_CANCELLED {
                                    done_stop_cancelled = true;
                                    ok = false;
                                }
                                break NodeTurnExit::Normal;
                            }
                            Some(Err(e)) => {
                                ok = false;
                                text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                                break NodeTurnExit::Error(e);
                            }
                            None => {
                                let dropped = stream_dropped_error();
                                ok = false;
                                text = format!("[node {} failed: {:?}]", node.id.as_str(), dropped);
                                break NodeTurnExit::Error(dropped);
                            }
                        },
                        _ = cancel.cancelled() => {
                            ok = false;
                            text = format!("[node {} canceled]", node.id.as_str());
                            break NodeTurnExit::Canceled;
                        }
                    }
                };
                if matches!(&exit, NodeTurnExit::Error(BridgeError::AgentCrashed { .. })) {
                    record_synthetic_failure(
                        &diagnostic,
                        bridge_core::diagnostics::DiagnosticPhase::PromptStream,
                        "workflow.stream.dropped",
                        bridge_core::diagnostics::DiagnosticFailureClass::AgentProcess,
                        format!(
                            "node {} stream ended before terminal Done",
                            node.id.as_str()
                        ),
                        vec!["missing Update::Done".to_string()],
                    )
                    .await;
                }
                if saw_done && ok && text.trim().is_empty() {
                    record_synthetic_failure(
                        &diagnostic,
                        bridge_core::diagnostics::DiagnosticPhase::PromptFinish,
                        "workflow.empty_final",
                        bridge_core::diagnostics::DiagnosticFailureClass::Protocol,
                        format!(
                            "node {} completed with an empty final agent message",
                            node.id.as_str()
                        ),
                        vec!["empty final agent message".to_string()],
                    )
                    .await;
                    ok = false;
                    text = format!(
                        "[node {} failed: {:?}]",
                        node.id.as_str(),
                        BridgeError::EmptyFinal
                    );
                    exit = NodeTurnExit::Error(BridgeError::EmptyFinal);
                }
                // The cleanup owner must learn the terminal state before rich-sink
                // flush, which is a cancellation point. Warm cleanup uses this to
                // make structured-failure expiry sticky if the executor is dropped
                // while the flush is pending.
                turn.cleanup.arm_exit(&exit);
                if let Some(sink) = &rich_sink {
                    if let Err(flush_error) = sink.flush().await {
                        if ok && matches!(&exit, NodeTurnExit::Normal) {
                            ok = false;
                            text = format!(
                                "[node {} rich-flush failed: {:?}]",
                                node.id.as_str(),
                                flush_error
                            );
                            exit = NodeTurnExit::Error(flush_error);
                            turn.cleanup.arm_exit(&exit);
                        } else {
                            eprintln!(
                                "rich sink flush failed after warm node exit for {}: {:?}",
                                node.id.as_str(),
                                flush_error
                            );
                        }
                    }
                }
                // Settle operation-owned cleanup before terminal observability. A
                // teardown failure is primary only when no earlier backend/rich
                // failure already owns the node outcome.
                let empty_final_failure =
                    matches!(&exit, NodeTurnExit::Error(BridgeError::EmptyFinal));
                let had_primary_failure = matches!(&exit, NodeTurnExit::Error(_));
                // Capture the classification facts needed after `exit` is moved
                // into `on_exit_observed` (origin classification below reads them).
                let exit_was_normal = matches!(&exit, NodeTurnExit::Normal);
                let exit_agent_crashed =
                    matches!(&exit, NodeTurnExit::Error(BridgeError::AgentCrashed { .. }));
                let mut node_outcome = match &exit {
                    NodeTurnExit::Canceled => TurnOutcome::Canceled,
                    NodeTurnExit::Error(error) => TurnOutcome::Failed(classify_failure(error)),
                    NodeTurnExit::Normal if ok => TurnOutcome::Success,
                    NodeTurnExit::Normal => TurnOutcome::Failed(FailureClass::Other),
                };
                let cleanup_result =
                    cleanup_warm_turn(turn.cleanup, exit, diagnostic.clone(), cleanup_tracker)
                        .await;
                if let Err(error) = cleanup_result {
                    if !had_primary_failure {
                        ok = false;
                        text = format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error);
                        node_outcome = TurnOutcome::Failed(classify_failure(&error));
                    }
                    // Otherwise the operation observer recorded bounded teardown
                    // evidence and the earlier backend/rich failure remains primary.
                }

                // Keep whatever usage the agent reported, even if the turn then errored or was
                // cancelled — the tokens were really consumed and belong in the durable footprint.
                // `last_usage` is already `None` when no `Update::Usage` was ever observed.
                ctx.observer.record(&ObsEvent::TurnFinished {
                    ctx: &obs_ctx,
                    latency: turn_start.elapsed(),
                    ttft: ttft_val,
                    outcome: &node_outcome,
                });
                ctx.observer.record(&ObsEvent::UsageFinalized {
                    ctx: &obs_ctx,
                    usage: last_usage.as_ref(),
                    fin: UsageFinalization::TurnFinal,
                });
                ctx.observer.record(&ObsEvent::NodeFinished {
                    ctx: &node_obs_ctx,
                    outcome: &node_outcome,
                });
                let disposition = NodeDisposition::from_turn(&node_outcome);
                let origin = if matches!(node_outcome, TurnOutcome::Canceled) {
                    CompletionBodyOrigin::BridgeSyntheticCancellation
                } else if empty_final_failure {
                    CompletionBodyOrigin::BridgeSyntheticEmptyFinal
                } else if !saw_done {
                    CompletionBodyOrigin::BridgeSyntheticMissingDone
                } else if done_stop_cancelled {
                    CompletionBodyOrigin::BridgeSyntheticCancellation
                } else if ok && exit_was_normal {
                    CompletionBodyOrigin::ModelText
                } else if exit_agent_crashed {
                    CompletionBodyOrigin::BridgeSyntheticMissingDone
                } else {
                    CompletionBodyOrigin::BridgeSyntheticStreamError
                };
                return Ok(NodeRunOutput {
                    text,
                    ok,
                    usage: last_usage,
                    disposition,
                    harvest: node_harvest_meta_from_context(
                        obs_ctx.clone(),
                        node,
                        prefix_capability.clone(),
                        prefix_attestation_status,
                        origin,
                    ),
                });
            }
        }
        let rendered = render(&node.prompt_template, vars);
        let base_session = match SessionId::parse(format!(
            "workflow-{}-{}-{}",
            wf_id,
            node.id.as_str(),
            run_id
        )) {
            Ok(s) => s,
            Err(_) => {
                return node_run_output(
                    wf_id,
                    node,
                    run_id,
                    ctx,
                    format!("[node {} failed: bad session id]", node.id.as_str()),
                    false,
                    None,
                    NodeDisposition::Failed,
                    CompletionBodyOrigin::BridgeSyntheticStreamError,
                )
            }
        };

        enum Attempt {
            Ok {
                text: String,
                usage: Option<UsageSnapshot>,
            },
            Canceled {
                marker: String,
                usage: Option<UsageSnapshot>,
            },
            Fatal {
                text: String,
                usage: Option<UsageSnapshot>,
                failure_class: FailureClass,
            },
            Transient {
                err: BridgeError,
                usage: Option<UsageSnapshot>,
                cleanup_allows_retry: bool,
                invalidation: RetryInvalidation,
            },
            EmptyFinal {
                usage: Option<UsageSnapshot>,
            },
        }

        let attempts = node.retry.as_ref().map(|r| r.attempts()).unwrap_or(1);
        let retry_enabled = node.retry.is_some();

        // Emit NodeStarted exactly once before the retry loop.
        let node_obs_ctx = node_turn_context(wf_id, node, run_id, ctx, None, None, None, 0)?;
        ctx.observer
            .record(&ObsEvent::NodeStarted { ctx: &node_obs_ctx });

        let (final_text, final_ok, final_usage, final_node_outcome, final_harvest) = 'node_loop: {
            let mut attempt = 1_u32;
            loop {
                if cancel.is_cancelled() {
                    break 'node_loop (
                        format!("[node {} canceled]", node.id.as_str()),
                        false,
                        None,
                        TurnOutcome::Canceled,
                        node_harvest_meta(
                            wf_id,
                            node,
                            run_id,
                            ctx,
                            attempt,
                            PrefixAttestationCapability::default(),
                            PrefixAttestationStatus::default(),
                            CompletionBodyOrigin::BridgeSyntheticCancellation,
                        )?,
                    );
                }

                let session = base_session.clone();
                let should_retry_after_attempt = attempt < attempts;
                let mut obs_ctx_opt: Option<TurnContext> = None;
                let mut turn_started: Option<std::time::Instant> = None;
                let mut ttft_val: Option<std::time::Duration> = None;
                let diagnostic = diagnostic_factory.make(&node.id, attempt);
                let mut attempt_harvest = node_harvest_meta(
                    wf_id,
                    node,
                    run_id,
                    ctx,
                    attempt,
                    PrefixAttestationCapability::default(),
                    PrefixAttestationStatus::default(),
                    CompletionBodyOrigin::BridgeSyntheticStreamError,
                )?;
                let outcome = 'attempt: {
                    let (attempt_use, preflight_decision) = if let Some(authority) =
                        frozen_authority
                    {
                        let decision = match self
                            .ensure_frozen_preflight(
                                wf_id,
                                node,
                                run_id,
                                ctx,
                                diagnostic_factory,
                                prompt_dispatch,
                                cancel,
                                authority,
                                preflight_cache,
                                cleanup_tracker,
                            )
                            .await
                        {
                            Ok(decision) => decision,
                            Err(PreflightFailure::Canceled) => {
                                attempt_harvest = node_harvest_meta(
                                    wf_id,
                                    node,
                                    run_id,
                                    ctx,
                                    attempt,
                                    PrefixAttestationCapability::default(),
                                    PrefixAttestationStatus::default(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                )?;
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            Err(PreflightFailure::Hard {
                                message,
                                failure_class,
                                ..
                            }) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!("[node {} failed: {message}]", node.id.as_str()),
                                    usage: None,
                                    failure_class,
                                };
                            }
                        };
                        let frozen = match self.bind_frozen_entry(
                            authority,
                            node,
                            FrozenProviderLogicalSessionV1::Execute {
                                candidate_ordinal: decision.candidate_ordinal,
                            },
                        ) {
                            Ok(frozen) => frozen,
                            Err(error) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage: None,
                                    failure_class: classify_failure(&error),
                                };
                            }
                        };
                        let resolved = tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                attempt_harvest = node_harvest_meta(
                                    wf_id,
                                    node,
                                    run_id,
                                    ctx,
                                    attempt,
                                    PrefixAttestationCapability::default(),
                                    PrefixAttestationStatus::default(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                )?;
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            result = self.resolve_frozen_entry(frozen, diagnostic.clone()) => match result {
                                Ok(resolved) => resolved,
                                Err((error, frozen)) => {
                                    if retry_enabled && error.is_transient() {
                                        break 'attempt Attempt::Transient {
                                            err: error,
                                            usage: None,
                                            cleanup_allows_retry: true,
                                            invalidation: frozen.into_retry_invalidation(),
                                        };
                                    }
                                    break 'attempt Attempt::Fatal {
                                        text: format!("[node {} failed: {:?}]", node.id.as_str(), error),
                                        usage: None,
                                        failure_class: classify_failure(&error),
                                    };
                                }
                            },
                        };
                        (WorkflowAttemptUse::Bound(resolved), decision)
                    } else {
                        // Legacy V1 resolution remains unchanged and never enters the bound path.
                        let mut resolved = tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                attempt_harvest = node_harvest_meta(
                                    wf_id,
                                    node,
                                    run_id,
                                    ctx,
                                    attempt,
                                    PrefixAttestationCapability::default(),
                                    PrefixAttestationStatus::default(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                )?;
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            result = self.registry.resolve_observed(&node.agent, diagnostic.clone()) => match result {
                                Ok(resolved) => resolved,
                                Err(error) => {
                                    if retry_enabled && error.is_transient() {
                                        break 'attempt Attempt::Transient {
                                            err: error,
                                            usage: None,
                                            cleanup_allows_retry: true,
                                            invalidation: RetryInvalidation::Legacy(node.agent.clone()),
                                        };
                                    }
                                    break 'attempt Attempt::Fatal {
                                        text: format!("[node {} failed: {:?}]", node.id.as_str(), error),
                                        usage: None,
                                        failure_class: classify_failure(&error),
                                    };
                                }
                            },
                        };
                        let decision = match self
                            .ensure_agent_preflight(
                                wf_id,
                                node,
                                run_id,
                                ctx,
                                diagnostic_factory,
                                prompt_dispatch,
                                cancel,
                                resolved.entry.clone(),
                                preflight_cache,
                                cleanup_tracker,
                            )
                            .await
                        {
                            Ok(decision) => decision,
                            Err(PreflightFailure::Canceled) => {
                                attempt_harvest = node_harvest_meta(
                                    wf_id,
                                    node,
                                    run_id,
                                    ctx,
                                    attempt,
                                    PrefixAttestationCapability::default(),
                                    PrefixAttestationStatus::default(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                )?;
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            Err(PreflightFailure::Hard {
                                message,
                                failure_class,
                                ..
                            }) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!("[node {} failed: {message}]", node.id.as_str()),
                                    usage: None,
                                    failure_class,
                                };
                            }
                        };
                        if decision.substituted_from.is_some() {
                            resolved = match self
                                .registry
                                .resolve_observed(&node.agent, diagnostic.clone())
                                .await
                            {
                                Ok(resolved) => resolved,
                                Err(error) => {
                                    break 'attempt Attempt::Fatal {
                                        text: format!(
                                            "[node {} failed after preflight: {:?}]",
                                            node.id.as_str(),
                                            error
                                        ),
                                        usage: None,
                                        failure_class: classify_failure(&error),
                                    };
                                }
                            };
                        }
                        let mut config = effective_config(&resolved.entry, None);
                        config.model = decision.selected_model.clone();
                        (WorkflowAttemptUse::Legacy { resolved, config }, decision)
                    };
                    let eff = attempt_use.config().clone();
                    let obs_model = eff.model.clone();
                    let obs_effort = eff
                        .effort
                        .as_ref()
                        .map(|e| format!("{e:?}").to_ascii_lowercase());
                    let obs_mode = eff.mode.clone();
                    let obs_ctx_here = node_turn_context(
                        wf_id, node, run_id, ctx, obs_model, obs_effort, obs_mode, attempt,
                    )?;
                    // NodeStarted was emitted before the loop; only emit TurnStarted here.
                    ctx.observer
                        .record(&ObsEvent::TurnStarted { ctx: &obs_ctx_here });
                    obs_ctx_opt = Some(obs_ctx_here);
                    turn_started = Some(std::time::Instant::now());
                    if preflight_decision.substituted_from.is_some() {
                        tracing::debug!(
                            node = node.id.as_str(),
                            agent = node.agent.as_str(),
                            model = %model_label(preflight_decision.selected_model.as_deref()),
                            "workflow node using preflight-selected fallback model"
                        );
                    }
                    if let Err(e) = attempt_use
                        .configure_session(&session, ctx.session_cwd.clone())
                        .await
                    {
                        if retry_enabled && e.is_transient() {
                            let action = if should_retry_after_attempt {
                                ColdCleanupAction::Release
                            } else {
                                ColdCleanupAction::Forget
                            };
                            let cleanup_allows_retry = cleanup_cold_session(
                                attempt_use.backend(),
                                &session,
                                &diagnostic,
                                action,
                                cleanup_tracker,
                            )
                            .await
                            .is_ok();
                            break 'attempt Attempt::Transient {
                                err: e,
                                usage: None,
                                cleanup_allows_retry,
                                invalidation: attempt_use.into_retry_invalidation(&node.agent),
                            };
                        }
                        let _ = cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await;
                        break 'attempt Attempt::Fatal {
                            text: format!("[node {} failed: configure {:?}]", node.id.as_str(), e),
                            usage: None,
                            failure_class: classify_failure(&e),
                        };
                    }
                    if cancel.is_cancelled() {
                        match cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await
                        {
                            Ok(()) => {
                                attempt_harvest = node_harvest_meta(
                                    wf_id,
                                    node,
                                    run_id,
                                    ctx,
                                    attempt,
                                    PrefixAttestationCapability::default(),
                                    PrefixAttestationStatus::default(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                )?;
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            Err(error) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} cleanup failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage: None,
                                    failure_class: classify_failure(&error),
                                };
                            }
                        }
                    }
                    // prompt, with cancel
                    let prefix_capability = attempt_use.backend().prefix_attestation_capability();
                    let node_harvest_mode = node.harvest_sanitization.unwrap_or_default();
                    let prefix_attestation_request = match prefix_attestation_request_for_capability(
                        node_harvest_mode,
                        &prefix_capability,
                    ) {
                        Ok(request) => request,
                        Err(e) => {
                            let _ = cleanup_cold_session(
                                attempt_use.backend(),
                                &session,
                                &diagnostic,
                                ColdCleanupAction::Forget,
                                cleanup_tracker,
                            )
                            .await;
                            break 'attempt Attempt::Fatal {
                                text: format!("[node {} failed: {:?}]", node.id.as_str(), e),
                                usage: None,
                                failure_class: classify_failure(&e),
                            };
                        }
                    };
                    // The per-attempt context was stashed into `obs_ctx_opt`
                    // just above; borrow it back for the turn identifiers.
                    let obs_ctx_ref = obs_ctx_opt
                        .as_ref()
                        .expect("turn context is stashed before the prompt is dispatched");
                    attempt_use
                        .backend()
                        .configure_turn(
                            &session,
                            TurnMeta {
                                context_id: obs_ctx_ref.session_id.clone(),
                                generation: u64::from(attempt),
                                op: OperationId::parse(format!(
                                    "workflow-{}-{attempt}",
                                    node.id.as_str()
                                ))
                                .expect("workflow operation id is nonempty"),
                                turn_id: obs_ctx_ref.turn_id.clone(),
                                requested_mode: node_harvest_mode,
                                prefix_attestation_request: prefix_attestation_request.clone(),
                            },
                        )
                        .await;
                    let prompt_parts = vec![Part {
                        text: append_prompt_contract(
                            rendered.clone(),
                            &prefix_capability,
                            &prefix_attestation_request,
                        ),
                    }];
                    let rich_sink = ctx
                        .make_rich_sink
                        .as_ref()
                        .map(|factory| factory.make(&node.id));
                    let activity = rich_sink
                        .as_ref()
                        .and_then(|sink| sink.attempt_recorder())
                        .unwrap_or_else(|| {
                            Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder)
                        });
                    let (mut stream, terminal_evidence) = tokio::select! {
                        biased;
                        s = async {
                            if let Some(barrier) = prompt_dispatch {
                                barrier().await;
                            }
                            let terminal_evidence = match rich_sink.as_ref() {
                                Some(sink) => sink
                                    .terminal_evidence_for_turn(
                                        attempt_use.backend().terminal_evidence_capability(),
                                        u64::from(attempt),
                                        session.as_str(),
                                        obs_ctx_ref.turn_id.as_str(),
                                    )?
                                    .unwrap_or_else(|| {
                                        Arc::new(
                                            bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                                        )
                                    }),
                                None => Arc::new(
                                    bridge_core::terminal_evidence::SharedTurnEvidence::unsupported(),
                                ),
                            };
                            let stream = attempt_use.backend().prompt_with_observers(
                                &session,
                                prompt_parts,
                                BackendObservers::new(diagnostic.clone(), rich_sink.clone())
                                    .with_attempt_telemetry(activity.clone(), terminal_evidence.clone()),
                            ).await?;
                            Ok::<_, BridgeError>((stream, terminal_evidence))
                        } => match s {
                            Ok(s) => s,
                            Err(e) => {
                                if let Some(sink) = &rich_sink {
                                    if let Err(flush_err) = sink.flush().await {
                                        eprintln!(
                                            "rich sink flush failed after prompt error for node {}: {:?}",
                                            node.id.as_str(),
                                            flush_err
                                        );
                                    }
                                }
                                if retry_enabled && e.is_transient() {
                                    let action = if should_retry_after_attempt {
                                        ColdCleanupAction::Release
                                    } else {
                                        ColdCleanupAction::Forget
                                    };
                                    let cleanup_allows_retry = cleanup_cold_session(
                                        attempt_use.backend(),
                                        &session,
                                        &diagnostic,
                                        action,
                                        cleanup_tracker,
                                    )
                                    .await
                                    .is_ok();
                                    break 'attempt Attempt::Transient {
                                        err: e,
                                        usage: None,
                                        cleanup_allows_retry,
                                        invalidation: attempt_use
                                            .into_retry_invalidation(&node.agent),
                                    };
                                }
                                let _ = cleanup_cold_session(
                                    attempt_use.backend(),
                                    &session,
                                    &diagnostic,
                                    ColdCleanupAction::Forget,
                                    cleanup_tracker,
                                )
                                .await;
                                break 'attempt Attempt::Fatal {
                                    text: format!("[node {} failed: {:?}]", node.id.as_str(), e),
                                    usage: None,
                                    failure_class: classify_failure(&e),
                                };
                            }
                        },
                        _ = cancel.cancelled() => {
                            if let Some(sink) = &rich_sink {
                                if let Err(flush_error) = sink.flush().await {
                                    eprintln!(
                                        "rich sink flush failed after cold prompt-open cancellation for node {}: {:?}",
                                        node.id.as_str(),
                                        flush_error
                                    );
                                }
                            }
                            // The prompt future may have crossed the provider's
                            // acceptance boundary before yielding its stream. A
                            // workflow cancel must therefore request backend
                            // cancellation even while prompt-open is pending,
                            // then settle the session teardown before projecting
                            // a terminal outcome.
                            let cancel_error = attempt_use
                                .backend()
                                .cancel_observed(&session, diagnostic.clone())
                                .await
                                .err();
                            let cleanup_error = cleanup_cold_session(
                                attempt_use.backend(),
                                &session,
                                &diagnostic,
                                ColdCleanupAction::Forget,
                                cleanup_tracker,
                            )
                            .await
                            .err();
                            match cancel_error.or(cleanup_error) {
                                Some(error) => {
                                    break 'attempt Attempt::Fatal {
                                        text: format!(
                                            "[node {} cleanup failed: {:?}]",
                                            node.id.as_str(),
                                            error
                                        ),
                                        usage: None,
                                        failure_class: classify_failure(&error),
                                    };
                                }
                                None => {
                                    attempt_harvest = node_harvest_meta_from_context(
                                        obs_ctx_ref.clone(),
                                        node,
                                        prefix_capability.clone(),
                                        PrefixAttestationStatus::default(),
                                        CompletionBodyOrigin::BridgeSyntheticCancellation,
                                    );
                                    break 'attempt Attempt::Canceled {
                                        marker: format!("[node {} canceled]", node.id.as_str()),
                                        usage: None,
                                    };
                                }
                            }
                        }
                    };
                    let mut text = String::new();
                    let mut ok = true;
                    let mut canceled_during_drain = false;
                    let mut saw_done = false;
                    let mut done_stop_cancelled = false;
                    let mut prefix_attestation_status = PrefixAttestationStatus::default();
                    let mut last_usage: Option<UsageSnapshot> = None;
                    let mut err: Option<BridgeError> = None;
                    loop {
                        tokio::select! {
                            biased;
                            item = stream.next() => match item {
                                Some(Ok(Update::Text(t))) => {
                                    if ttft_val.is_none() && !t.is_empty() {
                                        if let Some(start) = turn_started {
                                            ttft_val = Some(start.elapsed());
                                        }
                                    }
                                    text.push_str(&t);
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::FinalAnswer(t))) => {
                                    if !t.is_empty() {
                                        terminal_evidence.record_deliverable_final();
                                    }
                                    if ttft_val.is_none() && !t.is_empty() {
                                        if let Some(start) = turn_started {
                                            ttft_val = Some(start.elapsed());
                                        }
                                    }
                                    text.push_str(&t);
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Permission(_))) => {
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Usage(mut u))) => {
                                    if let Some(previous) = &last_usage {
                                        u.merge_missing_from(previous);
                                    }
                                    last_usage = Some(u);
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Done { stop_reason, prefix_attestation })) => {
                                    saw_done = true;
                                    prefix_attestation_status = prefix_attestation;
                                    if stop_reason == STOP_REASON_CANCELLED {
                                        done_stop_cancelled = true;
                                        ok = false;
                                    }
                                    break;
                                }
                                Some(Err(e)) => {
                                    ok = false;
                                    text = format!(
                                        "[node {} failed: {:?}]",
                                        node.id.as_str(),
                                        e
                                    );
                                    err = Some(e);
                                    break;
                                }
                                None => {
                                    let dropped = stream_dropped_error();
                                    ok = false;
                                    text = format!(
                                        "[node {} failed: {:?}]",
                                        node.id.as_str(),
                                        dropped
                                    );
                                    err = Some(dropped);
                                    break;
                                }
                            },
                            _ = cancel.cancelled() => {
                                canceled_during_drain = true;
                                ok = false;
                                text = format!("[node {} canceled]", node.id.as_str());
                                break;
                            }
                        }
                    }
                    let cancel_error = if canceled_during_drain {
                        attempt_use
                            .backend()
                            .cancel_observed(&session, diagnostic.clone())
                            .await
                            .err()
                    } else {
                        None
                    };
                    // Keep whatever usage the agent reported, even on error/cancel (see the warm path):
                    // `last_usage` is `None` only when no `Update::Usage` was ever observed.
                    let mut usage = last_usage;
                    if let Some(sink) = &rich_sink {
                        if let Err(e) = sink.flush().await {
                            if !ok {
                                let exit = if canceled_during_drain {
                                    "node cancellation"
                                } else {
                                    "node failure"
                                };
                                eprintln!(
                                    "rich sink flush failed after {exit} for node {}: {:?}",
                                    node.id.as_str(),
                                    e
                                );
                                usage = None;
                            } else {
                                let _ = cleanup_cold_session(
                                    attempt_use.backend(),
                                    &session,
                                    &diagnostic,
                                    ColdCleanupAction::Forget,
                                    cleanup_tracker,
                                )
                                .await;
                                attempt_harvest = node_harvest_meta_from_context(
                                    obs_ctx_ref.clone(),
                                    node,
                                    prefix_capability.clone(),
                                    prefix_attestation_status.clone(),
                                    CompletionBodyOrigin::BridgeSyntheticStreamError,
                                );
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} rich-flush failed: {:?}]",
                                        node.id.as_str(),
                                        e
                                    ),
                                    usage: None,
                                    failure_class: FailureClass::Other,
                                };
                            }
                        }
                    }
                    if canceled_during_drain {
                        let cleanup_error = cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await
                        .err();
                        match cancel_error.or(cleanup_error) {
                            Some(error) => {
                                attempt_harvest = node_harvest_meta_from_context(
                                    obs_ctx_ref.clone(),
                                    node,
                                    prefix_capability.clone(),
                                    prefix_attestation_status.clone(),
                                    CompletionBodyOrigin::BridgeSyntheticStreamError,
                                );
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} cleanup failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage,
                                    failure_class: classify_failure(&error),
                                };
                            }
                            None => {
                                attempt_harvest = node_harvest_meta_from_context(
                                    obs_ctx_ref.clone(),
                                    node,
                                    prefix_capability.clone(),
                                    prefix_attestation_status.clone(),
                                    CompletionBodyOrigin::BridgeSyntheticCancellation,
                                );
                                break 'attempt Attempt::Canceled {
                                    marker: text,
                                    usage,
                                };
                            }
                        }
                    }
                    if matches!(err, Some(BridgeError::AgentCrashed { .. })) {
                        record_synthetic_failure(
                            &diagnostic,
                            bridge_core::diagnostics::DiagnosticPhase::PromptStream,
                            "workflow.stream.dropped",
                            bridge_core::diagnostics::DiagnosticFailureClass::AgentProcess,
                            format!(
                                "node {} stream ended before terminal Done",
                                node.id.as_str()
                            ),
                            vec!["missing Update::Done".to_string()],
                        )
                        .await;
                    }
                    if saw_done && ok && text.trim().is_empty() {
                        record_synthetic_failure(
                            &diagnostic,
                            bridge_core::diagnostics::DiagnosticPhase::PromptFinish,
                            "workflow.empty_final",
                            bridge_core::diagnostics::DiagnosticFailureClass::Protocol,
                            format!(
                                "node {} completed with an empty final agent message",
                                node.id.as_str()
                            ),
                            vec!["empty final agent message".to_string()],
                        )
                        .await;
                        let _ = cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await;
                        attempt_harvest = node_harvest_meta_from_context(
                            obs_ctx_ref.clone(),
                            node,
                            prefix_capability.clone(),
                            prefix_attestation_status.clone(),
                            CompletionBodyOrigin::BridgeSyntheticEmptyFinal,
                        );
                        break 'attempt Attempt::EmptyFinal { usage };
                    }
                    if let Some(e) = err {
                        if retry_enabled && e.is_transient() {
                            let action = if should_retry_after_attempt {
                                ColdCleanupAction::Release
                            } else {
                                ColdCleanupAction::Forget
                            };
                            let cleanup_allows_retry = cleanup_cold_session(
                                attempt_use.backend(),
                                &session,
                                &diagnostic,
                                action,
                                cleanup_tracker,
                            )
                            .await
                            .is_ok();
                            break 'attempt Attempt::Transient {
                                err: e,
                                usage,
                                cleanup_allows_retry,
                                invalidation: attempt_use.into_retry_invalidation(&node.agent),
                            };
                        }
                        let fc = classify_failure(&e);
                        let _ = cleanup_cold_session(
                            attempt_use.backend(),
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                            cleanup_tracker,
                        )
                        .await;
                        let origin = if matches!(&e, BridgeError::AgentCrashed { .. }) {
                            CompletionBodyOrigin::BridgeSyntheticMissingDone
                        } else {
                            CompletionBodyOrigin::BridgeSyntheticStreamError
                        };
                        attempt_harvest = node_harvest_meta_from_context(
                            obs_ctx_ref.clone(),
                            node,
                            prefix_capability.clone(),
                            prefix_attestation_status.clone(),
                            origin,
                        );
                        break 'attempt Attempt::Fatal {
                            text,
                            usage,
                            failure_class: fc,
                        };
                    }
                    let cleanup = cleanup_cold_session(
                        attempt_use.backend(),
                        &session,
                        &diagnostic,
                        ColdCleanupAction::Forget,
                        cleanup_tracker,
                    )
                    .await;
                    if ok {
                        match cleanup {
                            Ok(()) => {
                                attempt_harvest = node_harvest_meta_from_context(
                                    obs_ctx_ref.clone(),
                                    node,
                                    prefix_capability.clone(),
                                    prefix_attestation_status.clone(),
                                    CompletionBodyOrigin::ModelText,
                                );
                                Attempt::Ok { text, usage }
                            }
                            Err(error) => {
                                attempt_harvest = node_harvest_meta_from_context(
                                    obs_ctx_ref.clone(),
                                    node,
                                    prefix_capability.clone(),
                                    prefix_attestation_status.clone(),
                                    CompletionBodyOrigin::BridgeSyntheticStreamError,
                                );
                                Attempt::Fatal {
                                    text: format!(
                                        "[node {} cleanup failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage,
                                    failure_class: classify_failure(&error),
                                }
                            }
                        }
                    } else {
                        let origin = if done_stop_cancelled {
                            CompletionBodyOrigin::BridgeSyntheticCancellation
                        } else {
                            CompletionBodyOrigin::BridgeSyntheticStreamError
                        };
                        attempt_harvest = node_harvest_meta_from_context(
                            obs_ctx_ref.clone(),
                            node,
                            prefix_capability.clone(),
                            prefix_attestation_status.clone(),
                            origin,
                        );
                        Attempt::Fatal {
                            text,
                            usage,
                            failure_class: FailureClass::Other,
                        }
                    }
                };

                match outcome {
                    Attempt::Canceled { marker, usage } => {
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: None,
                                outcome: &TurnOutcome::Canceled,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (
                            marker,
                            false,
                            usage,
                            TurnOutcome::Canceled,
                            attempt_harvest.clone(),
                        );
                    }
                    Attempt::Ok { text, usage } => {
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: ttft_val,
                                outcome: &TurnOutcome::Success,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (
                            text,
                            true,
                            usage,
                            TurnOutcome::Success,
                            attempt_harvest.clone(),
                        );
                    }
                    Attempt::Fatal {
                        text,
                        usage,
                        failure_class,
                    } => {
                        let fail_out = TurnOutcome::Failed(failure_class);
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: ttft_val,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (text, false, usage, fail_out, attempt_harvest.clone());
                    }
                    Attempt::EmptyFinal { usage } => {
                        let fail_out =
                            TurnOutcome::Failed(classify_failure(&BridgeError::EmptyFinal));
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: ttft_val,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (
                            format!(
                                "[node {} failed: {:?}]",
                                node.id.as_str(),
                                BridgeError::EmptyFinal
                            ),
                            false,
                            usage,
                            fail_out,
                            attempt_harvest.clone(),
                        );
                    }
                    Attempt::Transient {
                        err,
                        usage,
                        cleanup_allows_retry,
                        invalidation,
                    } => {
                        let err_for_log = err.clone();
                        let fail_class = classify_failure(&err);
                        let fail_out = TurnOutcome::Failed(fail_class);
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: None,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        if should_retry_after_attempt && cleanup_allows_retry {
                            invalidation.apply(self.registry.as_ref()).await;
                            tracing::warn!(
                                node = node.id.as_str(),
                                attempt,
                                error = ?err_for_log,
                                "node retry"
                            );
                            let retry = node.retry.as_ref().expect("retry attempts require policy");
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => {
                                    break 'node_loop (
                                        format!("[node {} canceled]", node.id.as_str()),
                                        false,
                                        None,
                                        TurnOutcome::Canceled,
                                        node_harvest_meta(
                                            wf_id,
                                            node,
                                            run_id,
                                            ctx,
                                            attempt,
                                            PrefixAttestationCapability::default(),
                                            PrefixAttestationStatus::default(),
                                            CompletionBodyOrigin::BridgeSyntheticCancellation,
                                        )?,
                                    );
                                }
                                _ = tokio::time::sleep(retry.backoff_for(attempt)) => {}
                            }
                            attempt = attempt.saturating_add(1);
                            continue;
                        }
                        if should_retry_after_attempt {
                            break 'node_loop (
                                format!(
                                    "[node {} failed on attempt {attempt}: {err:?}]",
                                    node.id.as_str()
                                ),
                                false,
                                usage,
                                fail_out,
                                attempt_harvest.clone(),
                            );
                        }
                        break 'node_loop (
                            format!(
                                "[node {} failed after {attempts} attempts: {err:?}]",
                                node.id.as_str()
                            ),
                            false,
                            usage,
                            fail_out,
                            attempt_harvest.clone(),
                        );
                    }
                }
            }
        };

        // Emit NodeFinished exactly once after the retry loop.
        ctx.observer.record(&ObsEvent::NodeFinished {
            ctx: &node_obs_ctx,
            outcome: &final_node_outcome,
        });
        let disposition = NodeDisposition::from_turn(&final_node_outcome);
        Ok(NodeRunOutput {
            text: final_text,
            ok: final_ok,
            usage: final_usage,
            disposition,
            harvest: final_harvest,
        })
    }

    /// Run a workflow from scratch (no prior checkpoints).
    /// Thin wrapper over [`run_from`](Self::run_from) with an empty seed and default context.
    pub fn run(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
    ) -> WorkflowStream {
        self.run_with_context(graph, input, run_id, cancel, WorkflowRunContext::default())
    }

    /// Run a workflow from scratch with an explicit per-request context.
    /// Thin wrapper over [`run_from_with_context`](Self::run_from_with_context) with an empty seed.
    pub fn run_with_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowRunContext,
    ) -> WorkflowStream {
        self.run_with_diagnostic_context(
            graph,
            input,
            run_id,
            cancel,
            WorkflowDiagnosticContext::in_memory(ctx),
        )
    }

    pub fn run_with_diagnostic_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowDiagnosticContext,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context(graph, input, run_id, cancel, HashMap::new(), ctx)
    }

    pub fn run_with_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowRunContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_with_diagnostic_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            WorkflowDiagnosticContext::in_memory(ctx),
            dispatcher,
        )
    }

    pub fn run_with_diagnostic_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowDiagnosticContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            HashMap::new(),
            ctx,
            dispatcher,
        )
    }

    /// Resume a workflow from a pre-loaded seed of already-completed node outputs.
    /// Seeded nodes are treated as done; only un-seeded nodes actually run.
    /// `run()` is a thin wrapper over this with an empty seed and default context.
    ///
    /// Each seed entry is `(output_text, ok, usage)`, matching the `NodeFinished` payload.
    ///
    /// # Errors (streamed)
    /// - `BridgeError::ConfigInvalid` if a seed key is not in `graph.nodes`.
    /// - `BridgeError::ConfigInvalid` if the seed is not closed under `inputs`
    ///   (a non-root seeded node's upstream is missing from the seed).
    pub fn run_from(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
    ) -> WorkflowStream {
        self.run_from_with_context(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowRunContext::default(),
        )
    }

    /// Resume a workflow from a pre-loaded seed with an explicit per-request context.
    /// The context is forwarded opaquely to each node's `configure_session` call
    /// (via `SessionSpec.cwd`). The scheduling/topo logic does NOT read it.
    pub fn run_from_with_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowDiagnosticContext::in_memory(ctx),
        )
    }

    pub fn run_from_with_diagnostic_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowDiagnosticContext,
    ) -> WorkflowStream {
        let (ctx, diagnostic_factory, prompt_dispatch, policy_trigger, frozen_authority) =
            ctx.into_parts();
        self.run_from_with_context_inner(
            graph,
            input,
            run_id,
            cancel,
            seed,
            ctx,
            diagnostic_factory,
            prompt_dispatch,
            policy_trigger,
            frozen_authority,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_from_with_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowDiagnosticContext::in_memory(ctx),
            dispatcher,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_from_with_diagnostic_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowDiagnosticContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        let (ctx, diagnostic_factory, prompt_dispatch, policy_trigger, frozen_authority) =
            ctx.into_parts();
        self.run_from_with_context_inner(
            graph,
            input,
            run_id,
            cancel,
            seed,
            ctx,
            diagnostic_factory,
            prompt_dispatch,
            policy_trigger,
            frozen_authority,
            Some(dispatcher),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_from_with_context_inner(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
        diagnostic_factory: Arc<dyn DiagnosticObserverFactory>,
        prompt_dispatch: Option<PromptDispatchBarrier>,
        policy_trigger_barrier: Option<PolicyTriggerBarrier>,
        frozen_authority: Option<FrozenWorkflowAuthority>,
        dispatcher: Option<Arc<dyn WorkflowNodeDispatcher>>,
    ) -> WorkflowStream {
        let this = WorkflowExecutor {
            registry: self.registry.clone(),
        };
        Box::pin(async_stream::stream! {
            if let Some(authority) = frozen_authority.as_ref() {
                if graph.as_ref() != &authority.run_spec.graph {
                    yield Err(BridgeError::ConfigMismatch { field: "workflow_graph" });
                    return;
                }
            }
            let cleanup_tracker = Arc::new(WorkflowCleanupTracker::default());
            // Off-mode audit exemption (§18-7, operator-adjudicated): audit
            // rows are durable whenever the feature is enabled for at least
            // one node that can still RUN in this invocation; workflows with
            // zero runnable enabled nodes are audit-exempt. Seeded nodes are
            // already-completed resume outputs — they are never re-sanitized
            // (§15.2 criterion 17) and therefore need no audit durability, so
            // a fully-seeded resume passes even with a non-retaining store.
            let audit_required = graph.nodes.iter().any(|node| {
                matches!(
                    node.harvest_sanitization.unwrap_or_default(),
                    HarvestSanitizationMode::AttestedPrefixV1
                ) && !seed.contains_key(node.id.as_str())
            });
            if audit_required && !ctx.harvest_audit_store.retains_audit_records() {
                if let Some(node) = graph.nodes.iter().find(|node| {
                    matches!(
                        node.harvest_sanitization.unwrap_or_default(),
                        HarvestSanitizationMode::AttestedPrefixV1
                    ) && !seed.contains_key(node.id.as_str())
                }) {
                    yield Err(BridgeError::ConfigInvalid {
                        reason: format!(
                            "workflow node {} enables harvest_sanitization=attested_prefix_v1 but this workflow context has no retaining harvest audit store",
                            node.id.as_str()
                        ),
                    });
                    return;
                }
            }
            let base_render_vars = match render_vars_for_input(&input) {
                Ok(vars) => vars,
                Err(msg) => {
                    yield Ok(WorkflowEvent::CleanupObserved {
                        disposition: WorkflowCleanupDisposition::NotNeeded,
                        duration_ms: 0,
                    });
                    yield Ok(WorkflowEvent::Terminal {
                        outcome: WorkflowOutcome::Failed,
                        output: msg,
                    });
                    return;
                }
            };

            // --- Seed validation ---
            // 1. Every seed key must name a real node.
            let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
            for key in seed.keys() {
                if !node_ids.contains(key.as_str()) {
                    yield Err(BridgeError::ConfigInvalid {
                        reason: "resume seed references unknown node".into(),
                    });
                    return;
                }
            }
            // 2. The seed must be closed under inputs: for every seeded non-root node,
            //    all of its declared inputs must also be in the seed.
            for node in graph.nodes.iter() {
                if seed.contains_key(node.id.as_str()) {
                    for inp in &node.inputs {
                        if !seed.contains_key(inp.as_str()) {
                            yield Err(BridgeError::ConfigInvalid {
                                reason: "resume seed is not closed under inputs".into(),
                            });
                            return;
                        }
                    }
                }
            }

            let mut dispositions: HashMap<String, NodeDisposition> = seed
                .iter()
                .map(|(node, (_, ok, _))| {
                    (
                        node.clone(),
                        if *ok {
                            NodeDisposition::Completed
                        } else {
                            NodeDisposition::Failed
                        },
                    )
                })
                .collect();
            let mut outputs: HashMap<String, (String, bool, Option<UsageSnapshot>)> = seed;
            let mut done: HashSet<String> = outputs.keys().cloned().collect();
            let terminal_id = graph.terminal().map(|n| n.id.as_str().to_string()).unwrap_or_default();

            // Box the per-node future to one uniform type: the `schedule_ready!`
            // macro expands at two textual sites and each bare `async move {}`
            // would otherwise be a *distinct* anonymous type, which a monomorphic
            // `FuturesUnordered<Fut>` cannot hold.
            let mut inflight: FuturesUnordered<NodeFut> = FuturesUnordered::new();
            let preflight_cache: PreflightCache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
            let mut scheduled: HashSet<String> = HashSet::new();
            let mut stop_scheduling = false; // set on cancel: drain in-flight, schedule nothing new
            let mut node_cancels = BTreeMap::<NodeId, CancellationToken>::new();
            let mut policy_canceled = HashSet::<String>::new();
            let mut node_terminals = BTreeMap::<NodeId, NodeTerminalV1>::new();
            let mut node_refs = BTreeMap::<NodeId, PolicyNodeRefV1>::new();
            let mut fanout_controller = frozen_authority
                .as_ref()
                .map(|authority| FanOutControllerV1::new(authority.run_spec.controls.fan_out.clone()));
            if let Some(authority) = frozen_authority.as_ref() {
                for node in &graph.nodes {
                    let identity = match authority.node_identity(node) {
                        Ok(identity) => identity,
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    };
                    node_refs.insert(node.id.clone(), identity.node.clone());
                }
            }

            // Push every not-done/not-scheduled node whose inputs are all done.
            // Returns the node ids newly scheduled (so the caller can emit NodeStarted).
            // NOTE: `ctx` is captured by clone into each node future (forwarded opaquely,
            // like `run_id`/`cancel`). The topo/scheduling logic above does NOT read it —
            // executor purity is preserved.
            macro_rules! schedule_ready {
                () => {{
                    let mut started: Vec<NodeId> = Vec::new();
                    if !stop_scheduling {
                        for n in graph.nodes.iter() {
                            let id = n.id.as_str();
                            if done.contains(id) || scheduled.contains(id) {
                                continue;
                            }
                            if n.inputs.iter().all(|i| done.contains(i.as_str())) {
                                scheduled.insert(id.to_string());
                                started.push(n.id.clone());
                                let mut owned: Vec<(String, String)> = base_render_vars.clone();
                                for inp in &n.inputs {
                                    if let Some((t, _, _)) = outputs.get(inp.as_str()) {
                                        owned.push((inp.as_str().into(), t.clone()));
                                    }
                                }
                                // Single-upstream alias: a node with exactly one input can render its
                                // predecessor's output as `{{draft}}` without hard-coding the predecessor's
                                // node id — so one refine prompt serves model-diverse legs whose draft nodes
                                // have distinct ids (e.g. reviewer_codex_draft / reviewer_claude_draft).
                                if let [only] = n.inputs.as_slice() {
                                    if let Some((t, _, _)) = outputs.get(only.as_str()) {
                                        owned.push(("draft".into(), t.clone()));
                                    }
                                }
                                if !n.inputs.is_empty() {
                                    let cost_rows: Vec<(String, Option<UsageSnapshot>)> = n.inputs.iter()
                                        .map(|inp| {
                                            (
                                                inp.as_str().to_string(),
                                                outputs
                                                    .get(inp.as_str())
                                                    .and_then(|(_, _, usage)| usage.clone()),
                                            )
                                        })
                                        .collect();
                                    owned.push(("workflow.costs".into(), render_costs_table(&cost_rows)));
                                    owned.push(("workflow.weights".into(), render_weights(&graph.panel)));
                                }
                                let node = n.clone();
                                let run_id = run_id.clone();
                                let node_cancel = cancel.child_token();
                                node_cancels.insert(n.id.clone(), node_cancel.clone());
                                let cancel = node_cancel;
                                let wf_id = graph.id.as_str().to_string();
                                let ctx = ctx.clone();
                                let diagnostic_factory = diagnostic_factory.clone();
                                let prompt_dispatch = prompt_dispatch.clone();
                                let cleanup_tracker = cleanup_tracker.clone();
                                let dispatcher = dispatcher.clone();
                                let preflight_cache = preflight_cache.clone();
                                let frozen_authority = frozen_authority.clone();
                                let this = &this;
                                inflight.push(Box::pin(async move {
                                    let vars: HashMap<&str, &str> =
                                        owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                                    let output = this.run_node(
                                        &wf_id,
                                        &node,
                                        &vars,
                                        &run_id,
                                        &cancel,
                                        &ctx,
                                        &diagnostic_factory,
                                        &prompt_dispatch,
                                        cleanup_tracker.as_ref(),
                                        dispatcher.as_ref(),
                                        &preflight_cache,
                                        frozen_authority.as_ref(),
                                    ).await;
                                    (node.id.clone(), output)
                                }) as NodeFut);
                            }
                        }
                    }
                    started
                }};
            }

            for node in schedule_ready!() {
                yield Ok(WorkflowEvent::NodeStarted { node });
            }
            loop {
                let workflow_cancel_observable_before_wait = cancel.is_cancelled();
                let Some(first) = inflight.next().await else {
                    break;
                };
                let mut ready = vec![first];
                while let Some(Some(next)) = inflight.next().now_or_never() {
                    ready.push(next);
                }
                ready.sort_by(|left, right| left.0.cmp(&right.0));

                let mut completed = Vec::with_capacity(ready.len());
                let mut ready_terminals = Vec::with_capacity(ready.len());
                let mut ready_event_evidence = BTreeMap::<
                    NodeId,
                    (String, Option<String>, Option<PolicyTriggerBarrierResultV1>),
                >::new();
                for (node_id, raw_output) in ready {
                    node_cancels.remove(&node_id);
                    let raw_output = match raw_output {
                        Ok(output) => output,
                        Err(error) => {
                            yield Err(error);
                            return;
                        }
                    };
                    let NodeRunOutput {
                        text: raw_text,
                        ok,
                        usage,
                        disposition,
                        harvest,
                    } = raw_output;
                    // §18-7 Off-mode audit exemption: with zero runnable enabled
                    // nodes there is no audit commit at all — Off mode is the §7.1
                    // identity, so the raw body IS the effective body, and no
                    // KeptOff decision is minted only to be discarded by a noop
                    // store. When any runnable node enables the feature, EVERY
                    // completion of the run (enabled and Off nodes alike, §18-4
                    // regardless of `ok`) commits its bundle before release.
                    let text = if audit_required {
                        let committed = match commit_harvested_completion(
                            &harvest.context,
                            harvest.mode,
                            &harvest.capability,
                            &harvest.producer_id,
                            harvest.origin,
                            raw_text,
                            harvest.status.clone(),
                            ctx.harvest_audit_store.as_ref(),
                        )
                        .await
                        {
                            Ok(committed) => committed,
                            Err(error) => {
                                yield Err(BridgeError::from(error));
                                return;
                            }
                        };
                        if let Some(factory) = &ctx.make_rich_sink {
                            let sink = factory.make(&node_id);
                            sink.record(bridge_core::orch::OrchEventKind::HarvestSanitizationDecision {
                                audit_id: committed.audit_id.clone(),
                                run_id: harvest.context.session_id.as_str().to_string(),
                                node_id: node_id.as_str().to_string(),
                                attempt_id: harvest.context.attempt,
                                producer_id: harvest.producer_id.clone(),
                                mode: committed.decision.mode,
                                decision: committed.decision.decision,
                                reason: committed.decision.reason.clone(),
                            });
                            if let Err(error) = sink.flush().await {
                                tracing::warn!(
                                    node = node_id.as_str(),
                                    audit_id = committed.audit_id.as_str(),
                                    error = ?error,
                                    "harvest sanitization decision event flush failed"
                                );
                            }
                        }
                        committed.effective_body
                    } else {
                        raw_text
                    };
                    let primary = match disposition {
                        NodeDisposition::Completed => NodePrimaryDispositionV1::Completed,
                        NodeDisposition::Failed => NodePrimaryDispositionV1::Failed,
                        NodeDisposition::Canceled
                            if policy_canceled.contains(node_id.as_str()) =>
                        {
                            NodePrimaryDispositionV1::CanceledPolicy
                        }
                        NodeDisposition::Canceled => NodePrimaryDispositionV1::CanceledWorkflow,
                    };
                    ready_terminals.push(ReadyNodeTerminalV1 {
                        node: node_id.clone(),
                        terminal: NodeTerminalV1 {
                            schema_version: EXECUTION_POLICY_SCHEMA_V1,
                            primary,
                            cleanup: NodeCleanupV1 {
                                disposition: NodeCleanupDispositionV1::NotNeeded,
                                duration_ms: 0,
                            },
                            cause: None,
                            prompt_may_have_been_accepted: false,
                            degraded_ancestry: false,
                            policy_trigger_id: None,
                        },
                    });
                    completed.push((node_id, text, ok, usage, disposition));
                }

                let mut action = PolicyActionV1::None;
                if let (Some(controller), Some(authority)) =
                    (fanout_controller.as_mut(), frozen_authority.as_ref())
                {
                    let selection = match controller.finalize_ready_batch(
                        &authority.run_spec.attempt_id,
                        ready_terminals,
                        &node_refs,
                        workflow_cancel_observable_before_wait,
                    ) {
                        Ok(selection) => selection,
                        Err(_) => {
                            yield Err(BridgeError::InvalidStateTransition);
                            return;
                        }
                    };
                    let trigger_json = match selection.trigger.as_ref() {
                        Some(trigger) => match canonical_trigger_json_v1(trigger) {
                            Ok(encoded) => Some(encoded),
                            Err(error) => {
                                yield Err(error);
                                return;
                            }
                        },
                        None => None,
                    };
                    let mut selected_node = None;
                    for ready in &selection.terminals {
                        let terminal_json = match canonical_terminal_json_v1(&ready.terminal) {
                            Ok(encoded) => encoded,
                            Err(error) => {
                                yield Err(error);
                                return;
                            }
                        };
                        let selected = selection.trigger.as_ref().is_some_and(|trigger| {
                            ready.terminal.policy_trigger_id.as_ref() == Some(&trigger.id)
                        });
                        if selected {
                            selected_node = Some(ready.node.clone());
                        }
                        ready_event_evidence.insert(
                            ready.node.clone(),
                            (
                                terminal_json,
                                selected.then(|| trigger_json.clone()).flatten(),
                                None,
                            ),
                        );
                    }
                    if let (Some(trigger_json), Some(selected_node)) =
                        (trigger_json, selected_node)
                    {
                        let Some((_, output, ok, usage, _)) = completed
                            .iter()
                            .find(|(node, ..)| node == &selected_node)
                        else {
                            yield Err(BridgeError::InvalidStateTransition);
                            return;
                        };
                        let Some((terminal_json, _, barrier_slot)) =
                            ready_event_evidence.get_mut(&selected_node)
                        else {
                            yield Err(BridgeError::InvalidStateTransition);
                            return;
                        };
                        let checkpoint = PolicyTriggerCheckpointV1 {
                            node: selected_node,
                            output: output.clone(),
                            ok: *ok,
                            usage: usage.clone(),
                            terminal_json: terminal_json.clone(),
                            policy_trigger_json: trigger_json,
                        };
                        let barrier = reach_policy_trigger_barrier_v1(
                            &authority.run_spec.ledger_admission,
                            policy_trigger_barrier.as_ref(),
                            checkpoint,
                        )
                        .await;
                        *barrier_slot = Some(barrier.clone());
                        action = controller.acknowledge_barrier(barrier, cancel.is_cancelled());
                    }
                    for ready in selection.terminals {
                        node_terminals.insert(ready.node, ready.terminal);
                    }
                    stop_scheduling |= controller.admission_stopped();
                } else {
                    for ready in ready_terminals {
                        node_terminals.insert(ready.node, ready.terminal);
                    }
                }

                match action {
                    PolicyActionV1::CancelRunningSiblings => {
                        for (node, token) in &node_cancels {
                            policy_canceled.insert(node.as_str().to_owned());
                            token.cancel();
                        }
                    }
                    PolicyActionV1::GlobalCancelAndDrain => cancel.cancel(),
                    PolicyActionV1::None | PolicyActionV1::ArmManualGrace { .. } => {}
                }

                for (node_id, text, ok, usage, disposition) in completed {
                    let (terminal_json, policy_trigger_json, policy_trigger_barrier_result) =
                        ready_event_evidence
                            .remove(&node_id)
                            .map(|(terminal, trigger, barrier)| {
                                (Some(terminal), trigger, barrier)
                            })
                            .unwrap_or((None, None, None));
                    yield Ok(WorkflowEvent::NodeFinished {
                        node: node_id.clone(),
                        ok,
                        output: text.clone(),
                        usage: usage.clone(),
                        terminal_json,
                        policy_trigger_json,
                        policy_trigger_barrier_result,
                    });
                    done.insert(node_id.as_str().to_string());
                    dispositions.insert(node_id.as_str().to_string(), disposition);
                    outputs.insert(node_id.as_str().to_string(), (text, ok, usage));
                }
                if cancel.is_cancelled() {
                    // Stop scheduling NEW nodes, but keep draining so every already-in-flight
                    // sibling completes its run_node cancel branch (backend.cancel() +
                    // forget_session()). Do NOT `break` — that drops in-flight futures
                    // mid-cleanup → stranded ACP sessions (dual-review blocker).
                    stop_scheduling = true;
                }
                for node in schedule_ready!() {
                    yield Ok(WorkflowEvent::NodeStarted { node });
                }
            }
            let (term_text, _, _usage) = outputs.get(&terminal_id).cloned().unwrap_or_default();
            let outcome = dispositions
                .get(&terminal_id)
                .copied()
                .map(NodeDisposition::workflow_outcome)
                .unwrap_or_else(|| {
                    // Cancellation stops downstream scheduling. If an in-flight
                    // node reports a concrete failure (including failed cancel
                    // or teardown), that evidence outranks the generic canceled
                    // fallback for the never-started terminal node.
                    if dispositions.values().any(|d| *d == NodeDisposition::Failed) {
                        WorkflowOutcome::Failed
                    } else if cancel.is_cancelled() {
                        WorkflowOutcome::Canceled
                    } else {
                        WorkflowOutcome::Failed
                    }
                });
            let (cleanup_disposition, cleanup_ms) = cleanup_tracker.observation();
            yield Ok(WorkflowEvent::CleanupObserved {
                disposition: cleanup_disposition,
                duration_ms: cleanup_ms,
            });
            yield Ok(WorkflowEvent::Terminal { outcome, output: term_text });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{RetryPolicy, WorkflowGraph, WorkflowNode};
    use bridge_core::diagnostics::{
        DiagnosticEvent, DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor,
        FailureDiagnostic, FailureDiagnosticInput, FailureDisposition, PhaseStatus,
    };
    use bridge_core::domain::{Part, PermissionRequest, RegistrySnapshot, SessionSpec};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, NodeId, SessionId, WorkflowId};
    use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};
    use futures::StreamExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[test]
    fn cleanup_duration_is_the_union_of_overlapping_intervals() {
        let tracker = WorkflowCleanupTracker::default();
        let base = std::time::Instant::now();
        tracker.record(base, base + std::time::Duration::from_millis(30), true);
        tracker.record(
            base + std::time::Duration::from_millis(10),
            base + std::time::Duration::from_millis(40),
            false,
        );
        tracker.record(
            base + std::time::Duration::from_millis(60),
            base + std::time::Duration::from_millis(70),
            true,
        );

        assert_eq!(
            tracker.observation(),
            (WorkflowCleanupDisposition::Failed, 50)
        );
    }

    #[derive(Default)]
    pub(super) struct Rec {
        pub configured: Mutex<bool>,
        pub prompts: Mutex<Vec<String>>,
        pub prompt_parts: Mutex<Vec<Vec<Part>>>,
        pub prompt_sessions: Mutex<Vec<SessionId>>,
        pub cancels: Mutex<u32>,
        pub forgets: Mutex<u32>,
    }
    pub(super) struct FakeBackend {
        pub reply: String,
        pub rec: Arc<Rec>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec
                .prompts
                .lock()
                .unwrap()
                .push(parts.iter().map(|p| p.text.clone()).collect());
            self.rec.prompt_parts.lock().unwrap().push(parts);
            self.rec.prompt_sessions.lock().unwrap().push(_s.clone());
            let updates = vec![
                Ok(Update::Text(self.reply.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            *self.rec.cancels.lock().unwrap() += 1;
            Ok(())
        }
        async fn forget_session(&self, _s: &SessionId) {
            *self.rec.forgets.lock().unwrap() += 1;
        }
        async fn configure_session(
            &self,
            _s: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            *self.rec.configured.lock().unwrap() = true;
            Ok(())
        }
    }
    pub(super) struct NoopLease;
    impl Lease for NoopLease {}
    pub(super) fn minimal_entry(id: &AgentId) -> bridge_core::domain::AgentEntry {
        bridge_core::domain::AgentEntry {
            id: id.clone(),
            cmd: Some("x".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: bridge_core::domain::AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            preflight: false,
            fallback_models: vec![],
            cwd: None,
            session_cwd: None,
            sandbox: None,
            watchdog: None,
            auth_method: None,
            pre_authenticated: false,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        }
    }
    pub(super) struct FakeRegistry {
        pub backends: std::collections::HashMap<String, (String, Arc<Rec>)>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let (reply, rec) =
                self.backends
                    .get(id.as_str())
                    .cloned()
                    .ok_or(BridgeError::UnknownAgent {
                        id: id.as_str().into(),
                    })?;
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(FakeBackend { reply, rec }),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(&self, _event: DiagnosticEvent) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturingDiagnostic {
        events: Mutex<Vec<DiagnosticEvent>>,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for CapturingDiagnostic {
        async fn record(&self, event: DiagnosticEvent) -> Result<(), BridgeError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturingDiagnosticFactory {
        observers: Mutex<Vec<Arc<CapturingDiagnostic>>>,
    }

    impl CapturingDiagnosticFactory {
        fn events(&self) -> Vec<DiagnosticEvent> {
            self.observers
                .lock()
                .unwrap()
                .iter()
                .flat_map(|observer| observer.events.lock().unwrap().clone())
                .collect()
        }
    }

    impl DiagnosticObserverFactory for CapturingDiagnosticFactory {
        fn make(&self, _node: &NodeId, _attempt: u32) -> Arc<dyn DiagnosticObserver> {
            let observer = Arc::new(CapturingDiagnostic::default());
            self.observers.lock().unwrap().push(observer.clone());
            observer
        }
    }

    type RecordedDiagnostic = (String, u32, Arc<dyn DiagnosticObserver>);

    #[derive(Default)]
    struct RecordingDiagnosticFactory {
        made: Mutex<Vec<RecordedDiagnostic>>,
    }

    impl DiagnosticObserverFactory for RecordingDiagnosticFactory {
        fn make(&self, node: &NodeId, attempt: u32) -> Arc<dyn DiagnosticObserver> {
            let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);
            self.made
                .lock()
                .unwrap()
                .push((node.as_str().to_string(), attempt, observer.clone()));
            observer
        }
    }

    #[derive(Default)]
    struct RecordingRichSink {
        events: AtomicUsize,
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for RecordingRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {
            self.events.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct RecordingRichFactory {
        sink: Arc<RecordingRichSink>,
    }

    impl RichEventSinkFactory for RecordingRichFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    #[derive(Default)]
    struct FailingRichSink {
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for FailingRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {}

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::StoreFailure)
        }
    }

    struct FailingRichFactory {
        sink: Arc<FailingRichSink>,
    }

    impl RichEventSinkFactory for FailingRichFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    struct CompositePathBackend {
        prompts: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        cleanups: Mutex<Vec<(&'static str, Arc<dyn DiagnosticObserver>)>>,
        calls: AtomicUsize,
        fail_first: bool,
        final_answer: bool,
        stream_failure: bool,
    }

    impl CompositePathBackend {
        fn new(fail_first: bool) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                cleanups: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                fail_first,
                final_answer: true,
                stream_failure: false,
            }
        }

        fn commentary_only(stream_failure: bool) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                cleanups: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                fail_first: false,
                final_answer: false,
                stream_failure,
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for CompositePathBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("workflow must use prompt_with_observers")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            self.prompts.lock().unwrap().push(observers.diagnostic);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && call == 0 {
                return Err(BridgeError::AgentTimedOut);
            }
            if let Some(sink) = observers.rich {
                sink.record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
            }
            let text = if self.final_answer {
                Update::FinalAnswer("OK".into())
            } else {
                Update::Text("OK".into())
            };
            let terminal = if self.stream_failure {
                Err(BridgeError::FrameError)
            } else {
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                })
            };
            Ok(Box::pin(tokio_stream::iter(vec![Ok(text), terminal])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("forget", observer));
            Ok(())
        }

        async fn release_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("release", observer));
            Ok(())
        }
    }

    struct CompositePathRegistry {
        backend: Arc<CompositePathBackend>,
        resolutions: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for CompositePathRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("workflow must use resolve_observed")
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.resolutions.lock().unwrap().push(observer);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {}

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    struct SingleBackendRegistry {
        backend: Arc<dyn AgentBackend>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for SingleBackendRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    struct CompositePathCleanup;

    #[async_trait::async_trait]
    impl NodeTurnCleanup for CompositePathCleanup {
        async fn on_exit(self: Box<Self>, _exit: NodeTurnExit) {}
    }

    struct CompositePathDispatcher {
        backend: Arc<CompositePathBackend>,
        checkouts: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for CompositePathDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            panic!("warm workflow must use checkout_observed")
        }

        async fn checkout_observed(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<NodeTurn, BridgeError> {
            self.checkouts.lock().unwrap().push(observer);
            Ok(NodeTurn {
                backend: self.backend.clone(),
                session: SessionId::parse("workflow-observed-warm").unwrap(),
                seed: None,
                cleanup: Box::new(CompositePathCleanup),
            })
        }
    }
    pub(super) fn one_node_graph() -> Arc<WorkflowGraph> {
        one_node_graph_with_template("echo {{input}}")
    }

    fn one_node_graph_with_template(prompt_template: &str) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: prompt_template.into(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        })
    }

    #[test]
    fn input_consumption_guard_detects_templates_that_use_input() {
        let graph = one_node_graph_with_template("please review {{input}}");
        assert!(graph_consumes_input(&graph));
        assert_eq!(input_consumption_error(&graph, "brief"), None);
    }

    #[test]
    fn input_consumption_guard_flags_nonempty_dropped_input() {
        let graph = one_node_graph_with_template("static prompt");
        let error = input_consumption_error(&graph, "brief").expect("dropped input error");
        assert!(error.contains("{{input}}"));
        assert!(error.contains("ignored"));
        assert_eq!(input_consumption_error(&graph, ""), None);
    }

    fn retry_graph(retry: Option<RetryPolicy>) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "echo {{input}}".into(),
                inputs: vec![],
                retry,
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        })
    }

    fn retry_policy(max_attempts: u32, backoff_ms: u64) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            backoff_ms,
            backoff_cap_ms: None,
        }
    }

    fn usage(used: u64) -> UsageSnapshot {
        UsageSnapshot {
            used: Some(used),
            size: Some(10_000),
            cost: None,
            terminal: None,
            at_ms: used as i64,
        }
    }

    #[tokio::test]
    async fn cold_direct_workflow_threads_one_observer_and_preserves_one_rich_event() {
        let backend = Arc::new(CompositePathBackend::new(false));
        let registry = Arc::new(CompositePathRegistry {
            backend: backend.clone(),
            resolutions: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("correlation-only").unwrap()),
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                one_node_graph(),
                "input".into(),
                "direct-observed".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let resolutions = registry.resolutions.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].0, "only");
        assert_eq!(made[0].1, 1);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(prompts.len(), 1);
        assert_eq!(cleanups.len(), 1);
        assert_eq!(cleanups[0].0, "forget");
        assert!(Arc::ptr_eq(&made[0].2, &resolutions[0]));
        assert!(Arc::ptr_eq(&made[0].2, &prompts[0]));
        assert!(Arc::ptr_eq(&made[0].2, &cleanups[0].1));
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn r2f0b_cold_workflow_marks_only_genuine_final_answer_as_deliverable() {
        let backend = Arc::new(CompositePathBackend::new(false));
        let registry = Arc::new(CompositePathRegistry {
            backend,
            resolutions: Mutex::new(Vec::new()),
        });
        let telemetry = Arc::new(
            bridge_core::attempt_activity::AttemptTelemetrySinkFactory::new("workflow-delivery"),
        );
        let context = WorkflowRunContext {
            make_rich_sink: Some(telemetry.clone()),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry)
            .run_with_context(
                one_node_graph(),
                "input".into(),
                "delivery-observed".into(),
                CancellationToken::new(),
                context,
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let (capability, _, _, _, _, _, deliverable_final_present) = telemetry
            .evidence()
            .single_turn()
            .expect("one production provider turn");
        assert_eq!(
            capability,
            bridge_core::terminal_evidence::EvidenceCapability::Unsupported
        );
        assert!(deliverable_final_present);
    }

    #[tokio::test]
    async fn r2f0b_cold_workflow_commentary_never_proves_delivery_even_on_failure() {
        for stream_failure in [false, true] {
            let backend = Arc::new(CompositePathBackend::commentary_only(stream_failure));
            let registry = Arc::new(CompositePathRegistry {
                backend,
                resolutions: Mutex::new(Vec::new()),
            });
            let telemetry = Arc::new(
                bridge_core::attempt_activity::AttemptTelemetrySinkFactory::new(
                    "workflow-commentary",
                ),
            );
            let context = WorkflowRunContext {
                make_rich_sink: Some(telemetry.clone()),
                ..WorkflowRunContext::default()
            };

            let _events = WorkflowExecutor::new(registry)
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    format!("commentary-{stream_failure}"),
                    CancellationToken::new(),
                    context,
                )
                .collect::<Vec<_>>()
                .await;
            let (_, _, _, _, _, _, deliverable_final_present) = telemetry
                .evidence()
                .single_turn()
                .expect("one production provider turn");
            assert!(!deliverable_final_present);
        }
    }

    #[tokio::test]
    async fn cold_retry_mints_one_observer_per_attempt_without_duplicating_rich_events() {
        let backend = Arc::new(CompositePathBackend::new(true));
        let registry = Arc::new(CompositePathRegistry {
            backend: backend.clone(),
            resolutions: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                retry_graph(Some(retry_policy(2, 0))),
                "input".into(),
                "retry-observed".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let resolutions = registry.resolutions.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();
        assert_eq!(
            made.iter()
                .map(|(_, attempt, _)| *attempt)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(resolutions.len(), 2);
        assert_eq!(prompts.len(), 2);
        assert_eq!(
            cleanups
                .iter()
                .map(|(action, _)| *action)
                .collect::<Vec<_>>(),
            ["release", "forget"]
        );
        for index in 0..2 {
            assert!(Arc::ptr_eq(&made[index].2, &resolutions[index]));
            assert!(Arc::ptr_eq(&made[index].2, &prompts[index]));
            assert!(Arc::ptr_eq(&made[index].2, &cleanups[index].1));
        }
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn warm_workflow_threads_checkout_observer_and_preserves_one_rich_event() {
        let backend = Arc::new(CompositePathBackend::new(false));
        let dispatcher = Arc::new(CompositePathDispatcher {
            backend: backend.clone(),
            checkouts: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }))
        .run_with_diagnostic_context_and_dispatcher(
            one_node_graph(),
            "input".into(),
            "warm-observed".into(),
            CancellationToken::new(),
            WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            dispatcher.clone(),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let checkouts = dispatcher.checkouts.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(checkouts.len(), 1);
        assert_eq!(prompts.len(), 1);
        assert!(Arc::ptr_eq(&made[0].2, &checkouts[0]));
        assert!(Arc::ptr_eq(&made[0].2, &prompts[0]));
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn existing_task_journal_factory_drives_warm_dispatcher_diagnostics() {
        use bridge_core::diagnostics::{
            DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
            PersistedPhaseTransitionInput, PhaseStatus, TaskJournalDiagnosticObserverFactory,
        };
        use bridge_core::ids::{OperationId, TaskId};
        use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};

        fn event(status: PhaseStatus) -> DiagnosticEvent {
            DiagnosticEvent::new(
                PersistedPhaseTransition::build(
                    PersistedPhaseTransitionInput {
                        phase: DiagnosticPhase::Resolve,
                        status,
                        at_ms: 10,
                        operation: None,
                        code: None,
                        auth: None,
                    },
                    &DiagnosticRedactor::default(),
                )
                .unwrap(),
                None,
            )
            .unwrap()
        }

        struct JournalDispatcher {
            backend: Arc<CompositePathBackend>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for JournalDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                panic!("journal warm workflow must use checkout_observed")
            }

            async fn checkout_observed(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
                observer: Arc<dyn DiagnosticObserver>,
            ) -> Result<NodeTurn, BridgeError> {
                observer.record(event(PhaseStatus::Started)).await?;
                observer.record(event(PhaseStatus::Completed)).await?;
                Ok(NodeTurn {
                    backend: self.backend.clone(),
                    session: SessionId::parse("workflow-journal-warm").unwrap(),
                    seed: None,
                    cleanup: Box::new(CompositePathCleanup),
                })
            }
        }

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("task-journal-warm").unwrap();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "w".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: "input".into(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();
        let factory: Arc<dyn DiagnosticObserverFactory> = Arc::new(
            TaskJournalDiagnosticObserverFactory::new(
                store.clone(),
                task.clone(),
                OperationId::parse("op-task-journal-warm").unwrap(),
            )
            .await
            .unwrap(),
        );
        let context = WorkflowRunContext {
            task_id: Some(task.clone()),
            ..WorkflowRunContext::default()
        };
        let backend = Arc::new(CompositePathBackend::new(false));

        let events = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }))
        .run_with_diagnostic_context_and_dispatcher(
            one_node_graph(),
            "input".into(),
            "journal-warm".into(),
            CancellationToken::new(),
            WorkflowDiagnosticContext::new(context, factory),
            Arc::new(JournalDispatcher { backend }),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(events.iter().all(Result::is_ok));

        let journal = store.journal_from(&task, -1).await.unwrap();
        let diagnostics: Vec<_> = journal
            .iter()
            .filter_map(|event| match &event.kind {
                bridge_core::orch::OrchEventKind::Progress { progress } => {
                    progress.diagnostic_event()
                }
                _ => None,
            })
            .collect();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].transition().status(), PhaseStatus::Started);
        assert_eq!(diagnostics[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn renders_body_as_input_and_task_tokens() {
        let body = "# T\n\n## Description\nBuild it.\n\n## Acceptance Criteria\n- Works\n\n## Files\n- a.rs\n";
        let input = format!("---\ntask-type: implement\n---\n{body}");
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}::{{task.files}}::{{task.spec_refs}}"),
                input,
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last(),
            Some(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            }) if output == "OK"
        ));
        let prompt = &rec.prompts.lock().unwrap()[0];
        assert_eq!(prompt, &format!("{body}::- a.rs\n::"));
    }

    #[tokio::test]
    async fn bare_input_is_freeform_no_task_tokens() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}::{{task.files}}"),
                "plain task".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last(),
            Some(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            }) if output == "OK"
        ));
        assert_eq!(rec.prompts.lock().unwrap()[0], "plain task::{{task.files}}");
    }

    #[tokio::test]
    async fn present_invalid_yields_failed_terminal() {
        let input =
            "---\ntask-type: implement\n---\n# T\n\n## Description\nBuild it.\n".to_string();
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}"),
                input,
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            WorkflowEvent::CleanupObserved {
                disposition: WorkflowCleanupDisposition::NotNeeded,
                duration_ms: 0
            }
        ));
        assert!(matches!(
            &events[1],
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Failed,
                output
            } if output.contains("task-spec schema")
        ));
        assert!(
            rec.prompts.lock().unwrap().is_empty(),
            "present-invalid input must fail before spawning any node"
        );
    }

    #[derive(Clone)]
    enum RetryBehavior {
        SucceedsAfterInvalidates {
            required_invalidates: usize,
        },
        AlwaysTimedOutWithUsage {
            final_generation: usize,
            first_usage: UsageSnapshot,
            final_usage: UsageSnapshot,
        },
        NonTransientPrompt,
        ConfigInvalid,
        UsageThenPending {
            usage: UsageSnapshot,
            usage_notify: Arc<tokio::sync::Notify>,
        },
    }

    #[derive(Default)]
    struct RetryRec {
        resolve_count: AtomicUsize,
        invalidate_count: AtomicUsize,
        configure_count: AtomicUsize,
        prompt_count: AtomicUsize,
        release_count: AtomicUsize,
        forget_count: AtomicUsize,
        prompt_notify: tokio::sync::Notify,
        invalidate_notify: tokio::sync::Notify,
    }

    struct RetryBackend {
        behavior: RetryBehavior,
        generation: usize,
        rec: Arc<RetryRec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for RetryBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec.prompt_count.fetch_add(1, Ordering::SeqCst);
            self.rec.prompt_notify.notify_waiters();
            match &self.behavior {
                RetryBehavior::SucceedsAfterInvalidates {
                    required_invalidates,
                } => {
                    if self.generation < *required_invalidates {
                        Err(BridgeError::AgentOverloaded)
                    } else {
                        Ok(Box::pin(tokio_stream::iter(vec![
                            Ok(Update::Text("OK".into())),
                            Ok(Update::Done {
                                stop_reason: "end_turn".into(),
                                prefix_attestation: Default::default(),
                            }),
                        ])))
                    }
                }
                RetryBehavior::AlwaysTimedOutWithUsage {
                    final_generation,
                    first_usage,
                    final_usage,
                } => {
                    let usage = if self.generation == *final_generation {
                        final_usage.clone()
                    } else {
                        first_usage.clone()
                    };
                    Ok(Box::pin(tokio_stream::iter(vec![
                        Ok(Update::Usage(usage)),
                        Err(BridgeError::AgentTimedOut),
                    ])))
                }
                RetryBehavior::NonTransientPrompt => Err(BridgeError::PermissionDenied),
                RetryBehavior::ConfigInvalid => Ok(Box::pin(tokio_stream::iter(Vec::<
                    Result<Update, BridgeError>,
                >::new(
                )))),
                RetryBehavior::UsageThenPending {
                    usage,
                    usage_notify,
                } => {
                    let usage = usage.clone();
                    let usage_notify = usage_notify.clone();
                    Ok(Box::pin(
                        futures::stream::once(async move {
                            usage_notify.notify_waiters();
                            Ok(Update::Usage(usage))
                        })
                        .chain(futures::stream::pending()),
                    ))
                }
            }
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _s: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.configure_count.fetch_add(1, Ordering::SeqCst);
            if matches!(&self.behavior, RetryBehavior::ConfigInvalid) {
                Err(BridgeError::ConfigInvalid {
                    reason: "invalid test config".into(),
                })
            } else {
                Ok(())
            }
        }

        async fn forget_session(&self, _s: &SessionId) {
            self.rec.forget_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn release_session(&self, _s: &SessionId) {
            self.rec.release_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct RetryRegistry {
        behavior: RetryBehavior,
        rec: Arc<RetryRec>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for RetryRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            self.rec.resolve_count.fetch_add(1, Ordering::SeqCst);
            let generation = self.rec.invalidate_count.load(Ordering::SeqCst);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(RetryBackend {
                    behavior: self.behavior.clone(),
                    generation,
                    rec: self.rec.clone(),
                }),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {
            self.rec.invalidate_count.fetch_add(1, Ordering::SeqCst);
            self.rec.invalidate_notify.notify_waiters();
        }

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    async fn run_retry_case(
        behavior: RetryBehavior,
        retry: Option<RetryPolicy>,
        cancel: CancellationToken,
        rec: Arc<RetryRec>,
    ) -> Vec<WorkflowEvent> {
        let ex = WorkflowExecutor::new(Arc::new(RetryRegistry { behavior, rec }));
        ex.run(retry_graph(retry), "DIFF".into(), "run1".into(), cancel)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect()
    }

    fn only_node_finished(events: &[WorkflowEvent]) -> (&bool, &String, &Option<UsageSnapshot>) {
        match events
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap()
        {
            WorkflowEvent::NodeFinished {
                ok, output, usage, ..
            } => (ok, output, usage),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::SucceedsAfterInvalidates {
                required_invalidates: 2,
            },
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, usage) = only_node_finished(&events);
        assert!(*ok, "node should recover after retry: {output}");
        assert_eq!(output, "OK");
        assert_eq!(usage, &None);
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 3);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 2);
        assert_eq!(rec.release_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_exhausts_then_degrades_with_last_usage() {
        let rec = Arc::new(RetryRec::default());
        let final_usage = usage(777);
        let events = run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: 1,
                first_usage: usage(111),
                final_usage: final_usage.clone(),
            },
            Some(retry_policy(2, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, reported_usage) = only_node_finished(&events);
        assert!(!*ok, "exhausted retry must degrade");
        assert!(
            output.contains("after 2 attempts"),
            "unexpected retry marker: {output}"
        );
        assert_eq!(reported_usage, &Some(final_usage));
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 2);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn non_transient_fails_without_retry() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::NonTransientPrompt,
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.contains("PermissionDenied"),
            "unexpected non-transient marker: {output}"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_retry_policy_is_single_attempt() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: 0,
                first_usage: usage(222),
                final_usage: usage(333),
            },
            None,
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.contains("AgentTimedOut"),
            "single-attempt path should keep today's marker: {output}"
        );
        assert!(
            !output.contains("after 1 attempts"),
            "retry marker must stay disabled when retry is None: {output}"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_mid_backoff_aborts_retry() {
        let rec = Arc::new(RetryRec::default());
        let cancel = CancellationToken::new();
        let run = tokio::spawn(run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: usize::MAX,
                first_usage: usage(444),
                final_usage: usage(555),
            },
            Some(retry_policy(5, 60_000)),
            cancel.clone(),
            rec.clone(),
        ));

        while rec.invalidate_count.load(Ordering::SeqCst) == 0 {
            rec.invalidate_notify.notified().await;
        }
        cancel.cancel();
        let events = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancel must abort retry backoff promptly")
            .unwrap();
        let (ok, output, usage) = only_node_finished(&events);
        assert!(!*ok);
        assert_eq!(output, "[node only canceled]");
        assert_eq!(usage, &None);
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn drain_cancel_preserves_usage_without_retry_policy() {
        let rec = Arc::new(RetryRec::default());
        let cancel = CancellationToken::new();
        let usage_notify = Arc::new(tokio::sync::Notify::new());
        let observed_usage = usage(616);
        let run = tokio::spawn(run_retry_case(
            RetryBehavior::UsageThenPending {
                usage: observed_usage.clone(),
                usage_notify: usage_notify.clone(),
            },
            None,
            cancel.clone(),
            rec.clone(),
        ));

        tokio::time::timeout(std::time::Duration::from_secs(2), usage_notify.notified())
            .await
            .expect("backend should emit usage before hanging");
        cancel.cancel();

        let events = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancel must end the hanging drain promptly")
            .unwrap();
        let (ok, output, reported_usage) = only_node_finished(&events);
        assert!(!*ok);
        assert_eq!(output, "[node only canceled]");
        assert_eq!(reported_usage, &Some(observed_usage));
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dropped_mid_retry_emits_no_checkpoint() {
        let rec = Arc::new(RetryRec::default());
        let ex = WorkflowExecutor::new(Arc::new(RetryRegistry {
            behavior: RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: usize::MAX,
                first_usage: usage(444),
                final_usage: usage(555),
            },
            rec: rec.clone(),
        }));
        let mut stream = ex.run(
            retry_graph(Some(retry_policy(5, 60_000))),
            "DIFF".into(),
            "run1".into(),
            CancellationToken::new(),
        );

        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("executor should emit NodeStarted before retry backoff")
            .expect("stream should yield NodeStarted")
            .expect("NodeStarted should be Ok");
        assert!(
            matches!(first, WorkflowEvent::NodeStarted { .. }),
            "first event should be NodeStarted, got {first:?}"
        );
        let mut seen = vec![first];

        let next = stream.next();
        tokio::pin!(next);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if rec.invalidate_count.load(Ordering::SeqCst) > 0 {
                    break;
                }
                tokio::select! {
                    item = &mut next => {
                        let event = item
                            .expect("stream should remain open before retry backoff")
                            .expect("workflow event should be Ok before retry backoff");
                        seen.push(event);
                    }
                    _ = rec.invalidate_notify.notified() => {}
                }
            }
        })
        .await
        .expect("retry path should invalidate before the long backoff");

        // `next` is a `Pin<&mut Next>`; dropping it is a no-op for Drop but ends the borrow of
        // `stream` (NLL last-use) so `stream` itself can be dropped to simulate the crash.
        #[allow(clippy::drop_non_drop)]
        drop(next);
        drop(stream);

        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, WorkflowEvent::NodeFinished { .. })),
            "dropping the stream mid-backoff must not record NodeFinished"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.prompt_count.load(Ordering::SeqCst),
            1,
            "dropping the stream mid-backoff must not run another prompt"
        );
    }

    #[tokio::test]
    async fn retry_enabled_config_invalid_fails_fast() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::ConfigInvalid,
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.starts_with("[node only failed: configure "),
            "unexpected configure marker: {output}"
        );
        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.prompt_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    struct SequenceBackend {
        replies: Mutex<std::collections::VecDeque<Vec<Result<Update, BridgeError>>>>,
        rec: Arc<Rec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for SequenceBackend {
        async fn prompt(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec
                .prompts
                .lock()
                .unwrap()
                .push(parts.iter().map(|p| p.text.clone()).collect());
            self.rec.prompt_parts.lock().unwrap().push(parts);
            self.rec
                .prompt_sessions
                .lock()
                .unwrap()
                .push(session.clone());
            let updates = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted reply for prompt");
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _s: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            *self.rec.configured.lock().unwrap() = true;
            Ok(())
        }

        async fn forget_session(&self, _s: &SessionId) {
            *self.rec.forgets.lock().unwrap() += 1;
        }
    }

    struct SharedBackendRegistry {
        entry: bridge_core::domain::AgentEntry,
        backend: Arc<dyn AgentBackend>,
        invalidates: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for SharedBackendRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(self.entry.clone()),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {
            self.invalidates.fetch_add(1, Ordering::SeqCst);
        }

        fn entry_snapshot(&self, _id: &AgentId) -> Option<Arc<AgentEntry>> {
            Some(Arc::new(self.entry.clone()))
        }

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    fn done_only() -> Vec<Result<Update, BridgeError>> {
        vec![Ok(Update::Done {
            stop_reason: "end_turn".into(),
            prefix_attestation: Default::default(),
        })]
    }

    fn text_done(text: &str) -> Vec<Result<Update, BridgeError>> {
        vec![
            Ok(Update::Text(text.into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ]
    }

    fn node_finished(events: &[WorkflowEvent]) -> (&bool, &String, &Option<UsageSnapshot>) {
        match events
            .iter()
            .find(|event| matches!(event, WorkflowEvent::NodeFinished { .. }))
            .expect("node finished")
        {
            WorkflowEvent::NodeFinished {
                ok, output, usage, ..
            } => (ok, output, usage),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn empty_final_fails_without_replaying_in_a_fresh_session() {
        let rec = Arc::new(Rec::default());
        let backend = Arc::new(SequenceBackend {
            replies: Mutex::new(std::collections::VecDeque::from(vec![
                done_only(),
                text_done("OK"),
            ])),
            rec: rec.clone(),
        });
        let agent = AgentId::parse("codex").unwrap();
        let registry = Arc::new(SharedBackendRegistry {
            entry: minimal_entry(&agent),
            backend,
            invalidates: AtomicUsize::new(0),
        });

        let events = WorkflowExecutor::new(registry.clone())
            .run(
                one_node_graph(),
                "DIFF".into(),
                "empty-retry".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(!*ok, "accepted empty final must not replay: {output}");
        assert!(output.contains("EmptyFinal"), "{output}");
        let sessions = rec.prompt_sessions.lock().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_final_records_prompt_finish_protocol_diagnostic() {
        let rec = Arc::new(Rec::default());
        let backend = Arc::new(SequenceBackend {
            replies: Mutex::new(std::collections::VecDeque::from(vec![
                done_only(),
                text_done("OK"),
            ])),
            rec,
        });
        let agent = AgentId::parse("codex").unwrap();
        let registry = Arc::new(SharedBackendRegistry {
            entry: minimal_entry(&agent),
            backend,
            invalidates: AtomicUsize::new(0),
        });
        let diagnostic_factory = Arc::new(CapturingDiagnosticFactory::default());

        let events = WorkflowExecutor::new(registry)
            .run_with_diagnostic_context(
                one_node_graph(),
                "DIFF".into(),
                "empty-diagnostic".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(
                    WorkflowRunContext::default(),
                    diagnostic_factory.clone(),
                ),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let diagnostics = diagnostic_factory.events();
        let event = diagnostics
            .iter()
            .find(|event| {
                event.transition().phase() == DiagnosticPhase::PromptFinish
                    && event.transition().status() == PhaseStatus::Failed
            })
            .expect("empty final should emit a PromptFinish failure diagnostic");
        let failure = event.failure().expect("failed event carries diagnostic");
        assert_eq!(failure.failed_phase(), DiagnosticPhase::PromptFinish);
        assert_eq!(failure.code().as_str(), "workflow.empty_final");
        assert_eq!(failure.class(), DiagnosticFailureClass::Protocol);
        assert!(
            failure
                .summary()
                .contains("completed with an empty final agent message"),
            "{}",
            failure.summary()
        );
    }

    #[tokio::test]
    async fn empty_final_is_permanent_even_with_configured_retry() {
        let rec = Arc::new(Rec::default());
        let backend = Arc::new(SequenceBackend {
            replies: Mutex::new(std::collections::VecDeque::from(vec![
                done_only(),
                text_done("WOULD-RECOVER"),
            ])),
            rec: rec.clone(),
        });
        let agent = AgentId::parse("codex").unwrap();
        let registry = Arc::new(SharedBackendRegistry {
            entry: minimal_entry(&agent),
            backend,
            invalidates: AtomicUsize::new(0),
        });

        let events = WorkflowExecutor::new(registry.clone())
            .run(
                retry_graph(Some(retry_policy(2, 0))),
                "DIFF".into(),
                "configured-retry-empty".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(!*ok);
        assert!(output.contains("EmptyFinal"), "{output}");
        assert_eq!(rec.prompt_sessions.lock().unwrap().len(), 1);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dropped_stream_is_agent_crash_failure() {
        let rec = Arc::new(Rec::default());
        let backend = Arc::new(SequenceBackend {
            replies: Mutex::new(std::collections::VecDeque::from(vec![Vec::<
                Result<Update, BridgeError>,
            >::new()])),
            rec,
        });
        let agent = AgentId::parse("codex").unwrap();
        let registry = Arc::new(SharedBackendRegistry {
            entry: minimal_entry(&agent),
            backend,
            invalidates: AtomicUsize::new(0),
        });

        let events = WorkflowExecutor::new(registry)
            .run(
                one_node_graph(),
                "DIFF".into(),
                "dropped-stream".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(!*ok);
        assert!(
            output.contains("backend stream ended before terminal Done"),
            "{output}"
        );
    }

    #[derive(Default)]
    struct PreflightRec {
        prompts: Mutex<Vec<String>>,
        configured_models: Mutex<Vec<Option<String>>>,
        session_models: Mutex<HashMap<String, Option<String>>>,
    }

    struct PreflightConfigureGate {
        paused_first: AtomicBool,
        entered: tokio::sync::Barrier,
        release: tokio::sync::Notify,
    }

    impl PreflightConfigureGate {
        fn new() -> Self {
            Self {
                paused_first: AtomicBool::new(false),
                entered: tokio::sync::Barrier::new(2),
                release: tokio::sync::Notify::new(),
            }
        }
    }

    struct PreflightBackend {
        rec: Arc<PreflightRec>,
        pong_models: Vec<String>,
        configure_gate: Option<Arc<PreflightConfigureGate>>,
    }

    #[derive(Clone, Copy, Debug)]
    enum PreflightFault {
        PromptAccepted,
        PromptRejected,
        StreamError,
        StreamEof,
        TerminalEmpty,
        TerminalUnexpected,
        TerminalCancelled,
    }

    #[derive(Default)]
    struct PreflightFaultState {
        prompts: AtomicUsize,
        cancels: AtomicUsize,
        forgets: AtomicUsize,
        session_models: Mutex<HashMap<String, Option<String>>>,
    }

    struct PreflightFaultBackend {
        fault: PreflightFault,
        state: Arc<PreflightFaultState>,
    }

    fn preflight_prompt_failure(accepted: bool) -> BridgeError {
        BridgeError::agent_failure(
            FailureDiagnostic::build_static_code(
                FailureDiagnosticInput {
                    failed_phase: DiagnosticPhase::PromptStart,
                    last_completed_phase: Some(DiagnosticPhase::ConfigApply),
                    class: DiagnosticFailureClass::Transport,
                    disposition: FailureDisposition::Fatal,
                    code: String::new(),
                    summary: "scripted prompt-open failure".to_string(),
                    causes: Vec::new(),
                    stderr_observed: false,
                    stderr_line_count: 0,
                    stderr_scope: None,
                    stderr_tail: None,
                    stderr_redaction: None,
                    retry_after_ms: None,
                    reset_at_ms: None,
                    prompt_may_have_been_accepted: accepted,
                },
                "test.preflight.prompt_open",
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
        )
    }

    #[async_trait::async_trait]
    impl AgentBackend for PreflightFaultBackend {
        async fn prompt(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.state.prompts.fetch_add(1, Ordering::SeqCst);
            let prompt = parts
                .first()
                .map(|part| part.text.as_str())
                .unwrap_or_default();
            let model = self
                .state
                .session_models
                .lock()
                .unwrap()
                .get(session.as_str())
                .cloned()
                .flatten();
            if prompt != WORKFLOW_PREFLIGHT_PROMPT || model.as_deref() == Some("good") {
                return Ok(Box::pin(tokio_stream::iter(text_done("PONG"))));
            }
            match self.fault {
                PreflightFault::PromptAccepted => Err(preflight_prompt_failure(true)),
                PreflightFault::PromptRejected => Err(preflight_prompt_failure(false)),
                PreflightFault::StreamError => Ok(Box::pin(tokio_stream::iter(vec![Err(
                    BridgeError::agent_crashed("scripted preflight stream failure"),
                )]))),
                PreflightFault::StreamEof => Ok(Box::pin(tokio_stream::empty())),
                PreflightFault::TerminalEmpty => Ok(Box::pin(tokio_stream::iter(done_only()))),
                PreflightFault::TerminalUnexpected => {
                    Ok(Box::pin(tokio_stream::iter(text_done("NOPE"))))
                }
                PreflightFault::TerminalCancelled => {
                    Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                        stop_reason: STOP_REASON_CANCELLED.to_string(),
                        prefix_attestation: Default::default(),
                    })])))
                }
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.state.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn configure_session(
            &self,
            session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.state
                .session_models
                .lock()
                .unwrap()
                .insert(session.as_str().to_string(), spec.config.model.clone());
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.state.forgets.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for PreflightBackend {
        async fn prompt(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let prompt = parts
                .first()
                .map(|part| part.text.clone())
                .unwrap_or_default();
            self.rec.prompts.lock().unwrap().push(prompt.clone());
            let model = self
                .rec
                .session_models
                .lock()
                .unwrap()
                .get(session.as_str())
                .cloned()
                .unwrap_or_default();
            if prompt == WORKFLOW_PREFLIGHT_PROMPT {
                if model.as_deref() == Some("reject") {
                    Err(preflight_prompt_failure(false))
                } else if model
                    .as_deref()
                    .is_some_and(|model| self.pong_models.iter().any(|allowed| allowed == model))
                {
                    Ok(Box::pin(tokio_stream::iter(text_done("PONG"))))
                } else {
                    Ok(Box::pin(tokio_stream::iter(done_only())))
                }
            } else {
                Ok(Box::pin(tokio_stream::iter(text_done("REAL"))))
            }
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            if let Some(gate) = &self.configure_gate {
                if session.as_str().starts_with("workflow-preflight-")
                    && !gate.paused_first.swap(true, Ordering::SeqCst)
                {
                    gate.entered.wait().await;
                    gate.release.notified().await;
                }
            }
            self.rec
                .configured_models
                .lock()
                .unwrap()
                .push(spec.config.model.clone());
            self.rec
                .session_models
                .lock()
                .unwrap()
                .insert(session.as_str().to_string(), spec.config.model.clone());
            Ok(())
        }
    }

    #[test]
    fn workflow_preflight_pong_match_is_byte_exact() {
        assert!(is_exact_preflight_pong("PONG"));
        for response in [" PONG", "PONG ", "\nPONG", "PONG\n", "pong"] {
            assert!(
                !is_exact_preflight_pong(response),
                "preflight accepted non-exact response {response:?}"
            );
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum PreflightCancelPhase {
        Resolve,
        Configure,
        PromptOpen,
    }

    struct PreflightCancelGate {
        phase: PreflightCancelPhase,
        entered: tokio::sync::Barrier,
        release: tokio::sync::Notify,
    }

    impl PreflightCancelGate {
        fn new(phase: PreflightCancelPhase) -> Self {
            Self {
                phase,
                entered: tokio::sync::Barrier::new(2),
                release: tokio::sync::Notify::new(),
            }
        }

        async fn pause(&self, phase: PreflightCancelPhase) {
            if self.phase == phase {
                self.entered.wait().await;
                self.release.notified().await;
            }
        }
    }

    #[derive(Default)]
    struct PreflightCancelState {
        resolves: AtomicUsize,
        configures: AtomicUsize,
        prompts: AtomicUsize,
        cancels: AtomicUsize,
        forgets: AtomicUsize,
    }

    struct CancelPreflightBackend {
        gate: Arc<PreflightCancelGate>,
        state: Arc<PreflightCancelState>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for CancelPreflightBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.state.prompts.fetch_add(1, Ordering::SeqCst);
            self.gate.pause(PreflightCancelPhase::PromptOpen).await;
            Ok(Box::pin(tokio_stream::iter(text_done("PONG"))))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.state.cancels.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.state.configures.fetch_add(1, Ordering::SeqCst);
            self.gate.pause(PreflightCancelPhase::Configure).await;
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.state.forgets.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct CancelPreflightRegistry {
        entry: AgentEntry,
        backend: Arc<dyn AgentBackend>,
        gate: Arc<PreflightCancelGate>,
        state: Arc<PreflightCancelState>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for CancelPreflightRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            let ordinal = self.state.resolves.fetch_add(1, Ordering::SeqCst) + 1;
            if ordinal == 2 {
                self.gate.pause(PreflightCancelPhase::Resolve).await;
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

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn entry_snapshot(&self, _id: &AgentId) -> Option<Arc<AgentEntry>> {
            Some(Arc::new(self.entry.clone()))
        }

        fn list(&self) -> Vec<AgentId> {
            vec![self.entry.id.clone()]
        }
    }

    async fn run_canceled_preflight(phase: PreflightCancelPhase) -> Arc<PreflightCancelState> {
        let gate = Arc::new(PreflightCancelGate::new(phase));
        let state = Arc::new(PreflightCancelState::default());
        let backend: Arc<dyn AgentBackend> = Arc::new(CancelPreflightBackend {
            gate: gate.clone(),
            state: state.clone(),
        });
        let registry = Arc::new(CancelPreflightRegistry {
            entry: preflight_entry("good", &[]),
            backend,
            gate: gate.clone(),
            state: state.clone(),
        });
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            WorkflowExecutor::new(registry)
                .run(
                    one_node_graph(),
                    "DIFF".into(),
                    format!("preflight-cancel-{phase:?}"),
                    task_cancel,
                )
                .collect::<Vec<_>>()
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), gate.entered.wait())
            .await
            .expect("preflight phase should reach the gate");
        cancel.cancel();
        let events = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("canceled preflight should settle")
            .expect("preflight task should join")
            .into_iter()
            .map(|event| event.expect("workflow event"))
            .collect::<Vec<_>>();
        let (ok, output, _) = node_finished(&events);
        assert!(!*ok, "canceled preflight cannot succeed: {output}");
        assert!(output.contains("canceled"), "{output}");
        state
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn preflight_cancel_during_resolve_settles_without_prompt() {
        let state = run_canceled_preflight(PreflightCancelPhase::Resolve).await;
        assert_eq!(state.resolves.load(Ordering::SeqCst), 2);
        assert_eq!(state.configures.load(Ordering::SeqCst), 0);
        assert_eq!(state.prompts.load(Ordering::SeqCst), 0);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn preflight_cancel_during_configure_forgets_without_prompt() {
        let state = run_canceled_preflight(PreflightCancelPhase::Configure).await;
        assert_eq!(state.resolves.load(Ordering::SeqCst), 2);
        assert_eq!(state.configures.load(Ordering::SeqCst), 1);
        assert_eq!(state.prompts.load(Ordering::SeqCst), 0);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn preflight_cancel_during_prompt_open_cancels_and_forgets() {
        let state = run_canceled_preflight(PreflightCancelPhase::PromptOpen).await;
        assert_eq!(state.resolves.load(Ordering::SeqCst), 2);
        assert_eq!(state.configures.load(Ordering::SeqCst), 1);
        assert_eq!(state.prompts.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 1);
    }

    #[derive(Default)]
    struct ProgressRichSink {
        progress: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for ProgressRichSink {
        fn record(&self, kind: bridge_core::orch::OrchEventKind) {
            if let bridge_core::orch::OrchEventKind::Progress { progress } = kind {
                self.progress
                    .lock()
                    .unwrap()
                    .push(progress.text().to_string());
            }
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ProgressRichFactory {
        sink: Arc<ProgressRichSink>,
    }

    impl RichEventSinkFactory for ProgressRichFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    struct PreflightDispatcher {
        backend: Arc<PreflightBackend>,
        checkout_models: Mutex<Vec<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for PreflightDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            panic!("preflight dispatcher test must use observed override checkout")
        }

        async fn checkout_observed_with_overrides(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
            overrides: Option<AgentOverride>,
            _observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<NodeTurn, BridgeError> {
            self.checkout_models
                .lock()
                .unwrap()
                .push(overrides.and_then(|override_config| override_config.model));
            Ok(NodeTurn {
                backend: self.backend.clone(),
                session: SessionId::parse("workflow-preflight-dispatcher").unwrap(),
                seed: None,
                cleanup: Box::new(CompositePathCleanup),
            })
        }
    }

    fn preflight_dispatcher(backend: Arc<PreflightBackend>) -> Arc<PreflightDispatcher> {
        Arc::new(PreflightDispatcher {
            backend,
            checkout_models: Mutex::new(Vec::new()),
        })
    }

    fn preflight_entry(base: &str, fallbacks: &[&str]) -> bridge_core::domain::AgentEntry {
        let agent = AgentId::parse("codex").unwrap();
        let mut entry = minimal_entry(&agent);
        entry.model = Some(base.to_string());
        entry.preflight = true;
        entry.fallback_models = fallbacks.iter().map(|model| model.to_string()).collect();
        entry
    }

    async fn exercise_preflight_fault(
        fault: PreflightFault,
    ) -> (
        Result<PreflightDecision, PreflightFailure>,
        Result<PreflightDecision, PreflightFailure>,
        Arc<PreflightFaultState>,
        Arc<SharedBackendRegistry>,
    ) {
        let state = Arc::new(PreflightFaultState::default());
        let backend: Arc<dyn AgentBackend> = Arc::new(PreflightFaultBackend {
            fault,
            state: state.clone(),
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("bad", &["good"]),
            backend,
            invalidates: AtomicUsize::new(0),
        });
        let executor = WorkflowExecutor::new(registry.clone());
        let graph = one_node_graph();
        let node = graph.nodes[0].clone();
        let diagnostic_factory: Arc<dyn DiagnosticObserverFactory> =
            Arc::new(RecordingDiagnosticFactory::default());
        let cancel = CancellationToken::new();
        let ctx = WorkflowRunContext::default();
        let cache: PreflightCache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let cleanup_tracker = WorkflowCleanupTracker::default();
        let prompt_dispatch = None;

        let first = executor
            .ensure_agent_preflight(
                "w",
                &node,
                "preflight-fault",
                &ctx,
                &diagnostic_factory,
                &prompt_dispatch,
                &cancel,
                Arc::new(registry.entry.clone()),
                &cache,
                &cleanup_tracker,
            )
            .await;
        let second = executor
            .ensure_agent_preflight(
                "w",
                &node,
                "preflight-fault",
                &ctx,
                &diagnostic_factory,
                &prompt_dispatch,
                &cancel,
                Arc::new(registry.entry.clone()),
                &cache,
                &cleanup_tracker,
            )
            .await;
        (first, second, state, registry)
    }

    #[tokio::test]
    async fn preflight_prompt_open_after_possible_acceptance_is_sticky_and_never_falls_back() {
        let (first, second, state, registry) =
            exercise_preflight_fault(PreflightFault::PromptAccepted).await;
        assert!(matches!(first, Err(PreflightFailure::Hard { .. })));
        assert!(matches!(second, Err(PreflightFailure::Hard { .. })));
        assert_eq!(state.prompts.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 1);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_stream_error_is_sticky_and_never_falls_back() {
        let (first, second, state, registry) =
            exercise_preflight_fault(PreflightFault::StreamError).await;
        assert!(matches!(first, Err(PreflightFailure::Hard { .. })));
        assert!(matches!(second, Err(PreflightFailure::Hard { .. })));
        assert_eq!(state.prompts.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 1);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_missing_terminal_is_sticky_and_never_falls_back() {
        let (first, second, state, registry) =
            exercise_preflight_fault(PreflightFault::StreamEof).await;
        assert!(matches!(first, Err(PreflightFailure::Hard { .. })));
        assert!(matches!(second, Err(PreflightFailure::Hard { .. })));
        assert_eq!(state.prompts.load(Ordering::SeqCst), 1);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 1);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn preflight_terminal_failures_are_sticky_and_never_fall_back() {
        for fault in [
            PreflightFault::TerminalEmpty,
            PreflightFault::TerminalUnexpected,
            PreflightFault::TerminalCancelled,
        ] {
            let (first, second, state, registry) = exercise_preflight_fault(fault).await;
            assert!(
                matches!(first, Err(PreflightFailure::Hard { .. })),
                "{fault:?} must fail after the accepted terminal response"
            );
            assert!(
                matches!(second, Err(PreflightFailure::Hard { .. })),
                "{fault:?} must remain sticky within the workflow run"
            );
            assert_eq!(state.prompts.load(Ordering::SeqCst), 1, "{fault:?}");
            assert_eq!(state.cancels.load(Ordering::SeqCst), 0, "{fault:?}");
            assert_eq!(state.forgets.load(Ordering::SeqCst), 1, "{fault:?}");
            assert_eq!(registry.invalidates.load(Ordering::SeqCst), 0, "{fault:?}");
        }
    }

    #[tokio::test]
    async fn proven_pre_acceptance_prompt_rejection_may_use_fallback() {
        let (first, second, state, registry) =
            exercise_preflight_fault(PreflightFault::PromptRejected).await;
        assert_eq!(first.unwrap().selected_model.as_deref(), Some("good"));
        assert_eq!(second.unwrap().selected_model.as_deref(), Some("good"));
        assert_eq!(state.prompts.load(Ordering::SeqCst), 2);
        assert_eq!(state.cancels.load(Ordering::SeqCst), 0);
        assert_eq!(state.forgets.load(Ordering::SeqCst), 2);
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn preflight_proven_rejection_substitutes_fallback_and_records_progress() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["good"]),
            backend,
            invalidates: AtomicUsize::new(0),
        });
        let rich = Arc::new(ProgressRichSink::default());
        let ctx = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(ProgressRichFactory { sink: rich.clone() })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_context(
                one_node_graph(),
                "DIFF".into(),
                "preflight-fallback".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(
            *ok,
            "fallback-selected model should run real turn: {output}"
        );
        assert_eq!(output, "REAL");
        assert_eq!(
            rec.configured_models.lock().unwrap().as_slice(),
            [
                Some("reject".into()),
                Some("good".into()),
                Some("good".into())
            ]
        );
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 1);
        assert!(
            rich.progress
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains("reject -> good")),
            "fallback substitution should be recorded"
        );
    }

    #[tokio::test]
    async fn preflight_ladder_stops_after_fallback_accepts_empty_final() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec![],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["worse"]),
            backend,
            invalidates: AtomicUsize::new(0),
        });

        let events = WorkflowExecutor::new(registry)
            .run(
                one_node_graph(),
                "DIFF".into(),
                "preflight-exhausted".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(!*ok);
        assert!(output.contains("preflight stopped"), "{output}");
        assert!(output.contains("model=reject"), "{output}");
        assert!(output.contains("model=worse"), "{output}");
        assert_eq!(
            rec.prompts.lock().unwrap().as_slice(),
            [WORKFLOW_PREFLIGHT_PROMPT, WORKFLOW_PREFLIGHT_PROMPT]
        );
    }

    #[tokio::test]
    async fn dispatcher_preflight_runs_before_warm_checkout_when_enabled() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("good", &[]),
            backend: backend.clone(),
            invalidates: AtomicUsize::new(0),
        });
        let dispatcher = preflight_dispatcher(backend);
        let telemetry = Arc::new(
            bridge_core::attempt_activity::AttemptTelemetrySinkFactory::new(
                "dispatcher-preflight-evidence",
            ),
        );

        let events = WorkflowExecutor::new(registry)
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "dispatcher-preflight".into(),
                CancellationToken::new(),
                WorkflowRunContext {
                    make_rich_sink: Some(telemetry.clone()),
                    ..WorkflowRunContext::default()
                },
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(
            *ok,
            "warm dispatcher real turn should run after preflight: {output}"
        );
        assert_eq!(output, "REAL");
        assert_eq!(
            rec.prompts.lock().unwrap().clone(),
            vec![
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                "echo DIFF".to_string()
            ]
        );
        assert_eq!(
            dispatcher.checkout_models.lock().unwrap().clone(),
            vec![None]
        );
        telemetry.evidence().close_all();
        assert_eq!(
            telemetry.evidence().counts().reached,
            2,
            "the genuine preflight and real node prompt are both provider turns"
        );
    }

    #[tokio::test]
    async fn dispatcher_preflight_proven_rejection_substitutes_fallback_and_records_progress() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["good"]),
            backend: backend.clone(),
            invalidates: AtomicUsize::new(0),
        });
        let dispatcher = preflight_dispatcher(backend);
        let rich = Arc::new(ProgressRichSink::default());
        let ctx = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(ProgressRichFactory { sink: rich.clone() })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "dispatcher-preflight-fallback".into(),
                CancellationToken::new(),
                ctx,
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(
            *ok,
            "fallback-selected warm model should run real turn: {output}"
        );
        assert_eq!(output, "REAL");
        assert_eq!(
            rec.configured_models.lock().unwrap().as_slice(),
            [Some("reject".into()), Some("good".into())]
        );
        assert_eq!(
            rec.prompts.lock().unwrap().clone(),
            vec![
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                "echo DIFF".to_string(),
            ]
        );
        assert_eq!(registry.invalidates.load(Ordering::SeqCst), 1);
        assert_eq!(
            dispatcher.checkout_models.lock().unwrap().clone(),
            vec![Some("good".to_string())]
        );
        assert!(
            rich.progress
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains("reject -> good")),
            "fallback substitution should be recorded"
        );
    }

    #[tokio::test]
    async fn dispatcher_preflight_stops_after_fallback_accepts_empty_final() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec![],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["worse"]),
            backend: backend.clone(),
            invalidates: AtomicUsize::new(0),
        });
        let dispatcher = preflight_dispatcher(backend);

        let events = WorkflowExecutor::new(registry)
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "dispatcher-preflight-exhausted".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(!*ok);
        assert!(output.contains("preflight stopped"), "{output}");
        assert!(output.contains("model=reject"), "{output}");
        assert!(output.contains("model=worse"), "{output}");
        assert_eq!(
            rec.prompts.lock().unwrap().clone(),
            vec![
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
            ]
        );
        assert_eq!(dispatcher.checkout_models.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn preflight_cache_is_shared_by_inline_and_dispatcher_paths() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: None,
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["good"]),
            backend: backend.clone(),
            invalidates: AtomicUsize::new(0),
        });
        let executor = WorkflowExecutor::new(registry);
        let graph = one_node_graph();
        let node = graph.nodes[0].clone();
        let vars = HashMap::from([("input", "DIFF")]);
        let run_id = "shared-preflight-cache";
        let cancel = CancellationToken::new();
        let ctx = WorkflowRunContext::default();
        let diagnostic_factory: Arc<dyn DiagnosticObserverFactory> =
            Arc::new(RecordingDiagnosticFactory::default());
        let prompt_dispatch = None;
        let cleanup_tracker = WorkflowCleanupTracker::default();
        let preflight_cache: PreflightCache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let inline_output = executor
            .run_node(
                "w",
                &node,
                &vars,
                run_id,
                &cancel,
                &ctx,
                &diagnostic_factory,
                &prompt_dispatch,
                &cleanup_tracker,
                None,
                &preflight_cache,
                None,
            )
            .await
            .unwrap();
        assert!(
            inline_output.ok,
            "inline path should run after fallback preflight: {}",
            inline_output.text
        );
        assert_eq!(inline_output.text, "REAL");

        let dispatcher = preflight_dispatcher(backend);
        let dispatcher_dyn: Arc<dyn WorkflowNodeDispatcher> = dispatcher.clone();
        let dispatcher_output = executor
            .run_node(
                "w",
                &node,
                &vars,
                run_id,
                &cancel,
                &ctx,
                &diagnostic_factory,
                &prompt_dispatch,
                &cleanup_tracker,
                Some(&dispatcher_dyn),
                &preflight_cache,
                None,
            )
            .await
            .unwrap();
        assert!(
            dispatcher_output.ok,
            "dispatcher path should reuse cached preflight: {}",
            dispatcher_output.text
        );
        assert_eq!(dispatcher_output.text, "REAL");

        let prompts = rec.prompts.lock().unwrap().clone();
        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| prompt.as_str() == WORKFLOW_PREFLIGHT_PROMPT)
                .count(),
            2,
            "dispatcher path must not run a second smoke ladder: {prompts:?}"
        );
        assert_eq!(
            prompts,
            vec![
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                WORKFLOW_PREFLIGHT_PROMPT.to_string(),
                "echo DIFF".to_string(),
                "echo DIFF".to_string(),
            ]
        );
        assert_eq!(
            dispatcher.checkout_models.lock().unwrap().clone(),
            vec![Some("good".to_string())],
            "cached fallback decisions must still be forwarded as dispatcher model overrides"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_preflight_first_miss_single_flights_per_agent() {
        let rec = Arc::new(PreflightRec::default());
        let gate = Arc::new(PreflightConfigureGate::new());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: Some(gate.clone()),
        });
        let registry = Arc::new(SharedBackendRegistry {
            entry: preflight_entry("reject", &["good"]),
            backend,
            invalidates: AtomicUsize::new(0),
        });
        let executor = Arc::new(WorkflowExecutor::new(registry.clone()));
        let graph = one_node_graph();
        let node = graph.nodes[0].clone();
        let diagnostic_factory: Arc<dyn DiagnosticObserverFactory> =
            Arc::new(RecordingDiagnosticFactory::default());
        let prompt_dispatch = None;
        let cleanup_tracker = Arc::new(WorkflowCleanupTracker::default());
        let preflight_cache: PreflightCache = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        let first_executor = executor.clone();
        let first_node = node.clone();
        let first_diagnostic_factory = diagnostic_factory.clone();
        let first_cleanup_tracker = cleanup_tracker.clone();
        let first_cache = preflight_cache.clone();
        let first = tokio::spawn(async move {
            let vars = HashMap::from([("input", "DIFF")]);
            let cancel = CancellationToken::new();
            let ctx = WorkflowRunContext::default();
            first_executor
                .run_node(
                    "w",
                    &first_node,
                    &vars,
                    "concurrent-preflight-a",
                    &cancel,
                    &ctx,
                    &first_diagnostic_factory,
                    &None,
                    first_cleanup_tracker.as_ref(),
                    None,
                    &first_cache,
                    None,
                )
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(2), gate.entered.wait())
            .await
            .expect("first preflight configure should reach the concurrency gate");

        let second_vars = HashMap::from([("input", "DIFF")]);
        let second_cancel = CancellationToken::new();
        let second_ctx = WorkflowRunContext::default();
        let second = executor.run_node(
            "w",
            &node,
            &second_vars,
            "concurrent-preflight-b",
            &second_cancel,
            &second_ctx,
            &diagnostic_factory,
            &prompt_dispatch,
            cleanup_tracker.as_ref(),
            None,
            &preflight_cache,
            None,
        );
        tokio::pin!(second);
        if !matches!(futures::poll!(&mut second), std::task::Poll::Pending) {
            gate.release.notify_one();
            panic!("second concurrent first miss should wait on the in-flight preflight cell");
        }
        gate.release.notify_one();

        let first_result = first.await;
        let second_result = second.await;
        let first_output = first_result.unwrap().unwrap();
        let second_output = second_result.unwrap();

        assert!(
            first_output.ok,
            "first real turn should run: {}",
            first_output.text
        );
        assert!(
            second_output.ok,
            "second real turn should run: {}",
            second_output.text
        );
        let prompts = rec.prompts.lock().unwrap().clone();
        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| prompt.as_str() == WORKFLOW_PREFLIGHT_PROMPT)
                .count(),
            2,
            "one rejected+good smoke ladder should be shared by concurrent first miss: {prompts:?}"
        );
        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| prompt.as_str() == "echo DIFF")
                .count(),
            2,
            "both real turns still run after the shared preflight: {prompts:?}"
        );
        assert_eq!(
            registry.invalidates.load(Ordering::SeqCst),
            1,
            "fallback invalidation should happen once for the shared ladder"
        );
    }

    #[tokio::test]
    async fn dispatcher_absent_preflight_config_runs_zero_smoke_turns() {
        let rec = Arc::new(PreflightRec::default());
        let backend = Arc::new(PreflightBackend {
            rec: rec.clone(),
            pong_models: vec!["good".into()],
            configure_gate: None,
        });
        let agent = AgentId::parse("codex").unwrap();
        let registry = Arc::new(SharedBackendRegistry {
            entry: minimal_entry(&agent),
            backend: backend.clone(),
            invalidates: AtomicUsize::new(0),
        });
        let dispatcher = preflight_dispatcher(backend);

        let events = WorkflowExecutor::new(registry)
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "dispatcher-no-preflight".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output, _) = node_finished(&events);
        assert!(
            *ok,
            "warm dispatcher should preserve default-off behavior: {output}"
        );
        let prompts = rec.prompts.lock().unwrap().clone();
        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| prompt.as_str() == WORKFLOW_PREFLIGHT_PROMPT)
                .count(),
            0,
            "absent config must not run smoke turns: {prompts:?}"
        );
        assert_eq!(prompts, vec!["echo DIFF".to_string()]);
        assert_eq!(rec.configured_models.lock().unwrap().len(), 0);
        assert_eq!(
            dispatcher.checkout_models.lock().unwrap().clone(),
            vec![None]
        );
    }

    #[tokio::test]
    async fn captures_node_usage_smoke() {
        struct UsageBackend;
        #[async_trait::async_trait]
        impl AgentBackend for UsageBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text("HI".into())),
                    Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                        used: Some(15071),
                        size: Some(258400),
                        cost: None,
                        terminal: None,
                        at_ms: 1,
                    })),
                    Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                        used: None,
                        size: None,
                        cost: None,
                        terminal: Some(bridge_core::orch::TerminalUsage {
                            total_tokens: 321,
                            input_tokens: 300,
                            output_tokens: 21,
                            thought_tokens: None,
                            cached_read_tokens: None,
                            cached_write_tokens: None,
                        }),
                        at_ms: 1,
                    })),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                        prefix_attestation: Default::default(),
                    }),
                ])))
            }

            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct UReg;
        #[async_trait::async_trait]
        impl AgentRegistry for UReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(UsageBackend),
                    lease: Box::new(NoopLease),
                })
            }

            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }

            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }

            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }

        let ex = WorkflowExecutor::new(Arc::new(UReg));
        let evs: Vec<_> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished { usage: Some(u), .. } => {
                assert_eq!((u.used, u.size), (Some(15071), Some(258400)));
                assert_eq!(
                    u.terminal.as_ref().map(|usage| usage.total_tokens),
                    Some(321)
                );
            }
            other => panic!("expected captured usage, got {other:?}"),
        }
    }

    // A backend that reports Usage and THEN errors mid-stream. Shared by the warm + cold
    // "usage kept on error" regressions (whole-branch review MAJOR-1): the real tokens were
    // consumed, so the usage must survive into NodeFinished even though ok=false.
    struct UsageThenErrBackend {
        used: u64,
    }
    #[async_trait::async_trait]
    impl AgentBackend for UsageThenErrBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    used: Some(self.used),
                    size: Some(100_000),
                    cost: None,
                    terminal: None,
                    at_ms: 1,
                })),
                Err(BridgeError::ConfigInvalid {
                    reason: "boom".into(),
                }),
            ])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn cold_usage_kept_when_node_errors_after_usage() {
        struct UReg;
        #[async_trait::async_trait]
        impl AgentRegistry for UReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(UsageThenErrBackend { used: 4242 }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let ex = WorkflowExecutor::new(Arc::new(UReg));
        let evs: Vec<_> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished {
                ok, usage: Some(u), ..
            } => {
                assert!(!ok, "node errored → ok=false");
                assert_eq!(u.used, Some(4242), "usage kept despite the stream error");
            }
            other => panic!("expected NodeFinished with kept usage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn warm_usage_kept_when_node_errors_after_usage() {
        struct D;
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for D {
            async fn checkout(
                &self,
                _wf: &str,
                _n: &WorkflowNode,
                _r: &str,
                _c: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(UsageThenErrBackend { used: 777 }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: Arc::new(Mutex::new(Vec::new())),
                    }),
                })
            }
        }
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));
        let evs: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                Arc::new(D),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished {
                ok, usage: Some(u), ..
            } => {
                assert!(!ok, "node errored → ok=false");
                assert_eq!(u.used, Some(777), "warm path keeps usage despite the error");
            }
            other => panic!("expected NodeFinished with kept usage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ready_backend_error_beats_simultaneous_workflow_cancellation_after_prompt_ownership() {
        struct ReadyErrorAndCancelBackend {
            cancel: CancellationToken,
        }

        #[async_trait::async_trait]
        impl AgentBackend for ReadyErrorAndCancelBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                // The prompt-open future wins its select while making the
                // cancellation branch ready for the immediately following
                // stream-drain select.
                self.cancel.cancel();
                Ok(Box::pin(tokio_stream::iter(vec![Err(
                    BridgeError::StoreFailure,
                )])))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyErrorDispatcher {
            cancel: CancellationToken,
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyErrorDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyErrorAndCancelBackend {
                        cancel: self.cancel.clone(),
                    }),
                    session: SessionId::parse("ready-error-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let cancel = CancellationToken::new();
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-race".into(),
                cancel.clone(),
                WorkflowRunContext::default(),
                Arc::new(ReadyErrorDispatcher {
                    cancel,
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            exits.lock().unwrap().as_slice(),
            ["error:StoreFailure"],
            "an already-ready backend failure owns the turn before simultaneous cancellation"
        );
        assert_eq!(
            workflow_terminal(&events),
            WorkflowOutcome::Failed,
            "the selected structured failure must also own the public workflow terminal"
        );
    }

    #[tokio::test]
    async fn ready_usage_burst_cannot_starve_workflow_cancellation() {
        struct ReadyUsageAndCancelBackend {
            cancel: CancellationToken,
            updates: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl AgentBackend for ReadyUsageAndCancelBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.cancel.cancel();
                let updates = self.updates.clone();
                let ready = futures::stream::iter((0..128).map(move |_| {
                    updates.fetch_add(1, Ordering::SeqCst);
                    Ok(Update::Usage(UsageSnapshot {
                        used: Some(1),
                        size: Some(10),
                        cost: None,
                        terminal: None,
                        at_ms: 0,
                    }))
                }))
                .chain(futures::stream::pending());
                Ok(Box::pin(ready))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyUsageDispatcher {
            cancel: CancellationToken,
            updates: Arc<AtomicUsize>,
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyUsageDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyUsageAndCancelBackend {
                        cancel: self.cancel.clone(),
                        updates: self.updates.clone(),
                    }),
                    session: SessionId::parse("ready-usage-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let cancel = CancellationToken::new();
        let updates = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let _events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-usage-cancel".into(),
                cancel.clone(),
                WorkflowRunContext::default(),
                Arc::new(ReadyUsageDispatcher {
                    cancel,
                    updates: updates.clone(),
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            updates.load(Ordering::SeqCst),
            1,
            "ready-result precedence may consume one benign item, then must honor cancellation"
        );
        assert_eq!(exits.lock().unwrap().as_slice(), ["canceled"]);
    }

    #[tokio::test]
    async fn ready_prompt_open_error_beats_cancellation_made_ready_after_precheck() {
        struct ReadyPromptErrorBackend;

        #[async_trait::async_trait]
        impl AgentBackend for ReadyPromptErrorBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Err(BridgeError::StoreFailure)
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyPromptErrorDispatcher {
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyPromptErrorDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyPromptErrorBackend),
                    session: SessionId::parse("ready-prompt-error-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        struct CancelAfterPrecheckFactory {
            cancel: CancellationToken,
            sink: Arc<RecordingRichSink>,
        }

        impl RichEventSinkFactory for CancelAfterPrecheckFactory {
            fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
                // `make` runs after run_node's eager cancellation check and
                // immediately before prompt-open ownership is selected. This
                // makes both select branches ready without a scheduler race.
                self.cancel.cancel();
                self.sink.clone()
            }
        }

        let cancel = CancellationToken::new();
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(CancelAfterPrecheckFactory {
                cancel: cancel.clone(),
                sink: Arc::new(RecordingRichSink::default()),
            })),
            ..WorkflowRunContext::default()
        };

        let events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-prompt-race".into(),
                cancel,
                context,
                Arc::new(ReadyPromptErrorDispatcher {
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            exits.lock().unwrap().as_slice(),
            ["error:StoreFailure"],
            "an already-ready prompt-open failure owns the turn before simultaneous cancellation"
        );
        assert_eq!(
            workflow_terminal(&events),
            WorkflowOutcome::Failed,
            "the selected prompt-open failure must also own the public workflow terminal"
        );
    }

    struct ColdCleanupResultBackend {
        backend_error: Option<BridgeError>,
        cleanup_error: Option<BridgeError>,
        legacy_forgets: Arc<AtomicUsize>,
        checked_forgets: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdCleanupResultBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = match &self.backend_error {
                Some(error) => vec![Err(error.clone())],
                None => vec![
                    Ok(Update::Text("OK".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".to_owned(),
                        prefix_attestation: Default::default(),
                    }),
                ],
            };
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.legacy_forgets.fetch_add(1, Ordering::SeqCst);
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.checked_forgets.fetch_add(1, Ordering::SeqCst);
            match &self.cleanup_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn final_review_cold_cleanup_failure_is_primary_only_without_backend_failure() {
        for (backend_error, expected_fragment) in [
            (None, "cleanup failed: StoreFailure"),
            (
                Some(BridgeError::ConfigMismatch { field: "model" }),
                "ConfigMismatch",
            ),
        ] {
            let legacy_forgets = Arc::new(AtomicUsize::new(0));
            let checked_forgets = Arc::new(AtomicUsize::new(0));
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdCleanupResultBackend {
                    backend_error,
                    cleanup_error: Some(BridgeError::StoreFailure),
                    legacy_forgets: legacy_forgets.clone(),
                    checked_forgets: checked_forgets.clone(),
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-cleanup-result".into(),
                    CancellationToken::new(),
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;
            let (ok, output) = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                    _ => None,
                })
                .unwrap();
            let terminal = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::Terminal { outcome, .. } => Some(outcome.clone()),
                    _ => None,
                })
                .unwrap();

            assert!(events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .any(|event| matches!(
                    event,
                    WorkflowEvent::CleanupObserved {
                        disposition: WorkflowCleanupDisposition::Failed,
                        ..
                    }
                )));
            assert!(!ok);
            assert!(output.contains(expected_fragment), "{output}");
            assert_eq!(terminal, WorkflowOutcome::Failed);
            assert_eq!(checked_forgets.load(Ordering::SeqCst), 1);
            assert_eq!(legacy_forgets.load(Ordering::SeqCst), 0);
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ColdTransientSite {
        Configure,
        PromptOpen,
        Stream,
    }

    struct ColdTransientCleanupBackend {
        site: ColdTransientSite,
        configures: AtomicUsize,
        prompts: AtomicUsize,
        cleanups: Mutex<Vec<(&'static str, Arc<dyn DiagnosticObserver>)>>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdTransientCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let call = self.prompts.fetch_add(1, Ordering::SeqCst);
            match (self.site, call) {
                (ColdTransientSite::PromptOpen, 0) => Err(BridgeError::AgentTimedOut),
                (ColdTransientSite::Stream, 0) => Ok(Box::pin(tokio_stream::iter(vec![Err(
                    BridgeError::AgentTimedOut,
                )]))),
                _ => Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text("OK".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".to_owned(),
                        prefix_attestation: Default::default(),
                    }),
                ]))),
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            let call = self.configures.fetch_add(1, Ordering::SeqCst);
            if self.site == ColdTransientSite::Configure && call == 0 {
                Err(BridgeError::AgentTimedOut)
            } else {
                Ok(())
            }
        }

        async fn forget_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("forget", observer));
            Ok(())
        }

        async fn release_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("release", observer));
            Err(BridgeError::StoreFailure)
        }
    }

    struct ColdTransientRetryRegistry {
        backend: Arc<ColdTransientCleanupBackend>,
        resolutions: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for ColdTransientRetryRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("cold retry must use resolve_observed")
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            _observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {}

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    async fn assert_cleanup_failure_vetoes_transient_retry(site: ColdTransientSite) {
        let backend = Arc::new(ColdTransientCleanupBackend {
            site,
            configures: AtomicUsize::new(0),
            prompts: AtomicUsize::new(0),
            cleanups: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(ColdTransientRetryRegistry {
            backend: backend.clone(),
            resolutions: AtomicUsize::new(0),
        });
        let factory = Arc::new(RecordingDiagnosticFactory::default());
        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                retry_graph(Some(retry_policy(2, 0))),
                "input".into(),
                "cold-cleanup-veto".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(WorkflowRunContext::default(), factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        let output = events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { output, .. } => Some(output),
                _ => None,
            })
            .unwrap();
        let made = factory.made.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();

        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
        assert!(output.contains("AgentTimedOut"), "{site:?}: {output}");
        assert!(!output.contains("StoreFailure"), "{site:?}: {output}");
        assert_eq!(registry.resolutions.load(Ordering::SeqCst), 1, "{site:?}");
        assert_eq!(backend.configures.load(Ordering::SeqCst), 1, "{site:?}");
        assert_eq!(
            backend.prompts.load(Ordering::SeqCst),
            usize::from(site != ColdTransientSite::Configure),
            "{site:?}"
        );
        assert_eq!(made.len(), 1, "{site:?}");
        assert_eq!(cleanups.len(), 1, "{site:?}");
        assert_eq!(cleanups[0].0, "release", "{site:?}");
        assert!(Arc::ptr_eq(&made[0].2, &cleanups[0].1), "{site:?}");
    }

    #[tokio::test]
    async fn final_review_configure_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::Configure).await;
    }

    #[tokio::test]
    async fn final_review_prompt_open_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::PromptOpen).await;
    }

    #[tokio::test]
    async fn final_review_stream_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::Stream).await;
    }

    enum ColdReadyRace {
        PromptOpenError,
        StreamError,
        CancellationOnly,
    }

    struct ColdReadyRaceBackend {
        cancel: CancellationToken,
        race: ColdReadyRace,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdReadyRaceBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            match self.race {
                ColdReadyRace::PromptOpenError => Err(BridgeError::StoreFailure),
                ColdReadyRace::StreamError => {
                    self.cancel.cancel();
                    Ok(Box::pin(tokio_stream::iter(vec![Err(
                        BridgeError::StoreFailure,
                    )])))
                }
                ColdReadyRace::CancellationOnly => {
                    self.cancel.cancel();
                    Ok(Box::pin(futures::stream::pending()))
                }
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ColdCancelAfterPrecheckFactory {
        cancel: CancellationToken,
        sink: Arc<RecordingRichSink>,
    }

    impl RichEventSinkFactory for ColdCancelAfterPrecheckFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.cancel.cancel();
            self.sink.clone()
        }
    }

    async fn run_cold_ready_race(
        race: ColdReadyRace,
        cancel_before_prompt_poll: bool,
    ) -> Vec<Result<WorkflowEvent, BridgeError>> {
        let cancel = CancellationToken::new();
        let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
            backend: Arc::new(ColdReadyRaceBackend {
                cancel: cancel.clone(),
                race,
            }),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: cancel_before_prompt_poll.then(|| {
                Arc::new(ColdCancelAfterPrecheckFactory {
                    cancel: cancel.clone(),
                    sink: Arc::new(RecordingRichSink::default()),
                }) as Arc<dyn RichEventSinkFactory>
            }),
            ..WorkflowRunContext::default()
        };
        executor
            .run_with_context(
                one_node_graph(),
                "input".into(),
                "cold-ready-race".into(),
                cancel,
                context,
            )
            .collect::<Vec<_>>()
            .await
    }

    fn workflow_terminal(events: &[Result<WorkflowEvent, BridgeError>]) -> WorkflowOutcome {
        events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { outcome, .. } => Some(outcome.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn final_review_cold_ready_prompt_open_error_beats_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::PromptOpenError, true).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
    }

    #[tokio::test]
    async fn final_review_cold_ready_stream_error_beats_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::StreamError, false).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
    }

    #[tokio::test]
    async fn final_review_cold_pending_stream_still_yields_to_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::CancellationOnly, false).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Canceled);
    }

    struct ColdCancellationCleanupBackend {
        cancel: CancellationToken,
        cancel_error: Option<BridgeError>,
        cleanup_error: Option<BridgeError>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdCancellationCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.cancel.cancel();
            Ok(Box::pin(futures::stream::pending()))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            match &self.cancel_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            match &self.cleanup_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn final_review_cold_cancellation_surfaces_cancel_or_cleanup_failure() {
        for (cancel_error, cleanup_error, expected) in [
            (None, None, WorkflowOutcome::Canceled),
            (
                Some(BridgeError::StoreFailure),
                None,
                WorkflowOutcome::Failed,
            ),
            (
                None,
                Some(BridgeError::StoreFailure),
                WorkflowOutcome::Failed,
            ),
        ] {
            let cancel = CancellationToken::new();
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdCancellationCleanupBackend {
                    cancel: cancel.clone(),
                    cancel_error,
                    cleanup_error,
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-cancel-cleanup".into(),
                    cancel,
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;
            assert_eq!(workflow_terminal(&events), expected);
        }
    }

    struct ColdPendingPromptCleanupBackend {
        checked_forgets: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdPendingPromptCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            std::future::pending().await
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.checked_forgets.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::StoreFailure)
        }
    }

    #[tokio::test]
    async fn final_review_cold_prompt_open_cancellation_surfaces_cleanup_failure() {
        let cancel = CancellationToken::new();
        let checked_forgets = Arc::new(AtomicUsize::new(0));
        let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
            backend: Arc::new(ColdPendingPromptCleanupBackend {
                checked_forgets: checked_forgets.clone(),
            }),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(ColdCancelAfterPrecheckFactory {
                cancel: cancel.clone(),
                sink: Arc::new(RecordingRichSink::default()),
            })),
            ..WorkflowRunContext::default()
        };
        let events = executor
            .run_with_context(
                one_node_graph(),
                "input".into(),
                "cold-prompt-cancel-cleanup".into(),
                cancel,
                context,
            )
            .collect::<Vec<_>>()
            .await;
        let output = events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { output, .. } => Some(output),
                _ => None,
            })
            .unwrap();

        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
        assert!(output.contains("cleanup failed: StoreFailure"), "{output}");
        assert_eq!(checked_forgets.load(Ordering::SeqCst), 1);
    }

    #[derive(Clone, Copy)]
    enum ColdBenignUpdate {
        Text,
        Permission,
        Usage,
    }

    struct ColdReadyBenignBackend {
        cancel: CancellationToken,
        updates: Arc<AtomicUsize>,
        update: ColdBenignUpdate,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdReadyBenignBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.cancel.cancel();
            let updates = self.updates.clone();
            let update = self.update;
            let stream = futures::stream::iter((0..128).map(move |_| {
                updates.fetch_add(1, Ordering::SeqCst);
                Ok(match update {
                    ColdBenignUpdate::Text => Update::Text("ready".to_owned()),
                    ColdBenignUpdate::Permission => {
                        Update::Permission(PermissionRequest::with_id("ready-permission", false))
                    }
                    ColdBenignUpdate::Usage => Update::Usage(UsageSnapshot {
                        used: Some(1),
                        size: Some(10),
                        cost: None,
                        terminal: None,
                        at_ms: 0,
                    }),
                })
            }))
            .chain(futures::stream::pending());
            Ok(Box::pin(stream))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn final_review_cold_ready_benign_item_checks_cancellation_before_repolling() {
        for update in [
            ColdBenignUpdate::Text,
            ColdBenignUpdate::Permission,
            ColdBenignUpdate::Usage,
        ] {
            let cancel = CancellationToken::new();
            let updates = Arc::new(AtomicUsize::new(0));
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdReadyBenignBackend {
                    cancel: cancel.clone(),
                    updates: updates.clone(),
                    update,
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-ready-benign".into(),
                    cancel,
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;

            assert_eq!(updates.load(Ordering::SeqCst), 1);
            assert_eq!(workflow_terminal(&events), WorkflowOutcome::Canceled);
        }
    }

    struct ResultCleanup {
        result: Result<(), BridgeError>,
        exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NodeTurnCleanup for ResultCleanup {
        async fn on_exit(self: Box<Self>, _exit: NodeTurnExit) {
            panic!("observed workflow execution must use result-bearing cleanup")
        }

        async fn on_exit_observed(
            self: Box<Self>,
            exit: NodeTurnExit,
            _observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.exits.lock().unwrap().push(match exit {
                NodeTurnExit::Normal => "normal".to_owned(),
                NodeTurnExit::Canceled => "canceled".to_owned(),
                NodeTurnExit::Error(error) => format!("error:{error:?}"),
            });
            self.result
        }
    }

    struct ImmediateResultBackend {
        error: Option<BridgeError>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ImmediateResultBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = match &self.error {
                Some(error) => vec![Err(error.clone())],
                None => vec![
                    Ok(Update::Text("OK".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".to_owned(),
                        prefix_attestation: Default::default(),
                    }),
                ],
            };
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ResultCleanupDispatcher {
        backend_error: Option<BridgeError>,
        cleanup_error: BridgeError,
        exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for ResultCleanupDispatcher {
        async fn checkout(
            &self,
            _workflow: &str,
            _node: &WorkflowNode,
            _run: &str,
            _context: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(ImmediateResultBackend {
                    error: self.backend_error.clone(),
                }),
                session: SessionId::parse("result-bearing-cleanup").unwrap(),
                seed: None,
                cleanup: Box::new(ResultCleanup {
                    result: Err(self.cleanup_error.clone()),
                    exits: self.exits.clone(),
                }),
            })
        }
    }

    #[tokio::test]
    async fn cleanup_failure_is_primary_only_without_an_earlier_backend_failure() {
        for (backend_error, expected_fragment) in [
            (None, "cleanup failed: StoreFailure"),
            (
                Some(BridgeError::ConfigMismatch { field: "model" }),
                "ConfigMismatch",
            ),
        ] {
            let exits = Arc::new(Mutex::new(Vec::new()));
            let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
                backends: HashMap::new(),
            }));
            let events = executor
                .run_with_context_and_dispatcher(
                    one_node_graph(),
                    "DIFF".into(),
                    "cleanup-result".into(),
                    CancellationToken::new(),
                    WorkflowRunContext::default(),
                    Arc::new(ResultCleanupDispatcher {
                        backend_error,
                        cleanup_error: BridgeError::StoreFailure,
                        exits: exits.clone(),
                    }),
                )
                .collect::<Vec<_>>()
                .await;
            let finished = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                    _ => None,
                })
                .unwrap();
            assert!(!finished.0);
            assert!(events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .any(|event| matches!(
                    event,
                    WorkflowEvent::CleanupObserved {
                        disposition: WorkflowCleanupDisposition::Failed,
                        ..
                    }
                )));
            assert!(finished.1.contains(expected_fragment), "{}", finished.1);
            assert_eq!(exits.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn costs_table_renders_per_field_with_n_a() {
        use bridge_core::orch::{UsageCost, UsageSnapshot};

        let rows = vec![
            (
                "codexer".to_string(),
                Some(UsageSnapshot {
                    used: Some(15071),
                    size: Some(258400),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                }),
            ),
            (
                "clauder".to_string(),
                Some(UsageSnapshot {
                    used: Some(8200),
                    size: Some(200000),
                    cost: Some(UsageCost {
                        amount: 0.03,
                        currency: "USD".into(),
                    }),
                    terminal: None,
                    at_ms: 0,
                }),
            ),
            ("dead".to_string(), None),
        ];

        let table = render_costs_table(&rows);
        assert!(table.contains("| source | used | size | windowFraction | cost |"));
        assert!(table.contains("| codexer | 15071 | 258400 | 0.0583 |"));
        assert!(table.contains("| clauder | 8200 | 200000 | 0.0410 | 0.03 USD |"));
        assert!(table.contains("| dead | n/a | n/a | n/a | n/a |"));
    }

    #[test]
    fn weights_render_sorted() {
        let mut weights = std::collections::BTreeMap::new();
        weights.insert("risk".to_string(), 0.3);
        weights.insert("benefit".to_string(), 0.4);

        let out = render_weights(&Some(crate::graph::PanelConfig { weights }));

        assert_eq!(out, "- benefit: 0.4\n- risk: 0.3\n");
        assert_eq!(render_weights(&None), "(no weights configured)");
    }

    pub(super) struct CountingCleanup {
        pub calls: Arc<AtomicUsize>,
        pub exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NodeTurnCleanup for CountingCleanup {
        async fn on_exit(self: Box<Self>, exit: NodeTurnExit) {
            let label = match exit {
                NodeTurnExit::Normal => "normal".to_string(),
                NodeTurnExit::Canceled => "canceled".to_string(),
                NodeTurnExit::Error(e) => format!("error:{e:?}"),
            };
            self.exits.lock().unwrap().push(label);
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) struct FakeDispatcher {
        pub calls: Arc<AtomicUsize>,
        pub exits: Arc<Mutex<Vec<String>>>,
        pub rec: Arc<Rec>,
        pub session: SessionId,
        pub seed: Option<String>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for FakeDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(NodeTurn {
                backend: Arc::new(FakeBackend {
                    reply: "WARM".into(),
                    rec: self.rec.clone(),
                }),
                session: self.session.clone(),
                seed: self.seed.clone(),
                cleanup: Box::new(CountingCleanup {
                    calls: self.calls.clone(),
                    exits: self.exits.clone(),
                }),
            })
        }
    }

    #[tokio::test]
    async fn node_turn_cleanup_trait_object_runs_on_exit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = FakeDispatcher {
            calls: calls.clone(),
            exits: exits.clone(),
            rec: Arc::new(Rec::default()),
            session: SessionId::parse("workflow-w-only-run1").unwrap(),
            seed: None,
        };
        let graph = one_node_graph();
        let turn = dispatcher
            .checkout("w", &graph.nodes[0], "run1", &WorkflowRunContext::default())
            .await
            .unwrap();

        turn.cleanup.on_exit(NodeTurnExit::Normal).await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn warm_dispatch_no_forget() {
        let rec = Arc::new(Rec::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let session = SessionId::parse("warm-session").unwrap();
        let dispatcher = Arc::new(FakeDispatcher {
            calls: calls.clone(),
            exits: exits.clone(),
            rec: rec.clone(),
            session: session.clone(),
            seed: None,
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            } if output == "WARM"
        ));
        assert_eq!(*rec.forgets.lock().unwrap(), 0, "warm path must not forget");
        assert!(matches!(
            &events[events.len() - 2],
            WorkflowEvent::CleanupObserved {
                disposition: WorkflowCleanupDisposition::Complete,
                ..
            }
        ));
        assert_eq!(rec.prompt_sessions.lock().unwrap().as_slice(), [session]);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn warm_seed_prepended() {
        let rec = Arc::new(Rec::default());
        let dispatcher = Arc::new(FakeDispatcher {
            calls: Arc::new(AtomicUsize::new(0)),
            exits: Arc::new(Mutex::new(Vec::new())),
            rec: rec.clone(),
            session: SessionId::parse("warm-session").unwrap(),
            seed: Some("S".into()),
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let _events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher,
            )
            .collect::<Vec<_>>()
            .await;

        let parts = rec.prompt_parts.lock().unwrap();
        assert_eq!(
            parts[0][0].text,
            "[Summary of earlier context in this session]\nS"
        );
        assert_eq!(parts[0][1].text, "echo DIFF");
    }

    #[tokio::test]
    async fn dispatcher_cancel_drains() {
        use tokio::sync::Notify;

        struct Shared {
            entered: AtomicUsize,
            exits: Mutex<Vec<String>>,
            both_in_flight: Notify,
        }
        struct PendingWarmBackend {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for PendingWarmBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                if self.shared.entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    self.shared.both_in_flight.notify_one();
                }
                Ok(Box::pin(futures::stream::pending()))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                panic!("warm in-drain cancel is owned by cleanup")
            }
        }
        struct ExitCleanup {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl NodeTurnCleanup for ExitCleanup {
            async fn on_exit(self: Box<Self>, exit: NodeTurnExit) {
                let label = match exit {
                    NodeTurnExit::Normal => "normal",
                    NodeTurnExit::Canceled => "canceled",
                    NodeTurnExit::Error(_) => "error",
                };
                self.shared.exits.lock().unwrap().push(label.to_string());
            }
        }
        struct WarmPendingDispatcher {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for WarmPendingDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(PendingWarmBackend {
                        shared: self.shared.clone(),
                    }),
                    session: SessionId::parse(format!("warm-{}", node.id.as_str())).unwrap(),
                    seed: None,
                    cleanup: Box::new(ExitCleanup {
                        shared: self.shared.clone(),
                    }),
                })
            }
        }

        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let shared = Arc::new(Shared {
            entered: AtomicUsize::new(0),
            exits: Mutex::new(Vec::new()),
            both_in_flight: Notify::new(),
        });
        let token = CancellationToken::new();
        let t2 = token.clone();
        let s2 = shared.clone();
        tokio::spawn(async move {
            if s2.entered.load(Ordering::SeqCst) < 2 {
                s2.both_in_flight.notified().await;
            }
            t2.cancel();
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run_with_context_and_dispatcher(
                graph,
                "x".into(),
                "r".into(),
                token,
                WorkflowRunContext::default(),
                Arc::new(WarmPendingDispatcher {
                    shared: shared.clone(),
                }),
            )
            .collect::<Vec<_>>(),
        )
        .await
        .expect("warm cancel must drain in-flight nodes")
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert_eq!(
            shared.exits.lock().unwrap().as_slice(),
            ["canceled", "canceled"]
        );
    }

    #[tokio::test]
    async fn warm_done_cancelled_finishes_not_cancels() {
        struct DoneCancelledBackend {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for DoneCancelledBackend {
            async fn prompt(
                &self,
                s: &SessionId,
                parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.rec.prompt_sessions.lock().unwrap().push(s.clone());
                self.rec.prompt_parts.lock().unwrap().push(parts);
                Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                    stop_reason: STOP_REASON_CANCELLED.into(),
                    prefix_attestation: Default::default(),
                })])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                *self.rec.cancels.lock().unwrap() += 1;
                Ok(())
            }
        }
        struct DoneCancelledDispatcher {
            rec: Arc<Rec>,
            exits: Arc<Mutex<Vec<String>>>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for DoneCancelledDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(DoneCancelledBackend {
                        rec: self.rec.clone(),
                    }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: self.calls.clone(),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let rec = Arc::new(Rec::default());
        let exits = Arc::new(Mutex::new(Vec::new()));
        let rich_sink = Arc::new(FailingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(FailingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));
        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                context,
                Arc::new(DoneCancelledDispatcher {
                    rec: rec.clone(),
                    exits: exits.clone(),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events
                .iter()
                .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
                .unwrap(),
            WorkflowEvent::NodeFinished { ok: false, .. }
        ));
        assert_eq!(*rec.cancels.lock().unwrap(), 0);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_cancel_after_checkout_finishes_no_prompt_no_cancel() {
        struct CancelAfterCheckoutDispatcher {
            token: CancellationToken,
            rec: Arc<Rec>,
            exits: Arc<Mutex<Vec<String>>>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for CancelAfterCheckoutDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                self.token.cancel();
                Ok(NodeTurn {
                    backend: Arc::new(FakeBackend {
                        reply: "UNUSED".into(),
                        rec: self.rec.clone(),
                    }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: self.calls.clone(),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let rec = Arc::new(Rec::default());
        let exits = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                token.clone(),
                WorkflowRunContext::default(),
                Arc::new(CancelAfterCheckoutDispatcher {
                    token,
                    rec: rec.clone(),
                    exits: exits.clone(),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert!(rec.prompt_parts.lock().unwrap().is_empty(), "no prompt");
        assert_eq!(*rec.cancels.lock().unwrap(), 0);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn single_node_configures_renders_concatenates() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);
        let mut events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let term = events.pop().unwrap();
        assert!(
            matches!(term, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "HELLO")
        );
        assert!(*rec.configured.lock().unwrap(), "configure_session called");
        assert_eq!(
            rec.prompts.lock().unwrap()[0],
            "echo DIFF",
            "template rendered with {{input}}"
        );
    }

    #[tokio::test]
    async fn cold_configure_error_fails_node_without_prompting() {
        struct CfgErrBackend {
            rec: Arc<Rec>,
            checked_forgets: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl AgentBackend for CfgErrBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.rec
                    .prompts
                    .lock()
                    .unwrap()
                    .push(parts.iter().map(|p| p.text.clone()).collect());
                Ok(Box::pin(tokio_stream::iter(Vec::<
                    Result<Update, BridgeError>,
                >::new())))
            }

            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }

            async fn configure_session(
                &self,
                _s: &SessionId,
                _spec: &SessionSpec,
            ) -> Result<(), BridgeError> {
                Err(BridgeError::ConfigInvalid {
                    reason: "worktree add failed".into(),
                })
            }

            async fn forget_session(&self, _s: &SessionId) {
                *self.rec.forgets.lock().unwrap() += 1;
            }

            async fn forget_session_checked(&self, _s: &SessionId) -> Result<(), BridgeError> {
                self.checked_forgets.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        struct CfgErrReg {
            rec: Arc<Rec>,
            checked_forgets: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl AgentRegistry for CfgErrReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(CfgErrBackend {
                        rec: self.rec.clone(),
                        checked_forgets: self.checked_forgets.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }

            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }

            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }

            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }

        let rec = Arc::new(Rec::default());
        let checked_forgets = Arc::new(AtomicUsize::new(0));
        let ex = WorkflowExecutor::new(Arc::new(CfgErrReg {
            rec: rec.clone(),
            checked_forgets: checked_forgets.clone(),
        }));
        let events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = events
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished { ok, output, .. } => {
                assert!(!ok, "configure error must fail the node");
                assert!(
                    output.starts_with("[node only failed: configure "),
                    "unexpected node output: {output}"
                );
            }
            other => panic!("expected NodeFinished, got {other:?}"),
        }
        assert!(
            rec.prompts.lock().unwrap().is_empty(),
            "prompt must not run after configure_session fails"
        );
        assert_eq!(
            *rec.forgets.lock().unwrap(),
            0,
            "configure_session error must not fall back to result-discarding legacy cleanup"
        );
        assert_eq!(
            checked_forgets.load(Ordering::SeqCst),
            1,
            "configure_session error must use result-bearing cleanup"
        );
    }

    #[tokio::test]
    async fn cold_path_unchanged() {
        // The `None` (cold) branch must be byte-identical to pre-Slice-5 behavior: the cold session id
        // `workflow-{wf}-{node}-{run_id}` AND `forget_session` at the end (NOT the warm dispatcher path).
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);
        let _events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rec.prompt_sessions.lock().unwrap().as_slice(),
            [SessionId::parse("workflow-w-only-run1").unwrap()],
            "cold path uses the cold workflow-wf-node-runid session id"
        );
        assert_eq!(
            *rec.forgets.lock().unwrap(),
            1,
            "cold path forgets the session (no warm keep-alive)"
        );
    }

    fn review_graph() -> Arc<WorkflowGraph> {
        let n = |id: &str, ag: &str, ins: &[&str], tpl: &str| WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(ag).unwrap(),
            prompt_template: tpl.into(),
            inputs: ins.iter().map(|i| NodeId::parse(*i).unwrap()).collect(),
            retry: None,
            harvest_sanitization: None,
        };
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("code-review").unwrap(),
            nodes: vec![
                n("codex", "codex", &[], "review {{input}}"),
                n("claude", "claude", &[], "review {{input}}"),
                n(
                    "synth",
                    "synth",
                    &["codex", "claude"],
                    "merge {{codex}} + {{claude}} for {{input}}\n{{workflow.costs}}",
                ),
            ],
            panel: None,
            controls: None,
        })
    }

    #[tokio::test]
    async fn single_input_node_renders_draft_alias() {
        // A refine node with exactly one input can reference its predecessor's output as {{draft}}
        // (so one shared refine prompt serves legs whose draft nodes have distinct ids).
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("refine").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("draftnode").unwrap(),
                    agent: AgentId::parse("codex").unwrap(),
                    prompt_template: "draft {{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("refinenode").unwrap(),
                    agent: AgentId::parse("claude").unwrap(),
                    prompt_template: "refine against {{draft}} for {{input}}".into(),
                    inputs: vec![NodeId::parse("draftnode").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("DRAFT_OUT")),
                ("claude".to_string(), mk("REFINED")),
            ]
            .into(),
        });
        let refine_rec = reg.backends.get("claude").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let _ = ex
            .run(graph, "DIFF".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;
        let p = &refine_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("DRAFT_OUT") && p.contains("DIFF"),
            "refine node must see the draft via {{draft}} AND the original via {{input}}: {p}"
        );
    }

    #[tokio::test]
    async fn fan_in_synth_receives_both_reviews_and_input() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("CODEX_REVIEW")),
                ("claude".to_string(), mk("CLAUDE_REVIEW")),
                ("synth".to_string(), mk("FINAL")),
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await;
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "FINAL")
        );
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("CODEX_REVIEW") && p.contains("CLAUDE_REVIEW") && p.contains("DIFF"),
            "synth got both reviews + {{input}}: {p}"
        );
    }

    #[tokio::test]
    async fn enabled_harvest_requires_retaining_audit_store_before_node_start() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [(
                "codex".to_string(),
                ("SHOULD_NOT_RUN".to_string(), rec.clone()),
            )]
            .into(),
        });
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: Some(HarvestSanitizationMode::AttestedPrefixV1),
            }],
            panel: None,
            controls: None,
        });
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run(graph, "DIFF".into(), "r".into(), CancellationToken::new())
            .collect()
            .await;

        assert!(matches!(
            &evs[0],
            Err(BridgeError::ConfigInvalid { reason })
                if reason.contains("no retaining harvest audit store")
        ));
        assert!(rec.prompts.lock().unwrap().is_empty());
    }

    struct FailingHarvestAuditStore;

    #[async_trait::async_trait]
    impl bridge_core::harvest::HarvestAuditStore for FailingHarvestAuditStore {
        async fn commit_bundle(
            &self,
            _raw: bridge_core::harvest::HarvestRawRecordV1,
            _decision: bridge_core::harvest::HarvestSanitizationDecisionV1,
        ) -> Result<
            bridge_core::harvest::HarvestAuditCommit,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Err(bridge_core::harvest::HarvestAuditStoreError::Persistence(
                Box::new(std::io::Error::other("intentional harvest audit failure")),
            ))
        }

        async fn get_by_audit_id(
            &self,
            _audit_id: &str,
        ) -> Result<
            Option<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(None)
        }

        async fn get_by_attempt_key(
            &self,
            _run_id: &str,
            _node_id: &str,
            _attempt_id: u32,
            _turn_id: &str,
        ) -> Result<
            Option<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(None)
        }

        async fn list_by_task_id(
            &self,
            _task_id: &str,
            _after_audit_id: Option<&str>,
            _limit: u32,
        ) -> Result<
            Vec<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn harvest_audit_failure_releases_no_node_finished_or_fanin() {
        let root_rec = Arc::new(Rec::default());
        let synth_rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [
                (
                    "codex".to_string(),
                    ("RAW_BODY".to_string(), root_rec.clone()),
                ),
                (
                    "synth".to_string(),
                    ("SHOULD_NOT_RUN".to_string(), synth_rec.clone()),
                ),
            ]
            .into(),
        });
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("root").unwrap(),
                    agent: AgentId::parse("codex").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: Some(HarvestSanitizationMode::AttestedPrefixV1),
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{root}}".into(),
                    inputs: vec![NodeId::parse("root").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let ctx = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("task-harvest-fail").unwrap()),
            harvest_audit_store: Arc::new(FailingHarvestAuditStore),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_with_context(
                graph,
                "DIFF".into(),
                "run-harvest-fail".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect()
            .await;

        assert!(evs
            .iter()
            .any(|event| matches!(event, Err(BridgeError::HarvestAuditPersistFailed { .. }))));
        assert!(
            evs.iter()
                .all(|event| !matches!(event, Ok(WorkflowEvent::NodeFinished { .. }))),
            "audit failure must not release NodeFinished: {evs:?}"
        );
        assert!(
            synth_rec.prompts.lock().unwrap().is_empty(),
            "audit failure must not release root output into fan-in"
        );
        assert_eq!(
            root_rec.prompts.lock().unwrap().clone(),
            vec!["DIFF".to_string()]
        );
    }

    #[derive(Default)]
    struct CountingNonRetainingHarvestStore {
        commits: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::harvest::HarvestAuditStore for CountingNonRetainingHarvestStore {
        fn retains_audit_records(&self) -> bool {
            false
        }

        async fn commit_bundle(
            &self,
            _raw: bridge_core::harvest::HarvestRawRecordV1,
            _decision: bridge_core::harvest::HarvestSanitizationDecisionV1,
        ) -> Result<
            bridge_core::harvest::HarvestAuditCommit,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            self.commits.fetch_add(1, Ordering::SeqCst);
            Ok(bridge_core::harvest::HarvestAuditCommit::Inserted)
        }

        async fn get_by_audit_id(
            &self,
            _audit_id: &str,
        ) -> Result<
            Option<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(None)
        }

        async fn get_by_attempt_key(
            &self,
            _run_id: &str,
            _node_id: &str,
            _attempt_id: u32,
            _turn_id: &str,
        ) -> Result<
            Option<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(None)
        }

        async fn list_by_task_id(
            &self,
            _task_id: &str,
            _after_audit_id: Option<&str>,
            _limit: u32,
        ) -> Result<
            Vec<bridge_core::harvest::HarvestAuditBundleV1>,
            bridge_core::harvest::HarvestAuditStoreError,
        > {
            Ok(Vec::new())
        }
    }

    fn retaining_memory_audit_store() -> Arc<dyn bridge_core::harvest::HarvestAuditStore> {
        let task_store: Arc<dyn bridge_core::task_store::TaskStore> =
            Arc::new(bridge_core::task_store::MemoryTaskStore::default());
        Arc::new(bridge_core::task_store::TaskStoreHarvestAuditStore::new(
            task_store,
        ))
    }

    fn two_node_graph(
        root_mode: Option<HarvestSanitizationMode>,
        synth_mode: Option<HarvestSanitizationMode>,
    ) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("root").unwrap(),
                    agent: AgentId::parse("codex").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: root_mode,
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{root}}".into(),
                    inputs: vec![NodeId::parse("root").unwrap()],
                    retry: None,
                    harvest_sanitization: synth_mode,
                },
            ],
            panel: None,
            controls: None,
        })
    }

    fn two_node_registry() -> Arc<FakeRegistry> {
        Arc::new(FakeRegistry {
            backends: [
                (
                    "codex".to_string(),
                    ("ROOT_OUT".to_string(), Arc::new(Rec::default())),
                ),
                (
                    "synth".to_string(),
                    ("SYNTH_OUT".to_string(), Arc::new(Rec::default())),
                ),
            ]
            .into(),
        })
    }

    /// §18-7 Off-mode audit exemption (MAJOR 3): a workflow with zero enabled
    /// nodes commits nothing to the audit store — no KeptOff rows minted and
    /// discarded — and succeeds both with a retaining store (which must stay
    /// empty) and with the noop store.
    #[tokio::test]
    async fn all_off_workflow_is_audit_exempt_and_emits_no_rows() {
        let audit_store = retaining_memory_audit_store();
        let ctx = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("task-alloff").unwrap()),
            harvest_audit_store: audit_store.clone(),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(two_node_registry());
        let evs: Vec<_> = ex
            .run_with_context(
                two_node_graph(None, Some(HarvestSanitizationMode::Off)),
                "DIFF".into(),
                "run-alloff".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect()
            .await;
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "SYNTH_OUT"),
            "all-Off workflow must complete, got {last:?}"
        );
        let rows = bridge_core::harvest::HarvestAuditStore::list_by_task_id(
            audit_store.as_ref(),
            "task-alloff",
            None,
            10,
        )
        .await
        .unwrap();
        assert!(
            rows.is_empty(),
            "all-Off workflow must produce no durable audit rows, got {}",
            rows.len()
        );

        // The same all-Off workflow also succeeds on a non-retaining store:
        // no commit is attempted, so nothing is silently discarded. This counted
        // variant discriminates the production Noop path from a pre-fix
        // mint-and-discard KeptOff bundle.
        let counted_store = Arc::new(CountingNonRetainingHarvestStore::default());
        let ex = WorkflowExecutor::new(two_node_registry());
        let evs: Vec<_> = ex
            .run_with_context(
                two_node_graph(None, None),
                "DIFF".into(),
                "run-alloff-noop".into(),
                CancellationToken::new(),
                WorkflowRunContext {
                    harvest_audit_store: counted_store.clone(),
                    ..WorkflowRunContext::default()
                },
            )
            .collect()
            .await;
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
        assert_eq!(
            counted_store.commits.load(Ordering::SeqCst),
            0,
            "all-Off workflow must not commit KeptOff bundles to a non-retaining store"
        );
    }

    /// §18-4/§18-7 (MAJOR 3) + MAJOR 4 audit distinction: when at least one
    /// runnable node enables the feature, BOTH the enabled and the Off node's
    /// completions commit durable bundles — the Off node as `kept_off`, the
    /// enabled-but-incapable node as `kept_no_attestation` with the distinct
    /// `backend_declared_incapable` reason (never `sanitization_not_requested`
    /// and never silently mistaken for Off).
    ///
    /// A non-retaining-store variant cannot exercise this mixed graph: the enabled
    /// runnable node makes audit durability required for the whole invocation, so
    /// the executor fails before either node runs. The all-Off counted-store test
    /// above covers the only non-retaining path that may legitimately execute and
    /// proves Off completions do not mint discarded KeptOff bundles there.
    #[tokio::test]
    async fn mixed_workflow_audits_both_enabled_and_off_completions() {
        let audit_store = retaining_memory_audit_store();
        let ctx = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("task-mixed").unwrap()),
            harvest_audit_store: audit_store.clone(),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(two_node_registry());
        let evs: Vec<_> = ex
            .run_with_context(
                two_node_graph(Some(HarvestSanitizationMode::AttestedPrefixV1), None),
                "DIFF".into(),
                "run-mixed".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect()
            .await;
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));

        let rows = bridge_core::harvest::HarvestAuditStore::list_by_task_id(
            audit_store.as_ref(),
            "task-mixed",
            None,
            10,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2, "both completions must be audited");
        let by_node = |node: &str| {
            rows.iter()
                .find(|bundle| bundle.raw.node_id == node)
                .unwrap_or_else(|| panic!("bundle for {node}"))
        };
        let enabled = by_node("root");
        assert_eq!(
            enabled.decision.mode,
            HarvestSanitizationMode::AttestedPrefixV1
        );
        assert_eq!(
            enabled.decision.decision,
            bridge_core::harvest::HarvestDecision::KeptNoAttestation
        );
        assert_eq!(
            enabled.decision.reason.as_deref(),
            Some("backend_declared_incapable"),
            "enabled-but-incapable must carry its distinct reason code"
        );
        let off = by_node("synth");
        assert_eq!(off.decision.mode, HarvestSanitizationMode::Off);
        assert_eq!(
            off.decision.decision,
            bridge_core::harvest::HarvestDecision::KeptOff
        );
        assert_eq!(off.decision.reason, None);
    }

    /// MINOR 7 (verified per the MAJOR 3 adjudication): a fully-seeded resume
    /// runs zero nodes, so even a graph with enabled nodes needs no audit
    /// durability — the noop-store guard must not fire and the resume must
    /// complete from the seed.
    #[tokio::test]
    async fn fully_seeded_resume_with_enabled_node_passes_noop_store_guard() {
        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = [
            ("root".to_string(), ("SEEDED_ROOT".to_string(), true, None)),
            (
                "synth".to_string(),
                ("SEEDED_SYNTH".to_string(), true, None),
            ),
        ]
        .into();
        let reg = two_node_registry();
        let root_rec = reg.backends.get("codex").unwrap().1.clone();
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from_with_context(
                two_node_graph(Some(HarvestSanitizationMode::AttestedPrefixV1), None),
                "DIFF".into(),
                "resume-full".into(),
                CancellationToken::new(),
                seed,
                WorkflowRunContext::default(),
            )
            .collect()
            .await;
        assert!(
            !evs.iter().any(|event| matches!(
                event,
                Err(BridgeError::ConfigInvalid { reason })
                    if reason.contains("no retaining harvest audit store")
            )),
            "fully-seeded resume must not trip the noop-store guard: {evs:?}"
        );
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "SEEDED_SYNTH"),
            "resume must complete from the seed, got {last:?}"
        );
        assert!(root_rec.prompts.lock().unwrap().is_empty());
        assert!(synth_rec.prompts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn enabled_harvest_cuts_two_fan_in_inputs_before_synth_render() {
        fn fixture_sha256(body: &str) -> [u8; 32] {
            match body {
                "process-a\nalpha" => [
                    92, 114, 212, 225, 97, 190, 129, 251, 240, 113, 14, 174, 183, 110, 27, 21, 249,
                    101, 252, 239, 17, 71, 115, 27, 47, 24, 163, 41, 226, 183, 144, 175,
                ],
                "process-b\nbeta" => [
                    240, 166, 40, 223, 12, 152, 188, 43, 168, 170, 32, 54, 148, 141, 7, 139, 161,
                    32, 99, 48, 12, 190, 101, 74, 8, 154, 227, 100, 66, 158, 235, 190,
                ],
                other => panic!("unexpected fixture body for sha256: {other}"),
            }
        }

        fn attested(
            producer_id: &str,
            turn_id: &str,
            body: &str,
            prefix_len: usize,
        ) -> bridge_core::attestation::PrefixAttestationStatus {
            bridge_core::attestation::PrefixAttestationStatus::AttestedV1(
                bridge_core::attestation::AttestedPrefixV1 {
                    issuer_id: bridge_core::attestation::ATTESTED_PREFIX_ISSUER_V1.to_string(),
                    producer_id: producer_id.to_string(),
                    turn_id: turn_id.to_string(),
                    body_len_bytes: body.len() as u64,
                    body_sha256: fixture_sha256(body),
                    process_prefix_bytes: prefix_len as u64,
                },
            )
        }

        #[derive(Default)]
        struct AttestedFanInBackend {
            turns: Mutex<std::collections::HashMap<String, bridge_core::permission::TurnMeta>>,
            synth_prompts: Mutex<Vec<String>>,
        }

        #[async_trait::async_trait]
        impl AgentBackend for AttestedFanInBackend {
            async fn prompt(
                &self,
                session: &SessionId,
                parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                let prompt: String = parts.iter().map(|part| part.text.as_str()).collect();
                let meta = self
                    .turns
                    .lock()
                    .unwrap()
                    .remove(session.as_str())
                    .ok_or(BridgeError::StoreFailure)?;
                let (producer_id, body, prefix_len) = if prompt.contains("root-a") {
                    ("a", "process-a\nalpha", "process-a\n".len())
                } else if prompt.contains("root-b") {
                    ("b", "process-b\nbeta", "process-b\n".len())
                } else {
                    self.synth_prompts.lock().unwrap().push(prompt);
                    ("synth", "FINAL", 0)
                };
                let prefix_attestation = if producer_id == "synth" {
                    Default::default()
                } else {
                    attested(producer_id, meta.turn_id.as_str(), body, prefix_len)
                };
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(body.to_string())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                        prefix_attestation,
                    }),
                ])))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }

            fn prefix_attestation_capability(
                &self,
            ) -> bridge_core::attestation::PrefixAttestationCapability {
                bridge_core::attestation::PrefixAttestationCapability::codex_commit_marker_v1()
            }

            async fn configure_turn(
                &self,
                session: &SessionId,
                meta: bridge_core::permission::TurnMeta,
            ) {
                self.turns
                    .lock()
                    .unwrap()
                    .insert(session.as_str().to_string(), meta);
            }
        }

        let backend = Arc::new(AttestedFanInBackend::default());
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "root-a {{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: Some(HarvestSanitizationMode::AttestedPrefixV1),
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "root-b {{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: Some(HarvestSanitizationMode::AttestedPrefixV1),
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{a}}|{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let audit_store = Arc::new(bridge_core::task_store::MemoryTaskStore::with_clock(
            Arc::new(|| 100),
        ));
        let ctx = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("task-harvest-fanin").unwrap()),
            harvest_audit_store: audit_store.clone(),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
            backend: backend.clone(),
        }));
        let evs: Vec<_> = ex
            .run_with_context(
                graph,
                "DIFF".into(),
                "run-harvest-fanin".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect()
            .await;
        for event in &evs {
            if let Err(error) = event {
                panic!("workflow failed unexpectedly: {error:?}");
            }
        }
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output,
            } if output == "FINAL"
        ));
        assert_eq!(
            backend.synth_prompts.lock().unwrap().clone(),
            vec!["alpha|beta".to_string()]
        );

        let bundles = bridge_core::harvest::HarvestAuditStore::list_by_task_id(
            audit_store.as_ref(),
            "task-harvest-fanin",
            None,
            10,
        )
        .await
        .unwrap();
        let mut cut_nodes = bundles
            .iter()
            .filter(|bundle| {
                bundle.decision.decision == bridge_core::harvest::HarvestDecision::CutAttested
            })
            .map(|bundle| {
                (
                    bundle.raw.node_id.as_str(),
                    bundle.raw.raw_body.as_str(),
                    bundle.decision.cut_byte_offset,
                    bundle.decision.effective_len_bytes,
                )
            })
            .collect::<Vec<_>>();
        cut_nodes.sort_by_key(|(node, ..)| *node);
        assert_eq!(
            cut_nodes,
            vec![
                ("a", "process-a\nalpha", Some("process-a\n".len() as u64), 5),
                ("b", "process-b\nbeta", Some("process-b\n".len() as u64), 4),
            ]
        );
    }

    #[tokio::test]
    async fn fan_out_runs_concurrently() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Barrier;
        // Both fan-out legs must ENTER prompt() before either replies → only possible if run in parallel.
        struct BarrierBackend {
            reply: String,
            barrier: Arc<Barrier>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for BarrierBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.barrier.wait().await; // deadlocks unless the other leg also reaches here
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(self.reply.clone())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                        prefix_attestation: Default::default(),
                    }),
                ])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        // BReg hands out BarrierBackend only for the first 2 resolves (the fan-out nodes);
        // node `t` (the terminal, resolved 3rd) gets a plain non-blocking backend so it
        // doesn't deadlock on a single-party wait.
        struct BReg {
            barrier: Arc<Barrier>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for BReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                let backend: Arc<dyn bridge_core::ports::AgentBackend> = if n < 2 {
                    Arc::new(BarrierBackend {
                        reply: id.as_str().to_uppercase(),
                        barrier: self.barrier.clone(),
                    })
                } else {
                    Arc::new(FakeBackend {
                        reply: id.as_str().to_uppercase(),
                        rec: Arc::new(Rec::default()),
                    })
                };
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend,
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // two-node graph: a, b both inputs=[] (fan-out), plus a terminal t depending on both.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let reg = Arc::new(BReg {
            barrier: Arc::new(Barrier::new(2)),
            calls: Arc::new(AtomicUsize::new(0)),
        }); // a + b must rendezvous
        let ex = WorkflowExecutor::new(reg);
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), CancellationToken::new())
                .collect::<Vec<_>>(),
        )
        .await;
        assert!(
            res.is_ok(),
            "fan-out legs ran concurrently (no deadlock/timeout)"
        );
    }

    #[tokio::test]
    async fn pipeline_threads_output_to_input() {
        // a -> b -> c ; b sees a's output, c sees b's.
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("a".to_string(), mk("AOUT")),
                ("b".to_string(), mk("BOUT")),
                ("c".to_string(), mk("COUT")),
            ]
            .into(),
        });
        let b_rec = reg.backends.get("b").unwrap().1.clone();
        let c_rec = reg.backends.get("c").unwrap().1.clone();
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("p").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let ex = WorkflowExecutor::new(reg);
        let _ = ex
            .run(g, "x".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;
        assert_eq!(b_rec.prompts.lock().unwrap()[0], "got AOUT");
        assert_eq!(c_rec.prompts.lock().unwrap()[0], "got BOUT");
    }

    #[tokio::test]
    async fn failed_fan_out_leg_marker_reaches_synth_and_run_completes() {
        // No "codex" backend registered → the codex node's resolve fails → error marker;
        // claude + synth still run (graceful degradation).
        let reg = Arc::new(FakeRegistry {
            backends: [
                (
                    "claude".to_string(),
                    ("CLAUDE_REVIEW".to_string(), Arc::new(Rec::default())),
                ),
                (
                    "synth".to_string(),
                    ("FINAL".to_string(), Arc::new(Rec::default())),
                ),
                // NOTE: no "codex" → resolve fails for the codex node
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await;
        // run COMPLETES (terminal synth ok) — graceful degradation
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
        // a NodeFinished{ok:false} was emitted for codex
        assert!(evs.iter().any(|e| matches!(e.as_ref().unwrap(),
            WorkflowEvent::NodeFinished { node, ok: false, .. } if node.as_str() == "codex")));
        // the EXACT failure marker reached synth's prompt
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("[node codex failed:"),
            "marker reached synth: {p}"
        );
    }

    #[tokio::test]
    async fn panel_degrades_failed_member_usage_is_n_a() {
        // No "member_a" backend registered → its node fails (error marker, usage None);
        // member_b + synth still run, synth's costs table shows member_a as n/a.
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("member_b".to_string(), mk("B_ANALYSIS")),
                ("synth".to_string(), mk("PANEL")),
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("panel").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("member_a").unwrap(),
                    agent: AgentId::parse("member_a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("member_b").unwrap(),
                    agent: AgentId::parse("member_b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{member_b}}\n{{workflow.costs}}".into(),
                    inputs: vec![
                        NodeId::parse("member_a").unwrap(),
                        NodeId::parse("member_b").unwrap(),
                    ],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let evs: Vec<_> = WorkflowExecutor::new(reg)
            .run(g, "DIFF".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("| member_a | n/a | n/a | n/a | n/a |"),
            "failed member usage row must be n/a: {p}"
        );
    }

    #[tokio::test]
    async fn cancel_calls_backend_cancel_and_ends_canceled() {
        // A backend whose prompt() stream NEVER yields Done (pending) → only the cancel path ends it.
        struct Pending {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for Pending {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(futures::stream::pending())) // never yields
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                *self.rec.cancels.lock().unwrap() += 1;
                Ok(())
            }
        }
        let rec = Arc::new(Rec::default());
        struct PReg {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for PReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(Pending {
                        rec: self.rec.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let token = CancellationToken::new();
        let reg = Arc::new(PReg { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            t2.cancel();
        });
        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ex.run(one_node_graph(), "x".into(), "r".into(), token)
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert_eq!(
            *rec.cancels.lock().unwrap(),
            1,
            "backend.cancel was called for the in-flight node"
        );
    }

    #[tokio::test]
    async fn cancel_drains_inflight() {
        // TWO fan-out legs, both genuinely in-flight (their prompt stream is pending),
        // when the token fires. Each leg's run_node cancel branch must run
        // backend.cancel() AND forget_session() — proving the FuturesUnordered drains
        // (not `break`s) after the first post-cancel completion. A `break` would drop
        // the second leg's future mid-cleanup → its counter never reaches 2.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        // Shared observability: cleanups counts cancel()+forget_session() calls;
        // entered counts prompt() entries; both_in_flight wakes the driver once
        // both legs have parked on their pending stream.
        struct Shared {
            cleanups: AtomicUsize,
            entered: AtomicUsize,
            both_in_flight: Notify,
        }
        struct CancelObservingBackend {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for CancelObservingBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                // Mark this leg as in-flight; once both legs are here, wake the driver.
                if self.shared.entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    self.shared.both_in_flight.notify_one();
                }
                // Pending stream → the node parks in run_node's select! until cancel.
                Ok(Box::pin(futures::stream::pending()))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                self.shared.cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn forget_session(&self, _s: &SessionId) {
                self.shared.cleanups.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct CReg {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for CReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(CancelObservingBackend {
                        shared: self.shared.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // Two fan-out legs (a, b — no inputs) + terminal t depending on both. Cancel
        // fires while a and b are in-flight, so t is never scheduled.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let shared = Arc::new(Shared {
            cleanups: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            both_in_flight: Notify::new(),
        });
        let reg = Arc::new(CReg {
            shared: shared.clone(),
        });
        let ex = WorkflowExecutor::new(reg);
        let token = CancellationToken::new();

        // Wait for both legs to be in-flight, then cancel.
        let t2 = token.clone();
        let s2 = shared.clone();
        tokio::spawn(async move {
            // notify_one before any waiter is dropped; re-check the counter to avoid races.
            if s2.entered.load(Ordering::SeqCst) < 2 {
                s2.both_in_flight.notified().await;
            }
            t2.cancel();
        });

        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), token).collect::<Vec<_>>(),
        )
        .await
        .expect("drain must complete after cancel (a `break` would also finish, but leak cleanup)");

        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        // BOTH legs must have run cancel()+forget_session() = 4 total cleanup calls.
        // A `break` after the first post-cancel completion drops the second leg's
        // future, aborting its cleanup → count would be 2, not 4.
        assert_eq!(
            shared.cleanups.load(Ordering::SeqCst),
            4,
            "both in-flight legs must run cancel()+forget_session() (drain, not break)"
        );
    }

    #[tokio::test]
    async fn completion_order() {
        // Two parallel nodes: a (fast) + b (slow). Completion-driven scheduling must
        // yield a's NodeFinished BEFORE b's — an ordering join_all did NOT guarantee
        // (join_all yields in ready-batch iteration order regardless of finish time).
        use std::sync::atomic::{AtomicBool, Ordering as AO};
        use tokio::sync::Notify;
        struct TimedBackend {
            reply: String,
            // None → reply immediately; Some(gate) → wait on gate before replying.
            gate: Option<Arc<Notify>>,
            // When `a` starts its prompt, signal the releaser task (None for non-a nodes).
            a_done: Option<(Arc<Notify>, Arc<AtomicBool>)>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for TimedBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                if let Some(g) = &self.gate {
                    g.notified().await; // park until released
                }
                // After returning from prompt(), the stream is a synchronous iter, so
                // run_node for this node will finish as soon as the executor polls it.
                // Signal the releaser that `a` has completed its prompt (and is therefore
                // done, since the iter stream yields synchronously).
                if let Some((notify, flag)) = &self.a_done {
                    flag.store(true, AO::SeqCst);
                    notify.notify_one();
                }
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(self.reply.clone())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                        prefix_attestation: Default::default(),
                    }),
                ])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let slow_gate = Arc::new(Notify::new());
        let a_done_notify = Arc::new(Notify::new());
        let a_done_flag = Arc::new(AtomicBool::new(false));
        struct TReg {
            slow_gate: Arc<Notify>,
            a_done_notify: Arc<Notify>,
            a_done_flag: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for TReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                // "b" is the slow leg (gated); "a" gets the a_done signal; "t" is plain.
                let gate = if id.as_str() == "b" {
                    Some(self.slow_gate.clone())
                } else {
                    None
                };
                let a_done = if id.as_str() == "a" {
                    Some((self.a_done_notify.clone(), self.a_done_flag.clone()))
                } else {
                    None
                };
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(TimedBackend {
                        reply: id.as_str().to_uppercase(),
                        gate,
                        a_done,
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // a, b parallel (no inputs); terminal t depends on both so the run completes.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });
        let reg = Arc::new(TReg {
            slow_gate: slow_gate.clone(),
            a_done_notify: a_done_notify.clone(),
            a_done_flag: a_done_flag.clone(),
        });
        let ex = WorkflowExecutor::new(reg);

        // Release the slow leg only AFTER `a` has signalled completion — causal ordering,
        // no wall-clock dependency. Guard against the notify firing before the waiter
        // starts (mirror the cancel_drains_inflight pattern).
        let g2 = slow_gate.clone();
        tokio::spawn(async move {
            if !a_done_flag.load(AO::SeqCst) {
                a_done_notify.notified().await;
            }
            g2.notify_waiters();
        });

        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), CancellationToken::new())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("run must complete")
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

        // Collect the order of NodeFinished ids for the two parallel legs.
        let finished_order: Vec<&str> = evs
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeFinished { node, .. }
                    if node.as_str() == "a" || node.as_str() == "b" =>
                {
                    Some(node.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            finished_order,
            vec!["a", "b"],
            "fast leg 'a' must finish before slow leg 'b' (completion-driven order)"
        );
    }

    #[tokio::test]
    async fn cancel_during_slow_prompt_ends_canceled_promptly() {
        struct SlowPrompt {
            rich_recorded: Arc<tokio::sync::Notify>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for SlowPrompt {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                panic!("cold prompt-open owner must use prompt_with_observers")
            }
            async fn prompt_with_observers(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
                observers: BackendObservers,
            ) -> Result<BackendStream, BridgeError> {
                observers
                    .rich
                    .expect("test supplies a rich sink")
                    .record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
                self.rich_recorded.notify_one();
                std::future::pending().await
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        struct SReg {
            backend: Arc<SlowPrompt>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for SReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: self.backend.clone(),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let token = CancellationToken::new();
        let t2 = token.clone();
        let rich_recorded = Arc::new(tokio::sync::Notify::new());
        let cancel_after_record = rich_recorded.clone();
        tokio::spawn(async move {
            cancel_after_record.notified().await;
            t2.cancel();
        });
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(Arc::new(SReg {
            backend: Arc::new(SlowPrompt { rich_recorded }),
        }));
        let evs = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ex.run_with_context(one_node_graph(), "x".into(), "r".into(), token, context)
                .collect::<Vec<_>>(),
        )
        .await
        .expect("cancel preempts the slow prompt setup");
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    // ── run_from tests ──────────────────────────────────────────────────────

    /// A 3-node fan-in (codex + claude → synth). Seed {codex, claude} as done;
    /// assert only `synth` is actually prompted, run completes, and `synth`'s
    /// prompt contains the seeded outputs.
    #[tokio::test]
    async fn run_from_skips_seeded_runs_rest() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("CODEX_SEEDED_IGNORED")),
                ("claude".to_string(), mk("CLAUDE_SEEDED_IGNORED")),
                ("synth".to_string(), mk("SYNTH_FINAL")),
            ]
            .into(),
        });
        let codex_rec = reg.backends.get("codex").unwrap().1.clone();
        let claude_rec = reg.backends.get("claude").unwrap().1.clone();
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();

        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = [
            ("codex".to_string(), ("OUTA".to_string(), true, None)),
            ("claude".to_string(), ("OUTB".to_string(), true, None)),
        ]
        .into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(
                review_graph(),
                "DIFF".into(),
                "resume1".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        // Run must complete successfully.
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "SYNTH_FINAL"),
            "terminal should be Completed/SYNTH_FINAL, got: {last:?}"
        );

        // codex and claude must NOT have been prompted (they were seeded).
        assert!(
            codex_rec.prompts.lock().unwrap().is_empty(),
            "codex was seeded; its backend must not be prompted"
        );
        assert!(
            claude_rec.prompts.lock().unwrap().is_empty(),
            "claude was seeded; its backend must not be prompted"
        );

        // synth MUST have been prompted exactly once, and its prompt must contain
        // the seeded outputs OUTA and OUTB (passed as template vars).
        let synth_prompts = synth_rec.prompts.lock().unwrap();
        assert_eq!(
            synth_prompts.len(),
            1,
            "synth should be prompted exactly once"
        );
        let p = &synth_prompts[0];
        assert!(
            p.contains("OUTA") && p.contains("OUTB"),
            "synth prompt must contain seeded outputs OUTA and OUTB: {p}"
        );

        // Exactly ONE NodeStarted (synth) and ONE NodeFinished (synth) emitted.
        let started: Vec<_> = evs
            .iter()
            .filter_map(|e| match e.as_ref().unwrap() {
                WorkflowEvent::NodeStarted { node } => Some(node.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(started, vec!["synth"], "only synth should be started");

        // Exactly ONE NodeFinished (synth) emitted — symmetry with NodeStarted.
        let finished: Vec<_> = evs
            .iter()
            .filter_map(|e| match e.as_ref().unwrap() {
                WorkflowEvent::NodeFinished { node, .. } => Some(node.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec!["synth"], "only synth should be finished");
    }

    #[tokio::test]
    async fn resumed_synth_sees_seeded_member_usage() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [("synth".to_string(), mk("FINAL"))].into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);

        let mut seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = HashMap::new();
        seed.insert(
            "codex".into(),
            (
                "CODEX_REVIEW".into(),
                true,
                Some(UsageSnapshot {
                    used: Some(15071),
                    size: Some(258400),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                }),
            ),
        );
        seed.insert("claude".into(), ("CLAUDE_REVIEW".into(), true, None));

        let _ = ex
            .run_from(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("| codex | 15071 | 258400 |"),
            "resumed synth costs table shows seeded member usage: {p}"
        );
        assert!(
            p.contains("| claude | n/a |"),
            "member with no captured usage -> n/a: {p}"
        );
    }

    /// Seed contains a node id not present in the graph → stream yields ConfigInvalid.
    #[tokio::test]
    async fn run_from_unknown_seed_node_errors() {
        let reg = Arc::new(FakeRegistry {
            backends: [(
                "codex".to_string(),
                ("X".to_string(), Arc::new(Rec::default())),
            )]
            .into(),
        });
        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> =
            [("ghost_node".to_string(), ("OUT".to_string(), true, None))].into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(
                one_node_graph(),
                "inp".into(),
                "r".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(evs.len(), 1, "should yield exactly one error event");
        let err = evs[0].as_ref().unwrap_err();
        assert!(
            matches!(err, BridgeError::ConfigInvalid { reason } if reason.contains("unknown node")),
            "expected ConfigInvalid about unknown node, got: {err:?}"
        );
    }

    // ── WorkflowRunContext / cwd threading tests ────────────────────────────

    /// Recording backend that captures the `SessionSpec.cwd` from each
    /// `configure_session` call. Used to verify `WorkflowRunContext` is
    /// forwarded to EVERY node.
    #[derive(Default)]
    struct CwdRec {
        cwds: Mutex<Vec<Option<SessionCwd>>>,
    }
    struct CwdCapBackend {
        reply: String,
        rec: Arc<CwdRec>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for CwdCapBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text(self.reply.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn configure_session(
            &self,
            _s: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.cwds.lock().unwrap().push(spec.cwd.clone());
            Ok(())
        }
    }
    struct CwdCapRegistry {
        rec: Arc<CwdRec>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for CwdCapRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(CwdCapBackend {
                    reply: "OK".into(),
                    rec: self.rec.clone(),
                }),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("a").unwrap()
        }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    /// `run_from_with_context` with `session_cwd = Some("/req")` → EVERY node's
    /// `configure_session` receives `spec.cwd == Some("/req")`.
    #[tokio::test]
    async fn run_from_with_context_cwd_set_reaches_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let ctx = WorkflowRunContext {
            session_cwd: Some(SessionCwd::parse("/req").unwrap()),
            make_rich_sink: None,
            ..WorkflowRunContext::default()
        };
        let _evs: Vec<_> = ex
            .run_from_with_context(
                review_graph(), // 3 nodes: codex, claude, synth
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                HashMap::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert_eq!(
                cwd.as_ref().map(|c| c.as_str()),
                Some("/req"),
                "every node must receive cwd=/req, got {:?}",
                cwd
            );
        }
    }

    /// `run_from_with_context` with `WorkflowRunContext::default()` (None cwd) →
    /// every node's `configure_session` receives `spec.cwd == None`.
    #[tokio::test]
    async fn run_from_with_context_cwd_none_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let _evs: Vec<_> = ex
            .run_from_with_context(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                HashMap::new(),
                WorkflowRunContext::default(),
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert!(
                cwd.is_none(),
                "every node must receive cwd=None, got {:?}",
                cwd
            );
        }
    }

    /// `run_with_context` (scratch, no seed) propagates cwd to every node.
    #[tokio::test]
    async fn run_with_context_cwd_set_reaches_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let ctx = WorkflowRunContext {
            session_cwd: Some(SessionCwd::parse("/req2").unwrap()),
            make_rich_sink: None,
            ..WorkflowRunContext::default()
        };
        let _evs: Vec<_> = ex
            .run_with_context(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert_eq!(
                cwd.as_ref().map(|c| c.as_str()),
                Some("/req2"),
                "every node must receive cwd=/req2, got {:?}",
                cwd
            );
        }
    }

    /// Seed contains a non-root node (b, which depends on a) but NOT its upstream (a).
    /// This violates the closure invariant → stream yields ConfigInvalid.
    ///
    /// Graph: a → b → c  (pipeline_threads_output_to_input shape)
    #[tokio::test]
    async fn run_from_seed_not_closed_errors() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("a".to_string(), mk("AOUT")),
                ("b".to_string(), mk("BOUT")),
                ("c".to_string(), mk("COUT")),
            ]
            .into(),
        });

        // Graph: a → b → c
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("p").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                    retry: None,
                    harvest_sanitization: None,
                },
            ],
            panel: None,
            controls: None,
        });

        // Seed only `b` without its upstream `a` → closure violation.
        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> =
            [("b".to_string(), ("BOUT".to_string(), true, None))].into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(g, "inp".into(), "r".into(), CancellationToken::new(), seed)
            .collect::<Vec<_>>()
            .await;

        assert_eq!(evs.len(), 1, "should yield exactly one error event");
        let err = evs[0].as_ref().unwrap_err();
        assert!(
            matches!(err, BridgeError::ConfigInvalid { reason } if reason.contains("closed under inputs")),
            "expected ConfigInvalid about closure, got: {err:?}"
        );
    }
}

#[cfg(test)]
mod observability_tests {
    use super::*;
    use crate::executor::tests::{FakeRegistry, Rec as PromptRec};
    use crate::graph::{RetryPolicy, WorkflowGraph, WorkflowNode};
    use bridge_core::domain::Part;
    use bridge_core::error::BridgeError;
    use bridge_core::harvest::NoopHarvestAuditStore;
    use bridge_core::ids::{AgentId, SessionId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::{
        AgentBackend, AgentRegistry, BackendStream, FailureClass, Lease, ObsEvent, Observer,
        Resolved, TurnOutcome, Update,
    };
    use futures::StreamExt;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct Rec(Mutex<Vec<&'static str>>);
    impl Observer for Rec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::NodeStarted { .. } => "node_started",
                ObsEvent::TurnStarted { .. } => "turn_started",
                ObsEvent::UsageFinalized { .. } => "usage",
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::NodeFinished { .. } => "node_finished",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    struct UsageBackend;
    #[async_trait::async_trait]
    impl AgentBackend for UsageBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, bridge_core::error::BridgeError> {
            let updates = vec![
                Ok(Update::Usage(UsageSnapshot {
                    used: Some(1),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                })),
                Ok(Update::Text("usage-only-text".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), bridge_core::error::BridgeError> {
            Ok(())
        }
    }
    struct NoopLease2;
    impl Lease for NoopLease2 {}
    struct UsageRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for UsageRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, bridge_core::error::BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    preflight: false,
                    fallback_models: vec![],
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    host_fallback_eligible: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(UsageBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(
            &self,
            _: bridge_core::domain::RegistrySnapshot,
        ) -> Result<(), bridge_core::error::BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn workflow_node_emits_lifecycle_around_usage() {
        let rec = Arc::new(Rec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: Some("prompt/workflow".to_string()),
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let graph = Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        });
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context(
            graph,
            "input".into(),
            "task-1".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec![
                "node_started",
                "turn_started",
                "turn_finished",
                "usage",
                "node_finished"
            ]
        );
    }

    // ---------------------------------------------------------------------------
    // Helpers shared by the remaining tests
    // ---------------------------------------------------------------------------

    /// Detailed recording observer: captures per-event structs so tests can
    /// inspect outcome and ttft values, not just tag order.
    #[derive(Default, Clone)]
    struct DetailedRec {
        events: Arc<Mutex<Vec<DetailedEvt>>>,
    }
    #[derive(Debug, Clone)]
    enum DetailedEvt {
        NodeStarted,
        TurnStarted,
        TurnFinished {
            outcome: TurnOutcome,
            ttft: Option<std::time::Duration>,
        },
        NodeFinished(TurnOutcome),
    }
    impl Observer for DetailedRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let ev = match e {
                ObsEvent::NodeStarted { .. } => DetailedEvt::NodeStarted,
                ObsEvent::TurnStarted { .. } => DetailedEvt::TurnStarted,
                ObsEvent::TurnFinished { outcome, ttft, .. } => DetailedEvt::TurnFinished {
                    outcome: (*outcome).clone(),
                    ttft: *ttft,
                },
                ObsEvent::NodeFinished { outcome, .. } => {
                    DetailedEvt::NodeFinished((*outcome).clone())
                }
                _ => return,
            };
            self.events.lock().unwrap().push(ev);
        }
    }

    /// A no-op `NodeTurnCleanup` for warm-path tests.
    struct NoopCleanup;
    #[async_trait::async_trait]
    impl NodeTurnCleanup for NoopCleanup {
        async fn on_exit(self: Box<Self>, _: NodeTurnExit) {}
    }

    /// Backend that emits `Update::Text(text)` then `Update::Done`.
    struct TextBackend(String);
    #[async_trait::async_trait]
    impl AgentBackend for TextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let t = self.0.clone();
            let updates = vec![
                Ok(Update::Text(t)),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct TextLease;
    impl Lease for TextLease {}

    fn make_single_node_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        })
    }

    fn make_retry_node_graph(policy: RetryPolicy) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: Some(policy),
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        })
    }

    // ---------------------------------------------------------------------------
    // Test 1: warm dispatcher path emits full ordered lifecycle
    // ---------------------------------------------------------------------------

    struct TextDispatcher(String);
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for TextDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(TextBackend(self.0.clone())),
                session: SessionId::parse("warm-sess").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn warm_path_emits_full_lifecycle() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run1".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(TextDispatcher("hello".into())),
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        assert_eq!(evs.len(), 4, "expected 4 lifecycle events, got: {evs:?}");
        assert!(matches!(evs[0], DetailedEvt::NodeStarted), "evs[0]={evs:?}");
        assert!(matches!(evs[1], DetailedEvt::TurnStarted), "evs[1]={evs:?}");
        assert!(
            matches!(
                evs[2],
                DetailedEvt::TurnFinished {
                    outcome: TurnOutcome::Success,
                    ..
                }
            ),
            "evs[2]={evs:?}"
        );
        assert!(
            matches!(evs[3], DetailedEvt::NodeFinished(TurnOutcome::Success)),
            "evs[3]={evs:?}"
        );
    }

    struct EmptyThenTextBackend {
        text: Option<String>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for EmptyThenTextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let mut updates = Vec::new();
            if let Some(text) = &self.text {
                updates.push(Ok(Update::Text(text.clone())));
            }
            updates.push(Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }));
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct RecordingCleanup {
        exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NodeTurnCleanup for RecordingCleanup {
        async fn on_exit(self: Box<Self>, exit: NodeTurnExit) {
            let label = match exit {
                NodeTurnExit::Normal => "normal".to_string(),
                NodeTurnExit::Canceled => "canceled".to_string(),
                NodeTurnExit::Error(error) => format!("error:{error:?}"),
            };
            self.exits.lock().unwrap().push(label);
        }
    }

    #[derive(Default)]
    struct EmptyThenTextDispatcher {
        checkouts: Arc<AtomicUsize>,
        exits: Arc<Mutex<Vec<String>>>,
        recover_on_retry: bool,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for EmptyThenTextDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            let checkout = self.checkouts.fetch_add(1, Ordering::SeqCst) + 1;
            let text = if checkout == 1 || !self.recover_on_retry {
                None
            } else {
                Some("warm-retried".to_string())
            };
            Ok(NodeTurn {
                backend: Arc::new(EmptyThenTextBackend { text }),
                session: SessionId::parse(format!("warm-empty-retry-{checkout}")).unwrap(),
                seed: None,
                cleanup: Box::new(RecordingCleanup {
                    exits: self.exits.clone(),
                }),
            })
        }
    }

    #[tokio::test]
    async fn dispatcher_empty_final_fails_without_replaying_in_a_fresh_checkout() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec,
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let dispatcher = Arc::new(EmptyThenTextDispatcher {
            recover_on_retry: true,
            ..Default::default()
        });
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let events = exec
            .run_with_context_and_dispatcher(
                make_single_node_graph(),
                "inp".into(),
                "run-empty-warm".into(),
                CancellationToken::new(),
                ctx,
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output) = events
            .iter()
            .find_map(|event| match event {
                WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                _ => None,
            })
            .expect("node finished");
        assert!(
            !ok,
            "accepted dispatcher empty final must not replay: {output}"
        );
        assert!(output.contains("EmptyFinal"), "{output}");
        assert_eq!(dispatcher.checkouts.load(Ordering::SeqCst), 1);
        let exits = dispatcher.exits.lock().unwrap().clone();
        assert_eq!(exits, vec!["error:EmptyFinal".to_string()]);
    }

    #[tokio::test]
    async fn dispatcher_empty_final_stays_single_attempt_when_recovery_is_unavailable() {
        let dispatcher = Arc::new(EmptyThenTextDispatcher::default());
        let events = WorkflowExecutor::new(Arc::new(UsageRegistry))
            .run_with_context_and_dispatcher(
                make_single_node_graph(),
                "inp".into(),
                "run-twin-empty-warm".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher.clone(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|event| event.unwrap())
            .collect::<Vec<_>>();

        let (ok, output) = events
            .iter()
            .find_map(|event| match event {
                WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                _ => None,
            })
            .expect("node finished");
        assert!(!ok, "accepted empty final cannot succeed: {output}");
        assert!(output.contains("EmptyFinal"), "{output}");
        assert_eq!(dispatcher.checkouts.load(Ordering::SeqCst), 1);
        assert_eq!(
            dispatcher.exits.lock().unwrap().as_slice(),
            ["error:EmptyFinal"]
        );
    }

    // ---------------------------------------------------------------------------
    // Test 2: cold path TurnFinished.ttft is Some when text is emitted
    // ---------------------------------------------------------------------------

    struct TextRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for TextRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    preflight: false,
                    fallback_models: vec![],
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    host_fallback_eligible: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(TextBackend("hello".into())),
                lease: Box::new(TextLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn cold_path_ttft_some_when_text_emitted() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(TextRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run2".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let turn_finished = evs
            .iter()
            .find(|e| matches!(e, DetailedEvt::TurnFinished { .. }));
        let ttft = match turn_finished {
            Some(DetailedEvt::TurnFinished { ttft, .. }) => *ttft,
            _ => panic!("no TurnFinished event found in {evs:?}"),
        };
        assert!(
            ttft.is_some(),
            "expected ttft=Some(..) when backend emits text, got None"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 3: fatal AgentTimedOut → TurnFinished outcome = Failed(TimedOut)
    // ---------------------------------------------------------------------------

    struct TimedOutBackend;
    #[async_trait::async_trait]
    impl AgentBackend for TimedOutBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Err(BridgeError::AgentTimedOut)
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct TimedOutRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for TimedOutRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    preflight: false,
                    fallback_models: vec![],
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    host_fallback_eligible: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(TimedOutBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn fatal_timedout_produces_failed_timed_out_class() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        // No retry → AgentTimedOut is fatal on the only attempt.
        let exec = WorkflowExecutor::new(Arc::new(TimedOutRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run3".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let turn_finished = evs
            .iter()
            .find(|e| matches!(e, DetailedEvt::TurnFinished { .. }));
        match turn_finished {
            Some(DetailedEvt::TurnFinished {
                outcome: TurnOutcome::Failed(FailureClass::TimedOut),
                ..
            }) => {}
            other => panic!("expected TurnFinished(Failed(TimedOut)), got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------
    // Test 4: retry path — exactly 1 NodeStarted / 1 NodeFinished, 2 each TurnStarted / TurnFinished
    // ---------------------------------------------------------------------------

    /// Backend that fails with AgentTimedOut (transient) on the first `prompt` call,
    /// then succeeds with a text response on subsequent calls.
    struct FailOnceThenTextBackend {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for FailOnceThenTextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(BridgeError::AgentTimedOut);
            }
            let updates = vec![
                Ok(Update::Text("ok".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                    prefix_attestation: Default::default(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct FailOnceThenTextRegistry {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for FailOnceThenTextRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    preflight: false,
                    fallback_models: vec![],
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    host_fallback_eligible: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(FailOnceThenTextBackend {
                    calls: self.calls.clone(),
                }),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn retry_emits_node_started_once_node_finished_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(FailOnceThenTextRegistry { calls }));
        // 2 attempts, 0ms backoff so the test doesn't sleep.
        let graph = make_retry_node_graph(RetryPolicy {
            max_attempts: 2,
            backoff_ms: 0,
            backoff_cap_ms: Some(0),
        });
        let mut stream = exec.run_with_context(
            graph,
            "inp".into(),
            "run4".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let node_started = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::NodeStarted))
            .count();
        let node_finished = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::NodeFinished(..)))
            .count();
        let turn_started = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::TurnStarted))
            .count();
        let turn_finished = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::TurnFinished { .. }))
            .count();

        assert_eq!(node_started, 1, "expected 1 NodeStarted; events: {evs:?}");
        assert_eq!(node_finished, 1, "expected 1 NodeFinished; events: {evs:?}");
        assert_eq!(
            turn_started, 2,
            "expected 2 TurnStarted (one per attempt); events: {evs:?}"
        );
        assert_eq!(
            turn_finished, 2,
            "expected 2 TurnFinished (one per attempt); events: {evs:?}"
        );
        // Final node outcome should be success (second attempt succeeded).
        assert!(
            matches!(
                evs.last(),
                Some(DetailedEvt::NodeFinished(TurnOutcome::Success))
            ),
            "expected final NodeFinished(Success); events: {evs:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 5: TurnFinished emitted BEFORE UsageFinalized on both warm and cold paths
    // ---------------------------------------------------------------------------

    #[derive(Default)]
    struct OrderRec(Mutex<Vec<&'static str>>);
    impl Observer for OrderRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::UsageFinalized { .. } => "usage_finalized",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    #[derive(Default)]
    struct UsageFinalRec(Mutex<Vec<bool>>);
    impl Observer for UsageFinalRec {
        fn record(&self, e: &ObsEvent<'_>) {
            if let ObsEvent::UsageFinalized { usage, fin, .. } = e {
                if *fin == UsageFinalization::TurnFinal {
                    self.0.lock().unwrap().push(usage.is_some());
                }
            }
        }
    }

    #[derive(Default)]
    struct PromptOpenFinalizationRec(Mutex<Vec<&'static str>>);
    impl Observer for PromptOpenFinalizationRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::UsageFinalized {
                    usage: None,
                    fin: UsageFinalization::TurnFinal,
                    ..
                } => "no_usage_finalized",
                ObsEvent::UsageFinalized { .. } => "unexpected_usage_finalization",
                ObsEvent::NodeFinished { .. } => "node_finished",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    struct NoUsageIdleBackend;
    #[async_trait::async_trait]
    impl AgentBackend for NoUsageIdleBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(futures::stream::pending::<
                Result<Update, BridgeError>,
            >()))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct NoUsageIdleRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for NoUsageIdleRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(super::tests::minimal_entry(id)),
                backend: Arc::new(NoUsageIdleBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[derive(Default)]
    struct StartAndUsageFinalRec {
        started: AtomicUsize,
        usages: Mutex<Vec<bool>>,
    }
    impl Observer for StartAndUsageFinalRec {
        fn record(&self, e: &ObsEvent<'_>) {
            match e {
                ObsEvent::TurnStarted { .. } => {
                    self.started.fetch_add(1, Ordering::SeqCst);
                }
                ObsEvent::UsageFinalized { usage, fin, .. }
                    if *fin == UsageFinalization::TurnFinal =>
                {
                    self.usages.lock().unwrap().push(usage.is_some());
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn workflow_success_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(UsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(TextRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-success".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn workflow_failure_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(UsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(TimedOutRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-failure".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn workflow_cancel_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(StartAndUsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let token = CancellationToken::new();
        let exec = WorkflowExecutor::new(Arc::new(NoUsageIdleRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-cancel".into(),
            token.clone(),
            ctx,
        );
        let drain = tokio::spawn(async move { while stream.next().await.is_some() {} });

        for _ in 0..1000 {
            if rec.started.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert_eq!(rec.started.load(Ordering::SeqCst), 1);
        token.cancel();
        drain.await.unwrap();

        let events = rec.usages.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn turn_finished_emitted_before_usage_finalized_cold_path() {
        let rec = Arc::new(OrderRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-order-cold".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec!["turn_finished", "usage_finalized"],
            "TurnFinished must precede UsageFinalized; got: {tags:?}"
        );
    }

    struct UsageDispatcher;
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for UsageDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(UsageBackend),
                session: SessionId::parse("warm-usage-sess").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn turn_finished_emitted_before_usage_finalized_warm_path() {
        let rec = Arc::new(OrderRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run-order-warm".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(UsageDispatcher),
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec!["turn_finished", "usage_finalized"],
            "TurnFinished must precede UsageFinalized on warm path; got: {tags:?}"
        );
    }

    #[derive(Default)]
    struct PromptOpenRichSink {
        events: AtomicUsize,
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for PromptOpenRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {
            self.events.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PromptOpenRichFactory {
        sink: Arc<PromptOpenRichSink>,
    }

    impl bridge_core::ports::RichEventSinkFactory for PromptOpenRichFactory {
        fn make(
            &self,
            _node: &bridge_core::ids::NodeId,
        ) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    struct SlowPromptOpenDispatcher {
        rich_recorded: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for SlowPromptOpenDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            struct SlowPromptOpenBackend {
                rich_recorded: Arc<tokio::sync::Notify>,
            }
            #[async_trait::async_trait]
            impl AgentBackend for SlowPromptOpenBackend {
                async fn prompt(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                ) -> Result<BackendStream, BridgeError> {
                    panic!("warm prompt-open owner must use prompt_with_observers")
                }

                async fn prompt_with_observers(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                    observers: BackendObservers,
                ) -> Result<BackendStream, BridgeError> {
                    observers
                        .rich
                        .expect("test supplies a rich sink")
                        .record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
                    self.rich_recorded.notify_one();
                    std::future::pending().await
                }

                async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                    Ok(())
                }
            }

            Ok(NodeTurn {
                backend: Arc::new(SlowPromptOpenBackend {
                    rich_recorded: self.rich_recorded.clone(),
                }),
                session: SessionId::parse("warm-slow-prompt-open").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn dispatcher_cancel_during_prompt_open_emits_explicit_no_usage() {
        let rec = Arc::new(PromptOpenFinalizationRec::default());
        let token = CancellationToken::new();
        let cancel = token.clone();
        let rich_recorded = Arc::new(tokio::sync::Notify::new());
        let cancel_after_record = rich_recorded.clone();
        tokio::spawn(async move {
            cancel_after_record.notified().await;
            cancel.cancel();
        });
        let rich_sink = Arc::new(PromptOpenRichSink::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: Some(Arc::new(PromptOpenRichFactory {
                sink: rich_sink.clone(),
            })),
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            exec.run_with_context_and_dispatcher(
                make_single_node_graph(),
                "inp".into(),
                "run-cancel-prompt-open".into(),
                token,
                ctx,
                Arc::new(SlowPromptOpenDispatcher { rich_recorded }),
            )
            .collect::<Vec<_>>(),
        )
        .await
        .expect("cancellation must preempt warm prompt setup");

        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["turn_finished", "no_usage_finalized", "node_finished"]
        );
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    struct FailingPromptOpenDispatcher;
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for FailingPromptOpenDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            struct FailingPromptOpenBackend;
            #[async_trait::async_trait]
            impl AgentBackend for FailingPromptOpenBackend {
                async fn prompt(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                ) -> Result<BackendStream, BridgeError> {
                    Err(BridgeError::AgentTimedOut)
                }

                async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                    Ok(())
                }
            }

            Ok(NodeTurn {
                backend: Arc::new(FailingPromptOpenBackend),
                session: SessionId::parse("warm-failing-prompt-open").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn dispatcher_prompt_error_emits_explicit_no_usage() {
        let rec = Arc::new(PromptOpenFinalizationRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
            harvest_audit_store: Arc::new(NoopHarvestAuditStore),
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));

        exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run-error-prompt-open".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(FailingPromptOpenDispatcher),
        )
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["turn_finished", "no_usage_finalized", "node_finished"]
        );
    }

    fn prompt_barrier_fixture() -> (WorkflowExecutor, Arc<PromptRec>) {
        let rec = Arc::new(PromptRec::default());
        let registry = Arc::new(FakeRegistry {
            backends: [("codex".to_owned(), ("done".to_owned(), rec.clone()))].into(),
        });
        (WorkflowExecutor::new(registry), rec)
    }

    #[tokio::test]
    async fn prompt_dispatch_barrier_completes_before_provider_poll() {
        let (executor, rec) = prompt_barrier_fixture();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let barrier: PromptDispatchBarrier = {
            let entered = entered.clone();
            let release = release.clone();
            Arc::new(move || {
                let entered = entered.clone();
                let release = release.clone();
                Box::pin(async move {
                    entered.notify_one();
                    let permit = release.acquire().await.unwrap();
                    permit.forget();
                })
            })
        };
        let telemetry = Arc::new(
            bridge_core::attempt_activity::AttemptTelemetrySinkFactory::new(
                "workflow-barrier-evidence",
            ),
        );
        let ctx = WorkflowDiagnosticContext::in_memory(WorkflowRunContext {
            make_rich_sink: Some(telemetry.clone()),
            ..WorkflowRunContext::default()
        })
        .with_prompt_dispatch_barrier(barrier);
        let run = tokio::spawn(async move {
            executor
                .run_with_diagnostic_context(
                    make_single_node_graph(),
                    "input".into(),
                    "run".into(),
                    CancellationToken::new(),
                    ctx,
                )
                .collect::<Vec<_>>()
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        assert!(
            rec.prompts.lock().unwrap().is_empty(),
            "provider prompt must remain unpolled while the durable barrier is pending"
        );
        assert_eq!(
            telemetry.evidence().turn_count(),
            0,
            "a canceled or blocked barrier must not register a provider turn"
        );
        release.add_permits(1);
        let events = run.await.unwrap();
        assert!(!rec.prompts.lock().unwrap().is_empty());
        assert_eq!(telemetry.evidence().turn_count(), 1);
        assert!(matches!(
            events.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn cancellation_before_prompt_poll_does_not_claim_acceptance() {
        let (executor, rec) = prompt_barrier_fixture();
        let barrier_calls = Arc::new(AtomicUsize::new(0));
        let barrier: PromptDispatchBarrier = {
            let barrier_calls = barrier_calls.clone();
            Arc::new(move || {
                let barrier_calls = barrier_calls.clone();
                Box::pin(async move {
                    barrier_calls.fetch_add(1, Ordering::SeqCst);
                })
            })
        };
        let token = CancellationToken::new();
        token.cancel();
        let events = executor
            .run_with_diagnostic_context(
                make_single_node_graph(),
                "input".into(),
                "run".into(),
                token,
                WorkflowDiagnosticContext::in_memory(WorkflowRunContext::default())
                    .with_prompt_dispatch_barrier(barrier),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(barrier_calls.load(Ordering::SeqCst), 0);
        assert!(rec.prompts.lock().unwrap().is_empty());
        assert!(matches!(
            events.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
    }
}
