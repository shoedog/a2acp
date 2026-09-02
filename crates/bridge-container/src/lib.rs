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
use bridge_core::execution_policy::{
    BoundMcpDeliveryPayloadV1, BoundProviderEffectV1, BoundSessionSpecV1,
};
use bridge_core::ids::{NodeId, SessionId};
use bridge_core::permission::TurnMeta;
use bridge_core::ports::{
    AgentBackend, BackendCleanupDispositionV1, BackendObservers, BackendResourceFlightV1,
    BackendStream, DiagnosticObserver, RichEventSink,
};
use bridge_core::process::DurableProcessFlightAttemptV3;
use bridge_core::reaper::{
    spawn_detached, ContainerIdentityProbeFn, ContainerRuntimeIdentityV1,
    ContainerSubordinateCleanupFn, ReapAttemptFn, ReapController, ReapFailure, ReapFn,
};
use bridge_core::resource_flight::{BoundedRecoveryReasonV1, ResourceIdentityV1};
use bridge_core::retained_resource_flight::{CleanupDeadlineTransferV1, ResourceFlightOwnerV1};
use bridge_core::run_identity::{CanonicalContainerOwnershipV1, ContainerLabels, RunHandle};
use bridge_core::sandbox::{
    a2a_name, check_rw_target, compose_container_rw, compose_container_rw_with_source,
};
use bridge_core::session_cwd::SessionCwd;
use futures::StreamExt;
use tokio::sync::Mutex;

/// Injection seam so warm-reuse / reaper tests run Docker-free. Production wraps `AcpBackend::spawn`
/// (and applies the system `PolicyEngine` to the inner backend — see `main.rs`'s `AcpContainerSpawn`).
#[derive(Clone, Debug)]
pub struct ContainerSpawnRequestV1 {
    pub runtime: String,
    pub name: String,
    pub ownership: CanonicalContainerOwnershipV1,
}

pub struct ContainerSpawnResultV1 {
    pub backend: Arc<dyn AgentBackend>,
    pub immutable_container_id: String,
    pub ownership_labels: Vec<(String, String)>,
}

impl std::fmt::Debug for ContainerSpawnResultV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerSpawnResultV1")
            .field("immutable_container_id", &self.immutable_container_id)
            .field("ownership_label_count", &self.ownership_labels.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ContainerSpawn: Send + Sync {
    /// Production validates composition-owned host/runtime evidence before a generation is published.
    /// Test seams default to healthy so Docker-free behavior tests do not depend on host tooling.
    fn validate_infrastructure(&self, _sandbox: &SandboxConfig) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn spawn(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
        request: &ContainerSpawnRequestV1,
    ) -> Result<ContainerSpawnResultV1, BridgeError>;

    async fn spawn_observed(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
        request: &ContainerSpawnRequestV1,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<ContainerSpawnResultV1, BridgeError> {
        self.spawn(program, argv, cfg, request).await
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
    /// Slice-4 admission supplies this attempt-owned route. `None` is the
    /// production V2 default and keeps V3 destructive refusal unarmed.
    pub resource_flight_attempt_v3: Option<Arc<DurableProcessFlightAttemptV3>>,
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
    authority: Arc<SpawnAuthority>,
    /// Linearizes the final inner prompt installation against cancel, release,
    /// and retirement for this exact container generation. The prompt holds it
    /// only until `inner.prompt*` returns its stream; teardown starts the
    /// process-owned reaper first, then joins this gate before returning.
    dispatch_gate: Arc<Mutex<()>>,
}

enum SpawnAuthorityState {
    Pending {
        owners: Vec<ResourceFlightOwnerV1>,
    },
    Bound(Box<ReapController>),
    /// `spawn_failed` records only that the spawn call REPORTED failure — it is
    /// not proof that no container was created (the runtime command may have run
    /// before the failure). It authorizes reserve-time generation replacement
    /// (retry), never a custody-clearing release.
    RefusedUnknown {
        spawn_failed: bool,
    },
}

struct SpawnAuthority {
    state: StdMutex<SpawnAuthorityState>,
    notify: tokio::sync::Notify,
}

impl SpawnAuthority {
    fn pending() -> Self {
        Self {
            state: StdMutex::new(SpawnAuthorityState::Pending { owners: Vec::new() }),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn refuse_pre_id(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, SpawnAuthorityState::Pending { .. }) {
            *state = SpawnAuthorityState::RefusedUnknown {
                spawn_failed: false,
            };
            self.notify.notify_waiters();
        }
    }

    fn refuse_spawn_failed(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            SpawnAuthorityState::Pending { .. } => {
                *state = SpawnAuthorityState::RefusedUnknown { spawn_failed: true };
                self.notify.notify_waiters();
            }
            SpawnAuthorityState::RefusedUnknown { spawn_failed } => {
                *spawn_failed = true;
            }
            SpawnAuthorityState::Bound(_) => {}
        }
    }

    fn is_clearable_spawn_failed(&self) -> bool {
        matches!(
            *self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            SpawnAuthorityState::RefusedUnknown { spawn_failed: true }
        )
    }

    async fn controller(&self) -> Option<ReapController> {
        loop {
            let notified = self.notify.notified();
            {
                let state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match &*state {
                    SpawnAuthorityState::Pending { .. } => {}
                    SpawnAuthorityState::Bound(controller) => {
                        return Some(controller.as_ref().clone());
                    }
                    SpawnAuthorityState::RefusedUnknown { .. } => return None,
                }
            }
            notified.await;
        }
    }
    fn bound_controller(&self) -> Option<ReapController> {
        match &*self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            SpawnAuthorityState::Bound(controller) => Some(controller.as_ref().clone()),
            _ => None,
        }
    }
}

struct SpawnSettlementGuard {
    authority: Arc<SpawnAuthority>,
    armed: bool,
}
impl SpawnSettlementGuard {
    fn complete(&mut self) {
        self.armed = false;
    }

    fn refuse_spawn_failed(&mut self) {
        self.authority.refuse_spawn_failed();
        self.armed = false;
    }
}

impl Drop for SpawnSettlementGuard {
    fn drop(&mut self) {
        if self.armed {
            self.authority.refuse_pre_id();
        }
    }
}

impl ReapOwner {
    fn request_session_owner(&self, session: &SessionId) -> Result<(), ReapFailure> {
        let owner = ResourceFlightOwnerV1::new(
            NodeId::parse("container-session").map_err(|_| ReapFailure::FlightRefused)?,
            session.as_str(),
        )
        .map_err(|_| ReapFailure::FlightRefused)?;
        let mut state = self
            .authority
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            SpawnAuthorityState::Pending { owners } => {
                if !owners.contains(&owner) {
                    owners.push(owner);
                }
                Ok(())
            }
            SpawnAuthorityState::Bound(controller) => controller.attach_owner(owner).map(|_| ()),
            SpawnAuthorityState::RefusedUnknown { .. } => Err(ReapFailure::FlightRefused),
        }
    }

    fn bind(&self, controller: ReapController) -> Result<(), ReapFailure> {
        let mut state = self
            .authority
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let SpawnAuthorityState::Pending { owners } = &*state else {
            return Err(ReapFailure::FlightRefused);
        };
        for owner in owners.iter().cloned() {
            controller.attach_owner(owner)?;
        }
        *state = SpawnAuthorityState::Bound(Box::new(controller));
        self.authority.notify.notify_waiters();
        Ok(())
    }

    fn reap_detached(&self) {
        // A teardown entrance cannot wait for a future name-to-ID binding:
        // while spawn evidence is pending it closes the authority as Unknown,
        // which makes every later bind and destructive action refuse.
        self.authority.refuse_pre_id();
        let authority = Arc::clone(&self.authority);
        spawn_detached(async move {
            if let Some(controller) = authority.controller().await {
                let _ = controller.reap_observed().await;
            }
        });
    }

    async fn reap_observed(&self) -> Result<BackendCleanupDispositionV1, ReapFailure> {
        self.reap_detached();
        let Some(controller) = self.authority.controller().await else {
            return Ok(BackendCleanupDispositionV1::Unknown);
        };
        let protected_v3 = controller.is_protected_v3();
        match controller.reap_observed().await {
            Ok(()) => Ok(BackendCleanupDispositionV1::Complete),
            Err(error) if protected_v3 => Ok(match error {
                ReapFailure::IdentityUnavailable => BackendCleanupDispositionV1::Unknown,
                _ => BackendCleanupDispositionV1::Retained,
            }),
            Err(error) => Err(error),
        }
    }

    fn is_clearable_spawn_failed(&self) -> bool {
        self.authority.is_clearable_spawn_failed()
    }
}

fn require_complete_cleanup(disposition: BackendCleanupDispositionV1) -> Result<(), BridgeError> {
    match disposition {
        BackendCleanupDispositionV1::Complete => Ok(()),
        BackendCleanupDispositionV1::Retained => Err(BridgeError::DurableEvidenceUnavailable {
            reason: "container cleanup retained",
        }),
        BackendCleanupDispositionV1::Preserved => Err(BridgeError::DurableEvidenceUnavailable {
            reason: "container cleanup preserved",
        }),
        BackendCleanupDispositionV1::Unknown => Err(BridgeError::DurableEvidenceUnavailable {
            reason: "container cleanup unknown",
        }),
    }
}

fn container_cleanup_detail(disposition: BackendCleanupDispositionV1) -> &'static str {
    match disposition {
        BackendCleanupDispositionV1::Complete => "container.teardown.reaped",
        BackendCleanupDispositionV1::Retained => "container.teardown.retained",
        BackendCleanupDispositionV1::Preserved => "container.teardown.preserved",
        BackendCleanupDispositionV1::Unknown => "container.teardown.unknown",
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
    program: String,
    argv: Vec<String>,
    acp: AcpConfig,
    rw_canon: SessionCwd,
    spawn_request: ContainerSpawnRequestV1,
    ownership: CanonicalContainerOwnershipV1,
    flight_attempt: Option<Arc<DurableProcessFlightAttemptV3>>,
}

#[derive(Clone)]
struct ContainerSessionSpec {
    session: SessionSpec,
    bound_effect: Option<Arc<BoundProviderEffectV1>>,
}

type Inflight = Arc<Mutex<HashMap<SessionId, InflightState>>>;
type ReapFactory = Arc<
    dyn Fn(
            ResourceIdentityV1,
            String,
            CanonicalContainerOwnershipV1,
            ContainerSubordinateCleanupFn,
            Option<bridge_core::reaper::DurableContainerFlightV3>,
        ) -> Result<ReapController, ReapFailure>
        + Send
        + Sync,
>;

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
    session_cfg: Mutex<HashMap<SessionId, ContainerSessionSpec>>,
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
        let reap_factory: ReapFactory =
            Arc::new(move |identity, selector, ownership, subordinate, durable| {
                let expected_id = match &identity {
                    ResourceIdentityV1::ManagedContainer {
                        immutable_container_id,
                        ..
                    } => immutable_container_id.clone(),
                    _ => return Err(ReapFailure::IdentityUnavailable),
                };
                let observed_labels = ownership.ordered().to_vec();
                let probe: ContainerIdentityProbeFn = Arc::new(move |_runtime, _selector| {
                    let immutable_container_id = expected_id.clone();
                    let ownership_labels = observed_labels.clone();
                    Box::pin(async move {
                        Ok(ContainerRuntimeIdentityV1 {
                            immutable_container_id,
                            ownership_labels,
                        })
                    })
                });
                let reap_fn = Arc::clone(&reap_fn);
                let attempt: ReapAttemptFn = Arc::new(move |runtime, immutable_id| {
                    let reap_fn = Arc::clone(&reap_fn);
                    Box::pin(async move {
                        reap_fn(runtime, immutable_id);
                        Ok(())
                    })
                });
                match durable {
                    Some(durable) => ReapController::managed_durable_v3(
                        identity,
                        selector,
                        ownership,
                        attempt,
                        probe,
                        subordinate,
                        durable,
                    ),
                    None => ReapController::managed_legacy_v2(
                        identity,
                        selector,
                        ownership,
                        attempt,
                        probe,
                        subordinate,
                    ),
                }
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
        let reap_factory: ReapFactory = Arc::new(
            |identity, selector, ownership, subordinate, durable| match durable {
                Some(durable) => ReapController::managed_production_v3(
                    identity,
                    selector,
                    ownership,
                    subordinate,
                    durable,
                ),
                None => ReapController::managed_production_v2(
                    identity,
                    selector,
                    ownership,
                    subordinate,
                ),
            },
        );
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
        let reap_factory: ReapFactory = Arc::new(
            |identity, selector, ownership, subordinate, durable| match durable {
                Some(durable) => ReapController::managed_production_v3(
                    identity,
                    selector,
                    ownership,
                    subordinate,
                    durable,
                ),
                None => ReapController::managed_production_v2(
                    identity,
                    selector,
                    ownership,
                    subordinate,
                ),
            },
        );
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
    fn prepare_inner(&self, spec: &ContainerSessionSpec) -> Result<PreparedInner, BridgeError> {
        let runtime = self.cfg.sandbox.runtime().to_string();
        let cwd = spec.session.cwd.clone().ok_or(BridgeError::ConfigInvalid {
            reason: "missing session cwd".into(),
        })?;
        let rw_canon = self.resolve_rw_target(&cwd)?;
        let delivery_cwd = if spec.bound_effect.is_some() {
            cwd.clone()
        } else {
            rw_canon.clone()
        };
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
        let kind = if self.is_warm() { "warm" } else { "perturn" };
        let repo = delivery_cwd.as_str();
        let label_model: ContainerLabels = self.cfg.run.labels(
            "rw",
            kind,
            &self.cfg.agent,
            &self.owner,
            Some(repo),
            Some(repo),
        );
        let ownership = label_model.canonical_ownership();
        let labels = ownership.ordered().to_vec();
        let owner = ReapOwner {
            generation,
            authority: Arc::new(SpawnAuthority::pending()),
            dispatch_gate: Arc::new(Mutex::new(())),
        };
        let spawn_request = ContainerSpawnRequestV1 {
            runtime,
            name: name.clone(),
            ownership: ownership.clone(),
        };
        // Native codex MCP (ADR-0028): append `-c mcp_servers.*` args to the inner codex-acp argv,
        // `{cwd}`-substituted with THIS turn's `:rw` clone (identical-path mount → the same path
        // resolves inside the container). claude/non-codex leave `mcp` empty.
        let inner_args: Vec<String> = match spec.bound_effect.as_deref() {
            Some(effect) => match effect.delivery().payload() {
                BoundMcpDeliveryPayloadV1::CodexNative(suffix) => {
                    let mut args = self.cfg.args.clone();
                    args.extend(suffix.iter().cloned());
                    args
                }
                BoundMcpDeliveryPayloadV1::Acp(_) => self.cfg.args.clone(),
                BoundMcpDeliveryPayloadV1::KiroNative { .. } => {
                    return Err(BridgeError::ConfigMismatch {
                        field: "bound_container_mcp_delivery",
                    });
                }
            },
            None if matches!(
                self.cfg.mcp_delivery,
                bridge_core::mcp::McpDelivery::CodexNative
            ) && !self.cfg.mcp.is_empty() =>
            {
                let mut args = self.cfg.args.clone();
                args.extend(bridge_core::mcp::render_codex_mcp_args(
                    &self.cfg.mcp,
                    rw_canon.as_str(),
                ));
                args
            }
            None => self.cfg.args.clone(),
        };
        let (program, argv) = if spec.bound_effect.is_some() {
            compose_container_rw_with_source(
                &self.cfg.sandbox,
                &rw_canon,
                &delivery_cwd,
                &name,
                &self.cfg.cmd,
                &inner_args,
                &labels,
            )
        } else {
            compose_container_rw(
                &self.cfg.sandbox,
                &rw_canon,
                &name,
                &self.cfg.cmd,
                &inner_args,
                &labels,
            )
        };
        let delivery_mcp = match spec.bound_effect.as_deref() {
            Some(effect) => match effect.delivery().payload() {
                BoundMcpDeliveryPayloadV1::Acp(servers) => servers.clone(),
                BoundMcpDeliveryPayloadV1::CodexNative(_)
                | BoundMcpDeliveryPayloadV1::KiroNative { .. } => Vec::new(),
            },
            None => self.cfg.mcp.clone(),
        };
        let acp = AcpConfig {
            agent_id: self.cfg.agent.clone(),
            cwd: PathBuf::from(delivery_cwd.as_str()),
            model: self.cfg.model.clone(),
            mode: self.cfg.mode.clone(),
            auth_method: self.cfg.auth_method.clone(),
            pre_authenticated: self.cfg.pre_authenticated,
            watchdog: self.cfg.watchdog.clone(),
            process_flight_route_v3: self.cfg.resource_flight_attempt_v3.as_ref().map(|attempt| {
                let owner = ResourceFlightOwnerV1::new(
                    NodeId::parse("container-inner-process").expect("static node id"),
                    format!("inner-{name}"),
                )
                .expect("non-empty owner key");
                bridge_acp::acp_backend::AcpProcessFlightRouteV3::new(
                    Arc::clone(attempt),
                    format!("inner-{name}"),
                    owner,
                )
            }),
            prefix_attestation_transport:
                bridge_acp::acp_backend::PrefixAttestationTransport::Unsupported,
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
            diagnostic_redactor: bridge_core::diagnostics::DiagnosticRedactor::new(
                bridge_core::mcp::env_redaction_values(&delivery_mcp, delivery_cwd.as_str()),
            ),
            child_env_remove: {
                let mut variables: Vec<String> = delivery_mcp
                    .iter()
                    .flat_map(|server| server.env.iter())
                    .filter_map(|(_, source)| source.source_name().map(str::to_owned))
                    .collect();
                variables.sort();
                variables.dedup();
                variables
            },
            // :rw has its own reaper (this crate); the inner AcpBackend's :ro reaper stays off.
            container: None,
            // MCP delivery to the inner CONTAINER agent (#1b):
            //  - CodexNative: rides the codex-acp argv `-c mcp_servers.*` (rendered into `inner_args` above)
            //    -> the inner backend's ACP-param list stays EMPTY (ADR-0028).
            //  - Acp (claude): the inner AcpBackend mints `NewSessionRequest.mcpServers` from this list,
            //    `{cwd}`-substituted at mint with this turn's clone -> in-container lsp/prism nav for claude.
            //  - KiroNative: kiro honors neither channel for stdio MCP (settings file) -> not wired here.
            mcp: if spec.bound_effect.is_some() {
                Vec::new()
            } else if matches!(self.cfg.mcp_delivery, bridge_core::mcp::McpDelivery::Acp) {
                self.cfg.mcp.clone()
            } else {
                Vec::new()
            },
        };
        Ok(PreparedInner {
            owner,
            program,
            argv,
            acp,
            rw_canon,
            spawn_request,
            ownership,
            flight_attempt: self.cfg.resource_flight_attempt_v3.clone(),
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
        if let Some(existing) = reaps.get(session) {
            if existing.is_clearable_spawn_failed() {
                reaps.remove(session);
            } else {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!(
                        "session {} cleanup is still owned by its previous container generation",
                        session.as_str()
                    ),
                });
            }
        }
        owner
            .request_session_owner(session)
            .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
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
        spec: &ContainerSessionSpec,
        prepared: PreparedInner,
        diagnostic_observer: Option<Arc<dyn DiagnosticObserver>>,
    ) -> Result<WarmInner, BridgeError> {
        let PreparedInner {
            owner,
            program,
            argv,
            acp,
            rw_canon,
            spawn_request,
            ownership,
            flight_attempt,
        } = prepared;
        // Until this guard is disarmed no immutable authority exists. Drop or
        // caller cancellation settles the pending generation as Unknown and
        // can never dispatch a runtime removal.
        let mut spawn_settlement = SpawnSettlementGuard {
            authority: Arc::clone(&owner.authority),
            armed: true,
        };
        let spawned = match diagnostic_observer {
            Some(observer) => {
                self.spawn
                    .spawn_observed(&program, &argv, acp, &spawn_request, observer)
                    .await
            }
            None => self.spawn.spawn(&program, &argv, acp, &spawn_request).await,
        };
        let spawned = match spawned {
            Ok(spawned) => spawned,
            Err(e) => {
                spawn_settlement.refuse_spawn_failed();
                owner.reap_detached();
                return Err(e);
            }
        };
        if spawned.immutable_container_id.is_empty()
            || ownership
                .validate_observed(&spawned.ownership_labels)
                .is_err()
        {
            owner.reap_detached();
            return Err(BridgeError::IdentityUnavailable);
        }
        let identity_generation = format!("container-id:{}", spawned.immutable_container_id);
        let identity = ResourceIdentityV1::ManagedContainer {
            generation: identity_generation.clone(),
            runtime: spawn_request.runtime.clone(),
            immutable_container_id: spawned.immutable_container_id.clone(),
            ownership_labels_digest: ownership.digest().clone(),
        };
        let durable = match flight_attempt {
            Some(attempt) => {
                let flight_owner = ResourceFlightOwnerV1::new(
                    NodeId::parse("container-spawn").expect("static node id"),
                    identity_generation.clone(),
                )
                .map_err(|_| BridgeError::IdentityUnavailable)?;
                Some(
                    attempt
                        .bind_container_generation(identity_generation, flight_owner)
                        .map_err(|_| BridgeError::IdentityUnavailable)?,
                )
            }
            None => None,
        };
        let inner = spawned.backend;
        let subordinate_inner = Arc::clone(&inner);
        let subordinate_session = session.clone();
        let subordinate: ContainerSubordinateCleanupFn = Arc::new(move || {
            let inner = Arc::clone(&subordinate_inner);
            let session = subordinate_session.clone();
            Box::pin(async move { inner.cancel(&session).await.map_err(|_| ()) })
        });
        let controller = (self.reap_factory)(
            identity,
            spawn_request.name,
            ownership,
            subordinate,
            durable,
        )
        .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
        owner
            .bind(controller)
            .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
        spawn_settlement.complete();
        let configure = if let Some(effect) = &spec.bound_effect {
            inner
                .configure_bound_session(
                    session,
                    &BoundSessionSpecV1 {
                        session: spec.session.clone(),
                        provider_effect: Arc::clone(effect),
                    },
                )
                .await
        } else {
            // Legacy V1 canonicalizes its writable target before minting.
            let mut canonical = spec.session.clone();
            canonical.cwd = Some(rw_canon.clone());
            inner.configure_session(session, &canonical).await
        };
        if let Err(e) = configure {
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
            let configure = if let Some(effect) = &spec.bound_effect {
                inner
                    .configure_bound_session(
                        session,
                        &BoundSessionSpecV1 {
                            session: spec.session.clone(),
                            provider_effect: Arc::clone(effect),
                        },
                    )
                    .await
            } else {
                let mut canonical = spec.session.clone();
                canonical.cwd = Some(rw_canon);
                inner.configure_session(session, &canonical).await
            };
            if let Err(e) = configure {
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
        drop(_dispatch);
        if was_reserving {
            let disposition = owner
                .reap_observed()
                .await
                .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
            require_complete_cleanup(disposition)?;
        } else if let Some(inner) = inner {
            // Live warm cancellation is turn-scoped and intentionally retains
            // the outer container for reuse; retirement owns its flight.
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
        // The controller owns the terminal cleanup decision before any async
        // gate/map wait. Canceling this waiter can detach reporting, but cannot
        // reopen destructive authority after a pre-ID Unknown refusal.
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
        result: &Result<BackendCleanupDispositionV1, ReapFailure>,
    ) {
        // Only a proven-Complete reap surrenders custody here. A spawn-failure
        // refusal (`Unknown`) authorizes reserve-time REPLACEMENT only: a spawn
        // `Err` does not prove no container was created (the runtime command may
        // have run before the failure), so clearing on release would let a later
        // vacuous release report Complete for a possibly-live orphan. A retry
        // that collides with such an orphan's name fails loudly at `run`.
        let clear = matches!(result, Ok(BackendCleanupDispositionV1::Complete));
        if clear {
            if let Some(owner) = owner {
                self.clear_reap_owner(session, owner.generation);
            }
        }
    }

    async fn release_warm_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, ReapFailure> {
        let (owner, _inner) = self.begin_warm_cleanup(session).await;
        let result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(BackendCleanupDispositionV1::Complete),
        };
        self.finish_reap(session, &owner, &result);
        result
    }

    async fn release_warm_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        use bridge_core::diagnostics::{DiagnosticPhase, PhaseStatus};

        let (owner, inner) = self.begin_warm_cleanup(session).await;
        let start_result = record_container_transition(
            &observer,
            DiagnosticPhase::Teardown,
            PhaseStatus::Started,
            Some("container.teardown.reap"),
        )
        .await;

        let _inner = inner;
        let reap_result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(BackendCleanupDispositionV1::Complete),
        };
        self.finish_reap(session, &owner, &reap_result);
        start_result?;
        match reap_result {
            Ok(disposition) => {
                record_container_transition(
                    &observer,
                    DiagnosticPhase::Teardown,
                    PhaseStatus::Completed,
                    Some(container_cleanup_detail(disposition)),
                )
                .await?;
                Ok(disposition)
            }
            Err(failure) => Err(record_reap_failure(&observer, failure).await),
        }
    }

    async fn release_cold_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, ReapFailure> {
        let (owner, _inner) = self.begin_cold_cleanup(session).await;
        let result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(BackendCleanupDispositionV1::Complete),
        };
        self.finish_reap(session, &owner, &result);
        result
    }

    async fn release_cold_observed(
        &self,
        session: &SessionId,
        observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        use bridge_core::diagnostics::{DiagnosticPhase, PhaseStatus};

        let (owner, inner) = self.begin_cold_cleanup(session).await;
        let start_result = record_container_transition(
            &observer,
            DiagnosticPhase::Teardown,
            PhaseStatus::Started,
            Some("container.teardown.reap"),
        )
        .await;
        let _inner = inner;
        let reap_result = match &owner {
            Some(owner) => owner.reap_observed().await,
            None => Ok(BackendCleanupDispositionV1::Complete),
        };
        self.finish_reap(session, &owner, &reap_result);
        start_result?;
        match reap_result {
            Ok(disposition) => {
                record_container_transition(
                    &observer,
                    DiagnosticPhase::Teardown,
                    PhaseStatus::Completed,
                    Some(container_cleanup_detail(disposition)),
                )
                .await?;
                Ok(disposition)
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
        if spec.session.cwd.is_none() {
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

    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        Ok(if self.cfg.resource_flight_attempt_v3.is_some() {
            BackendResourceFlightV1::ProtectedV3
        } else {
            BackendResourceFlightV1::LegacyV2
        })
    }

    fn attach_resource_flight_owner_v1(
        &self,
        session: &SessionId,
    ) -> Result<BackendResourceFlightV1, BridgeError> {
        if let Some(owner) = self.current_reap_owner(session) {
            owner
                .request_session_owner(session)
                .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
        }
        self.resource_flight_v1()
    }

    fn transfer_cleanup_deadline_v1(
        &self,
        session: &SessionId,
        reason: BoundedRecoveryReasonV1,
    ) -> Result<CleanupDeadlineTransferV1, BridgeError> {
        self.current_reap_owner(session)
            .and_then(|owner| owner.authority.bound_controller())
            .ok_or(BridgeError::ResourceFlightUnsupported)?
            .transfer_cleanup_deadline(reason)
            .map_err(|failure| BridgeError::agent_crashed(failure.code()))
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
            let _state = {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(session).map(InflightState::generation) == Some(owner.generation) {
                    inflight.remove(session)
                } else {
                    None
                }
            };
            drop(_dispatch);
            let disposition = owner
                .reap_observed()
                .await
                .map_err(|failure| BridgeError::agent_crashed(failure.code()))?;
            require_complete_cleanup(disposition)?;
        }
        Ok(())
    }

    async fn configure_session(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.session_cfg.lock().await.insert(
            session.clone(),
            ContainerSessionSpec {
                session: spec.clone(),
                bound_effect: None,
            },
        );
        Ok(())
    }

    async fn configure_bound_session(
        &self,
        session: &SessionId,
        spec: &BoundSessionSpecV1,
    ) -> Result<(), BridgeError> {
        let frozen = spec.provider_effect.frozen();
        let cwd = spec
            .session
            .cwd
            .as_ref()
            .ok_or(BridgeError::ConfigMismatch {
                field: "bound_session_cwd",
            })?;
        use bridge_core::mcp::McpDelivery;
        let channel_matches = matches!(
            (
                self.cfg.mcp_delivery,
                spec.provider_effect.delivery().payload()
            ),
            (McpDelivery::Acp, BoundMcpDeliveryPayloadV1::Acp(_))
                | (
                    McpDelivery::CodexNative,
                    BoundMcpDeliveryPayloadV1::CodexNative(_)
                )
        );
        if frozen.effect.agent.as_str() != self.cfg.agent
            || cwd != &frozen.effect.effective_session_cwd
            || cwd != frozen.checkout.effective_cwd()
            || frozen.effect.mcp_delivery_digest != *spec.provider_effect.delivery().digest()
            || !channel_matches
        {
            return Err(BridgeError::ConfigMismatch {
                field: "bound_provider_effect",
            });
        }
        // Perform containment/infrastructure validation before publishing the session stash, but do
        // not derive or rewrite the persisted lexical delivery cwd.
        let _ = self.resolve_rw_target(cwd)?;
        self.session_cfg.lock().await.insert(
            session.clone(),
            ContainerSessionSpec {
                session: spec.session.clone(),
                bound_effect: Some(Arc::clone(&spec.provider_effect)),
            },
        );
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

    async fn forget_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        let result = if self.is_warm() {
            Ok(BackendCleanupDispositionV1::Complete)
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
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
        let result = if self.is_warm() {
            Ok(BackendCleanupDispositionV1::Complete)
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

    async fn release_session_checked(
        &self,
        session: &SessionId,
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
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
    ) -> Result<BackendCleanupDispositionV1, BridgeError> {
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
        let mut first_error = None;
        for (session, owner) in owners {
            let _dispatch = owner.dispatch_gate.lock().await;
            let mut _inner = None;
            {
                let mut inflight = self.inflight.lock().await;
                if inflight.get(&session).map(InflightState::generation) == Some(owner.generation) {
                    if let Some(InflightState::Live(turn)) = inflight.remove(&session) {
                        _inner = Some(turn.inner);
                    }
                }
                let mut warm = self.warm.lock().await;
                if warm.get(&session).map(|warm| warm.owner.generation) == Some(owner.generation) {
                    if let Some(warm) = warm.remove(&session) {
                        _inner = Some(warm.inner);
                    }
                }
            }
            self.turn_active.lock().await.remove(&session);
            drop(_dispatch);
            let result = owner.reap_observed().await;
            self.finish_reap(&session, &Some(owner.clone()), &result);
            if first_error.is_none() {
                first_error = match result {
                    Ok(disposition) => require_complete_cleanup(disposition).err(),
                    Err(failure) => Some(BridgeError::agent_crashed(failure.code())),
                };
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
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
    use bridge_core::terminal_evidence::{
        AcpChildLiveness, EvidenceCapability, EvidenceCompleteness, SharedTurnEvidence,
        TerminalEvidenceSink, TurnEvidenceBinding,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    #[tokio::test]
    #[ignore = "requires a host Docker daemon and the alpine image"]
    async fn docker_identity_template_round_trips_all_a2a_labels() {
        let name = format!("a2a-identity-roundtrip-{}", std::process::id());
        let started = std::process::Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &name,
                "--label",
                "a2a.managed=1",
                "--label",
                "a2a.owner=roundtrip-owner",
                "--label",
                "a2a.future=image-extra",
                "alpine",
                "sleep",
                "60",
            ])
            .output()
            .expect("docker must be executable for the ignored host test");
        assert!(
            started.status.success(),
            "docker run failed: {}",
            String::from_utf8_lossy(&started.stderr)
        );

        let observed = bridge_core::reaper::production_container_identity("docker", &name)
            .await
            .expect("the production identity template must parse Docker output");
        let removed = std::process::Command::new("docker")
            .args(["rm", "-f", &observed.immutable_container_id])
            .output()
            .expect("docker rm must execute");
        assert!(removed.status.success());
        assert!(!observed.immutable_container_id.is_empty());
        assert_eq!(
            observed.ownership_labels,
            vec![
                ("a2a.future".into(), "image-extra".into()),
                ("a2a.managed".into(), "1".into()),
                ("a2a.owner".into(), "roundtrip-owner".into()),
            ]
        );
    }

    #[test]
    fn unit_cleanup_wrappers_accept_complete_and_refuse_protective_outcomes() {
        assert_eq!(
            require_complete_cleanup(BackendCleanupDispositionV1::Complete),
            Ok(())
        );
        assert_eq!(
            require_complete_cleanup(BackendCleanupDispositionV1::Retained),
            Err(BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup retained"
            })
        );
        assert_eq!(
            require_complete_cleanup(BackendCleanupDispositionV1::Preserved),
            Err(BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup preserved"
            })
        );
        assert_eq!(
            require_complete_cleanup(BackendCleanupDispositionV1::Unknown),
            Err(BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup unknown"
            })
        );
    }

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
        terminal_evidence_v1: bool,
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
                    prefix_attestation: Default::default(),
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

        async fn configure_bound_session(
            &self,
            _session: &SessionId,
            _spec: &BoundSessionSpecV1,
        ) -> Result<(), BridgeError> {
            self.call_order.lock().await.push("configure_bound_session");
            Ok(())
        }

        async fn prompt_with_observers(
            &self,
            session: &SessionId,
            parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            let capability = if self.terminal_evidence_v1 {
                EvidenceCapability::V1
            } else {
                EvidenceCapability::Unsupported
            };
            observers.terminal_evidence.declare_capability(capability);
            if self.terminal_evidence_v1 {
                observers
                    .terminal_evidence
                    .record_child_liveness(AcpChildLiveness::Live);
            }
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
        terminal_evidence_v1: AtomicBool,
        extra_ownership_label: AtomicBool,
        mismatched_ownership_label: AtomicBool,
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

    #[async_trait]
    impl ContainerSpawn for RejectingPreflightSpawn {
        fn validate_infrastructure(&self, sandbox: &SandboxConfig) -> Result<(), BridgeError> {
            let mut invalid = sandbox.clone();
            invalid.runtime = Some(
                std::env::current_exe()
                    .expect("current test executable should resolve")
                    .to_str()
                    .expect("current test executable path should be UTF-8")
                    .to_owned(),
            );
            invalid.image.clear();
            bridge_core::sandbox::validate_container_infrastructure(&invalid)
        }

        async fn spawn(
            &self,
            _program: &str,
            _argv: &[String],
            _cfg: AcpConfig,
            _request: &ContainerSpawnRequestV1,
        ) -> Result<ContainerSpawnResultV1, BridgeError> {
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::InvalidStateTransition)
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
                terminal_evidence_v1: AtomicBool::new(false),
                extra_ownership_label: AtomicBool::new(false),
                mismatched_ownership_label: AtomicBool::new(false),
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
            request: &ContainerSpawnRequestV1,
        ) -> Result<ContainerSpawnResultV1, BridgeError> {
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
                terminal_evidence_v1: self.terminal_evidence_v1.load(Ordering::SeqCst),
                prompt_gate: self.prompt_gate.clone(),
                turn_gate: self.turn_gate.clone(),
                cancel_gate: self.cancel_gate.clone(),
            });
            *self.last_inner.lock().await = Some(inner.clone());
            let mut ownership_labels = request.ownership.ordered().to_vec();
            if self.extra_ownership_label.load(Ordering::SeqCst) {
                ownership_labels.push(("a2a.future".into(), "unexpected".into()));
            }
            if self.mismatched_ownership_label.load(Ordering::SeqCst) {
                if let Some(first) = ownership_labels.first_mut() {
                    first.1 = "tampered".into();
                }
            }
            Ok(ContainerSpawnResultV1 {
                backend: inner,
                immutable_container_id: format!("sha256:test-{}", request.name),
                ownership_labels,
            })
        }

        async fn spawn_observed(
            &self,
            program: &str,
            argv: &[String],
            cfg: AcpConfig,
            request: &ContainerSpawnRequestV1,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<ContainerSpawnResultV1, BridgeError> {
            self.observed_count.fetch_add(1, Ordering::SeqCst);
            observer
                .record(test_transition(
                    DiagnosticPhase::Spawn,
                    PhaseStatus::Started,
                    None,
                ))
                .await?;
            let inner = self.spawn(program, argv, cfg, request).await?;
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

    fn bound_test_owner(reap: ReapFn) -> ReapOwner {
        let labels = ContainerLabels {
            role: "rw".into(),
            kind: "perturn".into(),
            agent: "impl".into(),
            owner: "inst".into(),
            run_id: "run0".into(),
            host: "h".into(),
            lease: "/l/run0.lock".into(),
            repo: None,
            cwd: None,
            start: "0".into(),
        };
        let ownership = labels.canonical_ownership();
        let immutable_id = "container-id-off-runtime".to_owned();
        let observed_labels = ownership.ordered().to_vec();
        let expected_id = immutable_id.clone();
        let probe: ContainerIdentityProbeFn = Arc::new(move |_runtime, _selector| {
            let immutable_container_id = expected_id.clone();
            let ownership_labels = observed_labels.clone();
            Box::pin(async move {
                Ok(ContainerRuntimeIdentityV1 {
                    immutable_container_id,
                    ownership_labels,
                })
            })
        });
        let attempt: ReapAttemptFn = Arc::new(move |runtime, immutable_id| {
            let reap = Arc::clone(&reap);
            Box::pin(async move {
                reap(runtime, immutable_id);
                Ok(())
            })
        });
        let subordinate: ContainerSubordinateCleanupFn = Arc::new(|| Box::pin(async { Ok(()) }));
        let controller = ReapController::managed_legacy_v2(
            ResourceIdentityV1::ManagedContainer {
                generation: "container-id:off-runtime".into(),
                runtime: "docker".into(),
                immutable_container_id: immutable_id,
                ownership_labels_digest: ownership.digest().clone(),
            },
            "a2a-rw-inst-0",
            ownership,
            attempt,
            probe,
            subordinate,
        )
        .unwrap();
        ReapOwner {
            generation: 0,
            authority: Arc::new(SpawnAuthority {
                state: StdMutex::new(SpawnAuthorityState::Bound(Box::new(controller))),
                notify: tokio::sync::Notify::new(),
            }),
            dispatch_gate: Arc::new(Mutex::new(())),
        }
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
            resource_flight_attempt_v3: None,
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

    fn managed_factory(attempt: ReapAttemptFn) -> ReapFactory {
        Arc::new(move |identity, selector, ownership, subordinate, durable| {
            let expected_id = match &identity {
                ResourceIdentityV1::ManagedContainer {
                    immutable_container_id,
                    ..
                } => immutable_container_id.clone(),
                _ => return Err(ReapFailure::IdentityUnavailable),
            };
            let labels = ownership.ordered().to_vec();
            let probe: ContainerIdentityProbeFn = Arc::new(move |_runtime, _selector| {
                let immutable_container_id = expected_id.clone();
                let ownership_labels = labels.clone();
                Box::pin(async move {
                    Ok(ContainerRuntimeIdentityV1 {
                        immutable_container_id,
                        ownership_labels,
                    })
                })
            });
            match durable {
                Some(durable) => ReapController::managed_durable_v3(
                    identity,
                    selector,
                    ownership,
                    Arc::clone(&attempt),
                    probe,
                    subordinate,
                    durable,
                ),
                None => ReapController::managed_legacy_v2(
                    identity,
                    selector,
                    ownership,
                    Arc::clone(&attempt),
                    probe,
                    subordinate,
                ),
            }
        })
    }

    async fn warm_backend_with_attempt(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        attempt: ReapAttemptFn,
    ) -> ContainerRwBackend {
        let factory = managed_factory(attempt);
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
        let factory = managed_factory(attempt);
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
            turn_id: bridge_core::ids::TurnId::parse(format!("turn_{generation:032x}")).unwrap(),
            requested_mode: bridge_core::attestation::HarvestSanitizationMode::Off,
            prefix_attestation_request:
                bridge_core::attestation::PrefixAttestationRequest::Disabled,
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
    async fn extra_noncanonical_spawn_ownership_evidence_is_tolerated() {
        let mount = tempfile::tempdir().unwrap();
        let spawn = CountingSpawn::new(false);
        spawn.extra_ownership_label.store(true, Ordering::SeqCst);
        let (reap, reaps) = counting_reap();
        let backend = backend(mount.path().to_str().unwrap(), spawn.clone(), reap).await;
        let session = SessionId::parse("extra-spawn-label").unwrap();
        backend
            .configure_session(&session, &spec_cwd(mount.path().to_str().unwrap()))
            .await
            .unwrap();
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        while stream.next().await.is_some() {}
        assert_eq!(
            backend.release_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Complete
        );
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn canonical_ownership_mismatch_refuses_spawn_without_removal() {
        let mount = tempfile::tempdir().unwrap();
        let spawn = CountingSpawn::new(false);
        spawn
            .mismatched_ownership_label
            .store(true, Ordering::SeqCst);
        let (reap, reaps) = counting_reap();
        let backend = backend(mount.path().to_str().unwrap(), spawn.clone(), reap).await;
        let session = SessionId::parse("mismatched-spawn-label").unwrap();
        backend
            .configure_session(&session, &spec_cwd(mount.path().to_str().unwrap()))
            .await
            .unwrap();
        assert!(matches!(
            prompt_err(&backend, &session).await,
            BridgeError::IdentityUnavailable
        ));
        let owner = backend.current_reap_owner(&session).unwrap();
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert!(!inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn container_decorator_exposes_legacy_v2_until_attempt_route_is_supplied() {
        let (reap, _) = counting_reap();
        let backend = backend("/root", CountingSpawn::new(false), reap).await;
        let session = SessionId::parse("legacy-exposure").unwrap();
        assert_eq!(
            backend.resource_flight_v1().unwrap(),
            BackendResourceFlightV1::LegacyV2
        );
        assert_eq!(
            backend.attach_resource_flight_owner_v1(&session).unwrap(),
            BackendResourceFlightV1::LegacyV2
        );
    }

    #[tokio::test]
    async fn protected_container_exposes_real_generation_and_retains_identity_refusal() {
        let mount = tempfile::tempdir().unwrap();
        let journal_root = tempfile::tempdir().unwrap();
        let journal = Arc::new(
            bridge_core::retained_resource_flight::FileResourceFlightJournal::open(
                journal_root.path(),
                512,
            )
            .unwrap(),
        );
        let route = Arc::new(DurableProcessFlightAttemptV3::new(
            bridge_core::ids::AttemptId::mint().unwrap(),
            journal,
        ));
        let mut cfg = cfg_with_mount(mount.path().to_str().unwrap());
        cfg.resource_flight_attempt_v3 = Some(route);

        let removal_calls = Arc::new(AtomicUsize::new(0));
        let factory: ReapFactory = {
            let removal_calls = Arc::clone(&removal_calls);
            Arc::new(move |identity, selector, ownership, subordinate, durable| {
                let attempt: ReapAttemptFn = {
                    let removal_calls = Arc::clone(&removal_calls);
                    Arc::new(move |_runtime, _immutable_id| {
                        let removal_calls = Arc::clone(&removal_calls);
                        Box::pin(async move {
                            removal_calls.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    })
                };
                let labels = ownership.ordered().to_vec();
                let expected_selector = selector.clone();
                let probe: ContainerIdentityProbeFn =
                    Arc::new(move |_runtime, observed_selector| {
                        let labels = labels.clone();
                        let expected_selector = expected_selector.clone();
                        Box::pin(async move {
                            assert_eq!(observed_selector, expected_selector);
                            Ok(ContainerRuntimeIdentityV1 {
                                immutable_container_id: "same-name-successor-id".into(),
                                ownership_labels: labels,
                            })
                        })
                    });
                ReapController::managed_durable_v3(
                    identity,
                    selector,
                    ownership,
                    attempt,
                    probe,
                    subordinate,
                    durable.expect("protected config supplies the container route"),
                )
            })
        };
        let backend = ContainerRwBackend::new_with_reap_factory(
            cfg,
            CountingSpawn::new(false),
            "inst".into(),
            factory,
        )
        .await
        .unwrap();
        let session = SessionId::parse("protected-exposure").unwrap();
        assert_eq!(
            backend.resource_flight_v1().unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        // Before spawn there is no generation to attach; the route is exposed
        // without inventing a capability from a future container name.
        assert_eq!(
            backend.attach_resource_flight_owner_v1(&session).unwrap(),
            BackendResourceFlightV1::ProtectedV3
        );
        backend
            .configure_session(&session, &spec_cwd(mount.path().to_str().unwrap()))
            .await
            .unwrap();
        let stream = backend.prompt(&session, vec![]).await.unwrap();
        assert_eq!(
            backend.attach_resource_flight_owner_v1(&session).unwrap(),
            BackendResourceFlightV1::ProtectedV3,
            "the decorator attaches to the published immutable-ID generation"
        );
        assert_eq!(
            backend.release_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Retained,
            "a leaked outer container must remain observable through the decorator"
        );
        drop(stream);
        assert_eq!(removal_calls.load(Ordering::SeqCst), 0);
    }

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
    async fn bound_container_codex_consumes_only_the_frozen_argv_suffix() {
        use bridge_core::domain::{AgentEntry, AgentKind};
        use bridge_core::execution_policy::{
            freeze_direct_checkout_v1, freeze_provider_attempt_v1, BoundSessionSpecV1,
            FrozenProviderLogicalSessionV1, PolicyNodeRefV1, ProviderFreezeInputV1,
        };
        use bridge_core::ids::AgentId;
        use bridge_core::mcp::{McpDelivery, McpServerSpec};

        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real-worktree");
        let logical = temp.path().join("logical-worktree");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &logical).unwrap();
        let root = logical.to_str().unwrap();
        let canonical_root = std::fs::canonicalize(&real).unwrap();
        let canonical_root = canonical_root.to_str().unwrap();
        let mut config = cfg_with_mount(root);
        config.cmd = "codex-acp".into();
        config.mcp_delivery = McpDelivery::CodexNative;
        config.mcp = vec![McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![],
        }];
        let entry = AgentEntry {
            id: AgentId::parse("impl").unwrap(),
            cmd: Some(config.cmd.clone()),
            base_url: None,
            api_key_env: None,
            args: config.args.clone(),
            kind: AgentKind::ContainerRw,
            model_provider: None,
            model: config.model.clone(),
            effort: None,
            mode: config.mode.clone(),
            preflight: false,
            fallback_models: vec![],
            cwd: None,
            session_cwd: None,
            sandbox: Some(config.sandbox.clone()),
            watchdog: config.watchdog.clone(),
            mcp: config.mcp.clone(),
            mcp_delivery: config.mcp_delivery,
            auth_method: config.auth_method.clone(),
            pre_authenticated: config.pre_authenticated,
            host_fallback_eligible: false,
            name: None,
            description: None,
            tags: vec![],
            version: None,
            extensions: Default::default(),
        };
        let bundle = freeze_provider_attempt_v1(&ProviderFreezeInputV1 {
            entry: &entry,
            overrides: None,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            logical_session: FrozenProviderLogicalSessionV1::Execute {
                candidate_ordinal: 0,
            },
            checkout: freeze_direct_checkout_v1(SessionCwd::parse(root).unwrap()),
            provider_effect_key: None,
        })
        .unwrap();
        let spawn = CountingSpawn::new(false);
        let (reap, _) = counting_reap();
        let mut backend =
            ContainerRwBackend::new_with_hooks(config, spawn.clone(), "inst".into(), reap)
                .await
                .unwrap();
        backend.cfg.mcp[0].args[1] = "/mutated-after-freeze".into();
        let session = SessionId::parse("bound-container").unwrap();
        let spec = BoundSessionSpecV1::new(EffectiveConfig::default(), Arc::new(bundle.bound));

        backend
            .configure_bound_session(&session, &spec)
            .await
            .unwrap();
        let mut stream = backend.prompt(&session, vec![]).await.unwrap();
        let argv = spawn.last_argv.lock().await.clone();
        assert!(argv.iter().any(|arg| arg.contains(root)));
        assert!(argv
            .iter()
            .any(|arg| { arg == &format!("{canonical_root}:{root}") }));
        assert!(!argv
            .iter()
            .any(|arg| { arg == &format!("{canonical_root}:{canonical_root}") }));
        assert!(!argv.iter().any(|arg| arg.contains("/mutated-after-freeze")));
        while stream.next().await.is_some() {}
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
    async fn prompt_spawn_failure_refuses_pre_id_removal_and_errors() {
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
        let owner = be.current_reap_owner(&s).unwrap();
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            0,
            "spawn failure has no immutable ID and MUST NOT remove by name"
        );
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
    async fn launch_failure_is_preserved_without_keyword_promotion_or_name_reap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(true);
        let (reap, reaps) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let session = SessionId::parse("post-failure-preserve").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        let error = prompt_err(&be, &session).await;
        let BridgeError::AgentCrashed { reason } = error else {
            panic!("opaque launch failure should be preserved");
        };
        assert_eq!(reason, "boom docker image network mount credential");
        assert_eq!(spawn.count.load(Ordering::SeqCst), 1);
        let owner = be.current_reap_owner(&session).unwrap();
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        assert!(be.inflight.lock().await.is_empty());
    }

    #[tokio::test]
    async fn cold_failed_spawn_is_joinable_but_next_prompt_retries_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(true);
        let (reap, _) = counting_reap();
        let be = backend(root, spawn.clone(), reap).await;
        let session = SessionId::parse("cold-never-published-retry").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();

        assert!(be.prompt(&session, vec![]).await.is_err());
        assert_eq!(spawn.count.load(Ordering::SeqCst), 1);
        let first_owner = be.current_reap_owner(&session).unwrap();
        assert_eq!(
            first_owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );

        assert!(be.prompt(&session, vec![]).await.is_err());
        assert_eq!(
            spawn.count.load(Ordering::SeqCst),
            2,
            "the sequential prompt must perform one fresh spawn attempt"
        );
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
    async fn cold_cancel_during_spawn_refuses_pre_id_generation() {
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
        let owner = backend.current_reap_owner(&session).unwrap();
        assert_eq!(
            backend.cancel(&session).await.unwrap_err(),
            BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup unknown"
            }
        );
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        spawn_release.notify_one();

        assert!(prompt.await.unwrap().is_err());
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(!inner.canceled.load(Ordering::SeqCst));
        assert!(!backend.inflight.lock().await.contains_key(&session));
    }

    #[tokio::test]
    async fn cold_retire_during_spawn_refuses_pre_id_generation() {
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
        let owner = backend.current_reap_owner(&session).unwrap();
        assert_eq!(
            backend.retire().await.unwrap_err(),
            BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup unknown"
            }
        );
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        spawn_release.notify_one();

        assert!(prompt.await.unwrap().is_err());
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(!inner.canceled.load(Ordering::SeqCst));
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
                        backend.release_session_checked(&session).await.map(|_| ())
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

    async fn assert_pre_id_teardown_refuses_unknown_without_removal(
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
        let teardown = {
            let backend = Arc::clone(&backend);
            let session = session.clone();
            tokio::spawn(async move {
                match action {
                    TeardownAction::Cancel => backend.cancel(&session).await,
                    TeardownAction::ReleaseChecked => {
                        backend.release_session_checked(&session).await.map(|_| ())
                    }
                    TeardownAction::Retire => backend.retire().await,
                }
            })
        };

        tokio::time::timeout(Duration::from_secs(1), teardown)
            .await
            .expect("pre-ID teardown settles without waiting for spawn")
            .unwrap()
            .unwrap();
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(reap_calls.load(Ordering::SeqCst), 0);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), reap_entered.notified())
                .await
                .is_err(),
            "pre-ID refusal must never dispatch the removal port"
        );

        spawn_release.notify_one();
        assert!(prompt.await.unwrap().is_err());
        assert_eq!(reap_calls.load(Ordering::SeqCst), 0);
        assert!(resource_exists.load(Ordering::SeqCst));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert!(!inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cold_checked_release_in_pre_id_window_refuses_unknown_without_removal() {
        assert_pre_id_teardown_refuses_unknown_without_removal(
            false,
            TeardownAction::ReleaseChecked,
        )
        .await;
    }

    #[tokio::test]
    async fn warm_checked_release_in_pre_id_window_refuses_unknown_without_removal() {
        assert_pre_id_teardown_refuses_unknown_without_removal(
            true,
            TeardownAction::ReleaseChecked,
        )
        .await;
    }

    #[tokio::test]
    async fn aborting_spawn_future_refuses_pre_id_cleanup_as_unknown() {
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
        prompt.abort();
        match prompt.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("aborted spawn prompt unexpectedly completed"),
        }

        match backend.cancel(&session).await {
            Err(BridgeError::DurableEvidenceUnavailable { .. }) => {}
            other => panic!("pre-ID cancel must refuse with typed unknown evidence, got {other:?}"),
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), owner.reap_observed())
                .await
                .expect("aborting spawn must open the cleanup settlement fence"),
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(reap_calls.load(Ordering::SeqCst), 0);
        assert!(!resource_exists.load(Ordering::SeqCst));
    }

    fn dormant_evidence(label: &str) -> Arc<SharedTurnEvidence> {
        Arc::new(SharedTurnEvidence::dormant(TurnEvidenceBinding {
            generation: 1,
            session_id: format!("session-{label}"),
            turn_id: format!("turn-{label}"),
            attempt_id: format!("attempt-{label}"),
            marker_nonce: "00112233445566778899aabbccddeeff".into(),
        }))
    }

    async fn assert_lazy_first_turn_observers_preserve_exact_evidence(warm: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        spawn.terminal_evidence_v1.store(true, Ordering::SeqCst);
        let be = if warm {
            warm_backend(root, spawn, counting_reap().0).await
        } else {
            backend(root, spawn, counting_reap().0).await
        };
        let label = if warm { "warm-v1" } else { "cold-v1" };
        let session = SessionId::parse(label).unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let sink = dormant_evidence(label);
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let mut stream = be
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(observer, None).with_attempt_telemetry(
                    Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder),
                    sink.clone(),
                ),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        assert_eq!(sink.capability(), EvidenceCapability::V1);
        assert!(sink.binding().is_some());
        assert_eq!(sink.child_liveness(), AcpChildLiveness::Live);
        if warm {
            be.retire().await.unwrap();
        }
    }

    #[tokio::test]
    async fn cold_first_turn_preserves_prompt_time_v1_and_exact_child_liveness() {
        assert_lazy_first_turn_observers_preserve_exact_evidence(false).await;
    }

    #[tokio::test]
    async fn warm_first_turn_preserves_prompt_time_v1_and_exact_child_liveness() {
        assert_lazy_first_turn_observers_preserve_exact_evidence(true).await;
    }

    #[tokio::test]
    async fn lazy_unsupported_inner_remains_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let spawn = CountingSpawn::new(false);
        let be = backend(root, spawn, counting_reap().0).await;
        let session = SessionId::parse("cold-unsupported").unwrap();
        be.configure_session(&session, &spec_cwd(root))
            .await
            .unwrap();
        let sink = dormant_evidence("cold-unsupported");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(16).unwrap());
        let mut stream = be
            .prompt_with_observers(
                &session,
                vec![],
                BackendObservers::new(observer, None).with_attempt_telemetry(
                    Arc::new(bridge_core::attempt_activity::NoopAttemptRecorder),
                    sink.clone(),
                ),
            )
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        sink.close();
        assert_eq!(sink.capability(), EvidenceCapability::Unsupported);
        assert_eq!(sink.observation().0, EvidenceCompleteness::Unsupported);
        assert_eq!(sink.child_liveness(), AcpChildLiveness::Unknown);
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
        let reaper = ContainerReaper {
            owner: bound_test_owner(reap),
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
        let controller = backend
            .current_reap_owner(&session)
            .unwrap()
            .authority
            .controller()
            .await
            .unwrap();

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
        let controller = backend
            .current_reap_owner(&session)
            .unwrap()
            .authority
            .controller()
            .await
            .unwrap();

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
    async fn observed_cold_release_reports_unknown_after_agent_spawn_failure() {
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
        assert_eq!(
            be.release_session_observed(&session, observer.clone())
                .await
                .unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let events = observer.snapshot().await;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].transition().status(), PhaseStatus::Started);
        assert_eq!(events[1].transition().status(), PhaseStatus::Completed);
        assert_eq!(
            events[1].transition().code().map(|code| code.as_str()),
            Some("container.teardown.unknown")
        );
        assert_eq!(
            be.release_session_checked(&session).await.unwrap(),
            BackendCleanupDispositionV1::Unknown
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
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

        // retire() joins every generation's reap flight before returning, so it
        // must run concurrently with the gated attempt: awaiting it inline would
        // deadlock against the attempt's release gate.
        let retire_task = {
            let be = Arc::clone(&be);
            tokio::spawn(async move { be.retire().await })
        };
        tokio::time::timeout(Duration::from_secs(30), entered.notified())
            .await
            .expect("retirement must enter the gated reap attempt");
        let observer = Arc::new(InMemoryDiagnosticObserver::new(8).unwrap());
        let observer_dyn: Arc<dyn DiagnosticObserver> = observer.clone();
        let weak = Arc::downgrade(&observer_dyn);
        let release_task = {
            let be = Arc::clone(&be);
            let session = session.clone();
            tokio::spawn(async move { be.release_session_observed(&session, observer_dyn).await })
        };
        tokio::task::yield_now().await;
        assert!(
            !release_task.is_finished(),
            "observed release must join the still-running retained reap"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        tokio::time::timeout(Duration::from_secs(30), release_task)
            .await
            .expect("observed release must settle once the gated reap attempt completes")
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(30), retire_task)
            .await
            .expect("retire must settle once the gated reap attempt completes")
            .unwrap()
            .unwrap();
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
    async fn warm_cancel_during_cache_miss_refuses_pre_id_generation() {
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
        let owner = backend.current_reap_owner(&session).unwrap();
        assert_eq!(
            backend.cancel(&session).await.unwrap_err(),
            BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup unknown"
            }
        );
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        spawn_release.notify_one();

        assert!(prompt.await.unwrap().is_err());
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        assert!(!backend.warm.lock().await.contains_key(&session));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(!inner.canceled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn warm_retire_during_cache_miss_refuses_pre_id_generation() {
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
        let owner = backend.current_reap_owner(&session).unwrap();
        assert_eq!(
            backend.retire().await.unwrap_err(),
            BridgeError::DurableEvidenceUnavailable {
                reason: "container cleanup unknown"
            }
        );
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        spawn_release.notify_one();

        assert!(prompt.await.unwrap().is_err());
        assert_eq!(reaps.load(Ordering::SeqCst), 0);
        assert!(!backend.warm.lock().await.contains_key(&session));
        let inner = spawn.last_inner.lock().await.clone().unwrap();
        assert_eq!(inner.prompts.load(Ordering::SeqCst), 0);
        assert!(!inner.canceled.load(Ordering::SeqCst));
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
    async fn warm_edit_turn_open_failure_refuses_name_removal_and_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(true), reap).await; // spawn fails before ID capture
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        be.configure_turn(
            &s,
            turn_meta("ctx-warm-open-fail", 1, "turn-warm-open-fail"),
        )
        .await;
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("boom"), "got {err:?}");
        let owner = be.current_reap_owner(&s).unwrap();
        assert_eq!(
            owner.reap_observed().await,
            Ok(BackendCleanupDispositionV1::Unknown)
        );
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            0,
            "cache-miss spawn failure has no immutable ID and MUST NOT remove by name"
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
