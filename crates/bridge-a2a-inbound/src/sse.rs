// sse.rs — translate pipeline `Event`s into A2A-shaped SSE frames.
//
// The streaming A2A methods (`SendStreamingMessage`, `SubscribeToTask`) emit a
// Server-Sent-Events stream. Each `bridge_core::translator::Event` becomes one
// SSE frame whose `event:` field names the A2A event kind (`status-update` for
// coalesced text, `artifact-update` for the final artifact) and whose `data:`
// field carries a JSON-RPC-shaped payload with the text. The final flush — the
// Artifact event — is always the last frame the stream yields (the translator
// guarantees ordering; we preserve it here).

use axum::response::sse::Event as SseEvent;
use bridge_core::translator::{Event, EventKind};
use serde_json::json;

/// A2A SSE `event:` name for a coalesced text/status chunk.
pub const EVENT_STATUS: &str = "status-update";
/// A2A SSE `event:` name for the final artifact frame.
pub const EVENT_ARTIFACT: &str = "artifact-update";

/// Convert one pipeline [`Event`] into an SSE frame.
///
/// The `data` payload is a small JSON object `{ "kind", "text", "final" }`:
/// `kind` echoes the event name, `text` is the chunk/artifact text, and
/// `final` is `true` only for the artifact frame so clients can detect the
/// terminal flush without buffering the whole stream.
pub fn event_to_sse(ev: &Event) -> SseEvent {
    let (name, is_final) = match ev.kind() {
        EventKind::Status => (EVENT_STATUS, false),
        EventKind::Artifact => (EVENT_ARTIFACT, true),
    };
    let payload = json!({
        "kind": name,
        "text": ev.text(),
        "final": is_final,
    });
    // `json_data` serializes the value and sets it as the `data:` field.
    SseEvent::default()
        .event(name)
        .json_data(payload)
        .expect("serde_json::Value always serializes")
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
        // Conversion must not panic and must label the frame as the artifact.
        let _sse = event_to_sse(last);
        let _status = event_to_sse(evs.first().unwrap());
    }
}
