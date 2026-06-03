// card.rs — A2A Agent Card advertising the Kiro skill, plus version-pin guard.
//
// The a2a-lf 0.3.0 API uses:
//   - `AgentCard` { skills: Vec<AgentSkill>, supported_interfaces: Vec<AgentInterface>, … }
//   - `AgentInterface::new(url, protocol_binding)` auto-sets protocol_version = a2a::VERSION
//   - There is NO protocol_version field on AgentCard itself; it lives on each AgentInterface.
//   - `a2a::VERSION = "1.0"` is the A2A v1 protocol version string the crate uses.

use a2a::{
    AgentCapabilities, AgentCard, AgentInterface, AgentSkill, TRANSPORT_PROTOCOL_JSONRPC, VERSION,
};

use bridge_core::error::BridgeError;

/// The A2A protocol version this bridge is pinned to.
/// Equals `a2a::VERSION` from the a2a-lf 0.3.0 crate (the A2A v1 wire protocol).
pub const A2A_PINNED_VERSION: &str = VERSION;

/// Build the AgentCard advertising the fixed skills (Kiro coding, `delegate`,
/// `fan-out`) plus one `workflow`-tagged skill per configured workflow id.
///
/// The card exposes a single JSONRPC interface at `<base_url>`.
pub fn agent_card(base_url: &str, workflow_ids: &[&str]) -> AgentCard {
    let kiro_skill = AgentSkill {
        id: "kiro-code".to_string(),
        name: "Kiro Code".to_string(),
        description: "Drive the Kiro CLI agent to perform code generation, editing, and \
                       development tasks on behalf of A2A clients."
            .to_string(),
        tags: vec!["code".to_string(), "kiro".to_string(), "cli".to_string()],
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

    let mut skills = vec![kiro_skill, delegate_skill, fanout_skill];
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

    AgentCard {
        name: "A2A-Bridge / Kiro".to_string(),
        description: "A2A bridge that routes agent tasks to the Kiro CLI coding agent.".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        supported_interfaces: vec![AgentInterface::new(base_url, TRANSPORT_PROTOCOL_JSONRPC)],
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: None,
            extensions: None,
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
        let c = agent_card("http://localhost:8080", &[]);
        // Updated for Task 5a: three skills now (kiro-code, delegate, fan-out).
        assert!(c.skills.len() >= 2);
        assert!(c.skills.iter().any(|s| s.id == "kiro-code"));
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
        let c = agent_card("http://localhost:8080", &[]);
        // Updated for Task 5a: three skills now.
        assert!(c.skills.len() >= 2);
        assert!(c.skills.iter().any(|s| s.id == "delegate"));
        assert!(c.skills.iter().any(|s| s.id == "kiro-code"));
    }

    // ---- Task 5a: fan-out skill advertisement ----

    #[test]
    fn card_has_three_skills_incl_fanout() {
        let c = agent_card("http://x", &[]);
        assert_eq!(c.skills.len(), 3);
        assert!(c.skills.iter().any(|s| s.id == "fan-out"));
    }

    // ---- Task 9 (W1): workflow skills appended ----

    #[test]
    fn card_appends_one_skill_per_workflow_id() {
        let ids = ["code-review", "triage"];
        let c = agent_card("http://x", &ids);
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
        let card = agent_card("http://x", &["code-review"]);
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
        let card = agent_card("http://x", &["code-review"]);
        for id in &["kiro-code", "delegate", "fan-out"] {
            let skill = card.skills.iter().find(|s| s.id == *id).unwrap();
            let marked = skill.tags.iter().any(|x| x == "detached")
                || skill.description.to_lowercase().contains("detached");
            assert!(!marked, "base skill '{id}' must NOT be marked detached");
        }
    }
}
