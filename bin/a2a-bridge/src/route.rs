// route.rs — Composition-root routing for a2a-bridge binary.
// Routing lives in the binary, NOT in bridge-policy (spec §8, Task 15).

use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::RouteDecision;

/// Skill-aware routing (v2.5): tasks with `skill == "delegate"` are sent to the
/// configured peer; all others fall back to the local Kiro backend.
pub struct SkillRoute;

impl RouteDecision for SkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        match meta.skill.as_deref() {
            Some("delegate") => Ok(RouteTarget::Delegate),
            Some("fan-out") => Ok(RouteTarget::Fanout),
            _ => Ok(RouteTarget::Local(AgentId::parse("kiro")?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{RouteTarget, TaskMeta};
    use bridge_core::ports::RouteDecision;

    #[test]
    fn skill_route_delegates_on_delegate_skill() {
        assert!(matches!(
            SkillRoute
                .route(&TaskMeta {
                    skill: Some("delegate".into()),
                    ..Default::default()
                })
                .unwrap(),
            RouteTarget::Delegate
        ));
        assert!(matches!(
            SkillRoute
                .route(&TaskMeta {
                    skill: None,
                    ..Default::default()
                })
                .unwrap(),
            RouteTarget::Local(a) if a.as_str() == "kiro"
        ));
    }

    #[test]
    fn skill_route_fanout_on_fanout_skill() {
        assert!(matches!(
            SkillRoute
                .route(&TaskMeta {
                    skill: Some("fan-out".into()),
                    ..Default::default()
                })
                .unwrap(),
            RouteTarget::Fanout
        ));
        assert!(matches!(
            SkillRoute
                .route(&TaskMeta {
                    skill: None,
                    ..Default::default()
                })
                .unwrap(),
            RouteTarget::Local(a) if a.as_str() == "kiro"
        ));
    }
}
