// config.rs — TOML configuration for the a2a-bridge binary (spec §8, Task 15).

use std::collections::BTreeMap;
use std::fmt;

use bridge_core::domain::{AgentEntry, Effort, RegistrySnapshot};
use bridge_core::ids::AgentId;

/// Unified parse error covering TOML parse failures and missing env-var references.
#[derive(Debug)]
pub enum ConfigError {
    Toml(toml::de::Error),
    MissingEnvVar(String),
    /// Invalid registry config value (e.g. unknown effort level, empty agent id).
    /// Wired to main in Task 12.
    #[allow(dead_code)]
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
pub struct Config {
    pub agent: AgentConfig,
    pub server: ServerConfig,
    #[serde(default)]
    pub delegation: Option<DelegationConfig>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub cmd: String,
    pub args: Vec<String>,
    /// Optional model id; threaded into `AcpConfig::model` (best-effort
    /// `session/set_model`). Absent = the agent's default model.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional session mode id; threaded into `AcpConfig::mode` (hard
    /// `session/set_mode`). Absent = the agent's default mode.
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional auth-method id; threaded into `AcpConfig::auth_method`. Absent =
    /// the first method the agent advertises at `initialize`.
    #[serde(default)]
    pub auth_method: Option<String>,
    /// Optional absolute working directory the agent runs sessions in. Absent =
    /// the bridge's current working directory (resolved at startup). See
    /// [`AgentConfig::resolve_cwd`].
    #[serde(default)]
    pub cwd: Option<String>,
}

impl AgentConfig {
    /// Resolve the agent `cwd`: the configured value if present, otherwise the
    /// process's current working directory. The result MUST be absolute (ACP §11A
    /// requires an absolute `cwd` for `session/new`); a configured relative path is
    /// joined onto the current directory to make it absolute.
    pub fn resolve_cwd(&self) -> std::io::Result<std::path::PathBuf> {
        match &self.cwd {
            Some(c) => {
                let p = std::path::PathBuf::from(c);
                if p.is_absolute() {
                    Ok(p)
                } else {
                    Ok(std::env::current_dir()?.join(p))
                }
            }
            None => std::env::current_dir(),
        }
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

impl Config {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        let mut cfg: Config = toml::from_str(s)?;
        // Expand env vars in delegation strings after deserialization.
        if let Some(d) = cfg.delegation.as_mut() {
            d.peer_url = expand_env(&d.peer_url)?;
            d.auth = expand_env(&d.auth)?;
        }
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// Multi-agent registry config (Task 7 / Increment 3b).
// Parses a TOML with `[[agents]]` array + optional `[registry]` section.
// Main is rewired to use this in Task 12.
// ---------------------------------------------------------------------------

/// Top-level TOML structure for the multi-agent bridge config.
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // wired to main in Task 12
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
#[allow(dead_code)] // wired to main in Task 12
pub struct RegistrySection {
    #[serde(default)]
    pub allowed_cmds: Vec<String>,
}

/// One entry in the `[[agents]]` array, as parsed from TOML.
/// String fields are converted to typed domain values in `into_snapshot`.
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)] // wired to main in Task 12
pub struct AgentEntryToml {
    pub id: String,
    pub cmd: String,
    #[serde(default)]
    pub args: Vec<String>,
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

#[allow(dead_code)] // wired to main in Task 12
impl RegistryConfig {
    /// Parse a multi-agent TOML string into a `RegistryConfig`.
    /// Mirrors `Config::parse` (TOML deserialization; env-expansion of `[delegation]`
    /// is NOT applied here — Task 12 can add it when wiring to main).
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(s)?)
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
            entries.push(AgentEntry {
                id,
                cmd: a.cmd,
                args: a.args,
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
#[allow(dead_code)] // wired to main in Task 12
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_toml() {
        let c =
            Config::parse("[agent]\nname=\"kiro\"\ncmd=\"kiro-cli\"\nargs=[\"acp\"]\n[server]\n")
                .unwrap();
        assert_eq!(c.agent.name, "kiro");
        assert_eq!(c.agent.cmd, "kiro-cli");
        assert_eq!(c.agent.args, vec!["acp"]);
        assert_eq!(c.server.addr, "127.0.0.1:8080");
    }

    #[test]
    fn config_without_delegation_still_valid() {
        let c = Config::parse("[agent]\nname=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n").unwrap();
        assert!(c.delegation.is_none());
    }

    #[test]
    fn delegation_parsed_with_env_expansion() {
        std::env::set_var("PEER_TOKEN_T10", "sek");
        let c = Config::parse("[agent]\nname=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n[delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${PEER_TOKEN_T10}\"\n").unwrap();
        let d = c.delegation.unwrap();
        assert_eq!(d.peer_url, "http://p");
        assert_eq!(d.auth, "bearer:sek");
    }

    #[test]
    fn config_parses_optional_agent_model_mode_cwd() {
        // An [agent] section with model/mode/cwd/auth_method is accepted and
        // threaded onto AgentConfig.
        let c = Config::parse(
            "[agent]\nname=\"codex\"\ncmd=\"codex\"\nargs=[\"acp\"]\n\
             model=\"gpt-x\"\nmode=\"yolo\"\nauth_method=\"oauth\"\ncwd=\"/work/dir\"\n[server]\n",
        )
        .unwrap();
        assert_eq!(c.agent.model.as_deref(), Some("gpt-x"));
        assert_eq!(c.agent.mode.as_deref(), Some("yolo"));
        assert_eq!(c.agent.auth_method.as_deref(), Some("oauth"));
        assert_eq!(c.agent.cwd.as_deref(), Some("/work/dir"));
        // A configured absolute cwd resolves to itself (absolute).
        let resolved = c.agent.resolve_cwd().unwrap();
        assert_eq!(resolved, std::path::PathBuf::from("/work/dir"));
        assert!(resolved.is_absolute());
    }

    #[test]
    fn config_agent_model_mode_cwd_are_optional() {
        // Absence of model/mode/cwd/auth_method is fine; cwd resolves to the
        // process current dir (absolute) when unset.
        let c = Config::parse("[agent]\nname=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n").unwrap();
        assert!(c.agent.model.is_none());
        assert!(c.agent.mode.is_none());
        assert!(c.agent.auth_method.is_none());
        assert!(c.agent.cwd.is_none());
        let resolved = c.agent.resolve_cwd().unwrap();
        assert!(
            resolved.is_absolute(),
            "default cwd (current_dir) must be absolute: {resolved:?}"
        );
    }

    #[test]
    fn missing_env_var_errors() {
        let r = Config::parse("[agent]\nname=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n[delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${DEFINITELY_UNSET_VAR_XYZ}\"\n");
        assert!(r.is_err());
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
}
