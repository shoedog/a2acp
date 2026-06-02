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
            Ok(WorkflowEvent::NodeFinished { node, ok }) => {
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
#[allow(dead_code)]
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
