//! Shared container-reaping primitives (used by the :rw ContainerRwBackend and the :ro AcpBackend path).
//! Detached + idempotent so a `Drop` (which may fire off-runtime at process shutdown) never blocks/panics.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// `(runtime, name) -> fire-and-forget reap`. Injectable so tests don't spawn Docker.
pub type ReapFn = Arc<dyn Fn(String, String) + Send + Sync>;
/// `(name-filter) -> async sweep`. Boot-time orphan sweep over a `name=<filter>` filter.
pub type SweepFn = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

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

/// Production boot-sweep: `<runtime> ps -aq --filter name=<filter>` then `rm -f` each id. Best-effort.
pub fn production_sweep_fn(runtime: String) -> SweepFn {
    Arc::new(move |filter: String| {
        let runtime = runtime.clone();
        Box::pin(async move {
            let ps = tokio::process::Command::new(&runtime)
                .args(["ps", "-aq", "--filter", &format!("name={filter}")])
                .output()
                .await;
            let Ok(ps) = ps else {
                tracing::warn!(runtime = %runtime, "container boot sweep: runtime unavailable");
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
