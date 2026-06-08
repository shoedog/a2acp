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
            ("/l/dead.lock".to_string(), Some(true)),  // free ⇒ dead
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
