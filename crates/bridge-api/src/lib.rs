//! bridge-api — a non-process, OpenAI-compatible HTTP AgentBackend (kind="api").
//! See docs/superpowers/specs/2026-06-01-a2a-bridge-api-backend-design.md.
pub mod backend;
pub mod config;
mod provider;
pub mod tool;
pub mod wire;

pub use backend::ApiBackend;
pub use config::ApiConfig;
