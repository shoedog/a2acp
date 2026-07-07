// reattach.rs — per-task in-memory broadcast hub + wire types for streaming reattach.
// Consumed by Task 4 (DetachedProgressSink publishes) and Tasks 7-9 (SubscribeToTask reads).

use bridge_core::orch::{OrchEventKind, PlanEntry};
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

pub fn frame_from_orch(kind: &OrchEventKind, phase: Phase, seq: i64) -> WorkflowProgressFrame {
    let kind = match kind {
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
        _ => unreachable!("frame_from_orch only accepts rich orchestration events"),
    };

    WorkflowProgressFrame {
        v: 1,
        seq,
        phase,
        kind,
    }
}

/// Per-task in-memory broadcast hub. Wraps a `tokio::sync::broadcast` channel so
/// the DetachedProgressSink (publisher) and SubscribeToTask handler (subscriber)
/// can communicate progress frames without sharing a lock.
pub struct TaskProgressHub {
    tx: broadcast::Sender<WorkflowProgressFrame>,
}

impl TaskProgressHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowProgressFrame> {
        self.tx.subscribe()
    }

    /// Publish a frame best-effort: if no receivers are listening the send is silently dropped.
    pub fn publish(&self, f: WorkflowProgressFrame) {
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
use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome, WorkflowStream};
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
    mut stream: WorkflowStream,
    sink: &mut S,
) -> Result<bool, BridgeError> {
    let mut terminal_seen = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => sink.node_started(node.as_str()).await?,
            Ok(WorkflowEvent::NodeFinished {
                node,
                ok,
                output,
                usage,
            }) => {
                sink.node_finished(node.as_str(), ok, &output, usage.as_ref())
                    .await?
            }
            Ok(WorkflowEvent::Terminal { outcome, output }) => {
                sink.terminal(outcome, output).await?;
                terminal_seen = true;
            }
            Err(e) => sink.error(e).await?,
        }
    }
    Ok(terminal_seen)
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
use bridge_workflow::executor::{WorkflowExecutor, WorkflowRunContext};
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
}

/// Detached progress sink: persists each event via the sequenced store methods
/// (durable-first), then publishes a `WorkflowProgressFrame` to the task's
/// in-memory `TaskProgressHub`. A durable-write `Err` propagates (aborts the
/// drain) — preserving the W3b "checkpoint-write-failure ⇒ task Failed" contract.
pub struct DetachedProgressSink {
    store: Arc<dyn TaskStore>,
    task: TaskId,
    hub: Arc<TaskProgressHub>,
}

impl DetachedProgressSink {
    pub fn new(store: Arc<dyn TaskStore>, task: TaskId, hub: Arc<TaskProgressHub>) -> Self {
        Self { store, task, hub }
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
        let operation_id = self.operation_id()?;
        let seq = self
            .store
            .set_terminal_sequenced(&self.task, &operation_id, status, result, error, now_ms())
            .await?;
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
                    self.hub
                        .publish(crate::detached::frame_from_orch(&kind, Phase::Live, seq))
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
        tokio::spawn(async move {
            // Reuse the shared detached-finalize path: sequenced terminal + Terminal frame
            // broadcast + hub removal (so the panic path matches every other terminal).
            let _ = finalize_detached(
                &store,
                &progress_hubs,
                &task,
                TaskRecordStatus::Failed,
                None,
                Some("runner ended without terminal"),
                Some(&hub),
            )
            .await;
            cancels.lock().await.remove(&task);
        });
    }
}

#[cfg(test)]
mod sink_tests {
    use super::*;

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
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
            batch_id: None,
            item_id: None,
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
                input: "DIFF".into(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
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
    use bridge_core::task_store::TaskRecordStatus;
    // Durable-first: write the sequenced terminal. Capture the result instead of
    // early-returning so the hub is removed REGARDLESS of write success (I-1) — a
    // write-Err must NEVER leak the in-memory hub. The durable Working-row gap on a
    // write-Err is the pre-existing W3b §8 gap, backstopped by the boot sweep.
    let operation_id = bridge_core::ids::OperationId::parse(format!("op-{}", task.as_str()))?;
    let write = store
        .set_terminal_sequenced(task, &operation_id, status, result, error, now_ms())
        .await;
    if let (Ok(seq), Some(hub)) = (&write, hub) {
        // Publish the Terminal frame ONLY on a committed seq (durable-first): on a
        // write-Err there is no seq to publish on.
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
            seq: *seq,
            phase: Phase::Live,
            kind: FrameKind::Terminal { outcome, output },
        });
    }
    // ALWAYS remove the hub, even on write error (I-1: the hub never leaks).
    progress_hubs.lock().await.remove(task);
    write.map(|_| ())
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_detached_workflow(
    deps: &DetachedDeps,
    task: TaskId,
    input: String,
    graph: Arc<WorkflowGraph>,
    run_id: String,
    token: CancellationToken,
    seed: HashMap<String, (String, bool, Option<bridge_core::orch::UsageSnapshot>)>,
    mut ctx: WorkflowRunContext,
    hub: Arc<TaskProgressHub>,
) -> tokio::task::JoinHandle<()> {
    let deps = deps.clone();
    tokio::spawn(async move {
        let mut fin = Finalizer {
            store: deps.task_store.clone(),
            task: task.clone(),
            cancels: deps.workflow_cancels.clone(),
            progress_hubs: deps.progress_hubs.clone(),
            hub: hub.clone(),
            done: false,
        };
        let executor = match &deps.executor {
            Some(e) => e.clone(),
            None => {
                // No executor wired: finalize Failed via the sequenced path (the hub was
                // inserted before spawn, so publish + clean it up).
                let _ = finalize_detached(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("no executor wired"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                return;
            }
        };
        let op = match OperationId::parse(format!("op-{}", task.as_str())) {
            Ok(op) => op,
            Err(_) => {
                let _ = finalize_detached(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("bad operation id"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
                return;
            }
        };
        ctx.make_rich_sink = Some(Arc::new(DetachedRichSinkFactory {
            store: deps.task_store.clone(),
            task: task.clone(),
            op,
            hub: hub.clone(),
        }));
        let stream = executor.run_from_with_context(graph, input, run_id, token, seed, ctx);
        // The DetachedProgressSink OWNS the sequenced terminal write: on a clean drain it
        // has already written `set_terminal_sequenced` AND published the Terminal frame.
        let mut sink =
            DetachedProgressSink::new(deps.task_store.clone(), task.clone(), hub.clone());
        match drain_workflow(stream, &mut sink).await {
            Ok(true) => {
                // Sink already committed+published the terminal. Do NOT write it again.
                // M1: flip the finalizer done flag BEFORE the hub-removal await so the
                // Finalizer's Drop can never clobber the committed terminal during the
                // .await suspension point below.
                fin.done = true;
                deps.progress_hubs.lock().await.remove(&task);
                deps.workflow_cancels.lock().await.remove(&task);
            }
            Ok(false) => {
                // Drain ended with no terminal: finalize Failed via the sequenced path
                // (also removes the hub).
                let _ = finalize_detached(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("workflow ended without terminal"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
            }
            Err(e) => {
                tracing::warn!(task = task.as_str(), error = ?e, "drain_workflow sink error; marking task Failed");
                let _ = finalize_detached(
                    &deps.task_store,
                    &deps.progress_hubs,
                    &task,
                    bridge_core::task_store::TaskRecordStatus::Failed,
                    None,
                    Some("checkpoint write failed"),
                    Some(&hub),
                )
                .await;
                fin.done = true;
                deps.workflow_cancels.lock().await.remove(&task);
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
) -> tokio::task::JoinHandle<()> {
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
) -> tokio::task::JoinHandle<()> {
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

    // (4) Terminal short-circuit: if the graph's terminal node already has a
    //     checkpoint, the workflow had actually FINISHED before the crash (its
    //     terminal output was produced but the row wasn't flipped — the W3a §8
    //     write-failure gap). Finalize DIRECTLY from the checkpoint, with NO
    //     re-run and WITHOUT consuming a resume attempt. Completed carries the
    //     output as `result`; Failed carries it as `error` (mirrors
    //     `finalize_detached` / `DetachedProgressSink::terminal`).
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
    if let Some((output, ok, _usage)) = seed.get(&terminal_id) {
        let (status, result, error) = if *ok {
            (TaskRecordStatus::Completed, Some(output.as_str()), None)
        } else {
            (TaskRecordStatus::Failed, None, Some(output.as_str()))
        };
        // Pre-spawn terminal (no hub inserted): sequenced finalize so terminal_seq
        // is never NULL.
        if let Err(e) = finalize_detached(
            &deps.task_store,
            &deps.progress_hubs,
            &task,
            status,
            result,
            error,
            None,
        )
        .await
        {
            tracing::warn!(task = task.as_str(), error = ?e, "resume scan: set_terminal(short-circuit) failed");
        } else {
            tracing::info!(task = task.as_str(), status = ?status, "resume scan: short-circuited to terminal");
        }
        return;
    }

    // (5) Otherwise claim a resume attempt (atomic; increments resume_attempts).
    match deps
        .task_store
        .claim_resume_attempt(&task, cap, deps.clock.now_ms())
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
            // Re-validate the persisted session_cwd before spawning: never trust
            // the stored string blindly. A corrupt/invalid stored cwd is not
            // resumable — interrupt BEFORE registering the cancel token or spawning
            // so no orphaned token/runner is left behind.
            let ctx = match wt.session_cwd.as_deref() {
                Some(s) => match bridge_core::SessionCwd::parse(s) {
                    Ok(c) => bridge_workflow::executor::WorkflowRunContext {
                        session_cwd: Some(c),
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
                        tracing::info!(
                            task = task.as_str(),
                            "resume scan: interrupted (unreadable session cwd)"
                        );
                        return;
                    }
                },
                None => bridge_workflow::executor::WorkflowRunContext {
                    observer: deps.observer.clone(),
                    ..bridge_workflow::executor::WorkflowRunContext::default()
                },
            };
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
            let run_id = format!("{}-resume-{}", task.as_str(), attempt);
            // Detached: the runner re-runs only the un-checkpointed nodes
            // (run_from skips the seeded ones) and writes their checkpoints + the
            // terminal as usual. No JoinHandle is awaited here.
            drop(spawn_detached_workflow(
                deps,
                task.clone(),
                wt.input.clone(),
                Arc::new(graph),
                run_id.clone(),
                token,
                seed,
                ctx,
                hub,
            ));
            tracing::info!(task = task.as_str(), attempt, run_id = %run_id, "resume scan: resumed from checkpoints");
        }
        Err(e) => {
            tracing::warn!(task = task.as_str(), error = ?e, "resume scan: claim_resume_attempt() failed; skipping task");
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
    use bridge_core::domain::{AgentEntry, AgentKind, Part, RegistrySnapshot};
    use bridge_core::ids::{AgentId, NodeId, OperationId, SessionId, WorkflowId};
    use bridge_core::orch::UsageSnapshot;
    use bridge_core::ports::{
        AgentBackend, AgentRegistry, BackendStream, Lease, ObsEvent, Observer, Resolved, Update,
    };
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
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

    #[tokio::test]
    async fn resume_working_task_synth_sees_checkpointed_member_usage() {
        let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
        let task = TaskId::parse("t-resume-usage").unwrap();
        let graph = panel_graph();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "panel".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                input: "DIFF".into(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
            })
            .await
            .unwrap();

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
        let task = TaskId::parse("t-resume-no-checkpoint").unwrap();
        let graph = single_retry_graph();
        store
            .create(&TaskRecord {
                id: task.clone(),
                workflow: "retry-resume".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: 1,
                updated_ms: 1,
                input: "DIFF".into(),
                workflow_spec_json: Some(encode_workflow_spec(&graph)),
                resume_attempts: 0,
                session_cwd: None,
                batch_id: None,
                item_id: None,
            })
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
    fn frame_from_orch_rich() {
        let f = frame_from_orch(
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
        );
        let j = serde_json::to_value(&f).unwrap();
        assert_eq!(j["kind"], "tool_call");
        assert_eq!(j["tool_kind"], "read");
        assert_eq!(j["content_preview"], "p");
        assert_eq!(f.seq, 5);
        let pf = frame_from_orch(&OrchEventKind::Plan { entries: vec![] }, Phase::Live, 6);
        assert!(matches!(pf.kind, FrameKind::Plan { .. }));
    }
}
