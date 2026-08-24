//! Workflow DAG types + validation. Edges are implicit from each node's `inputs`.
use bridge_core::attestation::HarvestSanitizationMode;
use bridge_core::domain::{EffectiveConfig, Effort};
use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    PolicyActivationV1,
};
use bridge_core::ids::{AgentId, NodeId, WorkflowId};
use bridge_core::ports::AgentRegistry;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PanelConfig {
    #[serde(default)]
    pub weights: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelConfig>,
    /// Additive R2f1a declared controls. True omission retains the compatibility profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controls: Option<bridge_core::execution_policy::WorkflowControlDefaultsV1>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_cap_ms: Option<u64>,
}

impl RetryPolicy {
    /// Total attempts (>=1). `max_attempts == 0` is treated as 1 (defensive).
    pub fn attempts(&self) -> u32 {
        self.max_attempts.max(1)
    }

    /// Overflow-safe backoff for `attempt` (1-based): min(backoff_ms * 2^(attempt-1), cap).
    pub fn backoff_for(&self, attempt: u32) -> std::time::Duration {
        let cap = self.backoff_cap_ms.unwrap_or(30_000);
        let shift = attempt.saturating_sub(1);
        // `checked_shl` only rejects shift >= bit-width (it WRAPS the value otherwise), so a large
        // `attempt` would silently wrap `backoff_ms << shift` to a small value and defeat the cap.
        // Multiply by `2^shift` with `checked_mul` (saturating to MAX) to catch VALUE overflow.
        let base = if shift >= 64 {
            u64::MAX
        } else {
            self.backoff_ms.saturating_mul(1u64 << shift)
        };
        std::time::Duration::from_millis(base.min(cap))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,
    pub prompt_template: String,
    pub inputs: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harvest_sanitization: Option<HarvestSanitizationMode>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowError {
    Empty,
    DuplicateNode(String),
    UnknownInput { node: String, input: String },
    Cyclic,
    NotSingleTerminal(usize),
}

fn push_field(target: &mut String, value: &str) {
    use std::fmt::Write as _;
    let _ = write!(target, "{}:", value.len());
    target.push_str(value);
}

fn push_optional(target: &mut String, value: Option<&str>) {
    match value {
        Some(value) => {
            target.push('1');
            push_field(target, value);
        }
        None => target.push('0'),
    }
}

fn effort_name(effort: Effort) -> &'static str {
    match effort {
        Effort::Minimal => "minimal",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

fn push_effective_config(target: &mut String, config: &EffectiveConfig) {
    push_optional(target, config.model.as_deref());
    push_optional(target, config.effort.map(effort_name));
    push_optional(target, config.mode.as_deref());
}

/// Return a stable, prompt-free hash of the configured graph shape and whether
/// every referenced agent's model/effort/mode configuration was observable
/// without resolving a backend.
pub fn workload_fingerprint_with_activation(
    graph: &WorkflowGraph,
    activation: DeadlineActivationV2,
    mut configured_effective: impl FnMut(&AgentId) -> Option<EffectiveConfig>,
) -> (String, bool) {
    use std::fmt::Write as _;
    let mut canonical = String::new();
    push_field(&mut canonical, graph.id.as_str());
    let mut nodes: Vec<_> = graph.nodes.iter().collect();
    nodes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut complete = true;
    for node in nodes {
        canonical.push('n');
        push_field(&mut canonical, node.id.as_str());
        push_field(&mut canonical, node.agent.as_str());
        match configured_effective(&node.agent) {
            Some(config) => {
                canonical.push('1');
                push_effective_config(&mut canonical, &config);
            }
            None => {
                complete = false;
                canonical.push('0');
            }
        }
        let mut inputs: Vec<_> = node.inputs.iter().map(NodeId::as_str).collect();
        inputs.sort_unstable();
        let _ = write!(&mut canonical, "i{}:", inputs.len());
        for input in inputs {
            push_field(&mut canonical, input);
        }
        match &node.retry {
            Some(retry) => {
                canonical.push('1');
                let _ = write!(
                    &mut canonical,
                    "{}:{}:{};",
                    retry.max_attempts,
                    retry.backoff_ms,
                    retry.backoff_cap_ms.unwrap_or(u64::MAX)
                );
            }
            None => canonical.push('0'),
        }
    }
    match activation {
        DeadlineActivationV2::ManualOnlyR2f1a => {}
        DeadlineActivationV2::AutomaticR2f1b => {
            canonical.push('a');
            push_field(&mut canonical, "automatic_r2f1b");
        }
    }
    if let Some(panel) = &graph.panel {
        canonical.push('p');
        for (key, value) in &panel.weights {
            push_field(&mut canonical, key);
            let _ = write!(&mut canonical, "{:016x};", value.to_bits());
        }
    } else {
        canonical.push('q');
    }
    (
        bridge_core::workflow_history::fingerprint_workload_shape(canonical.as_bytes()),
        complete,
    )
}

pub fn workload_fingerprint_with(
    graph: &WorkflowGraph,
    configured_effective: impl FnMut(&AgentId) -> Option<EffectiveConfig>,
) -> (String, bool) {
    workload_fingerprint_with_activation(
        graph,
        deadline_activation_v2_for(
            scheduler_activation_readiness_v1(),
            PolicyActivationV1::Production,
        ),
        configured_effective,
    )
}

pub fn workload_fingerprint(graph: &WorkflowGraph, registry: &dyn AgentRegistry) -> (String, bool) {
    workload_fingerprint_with(graph, |agent| registry.configured_effective(agent))
}

impl WorkflowGraph {
    /// Validate: non-empty, unique node ids, all `inputs` reference real nodes,
    /// acyclic, exactly one terminal (no other node lists it in `inputs`).
    pub fn validate(&self) -> Result<(), WorkflowError> {
        if self.nodes.is_empty() {
            return Err(WorkflowError::Empty);
        }
        let mut seen = HashSet::new();
        for n in &self.nodes {
            if !seen.insert(n.id.as_str()) {
                return Err(WorkflowError::DuplicateNode(n.id.as_str().into()));
            }
        }
        let ids: HashSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        for n in &self.nodes {
            for inp in &n.inputs {
                if !ids.contains(inp.as_str()) {
                    return Err(WorkflowError::UnknownInput {
                        node: n.id.as_str().into(),
                        input: inp.as_str().into(),
                    });
                }
            }
        }
        self.assert_acyclic()?;
        let referenced: HashSet<&str> = self
            .nodes
            .iter()
            .flat_map(|n| n.inputs.iter().map(|i| i.as_str()))
            .collect();
        let terminals = self
            .nodes
            .iter()
            .filter(|n| !referenced.contains(n.id.as_str()))
            .count();
        if terminals != 1 {
            return Err(WorkflowError::NotSingleTerminal(terminals));
        }
        Ok(())
    }

    /// The single terminal node (call only after `validate`).
    pub fn terminal(&self) -> Option<&WorkflowNode> {
        let referenced: HashSet<&str> = self
            .nodes
            .iter()
            .flat_map(|n| n.inputs.iter().map(|i| i.as_str()))
            .collect();
        self.nodes
            .iter()
            .find(|n| !referenced.contains(n.id.as_str()))
    }

    fn assert_acyclic(&self) -> Result<(), WorkflowError> {
        // Kahn's algorithm: repeatedly remove nodes whose inputs are all already removed.
        let mut remaining: Vec<&WorkflowNode> = self.nodes.iter().collect();
        let mut done: HashSet<&str> = HashSet::new();
        while !remaining.is_empty() {
            let ready: Vec<&str> = remaining
                .iter()
                .filter(|n| n.inputs.iter().all(|i| done.contains(i.as_str())))
                .map(|n| n.id.as_str())
                .collect();
            if ready.is_empty() {
                return Err(WorkflowError::Cyclic);
            }
            for r in &ready {
                done.insert(r);
            }
            remaining.retain(|n| !ready.contains(&n.id.as_str()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::ids::{AgentId, NodeId, WorkflowId};

    fn node(id: &str, agent: &str, inputs: &[&str]) -> WorkflowNode {
        WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(agent).unwrap(),
            prompt_template: format!("{{{{input}}}} {}", id),
            inputs: inputs.iter().map(|i| NodeId::parse(*i).unwrap()).collect(),
            retry: None,
            harvest_sanitization: None,
        }
    }

    #[test]
    fn valid_review_graph_has_single_terminal() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("code-review").unwrap(),
            nodes: vec![
                node("codex", "codex", &[]),
                node("claude", "claude", &[]),
                node("synth", "claude", &["codex", "claude"]),
            ],
            panel: None,
            controls: None,
        };
        g.validate().unwrap();
        assert_eq!(g.terminal().unwrap().id.as_str(), "synth");
    }
    #[test]
    fn rejects_cycle() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &["b"]), node("b", "x", &["a"])],
            panel: None,
            controls: None,
        };
        assert!(matches!(g.validate(), Err(WorkflowError::Cyclic)));
    }
    #[test]
    fn rejects_multi_terminal() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &[]), node("b", "x", &[])],
            panel: None,
            controls: None,
        };
        assert!(matches!(
            g.validate(),
            Err(WorkflowError::NotSingleTerminal(_))
        ));
    }
    #[test]
    fn rejects_unknown_input_ref() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &["ghost"])],
            panel: None,
            controls: None,
        };
        assert!(matches!(
            g.validate(),
            Err(WorkflowError::UnknownInput { .. })
        ));
    }
    #[test]
    fn rejects_duplicate_node_id() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &[]), node("a", "x", &[])],
            panel: None,
            controls: None,
        };
        assert!(matches!(g.validate(), Err(WorkflowError::DuplicateNode(_))));
    }

    #[test]
    fn graph_serde_roundtrip() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("a").unwrap(),
                agent: AgentId::parse("x").unwrap(),
                prompt_template: "t {{input}}".into(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: None,
            }],
            panel: None,
            controls: None,
        };
        let s = serde_json::to_string(&g).unwrap();
        let g2: WorkflowGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(g2.nodes.len(), 1);
        assert_eq!(g2.nodes[0].id.as_str(), "a");
    }

    #[test]
    fn graph_panel_serde_is_additive() {
        let mut weights = std::collections::BTreeMap::new();
        weights.insert("usage".to_string(), 0.2);
        weights.insert("benefit".to_string(), 0.4);
        let g = WorkflowGraph {
            id: WorkflowId::parse("panel").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("a").unwrap(),
                agent: AgentId::parse("x").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
                retry: None,
                harvest_sanitization: None,
            }],
            panel: Some(PanelConfig { weights }),
            controls: None,
        };
        let s = serde_json::to_string(&g).unwrap();
        assert!(s.contains("\"benefit\":0.4"));
        let back: WorkflowGraph = serde_json::from_str(&s).unwrap();
        assert_eq!(back.panel.unwrap().weights["usage"], 0.2);

        let old: WorkflowGraph = serde_json::from_str(
            r#"{"id":"w","nodes":[{"id":"a","agent":"x","prompt_template":"{{input}}","inputs":[]}]}"#,
        )
        .unwrap();
        assert!(old.panel.is_none());
    }

    #[test]
    fn retry_policy_rides_the_spec_snapshot_round_trip() {
        let node = WorkflowNode {
            id: NodeId::parse("n1").unwrap(),
            agent: AgentId::parse("codex").unwrap(),
            prompt_template: "p".into(),
            inputs: vec![],
            retry: Some(RetryPolicy {
                max_attempts: 3,
                backoff_ms: 500,
                backoff_cap_ms: Some(30_000),
            }),
            harvest_sanitization: None,
        };

        let json = serde_json::to_string(&node).unwrap();
        let back: WorkflowNode = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.retry,
            Some(RetryPolicy {
                max_attempts: 3,
                backoff_ms: 500,
                backoff_cap_ms: Some(30_000),
            })
        );

        let no_retry: WorkflowNode = serde_json::from_str(
            r#"{"id":"n1","agent":"codex","prompt_template":"p","inputs":[]}"#,
        )
        .unwrap();
        assert_eq!(no_retry.retry, None);
    }

    #[test]
    fn backoff_for_is_overflow_safe() {
        let capped = RetryPolicy {
            max_attempts: 5,
            backoff_ms: 500,
            backoff_cap_ms: Some(30_000),
        };

        assert_eq!(capped.backoff_for(1), std::time::Duration::from_millis(500));
        assert_eq!(
            capped.backoff_for(10),
            std::time::Duration::from_millis(30_000)
        );
        assert_eq!(
            capped.backoff_for(64),
            std::time::Duration::from_millis(30_000)
        );
        assert_eq!(
            RetryPolicy {
                max_attempts: 0,
                backoff_ms: 500,
                backoff_cap_ms: None,
            }
            .attempts(),
            1
        );
        assert_eq!(capped.attempts(), 5);
    }

    fn configured(model: &str) -> EffectiveConfig {
        EffectiveConfig {
            model: Some(model.to_owned()),
            effort: Some(Effort::High),
            mode: Some("default".to_owned()),
        }
    }

    #[test]
    fn workload_fingerprint_is_prompt_free_and_node_order_stable() {
        let first = WorkflowGraph {
            id: WorkflowId::parse("review").unwrap(),
            nodes: vec![
                node("draft", "codex", &[]),
                node("synth", "claude", &["draft"]),
            ],
            panel: None,
            controls: None,
        };
        let mut second = first.clone();
        second.nodes.reverse();
        second.nodes[0].prompt_template = "entirely different secret prompt".into();
        second.nodes[1].prompt_template = "another prompt".into();

        let lookup = |agent: &AgentId| match agent.as_str() {
            "codex" => Some(configured("gpt-5.5")),
            "claude" => Some(configured("claude-sonnet")),
            _ => None,
        };
        let left = workload_fingerprint_with(&first, lookup);
        let right = workload_fingerprint_with(&second, lookup);
        assert_eq!(left, right);
        assert!(left.1);
    }

    #[test]
    fn workload_fingerprint_partitions_config_topology_and_unknown_config() {
        let base = WorkflowGraph {
            id: WorkflowId::parse("review").unwrap(),
            nodes: vec![
                node("draft", "codex", &[]),
                node("synth", "claude", &["draft"]),
            ],
            panel: None,
            controls: None,
        };
        let baseline = workload_fingerprint_with(&base, |agent| {
            Some(configured(match agent.as_str() {
                "codex" => "gpt-5.5",
                _ => "claude-sonnet",
            }))
        });

        let model_changed = workload_fingerprint_with(&base, |agent| {
            Some(configured(match agent.as_str() {
                "codex" => "gpt-5.6",
                _ => "claude-sonnet",
            }))
        });
        assert_ne!(baseline.0, model_changed.0);

        let mut topology_changed = base.clone();
        topology_changed.nodes[1].inputs.clear();
        let topology =
            workload_fingerprint_with(&topology_changed, |_| Some(configured("same-model")));
        let same_config_base = workload_fingerprint_with(&base, |_| Some(configured("same-model")));
        assert_ne!(same_config_base.0, topology.0);

        let unknown = workload_fingerprint_with(&base, |agent| {
            (agent.as_str() == "codex").then(|| configured("gpt-5.5"))
        });
        assert!(!unknown.1);
        assert_ne!(baseline.0, unknown.0);
    }

    #[test]
    fn workload_fingerprint_partitions_deadline_activation_without_moving_manual_baseline() {
        use DeadlineActivationV2::{AutomaticR2f1b, ManualOnlyR2f1a};

        let graph = WorkflowGraph {
            id: WorkflowId::parse("review").unwrap(),
            nodes: vec![
                node("draft", "codex", &[]),
                node("synth", "claude", &["draft"]),
            ],
            panel: None,
            controls: None,
        };
        let lookup = |agent: &AgentId| {
            Some(configured(match agent.as_str() {
                "codex" => "gpt-5.5",
                _ => "claude-sonnet",
            }))
        };
        let shipped = workload_fingerprint_with(&graph, lookup);
        let manual = workload_fingerprint_with_activation(&graph, ManualOnlyR2f1a, lookup);
        let automatic = workload_fingerprint_with_activation(&graph, AutomaticR2f1b, lookup);

        assert_eq!(shipped, manual, "shipped readiness must remain manual");
        assert_eq!(
            manual.0,
            "shape-9892a9f12f1daf2edcc832b7f85437b937abd389e6691cad09c2f0bb0467b1c4"
        );
        assert_ne!(manual.0, automatic.0);
    }
}
