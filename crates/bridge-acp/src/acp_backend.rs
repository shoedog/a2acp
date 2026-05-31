// acp_backend.rs — AcpBackend: a conformant ACP *client* over the
// `agent-client-protocol` SDK (=0.12.1). It drives `initialize`, lazy
// `session/new`, streaming `session/prompt` (fan-in of `session/update`
// notifications), and `session/cancel`.
//
// Spec §5.3 cancellation rule: completion is the prompt RESULT (stopReason
// "cancelled"), NOT the act of sending session/cancel. See Codex finding 2.
// Full cancel *completion* semantics live in Task 4; Task 3's `cancel` only
// latches + sends the notification.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use agent_client_protocol::schema::{
    AgentCapabilities, AuthMethod, AuthMethodId, AuthenticateRequest, CancelNotification,
    ContentBlock, CreateTerminalRequest, CreateTerminalResponse, InitializeRequest,
    InitializeResponse, KillTerminalRequest, KillTerminalResponse, NewSessionRequest,
    PermissionOptionKind, PromptRequest, ProtocolVersion, ReadTextFileRequest,
    ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId as AgentSessionId, SessionNotification, SessionUpdate,
    SetSessionModeRequest, SetSessionModelRequest, StopReason, TerminalOutputRequest,
    TerminalOutputResponse, TextContent, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo, ConnectionTo};
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot, Mutex, OnceCell, OwnedMutexGuard};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use bridge_core::domain::{PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{
    AgentBackend, BackendStream, PolicyEngine, Update, STOP_REASON_CANCELLED,
};

use crate::supervisor::Supervised;

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

/// Static configuration for an ACP agent connection.
///
/// `mode` drives a HARD `session/set_mode` after each `session/new` (a rejected
/// mode fails session setup); `model` drives a BEST-EFFORT `session/set_model`
/// (a failure is logged and the session continues — see [`AcpBackend::ensure_session`]).
/// `auth_method` optionally pins which advertised auth method `connect` uses.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Absolute working directory the agent runs sessions in.
    pub cwd: PathBuf,
    /// Optional model id to request via `session/set_model` (best-effort).
    pub model: Option<String>,
    /// Optional mode id to request via `session/set_mode` (hard error if rejected).
    pub mode: Option<String>,
    /// Optional auth-method id to use for `authenticate`. When `None`, `connect`
    /// uses the FIRST method the agent advertised at `initialize` (if any).
    pub auth_method: Option<String>,
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
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            model: None,
            mode: None,
            auth_method: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            cancel_grace: DEFAULT_CANCEL_GRACE,
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
type UpdateRegistry = Arc<StdMutex<HashMap<AgentSessionId, UpdateSender>>>;

/// What the notification handler forwards to a turn's driver/stream. Kept
/// minimal: only the variants the bridge models today. Unmodeled
/// `SessionUpdate` variants are dropped by the handler (tolerant reader).
enum TurnEvent {
    /// A streamed chunk of the agent's textual response.
    Text(String),
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
    /// Per-session turn lock. Held for the duration of a prompt turn so turns on
    /// one agent session run strictly sequentially. `Arc<Mutex<()>>` (not a bare
    /// field) so `prompt` can take an OWNED guard (`lock_owned`) and move it into
    /// the driver task that holds it for the whole streamed turn.
    turn_lock: Arc<Mutex<()>>,
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
            turn_lock: Arc::new(Mutex::new(())),
            cancel_requested: AtomicBool::new(false),
            turn_kill: Arc::new(StdMutex::new(None)),
        }
    }
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
    /// Static config (cwd for `session/new`, model/mode for later tasks).
    config: Option<AcpConfig>,
    /// bridge-session-key → per-session agent state. The map itself is behind a
    /// `Mutex` held ONLY long enough to look up / insert the `Arc<AgentSession>`;
    /// it is dropped before any `session/new` await so the mint of one session
    /// never blocks lookups of another.
    sessions: Arc<Mutex<HashMap<SessionId, Arc<AgentSession>>>>,
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
}

/// Shared, swappable handle to the active [`PolicyEngine`]. See [`AcpBackend::policy`].
type PolicyHandle = Arc<StdMutex<Arc<dyn PolicyEngine>>>;

impl AcpBackend {
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
    /// `mcpServers` is sent as an explicit empty array, never omitted.
    ///
    /// Exposed so the wire-golden test can assert the serialized `params` shape
    /// (`{"cwd":<abs>,"mcpServers":[]}`) against the SAME value `ensure_session`
    /// transmits — not a re-derivation of the SDK type.
    #[must_use]
    pub fn new_session_request(cwd: impl Into<PathBuf>) -> NewSessionRequest {
        NewSessionRequest::new(cwd).mcp_servers(vec![])
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

    /// Build the `session/set_model` request the backend sends (BEST-EFFORT) after
    /// `session/new` when [`AcpConfig::model`] is set. ACP §11A:
    /// `params:{ "sessionId":<agent id>, "modelId":<model> }`, method
    /// `session/set_model` (snake_case, behind the `unstable_session_model` feature).
    #[must_use]
    pub fn set_model_request(
        agent_id: AgentSessionId,
        model_id: impl Into<String>,
    ) -> SetSessionModelRequest {
        SetSessionModelRequest::new(agent_id, model_id.into())
    }

    /// **Production** constructor: spawn `cmd args` as a `Supervised` child
    /// (its own process group, tested SIGTERM→SIGKILL reaping) and drive the
    /// ACP connection over its stdin/stdout as `ByteStreams`.
    ///
    /// This is `Supervised` + `connect(ByteStreams)`: process lifecycle stays
    /// with `Supervised`; protocol drive is the shared `connect` core.
    pub async fn spawn(cmd: &str, args: &[&str], config: AcpConfig) -> Result<Self, BridgeError> {
        let mut supervised = Supervised::spawn(cmd, args).map_err(|_| BridgeError::AgentCrashed)?;
        let child = supervised.child_mut();
        let stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed)?;
        let stdout = child.stdout.take().ok_or(BridgeError::AgentCrashed)?;
        // The crate uses `futures` async-io; our child uses tokio pipes — adapt
        // with tokio_util::compat. ByteStreams::new(outgoing_writer, incoming_reader).
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        // `supervised` (the process-group owner) MUST live for the whole backend
        // lifetime: `kill_on_drop(true)` would SIGKILL the child the instant it
        // dropped, killing the event-loop task's pipes. Hold it on the backend.
        let backend = Self::connect(transport, config).await?;
        *backend.supervised.lock().expect("supervised lock") = Some(supervised);
        Ok(backend)
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
                            // Map the inbound `session/update` to a modeled `Update`
                            // via the SAME pure helper the corpus replay tests drive,
                            // so a captured real-agent frame exercises this exact path.
                            let session_id = notif.session_id.clone();
                            if let Some(Update::Text(text)) = Self::map_session_update(notif) {
                                // Plain get + non-blocking send under a
                                // std::Mutex: no await is held across the lock.
                                if let Ok(map) = updates.lock() {
                                    if let Some(tx) = map.get(&session_id) {
                                        let _ = tx.send(TurnEvent::Text(text));
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
                        async move {
                            // Offload so the dispatch loop is NOT blocked. The
                            // spawned task owns the `Responder` and answers from there.
                            cx.spawn(async move {
                                let outcome = Self::decide_permission(&policy, &req);
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
            // version, 0.12.1) an UNREGISTERED inbound request is NOT auto-replied by
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

        // Bound the WHOLE handshake — transport connect + `initialize` response +
        // `authenticate` — under the SAME `handshake_timeout`. `authenticate` MUST
        // be inside this bounded block: an agent that answers `initialize` but then
        // HANGS on `authenticate` would otherwise block `connect`/`spawn`/`main`
        // forever. A closed transport EOFs cleanly (the `map_err` arms); a true
        // hang on either step is caught by the outer timeout.
        let auth_method_cfg = config.auth_method.clone();
        let handshake = async move {
            let cx = cx_rx.await.map_err(|_| BridgeError::AgentCrashed)?;

            // Run the ACP `initialize` handshake and capture the negotiated caps.
            let resp: InitializeResponse = cx
                .send_request(Self::initialize_request())
                .block_task()
                .await
                .inspect_err(|e| tracing::warn!(error = ?e, "initialize handshake failed"))
                .map_err(|_| BridgeError::AgentCrashed)?;

            // ── authenticate ──────────────────────────────────────────────────
            //
            // Lifecycle order: initialize → authenticate → (later) session/new. The
            // `initialize` response lists the auth methods the agent supports. If it
            // advertised NONE (and none is configured), the agent needs no
            // client-driven auth, so we SKIP. Otherwise attempt `authenticate` ONCE
            // with either the configured method id or the first advertised one.
            //
            // POLICY for an already-authenticated agent (the T9 gated e2e validates
            // against real codex): codex with an existing login may still advertise
            // methods but not actually require a fresh auth — and may either accept a
            // redundant `authenticate` (success → fine) or REJECT it. We cannot
            // distinguish "wrong credentials" from "redundant auth" from the wire, so
            // we choose the pragmatic, least-surprising policy: attempt ONCE; treat a
            // SUCCESS (or no advertised methods) as authenticated; a definitive Err
            // is FATAL and surfaces `AgentNotAuthenticated`. This matches the
            // spec-intended flow (authenticate before sessions) while keeping the
            // bound tight. (If a future real agent is found to reject redundant auth,
            // revisit toward a softer policy.)
            let chosen = match auth_method_cfg.as_deref() {
                Some(m) => {
                    // I4: a configured auth_method that the agent did NOT advertise
                    // is a likely operator misconfiguration. We still ATTEMPT it
                    // (the agent is authoritative — it may accept an unlisted id),
                    // but WARN naming the configured value and the advertised list so
                    // an opaque `AgentNotAuthenticated` can be diagnosed.
                    let advertised: Vec<String> = resp
                        .auth_methods
                        .iter()
                        .map(|a| a.id().0.to_string())
                        .collect();
                    if !advertised.iter().any(|a| a == m) {
                        tracing::warn!(
                            configured_auth_method = %m,
                            advertised = ?advertised,
                            "configured auth_method is not among the methods the agent \
                             advertised; attempting anyway (agent is authoritative)"
                        );
                    }
                    Some(AuthMethodId::new(m))
                }
                None => resp.auth_methods.first().map(|a| a.id().clone()),
            };
            if let Some(method_id) = chosen {
                cx.send_request(AuthenticateRequest::new(method_id))
                    .block_task()
                    .await
                    .map_err(|_| BridgeError::AgentNotAuthenticated)?;
            }

            Ok::<_, BridgeError>((cx, resp))
        };

        let (cx, resp) = tokio::time::timeout(config.handshake_timeout, handshake)
            .await
            .map_err(|_| BridgeError::AgentCrashed)??;

        Ok(Self {
            conn: Some(AcpConn {
                cx,
                agent_capabilities: resp.agent_capabilities,
                auth_methods: resp.auth_methods,
                _shutdown: shutdown_tx,
                updates,
            }),
            supervised: Arc::new(StdMutex::new(None)),
            config: Some(config),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            policy,
        })
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

        // Pick the first option whose kind matches any in `kinds`, in priority order.
        let select = |kinds: &[PermissionOptionKind]| -> Option<RequestPermissionOutcome> {
            for k in kinds {
                if let Some(opt) = req.options.iter().find(|o| o.kind == *k) {
                    return Some(RequestPermissionOutcome::Selected(
                        SelectedPermissionOutcome::new(opt.option_id.clone()),
                    ));
                }
            }
            None
        };

        match decision {
            // Approve: prefer the least-committal grant.
            Some(Ok(PermissionDecision::Approve)) => select(&[
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            ])
            // No allow option offered (e.g. agent only has reject options) →
            // Cancelled. We must NOT fall back to an arbitrary first option: if
            // the agent offered ONLY reject options, selecting one would
            // permanently blacklist a tool call under an *approve* policy —
            // a correctness inversion. Cancelled doesn't grant but also doesn't
            // permanently deny; it is strictly safer than selecting a reject.
            .unwrap_or(RequestPermissionOutcome::Cancelled),
            // Deny: pick a reject option; if none exists, Cancelled.
            Some(Err(BridgeError::PermissionDenied)) => select(&[
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            ])
            .unwrap_or(RequestPermissionOutcome::Cancelled),
            // Abstain / any other policy error / poisoned lock → Cancelled.
            _ => RequestPermissionOutcome::Cancelled,
        }
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
        self.conn
            .as_ref()
            .map(|c| &c.cx)
            .ok_or(BridgeError::AgentCrashed)
    }

    /// The per-turn chunk routing registry shared with the notification handler.
    fn updates(&self) -> Result<&UpdateRegistry, BridgeError> {
        self.conn
            .as_ref()
            .map(|c| &c.updates)
            .ok_or(BridgeError::AgentCrashed)
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
    /// share one `OnceCell` init future, so the agent sees `session/new` ONCE.
    ///
    /// Cancel-latch [Cx-M2]: the minting task — and only it — drains the latch
    /// AFTER `OnceCell` has published the id (so a concurrent `request_cancel`
    /// can already observe it); if a `cancel` raced ahead of creation it fires
    /// `session/cancel` for the freshly-minted id so the cancel is not dropped.
    /// The latch is *claimed* with an atomic swap so exactly one of the minting
    /// task and a concurrent `request_cancel` sends the notification (no double).
    ///
    /// Lost-cancel window closed: the drain runs after `get_or_try_init` returns,
    /// not inside the init closure. If a `request_cancel` ran while the id was
    /// not yet observable (`get() == None`), it stored `true` and did not send;
    /// the post-init drain (which runs once the id IS observable) then sees the
    /// latch and sends. If `request_cancel` ran after the id became observable,
    /// it and the drain race on the same `swap` and exactly one sends.
    ///
    /// `prompt` calls this, then acquires `turn_lock` and sends `session/prompt`.
    async fn ensure_session(&self, key: &SessionId) -> Result<AgentSessionId, BridgeError> {
        let entry = self.session_entry(key).await;
        let cx = self.cx()?;
        let cwd = self
            .config
            .as_ref()
            .map(|c| c.cwd.clone())
            .ok_or(BridgeError::AgentCrashed)?;
        // Capture the configured mode/model up front (cloned `String`s) so the
        // init closure can own them — session configuration now lives INSIDE the
        // closure (see below), so its captures must be `'static`/move-safe.
        let mode = self.config.as_ref().and_then(|c| c.mode.clone());
        let model = self.config.as_ref().and_then(|c| c.model.clone());

        // Did THIS call mint the agent session? The init closure runs for at most
        // one caller (`OnceCell`); set the flag inside it so only the minter does
        // the post-init latch drain below.
        let mut newly_minted = false;
        let id = entry
            .agent_id
            .get_or_try_init(|| async {
                newly_minted = true;
                // (1) session/new — mint the agent session id.
                let req = Self::new_session_request(cwd);
                let resp = cx
                    .send_request(req)
                    .block_task()
                    .await
                    .inspect_err(|e| tracing::warn!(error = ?e, "session/new mint failed"))
                    .map_err(|_| BridgeError::AgentCrashed)?;
                let id = resp.session_id;

                // (2) set_mode — HARD error, configured INSIDE the closure (before
                // returning the id). The operator asked for a specific mode; if the
                // agent REJECTS the mode id we FAIL session setup. Because this
                // `?`-returns from the init closure, `get_or_try_init` FAILS and the
                // `OnceCell` stays UNINITIALIZED — so the next `ensure_session`
                // re-runs the full mint+configure rather than seeing a committed-but-
                // unconfigured session and silently proceeding in the WRONG mode.
                // (No prompt is sent on a failed setup, so the minted-then-abandoned
                // agent session does no uncontrolled work.)
                if let Some(mode) = mode.as_deref() {
                    cx.send_request(Self::set_mode_request(id.clone(), mode))
                        .block_task()
                        .await
                        .map_err(|_| BridgeError::AgentCrashed)?;
                }

                // (3) set_model — BEST-EFFORT (NON-FATAL). Rationale: codex-acp's
                // `set_model` is custom-provider-only; the builtin OpenAI provider
                // returns `models:null` / errors on `session/set_model`. A model-set
                // failure must therefore NOT kill the session — we LOG it and
                // continue, running the agent's default model. (A configured model
                // the agent silently ignores is acceptable; a hard failure here
                // would make every session on such an agent unusable.)
                if let Some(model) = model.as_deref() {
                    if let Err(e) = cx
                        .send_request(Self::set_model_request(id.clone(), model))
                        .block_task()
                        .await
                    {
                        tracing::warn!(
                            model = %model,
                            error = ?e,
                            "session/set_model failed; continuing with the agent's default \
                             model (set_model is best-effort: e.g. builtin OpenAI returns \
                             models:null)"
                        );
                    }
                }

                Ok::<_, BridgeError>(id)
            })
            .await?;

        // Post-init cancel-latch drain — runs only on the minting call, and only
        // AFTER `get_or_try_init` returned, i.e. once the id is observable to a
        // concurrent `request_cancel`. CLAIM the latch with an atomic swap so
        // exactly one of {this drain, a concurrent `request_cancel`} sends the
        // notification (the other sees `false` and is a no-op): no double-send,
        // and no lost cancel (see the interleaving argument on the method docs).
        if newly_minted && entry.cancel_requested.swap(false, Ordering::SeqCst) {
            cx.send_notification(CancelNotification::new(id.clone()))
                .map_err(|_| BridgeError::AgentCrashed)?;
        }

        Ok(id.clone())
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
    async fn request_cancel(&self, key: &SessionId) -> Result<(), BridgeError> {
        let entry = self.session_entry(key).await;
        let cx = self.cx()?;
        // Set the latch FIRST so a `session/new` completing concurrently observes
        // it. If the id is ALREADY present, CLAIM the latch (swap→false): if we
        // win the claim we fire now; if the minting task already claimed and
        // fired, we don't double-send.
        entry.cancel_requested.store(true, Ordering::SeqCst);
        if let Some(agent_id) = entry.agent_id.get() {
            if entry.cancel_requested.swap(false, Ordering::SeqCst) {
                cx.send_notification(CancelNotification::new(agent_id.clone()))
                    .map_err(|_| BridgeError::AgentCrashed)?;
            }
        }
        Ok(())
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
        let child = supervised.child_mut();
        let stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed)?;
        let stdout = child.stdout.take().ok_or(BridgeError::AgentCrashed)?;
        let transport = ByteStreams::new(stdin.compat_write(), stdout.compat());
        let backend = Self::connect(transport, config).await?;
        *backend.supervised.lock().expect("supervised lock") = Some(supervised);
        Ok(backend)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    /// The configured cancel grace (see [`AcpConfig::cancel_grace`]). Falls back
    /// to the default if no config is set (the `conn: None` test-only path).
    fn cancel_grace(&self) -> std::time::Duration {
        self.config
            .as_ref()
            .map(|c| c.cancel_grace)
            .unwrap_or(DEFAULT_CANCEL_GRACE)
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
    fn escalate_terminate(supervised: &Arc<StdMutex<Option<Supervised>>>) {
        let taken = supervised.lock().ok().and_then(|mut g| g.take());
        if let Some(child) = taken {
            tokio::spawn(async move {
                child.terminate(TERMINATE_GRACE).await;
            });
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
    /// Only `agent_message_chunk` with text content is modeled today (→
    /// `Update::Text`); every other `SessionUpdate` variant (thought chunks, plans,
    /// tool-call updates, …) and non-text content is a tolerant-reader DROP (`None`).
    #[must_use]
    pub fn map_session_update(notif: SessionNotification) -> Option<Update> {
        if let SessionUpdate::AgentMessageChunk(chunk) = notif.update {
            if let ContentBlock::Text(t) = chunk.content {
                return Some(Update::Text(t.text));
            }
        }
        None
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

#[async_trait]
impl AgentBackend for AcpBackend {
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
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> Result<BackendStream, BridgeError> {
        // (1) Mint/get the agent session id. Done OUTSIDE the turn lock so a
        // first-prompt's `session/new` doesn't hold the lock while awaiting.
        let entry = self.session_entry(session).await;
        let agent_id = self.ensure_session(session).await?;

        // Acquire the turn lock as an OWNED guard so it can move into the driver
        // task and be held for the whole streamed turn (released on drop there).
        let turn_guard: OwnedMutexGuard<()> = Arc::clone(&entry.turn_lock).lock_owned().await;

        // Build the per-turn channel and register its sender BEFORE sending the
        // prompt, so the handler routes every chunk (no drop between send and
        // registration). The driver keeps a clone of the sender to push the
        // terminal Done onto the SAME channel the chunks flow through (so the
        // stream yields chunks then Done, in order).
        let (tx, rx) = mpsc::unbounded_channel::<TurnEvent>();
        let done_sender = tx.clone();
        let registry = Arc::clone(self.updates()?);
        {
            let mut map = registry.lock().map_err(|_| BridgeError::AgentCrashed)?;
            map.insert(agent_id.clone(), tx);
        }

        let cx = self.cx()?.clone();
        let req = Self::prompt_request(agent_id.clone(), &parts);

        // Install a FRESH per-turn kill switch on the session: the external cancel
        // grace-watcher fires it to unblock a hung driver (see `cancel`). The
        // driver `select!`s on it and clears the slot on exit.
        let kill = Arc::new(tokio::sync::Notify::new());
        *entry.turn_kill.lock().expect("turn_kill lock") = Some(Arc::clone(&kill));

        // (3) Driver: holds the turn lock for the whole streamed turn (it OWNS
        // `turn_guard`, releasing the lock only when it finishes) and awaits the
        // `PromptResponse`; the SDK delivers chunks meanwhile via the handler.
        let registry_for_driver = Arc::clone(&registry);
        let agent_id_for_driver = agent_id.clone();
        let supervised_for_driver = Arc::clone(&self.supervised);
        let kill_slot = Arc::clone(&entry.turn_kill);
        let grace = self.cancel_grace();
        tokio::spawn(async move {
            // Hold the turn lock for the entire turn.
            let _turn = turn_guard;

            // Await the prompt result, but bail out on either:
            //   * the CONSUMER dropping the stream (`done_sender.closed()` resolves
            //     when the paired `rx`, moved into the returned `BackendStream`, is
            //     dropped — the A2A caller disconnected mid-turn); we must then
            //     cancel the agent turn rather than leave it running holding the
            //     turn lock; or
            //   * the external cancel grace-watcher firing the kill switch (a hung
            //     agent that ignored `session/cancel` past grace) — we abandon the
            //     await so the lock releases and the caller's stream ends.
            let prompt_fut = cx.send_request(req).block_task();
            tokio::pin!(prompt_fut);
            let outcome: Result<_, ()> = tokio::select! {
                outcome = &mut prompt_fut => outcome.map_err(|_| ()),
                _ = kill.notified() => Err(()),
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
                    tokio::select! {
                        outcome = &mut prompt_fut => outcome.map_err(|_| ()),
                        _ = kill.notified() => Err(()),
                        _ = tokio::time::sleep(grace) => {
                            AcpBackend::escalate_terminate(&supervised_for_driver);
                            Err(())
                        }
                    }
                }
            };

            // Unregister this turn's sender FIRST so no late chunk is routed
            // after the terminal Done is emitted.
            if let Ok(mut map) = registry_for_driver.lock() {
                map.remove(&agent_id_for_driver);
            }
            // Clear the kill switch slot now the turn is ending (next turn installs
            // its own); avoids a stale notify firing across turns.
            if let Ok(mut slot) = kill_slot.lock() {
                *slot = None;
            }
            let event = match outcome {
                // Turn COMPLETED (incl. a real StopReason::Cancelled, which maps
                // to Done{"cancelled"} — NOT an error). Emit the mapped Done.
                Ok(resp) => TurnEvent::Done(Update::Done {
                    stop_reason: AcpBackend::stop_reason_str(resp.stop_reason),
                }),
                // A transport/agent error (agent crash / mid-turn transport
                // failure), OR a kill-switch/grace escalation, FAILED the turn:
                // surface a terminal Err on the stream so downstream reports the
                // inbound A2A caller `Failed` — never a silent Done{"unknown"}
                // that reads as a clean `Completed`.
                Err(()) => {
                    tracing::warn!(
                        session = ?agent_id_for_driver,
                        "session/prompt failed (transport/SDK error or kill-switch): \
                         surfacing AgentCrashed"
                    );
                    TurnEvent::Failed(BridgeError::AgentCrashed)
                }
            };
            // If the consumer already dropped the stream this `send` is a no-op,
            // but the lock-release below is what matters there.
            let _ = done_sender.send(event);
            // `_turn` (the OwnedMutexGuard) drops here, releasing the turn lock.
        });

        // The returned stream drains the per-turn channel, mapping events to
        // `Update`s and terminating after the Done.
        let stream = futures::stream::unfold((rx, false), |(mut rx, done)| async move {
            if done {
                return None;
            }
            match rx.recv().await {
                Some(TurnEvent::Text(t)) => Some((Ok(Update::Text(t)), (rx, false))),
                Some(TurnEvent::Done(u)) => Some((Ok(u), (rx, true))),
                // Terminal failure: yield the Err as the final stream item, then
                // end. Downstream re-yields the Err → producer marks `errored` →
                // terminal frame is `TaskOutcome::Failed` (the correct path).
                Some(TurnEvent::Failed(e)) => Some((Err(e), (rx, true))),
                // Channel closed without a Done/Failed (driver dropped) — terminate.
                None => None,
            }
        });

        Ok(Box::pin(stream))
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
        self.request_cancel(session).await?;

        let entry = self.session_entry(session).await;
        // No in-flight turn → the lock is free → nothing to bound. (A turn that
        // starts AFTER this check is a fresh turn, not the one this cancel
        // targeted, so it is correct not to arm a watcher for it.)
        if entry.turn_lock.try_lock().is_ok() {
            return Ok(());
        }

        let turn_lock = Arc::clone(&entry.turn_lock);
        let supervised = Arc::clone(&self.supervised);
        let kill_slot = Arc::clone(&entry.turn_kill);
        let grace = self.cancel_grace();
        tokio::spawn(async move {
            // Wait up to `grace` for the in-flight turn to release the lock (its
            // driver drops the guard on every exit). If it does, the turn
            // completed (cleanly cancelled or otherwise) — no escalation. If the
            // grace elapses first, the agent ignored `session/cancel`: ESCALATE.
            if tokio::time::timeout(grace, turn_lock.lock()).await.is_err() {
                // Nuke a real agent PROCESS (closes its pipes → every in-flight
                // `send_request` errors). For the in-process transport (no child)
                // this is a no-op, so ALSO fire the per-turn kill switch to unblock
                // the driver deterministically; either path ends the turn with a
                // terminal `Err`, releases the lock, and unhangs the caller.
                AcpBackend::escalate_terminate(&supervised);
                let kill = kill_slot.lock().ok().and_then(|g| g.clone());
                if let Some(k) = kill {
                    // `notify_one` stores a permit if the driver has not yet
                    // registered its `notified()` waiter, so the kill is never lost.
                    k.notify_one();
                }
            }
        });
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::Supervised;
    use bridge_core::error::BridgeError;
    use bridge_core::ports::{AgentBackend, Update};
    use futures::StreamExt;
    use std::time::Duration;

    // ── SDK connection path (transport-generic, in-process fake agent) ──────────

    use agent_client_protocol::schema::{
        AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodId, InitializeRequest,
        InitializeResponse, ProtocolVersion,
    };
    use agent_client_protocol::{Agent, Channel};

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
                    move |_req: agent_client_protocol::schema::AuthenticateRequest,
                          responder: agent_client_protocol::Responder<
                        agent_client_protocol::schema::AuthenticateResponse,
                    >,
                          _cx| async move {
                        responder
                            .respond(agent_client_protocol::schema::AuthenticateResponse::new())?;
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
    async fn connect_errors_when_agent_never_answers() {
        // Agent side is dropped immediately -> initialize never completes -> AgentCrashed.
        let (client_side, agent_side) = Channel::duplex();
        drop(agent_side);
        match AcpBackend::connect(client_side, test_config()).await {
            Err(e) => assert_eq!(e, BridgeError::AgentCrashed),
            Ok(_) => panic!("expected AgentCrashed when the agent never answers initialize"),
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
            Err(e) => assert_eq!(
                e,
                BridgeError::AgentCrashed,
                "hung initialize handshake must surface a clear error"
            ),
            Ok(_) => panic!("expected an error when the agent never answers initialize"),
        }
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
        let mut supervised = Supervised::spawn("/bin/cat", &[]).expect("spawn cat");
        let pid = supervised.pid();
        // Take the pipes exactly as `spawn` does (also exercises the I3 seam).
        let child = supervised.child_mut();
        let _stdin = child.stdin.take().ok_or(BridgeError::AgentCrashed).unwrap();
        let _stdout = child
            .stdout
            .take()
            .ok_or(BridgeError::AgentCrashed)
            .unwrap();

        let backend = AcpBackend {
            conn: None,
            supervised: Arc::new(StdMutex::new(Some(supervised))),
            config: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            policy: Arc::new(StdMutex::new(
                Arc::new(AutoApprovePolicy) as Arc<dyn PolicyEngine>
            )),
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
    use agent_client_protocol::schema::{
        AuthenticateRequest, AuthenticateResponse, ContentChunk, NewSessionResponse,
        PermissionOption, PermissionOptionId, PromptRequest, PromptResponse,
        RequestPermissionRequest, RequestPermissionResponse, SetSessionModeRequest,
        SetSessionModeResponse, SetSessionModelRequest, SetSessionModelResponse, StopReason,
        ToolCallId, ToolCallUpdate, ToolCallUpdateFields,
    };
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    /// A scripted `session/update` the fake agent emits mid-turn, before it
    /// returns the `PromptResponse`. Lets a test drive the streaming fan-in:
    /// text chunks (modeled) and unmodeled variants (thought / tool call) that
    /// the tolerant reader must drop.
    #[derive(Clone)]
    enum ScriptedUpdate {
        /// `session/update` with an `agent_message_chunk` carrying this text.
        Text(&'static str),
        /// `session/update` with an `agent_thought_chunk` (unmodeled → dropped).
        Thought(&'static str),
        /// `session/update` with an empty `plan` (unmodeled → dropped).
        Plan,
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
        /// Scripted `session/update`s the prompt handler emits (in order) BEFORE
        /// it returns the `PromptResponse`. Empty by default.
        prompt_updates: Arc<Mutex<Vec<ScriptedUpdate>>>,
        /// The `StopReason` the prompt handler returns. `EndTurn` by default.
        stop_reason: Arc<Mutex<StopReason>>,
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

        // ── Task 6: set_mode / set_model / authenticate ───────────────────────
        /// Mode ids observed via `session/set_mode` requests (in order).
        set_modes: Arc<Mutex<Vec<String>>>,
        /// Fires every time a `session/set_mode` is recorded.
        set_mode_seen: Arc<Notify>,
        /// When set, the `session/set_mode` handler REJECTS the request with a
        /// JSON-RPC error (modeling an agent that does not know the mode id).
        reject_set_mode: Arc<AtomicBool>,
        /// Model ids observed via `session/set_model` requests (in order).
        set_models: Arc<Mutex<Vec<String>>>,
        /// Fires every time a `session/set_model` is recorded.
        set_model_seen: Arc<Notify>,
        /// When set, the `session/set_model` handler REJECTS the request with a
        /// JSON-RPC error (modeling an agent — e.g. builtin OpenAI — that errors
        /// on set_model). The backend treats this as NON-FATAL.
        reject_set_model: Arc<AtomicBool>,
        /// Auth-method ids observed via `authenticate` requests (in order).
        authenticates: Arc<Mutex<Vec<String>>>,
        /// When set, the `authenticate` handler REJECTS with a JSON-RPC error
        /// (modeling an auth failure). The backend surfaces `AgentNotAuthenticated`.
        reject_authenticate: Arc<AtomicBool>,
        /// Auth methods the fake agent advertises in its `initialize` response.
        auth_methods: Arc<Mutex<Vec<AuthMethod>>>,
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
                prompt_updates: Arc::new(Mutex::new(Vec::new())),
                stop_reason: Arc::new(Mutex::new(StopReason::EndTurn)),
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
                set_models: Arc::new(Mutex::new(Vec::new())),
                set_model_seen: Arc::new(Notify::new()),
                reject_set_model: Arc::new(AtomicBool::new(false)),
                authenticates: Arc::new(Mutex::new(Vec::new())),
                reject_authenticate: Arc::new(AtomicBool::new(false)),
                auth_methods: Arc::new(Mutex::new(Vec::new())),
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

    /// Spawn the recording fake agent on `channel`, wired to `rec`'s shared state.
    fn spawn_recording_agent(channel: Channel, rec: Recorder) {
        tokio::spawn(async move {
            let r_init = rec.clone();
            let r_auth = rec.clone();
            let r_new = rec.clone();
            let r_mode = rec.clone();
            let r_model = rec.clone();
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
                    move |req: SetSessionModelRequest,
                          responder: agent_client_protocol::Responder<SetSessionModelResponse>,
                          _cx| {
                        let r = r_model.clone();
                        async move {
                            r.set_models.lock().await.push(req.model_id.0.to_string());
                            r.set_model_seen.notify_one();
                            if r.reject_set_model.load(Ordering::SeqCst) {
                                responder.respond_with_internal_error("set_model unsupported")?;
                            } else {
                                responder.respond(SetSessionModelResponse::new())?;
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    move |_req: NewSessionRequest,
                          responder: agent_client_protocol::Responder<NewSessionResponse>,
                          _cx| {
                        let r = r_new.clone();
                        async move {
                            r.new_session_calls.fetch_add(1, Ordering::SeqCst);
                            // Signal entry BEFORE awaiting the gate so a driver can
                            // deterministically know the mint is in flight (no sleep).
                            r.new_session_started.notify_one();
                            if r.gate_new_session.load(Ordering::SeqCst) {
                                // Hold the reply until the test opens the gate,
                                // widening the create/cancel + concurrency window.
                                r.new_session_gate.notified().await;
                            }
                            responder.respond(NewSessionResponse::new(AgentSessionId::new(
                                r.minted_id,
                            )))?;
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
                                            ScriptedUpdate::Plan => SessionUpdate::Plan(
                                                agent_client_protocol::schema::Plan::new(vec![]),
                                            ),
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
                                    responder
                                        .respond_with_internal_error("agent failed the turn")?;
                                    return Ok(());
                                }
                                r2.prompt_log.lock().await.push("end");
                                let sr = *r2.stop_reason.lock().await;
                                responder.respond(PromptResponse::new(sr))?;
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

    fn bkey(s: &str) -> SessionId {
        SessionId::parse(s).unwrap()
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

        // Release session/new; the post-init drain (only the minter) flushes the
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
            Some(Err(BridgeError::AgentCrashed)) => {}
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
            Some(Err(BridgeError::AgentCrashed)) => {}
            other => panic!("hung-agent escalation must end the turn with Err, got {other:?}"),
        }
        assert!(
            s.next().await.is_none(),
            "stream terminates after the escalation Err"
        );

        // The turn lock is released (escalation dropped the driver's guard): a
        // subsequent prompt on S can proceed (it would deadlock if still held).
        rec.wait_cancel_before_respond
            .store(false, Ordering::SeqCst);
        rec.hang_after_cancel.store(false, Ordering::SeqCst);
        rec.set_stop_reason(StopReason::EndTurn).await;
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
            "the lock released → the next turn runs to completion"
        );
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
        use agent_client_protocol::schema::{ReadTextFileRequest, ReadTextFileResponse};

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

    // ── Task 6: set_mode / set_model / authenticate ────────────────────────────

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
        // INSIDE the `get_or_try_init` closure (before the id is returned), a hard
        // `?`-error makes `get_or_try_init` FAIL and the `OnceCell` stays
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
            Err(BridgeError::AgentCrashed) => {}
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
            Err(BridgeError::AgentCrashed) => {}
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
    async fn set_model_error_is_non_fatal() {
        // The agent ERRORS `session/set_model` (modeling builtin OpenAI returning
        // models:null). This must be NON-FATAL: the session is still set up, the
        // model failure is logged, and a subsequent prompt still completes.
        let rec = Recorder::new("agent-sess-MODELERR");
        rec.reject_set_model.store(true, Ordering::SeqCst);
        let cfg = AcpConfig {
            model: Some("gpt-x".to_string()),
            ..test_config()
        };
        let be = connect_recording_with(rec.clone(), cfg).await;
        let key = bkey("bridge-MODELERR");

        // Session setup succeeds despite the set_model error.
        be.ensure_session(&key).await.expect(
            "a set_model error must NOT fail session setup (best-effort: continue on the \
             agent's default model)",
        );
        tokio::time::timeout(Duration::from_secs(2), rec.set_model_seen.notified())
            .await
            .expect("set_model must have reached the (erroring) agent");
        assert_eq!(rec.set_models.lock().await.as_slice(), &["gpt-x"]);

        // And a subsequent prompt still works (the connection/session survived).
        rec.set_updates(vec![ScriptedUpdate::Text("ok")]).await;
        let mut s = be.prompt(&key, vec![]).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(Update::Text(t))) if t == "ok"));
        assert!(
            matches!(s.next().await, Some(Ok(Update::Done { stop_reason })) if stop_reason == "end_turn")
        );
    }

    #[tokio::test]
    async fn authenticate_failure_surfaces_agent_not_authenticated() {
        // The agent advertises an auth method, then REJECTS `authenticate`. The
        // backend must surface `AgentNotAuthenticated` from `connect` (hard fail).
        let rec = Recorder::new("agent-sess-AUTH");
        rec.advertise_auth_method("oauth").await;
        rec.reject_authenticate.store(true, Ordering::SeqCst);

        let (client_side, agent_side) = Channel::duplex();
        spawn_recording_agent(agent_side, rec.clone());
        match AcpBackend::connect(client_side, test_config()).await {
            Err(BridgeError::AgentNotAuthenticated) => {}
            Err(e) => panic!("authenticate failure must surface AgentNotAuthenticated, got {e:?}"),
            Ok(_) => panic!("authenticate failure must surface AgentNotAuthenticated, got Ok"),
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
            Err(BridgeError::AgentCrashed) => {}
            Err(e) => panic!("a hung initialize handshake must surface a clear error, got {e:?}"),
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
                    move |_req: agent_client_protocol::schema::AuthenticateRequest,
                          _responder: agent_client_protocol::Responder<
                        agent_client_protocol::schema::AuthenticateResponse,
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
            Err(BridgeError::AgentCrashed) => {}
            Err(e) => panic!("a hung authenticate must surface a clear bounded error, got {e:?}"),
            Ok(_) => panic!("a hung authenticate must surface a clear bounded error, got Ok"),
        }
    }

    #[tokio::test]
    async fn configured_auth_method_not_advertised_still_attempts() {
        // I4: a configured `auth_method` that the agent did NOT advertise is still
        // attempted (the agent is authoritative). Here the agent advertises a
        // DIFFERENT method ("oauth") and REJECTS the (mismatched) configured one;
        // the backend attempts the configured id, warns about the mismatch, and the
        // rejection surfaces cleanly as `AgentNotAuthenticated`.
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
            Err(BridgeError::AgentNotAuthenticated) => {}
            Err(e) => panic!("a mismatched+rejected auth_method must fail cleanly, got {e:?}"),
            Ok(_) => panic!("a mismatched+rejected auth_method must fail cleanly, got Ok"),
        }
        // The backend attempted the CONFIGURED method id (not the advertised one).
        assert_eq!(rec.authenticates.lock().await.as_slice(), &["apikey"]);
    }
}
