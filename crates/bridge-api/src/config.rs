//! Configuration for ApiBackend. `model`/`api_key_env` are NOT frozen here — the
//! backend resolves the key per-prompt (env) and the model per-session (stash);
//! `ApiConfig` holds only the construction-time defaults.
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// OpenAI-compatible base, e.g. "http://localhost:11434/v1". The backend POSTs
    /// to `{base_url}/chat/completions`.
    pub base_url: String,
    /// Default request model id; per-session `configure_session` may override it.
    pub model: Option<String>,
    /// NAME of an env var holding a bearer token (never the secret). Read per-prompt.
    pub api_key_env: Option<String>,
    /// Bounds the tool loop — no infinite tool_call cycles.
    pub max_tool_rounds: usize,
    pub request_timeout: Duration,
    /// Use SSE streaming (default). `false` uses the non-streamed `message.tool_calls` shape.
    pub stream: bool,
}

impl ApiConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: None,
            api_key_env: None,
            max_tool_rounds: 4,
            request_timeout: Duration::from_secs(120),
            stream: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_sane() {
        let c = ApiConfig::new("http://localhost:11434/v1");
        assert_eq!(c.base_url, "http://localhost:11434/v1");
        assert_eq!(c.max_tool_rounds, 4);
        assert!(c.stream);
        assert_eq!(c.request_timeout, std::time::Duration::from_secs(120));
        assert!(c.model.is_none() && c.api_key_env.is_none());
    }
}
