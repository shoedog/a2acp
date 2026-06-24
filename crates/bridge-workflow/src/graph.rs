//! Workflow DAG types + validation. Edges are implicit from each node's `inputs`.
use bridge_core::ids::{AgentId, NodeId, WorkflowId};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PanelConfig {
    #[serde(default)]
    pub weights: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelConfig>,
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
            self.backoff_ms
                .checked_mul(1u64 << shift)
                .unwrap_or(u64::MAX)
        };
        std::time::Duration::from_millis(base.min(cap))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,
    pub prompt_template: String,
    pub inputs: Vec<NodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowError {
    Empty,
    DuplicateNode(String),
    UnknownInput { node: String, input: String },
    Cyclic,
    NotSingleTerminal(usize),
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
        };
        assert!(matches!(g.validate(), Err(WorkflowError::Cyclic)));
    }
    #[test]
    fn rejects_multi_terminal() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &[]), node("b", "x", &[])],
            panel: None,
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
            }],
            panel: None,
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
            }],
            panel: Some(PanelConfig { weights }),
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
}
