use crate::provider::{prune_argv, remove_argv};
use crate::provider_path::{canonicalize_lenient, read_sidecar, sidecar_path};
use bridge_core::liveness::LeaseProbe;
use bridge_core::run_identity::{classify, Verdict};
use bridge_core::SessionCwd;
use std::collections::HashMap;
use std::path::Path;

// Stays sync (not de-blocked like host_git.rs's run_git): this call runs inside
// `WorktreeRunEndGuard::drop` (a `Drop` impl cannot await) and during the
// startup/boot sweep — not a per-turn path. See spec
// docs/superpowers/specs/2026-07-03-wave-1-hardening.md §W1-C.
fn run_git_sync(argv: &[&str]) {
    let _ = std::process::Command::new("git").args(argv).output();
}

/// Best-effort remove a worktree + its sidecar.
fn remove_worktree(canonical_source: &str, common_dir: &str, worktree_path: &str) {
    run_git_sync(&remove_argv(canonical_source, worktree_path));
    run_git_sync(&prune_argv(canonical_source));
    if !common_dir.is_empty() {
        run_git_sync(&["--git-dir", common_dir, "worktree", "prune"]);
    }
    let _ = std::fs::remove_dir_all(worktree_path);
    let _ = std::fs::remove_file(sidecar_path(worktree_path));
}

fn sidecar_file_matches(sidecar_file: &str, worktree_path: &str) -> bool {
    let Ok(sidecar_file) = std::fs::canonicalize(Path::new(sidecar_file)) else {
        return false;
    };
    let Ok(expected) = std::fs::canonicalize(Path::new(&sidecar_path(worktree_path))) else {
        return false;
    };
    sidecar_file == expected
}

fn worktree_under_root(root: &SessionCwd, worktree_path: &str) -> bool {
    canonicalize_lenient(worktree_path)
        .map(|wt| wt.is_under(root))
        .unwrap_or(false)
}

fn remove_worktree_if_safe(
    root: &SessionCwd,
    sidecar_file: &str,
    s: &crate::provider_path::WorktreeSidecar,
) {
    if !sidecar_file_matches(sidecar_file, &s.worktree_path) {
        tracing::warn!(
            sidecar = sidecar_file,
            worktree_path = s.worktree_path,
            "skipping worktree sidecar whose file does not match its worktree sibling"
        );
        return;
    }
    if !worktree_under_root(root, &s.worktree_path) {
        tracing::warn!(
            sidecar = sidecar_file,
            worktree_path = s.worktree_path,
            root = root.as_str(),
            "skipping worktree sidecar outside sweep root"
        );
        return;
    }
    remove_worktree(&s.canonical_source, &s.common_dir, &s.worktree_path);
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
    let Ok(root_cwd) = canonicalize_lenient(root) else {
        tracing::warn!(root, "skipping worktree sweep with non-canonical root");
        return;
    };
    for (path, s) in sidecars(root) {
        let labels = HashMap::from([
            ("a2a.host".to_string(), s.host.clone()),
            ("a2a.lease".to_string(), s.lease.clone()),
        ]);
        if classify(&labels, my_host, probe) == Verdict::Dead {
            remove_worktree_if_safe(&root_cwd, &path, &s);
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
        let Ok(root_cwd) = canonicalize_lenient(&self.root) else {
            tracing::warn!(
                root = self.root,
                "skipping worktree end sweep with non-canonical root"
            );
            return;
        };
        for (path, s) in sidecars(&self.root) {
            if s.run_id == self.instance_id {
                remove_worktree_if_safe(&root_cwd, &path, &s);
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

    #[test]
    fn sweep_skips_sidecar_that_points_at_non_sibling_worktree() {
        let root = unique_temp_dir("sidecar-mismatch");
        let victim = unique_temp_dir("sidecar-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "run-a".into(),
            host: "my-host".into(),
            lease: "/leases/dead.lock".into(),
        };
        let forged = root.join("forged.meta.json");
        fs::write(&forged, serde_json::to_vec(&sidecar).unwrap()).unwrap();
        let probe = FakeProbe(HashMap::from([(
            "/leases/dead.lock".to_string(),
            Some(true),
        )]));

        super::sweep_orphans(&root.to_string_lossy(), "my-host", &probe);

        assert!(victim.join("keep").exists());
        assert!(forged.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    #[test]
    fn end_guard_skips_sidecar_that_points_at_non_sibling_worktree() {
        let root = unique_temp_dir("end-guard-mismatch");
        let victim = unique_temp_dir("end-guard-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let sidecar = WorktreeSidecar {
            canonical_source: root.join("source").to_string_lossy().into_owned(),
            common_dir: root.join("source/.git").to_string_lossy().into_owned(),
            worktree_path: victim.to_string_lossy().into_owned(),
            owner: "owner".into(),
            run_id: "mine".into(),
            host: "my-host".into(),
            lease: "/leases/mine.lock".into(),
        };
        let forged = root.join("forged.meta.json");
        fs::write(&forged, serde_json::to_vec(&sidecar).unwrap()).unwrap();

        {
            let _guard = super::WorktreeRunEndGuard {
                root: root.to_string_lossy().into_owned(),
                instance_id: "mine".into(),
            };
        }

        assert!(victim.join("keep").exists());
        assert!(forged.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }
}
