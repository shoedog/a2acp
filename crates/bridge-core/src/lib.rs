//! bridge-core — domain core: Task/Session state machines, port traits, error model.

pub mod attempt_activity;
pub mod attestation;
pub mod brief_lint;
pub mod catalog;
pub mod diagnostics;
pub mod domain;
pub mod error;
pub mod execution_policy;
pub mod failure_wire;
pub mod harvest;
pub mod ids;
#[cfg(unix)]
pub mod liveness;
pub mod mcp;
pub mod orch;
pub mod permission;
pub mod ports;
#[cfg(unix)]
pub mod process;
pub mod profile;
pub mod provider;
#[cfg(unix)]
pub mod reaper;
#[cfg(unix)]
pub mod run_identity;
pub mod sandbox;
pub mod session;
pub mod session_cwd;
pub mod session_fingerprint;
pub mod task;
pub mod task_spec;
pub mod task_store;
pub mod terminal_evidence;
pub mod translator;
pub mod workflow_history;

pub use profile::{rust_profile, CacheBinding, CacheCtx, LanguageProfile};
pub use session_cwd::SessionCwd;
