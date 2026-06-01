//! Warm Claude Code (`claude` stream-json) agent backend for the A2A bridge.
pub mod config;
pub mod proc;
pub mod wire;
// pub mod backend; // Task 8

pub use config::ClaudeConfig;
