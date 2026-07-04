use std::path::PathBuf;

/// A workflow run pinned to one repository via `session_cwd`. Every relative
/// path from the task-spec must resolve UNDER this base, never against the
/// bridge process's own launch directory.
pub struct RunContext {
    pub session_cwd: PathBuf,
}

impl RunContext {
    pub fn new(session_cwd: impl Into<PathBuf>) -> Self {
        Self {
            session_cwd: session_cwd.into(),
        }
    }

    /// Resolve a relative artifact path from the task-spec to an absolute path
    /// the agent will read/write.
    pub fn resolve(&self, rel: &str) -> PathBuf {
        let base = std::env::current_dir().unwrap_or_default();
        base.join(rel)
    }
}
