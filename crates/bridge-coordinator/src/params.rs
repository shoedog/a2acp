use bridge_core::domain::{AgentOverride, Effort};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, ContextId};
use bridge_core::session_cwd::SessionCwd;

/// D1 typed operation params.
///
/// The typed surface over `AgentOverride`/`SessionCwd`, populated identically
/// from MCP arguments / CLI flags / A2A `a2a-bridge.*` metadata. `cwd` is the
/// raw string; shape and allowed-root validation are handled by `validate_cwd`.
/// `workflow`/`skill` select the route.
#[derive(Debug, Clone)]
pub struct OpParams {
    pub workflow: Option<String>,
    pub skill: Option<String>,
    pub input: String,
    pub context: Option<ContextId>,
    pub agent: Option<AgentId>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub mode: Option<String>,
    pub cwd: Option<String>,
}

impl OpParams {
    /// From an MCP `tools/call` arguments object.
    pub fn from_mcp_args(v: &serde_json::Value) -> Result<Self, BridgeError> {
        let input = string_field(v, "input")
            .ok_or(BridgeError::InvalidRequest { field: "input" })?
            .to_string();
        Ok(Self {
            workflow: string_field(v, "workflow").map(str::to_string),
            skill: string_field(v, "skill").map(str::to_string),
            input,
            context: parse_context(string_field(v, "context"))?,
            agent: parse_agent(string_field(v, "agent"))?,
            model: string_field(v, "model").map(str::to_string),
            effort: parse_effort(string_field(v, "effort"))?,
            mode: string_field(v, "mode").map(str::to_string),
            cwd: string_field(v, "cwd").map(str::to_string),
        })
    }

    /// FIX-7: the run_workflow form rejects per-agent overrides.
    pub fn from_mcp_args_for_workflow(v: &serde_json::Value) -> Result<Self, BridgeError> {
        let p = Self::from_mcp_args(v)?;
        if p.agent.is_some() || p.model.is_some() || p.effort.is_some() || p.mode.is_some() {
            return Err(BridgeError::InvalidRequest {
                field: "agent/model/effort/mode (run_workflow ignores overrides)",
            });
        }
        if p.workflow.is_none() {
            return Err(BridgeError::InvalidRequest { field: "workflow" });
        }
        Ok(p)
    }

    /// From CLI flags, with `input` already read from `--input`.
    pub fn from_cli_flags(args: &[String], input: String) -> Result<Self, BridgeError> {
        let mut positionals = Vec::new();
        let mut context = None;
        let mut agent = None;
        let mut model = None;
        let mut effort = None;
        let mut mode = None;
        let mut cwd = None;
        let mut run_workflow_shape = false;

        let mut idx = 0;
        while idx < args.len() {
            match args[idx].as_str() {
                "--context" => {
                    context = Some(cli_value(args, &mut idx, "context")?.to_string());
                }
                "--agent" => {
                    agent = Some(cli_value(args, &mut idx, "agent")?.to_string());
                }
                "--model" => {
                    model = Some(cli_value(args, &mut idx, "model")?.to_string());
                }
                "--effort" => {
                    effort = Some(cli_value(args, &mut idx, "effort")?.to_string());
                }
                "--mode" => {
                    mode = Some(cli_value(args, &mut idx, "mode")?.to_string());
                }
                "--cwd" => {
                    cwd = Some(cli_value(args, &mut idx, "cwd")?.to_string());
                }
                "--session-cwd" => {
                    run_workflow_shape = true;
                    cwd = Some(cli_value(args, &mut idx, "cwd")?.to_string());
                }
                "--input" | "--url" | "--out" | "--config" => {
                    let _ = cli_value(args, &mut idx, "flags")?;
                }
                "--serve" => {
                    run_workflow_shape = true;
                    idx += 1;
                }
                other if other.starts_with("--") => {
                    return Err(BridgeError::InvalidRequest { field: "flags" });
                }
                other => {
                    positionals.push(other.to_string());
                    idx += 1;
                }
            }
        }

        if positionals.len() > 1 {
            return Err(BridgeError::InvalidRequest { field: "route" });
        }
        let route = positionals.into_iter().next();
        Ok(Self {
            workflow: run_workflow_shape.then(|| route.clone()).flatten(),
            skill: (!run_workflow_shape).then_some(route).flatten(),
            input,
            context: parse_context(context.as_deref())?,
            agent: parse_agent(agent.as_deref())?,
            model,
            effort: parse_effort(effort.as_deref())?,
            mode,
            cwd,
        })
    }

    /// From A2A `message.metadata` `a2a-bridge.*` keys.
    pub fn from_a2a_metadata(
        md: &serde_json::Map<String, serde_json::Value>,
        input: String,
    ) -> Result<Self, BridgeError> {
        Ok(Self {
            workflow: None,
            skill: metadata_string(md, "a2a-bridge.skill").map(str::to_string),
            input,
            context: parse_context(metadata_string(md, "a2a-bridge.context"))?,
            agent: parse_agent(metadata_string(md, "a2a-bridge.agent"))?,
            model: metadata_string(md, "a2a-bridge.model").map(str::to_string),
            effort: parse_effort(metadata_string(md, "a2a-bridge.effort"))?,
            mode: metadata_string(md, "a2a-bridge.mode").map(str::to_string),
            cwd: metadata_string(md, "a2a-bridge.cwd").map(str::to_string),
        })
    }

    pub fn agent_override(&self) -> AgentOverride {
        AgentOverride {
            model: self.model.clone(),
            effort: self.effort,
            mode: self.mode.clone(),
        }
    }

    /// FIX-9/PFIX-N: shape-validate cwd and enforce the configured root.
    pub fn validate_cwd(
        &self,
        root: Option<&SessionCwd>,
    ) -> Result<Option<SessionCwd>, BridgeError> {
        let Some(raw) = &self.cwd else {
            return Ok(None);
        };
        let cwd =
            SessionCwd::parse(raw).map_err(|_| BridgeError::InvalidRequest { field: "cwd" })?;
        if let Some(root) = root {
            if !cwd.is_under(root) {
                return Err(BridgeError::InvalidRequest { field: "cwd" });
            }
        }
        Ok(Some(cwd))
    }
}

fn string_field<'a>(v: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    v.get(field).and_then(|x| x.as_str())
}

fn metadata_string<'a>(
    md: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    md.get(key).and_then(|v| v.as_str())
}

fn parse_context(raw: Option<&str>) -> Result<Option<ContextId>, BridgeError> {
    raw.map(ContextId::parse)
        .transpose()
        .map_err(|_| BridgeError::InvalidRequest { field: "context" })
}

fn parse_agent(raw: Option<&str>) -> Result<Option<AgentId>, BridgeError> {
    raw.map(AgentId::parse)
        .transpose()
        .map_err(|_| BridgeError::InvalidRequest { field: "agent" })
}

fn parse_effort(raw: Option<&str>) -> Result<Option<Effort>, BridgeError> {
    raw.map(str::parse)
        .transpose()
        .map_err(|_| BridgeError::InvalidRequest { field: "effort" })
}

fn cli_value<'a>(
    args: &'a [String],
    idx: &mut usize,
    field: &'static str,
) -> Result<&'a str, BridgeError> {
    *idx += 1;
    let value = args
        .get(*idx)
        .ok_or(BridgeError::InvalidRequest { field })?;
    *idx += 1;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(input: &str) -> OpParams {
        OpParams {
            workflow: None,
            skill: None,
            input: input.to_string(),
            context: None,
            agent: None,
            model: None,
            effort: None,
            mode: None,
            cwd: None,
        }
    }

    #[test]
    fn from_mcp_args_lowers_to_override() {
        let v = serde_json::json!({"input":"hi","agent":"codex","model":"gpt-5.5","effort":"high"});
        let p = OpParams::from_mcp_args(&v).unwrap();
        assert_eq!(p.input, "hi");
        assert_eq!(p.agent.as_ref().map(|a| a.as_str()), Some("codex"));
        let ov = p.agent_override();
        assert_eq!(ov.model.as_deref(), Some("gpt-5.5"));
        assert!(matches!(ov.effort, Some(Effort::High)));
    }

    #[test]
    fn from_mcp_args_missing_input_is_invalid() {
        assert!(matches!(
            OpParams::from_mcp_args(&serde_json::json!({})),
            Err(BridgeError::InvalidRequest { field }) if field == "input"
        ));
    }

    #[test]
    fn run_workflow_variant_rejects_agent_overrides() {
        let v = serde_json::json!({"workflow":"code-review","input":"x","effort":"high"});
        assert!(matches!(
            OpParams::from_mcp_args_for_workflow(&v),
            Err(BridgeError::InvalidRequest { .. })
        ));

        let ok = serde_json::json!({"workflow":"code-review","input":"x"});
        assert!(OpParams::from_mcp_args_for_workflow(&ok).is_ok());
    }

    #[test]
    fn validate_cwd_uses_is_under() {
        let root = SessionCwd::parse("/Users/x/code").unwrap();
        let inside = OpParams {
            cwd: Some("/Users/x/code/repo".into()),
            ..base("hi")
        };
        assert!(inside.validate_cwd(Some(&root)).unwrap().is_some());
        let outside = OpParams {
            cwd: Some("/etc".into()),
            ..base("hi")
        };
        assert!(matches!(
            outside.validate_cwd(Some(&root)),
            Err(BridgeError::InvalidRequest { field }) if field == "cwd"
        ));
        assert!(inside.validate_cwd(None).unwrap().is_some());
    }

    #[test]
    fn from_cli_flags_mirrors_submit_flags() {
        let args = vec![
            "code-review".to_string(),
            "--context".to_string(),
            "ctx-1".to_string(),
            "--agent".to_string(),
            "codex".to_string(),
            "--model".to_string(),
            "gpt-5.5".to_string(),
            "--effort".to_string(),
            "xhigh".to_string(),
            "--mode".to_string(),
            "read-only".to_string(),
            "--cwd".to_string(),
            "/repo".to_string(),
        ];
        let p = OpParams::from_cli_flags(&args, "hello".to_string()).unwrap();
        assert_eq!(p.input, "hello");
        assert_eq!(p.skill.as_deref(), Some("code-review"));
        assert_eq!(p.context.as_ref().map(|c| c.as_str()), Some("ctx-1"));
        assert_eq!(p.agent.as_ref().map(|a| a.as_str()), Some("codex"));
        assert_eq!(p.model.as_deref(), Some("gpt-5.5"));
        assert!(matches!(p.effort, Some(Effort::Xhigh)));
        assert_eq!(p.mode.as_deref(), Some("read-only"));
        assert_eq!(p.cwd.as_deref(), Some("/repo"));
    }

    #[test]
    fn from_a2a_metadata_mirrors_existing_keys() {
        let md = serde_json::json!({
            "a2a-bridge.skill": "delegate",
            "a2a-bridge.context": "ctx-2",
            "a2a-bridge.agent": "kiro",
            "a2a-bridge.effort": "medium",
            "a2a-bridge.cwd": "/work/repo"
        });
        let md = md.as_object().unwrap();
        let p = OpParams::from_a2a_metadata(md, "go".to_string()).unwrap();
        assert_eq!(p.input, "go");
        assert_eq!(p.skill.as_deref(), Some("delegate"));
        assert_eq!(p.context.as_ref().map(|c| c.as_str()), Some("ctx-2"));
        assert_eq!(p.agent.as_ref().map(|a| a.as_str()), Some("kiro"));
        assert!(matches!(p.effort, Some(Effort::Medium)));
        assert_eq!(p.cwd.as_deref(), Some("/work/repo"));
    }
}
