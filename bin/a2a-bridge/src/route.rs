// route.rs — Composition-root routing for a2a-bridge binary.
// Routing lives in the binary, NOT in bridge-policy (spec §8, Task 15).

use std::sync::Arc;

use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ports::{AgentRegistry, RouteDecision};

/// Skill-aware routing (3b): tasks with `skill == "delegate"` are sent to the
/// configured peer; `skill == "fan-out"` is fanned out; all others fall back to
/// the agent specified by `meta.agent`, or the registry default.
pub struct SkillRoute {
    registry: Arc<dyn AgentRegistry>,
    workflows: std::collections::HashSet<String>,
}

impl SkillRoute {
    /// Construct with the boot-time set of known workflow ids (the route arm reads this).
    pub fn with_workflows(
        registry: Arc<dyn AgentRegistry>,
        workflows: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            registry,
            workflows,
        }
    }

    /// True if `id` names a configured workflow.
    pub fn knows_workflow(&self, id: &str) -> bool {
        self.workflows.contains(id)
    }
}

impl RouteDecision for SkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        match meta.skill.as_deref() {
            Some("delegate") => Ok(RouteTarget::Delegate),
            Some("fan-out") => Ok(RouteTarget::Fanout),
            // A skill naming a configured workflow routes to that workflow. Checked
            // before the Local fallback so a `skill="code-review"` runs the DAG.
            Some(s) if self.knows_workflow(s) => Ok(RouteTarget::Workflow(
                bridge_core::ids::WorkflowId::parse(s)?,
            )),
            _ => Ok(RouteTarget::Local(
                meta.agent
                    .clone()
                    .unwrap_or_else(|| self.registry.default_id()),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use bridge_core::domain::Part;
    use bridge_core::domain::{
        AgentEntry, AgentKind, Effort, RegistrySnapshot, RouteTarget, TaskMeta,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::AgentId;
    use bridge_core::ids::SessionId;
    use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};

    // ---- Minimal fake AgentRegistry whose default_id returns "codex" ----

    struct FakeRegistry {
        default: AgentId,
    }

    impl FakeRegistry {
        fn new(default: &str) -> Arc<Self> {
            Arc::new(Self {
                default: AgentId::parse(default).unwrap(),
            })
        }
    }

    struct NoopLease;
    impl Lease for NoopLease {}

    struct NoopBackend;
    #[async_trait::async_trait]
    impl AgentBackend for NoopBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(futures::stream::once(async {
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                })
            })))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: self.default.clone(),
                    cmd: Some("fake".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None::<Effort>,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(NoopBackend),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            self.default.clone()
        }
        async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![self.default.clone()]
        }
    }

    fn skill_route_with_default(default_id: &str) -> SkillRoute {
        SkillRoute::with_workflows(
            FakeRegistry::new(default_id),
            std::collections::HashSet::new(),
        )
    }

    // ---- Task 9 route tests ----

    #[test]
    fn skill_route_uses_registry_default_when_no_agent() {
        let r = skill_route_with_default("codex");
        assert!(matches!(
            r.route(&TaskMeta { skill: None, agent: None, ..Default::default() }).unwrap(),
            RouteTarget::Local(a) if a.as_str() == "codex"
        ));
    }

    #[test]
    fn skill_route_uses_explicit_agent_over_default() {
        let r = skill_route_with_default("codex");
        assert!(matches!(
            r.route(&TaskMeta {
                skill: None,
                agent: Some(AgentId::parse("kiro").unwrap()),
                ..Default::default()
            }).unwrap(),
            RouteTarget::Local(a) if a.as_str() == "kiro"
        ));
    }

    #[test]
    fn skill_route_delegates_on_delegate_skill() {
        let r = skill_route_with_default("codex");
        assert!(matches!(
            r.route(&TaskMeta {
                skill: Some("delegate".into()),
                ..Default::default()
            })
            .unwrap(),
            RouteTarget::Delegate
        ));
    }

    #[test]
    fn skill_route_fanout_on_fanout_skill() {
        let r = skill_route_with_default("codex");
        assert!(matches!(
            r.route(&TaskMeta {
                skill: Some("fan-out".into()),
                ..Default::default()
            })
            .unwrap(),
            RouteTarget::Fanout
        ));
    }

    #[test]
    fn with_workflows_stores_and_reports_ids() {
        let r = SkillRoute::with_workflows(
            FakeRegistry::new("codex"),
            ["code-review".to_string(), "triage".to_string()]
                .into_iter()
                .collect(),
        );
        assert!(r.knows_workflow("code-review"));
        assert!(r.knows_workflow("triage"));
        assert!(!r.knows_workflow("nope"));
        // a SkillRoute::new(...) (no workflows) knows none
        assert!(!skill_route_with_default("codex").knows_workflow("code-review"));
    }
}
