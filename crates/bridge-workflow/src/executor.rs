//! WorkflowExecutor — runs a validated DAG over the registry. Each node: configure_session
//! → prompt → concatenate Update::Text into the node output. Cancel via token.
use crate::graph::{WorkflowGraph, WorkflowNode};
use crate::template::render;
use bridge_core::domain::{effective_config, Part, SessionSpec};
use bridge_core::error::BridgeError;
use bridge_core::ids::{NodeId, SessionId};
use bridge_core::orch::UsageSnapshot;
use bridge_core::ports::{
    classify_failure, AgentBackend, AgentRegistry, BackendObservers, DiagnosticObserver,
    DiagnosticObserverFactory, FailureClass, ObsEvent, Observer, RichEventSinkFactory, TurnContext,
    TurnOutcome, Update, UsageFinalization, STOP_REASON_CANCELLED,
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
#[derive(Clone)]
pub struct WorkflowRunContext {
    pub session_cwd: Option<SessionCwd>,
    pub make_rich_sink: Option<Arc<dyn RichEventSinkFactory>>,
    pub observer: Arc<dyn Observer>,
    pub parent_traceparent: Option<bridge_core::ports::TraceParent>,
    pub task_id: Option<bridge_core::ids::TaskId>,
    pub prompt_id: Option<String>,
}

impl Default for WorkflowRunContext {
    fn default() -> Self {
        Self {
            session_cwd: None,
            make_rich_sink: None,
            observer: Arc::new(bridge_observ::NoopObserver),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        }
    }
}

/// Additive diagnostic-authority wrapper for workflow execution. Keeping the
/// factory out of [`WorkflowRunContext`] preserves source compatibility for
/// downstream exhaustive struct literals while making durable authority an
/// explicit choice at the executor entrypoint.
#[derive(Clone)]
pub struct WorkflowDiagnosticContext {
    request: WorkflowRunContext,
    factory: Arc<dyn DiagnosticObserverFactory>,
}

impl WorkflowDiagnosticContext {
    pub fn new(request: WorkflowRunContext, factory: Arc<dyn DiagnosticObserverFactory>) -> Self {
        Self { request, factory }
    }

    pub fn in_memory(request: WorkflowRunContext) -> Self {
        Self::new(
            request,
            Arc::new(
                bridge_core::diagnostics::InMemoryDiagnosticObserverFactory::new(64)
                    .expect("workflow diagnostic capacity is nonzero"),
            ),
        )
    }

    fn into_parts(self) -> (WorkflowRunContext, Arc<dyn DiagnosticObserverFactory>) {
        (self.request, self.factory)
    }
}

pub enum NodeTurnExit {
    Normal,
    Canceled,
    Error(BridgeError),
}

#[async_trait::async_trait]
pub trait NodeTurnCleanup: Send {
    /// Synchronously arm the terminal action before any later cancellation
    /// point (for example, flushing a rich-event sink). Compatibility
    /// implementations have no pre-settlement state and may ignore this.
    fn arm_exit(&mut self, _exit: &NodeTurnExit) {}

    /// Invoked once after prompt+drain on the node's exit branch. Each impl closes over what it owns
    /// (cold: backend+session for forget; warm: SessionManager+child+gen+op for finish/cancel/expire).
    async fn on_exit(self: Box<Self>, exit: NodeTurnExit);

    /// Result-bearing operation-owned cleanup. Compatibility implementations
    /// delegate to the legacy method; warm/observed owners override this so
    /// teardown settles before node-terminal observability.
    async fn on_exit_observed(
        self: Box<Self>,
        exit: NodeTurnExit,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<(), BridgeError> {
        self.on_exit(exit).await;
        Ok(())
    }
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

    async fn checkout_observed(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        run_id: &str,
        ctx: &WorkflowRunContext,
        _observer: Arc<dyn DiagnosticObserver>,
    ) -> Result<NodeTurn, BridgeError> {
        self.checkout(wf_id, node, run_id, ctx).await
    }
}

/// Uniform future type used in the per-run `FuturesUnordered` pool.
/// Each fan-out node is boxed to this type so `FuturesUnordered` can hold
/// futures of different async-block monomorphisations in one collection.
type NodeFut<'a> = std::pin::Pin<
    Box<
        dyn futures::Future<Output = (NodeId, String, bool, Option<UsageSnapshot>, NodeDisposition)>
            + Send
            + 'a,
    >,
>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeDisposition {
    Completed,
    Failed,
    Canceled,
}

impl NodeDisposition {
    fn from_turn(outcome: &TurnOutcome) -> Self {
        match outcome {
            TurnOutcome::Success => Self::Completed,
            TurnOutcome::Failed(_) => Self::Failed,
            TurnOutcome::Canceled => Self::Canceled,
        }
    }

    fn workflow_outcome(self) -> WorkflowOutcome {
        match self {
            Self::Completed => WorkflowOutcome::Completed,
            Self::Failed => WorkflowOutcome::Failed,
            Self::Canceled => WorkflowOutcome::Canceled,
        }
    }
}

#[derive(Clone, Copy)]
enum ColdCleanupAction {
    Forget,
    Release,
}

async fn cleanup_cold_session(
    backend: &Arc<dyn AgentBackend>,
    session: &SessionId,
    observer: &Arc<dyn DiagnosticObserver>,
    action: ColdCleanupAction,
) -> Result<(), BridgeError> {
    match action {
        ColdCleanupAction::Forget => {
            backend
                .forget_session_observed(session, observer.clone())
                .await
        }
        ColdCleanupAction::Release => {
            backend
                .release_session_observed(session, observer.clone())
                .await
        }
    }
}

enum RenderInput {
    Freeform(String),
    Spec(bridge_core::task_spec::TaskSpec),
    Invalid(String),
}

fn parse_for_render(raw: &str) -> RenderInput {
    use bridge_core::task_spec::{parse, validate, TaskSpecError};

    match parse(raw) {
        Ok(spec) => match validate(&spec) {
            Ok(()) => RenderInput::Spec(spec),
            Err(e) => RenderInput::Invalid(e.to_string()),
        },
        Err(TaskSpecError::NoTaskType) => RenderInput::Freeform(raw.to_string()),
        Err(e) => RenderInput::Invalid(e.to_string()),
    }
}

fn render_vars_for_input(input: &str) -> Result<Vec<(String, String)>, String> {
    match parse_for_render(input) {
        RenderInput::Freeform(raw) => Ok(vec![("input".to_string(), raw)]),
        RenderInput::Spec(spec) => {
            let mut vars = vec![(
                "input".to_string(),
                bridge_core::task_spec::body(&spec).to_string(),
            )];
            let mut task_vars = Vec::new();

            if let Some(schema) = bridge_core::task_spec::schema(&spec.task_type) {
                for section in schema.sections {
                    task_vars.push((
                        format!(
                            "task.{}",
                            bridge_core::task_spec::normalize_field_name(section.name)
                        ),
                        String::new(),
                    ));
                }
            }

            for (name, value) in bridge_core::task_spec::fields(&spec) {
                task_vars.push((format!("task.{name}"), value));
            }

            vars.extend(task_vars);
            Ok(vars)
        }
        RenderInput::Invalid(msg) => Err(msg),
    }
}

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

#[allow(clippy::too_many_arguments)]
fn node_turn_context(
    wf_id: &str,
    node: &WorkflowNode,
    run_id: &str,
    ctx: &WorkflowRunContext,
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    attempt: u32,
) -> TurnContext {
    TurnContext {
        turn_id: bridge_core::ids::TurnId::parse(format!(
            "turn-{run_id}-{node_id}-{attempt}",
            node_id = node.id.as_str()
        ))
        .unwrap_or_else(|_| {
            bridge_core::ids::TurnId::parse("turn-fallback").expect("fallback is valid")
        }),
        session_id: bridge_core::ids::ContextId::parse(run_id).unwrap_or_else(|_| {
            bridge_core::ids::ContextId::parse("workflow-fallback").expect("fallback is valid")
        }),
        task_id: ctx.task_id.clone(),
        workflow: Some(wf_id.to_string()),
        node: Some(node.id.as_str().to_string()),
        attempt,
        agent: node.agent.as_str().to_string(),
        model,
        effort,
        mode,
        prompt_id: ctx.prompt_id.clone(),
        traceparent: ctx.parent_traceparent.clone(),
    }
}

impl WorkflowExecutor {
    pub fn new(registry: Arc<dyn AgentRegistry>) -> Self {
        Self { registry }
    }

    /// Run one node: render its prompt from `vars`, resolve+configure+prompt+drain, forget.
    /// Returns (text, ok, usage, disposition). On any failure returns the error marker + ok=false.
    #[allow(clippy::too_many_arguments)]
    async fn run_node(
        &self,
        wf_id: &str,
        node: &WorkflowNode,
        vars: &HashMap<&str, &str>,
        run_id: &str,
        cancel: &CancellationToken,
        ctx: &WorkflowRunContext,
        diagnostic_factory: &Arc<dyn DiagnosticObserverFactory>,
        dispatcher: Option<&Arc<dyn WorkflowNodeDispatcher>>,
    ) -> (String, bool, Option<UsageSnapshot>, NodeDisposition) {
        if cancel.is_cancelled() {
            return (
                format!("[node {} canceled]", node.id.as_str()),
                false,
                None,
                NodeDisposition::Canceled,
            );
        }
        if let Some(d) = dispatcher {
            let rendered = render(&node.prompt_template, vars);
            let diagnostic = diagnostic_factory.make(&node.id, 1);
            // Emit NodeStarted before checkout (analogous to the cold path emitting before resolve).
            let obs_ctx = node_turn_context(wf_id, node, run_id, ctx, None, None, None, 1);
            ctx.observer
                .record(&ObsEvent::NodeStarted { ctx: &obs_ctx });
            let mut turn = match d
                .checkout_observed(wf_id, node, run_id, ctx, diagnostic.clone())
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    let fail_out = TurnOutcome::Failed(classify_failure(&e));
                    ctx.observer.record(&ObsEvent::NodeFinished {
                        ctx: &obs_ctx,
                        outcome: &fail_out,
                    });
                    return (
                        format!("[node {} failed: {:?}]", node.id.as_str(), e),
                        false,
                        None,
                        NodeDisposition::Failed,
                    );
                }
            };
            if cancel.is_cancelled() {
                let cleanup = turn
                    .cleanup
                    .on_exit_observed(NodeTurnExit::Normal, diagnostic.clone())
                    .await;
                let (text, outcome) = match cleanup {
                    Ok(()) => (
                        format!("[node {} canceled]", node.id.as_str()),
                        TurnOutcome::Canceled,
                    ),
                    Err(error) => (
                        format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error),
                        TurnOutcome::Failed(classify_failure(&error)),
                    ),
                };
                ctx.observer.record(&ObsEvent::NodeFinished {
                    ctx: &obs_ctx,
                    outcome: &outcome,
                });
                let disposition = NodeDisposition::from_turn(&outcome);
                return (text, false, None, disposition);
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

            ctx.observer
                .record(&ObsEvent::TurnStarted { ctx: &obs_ctx });
            let turn_start = std::time::Instant::now();
            let mut ttft_val: Option<std::time::Duration> = None;
            let rich_sink = ctx
                .make_rich_sink
                .as_ref()
                .map(|factory| factory.make(&node.id));

            let mut stream = tokio::select! {
                biased;
                // Prompt-open is the ownership boundary. If the backend result
                // and workflow cancellation are simultaneously ready, observe
                // the concrete backend result first so a structured failure
                // can arm warm-session expiry. A pending prompt still yields
                // immediately to cancellation.
                s = turn.backend.prompt_with_observers(
                    &turn.session,
                    parts,
                    BackendObservers::new(diagnostic.clone(), rich_sink.clone()),
                ) => match s {
                    Ok(s) => s,
                    Err(e) => {
                        turn
                            .cleanup
                            .arm_exit(&NodeTurnExit::Error(e.clone()));
                        if let Some(sink) = &rich_sink {
                            if let Err(flush_error) = sink.flush().await {
                                eprintln!(
                                    "rich sink flush failed after warm prompt error for node {}: {:?}",
                                    node.id.as_str(),
                                    flush_error
                                );
                            }
                        }
                        let text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                        let fail_out = TurnOutcome::Failed(classify_failure(&e));
                        let _ = turn
                            .cleanup
                            .on_exit_observed(NodeTurnExit::Error(e), diagnostic.clone())
                            .await;
                        ctx.observer.record(&ObsEvent::TurnFinished {
                            ctx: &obs_ctx,
                            latency: turn_start.elapsed(),
                            ttft: None,
                            outcome: &fail_out,
                        });
                        ctx.observer.record(&ObsEvent::UsageFinalized {
                            ctx: &obs_ctx,
                            usage: None,
                            fin: UsageFinalization::TurnFinal,
                        });
                        ctx.observer
                            .record(&ObsEvent::NodeFinished { ctx: &obs_ctx, outcome: &fail_out });
                        return (text, false, None, NodeDisposition::Failed);
                    }
                },
                _ = cancel.cancelled() => {
                    turn.cleanup.arm_exit(&NodeTurnExit::Canceled);
                    if let Some(sink) = &rich_sink {
                        if let Err(flush_error) = sink.flush().await {
                            eprintln!(
                                "rich sink flush failed after warm prompt-open cancellation for node {}: {:?}",
                                node.id.as_str(),
                                flush_error
                            );
                        }
                    }
                    let cleanup = turn
                        .cleanup
                        .on_exit_observed(NodeTurnExit::Canceled, diagnostic.clone())
                        .await;
                    let (text, outcome) = match cleanup {
                        Ok(()) => (
                            format!("[node {} canceled]", node.id.as_str()),
                            TurnOutcome::Canceled,
                        ),
                        Err(error) => (
                            format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error),
                            TurnOutcome::Failed(classify_failure(&error)),
                        ),
                    };
                    ctx.observer.record(&ObsEvent::TurnFinished {
                        ctx: &obs_ctx,
                        latency: turn_start.elapsed(),
                        ttft: None,
                        outcome: &outcome,
                    });
                    ctx.observer.record(&ObsEvent::UsageFinalized {
                        ctx: &obs_ctx,
                        usage: None,
                        fin: UsageFinalization::TurnFinal,
                    });
                    ctx.observer.record(&ObsEvent::NodeFinished {
                        ctx: &obs_ctx,
                        outcome: &outcome,
                    });
                    let disposition = NodeDisposition::from_turn(&outcome);
                    return (text, false, None, disposition);
                }
            };
            let mut text = String::new();
            let mut ok = true;
            let mut last_usage: Option<UsageSnapshot> = None;
            let mut exit = loop {
                tokio::select! {
                    biased;
                    // Prompt ownership already exists here. If a backend item
                    // and workflow cancellation are simultaneously ready, the
                    // concrete backend result wins so a queued AgentFailure can
                    // arm warm-session expiry. A pending stream still yields
                    // immediately to cancellation.
                    item = stream.next() => match item {
                        Some(Ok(Update::Text(t))) => {
                            if ttft_val.is_none() && !t.is_empty() {
                                ttft_val = Some(turn_start.elapsed());
                            }
                            text.push_str(&t);
                            if cancel.is_cancelled() {
                                ok = false;
                                text = format!("[node {} canceled]", node.id.as_str());
                                break NodeTurnExit::Canceled;
                            }
                        }
                        Some(Ok(Update::Permission(_))) => {
                            if cancel.is_cancelled() {
                                ok = false;
                                text = format!("[node {} canceled]", node.id.as_str());
                                break NodeTurnExit::Canceled;
                            }
                        }
                        Some(Ok(Update::Usage(mut u))) => {
                            if let Some(previous) = &last_usage {
                                u.merge_missing_from(previous);
                            }
                            last_usage = Some(u);
                            if cancel.is_cancelled() {
                                ok = false;
                                text = format!("[node {} canceled]", node.id.as_str());
                                break NodeTurnExit::Canceled;
                            }
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
                    },
                    _ = cancel.cancelled() => {
                        ok = false;
                        text = format!("[node {} canceled]", node.id.as_str());
                        break NodeTurnExit::Canceled;
                    }
                }
            };
            // The cleanup owner must learn the terminal state before rich-sink
            // flush, which is a cancellation point. Warm cleanup uses this to
            // make structured-failure expiry sticky if the executor is dropped
            // while the flush is pending.
            turn.cleanup.arm_exit(&exit);
            if let Some(sink) = &rich_sink {
                if let Err(flush_error) = sink.flush().await {
                    if ok && matches!(&exit, NodeTurnExit::Normal) {
                        ok = false;
                        text = format!(
                            "[node {} rich-flush failed: {:?}]",
                            node.id.as_str(),
                            flush_error
                        );
                        exit = NodeTurnExit::Error(flush_error);
                        turn.cleanup.arm_exit(&exit);
                    } else {
                        eprintln!(
                            "rich sink flush failed after warm node exit for {}: {:?}",
                            node.id.as_str(),
                            flush_error
                        );
                    }
                }
            }
            // Settle operation-owned cleanup before terminal observability. A
            // teardown failure is primary only when no earlier backend/rich
            // failure already owns the node outcome.
            let had_primary_failure = matches!(&exit, NodeTurnExit::Error(_));
            let mut node_outcome = match &exit {
                NodeTurnExit::Canceled => TurnOutcome::Canceled,
                NodeTurnExit::Error(error) => TurnOutcome::Failed(classify_failure(error)),
                NodeTurnExit::Normal if ok => TurnOutcome::Success,
                NodeTurnExit::Normal => TurnOutcome::Failed(FailureClass::Other),
            };
            let cleanup_result = turn
                .cleanup
                .on_exit_observed(exit, diagnostic.clone())
                .await;
            if let Err(error) = cleanup_result {
                if !had_primary_failure {
                    ok = false;
                    text = format!("[node {} cleanup failed: {:?}]", node.id.as_str(), error);
                    node_outcome = TurnOutcome::Failed(classify_failure(&error));
                }
                // Otherwise the operation observer recorded bounded teardown
                // evidence and the earlier backend/rich failure remains primary.
            }

            // Keep whatever usage the agent reported, even if the turn then errored or was
            // cancelled — the tokens were really consumed and belong in the durable footprint.
            // `last_usage` is already `None` when no `Update::Usage` was ever observed.
            ctx.observer.record(&ObsEvent::TurnFinished {
                ctx: &obs_ctx,
                latency: turn_start.elapsed(),
                ttft: ttft_val,
                outcome: &node_outcome,
            });
            ctx.observer.record(&ObsEvent::UsageFinalized {
                ctx: &obs_ctx,
                usage: last_usage.as_ref(),
                fin: UsageFinalization::TurnFinal,
            });
            ctx.observer.record(&ObsEvent::NodeFinished {
                ctx: &obs_ctx,
                outcome: &node_outcome,
            });
            let disposition = NodeDisposition::from_turn(&node_outcome);
            return (text, ok, last_usage, disposition);
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
                    NodeDisposition::Failed,
                )
            }
        };

        enum Attempt {
            Ok {
                text: String,
                usage: Option<UsageSnapshot>,
            },
            Canceled {
                marker: String,
                usage: Option<UsageSnapshot>,
            },
            Fatal {
                text: String,
                usage: Option<UsageSnapshot>,
                failure_class: FailureClass,
            },
            Transient {
                err: BridgeError,
                usage: Option<UsageSnapshot>,
                cleanup_allows_retry: bool,
            },
        }

        let attempts = node.retry.as_ref().map(|r| r.attempts()).unwrap_or(1);
        let retry_enabled = node.retry.is_some();

        // Emit NodeStarted exactly once before the retry loop.
        let node_obs_ctx = node_turn_context(wf_id, node, run_id, ctx, None, None, None, 0);
        ctx.observer
            .record(&ObsEvent::NodeStarted { ctx: &node_obs_ctx });

        let (final_text, final_ok, final_usage, final_node_outcome) = 'node_loop: {
            for attempt in 1..=attempts {
                if cancel.is_cancelled() {
                    break 'node_loop (
                        format!("[node {} canceled]", node.id.as_str()),
                        false,
                        None,
                        TurnOutcome::Canceled,
                    );
                }

                let should_retry_after_attempt = attempt < attempts;
                let mut obs_ctx_opt: Option<TurnContext> = None;
                let mut turn_started: Option<std::time::Instant> = None;
                let mut ttft_val: Option<std::time::Duration> = None;
                let diagnostic = diagnostic_factory.make(&node.id, attempt);
                let outcome = 'attempt: {
                    // resolve, with cancel
                    let resolved = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            break 'attempt Attempt::Canceled {
                                marker: format!("[node {} canceled]", node.id.as_str()),
                                usage: None,
                            };
                        }
                        r = self.registry.resolve_observed(&node.agent, diagnostic.clone()) => match r {
                            Ok(r) => r,
                            Err(e) => {
                                if retry_enabled && e.is_transient() {
                                    break 'attempt Attempt::Transient {
                                        err: e,
                                        usage: None,
                                        cleanup_allows_retry: true,
                                    };
                                }
                                break 'attempt Attempt::Fatal {
                                    text: format!("[node {} failed: {:?}]", node.id.as_str(), e),
                                    usage: None,
                                    failure_class: classify_failure(&e),
                                };
                            }
                        },
                    };
                    let obs_ctx_here = node_turn_context(
                        wf_id,
                        node,
                        run_id,
                        ctx,
                        resolved.entry.model.clone(),
                        resolved
                            .entry
                            .effort
                            .as_ref()
                            .map(|e| format!("{e:?}").to_ascii_lowercase()),
                        resolved.entry.mode.clone(),
                        attempt,
                    );
                    // NodeStarted was emitted before the loop; only emit TurnStarted here.
                    ctx.observer
                        .record(&ObsEvent::TurnStarted { ctx: &obs_ctx_here });
                    obs_ctx_opt = Some(obs_ctx_here);
                    turn_started = Some(std::time::Instant::now());
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
                        if retry_enabled && e.is_transient() {
                            let action = if should_retry_after_attempt {
                                ColdCleanupAction::Release
                            } else {
                                ColdCleanupAction::Forget
                            };
                            let cleanup_allows_retry = cleanup_cold_session(
                                &resolved.backend,
                                &session,
                                &diagnostic,
                                action,
                            )
                            .await
                            .is_ok();
                            break 'attempt Attempt::Transient {
                                err: e,
                                usage: None,
                                cleanup_allows_retry,
                            };
                        }
                        let _ = cleanup_cold_session(
                            &resolved.backend,
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                        )
                        .await;
                        break 'attempt Attempt::Fatal {
                            text: format!("[node {} failed: configure {:?}]", node.id.as_str(), e),
                            usage: None,
                            failure_class: classify_failure(&e),
                        };
                    }
                    if cancel.is_cancelled() {
                        match cleanup_cold_session(
                            &resolved.backend,
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                        )
                        .await
                        {
                            Ok(()) => {
                                break 'attempt Attempt::Canceled {
                                    marker: format!("[node {} canceled]", node.id.as_str()),
                                    usage: None,
                                };
                            }
                            Err(error) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} cleanup failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage: None,
                                    failure_class: classify_failure(&error),
                                };
                            }
                        }
                    }
                    // prompt, with cancel
                    let rich_sink = ctx
                        .make_rich_sink
                        .as_ref()
                        .map(|factory| factory.make(&node.id));
                    let mut stream = tokio::select! {
                        biased;
                        s = resolved.backend.prompt_with_observers(
                            &session,
                            vec![Part { text: rendered.clone() }],
                            BackendObservers::new(diagnostic.clone(), rich_sink.clone()),
                        ) => match s {
                            Ok(s) => s,
                            Err(e) => {
                                if let Some(sink) = &rich_sink {
                                    if let Err(flush_err) = sink.flush().await {
                                        eprintln!(
                                            "rich sink flush failed after prompt error for node {}: {:?}",
                                            node.id.as_str(),
                                            flush_err
                                        );
                                    }
                                }
                                if retry_enabled && e.is_transient() {
                                    let action = if should_retry_after_attempt {
                                        ColdCleanupAction::Release
                                    } else {
                                        ColdCleanupAction::Forget
                                    };
                                    let cleanup_allows_retry = cleanup_cold_session(
                                        &resolved.backend,
                                        &session,
                                        &diagnostic,
                                        action,
                                    )
                                    .await
                                    .is_ok();
                                    break 'attempt Attempt::Transient {
                                        err: e,
                                        usage: None,
                                        cleanup_allows_retry,
                                    };
                                }
                                let _ = cleanup_cold_session(
                                    &resolved.backend,
                                    &session,
                                    &diagnostic,
                                    ColdCleanupAction::Forget,
                                )
                                .await;
                                break 'attempt Attempt::Fatal {
                                    text: format!("[node {} failed: {:?}]", node.id.as_str(), e),
                                    usage: None,
                                    failure_class: classify_failure(&e),
                                };
                            }
                        },
                        _ = cancel.cancelled() => {
                            if let Some(sink) = &rich_sink {
                                if let Err(flush_error) = sink.flush().await {
                                    eprintln!(
                                        "rich sink flush failed after cold prompt-open cancellation for node {}: {:?}",
                                        node.id.as_str(),
                                        flush_error
                                    );
                                }
                            }
                            match cleanup_cold_session(
                                &resolved.backend,
                                &session,
                                &diagnostic,
                                ColdCleanupAction::Forget,
                            )
                            .await
                            {
                                Ok(()) => {
                                    break 'attempt Attempt::Canceled {
                                        marker: format!("[node {} canceled]", node.id.as_str()),
                                        usage: None,
                                    };
                                }
                                Err(error) => {
                                    break 'attempt Attempt::Fatal {
                                        text: format!(
                                            "[node {} cleanup failed: {:?}]",
                                            node.id.as_str(),
                                            error
                                        ),
                                        usage: None,
                                        failure_class: classify_failure(&error),
                                    };
                                }
                            }
                        }
                    };
                    let mut text = String::new();
                    let mut ok = true;
                    let mut canceled_during_drain = false;
                    let mut last_usage: Option<UsageSnapshot> = None;
                    let mut err: Option<BridgeError> = None;
                    loop {
                        tokio::select! {
                            biased;
                            item = stream.next() => match item {
                                Some(Ok(Update::Text(t))) => {
                                    if ttft_val.is_none() && !t.is_empty() {
                                        if let Some(start) = turn_started {
                                            ttft_val = Some(start.elapsed());
                                        }
                                    }
                                    text.push_str(&t);
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Permission(_))) => {
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Usage(mut u))) => {
                                    if let Some(previous) = &last_usage {
                                        u.merge_missing_from(previous);
                                    }
                                    last_usage = Some(u);
                                    if cancel.is_cancelled() {
                                        canceled_during_drain = true;
                                        ok = false;
                                        text = format!("[node {} canceled]", node.id.as_str());
                                        break;
                                    }
                                }
                                Some(Ok(Update::Done { stop_reason })) => {
                                    if stop_reason == STOP_REASON_CANCELLED { ok = false; }
                                    break;
                                }
                                Some(Err(e)) => {
                                    ok = false;
                                    text = format!("[node {} failed: {:?}]", node.id.as_str(), e);
                                    err = Some(e);
                                    break;
                                }
                                None => break,
                            },
                            _ = cancel.cancelled() => {
                                canceled_during_drain = true;
                                ok = false;
                                text = format!("[node {} canceled]", node.id.as_str());
                                break;
                            }
                        }
                    }
                    let cancel_error = if canceled_during_drain {
                        resolved
                            .backend
                            .cancel_observed(&session, diagnostic.clone())
                            .await
                            .err()
                    } else {
                        None
                    };
                    // Keep whatever usage the agent reported, even on error/cancel (see the warm path):
                    // `last_usage` is `None` only when no `Update::Usage` was ever observed.
                    let mut usage = last_usage;
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
                                usage = None;
                            } else {
                                let _ = cleanup_cold_session(
                                    &resolved.backend,
                                    &session,
                                    &diagnostic,
                                    ColdCleanupAction::Forget,
                                )
                                .await;
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} rich-flush failed: {:?}]",
                                        node.id.as_str(),
                                        e
                                    ),
                                    usage: None,
                                    failure_class: FailureClass::Other,
                                };
                            }
                        }
                    }
                    if canceled_during_drain {
                        let cleanup_error = cleanup_cold_session(
                            &resolved.backend,
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                        )
                        .await
                        .err();
                        match cancel_error.or(cleanup_error) {
                            Some(error) => {
                                break 'attempt Attempt::Fatal {
                                    text: format!(
                                        "[node {} cleanup failed: {:?}]",
                                        node.id.as_str(),
                                        error
                                    ),
                                    usage,
                                    failure_class: classify_failure(&error),
                                };
                            }
                            None => {
                                break 'attempt Attempt::Canceled {
                                    marker: text,
                                    usage,
                                };
                            }
                        }
                    }
                    if let Some(e) = err {
                        if retry_enabled && e.is_transient() {
                            let action = if should_retry_after_attempt {
                                ColdCleanupAction::Release
                            } else {
                                ColdCleanupAction::Forget
                            };
                            let cleanup_allows_retry = cleanup_cold_session(
                                &resolved.backend,
                                &session,
                                &diagnostic,
                                action,
                            )
                            .await
                            .is_ok();
                            break 'attempt Attempt::Transient {
                                err: e,
                                usage,
                                cleanup_allows_retry,
                            };
                        }
                        let fc = classify_failure(&e);
                        let _ = cleanup_cold_session(
                            &resolved.backend,
                            &session,
                            &diagnostic,
                            ColdCleanupAction::Forget,
                        )
                        .await;
                        break 'attempt Attempt::Fatal {
                            text,
                            usage,
                            failure_class: fc,
                        };
                    }
                    let cleanup = cleanup_cold_session(
                        &resolved.backend,
                        &session,
                        &diagnostic,
                        ColdCleanupAction::Forget,
                    )
                    .await;
                    if ok {
                        match cleanup {
                            Ok(()) => Attempt::Ok { text, usage },
                            Err(error) => Attempt::Fatal {
                                text: format!(
                                    "[node {} cleanup failed: {:?}]",
                                    node.id.as_str(),
                                    error
                                ),
                                usage,
                                failure_class: classify_failure(&error),
                            },
                        }
                    } else {
                        Attempt::Fatal {
                            text,
                            usage,
                            failure_class: FailureClass::Other,
                        }
                    }
                };

                match outcome {
                    Attempt::Canceled { marker, usage } => {
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: None,
                                outcome: &TurnOutcome::Canceled,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (marker, false, usage, TurnOutcome::Canceled);
                    }
                    Attempt::Ok { text, usage } => {
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: ttft_val,
                                outcome: &TurnOutcome::Success,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (text, true, usage, TurnOutcome::Success);
                    }
                    Attempt::Fatal {
                        text,
                        usage,
                        failure_class,
                    } => {
                        let fail_out = TurnOutcome::Failed(failure_class);
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: ttft_val,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        break 'node_loop (text, false, usage, fail_out);
                    }
                    Attempt::Transient {
                        err,
                        usage,
                        cleanup_allows_retry,
                    } => {
                        let err_for_log = err.clone();
                        let fail_class = classify_failure(&err);
                        let fail_out = TurnOutcome::Failed(fail_class);
                        if let (Some(obs_ctx), Some(start)) = (obs_ctx_opt.as_ref(), turn_started) {
                            ctx.observer.record(&ObsEvent::TurnFinished {
                                ctx: obs_ctx,
                                latency: start.elapsed(),
                                ttft: None,
                                outcome: &fail_out,
                            });
                            ctx.observer.record(&ObsEvent::UsageFinalized {
                                ctx: obs_ctx,
                                usage: usage.as_ref(),
                                fin: UsageFinalization::TurnFinal,
                            });
                        }
                        if should_retry_after_attempt && cleanup_allows_retry {
                            self.registry.invalidate(&node.agent).await;
                            tracing::warn!(
                                node = node.id.as_str(),
                                attempt,
                                error = ?err_for_log,
                                "node retry"
                            );
                            let retry = node.retry.as_ref().expect("retry attempts require policy");
                            tokio::select! {
                                biased;
                                _ = cancel.cancelled() => {
                                    break 'node_loop (
                                        format!("[node {} canceled]", node.id.as_str()),
                                        false,
                                        None,
                                        TurnOutcome::Canceled,
                                    );
                                }
                                _ = tokio::time::sleep(retry.backoff_for(attempt)) => {}
                            }
                            continue;
                        }
                        if should_retry_after_attempt {
                            break 'node_loop (
                                format!(
                                    "[node {} failed on attempt {attempt}: {err:?}]",
                                    node.id.as_str()
                                ),
                                false,
                                usage,
                                fail_out,
                            );
                        }
                        break 'node_loop (
                            format!(
                                "[node {} failed after {attempts} attempts: {err:?}]",
                                node.id.as_str()
                            ),
                            false,
                            usage,
                            fail_out,
                        );
                    }
                }
            }
            unreachable!("retry attempts are always at least one")
        };

        // Emit NodeFinished exactly once after the retry loop.
        ctx.observer.record(&ObsEvent::NodeFinished {
            ctx: &node_obs_ctx,
            outcome: &final_node_outcome,
        });
        let disposition = NodeDisposition::from_turn(&final_node_outcome);
        (final_text, final_ok, final_usage, disposition)
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
        self.run_with_diagnostic_context(
            graph,
            input,
            run_id,
            cancel,
            WorkflowDiagnosticContext::in_memory(ctx),
        )
    }

    pub fn run_with_diagnostic_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowDiagnosticContext,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context(graph, input, run_id, cancel, HashMap::new(), ctx)
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
        self.run_with_diagnostic_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            WorkflowDiagnosticContext::in_memory(ctx),
            dispatcher,
        )
    }

    pub fn run_with_diagnostic_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        ctx: WorkflowDiagnosticContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        self.run_from_with_diagnostic_context_and_dispatcher(
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
        self.run_from_with_diagnostic_context(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowDiagnosticContext::in_memory(ctx),
        )
    }

    pub fn run_from_with_diagnostic_context(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowDiagnosticContext,
    ) -> WorkflowStream {
        let (ctx, diagnostic_factory) = ctx.into_parts();
        self.run_from_with_context_inner(
            graph,
            input,
            run_id,
            cancel,
            seed,
            ctx,
            diagnostic_factory,
            None,
        )
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
        self.run_from_with_diagnostic_context_and_dispatcher(
            graph,
            input,
            run_id,
            cancel,
            seed,
            WorkflowDiagnosticContext::in_memory(ctx),
            dispatcher,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_from_with_diagnostic_context_and_dispatcher(
        &self,
        graph: Arc<WorkflowGraph>,
        input: String,
        run_id: String,
        cancel: CancellationToken,
        seed: HashMap<String, (String, bool, Option<UsageSnapshot>)>,
        ctx: WorkflowDiagnosticContext,
        dispatcher: Arc<dyn WorkflowNodeDispatcher>,
    ) -> WorkflowStream {
        let (ctx, diagnostic_factory) = ctx.into_parts();
        self.run_from_with_context_inner(
            graph,
            input,
            run_id,
            cancel,
            seed,
            ctx,
            diagnostic_factory,
            Some(dispatcher),
        )
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
        diagnostic_factory: Arc<dyn DiagnosticObserverFactory>,
        dispatcher: Option<Arc<dyn WorkflowNodeDispatcher>>,
    ) -> WorkflowStream {
        let this = WorkflowExecutor {
            registry: self.registry.clone(),
        };
        Box::pin(async_stream::stream! {
            let base_render_vars = match render_vars_for_input(&input) {
                Ok(vars) => vars,
                Err(msg) => {
                    yield Ok(WorkflowEvent::Terminal {
                        outcome: WorkflowOutcome::Failed,
                        output: msg,
                    });
                    return;
                }
            };

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

            let mut dispositions: HashMap<String, NodeDisposition> = seed
                .iter()
                .map(|(node, (_, ok, _))| {
                    (
                        node.clone(),
                        if *ok {
                            NodeDisposition::Completed
                        } else {
                            NodeDisposition::Failed
                        },
                    )
                })
                .collect();
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
                                let mut owned: Vec<(String, String)> = base_render_vars.clone();
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
                                let diagnostic_factory = diagnostic_factory.clone();
                                let dispatcher = dispatcher.clone();
                                let this = &this;
                                inflight.push(Box::pin(async move {
                                    let vars: HashMap<&str, &str> =
                                        owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                                    let (text, ok, usage, disposition) = this.run_node(
                                        &wf_id,
                                        &node,
                                        &vars,
                                        &run_id,
                                        &cancel,
                                        &ctx,
                                        &diagnostic_factory,
                                        dispatcher.as_ref(),
                                    ).await;
                                    (node.id.clone(), text, ok, usage, disposition)
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
            while let Some((node_id, text, ok, usage, disposition)) = inflight.next().await {
                yield Ok(WorkflowEvent::NodeFinished { node: node_id.clone(), ok, output: text.clone(), usage: usage.clone() });
                done.insert(node_id.as_str().to_string());
                dispositions.insert(node_id.as_str().to_string(), disposition);
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
            let (term_text, _, _usage) = outputs.get(&terminal_id).cloned().unwrap_or_default();
            let outcome = dispositions
                .get(&terminal_id)
                .copied()
                .map(NodeDisposition::workflow_outcome)
                .unwrap_or_else(|| {
                    if cancel.is_cancelled() {
                        WorkflowOutcome::Canceled
                    } else {
                        WorkflowOutcome::Failed
                    }
                });
            yield Ok(WorkflowEvent::Terminal { outcome, output: term_text });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{RetryPolicy, WorkflowGraph, WorkflowNode};
    use bridge_core::domain::{Part, PermissionRequest, RegistrySnapshot, SessionSpec};
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
            pre_authenticated: false,
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

    #[derive(Default)]
    struct MarkerDiagnostic;

    #[async_trait::async_trait]
    impl DiagnosticObserver for MarkerDiagnostic {
        async fn record(
            &self,
            _event: bridge_core::diagnostics::DiagnosticEvent,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    type RecordedDiagnostic = (String, u32, Arc<dyn DiagnosticObserver>);

    #[derive(Default)]
    struct RecordingDiagnosticFactory {
        made: Mutex<Vec<RecordedDiagnostic>>,
    }

    impl DiagnosticObserverFactory for RecordingDiagnosticFactory {
        fn make(&self, node: &NodeId, attempt: u32) -> Arc<dyn DiagnosticObserver> {
            let observer: Arc<dyn DiagnosticObserver> = Arc::new(MarkerDiagnostic);
            self.made
                .lock()
                .unwrap()
                .push((node.as_str().to_string(), attempt, observer.clone()));
            observer
        }
    }

    #[derive(Default)]
    struct RecordingRichSink {
        events: AtomicUsize,
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for RecordingRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {
            self.events.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct RecordingRichFactory {
        sink: Arc<RecordingRichSink>,
    }

    impl RichEventSinkFactory for RecordingRichFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    #[derive(Default)]
    struct FailingRichSink {
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for FailingRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {}

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::StoreFailure)
        }
    }

    struct FailingRichFactory {
        sink: Arc<FailingRichSink>,
    }

    impl RichEventSinkFactory for FailingRichFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    struct CompositePathBackend {
        prompts: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
        cleanups: Mutex<Vec<(&'static str, Arc<dyn DiagnosticObserver>)>>,
        calls: AtomicUsize,
        fail_first: bool,
    }

    impl CompositePathBackend {
        fn new(fail_first: bool) -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
                cleanups: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
                fail_first,
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentBackend for CompositePathBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("workflow must use prompt_with_observers")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            self.prompts.lock().unwrap().push(observers.diagnostic);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_first && call == 0 {
                return Err(BridgeError::AgentTimedOut);
            }
            if let Some(sink) = observers.rich {
                sink.record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
            }
            Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                stop_reason: "end_turn".into(),
            })])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("forget", observer));
            Ok(())
        }

        async fn release_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("release", observer));
            Ok(())
        }
    }

    struct CompositePathRegistry {
        backend: Arc<CompositePathBackend>,
        resolutions: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for CompositePathRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("workflow must use resolve_observed")
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.resolutions.lock().unwrap().push(observer);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {}

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    struct SingleBackendRegistry {
        backend: Arc<dyn AgentBackend>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for SingleBackendRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
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

    struct CompositePathCleanup;

    #[async_trait::async_trait]
    impl NodeTurnCleanup for CompositePathCleanup {
        async fn on_exit(self: Box<Self>, _exit: NodeTurnExit) {}
    }

    struct CompositePathDispatcher {
        backend: Arc<CompositePathBackend>,
        checkouts: Mutex<Vec<Arc<dyn DiagnosticObserver>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for CompositePathDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            panic!("warm workflow must use checkout_observed")
        }

        async fn checkout_observed(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<NodeTurn, BridgeError> {
            self.checkouts.lock().unwrap().push(observer);
            Ok(NodeTurn {
                backend: self.backend.clone(),
                session: SessionId::parse("workflow-observed-warm").unwrap(),
                seed: None,
                cleanup: Box::new(CompositePathCleanup),
            })
        }
    }
    pub(super) fn one_node_graph() -> Arc<WorkflowGraph> {
        one_node_graph_with_template("echo {{input}}")
    }

    fn one_node_graph_with_template(prompt_template: &str) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: prompt_template.into(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        })
    }

    fn retry_graph(retry: Option<RetryPolicy>) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "echo {{input}}".into(),
                inputs: vec![],
                retry,
            }],
            panel: None,
        })
    }

    fn retry_policy(max_attempts: u32, backoff_ms: u64) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            backoff_ms,
            backoff_cap_ms: None,
        }
    }

    fn usage(used: u64) -> UsageSnapshot {
        UsageSnapshot {
            used: Some(used),
            size: Some(10_000),
            cost: None,
            terminal: None,
            at_ms: used as i64,
        }
    }

    #[tokio::test]
    async fn cold_direct_workflow_threads_one_observer_and_preserves_one_rich_event() {
        let backend = Arc::new(CompositePathBackend::new(false));
        let registry = Arc::new(CompositePathRegistry {
            backend: backend.clone(),
            resolutions: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            task_id: Some(bridge_core::ids::TaskId::parse("correlation-only").unwrap()),
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                one_node_graph(),
                "input".into(),
                "direct-observed".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let resolutions = registry.resolutions.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].0, "only");
        assert_eq!(made[0].1, 1);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(prompts.len(), 1);
        assert_eq!(cleanups.len(), 1);
        assert_eq!(cleanups[0].0, "forget");
        assert!(Arc::ptr_eq(&made[0].2, &resolutions[0]));
        assert!(Arc::ptr_eq(&made[0].2, &prompts[0]));
        assert!(Arc::ptr_eq(&made[0].2, &cleanups[0].1));
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cold_retry_mints_one_observer_per_attempt_without_duplicating_rich_events() {
        let backend = Arc::new(CompositePathBackend::new(true));
        let registry = Arc::new(CompositePathRegistry {
            backend: backend.clone(),
            resolutions: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                retry_graph(Some(retry_policy(2, 0))),
                "input".into(),
                "retry-observed".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let resolutions = registry.resolutions.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();
        assert_eq!(
            made.iter()
                .map(|(_, attempt, _)| *attempt)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(resolutions.len(), 2);
        assert_eq!(prompts.len(), 2);
        assert_eq!(
            cleanups
                .iter()
                .map(|(action, _)| *action)
                .collect::<Vec<_>>(),
            ["release", "forget"]
        );
        for index in 0..2 {
            assert!(Arc::ptr_eq(&made[index].2, &resolutions[index]));
            assert!(Arc::ptr_eq(&made[index].2, &prompts[index]));
            assert!(Arc::ptr_eq(&made[index].2, &cleanups[index].1));
        }
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn warm_workflow_threads_checkout_observer_and_preserves_one_rich_event() {
        let backend = Arc::new(CompositePathBackend::new(false));
        let dispatcher = Arc::new(CompositePathDispatcher {
            backend: backend.clone(),
            checkouts: Mutex::new(Vec::new()),
        });
        let diagnostic_factory = Arc::new(RecordingDiagnosticFactory::default());
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };

        let events = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }))
        .run_with_diagnostic_context_and_dispatcher(
            one_node_graph(),
            "input".into(),
            "warm-observed".into(),
            CancellationToken::new(),
            WorkflowDiagnosticContext::new(context, diagnostic_factory.clone()),
            dispatcher.clone(),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(events.iter().all(Result::is_ok));

        let made = diagnostic_factory.made.lock().unwrap();
        let checkouts = dispatcher.checkouts.lock().unwrap();
        let prompts = backend.prompts.lock().unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(checkouts.len(), 1);
        assert_eq!(prompts.len(), 1);
        assert!(Arc::ptr_eq(&made[0].2, &checkouts[0]));
        assert!(Arc::ptr_eq(&made[0].2, &prompts[0]));
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn existing_task_journal_factory_drives_warm_dispatcher_diagnostics() {
        use bridge_core::diagnostics::{
            DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
            PersistedPhaseTransitionInput, PhaseStatus, TaskJournalDiagnosticObserverFactory,
        };
        use bridge_core::ids::{OperationId, TaskId};
        use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};

        fn event(status: PhaseStatus) -> DiagnosticEvent {
            DiagnosticEvent::new(
                PersistedPhaseTransition::build(
                    PersistedPhaseTransitionInput {
                        phase: DiagnosticPhase::Resolve,
                        status,
                        at_ms: 10,
                        operation: None,
                        code: None,
                        auth: None,
                    },
                    &DiagnosticRedactor::default(),
                )
                .unwrap(),
                None,
            )
            .unwrap()
        }

        struct JournalDispatcher {
            backend: Arc<CompositePathBackend>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for JournalDispatcher {
            async fn checkout(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                panic!("journal warm workflow must use checkout_observed")
            }

            async fn checkout_observed(
                &self,
                _wf_id: &str,
                _node: &WorkflowNode,
                _run_id: &str,
                _ctx: &WorkflowRunContext,
                observer: Arc<dyn DiagnosticObserver>,
            ) -> Result<NodeTurn, BridgeError> {
                observer.record(event(PhaseStatus::Started)).await?;
                observer.record(event(PhaseStatus::Completed)).await?;
                Ok(NodeTurn {
                    backend: self.backend.clone(),
                    session: SessionId::parse("workflow-journal-warm").unwrap(),
                    seed: None,
                    cleanup: Box::new(CompositePathCleanup),
                })
            }
        }

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("task-journal-warm").unwrap();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "w".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: "input".into(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();
        let factory: Arc<dyn DiagnosticObserverFactory> = Arc::new(
            TaskJournalDiagnosticObserverFactory::new(
                store.clone(),
                task.clone(),
                OperationId::parse("op-task-journal-warm").unwrap(),
            )
            .await
            .unwrap(),
        );
        let context = WorkflowRunContext {
            task_id: Some(task.clone()),
            ..WorkflowRunContext::default()
        };
        let backend = Arc::new(CompositePathBackend::new(false));

        let events = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }))
        .run_with_diagnostic_context_and_dispatcher(
            one_node_graph(),
            "input".into(),
            "journal-warm".into(),
            CancellationToken::new(),
            WorkflowDiagnosticContext::new(context, factory),
            Arc::new(JournalDispatcher { backend }),
        )
        .collect::<Vec<_>>()
        .await;
        assert!(events.iter().all(Result::is_ok));

        let journal = store.journal_from(&task, -1).await.unwrap();
        let diagnostics: Vec<_> = journal
            .iter()
            .filter_map(|event| match &event.kind {
                bridge_core::orch::OrchEventKind::Progress { progress } => {
                    progress.diagnostic_event()
                }
                _ => None,
            })
            .collect();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].transition().status(), PhaseStatus::Started);
        assert_eq!(diagnostics[1].transition().status(), PhaseStatus::Completed);
    }

    #[tokio::test]
    async fn renders_body_as_input_and_task_tokens() {
        let body = "# T\n\n## Description\nBuild it.\n\n## Acceptance Criteria\n- Works\n\n## Files\n- a.rs\n";
        let input = format!("---\ntask-type: implement\n---\n{body}");
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}::{{task.files}}::{{task.spec_refs}}"),
                input,
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last(),
            Some(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            }) if output == "OK"
        ));
        let prompt = &rec.prompts.lock().unwrap()[0];
        assert_eq!(prompt, &format!("{body}::- a.rs\n::"));
    }

    #[tokio::test]
    async fn bare_input_is_freeform_no_task_tokens() {
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}::{{task.files}}"),
                "plain task".into(),
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(matches!(
            events.last(),
            Some(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output
            }) if output == "OK"
        ));
        assert_eq!(rec.prompts.lock().unwrap()[0], "plain task::{{task.files}}");
    }

    #[tokio::test]
    async fn present_invalid_yields_failed_terminal() {
        let input =
            "---\ntask-type: implement\n---\n# T\n\n## Description\nBuild it.\n".to_string();
        let rec = Arc::new(Rec::default());
        let reg = Arc::new(FakeRegistry {
            backends: [("codex".to_string(), ("OK".to_string(), rec.clone()))].into(),
        });
        let ex = WorkflowExecutor::new(reg);

        let events: Vec<_> = ex
            .run(
                one_node_graph_with_template("{{input}}"),
                input,
                "r".into(),
                CancellationToken::new(),
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Failed,
                output
            } if output.contains("task-spec schema")
        ));
        assert!(
            rec.prompts.lock().unwrap().is_empty(),
            "present-invalid input must fail before spawning any node"
        );
    }

    #[derive(Clone)]
    enum RetryBehavior {
        SucceedsAfterInvalidates {
            required_invalidates: usize,
        },
        AlwaysTimedOutWithUsage {
            final_generation: usize,
            first_usage: UsageSnapshot,
            final_usage: UsageSnapshot,
        },
        NonTransientPrompt,
        ConfigInvalid,
        UsageThenPending {
            usage: UsageSnapshot,
            usage_notify: Arc<tokio::sync::Notify>,
        },
    }

    #[derive(Default)]
    struct RetryRec {
        resolve_count: AtomicUsize,
        invalidate_count: AtomicUsize,
        configure_count: AtomicUsize,
        prompt_count: AtomicUsize,
        release_count: AtomicUsize,
        forget_count: AtomicUsize,
        prompt_notify: tokio::sync::Notify,
        invalidate_notify: tokio::sync::Notify,
    }

    struct RetryBackend {
        behavior: RetryBehavior,
        generation: usize,
        rec: Arc<RetryRec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for RetryBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec.prompt_count.fetch_add(1, Ordering::SeqCst);
            self.rec.prompt_notify.notify_waiters();
            match &self.behavior {
                RetryBehavior::SucceedsAfterInvalidates {
                    required_invalidates,
                } => {
                    if self.generation < *required_invalidates {
                        Err(BridgeError::AgentOverloaded)
                    } else {
                        Ok(Box::pin(tokio_stream::iter(vec![
                            Ok(Update::Text("OK".into())),
                            Ok(Update::Done {
                                stop_reason: "end_turn".into(),
                            }),
                        ])))
                    }
                }
                RetryBehavior::AlwaysTimedOutWithUsage {
                    final_generation,
                    first_usage,
                    final_usage,
                } => {
                    let usage = if self.generation == *final_generation {
                        final_usage.clone()
                    } else {
                        first_usage.clone()
                    };
                    Ok(Box::pin(tokio_stream::iter(vec![
                        Ok(Update::Usage(usage)),
                        Err(BridgeError::AgentTimedOut),
                    ])))
                }
                RetryBehavior::NonTransientPrompt => Err(BridgeError::PermissionDenied),
                RetryBehavior::ConfigInvalid => Ok(Box::pin(tokio_stream::iter(Vec::<
                    Result<Update, BridgeError>,
                >::new(
                )))),
                RetryBehavior::UsageThenPending {
                    usage,
                    usage_notify,
                } => {
                    let usage = usage.clone();
                    let usage_notify = usage_notify.clone();
                    Ok(Box::pin(
                        futures::stream::once(async move {
                            usage_notify.notify_waiters();
                            Ok(Update::Usage(usage))
                        })
                        .chain(futures::stream::pending()),
                    ))
                }
            }
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _s: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            self.rec.configure_count.fetch_add(1, Ordering::SeqCst);
            if matches!(&self.behavior, RetryBehavior::ConfigInvalid) {
                Err(BridgeError::ConfigInvalid {
                    reason: "invalid test config".into(),
                })
            } else {
                Ok(())
            }
        }

        async fn forget_session(&self, _s: &SessionId) {
            self.rec.forget_count.fetch_add(1, Ordering::SeqCst);
        }

        async fn release_session(&self, _s: &SessionId) {
            self.rec.release_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct RetryRegistry {
        behavior: RetryBehavior,
        rec: Arc<RetryRec>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for RetryRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            self.rec.resolve_count.fetch_add(1, Ordering::SeqCst);
            let generation = self.rec.invalidate_count.load(Ordering::SeqCst);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: Arc::new(RetryBackend {
                    behavior: self.behavior.clone(),
                    generation,
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

        async fn invalidate(&self, _agent: &AgentId) {
            self.rec.invalidate_count.fetch_add(1, Ordering::SeqCst);
            self.rec.invalidate_notify.notify_waiters();
        }

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    async fn run_retry_case(
        behavior: RetryBehavior,
        retry: Option<RetryPolicy>,
        cancel: CancellationToken,
        rec: Arc<RetryRec>,
    ) -> Vec<WorkflowEvent> {
        let ex = WorkflowExecutor::new(Arc::new(RetryRegistry { behavior, rec }));
        ex.run(retry_graph(retry), "DIFF".into(), "run1".into(), cancel)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect()
    }

    fn only_node_finished(events: &[WorkflowEvent]) -> (&bool, &String, &Option<UsageSnapshot>) {
        match events
            .iter()
            .find(|e| matches!(e, WorkflowEvent::NodeFinished { .. }))
            .unwrap()
        {
            WorkflowEvent::NodeFinished {
                ok, output, usage, ..
            } => (ok, output, usage),
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn retry_succeeds_after_transient_failures() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::SucceedsAfterInvalidates {
                required_invalidates: 2,
            },
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, usage) = only_node_finished(&events);
        assert!(*ok, "node should recover after retry: {output}");
        assert_eq!(output, "OK");
        assert_eq!(usage, &None);
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 3);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 2);
        assert_eq!(rec.release_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_exhausts_then_degrades_with_last_usage() {
        let rec = Arc::new(RetryRec::default());
        let final_usage = usage(777);
        let events = run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: 1,
                first_usage: usage(111),
                final_usage: final_usage.clone(),
            },
            Some(retry_policy(2, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, reported_usage) = only_node_finished(&events);
        assert!(!*ok, "exhausted retry must degrade");
        assert!(
            output.contains("after 2 attempts"),
            "unexpected retry marker: {output}"
        );
        assert_eq!(reported_usage, &Some(final_usage));
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 2);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn non_transient_fails_without_retry() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::NonTransientPrompt,
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.contains("PermissionDenied"),
            "unexpected non-transient marker: {output}"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_retry_policy_is_single_attempt() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: 0,
                first_usage: usage(222),
                final_usage: usage(333),
            },
            None,
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.contains("AgentTimedOut"),
            "single-attempt path should keep today's marker: {output}"
        );
        assert!(
            !output.contains("after 1 attempts"),
            "retry marker must stay disabled when retry is None: {output}"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_mid_backoff_aborts_retry() {
        let rec = Arc::new(RetryRec::default());
        let cancel = CancellationToken::new();
        let run = tokio::spawn(run_retry_case(
            RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: usize::MAX,
                first_usage: usage(444),
                final_usage: usage(555),
            },
            Some(retry_policy(5, 60_000)),
            cancel.clone(),
            rec.clone(),
        ));

        while rec.invalidate_count.load(Ordering::SeqCst) == 0 {
            rec.invalidate_notify.notified().await;
        }
        cancel.cancel();
        let events = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancel must abort retry backoff promptly")
            .unwrap();
        let (ok, output, usage) = only_node_finished(&events);
        assert!(!*ok);
        assert_eq!(output, "[node only canceled]");
        assert_eq!(usage, &None);
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn drain_cancel_preserves_usage_without_retry_policy() {
        let rec = Arc::new(RetryRec::default());
        let cancel = CancellationToken::new();
        let usage_notify = Arc::new(tokio::sync::Notify::new());
        let observed_usage = usage(616);
        let run = tokio::spawn(run_retry_case(
            RetryBehavior::UsageThenPending {
                usage: observed_usage.clone(),
                usage_notify: usage_notify.clone(),
            },
            None,
            cancel.clone(),
            rec.clone(),
        ));

        tokio::time::timeout(std::time::Duration::from_secs(2), usage_notify.notified())
            .await
            .expect("backend should emit usage before hanging");
        cancel.cancel();

        let events = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("cancel must end the hanging drain promptly")
            .unwrap();
        let (ok, output, reported_usage) = only_node_finished(&events);
        assert!(!*ok);
        assert_eq!(output, "[node only canceled]");
        assert_eq!(reported_usage, &Some(observed_usage));
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dropped_mid_retry_emits_no_checkpoint() {
        let rec = Arc::new(RetryRec::default());
        let ex = WorkflowExecutor::new(Arc::new(RetryRegistry {
            behavior: RetryBehavior::AlwaysTimedOutWithUsage {
                final_generation: usize::MAX,
                first_usage: usage(444),
                final_usage: usage(555),
            },
            rec: rec.clone(),
        }));
        let mut stream = ex.run(
            retry_graph(Some(retry_policy(5, 60_000))),
            "DIFF".into(),
            "run1".into(),
            CancellationToken::new(),
        );

        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("executor should emit NodeStarted before retry backoff")
            .expect("stream should yield NodeStarted")
            .expect("NodeStarted should be Ok");
        assert!(
            matches!(first, WorkflowEvent::NodeStarted { .. }),
            "first event should be NodeStarted, got {first:?}"
        );
        let mut seen = vec![first];

        let next = stream.next();
        tokio::pin!(next);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if rec.invalidate_count.load(Ordering::SeqCst) > 0 {
                    break;
                }
                tokio::select! {
                    item = &mut next => {
                        let event = item
                            .expect("stream should remain open before retry backoff")
                            .expect("workflow event should be Ok before retry backoff");
                        seen.push(event);
                    }
                    _ = rec.invalidate_notify.notified() => {}
                }
            }
        })
        .await
        .expect("retry path should invalidate before the long backoff");

        // `next` is a `Pin<&mut Next>`; dropping it is a no-op for Drop but ends the borrow of
        // `stream` (NLL last-use) so `stream` itself can be dropped to simulate the crash.
        #[allow(clippy::drop_non_drop)]
        drop(next);
        drop(stream);

        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, WorkflowEvent::NodeFinished { .. })),
            "dropping the stream mid-backoff must not record NodeFinished"
        );
        assert_eq!(rec.resolve_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            rec.prompt_count.load(Ordering::SeqCst),
            1,
            "dropping the stream mid-backoff must not run another prompt"
        );
    }

    #[tokio::test]
    async fn retry_enabled_config_invalid_fails_fast() {
        let rec = Arc::new(RetryRec::default());
        let events = run_retry_case(
            RetryBehavior::ConfigInvalid,
            Some(retry_policy(3, 0)),
            CancellationToken::new(),
            rec.clone(),
        )
        .await;

        let (ok, output, _) = only_node_finished(&events);
        assert!(!*ok);
        assert!(
            output.starts_with("[node only failed: configure "),
            "unexpected configure marker: {output}"
        );
        assert_eq!(rec.configure_count.load(Ordering::SeqCst), 1);
        assert_eq!(rec.prompt_count.load(Ordering::SeqCst), 0);
        assert_eq!(rec.invalidate_count.load(Ordering::SeqCst), 0);
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
                        terminal: None,
                        at_ms: 1,
                    })),
                    Ok(Update::Usage(bridge_core::orch::UsageSnapshot {
                        used: None,
                        size: None,
                        cost: None,
                        terminal: Some(bridge_core::orch::TerminalUsage {
                            total_tokens: 321,
                            input_tokens: 300,
                            output_tokens: 21,
                            thought_tokens: None,
                            cached_read_tokens: None,
                            cached_write_tokens: None,
                        }),
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
                assert_eq!((u.used, u.size), (Some(15071), Some(258400)));
                assert_eq!(
                    u.terminal.as_ref().map(|usage| usage.total_tokens),
                    Some(321)
                );
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
                    terminal: None,
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

    #[tokio::test]
    async fn ready_backend_error_beats_simultaneous_workflow_cancellation_after_prompt_ownership() {
        struct ReadyErrorAndCancelBackend {
            cancel: CancellationToken,
        }

        #[async_trait::async_trait]
        impl AgentBackend for ReadyErrorAndCancelBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                // The prompt-open future wins its select while making the
                // cancellation branch ready for the immediately following
                // stream-drain select.
                self.cancel.cancel();
                Ok(Box::pin(tokio_stream::iter(vec![Err(
                    BridgeError::StoreFailure,
                )])))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyErrorDispatcher {
            cancel: CancellationToken,
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyErrorDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyErrorAndCancelBackend {
                        cancel: self.cancel.clone(),
                    }),
                    session: SessionId::parse("ready-error-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let cancel = CancellationToken::new();
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-race".into(),
                cancel.clone(),
                WorkflowRunContext::default(),
                Arc::new(ReadyErrorDispatcher {
                    cancel,
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            exits.lock().unwrap().as_slice(),
            ["error:StoreFailure"],
            "an already-ready backend failure owns the turn before simultaneous cancellation"
        );
        assert_eq!(
            workflow_terminal(&events),
            WorkflowOutcome::Failed,
            "the selected structured failure must also own the public workflow terminal"
        );
    }

    #[tokio::test]
    async fn ready_usage_burst_cannot_starve_workflow_cancellation() {
        struct ReadyUsageAndCancelBackend {
            cancel: CancellationToken,
            updates: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl AgentBackend for ReadyUsageAndCancelBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                self.cancel.cancel();
                let updates = self.updates.clone();
                let ready = futures::stream::iter((0..128).map(move |_| {
                    updates.fetch_add(1, Ordering::SeqCst);
                    Ok(Update::Usage(UsageSnapshot {
                        used: Some(1),
                        size: Some(10),
                        cost: None,
                        terminal: None,
                        at_ms: 0,
                    }))
                }))
                .chain(futures::stream::pending());
                Ok(Box::pin(ready))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyUsageDispatcher {
            cancel: CancellationToken,
            updates: Arc<AtomicUsize>,
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyUsageDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyUsageAndCancelBackend {
                        cancel: self.cancel.clone(),
                        updates: self.updates.clone(),
                    }),
                    session: SessionId::parse("ready-usage-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        let cancel = CancellationToken::new();
        let updates = Arc::new(AtomicUsize::new(0));
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let _events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-usage-cancel".into(),
                cancel.clone(),
                WorkflowRunContext::default(),
                Arc::new(ReadyUsageDispatcher {
                    cancel,
                    updates: updates.clone(),
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            updates.load(Ordering::SeqCst),
            1,
            "ready-result precedence may consume one benign item, then must honor cancellation"
        );
        assert_eq!(exits.lock().unwrap().as_slice(), ["canceled"]);
    }

    #[tokio::test]
    async fn ready_prompt_open_error_beats_cancellation_made_ready_after_precheck() {
        struct ReadyPromptErrorBackend;

        #[async_trait::async_trait]
        impl AgentBackend for ReadyPromptErrorBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                Err(BridgeError::StoreFailure)
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct ReadyPromptErrorDispatcher {
            exits: Arc<Mutex<Vec<String>>>,
        }

        #[async_trait::async_trait]
        impl WorkflowNodeDispatcher for ReadyPromptErrorDispatcher {
            async fn checkout(
                &self,
                _workflow: &str,
                _node: &WorkflowNode,
                _run: &str,
                _context: &WorkflowRunContext,
            ) -> Result<NodeTurn, BridgeError> {
                Ok(NodeTurn {
                    backend: Arc::new(ReadyPromptErrorBackend),
                    session: SessionId::parse("ready-prompt-error-and-cancel").unwrap(),
                    seed: None,
                    cleanup: Box::new(CountingCleanup {
                        calls: Arc::new(AtomicUsize::new(0)),
                        exits: self.exits.clone(),
                    }),
                })
            }
        }

        struct CancelAfterPrecheckFactory {
            cancel: CancellationToken,
            sink: Arc<RecordingRichSink>,
        }

        impl RichEventSinkFactory for CancelAfterPrecheckFactory {
            fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
                // `make` runs after run_node's eager cancellation check and
                // immediately before prompt-open ownership is selected. This
                // makes both select branches ready without a scheduler race.
                self.cancel.cancel();
                self.sink.clone()
            }
        }

        let cancel = CancellationToken::new();
        let exits = Arc::new(Mutex::new(Vec::new()));
        let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: HashMap::new(),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(CancelAfterPrecheckFactory {
                cancel: cancel.clone(),
                sink: Arc::new(RecordingRichSink::default()),
            })),
            ..WorkflowRunContext::default()
        };

        let events = executor
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "ready-prompt-race".into(),
                cancel,
                context,
                Arc::new(ReadyPromptErrorDispatcher {
                    exits: exits.clone(),
                }),
            )
            .collect::<Vec<_>>()
            .await;

        assert_eq!(
            exits.lock().unwrap().as_slice(),
            ["error:StoreFailure"],
            "an already-ready prompt-open failure owns the turn before simultaneous cancellation"
        );
        assert_eq!(
            workflow_terminal(&events),
            WorkflowOutcome::Failed,
            "the selected prompt-open failure must also own the public workflow terminal"
        );
    }

    struct ColdCleanupResultBackend {
        backend_error: Option<BridgeError>,
        cleanup_error: Option<BridgeError>,
        legacy_forgets: Arc<AtomicUsize>,
        checked_forgets: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdCleanupResultBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = match &self.backend_error {
                Some(error) => vec![Err(error.clone())],
                None => vec![Ok(Update::Done {
                    stop_reason: "end_turn".to_owned(),
                })],
            };
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session(&self, _session: &SessionId) {
            self.legacy_forgets.fetch_add(1, Ordering::SeqCst);
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.checked_forgets.fetch_add(1, Ordering::SeqCst);
            match &self.cleanup_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn final_review_cold_cleanup_failure_is_primary_only_without_backend_failure() {
        for (backend_error, expected_fragment) in [
            (None, "cleanup failed: StoreFailure"),
            (
                Some(BridgeError::ConfigMismatch { field: "model" }),
                "ConfigMismatch",
            ),
        ] {
            let legacy_forgets = Arc::new(AtomicUsize::new(0));
            let checked_forgets = Arc::new(AtomicUsize::new(0));
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdCleanupResultBackend {
                    backend_error,
                    cleanup_error: Some(BridgeError::StoreFailure),
                    legacy_forgets: legacy_forgets.clone(),
                    checked_forgets: checked_forgets.clone(),
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-cleanup-result".into(),
                    CancellationToken::new(),
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;
            let (ok, output) = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                    _ => None,
                })
                .unwrap();
            let terminal = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::Terminal { outcome, .. } => Some(outcome.clone()),
                    _ => None,
                })
                .unwrap();

            assert!(!ok);
            assert!(output.contains(expected_fragment), "{output}");
            assert_eq!(terminal, WorkflowOutcome::Failed);
            assert_eq!(checked_forgets.load(Ordering::SeqCst), 1);
            assert_eq!(legacy_forgets.load(Ordering::SeqCst), 0);
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ColdTransientSite {
        Configure,
        PromptOpen,
        Stream,
    }

    struct ColdTransientCleanupBackend {
        site: ColdTransientSite,
        configures: AtomicUsize,
        prompts: AtomicUsize,
        cleanups: Mutex<Vec<(&'static str, Arc<dyn DiagnosticObserver>)>>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdTransientCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let call = self.prompts.fetch_add(1, Ordering::SeqCst);
            match (self.site, call) {
                (ColdTransientSite::PromptOpen, 0) => Err(BridgeError::AgentTimedOut),
                (ColdTransientSite::Stream, 0) => Ok(Box::pin(tokio_stream::iter(vec![Err(
                    BridgeError::AgentTimedOut,
                )]))),
                _ => Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                    stop_reason: "end_turn".to_owned(),
                })]))),
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn configure_session(
            &self,
            _session: &SessionId,
            _spec: &SessionSpec,
        ) -> Result<(), BridgeError> {
            let call = self.configures.fetch_add(1, Ordering::SeqCst);
            if self.site == ColdTransientSite::Configure && call == 0 {
                Err(BridgeError::AgentTimedOut)
            } else {
                Ok(())
            }
        }

        async fn forget_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("forget", observer));
            Ok(())
        }

        async fn release_session_observed(
            &self,
            _session: &SessionId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.cleanups.lock().unwrap().push(("release", observer));
            Err(BridgeError::StoreFailure)
        }
    }

    struct ColdTransientRetryRegistry {
        backend: Arc<ColdTransientCleanupBackend>,
        resolutions: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for ColdTransientRetryRegistry {
        async fn resolve(&self, _id: &AgentId) -> Result<Resolved, BridgeError> {
            panic!("cold retry must use resolve_observed")
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            _observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(Resolved {
                entry: Arc::new(minimal_entry(id)),
                backend: self.backend.clone(),
                lease: Box::new(NoopLease),
            })
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }

        async fn apply(&self, _: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn invalidate(&self, _agent: &AgentId) {}

        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    async fn assert_cleanup_failure_vetoes_transient_retry(site: ColdTransientSite) {
        let backend = Arc::new(ColdTransientCleanupBackend {
            site,
            configures: AtomicUsize::new(0),
            prompts: AtomicUsize::new(0),
            cleanups: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(ColdTransientRetryRegistry {
            backend: backend.clone(),
            resolutions: AtomicUsize::new(0),
        });
        let factory = Arc::new(RecordingDiagnosticFactory::default());
        let events = WorkflowExecutor::new(registry.clone())
            .run_with_diagnostic_context(
                retry_graph(Some(retry_policy(2, 0))),
                "input".into(),
                "cold-cleanup-veto".into(),
                CancellationToken::new(),
                WorkflowDiagnosticContext::new(WorkflowRunContext::default(), factory.clone()),
            )
            .collect::<Vec<_>>()
            .await;
        let output = events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { output, .. } => Some(output),
                _ => None,
            })
            .unwrap();
        let made = factory.made.lock().unwrap();
        let cleanups = backend.cleanups.lock().unwrap();

        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
        assert!(output.contains("AgentTimedOut"), "{site:?}: {output}");
        assert!(!output.contains("StoreFailure"), "{site:?}: {output}");
        assert_eq!(registry.resolutions.load(Ordering::SeqCst), 1, "{site:?}");
        assert_eq!(backend.configures.load(Ordering::SeqCst), 1, "{site:?}");
        assert_eq!(
            backend.prompts.load(Ordering::SeqCst),
            usize::from(site != ColdTransientSite::Configure),
            "{site:?}"
        );
        assert_eq!(made.len(), 1, "{site:?}");
        assert_eq!(cleanups.len(), 1, "{site:?}");
        assert_eq!(cleanups[0].0, "release", "{site:?}");
        assert!(Arc::ptr_eq(&made[0].2, &cleanups[0].1), "{site:?}");
    }

    #[tokio::test]
    async fn final_review_configure_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::Configure).await;
    }

    #[tokio::test]
    async fn final_review_prompt_open_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::PromptOpen).await;
    }

    #[tokio::test]
    async fn final_review_stream_cleanup_failure_vetoes_transient_retry() {
        assert_cleanup_failure_vetoes_transient_retry(ColdTransientSite::Stream).await;
    }

    enum ColdReadyRace {
        PromptOpenError,
        StreamError,
        CancellationOnly,
    }

    struct ColdReadyRaceBackend {
        cancel: CancellationToken,
        race: ColdReadyRace,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdReadyRaceBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            match self.race {
                ColdReadyRace::PromptOpenError => Err(BridgeError::StoreFailure),
                ColdReadyRace::StreamError => {
                    self.cancel.cancel();
                    Ok(Box::pin(tokio_stream::iter(vec![Err(
                        BridgeError::StoreFailure,
                    )])))
                }
                ColdReadyRace::CancellationOnly => {
                    self.cancel.cancel();
                    Ok(Box::pin(futures::stream::pending()))
                }
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ColdCancelAfterPrecheckFactory {
        cancel: CancellationToken,
        sink: Arc<RecordingRichSink>,
    }

    impl RichEventSinkFactory for ColdCancelAfterPrecheckFactory {
        fn make(&self, _node: &NodeId) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.cancel.cancel();
            self.sink.clone()
        }
    }

    async fn run_cold_ready_race(
        race: ColdReadyRace,
        cancel_before_prompt_poll: bool,
    ) -> Vec<Result<WorkflowEvent, BridgeError>> {
        let cancel = CancellationToken::new();
        let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
            backend: Arc::new(ColdReadyRaceBackend {
                cancel: cancel.clone(),
                race,
            }),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: cancel_before_prompt_poll.then(|| {
                Arc::new(ColdCancelAfterPrecheckFactory {
                    cancel: cancel.clone(),
                    sink: Arc::new(RecordingRichSink::default()),
                }) as Arc<dyn RichEventSinkFactory>
            }),
            ..WorkflowRunContext::default()
        };
        executor
            .run_with_context(
                one_node_graph(),
                "input".into(),
                "cold-ready-race".into(),
                cancel,
                context,
            )
            .collect::<Vec<_>>()
            .await
    }

    fn workflow_terminal(events: &[Result<WorkflowEvent, BridgeError>]) -> WorkflowOutcome {
        events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { outcome, .. } => Some(outcome.clone()),
                _ => None,
            })
            .unwrap()
    }

    #[tokio::test]
    async fn final_review_cold_ready_prompt_open_error_beats_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::PromptOpenError, true).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
    }

    #[tokio::test]
    async fn final_review_cold_ready_stream_error_beats_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::StreamError, false).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
    }

    #[tokio::test]
    async fn final_review_cold_pending_stream_still_yields_to_cancellation() {
        let events = run_cold_ready_race(ColdReadyRace::CancellationOnly, false).await;
        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Canceled);
    }

    struct ColdCancellationCleanupBackend {
        cancel: CancellationToken,
        cancel_error: Option<BridgeError>,
        cleanup_error: Option<BridgeError>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdCancellationCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.cancel.cancel();
            Ok(Box::pin(futures::stream::pending()))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            match &self.cancel_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            match &self.cleanup_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn final_review_cold_cancellation_surfaces_cancel_or_cleanup_failure() {
        for (cancel_error, cleanup_error, expected) in [
            (None, None, WorkflowOutcome::Canceled),
            (
                Some(BridgeError::StoreFailure),
                None,
                WorkflowOutcome::Failed,
            ),
            (
                None,
                Some(BridgeError::StoreFailure),
                WorkflowOutcome::Failed,
            ),
        ] {
            let cancel = CancellationToken::new();
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdCancellationCleanupBackend {
                    cancel: cancel.clone(),
                    cancel_error,
                    cleanup_error,
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-cancel-cleanup".into(),
                    cancel,
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;
            assert_eq!(workflow_terminal(&events), expected);
        }
    }

    struct ColdPendingPromptCleanupBackend {
        checked_forgets: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdPendingPromptCleanupBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            std::future::pending().await
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn forget_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.checked_forgets.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::StoreFailure)
        }
    }

    #[tokio::test]
    async fn final_review_cold_prompt_open_cancellation_surfaces_cleanup_failure() {
        let cancel = CancellationToken::new();
        let checked_forgets = Arc::new(AtomicUsize::new(0));
        let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
            backend: Arc::new(ColdPendingPromptCleanupBackend {
                checked_forgets: checked_forgets.clone(),
            }),
        }));
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(ColdCancelAfterPrecheckFactory {
                cancel: cancel.clone(),
                sink: Arc::new(RecordingRichSink::default()),
            })),
            ..WorkflowRunContext::default()
        };
        let events = executor
            .run_with_context(
                one_node_graph(),
                "input".into(),
                "cold-prompt-cancel-cleanup".into(),
                cancel,
                context,
            )
            .collect::<Vec<_>>()
            .await;
        let output = events
            .iter()
            .filter_map(|event| event.as_ref().ok())
            .find_map(|event| match event {
                WorkflowEvent::Terminal { output, .. } => Some(output),
                _ => None,
            })
            .unwrap();

        assert_eq!(workflow_terminal(&events), WorkflowOutcome::Failed);
        assert!(output.contains("cleanup failed: StoreFailure"), "{output}");
        assert_eq!(checked_forgets.load(Ordering::SeqCst), 1);
    }

    #[derive(Clone, Copy)]
    enum ColdBenignUpdate {
        Text,
        Permission,
        Usage,
    }

    struct ColdReadyBenignBackend {
        cancel: CancellationToken,
        updates: Arc<AtomicUsize>,
        update: ColdBenignUpdate,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ColdReadyBenignBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.cancel.cancel();
            let updates = self.updates.clone();
            let update = self.update;
            let stream = futures::stream::iter((0..128).map(move |_| {
                updates.fetch_add(1, Ordering::SeqCst);
                Ok(match update {
                    ColdBenignUpdate::Text => Update::Text("ready".to_owned()),
                    ColdBenignUpdate::Permission => {
                        Update::Permission(PermissionRequest::with_id("ready-permission", false))
                    }
                    ColdBenignUpdate::Usage => Update::Usage(UsageSnapshot {
                        used: Some(1),
                        size: Some(10),
                        cost: None,
                        terminal: None,
                        at_ms: 0,
                    }),
                })
            }))
            .chain(futures::stream::pending());
            Ok(Box::pin(stream))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn final_review_cold_ready_benign_item_checks_cancellation_before_repolling() {
        for update in [
            ColdBenignUpdate::Text,
            ColdBenignUpdate::Permission,
            ColdBenignUpdate::Usage,
        ] {
            let cancel = CancellationToken::new();
            let updates = Arc::new(AtomicUsize::new(0));
            let executor = WorkflowExecutor::new(Arc::new(SingleBackendRegistry {
                backend: Arc::new(ColdReadyBenignBackend {
                    cancel: cancel.clone(),
                    updates: updates.clone(),
                    update,
                }),
            }));
            let events = executor
                .run_with_context(
                    one_node_graph(),
                    "input".into(),
                    "cold-ready-benign".into(),
                    cancel,
                    WorkflowRunContext::default(),
                )
                .collect::<Vec<_>>()
                .await;

            assert_eq!(updates.load(Ordering::SeqCst), 1);
            assert_eq!(workflow_terminal(&events), WorkflowOutcome::Canceled);
        }
    }

    struct ResultCleanup {
        result: Result<(), BridgeError>,
        exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl NodeTurnCleanup for ResultCleanup {
        async fn on_exit(self: Box<Self>, _exit: NodeTurnExit) {
            panic!("observed workflow execution must use result-bearing cleanup")
        }

        async fn on_exit_observed(
            self: Box<Self>,
            exit: NodeTurnExit,
            _observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<(), BridgeError> {
            self.exits.lock().unwrap().push(match exit {
                NodeTurnExit::Normal => "normal".to_owned(),
                NodeTurnExit::Canceled => "canceled".to_owned(),
                NodeTurnExit::Error(error) => format!("error:{error:?}"),
            });
            self.result
        }
    }

    struct ImmediateResultBackend {
        error: Option<BridgeError>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for ImmediateResultBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let updates = match &self.error {
                Some(error) => vec![Err(error.clone())],
                None => vec![Ok(Update::Done {
                    stop_reason: "end_turn".to_owned(),
                })],
            };
            Ok(Box::pin(tokio_stream::iter(updates)))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct ResultCleanupDispatcher {
        backend_error: Option<BridgeError>,
        cleanup_error: BridgeError,
        exits: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for ResultCleanupDispatcher {
        async fn checkout(
            &self,
            _workflow: &str,
            _node: &WorkflowNode,
            _run: &str,
            _context: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(ImmediateResultBackend {
                    error: self.backend_error.clone(),
                }),
                session: SessionId::parse("result-bearing-cleanup").unwrap(),
                seed: None,
                cleanup: Box::new(ResultCleanup {
                    result: Err(self.cleanup_error.clone()),
                    exits: self.exits.clone(),
                }),
            })
        }
    }

    #[tokio::test]
    async fn cleanup_failure_is_primary_only_without_an_earlier_backend_failure() {
        for (backend_error, expected_fragment) in [
            (None, "cleanup failed: StoreFailure"),
            (
                Some(BridgeError::ConfigMismatch { field: "model" }),
                "ConfigMismatch",
            ),
        ] {
            let exits = Arc::new(Mutex::new(Vec::new()));
            let executor = WorkflowExecutor::new(Arc::new(FakeRegistry {
                backends: HashMap::new(),
            }));
            let events = executor
                .run_with_context_and_dispatcher(
                    one_node_graph(),
                    "DIFF".into(),
                    "cleanup-result".into(),
                    CancellationToken::new(),
                    WorkflowRunContext::default(),
                    Arc::new(ResultCleanupDispatcher {
                        backend_error,
                        cleanup_error: BridgeError::StoreFailure,
                        exits: exits.clone(),
                    }),
                )
                .collect::<Vec<_>>()
                .await;
            let finished = events
                .iter()
                .filter_map(|event| event.as_ref().ok())
                .find_map(|event| match event {
                    WorkflowEvent::NodeFinished { ok, output, .. } => Some((*ok, output.clone())),
                    _ => None,
                })
                .unwrap();
            assert!(!finished.0);
            assert!(finished.1.contains(expected_fragment), "{}", finished.1);
            assert_eq!(exits.lock().unwrap().len(), 1);
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
                    terminal: None,
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
                    terminal: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
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
        let rich_sink = Arc::new(FailingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(FailingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(Arc::new(FakeRegistry {
            backends: std::collections::HashMap::new(),
        }));
        let events: Vec<_> = ex
            .run_with_context_and_dispatcher(
                one_node_graph(),
                "DIFF".into(),
                "run1".into(),
                CancellationToken::new(),
                context,
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
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
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
            checked_forgets: Arc<AtomicUsize>,
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

            async fn forget_session_checked(&self, _s: &SessionId) -> Result<(), BridgeError> {
                self.checked_forgets.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        struct CfgErrReg {
            rec: Arc<Rec>,
            checked_forgets: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl AgentRegistry for CfgErrReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: Arc::new(CfgErrBackend {
                        rec: self.rec.clone(),
                        checked_forgets: self.checked_forgets.clone(),
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
        let checked_forgets = Arc::new(AtomicUsize::new(0));
        let ex = WorkflowExecutor::new(Arc::new(CfgErrReg {
            rec: rec.clone(),
            checked_forgets: checked_forgets.clone(),
        }));
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
            0,
            "configure_session error must not fall back to result-discarding legacy cleanup"
        );
        assert_eq!(
            checked_forgets.load(Ordering::SeqCst),
            1,
            "configure_session error must use result-bearing cleanup"
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
            retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("refinenode").unwrap(),
                    agent: AgentId::parse("claude").unwrap(),
                    prompt_template: "refine against {{draft}} for {{input}}".into(),
                    inputs: vec![NodeId::parse("draftnode").unwrap()],
                    retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                    retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("member_b").unwrap(),
                    agent: AgentId::parse("member_b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("synth").unwrap(),
                    agent: AgentId::parse("synth").unwrap(),
                    prompt_template: "{{member_b}}\n{{workflow.costs}}".into(),
                    inputs: vec![
                        NodeId::parse("member_a").unwrap(),
                        NodeId::parse("member_b").unwrap(),
                    ],
                    retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: vec![],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("t").unwrap(),
                    agent: AgentId::parse("a").unwrap(),
                    prompt_template: "{{a}}{{b}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap(), NodeId::parse("b").unwrap()],
                    retry: None,
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
        struct SlowPrompt {
            rich_recorded: Arc<tokio::sync::Notify>,
        }
        #[async_trait::async_trait]
        impl AgentBackend for SlowPrompt {
            async fn prompt(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                panic!("cold prompt-open owner must use prompt_with_observers")
            }
            async fn prompt_with_observers(
                &self,
                _s: &SessionId,
                _p: Vec<Part>,
                observers: BackendObservers,
            ) -> Result<BackendStream, BridgeError> {
                observers
                    .rich
                    .expect("test supplies a rich sink")
                    .record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
                self.rich_recorded.notify_one();
                std::future::pending().await
            }
            async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        struct SReg {
            backend: Arc<SlowPrompt>,
        }
        #[async_trait::async_trait]
        impl AgentRegistry for SReg {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
                Ok(Resolved {
                    entry: Arc::new(minimal_entry(id)),
                    backend: self.backend.clone(),
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
        let rich_recorded = Arc::new(tokio::sync::Notify::new());
        let cancel_after_record = rich_recorded.clone();
        tokio::spawn(async move {
            cancel_after_record.notified().await;
            t2.cancel();
        });
        let rich_sink = Arc::new(RecordingRichSink::default());
        let context = WorkflowRunContext {
            make_rich_sink: Some(Arc::new(RecordingRichFactory {
                sink: rich_sink.clone(),
            })),
            ..WorkflowRunContext::default()
        };
        let ex = WorkflowExecutor::new(Arc::new(SReg {
            backend: Arc::new(SlowPrompt { rich_recorded }),
        }));
        let evs = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ex.run_with_context(one_node_graph(), "x".into(), "r".into(), token, context)
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
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
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
                    terminal: None,
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
            ..WorkflowRunContext::default()
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
            ..WorkflowRunContext::default()
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
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("b").unwrap(),
                    agent: AgentId::parse("b").unwrap(),
                    prompt_template: "got {{a}}".into(),
                    inputs: vec![NodeId::parse("a").unwrap()],
                    retry: None,
                },
                WorkflowNode {
                    id: NodeId::parse("c").unwrap(),
                    agent: AgentId::parse("c").unwrap(),
                    prompt_template: "got {{b}}".into(),
                    inputs: vec![NodeId::parse("b").unwrap()],
                    retry: None,
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

#[cfg(test)]
mod observability_tests {
    use super::*;
    use crate::graph::{RetryPolicy, WorkflowGraph, WorkflowNode};
    use bridge_core::domain::Part;
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, SessionId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::{
        AgentBackend, AgentRegistry, BackendStream, FailureClass, Lease, ObsEvent, Observer,
        Resolved, TurnOutcome, Update,
    };
    use futures::StreamExt;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio_util::sync::CancellationToken;

    #[derive(Default)]
    struct Rec(Mutex<Vec<&'static str>>);
    impl Observer for Rec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::NodeStarted { .. } => "node_started",
                ObsEvent::TurnStarted { .. } => "turn_started",
                ObsEvent::UsageFinalized { .. } => "usage",
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::NodeFinished { .. } => "node_finished",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    struct UsageBackend;
    #[async_trait::async_trait]
    impl AgentBackend for UsageBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, bridge_core::error::BridgeError> {
            let updates = vec![
                Ok(Update::Usage(UsageSnapshot {
                    used: Some(1),
                    size: Some(100),
                    cost: None,
                    terminal: None,
                    at_ms: 0,
                })),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), bridge_core::error::BridgeError> {
            Ok(())
        }
    }
    struct NoopLease2;
    impl Lease for NoopLease2 {}
    struct UsageRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for UsageRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, bridge_core::error::BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(UsageBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(
            &self,
            _: bridge_core::domain::RegistrySnapshot,
        ) -> Result<(), bridge_core::error::BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn workflow_node_emits_lifecycle_around_usage() {
        let rec = Arc::new(Rec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: Some("prompt/workflow".to_string()),
        };
        let graph = Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        });
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context(
            graph,
            "input".into(),
            "task-1".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec![
                "node_started",
                "turn_started",
                "turn_finished",
                "usage",
                "node_finished"
            ]
        );
    }

    // ---------------------------------------------------------------------------
    // Helpers shared by the remaining tests
    // ---------------------------------------------------------------------------

    /// Detailed recording observer: captures per-event structs so tests can
    /// inspect outcome and ttft values, not just tag order.
    #[derive(Default, Clone)]
    struct DetailedRec {
        events: Arc<Mutex<Vec<DetailedEvt>>>,
    }
    #[derive(Debug, Clone)]
    enum DetailedEvt {
        NodeStarted,
        TurnStarted,
        TurnFinished {
            outcome: TurnOutcome,
            ttft: Option<std::time::Duration>,
        },
        NodeFinished(TurnOutcome),
    }
    impl Observer for DetailedRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let ev = match e {
                ObsEvent::NodeStarted { .. } => DetailedEvt::NodeStarted,
                ObsEvent::TurnStarted { .. } => DetailedEvt::TurnStarted,
                ObsEvent::TurnFinished { outcome, ttft, .. } => DetailedEvt::TurnFinished {
                    outcome: (*outcome).clone(),
                    ttft: *ttft,
                },
                ObsEvent::NodeFinished { outcome, .. } => {
                    DetailedEvt::NodeFinished((*outcome).clone())
                }
                _ => return,
            };
            self.events.lock().unwrap().push(ev);
        }
    }

    /// A no-op `NodeTurnCleanup` for warm-path tests.
    struct NoopCleanup;
    #[async_trait::async_trait]
    impl NodeTurnCleanup for NoopCleanup {
        async fn on_exit(self: Box<Self>, _: NodeTurnExit) {}
    }

    /// Backend that emits `Update::Text(text)` then `Update::Done`.
    struct TextBackend(String);
    #[async_trait::async_trait]
    impl AgentBackend for TextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let t = self.0.clone();
            let updates = vec![
                Ok(Update::Text(t)),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct TextLease;
    impl Lease for TextLease {}

    fn make_single_node_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        })
    }

    fn make_retry_node_graph(policy: RetryPolicy) -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("wf").unwrap(),
            nodes: vec![WorkflowNode {
                id: bridge_core::ids::NodeId::parse("n").unwrap(),
                agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".to_string(),
                inputs: vec![],
                retry: Some(policy),
            }],
            panel: None,
        })
    }

    // ---------------------------------------------------------------------------
    // Test 1: warm dispatcher path emits full ordered lifecycle
    // ---------------------------------------------------------------------------

    struct TextDispatcher(String);
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for TextDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(TextBackend(self.0.clone())),
                session: SessionId::parse("warm-sess").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn warm_path_emits_full_lifecycle() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run1".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(TextDispatcher("hello".into())),
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        assert_eq!(evs.len(), 4, "expected 4 lifecycle events, got: {evs:?}");
        assert!(matches!(evs[0], DetailedEvt::NodeStarted), "evs[0]={evs:?}");
        assert!(matches!(evs[1], DetailedEvt::TurnStarted), "evs[1]={evs:?}");
        assert!(
            matches!(
                evs[2],
                DetailedEvt::TurnFinished {
                    outcome: TurnOutcome::Success,
                    ..
                }
            ),
            "evs[2]={evs:?}"
        );
        assert!(
            matches!(evs[3], DetailedEvt::NodeFinished(TurnOutcome::Success)),
            "evs[3]={evs:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 2: cold path TurnFinished.ttft is Some when text is emitted
    // ---------------------------------------------------------------------------

    struct TextRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for TextRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(TextBackend("hello".into())),
                lease: Box::new(TextLease),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn cold_path_ttft_some_when_text_emitted() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(TextRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run2".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let turn_finished = evs
            .iter()
            .find(|e| matches!(e, DetailedEvt::TurnFinished { .. }));
        let ttft = match turn_finished {
            Some(DetailedEvt::TurnFinished { ttft, .. }) => *ttft,
            _ => panic!("no TurnFinished event found in {evs:?}"),
        };
        assert!(
            ttft.is_some(),
            "expected ttft=Some(..) when backend emits text, got None"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 3: fatal AgentTimedOut → TurnFinished outcome = Failed(TimedOut)
    // ---------------------------------------------------------------------------

    struct TimedOutBackend;
    #[async_trait::async_trait]
    impl AgentBackend for TimedOutBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Err(BridgeError::AgentTimedOut)
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct TimedOutRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for TimedOutRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(TimedOutBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn fatal_timedout_produces_failed_timed_out_class() {
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        // No retry → AgentTimedOut is fatal on the only attempt.
        let exec = WorkflowExecutor::new(Arc::new(TimedOutRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run3".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let turn_finished = evs
            .iter()
            .find(|e| matches!(e, DetailedEvt::TurnFinished { .. }));
        match turn_finished {
            Some(DetailedEvt::TurnFinished {
                outcome: TurnOutcome::Failed(FailureClass::TimedOut),
                ..
            }) => {}
            other => panic!("expected TurnFinished(Failed(TimedOut)), got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------
    // Test 4: retry path — exactly 1 NodeStarted / 1 NodeFinished, 2 each TurnStarted / TurnFinished
    // ---------------------------------------------------------------------------

    /// Backend that fails with AgentTimedOut (transient) on the first `prompt` call,
    /// then succeeds with a text response on subsequent calls.
    struct FailOnceThenTextBackend {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for FailOnceThenTextBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return Err(BridgeError::AgentTimedOut);
            }
            let updates = vec![
                Ok(Update::Text("ok".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct FailOnceThenTextRegistry {
        calls: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl AgentRegistry for FailOnceThenTextRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            use bridge_core::domain::{AgentEntry, AgentKind};
            Ok(Resolved {
                entry: Arc::new(AgentEntry {
                    id: id.clone(),
                    cmd: Some("x".into()),
                    base_url: None,
                    api_key_env: None,
                    args: vec![],
                    kind: AgentKind::Acp,
                    model_provider: None,
                    model: None,
                    effort: None,
                    mode: None,
                    cwd: None,
                    session_cwd: None,
                    sandbox: None,
                    watchdog: None,
                    auth_method: None,
                    pre_authenticated: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(FailOnceThenTextBackend {
                    calls: self.calls.clone(),
                }),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[tokio::test]
    async fn retry_emits_node_started_once_node_finished_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let rec = Arc::new(DetailedRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(FailOnceThenTextRegistry { calls }));
        // 2 attempts, 0ms backoff so the test doesn't sleep.
        let graph = make_retry_node_graph(RetryPolicy {
            max_attempts: 2,
            backoff_ms: 0,
            backoff_cap_ms: Some(0),
        });
        let mut stream = exec.run_with_context(
            graph,
            "inp".into(),
            "run4".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let evs = rec.events.lock().unwrap().clone();
        let node_started = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::NodeStarted))
            .count();
        let node_finished = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::NodeFinished(..)))
            .count();
        let turn_started = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::TurnStarted))
            .count();
        let turn_finished = evs
            .iter()
            .filter(|e| matches!(e, DetailedEvt::TurnFinished { .. }))
            .count();

        assert_eq!(node_started, 1, "expected 1 NodeStarted; events: {evs:?}");
        assert_eq!(node_finished, 1, "expected 1 NodeFinished; events: {evs:?}");
        assert_eq!(
            turn_started, 2,
            "expected 2 TurnStarted (one per attempt); events: {evs:?}"
        );
        assert_eq!(
            turn_finished, 2,
            "expected 2 TurnFinished (one per attempt); events: {evs:?}"
        );
        // Final node outcome should be success (second attempt succeeded).
        assert!(
            matches!(
                evs.last(),
                Some(DetailedEvt::NodeFinished(TurnOutcome::Success))
            ),
            "expected final NodeFinished(Success); events: {evs:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // Test 5: TurnFinished emitted BEFORE UsageFinalized on both warm and cold paths
    // ---------------------------------------------------------------------------

    #[derive(Default)]
    struct OrderRec(Mutex<Vec<&'static str>>);
    impl Observer for OrderRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::UsageFinalized { .. } => "usage_finalized",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    #[derive(Default)]
    struct UsageFinalRec(Mutex<Vec<bool>>);
    impl Observer for UsageFinalRec {
        fn record(&self, e: &ObsEvent<'_>) {
            if let ObsEvent::UsageFinalized { usage, fin, .. } = e {
                if *fin == UsageFinalization::TurnFinal {
                    self.0.lock().unwrap().push(usage.is_some());
                }
            }
        }
    }

    #[derive(Default)]
    struct PromptOpenFinalizationRec(Mutex<Vec<&'static str>>);
    impl Observer for PromptOpenFinalizationRec {
        fn record(&self, e: &ObsEvent<'_>) {
            let tag = match e {
                ObsEvent::TurnFinished { .. } => "turn_finished",
                ObsEvent::UsageFinalized {
                    usage: None,
                    fin: UsageFinalization::TurnFinal,
                    ..
                } => "no_usage_finalized",
                ObsEvent::UsageFinalized { .. } => "unexpected_usage_finalization",
                ObsEvent::NodeFinished { .. } => "node_finished",
                _ => return,
            };
            self.0.lock().unwrap().push(tag);
        }
    }

    struct NoUsageIdleBackend;
    #[async_trait::async_trait]
    impl AgentBackend for NoUsageIdleBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            Ok(Box::pin(futures::stream::pending::<
                Result<Update, BridgeError>,
            >()))
        }

        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct NoUsageIdleRegistry;
    #[async_trait::async_trait]
    impl AgentRegistry for NoUsageIdleRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            Ok(Resolved {
                entry: Arc::new(super::tests::minimal_entry(id)),
                backend: Arc::new(NoUsageIdleBackend),
                lease: Box::new(NoopLease2),
            })
        }
        fn default_id(&self) -> AgentId {
            AgentId::parse("codex").unwrap()
        }
        async fn apply(&self, _: bridge_core::domain::RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }
        fn list(&self) -> Vec<AgentId> {
            vec![]
        }
    }

    #[derive(Default)]
    struct StartAndUsageFinalRec {
        started: AtomicUsize,
        usages: Mutex<Vec<bool>>,
    }
    impl Observer for StartAndUsageFinalRec {
        fn record(&self, e: &ObsEvent<'_>) {
            match e {
                ObsEvent::TurnStarted { .. } => {
                    self.started.fetch_add(1, Ordering::SeqCst);
                }
                ObsEvent::UsageFinalized { usage, fin, .. }
                    if *fin == UsageFinalization::TurnFinal =>
                {
                    self.usages.lock().unwrap().push(usage.is_some());
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn workflow_success_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(UsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(TextRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-success".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn workflow_failure_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(UsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(TimedOutRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-failure".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}

        let events = rec.0.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn workflow_cancel_without_usage_emits_explicit_no_usage() {
        let rec = Arc::new(StartAndUsageFinalRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let token = CancellationToken::new();
        let exec = WorkflowExecutor::new(Arc::new(NoUsageIdleRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-no-usage-cancel".into(),
            token.clone(),
            ctx,
        );
        let drain = tokio::spawn(async move { while stream.next().await.is_some() {} });

        for _ in 0..1000 {
            if rec.started.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert_eq!(rec.started.load(Ordering::SeqCst), 1);
        token.cancel();
        drain.await.unwrap();

        let events = rec.usages.lock().unwrap().clone();
        assert_eq!(events, vec![false]);
    }

    #[tokio::test]
    async fn turn_finished_emitted_before_usage_finalized_cold_path() {
        let rec = Arc::new(OrderRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context(
            make_single_node_graph(),
            "inp".into(),
            "run-order-cold".into(),
            CancellationToken::new(),
            ctx,
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec!["turn_finished", "usage_finalized"],
            "TurnFinished must precede UsageFinalized; got: {tags:?}"
        );
    }

    struct UsageDispatcher;
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for UsageDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            Ok(NodeTurn {
                backend: Arc::new(UsageBackend),
                session: SessionId::parse("warm-usage-sess").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn turn_finished_emitted_before_usage_finalized_warm_path() {
        let rec = Arc::new(OrderRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));
        let mut stream = exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run-order-warm".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(UsageDispatcher),
        );
        while stream.next().await.is_some() {}
        let tags = rec.0.lock().unwrap().clone();
        assert_eq!(
            tags,
            vec!["turn_finished", "usage_finalized"],
            "TurnFinished must precede UsageFinalized on warm path; got: {tags:?}"
        );
    }

    #[derive(Default)]
    struct PromptOpenRichSink {
        events: AtomicUsize,
        flushes: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::ports::RichEventSink for PromptOpenRichSink {
        fn record(&self, _kind: bridge_core::orch::OrchEventKind) {
            self.events.fetch_add(1, Ordering::SeqCst);
        }

        async fn flush(&self) -> Result<(), BridgeError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct PromptOpenRichFactory {
        sink: Arc<PromptOpenRichSink>,
    }

    impl bridge_core::ports::RichEventSinkFactory for PromptOpenRichFactory {
        fn make(
            &self,
            _node: &bridge_core::ids::NodeId,
        ) -> Arc<dyn bridge_core::ports::RichEventSink> {
            self.sink.clone()
        }
    }

    struct SlowPromptOpenDispatcher {
        rich_recorded: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for SlowPromptOpenDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            struct SlowPromptOpenBackend {
                rich_recorded: Arc<tokio::sync::Notify>,
            }
            #[async_trait::async_trait]
            impl AgentBackend for SlowPromptOpenBackend {
                async fn prompt(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                ) -> Result<BackendStream, BridgeError> {
                    panic!("warm prompt-open owner must use prompt_with_observers")
                }

                async fn prompt_with_observers(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                    observers: BackendObservers,
                ) -> Result<BackendStream, BridgeError> {
                    observers
                        .rich
                        .expect("test supplies a rich sink")
                        .record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
                    self.rich_recorded.notify_one();
                    std::future::pending().await
                }

                async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                    Ok(())
                }
            }

            Ok(NodeTurn {
                backend: Arc::new(SlowPromptOpenBackend {
                    rich_recorded: self.rich_recorded.clone(),
                }),
                session: SessionId::parse("warm-slow-prompt-open").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn dispatcher_cancel_during_prompt_open_emits_explicit_no_usage() {
        let rec = Arc::new(PromptOpenFinalizationRec::default());
        let token = CancellationToken::new();
        let cancel = token.clone();
        let rich_recorded = Arc::new(tokio::sync::Notify::new());
        let cancel_after_record = rich_recorded.clone();
        tokio::spawn(async move {
            cancel_after_record.notified().await;
            cancel.cancel();
        });
        let rich_sink = Arc::new(PromptOpenRichSink::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: Some(Arc::new(PromptOpenRichFactory {
                sink: rich_sink.clone(),
            })),
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            exec.run_with_context_and_dispatcher(
                make_single_node_graph(),
                "inp".into(),
                "run-cancel-prompt-open".into(),
                token,
                ctx,
                Arc::new(SlowPromptOpenDispatcher { rich_recorded }),
            )
            .collect::<Vec<_>>(),
        )
        .await
        .expect("cancellation must preempt warm prompt setup");

        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["turn_finished", "no_usage_finalized", "node_finished"]
        );
        assert_eq!(rich_sink.events.load(Ordering::SeqCst), 1);
        assert_eq!(rich_sink.flushes.load(Ordering::SeqCst), 1);
    }

    struct FailingPromptOpenDispatcher;
    #[async_trait::async_trait]
    impl WorkflowNodeDispatcher for FailingPromptOpenDispatcher {
        async fn checkout(
            &self,
            _wf_id: &str,
            _node: &WorkflowNode,
            _run_id: &str,
            _ctx: &WorkflowRunContext,
        ) -> Result<NodeTurn, BridgeError> {
            struct FailingPromptOpenBackend;
            #[async_trait::async_trait]
            impl AgentBackend for FailingPromptOpenBackend {
                async fn prompt(
                    &self,
                    _s: &SessionId,
                    _parts: Vec<Part>,
                ) -> Result<BackendStream, BridgeError> {
                    Err(BridgeError::AgentTimedOut)
                }

                async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
                    Ok(())
                }
            }

            Ok(NodeTurn {
                backend: Arc::new(FailingPromptOpenBackend),
                session: SessionId::parse("warm-failing-prompt-open").unwrap(),
                seed: None,
                cleanup: Box::new(NoopCleanup),
            })
        }
    }

    #[tokio::test]
    async fn dispatcher_prompt_error_emits_explicit_no_usage() {
        let rec = Arc::new(PromptOpenFinalizationRec::default());
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: None,
            observer: rec.clone(),
            parent_traceparent: None,
            task_id: None,
            prompt_id: None,
        };
        let exec = WorkflowExecutor::new(Arc::new(UsageRegistry));

        exec.run_with_context_and_dispatcher(
            make_single_node_graph(),
            "inp".into(),
            "run-error-prompt-open".into(),
            CancellationToken::new(),
            ctx,
            Arc::new(FailingPromptOpenDispatcher),
        )
        .collect::<Vec<_>>()
        .await;

        assert_eq!(
            rec.0.lock().unwrap().as_slice(),
            ["turn_finished", "no_usage_finalized", "node_finished"]
        );
    }
}
