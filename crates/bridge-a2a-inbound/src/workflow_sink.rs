//! One drain over the workflow stream, parameterized by a sink. The streaming
//! producer (SSE) and the detached runner (TaskStore) share the drain, the
//! WorkflowOutcome mapping, and the no-terminal guard — they differ only in sink.

use bridge_core::error::BridgeError;
use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome, WorkflowStream};
use futures::StreamExt;

use crate::reattach::{FrameKind, Phase, TaskProgressHub, TerminalOutcome, WorkflowProgressFrame};

/// A sink consumes the workflow's events. Intermediate node events are optional
/// (the detached sink persists each node_finished as a checkpoint in W3b); terminal is also required.
#[async_trait::async_trait]
pub(crate) trait WorkflowSink: Send {
    async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn node_finished(
        &mut self,
        _node: &str,
        _ok: bool,
        _output: &str,
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
pub(crate) async fn drain_workflow<S: WorkflowSink>(
    mut stream: WorkflowStream,
    sink: &mut S,
) -> Result<bool, BridgeError> {
    let mut terminal_seen = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => sink.node_started(node.as_str()).await?,
            Ok(WorkflowEvent::NodeFinished { node, ok, output }) => {
                sink.node_finished(node.as_str(), ok, &output).await?
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
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

use bridge_core::ids::{NodeId, OperationId, TaskId};
use bridge_core::orch::OrchEventKind;
use bridge_core::ports::{RichEventSink, RichEventSinkFactory};
use bridge_core::task_store::{TaskRecordStatus, TaskStore};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Detached progress sink: persists each event via the sequenced store methods
/// (durable-first), then publishes a `WorkflowProgressFrame` to the task's
/// in-memory `TaskProgressHub`. A durable-write `Err` propagates (aborts the
/// drain) — preserving the W3b "checkpoint-write-failure ⇒ task Failed" contract.
pub(crate) struct DetachedProgressSink {
    store: Arc<dyn TaskStore>,
    task: TaskId,
    hub: Arc<TaskProgressHub>,
}

impl DetachedProgressSink {
    pub(crate) fn new(store: Arc<dyn TaskStore>, task: TaskId, hub: Arc<TaskProgressHub>) -> Self {
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

pub(crate) struct DetachedRichSink {
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
                        .publish(crate::reattach::frame_from_orch(&kind, Phase::Live, seq))
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

pub(crate) struct DetachedRichSinkFactory {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) task: TaskId,
    pub(crate) op: OperationId,
    pub(crate) hub: Arc<TaskProgressHub>,
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
pub(crate) struct Finalizer {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) task: TaskId,
    pub(crate) cancels: Arc<Mutex<std::collections::HashMap<TaskId, CancellationToken>>>,
    pub(crate) progress_hubs:
        Arc<Mutex<std::collections::HashMap<TaskId, Arc<crate::reattach::TaskProgressHub>>>>,
    pub(crate) hub: Arc<crate::reattach::TaskProgressHub>,
    pub(crate) done: bool,
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
            let _ = crate::server::finalize_detached(
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
            }),
            Ok(WorkflowEvent::NodeStarted { node: b.clone() }),
            Ok(WorkflowEvent::NodeFinished {
                node: b.clone(),
                ok: true,
                output: "out-b".into(),
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
        }
    }

    /// `DetachedProgressSink` persists durable events via the sequenced store
    /// methods AND publishes frames to the hub. Verifies:
    ///   - subscriber receives [NodeStarted, NodeFinished, Terminal] in order
    ///   - seqs are strictly monotonic
    ///   - the store snapshot has a checkpoint with a seq and a `terminal_seq`
    #[tokio::test]
    async fn detached_progress_sink_persists_and_publishes() {
        use crate::reattach::{FrameKind, TaskProgressHub};
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
        assert!(matches!(frames[0].phase, crate::reattach::Phase::Live));
        assert!(matches!(frames[1].phase, crate::reattach::Phase::Live));
        assert!(matches!(frames[2].phase, crate::reattach::Phase::Live));
    }

    /// A `DetachedProgressSink` whose store's `put_node_checkpoint_sequenced`
    /// returns `Err` must propagate the error so `drain_workflow` aborts —
    /// preserving the W3b "checkpoint-write-failure ⇒ task Failed" contract.
    #[tokio::test]
    async fn detached_progress_sink_write_error_aborts_drain() {
        use crate::reattach::TaskProgressHub;
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
            ) -> Result<Vec<(bridge_core::ids::NodeId, String, bool)>, BridgeError> {
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
            async fn put_node_checkpoint_sequenced(
                &self,
                _task: &TaskId,
                _node: &bridge_core::ids::NodeId,
                _operation_id: &bridge_core::ids::OperationId,
                _output: &str,
                _ok: bool,
                _ts: i64,
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
        use crate::reattach::TaskProgressHub;
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
            }],
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
