//! One drain over the workflow stream, parameterized by a sink. The streaming
//! producer (SSE) and the detached runner (TaskStore) share the drain, the
//! WorkflowOutcome mapping, and the no-terminal guard — they differ only in sink.

use bridge_core::error::BridgeError;
use bridge_workflow::executor::{WorkflowEvent, WorkflowOutcome, WorkflowStream};
use futures::StreamExt;

/// A sink consumes the workflow's events. Intermediate node events are optional
/// (the detached sink ignores them in W3a); `terminal` is the meaningful one.
#[async_trait::async_trait]
pub(crate) trait WorkflowSink: Send {
    async fn node_started(&mut self, _node: &str) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn node_finished(&mut self, _node: &str, _ok: bool) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn terminal(&mut self, outcome: WorkflowOutcome, output: String)
        -> Result<(), BridgeError>;
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
                let _ = output;
                sink.node_finished(node.as_str(), ok).await?
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

use bridge_core::ids::TaskId;
use bridge_core::task_store::{TaskRecordStatus, TaskStore};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Detached sink: ignores intermediate node events (W3a has no history) and
/// captures the terminal mapping for the runner to persist.
pub(crate) struct TaskStoreSink {
    terminal: Option<(TaskRecordStatus, Option<String>, Option<String>)>,
}

impl TaskStoreSink {
    pub(crate) fn new() -> Self {
        Self { terminal: None }
    }
    /// The captured terminal mapping (status, result, error), or None if no
    /// terminal arrived.
    pub(crate) fn take(self) -> Option<(TaskRecordStatus, Option<String>, Option<String>)> {
        self.terminal
    }
}

#[async_trait::async_trait]
impl WorkflowSink for TaskStoreSink {
    async fn terminal(
        &mut self,
        outcome: WorkflowOutcome,
        output: String,
    ) -> Result<(), BridgeError> {
        self.terminal = Some(match outcome {
            WorkflowOutcome::Completed => (TaskRecordStatus::Completed, Some(output), None),
            WorkflowOutcome::Failed => (TaskRecordStatus::Failed, None, Some(output)),
            WorkflowOutcome::Canceled => (TaskRecordStatus::Canceled, None, None),
        });
        Ok(())
    }
}

/// Drop guard: if the runner exits without finalizing (early return, error, or
/// **panic**), write `Failed` and remove the cancel token. Within a serve lifetime
/// a `Working` row is then orphaned only if `set_terminal` itself fails to write
/// (the named §8 gap); the boot sweep is the cross-restart backstop.
pub(crate) struct Finalizer {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) task: TaskId,
    pub(crate) cancels: Arc<Mutex<std::collections::HashMap<TaskId, CancellationToken>>>,
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
        tokio::spawn(async move {
            let _ = store
                .set_terminal(
                    &task,
                    TaskRecordStatus::Failed,
                    None,
                    Some("runner ended without terminal"),
                    now_ms(),
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
        async fn terminal(
            &mut self,
            _o: WorkflowOutcome,
            _out: String,
        ) -> Result<(), BridgeError> {
            Err(BridgeError::StoreFailure)
        }
    }

    #[tokio::test]
    async fn drain_aborts_on_sink_error() {
        let stream: WorkflowStream = Box::pin(futures::stream::iter(vec![Ok(
            WorkflowEvent::Terminal {
                outcome: WorkflowOutcome::Completed,
                output: "x".into(),
            },
        )]));
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
        async fn node_finished(&mut self, _node: &str, _ok: bool) -> Result<(), BridgeError> {
            self.log.push("node_finished");
            Ok(())
        }
        async fn terminal(
            &mut self,
            _o: WorkflowOutcome,
            _out: String,
        ) -> Result<(), BridgeError> {
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
}
