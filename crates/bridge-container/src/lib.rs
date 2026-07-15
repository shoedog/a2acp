//! Write-capable containerized ACP agent (Slice B2a + B2b-3c). [`ContainerRwBackend`] composes
//! [`bridge_acp::acp_backend::AcpBackend`] via the [`ContainerSpawn`] seam. Default `PerTurn` mode spawns
//! a fresh `:rw` container per `prompt` turn and reaps it on every terminal path. `Warm` mode
//! (`new_warm`) reuses ONE container + ONE ACP session across the turns of a session, reaping ONLY at
//! `retire()` — used by the `implement` review→tweak loop so edit + fix turns share continuity.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use bridge_acp::acp_backend::AcpConfig;
use bridge_core::domain::{Part, SandboxConfig, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{
    AgentBackend, BackendObservers, BackendStream, DiagnosticObserver, RichEventSink,
};
use bridge_core::reaper::{spawn_detached, ReapController, ReapFailure, ReapFn};
use bridge_core::run_identity::RunHandle;
use bridge_core::sandbox::{a2a_name, check_rw_target, compose_container_rw};
use bridge_core::session_cwd::SessionCwd;
use futures::StreamExt;
use tokio::sync::Mutex;

/// Injection seam so warm-reuse / reaper tests run Docker-free. Production wraps `AcpBackend::spawn`
/// (and applies the system `PolicyEngine` to the inner backend — see `main.rs`'s `AcpContainerSpawn`).
#[async_trait]
pub trait ContainerSpawn: Send + Sync {
    /// Production validates composition-owned host/runtime evidence before a generation is published.
    /// Test seams default to healthy so Docker-free behavior tests do not depend on host tooling.
    fn validate_infrastructure(&self, _sandbox: &SandboxConfig) -> Result<(), BridgeError> {
        Ok(())
    }

    /// Production may replace one pre-prompt opaque launch error with uniquely typed, bounded,
    /// read-only infrastructure evidence. Test seams preserve their original error by default.
    async fn classify_spawn_failure(
        &self,
        _sandbox: SandboxConfig,
        error: BridgeError,
    ) -> BridgeError {
        error
    }

    async fn spawn(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
    ) -> Result<Arc<dyn AgentBackend>, BridgeError>;

    async fn spawn_observed(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
        self.spawn(program, argv, cfg).await
    }
}

async fn record_container_transition(
    observer: &Arc<dyn DiagnosticObserver>,
    phase: bridge_core::diagnostics::DiagnosticPhase,
    status: bridge_core::diagnostics::PhaseStatus,
    code: Option<&'static str>,
) -> Result<(), BridgeError> {
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticRedactor, PersistedPhaseTransition,
        PersistedPhaseTransitionInput,
    };
    let redactor = DiagnosticRedactor::default();
    let transition = PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase,
            status,
            at_ms: diagnostic_timestamp_ms(),
            operation: None,
            code: None,
            auth: None,
        },
        code,
        &redactor,
    )
    .map_err(|_| BridgeError::InvalidStateTransition)?;
    let event =
        DiagnosticEvent::new(transition, None).map_err(|_| BridgeError::InvalidStateTransition)?;
    observer.record(event).await
}

fn build_reap_failure(
    failure: ReapFailure,
) -> Result<bridge_core::diagnostics::FailureDiagnostic, BridgeError> {
    use bridge_core::diagnostics::{
        DiagnosticFailureClass, DiagnosticPhase, DiagnosticRedactor, FailureDiagnostic,
        FailureDiagnosticInput, FailureDisposition,
    };
    FailureDiagnostic::build_static_code(
        FailureDiagnosticInput {
            failed_phase: DiagnosticPhase::Teardown,
            last_completed_phase: None,
            class: DiagnosticFailureClass::ContainerRuntime,
            disposition: FailureDisposition::Fatal,
            code: String::new(),
            summary: "Container removal failed".into(),
            causes: vec![],
            stderr_observed: false,
            stderr_line_count: 0,
            stderr_scope: None,
            stderr_tail: None,
            stderr_redaction: None,
            retry_after_ms: None,
            reset_at_ms: None,
            // Cleanup follows an arbitrary warm turn; fail closed for replay and
            // fallback even when this particular session never crossed a prompt.
            prompt_may_have_been_accepted: true,
        },
        failure.code(),
        &DiagnosticRedactor::default(),
    )
    .map_err(|_| BridgeError::InvalidStateTransition)
}

fn container_reap_failure_error(
    diagnostic: bridge_core::diagnostics::FailureDiagnostic,
) -> BridgeError {
    BridgeError::agent_failure(diagnostic)
}

async fn record_reap_failure(
    observer: &Arc<dyn DiagnosticObserver>,
    failure: ReapFailure,
) -> BridgeError {
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor,
        PersistedPhaseTransition, PersistedPhaseTransitionInput, PhaseStatus,
    };
    let diagnostic = match build_reap_failure(failure) {
        Ok(diagnostic) => diagnostic,
        Err(error) => return error,
    };
    let transition = match PersistedPhaseTransition::build_static_code(
        PersistedPhaseTransitionInput {
            phase: DiagnosticPhase::Teardown,
            status: PhaseStatus::Failed,
            at_ms: diagnostic_timestamp_ms(),
            operation: None,
            code: None,
            auth: None,
        },
        Some(failure.code()),
        &DiagnosticRedactor::default(),
    ) {
        Ok(transition) => transition,
        Err(_) => return BridgeError::InvalidStateTransition,
    };
    let event = match DiagnosticEvent::new(transition, Some(diagnostic.clone())) {
        Ok(event) => event,
        Err(_) => return BridgeError::InvalidStateTransition,
    };
    match observer.record(event).await {
        Ok(()) => container_reap_failure_error(diagnostic),
        Err(error) => error,
    }
}

/// Static config for a `ContainerRw` agent (cheap, no Docker at construction — crash-orphan recovery is
/// process-level now, see `main.rs`).
#[derive(Clone)]
pub struct ContainerRwConfig {
    pub sandbox: SandboxConfig,
    /// The inner ACP CLI (e.g. `claude-agent-acp`) — runs contained.
    pub cmd: String,
    pub args: Vec<String>,
    /// MCP servers for `McpDelivery::CodexNative` — rendered to `-c mcp_servers.*` args and appended
    /// to the inner codex-acp argv at open (with `{cwd}` = the per-turn `:rw` clone). Empty otherwise.
    pub mcp: Vec<bridge_core::mcp::McpServerSpec>,
    pub mcp_delivery: bridge_core::mcp::McpDelivery,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub auth_method: Option<String>,
    pub pre_authenticated: bool,
    pub watchdog: Option<bridge_core::domain::WatchdogConfig>,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
    /// Increment A: the per-process run identity — stamps the `a2a.run`/`a2a.host`/`a2a.lease` labels +
    /// the `run_id` segment of each container name (so a concurrent same-owner run never name-clashes).
    pub run: RunHandle,
    /// Increment A: the agent id (stamps the display-only `a2a.agent` label).
    pub agent: String,
}

/// A live per-turn container handle, kept so `cancel` can reach the inner. Its `reaped` is SHARED with
/// the stream-owned [`ContainerReaper`] so cancel + stream-drop can't double-reap.
#[derive(Clone)]
struct ReapOwner {
    generation: u64,
    reap: ReapController,
    /// A removal request may be selected before the async spawn attempt has
    /// reached the point where it can no longer create the named resource.
    /// The detached cleanup flight waits for this cancellation-safe fence so
    /// its one underlying attempt cannot settle against an absent container
    /// and then be followed by a late `docker run`.
    spawn_settlement: Arc<SpawnSettlement>,
    /// Linearizes the final inner prompt installation against cancel, release,
    /// and retirement for this exact container generation. The prompt holds it
    /// only until `inner.prompt*` returns its stream; teardown starts the
    /// process-owned reaper first, then joins this gate before returning.
    dispatch_gate: Arc<Mutex<()>>,
}

struct SpawnSettlement {
    settled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl SpawnSettlement {
    fn pending() -> Self {
        Self {
            settled: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn settle(&self) {
        if !self.settled.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
        }
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.settled.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

struct SpawnSettlementGuard(Arc<SpawnSettlement>);

impl Drop for SpawnSettlementGuard {
    fn drop(&mut self) {
        self.0.settle();
    }
}

impl ReapOwner {
    /// Transfer cleanup to an observer-free task immediately, but do not
    /// consume the one-shot removal until spawn returns or is canceled.
    fn reap_detached(&self) {
        let settlement = Arc::clone(&self.spawn_settlement);
        let reap = self.reap.clone();
        spawn_detached(async move {
            settlement.wait().await;
            // Await internally so an off-runtime throwaway runtime cannot drop
            // after spawning, but before polling, a nested detached worker.
            // This task owns no operation observer and discards only the report.
            let _ = reap.reap_observed().await;
        });
    }

    /// Join the same cleanup flight. Starting the detached waiter before this
    /// await keeps removal owned if the observed caller is canceled.
    async fn reap_observed(&self) -> Result<(), ReapFailure> {
        self.reap_detached();
        self.spawn_settlement.wait().await;
        self.reap.reap_observed().await
    }
}

struct InflightTurn {
    inner: Arc<dyn AgentBackend>,
    owner: ReapOwner,
}

/// A spawned, configured inner backend + its container identity. Shared shape for per-turn (promoted to
/// [`InflightState::Live`]) and warm (cached in `warm`). `rw_canon` is the canonicalized `:rw` target the
/// session was configured with (re-applied on a warm reuse turn).
#[derive(Clone)]
struct WarmInner {
    inner: Arc<dyn AgentBackend>,
    owner: ReapOwner,
    rw_canon: SessionCwd,
}

/// One entry per session: `Reserving` is held across the (async) spawn so a concurrent second prompt is
/// rejected atomically (no check-then-insert race); `Live` carries the cancel handle.
enum InflightState {
    Reserving(ReapOwner),
    Live(InflightTurn),
}

impl InflightState {
    fn generation(&self) -> u64 {
        match self {
            Self::Reserving(owner) => owner.generation,
            Self::Live(turn) => turn.owner.generation,
        }
    }

    fn owner(&self) -> &ReapOwner {
        match self {
            Self::Reserving(owner) => owner,
            Self::Live(turn) => &turn.owner,
        }
    }
}

struct PreparedInner {
    owner: ReapOwner,
    sandbox: SandboxConfig,
    program: String,
    argv: Vec<String>,
    acp: AcpConfig,
    rw_canon: SessionCwd,
}

type Inflight = Arc<Mutex<HashMap<SessionId, InflightState>>>;
type ReapFactory = Arc<dyn Fn(String, String) -> ReapController + Send + Sync>;

/// Per-turn (cold) vs warm (one container + session reused across turns, reaped only at `retire`).
#[derive(Clone, Copy, PartialEq)]
enum Lifecycle {
    PerTurn,
    Warm,
}

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    spawn: Arc<dyn ContainerSpawn>,
    reap_factory: ReapFactory,
    /// STABLE per-instance owner token (hash of config-path + mount + agent id), set by the caller.
    owner: String,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,
    pending_turn_meta: Mutex<HashMap<SessionId, TurnMeta>>,
    inflight: Inflight,
    turn_seq: AtomicU64,
    /// Set under `inflight` before retirement drains ownership. Prompt
    /// admission checks it under the same lock, closing the drain-then-spawn
    /// window.
    retired: AtomicBool,
    lifecycle: Lifecycle,
    /// Warm mode only: the authoritative cached container/session per `SessionId` (drained at `retire`).
    warm: Mutex<HashMap<SessionId, WarmInner>>,
    /// Latest per-session join handle, installed as soon as a named container
    /// is owned. This survives spawn/config/prompt failure and cache/inflight
    /// removal so observed cleanup can join the exact detached attempt. A later
    /// container generation for the same session replaces the settled entry.
    session_reaps: StdMutex<HashMap<SessionId, ReapOwner>>,
    /// Warm mode only: sessions with an in-flight turn → the turn's monotonic epoch (concurrency reject).
    /// The epoch lets a stale (early-drop) detached clear remove ONLY its own turn's marker, never a later
    /// turn's (review finding: a bare `HashSet` clear could erase the next turn's marker).
    turn_active: Arc<Mutex<HashMap<SessionId, u64>>>,
    /// Warm mode only: monotonic per-turn epoch source for `turn_active`.
    turn_epoch: AtomicU64,
}

impl ContainerRwBackend {
    /// Hook-injectable constructor (the ONE constructor — tests inject `reap_fn`). Crash-orphan recovery
    /// is NOT done here: Increment A moved it to the process-level before-first-use `classify_sweep`
    /// (`main.rs`), which is lease/host-scoped (only DEAD same-host orphans are reaped) so it never touches
    /// a CONCURRENT run's containers. The unique per-run name segment (`a2a.run`) means a surviving orphan
    /// can no longer collide with this run's first mint, so construction is now pure bookkeeping.
    pub async fn new_with_hooks(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
        reap_fn: ReapFn,
    ) -> Result<Self, BridgeError> {
        let reap_factory: ReapFactory = Arc::new(move |runtime, name| {
            ReapController::from_legacy(runtime, name, Arc::clone(&reap_fn))
        });
        Self::new_with_reap_factory(cfg, spawn, owner, reap_factory).await
    }

    async fn new_with_reap_factory(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
        reap_factory: ReapFactory,
    ) -> Result<Self, BridgeError> {
        Ok(Self {
            cfg,
            spawn,
            reap_factory,
            owner,
            session_cfg: Mutex::new(HashMap::new()),
            pending_turn_meta: Mutex::new(HashMap::new()),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            turn_seq: AtomicU64::new(0),
            retired: AtomicBool::new(false),
            lifecycle: Lifecycle::PerTurn,
            warm: Mutex::new(HashMap::new()),
            session_reaps: StdMutex::new(HashMap::new()),
            turn_active: Arc::new(Mutex::new(HashMap::new())),
            turn_epoch: AtomicU64::new(0),
        })
    }

    /// Warm hook-injectable constructor: identical to [`Self::new_with_hooks`] but flips the lifecycle to
    /// `Warm` (reuse one container/session across turns; reap only at `retire`).
    pub async fn new_warm_with_hooks(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
        reap_fn: ReapFn,
    ) -> Result<Self, BridgeError> {
        let mut be = Self::new_with_hooks(cfg, spawn, owner, reap_fn).await?;
        be.lifecycle = Lifecycle::Warm;
        Ok(be)
    }

    /// Warm production constructor (detached reaper, like [`Self::new`]).
    pub async fn new_warm(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
    ) -> Result<Self, BridgeError> {
        let reap_factory: ReapFactory = Arc::new(ReapController::production);
        let mut backend = Self::new_with_reap_factory(cfg, spawn, owner, reap_factory).await?;
        backend.lifecycle = Lifecycle::Warm;
        Ok(backend)
    }

    fn is_warm(&self) -> bool {
        self.lifecycle == Lifecycle::Warm
    }

    /// Production constructor: detached `docker rm -f` reaper (crash-orphan recovery is process-level now —
    /// see `new_with_hooks`).
    pub async fn new(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
    ) -> Result<Self, BridgeError> {
        let reap_factory: ReapFactory = Arc::new(ReapController::production);
        Self::new_with_reap_factory(cfg, spawn, owner, reap_factory).await
    }

    /// Canonicalize BOTH the mount anchor and the rw target (resolving symlinks — the writable-mount
    /// security fix), then apply the pure lexical `check_rw_target`. A not-yet-existing scratch dir is
    /// canonicalized via its nearest existing ancestor + the lexical tail. The anchor is
    /// `cfg.sandbox.mount` (== normalized `allowed_cwd_root`, parse-layer S2).
    fn resolve_rw_target(&self, rw: &SessionCwd) -> Result<SessionCwd, BridgeError> {
        let mount_canon = canonicalize_lenient(self.cfg.sandbox.mount.as_str())?;
        let rw_canon = canonicalize_lenient(rw.as_str())?;
        check_rw_target(&mount_canon, &rw_canon)?;
        Ok(rw_canon)
    }

    /// Prepare the complete named-container generation before the first spawn
    /// await. Callers publish `prepared.owner` as a reservation before passing
    /// this value to [`Self::open_inner`].
    fn prepare_inner(&self, spec: &SessionSpec) -> Result<PreparedInner, BridgeError> {
        let runtime = self.cfg.sandbox.runtime().to_string();
        let cwd = spec.cwd.clone().ok_or(BridgeError::ConfigInvalid {
            reason: "missing session cwd".into(),
        })?;
        let rw_canon = self.resolve_rw_target(&cwd)?;
        let preflight_sandbox = SandboxConfig {
            mount: rw_canon.as_str().to_owned(),
            access: bridge_core::domain::MountAccess::Rw,
            ..self.cfg.sandbox.clone()
        };
        self.spawn.validate_infrastructure(&preflight_sandbox)?;
        let generation = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        // Increment A: the run-id segment defeats same-owner concurrent name clashes; the label set is
        // built PER MINT so `kind` (warm|perturn) is never stale and `repo`/`cwd` reflect this :rw target.
        let name = a2a_name(
            "rw",
            &self.owner,
            &self.cfg.run.instance_id,
            &generation.to_string(),
        );
        let reap = (self.reap_factory)(runtime.clone(), name.clone());
        let owner = ReapOwner {
            generation,
            reap,
            spawn_settlement: Arc::new(SpawnSettlement::pending()),
            dispatch_gate: Arc::new(Mutex::new(())),
        };
        let kind = if self.is_warm() { "warm" } else { "perturn" };
        let repo = rw_canon.as_str();
        let labels = self
            .cfg
            .run
            .labels(
                "rw",
                kind,
                &self.cfg.agent,
                &self.owner,
                Some(repo),
                Some(repo),
            )
            .to_arg_pairs();
        // Native codex MCP (ADR-0028): append `-c mcp_servers.*` args to the inner codex-acp argv,
        // `{cwd}`-substituted with THIS turn's `:rw` clone (identical-path mount → the same path
        // resolves inside the container). claude/non-codex leave `mcp` empty.
        let inner_args: Vec<String> = if matches!(
            self.cfg.mcp_delivery,
            bridge_core::mcp::McpDelivery::CodexNative
        ) && !self.cfg.mcp.is_empty()
        {
            let mut a = self.cfg.args.clone();
            a.extend(bridge_core::mcp::render_codex_mcp_args(
                &self.cfg.mcp,
                rw_canon.as_str(),
            ));
            a
        } else {
            self.cfg.args.clone()
        };
        let (program, argv) = compose_container_rw(
            &self.cfg.sandbox,
            &rw_canon,
            &name,
            &self.cfg.cmd,
            &inner_args,
            &labels,
        );
        let acp = AcpConfig {
            agent_id: self.cfg.agent.clone(),
            cwd: PathBuf::from(rw_canon.as_str()),
            model: self.cfg.model.clone(),
            mode: self.cfg.mode.clone(),
            auth_method: self.cfg.auth_method.clone(),
            pre_authenticated: self.cfg.pre_authenticated,
            watchdog: self.cfg.watchdog.clone(),
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
            diagnostic_redactor: bridge_core::diagnostics::DiagnosticRedactor::new(
                bridge_core::mcp::env_redaction_values(&self.cfg.mcp, rw_canon.as_str()),
            ),
            // :rw has its own reaper (this crate); the inner AcpBackend's :ro reaper stays off.
            container: None,
            // MCP delivery to the inner CONTAINER agent (#1b):
            //  - CodexNative: rides the codex-acp argv `-c mcp_servers.*` (rendered into `inner_args` above)
            //    -> the inner backend's ACP-param list stays EMPTY (ADR-0028).
            //  - Acp (claude): the inner AcpBackend mints `NewSessionRequest.mcpServers` from this list,
            //    `{cwd}`-substituted at mint with this turn's clone -> in-container lsp/prism nav for claude.
            //  - KiroNative: kiro honors neither channel for stdio MCP (settings file) -> not wired here.
            mcp: if matches!(self.cfg.mcp_delivery, bridge_core::mcp::McpDelivery::Acp) {
                self.cfg.mcp.clone()
            } else {
                Vec::new()
            },
        };
        Ok(PreparedInner {
            owner,
            sandbox: preflight_sandbox,
            program,
            argv,
            acp,
            rw_canon,
        })
    }

    async fn reserve_generation(
        &self,
        session: &SessionId,
        owner: &ReapOwner,
    ) -> Result<(), BridgeError> {
        let mut inflight = self.inflight.lock().await;
        if self.retired.load(Ordering::SeqCst) {
            return Err(BridgeError::SessionExpired);
        }
        if inflight.contains_key(session) {
            return Err(BridgeError::ConfigInvalid {
                reason: format!("session {} already has an in-flight turn", session.as_str()),
            });
        }
        let mut reaps = self
            .session_reaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reaps.contains_key(session) {
            return Err(BridgeError::ConfigInvalid {
                reason: format!(
                    "session {} cleanup is still owned by its previous container generation",
                    session.as_str()
                ),
            });
        }
        reaps.insert(session.clone(), owner.clone());
        inflight.insert(session.clone(), InflightState::Reserving(owner.clone()));
        Ok(())
    }

    fn current_reap_owner(&self, session: &SessionId) -> Option<ReapOwner> {
        self.session_reaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session)
            .cloned()
    }

    fn clear_reap_owner(&self, session: &SessionId, generation: u64) {
        let mut reaps = self
            .session_reaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reaps.get(session).map(|owner| owner.generation) == Some(generation) {
            reaps.remove(session);
        }
    }

    /// Spawn + configure one already-reserved generation. On failure cleanup
    /// starts but the retained owner remains available for checked/observed
    /// forget or release to join.
    async fn open_inner(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
        prepared: PreparedInner,
        diagnostic_observer: Option<Arc<dyn DiagnosticObserver>>,
    ) -> Result<WarmInner, BridgeError> {
        let PreparedInner {
            owner,
            sandbox,
            program,
            argv,
            acp,
            rw_canon,
        } = prepared;
        let spawned = {
            // Dropping this scope after success, failure, panic unwind, or
            // caller cancellation proves that no later poll of this spawn
            // future can create the named resource.
            let _spawn_settlement = SpawnSettlementGuard(Arc::clone(&owner.spawn_settlement));
            match diagnostic_observer {
                Some(observer) => {
                    self.spawn
                        .spawn_observed(&program, &argv, acp, observer)
                        .await
                }
                None => self.spawn.spawn(&program, &argv, acp).await,
            }
        };
        let inner = match spawned {
            Ok(i) => i,
            Err(e) => {
                owner.reap_detached();
                return Err(self.spawn.classify_spawn_failure(sandbox, e).await);
            }
        };
        // The inner prefers the stashed SessionSpec.cwd over AcpConfig.cwd → configure with CANONICAL cwd.
        let mut spec_canon = spec.clone();
        spec_canon.cwd = Some(rw_canon.clone());
        if let Err(e) = inner.configure_session(session, &spec_canon).await {
            owner.reap_detached();
            return Err(e);
        }
        Ok(WarmInner {
            inner,
            owner,
            rw_canon,
        })
    }

    /// Warm turn: reuse ONE cached container/session across prompts. Concurrency-reject via `turn_active`.
    /// Cache-miss opens (and reaps on its own failure); reuse re-applies the cached canonical cwd. A
    /// REUSE-turn error (configure/prompt) clears `turn_active`, does NOT reap, and returns `Err` — a
    /// transient error must not nuke the warm container (the loop converts it to `FixIncomplete`). A
    /// cache-MISS prompt error reaps + removes the just-opened entry (no cumulative work to protect). The
    /// stream's `TurnGuard` clears `turn_active` on end/early-drop and NEVER reaps; warm reaping is owned
    /// by `retire_warm`/`release_warm`.
    async fn prompt_warm(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: Option<BackendObservers>,
    ) -> Result<BackendStream, BridgeError> {
        let meta = { self.pending_turn_meta.lock().await.remove(session) };
        let spec = self.session_cfg.lock().await.get(session).cloned().ok_or(
            BridgeError::ConfigInvalid {
                reason: "missing session cwd".into(),
            },
        )?;
        {
            let _admission = self.inflight.lock().await;
            if self.retired.load(Ordering::SeqCst) {
                return Err(BridgeError::SessionExpired);
            }
        }
        // Concurrency reject + mark active with a fresh monotonic epoch — the epoch lets the eventual
        // clear (sync or detached) target ONLY this turn's marker, never a later turn's.
        let epoch = self.turn_epoch.fetch_add(1, Ordering::Relaxed);
        {
            let mut ta = self.turn_active.lock().await;
            if ta.contains_key(session) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("session {} already has an in-flight turn", session.as_str()),
                });
            }
            ta.insert(session.clone(), epoch);
        }
        // Clear THIS turn's marker synchronously on every pre-stream error path (epoch-guarded).
        macro_rules! fail {
            ($e:expr) => {{
                let mut ta = self.turn_active.lock().await;
                if ta.get(session) == Some(&epoch) {
                    ta.remove(session);
                }
                return Err($e);
            }};
        }

        let cache_miss = !self.warm.lock().await.contains_key(session);
        if cache_miss {
            let prepared = match self.prepare_inner(&spec) {
                Ok(prepared) => prepared,
                Err(error) => fail!(error),
            };
            let generation = prepared.owner.generation;
            if let Err(error) = self.reserve_generation(session, &prepared.owner).await {
                fail!(error);
            }
            let wi = match self
                .open_inner(
                    session,
                    &spec,
                    prepared,
                    observers
                        .as_ref()
                        .map(|observers| Arc::clone(&observers.diagnostic)),
                )
                .await
            {
                Ok(wi) => wi,
                Err(e) => {
                    let mut inflight = self.inflight.lock().await;
                    if inflight.get(session).map(InflightState::generation) == Some(generation) {
                        inflight.remove(session);
                    }
                    fail!(e)
                }
            };
            let published = {
                let mut inflight = self.inflight.lock().await;
                let owns_reservation = matches!(
                    inflight.get(session),
                    Some(InflightState::Reserving(owner)) if owner.generation == generation
                );
                if owns_reservation {
                    self.warm.lock().await.insert(session.clone(), wi.clone());
                    inflight.remove(session);
                }
                owns_reservation
            };
            if !published {
                wi.owner.reap_detached();
                let _ = wi.inner.cancel(session).await;
                fail!(BridgeError::SessionExpired);
            }
            // NO re-configure on cache-miss: open_inner already configured with the canonical cwd.
        } else {
            if let Some(observers) = &observers {
                use bridge_core::diagnostics::{DiagnosticPhase, PhaseStatus};
                if let Err(error) = record_container_transition(
                    &observers.diagnostic,
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Started,
                    None,
                )
                .await
                {
                    fail!(error);
                }
                if let Err(error) = record_container_transition(
                    &observers.diagnostic,
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Completed,
                    Some("backend.reused"),
                )
                .await
                {
                    fail!(error);
                }
            }
            // Reuse: re-apply the cached canonical cwd. A concurrent `retire` can drain the entry after the
            // cache-hit check above → treat absence as "retired under me" (Err, NOT a panic on unwrap).
            let reuse = {
                let w = self.warm.lock().await;
                w.get(session)
                    .map(|wi| (wi.inner.clone(), wi.rw_canon.clone()))
            };
            let (inner, rw_canon) = match reuse {
                Some(t) => t,
                None => fail!(BridgeError::agent_crashed(
                    "warm session retired during prompt"
                )),
            };
            let mut spec_canon = spec.clone();
            spec_canon.cwd = Some(rw_canon);
            if let Err(e) = inner.configure_session(session, &spec_canon).await {
                fail!(e) // reuse: no reap
            }
        }
        let got = {
            let w = self.warm.lock().await;
            w.get(session)
                .map(|wi| (wi.inner.clone(), wi.owner.clone()))
        };
        let (inner, owner) = match got {
            Some(t) => t,
            None => fail!(BridgeError::agent_crashed(
                "warm session retired during prompt"
            )),
        };
        if let Some(meta) = meta {
            inner.configure_turn(session, meta).await;
        }
        // This exact generation owns dispatch until the inner backend has
        // installed its prompt and returned the stream. Teardown that starts
        // after this lock acquisition waits here; teardown that wins first
        // clears admission, so the recheck below fails before inner dispatch.
        let dispatch = owner.dispatch_gate.lock().await;
        let still_active = !self.retired.load(Ordering::SeqCst)
            && self.turn_active.lock().await.get(session) == Some(&epoch);
        if !still_active {
            drop(dispatch);
            if cache_miss {
                let mut warm = self.warm.lock().await;
                if warm.get(session).map(|wi| wi.owner.generation) == Some(owner.generation) {
                    warm.remove(session);
                }
                drop(warm);
                owner.reap_detached();
                let _ = inner.cancel(session).await;
            }
            return Err(BridgeError::SessionExpired);
        }
        let prompt_result = match observers {
            Some(observers) => inner.prompt_with_observers(session, parts, observers).await,
            None => inner.prompt(session, parts).await,
        };
        drop(dispatch);
        let inner_stream = match prompt_result {
            Ok(s) => s,
            Err(e) => {
                if cache_miss {
                    // First-turn failure → reap + remove (no cumulative work to protect).
                    let mut warm = self.warm.lock().await;
                    if warm.get(session).map(|wi| wi.owner.generation) == Some(owner.generation) {
                        warm.remove(session);
                    }
                    owner.reap_detached();
                }
                fail!(e) // reuse: keep the warm entry, do NOT reap
            }
        };
        let guard = TurnGuard {
            turn_active: self.turn_active.clone(),
            session: session.clone(),
            epoch,
            armed: true,
        };
        Ok(wrap_with_turn_guard(inner, inner_stream, guard))
    }

    /// Warm cancel: cancel the cached inner's current turn + clear `turn_active`. Does NOT reap (the warm
    /// container survives for the next turn; `retire` owns reaping).
    async fn cancel_warm(&self, session: &SessionId) -> Result<(), BridgeError> {
        let candidate = {
            let inflight = self.inflight.lock().await;
            match inflight.get(session) {
                Some(InflightState::Reserving(owner)) => Some((owner.clone(), true)),
                _ => self
                    .warm
                    .lock()
                    .await
                    .get(session)
                    .map(|warm| (warm.owner.clone(), false)),
            }
        };
        let Some((owner, was_reserving)) = candidate else {
            self.turn_active.lock().await.remove(session);
            return Ok(());
        };
        if was_reserving {
            owner.reap_detached();
        }
        let _dispatch = owner.dispatch_gate.lock().await;
        let mut inner = None;
        {
            let mut inflight = self.inflight.lock().await;
            if inflight.get(session).map(InflightState::generation) == Some(owner.generation) {
                if let Some(InflightState::Live(turn)) = inflight.remove(session) {
                    inner = Some(turn.inner);
                }
            }
            let mut warm = self.warm.lock().await;
            if warm.get(session).map(|warm| warm.owner.generation) == Some(owner.generation) {
                if was_reserving {
                    if let Some(warm) = warm.remove(session) {
                        inner = Some(warm.inner);
                    }
                } else {
                    inner = warm.get(session).map(|warm| Arc::clone(&warm.inner));
                }
            }
        }
        self.turn_active.lock().await.remove(session);
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        Ok(())
    }

    async fn begin_warm_cleanup(
        &self,
        session: &SessionId,
    ) -> (Option<ReapOwner>, Option<Arc<dyn AgentBackend>>) {
        let owner = self.current_reap_owner(session);
        let Some(owner) = owner else {
            self.turn_active.lock().await.remove(session);
            return (None, None);
        };
        // The controller owns cleanup before any async gate/map wait. Canceling
        // this waiter can detach reporting, but cannot suppress container
        // removal.
        owner.reap_detached();
        let _dispatch = owner.dispatch_gate.lock().await;
        let mut inner = None;
        {
            // Same lock order as cache-miss publication and retirement.
            let mut inflight = self.inflight.lock().await;
            if inflight.get(session).map(InflightState::generation) == Some(owner.generation) {
                if let Some(InflightState::Live(turn)) = inflight.remove(session) {
                    inner = Some(turn.inner);
                }
            }
            let mut warm = self.warm.lock().await;
            if warm.get(session).map(|wi| wi.owner.generation) == Some(owner.generation) {
                if let Some(wi) = warm.remove(session) {
                    inner = Some(wi.inner);
                }
            }
        }
        self.turn_active.lock().await.remove(session);
        drop(_dispatch);
        (Some(owner), inner)
    }

    async fn begin_cold_cleanup(
        &self,
        session: &SessionId,
    ) -> (Option<ReapOwner>, Option<Arc<dyn AgentBackend>>) {
        let owner = self.current_reap_owner(session);
        let Some(owner) = owner else {
            return (None, None);
        };
        owner.reap_detached();
        let _dispatch = owner.dispatch_gate.lock().await;
        let inner = {
            let mut inflight = self.inflight.lock().await;
            if inflight.get(session).map(InflightState::generation) == Some(owner.generation) {
                match inflight.remove(session) {
                    Some(InflightState::Live(turn)) => Some(turn.inner),
                    Some(InflightState::Reserving(_)) | None => None,
                }
            } else {
                None
            }
        };
        drop(_dispatch);
        (Some(owner), inner)
    }

    fn finish_reap(
        &self,
        session: &SessionId,
        owner: &Option<ReapOwner>,
        result: &Result<(), ReapFailure>,
    ) {
        if result.is_ok() {
            if let Some(owner) = owner {
                self.clear_reap_owner(session, owner.generation);
            }
        }
    }

    async fn release_warm_checked(&self, session: &SessionId) -> Result<(), ReapFailure> {
        let (owner, inner) = self.begin_warm_cleanup(session).await;
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        let result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(()),
        };
        self.finish_reap(session, &owner, &result);
        result
    }

    async fn release_warm_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        use bridge_core::diagnostics::{DiagnosticPhase, PhaseStatus};

        let (owner, inner) = self.begin_warm_cleanup(session).await;
        let start_result = record_container_transition(
            &observer,
            DiagnosticPhase::Teardown,
            PhaseStatus::Started,
            Some("container.teardown.reap"),
        )
        .await;

        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        let reap_result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(()),
        };
        self.finish_reap(session, &owner, &reap_result);
        start_result?;
        match reap_result {
            Ok(()) => {
                record_container_transition(
                    &observer,
                    DiagnosticPhase::Teardown,
                    PhaseStatus::Completed,
                    Some("container.teardown.reaped"),
                )
                .await
            }
            Err(failure) => Err(record_reap_failure(&observer, failure).await),
        }
    }

    async fn release_cold_checked(&self, session: &SessionId) -> Result<(), ReapFailure> {
        let (owner, inner) = self.begin_cold_cleanup(session).await;
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        let result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(()),
        };
        self.finish_reap(session, &owner, &result);
        result
    }

    async fn release_cold_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        use bridge_core::diagnostics::{DiagnosticPhase, PhaseStatus};

        let (owner, inner) = self.begin_cold_cleanup(session).await;
        let start_result = record_container_transition(
            &observer,
            DiagnosticPhase::Teardown,
            PhaseStatus::Started,
            Some("container.teardown.reap"),
        )
        .await;
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        let reap_result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(()),
        };
        self.finish_reap(session, &owner, &reap_result);
        start_result?;
        match reap_result {
            Ok(()) => {
                record_container_transition(
                    &observer,
                    DiagnosticPhase::Teardown,
                    PhaseStatus::Completed,
                    Some("container.teardown.reaped"),
                )
                .await
            }
            Err(failure) => Err(record_reap_failure(&observer, failure).await),
        }
    }
}

impl ContainerRwBackend {
    async fn prompt_inner(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: Option<BackendObservers>,
    ) -> Result<BackendStream, BridgeError> {
        if self.is_warm() {
            return self.prompt_warm(session, parts, observers).await;
        }

        let meta = { self.pending_turn_meta.lock().await.remove(session) };

        // Strict-reject: a writer MUST name its :rw target (no fallback to the broad root). The early
        // presence check keeps reject-before-reserve; `open_inner` re-resolves the same cwd.
        let spec = self.session_cfg.lock().await.get(session).cloned().ok_or(
            BridgeError::ConfigInvalid {
                reason: "missing session cwd".into(),
            },
        )?;
        if spec.cwd.is_none() {
            return Err(BridgeError::ConfigInvalid {
                reason: "missing session cwd".into(),
            });
        }

        let prepared = self.prepare_inner(&spec)?;
        let generation = prepared.owner.generation;
        self.reserve_generation(session, &prepared.owner).await?;
        let wi = match self
            .open_inner(
                session,
                &spec,
                prepared,
                observers
                    .as_ref()
                    .map(|observers| Arc::clone(&observers.diagnostic)),
            )
            .await
        {
            Ok(wi) => wi,
            Err(e) => {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(session).map(InflightState::generation) == Some(generation) {
                    inflight.remove(session);
                }
                return Err(e);
            }
        };

        // Promote only the exact reservation. Cancel/retire may have taken it
        // while spawn was awaiting; a stale opener must never publish work.
        let promoted = {
            let mut inflight = self.inflight.lock().await;
            let owns_reservation = matches!(
                inflight.get(session),
                Some(InflightState::Reserving(owner)) if owner.generation == generation
            );
            if owns_reservation {
                inflight.insert(
                    session.clone(),
                    InflightState::Live(InflightTurn {
                        inner: wi.inner.clone(),
                        owner: wi.owner.clone(),
                    }),
                );
            }
            owns_reservation
        };
        if !promoted {
            wi.owner.reap_detached();
            let _ = wi.inner.cancel(session).await;
            return Err(BridgeError::SessionExpired);
        }

        if let Some(meta) = meta {
            wi.inner.configure_turn(session, meta).await;
        }
        // Match the warm path's generation gate: prompt installation and
        // teardown have one exact linearization point instead of a check/call
        // window.
        let dispatch = wi.owner.dispatch_gate.lock().await;
        let still_owned = {
            let inflight = self.inflight.lock().await;
            !self.retired.load(Ordering::SeqCst)
                && matches!(
                    inflight.get(session),
                    Some(InflightState::Live(turn)) if turn.owner.generation == generation
                )
        };
        if !still_owned {
            drop(dispatch);
            wi.owner.reap_detached();
            return Err(BridgeError::SessionExpired);
        }
        let prompt_result = match observers {
            Some(observers) => {
                wi.inner
                    .prompt_with_observers(session, parts, observers)
                    .await
            }
            None => wi.inner.prompt(session, parts).await,
        };
        drop(dispatch);
        let inner_stream = match prompt_result {
            Ok(s) => s,
            Err(e) => {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(session).map(InflightState::generation) == Some(generation) {
                    inflight.remove(session);
                }
                wi.owner.reap_detached();
                return Err(e);
            }
        };

        let reaper = ContainerReaper {
            owner: wi.owner,
            inflight: self.inflight.clone(),
            session: session.clone(),
        };
        Ok(wrap_with_reaper(wi.inner, inner_stream, reaper))
    }
}

#[async_trait]
impl AgentBackend for ContainerRwBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(session, parts, None).await
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
            Some(BackendObservers::new(
                Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
                Some(sink),
            )),
        )
        .await
    }

    async fn prompt_with_observers(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
        observers: BackendObservers,
    ) -> Result<BackendStream, BridgeError> {
        self.prompt_inner(session, parts, Some(observers)).await
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        if self.is_warm() {
            return self.cancel_warm(session).await;
        }
        let owner = {
            let inflight = self.inflight.lock().await;
            inflight.get(session).map(|state| state.owner().clone())
        };
        if let Some(owner) = owner {
            owner.reap_detached();
            let _dispatch = owner.dispatch_gate.lock().await;
            let state = {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(session).map(InflightState::generation) == Some(owner.generation) {
                    inflight.remove(session)
                } else {
                    None
                }
            };
            if let Some(InflightState::Live(turn)) = state {
                let _ = turn.inner.cancel(session).await;
            }
        }
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.session_cfg
            .lock()
            .await
            .insert(session.clone(), spec.clone());
        Ok(())
    }

    async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
        self.pending_turn_meta
            .lock()
            .await
            .insert(session.clone(), meta);
    }

    /// Legacy cleanup still joins the process-owned flight; it only discards
    /// the result after ownership has settled.
    async fn forget_session(&self, session: &SessionId) {
        let _ = self.forget_session_checked(session).await;
    }

    async fn forget_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        let result = if self.is_warm() {
            Ok(())
        } else {
            self.release_cold_checked(session).await
        };
        self.session_cfg.lock().await.remove(session);
        self.pending_turn_meta.lock().await.remove(session);
        result.map_err(|failure| match build_reap_failure(failure) {
            Ok(diagnostic) => container_reap_failure_error(diagnostic),
            Err(error) => error,
        })
    }

    async fn forget_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let result = if self.is_warm() {
            Ok(())
        } else {
            self.release_cold_observed(session, observer).await
        };
        self.session_cfg.lock().await.remove(session);
        self.pending_turn_meta.lock().await.remove(session);
        result
    }

    async fn release_session(&self, session: &SessionId) {
        let _ = self.release_session_checked(session).await;
    }

    async fn release_session_checked(&self, session: &SessionId) -> Result<(), BridgeError> {
        let result = if self.is_warm() {
            self.release_warm_checked(session).await
        } else {
            self.release_cold_checked(session).await
        };
        self.session_cfg.lock().await.remove(session);
        self.pending_turn_meta.lock().await.remove(session);
        result.map_err(|failure| match build_reap_failure(failure) {
            Ok(diagnostic) => container_reap_failure_error(diagnostic),
            Err(error) => error,
        })
    }

    async fn release_session_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        let result = if self.is_warm() {
            self.release_warm_observed(session, observer).await
        } else {
            self.release_cold_observed(session, observer).await
        };
        self.session_cfg.lock().await.remove(session);
        self.pending_turn_meta.lock().await.remove(session);
        result
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        // Seal admission and snapshot every retained generation under the same
        // lock used by reservation. Start every process-owned reap before the
        // first dispatch-gate await, then join each generation's gate before
        // removing/canceling its inner backend.
        let owners: Vec<(SessionId, ReapOwner)> = {
            let _inflight = self.inflight.lock().await;
            self.retired.store(true, Ordering::SeqCst);
            self.session_reaps
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(session, owner)| (session.clone(), owner.clone()))
                .collect()
        };
        for (_, owner) in &owners {
            owner.reap_detached();
        }
        for (session, owner) in owners {
            let _dispatch = owner.dispatch_gate.lock().await;
            let mut inner = None;
            {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(&session).map(InflightState::generation) == Some(owner.generation) {
                    if let Some(InflightState::Live(turn)) = inflight.remove(&session) {
                        inner = Some(turn.inner);
                    }
                }
                let mut warm = self.warm.lock().await;
                if warm.get(&session).map(|warm| warm.owner.generation) == Some(owner.generation) {
                    if let Some(warm) = warm.remove(&session) {
                        inner = Some(warm.inner);
                    }
                }
            }
            self.turn_active.lock().await.remove(&session);
            if let Some(inner) = inner {
                let _ = inner.cancel(&session).await;
            }
        }
        Ok(())
    }
}

impl Drop for ContainerRwBackend {
    fn drop(&mut self) {
        let owners: Vec<ReapOwner> = self
            .session_reaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for owner in owners {
            owner.reap_detached();
        }
    }
}

/// Owned by the returned stream: reaps the container + clears the inflight entry on EVERY exit path
/// (Done / error / consumer-drop). Reap is idempotent + detached — `Drop` never blocks a worker.
struct ContainerReaper {
    owner: ReapOwner,
    inflight: Inflight,
    session: SessionId,
}
impl ContainerReaper {
    async fn clear_inflight(&self) {
        let mut inflight = self.inflight.lock().await;
        if inflight.get(&self.session).map(InflightState::generation) == Some(self.owner.generation)
        {
            inflight.remove(&self.session);
        }
    }
}
impl Drop for ContainerReaper {
    fn drop(&mut self) {
        // Detach the inflight clear (Drop can't await) — covers the early-drop path.
        let inflight = self.inflight.clone();
        let session = self.session.clone();
        let generation = self.owner.generation;
        spawn_detached(async move {
            let mut inflight = inflight.lock().await;
            if inflight.get(&session).map(InflightState::generation) == Some(generation) {
                inflight.remove(&session);
            }
        });
        self.owner.reap_detached();
    }
}

/// Wrap the inner turn stream so its state OWNS `inner` (keeps the ACP child alive for the whole turn)
/// and `reaper` (reaps + clears inflight on completion OR early drop). On NORMAL completion the inflight
/// entry is cleared synchronously (awaited) so a sequential next turn isn't spuriously rejected.
fn wrap_with_reaper(
    inner: Arc<dyn AgentBackend>,
    inner_stream: BackendStream,
    reaper: ContainerReaper,
) -> BackendStream {
    Box::pin(async_stream::stream! {
        let _inner = inner;
        let reaper = reaper;
        let mut s = inner_stream;
        while let Some(item) = s.next().await {
            yield item;
        }
        reaper.clear_inflight().await;
        // `reaper` + `_inner` drop here → reap (idempotent) + SIGKILL the docker client.
    })
}

/// Warm-turn guard: clears THIS turn's `turn_active` marker on normal stream end (synchronously, in
/// `wrap_with_turn_guard`) OR on early consumer-drop (detached, here). The clear is EPOCH-GUARDED — it only
/// removes the marker if it still carries this turn's epoch — so a late detached clear can never erase a
/// subsequent turn's marker. NEVER reaps — the only warm reap site is `retire_warm`.
struct TurnGuard {
    turn_active: Arc<Mutex<HashMap<SessionId, u64>>>,
    session: SessionId,
    epoch: u64,
    armed: bool,
}
impl Drop for TurnGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let ta = self.turn_active.clone();
        let s = self.session.clone();
        let epoch = self.epoch;
        spawn_detached(async move {
            let mut m = ta.lock().await;
            if m.get(&s) == Some(&epoch) {
                m.remove(&s);
            }
        });
    }
}

/// Wrap a warm turn stream so its state OWNS `inner` (keeps the ACP child alive) and the [`TurnGuard`].
/// On NORMAL completion the active marker is cleared synchronously (awaited) so a sequential next turn
/// isn't spuriously rejected; `armed` is then cleared so the `Drop` doesn't detach a SECOND clear that
/// could race (and erase) the next turn's marker. The `armed = false` write IS read — by `TurnGuard::drop`
/// — but the `unused_assignments` lint doesn't count destructor reads, hence the allow.
#[allow(unused_assignments)]
fn wrap_with_turn_guard(
    inner: Arc<dyn AgentBackend>,
    inner_stream: BackendStream,
    mut guard: TurnGuard,
) -> BackendStream {
    Box::pin(async_stream::stream! {
        let _inner = inner;
        let mut s = inner_stream;
        while let Some(item) = s.next().await {
            yield item;
        }
        // Epoch-guarded synchronous clear so a sequential next turn isn't spuriously rejected.
        {
            let mut m = guard.turn_active.lock().await;
            if m.get(&guard.session) == Some(&guard.epoch) {
                m.remove(&guard.session);
            }
        }
        guard.armed = false;
    })
}

/// Canonicalize `path`, resolving symlinks. If it doesn't exist yet (a fresh scratch dir), canonicalize
/// the nearest existing ancestor and re-append the missing tail.
fn canonicalize_lenient(path: &str) -> Result<SessionCwd, BridgeError> {
    use std::path::Path;
    let p = Path::new(path);
    let mut existing = p;
    let mut tail: Vec<std::ffi::OsString> = vec![];
    let canon = loop {
        match std::fs::canonicalize(existing) {
            Ok(c) => break c,
            Err(_) => {
                let file = existing
                    .file_name()
                    .ok_or(BridgeError::ConfigInvalid {
                        reason: format!(":rw target has no canonical root: {path}"),
                    })?
                    .to_os_string();
                tail.push(file);
                existing = existing.parent().ok_or(BridgeError::ConfigInvalid {
                    reason: format!(":rw target has no canonical root: {path}"),
                })?;
            }
        }
    };
    let mut out: PathBuf = canon;
    for seg in tail.iter().rev() {
        out.push(seg);
    }
    SessionCwd::parse(&out.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::diagnostics::{
        diagnostic_timestamp_ms, DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor,
        InMemoryDiagnosticObserver, PersistedPhaseTransition, PersistedPhaseTransitionInput,
        PhaseStatus,
    };
    use bridge_core::domain::{EffectiveConfig, EgressPolicy, MountAccess};
    use bridge_core::ids::{ContextId, OperationId};
    use bridge_core::permission::TurnMeta;
    use bridge_core::ports::{BackendObservers, DiagnosticObserver, RichEventSink};
    use bridge_core::reaper::ReapAttemptFn;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    // ---- stubs -------------------------------------------------------------

    /// Stub inner backend: emits one `Done`, records cancel + prompt count + the sessions it served.
    /// `fail_prompt` (atomic, flippable through `&self`) makes the NEXT `prompt` error — used to drive the
    /// warm reuse-error path.
    struct StubInner {
        canceled: AtomicBool,
        prompts: AtomicUsize,
        sessions: Mutex<HashSet<String>>,
        fail_prompt: AtomicBool,
        configured_turns: Mutex<Vec<(SessionId, TurnMeta)>>,
        call_order: Mutex<Vec<&'static str>>,
        prompt_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        turn_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        cancel_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    }
    #[async_trait]
    impl AgentBackend for StubInner {
        async fn prompt(&self, s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            if let Some((entered, release)) = &self.prompt_gate {
                entered.notify_one();
                release.notified().await;
            }
            self.prompts.fetch_add(1, Ordering::SeqCst);
            self.sessions.lock().await.insert(s.as_str().to_string());
            self.call_order.lock().await.push("prompt");
            if self.fail_prompt.load(Ordering::SeqCst) {
                return Err(BridgeError::agent_crashed("prompt boom"));
            }
            Ok(Box::pin(tokio_stream::iter(vec![Ok(
                bridge_core::ports::Update::Done {
                    stop_reason: "end_turn".into(),
                },
            )])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.canceled.store(true, Ordering::SeqCst);
            if let Some((entered, release)) = &self.cancel_gate {
                entered.notify_one();
                release.notified().await;
            }
            Ok(())
        }
        async fn configure_turn(&self, session: &SessionId, meta: TurnMeta) {
            if let Some((entered, release)) = &self.turn_gate {
                entered.notify_one();
                release.notified().await;
            }
            self.configured_turns
                .lock()
                .await
                .push((session.clone(), meta));
            self.call_order.lock().await.push("configure_turn");
        }

        async fn prompt_with_observers(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            if let Some(sink) = observers.rich {
                sink.record(bridge_core::orch::OrchEventKind::ToolCall {
                    tool_call_id: "tool-1".into(),
                    title: "container test".into(),
                    kind: "read".into(),
                    status: "completed".into(),
                    locations: vec![],
                    content: None,
                });
            }
            self.prompt(session, parts).await
        }
    }

    struct CountingSpawn {
        count: AtomicUsize,
        fail: bool,
        fail_prompt: bool,
        observed_count: AtomicUsize,
        last_argv: Mutex<Vec<String>>,
        last_acp_mcp: Mutex<Vec<bridge_core::mcp::McpServerSpec>>,
        last_diagnostic_redactor: Mutex<Option<bridge_core::diagnostics::DiagnosticRedactor>>,
        last_inner: Mutex<Option<Arc<StubInner>>>,
        spawn_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        resource_exists: Option<Arc<AtomicBool>>,
        prompt_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        turn_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        cancel_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    }

    #[derive(Default)]
    struct RejectingPreflightSpawn {
        spawn_count: AtomicUsize,
    }

    #[derive(Default)]
    struct ClassifyingFailureSpawn {
        spawn_count: AtomicUsize,
        classify_count: AtomicUsize,
    }

    #[async_trait]
    impl ContainerSpawn for RejectingPreflightSpawn {
        fn validate_infrastructure(&self, sandbox: &SandboxConfig) -> Result<(), BridgeError> {
            let mut invalid = sandbox.clone();
            invalid.image.clear();
            bridge_core::sandbox::validate_container_infrastructure(&invalid)
        }

        async fn spawn(
            &self,
            _program: &str,
            _argv: &[String],
            _cfg: AcpConfig,
        ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::InvalidStateTransition)
        }
    }

    #[async_trait]
    impl ContainerSpawn for ClassifyingFailureSpawn {
        async fn classify_spawn_failure(
            &self,
            _sandbox: SandboxConfig,
            _error: BridgeError,
        ) -> BridgeError {
            self.classify_count.fetch_add(1, Ordering::SeqCst);
            BridgeError::ConfigInvalid {
                reason: "post-failure classifier invoked".into(),
            }
        }

        async fn spawn(
            &self,
            _program: &str,
            _argv: &[String],
            _cfg: AcpConfig,
        ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::agent_crashed("opaque launch failure"))
        }
    }

    impl CountingSpawn {
        fn new(fail: bool) -> Arc<Self> {
            Self::with_optional_gates(fail, None, None, None, None, None)
        }

        fn with_cancel_gate(
            fail: bool,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        ) -> Arc<Self> {
            Self::with_optional_gates(fail, None, None, None, None, Some((entered, release)))
        }

        fn with_spawn_gate(
            fail: bool,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        ) -> Arc<Self> {
            Self::with_optional_gates(fail, Some((entered, release)), None, None, None, None)
        }

        fn with_spawn_gate_and_resource(
            fail: bool,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
            resource_exists: Arc<AtomicBool>,
        ) -> Arc<Self> {
            Self::with_optional_gates(
                fail,
                Some((entered, release)),
                Some(resource_exists),
                None,
                None,
                None,
            )
        }

        fn with_prompt_gate(
            fail: bool,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        ) -> Arc<Self> {
            Self::with_optional_gates(fail, None, None, Some((entered, release)), None, None)
        }

        fn with_turn_gate(
            fail: bool,
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Notify>,
        ) -> Arc<Self> {
            Self::with_optional_gates(fail, None, None, None, Some((entered, release)), None)
        }

        fn with_optional_gates(
            fail: bool,
            spawn_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
            resource_exists: Option<Arc<AtomicBool>>,
            prompt_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
            turn_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
            cancel_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
        ) -> Arc<Self> {
            Arc::new(Self {
                count: AtomicUsize::new(0),
                fail,
                fail_prompt: false,
                observed_count: AtomicUsize::new(0),
                last_argv: Mutex::new(vec![]),
                last_acp_mcp: Mutex::new(vec![]),
                last_diagnostic_redactor: Mutex::new(None),
                last_inner: Mutex::new(None),
                spawn_gate,
                resource_exists,
                prompt_gate,
                turn_gate,
                cancel_gate,
            })
        }
    }
    #[async_trait]
    impl ContainerSpawn for CountingSpawn {
        async fn spawn(
            &self,
            _program: &str,
            argv: &[String],
            cfg: AcpConfig,
        ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            *self.last_argv.lock().await = argv.to_vec();
            *self.last_acp_mcp.lock().await = cfg.mcp.clone();
            *self.last_diagnostic_redactor.lock().await = Some(cfg.diagnostic_redactor.clone());
            if let Some((entered, release)) = &self.spawn_gate {
                entered.notify_one();
                release.notified().await;
            }
            if self.fail {
                return Err(BridgeError::agent_crashed(
                    "boom docker image network mount credential",
                ));
            }
            if let Some(resource_exists) = &self.resource_exists {
                resource_exists.store(true, Ordering::SeqCst);
            }
            let inner = Arc::new(StubInner {
                canceled: AtomicBool::new(false),
                prompts: AtomicUsize::new(0),
                sessions: Mutex::new(HashSet::new()),
                fail_prompt: AtomicBool::new(self.fail_prompt),
                configured_turns: Mutex::new(Vec::new()),
                call_order: Mutex::new(Vec::new()),
                prompt_gate: self.prompt_gate.clone(),
                turn_gate: self.turn_gate.clone(),
                cancel_gate: self.cancel_gate.clone(),
            });
            *self.last_inner.lock().await = Some(inner.clone());
            Ok(inner)
        }

        async fn spawn_observed(
            &self,
            program: &str,
            argv: &[String],
            cfg: AcpConfig,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.observed_count.fetch_add(1, Ordering::SeqCst);
            observer
                .record(test_transition(
                    DiagnosticPhase::Spawn,
                    PhaseStatus::Started,
                    None,
                ))
                .await?;
            let inner = self.spawn(program, argv, cfg).await?;
            observer
                .record(test_transition(
                    DiagnosticPhase::Spawn,
                    PhaseStatus::Completed,
                    None,
                ))
                .await?;
            Ok(inner)
        }
    }

    fn test_transition(
        phase: DiagnosticPhase,
        status: PhaseStatus,
        code: Option<&'static str>,
    ) -> DiagnosticEvent {
        let redactor = DiagnosticRedactor::default();
        let transition = PersistedPhaseTransition::build_static_code(
            PersistedPhaseTransitionInput {
                phase,
                status,
                at_ms: diagnostic_timestamp_ms(),
                operation: None,
                code: None,
                auth: None,
            },
            code,
            &redactor,
        )
        .unwrap();
        DiagnosticEvent::new(transition, None).unwrap()
    }

    #[derive(Default)]
    struct CountingRichSink(AtomicUsize);

    #[async_trait]
    impl RichEventSink for CountingRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct RejectOnRecord {
        count: AtomicUsize,
        reject_at: usize,
    }

    #[async_trait]
    impl DiagnosticObserver for RejectOnRecord {
        async fn record(&self, _event: DiagnosticEvent) -> Result<(), BridgeError> {
            let current = self.count.fetch_add(1, Ordering::SeqCst) + 1;
            if current == self.reject_at {
                Err(BridgeError::StoreFailure)
            } else {
                Ok(())
            }
        }
    }

    fn counting_reap() -> (ReapFn, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let f: ReapFn = Arc::new(move |_rt, _name| {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        (f, n)
    }

    async fn wait_for_reaps(reaps: &AtomicUsize, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while reaps.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached reap starts within the test bound");
    }
    fn cfg_with_mount(mount: &str) -> ContainerRwConfig {
        ContainerRwConfig {
            sandbox: SandboxConfig {
                runtime: None,
                image: "img".into(),
                mount: mount.into(),
                access: MountAccess::Ro, // composer overrides to Rw
                egress: EgressPolicy::Open,
                volumes: vec![],
            },
            cmd: "claude-agent-acp".into(),
            args: vec![],
            mcp: vec![],
            mcp_delivery: Default::default(),
            model: None,
            mode: None,
            auth_method: None,
            pre_authenticated: false,
            watchdog: None,
            handshake_timeout: Duration::from_secs(30),
            cancel_grace: Duration::from_secs(5),
            run: RunHandle {
                instance_id: "run0".into(),
                host: "h".into(),
                lease: "/l/run0.lock".into(),
                start: "0".into(),
            },
            agent: "impl".into(),
        }
    }

    async fn backend(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        reap: ReapFn,
    ) -> ContainerRwBackend {
        ContainerRwBackend::new_with_hooks(cfg_with_mount(mount), spawn, "inst".into(), reap)
            .await
            .unwrap()
    }
    async fn warm_backend(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        reap: ReapFn,
    ) -> ContainerRwBackend {
        ContainerRwBackend::new_warm_with_hooks(cfg_with_mount(mount), spawn, "inst".into(), reap)
            .await
            .unwrap()
    }

    async fn warm_backend_with_attempt(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        attempt: ReapAttemptFn,
    ) -> ContainerRwBackend {
        let factory: ReapFactory =
            Arc::new(move |runtime, name| ReapController::new(runtime, name, Arc::clone(&attempt)));
        let mut backend = ContainerRwBackend::new_with_reap_factory(
            cfg_with_mount(mount),
            spawn,
            "inst".into(),
            factory,
        )
        .await
        .unwrap();
        backend.lifecycle = Lifecycle::Warm;
        backend
    }

    async fn backend_with_attempt(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        attempt: ReapAttemptFn,
    ) -> ContainerRwBackend {
        let factory: ReapFactory =
            Arc::new(move |runtime, name| ReapController::new(runtime, name, Arc::clone(&attempt)));
        ContainerRwBackend::new_with_reap_factory(
            cfg_with_mount(mount),
            spawn,
            "inst".into(),
            factory,
        )
        .await
        .unwrap()
    }
    fn spec_cwd(p: &str) -> SessionSpec {
        SessionSpec {
            config: EffectiveConfig::default(),
            cwd: Some(SessionCwd::parse(p).unwrap()),
        }
    }
    fn turn_meta(ctx: &str, generation: u64, op: &str) -> TurnMeta {
        TurnMeta {
            context_id: ContextId::parse(ctx).unwrap(),
            generation,
            op: OperationId::parse(op).unwrap(),
        }
    }
    /// `prompt` returns `Result<BackendStream, _>`; BackendStream isn't `Debug`, so we can't
    /// `.unwrap_err()` — match instead.
    async fn prompt_err(be: &ContainerRwBackend, s: &SessionId) -> BridgeError {
        match be.prompt(s, vec![]).await {
            Err(e) => e,
            Ok(_) => panic!("expected prompt error"),
        }
    }

    // ---- tests -------------------------------------------------------------

    #[tokio::test]
    async fn configure_then_forget_clears_stash() {
        let (reap, _) = counting_reap();
        let be = backend("/root", CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd("/root")).await.unwrap();
        be.configure_turn(&s, turn_meta("ctx-forget", 1, "turn-forget"))
            .await;
        assert!(be.session_cfg.lock().await.contains_key(&s));
        assert!(be.pending_turn_meta.lock().await.contains_key(&s));
        be.forget_session(&s).await;
        assert!(!be.session_cfg.lock().await.contains_key(&s));
        assert!(!be.pending_turn_meta.lock().await.contains_key(&s));
    }

    #[tokio::test]
    async fn configure_turn_is_forwarded_to_inner_before_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("s1").unwrap();
        let meta = turn_meta("ctx-forward", 7, "turn-forward");

        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        be.configure_turn(&s, meta.clone()).await;
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        while stream.next().await.is_some() {}

        let inner = spawn.last_inner.lock().await.clone().unwrap();
        let turns = inner.configured_turns.lock().await;
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].0, s);
        assert_eq!(turns[0].1.context_id, meta.context_id);
        assert_eq!(turns[0].1.generation, meta.generation);
        assert_eq!(turns[0].1.op, meta.op);
        drop(turns);
        assert_eq!(
            inner.call_order.lock().await.as_slice(),
            ["configure_turn", "prompt"]
        );
    }

    #[tokio::test]
    async fn warm_configure_turn_is_forwarded_to_inner_before_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        let meta = turn_meta("ctx-warm-forward", 9, "turn-warm-forward");

        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        be.configure_turn(&s, meta.clone()).await;
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        while stream.next().await.is_some() {}

        let inner = spawn.last_inner.lock().await.clone().unwrap();
        let turns = inner.configured_turns.lock().await;
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].0, s);
        assert_eq!(turns[0].1.context_id, meta.context_id);
        assert_eq!(turns[0].1.generation, meta.generation);
        assert_eq!(turns[0].1.op, meta.op);
        drop(turns);
        assert_eq!(
            inner.call_order.lock().await.as_slice(),
            ["configure_turn", "prompt"]
        );
    }

    #[tokio::test]
    async fn prompt_without_cwd_strict_rejects() {
        let (reap, _) = counting_reap();
        let be = backend("/root", CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("s1").unwrap();
        let err = prompt_err(&be, &s).await;
        assert!(
            format!("{err:?}").contains("missing session cwd"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn prompt_spawns_once_with_rw_mount_and_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        assert_eq!(
            spawn.count.load(Ordering::SeqCst),
            1,
            "one spawn per prompt"
        );
        let argv = spawn.last_argv.lock().await.clone();
        assert_eq!(&argv[0..3], &["run", "-i", "--rm"]);
        assert_eq!(argv[3], "--name");
        assert!(
            argv[4].starts_with("a2a-rw-inst-"),
            "owner prefix: {}",
            argv[4]
        );
        // backend-level :rw mount / no :ro suffix assertion. The mount is the CANONICALIZED rw target,
        // identical-path (macOS resolves /var -> /private/var, so compare against the canonical form).
        assert!(!argv.iter().any(|a| a.ends_with(":ro")));
        let canon = std::fs::canonicalize(root).unwrap();
        let canon = canon.to_str().unwrap();
        assert!(
            argv.iter().any(|a| a == &format!("{canon}:{canon}")),
            "identical-path canonical mount {canon}:{canon} not in {argv:?}"
        );
        while stream.next().await.is_some() {}
    }

    #[tokio::test]
    async fn codex_native_appends_mcp_c_args_with_clone_cwd() {
        use bridge_core::mcp::{McpDelivery, McpServerSpec};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let mut cfg = cfg_with_mount(root);
        cfg.cmd = "codex-acp".into();
        cfg.mcp_delivery = McpDelivery::CodexNative;
        cfg.mcp = vec![McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![("PRIVATE_TOKEN".into(), "alpha{cwd}omega".into())],
        }];
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let be = ContainerRwBackend::new_with_hooks(cfg, spawn.clone(), "inst".into(), reap)
            .await
            .unwrap();
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        let argv = spawn.last_argv.lock().await.clone();
        let canon = std::fs::canonicalize(root).unwrap();
        let canon = canon.to_str().unwrap();
        assert!(argv.iter().any(|a| a == "-c"), "argv has -c: {argv:?}");
        assert!(
            argv.iter()
                .any(|a| a == r#"mcp_servers.prism.command="/opt/prism""#),
            "command override present: {argv:?}"
        );
        // {cwd} substituted to THIS turn's canonical clone path (identical-path mount).
        assert!(
            argv.iter()
                .any(|a| a == &format!(r#"mcp_servers.prism.args=["--repo", "{canon}"]"#)),
            "args {{cwd}}->{canon}: {argv:?}"
        );
        assert!(!argv.iter().any(|a| a.contains("{cwd}")));
        let expanded = format!("alpha{canon}omega");
        let redactor = spawn
            .last_diagnostic_redactor
            .lock()
            .await
            .clone()
            .expect("spawn receives the effective MCP redactor");
        let sanitized = redactor.sanitize_stderr_line(&format!("adapter echoed {expanded}"), 512);
        assert!(
            !sanitized.contains(&expanded),
            "container delivery must redact the {{cwd}}-expanded credential"
        );
        assert!(sanitized.contains("REDACTED KNOWN SECRET"));
        while stream.next().await.is_some() {}
    }

    #[tokio::test]
    async fn acp_delivery_passes_mcp_to_inner_session_not_codex_args() {
        // #1b: a claude (Acp-delivery) container_rw agent must deliver MCP via the inner AcpConfig.mcp
        // (-> NewSessionRequest.mcpServers at mint), NOT via codex `-c` args. So the inner backend's
        // ACP-param MCP list is populated AND no `-c mcp_servers.*` arg is appended.
        use bridge_core::mcp::{McpDelivery, McpServerSpec};
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let mut cfg = cfg_with_mount(root);
        cfg.cmd = "claude-agent-acp".into();
        cfg.mcp_delivery = McpDelivery::Acp;
        cfg.mcp = vec![McpServerSpec {
            name: "lsp".into(),
            command: "/usr/local/bin/lsp-mcp".into(),
            args: vec![
                "--repo".into(),
                "{cwd}".into(),
                "--lang".into(),
                "auto".into(),
            ],
            env: vec![],
        }];
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let be = ContainerRwBackend::new_with_hooks(cfg, spawn.clone(), "inst".into(), reap)
            .await
            .unwrap();
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        let inner_mcp = spawn.last_acp_mcp.lock().await.clone();
        assert_eq!(inner_mcp.len(), 1, "inner ACP session must get the lsp MCP");
        assert_eq!(inner_mcp[0].name, "lsp");
        assert_eq!(inner_mcp[0].command, "/usr/local/bin/lsp-mcp");
        // NOT delivered via codex `-c` args.
        let argv = spawn.last_argv.lock().await.clone();
        assert!(
            !argv.iter().any(|a| a.starts_with("mcp_servers.")),
            "claude path must not append codex -c mcp args: {argv:?}"
        );
        while stream.next().await.is_some() {}
    }

    #[tokio::test]
    async fn prompt_spawn_failure_reaps_and_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = backend(root, CountingSpawn::new(true), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        be.configure_turn(&s, turn_meta("ctx-spawn-fail", 1, "turn-spawn-fail"))
            .await;
        let err = prompt_err(&be, &s).await;
        let BridgeError::AgentCrashed { reason } = &err else {
            panic!("inner process prose must remain an agent-process error: {err:?}");
        };
        for keyword in ["docker", "image", "network", "mount", "credential"] {
            assert!(reason.contains(keyword));
        }
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1, "spawn failure MUST reap");
        assert!(be.inflight.lock().await.is_empty(), "reservation removed");
        assert!(
            !be.pending_turn_meta.lock().await.contains_key(&s),
            "open_inner failure consumed pending turn metadata"
        );
    }

    #[tokio::test]
    async fn typed_preflight_failure_stops_before_generation_or_spawn() {
        use bridge_core::diagnostics::{DiagnosticFailureClass, FailureDisposition};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = Arc::new(RejectingPreflightSpawn::default());
        let (reap, reaps) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let session = SessionId::parse("typed-preflight").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let error = prompt_err(&be, &session).await;
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("typed preflight should return a structured failure");
        };
        assert_eq!(diagnostic.class(), DiagnosticFailureClass::ContainerImage);
        assert_eq!(
            diagnostic.disposition(),
            FailureDisposition::ContainerFallbackCandidate
        );
        assert!(!diagnostic.prompt_may_have_been_accepted());
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 0);
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        assert!(be.inflight.lock().await.is_empty());
        assert!(be
            .session_reaps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty());
    }

    #[tokio::test]
    async fn launch_failure_is_classified_before_return_and_still_reaped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = Arc::new(ClassifyingFailureSpawn::default());
        let (reap, reaps) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let session = SessionId::parse("post-failure-classify").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let error = prompt_err(&be, &session).await;
        let BridgeError::ConfigInvalid { reason } = error else {
            panic!("post-failure classifier result was not returned");
        };
        assert_eq!(reason, "post-failure classifier invoked");
        assert_eq!(spawn.spawn_count.load(Ordering::SeqCst), 1);
        assert_eq!(spawn.classify_count.load(Ordering::SeqCst), 1);
        wait_for_reaps(&reaps, 1).await;
        assert!(be.inflight.lock().await.is_empty());
    }

    #[tokio::test]
    async fn cold_generation_cannot_replace_unacknowledged_cleanup_owner() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(true);
        let (reap, _) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let session = SessionId::parse("cold-generation-owner").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        assert!(be.prompt(&session, vec![]).await.is_err());
        assert_eq!(spawn.count.load(Ordering::SeqCst), 1);

        let second = prompt_err(&be, &session).await;
        assert!(
            format!("{second:?}").contains("cleanup is still owned"),
            "unexpected second-generation rejection: {second:?}"
        );
        assert_eq!(
            spawn.count.load(Ordering::SeqCst),
            1,
            "a retained cleanup owner must fence the next spawn"
        );

        be.release_session_checked(&session).await.unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let third = prompt_err(&be, &session).await;
        assert!(format!("{third:?}").contains("boom"));
        assert_eq!(spawn.count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn prompt_rejects_second_concurrent_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, _) = counting_reap();
        let be = backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap(); // hold the stream → turn stays in-flight
        let err = prompt_err(&be, &s).await;
        assert!(
            format!("{err:?}").contains("already has an in-flight turn"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn stream_completion_reaps_once_and_clears_inflight() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let mut stream = be.prompt(&s, vec![]).await.unwrap();
        assert!(
            be.inflight.lock().await.contains_key(&s),
            "in-flight during turn"
        );
        while stream.next().await.is_some() {}
        drop(stream);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "exactly one reap on completion"
        );
        assert!(
            !be.inflight.lock().await.contains_key(&s),
            "inflight cleared"
        );
    }

    #[tokio::test]
    async fn early_drop_reaps_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let stream = be.prompt(&s, vec![]).await.unwrap();
        drop(stream); // consumer disconnects before draining
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancel_reaches_inner_and_reaps_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let stream = be.prompt(&s, vec![]).await.unwrap();
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        be.cancel(&s).await.unwrap();
        assert!(
            inner.canceled.load(Ordering::SeqCst),
            "cancel reached the inner"
        );
        // stream-drop after cancel must NOT double-reap (shared `reaped`).
        drop(stream);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "cancel + stream-drop reap exactly once"
        );
    }

    #[tokio::test]
    async fn cold_cancel_during_spawn_reaps_reserved_generation_before_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_spawn_gate(
            false,
            Arc::clone(&spawn_entered),
            Arc::clone(&spawn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("cold-cancel-during-spawn").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        backend.cancel(&session).await.unwrap();
        spawn_release.notify_one();

        let result = prompt.await.unwrap();
        assert!(matches!(result, Err(BridgeError::SessionExpired)));
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(inner.canceled.load(Ordering::SeqCst));
        assert!(!backend.inflight.lock().await.contains_key(&session));
    }

    #[tokio::test]
    async fn cold_retire_during_spawn_reaps_reserved_generation_before_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_spawn_gate(
            false,
            Arc::clone(&spawn_entered),
            Arc::clone(&spawn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("cold-retire-during-spawn").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        backend.retire().await.unwrap();
        spawn_release.notify_one();

        let result = prompt.await.unwrap();
        assert!(matches!(result, Err(BridgeError::SessionExpired)));
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_cancel_during_turn_configuration_prevents_late_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let turn_entered = Arc::new(tokio::sync::Notify::new());
        let turn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_turn_gate(
            false,
            Arc::clone(&turn_entered),
            Arc::clone(&turn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("cold-cancel-during-turn-config").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        backend
            .configure_turn(&session, turn_meta("ctx-cold-cancel", 1, "op-cold-cancel"))
            .await;

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        turn_entered.notified().await;
        backend.cancel(&session).await.unwrap();
        turn_release.notify_one();

        assert!(matches!(
            prompt.await.unwrap(),
            Err(BridgeError::SessionExpired)
        ));
        wait_for_reaps(&reaps, 1).await;
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
    }

    #[derive(Clone, Copy)]
    enum TeardownAction {
        Cancel,
        ReleaseChecked,
        Retire,
    }

    async fn assert_teardown_waits_for_inner_prompt_dispatch(warm: bool, action: TeardownAction) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let prompt_entered = Arc::new(tokio::sync::Notify::new());
        let prompt_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_prompt_gate(
            false,
            Arc::clone(&prompt_entered),
            Arc::clone(&prompt_release),
        );
        let (reap, _) = counting_reap();
        let backend = Arc::new(if warm {
            warm_backend(root, spawn.clone(), reap).await
        } else {
            backend(root, spawn.clone(), reap).await
        });
        let session = SessionId::parse(if warm {
            "warm-dispatch-linearization"
        } else {
            "cold-dispatch-linearization"
        })
        .unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        prompt_entered.notified().await;

        let mut teardown = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move {
                match action {
                    TeardownAction::Cancel => backend.cancel(&session).await,
                    TeardownAction::ReleaseChecked => {
                        backend.release_session_checked(&session).await
                    }
                    TeardownAction::Retire => backend.retire().await,
                }
            })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut teardown)
                .await
                .is_err(),
            "teardown returned while the winning inner prompt had not installed dispatch"
        );

        prompt_release.notify_one();
        let stream = prompt.await.unwrap().unwrap();
        teardown.await.unwrap().unwrap();
        drop(stream);

        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 1);
        assert!(inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_cancel_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(false, TeardownAction::Cancel).await;
    }

    #[tokio::test]
    async fn cold_retire_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(false, TeardownAction::Retire).await;
    }

    #[tokio::test]
    async fn cold_checked_release_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(false, TeardownAction::ReleaseChecked)
            .await;
    }

    #[tokio::test]
    async fn warm_cancel_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(true, TeardownAction::Cancel).await;
    }

    #[tokio::test]
    async fn warm_retire_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(true, TeardownAction::Retire).await;
    }

    #[tokio::test]
    async fn warm_checked_release_waits_for_winning_inner_prompt_dispatch() {
        assert_teardown_waits_for_inner_prompt_dispatch(true, TeardownAction::ReleaseChecked).await;
    }

    async fn assert_reap_waits_until_spawn_can_no_longer_create_resource(
        warm: bool,
        action: TeardownAction,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let resource_exists = Arc::new(AtomicBool::new(false));
        let spawn = CountingSpawn::with_spawn_gate_and_resource(
            false,
            Arc::clone(&spawn_entered),
            Arc::clone(&spawn_release),
            Arc::clone(&resource_exists),
        );
        let reap_entered = Arc::new(tokio::sync::Notify::new());
        let reap_calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let resource_exists = Arc::clone(&resource_exists);
            let reap_entered = Arc::clone(&reap_entered);
            let reap_calls = Arc::clone(&reap_calls);
            Arc::new(move |_runtime, _name| {
                let resource_exists = Arc::clone(&resource_exists);
                let reap_entered = Arc::clone(&reap_entered);
                let reap_calls = Arc::clone(&reap_calls);
                Box::pin(async move {
                    reap_calls.fetch_add(1, Ordering::SeqCst);
                    reap_entered.notify_one();
                    if resource_exists.swap(false, Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(ReapFailure::NonZeroExit)
                    }
                })
            })
        };
        let backend = Arc::new(if warm {
            warm_backend_with_attempt(root, spawn.clone(), attempt).await
        } else {
            backend_with_attempt(root, spawn.clone(), attempt).await
        });
        let session = SessionId::parse(if warm {
            "warm-spawn-settlement"
        } else {
            "cold-spawn-settlement"
        })
        .unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        let owner = backend.current_reap_owner(&session).unwrap();
        match action {
            TeardownAction::Cancel => backend.cancel(&session).await.unwrap(),
            TeardownAction::ReleaseChecked => {
                backend.release_session_checked(&session).await.unwrap()
            }
            TeardownAction::Retire => backend.retire().await.unwrap(),
        }

        assert!(
            tokio::time::timeout(Duration::from_millis(50), reap_entered.notified())
                .await
                .is_err(),
            "the one-shot removal ran before spawn could no longer create the resource"
        );
        spawn_release.notify_one();
        assert!(matches!(
            prompt.await.unwrap(),
            Err(BridgeError::SessionExpired)
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), owner.reap.reap_observed())
                .await
                .expect("post-spawn cleanup must settle"),
            Ok(())
        );
        assert_eq!(reap_calls.load(Ordering::SeqCst), 1);
        assert!(!resource_exists.load(Ordering::SeqCst));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert!(inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_cancel_cannot_consume_reap_before_late_spawn_creates_resource() {
        assert_reap_waits_until_spawn_can_no_longer_create_resource(false, TeardownAction::Cancel)
            .await;
    }

    #[tokio::test]
    async fn warm_retire_cannot_consume_reap_before_late_spawn_creates_resource() {
        assert_reap_waits_until_spawn_can_no_longer_create_resource(true, TeardownAction::Retire)
            .await;
    }

    #[tokio::test]
    async fn aborting_spawn_future_opens_settlement_fence_for_owned_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let resource_exists = Arc::new(AtomicBool::new(false));
        let spawn = CountingSpawn::with_spawn_gate_and_resource(
            false,
            Arc::clone(&spawn_entered),
            spawn_release,
            Arc::clone(&resource_exists),
        );
        let reap_calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let resource_exists = Arc::clone(&resource_exists);
            let reap_calls = Arc::clone(&reap_calls);
            Arc::new(move |_runtime, _name| {
                let resource_exists = Arc::clone(&resource_exists);
                let reap_calls = Arc::clone(&reap_calls);
                Box::pin(async move {
                    reap_calls.fetch_add(1, Ordering::SeqCst);
                    if resource_exists.swap(false, Ordering::SeqCst) {
                        Ok(())
                    } else {
                        Err(ReapFailure::NonZeroExit)
                    }
                })
            })
        };
        let backend = Arc::new(backend_with_attempt(root, spawn, attempt).await);
        let session = SessionId::parse("aborted-spawn-settlement").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        let owner = backend.current_reap_owner(&session).unwrap();
        backend.cancel(&session).await.unwrap();
        prompt.abort();
        match prompt.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("aborted spawn prompt unexpectedly completed"),
        }

        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), owner.reap_observed())
                .await
                .expect("aborting spawn must open the cleanup settlement fence"),
            Err(ReapFailure::NonZeroExit)
        );
        assert_eq!(reap_calls.load(Ordering::SeqCst), 1);
        assert!(!resource_exists.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_prompt_threads_diagnostic_and_rich_observers_through_spawn_and_inner() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let be = backend(root, spawn.clone(), counting_reap().0).await;
        let session = SessionId::parse("observed-cold").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let rich = Arc::new(CountingRichSink::default());
        let mut stream = be
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(observer.clone(), Some(rich.clone())),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        assert_eq!(spawn.observed_count.load(Ordering::SeqCst), 1);
        assert_eq!(rich.0.load(Ordering::SeqCst), 1);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().phase(), DiagnosticPhase::Spawn);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().phase(), DiagnosticPhase::Spawn);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn warm_cache_miss_is_observed_and_reuse_emits_backend_reused_without_respawn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let be = warm_backend(root, spawn.clone(), counting_reap().0).await;
        let session = SessionId::parse("observed-warm").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let first_observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let first_rich = Arc::new(CountingRichSink::default());
        let mut first = be
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(first_observer.clone(), Some(first_rich.clone())),
            )
            .await
            .unwrap();
        while first.next().await.is_some() {}
        assert_eq!(spawn.observed_count.load(Ordering::SeqCst), 1);
        assert_eq!(first_rich.0.load(Ordering::SeqCst), 1);
        assert!(first_observer
            .snapshot()
            .await
            .iter()
            .any(|event| event.transition().phase() == DiagnosticPhase::Spawn));

        let second_observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let second_rich = Arc::new(CountingRichSink::default());
        let mut second = be
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(second_observer.clone(), Some(second_rich.clone())),
            )
            .await
            .unwrap();
        while second.next().await.is_some() {}

        assert_eq!(spawn.observed_count.load(Ordering::SeqCst), 1);
        assert_eq!(second_rich.0.load(Ordering::SeqCst), 1);
        let second_events = second_observer.snapshot().await;
        assert_eq!(second_events.len(), 2);
        assert!(second_events
            .iter()
            .all(|event| event.transition().phase() == DiagnosticPhase::Resolve));
        assert_eq!(
            second_events[1]
                .transition()
                .code()
                .map(|code| code.as_str()),
            Some("backend.reused")
        );
    }

    #[tokio::test]
    async fn retire_cancels_and_reaps() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("s1").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap();
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        be.retire().await.unwrap();
        assert!(
            inner.canceled.load(Ordering::SeqCst),
            "retire cancels the inner"
        );
        wait_for_reaps(&reaps, 1).await;
        assert!(reaps.load(Ordering::SeqCst) >= 1, "retire reaps");
    }

    #[test]
    fn off_runtime_reaper_drop_does_not_panic() {
        // Drop firing OUTSIDE a tokio runtime must not panic (process-shutdown path).
        let (reap, reaps) = counting_reap();
        let inflight: Inflight = Arc::new(Mutex::new(HashMap::new()));
        let spawn_settlement = Arc::new(SpawnSettlement::pending());
        spawn_settlement.settle();
        let reaper = ContainerReaper {
            owner: ReapOwner {
                generation: 0,
                reap: ReapController::from_legacy("docker", "a2a-rw-inst-0", reap),
                spawn_settlement,
                dispatch_gate: Arc::new(Mutex::new(())),
            },
            inflight,
            session: SessionId::parse("s1").unwrap(),
        };
        drop(reaper); // no runtime in scope → spawn_detached uses the thread fallback
        for _ in 0..100 {
            if reaps.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "reap still fires off-runtime"
        );
    }

    #[tokio::test]
    async fn rw_target_guard_rejects_symlink_escape_and_accepts_nonexistent_scratch() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let be = backend(
            root.path().to_str().unwrap(),
            CountingSpawn::new(false),
            counting_reap().0,
        )
        .await;
        // a not-yet-existing scratch dir UNDER the root: nearest-ancestor canonicalization accepts it.
        let scratch = root.path().join("does-not-exist-yet");
        assert!(be
            .resolve_rw_target(&SessionCwd::parse(scratch.to_str().unwrap()).unwrap())
            .is_ok());
        // the symlink resolves OUTSIDE the root → reject.
        let err = be
            .resolve_rw_target(&SessionCwd::parse(link.to_str().unwrap()).unwrap())
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("escapes mount root"),
            "got {err:?}"
        );
    }

    // ---- warm-mode tests (B2b-3c) -----------------------------------------

    #[tokio::test]
    async fn warm_reuses_one_inner_across_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        {
            let mut a = be.prompt(&s, vec![]).await.unwrap();
            while a.next().await.is_some() {}
        }
        {
            let mut b = be.prompt(&s, vec![]).await.unwrap();
            while b.next().await.is_some() {}
        }
        assert_eq!(
            spawn.count.load(Ordering::SeqCst),
            1,
            "ONE container across both turns"
        );
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "NOT reaped between turns");
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(
            inner.prompts.load(Ordering::SeqCst),
            2,
            "both turns hit the SAME inner"
        );
    }

    #[tokio::test]
    async fn warm_reuse_turn_error_clears_turn_active_and_does_not_reap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false); // turn 1 ok
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        {
            let mut a = be.prompt(&s, vec![]).await.unwrap();
            while a.next().await.is_some() {}
        }
        // Make the cached inner fail its NEXT prompt (a transient reuse-turn error).
        spawn
            .last_inner
            .lock()
            .await
            .as_ref()
            .unwrap()
            .fail_prompt
            .store(true, Ordering::SeqCst);
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("prompt boom"), "got {err:?}");
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            0,
            "a transient reuse error must NOT reap the warm container"
        );
        assert!(
            be.warm.lock().await.contains_key(&s),
            "warm entry retained across a reuse error"
        );
        assert!(
            !be.turn_active.lock().await.contains_key(&s),
            "turn_active cleared after the error"
        );
    }

    #[tokio::test]
    async fn warm_rejects_second_concurrent_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, _) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap(); // hold the stream
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("in-flight turn"), "got {err:?}");
    }

    #[tokio::test]
    async fn warm_retire_reaps_cached_container() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, spawn.clone(), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        {
            let mut a = be.prompt(&s, vec![]).await.unwrap();
            while a.next().await.is_some() {}
        }
        {
            let mut b = be.prompt(&s, vec![]).await.unwrap();
            while b.next().await.is_some() {}
        }
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "no reap across turns");
        be.retire().await.unwrap();
        wait_for_reaps(&reaps, 1).await;
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert!(
            inner.canceled.load(Ordering::SeqCst),
            "retire cancels the inner"
        );
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "reaped exactly once at retire"
        );
        assert!(be.warm.lock().await.is_empty(), "warm cache drained");
    }

    #[tokio::test]
    async fn dropping_warm_backend_starts_cached_container_cleanup() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let session = SessionId::parse("warm-drop-cleanup").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = be.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        drop(stream);

        drop(be);
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_retirement_starts_reap_before_cancellable_agent_cancel() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let cancel_entered = Arc::new(tokio::sync::Notify::new());
        let cancel_release = Arc::new(tokio::sync::Notify::new());
        let reap_entered = Arc::new(tokio::sync::Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            let reap_entered = Arc::clone(&reap_entered);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                let reap_entered = Arc::clone(&reap_entered);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    reap_entered.notify_one();
                    Ok(())
                })
            })
        };
        let spawn = CountingSpawn::with_cancel_gate(
            false,
            Arc::clone(&cancel_entered),
            Arc::clone(&cancel_release),
        );
        let backend = Arc::new(warm_backend_with_attempt(root, spawn, attempt).await);
        let session = SessionId::parse("retire-cancel-window").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}

        let retire = {
            let backend = Arc::clone(&backend);
            tokio::spawn(async move { backend.retire().await })
        };
        tokio::time::timeout(Duration::from_secs(2), cancel_entered.notified())
            .await
            .expect("retirement reaches the gated agent cancel");
        tokio::time::timeout(Duration::from_secs(2), reap_entered.notified())
            .await
            .expect("reap starts even while agent cancel remains blocked");
        assert!(!retire.is_finished());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        cancel_release.notify_one();
        retire.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn release_session_reaps_only_that_warm_container() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("ctx-a-g0").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let mut stream = be
            .prompt(&s, vec![Part { text: "hi".into() }])
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        be.release_session(&s).await;
        wait_for_reaps(&reaps, 1).await;
        assert!(be.warm.lock().await.get(&s).is_none(), "warm entry removed");
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "exactly one container reaped"
        );
    }

    #[tokio::test]
    async fn observed_warm_release_awaits_success_and_records_teardown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        let be = warm_backend_with_attempt(root, CountingSpawn::new(false), attempt).await;
        let session = SessionId::parse("observed-release-ok").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = be.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

        be.release_session_observed(&session, observer.clone())
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().phase(), DiagnosticPhase::Teardown);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
        assert_eq!(
            events[1].transition().code().map(|code| code.as_str()),
            Some("container.teardown.reaped")
        );
        assert!(!be.warm.lock().await.contains_key(&session));
    }

    #[tokio::test]
    async fn canceled_warm_checked_release_cannot_suppress_reap_start_before_inflight_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let attempt: ReapAttemptFn = {
            let entered = Arc::clone(&entered);
            Arc::new(move |_runtime, _name| {
                let entered = Arc::clone(&entered);
                Box::pin(async move {
                    entered.notify_one();
                    Ok(())
                })
            })
        };
        let backend =
            Arc::new(warm_backend_with_attempt(root, CountingSpawn::new(false), attempt).await);
        let session = SessionId::parse("warm-release-cancel-safe-start").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        let controller = backend.current_reap_owner(&session).unwrap().reap;

        let inflight_guard = backend.inflight.lock().await;
        let release = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.release_session_checked(&session).await })
        };
        tokio::time::timeout(Duration::from_millis(100), entered.notified())
            .await
            .expect("checked release must start its reaper before waiting on async state");
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());
        assert_eq!(controller.reap_observed().await, Ok(()));
        drop(inflight_guard);
    }

    #[tokio::test]
    async fn canceled_cold_observed_release_cannot_suppress_reap_start_before_inflight_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let attempt: ReapAttemptFn = {
            let entered = Arc::clone(&entered);
            Arc::new(move |_runtime, _name| {
                let entered = Arc::clone(&entered);
                Box::pin(async move {
                    entered.notify_one();
                    Ok(())
                })
            })
        };
        let backend =
            Arc::new(backend_with_attempt(root, CountingSpawn::new(false), attempt).await);
        let session = SessionId::parse("cold-release-cancel-safe-start").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let stream = backend.prompt(&session, vec![]).await.unwrap();
        let controller = backend.current_reap_owner(&session).unwrap().reap;

        let inflight_guard = backend.inflight.lock().await;
        let release = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
            tokio::spawn(async move { backend.release_session_observed(&session, observer).await })
        };
        tokio::time::timeout(Duration::from_millis(100), entered.notified())
            .await
            .expect("observed release must start its reaper before waiting on async state");
        release.abort();
        assert!(release.await.unwrap_err().is_cancelled());
        assert_eq!(controller.reap_observed().await, Ok(()));
        drop(inflight_guard);
        drop(stream);
    }

    #[tokio::test]
    async fn observed_cold_release_joins_reap_after_agent_spawn_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_attempt = Arc::clone(&calls);
        let attempt: ReapAttemptFn = Arc::new(move |_runtime, _name| {
            let calls = Arc::clone(&calls_for_attempt);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ReapFailure::Spawn)
            })
        });
        let be = backend_with_attempt(root, CountingSpawn::new(true), attempt).await;
        let session = SessionId::parse("observed-cold-spawn-failure").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        assert!(be.prompt(&session, vec![]).await.is_err());
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = be
            .release_session_observed(&session, observer.clone())
            .await
            .expect_err("observed cleanup must join the failed detached reap");
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("typed reap failure must be structured");
        };
        assert_eq!(
            diagnostic.class(),
            bridge_core::diagnostics::DiagnosticFailureClass::ContainerRuntime
        );
        assert_eq!(diagnostic.code().as_str(), ReapFailure::Spawn.code());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Failed);
        assert!(be.release_session_checked(&session).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn observed_cold_forget_joins_the_stream_owned_cleanup_flight() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            })
        };
        let backend =
            Arc::new(backend_with_attempt(root, CountingSpawn::new(false), attempt).await);
        let session = SessionId::parse("observed-cold-forget").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        drop(stream);
        entered.notified().await;

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let forget = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            let observer = observer.clone();
            tokio::spawn(async move { backend.forget_session_observed(&session, observer).await })
        };
        tokio::task::yield_now().await;
        assert!(
            !forget.is_finished(),
            "observed forget must join, not detach from, cleanup"
        );
        release.notify_one();
        forget.await.unwrap().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn observed_cold_forget_surfaces_the_stable_cleanup_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let attempt: ReapAttemptFn =
            Arc::new(|_runtime, _name| Box::pin(async move { Err(ReapFailure::Timeout) }));
        let be = backend_with_attempt(root, CountingSpawn::new(false), attempt).await;
        let session = SessionId::parse("observed-cold-forget-failure").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = be.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        drop(stream);

        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let error = be
            .forget_session_observed(&session, observer)
            .await
            .expect_err("cold forget must surface the stream-owned reap failure");
        let BridgeError::AgentFailure { diagnostic } = error else {
            panic!("typed reap failure must be structured");
        };
        assert_eq!(diagnostic.code().as_str(), ReapFailure::Timeout.code());
        assert!(be.forget_session_checked(&session).await.is_err());
    }

    #[tokio::test]
    async fn observed_warm_release_maps_every_typed_reap_failure_without_retry() {
        for failure in [
            ReapFailure::Spawn,
            ReapFailure::Timeout,
            ReapFailure::NonZeroExit,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_str().unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let attempt: ReapAttemptFn = {
                let calls = Arc::clone(&calls);
                Arc::new(move |_runtime, _name| {
                    let calls = Arc::clone(&calls);
                    Box::pin(async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Err(failure)
                    })
                })
            };
            let be = warm_backend_with_attempt(root, CountingSpawn::new(false), attempt).await;
            let session = SessionId::parse(format!("observed-release-{failure:?}")).unwrap();
            be.configure_session(&session, &spec_cwd(root))
                .await
                .unwrap();
            let mut stream = be.prompt(&session, vec![]).await.unwrap();
            while stream.next().await.is_some() {}
            let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());

            let error = be
                .release_session_observed(&session, observer.clone())
                .await
                .unwrap_err();
            let BridgeError::AgentFailure { diagnostic } = &error else {
                panic!("typed reap failure must be structured: {error:?}");
            };
            assert_eq!(
                diagnostic.class(),
                bridge_core::diagnostics::DiagnosticFailureClass::ContainerRuntime
            );
            assert_eq!(diagnostic.code().as_str(), failure.code());
            assert_eq!(
                diagnostic.disposition(),
                bridge_core::diagnostics::FailureDisposition::Fatal
            );
            assert!(diagnostic.prompt_may_have_been_accepted());
            assert!(!error.is_transient());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let events = observer.snapshot().await;
            assert_eq!(events.len(), 2);
            assert_eq!(events[1].transition().status(), PhaseStatus::Failed);

            // The retained controller returns the same settled failure without
            // starting a second removal attempt.
            assert!(be.release_session_checked(&session).await.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn failed_cleanup_event_persistence_remains_the_public_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let attempt: ReapAttemptFn =
            Arc::new(|_runtime, _name| Box::pin(async move { Err(ReapFailure::Timeout) }));
        let be = warm_backend_with_attempt(root, CountingSpawn::new(false), attempt).await;
        let session = SessionId::parse("warm-observer-precedence").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = be.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}

        let rejecting = Arc::new(RejectOnRecord {
            count: AtomicUsize::new(0),
            reject_at: 2,
        });
        assert_eq!(
            be.release_session_observed(&session, rejecting).await,
            Err(BridgeError::StoreFailure),
            "a real journal write failure remains authoritative"
        );
        let stable = be.release_session_checked(&session).await.unwrap_err();
        let BridgeError::AgentFailure { diagnostic } = stable else {
            panic!("the controller must retain the typed cleanup result");
        };
        assert_eq!(diagnostic.code().as_str(), ReapFailure::Timeout.code());
    }

    #[tokio::test]
    async fn retirement_and_observed_release_join_without_retaining_observer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            })
        };
        let be =
            Arc::new(warm_backend_with_attempt(root, CountingSpawn::new(false), attempt).await);
        let session = SessionId::parse("release-retire-join").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let mut stream = be.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}

        be.retire().await.unwrap();
        entered.notified().await;
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let observer_dyn: Arc<dyn DiagnosticObserver> = observer.clone();
        let weak = Arc::downgrade(&observer_dyn);
        let release_task = {
            let be = Arc::clone(&be);
            let session = session.clone();
            tokio::spawn(async move { be.release_session_observed(&session, observer_dyn).await })
        };
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        release_task.await.unwrap().unwrap();
        drop(observer);
        assert!(
            weak.upgrade().is_none(),
            "settled controller must not retain the operation observer"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_cancel_clears_turn_active_without_reaping() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let _held = be.prompt(&s, vec![]).await.unwrap();
        be.cancel(&s).await.unwrap();
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "warm cancel does NOT reap");
        assert!(
            !be.turn_active.lock().await.contains_key(&s),
            "cancel cleared turn_active"
        );
        be.retire().await.unwrap(); // retire still reaps the cached container
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_cancel_during_cache_miss_reaps_reserved_generation_before_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_spawn_gate(
            false,
            Arc::clone(&spawn_entered),
            Arc::clone(&spawn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(warm_backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("warm-cancel-during-spawn").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        backend.cancel(&session).await.unwrap();
        spawn_release.notify_one();

        assert!(matches!(
            prompt.await.unwrap(),
            Err(BridgeError::SessionExpired)
        ));
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
        assert!(!backend.warm.lock().await.contains_key(&session));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn warm_retire_during_cache_miss_reaps_reserved_generation_before_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn_entered = Arc::new(tokio::sync::Notify::new());
        let spawn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_spawn_gate(
            false,
            Arc::clone(&spawn_entered),
            Arc::clone(&spawn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(warm_backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("warm-retire-during-spawn").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        spawn_entered.notified().await;
        backend.retire().await.unwrap();
        spawn_release.notify_one();

        assert!(matches!(
            prompt.await.unwrap(),
            Err(BridgeError::SessionExpired)
        ));
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
        assert!(!backend.warm.lock().await.contains_key(&session));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn warm_cancel_during_first_turn_configuration_prevents_late_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let turn_entered = Arc::new(tokio::sync::Notify::new());
        let turn_release = Arc::new(tokio::sync::Notify::new());
        let spawn = CountingSpawn::with_turn_gate(
            false,
            Arc::clone(&turn_entered),
            Arc::clone(&turn_release),
        );
        let (reap, reaps) = counting_reap();
        let backend = Arc::new(warm_backend(root, spawn.clone(), reap).await);
        let session = SessionId::parse("warm-cancel-during-turn-config").unwrap();
        backend
            .configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        backend
            .configure_turn(&session, turn_meta("ctx-warm-cancel", 1, "op-warm-cancel"))
            .await;

        let prompt = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move { backend.prompt(&session, vec![]).await })
        };
        turn_entered.notified().await;
        backend.cancel(&session).await.unwrap();
        turn_release.notify_one();

        assert!(matches!(
            prompt.await.unwrap(),
            Err(BridgeError::SessionExpired)
        ));
        wait_for_reaps(&reaps, 1).await;
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(!backend.warm.lock().await.contains_key(&session));
    }

    #[tokio::test]
    async fn warm_edit_turn_open_failure_reaps_and_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(true), reap).await; // spawn fails (cache-miss open)
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        be.configure_turn(
            &s,
            turn_meta("ctx-warm-open-fail", 1, "turn-warm-open-fail"),
        )
        .await;
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("boom"), "got {err:?}");
        wait_for_reaps(&reaps, 1).await;
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "cache-miss spawn failure reaps the just-started container"
        );
        assert!(
            be.warm.lock().await.is_empty(),
            "no warm entry inserted on open failure"
        );
        assert!(
            !be.turn_active.lock().await.contains_key(&s),
            "turn_active cleared on open failure"
        );
        assert!(
            !be.pending_turn_meta.lock().await.contains_key(&s),
            "open_inner failure consumed pending turn metadata"
        );
    }

    #[tokio::test]
    async fn warm_stale_turn_guard_clear_is_epoch_scoped() {
        // The core of the review fix: a stale (early-drop) TurnGuard's detached clear must remove ONLY its
        // own turn's marker, never a later turn's.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, _) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        // A later turn owns the marker at epoch 5.
        be.turn_active.lock().await.insert(s.clone(), 5);
        // A STALE guard from an earlier turn (epoch 0) drops → its clear must NOT erase epoch 5.
        drop(TurnGuard {
            turn_active: be.turn_active.clone(),
            session: s.clone(),
            epoch: 0,
            armed: true,
        });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            be.turn_active.lock().await.get(&s),
            Some(&5),
            "stale clear must not erase a later turn's marker"
        );
        // A guard whose epoch MATCHES does clear it.
        drop(TurnGuard {
            turn_active: be.turn_active.clone(),
            session: s.clone(),
            epoch: 5,
            armed: true,
        });
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            !be.turn_active.lock().await.contains_key(&s),
            "matching-epoch clear removes the marker"
        );
    }

    #[tokio::test]
    async fn warm_cancel_then_reprompt_survives_old_stream_drop() {
        // cancel clears turn 1's marker; turn 2 takes a fresh epoch; dropping the OLD (turn 1) stream must
        // not erase turn 2's marker (review MAJOR: cancel-while-held + stale detached clear).
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(false), reap).await;
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let h1 = be.prompt(&s, vec![]).await.unwrap(); // turn 1 (epoch 0), held un-drained
        be.cancel(&s).await.unwrap(); // clears turn 1 marker, no reap
        assert_eq!(reaps.load(Ordering::SeqCst), 0, "cancel does not reap warm");
        let _h2 = be.prompt(&s, vec![]).await.unwrap(); // turn 2 (epoch 1), accepted + held
        assert!(
            be.turn_active.lock().await.contains_key(&s),
            "turn 2 is active"
        );
        drop(h1); // old stream drop → epoch-0 detached clear (must be a no-op vs turn 2's epoch 1)
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            be.turn_active.lock().await.contains_key(&s),
            "turn 2's marker survives the stale drop of turn 1"
        );
    }
}
