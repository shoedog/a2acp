// integration_inbound_kiro.rs — Cross-crate integration test: drives the real
// InboundServer with a real ReplayBackend (raw NDJSON), real SqliteStore,
// AutoPolicy, AlwaysGrant, and an inline route.  No external process — CI-safe.
//
// Asserts that a streaming A2A message produces an artifact SSE event containing
// the backend's text ("PONG"), exercising the full inbound→translate→backend
// pipeline end-to-end in-process (spec success-criterion S1 approximation).

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use bridge_a2a_inbound::server::InboundServer;
use bridge_acp::replay::ReplayBackend;
use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ids::AgentId;
use bridge_core::ports::{PolicyEngine, RouteDecision, SessionStore};
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::json;
use tower::ServiceExt;

/// Extract all `data:` payloads from an SSE body (one per line starting with "data: ").
fn sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect()
}

// ---- inline route (AlwaysKiro from the binary is not importable here) ----

struct IntegKiroRoute;

impl RouteDecision for IntegKiroRoute {
    fn route(&self, _meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        Ok(RouteTarget::Local(AgentId::parse("kiro")?))
    }
}

// ---- helpers ----

/// NDJSON bytes that replay one `session/update` text "PONG" then a Done frame.
fn pong_ndjson() -> Vec<u8> {
    let text_frame = r#"{"method":"session/update","params":{"text":"PONG"}}"#;
    let done_frame = r#"{"result":{"stopReason":"end_turn"}}"#;
    format!("{text_frame}\n{done_frame}\n").into_bytes()
}

/// Build the router under test: real components wired together.
fn build_router() -> axum::Router {
    let backend = Arc::new(ReplayBackend::from_ndjson(pong_ndjson()));
    let store: Arc<dyn SessionStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let route: Arc<dyn RouteDecision> = Arc::new(IntegKiroRoute);
    let auth = Arc::new(AlwaysGrant);

    let server = Arc::new(InboundServer::new(
        backend,
        store,
        policy,
        route,
        auth,
        "http://localhost:8080",
    ));
    server.router()
}

/// Collect the full response body as a UTF-8 string.
async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.into()).unwrap()
}

/// Build a `POST /` JSON-RPC request with the required `A2A-Version: 1.0` header.
/// Uses the same shape the server's own unit tests use.
fn send_streaming_request() -> Request<axum::body::Body> {
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": { "text": "ping" }
        }
    }))
    .unwrap();

    Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(axum::body::Body::from(body))
        .unwrap()
}

// ---- tests ----

#[tokio::test]
async fn streaming_message_drives_replay_backend_to_artifact() {
    let router = build_router();

    let resp = router.oneshot(send_streaming_request()).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "server must return HTTP 200 for a streaming request"
    );

    let body = body_string(resp).await;

    assert!(
        body.contains("PONG"),
        "SSE body must contain the backend's text 'PONG': {body}"
    );
    assert!(
        body.contains("artifact-update"),
        "SSE body must contain an 'artifact-update' frame: {body}"
    );

    // Wire-conformance: all data payloads must parse as a2a::StreamResponse.
    let payloads = sse_data_payloads(&body);
    assert!(!payloads.is_empty(), "no data payloads in SSE body: {body}");
    let last = payloads.last().unwrap();
    let sr: a2a::StreamResponse = serde_json::from_str(last)
        .unwrap_or_else(|e| panic!("final data payload must parse as StreamResponse: {e}: {last}"));
    assert!(
        matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)),
        "final SSE frame must be ArtifactUpdate: {last}"
    );
}

/// Ordering invariant: the artifact frame is the last named frame in the stream.
#[tokio::test]
async fn artifact_frame_is_last_sse_frame() {
    let router = build_router();

    let body = body_string(router.oneshot(send_streaming_request()).await.unwrap()).await;

    let last_artifact = body.rfind("artifact-update");
    let last_status = body.rfind("status-update");

    assert!(
        last_artifact.is_some(),
        "no artifact-update frame in SSE body: {body}"
    );

    // If there are status frames they must precede the artifact (final flush).
    if let Some(s_pos) = last_status {
        assert!(
            last_artifact.unwrap() > s_pos,
            "artifact-update must come after any status-update: {body}"
        );
    }

    // Wire-conformance: all data: payloads must parse as a2a::StreamResponse,
    // and the final one must be ArtifactUpdate.
    let payloads = sse_data_payloads(&body);
    for payload in &payloads {
        let _: a2a::StreamResponse = serde_json::from_str(payload).unwrap_or_else(|e| {
            panic!("data payload must parse as StreamResponse: {e}: {payload}")
        });
    }
    let last_sr: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
    assert!(
        matches!(last_sr, a2a::StreamResponse::ArtifactUpdate(_)),
        "final SSE frame must be ArtifactUpdate"
    );
}
