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
    async fn node_started(&mut self, _node: &str) {}
    async fn node_finished(&mut self, _node: &str, _ok: bool) {}
    async fn terminal(&mut self, outcome: WorkflowOutcome, output: String);
    async fn error(&mut self, _err: BridgeError) {}
}

/// Drive the stream into the sink. Returns `true` if a terminal event was seen
/// (the caller handles the no-terminal case per its own sink semantics).
pub(crate) async fn drain_workflow<S: WorkflowSink>(
    mut stream: WorkflowStream,
    sink: &mut S,
) -> bool {
    let mut terminal_seen = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(WorkflowEvent::NodeStarted { node }) => sink.node_started(node.as_str()).await,
            Ok(WorkflowEvent::NodeFinished { node, ok, output }) => {
                let _ = output;
                sink.node_finished(node.as_str(), ok).await
            }
            Ok(WorkflowEvent::Terminal { outcome, output }) => {
                sink.terminal(outcome, output).await;
                terminal_seen = true;
            }
            Err(e) => sink.error(e).await,
        }
    }
    terminal_seen
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
    async fn terminal(&mut self, outcome: WorkflowOutcome, output: String) {
        self.terminal = Some(match outcome {
            WorkflowOutcome::Completed => (TaskRecordStatus::Completed, Some(output), None),
            WorkflowOutcome::Failed => (TaskRecordStatus::Failed, None, Some(output)),
            WorkflowOutcome::Canceled => (TaskRecordStatus::Canceled, None, None),
        });
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
