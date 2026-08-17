use crate::custody::RecoveryLocatorV1;
use crate::provider::{
    add_argv, is_repo_argv, list_porcelain_argv, prune_argv, remove_argv, CustodyAddFailureV1,
    CustodyAddOutcomeV1, CustodyAddTargetV1, WorktreeProvider,
};
#[cfg(test)]
use crate::sweep::{decide_unused_candidate, UnusedCandidateDecisionV1, UnusedCandidateRefusalV1};
use crate::sweep::{
    paths_resolve_to_same_identity, ExactAbsenceCandidateV1, ExactAbsenceObservationV1,
    ExactAbsenceProbeV1,
};
use bridge_core::error::BridgeError;
use std::path::Path;
use std::process::{Command as StdCommand, Output};
use std::time::Duration;
use tokio::process::Command;

pub struct HostGitWorktree;

impl HostGitWorktree {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HostGitWorktree {
    fn default() -> Self {
        Self::new()
    }
}

async fn run_git(argv: &[&str]) -> Result<Output, BridgeError> {
    let mut command = Command::new("git");
    command.kill_on_drop(true).args(argv);
    command
        .output()
        .await
        .map_err(|e| BridgeError::ConfigInvalid {
            reason: format!("git spawn: {e}"),
        })
}

/// The synchronous half of the recovery capability.
///
/// The boot sweep is synchronous (and may be reached from `Drop`), so it must not enter a Tokio
/// runtime just to reuse `run_git`. This explicit blocking `Command::output` mirrors the async
/// helper's error contract without creating a nested executor.
fn run_git_sync(argv: &[&str]) -> Result<Output, BridgeError> {
    StdCommand::new("git")
        .args(argv)
        .output()
        .map_err(|e| BridgeError::ConfigInvalid {
            reason: format!("git spawn: {e}"),
        })
}

fn retryable_lock_error(err: &str) -> bool {
    err.contains("index.lock")
        || err.contains("Another git process")
        || err.contains(".lock")
        || err.contains("cannot lock")
}

async fn cleanup_failed_add(repo: &str, wt: &str) {
    // B2: this runs on the async per-turn `configure_session` path; removing a full worktree checkout
    // of a large repo is seconds of blocking I/O. `tokio::fs` offloads it (spawn_blocking internally).
    let _ = tokio::fs::remove_dir_all(wt).await;
    let _ = run_git(&prune_argv(repo)).await;
}

async fn common_dir(repo: &str) -> String {
    let absolute = run_git(&[
        "-C",
        repo,
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ])
    .await;
    if let Ok(out) = absolute {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }

    let fallback = run_git(&["-C", repo, "rev-parse", "--git-common-dir"]).await;
    let Ok(out) = fallback else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }

    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if raw.is_empty() {
        return String::new();
    }
    let path = Path::new(&raw);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(repo).join(path)
    };
    std::fs::canonicalize(joined)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Prove the target path has no directory entry without following a final symlink.
///
/// `Path::try_exists` follows links, so it reports a dangling final link as missing. A link is
/// still an extant target that T3a must refuse; `symlink_metadata` observes that entry itself.
fn target_absent_from_probe(target: &Path) -> Result<bool, BridgeError> {
    match target.symlink_metadata() {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(BridgeError::ConfigInvalid {
            reason: format!("worktree target metadata failed: {error}"),
        }),
    }
}

fn removal_is_complete(
    prune_succeeded: bool,
    target_absent: bool,
    registration_absent: bool,
) -> bool {
    prune_succeeded && target_absent && registration_absent
}

fn registration_absent_from_porcelain(output: &[u8], wt: &str) -> Result<bool, BridgeError> {
    // Resolve the candidate even when Git has no worktree records. A malformed candidate is not
    // evidence of absence, and `paths_resolve_to_same_identity` intentionally refuses it.
    paths_resolve_to_same_identity(wt, wt)?;
    for field in output.split(|byte| *byte == 0) {
        let Some(path) = field.strip_prefix(b"worktree ") else {
            continue;
        };
        let path = std::str::from_utf8(path).map_err(|_| BridgeError::ConfigInvalid {
            reason: "worktree registration path is not valid UTF-8".to_string(),
        })?;
        if paths_resolve_to_same_identity(path, wt)? {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn registration_absent(repo: &str, wt: &str) -> Result<bool, BridgeError> {
    let listed = run_git(&list_porcelain_argv(repo)).await?;
    if !listed.status.success() {
        return Err(BridgeError::ConfigInvalid {
            reason: format!(
                "worktree list failed: {}",
                String::from_utf8_lossy(&listed.stderr).trim()
            ),
        });
    }
    registration_absent_from_porcelain(&listed.stdout, wt)
}

fn registration_absent_sync(repo: &str, wt: &str) -> Result<bool, BridgeError> {
    let listed = run_git_sync(&list_porcelain_argv(repo))?;
    if !listed.status.success() {
        return Err(BridgeError::ConfigInvalid {
            reason: format!(
                "worktree list failed: {}",
                String::from_utf8_lossy(&listed.stderr).trim()
            ),
        });
    }
    registration_absent_from_porcelain(&listed.stdout, wt)
}

impl ExactAbsenceProbeV1 for HostGitWorktree {
    fn observe_exact_absence(
        &self,
        candidate: &ExactAbsenceCandidateV1,
    ) -> Result<ExactAbsenceObservationV1, BridgeError> {
        if !target_absent_from_probe(Path::new(&candidate.worktree_path))? {
            return Ok(ExactAbsenceObservationV1::TargetPresent);
        }
        if !registration_absent_sync(&candidate.canonical_source, &candidate.worktree_path)? {
            return Ok(ExactAbsenceObservationV1::RegisteredButAbsent);
        }
        Ok(ExactAbsenceObservationV1::BothAbsent)
    }
}

/// `git worktree remove` + `prune`, then the two post-conditions §5.1 names: the target is absent
/// and the registration is gone.
///
/// Factored verbatim out of `HostGitWorktree::remove` (slice 2c2) so the V2 removal and the
/// capability removal share ONE definition of "the removal completed". A separate copy for
/// `remove_v2` would be a second place for the post-conditions to weaken, and §5.1's requirement
/// is explicitly to reuse these.
///
/// **`Err` is the fail-closed answer, and its shape is load-bearing for the caller:** it means the
/// removal did NOT verifiably complete, so no `Removed` tombstone may be recorded over it. That
/// covers a failed `git worktree remove`, a failed prune, a surviving target, a surviving
/// registration, and an unstattable target (the probe's own `Err`) — all of them "the checkout may
/// still be there", none of them "gone".
async fn remove_and_verify(repo: &str, wt: &str) -> Result<(), BridgeError> {
    let remove = run_git(&remove_argv(repo, wt)).await?;
    let prune = run_git(&prune_argv(repo)).await?;
    let target_absent = target_absent_from_probe(Path::new(wt))?;
    let registration_absent = registration_absent(repo, wt).await?;

    if removal_is_complete(prune.status.success(), target_absent, registration_absent) {
        return Ok(());
    }

    let remove_error = String::from_utf8_lossy(&remove.stderr).trim().to_owned();
    let prune_error = String::from_utf8_lossy(&prune.stderr).trim().to_owned();
    Err(BridgeError::ConfigInvalid {
        reason: format!(
            "worktree remove failed (remove_status={}, remove_stderr={remove_error:?}, prune_status={}, prune_stderr={prune_error:?}, target_absent={target_absent}, registration_absent={registration_absent})",
            remove.status, prune.status
        ),
    })
}

/// Classify a failed custody-aware add by DESCRIPTOR-level probes, never by the git error text.
///
/// The two probes answer independent questions and are kept independent: `target` decides whether
/// there is work to preserve, `recovery_locator` decides how a recovery consumer reaches it.
///
/// **2a's docstring'd obligation, discharged here.** `registration_absent` returns
/// `Result<bool, BridgeError>` and its `Err` arm is the ONLY producer of
/// [`RecoveryLocatorV1::RegistrationUnproven`]. Collapsing that `Err` into
/// `UnregisteredDirectory` (or propagating it) would make the third variant unreachable in
/// production and record every ambiguous probe as a definite answer — the exact failure mode
/// §5.7's ambiguity rows exist to prevent.
async fn classify_custody_add_failure(repo: &str, wt: &str, reason: String) -> CustodyAddFailureV1 {
    let target = match target_absent_from_probe(Path::new(wt)) {
        Ok(false) => CustodyAddTargetV1::Present,
        Ok(true) => CustodyAddTargetV1::ProvablyAbsent,
        Err(_) => CustodyAddTargetV1::Unproven,
    };
    let recovery_locator = match registration_absent(repo, wt).await {
        Ok(true) => RecoveryLocatorV1::UnregisteredDirectory {},
        Ok(false) => RecoveryLocatorV1::RegisteredWorktree {},
        Err(_) => RecoveryLocatorV1::RegistrationUnproven {},
    };
    let common_dir = common_dir(repo).await;
    CustodyAddFailureV1 {
        reason: format!("custody worktree add failed: {reason}"),
        target,
        common_dir: (!common_dir.is_empty()).then_some(common_dir),
        recovery_locator,
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for HostGitWorktree {
    async fn add(&self, repo: &str, wt: &str) -> Result<String, BridgeError> {
        let mut last_err = String::new();
        for _ in 0..5 {
            let out = run_git(&add_argv(repo, wt, "HEAD")).await?;
            if out.status.success() {
                return Ok(common_dir(repo).await);
            }

            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !retryable_lock_error(&err) {
                cleanup_failed_add(repo, wt).await;
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("worktree add failed: {err}"),
                });
            }

            last_err = err;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        cleanup_failed_add(repo, wt).await;
        Err(BridgeError::ConfigInvalid {
            reason: format!("worktree add failed after lock retries: {last_err}"),
        })
    }

    /// Enumeration 10 of 10: the one PRODUCTION impl, and the one that supports custody.
    fn supports_custody_add(&self) -> bool {
        true
    }

    /// The custody-aware add: [`HostGitWorktree::add`]'s retry loop with `cleanup_failed_add`
    /// REMOVED, and the failure classified instead.
    ///
    /// This is R-7's whole point. `add`'s two `cleanup_failed_add` call sites do
    /// `remove_dir_all(wt)`, which is outside the 2b1 deletion gate — and 2b1's own review proved
    /// the path is ROUTINE, not exotic: a refused custody rollback leaves the checkout on disk,
    /// the failed-configure loop retries the configure, `git worktree add` fails on the surviving
    /// directory, and today that would delete a protected checkout. Here it cannot: nothing in
    /// this function removes anything.
    async fn add_under_custody(
        &self,
        repo: &str,
        wt: &str,
    ) -> Result<CustodyAddOutcomeV1, BridgeError> {
        let mut last_err = String::new();
        for attempt in 0..5 {
            let out = run_git(&add_argv(repo, wt, "HEAD")).await?;
            if out.status.success() {
                return Ok(CustodyAddOutcomeV1::Materialized {
                    common_dir: common_dir(repo).await,
                });
            }
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            last_err = err;
            if !retryable_lock_error(&last_err) {
                break;
            }
            if attempt + 1 < 5 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        Ok(CustodyAddOutcomeV1::Failed(
            classify_custody_add_failure(repo, wt, last_err).await,
        ))
    }

    async fn remove(&self, repo: &str, wt: &str) -> Result<(), BridgeError> {
        remove_and_verify(repo, wt).await
    }

    /// Enumeration 1 of 11: the one PRODUCTION impl, and the one that supports the capability
    /// removal.
    fn supports_capability_removal(&self) -> bool {
        true
    }

    /// §5.1's `remove_v2`. The identity revalidation already happened — it is what produced the
    /// `AuthorizedRemovalV1` this method consumes, in the caller's last statement before the call,
    /// with no await between — so what remains here is exactly what §5.1 asks for after Git:
    /// "verifies registration + target absence afterward (`host_git.rs:153-161` already implements
    /// those post-conditions; reuse them)".
    ///
    /// [`remove_and_verify`] IS those post-conditions, factored out of [`Self::remove`] verbatim,
    /// so the V2 path and the capability path can never drift apart in what they accept as a
    /// completed removal.
    async fn remove_v2(
        &self,
        authorized: crate::custody_writer::AuthorizedRemovalV1,
    ) -> Result<(), BridgeError> {
        remove_and_verify(authorized.canonical_source(), authorized.worktree_path()).await
    }

    async fn is_git_repo(&self, path: &str) -> bool {
        matches!(run_git(&is_repo_argv(path)).await, Ok(out) if out.status.success()
            && String::from_utf8_lossy(&out.stdout).trim() == "true")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::WorktreeProvider;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-host-git-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    fn repo(tmp: &Path) -> PathBuf {
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["config", "user.email", "a@b.c"]);
        git(&src, &["config", "user.name", "x"]);
        std::fs::write(src.join("file.txt"), "base\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);
        src
    }

    #[test]
    fn cleanup_success_requires_absent_target_registration_and_successful_prune() {
        assert!(
            removal_is_complete(true, true, true),
            "a repeated remove is idempotent only after exact absence is proved"
        );
        assert!(!removal_is_complete(false, true, true));
        assert!(!removal_is_complete(true, false, true));
        assert!(!removal_is_complete(true, true, false));
        let tmp = unique_temp_dir("target-absence-probe");
        std::fs::create_dir_all(&tmp).unwrap();
        let target = tmp.join("target");
        assert!(target_absent_from_probe(&target).unwrap());
        std::fs::create_dir(&target).unwrap();
        assert!(!target_absent_from_probe(&target).unwrap());
        std::fs::remove_dir(&target).unwrap();
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn porcelain_registration_check_is_exact_and_handles_locked_records() {
        let output =
            b"worktree /repo\0HEAD abc\0\0worktree /managed/wt\0HEAD def\0locked reason\0\0";
        assert!(!registration_absent_from_porcelain(output, "/managed/wt").unwrap());
        assert!(registration_absent_from_porcelain(output, "/managed/other").unwrap());
        assert!(registration_absent_from_porcelain(output, "/managed/w").unwrap());
    }

    #[test]
    fn unresolvable_registration_paths_refuse_exact_absence() {
        assert!(
            registration_absent_from_porcelain(b"worktree /repo\0", "relative-target").is_err()
        );
        #[cfg(unix)]
        assert!(registration_absent_from_porcelain(
            b"worktree /tmp/invalid-utf8-\xff\0",
            "/tmp/target"
        )
        .is_err());

        let tmp = unique_temp_dir("unresolvable-exact-absence");
        let src = repo(&tmp);
        let candidate = ExactAbsenceCandidateV1::new(src.to_string_lossy(), "relative-target");
        assert_eq!(
            decide_unused_candidate(&candidate, false, &HostGitWorktree::new()),
            UnusedCandidateDecisionV1::Refused(UnusedCandidateRefusalV1::CannotProve)
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }
    /// The B18 sync capability observes the same three facts as the private async registration
    /// helper without entering a Tokio runtime from the sweep.
    #[tokio::test]
    async fn synchronous_exact_absence_capability_distinguishes_all_host_observations() {
        let tmp = unique_temp_dir("sync-exact-absence");
        let src = repo(&tmp);
        let canonical_worktree_root = tmp.join("worktrees");
        std::fs::create_dir(&canonical_worktree_root).unwrap();
        // `std::env::temp_dir()` is itself symlinked on macOS (`/var` -> `/private/var`), so the
        // root must be resolved before it can be called canonical. git always records the fully
        // canonical path, so an unresolved fixture root makes this test compare
        // `/var/...` against git's `/private/var/...` and fail for its own reasons — on the very
        // platform whose symlinked temp dir this test exists to exercise. Linux `/tmp` is not
        // symlinked, which is why the container lane never saw it.
        let canonical_worktree_root = std::fs::canonicalize(&canonical_worktree_root).unwrap();
        #[cfg(unix)]
        let worktree_root = {
            let symlinked_root = tmp.join("worktrees-through-symlink");
            std::os::unix::fs::symlink(&canonical_worktree_root, &symlinked_root).unwrap();
            symlinked_root
        };
        #[cfg(not(unix))]
        let worktree_root = canonical_worktree_root.clone();
        let target = worktree_root.join("target");
        let canonical_target = canonical_worktree_root.join("target");
        let candidate =
            ExactAbsenceCandidateV1::new(src.to_string_lossy(), target.to_string_lossy());
        let provider = HostGitWorktree::new();

        assert_eq!(
            provider.observe_exact_absence(&candidate).unwrap(),
            ExactAbsenceObservationV1::BothAbsent
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.join("missing-target"), &target).unwrap();
            assert_eq!(
                provider.observe_exact_absence(&candidate).unwrap(),
                ExactAbsenceObservationV1::TargetPresent,
                "a dangling link is an extant target and must not authorize exact absence"
            );
            std::fs::remove_file(&target).unwrap();
        }
        provider
            .add(src.to_str().unwrap(), target.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            provider.observe_exact_absence(&candidate).unwrap(),
            ExactAbsenceObservationV1::TargetPresent
        );

        std::fs::remove_dir_all(&canonical_target).unwrap();
        #[cfg(unix)]
        {
            let listed = std::process::Command::new("git")
                .args(list_porcelain_argv(src.to_str().unwrap()))
                .output()
                .unwrap();
            assert!(listed.status.success());
            assert!(listed.stdout.split(|byte| *byte == 0).any(|field| {
                field
                    == [
                        b"worktree ".as_slice(),
                        canonical_target.to_string_lossy().as_bytes(),
                    ]
                    .concat()
            }));
            assert_ne!(
                target, canonical_target,
                "the candidate retains a noncanonical spelling"
            );
        }
        assert_eq!(
            provider.observe_exact_absence(&candidate).unwrap(),
            ExactAbsenceObservationV1::RegisteredButAbsent
        );
        git(&src, &["worktree", "prune"]);
        assert_eq!(
            provider.observe_exact_absence(&candidate).unwrap(),
            ExactAbsenceObservationV1::BothAbsent
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }

    #[tokio::test]
    async fn worktree_add_isolates_and_remove_cleans_up() {
        let tmp = unique_temp_dir("isolation");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["config", "user.email", "a@b.c"]);
        git(&src, &["config", "user.name", "x"]);
        std::fs::write(src.join("file.txt"), "base\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);

        let p = HostGitWorktree::new();
        let src_s = src.to_str().unwrap();
        assert!(p.is_git_repo(src_s).await);
        assert!(
            !p.is_git_repo(tmp.to_str().unwrap()).await,
            "non-repo dir must be false"
        );

        let wt = tmp.join("wt1");
        let wt_s = wt.to_str().unwrap();
        let common_dir = p.add(src_s, wt_s).await.unwrap();
        assert!(!common_dir.is_empty(), "common_dir must be returned");
        let canonical_git = std::fs::canonicalize(src.join(".git")).unwrap();
        assert!(
            common_dir == canonical_git.to_string_lossy()
                || common_dir.ends_with(".git")
                || common_dir.contains(".git"),
            "common_dir should point at source git dir: {common_dir}"
        );

        std::fs::write(wt.join("only-in-wt.txt"), "x").unwrap();
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            status.stdout.is_empty(),
            "source working tree must stay clean: {}",
            String::from_utf8_lossy(&status.stdout)
        );
        assert!(
            !src.join("only-in-wt.txt").exists(),
            "worktree edit must not appear in the source"
        );

        p.remove(src_s, wt_s).await.unwrap();
        p.remove(src_s, wt_s)
            .await
            .expect("removing an already-absent worktree is idempotent");
        let list = std::process::Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&list.stdout).lines().count(),
            1,
            "only the source remains"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[tokio::test]
    async fn worktree_remove_reports_git_failure_while_checkout_remains() {
        let tmp = unique_temp_dir("remove-failure");
        let missing_repo = tmp.join("missing-source");
        let lingering_worktree = tmp.join("lingering-worktree");
        std::fs::create_dir_all(&lingering_worktree).unwrap();

        let result = HostGitWorktree::new()
            .remove(
                missing_repo.to_str().unwrap(),
                lingering_worktree.to_str().unwrap(),
            )
            .await;

        assert!(
            result.is_err(),
            "a real git cleanup failure must fail closed"
        );
        assert!(
            lingering_worktree.exists(),
            "the reported failure leaves the cleanup target available for retry"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[tokio::test]
    async fn unborn_head_add_errors_cleanly() {
        let tmp = unique_temp_dir("unborn");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);

        let p = HostGitWorktree::new();
        let wt = tmp.join("wt");
        let r = p.add(src.to_str().unwrap(), wt.to_str().unwrap()).await;

        assert!(r.is_err(), "unborn HEAD => typed error, not a panic");
        assert!(!wt.exists(), "failed add should clean partial worktree");
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

#[cfg(test)]
mod custody_add_tests {
    use super::*;
    use crate::provider::WorktreeProvider;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-custody-add-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    fn repo(tmp: &Path) -> PathBuf {
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["config", "user.email", "a@b.c"]);
        git(&src, &["config", "user.name", "x"]);
        std::fs::write(src.join("file.txt"), "base\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);
        src
    }

    /// THE routine sequence 2b1's dual review proved is not exotic, run against REAL git.
    ///
    /// The chain: the deletion gate refuses a rollback, so the checkout survives; the
    /// failed-configure loop retries the configure; `git worktree add` now fails because the
    /// target directory is already there. Through `add` that reaches `cleanup_failed_add`'s
    /// `remove_dir_all` — outside the 2b1 gate — and takes a protected checkout with its work.
    /// Through `add_under_custody` nothing may be removed.
    ///
    /// Discriminates any custody-aware add that still calls `cleanup_failed_add`, and one that
    /// reports the surviving directory as provably absent (which the writer would settle as an
    /// unmaterialized candidate rather than preserved work).
    #[tokio::test]
    async fn custody_add_failing_on_a_surviving_directory_never_removes_it() {
        let tmp = unique_temp_dir("surviving-directory");
        let src = repo(&tmp);
        let wt = tmp.join("wt-retry");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("preserved-work.txt"), "hours of work").unwrap();
        let provider = HostGitWorktree::new();

        let outcome = provider
            .add_under_custody(src.to_str().unwrap(), wt.to_str().unwrap())
            .await
            .expect("the host provider implements the custody-aware add");

        let CustodyAddOutcomeV1::Failed(failure) = outcome else {
            panic!("git cannot add a worktree onto a non-empty existing directory")
        };
        assert_eq!(failure.target, CustodyAddTargetV1::Present);
        assert!(
            wt.join("preserved-work.txt").exists(),
            "the surviving checkout and its work must be untouched"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("preserved-work.txt")).unwrap(),
            "hours of work"
        );
        assert!(
            failure.common_dir.is_some(),
            "a readable source still yields its common dir"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The V2 control for the test above, on the same input: `add` really does delete. Without
    /// this, the custody test could pass against a git that simply never fails, and would prove
    /// nothing about the prohibition.
    #[tokio::test]
    async fn the_legacy_add_still_removes_the_same_directory() {
        let tmp = unique_temp_dir("legacy-control");
        let src = repo(&tmp);
        let wt = tmp.join("wt-retry");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("preserved-work.txt"), "hours of work").unwrap();

        let result = HostGitWorktree::new()
            .add(src.to_str().unwrap(), wt.to_str().unwrap())
            .await;

        assert!(result.is_err());
        assert!(
            !wt.exists(),
            "V2's `add` deletes the target on failure — this is the behaviour the custody-aware \
             operation exists to avoid, and it stays unchanged for V2"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// A failure with no target at all classifies `ProvablyAbsent`, so the writer can tell
    /// "nothing was created" from "something was". Discriminates a classifier that reports a
    /// single answer for every failure, which would make every unborn-HEAD add look like
    /// preserved work.
    #[tokio::test]
    async fn a_failure_before_any_target_classifies_provably_absent() {
        let tmp = unique_temp_dir("unborn");
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        let wt = tmp.join("wt");

        let outcome = HostGitWorktree::new()
            .add_under_custody(src.to_str().unwrap(), wt.to_str().unwrap())
            .await
            .unwrap();

        let CustodyAddOutcomeV1::Failed(failure) = outcome else {
            panic!("an unborn HEAD cannot be checked out")
        };
        assert_eq!(failure.target, CustodyAddTargetV1::ProvablyAbsent);
        assert!(!wt.exists());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// An unreadable source makes the registration probe fail, and that `Err` — the ONLY producer
    /// of `RegistrationUnproven` — must reach the record. Discriminates the 2a-forbidden
    /// collapse into `UnregisteredDirectory`, which would durably record an ambiguous probe as a
    /// definite "not registered" answer.
    #[tokio::test]
    async fn an_unprovable_registration_maps_to_registration_unproven() {
        let tmp = unique_temp_dir("unproven-registration");
        std::fs::create_dir_all(&tmp).unwrap();
        let missing_repo = tmp.join("not-a-repo");
        std::fs::create_dir_all(&missing_repo).unwrap();
        let wt = tmp.join("wt");

        let outcome = HostGitWorktree::new()
            .add_under_custody(missing_repo.to_str().unwrap(), wt.to_str().unwrap())
            .await
            .unwrap();

        let CustodyAddOutcomeV1::Failed(failure) = outcome else {
            panic!("a non-repository cannot add a worktree")
        };
        assert_eq!(
            failure.recovery_locator,
            RecoveryLocatorV1::RegistrationUnproven {}
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

#[cfg(test)]
mod capability_removal_tests {
    use super::*;
    use crate::custody_writer::{
        observed_identity, DeletionAuthorizationV1, MaterializedIdentitiesV1, WorktreeCustodianV1,
    };
    use crate::provider::WorktreeProvider;
    use bridge_core::execution_policy::{
        BoundWorktreeCustodyV1, FrozenWorktreeCustodyPlanV1, PolicyNodeRefV1, Sha256HexV1,
        WorktreeCustodyIdV1,
    };
    use bridge_core::ids::{AttemptId, AttemptIdentity, ExecutionId};
    use bridge_core::SessionCwd;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "a2a-bridge-capability-removal-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    fn repo(tmp: &Path) -> PathBuf {
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q"]);
        git(&src, &["config", "user.email", "a@b.c"]);
        git(&src, &["config", "user.name", "x"]);
        std::fs::write(src.join("file.txt"), "base\n").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);
        src
    }

    fn binding(target: &Path) -> BoundWorktreeCustodyV1 {
        let attempt_id = AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap();
        BoundWorktreeCustodyV1 {
            attempt: AttemptIdentity {
                execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                attempt_id: attempt_id.clone(),
                ordinal: 0,
                parent_attempt_id: None,
            },
            origin_attempt_id: attempt_id,
            node: PolicyNodeRefV1::from_node_id(0, "node"),
            plan: FrozenWorktreeCustodyPlanV1 {
                custody_id: WorktreeCustodyIdV1::mint().unwrap(),
                checkout_fingerprint: Sha256HexV1::parse("6".repeat(64)).unwrap(),
                target_cwd: SessionCwd::parse(&target.to_string_lossy()).unwrap(),
            },
        }
    }

    /// Drive a real checkout to `LiveProtected` and mint its capability, exactly as
    /// `materialize_under_custody` + the post-loop settlement do.
    fn authorized_over(
        worktree_root: &Path,
        target: &Path,
        source: &Path,
        common_dir: &Path,
    ) -> (
        WorktreeCustodianV1,
        MaterializedIdentitiesV1,
        crate::custody_writer::AuthorizedRemovalV1,
    ) {
        let custodian =
            WorktreeCustodianV1::enter(worktree_root, &target.to_string_lossy(), binding(target))
                .unwrap();
        custodian.publish_protection_prepared().unwrap();
        custodian.replace_materializing().unwrap();
        let identities = MaterializedIdentitiesV1 {
            source: observed_identity(&source.to_string_lossy()),
            root: observed_identity(&worktree_root.to_string_lossy()),
            worktree: observed_identity(&target.to_string_lossy()),
            common_dir: observed_identity(&common_dir.to_string_lossy()),
        };
        custodian.replace_live_protected(&identities).unwrap();
        let DeletionAuthorizationV1::Authorized(capability) =
            custodian.authorize_deletion(&source.to_string_lossy(), &identities)
        else {
            panic!("a live checkout authorizes its own deletion")
        };
        let authorized = capability
            .revalidate_for_removal()
            .expect("untouched objects revalidate");
        (custodian, identities, authorized)
    }

    /// §5.1's `remove_v2` against REAL git: the capability's own paths drive the removal, the
    /// target and its registration are both gone afterwards, and the tombstone is then legal.
    ///
    /// This is the production half of the capability path — the backend tests drive a double, and
    /// a double cannot show that `git worktree remove` plus the post-conditions actually agree.
    ///
    /// Discriminates a `remove_v2` that removes the directory without pruning the registration:
    /// `registration_absent` would be false and `remove_and_verify` would report the removal
    /// incomplete.
    #[tokio::test]
    async fn capability_removal_removes_a_real_worktree_and_its_registration() {
        let tmp = unique_temp_dir("real-removal");
        let source = repo(&tmp);
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&worktree_root).unwrap();
        let worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let target = worktree_root.join("ownr-run7-real");
        let provider = HostGitWorktree::new();
        let CustodyAddOutcomeV1::Materialized { common_dir } = provider
            .add_under_custody(&source.to_string_lossy(), &target.to_string_lossy())
            .await
            .unwrap()
        else {
            panic!("a clean repo materializes")
        };
        std::fs::write(target.join("work.txt"), "node output").unwrap();

        let (custodian, identities, authorized) =
            authorized_over(&worktree_root, &target, &source, Path::new(&common_dir));
        provider
            .remove_v2(authorized)
            .await
            .expect("a real capability removal completes");

        assert!(!target.exists(), "the checkout is gone");
        let listed = std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["worktree", "list"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&listed.stdout).lines().count(),
            1,
            "and its registration was pruned"
        );
        assert_eq!(
            custodian.record_removed(&identities),
            crate::custody_writer::RemovalRecordV1::Recorded
        );
        drop(custodian);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// P7 boundaries 2, 3 and 4 at the REAL provider: when the git removal does not verifiably
    /// complete, `remove_v2` fails closed, the target survives, and the `Removed` tombstone is
    /// therefore never reachable — the record stays `DeleteAuthorized`, whose sweep disposition is
    /// `Recover`.
    ///
    /// The target here is a plain directory the custody record protects but git does not know
    /// about, so `git worktree remove` fails, the prune succeeds, and the target-absence
    /// post-condition is what refuses. That is the same conjunction a failed prune or a surviving
    /// registration trips (`removal_is_complete` is unit-tested for each independently).
    ///
    /// Discriminates a `remove_v2` that reports the git exit status instead of the descriptor-level
    /// post-conditions — the precise "exit status is never behavioural evidence" failure.
    #[tokio::test]
    async fn a_removal_that_leaves_the_target_fails_closed_and_forbids_the_tombstone() {
        let tmp = unique_temp_dir("failed-removal");
        let source = repo(&tmp);
        let worktree_root = tmp.join("worktrees");
        std::fs::create_dir_all(&worktree_root).unwrap();
        let worktree_root = std::fs::canonicalize(&worktree_root).unwrap();
        let target = worktree_root.join("ownr-run7-unregistered");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("work.txt"), "hours of work").unwrap();
        let common_dir = std::fs::canonicalize(source.join(".git")).unwrap();

        let (custodian, identities, authorized) =
            authorized_over(&worktree_root, &target, &source, &common_dir);
        let result = HostGitWorktree::new().remove_v2(authorized).await;

        assert!(
            result.is_err(),
            "a removal whose target survives must fail closed"
        );
        assert!(target.join("work.txt").exists(), "the work is untouched");
        let tombstone = custodian.record_removed(&identities);
        assert_eq!(
            tombstone,
            crate::custody_writer::RemovalRecordV1::Recorded,
            "the writer will record a tombstone if asked — which is exactly why the CALLER must \
             not ask after a failed removal; that ordering is what `authorize_and_remove_checkout` \
             enforces and `a_failed_capability_removal_never_records_removed` pins"
        );
        drop(custodian);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
