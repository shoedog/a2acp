//! Write-capable containerized ACP agent (Slice B2a + B2b-3c). [`ContainerRwBackend`] composes
//! [`bridge_acp::acp_backend::AcpBackend`] via the [`ContainerSpawn`] seam. Default `PerTurn` mode spawns
//! a fresh `:rw` container per `prompt` turn and reaps it on every terminal path. `Warm` mode
//! (`new_warm`) reuses ONE container + ONE ACP session across the turns of a session, reaping ONLY at
//! `retire()` — used by the `implement` review→tweak loop so edit + fix turns share continuity.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge_acp::acp_backend::AcpConfig;
use bridge_core::domain::{Part, SandboxConfig, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream};
use bridge_core::reaper::{production_reap_fn, reap_once, spawn_detached, ReapFn};
use bridge_core::run_identity::RunHandle;
use bridge_core::sandbox::{a2a_name, check_rw_target, compose_container_rw};
use bridge_core::session_cwd::SessionCwd;
use futures::StreamExt;
use tokio::sync::Mutex;

/// Injection seam so warm-reuse / reaper tests run Docker-free. Production wraps `AcpBackend::spawn`
/// (and applies the system `PolicyEngine` to the inner backend — see `main.rs`'s `AcpContainerSpawn`).
#[async_trait]
pub trait ContainerSpawn: Send + Sync {
    async fn spawn(
        &self,
        program: &str,
        argv: &[String],
        cfg: AcpConfig,
    ) -> Result<Arc<dyn AgentBackend>, BridgeError>;
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
struct InflightTurn {
    inner: Arc<dyn AgentBackend>,
    name: String,
    reaped: Arc<AtomicBool>,
}

/// A spawned, configured inner backend + its container identity. Shared shape for per-turn (promoted to
/// [`InflightState::Live`]) and warm (cached in `warm`). `rw_canon` is the canonicalized `:rw` target the
/// session was configured with (re-applied on a warm reuse turn).
struct WarmInner {
    inner: Arc<dyn AgentBackend>,
    name: String,
    reaped: Arc<AtomicBool>,
    rw_canon: SessionCwd,
}

/// One entry per session: `Reserving` is held across the (async) spawn so a concurrent second prompt is
/// rejected atomically (no check-then-insert race); `Live` carries the cancel handle.
enum InflightState {
    Reserving,
    Live(InflightTurn),
}

type Inflight = Arc<Mutex<HashMap<SessionId, InflightState>>>;

/// Per-turn (cold) vs warm (one container + session reused across turns, reaped only at `retire`).
#[derive(Clone, Copy, PartialEq)]
enum Lifecycle {
    PerTurn,
    Warm,
}

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    spawn: Arc<dyn ContainerSpawn>,
    reap_fn: ReapFn,
    /// STABLE per-instance owner token (hash of config-path + mount + agent id), set by the caller.
    owner: String,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,
    inflight: Inflight,
    turn_seq: AtomicU64,
    lifecycle: Lifecycle,
    /// Warm mode only: the authoritative cached container/session per `SessionId` (drained at `retire`).
    warm: Mutex<HashMap<SessionId, WarmInner>>,
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
        Ok(Self {
            cfg,
            spawn,
            reap_fn,
            owner,
            session_cfg: Mutex::new(HashMap::new()),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            turn_seq: AtomicU64::new(0),
            lifecycle: Lifecycle::PerTurn,
            warm: Mutex::new(HashMap::new()),
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
        Self::new_warm_with_hooks(cfg, spawn, owner, production_reap_fn()).await
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
        Self::new_with_hooks(cfg, spawn, owner, production_reap_fn()).await
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

    /// Spawn + configure ONE inner container for `session`. On ANY failure the just-started container is
    /// reaped by name (the `docker run` client can be up before the handshake fails) and `Err` is
    /// returned. The caller owns the cache bookkeeping + the cwd strict-reject. `session/new` is lazy
    /// (inside the inner's first `prompt`), so this method does NOT mint the ACP session. Shared by the
    /// per-turn `prompt` and the warm `prompt_warm` cache-miss path; it touches neither
    /// `inflight`/`warm`/`turn_active`.
    async fn open_inner(
        &self,
        session: &SessionId,
        spec: &SessionSpec,
    ) -> Result<WarmInner, BridgeError> {
        let runtime = self.cfg.sandbox.runtime().to_string();
        let cwd = spec.cwd.clone().ok_or(BridgeError::ConfigInvalid {
            reason: "missing session cwd".into(),
        })?;
        let rw_canon = self.resolve_rw_target(&cwd)?;
        let n = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        // Increment A: the run-id segment defeats same-owner concurrent name clashes; the label set is
        // built PER MINT so `kind` (warm|perturn) is never stale and `repo`/`cwd` reflect this :rw target.
        let name = a2a_name("rw", &self.owner, &self.cfg.run.instance_id, &n.to_string());
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
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
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
        let inner = match self.spawn.spawn(&program, &argv, acp).await {
            Ok(i) => i,
            Err(e) => {
                (self.reap_fn)(runtime.clone(), name.clone()); // spawn-failure reap (never inserted)
                return Err(e);
            }
        };
        let reaped = Arc::new(AtomicBool::new(false));
        // The inner prefers the stashed SessionSpec.cwd over AcpConfig.cwd → configure with CANONICAL cwd.
        let mut spec_canon = spec.clone();
        spec_canon.cwd = Some(rw_canon.clone());
        if let Err(e) = inner.configure_session(session, &spec_canon).await {
            reap_once(&self.reap_fn, &runtime, &name, &reaped);
            return Err(e);
        }
        Ok(WarmInner {
            inner,
            name,
            reaped,
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
    ) -> Result<BackendStream, BridgeError> {
        let spec = self.session_cfg.lock().await.get(session).cloned().ok_or(
            BridgeError::ConfigInvalid {
                reason: "missing session cwd".into(),
            },
        )?;
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
            // `open_inner` already reaps on its own failure; nothing inserted yet.
            let wi = match self.open_inner(session, &spec).await {
                Ok(wi) => wi,
                Err(e) => fail!(e),
            };
            self.warm.lock().await.insert(session.clone(), wi);
            // NO re-configure on cache-miss: open_inner already configured with the canonical cwd.
        } else {
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
                .map(|wi| (wi.inner.clone(), wi.name.clone(), wi.reaped.clone()))
        };
        let (inner, name, reaped) = match got {
            Some(t) => t,
            None => fail!(BridgeError::agent_crashed(
                "warm session retired during prompt"
            )),
        };
        let inner_stream = match inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => {
                if cache_miss {
                    // First-turn failure → reap + remove (no cumulative work to protect).
                    self.warm.lock().await.remove(session);
                    reap_once(&self.reap_fn, self.cfg.sandbox.runtime(), &name, &reaped);
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
        let inner = self
            .warm
            .lock()
            .await
            .get(session)
            .map(|wi| wi.inner.clone());
        if let Some(inner) = inner {
            let _ = inner.cancel(session).await;
        }
        self.turn_active.lock().await.remove(session);
        Ok(())
    }

    /// Warm retire. Drain the cache; per entry: cancel the inner, reap once, and clear any stale
    /// `turn_active` marker (a held/raced stream could leave one behind).
    async fn retire_warm(&self) -> Result<(), BridgeError> {
        let entries: Vec<(SessionId, WarmInner)> = { self.warm.lock().await.drain().collect() };
        let runtime = self.cfg.sandbox.runtime().to_string();
        for (s, wi) in entries {
            let _ = wi.inner.cancel(&s).await;
            reap_once(&self.reap_fn, &runtime, &wi.name, &wi.reaped);
            self.turn_active.lock().await.remove(&s);
        }
        Ok(())
    }

    /// Reap ONE warm session's container (per-session analogue of `retire_warm`).
    async fn release_warm(&self, session: &SessionId) {
        let wi = self.warm.lock().await.remove(session);
        if let Some(wi) = wi {
            let _ = wi.inner.cancel(session).await;
            reap_once(
                &self.reap_fn,
                self.cfg.sandbox.runtime(),
                &wi.name,
                &wi.reaped,
            );
        }
        self.turn_active.lock().await.remove(session);
    }
}

#[async_trait]
impl AgentBackend for ContainerRwBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        if self.is_warm() {
            return self.prompt_warm(session, parts).await;
        }

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

        // Atomic check-and-reserve: reject a second concurrent prompt on a live session under ONE lock.
        {
            let mut m = self.inflight.lock().await;
            if m.contains_key(session) {
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("session {} already has an in-flight turn", session.as_str()),
                });
            }
            m.insert(session.clone(), InflightState::Reserving);
        }
        // From here every error path must remove the reservation (open_inner reaps on its own failure).
        let runtime = self.cfg.sandbox.runtime().to_string();
        let wi = match self.open_inner(session, &spec).await {
            Ok(wi) => wi,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                return Err(e);
            }
        };

        // Promote the reservation to Live (the cancel handle), sharing the `reaped` bool.
        self.inflight.lock().await.insert(
            session.clone(),
            InflightState::Live(InflightTurn {
                inner: wi.inner.clone(),
                name: wi.name.clone(),
                reaped: wi.reaped.clone(),
            }),
        );

        let inner_stream = match wi.inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                reap_once(&self.reap_fn, &runtime, &wi.name, &wi.reaped);
                return Err(e);
            }
        };

        let reaper = ContainerReaper {
            runtime,
            name: wi.name,
            reap_fn: self.reap_fn.clone(),
            reaped: wi.reaped,
            inflight: self.inflight.clone(),
            session: session.clone(),
        };
        Ok(wrap_with_reaper(wi.inner, inner_stream, reaper))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
        if self.is_warm() {
            return self.cancel_warm(session).await;
        }
        let turn = {
            let mut m = self.inflight.lock().await;
            match m.remove(session) {
                Some(InflightState::Live(t)) => Some(t),
                _ => None, // Reserving (mid-spawn) or absent: nothing live to cancel
            }
        };
        if let Some(t) = turn {
            let _ = t.inner.cancel(session).await; // graceful session/cancel first
            reap_once(
                &self.reap_fn,
                self.cfg.sandbox.runtime(),
                &t.name,
                &t.reaped,
            );
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

    /// Stash-only (uniform with the ACP/API backends). Does NOT reap — the stream owns the reaper.
    async fn forget_session(&self, session: &SessionId) {
        self.session_cfg.lock().await.remove(session);
    }

    async fn release_session(&self, session: &SessionId) {
        if self.is_warm() {
            self.release_warm(session).await;
        }
        self.session_cfg.lock().await.remove(session);
    }

    async fn retire(&self) -> Result<(), BridgeError> {
        if self.is_warm() {
            return self.retire_warm().await;
        }
        let turns: Vec<(SessionId, InflightTurn)> = {
            let mut m = self.inflight.lock().await;
            m.drain()
                .filter_map(|(s, st)| match st {
                    InflightState::Live(t) => Some((s, t)),
                    InflightState::Reserving => None,
                })
                .collect()
        };
        for (s, t) in turns {
            let _ = t.inner.cancel(&s).await;
            reap_once(
                &self.reap_fn,
                self.cfg.sandbox.runtime(),
                &t.name,
                &t.reaped,
            );
        }
        Ok(())
    }
}

/// Owned by the returned stream: reaps the container + clears the inflight entry on EVERY exit path
/// (Done / error / consumer-drop). Reap is idempotent + detached — `Drop` never blocks a worker.
struct ContainerReaper {
    runtime: String,
    name: String,
    reap_fn: ReapFn,
    reaped: Arc<AtomicBool>,
    inflight: Inflight,
    session: SessionId,
}
impl ContainerReaper {
    async fn clear_inflight(&self) {
        self.inflight.lock().await.remove(&self.session);
    }
}
impl Drop for ContainerReaper {
    fn drop(&mut self) {
        // Detach the inflight clear (Drop can't await) — covers the early-drop path.
        let inflight = self.inflight.clone();
        let session = self.session.clone();
        spawn_detached(async move {
            inflight.lock().await.remove(&session);
        });
        reap_once(&self.reap_fn, &self.runtime, &self.name, &self.reaped);
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
    use bridge_core::domain::{EffectiveConfig, EgressPolicy, MountAccess};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;

    // ---- stubs -------------------------------------------------------------

    /// Stub inner backend: emits one `Done`, records cancel + prompt count + the sessions it served.
    /// `fail_prompt` (atomic, flippable through `&self`) makes the NEXT `prompt` error — used to drive the
    /// warm reuse-error path.
    struct StubInner {
        canceled: AtomicBool,
        prompts: AtomicUsize,
        sessions: Mutex<HashSet<String>>,
        fail_prompt: AtomicBool,
    }
    #[async_trait]
    impl AgentBackend for StubInner {
        async fn prompt(&self, s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
            self.prompts.fetch_add(1, Ordering::SeqCst);
            self.sessions.lock().await.insert(s.as_str().to_string());
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
            Ok(())
        }
    }

    struct CountingSpawn {
        count: AtomicUsize,
        fail: bool,
        fail_prompt: bool,
        last_argv: Mutex<Vec<String>>,
        last_acp_mcp: Mutex<Vec<bridge_core::mcp::McpServerSpec>>,
        last_inner: Mutex<Option<Arc<StubInner>>>,
    }
    impl CountingSpawn {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                count: AtomicUsize::new(0),
                fail,
                fail_prompt: false,
                last_argv: Mutex::new(vec![]),
                last_acp_mcp: Mutex::new(vec![]),
                last_inner: Mutex::new(None),
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
            if self.fail {
                return Err(BridgeError::agent_crashed("boom"));
            }
            let inner = Arc::new(StubInner {
                canceled: AtomicBool::new(false),
                prompts: AtomicUsize::new(0),
                sessions: Mutex::new(HashSet::new()),
                fail_prompt: AtomicBool::new(self.fail_prompt),
            });
            *self.last_inner.lock().await = Some(inner.clone());
            Ok(inner)
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
    fn spec_cwd(p: &str) -> SessionSpec {
        SessionSpec {
            config: EffectiveConfig::default(),
            cwd: Some(SessionCwd::parse(p).unwrap()),
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
        assert!(be.session_cfg.lock().await.contains_key(&s));
        be.forget_session(&s).await;
        assert!(!be.session_cfg.lock().await.contains_key(&s));
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
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("boom"), "got {err:?}");
        assert_eq!(reaps.load(Ordering::SeqCst), 1, "spawn failure MUST reap");
        assert!(be.inflight.lock().await.is_empty(), "reservation removed");
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
        assert!(reaps.load(Ordering::SeqCst) >= 1, "retire reaps");
    }

    #[test]
    fn off_runtime_reaper_drop_does_not_panic() {
        // Drop firing OUTSIDE a tokio runtime must not panic (process-shutdown path).
        let (reap, reaps) = counting_reap();
        let inflight: Inflight = Arc::new(Mutex::new(HashMap::new()));
        let reaper = ContainerReaper {
            runtime: "docker".into(),
            name: "a2a-rw-inst-0".into(),
            reap_fn: reap,
            reaped: Arc::new(AtomicBool::new(false)),
            inflight,
            session: SessionId::parse("s1").unwrap(),
        };
        drop(reaper); // no runtime in scope → spawn_detached uses the thread fallback
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
        assert!(be.warm.lock().await.get(&s).is_none(), "warm entry removed");
        assert_eq!(
            reaps.load(Ordering::SeqCst),
            1,
            "exactly one container reaped"
        );
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
        assert_eq!(reaps.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn warm_edit_turn_open_failure_reaps_and_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_str().unwrap();
        let (reap, reaps) = counting_reap();
        let be = warm_backend(root, CountingSpawn::new(true), reap).await; // spawn fails (cache-miss open)
        let s = SessionId::parse("implement-x").unwrap();
        be.configure_session(&s, &spec_cwd(root)).await.unwrap();
        let err = prompt_err(&be, &s).await;
        assert!(format!("{err:?}").contains("boom"), "got {err:?}");
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
