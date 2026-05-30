// sse.rs — translate pipeline `Event`s into real A2A `StreamResponse` SSE frames.
//
// The streaming A2A methods (`SendStreamingMessage`, `SubscribeToTask`) emit a
// Server-Sent-Events stream. Each `bridge_core::translator::Event` becomes one
// SSE frame whose `data:` field carries a JSON-serialized `a2a::StreamResponse`,
// making our inbound stream parseable by any A2A-conformant client (including
// our own outbound client). This closes the v1 wire-conformance gap.
//
// Design: `event_to_streamresponse` is the pure, testable seam that constructs
// the correct `a2a::StreamResponse` variant; `event_to_sse` is a thin wrapper
// that serialises the response and wraps it in an `axum::response::sse::Event`.

use std::collections::HashMap;

use axum::response::sse::Event as SseEvent;
use bridge_core::translator::{Event, EventKind, TaskOutcome};

/// A2A SSE `event:` name for a coalesced text/status chunk.
pub const EVENT_STATUS: &str = "status-update";
/// A2A SSE `event:` name for the final artifact frame.
pub const EVENT_ARTIFACT: &str = "artifact-update";

/// Pure mapping from a pipeline [`Event`] to a real [`a2a::StreamResponse`].
///
/// * A `Status` event becomes a `StreamResponse::StatusUpdate` carrying the
///   chunk text in a `Working` task status with an agent message.
/// * An `Artifact` event becomes a `StreamResponse::ArtifactUpdate` with
///   `last_chunk: Some(true)` so clients can detect the terminal flush.
/// * A `Terminal` event becomes a `StreamResponse::StatusUpdate` carrying the
///   terminal `TaskState` (Completed/Failed/Canceled) with no message.
/// * If `ev.source()` is `Some(id)`, `metadata["a2a-bridge.source"]` is set on
///   the emitted event; for artifacts, `artifact.name` is also set to `id`.
pub fn event_to_streamresponse(ev: &Event, task_id: &str, context_id: &str) -> a2a::StreamResponse {
    /// Build a `metadata` map from a source label, if present.
    fn source_metadata(ev: &Event) -> Option<HashMap<String, serde_json::Value>> {
        ev.source().map(|id| {
            let mut m = HashMap::new();
            m.insert(
                "a2a-bridge.source".to_owned(),
                serde_json::Value::String(id.to_owned()),
            );
            m
        })
    }

    match ev.kind() {
        EventKind::Status => {
            let message = a2a::Message {
                message_id: a2a::new_message_id(),
                context_id: Some(context_id.to_owned()),
                task_id: Some(task_id.to_owned()),
                role: a2a::Role::Agent,
                parts: vec![a2a::Part::text(ev.text())],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            };
            a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
                task_id: task_id.to_owned(),
                context_id: context_id.to_owned(),
                status: a2a::TaskStatus {
                    state: a2a::TaskState::Working,
                    message: Some(message),
                    timestamp: None,
                },
                metadata: source_metadata(ev),
            })
        }
        EventKind::Artifact => {
            // Use source as the artifact name when present; otherwise fall back to "output".
            let name = ev
                .source()
                .map(|s| s.to_owned())
                .unwrap_or_else(|| "output".to_owned());
            a2a::StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
                task_id: task_id.to_owned(),
                context_id: context_id.to_owned(),
                artifact: a2a::Artifact {
                    artifact_id: a2a::new_artifact_id(),
                    name: Some(name),
                    description: None,
                    parts: vec![a2a::Part::text(ev.text())],
                    metadata: None,
                    extensions: None,
                },
                append: None,
                last_chunk: Some(true),
                metadata: source_metadata(ev),
            })
        }
        EventKind::Terminal => {
            let state = match ev.outcome() {
                Some(TaskOutcome::Completed) | None => a2a::TaskState::Completed,
                Some(TaskOutcome::Failed) => a2a::TaskState::Failed,
                Some(TaskOutcome::Canceled) => a2a::TaskState::Canceled,
            };
            a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
                task_id: task_id.to_owned(),
                context_id: context_id.to_owned(),
                status: a2a::TaskStatus {
                    state,
                    message: None,
                    timestamp: None,
                },
                metadata: source_metadata(ev),
            })
        }
    }
}

/// Convert one pipeline [`Event`] into an SSE frame carrying a real
/// `a2a::StreamResponse` as the `data:` JSON payload.
pub fn event_to_sse(ev: &Event, task_id: &str, context_id: &str) -> SseEvent {
    let sr = event_to_streamresponse(ev, task_id, context_id);
    let event_name = match ev.kind() {
        EventKind::Status => EVENT_STATUS,
        EventKind::Artifact => EVENT_ARTIFACT,
        // Terminal events are mapped in Task 2; use status-update as a placeholder.
        EventKind::Terminal => EVENT_STATUS,
    };
    let data = serde_json::to_string(&sr).expect("a2a::StreamResponse always serializes");
    SseEvent::default().event(event_name).data(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{SessionId, TaskId};
    use bridge_core::ports::*;
    use bridge_core::translator::Translator;
    use futures::StreamExt;
    use std::sync::Mutex;

    // ---- TDD: pure function tests (no need to parse axum SseEvent internals) ----

    #[test]
    fn status_event_serializes_as_streamresponse_statusupdate() {
        let sr = event_to_streamresponse(&Event::status("hello"), "task-1", "ctx-1");
        let data = serde_json::to_string(&sr).unwrap();
        let parsed: a2a::StreamResponse = serde_json::from_str(&data).unwrap();
        assert!(matches!(parsed, a2a::StreamResponse::StatusUpdate(_)));
        assert!(
            data.contains("hello"),
            "data should contain 'hello': {data}"
        );
    }

    #[test]
    fn artifact_event_serializes_as_streamresponse_artifactupdate_lastchunk() {
        let sr = event_to_streamresponse(&Event::artifact("RESULT"), "task-1", "ctx-1");
        let data = serde_json::to_string(&sr).unwrap();
        let parsed: a2a::StreamResponse = serde_json::from_str(&data).unwrap();
        match parsed {
            a2a::StreamResponse::ArtifactUpdate(e) => {
                assert_eq!(e.last_chunk, Some(true));
            }
            _ => panic!("expected artifactUpdate"),
        }
        assert!(
            data.contains("RESULT"),
            "data should contain 'RESULT': {data}"
        );
    }

    #[test]
    fn status_event_carries_task_and_context_ids() {
        let sr = event_to_streamresponse(&Event::status("chunk"), "my-task", "my-ctx");
        match sr {
            a2a::StreamResponse::StatusUpdate(e) => {
                assert_eq!(e.task_id, "my-task");
                assert_eq!(e.context_id, "my-ctx");
                assert_eq!(e.status.state, a2a::TaskState::Working);
                let msg = e.status.message.expect("message should be set");
                assert_eq!(msg.role, a2a::Role::Agent);
                assert!(msg.parts.iter().any(|p| p.as_text() == Some("chunk")));
            }
            _ => panic!("expected StatusUpdate"),
        }
    }

    #[test]
    fn artifact_event_carries_task_and_context_ids() {
        let sr = event_to_streamresponse(&Event::artifact("done"), "t-2", "c-2");
        match sr {
            a2a::StreamResponse::ArtifactUpdate(e) => {
                assert_eq!(e.task_id, "t-2");
                assert_eq!(e.context_id, "c-2");
                assert_eq!(e.artifact.name.as_deref(), Some("output"));
                assert!(e.artifact.parts.iter().any(|p| p.as_text() == Some("done")));
                assert_eq!(e.last_chunk, Some(true));
                assert!(e.append.is_none());
            }
            _ => panic!("expected ArtifactUpdate"),
        }
    }

    #[test]
    fn event_to_sse_produces_correct_event_names() {
        // Verify the event: field is set correctly (inspect via Debug).
        let status_sse = event_to_sse(&Event::status("hi"), "t", "c");
        let artifact_sse = event_to_sse(&Event::artifact("bye"), "t", "c");
        // The SseEvent Debug output includes the event name — use it as a proxy.
        let status_debug = format!("{status_sse:?}");
        let artifact_debug = format!("{artifact_sse:?}");
        assert!(
            status_debug.contains("status-update"),
            "status SSE event name wrong: {status_debug}"
        );
        assert!(
            artifact_debug.contains("artifact-update"),
            "artifact SSE event name wrong: {artifact_debug}"
        );
    }

    // ---- integration-style: verify the full translator→sse pipeline ----

    struct FakeBackend(Mutex<Option<Vec<Result<Update, BridgeError>>>>);
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<bridge_core::domain::Part>,
        ) -> Result<BackendStream, BridgeError> {
            let v = self.0.lock().unwrap().take().unwrap();
            Ok(Box::pin(tokio_stream::iter(v)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore;
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, _t: &TaskId, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn session_for(&self, _t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(None)
        }
        async fn put_pending(
            &self,
            _t: &TaskId,
            _r: &bridge_core::domain::PendingRequest,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn take_pending(
            &self,
            _t: &TaskId,
        ) -> Result<Option<bridge_core::domain::PendingRequest>, BridgeError> {
            Ok(None)
        }
        async fn set_peer_task(
            &self,
            _t: &TaskId,
            _peer: &bridge_core::domain::PeerTaskId,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn peer_task_for(
            &self,
            _t: &TaskId,
        ) -> Result<Option<bridge_core::domain::PeerTaskId>, BridgeError> {
            Ok(None)
        }
        async fn request_cancel(&self, _t: &TaskId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn cancel_requested(&self, _t: &TaskId) -> Result<bool, BridgeError> {
            Ok(false)
        }
        async fn set_fanout(&self, _t: &TaskId) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn is_fanout(&self, _t: &TaskId) -> Result<bool, BridgeError> {
            Ok(false)
        }
    }

    struct AutoApprove;
    impl PolicyEngine for AutoApprove {
        fn decide(
            &self,
            _req: &bridge_core::domain::PermissionRequest,
            _c: &bridge_core::domain::SessionContext,
        ) -> Result<bridge_core::domain::PermissionDecision, BridgeError> {
            Ok(bridge_core::domain::PermissionDecision::Approve)
        }
    }

    #[tokio::test]
    async fn translator_pipeline_last_event_is_artifact_streamresponse() {
        let be = FakeBackend(Mutex::new(Some(vec![
            Ok(Update::Text("hello".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }),
        ])));
        let st = FakeStore;
        let pol = AutoApprove;
        let t = TaskId::parse("t").unwrap();
        let s = SessionId::parse("s").unwrap();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        // Last event must map to ArtifactUpdate.
        let last = evs.last().unwrap();
        assert_eq!(last.kind(), &EventKind::Artifact);
        let sr = event_to_streamresponse(last, t.as_str(), t.as_str());
        assert!(matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)));

        // First event must map to StatusUpdate.
        let first = evs.first().unwrap();
        assert_eq!(first.kind(), &EventKind::Status);
        let sr_first = event_to_streamresponse(first, t.as_str(), t.as_str());
        assert!(matches!(sr_first, a2a::StreamResponse::StatusUpdate(_)));
    }

    // ---- Task 2: Terminal mapping + source-labeled frames ----

    #[test]
    fn terminal_event_maps_to_status_update_with_terminal_state() {
        use bridge_core::translator::{Event, TaskOutcome};
        let done = event_to_streamresponse(&Event::terminal(TaskOutcome::Completed), "t", "c");
        let a2a::StreamResponse::StatusUpdate(e) = done else {
            panic!("expected StatusUpdate for Completed")
        };
        assert_eq!(e.status.state, a2a::TaskState::Completed);
        let f = event_to_streamresponse(&Event::terminal(TaskOutcome::Failed), "t", "c");
        assert!(
            matches!(f, a2a::StreamResponse::StatusUpdate(e) if e.status.state == a2a::TaskState::Failed)
        );
        let c = event_to_streamresponse(&Event::terminal(TaskOutcome::Canceled), "t", "c");
        assert!(
            matches!(c, a2a::StreamResponse::StatusUpdate(e) if e.status.state == a2a::TaskState::Canceled)
        );
    }

    #[test]
    fn status_event_with_source_sets_metadata() {
        use bridge_core::translator::Event;
        let a2a::StreamResponse::StatusUpdate(e) =
            event_to_streamresponse(&Event::status("x").with_source("peer"), "t", "c")
        else {
            panic!("expected StatusUpdate")
        };
        assert_eq!(
            e.metadata.unwrap().get("a2a-bridge.source").unwrap(),
            "peer"
        );
    }

    #[test]
    fn artifact_event_with_source_sets_name_and_metadata() {
        use bridge_core::translator::Event;
        let a2a::StreamResponse::ArtifactUpdate(e) =
            event_to_streamresponse(&Event::artifact("R").with_source("kiro"), "t", "c")
        else {
            panic!("expected ArtifactUpdate")
        };
        assert_eq!(e.artifact.name.as_deref(), Some("kiro"));
        assert_eq!(
            e.metadata.unwrap().get("a2a-bridge.source").unwrap(),
            "kiro"
        );
    }

    #[tokio::test]
    async fn artifact_event_is_marked_final() {
        let be = FakeBackend(Mutex::new(Some(vec![
            Ok(Update::Text("hello".into())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }),
        ])));
        let st = FakeStore;
        let pol = AutoApprove;
        let t = TaskId::parse("t").unwrap();
        let s = SessionId::parse("s").unwrap();
        let evs: Vec<Event> = Translator::new()
            .run(&be, &st, &pol, &t, &s, vec![])
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        // Last event maps to the artifact SSE frame.
        let last = evs.last().unwrap();
        assert_eq!(last.kind(), &EventKind::Artifact);
        // Conversion must not panic and must produce ArtifactUpdate with last_chunk=true.
        let sr = event_to_streamresponse(last, "t", "t");
        match sr {
            a2a::StreamResponse::ArtifactUpdate(e) => {
                assert_eq!(e.last_chunk, Some(true));
            }
            _ => panic!("expected ArtifactUpdate"),
        }
        // event_to_sse must not panic.
        let _sse = event_to_sse(last, "t", "t");
        let _status = event_to_sse(evs.first().unwrap(), "t", "t");
    }
}
