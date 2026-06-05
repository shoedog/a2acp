//! Per-turn write-capable containerized ACP agent (Slice B2a). [`ContainerRwBackend`] spawns a fresh
//! `:rw` container per `prompt` turn (composing [`bridge_acp::acp_backend::AcpBackend`] via the
//! [`ContainerSpawn`] seam) and reaps it on every terminal path. Warm reuse across turns is a separate
//! future slice (see `docs/superpowers/specs/2026-06-05-containerized-agents-warm-pool-slice.md`).

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bridge_acp::acp_backend::AcpConfig;
use bridge_core::domain::{Part, SandboxConfig, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::SessionId;
use bridge_core::ports::{AgentBackend, BackendStream};
use bridge_core::sandbox::{check_rw_target, compose_container_rw, reap_argv};
use bridge_core::session_cwd::SessionCwd;
use futures::StreamExt;
use tokio::sync::Mutex;

/// Fire-and-forget reap of a named container: `(runtime, name)`. Production detaches a `docker rm -f`;
/// tests inject a counter. NEVER blocks the caller.
pub type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;

/// Boot-time orphan sweep over a `name=<prefix>` filter. Production runs `<runtime> ps -aq …` → `rm -f`;
/// tests inject a recorder.
pub type SweepFn = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

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

/// Static config for a `ContainerRw` agent (cheap, no Docker at construction beyond the boot sweep).
pub struct ContainerRwConfig {
    pub sandbox: SandboxConfig,
    /// The inner ACP CLI (e.g. `claude-agent-acp`) — runs contained.
    pub cmd: String,
    pub args: Vec<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub auth_method: Option<String>,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
}

/// A live per-turn container handle, kept so `cancel` can reach the inner. Its `reaped` is SHARED with
/// the stream-owned [`ContainerReaper`] so cancel + stream-drop can't double-reap.
struct InflightTurn {
    inner: Arc<dyn AgentBackend>,
    name: String,
    reaped: Arc<AtomicBool>,
}

/// One entry per session: `Reserving` is held across the (async) spawn so a concurrent second prompt is
/// rejected atomically (no check-then-insert race); `Live` carries the cancel handle.
enum InflightState {
    Reserving,
    Live(InflightTurn),
}

type Inflight = Arc<Mutex<HashMap<SessionId, InflightState>>>;

pub struct ContainerRwBackend {
    cfg: ContainerRwConfig,
    spawn: Arc<dyn ContainerSpawn>,
    reap_fn: ReapFn,
    /// STABLE per-instance owner token (hash of config-path + mount + agent id), set by the caller.
    owner: String,
    session_cfg: Mutex<HashMap<SessionId, SessionSpec>>,
    inflight: Inflight,
    turn_seq: AtomicU64,
}

impl ContainerRwBackend {
    /// Hook-injectable constructor (the ONE constructor — tests inject `reap_fn`/`sweep_fn`). AWAITS the
    /// boot-orphan sweep BEFORE returning: the sweep is scoped by the stable `owner`, and because
    /// `turn_seq` restarts at 0 a surviving orphan would collide with the first mint's `--name`, so the
    /// sweep MUST complete before any mint (blocking-at-construction invariant).
    pub async fn new_with_hooks(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
        reap_fn: ReapFn,
        sweep_fn: SweepFn,
    ) -> Result<Self, BridgeError> {
        sweep_fn(format!("a2a-rw-{owner}-")).await;
        Ok(Self {
            cfg,
            spawn,
            reap_fn,
            owner,
            session_cfg: Mutex::new(HashMap::new()),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            turn_seq: AtomicU64::new(0),
        })
    }

    /// Production constructor: detached `docker rm -f` reaper + a runtime-parametric `docker ps`/`rm`
    /// boot sweep, both keyed on the configured runtime (docker|podman).
    pub async fn new(
        cfg: ContainerRwConfig,
        spawn: Arc<dyn ContainerSpawn>,
        owner: String,
    ) -> Result<Self, BridgeError> {
        let runtime = cfg.sandbox.runtime().to_string();
        Self::new_with_hooks(
            cfg,
            spawn,
            owner,
            production_reap_fn(),
            production_sweep_fn(runtime),
        )
        .await
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
}

#[async_trait]
impl AgentBackend for ContainerRwBackend {
    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        // Strict-reject: a writer MUST name its :rw target (no fallback to the broad root).
        let spec = self.session_cfg.lock().await.get(session).cloned().ok_or(
            BridgeError::ConfigInvalid {
                reason: "missing session cwd".into(),
            },
        )?;
        let cwd = spec.cwd.clone().ok_or(BridgeError::ConfigInvalid {
            reason: "missing session cwd".into(),
        })?;

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
        // From here, EVERY error path must remove the reservation (and reap if a container started).
        let runtime = self.cfg.sandbox.runtime().to_string();
        let rw_canon = match self.resolve_rw_target(&cwd) {
            Ok(c) => c,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                return Err(e);
            }
        };
        let n = self.turn_seq.fetch_add(1, Ordering::Relaxed);
        let name = format!("a2a-rw-{}-{}", self.owner, n);
        let (program, argv) = compose_container_rw(
            &self.cfg.sandbox,
            &rw_canon,
            &name,
            &self.cfg.cmd,
            &self.cfg.args,
        );
        let acp = AcpConfig {
            cwd: PathBuf::from(rw_canon.as_str()),
            model: self.cfg.model.clone(),
            mode: self.cfg.mode.clone(),
            auth_method: self.cfg.auth_method.clone(),
            handshake_timeout: self.cfg.handshake_timeout,
            cancel_grace: self.cfg.cancel_grace,
        };

        // Spawn — the `docker run` client may be up before the handshake fails → reap by name on error.
        let inner = match self.spawn.spawn(&program, &argv, acp).await {
            Ok(i) => i,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                (self.reap_fn)(runtime.clone(), name.clone()); // spawn-failure reap
                return Err(e);
            }
        };
        let reaped = Arc::new(AtomicBool::new(false));

        // Forward the CANONICAL cwd to the inner: AcpBackend prefers the stashed SessionSpec.cwd over
        // AcpConfig.cwd, so the ACP session cwd must equal the mounted path.
        let mut spec_canon = spec.clone();
        spec_canon.cwd = Some(rw_canon.clone());
        if let Err(e) = inner.configure_session(session, &spec_canon).await {
            self.inflight.lock().await.remove(session);
            reap_once(&self.reap_fn, &runtime, &name, &reaped);
            return Err(e);
        }

        // Promote the reservation to Live (the cancel handle), sharing the `reaped` bool.
        self.inflight.lock().await.insert(
            session.clone(),
            InflightState::Live(InflightTurn {
                inner: inner.clone(),
                name: name.clone(),
                reaped: reaped.clone(),
            }),
        );

        let inner_stream = match inner.prompt(session, parts).await {
            Ok(s) => s,
            Err(e) => {
                self.inflight.lock().await.remove(session);
                reap_once(&self.reap_fn, &runtime, &name, &reaped);
                return Err(e);
            }
        };

        let reaper = ContainerReaper {
            runtime,
            name,
            reap_fn: self.reap_fn.clone(),
            reaped,
            inflight: self.inflight.clone(),
            session: session.clone(),
        };
        Ok(wrap_with_reaper(inner, inner_stream, reaper))
    }

    async fn cancel(&self, session: &SessionId) -> Result<(), BridgeError> {
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

    async fn retire(&self) -> Result<(), BridgeError> {
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

/// Reap a named container exactly once (idempotent via the shared `reaped` flag).
fn reap_once(reap_fn: &ReapFn, runtime: &str, name: &str, reaped: &Arc<AtomicBool>) {
    if !reaped.swap(true, Ordering::SeqCst) {
        reap_fn(runtime.to_string(), name.to_string());
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

/// Spawn a future onto the current runtime if there is one, else on a throwaway thread+runtime. `Drop`
/// can fire off-runtime (process shutdown), so this must never panic.
fn spawn_detached<F: Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(h) => {
            h.spawn(fut);
        }
        Err(_) => {
            std::thread::spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(fut);
                }
            });
        }
    }
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

fn production_reap_fn() -> ReapFn {
    Arc::new(|runtime: String, name: String| {
        let (prog, argv) = reap_argv(&runtime, &name);
        spawn_detached(async move {
            // Best-effort; `rm -f` of a gone container is harmless. Bounded so a hung daemon can't pile up.
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::process::Command::new(&prog).args(&argv).output(),
            )
            .await;
        });
    })
}

fn production_sweep_fn(runtime: String) -> SweepFn {
    Arc::new(move |filter: String| {
        let runtime = runtime.clone();
        Box::pin(async move {
            let ps = tokio::process::Command::new(&runtime)
                .args(["ps", "-aq", "--filter", &format!("name={filter}")])
                .output()
                .await;
            let Ok(ps) = ps else {
                tracing::warn!(runtime = %runtime, "container_rw boot sweep: runtime unavailable");
                return;
            };
            let ids = String::from_utf8_lossy(&ps.stdout);
            for id in ids.split_whitespace() {
                let _ = tokio::process::Command::new(&runtime)
                    .args(["rm", "-f", id])
                    .output()
                    .await;
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, EgressPolicy, MountAccess};
    use std::sync::atomic::AtomicUsize;

    // ---- stubs -------------------------------------------------------------

    /// Stub inner backend: emits one `Done`, records cancel.
    struct StubInner {
        canceled: AtomicBool,
    }
    #[async_trait]
    impl AgentBackend for StubInner {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
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
        last_argv: Mutex<Vec<String>>,
        last_inner: Mutex<Option<Arc<StubInner>>>,
    }
    impl CountingSpawn {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                count: AtomicUsize::new(0),
                fail,
                last_argv: Mutex::new(vec![]),
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
            _cfg: AcpConfig,
        ) -> Result<Arc<dyn AgentBackend>, BridgeError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            *self.last_argv.lock().await = argv.to_vec();
            if self.fail {
                return Err(BridgeError::agent_crashed("boom"));
            }
            let inner = Arc::new(StubInner {
                canceled: AtomicBool::new(false),
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
    fn noop_sweep() -> SweepFn {
        Arc::new(|_filter| Box::pin(async {}))
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
            model: None,
            mode: None,
            auth_method: None,
            handshake_timeout: Duration::from_secs(30),
            cancel_grace: Duration::from_secs(5),
        }
    }

    async fn backend(
        mount: &str,
        spawn: Arc<dyn ContainerSpawn>,
        reap: ReapFn,
    ) -> ContainerRwBackend {
        ContainerRwBackend::new_with_hooks(
            cfg_with_mount(mount),
            spawn,
            "inst".into(),
            reap,
            noop_sweep(),
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

    #[tokio::test]
    async fn boot_sweep_runs_at_construction_with_owner_filter() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let sweep: SweepFn = Arc::new(move |filter| {
            let seen2 = seen2.clone();
            Box::pin(async move {
                seen2.lock().await.push(filter);
            })
        });
        let _be = ContainerRwBackend::new_with_hooks(
            cfg_with_mount("/root"),
            CountingSpawn::new(false),
            "inst42".into(),
            counting_reap().0,
            sweep,
        )
        .await
        .unwrap();
        assert_eq!(
            seen.lock().await.clone(),
            vec!["a2a-rw-inst42-".to_string()]
        );
    }
}
