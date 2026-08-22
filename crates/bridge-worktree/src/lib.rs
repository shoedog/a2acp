//! Worktree-per-session isolation: a WorktreeBackend decorator + a host-git WorktreeProvider.

pub mod backend;
pub mod custody;
pub mod custody_lock;
pub mod custody_writer;
pub mod host_git;
pub mod provider;
pub mod provider_path;
pub mod settle;
pub mod sweep;
pub mod workflow_planner;
