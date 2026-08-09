use crate::custody::{
    custody_record_path, is_custody_record_name, read_custody_record_in, CustodyReadRefusalV1,
    CustodySweepDispositionV1, WorktreeCustodyRecordV1,
};
use crate::provider::{prune_argv, remove_argv};
use crate::provider_path::{canonicalize_lenient, read_sidecar, sidecar_path};
use bridge_core::fs_custody::PinnedDirectoryV1;
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

/// A record enumerated by the dual-pattern scan.
///
/// Focused boundary §2.2 requires the boot sweep to scan **both** patterns: legacy
/// `*.meta.json` under the existing bounded policy, and `*.custody.v1.json` under §5
/// policy. Without the second pattern V3 checkouts would leak unreclaimed forever.
#[derive(Debug)]
pub enum ScannedWorktreeRecordV1 {
    /// A readable legacy V2 sidecar.
    Legacy(crate::provider_path::WorktreeSidecar),
    /// A readable, canonically-decoded V3 custody record.
    Custody(Box<WorktreeCustodyRecordV1>),
    /// A V3-named entry that could not be read under descriptor custody, or that did not
    /// decode. Never actionable — it classifies as unknown.
    UnreadableCustody(CustodyReadRefusalV1),
}

/// Iterate the readable worktree records directly under `root`, in both patterns.
///
/// V3 entries are read through a single pinned handle on `root`, so the record open is
/// descriptor-relative, no-follow, regular-file-only, single-link-only and byte-bounded.
pub fn scan_worktree_records(root: &str) -> Vec<(String, ScannedWorktreeRecordV1)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    let pinned = PinnedDirectoryV1::open(Path::new(root), "worktree sweep root").ok();
    for e in rd.flatten() {
        let p = e.path();
        let ps = p.to_string_lossy().to_string();
        if ps.ends_with(".meta.json") {
            if let Some(s) = read_sidecar(&ps) {
                out.push((ps, ScannedWorktreeRecordV1::Legacy(s)));
            }
        } else if is_custody_record_name(&ps) {
            let scanned = match pinned.as_ref() {
                Some(dir) => match read_custody_record_in(dir, &e.file_name()) {
                    Ok(record) => ScannedWorktreeRecordV1::Custody(Box::new(record)),
                    Err(refusal) => ScannedWorktreeRecordV1::UnreadableCustody(refusal),
                },
                None => ScannedWorktreeRecordV1::UnreadableCustody(
                    CustodyReadRefusalV1::Unreadable("sweep root is not pinnable".to_string()),
                ),
            };
            out.push((ps, scanned));
        }
    }
    out
}

/// Does `record_file` name the custody record of its own existing worktree sibling?
///
/// The V3 twin of `sidecar_file_matches`, with one addition the legacy check does not
/// need: the sibling directory must exist. A record naming a vanished checkout is a
/// *missing* pair (§5.2), and a missing pair is unknown, not actionable.
fn custody_record_file_matches(record_file: &str, worktree_path: &str) -> bool {
    if !Path::new(worktree_path).is_dir() {
        return false;
    }
    let Ok(record_file) = std::fs::canonicalize(Path::new(record_file)) else {
        return false;
    };
    let Ok(expected) = std::fs::canonicalize(Path::new(&custody_record_path(worktree_path))) else {
        return false;
    };
    record_file == expected
}

/// Classify one scanned V3 entry. **Recovery-only: no result authorizes deletion.**
///
/// Per §5.2 the state is parsed before any run id or lease is examined, and both existing
/// custody guards apply to the V3 arm exactly as they do to the legacy arm: the
/// record↔sibling match defeats a forged record pointing at another directory, and the
/// under-root check defeats one pointing outside the sweep root.
///
/// Mutation-checked (both reverted before commit): deleting `custody_record_file_matches`'s
/// `is_dir` precondition turned `sweep_treats_mismatched_and_missing_v3_pairs_as_unknown`
/// red on its *missing*-pair case only; weakening `read_custody_record_in`'s `nlink != 1`
/// to `nlink > 99` turned `sweep_treats_multi_link_v3_record_as_unknown_and_never_deletes`
/// red, with the record classifying `Recover` instead of `Unknown`.
#[must_use]
pub fn custody_entry_disposition(
    root: &SessionCwd,
    record_file: &str,
    record: Result<&WorktreeCustodyRecordV1, &CustodyReadRefusalV1>,
) -> CustodySweepDispositionV1 {
    let Ok(record) = record else {
        // Corrupt, missing, symlinked, multiply-linked, or over-bound: unknown.
        return CustodySweepDispositionV1::Unknown;
    };
    let worktree_path = record.worktree.canonical_path.as_str();
    if !custody_record_file_matches(record_file, worktree_path) {
        return CustodySweepDispositionV1::Unknown;
    }
    if !worktree_under_root(root, worktree_path) {
        return CustodySweepDispositionV1::Refused;
    }
    if recorded_identity_matches_sibling(record) == Some(false) {
        // The record is well-formed and correctly placed, but the directory now
        // present is not the one whose identity it recorded. That is ambiguous
        // evidence, not a licence: fall back to the protective classification.
        tracing::warn!(
            record = record_file,
            worktree_path,
            "worktree custody record does not match the directory identity now present"
        );
        return CustodySweepDispositionV1::Recover;
    }
    record.sweep_disposition()
}

/// Compare the record's recorded object identity against the directory that is actually
/// there, **by descriptor** — §2.2: "Identity is checked by descriptor, not by
/// re-canonicalizing a string, at every decision point."
///
/// `None` means there is nothing to check: either the record is degraded (P2 — a
/// pre-materialization writer records plan-derived paths with no `dev`/`ino`), or the
/// platform has no such evidence (brief risk R-10, non-unix).
fn recorded_identity_matches_sibling(record: &WorktreeCustodyRecordV1) -> Option<bool> {
    let recorded_dev = record.worktree.directory_identity.dev?;
    let recorded_ino = record.worktree.directory_identity.ino?;
    let file = bridge_core::fs_custody::open_directory_no_follow_raw(Path::new(
        record.worktree.canonical_path.as_str(),
    ))
    .ok()?;
    let metadata = file.metadata().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        Some(metadata.dev() == recorded_dev && metadata.ino() == recorded_ino)
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, recorded_dev, recorded_ino);
        None
    }
}

/// Report one classified V3 entry. Slice 2a acts on none of them: it never runs `git`,
/// `remove_dir_all`, reset, clean, or checkout for a V3 record.
fn report_custody_entry(
    record_file: &str,
    disposition: CustodySweepDispositionV1,
    state_tag: Option<String>,
    refusal: Option<&CustodyReadRefusalV1>,
) {
    debug_assert!(
        !disposition.authorizes_checkout_removal(),
        "slice 2a mints no deletion authority for V3 custody records"
    );
    match refusal {
        Some(refusal) => tracing::warn!(
            record = record_file,
            category = disposition.report_category(),
            refusal = %refusal,
            "worktree custody record is unreadable; leaving it for recovery"
        ),
        None => tracing::info!(
            record = record_file,
            category = disposition.report_category(),
            state = state_tag.unwrap_or_default(),
            "worktree custody record is protected from the sweep"
        ),
    }
}

/// Reap only same-host **legacy** worktrees whose owner lease is free.
///
/// V3 custody records are recognized and classified, never deleted (§5.2).
pub fn sweep_orphans(root: &str, my_host: &str, probe: &dyn LeaseProbe) {
    let Ok(root_cwd) = canonicalize_lenient(root) else {
        tracing::warn!(root, "skipping worktree sweep with non-canonical root");
        return;
    };
    for (path, scanned) in scan_worktree_records(root) {
        match scanned {
            ScannedWorktreeRecordV1::Legacy(s) => {
                let labels = HashMap::from([
                    ("a2a.host".to_string(), s.host.clone()),
                    ("a2a.lease".to_string(), s.lease.clone()),
                ]);
                if classify(&labels, my_host, probe) == Verdict::Dead {
                    remove_worktree_if_safe(&root_cwd, &path, &s);
                }
            }
            ScannedWorktreeRecordV1::Custody(record) => {
                let disposition = custody_entry_disposition(&root_cwd, &path, Ok(&record));
                report_custody_entry(
                    &path,
                    disposition,
                    Some(record.state.kind().wire_tag()),
                    None,
                );
            }
            ScannedWorktreeRecordV1::UnreadableCustody(refusal) => {
                let disposition = custody_entry_disposition(&root_cwd, &path, Err(&refusal));
                report_custody_entry(&path, disposition, None, Some(&refusal));
            }
        }
    }
}

/// Run-end backstop for worktrees created by a single bridge process run.
///
/// **Unconditionally non-destructive for V3 custody records** (focused boundary §5.2
/// bullet 2). For legacy V2 sidecars it applies the slice-2 brief's R9 ruling where R9
/// actually bites: deletion authority is removed from the **unwind** path.
///
/// R9's thrust is that an abrupt `Drop` is the moment the process knows least about
/// whether the work is still wanted, so it must not delete. That argument is about the
/// abrupt path. Making a *clean* exit defer as well would not defer the reclaim — it
/// would leak it permanently, because the boot sweep provably cannot fire afterwards:
/// `LeaseGuard::drop` unlinks the lease file on a clean drop (`liveness.rs:130-136`, and
/// its own doc-comment at `:115`: "The file is removed on a clean drop; after a crash it
/// persists with the lock FREE (the recovery signal)"), `FsLeaseProbe::try_state` then
/// answers `None` (`:253-258`), and `classify` maps `None` to `Unknown`, never `Dead`
/// (`run_identity.rs:110`). The free-lease recovery signal exists only after a crash.
///
/// So: clean drop reclaims this run's own legacy entries exactly as before R9; a drop
/// during an unwind defers to the boot sweep, which *can* fire in that case because the
/// crashed process left its lease file behind with the lock free. Pinned by
/// `boot_sweep_cannot_reclaim_a_cleanly_exited_run`.
///
/// Explicit run-end *settlement* — converting unresolved live V3 entries to
/// preserved/unknown before this backstop ever runs — is a later sub-slice's, and needs
/// the durable replace primitive this slice does not have.
pub struct WorktreeRunEndGuard {
    pub root: String,
    pub instance_id: String,
}

impl Drop for WorktreeRunEndGuard {
    fn drop(&mut self) {
        let unwinding = std::thread::panicking();
        let root_cwd = canonicalize_lenient(&self.root);
        for (path, scanned) in scan_worktree_records(&self.root) {
            match scanned {
                ScannedWorktreeRecordV1::Legacy(s) if s.run_id == self.instance_id => {
                    match (&root_cwd, unwinding) {
                        (_, true) => tracing::info!(
                            sidecar = path,
                            worktree_path = s.worktree_path,
                            run_id = self.instance_id,
                            "deferring worktree reclaim of this run to the next boot sweep \
                             (dropping during an unwind)"
                        ),
                        (Ok(root_cwd), false) => remove_worktree_if_safe(root_cwd, &path, &s),
                        (Err(_), false) => tracing::warn!(
                            root = self.root,
                            "skipping worktree end sweep with non-canonical root"
                        ),
                    }
                }
                ScannedWorktreeRecordV1::Legacy(_) => {}
                ScannedWorktreeRecordV1::Custody(record) => tracing::info!(
                    record = path,
                    state = record.state.kind().wire_tag(),
                    run_id = self.instance_id,
                    "leaving custody-protected worktree record untouched at run end"
                ),
                ScannedWorktreeRecordV1::UnreadableCustody(refusal) => tracing::warn!(
                    record = path,
                    refusal = %refusal,
                    run_id = self.instance_id,
                    "leaving unreadable worktree custody record untouched at run end"
                ),
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

    /// REVISED per the slice-2 brief's R9 ruling, then repaired: R9 removes the
    /// run-end guard's deletion authority from the **unwind** path, which is
    /// where an abrupt `Drop` knows least about whether the work is wanted. It
    /// does *not* make a clean exit defer, because the boot sweep provably
    /// cannot reclaim afterwards — see
    /// `boot_sweep_cannot_reclaim_a_cleanly_exited_run` below, which pins the
    /// mechanism. Discriminates: the clean-exit legacy reclaim being dropped
    /// (a permanent worktree + sidecar leak on every clean `[worktrees]` run),
    /// and the guard widening beyond its own run.
    #[test]
    fn end_guard_reclaims_only_this_run_on_a_clean_exit() {
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

    /// The R9 half that survives: dropping **during an unwind** defers instead
    /// of deleting. Discriminates: `std::thread::panicking()` being dropped
    /// from the guard, which would restore deletion on exactly the path where
    /// the process cannot know whether the checkout still matters.
    #[test]
    fn end_guard_defers_reclaim_when_dropping_during_an_unwind() {
        let root = unique_temp_dir("end-guard-unwind");
        fs::create_dir_all(&root).unwrap();
        let mine = write_worktree_sidecar(&root, "mine", "my-host", "/leases/mine.lock", "mine");

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = super::WorktreeRunEndGuard {
                root: root.to_string_lossy().into_owned(),
                instance_id: "mine".into(),
            };
            panic!("run failed after the worktree was configured");
        }));
        assert!(unwound.is_err(), "the harness must actually unwind");

        assert!(
            Path::new(&mine.worktree_path).exists(),
            "an unwinding drop must not delete this run's checkout"
        );
        assert!(Path::new(&sidecar_path(&mine.worktree_path)).exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// The mechanism behind `end_guard_reclaims_only_this_run_on_a_clean_exit`,
    /// pinned so a future "just defer to the boot sweep" simplification cannot
    /// be made without seeing the leak it causes. `LeaseGuard::drop` **unlinks**
    /// the lease file on a clean drop (`liveness.rs:130-136`, and its own
    /// doc-comment at `:115`), `FsLeaseProbe::try_state` then answers `None`
    /// (`liveness.rs:253-258`), and `classify` maps `None` to `Unknown`, never
    /// `Dead` (`run_identity.rs:110`) — so the boot sweep's legacy arm never
    /// fires for a cleanly exited run. Uses the real lease and probe, not
    /// `FakeProbe`: this is a claim about production wiring.
    #[test]
    fn boot_sweep_cannot_reclaim_a_cleanly_exited_run() {
        use bridge_core::liveness::{acquire_lease_in, FsLeaseProbe};

        let root = unique_temp_dir("clean-exit-leak");
        let leases = unique_temp_dir("clean-exit-leak-leases");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&leases).unwrap();

        let lease = acquire_lease_in(&leases, "run-clean").unwrap();
        let lease_path = lease.path().to_string_lossy().into_owned();
        let orphan = write_worktree_sidecar(&root, "orphan", "my-host", &lease_path, "run-clean");

        // Held ⇒ Alive: the sweep must not touch a live run's checkout.
        super::sweep_orphans(&root.to_string_lossy(), "my-host", &FsLeaseProbe);
        assert!(Path::new(&orphan.worktree_path).exists());

        // Clean exit unlinks the lease file, so the probe can no longer answer
        // "free" — the verdict is Unknown and the boot sweep is a no-op forever.
        drop(lease);
        assert_eq!(
            FsLeaseProbe.try_state(&lease_path),
            None,
            "a cleanly dropped lease leaves no evidence for the boot sweep"
        );
        super::sweep_orphans(&root.to_string_lossy(), "my-host", &FsLeaseProbe);
        assert!(
            Path::new(&orphan.worktree_path).exists(),
            "the boot sweep cannot reclaim a cleanly exited run; the run-end \
             guard is the only thing that can"
        );

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&leases).unwrap();
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

    /// Still live, and still discriminating, after the R9 repair: the run-end
    /// guard reclaims on a clean drop, so this forged record really does reach
    /// `remove_worktree_if_safe` and really is stopped by its guards.
    ///
    /// Mutation-checked (all reverted before commit): neutering
    /// `sidecar_file_matches` alone leaves this green, and so does neutering
    /// `worktree_under_root` alone -- the two guards defend this input
    /// redundantly. Neutering **both** turns this test and its boot-sweep twin
    /// `sweep_skips_sidecar_that_points_at_non_sibling_worktree` red together.
    /// So the pair is genuine coverage of the guard *set*, not of either guard
    /// individually; a single-guard regression would slip past both.
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

    // ---- R2f1b slice 2a: dual-pattern recognition, recovery-only V3 arm ----

    use crate::custody::{
        custody_record_path, CustodySweepDispositionV1, PreservationReasonV1,
        PreservedWorktreeClaimV1, RecoveryLocatorV1, WorktreeCustodyRecordV1,
        WorktreeCustodyStateV1, WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
    };
    use bridge_core::execution_policy::{
        PolicyNodeRefV1, Sha256HexV1, WorktreeCustodyIdV1, WorktreeObjectIdentityV1,
    };
    use bridge_core::fs_custody::DirectoryIdentityV1;
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};

    fn sha(digit: char) -> Sha256HexV1 {
        Sha256HexV1::parse(digit.to_string().repeat(64)).unwrap()
    }

    /// An object identity carrying the directory's **observed** `dev`/`ino` when the
    /// path exists, so records built here are the shape a real writer publishes and the
    /// sweep's descriptor comparison (P3) actually has evidence to check. When
    /// `degraded`, the plan-derived path is all that is recorded — the shape a
    /// pre-materialization writer can produce (P2).
    fn object_with(path: &str, degraded: bool) -> WorktreeObjectIdentityV1 {
        let observed = (!degraded)
            .then(|| {
                std::fs::symlink_metadata(path).ok().map(|meta| {
                    use std::os::unix::fs::MetadataExt as _;
                    (meta.dev(), meta.ino())
                })
            })
            .flatten();
        let fallback = if degraded { None } else { Some((1, 2)) };
        let identity = observed.or(fallback);
        WorktreeObjectIdentityV1 {
            canonical_path: path.to_string(),
            directory_identity: DirectoryIdentityV1 {
                canonical_path: path.to_string(),
                dev: identity.map(|(dev, _)| dev),
                ino: identity.map(|(_, ino)| ino),
            },
        }
    }

    fn attempt_identity() -> AttemptIdentity {
        AttemptIdentity {
            execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
            attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
            ordinal: 0,
            parent_attempt_id: None,
        }
    }

    fn custody_record(worktree: &str, state: WorktreeCustodyStateV1) -> WorktreeCustodyRecordV1 {
        let custody_id = WorktreeCustodyIdV1::parse(format!("custody-{}", "3".repeat(64))).unwrap();
        // Publish the identity shape this state is settled to carry (P2).
        let degraded =
            state.identity_completeness() == crate::custody::IdentityCompletenessV1::MayBeDegraded;
        let object = |path: &str| object_with(path, degraded);
        let claim =
            (state.claim_presence() == crate::custody::ClaimPresenceV1::Required).then(|| {
                PreservedWorktreeClaimV1 {
                    schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
                    custody_id: custody_id.clone(),
                    execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                    // Ordinal 0: the delivery-origin attempt is the current one.
                    origin_attempt_id: attempt_identity().attempt_id,
                    current_attempt: attempt_identity(),
                    node: PolicyNodeRefV1 {
                        sorted_ordinal: 0,
                        id_sha256: sha('5'),
                    },
                    checkout_fingerprint: sha('6'),
                    source: object("/src"),
                    root: object("/root"),
                    worktree: object(worktree),
                    common_dir: object("/src/.git"),
                    reason: match &state {
                        WorktreeCustodyStateV1::PreservationUnknown { reason } => *reason,
                        _ => PreservationReasonV1::NodeFailure,
                    },
                    created_wall_ms: 1_700_000_000_000,
                    recovery_locator: RecoveryLocatorV1::RegisteredWorktree {},
                }
            });
        WorktreeCustodyRecordV1 {
            schema_version: WORKTREE_CUSTODY_RECORD_SCHEMA_V1,
            custody_id,
            checkout_fingerprint: sha('6'),
            current_attempt: attempt_identity(),
            worktree: object(worktree),
            state,
            claim,
        }
    }

    /// Materialize a V3 checkout: the worktree directory plus its sibling
    /// `.custody.v1.json` record. Returns `(worktree_path, record_path)`.
    fn write_custody_checkout(
        root: &Path,
        name: &str,
        state: WorktreeCustodyStateV1,
    ) -> (PathBuf, PathBuf) {
        let worktree_path = root.join(name);
        fs::create_dir_all(&worktree_path).unwrap();
        let canonical = fs::canonicalize(&worktree_path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let record = custody_record(&canonical, state);
        let record_path = PathBuf::from(custody_record_path(&canonical));
        fs::write(&record_path, record.encode_canonical().unwrap()).unwrap();
        (PathBuf::from(canonical), record_path)
    }

    fn dead_probe(lease: &str) -> FakeProbe {
        FakeProbe(HashMap::from([(lease.to_string(), Some(true))]))
    }

    fn scanned_disposition(root: &Path, record_path: &Path) -> CustodySweepDispositionV1 {
        let root_cwd = crate::provider_path::canonicalize_lenient(&root.to_string_lossy()).unwrap();
        let scanned = super::scan_worktree_records(&root.to_string_lossy());
        let target = fs::canonicalize(record_path)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| record_path.to_string_lossy().into_owned());
        let (path, entry) = scanned
            .into_iter()
            .find(|(path, _)| {
                fs::canonicalize(path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.clone())
                    == target
            })
            .expect("the V3 record must be enumerated by the dual-pattern scan");
        match entry {
            super::ScannedWorktreeRecordV1::Legacy(_) => {
                panic!("a .custody.v1.json record must not scan as a legacy sidecar")
            }
            super::ScannedWorktreeRecordV1::Custody(record) => {
                super::custody_entry_disposition(&root_cwd, &path, Ok(&record))
            }
            super::ScannedWorktreeRecordV1::UnreadableCustody(refusal) => {
                super::custody_entry_disposition(&root_cwd, &path, Err(&refusal))
            }
        }
    }

    /// Discriminates: the boot sweep failing to enumerate the V3 pattern at all
    /// (focused boundary §2.2 -- without dual-pattern recognition "V3 checkouts
    /// would leak unreclaimed forever"), or classifying a live V3 record as
    /// anything other than recovery. The dead-lease probe is the exact input
    /// that reaps a legacy sidecar, so this also pins that lease liveness never
    /// authorizes a V3 deletion.
    #[test]
    fn sweep_recognizes_live_v3_record_as_recovery_and_never_deletes_it() {
        let root = unique_temp_dir("v3-live");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "live", WorktreeCustodyStateV1::LiveProtected {});

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Recover
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists(), "V3 checkout must survive the boot sweep");
        assert!(record.exists(), "V3 record must survive the boot sweep");

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: a preserved (terminal, R2f2-owned) V3 record being
    /// classified as anything but preserved, or being removed.
    #[test]
    fn sweep_classifies_preserved_v3_record_as_preserved_and_never_deletes_it() {
        let root = unique_temp_dir("v3-preserved");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "kept", WorktreeCustodyStateV1::Preserved {});

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Preserved
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: an undecodable V3 record being treated as absent (and so
    /// as an ordinary orphan) rather than as unknown. §5.2: "every corrupt /
    /// missing / mismatched V3 pair" is ineligible for deletion.
    #[test]
    fn sweep_treats_corrupt_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-corrupt");
        fs::create_dir_all(&root).unwrap();
        let worktree = root.join("corrupt");
        fs::create_dir_all(&worktree).unwrap();
        let canonical = fs::canonicalize(&worktree).unwrap();
        let record = PathBuf::from(custody_record_path(&canonical.to_string_lossy()));
        fs::write(&record, b"{not json").unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the reader following a symlinked custody record instead
    /// of refusing it. A symlink is the cheapest way to make a sweep read one
    /// checkout's record while acting on another's directory.
    #[test]
    #[cfg(unix)]
    fn sweep_treats_symlinked_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-symlink");
        fs::create_dir_all(&root).unwrap();
        let worktree = root.join("linked");
        fs::create_dir_all(&worktree).unwrap();
        let canonical = fs::canonicalize(&worktree).unwrap();
        let elsewhere = root.join("real-record.json");
        fs::write(
            &elsewhere,
            custody_record(
                &canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        let record = PathBuf::from(custody_record_path(&canonical.to_string_lossy()));
        std::os::unix::fs::symlink(&elsewhere, &record).unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.symlink_metadata().is_ok());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the reader accepting a multiply-linked record. A second
    /// hard link means another name owns the same bytes, so exclusive custody
    /// of the record cannot be proved and its state cannot be trusted.
    #[test]
    #[cfg(unix)]
    fn sweep_treats_multi_link_v3_record_as_unknown_and_never_deletes() {
        let root = unique_temp_dir("v3-multilink");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "shared", WorktreeCustodyStateV1::LiveProtected {});
        fs::hard_link(&record, root.join("second-name.json")).unwrap();

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the sidecar-sibling guard not being applied to the V3
    /// arm. A record whose `worktree` names a directory that is not its own
    /// sibling -- or that does not exist at all -- is unknown, never
    /// actionable.
    #[test]
    fn sweep_treats_mismatched_and_missing_v3_pairs_as_unknown() {
        let root = unique_temp_dir("v3-mismatch");
        let victim = unique_temp_dir("v3-mismatch-victim");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(victim.join("keep"), "do not delete").unwrap();
        let victim_canonical = fs::canonicalize(&victim).unwrap();

        // Sibling mismatch: the record file is not `<its own worktree>.custody.v1.json`.
        let forged = root.join(format!("forged{}", crate::custody::CUSTODY_RECORD_SUFFIX));
        fs::write(
            &forged,
            custody_record(
                &victim_canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scanned_disposition(&root, &forged),
            CustodySweepDispositionV1::Unknown
        );

        // Missing sibling: the record is correctly named but its worktree is gone.
        let gone = root.join("gone");
        let gone_record = PathBuf::from(custody_record_path(&gone.to_string_lossy()));
        fs::write(
            &gone_record,
            custody_record(
                &gone.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            scanned_disposition(&root, &gone_record),
            CustodySweepDispositionV1::Unknown
        );

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(victim.join("keep").exists());
        assert!(forged.exists());
        assert!(gone_record.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&victim).unwrap();
    }

    /// Discriminates: the under-root guard not being applied to the V3 arm. A
    /// record that is its own well-formed sibling but sits outside the sweep
    /// root is refused, not classified by state.
    #[test]
    fn sweep_refuses_v3_record_pointing_outside_the_sweep_root() {
        let root = unique_temp_dir("v3-outside");
        let outside = unique_temp_dir("v3-outside-target");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_canonical = fs::canonicalize(&outside).unwrap();
        let record = PathBuf::from(custody_record_path(&outside_canonical.to_string_lossy()));
        fs::write(
            &record,
            custody_record(
                &outside_canonical.to_string_lossy(),
                WorktreeCustodyStateV1::LiveProtected {},
            )
            .encode_canonical()
            .unwrap(),
        )
        .unwrap();

        let root_cwd = crate::provider_path::canonicalize_lenient(&root.to_string_lossy()).unwrap();
        let decoded = custody_record(
            &outside_canonical.to_string_lossy(),
            WorktreeCustodyStateV1::LiveProtected {},
        );
        assert_eq!(
            super::custody_entry_disposition(&root_cwd, &record.to_string_lossy(), Ok(&decoded)),
            CustodySweepDispositionV1::Refused
        );

        assert!(outside.exists());

        fs::remove_dir_all(&root).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// P3. Discriminates: the V3 arm trusting the record's *path string*
    /// instead of comparing the identity it recorded against the directory that
    /// is actually there. §2.2: "Identity is checked by **descriptor**, not by
    /// re-canonicalizing a string, at every decision point." A directory
    /// swapped out from under a valid record is ambiguous evidence, so it falls
    /// back to `Recover` -- never actionable -- exactly like the corrupt arms.
    /// `Preserved` is used deliberately: its normal disposition is `Preserved`,
    /// so the fallback is observable (a `LiveProtected` record is `Recover`
    /// either way and would not discriminate).
    #[test]
    #[cfg(unix)]
    fn sweep_falls_back_to_recover_when_the_sibling_directory_was_swapped() {
        use std::os::unix::fs::MetadataExt as _;

        let root = unique_temp_dir("v3-swapped");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "swapped", WorktreeCustodyStateV1::Preserved {});

        // Control: the recorded identity matches the directory on disk.
        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Preserved
        );

        let before = fs::symlink_metadata(&worktree).unwrap().ino();
        fs::remove_dir_all(&worktree).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let after = fs::symlink_metadata(&worktree).unwrap().ino();
        assert_ne!(before, after, "the swap must actually change the inode");

        let disposition = scanned_disposition(&root, &record);
        assert_eq!(disposition, CustodySweepDispositionV1::Recover);
        assert!(!disposition.authorizes_checkout_removal());

        fs::remove_dir_all(&root).unwrap();
    }

    /// P3 + P2. Discriminates: the descriptor check firing on a record that
    /// legitimately has no identity to check. A degraded record (§5.1's
    /// materialization-unresolved case) carries plan-derived paths only, so
    /// there is nothing to compare and its classification must be unchanged.
    #[test]
    #[cfg(unix)]
    fn sweep_classification_of_a_degraded_record_ignores_the_directory_identity() {
        let root = unique_temp_dir("v3-degraded");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) = write_custody_checkout(
            &root,
            "degraded",
            WorktreeCustodyStateV1::PreservationUnknown {
                reason: PreservationReasonV1::MaterializationInFlight,
            },
        );

        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown
        );

        fs::remove_dir_all(&worktree).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        assert_eq!(
            scanned_disposition(&root, &record),
            CustodySweepDispositionV1::Unknown,
            "a degraded record has no identity evidence to invalidate"
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the legacy boot arm changing behaviour once the V3 arm
    /// exists. In a mixed root the legacy dead-lease reclaim must still fire,
    /// byte for byte, while the V3 record beside it survives.
    #[test]
    fn legacy_boot_arm_still_reclaims_alongside_a_v3_record() {
        let root = unique_temp_dir("mixed-root");
        fs::create_dir_all(&root).unwrap();
        let dead = write_worktree_sidecar(&root, "dead", "my-host", "/leases/dead.lock", "run-a");
        let (v3_worktree, v3_record) =
            write_custody_checkout(&root, "v3", WorktreeCustodyStateV1::LiveProtected {});

        super::sweep_orphans(
            &root.to_string_lossy(),
            "my-host",
            &dead_probe("/leases/dead.lock"),
        );

        assert!(!Path::new(&dead.worktree_path).exists());
        assert!(!Path::new(&sidecar_path(&dead.worktree_path)).exists());
        assert!(v3_worktree.exists());
        assert!(v3_record.exists());

        fs::remove_dir_all(&root).unwrap();
    }

    /// Discriminates: the run-end guard acquiring any authority over a V3
    /// record. §5.2: the guard's `Drop` backstop is non-destructive and the
    /// already-synced protection record is authoritative.
    #[test]
    fn end_guard_is_non_destructive_for_v3_records() {
        let root = unique_temp_dir("end-guard-v3");
        fs::create_dir_all(&root).unwrap();
        let (worktree, record) =
            write_custody_checkout(&root, "v3", WorktreeCustodyStateV1::LiveProtected {});

        {
            let _guard = super::WorktreeRunEndGuard {
                root: root.to_string_lossy().into_owned(),
                instance_id: attempt_identity().run_id().to_string(),
            };
        }

        assert!(worktree.exists());
        assert!(record.exists());

        fs::remove_dir_all(&root).unwrap();
    }
}
