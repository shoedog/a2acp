//! WorkflowExecutor — runs a validated DAG over the registry. Each node: configure_session
//! → prompt → concatenate Update::Text (NOT the translator's last_text). Cancel via token.
use crate::graph::{WorkflowGraph, WorkflowNode};
use crate::template::render;
use bridge_core::domain::{effective_config, Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{NodeId, SessionId};
use bridge_core::orch::UsageSnapshot;
use bridge_core::ports::{
    AgentBackend, AgentRegistry, RichEventSinkFactory, Update, STOP_REASON_CANCELLED,
};
use bridge_core::SessionCwd;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Per-request context forwarded opaquely through the executor to each node's
/// `configure_session` call. The scheduler/topo logic MUST NOT read this — it
/// is only consumed at the `SessionSpec` build site in `run_node`.
#[derive(Default, Clone)]
pub struct WorkflowRunContext {
    pub session_cwd: Option<SessionCwd>,
    pub make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>,
}

pub enum NodeTurnExit {
    Normal,
    Canceled,
    Error(BridgeError),
}

#[async_trait::async_trait]
pub trait NodeTurnCleanup: Send {
    /// Invoked once after prompt+drain on the node's exit branch. Each impl closes over what it owns
    /// (cold: backend+session for forget; warm: SessionManager+child+gen+op for finish/cancel/expire).
    async fn on_exit(self: Box<Self>, exit: NodeTurnExit);
}

pub struct NodeTurn {
    pub backend: Arc<dyn AgentBackend>,
    pub session: SessionId,
    pub seed: Option<String>, // warm-only; prepended to the node prompt parts (Slice-4 seed)
    pub cleanup: Box<dyn NodeTurnCleanup>,
}

#[async_trait::async_trait]
pub trait WorkflowNodeDispatcher: Send + Sync {
    async fn checkout(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
    ) -> Result<NodeTurn, BridgeError>;
}

/// Uniform future type used in the per-run `FuturesUnordered` pool.
/// Each fan-out node is boxed to this type so `FuturesUnordered` can hold
/// futures of different async-block monomorphisations in one collection.
type NodeFut<'a> = std::pin::Pin<
    Box<dyn futures::Future<Output = (NodeId, String, bool, Option<UsageSnapshot>)> + Send + 'a>,
>;

/// Render the reserved `{{workflow.costs}}` synth var: a markdown table of each
/// input source's captured usage. Per-field `n/a` when absent.
/// `windowFraction = used/size` as a raw fraction.
pub(crate) fn render_costs_table(rows: &[(String, Option<UsageSnapshot>)]) -> String {
    let mut table = String::from(
        "| source | used | size | windowFraction | cost |\n| --- | --- | --- | --- | --- |\n",
    );
    for (source, usage) in rows {
        let (used, size, window_fraction, cost) = match usage {
            Some(usage) => {
                let used = usage
                    .used
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into());
                let size = usage
                    .size
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into());
                let window_fraction = match (usage.used, usage.size) {
                    (Some(used), Some(size)) if size > 0 => {
                        format!("{:.4}", used as f64 / size as f64)
                    }
                    _ => "n/a".into(),
                };
                let cost = usage
                    .cost
                    .as_ref()
                    .map(|cost| format!("{} {}", cost.amount, cost.currency))
                    .unwrap_or_else(|| "n/a".into());
                (used, size, window_fraction, cost)
            }
            None => ("n/a".into(), "n/a".into(), "n/a".into(), "n/a".into()),
        };
        table.push_str(&format!(
            "| {source} | {used} | {size} | {window_fraction} | {cost} |\n"
        ));
    }
    table
}

pub(crate) fn render_weights(panel: &Option<crate::graph::PanelConfig>) -> String {
    match panel {
        Some(panel) if !panel.weights.is_empty() => {
            let mut rendered = String::new();
            for (key, value) in &panel.weights {
                rendered.push_str(&format!("- {key}: {value}\n"));
            }
            rendered
        }
        _ => "(no weights configured)".to_string(),
    }
}

pub struct WorkflowExecutor {
    registry: Arc<dyn AgentRegistry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowOutcome {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub enum WorkflowEvent {
    NodeStarted {
        node: NodeId,
    },
    NodeFinished {
        node: NodeId,
        ok: bool,
        output: String,
        usage: Option<bridge_core::orch::UsageSnapshot>,
    },
    Terminal {
        outcome: WorkflowOutcome,
        output: String,
    },
}

pub type WorkflowStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<WorkflowEvent, BridgeError>> + Send>>;

impl WorkflowExecutor {
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self {
        Self { registry }
    }

    /// Run one node: render its prompt from `vars`, resolve+configure+prompt+drain, forget.
    /// Returns (text, ok, usage). On any failure returns the error marker + ok=false.
    #[allow(clippy::too_many_arguments)]
    async fn run_node(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        vars: &HashMap<&str, &str>,
        run_id: &str,
        cancel: &CancellationToken,
        ctx: &WorkflowRunContext,
        dispatcher: Option<&Arc<dyn WorkflowNodeDispatcher>>,
    ) -> (String, bool, Option<UsageSnapshot>) {
        if cancel.is_cancelled() {
            return (format!("[node {} canceled]", node.id.as_str()), false, None);
        }
        if let Some(d) = dispatcher {
            let rendered = render(&node.prompt_template, vars);
            let turn = match d.checkout(wf_id, node, run_id, ctx).await {
                Ok(t) => t,
                Err(e) => {
                    return (
                        format!("[node {} failed: {:?}]", node.id.as_str(), e),
                        false,
                        None,
                    )
                }
            };
            if cancel.is_cancelled() {
                turn.cleanup.on_exit(NodeTurnExit::Normal).await;
                return (format!("[node {} canceled]", node.id.as_str()), false, None);
            }

            let mut parts = vec![Part { text: rendered }];
            if let Some(seed) = turn.seed {
                parts.insert(
                    0,
                    Part {
                        text: format!("[Summary of earlier context in this session]\n{seed}"),
                    },
                );
            }

            let mut stream = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    turn.cleanup.on_exit(NodeTurnExit::Canceled).await;
                    return (format!("[node {} canceled]", node.id.as_str()), false, None);
                }
                s = turn.backend.prompt(&turn.session, parts) => match s {
                    Ok(s) => s,
                    Err(e) => {
                        let text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                        turn.cleanup.on_exit(NodeTurnExit::Error(e)).await;
                        return (text, false, None);
                    }
                },
            };
            let mut text = String::new();
            let mut ok = true;
            let mut last_usage: Option<UsageSnapshot> = None;
            let exit = loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        ok = false;
                        text = format!("[node {} canceled]", node.id.as_str());
                        break NodeTurnExit::Canceled;
                    }
                    item = stream.next() => match item {
                        Some(Ok(Update::Text(t))) => text.push_str(&t),
                        Some(Ok(Update::Permission(_))) => {}
                        Some(Ok(Update::Usage(u))) => {
                            last_usage = Some(u);
                        }
                        Some(Ok(Update::Done { stop_reason })) => {
                            if stop_reason == STOP_REASON_CANCELLED { ok = false; }
                            break NodeTurnExit::Normal;
                        }
                        Some(Err(e)) => {
                            ok = false;
                            text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                            break NodeTurnExit::Error(e);
                        }
                        None => break NodeTurnExit::Normal,
                    }
                }
            };
            // Keep whatever usage the agent reported, even if the turn then errored or was
            // cancelled — the tokens were really consumed and belong in the durable footprint.
            // `last_usage` is already `None` when no `Update::Usage` was ever observed.
            turn.cleanup.on_exit(exit).await;
            return (text, ok, last_usage);
        }
        let rendered = render(&node.prompt_template, vars);
        let session = match SessionId::parse(format!(
            "workflow-{}-{}-{}",
            wf_id,
            node.id.as_str(),
            run_id
        )) {
            Ok(s) => s,
            Err(_) => {
                return (
                    format!("[node {} failed: bad session id]", node.id.as_str()),
                    false,
                    None,
                )
            }
        };
        // resolve, with cancel
        let resolved = tokio::select! {
            biased;
            _ = cancel.cancelled() => return (format!("[node {} canceled]", node.id.as_str()), false, None),
            r = self.registry.resolve(&node.agent) => match r {
                Ok(r) => r,
                Err(e) => return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false, None),
            },
        };
        let eff = effective_config(&resolved.entry, None);
        if let Err(e) = resolved
            .backend
            .configure_session(
                &session,
                &SessionSpec {
                    config: eff,
                    cwd: ctx.session_cwd.clone(),
                },
            )
            .await
        {
            resolved.backend.forget_session(&session).await;
            return (
                format!("[node {} failed: configure {:?}]", node.id.as_str(), e),
                false,
                None,
            );
        }
        if cancel.is_cancelled() {
            resolved.backend.forget_session(&session).await;
            return (format!("[node {} canceled]", node.id.as_str()), false, None);
        }
        // prompt, with cancel
        let rich_sink;
        let mut stream = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                resolved.backend.forget_session(&session).await;
                return (format!("[node {} canceled]", node.id.as_str()), false, None);
            }
            s = async {
                let sink = ctx.make_rich_sink.as_ref().map(|factory| factory.make(&node.id));
                let parts = vec![Part { text: rendered }];
                let stream = match &sink {
                    Some(sink) => resolved.backend.prompt_observed(&session, parts, sink.clone()).await,
                    None => resolved.backend.prompt(&session, parts).await,
                };
                (sink, stream)
            } => match s {
                (sink, Ok(s)) => {
                    rich_sink = sink;
                    s
                }
                (sink, Err(e)) => {
                    if let Some(sink) = &sink {
                        if let Err(flush_err) = sink.flush().await {
                            eprintln!(
                                "rich sink flush failed after prompt error for node {}: {:?}",
                                node.id.as_str(),
                                flush_err
                            );
                        }
                    }
                    resolved.backend.forget_session(&session).await;
                    return (format!("[node {} failed: {:?}]", node.id.as_str(), e), false, None);
                }
            },
        };
        let mut text = String::new();
        let mut ok = true;
        let mut canceled_during_drain = false;
        let mut last_usage: Option<UsageSnapshot> = None;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    let _ = resolved.backend.cancel(&session).await;
                    canceled_during_drain = true;
                    ok = false; text = format!("[node {} canceled]", node.id.as_str()); break;
                }
                item = stream.next() => match item {
                    Some(Ok(Update::Text(t))) => text.push_str(&t),
                    Some(Ok(Update::Permission(_))) => {} // safe: backends resolve permission internally
                    Some(Ok(Update::Usage(u))) => {
                        last_usage = Some(u);
                    }
                    Some(Ok(Update::Done { stop_reason })) => {
                        if stop_reason == STOP_REASON_CANCELLED { ok = false; }
                        break;
                    }
                    Some(Err(e)) => { ok = false; text = format!("[node {} failed: {:?}]", node.id.as_str(), e); break; }
                    None => break,
                }
            }
        }
        // Keep whatever usage the agent reported, even on error/cancel (see the warm path):
        // `last_usage` is `None` only when no `Update::Usage` was ever observed.
        let usage = last_usage;
        if let Some(sink) = &rich_sink {
            if let Err(e) = sink.flush().await {
                if !ok {
                    let exit = if canceled_during_drain {
                        "node cancellation"
                    } else {
                        "node failure"
                    };
                    eprintln!(
                        "rich sink flush failed after {exit} for node {}: {:?}",
                        node.id.as_str(),
                        e
                    );
                    resolved.backend.forget_session(&session).await;
                    return (text, ok, None);
                }
                resolved.backend.forget_session(&session).await;
                return (
                    format!("[node {} rich-flush failed: {:?}]", node.id.as_str(), e),
                    false,
                    None,
                );
            }
        }
        resolved.backend.forget_session(&session).await;
        (text, ok, usage)
    }

    /// Run a workflow from scratch (no prior checkpoints).
    /// Thin wrapper over [`run_from`](Self::run_from) with an empty seed and default context.
    pub fn run(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
    ) -> WorkflowStream {
        self.run_with_context(graph, input, run_id, cancel, WorkflowRunContext::default())
    }

    /// Run a workflow from scratch with an explicit per-request context.
    /// Thin wrapper over [`run_from_with_context`](Self::run_from_with_context) with an empty seed.
    pub fn run_with_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowRunContext,
    ) -> WorkflowStream {
        self.run_from_with_context(graph, input, run_id, cancel, HashMap::new(), ctx)
    }

    pub fn run_with_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowRunContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_from_with_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            HashMap::new(),
            ctx,
            dispatcher,
        )
    }

    /// Resume a workflow from a pre-loaded seed of already-completed node outputs.
    /// Seeded nodes are treated as done; only un-seeded nodes actually run.
    /// `run()` is a thin wrapper over this with an empty seed and default context.
    ///
    /// Each seed entry is `(output_text, ok, usage)`, matching the `NodeFinished` payload.
    ///
    /// # Errors (streamed)
    /// - `BridgeError::ConfigInvalid` if a seed key is not in `graph.nodes`.
    /// - `BridgeError::ConfigInvalid` if the seed is not closed under `inputs`
    ///   (a non-root seeded node's upstream is missing from the seed).
    pub fn run_from(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
    ) -> WorkflowStream {
        self.run_from_with_context(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowRunContext::default(),
        )
    }

    /// Resume a workflow from a pre-loaded seed with an explicit per-request context.
    /// The context is forwarded opaquely to each node's `configure_session` call
    /// (via `SessionSpec.cwd`). The scheduling/topo logic does NOT read it.
    pub fn run_from_with_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
    ) -> WorkflowStream {
        self.run_from_with_context_inner(graph, input, run_id, cancel, seed, ctx, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_from_with_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_from_with_context_inner(graph, input, run_id, cancel, seed, ctx, Some(dispatcher))
    }

    #[allow(clippy::too_many_arguments)]
    fn run_from_with_context_inner(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowRunContext,
        dispatcher: Option<Arc<dyn WorkflowNodeDispatcher>>,
    ) -> WorkflowStream {
        let this = WorkflowExecutor {
            registry: self.registry.clone(),
        };
        Box::pin(async_stream::stream! {
            // --- Seed validation ---
            // 1. Every seed key must name a real node.
            let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
            for key in seed.keys() {
                if !node_ids.contains(key.as_str()) {
                    yield Err(BridgeError::ConfigInvalid {
                        reason: "resume seed references unknown node".into(),
                    });
                    return;
                }
            }
            // 2. The seed must be closed under inputs: for every seeded non-root node,
            //    all of its declared inputs must also be in the seed.
            for node in graph.nodes.iter() {
                if seed.contains_key(node.id.as_str()) {
                    for inp in &node.inputs {
                        if !seed.contains_key(inp.as_str()) {
                            yield Err(BridgeError::ConfigInvalid {
                                reason: "resume seed is not closed under inputs".into(),
                            });
                            return;
                        }
                    }
                }
            }

            let mut outputs: HashMap<String, (String, bool, Option<UsageSnapshot>)> = seed;
            let mut done: HashSet<String> = outputs.keys().cloned().collect();
            let terminal_id = graph.terminal().map(|n| n.id.as_str().to_string()).unwrap_or_default();

            // Box the per-node future to one uniform type: the `schedule_ready!`
            // macro expands at two textual sites and each bare `async move {}`
            // would otherwise be a *distinct* anonymous type, which a monomorphic
            // `FuturesUnordered<Fut>` cannot hold.
            let mut inflight: FuturesUnordered<NodeFut> = FuturesUnordered::new();
            let mut scheduled: HashSet<String> = HashSet::new();
            let mut stop_scheduling = false; // set on cancel: drain in-flight, schedule nothing new

            // Push every not-done/not-scheduled node whose inputs are all done.
            // Returns the node ids newly scheduled (so the caller can emit NodeStarted).
            // NOTE: `ctx` is captured by clone into each node future (forwarded opaquely,
            // like `run_id`/`cancel`). The topo/scheduling logic above does NOT read it —
            // executor purity is preserved.
            macro_rules! schedule_ready {
                () => {{
                    let mut started: Vec<NodeId> = Vec::new();
                    if !stop_scheduling {
                        for n in graph.nodes.iter() {
                            let id = n.id.as_str();
                            if done.contains(id) || scheduled.contains(id) {
                                continue;
                            }
                            if n.inputs.iter().all(|i| done.contains(i.as_str())) {
                                scheduled.insert(id.to_string());
                                started.push(n.id.clone());
                                let mut owned: Vec<(String, String)> = vec![("input".into(), input.clone())];
                                for inp in &n.inputs {
                                    if let Some((t, _, _)) = outputs.get(inp.as_str()) {
                                        owned.push((inp.as_str().into(), t.clone()));
                                    }
                                }
                                // Single-upstream alias: a node with exactly one input can render its
                                // predecessor's output as `{{draft}}` without hard-coding the predecessor's
                                // node id — so one refine prompt serves model-diverse legs whose draft nodes
                                // have distinct ids (e.g. reviewer_codex_draft / reviewer_claude_draft).
                                if let [only] = n.inputs.as_slice() {
                                    if let Some((t, _, _)) = outputs.get(only.as_str()) {
                                        owned.push(("draft".into(), t.clone()));
                                    }
                                }
                                if !n.inputs.is_empty() {
                                    let cost_rows: Vec<(String, Option<UsageSnapshot>)> = n.inputs.iter()
                                        .map(|inp| {
                                            (
                                                inp.as_str().to_string(),
                                                outputs
                                                    .get(inp.as_str())
                                                    .and_then(|(_, _, usage)| usage.clone()),
                                            )
                                        })
                                        .collect();
                                    owned.push(("workflow.costs".into(), render_costs_table(&cost_rows)));
                                    owned.push(("workflow.weights".into(), render_weights(&graph.panel)));
                                }
                                let node = n.clone();
                                let run_id = run_id.clone();
                                let cancel = cancel.clone();
                                let wf_id = graph.id.as_str().to_string();
                                let ctx = ctx.clone();
                                let dispatcher = dispatcher.clone();
                                let this = &this;
                                inflight.push(Box::pin(async move {
                                    let vars: HashMap<&str, &str> =
                                        owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                                    let (text, ok, usage) = this.run_node(&wf_id, &node, &vars, &run_id, &cancel, &ctx, dispatcher.as_ref()).await;
                                    (node.id.clone(), text, ok, usage)
                                }) as NodeFut);
                            }
                        }
                    }
                    started
                }};
            }

            for node in schedule_ready!() {
                yield Ok(WorkflowEvent::NodeStarted { node });
            }
            while let Some((node_id, text, ok, usage)) = inflight.next().await {
                yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone(), usage: usage.clone() });
                done.insert(node_id.as_str().to_string());
                outputs.insert(node_id.as_str().to_string(), (text, ok, usage));
                if cancel.is_cancelled() {
                    // Stop scheduling NEW nodes, but keep draining so every already-in-flight
                    // sibling completes its run_node cancel branch (backend.cancel() +
                    // forget_session()). Do NOT `break` — that drops in-flight futures
                    // mid-cleanup → stranded ACP sessions (dual-review blocker).
                    stop_scheduling = true;
                    continue;
                }
                for node in schedule_ready!() {
                    yield Ok(WorkflowEvent::NodeStarted { node });
                }
            }
            let (term_text, term_ok, _usage) = outputs.get(&terminal_id).cloned().unwrap_or_default();
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
    use crate::graph::{WorkflowGraph, WorkflowNode};
    use bridge_core::domain::{Part, RegistrySnapshot, SessionSpec};
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, NodeId, SessionId, WorkflowId};
    use bridge_core::ports::{AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, Update};
    use futures::StreamExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    pub(super) struct Rec {
        pub configured: Mutex<bool>,
        pub prompts: Mutex<Vec<String>>,
        pub prompt_parts: Mutex<Vec<Vec<Part>>>,
        pub prompt_sessions: Mutex<Vec<SessionId>>,
        pub cancels: Mutex<u32>,
        pub forgets: Mutex<u32>,
    }
    pub(super) struct FakeBackend {
        pub reply: String,
        pub rec: Arc<Rec>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec
                .prompts
                .lock()
                .unwrap()
                .push(parts.iter().map(|p| p.text.clone()).collect());
            self.rec.prompt_parts.lock().unwrap().push(parts);
            self.rec.prompt_sessions.lock().unwrap().push(_s.clone());
            let updates = vec![
                Ok(Update::Text(self.reply.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            *self.rec.cancels.lock().unwrap() += 1;
            Ok(())
        }
        async fn forget_session(&self, _s: &SessionId) {
            *self.rec.forgets.lock().unwrap() += 1;
        }
        async fn configure_session(
            &self,
            _s: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            *self.rec.configured.lock().unwrap() = true;
            Ok(())
        }
    }
    pub(super) struct NoopLease;
    impl Lease for NoopLease {}
    pub(super) fn minimal_entry(id: &AgentId) -> bridge_core::domain::AgentEntry {
        bridge_core::domain::AgentEntry {
            id: id.clone(),
            cmd: Some("x".into()),
            base_url: None,
            api_key_env: None,
            args: vec![],
            kind: bridge_core::domain::AgentKind::Acp,
            model_provider: None,
            model: None,
            effort: None,
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
        }
    }
    pub(super) struct FakeRegistry {
        pub backends: std::collections::HashMap<String, (String, Arc<Rec>)>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for FakeRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let (reply, rec) =
                self.backends
                    .get(id.as_str())
                    .cloned()
                    .ok_or(BridgeError::UnknownAgent {
                        id: id.as_str().into(),
                    })?;
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(FakeBackend { reply, rec }),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }
    pub(super) fn one_node_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "echo {{input}}".into(),
                inputs: vec![],
            }],
            panel: None,
        })
    }

    #[tokio::test]
    async fn captures_node_usage_smoke() {
        struct UsageBackend;
        #[async_trait::async_trait]
        impl AgentBackend for UsageBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text("HI".into())),
                    Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                        used: Some(15071),
                        size: Some(258400),
                        cost: None,
                        at_ms: 1,
                    })),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])))
            }

            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct UReg;
        #[async_trait::async_trait]
        impl AgentRegistry for UReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(UsageBackend),
                    lease: Box::new(NoopLease),
                })
            }

            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }

            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }

            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }

        let ex = WorkflowExecutor::new(Arc::new(UReg));
        let evs: Vec<_> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished { usage: Some(u), .. } => {
                assert_eq!(u.used, Some(15071))
            }
            other => panic!("expected captured usage, got {other:?}"),
        }
    }

    // A backend that reports Usage and THEN errors mid-stream. Shared by the warm + cold
    // "usage kept on error" regressions (whole-branch review MAJOR-1): the real tokens were
    // consumed, so the usage must survive into NodeFinished even though ok=false.
    struct UsageThenErrBackend {
        used: u64,
    }
    #[async_trait::async_trait]
    impl AgentBackend for UsageThenErrBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                    used: Some(self.used),
                    size: Some(100_000),
                    cost: None,
                    at_ms: 1,
                })),
                Err(BridgeError::ConfigInvalid {
                    reason: "boom".into(),
                }),
            ])))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn cold_usage_kept_when_node_errors_after_usage() {
        struct UReg;
        #[async_trait::async_trait]
        impl AgentRegistry for UReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(UsageThenErrBackend { used: 4242 }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let ex = WorkflowExecutor::new(Arc::new(UReg));
        let evs: Vec<_> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished {
                ok, usage: Some(u), ..
            } => {
                assert!(!ok, "node errored → ok=false");
                assert_eq!(u.used, Some(4242), "usage kept despite the stream error");
            }
            other => panic!("expected NodeFinished with kept usage, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn warm_usage_kept_when_node_errors_after_usage() {
        struct D;
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for D {
            async fn checkout(
                &self,
                _wf: &str,
                _n: &WorkflowNode,
                _r: &str,
                _c: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(UsageThenErrBackend { used: 777 }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: Arc::new(Mutex::new(Vec::new())),
                    }),
                })
            }
        }
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));
        let evs: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                Arc::new(D),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = evs
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished {
                ok, usage: Some(u), ..
            } => {
                assert!(!ok, "node errored → ok=false");
                assert_eq!(u.used, Some(777), "warm path keeps usage despite the error");
            }
            other => panic!("expected NodeFinished with kept usage, got {other:?}"),
        }
    }

    #[test]
    fn costs_table_renders_per_field_with_n_a() {
        use bridge_core::orch::{UsageCost, UsageSnapshot};

        let rows = vec![
            (
                "codexer".to_string(),
                Some(UsageSnapshot {
                    used: Some(15071),
                    size: Some(258400),
                    cost: None,
                    at_ms: 0,
                }),
            ),
            (
                "clauder".to_string(),
                Some(UsageSnapshot {
                    used: Some(8200),
                    size: Some(200000),
                    cost: Some(UsageCost {
                        amount: 0.03,
                        currency: "USD".into(),
                    }),
                    at_ms: 0,
                }),
            ),
            ("dead".to_string(), None),
        ];

        let table = render_costs_table(&rows);
        assert!(table.contains("| source | used | size | windowFraction | cost |"));
        assert!(table.contains("| codexer | 15071 | 258400 | 0.0583 |"));
        assert!(table.contains("| clauder | 8200 | 200000 | 0.0410 | 0.03 USD |"));
        assert!(table.contains("| dead | n/a | n/a | n/a | n/a |"));
    }

    #[test]
    fn weights_render_sorted() {
        let mut weights = std::collections::BTreeMap::new();
        weights.insert("risk".to_string(), 0.3);
        weights.insert("benefit".to_string(), 0.4);

        let out = render_weights(&Some(crate::graph::PanelConfig { weights }));

        assert_eq!(out, "- benefit: 0.4\n- risk: 0.3\n");
        assert_eq!(render_weights(&None), "(no weights configured)");
    }

    pub(super) struct CountingCleanup {
        pub calls: Arc<AtomicUsize>,
        pub exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NodeTurnCleanup for CountingCleanup {
        async fn on_exit(self: Box<Self>, exit: NodeTurnExit) {
            let label = match exit {
                NodeTurnExit::Normal => "normal".to_string(),
                NodeTurnExit::Canceled => "canceled".to_string(),
                NodeTurnExit::Error(e) => format!("error:{e:?}"),
            };
            self.exits.lock().unwrap().push(label);
            self.calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) struct FakeDispatcher {
        pub calls: Arc<AtomicUsize>,
        pub exits: Arc<Mutex<Vec<String>>>,
        pub rec: Arc<Rec>,
        pub session: SessionId,
        pub seed: Option<String>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for FakeDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(NodeTurn {
                backend: Arc::new(FakeBackend {
                    reply: "WARM".into(),
                    rec: self.rec.clone(),
                }),
                session: self.session.clone(),
                seed: self.seed.clone(),
                cleanup: Box::new(CountingCleanup {
                    calls: self.calls.clone(),
                    exits: self.exits.clone(),
                }),
            })
        }
    }

    #[tokio::test]
    async fn node_turn_cleanup_trait_object_runs_on_exit() {
        let calls = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = FakeDispatcher {
            calls: calls.clone(),
            exits: exits.clone(),
            rec: Arc::new(Rec::default()),
            session: SessionId::parse("workflow-w-only-run1").unwrap(),
            seed: None,
        };
        let graph = one_node_graph();
        let turn = dispatcher
            .checkout("w", &graph.nodes[0], "run1", &WorkflowRunContext::default())
            .await
            .unwrap();

        turn.cleanup.on_exit(NodeTurnExit::Normal).await;

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn warm_dispatch_no_forget() {
        let rec = Arc::new(Rec::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let session = SessionId::parse("warm-session").unwrap();
        let dispatcher = Arc::new(FakeDispatcher {
            calls: calls.clone(),
            exits: exits.clone(),
            rec: rec.clone(),
            session: session.clone(),
            seed: None,
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            } if output == "WARM"
        ));
        assert_eq!(*rec.forgets.lock().unwrap(), 0, "warm path must not forget");
        assert_eq!(rec.prompt_sessions.lock().unwrap().as_slice(), [session]);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn warm_seed_prepended() {
        let rec = Arc::new(Rec::default());
        let dispatcher = Arc::new(FakeDispatcher {
            calls: Arc::new(AtomicUsize::new(0)),
            exits: Arc::new(Mutex::new(Vec::new())),
            rec: rec.clone(),
            session: SessionId::parse("warm-session").unwrap(),
            seed: Some("S".into()),
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let _events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                dispatcher,
            )
            .collect::<Vec<_>>()
            .await;

        let parts = rec.prompt_parts.lock().unwrap();
        assert_eq!(
            parts[0][0].text,
            "[Summary of earlier context in this session]\nS"
        );
        assert_eq!(parts[0][1].text, "echo DIFF");
    }

    #[tokio::test]
    async fn dispatcher_cancel_drains() {
        use tokio::sync::Notify;

        struct Shared {
            entered: AtomicUsize,
            exits: Mutex<Vec<String>>,
            both_in_flight: Notify,
        }
        struct PendingWarmBackend {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for PendingWarmBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                if self.shared.entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    self.shared.both_in_flight.notify_one();
                }
                Ok(Box::pin(futures::stream::pending()))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                panic!("warm in-drain cancel is owned by cleanup")
            }
        }
        struct ExitCleanup {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl NodeTurnCleanup for ExitCleanup {
            async fn on_exit(self: Box<Self>, exit: NodeTurnExit) {
                let label = match exit {
                    NodeTurnExit::Normal => "normal",
                    NodeTurnExit::Canceled => "canceled",
                    NodeTurnExit::Error(_) => "error",
                };
                self.shared.exits.lock().unwrap().push(label.to_string());
            }
        }
        struct WarmPendingDispatcher {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for WarmPendingDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(PendingWarmBackend {
                        shared: self.shared.clone(),
                    }),
                    session: SessionId::parse(format!("warm-{}", node.id.as_str())).unwrap(),
                    seed: None,
                    cleanup: Box::new(ExitCleanup {
                        shared: self.shared.clone(),
                    }),
                })
            }
        }

        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });
        let shared = Arc::new(Shared {
            entered: AtomicUsize::new(0),
            exits: Mutex::new(Vec::new()),
            both_in_flight: Notify::new(),
        });
        let token = CancellationToken::new();
        let t2 = token.clone();
        let s2 = shared.clone();
        tokio::spawn(async move {
            if s2.entered.load(Ordering::SeqCst) < 2 {
                s2.both_in_flight.notified().await;
            }
            t2.cancel();
        });
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run_with_context_and_dispatcher(
                graph,
                "x".into(),
                "r".into(),
                token,
                WorkflowRunContext::default(),
                Arc::new(WarmPendingDispatcher {
                    shared: shared.clone(),
                }),
            )
            .collect::<Vec<_>>(),
        )
        .await
        .expect("warm cancel must drain in-flight nodes")
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert_eq!(
            shared.exits.lock().unwrap().as_slice(),
            ["canceled", "canceled"]
        );
    }

    #[tokio::test]
    async fn warm_done_cancelled_finishes_not_cancels() {
        struct DoneCancelledBackend {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for DoneCancelledBackend {
            async fn prompt(
                &self,
                s: &SessionId,
                parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.rec.prompt_sessions.lock().unwrap().push(s.clone());
                self.rec.prompt_parts.lock().unwrap().push(parts);
                Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                    stop_reason: STOP_REASON_CANCELLED.into(),
                })])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                *self.rec.cancels.lock().unwrap() += 1;
                Ok(())
            }
        }
        struct DoneCancelledDispatcher {
            rec: Arc<Rec>,
            exits: Arc<Mutex<Vec<String>>>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for DoneCancelledDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(DoneCancelledBackend {
                        rec: self.rec.clone(),
                    }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: self.calls.clone(),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let rec = Arc::new(Rec::default());
        let exits = Arc::new(Mutex::new(Vec::new()));
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));
        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                WorkflowRunContext::default(),
                Arc::new(DoneCancelledDispatcher {
                    rec: rec.clone(),
                    exits: exits.clone(),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events
                .iter()
                .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
                .unwrap(),
            WorkflowEvent::NodeFinished { ok: false, .. }
        ));
        assert_eq!(*rec.cancels.lock().unwrap(), 0);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn warm_cancel_after_checkout_finishes_no_prompt_no_cancel() {
        struct CancelAfterCheckoutDispatcher {
            token: CancellationToken,
            rec: Arc<Rec>,
            exits: Arc<Mutex<Vec<String>>>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for CancelAfterCheckoutDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                self.token.cancel();
                Ok(NodeTurn {
                    backend: Arc::new(FakeBackend {
                        reply: "UNUSED".into(),
                        rec: self.rec.clone(),
                    }),
                    session: SessionId::parse("warm-session").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: self.calls.clone(),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let rec = Arc::new(Rec::default());
        let exits = Arc::new(Mutex::new(Vec::new()));
        let token = CancellationToken::new();
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));

        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                token.clone(),
                WorkflowRunContext::default(),
                Arc::new(CancelAfterCheckoutDispatcher {
                    token,
                    rec: rec.clone(),
                    exits: exits.clone(),
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert!(rec.prompt_parts.lock().unwrap().is_empty(), "no prompt");
        assert_eq!(*rec.cancels.lock().unwrap(), 0);
        assert_eq!(exits.lock().unwrap().as_slice(), ["normal"]);
    }

    #[tokio::test]
    async fn single_node_configures_renders_concatenates() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);
        let mut events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let term = events.pop().unwrap();
        assert!(
            matches!(term, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "HELLO")
        );
        assert!(*rec.configured.lock().unwrap(), "configure_session called");
        assert_eq!(
            rec.prompts.lock().unwrap()[0],
            "echo DIFF",
            "template rendered with {{input}}"
        );
    }

    #[tokio::test]
    async fn cold_configure_error_fails_node_without_prompting() {
        struct CfgErrBackend {
            rec: Arc<Rec>,
        }

        #[async_trait::async_trait]
        impl AgentBackend for CfgErrBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.rec
                    .prompts
                    .lock()
                    .unwrap()
                    .push(parts.iter().map(|p| p.text.clone()).collect());
                Ok(Box::pin(tokio_stream::iter(Vec::<
                    Result<Update, BridgeError>,
                >::new())))
            }

            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }

            async fn configure_session(
                &self,
                _s: &SessionId,
                _spec: &SessionSpec,
            ) -> Result<(), BridgeError> {
                Err(BridgeError::ConfigInvalid {
                    reason: "worktree add failed".into(),
                })
            }

            async fn forget_session(&self, _s: &SessionId) {
                *self.rec.forgets.lock().unwrap() += 1;
            }
        }

        struct CfgErrReg {
            rec: Arc<Rec>,
        }

        #[async_trait::async_trait]
        impl AgentRegistry for CfgErrReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(CfgErrBackend {
                        rec: self.rec.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }

            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }

            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }

            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }

        let rec = Arc::new(Rec::default());
        let ex = WorkflowExecutor::new(Arc::new(CfgErrReg { rec: rec.clone() }));
        let events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        let nf = events
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap();
        match nf {
            WorkflowEvent::NodeFinished { ok, output, .. } => {
                assert!(!ok, "configure error must fail the node");
                assert!(
                    output.starts_with("[node only failed: configure "),
                    "unexpected node output: {output}"
                );
            }
            other => panic!("expected NodeFinished, got {other:?}"),
        }
        assert!(
            rec.prompts.lock().unwrap().is_empty(),
            "prompt must not run after configure_session fails"
        );
        assert_eq!(
            *rec.forgets.lock().unwrap(),
            1,
            "configure_session error must forget the session"
        );
    }

    #[tokio::test]
    async fn cold_path_unchanged() {
        // The `None` (cold) branch must be byte-identical to pre-Slice-5 behavior: the cold session id
        // `workflow-{wf}-{node}-{run_id}` AND `forget_session` at the end (NOT the warm dispatcher path).
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("HELLO".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);
        let _events: Vec<WorkflowEvent> = ex
            .run(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rec.prompt_sessions.lock().unwrap().as_slice(),
            [SessionId::parse("workflow-w-only-run1").unwrap()],
            "cold path uses the cold workflow-wf-node-runid session id"
        );
        assert_eq!(
            *rec.forgets.lock().unwrap(),
            1,
            "cold path forgets the session (no warm keep-alive)"
        );
    }

    fn review_graph() -> Arc<WorkflowGraph> {
        let n = |id: &str, ag: &str, ins: &[&str], tpl: &str| WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(ag).unwrap(),
            prompt_template: tpl.into(),
            inputs: ins.iter().map(|i| NodeId::parse(*i).unwrap()).collect(),
        };
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("code-review").unwrap(),
            nodes: vec![
                n("codex", "codex", &[], "review {{input}}"),
                n("claude", "claude", &[], "review {{input}}"),
                n(
                    "synth",
                    "synth",
                    &["codex", "claude"],
                    "merge {{codex}} + {{claude}} for {{input}}\n{{workflow.costs}}",
                ),
            ],
            panel: None,
        })
    }

    #[tokio::test]
    async fn single_input_node_renders_draft_alias() {
        // A refine node with exactly one input can reference its predecessor's output as {{draft}}
        // (so one shared refine prompt serves legs whose draft nodes have distinct ids).
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("refine").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("draftnode").unwrap(),
                    agent: AgentId::parse("codex").unwrap(),
                    prompt_template: "draft {{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("refinenode").unwrap(),
                    agent: AgentId::parse("claude").unwrap(),
                    prompt_template: "refine against {{draft}} for {{input}}".into(),
                    inputs: vec![NodeId::parse("draftnode").unwrap()],
                },
            ],
            panel: None,
        });
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("DRAFT_OUT")),
                ("claude".to_string(), mk("REFINED")),
            ]
            .into(),
        });
        let refine_rec = reg.backends.get("claude").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let _ = ex
            .run(graph, "DIFF".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;
        let p = &refine_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("DRAFT_OUT") && p.contains("DIFF"),
            "refine node must see the draft via {{draft}} AND the original via {{input}}: {p}"
        );
    }

    #[tokio::test]
    async fn fan_in_synth_receives_both_reviews_and_input() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("CODEX_REVIEW")),
                ("claude".to_string(), mk("CLAUDE_REVIEW")),
                ("synth".to_string(), mk("FINAL")),
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await;
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "FINAL")
        );
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("CODEX_REVIEW") && p.contains("CLAUDE_REVIEW") && p.contains("DIFF"),
            "synth got both reviews + {{input}}: {p}"
        );
    }

    #[tokio::test]
    async fn fan_out_runs_concurrently() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Barrier;
        // Both fan-out legs must ENTER prompt() before either replies → only possible if run in parallel.
        struct BarrierBackend {
            reply: String,
            barrier: Arc<Barrier>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for BarrierBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.barrier.wait().await; // deadlocks unless the other leg also reaches here
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(self.reply.clone())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        // BReg hands out BarrierBackend only for the first 2 resolves (the fan-out nodes);
        // node `t` (the terminal, resolved 3rd) gets a plain non-blocking backend so it
        // doesn't deadlock on a single-party wait.
        struct BReg {
            barrier: Arc<Barrier>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for BReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                let n = self.calls.fetch_add(1, Ordering::SeqCst);
                let backend: Arc<dyn bridge_core::ports::AgentBackend> = if n < 2 {
                    Arc::new(BarrierBackend {
                        reply: id.as_str().to_uppercase(),
                        barrier: self.barrier.clone(),
                    })
                } else {
                    Arc::new(FakeBackend {
                        reply: id.as_str().to_uppercase(),
                        rec: Arc::new(Rec::default()),
                    })
                };
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend,
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // two-node graph: a, b both inputs=[] (fan-out), plus a terminal t depending on both.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });
        let reg = Arc::new(BReg {
            barrier: Arc::new(Barrier::new(2)),
            calls: Arc::new(AtomicUsize::new(0)),
        }); // a + b must rendezvous
        let ex = WorkflowExecutor::new(reg);
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), CancellationToken::new())
                .collect::<Vec<_>>(),
        )
        .await;
        assert!(
            res.is_ok(),
            "fan-out legs ran concurrently (no deadlock/timeout)"
        );
    }

    #[tokio::test]
    async fn pipeline_threads_output_to_input() {
        // a -> b -> c ; b sees a's output, c sees b's.
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("a".to_string(), mk("AOUT")),
                ("b".to_string(), mk("BOUT")),
                ("c".to_string(), mk("COUT")),
            ]
            .into(),
        });
        let b_rec = reg.backends.get("b").unwrap().1.clone();
        let c_rec = reg.backends.get("c").unwrap().1.clone();
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("p").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });
        let ex = WorkflowExecutor::new(reg);
        let _ = ex
            .run(g, "x".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;
        assert_eq!(b_rec.prompts.lock().unwrap()[0], "got AOUT");
        assert_eq!(c_rec.prompts.lock().unwrap()[0], "got BOUT");
    }

    #[tokio::test]
    async fn failed_fan_out_leg_marker_reaches_synth_and_run_completes() {
        // No "codex" backend registered → the codex node's resolve fails → error marker;
        // claude + synth still run (graceful degradation).
        let reg = Arc::new(FakeRegistry {
            backends: [
                (
                    "claude".to_string(),
                    ("CLAUDE_REVIEW".to_string(), Arc::new(Rec::default())),
                ),
                (
                    "synth".to_string(),
                    ("FINAL".to_string(), Arc::new(Rec::default())),
                ),
                // NOTE: no "codex" → resolve fails for the codex node
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await;
        // run COMPLETES (terminal synth ok) — graceful degradation
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
        // a NodeFinished{ok:false} was emitted for codex
        assert!(evs.iter().any(|e| matches!(e.as_ref().unwrap(),
            WorkflowEvent::NodeFinished { node, ok: false, .. } if node.as_str() == "codex")));
        // the EXACT failure marker reached synth's prompt
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("[node codex failed:"),
            "marker reached synth: {p}"
        );
    }

    #[tokio::test]
    async fn panel_degrades_failed_member_usage_is_n_a() {
        // No "member_a" backend registered → its node fails (error marker, usage None);
        // member_b + synth still run, synth's costs table shows member_a as n/a.
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("member_b".to_string(), mk("B_ANALYSIS")),
                ("synth".to_string(), mk("PANEL")),
            ]
            .into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("panel").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("member_a").unwrap(),
                    agent: AgentId::parse("member_a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("member_b").unwrap(),
                    agent: AgentId::parse("member_b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{member_b}}\n{{workflow.costs}}".into(),
                    inputs: vec![
                        NodeId::parse("member_a").unwrap(),
                        NodeId::parse("member_b").unwrap(),
                    ],
                },
            ],
            panel: None,
        });
        let evs: Vec<_> = WorkflowExecutor::new(reg)
            .run(g, "DIFF".into(), "r".into(), CancellationToken::new())
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                ..
            }
        ));
        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("| member_a | n/a | n/a | n/a | n/a |"),
            "failed member usage row must be n/a: {p}"
        );
    }

    #[tokio::test]
    async fn cancel_calls_backend_cancel_and_ends_canceled() {
        // A backend whose prompt() stream NEVER yields Done (pending) → only the cancel path ends it.
        struct Pending {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for Pending {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Ok(Box::pin(futures::stream::pending())) // never yields
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                *self.rec.cancels.lock().unwrap() += 1;
                Ok(())
            }
        }
        let rec = Arc::new(Rec::default());
        struct PReg {
            rec: Arc<Rec>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for PReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(Pending {
                        rec: self.rec.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let token = CancellationToken::new();
        let reg = Arc::new(PReg { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            t2.cancel();
        });
        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ex.run(one_node_graph(), "x".into(), "r".into(), token)
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        assert_eq!(
            *rec.cancels.lock().unwrap(),
            1,
            "backend.cancel was called for the in-flight node"
        );
    }

    #[tokio::test]
    async fn cancel_drains_inflight() {
        // TWO fan-out legs, both genuinely in-flight (their prompt stream is pending),
        // when the token fires. Each leg's run_node cancel branch must run
        // backend.cancel() AND forget_session() — proving the FuturesUnordered drains
        // (not `break`s) after the first post-cancel completion. A `break` would drop
        // the second leg's future mid-cleanup → its counter never reaches 2.
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        // Shared observability: cleanups counts cancel()+forget_session() calls;
        // entered counts prompt() entries; both_in_flight wakes the driver once
        // both legs have parked on their pending stream.
        struct Shared {
            cleanups: AtomicUsize,
            entered: AtomicUsize,
            both_in_flight: Notify,
        }
        struct CancelObservingBackend {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for CancelObservingBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                // Mark this leg as in-flight; once both legs are here, wake the driver.
                if self.shared.entered.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    self.shared.both_in_flight.notify_one();
                }
                // Pending stream → the node parks in run_node's select! until cancel.
                Ok(Box::pin(futures::stream::pending()))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                self.shared.cleanups.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn forget_session(&self, _s: &SessionId) {
                self.shared.cleanups.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct CReg {
            shared: Arc<Shared>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for CReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(CancelObservingBackend {
                        shared: self.shared.clone(),
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // Two fan-out legs (a, b — no inputs) + terminal t depending on both. Cancel
        // fires while a and b are in-flight, so t is never scheduled.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });
        let shared = Arc::new(Shared {
            cleanups: AtomicUsize::new(0),
            entered: AtomicUsize::new(0),
            both_in_flight: Notify::new(),
        });
        let reg = Arc::new(CReg {
            shared: shared.clone(),
        });
        let ex = WorkflowExecutor::new(reg);
        let token = CancellationToken::new();

        // Wait for both legs to be in-flight, then cancel.
        let t2 = token.clone();
        let s2 = shared.clone();
        tokio::spawn(async move {
            // notify_one before any waiter is dropped; re-check the counter to avoid races.
            if s2.entered.load(Ordering::SeqCst) < 2 {
                s2.both_in_flight.notified().await;
            }
            t2.cancel();
        });

        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), token).collect::<Vec<_>>(),
        )
        .await
        .expect("drain must complete after cancel (a `break` would also finish, but leak cleanup)");

        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
        // BOTH legs must have run cancel()+forget_session() = 4 total cleanup calls.
        // A `break` after the first post-cancel completion drops the second leg's
        // future, aborting its cleanup → count would be 2, not 4.
        assert_eq!(
            shared.cleanups.load(Ordering::SeqCst),
            4,
            "both in-flight legs must run cancel()+forget_session() (drain, not break)"
        );
    }

    #[tokio::test]
    async fn completion_order() {
        // Two parallel nodes: a (fast) + b (slow). Completion-driven scheduling must
        // yield a's NodeFinished BEFORE b's — an ordering join_all did NOT guarantee
        // (join_all yields in ready-batch iteration order regardless of finish time).
        use std::sync::atomic::{AtomicBool, Ordering as AO};
        use tokio::sync::Notify;
        struct TimedBackend {
            reply: String,
            // None → reply immediately; Some(gate) → wait on gate before replying.
            gate: Option<Arc<Notify>>,
            // When `a` starts its prompt, signal the releaser task (None for non-a nodes).
            a_done: Option<(Arc<Notify>, Arc<AtomicBool>)>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for TimedBackend {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                if let Some(g) = &self.gate {
                    g.notified().await; // park until released
                }
                // After returning from prompt(), the stream is a synchronous iter, so
                // run_node for this node will finish as soon as the executor polls it.
                // Signal the releaser that `a` has completed its prompt (and is therefore
                // done, since the iter stream yields synchronously).
                if let Some((notify, flag)) = &self.a_done {
                    flag.store(true, AO::SeqCst);
                    notify.notify_one();
                }
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text(self.reply.clone())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let slow_gate = Arc::new(Notify::new());
        let a_done_notify = Arc::new(Notify::new());
        let a_done_flag = Arc::new(AtomicBool::new(false));
        struct TReg {
            slow_gate: Arc<Notify>,
            a_done_notify: Arc<Notify>,
            a_done_flag: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for TReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                // "b" is the slow leg (gated); "a" gets the a_done signal; "t" is plain.
                let gate = if id.as_str() == "b" {
                    Some(self.slow_gate.clone())
                } else {
                    None
                };
                let a_done = if id.as_str() == "a" {
                    Some((self.a_done_notify.clone(), self.a_done_flag.clone()))
                } else {
                    None
                };
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(TimedBackend {
                        reply: id.as_str().to_uppercase(),
                        gate,
                        a_done,
                    }),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        // a, b parallel (no inputs); terminal t depends on both so the run completes.
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("g").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });
        let reg = Arc::new(TReg {
            slow_gate: slow_gate.clone(),
            a_done_notify: a_done_notify.clone(),
            a_done_flag: a_done_flag.clone(),
        });
        let ex = WorkflowExecutor::new(reg);

        // Release the slow leg only AFTER `a` has signalled completion — causal ordering,
        // no wall-clock dependency. Guard against the notify firing before the waiter
        // starts (mirror the cancel_drains_inflight pattern).
        let g2 = slow_gate.clone();
        tokio::spawn(async move {
            if !a_done_flag.load(AO::SeqCst) {
                a_done_notify.notified().await;
            }
            g2.notify_waiters();
        });

        let evs: Vec<_> = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            ex.run(g, "x".into(), "r".into(), CancellationToken::new())
                .collect::<Vec<_>>(),
        )
        .await
        .expect("run must complete")
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

        // Collect the order of NodeFinished ids for the two parallel legs.
        let finished_order: Vec<&str> = evs
            .iter()
            .filter_map(|e| match e {
                WorkflowEvent::NodeFinished { node, .. }
                    if node.as_str() == "a" || node.as_str() == "b" =>
                {
                    Some(node.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            finished_order,
            vec!["a", "b"],
            "fast leg 'a' must finish before slow leg 'b' (completion-driven order)"
        );
    }

    #[tokio::test]
    async fn cancel_during_slow_prompt_ends_canceled_promptly() {
        struct SlowPrompt;
        #[async_trait::async_trait]
        impl AgentBackend for SlowPrompt {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await; // long setup
                Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                })])))
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        struct SReg;
        #[async_trait::async_trait]
        impl AgentRegistry for SReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(SlowPrompt),
                    lease: Box::new(NoopLease),
                })
            }
            fn default_id(&self) -> AgentId {
                AgentId::parse("a").unwrap()
            }
            async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }
            fn list(&self) -> Vec<AgentId> {
                vec![]
            }
        }
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            t2.cancel();
        });
        let ex = WorkflowExecutor::new(Arc::new(SReg));
        // Must finish well under the 10s prompt sleep → the cancel preempted setup.
        let evs = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ex.run(one_node_graph(), "x".into(), "r".into(), token)
                .collect::<Vec<_>>(),
        )
        .await
        .expect("cancel preempts the slow prompt setup");
        assert!(matches!(
            evs.last().unwrap().as_ref().unwrap(),
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                ..
            }
        ));
    }

    // ── run_from tests ──────────────────────────────────────────────────────

    /// A 3-node fan-in (codex + claude → synth). Seed {codex, claude} as done;
    /// assert only `synth` is actually prompted, run completes, and `synth`'s
    /// prompt contains the seeded outputs.
    #[tokio::test]
    async fn run_from_skips_seeded_runs_rest() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("codex".to_string(), mk("CODEX_SEEDED_IGNORED")),
                ("claude".to_string(), mk("CLAUDE_SEEDED_IGNORED")),
                ("synth".to_string(), mk("SYNTH_FINAL")),
            ]
            .into(),
        });
        let codex_rec = reg.backends.get("codex").unwrap().1.clone();
        let claude_rec = reg.backends.get("claude").unwrap().1.clone();
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();

        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = [
            ("codex".to_string(), ("OUTA".to_string(), true, None)),
            ("claude".to_string(), ("OUTB".to_string(), true, None)),
        ]
        .into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(
                review_graph(),
                "DIFF".into(),
                "resume1".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        // Run must complete successfully.
        let last = evs.last().unwrap().as_ref().unwrap();
        assert!(
            matches!(last, WorkflowEvent::Terminal { outcome: WorkflowOutcome::Completed, output } if output == "SYNTH_FINAL"),
            "terminal should be Completed/SYNTH_FINAL, got: {last:?}"
        );

        // codex and claude must NOT have been prompted (they were seeded).
        assert!(
            codex_rec.prompts.lock().unwrap().is_empty(),
            "codex was seeded; its backend must not be prompted"
        );
        assert!(
            claude_rec.prompts.lock().unwrap().is_empty(),
            "claude was seeded; its backend must not be prompted"
        );

        // synth MUST have been prompted exactly once, and its prompt must contain
        // the seeded outputs OUTA and OUTB (passed as template vars).
        let synth_prompts = synth_rec.prompts.lock().unwrap();
        assert_eq!(
            synth_prompts.len(),
            1,
            "synth should be prompted exactly once"
        );
        let p = &synth_prompts[0];
        assert!(
            p.contains("OUTA") && p.contains("OUTB"),
            "synth prompt must contain seeded outputs OUTA and OUTB: {p}"
        );

        // Exactly ONE NodeStarted (synth) and ONE NodeFinished (synth) emitted.
        let started: Vec<_> = evs
            .iter()
            .filter_map(|e| match e.as_ref().unwrap() {
                WorkflowEvent::NodeStarted { node } => Some(node.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(started, vec!["synth"], "only synth should be started");

        // Exactly ONE NodeFinished (synth) emitted — symmetry with NodeStarted.
        let finished: Vec<_> = evs
            .iter()
            .filter_map(|e| match e.as_ref().unwrap() {
                WorkflowEvent::NodeFinished { node, .. } => Some(node.as_str().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(finished, vec!["synth"], "only synth should be finished");
    }

    #[tokio::test]
    async fn resumed_synth_sees_seeded_member_usage() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [("synth".to_string(), mk("FINAL"))].into(),
        });
        let synth_rec = reg.backends.get("synth").unwrap().1.clone();
        let ex = WorkflowExecutor::new(reg);

        let mut seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> = HashMap::new();
        seed.insert(
            "codex".into(),
            (
                "CODEX_REVIEW".into(),
                true,
                Some(UsageSnapshot {
                    used: Some(15071),
                    size: Some(258400),
                    cost: None,
                    at_ms: 0,
                }),
            ),
        );
        seed.insert("claude".into(), ("CLAUDE_REVIEW".into(), true, None));

        let _ = ex
            .run_from(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        let p = &synth_rec.prompts.lock().unwrap()[0];
        assert!(
            p.contains("| codex | 15071 | 258400 |"),
            "resumed synth costs table shows seeded member usage: {p}"
        );
        assert!(
            p.contains("| claude | n/a |"),
            "member with no captured usage -> n/a: {p}"
        );
    }

    /// Seed contains a node id not present in the graph → stream yields ConfigInvalid.
    #[tokio::test]
    async fn run_from_unknown_seed_node_errors() {
        let reg = Arc::new(FakeRegistry {
            backends: [(
                "codex".to_string(),
                ("X".to_string(), Arc::new(Rec::default())),
            )]
            .into(),
        });
        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> =
            [("ghost_node".to_string(), ("OUT".to_string(), true, None))].into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(
                one_node_graph(),
                "inp".into(),
                "r".into(),
                CancellationToken::new(),
                seed,
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(evs.len(), 1, "should yield exactly one error event");
        let err = evs[0].as_ref().unwrap_err();
        assert!(
            matches!(err, BridgeError::ConfigInvalid { reason } if reason.contains("unknown node")),
            "expected ConfigInvalid about unknown node, got: {err:?}"
        );
    }

    // ── WorkflowRunContext / cwd threading tests ────────────────────────────

    /// Recording backend that captures the `SessionSpec.cwd` from each
    /// `configure_session` call. Used to verify `WorkflowRunContext` is
    /// forwarded to EVERY node.
    #[derive(Default)]
    struct CwdRec {
        cwds: Mutex<Vec<Option<SessionCwd>>>,
    }
    struct CwdCapBackend {
        reply: String,
        rec: Arc<CwdRec>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for CwdCapBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = vec![
                Ok(Update::Text(self.reply.clone())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn configure_session(
            &self,
            _s: &SessionId,
            spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.cwds.lock().unwrap().push(spec.cwd.clone());
            Ok(())
        }
    }
    struct CwdCapRegistry {
        rec: Arc<CwdRec>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for CwdCapRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(CwdCapBackend {
                    reply: "OK".into(),
                    rec: self.rec.clone(),
                }),
                lease: Box::new(NoopLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("a").unwrap()
        }
        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    /// `run_from_with_context` with `session_cwd = Some("/req")` → EVERY node's
    /// `configure_session` receives `spec.cwd == Some("/req")`.
    #[tokio::test]
    async fn run_from_with_context_cwd_set_reaches_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let ctx = WorkflowRunContext {
            session_cwd: Some(SessionCwd::parse("/req").unwrap()),
            make_rich_sink: None,
        };
        let _evs: Vec<_> = ex
            .run_from_with_context(
                review_graph(), // 3 nodes: codex, claude, synth
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                HashMap::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert_eq!(
                cwd.as_ref().map(|c| c.as_str()),
                Some("/req"),
                "every node must receive cwd=/req, got {:?}",
                cwd
            );
        }
    }

    /// `run_from_with_context` with `WorkflowRunContext::default()` (None cwd) →
    /// every node's `configure_session` receives `spec.cwd == None`.
    #[tokio::test]
    async fn run_from_with_context_cwd_none_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let _evs: Vec<_> = ex
            .run_from_with_context(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                HashMap::new(),
                WorkflowRunContext::default(),
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert!(
                cwd.is_none(),
                "every node must receive cwd=None, got {:?}",
                cwd
            );
        }
    }

    /// `run_with_context` (scratch, no seed) propagates cwd to every node.
    #[tokio::test]
    async fn run_with_context_cwd_set_reaches_every_node() {
        let rec = Arc::new(CwdRec::default());
        let reg = Arc::new(CwdCapRegistry { rec: rec.clone() });
        let ex = WorkflowExecutor::new(reg);
        let ctx = WorkflowRunContext {
            session_cwd: Some(SessionCwd::parse("/req2").unwrap()),
            make_rich_sink: None,
        };
        let _evs: Vec<_> = ex
            .run_with_context(
                review_graph(),
                "DIFF".into(),
                "r".into(),
                CancellationToken::new(),
                ctx,
            )
            .collect::<Vec<_>>()
            .await;
        let cwds = rec.cwds.lock().unwrap();
        assert_eq!(cwds.len(), 3, "all 3 nodes must call configure_session");
        for cwd in cwds.iter() {
            assert_eq!(
                cwd.as_ref().map(|c| c.as_str()),
                Some("/req2"),
                "every node must receive cwd=/req2, got {:?}",
                cwd
            );
        }
    }

    /// Seed contains a non-root node (b, which depends on a) but NOT its upstream (a).
    /// This violates the closure invariant → stream yields ConfigInvalid.
    ///
    /// Graph: a → b → c  (pipeline_threads_output_to_input shape)
    #[tokio::test]
    async fn run_from_seed_not_closed_errors() {
        let mk = |reply: &str| (reply.to_string(), Arc::new(Rec::default()));
        let reg = Arc::new(FakeRegistry {
            backends: [
                ("a".to_string(), mk("AOUT")),
                ("b".to_string(), mk("BOUT")),
                ("c".to_string(), mk("COUT")),
            ]
            .into(),
        });

        // Graph: a → b → c
        let g = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("p").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: NodeId::parse("a").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                },
            ],
            panel: None,
        });

        // Seed only `b` without its upstream `a` → closure violation.
        let seed: HashMap<String, (String, bool, Option<UsageSnapshot>)> =
            [("b".to_string(), ("BOUT".to_string(), true, None))].into();

        let ex = WorkflowExecutor::new(reg);
        let evs: Vec<_> = ex
            .run_from(g, "inp".into(), "r".into(), CancellationToken::new(), seed)
            .collect::<Vec<_>>()
            .await;

        assert_eq!(evs.len(), 1, "should yield exactly one error event");
        let err = evs[0].as_ref().unwrap_err();
        assert!(
            matches!(err, BridgeError::ConfigInvalid { reason } if reason.contains("closed under inputs")),
            "expected ConfigInvalid about closure, got: {err:?}"
        );
    }
}
