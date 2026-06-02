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
        // resolve, with cancel
        let resolved = tokio::select! {
            biased;
            _ = cancel.cancelled() => return (format!("[node {} canceled]", node.id.as_str()), false),
            r = self.registry.resolve(&node.agent) => match r {
                Ok(r) => r,
                Err(e) => return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false),
            },
        };
        let eff = effective_config(&resolved.entry, None);
        let _ = resolved.backend.configure_session(&session, &eff).await; // best-effort (no-op default)
        if cancel.is_cancelled() {
            resolved.backend.forget_session(&session).await;
            return (format!("[node {} canceled]", node.id.as_str()), false);
        }
        // prompt, with cancel
        let mut stream = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                resolved.backend.forget_session(&session).await;
                return (format!("[node {} canceled]", node.id.as_str()), false);
            }
            s = resolved.backend.prompt(&session, vec![Part { text: rendered }]) => match s {
                Ok(s) => s,
                Err(e) => { resolved.backend.forget_session(&session).await;
                    return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false); }
            },
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
            let mut outputs: HashMap<String, (String, bool)> = HashMap::new();
            let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
            let terminal_id = graph.terminal().map(|n| n.id.as_str().to_string()).unwrap_or_default();
            while done.len() < graph.nodes.len() {
                if cancel.is_cancelled() { break; }   // stop scheduling downstream once canceled
                let ready: Vec<&WorkflowNode> = graph.nodes.iter()
                    .filter(|n| !done.contains(n.id.as_str())
                        && n.inputs.iter().all(|i| done.contains(i.as_str())))
                    .collect();
                if ready.is_empty() { break; } // validated acyclic, so unreachable
                for n in &ready { yield Ok(WorkflowEvent::NodeStarted { node: n.id.clone() }); }
                let futs = ready.iter().map(|n| {
                    let mut owned: Vec<(String, String)> = vec![("input".into(), input.clone())];
                    for inp in &n.inputs {
                        if let Some((t, _)) = outputs.get(inp.as_str()) { owned.push((inp.as_str().into(), t.clone())); }
                    }
                    let node = (*n).clone(); let run_id = run_id.clone(); let cancel = cancel.clone();
                    let wf_id = graph.id.as_str().to_string(); let this = &this;
                    async move {
                        let vars: HashMap<&str, &str> = owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                        let (text, ok) = this.run_node(&wf_id, &node, &vars, &run_id, &cancel).await;
                        (node.id.clone(), text, ok)
                    }
                });
                for (node_id, text, ok) in futures::future::join_all(futs).await {
                    yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok });
                    done.insert(node_id.as_str().to_string());
                    outputs.insert(node_id.as_str().to_string(), (text, ok));
                }
            }
            let (term_text, term_ok) = outputs.get(&terminal_id).cloned().unwrap_or_default();
            let outcome = if term_ok { WorkflowOutcome::Completed }
                else if cancel.is_cancelled() { WorkflowOutcome::Canceled }
                else { WorkflowOutcome::Failed };
            yield Ok(WorkflowEvent::Terminal { outcome, output: term_text });
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

    fn review_graph() -> Arc<WorkflowGraph> {
        let n = |id: &str, ag: &str, ins: &[&str], tpl: &str| WorkflowNode {
            id: NodeId::parse(id).unwrap(), agent: AgentId::parse(ag).unwrap(),
            prompt_template: tpl.into(), inputs: ins.iter().map(|i| NodeId::parse(*i).unwrap()).collect() };
        Arc::new(WorkflowGraph { id: WorkflowId::parse("code-review").unwrap(), nodes: vec![
            n("codex","codex",&[], "review {{input}}"),
            n("claude","claude",&[], "review {{input}}"),
            n("synth","synth",&["codex","claude"], "merge {{codex}} + {{claude}} for {{input}}"),
        ]})
    }

    #[tokio::test]
    async fn fan_in_synth_receives_both_reviews_and_input() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry { backends: [
            ("codex".to_string(), mk("CODEX_REVIEW")),
            ("claude".to_string(), mk("CLAUDE_REVIEW")),
            ("synth".to_string(), mk("FINAL")),
        ].into() });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex.run(review_graph(), "DIFF".into(), "r".into(), CancellationToken::new()).collect::<Vec<_>>().await;
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "FINAL"));
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(p.contains("CODEX_REVIEW") && p.contains("CLAUDE_REVIEW") && p.contains("DIFF"),
            "synth got both reviews + {{input}}: {p}");
    }

    #[tokio::test]
    async fn fan_out_runs_concurrently() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Barrier;
        // Both fan-out legs must ENTER prompt() before either replies → only possible if run in parallel.
        struct BarrierBackend { reply: String, barrier: Arc<Barrier> }
        #[async_trait::async_trait]
        impl AgentBackend for BarrierBackend {
            async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
                self.barrier.wait().await; // deadlocks unless the other leg also reaches here
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(self.reply.clone())), Ok(Update::Done { stop_reason: "end_turn".into() })])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { Ok(()) }
        }
        // BReg hands out BarrierBackend only for the first 2 resolves (the fan-out nodes);
        // node `t` (the terminal, resolved 3rd) gets a plain non-blocking backend so it
        // doesn't deadlock on a single-party wait.
        struct BReg { barrier: Arc<Barrier>, calls: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl AgentRegistry for BReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                let backend: Arc<dyn bridge_core::ports::AgentBackend> = if n < 2 {
                    Arc::new(BarrierBackend { reply: id.as_str().to_uppercase(), barrier: self.barrier.clone() })
                } else {
                    Arc::new(FakeBackend { reply: id.as_str().to_uppercase(), rec: Arc::new(Rec::default()) })
                };
                Ok(Resolved { entry: Arc::new(minimal_entry(id)), backend, lease: Box::new(NoopLease) })
            }
            fn default_id(&self) -> AgentId { AgentId::parse("a").unwrap() }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
            fn list(&self) -> Vec<AgentId> { vec![] }
        }
        // two-node graph: a, b both inputs=[] (fan-out), plus a terminal t depending on both.
        let g = Arc::new(WorkflowGraph { id: WorkflowId::parse("g").unwrap(), nodes: vec![
            WorkflowNode { id: NodeId::parse("a").unwrap(), agent: AgentId::parse("a").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] },
            WorkflowNode { id: NodeId::parse("b").unwrap(), agent: AgentId::parse("b").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] },
            WorkflowNode { id: NodeId::parse("t").unwrap(), agent: AgentId::parse("a").unwrap(), prompt_template: "{{a}}{{b}}".into(), inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()] },
        ]});
        let reg = Arc::new(BReg { barrier: Arc::new(Barrier::new(2)), calls: Arc::new(AtomicUsize::new(0)) }); // a + b must rendezvous
        let ex = WorkflowExecutor::new(reg);
        let res = tokio::time::timeout(std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), CancellationToken::new()).collect::<Vec<_>>()).await;
        assert!(res.is_ok(), "fan-out legs ran concurrently (no deadlock/timeout)");
    }

    #[tokio::test]
    async fn pipeline_threads_output_to_input() {
        // a -> b -> c ; b sees a's output, c sees b's.
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry { backends: [
            ("a".to_string(), mk("AOUT")), ("b".to_string(), mk("BOUT")), ("c".to_string(), mk("COUT")),
        ].into() });
        let b_rec = reg.backends.get("b").unwrap().1.clone();
        let c_rec = reg.backends.get("c").unwrap().1.clone();
        let g = Arc::new(WorkflowGraph { id: WorkflowId::parse("p").unwrap(), nodes: vec![
            WorkflowNode { id: NodeId::parse("a").unwrap(), agent: AgentId::parse("a").unwrap(), prompt_template: "{{input}}".into(), inputs: vec![] },
            WorkflowNode { id: NodeId::parse("b").unwrap(), agent: AgentId::parse("b").unwrap(), prompt_template: "got {{a}}".into(), inputs: vec![NodeId::parse("a").unwrap()] },
            WorkflowNode { id: NodeId::parse("c").unwrap(), agent: AgentId::parse("c").unwrap(), prompt_template: "got {{b}}".into(), inputs: vec![NodeId::parse("b").unwrap()] },
        ]});
        let ex = WorkflowExecutor::new(reg);
        let _ = ex.run(g, "x".into(), "r".into(), CancellationToken::new()).collect::<Vec<_>>().await;
        assert_eq!(b_rec.prompts.lock().unwrap()[0], "got AOUT");
        assert_eq!(c_rec.prompts.lock().unwrap()[0], "got BOUT");
    }

    #[tokio::test]
    async fn failed_fan_out_leg_marker_reaches_synth_and_run_completes() {
        // No "codex" backend registered → the codex node's resolve fails → error marker;
        // claude + synth still run (graceful degradation).
        let reg = Arc::new(FakeRegistry { backends: [
            ("claude".to_string(), ("CLAUDE_REVIEW".to_string(), Arc::new(Rec::default()))),
            ("synth".to_string(),  ("FINAL".to_string(),         Arc::new(Rec::default()))),
            // NOTE: no "codex" → resolve fails for the codex node
        ].into() });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex.run(review_graph(), "DIFF".into(), "r".into(), CancellationToken::new()).collect::<Vec<_>>().await;
        // run COMPLETES (terminal synth ok) — graceful degradation
        assert!(matches!(evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, .. }));
        // a NodeFinished{ok:false} was emitted for codex
        assert!(evs.iter().any(|e| matches!(e.as_ref().unwrap(),
            WorkflowEvent::NodeFinished { node, ok: false } if node.as_str() == "codex")));
        // the EXACT failure marker reached synth's prompt
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(p.contains("[node codex failed:"), "marker reached synth: {p}");
    }

    #[tokio::test]
    async fn cancel_calls_backend_cancel_and_ends_canceled() {
        // A backend whose prompt() stream NEVER yields Done (pending) → only the cancel path ends it.
        struct Pending { rec: Arc<Rec> }
        #[async_trait::async_trait]
        impl AgentBackend for Pending {
            async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(futures::stream::pending())) // never yields
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { *self.rec.cancels.lock().unwrap() += 1; Ok(()) }
        }
        let rec = Arc::new(Rec::default());
        struct PReg { rec: Arc<Rec> }
        #[async_trait::async_trait]
        impl AgentRegistry for PReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved { entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(Pending { rec: self.rec.clone() }), lease: Box::new(NoopLease) })
            }
            fn default_id(&self) -> AgentId { AgentId::parse("a").unwrap() }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
            fn list(&self) -> Vec<AgentId> { vec![] }
        }
        let token = CancellationToken::new();
        let reg = Arc::new(PReg { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let t2 = token.clone();
        tokio::spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(20)).await; t2.cancel(); });
        let evs: Vec<_> = tokio::time::timeout(std::time::Duration::from_secs(2),
            ex.run(one_node_graph(), "x".into(), "r".into(), token).collect::<Vec<_>>()).await.unwrap();
        assert!(matches!(evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal { outcome: WorkflowOutcome::Canceled, .. }));
        assert_eq!(*rec.cancels.lock().unwrap(), 1, "backend.cancel was called for the in-flight node");
    }

    #[tokio::test]
    async fn cancel_during_slow_prompt_ends_canceled_promptly() {
        struct SlowPrompt;
        #[async_trait::async_trait]
        impl AgentBackend for SlowPrompt {
            async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await; // long setup
                Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done { stop_reason: "end_turn".into() })])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> { Ok(()) }
        }
        struct SReg;
        #[async_trait::async_trait]
        impl AgentRegistry for SReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved { entry: Arc::new(minimal_entry(id)), backend: Arc::new(SlowPrompt), lease: Box::new(NoopLease) })
            }
            fn default_id(&self) -> AgentId { AgentId::parse("a").unwrap() }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> { Ok(()) }
            fn list(&self) -> Vec<AgentId> { vec![] }
        }
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move { tokio::time::sleep(std::time::Duration::from_millis(20)).await; t2.cancel(); });
        let ex = WorkflowExecutor::new(Arc::new(SReg));
        // Must finish well under the 10s prompt sleep → the cancel preempted setup.
        let evs = tokio::time::timeout(std::time::Duration::from_secs(2),
            ex.run(one_node_graph(), "x".into(), "r".into(), token).collect::<Vec<_>>()).await
            .expect("cancel preempts the slow prompt setup");
        assert!(matches!(evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal { outcome: WorkflowOutcome::Canceled, .. }));
    }
}
