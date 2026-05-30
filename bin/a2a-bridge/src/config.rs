// config.rs — TOML configuration for the a2a-bridge binary (spec §8, Task 15).

use std::fmt;

/// Unified parse error covering TOML parse failures and missing env-var references.
#[derive(Debug)]
pub enum ConfigError {
    Toml(toml::de::Error),
    MissingEnvVar(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Toml(e) => write!(f, "TOML parse error: {e}"),
            ConfigError::MissingEnvVar(v) => write!(f, "env var ${{{v}}} not set"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Toml(e) => Some(e),
            ConfigError::MissingEnvVar(_) => None,
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
    fn missing_env_var_errors() {
        let r = Config::parse("[agent]\nname=\"k\"\ncmd=\"k\"\nargs=[]\n[server]\n[delegation]\npeer_url=\"http://p\"\nauth=\"bearer:${DEFINITELY_UNSET_VAR_XYZ}\"\n");
        assert!(r.is_err());
    }
}
