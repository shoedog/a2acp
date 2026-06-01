//! Static config for `ClaudeCliBackend`: spawn args + bounded timeouts + warm-pool params.
use std::path::PathBuf;
use std::time::Duration;

pub const DEFAULT_INIT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_TURN_TIMEOUT: Duration = Duration::from_secs(600);
pub const DEFAULT_CANCEL_GRACE: Duration = Duration::from_secs(5);
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(3300); // 55 min ≈ Max prompt-cache window (§3.3)
pub const DEFAULT_MAX_WARM: usize = 16;
pub const DEFAULT_MAX_SESSIONS: usize = 64;
pub const DEFAULT_REAPER_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub extra_args: Vec<String>, // perm/trust flags + entry.args
    pub init_timeout: Duration,  // retained; init is now captured lazily during the first turn

    pub turn_timeout: Duration,
    pub cancel_grace: Duration,
    pub idle_ttl: Duration,
    pub max_warm: usize,
    pub max_sessions: usize,
    pub reaper_interval: Duration,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            model: None,
            extra_args: Vec::new(),
            init_timeout: DEFAULT_INIT_TIMEOUT,
            turn_timeout: DEFAULT_TURN_TIMEOUT,
            cancel_grace: DEFAULT_CANCEL_GRACE,
            idle_ttl: DEFAULT_IDLE_TTL,
            max_warm: DEFAULT_MAX_WARM,
            max_sessions: DEFAULT_MAX_SESSIONS,
            reaper_interval: DEFAULT_REAPER_INTERVAL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_match_spec() {
        let c = ClaudeConfig::default();
        assert_eq!(c.idle_ttl, Duration::from_secs(3300));
        assert_eq!(c.max_warm, 16);
        assert_eq!(c.max_sessions, 64);
    }
}
