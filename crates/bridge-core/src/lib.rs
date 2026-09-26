//! bridge-core — domain core: Task/Session state machines, port traits, error model.

pub mod attempt_activity;
pub mod attestation;
pub mod brief_lint;
pub mod catalog;
pub mod custody_capsule;
// The exporter's entry point is crate-private and, by design, production-unreachable: the capture
// capability has only a crate-private fixture constructor and the only sealer is the in-crate
// fixture. The wiring slice supplies the production mint and the Git route pin.
#[cfg(unix)]
#[allow(dead_code)]
mod custody_export;
#[cfg(unix)]
#[allow(dead_code)] // 2B2a is intentionally production-unwired until the exporter slice.
mod custody_git;
#[cfg(all(test, unix))]
mod custody_git_tests;
pub mod custody_inventory;
pub mod custody_inventory_collector;
pub mod custody_seal;
pub mod diagnostics;
pub mod domain;
pub mod error;
pub mod execution_policy;
pub mod failure_wire;
pub mod fixed_grace_timer;
pub mod fs_custody;
pub mod graph;
pub mod harvest;
pub mod ids;
#[cfg(unix)]
pub mod liveness;
pub mod mcp;
pub mod mechanical_impossibility;
#[cfg(unix)]
pub mod namespace_transaction;
pub mod no_progress_warning;
pub mod orch;
pub mod permission;
pub mod ports;
pub mod preparation_flight;
#[cfg(unix)]
pub mod process;
pub mod profile;
pub mod provider;
#[cfg(unix)]
pub mod reaper;
#[cfg(unix)]
pub mod remote_request_flight;
pub mod resource_flight;
pub mod retained_resource_flight;
#[cfg(unix)]
pub mod run_identity;
pub mod run_spec;
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
