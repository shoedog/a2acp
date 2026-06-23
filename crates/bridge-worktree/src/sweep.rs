use crate::provider::{prune_argv, remove_argv};
use crate::provider_path::{read_sidecar, sidecar_path};
use bridge_core::liveness::LeaseProbe;
use bridge_core::run_identity::{classify, Verdict};
use std::collections::HashMap;

fn run_git_sync(argv: &[&str]) {
    let _ = std::process::Command::new("git").args(argv).output();
}

/// Best-effort remove a worktree + its sidecar.
fn remove_worktree(canonical_source: &str, worktree_path: &str) {
    run_git_sync(&remove_argv(canonical_source, worktree_path));
    run_git_sync(&prune_argv(canonical_source));
    let _ = std::fs::remove_dir_all(worktree_path);
    let _ = std::fs::remove_file(sidecar_path(worktree_path));
}

/// Iterate readable worktree sidecars directly under `root`.
fn sidecars(root: &str) -> Vec<(String, crate::provider_path::WorktreeSidecar)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            let ps = p.to_string_lossy().to_string();
            if ps.ends_with(".meta.json") {
                if let Some(s) = read_sidecar(&ps) {
                    out.push((ps, s));
                }
            }
        }
    }
    out
}

/// Reap only same-host worktrees whose owner lease is free.
pub fn sweep_orphans(root: &str, my_host: &str, probe: &dyn LeaseProbe) {
    for (_path, s) in sidecars(root) {
        let labels = HashMap::from([
            ("a2a.host".to_string(), s.host.clone()),
            ("a2a.lease".to_string(), s.lease.clone()),
        ]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            remove_worktree(&s.canonical_source, &s.worktree_path);
        }
    }
}

/// Synchronous one-shot cleanup for worktrees created by a single bridge process run.
pub struct WorktreeRunEndGuard {
    pub root: String,
    pub instance_id: String,
}

impl Drop for WorktreeRunEndGuard {
    fn drop(&mut self) {
        for (_path, s) in sidecars(&self.root) {
            if s.run_id == self.instance_id {
                remove_worktree(&s.canonical_source, &s.worktree_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::provider_path::{sidecar_path, write_sidecar, WorktreeSidecar};
    use bridge_core::liveness::LeaseProbe;
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct FakeProbe(HashMap<String, Option<bool>>);

    impl LeaseProbe for FakeProbe {
        fn try_state(&self, lease_path: &str) -> Option<bool> {
            self.0.get(lease_path).copied().flatten()
        }
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-worktree-sweep-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn write_worktree_sidecar(
        root: &Path,
        name: &str,
        host: &str,
        lease: &str,
        run_id: &str,
    ) -> WorktreeSidecar {
        let worktree_path = root.join(name);
        fs::create_dir_all(&worktree_path).unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: worktree_path.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: run_id.into(),
            host: host.into(),
            lease: lease.into(),
        };
        write_sidecar(&sidecar).unwrap();
        sidecar
    }

    #[test]
    fn sweep_reaps_dead_owner_keeps_live() {
        let root = unique_temp_dir("orphans");
        fs::create_dir_all(&root).unwrap();
        let dead = write_worktree_sidecar(&root, "dead", "my-host", "/leases/dead.lock", "run-a");
        let live = write_worktree_sidecar(&root, "live", "my-host", "/leases/live.lock", "run-b");
        let other =
            write_worktree_sidecar(&root, "other", "other-host", "/leases/other.lock", "run-c");
        let probe = FakeProbe(HashMap::from([
            ("/leases/dead.lock".to_string(), Some(true)),
            ("/leases/live.lock".to_string(), Some(false)),
            ("/leases/other.lock".to_string(), Some(true)),
        ]));

        super::sweep_orphans(&root.to_string_lossy(), "my-host", &probe);

        assert!(!Path::new(&dead.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&dead.worktree_path)).exists());
        assert!(Path::new(&live.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&live.worktree_path)).exists());
        assert!(Path::new(&other.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&other.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn end_guard_removes_only_this_run() {
        let root = unique_temp_dir("end-guard");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");
        let other =
            write_worktree_sidecar(&root, "other", "my-host", "/leases/other.lock", "other");

        {
            let _guard = super::WorktreeRunEndGuard {
                root: root.to_string_lossy().into_owned(),
                instance_id: "mine".into(),
            };
        }

        assert!(!Path::new(&mine.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&mine.worktree_path)).exists());
        assert!(Path::new(&other.worktree_path).exists());
        assert!(Path::new(&sidecar_path(&other.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }
}
