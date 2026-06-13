//! Per-agent MCP server specs + delivery channel — SDK-free domain types.
//!
//! An agent is offered a set of [`McpServerSpec`]s; the bridge delivers them via the one
//! [`McpDelivery`] channel that agent honors (claude = the ACP `mcpServers` param; codex = native
//! `-c mcp_servers.*` override args; kiro = a native `settings/mcp.json`). `{cwd}` in args/env values
//! is substituted with the session's repo at delivery time. See ADR-0028.

/// The only template token allowed in MCP `args`/`env` values: the agent's working repo.
pub const CWD_TOKEN: &str = "{cwd}";

/// A single MCP server to offer an agent. Vendor-neutral — no ACP SDK types — so `bridge-core`
/// stays SDK-free and the same spec feeds every delivery channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Ordered (name, value) env pairs; values may contain `{cwd}`.
    pub env: Vec<(String, String)>,
}

impl McpServerSpec {
    /// A copy with every `{cwd}` in `args`/`env` values replaced by `cwd`. `name`/`command` are not
    /// templated (the command path must be literal; validation rejects `{...}` in `command`).
    pub fn substituted(&self, cwd: &str) -> McpServerSpec {
        McpServerSpec {
            name: self.name.clone(),
            command: self.command.clone(),
            args: self.args.iter().map(|a| substitute_cwd(a, cwd)).collect(),
            env: self
                .env
                .iter()
                .map(|(k, v)| (k.clone(), substitute_cwd(v, cwd)))
                .collect(),
        }
    }
}

/// How the bridge delivers an agent's MCP servers. Each agent honors exactly ONE channel; resolved
/// at config build from the agent's `cmd` (overridable). The spawn branches on this so MCP reaches
/// the agent through the mechanism it actually reads (review BLOCKER 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpDelivery {
    /// claude: the ACP `session/new` `mcpServers` param (in-protocol, `{cwd}` re-sent per session).
    #[default]
    Acp,
    /// codex: native `-c mcp_servers.<name>.*` override args appended to the codex-acp argv
    /// (probe-proven; keeps the real `~/.codex` auth, writes no file).
    CodexNative,
    /// kiro: a native `settings/mcp.json` (kiro honors neither the ACP param for stdio nor `-c`
    /// overrides for `mcp_servers`).
    KiroNative,
}

/// Substitute every `{cwd}` token in `s` with `cwd`.
pub fn substitute_cwd(s: &str, cwd: &str) -> String {
    s.replace(CWD_TOKEN, cwd)
}

/// Validate that the only `{...}` token in `s` is `{cwd}`. Left-to-right: every `{` must open a
/// literal `{cwd}`; an unterminated brace or any other `{...}` is an error. (Literal/JSON braces are
/// unsupported in v1 — MCP args are flat strings.)
pub fn validate_cwd_template(s: &str) -> Result<(), String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if s[i..].starts_with(CWD_TOKEN) {
                i += CWD_TOKEN.len();
                continue;
            }
            // Find the closing brace (if any) to quote the offending token in the error.
            let rest = &s[i..];
            let tok = match rest.find('}') {
                Some(end) => &rest[..=end],
                None => rest,
            };
            return Err(format!(
                "unsupported template token `{tok}` (only `{CWD_TOKEN}` is allowed)"
            ));
        }
        i += 1;
    }
    Ok(())
}

/// codex silently drops an MCP server whose startup exceeds the default timeout (probe finding on a
/// cold prism build) — set it high so a warm prism always connects.
const CODEX_MCP_STARTUP_TIMEOUT_SEC: u32 = 120;

/// A TOML basic-string literal for `s` (double-quoted, with `\`, `"`, newline and tab escaped).
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render the flattened `-c`/value override pairs that inject `mcp` into codex via the argv
/// (`McpDelivery::CodexNative`), with `{cwd}` substituted to `cwd`. Appended to the codex-acp argv
/// host-side and in the `:rw` container alike. Empty input → empty output. Server/env names are
/// restricted to TOML bare keys at config validation, so the dotted `mcp_servers.<name>.env.<KEY>`
/// paths are well-formed. codex keeps its real `~/.codex` (auth) — these args only *add* prism (ADR-0028).
pub fn render_codex_mcp_args(mcp: &[McpServerSpec], cwd: &str) -> Vec<String> {
    let mut out = Vec::new();
    for spec in mcp {
        let s = spec.substituted(cwd);
        let p = format!("mcp_servers.{}", s.name);
        out.push("-c".to_string());
        out.push(format!("{p}.command={}", toml_str(&s.command)));
        let args = s
            .args
            .iter()
            .map(|a| toml_str(a))
            .collect::<Vec<_>>()
            .join(", ");
        out.push("-c".to_string());
        out.push(format!("{p}.args=[{args}]"));
        for (k, v) in &s.env {
            out.push("-c".to_string());
            out.push(format!("{p}.env.{k}={}", toml_str(v)));
        }
        out.push("-c".to_string());
        out.push(format!(
            "{p}.startup_timeout_sec={CODEX_MCP_STARTUP_TIMEOUT_SEC}"
        ));
    }
    out
}

/// The bridge-managed kiro agent name for an agent id — the basename of the `~/.kiro/agents/<name>.json`
/// config the bridge writes AND the value passed to `kiro-cli acp --agent <name>` (kept in sync).
pub fn kiro_agent_name(agent_id: &str) -> String {
    format!("a2a-mcp-{agent_id}")
}

/// Render a kiro **agent-config JSON** (written to `~/.kiro/agents/<agent_name>.json`) carrying `mcp`
/// as `mcpServers`, with `{cwd}` substituted. kiro honors neither the ACP `mcpServers` param (stdio)
/// nor codex `-c` overrides; it loads MCP servers from a named agent via `kiro-cli acp --agent
/// <agent_name>`. NOTE: kiro registers the tools **bare** (e.g. `nav_repo_map`), NOT `mcp__<server>__*`.
/// See ADR-0028.
pub fn render_kiro_agent_config(mcp: &[McpServerSpec], cwd: &str, agent_name: &str) -> String {
    use serde_json::{json, Map, Value};
    let mut servers = Map::new();
    let mut tools: Vec<Value> = vec![json!("*")];
    let mut allowed: Vec<Value> = Vec::new();
    for spec in mcp {
        let s = spec.substituted(cwd);
        let mut entry = Map::new();
        entry.insert("command".into(), json!(s.command));
        entry.insert("args".into(), json!(s.args));
        if !s.env.is_empty() {
            let env: Map<String, Value> = s.env.into_iter().map(|(k, v)| (k, json!(v))).collect();
            entry.insert("env".into(), Value::Object(env));
        }
        servers.insert(s.name.clone(), Value::Object(entry));
        // `@<server>` includes ALL of that server's tools; also auto-trust them (non-interactive ACP).
        tools.push(json!(format!("@{}", s.name)));
        allowed.push(json!(format!("@{}", s.name)));
    }
    let config = json!({
        "name": agent_name,
        "description": "a2a-bridge managed agent (MCP delivery, ADR-0028)",
        "mcpServers": Value::Object(servers),
        "tools": tools,
        "allowedTools": allowed,
        "includeMcpJson": false,
        // Custom kiro agents (unlike default agents) must opt into skill discovery via `resources`
        // with `skill://` URIs — workspace + global. Lets the bridge's kiro agents load the
        // cross-agent skills library (lsp-nav, prism-nav, …). See kiro.dev/docs/cli/skills.
        "resources": [
            "skill://.kiro/skills/*/SKILL.md",
            "skill://~/.kiro/skills/*/SKILL.md",
        ],
    });
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_all_cwd_tokens() {
        assert_eq!(
            substitute_cwd("--repo {cwd} --cache {cwd}/c", "/r"),
            "--repo /r --cache /r/c"
        );
        assert_eq!(substitute_cwd("no-token", "/r"), "no-token");
    }

    #[test]
    fn validate_accepts_cwd_and_plain() {
        assert!(validate_cwd_template("{cwd}").is_ok());
        assert!(validate_cwd_template("--repo {cwd} --x").is_ok());
        assert!(validate_cwd_template("plain").is_ok());
        assert!(validate_cwd_template("a {cwd} b {cwd} c").is_ok());
    }

    #[test]
    fn validate_rejects_other_and_unterminated_braces() {
        assert!(validate_cwd_template("{repo}").is_err());
        assert!(validate_cwd_template("--x {Cwd}").is_err()); // case-sensitive
        assert!(validate_cwd_template("{cwd").is_err()); // unterminated
        assert!(validate_cwd_template("ok {cwd} then {bad}").is_err());
        // Error quotes the offending token, not {cwd}.
        let e = validate_cwd_template("{repo}/x").unwrap_err();
        assert!(e.contains("{repo}"), "got: {e}");
    }

    #[test]
    fn substituted_templates_args_and_env_not_command() {
        let spec = McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec!["--repo".into(), "{cwd}".into()],
            env: vec![("ROOT".into(), "{cwd}/src".into())],
        };
        let s = spec.substituted("/repo");
        assert_eq!(s.command, "/opt/prism");
        assert_eq!(s.args, vec!["--repo".to_string(), "/repo".to_string()]);
        assert_eq!(s.env, vec![("ROOT".to_string(), "/repo/src".to_string())]);
    }

    #[test]
    fn mcp_delivery_defaults_to_acp() {
        assert_eq!(McpDelivery::default(), McpDelivery::Acp);
    }

    fn prism() -> McpServerSpec {
        McpServerSpec {
            name: "prism".into(),
            command: "/opt/prism".into(),
            args: vec![
                "--repo".into(),
                "{cwd}".into(),
                "--cache-dir".into(),
                "/cache".into(),
            ],
            env: vec![("RUST_LOG".into(), "warn".into())],
        }
    }

    #[test]
    fn codex_args_render_flattened_pairs_with_cwd_substituted() {
        let args = render_codex_mcp_args(&[prism()], "/repo");
        assert_eq!(args[0], "-c");
        assert_eq!(args[1], r#"mcp_servers.prism.command="/opt/prism""#);
        assert_eq!(args[2], "-c");
        assert_eq!(
            args[3],
            r#"mcp_servers.prism.args=["--repo", "/repo", "--cache-dir", "/cache"]"#
        );
        assert_eq!(args[4], "-c");
        assert_eq!(args[5], r#"mcp_servers.prism.env.RUST_LOG="warn""#);
        assert_eq!(args[6], "-c");
        assert_eq!(args[7], "mcp_servers.prism.startup_timeout_sec=120");
        assert_eq!(args.len(), 8);
        assert!(!args.iter().any(|a| a.contains("{cwd}")));
    }

    #[test]
    fn codex_args_empty_in_empty_out() {
        assert!(render_codex_mcp_args(&[], "/r").is_empty());
    }

    #[test]
    fn codex_args_empty_args_renders_empty_toml_array() {
        let spec = McpServerSpec {
            name: "x".into(),
            command: "/x".into(),
            args: vec![],
            env: vec![],
        };
        assert_eq!(
            render_codex_mcp_args(&[spec], "/r")[3],
            "mcp_servers.x.args=[]"
        );
    }

    #[test]
    fn codex_args_escape_quotes_and_backslashes() {
        let spec = McpServerSpec {
            name: "x".into(),
            command: r#"/a/"q"\b"#.into(),
            args: vec![],
            env: vec![],
        };
        assert_eq!(
            render_codex_mcp_args(&[spec], "/r")[1],
            r#"mcp_servers.x.command="/a/\"q\"\\b""#
        );
    }

    #[test]
    fn kiro_agent_name_is_namespaced() {
        assert_eq!(kiro_agent_name("kiro"), "a2a-mcp-kiro");
    }

    #[test]
    fn kiro_agent_config_carries_mcp_servers_cwd_substituted() {
        let cfg = render_kiro_agent_config(&[prism()], "/repo", "a2a-mcp-kiro");
        let v: serde_json::Value = serde_json::from_str(&cfg).expect("valid JSON");
        assert_eq!(v["name"], "a2a-mcp-kiro");
        assert_eq!(v["mcpServers"]["prism"]["command"], "/opt/prism");
        assert_eq!(
            v["mcpServers"]["prism"]["args"],
            serde_json::json!(["--repo", "/repo", "--cache-dir", "/cache"])
        );
        assert_eq!(v["mcpServers"]["prism"]["env"]["RUST_LOG"], "warn");
        // `@prism` in tools + allowedTools so kiro registers + auto-trusts the server's tools.
        assert!(v["tools"].as_array().unwrap().iter().any(|t| t == "@prism"));
        assert!(v["allowedTools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "@prism"));
        assert!(!cfg.contains("{cwd}"));
    }

    #[test]
    fn kiro_agent_config_advertises_skill_resources() {
        // Custom agents must opt into skill discovery via `resources` with skill:// URIs.
        let cfg = render_kiro_agent_config(&[], "/repo", "a2a-mcp-kiro");
        let v: serde_json::Value = serde_json::from_str(&cfg).expect("valid JSON");
        let res = v["resources"].as_array().expect("resources array");
        let joined = res
            .iter()
            .filter_map(|r| r.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("skill://"),
            "must advertise skill:// resources, got {joined}"
        );
        assert!(
            joined.contains("~/.kiro/skills/"),
            "must include the global skills glob"
        );
    }
}
