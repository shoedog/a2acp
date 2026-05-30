// config.rs — TOML configuration for the a2a-bridge binary (spec §8, Task 15).

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub agent: AgentConfig,
    pub server: ServerConfig,
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

fn default_addr() -> String {
    "127.0.0.1:8080".into()
}

impl Config {
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
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
}
