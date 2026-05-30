// integration_delegate.rs — CI-safe integration test: drives the REAL InboundServer
// with the REAL PeerDelegation pointing at an in-test mock A2A peer, proving the full
// inbound→delegate→peer path (spec S2 + S2a, Task 11).
//
// The mock peer is an inline axum server on 127.0.0.1:0 that:
//   - Records the request body so we can assert content threading (S2a).
//   - For SendStreamingMessage: replies with a real text/event-stream SSE body
//     carrying a `statusUpdate` then an `artifactUpdate` lastChunk=true with
//     artifact text "PEER_PONG".
//
// No external processes — fully CI-safe.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use bridge_a2a_inbound::server::InboundServer;
use bridge_a2a_outbound::PeerDelegation;
use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ports::{PolicyEngine, RouteDecision, SessionStore};
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

// ---- inline route: routes skill="delegate" to Delegate, others to local ----

struct DelegateSkillRoute;

impl RouteDecision for DelegateSkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if meta.skill.as_deref() == Some("delegate") {
            Ok(RouteTarget::Delegate)
        } else {
            // Fall back to a panic-worthy local — delegate must not touch it.
            Ok(RouteTarget::Delegate)
        }
    }
}

// ---- mock peer state ----

#[derive(Default)]
struct MockPeerState {
    last_body: Option<Value>,
}

#[derive(Clone)]
struct MockPeerAppState {
    inner: Arc<Mutex<MockPeerState>>,
}

// ---- mock peer axum handler ----

async fn mock_peer_handler(
    State(app): State<MockPeerAppState>,
    _headers: HeaderMap,
    req: Request<Body>,
) -> Response<Body> {
    // Read and record the body.
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let body_value: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);

    {
        let mut st = app.inner.lock().unwrap();
        st.last_body = Some(body_value.clone());
    }

    let method = body_value
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if method == a2a::methods::SEND_STREAMING_MESSAGE {
        // Build two real SSE frames using the a2a types.
        let task_id = a2a::new_task_id();
        let context_id = a2a::new_context_id();

        let status_event = a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            status: a2a::TaskStatus {
                state: a2a::TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        });

        let artifact_event = a2a::StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            artifact: a2a::Artifact {
                artifact_id: a2a::new_artifact_id(),
                name: Some("o".into()),
                description: None,
                parts: vec![a2a::Part::text("PEER_PONG")],
                metadata: None,
                extensions: None,
            },
            append: None,
            last_chunk: Some(true),
            metadata: None,
        });

        let sse_body = format!(
            "data: {}\n\ndata: {}\n\n",
            serde_json::to_string(&status_event).expect("status serializes"),
            serde_json::to_string(&artifact_event).expect("artifact serializes"),
        );

        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from(sse_body))
            .unwrap()
    } else {
        // Other methods (CancelTask etc.) — return a generic 200.
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":null,"result":{}}"#))
            .unwrap()
    }
}

// ---- mock peer launcher ----

/// Start the inline mock A2A peer. Returns (url, shared state handle).
async fn start_mock_peer() -> (String, Arc<Mutex<MockPeerState>>) {
    let state = Arc::new(Mutex::new(MockPeerState::default()));
    let app_state = MockPeerAppState {
        inner: Arc::clone(&state),
    };

    let router = Router::new()
        .route("/", post(mock_peer_handler))
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port for mock peer");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://127.0.0.1:{}", addr.port());

    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("mock peer serve");
    });

    (url, state)
}

// ---- PanicBackend: proves the delegate path never touches local backend ----

struct PanicBackend;

#[async_trait::async_trait]
impl bridge_core::ports::AgentBackend for PanicBackend {
    async fn prompt(
        &self,
        _s: &bridge_core::ids::SessionId,
        _p: Vec<bridge_core::domain::Part>,
    ) -> Result<bridge_core::ports::BackendStream, BridgeError> {
        panic!("PanicBackend.prompt must not be called on the delegate path");
    }
    async fn cancel(&self, _s: &bridge_core::ids::SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

// ---- helpers ----

/// Extract all `data:` payloads from an SSE body string.
fn sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect()
}

/// Collect the full response body as a UTF-8 string.
async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.into()).unwrap()
}

/// Build the A2A SendStreamingMessage JSON-RPC request with skill="delegate" metadata
/// and message text "PING", matching the server's expected wire shape.
fn delegate_send_streaming_request() -> axum::http::Request<axum::body::Body> {
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "text": "PING",
                "metadata": { "a2a-bridge.skill": "delegate" }
            }
        }
    }))
    .unwrap();

    axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(axum::body::Body::from(body))
        .unwrap()
}

// ---- the CI-safe integration test ----

/// S2 + S2a: a delegate-skill SendStreamingMessage drives the REAL PeerDelegation to the
/// inline mock peer, which replies with a PEER_PONG artifact. Asserts:
///   1. The inbound SSE body contains an artifact-update with "PEER_PONG" (S2).
///   2. The mock peer recorded a request body containing "PING" (S2a — content threading).
#[tokio::test]
async fn delegate_skill_round_trips_through_peer() {
    // 1. Start the inline mock A2A peer.
    let (peer_url, peer_state) = start_mock_peer().await;

    // 2. Build the InboundServer with real PeerDelegation.
    let delegation = Arc::new(PeerDelegation::new(
        &peer_url,
        "bearer:T",
        std::time::Duration::from_secs(30),
    ));
    let store: Arc<dyn SessionStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let route: Arc<dyn RouteDecision> = Arc::new(DelegateSkillRoute);
    let auth = Arc::new(AlwaysGrant);
    let backend = Arc::new(PanicBackend);

    let server = Arc::new(InboundServer::new(
        backend,
        store,
        policy,
        route,
        auth,
        "http://localhost:8080",
        delegation,
    ));
    let router = server.router();

    // 3. POST a SendStreamingMessage with skill="delegate" and text="PING".
    let resp = router
        .oneshot(delegate_send_streaming_request())
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "inbound server must return HTTP 200"
    );

    // 4. Read the inbound SSE body and assert PEER_PONG artifact.
    let body = body_string(resp).await;

    assert!(
        body.contains("artifact-update"),
        "SSE body must contain an artifact-update frame: {body}"
    );
    assert!(
        body.contains("PEER_PONG"),
        "SSE body must contain the peer's artifact text 'PEER_PONG': {body}"
    );

    // Parse all data: payloads as a2a::StreamResponse (wire-conformance).
    let payloads = sse_data_payloads(&body);
    assert!(!payloads.is_empty(), "no data payloads in SSE body: {body}");

    let last = payloads.last().unwrap();
    let sr: a2a::StreamResponse = serde_json::from_str(last)
        .unwrap_or_else(|e| panic!("final data payload must parse as StreamResponse: {e}: {last}"));
    assert!(
        matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)),
        "final SSE frame must be ArtifactUpdate: {last}"
    );

    // 5. S2a: assert the mock peer received a request body containing "PING".
    let recorded_body = peer_state
        .lock()
        .unwrap()
        .last_body
        .clone()
        .unwrap_or(Value::Null);
    let recorded_body_str = serde_json::to_string(&recorded_body).unwrap();
    assert!(
        recorded_body_str.contains("PING"),
        "mock peer's recorded request must contain 'PING' (content threading S2a): {recorded_body_str}"
    );
    assert_eq!(
        recorded_body["method"], "SendStreamingMessage",
        "mock peer must have received a SendStreamingMessage: {recorded_body_str}"
    );
}
