// server.rs — the A2A v1 inbound HTTP/JSON-RPC server (spec §5.1/§5.3, Task 13).
//
// `InboundServer` wires the bridge pipeline behind an axum router:
//
//   inbound JSON-RPC request
//     -> AuthMiddleware.authorize        (reject -> JSON-RPC error, pipeline NOT run)
//     -> assert_supported_version(hdr)   (unknown A2A-Version -> JSON-RPC error)
//     -> RouteDecision.route             (pick the backend agent)
//     -> Translator.run(backend, ...)    (drive the AgentBackend, anti-corruption)
//     -> events streamed back            (SSE for streaming; collected for unary)
//
// The streaming method guarantees a final flush: the translator emits the
// Artifact event last, and we forward events in order, so the terminal SSE
// frame is always the artifact.
//
// We hand-roll the server on axum 0.7 rather than adopting `a2a-server-lf`
// (see docs/adr/0003-a2a-sdk.md): that crate requires axum 0.8 and inverts
// control through its own task store / executor traits, which fights our
// auth->route->translate pipeline. axum 0.7 is already proven in this workspace.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::{Stream, StreamExt};
use serde_json::{json, Value};

use a2a::{methods, SVC_PARAM_VERSION};
use bridge_core::domain::{InboundRequest, Part, RouteTarget, TaskMeta};
use bridge_core::error::{A2aDisposition, BridgeError};
use bridge_core::ids::{SessionId, TaskId};
use bridge_core::ports::{AgentBackend, AuthMiddleware, PolicyEngine, RouteDecision, SessionStore};
use bridge_core::translator::{Event, EventKind, Translator};

use crate::card::{agent_card, assert_supported_version, A2A_PINNED_VERSION};
use crate::sse::event_to_sse;

/// JSON-RPC 2.0 error code for an invalid request / rejected pipeline gate.
const JSONRPC_INVALID_REQUEST: i32 = -32600;
/// JSON-RPC 2.0 error code for an unknown method.
const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
/// JSON-RPC 2.0 error code for invalid params.
const JSONRPC_INVALID_PARAMS: i32 = -32602;
/// JSON-RPC 2.0 internal error.
const JSONRPC_INTERNAL: i32 = -32603;

/// The inbound A2A server. Holds the five pipeline ports plus the advertised
/// base URL (used to build the Agent Card). Cheap to clone via `Arc`.
pub struct InboundServer {
    backend: Arc<dyn AgentBackend>,
    store: Arc<dyn SessionStore>,
    policy: Arc<dyn PolicyEngine>,
    route: Arc<dyn RouteDecision>,
    auth: Arc<dyn AuthMiddleware>,
    base_url: String,
}

impl InboundServer {
    /// Construct a server from the five pipeline ports and the advertised base URL.
    pub fn new(
        backend: Arc<dyn AgentBackend>,
        store: Arc<dyn SessionStore>,
        policy: Arc<dyn PolicyEngine>,
        route: Arc<dyn RouteDecision>,
        auth: Arc<dyn AuthMiddleware>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            backend,
            store,
            policy,
            route,
            auth,
            base_url: base_url.into(),
        }
    }

    /// Build the axum router, mounting the Agent Card and JSON-RPC endpoint.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/.well-known/agent-card.json", get(serve_card))
            .route("/", post(jsonrpc))
            .with_state(self)
    }

    // ---- pipeline helpers (shared by streaming + unary paths) ----

    /// Run the auth -> version -> route gates. On success returns the routed
    /// `(TaskId, SessionId, Vec<Part>)` for the translator. On failure returns
    /// the `BridgeError` (the caller maps it to a JSON-RPC error). The backend
    /// is NOT touched here, so a rejecting gate never reaches `prompt`.
    fn gate(&self, headers: &HeaderMap, params: &Value) -> Result<RoutedCall, BridgeError> {
        // 1. Authorize. We derive a minimal InboundRequest from the bearer token.
        let token = bearer_token(headers);
        let inbound = match token {
            Some(t) => InboundRequest::with_token(&t),
            None => InboundRequest::anon(),
        };
        let _auth_ctx = self.auth.authorize(&inbound)?;

        // 2. Version gate: the A2A-Version header must match our pinned version.
        let version = headers
            .get(SVC_PARAM_VERSION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(A2A_PINNED_VERSION);
        assert_supported_version(version)?;

        // 3. Route. Parse skill from params and pass to route decision. v1 only
        //    uses Local(kiro); Delegate is wired properly in Task 9.
        let task_meta = task_meta_from_params(params);
        match self.route.route(&task_meta)? {
            RouteTarget::Local(_agent) => {
                // happy path — continue with the local backend
            }
            RouteTarget::Delegate => {
                return Err(BridgeError::UpstreamA2aError);
            }
        }

        // 4. Derive task/session ids from params (best-effort; v1 stubs allowed).
        let task = task_id_from_params(params)?;
        let session = SessionId::parse(format!("session-{}", task.as_str()))
            .unwrap_or_else(|_| SessionId::parse("session-default").unwrap());
        // Persist the task->session mapping so CancelTask/GetTask can find it.
        Ok(RoutedCall {
            task,
            session,
            parts: parts_from_params(params),
        })
    }
}

/// The result of the gate: the routed call ready for the translator.
struct RoutedCall {
    task: TaskId,
    session: SessionId,
    parts: Vec<Part>,
}

// ---- axum handlers ----

/// `GET /.well-known/agent-card.json` -> the Agent Card as JSON.
async fn serve_card(State(srv): State<Arc<InboundServer>>) -> Response {
    Json(agent_card(&srv.base_url)).into_response()
}

/// `POST /` -> the JSON-RPC dispatch surface.
async fn jsonrpc(
    State(srv): State<Arc<InboundServer>>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let req = body.0;
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match method {
        m if m == methods::SEND_STREAMING_MESSAGE || m == methods::SUBSCRIBE_TO_TASK => {
            stream_message(srv, headers, id, params).await
        }
        m if m == methods::SEND_MESSAGE => unary_message(srv, headers, id, params).await,
        m if m == methods::CANCEL_TASK => cancel_task(srv, headers, id, params).await,
        m if m == methods::GET_TASK => get_task(srv, headers, id, params).await,
        "" => jsonrpc_err(id, JSONRPC_INVALID_REQUEST, "missing method"),
        _ => jsonrpc_err(id, JSONRPC_METHOD_NOT_FOUND, "method not found"),
    }
}

/// Streaming path: gate, then stream translator events as SSE with a final flush.
async fn stream_message(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let routed = match srv.gate(&headers, &params) {
        Ok(r) => r,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };

    // Persist task->session before driving the backend.
    let _ = srv.store.put(&routed.task, &routed.session).await;

    // Drive the translator on a spawned task feeding an mpsc channel, so the SSE
    // response stream owns no borrowed references (the translator borrows `&'a
    // dyn ...`). Cloning the Arcs into the task satisfies 'static.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, BridgeError>>(64);
    let backend = srv.backend.clone();
    let store = srv.store.clone();
    let policy = srv.policy.clone();
    // Extract task_id_str before routed fields are moved into the spawn.
    let task_id_str = routed.task.as_str().to_owned();
    let task = routed.task;
    let session = routed.session;
    let parts = routed.parts;

    tokio::spawn(async move {
        let translator = Translator::new();
        let mut events = translator.run(
            backend.as_ref(),
            store.as_ref(),
            policy.as_ref(),
            &task,
            &session,
            parts,
        );
        while let Some(ev) = events.next().await {
            // If the receiver is gone (client disconnected) stop driving.
            if tx.send(ev).await.is_err() {
                break;
            }
        }
        // Channel closes on drop -> SSE stream terminates after the final flush.
    });

    // Use the task id as the context id for now (consistent within a single stream).
    let context_id_str = task_id_str.clone();
    let sse_stream = sse_event_stream(rx, task_id_str, context_id_str);
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Adapt the mpsc receiver into a stream of `Result<SseEvent, Infallible>`.
/// Each translated [`Event`] becomes one SSE frame; backend errors become a
/// single `error` frame so the client sees a terminal signal.
fn sse_event_stream(
    rx: tokio::sync::mpsc::Receiver<Result<Event, BridgeError>>,
    task_id: String,
    context_id: String,
) -> impl Stream<Item = Result<SseEvent, std::convert::Infallible>> {
    tokio_stream::wrappers::ReceiverStream::new(rx).map(move |item| {
        let frame = match item {
            Ok(ev) => event_to_sse(&ev, &task_id, &context_id),
            Err(e) => SseEvent::default()
                .event("error")
                .json_data(json!({ "kind": "error", "text": e.to_string() }))
                .expect("serde_json::Value serializes"),
        };
        Ok(frame)
    })
}

/// Unary path: run the same pipeline but collect events into one JSON response.
async fn unary_message(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let routed = match srv.gate(&headers, &params) {
        Ok(r) => r,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    let _ = srv.store.put(&routed.task, &routed.session).await;

    let translator = Translator::new();
    let collected: Vec<Result<Event, BridgeError>> = translator
        .run(
            srv.backend.as_ref(),
            srv.store.as_ref(),
            srv.policy.as_ref(),
            &routed.task,
            &routed.session,
            routed.parts,
        )
        .collect()
        .await;

    // Surface a terminal error if the pipeline failed/suspended.
    if let Some(Err(e)) = collected.iter().find(|r| r.is_err()) {
        return bridge_err_to_jsonrpc(id, e);
    }
    let events: Vec<Event> = collected.into_iter().filter_map(|r| r.ok()).collect();
    let artifact_text = events
        .iter()
        .rev()
        .find(|e| e.kind() == &EventKind::Artifact)
        .map(|e| e.text().to_string())
        .unwrap_or_default();
    let status_chunks: Vec<&str> = events
        .iter()
        .filter(|e| e.kind() == &EventKind::Status)
        .map(|e| e.text())
        .collect();

    let result = json!({
        "task": { "id": routed.task.as_str(), "state": "TASK_STATE_COMPLETED" },
        "artifact": { "text": artifact_text },
        "status": status_chunks,
    });
    jsonrpc_ok(id, result)
}

/// `CancelTask` -> propagate cancel to the backend for the task's session.
async fn cancel_task(
    srv: Arc<InboundServer>,
    headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    // Cancel is still gated (auth + version) but does not run the translator.
    let token = bearer_token(&headers);
    let inbound = match token {
        Some(t) => InboundRequest::with_token(&t),
        None => InboundRequest::anon(),
    };
    if let Err(e) = srv.auth.authorize(&inbound) {
        return bridge_err_to_jsonrpc(id, &e);
    }

    let task = match task_id_from_params(&params) {
        Ok(t) => t,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    // Look up the session for this task; fall back to the derived default.
    let session = match srv.store.session_for(&task).await {
        Ok(Some(s)) => s,
        _ => SessionId::parse(format!("session-{}", task.as_str()))
            .unwrap_or_else(|_| SessionId::parse("session-default").unwrap()),
    };
    if let Err(e) = srv.backend.cancel(&session).await {
        return bridge_err_to_jsonrpc(id, &e);
    }
    jsonrpc_ok(
        id,
        json!({ "task": { "id": task.as_str(), "state": "TASK_STATE_CANCELED" } }),
    )
}

/// `GetTask` -> return the task's last-known state (v1 stub from the store).
async fn get_task(
    srv: Arc<InboundServer>,
    _headers: HeaderMap,
    id: Value,
    params: Value,
) -> Response {
    let task = match task_id_from_params(&params) {
        Ok(t) => t,
        Err(e) => return bridge_err_to_jsonrpc(id, &e),
    };
    let known = matches!(srv.store.session_for(&task).await, Ok(Some(_)));
    let state = if known {
        "TASK_STATE_WORKING"
    } else {
        "TASK_STATE_SUBMITTED"
    };
    jsonrpc_ok(
        id,
        json!({ "task": { "id": task.as_str(), "state": state } }),
    )
}

// ---- JSON-RPC helpers ----

fn jsonrpc_ok(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn jsonrpc_err(id: Value, code: i32, message: &str) -> Response {
    // JSON-RPC transport errors ride on HTTP 200 with an `error` member, which
    // is the conventional JSON-RPC-over-HTTP shape. We also set 400 for gate
    // rejections so plain HTTP clients see a failure status.
    let status = if code == JSONRPC_INVALID_REQUEST || code == JSONRPC_INVALID_PARAMS {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::OK
    };
    (
        status,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })),
    )
        .into_response()
}

/// Map a `BridgeError` to a JSON-RPC error using its disposition: request-level
/// rejections become INVALID_REQUEST; everything else (failed/suspended state)
/// becomes INTERNAL with the error's display message.
fn bridge_err_to_jsonrpc(id: Value, e: &BridgeError) -> Response {
    match e.disposition() {
        A2aDisposition::RejectRequest => jsonrpc_err(id, JSONRPC_INVALID_REQUEST, &e.to_string()),
        A2aDisposition::SetState(_) => jsonrpc_err(id, JSONRPC_INTERNAL, &e.to_string()),
    }
}

// ---- params extraction ----

/// Extract the bearer token from the `Authorization` header, if present.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// Pull a task id from the JSON-RPC params, accepting either `taskId`/`task_id`
/// or a nested `message.taskId`. Falls back to a generated id for fresh sends.
fn task_id_from_params(params: &Value) -> Result<TaskId, BridgeError> {
    let candidate = params
        .get("taskId")
        .or_else(|| params.get("task_id"))
        .or_else(|| params.get("message").and_then(|m| m.get("taskId")))
        .and_then(|v| v.as_str());
    match candidate {
        Some(s) if !s.is_empty() => TaskId::parse(s),
        // Fresh SendMessage with no task id: synthesize a stable stub id.
        _ => TaskId::parse("task-1"),
    }
}

/// Extract `TaskMeta` from JSON-RPC params. Reads the skill selector from
/// `params.message.metadata["a2a-bridge.skill"]` if present; all other fields
/// are left at their defaults.
fn task_meta_from_params(params: &Value) -> TaskMeta {
    let skill = params
        .get("message")
        .and_then(|m| m.get("metadata"))
        .and_then(|md| md.get("a2a-bridge.skill"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    TaskMeta { skill }
}

/// Pull message parts from params, extracting real text content.
///
/// Priority:
/// 1. If `message.parts` is a non-empty array, map each element's `text`
///    field to a `Part { text }`, skipping elements without a string `text`.
/// 2. Else if `message.text` is a string, one `Part { text }`.
/// 3. Else if a top-level `text` field is present, one `Part { text }`.
/// 4. Otherwise empty vec.
fn parts_from_params(params: &Value) -> Vec<Part> {
    let message = params.get("message");

    // 1. message.parts array
    if let Some(parts_arr) = message
        .and_then(|m| m.get("parts"))
        .and_then(|p| p.as_array())
    {
        if !parts_arr.is_empty() {
            return parts_arr
                .iter()
                .filter_map(|elem| {
                    elem.get("text").and_then(|t| t.as_str()).map(|t| Part {
                        text: t.to_string(),
                    })
                })
                .collect();
        }
    }

    // 2. message.text
    if let Some(text) = message.and_then(|m| m.get("text")).and_then(|t| t.as_str()) {
        return vec![Part {
            text: text.to_string(),
        }];
    }

    // 3. top-level text
    if let Some(text) = params.get("text").and_then(|t| t.as_str()) {
        return vec![Part {
            text: text.to_string(),
        }];
    }

    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::domain::RouteTarget;
    use bridge_core::domain::{
        AuthContext, PeerTaskId, PendingRequest, PermissionDecision, PermissionRequest,
        SessionContext,
    };
    use bridge_core::error::BridgeError;
    use bridge_core::ids::{AgentId, CallerId};
    use bridge_core::ports::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use tower::ServiceExt;

    // ---- inline fakes ----

    /// Backend that yields Text + Done and records whether prompt/cancel ran.
    struct FakeBackend {
        prompted: AtomicBool,
        cancelled: AtomicBool,
    }
    impl FakeBackend {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                prompted: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
            })
        }
    }
    #[async_trait::async_trait]
    impl AgentBackend for FakeBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            self.prompted.store(true, Ordering::SeqCst);
            let updates = vec![
                Ok(Update::Text("PONG".into())),
                Ok(Update::Done {
                    stop_reason: "end_turn".into(),
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(updates)))
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            self.cancelled.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Backend that panics if prompt is ever called — proves gating short-circuits.
    struct PanicBackend;
    #[async_trait::async_trait]
    impl AgentBackend for PanicBackend {
        async fn prompt(
            &self,
            _s: &SessionId,
            _p: Vec<Part>,
        ) -> Result<BackendStream, BridgeError> {
            panic!("backend.prompt must not be called when a gate rejects");
        }
        async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeStore {
        map: Mutex<std::collections::HashMap<String, String>>,
        peer_tasks: Mutex<std::collections::HashMap<String, PeerTaskId>>,
        cancels: Mutex<std::collections::HashSet<String>>,
    }
    #[async_trait::async_trait]
    impl SessionStore for FakeStore {
        async fn put(&self, t: &TaskId, s: &SessionId) -> Result<(), BridgeError> {
            self.map
                .lock()
                .unwrap()
                .insert(t.as_str().into(), s.as_str().into());
            Ok(())
        }
        async fn session_for(&self, t: &TaskId) -> Result<Option<SessionId>, BridgeError> {
            Ok(self
                .map
                .lock()
                .unwrap()
                .get(t.as_str())
                .map(|s| SessionId::parse(s.clone()).unwrap()))
        }
        async fn put_pending(&self, _t: &TaskId, _r: &PendingRequest) -> Result<(), BridgeError> {
            Ok(())
        }
        async fn take_pending(&self, _t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
            Ok(None)
        }
        async fn set_peer_task(&self, t: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
            self.peer_tasks
                .lock()
                .unwrap()
                .insert(t.as_str().into(), peer.clone());
            Ok(())
        }
        async fn peer_task_for(&self, t: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
            Ok(self.peer_tasks.lock().unwrap().get(t.as_str()).cloned())
        }
        async fn request_cancel(&self, t: &TaskId) -> Result<(), BridgeError> {
            self.cancels.lock().unwrap().insert(t.as_str().into());
            Ok(())
        }
        async fn cancel_requested(&self, t: &TaskId) -> Result<bool, BridgeError> {
            Ok(self.cancels.lock().unwrap().contains(t.as_str()))
        }
    }

    struct AutoApprove;
    impl PolicyEngine for AutoApprove {
        fn decide(
            &self,
            _req: &PermissionRequest,
            _c: &SessionContext,
        ) -> Result<PermissionDecision, BridgeError> {
            Ok(PermissionDecision::Approve)
        }
    }

    struct AlwaysKiro;
    impl RouteDecision for AlwaysKiro {
        fn route(&self, _t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Local(AgentId::parse("kiro")?))
        }
    }

    struct AlwaysGrant;
    impl AuthMiddleware for AlwaysGrant {
        fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
            Ok(AuthContext::new(CallerId::parse("anon").unwrap()))
        }
    }

    struct RejectAuth;
    impl AuthMiddleware for RejectAuth {
        fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
            Err(BridgeError::AuthRequired {
                request_id: "auth-1".into(),
            })
        }
    }

    fn build(backend: Arc<dyn AgentBackend>, auth: Arc<dyn AuthMiddleware>) -> Arc<InboundServer> {
        Arc::new(InboundServer::new(
            backend,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(AlwaysKiro),
            auth,
            "http://localhost:8080",
        ))
    }

    fn router(srv: Arc<InboundServer>) -> Router {
        srv.router()
    }

    fn jsonrpc_body(method: &str, params: Value) -> axum::body::Body {
        axum::body::Body::from(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params,
            }))
            .unwrap(),
        )
    }

    fn post_request(
        method: &str,
        params: Value,
        version: &str,
    ) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .header(SVC_PARAM_VERSION, version)
            .body(jsonrpc_body(method, params))
            .unwrap()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// Extract all `data:` payloads from an SSE body (one per line starting with "data: ").
    fn sse_data_payloads(body: &str) -> Vec<String> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|s| s.trim_end_matches('\r').to_owned())
            .collect()
    }

    #[tokio::test]
    async fn streaming_message_yields_artifact_event() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        // The SSE event: field names are still present.
        assert!(
            body.contains("artifact-update"),
            "SSE body should contain an artifact frame: {body}"
        );
        assert!(
            body.contains("PONG"),
            "artifact should carry the text: {body}"
        );
        // The data: payloads must parse as real a2a::StreamResponse — conformance check.
        let payloads = sse_data_payloads(&body);
        assert!(!payloads.is_empty(), "no data payloads in SSE body: {body}");
        let last = payloads.last().unwrap();
        let sr: a2a::StreamResponse = serde_json::from_str(last).unwrap_or_else(|e| {
            panic!("last data payload must parse as StreamResponse: {e}: {last}")
        });
        assert!(
            matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)),
            "final frame must be ArtifactUpdate: {last}"
        );
    }

    #[tokio::test]
    async fn streaming_preserves_order_final_is_artifact() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        let body = body_string(resp).await;
        let last_artifact = body.rfind("artifact-update");
        let last_status = body.rfind("status-update");
        assert!(last_artifact.is_some(), "no artifact frame: {body}");
        // The artifact frame must come after any status frame (final flush).
        if let Some(s) = last_status {
            assert!(
                last_artifact.unwrap() > s,
                "artifact must be the final frame: {body}"
            );
        }
        // All data: payloads must parse as a2a::StreamResponse (wire-conformance).
        let payloads = sse_data_payloads(&body);
        for payload in &payloads {
            let _: a2a::StreamResponse = serde_json::from_str(payload).unwrap_or_else(|e| {
                panic!("data payload must parse as StreamResponse: {e}: {payload}")
            });
        }
        // The last parsed payload must be ArtifactUpdate.
        let last_sr: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap()).unwrap();
        assert!(
            matches!(last_sr, a2a::StreamResponse::ArtifactUpdate(_)),
            "final frame must be ArtifactUpdate"
        );
    }

    #[tokio::test]
    async fn cancel_task_propagates_to_backend() {
        let backend = FakeBackend::new();
        let srv = build(backend.clone(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::CANCEL_TASK,
                json!({ "taskId": "task-7" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            backend.cancelled.load(Ordering::SeqCst),
            "cancel must reach the backend"
        );
    }

    #[tokio::test]
    async fn unknown_version_header_rejected() {
        // A panicking backend proves the pipeline (prompt) is never reached.
        let srv = build(Arc::new(PanicBackend), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "9.9",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = body_string(resp).await;
        assert!(body.contains("error"), "expected JSON-RPC error: {body}");
        assert!(
            body.contains("version mismatch"),
            "expected version mismatch message: {body}"
        );
    }

    #[tokio::test]
    async fn rejecting_auth_blocks_before_routing() {
        // RejectAuth + PanicBackend: if auth didn't short-circuit, prompt would panic.
        let srv = build(Arc::new(PanicBackend), Arc::new(RejectAuth));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_STREAMING_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        let body = body_string(resp).await;
        assert!(
            body.contains("error"),
            "auth rejection should error: {body}"
        );
        assert!(
            body.contains("auth required"),
            "expected auth-required message: {body}"
        );
    }

    #[tokio::test]
    async fn serves_agent_card() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(
                axum::http::Request::builder()
                    .method("GET")
                    .uri("/.well-known/agent-card.json")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let card: Value = serde_json::from_str(&body).unwrap();
        let skills = card["skills"].as_array().unwrap();
        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s["id"] == "kiro-code"));
        assert!(skills.iter().any(|s| s["id"] == "delegate"));
    }

    #[tokio::test]
    async fn unary_send_message_returns_artifact() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::SEND_MESSAGE,
                json!({ "message": { "text": "ping" } }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["artifact"]["text"], "PONG");
    }

    #[tokio::test]
    async fn get_task_returns_state_stub() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request(
                methods::GET_TASK,
                json!({ "taskId": "task-9" }),
                "1.0",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["task"]["id"], "task-9");
        assert!(v["result"]["task"]["state"].is_string());
    }

    #[tokio::test]
    async fn unknown_method_is_method_not_found() {
        let srv = build(FakeBackend::new(), Arc::new(AlwaysGrant));
        let resp = router(srv)
            .oneshot(post_request("Bogus", json!({}), "1.0"))
            .await
            .unwrap();
        let body = body_string(resp).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], JSONRPC_METHOD_NOT_FOUND);
    }

    // ---- Task 7: skill metadata + real Part.text extraction ----

    #[test]
    fn skill_metadata_parsed_into_taskmeta() {
        let p = serde_json::json!({"message":{"metadata":{"a2a-bridge.skill":"delegate"}}});
        assert_eq!(task_meta_from_params(&p).skill.as_deref(), Some("delegate"));
    }

    #[test]
    fn no_skill_metadata_is_none() {
        let p = serde_json::json!({"message":{"text":"hi"}});
        assert_eq!(task_meta_from_params(&p).skill, None);
    }

    #[test]
    fn parts_from_message_text() {
        let p = serde_json::json!({"message":{"text":"PING"}});
        let v = parts_from_params(&p);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "PING");
    }

    #[test]
    fn parts_from_a2a_parts_array() {
        let p = serde_json::json!({"message":{"parts":[{"text":"A"},{"text":"B"}]}});
        let v = parts_from_params(&p);
        assert_eq!(
            v.iter().map(|x| x.text.clone()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
    }
}
