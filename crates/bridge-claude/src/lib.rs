//! Warm Claude Code (`claude` stream-json) agent backend for the A2A bridge.
pub mod backend;
pub mod config;
pub mod proc;
pub mod wire;

pub use backend::ClaudeCliBackend;
pub use config::ClaudeConfig;
