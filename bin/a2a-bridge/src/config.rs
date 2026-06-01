// config.rs — TOML configuration for the a2a-bridge binary (spec §8, Task 15).

use std::collections::BTreeMap;
use std::fmt;

use bridge_core::domain::{AgentEntry, AgentKind, Effort, RegistrySnapshot};
use bridge_core::ids::AgentId;

/// Unified parse error covering TOML parse failures and missing env-var references.
#[derive(Debug)]
pub enum ConfigError {
    Toml(toml::de::Error),
    MissingEnvVar(String),
    /// Invalid registry config value (e.g. unknown effort level, empty agent id).
    /// Wired to main in Task 12.
    Registry(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Toml(e) => write!(f, "TOML parse error: {e}"),
            ConfigError::MissingEnvVar(v) => write!(f, "env var ${{{v}}} not set"),
            ConfigError::Registry(msg) => write!(f, "registry config: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Toml(e) => Some(e),
            ConfigError::MissingEnvVar(_) | ConfigError::Registry(_) => None,
        }
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Toml(e)
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct DelegationConfig {
    pub peer_url: String,
    pub auth: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_addr() -> String {
    "127.0.0.1:8080".into()
}

fn default_timeout_secs() -> u64 {
    60
}

/// Expand `${VAR_NAME}` placeholders in `s` using `std::env::var`.
/// Returns `Err(ConfigError::MissingEnvVar)` if any referenced variable is unset.
fn expand_env(s: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after_brace = &rest[start + 2..];
        let end = after_brace
            .find('}')
            .ok_or_else(|| ConfigError::MissingEnvVar("<unterminated ${...}>".into()))?;
        let var_name = &after_brace[..end];
        let value = std::env::var(var_name)
            .map_err(|_| ConfigError::MissingEnvVar(var_name.to_string()))?;
        result.push_str(&value);
        rest = &after_brace[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}

// ---------------------------------------------------------------------------
// Multi-agent registry config (Task 7 / Increment 3b).
// Parses a TOML with `[[agents]]` array + optional `[registry]` section.
// Main is rewired to use this in Task 12.
// ---------------------------------------------------------------------------

/// Top-level TOML structure for the multi-agent bridge config.
#[derive(Debug, serde::Deserialize)]
pub struct RegistryConfig {
    pub default: String,
    #[serde(default)]
    pub registry: Option<RegistrySection>,
    #[serde(default)]
    pub agents: Vec<AgentEntryToml>,
    pub server: ServerConfig,
    #[serde(default)]
    pub delegation: Option<DelegationConfig>,
}

/// `[registry]` section — optional; controls which cmds are allowed.
#[derive(Debug, serde::Deserialize)]
pub struct RegistrySection {
    #[serde(default)]
    pub allowed_cmds: Vec<String>,
}

/// One entry in the `[[agents]]` array, as parsed from TOML.
/// String fields are converted to typed domain values in `into_snapshot`.
#[derive(Debug, serde::Deserialize)]
pub struct AgentEntryToml {
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Parsed to `AgentKind` in `into_snapshot`; "acp" (default) | "claude-cli".
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub model_provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Parsed to `Effort` in `into_snapshot`; valid values: minimal/low/medium/high/max.
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub auth_method: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub extensions: BTreeMap<String, toml::Value>,
}

impl RegistryConfig {
    /// Parse a multi-agent TOML string into a `RegistryConfig`.
    /// TOML deserialization plus env-expansion of the `[delegation]` `peer_url`/`auth`
    /// strings (so a `${PEER_TOKEN}`-style secret is resolved from the environment,
    /// matching the inbound-server expectation that the auth header is already concrete).
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        let mut cfg: RegistryConfig = toml::from_str(s)?;
        if let Some(d) = cfg.delegation.as_mut() {
            d.peer_url = expand_env(&d.peer_url)?;
            d.auth = expand_env(&d.auth)?;
        }
        Ok(cfg)
    }

    /// Convert this parsed config into a `RegistrySnapshot` with typed domain values.
    pub fn into_snapshot(self) -> Result<RegistrySnapshot, ConfigError> {
        // `allowed_cmds`: use the explicit list if provided; otherwise default to the
        // union of all entry cmds (so every entry is trivially allowed).
        let allowed_cmds = match self.registry {
            Some(r) if !r.allowed_cmds.is_empty() => r.allowed_cmds,
            _ => {
                let mut v: Vec<String> = self.agents.iter().map(|a| a.cmd.clone()).collect();
                v.sort();
                v.dedup();
                v
            }
        };

        let mut entries = Vec::with_capacity(self.agents.len());
        for a in self.agents {
            let id = AgentId::parse(a.id).map_err(|e| ConfigError::Registry(e.to_string()))?;
            let effort = a.effort.as_deref().map(parse_effort).transpose()?;
            let kind = match a.kind.as_deref() {
                Some(s) => parse_kind(s)?,
                None => AgentKind::default(),
            };
            entries.push(AgentEntry {
                id,
                cmd: a.cmd,
                args: a.args,
                kind,
                model_provider: a.model_provider,
                model: a.model,
                effort,
                mode: a.mode,
                cwd: a.cwd,
                auth_method: a.auth_method,
                name: a.name,
                description: a.description,
                tags: a.tags,
                version: a.version,
                extensions: a.extensions,
            });
        }

        let default =
            AgentId::parse(self.default).map_err(|e| ConfigError::Registry(e.to_string()))?;

        Ok(RegistrySnapshot {
            default,
            entries,
            allowed_cmds,
        })
    }
}

/// Parse an effort-level string into the `Effort` enum.
/// Valid inputs (case-sensitive): "minimal", "low", "medium", "high", "max".
fn parse_effort(s: &str) -> Result<Effort, ConfigError> {
    Ok(match s {
        "minimal" => Effort::Minimal,
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "max" => Effort::Max,
        other => {
            return Err(ConfigError::Registry(format!(
                "invalid effort: {other:?} (expected minimal/low/medium/high/max)"
            )))
        }
    })
}

/// Parse the adapter-kind string into `AgentKind`. None → Acp (back-compat).
fn parse_kind(s: &str) -> Result<AgentKind, ConfigError> {
    Ok(match s {
        "acp" => AgentKind::Acp,
        "claude-cli" => AgentKind::ClaudeCli,
        other => {
            return Err(ConfigError::Registry(format!(
                "invalid kind: {other:?} (expected acp/claude-cli)"
            )))
        }
    })
}

/// Read a `u64` extension value (TOML integer), if present and valid.
pub fn ext_u64(ext: &std::collections::BTreeMap<String, toml::Value>, key: &str) -> Option<u64> {
    ext.get(key)
        .and_then(|v| v.as_integer())
        .and_then(|i| u64::try_from(i).ok())
}

/// Read a `usize` extension value.
pub fn ext_usize(
    ext: &std::collections::BTreeMap<String, toml::Value>,
    key: &str,
) -> Option<usize> {
    ext_u64(ext, key).and_then(|n| usize::try_from(n).ok())
}

// ---------------------------------------------------------------------------
// FileConfigSource — the File `ConfigSource` adapter (Task 8 / Increment 3b).
//
// `load()` reads + parses the TOML at `path` into a `RegistrySnapshot` (via the
// Task-7 `RegistryConfig::parse` → `into_snapshot` pipeline). `watch()` returns a
// stream that fires whenever the file changes on disk.
//
// The four must-haves for a robust file watch:
//   (a) PARENT-DIR watch — editors save by atomic-rename (write `.tmp`, rename over
//       the target), which gives the file a NEW inode; a file-inode watch goes stale
//       and silently misses the edit. Watching the parent directory survives this.
//   (b) DEBOUNCE — one logical save can emit several fs events; we coalesce a burst
//       into a single re-load with a short settle window.
//   (c) WATCHER KEPT ALIVE — `notify::RecommendedWatcher` stops delivering events the
//       moment it is dropped, so it MUST be moved into (and live for the whole life of)
//       the spawned task.
//   (d) KEEP-LAST-GOOD — a transient parse failure (e.g. a half-written file) MUST NOT
//       tear the stream down; we log and skip emitting, leaving the consumer on the
//       last good snapshot.
// ---------------------------------------------------------------------------

/// File-backed [`ConfigSource`](bridge_core::ports::ConfigSource): loads a
/// `RegistrySnapshot` from a TOML file and watches its parent directory for edits.
pub struct FileConfigSource {
    path: std::path::PathBuf,
}

impl FileConfigSource {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Read + parse the TOML at `path` into a `RegistrySnapshot`. Shared by `load()`
    /// and the watch task's re-load. `None` on read/parse failure (so the watch task
    /// can keep-last-good); `load()` maps the failure to a `BridgeError` instead.
    async fn try_load(path: &std::path::Path) -> Option<RegistrySnapshot> {
        let s = tokio::fs::read_to_string(path).await.ok()?;
        RegistryConfig::parse(&s)
            .and_then(|c| c.into_snapshot())
            .ok()
    }
}

#[async_trait::async_trait]
impl bridge_core::ports::ConfigSource for FileConfigSource {
    async fn load(&self) -> Result<RegistrySnapshot, bridge_core::error::BridgeError> {
        let s = tokio::fs::read_to_string(&self.path).await.map_err(|e| {
            bridge_core::error::BridgeError::ConfigInvalid {
                reason: format!("read {}: {e}", self.path.display()),
            }
        })?;
        RegistryConfig::parse(&s)
            .and_then(|c| c.into_snapshot())
            .map_err(|e| bridge_core::error::BridgeError::ConfigInvalid {
                reason: e.to_string(),
            })
    }

    fn watch(&self) -> futures::stream::BoxStream<'static, RegistrySnapshot> {
        let path = self.path.clone();
        // (a) Watch the PARENT directory, not the file inode — atomic-rename saves
        // replace the inode, which a file-watch would miss after the first event.
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        // The filename we re-load on any relevant directory event.
        let file_name = path.file_name().map(|n| n.to_os_string());

        let (tx, rx) = tokio::sync::mpsc::channel::<RegistrySnapshot>(8);

        // notify's callback runs on its own thread; bridge its events to async land
        // over an unbounded channel of "something changed" signals.
        let (raw_tx, mut raw_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let filter_name = file_name.clone();
        // Create + REGISTER the watcher SYNCHRONOUSLY, before this function returns.
        // Registering inside the spawned task would race the caller: a `watch()`-then-
        // edit sequence could fire the (single) edit before the watcher is live and miss
        // it forever. Events that land before the loop starts are buffered in `raw_rx`.
        let watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    // Filter to events touching OUR file (by filename) when notify gives us
                    // paths; if it gives none, treat it as a coarse signal and re-check by
                    // path below. Robust to atomic-rename, which reports the target path in
                    // the rename's `paths`.
                    let relevant = match &filter_name {
                        Some(name) => {
                            ev.paths.is_empty()
                                || ev
                                    .paths
                                    .iter()
                                    .any(|p| p.file_name() == Some(name.as_os_str()))
                        }
                        None => true,
                    };
                    if relevant {
                        let _ = raw_tx.send(());
                    }
                }
            }) {
                Ok(w) => Some(w),
                Err(e) => {
                    tracing::warn!(error = %e, "config watcher init failed; watch disabled");
                    None
                }
            };
        let watcher = watcher.and_then(|mut w| {
            use notify::Watcher;
            match w.watch(&parent, notify::RecursiveMode::NonRecursive) {
                Ok(()) => Some(w),
                Err(e) => {
                    tracing::warn!(dir = %parent.display(), error = %e, "config watch failed; watch disabled");
                    None
                }
            }
        });

        tokio::spawn(async move {
            // (c) Keep the watcher alive for the whole task — `notify` stops delivering
            // events the instant it is dropped. `None` = init failed; the loop below then
            // idles until the receiver is dropped.
            let _watcher = watcher;

            loop {
                // Block until at least one change signal arrives.
                if raw_rx.recv().await.is_none() {
                    break; // watcher dropped (only happens at task end) → stop.
                }
                // (b) Debounce: let a burst of events for one logical save settle, then
                // drain the backlog so we re-load exactly once per settled edit.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                while raw_rx.try_recv().is_ok() {}

                // Re-load by PATH (not inode) so we pick up the freshly-renamed file.
                match Self::try_load(&path).await {
                    // (d) Keep-last-good: only emit on a successful parse.
                    Some(snap) => {
                        if tx.send(snap).await.is_err() {
                            break; // (e) receiver dropped → stop the task.
                        }
                    }
                    None => {
                        tracing::warn!(
                            path = %path.display(),
                            "config reload failed; keeping last-good"
                        );
                    }
                }
            }

            // (c) `_watcher` lived for the whole task; drop it explicitly here.
            drop(_watcher);
        });

        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_parsed_with_env_expansion() {
        std::env::set_var("PEER_TOKEN_T10", "sek");
        let c = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n\
             [delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${PEER_TOKEN_T10}\"\n",
        )
        .unwrap();
        let d = c.delegation.unwrap();
        assert_eq!(d.peer_url, "http://p");
        assert_eq!(d.auth, "bearer:sek");
    }

    #[test]
    fn config_without_delegation_still_valid() {
        let c = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n",
        )
        .unwrap();
        assert!(c.delegation.is_none());
        assert_eq!(c.server.addr, "127.0.0.1:8080");
    }

    #[test]
    fn missing_env_var_errors() {
        let r = RegistryConfig::parse(
            "default=\"k\"\n[[agents]]\nid=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n\
             [delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${DEFINITELY_UNSET_VAR_XYZ}\"\n",
        );
        assert!(matches!(r, Err(ConfigError::MissingEnvVar(_))));
    }

    // -----------------------------------------------------------------------
    // RegistryConfig / RegistrySnapshot tests (Task 7 / Increment 3b)
    // -----------------------------------------------------------------------

    #[test]
    fn parses_agents_and_default() {
        let toml = r#"
default = "codex"

[registry]
allowed_cmds = ["codex-acp", "kiro-cli"]

[[agents]]
id = "codex"
cmd = "codex-acp"
model = "gpt-5.5"
effort = "high"
mode = "read-only"

[[agents]]
id = "kiro"
cmd = "kiro-cli"
args = ["acp"]
model = "auto"

[server]
addr = "127.0.0.1:8080"
"#;
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.default.as_str(), "codex");
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.allowed_cmds.contains(&"kiro-cli".to_string()));
        let codex = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "codex")
            .unwrap();
        assert_eq!(codex.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(codex.effort, Some(bridge_core::domain::Effort::High));
        let kiro = snap
            .entries
            .iter()
            .find(|e| e.id.as_str() == "kiro")
            .unwrap();
        assert_eq!(kiro.args, vec!["acp".to_string()]);
    }

    #[test]
    fn allowed_cmds_defaults_to_entry_cmds_when_absent() {
        // A TOML with NO [registry] section → allowed_cmds defaults to the set of entry cmds.
        let toml = r#"
default = "alpha"

[[agents]]
id = "alpha"
cmd = "alpha-cli"

[[agents]]
id = "beta"
cmd = "beta-cli"

[server]
"#;
        let snap = RegistryConfig::parse(toml)
            .unwrap()
            .into_snapshot()
            .unwrap();
        // Both cmds should be in allowed_cmds (sorted + deduped).
        assert!(snap.allowed_cmds.contains(&"alpha-cli".to_string()));
        assert!(snap.allowed_cmds.contains(&"beta-cli".to_string()));
        assert_eq!(snap.allowed_cmds.len(), 2);
    }

    #[test]
    fn effort_parses_all_levels_and_rejects_invalid() {
        // All valid levels round-trip.
        for (s, expected) in [
            ("minimal", bridge_core::domain::Effort::Minimal),
            ("low", bridge_core::domain::Effort::Low),
            ("medium", bridge_core::domain::Effort::Medium),
            ("high", bridge_core::domain::Effort::High),
            ("max", bridge_core::domain::Effort::Max),
        ] {
            assert_eq!(parse_effort(s).unwrap(), expected, "failed for {s:?}");
        }
        // Invalid value → Err(ConfigError::Registry).
        let err = parse_effort("bogus").unwrap_err();
        assert!(
            matches!(err, ConfigError::Registry(_)),
            "expected Registry variant, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // FileConfigSource tests (Task 8 / Increment 3b)
    // -----------------------------------------------------------------------

    const V1_STRING: &str = r#"default="codex"
[registry]
allowed_cmds=["codex-acp"]
[[agents]]
id="codex"
cmd="codex-acp"
[server]
addr="127.0.0.1:8080"
"#;

    // A self-consistent v2: default="kiro", one agent id="kiro"/cmd="kiro-cli",
    // allowed_cmds=["kiro-cli"].
    const V2_STRING: &str = r#"default="kiro"
[registry]
allowed_cmds=["kiro-cli"]
[[agents]]
id="kiro"
cmd="kiro-cli"
args=["acp"]
[server]
addr="127.0.0.1:8080"
"#;

    #[tokio::test]
    async fn load_parses_via_into_snapshot() {
        use bridge_core::ports::ConfigSource;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();
        let src = FileConfigSource::new(path.clone());
        let snap = src.load().await.unwrap();
        assert_eq!(snap.default.as_str(), "codex");
        assert_eq!(snap.entries.len(), 1);
    }

    #[tokio::test]
    async fn load_errors_on_missing_file() {
        use bridge_core::ports::ConfigSource;
        let dir = tempfile::tempdir().unwrap();
        let src = FileConfigSource::new(dir.path().join("does-not-exist.toml"));
        let err = src.load().await.unwrap_err();
        assert!(
            matches!(err, bridge_core::error::BridgeError::ConfigInvalid { .. }),
            "expected ConfigInvalid, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn watch_emits_on_edit_via_atomic_rename() {
        use bridge_core::ports::ConfigSource;
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();

        let src = FileConfigSource::new(path.clone());
        // load() returns v1.
        assert_eq!(src.load().await.unwrap().default.as_str(), "codex");

        // Start watching, then ATOMICALLY RENAME a v2 over the file (editor-style
        // save → new inode — the footgun a file-inode watch silently misses).
        let mut stream = src.watch();
        let tmp = dir.path().join(".a2a-bridge.toml.tmp");
        std::fs::write(&tmp, V2_STRING).unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // A snapshot with default "kiro" must arrive within the timeout. The window
        // is generous (200ms debounce + fs-event latency) to stay non-flaky.
        let snap = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("watch must emit within 5s")
            .expect("stream not ended");
        assert_eq!(snap.default.as_str(), "kiro");
    }

    #[tokio::test]
    async fn watch_keeps_last_good_on_parse_error() {
        use bridge_core::ports::ConfigSource;
        use futures::StreamExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a2a-bridge.toml");
        std::fs::write(&path, V1_STRING).unwrap();

        let src = FileConfigSource::new(path.clone());
        let mut stream = src.watch();

        // First write GARBAGE (parse fails) — must NOT emit, must NOT tear down.
        let tmp = dir.path().join(".garbage.tmp");
        std::fs::write(&tmp, "this is not valid toml = = =").unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        // Then write a valid v2 — the stream survives and emits the good snapshot.
        let tmp2 = dir.path().join(".v2.tmp");
        std::fs::write(&tmp2, V2_STRING).unwrap();
        std::fs::rename(&tmp2, &path).unwrap();

        let snap = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("watch must still emit after a transient parse error")
            .expect("stream not ended");
        assert_eq!(snap.default.as_str(), "kiro");
    }

    // -----------------------------------------------------------------------
    // Task 13: kind parse + warm-pool extension getters
    // -----------------------------------------------------------------------

    #[test]
    fn kind_parses_and_defaults_to_acp() {
        // RegistryConfig::parse is the real entry point; `[server]` is required (it has
        // no #[serde(default)]). Mirrors the existing config.rs test style.
        let snap = RegistryConfig::parse(
            "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"claude\"\nkind=\"claude-cli\"\n\
             [[agents]]\nid=\"k\"\ncmd=\"kiro-cli\"\n[server]\n",
        )
        .unwrap()
        .into_snapshot()
        .unwrap();
        let c = snap.entries.iter().find(|e| e.id.as_str() == "c").unwrap();
        let k = snap.entries.iter().find(|e| e.id.as_str() == "k").unwrap();
        assert_eq!(c.kind, bridge_core::domain::AgentKind::ClaudeCli);
        assert_eq!(k.kind, bridge_core::domain::AgentKind::Acp); // default
    }

    #[test]
    fn invalid_kind_is_config_error() {
        let r = RegistryConfig::parse(
            "default=\"c\"\n[[agents]]\nid=\"c\"\ncmd=\"claude\"\nkind=\"nope\"\n[server]\n",
        )
        .unwrap()
        .into_snapshot();
        assert!(r.is_err());
    }
}
