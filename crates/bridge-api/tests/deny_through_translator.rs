use bridge_api::{ApiBackend, ApiConfig};
use bridge_core::domain::{Part, PendingRequest, PeerTaskId, PermissionDecision, PermissionRequest, SessionContext};
use bridge_core::error::BridgeError;
use bridge_core::ids::{SessionId, TaskId};
use bridge_core::ports::{PolicyEngine, SessionStore};
use bridge_core::translator::Translator;
use futures::StreamExt;
use std::sync::Mutex;
use std::sync::Arc;
use wiremock::matchers::{method, path, body_string_contains};
use wiremock::{Mock, MockServer, ResponseTemplate};

// Minimal in-test store — mirror the canonical FakeStore in bridge-core/src/ports.rs tests.
#[derive(Default)]
struct FakeStore { pending: Mutex<std::collections::HashMap<String, PendingRequest>> }
#[async_trait::async_trait]
impl SessionStore for FakeStore {
    async fn put(&self, _: &TaskId, _: &SessionId) -> Result<(), BridgeError> { Ok(()) }
    async fn session_for(&self, _: &TaskId) -> Result<Option<SessionId>, BridgeError> { Ok(None) }
    async fn put_pending(&self, t: &TaskId, r: &PendingRequest) -> Result<(), BridgeError> {
        self.pending.lock().unwrap().insert(t.as_str().into(), r.clone()); Ok(())
    }
    async fn take_pending(&self, t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
        Ok(self.pending.lock().unwrap().remove(t.as_str()))
    }
    async fn set_peer_task(&self, _: &TaskId, _: &PeerTaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn peer_task_for(&self, _: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> { Ok(None) }
    async fn request_cancel(&self, _: &TaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn cancel_requested(&self, _: &TaskId) -> Result<bool, BridgeError> { Ok(false) }
    async fn set_fanout(&self, _: &TaskId) -> Result<(), BridgeError> { Ok(()) }
    async fn is_fanout(&self, _: &TaskId) -> Result<bool, BridgeError> { Ok(false) }
}

struct Deny;
impl PolicyEngine for Deny {
    fn decide(&self, _: &PermissionRequest, _: &SessionContext) -> Result<PermissionDecision, BridgeError> {
        Err(BridgeError::PermissionDenied)
    }
}

#[tokio::test]
async fn deny_through_translator_does_not_suspend() {
    let server = MockServer::start().await;
    let sse = |b: &str| ResponseTemplate::new(200).insert_header("content-type","text/event-stream").set_body_string(b);
    let call1 = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"get_current_time\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
    let call2 = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains("\"role\":\"tool\""))
        .respond_with(sse(call2)).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).and(path("/v1/chat/completions")).respond_with(sse(call1)).mount(&server).await;

    // The SAME deny policy is threaded into both the backend AND the translator (as main.rs does).
    let backend = ApiBackend::new(ApiConfig::new(format!("{}/v1", server.uri()))).with_policy(Arc::new(Deny));
    let store = FakeStore::default();
    let policy = Deny;
    let task = TaskId::parse("t1").unwrap();
    let session = SessionId::parse("s1").unwrap();

    let events: Vec<_> = Translator::new()
        .run(&backend, &store, &policy, &task, &session, vec![Part { text: "what time is it".into() }])
        .collect().await;

    // 1) The run COMPLETED — every event Ok (NO PermissionRequired suspend).
    assert!(events.iter().all(|e| e.is_ok()), "translator must not error/suspend: {events:?}");
    // 2) No pending permission persisted.
    assert!(store.take_pending(&task).await.unwrap().is_none(), "no pending — backend decided silently");
    // 3) EXACTLY two completions (a loop-to-max_tool_rounds would be 4); proves a normal terminal.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2, "tool round + one follow-up = two completions");
    // 4) The deny reached the model as a tool result; the stub tool did NOT run.
    let second = String::from_utf8_lossy(&reqs[1].body);
    assert!(second.contains("permission denied: tool not executed"));
    assert!(!second.contains("2026-01-01T00:00:00Z"));
}
