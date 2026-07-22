// reattach.rs — per-task in-memory broadcast hub + wire types for streaming reattach.
// Consumed by Task 4 (DetachedProgressSink publishes) and Tasks 7-9 (SubscribeToTask reads).

use bridge_core::orch::{OrchEventKind, PlanEntry, TerminalStatus};
use serde::Serialize;
use tokio::sync::broadcast;

/// Whether this frame comes from a historical snapshot replay or from live streaming.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Snapshot,
    Live,
}

/// Serializable mirror of bridge_workflow::executor::WorkflowOutcome (which is not Serialize).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Completed,
    Failed,
    Canceled,
}

impl TerminalOutcome {
    pub fn from_workflow(o: &bridge_workflow::executor::WorkflowOutcome) -> Self {
        use bridge_workflow::executor::WorkflowOutcome as W;
        // Real variants (executor.rs:35-39): unit Completed, Failed, Canceled — no payloads.
        match o {
            W::Completed => Self::Completed,
            W::Failed => Self::Failed,
            W::Canceled => Self::Canceled,
        }
    }
}

/// The payload shape of a single progress frame on the broadcast channel.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameKind {
    AttemptLocator {
        execution_id: String,
        attempt_id: String,
        ordinal: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_attempt_id: Option<String>,
    },
    TelemetryUnavailable {
        reason: bridge_core::workflow_history::LedgerUnavailableReason,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        #[serde(rename = "tool_kind")]
        kind: String,
        status: String,
        locations: Vec<String>,
        content_preview: Option<String>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(rename = "tool_kind", skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        locations: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_preview: Option<String>,
    },
    NodeStarted {
        node: String,
    },
    NodeFinished {
        node: String,
        ok: bool,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<bridge_core::orch::UsageSnapshot>,
    },
    SnapshotComplete,
    Terminal {
        outcome: TerminalOutcome,
        output: String,
    },
}

/// A single wire frame carried over the broadcast channel and serialized to SSE.
///
/// `kind` is `#[serde(flatten)]` so the internally-tagged `FrameKind` discriminator
/// lands at the TOP level of the JSON (`{"v":1,"seq":5,"phase":"live","kind":"node_finished",
/// "node":"a","ok":true,"output":"o"}`) rather than nested as `"kind":{"kind":...}`. This is
/// the locked SSE wire contract (serialize-only; the bridge never deserializes this type).
#[derive(Clone, Debug, Serialize)]
pub struct WorkflowProgressFrame {
    pub v: u8,
    pub seq: i64,
    pub phase: Phase,
    #[serde(flatten)]
    pub kind: FrameKind,
}

pub fn project_orch_frame(
    kind: &OrchEventKind,
    phase: Phase,
    seq: i64,
) -> Option<WorkflowProgressFrame> {
    let kind = match kind {
        OrchEventKind::NodeStarted { node } => FrameKind::NodeStarted { node: node.clone() },
        OrchEventKind::NodeFinished {
            node,
            ok,
            output,
            usage,
        } => FrameKind::NodeFinished {
            node: node.clone(),
            ok: *ok,
            output: output.clone(),
            usage: usage.clone(),
        },
        OrchEventKind::Terminal { status, output } => FrameKind::Terminal {
            outcome: match status {
                TerminalStatus::Completed => TerminalOutcome::Completed,
                TerminalStatus::Failed { .. } => TerminalOutcome::Failed,
                TerminalStatus::Canceled => TerminalOutcome::Canceled,
            },
            output: output.clone(),
        },
        OrchEventKind::Progress { .. } | OrchEventKind::Usage { .. } => return None,
        OrchEventKind::Plan { entries } => FrameKind::Plan {
            entries: entries.clone(),
        },
        OrchEventKind::ToolCall {
            tool_call_id,
            title,
            kind,
            status,
            locations,
            content,
        } => FrameKind::ToolCall {
            tool_call_id: tool_call_id.clone(),
            title: title.clone(),
            kind: kind.clone(),
            status: status.clone(),
            locations: locations.clone(),
            content_preview: content.as_ref().map(|c| c.preview.clone()),
        },
        OrchEventKind::ToolCallUpdate {
            tool_call_id,
            title,
            kind,
            status,
            locations,
            content,
        } => FrameKind::ToolCallUpdate {
            tool_call_id: tool_call_id.clone(),
            title: title.clone(),
            kind: kind.clone(),
            status: status.clone(),
            locations: locations.clone(),
            content_preview: content.as_ref().map(|c| c.preview.clone()),
        },
    };

    Some(WorkflowProgressFrame {
        v: 1,
        seq,
        phase,
        kind,
    })
}

/// Per-task in-memory broadcast hub. Wraps a `tokio::sync::broadcast` channel so
/// the DetachedProgressSink (publisher) and SubscribeToTask handler (subscriber)
/// can communicate progress frames without sharing a lock.
pub struct TaskProgressHub {
    tx: broadcast::Sender<WorkflowProgressFrame>,
    terminal_published: std::sync::atomic::AtomicBool,
}

impl TaskProgressHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            terminal_published: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowProgressFrame> {
        self.tx.subscribe()
    }

    /// True only after summary/marker finalization has reached the terminal
    /// transport projection point. Reattach uses this latch to distinguish a
    /// primary terminal row that is still inside the finalization barrier.
    pub fn terminal_published(&self) -> bool {
        self.terminal_published
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Publish a frame best-effort: if no receivers are listening the send is silently dropped.
    pub fn publish(&self, f: WorkflowProgressFrame) {
        if matches!(&f.kind, FrameKind::Terminal { .. }) {
            self.terminal_published
                .store(true, std::sync::atomic::Ordering::Release);
        }
        let _ = self.tx.send(f);
    }
}

impl Default for TaskProgressHub {
    fn default() -> Self {
        Self::new()
    }
}

// One drain over the workflow stream, parameterized by a sink. The streaming
// producer (SSE) and the detached runner (TaskStore) share the drain, the
// WorkflowOutcome mapping, and the no-terminal guard — they differ only in sink.

use bridge_core::error::BridgeError;
use bridge_workflow::executor::{
    WorkflowCleanupDisposition, WorkflowEvent, WorkflowOutcome, WorkflowStream,
};
use futures::StreamExt;

/// A sink consumes the workflow's events. Intermediate node events are optional
/// (the detached sink persists each node_finished as a checkpoint in W3b); terminal is also required.
#[async_trait::async_trait]
pub trait WorkflowSink: Send {
    async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn node_finished(
        &mut self,
        _node: &str,
        _ok: bool,
        _output: &str,
        _usage: Option<&bridge_core::orch::UsageSnapshot>,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn cleanup_observed(
        &mut self,
        _disposition: WorkflowCleanupDisposition,
        _duration_ms: u64,
    ) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn terminal(
        &mut self,
        outcome: WorkflowOutcome,
        output: String,
    ) -> Result<(), BridgeError>;
    async fn error(&mut self, _err: BridgeError) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Drive the stream into the sink. Returns `true` if a terminal event was seen
/// (the caller handles the no-terminal case per its own sink semantics).
/// Returns `Err` on the first sink error, aborting the drain.
pub async fn drain_workflow<S: WorkflowSink>(
    stream: WorkflowStream,
    sink: &mut S,
) -> Result<bool, BridgeError> {
    drain_workflow_inner(stream, sink, None).await
}

/// Detached-owner drain: preserve the first sink error, cancel the workflow,
/// and keep polling until every already-in-flight node reaches its prompt/drain
/// cleanup and rich-sink flush path. No further sink calls occur after the
/// first error, so durable writes remain fail-closed while sibling ownership is
/// still settled.
pub async fn drain_workflow_cancel_on_sink_error<S: WorkflowSink>(
    stream: WorkflowStream,
    sink: &mut S,
    cancel: CancellationToken,
) -> Result<bool, BridgeError> {
    drain_workflow_inner(stream, sink, Some(cancel)).await
}

async fn drain_workflow_inner<S: WorkflowSink>(
    mut stream: WorkflowStream,
    sink: &mut S,
    cancel_on_error: Option<CancellationToken>,
) -> Result<bool, BridgeError> {
    let mut terminal_seen = false;
    let mut first_error = None;
    while let Some(item) = stream.next().await {
        if first_error.is_some() {
            continue;
        }
        let result = match item {
            Ok(WorkflowEvent::NodeStarted { node }) => sink.node_started(node.as_str()).await,
            Ok(WorkflowEvent::NodeFinished {
                node,
                ok,
                output,
                usage,
            }) => {
                sink.node_finished(node.as_str(), ok, &output, usage.as_ref())
                    .await
            }
            Ok(WorkflowEvent::CleanupObserved {
                disposition,
                duration_ms,
            }) => sink.cleanup_observed(disposition, duration_ms).await,
            Ok(WorkflowEvent::Terminal { outcome, output }) => {
                let result = sink.terminal(outcome, output).await;
                if result.is_ok() {
                    terminal_seen = true;
                }
                result
            }
            Err(e) => sink.error(e).await,
        };
        if let Err(error) = result {
            let Some(cancel) = cancel_on_error.as_ref() else {
                return Err(error);
            };
            first_error = Some(error);
            cancel.cancel();
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(terminal_seen),
    }
}

/// Unix-ms timestamp (server-side; `bridge-core` forbids `Date::now`, the server does not).
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

use bridge_core::ids::{NodeId, OperationId, TaskId};
use bridge_core::ports::{RichEventSink, RichEventSinkFactory};
use bridge_core::task_store::{ResumeClaim, TaskRecord, TaskRecordStatus, TaskStore};
use bridge_workflow::executor::{
    PromptDispatchBarrier, WorkflowDiagnosticContext, WorkflowExecutor, WorkflowRunContext,
};
use bridge_workflow::graph::WorkflowGraph;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct DetachedDeps {
    pub task_store: Arc<dyn TaskStore>,
    pub executor: Option<Arc<WorkflowExecutor>>,
    pub workflows: Arc<HashMap<bridge_core::ids::WorkflowId, Arc<WorkflowGraph>>>,
    pub workflow_cancels: Arc<Mutex<HashMap<TaskId, CancellationToken>>>,
    pub progress_hubs: Arc<Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>,
    pub clock: Arc<dyn crate::clock::Clock>,
    pub observer: Arc<dyn bridge_core::ports::Observer>,
    pub workflow_history: Option<
        Result<
            Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
            bridge_core::workflow_history::LedgerUnavailableReason,
        >,
    >,
}

/// Sticky, process-local telemetry degradation paired with the best-effort durable
/// locator marker. Once set it cannot be cleared by a later successful write.
#[derive(Clone, Default)]
pub struct AttemptTelemetryState {
    reason: Arc<std::sync::Mutex<Option<bridge_core::workflow_history::LedgerUnavailableReason>>>,
}

impl AttemptTelemetryState {
    pub fn record(&self, reason: bridge_core::workflow_history::LedgerUnavailableReason) {
        let mut slot = self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(reason);
        }
    }

    pub fn reason(&self) -> Option<bridge_core::workflow_history::LedgerUnavailableReason> {
        *self
            .reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Clone)]
pub struct TerminalSummaryBarrier {
    inner: Arc<TerminalSummaryBarrierInner>,
}

struct TerminalSummaryBarrierInner {
    history: Option<Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>>,
    telemetry: AttemptTelemetryState,
    task_store: Arc<dyn TaskStore>,
    task: TaskId,
    identity: bridge_core::ids::AttemptIdentity,
    started: std::time::Instant,
    queue_ms: u64,
    completion: WorkflowCompletionGuard,
}

struct WorkflowCompletionGuard {
    observer: Arc<dyn bridge_core::ports::Observer>,
    identity: bridge_core::ids::AttemptIdentity,
    workflow: String,
    settled: std::sync::atomic::AtomicBool,
}

struct TerminalBarrierSettlement {
    telemetry_unavailable: Option<bridge_core::workflow_history::LedgerUnavailableReason>,
    projection_ready: bool,
}

struct TerminalProjectionCommit {
    seq: i64,
    telemetry_unavailable: Option<bridge_core::workflow_history::LedgerUnavailableReason>,
}

/// Detached progress sink: persists each event via the sequenced store methods
/// (durable-first), then publishes a `WorkflowProgressFrame` to the task's
/// in-memory `TaskProgressHub`. A durable-write `Err` propagates (aborts the
/// drain) — preserving the W3b "checkpoint-write-failure ⇒ task Failed" contract.
pub struct DetachedProgressSink {
    store: Arc<dyn TaskStore>,
    task: TaskId,
    hub: Arc<TaskProgressHub>,
    terminal_barrier: Option<TerminalSummaryBarrier>,
    cleanup_observation: Option<(WorkflowCleanupDisposition, u64)>,
}

impl DetachedProgressSink {
    pub fn new(store: Arc<dyn TaskStore>, task: TaskId, hub: Arc<TaskProgressHub>) -> Self {
        Self {
            store,
            task,
            hub,
            terminal_barrier: None,
            cleanup_observation: None,
        }
    }

    pub fn with_terminal_barrier(mut self, barrier: Option<TerminalSummaryBarrier>) -> Self {
        self.terminal_barrier = barrier;
        self
    }
    fn operation_id(&self) -> Result<OperationId, BridgeError> {
        OperationId::parse(format!("op-{}", self.task.as_str()))
    }
}

#[async_trait::async_trait]
impl WorkflowSink for DetachedProgressSink {
    // `NodeId::parse` here uses a raw `?`: node names come from the already-validated
    // workflow graph, so this parse is infallible in practice. This sink
    // is only ever driven by the detached runner, which normalizes ANY drain `Err` to a
    // terminal `Failed` — so the parse error's disposition is moot on this path.
    async fn node_started(&mut self, node: &str) -> Result<(), BridgeError> {
        let node_id = bridge_core::ids::NodeId::parse(node)?;
        let operation_id = self.operation_id()?;
        let seq = self
            .store
            .record_node_started(&self.task, &node_id, &operation_id, now_ms())
            .await?;
        self.hub.publish(WorkflowProgressFrame {
            v: 1,
            seq,
            phase: Phase::Live,
            kind: FrameKind::NodeStarted {
                node: node.to_string(),
            },
        });
        Ok(())
    }

    async fn node_finished(
        &mut self,
        node: &str,
        ok: bool,
        output: &str,
        usage: Option<&bridge_core::orch::UsageSnapshot>,
    ) -> Result<(), BridgeError> {
        let node_id = bridge_core::ids::NodeId::parse(node)?;
        let operation_id = self.operation_id()?;
        let seq = self
            .store
            .put_node_checkpoint_sequenced(
                &self.task,
                &node_id,
                &operation_id,
                output,
                ok,
                now_ms(),
                usage,
            )
            .await?;
        self.hub.publish(WorkflowProgressFrame {
            v: 1,
            seq,
            phase: Phase::Live,
            kind: FrameKind::NodeFinished {
                node: node.to_string(),
                ok,
                output: output.to_string(),
                usage: usage.cloned(),
            },
        });
        Ok(())
    }

    async fn cleanup_observed(
        &mut self,
        disposition: WorkflowCleanupDisposition,
        duration_ms: u64,
    ) -> Result<(), BridgeError> {
        if self.cleanup_observation.is_some() {
            return Err(BridgeError::StoreFailure);
        }
        self.cleanup_observation = Some((disposition, duration_ms));
        Ok(())
    }

    async fn terminal(
        &mut self,
        outcome: WorkflowOutcome,
        output: String,
    ) -> Result<(), BridgeError> {
        let (status, result, error) = match &outcome {
            WorkflowOutcome::Completed => (
                bridge_core::task_store::TaskRecordStatus::Completed,
                Some(output.as_str()),
                None,
            ),
            WorkflowOutcome::Failed => (
                bridge_core::task_store::TaskRecordStatus::Failed,
                None,
                Some(output.as_str()),
            ),
            WorkflowOutcome::Canceled => (
                bridge_core::task_store::TaskRecordStatus::Canceled,
                None,
                None,
            ),
        };
        let (cleanup_disposition, cleanup_ms) = self
            .cleanup_observation
            .map(|(disposition, duration_ms)| (disposition.as_str(), duration_ms))
            .unwrap_or(("unknown", 0));
        let operation_id = self.operation_id()?;
        let commit = commit_terminal_projection(
            &self.store,
            &self.task,
            &operation_id,
            status,
            result,
            error,
            now_ms(),
            self.terminal_barrier.as_ref(),
            cleanup_disposition,
            cleanup_ms,
        )
        .await?;
        let seq = commit.seq;
        if let Some(reason) = commit.telemetry_unavailable {
            self.hub.publish(WorkflowProgressFrame {
                v: 1,
                seq: seq.saturating_sub(1),
                phase: Phase::Live,
                kind: FrameKind::TelemetryUnavailable { reason },
            });
        }
        self.hub.publish(WorkflowProgressFrame {
            v: 1,
            seq,
            phase: Phase::Live,
            kind: FrameKind::Terminal {
                outcome: TerminalOutcome::from_workflow(&outcome),
                output,
            },
        });
        Ok(())
    }
}

pub struct DetachedRichSink {
    store: Arc<dyn TaskStore>,
    task: TaskId,
    op: OperationId,
    hub: Arc<TaskProgressHub>,
    queue: std::sync::Mutex<VecDeque<OrchEventKind>>,
}

#[async_trait::async_trait]
impl RichEventSink for DetachedRichSink {
    fn record(&self, kind: OrchEventKind) {
        self.queue.lock().unwrap().push_back(kind);
    }

    async fn flush(&self) -> Result<(), BridgeError> {
        let kinds: Vec<_> = {
            let mut queue = self.queue.lock().unwrap();
            queue.drain(..).collect()
        };
        // Attempt EVERY queued row (do NOT abort on the first error) so one bad row can't drop the
        // rest of a node's liveness; report a failure if ANY row failed (the executor's happy-path
        // barrier maps that to a node failure). Each committed row's seq is strictly < the node's
        // later NodeFinished seq, so committed rows always precede NodeFinished (the ordering keystone).
        let mut first_err: Option<BridgeError> = None;
        for kind in kinds {
            match self
                .store
                .record_event_sequenced(&self.task, &self.op, now_ms(), kind.clone())
                .await
            {
                Ok(seq) => {
                    if let Some(frame) =
                        crate::detached::project_orch_frame(&kind, Phase::Live, seq)
                    {
                        self.hub.publish(frame);
                    }
                }
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

pub struct DetachedRichSinkFactory {
    pub store: Arc<dyn TaskStore>,
    pub task: TaskId,
    pub op: OperationId,
    pub hub: Arc<TaskProgressHub>,
}

impl RichEventSinkFactory for DetachedRichSinkFactory {
    fn make(&self, _node: &NodeId) -> Arc<dyn RichEventSink> {
        Arc::new(DetachedRichSink {
            store: self.store.clone(),
            task: self.task.clone(),
            op: self.op.clone(),
            hub: self.hub.clone(),
            queue: std::sync::Mutex::new(VecDeque::new()),
        })
    }
}

/// Drop guard: if the runner exits without finalizing (early return, error, or
/// **panic**), finalize `Failed` (via `finalize_detached`) and remove the cancel token.
/// `finalize_detached` writes the terminal through the SEQUENCED path (so `terminal_seq`
/// is never NULL), broadcasts a `Terminal{Failed}` frame to the task's hub, and removes
/// the hub from `progress_hubs` (the hub never leaks on panic). Within a serve lifetime a
/// `Working` row is then orphaned only if the sequenced write itself fails (the named §8
/// gap); the boot sweep is the cross-restart backstop.
pub struct Finalizer {
    pub store: Arc<dyn TaskStore>,
    pub task: TaskId,
    pub cancels: Arc<Mutex<std::collections::HashMap<TaskId, CancellationToken>>>,
    pub progress_hubs: Arc<Mutex<std::collections::HashMap<TaskId, Arc<TaskProgressHub>>>>,
    pub hub: Arc<TaskProgressHub>,
    pub terminal_barrier: Option<TerminalSummaryBarrier>,
    pub done: bool,
}

impl Drop for Finalizer {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let store = self.store.clone();
        let task = self.task.clone();
        let cancels = self.cancels.clone();
        let progress_hubs = self.progress_hubs.clone();
        let hub = self.hub.clone();
        let terminal_barrier = self.terminal_barrier.clone();
        tokio::spawn(async move {
            // Reuse the shared detached-finalize path: sequenced terminal + Terminal frame
            // broadcast + hub removal (so the panic path matches every other terminal).
            let _ = finalize_detached_with_barrier(
                &store,
                &progress_hubs,
                &task,
                TaskRecordStatus::Failed,
                None,
                Some("runner ended without terminal"),
                Some(&hub),
                terminal_barrier.as_ref(),
                "unknown",
            )
            .await;
            cancels.lock().await.remove(&task);
        });
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;
    use bridge_core::workflow_history::WorkflowHistoryStore;

    /// Sink whose `terminal` always returns an error — used to verify that
    /// `drain_workflow` aborts and propagates the error.
    struct FailTerminalSink;

    #[async_trait::async_trait]
    impl WorkflowSink for FailTerminalSink {
        async fn terminal(&mut self, _o: WorkflowOutcome, _out: String) -> Result<(), BridgeError> {
            Err(BridgeError::StoreFailure)
        }
    }

    #[tokio::test]
    async fn drain_aborts_on_sink_error() {
        let stream: WorkflowStream =
            Box::pin(futures::stream::iter(vec![Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "x".into(),
            })]));
        let mut sink = FailTerminalSink;
        assert!(drain_workflow(stream, &mut sink).await.is_err());
    }

    #[test]
    fn measured_terminal_partitions_queue_and_cleanup_out_of_work() {
        let terminal = measured_workflow_terminal(
            std::time::Instant::now() - std::time::Duration::from_millis(100),
            u64::MAX,
            1_000,
            "completed",
            bridge_core::workflow_history::NodeCounts::default(),
            true,
            30,
            20,
            "failed",
        );

        assert_eq!(terminal.queue_ms, 30);
        assert_eq!(terminal.cleanup_ms, 20);
        assert_eq!(terminal.cleanup_disposition, "failed");
        assert!(
            terminal.work_ms <= terminal.end_to_end_ms.saturating_sub(50),
            "queue and cleanup must never be charged to work: {terminal:?}"
        );
        assert_eq!(
            terminal.queue_ms + terminal.work_ms + terminal.cleanup_ms + terminal.finalization_ms,
            terminal.end_to_end_ms
        );
        assert_eq!(
            terminal
                .phase_durations
                .iter()
                .map(|phase| phase.duration_ms)
                .sum::<u64>(),
            terminal.end_to_end_ms
        );

        let no_wait = measured_workflow_terminal(
            std::time::Instant::now() - std::time::Duration::from_millis(20),
            u64::MAX,
            1_001,
            "completed",
            bridge_core::workflow_history::NodeCounts::default(),
            true,
            0,
            0,
            "not_needed",
        );
        assert_eq!(no_wait.queue_ms, 0);
        assert_eq!(no_wait.cleanup_ms, 0);
        assert_eq!(
            no_wait.work_ms + no_wait.finalization_ms,
            no_wait.end_to_end_ms
        );
    }
    struct FailFirstNodeSink {
        calls: usize,
    }

    #[async_trait::async_trait]
    impl WorkflowSink for FailFirstNodeSink {
        async fn node_finished(
            &mut self,
            _node: &str,
            _ok: bool,
            _output: &str,
            _usage: Option<&bridge_core::orch::UsageSnapshot>,
        ) -> Result<(), BridgeError> {
            self.calls += 1;
            Err(BridgeError::StoreFailure)
        }

        async fn terminal(
            &mut self,
            _outcome: WorkflowOutcome,
            _output: String,
        ) -> Result<(), BridgeError> {
            panic!("no sink calls are allowed after the first durable error")
        }
    }

    #[tokio::test]
    async fn detached_sink_error_cancels_and_drains_sibling_flush_before_returning() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cancel = CancellationToken::new();
        let sibling_cancel = cancel.clone();
        let rich_flushes = Arc::new(AtomicUsize::new(0));
        let sibling_flushes = rich_flushes.clone();
        let node = NodeId::parse("checkpoint-owner").unwrap();
        let first = futures::stream::once(async move {
            Ok(WorkflowEvent::NodeFinished {
                node,
                ok: true,
                output: "checkpoint".into(),
                usage: None,
            })
        });
        let sibling = futures::stream::once(async move {
            sibling_cancel.cancelled().await;
            sibling_flushes.fetch_add(1, Ordering::SeqCst);
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Canceled,
                output: "canceled after sibling flush".into(),
            })
        });
        let stream: WorkflowStream = Box::pin(first.chain(sibling));
        let mut sink = FailFirstNodeSink { calls: 0 };

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            drain_workflow_cancel_on_sink_error(stream, &mut sink, cancel.clone()),
        )
        .await
        .expect("cancel-and-drain must not park")
        .expect_err("the first checkpoint error remains primary");

        assert!(matches!(error, BridgeError::StoreFailure));
        assert!(cancel.is_cancelled());
        assert_eq!(sink.calls, 1);
        assert_eq!(rich_flushes.load(Ordering::SeqCst), 1);
    }

    /// Recording sink that logs the order of calls. Used to assert that
    /// `drain_workflow` fully awaits `node_finished` before delivering the next
    /// event (the sequential `while let … stream.next().await` loop guarantees
    /// this; the test makes it explicit and observable).
    struct RecordingSink {
        log: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl WorkflowSink for RecordingSink {
        async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> {
            self.log.push("node_started");
            Ok(())
        }
        async fn node_finished(
            &mut self,
            _node: &str,
            _ok: bool,
            _output: &str,
            _usage: Option<&bridge_core::orch::UsageSnapshot>,
        ) -> Result<(), BridgeError> {
            self.log.push("node_finished");
            Ok(())
        }
        async fn terminal(&mut self, _o: WorkflowOutcome, _out: String) -> Result<(), BridgeError> {
            self.log.push("terminal");
            Ok(())
        }
    }

    /// Proves that `drain_workflow` awaits `node_finished("a", …)` before
    /// delivering the next stream event (NodeStarted "b") to the sink.
    /// The hand-built stream emulates a 2-node (a → b) pipeline ordering:
    ///   NodeStarted(a) → NodeFinished(a) → NodeStarted(b) → NodeFinished(b) → Terminal
    /// The sink records every call in order; after drain we assert the exact
    /// sequence, which can only be correct if each `node_finished` is awaited
    /// before the loop advances to the next stream item.
    #[tokio::test]
    async fn drain_awaits_node_finished_before_next() {
        use bridge_core::ids::NodeId;
        let a = NodeId::parse("a").unwrap();
        let b = NodeId::parse("b").unwrap();
        let events = vec![
            Ok(WorkflowEvent::NodeStarted { node: a.clone() }),
            Ok(WorkflowEvent::NodeFinished {
                node: a.clone(),
                ok: true,
                output: "out-a".into(),
                usage: None,
            }),
            Ok(WorkflowEvent::NodeStarted { node: b.clone() }),
            Ok(WorkflowEvent::NodeFinished {
                node: b.clone(),
                ok: true,
                output: "out-b".into(),
                usage: None,
            }),
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "done".into(),
            }),
        ];
        let stream: WorkflowStream = Box::pin(futures::stream::iter(events));
        let mut sink = RecordingSink { log: Vec::new() };
        let result = drain_workflow(stream, &mut sink).await;
        assert!(result.is_ok());
        assert!(result.unwrap(), "terminal_seen should be true");
        // node_finished("a") must appear before node_started("b") in the log.
        assert_eq!(
            sink.log,
            vec![
                "node_started",
                "node_finished",
                "node_started",
                "node_finished",
                "terminal"
            ],
            "drain must fully await each sink call before advancing to the next event"
        );
    }

    // ── DetachedProgressSink tests ────────────────────────────────────────────

    /// Helper: build a minimal Working `TaskRecord` for use in store tests.
    fn make_task_record(id: &str) -> bridge_core::task_store::TaskRecord {
        bridge_core::task_store::TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    #[derive(Clone, Copy)]
    enum TerminalHistoryMode {
        Applied,
        Conflict,
        Error,
    }

    struct TerminalFaultHistory {
        inner: bridge_core::workflow_history::MemoryWorkflowHistoryStore,
        mode: TerminalHistoryMode,
        terminal_calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl bridge_core::workflow_history::WorkflowHistoryStore for TerminalFaultHistory {
        async fn reserve(
            &self,
            row: &bridge_core::workflow_history::AttemptReservation,
        ) -> Result<(), bridge_core::workflow_history::LedgerError> {
            self.inner.reserve(row).await
        }

        async fn mark_prompt_acceptance(
            &self,
            id: &bridge_core::ids::AttemptId,
            acceptance: &str,
        ) -> Result<(), bridge_core::workflow_history::LedgerError> {
            self.inner.mark_prompt_acceptance(id, acceptance).await
        }

        async fn terminalize(
            &self,
            id: &bridge_core::ids::AttemptId,
            terminal: &bridge_core::workflow_history::AttemptTerminal,
        ) -> Result<
            bridge_core::workflow_history::TerminalWrite,
            bridge_core::workflow_history::LedgerError,
        > {
            self.terminal_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.mode {
                TerminalHistoryMode::Applied => self.inner.terminalize(id, terminal).await,
                TerminalHistoryMode::Conflict => {
                    Ok(bridge_core::workflow_history::TerminalWrite::Conflict)
                }
                TerminalHistoryMode::Error => Err(bridge_core::workflow_history::LedgerError::new(
                    bridge_core::workflow_history::LedgerUnavailableReason::Io,
                )),
            }
        }

        async fn set_pinned(
            &self,
            id: &bridge_core::ids::AttemptId,
            pinned: bool,
        ) -> Result<bool, bridge_core::workflow_history::LedgerError> {
            self.inner.set_pinned(id, pinned).await
        }

        async fn interrupt_active(
            &self,
            completed_ms: i64,
        ) -> Result<u64, bridge_core::workflow_history::LedgerError> {
            self.inner.interrupt_active(completed_ms).await
        }

        async fn latest_reservation_for_task(
            &self,
            task: &TaskId,
        ) -> Result<
            Option<bridge_core::workflow_history::AttemptReservation>,
            bridge_core::workflow_history::LedgerError,
        > {
            self.inner.latest_reservation_for_task(task).await
        }

        async fn completed_between(
            &self,
            start_ms: i64,
            end_ms: i64,
        ) -> Result<
            Vec<bridge_core::workflow_history::CompletedAttempt>,
            bridge_core::workflow_history::LedgerError,
        > {
            self.inner.completed_between(start_ms, end_ms).await
        }
    }

    #[derive(Default)]
    struct WorkflowCompletionCounts {
        started: std::sync::atomic::AtomicUsize,
        finished: std::sync::atomic::AtomicUsize,
        stopped: std::sync::atomic::AtomicUsize,
        telemetry: std::sync::atomic::AtomicUsize,
        in_flight: std::sync::atomic::AtomicIsize,
    }

    impl bridge_core::ports::Observer for WorkflowCompletionCounts {
        fn record(&self, _event: &bridge_core::ports::ObsEvent<'_>) {}

        fn record_workflow(&self, event: &bridge_core::ports::WorkflowObsEvent<'_>) {
            use std::sync::atomic::Ordering;
            match event {
                bridge_core::ports::WorkflowObsEvent::Started { .. } => {
                    self.started.fetch_add(1, Ordering::SeqCst);
                    self.in_flight.fetch_add(1, Ordering::SeqCst);
                }
                bridge_core::ports::WorkflowObsEvent::Finished { .. } => {
                    self.finished.fetch_add(1, Ordering::SeqCst);
                    assert!(self.in_flight.fetch_sub(1, Ordering::SeqCst) > 0);
                }
                bridge_core::ports::WorkflowObsEvent::Stopped { .. } => {
                    self.stopped.fetch_add(1, Ordering::SeqCst);
                    assert!(self.in_flight.fetch_sub(1, Ordering::SeqCst) > 0);
                }
                bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { .. } => {
                    self.telemetry.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    impl WorkflowCompletionCounts {
        fn values(&self) -> (usize, usize, usize, usize, isize) {
            use std::sync::atomic::Ordering;
            (
                self.started.load(Ordering::SeqCst),
                self.finished.load(Ordering::SeqCst),
                self.stopped.load(Ordering::SeqCst),
                self.telemetry.load(Ordering::SeqCst),
                self.in_flight.load(Ordering::SeqCst),
            )
        }
    }

    #[tokio::test]
    async fn production_terminal_projection_balances_every_settlement_path_exactly_once() {
        struct Case {
            name: &'static str,
            status: TaskRecordStatus,
            history: TerminalHistoryMode,
            fail_primary: bool,
            fail_marker: bool,
            fail_projection_ready: bool,
            succeeds: bool,
            finishes: bool,
            telemetry_events: usize,
            terminal_calls: usize,
        }

        let cases = [
            Case {
                name: "normal_success",
                status: TaskRecordStatus::Completed,
                history: TerminalHistoryMode::Applied,
                fail_primary: false,
                fail_marker: false,
                fail_projection_ready: false,
                succeeds: true,
                finishes: true,
                telemetry_events: 0,
                terminal_calls: 1,
            },
            Case {
                name: "cancellation",
                status: TaskRecordStatus::Canceled,
                history: TerminalHistoryMode::Applied,
                fail_primary: false,
                fail_marker: false,
                fail_projection_ready: false,
                succeeds: true,
                finishes: true,
                telemetry_events: 0,
                terminal_calls: 1,
            },
            Case {
                name: "summary_conflict_marker_fallback",
                status: TaskRecordStatus::Failed,
                history: TerminalHistoryMode::Conflict,
                fail_primary: false,
                fail_marker: false,
                fail_projection_ready: false,
                succeeds: true,
                finishes: false,
                telemetry_events: 1,
                terminal_calls: 1,
            },
            Case {
                name: "summary_error_marker_fallback",
                status: TaskRecordStatus::Failed,
                history: TerminalHistoryMode::Error,
                fail_primary: false,
                fail_marker: false,
                fail_projection_ready: false,
                succeeds: true,
                finishes: false,
                telemetry_events: 1,
                terminal_calls: 1,
            },
            Case {
                name: "marker_persistence_error",
                status: TaskRecordStatus::Failed,
                history: TerminalHistoryMode::Error,
                fail_primary: false,
                fail_marker: true,
                fail_projection_ready: false,
                succeeds: false,
                finishes: false,
                telemetry_events: 1,
                terminal_calls: 1,
            },
            Case {
                name: "projection_persistence_error",
                status: TaskRecordStatus::Failed,
                history: TerminalHistoryMode::Applied,
                fail_primary: false,
                fail_marker: false,
                fail_projection_ready: true,
                succeeds: false,
                finishes: true,
                telemetry_events: 0,
                terminal_calls: 1,
            },
            Case {
                name: "primary_finalization_failure",
                status: TaskRecordStatus::Failed,
                history: TerminalHistoryMode::Applied,
                fail_primary: true,
                fail_marker: false,
                fail_projection_ready: false,
                succeeds: false,
                finishes: false,
                telemetry_events: 0,
                terminal_calls: 0,
            },
        ];

        for case in cases {
            let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
            let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
            let concrete_store = Arc::new(super::resume_tests::SelectiveFailureStore {
                inner: bridge_core::task_store::MemoryTaskStore::new(),
                fail_checkpoint: false,
                fail_diagnostic: false,
                fail_primary_terminal: case.fail_primary,
                fail_marker: case.fail_marker,
                fail_projection_ready: case.fail_projection_ready,
            });
            let store: Arc<dyn TaskStore> = concrete_store.clone();
            store
                .create_with_attempt_locator(
                    &make_task_record(task.as_str()),
                    &bridge_core::task_store::TaskAttemptLocator {
                        identity: identity.clone(),
                        telemetry_unavailable: None,
                    },
                )
                .await
                .unwrap();
            let history = Arc::new(TerminalFaultHistory {
                inner: bridge_core::workflow_history::MemoryWorkflowHistoryStore::new(),
                mode: case.history,
                terminal_calls: std::sync::atomic::AtomicUsize::new(0),
            });
            history
                .reserve(&bridge_core::workflow_history::AttemptReservation {
                    identity: identity.clone(),
                    task_id: Some(task.clone()),
                    workflow: "code-review".into(),
                    task_class: "workflow".into(),
                    surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                    policy: "r2f0a".into(),
                    workload_fingerprint: "shape-accounting".into(),
                    started_ms: 1,
                    workload_fingerprint_complete: true,
                    prompt_acceptance: "not_dispatched".into(),
                    pinned: false,
                })
                .await
                .unwrap();
            let observer = Arc::new(WorkflowCompletionCounts::default());
            let barrier = workflow_terminal_summary_barrier(
                Some(history.clone()),
                AttemptTelemetryState::default(),
                store.clone(),
                task.clone(),
                identity,
                observer.clone(),
                case.name.to_owned(),
                std::time::Instant::now(),
                0,
            );
            let duplicate_owner = barrier.clone();
            let hubs = Arc::new(Mutex::new(HashMap::new()));
            let result = finalize_detached_with_barrier(
                &store,
                &hubs,
                &task,
                case.status,
                (case.status == TaskRecordStatus::Completed).then_some("done"),
                (case.status == TaskRecordStatus::Failed).then_some("failed"),
                None,
                Some(&barrier),
                "complete",
            )
            .await;
            assert_eq!(result.is_ok(), case.succeeds, "{}", case.name);
            assert_eq!(
                history
                    .terminal_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                case.terminal_calls,
                "{}",
                case.name
            );

            if case.fail_primary {
                assert_eq!(observer.values(), (1, 0, 0, 0, 1), "{}", case.name);
            }
            drop(barrier);
            drop(duplicate_owner);
            assert_eq!(
                observer.values(),
                (
                    1,
                    usize::from(case.finishes),
                    usize::from(!case.finishes),
                    case.telemetry_events,
                    0,
                ),
                "{}",
                case.name
            );

            let public_task_row = store.get(&task).await.unwrap().unwrap();
            let pending_projection = store.pending_terminal_projection(&task).await.unwrap();
            let projection_pending = case.fail_marker || case.fail_projection_ready;
            assert_eq!(
                pending_projection.is_some(),
                projection_pending,
                "{}",
                case.name
            );
            let primary_task_row = pending_projection
                .as_ref()
                .map(|pending| &pending.task)
                .unwrap_or(&public_task_row);
            assert_eq!(
                primary_task_row.status,
                if case.fail_primary {
                    TaskRecordStatus::Working
                } else {
                    case.status
                },
                "{}",
                case.name
            );
            if projection_pending {
                assert_eq!(
                    public_task_row.status,
                    TaskRecordStatus::Working,
                    "{}: a pending projection must remain hidden from public reads",
                    case.name
                );
            }
        }
    }

    /// `DetachedProgressSink` persists durable events via the sequenced store
    /// methods AND publishes frames to the hub. Verifies:
    ///   - subscriber receives [NodeStarted, NodeFinished, Terminal] in order
    ///   - seqs are strictly monotonic
    ///   - the store snapshot has a checkpoint with a seq and a `terminal_seq`
    #[tokio::test]
    async fn detached_progress_sink_persists_and_publishes() {
        use crate::detached::{FrameKind, TaskProgressHub};
        use bridge_core::ids::NodeId;
        use bridge_core::task_store::{MemoryTaskStore, TaskStore};

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task_id = TaskId::parse("t-prog").unwrap();
        store.create(&make_task_record("t-prog")).await.unwrap();

        let hub = Arc::new(TaskProgressHub::new());
        // Subscribe BEFORE draining so we don't miss frames.
        let mut rx = hub.subscribe();

        let mut sink = DetachedProgressSink::new(store.clone(), task_id.clone(), hub);

        let a = NodeId::parse("a").unwrap();
        let events = vec![
            Ok(WorkflowEvent::NodeStarted { node: a.clone() }),
            Ok(WorkflowEvent::NodeFinished {
                node: a.clone(),
                ok: true,
                output: "out-a".into(),
                usage: None,
            }),
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "done".into(),
            }),
        ];
        let stream: WorkflowStream = Box::pin(futures::stream::iter(events));
        let terminal_seen = drain_workflow(stream, &mut sink).await.unwrap();
        assert!(terminal_seen, "terminal_seen must be true");

        // Assert durable state: checkpoint with seq + terminal_seq.
        let snap = store.progress_snapshot(&task_id).await.unwrap();
        assert_eq!(snap.checkpoints.len(), 1, "one checkpoint for node 'a'");
        let (_node, _output, ok, chk_seq) = &snap.checkpoints[0];
        assert!(*ok);
        assert!(*chk_seq > 0, "checkpoint must have a positive seq");
        assert!(snap.terminal_seq.is_some(), "terminal_seq must be set");
        let term_seq = snap.terminal_seq.unwrap();
        assert!(term_seq > *chk_seq, "terminal seq must be > checkpoint seq");

        // Collect published frames from the hub.
        let mut frames = Vec::new();
        while let Ok(f) = rx.try_recv() {
            frames.push(f);
        }
        assert_eq!(
            frames.len(),
            3,
            "expected NodeStarted, NodeFinished, Terminal frames"
        );

        // Verify frame kinds in order.
        assert!(
            matches!(frames[0].kind, FrameKind::NodeStarted { .. }),
            "frame[0] must be NodeStarted"
        );
        assert!(
            matches!(frames[1].kind, FrameKind::NodeFinished { .. }),
            "frame[1] must be NodeFinished"
        );
        assert!(
            matches!(frames[2].kind, FrameKind::Terminal { .. }),
            "frame[2] must be Terminal"
        );

        // Verify strictly monotonic seqs.
        assert!(
            frames[1].seq > frames[0].seq,
            "seqs must be strictly monotonic"
        );
        assert!(
            frames[2].seq > frames[1].seq,
            "seqs must be strictly monotonic"
        );

        // The terminal frame's seq must match the stored terminal_seq (durable-first).
        assert_eq!(
            frames[2].seq, term_seq,
            "terminal frame seq == stored terminal_seq"
        );

        // All frames are Live phase.
        assert!(matches!(frames[0].phase, crate::detached::Phase::Live));
        assert!(matches!(frames[1].phase, crate::detached::Phase::Live));
        assert!(matches!(frames[2].phase, crate::detached::Phase::Live));
    }

    #[tokio::test]
    async fn durable_marker_releases_hidden_terminal_projection() {
        use bridge_core::task_store::{MemoryTaskStore, TaskAttemptLocator, TaskStore};

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
        store
            .create_with_attempt_locator(
                &make_task_record(task.as_str()),
                &TaskAttemptLocator {
                    identity: identity.clone(),
                    telemetry_unavailable: None,
                },
            )
            .await
            .unwrap();
        let hub = Arc::new(TaskProgressHub::new());
        let mut receiver = hub.subscribe();
        let observer = Arc::new(WorkflowCompletionCounts::default());
        let barrier = workflow_terminal_summary_barrier(
            None,
            AttemptTelemetryState::default(),
            store.clone(),
            task.clone(),
            identity,
            observer.clone(),
            "code-review".into(),
            std::time::Instant::now(),
            0,
        );
        let mut sink = DetachedProgressSink::new(store.clone(), task.clone(), hub)
            .with_terminal_barrier(Some(barrier));
        let stream: WorkflowStream = Box::pin(futures::stream::iter(vec![
            Ok(WorkflowEvent::CleanupObserved {
                disposition: WorkflowCleanupDisposition::Complete,
                duration_ms: 17,
            }),
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "done".into(),
            }),
        ]));

        assert!(drain_workflow(stream, &mut sink).await.unwrap());
        assert!(store
            .pending_terminal_projection(&task)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
        let mut frames = Vec::new();
        while let Ok(frame) = receiver.try_recv() {
            frames.push(frame);
        }
        assert_eq!(frames.len(), 2);
        assert!(matches!(
            frames[0].kind,
            FrameKind::TelemetryUnavailable {
                reason: bridge_core::workflow_history::LedgerUnavailableReason::Open
            }
        ));
        assert!(matches!(frames[1].kind, FrameKind::Terminal { .. }));
        assert_eq!(
            frames[0].seq + 1,
            frames[1].seq,
            "marker sequence N must precede terminal sequence N+1"
        );
        assert_eq!(observer.values(), (1, 0, 1, 1, 0));
    }

    #[tokio::test]
    async fn boot_reconciler_replays_exact_pending_terminal_before_publication() {
        use bridge_core::task_store::{MemoryTaskStore, TaskAttemptLocator, TaskStore};
        use bridge_core::workflow_history::{
            AttemptReservation, AttemptTerminal, ExecutionSurface, MemoryWorkflowHistoryStore,
            NodeCounts, WorkflowHistoryStore,
        };

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
        store
            .create_with_attempt_locator(
                &make_task_record(task.as_str()),
                &TaskAttemptLocator {
                    identity: identity.clone(),
                    telemetry_unavailable: None,
                },
            )
            .await
            .unwrap();
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: "code-review".into(),
                task_class: "workflow".into(),
                surface: ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint: "shape-restart".into(),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            })
            .await
            .unwrap();
        let terminal = AttemptTerminal {
            completed_ms: 2_000,
            work_ms: 1,
            end_to_end_ms: 1,
            queue_ms: 0,
            cancellation_ms: 0,
            cleanup_ms: 0,
            finalization_ms: 0,
            outcome: "completed".into(),
            terminal_reason: "owner_finalized".into(),
            producer_terminal: "unknown".into(),
            final_message: "unknown".into(),
            process_liveness: "unknown".into(),
            terminal_evidence_capability: "unsupported".into(),
            terminal_evidence_version: "none".into(),
            terminal_evidence_source: "none".into(),
            terminal_evidence_complete: false,
            degraded: false,
            prompt_acceptance: "unknown".into(),
            cleanup_disposition: "complete".into(),
            node_counts: NodeCounts::default(),
            phase_durations: Vec::new(),
            telemetry_complete: true,
            monotonic_clock: true,
        };
        store
            .set_terminal_sequenced_pending(
                &task,
                &OperationId::parse("op-boot-terminal").unwrap(),
                TaskRecordStatus::Completed,
                Some("done"),
                None,
                terminal.completed_ms,
                &identity.attempt_id,
                &terminal,
            )
            .await
            .unwrap();
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Working
        );

        let selected: Arc<dyn WorkflowHistoryStore> = history.clone();
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: None,
            workflows: Arc::new(HashMap::new()),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(crate::clock::SystemClock),
            observer: Arc::new(bridge_observ::NoopObserver),
            workflow_history: Some(Ok(selected)),
        };
        assert!(reconcile_pending_terminal_projections(&deps).await);
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Completed
        );
        assert_eq!(
            history
                .attempt(&identity.attempt_id)
                .await
                .unwrap()
                .unwrap()
                .terminal,
            Some(terminal)
        );
        assert!(reconcile_pending_terminal_projections(&deps).await);
    }
    #[tokio::test]
    async fn successful_summary_balances_workflow_completion_exactly_once() {
        use bridge_core::task_store::{MemoryTaskStore, TaskAttemptLocator, TaskStore};
        use bridge_core::workflow_history::{
            AttemptReservation, ExecutionSurface, MemoryWorkflowHistoryStore, WorkflowHistoryStore,
        };

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let history = Arc::new(MemoryWorkflowHistoryStore::new());
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
        store
            .create_with_attempt_locator(
                &make_task_record(task.as_str()),
                &TaskAttemptLocator {
                    identity: identity.clone(),
                    telemetry_unavailable: None,
                },
            )
            .await
            .unwrap();
        history
            .reserve(&AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: "code-review".into(),
                task_class: "workflow".into(),
                surface: ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint: "shape-success".into(),
                started_ms: 1,
                workload_fingerprint_complete: true,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            })
            .await
            .unwrap();
        let observer = Arc::new(WorkflowCompletionCounts::default());
        let barrier = workflow_terminal_summary_barrier(
            Some(history.clone()),
            AttemptTelemetryState::default(),
            store.clone(),
            task.clone(),
            identity.clone(),
            observer.clone(),
            "code-review".into(),
            std::time::Instant::now(),
            0,
        );
        let mut sink = DetachedProgressSink::new(store, task, Arc::new(TaskProgressHub::new()))
            .with_terminal_barrier(Some(barrier));
        let stream: WorkflowStream = Box::pin(futures::stream::iter(vec![
            Ok(WorkflowEvent::CleanupObserved {
                disposition: WorkflowCleanupDisposition::NotNeeded,
                duration_ms: 0,
            }),
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "done".into(),
            }),
        ]));

        assert!(drain_workflow(stream, &mut sink).await.unwrap());
        assert_eq!(observer.values(), (1, 1, 0, 0, 0));
        assert!(history
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal
            .is_some());
    }
    #[tokio::test]
    async fn detached_sink_persists_and_publishes_node_usage() {
        use crate::detached::{FrameKind, TaskProgressHub};
        use bridge_core::orch::UsageSnapshot;
        use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskStore};

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("t-frame").unwrap();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "code-review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                last_artifact_ms: None,
                input: "DIFF".into(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
                artifacts_purged_at: None,
            })
            .await
            .unwrap();

        let hub = Arc::new(TaskProgressHub::new());
        let mut rx = hub.subscribe();
        let mut sink = DetachedProgressSink::new(store.clone(), task.clone(), hub);

        let usage = UsageSnapshot {
            used: Some(123),
            size: Some(1000),
            cost: None,
            terminal: None,
            at_ms: 1,
        };
        sink.node_finished("member", true, "OUT", Some(&usage))
            .await
            .unwrap();

        let frame = rx.try_recv().unwrap();
        match frame.kind {
            FrameKind::NodeFinished { usage: Some(u), .. } => assert_eq!(u.used, Some(123)),
            other => panic!("expected NodeFinished with usage, got {other:?}"),
        }

        let cps = store.node_checkpoints(&task).await.unwrap();
        assert_eq!(cps[0].3.as_ref().unwrap().used, Some(123));
    }

    /// A `DetachedProgressSink` whose store's `put_node_checkpoint_sequenced`
    /// returns `Err` must propagate the error so `drain_workflow` aborts —
    /// preserving the W3b "checkpoint-write-failure ⇒ task Failed" contract.
    #[tokio::test]
    async fn detached_progress_sink_write_error_aborts_drain() {
        use crate::detached::TaskProgressHub;
        use bridge_core::ids::NodeId;
        use bridge_core::task_store::{
            MemoryTaskStore, ResumeClaim, TaskProgressSnapshot, TaskRecord, TaskRecordStatus,
            TaskStore,
        };

        /// A `TaskStore` wrapper that delegates everything to an inner
        /// `MemoryTaskStore` but makes `put_node_checkpoint_sequenced` always
        /// return `Err(StoreFailure)`.
        struct FailingCheckpointStore {
            inner: MemoryTaskStore,
        }

        #[async_trait::async_trait]
        impl TaskStore for FailingCheckpointStore {
            async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
                self.inner.create(rec).await
            }
            async fn set_terminal(
                &self,
                id: &TaskId,
                status: TaskRecordStatus,
                result: Option<&str>,
                error: Option<&str>,
                updated_ms: i64,
            ) -> Result<(), BridgeError> {
                self.inner
                    .set_terminal(id, status, result, error, updated_ms)
                    .await
            }
            async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
                self.inner.get(id).await
            }
            async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
                self.inner.list(limit).await
            }
            async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
                self.inner.sweep_interrupted(updated_ms).await
            }
            async fn cancel_if_working(
                &self,
                id: &TaskId,
                updated_ms: i64,
            ) -> Result<bool, BridgeError> {
                self.inner.cancel_if_working(id, updated_ms).await
            }
            async fn put_node_checkpoint(
                &self,
                task: &TaskId,
                node: &bridge_core::ids::NodeId,
                output: &str,
                ok: bool,
                ts: i64,
            ) -> Result<(), BridgeError> {
                self.inner
                    .put_node_checkpoint(task, node, output, ok, ts)
                    .await
            }
            async fn node_checkpoints(
                &self,
                task: &TaskId,
            ) -> Result<
                Vec<(
                    bridge_core::ids::NodeId,
                    String,
                    bool,
                    Option<bridge_core::orch::UsageSnapshot>,
                )>,
                BridgeError,
            > {
                self.inner.node_checkpoints(task).await
            }
            async fn claim_resume_attempt(
                &self,
                task: &TaskId,
                cap: u32,
                now_ms: i64,
            ) -> Result<ResumeClaim, BridgeError> {
                self.inner.claim_resume_attempt(task, cap, now_ms).await
            }
            async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
                self.inner.working_tasks().await
            }
            async fn record_node_started(
                &self,
                task: &TaskId,
                node: &bridge_core::ids::NodeId,
                operation_id: &bridge_core::ids::OperationId,
                ts: i64,
            ) -> Result<i64, BridgeError> {
                self.inner
                    .record_node_started(task, node, operation_id, ts)
                    .await
            }
            /// Always fails — used to test the W3b abort-on-write-failure contract.
            #[allow(clippy::too_many_arguments)]
            async fn put_node_checkpoint_sequenced(
                &self,
                _task: &TaskId,
                _node: &bridge_core::ids::NodeId,
                _operation_id: &bridge_core::ids::OperationId,
                _output: &str,
                _ok: bool,
                _ts: i64,
                _usage: Option<&bridge_core::orch::UsageSnapshot>,
            ) -> Result<i64, BridgeError> {
                Err(BridgeError::StoreFailure)
            }
            async fn set_terminal_sequenced(
                &self,
                task: &TaskId,
                operation_id: &bridge_core::ids::OperationId,
                status: TaskRecordStatus,
                result: Option<&str>,
                error: Option<&str>,
                ts: i64,
            ) -> Result<i64, BridgeError> {
                self.inner
                    .set_terminal_sequenced(task, operation_id, status, result, error, ts)
                    .await
            }
            async fn journal_from(
                &self,
                task: &TaskId,
                after_seq: i64,
            ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
                self.inner.journal_from(task, after_seq).await
            }
            async fn progress_snapshot(
                &self,
                task: &TaskId,
            ) -> Result<TaskProgressSnapshot, BridgeError> {
                self.inner.progress_snapshot(task).await
            }
        }

        let failing_store: Arc<dyn TaskStore> = Arc::new(FailingCheckpointStore {
            inner: MemoryTaskStore::new(),
        });
        let task_id = TaskId::parse("t-fail").unwrap();
        failing_store
            .create(&make_task_record("t-fail"))
            .await
            .unwrap();

        let hub = Arc::new(TaskProgressHub::new());
        let mut rx = hub.subscribe(); // subscribe BEFORE the hub is moved into the sink
        let mut sink = DetachedProgressSink::new(failing_store.clone(), task_id.clone(), hub);

        let a = NodeId::parse("a").unwrap();
        // NodeStarted succeeds (record_node_started delegates to inner);
        // NodeFinished calls put_node_checkpoint_sequenced which always errors.
        let events = vec![
            Ok(WorkflowEvent::NodeStarted { node: a.clone() }),
            Ok(WorkflowEvent::NodeFinished {
                node: a.clone(),
                ok: true,
                output: "out-a".into(),
                usage: None,
            }),
            Ok(WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "done".into(),
            }),
        ];
        let stream: WorkflowStream = Box::pin(futures::stream::iter(events));
        let result = drain_workflow(stream, &mut sink).await;
        assert!(
            result.is_err(),
            "drain_workflow must return Err when checkpoint write fails"
        );

        // Durable-first: the failed NodeFinished must NOT have published a frame.
        // Only the successful NodeStarted should have reached the subscriber.
        match rx.try_recv() {
            Ok(f) => assert!(
                matches!(f.kind, FrameKind::NodeStarted { .. }),
                "first frame should be the NodeStarted that persisted successfully"
            ),
            Err(e) => panic!("expected the NodeStarted frame, got {e:?}"),
        }
        assert!(
            rx.try_recv().is_err(),
            "no frame may be published after the write failure (no-publish-on-error)"
        );
    }

    #[tokio::test]
    async fn detached_rich_sink_persists_diagnostic_without_live_frame() {
        use bridge_core::diagnostics::{
            DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
            PersistedPhaseTransitionInput, PhaseStatus,
        };
        use bridge_core::ids::OperationId;
        use bridge_core::orch::{OrchEventKind, ProgressPayload};
        use bridge_core::ports::RichEventSink;
        use bridge_core::task_store::{MemoryTaskStore, TaskStore};

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("t-diagnostic-live").unwrap();
        store
            .create(&make_task_record("t-diagnostic-live"))
            .await
            .unwrap();
        let hub = Arc::new(TaskProgressHub::new());
        let mut receiver = hub.subscribe();
        let sink = DetachedRichSink {
            store: store.clone(),
            task: task.clone(),
            op: OperationId::parse("op-t-diagnostic-live").unwrap(),
            hub,
            queue: std::sync::Mutex::new(VecDeque::new()),
        };
        let diagnostic = DiagnosticEvent::new(
            PersistedPhaseTransition::build(
                PersistedPhaseTransitionInput {
                    phase: DiagnosticPhase::Initialize,
                    status: PhaseStatus::Started,
                    at_ms: 42,
                    operation: None,
                    code: Some("acp.initialize.started".into()),
                    auth: None,
                },
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
            None,
        )
        .unwrap();

        sink.record(OrchEventKind::Progress {
            progress: ProgressPayload::diagnostic(diagnostic),
        });
        sink.record(OrchEventKind::Plan { entries: vec![] });
        sink.flush().await.unwrap();

        let events = store.journal_from(&task, -1).await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0].kind, OrchEventKind::Progress { .. }));
        assert!(matches!(events[1].kind, OrchEventKind::Plan { .. }));

        let frame = receiver.try_recv().unwrap();
        assert!(matches!(frame.kind, FrameKind::Plan { .. }));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn detached_node_journals_rich_before_nodefinished() {
        use crate::detached::TaskProgressHub;
        use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
        use bridge_core::ids::{AgentId, NodeId, OperationId, SessionId, WorkflowId};
        use bridge_core::orch::OrchEventKind;
        use bridge_core::ports::{
            AgentBackend, AgentRegistry, BackendStream, Lease, Resolved, RichEventSink, Update,
        };
        use bridge_core::task_store::{MemoryTaskStore, TaskStore};
        use bridge_workflow::executor::{WorkflowExecutor, WorkflowRunContext};
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};

        struct NoopLease;
        impl Lease for NoopLease {}

        struct RichBackend;

        #[async_trait::async_trait]
        impl AgentBackend for RichBackend {
            async fn prompt(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
            ) -> Result<BackendStream, BridgeError> {
                unreachable!("detached rich runs must call prompt_observed")
            }

            async fn prompt_observed(
                &self,
                _session: &SessionId,
                _parts: Vec<Part>,
                sink: Arc<dyn RichEventSink>,
            ) -> Result<BackendStream, BridgeError> {
                sink.record(OrchEventKind::ToolCall {
                    tool_call_id: "tc-1".into(),
                    title: "Read file".into(),
                    kind: "read".into(),
                    status: "completed".into(),
                    locations: vec!["src/lib.rs".into()],
                    content: None,
                });
                Ok(Box::pin(tokio_stream::iter(vec![
                    Ok(Update::Text("done".into())),
                    Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    }),
                ])))
            }

            async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
                Ok(())
            }
        }

        struct RichRegistry;

        #[async_trait::async_trait]
        impl AgentRegistry for RichRegistry {
            async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
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
                        host_fallback_eligible: false,
                        name: None,
                        description: None,
                        tags: vec![],
                        version: None,
                        mcp: vec![],
                        mcp_delivery: Default::default(),
                        extensions: Default::default(),
                    }),
                    backend: Arc::new(RichBackend),
                    lease: Box::new(NoopLease),
                })
            }

            fn default_id(&self) -> AgentId {
                AgentId::parse("codex").unwrap()
            }

            async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
                Ok(())
            }

            fn list(&self) -> Vec<AgentId> {
                vec![AgentId::parse("codex").unwrap()]
            }
        }

        fn kind_tag(kind: &OrchEventKind) -> &'static str {
            match kind {
                OrchEventKind::NodeStarted { .. } => "node_started",
                OrchEventKind::ToolCall { .. } => "tool_call",
                OrchEventKind::NodeFinished { .. } => "node_finished",
                OrchEventKind::Terminal { .. } => "terminal",
                _ => "other",
            }
        }

        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("t-rich").unwrap();
        store.create(&make_task_record("t-rich")).await.unwrap();
        let hub = Arc::new(TaskProgressHub::new());
        let op = OperationId::parse(format!("op-{}", task.as_str())).unwrap();
        let graph = Arc::new(WorkflowGraph {
            id: WorkflowId::parse("w").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
                retry: None,
            }],
            panel: None,
        });
        let executor = WorkflowExecutor::new(Arc::new(RichRegistry));
        let ctx = WorkflowRunContext {
            session_cwd: None,
            make_rich_sink: Some(Arc::new(DetachedRichSinkFactory {
                store: store.clone(),
                task: task.clone(),
                op,
                hub: hub.clone(),
            })),
            ..WorkflowRunContext::default()
        };
        let stream = executor.run_from_with_context(
            graph,
            "input".into(),
            task.as_str().into(),
            CancellationToken::new(),
            std::collections::HashMap::new(),
            ctx,
        );
        let mut sink = DetachedProgressSink::new(store.clone(), task.clone(), hub);

        assert!(drain_workflow(stream, &mut sink).await.unwrap());

        let evs = store.journal_from(&task, -1).await.unwrap();
        let tags: Vec<&str> = evs
            .iter()
            .map(|e| kind_tag(&e.kind))
            .filter(|tag| *tag != "terminal")
            .collect();
        assert_eq!(tags, vec!["node_started", "tool_call", "node_finished"]);
    }
}
pub fn new_detached_task_id() -> TaskId {
    TaskId::parse(a2a::new_task_id()).expect("new_task_id is non-empty")
}

/// Build the optional workflow-history barrier that advances prompt acceptance
/// at the exact provider-poll boundary. A summary write can never cancel the
/// primary workload; failures are projected onto the exact durable locator.
pub fn workflow_prompt_dispatch_barrier(
    history: Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
    task_store: Arc<dyn TaskStore>,
    task: TaskId,
    attempt_id: bridge_core::ids::AttemptId,
    observer: Arc<dyn bridge_core::ports::Observer>,
) -> PromptDispatchBarrier {
    workflow_prompt_dispatch_barrier_with_state(
        history,
        task_store,
        task,
        attempt_id,
        observer,
        AttemptTelemetryState::default(),
    )
}

pub fn workflow_prompt_dispatch_barrier_with_state(
    history: Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>,
    task_store: Arc<dyn TaskStore>,
    task: TaskId,
    attempt_id: bridge_core::ids::AttemptId,
    observer: Arc<dyn bridge_core::ports::Observer>,
    telemetry: AttemptTelemetryState,
) -> PromptDispatchBarrier {
    Arc::new(move || {
        let history = history.clone();
        let task_store = task_store.clone();
        let task = task.clone();
        let attempt_id = attempt_id.clone();
        let observer = observer.clone();
        let telemetry = telemetry.clone();
        Box::pin(async move {
            if let Err(error) = history
                .mark_prompt_acceptance(&attempt_id, "dispatch_uncertain")
                .await
            {
                telemetry.record(error.reason);
                observer.record_workflow(
                    &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable {
                        reason: error.reason,
                    },
                );
                if let Err(marker_error) = task_store
                    .mark_attempt_telemetry_unavailable(&task, &attempt_id, error.reason)
                    .await
                {
                    tracing::warn!(
                        task = task.as_str(),
                        error = ?marker_error,
                        "workflow prompt telemetry marker persistence failed"
                    );
                }
            }
        })
    })
}

impl WorkflowCompletionGuard {
    fn new(
        observer: Arc<dyn bridge_core::ports::Observer>,
        identity: bridge_core::ids::AttemptIdentity,
        workflow: String,
    ) -> Self {
        observer.record_workflow(&bridge_core::ports::WorkflowObsEvent::Started {
            task_class: "workflow",
            surface: "served_task",
        });
        Self {
            observer,
            identity,
            workflow,
            settled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn finish(&self, terminal: &bridge_core::workflow_history::AttemptTerminal) {
        if self
            .settled
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        self.observer
            .record_workflow(&bridge_core::ports::WorkflowObsEvent::Finished {
                attempt_id: &self.identity.attempt_id,
                workflow: &self.workflow,
                task_class: "workflow",
                surface: "served_task",
                policy: "r2f0a",
                outcome: &terminal.outcome,
                telemetry_complete: terminal.telemetry_complete,
                work_seconds: terminal.work_ms as f64 / 1000.0,
                end_to_end_seconds: terminal.end_to_end_ms as f64 / 1000.0,
            });
    }

    fn stop(&self) {
        if self
            .settled
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            self.observer
                .record_workflow(&bridge_core::ports::WorkflowObsEvent::Stopped {
                    task_class: "workflow",
                    surface: "served_task",
                });
        }
    }
}

impl Drop for WorkflowCompletionGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

impl TerminalSummaryBarrier {
    fn attempt_id(&self) -> &bridge_core::ids::AttemptId {
        &self.inner.identity.attempt_id
    }

    async fn prepare_terminal(
        &self,
        status: bridge_core::task_store::TaskRecordStatus,
        cleanup_disposition: &str,
        cleanup_ms: u64,
    ) -> bridge_core::workflow_history::AttemptTerminal {
        use bridge_core::task_store::TaskRecordStatus;
        use bridge_core::workflow_history::{LedgerUnavailableReason as R, NodeCounts};

        let prefinal_ms =
            u64::try_from(self.inner.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let queue_ms = self.inner.queue_ms.min(prefinal_ms);
        let cleanup_ms = cleanup_ms.min(prefinal_ms.saturating_sub(queue_ms));
        let work_ms = prefinal_ms
            .saturating_sub(queue_ms)
            .saturating_sub(cleanup_ms);
        let (node_counts, node_counts_complete) = match self
            .inner
            .task_store
            .node_checkpoints(&self.inner.task)
            .await
        {
            Ok(checkpoints) => (
                NodeCounts {
                    completed: u32::try_from(
                        checkpoints.iter().filter(|(_, _, ok, _)| *ok).count(),
                    )
                    .unwrap_or(u32::MAX),
                    failed: u32::try_from(checkpoints.iter().filter(|(_, _, ok, _)| !*ok).count())
                        .unwrap_or(u32::MAX),
                    ..NodeCounts::default()
                },
                true,
            ),
            Err(error) => {
                tracing::warn!(
                    task = self.inner.task.as_str(),
                    error = ?error,
                    "workflow node-count finalization failed"
                );
                self.inner.telemetry.record(R::Io);
                (NodeCounts::default(), false)
            }
        };
        let outcome = match status {
            TaskRecordStatus::Completed => "completed",
            TaskRecordStatus::Failed => "failed",
            TaskRecordStatus::Canceled => "canceled",
            TaskRecordStatus::Interrupted => "interrupted",
            TaskRecordStatus::Working => "failed",
        };
        let telemetry_complete = self.inner.telemetry.reason().is_none() && node_counts_complete;
        let mut terminal = measured_workflow_terminal(
            self.inner.started,
            work_ms,
            bridge_core::task_store::system_wall_now_ms(),
            outcome,
            node_counts,
            telemetry_complete,
            queue_ms,
            cleanup_ms,
            cleanup_disposition,
        );
        terminal.degraded = status != TaskRecordStatus::Completed || !telemetry_complete;
        terminal.terminal_reason = "owner_finalized".to_owned();
        terminal
    }

    async fn settle_terminal(
        &self,
        terminal: &bridge_core::workflow_history::AttemptTerminal,
    ) -> TerminalBarrierSettlement {
        use bridge_core::workflow_history::{LedgerUnavailableReason as R, TerminalWrite};

        let summary_ready = match &self.inner.history {
            Some(history) => match history
                .terminalize(&self.inner.identity.attempt_id, terminal)
                .await
            {
                Ok(TerminalWrite::Applied) => {
                    self.inner.completion.finish(terminal);
                    true
                }
                Ok(TerminalWrite::Replayed) => {
                    self.inner.completion.stop();
                    true
                }
                Ok(TerminalWrite::Conflict) => {
                    self.inner.telemetry.record(R::Collision);
                    self.inner.completion.stop();
                    false
                }
                Err(error) => {
                    self.inner.telemetry.record(error.reason);
                    self.inner.completion.stop();
                    false
                }
            },
            None => {
                self.inner.telemetry.record(R::Open);
                self.inner.completion.stop();
                false
            }
        };

        let telemetry_unavailable = self.inner.telemetry.reason();
        let marker_ready = if let Some(reason) = telemetry_unavailable {
            self.inner.observer().record_workflow(
                &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
            );
            match self
                .inner
                .task_store
                .mark_attempt_telemetry_unavailable(
                    &self.inner.task,
                    &self.inner.identity.attempt_id,
                    reason,
                )
                .await
            {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(
                        task = self.inner.task.as_str(),
                        error = ?error,
                        "workflow terminal telemetry marker persistence failed"
                    );
                    false
                }
            }
        } else {
            false
        };

        TerminalBarrierSettlement {
            telemetry_unavailable,
            projection_ready: summary_ready || marker_ready,
        }
    }
}

impl TerminalSummaryBarrierInner {
    fn observer(&self) -> &dyn bridge_core::ports::Observer {
        self.completion.observer.as_ref()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn workflow_terminal_summary_barrier(
    history: Option<Arc<dyn bridge_core::workflow_history::WorkflowHistoryStore>>,
    telemetry: AttemptTelemetryState,
    task_store: Arc<dyn TaskStore>,
    task: TaskId,
    identity: bridge_core::ids::AttemptIdentity,
    observer: Arc<dyn bridge_core::ports::Observer>,
    workflow: String,
    started: std::time::Instant,
    queue_ms: u64,
) -> TerminalSummaryBarrier {
    let completion = WorkflowCompletionGuard::new(observer, identity.clone(), workflow);
    TerminalSummaryBarrier {
        inner: Arc::new(TerminalSummaryBarrierInner {
            history,
            telemetry,
            task_store,
            task,
            identity,
            started,
            queue_ms,
            completion,
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn commit_terminal_projection(
    store: &Arc<dyn TaskStore>,
    task: &TaskId,
    operation_id: &OperationId,
    status: bridge_core::task_store::TaskRecordStatus,
    result: Option<&str>,
    error: Option<&str>,
    ts: i64,
    terminal_barrier: Option<&TerminalSummaryBarrier>,
    cleanup_disposition: &str,
    cleanup_ms: u64,
) -> Result<TerminalProjectionCommit, BridgeError> {
    let Some(barrier) = terminal_barrier else {
        let seq = store
            .set_terminal_sequenced(task, operation_id, status, result, error, ts)
            .await?;
        return Ok(TerminalProjectionCommit {
            seq,
            telemetry_unavailable: None,
        });
    };

    let terminal = match store.pending_terminal_projection(task).await? {
        Some(pending) => {
            if pending.attempt_id != *barrier.attempt_id() {
                return Err(BridgeError::StoreFailure);
            }
            pending.terminal
        }
        None => {
            barrier
                .prepare_terminal(status, cleanup_disposition, cleanup_ms)
                .await
        }
    };
    let seq = store
        .set_terminal_sequenced_pending(
            task,
            operation_id,
            status,
            result,
            error,
            ts,
            barrier.attempt_id(),
            &terminal,
        )
        .await?;
    let settlement = barrier.settle_terminal(&terminal).await;
    if !settlement.projection_ready {
        return Err(BridgeError::StoreFailure);
    }
    store
        .mark_terminal_projection_ready(task, barrier.attempt_id())
        .await?;
    Ok(TerminalProjectionCommit {
        seq,
        telemetry_unavailable: settlement.telemetry_unavailable,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn measured_workflow_terminal(
    started: std::time::Instant,
    work_ms: u64,
    completed_ms: i64,
    outcome: &str,
    node_counts: bridge_core::workflow_history::NodeCounts,
    telemetry_complete: bool,
    queue_ms: u64,
    cleanup_ms: u64,
    cleanup_disposition: &str,
) -> bridge_core::workflow_history::AttemptTerminal {
    let end_to_end_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let queue_ms = queue_ms.min(end_to_end_ms);
    let after_queue = end_to_end_ms.saturating_sub(queue_ms);
    let cleanup_ms = cleanup_ms.min(after_queue);
    let work_ms = work_ms.min(after_queue.saturating_sub(cleanup_ms));
    let finalization_ms = after_queue
        .saturating_sub(cleanup_ms)
        .saturating_sub(work_ms);
    bridge_core::workflow_history::AttemptTerminal {
        completed_ms,
        work_ms,
        end_to_end_ms,
        queue_ms,
        cancellation_ms: 0,
        cleanup_ms,
        finalization_ms,
        outcome: outcome.into(),
        terminal_reason: outcome.into(),
        producer_terminal: "unknown".into(),
        final_message: "unknown".into(),
        process_liveness: "unknown".into(),
        terminal_evidence_capability: "unsupported".into(),
        terminal_evidence_version: "none".into(),
        terminal_evidence_source: "none".into(),
        terminal_evidence_complete: false,
        degraded: outcome != "completed",
        prompt_acceptance: "unknown".into(),
        cleanup_disposition: cleanup_disposition.into(),
        node_counts,
        phase_durations: vec![
            bridge_core::workflow_history::PhaseDuration {
                phase: "queue".into(),
                duration_ms: queue_ms,
            },
            bridge_core::workflow_history::PhaseDuration {
                phase: "work".into(),
                duration_ms: work_ms,
            },
            bridge_core::workflow_history::PhaseDuration {
                phase: "cleanup".into(),
                duration_ms: cleanup_ms,
            },
            bridge_core::workflow_history::PhaseDuration {
                phase: "finalization".into(),
                duration_ms: finalization_ms,
            },
        ],
        telemetry_complete,
        monotonic_clock: true,
    }
}

/// Finalize a detached task through the SEQUENCED store path, optionally publishing a
/// `Terminal` frame to the task's progress hub, and ALWAYS removing the hub from
/// `progress_hubs` (so the in-memory hub never leaks). Used by every detached terminal
/// transition EXCEPT the runner's `Ok(true)` happy path (where the `DetachedProgressSink`
/// already wrote+published the sequenced terminal — see `spawn_detached_workflow`).
///
/// Because the terminal is written via `set_terminal_sequenced`, `terminal_seq` is never
/// left NULL on any detached path (the reattach snapshot requirement). On a path where no
/// hub was ever inserted (e.g. the pre-spawn unknown-workflow reject), pass `hub: None`.
pub async fn finalize_detached(
    store: &Arc<dyn bridge_core::task_store::TaskStore>,
    progress_hubs: &Arc<tokio::sync::Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>,
    task: &TaskId,
    status: bridge_core::task_store::TaskRecordStatus,
    result: Option<&str>,
    error: Option<&str>,
    hub: Option<&Arc<TaskProgressHub>>,
) -> Result<(), BridgeError> {
    finalize_detached_with_barrier(
        store,
        progress_hubs,
        task,
        status,
        result,
        error,
        hub,
        None,
        "unknown",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn finalize_detached_with_barrier(
    store: &Arc<dyn bridge_core::task_store::TaskStore>,
    progress_hubs: &Arc<tokio::sync::Mutex<HashMap<TaskId, Arc<TaskProgressHub>>>>,
    task: &TaskId,
    status: bridge_core::task_store::TaskRecordStatus,
    result: Option<&str>,
    error: Option<&str>,
    hub: Option<&Arc<TaskProgressHub>>,
    terminal_barrier: Option<&TerminalSummaryBarrier>,
    cleanup_disposition: &'static str,
) -> Result<(), BridgeError> {
    use bridge_core::task_store::TaskRecordStatus;
    let operation_id = bridge_core::ids::OperationId::parse(format!("op-{}", task.as_str()))?;
    let write = commit_terminal_projection(
        store,
        task,
        &operation_id,
        status,
        result,
        error,
        now_ms(),
        terminal_barrier,
        cleanup_disposition,
        0,
    )
    .await;
    if let (Ok(commit), Some(hub)) = (&write, hub) {
        // Publish the telemetry marker after primary and summary durability, but
        // before the terminal projection. Reattach synthesizes the same marker
        // from the durable locator.
        if let Some(reason) = commit.telemetry_unavailable {
            hub.publish(WorkflowProgressFrame {
                v: 1,
                seq: commit.seq.saturating_sub(1),
                phase: Phase::Live,
                kind: FrameKind::TelemetryUnavailable { reason },
            });
        }
        // Publish the Terminal frame ONLY on a committed seq (durable-first).
        // Interrupted has no WorkflowOutcome analogue; the closest wire terminal is Failed.
        let outcome = match status {
            TaskRecordStatus::Completed => TerminalOutcome::Completed,
            TaskRecordStatus::Canceled => TerminalOutcome::Canceled,
            TaskRecordStatus::Failed
            | TaskRecordStatus::Interrupted
            | TaskRecordStatus::Working => TerminalOutcome::Failed,
        };
        let output = result.or(error).unwrap_or("").to_string();
        hub.publish(WorkflowProgressFrame {
            v: 1,
            seq: commit.seq,
            phase: Phase::Live,
            kind: FrameKind::Terminal { outcome, output },
        });
    }
    // ALWAYS remove the hub, even on write error (I-1: the hub never leaks).
    progress_hubs.lock().await.remove(task);
    write.map(|_| ())
}

/// Spawn the finalizer-guarded background runner for a detached workflow. Returns
/// the JoinHandle so callers/tests can await completion. The caller MUST have
/// already `create`d the Working row and registered the token in `workflow_cancels`.
///
/// The caller supplies the already-resolved `graph` (fresh submit: resolved from
/// `deps.workflows` at submit time; boot resume: deserialized from the stored spec),
/// the `input` string (pre-joined text), the `run_id` (fresh submit: task id; boot
/// resume: `"{task}-resume-{n}"`), and a `seed` of already-completed node outputs
/// (fresh submit: empty; boot resume: checkpoints from the store). With an empty
/// seed, `run_from` is behaviorally identical to `run`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_detached_workflow(
    deps: &DetachedDeps,
    task: TaskId,
    input: String,
    graph: Arc<WorkflowGraph>,
    run_id: String,
    token: CancellationToken,
    seed: HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)>,
    ctx: WorkflowRunContext,
    hub: Arc<TaskProgressHub>,
) -> tokio::task::JoinHandle<bool> {
    spawn_detached_workflow_with_prompt_dispatch(
        deps, task, input, graph, run_id, token, seed, ctx, hub, None,
    )
}

/// Spawn a detached workflow with an optional fail-open prompt telemetry
/// barrier. Kept separate from the compatibility wrapper so downstream callers
/// using the original function remain source compatible.
#[allow(clippy::too_many_arguments)]
pub fn spawn_detached_workflow_with_prompt_dispatch(
    deps: &DetachedDeps,
    task: TaskId,
    input: String,
    graph: Arc<WorkflowGraph>,
    run_id: String,
    token: CancellationToken,
    seed: HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)>,
    ctx: WorkflowRunContext,
    hub: Arc<TaskProgressHub>,
    prompt_dispatch: Option<PromptDispatchBarrier>,
) -> tokio::task::JoinHandle<bool> {
    spawn_detached_workflow_with_attempt_barriers(
        deps,
        task,
        input,
        graph,
        run_id,
        token,
        seed,
        ctx,
        hub,
        prompt_dispatch,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_detached_workflow_with_attempt_barriers(
    deps: &DetachedDeps,
    task: TaskId,
    input: String,
    graph: Arc<WorkflowGraph>,
    run_id: String,
    token: CancellationToken,
    seed: HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)>,
    mut ctx: WorkflowRunContext,
    hub: Arc<TaskProgressHub>,
    prompt_dispatch: Option<PromptDispatchBarrier>,
    terminal_barrier: Option<TerminalSummaryBarrier>,
) -> tokio::task::JoinHandle<bool> {
    let deps = deps.clone();
    tokio::spawn(async move {
        let mut fin = Finalizer {
            store: deps.task_store.clone(),
            task: task.clone(),
            cancels: deps.workflow_cancels.clone(),
            progress_hubs: deps.progress_hubs.clone(),
            hub: hub.clone(),
            terminal_barrier: terminal_barrier.clone(),
            done: false,
        };
        let executor = match &deps.executor {
            Some(e) => e.clone(),
            None => {
                // No executor wired: finalize Failed via the sequenced path (the hub was
                // inserted before spawn, so publish + clean it up).
                let _ = finalize_detached_with_barrier(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("no executor wired"),
                    Some(&hub),
                    terminal_barrier.as_ref(),
                    "not_needed",
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                return false;
            }
        };
        let op = match OperationId::parse(format!("op-{}", task.as_str())) {
            Ok(op) => op,
            Err(_) => {
                let _ = finalize_detached_with_barrier(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("bad operation id"),
                    Some(&hub),
                    terminal_barrier.as_ref(),
                    "not_needed",
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                return false;
            }
        };
        let diagnostic_factory =
            match bridge_core::diagnostics::TaskJournalDiagnosticObserverFactory::new(
                deps.task_store.clone(),
                task.clone(),
                op.clone(),
            )
            .await
            {
                Ok(factory) => factory,
                Err(_) => {
                    let _ = finalize_detached_with_barrier(
                        &deps.task_store,
                        &deps.progress_hubs,
                        &task,
                        bridge_core::task_store::TaskRecordStatus::Failed,
                        None,
                        Some("diagnostic observer factory failed"),
                        Some(&hub),
                        terminal_barrier.as_ref(),
                        "not_needed",
                    )
                    .await;
                    fin.done = true;
                    deps.workflow_cancels.lock().await.remove(&task);
                    return false;
                }
            };
        let diagnostic_factory: Arc<dyn bridge_core::ports::DiagnosticObserverFactory> =
            Arc::new(diagnostic_factory);
        ctx.make_rich_sink = Some(Arc::new(DetachedRichSinkFactory {
            store: deps.task_store.clone(),
            task: task.clone(),
            op,
            hub: hub.clone(),
        }));
        // M4 Slice 3a: the detached runner is the authoritative owner of every workflow
        // turn it emits. Overwrite any caller-supplied (or missing) task_id so detached /
        // batch / resume turn_log rows are always linked to the real task, never NULL or a
        // stale value. This is the central safety boundary; caller-side fixes are defense in
        // depth. Must sit after sink construction and before any turn can be emitted.
        ctx.task_id = Some(task.clone());
        let drain_cancel = token.clone();
        let diagnostic_context = WorkflowDiagnosticContext::new(ctx, diagnostic_factory);
        let diagnostic_context = match prompt_dispatch {
            Some(barrier) => diagnostic_context.with_prompt_dispatch_barrier(barrier),
            None => diagnostic_context,
        };
        let stream = executor.run_from_with_diagnostic_context(
            graph,
            input,
            run_id,
            token,
            seed,
            diagnostic_context,
        );
        // The DetachedProgressSink OWNS the sequenced terminal write: on a clean drain it
        // has already written `set_terminal_sequenced` AND published the Terminal frame.
        let mut sink =
            DetachedProgressSink::new(deps.task_store.clone(), task.clone(), hub.clone())
                .with_terminal_barrier(terminal_barrier.clone());
        let drained = drain_workflow_cancel_on_sink_error(stream, &mut sink, drain_cancel).await;
        let observed_cleanup = sink
            .cleanup_observation
            .map(|(disposition, _)| disposition.as_str())
            .unwrap_or("unknown");
        match drained {
            Ok(true) => {
                // Sink already committed+published the terminal. Do NOT write it again.
                // M1: flip the finalizer done flag BEFORE the hub-removal await so the
                // Finalizer's Drop can never clobber the committed terminal during the
                // .await suspension point below.
                fin.done = true;
                deps.progress_hubs.lock().await.remove(&task);
                deps.workflow_cancels.lock().await.remove(&task);
                true
            }
            Ok(false) => {
                // Drain ended with no terminal: finalize Failed via the sequenced path
                // (also removes the hub).
                let _ = finalize_detached_with_barrier(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("workflow ended without terminal"),
                    Some(&hub),
                    terminal_barrier.as_ref(),
                    observed_cleanup,
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                false
            }
            Err(e) => {
                tracing::warn!(task = task.as_str(), error = ?e, "drain_workflow sink error; marking task Failed");
                let _ = finalize_detached_with_barrier(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("checkpoint write failed"),
                    Some(&hub),
                    terminal_barrier.as_ref(),
                    observed_cleanup,
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                false
            }
        }
    })
}

/// Test-only seam: spawn the runner with a fresh token and an empty seed.
/// Resolves the graph from the deps' `workflows` map (the graph must already be
/// registered). `run_id` is set to the task id (matching the fresh-submit path).
#[doc(hidden)]
pub async fn spawn_detached_workflow_for_test(
    deps: &DetachedDeps,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
) -> tokio::task::JoinHandle<bool> {
    let token = CancellationToken::new();
    let graph = deps
        .workflows
        .get(&wf_id)
        .cloned()
        .expect("workflow must be registered in the test server");
    let input = text_parts.join("\n");
    let run_id = task.as_str().to_string();
    // Mirror the real callers: insert the hub BEFORE spawning.
    let hub = Arc::new(TaskProgressHub::new());
    deps.progress_hubs
        .lock()
        .await
        .insert(task.clone(), hub.clone());
    spawn_detached_workflow(
        deps,
        task,
        input,
        graph,
        run_id,
        token,
        HashMap::new(),
        WorkflowRunContext {
            observer: deps.observer.clone(),
            ..WorkflowRunContext::default()
        },
        hub,
    )
}

/// Test-only seam that takes an explicit token (so a cancel test can fire it).
/// Resolves the graph from the deps' `workflows` map (the graph must already be
/// registered). `run_id` is set to the task id (matching the fresh-submit path).
#[doc(hidden)]
pub async fn spawn_detached_workflow_with_token_for_test(
    deps: &DetachedDeps,
    task: TaskId,
    text_parts: Vec<String>,
    wf_id: bridge_core::ids::WorkflowId,
    token: CancellationToken,
) -> tokio::task::JoinHandle<bool> {
    let graph = deps
        .workflows
        .get(&wf_id)
        .cloned()
        .expect("workflow must be registered in the test server");
    let input = text_parts.join("\n");
    let run_id = task.as_str().to_string();
    // Mirror the real callers: insert the hub BEFORE spawning.
    let hub = Arc::new(TaskProgressHub::new());
    deps.progress_hubs
        .lock()
        .await
        .insert(task.clone(), hub.clone());
    spawn_detached_workflow(
        deps,
        task,
        input,
        graph,
        run_id,
        token,
        HashMap::new(),
        WorkflowRunContext {
            observer: deps.observer.clone(),
            ..WorkflowRunContext::default()
        },
        hub,
    )
}

/// The only snapshot schema version this server can resume. The forward-compat door:
/// a snapshot whose `v` field does not match this const is treated as unreadable and
/// the task is marked `Interrupted` rather than mis-deserialized.
pub const SUPPORTED_SNAPSHOT_VERSION: u32 = 1;

/// The persisted workflow-spec snapshot envelope (mirrors the `{"v":1,"graph":...}`
/// written at detached-submit time — see the `RouteTarget::Workflow` arm of
/// `unary_message`). The `v` field is the forward-compat door: an unknown version
/// fails to match `SUPPORTED_SNAPSHOT_VERSION` in the resume routine and the task is
/// marked `Interrupted` rather than mis-deserialized. `graph` deserializes into the
/// exact `WorkflowGraph` that was running at submit time (NOT the live on-disk spec,
/// which may have changed since).
#[derive(serde::Deserialize)]
pub struct WorkflowSpecEnvelope {
    v: u32,
    graph: bridge_workflow::graph::WorkflowGraph,
}

/// Serialize the persisted workflow-spec snapshot envelope (`{"v":SUPPORTED_SNAPSHOT_VERSION,"graph":…}`).
///
/// This is the SINGLE construction site for that snapshot: BOTH detached-submit surfaces
/// — [`crate::Coordinator::run_workflow`] and the A2A `unary_message` `RouteTarget::Workflow` arm —
/// call this, so the persisted shape can never drift between the two adapters (it round-trips through
/// [`WorkflowSpecEnvelope`] in [`resume_working_tasks`]). The previous A2A path hardcoded `"v": 1`,
/// which would have silently diverged from the Coordinator on a version bump; routing both through
/// this helper closes that gap.
pub fn encode_workflow_spec(graph: &bridge_workflow::graph::WorkflowGraph) -> String {
    serde_json::json!({ "v": SUPPORTED_SNAPSHOT_VERSION, "graph": graph }).to_string()
}

pub fn workflow_spec_node_ids(
    spec_json: &str,
) -> Result<std::collections::BTreeSet<bridge_core::ids::NodeId>, bridge_core::error::BridgeError> {
    let env: WorkflowSpecEnvelope = serde_json::from_str(spec_json)
        .map_err(|_| bridge_core::error::BridgeError::StoreFailure)?;
    if env.v != SUPPORTED_SNAPSHOT_VERSION {
        return Err(bridge_core::error::BridgeError::StoreFailure);
    }
    let mut ids = std::collections::BTreeSet::new();
    for node in env.graph.nodes {
        // A node id that fails the strict grammar means the STORED snapshot is corrupt
        // (not a client error) — classify as StoreFailure, consistent with the JSON/version
        // failures above (and so a route surfaces it as 500, not 400).
        ids.insert(
            bridge_core::ids::NodeId::parse(node.id.as_str())
                .map_err(|_| bridge_core::error::BridgeError::StoreFailure)?,
        );
    }
    Ok(ids)
}

async fn settle_terminal_evidence(
    deps: &DetachedDeps,
    task: &TaskId,
    attempt_id: &bridge_core::ids::AttemptId,
    terminal: &bridge_core::workflow_history::AttemptTerminal,
) -> bool {
    use bridge_core::workflow_history::{LedgerUnavailableReason as R, TerminalWrite};

    let (summary_ready, unavailable) = match &deps.workflow_history {
        Some(Ok(history)) => match history.terminalize(attempt_id, terminal).await {
            Ok(TerminalWrite::Applied) | Ok(TerminalWrite::Replayed) => (true, None),
            Ok(TerminalWrite::Conflict) => (false, Some(R::Collision)),
            Err(error) => (false, Some(error.reason)),
        },
        Some(Err(reason)) => (false, Some(*reason)),
        None => (false, Some(R::Open)),
    };

    let marker_ready = if let Some(reason) = unavailable {
        deps.observer.record_workflow(
            &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
        );
        match deps
            .task_store
            .mark_attempt_telemetry_unavailable(task, attempt_id, reason)
            .await
        {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    task = task.as_str(),
                    error = ?error,
                    "terminal telemetry marker persistence failed"
                );
                false
            }
        }
    } else {
        false
    };

    summary_ready || marker_ready
}

async fn settle_pending_terminal_projection(
    deps: &DetachedDeps,
    pending: &bridge_core::task_store::PendingTerminalProjection,
) -> bool {
    let locator_matches = matches!(
        deps.task_store.get_attempt_locator(&pending.task.id).await,
        Ok(Some(locator))
            if locator.belongs_to(&pending.task.id)
                && locator.identity.attempt_id == pending.attempt_id
    );
    if !locator_matches {
        tracing::warn!(
            task = pending.task.id.as_str(),
            attempt = pending.attempt_id.as_str(),
            "pending terminal projection has no matching exact attempt locator"
        );
        return false;
    }

    if !settle_terminal_evidence(
        deps,
        &pending.task.id,
        &pending.attempt_id,
        &pending.terminal,
    )
    .await
    {
        return false;
    }
    match deps
        .task_store
        .mark_terminal_projection_ready(&pending.task.id, &pending.attempt_id)
        .await
    {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                task = pending.task.id.as_str(),
                error = ?error,
                "pending terminal projection publication failed"
            );
            false
        }
    }
}

/// Reconcile terminal rows whose public projection was withheld by a crash or
/// ambiguous finalization result. This runs before any active-attempt sweep.
pub async fn reconcile_pending_terminal_projections(deps: &DetachedDeps) -> bool {
    let pending = match deps.task_store.pending_terminal_projections().await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = ?error, "pending terminal projection scan failed");
            return false;
        }
    };
    let mut complete = true;
    for row in pending {
        if !settle_pending_terminal_projection(deps, &row).await {
            complete = false;
        }
    }
    complete
}

/// Reconcile durable terminal checkpoints before the boot-wide active-summary
/// interruption sweep. This preserves the existing attempt's actual terminal
/// outcome and never mints a provider attempt for already-finished work.
/// Returns false when any checkpoint-bearing attempt could not be settled, in
/// which case the caller must not run the global active-attempt interruption.
pub async fn reconcile_terminal_checkpoints(deps: &DetachedDeps) -> bool {
    use bridge_core::task_store::TaskRecordStatus;
    use bridge_core::workflow_history::{AttemptTerminal, NodeCounts};

    let working = match deps.task_store.working_tasks().await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(error = ?error, "checkpoint reconciliation: working task scan failed");
            return false;
        }
    };
    let mut safe_to_interrupt = true;
    for record in working {
        let Some(spec_json) = record.workflow_spec_json.as_deref() else {
            continue;
        };
        let graph = match serde_json::from_str::<WorkflowSpecEnvelope>(spec_json) {
            Ok(envelope) if envelope.v == SUPPORTED_SNAPSHOT_VERSION => envelope.graph,
            _ => continue,
        };
        let Some(terminal_node) = graph.terminal() else {
            continue;
        };
        let checkpoints = match deps.task_store.node_checkpoints(&record.id).await {
            Ok(checkpoints) => checkpoints,
            Err(error) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    error = ?error,
                    "checkpoint reconciliation: checkpoint read failed"
                );
                safe_to_interrupt = false;
                continue;
            }
        };
        let Some((_, output, ok, _)) = checkpoints
            .iter()
            .find(|(node, _, _, _)| node.as_str() == terminal_node.id.as_str())
        else {
            continue;
        };
        let status = if *ok {
            TaskRecordStatus::Completed
        } else {
            TaskRecordStatus::Failed
        };
        let locator = match deps.task_store.get_attempt_locator(&record.id).await {
            Ok(Some(locator)) => locator,
            Ok(None) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    "checkpoint reconciliation: missing exact attempt locator"
                );
                safe_to_interrupt = false;
                continue;
            }
            Err(error) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    error = ?error,
                    "checkpoint reconciliation: locator read failed"
                );
                safe_to_interrupt = false;
                continue;
            }
        };
        let completed_ms = record.last_artifact_ms.unwrap_or(record.updated_ms).max(1);
        let outcome = if *ok { "completed" } else { "failed" };
        let terminal = AttemptTerminal {
            completed_ms,
            work_ms: 0,
            end_to_end_ms: 0,
            queue_ms: 0,
            cancellation_ms: 0,
            cleanup_ms: 0,
            finalization_ms: 0,
            outcome: outcome.into(),
            terminal_reason: "terminal_checkpoint_recovered".into(),
            producer_terminal: "unknown".into(),
            final_message: "unknown".into(),
            process_liveness: "unknown".into(),
            terminal_evidence_capability: "unsupported".into(),
            terminal_evidence_version: "none".into(),
            terminal_evidence_source: "none".into(),
            terminal_evidence_complete: false,
            degraded: true,
            prompt_acceptance: "unknown".into(),
            cleanup_disposition: "unknown".into(),
            node_counts: NodeCounts {
                completed: u32::try_from(
                    checkpoints
                        .iter()
                        .filter(|(_, _, success, _)| *success)
                        .count(),
                )
                .unwrap_or(u32::MAX),
                failed: u32::try_from(
                    checkpoints
                        .iter()
                        .filter(|(_, _, success, _)| !*success)
                        .count(),
                )
                .unwrap_or(u32::MAX),
                ..NodeCounts::default()
            },
            phase_durations: Vec::new(),
            telemetry_complete: false,
            monotonic_clock: false,
        };
        let operation_id = match OperationId::parse(format!("op-{}", record.id.as_str())) {
            Ok(operation_id) => operation_id,
            Err(error) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    error = ?error,
                    "checkpoint reconciliation: operation identity failed"
                );
                safe_to_interrupt = false;
                continue;
            }
        };
        let (result, error) = if *ok {
            (Some(output.as_str()), None)
        } else {
            (None, Some(output.as_str()))
        };
        // Recovered terminal evidence is settled first on purpose. If the
        // following primary write fails or its result is ambiguous, the next
        // boot regenerates the same evidence from durable checkpoint times and
        // receives an idempotent summary replay.
        if !settle_terminal_evidence(deps, &record.id, &locator.identity.attempt_id, &terminal)
            .await
        {
            safe_to_interrupt = false;
            continue;
        }
        let primary = deps
            .task_store
            .set_terminal_sequenced_pending(
                &record.id,
                &operation_id,
                status,
                result,
                error,
                completed_ms,
                &locator.identity.attempt_id,
                &terminal,
            )
            .await;
        let pending = match deps
            .task_store
            .pending_terminal_projection(&record.id)
            .await
        {
            Ok(Some(pending))
                if pending.attempt_id == locator.identity.attempt_id
                    && pending.terminal == terminal =>
            {
                pending
            }
            Ok(_) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    write_error = ?primary.err(),
                    "checkpoint reconciliation: primary terminal projection unavailable"
                );
                safe_to_interrupt = false;
                continue;
            }
            Err(error) => {
                tracing::warn!(
                    task = record.id.as_str(),
                    error = ?error,
                    write_error = ?primary.err(),
                    "checkpoint reconciliation: pending terminal read failed"
                );
                safe_to_interrupt = false;
                continue;
            }
        };
        if let Err(error) = deps
            .task_store
            .mark_terminal_projection_ready(&pending.task.id, &pending.attempt_id)
            .await
        {
            tracing::warn!(
                task = pending.task.id.as_str(),
                error = ?error,
                "checkpoint reconciliation: terminal projection publication failed"
            );
            safe_to_interrupt = false;
        }
    }
    safe_to_interrupt
}

/// Boot-time crash-resume scan (W3b Task 10a). Replaces the W3a behavior of sweeping
/// every `Working` row to `Interrupted`: instead, for each `Working` task this either
/// (a) **short-circuits** it to terminal if its terminal node already has a checkpoint
/// (the W3a §8 write-failure gap — the terminal output was produced but the row wasn't
/// flipped), (b) **resumes** it by re-running only the un-checkpointed nodes (seeding
/// `run_from` with the stored checkpoints, consuming one resume attempt), or
/// (c) marks it `Interrupted` if it cannot be resumed (no/unreadable snapshot, unknown
/// schema version, or the resume-attempt cap is exhausted — the poison-pill guard).
///
/// Resilience policy: a top-level `working_tasks()` failure logs and returns (the boot
/// scan is best-effort — a store that can't be read at boot must not abort `serve`).
/// A per-task store error logs and continues to the NEXT task, so one bad row never
/// aborts the whole scan.
///
/// Detachment: a resumed task is spawned via [`spawn_detached_workflow`] (no JoinHandle
/// is awaited here) and runs in the background, exactly like a fresh detached submit.
/// The cancel token is registered in `workflow_cancels` BEFORE the spawn so a
/// concurrent `tasks/cancel` arriving during resume can find and fire it.
pub async fn resume_one_working_task(deps: &DetachedDeps, wt: &TaskRecord, cap: u32) {
    let task = wt.id.clone();

    // (1) No snapshot → cannot reconstruct the graph that was running. Interrupt.
    let Some(spec_json) = wt.workflow_spec_json.as_deref() else {
        // Pre-spawn terminal (no hub inserted): finalize via the sequenced path
        // (hub: None) so terminal_seq is never NULL.
        if let Err(e) = finalize_detached(
            &deps.task_store,
            &deps.progress_hubs,
            &task,
            TaskRecordStatus::Interrupted,
            None,
            Some("not resumable: no workflow snapshot"),
            None,
        )
        .await
        {
            tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/no-snapshot) failed");
        } else {
            tracing::info!(
                task = task.as_str(),
                "resume scan: interrupted (no workflow snapshot)"
            );
        }
        return;
    };

    // (2) Parse the envelope. Unparseable JSON, an unknown `v`, or a `graph` that
    //     won't deserialize into a `WorkflowGraph` all mean "not resumable". The
    //     version check is the forward-compat door (unknown version → Interrupted,
    //     never a panic).
    let graph = match serde_json::from_str::<WorkflowSpecEnvelope>(spec_json) {
        Ok(env) if env.v == SUPPORTED_SNAPSHOT_VERSION => env.graph,
        _ => {
            if let Err(e) = finalize_detached(
                &deps.task_store,
                &deps.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: unreadable workflow snapshot"),
                None,
            )
            .await
            {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/unreadable) failed");
            } else {
                tracing::info!(
                    task = task.as_str(),
                    "resume scan: interrupted (unreadable workflow snapshot)"
                );
            }
            return;
        }
    };

    // (3) Load checkpoints → seed map keyed by node id: node_id → (output, ok, usage).
    let cps = match deps.task_store.node_checkpoints(&task).await {
        Ok(cps) => cps,
        Err(e) => {
            tracing::warn!(task = task.as_str(), error = ?e, "resume scan: node_checkpoints() failed; skipping task");
            return;
        }
    };
    let seed: std::collections::HashMap<
        String,
        (String, bool, Option<bridge_core::orch::UsageSnapshot>),
    > = cps
        .iter()
        .map(|(node, output, ok, usage)| {
            (
                node.as_str().to_string(),
                (output.clone(), *ok, usage.clone()),
            )
        })
        .collect();

    // (4) A terminal checkpoint is owned exclusively by the attempt-first
    //     reconciliation pass. Reaching it here means that pass could not prove
    //     the exact attempt terminal, so neither resume nor primary terminal
    //     publication is safe. Leave the task Working for a later healthy boot.
    let terminal_id = match graph.terminal() {
        Some(n) => n.id.as_str().to_string(),
        None => {
            // A snapshot that validate()'d at submit time always has exactly one
            // terminal; a malformed snapshot with no terminal is not resumable.
            if let Err(e) = finalize_detached(
                &deps.task_store,
                &deps.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: workflow snapshot has no terminal node"),
                None,
            )
            .await
            {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/no-terminal) failed");
            } else {
                tracing::info!(
                    task = task.as_str(),
                    "resume scan: interrupted (unreadable workflow snapshot)"
                );
            }
            return;
        }
    };
    if seed.contains_key(&terminal_id) {
        tracing::warn!(
            task = task.as_str(),
            "resume scan: terminal checkpoint left Working because attempt reconciliation did not complete"
        );
        return;
    }

    // (5) Validate all persisted launch inputs before consuming a resume slot.
    // The primary attempt locator is mandatory for every R2f0a served task: a
    // missing/corrupt lineage is interrupted rather than assigned an identity
    // that can be reused after another crash.
    let ctx = match wt.session_cwd.as_deref() {
        Some(s) => match bridge_core::SessionCwd::parse(s) {
            Ok(c) => bridge_workflow::executor::WorkflowRunContext {
                session_cwd: Some(c),
                task_id: Some(task.clone()),
                make_rich_sink: None,
                observer: deps.observer.clone(),
                ..bridge_workflow::executor::WorkflowRunContext::default()
            },
            Err(_) => {
                let _ = finalize_detached(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("not resumable: unreadable session cwd"),
                    None,
                )
                .await;
                return;
            }
        },
        None => bridge_workflow::executor::WorkflowRunContext {
            task_id: Some(task.clone()),
            observer: deps.observer.clone(),
            ..bridge_workflow::executor::WorkflowRunContext::default()
        },
    };
    let previous = match deps.task_store.get_attempt_locator(&task).await {
        Ok(Some(locator)) => locator,
        _ => {
            let _ = finalize_detached(
                &deps.task_store,
                &deps.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("not resumable: missing durable attempt lineage"),
                None,
            )
            .await;
            return;
        }
    };
    let identity = match previous.identity.resume() {
        Ok(identity) => identity,
        Err(_) => {
            let _ = finalize_detached(
                &deps.task_store,
                &deps.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("resume attempt identity overflow"),
                None,
            )
            .await;
            return;
        }
    };
    let next_locator = bridge_core::task_store::TaskAttemptLocator {
        identity: identity.clone(),
        telemetry_unavailable: None,
    };

    // The resumed attempt clock begins before its primary lineage CAS and all
    // optional ledger work.
    let attempt_started = std::time::Instant::now();
    // Consume the poison-pill slot and advance lineage in one store
    // transaction. Optional telemetry is reserved only after this primary CAS,
    // so a crash can skip an ordinal but can never reuse one.
    match deps
        .task_store
        .claim_resume_attempt_with_locator(
            &task,
            cap,
            deps.clock.now_ms(),
            &previous,
            &next_locator,
        )
        .await
    {
        Ok(ResumeClaim::Exhausted) => {
            // Poison-pill guard: a task that keeps crashing the server is marked
            // Interrupted after `cap` attempts instead of looping forever.
            if let Err(e) = finalize_detached(
                &deps.task_store,
                &deps.progress_hubs,
                &task,
                TaskRecordStatus::Interrupted,
                None,
                Some("resume attempt cap exceeded"),
                None,
            )
            .await
            {
                tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(Interrupted/cap) failed");
            } else {
                tracing::info!(
                    task = task.as_str(),
                    "resume scan: interrupted (resume attempt cap exceeded)"
                );
            }
        }
        Ok(ResumeClaim::Resumable { attempt }) => {
            let mut summary_history = None;
            let (workload_fingerprint, workload_fingerprint_complete) = deps
                .executor
                .as_ref()
                .map(|executor| executor.workload_fingerprint(&graph))
                .unwrap_or_else(|| {
                    bridge_workflow::graph::workload_fingerprint_with(&graph, |_| None)
                });
            let reservation = bridge_core::workflow_history::AttemptReservation {
                identity: identity.clone(),
                task_id: Some(task.clone()),
                workflow: wt.workflow.clone(),
                task_class: "workflow".into(),
                surface: bridge_core::workflow_history::ExecutionSurface::ServedTask,
                policy: "r2f0a".into(),
                workload_fingerprint,
                started_ms: deps.clock.now_ms(),
                workload_fingerprint_complete,
                prompt_acceptance: "not_dispatched".into(),
                pinned: false,
            };
            let telemetry_unavailable = match &deps.workflow_history {
                Some(Ok(history)) => match history.reserve(&reservation).await {
                    Ok(()) => {
                        summary_history = Some(history.clone());
                        None
                    }
                    Err(error) => Some(error.reason),
                },
                Some(Err(reason)) => Some(*reason),
                None => Some(bridge_core::workflow_history::LedgerUnavailableReason::Open),
            };
            let attempt_telemetry = AttemptTelemetryState::default();
            if let Some(reason) = telemetry_unavailable {
                attempt_telemetry.record(reason);
                deps.observer.record_workflow(
                    &bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { reason },
                );
                if let Err(error) = deps
                    .task_store
                    .mark_attempt_telemetry_unavailable(&task, &identity.attempt_id, reason)
                    .await
                {
                    tracing::warn!(task = task.as_str(), error = ?error, "resume scan: telemetry marker persistence failed");
                }
            }
            let prompt_dispatch = summary_history.as_ref().map(|history| {
                workflow_prompt_dispatch_barrier_with_state(
                    history.clone(),
                    deps.task_store.clone(),
                    task.clone(),
                    identity.attempt_id.clone(),
                    deps.observer.clone(),
                    attempt_telemetry.clone(),
                )
            });
            let terminal_barrier = workflow_terminal_summary_barrier(
                summary_history,
                attempt_telemetry,
                deps.task_store.clone(),
                task.clone(),
                identity.clone(),
                deps.observer.clone(),
                wt.workflow.clone(),
                attempt_started,
                0,
            );
            if telemetry_unavailable
                == Some(bridge_core::workflow_history::LedgerUnavailableReason::Collision)
            {
                let _ = finalize_detached_with_barrier(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    TaskRecordStatus::Interrupted,
                    None,
                    Some("attempt identity collision"),
                    None,
                    Some(&terminal_barrier),
                    "not_needed",
                )
                .await;
                return;
            }

            // Insert the progress hub BEFORE spawning (mirrors the fresh-submit
            // path) so a reattach subscriber can find it.
            let hub = Arc::new(TaskProgressHub::new());
            deps.progress_hubs
                .lock()
                .await
                .insert(task.clone(), hub.clone());
            // Register a fresh cancel token BEFORE spawning so a concurrent
            // tasks/cancel during resume can find and fire it.
            let token = tokio_util::sync::CancellationToken::new();
            deps.workflow_cancels
                .lock()
                .await
                .insert(task.clone(), token.clone());
            let run_id = identity.run_id().to_string();
            let _handle = spawn_detached_workflow_with_attempt_barriers(
                deps,
                task.clone(),
                wt.input.clone(),
                Arc::new(graph),
                run_id.clone(),
                token,
                seed,
                ctx,
                hub,
                prompt_dispatch,
                Some(terminal_barrier),
            );
            tracing::info!(task = task.as_str(), attempt, run_id = %run_id, "resume scan: resumed from checkpoints");
        }
        Err(e) => {
            tracing::warn!(task = task.as_str(), error = ?e, "resume scan: atomic resume claim failed; skipping task");
        }
    }
}

pub async fn resume_non_batch_tasks(deps: &DetachedDeps, cap: u32) {
    let working = match deps.task_store.working_tasks().await {
        Ok(w) => w,
        Err(e) => {
            // A store that can't even be scanned at boot is logged and the scan is
            // skipped — `serve` still comes up (best-effort resume).
            tracing::warn!(error = ?e, "resume scan: working_tasks() failed; skipping boot resume");
            return;
        }
    };

    for wt in working {
        if wt.batch_id.is_some() {
            continue;
        }
        resume_one_working_task(deps, &wt, cap).await;
    }
}

pub async fn resume_working_tasks(deps: &DetachedDeps, cap: u32) {
    resume_non_batch_tasks(deps, cap).await;
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use crate::clock::ManualClock;
    use bridge_core::diagnostics::{
        DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
        PersistedPhaseTransitionInput, PhaseStatus,
    };
    use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
    use bridge_core::ids::{AgentId, NodeId, OperationId, SessionId, WorkflowId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::{
        AgentBackend, AgentRegistry, BackendObservers, BackendStream, DiagnosticObserver, Lease,
        ObsEvent, Observer, Resolved, Update,
    };
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use bridge_observ::{DropCounter, TurnLogObserver};
    use bridge_workflow::executor::WorkflowExecutor;
    use bridge_workflow::graph::{PanelConfig, RetryPolicy, WorkflowGraph, WorkflowNode};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex as StdMutex};

    struct NoopLease;
    impl Lease for NoopLease {}

    #[derive(Clone)]
    struct NoopObserver;
    impl Observer for NoopObserver {
        fn record(&self, _: &ObsEvent<'_>) {}
    }

    fn turn_log_observer(store: Arc<dyn TaskStore>) -> Arc<TurnLogObserver> {
        Arc::new(TurnLogObserver::new(
            store,
            DropCounter::disabled(),
            64,
            Arc::new(|| 1_000),
        ))
    }

    fn make_task_record(id: &str) -> TaskRecord {
        TaskRecord {
            id: TaskId::parse(id).unwrap(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            last_artifact_ms: None,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
            artifacts_purged_at: None,
        }
    }

    async fn create_resumable_task(store: &Arc<dyn TaskStore>, mut record: TaskRecord) -> TaskId {
        let identity = bridge_core::ids::AttemptIdentity::initial().unwrap();
        let task = TaskId::parse(identity.execution_id.as_str().to_owned()).unwrap();
        record.id = task.clone();
        store.create(&record).await.unwrap();
        store
            .put_attempt_locator(
                &task,
                &bridge_core::task_store::TaskAttemptLocator {
                    identity,
                    telemetry_unavailable: None,
                },
            )
            .await
            .unwrap();
        task
    }

    fn diagnostic_event(phase: DiagnosticPhase, status: PhaseStatus) -> DiagnosticEvent {
        DiagnosticEvent::new(
            PersistedPhaseTransition::build(
                PersistedPhaseTransitionInput {
                    phase,
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

    async fn wait_turn_rows_for_task(
        store: &Arc<dyn TaskStore>,
        task: &TaskId,
    ) -> Vec<bridge_core::task_store::TurnLogRow> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let rows = store.turn_log_rows_for_task(task, 16).await.unwrap();
            if !rows.is_empty() {
                return rows;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for turn_log rows for {}",
                task.as_str()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[derive(Default)]
    struct PromptRec {
        prompts: StdMutex<Vec<String>>,
    }

    struct RecordingBackend {
        rec: Arc<PromptRec>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for RecordingBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.rec
                .prompts
                .lock()
                .unwrap()
                .push(parts.iter().map(|p| p.text.clone()).collect());
            Ok(Box::pin(tokio_stream::iter(vec![
                Ok(Update::Text("FINAL".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ])))
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    struct RecordingRegistry {
        synth: Arc<PromptRec>,
    }

    #[async_trait::async_trait]
    impl AgentRegistry for RecordingRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            if id.as_str() != "synth" {
                return Err(BridgeError::UnknownAgent {
                    id: id.as_str().into(),
                });
            }
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
                    host_fallback_eligible: false,
                    name: None,
                    description: None,
                    tags: vec![],
                    version: None,
                    mcp: vec![],
                    mcp_delivery: Default::default(),
                    extensions: Default::default(),
                }),
                backend: Arc::new(RecordingBackend {
                    rec: self.synth.clone(),
                }),
                lease: Box::new(NoopLease),
            })
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            observer
                .record(diagnostic_event(
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Started,
                ))
                .await?;
            let resolved = self.resolve(id).await?;
            observer
                .record(diagnostic_event(
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Completed,
                ))
                .await?;
            Ok(resolved)
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("synth").unwrap()
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            vec![AgentId::parse("synth").unwrap()]
        }
    }

    pub(super) struct SelectiveFailureStore {
        pub(super) inner: MemoryTaskStore,
        pub(super) fail_checkpoint: bool,
        pub(super) fail_diagnostic: bool,
        pub(super) fail_primary_terminal: bool,
        pub(super) fail_marker: bool,
        pub(super) fail_projection_ready: bool,
    }

    #[async_trait::async_trait]
    impl TaskStore for SelectiveFailureStore {
        async fn create(&self, rec: &TaskRecord) -> Result<(), BridgeError> {
            self.inner.create(rec).await
        }

        async fn create_with_attempt_locator(
            &self,
            rec: &TaskRecord,
            locator: &bridge_core::task_store::TaskAttemptLocator,
        ) -> Result<(), BridgeError> {
            self.inner.create_with_attempt_locator(rec, locator).await
        }

        async fn set_terminal(
            &self,
            id: &TaskId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            updated_ms: i64,
        ) -> Result<(), BridgeError> {
            self.inner
                .set_terminal(id, status, result, error, updated_ms)
                .await
        }

        async fn get(&self, id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
            self.inner.get(id).await
        }

        async fn mark_attempt_telemetry_unavailable(
            &self,
            task: &TaskId,
            attempt: &bridge_core::ids::AttemptId,
            reason: bridge_core::workflow_history::LedgerUnavailableReason,
        ) -> Result<(), BridgeError> {
            if self.fail_marker {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .mark_attempt_telemetry_unavailable(task, attempt, reason)
                .await
        }

        async fn get_attempt_locator(
            &self,
            task: &TaskId,
        ) -> Result<Option<bridge_core::task_store::TaskAttemptLocator>, BridgeError> {
            self.inner.get_attempt_locator(task).await
        }

        async fn list(&self, limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
            self.inner.list(limit).await
        }

        async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
            self.inner.sweep_interrupted(updated_ms).await
        }

        async fn cancel_if_working(
            &self,
            id: &TaskId,
            updated_ms: i64,
        ) -> Result<bool, BridgeError> {
            self.inner.cancel_if_working(id, updated_ms).await
        }

        async fn put_node_checkpoint(
            &self,
            task: &TaskId,
            node: &NodeId,
            output: &str,
            ok: bool,
            ts: i64,
        ) -> Result<(), BridgeError> {
            self.inner
                .put_node_checkpoint(task, node, output, ok, ts)
                .await
        }

        async fn node_checkpoints(
            &self,
            task: &TaskId,
        ) -> Result<Vec<(NodeId, String, bool, Option<UsageSnapshot>)>, BridgeError> {
            self.inner.node_checkpoints(task).await
        }

        async fn claim_resume_attempt(
            &self,
            task: &TaskId,
            cap: u32,
            now_ms: i64,
        ) -> Result<bridge_core::task_store::ResumeClaim, BridgeError> {
            self.inner.claim_resume_attempt(task, cap, now_ms).await
        }

        async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
            self.inner.working_tasks().await
        }

        async fn record_node_started(
            &self,
            task: &TaskId,
            node: &NodeId,
            operation_id: &OperationId,
            ts: i64,
        ) -> Result<i64, BridgeError> {
            self.inner
                .record_node_started(task, node, operation_id, ts)
                .await
        }

        async fn put_node_checkpoint_sequenced(
            &self,
            task: &TaskId,
            node: &NodeId,
            operation_id: &OperationId,
            output: &str,
            ok: bool,
            ts: i64,
            usage: Option<&UsageSnapshot>,
        ) -> Result<i64, BridgeError> {
            if self.fail_checkpoint {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .put_node_checkpoint_sequenced(task, node, operation_id, output, ok, ts, usage)
                .await
        }

        async fn set_terminal_sequenced(
            &self,
            task: &TaskId,
            operation_id: &OperationId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            ts: i64,
        ) -> Result<i64, BridgeError> {
            self.inner
                .set_terminal_sequenced(task, operation_id, status, result, error, ts)
                .await
        }

        async fn set_terminal_sequenced_pending(
            &self,
            task: &TaskId,
            operation_id: &OperationId,
            status: TaskRecordStatus,
            result: Option<&str>,
            error: Option<&str>,
            ts: i64,
            attempt_id: &bridge_core::ids::AttemptId,
            terminal: &bridge_core::workflow_history::AttemptTerminal,
        ) -> Result<i64, BridgeError> {
            if self.fail_primary_terminal {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .set_terminal_sequenced_pending(
                    task,
                    operation_id,
                    status,
                    result,
                    error,
                    ts,
                    attempt_id,
                    terminal,
                )
                .await
        }

        async fn pending_terminal_projection(
            &self,
            task: &TaskId,
        ) -> Result<Option<bridge_core::task_store::PendingTerminalProjection>, BridgeError>
        {
            self.inner.pending_terminal_projection(task).await
        }

        async fn mark_terminal_projection_ready(
            &self,
            task: &TaskId,
            attempt_id: &bridge_core::ids::AttemptId,
        ) -> Result<(), BridgeError> {
            if self.fail_projection_ready {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .mark_terminal_projection_ready(task, attempt_id)
                .await
        }

        async fn record_event_sequenced(
            &self,
            task: &TaskId,
            operation_id: &OperationId,
            ts: i64,
            kind: bridge_core::orch::OrchEventKind,
        ) -> Result<i64, BridgeError> {
            if self.fail_diagnostic
                && matches!(
                    &kind,
                    bridge_core::orch::OrchEventKind::Progress { progress }
                        if progress.diagnostic_event().is_some()
                )
            {
                return Err(BridgeError::StoreFailure);
            }
            self.inner
                .record_event_sequenced(task, operation_id, ts, kind)
                .await
        }

        async fn journal_from(
            &self,
            task: &TaskId,
            after_seq: i64,
        ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
            self.inner.journal_from(task, after_seq).await
        }

        async fn progress_snapshot(
            &self,
            task: &TaskId,
        ) -> Result<bridge_core::task_store::TaskProgressSnapshot, BridgeError> {
            self.inner.progress_snapshot(task).await
        }
    }

    fn panel_graph() -> Arc<WorkflowGraph> {
        let node = |id: &str, agent: &str, inputs: &[&str], prompt: &str| WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(agent).unwrap(),
            prompt_template: prompt.into(),
            inputs: inputs
                .iter()
                .map(|input| NodeId::parse(*input).unwrap())
                .collect(),
            retry: None,
        };
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("panel").unwrap(),
            nodes: vec![
                node("codex", "codex", &[], "review {{input}}"),
                node("claude", "claude", &[], "review {{input}}"),
                node(
                    "synth",
                    "synth",
                    &["codex", "claude"],
                    "merge {{codex}} + {{claude}}\n{{workflow.costs}}",
                ),
            ],
            panel: Some(PanelConfig {
                weights: BTreeMap::from([("usage".into(), 0.2), ("benefit".into(), 0.8)]),
            }),
        })
    }

    fn single_retry_graph() -> Arc<WorkflowGraph> {
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("retry-resume").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("synth").unwrap(),
                prompt_template: "resume {{input}}".into(),
                inputs: vec![],
                retry: Some(RetryPolicy {
                    max_attempts: 5,
                    backoff_ms: 60_000,
                    backoff_cap_ms: None,
                }),
            }],
            panel: None,
        })
    }

    fn detached_deps_with_observer(
        store: Arc<dyn TaskStore>,
        observer: Arc<dyn Observer>,
        graph: Arc<WorkflowGraph>,
    ) -> DetachedDeps {
        let synth = Arc::new(PromptRec::default());
        DetachedDeps {
            task_store: store,
            executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(
                RecordingRegistry { synth },
            )))),
            workflows: Arc::new(HashMap::from([(graph.id.clone(), graph)])),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer,
            workflow_history: None,
        }
    }

    #[derive(Clone, Copy)]
    enum RichRaceRole {
        CheckpointOwner,
        PendingRichSibling,
        Synth,
    }

    struct RichRaceBackend {
        role: RichRaceRole,
        sibling_recorded: Arc<tokio::sync::Notify>,
        cancels: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl AgentBackend for RichRaceBackend {
        async fn prompt(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("detached rich ownership must use prompt_with_observers")
        }

        async fn prompt_with_observers(
            &self,
            _session: &SessionId,
            _parts: Vec<Part>,
            observers: BackendObservers,
        ) -> Result<BackendStream, BridgeError> {
            match self.role {
                RichRaceRole::CheckpointOwner => {
                    self.sibling_recorded.notified().await;
                    Ok(Box::pin(tokio_stream::iter(vec![Ok(Update::Done {
                        stop_reason: "end_turn".into(),
                    })])))
                }
                RichRaceRole::PendingRichSibling => {
                    observers
                        .rich
                        .expect("detached owner supplies a rich sink")
                        .record(bridge_core::orch::OrchEventKind::Plan { entries: vec![] });
                    self.sibling_recorded.notify_one();
                    Ok(Box::pin(futures::stream::pending()))
                }
                RichRaceRole::Synth => panic!("cancellation must prevent synth scheduling"),
            }
        }

        async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
            self.cancels
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    struct RichRaceRegistry {
        sibling_recorded: Arc<tokio::sync::Notify>,
        sibling_cancels: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RichRaceRegistry {
        fn entry(id: &AgentId) -> AgentEntry {
            AgentEntry {
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
                host_fallback_eligible: false,
                name: None,
                description: None,
                tags: vec![],
                version: None,
                mcp: vec![],
                mcp_delivery: Default::default(),
                extensions: Default::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentRegistry for RichRaceRegistry {
        async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
            let (role, cancels) = match id.as_str() {
                "checkpoint" => (
                    RichRaceRole::CheckpointOwner,
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ),
                "pending" => (
                    RichRaceRole::PendingRichSibling,
                    self.sibling_cancels.clone(),
                ),
                "synth" => (
                    RichRaceRole::Synth,
                    Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                ),
                other => return Err(BridgeError::UnknownAgent { id: other.into() }),
            };
            Ok(Resolved {
                entry: Arc::new(Self::entry(id)),
                backend: Arc::new(RichRaceBackend {
                    role,
                    sibling_recorded: self.sibling_recorded.clone(),
                    cancels,
                }),
                lease: Box::new(NoopLease),
            })
        }

        async fn resolve_observed(
            &self,
            id: &AgentId,
            observer: Arc<dyn DiagnosticObserver>,
        ) -> Result<Resolved, BridgeError> {
            observer
                .record(diagnostic_event(
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Started,
                ))
                .await?;
            let resolved = self.resolve(id).await?;
            observer
                .record(diagnostic_event(
                    DiagnosticPhase::Resolve,
                    PhaseStatus::Completed,
                ))
                .await?;
            Ok(resolved)
        }

        fn default_id(&self) -> AgentId {
            AgentId::parse("checkpoint").unwrap()
        }

        async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
            Ok(())
        }

        fn list(&self) -> Vec<AgentId> {
            ["checkpoint", "pending", "synth"]
                .into_iter()
                .map(|id| AgentId::parse(id).unwrap())
                .collect()
        }
    }

    fn rich_race_graph() -> Arc<WorkflowGraph> {
        let node = |id: &str, agent: &str, inputs: &[&str]| WorkflowNode {
            id: NodeId::parse(id).unwrap(),
            agent: AgentId::parse(agent).unwrap(),
            prompt_template: "{{input}}".into(),
            inputs: inputs
                .iter()
                .map(|input| NodeId::parse(*input).unwrap())
                .collect(),
            retry: None,
        };
        Arc::new(WorkflowGraph {
            id: WorkflowId::parse("rich-race").unwrap(),
            nodes: vec![
                node("checkpoint", "checkpoint", &[]),
                node("pending", "pending", &[]),
                node("synth", "synth", &["checkpoint", "pending"]),
            ],
            panel: None,
        })
    }

    #[tokio::test]
    async fn detached_checkpoint_failure_flushes_inflight_sibling_before_terminal_failure() {
        let store = Arc::new(SelectiveFailureStore {
            inner: MemoryTaskStore::new(),
            fail_checkpoint: true,
            fail_diagnostic: false,
            fail_primary_terminal: false,
            fail_marker: false,
            fail_projection_ready: false,
        });
        let task = TaskId::parse("t-detached-checkpoint-rich-race").unwrap();
        store
            .create(&make_task_record(task.as_str()))
            .await
            .unwrap();
        let graph = rich_race_graph();
        let sibling_recorded = Arc::new(tokio::sync::Notify::new());
        let sibling_cancels = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(
                RichRaceRegistry {
                    sibling_recorded,
                    sibling_cancels: sibling_cancels.clone(),
                },
            )))),
            workflows: Arc::new(HashMap::from([(graph.id.clone(), graph.clone())])),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer: Arc::new(NoopObserver),
            workflow_history: None,
        };
        let hub = Arc::new(TaskProgressHub::new());
        deps.progress_hubs
            .lock()
            .await
            .insert(task.clone(), hub.clone());

        let handle = spawn_detached_workflow(
            &deps,
            task.clone(),
            "input".into(),
            graph,
            task.as_str().into(),
            CancellationToken::new(),
            HashMap::new(),
            WorkflowRunContext::default(),
            hub,
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("detached failure must cancel and drain the pending sibling")
            .unwrap();

        assert_eq!(sibling_cancels.load(std::sync::atomic::Ordering::SeqCst), 1);
        let journal = store.journal_from(&task, -1).await.unwrap();
        assert_eq!(
            journal
                .iter()
                .filter(|event| {
                    matches!(&event.kind, bridge_core::orch::OrchEventKind::Plan { .. })
                })
                .count(),
            1,
            "the pending sibling's accepted rich event must flush before failure returns"
        );
        assert_eq!(
            store.get(&task).await.unwrap().unwrap().status,
            TaskRecordStatus::Failed
        );
    }

    #[tokio::test]
    async fn spawn_detached_workflow_overwrites_ctx_task_id() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let turnlog = turn_log_observer(store.clone());
        let graph = single_retry_graph();
        let deps = detached_deps_with_observer(store.clone(), turnlog.clone(), graph.clone());

        let cases = vec![
            ("t-overwrite-missing", None),
            (
                "t-overwrite-conflict",
                Some(TaskId::parse("t-wrong-owner").unwrap()),
            ),
        ];
        for (task_raw, supplied_task_id) in cases {
            let task = TaskId::parse(task_raw).unwrap();
            store.create(&make_task_record(task_raw)).await.unwrap();
            let hub = Arc::new(TaskProgressHub::new());
            deps.progress_hubs
                .lock()
                .await
                .insert(task.clone(), hub.clone());
            let handle = spawn_detached_workflow(
                &deps,
                task.clone(),
                "DIFF".into(),
                graph.clone(),
                task.as_str().to_string(),
                CancellationToken::new(),
                HashMap::new(),
                WorkflowRunContext {
                    task_id: supplied_task_id,
                    observer: turnlog.clone(),
                    ..WorkflowRunContext::default()
                },
                hub,
            );
            handle.await.unwrap();
            turnlog.flush().await;

            let rows = wait_turn_rows_for_task(&store, &task).await;
            assert!(
                rows.iter()
                    .all(|row| row.task_id.as_ref().map(|id| id.as_str()) == Some(task.as_str())),
                "detached runner must persist authoritative task_id for {task_raw}: {rows:?}"
            );
        }
    }

    #[tokio::test]
    async fn detached_fresh_turn_persists_task_id() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let turnlog = turn_log_observer(store.clone());
        let graph = single_retry_graph();
        let deps = detached_deps_with_observer(store.clone(), turnlog.clone(), graph.clone());
        let task = TaskId::parse("t-detached-fresh-owner").unwrap();
        store
            .create(&make_task_record(task.as_str()))
            .await
            .unwrap();

        let handle = spawn_detached_workflow_for_test(
            &deps,
            task.clone(),
            vec!["DIFF".into()],
            graph.id.clone(),
        )
        .await;
        handle.await.unwrap();
        turnlog.flush().await;

        let rows = wait_turn_rows_for_task(&store, &task).await;
        assert!(rows.iter().all(|row| row.task_id == Some(task.clone())));

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
    async fn detached_diagnostic_write_failure_is_fatal_before_backend_prompt() {
        let store: Arc<dyn TaskStore> = Arc::new(SelectiveFailureStore {
            inner: MemoryTaskStore::new(),
            fail_checkpoint: false,
            fail_diagnostic: true,
            fail_primary_terminal: false,
            fail_marker: false,
            fail_projection_ready: false,
        });
        let task = TaskId::parse("t-detached-diagnostic-write-fails").unwrap();
        store
            .create(&make_task_record(task.as_str()))
            .await
            .unwrap();
        let graph = single_retry_graph();
        let prompts = Arc::new(PromptRec::default());
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(
                RecordingRegistry {
                    synth: prompts.clone(),
                },
            )))),
            workflows: Arc::new(HashMap::from([(graph.id.clone(), graph.clone())])),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer: Arc::new(NoopObserver),
            workflow_history: None,
        };

        let handle = spawn_detached_workflow_for_test(
            &deps,
            task.clone(),
            vec!["DIFF".into()],
            graph.id.clone(),
        )
        .await;
        handle.await.unwrap();

        assert!(
            prompts.prompts.lock().unwrap().is_empty(),
            "a durable diagnostic failure during resolution must stop before prompt"
        );
        let record = store.get(&task).await.unwrap().unwrap();
        assert_eq!(record.status, TaskRecordStatus::Failed);
        let journal = store.journal_from(&task, -1).await.unwrap();
        assert!(journal.iter().all(|event| {
            !matches!(
                &event.kind,
                bridge_core::orch::OrchEventKind::Progress { progress }
                    if progress.diagnostic_event().is_some()
            )
        }));
        assert!(
            journal.iter().any(|event| matches!(
                event.kind,
                bridge_core::orch::OrchEventKind::Terminal { .. }
            )),
            "the failed task still receives one durable terminal"
        );
    }

    #[tokio::test]
    async fn detached_missing_task_row_never_constructs_journal_authority_or_prompts() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("t-detached-missing-row").unwrap();
        let graph = single_retry_graph();
        let prompts = Arc::new(PromptRec::default());
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: Some(Arc::new(WorkflowExecutor::new(Arc::new(
                RecordingRegistry {
                    synth: prompts.clone(),
                },
            )))),
            workflows: Arc::new(HashMap::from([(graph.id.clone(), graph.clone())])),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer: Arc::new(NoopObserver),
            workflow_history: None,
        };

        let handle = spawn_detached_workflow_for_test(
            &deps,
            task.clone(),
            vec!["DIFF".into()],
            graph.id.clone(),
        )
        .await;
        handle.await.unwrap();

        assert!(prompts.prompts.lock().unwrap().is_empty());
        assert!(store.get(&task).await.unwrap().is_none());
        assert!(deps.progress_hubs.lock().await.is_empty());
    }

    #[tokio::test]
    async fn detached_resume_turn_persists_task_id() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let turnlog = turn_log_observer(store.clone());
        let graph = single_retry_graph();
        let deps = detached_deps_with_observer(store.clone(), turnlog.clone(), graph.clone());
        let task = create_resumable_task(
            &store,
            TaskRecord {
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                ..make_task_record("placeholder")
            },
        )
        .await;

        resume_working_tasks(&deps, 1).await;

        let rows = wait_turn_rows_for_task(&store, &task).await;
        assert!(rows.iter().all(|row| row.task_id == Some(task.clone())));
    }

    #[tokio::test]
    async fn resume_working_task_synth_sees_checkpointed_member_usage() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let graph = panel_graph();
        let task = create_resumable_task(
            &store,
            TaskRecord {
                workflow: "panel".into(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                ..make_task_record("placeholder")
            },
        )
        .await;

        let op = OperationId::parse(format!("op-{}", task.as_str())).unwrap();
        let codex = NodeId::parse("codex").unwrap();
        let claude = NodeId::parse("claude").unwrap();
        let codex_usage = UsageSnapshot {
            used: Some(15071),
            size: Some(258400),
            cost: None,
            terminal: None,
            at_ms: 10,
        };
        let claude_usage = UsageSnapshot {
            used: Some(42),
            size: Some(100),
            cost: None,
            terminal: None,
            at_ms: 11,
        };
        store
            .put_node_checkpoint_sequenced(
                &task,
                &codex,
                &op,
                "CODEX_REVIEW",
                true,
                2,
                Some(&codex_usage),
            )
            .await
            .unwrap();
        store
            .put_node_checkpoint_sequenced(
                &task,
                &claude,
                &op,
                "CLAUDE_REVIEW",
                true,
                3,
                Some(&claude_usage),
            )
            .await
            .unwrap();

        let synth = Arc::new(PromptRec::default());
        let executor = Arc::new(WorkflowExecutor::new(Arc::new(RecordingRegistry {
            synth: synth.clone(),
        })));
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: Some(executor),
            workflows: Arc::new(HashMap::new()),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer: Arc::new(NoopObserver),
            workflow_history: None,
        };

        resume_working_tasks(&deps, 1).await;

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if !synth.prompts.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("resumed synth prompt was recorded");

        let prompts = synth.prompts.lock().unwrap();
        let prompt = &prompts[0];
        assert!(
            prompt.contains("| codex | 15071 | 258400 |"),
            "resumed synth costs table includes codex usage: {prompt}"
        );
        assert!(
            prompt.contains("| claude | 42 | 100 |"),
            "resumed synth costs table includes claude usage: {prompt}"
        );
    }

    #[tokio::test]
    async fn working_task_without_checkpoint_reruns_on_resume() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let graph = single_retry_graph();
        let task = create_resumable_task(
            &store,
            TaskRecord {
                workflow: "retry-resume".into(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                ..make_task_record("placeholder")
            },
        )
        .await;

        let synth = Arc::new(PromptRec::default());
        let executor = Arc::new(WorkflowExecutor::new(Arc::new(RecordingRegistry {
            synth: synth.clone(),
        })));
        let deps = DetachedDeps {
            task_store: store.clone(),
            executor: Some(executor),
            workflows: Arc::new(HashMap::new()),
            workflow_cancels: Arc::new(Mutex::new(HashMap::new())),
            progress_hubs: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(ManualClock::new(100)),
            observer: Arc::new(NoopObserver),
            workflow_history: None,
        };

        resume_working_tasks(&deps, 1).await;

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let cps = store.node_checkpoints(&task).await.unwrap();
                if !cps.is_empty() {
                    return cps;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("resume should re-run and checkpoint the uncheckpointed retry node");

        // Clone the prompts out of the std MutexGuard in a scope so the guard is dropped before any
        // await below (clippy await_holding_lock doesn't honor an explicit drop()).
        let prompts: Vec<String> = { synth.prompts.lock().unwrap().clone() };
        assert_eq!(
            prompts.len(),
            1,
            "resume should invoke the node prompt once"
        );
        assert!(
            prompts[0].contains("resume DIFF"),
            "resumed prompt should use the original task input: {}",
            prompts[0]
        );

        let checkpoints = store.node_checkpoints(&task).await.unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].0.as_str(), "only");
        assert_eq!(checkpoints[0].1, "FINAL");
        assert!(checkpoints[0].2);
    }
}

#[cfg(test)]
mod frame_tests {
    // Ported with the move (s8 T3 controller fix): these 3 frame/hub tests were dropped
    // when the reattach DTOs moved here from bridge-a2a-inbound; they guard the LOCKED SSE
    // wire contract (the top-level `kind` discriminator + `tool_kind`/`content_preview` rename).
    use super::*;
    use bridge_core::orch::{ContentSummary, OrchEventKind};

    // s8 T9 (non-divergence): the workflow-spec snapshot has ONE construction site
    // (`encode_workflow_spec`) shared by `Coordinator::run_workflow` and the A2A unary Workflow
    // arm, and it round-trips through the resume-path `WorkflowSpecEnvelope` at the supported
    // version. If these ever drift, a detached task submitted on one surface can't be resumed.
    #[test]
    fn workflow_spec_envelope_round_trips_at_supported_version() {
        use bridge_core::ids::{AgentId, NodeId, WorkflowId};
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
        let graph = WorkflowGraph {
            id: WorkflowId::parse("code-review").unwrap(),
            nodes: vec![WorkflowNode {
                id: NodeId::parse("only").unwrap(),
                agent: AgentId::parse("codex").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: Vec::new(),
                retry: None,
            }],
            panel: None,
        };
        let json = encode_workflow_spec(&graph);
        let env: WorkflowSpecEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env.v, SUPPORTED_SNAPSHOT_VERSION);
        assert_eq!(env.graph.id.as_str(), "code-review");
        assert_eq!(env.graph.nodes.len(), 1);
    }

    #[test]
    fn workflow_spec_node_ids_reads_persisted_snapshot() {
        use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
        let graph = WorkflowGraph {
            id: bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
            nodes: vec![
                WorkflowNode {
                    id: bridge_core::ids::NodeId::parse("reviewer").unwrap(),
                    agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                    prompt_template: "{{input}}".into(),
                    inputs: Vec::new(),
                    retry: None,
                },
                WorkflowNode {
                    id: bridge_core::ids::NodeId::parse("synth").unwrap(),
                    agent: bridge_core::ids::AgentId::parse("codex").unwrap(),
                    prompt_template: "{{reviewer}}".into(),
                    inputs: vec![bridge_core::ids::NodeId::parse("reviewer").unwrap()],
                    retry: None,
                },
            ],
            panel: None,
        };
        let json = encode_workflow_spec(&graph);

        let nodes = workflow_spec_node_ids(&json).unwrap();

        assert_eq!(
            nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["reviewer", "synth"]
        );
    }

    #[test]
    fn workflow_spec_node_ids_rejects_bad_snapshot() {
        // A corrupt stored snapshot (bad node id, unknown version, non-JSON) is a
        // StoreFailure, never a client InvalidRequest.
        assert!(matches!(
            workflow_spec_node_ids(
                r#"{"v":1,"graph":{"id":"w","nodes":[{"id":"BAD","agent":"codex","prompt_template":"","inputs":[]}]}}"#
            ),
            Err(bridge_core::error::BridgeError::StoreFailure)
        ));
        assert!(workflow_spec_node_ids(r#"{"v":999,"graph":{"id":"w","nodes":[]}}"#).is_err());
        assert!(workflow_spec_node_ids("not json").is_err());
    }

    #[tokio::test]
    async fn hub_delivers_published_frame_to_active_subscriber() {
        let hub = TaskProgressHub::new();
        let mut rx = hub.subscribe();
        hub.publish(WorkflowProgressFrame {
            v: 1,
            seq: 5,
            phase: Phase::Live,
            kind: FrameKind::NodeFinished {
                node: "a".into(),
                ok: true,
                output: "o".into(),
                usage: None,
            },
        });
        let f = rx.recv().await.unwrap();
        assert_eq!(f.seq, 5);
    }

    #[test]
    fn frame_serializes_with_top_level_kind_discriminator() {
        // Lock the SSE wire contract: the FrameKind tag lands at the TOP level
        // (no `kind.kind` nesting), and variant fields are flattened up.
        let frame = WorkflowProgressFrame {
            v: 1,
            seq: 5,
            phase: Phase::Snapshot,
            kind: FrameKind::NodeFinished {
                node: "synth".into(),
                ok: true,
                output: "done".into(),
                usage: None,
            },
        };
        let val: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(val["v"], 1);
        assert_eq!(val["seq"], 5);
        assert_eq!(val["phase"], "snapshot");
        assert_eq!(val["kind"], "node_finished"); // top-level discriminator, not nested
        assert_eq!(val["node"], "synth");
        assert_eq!(val["ok"], true);
        assert_eq!(val["output"], "done");
        // SnapshotComplete is a bare discriminator with no extra fields.
        let sentinel = WorkflowProgressFrame {
            v: 1,
            seq: 9,
            phase: Phase::Snapshot,
            kind: FrameKind::SnapshotComplete,
        };
        let sv: serde_json::Value = serde_json::to_value(&sentinel).unwrap();
        assert_eq!(sv["kind"], "snapshot_complete");
    }

    #[test]
    fn project_orch_frame_rich() {
        let f = project_orch_frame(
            &OrchEventKind::ToolCall {
                tool_call_id: "t1".into(),
                title: "x".into(),
                kind: "read".into(),
                status: "completed".into(),
                locations: vec![],
                content: Some(ContentSummary {
                    item_count: 1,
                    preview: "p".into(),
                }),
            },
            Phase::Live,
            5,
        )
        .unwrap();
        let j = serde_json::to_value(&f).unwrap();
        assert_eq!(j["kind"], "tool_call");
        assert_eq!(j["tool_kind"], "read");
        assert_eq!(j["content_preview"], "p");
        assert_eq!(f.seq, 5);
        let pf =
            project_orch_frame(&OrchEventKind::Plan { entries: vec![] }, Phase::Live, 6).unwrap();
        assert!(matches!(pf.kind, FrameKind::Plan { .. }));
    }

    #[test]
    fn diagnostic_progress_is_journal_only_and_projection_is_total() {
        use bridge_core::diagnostics::{
            DiagnosticEvent, DiagnosticPhase, DiagnosticRedactor, PersistedPhaseTransition,
            PersistedPhaseTransitionInput, PhaseStatus,
        };

        let diagnostic = DiagnosticEvent::new(
            PersistedPhaseTransition::build(
                PersistedPhaseTransitionInput {
                    phase: DiagnosticPhase::Initialize,
                    status: PhaseStatus::Started,
                    at_ms: 42,
                    operation: None,
                    code: Some("acp.initialize.started".into()),
                    auth: None,
                },
                &DiagnosticRedactor::default(),
            )
            .unwrap(),
            None,
        )
        .unwrap();
        let progress = OrchEventKind::Progress {
            progress: bridge_core::orch::ProgressPayload::diagnostic(diagnostic),
        };

        assert!(project_orch_frame(&progress, Phase::Live, 1).is_none());
        assert!(project_orch_frame(&progress, Phase::Snapshot, 1).is_none());

        let node = project_orch_frame(
            &OrchEventKind::NodeStarted {
                node: "next".into(),
            },
            Phase::Live,
            2,
        )
        .unwrap();
        assert!(matches!(node.kind, FrameKind::NodeStarted { .. }));

        let terminal = project_orch_frame(
            &OrchEventKind::Terminal {
                status: TerminalStatus::Completed,
                output: "done".into(),
            },
            Phase::Live,
            3,
        )
        .unwrap();
        assert!(matches!(
            terminal.kind,
            FrameKind::Terminal {
                outcome: TerminalOutcome::Completed,
                ..
            }
        ));
    }
}
