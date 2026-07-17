//! Shared container-reaping primitives (used by the :rw ContainerRwBackend and the :ro AcpBackend path).
//! Detached + idempotent so a `Drop` (which may fire off-runtime at process shutdown) never blocks/panics.
use futures::FutureExt;
use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

const CONTAINER_START_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const CONTAINER_START_STATUS_MAX_BYTES: u64 = 64;

/// `(runtime, name) -> fire-and-forget reap`. Injectable so tests don't spawn Docker.
pub type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;

/// One bounded removal attempt. The result is metadata-only and safe to share
/// with operation-owned teardown diagnostics.
pub type ReapAttemptFn = Arc<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<(), ReapFailure>> + Send>>
        + Send
        + Sync,
>;

/// Runtime-observed state of one exact named container. `NotStarted` is deliberately narrower than
/// generic runtime unavailability: it is returned only when the runtime itself says the object remains
/// in a pre-start state. Callers must preserve their existing diagnosis for `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerStartState {
    NotStarted,
    Started,
    Unknown,
}

/// One bounded exact-name state observation. Injectable so ACP lifecycle tests never invoke Docker.
pub type ContainerStartProbeFn = Arc<
    dyn Fn(String, String) -> Pin<Box<dyn Future<Output = ContainerStartState> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReapFailure {
    Spawn,
    Timeout,
    NonZeroExit,
    WorkerPanicked,
}

impl ReapFailure {
    pub fn code(self) -> &'static str {
        match self {
            Self::Spawn => "container.reap.spawn_failed",
            Self::Timeout => "container.reap.timeout",
            Self::NonZeroExit => "container.reap.nonzero_exit",
            Self::WorkerPanicked => "container.reap.worker_panicked",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReapState {
    NotStarted,
    Running,
    Settled(Result<(), ReapFailure>),
}

struct ReapShared {
    state: StdMutex<ReapState>,
    settled: tokio::sync::Notify,
}

/// Shared, cancellation-safe, joinable ownership for one named container reap.
/// The worker never owns an operation observer; observed callers only await and
/// locally report the shared metadata-only result.
#[derive(Clone)]
pub struct ReapController {
    runtime: String,
    name: String,
    attempt: ReapAttemptFn,
    start_probe: Option<ContainerStartProbeFn>,
    shared: Arc<ReapShared>,
}

impl std::fmt::Debug for ReapController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReapController")
            .field("runtime", &self.runtime)
            .field("name", &self.name)
            .field("result", &self.result())
            .finish_non_exhaustive()
    }
}

impl ReapController {
    pub fn new(
        runtime: impl Into<String>,
        name: impl Into<String>,
        attempt: ReapAttemptFn,
    ) -> Self {
        Self {
            runtime: runtime.into(),
            name: name.into(),
            attempt,
            start_probe: None,
            shared: Arc::new(ReapShared {
                state: StdMutex::new(ReapState::NotStarted),
                settled: tokio::sync::Notify::new(),
            }),
        }
    }

    /// Source-compatible adapter for existing injectable fire-and-forget tests
    /// and constructors. Invocation completion is the only result that legacy
    /// closures can expose, so it settles successfully after the call returns.
    pub fn from_legacy(
        runtime: impl Into<String>,
        name: impl Into<String>,
        reap_fn: ReapFn,
    ) -> Self {
        let attempt: ReapAttemptFn = Arc::new(move |runtime, name| {
            let reap_fn = Arc::clone(&reap_fn);
            Box::pin(async move {
                reap_fn(runtime, name);
                Ok(())
            })
        });
        Self::new(runtime, name, attempt)
    }

    pub fn production(runtime: impl Into<String>, name: impl Into<String>) -> Self {
        Self::production_with_timeout(runtime, name, Duration::from_secs(10))
    }

    fn production_with_timeout(
        runtime: impl Into<String>,
        name: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        let attempt: ReapAttemptFn = Arc::new(move |runtime, name| {
            let timeout = timeout;
            Box::pin(async move {
                let (program, argv) = crate::sandbox::reap_argv(&runtime, &name);
                let mut command = tokio::process::Command::new(&program);
                command.args(&argv).kill_on_drop(true);
                let child = command.spawn().map_err(|_| ReapFailure::Spawn)?;
                let output = tokio::time::timeout(timeout, child.wait_with_output())
                    .await
                    .map_err(|_| ReapFailure::Timeout)?
                    .map_err(|_| ReapFailure::Spawn)?;
                if output.status.success() {
                    Ok(())
                } else {
                    Err(ReapFailure::NonZeroExit)
                }
            })
        });
        Self::new(runtime, name, attempt)
            .with_start_probe(production_start_probe(CONTAINER_START_PROBE_TIMEOUT))
    }

    /// Attach the bridge-owned exact-name start observer. Legacy controllers intentionally omit this
    /// active runtime seam and retain their historical handshake behavior.
    #[doc(hidden)]
    pub fn with_start_probe(mut self, start_probe: ContainerStartProbeFn) -> Self {
        self.start_probe = Some(start_probe);
        self
    }

    #[doc(hidden)]
    pub fn has_start_probe(&self) -> bool {
        self.start_probe.is_some()
    }

    /// Observe the exact named container once. A panicking injected observer fails closed to `Unknown`;
    /// only positive runtime evidence can become `NotStarted` or `Started`.
    #[doc(hidden)]
    pub async fn probe_start_state(&self) -> ContainerStartState {
        let Some(probe) = &self.start_probe else {
            return ContainerStartState::Unknown;
        };
        let runtime = self.runtime.clone();
        let name = self.name.clone();
        let future =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe(runtime, name))) {
                Ok(future) => future,
                Err(_) => return ContainerStartState::Unknown,
            };
        match std::panic::AssertUnwindSafe(future).catch_unwind().await {
            Ok(state) => state,
            Err(_) => ContainerStartState::Unknown,
        }
    }

    fn ensure_started(&self) {
        let should_start = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *state == ReapState::NotStarted {
                *state = ReapState::Running;
                true
            } else {
                false
            }
        };
        if !should_start {
            return;
        }

        let runtime = self.runtime.clone();
        let name = self.name.clone();
        let attempt = Arc::clone(&self.attempt);
        let shared = Arc::clone(&self.shared);
        spawn_detached(async move {
            let attempt_future =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| attempt(runtime, name)));
            let result = match attempt_future {
                Ok(future) => match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                    Ok(result) => result,
                    Err(_) => Err(ReapFailure::WorkerPanicked),
                },
                Err(_) => Err(ReapFailure::WorkerPanicked),
            };
            *shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = ReapState::Settled(result);
            shared.settled.notify_waiters();
        });
    }

    /// Start or join the single removal attempt and return its stable result.
    pub async fn reap_observed(&self) -> Result<(), ReapFailure> {
        self.ensure_started();
        loop {
            // Register before sampling state so settlement cannot be lost between
            // the sample and the await.
            let notified = self.shared.settled.notified();
            let state = *self
                .shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match state {
                ReapState::Settled(result) => return result,
                ReapState::NotStarted => self.ensure_started(),
                ReapState::Running => notified.await,
            }
        }
    }

    /// Start the same single attempt without retaining any operation observer.
    pub fn reap_detached(&self) {
        self.ensure_started();
    }

    pub fn result(&self) -> Option<Result<(), ReapFailure>> {
        match *self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            ReapState::Settled(result) => Some(result),
            ReapState::NotStarted | ReapState::Running => None,
        }
    }
}

fn classify_container_start_status(status: &[u8]) -> ContainerStartState {
    let Ok(status) = std::str::from_utf8(status) else {
        return ContainerStartState::Unknown;
    };
    match status.trim() {
        // Docker reports `created`; Podman may expose the adjacent `configured`/`initialized` states.
        "created" | "configured" | "initialized" => ContainerStartState::NotStarted,
        "running" | "restarting" | "paused" | "exited" | "stopped" | "dead" | "removing"
        | "stopping" => ContainerStartState::Started,
        _ => ContainerStartState::Unknown,
    }
}

fn production_start_probe(timeout: Duration) -> ContainerStartProbeFn {
    Arc::new(move |runtime, name| {
        Box::pin(async move {
            let mut command = tokio::process::Command::new(&runtime);
            command
                .args([
                    "container",
                    "inspect",
                    "--format",
                    "{{.State.Status}}",
                    &name,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(_) => return ContainerStartState::Unknown,
            };
            let Some(stdout) = child.stdout.take() else {
                return ContainerStartState::Unknown;
            };
            let observation = async move {
                let mut bytes = Vec::new();
                stdout
                    .take(CONTAINER_START_STATUS_MAX_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ())?;
                let status = child.wait().await.map_err(|_| ())?;
                if !status.success()
                    || u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                        > CONTAINER_START_STATUS_MAX_BYTES
                {
                    return Ok(ContainerStartState::Unknown);
                }
                Ok(classify_container_start_status(&bytes))
            };
            match tokio::time::timeout(timeout, observation).await {
                Ok(Ok(state)) => state,
                Ok(Err(())) | Err(_) => ContainerStartState::Unknown,
            }
        })
    })
}

/// Reap a named container exactly once (idempotent via the shared `reaped` flag).
pub fn reap_once(reap_fn: &ReapFn, runtime: &str, name: &str, reaped: &Arc<AtomicBool>) {
    if !reaped.swap(true, Ordering::SeqCst) {
        reap_fn(runtime.to_string(), name.to_string());
    }
}

/// Spawn a future onto the current runtime if there is one, else on a throwaway thread+runtime. `Drop`
/// can fire off-runtime (process shutdown), so this must never panic.
pub fn spawn_detached<F: Future<Output = ()> + Send + 'static>(fut: F) {
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

/// Production reaper: detached `<runtime> rm -f <name>` (reap_argv) with a 10s timeout.
pub fn production_reap_fn() -> ReapFn {
    Arc::new(|runtime: String, name: String| {
        let (prog, argv) = crate::sandbox::reap_argv(&runtime, &name);
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

// ---- Increment A: label-scoped reaping + liveness sweep + staleness (shell out; live-gated) -------------

/// Reap THIS run's containers (END-sweep): `ps -aq --filter label=a2a.run=<id>` → `rm -f` each. Best-effort.
pub fn run_scoped_reap(runtime: &str, run_id: &str) {
    let (p, argv) = crate::sandbox::by_run_filter_argv(runtime, run_id);
    if let Ok(out) = std::process::Command::new(&p).args(&argv).output() {
        for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let _ = std::process::Command::new(runtime)
                .args(["rm", "-f", id])
                .output();
        }
    }
}

/// PURE recovery planner: classify a batch of inspected managed records `(id, host, lease)` against the
/// CURRENT lease state and return `(reap_ids, dead_leases)` — the container ids to `rm -f` (DEAD only:
/// same host + a FREE lease lock) and the DISTINCT dead lease files (order-preserving, deduped).
///
/// Classifying the WHOLE batch BEFORE any lease file is removed is the correctness keystone: a crashed run's
/// containers span MULTIPLE owners (e.g. a `:rw` implementor + per-reviewer `:ro` readers) but share ONE
/// lease file. If a sibling reap deleted that lease mid-recovery, every later owner's sweep would probe an
/// ABSENT lease → [`crate::run_identity::classify`] returns `Unknown` → spared → the orphan LEAKS (live-gate
/// finding). So lease DELETION is the caller's job, performed ONCE after EVERY owner has been swept.
pub fn plan_recovery(
    records: &[(String, String, String)],
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> (Vec<String>, Vec<String>) {
    use crate::run_identity::{classify, Verdict};
    let mut reap_ids = Vec::new();
    let mut dead_leases: Vec<String> = Vec::new();
    for (id, host, lease) in records {
        let labels = std::collections::HashMap::from([
            ("a2a.host".to_string(), host.clone()),
            ("a2a.lease".to_string(), lease.clone()),
        ]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            reap_ids.push(id.clone());
            if !lease.is_empty() && !dead_leases.contains(lease) {
                dead_leases.push(lease.clone());
            }
        }
    }
    (reap_ids, dead_leases)
}

/// Owner-scoped crash-recovery: inspect each MANAGED container in `owner`, [`plan_recovery`] the batch, and
/// reap ONLY `Dead` (same host + free lease lock). RETURNS the dead lease files (deduped) for the caller to
/// delete ONCE, after EVERY owner has been swept — NOT here: a crashed run's containers span multiple owners
/// but share one lease, so deleting it per-owner would blind the later owners' sweeps (see [`plan_recovery`]).
/// Never touches Alive/Unknown. Best-effort (any docker error ⇒ no reaps, no dead leases).
#[must_use]
pub fn classify_sweep(
    runtime: &str,
    owner: &str,
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) -> Vec<String> {
    let (p, argv) = crate::sandbox::managed_inspect_argv(runtime, owner);
    let Ok(out) = std::process::Command::new(&p).args(&argv).output() else {
        return Vec::new();
    };
    let records: Vec<(String, String, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            match (it.next(), it.next(), it.next()) {
                (Some(id), Some(host), Some(lease)) => {
                    Some((id.to_string(), host.to_string(), lease.to_string()))
                }
                _ => None,
            }
        })
        .collect();
    let (reap_ids, dead_leases) = plan_recovery(&records, my_host, probe);
    for id in reap_ids {
        let _ = std::process::Command::new(runtime)
            .args(["rm", "-f", &id])
            .output();
    }
    dead_leases
}

/// True iff the container produced NO log line within `window` (⇒ stale). `docker logs --since <window>
/// --tail 1 <name>` empty ⇒ stale. Best-effort: any error ⇒ false (bias against a false-stale flag).
pub fn is_stale(runtime: &str, name: &str, window: &str) -> bool {
    match std::process::Command::new(runtime)
        .args(["logs", "--since", window, "--tail", "1", name])
        .output()
    {
        Ok(o) => o.stdout.is_empty() && o.stderr.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn reap_once_fires_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let reap_fn: ReapFn = Arc::new(move |_r, _n| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        let reaped = Arc::new(AtomicBool::new(false));
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped);
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped); // 2nd call no-ops
        reap_once(&reap_fn, "docker", "a2a-ro-x", &reaped);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spawn_detached_off_runtime_does_not_panic() {
        // Called from a plain thread (no tokio runtime) — the Drop-at-shutdown case.
        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);
        std::thread::spawn(move || {
            spawn_detached(async move {
                d.store(true, Ordering::SeqCst);
            });
        })
        .join()
        .unwrap();
        // No panic = pass (the detached work runs on its own thread; we don't join it).
    }

    #[test]
    fn container_start_status_classification_is_closed() {
        for status in [b"created".as_slice(), b"configured", b"initialized"] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::NotStarted
            );
        }
        for status in [
            b"running".as_slice(),
            b"restarting",
            b"paused",
            b"exited",
            b"stopped",
            b"dead",
            b"removing",
            b"stopping",
        ] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::Started
            );
        }
        for status in [
            b"".as_slice(),
            b"unknown",
            b"CREATED",
            b"created extra",
            &[0xff],
        ] {
            assert_eq!(
                classify_container_start_status(status),
                ContainerStartState::Unknown
            );
        }
    }

    #[tokio::test]
    async fn panicking_start_probes_fail_closed_to_unknown() {
        let attempt: ReapAttemptFn = Arc::new(|_runtime, _name| Box::pin(async move { Ok(()) }));
        let synchronous: ContainerStartProbeFn =
            Arc::new(|_runtime, _name| panic!("synchronous start probe panic"));
        let controller = ReapController::new("docker", "a2a-ro-sync-panic", Arc::clone(&attempt))
            .with_start_probe(synchronous);
        assert_eq!(
            controller.probe_start_state().await,
            ContainerStartState::Unknown
        );

        let asynchronous: ContainerStartProbeFn = Arc::new(|_runtime, _name| {
            Box::pin(async move { panic!("asynchronous start probe panic") })
        });
        let controller = ReapController::new("docker", "a2a-ro-async-panic", attempt)
            .with_start_probe(asynchronous);
        assert_eq!(
            controller.probe_start_state().await,
            ContainerStartState::Unknown
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_start_probe_is_bounded_and_requires_exact_status() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        std::fs::write(
            &runtime,
            "#!/bin/sh\ncase \"$5\" in\n  created) printf created ;;\n  running) printf running ;;\n  oversized) printf '%065d' 0 ;;\n  *) exit 1 ;;\nesac\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        // Match the production bound for ordinary exact-status observations. The separate hung-runtime
        // control below keeps its deliberately short timeout and proves cancellation independently.
        let probe = production_start_probe(CONTAINER_START_PROBE_TIMEOUT);
        let runtime = runtime.to_string_lossy().into_owned();

        assert_eq!(
            probe(runtime.clone(), "created".into()).await,
            ContainerStartState::NotStarted
        );
        assert_eq!(
            probe(runtime.clone(), "running".into()).await,
            ContainerStartState::Started
        );
        assert_eq!(
            probe(runtime.clone(), "oversized".into()).await,
            ContainerStartState::Unknown
        );
        assert_eq!(
            probe(runtime, "missing".into()).await,
            ContainerStartState::Unknown
        );

        let hung_runtime = temp.path().join("hung-runtime");
        let marker = temp.path().join("late-side-effect");
        std::fs::write(
            &hung_runtime,
            "#!/bin/sh\nsleep 0.25\nprintf reached > \"$5\"\nprintf running\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&hung_runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hung_runtime, permissions).unwrap();
        let probe = production_start_probe(Duration::from_millis(20));
        assert_eq!(
            probe(
                hung_runtime.to_string_lossy().into_owned(),
                marker.to_string_lossy().into_owned(),
            )
            .await,
            ContainerStartState::Unknown
        );
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !marker.exists(),
            "a timed-out runtime probe must not reach its delayed side effect"
        );
    }

    #[tokio::test]
    async fn joinable_reaper_runs_once_for_concurrent_waiters() {
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
        let controller = ReapController::new("docker", "a2a-rw-x", attempt);
        let first = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.reap_observed().await })
        };
        entered.notified().await;
        let second = {
            let controller = controller.clone();
            tokio::spawn(async move { controller.reap_observed().await })
        };
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        assert_eq!(first.await.unwrap(), Ok(()));
        assert_eq!(second.await.unwrap(), Ok(()));
        assert_eq!(controller.reap_observed().await, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn joinable_reaper_returns_same_typed_failure_to_every_waiter() {
        for failure in [
            ReapFailure::Spawn,
            ReapFailure::Timeout,
            ReapFailure::NonZeroExit,
        ] {
            let attempt: ReapAttemptFn =
                Arc::new(move |_runtime, _name| Box::pin(async move { Err(failure) }));
            let controller = ReapController::new("docker", "a2a-rw-x", attempt);
            assert_eq!(controller.reap_observed().await, Err(failure));
            assert_eq!(controller.reap_observed().await, Err(failure));
            assert_eq!(controller.result(), Some(Err(failure)));
            assert_eq!(
                failure.code(),
                match failure {
                    ReapFailure::Spawn => "container.reap.spawn_failed",
                    ReapFailure::Timeout => "container.reap.timeout",
                    ReapFailure::NonZeroExit => "container.reap.nonzero_exit",
                    ReapFailure::WorkerPanicked => unreachable!(),
                }
            );
        }
    }

    #[tokio::test]
    async fn synchronous_attempt_panic_settles_once_as_worker_panicked() {
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                calls.fetch_add(1, Ordering::SeqCst);
                panic!("synchronous reaper panic")
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-sync-panic", attempt);

        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn asynchronous_attempt_panic_settles_once_as_worker_panicked() {
        let calls = Arc::new(AtomicUsize::new(0));
        let attempt: ReapAttemptFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                let calls = Arc::clone(&calls);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    panic!("asynchronous reaper panic")
                })
            })
        };
        let controller = ReapController::new("docker", "a2a-rw-async-panic", attempt);

        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(
            controller.reap_observed().await,
            Err(ReapFailure::WorkerPanicked)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn production_timeout_kills_child_before_delayed_side_effect() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("hung-runtime");
        let marker = temp.path().join("late-side-effect");
        std::fs::write(&runtime, "#!/bin/sh\nsleep 0.25\nprintf reached > \"$3\"\n").unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();

        let controller = ReapController::production_with_timeout(
            runtime.to_string_lossy(),
            marker.to_string_lossy(),
            Duration::from_millis(20),
        );
        assert_eq!(controller.reap_observed().await, Err(ReapFailure::Timeout));
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !marker.exists(),
            "kill_on_drop must stop the timed-out runtime before its delayed side effect"
        );
    }

    #[tokio::test]
    async fn detached_reap_starts_the_same_joinable_attempt() {
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
        let controller = ReapController::new("docker", "a2a-rw-x", attempt);
        controller.reap_detached();
        tokio::time::timeout(Duration::from_secs(1), controller.reap_observed())
            .await
            .expect("detached attempt settles")
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn legacy_reap_fn_adapter_is_joinable_and_exactly_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reap_fn: ReapFn = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_runtime, _name| {
                calls.fetch_add(1, Ordering::SeqCst);
            })
        };
        let controller = ReapController::from_legacy("docker", "a2a-rw-x", reap_fn);
        controller.reap_observed().await.unwrap();
        controller.reap_detached();
        controller.reap_observed().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ---- plan_recovery (pure crash-recovery planner) ---------------------------------------------------
    use crate::liveness::LeaseProbe;

    /// Map `lease_path -> Some(true)=free/dead | Some(false)=held/alive | None=absent`.
    struct MapProbe(std::collections::HashMap<String, Option<bool>>);
    impl LeaseProbe for MapProbe {
        fn try_state(&self, lease_path: &str) -> Option<bool> {
            self.0.get(lease_path).copied().flatten()
        }
    }
    fn rec(id: &str, host: &str, lease: &str) -> (String, String, String) {
        (id.to_string(), host.to_string(), lease.to_string())
    }

    #[test]
    fn plan_recovery_reaps_only_dead_same_host() {
        let probe = MapProbe(std::collections::HashMap::from([
            ("/l/dead.lock".to_string(), Some(true)),   // free ⇒ dead
            ("/l/alive.lock".to_string(), Some(false)), // held ⇒ alive
            ("/l/gone.lock".to_string(), None),         // absent ⇒ unknown
        ]));
        let records = vec![
            rec("c_dead", "h1", "/l/dead.lock"),
            rec("c_alive", "h1", "/l/alive.lock"),
            rec("c_absent", "h1", "/l/gone.lock"),
            rec("c_otherhost", "h2", "/l/dead.lock"), // free lock but DIFFERENT host ⇒ spared
        ];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert_eq!(reap, vec!["c_dead".to_string()]);
        assert_eq!(leases, vec!["/l/dead.lock".to_string()]);
    }

    #[test]
    fn plan_recovery_shared_lease_across_owners_reaps_all_and_dedups_lease() {
        // The live-gate keystone: a crashed run's :rw + :ro containers (distinct ids, DIFFERENT owners) share
        // ONE free lease. Classified as a single batch BEFORE any deletion ⇒ ALL reaped, the lease returned
        // EXACTLY ONCE for the caller to delete after every owner is swept (no mid-pass blinding).
        let probe = MapProbe(std::collections::HashMap::from([(
            "/l/run.lock".to_string(),
            Some(true),
        )]));
        let records = vec![
            rec("c_rw", "h1", "/l/run.lock"),
            rec("c_ro_codex", "h1", "/l/run.lock"),
            rec("c_ro_claude", "h1", "/l/run.lock"),
        ];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert_eq!(
            reap,
            vec![
                "c_rw".to_string(),
                "c_ro_codex".to_string(),
                "c_ro_claude".to_string()
            ]
        );
        assert_eq!(leases, vec!["/l/run.lock".to_string()]); // deduped to one
    }

    #[test]
    fn plan_recovery_blank_lease_label_is_spared() {
        // A blank a2a.lease label probes to None ⇒ classify spares (Unknown): never reaped, no dead lease.
        let probe = MapProbe(std::collections::HashMap::new());
        let records = vec![rec("c", "h1", "")];
        let (reap, leases) = plan_recovery(&records, "h1", &probe);
        assert!(reap.is_empty());
        assert!(leases.is_empty());
    }
}
