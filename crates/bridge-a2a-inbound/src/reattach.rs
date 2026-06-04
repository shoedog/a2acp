// reattach.rs — per-task in-memory broadcast hub + wire types for streaming reattach.
// Consumed by Task 4 (DetachedProgressSink publishes) and Tasks 7-9 (SubscribeToTask reads).

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
}
