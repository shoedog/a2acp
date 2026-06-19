// server.rs — the A2A v1 inbound HTTP/JSON-RPC server (spec §5.1/§5.3, Task 13).
//
// `InboundServer` wires the bridge pipeline behind an axum router:
//
//   inbound JSON-RPC request
//     -> AuthMiddleware.authorize        (reject -> JSON-RPC error, pipeline NOT run)
//     -> assert_supported_version(hdr)   (unknown A2A-Version -> JSON-RPC error)
//     -> RouteDecision.route             (pick the backend agent)
//     -> Translator.run(backend, ...)    (drive the AgentBackend, anti-corruption)
//     -> events streamed back            (SSE for streaming; collected for unary)
//
// The streaming method guarantees a final flush: the translator emits the
// Artifact event last, and we forward events in order, so the terminal SSE
// frame is always the artifact.
//
// We hand-roll the server on axum 0.7 rather than adopting `a2a-server-lf`
// (see docs/adr/0003-a2a-sdk.md): that crate requires axum 0.8 and inverts
// control through its own task store / executor traits, which fights our
// auth->route->translate pipeline. axum 0.7 is already proven in this workspace.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};

use a2a::{methods, SVC_PARAM_VERSION};
use bridge_core::domain::{
    effective_config, AgentOverride, AuthContext, EffectiveConfig, InboundRequest, Part,
    PeerTaskId, RouteTarget, SessionSpec, TaskMeta,
};
use bridge_core::error::{A2aDisposition, BridgeError};
use bridge_core::ids::{AgentId, ContextId, OperationId, SessionId, TaskId};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, AuthMiddleware, DelegationPort, Lease, PolicyEngine,
    RouteDecision, SessionStore,
};
use bridge_core::translator::{Event, EventKind, TaskOutcome, Translator};
use bridge_core::SessionCwd;

use crate::card::{agent_card, assert_supported_version, A2A_PINNED_VERSION};
use crate::fanout::{self, Source};
use crate::sse::event_to_sse;

/// JSON-RPC 2.0 error code for an invalid request / rejected pipeline gate.
const JSONRPC_INVALID_REQUEST: i32 = -32600;
/// JSON-RPC 2.0 error code for an unknown method.
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC 2.0 error code for invalid params.
const JSONRPC_INVALID_PARAMS: i32 = -32602;
/// JSON-RPC 2.0 internal error.
const JSONRPC_INTERNAL: i32 = -32603;

/// A task's binding to its resolved registry instance, created on the FIRST local
/// message. Holds the backend driving the task, the effective config applied to its
/// session, and the registry [`Lease`] keeping the slot alive for the task's
/// lifetime. The lease drops when the binding is removed from the map ([`BindingGuard`]
/// eviction on producer exit), decrementing the slot's active-task count.
struct TaskBinding {
    backend: Arc<dyn AgentBackend>,
    /// The effective config applied to the task's session — reused by binding-driven
    /// follow-ups (they prompt the bound backend without recomputing config). Kept on
    /// the binding so the resolved config is available for the task's whole lifetime.
    eff: EffectiveConfig,
    /// The registry lease keeping the slot alive for the task. Dropped (releasing the
    /// slot's active-task count) when the binding is removed on producer exit.
    lease: Box<dyn Lease>,
}

/// RAII eviction guard owned by a task's producer. While alive it represents the
/// task's binding; on `Drop` — whether the producer returns cleanly (Done/Failed/
/// Canceled) OR early (client disconnect / error) — it removes the [`TaskBinding`]
/// from the map (dropping the [`Lease`] → the slot's active-task count decrements)
/// and forgets the backend's per-session stash. This is the spec-critical
/// "eviction on EVERY producer exit": a leaked lease keeps a slot un-retirable
/// forever, so the guard must fire on the non-clean paths a manual cleanup might miss.
///
/// `Drop` is synchronous but the eviction is async (mutex lock + `forget_session`),
/// so it is performed on a spawned task. A follow-up that REUSES an existing binding
/// does NOT own a guard — only the FIRST message's producer evicts.
struct BindingGuard {
    bindings: Arc<tokio::sync::Mutex<HashMap<TaskId, TaskBinding>>>,
    task: TaskId,
    backend: Arc<dyn AgentBackend>,
    session: SessionId,
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
            // Take the binding out of the map and drop its Lease explicitly → the
            // slot's active-task count decrements. Then forget the per-session stash.
            if let Some(binding) = bindings.lock().await.remove(&task) {
                drop(binding.lease);
            }
            backend.forget_session(&session).await;
        });
    }
}

/// The inbound A2A server. Holds the six pipeline ports plus the advertised
/// base URL (used to build the Agent Card). Cheap to clone via `Arc`.
pub struct InboundServer {
    /// The agent registry (3b): replaces the single `backend`. First-message LOCAL
    /// dispatch resolves the routed `AgentId` to a `(backend, lease)` here, applies
    /// per-session config, and instance-binds the task (see [`InboundServer::bindings`]).
    registry: Arc<dyn AgentRegistry>,
    /// Task → resolved-instance binding, created on a task's FIRST local message.
    /// Holds the backend, its effective config, and the registry lease keeping the
    /// slot alive for the task's lifetime. T10 only CREATES bindings (and uses them
    /// for cancel when present); T11 adds binding-check-before-route on follow-ups
    /// and RAII eviction on producer exit.
    bindings: Arc<tokio::sync::Mutex<HashMap<TaskId, TaskBinding>>>,
    store: Arc<dyn SessionStore>,
    policy: Arc<dyn PolicyEngine>,
    route: Arc<dyn RouteDecision>,
    auth: Arc<dyn AuthMiddleware>,
    base_url: String,
    delegation: Arc<dyn DelegationPort>,
    /// Wire-observable label for the LOCAL backend's fan-out source (e.g. `"kiro"`,
    /// `"codex"`). Fed from `[agent] name` so a non-Kiro agent isn't mislabeled in
    /// fan-out artifacts. Used by [`local_kiro_source`] for the local `Source` id.
    local_source_label: String,
    /// Single-cancel guard: the set of local task ids whose upstream peer
    /// `CancelTask` has already been POSTed. An inbound `CancelTask` (the
    /// `cancel_task()` handler) and the streaming cancel supervisor both race to
    /// cancel an active delegated peer; this set ensures whichever wins the race
    /// POSTs exactly once and the other skips. Both `cancel` paths must remain —
    /// the handler covers the stream/supervisor already having ended, the
    /// supervisor covers disconnect/latch during the stream — so this is a GUARD,
    /// not a removed path.
    cancelled_peers: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    /// The workflow executor (W1), wired via [`InboundServer::with_workflows`]. `None`
    /// when no workflows are configured (the boot default), in which case a
    /// `RouteTarget::Workflow` producer fails its task (no executor to run it).
    executor: Option<std::sync::Arc<bridge_workflow::executor::WorkflowExecutor>>,
    /// The boot-loaded workflow graphs, keyed by id. Empty by default; populated by
    /// [`InboundServer::with_workflows`]. The producer looks up the routed
    /// `WorkflowId` here to obtain the graph to run.
    workflows: std::sync::Arc<
        std::collections::HashMap<
            bridge_core::ids::WorkflowId,
            std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
        >,
    >,
    /// Per-task workflow cancellation tokens, inserted when a workflow producer
    /// starts and removed when it ends. `cancel_task` cancels the token for the
    /// task if present (workflow cancel path), mirroring the local/delegate latches.
    workflow_cancels: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<
                bridge_core::ids::TaskId,
                tokio_util::sync::CancellationToken,
            >,
        >,
    >,
    /// Durable task control-plane store (W3a). Defaults to an in-memory store;
    /// replace with a persistent backend via [`InboundServer::with_task_store`].
    task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    session_manager: Option<std::sync::Arc<crate::session_manager::SessionManager>>,
    /// Global root path that gates which per-request session cwds are permitted
    /// (session_cwd increment). `None` → no global root restriction.
    /// Wired from `allowed_cwd_root` in the top-level config via
    /// [`InboundServer::with_allowed_cwd_root`].
    pub allowed_cwd_root: Option<String>,
    /// Per-task broadcast hubs for streaming reattach (Task 3). Keyed by TaskId;
    /// populated by the DetachedProgressSink (Task 4) and read by the
    /// SubscribeToTask handler (Tasks 7-9). Cleaned up by the Finalizer (Task 6).
    pub(crate) progress_hubs: Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<TaskId, Arc<crate::reattach::TaskProgressHub>>,
        >,
    >,
    /// Live per-agent model catalog (advertise-models). Probed host-side at `serve` startup and on
    /// `SIGHUP`; [`serve_card`] reads it lock-free via `ArcSwap` so the card path never probes. Default
    /// empty → the card omits the `agent-models` extension (same as a fully-failed probe). Wired from
    /// `main`'s startup probe via [`InboundServer::with_model_catalog`].
    pub model_catalog: Arc<arc_swap::ArcSwap<bridge_core::catalog::ModelCatalog>>,
}

impl InboundServer {
    /// Construct a server from the six pipeline ports, the advertised base URL,
    /// and the local-source label (the fan-out id for the local backend).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<dyn AgentRegistry>,
        store: Arc<dyn SessionStore>,
        policy: Arc<dyn PolicyEngine>,
        route: Arc<dyn RouteDecision>,
        auth: Arc<dyn AuthMiddleware>,
        base_url: impl Into<String>,
        delegation: Arc<dyn DelegationPort>,
        local_source_label: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            bindings: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            store,
            policy,
            route,
            auth,
            base_url: base_url.into(),
            delegation,
            local_source_label: local_source_label.into(),
            cancelled_peers: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            executor: None,
            workflows: Arc::new(HashMap::new()),
            workflow_cancels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            task_store: std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new()),
            session_manager: None,
            allowed_cwd_root: None,
            progress_hubs: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            model_catalog: Arc::new(arc_swap::ArcSwap::from_pointee(
                bridge_core::catalog::ModelCatalog::new(),
            )),
        }
    }

    /// Attach the workflow executor and the boot-loaded workflow graphs (W1). Builder
    /// over [`InboundServer::new`] so existing `new` call sites are untouched; `main`
    /// chains `.with_workflows(executor, map)` after constructing the server.
    #[must_use]
    pub fn with_workflows(
        mut self,
        executor: std::sync::Arc<bridge_workflow::executor::WorkflowExecutor>,
        workflows: std::collections::HashMap<
            bridge_core::ids::WorkflowId,
            std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
        >,
    ) -> Self {
        self.executor = Some(executor);
        self.workflows = std::sync::Arc::new(workflows);
        self
    }

    /// Override the durable task store (W3a). Builder over [`InboundServer::new`]
    /// so existing `new` call sites are untouched; the detached runner (Task 7)
    /// uses this to wire a persistent SQLite store.
    #[must_use]
    pub fn with_task_store(
        mut self,
        task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    ) -> Self {
        self.task_store = task_store;
        self
    }

    #[must_use]
    pub fn with_session_manager(
        mut self,
        sm: std::sync::Arc<crate::session_manager::SessionManager>,
    ) -> Self {
        self.session_manager = Some(sm);
        self
    }

    /// Set the global root path that gates per-request session cwds (session_cwd increment).
    /// Builder over [`InboundServer::new`]; wired from `allowed_cwd_root` in the top-level
    /// config. `None` (the default) → no global root restriction.
    #[must_use]
    pub fn with_allowed_cwd_root(mut self, root: Option<String>) -> Self {
        self.allowed_cwd_root = root;
        self
    }

    /// Attach the live model catalog handle (advertise-models). Builder over [`InboundServer::new`];
    /// `main` probes all agents at startup, wraps the result in an `ArcSwap`, and threads it here so the
    /// card reads the current catalog and a `SIGHUP` re-probe can atomically swap it.
    #[must_use]
    pub fn with_model_catalog(
        mut self,
        catalog: Arc<arc_swap::ArcSwap<bridge_core::catalog::ModelCatalog>>,
    ) -> Self {
        self.model_catalog = catalog;
        self
    }

    /// Test-only: is a progress hub currently registered for `task`? Used by the
    /// reattach tests to assert hub-before-spawn insertion and hub cleanup on
    /// terminal (the `progress_hubs` field is `pub(crate)`, so integration tests in
    /// a separate crate cannot inspect it directly).
    #[doc(hidden)]
    pub async fn has_progress_hub_for_test(&self, task: &TaskId) -> bool {
        self.progress_hubs.lock().await.contains_key(task)
    }

    /// Build the axum router, mounting the Agent Card and JSON-RPC endpoint.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/.well-known/agent-card.json", get(serve_card))
            .route("/", post(jsonrpc))
            .with_state(self)
    }

    // ---- pipeline helpers (shared by streaming + unary paths) ----

    /// Run the auth -> version -> route gates. On success returns the routed
    /// `(TaskId, SessionId, Vec<Part>)` for the translator. On failure returns
    /// the `BridgeError` (the caller maps it to a JSON-RPC error). The backend
    /// is NOT touched here, so a rejecting gate never reaches `prompt`.
    fn gate(&self, headers: &HeaderMap, params: &Value) -> Result<RoutedCall, BridgeError> {
        // 1. Authorize. We derive a minimal InboundRequest from the bearer token.
        let token = bearer_token(headers);
        let inbound = match token {
            Some(t) => InboundRequest::with_token(&t),
            None => InboundRequest::anon(),
        };
        let auth = self.auth.authorize(&inbound)?;

        // 2. Version gate: the A2A-Version header must match our pinned version.
        let version = headers
            .get(SVC_PARAM_VERSION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(A2A_PINNED_VERSION);
        assert_supported_version(version)?;

        // 2b. H1: reject a message with NO extractable text content. Done BEFORE routing
        //     so message validity is independent of routing — a prober can't distinguish
        //     "valid agent, bad content" from "invalid agent" by the error. An empty
        //     `parts` would otherwise reach the backend as a contentless prompt (zero ACP
        //     content blocks) and surface as an opaque "agent crashed".
        let parts = parts_from_params(params);
        if parts.is_empty() {
            return Err(BridgeError::InvalidRequest {
                field: "message: no text content (expected message.parts[].text or message.text)",
            });
        }

        // 3. Route. Parse skill/agent/overrides from params and pass to route decision. The
        //    target (Local vs Delegate) is carried through so the handler picks
        //    the local-backend producer or the delegation producer.
        //    Invalid metadata (bad agent id, unknown effort) returns InvalidRequest → client error.
        let task_meta = task_meta_from_params(params)?;
        let target = self.route.route(&task_meta)?;

        let context_id = context_id_from_params(params)?;
        if context_id.is_some() && !matches!(target, RouteTarget::Local(_)) {
            return Err(BridgeError::InvalidRequest {
                field: "contextId is only supported on the local route in Slice 0",
            });
        }

        // 3b. Parse + validate the per-request cwd (a2a-bridge.cwd).  Validation
        //     must happen BEFORE task/session ids are derived so a malformed request
        //     is rejected before any state is created.
        let session_cwd = session_cwd_from_params(params, self.allowed_cwd_root.as_deref())?;

        // 4. Derive task/session ids from params (best-effort; v1 stubs allowed).
        let task = task_id_from_params(params)?;
        let session = SessionId::parse(format!("session-{}", task.as_str()))
            .unwrap_or_else(|_| SessionId::parse("session-default").unwrap());
        Ok(RoutedCall {
            task,
            session,
            parts,
            target,
            auth,
            // Per-request overrides ride along so LOCAL dispatch can compute the
            // effective config (entry defaults layered with these) before the prompt.
            overrides: task_meta.overrides,
            context_id,
            session_cwd,
        })
    }
}

/// Single-cancel guard for an active delegated task. Atomically check-and-insert
/// the local task id into the shared set, returning `true` exactly once per task
/// — the caller that gets `true` "wins the race" and must POST the upstream
/// `delegation.cancel(peer)`; all later callers get `false` and must skip.
async fn try_win_peer_cancel(
    guard: &Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    local: &TaskId,
) -> bool {
    guard.lock().await.insert(local.as_str().to_owned())
}

/// Per-source single-cancel guard. Like [`try_win_peer_cancel`] but keyed by an
/// arbitrary string so fan-out can guard each source independently
/// (`"{task}:kiro"`, `"{task}:peer"`) while plain-delegate keeps using the bare
/// task id. Returns `true` exactly once per key — the winner performs the cancel.
async fn try_win_cancel_key(
    guard: &Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    key: String,
) -> bool {
    guard.lock().await.insert(key)
}

/// The result of the gate: the routed call ready for the translator or delegation.
struct RoutedCall {
    task: TaskId,
    session: SessionId,
    parts: Vec<Part>,
    target: RouteTarget,
    auth: AuthContext,
    /// Per-request config overrides parsed from `a2a-bridge.{model,effort,mode}`.
    /// LOCAL dispatch layers these on the resolved entry's defaults via
    /// [`effective_config`] before `configure_session`. The selected `AgentId`
    /// itself is carried by `target` (`RouteTarget::Local(id)`).
    overrides: Option<AgentOverride>,
    /// A2A contextId for warm continuation (Slice 0). Honored only on the Local route. None = legacy.
    #[allow(dead_code)]
    context_id: Option<ContextId>,
    /// Per-request working directory parsed from `a2a-bridge.cwd`.
    /// Distinct from `AgentOverride` so it reaches both single-agent and workflow
    /// dispatch (AgentOverride is dropped for workflows). `None` when absent.
    /// Applied by Tasks 6 (single-agent) and 7 (workflow).
    session_cwd: Option<SessionCwd>,
}

/// Extract the resolved `AgentId` from a `RouteTarget::Local`, falling back to the
/// registry default for any non-Local target (the fan-out local source resolves the
/// default; Delegate never reaches here).
fn local_agent_id(srv: &InboundServer, target: &RouteTarget) -> AgentId {
    match target {
        RouteTarget::Local(id) => id.clone(),
        _ => srv.registry.default_id(),
    }
}

/// The local backend ready to drive a task, plus its RAII eviction guard. A
/// FIRST-message dispatch returns `Some(guard)` (the producer owns it → evicts the
/// binding/lease/stash on exit); a FOLLOW-UP that reused an existing binding returns
/// `None` (the original producer owns eviction — a follow-up must not evict a still-
/// live binding when its own short-lived call ends).
struct LocalDispatch {
    backend: Arc<dyn AgentBackend>,
    /// The session to prompt against — warm `ctx-…` session, or legacy `session-{task}`.
    session: SessionId,
    guard: Option<BindingGuard>,
    /// Warm path only: finishes the warm turn (→ Idle) on drop. Mutually exclusive with `guard`.
    warm_guard: Option<WarmTurnGuard>,
}

/// Drops the warm turn back to Idle on producer exit (mirrors BindingGuard::Drop's spawn pattern).
struct WarmTurnGuard {
    sm: std::sync::Arc<crate::session_manager::SessionManager>,
    ctx: bridge_core::ids::ContextId,
    generation: bridge_core::ids::SessionGeneration,
    op: bridge_core::ids::OperationId,
}

impl Drop for WarmTurnGuard {
    fn drop(&mut self) {
        let sm = self.sm.clone();
        let ctx = self.ctx.clone();
        let generation = self.generation;
        let op = self.op.clone();
        tokio::spawn(async move {
            sm.finish_turn(&ctx, generation, &op).await;
        });
    }
}

/// LOCAL dispatch — binding-driven (T11). If the task already has a [`TaskBinding`]
/// (a FOLLOW-UP / re-send for a live task) reuse the BOUND backend directly: bypass
/// `route()`/`resolve()`/`configure_session` entirely, so a concurrent registry edit
/// (the bound agent removed) can't make a follow-up fail or hit the wrong backend.
/// Otherwise (a FIRST message) resolve the agent id to a `(backend, lease)`, compute
/// its effective config (entry defaults layered with the per-request override), apply
/// it via `configure_session`, insert a [`TaskBinding`], and hand back a
/// [`BindingGuard`] so the producer evicts on exit. On resolve/configure failure the
/// error propagates (callers map it to a `Failed` terminal / JSON-RPC error).
///
/// Bind-before-spawn: callers run this in the handler BEFORE spawning the producer,
/// so the binding exists the instant the call returns — a follow-up or cancel can
/// never race an async bind window.
async fn resolve_configure_bind(
    srv: &InboundServer,
    agent_id: &AgentId,
    task: &TaskId,
    session: &SessionId,
    overrides: Option<&AgentOverride>,
    session_cwd: Option<SessionCwd>,
) -> Result<LocalDispatch, BridgeError> {
    // Follow-up: a binding already exists → reuse the bound `(backend, eff)`, no
    // route/resolve/recompute. Re-stash the bound effective config with the per-request
    // cwd (harmless idempotent overwrite; it only affects a not-yet-minted session,
    // never retroactively re-configures a live one), then prompt the bound backend.
    {
        let bindings = srv.bindings.lock().await;
        if let Some(binding) = bindings.get(task) {
            let backend = binding.backend.clone();
            let eff = binding.eff.clone();
            drop(bindings);
            backend
                .configure_session(
                    session,
                    &SessionSpec {
                        config: eff,
                        cwd: session_cwd,
                    },
                )
                .await?;
            return Ok(LocalDispatch {
                backend,
                session: session.clone(),
                guard: None,
                warm_guard: None,
            });
        }
    }
    // First message: resolve, configure, bind, and hand back an eviction guard.
    let resolved = srv.registry.resolve(agent_id).await?;
    let eff = effective_config(&resolved.entry, overrides);
    resolved
        .backend
        .configure_session(
            session,
            &SessionSpec {
                config: eff.clone(),
                cwd: session_cwd,
            },
        )
        .await?;
    let backend = resolved.backend.clone();
    srv.bindings.lock().await.insert(
        task.clone(),
        TaskBinding {
            backend: backend.clone(),
            eff,
            lease: resolved.lease,
        },
    );
    let guard = BindingGuard {
        bindings: srv.bindings.clone(),
        task: task.clone(),
        backend: backend.clone(),
        session: session.clone(),
    };
    Ok(LocalDispatch {
        backend,
        session: session.clone(),
        guard: Some(guard),
        warm_guard: None,
    })
}

/// Slice 0 warm path. Returns None when there's no contextId or no SessionManager (caller uses
/// the legacy resolve_configure_bind). Resolves ONCE inside the manager.
async fn warm_local_dispatch(
    srv: &Arc<InboundServer>,
    agent_id: &AgentId,
    routed: &RoutedCall,
    op: OperationId,
) -> Option<Result<LocalDispatch, BridgeError>> {
    let ctx = routed.context_id.clone()?;
    let sm = srv.session_manager.clone()?;
    match sm
        .checkout_turn(
            &ctx,
            agent_id.clone(),
            routed.overrides.clone(),
            routed.session_cwd.clone(),
            op,
        )
        .await
    {
        Ok(turn) => {
            if let Some(w) = &turn.usage_warning {
                tracing::warn!(target: "a2a_bridge::usage", ctx = %ctx.as_str(),
                    used = w.used, size = w.size, fraction = w.fraction, threshold = w.threshold,
                    "usage_threshold_warn");
            }
            Some(Ok(LocalDispatch {
                backend: turn.backend,
                session: turn.session,
                guard: None,
                warm_guard: Some(WarmTurnGuard {
                    sm,
                    ctx,
                    generation: turn.generation,
                    op: turn.op.clone(),
                }),
            }))
        }
        Err(e) => Some(Err(e)),
    }
}

/// Fan-out variant of [`resolve_configure_bind`]: resolve the local agent, apply
/// its effective config, and return the `(backend, lease)` so the fan-out producer
/// can HOLD them for the source's lifetime — the same instance drives the prompt
/// AND the cancel, and the lease keeps the slot from retiring mid-fan-out. Unlike
/// the single-source path it does NOT insert a [`TaskBinding`]: a fan-out task is
/// not a follow-up-local task, so it has no binding-driven follow-up to serve.
async fn resolve_for_fanout(
    srv: &InboundServer,
    agent_id: &AgentId,
    _task: &TaskId,
    session: &SessionId,
    overrides: Option<&AgentOverride>,
    session_cwd: Option<SessionCwd>,
) -> Result<(Arc<dyn AgentBackend>, Box<dyn Lease>), BridgeError> {
    let resolved = srv.registry.resolve(agent_id).await?;
    let eff = effective_config(&resolved.entry, overrides);
    resolved
        .backend
        .configure_session(
            session,
            &SessionSpec {
                config: eff,
                cwd: session_cwd,
            },
        )
        .await?;
    Ok((resolved.backend, resolved.lease))
}

/// Resolve the backend to cancel a LOCAL task — binding-driven (T11). If the task has
/// a live [`TaskBinding`] (created by its first message), cancel THAT exact instance
/// — the one driving the task — so a multi-agent or post-reload cancel never hits the
/// WRONG backend (the spec-critical property the default-fallback violated).
///
/// When no `TaskBinding` exists, the function resolves the **registry default** agent
/// as a fallback (calls `registry.default_id()` then `registry.resolve()`). This
/// covers two legitimate no-binding cases: (1) a cancel that arrives after the binding
/// was already evicted (the task completed/failed/was-canceled and is no longer a live
/// local task), and (2) a fan-out task's Kiro cancel, which never creates a
/// `TaskBinding` (fan-out uses `resolve_for_fanout` instead). There is no store
/// read here — the fallback goes directly to the registry.
///
/// // TODO(3d): In 3b a `Fanout` route's local agent IS always the registry default
/// // (`local_agent_id` returns `default_id()` for any non-Local target, and the
/// // router returns `RouteTarget::Fanout` for fan-out tasks). So resolving the
/// // registry default here cancels the correct backend. When Increment 3d adds
/// // fan-out across NON-default registered agents, a fan-out cancel must target the
/// // specific instance that drove each source (e.g. via the held backend from
/// // `resolve_for_fanout`, or a per-source binding), NOT the registry default —
/// // otherwise it cancels the wrong backend for any non-default fan-out leg.
async fn cancel_backend_for(
    srv: &InboundServer,
    task: &TaskId,
) -> Result<Arc<dyn AgentBackend>, BridgeError> {
    if let Some(binding) = srv.bindings.lock().await.get(task) {
        return Ok(binding.backend.clone());
    }
    // Fallback: no binding exists — resolve the registry default agent.
    // See function doc for the two legitimate no-binding cases this covers.
    let default = srv.registry.default_id();
    Ok(srv.registry.resolve(&default).await?.backend)
}

// ---- axum handlers ----

/// `GET /.well-known/agent-card.json` -> the Agent Card as JSON.
async fn serve_card(State(srv): State<Arc<InboundServer>>) -> Response {
    let workflow_ids: Vec<&str> = srv.workflows.keys().map(|k| k.as_str()).collect();
    let mcp = srv.registry.mcp_advertisement();
    let catalog = srv.model_catalog.load();
    Json(agent_card(&srv.base_url, &workflow_ids, &mcp, &catalog)).into_response()
}

/// `POST /` -> the JSON-RPC dispatch surface.
async fn jsonrpc(
    State(srv): State<Arc<InboundServer>>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let req = body.0;
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match method {
        m if m == methods::SEND_STREAMING_MESSAGE => stream_message(srv, headers, id, params).await,
        m if m == methods::SUBSCRIBE_TO_TASK => subscribe_to_task(srv, headers, id, params).await,
        m if m == methods::SEND_MESSAGE => unary_message(srv, headers, id, params).await,
        m if m == methods::CANCEL_TASK => cancel_task(srv, headers, id, params).await,
        m if m == methods::GET_TASK => get_task(srv, headers, id, params).await,
        m if m == methods::LIST_TASKS => list_tasks(srv, headers, id, params).await,
        "SessionStatus" => session_status(srv, headers, id, params).await,
        "SessionRelease" => session_release(srv, headers, id, params).await,
        "SessionCancel" => session_cancel(srv, headers, id, params).await,
        "SessionClear" => session_clear(srv, headers, id, params).await,
        "" => jsonrpc_err(id, JSONRPC_INVALID_REQUEST, "missing method"),
        _ => jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "method not found"),
    }
}

/// Streaming path: gate, then stream translator events as SSE with a final flush.
async fn stream_message(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let routed = match srv.gate(&headers, &params) {
        Ok(r) => r,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };

    // Persist task->session before driving the backend.
    let _ = srv.store.put(&routed.task, &routed.session).await;

    // The SSE producer feeds events into an mpsc channel; the response stream
    // owns no borrowed references. Both the local and delegate paths reuse this
    // exact channel -> SSE wiring (DRY).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, BridgeError>>(64);
    let task_id_str = routed.task.as_str().to_owned();
    let context_id_str = routed
        .context_id
        .as_ref()
        .map(|c| c.as_str().to_string())
        .unwrap_or_else(|| task_id_str.clone());

    match routed.target {
        RouteTarget::Local(_) => {
            // Bind-before-spawn: resolve+configure+bind (or reuse a follow-up binding)
            // in the handler so the binding exists the instant the producer starts —
            // a follow-up or cancel can never race an async bind window. On a
            // resolve/configure failure (e.g. unknown agent) emit a terminal Failed
            // SSE frame rather than a JSON-RPC error (streaming has already committed
            // to an SSE response).
            let agent_id = local_agent_id(&srv, &routed.target);
            let op = OperationId::parse(format!("op-{}", routed.task.as_str())).unwrap();
            let dispatch = match warm_local_dispatch(&srv, &agent_id, &routed, op).await {
                Some(r) => r,
                None => {
                    resolve_configure_bind(
                        &srv,
                        &agent_id,
                        &routed.task,
                        &routed.session,
                        routed.overrides.as_ref(),
                        routed.session_cwd.clone(),
                    )
                    .await
                }
            };
            match dispatch {
                Ok(dispatch) => {
                    let _ = srv.store.put(&routed.task, &dispatch.session).await;
                    spawn_local_producer(&srv, routed, dispatch, tx)
                }
                Err(e) => {
                    tokio::spawn(async move {
                        let _ = tx.send(Err(e)).await;
                        let _ = tx.send(Ok(Event::terminal(TaskOutcome::Failed))).await;
                    });
                }
            };
        }
        RouteTarget::Delegate => spawn_delegate_producer(&srv, routed, tx),
        RouteTarget::Fanout => spawn_fanout_producer(&srv, routed, tx),
        // Bind by ref + clone so `routed` stays whole for the producer (which
        // consumes it for `task`/`parts`).
        RouteTarget::Workflow(ref id) => {
            let id = id.clone();
            spawn_workflow_producer(&srv, routed, id, tx)
        }
    }

    let sse_stream = sse_event_stream(rx, task_id_str, context_id_str);
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `SubscribeToTask` handler: auth + A2A-version check, extract `id` (with
/// `taskId` as a lenient alias per I3), parse the `Last-Event-ID` cursor as
/// `Option<i64>`, look up the task in the durable store, return not-found if
/// absent. Does NOT call `gate()` — it never starts a new run.
///
/// Stub: the snapshot/SSE body is filled in by Tasks 8-9; for now returns an
/// immediately-closing empty SSE event-stream so the wire sees a proper
/// `text/event-stream` response.
async fn subscribe_to_task(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    // Auth + version: the same bearer-token + supported-version check `gate()` performs,
    // WITHOUT gate()'s routing (gate() would synthesize a task and start a run).
    let token = bearer_token(&headers);
    let inbound = match token {
        Some(t) => InboundRequest::with_token(&t),
        None => InboundRequest::anon(),
    };
    if let Err(e) = srv.auth.authorize(&inbound) {
        return bridge_err_to_jsonrpc(id, &e);
    }

    // A2A-version check.
    let version = headers
        .get(SVC_PARAM_VERSION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(A2A_PINNED_VERSION);
    if let Err(e) = assert_supported_version(version) {
        return bridge_err_to_jsonrpc(id, &e);
    }

    // I3: read the standard a2a-lf `id` field first; fall back to `taskId` as a
    // lenient alias so old clients that send only `taskId` are not broken.
    let task_str = params["id"].as_str().or_else(|| params["taskId"].as_str());
    let task_str = match task_str {
        Some(s) => s,
        None => {
            return bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest { field: "id" });
        }
    };

    // Parse the raw string into a bridge-core TaskId (rejects empty strings).
    let task_id = match bridge_core::ids::TaskId::parse(task_str) {
        Ok(t) => t,
        Err(_) => {
            return bridge_err_to_jsonrpc(id, &BridgeError::InvalidRequest { field: "id" });
        }
    };

    // I2 cursor: absent Last-Event-ID → None (not 0); Some(K) → only seq > K.
    let cursor: Option<i64> = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());

    // Look up the task in the durable store.  Not found → not-found error (M5: do
    // NOT fall through to gate() and start a new run).
    match srv.task_store.get(&task_id).await {
        Ok(Some(rec)) => {
            if rec.status.is_terminal() {
                // --- Terminal-task flow (Task 8) --- read the snapshot and replay it
                // as a FINITE SSE stream (snapshot → SnapshotComplete → Terminal → close).
                let snap = match srv.task_store.progress_snapshot(&task_id).await {
                    Ok(s) => s,
                    Err(_) => return bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure),
                };
                terminal_sse_response(&snap, cursor)
            } else {
                // --- Working-task flow (Task 9) --- subscribe-first, then snapshot,
                // then live-tail the hub until the Terminal frame.
                working_sse_response(&srv, &task_id, cursor, id).await
            }
        }
        Ok(None) => bridge_err_to_jsonrpc(id, &BridgeError::TaskNotFound),
        Err(_) => bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure),
    }
}

/// Build the FINITE terminal SSE response from a durable snapshot: the
/// cursor-filtered snapshot frames, a `SnapshotComplete` sentinel, and (unless the
/// cursor already covers a known `terminal_seq`) the `Terminal` frame, then close.
///
/// Shared by BOTH the initial-terminal branch (the `get()` already returned a
/// terminal record) AND the I5 terminal-during-snapshot race in the working flow
/// (the post-subscribe snapshot read sees a terminal status). A terminal stream is
/// FINITE — it ends after the Terminal frame, so NO keep-alive (which would be a
/// reader-trap suggesting more may come).
fn terminal_sse_response(
    snap: &bridge_core::task_store::TaskProgressSnapshot,
    cursor: Option<i64>,
) -> Response {
    // Build snapshot frames (cursor-filtered, seq-ordered).
    let mut frames = snapshot_frames(snap, cursor);

    // Append SnapshotComplete sentinel (seq = max snapshot frame seq or cut_seq).
    let sentinel_seq = frames.last().map(|f| f.seq).unwrap_or(snap.cut_seq);
    frames.push(crate::reattach::WorkflowProgressFrame {
        v: 1,
        seq: sentinel_seq,
        phase: crate::reattach::Phase::Snapshot,
        kind: crate::reattach::FrameKind::SnapshotComplete,
    });

    // Append Terminal frame unless cursor already covers a KNOWN terminal_seq.
    // None (legacy) → always emit; Some(ts) → emit only if cursor < ts.
    let emit_terminal = snap
        .terminal_seq
        .is_none_or(|ts| cursor.is_none_or(|k| ts > k));
    if emit_terminal {
        use bridge_core::task_store::TaskRecordStatus;
        let outcome = match snap.status {
            TaskRecordStatus::Completed => crate::reattach::TerminalOutcome::Completed,
            TaskRecordStatus::Canceled => crate::reattach::TerminalOutcome::Canceled,
            // Failed, Interrupted, Working all map to Failed on the wire.
            _ => crate::reattach::TerminalOutcome::Failed,
        };
        let output = snap
            .result
            .clone()
            .or(snap.error.clone())
            .unwrap_or_default();
        frames.push(crate::reattach::WorkflowProgressFrame {
            v: 1,
            seq: snap.terminal_seq.unwrap_or(0),
            phase: crate::reattach::Phase::Live,
            kind: crate::reattach::FrameKind::Terminal { outcome, output },
        });
    }

    // Convert Vec<WorkflowProgressFrame> into an SSE stream and return.
    let sse_stream = futures::stream::iter(frames.into_iter().map(|f| {
        Ok::<_, std::convert::Infallible>(
            SseEvent::default()
                .id(f.seq.to_string())
                .data(serde_json::to_string(&f).unwrap_or_default()),
        )
    }));
    Sse::new(sse_stream).into_response()
}

/// Build the streaming working SSE response: subscribe to the task's live progress
/// hub FIRST (the exactly-once boundary), replay the durable snapshot, emit
/// `SnapshotComplete`, then live-tail the broadcast receiver until the `Terminal`
/// frame, then close.
///
/// Exactly-once across the snapshot↔live boundary: `rx = hub.subscribe()` happens
/// BEFORE the snapshot read, so a frame published between the snapshot cut and the
/// live tail is buffered in `rx` (no gap), and the dedup floor drops any live frame
/// whose seq is already covered by the snapshot (no dup).
///
/// Races handled:
/// - No hub registered yet (runner not started / just deregistered) → re-`get`; if
///   now terminal, replay via [`terminal_sse_response`]; else a RETRYABLE JSON-RPC
///   error (the client retries — the durable snapshot makes this lossless).
/// - I5 terminal-during-snapshot: the post-subscribe snapshot reads a terminal
///   status → replay via [`terminal_sse_response`] (do NOT rely on `rx` Closed).
/// - I7 broadcast lag: `RecvError::Lagged` → emit ONE retryable `event: error` SSE
///   event then close (the client reconnects with its `Last-Event-ID` cursor and
///   re-snapshots — no lost state because the snapshot is durable).
async fn working_sse_response(
    srv: &Arc<InboundServer>,
    task_id: &TaskId,
    cursor: Option<i64>,
    id: Value,
) -> Response {
    // Look up the per-task progress hub.
    let hub = srv.progress_hubs.lock().await.get(task_id).cloned();
    let hub = match hub {
        Some(h) => h,
        None => {
            // No hub: the runner hasn't registered yet, or it just deregistered on
            // finishing. Re-`get` to disambiguate: if the task is now terminal, the
            // runner finished — replay the terminal snapshot. Otherwise it is a
            // transient bind/dereg window → a RETRYABLE error (the client retries;
            // the durable snapshot makes the retry lossless).
            return match srv.task_store.get(task_id).await {
                Ok(Some(rec)) if rec.status.is_terminal() => {
                    match srv.task_store.progress_snapshot(task_id).await {
                        Ok(snap) => terminal_sse_response(&snap, cursor),
                        Err(_) => bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure),
                    }
                }
                // Still Working but no hub: a transient bind/dereg window. Map to a
                // server-side (INTERNAL, not 400-reject) error so the client retries
                // rather than treating it as a permanent not-found.
                Ok(Some(_)) => bridge_err_to_jsonrpc(id, &BridgeError::AgentOverloaded),
                Ok(None) => bridge_err_to_jsonrpc(id, &BridgeError::TaskNotFound),
                Err(_) => bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure),
            };
        }
    };

    // EXACTLY-ONCE BOUNDARY: subscribe BEFORE reading the snapshot. A frame
    // published after this point is buffered in `rx`, so nothing is lost in the
    // window between the snapshot cut and the start of the live tail.
    let rx = hub.subscribe();

    // Read the durable snapshot.
    let snap = match srv.task_store.progress_snapshot(task_id).await {
        Ok(s) => s,
        Err(_) => return bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure),
    };

    // I5: the task finished during/just-before the snapshot read → replay the
    // terminal snapshot (do NOT rely on `rx` Closed; the runner may have published
    // its Terminal frame before we subscribed).
    if snap.status.is_terminal() {
        return terminal_sse_response(&snap, cursor);
    }

    // Snapshot phase: cursor-filtered, seq-ordered frames + a SnapshotComplete
    // sentinel (seq = max snapshot frame seq, else cut_seq).
    let mut snapshot_vec = snapshot_frames(&snap, cursor);
    let sentinel_seq = snapshot_vec.last().map(|f| f.seq).unwrap_or(snap.cut_seq);
    snapshot_vec.push(crate::reattach::WorkflowProgressFrame {
        v: 1,
        seq: sentinel_seq,
        phase: crate::reattach::Phase::Snapshot,
        kind: crate::reattach::FrameKind::SnapshotComplete,
    });

    // Dedup floor: a cursor-less subscriber keeps seq 0, and the snapshot↔live
    // overlap is deduped (drop live frames already covered by the snapshot cut).
    let dedup_floor = cursor.unwrap_or(-1).max(snap.cut_seq);

    // Build the streaming SSE body: the snapshot vec first, then the live tail from
    // `rx`. `async_stream::stream!` lets us write the imperative snapshot-then-tail
    // logic; the receiver is MOVED in so the subscribe already happened at
    // handler-call time (the test publishes AFTER the handler returns).
    let stream = async_stream::stream! {
        // 1. Replay the snapshot phase.
        for f in snapshot_vec {
            yield Ok::<_, std::convert::Infallible>(
                SseEvent::default()
                    .id(f.seq.to_string())
                    .data(serde_json::to_string(&f).unwrap_or_default()),
            );
        }
        // 2. Live-tail the hub until the Terminal frame.
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    // Dedup: drop frames already covered by the snapshot cut/cursor.
                    if frame.seq <= dedup_floor {
                        continue;
                    }
                    let is_terminal = matches!(
                        &frame.kind,
                        crate::reattach::FrameKind::Terminal { .. }
                    );
                    yield Ok(
                        SseEvent::default()
                            .id(frame.seq.to_string())
                            .data(serde_json::to_string(&frame).unwrap_or_default()),
                    );
                    // END the stream after emitting a Terminal-kind frame.
                    if is_terminal {
                        break;
                    }
                }
                // I7: the receiver lagged (the runner outran this slow consumer) →
                // emit ONE retryable error event then close. The client reconnects
                // with its Last-Event-ID cursor and re-snapshots — lossless because
                // the snapshot is durable.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    yield Ok(
                        SseEvent::default()
                            .event("error")
                            .data(
                                serde_json::json!({"retryable": true, "reason": "lagged"})
                                    .to_string(),
                            ),
                    );
                    break;
                }
                // The hub was dropped (the task finalized and removed the hub) →
                // close. The durable terminal is still readable via a fresh subscribe.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // The working stream is long-lived (the in-flight tail) → keep-alive is
    // appropriate here, unlike the FINITE terminal branch.
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Build the ordered, cursor-filtered `Vec<WorkflowProgressFrame>` for the snapshot phase.
///
/// `cursor`: `None` passes everything including seq 0; `Some(K)` passes only `seq > K`.
/// Frames from `snap.checkpoints` become `NodeFinished` entries; frames from
/// `snap.starts` become `NodeStarted` entries. The result is sorted ascending by seq.
fn snapshot_frames(
    snap: &bridge_core::task_store::TaskProgressSnapshot,
    cursor: Option<i64>,
) -> Vec<crate::reattach::WorkflowProgressFrame> {
    // `pass(seq)` returns true if the frame should be included given the cursor.
    // Absent cursor → include everything (including seq 0). Some(K) → only seq > K.
    let pass = |seq: i64| cursor.is_none_or(|k| seq > k);

    let mut frames = Vec::new();

    // Finished nodes (checkpoints): tuple = (node, output, ok, seq).
    for (node, output, ok, seq) in &snap.checkpoints {
        if pass(*seq) {
            frames.push(crate::reattach::WorkflowProgressFrame {
                v: 1,
                seq: *seq,
                phase: crate::reattach::Phase::Snapshot,
                kind: crate::reattach::FrameKind::NodeFinished {
                    node: node.as_str().to_string(),
                    ok: *ok,
                    output: output.clone(),
                },
            });
        }
    }

    // In-progress nodes (starts): tuple = (node, seq).
    for (node, seq) in &snap.starts {
        if pass(*seq) {
            frames.push(crate::reattach::WorkflowProgressFrame {
                v: 1,
                seq: *seq,
                phase: crate::reattach::Phase::Snapshot,
                kind: crate::reattach::FrameKind::NodeStarted {
                    node: node.as_str().to_string(),
                },
            });
        }
    }

    // Sort ascending by seq so the client sees events in the order they occurred.
    frames.sort_by_key(|f| f.seq);
    frames
}

/// Spawn the local-backend producer for an already-resolved [`LocalDispatch`]: drive
/// the translator on the bound backend and forward each translated event into the
/// mpsc channel. Stops if the receiver is dropped (client disconnect).
///
/// The producer OWNS the dispatch's [`BindingGuard`] (when present — a first message;
/// a follow-up reuses the binding and carries no guard). The guard is moved into the
/// spawned task and dropped on EVERY exit — clean terminal flush, translator-emitted
/// terminal, OR early `return` on receiver-gone — so the binding/lease/stash are
/// evicted on the non-clean disconnect path too, not just on a clean Done/Failed.
fn spawn_local_producer(
    srv: &Arc<InboundServer>,
    routed: RoutedCall,
    dispatch: LocalDispatch,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
) {
    let store = srv.store.clone();
    let policy = srv.policy.clone();
    let task = routed.task;
    let session = dispatch.session.clone();
    let parts = routed.parts;
    let backend = dispatch.backend;
    // Moved into the task: its Drop evicts the binding/lease/stash on ANY exit.
    let guard = dispatch.guard;
    let warm = dispatch.warm_guard;

    tokio::spawn(async move {
        // Hold the guard for the whole producer; dropped on every return path below.
        let _guard = guard;
        let warm = warm;

        let translator = Translator::new();
        let mut events = translator.run(
            backend.as_ref(),
            store.as_ref(),
            policy.as_ref(),
            &task,
            &session,
            parts,
        );
        let mut errored = false;
        // Whether the translator already emitted its own terminal frame (a
        // user-cancelled turn ends with Update::Done{stop_reason:"cancelled"},
        // which the translator maps to a terminal Canceled event). If so we must
        // NOT append a second terminal — we honor the one it sent.
        let mut translator_terminal = false;
        loop {
            // Race the next translator event against caller-disconnect. The
            // `tx.closed()` arm is essential for an IDLE backend stream: a producer
            // parked in `events.next()` (a still-running turn) would otherwise never
            // observe the dropped receiver, and its `_guard` Drop (lease/stash
            // eviction) would never fire. On disconnect we return early → guard drops.
            let ev = tokio::select! {
                maybe = events.next() => match maybe {
                    Some(ev) => ev,
                    None => break,
                },
                _ = tx.closed() => {
                    // Receiver gone (client disconnected) — stop driving; the `_guard`
                    // Drop evicts the binding/lease/stash on this early return.
                    return;
                }
            };
            // Slice 2: usage is telemetry — record it on the warm handle, never forward to SSE.
            if let Ok(e) = &ev {
                if e.kind() == &EventKind::Usage {
                    if let (Some(snap), Some(w)) = (e.usage_snapshot(), warm.as_ref()) {
                        w.sm.record_usage(&w.ctx, w.generation, &w.op, snap.clone())
                            .await;
                    }
                    continue;
                }
            }
            // Track whether the stream ended with an error.
            if ev.is_err() {
                errored = true;
            }
            // Note a translator-emitted terminal (e.g. Canceled) so we don't
            // overwrite it with our default clean-end Completed below.
            if let Ok(e) = &ev {
                if e.kind() == &EventKind::Terminal {
                    translator_terminal = true;
                }
            }
            // If the receiver is gone (client disconnected) stop driving. The
            // `_guard` Drop still evicts the binding/lease/stash on this early return.
            if tx.send(ev).await.is_err() {
                // Receiver gone — skip the terminal frame (client disconnected).
                return;
            }
        }
        // Append exactly one terminal frame after the inner stream ends, UNLESS
        // the translator already sent its own terminal (cancelled turn).
        // A clean stream end -> Completed; an errored stream -> Failed.
        if !translator_terminal {
            let outcome = if errored {
                TaskOutcome::Failed
            } else {
                TaskOutcome::Completed
            };
            let _ = tx.send(Ok(Event::terminal(outcome))).await;
        }
        // `_guard` drops here too (clean exit) → eviction. Channel closes on drop ->
        // SSE stream terminates after the terminal flush.
    });
}

/// Spawn the delegate producer: open the delegation, persist `local->peer` as
/// soon as the peer id is known, feed peer events into the same mpsc->SSE path,
/// and run the CONSOLIDATED cancel supervisor (`select!` over the next peer
/// event, `tx.closed()` for caller-disconnect — works even if the peer stream is
/// IDLE — and an inbound `CancelTask` having latched `cancel_requested`).
fn spawn_delegate_producer(
    srv: &Arc<InboundServer>,
    routed: RoutedCall,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
) {
    let delegation = srv.delegation.clone();
    let store = srv.store.clone();
    let guard = srv.cancelled_peers.clone();
    let local = routed.task;
    let parts = routed.parts;
    let auth = routed.auth;

    tokio::spawn(async move {
        // 1. Open the delegation. On failure, surface a terminal error frame.
        let delegated = match delegation.delegate(&auth, &local, parts).await {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(Err(e)).await;
                // Delegation-open failure: no terminal frame — the error frame is terminal.
                return;
            }
        };
        let mut events = delegated.events;
        let mut peer_watch = delegated.peer_task;

        // 2. Background watcher: persist local->peer once the id appears, and
        //    honor the early-cancel latch (case c) the instant the id is known.
        spawn_peer_persist(
            store.clone(),
            delegation.clone(),
            guard.clone(),
            local.clone(),
            peer_watch.clone(),
        );

        // 3. Consolidated event/cancel loop.
        loop {
            tokio::select! {
                // (i) next peer event -> forward to SSE.
                maybe = events.next() => {
                    match maybe {
                        Some(ev) => {
                            if tx.send(ev).await.is_err() {
                                // Receiver gone mid-send: treat as disconnect.
                                cancel_peer_now(&delegation, &store, &guard, &local, &mut peer_watch).await;
                                // Client disconnected — no terminal frame needed.
                                return;
                            }
                        }
                        None => {
                            // Peer stream terminated normally — append terminal Completed frame.
                            let _ = tx.send(Ok(Event::terminal(TaskOutcome::Completed))).await;
                            return;
                        }
                    }
                }
                // (ii) caller disconnected (works even if the peer stream is IDLE).
                _ = tx.closed() => {
                    cancel_peer_now(&delegation, &store, &guard, &local, &mut peer_watch).await;
                    // Client disconnected — no terminal frame (receiver is gone).
                    return;
                }
                // (iii) an inbound CancelTask latched cancel_requested.
                _ = poll_cancel_requested(store.as_ref(), &local) => {
                    cancel_peer_now(&delegation, &store, &guard, &local, &mut peer_watch).await;
                    // Canceled by inbound request — append terminal Canceled frame.
                    let _ = tx.send(Ok(Event::terminal(TaskOutcome::Canceled))).await;
                    return;
                }
            }
        }
    });
}

/// Watch the delegation's peer-task channel; when it becomes `Some(peer)`,
/// persist `local->peer`. If a cancel was already requested (early-cancel latch),
/// cancel the peer immediately (case c).
fn spawn_peer_persist(
    store: Arc<dyn SessionStore>,
    delegation: Arc<dyn DelegationPort>,
    guard: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    local: TaskId,
    mut peer_watch: tokio::sync::watch::Receiver<Option<PeerTaskId>>,
) {
    tokio::spawn(async move {
        loop {
            // Clone the value out and drop the (non-Send) watch ref before awaiting.
            let current = peer_watch.borrow_and_update().clone();
            if let Some(peer) = current {
                let _ = store.set_peer_task(&local, &peer).await;
                if store.cancel_requested(&local).await.unwrap_or(false)
                    && try_win_peer_cancel(&guard, &local).await
                {
                    let _ = delegation.cancel(&peer).await;
                }
                return;
            }
            // Wait for the next change; if the sender is dropped, give up.
            if peer_watch.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Resolve the peer id (from the watch; if still `None`, briefly await the next
/// change so a just-assigned id is honored) and cancel the peer. The store's
/// `request_cancel` latch is also set so a not-yet-known id is covered when it
/// later appears via `spawn_peer_persist`.
async fn cancel_peer_now(
    delegation: &Arc<dyn DelegationPort>,
    store: &Arc<dyn SessionStore>,
    guard: &Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    local: &TaskId,
    peer_watch: &mut tokio::sync::watch::Receiver<Option<PeerTaskId>>,
) {
    // Latch first so the race (id appears later) is covered by spawn_peer_persist.
    let _ = store.request_cancel(local).await;

    // Clone the value out and drop the (non-Send) watch ref before awaiting.
    let current = peer_watch.borrow().clone();
    if let Some(peer) = current {
        // Single-cancel guard: only POST if we win the race against cancel_task().
        if try_win_peer_cancel(guard, local).await {
            let _ = delegation.cancel(&peer).await;
        }
        return;
    }
    // Peer id not yet known: wait briefly for it to appear, else rely on the latch.
    if peer_watch.changed().await.is_ok() {
        let next = peer_watch.borrow().clone();
        if let Some(peer) = next {
            if try_win_peer_cancel(guard, local).await {
                let _ = delegation.cancel(&peer).await;
            }
        }
    }
}

/// Resolve only once `cancel_requested(local)` is true. Polls the store on a
/// short interval so an inbound `CancelTask` (which latches the flag) wakes the
/// supervisor's `select!`.
async fn poll_cancel_requested(store: &dyn SessionStore, local: &TaskId) {
    loop {
        if store.cancel_requested(local).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Build a `Source` for the local Kiro backend by running the Translator inside an
/// `async_stream::stream!` that owns all the `Arc` clones it needs — no lifetime fight.
fn local_kiro_source(
    label: String,
    backend: Arc<dyn AgentBackend>,
    store: Arc<dyn SessionStore>,
    policy: Arc<dyn PolicyEngine>,
    task: TaskId,
    session: SessionId,
    parts: Vec<Part>,
) -> Source {
    // Build the stream by cloning Arc refs into a `'static + Send` stream.
    let stream: crate::fanout::EventStream = Box::pin(async_stream::stream! {
        let translator = Translator::new();
        let mut events = translator.run(
            backend.as_ref(),
            store.as_ref(),
            policy.as_ref(),
            &task,
            &session,
            parts,
        );
        while let Some(ev) = events.next().await {
            // A fan-out SOURCE must never emit a terminal frame — the fan-out
            // coordinator owns the single terminal decision. The translator may now
            // emit a terminal Canceled on a cancelled local Done (used by the
            // local-only producer); swallow it here so it doesn't leak into the
            // merge as a labeled mid-stream terminal.
            if matches!(&ev, Ok(e) if e.kind() == &EventKind::Terminal || e.kind() == &EventKind::Usage) {
                continue;
            }
            yield ev;
        }
    });
    Source::from_stream(label, stream)
}

/// Spawn the fan-out producer: build a Kiro source and a peer source, then run
/// `fanout::run` which merges them and sends the terminal frame.
fn spawn_fanout_producer(
    srv: &Arc<InboundServer>,
    routed: RoutedCall,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
) {
    let srv = srv.clone();
    let store = srv.store.clone();
    let policy = srv.policy.clone();
    let delegation = srv.delegation.clone();
    let guard = srv.cancelled_peers.clone();
    let local_source_label = srv.local_source_label.clone();
    let agent_id = local_agent_id(&srv, &routed.target);
    let task = routed.task;
    let session = routed.session;
    let parts = routed.parts.clone();
    let auth = routed.auth;
    let overrides = routed.overrides;
    let session_cwd = routed.session_cwd;

    tokio::spawn(async move {
        // Mark this task as a fan-out task so Task 6 (cancel_task) can distinguish
        // it from a plain delegate (which also has a peer id in the store).
        let _ = store.set_fanout(&task).await;

        // 1. Resolve the local agent ONCE and HOLD its (backend, lease) for the
        //    source's lifetime: the SAME instance drives the prompt AND is used for
        //    the cancel, so a cmd-change reload mid-fan-out can't swap the backend
        //    out from under either path. `_lease` is kept alive until the producer
        //    task exits. A resolve/configure failure makes the local source a single
        //    labeled error frame (the coordinator's terminal still covers completion).
        let (kiro_source, kiro_backend, _lease): (Source, Option<Arc<dyn AgentBackend>>, _) =
            match resolve_for_fanout(
                &srv,
                &agent_id,
                &task,
                &session,
                overrides.as_ref(),
                session_cwd,
            )
            .await
            {
                Ok((backend, lease)) => {
                    let src = local_kiro_source(
                        local_source_label,
                        backend.clone(),
                        store.clone(),
                        policy,
                        task.clone(),
                        session.clone(),
                        parts.clone(),
                    );
                    (src, Some(backend), Some(lease))
                }
                Err(e) => (Source::failed(&local_source_label, e), None, None),
            };

        // 2. Build the peer source by opening delegation, keeping its peer-task
        //    watch for the supervisor's latched peer cancel.
        let (peer_source, peer_watch) = match delegation.delegate(&auth, &task, parts).await {
            Ok(d) => {
                // Keep the peer-task watch for the supervisor's latched peer cancel.
                let watch = d.peer_task.clone();
                let src = Source::from_stream("peer", d.events);
                (src, watch)
            }
            Err(e) => {
                // Delegation startup failed: emit one labeled error frame for the peer,
                // then the coordinator's terminal frame covers completion.
                let (_, dummy_rx) = tokio::sync::watch::channel::<Option<PeerTaskId>>(None);
                let watch = dummy_rx.clone();
                let src = Source::failed("peer", e);
                (src, watch)
            }
        };

        // 3. Cancel plumbing: a watch flag the coordinator observes, and one
        //    "finished" flag per source (index-aligned with the sources vec).
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let kiro_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let peer_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let finished = vec![kiro_done.clone(), peer_done.clone()];

        // 4. Run the coordinator on its own task (it OWNS `tx` and is the sole
        //    sender — we never clone `tx`, so the SSE channel closes the instant
        //    the coordinator ends). It returns a `RunOutcome` telling us whether
        //    the caller disconnected mid-stream.
        let coordinator = tokio::spawn(fanout::run_with_cancel(
            vec![kiro_source, peer_source],
            tx,
            cancel_rx,
            finished,
        ));

        // 4b. Finished-source claimer: when a source's stream ENDS, claim its
        //     per-source guard key so NEITHER the supervisor NOR a racing
        //     `cancel_task()` ever cancels an already-finished source. This is what
        //     makes "a finished source is a cancel no-op" hold across both cancel
        //     paths (the supervisor's flag check covers itself; this claim covers
        //     cancel_task's direct path). It exits once both keys are claimed.
        spawn_finished_claimer(
            guard.clone(),
            task.clone(),
            kiro_done.clone(),
            peer_done.clone(),
        );

        // 5. Supervisor: race the coordinator finishing against an inbound
        //    CancelTask (the `request_cancel` latch). On a CancelTask we cancel
        //    BOTH sources (Kiro immediately by its known session; peer latched via
        //    its watch), each guarded + finished-aware, then flip the cancel flag.
        //    If the coordinator finishes FIRST with `Disconnected` (caller dropped
        //    the SSE receiver mid-stream) we cancel any surviving sources too.
        let mut peer_watch = peer_watch;
        tokio::select! {
            // (i) coordinator ended on its own. If it was a mid-stream disconnect,
            //     cancel any surviving sources (finished ones are a no-op).
            joined = coordinator => {
                if matches!(joined, Ok(fanout::RunOutcome::Disconnected)) {
                    cancel_fanout_sources(
                        kiro_backend.as_ref(), &delegation, &store, &guard, &task,
                        &session, &mut peer_watch, &kiro_done, &peer_done,
                    )
                    .await;
                }
            }
            // (ii) an inbound CancelTask latched cancel_requested.
            _ = poll_cancel_requested(store.as_ref(), &task) => {
                cancel_fanout_sources(
                    kiro_backend.as_ref(), &delegation, &store, &guard, &task,
                    &session, &mut peer_watch, &kiro_done, &peer_done,
                )
                .await;
                let _ = cancel_tx.send(true);
            }
        }
    });
}

/// SSE sink: forwards workflow events into the mpsc->SSE channel.
struct SseSink {
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
}

#[async_trait::async_trait]
impl crate::workflow_sink::WorkflowSink for SseSink {
    async fn node_started(&mut self, node: &str) -> Result<(), BridgeError> {
        let _ = self
            .tx
            .send(Ok(Event::status(format!("node {node} started"))))
            .await;
        Ok(())
    }
    async fn node_finished(
        &mut self,
        node: &str,
        ok: bool,
        _output: &str,
    ) -> Result<(), BridgeError> {
        let _ = self
            .tx
            .send(Ok(Event::status(format!(
                "node {node} {}",
                if ok { "ok" } else { "failed" }
            ))))
            .await;
        Ok(())
    }
    async fn terminal(
        &mut self,
        outcome: bridge_workflow::executor::WorkflowOutcome,
        output: String,
    ) -> Result<(), BridgeError> {
        use bridge_workflow::executor::WorkflowOutcome;
        let _ = self.tx.send(Ok(Event::artifact(output))).await;
        let to = match outcome {
            WorkflowOutcome::Completed => TaskOutcome::Completed,
            WorkflowOutcome::Failed => TaskOutcome::Failed,
            WorkflowOutcome::Canceled => TaskOutcome::Canceled,
        };
        let _ = self.tx.send(Ok(Event::terminal(to))).await;
        Ok(())
    }
    async fn error(&mut self, err: BridgeError) -> Result<(), BridgeError> {
        let _ = self.tx.send(Err(err)).await;
        Ok(())
    }
}

/// Spawn the workflow producer (W1): run the routed workflow graph over the
/// executor and forward its events into the same mpsc->SSE path. Each
/// `WorkflowEvent::Node{Started,Finished}` becomes a Status frame; the
/// `Terminal` becomes an Artifact (the terminal node's output) followed by a
/// terminal frame mapped from the workflow outcome. A per-task cancellation
/// token is registered in `workflow_cancels` for the run's duration so
/// `cancel_task` can preempt it, and removed on exit.
fn spawn_workflow_producer(
    srv: &Arc<InboundServer>,
    routed: RoutedCall,
    wf_id: bridge_core::ids::WorkflowId,
    tx: tokio::sync::mpsc::Sender<Result<Event, BridgeError>>,
) {
    let srv = srv.clone();
    let task = routed.task;
    let parts = routed.parts.clone();
    tokio::spawn(async move {
        // Resolve the executor + graph; absent either → fail the task with a
        // terminal Failed frame (no executor wired, or an unknown workflow id).
        let (executor, graph) = match (&srv.executor, srv.workflows.get(&wf_id)) {
            (Some(e), Some(g)) => (e.clone(), g.clone()),
            _ => {
                let _ = tx.send(Ok(Event::terminal(TaskOutcome::Failed))).await;
                return;
            }
        };
        // The workflow input is the concatenation of the request's text parts.
        let input: String = parts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // Register the cancel token before driving the stream so an inbound
        // CancelTask can never race the bind window.
        let token = tokio_util::sync::CancellationToken::new();
        srv.workflow_cancels
            .lock()
            .await
            .insert(task.clone(), token.clone());
        let wf_ctx = bridge_workflow::executor::WorkflowRunContext {
            session_cwd: routed.session_cwd.clone(),
        };
        let stream =
            executor.run_with_context(graph, input, task.as_str().to_string(), token, wf_ctx);
        let mut sink = SseSink { tx: tx.clone() };
        // SseSink never errors (sends are best-effort); on a hypothetical error
        // treat it as no-terminal so the existing no-terminal fallback fires.
        let terminal_seen = crate::workflow_sink::drain_workflow(stream, &mut sink)
            .await
            .unwrap_or(false);
        // The executor always emits a Terminal, but guard against an early stream
        // end (e.g. a dropped receiver) so the SSE side always sees a terminal.
        if !terminal_seen {
            let _ = tx.send(Ok(Event::terminal(TaskOutcome::Failed))).await;
        }
        srv.workflow_cancels.lock().await.remove(&task);
    });
}

/// Mint a fresh unique task id for a detached submit (SDK UUIDv7). NOT
/// `task_id_from_params`, which returns the fixed `"task-1"` stub.
fn new_detached_task_id() -> TaskId {
    TaskId::parse(a2a::new_task_id()).expect("new_task_id is non-empty")
}

/// Spawn the finalizer-guarded background runner for a detached workflow. Returns
/// the JoinHandle so callers/tests can await completion. The caller MUST have
/// already `create`d the Working row and registered the token in `workflow_cancels`.
///
/// The caller supplies the already-resolved `graph` (fresh submit: resolved from
/// `srv.workflows` at submit time; boot resume: deserialized from the stored spec),
/// the `input` string (pre-joined text), the `run_id` (fresh submit: task id; boot
/// resume: `"{task}-resume-{n}"`), and a `seed` of already-completed node outputs
/// (fresh submit: empty; boot resume: checkpoints from the store). With an empty
/// seed, `run_from` is behaviorally identical to `run`.
/// Finalize a detached task through the SEQUENCED store path, optionally publishing a
/// `Terminal` frame to the task's progress hub, and ALWAYS removing the hub from
/// `progress_hubs` (so the in-memory hub never leaks). Used by every detached terminal
/// transition EXCEPT the runner's `Ok(true)` happy path (where the `DetachedProgressSink`
/// already wrote+published the sequenced terminal — see `spawn_detached_workflow`).
///
/// Because the terminal is written via `set_terminal_sequenced`, `terminal_seq` is never
/// left NULL on any detached path (the reattach snapshot requirement). On a path where no
/// hub was ever inserted (e.g. the pre-spawn unknown-workflow reject), pass `hub: None`.
pub(crate) async fn finalize_detached(
    store: &Arc<dyn bridge_core::task_store::TaskStore>,
    progress_hubs: &Arc<tokio::sync::Mutex<HashMap<TaskId, Arc<crate::reattach::TaskProgressHub>>>>,
    task: &TaskId,
    status: bridge_core::task_store::TaskRecordStatus,
    result: Option<&str>,
    error: Option<&str>,
    hub: Option<&Arc<crate::reattach::TaskProgressHub>>,
) -> Result<(), BridgeError> {
    use bridge_core::task_store::TaskRecordStatus;
    // Durable-first: write the sequenced terminal. Capture the result instead of
    // early-returning so the hub is removed REGARDLESS of write success (I-1) — a
    // write-Err must NEVER leak the in-memory hub. The durable Working-row gap on a
    // write-Err is the pre-existing W3b §8 gap, backstopped by the boot sweep.
    let write = store
        .set_terminal_sequenced(task, status, result, error, crate::workflow_sink::now_ms())
        .await;
    if let (Ok(seq), Some(hub)) = (&write, hub) {
        // Publish the Terminal frame ONLY on a committed seq (durable-first): on a
        // write-Err there is no seq to publish on.
        // Interrupted has no WorkflowOutcome analogue; the closest wire terminal is Failed.
        let outcome = match status {
            TaskRecordStatus::Completed => crate::reattach::TerminalOutcome::Completed,
            TaskRecordStatus::Canceled => crate::reattach::TerminalOutcome::Canceled,
            TaskRecordStatus::Failed
            | TaskRecordStatus::Interrupted
            | TaskRecordStatus::Working => crate::reattach::TerminalOutcome::Failed,
        };
        let output = result.or(error).unwrap_or("").to_string();
        hub.publish(crate::reattach::WorkflowProgressFrame {
            v: 1,
            seq: *seq,
            phase: crate::reattach::Phase::Live,
            kind: crate::reattach::FrameKind::Terminal { outcome, output },
        });
    }
    // ALWAYS remove the hub, even on write error (I-1: the hub never leaks).
    progress_hubs.lock().await.remove(task);
    write.map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn spawn_detached_workflow(
    srv: &Arc<InboundServer>,
    task: TaskId,
    input: String,
    graph: std::sync::Arc<bridge_workflow::graph::WorkflowGraph>,
    run_id: String,
    token: tokio_util::sync::CancellationToken,
    seed: std::collections::HashMap<String, (String, bool)>,
    ctx: bridge_workflow::executor::WorkflowRunContext,
    hub: Arc<crate::reattach::TaskProgressHub>,
) -> tokio::task::JoinHandle<()> {
    let srv = srv.clone();
    tokio::spawn(async move {
        let mut fin = crate::workflow_sink::Finalizer {
            store: srv.task_store.clone(),
            task: task.clone(),
            cancels: srv.workflow_cancels.clone(),
            progress_hubs: srv.progress_hubs.clone(),
            hub: hub.clone(),
            done: false,
        };
        let executor = match &srv.executor {
            Some(e) => e.clone(),
            None => {
                // No executor wired: finalize Failed via the sequenced path (the hub was
                // inserted before spawn, so publish + clean it up).
                let _ = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("no executor wired"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                srv.workflow_cancels.lock().await.remove(&task);
                return;
            }
        };
        let stream = executor.run_from_with_context(graph, input, run_id, token, seed, ctx);
        // The DetachedProgressSink OWNS the sequenced terminal write: on a clean drain it
        // has already written `set_terminal_sequenced` AND published the Terminal frame.
        let mut sink = crate::workflow_sink::DetachedProgressSink::new(
            srv.task_store.clone(),
            task.clone(),
            hub.clone(),
        );
        match crate::workflow_sink::drain_workflow(stream, &mut sink).await {
            Ok(true) => {
                // Sink already committed+published the terminal. Do NOT write it again.
                // M1: flip the finalizer done flag BEFORE the hub-removal await so the
                // Finalizer's Drop can never clobber the committed terminal during the
                // .await suspension point below.
                fin.done = true;
                srv.progress_hubs.lock().await.remove(&task);
                srv.workflow_cancels.lock().await.remove(&task);
            }
            Ok(false) => {
                // Drain ended with no terminal: finalize Failed via the sequenced path
                // (also removes the hub).
                let _ = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("workflow ended without terminal"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                srv.workflow_cancels.lock().await.remove(&task);
            }
            Err(e) => {
                tracing::warn!(task = task.as_str(), error = ?e, "drain_workflow sink error; marking task Failed");
                let _ = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("checkpoint write failed"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                srv.workflow_cancels.lock().await.remove(&task);
            }
        }
    })
}

/// Test-only seam: spawn the runner with a fresh token and an empty seed.
/// Resolves the graph from the server's `workflows` map (the graph must already be
/// registered). `run_id` is set to the task id (matching the fresh-submit path).
#[doc(hidden)]
pub async fn spawn_detached_workflow_for_test(
    srv: &Arc<InboundServer>,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
) -> tokio::task::JoinHandle<()> {
    let token = tokio_util::sync::CancellationToken::new();
    let graph = srv
        .workflows
        .get(&wf_id)
        .cloned()
        .expect("workflow must be registered in the test server");
    let input = text_parts.join("\n");
    let run_id = task.as_str().to_string();
    // Mirror the real callers: insert the hub BEFORE spawning.
    let hub = Arc::new(crate::reattach::TaskProgressHub::new());
    srv.progress_hubs
        .lock()
        .await
        .insert(task.clone(), hub.clone());
    spawn_detached_workflow(
        srv,
        task,
        input,
        graph,
        run_id,
        token,
        std::collections::HashMap::new(),
        bridge_workflow::executor::WorkflowRunContext::default(),
        hub,
    )
}

/// Test-only seam that takes an explicit token (so a cancel test can fire it).
/// Resolves the graph from the server's `workflows` map (the graph must already be
/// registered). `run_id` is set to the task id (matching the fresh-submit path).
#[doc(hidden)]
pub async fn spawn_detached_workflow_with_token_for_test(
    srv: &Arc<InboundServer>,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
    token: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    let graph = srv
        .workflows
        .get(&wf_id)
        .cloned()
        .expect("workflow must be registered in the test server");
    let input = text_parts.join("\n");
    let run_id = task.as_str().to_string();
    // Mirror the real callers: insert the hub BEFORE spawning.
    let hub = Arc::new(crate::reattach::TaskProgressHub::new());
    srv.progress_hubs
        .lock()
        .await
        .insert(task.clone(), hub.clone());
    spawn_detached_workflow(
        srv,
        task,
        input,
        graph,
        run_id,
        token,
        std::collections::HashMap::new(),
        bridge_workflow::executor::WorkflowRunContext::default(),
        hub,
    )
}

/// The only snapshot schema version this server can resume. The forward-compat door:
/// a snapshot whose `v` field does not match this const is treated as unreadable and
/// the task is marked `Interrupted` rather than mis-deserialized.
const SUPPORTED_SNAPSHOT_VERSION: u32 = 1;

/// The persisted workflow-spec snapshot envelope (mirrors the `{"v":1,"graph":...}`
/// written at detached-submit time — see the `RouteTarget::Workflow` arm of
/// `unary_message`). The `v` field is the forward-compat door: an unknown version
/// fails to match `SUPPORTED_SNAPSHOT_VERSION` in the resume routine and the task is
/// marked `Interrupted` rather than mis-deserialized. `graph` deserializes into the
/// exact `WorkflowGraph` that was running at submit time (NOT the live on-disk spec,
/// which may have changed since).
#[derive(serde::Deserialize)]
struct WorkflowSpecEnvelope {
    v: u32,
    graph: bridge_workflow::graph::WorkflowGraph,
}

/// Boot-time crash-resume scan (W3b Task 10a). Replaces the W3a behavior of sweeping
/// every `Working` row to `Interrupted`: instead, for each `Working` task this either
/// (a) **short-circuits** it to terminal if its terminal node already has a checkpoint
/// (the W3a §8 write-failure gap — the terminal output was produced but the row wasn't
/// flipped), (b) **resumes** it by re-running only the un-checkpointed nodes (seeding
/// `run_from` with the stored checkpoints, consuming one resume attempt), or
/// (c) marks it `Interrupted` if it cannot be resumed (no/unreadable snapshot, unknown
/// schema version, or the resume-attempt cap is exhausted — the poison-pill guard).
///
/// Resilience policy: a top-level `working_tasks()` failure logs and returns (the boot
/// scan is best-effort — a store that can't be read at boot must not abort `serve`).
/// A per-task store error logs and continues to the NEXT task, so one bad row never
/// aborts the whole scan.
///
/// Detachment: a resumed task is spawned via [`spawn_detached_workflow`] (no JoinHandle
/// is awaited here) and runs in the background, exactly like a fresh detached submit.
/// The cancel token is registered in `workflow_cancels` BEFORE the spawn so a
/// concurrent `tasks/cancel` arriving during resume can find and fire it.
pub async fn resume_working_tasks(srv: &Arc<InboundServer>, cap: u32) {
    use bridge_core::task_store::{ResumeClaim, TaskRecordStatus};

    let working = match srv.task_store.working_tasks().await {
        Ok(w) => w,
        Err(e) => {
            // A store that can't even be scanned at boot is logged and the scan is
            // skipped — `serve` still comes up (best-effort resume).
            tracing::warn!(error = ?e, "resume scan: working_tasks() failed; skipping boot resume");
            return;
        }
    };

    for wt in working {
        let task = wt.id.clone();

        // (1) No snapshot → cannot reconstruct the graph that was running. Interrupt.
        let Some(spec_json) = wt.workflow_spec_json.as_deref() else {
            // Pre-spawn terminal (no hub inserted): finalize via the sequenced path
            // (hub: None) so terminal_seq is never NULL.
            if let Err(e) = finalize_detached(
                &srv.task_store,
                &srv.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: no workflow snapshot"),
                None,
            )
            .await
            {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/no-snapshot) failed");
            } else {
                tracing::info!(
                    task = task.as_str(),
                    "resume scan: interrupted (no workflow snapshot)"
                );
            }
            continue;
        };

        // (2) Parse the envelope. Unparseable JSON, an unknown `v`, or a `graph` that
        //     won't deserialize into a `WorkflowGraph` all mean "not resumable". The
        //     version check is the forward-compat door (unknown version → Interrupted,
        //     never a panic).
        let graph = match serde_json::from_str::<WorkflowSpecEnvelope>(spec_json) {
            Ok(env) if env.v == SUPPORTED_SNAPSHOT_VERSION => env.graph,
            _ => {
                if let Err(e) = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("not resumable: unreadable workflow snapshot"),
                    None,
                )
                .await
                {
                    tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/unreadable) failed");
                } else {
                    tracing::info!(
                        task = task.as_str(),
                        "resume scan: interrupted (unreadable workflow snapshot)"
                    );
                }
                continue;
            }
        };

        // (3) Load checkpoints → seed map keyed by node id: node_id → (output, ok).
        let cps = match srv.task_store.node_checkpoints(&task).await {
            Ok(cps) => cps,
            Err(e) => {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: node_checkpoints() failed; skipping task");
                continue;
            }
        };
        let seed: std::collections::HashMap<String, (String, bool)> = cps
            .iter()
            .map(|(node, output, ok)| (node.as_str().to_string(), (output.clone(), *ok)))
            .collect();

        // (4) Terminal short-circuit: if the graph's terminal node already has a
        //     checkpoint, the workflow had actually FINISHED before the crash (its
        //     terminal output was produced but the row wasn't flipped — the W3a §8
        //     write-failure gap). Finalize DIRECTLY from the checkpoint, with NO
        //     re-run and WITHOUT consuming a resume attempt. Completed carries the
        //     output as `result`; Failed carries it as `error` (mirrors
        //     `finalize_detached` / `DetachedProgressSink::terminal`).
        let terminal_id = match graph.terminal() {
            Some(n) => n.id.as_str().to_string(),
            None => {
                // A snapshot that validate()'d at submit time always has exactly one
                // terminal; a malformed snapshot with no terminal is not resumable.
                if let Err(e) = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("not resumable: workflow snapshot has no terminal node"),
                    None,
                )
                .await
                {
                    tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/no-terminal) failed");
                } else {
                    tracing::info!(
                        task = task.as_str(),
                        "resume scan: interrupted (unreadable workflow snapshot)"
                    );
                }
                continue;
            }
        };
        if let Some((output, ok)) = seed.get(&terminal_id) {
            let (status, result, error) = if *ok {
                (TaskRecordStatus::Completed, Some(output.as_str()), None)
            } else {
                (TaskRecordStatus::Failed, None, Some(output.as_str()))
            };
            // Pre-spawn terminal (no hub inserted): sequenced finalize so terminal_seq
            // is never NULL.
            if let Err(e) = finalize_detached(
                &srv.task_store,
                &srv.progress_hubs,
                &task,
                status,
                result,
                error,
                None,
            )
            .await
            {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(short-circuit) failed");
            } else {
                tracing::info!(task = task.as_str(), status = ?status, "resume scan: short-circuited to terminal");
            }
            continue;
        }

        // (5) Otherwise claim a resume attempt (atomic; increments resume_attempts).
        match srv
            .task_store
            .claim_resume_attempt(&task, cap, crate::workflow_sink::now_ms())
            .await
        {
            Ok(ResumeClaim::Exhausted) => {
                // Poison-pill guard: a task that keeps crashing the server is marked
                // Interrupted after `cap` attempts instead of looping forever.
                if let Err(e) = finalize_detached(
                    &srv.task_store,
                    &srv.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("resume attempt cap exceeded"),
                    None,
                )
                .await
                {
                    tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/cap) failed");
                } else {
                    tracing::info!(
                        task = task.as_str(),
                        "resume scan: interrupted (resume attempt cap exceeded)"
                    );
                }
                continue;
            }
            Ok(ResumeClaim::Resumable { attempt }) => {
                // Re-validate the persisted session_cwd before spawning: never trust
                // the stored string blindly. A corrupt/invalid stored cwd is not
                // resumable — interrupt BEFORE registering the cancel token or spawning
                // so no orphaned token/runner is left behind.
                let ctx = match wt.session_cwd.as_deref() {
                    Some(s) => match bridge_core::SessionCwd::parse(s) {
                        Ok(c) => bridge_workflow::executor::WorkflowRunContext {
                            session_cwd: Some(c),
                        },
                        Err(_) => {
                            let _ = finalize_detached(
                                &srv.task_store,
                                &srv.progress_hubs,
                                &task,
                                TaskRecordStatus::Interrupted,
                                None,
                                Some("not resumable: unreadable session cwd"),
                                None,
                            )
                            .await;
                            tracing::info!(
                                task = task.as_str(),
                                "resume scan: interrupted (unreadable session cwd)"
                            );
                            continue;
                        }
                    },
                    None => bridge_workflow::executor::WorkflowRunContext::default(),
                };
                // Insert the progress hub BEFORE spawning (mirrors the fresh-submit
                // path) so a reattach subscriber can find it.
                let hub = Arc::new(crate::reattach::TaskProgressHub::new());
                srv.progress_hubs
                    .lock()
                    .await
                    .insert(task.clone(), hub.clone());
                // Register a fresh cancel token BEFORE spawning so a concurrent
                // tasks/cancel during resume can find and fire it.
                let token = tokio_util::sync::CancellationToken::new();
                srv.workflow_cancels
                    .lock()
                    .await
                    .insert(task.clone(), token.clone());
                let run_id = format!("{}-resume-{}", task.as_str(), attempt);
                // Detached: the runner re-runs only the un-checkpointed nodes
                // (run_from skips the seeded ones) and writes their checkpoints + the
                // terminal as usual. No JoinHandle is awaited here.
                drop(spawn_detached_workflow(
                    srv,
                    task.clone(),
                    wt.input.clone(),
                    std::sync::Arc::new(graph),
                    run_id.clone(),
                    token,
                    seed,
                    ctx,
                    hub,
                ));
                tracing::info!(task = task.as_str(), attempt, run_id = %run_id, "resume scan: resumed from checkpoints");
            }
            Err(e) => {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: claim_resume_attempt() failed; skipping task");
                continue;
            }
        }
    }
}

/// Background claimer: as each fan-out source's stream ENDS (its `*_done` flag
/// flips true), claim that source's per-source guard key (`"{task}:kiro"` /
/// `"{task}:peer"`). Claiming the key makes any later `try_win_*` for it return
/// `false`, so a finished source is never cancelled — by the supervisor OR by a
/// racing `cancel_task()`. Polls on a short interval (mirrors
/// `poll_cancel_requested`) and exits once both keys are claimed.
fn spawn_finished_claimer(
    guard: Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    task: TaskId,
    kiro_done: Arc<std::sync::atomic::AtomicBool>,
    peer_done: Arc<std::sync::atomic::AtomicBool>,
) {
    tokio::spawn(async move {
        let mut kiro_claimed = false;
        let mut peer_claimed = false;
        loop {
            if !kiro_claimed && kiro_done.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = try_win_cancel_key(&guard, format!("{}:kiro", task.as_str())).await;
                kiro_claimed = true;
            }
            if !peer_claimed && peer_done.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = try_win_cancel_key(&guard, format!("{}:peer", task.as_str())).await;
                peer_claimed = true;
            }
            if kiro_claimed && peer_claimed {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
}

/// Cancel BOTH fan-out sources on a cancel trigger: the Kiro session immediately
/// by its known `session` (never awaiting the peer id) and the peer via its watch
/// (latched if the id is not yet known). Each source is guarded by a per-source
/// key (`"{task}:kiro"` / `"{task}:peer"`) so it is cancelled exactly once across
/// the supervisor and `cancel_task()`, and a source whose stream already FINISHED
/// is a cancel no-op.
#[allow(clippy::too_many_arguments)]
async fn cancel_fanout_sources(
    // The HELD local backend (the same resolved instance that drove the prompt).
    // `None` when the local source failed to resolve — there is then nothing to cancel.
    backend: Option<&Arc<dyn AgentBackend>>,
    delegation: &Arc<dyn DelegationPort>,
    store: &Arc<dyn SessionStore>,
    guard: &Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
    task: &TaskId,
    session: &SessionId,
    peer_watch: &mut tokio::sync::watch::Receiver<Option<PeerTaskId>>,
    kiro_done: &std::sync::atomic::AtomicBool,
    peer_done: &std::sync::atomic::AtomicBool,
) {
    // Latch first so a not-yet-known peer id is covered by spawn-style appliers.
    let _ = store.request_cancel(task).await;

    // Kiro: cancel IMMEDIATELY by its known session, unless its stream finished.
    // Use the HELD instance (never re-resolved) so a cmd-change reload can't cancel
    // a different backend than the one running the prompt.
    if let Some(backend) = backend {
        if !kiro_done.load(std::sync::atomic::Ordering::SeqCst)
            && try_win_cancel_key(guard, format!("{}:kiro", task.as_str())).await
        {
            let _ = backend.cancel(session).await;
        }
    }

    // Peer: cancel via its watch (latched if the id is not yet known), unless its
    // stream finished. Guarded by the per-source key.
    if peer_done.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let current = peer_watch.borrow().clone();
    if let Some(peer) = current {
        if try_win_cancel_key(guard, format!("{}:peer", task.as_str())).await {
            let _ = delegation.cancel(&peer).await;
        }
        return;
    }
    // Peer id not yet known: wait briefly for it, else rely on the request_cancel
    // latch (the unary/streaming peer-id appliers honor it once the id appears).
    if peer_watch.changed().await.is_ok() {
        let next = peer_watch.borrow().clone();
        if let Some(peer) = next {
            if try_win_cancel_key(guard, format!("{}:peer", task.as_str())).await {
                let _ = delegation.cancel(&peer).await;
            }
        }
    }
}

/// Adapt the mpsc receiver into a stream of `Result<SseEvent, Infallible>`.
/// Each translated [`Event`] becomes one SSE frame; backend errors become a
/// single `error` frame so the client sees a terminal signal.
fn sse_event_stream(
    rx: tokio::sync::mpsc::Receiver<Result<Event, BridgeError>>,
    task_id: String,
    context_id: String,
) -> impl Stream<Item = Result<SseEvent, std::convert::Infallible>> {
    tokio_stream::wrappers::ReceiverStream::new(rx).filter_map(move |item| {
        let out: Option<Result<SseEvent, std::convert::Infallible>> = match item {
            Ok(ev) => event_to_sse(&ev, &task_id, &context_id).map(Ok),
            Err(e) => {
                tracing::warn!(error = %e, "workflow stream error");
                Some(Ok(SseEvent::default()
                    .event("error")
                    // Static category to the wire; full reason logged above.
                    .json_data(json!({ "kind": "error", "text": e.client_message() }))
                    .expect("serde_json::Value serializes")))
            }
        };
        std::future::ready(out)
    })
}

/// Unary path: run the same pipeline but collect events into one JSON response.
async fn unary_message(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let routed = match srv.gate(&headers, &params) {
        Ok(r) => r,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    let _ = srv.store.put(&routed.task, &routed.session).await;

    // Fan-out unary: collect all fanout::run events and build an a2a::Task
    // response with both labeled artifacts.
    if let RouteTarget::Fanout = routed.target {
        return unary_fanout_message(srv, id, routed).await;
    }

    // Collect the same event stream the streaming path produces, into one JSON
    // response. Local drives the translator; Delegate drives the delegation.
    let collected: Vec<Result<Event, BridgeError>> = match routed.target {
        RouteTarget::Local(ref agent_id) => {
            // First-message dispatch: resolve the routed agent, apply its effective
            // config, and bind the task (or reuse a follow-up binding). An unknown
            // agent surfaces as a JSON-RPC error. The dispatch's BindingGuard binds
            // for the call's DURATION (so an interleaved cancel finds the binding) and
            // is dropped at the end of this scope → eviction after `collect().await`.
            // A follow-up reuses the binding and carries no guard (no premature evict).
            let op = OperationId::parse(format!("op-{}", routed.task.as_str())).unwrap();
            let dispatch = match warm_local_dispatch(&srv, agent_id, &routed, op).await {
                Some(r) => r,
                None => {
                    resolve_configure_bind(
                        &srv,
                        agent_id,
                        &routed.task,
                        &routed.session,
                        routed.overrides.as_ref(),
                        routed.session_cwd.clone(),
                    )
                    .await
                }
            };
            let dispatch = match dispatch {
                Ok(d) => d,
                Err(e) => return bridge_err_to_jsonrpc(id, &e),
            };
            let _ = srv.store.put(&routed.task, &dispatch.session).await;
            // Held until this arm returns; its Drop evicts the binding/lease/stash
            // after the synchronous collect completes.
            let _guard = dispatch.guard;
            let warm = dispatch.warm_guard;
            let translator = Translator::new();
            let mut events = translator.run(
                dispatch.backend.as_ref(),
                srv.store.as_ref(),
                srv.policy.as_ref(),
                &routed.task,
                &dispatch.session,
                routed.parts,
            );
            let mut collected: Vec<Result<Event, BridgeError>> = Vec::new();
            while let Some(ev) = events.next().await {
                if let Ok(e) = &ev {
                    if e.kind() == &EventKind::Usage {
                        if let (Some(snap), Some(w)) = (e.usage_snapshot(), warm.as_ref()) {
                            w.sm.record_usage(&w.ctx, w.generation, &w.op, snap.clone())
                                .await;
                        }
                        continue; // exclude usage from the unary output
                    }
                }
                collected.push(ev);
            }
            collected
        }
        RouteTarget::Delegate => {
            match srv
                .delegation
                .delegate(&routed.auth, &routed.task, routed.parts)
                .await
            {
                Ok(delegated) => {
                    let mut peer_watch = delegated.peer_task;
                    // Drain the events first: the real client captures the peer id
                    // lazily as frames are consumed, so the watch only becomes
                    // Some(peer) AFTER the stream has been driven. Reading it before
                    // collect (the old behavior) saw None and never persisted the
                    // mapping, leaving the unary-delegated task un-cancellable.
                    let collected: Vec<Result<Event, BridgeError>> =
                        delegated.events.collect().await;

                    // Now persist local->peer. Clone the value out and drop the
                    // (non-Send) watch ref before awaiting.
                    let peer = peer_watch.borrow_and_update().clone();
                    if let Some(peer) = peer {
                        let _ = srv.store.set_peer_task(&routed.task, &peer).await;
                        // Latch-apply: if an inbound CancelTask already requested a
                        // cancel for this task, honor it now — respecting the
                        // single-cancel guard so we don't double-POST.
                        if srv
                            .store
                            .cancel_requested(&routed.task)
                            .await
                            .unwrap_or(false)
                            && try_win_peer_cancel(&srv.cancelled_peers, &routed.task).await
                        {
                            let _ = srv.delegation.cancel(&peer).await;
                        }
                    }
                    collected
                }
                Err(e) => vec![Err(e)],
            }
        }
        // Fanout handled above; this arm is unreachable.
        RouteTarget::Fanout => unreachable!("fanout handled by unary_fanout_message"),
        // Detached submit: mint a unique id, persist Working, register the
        // cancel token, spawn the runner, and return a working Task NOW.
        RouteTarget::Workflow(ref wf_id) => {
            let task = new_detached_task_id();
            let now = crate::workflow_sink::now_ms();
            // Pre-join the input text so the persisted `input` is byte-identical to
            // what the runner receives. Both the record and the spawn use the same value.
            let input: String = routed
                .parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            // Resolve the graph at submit time. If the wf_id is unknown, the graph is
            // None and the record persists without a spec snapshot (the runner will
            // fail when it also can't find the graph — but since the wf_id was
            // validated by the route decision, this is always Some in practice).
            let graph = srv.workflows.get(wf_id).cloned();
            // Snapshot the resolved graph at submit time.
            // The `{"v":1,"graph":...}` envelope lets the boot resume
            // routine detect an unknown schema version (treat as unparseable).
            let workflow_spec_json = graph
                .as_ref()
                .map(|g| serde_json::json!({ "v": 1, "graph": &**g }).to_string());
            let rec = bridge_core::task_store::TaskRecord {
                id: task.clone(),
                workflow: wf_id.as_str().to_string(),
                status: bridge_core::task_store::TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: now,
                updated_ms: now,
                input: input.clone(),
                workflow_spec_json,
                resume_attempts: 0,
                session_cwd: routed.session_cwd.as_ref().map(|c| c.as_str().to_string()),
            };
            if srv.task_store.create(&rec).await.is_err() {
                return bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure);
            }
            // If the graph is not registered (should not happen for a routed workflow,
            // but guard defensively), mark the task Failed and return early.
            let graph = match graph {
                Some(g) => g,
                None => {
                    // Unknown workflow, pre-spawn: no hub was inserted (hub: None). Finalize
                    // Failed via the sequenced path so terminal_seq is never NULL.
                    let _ = finalize_detached(
                        &srv.task_store,
                        &srv.progress_hubs,
                        &task,
                        bridge_core::task_store::TaskRecordStatus::Failed,
                        None,
                        Some("unknown workflow"),
                        None,
                    )
                    .await;
                    return bridge_err_to_jsonrpc(id, &BridgeError::StoreFailure);
                }
            };
            // Insert the progress hub BEFORE spawning so a reattach subscriber arriving
            // immediately after this submit can find it.
            let hub = Arc::new(crate::reattach::TaskProgressHub::new());
            srv.progress_hubs
                .lock()
                .await
                .insert(task.clone(), hub.clone());
            let token = tokio_util::sync::CancellationToken::new();
            srv.workflow_cancels
                .lock()
                .await
                .insert(task.clone(), token.clone());
            drop(spawn_detached_workflow(
                &srv,
                task.clone(),
                input,
                graph,
                task.as_str().to_string(),
                token,
                std::collections::HashMap::new(),
                bridge_workflow::executor::WorkflowRunContext {
                    session_cwd: routed.session_cwd.clone(),
                },
                hub,
            ));
            let working = a2a::Task {
                id: task.as_str().to_owned(),
                context_id: task.as_str().to_owned(),
                status: a2a::TaskStatus {
                    state: a2a::TaskState::Working,
                    message: None,
                    timestamp: None,
                },
                artifacts: None,
                history: None,
                metadata: None,
            };
            return jsonrpc_ok(id, json!({ "task": working }));
        }
    };

    // Surface a terminal error if the pipeline failed/suspended.
    if let Some(Err(e)) = collected.iter().find(|r| r.is_err()) {
        return bridge_err_to_jsonrpc(id, e);
    }
    let events: Vec<Event> = collected.into_iter().filter_map(|r| r.ok()).collect();
    let artifact_text = events
        .iter()
        .rev()
        .find(|e| e.kind() == &EventKind::Artifact)
        .map(|e| e.text().to_string())
        .unwrap_or_default();
    let status_chunks: Vec<&str> = events
        .iter()
        .filter(|e| e.kind() == &EventKind::Status)
        .map(|e| e.text())
        .collect();

    // The terminal state is Completed unless the translator emitted a terminal
    // outcome (a cancelled local turn -> Canceled); a backend error is handled
    // above as a JSON-RPC error.
    let state = match events.iter().rev().find_map(|e| e.outcome()) {
        Some(TaskOutcome::Canceled) => "TASK_STATE_CANCELED",
        Some(TaskOutcome::Failed) => "TASK_STATE_FAILED",
        _ => "TASK_STATE_COMPLETED",
    };
    let result = json!({
        "task": { "id": routed.task.as_str(), "state": state },
        "artifact": { "text": artifact_text },
        "status": status_chunks,
    });
    jsonrpc_ok(id, result)
}

/// Unary fan-out path: run both sources concurrently via `fanout::run`, collect
/// all events, then build an `a2a::Task` response with one `Artifact` per source.
async fn unary_fanout_message(srv: Arc<InboundServer>, id: Value, routed: RoutedCall) -> Response {
    // Mark the task as fanout so Task 6 (cancel_task) can distinguish it.
    let _ = srv.store.set_fanout(&routed.task).await;

    // Resolve the local agent ONCE, apply its effective config, and HOLD its lease
    // (`_lease`) for the unary collect's lifetime so the slot can't retire mid-run.
    // A resolve/configure failure makes the local source a single labeled error frame.
    let agent_id = local_agent_id(&srv, &routed.target);
    let (kiro_source, _lease) = match resolve_for_fanout(
        &srv,
        &agent_id,
        &routed.task,
        &routed.session,
        routed.overrides.as_ref(),
        routed.session_cwd.clone(),
    )
    .await
    {
        Ok((backend, lease)) => {
            let src = local_kiro_source(
                srv.local_source_label.clone(),
                backend,
                srv.store.clone(),
                srv.policy.clone(),
                routed.task.clone(),
                routed.session.clone(),
                routed.parts.clone(),
            );
            (src, Some(lease))
        }
        Err(e) => (Source::failed(&srv.local_source_label, e), None),
    };

    // Build the peer source by opening delegation.
    let peer_source = match srv
        .delegation
        .delegate(&routed.auth, &routed.task, routed.parts)
        .await
    {
        Ok(d) => Source::from_stream("peer", d.events),
        Err(e) => Source::failed("peer", e),
    };

    // Drain all fanout events synchronously via an mpsc channel.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Event, BridgeError>>(64);
    let run_handle = tokio::spawn(async move {
        fanout::run(vec![kiro_source, peer_source], tx).await;
    });

    let mut all_events: Vec<Event> = Vec::new();
    let mut terminal_outcome = TaskOutcome::Completed;
    while let Some(item) = rx.recv().await {
        match item {
            Ok(ev) => {
                if ev.kind() == &EventKind::Terminal {
                    if let Some(o) = ev.outcome() {
                        terminal_outcome = o;
                    }
                } else {
                    all_events.push(ev);
                }
            }
            Err(e) => return bridge_err_to_jsonrpc(id, &e),
        }
    }
    let _ = run_handle.await;

    // Build one a2a::Artifact per source from the collected artifact events.
    let artifacts: Vec<a2a::Artifact> = all_events
        .iter()
        .filter(|e| e.kind() == &EventKind::Artifact)
        .map(|e| {
            let name = e.source().map(|s| s.to_owned());
            a2a::Artifact {
                artifact_id: a2a::new_artifact_id(),
                name,
                description: None,
                parts: vec![a2a::Part::text(e.text())],
                metadata: None,
                extensions: None,
            }
        })
        .collect();

    let state = match terminal_outcome {
        TaskOutcome::Completed => a2a::TaskState::Completed,
        TaskOutcome::Failed => a2a::TaskState::Failed,
        TaskOutcome::Canceled => a2a::TaskState::Canceled,
    };

    let task = a2a::Task {
        id: routed.task.as_str().to_owned(),
        context_id: routed.task.as_str().to_owned(),
        status: a2a::TaskStatus {
            state,
            message: None,
            timestamp: None,
        },
        artifacts: if artifacts.is_empty() {
            None
        } else {
            Some(artifacts)
        },
        history: None,
        metadata: None,
    };

    jsonrpc_ok(
        id,
        serde_json::to_value(&task).expect("a2a::Task serializes"),
    )
}

/// `CancelTask` -> propagate cancel to the backend for the task's session.
async fn cancel_task(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    // Cancel is still gated (auth + version) but does not run the translator.
    let token = bearer_token(&headers);
    let inbound = match token {
        Some(t) => InboundRequest::with_token(&t),
        None => InboundRequest::anon(),
    };
    if let Err(e) = srv.auth.authorize(&inbound) {
        return bridge_err_to_jsonrpc(id, &e);
    }

    let task = match task_id_from_params(&params) {
        Ok(t) => t,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };

    // Always latch the early-cancel flag: this covers the race where the peer id
    // is not yet known (the streaming supervisor's select! / peer-persist watcher
    // will apply the cancel once the id appears) and signals an in-flight stream.
    let _ = srv.store.request_cancel(&task).await;

    // Durable detached task? Consult the store first (it owns the truth).
    if let Ok(Some(rec)) = srv.task_store.get(&task).await {
        use bridge_core::task_store::TaskRecordStatus;
        if rec.status.is_terminal() {
            // Already finished — return its true state; do NOT re-cancel / touch a backend.
            let wire = match rec.status {
                TaskRecordStatus::Completed => "TASK_STATE_COMPLETED",
                TaskRecordStatus::Canceled => "TASK_STATE_CANCELED",
                _ => "TASK_STATE_FAILED", // Failed | Interrupted
            };
            return jsonrpc_ok(
                id,
                json!({ "task": { "id": task.as_str(), "state": wire } }),
            );
        }
        // Working: fire the token if a live runner owns it (the runner observes the
        // token and writes Canceled). Scope the guard so it is NOT held across an await.
        let fired = {
            let guard = srv.workflow_cancels.lock().await;
            match guard.get(&task) {
                Some(tok) => {
                    tok.cancel();
                    true
                }
                None => false,
            }
        };
        if fired {
            return jsonrpc_ok(
                id,
                json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
            );
        }
        // No live token. The runner removes its token only AFTER writing the terminal
        // row, so "no token" usually means it already finished — flip ONLY if still
        // Working (atomic guard against clobbering that just-written terminal).
        let flipped = srv
            .task_store
            .cancel_if_working(&task, crate::workflow_sink::now_ms())
            .await
            .unwrap_or(false);
        if flipped {
            return jsonrpc_ok(
                id,
                json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
            );
        }
        // The runner finished between our read and here — report its true terminal state.
        if let Ok(Some(rec2)) = srv.task_store.get(&task).await {
            let wire = match rec2.status {
                TaskRecordStatus::Completed => "TASK_STATE_COMPLETED",
                TaskRecordStatus::Canceled => "TASK_STATE_CANCELED",
                _ => "TASK_STATE_FAILED",
            };
            return jsonrpc_ok(
                id,
                json!({ "task": { "id": task.as_str(), "state": wire } }),
            );
        }
        return jsonrpc_ok(
            id,
            json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
        );
    }

    // Workflow cancel: if this task is an in-flight workflow run, cancel its token
    // (the producer's executor stream observes the token and ends Canceled) and
    // return the same CANCELED response the other cancel arms return. Checked
    // before the is_fanout branch — a workflow task is neither fan-out nor delegate.
    if let Some(tok) = srv.workflow_cancels.lock().await.get(&task) {
        tok.cancel();
        return jsonrpc_ok(
            id,
            json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
        );
    }

    // Decide cancel-both (fan-out) vs peer-only (plain delegate) vs local-only.
    // Cx1: a fan-out task has BOTH a Kiro session AND a peer, so the peer-only
    // branch (used for plain delegate) would LOSE the Kiro cancel — branch on
    // is_fanout to cancel both, each guarded by its per-source key.
    if srv.store.is_fanout(&task).await.unwrap_or(false) {
        // Fan-out: cancel the Kiro session immediately by its known id (never
        // awaiting the peer), AND the peer (latched if not yet known). The
        // supervisor (if still alive) races on the same per-source keys, so each
        // source is cancelled exactly once across both paths.
        let session = match srv.store.session_for(&task).await {
            Ok(Some(s)) => s,
            _ => SessionId::parse(format!("session-{}", task.as_str()))
                .unwrap_or_else(|_| SessionId::parse("session-default").unwrap()),
        };
        // Attempt BOTH cancels regardless of either's result so a failing Kiro
        // cancel never orphans the peer's upstream task (and vice-versa). Each is
        // still guarded by its per-source key, so exactly-once holds across this
        // path and the supervisor. We collect the first error (if any) and return
        // it only AFTER both cancels have been attempted.
        let mut first_err: Option<BridgeError> = None;
        if try_win_cancel_key(&srv.cancelled_peers, format!("{}:kiro", task.as_str())).await {
            // Prefer the task's bound instance; fall back to the default agent if no
            // binding exists yet (binding-or-fallback; T11 makes this binding-only).
            match cancel_backend_for(&srv, &task).await {
                Ok(backend) => {
                    if let Err(e) = backend.cancel(&session).await {
                        first_err.get_or_insert(e);
                    }
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        if let Ok(Some(peer)) = srv.store.peer_task_for(&task).await {
            if try_win_cancel_key(&srv.cancelled_peers, format!("{}:peer", task.as_str())).await {
                if let Err(e) = srv.delegation.cancel(&peer).await {
                    first_err.get_or_insert(e);
                }
            }
        }
        // If the peer id is not yet known, the request_cancel latch (set above)
        // plus the supervisor's peer-watch applier cancel it once it appears.
        if let Some(e) = first_err {
            return bridge_err_to_jsonrpc(id, &e);
        }
    } else {
        // S2b: if the task is delegated, cancel the peer directly. This path covers
        // the case where the stream/supervisor has already ended; the single-cancel
        // guard ensures we don't double-POST when the supervisor is still alive and
        // its poll_cancel_requested arm (woken by the request_cancel latch above)
        // would otherwise cancel the same peer.
        match srv.store.peer_task_for(&task).await {
            Ok(Some(peer)) => {
                if try_win_peer_cancel(&srv.cancelled_peers, &task).await {
                    if let Err(e) = srv.delegation.cancel(&peer).await {
                        return bridge_err_to_jsonrpc(id, &e);
                    }
                }
            }
            _ => {
                // Local task: cancel the backend for the task's session. Prefer the
                // task's bound instance; fall back to the default agent if no binding
                // exists yet (binding-or-fallback; T11 makes this binding-only).
                let session = match srv.store.session_for(&task).await {
                    Ok(Some(s)) => s,
                    _ => SessionId::parse(format!("session-{}", task.as_str()))
                        .unwrap_or_else(|_| SessionId::parse("session-default").unwrap()),
                };
                let backend = match cancel_backend_for(&srv, &task).await {
                    Ok(b) => b,
                    Err(e) => return bridge_err_to_jsonrpc(id, &e),
                };
                if let Err(e) = backend.cancel(&session).await {
                    return bridge_err_to_jsonrpc(id, &e);
                }
            }
        }
    }
    jsonrpc_ok(
        id,
        json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
    )
}

fn authorize_headers(srv: &InboundServer, headers: &HeaderMap) -> Result<(), BridgeError> {
    let inbound = match bearer_token(headers) {
        Some(t) => InboundRequest::with_token(&t),
        None => InboundRequest::anon(),
    };
    srv.auth.authorize(&inbound).map(|_| ())
}

fn context_id_arg(params: &Value) -> Result<ContextId, BridgeError> {
    params
        .get("contextId")
        .and_then(|v| v.as_str())
        .ok_or(BridgeError::InvalidRequest { field: "contextId" })
        .and_then(ContextId::parse)
}

async fn session_status(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) {
        return bridge_err_to_jsonrpc(id, &e);
    }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) {
        Ok(c) => c,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    match sm.status(&ctx).await {
        Some(s) => jsonrpc_ok(
            id,
            json!({
                "contextId": ctx.as_str(),
                "state": s.state,
                "agent": s.agent,
                "generation": s.generation,
                "idleAgeMs": s.idle_age_ms,
                "capabilities": {
                    "loadSession": s.capabilities.load_session,
                    "resume": s.capabilities.resume,
                    "close": s.capabilities.close,
                    "list": s.capabilities.list,
                    "delete": s.capabilities.delete,
                },
                "usage": {
                    "used": s.usage.used,
                    "size": s.usage.size,
                    "windowFraction": s.window_fraction(),
                    "overThreshold": s.over_threshold,
                    "cost": s.usage.cost.as_ref().map(|c| serde_json::json!({
                        "amount": c.amount, "currency": c.currency
                    })),
                    "atMs": s.usage.at_ms,
                },
            }),
        ),
        None => bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound),
    }
}

async fn session_release(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) {
        return bridge_err_to_jsonrpc(id, &e);
    }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) {
        Ok(c) => c,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    sm.release(&ctx).await;
    jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "released": true }))
}

async fn session_clear(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) {
        return bridge_err_to_jsonrpc(id, &e);
    }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) {
        Ok(c) => c,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    let force = params
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    match sm
        .reset_session(&ctx, crate::session_manager::ResetOpts { force })
        .await
    {
        Ok(crate::session_manager::ResetOutcome::Cleared { generation }) => jsonrpc_ok(
            id,
            json!({ "contextId": ctx.as_str(), "cleared": true, "generation": generation }),
        ),
        Ok(crate::session_manager::ResetOutcome::NotFound) => {
            bridge_err_to_jsonrpc(id, &BridgeError::SessionNotFound)
        }
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}

async fn session_cancel(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    if let Err(e) = authorize_headers(&srv, &headers) {
        return bridge_err_to_jsonrpc(id, &e);
    }
    let Some(sm) = srv.session_manager.clone() else {
        return jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "no session manager");
    };
    let ctx = match context_id_arg(&params) {
        Ok(c) => c,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    match sm.cancel(&ctx).await {
        Ok(()) => jsonrpc_ok(id, json!({ "contextId": ctx.as_str(), "canceled": true })),
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}

/// `GetTask` -> return the task's last-known state (v1 stub from the store).
async fn get_task(
    srv: Arc<InboundServer>,
    _headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let task = match task_id_from_params(&params) {
        Ok(t) => t,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    // Durable task row first (detached workflows).
    match srv.task_store.get(&task).await {
        Ok(Some(rec)) => {
            let (state, artifacts) = task_record_to_a2a(&rec);
            let t = a2a::Task {
                id: rec.id.as_str().to_owned(),
                context_id: rec.id.as_str().to_owned(),
                status: a2a::TaskStatus {
                    state,
                    message: None,
                    timestamp: None,
                },
                artifacts,
                history: None,
                metadata: None,
            };
            return jsonrpc_ok(id, json!({ "task": t }));
        }
        Ok(None) => {} // not a detached task — fall through to the heuristic
        Err(e) => {
            // A store read failure degrades to the heuristic; surface it so a
            // persistent failure isn't silently reported as SUBMITTED.
            tracing::warn!(task = task.as_str(), error = ?e, "task_store.get failed in get_task");
        }
    }
    // Fallback: session-mapping heuristic (non-workflow tasks; unchanged).
    let known = matches!(srv.store.session_for(&task).await, Ok(Some(_)));
    let state = if known {
        "TASK_STATE_WORKING"
    } else {
        "TASK_STATE_SUBMITTED"
    };
    jsonrpc_ok(
        id,
        json!({ "task": { "id": task.as_str(), "state": state } }),
    )
}

async fn list_tasks(
    srv: Arc<InboundServer>,
    _headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
    match srv.task_store.list(limit).await {
        Ok(recs) => {
            let tasks: Vec<Value> = recs
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id.as_str(),
                        "workflow": r.workflow,
                        "state": r.status.as_str(),
                        "updated_ms": r.updated_ms,
                    })
                })
                .collect();
            jsonrpc_ok(id, json!({ "tasks": tasks }))
        }
        Err(e) => bridge_err_to_jsonrpc(id, &e),
    }
}

/// Map a durable `TaskRecord` to (A2A state, artifacts). `Interrupted` collapses
/// to `failed` at the wire (reason in the artifact).
fn task_record_to_a2a(
    rec: &bridge_core::task_store::TaskRecord,
) -> (a2a::TaskState, Option<Vec<a2a::Artifact>>) {
    use bridge_core::task_store::TaskRecordStatus;
    let state = match rec.status {
        TaskRecordStatus::Working => a2a::TaskState::Working,
        TaskRecordStatus::Completed => a2a::TaskState::Completed,
        TaskRecordStatus::Failed => a2a::TaskState::Failed,
        TaskRecordStatus::Canceled => a2a::TaskState::Canceled,
        TaskRecordStatus::Interrupted => a2a::TaskState::Failed,
    };
    // Surface the result (success) or the error text (failed/interrupted) as the artifact.
    let payload = rec.result.clone().or_else(|| rec.error.clone());
    let artifacts = payload.map(|r| {
        vec![a2a::Artifact {
            artifact_id: a2a::new_artifact_id(),
            name: None,
            description: None,
            parts: vec![a2a::Part::text(r)],
            metadata: None,
            extensions: None,
        }]
    });
    (state, artifacts)
}

// ---- JSON-RPC helpers ----

fn jsonrpc_ok(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn jsonrpc_err(id: Value, code: i32, message: &str) -> Response {
    // JSON-RPC transport errors ride on HTTP 200 with an `error` member, which
    // is the conventional JSON-RPC-over-HTTP shape. We also set 400 for gate
    // rejections so plain HTTP clients see a failure status.
    let status = if code == JSONRPC_INVALID_REQUEST || code == JSONRPC_INVALID_PARAMS {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })),
    )
        .into_response()
}

/// Map a `BridgeError` to a JSON-RPC error using its disposition: request-level
/// rejections become INVALID_REQUEST; everything else (failed/suspended state)
/// becomes INTERNAL with the error's display message.
fn bridge_err_to_jsonrpc(id: Value, e: &BridgeError) -> Response {
    match e.disposition() {
        // Client-caused: `client_message()` is the (safe, helpful) Display.
        A2aDisposition::RejectRequest => {
            jsonrpc_err(id, JSONRPC_INVALID_REQUEST, &e.client_message())
        }
        // Internal failure: the full reason (may carry infra detail) goes to logs;
        // the wire gets a static category via `client_message()`.
        A2aDisposition::SetState(_) => {
            tracing::warn!(error = %e, "request failed (internal)");
            jsonrpc_err(id, JSONRPC_INTERNAL, &e.client_message())
        }
    }
}

// ---- params extraction ----

/// Extract the bearer token from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Pull a task id from the JSON-RPC params, accepting either `taskId`/`task_id`
/// or a nested `message.taskId`. Falls back to a generated id for fresh sends.
fn task_id_from_params(params: &Value) -> Result<TaskId, BridgeError> {
    let candidate = params
        .get("id")
        .or_else(|| params.get("taskId"))
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("message").and_then(|m| m.get("taskId")))
        .and_then(|v| v.as_str());
    match candidate {
        Some(s) if !s.is_empty() => TaskId::parse(s),
        // Fresh SendMessage with no task id: synthesize a stable stub id.
        _ => TaskId::parse("task-1"),
    }
}

/// A2A contextId: `message.contextId` (camelCase) → top-level `contextId` → `a2a-bridge.context`
/// metadata fallback. `None` if absent; empty → error.
fn context_id_from_params(params: &Value) -> Result<Option<ContextId>, BridgeError> {
    let raw = params
        .get("message")
        .and_then(|m| m.get("contextId"))
        .and_then(|v| v.as_str())
        .or_else(|| params.get("contextId").and_then(|v| v.as_str()))
        .or_else(|| {
            params
                .get("message")
                .and_then(|m| m.get("metadata"))
                .and_then(|md| md.get("a2a-bridge.context"))
                .and_then(|v| v.as_str())
        });
    match raw {
        Some(s) => Ok(Some(ContextId::parse(s)?)),
        None => Ok(None),
    }
}

/// Parse an effort-level string into the `Effort` enum, returning
/// `BridgeError::InvalidRequest{field:"effort"}` for unrecognised strings.
fn parse_effort_meta(s: &str) -> Result<bridge_core::domain::Effort, BridgeError> {
    s.parse::<bridge_core::domain::Effort>()
        .map_err(|_| BridgeError::InvalidRequest { field: "effort" })
}

/// Extract and validate the per-request `a2a-bridge.cwd` from `message.metadata`.
///
/// - Absent key → `Ok(None)`.
/// - Present key → structural validation via [`SessionCwd::parse`]; an invalid path
///   returns `BridgeError::InvalidRequest { field: "a2a-bridge.cwd" }`.
/// - If `allowed_root` is `Some(root)`: the cwd must satisfy `cwd.is_under(root)`;
///   a cwd outside the root also returns `InvalidRequest`.
///
/// Tested directly by unit tests; called by `gate()` before minting a task id.
fn session_cwd_from_params(
    params: &Value,
    allowed_root: Option<&str>,
) -> Result<Option<SessionCwd>, BridgeError> {
    let raw = params
        .get("message")
        .and_then(|m| m.get("metadata"))
        .and_then(|md| md.get("a2a-bridge.cwd"))
        .and_then(|v| v.as_str());
    let Some(s) = raw else {
        return Ok(None);
    };
    let cwd = SessionCwd::parse(s)?;
    if let Some(root_str) = allowed_root {
        let root = SessionCwd::parse(root_str).map_err(|_| BridgeError::InvalidRequest {
            field: "a2a-bridge.cwd",
        })?;
        if !cwd.is_under(&root) {
            return Err(BridgeError::InvalidRequest {
                field: "a2a-bridge.cwd",
            });
        }
    }
    Ok(Some(cwd))
}

/// Extract `TaskMeta` from JSON-RPC params.
///
/// Reads from `params.message.metadata`:
/// - `a2a-bridge.skill`  → `TaskMeta::skill`
/// - `a2a-bridge.agent`  → `TaskMeta::agent` (if present, must be non-empty; absent → `None`)
/// - `a2a-bridge.model` / `a2a-bridge.effort` / `a2a-bridge.mode` → `TaskMeta::overrides`
///
/// Returns `Err(BridgeError::InvalidRequest)` if `agent` is present but empty/invalid,
/// or if `effort` is present but not one of the recognised tier strings.
fn task_meta_from_params(params: &Value) -> Result<TaskMeta, BridgeError> {
    let metadata = params.get("message").and_then(|m| m.get("metadata"));

    let skill = metadata
        .and_then(|md| md.get("a2a-bridge.skill"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Agent: if the key is present, parse it (rejects empty → InvalidRequest); if absent → None.
    // Remap the AgentId newtype's `field: "AgentId"` to the wire field name `"agent"`
    // so the JSON-RPC error points at the `a2a-bridge.agent` metadata key (Task-9 nit).
    let agent = match metadata
        .and_then(|md| md.get("a2a-bridge.agent"))
        .and_then(|v| v.as_str())
    {
        Some(s) => Some(
            bridge_core::ids::AgentId::parse(s)
                .map_err(|_| BridgeError::InvalidRequest { field: "agent" })?,
        ),
        None => None,
    };

    // Per-request overrides: build Some(AgentOverride) if ANY of the three override keys is present.
    let model = metadata
        .and_then(|md| md.get("a2a-bridge.model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let effort = match metadata
        .and_then(|md| md.get("a2a-bridge.effort"))
        .and_then(|v| v.as_str())
    {
        Some(s) => Some(parse_effort_meta(s)?),
        None => None,
    };
    let mode = metadata
        .and_then(|md| md.get("a2a-bridge.mode"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let overrides = if model.is_some() || effort.is_some() || mode.is_some() {
        Some(bridge_core::domain::AgentOverride {
            model,
            effort,
            mode,
        })
    } else {
        None
    };

    Ok(TaskMeta {
        skill,
        agent,
        overrides,
    })
}

/// Pull message parts from params, extracting real text content.
///
/// Priority:
/// 1. If `message.parts` is a non-empty array, map each TEXT element's `text`
///    field to a `Part { text }`. An element contributes text only when its
///    `kind` is `"text"` (or absent — lenient); `data`/`file`/other-kind parts
///    are NOT prompt text and are skipped (so a `data` part's stray `text`
///    field is not misread as a prompt). If the `parts` array yields no usable
///    text (e.g. only `data` parts, or only blank text) it FALLS THROUGH to (2).
/// 2. Else if `message.text` is non-blank, one `Part { text }`.
/// 3. Else if a top-level `text` field is non-blank, one `Part { text }`.
/// 4. Otherwise empty vec — `gate()` rejects an empty result as a client error.
///
/// Blank/whitespace-only text never counts as content at any level.
fn parts_from_params(params: &Value) -> Vec<Part> {
    let message = params.get("message");

    // A non-blank text Part (blank/whitespace-only text never counts as content).
    let part = |t: &str| {
        let t = t.trim();
        (!t.is_empty()).then(|| Part {
            text: t.to_string(),
        })
    };

    // 1. message.parts array — TEXT parts only (kind=="text" or absent ⇒ lenient).
    //    If the array yields NO usable text (empty, or only data/file/blank parts)
    //    we FALL THROUGH to message.text rather than returning empty — so e.g.
    //    `{parts:[{kind:"file"}], text:"summarize"}` still uses "summarize".
    if let Some(parts_arr) = message
        .and_then(|m| m.get("parts"))
        .and_then(|p| p.as_array())
    {
        let texts: Vec<Part> = parts_arr
            .iter()
            .filter_map(|elem| {
                let is_text = matches!(
                    elem.get("kind").and_then(|k| k.as_str()),
                    Some("text") | None
                );
                if !is_text {
                    return None;
                }
                elem.get("text").and_then(|t| t.as_str()).and_then(part)
            })
            .collect();
        if !texts.is_empty() {
            return texts;
        }
    }

    // 2. message.text
    if let Some(p) = message
        .and_then(|m| m.get("text"))
        .and_then(|t| t.as_str())
        .and_then(part)
    {
        return vec![p];
    }

    // 3. top-level text
    if let Some(p) = params.get("text").and_then(|t| t.as_str()).and_then(part) {
        return vec![p];
    }

    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::RouteTarget;
    use bridge_core::domain::{AgentEntry, AgentKind, RegistrySnapshot, SessionSpec};
    use bridge_core::domain::{
        AuthContext, PeerTaskId, PendingRequest, PermissionDecision, PermissionRequest,
        SessionContext,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, CallerId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::*;
    use bridge_core::ports::{Delegation, DelegationPort, DelegationStream};
    use bridge_core::translator::Event;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tower::ServiceExt;

    // ---- inline fakes ----

    /// No-op registry lease (the in-test registry tracks nothing).
    struct NoopLease;
    impl Lease for NoopLease {}

    /// A minimal in-test `AgentRegistry`: maps agent ids to `(entry, backend)`.
    /// `resolve` returns `UnknownAgent` for an absent id (so the unknown-agent
    /// path is exercised without a panic). Used in place of the single `backend`.
    struct FakeRegistry {
        default: AgentId,
        entries: std::collections::HashMap<String, AgentEntry>,
        backends: std::collections::HashMap<String, Arc<dyn AgentBackend>>,
    }

    impl FakeRegistry {
        /// A single-agent registry mapping `id` → `backend` with a bare entry (no
        /// model/effort/mode). Mirrors the legacy single-backend wiring.
        fn single(id: &str, backend: Arc<dyn AgentBackend>) -> Arc<Self> {
            Self::with_entries(id, vec![(bare_entry(id), backend)])
        }

        /// A multi-agent registry. `default` is the first entry's id. Each tuple
        /// supplies the entry config (so `configure_session` receives base config)
        /// and the backend resolved for that id.
        fn with_entries(
            default: &str,
            agents: Vec<(AgentEntry, Arc<dyn AgentBackend>)>,
        ) -> Arc<Self> {
            let mut entries = std::collections::HashMap::new();
            let mut backends = std::collections::HashMap::new();
            for (entry, backend) in agents {
                let key = entry.id.as_str().to_owned();
                entries.insert(key.clone(), entry);
                backends.insert(key, backend);
            }
            Arc::new(Self {
                default: AgentId::parse(default).unwrap(),
                entries,
                backends,
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let key = id.as_str();
            match (self.entries.get(key), self.backends.get(key)) {
                (Some(entry), Some(backend)) => Ok(Resolved {
                    entry: Arc::new(entry.clone()),
                    backend: backend.clone(),
                    lease: Box::new(NoopLease),
                }),
                _ => Err(BridgeError::UnknownAgent { id: key.to_owned() }),
            }
        }
        fn default_id(&self) -> AgentId {
            self.default.clone()
        }
        async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            self.entries
                .keys()
                .map(|k| AgentId::parse(k).unwrap())
                .collect()
        }
    }

    /// An `AgentEntry` with the given id and no model/effort/mode defaults.
    fn bare_entry(id: &str) -> AgentEntry {
        AgentEntry {
            id: AgentId::parse(id).unwrap(),
            cmd: Some("fake".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
            session_cwd: None,
            sandbox: None,
            auth_method: None,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            mcp: vec![],
            mcp_delivery: Default::default(),
            extensions: Default::default(),
        }
    }

    /// Delegation double. Yields a scripted set of events; exposes a preset
    /// `peer_task` watch; records every `cancel(peer_id)` call. In "idle" mode it
    /// yields its scripted events then hangs forever (a peer stream that never
    /// completes), and its `peer_task` starts `None` and flips to `Some` after the
    /// first event if `peer_after_first` is set.
    struct FakeDelegation {
        events: Mutex<Option<Vec<Result<Event, BridgeError>>>>,
        peer_initial: Option<PeerTaskId>,
        /// If set, peer id becomes Some(this) only after the first event is emitted.
        peer_after_first: Option<PeerTaskId>,
        idle: bool,
        /// If set, the peer id is sent from INSIDE the events stream as the first
        /// frame is yielded (models the real client capturing the id lazily as
        /// frames are consumed/drained, rather than on an independent timer).
        peer_on_drain: Option<PeerTaskId>,
        cancels: Arc<Mutex<Vec<String>>>,
    }

    impl FakeDelegation {
        fn new(events: Vec<Result<Event, BridgeError>>, peer: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Some(events)),
                peer_initial: peer.map(|p| PeerTaskId(p.into())),
                peer_after_first: None,
                idle: false,
                peer_on_drain: None,
                cancels: Arc::new(Mutex::new(Vec::new())),
            })
        }
        fn idle(events: Vec<Result<Event, BridgeError>>, peer: Option<&str>) -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Some(events)),
                peer_initial: peer.map(|p| PeerTaskId(p.into())),
                peer_after_first: None,
                idle: true,
                peer_on_drain: None,
                cancels: Arc::new(Mutex::new(Vec::new())),
            })
        }
        /// Late-binding peer id: starts None, becomes Some(peer) after the first event.
        fn late_peer(events: Vec<Result<Event, BridgeError>>, peer: &str) -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Some(events)),
                peer_initial: None,
                peer_after_first: Some(PeerTaskId(peer.into())),
                idle: false,
                peer_on_drain: None,
                cancels: Arc::new(Mutex::new(Vec::new())),
            })
        }
        /// IDLE peer stream (yields its events then hangs forever) whose peer id
        /// binds LATE: starts None, becomes Some(peer) ~15ms after delegate runs.
        /// Models a still-running peer whose id is not yet known at cancel time.
        fn idle_late_peer(events: Vec<Result<Event, BridgeError>>, peer: &str) -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Some(events)),
                peer_initial: None,
                peer_after_first: Some(PeerTaskId(peer.into())),
                idle: true,
                peer_on_drain: None,
                cancels: Arc::new(Mutex::new(Vec::new())),
            })
        }
        /// Peer id captured lazily as frames are consumed: starts None and becomes
        /// Some(peer) from inside the events stream when the first frame is drained.
        fn late_peer_on_drain(events: Vec<Result<Event, BridgeError>>, peer: &str) -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Some(events)),
                peer_initial: None,
                peer_after_first: None,
                idle: false,
                peer_on_drain: Some(PeerTaskId(peer.into())),
                cancels: Arc::new(Mutex::new(Vec::new())),
            })
        }
        fn cancels(&self) -> Arc<Mutex<Vec<String>>> {
            self.cancels.clone()
        }
    }

    #[async_trait::async_trait]
    impl DelegationPort for FakeDelegation {
        async fn delegate(
            &self,
            _auth: &AuthContext,
            _local: &TaskId,
            _parts: Vec<Part>,
        ) -> Result<Delegation, BridgeError> {
            let scripted = self.events.lock().unwrap().take().unwrap_or_default();
            let (peer_tx, peer_rx) =
                tokio::sync::watch::channel::<Option<PeerTaskId>>(self.peer_initial.clone());
            let idle = self.idle;
            let after_first = self.peer_after_first.clone();
            let on_drain = self.peer_on_drain.clone();

            // Drive the late peer-id update from an INDEPENDENT task (mirrors the
            // real outbound client, whose background reader updates the watch
            // regardless of whether the caller is still consuming events). This is
            // what makes the early-cancel latch observable even after the
            // supervisor stops polling the event stream (case c).
            if let Some(p) = after_first {
                let tx = peer_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                    let _ = tx.send(Some(p));
                });
            }

            // Keep peer_tx alive for the lifetime of the stream so the watch
            // channel stays open while the peer is in flight.
            let events: DelegationStream = Box::pin(async_stream::stream! {
                let _hold = peer_tx;
                let mut first = true;
                for ev in scripted {
                    // Capture the peer id lazily as the first frame is consumed.
                    if first {
                        if let Some(p) = on_drain.clone() {
                            let _ = _hold.send(Some(p));
                        }
                        first = false;
                    }
                    yield ev;
                }
                if idle {
                    // Hang forever — an IDLE peer stream that never completes.
                    futures::future::pending::<()>().await;
                }
            });
            Ok(Delegation {
                events,
                peer_task: peer_rx,
            })
        }
        async fn cancel(&self, peer_task: &PeerTaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().push(peer_task.0.clone());
            Ok(())
        }
    }

    /// Backend that yields Text + Done and records whether prompt/cancel ran.
    struct FakeBackend {
        prompted: AtomicBool,
        cancelled: AtomicBool,
    }
    impl FakeBackend {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                prompted: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
            })
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompted.store(true, Ordering::SeqCst);
            let updates = vec![
                Ok(Update::Text("PONG".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.cancelled.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Backend whose turn ends with `Update::Done{stop_reason:STOP_REASON_CANCELLED}` (the
    /// ACP wire string for a user-cancelled turn). Used to prove the local producer
    /// reports `Canceled` (not `Completed`).
    struct CancelledBackend;
    #[async_trait::async_trait]
    impl AgentBackend for CancelledBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text("PARTIAL".into())),
                Ok(Update::Done {
                    stop_reason: STOP_REASON_CANCELLED.into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// Backend that panics if prompt is ever called — proves gating short-circuits.
    struct PanicBackend;
    #[async_trait::async_trait]
    impl AgentBackend for PanicBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("backend.prompt must not be called when a gate rejects");
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        map: Mutex<std::collections::HashMap<String, String>>,
        peer_tasks: Mutex<std::collections::HashMap<String, PeerTaskId>>,
        cancels: Mutex<std::collections::HashSet<String>>,
        fanouts: Mutex<std::collections::HashSet<String>>,
    }
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, t: &TaskId, s: &SessionId) -> Result<(), BridgeError> {
            self.map
                .lock()
                .unwrap()
                .insert(t.as_str().into(), s.as_str().into());
            Ok(())
        }
        async fn session_for(&self, t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(t.as_str())
                .map(|s| SessionId::parse(s.clone()).unwrap()))
        }
        async fn put_pending(&self, _t: &TaskId, _r: &PendingRequest) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn take_pending(&self, _t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
            Ok(None)
        }
        async fn set_peer_task(&self, t: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
            self.peer_tasks
                .lock()
                .unwrap()
                .insert(t.as_str().into(), peer.clone());
            Ok(())
        }
        async fn peer_task_for(&self, t: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
            Ok(self.peer_tasks.lock().unwrap().get(t.as_str()).cloned())
        }
        async fn request_cancel(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn cancel_requested(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.cancels.lock().unwrap().contains(t.as_str()))
        }
        async fn set_fanout(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.fanouts.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn is_fanout(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.fanouts.lock().unwrap().contains(t.as_str()))
        }
    }

    struct AutoApprove;
    impl PolicyEngine for AutoApprove {
        fn decide(
            &self,
            _req: &PermissionRequest,
            _c: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Ok(PermissionDecision::Approve)
        }
    }

    struct AlwaysKiro;
    impl RouteDecision for AlwaysKiro {
        fn route(&self, _t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Local(AgentId::parse("kiro")?))
        }
    }

    /// Routes `skill=="delegate"` to `Delegate`, everything else to local kiro.
    struct SkillRoute;
    impl RouteDecision for SkillRoute {
        fn route(&self, t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            if t.skill.as_deref() == Some("delegate") {
                Ok(RouteTarget::Delegate)
            } else {
                Ok(RouteTarget::Local(AgentId::parse("kiro")?))
            }
        }
    }

    /// No-op delegation used by tests that never route to Delegate.
    struct NoDelegation;
    #[async_trait::async_trait]
    impl DelegationPort for NoDelegation {
        async fn delegate(
            &self,
            _auth: &AuthContext,
            _local: &TaskId,
            _parts: Vec<Part>,
        ) -> Result<Delegation, BridgeError> {
            Err(BridgeError::UpstreamA2aError)
        }
        async fn cancel(&self, _peer_task: &PeerTaskId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct AlwaysGrant;
    impl AuthMiddleware for AlwaysGrant {
        fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
            Ok(AuthContext::new(CallerId::parse("anon").unwrap()))
        }
    }

    struct RejectAuth;
    impl AuthMiddleware for RejectAuth {
        fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
            Err(BridgeError::AuthRequired {
                request_id: "auth-1".into(),
            })
        }
    }

    fn build(backend: Arc<dyn AgentBackend>, auth: Arc<dyn AuthMiddleware>) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            FakeRegistry::single("kiro", backend),
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(AlwaysKiro),
            auth,
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "kiro",
        ))
    }

    /// Build a delegate-capable server, sharing the given store + delegation so
    /// tests can inspect peer-task persistence and recorded cancels.
    fn build_delegate(
        backend: Arc<dyn AgentBackend>,
        store: Arc<dyn SessionStore>,
        delegation: Arc<dyn DelegationPort>,
    ) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            FakeRegistry::single("kiro", backend),
            store,
            Arc::new(AutoApprove),
            Arc::new(SkillRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            delegation,
            "kiro",
        ))
    }

    fn delegate_params() -> Value {
        json!({ "message": {
            "text": "go",
            "metadata": { "a2a-bridge.skill": "delegate" }
        }})
    }

    fn router(srv: Arc<InboundServer>) -> Router {
        srv.router()
    }

    fn jsonrpc_body(method: &str, params: Value) -> axum::body::Body {
        axum::body::Body::from(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .unwrap(),
        )
    }

    fn post_request(
        method: &str,
        params: Value,
        version: &str,
    ) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .header(SVC_PARAM_VERSION, version)
            .body(jsonrpc_body(method, params))
            .unwrap()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Extract all `data:` payloads from an SSE body (one per line starting with "data: ").
    fn sse_data_payloads(body: &str) -> Vec<String> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|s| s.trim_end_matches('\r').to_owned())
            .collect()
    }

    #[tokio::test]
    async fn streaming_message_yields_artifact_event() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        // The SSE event: field names are still present.
        assert!(
            body.contains("artifact-update"),
            "SSE body should contain an artifact frame: {body}"
        );
        assert!(
            body.contains("PONG"),
            "artifact should carry the text: {body}"
        );
        // The data: payloads must parse as real a2a::StreamResponse — conformance check.
        let payloads = sse_data_payloads(&body);
        assert!(!payloads.is_empty(), "no data payloads in SSE body: {body}");
        // Final frame is now the terminal statusUpdate(Completed); artifact is penultimate.
        let last = payloads.last().unwrap();
        let sr: a2a::StreamResponse = serde_json::from_str(last).unwrap_or_else(|e| {
            panic!("last data payload must parse as StreamResponse: {e}: {last}")
        });
        assert!(
            matches!(
                &sr,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed
            ),
            "final frame must be terminal statusUpdate(Completed): {last}"
        );
        // The penultimate frame must be the artifact.
        let penultimate = &payloads[payloads.len() - 2];
        let sr2: a2a::StreamResponse = serde_json::from_str(penultimate).unwrap_or_else(|e| {
            panic!("penultimate data payload must parse as StreamResponse: {e}: {penultimate}")
        });
        assert!(
            matches!(sr2, a2a::StreamResponse::ArtifactUpdate(_)),
            "penultimate frame must be ArtifactUpdate: {penultimate}"
        );
    }

    #[tokio::test]
    async fn streaming_preserves_order_final_is_artifact() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        let body = body_string(resp).await;
        let last_artifact = body.rfind("artifact-update");
        assert!(last_artifact.is_some(), "no artifact frame: {body}");
        // All data: payloads must parse as a2a::StreamResponse (wire-conformance).
        let payloads = sse_data_payloads(&body);
        for payload in &payloads {
            let _: a2a::StreamResponse = serde_json::from_str(payload).unwrap_or_else(|e| {
                panic!("data payload must parse as StreamResponse: {e}: {payload}")
            });
        }
        // The last parsed payload must be the terminal statusUpdate(Completed).
        let last_sr: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert!(
            matches!(
                &last_sr,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed
            ),
            "final frame must be terminal statusUpdate(Completed)"
        );
        // The penultimate must be the ArtifactUpdate.
        let penultimate: a2a::StreamResponse =
            serde_json::from_str(&payloads[payloads.len() - 2]).unwrap();
        assert!(
            matches!(penultimate, a2a::StreamResponse::ArtifactUpdate(_)),
            "penultimate frame must be ArtifactUpdate"
        );
    }

    #[tokio::test]
    async fn streaming_cancelled_done_yields_terminal_canceled() {
        // A local turn ending with Done{stop_reason:"cancelled"} must produce a
        // terminal statusUpdate(Canceled) — NOT Completed.
        let srv = build(Arc::new(CancelledBackend), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let payloads = sse_data_payloads(&body);
        assert!(!payloads.is_empty(), "no data payloads in SSE body: {body}");
        // Exactly one terminal frame, and it is Canceled.
        let last_sr: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert!(
            matches!(
                &last_sr,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Canceled
            ),
            "final frame must be terminal statusUpdate(Canceled): {}",
            payloads.last().unwrap()
        );
        // No Completed terminal should appear (the producer must not append one).
        let completed_terminals = payloads
            .iter()
            .filter_map(|p| serde_json::from_str::<a2a::StreamResponse>(p).ok())
            .filter(|sr| {
                matches!(sr, a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed)
            })
            .count();
        assert_eq!(
            completed_terminals, 0,
            "a cancelled turn must not emit a Completed terminal: {body}"
        );
        // Ordering guard: an ArtifactUpdate must appear before the Canceled terminal.
        // The translator emits Artifact then Terminal(Canceled); the SSE layer must
        // preserve that order. Mirror the pattern in streaming_message_yields_artifact_event:
        // assert the penultimate payload is an ArtifactUpdate.
        assert!(
            payloads.len() >= 2,
            "must have at least artifact + terminal frames: {body}"
        );
        let penultimate: a2a::StreamResponse = serde_json::from_str(&payloads[payloads.len() - 2])
            .unwrap_or_else(|e| {
                panic!(
                    "penultimate payload must parse as StreamResponse: {e}: {}",
                    &payloads[payloads.len() - 2]
                )
            });
        assert!(
            matches!(penultimate, a2a::StreamResponse::ArtifactUpdate(_)),
            "penultimate frame must be ArtifactUpdate before the Canceled terminal: {}",
            &payloads[payloads.len() - 2]
        );
    }

    #[tokio::test]
    async fn unary_cancelled_done_returns_canceled_state() {
        // The unary local path must report TASK_STATE_CANCELED for a cancelled turn.
        let srv = build(Arc::new(CancelledBackend), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["task"]["state"], "TASK_STATE_CANCELED");
        assert_eq!(v["result"]["artifact"]["text"], "PARTIAL");
    }

    #[tokio::test]
    async fn cancel_task_propagates_to_backend() {
        let backend = FakeBackend::new();
        let srv = build(backend.clone(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-7" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            backend.cancelled.load(Ordering::SeqCst),
            "cancel must reach the backend"
        );
    }

    #[tokio::test]
    async fn unknown_version_header_rejected() {
        // A panicking backend proves the pipeline (prompt) is never reached.
        let srv = build(Arc::new(PanicBackend), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "9.9",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        assert!(body.contains("error"), "expected JSON-RPC error: {body}");
        assert!(
            body.contains("version mismatch"),
            "expected version mismatch message: {body}"
        );
    }

    #[tokio::test]
    async fn rejecting_auth_blocks_before_routing() {
        // RejectAuth + PanicBackend: if auth didn't short-circuit, prompt would panic.
        let srv = build(Arc::new(PanicBackend), Arc::new(RejectAuth));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("error"),
            "auth rejection should error: {body}"
        );
        assert!(
            body.contains("auth required"),
            "expected auth-required message: {body}"
        );
    }

    #[tokio::test]
    async fn serves_agent_card() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/.well-known/agent-card.json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let card: Value = serde_json::from_str(&body).unwrap();
        let skills = card["skills"].as_array().unwrap();
        // Updated for Task 5a: three skills (code, delegate, fan-out).
        assert_eq!(skills.len(), 3);
        assert!(skills.iter().any(|s| s["id"] == "code"));
        assert!(skills.iter().any(|s| s["id"] == "delegate"));
        assert!(skills.iter().any(|s| s["id"] == "fan-out"));
    }

    #[tokio::test]
    async fn unary_send_message_returns_artifact() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["artifact"]["text"], "PONG");
    }

    #[tokio::test]
    async fn get_task_returns_state_stub() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::GET_TASK,
                json!({ "taskId": "task-9" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["task"]["id"], "task-9");
        assert!(v["result"]["task"]["state"].is_string());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request("Bogus", json!({}), "1.0"))
            .await
            .unwrap();
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], JSONRPC_METHOD_NOT_FOUND);
    }

    // ---- Task 7: skill metadata + real Part.text extraction ----

    #[test]
    fn skill_metadata_parsed_into_taskmeta() {
        let p = serde_json::json!({"message":{"metadata":{"a2a-bridge.skill":"delegate"}}});
        assert_eq!(
            task_meta_from_params(&p).unwrap().skill.as_deref(),
            Some("delegate")
        );
    }

    #[test]
    fn no_skill_metadata_is_none() {
        let p = serde_json::json!({"message":{"text":"hi"}});
        assert_eq!(task_meta_from_params(&p).unwrap().skill, None);
    }

    #[test]
    fn context_id_parsed_from_field_and_metadata() {
        let v = serde_json::json!({ "message": { "contextId": "c-1", "text": "hi" } });
        assert_eq!(context_id_from_params(&v).unwrap().unwrap().as_str(), "c-1");
        let v2 = serde_json::json!({ "message": { "metadata": { "a2a-bridge.context": "c-2" }, "text": "hi" } });
        assert_eq!(
            context_id_from_params(&v2).unwrap().unwrap().as_str(),
            "c-2"
        );
        assert!(
            context_id_from_params(&serde_json::json!({ "message": { "text": "hi" } }))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn task_id_accepts_standard_id_field() {
        let v = serde_json::json!({ "id": "t-9", "message": { "text": "hi" } });
        assert_eq!(task_id_from_params(&v).unwrap().as_str(), "t-9");
    }

    #[test]
    fn context_id_rejected_on_delegate_route() {
        let srv = build_delegate(
            FakeBackend::new(),
            Arc::new(FakeStore::default()),
            Arc::new(NoDelegation),
        );
        let params = serde_json::json!({
            "message": {
                "contextId": "c-1",
                "text": "hi",
                "metadata": { "a2a-bridge.skill": "delegate" }
            }
        });

        match srv.gate(&HeaderMap::new(), &params) {
            Err(BridgeError::InvalidRequest {
                field: "contextId is only supported on the local route in Slice 0",
            }) => {}
            Err(other) => panic!("expected contextId Local-only rejection, got: {other:?}"),
            Ok(_) => panic!("expected contextId Local-only rejection, got Ok"),
        }
    }

    // ---- Task 9: task_meta_from_params reads agent + overrides ----

    #[test]
    fn task_meta_reads_agent_and_overrides() {
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.agent": "codex",
                    "a2a-bridge.model": "gpt-5.5",
                    "a2a-bridge.effort": "high",
                    "a2a-bridge.mode": "read-only"
                }
            }
        });
        let meta = task_meta_from_params(&p).unwrap();
        assert_eq!(meta.agent.as_ref().map(|a| a.as_str()), Some("codex"));
        let ov = meta.overrides.as_ref().expect("overrides should be Some");
        assert_eq!(ov.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(ov.effort, Some(bridge_core::domain::Effort::High));
        assert_eq!(ov.mode.as_deref(), Some("read-only"));
    }

    #[test]
    fn task_meta_accepts_xhigh_effort() {
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.effort": "xhigh"
                }
            }
        });
        let meta = task_meta_from_params(&p).unwrap();
        let ov = meta.overrides.as_ref().expect("overrides should be Some");
        assert_eq!(ov.effort, Some(bridge_core::domain::Effort::Xhigh));
    }

    #[test]
    fn task_meta_invalid_effort_returns_err() {
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.effort": "bogus"
                }
            }
        });
        match task_meta_from_params(&p) {
            Err(BridgeError::InvalidRequest { field: "effort" }) => {}
            other => panic!("expected InvalidRequest{{field:\"effort\"}}, got: {other:?}"),
        }
    }

    #[test]
    fn task_meta_empty_agent_returns_err() {
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.agent": ""
                }
            }
        });
        match task_meta_from_params(&p) {
            Err(BridgeError::InvalidRequest { .. }) => {}
            other => panic!("expected InvalidRequest, got: {other:?}"),
        }
    }

    #[test]
    fn task_meta_absent_agent_is_none() {
        // Absent `a2a-bridge.agent` key → agent field is None (default falls through at route time).
        let p = serde_json::json!({"message":{"metadata":{"a2a-bridge.skill":"delegate"}}});
        let meta = task_meta_from_params(&p).unwrap();
        assert!(meta.agent.is_none());
    }

    #[test]
    fn parts_from_message_text() {
        let p = serde_json::json!({"message":{"text":"PING"}});
        let v = parts_from_params(&p);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "PING");
    }

    #[test]
    fn parts_from_a2a_parts_array() {
        let p = serde_json::json!({"message":{"parts":[{"text":"A"},{"text":"B"}]}});
        let v = parts_from_params(&p);
        assert_eq!(
            v.iter().map(|x| x.text.clone()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }

    // ---- H2: kind-aware part extraction ----

    #[test]
    fn parts_skips_non_text_kinds() {
        // A standard A2A message: text parts contribute; data/file parts do not
        // (and a data part's stray `text` field must NOT be misread as a prompt).
        let p = serde_json::json!({"message":{"parts":[
            {"kind":"text","text":"hello"},
            {"kind":"data","data":{"x":1},"text":"should-be-ignored"},
            {"kind":"file","file":{"name":"f"}},
            {"text":"kind-absent-is-lenient-text"},
        ]}});
        let v: Vec<String> = parts_from_params(&p).into_iter().map(|x| x.text).collect();
        assert_eq!(v, vec!["hello", "kind-absent-is-lenient-text"]);
    }

    #[test]
    fn parts_only_non_text_is_empty() {
        let p = serde_json::json!({"message":{"parts":[{"kind":"data","data":{"x":1}}]}});
        assert!(parts_from_params(&p).is_empty());
    }

    #[test]
    fn parts_blank_text_is_dropped() {
        // Whitespace-only / empty text is not content (would otherwise dispatch a
        // textless prompt). Covers the PoC-codex finding.
        assert!(parts_from_params(&serde_json::json!({"message":{"text":"   "}})).is_empty());
        assert!(
            parts_from_params(&serde_json::json!({"message":{"parts":[{"text":""}]}})).is_empty()
        );
        // and a blank part among real ones is dropped, not kept.
        let p = serde_json::json!({"message":{"parts":[{"text":"  "},{"text":"real"}]}});
        let v: Vec<String> = parts_from_params(&p).into_iter().map(|x| x.text).collect();
        assert_eq!(v, vec!["real"]);
    }

    #[test]
    fn parts_array_without_text_falls_through_to_message_text() {
        // A parts array of only non-text parts must NOT suppress message.text
        // (the self-hosted review's MAJOR 3).
        let p = serde_json::json!({"message":{"parts":[{"kind":"file","file":{"name":"f"}}],"text":"summarize"}});
        let v: Vec<String> = parts_from_params(&p).into_iter().map(|x| x.text).collect();
        assert_eq!(v, vec!["summarize"]);
    }

    #[tokio::test]
    async fn send_message_data_only_parts_returns_invalid_request() {
        // Routed end-to-end: a data-only message is rejected, not dispatched.
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "parts": [{"kind":"data","data":{"x":1}}] } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn streaming_message_empty_text_rejected_before_sse() {
        // H1 covers the STREAMING path too (both unary + stream go through gate()).
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "   " } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ---- H1: empty/textless message is rejected (not dispatched empty) ----

    #[tokio::test]
    async fn send_message_with_no_text_returns_invalid_request() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "parts": [] } }),
                "1.0",
            ))
            .await
            .unwrap();
        // A request-level rejection (not a dispatched empty prompt to the agent).
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("no text content"),
            "expected a 'no text content' error, got: {msg}"
        );
    }

    // ---- Task 9: delegate route + consolidated cancel ----

    #[tokio::test]
    async fn delegate_skill_streams_peer_artifact() {
        let deleg = FakeDelegation::new(
            vec![Ok(Event::status("work")), Ok(Event::artifact("DONE"))],
            Some("p1"),
        );
        let srv = build_delegate(FakeBackend::new(), Arc::new(FakeStore::default()), deleg);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("artifact-update") && body.contains("DONE"),
            "SSE body should carry the peer artifact: {body}"
        );
        let payloads = sse_data_payloads(&body);
        // Final frame: terminal statusUpdate(Completed) synthesized by the delegate producer.
        let last = payloads.last().expect("at least one SSE data payload");
        let sr: a2a::StreamResponse = serde_json::from_str(last)
            .unwrap_or_else(|e| panic!("final frame must parse as StreamResponse: {e}: {last}"));
        assert!(
            matches!(
                &sr,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed
            ),
            "final frame must be terminal statusUpdate(Completed): {last}"
        );
        // Penultimate: the ArtifactUpdate from the peer.
        let penultimate = &payloads[payloads.len() - 2];
        let sr2: a2a::StreamResponse = serde_json::from_str(penultimate)
            .unwrap_or_else(|e| panic!("penultimate frame must parse as StreamResponse: {e}"));
        assert!(
            matches!(sr2, a2a::StreamResponse::ArtifactUpdate(_)),
            "penultimate frame must be ArtifactUpdate: {penultimate}"
        );
    }

    #[tokio::test]
    async fn delegate_route_never_touches_local_backend() {
        // PanicBackend would panic in prompt if the local path were taken.
        let deleg = FakeDelegation::new(vec![Ok(Event::artifact("DONE"))], Some("p1"));
        let srv = build_delegate(
            Arc::new(PanicBackend),
            Arc::new(FakeStore::default()),
            deleg,
        );
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(body.contains("DONE"), "delegate must complete: {body}");
    }

    #[tokio::test]
    async fn inbound_cancel_task_cancels_peer() {
        // S2b: after a delegate stream persists local->peer, CancelTask cancels the peer.
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let deleg = FakeDelegation::new(vec![Ok(Event::artifact("DONE"))], Some("p1"));
        let recorded = deleg.cancels();
        let srv = build_delegate(FakeBackend::new(), store.clone(), deleg);

        // Drive the delegate stream to completion so local->peer is persisted.
        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        let _ = body_string(resp).await;

        // The peer-task mapping must now be present (task-1 is the synthesized id).
        let local = TaskId::parse("task-1").unwrap();
        for _ in 0..200 {
            if store.peer_task_for(&local).await.unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            store.peer_task_for(&local).await.unwrap().is_some(),
            "local->peer mapping must be persisted after the delegate stream"
        );

        // POST CancelTask for the local task id.
        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            recorded.lock().unwrap().iter().any(|c| c == "p1"),
            "inbound CancelTask must cancel the peer: {:?}",
            recorded.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn caller_disconnect_cancels_idle_peer() {
        // (b): idle peer (one event, then hangs). Drop the SSE receiver -> cancel("p1").
        let deleg = FakeDelegation::idle(vec![Ok(Event::status("work"))], Some("p1"));
        let recorded = deleg.cancels();
        let srv = build_delegate(FakeBackend::new(), Arc::new(FakeStore::default()), deleg);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Drop the response body (and its SSE receiver) -> caller disconnect.
        drop(resp);
        wait_until(|| recorded.lock().unwrap().iter().any(|c| c == "p1")).await;
        assert!(
            recorded.lock().unwrap().iter().any(|c| c == "p1"),
            "dropping an idle peer stream must cancel the peer: {:?}",
            recorded.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn early_cancel_before_peer_id_is_latched_then_applied() {
        // (c): request_cancel BEFORE the peer id is known; id appears after first event.
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let local = TaskId::parse("task-1").unwrap();
        // Latch the cancel before delegation runs.
        store.request_cancel(&local).await.unwrap();

        let deleg = FakeDelegation::late_peer(
            vec![Ok(Event::status("work")), Ok(Event::artifact("DONE"))],
            "p1",
        );
        let recorded = deleg.cancels();
        let srv = build_delegate(FakeBackend::new(), store.clone(), deleg);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        let _ = body_string(resp).await;
        wait_until(|| recorded.lock().unwrap().iter().any(|c| c == "p1")).await;
        assert!(
            recorded.lock().unwrap().iter().any(|c| c == "p1"),
            "early-cancel latch must apply once the peer id appears: {:?}",
            recorded.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn inbound_cancel_on_active_delegated_stream_cancels_peer_exactly_once() {
        // Fix 1: an inbound CancelTask on an ACTIVE delegated stream must result in
        // exactly ONE upstream cancel("p1"), even though BOTH the cancel_task()
        // handler (direct POST) and the supervisor's poll_cancel_requested arm
        // would otherwise fire. The idle delegation keeps the supervisor alive,
        // and peer_initial=Some("p1") means the peer id is known immediately so
        // both paths can race to cancel.
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let deleg = FakeDelegation::idle(vec![Ok(Event::status("work"))], Some("p1"));
        let recorded = deleg.cancels();
        let srv = build_delegate(FakeBackend::new(), store.clone(), deleg);

        // Open the delegate stream (do NOT drop the response, so the supervisor
        // stays alive — tx.closed() never fires). The producer persists local->peer.
        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Wait until local->peer is persisted (supervisor is running, peer known).
        let local = TaskId::parse("task-1").unwrap();
        for _ in 0..200 {
            if store.peer_task_for(&local).await.unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            store.peer_task_for(&local).await.unwrap().is_some(),
            "local->peer mapping must be persisted before CancelTask"
        );

        // POST CancelTask: cancel_task() POSTs directly AND latches request_cancel,
        // which wakes the supervisor's poll_cancel_requested arm. With the guard,
        // exactly one of them wins.
        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Give the supervisor time to (try to) fire its own cancel.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let calls: Vec<String> = recorded.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            1,
            "exactly one upstream cancel must be recorded, got: {calls:?}"
        );
        assert_eq!(calls[0], "p1");
    }

    #[tokio::test]
    async fn unary_delegate_persists_local_to_peer() {
        // Fix 2: a unary SendMessage delegate must persist local->peer. The peer id
        // becomes Some("p1") only as the events stream is drained (late_peer), so
        // reading the watch before draining (today's bug) yields None.
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let deleg = FakeDelegation::late_peer_on_drain(vec![Ok(Event::artifact("DONE"))], "p1");
        let srv = build_delegate(FakeBackend::new(), store.clone(), deleg);

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                delegate_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;

        let local = TaskId::parse("task-1").unwrap();
        assert_eq!(
            store.peer_task_for(&local).await.unwrap(),
            Some(PeerTaskId("p1".into())),
            "unary delegate must persist local->peer after draining events"
        );
    }

    // ---- Task 2: single-source terminal synthesis ----

    /// A fake backend that yields a Status then an Artifact then ends cleanly.
    struct TerminalSynthBackend;
    #[async_trait::async_trait]
    impl AgentBackend for TerminalSynthBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text("status-chunk".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn single_source_local_producer_appends_terminal_completed_frame() {
        let srv = build(Arc::new(TerminalSynthBackend), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let payloads = sse_data_payloads(&body);
        assert!(
            payloads.len() >= 2,
            "must have at least artifact + terminal: {body}"
        );

        // Final frame: terminal statusUpdate(Completed).
        let last: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap())
            .unwrap_or_else(|e| panic!("last data payload must parse as StreamResponse: {e}"));
        assert!(
            matches!(
                &last,
                a2a::StreamResponse::StatusUpdate(e) if e.status.state == a2a::TaskState::Completed
            ),
            "final frame must be terminal statusUpdate(Completed): {:?}",
            payloads.last()
        );

        // Penultimate frame: the artifact from the translator.
        let penultimate: a2a::StreamResponse = serde_json::from_str(&payloads[payloads.len() - 2])
            .unwrap_or_else(|e| panic!("penultimate payload must parse as StreamResponse: {e}"));
        assert!(
            matches!(penultimate, a2a::StreamResponse::ArtifactUpdate(_)),
            "penultimate frame must be ArtifactUpdate: {:?}",
            &payloads[payloads.len() - 2]
        );
    }

    /// Poll `cond` up to ~2s, sleeping briefly between checks. Panics on timeout.
    async fn wait_until(mut cond: impl FnMut() -> bool) {
        for _ in 0..200 {
            if cond() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("condition not met within budget");
    }

    // ---- Task 5b: explicit fan-out task-mode marker + unary fan-out Task shape ----

    #[tokio::test]
    async fn unary_fanout_returns_task_with_both_artifacts() {
        // FakeBackend yields Text("KA") + Done -> kiro artifact "KA".
        // FakeDelegation yields artifact("PA") -> peer artifact "PA".
        // RouteTarget::Fanout via FanoutSkillRoute("fan-out").
        // The unary response must be a JSON-RPC result whose result is an a2a::Task
        // with status.state==Completed and artifacts: [{name:"kiro", text:"KA"}, {name:"peer", text:"PA"}].
        struct KiroABackend;
        #[async_trait::async_trait]
        impl AgentBackend for KiroABackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                let updates = vec![
                    Ok(Update::Text("KA".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ];
                Ok(Box::pin(tokio_stream::iter(updates)))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let deleg = FakeDelegation::new(vec![Ok(Event::artifact("PA"))], Some("p1"));
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(Arc::new(KiroABackend), store.clone(), deleg);

        let resp = router(srv)
            .oneshot(post_request(methods::SEND_MESSAGE, fanout_params(), "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("unary fanout response must be valid JSON: {e}: {body}"));
        // Must be a JSON-RPC success (no error).
        assert!(v.get("error").is_none(), "expected no error: {body}");
        let result = &v["result"];
        // The result IS the a2a::Task directly (contextId, id, status, artifacts).
        // result.status.state must be Completed.
        let state = result["status"]["state"].as_str().unwrap_or("");
        assert_eq!(
            state, "TASK_STATE_COMPLETED",
            "status.state must be 'TASK_STATE_COMPLETED': {body}"
        );
        // result.artifacts must have 2 entries.
        let artifacts = result["artifacts"]
            .as_array()
            .unwrap_or_else(|| panic!("result.artifacts must be an array: {body}"));
        assert_eq!(artifacts.len(), 2, "must have exactly 2 artifacts: {body}");
        // Check that there is one artifact named "kiro" with text "KA" and one named "peer" with text "PA".
        let names: Vec<&str> = artifacts
            .iter()
            .filter_map(|a| a["name"].as_str())
            .collect();
        assert!(names.contains(&"kiro"), "must have kiro artifact: {body}");
        assert!(names.contains(&"peer"), "must have peer artifact: {body}");
        let kiro_art = artifacts.iter().find(|a| a["name"] == "kiro").unwrap();
        let kiro_text = kiro_art["parts"][0]["text"].as_str().unwrap_or("");
        assert_eq!(kiro_text, "KA", "kiro artifact text must be 'KA': {body}");
        let peer_art = artifacts.iter().find(|a| a["name"] == "peer").unwrap();
        let peer_text = peer_art["parts"][0]["text"].as_str().unwrap_or("");
        assert_eq!(peer_text, "PA", "peer artifact text must be 'PA': {body}");
        // Also verify is_fanout was set on the task in the store.
        let task_id = TaskId::parse("task-1").unwrap();
        assert!(
            store.is_fanout(&task_id).await.unwrap(),
            "store must mark task-1 as fanout after unary fanout dispatch"
        );
    }

    #[tokio::test]
    async fn unary_single_source_response_unchanged() {
        // Regression: plain (non-fanout) unary SendMessage still returns the legacy shape.
        // The existing unary_send_message_returns_artifact test expects result.artifact.text == "PONG".
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        // Legacy shape: result.artifact.text, not result.task.artifacts.
        assert_eq!(
            v["result"]["artifact"]["text"], "PONG",
            "single-source unary shape unchanged: {body}"
        );
        // Must NOT have result.artifacts (that's only the fan-out shape).
        // In the legacy shape, result has "task" (with id+state), "artifact" (with text), "status".
        assert!(
            v["result"]["artifacts"].is_null(),
            "single-source must not have result.artifacts (fan-out only): {body}"
        );
    }

    // ---- Task 5a: fan-out streaming dispatch ----

    /// Routes `skill=="fan-out"` to `Fanout`; `skill=="delegate"` to `Delegate`;
    /// everything else to local kiro. Used only in fan-out tests.
    struct FanoutSkillRoute;
    impl RouteDecision for FanoutSkillRoute {
        fn route(&self, t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            match t.skill.as_deref() {
                Some("fan-out") => Ok(RouteTarget::Fanout),
                Some("delegate") => Ok(RouteTarget::Delegate),
                _ => Ok(RouteTarget::Local(AgentId::parse("kiro")?)),
            }
        }
    }

    /// Build a fan-out-capable server sharing backend, store, and delegation.
    fn build_fanout(
        backend: Arc<dyn AgentBackend>,
        store: Arc<dyn SessionStore>,
        delegation: Arc<dyn DelegationPort>,
    ) -> Arc<InboundServer> {
        build_fanout_labeled(backend, store, delegation, "kiro")
    }

    /// Like [`build_fanout`] but with an explicit local-source label so a test can
    /// assert the fan-out local source is labeled from config (e.g. `"codex"`).
    fn build_fanout_labeled(
        backend: Arc<dyn AgentBackend>,
        store: Arc<dyn SessionStore>,
        delegation: Arc<dyn DelegationPort>,
        local_source_label: &str,
    ) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            FakeRegistry::single("kiro", backend),
            store,
            Arc::new(AutoApprove),
            Arc::new(FanoutSkillRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            delegation,
            local_source_label.to_string(),
        ))
    }

    fn fanout_params() -> Value {
        json!({ "message": {
            "text": "go",
            "metadata": { "a2a-bridge.skill": "fan-out" }
        }})
    }

    #[tokio::test]
    async fn local_kiro_source_drops_usage_events() {
        let source = local_kiro_source(
            "kiro".into(),
            Arc::new(UsageThenTextBackend),
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            TaskId::parse("task-usage").unwrap(),
            SessionId::parse("session-usage").unwrap(),
            vec![Part { text: "go".into() }],
        );

        let out: Vec<Result<Event, BridgeError>> = source.stream.collect().await;
        // Usage is swallowed before the fan-out merge; the translator still emits its
        // Status flush + final Artifact for the text, so NO Usage event may survive.
        assert!(
            out.iter()
                .all(|r| r.as_ref().map_or(true, |e| e.kind() != &EventKind::Usage)),
            "no Usage event may survive the fan-out source",
        );
        let artifact = out
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .find(|e| e.kind() == &EventKind::Artifact)
            .expect("artifact present");
        assert_eq!(artifact.text(), "ok");
    }

    #[tokio::test]
    async fn fanout_streaming_merges_both_sources_with_terminal() {
        // FakeBackend yields Text("KIRO") + Done -> kiro artifact "KIRO".
        // FakeDelegation yields status("work") + artifact("PEER") -> peer artifact.
        // Both sources labeled; terminal frame is Completed.
        let deleg = FakeDelegation::new(
            vec![Ok(Event::status("work")), Ok(Event::artifact("PEER"))],
            Some("p1"),
        );
        let srv = build_fanout(FakeBackend::new(), Arc::new(FakeStore::default()), deleg);

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let payloads = sse_data_payloads(&body);
        assert!(
            !payloads.is_empty(),
            "fan-out SSE must emit at least one frame: {body}"
        );

        // Parse all payloads as StreamResponse (wire conformance).
        let parsed: Vec<a2a::StreamResponse> = payloads
            .iter()
            .map(|p| {
                serde_json::from_str(p)
                    .unwrap_or_else(|e| panic!("payload must parse as StreamResponse: {e}: {p}"))
            })
            .collect();

        // There must be an artifact from kiro source.
        let has_kiro_artifact = parsed.iter().any(|sr| {
            matches!(sr, a2a::StreamResponse::ArtifactUpdate(e)
                if e.metadata.as_ref()
                    .and_then(|m| m.get("a2a-bridge.source"))
                    .and_then(|v| v.as_str())
                    == Some("kiro")
            )
        });
        assert!(
            has_kiro_artifact,
            "fan-out SSE must contain a kiro-labeled artifact: {body}"
        );

        // There must be an artifact from peer source.
        let has_peer_artifact = parsed.iter().any(|sr| {
            matches!(sr, a2a::StreamResponse::ArtifactUpdate(e)
                if e.metadata.as_ref()
                    .and_then(|m| m.get("a2a-bridge.source"))
                    .and_then(|v| v.as_str())
                    == Some("peer")
            )
        });
        assert!(
            has_peer_artifact,
            "fan-out SSE must contain a peer-labeled artifact: {body}"
        );

        // The LAST frame must be a terminal statusUpdate(Completed).
        let last = parsed.last().unwrap();
        assert!(
            matches!(
                last,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed
            ),
            "final fan-out frame must be terminal statusUpdate(Completed): {:?}",
            payloads.last()
        );
    }

    #[tokio::test]
    async fn fanout_local_source_labeled_from_config() {
        // The local-source label is fed from `[agent] name`, not hardcoded "kiro".
        // With a "codex" agent the fan-out local artifact must be labeled "codex".
        let deleg = FakeDelegation::new(
            vec![Ok(Event::status("work")), Ok(Event::artifact("PEER"))],
            Some("p1"),
        );
        let srv = build_fanout_labeled(
            FakeBackend::new(),
            Arc::new(FakeStore::default()),
            deleg,
            "codex",
        );

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let payloads = sse_data_payloads(&body);
        let parsed: Vec<a2a::StreamResponse> = payloads
            .iter()
            .map(|p| serde_json::from_str(p).expect("payload parses as StreamResponse"))
            .collect();

        // The local source's artifact must be labeled "codex" (wire-observable),
        // and NOT "kiro".
        let has_codex_artifact = parsed.iter().any(|sr| {
            matches!(sr, a2a::StreamResponse::ArtifactUpdate(e)
                if e.metadata.as_ref()
                    .and_then(|m| m.get("a2a-bridge.source"))
                    .and_then(|v| v.as_str())
                    == Some("codex")
            )
        });
        assert!(
            has_codex_artifact,
            "fan-out local source must be labeled 'codex' (from agent.name): {body}"
        );
        let has_kiro_artifact = parsed.iter().any(|sr| {
            matches!(sr, a2a::StreamResponse::ArtifactUpdate(e)
                if e.metadata.as_ref()
                    .and_then(|m| m.get("a2a-bridge.source"))
                    .and_then(|v| v.as_str())
                    == Some("kiro")
            )
        });
        assert!(
            !has_kiro_artifact,
            "no source should be hardcoded 'kiro' when agent.name is 'codex': {body}"
        );
    }

    // ---- Task 6: fan-out cancel-all (immediate Kiro, latched peer, per-source guard) ----

    /// Backend that counts `cancel(session)` calls and exposes a never-ending
    /// prompt stream (so the kiro source stays ALIVE until cancelled). Used to
    /// assert exactly-one Kiro cancel and immediate (non-peer-blocked) cancel.
    struct CountingIdleBackend {
        cancel_count: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl CountingIdleBackend {
        fn new() -> (Arc<Self>, Arc<std::sync::atomic::AtomicUsize>) {
            let cancel_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Arc::new(Self {
                    cancel_count: cancel_count.clone(),
                }),
                cancel_count,
            )
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for CountingIdleBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            // One Text frame, then hang forever — an IDLE kiro stream.
            let s = async_stream::stream! {
                yield Ok(Update::Text("KWORK".into()));
                futures::future::pending::<()>().await;
            };
            Ok(Box::pin(s))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.cancel_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Backend whose prompt ends immediately (kiro source FINISHES) and which
    /// counts cancels — used to prove a finished source is a cancel no-op.
    struct CountingDoneBackend {
        cancel_count: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl CountingDoneBackend {
        fn new() -> (Arc<Self>, Arc<std::sync::atomic::AtomicUsize>) {
            let cancel_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            (
                Arc::new(Self {
                    cancel_count: cancel_count.clone(),
                }),
                cancel_count,
            )
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for CountingDoneBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text("KDONE".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.cancel_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn fanout_cancel_cancels_all_sources_exactly_once() {
        // A fan-out streaming task with both sources ALIVE (kiro idle, peer idle).
        // POST an inbound CancelTask -> backend.cancel recorded exactly once AND
        // delegation.cancel recorded exactly once; terminal is Canceled.
        let (backend, kiro_cancels) = CountingIdleBackend::new();
        let deleg = FakeDelegation::idle(vec![Ok(Event::status("pwork"))], Some("p1"));
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        // Open the fan-out stream; keep the response so the supervisor stays alive.
        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Wait until the task is marked fan-out (producer started).
        let local = TaskId::parse("task-1").unwrap();
        wait_until(|| futures::executor::block_on(store.is_fanout(&local)).unwrap_or(false)).await;

        // POST CancelTask.
        let resp2 = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        // Both sources cancelled, each exactly once.
        wait_until(|| {
            kiro_cancels.load(Ordering::SeqCst) >= 1
                && peer_cancels.lock().unwrap().iter().any(|c| c == "p1")
        })
        .await;
        // Give any duplicate-cancel race time to (incorrectly) fire.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(
            kiro_cancels.load(Ordering::SeqCst),
            1,
            "backend.cancel must fire exactly once"
        );
        let peers: Vec<String> = peer_cancels.lock().unwrap().clone();
        assert_eq!(
            peers,
            vec!["p1".to_string()],
            "delegation.cancel must fire exactly once for the peer"
        );

        // Terminal frame on the stream is Canceled.
        let body = body_string(resp).await;
        let payloads = sse_data_payloads(&body);
        let last = payloads.last().expect("at least one SSE frame");
        let sr: a2a::StreamResponse = serde_json::from_str(last)
            .unwrap_or_else(|e| panic!("final frame must parse as StreamResponse: {e}: {last}"));
        assert!(
            matches!(
                &sr,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Canceled
            ),
            "final fan-out frame must be terminal statusUpdate(Canceled): {last}"
        );
    }

    #[tokio::test]
    async fn fanout_cancel_does_not_block_on_peer_id() {
        // Peer id is NOT yet known at cancel time (late_peer: appears after first
        // event). Kiro cancel must fire IMMEDIATELY (within a short bound) without
        // waiting on the peer id; the peer cancel is latched and applied once the
        // watch yields an id.
        let (backend, kiro_cancels) = CountingIdleBackend::new();
        // Peer is idle (still running) and its id binds late. The Kiro cancel must
        // not wait on the peer id; the peer cancel is latched and applied once the
        // watch yields the id.
        let deleg = FakeDelegation::idle_late_peer(vec![Ok(Event::status("pwork"))], "p1");
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let local = TaskId::parse("task-1").unwrap();
        wait_until(|| futures::executor::block_on(store.is_fanout(&local)).unwrap_or(false)).await;

        // POST CancelTask and assert Kiro cancel fired within a short bound,
        // regardless of the peer id.
        let resp2 = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        // Immediate Kiro cancel: within ~300ms (does not await the peer id).
        let mut kiro_fired = false;
        for _ in 0..30 {
            if kiro_cancels.load(Ordering::SeqCst) >= 1 {
                kiro_fired = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            kiro_fired,
            "Kiro cancel must fire immediately, not blocked on the peer id"
        );

        // The peer cancel is latched and applied once the id appears.
        wait_until(|| peer_cancels.lock().unwrap().iter().any(|c| c == "p1")).await;
        let _ = body_string(resp).await;
    }

    #[tokio::test]
    async fn fanout_caller_disconnect_cancels_all() {
        // Drop the SSE receiver mid-stream -> both backend.cancel and
        // delegation.cancel recorded.
        let (backend, kiro_cancels) = CountingIdleBackend::new();
        let deleg = FakeDelegation::idle(vec![Ok(Event::status("pwork"))], Some("p1"));
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let local = TaskId::parse("task-1").unwrap();
        wait_until(|| futures::executor::block_on(store.is_fanout(&local)).unwrap_or(false)).await;

        // Drop the response (and its SSE receiver) -> caller disconnect.
        drop(resp);

        wait_until(|| {
            kiro_cancels.load(Ordering::SeqCst) >= 1
                && peer_cancels.lock().unwrap().iter().any(|c| c == "p1")
        })
        .await;
        assert_eq!(
            kiro_cancels.load(Ordering::SeqCst),
            1,
            "disconnect must cancel the kiro source exactly once: {}",
            kiro_cancels.load(Ordering::SeqCst)
        );
        assert!(
            peer_cancels.lock().unwrap().iter().any(|c| c == "p1"),
            "disconnect must cancel the peer source: {:?}",
            peer_cancels.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn fanout_cancel_after_one_source_finished_cancels_only_survivor() {
        // Kiro finishes (its stream ends); peer stays idle. Cancel ->
        // delegation.cancel(peer) fires; backend.cancel is NOT called for the
        // already-finished kiro.
        let (backend, kiro_cancels) = CountingDoneBackend::new();
        let deleg = FakeDelegation::idle(vec![Ok(Event::status("pwork"))], Some("p1"));
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                fanout_params(),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let local = TaskId::parse("task-1").unwrap();
        wait_until(|| futures::executor::block_on(store.is_fanout(&local)).unwrap_or(false)).await;

        // Give the kiro source time to finish (its stream ends quickly).
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // POST CancelTask.
        let resp2 = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);

        // Peer (the survivor) is cancelled.
        wait_until(|| peer_cancels.lock().unwrap().iter().any(|c| c == "p1")).await;
        // The finished kiro source is a cancel no-op.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(
            kiro_cancels.load(Ordering::SeqCst),
            0,
            "a finished kiro source must NOT be cancelled"
        );
        assert!(
            peer_cancels.lock().unwrap().iter().any(|c| c == "p1"),
            "the surviving peer source must be cancelled"
        );
        let _ = body_string(resp).await;
    }

    #[tokio::test]
    async fn cancel_task_fanout_cancels_both() {
        // cancel_task() must branch on is_fanout: for a fan-out task, cancel BOTH
        // the Kiro session (backend.cancel) AND the peer (delegation.cancel),
        // each exactly once. We pre-seed the store as a fan-out task with both a
        // session and a peer mapping, so cancel_task() exercises the both-branch
        // directly (no live stream needed).
        let (backend, kiro_cancels) = CountingDoneBackend::new();
        let deleg = FakeDelegation::new(vec![Ok(Event::artifact("DONE"))], Some("p1"));
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        let local = TaskId::parse("task-1").unwrap();
        let session = SessionId::parse("session-task-1").unwrap();
        store.put(&local, &session).await.unwrap();
        store
            .set_peer_task(&local, &PeerTaskId("p1".into()))
            .await
            .unwrap();
        store.set_fanout(&local).await.unwrap();

        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        wait_until(|| {
            kiro_cancels.load(Ordering::SeqCst) >= 1
                && peer_cancels.lock().unwrap().iter().any(|c| c == "p1")
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        assert_eq!(
            kiro_cancels.load(Ordering::SeqCst),
            1,
            "fan-out cancel_task must cancel the Kiro session exactly once"
        );
        assert_eq!(
            peer_cancels.lock().unwrap().clone(),
            vec!["p1".to_string()],
            "fan-out cancel_task must cancel the peer exactly once"
        );
    }

    /// Backend whose `cancel` ALWAYS errors — used to prove the fan-out
    /// `cancel_task()` path does not orphan the peer when the Kiro cancel fails.
    struct CancelErrBackend;
    #[async_trait::async_trait]
    impl AgentBackend for CancelErrBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                stop_reason: "end_turn".into(),
            })])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Err(BridgeError::agent_crashed("test: cancel always errors"))
        }
    }

    #[tokio::test]
    async fn cancel_task_fanout_kiro_cancel_error_still_cancels_peer() {
        // Robustness: in the fan-out cancel_task() path, if the Kiro
        // `backend.cancel` returns Err, the peer cancel MUST still fire (no
        // orphaned upstream task) and the handler must still return a sensible
        // result rather than bailing before the peer-cancel block.
        let backend: Arc<dyn AgentBackend> = Arc::new(CancelErrBackend);
        let deleg = FakeDelegation::new(vec![Ok(Event::artifact("DONE"))], Some("p1"));
        let peer_cancels = deleg.cancels();
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_fanout(backend, store.clone(), deleg);

        let local = TaskId::parse("task-1").unwrap();
        let session = SessionId::parse("session-task-1").unwrap();
        store.put(&local, &session).await.unwrap();
        store
            .set_peer_task(&local, &PeerTaskId("p1".into()))
            .await
            .unwrap();
        store.set_fanout(&local).await.unwrap();

        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-1" }),
                "1.0",
            ))
            .await
            .unwrap();
        // Handler still returns a sensible (HTTP 200) JSON-RPC response.
        assert_eq!(resp.status(), StatusCode::OK);

        wait_until(|| peer_cancels.lock().unwrap().iter().any(|c| c == "p1")).await;
        assert_eq!(
            peer_cancels.lock().unwrap().clone(),
            vec!["p1".to_string()],
            "the peer must NOT be orphaned when the Kiro cancel errors"
        );
    }

    // ---- Task 10: registry-resolved local dispatch + per-session config ----

    /// A backend that records the `EffectiveConfig` it received via
    /// `configure_session` and reports a unique tag (its `id`) when prompted, so a
    /// test can prove WHICH agent's backend was driven. Yields Text(id) + Done.
    struct RecordingBackend {
        id: String,
        prompted: AtomicBool,
        /// Records the full `SessionSpec` from the most recent `configure_session` call,
        /// so tests can assert both `spec.config` (model/effort/mode) and `spec.cwd`.
        configured: Arc<Mutex<Option<SessionSpec>>>,
    }
    impl RecordingBackend {
        fn new(id: &str) -> (Arc<Self>, Arc<Mutex<Option<SessionSpec>>>) {
            let configured = Arc::new(Mutex::new(None));
            (
                Arc::new(Self {
                    id: id.to_owned(),
                    prompted: AtomicBool::new(false),
                    configured: configured.clone(),
                }),
                configured,
            )
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for RecordingBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompted.store(true, Ordering::SeqCst);
            let updates = vec![
                Ok(Update::Text(self.id.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn configure_session(
            &self,
            _session: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            *self.configured.lock().unwrap() = Some(spec.clone());
            Ok(())
        }
    }

    /// A lease that decrements a shared counter on drop — so a test can assert the
    /// slot's active-task count returns to zero after the binding is evicted.
    struct CountingLease {
        count: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Drop for CountingLease {
        fn drop(&mut self) {
            self.count.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl Lease for CountingLease {}

    /// A registry over `RecordingBackend`s that tracks a per-agent live-lease count:
    /// `resolve` increments the agent's counter (handing out a `CountingLease` that
    /// decrements on drop) so a test can assert eviction released the lease, and
    /// `apply` can REMOVE an agent so a follow-up that re-resolved it would fail —
    /// proving the follow-up uses the BOUND Arc instead. `resolve_calls` counts how
    /// many times `resolve` has been called per agent (monotonic, never decremented)
    /// so warm-reuse tests can prove a backend is never re-resolved/re-spawned.
    struct CountingRegistry {
        default: AgentId,
        inner: tokio::sync::Mutex<CountingRegistryInner>,
        leases: std::collections::HashMap<String, Arc<std::sync::atomic::AtomicUsize>>,
        /// Monotonically-increasing count of `resolve()` calls per agent key.
        resolve_calls: std::collections::HashMap<String, Arc<std::sync::atomic::AtomicUsize>>,
        /// Counts how many times a NEW backend instance was "spawned" for an agent.
        /// In this test registry the backend is pre-built, so `spawn_counts` increments
        /// only on the very first `resolve()` call for each agent and stays at 1 for all
        /// subsequent calls (modelling warm-process reuse). A real CWD-keyed-spawn
        /// regression would require a new backend per CWD → this counter would reach 2.
        spawn_counts: std::collections::HashMap<String, Arc<std::sync::atomic::AtomicUsize>>,
    }
    struct CountingRegistryInner {
        entries: std::collections::HashMap<String, AgentEntry>,
        backends: std::collections::HashMap<String, Arc<dyn AgentBackend>>,
    }
    impl CountingRegistry {
        fn new(default: &str, agents: Vec<(AgentEntry, Arc<dyn AgentBackend>)>) -> Arc<Self> {
            let mut entries = std::collections::HashMap::new();
            let mut backends = std::collections::HashMap::new();
            let mut leases = std::collections::HashMap::new();
            let mut resolve_calls = std::collections::HashMap::new();
            let mut spawn_counts = std::collections::HashMap::new();
            for (entry, backend) in agents {
                let key = entry.id.as_str().to_owned();
                leases.insert(
                    key.clone(),
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                );
                resolve_calls.insert(
                    key.clone(),
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                );
                spawn_counts.insert(
                    key.clone(),
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                );
                entries.insert(key.clone(), entry);
                backends.insert(key, backend);
            }
            Arc::new(Self {
                default: AgentId::parse(default).unwrap(),
                inner: tokio::sync::Mutex::new(CountingRegistryInner { entries, backends }),
                leases,
                resolve_calls,
                spawn_counts,
            })
        }
        /// The current live-lease count for an agent (0 once all its leases dropped).
        fn lease_count(&self, id: &str) -> usize {
            self.leases
                .get(id)
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(0)
        }
        /// Total number of `resolve()` calls ever made for `id` (monotonic — never
        /// decrements). Use this to prove a backend is NOT re-resolved across requests.
        fn resolve_call_count(&self, id: &str) -> usize {
            self.resolve_calls
                .get(id)
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(0)
        }
        /// How many times the backend for `id` was "spawned" (i.e. first activated).
        /// Stays at 1 across all warm-reuse resolves. Guards against CWD-keyed-spawn
        /// regressions: if per-request cwd were threaded into the spawn key, a second
        /// distinct cwd would produce a second backend → this count would reach 2.
        fn spawn_count(&self, id: &str) -> usize {
            self.spawn_counts
                .get(id)
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(0)
        }
    }
    #[async_trait::async_trait]
    impl AgentRegistry for CountingRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let key = id.as_str();
            let inner = self.inner.lock().await;
            match (inner.entries.get(key), inner.backends.get(key)) {
                (Some(entry), Some(backend)) => {
                    let count = self.leases.get(key).expect("agent has a lease counter");
                    count.fetch_add(1, Ordering::SeqCst);
                    if let Some(rc) = self.resolve_calls.get(key) {
                        // Increment spawn_counts only on the first ever resolve (the
                        // backend "comes alive" once; subsequent resolves are warm reuse).
                        let prev = rc.fetch_add(1, Ordering::SeqCst);
                        if prev == 0 {
                            if let Some(sc) = self.spawn_counts.get(key) {
                                sc.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                    Ok(Resolved {
                        entry: Arc::new(entry.clone()),
                        backend: backend.clone(),
                        lease: Box::new(CountingLease {
                            count: count.clone(),
                        }),
                    })
                }
                _ => Err(BridgeError::UnknownAgent { id: key.to_owned() }),
            }
        }
        fn default_id(&self) -> AgentId {
            self.default.clone()
        }
        async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            Vec::new()
        }
    }
    impl CountingRegistry {
        /// Test-only mutation standing in for an `apply` that REMOVES `id` (the real
        /// reconcile drains on the lease; here we just make a future `resolve(id)` fail).
        async fn remove_agent(&self, id: &str) {
            let mut inner = self.inner.lock().await;
            inner.entries.remove(id);
            inner.backends.remove(id);
        }
    }

    /// A `RecordingBackend`-like backend that additionally records `cancel` and
    /// `forget_session` calls, so a test can prove cancel reached the BOUND backend
    /// and that eviction forgot the per-session stash.
    struct TrackingBackend {
        id: String,
        cancelled: AtomicBool,
        forgotten: AtomicBool,
        /// Set the first time `prompt` is called — lets a test prove which backend
        /// instance a (follow-up) dispatch actually reached.
        prompted: AtomicBool,
        /// When true, `prompt` hangs forever after one Text frame (an idle stream
        /// keeping the producer — and thus the binding — alive for follow-up tests).
        idle: bool,
    }
    impl TrackingBackend {
        fn new(id: &str, idle: bool) -> Arc<Self> {
            Arc::new(Self {
                id: id.to_owned(),
                cancelled: AtomicBool::new(false),
                forgotten: AtomicBool::new(false),
                prompted: AtomicBool::new(false),
                idle,
            })
        }
        fn was_prompted(&self) -> bool {
            self.prompted.load(Ordering::SeqCst)
        }
        /// Clear the prompt flag so a later dispatch can be attributed in isolation.
        fn clear_prompted(&self) {
            self.prompted.store(false, Ordering::SeqCst);
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for TrackingBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompted.store(true, Ordering::SeqCst);
            let id = self.id.clone();
            if self.idle {
                let s = async_stream::stream! {
                    yield Ok(Update::Text(id));
                    futures::future::pending::<()>().await;
                };
                Ok(Box::pin(s))
            } else {
                let updates = vec![
                    Ok(Update::Text(id)),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ];
                Ok(Box::pin(tokio_stream::iter(updates)))
            }
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.cancelled.store(true, Ordering::SeqCst);
            Ok(())
        }
        async fn forget_session(&self, _session: &SessionId) {
            self.forgotten.store(true, Ordering::SeqCst);
        }
    }

    struct WarmRecordingBackend {
        sessions: Arc<Mutex<Vec<String>>>,
        forgotten: Arc<Mutex<Vec<String>>>,
    }

    impl WarmRecordingBackend {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                sessions: Arc::new(Mutex::new(Vec::new())),
                forgotten: Arc::new(Mutex::new(Vec::new())),
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for WarmRecordingBackend {
        async fn prompt(&self, s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            self.sessions.lock().unwrap().push(s.as_str().to_owned());
            let updates = vec![
                Ok(Update::Usage(UsageSnapshot {
                    used: Some(14584),
                    size: Some(258400),
                    cost: None,
                    at_ms: 0,
                })),
                Ok(Update::Text("warm".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session(&self, session: &SessionId) {
            self.forgotten
                .lock()
                .unwrap()
                .push(session.as_str().to_owned());
        }
    }

    struct UsageThenTextBackend;

    #[async_trait::async_trait]
    impl AgentBackend for UsageThenTextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Usage(UsageSnapshot {
                    used: Some(123),
                    size: Some(1000),
                    cost: None,
                    at_ms: 0,
                })),
                Ok(Update::Text("ok".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// A route that honors `meta.agent` (resolving to `Local(agent)`), falling back
    /// to the registry default — mirrors the real binary `SkillRoute` for the local
    /// arm so the registry-resolution path is exercised by agent id.
    struct RegistryRoute {
        default: AgentId,
    }
    impl RouteDecision for RegistryRoute {
        fn route(&self, t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Local(
                t.agent.clone().unwrap_or_else(|| self.default.clone()),
            ))
        }
    }

    /// An `AgentEntry` with a model default (used to prove base config flows).
    fn entry_with_model(id: &str, model: &str) -> AgentEntry {
        let mut e = bare_entry(id);
        e.model = Some(model.into());
        e
    }

    /// Build a server over an explicit registry + `RegistryRoute(default)`.
    fn build_registry(registry: Arc<dyn AgentRegistry>, default: &str) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            registry,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(RegistryRoute {
                default: AgentId::parse(default).unwrap(),
            }),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "a",
        ))
    }

    /// A unary SendMessage selecting `agent`, with optional model override.
    fn agent_params(agent: &str, model: Option<&str>) -> Value {
        let mut md = json!({ "a2a-bridge.agent": agent });
        if let Some(m) = model {
            md["a2a-bridge.model"] = json!(m);
        }
        json!({ "message": { "text": "go", "metadata": md } })
    }

    #[tokio::test]
    async fn local_dispatch_routes_by_agent_id() {
        // Registry has "a" and "b"; a request selecting agent="b" must drive the
        // "b" backend (its artifact text is "b"), NOT the "a"/default backend.
        let (a, _a_cfg) = RecordingBackend::new("a");
        let (b, _b_cfg) = RecordingBackend::new("b");
        let a_prompted = a.clone();
        let b_prompted = b.clone();
        let registry = FakeRegistry::with_entries(
            "a",
            vec![
                (bare_entry("a"), a as Arc<dyn AgentBackend>),
                (bare_entry("b"), b as Arc<dyn AgentBackend>),
            ],
        );
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params("b", None),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["result"]["artifact"]["text"], "b",
            "agent=b must drive the b backend: {body}"
        );
        assert!(
            b_prompted.prompted.load(Ordering::SeqCst),
            "the b backend must be prompted"
        );
        assert!(
            !a_prompted.prompted.load(Ordering::SeqCst),
            "the a/default backend must NOT be prompted"
        );
    }

    #[tokio::test]
    async fn unknown_agent_returns_clear_error() {
        // agent="zzz" is absent → a JSON-RPC error mentioning the unknown agent,
        // never a panic.
        let (a, _) = RecordingBackend::new("a");
        let registry =
            FakeRegistry::with_entries("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params("zzz", None),
                "1.0",
            ))
            .await
            .unwrap();
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v.get("error").is_some(),
            "expected a JSON-RPC error: {body}"
        );
        let msg = v["error"]["message"].as_str().unwrap_or("");
        assert!(
            msg.contains("unknown agent") && msg.contains("zzz"),
            "error must name the unknown agent: {body}"
        );
    }

    #[tokio::test]
    async fn unknown_agent_streaming_yields_failed_terminal_not_panic() {
        // Streaming counterpart: an unknown agent must produce a terminal Failed
        // SSE frame, never a panic.
        let (a, _) = RecordingBackend::new("a");
        let registry =
            FakeRegistry::with_entries("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                agent_params("zzz", None),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        assert!(
            body.contains("unknown agent"),
            "SSE error frame must mention the unknown agent: {body}"
        );
        let payloads = sse_data_payloads(&body);
        let last: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert!(
            matches!(
                &last,
                a2a::StreamResponse::StatusUpdate(e) if e.status.state == a2a::TaskState::Failed
            ),
            "final frame must be terminal statusUpdate(Failed): {}",
            payloads.last().unwrap()
        );
    }

    #[tokio::test]
    async fn override_applies_via_configure_session() {
        // agent="a" + model override → the a backend's configure_session receives
        // EffectiveConfig{ model: "override-m", .. }.
        let (a, a_cfg) = RecordingBackend::new("a");
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(entry_with_model("a", "base-m"), a as Arc<dyn AgentBackend>)],
        );
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params("a", Some("override-m")),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;

        let spec = a_cfg
            .lock()
            .unwrap()
            .clone()
            .expect("configure_session ran");
        assert_eq!(
            spec.config.model.as_deref(),
            Some("override-m"),
            "the per-request model override must reach configure_session"
        );
    }

    #[tokio::test]
    async fn base_config_applied_with_no_override() {
        // agent="a" (entry "a" has model="base-m"), NO override → configure_session
        // receives the entry's base model.
        let (a, a_cfg) = RecordingBackend::new("a");
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(entry_with_model("a", "base-m"), a as Arc<dyn AgentBackend>)],
        );
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params("a", None),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;

        let spec = a_cfg
            .lock()
            .unwrap()
            .clone()
            .expect("configure_session ran");
        assert_eq!(
            spec.config.model.as_deref(),
            Some("base-m"),
            "the entry's base model must flow even with no override"
        );
    }

    // ---- Task 11: binding-driven follow-up/cancel + RAII eviction ----

    /// Build a server over an explicit registry + `RegistryRoute(default)`, sharing
    /// the given store so a test can inspect persisted task/session state.
    fn build_registry_store(
        registry: Arc<dyn AgentRegistry>,
        store: Arc<dyn SessionStore>,
        default: &str,
    ) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            registry,
            store,
            Arc::new(AutoApprove),
            Arc::new(RegistryRoute {
                default: AgentId::parse(default).unwrap(),
            }),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "a",
        ))
    }

    #[tokio::test]
    async fn warm_continue_reuses_session_no_binding_guard() {
        let backend = WarmRecordingBackend::new();
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(bare_entry("a"), backend.clone() as Arc<dyn AgentBackend>)],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store.clone(),
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm.clone()),
        );

        let ctx = ContextId::parse("c1").unwrap();
        let params = || {
            json!({
                "message": {
                    "contextId": "c1",
                    "text": "go",
                    "metadata": { "a2a-bridge.agent": "a" }
                }
            })
        };

        let resp1 = router(srv.clone())
            .oneshot(post_request(methods::SEND_MESSAGE, params(), "1.0"))
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let _ = body_string(resp1).await;
        for _ in 0..50 {
            if matches!(
                sm.status(&ctx).await.as_ref().map(|s| s.state),
                Some("idle")
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(matches!(
            sm.status(&ctx).await.as_ref().map(|s| s.state),
            Some("idle")
        ));
        assert!(
            backend.forgotten.lock().unwrap().is_empty(),
            "warm dispatch must not install/drop a BindingGuard between turns"
        );

        let resp2 = router(srv.clone())
            .oneshot(post_request(methods::SEND_MESSAGE, params(), "1.0"))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let _ = body_string(resp2).await;
        for _ in 0..50 {
            if matches!(
                sm.status(&ctx).await.as_ref().map(|s| s.state),
                Some("idle")
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert_eq!(
            *backend.sessions.lock().unwrap(),
            vec!["ctx-c1-g0".to_string(), "ctx-c1-g0".to_string()]
        );
        assert!(
            backend.forgotten.lock().unwrap().is_empty(),
            "warm turns finish through WarmTurnGuard, not BindingGuard::forget_session"
        );
        let task = TaskId::parse("task-1").unwrap();
        assert_eq!(
            store
                .session_for(&task)
                .await
                .unwrap()
                .expect("task session stored")
                .as_str(),
            "ctx-c1-g0"
        );
    }

    #[tokio::test]
    async fn warm_streaming_records_usage_without_emitting_usage_frame() {
        let backend: Arc<dyn AgentBackend> = Arc::new(UsageThenTextBackend);
        let registry = FakeRegistry::with_entries("a", vec![(bare_entry("a"), backend)]);
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store,
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm.clone()),
        );
        let ctx = ContextId::parse("c1").unwrap();

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({
                    "message": {
                        "contextId": "c1",
                        "text": "go",
                        "metadata": { "a2a-bridge.agent": "a" }
                    }
                }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;

        let mut usage = None;
        for _ in 0..50 {
            usage = sm.status(&ctx).await.map(|s| s.usage);
            if usage.as_ref().and_then(|u| u.used) == Some(123) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let usage = usage.expect("warm handle should still exist after turn");
        assert_eq!(usage.used, Some(123));
        assert_eq!(usage.size, Some(1000));

        let payloads = sse_data_payloads(&body);
        // The normal Slice-1 frame sequence is UNCHANGED (DoD-5): the translator's Status flush
        // ("ok") + the final Artifact ("ok") + the terminal Completed. Usage adds NO frame.
        assert_eq!(
            payloads.len(),
            3,
            "usage telemetry must not add an SSE frame: {body}"
        );
        assert!(
            !body.contains("123") && !body.contains("1000"),
            "usage telemetry must not be present on the wire: {body}"
        );
        // The artifact frame carries the agent text; usage never appears as its own frame.
        let artifact = payloads
            .iter()
            .find_map(
                |p| match serde_json::from_str::<a2a::StreamResponse>(p).unwrap() {
                    a2a::StreamResponse::ArtifactUpdate(e) => Some(e),
                    _ => None,
                },
            )
            .expect("an artifact frame must be present");
        let data = serde_json::to_value(&artifact.artifact).unwrap();
        assert_eq!(data["parts"][0]["text"], "ok");
        let terminal: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert!(
            matches!(
                terminal,
                a2a::StreamResponse::StatusUpdate(e)
                    if e.status.state == a2a::TaskState::Completed
            ),
            "last payload must be terminal Completed: {}",
            payloads.last().unwrap()
        );
    }

    #[tokio::test]
    async fn warm_unary_records_usage_without_emitting_usage_on_wire() {
        // The unary (collect) recording path: a warm SEND_MESSAGE turn whose backend emits
        // Update::Usage records it on the handle, the artifact is the agent text, and usage
        // never appears in the unary response (DoD-5).
        let backend: Arc<dyn AgentBackend> = Arc::new(UsageThenTextBackend);
        let registry = FakeRegistry::with_entries("a", vec![(bare_entry("a"), backend)]);
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store,
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm.clone()),
        );
        let ctx = ContextId::parse("c-unary").unwrap();

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({
                    "message": {
                        "contextId": "c-unary",
                        "text": "go",
                        "metadata": { "a2a-bridge.agent": "a" }
                    }
                }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["result"]["artifact"]["text"], "ok",
            "unary artifact must be the agent text, usage excluded: {body}"
        );
        assert!(
            !body.contains("123") && !body.contains("1000"),
            "usage telemetry must not be present on the unary wire: {body}"
        );
        // The unary loop records usage synchronously before the response is built.
        let usage = sm
            .status(&ctx)
            .await
            .expect("warm handle should still exist after the turn")
            .usage;
        assert_eq!(usage.used, Some(123));
        assert_eq!(usage.size, Some(1000));
    }

    #[tokio::test]
    async fn session_status_release_cancel_dispatch() {
        let backend = WarmRecordingBackend::new();
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(bare_entry("a"), backend.clone() as Arc<dyn AgentBackend>)],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store,
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm.clone()),
        );
        let ctx = ContextId::parse("c1").unwrap();
        let warm_params = json!({
            "message": {
                "contextId": "c1",
                "text": "go",
                "metadata": { "a2a-bridge.agent": "a" }
            }
        });

        let resp = router(srv.clone())
            .oneshot(post_request(methods::SEND_MESSAGE, warm_params, "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;
        for _ in 0..50 {
            if matches!(
                sm.status(&ctx).await.as_ref().map(|s| s.state),
                Some("idle")
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let resp = router(srv.clone())
            .oneshot(post_request(
                "SessionStatus",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["contextId"], "c1");
        assert_eq!(v["result"]["state"], "idle");
        assert_eq!(v["result"]["agent"], "a");
        assert_eq!(v["result"]["generation"], 0);
        assert!(v["result"]["idleAgeMs"].is_number());
        assert_eq!(
            v["result"]["capabilities"],
            json!({
                "loadSession": false,
                "resume": false,
                "close": false,
                "list": false,
                "delete": false,
            })
        );
        assert_eq!(v["result"]["usage"]["used"], 14584);
        assert_eq!(v["result"]["usage"]["size"], 258400);
        let wf = v["result"]["usage"]["windowFraction"].as_f64().unwrap();
        assert!((wf - 0.0564).abs() < 1e-3, "windowFraction was {wf}");
        assert!(v["result"]["usage"]["atMs"].as_u64().unwrap() > 0);
        assert!(v["result"]["usage"]["cost"].is_null());

        let resp = router(srv.clone())
            .oneshot(post_request(
                "SessionCancel",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["contextId"], "c1");
        assert_eq!(v["result"]["canceled"], true);

        let resp = router(srv.clone())
            .oneshot(post_request(
                "SessionStatus",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["state"], "idle");

        let resp = router(srv.clone())
            .oneshot(post_request(
                "SessionRelease",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["contextId"], "c1");
        assert_eq!(v["result"]["released"], true);

        let resp = router(srv)
            .oneshot(post_request(
                "SessionStatus",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], JSONRPC_INVALID_REQUEST);
        assert_eq!(v["error"]["message"], "session not found");
    }

    #[tokio::test]
    async fn session_clear_dispatch() {
        let backend = WarmRecordingBackend::new();
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(bare_entry("a"), backend.clone() as Arc<dyn AgentBackend>)],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store,
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm.clone()),
        );
        let ctx = ContextId::parse("c1").unwrap();
        let warm_params = json!({
            "message": {
                "contextId": "c1",
                "text": "go",
                "metadata": { "a2a-bridge.agent": "a" }
            }
        });

        let resp = router(srv.clone())
            .oneshot(post_request(methods::SEND_MESSAGE, warm_params, "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;
        for _ in 0..50 {
            if matches!(
                sm.status(&ctx).await.as_ref().map(|s| s.state),
                Some("idle")
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let resp = router(srv)
            .oneshot(post_request(
                "SessionClear",
                json!({ "contextId": "c1" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            v["result"],
            json!({ "contextId": "c1", "cleared": true, "generation": 1 })
        );
    }

    #[tokio::test]
    async fn session_clear_unknown_ctx_is_not_found() {
        let backend = WarmRecordingBackend::new();
        let registry = FakeRegistry::with_entries(
            "a",
            vec![(bare_entry("a"), backend.clone() as Arc<dyn AgentBackend>)],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let registry_for_sm: Arc<dyn AgentRegistry> = registry.clone();
        let sm = Arc::new(crate::session_manager::SessionManager::new(
            registry_for_sm,
            std::time::Duration::from_secs(60),
        ));
        let registry_for_srv: Arc<dyn AgentRegistry> = registry;
        let srv = Arc::new(
            InboundServer::new(
                registry_for_srv,
                store,
                Arc::new(AutoApprove),
                Arc::new(RegistryRoute {
                    default: AgentId::parse("a").unwrap(),
                }),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "a",
            )
            .with_session_manager(sm),
        );

        let resp = router(srv)
            .oneshot(post_request(
                "SessionClear",
                json!({ "contextId": "nope" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], JSONRPC_INVALID_REQUEST);
        assert_eq!(v["error"]["message"], "session not found");
    }

    #[tokio::test]
    async fn followup_uses_bound_backend_after_registry_edit() {
        // Start a task on agent "a" using an IDLE backend so the first stream's
        // producer stays alive — the binding therefore persists. Then "remove" agent
        // "a" from the registry (a future resolve("a") would now fail). A FOLLOW-UP
        // message for the SAME task must still reach the ORIGINAL "a" backend via the
        // bound Arc — NOT unknown-agent, NOT the default. We prove it by asserting the
        // follow-up's first SSE frame carries text "a" (the bound backend's id) and
        // that ONLY the bound "a" instance was (re)prompted — never "b".
        let a = TrackingBackend::new("a", /*idle*/ true);
        let b = TrackingBackend::new("b", /*idle*/ false);
        let a_probe = a.clone();
        let b_probe = b.clone();
        let registry = CountingRegistry::new(
            "a",
            vec![
                (bare_entry("a"), a as Arc<dyn AgentBackend>),
                (bare_entry("b"), b as Arc<dyn AgentBackend>),
            ],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_registry_store(registry.clone(), store.clone(), "a");

        // First message (streaming) on agent "a" with an explicit task id so the
        // follow-up targets the same task. The idle backend keeps the producer (and
        // thus the binding) alive.
        let first = json!({
            "taskId": "t-fu",
            "message": { "text": "go", "metadata": { "a2a-bridge.agent": "a" } }
        });
        let resp = router(srv.clone())
            .oneshot(post_request(methods::SEND_STREAMING_MESSAGE, first, "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Wait until the binding exists AND the first stream's producer has actually
        // prompted "a" (bind-before-spawn means the binding can appear slightly before
        // the prompt fires).
        let task = TaskId::parse("t-fu").unwrap();
        let bindings = srv.bindings.clone();
        let task_c = task.clone();
        wait_until(|| futures::executor::block_on(bindings.lock()).contains_key(&task_c)).await;
        wait_until(|| a_probe.was_prompted()).await;

        // The first message already prompted "a". Clear the flag so the follow-up's
        // dispatch can be attributed to the bound "a" instance in isolation.
        a_probe.clear_prompted();

        // "Remove" agent "a": a future resolve("a") now fails. The bound Arc keeps the
        // original backend reachable for the follow-up.
        registry.remove_agent("a").await;

        // Follow-up for the SAME task (no agent metadata → would route to default "a"
        // and re-resolve, which now fails — but the binding short-circuits that). Drive
        // it through the STREAMING path, NOT unary: the bound "a" is an IDLE backend
        // (one short Text frame, then parks forever with no terminal Done). The unary
        // path does `translator.run(...).collect().await`, which on an idle backend
        // never resolves → a suite-blocking hang. The streaming path instead spawns the
        // producer (which calls `prompt` on the BOUND backend) and returns immediately;
        // we never read the body to completion.
        //
        // We prove the follow-up reached the ORIGINAL bound "a" instance — not the
        // default/unknown re-resolve, not the other agent "b" — by observing that the
        // bound "a" `TrackingBackend` was (re)prompted while "b" was not. The whole
        // follow-up is wrapped in a 5s timeout so any regression (re-resolve failure,
        // wrong backend, or a real hang) fails fast instead of blocking the suite.
        let followup = json!({ "taskId": "t-fu", "message": { "text": "again" } });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let resp = router(srv.clone())
                .oneshot(post_request(
                    methods::SEND_STREAMING_MESSAGE,
                    followup,
                    "1.0",
                ))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            // The producer is spawned eagerly; wait until the bound "a" backend is
            // (re)prompted. Drop the never-terminating SSE body — we do not read it.
            wait_until(|| a_probe.was_prompted()).await;
            drop(resp);
        })
        .await
        .expect("follow-up must (re)prompt the BOUND 'a' backend within 5s (no hang)");

        // The follow-up reached the ORIGINAL bound "a" instance (it was re-prompted)
        // and NOT the other agent "b" (proving it wasn't a re-resolve to default/other).
        assert!(
            a_probe.was_prompted(),
            "follow-up must (re)prompt the BOUND 'a' backend, not default/unknown"
        );
        assert!(
            !b_probe.was_prompted(),
            "follow-up must NOT reach the other 'b' backend"
        );

        // Both idle streams are still open; drop server-side state by ending.
        drop(srv);
    }

    #[tokio::test]
    async fn cancel_uses_bound_backend_not_default() {
        // Start a task on the NON-default agent "b" (binding created). An inbound
        // tasks/cancel must cancel the "b" backend (the bound instance), NOT the
        // default "a".
        let a = TrackingBackend::new("a", /*idle*/ false);
        let b = TrackingBackend::new("b", /*idle*/ true);
        let a_probe = a.clone();
        let b_probe = b.clone();
        let registry = CountingRegistry::new(
            "a",
            vec![
                (bare_entry("a"), a as Arc<dyn AgentBackend>),
                (bare_entry("b"), b as Arc<dyn AgentBackend>),
            ],
        );
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_registry_store(registry, store, "a");

        // First (streaming, idle) message on agent "b" → binding for "b".
        let first = json!({
            "taskId": "t-cb",
            "message": { "text": "go", "metadata": { "a2a-bridge.agent": "b" } }
        });
        let resp = router(srv.clone())
            .oneshot(post_request(methods::SEND_STREAMING_MESSAGE, first, "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let task = TaskId::parse("t-cb").unwrap();
        let bindings = srv.bindings.clone();
        let task_c = task.clone();
        wait_until(|| futures::executor::block_on(bindings.lock()).contains_key(&task_c)).await;

        // Cancel the task.
        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "t-cb" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        wait_until(|| b_probe.cancelled.load(Ordering::SeqCst)).await;
        assert!(
            b_probe.cancelled.load(Ordering::SeqCst),
            "cancel must reach the BOUND 'b' backend"
        );
        assert!(
            !a_probe.cancelled.load(Ordering::SeqCst),
            "cancel must NOT reach the default 'a' backend"
        );
        drop(srv);
    }

    #[tokio::test]
    async fn binding_and_lease_evicted_on_producer_exit() {
        // Start AND complete a local task (non-idle backend → producer exits cleanly).
        // After the producer exits, the binding is removed, the slot's lease is
        // released (lease_count == 0), and the per-session stash is forgotten.
        let a = TrackingBackend::new("a", /*idle*/ false);
        let a_probe = a.clone();
        let registry =
            CountingRegistry::new("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_registry_store(registry.clone(), store, "a");

        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "taskId": "t-ev", "message": { "text": "go" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // Drain the stream to completion so the producer exits.
        let _ = body_string(resp).await;

        let task = TaskId::parse("t-ev").unwrap();
        let bindings = srv.bindings.clone();
        let task_c = task.clone();
        // Eviction is async (spawned from the guard's Drop); poll for it.
        wait_until(|| !futures::executor::block_on(bindings.lock()).contains_key(&task_c)).await;
        wait_until(|| registry.lease_count("a") == 0).await;
        assert_eq!(
            registry.lease_count("a"),
            0,
            "the slot's lease must be released after the producer exits"
        );
        wait_until(|| a_probe.forgotten.load(Ordering::SeqCst)).await;
        assert!(
            a_probe.forgotten.load(Ordering::SeqCst),
            "eviction must forget the per-session stash"
        );
    }

    #[tokio::test]
    async fn binding_evicted_on_early_disconnect() {
        // Start a streaming task with an IDLE backend (producer stays alive), then
        // drop the SSE receiver mid-stream (client disconnect). The RAII guard must
        // still fire: binding removed AND lease released — proving eviction on the
        // NON-clean exit path.
        let a = TrackingBackend::new("a", /*idle*/ true);
        let registry =
            CountingRegistry::new("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_registry_store(registry.clone(), store, "a");

        let resp = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "taskId": "t-dc", "message": { "text": "go" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let task = TaskId::parse("t-dc").unwrap();
        let bindings = srv.bindings.clone();
        let task_c = task.clone();
        wait_until(|| futures::executor::block_on(bindings.lock()).contains_key(&task_c)).await;
        assert_eq!(
            registry.lease_count("a"),
            1,
            "lease held while task is live"
        );

        // Drop the response (SSE receiver) → client disconnect mid-stream.
        drop(resp);

        let task_c2 = task.clone();
        wait_until(|| !futures::executor::block_on(bindings.lock()).contains_key(&task_c2)).await;
        wait_until(|| registry.lease_count("a") == 0).await;
        assert_eq!(
            registry.lease_count("a"),
            0,
            "the RAII guard must release the lease on early disconnect"
        );
    }

    // ---- Task 5 (session_cwd increment): parse + validate a2a-bridge.cwd ----

    #[test]
    fn cwd_metadata_parsed() {
        // Present + valid absolute path → Some(SessionCwd) matching the value.
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.cwd": "/abs/repo"
                }
            }
        });
        let cwd = session_cwd_from_params(&p, None)
            .expect("valid absolute cwd must parse OK")
            .expect("present cwd key must return Some");
        assert_eq!(
            cwd,
            bridge_core::SessionCwd::parse("/abs/repo").unwrap(),
            "parsed SessionCwd must equal the expected value"
        );
        // Absent key → None (no behavior change for callers that omit it).
        let p_absent = serde_json::json!({ "message": { "text": "hi" } });
        assert!(
            session_cwd_from_params(&p_absent, None)
                .expect("absent cwd must not error")
                .is_none(),
            "absent a2a-bridge.cwd must yield None"
        );
    }

    #[test]
    fn cwd_relative_rejected() {
        // A relative path must be rejected before reaching the backend.
        let p = serde_json::json!({
            "message": {
                "metadata": {
                    "a2a-bridge.cwd": "rel/path"
                }
            }
        });
        match session_cwd_from_params(&p, None) {
            Err(BridgeError::InvalidRequest { field: "a2a-bridge.cwd" }) => {}
            other => panic!(
                "relative cwd must return InvalidRequest{{field:\"a2a-bridge.cwd\"}}, got: {other:?}"
            ),
        }
    }

    #[test]
    fn cwd_allowed_root_enforced() {
        let mk = |cwd: &str| {
            serde_json::json!({
                "message": {
                    "metadata": { "a2a-bridge.cwd": cwd }
                }
            })
        };

        // /work/r is under /work → accepted.
        assert!(
            session_cwd_from_params(&mk("/work/r"), Some("/work"))
                .expect("/work/r under /work must be OK")
                .is_some(),
            "/work/r must be accepted when root=/work"
        );

        // /other is NOT under /work → rejected.
        match session_cwd_from_params(&mk("/other"), Some("/work")) {
            Err(BridgeError::InvalidRequest { field: "a2a-bridge.cwd" }) => {}
            other => panic!(
                "/other outside /work must return InvalidRequest{{field:\"a2a-bridge.cwd\"}}, got: {other:?}"
            ),
        }

        // /work-evil is not under /work (component-wise check must not match str prefix).
        match session_cwd_from_params(&mk("/work-evil"), Some("/work")) {
            Err(BridgeError::InvalidRequest {
                field: "a2a-bridge.cwd",
            }) => {}
            other => panic!(
                "/work-evil must be rejected (component-wise, not str-prefix), got: {other:?}"
            ),
        }
    }

    // ---- Task 6: single-agent dispatch applies per-request cwd + warm-reuse ----

    /// Build params for a unary SendMessage selecting `agent`, with optional model
    /// override and optional per-request cwd (Task 6 dispatch tests).
    fn agent_params_cwd(agent: &str, model: Option<&str>, cwd: Option<&str>) -> Value {
        agent_params_cwd_task(agent, model, cwd, None)
    }

    /// Like `agent_params_cwd` but also injects an explicit `taskId` into the params
    /// so the request bypasses the "task-1" fallback and hits its own binding slot.
    /// Pass distinct task ids to prevent two requests from sharing the binding cache.
    fn agent_params_cwd_task(
        agent: &str,
        model: Option<&str>,
        cwd: Option<&str>,
        task_id: Option<&str>,
    ) -> Value {
        let mut md = json!({ "a2a-bridge.agent": agent });
        if let Some(m) = model {
            md["a2a-bridge.model"] = json!(m);
        }
        if let Some(c) = cwd {
            md["a2a-bridge.cwd"] = json!(c);
        }
        let mut params = json!({ "message": { "text": "go", "metadata": md } });
        if let Some(tid) = task_id {
            params["taskId"] = json!(tid);
        }
        params
    }

    #[tokio::test]
    async fn single_agent_dispatch_applies_cwd() {
        // A unary message/send with `a2a-bridge.cwd="/req"` must cause
        // `configure_session` to receive a `SessionSpec` where `cwd ==
        // Some(SessionCwd::parse("/req"))`. This proves the per-request cwd flows
        // from `RoutedCall.session_cwd` all the way into the backend mint call.
        let (a, a_spec) = RecordingBackend::new("a");
        let registry =
            FakeRegistry::with_entries("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let srv = build_registry(registry, "a");

        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params_cwd("a", None, Some("/req")),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = body_string(resp).await;

        let spec = a_spec
            .lock()
            .unwrap()
            .clone()
            .expect("configure_session must have run");
        assert_eq!(
            spec.cwd,
            Some(bridge_core::SessionCwd::parse("/req").unwrap()),
            "per-request cwd must reach configure_session as spec.cwd"
        );
    }

    #[tokio::test]
    async fn per_request_cwd_does_not_respawn() {
        // Two sequential unary sends to the SAME agent, each carrying a DIFFERENT
        // `a2a-bridge.cwd`, and each carrying a DISTINCT `taskId` so neither request
        // hits the other's binding cache — both independently exercise the full
        // resolve path through the registry.
        //
        // The invariant: the backend is "spawned" (first-activated) exactly ONCE.
        // Both requests see the same warm process slot even though the cwd differs,
        // because `session_cwd` is a mint-time session param, NOT part of the
        // backend's spawn/identity key.
        //
        // Regression guard: if per-request cwd were (wrongly) threaded into the
        // spawn key, a second distinct cwd would require a second backend instance →
        // `spawn_count` would reach 2, failing this assertion.
        let a = TrackingBackend::new("a", /*idle*/ false);
        let registry =
            CountingRegistry::new("a", vec![(bare_entry("a"), a as Arc<dyn AgentBackend>)]);
        let store: Arc<dyn SessionStore> = Arc::new(FakeStore::default());
        let srv = build_registry_store(registry.clone(), store, "a");

        // Request 1: task-a, cwd=/repo-a.  Goes through the full resolve path
        // (no prior binding) and activates the backend for the first time.
        let resp1 = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params_cwd_task("a", None, Some("/repo-a"), Some("task-a")),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp1.status(), StatusCode::OK);
        let _ = body_string(resp1).await;

        // Request 2: task-b, cwd=/repo-b — a DIFFERENT task id and a DIFFERENT cwd.
        // Because the task id differs, this request has no binding cache entry and
        // also goes through the full resolve path.  The backend must be REUSED, not
        // re-spawned, proving cwd is not part of the spawn identity key.
        let resp2 = router(srv.clone())
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                agent_params_cwd_task("a", None, Some("/repo-b"), Some("task-b")),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let _ = body_string(resp2).await;

        // Both requests went through resolve() (2 calls total — one per new task).
        assert_eq!(
            registry.resolve_call_count("a"),
            2,
            "each distinct task must go through resolve() independently"
        );
        // But the backend was only ever spawned ONCE — the second resolve reused the
        // warm process slot.  Would be 2 if cwd were (wrongly) a spawn key.
        assert_eq!(
            registry.spawn_count("a"),
            1,
            "changing a2a-bridge.cwd must NOT spawn a new backend; \
             the warm process is shared across distinct cwds (only the ACP session mint differs)"
        );
    }

    // ---- Task 7: SubscribeToTask dispatch + handler skeleton ----

    use bridge_core::task_store::TaskStore as _;

    /// Build a server with a MemoryTaskStore for the reattach tests (the durable
    /// task_store is what `subscribe_to_task` consults for task lookup).
    fn build_with_task_store(
        task_store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    ) -> Arc<InboundServer> {
        Arc::new(
            InboundServer::new(
                FakeRegistry::single("kiro", FakeBackend::new()),
                Arc::new(FakeStore::default()),
                Arc::new(AutoApprove),
                Arc::new(AlwaysKiro),
                Arc::new(AlwaysGrant),
                "http://localhost:8080",
                Arc::new(NoDelegation),
                "kiro",
            )
            .with_task_store(task_store),
        )
    }

    /// (a) SubscribeToTask with NO task id (neither `id` nor `taskId` in params)
    /// must return a JSON-RPC invalid-request error.
    #[tokio::test]
    async fn subscribe_to_task_missing_id_returns_error() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(methods::SUBSCRIBE_TO_TASK, json!({}), "1.0"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v.get("error").is_some(),
            "expected a JSON-RPC error for missing task id, got: {body}"
        );
    }

    /// (b) SubscribeToTask with an unknown task id must return a not-found JSON-RPC error.
    #[tokio::test]
    async fn subscribe_to_task_unknown_id_returns_not_found() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let srv = build_with_task_store(store);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "no-such-task" }),
                "1.0",
            ))
            .await
            .unwrap();
        // not-found maps to RejectRequest => INVALID_REQUEST => 400
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v.get("error").is_some(),
            "expected a JSON-RPC error for unknown task id, got: {body}"
        );
    }

    /// (c) I3 wire-conformance: a SubscribeToTask with the standard a2a-lf field
    /// `{"id": "<known-task-id>"}` (a task that exists in the store) is ACCEPTED —
    /// the response is an event-stream (SSE), NOT a JSON-RPC error.
    /// This test would FAIL if the handler read only `taskId`.
    #[tokio::test]
    async fn subscribe_to_task_standard_id_field_accepted() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        // Create a task record in the store so the lookup succeeds.
        let now = crate::workflow_sink::now_ms();
        let rec = bridge_core::task_store::TaskRecord {
            id: TaskId::parse("test-task-7c").unwrap(),
            workflow: "test-wf".to_string(),
            status: bridge_core::task_store::TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: now,
            updated_ms: now,
            input: "test input".to_string(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        };
        store.create(&rec).await.unwrap();

        let srv = build_with_task_store(store);
        // Working task: the working flow requires a live progress hub (Task 9). Insert
        // one so the request is routed to the working flow and yields an SSE response.
        insert_hub(&srv, &TaskId::parse("test-task-7c").unwrap()).await;
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "test-task-7c" }),
                "1.0",
            ))
            .await
            .unwrap();
        // Must succeed (SSE response, NOT a JSON-RPC error).
        assert_eq!(resp.status(), StatusCode::OK);
        // The response must be an event-stream, not JSON.
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/event-stream"),
            "expected event-stream content type, got: {content_type}"
        );
    }

    /// (c2) I3 lenient alias: a request carrying the legacy `taskId` field (instead of
    /// the standard a2a-lf `id`) is still accepted — locks the `.or_else(taskId)` fallback
    /// so a refactor that drops it is caught.
    #[tokio::test]
    async fn subscribe_to_task_legacy_task_id_alias_accepted() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let now = crate::workflow_sink::now_ms();
        let rec = bridge_core::task_store::TaskRecord {
            id: TaskId::parse("test-task-7c2").unwrap(),
            workflow: "test-wf".to_string(),
            status: bridge_core::task_store::TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: now,
            updated_ms: now,
            input: "test input".to_string(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        };
        store.create(&rec).await.unwrap();

        let srv = build_with_task_store(store);
        // Working task: the working flow requires a live progress hub (Task 9).
        insert_hub(&srv, &TaskId::parse("test-task-7c2").unwrap()).await;
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "taskId": "test-task-7c2" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/event-stream"),
            "the taskId alias must be accepted; got content type: {content_type}"
        );
    }

    /// (d) M5: a task id that is NOT in the TaskStore must return a not-found error
    /// without falling through to gate() and starting a new run. This distinguishes
    /// subscribe_to_task from stream_message (which would mint a new run for any id).
    #[tokio::test]
    async fn subscribe_to_task_unknown_id_no_gate_fallthrough() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let srv = build_with_task_store(store.clone());
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "phantom-task" }),
                "1.0",
            ))
            .await
            .unwrap();
        // Must be a not-found error (no run started).
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(
            v.get("error").is_some(),
            "expected a JSON-RPC error for phantom task id, got: {body}"
        );
        // Confirm no new task was created in the store (gate() was NOT called).
        let tasks = store.list(100).await.unwrap();
        assert!(
            tasks.is_empty(),
            "no task should have been created in the store: {:?}",
            tasks
        );
    }

    // ---- Task 8: snapshot builder + terminal-state flow ----

    /// Build a `post_request` and attach a `Last-Event-ID` header for the cursor.
    fn post_request_with_cursor(
        method: &str,
        params: Value,
        version: &str,
        cursor: i64,
    ) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .header(SVC_PARAM_VERSION, version)
            .header("Last-Event-ID", cursor.to_string())
            .body(jsonrpc_body(method, params))
            .unwrap()
    }

    /// Parse an SSE body into an ordered `Vec<(seq, kind_tag)>`.
    /// Each SSE event block is separated by a blank line; we look for:
    ///   `id: <seq>` and `data: <json>` (extracting the `"kind"` field).
    async fn collect_sse_frames(resp: Response) -> Vec<(i64, String)> {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        let mut result = Vec::new();
        // Split on blank lines to get event blocks.
        for block in body.split("\n\n") {
            let mut seq: Option<i64> = None;
            let mut kind: Option<String> = None;
            for line in block.lines() {
                if let Some(id_str) = line.strip_prefix("id:") {
                    seq = id_str.trim().parse().ok();
                } else if let Some(data_str) = line.strip_prefix("data:") {
                    let trimmed = data_str.trim();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        if let Some(k) = v.get("kind").and_then(|k| k.as_str()) {
                            kind = Some(k.to_string());
                        }
                    }
                }
            }
            if let (Some(s), Some(k)) = (seq, kind) {
                result.push((s, k));
            }
        }
        result
    }

    /// Seed a `MemoryTaskStore` task record in the Working state.
    async fn seed_task_record(
        store: &std::sync::Arc<bridge_core::task_store::MemoryTaskStore>,
        task_id: &str,
    ) -> bridge_core::ids::TaskId {
        let id = bridge_core::ids::TaskId::parse(task_id).unwrap();
        let now = crate::workflow_sink::now_ms();
        store
            .create(&bridge_core::task_store::TaskRecord {
                id: id.clone(),
                workflow: "code-review".to_string(),
                status: bridge_core::task_store::TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: now,
                updated_ms: now,
                input: "test input".to_string(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
            })
            .await
            .unwrap();
        id
    }

    /// (8a) Terminal task with 2 checkpoints (seqs 1, 2) + terminal_seq 3, no cursor.
    /// Expected frames: [(1,"node_finished"),(2,"node_finished"),(2,"snapshot_complete"),(3,"terminal")]
    #[tokio::test]
    async fn subscribe_terminal_task_no_cursor_yields_snapshot_then_terminal() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t8a").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let node_b = bridge_core::ids::NodeId::parse("node-b").unwrap();
        let now = crate::workflow_sink::now_ms();
        // seq 1: node-a finished; seq 2: node-b finished; seq 3: terminal.
        let s1 = store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap();
        let s2 = store
            .put_node_checkpoint_sequenced(&task_id, &node_b, "out-b", true, now)
            .await
            .unwrap();
        let s3 = store
            .set_terminal_sequenced(
                &task_id,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                now,
            )
            .await
            .unwrap();
        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        assert_eq!(s3, 3);

        let srv = build_with_task_store(store);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "t8a" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![
                (1, "node_finished".to_string()),
                (2, "node_finished".to_string()),
                (2, "snapshot_complete".to_string()),
                (3, "terminal".to_string()),
            ],
            "expected ordered snapshot+terminal frames: {frames:?}"
        );
    }

    /// (8b) Same terminal task, cursor=1 → only frames with seq > 1 are emitted.
    /// Expected: [(2,"node_finished"),(2,"snapshot_complete"),(3,"terminal")]
    #[tokio::test]
    async fn subscribe_terminal_task_cursor_1_filters_seq_gt_1() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t8b").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let node_b = bridge_core::ids::NodeId::parse("node-b").unwrap();
        let now = crate::workflow_sink::now_ms();
        store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap(); // seq=1
        store
            .put_node_checkpoint_sequenced(&task_id, &node_b, "out-b", true, now)
            .await
            .unwrap(); // seq=2
        store
            .set_terminal_sequenced(
                &task_id,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                now,
            )
            .await
            .unwrap(); // seq=3

        let srv = build_with_task_store(store);
        let resp = router(srv)
            .oneshot(post_request_with_cursor(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "t8b" }),
                "1.0",
                1, // cursor: Last-Event-ID = 1 → only seq > 1
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![
                (2, "node_finished".to_string()),
                (2, "snapshot_complete".to_string()),
                (3, "terminal".to_string()),
            ],
            "cursor=1 should filter out seq=1 frame: {frames:?}"
        );
    }

    /// (8c) I2 NULL-seq: terminal task whose checkpoint was written via legacy
    /// `put_node_checkpoint` (seq=0) and terminal via legacy `set_terminal` (terminal_seq=None).
    /// No cursor → the (0,"node_finished") IS delivered AND a terminal frame is still emitted.
    #[tokio::test]
    async fn subscribe_terminal_task_legacy_null_seq_delivers_seq0_and_terminal() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t8c").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let now = crate::workflow_sink::now_ms();
        // Legacy put_node_checkpoint writes seq=0 (stored as 0 in the CheckpointValue).
        store
            .put_node_checkpoint(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap();
        // Legacy set_terminal: terminal_seq=None (not stored in terminal_seqs).
        store
            .set_terminal(
                &task_id,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                now,
            )
            .await
            .unwrap();

        let srv = build_with_task_store(store);
        let resp = router(srv)
            .oneshot(post_request(
                methods::SUBSCRIBE_TO_TASK,
                json!({ "id": "t8c" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let frames = collect_sse_frames(resp).await;
        // seq=0 node_finished MUST be delivered (cursor absent → pass everything incl seq 0)
        // terminal_seq=None → always emit terminal (seq=0 for the terminal frame since terminal_seq is None).
        assert_eq!(
            frames,
            vec![
                (0, "node_finished".to_string()),
                (0, "snapshot_complete".to_string()),
                (0, "terminal".to_string()),
            ],
            "NULL-seq legacy task: seq0 node_finished AND terminal must be delivered: {frames:?}"
        );
    }

    /// Confirm SendStreamingMessage STILL routes to stream_message (produces SSE,
    /// not a JSON-RPC error). This verifies the dispatch split did not break the
    /// existing streaming path.
    #[tokio::test]
    async fn send_streaming_message_still_routes_to_stream_message() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/event-stream"),
            "SendStreamingMessage must still produce SSE: {content_type}"
        );
        let body = body_string(resp).await;
        assert!(
            body.contains("PONG"),
            "SendStreamingMessage must still route to stream_message and get PONG: {body}"
        );
    }

    // ---- Task 9: working-state flow (subscribe-first + cut_seq dedup + live-tail) ----

    /// Build a live `NodeFinished` frame for a hub publish at the given seq.
    fn live_node_finished(seq: i64, node: &str) -> crate::reattach::WorkflowProgressFrame {
        crate::reattach::WorkflowProgressFrame {
            v: 1,
            seq,
            phase: crate::reattach::Phase::Live,
            kind: crate::reattach::FrameKind::NodeFinished {
                node: node.to_string(),
                ok: true,
                output: "live".to_string(),
            },
        }
    }

    /// Build a live `Terminal` frame for a hub publish at the given seq.
    fn live_terminal(seq: i64) -> crate::reattach::WorkflowProgressFrame {
        crate::reattach::WorkflowProgressFrame {
            v: 1,
            seq,
            phase: crate::reattach::Phase::Live,
            kind: crate::reattach::FrameKind::Terminal {
                outcome: crate::reattach::TerminalOutcome::Completed,
                output: "done".to_string(),
            },
        }
    }

    /// Call the `subscribe_to_task` handler DIRECTLY (not via the router) so the test
    /// can publish live frames AFTER the handler returns — the handler subscribes
    /// synchronously, so frames published after the call still land in the response
    /// stream's buffered receiver.
    async fn call_subscribe(
        srv: &Arc<InboundServer>,
        task_id: &str,
        cursor: Option<i64>,
    ) -> Response {
        let mut headers = HeaderMap::new();
        headers.insert(SVC_PARAM_VERSION, "1.0".parse().unwrap());
        if let Some(k) = cursor {
            headers.insert("Last-Event-ID", k.to_string().parse().unwrap());
        }
        subscribe_to_task(srv.clone(), headers, json!(1), json!({ "id": task_id })).await
    }

    /// Insert a fresh hub for `task` into the server's `progress_hubs` and return it
    /// so the test can publish live frames to the same hub the handler subscribes to.
    async fn insert_hub(
        srv: &Arc<InboundServer>,
        task: &TaskId,
    ) -> Arc<crate::reattach::TaskProgressHub> {
        let hub = Arc::new(crate::reattach::TaskProgressHub::new());
        srv.progress_hubs
            .lock()
            .await
            .insert(task.clone(), hub.clone());
        hub
    }

    /// (9a) In-flight: snapshot checkpoints (seqs 1,2), then live NodeFinished(3) +
    /// Terminal(4). Expected ordered vector with NO dup of 1,2 in the live tail, NO gap.
    #[tokio::test]
    async fn subscribe_working_in_flight_snapshot_then_live_tail() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9a").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let node_b = bridge_core::ids::NodeId::parse("node-b").unwrap();
        let now = crate::workflow_sink::now_ms();
        // Durable snapshot: seqs 1,2 are finished checkpoints (task stays Working).
        let s1 = store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap();
        let s2 = store
            .put_node_checkpoint_sequenced(&task_id, &node_b, "out-b", true, now)
            .await
            .unwrap();
        assert_eq!((s1, s2), (1, 2));

        let srv = build_with_task_store(store);
        let hub = insert_hub(&srv, &task_id).await;

        // Subscribe synchronously (the handler subscribes before returning).
        let resp = call_subscribe(&srv, "t9a", None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Now publish the live tail (buffers in the response stream's receiver).
        hub.publish(live_node_finished(3, "node-c"));
        hub.publish(live_terminal(4));

        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![
                (1, "node_finished".to_string()),
                (2, "node_finished".to_string()),
                (2, "snapshot_complete".to_string()),
                (3, "node_finished".to_string()),
                (4, "terminal".to_string()),
            ],
            "in-flight: snapshot then live tail, no dup/gap: {frames:?}"
        );
    }

    /// (9a2) A Working task with an IN-PROGRESS node — a `record_node_started` with no
    /// matching checkpoint — surfaces as a `node_started` frame in the snapshot. This
    /// exercises the `snap.starts → NodeStarted` branch of `snapshot_frames` (terminal
    /// tasks clear their starts, so this is the only path that reaches it).
    #[tokio::test]
    async fn subscribe_working_snapshot_includes_in_progress_start() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9a2").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let node_x = bridge_core::ids::NodeId::parse("node-x").unwrap();
        let now = crate::workflow_sink::now_ms();
        // node-a finished (seq 1); node-x started but NOT finished (seq 2, stays in starts).
        let s1 = store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap();
        let s2 = store
            .record_node_started(&task_id, &node_x, now)
            .await
            .unwrap();
        assert_eq!((s1, s2), (1, 2));

        let srv = build_with_task_store(store);
        let hub = insert_hub(&srv, &task_id).await;
        let resp = call_subscribe(&srv, "t9a2", None).await;
        assert_eq!(resp.status(), StatusCode::OK);
        // Finish the in-flight run so the stream closes.
        hub.publish(live_terminal(3));

        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![
                (1, "node_finished".to_string()),
                (2, "node_started".to_string()),
                (2, "snapshot_complete".to_string()),
                (3, "terminal".to_string()),
            ],
            "snapshot must include the in-progress node as node_started: {frames:?}"
        );
    }

    /// (9b) Two concurrent subscribers each subscribe their own rx; publish the live
    /// frames ONCE; both collected bodies equal the full ordered vector.
    #[tokio::test]
    async fn subscribe_working_two_concurrent_subscribers() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9b").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let now = crate::workflow_sink::now_ms();
        store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap(); // seq 1

        let srv = build_with_task_store(store);
        let hub = insert_hub(&srv, &task_id).await;

        // Two subscribers, each gets its own rx (subscribe-first).
        let resp1 = call_subscribe(&srv, "t9b", None).await;
        let resp2 = call_subscribe(&srv, "t9b", None).await;
        assert_eq!(resp1.status(), StatusCode::OK);
        assert_eq!(resp2.status(), StatusCode::OK);

        // Publish the live frames ONCE — broadcast fans out to both receivers.
        hub.publish(live_node_finished(2, "node-b"));
        hub.publish(live_terminal(3));

        let expected = vec![
            (1, "node_finished".to_string()),
            (1, "snapshot_complete".to_string()),
            (2, "node_finished".to_string()),
            (3, "terminal".to_string()),
        ];
        let f1 = collect_sse_frames(resp1).await;
        let f2 = collect_sse_frames(resp2).await;
        assert_eq!(f1, expected, "subscriber 1: {f1:?}");
        assert_eq!(f2, expected, "subscriber 2: {f2:?}");
    }

    /// (9c) Empty snapshot: Working task, no checkpoints/starts → SnapshotComplete
    /// at seq=cut_seq(0), then the live tail, then Terminal.
    #[tokio::test]
    async fn subscribe_working_empty_snapshot_emits_snapshot_complete_then_live() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9c").await;

        let srv = build_with_task_store(store);
        let hub = insert_hub(&srv, &task_id).await;

        let resp = call_subscribe(&srv, "t9c", None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // dedup_floor = max(-1, cut_seq=0) = 0 → live frames must have seq > 0.
        hub.publish(live_node_finished(1, "node-a"));
        hub.publish(live_terminal(2));

        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![
                (0, "snapshot_complete".to_string()),
                (1, "node_finished".to_string()),
                (2, "terminal".to_string()),
            ],
            "empty snapshot: snapshot_complete(cut_seq) then live tail: {frames:?}"
        );
    }

    /// (9d) Cursor >= cut_seq on a TERMINAL task → SnapshotComplete then immediate
    /// close (no hang). The terminal branch with a high cursor must not block.
    #[tokio::test]
    async fn subscribe_terminal_high_cursor_snapshot_complete_then_close() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9d").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let now = crate::workflow_sink::now_ms();
        store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap(); // seq 1
        let ts = store
            .set_terminal_sequenced(
                &task_id,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                now,
            )
            .await
            .unwrap(); // seq 2 (terminal_seq)
        assert_eq!(ts, 2);

        let srv = build_with_task_store(store);
        // No hub needed (terminal branch). cursor=5 >= cut_seq=2 and >= terminal_seq=2.
        let resp = call_subscribe(&srv, "t9d", Some(5)).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Must NOT hang: collect returns. All snapshot frames are filtered by the
        // high cursor, the terminal frame is suppressed (cursor >= terminal_seq),
        // leaving only SnapshotComplete (its seq = cut_seq when no frames pass).
        let frames = collect_sse_frames(resp).await;
        assert_eq!(
            frames,
            vec![(2, "snapshot_complete".to_string())],
            "high cursor terminal: snapshot_complete only, no hang: {frames:?}"
        );
    }

    /// (9e) I5 terminal-during-snapshot: insert the hub, write snapshot checkpoints,
    /// THEN set the task terminal, THEN call the handler. `get` returns the terminal
    /// rec → the terminal branch runs. Assertion: a Terminal frame IS delivered and
    /// the stream closes (never hangs / closes-without-terminal).
    #[tokio::test]
    async fn subscribe_terminal_during_snapshot_delivers_terminal() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9e").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let now = crate::workflow_sink::now_ms();
        let srv = build_with_task_store(store.clone());
        // Insert the hub (as a live runner would), write snapshot checkpoints...
        let _hub = insert_hub(&srv, &task_id).await;
        store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap(); // seq 1
                       // ...then the task finishes (terminal written) BEFORE the handler is called.
        let ts = store
            .set_terminal_sequenced(
                &task_id,
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some("done"),
                None,
                now,
            )
            .await
            .unwrap(); // seq 2

        let resp = call_subscribe(&srv, "t9e", None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let frames = collect_sse_frames(resp).await;
        // Must terminate with a Terminal frame (never hang / close without terminal).
        assert_eq!(
            frames.last(),
            Some(&(ts, "terminal".to_string())),
            "terminal-during-snapshot must end with a Terminal frame: {frames:?}"
        );
    }

    /// (9f) I7 broadcast-lag: subscribe, then publish MORE than the channel capacity
    /// (300 > 256) to force the receiver to lag → the body ends with a retryable
    /// `event: error` SSE event, then closes. A fresh re-subscribe with the last-seen
    /// cursor still yields a coherent snapshot.
    #[tokio::test]
    async fn subscribe_working_broadcast_lag_yields_retryable_error_then_close() {
        let store = std::sync::Arc::new(bridge_core::task_store::MemoryTaskStore::new());
        let task_id = seed_task_record(&store, "t9f").await;
        let node_a = bridge_core::ids::NodeId::parse("node-a").unwrap();
        let now = crate::workflow_sink::now_ms();
        store
            .put_node_checkpoint_sequenced(&task_id, &node_a, "out-a", true, now)
            .await
            .unwrap(); // seq 1

        let srv = build_with_task_store(store.clone());
        let hub = insert_hub(&srv, &task_id).await;

        let resp = call_subscribe(&srv, "t9f", None).await;
        assert_eq!(resp.status(), StatusCode::OK);

        // Publish 300 frames BEFORE the stream is drained → the receiver (cap 256)
        // overflows → RecvError::Lagged on the first live recv.
        for s in 2..302 {
            hub.publish(live_node_finished(s, "node-x"));
        }

        // Collect the raw body and assert the retryable error event closes the stream.
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            body.contains("event: error") || body.contains("event:error"),
            "lag must emit an SSE error event: {body}"
        );
        assert!(
            body.contains("retryable") && body.contains("lagged"),
            "lag error event must carry retryable/lagged: {body}"
        );

        // A fresh re-subscribe with the last-seen cursor re-snapshots coherently.
        // (The durable snapshot makes reconnect lossless — the snapshot still
        // reports the Working task and its checkpoint.)
        let snap = store.progress_snapshot(&task_id).await.unwrap();
        assert_eq!(
            snap.status,
            bridge_core::task_store::TaskRecordStatus::Working
        );
        let hub2 = insert_hub(&srv, &task_id).await;
        let resp2 = call_subscribe(&srv, "t9f", Some(1)).await;
        assert_eq!(resp2.status(), StatusCode::OK);
        hub2.publish(live_terminal(2));
        let frames2 = collect_sse_frames(resp2).await;
        // cursor=1 filters the seq-1 checkpoint; snapshot_complete seq = cut_seq (1),
        // then the live terminal.
        assert_eq!(
            frames2,
            vec![
                (1, "snapshot_complete".to_string()),
                (2, "terminal".to_string()),
            ],
            "re-subscribe after lag must re-snapshot coherently: {frames2:?}"
        );
    }
}
