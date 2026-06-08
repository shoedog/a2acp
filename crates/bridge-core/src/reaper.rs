//! Shared container-reaping primitives (used by the :rw ContainerRwBackend and the :ro AcpBackend path).
//! Detached + idempotent so a `Drop` (which may fire off-runtime at process shutdown) never blocks/panics.
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// `(runtime, name) -> fire-and-forget reap`. Injectable so tests don't spawn Docker.
pub type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;

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

/// Owner-scoped crash-recovery: inspect each MANAGED container in `owner`, [`crate::run_identity::classify`]
/// it, and reap ONLY `Dead` (same host + free lease lock). After reaping, remove each dead run's lease file
/// (deduped — a run's containers share one lease). Never touches Alive/Unknown. Best-effort.
pub fn classify_sweep(
    runtime: &str,
    owner: &str,
    my_host: &str,
    probe: &dyn crate::liveness::LeaseProbe,
) {
    use crate::run_identity::{classify, Verdict};
    let (p, argv) = crate::sandbox::managed_inspect_argv(runtime, owner);
    let Ok(out) = std::process::Command::new(&p).args(&argv).output() else {
        return;
    };
    let mut dead_leases: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut it = line.split('\t');
        let (Some(id), Some(host), Some(lease)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let labels = std::collections::HashMap::from([
            ("a2a.host".to_string(), host.to_string()),
            ("a2a.lease".to_string(), lease.to_string()),
        ]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            let _ = std::process::Command::new(runtime)
                .args(["rm", "-f", id])
                .output();
            if !lease.is_empty() {
                dead_leases.insert(lease.to_string());
            }
        }
    }
    for lease in dead_leases {
        let _ = std::fs::remove_file(&lease);
    }
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
}
