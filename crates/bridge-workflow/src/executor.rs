//! WorkflowExecutor — runs a validated DAG over the registry. Each node: configure_session
//! → prompt → concatenate Update::Text (NOT the translator's last_text). Cancel via token.
use crate::graph::{WorkflowGraph, WorkflowNode};
use crate::template::render;
use bridge_core::domain::{effective_config, Part};
use bridge_core::error::BridgeError;
use bridge_core::ids::{NodeId, SessionId};
use bridge_core::ports::{AgentRegistry, Update, STOP_REASON_CANCELLED};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct WorkflowExecutor { registry: Arc<dyn AgentRegistry> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutcome { Completed, Failed, Canceled }

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    NodeStarted { node: NodeId },
    NodeFinished { node: NodeId, ok: bool },
    Terminal { outcome: WorkflowOutcome, output: String },
}

pub type WorkflowStream = std::pin::Pin<Box<dyn futures::Stream<Item = Result<WorkflowEvent, BridgeError>> + Send>>;

impl WorkflowExecutor {
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self { Self { registry } }

    /// Run one node: render its prompt from `vars`, resolve+configure+prompt+drain, forget.
    /// Returns (text, ok). On any failure returns the error marker + ok=false.
    async fn run_node(&self, wf_id: &str, node: &WorkflowNode, vars: &HashMap<&str, &str>, run_id: &str,
                      cancel: &CancellationToken) -> (String, bool) {
        if cancel.is_cancelled() { return (format!("[node {} canceled]", node.id.as_str()), false); }
        let rendered = render(&node.prompt_template, vars);
        let session = match SessionId::parse(format!("workflow-{}-{}-{}", wf_id, node.id.as_str(), run_id)) {
            Ok(s) => s, Err(_) => return (format!("[node {} failed: bad session id]", node.id.as_str()), false),
        };
        let resolved = match self.registry.resolve(&node.agent).await {
            Ok(r) => r, Err(e) => return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false),
        };
        let eff = effective_config(&resolved.entry, None);
        let _ = resolved.backend.configure_session(&session, &eff).await;
        let mut stream = match resolved.backend.prompt(&session, vec![Part { text: rendered }]).await {
            Ok(s) => s, Err(e) => { resolved.backend.forget_session(&session).await;
                return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false); }
        };
        let mut text = String::new();
        let mut ok = true;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = resolved.backend.cancel(&session).await;
                    ok = false; text = format!("[node {} canceled]", node.id.as_str()); break;
                }
                item = stream.next() => match item {
                    Some(Ok(Update::Text(t))) => text.push_str(&t),
                    Some(Ok(Update::Permission(_))) => {} // safe: backends resolve permission internally
                    Some(Ok(Update::Done { stop_reason })) => {
                        if stop_reason == STOP_REASON_CANCELLED { ok = false; }
                        break;
                    }
                    Some(Err(e)) => { ok = false; text = format!("[node {} failed: {:?}]", node.id.as_str(), e); break; }
                    None => break,
                }
            }
        }
        resolved.backend.forget_session(&session).await;
        (text, ok)
    }

    pub fn run(&self, graph: Arc<WorkflowGraph>, input: String, run_id: String, cancel: CancellationToken) -> WorkflowStream {
        let this = WorkflowExecutor { registry: self.registry.clone() };
        Box::pin(async_stream::stream! {
            // Task 4 replaces this with parallel topo scheduling. Single-node milestone:
            // run nodes in declaration order, each consuming `{{input}}` (+ any ready inputs).
            let mut outputs: HashMap<String, (String, bool)> = HashMap::new();
            let mut terminal_output = String::new();
            let mut terminal_ok = true;
            for node in &graph.nodes {
                yield Ok(WorkflowEvent::NodeStarted { node: node.id.clone() });
                // Clone upstream outputs into owned strings so we can borrow `&str` from them
                // across the `.await` in `run_node` without fighting the borrow checker.
                let mut owned_vars: Vec<(String, String)> = vec![("input".to_string(), input.clone())];
                for inp in &node.inputs {
                    if let Some((t, _)) = outputs.get(inp.as_str()) {
                        owned_vars.push((inp.as_str().to_string(), t.clone()));
                    }
                }
                let vars: HashMap<&str, &str> = owned_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                let (text, ok) = this.run_node(graph.id.as_str(), node, &vars, &run_id, &cancel).await;
                yield Ok(WorkflowEvent::NodeFinished { node: node.id.clone(), ok });
                terminal_output = text.clone(); terminal_ok = ok;
                outputs.insert(node.id.as_str().to_string(), (text, ok));
            }
            let outcome = if cancel.is_cancelled() { WorkflowOutcome::Canceled }
                else if terminal_ok { WorkflowOutcome::Completed } else { WorkflowOutcome::Failed };
            yield Ok(WorkflowEvent::Terminal { outcome, output: terminal_output });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::{EffectiveConfig, Part, RegistrySnapshot};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, NodeId, SessionId, WorkflowId};
    use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};
    use crate::graph::{WorkflowGraph, WorkflowNode};
    use futures::StreamExt;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    pub(super) struct Rec { pub configured: Mutex<bool>, pub prompts: Mutex<Vec<String>>, pub cancels: Mutex<u32> }
    pub(super) struct FakeBackend { pub reply: String, pub rec: Arc<Rec> }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(&self, _s: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
            self.rec.prompts.lock().unwrap().push(parts.iter().map(|p| p.text.clone()).collect());
            let updates = vec![Ok(Update::Text(self.reply.clone())),
                               Ok(Update::Done { stop_reason: "end_turn".into() })];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { *self.rec.cancels.lock().unwrap() += 1; Ok(()) }
        async fn configure_session(&self, _s: &SessionId, _c: &EffectiveConfig) -> Result<(), BridgeError> {
            *self.rec.configured.lock().unwrap() = true; Ok(())
        }
    }
    pub(super) struct NoopLease; impl Lease for NoopLease {}
    pub(super) fn minimal_entry(id: &AgentId) -> bridge_core::domain::AgentEntry {
        bridge_core::domain::AgentEntry { id: id.clone(), cmd: Some("x".into()), base_url: None, api_key_env: None,
            args: vec![], kind: bridge_core::domain::AgentKind::Acp, model_provider: None, model: None, effort: None,
            mode: None, cwd: None, auth_method: None, name: None, description: None, tags: vec![], version: None,
            extensions: Default::default() }
    }
    pub(super) struct FakeRegistry { pub backends: std::collections::HashMap<String, (String, Arc<Rec>)> }
    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let (reply, rec) = self.backends.get(id.as_str()).cloned()
                .ok_or(BridgeError::UnknownAgent { id: id.as_str().into() })?;
            Ok(Resolved { entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(FakeBackend { reply, rec }), lease: Box::new(NoopLease) })
        }
        fn default_id(&self) -> AgentId { AgentId::parse("codex").unwrap() }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
        fn list(&self) -> Vec<AgentId> { vec![] }
    }
    pub(super) fn one_node_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph { id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode { id: NodeId::parse("only").unwrap(), agent: AgentId::parse("codex").unwrap(),
                prompt_template: "echo {{input}}".into(), inputs: vec![] }] })
    }

    #[tokio::test]
    async fn single_node_configures_renders_concatenates() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry { backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into() });
        let ex = WorkflowExecutor::new(reg);
        let mut events: Vec<WorkflowEvent> = ex.run(one_node_graph(), "DIFF".into(), "run1".into(), CancellationToken::new())
            .collect::<Vec<_>>().await.into_iter().map(|r| r.unwrap()).collect();
        let term = events.pop().unwrap();
        assert!(matches!(term, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "HELLO"));
        assert!(*rec.configured.lock().unwrap(), "configure_session called");
        assert_eq!(rec.prompts.lock().unwrap()[0], "echo DIFF", "template rendered with {{input}}");
    }
}
