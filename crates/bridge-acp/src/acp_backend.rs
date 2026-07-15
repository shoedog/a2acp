// acp_backend.rs — AcpBackend: a conformant ACP *client* over the
// `agent-client-protocol` SDK (=1.0.1). It drives `initialize`, lazy
// `session/new`, streaming `session/prompt` (fan-in of `session/update`
// notifications), and `session/cancel`.
//
// Spec §5.3 cancellation rule: completion is the prompt RESULT (stopReason
// "cancelled"), NOT the act of sending session/cancel. See Codex finding 2.
// Full cancel *completion* semantics live in Task 4; Task 3's `cancel` only
// latches + sends the notification.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodId, AuthenticateRequest, CancelNotification,
    ContentBlock, CreateTerminalRequest, CreateTerminalResponse, EnvVariable, InitializeRequest,
    InitializeResponse, KillTerminalRequest, KillTerminalResponse, McpServer, McpServerStdio,
    NewSessionRequest, PermissionOption, PermissionOptionKind, PlanEntryPriority, PlanEntryStatus,
    PromptRequest, ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest,
    ReleaseTerminalResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionConfigId, SessionConfigOption,
    SessionConfigValueId, SessionId as AgentSessionId, SessionModeState, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason,
    TerminalOutputRequest, TerminalOutputResponse, TextContent, ToolCallContent, ToolCallLocation,
    ToolCallStatus, ToolKind, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{
    Agent, ByteStreams, Client, ConnectTo, ConnectionTo, Error as AcpError, ErrorCode,
};
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot, Mutex, OnceCell, OwnedMutexGuard};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::model_effort::{
    caps_from_config_options, effort_opt, is_blocked_model, is_unsupported_effort_error,
    model_values, resolve_effort, resolve_model, EffortDecision, ModelDecision,
    ModelResolutionError, EFFORT_ORDER,
};
use bridge_core::catalog::AgentCaps;
use bridge_core::diagnostics::{
    diagnostic_timestamp_ms, AuthenticationEvidenceInput, DiagnosticFailureClass,
    DiagnosticOperation, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
    FailureDiagnosticInput, FailureDisposition, PersistedPhaseTransition,
    PersistedPhaseTransitionInput, PhaseStatus, StderrScope,
};
use bridge_core::domain::{
    EffectiveConfig, Effort, PermissionDecision, PermissionRequest, PermitDecision, SessionContext,
    SessionSpec,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::orch::{
    AgentSessionCaps, ContentSummary, OrchEventKind, PlanEntry as BridgePlanEntry, ReconcileOutcome,
};
use bridge_core::permission::{
    PendingPermissionView, PermKey, PermissionOptionView, PermissionRegistry, PermissionResolution,
    TurnMeta,
};
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, PolicyEngine, PolicyOutcome,
    RichEventSink, Update, STOP_REASON_CANCELLED,
};

use bridge_core::process::{
    ProcessStderrCursor, ProcessStderrRing, ProcessStderrSnapshot, Supervised,
};
use bridge_core::provider::{classify_acp_error_data, ProviderEvidence};
use bridge_core::reaper::{production_reap_fn, ReapController, ReapFn};

/// Default bound on the `initialize` handshake. A real agent that connects its
/// stdio but never sends the initialize response would otherwise hang
/// `connect`/`spawn` forever; on elapse we return a clear `BridgeError`.
const DEFAULT_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Default grace a `cancel` (or an early stream-drop) gives the agent to honor
/// `session/cancel` and return its terminal `StopReason::Cancelled` result
/// before we escalate. On elapse we SIGTERM→SIGKILL the whole agent process
/// (see [`AcpBackend::escalate_terminate`]) so a hung in-flight turn cannot hold
/// the per-session turn lock — and hang the caller's stream — forever.
const DEFAULT_CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Grace handed to `Supervised::terminate` between SIGTERM and the SIGKILL
/// escalation when we nuke the agent process on a cancel/drop timeout.
const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

const RICH_CONTENT_CAP: usize = 2048;
const RICH_VEC_CAP: usize = 64;
const PERMISSION_VIEW_CAP: usize = 4096;

#[derive(Clone)]
struct AcpLifecycle {
    observer: Arc<dyn DiagnosticObserver>,
    redactor: DiagnosticRedactor,
    stderr: Option<(ProcessStderrRing, ProcessStderrCursor)>,
}

impl AcpLifecycle {
    fn new(
        observer: Arc<dyn DiagnosticObserver>,
        redactor: DiagnosticRedactor,
        stderr: Option<(ProcessStderrRing, ProcessStderrCursor)>,
    ) -> Self {
        Self {
            observer,
            redactor,
            stderr,
        }
    }

    fn stderr_snapshot(&self) -> Option<ProcessStderrSnapshot> {
        self.stderr
            .as_ref()
            .map(|(ring, cursor)| ring.metadata_since(*cursor))
    }

    async fn record(
        &self,
        phase: DiagnosticPhase,
        status: PhaseStatus,
        operation: Option<DiagnosticOperation>,
        code: Option<&'static str>,
        auth: Option<AuthenticationEvidenceInput>,
    ) -> Result<(), BridgeError> {
        let transition = PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status,
                at_ms: diagnostic_timestamp_ms(),
                operation,
                code: None,
                auth,
            },
            code,
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
        phase: DiagnosticPhase,
        last_completed_phase: Option<DiagnosticPhase>,
        class: DiagnosticFailureClass,
        disposition: FailureDisposition,
        code: &'static str,
        summary: &'static str,
        cause: Option<String>,
        prompt_may_have_been_accepted: bool,
        stderr: Option<ProcessStderrSnapshot>,
        operation: Option<DiagnosticOperation>,
        auth: Option<AuthenticationEvidenceInput>,
    ) -> BridgeError {
        self.failure_with_retry_metadata(
            phase,
            last_completed_phase,
            class,
            disposition,
            code,
            summary,
            cause,
            None,
            None,
            prompt_may_have_been_accepted,
            stderr,
            operation,
            auth,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn failure_with_retry_metadata(
        &self,
        phase: DiagnosticPhase,
        last_completed_phase: Option<DiagnosticPhase>,
        class: DiagnosticFailureClass,
        disposition: FailureDisposition,
        code: &'static str,
        summary: &'static str,
        cause: Option<String>,
        retry_after_ms: Option<u64>,
        reset_at_ms: Option<i64>,
        prompt_may_have_been_accepted: bool,
        stderr: Option<ProcessStderrSnapshot>,
        operation: Option<DiagnosticOperation>,
        auth: Option<AuthenticationEvidenceInput>,
    ) -> BridgeError {
        let stderr = stderr.or_else(|| self.stderr_snapshot());
        let stderr_line_count = stderr.as_ref().map_or(0, ProcessStderrSnapshot::line_count);
        let stderr_observed = stderr_line_count != 0;
        let failure = match FailureDiagnostic::build_static_code(
            FailureDiagnosticInput {
                failed_phase: phase,
                last_completed_phase,
                class,
                disposition,
                code: String::new(),
                summary: summary.to_owned(),
                causes: cause.into_iter().collect(),
                stderr_observed,
                stderr_line_count,
                stderr_scope: stderr_observed.then_some(StderrScope::Process),
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
            Err(_) => return BridgeError::InvalidStateTransition,
        };
        let transition = match PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status: PhaseStatus::Failed,
                at_ms: diagnostic_timestamp_ms(),
                operation,
                code: None,
                auth,
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

#[derive(Clone, Copy)]
enum AcpTraceEvent {
    ModelOptionsMissing,
    EffortBelowMinimum {
        advertised_count: u16,
    },
    EffortOptionMissing,
    ConfigResolved {
        effort_applied: bool,
        fell_back: bool,
    },
    EffortFallback,
    EffortRequestRejected {
        rpc_code: i64,
    },
    EffortWalkdownExhausted {
        rpc_code: i64,
    },
    EffortWalkdownRetry {
        rpc_code: i64,
    },
    InitializeFailed,
    AuthMethodMismatch {
        advertised_count: u16,
    },
    PreAuthenticated {
        advertised_count: u16,
    },
    SessionCreateFailed,
    DiscoverySessionCreateFailed,
    PromptFailed,
    WarmConfigNotAdvertised,
    WarmConfigRejected,
}

impl AcpTraceEvent {
    fn bounded_count(count: usize) -> u16 {
        u16::try_from(count).unwrap_or(u16::MAX)
    }

    fn emit(self) {
        match self {
            Self::ModelOptionsMissing => tracing::warn!(
                event = "acp.model_options_missing",
                "ACP lifecycle metadata"
            ),
            Self::EffortBelowMinimum { advertised_count } => tracing::warn!(
                event = "acp.effort_below_minimum",
                advertised_count,
                "ACP lifecycle metadata"
            ),
            Self::EffortOptionMissing => tracing::warn!(
                event = "acp.effort_option_missing",
                "ACP lifecycle metadata"
            ),
            Self::ConfigResolved {
                effort_applied,
                fell_back,
            } => tracing::info!(
                event = "acp.config_resolved",
                effort_applied,
                fell_back,
                "ACP lifecycle metadata"
            ),
            Self::EffortFallback => {
                tracing::warn!(event = "acp.effort_fallback", "ACP lifecycle metadata")
            }
            Self::EffortRequestRejected { rpc_code } => tracing::warn!(
                event = "acp.effort_request_rejected",
                rpc_code,
                "ACP lifecycle metadata"
            ),
            Self::EffortWalkdownExhausted { rpc_code } => tracing::warn!(
                event = "acp.effort_walkdown_exhausted",
                rpc_code,
                "ACP lifecycle metadata"
            ),
            Self::EffortWalkdownRetry { rpc_code } => tracing::warn!(
                event = "acp.effort_walkdown_retry",
                rpc_code,
                "ACP lifecycle metadata"
            ),
            Self::InitializeFailed => {
                tracing::warn!(event = "acp.initialize_failed", "ACP lifecycle metadata")
            }
            Self::AuthMethodMismatch { advertised_count } => tracing::warn!(
                event = "acp.auth_method_mismatch",
                advertised_count,
                "ACP lifecycle metadata"
            ),
            Self::PreAuthenticated { advertised_count } => tracing::debug!(
                event = "acp.pre_authenticated",
                advertised_count,
                "ACP lifecycle metadata"
            ),
            Self::SessionCreateFailed => tracing::warn!(
                event = "acp.session_create_failed",
                "ACP lifecycle metadata"
            ),
            Self::DiscoverySessionCreateFailed => tracing::warn!(
                event = "acp.discovery_session_create_failed",
                "ACP lifecycle metadata"
            ),
            Self::PromptFailed => {
                tracing::warn!(event = "acp.prompt_failed", "ACP lifecycle metadata")
            }
            Self::WarmConfigNotAdvertised => tracing::warn!(
                event = "acp.warm_config_not_advertised",
                "ACP lifecycle metadata"
            ),
            Self::WarmConfigRejected => {
                tracing::warn!(event = "acp.warm_config_rejected", "ACP lifecycle metadata")
            }
        }
    }
}

/// Static configuration for an ACP agent connection.
///
/// `mode` drives a HARD `session/set_mode` after each `session/new` (a rejected
/// mode fails session setup); `model` validates and applies the advertised model
/// config option (a bad pin fails session setup).
/// `auth_method` optionally pins which advertised auth method `connect` uses;
/// `pre_authenticated` skips that request when credentials are already ambient.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Registry id of the agent this config belongs to, used in operator-facing diagnostics.
    pub agent_id: String,
    /// Absolute working directory the agent runs sessions in.
    pub cwd: PathBuf,
    /// Optional model id to request via advertised `session/set_config_option`.
    pub model: Option<String>,
    /// Optional mode id to request via `session/set_mode` (hard error if rejected).
    pub mode: Option<String>,
    /// Optional auth-method id to use for `authenticate`. When `None`, `connect`
    /// prefers ChatGPT-style methods (`chat-gpt`, then legacy `chatgpt`) and
    /// otherwise uses the first method the agent advertised at `initialize` (if any).
    pub auth_method: Option<String>,
    /// Skip the ACP `authenticate` request because the launched agent already has
    /// ambient credentials (for example, a mounted Codex `auth.json`). Mutually
    /// exclusive with `auth_method`.
    pub pre_authenticated: bool,
    /// Bound on the `initialize` handshake (transport connect + response).
    /// Defaults to [`DEFAULT_HANDSHAKE_TIMEOUT`]; on elapse `connect`/`spawn`
    /// return `BridgeError::AgentCrashed` rather than hanging. Task 6 surfaces
    /// this as a clear handshake-timeout error to the caller.
    pub handshake_timeout: std::time::Duration,
    /// Bound on how long a `cancel` (or an early stream-drop) waits for the
    /// agent to honor `session/cancel` and return its terminal result before we
    /// escalate by terminating the agent process. Defaults to
    /// [`DEFAULT_CANCEL_GRACE`]; tests override it to a short value to assert the
    /// hung-agent escalation deterministically without hanging the suite.
    pub cancel_grace: std::time::Duration,
    /// Reaper for a containerized agent's `docker run` container (`:ro` sandbox). `None` for local-process
    /// and in-process (test) backends — reaping is then a no-op.
    pub container: Option<ContainerReap>,
    /// MCP servers to offer via the ACP `session/new` `mcpServers` param (ADR-0028). Populated ONLY for
    /// `McpDelivery::Acp` agents (claude); `{cwd}` in args/env is substituted per session at mint.
    /// Codex/kiro native delivery leaves this empty (they ignore the param).
    pub mcp: Vec<bridge_core::mcp::McpServerSpec>,
    /// Optional per-turn watchdog. `None` disables watchdog behavior.
    pub watchdog: Option<bridge_core::domain::WatchdogConfig>,
    /// Exact credential values the bridge already possesses and must remove
    /// from lifecycle causes and process stderr before either is retained.
    /// `Debug` reports only a count; values are never rendered.
    pub diagnostic_redactor: DiagnosticRedactor,
}

/// Reaper configuration for a containerized (`:ro` sandbox) agent. The public
/// fields intentionally retain the original source-compatible literal shape;
/// [`AcpBackend`] converts legacy injections to a private shared controller.
#[derive(Clone)]
pub struct ContainerReap {
    pub runtime: String,
    pub name: String,
    pub reap_fn: ReapFn,
}

impl ContainerReap {
    /// Compatibility adapter for existing injected fire-and-forget reapers.
    pub fn from_legacy(
        runtime: impl Into<String>,
        name: impl Into<String>,
        reap_fn: ReapFn,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            name: name.into(),
            reap_fn,
        }
    }

    /// Source-compatible production configuration. The bridge binary supplies
    /// a typed [`ReapController`] alongside this value when it needs to report
    /// the bounded runtime result; external legacy callers keep best-effort
    /// behavior through `reap_fn`.
    pub fn production(runtime: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            runtime: runtime.into(),
            name: name.into(),
            reap_fn: production_reap_fn(),
        }
    }

    /// Build the typed production flight for bridge-owned spawn paths.
    #[doc(hidden)]
    pub fn production_controller(&self) -> ReapController {
        ReapController::production(self.runtime.clone(), self.name.clone())
    }

    fn legacy_controller(&self) -> ReapController {
        ReapController::from_legacy(
            self.runtime.clone(),
            self.name.clone(),
            Arc::clone(&self.reap_fn),
        )
    }
}

impl std::fmt::Debug for ContainerReap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerReap")
            .field("runtime", &self.runtime)
            .field("name", &self.name)
            .field("reap_fn", &"<fn>")
            .finish()
    }
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            agent_id: String::new(),
            cwd: PathBuf::from("."),
            model: None,
            mode: None,
            auth_method: None,
            pre_authenticated: false,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            cancel_grace: DEFAULT_CANCEL_GRACE,
            container: None,
            mcp: Vec::new(),
            watchdog: None,
            diagnostic_redactor: DiagnosticRedactor::default(),
        }
    }
}

// ── Default permission policy ────────────────────────────────────────────────
//
// Reverse `session/request_permission` requests are decided by a `PolicyEngine`
// (injected — see `AcpBackend::policy` / `with_policy`). The deployed 3a policy
// is auto-approve. So the backend defaults to this internal auto-approve impl
// when no policy is injected, keeping every existing `connect`/`spawn`/`from_child`
// call site (main + the e2es) source-compatible. `with_policy` overrides it.
//
// `decide` is the `bridge_core::ports::PolicyEngine` SYNC contract: `Approve`
// means grant; the auto-approver never denies. (A real `bridge_policy::AutoPolicy`
// denies INTERACTIVE asks; here we model the agent tool-call permission as
// non-interactive auto-grant, which Task 6's `main` can override by threading a
// concrete `PolicyEngine` through `with_policy`.)
struct AutoApprovePolicy;

impl PolicyEngine for AutoApprovePolicy {
    fn decide(
        &self,
        _req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

/// Register `method_not_found` reject handlers for a set of UNSUPPORTED inbound
/// request types onto a `Client` connection builder, returning the extended
/// builder. See the `fs/terminal UNSUPPORTED` note in [`AcpBackend::connect`] for
/// WHY explicit handlers are required (this SDK does NOT auto-reply to an
/// unregistered method — it drops it, hanging the agent's `block_task`).
///
/// Each generated handler is trivial + synchronous (responds immediately, no
/// await), so it never stalls the dispatch loop. Returning the `respond` error
/// (if the peer is gone) up out of the handler is fine: a lost reply to an
/// unsupported request is not a connection-fatal condition the SDK escalates here.
macro_rules! reject_unsupported {
    ($builder:expr; $( $req:ty => $resp:ty ),+ $(,)?) => {{
        let b = $builder;
        $(
            let b = b.on_receive_request(
                move |_req: $req,
                      responder: agent_client_protocol::Responder<$resp>,
                      _cx: ConnectionTo<Agent>| async move {
                    responder.respond_with_error(agent_client_protocol::Error::method_not_found())
                },
                agent_client_protocol::on_receive_request!(),
            );
        )+
        b
    }};
}

// ── Streaming routing registry ───────────────────────────────────────────────
//
// `session/update` notifications are delivered by the SDK on its event-loop
// task: a notification handler registered on the `Client.builder()`. That
// handler runs INSIDE the loop, so it MUST NOT call `cx` / block — it may only
// forward. We route each turn's chunks to its driver via an mpsc keyed by the
// agent session id.
//
// The lock is a plain `std::sync::Mutex` (NOT the async `tokio::Mutex`): the
// handler only does a `get` + non-blocking `send` under it, never awaits while
// holding it, so a non-async lock is correct and avoids `.await` in the handler.
type UpdateSender = mpsc::UnboundedSender<TurnEvent>;
type UpdateRegistry = Arc<StdMutex<HashMap<AgentSessionId, TurnRoute>>>;

struct TurnRoute {
    tx: UpdateSender,
    watch: Option<Arc<TurnWatch>>,
    // Task 6 consumes this from the reverse permission handler; Task 5 only
    // threads it onto the live route.
    #[allow(dead_code)]
    turn_meta: Option<TurnMeta>,
    cancelled: Arc<AtomicBool>,
    /// True only while the accepted agent turn can still require escalation.
    /// The terminal driver and cancel watcher race on this bit so a slow
    /// post-response observer cannot be mistaken for live agent work.
    active: Arc<AtomicBool>,
}

struct TurnWatch {
    turn_start: Instant,
    last_activity_ms: AtomicU64,
}

fn bump_activity(w: &TurnWatch) {
    w.last_activity_ms.store(
        w.turn_start.elapsed().as_millis() as u64 + 1,
        Ordering::Relaxed,
    );
}

/// What the notification handler forwards to a turn's driver/stream. Kept
/// minimal: only the variants the bridge models today. Unmodeled
/// `SessionUpdate` variants are dropped by the handler (tolerant reader).
enum TurnEvent {
    /// A streamed chunk of the agent's textual response.
    Text(String),
    /// A streamed context-window usage snapshot (ACP `usage_update`). Non-terminal,
    /// routed exactly like `Text`. [Slice 2]
    Usage(bridge_core::orch::UsageSnapshot),
    /// A rich ACP update routed to the side-channel sink by the stream driver.
    /// This never surfaces as a public [`Update`].
    Rich(bridge_core::orch::OrchEventKind),
    /// The terminal turn result. Pushed by the driver after the `PromptResponse`
    /// arrives, carrying the mapped `Update::Done`. Always the last event on a
    /// turn that the agent COMPLETED (incl. a real `StopReason::Cancelled`).
    Done(Update),
    /// A terminal turn FAILURE: `session/prompt` returned `Err` (agent crash /
    /// transport failure mid-turn). The `unfold` stream maps this to a terminal
    /// `Err` item, so a crash surfaces to the A2A caller as `Failed` — never the
    /// silent `Done{"unknown"}` that downstream reads as a clean `Completed`.
    Failed(BridgeError),
}

enum PromptDriverFailure {
    Sdk(AcpError),
    KillSwitch,
    Watchdog(Option<String>),
    DroppedStreamTimeout,
}

enum CancelSettle<T, E> {
    Prompt(Result<T, E>),
    KillSwitch,
    GraceElapsed,
}

async fn settle_prompt_after_cancel<F, T, E>(
    prompt_fut: std::pin::Pin<&mut F>,
    kill: &tokio::sync::Notify,
    grace: std::time::Duration,
) -> CancelSettle<T, E>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    tokio::select! {
        // If an SDK terminal and a local deadline are both ready in the same
        // poll, preserve the SDK result and its deeper cause deterministically.
        biased;
        outcome = prompt_fut => CancelSettle::Prompt(outcome),
        _ = kill.notified() => CancelSettle::KillSwitch,
        _ = tokio::time::sleep(grace) => CancelSettle::GraceElapsed,
    }
}

// ── SDK connection handle ────────────────────────────────────────────────────
//
// The connection's event loop (`connect_with`) owns a single task that runs
// until the connection closes, so we cannot keep the loop "in line". Instead
// `connect`/`spawn` start the loop in a dedicated tokio task whose `main_fn`
// publishes a clone of the `ConnectionTo<Agent>` handle out through a oneshot,
// then parks until shutdown. All agent-bound requests go through that cloned
// `cx`. (Driving the loop via a command channel is the alternative; a shared
// `cx` is simpler and is what Tasks 2–6 build prompt/cancel on top of.)
struct AcpConn {
    cx: ConnectionTo<Agent>,
    /// Negotiated agent capabilities from `initialize`.
    agent_capabilities: AgentCapabilities,
    /// Authentication methods the agent advertised (drives `authenticate`).
    auth_methods: Vec<AuthMethod>,
    /// Held for the backend's lifetime: the event-loop task parks on the paired
    /// receiver, so dropping this (on backend drop) closes the connection and
    /// lets the loop task exit cleanly. Tasks 2+ may signal it explicitly to
    /// drive shutdown / terminate.
    _shutdown: oneshot::Sender<()>,
    /// Per-turn chunk routing: agent session id → the `Sender` for the turn
    /// currently streaming on that session. Shared with the notification handler
    /// closure registered in `connect`. `prompt` registers a `Sender` here
    /// BEFORE sending `session/prompt` and removes it once the turn ends.
    updates: UpdateRegistry,
}

// ── Per-bridge-session agent state ───────────────────────────────────────────
//
// The bridge multiplexes many bridge sessions (keyed by `bridge_core` `SessionId`)
// over one ACP connection. Each bridge session maps to exactly one agent-minted
// `session/new` session, created LAZILY on first use and reused thereafter.
//
// `AgentSession` is the per-bridge-session state Tasks 2–4 build on:
//   * `agent_id`  — a `OnceCell` so concurrent first-prompts mint the agent
//                   session EXACTLY ONCE; the init future runs once and every
//                   concurrent caller awaits the same result (no double-mint).
//   * `turn_lock` — serializes prompt turns for this session [Cx-M2]: a second
//                   prompt waits here until the in-flight turn releases, so turns
//                   never interleave on one agent session.
//   * `cancel_requested` — the cancel LATCH [Cx-M2]: a `cancel` that races ahead
//                   of `session/new` sets this; the minting task observes it the
//                   instant the id exists and fires `session/cancel` so the
//                   cancel is never dropped.
struct AgentSession {
    /// The agent-minted session id, set exactly once by the `session/new` that
    /// `ensure_session` drives. `OnceCell` guarantees single init under races.
    agent_id: OnceCell<AgentSessionId>,
    /// Single-flight owner for initialization. The owner moves into a spawned
    /// task, so cancelling the caller that first requested a session cannot
    /// cancel an already-started `session/new`/configuration lifecycle and
    /// leave a durable `Started` without a terminal event.
    init_lock: Arc<Mutex<()>>,
    /// The cwd string that was ACTUALLY passed to `session/new` when this session
    /// was minted. Set by the same shielded single-flight task as `agent_id`
    /// (so it is always set iff the session exists) and used by the immutability
    /// guard: if a later `configure_session` stashes a DIFFERENT cwd for an
    /// already-minted session, `ensure_session` returns `InvalidStateTransition`
    /// rather than silently re-using the old cwd. ACP §11A: a session's cwd is
    /// fixed at `session/new` time.
    minted_cwd: OnceCell<String>,
    /// Cwd selected by the one active mint attempt. `init_lock` makes this
    /// single-valued; the initializer clears it on every exit, while successful
    /// minting transfers durable ownership to `minted_cwd`.
    active_mint_cwd: StdMutex<Option<String>>,
    /// Per-session turn lock. Held for the duration of a prompt turn so turns on
    /// one agent session run strictly sequentially. `Arc<Mutex<()>>` (not a bare
    /// field) so `prompt` can take an OWNED guard (`lock_owned`) and move it into
    /// the driver task that holds it for the whole streamed turn.
    turn_lock: Arc<Mutex<()>>,
    /// Operation-scoped accepted-work evidence for the turn that owns
    /// `turn_lock`. Routing ends as soon as the SDK result is terminal, but
    /// completion observation can still be in flight; this slot deliberately
    /// outlives the routing entry and is cleared before the turn lock releases.
    turn_accepted: Arc<StdMutex<Option<Arc<AtomicBool>>>>,
    /// The advertised config surface from `session/new`, refreshed by later
    /// `session/set_config_option` calls so warm config reconcile can reuse it.
    config_surface: StdMutex<Option<ConfigSurface>>,
    /// Cancel latch: set by `request_cancel` when a cancel arrives before the
    /// agent session exists, so the minting task can fire `session/cancel` as
    /// soon as the id is known.
    cancel_requested: AtomicBool,
    /// Per-turn KILL SWITCH the grace escalation fires to unblock a hung driver.
    /// `prompt` installs a FRESH `Notify` here for each turn and hands the driver
    /// a clone; the driver `select!`s on it so that, when a cancelled turn does
    /// not complete within grace, the cancel watcher (or the driver's own
    /// drop-path) can notify it to abandon its `send_request` await — surfacing a
    /// terminal `Err`, releasing the lock, and ending the caller's stream even if
    /// the agent never answers. `None` between turns. (Alongside this we also
    /// `terminate()` a real `Supervised` child so a runaway agent PROCESS is
    /// actually killed; the kill switch is what makes the in-process transport —
    /// which has no process to kill — unblock deterministically too.)
    turn_kill: Arc<StdMutex<Option<Arc<tokio::sync::Notify>>>>,
}

impl AgentSession {
    fn new() -> Self {
        Self {
            agent_id: OnceCell::new(),
            init_lock: Arc::new(Mutex::new(())),
            minted_cwd: OnceCell::new(),
            active_mint_cwd: StdMutex::new(None),
            turn_lock: Arc::new(Mutex::new(())),
            turn_accepted: Arc::new(StdMutex::new(None)),
            config_surface: StdMutex::new(None),
            cancel_requested: AtomicBool::new(false),
            turn_kill: Arc::new(StdMutex::new(None)),
        }
    }
}

struct ActiveMintCwdGuard(Arc<AgentSession>);

impl Drop for ActiveMintCwdGuard {
    fn drop(&mut self) {
        if let Ok(mut active_cwd) = self.0.active_mint_cwd.lock() {
            *active_cwd = None;
        }
    }
}

struct MintProcessEvidenceGuard {
    stderr_ring: Option<ProcessStderrRing>,
    credential_delivery_uncertain: bool,
}

impl MintProcessEvidenceGuard {
    fn new(stderr_ring: Option<ProcessStderrRing>) -> Self {
        Self {
            stderr_ring,
            credential_delivery_uncertain: false,
        }
    }

    fn arm(&mut self) {
        self.credential_delivery_uncertain = true;
    }

    fn commit(&mut self) {
        self.credential_delivery_uncertain = false;
    }
}

impl Drop for MintProcessEvidenceGuard {
    fn drop(&mut self) {
        if self.credential_delivery_uncertain {
            if let Some(stderr_ring) = self.stderr_ring.as_ref() {
                stderr_ring.retain_metadata_only();
            }
        }
    }
}

struct TurnAcceptanceGuard {
    slot: Arc<StdMutex<Option<Arc<AtomicBool>>>>,
    accepted: Arc<AtomicBool>,
}

impl TurnAcceptanceGuard {
    fn install(slot: Arc<StdMutex<Option<Arc<AtomicBool>>>>, accepted: Arc<AtomicBool>) -> Self {
        *slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&accepted));
        Self { slot, accepted }
    }
}

impl Drop for TurnAcceptanceGuard {
    fn drop(&mut self) {
        let mut slot = self
            .slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot
            .as_ref()
            .is_some_and(|accepted| Arc::ptr_eq(accepted, &self.accepted))
        {
            *slot = None;
        }
    }
}

#[derive(Clone, Default)]
struct ConfigSurface {
    opts: Vec<SessionConfigOption>,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum ApplyPurpose {
    Mint,
    Warm,
}

enum ApplyConfigError {
    NotAdvertised(BridgeError),
    Rejected(BridgeError),
}

#[derive(Debug)]
struct TeardownFailure {
    error: BridgeError,
    prompt_may_have_been_accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CancelDispatch {
    Delivered,
    Latched,
}

#[derive(Clone)]
struct SessionConfigSnapshot {
    desired_cwd: String,
    config: EffectiveConfig,
}

// ── Public struct ────────────────────────────────────────────────────────────

pub struct AcpBackend {
    /// SDK connection handle. Always present (all constructors build the SDK
    /// connection); `Option` only so the `cx()`/`updates()` accessors have a
    /// clean error seam if a future constructor ever leaves it unset.
    conn: Option<AcpConn>,
    /// The spawned `Supervised` child, held for the whole backend lifetime so
    /// `kill_on_drop(true)` does not SIGKILL it the instant `spawn` returns.
    /// `Some` on the `spawn`/`from_child` paths (we own the child); `None` on
    /// `connect` (in-process transport).
    ///
    /// Behind an `Arc<StdMutex<Option<_>>>` because the cancel grace-watcher and
    /// the driver's early-drop path escalate by TAKING the child out and
    /// `terminate()`-ing it — and `terminate(self, _)` consumes `Supervised`,
    /// which a `&self` method cannot move out of a plain field. The shared,
    /// take-once handle lets either escalation path claim the child exactly once
    /// (the loser sees `None`), and the backend still drops it on `Drop` if no
    /// escalation ever fired.
    supervised: Arc<StdMutex<Option<Supervised>>>,
    /// Process-scoped stderr evidence retained independently from the child
    /// owner so prompt attempts can snapshot a cursor without retaining the
    /// operation observer on the cached backend.
    stderr_ring: Option<ProcessStderrRing>,
    /// Static config (cwd for `session/new`, model/mode for later tasks).
    config: Option<AcpConfig>,
    /// One private, joinable container-removal flight. Public `AcpConfig`
    /// retains its legacy literal shape; production may inject a typed
    /// controller while legacy callers are adapted from `ContainerReap`.
    container_reap: Option<ReapController>,
    /// Idempotency flag for the `:ro` container reaper (shared across the teardown sites: cancel-escalate,
    /// retire, Drop). Always present; reaping is a no-op when `config.container` is `None`.
    reaped: Arc<AtomicBool>,
    /// Set when bounded cancellation escalates the shared connection. The real
    /// process is terminated; the flag also fences the in-process transport so
    /// no later prompt can overlap work from an agent that ignored cancel.
    unavailable: Arc<AtomicBool>,
    /// Orders the final availability check plus SDK request installation
    /// against connection-wide escalation and retirement. No await occurs
    /// while held.
    dispatch_gate: Arc<StdMutex<()>>,
    /// bridge-session-key → per-session agent state. The map itself is behind a
    /// `Mutex` held ONLY long enough to look up / insert the `Arc<AgentSession>`;
    /// it is dropped before any `session/new` await so the mint of one session
    /// never blocks lookups of another.
    sessions: Arc<Mutex<HashMap<SessionId, Arc<AgentSession>>>>,
    /// Per-bridge-session spec stash (Increment 3b/session-cwd). Dispatch (T10)
    /// calls [`Self::configure_session`] to insert the `SessionSpec` (effective
    /// `model`/`effort`/`mode` + `cwd`) for a session BEFORE its first prompt;
    /// [`Self::ensure_session`] reads THIS map (keyed by the bridge `SessionId`) at
    /// lazy mint and applies the values, so a live registry config edit affects new
    /// sessions with no respawn and per-request overrides stay isolated per session.
    /// Behind a plain `std::sync::Mutex` (only short, non-async insert/lookup/remove
    /// under it).
    ///
    /// FALLBACK: a session with NO stash entry (direct callers / the gated e2es that
    /// don't go through `configure_session`) falls back to [`AcpConfig`]'s static
    /// `model`/`mode`, preserving the pre-3b behavior.
    session_cfg: Arc<StdMutex<HashMap<SessionId, SessionSpec>>>,
    /// Per-turn metadata stashed by the A2A producer immediately before the next
    /// prompt. `prompt_inner` takes it at entry, before lazy session minting, so
    /// early setup errors cannot leave stale metadata for a later turn.
    pending_turn_meta: StdMutex<HashMap<SessionId, TurnMeta>>,
    /// Policy engine that decides reverse `session/request_permission` requests.
    /// Defaults to an internal auto-approve impl (the deployed 3a policy); a
    /// caller (Task 6's `main`) threads a concrete engine via [`Self::with_policy`].
    ///
    /// Behind `Arc<StdMutex<Arc<dyn PolicyEngine>>>` so the SAME handle is shared
    /// with the permission handler registered inside [`Self::connect`]'s event-loop
    /// task, yet [`Self::with_policy`] — which runs AFTER `connect` returns — can
    /// still SWAP the engine the already-registered handler reads. The handler
    /// clones the inner `Arc` out under the lock (no await held), so swapping never
    /// races a decision.
    policy: PolicyHandle,
    /// Shared registry used by deferred interactive permission requests. This is
    /// a swappable handle for the same reason as `policy`: `connect` registers the
    /// ACP handler before builder-style injection can run.
    permission_registry: PermissionRegistryHandle,
    /// Bounded operator-decision wait for deferred permissions.
    perm_timeout_ms: Arc<AtomicU64>,
    #[cfg(test)]
    prompt_snapshot_hook: StdMutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    before_process_redactor_hook: StdMutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    fail_deferred_cancel_send: Arc<AtomicBool>,
    #[cfg(test)]
    fail_cancel_send: Arc<AtomicBool>,
}

/// Shared, swappable handle to the active [`PolicyEngine`]. See [`AcpBackend::policy`].
type PolicyHandle = Arc<StdMutex<Arc<dyn PolicyEngine>>>;
type PermissionRegistryHandle = Arc<StdMutex<Option<Arc<PermissionRegistry>>>>;

impl AcpBackend {
    fn install_prompt_request<T>(
        dispatch_gate: &Arc<StdMutex<()>>,
        unavailable: &Arc<AtomicBool>,
        accepted: &Arc<AtomicBool>,
        install: impl FnOnce() -> T,
    ) -> Option<T> {
        let _dispatch = dispatch_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if unavailable.load(Ordering::SeqCst) {
            None
        } else {
            let installed = install();
            accepted.store(true, Ordering::SeqCst);
            Some(installed)
        }
    }

    #[cfg(test)]
    fn turn_prompt_accepted(entry: &AgentSession) -> bool {
        Self::turn_prompt_accepted_handle(entry)
            .is_some_and(|accepted| accepted.load(Ordering::SeqCst))
    }

    fn turn_prompt_accepted_handle(entry: &AgentSession) -> Option<Arc<AtomicBool>> {
        entry
            .turn_accepted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn session_config_snapshot(
        &self,
        key: &SessionId,
    ) -> Result<SessionConfigSnapshot, BridgeError> {
        let Some(config) = self.config.as_ref() else {
            return Err(BridgeError::agent_crashed(
                "no config available for ACP session",
            ));
        };
        let stashed = self
            .session_cfg
            .lock()
            .ok()
            .and_then(|configs| configs.get(key).cloned());
        let desired_cwd = stashed
            .as_ref()
            .and_then(|spec| spec.cwd.as_ref())
            .map(|cwd| cwd.as_str().to_owned())
            .unwrap_or_else(|| config.cwd.to_string_lossy().into_owned());
        let effective_config = stashed.map_or_else(
            || EffectiveConfig {
                model: config.model.clone(),
                effort: None,
                mode: config.mode.clone(),
            },
            |spec| spec.config,
        );
        Ok(SessionConfigSnapshot {
            desired_cwd,
            config: effective_config,
        })
    }

    fn diagnostic_redactor_for_cwds(&self, cwds: &[String]) -> DiagnosticRedactor {
        let Some(config) = self.config.as_ref() else {
            return DiagnosticRedactor::default();
        };
        let values = cwds
            .iter()
            .flat_map(|cwd| bridge_core::mcp::env_redaction_values(&config.mcp, cwd).into_iter());
        config.diagnostic_redactor.clone().with_known_values(values)
    }

    async fn diagnostic_redactor_for_operation(&self, key: &SessionId) -> DiagnosticRedactor {
        let mut cwds = self
            .session_config_snapshot(key)
            .map(|snapshot| vec![snapshot.desired_cwd])
            .unwrap_or_default();
        let entry = self.sessions.lock().await.get(key).cloned();
        if let Some(entry) = entry {
            if let Ok(active) = entry.active_mint_cwd.lock() {
                cwds.extend(active.iter().cloned());
            }
            if let Some(minted) = entry.minted_cwd.get() {
                cwds.push(minted.clone());
            }
        }
        self.diagnostic_redactor_for_cwds(&cwds)
    }

    async fn apply_process_redactor_for_live_sessions(&self, extra_cwd: Option<String>) {
        let Some(stderr_ring) = self.stderr_ring.as_ref() else {
            return;
        };
        // Keep collection and replacement under the map lock. A concurrent mint
        // can publish its active cwd only before or after this critical section,
        // and must run this same method before sending its own session/new. That
        // prevents an older snapshot from overwriting a newer union.
        let sessions = self.sessions.lock().await;
        let mut cwds = Vec::with_capacity(sessions.len().saturating_mul(2).saturating_add(1));
        for entry in sessions.values() {
            if let Ok(active) = entry.active_mint_cwd.lock() {
                cwds.extend(active.iter().cloned());
            }
            if let Some(minted) = entry.minted_cwd.get() {
                cwds.push(minted.clone());
            }
        }
        cwds.extend(extra_cwd);
        stderr_ring.apply_redactor(self.diagnostic_redactor_for_cwds(&cwds));
    }

    fn send_cancel_under_dispatch_fence<E>(
        dispatch_gate: &Arc<StdMutex<()>>,
        unavailable: &Arc<AtomicBool>,
        accepted: impl FnOnce() -> Option<Arc<AtomicBool>>,
        send: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), TeardownFailure>
    where
        E: std::fmt::Display,
    {
        // Route lookup and notification installation share the same short,
        // non-awaiting gate as prompt installation. If cancellation wins while
        // no route is published, a failed send closes the fence before a later
        // prompt can cross the accepted-work barrier. If prompt installation
        // won, its accepted store is visible before the failure sample.
        let _dispatch = dispatch_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let accepted = accepted();
        send().map_err(|cause| {
            unavailable.store(true, Ordering::SeqCst);
            TeardownFailure {
                error: BridgeError::agent_crashed(format!(
                    "failed to send session/cancel notification: {cause}"
                )),
                prompt_may_have_been_accepted: accepted
                    .is_some_and(|accepted| accepted.load(Ordering::SeqCst)),
            }
        })
    }

    async fn deferred_cancel_delivery_failure(
        lifecycle: &AcpLifecycle,
        cause: String,
    ) -> BridgeError {
        if let Err(error) = lifecycle
            .record(
                DiagnosticPhase::PromptStart,
                PhaseStatus::Started,
                None,
                Some("acp.prompt_start.deferred_cancel"),
                None,
            )
            .await
        {
            return error;
        }
        lifecycle
            .failure(
                DiagnosticPhase::PromptStart,
                Some(DiagnosticPhase::ConfigApply),
                DiagnosticFailureClass::Transport,
                FailureDisposition::Fatal,
                "acp.prompt_start.deferred_cancel_failed",
                "ACP deferred cancellation could not be delivered before prompt dispatch",
                Some(cause),
                false,
                None,
                None,
                None,
            )
            .await
    }

    async fn require_prompt_connection(&self, lifecycle: &AcpLifecycle) -> Result<(), BridgeError> {
        if !self.unavailable.load(Ordering::SeqCst) {
            return Ok(());
        }
        lifecycle
            .record(
                DiagnosticPhase::PromptStart,
                PhaseStatus::Started,
                None,
                None,
                None,
            )
            .await?;
        Err(lifecycle
            .failure(
                DiagnosticPhase::PromptStart,
                Some(DiagnosticPhase::ConfigApply),
                DiagnosticFailureClass::AgentProcess,
                FailureDisposition::Fatal,
                "acp.prompt_start.connection_terminated",
                "ACP connection was terminated by a prior lifecycle escalation",
                None,
                false,
                None,
                None,
                None,
            )
            .await)
    }

    /// Build the `initialize` request this backend sends to the agent.
    ///
    /// Exposed so the wire-golden test can assert the serialized frame is
    /// conformant (integer `protocolVersion`, no fs/terminal capabilities)
    /// against the SAME value the connection actually transmits.
    #[must_use]
    pub fn initialize_request() -> InitializeRequest {
        // `InitializeRequest::new` defaults `client_capabilities` to
        // `ClientCapabilities::default()`, which advertises no fs read/write and
        // no terminal support — exactly what we want (no fs/terminal seam).
        InitializeRequest::new(ProtocolVersion::V1)
    }

    /// Build the `session/new` request this backend sends to mint an agent
    /// session for a bridge session. `cwd` MUST be an absolute path (ACP §11A);
    /// `mcpServers` is sent as an explicit array (empty when no MCP servers),
    /// never omitted. `mcp` specs have their `{cwd}` args/env substituted with
    /// this session's `cwd` and are converted to `McpServer::Stdio` (ADR-0028).
    ///
    /// Exposed so the wire-golden test can assert the serialized `params` shape
    /// against the SAME value `ensure_session` transmits — not a re-derivation
    /// of the SDK type.
    #[must_use]
    pub fn new_session_request(
        cwd: impl Into<PathBuf>,
        mcp: &[bridge_core::mcp::McpServerSpec],
    ) -> NewSessionRequest {
        let cwd = cwd.into();
        let cwd_str = cwd.to_string_lossy().into_owned();
        let servers: Vec<McpServer> = mcp
            .iter()
            .map(|spec| {
                let s = spec.substituted(&cwd_str);
                McpServer::Stdio(
                    McpServerStdio::new(s.name, s.command).args(s.args).env(
                        s.env
                            .into_iter()
                            .map(|(name, value)| EnvVariable::new(name, value))
                            .collect(),
                    ),
                )
            })
            .collect();
        NewSessionRequest::new(cwd).mcp_servers(servers)
    }

    /// Build the `session/prompt` request the backend sends for a turn: the
    /// agent session id plus each bridge `Part` mapped to a tagged text
    /// `ContentBlock`. ACP §11A: the wire field is `prompt` (an array of tagged
    /// content blocks), NOT `parts`.
    ///
    /// Exposed so the wire-golden test can assert the serialized `params` shape
    /// (`{"sessionId":<id>,"prompt":[{"type":"text","text":<t>}]}`) against the
    /// SAME value `prompt` transmits — not a re-derivation of the SDK type.
    #[must_use]
    pub fn prompt_request(
        agent_id: AgentSessionId,
        parts: &[bridge_core::domain::Part],
    ) -> PromptRequest {
        let blocks: Vec<ContentBlock> = parts
            .iter()
            .map(|p| ContentBlock::Text(TextContent::new(p.text.clone())))
            .collect();
        PromptRequest::new(agent_id, blocks)
    }

    /// Build the `session/cancel` NOTIFICATION this backend sends to cancel an
    /// in-flight turn (via `request_cancel` / the cancel latch). ACP §11A:
    /// `session/cancel` is a NOTIFICATION (no `id`, no response), with
    /// `params:{ "sessionId": <agent id> }`.
    ///
    /// Exposed so the wire-golden test can assert the serialized `params` shape
    /// (`{"sessionId":<id>}`) and notification-shape against the SAME value the
    /// backend transmits — not a re-derivation of the SDK type.
    #[must_use]
    pub fn cancel_notification(agent_id: AgentSessionId) -> CancelNotification {
        CancelNotification::new(agent_id)
    }

    /// Build the `session/set_mode` request the backend sends after `session/new`
    /// when [`AcpConfig::mode`] is set. ACP §11A: `params:{ "sessionId":<agent id>,
    /// "modeId":<mode> }`, method `session/set_mode` (snake_case).
    ///
    /// Exposed so the wire-golden test can assert the serialized `params` shape
    /// against the SAME value `ensure_session` transmits — not a re-derivation of
    /// the SDK type.
    #[must_use]
    pub fn set_mode_request(
        agent_id: AgentSessionId,
        mode_id: impl Into<String>,
    ) -> SetSessionModeRequest {
        SetSessionModeRequest::new(agent_id, mode_id.into())
    }

    /// Build a `session/set_config_option` request for the advertised config option.
    #[must_use]
    pub fn set_config_option_request(
        agent_id: AgentSessionId,
        config_id: impl Into<String>,
        value: impl Into<String>,
    ) -> SetSessionConfigOptionRequest {
        SetSessionConfigOptionRequest::new(
            agent_id,
            SessionConfigId::new(config_id.into()),
            SessionConfigValueId::new(value.into()),
        )
    }

    async fn set_config_option(
        cx: &ConnectionTo<Agent>,
        agent_id: &AgentSessionId,
        config_id: &str,
        value: &str,
    ) -> Result<Vec<SessionConfigOption>, agent_client_protocol::Error> {
        Ok(cx
            .send_request::<SetSessionConfigOptionRequest>(Self::set_config_option_request(
                agent_id.clone(),
                config_id,
                value,
            ))
            .block_task()
            .await?
            .config_options)
    }

    fn error_code_i64(code: ErrorCode) -> i64 {
        match code {
            ErrorCode::ParseError => -32700,
            ErrorCode::InvalidRequest => -32600,
            ErrorCode::MethodNotFound => -32601,
            ErrorCode::InvalidParams => -32602,
            ErrorCode::InternalError => -32603,
            ErrorCode::AuthRequired => -32000,
            ErrorCode::ResourceNotFound => -32001,
            ErrorCode::Other(code) => i64::from(code),
            _ => 0,
        }
    }

    /// Resolve + apply the configured model against the ACP v1 `config_options`
    /// model selector, and return `(refreshed_options, current_model, applied_model)`.
    ///
    /// A configured model with no advertised model option is operator config drift
    /// → `config_invalid` (fatal mint), listing the advertised values when available.
    async fn configure_model_option(
        cx: &ConnectionTo<Agent>,
        agent_session_id: &AgentSessionId,
        agent_id: &str,
        opts0: &[SessionConfigOption],
        model: Option<&str>,
    ) -> Result<(Vec<SessionConfigOption>, Option<String>, Option<String>), BridgeError> {
        if let Some((config_id, current, values)) = model_values(opts0) {
            return match Self::resolve_model_or_invalid(agent_id, model, &values)? {
                ModelDecision::Default => {
                    if is_blocked_model(&current) {
                        return Err(BridgeError::config_invalid(format!(
                            "agent {agent_id} current model={current} is blocked by this bridge; configure a non-blocked model"
                        )));
                    }
                    Ok((opts0.to_vec(), Some(current), None))
                }
                ModelDecision::Apply(value) => {
                    let replacing_blocked_current = is_blocked_model(&current);
                    let refreshed =
                        Self::set_config_option(cx, agent_session_id, &config_id, &value)
                            .await
                            .map_err(|e| {
                                BridgeError::agent_crashed(format!(
                                    "session/set_config_option({config_id}) rejected: {e}"
                                ))
                            })?;
                    let opts = if refreshed.is_empty() {
                        AcpTraceEvent::ModelOptionsMissing.emit();
                        opts0.to_vec()
                    } else {
                        refreshed
                    };
                    let model_current = model_values(&opts).map(|(_, current, _)| current);
                    if replacing_blocked_current && model_current.as_deref() != Some(value.as_str())
                    {
                        return Err(BridgeError::config_invalid(format!(
                            "agent {agent_id} could not confirm model={value} replaced blocked current model={current}; current {}",
                            model_current.as_deref().unwrap_or("unknown")
                        )));
                    }
                    Ok((opts, model_current, Some(value)))
                }
            };
        }

        // No model option advertised: a pinned model is config drift; otherwise skip.
        if let Some(model) = model {
            return Err(BridgeError::config_invalid(format!(
                "agent {agent_id} advertised no model option but model={model} configured"
            )));
        }
        Ok((opts0.to_vec(), Some("unadvertised".to_string()), None))
    }

    /// Validate a configured model against the advertised values, mapping a miss to
    /// a fatal `config_invalid` that lists them (logged/CLI; redacted on the wire).
    fn resolve_model_or_invalid(
        agent_id: &str,
        model: Option<&str>,
        values: &[String],
    ) -> Result<ModelDecision, BridgeError> {
        resolve_model(model, values).map_err(|err| match err {
            ModelResolutionError::Blocked { want, valid } => BridgeError::config_invalid(format!(
                "agent {agent_id} model={want} is blocked by this bridge; valid models: {}",
                valid.join(", ")
            )),
            ModelResolutionError::NotAdvertised(err) => BridgeError::config_invalid(format!(
                "agent {agent_id} model={} is not advertised; valid models: {}",
                err.want,
                err.valid.join(", ")
            )),
        })
    }

    /// Apply model + effort against an advertised surface on a live agent session.
    /// This is the mint-time config sequence lifted so warm reconcile can call the
    /// same code without re-minting. Mint preserves today's permissive effort
    /// behavior; warm requires an exact requested effort apply.
    async fn apply_model_effort(
        cx: &ConnectionTo<Agent>,
        agent_session_id: &AgentSessionId,
        agent_id: &str,
        surface: &ConfigSurface,
        model: Option<&str>,
        effort: Option<Effort>,
        purpose: ApplyPurpose,
    ) -> Result<(ConfigSurface, String), ApplyConfigError> {
        let (mut refreshed_opts, model_current, applied_model) =
            Self::configure_model_option(cx, agent_session_id, agent_id, &surface.opts, model)
                .await
                .map_err(|err| match err {
                    err @ BridgeError::ConfigInvalid { .. } => ApplyConfigError::NotAdvertised(err),
                    err @ BridgeError::AgentCrashed { .. } => ApplyConfigError::Rejected(err),
                    err => ApplyConfigError::Rejected(err),
                })?;

        // PF-9: a WARM reconcile must apply the requested model EXACTLY. `configure_model_option`
        // can return Ok with a stale/unchanged `current` (e.g. empty refreshed opts) — at Warm
        // that is NOT an exact apply, so fail rather than let the fingerprint advance to a model
        // the live session may not be using. (Mint keeps today's lenient behavior.)
        if matches!(purpose, ApplyPurpose::Warm) {
            if let Some(want) = applied_model.as_deref() {
                if model_current.as_deref() != Some(want) {
                    return Err(ApplyConfigError::NotAdvertised(BridgeError::config_invalid(
                        format!(
                            "warm reconcile: model not applied exactly (requested {want}, current {})",
                            model_current.as_deref().unwrap_or("unknown")
                        ),
                    )));
                }
            }
        }

        let effort_outcome = match effort_opt(&refreshed_opts) {
            Some(advertised) => {
                let decision = resolve_effort(effort, &advertised);
                match decision {
                    EffortDecision::Unsupported { from } => {
                        AcpTraceEvent::EffortBelowMinimum {
                            advertised_count: AcpTraceEvent::bounded_count(advertised.levels.len()),
                        }
                        .emit();
                        EffortDecision::Unsupported { from }
                    }
                    EffortDecision::Skip => EffortDecision::Skip,
                    decision @ (EffortDecision::Apply { .. } | EffortDecision::FellBack { .. }) => {
                        match Self::apply_effort_walkdown(
                            cx,
                            agent_session_id,
                            agent_id,
                            decision,
                            &advertised.levels,
                        )
                        .await
                        {
                            Ok((decision, refreshed)) => {
                                if let Some(opts) = refreshed {
                                    refreshed_opts = opts;
                                }
                                decision
                            }
                            Err(err) => {
                                if matches!(purpose, ApplyPurpose::Warm) {
                                    return Err(ApplyConfigError::Rejected(err));
                                }
                                EffortDecision::Skip
                            }
                        }
                    }
                }
            }
            None => {
                if effort.is_some() {
                    AcpTraceEvent::EffortOptionMissing.emit();
                }
                EffortDecision::Skip
            }
        };
        let model_current_for_log = model_current.as_deref().unwrap_or("unknown");
        AcpTraceEvent::ConfigResolved {
            effort_applied: matches!(effort_outcome, EffortDecision::Apply { .. }),
            fell_back: matches!(effort_outcome, EffortDecision::FellBack { .. }),
        }
        .emit();

        if matches!(purpose, ApplyPurpose::Warm) && effort.is_some() {
            match &effort_outcome {
                EffortDecision::Apply { .. } => {}
                EffortDecision::Skip
                | EffortDecision::FellBack { .. }
                | EffortDecision::Unsupported { .. } => {
                    return Err(ApplyConfigError::NotAdvertised(
                        BridgeError::config_invalid(format!(
                            "agent {agent_id} did not apply requested effort exactly"
                        )),
                    ));
                }
            }
        }

        Ok((
            ConfigSurface {
                opts: refreshed_opts,
            },
            model_current_for_log.to_string(),
        ))
    }

    async fn apply_effort_walkdown(
        cx: &ConnectionTo<Agent>,
        agent_session_id: &AgentSessionId,
        _agent_id: &str,
        initial: EffortDecision,
        advertised_levels: &[String],
    ) -> Result<(EffortDecision, Option<Vec<SessionConfigOption>>), BridgeError> {
        let (config_id, requested_from, mut level) = match initial {
            EffortDecision::Apply { config_id, level } => (config_id, level.clone(), level),
            EffortDecision::FellBack {
                config_id,
                from,
                to,
            } => {
                AcpTraceEvent::EffortFallback.emit();
                (config_id, from, to)
            }
            EffortDecision::Skip => return Ok((EffortDecision::Skip, None)),
            EffortDecision::Unsupported { from } => {
                return Ok((EffortDecision::Unsupported { from }, None));
            }
        };

        loop {
            match Self::set_config_option(cx, agent_session_id, &config_id, &level).await {
                Ok(refreshed) => {
                    if level == requested_from {
                        return Ok((EffortDecision::Apply { config_id, level }, Some(refreshed)));
                    }
                    return Ok((
                        EffortDecision::FellBack {
                            config_id,
                            from: requested_from,
                            to: level,
                        },
                        Some(refreshed),
                    ));
                }
                Err(e) => {
                    let code = Self::error_code_i64(e.code);
                    if !is_unsupported_effort_error(code, &e.message, e.data.as_ref()) {
                        AcpTraceEvent::EffortRequestRejected { rpc_code: code }.emit();
                        return Err(BridgeError::agent_crashed(format!(
                            "session/set_config_option({config_id}) rejected: {e}"
                        )));
                    }

                    let Some(next) = Self::next_lower_effort(&level, advertised_levels) else {
                        AcpTraceEvent::EffortWalkdownExhausted { rpc_code: code }.emit();
                        return Ok((
                            EffortDecision::Unsupported {
                                from: requested_from,
                            },
                            None,
                        ));
                    };
                    AcpTraceEvent::EffortWalkdownRetry { rpc_code: code }.emit();
                    level = next;
                }
            }
        }
    }

    fn next_lower_effort(current: &str, advertised_levels: &[String]) -> Option<String> {
        let current_rank = EFFORT_ORDER.iter().position(|level| *level == current)?;
        EFFORT_ORDER[..current_rank]
            .iter()
            .rev()
            .find(|candidate| {
                advertised_levels
                    .iter()
                    .any(|level| level.as_str() == **candidate)
            })
            .map(|level| (*level).to_string())
    }

    /// **Production** constructor: spawn `cmd args` as a `Supervised` child
    /// (its own process group, tested SIGTERM→SIGKILL reaping) and drive the
    /// ACP connection over its stdin/stdout as `ByteStreams`.
    ///
    /// This is `Supervised` + `connect(ByteStreams)`: process lifecycle stays
    /// with `Supervised`; protocol drive is the shared `connect` core.
    pub async fn spawn(cmd: &str, args: &[&str], config: AcpConfig) -> Result<Self, BridgeError> {
        Self::spawn_observed(
            cmd,
            args,
            config,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    pub async fn spawn_observed(
        cmd: &str,
        args: &[&str],
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<Self, BridgeError> {
        Self::spawn_observed_with_container_controller(cmd, args, config, observer, None).await
    }

    /// Bridge-owned production seam for retaining the public legacy
    /// `ContainerReap` shape while joining a typed runtime-removal attempt.
    #[doc(hidden)]
    pub async fn spawn_observed_with_container_controller(
        cmd: &str,
        args: &[&str],
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
        container_controller: Option<ReapController>,
    ) -> Result<Self, BridgeError> {
        let redactor = config.diagnostic_redactor.clone();
        let lifecycle = AcpLifecycle::new(observer.clone(), redactor.clone(), None);
        lifecycle
            .record(
                DiagnosticPhase::Spawn,
                PhaseStatus::Started,
                None,
                None,
                None,
            )
            .await?;
        // Reaper handle for the SPAWN-FAILURE path (Site A): once `Supervised::spawn` succeeds the
        // `docker run` container is up, but if pipe-take or the handshake then fails there is no backend
        // to reap from — so reap here. Cloned before `config` moves into `connect`.
        let container_controller = container_controller.or_else(|| {
            config
                .container
                .as_ref()
                .map(ContainerReap::legacy_controller)
        });
        let container_on_fail = container_controller.clone();
        let mut supervised =
            match Supervised::spawn_with_stderr_redactor(cmd, args, None, redactor.clone()) {
                Ok(supervised) => supervised,
                Err(error) => {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::Spawn,
                            None,
                            DiagnosticFailureClass::AgentProcess,
                            FailureDisposition::RetrySameTarget,
                            "acp.spawn.failed",
                            "ACP agent process failed to start",
                            Some(error.to_string()),
                            false,
                            None,
                            None,
                            None,
                        )
                        .await)
                }
            };
        let stderr_ring = supervised.stderr_ring();
        let stderr_cursor = stderr_ring.origin();
        let lifecycle = AcpLifecycle::new(
            observer.clone(),
            redactor,
            Some((stderr_ring.clone(), stderr_cursor)),
        );
        // Everything after the (now-running) child: any error orphans the container → reap it.
        let result = async {
            let child = supervised.child_mut();
            let stdin = match child.stdin.take() {
                Some(stdin) => stdin,
                None => {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::Spawn,
                            None,
                            DiagnosticFailureClass::AgentProcess,
                            FailureDisposition::RetrySameTarget,
                            "acp.spawn.stdin_unavailable",
                            "ACP agent stdin was unavailable after spawn",
                            None,
                            false,
                            None,
                            None,
                            None,
                        )
                        .await)
                }
            };
            let stdout = match child.stdout.take() {
                Some(stdout) => stdout,
                None => {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::Spawn,
                            None,
                            DiagnosticFailureClass::AgentProcess,
                            FailureDisposition::RetrySameTarget,
                            "acp.spawn.stdout_unavailable",
                            "ACP agent stdout was unavailable after spawn",
                            None,
                            false,
                            None,
                            None,
                            None,
                        )
                        .await)
                }
            };
            // The crate uses `futures` async-io; our child uses tokio pipes — adapt
            // with tokio_util::compat. ByteStreams::new(outgoing_writer, incoming_reader).
            let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
            lifecycle
                .record(
                    DiagnosticPhase::Spawn,
                    PhaseStatus::Completed,
                    None,
                    None,
                    None,
                )
                .await?;
            Self::connect_observed_after(
                transport,
                config,
                container_controller,
                observer,
                Some(DiagnosticPhase::Spawn),
                Some((stderr_ring.clone(), stderr_cursor)),
            )
            .await
        }
        .await;
        match result {
            Ok(mut backend) => {
                // `supervised` (the process-group owner) MUST live for the whole backend lifetime:
                // `kill_on_drop(true)` would SIGKILL the child the instant it dropped, killing the
                // event-loop task's pipes. Hold it on the backend.
                *backend.supervised.lock().expect("supervised lock") = Some(supervised);
                backend.stderr_ring = Some(stderr_ring);
                Ok(backend)
            }
            Err(e) => {
                // One-shot reap (no backend exists; `supervised` drops here → SIGKILLs the client).
                let reaped = Arc::new(AtomicBool::new(false));
                AcpBackend::reap_container(&container_on_fail, &reaped);
                Err(e)
            }
        }
    }

    /// **Transport-generic** core constructor. Accepts any SDK transport, so
    /// in-process fake-agent unit tests can pass `Channel::duplex()`.
    ///
    /// Starts the connection event loop in a dedicated task, captures a clone of
    /// the `ConnectionTo<Agent>` handle, then runs `initialize`.
    pub async fn connect(
        transport: impl ConnectTo<Client> + 'static,
        config: AcpConfig,
    ) -> Result<Self, BridgeError> {
        Self::connect_observed(
            transport,
            config,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    pub async fn connect_observed(
        transport: impl ConnectTo<Client> + 'static,
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<Self, BridgeError> {
        Self::connect_observed_after(transport, config, None, observer, None, None).await
    }

    async fn connect_observed_after(
        transport: impl ConnectTo<Client> + 'static,
        config: AcpConfig,
        container_controller: Option<ReapController>,
        observer: Arc<dyn DiagnosticObserver>,
        last_completed_before_initialize: Option<DiagnosticPhase>,
        stderr: Option<(ProcessStderrRing, ProcessStderrCursor)>,
    ) -> Result<Self, BridgeError> {
        if config.pre_authenticated && config.auth_method.is_some() {
            return Err(BridgeError::config_invalid(
                "pre_authenticated=true cannot be combined with auth_method",
            ));
        }
        let lifecycle = AcpLifecycle::new(observer, config.diagnostic_redactor.clone(), stderr);
        let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Per-turn chunk routing registry, shared between the notification
        // handler (below) and `prompt` (which registers/unregisters senders).
        let updates: UpdateRegistry = Arc::new(StdMutex::new(HashMap::new()));
        let updates_handler = Arc::clone(&updates);

        // Active policy engine for reverse `session/request_permission` requests.
        // Default = auto-approve (deployed 3a policy); `with_policy` swaps the
        // inner `Arc` later. Shared with the permission handler below so the
        // handler always reads the CURRENT engine.
        let policy: PolicyHandle = Arc::new(StdMutex::new(
            Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
        ));
        let policy_handler = Arc::clone(&policy);
        let updates_perm_handler = Arc::clone(&updates);
        let permission_registry: PermissionRegistryHandle = Arc::new(StdMutex::new(None));
        let registry_perm_handler = Arc::clone(&permission_registry);
        let perm_timeout_ms = Arc::new(AtomicU64::new(120_000));
        let perm_timeout_handler = Arc::clone(&perm_timeout_ms);

        // The event loop owns a long-lived task. `main_fn` publishes a clone of
        // `cx` and then parks on `shutdown_rx` so the connection stays open for
        // the lifetime of the backend (returning from `main_fn` would close it).
        tokio::spawn(async move {
            let builder = Client
                .builder()
                .name("a2a-bridge")
                // `session/update` fan-in. This runs ON the event loop, so it
                // must NEVER call `cx`/block — it only routes a chunk to the
                // matching turn's mpsc and returns. Unmodeled `SessionUpdate`
                // variants are dropped (tolerant reader). A `send` failure
                // (receiver gone: turn already ended) is ignored.
                .on_receive_notification(
                    move |notif: SessionNotification, _cx| {
                        let updates = Arc::clone(&updates_handler);
                        async move {
                            let session_id = notif.session_id.clone();
                            if let Ok(map) = updates.lock() {
                                if let Some(route) = map.get(&session_id) {
                                    if let Some(w) = &route.watch {
                                        bump_activity(w);
                                    }
                                }
                            }

                            // Map rich updates first by borrow, then fall back to the
                            // value-consuming text/usage mapper. This handler stays
                            // try-send only: rich sink writes happen in the off-loop
                            // stream driver.
                            let te = if let Some(kind) = Self::map_session_update_rich(&notif) {
                                Some(TurnEvent::Rich(kind))
                            } else {
                                match Self::map_session_update(notif) {
                                    Some(Update::Text(text)) => Some(TurnEvent::Text(text)),
                                    Some(Update::Usage(snap)) => Some(TurnEvent::Usage(snap)),
                                    _ => None, // unmodeled / non-text (tolerant reader)
                                }
                            };
                            if let Some(te) = te {
                                // Plain get + non-blocking send under a
                                // std::Mutex: no await is held across the lock.
                                if let Ok(map) = updates.lock() {
                                    if let Some(route) = map.get(&session_id) {
                                        let _ = route.tx.send(te);
                                    }
                                }
                            }
                            // else: ignore unmodeled SessionUpdate variants /
                            // non-text chunk content (tolerant reader).
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                // Reverse `session/request_permission`. A real ACP agent (e.g.
                // codex-acp) issues this BACK to us mid-turn. CRITICAL: SDK request
                // handlers run ON the dispatch loop and BLOCK all further message
                // processing while they `await`. So we MUST NOT decide-and-respond
                // inline — a slow policy (or merely holding the loop) would stall the
                // in-flight `session/cancel`, the next `agent_message_chunk`, and the
                // `PromptResponse`, deadlocking the turn. Instead we OFFLOAD the
                // decide+respond to a `cx.spawn` task (the `Responder` is `Send` and
                // moves into it) and RETURN immediately, freeing the loop to keep
                // dispatching. The decide itself is the sync `PolicyEngine::decide`.
                .on_receive_request(
                    move |req: RequestPermissionRequest,
                          responder: agent_client_protocol::Responder<
                        RequestPermissionResponse,
                    >,
                          cx: ConnectionTo<Agent>| {
                        let policy = Arc::clone(&policy_handler);
                        let updates = Arc::clone(&updates_perm_handler);
                        let registry_handler = Arc::clone(&registry_perm_handler);
                        let timeout_handler = Arc::clone(&perm_timeout_handler);
                        async move {
                            let mut turn_meta = None;
                            let mut cancelled = None;
                            if let Ok(map) = updates.lock() {
                                if let Some(route) = map.get(&req.session_id) {
                                    if let Some(w) = &route.watch {
                                        bump_activity(w);
                                    }
                                    turn_meta = route.turn_meta.clone();
                                    cancelled = Some(route.cancelled.clone());
                                }
                            }
                            let registry = registry_handler.lock().ok().and_then(|r| r.clone());
                            let timeout_ms = timeout_handler.load(Ordering::Relaxed);

                            // Offload so the dispatch loop is NOT blocked. The
                            // spawned task owns the `Responder` and answers from there.
                            cx.spawn(async move {
                                let outcome = Self::resolve_permission_outcome(
                                    &policy,
                                    registry.as_ref(),
                                    turn_meta,
                                    cancelled,
                                    timeout_ms,
                                    &req,
                                )
                                .await;
                                // Ignore a `respond` error (peer gone / turn ended):
                                // returning `Err` from a `cx.spawn` task would shut the
                                // whole connection down, which a lost reply must not do.
                                let _ = responder.respond(RequestPermissionResponse::new(outcome));
                                Ok(())
                            })?;
                            // Return PROMPTLY — the loop keeps dispatching.
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                );

            // fs/terminal UNSUPPORTED. We advertise NO fs/terminal client
            // capabilities at `initialize`, so a CONFORMANT agent never sends these.
            // But a non-conformant agent might — and (verified against this SDK
            // version, 1.0.1) an UNREGISTERED inbound request is NOT auto-replied by
            // the default dispatch: it is silently dropped, hanging the agent's
            // `block_task` forever. So we register explicit reject handlers that
            // immediately answer `method_not_found`, keeping the agent unblocked and
            // the loop running. Each handler is trivial + synchronous, so it never
            // stalls the loop.
            let builder = reject_unsupported!(builder;
                ReadTextFileRequest => ReadTextFileResponse,
                WriteTextFileRequest => WriteTextFileResponse,
                CreateTerminalRequest => CreateTerminalResponse,
                TerminalOutputRequest => TerminalOutputResponse,
                ReleaseTerminalRequest => ReleaseTerminalResponse,
                WaitForTerminalExitRequest => WaitForTerminalExitResponse,
                KillTerminalRequest => KillTerminalResponse,
            );

            let _ = builder
                .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                    // Hand a clone back to the AcpBackend; ignore send errors
                    // (receiver dropped => backend gone => nothing to drive).
                    let _ = cx_tx.send(cx.clone());
                    // Park until the backend signals shutdown (or is dropped).
                    let _ = shutdown_rx.await;
                    Ok(())
                })
                .await;
        });

        // One deadline bounds transport connect, initialize, and authenticate,
        // while each await is observed separately so a timeout cannot be
        // mislabeled as a later phase.
        let deadline = tokio::time::Instant::now() + config.handshake_timeout;
        let auth_method_cfg = config.auth_method.clone();
        let pre_authenticated = config.pre_authenticated;
        lifecycle
            .record(
                DiagnosticPhase::Initialize,
                PhaseStatus::Started,
                None,
                None,
                None,
            )
            .await?;
        let cx = match tokio::time::timeout_at(deadline, cx_rx).await {
            Err(_) => {
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::Initialize,
                        last_completed_before_initialize,
                        DiagnosticFailureClass::Timeout,
                        FailureDisposition::RetrySameTarget,
                        "acp.initialize.timeout",
                        "ACP initialize handshake timed out",
                        None,
                        false,
                        None,
                        None,
                        None,
                    )
                    .await)
            }
            Ok(Err(error)) => {
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::Initialize,
                        last_completed_before_initialize,
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::RetrySameTarget,
                        "acp.initialize.connection_closed",
                        "ACP transport closed before initialize",
                        Some(error.to_string()),
                        false,
                        None,
                        None,
                        None,
                    )
                    .await)
            }
            Ok(Ok(cx)) => cx,
        };

        let initialize = cx.send_request(Self::initialize_request()).block_task();
        let resp: InitializeResponse = match tokio::time::timeout_at(deadline, initialize).await {
            Err(_) => {
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::Initialize,
                        last_completed_before_initialize,
                        DiagnosticFailureClass::Timeout,
                        FailureDisposition::RetrySameTarget,
                        "acp.initialize.timeout",
                        "ACP initialize handshake timed out",
                        None,
                        false,
                        None,
                        None,
                        None,
                    )
                    .await)
            }
            Ok(Err(error)) => {
                AcpTraceEvent::InitializeFailed.emit();
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::Initialize,
                        last_completed_before_initialize,
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::RetrySameTarget,
                        "acp.initialize.transport",
                        "ACP initialize request failed",
                        Some(error.to_string()),
                        false,
                        None,
                        None,
                        None,
                    )
                    .await);
            }
            Ok(Ok(resp)) => resp,
        };
        lifecycle
            .record(
                DiagnosticPhase::Initialize,
                PhaseStatus::Completed,
                None,
                None,
                None,
            )
            .await?;

        let advertised: Vec<String> = resp
            .auth_methods
            .iter()
            .map(|method| method.id().0.to_string())
            .collect();
        let chosen = Self::choose_auth_method(auth_method_cfg.as_deref(), &resp.auth_methods);
        let auth_evidence = if pre_authenticated {
            AuthenticationEvidenceInput::PreAuthenticated {
                advertised_method_ids: advertised.clone(),
            }
        } else if let Some(configured_id) = auth_method_cfg.clone() {
            AuthenticationEvidenceInput::ConfiguredMethod {
                advertised: advertised.iter().any(|id| id == &configured_id),
                configured_id,
            }
        } else if let Some(method) = chosen.as_ref() {
            AuthenticationEvidenceInput::SelectedAdvertisedMethod {
                selected_id: method.0.to_string(),
            }
        } else {
            AuthenticationEvidenceInput::NoMethodsAdvertised
        };
        lifecycle
            .record(
                DiagnosticPhase::Authenticate,
                PhaseStatus::Started,
                None,
                None,
                Some(auth_evidence.clone()),
            )
            .await?;

        if pre_authenticated {
            AcpTraceEvent::PreAuthenticated {
                advertised_count: AcpTraceEvent::bounded_count(advertised.len()),
            }
            .emit();
            lifecycle
                .record(
                    DiagnosticPhase::Authenticate,
                    PhaseStatus::Skipped,
                    None,
                    Some("acp.auth.pre_authenticated"),
                    Some(auth_evidence),
                )
                .await?;
        } else if let Some(method_id) = chosen {
            if auth_method_cfg.is_some() && !advertised.iter().any(|id| id == method_id.0.as_ref())
            {
                AcpTraceEvent::AuthMethodMismatch {
                    advertised_count: AcpTraceEvent::bounded_count(advertised.len()),
                }
                .emit();
            }
            let authenticate = cx
                .send_request(AuthenticateRequest::new(method_id))
                .block_task();
            match tokio::time::timeout_at(deadline, authenticate).await {
                Err(_) => {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::Authenticate,
                            Some(DiagnosticPhase::Initialize),
                            DiagnosticFailureClass::Timeout,
                            FailureDisposition::RetrySameTarget,
                            "acp.authenticate.timeout",
                            "ACP authenticate request timed out",
                            None,
                            false,
                            None,
                            None,
                            Some(auth_evidence),
                        )
                        .await)
                }
                Ok(Err(error)) => {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::Authenticate,
                            Some(DiagnosticPhase::Initialize),
                            DiagnosticFailureClass::Authentication,
                            FailureDisposition::Fatal,
                            "acp.authenticate.rejected",
                            "ACP authentication failed",
                            Some(error.to_string()),
                            false,
                            None,
                            None,
                            Some(auth_evidence),
                        )
                        .await)
                }
                Ok(Ok(_)) => {
                    lifecycle
                        .record(
                            DiagnosticPhase::Authenticate,
                            PhaseStatus::Completed,
                            None,
                            None,
                            Some(auth_evidence),
                        )
                        .await?;
                }
            }
        } else {
            lifecycle
                .record(
                    DiagnosticPhase::Authenticate,
                    PhaseStatus::Skipped,
                    None,
                    Some("acp.auth.no_methods_advertised"),
                    Some(auth_evidence),
                )
                .await?;
        }

        let container_reap = container_controller.or_else(|| {
            config
                .container
                .as_ref()
                .map(ContainerReap::legacy_controller)
        });
        Ok(Self {
            conn: Some(AcpConn {
                cx,
                agent_capabilities: resp.agent_capabilities,
                auth_methods: resp.auth_methods,
                _shutdown: shutdown_tx,
                updates,
            }),
            supervised: Arc::new(StdMutex::new(None)),
            stderr_ring: None,
            config: Some(config),
            container_reap,
            reaped: Arc::new(AtomicBool::new(false)),
            unavailable: Arc::new(AtomicBool::new(false)),
            dispatch_gate: Arc::new(StdMutex::new(())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cfg: Arc::new(StdMutex::new(HashMap::new())),
            pending_turn_meta: StdMutex::new(HashMap::new()),
            policy,
            permission_registry,
            perm_timeout_ms,
            #[cfg(test)]
            prompt_snapshot_hook: StdMutex::new(None),
            #[cfg(test)]
            before_process_redactor_hook: StdMutex::new(None),
            #[cfg(test)]
            fail_deferred_cancel_send: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fail_cancel_send: Arc::new(AtomicBool::new(false)),
        })
    }

    const CHATGPT_AUTH_IDS: &'static [&'static str] = &["chat-gpt", "chatgpt"];

    fn choose_auth_method(
        configured: Option<&str>,
        advertised: &[AuthMethod],
    ) -> Option<AuthMethodId> {
        if let Some(method) = configured {
            return Some(AuthMethodId::new(method));
        }
        if advertised.is_empty() {
            return None;
        }

        for preferred in Self::CHATGPT_AUTH_IDS {
            if let Some(method) = advertised
                .iter()
                .find(|method| method.id().0.as_ref() == *preferred)
            {
                return Some(method.id().clone());
            }
        }

        advertised.first().map(|method| method.id().clone())
    }

    /// Inject a concrete [`PolicyEngine`] for reverse `session/request_permission`
    /// decisions, replacing the default auto-approve. Swaps the engine the already-
    /// registered permission handler reads (see [`Self::policy`]), so it takes
    /// effect for subsequent requests. Builder-style so call sites stay
    /// `connect(..)?.with_policy(..)`; Task 6's `main` threads its policy through here.
    #[must_use]
    pub fn with_policy(self, policy: Arc<dyn PolicyEngine>) -> Self {
        if let Ok(mut p) = self.policy.lock() {
            *p = policy;
        }
        self
    }

    /// Inject the bridge-owned deferred-permission registry used by the reverse
    /// `session/request_permission` handler.
    #[must_use]
    pub fn with_permission_registry(self, reg: Arc<PermissionRegistry>) -> Self {
        if let Ok(mut r) = self.permission_registry.lock() {
            *r = Some(reg);
        }
        self
    }

    /// Override the deferred-permission timeout. Primarily used by tests; live
    /// config wiring is in the follow-up task.
    #[must_use]
    pub fn with_permission_timeout_ms(self, ms: u64) -> Self {
        self.perm_timeout_ms.store(ms, Ordering::Relaxed);
        self
    }

    /// Decide a reverse `session/request_permission` and map the `PolicyEngine`
    /// verdict onto an ACP [`RequestPermissionOutcome`].
    ///
    /// Mapping (per the Task-5 policy table):
    /// * **Approve** → select the first `AllowOnce` option; fallback the first
    ///   `AllowAlways`; if NEITHER allow option exists (e.g. agent offered only
    ///   reject options), `Cancelled`. We must NOT fall back to the first
    ///   arbitrary option: selecting a reject option under an approve policy
    ///   would be a correctness inversion (permanent deny when intent was grant).
    /// * **Deny** (`Err(PermissionDenied)`) → first `RejectOnce`; fallback first
    ///   `RejectAlways`; fallback `Cancelled` if no reject option exists.
    /// * **Any other policy error** (abstain) → `Cancelled`.
    fn decide_permission(
        policy: &PolicyHandle,
        req: &RequestPermissionRequest,
    ) -> RequestPermissionOutcome {
        // Model the agent's tool-call permission ask as a non-interactive
        // `PermissionRequest` carrying the tool-call id (best-effort request id).
        // The default auto-approver approves it; a real engine may deny/abstain.
        let perm_req = PermissionRequest::with_id(req.tool_call.tool_call_id.0.to_string(), false);
        let decision = policy
            .lock()
            .ok()
            .map(|p| p.decide(&perm_req, &SessionContext));

        match decision {
            Some(verdict) => Self::map_verdict_to_outcome(verdict, &req.options),
            None => RequestPermissionOutcome::Cancelled,
        }
    }

    /// Resolve one reverse `session/request_permission` to an ACP outcome.
    ///
    /// DEAD-SAFE: when the policy returns `Decide`, this preserves the pre-slice
    /// `decide_permission` mapping, including `with_id(..., false)`.
    async fn resolve_permission_outcome(
        policy: &PolicyHandle,
        registry: Option<&Arc<PermissionRegistry>>,
        turn_meta: Option<TurnMeta>,
        cancelled: Option<Arc<AtomicBool>>,
        timeout_ms: u64,
        req: &RequestPermissionRequest,
    ) -> RequestPermissionOutcome {
        let perm_req = PermissionRequest::with_id(req.tool_call.tool_call_id.0.to_string(), false);
        let outcome = policy
            .lock()
            .ok()
            .map(|p| p.interactive_decide(&perm_req, &SessionContext));

        match outcome {
            None => RequestPermissionOutcome::Cancelled,
            Some(PolicyOutcome::Decide(verdict)) => {
                Self::map_verdict_to_outcome(verdict, &req.options)
            }
            Some(PolicyOutcome::Defer) => {
                let (Some(reg), Some(meta)) = (registry, turn_meta) else {
                    return Self::deny_outcome(&req.options);
                };
                if cancelled.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                    return RequestPermissionOutcome::Cancelled;
                }
                let request_id = req.tool_call.tool_call_id.0.to_string();
                let key = PermKey {
                    context_id: meta.context_id,
                    generation: meta.generation,
                    op: meta.op,
                    request_id,
                };
                let view = Self::pending_view(req, &key, timeout_ms);
                let key_for_cancel = key.clone();
                let (rx, _guard) = reg.register(key, view);
                if cancelled.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                    reg.resolve(&key_for_cancel, PermissionResolution::Cancelled);
                }
                let res = tokio::select! {
                    biased;
                    r = rx => r.unwrap_or(PermissionResolution::Cancelled),
                    _ = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
                        PermissionResolution::Decided(PermitDecision::Deny {
                            option_id: None,
                            reason: Some("timeout".into()),
                        })
                    }
                };
                match res {
                    PermissionResolution::Decided(d) => {
                        Self::map_permit_to_outcome(d, &req.options)
                    }
                    PermissionResolution::Cancelled => RequestPermissionOutcome::Cancelled,
                }
            }
        }
    }

    fn map_verdict_to_outcome(
        verdict: Result<PermissionDecision, BridgeError>,
        options: &[PermissionOption],
    ) -> RequestPermissionOutcome {
        match verdict {
            Ok(PermissionDecision::Approve) => Self::select_permission_option(
                options,
                &[
                    PermissionOptionKind::AllowOnce,
                    PermissionOptionKind::AllowAlways,
                ],
            )
            .unwrap_or(RequestPermissionOutcome::Cancelled),
            Err(BridgeError::PermissionDenied) => Self::deny_outcome(options),
            _ => RequestPermissionOutcome::Cancelled,
        }
    }

    fn map_permit_to_outcome(
        decision: PermitDecision,
        options: &[PermissionOption],
    ) -> RequestPermissionOutcome {
        match decision {
            PermitDecision::Approve { option_id } => option_id
                .and_then(|id| {
                    Self::select_permission_option_id_kind(
                        options,
                        &id,
                        &[
                            PermissionOptionKind::AllowOnce,
                            PermissionOptionKind::AllowAlways,
                        ],
                    )
                })
                .unwrap_or_else(|| {
                    Self::select_permission_option(
                        options,
                        &[
                            PermissionOptionKind::AllowOnce,
                            PermissionOptionKind::AllowAlways,
                        ],
                    )
                    .unwrap_or(RequestPermissionOutcome::Cancelled)
                }),
            PermitDecision::Deny { option_id, .. } => option_id
                .and_then(|id| {
                    Self::select_permission_option_id_kind(
                        options,
                        &id,
                        &[
                            PermissionOptionKind::RejectOnce,
                            PermissionOptionKind::RejectAlways,
                        ],
                    )
                })
                .unwrap_or_else(|| Self::deny_outcome(options)),
            PermitDecision::Modify { option_id, .. } => {
                Self::select_permission_option_id(options, &option_id)
                    .unwrap_or(RequestPermissionOutcome::Cancelled)
            }
            PermitDecision::Escalate { .. } => Self::deny_outcome(options),
        }
    }

    fn deny_outcome(options: &[PermissionOption]) -> RequestPermissionOutcome {
        Self::select_permission_option(
            options,
            &[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ],
        )
        .unwrap_or(RequestPermissionOutcome::Cancelled)
    }

    fn select_permission_option(
        options: &[PermissionOption],
        kinds: &[PermissionOptionKind],
    ) -> Option<RequestPermissionOutcome> {
        for k in kinds {
            if let Some(opt) = options.iter().find(|o| o.kind == *k) {
                return Some(RequestPermissionOutcome::Selected(
                    SelectedPermissionOutcome::new(opt.option_id.clone()),
                ));
            }
        }
        None
    }

    fn select_permission_option_id(
        options: &[PermissionOption],
        option_id: &str,
    ) -> Option<RequestPermissionOutcome> {
        options
            .iter()
            .find(|o| o.option_id.0.as_ref() == option_id)
            .map(|opt| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    opt.option_id.clone(),
                ))
            })
    }

    fn select_permission_option_id_kind(
        options: &[PermissionOption],
        option_id: &str,
        kinds: &[PermissionOptionKind],
    ) -> Option<RequestPermissionOutcome> {
        options
            .iter()
            .find(|o| o.option_id.0.as_ref() == option_id && kinds.contains(&o.kind))
            .map(|opt| {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                    opt.option_id.clone(),
                ))
            })
    }

    fn pending_view(
        req: &RequestPermissionRequest,
        key: &PermKey,
        timeout_ms: u64,
    ) -> PendingPermissionView {
        PendingPermissionView {
            request_id: key.request_id.clone(),
            tool_call_id: req.tool_call.tool_call_id.0.to_string(),
            generation: key.generation,
            op: key.op.clone(),
            title: Self::cap_permission_text(
                req.tool_call.fields.title.as_deref().unwrap_or_default(),
            ),
            options: req
                .options
                .iter()
                .map(|opt| PermissionOptionView {
                    option_id: opt.option_id.0.to_string(),
                    name: opt.name.clone(),
                    kind: Self::permission_kind_string(opt.kind),
                })
                .collect(),
            raw_input: req
                .tool_call
                .fields
                .raw_input
                .as_ref()
                .and_then(|raw| serde_json::to_string(raw).ok())
                .map(|raw| Self::cap_permission_text(&raw)),
            timeout_ms,
        }
    }

    fn permission_kind_string(kind: PermissionOptionKind) -> String {
        serde_json::to_value(kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{kind:?}"))
    }

    fn cap_permission_text(s: &str) -> String {
        if s.len() <= PERMISSION_VIEW_CAP {
            return s.to_string();
        }
        let mut end = 0;
        for (idx, _) in s.char_indices() {
            if idx > PERMISSION_VIEW_CAP {
                break;
            }
            end = idx;
        }
        s[..end].to_string()
    }

    /// Negotiated agent capabilities from the most recent `initialize`.
    #[must_use]
    pub fn agent_capabilities(&self) -> Option<&AgentCapabilities> {
        self.conn.as_ref().map(|c| &c.agent_capabilities)
    }

    /// Authentication methods the agent advertised at `initialize`.
    #[must_use]
    pub fn auth_methods(&self) -> Option<&[AuthMethod]> {
        self.conn.as_ref().map(|c| c.auth_methods.as_slice())
    }

    /// Access the SDK connection handle. Returns `Err(AgentCrashed)` if no SDK
    /// connection exists, so prompt routing gets a clean error seam instead of a
    /// panic inside the event loop. Used to send agent-bound requests.
    fn cx(&self) -> Result<&ConnectionTo<Agent>, BridgeError> {
        self.conn.as_ref().map(|c| &c.cx).ok_or_else(|| {
            BridgeError::agent_crashed("SDK connection handle unavailable (backend not connected)")
        })
    }

    /// The per-turn chunk routing registry shared with the notification handler.
    fn updates(&self) -> Result<&UpdateRegistry, BridgeError> {
        self.conn.as_ref().map(|c| &c.updates).ok_or_else(|| {
            BridgeError::agent_crashed(
                "update routing registry unavailable (backend not connected)",
            )
        })
    }

    fn take_pending_turn_meta(&self, session: &SessionId) -> Option<TurnMeta> {
        self.pending_turn_meta
            .lock()
            .expect("pending_turn_meta lock")
            .remove(session)
    }

    /// Look up (or create) the per-bridge-session state for `key`, cloning the
    /// `Arc` out so the map mutex is released before any await. Always returns
    /// the SAME `Arc` for a given key, so the `OnceCell`/turn-lock/latch inside
    /// are shared across all callers for that bridge session.
    ///
    /// The critical section is a HashMap get-or-insert that NEVER yields, so the
    /// async `lock().await` is held only for nanoseconds — there is no deadlock
    /// risk and no chance of holding the map lock across an await. (`try_lock`
    /// would PANIC if two tasks on different runtime threads raced here.)
    async fn session_entry(&self, key: &SessionId) -> Arc<AgentSession> {
        let mut map = self.sessions.lock().await;
        if let Some(s) = map.get(key) {
            return Arc::clone(s);
        }
        let s = Arc::new(AgentSession::new());
        map.insert(key.clone(), Arc::clone(&s));
        s
    }

    /// Ensure the agent-minted session for bridge key `key` exists, minting it
    /// LAZILY via `session/new` on first call and reusing the stored id after.
    ///
    /// Exactly-once minting [Cx-M2]: concurrent first calls for the same `key`
    /// serialize on `init_lock`; the winner moves the guard into a spawned task,
    /// so the agent sees `session/new` ONCE and caller cancellation cannot abort it.
    ///
    /// Cancel-latch [Cx-M2]: the minting task — and only it — drains the latch
    /// AFTER `OnceCell` has published the id (so a concurrent `request_cancel`
    /// can already observe it); if a `cancel` raced ahead of creation it fires
    /// `session/cancel` for the freshly-minted id so the cancel is not dropped.
    /// The latch is *claimed* with an atomic swap so exactly one of the minting
    /// task and a concurrent `request_cancel` sends the notification (no double).
    ///
    /// Lost-cancel window closed: the task publishes the `OnceCell` id, then
    /// drains the latch. If `request_cancel` ran while the id was not observable,
    /// it stored `true`; once the id is visible, it and the initializer race on
    /// the same `swap` and exactly one sends.
    ///
    /// `prompt` calls this, then acquires `turn_lock` and sends `session/prompt`.
    async fn ensure_session(&self, key: &SessionId) -> Result<AgentSessionId, BridgeError> {
        self.ensure_session_observed(
            key,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn ensure_session_observed(
        &self,
        key: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<AgentSessionId, BridgeError> {
        let snapshot = self.session_config_snapshot(key)?;
        self.ensure_session_observed_with_snapshot(key, observer, snapshot)
            .await
    }

    async fn ensure_session_observed_with_snapshot(
        &self,
        key: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
        snapshot: SessionConfigSnapshot,
    ) -> Result<AgentSessionId, BridgeError> {
        let stderr = self
            .stderr_ring
            .as_ref()
            .map(|ring| (ring.clone(), ring.cursor()));
        let lifecycle = AcpLifecycle::new(
            observer,
            self.diagnostic_redactor_for_cwds(std::slice::from_ref(&snapshot.desired_cwd)),
            stderr,
        );
        let entry = self.session_entry(key).await;
        // One owned snapshot supplies both mint inputs and the lifecycle redactor.
        // A concurrent `configure_session` replacement affects the next snapshot,
        // never half of this attempt.
        let desired_cwd = snapshot.desired_cwd;
        let EffectiveConfig {
            mode,
            model,
            effort,
        } = snapshot.config;
        let agent_id_for_mint = self
            .config
            .as_ref()
            .map(|c| c.agent_id.clone())
            .unwrap_or_default();

        // A spawned single-flight task owns initialization once it starts. This
        // shields the ACP request and its lifecycle grammar from cancellation of
        // the caller that happened to win the race to initialize.
        let cwd_for_mint = desired_cwd.clone();
        // MCP servers to offer at session/new (ADR-0028). Static per agent (the entry's `mcp`);
        // `{cwd}` is substituted with `cwd_for_mint` inside `new_session_request`. Empty for
        // non-Acp delivery (codex/kiro get MCP via their native channel, not this param).
        let mcp_for_mint: Vec<bridge_core::mcp::McpServerSpec> = self
            .config
            .as_ref()
            .map(|c| c.mcp.clone())
            .unwrap_or_default();
        let (id, newly_minted) = if let Some(id) = entry.agent_id.get() {
            (id.clone(), false)
        } else {
            let init_guard = Arc::clone(&entry.init_lock).lock_owned().await;
            if let Some(id) = entry.agent_id.get() {
                drop(init_guard);
                (id.clone(), false)
            } else {
                if let Ok(mut active_cwd) = entry.active_mint_cwd.lock() {
                    *active_cwd = Some(desired_cwd.clone());
                }
                // Own cleanup before the first await after publication. If the
                // caller is cancelled while waiting to install the process
                // redactor, this local guard clears the attempt; once the
                // initializer is spawned, ownership moves into that task.
                let active_mint_cwd = ActiveMintCwdGuard(Arc::clone(&entry));
                #[cfg(test)]
                {
                    let hook = self
                        .before_process_redactor_hook
                        .lock()
                        .ok()
                        .and_then(|hook| hook.clone());
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                self.apply_process_redactor_for_live_sessions(None).await;
                let entry_for_init = Arc::clone(&entry);
                let cx_for_init = self.cx()?.clone();
                let lifecycle_for_init = lifecycle.clone();
                let stderr_ring_for_init = self.stderr_ring.clone();
                #[cfg(test)]
                let fail_deferred_cancel_send = Arc::clone(&self.fail_deferred_cancel_send);
                let task = tokio::spawn(async move {
                    let _init_guard = init_guard;
                    let entry = entry_for_init;
                    let cx = cx_for_init;
                    let lifecycle = lifecycle_for_init;
                    let _active_mint_cwd = active_mint_cwd;
                    let mut process_evidence = MintProcessEvidenceGuard::new(stderr_ring_for_init);
                    let outcome = async {
                        // (1) session/new — mint the agent session id.
                        lifecycle
                            .record(
                                DiagnosticPhase::SessionCreate,
                                PhaseStatus::Started,
                                None,
                                None,
                                None,
                            )
                            .await?;
                        let req =
                            Self::new_session_request(PathBuf::from(&cwd_for_mint), &mcp_for_mint);
                        // From request installation until durable minted-cwd
                        // publication, the process may have consumed a
                        // cwd-expanded credential that a later bounded policy
                        // cannot safely enumerate. Any error/abort in this
                        // interval makes process stderr metadata-only.
                        process_evidence.arm();
                        let resp = match cx.send_request(req).block_task().await {
                            Ok(resp) => resp,
                            Err(error) => {
                                AcpTraceEvent::SessionCreateFailed.emit();
                                return Err(lifecycle
                                    .failure(
                                        DiagnosticPhase::SessionCreate,
                                        Some(DiagnosticPhase::Authenticate),
                                        DiagnosticFailureClass::Transport,
                                        FailureDisposition::RetrySameTarget,
                                        "acp.session_create.transport",
                                        "ACP session creation failed",
                                        Some(error.to_string()),
                                        false,
                                        None,
                                        None,
                                        None,
                                    )
                                    .await);
                            }
                        };
                        let id = resp.session_id;
                        let opts0 = resp.config_options.unwrap_or_default();
                        lifecycle
                            .record(
                                DiagnosticPhase::SessionCreate,
                                PhaseStatus::Completed,
                                None,
                                None,
                                None,
                            )
                            .await?;

                        // (2) set_mode — HARD error, configured INSIDE the closure (before
                        // returning the id). The operator asked for a specific mode; if the
                        // agent REJECTS the mode id we FAIL session setup. Because this
                        // `?`-returns from the task before publishing the id, so the
                        // `OnceCell` stays UNINITIALIZED and the next `ensure_session`
                        // re-runs the full mint+configure rather than seeing a committed-but-
                        // unconfigured session and silently proceeding in the WRONG mode.
                        // (No prompt is sent on a failed setup, so the minted-then-abandoned
                        // agent session does no uncontrolled work.)
                        if let Some(mode) = mode.as_deref() {
                            lifecycle
                                .record(
                                    DiagnosticPhase::ConfigApply,
                                    PhaseStatus::Started,
                                    Some(DiagnosticOperation::Mode),
                                    None,
                                    None,
                                )
                                .await?;
                            if let Err(error) = cx
                                .send_request(Self::set_mode_request(id.clone(), mode))
                                .block_task()
                                .await
                            {
                                return Err(lifecycle
                                    .failure(
                                        DiagnosticPhase::ConfigApply,
                                        Some(DiagnosticPhase::SessionCreate),
                                        DiagnosticFailureClass::Model,
                                        FailureDisposition::Fatal,
                                        "acp.config.mode_rejected",
                                        "ACP mode configuration was rejected",
                                        Some(error.to_string()),
                                        false,
                                        None,
                                        Some(DiagnosticOperation::Mode),
                                        None,
                                    )
                                    .await);
                            }
                            lifecycle
                                .record(
                                    DiagnosticPhase::ConfigApply,
                                    PhaseStatus::Completed,
                                    Some(DiagnosticOperation::Mode),
                                    None,
                                    None,
                                )
                                .await?;
                        }

                        // (3) model remains hard. (4) effort remains best-effort, but is
                        // observed as its own typed operation after model settles.
                        let mut refreshed_surface = ConfigSurface { opts: opts0 };
                        // Always validate the advertised/current model surface. With no
                        // pin this still rejects a bridge-blocked current model, matching
                        // the pre-observation safety contract.
                        lifecycle
                            .record(
                                DiagnosticPhase::ConfigApply,
                                PhaseStatus::Started,
                                Some(DiagnosticOperation::Model),
                                None,
                                None,
                            )
                            .await?;
                        match Self::apply_model_effort(
                            &cx,
                            &id,
                            &agent_id_for_mint,
                            &refreshed_surface,
                            model.as_deref(),
                            None,
                            ApplyPurpose::Mint,
                        )
                        .await
                        {
                            Ok((surface, _)) => refreshed_surface = surface,
                            Err(
                                ApplyConfigError::NotAdvertised(error)
                                | ApplyConfigError::Rejected(error),
                            ) => {
                                return Err(lifecycle
                                    .failure(
                                        DiagnosticPhase::ConfigApply,
                                        Some(DiagnosticPhase::SessionCreate),
                                        DiagnosticFailureClass::Model,
                                        FailureDisposition::Fatal,
                                        "acp.config.model_rejected",
                                        "ACP model configuration failed",
                                        Some(error.to_string()),
                                        false,
                                        None,
                                        Some(DiagnosticOperation::Model),
                                        None,
                                    )
                                    .await);
                            }
                        }
                        lifecycle
                            .record(
                                DiagnosticPhase::ConfigApply,
                                PhaseStatus::Completed,
                                Some(DiagnosticOperation::Model),
                                model.is_none().then_some("acp.config.model_validated"),
                                None,
                            )
                            .await?;
                        if effort.is_some() {
                            lifecycle
                                .record(
                                    DiagnosticPhase::ConfigApply,
                                    PhaseStatus::Started,
                                    Some(DiagnosticOperation::Effort),
                                    None,
                                    None,
                                )
                                .await?;
                            match Self::apply_model_effort(
                                &cx,
                                &id,
                                &agent_id_for_mint,
                                &refreshed_surface,
                                None,
                                effort,
                                ApplyPurpose::Mint,
                            )
                            .await
                            {
                                Ok((surface, _)) => refreshed_surface = surface,
                                Err(
                                    ApplyConfigError::NotAdvertised(error)
                                    | ApplyConfigError::Rejected(error),
                                ) => {
                                    return Err(lifecycle
                                        .failure(
                                            DiagnosticPhase::ConfigApply,
                                            Some(DiagnosticPhase::SessionCreate),
                                            DiagnosticFailureClass::Model,
                                            FailureDisposition::Fatal,
                                            "acp.config.effort_rejected",
                                            "ACP effort configuration failed",
                                            Some(error.to_string()),
                                            false,
                                            None,
                                            Some(DiagnosticOperation::Effort),
                                            None,
                                        )
                                        .await);
                                }
                            }
                            lifecycle
                                .record(
                                    DiagnosticPhase::ConfigApply,
                                    PhaseStatus::Completed,
                                    Some(DiagnosticOperation::Effort),
                                    Some("acp.config.effort_settled"),
                                    None,
                                )
                                .await?;
                        }
                        *entry.config_surface.lock().expect("config_surface lock") =
                            Some(refreshed_surface);

                        // (5) Record the cwd that was actually used to mint this session so
                        // the immutability guard below can compare future requests against
                        // what the agent was ACTUALLY given at session/new (ACP §11A).
                        // `set` on a `OnceCell` can only fail if already set (impossible here
                        // since we own the single-flight init guard); ignore the result.
                        let _ = entry.minted_cwd.set(cwd_for_mint);
                        entry
                            .agent_id
                            .set(id.clone())
                            .map_err(|_| BridgeError::InvalidStateTransition)?;
                        process_evidence.commit();

                        // Publish the id before draining the cancel latch. A racing
                        // `request_cancel` either claims the same latch or observes
                        // the id and sends; exactly one side handles the raced cancel.
                        if entry.cancel_requested.swap(false, Ordering::SeqCst) {
                            #[cfg(test)]
                            let send_result = if fail_deferred_cancel_send.load(Ordering::SeqCst) {
                                Err(agent_client_protocol::Error::internal_error())
                            } else {
                                cx.send_notification(CancelNotification::new(id.clone()))
                            };
                            #[cfg(not(test))]
                            let send_result =
                                cx.send_notification(CancelNotification::new(id.clone()));
                            if let Err(error) = send_result {
                                return Err(AcpBackend::deferred_cancel_delivery_failure(
                                    &lifecycle,
                                    error.to_string(),
                                )
                                .await);
                            }
                        }

                        Ok::<_, BridgeError>(id)
                    }
                    .await;
                    outcome
                });
                let id = task.await.map_err(|_| {
                    BridgeError::agent_crashed("ACP session initialization task failed")
                })??;
                (id, true)
            }
        };

        // Immutability guard (ACP §11A): a session's cwd is fixed at `session/new`.
        // If this call did NOT mint the session (it already existed) but the desired
        // cwd differs from the recorded minted cwd, the caller is trying to reuse a
        // warm session for a DIFFERENT repo — error rather than silently operating in
        // the wrong directory. Matching cwd (same session recycled for the same repo)
        // is fine and does not error.
        if !newly_minted {
            lifecycle
                .record(
                    DiagnosticPhase::SessionCreate,
                    PhaseStatus::Started,
                    None,
                    None,
                    None,
                )
                .await?;
            lifecycle
                .record(
                    DiagnosticPhase::SessionCreate,
                    PhaseStatus::Skipped,
                    None,
                    Some("acp.session.reused"),
                    None,
                )
                .await?;
            lifecycle
                .record(
                    DiagnosticPhase::ConfigApply,
                    PhaseStatus::Started,
                    None,
                    None,
                    None,
                )
                .await?;
            if let Some(minted) = entry.minted_cwd.get() {
                if *minted != desired_cwd {
                    return Err(lifecycle
                        .failure(
                            DiagnosticPhase::ConfigApply,
                            Some(DiagnosticPhase::SessionCreate),
                            DiagnosticFailureClass::Config,
                            FailureDisposition::Fatal,
                            "acp.config.cwd_mismatch",
                            "ACP session cwd cannot change after creation",
                            None,
                            false,
                            None,
                            None,
                            None,
                        )
                        .await);
                }
            }
            lifecycle
                .record(
                    DiagnosticPhase::ConfigApply,
                    PhaseStatus::Skipped,
                    None,
                    Some("acp.config.reused"),
                    None,
                )
                .await?;
        }

        Ok(id)
    }

    /// Record a cancel for bridge key `key`, honoring the create/cancel race.
    ///
    /// * If the agent session already exists, send `session/cancel` for it now.
    /// * If `session/new` is still in flight (or hasn't started), set the latch
    ///   so `ensure_session` flushes the cancel the instant the id is minted.
    /// * If `key` was never seen (session ended / never started), it's a no-op
    ///   on the wire but still latches a freshly-created entry defensively.
    ///
    /// Task 4 builds full `cancel()` completion semantics (waiting for the
    /// prompt result with `stopReason:"cancelled"`) on top of this.
    async fn request_cancel(&self, key: &SessionId) -> Result<CancelDispatch, TeardownFailure> {
        let entry = self.session_entry(key).await;
        let cx = self.cx().map_err(|error| TeardownFailure {
            error,
            prompt_may_have_been_accepted: false,
        })?;
        // Set the latch FIRST so a `session/new` completing concurrently observes
        // it. If the id is ALREADY present, CLAIM the latch (swap→false): if we
        // win the claim we fire now; if the minting task already claimed and
        // fired, we don't double-send.
        entry.cancel_requested.store(true, Ordering::SeqCst);
        if let Some(agent_id) = entry.agent_id.get() {
            if entry.cancel_requested.swap(false, Ordering::SeqCst) {
                Self::send_cancel_under_dispatch_fence(
                    &self.dispatch_gate,
                    &self.unavailable,
                    || Self::turn_prompt_accepted_handle(&entry),
                    || {
                        #[cfg(test)]
                        if self.fail_cancel_send.load(Ordering::SeqCst) {
                            return Err(agent_client_protocol::Error::internal_error());
                        }
                        cx.send_notification(CancelNotification::new(agent_id.clone()))
                    },
                )?;
                return Ok(CancelDispatch::Delivered);
            }
        }
        Ok(CancelDispatch::Latched)
    }

    async fn cancel_inner(&self, session: &SessionId) -> Result<CancelDispatch, TeardownFailure> {
        let dispatch = self.request_cancel(session).await?;

        let entry = self.session_entry(session).await;
        let (turn_active, turn_meta) = if let Some(agent_id) = entry.agent_id.get() {
            self.updates()
                .ok()
                .and_then(|updates| {
                    updates.lock().ok().and_then(|map| {
                        map.get(agent_id).map(|route| {
                            route.cancelled.store(true, Ordering::SeqCst);
                            (Arc::clone(&route.active), route.turn_meta.clone())
                        })
                    })
                })
                .map_or((None, None), |(active, turn_meta)| {
                    (Some(active), turn_meta)
                })
        } else {
            (None, None)
        };
        if let (Some(reg), Some(meta)) = (
            self.permission_registry.lock().ok().and_then(|r| r.clone()),
            turn_meta,
        ) {
            reg.resolve_context_cancelled(&meta.context_id);
        }
        let Some(turn_active) = turn_active else {
            return Ok(dispatch);
        };
        if entry.turn_lock.try_lock().is_ok() {
            return Ok(dispatch);
        }

        let turn_lock = Arc::clone(&entry.turn_lock);
        let supervised = Arc::clone(&self.supervised);
        let container = self.container_reap.clone();
        let reaped = Arc::clone(&self.reaped);
        let unavailable = Arc::clone(&self.unavailable);
        let dispatch_gate = Arc::clone(&self.dispatch_gate);
        let kill_slot = Arc::clone(&entry.turn_kill);
        let grace = self.cancel_grace();
        tokio::spawn(async move {
            if tokio::time::timeout(grace, turn_lock.lock()).await.is_err() {
                if turn_active
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return;
                }
                AcpBackend::escalate_terminate(
                    &supervised,
                    &container,
                    &reaped,
                    &dispatch_gate,
                    &unavailable,
                );
                let kill = kill_slot.lock().ok().and_then(|guard| guard.clone());
                if let Some(kill) = kill {
                    kill.notify_one();
                }
            }
        });
        Ok(dispatch)
    }

    /// Construct from an already-spawned `Supervised` child, driving the ACP
    /// connection over its stdin/stdout via the SDK — a thin shim over `connect`
    /// (same `ByteStreams` + tokio→futures-io compat as `spawn`, but for a child
    /// the caller already spawned). The returned backend owns `supervised` for
    /// its lifetime (so `kill_on_drop` does not SIGKILL it on return).
    ///
    /// This replaces the v1 hand-rolled JSON-RPC `from_child`; call sites (the
    /// gated e2es, `main`) now `.await` it.
    pub async fn from_child(
        mut supervised: Supervised,
        config: AcpConfig,
    ) -> Result<Self, BridgeError> {
        supervised.apply_stderr_redactor(config.diagnostic_redactor.clone());
        let stderr_ring = supervised.stderr_ring();
        let stderr_cursor = stderr_ring.origin();
        let child = supervised.child_mut();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("agent stdin unavailable in from_child"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("agent stdout unavailable in from_child"))?;
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let mut backend = Self::connect_observed_after(
            transport,
            config,
            None,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
            None,
            Some((stderr_ring.clone(), stderr_cursor)),
        )
        .await?;
        *backend.supervised.lock().expect("supervised lock") = Some(supervised);
        backend.stderr_ring = Some(stderr_ring);
        Ok(backend)
    }

    /// Discovery seam: mint a THROWAWAY `session/new`, read the advertised model/effort/mode
    /// surfaces the agent returns, map them to [`AgentCaps`], and return — **no prompt is sent
    /// and nothing is configured**. This reads the SAME `config_options` the lazy mint
    /// ([`Self::ensure_session`]) reads at its step (1), plus the SDK 1.x `modes` state, BEFORE
    /// any model/mode/effort resolution.
    ///
    /// `cwd` is any readable directory: `session/new` requires one (ACP §11A) but discovery reads
    /// nothing from it. The minted session is intentionally NOT registered in `self.sessions` (it
    /// is throwaway), so there is no `forget_session` to call. Teardown is the CALLER's: the host
    /// model-catalog probe ([`crate::catalog_probe`] in the bin) builds a one-shot backend per
    /// agent and drops it, which SIGKILLs the `Supervised` child (`kill_on_drop`) and reaps any
    /// `:ro` container (the [`Drop`] impl). The advertised list is account/adapter-driven and
    /// sandbox-independent, so the probe builds this backend host-side (sandbox stripped).
    pub async fn describe_options(&self, cwd: &std::path::Path) -> Result<AgentCaps, BridgeError> {
        self.describe_options_observed(
            cwd,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    pub async fn describe_options_observed(
        &self,
        cwd: &std::path::Path,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<AgentCaps, BridgeError> {
        let cwd_text = cwd.to_string_lossy().into_owned();
        let stderr = self
            .stderr_ring
            .as_ref()
            .map(|ring| (ring.clone(), ring.cursor()));
        let lifecycle = AcpLifecycle::new(
            observer,
            self.diagnostic_redactor_for_cwds(std::slice::from_ref(&cwd_text)),
            stderr,
        );
        lifecycle
            .record(
                DiagnosticPhase::SessionCreate,
                PhaseStatus::Started,
                None,
                None,
                None,
            )
            .await?;
        let cx = match self.cx() {
            Ok(cx) => cx,
            Err(error) => {
                AcpTraceEvent::DiscoverySessionCreateFailed.emit();
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::SessionCreate,
                        Some(DiagnosticPhase::Authenticate),
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::RetrySameTarget,
                        "acp.discovery.session_create.transport",
                        "ACP discovery session creation failed",
                        Some(error.to_string()),
                        false,
                        None,
                        None,
                        None,
                    )
                    .await);
            }
        };
        // session/new with NO MCP servers — discovery configures nothing.
        let req = Self::new_session_request(cwd.to_path_buf(), &[]);
        let resp = match cx.send_request(req).block_task().await {
            Ok(resp) => resp,
            Err(error) => {
                AcpTraceEvent::DiscoverySessionCreateFailed.emit();
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::SessionCreate,
                        Some(DiagnosticPhase::Authenticate),
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::RetrySameTarget,
                        "acp.discovery.session_create.transport",
                        "ACP discovery session creation failed",
                        Some(error.to_string()),
                        false,
                        None,
                        None,
                        None,
                    )
                    .await);
            }
        };
        lifecycle
            .record(
                DiagnosticPhase::SessionCreate,
                PhaseStatus::Completed,
                None,
                None,
                None,
            )
            .await?;
        let opts0 = resp.config_options.unwrap_or_default();
        let mut caps = if !opts0.is_empty() {
            caps_from_config_options(&opts0)
        } else {
            AgentCaps::default()
        };
        Self::merge_session_modes(&mut caps, resp.modes);
        Ok(caps)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn merge_session_modes(caps: &mut AgentCaps, modes: Option<SessionModeState>) {
        let Some(modes) = modes else {
            return;
        };
        caps.current_mode = Some(modes.current_mode_id.0.to_string());
        caps.modes = modes
            .available_modes
            .into_iter()
            .map(|mode| mode.id.0.to_string())
            .collect();
    }

    /// The configured cancel grace (see [`AcpConfig::cancel_grace`]). Falls back
    /// to the default if no config is set (the `conn: None` test-only path).
    fn cancel_grace(&self) -> std::time::Duration {
        self.config
            .as_ref()
            .map(|c| c.cancel_grace)
            .unwrap_or(DEFAULT_CANCEL_GRACE)
    }

    async fn operation_lifecycle(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> AcpLifecycle {
        let stderr = self
            .stderr_ring
            .as_ref()
            .map(|ring| (ring.clone(), ring.cursor()));
        AcpLifecycle::new(
            observer,
            self.diagnostic_redactor_for_operation(session).await,
            stderr,
        )
    }

    async fn release_session_result(&self, session: &SessionId) -> Result<(), TeardownFailure> {
        // Cleanup is unconditional: even a transport error while sending cancel
        // must not retain stale per-session routing/config state. The error is
        // still returned to the observed owner instead of being discarded.
        let cancel_result = self.cancel_inner(session).await;
        {
            let mut sessions = self.sessions.lock().await;
            if sessions.contains_key(session) {
                // ACP has no session-close acknowledgement here. Once bridge
                // ownership is removed, the shared process can still emit late
                // text derived from this session's MCP environment, while a
                // future mint will replace the finite exact-value policy.
                // Fail closed before removal so that replacement can retain
                // counts but can never re-enable process stderr text.
                if let Some(stderr_ring) = self.stderr_ring.as_ref() {
                    stderr_ring.retain_metadata_only();
                }
            }
            sessions.remove(session);
        }
        if let Ok(mut configs) = self.session_cfg.lock() {
            configs.remove(session);
        }
        self.pending_turn_meta
            .lock()
            .expect("pending_turn_meta lock")
            .remove(session);
        cancel_result.map(|_| ())
    }

    /// Last-resort escalation when a cancelled turn does not complete within the
    /// grace window: TAKE the supervised child (exactly once — a concurrent
    /// escalator sees `None`) and SIGTERM→SIGKILL the whole agent PROCESS.
    ///
    /// NOTE this NUKES THE ENTIRE AGENT CONNECTION, not just the one turn: this
    /// backend multiplexes all bridge sessions over a single agent process, so
    /// there is no per-turn kill. It is the acceptable last resort for a hung
    /// agent that ignores `session/cancel` — killing it closes the stdio pipes,
    /// which makes every in-flight `send_request` error out, so each driver
    /// surfaces `Err` and releases its turn lock (no caller hangs forever).
    ///
    /// On the in-process `connect` test path `supervised` is `None`, so this is a
    /// no-op there (closing the duplex channel is the test's own concern).
    /// `terminate(self, _)` is async + consumes the child, so we run it on a
    /// detached task and return immediately.
    fn close_connection_fence(dispatch_gate: &Arc<StdMutex<()>>, unavailable: &Arc<AtomicBool>) {
        let _dispatch = dispatch_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unavailable.store(true, Ordering::SeqCst);
    }

    fn escalate_terminate(
        supervised: &Arc<StdMutex<Option<Supervised>>>,
        container: &Option<ReapController>,
        reaped: &Arc<AtomicBool>,
        dispatch_gate: &Arc<StdMutex<()>>,
        unavailable: &Arc<AtomicBool>,
    ) {
        Self::close_connection_fence(dispatch_gate, unavailable);
        let taken = supervised.lock().ok().and_then(|mut g| g.take());
        if let Some(child) = taken {
            tokio::spawn(async move {
                child.terminate(TERMINATE_GRACE).await;
            });
        }
        // Site B: the agent PROCESS is being nuked → reap its `:ro` container (idempotent; no-op if none).
        AcpBackend::reap_container(container, reaped);
    }

    /// Reap the agent's `:ro` container (idempotent; no-op when `container` is `None`). Called from every
    /// teardown site (spawn-failure, escalate_terminate, retire, Drop) — at most one `docker rm -f` total.
    fn reap_container(container: &Option<ReapController>, reaped: &Arc<AtomicBool>) {
        if let Some(c) = container {
            reaped.store(true, Ordering::SeqCst);
            c.reap_detached();
        }
    }

    /// Map an inbound `session/update` notification (the agent→client streaming
    /// direction) to a modeled [`Update`], or `None` for an unmodeled variant.
    ///
    /// This is the SINGLE inbound-frame mapping seam: the live `on_receive_notification`
    /// handler registered in [`Self::connect`] calls THIS function, and so does the
    /// captured-agent corpus replay test. So a REAL `agent_message_chunk` frame
    /// captured off the wire is parsed (SDK `SessionNotification` deserialization)
    /// and mapped through the exact same code that runs in production — that is what
    /// makes the corpus replay a real conformance proof, not a circular one.
    ///
    /// `agent_message_chunk` with text content is modeled as `Update::Text`, and
    /// `usage_update` is modeled as `Update::Usage`. Every other `SessionUpdate`
    /// variant (thought chunks, plans, tool-call updates, ...) and non-text content
    /// is a tolerant-reader DROP (`None`).
    #[must_use]
    pub fn map_session_update_rich(notif: &SessionNotification) -> Option<OrchEventKind> {
        match &notif.update {
            SessionUpdate::Plan(plan) => Some(OrchEventKind::Plan {
                entries: plan
                    .entries
                    .iter()
                    .take(RICH_VEC_CAP)
                    .map(|entry| BridgePlanEntry {
                        content: Self::cap(&entry.content),
                        priority: Self::plan_entry_priority_str(&entry.priority).to_string(),
                        status: Self::plan_entry_status_str(&entry.status).to_string(),
                    })
                    .collect(),
            }),
            SessionUpdate::ToolCall(tool_call) => Some(OrchEventKind::ToolCall {
                // tool_call_id is agent-controlled + persisted into event_json + the wire frame:
                // cap it at MAP time like every other persisted string (FIX-11).
                tool_call_id: Self::cap(&tool_call.tool_call_id.to_string()),
                title: Self::cap(&tool_call.title),
                kind: Self::tool_kind_str(&tool_call.kind).to_string(),
                status: Self::tool_call_status_str(&tool_call.status).to_string(),
                locations: Self::map_tool_call_locations(&tool_call.locations),
                content: Self::map_tool_call_content(&tool_call.content),
            }),
            SessionUpdate::ToolCallUpdate(update) => Some(OrchEventKind::ToolCallUpdate {
                tool_call_id: Self::cap(&update.tool_call_id.to_string()),
                title: update.fields.title.as_deref().map(Self::cap),
                kind: update
                    .fields
                    .kind
                    .as_ref()
                    .map(|kind| Self::tool_kind_str(kind).to_string()),
                status: update
                    .fields
                    .status
                    .as_ref()
                    .map(|status| Self::tool_call_status_str(status).to_string()),
                locations: update
                    .fields
                    .locations
                    .as_ref()
                    .map(|locations| Self::map_tool_call_locations(locations)),
                content: update
                    .fields
                    .content
                    .as_ref()
                    .map(|content| Self::summarize_tool_call_content(content)),
            }),
            _ => None,
        }
    }

    fn cap(s: &str) -> String {
        if s.len() <= RICH_CONTENT_CAP {
            return s.to_string();
        }

        let mut end = 0;
        for (idx, ch) in s.char_indices() {
            let next = idx + ch.len_utf8();
            if next > RICH_CONTENT_CAP {
                break;
            }
            end = next;
        }
        s[..end].to_string()
    }

    fn map_tool_call_locations(locations: &[ToolCallLocation]) -> Vec<String> {
        locations
            .iter()
            .take(RICH_VEC_CAP)
            .map(|loc| Self::cap(loc.path.to_string_lossy().as_ref()))
            .collect()
    }

    fn map_tool_call_content(content: &[ToolCallContent]) -> Option<ContentSummary> {
        if content.is_empty() {
            None
        } else {
            Some(Self::summarize_tool_call_content(content))
        }
    }

    fn summarize_tool_call_content(content: &[ToolCallContent]) -> ContentSummary {
        let preview = content
            .iter()
            .take(RICH_VEC_CAP)
            .map(Self::tool_call_content_preview)
            .collect::<Vec<_>>()
            .join("\n");
        ContentSummary {
            item_count: content.len(),
            preview: Self::cap(&preview),
        }
    }

    fn tool_call_content_preview(content: &ToolCallContent) -> String {
        match content {
            ToolCallContent::Content(content) => match &content.content {
                ContentBlock::Text(t) => Self::cap(&t.text),
                _ => "[non-text]".to_string(),
            },
            ToolCallContent::Diff(diff) => {
                let added = diff.new_text.lines().count();
                let removed = diff
                    .old_text
                    .as_deref()
                    .map(|old| old.lines().count())
                    .unwrap_or(0);
                Self::cap(&format!(
                    "{} (+{}/-{})",
                    diff.path.to_string_lossy(),
                    added,
                    removed
                ))
            }
            ToolCallContent::Terminal(_) => "[terminal]".to_string(),
            _ => "other".to_string(),
        }
    }

    fn plan_entry_priority_str(priority: &PlanEntryPriority) -> &'static str {
        match priority {
            PlanEntryPriority::High => "high",
            PlanEntryPriority::Medium => "medium",
            PlanEntryPriority::Low => "low",
            _ => "other",
        }
    }

    fn plan_entry_status_str(status: &PlanEntryStatus) -> &'static str {
        match status {
            PlanEntryStatus::Pending => "pending",
            PlanEntryStatus::InProgress => "in_progress",
            PlanEntryStatus::Completed => "completed",
            _ => "other",
        }
    }

    fn tool_kind_str(kind: &ToolKind) -> &'static str {
        match kind {
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Execute => "execute",
            ToolKind::Think => "think",
            ToolKind::Fetch => "fetch",
            ToolKind::SwitchMode => "switch_mode",
            ToolKind::Other => "other",
            _ => "other",
        }
    }

    fn tool_call_status_str(status: &ToolCallStatus) -> &'static str {
        match status {
            ToolCallStatus::Pending => "pending",
            ToolCallStatus::InProgress => "in_progress",
            ToolCallStatus::Completed => "completed",
            ToolCallStatus::Failed => "failed",
            _ => "other",
        }
    }

    #[must_use]
    pub fn map_session_update(notif: SessionNotification) -> Option<Update> {
        match notif.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let ContentBlock::Text(t) = chunk.content {
                    return Some(Update::Text(t.text));
                }
                None
            }
            // Slice 2: surface context-window usage. Clock-free (at_ms stamped downstream at record_usage)
            // so the corpus-replay conformance test stays deterministic.
            SessionUpdate::UsageUpdate(u) => {
                Some(Update::Usage(bridge_core::orch::UsageSnapshot {
                    used: Some(u.used),
                    size: Some(u.size),
                    cost: u.cost.map(|c| bridge_core::orch::UsageCost {
                        amount: c.amount,
                        currency: c.currency,
                    }),
                    terminal: None,
                    at_ms: 0,
                }))
            }
            _ => None, // tolerant reader: unmodeled variants / non-text chunk content
        }
    }

    fn map_prompt_response_usage(
        usage: agent_client_protocol::schema::v1::Usage,
    ) -> bridge_core::orch::UsageSnapshot {
        bridge_core::orch::UsageSnapshot {
            used: None,
            size: None,
            cost: None,
            terminal: Some(bridge_core::orch::TerminalUsage {
                total_tokens: usage.total_tokens,
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                thought_tokens: usage.thought_tokens,
                cached_read_tokens: usage.cached_read_tokens,
                cached_write_tokens: usage.cached_write_tokens,
            }),
            at_ms: 0,
        }
    }

    /// Map an ACP `StopReason` to the bridge's `Update::Done` stop_reason string.
    /// We use the ACP wire spelling (snake_case) so it matches the protocol and
    /// the existing `Update::Done { stop_reason: String }` convention (e.g.
    /// `end_turn`, `max_tokens`, `cancelled`). The enum is `#[non_exhaustive]`,
    /// so an unknown future variant maps to `"unknown"` rather than failing.
    fn stop_reason_str(stop: StopReason) -> String {
        match stop {
            StopReason::EndTurn => "end_turn",
            StopReason::MaxTokens => "max_tokens",
            StopReason::MaxTurnRequests => "max_turn_requests",
            StopReason::Refusal => "refusal",
            StopReason::Cancelled => STOP_REASON_CANCELLED,
            _ => "unknown",
        }
        .to_string()
    }

    // ── Corpus-replay seams ──────────────────────────────────────────────────
    //
    // These thin wrappers expose the production inbound mapping over the DEFAULT
    // (auto-approve) policy / the wire `stop_reason` mapping, so the captured-agent
    // frame corpus replay test (`tests/corpus_replay.rs`) can feed a REAL frame
    // through the EXACT logic the live connection runs — without standing up a full
    // transport. They add no new behavior; they only re-expose existing code.

    /// Decide a reverse `session/request_permission` under the DEFAULT auto-approve
    /// policy (the deployed 3a policy), for the corpus replay test. Drives the same
    /// [`Self::decide_permission`] the live permission handler calls.
    #[must_use]
    pub fn decide_for_corpus(req: &RequestPermissionRequest) -> RequestPermissionOutcome {
        let policy: PolicyHandle = Arc::new(StdMutex::new(
            Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
        ));
        Self::decide_permission(&policy, req)
    }

    /// Map a prompt-result [`StopReason`] to the wire `stop_reason` string, for the
    /// corpus replay test. Drives the same [`Self::stop_reason_str`] the prompt
    /// driver uses to build `Update::Done`.
    #[must_use]
    pub fn stop_reason_for_corpus(stop: StopReason) -> String {
        Self::stop_reason_str(stop)
    }
}

// ── AgentBackend impl ────────────────────────────────────────────────────────

impl AcpBackend {
    /// Conformant streaming `session/prompt`.
    ///
    /// 1. `ensure_session` mints/gets the agent session id (lazy, exactly-once).
    /// 2. Register an mpsc `Sender` in the routing registry keyed by the agent
    ///    id BEFORE sending the prompt, so the notification handler can route
    ///    this turn's `agent_message_chunk`s and no chunk races past
    ///    registration.
    /// 3. Spawn a driver task that holds the per-session turn lock for the WHOLE
    ///    streamed turn and `block_task().await`s the `PromptResponse`; the SDK
    ///    delivers chunks meanwhile via the handler → the registered `Sender`.
    /// 4. On the response, the driver unregisters the `Sender` and pushes a
    ///    terminal event, then releases the turn lock (by dropping the guard it
    ///    owns). A completed turn (incl. a real `StopReason::Cancelled`) pushes
    ///    `TurnEvent::Done` → stream yields `Ok(Update::Done{stop_reason})`. A
    ///    `session/prompt` `Err` (agent crash / transport failure) pushes
    ///    `TurnEvent::Failed` → stream yields a terminal `Err` so downstream
    ///    reports the A2A caller `Failed` (NOT a silent `Done{"unknown"}` that
    ///    would read as a clean `Completed`).
    ///
    /// The returned `BackendStream` yields the streamed `Update::Text`s in order,
    /// then exactly one terminal item: `Ok(Update::Done)` on success, or `Err`
    /// on a transport/agent failure.
    async fn prompt_inner(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
        rich_sink: Option<Arc<dyn RichEventSink>>,
        diagnostic_observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendStream, BridgeError> {
        let config_snapshot = self.session_config_snapshot(session)?;
        #[cfg(test)]
        {
            let hook = self
                .prompt_snapshot_hook
                .lock()
                .ok()
                .and_then(|hook| hook.clone());
            if let Some(hook) = hook {
                hook();
            }
        }
        let stderr_evidence = self
            .stderr_ring
            .as_ref()
            .map(|ring| (ring.clone(), ring.cursor()));
        let lifecycle = AcpLifecycle::new(
            diagnostic_observer.clone(),
            self.diagnostic_redactor_for_cwds(std::slice::from_ref(&config_snapshot.desired_cwd)),
            stderr_evidence,
        );
        self.require_prompt_connection(&lifecycle).await?;
        let stderr_cursor: Option<ProcessStderrCursor> =
            self.stderr_ring.as_ref().map(ProcessStderrRing::cursor);
        let turn_meta = self.take_pending_turn_meta(session);

        // (1) Mint/get the agent session id. Done OUTSIDE the turn lock so a
        // first-prompt's `session/new` doesn't hold the lock while awaiting.
        let entry = self.session_entry(session).await;
        let agent_id = self
            .ensure_session_observed_with_snapshot(session, diagnostic_observer, config_snapshot)
            .await?;

        // Acquire the turn lock as an OWNED guard so it can move into the driver
        // task and be held for the whole streamed turn (released on drop there).
        let turn_guard: OwnedMutexGuard<()> = Arc::clone(&entry.turn_lock).lock_owned().await;
        // A caller can pass the first fence check and then queue behind a turn
        // that escalates the shared connection. Revalidate after acquiring the
        // lock so that queued caller cannot dispatch onto a killed/fenced agent.
        self.require_prompt_connection(&lifecycle).await?;
        lifecycle
            .record(
                DiagnosticPhase::PromptStart,
                PhaseStatus::Started,
                None,
                None,
                None,
            )
            .await?;

        // Build the per-turn channel and register its sender BEFORE sending the
        // prompt, so the handler routes every chunk (no drop between send and
        // registration). The driver keeps a clone of the sender to push the
        // terminal Done onto the SAME channel the chunks flow through (so the
        // stream yields chunks then Done, in order).
        let (tx, rx) = mpsc::unbounded_channel::<TurnEvent>();
        let done_sender = tx.clone();
        let (registry, cx) = match self.conn.as_ref() {
            Some(conn) => (Arc::clone(&conn.updates), conn.cx.clone()),
            None => {
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::PromptStart,
                        Some(DiagnosticPhase::ConfigApply),
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::Fatal,
                        "acp.prompt_start.connection_unavailable",
                        "ACP prompt connection was unavailable",
                        None,
                        false,
                        None,
                        None,
                        None,
                    )
                    .await)
            }
        };
        let watch = if self
            .config
            .as_ref()
            .and_then(|c| c.watchdog.as_ref())
            .is_some()
        {
            Some(Arc::new(TurnWatch {
                turn_start: Instant::now(),
                last_activity_ms: AtomicU64::new(0),
            }))
        } else {
            None
        };
        if registry.is_poisoned() {
            return Err(lifecycle
                .failure(
                    DiagnosticPhase::PromptStart,
                    Some(DiagnosticPhase::ConfigApply),
                    DiagnosticFailureClass::Unknown,
                    FailureDisposition::Fatal,
                    "acp.prompt_start.registry_poisoned",
                    "ACP prompt routing lock was unavailable",
                    None,
                    false,
                    None,
                    None,
                    None,
                )
                .await);
        }
        let turn_active = Arc::new(AtomicBool::new(true));
        let prompt_accepted = Arc::new(AtomicBool::new(false));
        let turn_acceptance = TurnAcceptanceGuard::install(
            Arc::clone(&entry.turn_accepted),
            Arc::clone(&prompt_accepted),
        );
        {
            let mut map = registry.lock().expect("update routing registry lock");
            map.insert(
                agent_id.clone(),
                TurnRoute {
                    tx,
                    watch: watch.clone(),
                    turn_meta,
                    cancelled: Arc::new(AtomicBool::new(false)),
                    active: Arc::clone(&turn_active),
                },
            );
        }
        let req = Self::prompt_request(agent_id.clone(), &parts);

        // Install a FRESH per-turn kill switch on the session: the external cancel
        // grace-watcher fires it to unblock a hung driver (see `cancel`). The
        // driver `select!`s on it and clears the slot on exit.
        let kill = Arc::new(tokio::sync::Notify::new());
        *entry.turn_kill.lock().expect("turn_kill lock") = Some(Arc::clone(&kill));

        // This is the accepted-work barrier. The connection-wide gate makes
        // the final availability sample and SDK request installation one
        // atomic ordering point with escalation and retirement. All observer
        // awaits are complete before this short synchronous critical section.
        let prompt_fut = Self::install_prompt_request(
            &self.dispatch_gate,
            &self.unavailable,
            &prompt_accepted,
            || cx.send_request(req).block_task(),
        );
        let Some(prompt_fut) = prompt_fut else {
            turn_active.store(false, Ordering::SeqCst);
            if let Ok(mut map) = registry.lock() {
                map.remove(&agent_id);
            }
            if let Ok(mut slot) = entry.turn_kill.lock() {
                *slot = None;
            }
            return Err(lifecycle
                .failure(
                    DiagnosticPhase::PromptStart,
                    Some(DiagnosticPhase::ConfigApply),
                    DiagnosticFailureClass::AgentProcess,
                    FailureDisposition::Fatal,
                    "acp.prompt_start.connection_terminated",
                    "ACP connection terminated before prompt dispatch",
                    None,
                    false,
                    None,
                    None,
                    None,
                )
                .await);
        };

        // (3) Driver: holds the turn lock for the whole streamed turn (it OWNS
        // `turn_guard`, releasing the lock only when it finishes) and awaits the
        // `PromptResponse`; the SDK delivers chunks meanwhile via the handler.
        let registry_for_driver = Arc::clone(&registry);
        let agent_id_for_driver = agent_id.clone();
        let supervised_for_driver = Arc::clone(&self.supervised);
        let container_for_driver = self.container_reap.clone();
        let reaped_for_driver = Arc::clone(&self.reaped);
        let unavailable_for_driver = Arc::clone(&self.unavailable);
        let dispatch_gate_for_driver = Arc::clone(&self.dispatch_gate);
        let stderr_ring_for_driver = self.stderr_ring.clone();
        let kill_slot = Arc::clone(&entry.turn_kill);
        let turn_acceptance_for_driver = turn_acceptance;
        let grace = self.cancel_grace();
        let watchdog_cfg = self.config.as_ref().and_then(|c| c.watchdog.clone());
        let (watchdog_fired, watchdog_done_tx) = if let (Some(watchdog_cfg), Some(watch)) =
            (watchdog_cfg, watch.as_ref())
        {
            let watchdog_fired = Arc::new(tokio::sync::Notify::new());
            let watchdog_fired_for_task = Arc::clone(&watchdog_fired);
            let watch = Arc::clone(watch);
            let (done_tx, mut done_rx) = oneshot::channel::<()>();
            tokio::spawn(async move {
                loop {
                    let wall_deadline = watch.turn_start + watchdog_cfg.hard_wall_clock;
                    let la = watch.last_activity_ms.load(Ordering::Relaxed);
                    let idle_deadline = if la != 0 {
                        let la_instant = watch.turn_start
                            + std::time::Duration::from_millis(la.saturating_sub(1));
                        la_instant + watchdog_cfg.idle_timeout
                    } else {
                        wall_deadline
                    };
                    let deadline = std::cmp::min(wall_deadline, idle_deadline);

                    tokio::select! {
                        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
                        _ = &mut done_rx => return,
                    }

                    let la = watch.last_activity_ms.load(Ordering::Relaxed);
                    let wall_elapsed = watch.turn_start.elapsed() >= watchdog_cfg.hard_wall_clock;
                    let idle_elapsed = if la != 0 {
                        let la_instant = watch.turn_start
                            + std::time::Duration::from_millis(la.saturating_sub(1));
                        Instant::now().saturating_duration_since(la_instant)
                            >= watchdog_cfg.idle_timeout
                    } else {
                        false
                    };
                    if wall_elapsed || idle_elapsed {
                        watchdog_fired_for_task.notify_one();
                        return;
                    }
                }
            });
            (Some(watchdog_fired), Some(done_tx))
        } else {
            (None, None)
        };
        tokio::spawn(async move {
            // Hold the turn lock for the entire turn.
            let _turn = turn_guard;
            // Clear operation-scoped acceptance evidence before `_turn` drops,
            // including task abort/unwind paths. The routing entry can be
            // removed earlier once the SDK result is terminal.
            let _turn_acceptance = turn_acceptance_for_driver;

            // Await the prompt result, but bail out on either:
            //   * the CONSUMER dropping the stream (`done_sender.closed()` resolves
            //     when the paired `rx`, moved into the returned `BackendStream`, is
            //     dropped — the A2A caller disconnected mid-turn); we must then
            //     cancel the agent turn rather than leave it running holding the
            //     turn lock; or
            //   * the external cancel grace-watcher firing the kill switch (a hung
            //     agent that ignored `session/cancel` past grace) — we abandon the
            //     await so the lock releases and the caller's stream ends.
            // The accepted-work barrier was crossed under the connection-wide
            // dispatch gate immediately before this task was spawned. Every
            // exit below is therefore post-barrier and fatal.
            tokio::pin!(prompt_fut);
            let persistence_error = match lifecycle
                .record(
                    DiagnosticPhase::PromptStart,
                    PhaseStatus::Completed,
                    None,
                    None,
                    None,
                )
                .await
            {
                Ok(()) => lifecycle
                    .record(
                        DiagnosticPhase::PromptStream,
                        PhaseStatus::Started,
                        None,
                        None,
                        None,
                    )
                    .await
                    .err(),
                Err(error) => Some(error),
            };
            if let Some(error) = persistence_error {
                // Work crossed the accepted-work barrier before persistence
                // failed. Keep the route and turn lock until the request is
                // terminal, or until bounded cancellation escalates the whole
                // shared connection. Returning immediately would let a second
                // prompt overlap an agent that ignored this best-effort cancel.
                let _ = cx.send_notification(CancelNotification::new(agent_id_for_driver.clone()));
                tokio::select! {
                    _ = &mut prompt_fut => {}
                    _ = kill.notified() => {
                        AcpBackend::close_connection_fence(
                            &dispatch_gate_for_driver,
                            &unavailable_for_driver,
                        );
                    }
                    _ = tokio::time::sleep(grace) => {
                        AcpBackend::escalate_terminate(
                            &supervised_for_driver,
                            &container_for_driver,
                            &reaped_for_driver,
                            &dispatch_gate_for_driver,
                            &unavailable_for_driver,
                        );
                    }
                }
                turn_active.store(false, Ordering::SeqCst);
                if let Ok(mut map) = registry_for_driver.lock() {
                    map.remove(&agent_id_for_driver);
                }
                if let Ok(mut slot) = kill_slot.lock() {
                    *slot = None;
                }
                drop(watchdog_done_tx);
                let _ = done_sender.send(TurnEvent::Failed(error));
                return;
            }
            let outcome: Result<_, PromptDriverFailure> = tokio::select! {
                // BIASED: poll the arms in order so a `prompt_fut` that became ready in the SAME
                // poll as a fired watchdog ALWAYS wins (the natural completion is never relabeled
                // AgentTimedOut). Without `biased`, tokio picks a ready arm at random.
                biased;
                outcome = &mut prompt_fut => outcome.map_err(PromptDriverFailure::Sdk),
                _ = kill.notified() => Err(PromptDriverFailure::KillSwitch),
                _ = async {
                    match &watchdog_fired {
                        Some(n) => n.notified().await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let _ = cx.send_notification(CancelNotification::new(
                        agent_id_for_driver.clone(),
                    ));
                    let cause = match settle_prompt_after_cancel(
                        prompt_fut.as_mut(),
                        kill.as_ref(),
                        grace,
                    )
                    .await
                    {
                        CancelSettle::Prompt(outcome) => {
                            outcome.err().map(|error| error.to_string())
                        }
                        CancelSettle::KillSwitch => None,
                        CancelSettle::GraceElapsed => {
                            AcpBackend::escalate_terminate(
                                &supervised_for_driver,
                                &container_for_driver,
                                &reaped_for_driver,
                                &dispatch_gate_for_driver,
                                &unavailable_for_driver,
                            );
                            None
                        }
                    };
                    Err(PromptDriverFailure::Watchdog(cause))
                },
                _ = done_sender.closed() => {
                    // Early stream-drop → cancel THIS turn's agent session, then
                    // CONTINUE awaiting the prompt result so the turn lock still
                    // releases when the agent finishes/errors. A hung agent that
                    // never returns after the cancel is bounded by a grace timer:
                    // on elapse we nuke the process (closing the pipes) AND treat
                    // the turn as failed so the stream ends even on the in-process
                    // transport (which has no process to kill).
                    let _ = cx.send_notification(CancelNotification::new(
                        agent_id_for_driver.clone(),
                    ));
                    match settle_prompt_after_cancel(
                        prompt_fut.as_mut(),
                        kill.as_ref(),
                        grace,
                    )
                    .await
                    {
                        CancelSettle::Prompt(outcome) => outcome
                            .map_err(PromptDriverFailure::Sdk),
                        CancelSettle::KillSwitch => Err(PromptDriverFailure::KillSwitch),
                        CancelSettle::GraceElapsed => {
                            AcpBackend::escalate_terminate(
                                &supervised_for_driver,
                                &container_for_driver,
                                &reaped_for_driver,
                                &dispatch_gate_for_driver,
                                &unavailable_for_driver,
                            );
                            Err(PromptDriverFailure::DroppedStreamTimeout)
                        }
                    }
                }
            };

            // Unregister this turn's sender FIRST so no late chunk is routed
            // after the terminal Done is emitted.
            turn_active.store(false, Ordering::SeqCst);
            if let Ok(mut map) = registry_for_driver.lock() {
                map.remove(&agent_id_for_driver);
            }
            drop(watchdog_done_tx);
            // Clear the kill switch slot now the turn is ending (next turn installs
            // its own); avoids a stale notify firing across turns.
            if let Ok(mut slot) = kill_slot.lock() {
                *slot = None;
            }
            let event = match outcome {
                // Turn COMPLETED (incl. a real StopReason::Cancelled, which maps
                // to Done{"cancelled"} — NOT an error). Emit the mapped Done.
                Ok(resp) => {
                    let stop_reason = AcpBackend::stop_reason_str(resp.stop_reason);
                    if let Err(error) = lifecycle
                        .record(
                            DiagnosticPhase::PromptStream,
                            PhaseStatus::Completed,
                            None,
                            None,
                            None,
                        )
                        .await
                    {
                        TurnEvent::Failed(error)
                    } else if let Err(error) = lifecycle
                        .record(
                            DiagnosticPhase::PromptFinish,
                            PhaseStatus::Started,
                            None,
                            None,
                            None,
                        )
                        .await
                    {
                        TurnEvent::Failed(error)
                    } else {
                        if let Some(usage) = resp.usage {
                            let _ = done_sender.send(TurnEvent::Usage(
                                AcpBackend::map_prompt_response_usage(usage),
                            ));
                        }
                        match lifecycle
                            .record(
                                DiagnosticPhase::PromptFinish,
                                PhaseStatus::Completed,
                                None,
                                None,
                                None,
                            )
                            .await
                        {
                            Ok(()) => TurnEvent::Done(Update::Done { stop_reason }),
                            Err(error) => TurnEvent::Failed(error),
                        }
                    }
                }
                // A transport/agent error (agent crash / mid-turn transport
                // failure), OR a kill-switch/grace escalation, FAILED the turn:
                // surface a terminal Err on the stream so downstream reports the
                // inbound A2A caller `Failed` — never a silent Done{"unknown"}
                // that reads as a clean `Completed`.
                Err(failure) => {
                    AcpTraceEvent::PromptFailed.emit();
                    let stderr = stderr_ring_for_driver
                        .as_ref()
                        .and_then(|ring| stderr_cursor.map(|cursor| ring.metadata_since(cursor)));
                    let (class, code, summary, cause, retry_after_ms, reset_at_ms) = match failure {
                        PromptDriverFailure::Sdk(error) => {
                            let ProviderEvidence {
                                class,
                                code,
                                retry_after_ms,
                                reset_at_ms,
                            } = classify_acp_error_data(
                                error.code == ErrorCode::AuthRequired,
                                error.data.as_ref(),
                                diagnostic_timestamp_ms(),
                            );
                            (
                                class,
                                code,
                                "ACP prompt failed",
                                Some(error.message),
                                retry_after_ms,
                                reset_at_ms,
                            )
                        }
                        PromptDriverFailure::KillSwitch => (
                            DiagnosticFailureClass::AgentProcess,
                            "acp.prompt.kill_switch",
                            "ACP prompt was terminated after cancellation grace",
                            None,
                            None,
                            None,
                        ),
                        PromptDriverFailure::Watchdog(cause) => (
                            DiagnosticFailureClass::Timeout,
                            "acp.prompt.watchdog_timeout",
                            "ACP prompt exceeded its watchdog bound",
                            cause,
                            None,
                            None,
                        ),
                        PromptDriverFailure::DroppedStreamTimeout => (
                            DiagnosticFailureClass::Timeout,
                            "acp.prompt.dropped_stream_timeout",
                            "ACP prompt did not stop after its consumer disconnected",
                            None,
                            None,
                            None,
                        ),
                    };
                    TurnEvent::Failed(
                        lifecycle
                            .failure_with_retry_metadata(
                                DiagnosticPhase::PromptStream,
                                Some(DiagnosticPhase::PromptStart),
                                class,
                                FailureDisposition::Fatal,
                                code,
                                summary,
                                cause,
                                retry_after_ms,
                                reset_at_ms,
                                true,
                                stderr,
                                None,
                                None,
                            )
                            .await,
                    )
                }
            };
            // If the consumer already dropped the stream this `send` is a no-op,
            // but the lock-release below is what matters there.
            let _ = done_sender.send(event);
            // `_turn` (the OwnedMutexGuard) drops here, releasing the turn lock.
        });

        // The returned stream drains the per-turn channel, mapping events to
        // `Update`s and terminating after the Done.
        let stream =
            futures::stream::unfold((rx, false, rich_sink), |(mut rx, done, sink)| async move {
                if done {
                    return None;
                }

                loop {
                    match rx.recv().await {
                        Some(TurnEvent::Rich(kind)) => {
                            if let Some(sink) = &sink {
                                sink.record(kind);
                            }
                            continue;
                        }
                        Some(TurnEvent::Text(t)) => {
                            return Some((Ok(Update::Text(t)), (rx, false, sink)));
                        }
                        Some(TurnEvent::Usage(snap)) => {
                            return Some((Ok(Update::Usage(snap)), (rx, false, sink)));
                        }
                        Some(TurnEvent::Done(u)) => return Some((Ok(u), (rx, true, sink))),
                        // Terminal failure: yield the Err as the final stream item, then
                        // end. Downstream re-yields the Err → producer marks `errored` →
                        // terminal frame is `TaskOutcome::Failed` (the correct path).
                        Some(TurnEvent::Failed(e)) => return Some((Err(e), (rx, true, sink))),
                        // Channel closed without a Done/Failed (driver dropped) — terminate.
                        None => return None,
                    }
                }
            });

        Ok(Box::pin(stream))
    }
}

#[async_trait]
impl AgentBackend for AcpBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            None,
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_observed(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
        sink: Arc<dyn RichEventSink>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(
            session,
            parts,
            Some(sink),
            Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
        )
        .await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(session, parts, observers.rich, observers.diagnostic)
            .await
    }

    async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
        self.pending_turn_meta
            .lock()
            .expect("pending_turn_meta lock")
            .insert(session.clone(), meta);
    }

    /// Cancel the in-flight turn for the bridge session.
    ///
    /// Spec §5.3 / Codex finding 2: cancellation COMPLETION is the prompt RESULT
    /// arriving on the `BackendStream` with `stopReason:"cancelled"` (→
    /// `Update::Done{"cancelled"}`), NOT the act of sending this notification.
    /// This method's job is therefore twofold:
    ///
    /// 1. `request_cancel` sends `session/cancel` for the in-flight turn's agent
    ///    session (honoring the create/cancel latch race). A well-behaved agent
    ///    then returns `StopReason::Cancelled`, the driver emits
    ///    `Update::Done{"cancelled"}`, and the caller's stream completes — that
    ///    completion is the contract, owned by the prompt driver (Task 3).
    ///
    /// 2. HUNG-AGENT bound: a real agent might NEVER return after `session/cancel`,
    ///    leaving the driver parked on `send_request` while it holds the per-turn
    ///    lock and the caller's stream hangs forever. So if an in-flight turn does
    ///    not complete within [`AcpConfig::cancel_grace`], we ESCALATE by
    ///    terminating the agent process (`escalate_terminate`): killing it closes
    ///    the stdio pipes → `send_request` errors → the driver surfaces `Err`,
    ///    releases the lock, and the stream ends. We detect "turn completed" by
    ///    re-acquiring the per-session `turn_lock` (the driver holds it for the
    ///    whole turn and drops it on EVERY exit), so a successful lock-acquire
    ///    within grace means the turn finished and no escalation is needed.
    ///
    /// The grace watcher runs on a detached task so `cancel` stays prompt (it does
    /// not block the caller for the grace window). If no turn is in flight (the
    /// lock is free right now), there is nothing to bound and we skip the watcher.
    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.cancel_inner(session)
            .await
            .map(|_| ())
            .map_err(|failure| failure.error)
    }

    async fn cancel_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let lifecycle = self.operation_lifecycle(session, observer).await;
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Started,
                None,
                Some("acp.teardown.cancel"),
                None,
            )
            .await?;
        let dispatch = match self.cancel_inner(session).await {
            Ok(dispatch) => dispatch,
            Err(TeardownFailure {
                error,
                prompt_may_have_been_accepted,
            }) => {
                return Err(lifecycle
                    .failure(
                        DiagnosticPhase::Teardown,
                        None,
                        DiagnosticFailureClass::Transport,
                        FailureDisposition::Fatal,
                        "acp.teardown.cancel_failed",
                        "ACP session cancellation failed",
                        Some(error.to_string()),
                        prompt_may_have_been_accepted,
                        None,
                        None,
                        None,
                    )
                    .await)
            }
        };
        let completion_code = match dispatch {
            CancelDispatch::Delivered => "acp.teardown.cancelled",
            CancelDispatch::Latched => "acp.teardown.cancel_latched",
        };
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Completed,
                None,
                Some(completion_code),
                None,
            )
            .await
    }

    /// Stash the per-session spec (Increment 3b/session-cwd). Dispatch (T10) calls
    /// this BEFORE the first prompt for a session; [`Self::ensure_session`] reads
    /// the stash at lazy mint (keyed by the bridge `SessionId`) and applies
    /// `mode`/`model`/`effort` from `spec.config`. Insert-or-replace, so a
    /// re-`configure_session` with fresh spec (e.g. after a live registry edit) takes
    /// effect on the NEXT mint. Cheap + non-async under a plain `Mutex`.
    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        if let Ok(mut m) = self.session_cfg.lock() {
            m.insert(session.clone(), spec.clone());
        }
        Ok(())
    }

    /// Drop the per-session config stash entry when a task/session ends (T11
    /// inbound-binding eviction). A no-op if `session` was never configured. We
    /// intentionally do NOT touch the agent-side `sessions` map here: the ACP
    /// connection multiplexes all sessions and tearing the agent session down is
    /// the retirement/`Drop` path's concern, not per-session config forgetting.
    async fn forget_session(&self, session: &SessionId) {
        if let Ok(mut m) = self.session_cfg.lock() {
            m.remove(session);
        }
        self.pending_turn_meta
            .lock()
            .expect("pending_turn_meta lock")
            .remove(session);
    }

    async fn forget_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        self.forget_session(session).await;
        Ok(())
    }

    async fn forget_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let lifecycle = self.operation_lifecycle(session, observer).await;
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Started,
                None,
                Some("acp.teardown.forget"),
                None,
            )
            .await?;
        self.forget_session(session).await;
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Completed,
                None,
                Some("acp.teardown.forgotten"),
                None,
            )
            .await
    }

    /// Re-apply warm-session model/effort against the cached ACP config surface.
    /// cwd/mode are mint-only here; callers route those before asking for a warm
    /// reconcile.
    async fn reconcile_config(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<ReconcileOutcome, BridgeError> {
        let entry = self.session_entry(session).await;
        if entry.agent_id.get().is_none() {
            self.configure_session(session, spec).await?;
            let _aid = self.ensure_session(session).await?;
            return Ok(ReconcileOutcome::Applied);
        }

        // Already minted (checked above): read the live id DIRECTLY from this entry — do NOT call
        // `ensure_session` (it re-fetches the map by key, racing `release_session`, and re-runs the
        // `minted_cwd` immutability guard). We only reconcile model/effort on the live session.
        let aid =
            entry.agent_id.get().cloned().ok_or_else(|| {
                BridgeError::agent_crashed("warm reconcile: agent session vanished")
            })?;
        let _g = Arc::clone(&entry.turn_lock).lock_owned().await;
        let surface = entry
            .config_surface
            .lock()
            .expect("config_surface lock")
            .clone()
            .unwrap_or_default();
        let agent_id = self
            .config
            .as_ref()
            .map(|c| c.agent_id.clone())
            .unwrap_or_default();
        let cx = self.cx()?;
        match Self::apply_model_effort(
            cx,
            &aid,
            &agent_id,
            &surface,
            spec.config.model.as_deref(),
            spec.config.effort,
            ApplyPurpose::Warm,
        )
        .await
        {
            Ok((refreshed, _)) => {
                *entry.config_surface.lock().expect("config_surface lock") = Some(refreshed);
                Ok(ReconcileOutcome::Applied)
            }
            Err(ApplyConfigError::NotAdvertised(b)) => {
                let _ = b;
                AcpTraceEvent::WarmConfigNotAdvertised.emit();
                Ok(ReconcileOutcome::NotAdvertised)
            }
            Err(ApplyConfigError::Rejected(b)) => {
                let _ = b;
                AcpTraceEvent::WarmConfigRejected.emit();
                Ok(ReconcileOutcome::Rejected)
            }
        }
    }

    fn capabilities(&self) -> AgentSessionCaps {
        match self.agent_capabilities() {
            Some(c) => AgentSessionCaps {
                load_session: c.load_session,
                resume: c.session_capabilities.resume.is_some(),
                close: c.session_capabilities.close.is_some(),
                list: c.session_capabilities.list.is_some(),
                delete: false,
            },
            None => AgentSessionCaps::default(),
        }
    }

    /// Release a warm ACP session: best-effort cancel any in-flight turn, drop the
    /// agent-side `AgentSession` (a later reuse re-mints a fresh `session/new`), and drop
    /// the config stash. Does NOT `retire()` the shared process (warm for serve's lifetime,
    /// shared across sessions). [Slice 0]
    async fn release_session(&self, session: &SessionId) {
        let _ = self.release_session_checked(session).await;
    }

    async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        let container = self.container_reap.clone();
        let lifecycle = if container.is_some() {
            Some(
                self.operation_lifecycle(
                    session,
                    Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
                )
                .await,
            )
        } else {
            None
        };
        if let Some(container) = &container {
            self.reaped.store(true, Ordering::SeqCst);
            container.reap_detached();
        }
        if let Some(lifecycle) = &lifecycle {
            lifecycle
                .record(
                    DiagnosticPhase::Teardown,
                    PhaseStatus::Started,
                    None,
                    Some("acp.teardown.release"),
                    None,
                )
                .await?;
        }
        let release_result = self.release_session_result(session).await;
        let reap_result = match &container {
            Some(container) => container.reap_observed().await,
            None => Ok(()),
        };
        if let Err(failure) = release_result {
            return Err(failure.error);
        }
        if let Err(failure) = reap_result {
            return Err(lifecycle
                .expect("container cleanup lifecycle")
                .failure(
                    DiagnosticPhase::Teardown,
                    None,
                    DiagnosticFailureClass::ContainerRuntime,
                    FailureDisposition::Fatal,
                    failure.code(),
                    "Container removal failed",
                    None,
                    true,
                    None,
                    None,
                    None,
                )
                .await);
        }
        Ok(())
    }

    async fn release_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let lifecycle = self.operation_lifecycle(session, observer).await;
        let container = self.container_reap.clone();

        // A `:ro` release owns process cleanup, not just the ACP session map.
        // Select/start the observer-free flight before the first cancellable
        // diagnostic write. If that write is rejected, cleanup still runs and
        // this waiter returns only after the shared reap has settled.
        if let Some(container) = &container {
            self.reaped.store(true, Ordering::SeqCst);
            container.reap_detached();
        }

        let start_result = lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Started,
                None,
                Some("acp.teardown.release"),
                None,
            )
            .await;
        if container.is_none() {
            if let Err(error) = &start_result {
                // Preserve the existing persistence-first behavior for the
                // shared, non-container ACP backend.
                return Err(error.clone());
            }
        }
        let release_result = self.release_session_result(session).await;
        let reap_result = match &container {
            Some(container) => container.reap_observed().await,
            None => Ok(()),
        };

        start_result?;
        if let Err(TeardownFailure {
            error,
            prompt_may_have_been_accepted,
        }) = release_result
        {
            return Err(lifecycle
                .failure(
                    DiagnosticPhase::Teardown,
                    None,
                    DiagnosticFailureClass::Transport,
                    FailureDisposition::Fatal,
                    "acp.teardown.release_failed",
                    "ACP session release failed",
                    Some(error.to_string()),
                    prompt_may_have_been_accepted,
                    None,
                    None,
                    None,
                )
                .await);
        }
        if let Err(failure) = reap_result {
            return Err(lifecycle
                .failure(
                    DiagnosticPhase::Teardown,
                    None,
                    DiagnosticFailureClass::ContainerRuntime,
                    FailureDisposition::Fatal,
                    failure.code(),
                    "Container removal failed",
                    None,
                    true,
                    None,
                    None,
                    None,
                )
                .await);
        }
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Completed,
                None,
                Some(if container.is_some() {
                    "acp.teardown.container_reaped"
                } else {
                    "acp.teardown.released"
                }),
                None,
            )
            .await
    }

    /// Graceful async teardown of the agent process (Increment 3b §5.4). IDEMPOTENT
    /// by construction: TAKE the `Supervised` child out of the shared slot (exactly
    /// once — a concurrent/second caller sees `None`) and SIGTERM→SIGKILL it. Both
    /// `resolve`'s race-loss path AND the registry retirement task may call this on
    /// the same backend (T5 review), so the take-once makes the second call a clean
    /// no-op. The connection fence closes before either path so an active lease
    /// cannot install new work while retirement tears down the shared agent.
    async fn retire(&self) -> Result<(), BridgeError> {
        Self::close_connection_fence(&self.dispatch_gate, &self.unavailable);
        // Registry retirement must select/start process-owned container cleanup
        // before the cancellable graceful process termination await.
        let container = self.container_reap.clone();
        AcpBackend::reap_container(&container, &self.reaped);
        let sup = self.supervised.lock().ok().and_then(|mut g| g.take());
        if let Some(sup) = sup {
            sup.terminate(self.cancel_grace()).await;
        }
        Ok(())
    }
}

impl Drop for AcpBackend {
    /// Site D: the plain-drop path (normal workflow completion → registry drop). Reaps the `:ro` container
    /// if no earlier site already did. The shared controller's detached start is
    /// off-runtime-safe, so a Drop at process shutdown never panics.
    fn drop(&mut self) {
        let container = self.container_reap.clone();
        AcpBackend::reap_container(&container, &self.reaped);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, FailureDisposition, InMemoryDiagnosticObserver,
        PhaseStatus,
    };
    use bridge_core::domain::EffectiveConfig;
    use bridge_core::error::BridgeError;
    use bridge_core::ports::{AgentBackend, BackendObservers, RichEventSink, Update};
    use bridge_core::process::Supervised;
    use bridge_core::reaper::{ReapAttemptFn, ReapFailure};
    use bridge_core::SessionCwd;
    use futures::StreamExt;
    use std::time::Duration;

    // ── SDK connection path (transport-generic, in-process fake agent) ──────────

    use agent_client_protocol::schema::v1::{
        AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodId, InitializeRequest,
        InitializeResponse, SessionCapabilities, SessionCloseCapabilities, SessionListCapabilities,
        SessionResumeCapabilities,
    };
    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::{Agent, Channel};

    fn trace_source_violations(source: &str) -> Vec<String> {
        trace_source_violations_with_funnel(source, true)
    }

    fn trace_source_violations_with_funnel(source: &str, allow_typed_funnel: bool) -> Vec<String> {
        use syn::visit::Visit;

        struct Visitor {
            allow_typed_funnel: bool,
            module_depth: usize,
            inside_trace_impl: bool,
            inside_emit: bool,
            violations: Vec<String>,
        }

        fn use_tree_contains_tracing(tree: &syn::UseTree) -> bool {
            match tree {
                syn::UseTree::Path(path) => {
                    path.ident == "tracing" || use_tree_contains_tracing(&path.tree)
                }
                syn::UseTree::Name(name) => name.ident == "tracing",
                syn::UseTree::Rename(rename) => {
                    rename.ident == "tracing" || rename.rename == "tracing"
                }
                syn::UseTree::Group(group) => group.items.iter().any(use_tree_contains_tracing),
                syn::UseTree::Glob(_) => false,
            }
        }

        fn is_test_only_module(node: &syn::ItemMod) -> bool {
            node.attrs.iter().any(|attribute| match &attribute.meta {
                syn::Meta::List(list) if list.path.is_ident("cfg") => {
                    list.tokens.to_string() == "test"
                }
                _ => false,
            })
        }

        fn item_macro_contains_ident(node: &syn::ItemMacro, expected: &str) -> bool {
            node.mac
                .tokens
                .to_string()
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .any(|token| token == expected)
        }

        impl<'ast> Visit<'ast> for Visitor {
            fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
                if is_test_only_module(node) {
                    return;
                }
                let prior = self.module_depth;
                self.module_depth += 1;
                syn::visit::visit_item_mod(self, node);
                self.module_depth = prior;
            }

            fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
                let is_funnel = self.allow_typed_funnel
                    && self.module_depth == 0
                    && node.trait_.is_none()
                    && match node.self_ty.as_ref() {
                        syn::Type::Path(path) => {
                            path.qself.is_none()
                                && path.path.segments.len() == 1
                                && path.path.is_ident("AcpTraceEvent")
                        }
                        _ => false,
                    };
                let prior = self.inside_trace_impl;
                self.inside_trace_impl = is_funnel;
                syn::visit::visit_item_impl(self, node);
                self.inside_trace_impl = prior;
            }

            fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
                let prior = self.inside_emit;
                let exact_receiver = matches!(
                    node.sig.inputs.first(),
                    Some(syn::FnArg::Receiver(receiver))
                        if receiver.reference.is_none()
                            && receiver.mutability.is_none()
                            && receiver.colon_token.is_none()
                );
                self.inside_emit = self.inside_trace_impl
                    && node.sig.ident == "emit"
                    && node.sig.inputs.len() == 1
                    && exact_receiver
                    && node.sig.generics.params.is_empty()
                    && node.sig.asyncness.is_none()
                    && node.sig.unsafety.is_none()
                    && node.sig.abi.is_none()
                    && node.sig.variadic.is_none()
                    && matches!(node.sig.output, syn::ReturnType::Default);
                syn::visit::visit_impl_item_fn(self, node);
                self.inside_emit = prior;
            }

            fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
                if node
                    .path
                    .segments
                    .first()
                    .is_some_and(|segment| segment.ident == "tracing")
                {
                    self.violations
                        .push("direct tracing expression bypasses the typed funnel".to_string());
                }
                syn::visit::visit_expr_path(self, node);
            }

            fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
                if node
                    .path
                    .segments
                    .first()
                    .is_some_and(|segment| segment.ident == "tracing")
                {
                    self.violations
                        .push("direct tracing type bypasses the typed funnel".to_string());
                }
                syn::visit::visit_type_path(self, node);
            }

            fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
                if use_tree_contains_tracing(&node.tree) {
                    self.violations
                        .push("tracing import or alias bypasses the typed funnel".to_string());
                }
                syn::visit::visit_item_use(self, node);
            }

            fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
                if node.ident == "tracing" {
                    self.violations
                        .push("tracing extern-crate alias bypasses the typed funnel".to_string());
                }
                syn::visit::visit_item_extern_crate(self, node);
            }

            fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
                let segments: Vec<String> = node
                    .path()
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect();
                if segments.first().is_some_and(|name| name == "tracing")
                    || segments.last().is_some_and(|name| name == "instrument")
                {
                    self.violations
                        .push("tracing attribute bypasses the typed funnel".to_string());
                }
                syn::visit::visit_attribute(self, node);
            }

            fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
                if node.ident.is_some() && item_macro_contains_ident(node, "tracing") {
                    self.violations
                        .push("local macro expansion bypasses the typed trace funnel".to_string());
                }
                syn::visit::visit_item_macro(self, node);
            }

            fn visit_macro(&mut self, node: &'ast syn::Macro) {
                let segments: Vec<String> = node
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect();
                let is_log_macro = segments.last().is_some_and(|name| {
                    matches!(name.as_str(), "trace" | "debug" | "info" | "warn" | "error")
                });
                let is_trace_macro = segments.last().is_some_and(|name| {
                    matches!(
                        name.as_str(),
                        "trace"
                            | "debug"
                            | "info"
                            | "warn"
                            | "error"
                            | "event"
                            | "span"
                            | "trace_span"
                            | "debug_span"
                            | "info_span"
                            | "warn_span"
                            | "error_span"
                            | "enabled"
                    )
                });
                let direct_tracing_macro = segments.first().is_some_and(|name| name == "tracing");
                let exact_funnel_call = self.inside_emit
                    && segments.len() == 2
                    && segments[0] == "tracing"
                    && is_log_macro;
                if (direct_tracing_macro || is_trace_macro || self.inside_trace_impl)
                    && !exact_funnel_call
                {
                    self.violations
                        .push("trace macro bypasses AcpTraceEvent::emit".to_string());
                }
                syn::visit::visit_macro(self, node);
            }
        }

        let syntax = syn::parse_file(source).unwrap();
        let mut visitor = Visitor {
            allow_typed_funnel,
            module_depth: 0,
            inside_trace_impl: false,
            inside_emit: false,
            violations: Vec::new(),
        };
        visitor.visit_file(&syntax);
        visitor.violations
    }

    #[test]
    fn trace_source_guard_rejects_direct_calls_outside_typed_funnel() {
        let source = include_str!("acp_backend.rs");
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        for entry in std::fs::read_dir(src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let file = std::fs::read_to_string(&path).unwrap();
                let allow_typed_funnel = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == "acp_backend.rs");
                violations.extend(
                    trace_source_violations_with_funnel(&file, allow_typed_funnel)
                        .into_iter()
                        .map(|violation| format!("{}: {violation}", path.display())),
                );
            }
        }
        assert!(
            violations.is_empty(),
            "production trace calls bypassed the typed funnel: {violations:?}"
        );
        assert!(
            !trace_source_violations("fn leak(secret: &str) { tracing::warn!(%secret); }")
                .is_empty()
        );
        assert!(!trace_source_violations(
            "use tracing::warn; fn leak(secret: &str) { warn!(%secret); }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "use tracing as t; fn leak(secret: &str) { t::warn!(%secret); }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "enum AcpTraceEvent { Safe } impl AcpTraceEvent { fn helper(self) { tracing::warn!(\"leak\"); } }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "enum AcpTraceEvent { Safe } impl AcpTraceEvent { fn emit(self) { leak!(self); } }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "fn leak(secret: &str) { tracing::Span::current().record(\"secret\", secret); }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "trait Leak { fn emit(self, secret: &str); } enum AcpTraceEvent { Safe } \
             impl Leak for AcpTraceEvent { fn emit(self, secret: &str) { tracing::warn!(%secret); } }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "#[tracing::instrument] fn leak(secret: &str) { let _ = secret; }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "macro_rules! leak { ($secret:expr) => { tracing::warn!(%$secret); } } \
             fn expose(secret: &str) { leak!(secret); }"
        )
        .is_empty());
        assert!(!trace_source_violations(
            "mod nested { enum AcpTraceEvent { Leak(String) } \
             impl AcpTraceEvent { fn emit(self) { tracing::warn!(\"leak\"); } } }"
        )
        .is_empty());
        assert!(
            !trace_source_violations_with_funnel(
                "enum AcpTraceEvent { Leak } \
                 impl AcpTraceEvent { fn emit(self) { tracing::warn!(\"sibling leak\"); } }",
                false,
            )
            .is_empty(),
            "a root-level lookalike in a sibling source file must not become the trusted funnel"
        );
        assert!(!trace_source_violations(
            "extern crate tracing as telemetry; \
             macro_rules! leak { () => { telemetry::warn!(\"leak\"); } } fn expose() { leak!(); }"
        )
        .is_empty());
        assert!(trace_source_violations(
            "#[cfg(test)] mod tests { fn capture() { tracing::subscriber::with_default((), || {}); } }"
        )
        .is_empty());

        let syntax = syn::parse_file(source).unwrap();
        let event = syntax
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) if item.ident == "AcpTraceEvent" => Some(item),
                _ => None,
            })
            .expect("typed trace event enum");
        for field in event.variants.iter().flat_map(|variant| &variant.fields) {
            let syn::Type::Path(path) = &field.ty else {
                panic!("trace funnel accepts a non-scalar field");
            };
            let name = path.path.segments.last().unwrap().ident.to_string();
            assert!(
                matches!(name.as_str(), "bool" | "u16" | "i64"),
                "trace funnel must not accept opaque runtime data: {name}"
            );
        }
    }

    #[derive(Clone)]
    struct TraceCapture(Arc<StdMutex<Vec<u8>>>);

    struct TraceWriter(Arc<StdMutex<Vec<u8>>>);

    impl std::io::Write for TraceWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceCapture {
        type Writer = TraceWriter;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriter(self.0.clone())
        }
    }

    #[test]
    fn typed_trace_funnel_emits_metadata_without_opaque_secret_text() {
        let bytes = Arc::new(StdMutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing_subscriber::filter::LevelFilter::TRACE)
            .with_writer(TraceCapture(bytes.clone()))
            .finish();
        let known_secret = "sk-known-secret-must-not-appear";
        tracing::subscriber::with_default(subscriber, || {
            for event in [
                AcpTraceEvent::ModelOptionsMissing,
                AcpTraceEvent::EffortBelowMinimum {
                    advertised_count: AcpTraceEvent::bounded_count(known_secret.len()),
                },
                AcpTraceEvent::EffortOptionMissing,
                AcpTraceEvent::ConfigResolved {
                    effort_applied: true,
                    fell_back: false,
                },
                AcpTraceEvent::EffortFallback,
                AcpTraceEvent::EffortRequestRejected { rpc_code: -32603 },
                AcpTraceEvent::EffortWalkdownExhausted { rpc_code: -32602 },
                AcpTraceEvent::EffortWalkdownRetry { rpc_code: -32601 },
                AcpTraceEvent::InitializeFailed,
                AcpTraceEvent::AuthMethodMismatch {
                    advertised_count: AcpTraceEvent::bounded_count(known_secret.len()),
                },
                AcpTraceEvent::PreAuthenticated {
                    advertised_count: AcpTraceEvent::bounded_count(known_secret.len()),
                },
                AcpTraceEvent::SessionCreateFailed,
                AcpTraceEvent::DiscoverySessionCreateFailed,
                AcpTraceEvent::PromptFailed,
                AcpTraceEvent::WarmConfigNotAdvertised,
                AcpTraceEvent::WarmConfigRejected,
            ] {
                event.emit();
            }
        });
        let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert!(output.contains("acp.prompt_failed"));
        assert!(!output.contains(known_secret));
        assert!(!output.contains("sk-known-secret"));
    }

    struct RejectOnRecord {
        count: AtomicU64,
        reject_at: u64,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for RejectOnRecord {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if current == self.reject_at {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }
    }

    struct BlockOnRecord {
        count: AtomicU64,
        block_at: u64,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl DiagnosticObserver for BlockOnRecord {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if current == self.block_at {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(())
        }
    }

    #[test]
    fn bump_activity_advances_last_activity() {
        let w = TurnWatch {
            turn_start: std::time::Instant::now(),
            last_activity_ms: std::sync::atomic::AtomicU64::new(0),
        };
        std::thread::sleep(std::time::Duration::from_millis(2));
        bump_activity(&w);
        assert!(
            w.last_activity_ms
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 1
        );
    }

    #[test]
    fn dispatch_install_and_fence_close_share_one_atomic_gate() {
        let dispatch_gate = Arc::new(StdMutex::new(()));
        let unavailable = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicBool::new(false));
        let install_entered = Arc::new(std::sync::Barrier::new(2));
        let release_install = Arc::new(std::sync::Barrier::new(2));
        let (installed_tx, installed_rx) = std::sync::mpsc::channel();
        let install_thread = {
            let dispatch_gate = Arc::clone(&dispatch_gate);
            let unavailable = Arc::clone(&unavailable);
            let accepted = Arc::clone(&accepted);
            let install_entered = Arc::clone(&install_entered);
            let release_install = Arc::clone(&release_install);
            std::thread::spawn(move || {
                let installed = AcpBackend::install_prompt_request(
                    &dispatch_gate,
                    &unavailable,
                    &accepted,
                    || {
                        install_entered.wait();
                        release_install.wait();
                        "installed"
                    },
                );
                installed_tx.send(installed).unwrap();
            })
        };
        install_entered.wait();
        assert!(
            dispatch_gate.try_lock().is_err(),
            "request installation must hold the connection-wide gate"
        );
        assert!(
            !accepted.load(Ordering::SeqCst),
            "a published route must remain pre-acceptance while installation is incomplete"
        );

        let close_started = Arc::new(std::sync::Barrier::new(2));
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let close_thread = {
            let dispatch_gate = Arc::clone(&dispatch_gate);
            let unavailable = Arc::clone(&unavailable);
            let close_started = Arc::clone(&close_started);
            std::thread::spawn(move || {
                close_started.wait();
                AcpBackend::close_connection_fence(&dispatch_gate, &unavailable);
                closed_tx.send(()).unwrap();
            })
        };
        close_started.wait();
        assert!(
            closed_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "fence close must wait until request installation leaves the same gate"
        );

        release_install.wait();
        assert_eq!(installed_rx.recv().unwrap(), Some("installed"));
        assert!(
            accepted.load(Ordering::SeqCst),
            "installation must cross the accepted-work barrier before releasing the gate"
        );
        closed_rx.recv().unwrap();
        install_thread.join().unwrap();
        close_thread.join().unwrap();
        assert!(unavailable.load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_delivery_fence_orders_acceptance_sample() {
        let dispatch_gate = Arc::new(StdMutex::new(()));
        let unavailable = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicBool::new(false));
        let install_entered = Arc::new(std::sync::Barrier::new(2));
        let release_install = Arc::new(std::sync::Barrier::new(2));
        let install_thread = {
            let dispatch_gate = Arc::clone(&dispatch_gate);
            let unavailable = Arc::clone(&unavailable);
            let accepted = Arc::clone(&accepted);
            let install_entered = Arc::clone(&install_entered);
            let release_install = Arc::clone(&release_install);
            std::thread::spawn(move || {
                AcpBackend::install_prompt_request(&dispatch_gate, &unavailable, &accepted, || {
                    install_entered.wait();
                    release_install.wait();
                    "installed"
                })
            })
        };
        install_entered.wait();
        let (failure_tx, failure_rx) = std::sync::mpsc::channel();
        let failure_thread = {
            let dispatch_gate = Arc::clone(&dispatch_gate);
            let unavailable = Arc::clone(&unavailable);
            let accepted = Arc::clone(&accepted);
            std::thread::spawn(move || {
                let failure = AcpBackend::send_cancel_under_dispatch_fence(
                    &dispatch_gate,
                    &unavailable,
                    || Some(accepted),
                    || Err::<(), _>("closed"),
                )
                .unwrap_err();
                failure_tx.send(failure).unwrap();
            })
        };
        assert!(
            failure_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "cancel failure must wait for in-gate request installation"
        );
        release_install.wait();
        assert_eq!(install_thread.join().unwrap(), Some("installed"));
        let failure = failure_rx.recv().unwrap();
        failure_thread.join().unwrap();
        assert!(failure.prompt_may_have_been_accepted);
        assert!(unavailable.load(Ordering::SeqCst));

        let pre_gate = Arc::new(StdMutex::new(()));
        let pre_unavailable = Arc::new(AtomicBool::new(false));
        let pre_accepted = Arc::new(AtomicBool::new(false));
        let send_entered = Arc::new(std::sync::Barrier::new(2));
        let release_send = Arc::new(std::sync::Barrier::new(2));
        let (pre_failure_tx, pre_failure_rx) = std::sync::mpsc::channel();
        let pre_failure_thread = {
            let pre_gate = Arc::clone(&pre_gate);
            let pre_unavailable = Arc::clone(&pre_unavailable);
            let send_entered = Arc::clone(&send_entered);
            let release_send = Arc::clone(&release_send);
            std::thread::spawn(move || {
                let failure = AcpBackend::send_cancel_under_dispatch_fence(
                    &pre_gate,
                    &pre_unavailable,
                    || None,
                    || {
                        send_entered.wait();
                        release_send.wait();
                        Err::<(), _>("closed")
                    },
                )
                .unwrap_err();
                pre_failure_tx.send(failure).unwrap();
            })
        };
        send_entered.wait();
        let (pre_install_tx, pre_install_rx) = std::sync::mpsc::channel();
        let pre_install_thread = {
            let pre_gate = Arc::clone(&pre_gate);
            let pre_unavailable = Arc::clone(&pre_unavailable);
            let pre_accepted = Arc::clone(&pre_accepted);
            std::thread::spawn(move || {
                let installed = AcpBackend::install_prompt_request(
                    &pre_gate,
                    &pre_unavailable,
                    &pre_accepted,
                    || "must not install",
                );
                pre_install_tx.send(installed).unwrap();
            })
        };
        assert!(
            pre_install_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "prompt installation must wait while cancel delivery owns the dispatch gate"
        );
        release_send.wait();
        let failure = pre_failure_rx.recv().unwrap();
        assert!(!failure.prompt_may_have_been_accepted);
        assert_eq!(pre_install_rx.recv().unwrap(), None);
        pre_failure_thread.join().unwrap();
        pre_install_thread.join().unwrap();
        assert!(pre_unavailable.load(Ordering::SeqCst));
        assert!(!pre_accepted.load(Ordering::SeqCst));
    }

    #[test]
    fn turn_acceptance_is_operation_scoped_and_distinct_from_route_liveness() {
        let entry = AgentSession::new();
        let accepted = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));
        let guard =
            TurnAcceptanceGuard::install(Arc::clone(&entry.turn_accepted), Arc::clone(&accepted));

        assert!(active.load(Ordering::SeqCst));
        assert!(!AcpBackend::turn_prompt_accepted(&entry));
        accepted.store(true, Ordering::SeqCst);
        active.store(false, Ordering::SeqCst);
        assert!(
            AcpBackend::turn_prompt_accepted(&entry),
            "acceptance evidence must survive after active routing ends"
        );
        drop(guard);
        assert!(
            AcpBackend::turn_prompt_accepted_handle(&entry).is_none(),
            "operation-scoped acceptance must clear when the turn owner exits"
        );
    }

    #[tokio::test]
    async fn cancellation_settle_prefers_ready_sdk_result_over_ready_local_bounds() {
        // All three arms are ready on the first poll. Without `biased` prompt
        // priority, repeated selection eventually chooses kill/grace and loses
        // the deeper SDK cause.
        for _ in 0..64 {
            let prompt = std::future::ready(Err::<(), _>("deep-sdk-cause"));
            tokio::pin!(prompt);
            let kill = Notify::new();
            kill.notify_one();
            match settle_prompt_after_cancel(prompt.as_mut(), &kill, Duration::ZERO).await {
                CancelSettle::Prompt(Err(cause)) => assert_eq!(cause, "deep-sdk-cause"),
                _ => panic!("a ready SDK terminal must win same-poll local bounds"),
            }
        }
    }

    /// Spawn an in-process fake ACP agent on `channel` that answers `initialize`
    /// with the given response. Returns immediately; the agent loop runs in a task.
    fn spawn_fake_agent(channel: Channel, resp: InitializeResponse) {
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("fake-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| {
                        let resp = resp.clone();
                        async move {
                            responder.respond(resp)?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                // An agent that advertises an auth method must answer `authenticate`
                // (the backend attempts it post-initialize); accept it.
                .on_receive_request(
                    move |_req: agent_client_protocol::schema::v1::AuthenticateRequest,
                          responder: agent_client_protocol::Responder<
                        agent_client_protocol::schema::v1::AuthenticateResponse,
                    >,
                          _cx| async move {
                        responder.respond(
                            agent_client_protocol::schema::v1::AuthenticateResponse::new(),
                        )?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    /// Spawn an in-process fake ACP agent that *opens* the channel (so it does
    /// not EOF) but **never** answers `initialize`: the handler parks forever
    /// holding the responder, simulating a hung agent rather than a closed one.
    fn spawn_hung_agent(channel: Channel) {
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("hung-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          _responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| async move {
                        // Never respond; park forever so the channel stays open
                        // and the client's initialize request hangs.
                        std::future::pending::<()>().await;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    fn test_config() -> AcpConfig {
        AcpConfig {
            cwd: std::path::PathBuf::from("/tmp"),
            ..AcpConfig::default()
        }
    }

    /// Like [`test_config`] but with a short handshake bound so the
    /// never-answers test fails fast instead of waiting the 30s default.
    fn test_config_short_handshake() -> AcpConfig {
        AcpConfig {
            cwd: std::path::PathBuf::from("/tmp"),
            handshake_timeout: Duration::from_millis(200),
            ..AcpConfig::default()
        }
    }

    #[test]
    fn map_session_update_maps_usage_to_update_usage_clock_free() {
        use agent_client_protocol::schema::v1::UsageUpdate;

        let notif = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::UsageUpdate(UsageUpdate::new(14584, 258400)),
        );
        match AcpBackend::map_session_update(notif).expect("usage maps") {
            Update::Usage(s) => {
                assert_eq!(s.used, Some(14584));
                assert_eq!(s.size, Some(258400));
                assert_eq!(s.cost, None);
                assert_eq!(s.at_ms, 0);
            }
            other => panic!("expected Update::Usage, got {other:?}"),
        }
    }

    #[test]
    fn map_rich_plan_toolcall_update() {
        use agent_client_protocol::schema::v1::{
            ContentChunk, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, ToolCall,
            ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
            ToolCallUpdateFields, ToolKind,
        };

        let plan = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::Plan(Plan::new(vec![PlanEntry::new(
                "inspect repo",
                PlanEntryPriority::High,
                PlanEntryStatus::InProgress,
            )])),
        );
        assert!(matches!(
            AcpBackend::map_session_update_rich(&plan),
            Some(bridge_core::orch::OrchEventKind::Plan { .. })
        ));

        let tc = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::ToolCall(ToolCall::new("t1", "read").kind(ToolKind::Read)),
        );
        let Some(bridge_core::orch::OrchEventKind::ToolCall { tool_call_id, .. }) =
            AcpBackend::map_session_update_rich(&tc)
        else {
            panic!("expected rich tool call");
        };
        assert_eq!(tool_call_id, "t1");

        let update = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "t1",
                ToolCallUpdateFields::new()
                    .title("read file".to_string())
                    .kind(ToolKind::Read)
                    .status(ToolCallStatus::Completed)
                    .content(Vec::<ToolCallContent>::new())
                    .locations(vec![ToolCallLocation::new("src/lib.rs")]),
            )),
        );
        let Some(bridge_core::orch::OrchEventKind::ToolCallUpdate {
            title,
            kind,
            status,
            locations,
            content,
            ..
        }) = AcpBackend::map_session_update_rich(&update)
        else {
            panic!("expected rich tool call update");
        };
        assert_eq!(title.as_deref(), Some("read file"));
        assert_eq!(kind.as_deref(), Some("read"));
        assert_eq!(status.as_deref(), Some("completed"));
        assert_eq!(locations.as_deref(), Some(&["src/lib.rs".to_string()][..]));
        let content = content.expect("empty content patch remains present");
        assert_eq!(content.item_count, 0);
        assert_eq!(content.preview, "");

        let txt = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                TextContent::new("hi"),
            ))),
        );
        assert!(AcpBackend::map_session_update_rich(&txt).is_none());
    }

    #[test]
    fn map_rich_caps_content() {
        use agent_client_protocol::schema::v1::{ToolCall, ToolCallContent};

        let big = "x".repeat(10_000);
        let tc = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::ToolCall(ToolCall::new("t", "t").content(vec![ToolCallContent::from(
                ContentBlock::Text(TextContent::new(big)),
            )])),
        );
        let Some(bridge_core::orch::OrchEventKind::ToolCall {
            content: Some(cs), ..
        }) = AcpBackend::map_session_update_rich(&tc)
        else {
            panic!("expected rich tool call content");
        };
        assert!(cs.preview.len() <= RICH_CONTENT_CAP);
    }

    #[test]
    fn map_rich_caps_tool_call_id() {
        use agent_client_protocol::schema::v1::ToolCall;
        // tool_call_id is agent-controlled + persisted into event_json -> must be capped (FIX-11).
        let big_id = "z".repeat(10_000);
        let tc = SessionNotification::new(
            AgentSessionId::from("s"),
            SessionUpdate::ToolCall(ToolCall::new(big_id, "t")),
        );
        let Some(bridge_core::orch::OrchEventKind::ToolCall { tool_call_id, .. }) =
            AcpBackend::map_session_update_rich(&tc)
        else {
            panic!("expected rich tool call");
        };
        assert!(tool_call_id.len() <= RICH_CONTENT_CAP);
    }

    #[tokio::test]
    async fn container_reap_is_idempotent_across_sites_and_noop_without_container() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let reap_fn: bridge_core::reaper::ReapFn = Arc::new(move |_r, _n| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let controller =
            ReapController::from_legacy("docker", "a2a-ro-owner-nonce", Arc::clone(&reap_fn));
        let container = Some(controller.clone());
        let reaped = Arc::new(AtomicBool::new(false));
        // escalate_terminate + retire + Drop all firing → still one `docker rm -f`.
        AcpBackend::reap_container(&container, &reaped);
        AcpBackend::reap_container(&container, &reaped);
        AcpBackend::reap_container(&container, &reaped);
        controller.reap_observed().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "reaped at most once across all sites"
        );

        // No container → never reaps.
        let none: Option<ReapController> = None;
        let r2 = Arc::new(AtomicBool::new(false));
        AcpBackend::reap_container(&none, &r2);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn spawn_handshake_failure_reaps_the_container() {
        use std::sync::atomic::AtomicUsize;
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let reap_fn: bridge_core::reaper::ReapFn = Arc::new(move |_r, _n| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let cfg = AcpConfig {
            cwd: std::env::temp_dir(),
            handshake_timeout: Duration::from_millis(200), // force the timeout fast
            cancel_grace: Duration::from_millis(200),
            container: Some(ContainerReap::from_legacy(
                "docker",
                "a2a-ro-owner-nonce",
                reap_fn,
            )),
            ..AcpConfig::default()
        };
        // /bin/cat starts (Supervised ok) but never answers `initialize` → handshake timeout → spawn Err.
        let res = AcpBackend::spawn("/bin/cat", &[], cfg).await;
        assert!(res.is_err(), "handshake must time out");
        tokio::time::timeout(Duration::from_secs(2), async {
            while calls.load(Ordering::SeqCst) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the started container is reaped on the spawn-error path");
    }

    #[tokio::test]
    async fn connect_runs_initialize_and_captures_agent_capabilities() {
        // Fake agent advertises one auth method; assert the backend captured the
        // negotiated InitializeResponse (caps + auth methods) over the transport seam.
        let (client_side, agent_side) = Channel::duplex();
        let resp = InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(AgentCapabilities::default())
            .auth_methods(vec![AuthMethod::Agent(AuthMethodAgent::new(
                AuthMethodId::new("oauth"),
                "OAuth",
            ))]);
        spawn_fake_agent(agent_side, resp);

        let be = AcpBackend::connect(client_side, test_config())
            .await
            .expect("initialize handshake succeeds");

        assert!(
            be.agent_capabilities().is_some(),
            "SDK path must capture agent capabilities"
        );
        let methods = be.auth_methods().expect("auth methods captured");
        assert_eq!(methods.len(), 1, "advertised auth method round-trips");
    }

    #[tokio::test]
    async fn capabilities_maps_agent_session_capabilities() {
        let (client_side, agent_side) = Channel::duplex();
        let agent_caps = AgentCapabilities::new()
            .load_session(true)
            .session_capabilities(
                SessionCapabilities::new()
                    .list(SessionListCapabilities::new())
                    .resume(SessionResumeCapabilities::new())
                    .close(SessionCloseCapabilities::new()),
            );
        spawn_fake_agent(
            agent_side,
            InitializeResponse::new(ProtocolVersion::V1).agent_capabilities(agent_caps),
        );

        let be = AcpBackend::connect(client_side, test_config())
            .await
            .expect("initialize handshake succeeds");

        assert_eq!(
            be.capabilities(),
            bridge_core::orch::AgentSessionCaps {
                load_session: true,
                resume: true,
                close: true,
                list: true,
                delete: false,
            }
        );
    }

    #[tokio::test]
    async fn connect_errors_when_agent_never_answers() {
        // Agent side is dropped immediately -> initialize never completes -> AgentCrashed.
        let (client_side, agent_side) = Channel::duplex();
        drop(agent_side);
        match AcpBackend::connect(client_side, test_config()).await {
            Err(error) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::Initialize,
                    DiagnosticFailureClass::Transport,
                    false,
                );
            }
            Ok(_) => panic!("expected a failure when the agent never answers initialize"),
        }
    }

    // I1: a *hung* agent (channel open, no initialize reply) must NOT hang us
    // forever — the bounded handshake returns an error within the timeout.
    #[tokio::test]
    async fn connect_times_out_when_agent_opens_but_never_answers_initialize() {
        let (client_side, agent_side) = Channel::duplex();
        // Agent connects (channel stays open) but never responds to initialize.
        spawn_hung_agent(agent_side);

        // Bound the whole call so a regression (no handshake timeout) fails the
        // test instead of hanging the suite.
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            AcpBackend::connect(client_side, test_config_short_handshake()),
        )
        .await
        .expect("connect must return within the handshake bound, not hang");

        match outcome {
            Err(error) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::Initialize,
                    DiagnosticFailureClass::Timeout,
                    false,
                );
            }
            Ok(_) => panic!("expected an error when the agent never answers initialize"),
        }
    }

    #[tokio::test]
    async fn observed_initialize_timeout_fails_initialize_before_authenticate() {
        let (client_side, agent_side) = Channel::duplex();
        spawn_hung_agent(agent_side);
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());

        let error = match AcpBackend::connect_observed(
            client_side,
            test_config_short_handshake(),
            observer.clone(),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("hung initialize must fail"),
        };
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("observed initialize timeout must be structured, got {error:?}");
        };
        assert_eq!(diagnostic.failed_phase(), DiagnosticPhase::Initialize);
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Timeout);
        assert_eq!(
            diagnostic.disposition(),
            FailureDisposition::RetrySameTarget
        );
        assert!(!diagnostic.prompt_may_have_been_accepted());

        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().phase(), DiagnosticPhase::Initialize);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().phase(), DiagnosticPhase::Initialize);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
        assert!(events
            .iter()
            .all(|event| event.transition().phase() != DiagnosticPhase::Authenticate));
    }

    #[tokio::test]
    async fn observed_spawn_failure_is_agent_process_and_never_initializes() {
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = match AcpBackend::spawn_observed(
            "/definitely/not/an/a2a-bridge-agent",
            &[],
            test_config(),
            observer.clone(),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("missing executable must fail spawn"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Spawn,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
        assert_eq!(
            diagnostic.disposition(),
            FailureDisposition::RetrySameTarget
        );
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
        assert!(events
            .iter()
            .all(|event| event.transition().phase() == DiagnosticPhase::Spawn));
    }

    #[tokio::test]
    async fn connected_backend_does_not_retain_initialization_observer() {
        let (client_side, agent_side) = Channel::duplex();
        spawn_fake_agent(
            agent_side,
            InitializeResponse::new(ProtocolVersion::V1)
                .agent_capabilities(AgentCapabilities::default()),
        );
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let observer_dyn: Arc<dyn DiagnosticObserver> = observer.clone();
        let weak = Arc::downgrade(&observer_dyn);
        let backend = AcpBackend::connect_observed(client_side, test_config(), observer_dyn)
            .await
            .unwrap();
        drop(observer);
        assert!(
            weak.upgrade().is_none(),
            "cached backend must not retain its initialization observer"
        );
        drop(backend);
    }

    #[tokio::test]
    async fn from_child_installs_configured_stderr_redactor_on_adopted_process() {
        const SECRET: &str = "from-child-known-secret";
        let process = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                r#"echo from-child-known-secret 1>&2; IFS= read -r request; \
                   id=$(printf '%s\n' "$request" | sed -n 's/.*"id":\([^,}]*\).*/\1/p'); \
                   printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}\n' "$id"; \
                   echo from-child-known-secret 1>&2; sleep 30"#,
            ],
            None,
        )
        .unwrap();
        let ring = process.stderr_ring();
        for _ in 0..100 {
            if ring.metadata_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            ring.metadata_since(ring.origin()).line_count(),
            1,
            "the first stderr line must predate adoption"
        );

        let backend = tokio::time::timeout(
            Duration::from_secs(2),
            AcpBackend::from_child(
                process,
                AcpConfig {
                    diagnostic_redactor: DiagnosticRedactor::new([SECRET]),
                    ..test_config()
                },
            ),
        )
        .await
        .expect("scripted child must answer initialize")
        .expect("from_child must adopt a conformant process");
        let adopted_ring = backend.stderr_ring.as_ref().expect("adopted stderr ring");
        for _ in 0..100 {
            if adopted_ring
                .metadata_since(adopted_ring.origin())
                .line_count()
                == 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let debug = format!("{adopted_ring:?}");
        assert!(
            debug.contains("known_value_count: 1"),
            "from_child must install the configured policy on the actual adopted ring: {debug}"
        );
        assert!(!debug.contains(SECRET), "safe Debug must not reveal values");
        assert_eq!(
            adopted_ring
                .metadata_since(adopted_ring.origin())
                .line_count(),
            2,
            "the same adopted ring must drain stderr before and after initialization"
        );
    }

    #[tokio::test]
    async fn from_child_initialize_failure_preserves_bounded_stderr_metadata() {
        const SECRET: &str = "from-child-initialize-secret";
        let process = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                r#"echo from-child-initialize-secret 1>&2; IFS= read -r request; \
                   id=$(printf '%s\n' "$request" | sed -n 's/.*"id":\([^,}]*\).*/\1/p'); \
                   printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"initialize rejected"}}\n' "$id"; \
                   sleep 30"#,
            ],
            None,
        )
        .unwrap();
        let ring = process.stderr_ring();
        for _ in 0..100 {
            if ring.metadata_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(ring.metadata_since(ring.origin()).line_count(), 1);

        let error = match tokio::time::timeout(
            Duration::from_secs(2),
            AcpBackend::from_child(
                process,
                AcpConfig {
                    diagnostic_redactor: DiagnosticRedactor::new([SECRET]),
                    ..test_config()
                },
            ),
        )
        .await
        .expect("scripted child must reject initialize promptly")
        {
            Err(error) => error,
            Ok(_) => panic!("rejected initialize must fail from_child"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Initialize,
            DiagnosticFailureClass::Transport,
            false,
        );
        let json = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(json["stderr_observed"], true);
        assert_eq!(json["stderr_line_count"], 1);
        assert_eq!(json["stderr_scope"], "process");
        assert!(json.get("stderr_tail").is_none());
        assert!(!serde_json::to_string(&json).unwrap().contains(SECRET));
    }

    // B1: `spawn` must HOLD the Supervised child for the backend's lifetime.
    // Before the fix, `Supervised` (kill_on_drop) was dropped when `spawn`
    // returned, SIGKILLing the child immediately. We cannot run a real ACP
    // agent here, so we drive the same `Supervised::spawn` path through a long-
    // lived child and assert: (a) the backend retains `supervised.is_some()`,
    // and (b) the child is still alive (not reaped/SIGKILLed) shortly after.
    #[tokio::test]
    async fn spawn_holds_child_alive_after_returning() {
        // A long-lived child (`cat` blocks reading stdin), driven through the
        // exact `Supervised::spawn` + pipe-`take()` seam that `spawn` uses, then
        // held on the backend struct mirroring `spawn`'s end state.
        let mut supervised = Supervised::spawn("/bin/cat", &[], None).expect("spawn cat");
        let pid = supervised.pid();
        // Take the pipes exactly as `spawn` does (also exercises the I3 seam).
        let child = supervised.child_mut();
        let _stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("test: stdin unavailable"))
            .unwrap();
        let _stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("test: stdout unavailable"))
            .unwrap();

        let backend = AcpBackend {
            conn: None,
            supervised: Arc::new(StdMutex::new(Some(supervised))),
            stderr_ring: None,
            config: None,
            container_reap: None,
            reaped: Arc::new(AtomicBool::new(false)),
            unavailable: Arc::new(AtomicBool::new(false)),
            dispatch_gate: Arc::new(StdMutex::new(())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cfg: Arc::new(StdMutex::new(HashMap::new())),
            pending_turn_meta: StdMutex::new(HashMap::new()),
            policy: Arc::new(StdMutex::new(
                Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
            )),
            permission_registry: Arc::new(StdMutex::new(None)),
            perm_timeout_ms: Arc::new(AtomicU64::new(120_000)),
            prompt_snapshot_hook: StdMutex::new(None),
            before_process_redactor_hook: StdMutex::new(None),
            fail_deferred_cancel_send: Arc::new(AtomicBool::new(false)),
            fail_cancel_send: Arc::new(AtomicBool::new(false)),
        };

        assert!(
            backend.supervised.lock().unwrap().is_some(),
            "backend must retain the Supervised child (B1)"
        );

        // Give an erroneous kill_on_drop time to fire, then confirm the child is
        // still alive. signal 0 succeeds => the OS still has this owned process
        // (not SIGKILLed+reaped). This is the regression the BLOCKER describes:
        // before the fix, the local `supervised` dropped at `spawn`'s end and
        // the child was killed here.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(
            alive,
            "child must still be alive after spawn returns (B1: not SIGKILLed on drop)"
        );

        // Clean up deterministically (SIGTERM->reap), leaving no zombie.
        let taken = backend.supervised.lock().unwrap().take();
        if let Some(s) = taken {
            s.terminate(Duration::from_millis(100)).await;
        }
    }

    // ── Recording fake agent (session/new lazy-once, cancel-latch, turn order) ──
    //
    // A single in-process fake agent that RECORDS the requests it receives, so
    // Task-2 tests can assert protocol-level invariants:
    //   * `new_session_calls`  — count of `session/new` (exactly-once minting).
    //   * `new_session_gate`   — an awaitable barrier the agent waits on BEFORE
    //                            replying to `session/new`, so a test can open
    //                            the concurrent/racing window deterministically.
    //   * `cancels`            — agent session ids seen via `session/cancel`
    //                            (the cancel-latch must land one here).
    //   * `prompt_starts/ends` — prompt-turn ordering, to prove turns run
    //                            sequentially (non-interleaved) under the lock.
    // Tasks 3/4 reuse this harness for prompt streaming / cancel completion.
    // `CancelNotification`, `NewSessionRequest`, `AgentSessionId` are already in
    // scope via `super::*`; import only the agent-side response/prompt types.
    use agent_client_protocol::schema::v1::{
        AuthenticateRequest, AuthenticateResponse, ContentChunk, NewSessionResponse,
        PermissionOption, PermissionOptionId, PromptRequest, PromptResponse,
        RequestPermissionRequest, RequestPermissionResponse, SessionConfigKind,
        SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionMode,
        SessionModeState, SetSessionConfigOptionResponse, SetSessionModeRequest,
        SetSessionModeResponse, StopReason, ToolCall, ToolCallId, ToolCallUpdate,
        ToolCallUpdateFields, ToolKind, Usage,
    };
    use bridge_core::domain::Effort;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    /// A scripted `session/update` the fake agent emits mid-turn, before it
    /// returns the `PromptResponse`. Lets a test drive the streaming fan-in:
    /// text chunks (modeled) and rich/unmodeled variants (thought / tool call).
    #[derive(Clone)]
    enum ScriptedUpdate {
        /// `session/update` with an `agent_message_chunk` carrying this text.
        Text(&'static str),
        /// `session/update` with an `agent_thought_chunk` (unmodeled → dropped).
        Thought(&'static str),
        /// Pause before the next scripted update/terminal response.
        Delay(Duration),
        /// `session/update` with an empty `plan` (unmodeled → dropped).
        Plan,
        /// `session/update` with a tool call (rich side-channel when observed).
        ToolCall,
        /// `session/update` with a context-window `usage_update`.
        Usage(u64, u64),
    }

    #[derive(Default)]
    struct CountingSink {
        records: AtomicUsize,
    }

    impl CountingSink {
        fn records(&self) -> usize {
            self.records.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RichEventSink for CountingSink {
        fn record(&self, _kind: OrchEventKind) {
            self.records.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct Recorder {
        /// Number of `session/new` requests the agent received.
        new_session_calls: Arc<AtomicUsize>,
        /// Released to let a pending `session/new` reply proceed. When `None`,
        /// `session/new` replies immediately.
        new_session_gate: Arc<Notify>,
        /// Whether `session/new` should wait on the gate before replying.
        gate_new_session: Arc<AtomicBool>,
        /// Fires when a `session/new` handler ENTERS (before it awaits the gate),
        /// so a driver can deterministically know the mint is in flight without
        /// sleeping. Used to order create/cancel + concurrency races.
        new_session_started: Arc<Notify>,
        /// The agent-minted session id the fake returns from `session/new`.
        minted_id: &'static str,
        /// Agent session ids observed via `session/cancel` notifications.
        cancels: Arc<Mutex<Vec<String>>>,
        /// Fires every time a `session/cancel` is recorded (for awaiting it).
        cancel_seen: Arc<Notify>,
        /// Ordered log of prompt-turn events ("start", "end") to detect overlap.
        prompt_log: Arc<Mutex<Vec<&'static str>>>,
        /// Released to let an in-flight prompt turn complete (per-turn barrier).
        prompt_gate: Arc<Notify>,
        /// Fires when a prompt turn STARTS, so the driver can sequence turns.
        prompt_started: Arc<Notify>,
        /// Whether the prompt handler waits on `prompt_gate` before responding.
        /// Default `false` (respond immediately, after emitting any updates), so
        /// streaming tests don't have to drive the gate. The turn-ordering test
        /// sets it `true` to hold turns open.
        gate_prompt: Arc<AtomicBool>,
        /// Whether the prompt handler FAILS the turn (responds with a JSON-RPC
        /// error instead of a `PromptResponse`), so the client's `send_request`
        /// returns `Err` — driving the transport/agent-error path deterministically.
        fail_prompt: Arc<AtomicBool>,
        /// Optional structured JSON-RPC error returned when `fail_prompt` is set.
        /// `None` preserves the legacy internal-error fixture.
        prompt_error: Arc<Mutex<Option<AcpError>>>,
        /// Scripted `session/update`s the prompt handler emits (in order) BEFORE
        /// it returns the `PromptResponse`. Empty by default.
        prompt_updates: Arc<Mutex<Vec<ScriptedUpdate>>>,
        /// The `StopReason` the prompt handler returns. `EndTurn` by default.
        stop_reason: Arc<Mutex<StopReason>>,
        /// Optional terminal `PromptResponse.usage` token totals.
        terminal_usage: Arc<Mutex<Option<(u64, u64, u64)>>>,
        /// When set, the prompt handler WAITS for a `session/cancel` to arrive
        /// (awaits `cancel_arrived`) AFTER emitting its updates and BEFORE
        /// responding — modeling a real agent that only ends the turn once it has
        /// observed the cancel, returning whatever `stop_reason` is configured
        /// (typically `StopReason::Cancelled`). Proves completion is the RESULT,
        /// not the notification send.
        wait_cancel_before_respond: Arc<AtomicBool>,
        /// When set (with `wait_cancel_before_respond`), the prompt handler NEVER
        /// responds after observing the cancel — it parks forever, modeling a hung
        /// agent that ignores `session/cancel`. The backend must then escalate
        /// (terminate) to unblock the turn.
        hang_after_cancel: Arc<AtomicBool>,
        /// Fires when a `session/cancel` is recorded, used by the prompt handler
        /// to await the cancel deterministically (separate from `cancel_seen`,
        /// which the test driver awaits, so neither consumes the other's permit).
        cancel_arrived: Arc<Notify>,

        // ── Task 5: reverse `session/request_permission` ──────────────────────
        /// When set, the prompt handler issues a `session/request_permission`
        /// request BACK to the client (mid-turn) before ending the turn, offering
        /// the options scripted in `permission_options`, and records the client's
        /// reply in `permission_reply`.
        request_permission: Arc<AtomicBool>,
        /// The options the reverse permission request offers (order preserved).
        permission_options: Arc<Mutex<Vec<PermissionOption>>>,
        /// The client's reply to the reverse permission request:
        /// `Some(Some(option_id))` = Selected; `Some(None)` = Cancelled; `None` =
        /// no reply recorded yet (request not issued / still in flight / errored).
        permission_reply: Arc<Mutex<Option<Option<String>>>>,
        /// Fires once the client's permission reply has been recorded.
        permission_replied: Arc<Notify>,
        /// When set (with `request_permission`), the prompt handler emits its
        /// scripted text chunks + ends the turn ONLY AFTER it has received the
        /// permission reply — modeling a real agent that gates the rest of the
        /// turn on the permission decision. Proves the client's permission handler
        /// did NOT stall the dispatch loop (otherwise the reply, and hence these
        /// chunks + the PromptResponse, could never arrive → the test hangs).
        gate_turn_on_permission: Arc<AtomicBool>,
        /// Fires when the reverse permission request handler ENTERS (request about
        /// to be sent to the client), so a test can sequence deterministically.
        permission_requested: Arc<Notify>,

        // ── Task 6: set_mode / authenticate ───────────────────────────────────
        /// Mode ids observed via `session/set_mode` requests (in order).
        set_modes: Arc<Mutex<Vec<String>>>,
        /// Fires every time a `session/set_mode` is recorded.
        set_mode_seen: Arc<Notify>,
        /// When set, the `session/set_mode` handler REJECTS the request with a
        /// JSON-RPC error (modeling an agent that does not know the mode id).
        reject_set_mode: Arc<AtomicBool>,
        /// Auth-method ids observed via `authenticate` requests (in order).
        authenticates: Arc<Mutex<Vec<String>>>,
        /// When set, the `authenticate` handler REJECTS with a JSON-RPC error
        /// (modeling an auth failure). The backend surfaces a structured authentication failure.
        reject_authenticate: Arc<AtomicBool>,
        /// Auth methods the fake agent advertises in its `initialize` response.
        auth_methods: Arc<Mutex<Vec<AuthMethod>>>,
        /// Optional SDK session mode state advertised from `session/new`.
        session_modes: Arc<Mutex<Option<SessionModeState>>>,

        // ── Increment 3b: session/set_config_option (effort) ──────────────────
        /// `(config_id, value_id)` pairs observed via `session/set_config_option`.
        set_config_options: Arc<Mutex<Vec<(String, String)>>>,
        /// Fires every time a `session/set_config_option` is recorded.
        set_config_seen: Arc<Notify>,
        /// When set, the `session/set_config_option` handler REJECTS with a JSON-RPC
        /// error (modeling an adapter without a structured effort knob). NON-FATAL.
        reject_set_config: Arc<AtomicBool>,
        /// Whether `session/new` advertises a model config option.
        advertise_model_config: Arc<AtomicBool>,
        /// Config id/current/options for the advertised model select.
        model_config_id: Arc<Mutex<String>>,
        model_config_current: Arc<Mutex<String>>,
        model_config_values: Arc<Mutex<Vec<String>>>,
        /// Whether `session/new` advertises an effort config option.
        advertise_effort_config: Arc<AtomicBool>,
        /// Config id/current/options for the advertised effort select.
        effort_config_id: Arc<Mutex<String>>,
        effort_config_current: Arc<Mutex<String>>,
        effort_config_values: Arc<Mutex<Vec<String>>>,
        /// Optional effort levels to advertise after a model config option is applied.
        refreshed_effort_values_after_model: Arc<Mutex<Option<Vec<String>>>>,
        /// When set, successful `session/set_config_option` returns no refreshed options.
        empty_config_response_on_set: Arc<AtomicBool>,
        /// Optional 1-based `session/set_config_option` call number to reject.
        reject_set_config_call: Arc<Mutex<Option<usize>>>,
        /// Error body used when rejecting `session/set_config_option`.
        set_config_error_body: Arc<Mutex<String>>,

        // ── Task 4 (session-cwd): record the cwd the client sent at session/new ─
        /// The `cwd` from the most recent `session/new` request (as a lossy string).
        /// Tests assert against this to verify `ensure_session` passed the correct
        /// cwd down to the wire (stashed SessionSpec.cwd vs static AcpConfig.cwd).
        new_session_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    }

    impl Recorder {
        fn new(minted_id: &'static str) -> Self {
            Self {
                new_session_calls: Arc::new(AtomicUsize::new(0)),
                new_session_gate: Arc::new(Notify::new()),
                gate_new_session: Arc::new(AtomicBool::new(false)),
                new_session_started: Arc::new(Notify::new()),
                minted_id,
                cancels: Arc::new(Mutex::new(Vec::new())),
                cancel_seen: Arc::new(Notify::new()),
                prompt_log: Arc::new(Mutex::new(Vec::new())),
                prompt_gate: Arc::new(Notify::new()),
                prompt_started: Arc::new(Notify::new()),
                gate_prompt: Arc::new(AtomicBool::new(false)),
                fail_prompt: Arc::new(AtomicBool::new(false)),
                prompt_error: Arc::new(Mutex::new(None)),
                prompt_updates: Arc::new(Mutex::new(Vec::new())),
                stop_reason: Arc::new(Mutex::new(StopReason::EndTurn)),
                terminal_usage: Arc::new(Mutex::new(None)),
                wait_cancel_before_respond: Arc::new(AtomicBool::new(false)),
                hang_after_cancel: Arc::new(AtomicBool::new(false)),
                cancel_arrived: Arc::new(Notify::new()),
                request_permission: Arc::new(AtomicBool::new(false)),
                permission_options: Arc::new(Mutex::new(Vec::new())),
                permission_reply: Arc::new(Mutex::new(None)),
                permission_replied: Arc::new(Notify::new()),
                gate_turn_on_permission: Arc::new(AtomicBool::new(false)),
                permission_requested: Arc::new(Notify::new()),
                set_modes: Arc::new(Mutex::new(Vec::new())),
                set_mode_seen: Arc::new(Notify::new()),
                reject_set_mode: Arc::new(AtomicBool::new(false)),
                authenticates: Arc::new(Mutex::new(Vec::new())),
                reject_authenticate: Arc::new(AtomicBool::new(false)),
                auth_methods: Arc::new(Mutex::new(Vec::new())),
                session_modes: Arc::new(Mutex::new(None)),
                set_config_options: Arc::new(Mutex::new(Vec::new())),
                set_config_seen: Arc::new(Notify::new()),
                reject_set_config: Arc::new(AtomicBool::new(false)),
                advertise_model_config: Arc::new(AtomicBool::new(true)),
                model_config_id: Arc::new(Mutex::new("model".to_string())),
                model_config_current: Arc::new(Mutex::new("default".to_string())),
                model_config_values: Arc::new(Mutex::new(vec![
                    "default".to_string(),
                    "m".to_string(),
                    "a".to_string(),
                    "b".to_string(),
                    "gpt-x".to_string(),
                    "haiku".to_string(),
                ])),
                advertise_effort_config: Arc::new(AtomicBool::new(true)),
                effort_config_id: Arc::new(Mutex::new("effort".to_string())),
                effort_config_current: Arc::new(Mutex::new("medium".to_string())),
                effort_config_values: Arc::new(Mutex::new(vec![
                    "low".to_string(),
                    "medium".to_string(),
                    "high".to_string(),
                    "xhigh".to_string(),
                    "max".to_string(),
                ])),
                refreshed_effort_values_after_model: Arc::new(Mutex::new(None)),
                empty_config_response_on_set: Arc::new(AtomicBool::new(false)),
                reject_set_config_call: Arc::new(Mutex::new(None)),
                set_config_error_body: Arc::new(Mutex::new("Invalid value for effort".to_string())),
                new_session_cwd: Arc::new(Mutex::new(None)),
            }
        }

        fn select_options(values: Vec<String>) -> Vec<SessionConfigSelectOption> {
            values
                .into_iter()
                .map(|value| SessionConfigSelectOption::new(value.clone(), value))
                .collect()
        }

        async fn advertised_config_options(&self) -> Vec<SessionConfigOption> {
            let mut options = Vec::new();

            if self.advertise_model_config.load(Ordering::SeqCst) {
                let id = self.model_config_id.lock().await.clone();
                let current = self.model_config_current.lock().await.clone();
                let values = self.model_config_values.lock().await.clone();
                options.push(
                    SessionConfigOption::select(
                        id.clone(),
                        "Model".to_string(),
                        current,
                        Self::select_options(values),
                    )
                    .category(SessionConfigOptionCategory::Model),
                );
            }

            if self.advertise_effort_config.load(Ordering::SeqCst) {
                let id = self.effort_config_id.lock().await.clone();
                let current = self.effort_config_current.lock().await.clone();
                let values = self.effort_config_values.lock().await.clone();
                options.push(
                    SessionConfigOption::select(
                        id.clone(),
                        "Effort".to_string(),
                        current,
                        Self::select_options(values),
                    )
                    .category(SessionConfigOptionCategory::ThoughtLevel),
                );
            }

            options
        }

        async fn refresh_config_current(&self, config_id: &str, value_id: &str) {
            let model_config_id = self.model_config_id.lock().await.clone();
            if config_id == model_config_id {
                *self.model_config_current.lock().await = value_id.to_string();
                if let Some(values) = self
                    .refreshed_effort_values_after_model
                    .lock()
                    .await
                    .clone()
                {
                    *self.effort_config_values.lock().await = values;
                }
                return;
            }

            let effort_config_id = self.effort_config_id.lock().await.clone();
            if config_id == effort_config_id {
                *self.effort_config_current.lock().await = value_id.to_string();
            }
        }

        /// Script the `session/update`s this agent emits before responding.
        async fn set_updates(&self, updates: Vec<ScriptedUpdate>) {
            *self.prompt_updates.lock().await = updates;
        }

        /// Set the `StopReason` the prompt turn returns.
        async fn set_stop_reason(&self, sr: StopReason) {
            *self.stop_reason.lock().await = sr;
        }

        async fn set_terminal_usage(&self, total: u64, input: u64, output: u64) {
            *self.terminal_usage.lock().await = Some((total, input, output));
        }

        /// Arm the reverse `session/request_permission` path: the prompt turn will
        /// issue a permission request offering `options` and record the client's reply.
        async fn arm_permission(&self, options: Vec<PermissionOption>) {
            *self.permission_options.lock().await = options;
            self.request_permission.store(true, Ordering::SeqCst);
        }

        /// Advertise a single agent auth method with id `id` at `initialize`.
        async fn advertise_auth_method(&self, id: &'static str) {
            *self.auth_methods.lock().await = vec![AuthMethod::Agent(AuthMethodAgent::new(
                AuthMethodId::new(id),
                id,
            ))];
        }
    }

    /// Build the `[allow_once, reject_once]` option pair used by the permission
    /// tests, with stable ids `"a"` / `"r"`.
    fn allow_reject_options() -> Vec<PermissionOption> {
        vec![
            PermissionOption::new(
                PermissionOptionId::new("a"),
                "Allow once",
                PermissionOptionKind::AllowOnce,
            ),
            PermissionOption::new(
                PermissionOptionId::new("r"),
                "Reject once",
                PermissionOptionKind::RejectOnce,
            ),
        ]
    }

    /// A `PolicyEngine` double that always DENIES (returns `PermissionDenied`),
    /// so the deny-path option mapping can be asserted deterministically.
    struct DenyPolicy;
    impl PolicyEngine for DenyPolicy {
        fn decide(
            &self,
            _req: &PermissionRequest,
            _ctx: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Err(BridgeError::PermissionDenied)
        }
    }

    struct DeferPolicy;
    impl PolicyEngine for DeferPolicy {
        fn decide(
            &self,
            _req: &PermissionRequest,
            _ctx: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Ok(PermissionDecision::Approve)
        }

        fn interactive_decide(
            &self,
            _req: &PermissionRequest,
            _ctx: &SessionContext,
        ) -> bridge_core::ports::PolicyOutcome {
            bridge_core::ports::PolicyOutcome::Defer
        }
    }

    fn spike_permission_request() -> RequestPermissionRequest {
        RequestPermissionRequest::new(
            AgentSessionId::new("agent-sess-perm"),
            ToolCallUpdate::new(
                ToolCallId::new("tool-1"),
                ToolCallUpdateFields::new()
                    .kind(ToolKind::Execute)
                    .status(ToolCallStatus::Pending)
                    .title("create /tmp/x.txt")
                    .raw_input(serde_json::json!({
                        "command": ["/bin/zsh", "-lc", "create /tmp/x.txt"],
                        "cwd": "/tmp",
                    })),
            ),
            vec![
                PermissionOption::new(
                    PermissionOptionId::new("approved"),
                    "Yes, proceed",
                    PermissionOptionKind::AllowOnce,
                ),
                PermissionOption::new(
                    PermissionOptionId::new("approved-execpolicy-amendment"),
                    "Yes, and remember this command pattern",
                    PermissionOptionKind::AllowAlways,
                ),
                PermissionOption::new(
                    PermissionOptionId::new("abort"),
                    "No",
                    PermissionOptionKind::RejectOnce,
                ),
            ],
        )
    }

    fn selected_option_id(outcome: &RequestPermissionOutcome) -> Option<&str> {
        match outcome {
            RequestPermissionOutcome::Selected(sel) => Some(sel.option_id.0.as_ref()),
            RequestPermissionOutcome::Cancelled => None,
            _ => None,
        }
    }

    fn policy_handle(policy: Arc<dyn PolicyEngine>) -> PolicyHandle {
        Arc::new(StdMutex::new(policy))
    }

    async fn wait_pending(
        reg: &Arc<bridge_core::permission::PermissionRegistry>,
        ctx: &bridge_core::ids::ContextId,
    ) -> bridge_core::permission::PendingPermissionView {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let pending = reg.pending(ctx);
                if let Some(view) = pending.into_iter().next() {
                    return view;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("permission should be registered")
    }

    async fn defer_and_resolve(
        decision: bridge_core::domain::PermitDecision,
    ) -> RequestPermissionOutcome {
        let req = spike_permission_request();
        let meta = turn_meta("ctx-permit", 7, "op-permit");
        let key = bridge_core::permission::PermKey {
            context_id: meta.context_id.clone(),
            generation: meta.generation,
            op: meta.op.clone(),
            request_id: "tool-1".to_string(),
        };
        let reg = bridge_core::permission::PermissionRegistry::new();
        let policy = policy_handle(Arc::new(DeferPolicy));
        let reg_for_task = Arc::clone(&reg);
        let task = tokio::spawn(async move {
            AcpBackend::resolve_permission_outcome(
                &policy,
                Some(&reg_for_task),
                Some(meta),
                None,
                1_000,
                &req,
            )
            .await
        });

        let view = wait_pending(&reg, &key.context_id).await;
        assert_eq!(view.request_id, "tool-1");
        assert_eq!(view.tool_call_id, "tool-1");
        assert_eq!(view.generation, 7);
        assert_eq!(view.op, key.op);
        assert_eq!(view.title, "create /tmp/x.txt");
        assert_eq!(view.options.len(), 3);
        assert_eq!(view.options[0].option_id, "approved");
        assert_eq!(view.options[0].kind, "allow_once");
        assert!(view
            .raw_input
            .as_deref()
            .is_some_and(|raw| raw.contains("create /tmp/x.txt")));
        assert!(reg.resolve(
            &key,
            bridge_core::permission::PermissionResolution::Decided(decision)
        ));
        task.await.expect("resolver task should finish")
    }

    #[tokio::test]
    async fn auto_decide_is_byte_identical_and_no_registry_entry() {
        let req = spike_permission_request();
        let reg = bridge_core::permission::PermissionRegistry::new();
        let ctx = bridge_core::ids::ContextId::parse("ctx-auto").unwrap();
        let policy = policy_handle(Arc::new(AutoApprovePolicy));

        let old = AcpBackend::decide_permission(&policy, &req);
        let resolved = AcpBackend::resolve_permission_outcome(
            &policy,
            Some(&reg),
            Some(turn_meta("ctx-auto", 1, "op-auto")),
            None,
            1_000,
            &req,
        )
        .await;

        assert_eq!(resolved, old);
        assert_eq!(selected_option_id(&resolved), Some("approved"));
        assert!(reg.pending(&ctx).is_empty());
    }

    #[tokio::test]
    async fn defer_registers_then_approve_selects_allow() {
        let outcome =
            defer_and_resolve(bridge_core::domain::PermitDecision::Approve { option_id: None })
                .await;

        assert_eq!(selected_option_id(&outcome), Some("approved"));
    }

    #[tokio::test]
    async fn approve_with_reject_option_id_does_not_invert() {
        let outcome = defer_and_resolve(bridge_core::domain::PermitDecision::Approve {
            option_id: Some("abort".into()),
        })
        .await;

        assert_eq!(selected_option_id(&outcome), Some("approved"));
    }

    #[tokio::test]
    async fn defer_modify_selects_named_option() {
        let outcome = defer_and_resolve(bridge_core::domain::PermitDecision::Modify {
            option_id: "approved-execpolicy-amendment".to_string(),
            note: None,
        })
        .await;

        assert_eq!(
            selected_option_id(&outcome),
            Some("approved-execpolicy-amendment")
        );
    }

    #[tokio::test]
    async fn defer_deny_and_policy_denied_select_same_reject() {
        let defer_outcome = defer_and_resolve(bridge_core::domain::PermitDecision::Deny {
            option_id: None,
            reason: None,
        })
        .await;

        let req = spike_permission_request();
        let denied_policy = policy_handle(Arc::new(DenyPolicy));
        let denied_outcome = AcpBackend::resolve_permission_outcome(
            &denied_policy,
            None,
            Some(turn_meta("ctx-deny", 1, "op-deny")),
            None,
            1_000,
            &req,
        )
        .await;

        assert_eq!(defer_outcome, denied_outcome);
        assert_eq!(selected_option_id(&defer_outcome), Some("abort"));
    }

    #[tokio::test]
    async fn deny_with_allow_option_id_does_not_invert() {
        let outcome = defer_and_resolve(bridge_core::domain::PermitDecision::Deny {
            option_id: Some("approved".into()),
            reason: None,
        })
        .await;

        assert_eq!(selected_option_id(&outcome), Some("abort"));
    }

    #[tokio::test]
    async fn cancel_then_late_permission_does_not_park() {
        let req = spike_permission_request();
        let meta = turn_meta("ctx-cancel-late", 5, "op-cancel-late");
        let ctx = meta.context_id.clone();
        let reg = bridge_core::permission::PermissionRegistry::new();
        let policy = policy_handle(Arc::new(DeferPolicy));
        let cancelled = Arc::new(AtomicBool::new(true));

        let outcome = tokio::time::timeout(
            Duration::from_millis(100),
            AcpBackend::resolve_permission_outcome(
                &policy,
                Some(&reg),
                Some(meta),
                Some(cancelled),
                120_000,
                &req,
            ),
        )
        .await
        .expect("cancelled route must not wait for permission timeout");

        assert_eq!(outcome, RequestPermissionOutcome::Cancelled);
        assert!(reg.pending(&ctx).is_empty());
    }

    #[tokio::test]
    async fn defer_timeout_defaults_deny() {
        let req = spike_permission_request();
        let meta = turn_meta("ctx-timeout", 3, "op-timeout");
        let ctx = meta.context_id.clone();
        let reg = bridge_core::permission::PermissionRegistry::new();
        let policy = policy_handle(Arc::new(DeferPolicy));

        let outcome =
            AcpBackend::resolve_permission_outcome(&policy, Some(&reg), Some(meta), None, 30, &req)
                .await;

        assert_eq!(selected_option_id(&outcome), Some("abort"));
        assert!(reg.pending(&ctx).is_empty());
    }

    #[tokio::test]
    async fn defer_without_turn_meta_default_denies() {
        let req = spike_permission_request();
        let reg = bridge_core::permission::PermissionRegistry::new();
        let ctx = bridge_core::ids::ContextId::parse("ctx-no-meta").unwrap();
        let policy = policy_handle(Arc::new(DeferPolicy));

        let outcome =
            AcpBackend::resolve_permission_outcome(&policy, Some(&reg), None, None, 1_000, &req)
                .await;

        assert_eq!(selected_option_id(&outcome), Some("abort"));
        assert!(reg.pending(&ctx).is_empty());
    }

    /// Spawn the recording fake agent on `channel`, wired to `rec`'s shared state.
    fn spawn_recording_agent(channel: Channel, rec: Recorder) {
        tokio::spawn(async move {
            let r_init = rec.clone();
            let r_auth = rec.clone();
            let r_new = rec.clone();
            let r_mode = rec.clone();
            let r_config = rec.clone();
            let r_prompt = rec.clone();
            let r_cancel = rec.clone();
            let _ = Agent
                .builder()
                .name("recording-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| {
                        let r = r_init.clone();
                        async move {
                            // Advertise the configured auth methods (default: none)
                            // so the backend's `authenticate` step can be exercised.
                            let methods = r.auth_methods.lock().await.clone();
                            responder.respond(
                                InitializeResponse::new(ProtocolVersion::V1).auth_methods(methods),
                            )?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: AuthenticateRequest,
                          responder: agent_client_protocol::Responder<AuthenticateResponse>,
                          _cx| {
                        let r = r_auth.clone();
                        async move {
                            r.authenticates
                                .lock()
                                .await
                                .push(req.method_id.0.to_string());
                            if r.reject_authenticate.load(Ordering::SeqCst) {
                                responder.respond_with_internal_error("auth rejected")?;
                            } else {
                                responder.respond(AuthenticateResponse::new())?;
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: SetSessionModeRequest,
                          responder: agent_client_protocol::Responder<SetSessionModeResponse>,
                          _cx| {
                        let r = r_mode.clone();
                        async move {
                            r.set_modes.lock().await.push(req.mode_id.0.to_string());
                            r.set_mode_seen.notify_one();
                            if r.reject_set_mode.load(Ordering::SeqCst) {
                                responder.respond_with_internal_error("unknown mode id")?;
                            } else {
                                responder.respond(SetSessionModeResponse::new())?;
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: SetSessionConfigOptionRequest,
                          responder: agent_client_protocol::Responder<
                        SetSessionConfigOptionResponse,
                    >,
                          _cx| {
                        let r = r_config.clone();
                        async move {
                            let call_number = {
                                let mut set_config_options = r.set_config_options.lock().await;
                                set_config_options
                                    .push((req.config_id.0.to_string(), req.value.0.to_string()));
                                set_config_options.len()
                            };
                            r.set_config_seen.notify_one();
                            let reject_call = *r.reject_set_config_call.lock().await;
                            if r.reject_set_config.load(Ordering::SeqCst)
                                || reject_call == Some(call_number)
                            {
                                let body = r.set_config_error_body.lock().await.clone();
                                responder.respond_with_internal_error(body)?;
                            } else {
                                r.refresh_config_current(&req.config_id.0, &req.value.0)
                                    .await;
                                let config_options =
                                    if r.empty_config_response_on_set.load(Ordering::SeqCst) {
                                        Vec::new()
                                    } else {
                                        r.advertised_config_options().await
                                    };
                                responder
                                    .respond(SetSessionConfigOptionResponse::new(config_options))?;
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: NewSessionRequest,
                          responder: agent_client_protocol::Responder<NewSessionResponse>,
                          _cx| {
                        let r = r_new.clone();
                        async move {
                            r.new_session_calls.fetch_add(1, Ordering::SeqCst);
                            // Record the cwd the client sent so Task-4 tests can
                            // assert the correct cwd reached the wire.
                            *r.new_session_cwd.lock().await = Some(req.cwd);
                            // Signal entry BEFORE awaiting the gate so a driver can
                            // deterministically know the mint is in flight (no sleep).
                            r.new_session_started.notify_one();
                            if r.gate_new_session.load(Ordering::SeqCst) {
                                // Hold the reply until the test opens the gate,
                                // widening the create/cancel + concurrency window.
                                r.new_session_gate.notified().await;
                            }
                            let modes = r.session_modes.lock().await.clone();
                            responder.respond(
                                NewSessionResponse::new(AgentSessionId::new(r.minted_id))
                                    .modes(modes)
                                    .config_options(r.advertised_config_options().await),
                            )?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: PromptRequest,
                          responder: agent_client_protocol::Responder<PromptResponse>,
                          cx: ConnectionTo<Client>| {
                        let r = r_prompt.clone();
                        async move {
                            r.prompt_log.lock().await.push("start");
                            r.prompt_started.notify_one();

                            // Offload the WHOLE turn body (update emission + optional
                            // reverse permission request + gate-wait + response) to a
                            // spawned task via `cx.spawn`, then RETURN from the handler
                            // immediately. REQUIRED: SDK request handlers run inside the
                            // dispatch loop and block all further message processing
                            // while awaiting — so a handler that parked (on a gate, or
                            // on the client's reply to a reverse permission request)
                            // would prevent the agent from dispatching incoming messages
                            // (the cancel, the permission reply). Spawning frees the loop.
                            let r2 = r.clone();
                            let sid = req.session_id.clone();
                            let cx2 = cx.clone();
                            cx.spawn(async move {
                                // Helper: emit the scripted `session/update`s (the
                                // streaming fan-in the backend routes). `cx2` is the
                                // agent's connection to the client; a `SessionNotification`
                                // is the wire `session/update`.
                                let emit_updates = || async {
                                    let updates = r2.prompt_updates.lock().await.clone();
                                    for u in updates {
                                        let update = match u {
                                            ScriptedUpdate::Text(t) => {
                                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                                    ContentBlock::Text(TextContent::new(t)),
                                                ))
                                            }
                                            ScriptedUpdate::Thought(t) => {
                                                SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                                    ContentBlock::Text(TextContent::new(t)),
                                                ))
                                            }
                                            ScriptedUpdate::Delay(delay) => {
                                                tokio::time::sleep(delay).await;
                                                continue;
                                            }
                                            ScriptedUpdate::Plan => SessionUpdate::Plan(
                                                agent_client_protocol::schema::v1::Plan::new(
                                                    vec![],
                                                ),
                                            ),
                                            ScriptedUpdate::ToolCall => SessionUpdate::ToolCall(
                                                ToolCall::new("tool-1", "read file")
                                                    .kind(ToolKind::Read),
                                            ),
                                            ScriptedUpdate::Usage(used, size) => {
                                                SessionUpdate::UsageUpdate(
                                                    agent_client_protocol::schema::v1::UsageUpdate::new(
                                                        used, size,
                                                    ),
                                                )
                                            }
                                        };
                                        cx2.send_notification(SessionNotification::new(
                                            sid.clone(),
                                            update,
                                        ))?;
                                    }
                                    Ok::<(), agent_client_protocol::Error>(())
                                };

                                // Whether the rest of the turn (chunks + response) is
                                // gated on the reverse permission reply.
                                let gate_on_perm = r2.request_permission.load(Ordering::SeqCst)
                                    && r2.gate_turn_on_permission.load(Ordering::SeqCst);

                                // If NOT gating on the permission, emit the chunks now
                                // (preserves the original "stream then respond" order).
                                if !gate_on_perm {
                                    emit_updates().await?;
                                }

                                // Optionally issue a reverse `session/request_permission`
                                // BACK to the client mid-turn and record its reply. This
                                // is the bidirectional-peer path the backend's handler
                                // answers. We send the request and `block_task().await`
                                // the reply — which can ONLY arrive if the client's
                                // permission handler did not stall the client's loop.
                                if r2.request_permission.load(Ordering::SeqCst) {
                                    r2.permission_requested.notify_one();
                                    let options = r2.permission_options.lock().await.clone();
                                    let perm_req = RequestPermissionRequest::new(
                                        sid.clone(),
                                        ToolCallUpdate::new(
                                            ToolCallId::new("tool-1"),
                                            ToolCallUpdateFields::default(),
                                        ),
                                        options,
                                    );
                                    let resp: RequestPermissionResponse =
                                        cx2.send_request(perm_req).block_task().await?;
                                    let recorded = match resp.outcome {
                                        RequestPermissionOutcome::Selected(sel) => {
                                            Some(sel.option_id.0.to_string())
                                        }
                                        RequestPermissionOutcome::Cancelled => None,
                                        _ => None,
                                    };
                                    *r2.permission_reply.lock().await = Some(recorded);
                                    r2.permission_replied.notify_one();
                                }

                                // If gating on the permission, NOW emit the remaining
                                // chunks (after the reply) — so a stalled client loop
                                // would prevent them (and the response) from ever arriving.
                                if gate_on_perm {
                                    emit_updates().await?;
                                }

                                // Optionally hold the turn open until released, so a
                                // second concurrent turn — if the lock failed — would
                                // interleave and be caught by the ordering log.
                                if r2.gate_prompt.load(Ordering::SeqCst) {
                                    r2.prompt_gate.notified().await;
                                }
                                // Optionally WAIT for `session/cancel` before ending
                                // the turn (agent only ends once it sees the cancel).
                                // `notify_one` holds a permit if the cancel already
                                // arrived, so this is race-safe for the single cancel
                                // these tests send.
                                if r2.wait_cancel_before_respond.load(Ordering::SeqCst) {
                                    r2.cancel_arrived.notified().await;
                                    // Optionally HANG forever after observing the
                                    // cancel (agent ignores it): backend must escalate.
                                    if r2.hang_after_cancel.load(Ordering::SeqCst) {
                                        std::future::pending::<()>().await;
                                    }
                                }
                                // Optionally FAIL the turn: respond with a JSON-RPC
                                // error so the client's `send_request` returns `Err`,
                                // exercising the transport/agent-error path. Logged as
                                // "fail" (not "end") so a test can distinguish.
                                if r2.fail_prompt.load(Ordering::SeqCst) {
                                    r2.prompt_log.lock().await.push("fail");
                                    if let Some(error) = r2.prompt_error.lock().await.clone() {
                                        responder.respond_with_error(error)?;
                                    } else {
                                        responder.respond_with_internal_error(
                                            "agent failed the turn",
                                        )?;
                                    }
                                    return Ok(());
                                }
                                r2.prompt_log.lock().await.push("end");
                                let sr = *r2.stop_reason.lock().await;
                                let mut response = PromptResponse::new(sr);
                                if let Some((total, input, output)) =
                                    *r2.terminal_usage.lock().await
                                {
                                    response = response.usage(Usage::new(total, input, output));
                                }
                                responder.respond(response)?;
                                Ok(())
                            })?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_notification(
                    move |notif: CancelNotification, _cx| {
                        let r = r_cancel.clone();
                        async move {
                            r.cancels.lock().await.push(notif.session_id.0.to_string());
                            r.cancel_seen.notify_one();
                            r.cancel_arrived.notify_one();
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    /// Build a backend connected to a fresh recording agent; returns both.
    async fn connect_recording(rec: Recorder) -> AcpBackend {
        connect_recording_with(rec, test_config()).await
    }

    /// Like [`connect_recording`] but with a caller-supplied config (e.g. a short
    /// `cancel_grace` so the hung-agent escalation can be asserted deterministically).
    async fn connect_recording_with(rec: Recorder, config: AcpConfig) -> AcpBackend {
        let (client_side, agent_side) = Channel::duplex();
        spawn_recording_agent(agent_side, rec);
        AcpBackend::connect(client_side, config)
            .await
            .expect("initialize handshake succeeds against recording agent")
    }

    async fn connect_recording_observed_with(
        rec: Recorder,
        config: AcpConfig,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<AcpBackend, BridgeError> {
        let (client_side, agent_side) = Channel::duplex();
        spawn_recording_agent(agent_side, rec);
        AcpBackend::connect_observed(client_side, config, observer).await
    }

    fn spawn_session_rejecting_agent(
        channel: Channel,
        message: &'static str,
    ) -> Arc<Mutex<Option<PathBuf>>> {
        let recorded_cwd = Arc::new(Mutex::new(None));
        let recorded_cwd_for_agent = Arc::clone(&recorded_cwd);
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("session-rejecting-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| async move {
                        responder.respond(
                            InitializeResponse::new(ProtocolVersion::V1)
                                .agent_capabilities(AgentCapabilities::default()),
                        )?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |req: NewSessionRequest,
                          responder: agent_client_protocol::Responder<
                        agent_client_protocol::schema::v1::NewSessionResponse,
                    >,
                          _cx| {
                        let recorded_cwd = Arc::clone(&recorded_cwd_for_agent);
                        async move {
                            *recorded_cwd.lock().await = Some(req.cwd);
                            responder.respond_with_internal_error(message)?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
        recorded_cwd
    }

    fn bkey(s: &str) -> SessionId {
        SessionId::parse(s).unwrap()
    }

    fn assert_agent_failure(
        error: &BridgeError,
        phase: DiagnosticPhase,
        class: DiagnosticFailureClass,
        accepted: bool,
    ) -> &FailureDiagnostic {
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("expected structured agent failure, got {error:?}");
        };
        assert_eq!(diagnostic.failed_phase(), phase);
        assert_eq!(diagnostic.class(), class);
        assert_eq!(diagnostic.prompt_may_have_been_accepted(), accepted);
        diagnostic
    }

    fn turn_meta(ctx: &str, generation: u64, op: &str) -> bridge_core::permission::TurnMeta {
        bridge_core::permission::TurnMeta {
            context_id: bridge_core::ids::ContextId::parse(ctx).unwrap(),
            generation,
            op: bridge_core::ids::OperationId::parse(op).unwrap(),
        }
    }

    #[tokio::test]
    async fn observed_auth_paths_round_trip_typed_evidence() {
        let cases = [
            ("pre_authenticated", true, Some("chat-gpt"), None),
            ("configured_method", false, Some("oauth"), Some("oauth")),
            ("selected_advertised_method", false, Some("oauth"), None),
            ("no_methods_advertised", false, None, None),
        ];

        for (expected_kind, pre_authenticated, advertised, configured) in cases {
            let rec = Recorder::new("agent-sess-AUTH-EVIDENCE");
            if let Some(method) = advertised {
                rec.advertise_auth_method(method).await;
            }
            let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
            let config = AcpConfig {
                pre_authenticated,
                auth_method: configured.map(str::to_owned),
                ..test_config()
            };
            let backend = connect_recording_observed_with(rec, config, observer.clone())
                .await
                .unwrap_or_else(|error| panic!("{expected_kind} connect failed: {error:?}"));
            drop(backend);

            let auth_terminal = observer
                .snapshot()
                .await
                .into_iter()
                .find(|event| {
                    event.transition().phase() == DiagnosticPhase::Authenticate
                        && event.transition().status() != PhaseStatus::Started
                })
                .expect("authenticate terminal transition");
            let json = serde_json::to_value(&auth_terminal).unwrap();
            assert_eq!(json["transition"]["auth"]["kind"], expected_kind);
            let round_trip: bridge_core::diagnostics::DiagnosticEvent =
                serde_json::from_value(json).unwrap();
            assert_eq!(round_trip, auth_terminal);
        }
    }

    #[tokio::test]
    async fn observed_session_creation_failure_stays_in_session_phase() {
        let (client_side, agent_side) = Channel::duplex();
        spawn_session_rejecting_agent(agent_side, "session creation rejected");
        let backend = AcpBackend::connect(client_side, test_config())
            .await
            .unwrap();
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let error = match backend
            .prompt_with_observers(
                &bkey("bridge-SESSION-REJECT"),
                vec![],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("rejected session/new must fail before prompt"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert!(diagnostic
            .causes()
            .last()
            .is_some_and(|cause| cause.contains("session creation rejected")));
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
        assert!(events
            .iter()
            .all(|event| event.transition().phase() == DiagnosticPhase::SessionCreate));
    }

    #[tokio::test]
    async fn lifecycle_failure_redacts_bridge_known_credential_from_sdk_cause() {
        const SECRET: &str = "bridge-known-lifecycle-credential";
        const ERROR: &str = "session creation rejected with bridge-known-lifecycle-credential";
        let (client_side, agent_side) = Channel::duplex();
        spawn_session_rejecting_agent(agent_side, ERROR);
        let config = AcpConfig {
            diagnostic_redactor: DiagnosticRedactor::new([SECRET]),
            ..test_config()
        };
        let backend = AcpBackend::connect(client_side, config).await.unwrap();
        let error = match backend.prompt(&bkey("bridge-SESSION-REDACT"), vec![]).await {
            Err(error) => error,
            Ok(_) => panic!("rejected session/new must fail"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(SECRET));
        assert!(rendered.contains("REDACTED KNOWN SECRET"));
    }

    #[tokio::test]
    async fn lifecycle_failure_redacts_mcp_secret_expanded_with_session_cwd() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const EFFECTIVE: &str = "alpha/srv/requestomega";
        const ERROR: &str = "session creation rejected with alpha/srv/requestomega";
        let (client_side, agent_side) = Channel::duplex();
        spawn_session_rejecting_agent(agent_side, ERROR);
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let backend = AcpBackend::connect(client_side, config).await.unwrap();
        let session = bkey("bridge-SESSION-CWD-REDACT");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/request").unwrap()),
                },
            )
            .await
            .unwrap();

        let error = match backend.prompt(&session, vec![]).await {
            Err(error) => error,
            Ok(_) => panic!("rejected session/new must fail"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(EFFECTIVE));
        assert!(rendered.contains("REDACTED KNOWN SECRET"));
    }

    #[tokio::test]
    async fn mint_inputs_and_redactor_share_one_session_config_snapshot() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const EFFECTIVE_A: &str = "alpha/srv/aomega";
        const ERROR_A: &str = "session creation rejected with alpha/srv/aomega";
        let (client_side, agent_side) = Channel::duplex();
        let recorded_cwd = spawn_session_rejecting_agent(agent_side, ERROR_A);
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let backend = AcpBackend::connect(client_side, config).await.unwrap();
        let session = bkey("bridge-SESSION-SNAPSHOT-REDACT");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/a").unwrap()),
                },
            )
            .await
            .unwrap();
        let snapshot = backend.session_config_snapshot(&session).unwrap();
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/b").unwrap()),
                },
            )
            .await
            .unwrap();

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = backend
            .ensure_session_observed_with_snapshot(&session, observer, snapshot)
            .await
            .expect_err("the fake agent rejects session/new");
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            recorded_cwd.lock().await.as_deref(),
            Some(std::path::Path::new("/srv/a"))
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(EFFECTIVE_A));
        assert!(rendered.contains("REDACTED KNOWN SECRET"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prompt_inner_keeps_snapshot_atomic_across_config_replacement() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const EFFECTIVE_A: &str = "alpha/srv/prompt-aomega";
        const ERROR_A: &str = "session creation rejected with alpha/srv/prompt-aomega";
        let (client_side, agent_side) = Channel::duplex();
        let recorded_cwd = spawn_session_rejecting_agent(agent_side, ERROR_A);
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let backend = AcpBackend::connect(client_side, config).await.unwrap();
        let session = bkey("bridge-PROMPT-SNAPSHOT-REDACT");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/prompt-a").unwrap()),
                },
            )
            .await
            .unwrap();
        let snapshot_entered = Arc::new(std::sync::Barrier::new(2));
        let release_snapshot = Arc::new(std::sync::Barrier::new(2));
        *backend.prompt_snapshot_hook.lock().unwrap() = Some(Arc::new({
            let snapshot_entered = Arc::clone(&snapshot_entered);
            let release_snapshot = Arc::clone(&release_snapshot);
            move || {
                snapshot_entered.wait();
                release_snapshot.wait();
            }
        }));
        let backend = Arc::new(backend);
        let backend_for_prompt = Arc::clone(&backend);
        let session_for_prompt = session.clone();
        let prompt =
            tokio::spawn(
                async move { backend_for_prompt.prompt(&session_for_prompt, vec![]).await },
            );
        snapshot_entered.wait();
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/prompt-b").unwrap()),
                },
            )
            .await
            .unwrap();
        release_snapshot.wait();

        let error = match prompt.await.unwrap() {
            Err(error) => error,
            Ok(_) => panic!("the fake agent rejects session/new"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            recorded_cwd.lock().await.as_deref(),
            Some(std::path::Path::new("/srv/prompt-a"))
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(EFFECTIVE_A));
        assert!(rendered.contains("REDACTED KNOWN SECRET"));
    }

    #[tokio::test]
    async fn teardown_redactor_keeps_minted_cwd_after_config_replacement() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const MINTED_SECRET: &str = "alpha/srv/mintedomega";
        let rec = Recorder::new("agent-sess-TEARDOWN-REDACT");
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let backend = connect_recording_with(rec, config).await;
        let session = bkey("bridge-TEARDOWN-REDACT");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/minted").unwrap()),
                },
            )
            .await
            .unwrap();
        backend.ensure_session(&session).await.unwrap();
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/replacement").unwrap()),
                },
            )
            .await
            .unwrap();

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let lifecycle = backend.operation_lifecycle(&session, observer).await;
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Started,
                None,
                Some("acp.teardown.test"),
                None,
            )
            .await
            .unwrap();
        let error = lifecycle
            .failure(
                DiagnosticPhase::Teardown,
                None,
                DiagnosticFailureClass::Transport,
                FailureDisposition::Fatal,
                "acp.teardown.test_failure",
                "ACP teardown test failure",
                Some(format!("agent echoed {MINTED_SECRET}")),
                false,
                None,
                None,
                None,
            )
            .await;
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            false,
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(MINTED_SECRET));
        assert!(rendered.contains("REDACTED KNOWN SECRET"));
    }

    #[tokio::test]
    async fn teardown_redactor_covers_active_attempt_after_config_replacement() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const ACTIVE_SECRET: &str = "alpha/srv/activeomega";
        let rec = Recorder::new("agent-sess-ACTIVE-REDACT");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let backend = Arc::new(connect_recording_with(rec.clone(), config).await);
        let session = bkey("bridge-ACTIVE-REDACT");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/active").unwrap()),
                },
            )
            .await
            .unwrap();
        let backend_for_mint = Arc::clone(&backend);
        let session_for_mint = session.clone();
        let mint =
            tokio::spawn(async move { backend_for_mint.ensure_session(&session_for_mint).await });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must hold the active attempted cwd");
        backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/replacement").unwrap()),
                },
            )
            .await
            .unwrap();

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let lifecycle = backend.operation_lifecycle(&session, observer).await;
        lifecycle
            .record(
                DiagnosticPhase::Teardown,
                PhaseStatus::Started,
                None,
                Some("acp.teardown.test"),
                None,
            )
            .await
            .unwrap();
        let error = lifecycle
            .failure(
                DiagnosticPhase::Teardown,
                None,
                DiagnosticFailureClass::Transport,
                FailureDisposition::Fatal,
                "acp.teardown.test_failure",
                "ACP teardown test failure",
                Some(format!("agent echoed {ACTIVE_SECRET}")),
                false,
                None,
                None,
                None,
            )
            .await;
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            false,
        );
        let rendered = serde_json::to_string(diagnostic).unwrap();
        assert!(!rendered.contains(ACTIVE_SECRET));
        rec.new_session_gate.notify_waiters();
        mint.await.unwrap().unwrap();
        let entry = backend.session_entry(&session).await;
        assert!(entry.active_mint_cwd.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn session_mint_updates_process_redactor_before_request_and_preserves_live_cwds() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        const FIRST_SECRET: &str = "alpha/srv/process-aomega";
        const SECOND_SECRET: &str = "alpha/srv/process-bomega";
        let rec = Recorder::new("agent-sess-PROCESS-REDACTOR");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let mut backend = connect_recording_with(rec.clone(), config).await;
        let stderr_ring = ProcessStderrRing::default();
        backend.stderr_ring = Some(stderr_ring.clone());
        let backend = Arc::new(backend);

        let first = bkey("bridge-PROCESS-REDACTOR-A");
        backend
            .configure_session(
                &first,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/process-a").unwrap()),
                },
            )
            .await
            .unwrap();
        let backend_for_first = Arc::clone(&backend);
        let first_for_mint = first.clone();
        let first_mint =
            tokio::spawn(async move { backend_for_first.ensure_session(&first_for_mint).await });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("first session/new must observe its process redactor");
        assert!(
            format!("{stderr_ring:?}").contains("known_value_count: 2"),
            "raw template plus first effective cwd must be installed before session/new"
        );
        assert!(
            !backend
                .diagnostic_redactor_for_cwds(&["/srv/process-a".to_owned()])
                .sanitize_stderr_line(FIRST_SECRET, 1024)
                .contains(FIRST_SECRET),
            "the installed effective-cwd value must sanitize the exact delivered credential"
        );
        rec.new_session_gate.notify_waiters();
        first_mint.await.unwrap().unwrap();

        let second = bkey("bridge-PROCESS-REDACTOR-B");
        backend
            .configure_session(
                &second,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/process-b").unwrap()),
                },
            )
            .await
            .unwrap();
        let backend_for_second = Arc::clone(&backend);
        let second_for_mint = second.clone();
        let second_mint =
            tokio::spawn(async move { backend_for_second.ensure_session(&second_for_mint).await });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("second session/new must observe the union redactor");
        assert!(
            format!("{stderr_ring:?}").contains("known_value_count: 3"),
            "raw template plus both live effective cwd values must remain installed"
        );
        let union_redactor = backend.diagnostic_redactor_for_cwds(&[
            "/srv/process-a".to_owned(),
            "/srv/process-b".to_owned(),
        ]);
        assert!(
            !union_redactor
                .sanitize_stderr_line(FIRST_SECRET, 1024)
                .contains(FIRST_SECRET),
            "installing the second cwd must not drop the first live credential"
        );
        assert!(
            !union_redactor
                .sanitize_stderr_line(SECOND_SECRET, 1024)
                .contains(SECOND_SECRET),
            "the second effective-cwd credential must also be sanitized"
        );
        rec.new_session_gate.notify_waiters();
        second_mint.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_while_process_redactor_lock_is_contended_clears_active_cwd() {
        let rec = Recorder::new("agent-sess-PROCESS-REDACTOR-CANCEL");
        let mut backend = connect_recording(rec.clone()).await;
        backend.stderr_ring = Some(ProcessStderrRing::default());
        let key = bkey("bridge-PROCESS-REDACTOR-CANCEL");
        let entry = backend.session_entry(&key).await;

        let before_redactor = Arc::new(std::sync::Barrier::new(2));
        let release_redactor = Arc::new(std::sync::Barrier::new(2));
        *backend.before_process_redactor_hook.lock().unwrap() = Some(Arc::new({
            let before_redactor = Arc::clone(&before_redactor);
            let release_redactor = Arc::clone(&release_redactor);
            move || {
                before_redactor.wait();
                release_redactor.wait();
            }
        }));
        let backend = Arc::new(backend);
        let backend_for_mint = Arc::clone(&backend);
        let key_for_mint = key.clone();
        let mint =
            tokio::spawn(async move { backend_for_mint.ensure_session(&key_for_mint).await });

        before_redactor.wait();
        let sessions_guard = backend.sessions.lock().await;
        release_redactor.wait();
        tokio::task::yield_now().await;
        assert!(
            entry.active_mint_cwd.lock().unwrap().is_some(),
            "attempt cwd must be published while redactor installation waits"
        );

        mint.abort();
        let cancelled = tokio::time::timeout(Duration::from_secs(2), mint)
            .await
            .expect("cancelled pre-initializer future must stop promptly")
            .expect_err("aborted pre-initializer future must not succeed");
        assert!(cancelled.is_cancelled());
        assert!(
            entry.active_mint_cwd.lock().unwrap().is_none(),
            "guard created at publication must clear cwd without acquiring the sessions lock"
        );
        drop(sessions_guard);
        assert!(entry.agent_id.get().is_none());
        assert_eq!(rec.new_session_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_credential_bearing_mint_makes_process_metadata_only_across_next_mint() {
        const TEMPLATE: &str = "alpha{cwd}omega";
        let rec = Recorder::new("agent-sess-PROCESS-METADATA-ONLY");
        let config = AcpConfig {
            mcp: vec![bridge_core::mcp::McpServerSpec {
                name: "secret-server".to_owned(),
                command: "secret-server".to_owned(),
                args: Vec::new(),
                env: vec![("TOKEN".to_owned(), TEMPLATE.to_owned())],
            }],
            diagnostic_redactor: DiagnosticRedactor::new([TEMPLATE]),
            ..test_config()
        };
        let mut backend = connect_recording_with(rec.clone(), config).await;
        let stderr_ring = ProcessStderrRing::default();
        backend.stderr_ring = Some(stderr_ring.clone());

        let failed = bkey("bridge-PROCESS-METADATA-FAILED");
        backend
            .configure_session(
                &failed,
                &SessionSpec {
                    config: EffectiveConfig {
                        model: Some("not-advertised".to_owned()),
                        ..EffectiveConfig::default()
                    },
                    cwd: Some(SessionCwd::parse("/srv/failed-delivery").unwrap()),
                },
            )
            .await
            .unwrap();
        backend
            .ensure_session(&failed)
            .await
            .expect_err("model rejection after session/new must fail the mint");
        assert!(
            stderr_ring.is_metadata_only(),
            "an uncertain failed credential delivery must disable retained process text"
        );

        let next = bkey("bridge-PROCESS-METADATA-NEXT");
        backend
            .configure_session(
                &next,
                &SessionSpec {
                    config: EffectiveConfig::default(),
                    cwd: Some(SessionCwd::parse("/srv/next-delivery").unwrap()),
                },
            )
            .await
            .unwrap();
        backend.ensure_session(&next).await.unwrap();
        assert_eq!(rec.new_session_calls.load(Ordering::SeqCst), 2);
        assert!(
            stderr_ring.is_metadata_only(),
            "a later live-session redactor replacement must not re-enable process text"
        );
    }

    #[tokio::test]
    async fn releasing_minted_session_makes_process_stderr_metadata_only() {
        let rec = Recorder::new("agent-sess-RELEASE-METADATA-ONLY");
        let mut backend = connect_recording(rec).await;
        let stderr_ring = ProcessStderrRing::default();
        backend.stderr_ring = Some(stderr_ring.clone());
        let session = bkey("bridge-RELEASE-METADATA-ONLY");
        backend.ensure_session(&session).await.unwrap();
        assert!(!stderr_ring.is_metadata_only());

        backend.release_session_result(&session).await.unwrap();
        assert!(
            stderr_ring.is_metadata_only(),
            "removing bridge ownership must fail closed against late session stderr"
        );
    }

    #[tokio::test]
    async fn failed_mint_attempt_cwd_is_cleared_before_retry() {
        let (client_side, agent_side) = Channel::duplex();
        let recorded_cwd = spawn_session_rejecting_agent(agent_side, "session creation rejected");
        let backend = AcpBackend::connect(client_side, test_config())
            .await
            .unwrap();
        let session = bkey("bridge-FAILED-MINT-CWD");
        let entry = backend.session_entry(&session).await;
        for cwd in ["/srv/failed-a", "/srv/failed-b"] {
            backend
                .configure_session(
                    &session,
                    &SessionSpec {
                        config: EffectiveConfig::default(),
                        cwd: Some(SessionCwd::parse(cwd).unwrap()),
                    },
                )
                .await
                .unwrap();
            assert!(backend.ensure_session(&session).await.is_err());
            assert!(
                entry.active_mint_cwd.lock().unwrap().is_none(),
                "a failed attempt must not retain its cwd"
            );
        }
        assert_eq!(
            recorded_cwd.lock().await.as_deref(),
            Some(std::path::Path::new("/srv/failed-b"))
        );
    }

    #[tokio::test]
    async fn short_known_credential_cannot_collide_with_static_lifecycle_codes() {
        let rec = Recorder::new("agent-sess-SHORT-CREDENTIAL");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let backend = connect_recording_observed_with(
            rec,
            AcpConfig {
                diagnostic_redactor: DiagnosticRedactor::new(["a"]),
                ..test_config()
            },
            observer.clone(),
        )
        .await
        .expect("a short known value must not invalidate bridge-owned static codes");

        let auth_skip = observer
            .snapshot()
            .await
            .into_iter()
            .find(|event| {
                event.transition().phase() == DiagnosticPhase::Authenticate
                    && event.transition().status() == PhaseStatus::Skipped
            })
            .expect("authenticate skip transition");
        assert_eq!(
            auth_skip.transition().code().map(|code| code.as_str()),
            Some("acp.auth.no_methods_advertised")
        );
        drop(backend);
    }

    #[tokio::test]
    async fn observed_model_rejection_fails_config_before_prompt() {
        let rec = Recorder::new("agent-sess-MODEL-DIAG");
        let config = AcpConfig {
            agent_id: "recorder".to_string(),
            model: Some("missing-model".to_string()),
            ..test_config()
        };
        let backend = connect_recording_with(rec.clone(), config).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let error = match backend
            .prompt_with_observers(
                &bkey("bridge-MODEL-DIAG"),
                vec![],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("unadvertised model must fail before a prompt stream exists"),
        };
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("model rejection must be structured, got {error:?}");
        };
        assert_eq!(diagnostic.failed_phase(), DiagnosticPhase::ConfigApply);
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Model);
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(!diagnostic.prompt_may_have_been_accepted());
        assert!(observer.snapshot().await.iter().all(|event| {
            !matches!(
                event.transition().phase(),
                DiagnosticPhase::PromptStart
                    | DiagnosticPhase::PromptStream
                    | DiagnosticPhase::PromptFinish
            )
        }));
        assert!(rec.prompt_log.lock().await.is_empty());
    }

    #[tokio::test]
    async fn observed_prompt_sdk_failure_is_post_barrier_fatal() {
        let rec = Recorder::new("agent-sess-PROMPT-DIAG");
        rec.fail_prompt.store(true, Ordering::SeqCst);
        let backend = connect_recording(rec).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &bkey("bridge-PROMPT-DIAG"),
                vec![],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
            .unwrap();
        let error = match stream.next().await {
            Some(Err(error)) => error,
            other => panic!("prompt SDK failure must be terminal, got {other:?}"),
        };
        let BridgeError::AgentFailure { diagnostic } = &error else {
            panic!("prompt SDK failure must be structured, got {error:?}");
        };
        assert_eq!(diagnostic.failed_phase(), DiagnosticPhase::PromptStream);
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Unknown);
        assert_eq!(diagnostic.code().as_str(), "upstream.unknown");
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(diagnostic.prompt_may_have_been_accepted());
        assert_eq!(diagnostic.causes(), &["Internal error"]);
        assert!(
            diagnostic
                .causes()
                .iter()
                .all(|cause| !cause.contains("agent failed the turn")),
            "arbitrary JSON-RPC data prose is not diagnostic evidence"
        );
        assert!(
            !error.is_transient(),
            "E6 must not replay post-barrier work"
        );

        let events = observer.snapshot().await;
        let stream_events: Vec<_> = events
            .iter()
            .filter(|event| event.transition().phase() == DiagnosticPhase::PromptStream)
            .collect();
        assert_eq!(stream_events.len(), 2);
        assert_eq!(stream_events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(stream_events[1].transition().status(), PhaseStatus::Failed);
        assert!(events
            .iter()
            .all(|event| event.transition().phase() != DiagnosticPhase::PromptFinish));
    }

    #[tokio::test]
    async fn observed_prompt_structured_provider_limit_preserves_bounded_hints() {
        let rec = Recorder::new("agent-sess-PROMPT-LIMIT");
        rec.fail_prompt.store(true, Ordering::SeqCst);
        let reset_at_ms = diagnostic_timestamp_ms() + 60_000;
        *rec.prompt_error.lock().await = Some(
            AcpError::new(-32_099, "opaque provider rejection").data(serde_json::json!({
                "code": "usage_limit_reached",
                "error": {"type": "rate_limit_error"},
                "retry_after_ms": 1234,
                "reset_at_ms": reset_at_ms
            })),
        );
        let backend = connect_recording(rec).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &bkey("bridge-PROMPT-LIMIT"),
                vec![],
                BackendObservers::diagnostic_only(observer),
            )
            .await
            .unwrap();

        let error = stream.next().await.unwrap().unwrap_err();
        let BridgeError::AgentFailure { diagnostic } = &error else {
            panic!("structured provider limit must remain an AgentFailure");
        };
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::ProviderLimit);
        assert_eq!(diagnostic.code().as_str(), "upstream.provider_limit");
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(diagnostic.prompt_may_have_been_accepted());
        let serialized = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(serialized["retry_after_ms"], 1234);
        assert_eq!(serialized["reset_at_ms"], reset_at_ms);
        assert!(!error.is_transient());
        assert!(!diagnostic.class().is_container_fallback_class());
    }

    #[tokio::test]
    async fn observed_prompt_prose_only_usage_limit_remains_unknown() {
        let rec = Recorder::new("agent-sess-PROMPT-PROSE");
        rec.fail_prompt.store(true, Ordering::SeqCst);
        *rec.prompt_error.lock().await = Some(AcpError::new(
            -32_099,
            "usage limit reached; retry after an hour",
        ));
        let backend = connect_recording(rec).await;
        let mut stream = backend
            .prompt(&bkey("bridge-PROMPT-PROSE"), vec![])
            .await
            .unwrap();

        let error = stream.next().await.unwrap().unwrap_err();
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("prose-only rejection must remain a structured AgentFailure");
        };
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::Unknown);
        assert_eq!(diagnostic.code().as_str(), "upstream.unknown");
    }

    #[tokio::test]
    async fn prompt_failure_uses_attempt_cursor_and_persists_stderr_metadata_only() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut process = Supervised::spawn(
            "/bin/sh",
            &[
                "-c",
                "echo old-process-line 1>&2; echo READY; read _; echo new-unlabeled-secret 1>&2; sleep 30",
            ],
            None,
        )
        .unwrap();
        let ring = process.stderr_ring();
        let stdout = process.child_mut().stdout.take().unwrap();
        let mut stdout = BufReader::new(stdout).lines();
        assert_eq!(stdout.next_line().await.unwrap().as_deref(), Some("READY"));
        for _ in 0..100 {
            if ring.metadata_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let rec = Recorder::new("agent-sess-STDERR-DIAG");
        rec.fail_prompt.store(true, Ordering::SeqCst);
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let mut backend = connect_recording(rec.clone()).await;
        backend.stderr_ring = Some(ring.clone());
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &bkey("bridge-STDERR-DIAG"),
                vec![],
                BackendObservers::diagnostic_only(observer),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt reaches gated fake agent");

        process
            .child_mut()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .await
            .unwrap();
        for _ in 0..100 {
            if ring.metadata_since(ring.origin()).line_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        rec.prompt_gate.notify_one();

        let error = match stream.next().await {
            Some(Err(error)) => error,
            other => panic!("gated prompt must fail, got {other:?}"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStream,
            DiagnosticFailureClass::Unknown,
            true,
        );
        let json = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(json["stderr_observed"], true);
        assert_eq!(json["stderr_line_count"], 1, "old line excluded by cursor");
        assert_eq!(json["stderr_scope"], "process");
        assert!(
            json.get("stderr_tail").is_none(),
            "text is disabled by default"
        );
        let encoded = serde_json::to_string(&json).unwrap();
        assert!(!encoded.contains("old-process-line"));
        assert!(!encoded.contains("new-unlabeled-secret"));
    }

    #[tokio::test]
    async fn pre_prompt_config_failure_includes_available_process_stderr_metadata() {
        use tokio::io::AsyncWriteExt;

        let mut process = Supervised::spawn(
            "/bin/sh",
            &["-c", "read _; echo preprompt-process-secret 1>&2; sleep 30"],
            None,
        )
        .unwrap();
        let ring = process.stderr_ring();

        let rec = Recorder::new("agent-sess-PREPROMPT-STDERR");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        rec.reject_set_mode.store(true, Ordering::SeqCst);
        let mut backend = connect_recording_with(
            rec.clone(),
            AcpConfig {
                mode: Some("review".to_string()),
                ..test_config()
            },
        )
        .await;
        backend.stderr_ring = Some(ring.clone());
        let backend = Arc::new(backend);
        let backend_for_prompt = Arc::clone(&backend);
        let prompt = tokio::spawn(async move {
            backend_for_prompt
                .prompt(&bkey("bridge-PREPROMPT-STDERR"), vec![])
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight");

        process
            .child_mut()
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"continue\n")
            .await
            .unwrap();
        for _ in 0..100 {
            if ring.metadata_since(ring.origin()).line_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        rec.new_session_gate.notify_waiters();

        let error = match prompt.await.unwrap() {
            Err(error) => error,
            Ok(_) => panic!("rejected mode must fail before prompt dispatch"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::ConfigApply,
            DiagnosticFailureClass::Model,
            false,
        );
        let json = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(json["stderr_observed"], true);
        assert_eq!(json["stderr_line_count"], 1);
        assert_eq!(json["stderr_scope"], "process");
        assert!(json.get("stderr_tail").is_none());
        assert!(!serde_json::to_string(&json)
            .unwrap()
            .contains("preprompt-process-secret"));
    }

    #[tokio::test]
    async fn observed_success_emits_complete_prompt_grammar_without_stderr() {
        let rec = Recorder::new("agent-sess-PROMPT-OK-DIAG");
        let backend = connect_recording(rec).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &bkey("bridge-PROMPT-OK-DIAG"),
                vec![],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
            .unwrap();
        assert!(matches!(
            stream.next().await,
            Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn"
        ));
        assert!(stream.next().await.is_none());

        let events = observer.snapshot().await;
        for phase in [
            DiagnosticPhase::SessionCreate,
            DiagnosticPhase::ConfigApply,
            DiagnosticPhase::PromptStart,
            DiagnosticPhase::PromptStream,
            DiagnosticPhase::PromptFinish,
        ] {
            let statuses: Vec<_> = events
                .iter()
                .filter(|event| event.transition().phase() == phase)
                .map(|event| event.transition().status())
                .collect();
            assert_eq!(statuses.len(), 2, "{phase:?} has one start and terminal");
            assert_eq!(statuses[0], PhaseStatus::Started);
            assert!(matches!(
                statuses[1],
                PhaseStatus::Completed | PhaseStatus::Skipped
            ));
            assert!(events
                .iter()
                .filter_map(|event| event.failure())
                .all(|failure| failure.stderr_tail().is_none()));
        }
    }

    #[tokio::test]
    async fn synchronous_prompt_construction_failure_has_started_transition() {
        let rec = Recorder::new("agent-sess-PROMPT-SYNC-FAIL");
        let backend = connect_recording(rec).await;
        let key = bkey("bridge-PROMPT-SYNC-FAIL");
        backend.ensure_session(&key).await.unwrap();
        let registry = Arc::clone(backend.updates().unwrap());
        let _ = std::thread::spawn(move || {
            let _guard = registry.lock().unwrap();
            panic!("poison prompt routing registry for deterministic construction failure");
        })
        .join();

        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let error = match backend
            .prompt_with_observers(
                &key,
                vec![],
                BackendObservers::diagnostic_only(observer.clone()),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("poisoned prompt registry must fail synchronously"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::Unknown,
            false,
        );
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        let prompt_events: Vec<_> = observer
            .snapshot()
            .await
            .into_iter()
            .filter(|event| event.transition().phase() == DiagnosticPhase::PromptStart)
            .collect();
        assert_eq!(prompt_events.len(), 2);
        assert_eq!(prompt_events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(prompt_events[1].transition().status(), PhaseStatus::Failed);
    }

    #[tokio::test]
    async fn synchronous_missing_connection_is_pre_dispatch_and_not_accepted() {
        let backend = AcpBackend {
            conn: None,
            supervised: Arc::new(StdMutex::new(None)),
            stderr_ring: None,
            config: Some(test_config()),
            container_reap: None,
            reaped: Arc::new(AtomicBool::new(false)),
            unavailable: Arc::new(AtomicBool::new(false)),
            dispatch_gate: Arc::new(StdMutex::new(())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cfg: Arc::new(StdMutex::new(HashMap::new())),
            pending_turn_meta: StdMutex::new(HashMap::new()),
            policy: Arc::new(StdMutex::new(
                Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
            )),
            permission_registry: Arc::new(StdMutex::new(None)),
            perm_timeout_ms: Arc::new(AtomicU64::new(120_000)),
            prompt_snapshot_hook: StdMutex::new(None),
            before_process_redactor_hook: StdMutex::new(None),
            fail_deferred_cancel_send: Arc::new(AtomicBool::new(false)),
            fail_cancel_send: Arc::new(AtomicBool::new(false)),
        };
        let key = bkey("bridge-MISSING-CONNECTION");
        backend
            .session_entry(&key)
            .await
            .agent_id
            .set(AgentSessionId::new("agent-sess-MISSING-CONNECTION"))
            .expect("test pre-mints the session without a connection");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        let error = match backend
            .prompt_with_observers(&key, vec![], BackendObservers::diagnostic_only(observer))
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("a missing connection must fail before SDK request installation"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            diagnostic.code().as_str(),
            "acp.prompt_start.connection_unavailable"
        );
    }

    #[tokio::test]
    async fn completed_prompt_does_not_retain_operation_observer() {
        let rec = Recorder::new("agent-sess-PROMPT-WEAK");
        let backend = connect_recording(rec).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let observer_dyn: Arc<dyn DiagnosticObserver> = observer.clone();
        let weak = Arc::downgrade(&observer_dyn);
        let mut stream = backend
            .prompt_with_observers(
                &bkey("bridge-PROMPT-WEAK"),
                vec![],
                BackendObservers::diagnostic_only(observer_dyn),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}
        drop(stream);
        drop(observer);
        tokio::task::yield_now().await;
        assert!(
            weak.upgrade().is_none(),
            "cached backend and completed stream must release the prompt observer"
        );
        drop(backend);
    }

    #[tokio::test]
    async fn cancel_during_slow_completion_observer_does_not_kill_healthy_connection() {
        let rec = Recorder::new("agent-sess-SLOW-COMPLETION");
        let backend = connect_recording_with(
            rec.clone(),
            AcpConfig {
                cancel_grace: Duration::from_millis(50),
                ..test_config()
            },
        )
        .await;
        let key = bkey("bridge-SLOW-COMPLETION");
        backend.ensure_session(&key).await.unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let observer = Arc::new(BlockOnRecord {
            count: AtomicU64::new(0),
            block_at: 8,
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let mut stream = backend
            .prompt_with_observers(&key, vec![], BackendObservers::diagnostic_only(observer))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("prompt must reach its completion transition");

        let agent_id = backend
            .session_entry(&key)
            .await
            .agent_id
            .get()
            .cloned()
            .unwrap();
        assert!(
            backend
                .updates()
                .unwrap()
                .lock()
                .unwrap()
                .get(&agent_id)
                .is_none(),
            "agent work is terminal before completion persistence blocks"
        );
        backend.cancel(&key).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !backend.unavailable.load(Ordering::SeqCst),
            "a held diagnostics lock without a live prompt route must not kill the connection"
        );

        release.notify_one();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("completion observer release must finish the stream"),
            Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn"
        ));
    }

    #[tokio::test]
    async fn cancel_failure_after_route_removal_keeps_operation_acceptance() {
        let rec = Recorder::new("agent-sess-SLOW-COMPLETION-CANCEL-FAIL");
        let backend = connect_recording(rec).await;
        let key = bkey("bridge-SLOW-COMPLETION-CANCEL-FAIL");
        backend.ensure_session(&key).await.unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let observer = Arc::new(BlockOnRecord {
            count: AtomicU64::new(0),
            block_at: 8,
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let mut stream = backend
            .prompt_with_observers(&key, vec![], BackendObservers::diagnostic_only(observer))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("prompt must remove routing before slow completion observation");

        let entry = backend.session_entry(&key).await;
        let agent_id = entry.agent_id.get().cloned().unwrap();
        assert!(
            backend
                .updates()
                .unwrap()
                .lock()
                .unwrap()
                .get(&agent_id)
                .is_none(),
            "the SDK-terminal route must already be removed"
        );
        assert!(
            AcpBackend::turn_prompt_accepted(&entry),
            "operation acceptance must outlive routing through completion observation"
        );

        backend.fail_cancel_send.store(true, Ordering::SeqCst);
        let cancel_observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = backend
            .cancel_observed(&key, cancel_observer)
            .await
            .expect_err("injected cancel delivery failure must be structured");
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            true,
        );
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(backend.unavailable.load(Ordering::SeqCst));

        release.notify_one();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("completion observer release must finish the stream"),
            Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn"
        ));
        tokio::task::yield_now().await;
        assert!(
            AcpBackend::turn_prompt_accepted_handle(&entry).is_none(),
            "turn owner must clear operation acceptance after completion"
        );
    }

    #[tokio::test]
    async fn cancel_route_snapshot_cannot_escalate_after_turn_becomes_terminal() {
        let rec = Recorder::new("agent-sess-CANCEL-ROUTE-RACE");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let backend = connect_recording_with(
            rec.clone(),
            AcpConfig {
                cancel_grace: Duration::from_millis(50),
                ..test_config()
            },
        )
        .await;
        let key = bkey("bridge-CANCEL-ROUTE-RACE");
        backend.ensure_session(&key).await.unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let observer = Arc::new(BlockOnRecord {
            count: AtomicU64::new(0),
            block_at: 8,
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let mut stream = backend
            .prompt_with_observers(&key, vec![], BackendObservers::diagnostic_only(observer))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt route must be active before cancellation");

        // Cancel snapshots the live route and arms its grace watcher. The agent
        // then completes before grace, the driver marks the route terminal, and
        // only completion persistence remains blocked under the turn lock.
        backend.cancel(&key).await.unwrap();
        rec.prompt_gate.notify_one();
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("terminal driver must reach slow completion persistence");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !backend.unavailable.load(Ordering::SeqCst),
            "a cancel watcher that saw the old route must lose to terminal ownership"
        );

        release.notify_one();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("completion persistence release must finish the stream"),
            Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn"
        ));
    }

    #[tokio::test]
    async fn terminal_fence_closing_during_prompt_start_persistence_prevents_dispatch() {
        let rec = Recorder::new("agent-sess-FINAL-DISPATCH-FENCE");
        let backend = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-FINAL-DISPATCH-FENCE");
        backend.ensure_session(&key).await.unwrap();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let observer = Arc::new(BlockOnRecord {
            count: AtomicU64::new(0),
            // Four cached-session transitions precede PromptStart Started.
            block_at: 5,
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let backend_for_prompt = Arc::clone(&backend);
        let key_for_prompt = key.clone();
        let prompt = tokio::spawn(async move {
            backend_for_prompt
                .prompt_with_observers(
                    &key_for_prompt,
                    vec![],
                    BackendObservers::diagnostic_only(observer),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("prompt must pass its early checks and pause before dispatch");

        let container = backend.container_reap.clone();
        AcpBackend::escalate_terminate(
            &backend.supervised,
            &container,
            &backend.reaped,
            &backend.dispatch_gate,
            &backend.unavailable,
        );
        release.notify_one();
        let error = match tokio::time::timeout(Duration::from_secs(2), prompt)
            .await
            .expect("fenced prompt must settle")
            .unwrap()
        {
            Err(error) => error,
            Ok(_) => {
                panic!("a prompt paused before the final gate must not dispatch after fencing")
            }
        };
        assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
        assert!(
            rec.prompt_log.lock().await.is_empty(),
            "the accepted-work barrier must remain uncrossed"
        );
    }

    #[tokio::test]
    async fn observer_failure_after_prompt_install_is_terminal_before_done() {
        let rec = Recorder::new("agent-sess-PROMPT-STORE-FAIL");
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        rec.hang_after_cancel.store(true, Ordering::SeqCst);
        let backend = Arc::new(
            connect_recording_with(
                rec.clone(),
                AcpConfig {
                    cancel_grace: Duration::from_millis(200),
                    ..test_config()
                },
            )
            .await,
        );
        let key = bkey("bridge-PROMPT-STORE-FAIL");
        backend.ensure_session(&key).await.unwrap();

        // Cached-session observation writes four transitions, then prompt_start
        // Started is fifth. Reject the sixth write: PromptStart Completed, after
        // the accepted-work barrier and SDK future installation.
        let observer = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 6,
        });
        let mut stream = backend
            .prompt_with_observers(&key, vec![], BackendObservers::diagnostic_only(observer))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("observer failure must send cancellation before escalation");

        // Queue the second caller BEFORE escalation. Its initial fence check is
        // still false, and its four cached-session transitions prove it reached
        // the turn lock. The post-lock fence must stop dispatch after the first
        // driver escalates and releases that lock.
        let second_observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let backend_for_second = Arc::clone(&backend);
        let key_for_second = key.clone();
        let observer_for_second: Arc<dyn DiagnosticObserver> = second_observer.clone();
        let second = tokio::spawn(async move {
            backend_for_second
                .prompt_with_observers(
                    &key_for_second,
                    vec![],
                    BackendObservers::diagnostic_only(observer_for_second),
                )
                .await
        });
        tokio::time::timeout(Duration::from_millis(100), async {
            while second_observer.snapshot().await.len() < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second prompt must queue behind the accepted turn before escalation");

        match stream.next().await {
            Some(Err(BridgeError::StoreFailure)) => {}
            other => panic!("observer failure must replace terminal success, got {other:?}"),
        }
        assert!(stream.next().await.is_none());

        assert!(
            backend.unavailable.load(Ordering::SeqCst),
            "an ignored cancel after observer failure must fence the shared connection"
        );
        let error = match tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("queued second prompt must settle after the first releases its lock")
            .unwrap()
        {
            Err(error) => error,
            Ok(_) => panic!("a queued caller must recheck the connection fence after the lock"),
        };
        assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
        assert_eq!(
            rec.prompt_log.lock().await.as_slice(),
            &["start"],
            "the rejected second prompt must never enter the agent"
        );
    }

    #[tokio::test]
    async fn configure_turn_stash_is_taken_into_route() {
        let rec = Recorder::new("agent-sess-META");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-META");
        let meta = turn_meta("ctx-meta", 7, "op-meta");

        be.configure_turn(&key, meta.clone()).await;
        let mut stream = be.prompt(&key, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt should start and keep route registered");

        {
            let updates = be.updates().expect("backend has update registry");
            let map = updates.lock().expect("update registry lock");
            let route = map
                .get(&AgentSessionId::new("agent-sess-META"))
                .expect("live prompt route registered");
            let routed = route
                .turn_meta
                .as_ref()
                .expect("turn meta moved into route");
            assert_eq!(routed.context_id.as_str(), "ctx-meta");
            assert_eq!(routed.generation, 7);
            assert_eq!(routed.op.as_str(), "op-meta");
        }

        rec.prompt_gate.notify_one();
        assert!(
            matches!(stream.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
    }

    #[tokio::test]
    async fn configure_turn_take_at_entry_clears_on_missing_session() {
        let rec = Recorder::new("agent-sess-TAKE");
        let be = connect_recording(rec).await;
        let key = bkey("bridge-TAKE");
        let meta = turn_meta("ctx-take", 11, "op-take");

        be.configure_turn(&key, meta.clone()).await;
        {
            let stashed = be
                .pending_turn_meta
                .lock()
                .expect("pending_turn_meta lock")
                .get(&key)
                .cloned()
                .expect("configure_turn stashes meta");
            assert_eq!(stashed.context_id.as_str(), "ctx-take");
            assert_eq!(stashed.generation, 11);
            assert_eq!(stashed.op.as_str(), "op-take");
        }

        let taken = be
            .take_pending_turn_meta(&key)
            .expect("take at prompt entry returns stashed meta");
        assert_eq!(taken.context_id.as_str(), "ctx-take");
        assert_eq!(taken.generation, 11);
        assert_eq!(taken.op.as_str(), "op-take");
        assert!(
            be.take_pending_turn_meta(&key).is_none(),
            "take-at-entry clears stale meta even if the prompt later fails"
        );
    }

    fn select_current(opts: &[SessionConfigOption], id: &str) -> Option<String> {
        opts.iter().find_map(|opt| {
            if &*opt.id.0 != id {
                return None;
            }
            match &opt.kind {
                SessionConfigKind::Select(select) => Some(select.current_value.0.to_string()),
                _ => None,
            }
        })
    }

    #[tokio::test]
    async fn recorder_advertises_and_refreshes_config_options() {
        let rec = Recorder::new("agent-sess-CONFIGOPTS");
        let be = connect_recording(rec).await;
        let cx = be.cx().expect("connection handle").clone();

        let new_resp: NewSessionResponse = cx
            .send_request(AcpBackend::new_session_request("/tmp", &[]))
            .block_task()
            .await
            .expect("session/new succeeds");
        let initial_options = new_resp
            .config_options
            .expect("recorder advertises config options");
        assert_eq!(
            select_current(&initial_options, "model").as_deref(),
            Some("default")
        );
        assert_eq!(
            select_current(&initial_options, "effort").as_deref(),
            Some("medium")
        );

        let set_resp: SetSessionConfigOptionResponse = cx
            .send_request(SetSessionConfigOptionRequest::new(
                new_resp.session_id,
                SessionConfigId::new("effort"),
                SessionConfigValueId::new("high"),
            ))
            .block_task()
            .await
            .expect("set_config_option succeeds");
        assert_eq!(
            select_current(&set_resp.config_options, "effort").as_deref(),
            Some("high"),
            "set_config_option returns refreshed config_options"
        );
    }

    #[tokio::test]
    async fn session_new_minted_lazily_and_mapped() {
        // First `ensure_session(S)` triggers ONE session/new; the agent id is
        // stored and REUSED by subsequent calls (no second session/new).
        let rec = Recorder::new("agent-sess-1");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-A");

        let id1 = be.ensure_session(&key).await.unwrap();
        assert_eq!(id1.0.as_ref(), "agent-sess-1", "agent-minted id is mapped");
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "first ensure_session mints exactly one agent session"
        );

        // Reuse: a second ensure_session returns the SAME id, no new mint.
        let id2 = be.ensure_session(&key).await.unwrap();
        assert_eq!(id2.0.as_ref(), "agent-sess-1");
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "subsequent ensure_session reuses the stored id (no second session/new)"
        );
    }

    #[tokio::test]
    async fn concurrent_first_prompts_mint_one_session() {
        // Two concurrent first `ensure_session(S)` calls must mint the agent
        // session EXACTLY ONCE. Gate session/new so both callers are in flight
        // simultaneously before either reply lands.
        let rec = Recorder::new("agent-sess-X");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let be = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-CONC");

        let b1 = Arc::clone(&be);
        let b2 = Arc::clone(&be);
        let k1 = key.clone();
        let k2 = key.clone();
        let h1 = tokio::spawn(async move { b1.ensure_session(&k1).await });
        let h2 = tokio::spawn(async move { b2.ensure_session(&k2).await });

        // Deterministically wait for the (single) session/new init to be in flight
        // — its handler signals on entry — before unblocking, instead of sleeping.
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("a session/new must reach the agent");
        // Release the held session/new reply (only one is ever in flight).
        rec.new_session_gate.notify_waiters();

        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();
        assert_eq!(r1.0.as_ref(), "agent-sess-X");
        assert_eq!(
            r2.0.as_ref(),
            "agent-sess-X",
            "both share the one minted id"
        );
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "concurrent first-prompts mint session/new EXACTLY ONCE"
        );
    }

    #[tokio::test]
    async fn cancel_racing_session_creation_is_latched() {
        // A cancel issued BEFORE session/new completes must NOT be dropped: once
        // the agent id is minted, the latch fires a session/cancel for it.
        let rec = Recorder::new("agent-sess-LATCH");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let be = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-RACE");

        // Start the mint; it parks on the gate (session/new not yet answered).
        let b1 = Arc::clone(&be);
        let k1 = key.clone();
        let mint = tokio::spawn(async move { b1.ensure_session(&k1).await });
        // Wait deterministically until session/new is in flight (handler entered,
        // parked on the gate) before racing the cancel — no load-sensitive sleep.
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight before the racing cancel");

        // Cancel RACES ahead of creation: only the latch should be set (the id
        // does not exist yet), so no cancel is sent on the wire YET.
        be.request_cancel(&key).await.unwrap();
        assert!(
            rec.cancels.lock().await.is_empty(),
            "cancel before session/new must not be sent against a non-existent id"
        );

        // Now let session/new finish; the minting task must flush the latch.
        rec.new_session_gate.notify_waiters();
        let minted = mint.await.unwrap().unwrap();
        assert_eq!(minted.0.as_ref(), "agent-sess-LATCH");

        // Await the recorded session/cancel deterministically.
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("latched cancel must reach the agent after session/new");
        let cancels = rec.cancels.lock().await;
        assert_eq!(
            cancels.as_slice(),
            &["agent-sess-LATCH"],
            "latched cancel fires exactly once for the freshly-minted id"
        );
    }

    #[tokio::test]
    async fn deferred_cancel_delivery_failure_is_structured_fatal_pre_dispatch() {
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let lifecycle = AcpLifecycle::new(observer.clone(), DiagnosticRedactor::default(), None);
        let error = AcpBackend::deferred_cancel_delivery_failure(
            &lifecycle,
            "connection closed".to_owned(),
        )
        .await;
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            diagnostic.code().as_str(),
            "acp.prompt_start.deferred_cancel_failed"
        );
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(!error.is_transient());
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
    }

    #[tokio::test]
    async fn initializer_call_site_maps_deferred_cancel_send_failure() {
        let rec = Recorder::new("agent-sess-DEFERRED-CANCEL-CALLSITE");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let backend = Arc::new(connect_recording(rec.clone()).await);
        backend
            .fail_deferred_cancel_send
            .store(true, Ordering::SeqCst);
        let session = bkey("bridge-DEFERRED-CANCEL-CALLSITE");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(32).unwrap());
        let backend_for_prompt = Arc::clone(&backend);
        let session_for_prompt = session.clone();
        let observer_for_prompt = observer.clone();
        let prompt = tokio::spawn(async move {
            backend_for_prompt
                .prompt_with_observers(
                    &session_for_prompt,
                    vec![],
                    BackendObservers::diagnostic_only(observer_for_prompt),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight before cancellation");
        backend.cancel(&session).await.unwrap();
        rec.new_session_gate.notify_waiters();

        let error = match prompt.await.unwrap() {
            Err(error) => error,
            Ok(_) => {
                panic!("injected deferred cancel send failure must abort before prompt dispatch")
            }
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            diagnostic.code().as_str(),
            "acp.prompt_start.deferred_cancel_failed"
        );
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(!error.is_transient());
        let prompt_events: Vec<_> = observer
            .snapshot()
            .await
            .into_iter()
            .filter(|event| event.transition().phase() == DiagnosticPhase::PromptStart)
            .collect();
        assert_eq!(prompt_events.len(), 2);
        assert_eq!(prompt_events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(prompt_events[1].transition().status(), PhaseStatus::Failed);
    }

    #[tokio::test]
    async fn observed_cancel_during_mint_is_reported_as_latched() {
        let rec = Recorder::new("agent-sess-OBSERVED-CANCEL-LATCH");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let backend = Arc::new(connect_recording(rec.clone()).await);
        let session = bkey("bridge-OBSERVED-CANCEL-LATCH");
        let backend_for_mint = Arc::clone(&backend);
        let session_for_mint = session.clone();
        let mint =
            tokio::spawn(async move { backend_for_mint.ensure_session(&session_for_mint).await });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight before observed cancellation");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        backend
            .cancel_observed(&session, observer.clone())
            .await
            .unwrap();
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].transition().code().map(|code| code.as_str()),
            Some("acp.teardown.cancel_latched")
        );
        rec.new_session_gate.notify_waiters();
        mint.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn observed_cancel_send_failure_after_acceptance_closes_fence_and_reports_true() {
        let rec = Recorder::new("agent-sess-CANCEL-SEND-FAIL");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let backend = Arc::new(connect_recording(rec.clone()).await);
        let session = bkey("bridge-CANCEL-SEND-FAIL");
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt must cross the accepted-work barrier");
        backend.fail_cancel_send.store(true, Ordering::SeqCst);
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = backend
            .cancel_observed(&session, observer)
            .await
            .expect_err("injected cancel delivery failure must be structured");
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            true,
        );
        assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
        assert!(backend.unavailable.load(Ordering::SeqCst));

        rec.prompt_gate.notify_waiters();
        while stream.next().await.is_some() {}
        let later = match backend.prompt(&session, vec![]).await {
            Err(error) => error,
            Ok(_) => panic!("cancel-send failure must fence later prompt dispatch"),
        };
        assert_agent_failure(
            &later,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
    }

    #[tokio::test]
    async fn cancel_latched_during_mint_fires_exactly_once_no_double_send() {
        // B2 regression: a cancel issued WHILE session/new is in flight must be
        // delivered EXACTLY ONCE once the id is minted — never lost (the bug:
        // in-closure drain ran before OnceCell published the id, so a concurrent
        // request_cancel saw get()==None, didn't send, and the latch was cleared
        // → lost), and never double-sent (drain + request_cancel both fire).
        let rec = Recorder::new("agent-sess-B2");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let be = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-B2");

        // Mint parks on the gate (session/new entered but not yet answered).
        let b1 = Arc::clone(&be);
        let k1 = key.clone();
        let mint = tokio::spawn(async move { b1.ensure_session(&k1).await });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight before the racing cancel");

        // Cancel races ahead of the id becoming observable: it stores the latch
        // and (since the id does not exist yet) sends nothing on the wire.
        be.request_cancel(&key).await.unwrap();
        assert!(
            rec.cancels.lock().await.is_empty(),
            "cancel before the id is observable must not be sent yet"
        );

        // Release session/new; the shielded init task publishes the id and flushes the
        // latched cancel against the freshly-published id.
        rec.new_session_gate.notify_waiters();
        let minted = mint.await.unwrap().unwrap();
        assert_eq!(minted.0.as_ref(), "agent-sess-B2");

        // Exactly one session/cancel must land — proving not-lost.
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("latched cancel must reach the agent after the id is minted");

        // A SECOND cancel on the now-active (reused) session goes straight out via
        // request_cancel (id observable), so we expect exactly two total — proving
        // the first was neither lost nor double-sent (a double would make this 3).
        be.request_cancel(&key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("post-mint cancel reaches the agent");

        let cancels = rec.cancels.lock().await;
        assert_eq!(
            cancels.as_slice(),
            &["agent-sess-B2", "agent-sess-B2"],
            "latched cancel fires exactly once (not lost, not doubled); the later \
             cancel fires once via the observable-id path"
        );
    }

    #[tokio::test]
    async fn second_prompt_on_active_session_serializes() {
        // [Cx-M2] Two turns for the same bridge session must run SEQUENTIALLY.
        // The recording agent holds each prompt open on `prompt_gate`; if the
        // turn lock failed, both turns would "start" before either "end" and the
        // ordering log would interleave (start,start,...). With the lock, the
        // log MUST be start,end,start,end. The streaming `prompt` holds the turn
        // lock in its driver task for the whole turn, so the SECOND `prompt`
        // call blocks acquiring the lock until the first turn completes.
        let rec = Recorder::new("agent-sess-SEQ");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let be = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-SEQ");

        // Pre-mint so neither turn pays the session/new cost inside the lock.
        be.ensure_session(&key).await.unwrap();

        // Turn 1: kick off the prompt and a task that drains its stream to Done.
        let mut s1 = be.prompt(&key, vec![]).await.unwrap();
        let d1 = tokio::spawn(async move { while s1.next().await.is_some() {} });

        // Wait for turn 1 to actually START (its driver holds the lock).
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("turn 1 starts");

        // Subscribe to turn 2's potential start BEFORE spawning it, so a
        // (broken-lock) start can never slip past us between spawn and wait. The
        // `Notified` future is registered as a waiter the moment it's polled.
        let turn2_start = rec.prompt_started.notified();
        tokio::pin!(turn2_start);
        // Poll once to register as a waiter without blocking (Pending expected:
        // turn 2 hasn't started, nor has it even been spawned yet).
        let _ = futures::poll!(turn2_start.as_mut());

        // Turn 2: this `prompt` call blocks acquiring the turn lock (held by
        // turn 1's driver). Drive it from a task so we can observe it WAITING.
        let be2 = Arc::clone(&be);
        let key2 = key.clone();
        let h2 = tokio::spawn(async move {
            let mut s2 = be2.prompt(&key2, vec![]).await.unwrap();
            while s2.next().await.is_some() {}
        });

        // Turn 2 MUST stay blocked on the lock while turn 1 holds it. Deterministic
        // gate (no timing proxy): a BROKEN lock lets turn 2 start, which fires
        // `prompt_started` — so we wait on the pre-registered `turn2_start` notify
        // and REQUIRE it to TIME OUT (turn 2 stayed blocked). With a working lock
        // the wait always elapses; with a broken lock turn 2's start resolves the
        // notify before the bound and the test fails reliably (not a sleep proxy).
        assert!(
            tokio::time::timeout(Duration::from_millis(200), turn2_start.as_mut())
                .await
                .is_err(),
            "second turn must stay blocked on the lock while turn 1 holds it \
             (a broken lock would fire prompt_started before the bound)"
        );
        // And the agent's own log confirms exactly one start outstanding.
        assert_eq!(
            rec.prompt_log.lock().await.as_slice(),
            &["start"],
            "second turn must WAIT for the first (no interleave)"
        );

        // Release turn 1; it ends (driver drops the lock), then turn 2 starts.
        // Reuse the SAME pre-registered `turn2_start` waiter to observe turn 2's
        // actual start (avoids a second registered waiter racing the notify).
        rec.prompt_gate.notify_one();
        tokio::time::timeout(Duration::from_secs(2), turn2_start.as_mut())
            .await
            .expect("turn 2 starts after turn 1 released");
        rec.prompt_gate.notify_one(); // unblock turn 2

        d1.await.unwrap();
        h2.await.unwrap();

        assert_eq!(
            rec.prompt_log.lock().await.as_slice(),
            &["start", "end", "start", "end"],
            "turns run strictly sequentially, never interleaved"
        );
    }

    // ── Task 3: streaming session/prompt + agent_message_chunk fan-in ──────────

    #[tokio::test]
    async fn prompt_streams_text_then_done() {
        // The agent emits two `agent_message_chunk`s then returns end_turn; the
        // stream must yield Update::Text×2 in order, then Update::Done{end_turn}.
        let rec = Recorder::new("agent-sess-STREAM");
        rec.set_updates(vec![
            ScriptedUpdate::Text("hello "),
            ScriptedUpdate::Text("world"),
        ])
        .await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-STREAM");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "hello "));
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "world"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
        assert!(s.next().await.is_none(), "stream terminates after Done");
    }

    #[tokio::test]
    async fn prompt_observed_routes_rich_to_sink() {
        let rec = Recorder::new("agent-sess-RICH");
        rec.set_updates(vec![
            ScriptedUpdate::ToolCall,
            ScriptedUpdate::Text("visible"),
        ])
        .await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RICH");
        let sink = Arc::new(CountingSink::default());
        let dyn_sink: Arc<dyn RichEventSink> = sink.clone();

        let mut stream = be.prompt_observed(&key, vec![], dyn_sink).await.unwrap();
        let mut items = Vec::new();
        while let Some(item) = stream.next().await {
            items.push(item);
        }

        assert_eq!(sink.records(), 1, "tool call routed to rich sink");
        assert_eq!(items.len(), 2, "stream yields only text + terminal done");
        assert!(matches!(&items[0], Ok(Update::Text(t)) if t == "visible"));
        assert!(matches!(&items[1], Ok(Update::Done { stop_reason }) if stop_reason == "end_turn"));
        assert!(
            items
                .iter()
                .all(|item| !matches!(item, Ok(Update::Permission(_)))),
            "rich events must not surface as permission/update stream items"
        );
    }

    #[tokio::test]
    async fn usage_update_reaches_prompt_stream() {
        let rec = Recorder::new("agent-sess-USAGE");
        rec.set_updates(vec![
            ScriptedUpdate::Usage(100, 1000),
            ScriptedUpdate::Text("hi"),
        ])
        .await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-USAGE");

        let mut stream = be.prompt(&key, vec![]).await.unwrap();
        let mut items = Vec::new();
        while let Some(it) = stream.next().await {
            items.push(it);
        }

        let usage_pos = items
            .iter()
            .position(|it| matches!(it, Ok(Update::Usage(s)) if s.used == Some(100)));
        let done_pos = items
            .iter()
            .position(|it| matches!(it, Ok(Update::Done { .. })));
        assert!(
            matches!(usage_pos.zip(done_pos), Some((u, d)) if u < d),
            "usage must traverse TurnEvent -> unfold -> BackendStream before Done"
        );
    }

    #[tokio::test]
    async fn prompt_response_usage_reaches_prompt_stream_before_done() {
        let rec = Recorder::new("agent-sess-END-USAGE");
        rec.set_terminal_usage(321, 300, 21).await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-END-USAGE");

        let mut stream = be.prompt(&key, vec![]).await.unwrap();
        let mut items = Vec::new();
        while let Some(it) = stream.next().await {
            items.push(it);
        }

        let usage_pos = items.iter().position(|it| {
            matches!(
                it,
                Ok(Update::Usage(s))
                    if s.used.is_none()
                        && s.size.is_none()
                        && s.cost.is_none()
                        && s.terminal.as_ref().is_some_and(|usage|
                            usage.total_tokens == 321
                                && usage.input_tokens == 300
                                && usage.output_tokens == 21
                        )
            )
        });
        let done_pos = items
            .iter()
            .position(|it| matches!(it, Ok(Update::Done { .. })));
        assert!(
            matches!(usage_pos.zip(done_pos), Some((u, d)) if u < d),
            "PromptResponse.usage must surface as Update::Usage before Done"
        );
    }

    #[tokio::test]
    async fn prompt_ignores_unmodeled_updates() {
        // Between the two text chunks the agent emits an agent_thought_chunk and
        // a plan (both unmodeled). The tolerant reader drops them: the stream
        // still yields exactly the two texts + Done.
        let rec = Recorder::new("agent-sess-IGN");
        rec.set_updates(vec![
            ScriptedUpdate::Thought("(thinking)"),
            ScriptedUpdate::Text("A"),
            ScriptedUpdate::Plan,
            ScriptedUpdate::Text("B"),
            ScriptedUpdate::Thought("(more thinking)"),
        ])
        .await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-IGN");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "A"));
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "B"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn prompt_maps_stop_reasons() {
        // A non-end_turn StopReason must map correctly onto Update::Done. Check
        // two: max_tokens and cancelled.
        // NOTE: the Cancelled entry uses STOP_REASON_CANCELLED (the shared const)
        // to pin the const's VALUE to the ACP wire spelling. Any change to the
        // const string is caught here; both producer and consumer reference the
        // same const, so drift between them is impossible.
        for (sr, expected) in [
            (StopReason::MaxTokens, "max_tokens"),
            (StopReason::Cancelled, STOP_REASON_CANCELLED),
        ] {
            let rec = Recorder::new("agent-sess-SR");
            rec.set_stop_reason(sr).await;
            let be = connect_recording(rec.clone()).await;
            let key = bkey("bridge-SR");

            let mut s = be.prompt(&key, vec![]).await.unwrap();
            let last = loop {
                match s.next().await {
                    Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                    Some(_) => continue,
                    None => panic!("stream ended without a Done"),
                }
            };
            assert_eq!(last, expected, "StopReason {sr:?} maps to {expected}");
        }
    }

    #[tokio::test]
    async fn prompt_turn_error_surfaces_as_stream_err() {
        // A transport/agent error mid-turn (here: the agent FAILS `session/prompt`
        // with a JSON-RPC error, deterministically gated by `fail_prompt`) must
        // surface as a terminal `Err` on the BackendStream — NOT a silent
        // `Ok(Update::Done{"unknown"})`. The Err is what downstream re-yields so
        // the inbound A2A caller is reported `Failed`, not a clean `Completed`.
        let rec = Recorder::new("agent-sess-FAIL");
        // Stream a chunk first, THEN fail: proves chunks already delivered still
        // flow and the failure is the terminal item (not a swallowed Done).
        rec.set_updates(vec![ScriptedUpdate::Text("partial")]).await;
        rec.fail_prompt.store(true, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-FAIL");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        // The streamed chunk arrives first (ordering preserved).
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "partial"));
        // The turn's terminal item is an Err — NOT a Done.
        match s.next().await {
            Some(Err(error)) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::PromptStream,
                    DiagnosticFailureClass::Unknown,
                    true,
                );
                assert_eq!(diagnostic.code().as_str(), "upstream.unknown");
                assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
            }
            other => panic!(
                "prompt-turn error must surface as terminal Err(AgentCrashed), got {other:?}"
            ),
        }
        // Stream ends after the terminal Err (no trailing Done).
        assert!(
            s.next().await.is_none(),
            "stream terminates after the error item"
        );

        // The agent recorded the turn-start then a fail (not an end) — confirming
        // the prompt reached the agent and the failure path was the one taken.
        assert_eq!(
            rec.prompt_log.lock().await.as_slice(),
            &["start", "fail"],
            "agent saw the prompt start then failed the turn"
        );
    }

    #[tokio::test]
    async fn cancel_sends_session_cancel_for_active_session() {
        // Minimal SDK-fake-agent cancel test (Task 4 owns completion semantics):
        // a cancel on an active session sends `session/cancel` for its agent id.
        let rec = Recorder::new("agent-sess-CAN");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-CAN");

        // Make the session active (minted) so cancel goes straight out.
        be.ensure_session(&key).await.unwrap();
        be.cancel(&key).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("cancel must reach the agent");
        assert_eq!(rec.cancels.lock().await.as_slice(), &["agent-sess-CAN"]);
    }

    // ── Task 4: cancel completion = the prompt RESULT ──────────────────────────

    #[tokio::test]
    async fn cancel_completion_is_the_prompt_result() {
        // Spec §5.3: cancellation COMPLETION is the prompt RESULT (StopReason::
        // Cancelled → Update::Done{"cancelled"}), NOT the act of sending
        // `session/cancel`. The fake agent emits one chunk, then WAITS for
        // `session/cancel` before returning `StopReason::Cancelled`. We read the
        // first Text, issue `cancel(S)`, and assert: (a) the agent recorded the
        // `session/cancel`, and (b) the stream completes with Done{"cancelled"} —
        // which can only arrive AFTER the cancelled RESULT, since the agent blocks
        // until it sees the cancel. Deterministic gates, no sleeps.
        let rec = Recorder::new("agent-sess-CCR");
        rec.set_updates(vec![ScriptedUpdate::Text("chunk-1")]).await;
        rec.set_stop_reason(StopReason::Cancelled).await;
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-CCR");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        // First the streamed chunk arrives (the turn is in flight, NOT yet done).
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "chunk-1"));

        // Now cancel. The agent is blocked waiting for exactly this notification.
        be.cancel(&key).await.unwrap();

        // The agent must record the session/cancel for the in-flight turn's id.
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("cancel must reach the agent");
        assert_eq!(rec.cancels.lock().await.as_slice(), &["agent-sess-CCR"]);

        // The stream completes via the agent's Cancelled RESULT → Done{"cancelled"}.
        // (It could NOT have completed before the cancel: the agent blocked on it.)
        match tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("stream must complete after the cancelled result")
        {
            Some(Ok(Update::Done { stop_reason })) => {
                assert_eq!(
                    stop_reason, "cancelled",
                    "completion is the Cancelled RESULT"
                );
            }
            other => panic!("expected Done{{\"cancelled\"}}, got {other:?}"),
        }
        assert!(s.next().await.is_none(), "stream terminates after Done");
    }

    #[tokio::test]
    async fn cancel_racing_creation_still_cancels() {
        // A cancel issued BEFORE `session/new` completes must not be dropped: after
        // the id is minted, EXACTLY ONE `session/cancel` reaches the agent, and the
        // subsequent turn completes CANCELLED (completion = the Cancelled result).
        let rec = Recorder::new("agent-sess-RC");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        // The turn, once it runs, blocks for the cancel then returns Cancelled —
        // so the latched cancel both lands AND drives the turn to completion.
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        rec.set_stop_reason(StopReason::Cancelled).await;
        let be = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-RC");

        // Start the prompt; its `ensure_session` parks on the gated `session/new`.
        let b1 = Arc::clone(&be);
        let k1 = key.clone();
        let prompt = tokio::spawn(async move {
            let mut s = b1.prompt(&k1, vec![]).await.unwrap();
            let mut last = None;
            while let Some(item) = s.next().await {
                last = Some(item);
            }
            last
        });
        // Wait until `session/new` is in flight (handler entered, parked on gate).
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be in flight before the racing cancel");

        // Cancel RACES ahead of creation: only the latch is set (id not yet minted),
        // so nothing is on the wire yet.
        be.cancel(&key).await.unwrap();
        assert!(
            rec.cancels.lock().await.is_empty(),
            "cancel before session/new must not be sent against a non-existent id"
        );

        // Release session/new; the minting task flushes the latched cancel, which
        // also unblocks the (cancel-waiting) turn → it returns Cancelled.
        rec.new_session_gate.notify_waiters();

        // Exactly one session/cancel reaches the agent for the freshly-minted id.
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("latched cancel must reach the agent after session/new");
        assert_eq!(rec.cancels.lock().await.as_slice(), &["agent-sess-RC"]);

        // And the turn completes CANCELLED (the result), not via the notification.
        let last = tokio::time::timeout(Duration::from_secs(2), prompt)
            .await
            .expect("the racing-cancel turn must complete, not hang")
            .unwrap();
        match last {
            Some(Ok(Update::Done { stop_reason })) => {
                assert_eq!(
                    stop_reason, "cancelled",
                    "racing cancel completes the turn cancelled"
                );
            }
            other => panic!("expected terminal Done{{\"cancelled\"}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelling_initializer_waiter_does_not_orphan_or_remint_session() {
        let rec = Recorder::new("agent-sess-INIT-SHIELDED");
        rec.gate_new_session.store(true, Ordering::SeqCst);
        let backend = Arc::new(connect_recording(rec.clone()).await);
        let key = bkey("bridge-INIT-SHIELDED");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let observer_dyn: Arc<dyn DiagnosticObserver> = observer.clone();
        let observer_weak = Arc::downgrade(&observer_dyn);

        let backend_for_first = Arc::clone(&backend);
        let key_for_first = key.clone();
        let observer_for_first = observer_dyn;
        let first = tokio::spawn(async move {
            backend_for_first
                .prompt_with_observers(
                    &key_for_first,
                    vec![],
                    BackendObservers::diagnostic_only(observer_for_first),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), rec.new_session_started.notified())
            .await
            .expect("session/new must be dispatched before caller cancellation");

        // Dropping the original waiter used to cancel OnceCell's initializer,
        // leaving SessionCreate Started open and causing the next caller to mint
        // a second agent session.
        first.abort();
        let _ = first.await;
        rec.new_session_gate.notify_waiters();

        let entry = backend.session_entry(&key).await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while entry.agent_id.get().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("shielded initializer must publish the minted session");

        let events = observer.snapshot().await;
        for phase in [DiagnosticPhase::SessionCreate, DiagnosticPhase::ConfigApply] {
            let terminal = events
                .iter()
                .rfind(|event| event.transition().phase() == phase)
                .expect("started initialization phase must have a terminal event");
            assert_ne!(terminal.transition().status(), PhaseStatus::Started);
        }
        drop(observer);
        tokio::task::yield_now().await;
        assert!(
            observer_weak.upgrade().is_none(),
            "completed shielded initialization must release its operation observer"
        );

        let second_observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let mut stream = backend
            .prompt_with_observers(
                &key,
                vec![],
                BackendObservers::diagnostic_only(second_observer),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "a cancelled waiter must not cause a second session/new"
        );
    }

    #[tokio::test]
    async fn cancel_hung_agent_is_terminated_within_grace() {
        // A hung agent that receives `session/cancel` but NEVER returns must not
        // hang the caller forever. With a SHORT cancel grace, the backend escalates
        // (fires the per-turn kill switch — and would `terminate()` a real child)
        // so the turn ends with a terminal Err WITHIN the grace bound. An outer
        // `timeout` makes a regression (no escalation) fail fast instead of hanging.
        let rec = Recorder::new("agent-sess-HUNG");
        rec.set_updates(vec![ScriptedUpdate::Text("partial")]).await;
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        rec.hang_after_cancel.store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            cancel_grace: Duration::from_millis(150),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-HUNG");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        // The streamed chunk arrives; the turn is in flight.
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "partial"));

        // Cancel; the agent records it but then hangs forever (never responds).
        be.cancel(&key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("cancel must reach the (hung) agent");

        // The turn MUST be terminated within the grace bound: the stream ends with
        // a terminal Err (the kill-switch escalation), not a hang. Bound it well
        // above the 150ms grace but far below a "hung" wall so a regression fails.
        match tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("hung turn must be terminated within grace, not hang")
        {
            Some(Err(error)) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::PromptStream,
                    DiagnosticFailureClass::AgentProcess,
                    true,
                );
            }
            other => panic!("hung-agent escalation must end the turn with Err, got {other:?}"),
        }
        assert!(
            s.next().await.is_none(),
            "stream terminates after the escalation Err"
        );

        // Escalation kills the shared process connection. The lock is released,
        // but a later prompt must be fenced rather than overlap the still-running
        // in-process fake (or target a dead real process).
        let error = match tokio::time::timeout(Duration::from_secs(2), be.prompt(&key, vec![]))
            .await
            .expect("a fenced prompt must return promptly")
        {
            Err(error) => error,
            Ok(_) => panic!("a prompt after shared-connection escalation must be rejected"),
        };
        assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
        assert_eq!(rec.prompt_log.lock().await.as_slice(), &["start"]);
    }

    #[tokio::test]
    async fn dropping_stream_cancels_agent_turn() {
        // If the CONSUMER drops the returned BackendStream mid-turn (client
        // disconnect), the agent turn must be CANCELLED (not left running holding
        // the turn lock). The agent gates its prompt open; we drop the stream,
        // assert the agent records a `session/cancel`, then let it respond and
        // assert the turn lock RELEASED (a subsequent prompt on S proceeds).
        let rec = Recorder::new("agent-sess-DROP");
        rec.set_updates(vec![ScriptedUpdate::Text("streaming")])
            .await;
        // Hold the turn open so it is unambiguously in flight when we drop.
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-DROP");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        // The turn is in flight (chunk delivered; prompt handler parked on gate).
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "streaming"));

        // Consumer disconnects: drop the stream. The driver's `done_sender.closed()`
        // branch must fire and send `session/cancel` for this turn's agent id.
        drop(s);
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("dropping the stream must cancel the agent turn");
        assert_eq!(rec.cancels.lock().await.as_slice(), &["agent-sess-DROP"]);

        // Let the (now-cancelled) turn finish so the driver releases the lock.
        rec.prompt_gate.notify_one();
        rec.gate_prompt.store(false, Ordering::SeqCst);

        // The turn lock must have released: a subsequent prompt on S proceeds and
        // completes (it would block forever if the dropped turn still held it).
        let mut s2 = tokio::time::timeout(Duration::from_secs(2), be.prompt(&key, vec![]))
            .await
            .expect("a fresh prompt must acquire the released turn lock")
            .unwrap();
        let done = loop {
            match tokio::time::timeout(Duration::from_secs(2), s2.next())
                .await
                .expect("fresh turn must complete")
            {
                Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                Some(_) => continue,
                None => panic!("fresh turn ended without Done"),
            }
        };
        assert_eq!(
            done, "end_turn",
            "the dropped turn released the lock → next turn runs"
        );
    }

    // ── Slice 7b: E9 watchdog driver terminal ────────────────────────────────

    #[tokio::test]
    async fn watchdog_cancels_a_hung_turn_as_timed_out() {
        let rec = Recorder::new("agent-sess-WD-HUNG");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            watchdog: Some(bridge_core::domain::WatchdogConfig {
                idle_timeout: Duration::from_secs(10),
                hard_wall_clock: Duration::from_millis(50),
            }),
            cancel_grace: Duration::from_millis(25),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-WD-HUNG");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt must reach the fake agent");

        match tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("watchdog must terminate the hung turn")
        {
            Some(Err(error)) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::PromptStream,
                    DiagnosticFailureClass::Timeout,
                    true,
                );
                assert_eq!(diagnostic.code().as_str(), "acp.prompt.watchdog_timeout");
                assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
            }
            other => panic!("watchdog terminal must be a structured timeout, got {other:?}"),
        }
        assert!(s.next().await.is_none(), "stream terminates after timeout");

        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("watchdog must first send session/cancel");
        assert_eq!(rec.cancels.lock().await.as_slice(), &["agent-sess-WD-HUNG"]);
    }

    #[tokio::test]
    async fn watchdog_does_not_trip_active_or_unmodeled_turn() {
        for (agent_id, updates) in [
            (
                "agent-sess-WD-ACTIVE",
                vec![
                    ScriptedUpdate::Text("a"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                    ScriptedUpdate::Text("b"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                    ScriptedUpdate::Text("c"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                ],
            ),
            (
                "agent-sess-WD-UNMODELED",
                vec![
                    ScriptedUpdate::Thought("a"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                    ScriptedUpdate::Thought("b"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                    ScriptedUpdate::Thought("c"),
                    ScriptedUpdate::Delay(Duration::from_millis(20)),
                ],
            ),
        ] {
            let rec = Recorder::new(agent_id);
            rec.set_updates(updates).await;
            let cfg = AcpConfig {
                watchdog: Some(bridge_core::domain::WatchdogConfig {
                    idle_timeout: Duration::from_millis(100),
                    hard_wall_clock: Duration::from_secs(10),
                }),
                ..test_config()
            };
            let be = connect_recording_with(rec.clone(), cfg).await;
            let key = bkey("bridge-WD-ACTIVE");

            let mut s = be.prompt(&key, vec![]).await.unwrap();
            let terminal = loop {
                match tokio::time::timeout(Duration::from_secs(2), s.next())
                    .await
                    .expect("active turn must complete without watchdog timeout")
                {
                    Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                    Some(Ok(_)) => continue,
                    Some(Err(err)) => panic!("active turn must not fail: {err:?}"),
                    None => panic!("stream ended without Done"),
                }
            };
            assert_eq!(terminal, "end_turn");
            assert!(
                rec.cancels.lock().await.is_empty(),
                "watchdog must not cancel an active turn"
            );
        }
    }

    #[tokio::test]
    async fn no_watchdog_config_behaves_identically() {
        // With no `[agents.watchdog]`: no watchdog task is spawned, the driver's
        // watchdog select arm is a never-resolving `pending()`, and the handler's
        // activity bump is a no-op (`watch=None`). NB the outer select gained
        // `biased;` (a whole-branch review fix), so arm *arbitration* changed even
        // on the disabled path — but only benignly: a ready `prompt_fut` is now
        // preferred deterministically over the (here-`pending`) watchdog arm, which
        // is strictly-no-worse than the prior random choice. Behaviour is therefore
        // identical, not literally byte-identical scheduling.
        let rec = Recorder::new("agent-sess-NO-WD");
        rec.gate_prompt.store(true, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-NO-WD");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt must reach the fake agent");

        assert!(
            tokio::time::timeout(Duration::from_millis(150), s.next())
                .await
                .is_err(),
            "with watchdog disabled, a silent in-flight turn must not time out locally"
        );
        assert!(
            rec.cancels.lock().await.is_empty(),
            "with watchdog disabled, no local watchdog cancel is sent"
        );

        rec.gate_prompt.store(false, Ordering::SeqCst);
        rec.prompt_gate.notify_one();
        match tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("released no-watchdog turn should complete")
        {
            Some(Ok(Update::Done { stop_reason })) => assert_eq!(stop_reason, "end_turn"),
            other => panic!("expected natural Done after releasing the fake agent, got {other:?}"),
        }
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn watchdog_timeout_overrides_an_honored_cancel_within_grace() {
        // PFIX-F sharp edge: when the watchdog fires, it sends `session/cancel`,
        // then awaits the prompt within `cancel_grace`. If the agent HONORS that
        // cancel and returns a clean `StopReason::Cancelled` result WITHIN grace,
        // the driver must STILL surface `AgentTimedOut` — the watchdog arm DISCARDS
        // the inner prompt outcome (`timed_out_local` already set). A regression
        // that forwarded the agent's `Done{cancelled}` would mis-report the turn as
        // a clean user cancel rather than a forced timeout. Grace is generous (2s)
        // so the agent's response arrives well inside the inner select → the
        // `&mut prompt_fut` arm wins (not the grace-sleep escalation), exercising
        // exactly the honored-within-grace path.
        let rec = Recorder::new("agent-sess-WD-HONOR");
        // Agent waits for the cancel, then responds with a real Cancelled result.
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        rec.set_stop_reason(StopReason::Cancelled).await;
        let cfg = AcpConfig {
            watchdog: Some(bridge_core::domain::WatchdogConfig {
                idle_timeout: Duration::from_secs(10),
                hard_wall_clock: Duration::from_millis(50),
            }),
            cancel_grace: Duration::from_secs(2),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-WD-HONOR");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.prompt_started.notified())
            .await
            .expect("prompt must reach the fake agent");

        // The watchdog fires (wall-clock 50ms), sends session/cancel; the agent
        // honors it and returns Cancelled within grace — yet the terminal is
        // a structured timeout, NOT a Done{cancelled}.
        match tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("watchdog must terminate the turn even when the agent honors cancel")
        {
            Some(Err(error)) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::PromptStream,
                    DiagnosticFailureClass::Timeout,
                    true,
                );
                assert_eq!(diagnostic.code().as_str(), "acp.prompt.watchdog_timeout");
            }
            other => {
                panic!("an honored-within-grace cancel must STILL be a timeout, got {other:?}")
            }
        }
        assert!(s.next().await.is_none(), "stream terminates after timeout");

        // The agent did observe the watchdog's session/cancel (proves the honored
        // path, not a process kill).
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("watchdog must have sent session/cancel for the agent to honor");
        assert_eq!(
            rec.cancels.lock().await.as_slice(),
            &["agent-sess-WD-HONOR"]
        );
    }

    #[tokio::test]
    async fn watchdog_preserves_sdk_failure_returned_during_cancel_grace() {
        let rec = Recorder::new("agent-sess-WD-CAUSE");
        rec.wait_cancel_before_respond.store(true, Ordering::SeqCst);
        rec.fail_prompt.store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            watchdog: Some(bridge_core::domain::WatchdogConfig {
                idle_timeout: Duration::from_secs(10),
                hard_wall_clock: Duration::from_millis(50),
            }),
            cancel_grace: Duration::from_secs(2),
            ..test_config()
        };
        let backend = connect_recording_with(rec.clone(), cfg).await;
        let mut stream = backend
            .prompt(&bkey("bridge-WD-CAUSE"), vec![])
            .await
            .unwrap();

        let error = match tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("watchdog must settle after the agent returns its SDK error")
        {
            Some(Err(error)) => error,
            other => panic!("watchdog must remain the typed terminal, got {other:?}"),
        };
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStream,
            DiagnosticFailureClass::Timeout,
            true,
        );
        assert!(
            diagnostic
                .causes()
                .iter()
                .any(|cause| cause.contains("agent failed the turn")),
            "the deeper SDK cause returned during grace must not be discarded: {:?}",
            diagnostic.causes()
        );
    }

    // ── Task 5: reverse session/request_permission handler ─────────────────────

    #[tokio::test]
    async fn permission_auto_approved_selects_allow_once() {
        // The agent issues `session/request_permission` mid-turn with options
        // [{a, allow_once}, {r, reject_once}]. The default auto-approve policy
        // must make the backend reply Selected{optionId:"a"} (the allow_once).
        let rec = Recorder::new("agent-sess-PERM");
        rec.arm_permission(allow_reject_options()).await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-PERM");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        // Drain to Done; the permission round-trip happens during the turn.
        let done = loop {
            match tokio::time::timeout(Duration::from_secs(2), s.next())
                .await
                .expect("turn must complete (permission auto-approved)")
            {
                Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                Some(_) => continue,
                None => panic!("stream ended without Done"),
            }
        };
        assert_eq!(done, "end_turn");

        // The agent recorded the client's reply: Selected the allow_once id "a".
        tokio::time::timeout(Duration::from_secs(2), rec.permission_replied.notified())
            .await
            .expect("client must have replied to the permission request");
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(Some("a".to_string())),
            "auto-approve selects the allow_once option"
        );
    }

    #[tokio::test]
    async fn dead_safe_auto_policy_full_turn_no_deferral() {
        // DoD regression: even when a PermissionRegistry is attached and warm
        // turn metadata is available, the DEFAULT auto policy must stay on the
        // pre-slice immediate-decide path: no pending entry, no parking, and the
        // agent receives the same AllowOnce selection as before.
        let rec = Recorder::new("agent-sess-DSAFE");
        rec.arm_permission(allow_reject_options()).await;
        rec.gate_turn_on_permission.store(true, Ordering::SeqCst);
        rec.set_updates(vec![ScriptedUpdate::Text("after-perm")])
            .await;

        let registry = PermissionRegistry::new();
        let be = connect_recording(rec.clone())
            .await
            .with_permission_registry(Arc::clone(&registry));
        let key = bkey("bridge-DSAFE");
        let meta = turn_meta("ctx-dead-safe", 42, "op-dead-safe");
        let ctx = meta.context_id.clone();
        be.configure_turn(&key, meta).await;

        tokio::time::timeout(Duration::from_secs(5), async {
            let mut s = be.prompt(&key, vec![]).await.unwrap();
            assert!(
                matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "after-perm"),
                "the gated post-permission chunk must arrive"
            );
            let done = loop {
                match s.next().await {
                    Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                    Some(_) => continue,
                    None => panic!("stream ended without Done"),
                }
            };
            assert_eq!(done, "end_turn");
        })
        .await
        .expect("default auto policy turn must complete without permission parking");

        tokio::time::timeout(Duration::from_secs(2), rec.permission_replied.notified())
            .await
            .expect("client must have replied to the permission request");
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(Some("a".to_string())),
            "auto policy must select the same AllowOnce option as the pre-slice path"
        );
        assert!(
            registry.pending(&ctx).is_empty(),
            "auto policy must not create a pending permission entry"
        );
    }

    #[tokio::test]
    async fn permission_deny_selects_reject_or_cancelled() {
        // With a DENY policy injected via `with_policy`, the backend must reply
        // Selected{optionId:"r"} (the reject_once option) — not the allow.
        let rec = Recorder::new("agent-sess-DENY");
        rec.arm_permission(allow_reject_options()).await;
        let be = connect_recording(rec.clone())
            .await
            .with_policy(Arc::new(DenyPolicy));
        let key = bkey("bridge-DENY");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        let done = loop {
            match tokio::time::timeout(Duration::from_secs(2), s.next())
                .await
                .expect("turn must complete (permission denied)")
            {
                Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                Some(_) => continue,
                None => panic!("stream ended without Done"),
            }
        };
        assert_eq!(done, "end_turn");

        tokio::time::timeout(Duration::from_secs(2), rec.permission_replied.notified())
            .await
            .expect("client must have replied to the permission request");
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(Some("r".to_string())),
            "deny selects the reject_once option"
        );
    }

    #[tokio::test]
    async fn permission_deny_with_no_reject_option_is_cancelled() {
        // Deny policy, but the agent offers ONLY an allow_once option (no reject).
        // The conformant reply is then `Cancelled` (no reject to select, and we
        // must not approve under a deny).
        let rec = Recorder::new("agent-sess-DENYC");
        rec.arm_permission(vec![PermissionOption::new(
            PermissionOptionId::new("a"),
            "Allow once",
            PermissionOptionKind::AllowOnce,
        )])
        .await;
        let be = connect_recording(rec.clone())
            .await
            .with_policy(Arc::new(DenyPolicy));
        let key = bkey("bridge-DENYC");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        while tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("turn must complete")
            .is_some()
        {}

        tokio::time::timeout(Duration::from_secs(2), rec.permission_replied.notified())
            .await
            .expect("client must have replied");
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(None),
            "deny with no reject option → Cancelled"
        );
    }

    #[tokio::test]
    async fn permission_approve_with_no_allow_option_is_cancelled() {
        // Bug #5 regression: Approve policy, but the agent offers ONLY a reject
        // option (no AllowOnce / AllowAlways). The old code fell back to
        // `req.options.first()` — selecting the RejectAlways — permanently
        // blacklisting the tool call under an approve policy (correctness
        // inversion). The fix: when no allow option exists, the backend must
        // reply `Cancelled` (does not grant, but does NOT permanently deny).
        let rec = Recorder::new("agent-sess-APPC");
        rec.arm_permission(vec![PermissionOption::new(
            PermissionOptionId::new("r"),
            "Reject always",
            PermissionOptionKind::RejectAlways,
        )])
        .await;
        // Default policy is auto-approve; no `with_policy` override needed.
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-APPC");

        let mut s = be.prompt(&key, vec![]).await.unwrap();
        while tokio::time::timeout(Duration::from_secs(2), s.next())
            .await
            .expect("turn must complete")
            .is_some()
        {}

        tokio::time::timeout(Duration::from_secs(2), rec.permission_replied.notified())
            .await
            .expect("client must have replied");
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(None),
            "approve with no allow option → Cancelled (never a reject option)"
        );
    }

    #[tokio::test]
    async fn fs_read_request_is_unsupported() {
        // The backend registers NO `fs/read_text_file` handler and advertises no
        // fs caps, so the SDK's default dispatch auto-responds with a
        // method-not-found error to any inbound `fs/read_text_file`. We prove this
        // by having a fake AGENT issue `fs/read_text_file` BACK to the backend's
        // client and recording the reply: it must be `Err` (not a panic / hang),
        // and the connection must remain usable afterwards (a prompt completes).
        use agent_client_protocol::schema::v1::{ReadTextFileRequest, ReadTextFileResponse};

        // Agent records the outcome of its fs/read probe. Spawned BEFORE the client
        // connects so the initialize handshake succeeds.
        let err_flag = Arc::new(AtomicBool::new(false));
        let ok_flag = Arc::new(AtomicBool::new(false));
        let probe_done = Arc::new(Notify::new());
        // A second prompt round-trip after the probe proves the connection survives.
        let prompt_done = Arc::new(Notify::new());

        let (client_side, agent_side) = Channel::duplex();
        let err_f = Arc::clone(&err_flag);
        let ok_f = Arc::clone(&ok_flag);
        let probe_f = Arc::clone(&probe_done);
        let prompt_f = Arc::clone(&prompt_done);
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("fs-probe-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| async move {
                        responder.respond(InitializeResponse::new(ProtocolVersion::V1))?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |_req: NewSessionRequest,
                          responder: agent_client_protocol::Responder<NewSessionResponse>,
                          _cx| async move {
                        responder
                            .respond(NewSessionResponse::new(AgentSessionId::new("fs-sess")))?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                // The prompt handler probes fs/read_text_file BACK at the client
                // mid-turn (offloaded), records the Err it gets, THEN ends the turn.
                // This mirrors the proven reverse-request mechanism and proves the
                // connection stays usable (the same turn completes).
                .on_receive_request(
                    move |_req: PromptRequest,
                          responder: agent_client_protocol::Responder<PromptResponse>,
                          cx: ConnectionTo<Client>| {
                        let err_f = Arc::clone(&err_f);
                        let ok_f = Arc::clone(&ok_f);
                        let probe_f = Arc::clone(&probe_f);
                        let prompt_f = Arc::clone(&prompt_f);
                        async move {
                            let cx2 = cx.clone();
                            cx.spawn(async move {
                                let req = ReadTextFileRequest::new(
                                    AgentSessionId::new("fs-sess"),
                                    "/etc/hosts",
                                );
                                let res: Result<ReadTextFileResponse, _> =
                                    cx2.send_request(req).block_task().await;
                                match res {
                                    Ok(_) => ok_f.store(true, Ordering::SeqCst),
                                    Err(_) => err_f.store(true, Ordering::SeqCst),
                                }
                                probe_f.notify_one();
                                // Connection still usable: end the turn normally.
                                responder.respond(PromptResponse::new(StopReason::EndTurn))?;
                                prompt_f.notify_one();
                                Ok(())
                            })?;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(agent_side)
                .await;
        });

        let be = AcpBackend::connect(client_side, test_config())
            .await
            .expect("client initialize");

        // Drive a prompt; the agent's prompt handler issues the fs/read probe.
        let key = bkey("bridge-FS");
        let mut s = be.prompt(&key, vec![]).await.unwrap();

        // The agent's fs/read_text_file probe must return Err (unsupported), promptly.
        tokio::time::timeout(Duration::from_secs(3), probe_done.notified())
            .await
            .expect("fs/read probe must complete (not hang)");
        assert!(
            err_flag.load(Ordering::SeqCst),
            "fs/read_text_file must be unsupported (method-not-found Err)"
        );
        assert!(
            !ok_flag.load(Ordering::SeqCst),
            "fs/read_text_file must NOT succeed (no fs caps advertised)"
        );

        // The connection is still usable: the same turn completes (Done).
        let completed = loop {
            match tokio::time::timeout(Duration::from_secs(2), s.next())
                .await
                .expect("connection still usable after unsupported request")
            {
                Some(Ok(Update::Done { .. })) => break true,
                Some(_) => continue,
                None => break false,
            }
        };
        assert!(
            completed,
            "the connection keeps working after an unsupported request"
        );
        // The agent saw the post-probe prompt too.
        tokio::time::timeout(Duration::from_secs(2), prompt_done.notified())
            .await
            .expect("post-probe prompt must reach the agent");
    }

    #[tokio::test]
    async fn permission_mid_prompt_does_not_stall_loop() {
        // [Cx-M4] THE KEY TEST. The agent, mid-turn, issues a
        // `session/request_permission` request and WAITS for the client's reply
        // BEFORE emitting its remaining `agent_message_chunk`(s) + the
        // `PromptResponse` (`gate_turn_on_permission`). The backend's permission
        // handler runs ON the client's dispatch loop; if it BLOCKED the loop while
        // deciding/responding, the reply could never be sent → the gated chunks
        // and the prompt result could never arrive → this test would HANG. The
        // outer `timeout` makes such a regression FAIL FAST.
        let rec = Recorder::new("agent-sess-MID");
        rec.arm_permission(allow_reject_options()).await;
        rec.gate_turn_on_permission.store(true, Ordering::SeqCst);
        rec.set_updates(vec![ScriptedUpdate::Text("after-perm")])
            .await;
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-MID");

        let driven = tokio::time::timeout(Duration::from_secs(5), async {
            let mut s = be.prompt(&key, vec![]).await.unwrap();
            // The chunk is emitted ONLY after the permission reply was received by
            // the agent — so receiving it proves the reply round-tripped, i.e. the
            // client's permission handler did NOT stall the loop.
            assert!(
                matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "after-perm"),
                "the post-permission chunk must arrive (loop not stalled)"
            );
            let done = loop {
                match s.next().await {
                    Some(Ok(Update::Done { stop_reason })) => break stop_reason,
                    Some(_) => continue,
                    None => panic!("stream ended without Done"),
                }
            };
            assert_eq!(done, "end_turn");
        })
        .await;
        driven.expect("mid-prompt permission must not stall the loop (stream completes)");

        // And the backend's auto-approve selected the allow_once "a".
        assert_eq!(
            *rec.permission_reply.lock().await,
            Some(Some("a".to_string())),
            "auto-approve selected allow_once mid-prompt"
        );
    }

    // ── Task 6: set_mode / model config / authenticate ─────────────────────────

    /// `test_config` with a configured `mode`.
    fn test_config_with_mode(mode: &str) -> AcpConfig {
        AcpConfig {
            mode: Some(mode.to_string()),
            ..test_config()
        }
    }

    #[tokio::test]
    async fn set_mode_applied_after_session_new() {
        // With a configured mode, minting a session sends ONE `session/set_mode`
        // carrying that mode id; a second `ensure_session` (same key) reuses the
        // session and does NOT re-apply it (once per session, tied to the mint).
        let rec = Recorder::new("agent-sess-MODE");
        let be = connect_recording_with(rec.clone(), test_config_with_mode("yolo")).await;
        let key = bkey("bridge-MODE");

        be.ensure_session(&key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.set_mode_seen.notified())
            .await
            .expect("set_mode must reach the agent after session/new");
        assert_eq!(
            rec.set_modes.lock().await.as_slice(),
            &["yolo"],
            "set_mode applied once with the configured mode id"
        );

        // Reuse: a second ensure_session must NOT re-apply set_mode.
        be.ensure_session(&key).await.unwrap();
        assert_eq!(
            rec.set_modes.lock().await.as_slice(),
            &["yolo"],
            "set_mode is applied exactly once per session (not per prompt/ensure)"
        );
    }

    #[tokio::test]
    async fn set_mode_bad_id_is_hard_error_and_leaves_cell_uninitialized() {
        // The agent REJECTS the configured mode id. Because set_mode is configured
        // INSIDE the shielded init task (before the id is published), a hard
        // `?`-error makes initialization FAIL and the `OnceCell` stays
        // UNINITIALIZED. So:
        //   (a) `ensure_session` returns the hard error, AND
        //   (b) a SECOND `ensure_session` re-runs the FULL mint+set_mode (re-mints,
        //       re-attempts set_mode) and also errors — it does NOT silently
        //       proceed on a committed-but-unconfigured session in the WRONG mode.
        let rec = Recorder::new("agent-sess-BADMODE");
        rec.reject_set_mode.store(true, Ordering::SeqCst);
        let be = connect_recording_with(rec.clone(), test_config_with_mode("nonexistent")).await;
        let key = bkey("bridge-BADMODE");

        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                assert_eq!(diagnostic.code().as_str(), "acp.config.mode_rejected");
            }
            other => panic!("a rejected set_mode must fail session setup, got {other:?}"),
        }
        // The agent recorded the (rejected) mode request from the first attempt.
        assert_eq!(rec.set_modes.lock().await.as_slice(), &["nonexistent"]);
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "first ensure_session minted exactly once"
        );

        // SECOND attempt: cell is uninitialized → the closure re-runs → re-mints
        // and re-attempts set_mode, then errors again. NOT a silent success.
        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                assert_eq!(diagnostic.code().as_str(), "acp.config.mode_rejected");
            }
            other => panic!(
                "a re-attempt after a set_mode failure must re-run and error, \
                 not silently proceed unconfigured, got {other:?}"
            ),
        }
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            2,
            "the uninitialized cell makes the SECOND ensure_session re-mint (no masking)"
        );
        assert_eq!(
            rec.set_modes.lock().await.as_slice(),
            &["nonexistent", "nonexistent"],
            "set_mode is re-attempted on retry (committed-but-unconfigured session is impossible)"
        );
    }

    #[tokio::test]
    async fn non_advertised_model_fails_mint() {
        let rec = Recorder::new("agent-sess-MODELERR");
        let cfg = AcpConfig {
            agent_id: "recorder".to_string(),
            model: Some("missing-model".to_string()),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-MODELERR");

        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                let cause = diagnostic.causes().join(" ");
                assert!(cause.contains("agent recorder"), "{cause}");
                assert!(cause.contains("missing-model"), "{cause}");
                assert!(cause.contains("valid models:"), "{cause}");
                assert!(cause.contains("gpt-x"), "{cause}");
            }
            other => panic!("non-advertised model must fail mint, got {other:?}"),
        }
        assert!(
            rec.set_config_options.lock().await.is_empty(),
            "invalid model must fail before set_config_option is sent"
        );
    }

    #[tokio::test]
    async fn authenticate_failure_surfaces_agent_not_authenticated() {
        // The agent advertises an auth method, then REJECTS `authenticate`. The
        // backend must surface a structured authentication failure from `connect` (hard fail).
        let rec = Recorder::new("agent-sess-AUTH");
        rec.advertise_auth_method("oauth").await;
        rec.reject_authenticate.store(true, Ordering::SeqCst);

        let (client_side, agent_side) = Channel::duplex();
        spawn_recording_agent(agent_side, rec.clone());
        match AcpBackend::connect(client_side, test_config()).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::Authenticate,
                    DiagnosticFailureClass::Authentication,
                    false,
                );
                assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
            }
            Ok(_) => panic!("authenticate failure must fail connect"),
        }
        // The backend attempted authenticate with the advertised method id.
        assert_eq!(rec.authenticates.lock().await.as_slice(), &["oauth"]);
    }

    #[tokio::test]
    async fn authenticate_attempts_first_advertised_method() {
        // An agent that advertises a method and ACCEPTS `authenticate` connects
        // cleanly; the backend used the first advertised method id.
        let rec = Recorder::new("agent-sess-AUTHOK");
        rec.advertise_auth_method("oauth").await;
        let be = connect_recording(rec.clone()).await;
        // Connected successfully -> authenticate was attempted with "oauth".
        assert_eq!(rec.authenticates.lock().await.as_slice(), &["oauth"]);
        // And the backend captured the advertised method.
        assert_eq!(be.auth_methods().map(<[_]>::len), Some(1));
    }

    #[tokio::test]
    async fn pre_authenticated_skips_advertised_auth_method() {
        let rec = Recorder::new("agent-sess-PREAUTH");
        rec.advertise_auth_method("chat-gpt").await;
        let cfg = AcpConfig {
            pre_authenticated: true,
            ..test_config()
        };

        let _be = connect_recording_with(rec.clone(), cfg).await;

        assert!(
            rec.authenticates.lock().await.is_empty(),
            "ambient credentials must not trigger an interactive authenticate request"
        );
    }

    #[tokio::test]
    async fn pre_authenticated_rejects_explicit_auth_method() {
        let (client_side, _agent_side) = Channel::duplex();
        let cfg = AcpConfig {
            auth_method: Some("chat-gpt".to_string()),
            pre_authenticated: true,
            ..test_config()
        };

        match AcpBackend::connect(client_side, cfg).await {
            Err(BridgeError::ConfigInvalid { reason }) => {
                assert!(reason.contains("pre_authenticated"), "{reason}");
                assert!(reason.contains("auth_method"), "{reason}");
            }
            Err(other) => panic!("expected ConfigInvalid, got {other:?}"),
            Ok(_) => panic!("contradictory auth policy must not connect"),
        }
    }

    fn auth_method(id: &'static str) -> AuthMethod {
        AuthMethod::Agent(AuthMethodAgent::new(AuthMethodId::new(id), id))
    }

    #[test]
    fn choose_auth_method_prefers_chatgpt_over_api_key_default() {
        let advertised = vec![auth_method("api-key"), auth_method("chat-gpt")];
        let chosen =
            AcpBackend::choose_auth_method(None, &advertised).expect("auth method selected");

        assert_eq!(chosen.0.as_ref(), "chat-gpt");
    }

    #[test]
    fn choose_auth_method_prefers_new_chatgpt_id_over_legacy_order() {
        let advertised = vec![
            auth_method("chatgpt"),
            auth_method("chat-gpt"),
            auth_method("api-key"),
        ];
        let chosen =
            AcpBackend::choose_auth_method(None, &advertised).expect("auth method selected");

        assert_eq!(chosen.0.as_ref(), "chat-gpt");
    }

    #[test]
    fn choose_auth_method_falls_back_to_first_without_chatgpt() {
        let advertised = vec![auth_method("api-key"), auth_method("oauth")];
        let chosen =
            AcpBackend::choose_auth_method(None, &advertised).expect("auth method selected");

        assert_eq!(chosen.0.as_ref(), "api-key");
    }

    #[test]
    fn choose_auth_method_supports_legacy_chatgpt_id() {
        let advertised = vec![auth_method("codex-api-key"), auth_method("chatgpt")];
        let chosen =
            AcpBackend::choose_auth_method(None, &advertised).expect("auth method selected");

        assert_eq!(chosen.0.as_ref(), "chatgpt");
    }

    #[test]
    fn choose_auth_method_configured_value_wins() {
        let advertised = vec![auth_method("api-key"), auth_method("chat-gpt")];
        let chosen = AcpBackend::choose_auth_method(Some("custom"), &advertised)
            .expect("auth method selected");

        assert_eq!(chosen.0.as_ref(), "custom");
    }

    #[tokio::test]
    async fn no_auth_methods_skips_authenticate() {
        // An agent that advertises NO auth methods needs no client-driven auth:
        // the backend must SKIP `authenticate` entirely (no request on the wire).
        let rec = Recorder::new("agent-sess-NOAUTH");
        // (default: auth_methods empty)
        let _be = connect_recording(rec.clone()).await;
        assert!(
            rec.authenticates.lock().await.is_empty(),
            "no advertised auth methods -> authenticate must be skipped"
        );
    }

    #[tokio::test]
    async fn handshake_timeout_surfaces_clear_error() {
        // A non-responsive agent that opens stdio but NEVER answers `initialize`
        // must surface a clear BOUNDED error within `handshake_timeout`, not hang.
        let (client_side, agent_side) = Channel::duplex();
        spawn_hung_agent(agent_side);
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            AcpBackend::connect(client_side, test_config_short_handshake()),
        )
        .await
        .expect("connect must return within the handshake bound, not hang");
        match outcome {
            Err(error) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::Initialize,
                    DiagnosticFailureClass::Timeout,
                    false,
                );
            }
            Ok(_) => panic!("a hung initialize handshake must surface a clear error, got Ok"),
        }
    }

    /// Spawn a fake agent that ANSWERS `initialize` (advertising one auth method)
    /// but NEVER answers `authenticate` — it parks the responder forever. Models an
    /// agent that hangs on the auth step; `authenticate` must be bounded by the
    /// same `handshake_timeout` as `initialize` (B3) or `connect` would hang.
    fn spawn_init_ok_auth_hangs_agent(channel: Channel) {
        tokio::spawn(async move {
            let _ = Agent
                .builder()
                .name("auth-hang-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| async move {
                        responder.respond(
                            InitializeResponse::new(ProtocolVersion::V1).auth_methods(vec![
                                AuthMethod::Agent(AuthMethodAgent::new(
                                    AuthMethodId::new("oauth"),
                                    "OAuth",
                                )),
                            ]),
                        )?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |_req: agent_client_protocol::schema::v1::AuthenticateRequest,
                          _responder: agent_client_protocol::Responder<
                        agent_client_protocol::schema::v1::AuthenticateResponse,
                    >,
                          _cx| async move {
                        // Never respond: park forever holding the responder so the
                        // client's `authenticate` request hangs (channel stays open).
                        std::future::pending::<()>().await;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_to(channel)
                .await;
        });
    }

    #[tokio::test]
    async fn authenticate_hang_is_bounded() {
        // B3: an agent that answers `initialize` but HANGS on `authenticate` must
        // NOT block `connect` forever — `authenticate` is inside the bounded
        // handshake, so `connect` returns a clear error within `handshake_timeout`.
        // Outer timeout so a regression (unbounded authenticate) fails FAST.
        let (client_side, agent_side) = Channel::duplex();
        spawn_init_ok_auth_hangs_agent(agent_side);
        let outcome = tokio::time::timeout(
            Duration::from_secs(5),
            AcpBackend::connect(client_side, test_config_short_handshake()),
        )
        .await
        .expect("connect must return within the handshake bound, not hang on authenticate");
        match outcome {
            Err(error) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::Authenticate,
                    DiagnosticFailureClass::Timeout,
                    false,
                );
            }
            Ok(_) => panic!("a hung authenticate must surface a clear bounded error, got Ok"),
        }
    }

    #[tokio::test]
    async fn configured_auth_method_not_advertised_still_attempts() {
        // I4: a configured `auth_method` that the agent did NOT advertise is still
        // attempted (the agent is authoritative). Here the agent advertises a
        // DIFFERENT method ("oauth") and REJECTS the (mismatched) configured one;
        // the backend attempts the configured id, warns about the mismatch, and the
        // rejection surfaces cleanly as a structured authentication failure.
        let rec = Recorder::new("agent-sess-AUTHMISMATCH");
        rec.advertise_auth_method("oauth").await;
        rec.reject_authenticate.store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            auth_method: Some("apikey".to_string()),
            ..test_config()
        };
        let (client_side, agent_side) = Channel::duplex();
        spawn_recording_agent(agent_side, rec.clone());
        match AcpBackend::connect(client_side, cfg).await {
            Err(error) => {
                assert_agent_failure(
                    &error,
                    DiagnosticPhase::Authenticate,
                    DiagnosticFailureClass::Authentication,
                    false,
                );
            }
            Ok(_) => panic!("a mismatched+rejected auth_method must fail cleanly"),
        }
        // The backend attempted the CONFIGURED method id (not the advertised one).
        assert_eq!(rec.authenticates.lock().await.as_slice(), &["apikey"]);
    }

    // ── Task 6 (Increment 3b): per-session config stash ───────────────────────

    #[tokio::test]
    async fn configure_session_applies_model_mode_at_mint() {
        // `configure_session(S, cfg)` STASHES the per-session config; `ensure_session`
        // (driven here directly, exactly as `prompt` does) reads the stash for S at
        // mint and applies BOTH `session/set_mode(mode)` (hard) and
        // `session/set_config_option(model)` for S's agent session — with NO
        // model/mode baked into `AcpConfig` (proving the stash, not the static config,
        // is what drove the configuration).
        let rec = Recorder::new("agent-sess-CFG");
        let be = connect_recording(rec.clone()).await; // test_config(): no model/mode
        let key = bkey("bridge-CFG");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("m".to_string()),
                effort: None,
                mode: Some("x".to_string()),
            }),
        )
        .await
        .unwrap();

        be.ensure_session(&key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.set_mode_seen.notified())
            .await
            .expect("set_mode must reach the agent after session/new");
        tokio::time::timeout(Duration::from_secs(2), rec.set_config_seen.notified())
            .await
            .expect("set_config_option(model) must reach the agent after session/new");
        assert_eq!(
            rec.set_modes.lock().await.as_slice(),
            &["x"],
            "stashed mode applied at mint"
        );
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("model".to_string(), "m".to_string())],
            "stashed model applied at mint"
        );
    }

    #[tokio::test]
    async fn describe_options_reads_advertised_config_options() {
        // The default recording agent advertises a model select + an effort select at session/new.
        // describe_options reads them into AgentCaps WITHOUT sending a prompt or configuring anything.
        let rec = Recorder::new("agent-sess-DESCRIBE");
        let be = connect_recording(rec.clone()).await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let caps = be
            .describe_options_observed(std::path::Path::new("/tmp"), observer.clone())
            .await
            .expect("describe_options succeeds");
        assert_eq!(caps.current_model.as_deref(), Some("default"));
        assert_eq!(
            caps.models,
            vec!["default", "m", "a", "b", "gpt-x", "haiku"]
        );
        assert_eq!(
            caps.effort_levels,
            vec!["low", "medium", "high", "xhigh", "max"]
        );
        assert!(caps.modes.is_empty(), "no mode select advertised");
        // Exactly one throwaway session/new, no prompt, no model/mode configuration.
        assert_eq!(rec.new_session_calls.load(Ordering::SeqCst), 1);
        assert!(rec.prompt_log.lock().await.is_empty(), "no prompt sent");
        assert!(
            rec.set_config_options.lock().await.is_empty() && rec.set_modes.lock().await.is_empty(),
            "describe_options configures nothing"
        );
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .all(|event| event.transition().phase() == DiagnosticPhase::SessionCreate));
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn describe_options_observed_reports_structured_session_failure() {
        let (client_side, agent_side) = Channel::duplex();
        spawn_session_rejecting_agent(agent_side, "discovery session rejected");
        let backend = AcpBackend::connect(client_side, test_config())
            .await
            .unwrap();
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        let error = backend
            .describe_options_observed(std::path::Path::new("/tmp"), observer.clone())
            .await
            .expect_err("rejected discovery session must fail");

        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::SessionCreate,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert_eq!(
            diagnostic.disposition(),
            FailureDisposition::RetrySameTarget
        );
        assert_eq!(
            diagnostic.code().as_str(),
            "acp.discovery.session_create.transport"
        );
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
    }

    #[tokio::test]
    async fn describe_options_observer_failure_prevents_session_creation() {
        let rec = Recorder::new("agent-sess-DESCRIBE-OBSERVER-FAIL");
        let backend = connect_recording(rec.clone()).await;
        let observer = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 1,
        });

        let error = backend
            .describe_options_observed(std::path::Path::new("/tmp"), observer)
            .await
            .expect_err("failed started-event write must stop discovery");

        assert!(matches!(error, BridgeError::StoreFailure));
        assert_eq!(rec.new_session_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn describe_options_reads_session_mode_state() {
        let rec = Recorder::new("agent-sess-DESCRIBE-MODES");
        rec.advertise_model_config.store(false, Ordering::SeqCst);
        rec.advertise_effort_config.store(false, Ordering::SeqCst);
        *rec.session_modes.lock().await = Some(SessionModeState::new(
            "acceptEdits",
            vec![
                SessionMode::new("default", "Default"),
                SessionMode::new("acceptEdits", "Accept edits"),
                SessionMode::new("plan", "Plan"),
            ],
        ));
        let be = connect_recording(rec.clone()).await;

        let caps = be
            .describe_options(std::path::Path::new("/tmp"))
            .await
            .expect("describe_options succeeds");

        assert_eq!(caps.current_mode.as_deref(), Some("acceptEdits"));
        assert_eq!(caps.modes, vec!["default", "acceptEdits", "plan"]);
        assert!(caps.models.is_empty());
        assert!(caps.effort_levels.is_empty());
        assert_eq!(rec.new_session_calls.load(Ordering::SeqCst), 1);
        assert!(rec.prompt_log.lock().await.is_empty(), "no prompt sent");
        assert!(
            rec.set_config_options.lock().await.is_empty() && rec.set_modes.lock().await.is_empty(),
            "describe_options configures nothing"
        );
    }

    #[tokio::test]
    async fn per_session_config_is_isolated() {
        // ONE backend, TWO bridge sessions, each configured with a DIFFERENT model.
        // Minting each reads ITS OWN stash entry: the agent must see model="a"
        // for S1 and model="b" for S2 — no bleed across sessions on the shared
        // connection. (The fake mints the same agent id for both, which is fine: the
        // stash is keyed by the BRIDGE SessionId, so the values stay separate.)
        let rec = Recorder::new("agent-sess-ISO");
        let be = connect_recording(rec.clone()).await;
        let s1 = bkey("bridge-ISO-1");
        let s2 = bkey("bridge-ISO-2");

        be.configure_session(
            &s1,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("a".to_string()),
                effort: None,
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.configure_session(
            &s2,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("b".to_string()),
                effort: None,
                mode: None,
            }),
        )
        .await
        .unwrap();

        be.ensure_session(&s1).await.unwrap();
        be.ensure_session(&s2).await.unwrap();
        // Two mints → two model config calls. `ensure_session` only returns AFTER its
        // mint closure (which sends + awaits set_config_option) runs,
        // so both calls have completed their config update by here; assert on the
        // recorded vec rather than the (single-permit) `Notify` to avoid coalescing.
        let mut models = rec
            .set_config_options
            .lock()
            .await
            .iter()
            .filter(|(id, _)| id == "model")
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>();
        models.sort();
        assert_eq!(
            models.as_slice(),
            &["a", "b"],
            "each session applied its OWN stashed model — no bleed across sessions"
        );
    }

    #[tokio::test]
    async fn retire_is_idempotent() {
        // A long-lived child (`cat` blocks on stdin). `retire()` takes-once and
        // terminates the agent PROCESS; a SECOND `retire()` finds `None` (take-once)
        // and is a clean no-op (no panic, no double-kill).
        let mut supervised = Supervised::spawn("/bin/cat", &[], None).expect("spawn cat");
        let pid = supervised.pid();
        let child = supervised.child_mut();
        let _stdin = child
            .stdin
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("test: stdin unavailable"))
            .unwrap();
        let _stdout = child
            .stdout
            .take()
            .ok_or_else(|| BridgeError::agent_crashed("test: stdout unavailable"))
            .unwrap();

        let backend = AcpBackend {
            conn: None,
            supervised: Arc::new(StdMutex::new(Some(supervised))),
            stderr_ring: None,
            config: None,
            container_reap: None,
            reaped: Arc::new(AtomicBool::new(false)),
            unavailable: Arc::new(AtomicBool::new(false)),
            dispatch_gate: Arc::new(StdMutex::new(())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cfg: Arc::new(StdMutex::new(HashMap::new())),
            pending_turn_meta: StdMutex::new(HashMap::new()),
            policy: Arc::new(StdMutex::new(
                Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
            )),
            permission_registry: Arc::new(StdMutex::new(None)),
            perm_timeout_ms: Arc::new(AtomicU64::new(120_000)),
            prompt_snapshot_hook: StdMutex::new(None),
            before_process_redactor_hook: StdMutex::new(None),
            fail_deferred_cancel_send: Arc::new(AtomicBool::new(false)),
            fail_cancel_send: Arc::new(AtomicBool::new(false)),
        };

        assert!(
            unsafe { libc::kill(pid as i32, 0) } == 0,
            "child alive pre-retire"
        );

        backend.retire().await.expect("first retire terminates");
        assert!(
            backend.unavailable.load(Ordering::SeqCst),
            "retirement must close the connection fence before process teardown"
        );
        // The take-once consumed the child; a second retire is a no-op.
        backend
            .retire()
            .await
            .expect("second retire is a clean no-op (take-once → None)");

        // After the first retire the child is terminated (SIGTERM→reap); confirm the
        // pid is no longer a live, signalable process we own.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        assert!(!alive, "child must be terminated after the first retire");
    }

    #[tokio::test]
    async fn in_process_retire_fences_active_lease_before_next_prompt() {
        let rec = Recorder::new("agent-sess-RETIRED-LEASE");
        let backend = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RETIRED-LEASE");
        backend.ensure_session(&key).await.unwrap();

        backend.retire().await.unwrap();
        let error = match backend.prompt(&key, vec![]).await {
            Err(error) => error,
            Ok(_) => panic!("an active lease must not dispatch after backend retirement"),
        };
        assert_agent_failure(
            &error,
            DiagnosticPhase::PromptStart,
            DiagnosticFailureClass::AgentProcess,
            false,
        );
        assert!(
            rec.prompt_log.lock().await.is_empty(),
            "retirement must fence even when the in-process transport remains open"
        );
    }

    #[tokio::test]
    async fn configure_session_applies_effort_at_mint() {
        // A stashed `effort` drives `session/set_config_option` with the advertised
        // effort id and resolved value (`High -> "high"`) at mint.
        let rec = Recorder::new("agent-sess-EFFORT");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-EFFORT");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: Some(Effort::High),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.set_config_seen.notified())
            .await
            .expect("set_config_option must reach the agent after session/new");
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("effort".to_string(), "high".to_string())],
            "stashed effort applied as effort=high at mint"
        );
    }

    #[tokio::test]
    async fn effort_error_is_non_fatal() {
        // An unrelated config-option error must be NON-FATAL, and must not trigger
        // unsupported-effort walk-down retries.
        let rec = Recorder::new("agent-sess-EFFORTERR");
        rec.reject_set_config.store(true, Ordering::SeqCst);
        *rec.set_config_error_body.lock().await = "usage_update failed".to_string();
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-EFFORTERR");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: Some(Effort::Max),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.expect(
            "a set_config_option error must NOT fail session setup (effort is best-effort)",
        );
        // The (erroring) agent still recorded the attempt with the Max -> "max" mapping.
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("effort".to_string(), "max".to_string())],
        );
        // A subsequent prompt still works (connection/session survived the error).
        rec.set_updates(vec![ScriptedUpdate::Text("ok")]).await;
        let mut s = be.prompt(&key, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "ok"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
    }

    #[tokio::test]
    async fn no_model_option_but_pinned_model_fails_mint() {
        let rec = Recorder::new("agent-sess-NOMODELOPT");
        rec.advertise_model_config.store(false, Ordering::SeqCst);
        let cfg = AcpConfig {
            agent_id: "recorder".to_string(),
            model: Some("haiku".to_string()),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-NOMODELOPT");

        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                let reason = diagnostic.causes().join(" ");
                assert!(
                    reason.contains("agent recorder advertised no model option"),
                    "{reason}"
                );
                assert!(reason.contains("model=haiku"), "{reason}");
            }
            other => {
                panic!("pinned model without advertised model option must fail, got {other:?}")
            }
        }
        assert!(rec.set_config_options.lock().await.is_empty());
    }

    #[tokio::test]
    async fn blocked_current_model_without_pin_fails_mint() {
        let rec = Recorder::new("agent-sess-FABLE-CURRENT");
        *rec.model_config_current.lock().await = "claude-fable-5[1m]".to_string();
        *rec.model_config_values.lock().await = vec![
            "default".to_string(),
            "claude-fable-5[1m]".to_string(),
            "sonnet".to_string(),
        ];
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-FABLE-CURRENT");

        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                let reason = diagnostic.causes().join(" ");
                assert!(
                    reason.contains("current model=claude-fable-5[1m] is blocked by this bridge"),
                    "{reason}"
                );
            }
            other => {
                panic!("blocked current model without a pin must fail, got {other:?}")
            }
        }
        assert!(rec.set_config_options.lock().await.is_empty());
    }

    #[tokio::test]
    async fn blocked_current_model_requires_confirmed_nonblocked_apply_at_mint() {
        let rec = Recorder::new("agent-sess-FABLE-EMPTY-REFRESH");
        *rec.model_config_current.lock().await = "claude-fable-5[1m]".to_string();
        *rec.model_config_values.lock().await = vec![
            "default".to_string(),
            "claude-fable-5[1m]".to_string(),
            "sonnet".to_string(),
        ];
        rec.empty_config_response_on_set
            .store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            agent_id: "recorder".to_string(),
            model: Some("sonnet".to_string()),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-FABLE-EMPTY-REFRESH");

        match be.ensure_session(&key).await {
            Err(error) => {
                let diagnostic = assert_agent_failure(
                    &error,
                    DiagnosticPhase::ConfigApply,
                    DiagnosticFailureClass::Model,
                    false,
                );
                let reason = diagnostic.causes().join(" ");
                assert!(
                    reason.contains(
                        "could not confirm model=sonnet replaced blocked current model=claude-fable-5[1m]"
                    ),
                    "{reason}"
                );
            }
            other => {
                panic!(
                    "blocked current model must require confirmed non-blocked apply, got {other:?}"
                )
            }
        }
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("model".to_string(), "sonnet".to_string())]
        );
        assert!(rec.prompt_log.lock().await.is_empty());
    }

    #[tokio::test]
    async fn effort_configured_but_no_effort_option_warn_skips() {
        let rec = Recorder::new("agent-sess-NOEFFORTOPT");
        rec.advertise_effort_config.store(false, Ordering::SeqCst);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-NOEFFORTOPT");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: Some(Effort::High),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key)
            .await
            .expect("missing effort option is a warn+skip, not a mint failure");
        assert!(
            rec.set_config_options.lock().await.is_empty(),
            "no effort option means no set_config_option attempt"
        );
    }

    #[tokio::test]
    async fn effort_walkdown_converges_from_xhigh_to_high() {
        let rec = Recorder::new("agent-sess-WALKDOWN");
        *rec.effort_config_values.lock().await = vec!["high".to_string(), "xhigh".to_string()];
        *rec.reject_set_config_call.lock().await = Some(1);
        *rec.set_config_error_body.lock().await = "Invalid value for effort".to_string();
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-WALKDOWN");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: Some(Effort::Max),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.unwrap();

        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[
                ("effort".to_string(), "xhigh".to_string()),
                ("effort".to_string(), "high".to_string())
            ],
            "Max falls back to advertised xhigh, then unsupported xhigh walks down to high"
        );
    }

    #[tokio::test]
    async fn effort_walkdown_stops_on_unrelated_internal_error() {
        let rec = Recorder::new("agent-sess-WALKSTOP");
        *rec.effort_config_values.lock().await = vec!["high".to_string(), "xhigh".to_string()];
        *rec.reject_set_config_call.lock().await = Some(1);
        *rec.set_config_error_body.lock().await = "usage_update failed".to_string();
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-WALKSTOP");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: Some(Effort::Max),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key)
            .await
            .expect("unrelated effort error is non-fatal");

        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("effort".to_string(), "xhigh".to_string())],
            "unrelated -32603 must not retry lower effort levels"
        );
    }

    #[tokio::test]
    async fn effort_resolves_against_refreshed_post_model_options() {
        let rec = Recorder::new("agent-sess-REFRESHED");
        *rec.effort_config_values.lock().await = vec!["low".to_string()];
        *rec.refreshed_effort_values_after_model.lock().await = Some(vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ]);
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-REFRESHED");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("m".to_string()),
                effort: Some(Effort::High),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.unwrap();

        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[
                ("model".to_string(), "m".to_string()),
                ("effort".to_string(), "high".to_string())
            ],
            "effort must resolve from model's refreshed config_options, not stale session/new options"
        );
    }

    #[tokio::test]
    async fn mint_populates_config_surface_cache_with_refreshed_opts() {
        let rec = Recorder::new("agent-sess-CACHE");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-CACHE");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: Some("m".to_string()),
                effort: Some(Effort::High),
                mode: None,
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.unwrap();

        let entry = be.session_entry(&key).await;
        let surface = entry
            .config_surface
            .lock()
            .expect("config_surface lock")
            .clone()
            .expect("mint must cache the refreshed config surface");
        assert_eq!(select_current(&surface.opts, "model").as_deref(), Some("m"));
        assert_eq!(
            select_current(&surface.opts, "effort").as_deref(),
            Some("high")
        );
    }

    #[tokio::test]
    async fn reconcile_config_on_minted_session_applies_changed_model() {
        let rec = Recorder::new("agent-sess-RECON-MODEL");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RECON-MODEL");

        be.ensure_session(&key).await.unwrap();
        let outcome = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("m".to_string()),
                    effort: None,
                    mode: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(outcome, bridge_core::orch::ReconcileOutcome::Applied);
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("model".to_string(), "m".to_string())],
            "warm reconcile must apply the changed model via session/set_config_option"
        );
    }

    #[tokio::test]
    async fn reconcile_config_on_minted_session_rejects_blocked_fable_model() {
        let rec = Recorder::new("agent-sess-RECON-MODEL-ALIAS");
        *rec.model_config_values.lock().await = vec![
            "default".to_string(),
            "sonnet".to_string(),
            "claude-fable-5[1m]".to_string(),
        ];
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RECON-MODEL-ALIAS");

        be.ensure_session(&key).await.unwrap();
        let outcome = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("fable".to_string()),
                    effort: None,
                    mode: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(outcome, bridge_core::orch::ReconcileOutcome::NotAdvertised);
        assert!(rec.set_config_options.lock().await.is_empty());
    }

    #[tokio::test]
    async fn reconcile_config_model_without_refreshed_current_is_not_confirmed() {
        let rec = Recorder::new("agent-sess-RECON-MODEL-NO-CURRENT");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RECON-MODEL-NO-CURRENT");

        be.ensure_session(&key).await.unwrap();
        rec.advertise_model_config.store(false, Ordering::SeqCst);
        let outcome = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("m".to_string()),
                    effort: None,
                    mode: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(
            outcome,
            bridge_core::orch::ReconcileOutcome::NotAdvertised,
            "warm reconcile must not advance when refreshed options omit model state"
        );
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[("model".to_string(), "m".to_string())],
            "the model apply was attempted before the missing read-back was detected"
        );
    }

    #[tokio::test]
    async fn reconcile_config_unadvertised_model_returns_not_advertised() {
        let rec = Recorder::new("agent-sess-RECON-BADMODEL");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RECON-BADMODEL");

        be.ensure_session(&key).await.unwrap();
        let outcome = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: Some("not-a-model".to_string()),
                    effort: None,
                    mode: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(outcome, bridge_core::orch::ReconcileOutcome::NotAdvertised);
        assert!(
            rec.set_config_options.lock().await.is_empty(),
            "unadvertised model must fail before sending set_config_option"
        );
    }

    #[tokio::test]
    async fn reconcile_config_refreshes_cache_between_warm_effort_applies() {
        let rec = Recorder::new("agent-sess-RECON-EFFORT");
        let be = connect_recording(rec.clone()).await;
        let key = bkey("bridge-RECON-EFFORT");

        be.ensure_session(&key).await.unwrap();
        let high = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: None,
                    effort: Some(Effort::High),
                    mode: None,
                }),
            )
            .await
            .unwrap();
        let low = be
            .reconcile_config(
                &key,
                &SessionSpec::from_config(EffectiveConfig {
                    model: None,
                    effort: Some(Effort::Low),
                    mode: None,
                }),
            )
            .await
            .unwrap();

        assert_eq!(high, bridge_core::orch::ReconcileOutcome::Applied);
        assert_eq!(low, bridge_core::orch::ReconcileOutcome::Applied);
        assert_eq!(
            rec.set_config_options.lock().await.as_slice(),
            &[
                ("effort".to_string(), "high".to_string()),
                ("effort".to_string(), "low".to_string())
            ],
            "warm effort reconcile must use the refreshed cache so high then low both apply"
        );
    }

    #[tokio::test]
    async fn forget_session_drops_stash_falls_back_to_static_config() {
        // After `forget_session`, the stash entry is gone, so the NEXT mint falls
        // back to `AcpConfig`'s static mode (here "static-mode") rather than the
        // forgotten stashed mode. Uses two DIFFERENT bridge keys (a mint is once-per
        // key), proving the fallback path for an unconfigured session.
        let rec = Recorder::new("agent-sess-FORGET");
        let be = connect_recording_with(rec.clone(), test_config_with_mode("static-mode")).await;
        let configured = bkey("bridge-FORGET-cfg");
        let forgotten = bkey("bridge-FORGET-gone");

        // A configured session uses its stashed mode.
        be.configure_session(
            &configured,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: None,
                mode: Some("stashed-mode".to_string()),
            }),
        )
        .await
        .unwrap();
        be.ensure_session(&configured).await.unwrap();

        // Configure then FORGET a second session → its next mint has no stash entry
        // and falls back to the static config mode.
        be.configure_session(
            &forgotten,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: None,
                mode: Some("doomed-mode".to_string()),
            }),
        )
        .await
        .unwrap();
        be.forget_session(&forgotten).await;
        be.ensure_session(&forgotten).await.unwrap();

        let modes = rec.set_modes.lock().await.clone();
        assert!(
            modes.contains(&"stashed-mode".to_string()),
            "configured session used its stashed mode, got {modes:?}"
        );
        assert!(
            modes.contains(&"static-mode".to_string()),
            "forgotten session fell back to the static config mode, got {modes:?}"
        );
        assert!(
            !modes.contains(&"doomed-mode".to_string()),
            "the forgotten stash entry must NOT drive set_mode, got {modes:?}"
        );
    }

    #[tokio::test]
    async fn release_session_removes_both_sessions_and_cfg_entries() {
        let rec = Recorder::new("agent-sess-RELEASE");
        let be = connect_recording(rec).await;
        let s = SessionId::parse("ctx-x-g0").unwrap();
        be.configure_session(&s, &SessionSpec::from_config(Default::default()))
            .await
            .unwrap();
        let _ = be.session_entry(&s).await;
        be.release_session(&s).await;
        assert!(
            be.session_cfg.lock().unwrap().get(&s).is_none(),
            "cfg stash removed"
        );
        assert!(
            be.sessions.lock().await.get(&s).is_none(),
            "agent session removed"
        );
    }

    #[tokio::test]
    async fn observed_cancel_is_persistence_first_and_records_teardown() {
        let rec = Recorder::new("agent-sess-CANCEL-OBSERVED");
        let backend = connect_recording(rec.clone()).await;
        let session = bkey("bridge-CANCEL-OBSERVED");
        backend.ensure_session(&session).await.unwrap();

        let rejecting = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 1,
        });
        assert_eq!(
            backend.cancel_observed(&session, rejecting).await,
            Err(BridgeError::StoreFailure)
        );
        assert!(
            rec.cancels.lock().await.is_empty(),
            "cancel must not dispatch when Teardown Started cannot persist"
        );

        let observer = Arc::new(InMemoryDiagnosticObserver::new(4).unwrap());
        backend
            .cancel_observed(&session, observer.clone())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("observed cancel must reach the agent");
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().phase(), DiagnosticPhase::Teardown);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn observed_forget_is_persistence_first_and_clears_only_forget_state() {
        let backend = connect_recording(Recorder::new("agent-sess-FORGET-OBSERVED")).await;
        let session = bkey("bridge-FORGET-OBSERVED");
        backend
            .configure_session(&session, &SessionSpec::from_config(Default::default()))
            .await
            .unwrap();
        backend
            .configure_turn(
                &session,
                turn_meta("ctx-forget-observed", 1, "op-forget-observed"),
            )
            .await;
        let entry = backend.session_entry(&session).await;

        let rejecting = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 1,
        });
        assert_eq!(
            backend.forget_session_observed(&session, rejecting).await,
            Err(BridgeError::StoreFailure)
        );
        assert!(backend.session_cfg.lock().unwrap().contains_key(&session));
        assert!(backend.take_pending_turn_meta(&session).is_some());
        backend
            .configure_turn(
                &session,
                turn_meta("ctx-forget-observed", 2, "op-forget-observed-2"),
            )
            .await;

        let observer = Arc::new(InMemoryDiagnosticObserver::new(4).unwrap());
        backend
            .forget_session_observed(&session, observer.clone())
            .await
            .unwrap();
        assert!(!backend.session_cfg.lock().unwrap().contains_key(&session));
        assert!(backend.take_pending_turn_meta(&session).is_none());
        assert!(
            Arc::ptr_eq(&entry, &backend.session_entry(&session).await),
            "forget must not release the live ACP session"
        );
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn release_session_clears_pending_turn_meta() {
        let rec = Recorder::new("agent-sess-RELEASE-META");
        let be = connect_recording(rec).await;
        let s = SessionId::parse("ctx-release-meta-g0").unwrap();
        be.configure_turn(&s, turn_meta("ctx-release-meta", 1, "op-release-meta"))
            .await;

        be.release_session(&s).await;

        assert!(
            be.take_pending_turn_meta(&s).is_none(),
            "release_session removes pending turn metadata"
        );
    }

    #[tokio::test]
    async fn observed_release_records_teardown_and_removes_session_state() {
        let rec = Recorder::new("agent-sess-RELEASE-OBSERVED");
        let backend = connect_recording(rec.clone()).await;
        let session = bkey("bridge-RELEASE-OBSERVED");
        backend
            .configure_session(&session, &SessionSpec::from_config(Default::default()))
            .await
            .unwrap();
        backend.ensure_session(&session).await.unwrap();
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        backend
            .release_session_observed(&session, observer.clone())
            .await
            .unwrap();

        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert!(events
            .iter()
            .all(|event| { event.transition().phase() == DiagnosticPhase::Teardown }));
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
        assert!(backend.sessions.lock().await.get(&session).is_none());
        assert!(backend.session_cfg.lock().unwrap().get(&session).is_none());
        tokio::time::timeout(Duration::from_secs(2), rec.cancel_seen.notified())
            .await
            .expect("observed release must dispatch session/cancel");
    }

    #[tokio::test]
    async fn observed_ro_release_joins_one_container_reap_and_records_completion() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_attempt = Arc::clone(&calls);
        let attempt: ReapAttemptFn = Arc::new(move |_runtime, _name| {
            let calls = Arc::clone(&calls_for_attempt);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        });
        let controller = ReapController::new("docker", "a2a-ro-release-success", attempt);
        let config = test_config();
        let recorder = Recorder::new("agent-sess-RO-RELEASE-SUCCESS");
        let mut backend = connect_recording_with(recorder.clone(), config).await;
        backend.container_reap = Some(controller.clone());
        let session = bkey("bridge-RO-RELEASE-SUCCESS");
        backend.ensure_session(&session).await.unwrap();
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        backend
            .release_session_observed(&session, observer.clone())
            .await
            .unwrap();
        // Retirement/drop may join the same controller later, but no path may
        // install a second runtime removal attempt.
        backend.retire().await.unwrap();
        controller.reap_observed().await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
        assert_eq!(
            events[1].transition().code().map(|code| code.as_str()),
            Some("acp.teardown.container_reaped")
        );
        tokio::time::timeout(Duration::from_secs(2), recorder.cancel_seen.notified())
            .await
            .expect("observed :ro release still clears the ACP session");
    }

    #[tokio::test]
    async fn observed_ro_release_surfaces_each_typed_runtime_failure() {
        for failure in [
            ReapFailure::Spawn,
            ReapFailure::Timeout,
            ReapFailure::NonZeroExit,
            ReapFailure::WorkerPanicked,
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let calls_for_attempt = Arc::clone(&calls);
            let attempt: ReapAttemptFn = Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls_for_attempt);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err(failure)
                })
            });
            let controller = ReapController::new("docker", "a2a-ro-release-failure", attempt);
            let config = test_config();
            let mut backend =
                connect_recording_with(Recorder::new("agent-sess-RO-RELEASE-FAILURE"), config)
                    .await;
            backend.container_reap = Some(controller);
            let session = bkey("bridge-RO-RELEASE-FAILURE");
            backend.ensure_session(&session).await.unwrap();
            let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

            let error = backend
                .release_session_observed(&session, observer.clone())
                .await
                .expect_err("typed runtime failure must fail observed release");
            let diagnostic = assert_agent_failure(
                &error,
                DiagnosticPhase::Teardown,
                DiagnosticFailureClass::ContainerRuntime,
                true,
            );
            assert_eq!(diagnostic.disposition(), FailureDisposition::Fatal);
            assert_eq!(diagnostic.code().as_str(), failure.code());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let events = observer.snapshot().await;
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].transition().status(), PhaseStatus::Started);
            assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
        }
    }

    #[tokio::test]
    async fn checked_ro_release_surfaces_the_typed_runtime_failure() {
        let attempt: ReapAttemptFn =
            Arc::new(|_runtime, _name| Box::pin(async move { Err(ReapFailure::Timeout) }));
        let controller = ReapController::new("docker", "a2a-ro-checked", attempt);
        let mut backend =
            connect_recording_with(Recorder::new("agent-sess-RO-CHECKED"), test_config()).await;
        backend.container_reap = Some(controller.clone());
        let session = bkey("bridge-RO-CHECKED");
        backend.ensure_session(&session).await.unwrap();

        let error = backend
            .release_session_checked(&session)
            .await
            .expect_err("checked cleanup must join the container-removal flight");
        let diagnostic = assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::ContainerRuntime,
            true,
        );
        assert_eq!(diagnostic.code().as_str(), ReapFailure::Timeout.code());
        assert_eq!(controller.result(), Some(Err(ReapFailure::Timeout)));
    }

    #[tokio::test]
    async fn checked_ro_release_waits_for_the_owned_successful_cleanup() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let attempt: ReapAttemptFn = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |_runtime, _name| {
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            })
        };
        let controller = ReapController::new("docker", "a2a-ro-checked-ok", attempt);
        let mut backend =
            connect_recording_with(Recorder::new("agent-sess-RO-CHECKED-OK"), test_config()).await;
        backend.container_reap = Some(controller.clone());
        let backend = Arc::new(backend);
        let session = bkey("bridge-RO-CHECKED-OK");
        backend.ensure_session(&session).await.unwrap();

        let task = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.release_session_checked(&session).await })
        };
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("checked release starts the container cleanup");
        assert!(!task.is_finished());
        release.notify_one();
        task.await.unwrap().unwrap();
        assert_eq!(controller.result(), Some(Ok(())));
    }

    #[tokio::test]
    async fn rejected_ro_release_observation_cannot_suppress_or_detach_cleanup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_attempt = Arc::clone(&calls);
        let entered = Arc::new(Notify::new());
        let entered_for_attempt = Arc::clone(&entered);
        let release = Arc::new(Notify::new());
        let release_for_attempt = Arc::clone(&release);
        let attempt: ReapAttemptFn = Arc::new(move |_runtime, _name| {
            let calls = Arc::clone(&calls_for_attempt);
            let entered = Arc::clone(&entered_for_attempt);
            let release = Arc::clone(&release_for_attempt);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                entered.notify_one();
                release.notified().await;
                Ok(())
            })
        });
        let controller = ReapController::new("docker", "a2a-ro-release-rejected", attempt);
        let config = test_config();
        let mut backend =
            connect_recording_with(Recorder::new("agent-sess-RO-RELEASE-REJECTED"), config).await;
        backend.container_reap = Some(controller.clone());
        let backend = Arc::new(backend);
        let session = bkey("bridge-RO-RELEASE-REJECTED");
        backend.ensure_session(&session).await.unwrap();
        let rejecting = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 1,
        });
        let backend_for_release = Arc::clone(&backend);
        let session_for_release = session.clone();
        let task = tokio::spawn(async move {
            backend_for_release
                .release_session_observed(&session_for_release, rejecting)
                .await
        });

        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("reap must start before the rejected diagnostic write can return");
        assert!(
            !task.is_finished(),
            "observer failure must wait for the already-owned cleanup result"
        );
        release.notify_one();
        assert_eq!(task.await.unwrap(), Err(BridgeError::StoreFailure));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(controller.result(), Some(Ok(())));
        assert!(backend.sessions.lock().await.get(&session).is_none());
    }

    #[tokio::test]
    async fn failed_ro_cleanup_event_persistence_remains_the_public_error() {
        let attempt: ReapAttemptFn =
            Arc::new(|_runtime, _name| Box::pin(async move { Err(ReapFailure::Timeout) }));
        let controller = ReapController::new("docker", "a2a-ro-persistence", attempt);
        let mut backend =
            connect_recording_with(Recorder::new("agent-sess-RO-PERSISTENCE"), test_config()).await;
        backend.container_reap = Some(controller.clone());
        let session = bkey("bridge-RO-PERSISTENCE");
        backend.ensure_session(&session).await.unwrap();
        let rejecting = Arc::new(RejectOnRecord {
            count: AtomicU64::new(0),
            reject_at: 2,
        });

        assert_eq!(
            backend.release_session_observed(&session, rejecting).await,
            Err(BridgeError::StoreFailure),
            "the failed-event journal write is a real persistence boundary"
        );
        assert_eq!(
            controller.result(),
            Some(Err(ReapFailure::Timeout)),
            "the process-owned controller still retains its typed result"
        );
    }

    #[tokio::test]
    async fn observed_pre_dispatch_cancel_and_release_failures_are_not_accepted() {
        let backend = AcpBackend {
            conn: None,
            supervised: Arc::new(StdMutex::new(None)),
            stderr_ring: None,
            config: Some(test_config()),
            container_reap: None,
            reaped: Arc::new(AtomicBool::new(false)),
            unavailable: Arc::new(AtomicBool::new(false)),
            dispatch_gate: Arc::new(StdMutex::new(())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            session_cfg: Arc::new(StdMutex::new(HashMap::new())),
            pending_turn_meta: StdMutex::new(HashMap::new()),
            policy: Arc::new(StdMutex::new(
                Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
            )),
            permission_registry: Arc::new(StdMutex::new(None)),
            perm_timeout_ms: Arc::new(AtomicU64::new(120_000)),
            prompt_snapshot_hook: StdMutex::new(None),
            before_process_redactor_hook: StdMutex::new(None),
            fail_deferred_cancel_send: Arc::new(AtomicBool::new(false)),
            fail_cancel_send: Arc::new(AtomicBool::new(false)),
        };
        let session = bkey("bridge-RELEASE-ERROR");
        backend
            .configure_session(&session, &SessionSpec::from_config(Default::default()))
            .await
            .unwrap();
        let _ = backend.session_entry(&session).await;
        let cancel_observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let cancel_error = backend
            .cancel_observed(&session, cancel_observer.clone())
            .await
            .expect_err("missing ACP connection must fail observed cancellation");
        assert_agent_failure(
            &cancel_error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            false,
        );
        let cancel_events = cancel_observer.snapshot().await;
        assert_eq!(cancel_events.len(), 2);
        assert_eq!(cancel_events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(cancel_events[1].transition().status(), PhaseStatus::Failed);

        let checked_error = backend.release_session_checked(&session).await.expect_err(
            "checked release must not downgrade a transport failure to best-effort success",
        );
        assert!(matches!(checked_error, BridgeError::AgentCrashed { .. }));
        assert!(backend.sessions.lock().await.get(&session).is_none());
        assert!(backend.session_cfg.lock().unwrap().get(&session).is_none());

        backend
            .configure_session(&session, &SessionSpec::from_config(Default::default()))
            .await
            .unwrap();
        let _ = backend.session_entry(&session).await;

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        let error = backend
            .release_session_observed(&session, observer.clone())
            .await
            .expect_err("missing ACP connection must be a teardown failure");
        assert_agent_failure(
            &error,
            DiagnosticPhase::Teardown,
            DiagnosticFailureClass::Transport,
            false,
        );
        assert!(backend.sessions.lock().await.get(&session).is_none());
        assert!(backend.session_cfg.lock().unwrap().get(&session).is_none());
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
    }

    // ── Task 4 (session-cwd): per-session cwd at mint + immutability guard ────

    #[tokio::test]
    async fn mint_uses_stashed_cwd() {
        // `configure_session` with a stashed `SessionSpec.cwd` → `ensure_session`
        // passes THAT cwd to `session/new`, NOT the static `AcpConfig.cwd` (/tmp).
        let rec = Recorder::new("agent-sess-SCWD");
        let be = connect_recording(rec.clone()).await; // static cwd = /tmp
        let key = bkey("bridge-SCWD");

        be.configure_session(
            &key,
            &SessionSpec {
                config: EffectiveConfig {
                    model: None,
                    effort: None,
                    mode: None,
                },
                cwd: Some(SessionCwd::parse("/req").unwrap()),
            },
        )
        .await
        .unwrap();

        be.ensure_session(&key).await.unwrap();

        // The recording agent stashes the cwd from each session/new; assert it
        // received the stashed /req, NOT the static /tmp.
        let recorded = rec.new_session_cwd.lock().await.clone();
        assert_eq!(
            recorded.as_deref(),
            Some(std::path::Path::new("/req")),
            "ensure_session must pass the stashed SessionSpec.cwd (/req) to session/new, \
             not the static AcpConfig.cwd (/tmp); got {recorded:?}"
        );
    }

    #[tokio::test]
    async fn mint_falls_back_to_static_cwd() {
        // `configure_session` with `cwd: None` → `ensure_session` falls back to
        // the static `AcpConfig.cwd` (/tmp from `test_config()`).
        let rec = Recorder::new("agent-sess-SCWDSTATIC");
        let be = connect_recording(rec.clone()).await; // static cwd = /tmp
        let key = bkey("bridge-SCWDSTATIC");

        be.configure_session(
            &key,
            &SessionSpec::from_config(EffectiveConfig {
                model: None,
                effort: None,
                mode: None,
            }),
        )
        .await
        .unwrap();

        be.ensure_session(&key).await.unwrap();

        let recorded = rec.new_session_cwd.lock().await.clone();
        assert_eq!(
            recorded.as_deref(),
            Some(std::path::Path::new("/tmp")),
            "with no stashed cwd, ensure_session must use the static AcpConfig.cwd (/tmp); \
             got {recorded:?}"
        );
    }

    #[tokio::test]
    async fn cwd_immutable_after_mint() {
        // Once a session is minted with cwd /a, a subsequent `configure_session`
        // stashing cwd /b and then calling `ensure_session` again must return
        // a structured config failure — NOT silently reuse the warm /a session for /b.
        let rec = Recorder::new("agent-sess-SCWDIMM");
        // Use a custom config so the static cwd is /a (drives the first mint).
        let cfg = AcpConfig {
            cwd: std::path::PathBuf::from("/a"),
            ..AcpConfig::default()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-SCWDIMM");

        // First mint: configure with cwd /a (explicit, same as static — proving
        // the recorded cwd is what was ACTUALLY passed, not re-derived).
        be.configure_session(
            &key,
            &SessionSpec {
                config: EffectiveConfig {
                    model: None,
                    effort: None,
                    mode: None,
                },
                cwd: Some(SessionCwd::parse("/a").unwrap()),
            },
        )
        .await
        .unwrap();
        be.ensure_session(&key).await.unwrap(); // mints the session with /a

        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "session was minted exactly once"
        );

        // Now try to reuse the warm session for cwd /b — must error.
        be.configure_session(
            &key,
            &SessionSpec {
                config: EffectiveConfig {
                    model: None,
                    effort: None,
                    mode: None,
                },
                cwd: Some(SessionCwd::parse("/b").unwrap()),
            },
        )
        .await
        .unwrap();

        let err = be.ensure_session(&key).await.expect_err(
            "ensure_session on an already-minted session with a DIFFERENT cwd must return an error",
        );
        let diagnostic = assert_agent_failure(
            &err,
            DiagnosticPhase::ConfigApply,
            DiagnosticFailureClass::Config,
            false,
        );
        assert_eq!(diagnostic.code().as_str(), "acp.config.cwd_mismatch");

        // The session was NOT re-minted (still exactly one session/new).
        assert_eq!(
            rec.new_session_calls.load(Ordering::SeqCst),
            1,
            "no additional session/new must be sent on the immutability guard path"
        );
    }
}
