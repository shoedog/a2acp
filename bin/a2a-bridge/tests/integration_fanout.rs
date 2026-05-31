// integration_fanout.rs — CI-safe integration test: drives the REAL InboundServer
// through the full fan-out path (RouteTarget::Fanout), with:
//   - A real local ReplayBackend yielding a Kiro-side artifact ("KIRO_ART").
//   - An inline mock A2A peer (axum on 127.0.0.1:0) yielding a peer artifact ("PEER_ART").
//
// Proves spec S3.1/S3.2: fan-out merges both labeled artifacts and emits a terminal
// statusUpdate(Completed) as the final SSE frame.
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
use bridge_acp::replay::ReplayBackend;
use bridge_core::domain::{RouteTarget, TaskMeta};
use bridge_core::error::BridgeError;
use bridge_core::ports::{PolicyEngine, RouteDecision, SessionStore};
use bridge_policy::auth::AlwaysGrant;
use bridge_policy::permission::AutoPolicy;
use bridge_store::sqlite::SqliteStore;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower::ServiceExt;

// ---- SkillRoute: routes skill="fan-out" to Fanout, everything else to Delegate ----

struct FanoutSkillRoute;

impl RouteDecision for FanoutSkillRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if meta.skill.as_deref() == Some("fan-out") {
            Ok(RouteTarget::Fanout)
        } else {
            // Fall through to Fanout for all requests in these tests.
            Ok(RouteTarget::Fanout)
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
    /// If true, return HTTP 500 on SendStreamingMessage.
    fail: bool,
}

// ---- mock peer axum handler ----

async fn mock_peer_handler(
    State(app): State<MockPeerAppState>,
    _headers: HeaderMap,
    req: Request<Body>,
) -> Response<Body> {
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
        // Optionally fail to test degrade-to-survivor.
        if app.fail {
            return Response::builder()
                .status(500)
                .body(Body::from("internal server error"))
                .unwrap();
        }

        // Real SSE frames: statusUpdate(Working) + artifactUpdate("PEER_ART") + statusUpdate(Completed).
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
                name: Some("peer".into()),
                description: None,
                parts: vec![a2a::Part::text("PEER_ART")],
                metadata: None,
                extensions: None,
            },
            append: None,
            last_chunk: Some(true),
            metadata: None,
        });

        // Terminal statusUpdate(Completed) required by the outbound client model (Task 3).
        let completed_event = a2a::StreamResponse::StatusUpdate(a2a::TaskStatusUpdateEvent {
            task_id: task_id.clone(),
            context_id: context_id.clone(),
            status: a2a::TaskStatus {
                state: a2a::TaskState::Completed,
                message: None,
                timestamp: None,
            },
            metadata: None,
        });

        let sse_body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\n",
            serde_json::to_string(&status_event).expect("status serializes"),
            serde_json::to_string(&artifact_event).expect("artifact serializes"),
            serde_json::to_string(&completed_event).expect("completed serializes"),
        );

        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from(sse_body))
            .unwrap()
    } else {
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":null,"result":{}}"#))
            .unwrap()
    }
}

// ---- mock peer launcher ----

/// Start the inline mock A2A peer. Returns (url, shared state handle).
async fn start_mock_peer(fail: bool) -> (String, Arc<Mutex<MockPeerState>>) {
    let state = Arc::new(Mutex::new(MockPeerState::default()));
    let app_state = MockPeerAppState {
        inner: Arc::clone(&state),
        fail,
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

// ---- NDJSON for Kiro-side ReplayBackend ----

/// One `session/update` carrying "KIRO_ART" then a Done frame.
fn kiro_ndjson() -> Vec<u8> {
    let text_frame = r#"{"method":"session/update","params":{"text":"KIRO_ART"}}"#;
    let done_frame = r#"{"result":{"stopReason":"end_turn"}}"#;
    format!("{text_frame}\n{done_frame}\n").into_bytes()
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

/// Build the A2A SendStreamingMessage JSON-RPC request with skill="fan-out" metadata
/// and message text "PING".
fn fanout_send_streaming_request() -> Request<Body> {
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "SendStreamingMessage",
        "params": {
            "message": {
                "text": "PING",
                "metadata": { "a2a-bridge.skill": "fan-out" }
            }
        }
    }))
    .unwrap();

    Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header("A2A-Version", "1.0")
        .body(Body::from(body))
        .unwrap()
}

/// Build the InboundServer wired for fan-out:
///   - ReplayBackend yields KIRO_ART (local Kiro side)
///   - PeerDelegation -> mock peer (peer side)
///   - FanoutSkillRoute -> RouteTarget::Fanout
fn build_fanout_server(peer_url: &str) -> axum::Router {
    let backend = Arc::new(ReplayBackend::from_ndjson(kiro_ndjson()));
    let store: Arc<dyn SessionStore> = Arc::new(SqliteStore::open_in_memory().unwrap());
    let policy: Arc<dyn PolicyEngine> = Arc::new(AutoPolicy);
    let route: Arc<dyn RouteDecision> = Arc::new(FanoutSkillRoute);
    let auth = Arc::new(AlwaysGrant);
    let delegation = Arc::new(PeerDelegation::new(
        peer_url,
        "bearer:T",
        std::time::Duration::from_secs(30),
    ));

    let server = Arc::new(InboundServer::new(
        backend,
        store,
        policy,
        route,
        auth,
        "http://localhost:8080",
        delegation,
        "kiro",
    ));
    server.router()
}

// ---- CI-safe integration tests ----

/// S3.1/S3.2: the fan-out path through the REAL InboundServer merges:
///   - a source=kiro artifact (KIRO_ART from the ReplayBackend via the translator)
///   - a source=peer artifact (PEER_ART from the inline mock A2A peer)
///   - a terminal statusUpdate(Completed) as the LAST frame
#[tokio::test]
async fn fanout_merges_kiro_and_peer_with_terminal() {
    // 1. Start the inline mock A2A peer (happy path).
    let (peer_url, _peer_state) = start_mock_peer(false).await;

    // 2. Build the InboundServer with ReplayBackend + PeerDelegation + FanoutSkillRoute.
    let router = build_fanout_server(&peer_url);

    // 3. POST a SendStreamingMessage with skill="fan-out" and text="PING".
    let resp = router
        .oneshot(fanout_send_streaming_request())
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "InboundServer must return HTTP 200 for a fan-out streaming request"
    );

    // 4. Read and parse the SSE body.
    let body = body_string(resp).await;

    eprintln!("=== fan-out SSE body ===\n{body}\n========================");

    // 5a. Both artifacts must appear.
    assert!(
        body.contains("KIRO_ART"),
        "SSE body must contain the Kiro artifact text 'KIRO_ART': {body}"
    );
    assert!(
        body.contains("PEER_ART"),
        "SSE body must contain the peer artifact text 'PEER_ART': {body}"
    );

    // 5b. Parse every data: payload as a2a::StreamResponse (wire-conformance).
    let payloads = sse_data_payloads(&body);
    assert!(
        !payloads.is_empty(),
        "no data: payloads in SSE body: {body}"
    );

    let stream_responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("data payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // 5c. At least two ArtifactUpdate frames must be present (one per source).
    let artifact_frames: Vec<&a2a::StreamResponse> = stream_responses
        .iter()
        .filter(|sr| matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)))
        .collect();
    assert!(
        artifact_frames.len() >= 2,
        "SSE stream must contain at least two ArtifactUpdate frames (one per source); got {}: {body}",
        artifact_frames.len()
    );

    // 5d. Check that we see a source=kiro label and a source=peer label in the artifacts.
    let has_kiro_artifact = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::ArtifactUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("kiro")
        } else {
            false
        }
    });
    let has_peer_artifact = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::ArtifactUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("peer")
        } else {
            false
        }
    });
    assert!(
        has_kiro_artifact,
        "SSE stream must contain an ArtifactUpdate with metadata[a2a-bridge.source]==\"kiro\": {body}"
    );
    assert!(
        has_peer_artifact,
        "SSE stream must contain an ArtifactUpdate with metadata[a2a-bridge.source]==\"peer\": {body}"
    );

    // 5e. The LAST frame must be a terminal statusUpdate(Completed).
    let last = stream_responses.last().unwrap();
    assert!(
        matches!(
            last,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Completed
        ),
        "the LAST SSE frame must be a terminal statusUpdate(Completed): {}",
        payloads.last().unwrap()
    );
}

/// Degrade-to-survivor: when the peer returns HTTP 500, the kiro source succeeds
/// and the coordinator emits:
///   - a source=kiro artifact (KIRO_ART)
///   - a source=peer labeled error status frame
///   - a terminal statusUpdate(Completed) (one source survived)
#[tokio::test]
async fn fanout_degrades_when_peer_fails() {
    // 1. Start the mock peer in fail mode (returns HTTP 500).
    let (peer_url, _peer_state) = start_mock_peer(true).await;

    // 2. Build the InboundServer.
    let router = build_fanout_server(&peer_url);

    // 3. POST the fan-out request.
    let resp = router
        .oneshot(fanout_send_streaming_request())
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "InboundServer must return HTTP 200 even when peer fails (degrade-to-survivor)"
    );

    let body = body_string(resp).await;

    eprintln!(
        "=== fan-out (peer-fails) SSE body ===\n{body}\n======================================"
    );

    // 4. Kiro artifact must be present.
    assert!(
        body.contains("KIRO_ART"),
        "SSE body must contain the Kiro artifact text 'KIRO_ART' even when peer fails: {body}"
    );

    // 5. Parse every data: payload as a2a::StreamResponse (wire-conformance).
    let payloads = sse_data_payloads(&body);
    assert!(
        !payloads.is_empty(),
        "no data: payloads in SSE body: {body}"
    );

    let stream_responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("data payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // 6. Must have a source=kiro artifact.
    let has_kiro_artifact = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::ArtifactUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("kiro")
        } else {
            false
        }
    });
    assert!(
        has_kiro_artifact,
        "SSE stream must contain an ArtifactUpdate with source=kiro even when peer fails: {body}"
    );

    // 7. Must have a source=peer labeled error status frame (the degraded peer's error frame).
    let has_peer_error = stream_responses.iter().any(|sr| {
        if let a2a::StreamResponse::StatusUpdate(e) = sr {
            e.metadata
                .as_ref()
                .and_then(|m| m.get("a2a-bridge.source"))
                .and_then(|v| v.as_str())
                == Some("peer")
        } else {
            false
        }
    });
    assert!(
        has_peer_error,
        "SSE stream must contain a StatusUpdate with source=peer (peer error frame): {body}"
    );

    // 8. LAST frame must be a terminal statusUpdate(Completed) (one survivor = Completed).
    let last = stream_responses.last().unwrap();
    assert!(
        matches!(
            last,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Completed
        ),
        "the LAST SSE frame must be a terminal statusUpdate(Completed) (degrade-to-survivor): {}",
        payloads.last().unwrap()
    );
}
