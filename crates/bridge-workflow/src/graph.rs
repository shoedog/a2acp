//! Workflow DAG types + validation. Edges are implicit from each node's `inputs`.
use bridge_core::ids::{AgentId, NodeId, WorkflowId};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct WorkflowGraph {
    pub id: WorkflowId,
    pub nodes: Vec<WorkflowNode>,
}

#[derive(Debug, Clone)]
pub struct WorkflowNode {
    pub id: NodeId,
    pub agent: AgentId,
    pub prompt_template: String,
    pub inputs: Vec<NodeId>,
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
        };
        g.validate().unwrap();
        assert_eq!(g.terminal().unwrap().id.as_str(), "synth");
    }
    #[test]
    fn rejects_cycle() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &["b"]), node("b", "x", &["a"])],
        };
        assert!(matches!(g.validate(), Err(WorkflowError::Cyclic)));
    }
    #[test]
    fn rejects_multi_terminal() {
        let g = WorkflowGraph {
            id: WorkflowId::parse("c").unwrap(),
            nodes: vec![node("a", "x", &[]), node("b", "x", &[])],
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
        };
        assert!(matches!(g.validate(), Err(WorkflowError::DuplicateNode(_))));
    }
}
