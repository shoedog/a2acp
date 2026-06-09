//! bridge-core — domain core: Task/Session state machines, port traits, error model.

pub mod domain;
pub mod error;
pub mod ids;
pub mod liveness;
pub mod mcp;
pub mod ports;
pub mod process;
pub mod reaper;
pub mod run_identity;
pub mod sandbox;
pub mod session;
pub mod session_cwd;
pub mod task;
pub mod task_store;
pub mod translator;

pub use session_cwd::SessionCwd;
