// testpeer.rs — in-test mock A2A peer server for bridge-a2a-outbound tests.
//
// Binds an axum HTTP server on 127.0.0.1:0 (ephemeral port).
// Accepts POST / with a JSON-RPC body. For SendStreamingMessage it replies with
// a scripted SSE stream. For CancelTask it records the task id.
//
// This module is `pub(crate)` under `#[cfg(test)]` and reused by Tasks 4/5.

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, StatusCode},
    response::Response,
    routing::post,
    Router,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Shared state between the handler and the handle
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PeerState {
    last_body: Option<Value>,
    last_auth: Option<String>,
    cancelled_tasks: Vec<String>,
}

// ---------------------------------------------------------------------------
// MockPeerHandle — test-side inspection
// ---------------------------------------------------------------------------

/// Handle returned from [`MockPeer::start`] for inspecting recorded requests.
pub struct MockPeerHandle {
    state: Arc<Mutex<PeerState>>,
}

impl MockPeerHandle {
    /// The last JSON-RPC request body that arrived, as a `serde_json::Value`.
    pub fn last_request_body(&self) -> Value {
        self.state
            .lock()
            .unwrap()
            .last_body
            .clone()
            .unwrap_or(Value::Null)
    }

    /// The raw value of the `Authorization` header from the last request.
    pub fn last_authorization(&self) -> Option<String> {
        self.state.lock().unwrap().last_auth.clone()
    }

    /// Returns `true` if a `CancelTask` request was received for `task_id`.
    pub fn received_cancel_for(&self, task_id: &str) -> bool {
        self.state
            .lock()
            .unwrap()
            .cancelled_tasks
            .iter()
            .any(|id| id == task_id)
    }
}

// ---------------------------------------------------------------------------
// MockPeer — starts the server
// ---------------------------------------------------------------------------

pub struct MockPeer;

impl MockPeer {
    /// Bind an axum server on an ephemeral port and return `(url, handle)`.
    ///
    /// `script` is the list of `data:` JSON payloads to emit as SSE frames
    /// (one per line), then the connection is closed.
    pub async fn start(script: Vec<String>) -> (String, MockPeerHandle) {
        let state = Arc::new(Mutex::new(PeerState::default()));
        let handle = MockPeerHandle {
            state: Arc::clone(&state),
        };

        let app_state = AppState {
            peer: Arc::clone(&state),
            script: Arc::new(script),
        };

        let app = Router::new()
            .route("/", post(handle_rpc))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://127.0.0.1:{}", addr.port());

        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock peer serve");
        });

        (url, handle)
    }
}

// ---------------------------------------------------------------------------
// Axum handler state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    peer: Arc<Mutex<PeerState>>,
    script: Arc<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Request handler
// ---------------------------------------------------------------------------

async fn handle_rpc(
    State(app): State<AppState>,
    headers: HeaderMap,
    req: Request<Body>,
) -> Response<Body> {
    // Collect body bytes
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .unwrap_or_default();

    // Parse JSON body
    let body_value: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);

    // Record auth header
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Determine the method
    let method = body_value
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Record state
    {
        let mut st = app.peer.lock().unwrap();
        st.last_body = Some(body_value.clone());
        st.last_auth = auth;

        if method == a2a::methods::CANCEL_TASK {
            // Extract task id from params.id
            if let Some(task_id) = body_value
                .get("params")
                .and_then(|p| p.get("id"))
                .and_then(|v| v.as_str())
            {
                st.cancelled_tasks.push(task_id.to_string());
            }
        }
    }

    if method == a2a::methods::SEND_STREAMING_MESSAGE {
        // Build SSE body from script
        build_sse_response(&app.script)
    } else {
        // Generic 200 OK for other methods (e.g. CancelTask)
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":null,"result":{}}"#))
            .unwrap()
    }
}

/// Build an SSE `Response<Body>` from the list of JSON payload strings.
fn build_sse_response(script: &[String]) -> Response<Body> {
    // Each SSE event: "data: <payload>\n\n"
    let mut sse_body = String::new();
    for payload in script {
        sse_body.push_str("data: ");
        sse_body.push_str(payload);
        sse_body.push_str("\n\n");
    }

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from(sse_body))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Script helpers — build scripted StreamResponse SSE payloads
// ---------------------------------------------------------------------------

/// A statusUpdate frame (TASK_STATE_WORKING) followed by an artifactUpdate
/// lastChunk=true frame carrying `text` as the artifact content.
///
/// These are real `a2a::StreamResponse` values serialized to JSON strings so
/// that they round-trip through `serde_json::from_str::<a2a::StreamResponse>`.
pub fn script_status_then_final(text: &str) -> Vec<String> {
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
            name: None,
            description: None,
            parts: vec![a2a::Part::text(text)],
            metadata: None,
            extensions: None,
        },
        append: None,
        last_chunk: Some(true),
        metadata: None,
    });

    vec![
        serde_json::to_string(&status_event).expect("status_event serializes"),
        serde_json::to_string(&artifact_event).expect("artifact_event serializes"),
    ]
}

/// A single artifactUpdate lastChunk=true frame (no status prefix).
/// Stub for Task 4 extension.
#[allow(dead_code)]
pub fn script_final_only(text: &str) -> Vec<String> {
    let task_id = a2a::new_task_id();
    let context_id = a2a::new_context_id();

    let artifact_event = a2a::StreamResponse::ArtifactUpdate(a2a::TaskArtifactUpdateEvent {
        task_id,
        context_id,
        artifact: a2a::Artifact {
            artifact_id: a2a::new_artifact_id(),
            name: None,
            description: None,
            parts: vec![a2a::Part::text(text)],
            metadata: None,
            extensions: None,
        },
        append: None,
        last_chunk: Some(true),
        metadata: None,
    });

    vec![serde_json::to_string(&artifact_event).expect("artifact_event serializes")]
}

// ---------------------------------------------------------------------------
// Tests for the mock peer itself
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_peer_records_body_and_auth() {
        let (url, peer) = MockPeer::start(script_status_then_final("hello")).await;

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "test-1",
            "method": "SendStreamingMessage",
            "params": {"message": {"messageId": "m1", "role": "ROLE_USER", "parts": [{"text": "hi"}]}}
        });

        let _resp = client
            .post(&url)
            .header("Authorization", "Bearer MYTOKEN")
            .json(&payload)
            .send()
            .await
            .unwrap();

        let body = peer.last_request_body();
        assert_eq!(body["method"], "SendStreamingMessage");
        assert!(peer.last_authorization().unwrap().contains("MYTOKEN"));
    }

    #[tokio::test]
    async fn mock_peer_cancel_recorded() {
        let (url, peer) = MockPeer::start(vec![]).await;

        let client = reqwest::Client::new();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "c1",
            "method": "CancelTask",
            "params": {"id": "task-abc"}
        });

        client.post(&url).json(&payload).send().await.unwrap();

        assert!(peer.received_cancel_for("task-abc"));
        assert!(!peer.received_cancel_for("other"));
    }

    #[test]
    fn script_status_then_final_round_trips() {
        let script = script_status_then_final("RESULT");
        assert_eq!(script.len(), 2);
        // Both must deserialize as StreamResponse
        for s in &script {
            let _parsed: a2a::StreamResponse =
                serde_json::from_str(s).expect("script payload must be valid StreamResponse");
        }
        // First is StatusUpdate, second is ArtifactUpdate with lastChunk=true
        let first: a2a::StreamResponse = serde_json::from_str(&script[0]).unwrap();
        assert!(matches!(first, a2a::StreamResponse::StatusUpdate(_)));
        let second: a2a::StreamResponse = serde_json::from_str(&script[1]).unwrap();
        if let a2a::StreamResponse::ArtifactUpdate(ev) = second {
            assert_eq!(ev.last_chunk, Some(true));
            assert_eq!(ev.artifact.parts[0].as_text(), Some("RESULT"));
        } else {
            panic!("expected ArtifactUpdate");
        }
    }
}
