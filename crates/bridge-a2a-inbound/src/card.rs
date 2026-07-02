// card.rs — A2A Agent Card advertising the bridge skills, plus version-pin guard.
//
// The a2a-lf 0.3.0 API uses:
//   - `AgentCard` { skills: Vec<AgentSkill>, supported_interfaces: Vec<AgentInterface>, … }
//   - `AgentInterface::new(url, protocol_binding)` auto-sets protocol_version = a2a::VERSION
//   - There is NO protocol_version field on AgentCard itself; it lives on each AgentInterface.
//   - `a2a::VERSION = "1.0"` is the A2A v1 protocol version string the crate uses.

use a2a::{
    AgentCapabilities, AgentCard, AgentExtension, AgentInterface, AgentSkill,
    TRANSPORT_PROTOCOL_JSONRPC, VERSION,
};

use bridge_core::error::BridgeError;

/// The A2A protocol version this bridge is pinned to.
/// Equals `a2a::VERSION` from the a2a-lf 0.3.0 crate (the A2A v1 wire protocol).
pub const A2A_PINNED_VERSION: &str = VERSION;

/// Build the AgentCard advertising the fixed skills (`code`, `delegate`,
/// `fan-out`) plus one `workflow`-tagged skill per configured workflow id.
///
/// `mcp_servers` is `(agent_id, [server names])` for each agent that exposes MCP servers; when
/// non-empty it is advertised as a `capabilities.extensions` entry so A2A orchestrators can discover
/// e.g. prism without out-of-band knowledge (ADR-0028).
///
/// The card exposes a single JSONRPC interface at `<base_url>`.
pub fn agent_card(
    base_url: &str,
    workflow_ids: &[&str],
    mcp_servers: &[(String, Vec<String>)],
    catalog: &bridge_core::catalog::ModelCatalog,
) -> AgentCard {
    let code_skill = AgentSkill {
        id: "code".to_string(),
        name: "Code".to_string(),
        description: "Drive the configured local coding agent (ACP) to perform code \
                       generation, editing, and development tasks on behalf of A2A clients."
            .to_string(),
        tags: vec!["code".to_string(), "acp".to_string(), "cli".to_string()],
        examples: Some(vec![
            "Implement a Rust function that parses JSON".to_string(),
            "Add unit tests to the auth module".to_string(),
        ]),
        input_modes: None,
        output_modes: None,
        security_requirements: None,
    };

    let delegate_skill = AgentSkill {
        id: "delegate".to_string(),
        name: "Delegate".to_string(),
        description: "Delegate the task to a configured remote A2A peer agent.".to_string(),
        tags: vec!["delegate".to_string(), "proxy".to_string()],
        examples: Some(vec![
            "Forward this task to the upstream coding agent".to_string()
        ]),
        input_modes: None,
        output_modes: None,
        security_requirements: None,
    };

    let fanout_skill = AgentSkill {
        id: "fan-out".to_string(),
        name: "Fan-Out".to_string(),
        description: "Run on both the local agent and the configured peer (second opinion). \
                       Merges responses from both sources into a single stream."
            .to_string(),
        tags: vec!["fanout".to_string(), "merge".to_string()],
        examples: Some(vec![
            "Get a second opinion on this implementation".to_string()
        ]),
        input_modes: None,
        output_modes: None,
        security_requirements: None,
    };

    let mut skills = vec![code_skill, delegate_skill, fanout_skill];
    // One advertised skill per configured workflow id (W1): clients send
    // `a2a-bridge.skill = "<id>"` to run that workflow as a streaming task.
    for id in workflow_ids {
        skills.push(AgentSkill {
            id: (*id).to_string(),
            name: (*id).to_string(),
            description: format!(
                "Run the {id} workflow (detached: returns a working task; poll tasks/get)."
            ),
            tags: vec!["workflow".to_string(), "detached".to_string()],
            examples: None,
            input_modes: None,
            output_modes: None,
            security_requirements: None,
        });
    }

    // MCP advertisement (ADR-0028): one extension listing each agent's MCP server names, so an A2A
    // orchestrator can discover (e.g.) prism and the usage contract without out-of-band knowledge.
    let extensions = if mcp_servers.is_empty() {
        None
    } else {
        let servers: serde_json::Map<String, serde_json::Value> = mcp_servers
            .iter()
            .map(|(agent, names)| (agent.clone(), serde_json::json!(names)))
            .collect();
        let mut params = std::collections::HashMap::new();
        params.insert("servers".to_string(), serde_json::Value::Object(servers));
        Some(vec![AgentExtension {
            uri: "https://github.com/shoedog/a2acp/ext/mcp-servers/v1".to_string(),
            description: Some(
                "MCP servers exposed to this bridge's agents (per ADR-0028). `params.servers` maps \
                 agent id -> server names. To use: target one of those agents, set \
                 `message.metadata.cwd` to the target repo, and prompt the agent to use its \
                 `mcp__<server>__*` tools. claude is multi-repo (re-targeted per request); \
                 codex/kiro are single-repo under serve (the agent's configured cwd)."
                    .to_string(),
            ),
            required: Some(false),
            params: Some(params),
        }])
    };

    // agent-models extension: per-agent model catalog from the live probes. The per-agent object
    // (empty effort/modes omitted) is built by the shared `caps_to_json` the CLI also uses (DRY).
    let mut ext_vec = extensions.unwrap_or_default();
    if !catalog.is_empty() {
        let agents: serde_json::Map<String, serde_json::Value> = catalog
            .iter()
            .map(|(id, c)| (id.clone(), bridge_core::catalog::caps_to_json(c)))
            .collect();
        let mut params = std::collections::HashMap::new();
        params.insert("agents".to_string(), serde_json::Value::Object(agents));
        ext_vec.push(AgentExtension {
            uri: "https://github.com/shoedog/a2acp/ext/agent-models/v1".to_string(),
            description: Some(
                "Per-agent model/effort/mode catalog. To override a default, send message.metadata \
            `a2a-bridge.model` only for agents with `model_configurable: true`, and \
            `a2a-bridge.effort` / `a2a-bridge.mode` only when those lists are present."
                    .to_string(),
            ),
            required: Some(false),
            params: Some(params),
        });
    }
    let extensions = if ext_vec.is_empty() {
        None
    } else {
        Some(ext_vec)
    };

    AgentCard {
        name: "a2a-bridge".to_string(),
        description: "A2A↔ACP bridge that routes agent tasks to the configured local \
                      agent(s) and review workflows."
            .to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: vec![AgentInterface::new(base_url, TRANSPORT_PROTOCOL_JSONRPC)],
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: None,
            extensions,
            extended_agent_card: None,
        },
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        skills,
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    }
}

/// Returns `Ok(())` if `v` matches the pinned A2A protocol version, otherwise
/// `Err(BridgeError::A2aVersionMismatch)`.
///
/// Call this on the `A2A-Version` service parameter of every inbound request
/// to reject clients speaking a different protocol revision.
pub fn assert_supported_version(v: &str) -> Result<(), BridgeError> {
    if v == A2A_PINNED_VERSION {
        Ok(())
    } else {
        Err(BridgeError::A2aVersionMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::error::BridgeError;

    #[test]
    fn card_has_two_skills_and_pinned_version() {
        let c = agent_card(
            "http://localhost:8080",
            &[],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        // Updated for Task 5a: three skills now (code, delegate, fan-out).
        assert!(c.skills.len() >= 2);
        assert!(c.skills.iter().any(|s| s.id == "code"));
        // Protocol version lives on the AgentInterface (not on AgentCard itself).
        // AgentInterface::new() auto-sets protocol_version = a2a::VERSION.
        assert_eq!(c.supported_interfaces.len(), 1);
        assert_eq!(
            c.supported_interfaces[0].protocol_version,
            A2A_PINNED_VERSION
        );
        // Confirm the pinned constant matches the crate's own VERSION constant.
        assert_eq!(A2A_PINNED_VERSION, a2a::VERSION);
    }

    #[test]
    fn supported_version_accepts_pinned_rejects_other() {
        assert!(assert_supported_version(A2A_PINNED_VERSION).is_ok());
        assert_eq!(
            assert_supported_version("0.0.0-bogus").unwrap_err(),
            BridgeError::A2aVersionMismatch
        );
    }

    // ---- Task 7: delegate skill advertisement ----

    #[test]
    fn card_advertises_two_skills() {
        let c = agent_card(
            "http://localhost:8080",
            &[],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        // Updated for Task 5a: three skills now.
        assert!(c.skills.len() >= 2);
        assert!(c.skills.iter().any(|s| s.id == "delegate"));
        assert!(c.skills.iter().any(|s| s.id == "code"));
    }

    // ---- Task 5a: fan-out skill advertisement ----

    #[test]
    fn card_has_three_skills_incl_fanout() {
        let c = agent_card(
            "http://x",
            &[],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        assert_eq!(c.skills.len(), 3);
        assert!(c.skills.iter().any(|s| s.id == "fan-out"));
    }

    // ---- Task 9 (W1): workflow skills appended ----

    #[test]
    fn card_appends_one_skill_per_workflow_id() {
        let ids = ["code-review", "triage"];
        let c = agent_card(
            "http://x",
            &ids,
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        // 3 fixed skills + one per workflow id.
        assert_eq!(c.skills.len(), 3 + ids.len());
        let wf = c.skills.iter().find(|s| s.id == "code-review").unwrap();
        assert!(wf.tags.iter().any(|t| t == "workflow"));
        assert!(wf.description.to_lowercase().contains("detached"));
        assert!(c.skills.iter().any(|s| s.id == "triage"));
    }

    // ---- Task 12c: workflow skills advertise detached submit ----

    #[test]
    fn agent_card_marks_workflow_skills_detached() {
        let card = agent_card(
            "http://x",
            &["code-review"],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        let skill = card
            .skills
            .iter()
            .find(|s| s.id == "code-review")
            .expect("workflow skill");
        let marked = skill.tags.iter().any(|x| x == "detached")
            || skill.description.to_lowercase().contains("detached");
        assert!(marked, "workflow skill must advertise detached submit");
    }

    #[test]
    fn base_skills_are_not_marked_detached() {
        let card = agent_card(
            "http://x",
            &["code-review"],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        for id in &["code", "delegate", "fan-out"] {
            let skill = card.skills.iter().find(|s| s.id == *id).unwrap();
            let marked = skill.tags.iter().any(|x| x == "detached")
                || skill.description.to_lowercase().contains("detached");
            assert!(!marked, "base skill '{id}' must NOT be marked detached");
        }
    }

    // ---- ADR-0028: MCP server advertisement extension ----

    #[test]
    fn card_advertises_mcp_servers_as_extension() {
        let mcp = vec![
            ("claude".to_string(), vec!["prism".to_string()]),
            ("codex".to_string(), vec!["prism".to_string()]),
        ];
        let c = agent_card(
            "http://x",
            &[],
            &mcp,
            &bridge_core::catalog::ModelCatalog::new(),
        );
        let exts = c
            .capabilities
            .extensions
            .expect("capabilities.extensions present when MCP servers exist");
        assert_eq!(exts.len(), 1);
        assert!(exts[0].uri.contains("mcp-servers"), "uri: {}", exts[0].uri);
        let servers = exts[0]
            .params
            .as_ref()
            .and_then(|p| p.get("servers"))
            .expect("params.servers");
        assert_eq!(servers["claude"], serde_json::json!(["prism"]));
        assert_eq!(servers["codex"], serde_json::json!(["prism"]));
    }

    #[test]
    fn card_has_no_extension_without_mcp() {
        let c = agent_card(
            "http://x",
            &["code-review"],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        assert!(c.capabilities.extensions.is_none());
    }

    #[test]
    fn card_advertises_agent_models_extension() {
        use bridge_core::catalog::{AgentCaps, ModelCatalog};
        let mut cat = ModelCatalog::new();
        cat.insert(
            "claude".into(),
            AgentCaps {
                current_model: Some("sonnet".into()),
                models: vec!["default".into(), "sonnet".into(), "haiku".into()],
                model_configurable: true,
                effort_levels: vec!["low".into(), "high".into()],
                modes: vec![],
                current_mode: None,
            },
        );
        let c = agent_card("http://x", &[], &[], &cat);
        let exts = c.capabilities.extensions.expect("extensions");
        let ext = exts
            .iter()
            .find(|e| e.uri.contains("agent-models"))
            .expect("agent-models ext");
        let agents = ext
            .params
            .as_ref()
            .and_then(|p| p.get("agents"))
            .expect("params.agents");
        assert_eq!(agents["claude"]["current"], serde_json::json!("sonnet"));
        assert_eq!(
            agents["claude"]["models"],
            serde_json::json!(["default", "sonnet", "haiku"])
        );
        assert_eq!(
            agents["claude"]["model_configurable"],
            serde_json::json!(true)
        );
        assert_eq!(
            agents["claude"]["effort"],
            serde_json::json!(["low", "high"])
        );
        assert!(
            agents["claude"].get("modes").is_none(),
            "empty modes omitted"
        );
    }

    #[test]
    fn card_has_no_agent_models_ext_when_catalog_empty() {
        let c = agent_card(
            "http://x",
            &[],
            &[],
            &bridge_core::catalog::ModelCatalog::new(),
        );
        let has = c
            .capabilities
            .extensions
            .unwrap_or_default()
            .iter()
            .any(|e| e.uri.contains("agent-models"));
        assert!(!has);
    }
}
