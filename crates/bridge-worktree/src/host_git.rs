use crate::custody::RecoveryLocatorV1;
use crate::provider::{
    add_argv, is_repo_argv, list_porcelain_argv, prune_argv, remove_argv, CustodyAddFailureV1,
    CustodyAddOutcomeV1, CustodyAddTargetV1, WorktreeProvider,
};
use bridge_core::error::BridgeError;
use std::path::Path;
use std::process::Output;
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

fn target_absent_from_probe(probe: std::io::Result<bool>) -> Result<bool, BridgeError> {
    probe
        .map(|exists| !exists)
        .map_err(|error| BridgeError::ConfigInvalid {
            reason: format!("worktree target metadata failed: {error}"),
        })
}

fn removal_is_complete(
    prune_succeeded: bool,
    target_absent: bool,
    registration_absent: bool,
) -> bool {
    prune_succeeded && target_absent && registration_absent
}

fn registration_absent_from_porcelain(output: &[u8], wt: &str) -> bool {
    let target = wt.as_bytes();
    !output.split(|byte| *byte == 0).any(|field| {
        field
            .strip_prefix(b"worktree ")
            .is_some_and(|path| path == target)
    })
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
    Ok(registration_absent_from_porcelain(&listed.stdout, wt))
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
    let target = match Path::new(wt).try_exists() {
        Ok(true) => CustodyAddTargetV1::Present,
        Ok(false) => CustodyAddTargetV1::ProvablyAbsent,
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
        let remove = run_git(&remove_argv(repo, wt)).await?;
        let prune = run_git(&prune_argv(repo)).await?;
        let target_absent = target_absent_from_probe(Path::new(wt).try_exists())?;
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

    #[test]
    fn cleanup_success_requires_absent_target_registration_and_successful_prune() {
        assert!(
            removal_is_complete(true, true, true),
            "a repeated remove is idempotent only after exact absence is proved"
        );
        assert!(!removal_is_complete(false, true, true));
        assert!(!removal_is_complete(true, false, true));
        assert!(!removal_is_complete(true, true, false));
        assert!(target_absent_from_probe(Ok(false)).unwrap());
        assert!(target_absent_from_probe(Ok(true)).is_ok_and(|absent| !absent));
        assert!(target_absent_from_probe(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        )))
        .is_err());
    }

    #[test]
    fn porcelain_registration_check_is_exact_and_handles_locked_records() {
        let output =
            b"worktree /repo\0HEAD abc\0\0worktree /managed/wt\0HEAD def\0locked reason\0\0";
        assert!(!registration_absent_from_porcelain(output, "/managed/wt"));
        assert!(registration_absent_from_porcelain(output, "/managed/other"));
        assert!(registration_absent_from_porcelain(output, "/managed/w"));
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
