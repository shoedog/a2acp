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
}
