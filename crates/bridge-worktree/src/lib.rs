//! Worktree-per-session isolation: a WorktreeBackend decorator + a host-git WorktreeProvider.

pub mod backend;
pub mod host_git;
pub mod provider;
pub mod sweep;
