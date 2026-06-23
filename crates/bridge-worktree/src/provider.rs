use bridge_core::error::BridgeError;

/// Materializes/removes per-session git worktrees. Host impl shells out to git; tests use a fake.
#[async_trait::async_trait]
pub trait WorktreeProvider: Send + Sync {
    /// Create a detached worktree of `repo` at `worktree_path` (base ref = repo HEAD).
    async fn add(&self, repo: &str, worktree_path: &str) -> Result<(), BridgeError>;

    /// Remove the worktree + prune the source's dangling registration. Best-effort.
    async fn remove(&self, repo: &str, worktree_path: &str) -> Result<(), BridgeError>;

    /// True if `path` is inside a git work tree.
    async fn is_git_repo(&self, path: &str) -> bool;
}

#[allow(dead_code)]
pub(crate) fn add_argv<'a>(repo: &'a str, wt: &'a str, base: &'a str) -> Vec<&'a str> {
    vec!["-C", repo, "worktree", "add", "--detach", wt, base]
}

#[allow(dead_code)]
pub(crate) fn remove_argv<'a>(repo: &'a str, wt: &'a str) -> Vec<&'a str> {
    vec!["-C", repo, "worktree", "remove", "--force", wt]
}

#[allow(dead_code)]
pub(crate) fn is_repo_argv(path: &str) -> Vec<&str> {
    vec!["-C", path, "rev-parse", "--is-inside-work-tree"]
}

#[allow(dead_code)]
pub(crate) fn prune_argv(repo: &str) -> Vec<&str> {
    vec!["-C", repo, "worktree", "prune"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_builders_emit_expected_git_invocations() {
        assert_eq!(
            add_argv("/repo", "/wt/x", "HEAD"),
            vec!["-C", "/repo", "worktree", "add", "--detach", "/wt/x", "HEAD"]
        );
        assert_eq!(
            remove_argv("/repo", "/wt/x"),
            vec!["-C", "/repo", "worktree", "remove", "--force", "/wt/x"]
        );
        assert_eq!(
            is_repo_argv("/some/dir"),
            vec!["-C", "/some/dir", "rev-parse", "--is-inside-work-tree"]
        );
        assert_eq!(
            prune_argv("/repo"),
            vec!["-C", "/repo", "worktree", "prune"]
        );
    }
}
