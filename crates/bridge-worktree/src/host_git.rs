use crate::provider::{add_argv, is_repo_argv, prune_argv, remove_argv, WorktreeProvider};
use bridge_core::error::BridgeError;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

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

fn run_git(argv: &[&str]) -> Result<Output, BridgeError> {
    Command::new("git")
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

fn cleanup_failed_add(repo: &str, wt: &str) {
    let _ = std::fs::remove_dir_all(wt);
    let _ = run_git(&prune_argv(repo));
}

fn common_dir(repo: &str) -> String {
    let absolute = run_git(&[
        "-C",
        repo,
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ]);
    if let Ok(out) = absolute {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
    }

    let fallback = run_git(&["-C", repo, "rev-parse", "--git-common-dir"]);
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

#[async_trait::async_trait]
impl WorktreeProvider for HostGitWorktree {
    async fn add(&self, repo: &str, wt: &str) -> Result<String, BridgeError> {
        let mut last_err = String::new();
        for _ in 0..5 {
            let out = run_git(&add_argv(repo, wt, "HEAD"))?;
            if out.status.success() {
                return Ok(common_dir(repo));
            }

            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if !retryable_lock_error(&err) {
                cleanup_failed_add(repo, wt);
                return Err(BridgeError::ConfigInvalid {
                    reason: format!("worktree add failed: {err}"),
                });
            }

            last_err = err;
            std::thread::sleep(Duration::from_millis(200));
        }

        cleanup_failed_add(repo, wt);
        Err(BridgeError::ConfigInvalid {
            reason: format!("worktree add failed after lock retries: {last_err}"),
        })
    }

    async fn remove(&self, repo: &str, wt: &str) -> Result<(), BridgeError> {
        let _ = run_git(&remove_argv(repo, wt));
        let _ = run_git(&prune_argv(repo));
        Ok(())
    }

    async fn is_git_repo(&self, path: &str) -> bool {
        matches!(run_git(&is_repo_argv(path)), Ok(out) if out.status.success()
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
}
