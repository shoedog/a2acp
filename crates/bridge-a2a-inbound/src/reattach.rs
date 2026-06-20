// reattach.rs — per-task in-memory broadcast hub + wire types for streaming reattach.
// Consumed by Task 4 (DetachedProgressSink publishes) and Tasks 7-9 (SubscribeToTask reads).

use bridge_core::orch::{OrchEventKind, PlanEntry};
use serde::Serialize;
use tokio::sync::broadcast;

/// Whether this frame comes from a historical snapshot replay or from live streaming.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Snapshot,
    Live,
}

/// Serializable mirror of bridge_workflow::executor::WorkflowOutcome (which is not Serialize).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TerminalOutcome {
    Completed,
    Failed,
    Canceled,
}

impl TerminalOutcome {
    pub(crate) fn from_workflow(o: &bridge_workflow::executor::WorkflowOutcome) -> Self {
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
pub(crate) enum FrameKind {
    #[allow(dead_code)] // Task 7 lands before the rich sink/projection callers.
    Plan {
        entries: Vec<PlanEntry>,
    },
    #[allow(dead_code)] // Task 7 lands before the rich sink/projection callers.
    ToolCall {
        tool_call_id: String,
        title: String,
        #[serde(rename = "tool_kind")]
        kind: String,
        status: String,
        locations: Vec<String>,
        content_preview: Option<String>,
    },
    #[allow(dead_code)] // Task 7 lands before the rich sink/projection callers.
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
pub(crate) struct WorkflowProgressFrame {
    pub v: u8,
    pub seq: i64,
    pub phase: Phase,
    #[serde(flatten)]
    pub kind: FrameKind,
}

#[allow(dead_code)] // Task 7 lands before the rich sink/projection callers.
pub(crate) fn frame_from_orch(
    kind: &OrchEventKind,
    phase: Phase,
    seq: i64,
) -> WorkflowProgressFrame {
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
pub(crate) struct TaskProgressHub {
    tx: broadcast::Sender<WorkflowProgressFrame>,
}

impl TaskProgressHub {
    pub(crate) fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WorkflowProgressFrame> {
        self.tx.subscribe()
    }

    /// Publish a frame best-effort: if no receivers are listening the send is silently dropped.
    pub(crate) fn publish(&self, f: WorkflowProgressFrame) {
        let _ = self.tx.send(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::orch::{ContentSummary, OrchEventKind};

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
