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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use agent_client_protocol::schema::{
    AgentCapabilities, AuthMethod, CancelNotification, ContentBlock, InitializeRequest,
    InitializeResponse, NewSessionRequest, PromptRequest, ProtocolVersion,
    SessionId as AgentSessionId, SessionNotification, SessionUpdate, StopReason, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectTo, ConnectionTo};
use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot, Mutex, OnceCell, OwnedMutexGuard};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream, Update};

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
/// `model` / `mode` are introduced now but only consumed by later tasks
/// (`set_model` / `set_mode` after `session/new`); Task 1 only uses `cwd`
/// when building sessions, which arrives in a later task too.
#[derive(Debug, Clone)]
pub struct AcpConfig {
    /// Absolute working directory the agent runs sessions in.
    pub cwd: PathBuf,
    /// Optional model id to request via `session/set_model` (later tasks).
    pub model: Option<String>,
    /// Optional mode id to request via `session/set_mode` (later tasks).
    pub mode: Option<String>,
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
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            cancel_grace: DEFAULT_CANCEL_GRACE,
        }
    }
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
    id_counter: Arc<AtomicU64>,
    /// Static config (cwd for `session/new`, model/mode for later tasks).
    config: Option<AcpConfig>,
    /// bridge-session-key → per-session agent state. The map itself is behind a
    /// `Mutex` held ONLY long enough to look up / insert the `Arc<AgentSession>`;
    /// it is dropped before any `session/new` await so the mint of one session
    /// never blocks lookups of another.
    sessions: Arc<Mutex<HashMap<SessionId, Arc<AgentSession>>>>,
}

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

        // The event loop owns a long-lived task. `main_fn` publishes a clone of
        // `cx` and then parks on `shutdown_rx` so the connection stays open for
        // the lifetime of the backend (returning from `main_fn` would close it).
        tokio::spawn(async move {
            let _ = Client
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
                            if let SessionUpdate::AgentMessageChunk(chunk) = notif.update {
                                if let ContentBlock::Text(t) = chunk.content {
                                    // Plain get + non-blocking send under a
                                    // std::Mutex: no await is held across the lock.
                                    if let Ok(map) = updates.lock() {
                                        if let Some(tx) = map.get(&notif.session_id) {
                                            let _ = tx.send(TurnEvent::Text(t.text));
                                        }
                                    }
                                }
                                // else: ignore non-text chunk content.
                            }
                            // else: ignore unmodeled SessionUpdate variants.
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
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

        // Bound the whole handshake (transport connect + initialize response) so
        // an agent that opens stdio but never replies cannot hang us forever.
        // A closed transport EOFs cleanly (the `map_err` arms below); a true hang
        // is caught by the outer timeout.
        let handshake = async {
            let cx = cx_rx.await.map_err(|_| BridgeError::AgentCrashed)?;

            // Run the ACP `initialize` handshake and capture the negotiated caps.
            let resp: InitializeResponse = cx
                .send_request(Self::initialize_request())
                .block_task()
                .await
                .map_err(|_| BridgeError::AgentCrashed)?;
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
            id_counter: Arc::new(AtomicU64::new(1)),
            config: Some(config),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
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

        // Did THIS call mint the agent session? The init closure runs for at most
        // one caller (`OnceCell`); set the flag inside it so only the minter does
        // the post-init latch drain below.
        let mut newly_minted = false;
        let id = entry
            .agent_id
            .get_or_try_init(|| async {
                // The init closure does ONLY `session/new`: send the request and
                // return the freshly-minted id. The cancel-latch drain is moved
                // OUT of here (see below) so it runs AFTER `OnceCell` makes the id
                // observable — closing the lost-cancel window where a concurrent
                // `request_cancel` saw `get() == None`, didn't send, and the
                // in-closure drain had already swapped the latch to false.
                newly_minted = true;
                let req = Self::new_session_request(cwd);
                let resp = cx
                    .send_request(req)
                    .block_task()
                    .await
                    .map_err(|_| BridgeError::AgentCrashed)?;
                Ok::<_, BridgeError>(resp.session_id)
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

    #[allow(dead_code)] // retained for later tasks that mint request ids
    fn next_id(&self) -> u64 {
        self.id_counter.fetch_add(1, Ordering::Relaxed)
    }

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
            StopReason::Cancelled => "cancelled",
            _ => "unknown",
        }
        .to_string()
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
                Err(()) => TurnEvent::Failed(BridgeError::AgentCrashed),
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
            id_counter: Arc::new(AtomicU64::new(1)),
            config: None,
            sessions: Arc::new(Mutex::new(HashMap::new())),
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
        ContentChunk, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
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
    }

    /// Spawn the recording fake agent on `channel`, wired to `rec`'s shared state.
    fn spawn_recording_agent(channel: Channel, rec: Recorder) {
        tokio::spawn(async move {
            let r_init = rec.clone();
            let r_new = rec.clone();
            let r_prompt = rec.clone();
            let r_cancel = rec.clone();
            let _ = Agent
                .builder()
                .name("recording-agent")
                .on_receive_request(
                    move |_req: InitializeRequest,
                          responder: agent_client_protocol::Responder<InitializeResponse>,
                          _cx| {
                        let _ = &r_init;
                        async move {
                            responder.respond(InitializeResponse::new(ProtocolVersion::V1))?;
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

                            // Emit the scripted `session/update`s BEFORE responding,
                            // so they stream to the client mid-turn (the fan-in the
                            // backend routes). `cx` here is the agent's connection
                            // to the client; sending a `SessionNotification` is the
                            // wire `session/update`.
                            let sid = req.session_id.clone();
                            let updates = r.prompt_updates.lock().await.clone();
                            for u in updates {
                                let update = match u {
                                    ScriptedUpdate::Text(t) => SessionUpdate::AgentMessageChunk(
                                        ContentChunk::new(ContentBlock::Text(TextContent::new(t))),
                                    ),
                                    ScriptedUpdate::Thought(t) => SessionUpdate::AgentThoughtChunk(
                                        ContentChunk::new(ContentBlock::Text(TextContent::new(t))),
                                    ),
                                    ScriptedUpdate::Plan => SessionUpdate::Plan(
                                        agent_client_protocol::schema::Plan::new(vec![]),
                                    ),
                                };
                                cx.send_notification(SessionNotification::new(
                                    sid.clone(),
                                    update,
                                ))?;
                            }

                            // Offload the (possibly-blocking) gate-wait + response to
                            // a spawned task via `cx.spawn`, then RETURN from the
                            // handler immediately. This is REQUIRED: SDK request
                            // handlers run inside the dispatch loop and block all
                            // further message processing while awaiting — so a handler
                            // that parked on a gate would prevent the agent from ever
                            // dispatching an incoming `session/cancel`. Spawning frees
                            // the loop to deliver the cancel that these gates await.
                            let r2 = r.clone();
                            cx.spawn(async move {
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
        for (sr, expected) in [
            (StopReason::MaxTokens, "max_tokens"),
            (StopReason::Cancelled, "cancelled"),
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
}
