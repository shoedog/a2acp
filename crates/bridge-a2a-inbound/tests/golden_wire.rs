// golden_wire.rs — this is the drift tripwire for a2a-lf bumps: the ACP twin
// (crates/bridge-acp/tests/golden_frames.rs + corpus_replay.rs) caught real
// wire drift at the ACP SDK's 1.x bump; this file is the A2A-inbound-side
// counterpart. Hand-authored to docs/superpowers/specs/2026-07-03-wave-3-cli-wire.md
// §W3-C.
//
// SCOPE / BRITTLENESS RULES (normative, from the spec — v2, adversarially
// reviewed): assert BRIDGE-OWNED wire semantics only. Concretely:
//   * OK   — field presence/types, stable enum VALUES (TASK_STATE_*), stable
//            JSON-RPC error codes/categories, the two response families below,
//            ordered (seq, kind) pairs for streaming reattach.
//   * NEVER — key order, generated-id FORMATS, optional-null presence, SSE
//            keepalive framing, full byte/object equality against a captured
//            payload.
// A harmless a2a-lf patch bump must not break these tests; a real regression
// in OUR wire contract must.
//
// TWO RESPONSE FAMILIES (verified against the running server, not assumed
// from the spec — server.rs is ground truth):
//   * LEGACY hand-built (server.rs `unary_message` ~L2610, `get_task`'s
//     session-heuristic fallback ~L3219): `{"task":{"id":..,"state":
//     "TASK_STATE_*"},"artifact":{"text":..},"status":[..]}` — `state` sits
//     FLAT on `task`; there is no nested `status` object.
//   * a2a::Task-TYPED (server.rs `get_task`'s durable-row branch ~L3190,
//     detached Workflow submit ~L2557): `{"task":{"id":..,"contextId":..,
//     "status":{"state":"TASK_STATE_*",..},"artifacts":[..]}}` — `state` is
//     nested under `status`; `task.state` (flat) is ABSENT.
// Each golden below states in its doc-comment which family it freezes.
//
// HARNESS: drives the axum router in-process via `srv.router().oneshot(...)`,
// exactly as `workflow_producer.rs` does (no sockets). The fake
// backend/registry/store scaffolding below follows that file's pattern
// (integration-test binaries in this crate don't share code across files, so
// it's duplicated rather than imported — see workflow_producer.rs for the
// canonical shape this was copied from).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tower::ServiceExt;

use a2a::{methods, SVC_PARAM_VERSION};
use bridge_a2a_inbound::server::InboundServer;
use bridge_core::domain::{
    AgentEntry, AgentKind, AuthContext, InboundRequest, Part, PeerTaskId, PermissionDecision,
    PermissionRequest, RegistrySnapshot, RouteTarget, SessionContext, SessionSpec, TaskMeta,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, NodeId, OperationId, SessionId, TaskId, WorkflowId};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, AuthMiddleware, BackendStream, Delegation, DelegationPort, Lease,
    PolicyEngine, Resolved, RouteDecision, SessionStore, Update,
};
use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};

const VERSION: &str = "1.0";

/// Fixed dummy timestamp — no test asserts on timestamp values, only ordering
/// via the store's own seq allocation.
fn now_ms() -> i64 {
    1_700_000_000_000
}

// ============================================================================
// harness (pattern copied from workflow_producer.rs)
// ============================================================================

struct NoopLease;
impl Lease for NoopLease {}

fn minimal_entry(id: &AgentId) -> AgentEntry {
    AgentEntry {
        id: id.clone(),
        cmd: Some("fake".into()),
        base_url: None,
        api_key_env: None,
        args: vec![],
        kind: AgentKind::Acp,
        model_provider: None,
        model: None,
        effort: None,
        mode: None,
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        auth_method: None,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        mcp: vec![],
        mcp_delivery: Default::default(),
        extensions: Default::default(),
    }
}

/// A backend that replies with a fixed text then completes normally.
struct FakeBackend {
    reply: String,
}
#[async_trait]
impl AgentBackend for FakeBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        let updates = vec![
            Ok(Update::Text(self.reply.clone())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
            }),
        ];
        Ok(Box::pin(tokio_stream::iter(updates)))
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// A backend whose `prompt()` fails with `AgentCrashed`, carrying a reason
/// that embeds infra detail (a URL with a token) — the same shape the
/// wire-leak guard (bridge-core/src/error.rs) exists to protect against.
struct AgentCrashedBackend {
    reason: String,
}
#[async_trait]
impl AgentBackend for AgentCrashedBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        Err(BridgeError::AgentCrashed {
            reason: self.reason.clone(),
        })
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// A backend whose `configure_session()` fails with `ConfigInvalid`, carrying
/// a reason that embeds a filesystem path — same wire-leak shape as above,
/// but through the OTHER redacted variant.
struct ConfigInvalidBackend {
    reason: String,
}
#[async_trait]
impl AgentBackend for ConfigInvalidBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        // Should never be reached: configure_session fails first. If this DOES
        // run (an assumption in this file broke), fail the turn normally so the
        // test reports a clear shape mismatch instead of an opaque panic.
        let updates = vec![Ok(Update::Done {
            stop_reason: "end_turn".into(),
        })];
        Ok(Box::pin(tokio_stream::iter(updates)))
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn configure_session(
        &self,
        _session: &SessionId,
        _spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        Err(BridgeError::ConfigInvalid {
            reason: self.reason.clone(),
        })
    }
}

/// Agent id -> backend dispatch (mirrors workflow_producer.rs's `PerAgentRegistry`).
struct PerAgentRegistry {
    backends: HashMap<String, Arc<dyn AgentBackend>>,
    default: AgentId,
}
#[async_trait]
impl AgentRegistry for PerAgentRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let backend = self
            .backends
            .get(id.as_str())
            .cloned()
            .ok_or(BridgeError::UnknownAgent {
                id: id.as_str().into(),
            })?;
        Ok(Resolved {
            entry: Arc::new(minimal_entry(id)),
            backend,
            lease: Box::new(NoopLease),
        })
    }
    fn default_id(&self) -> AgentId {
        self.default.clone()
    }
    async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
        Ok(())
    }
    fn list(&self) -> Vec<AgentId> {
        self.backends
            .keys()
            .map(|k| AgentId::parse(k).unwrap())
            .collect()
    }
}

#[derive(Default)]
struct FakeStore {
    map: std::sync::Mutex<HashMap<String, String>>,
}
#[async_trait]
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
    async fn set_peer_task(&self, _t: &TaskId, _peer: &PeerTaskId) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn peer_task_for(&self, _t: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
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
        _req: &PermissionRequest,
        _c: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

struct AlwaysGrant;
impl AuthMiddleware for AlwaysGrant {
    fn authorize(&self, _req: &InboundRequest) -> Result<AuthContext, BridgeError> {
        Ok(AuthContext::new(
            bridge_core::ids::CallerId::parse("anon").unwrap(),
        ))
    }
}

struct NoDelegation;
#[async_trait]
impl DelegationPort for NoDelegation {
    async fn delegate(
        &self,
        _auth: &AuthContext,
        _local: &TaskId,
        _parts: Vec<Part>,
    ) -> Result<Delegation, BridgeError> {
        Err(BridgeError::UpstreamA2aError)
    }
    async fn cancel(&self, _peer_task: &PeerTaskId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Routes by `a2a-bridge.agent` metadata (falling back to `default`), EXCEPT
/// `skill == "review"` routes to a fixed Workflow id. No workflow GRAPH is
/// ever registered for that id: `gate()`'s task-spec validation (H1.5) runs
/// BEFORE workflow dispatch even looks the graph up, so the
/// `TaskSpecInvalid` golden never needs a real graph to prove its rejection.
struct AgentOrWorkflowRoute {
    default: AgentId,
}
impl RouteDecision for AgentOrWorkflowRoute {
    fn route(&self, t: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if t.skill.as_deref() == Some("review") {
            return Ok(RouteTarget::Workflow(WorkflowId::parse("review")?));
        }
        Ok(RouteTarget::Local(
            t.agent.clone().unwrap_or_else(|| self.default.clone()),
        ))
    }
}

fn build_server_ex(
    backends: HashMap<String, Arc<dyn AgentBackend>>,
    store: Arc<FakeStore>,
    task_store: Arc<dyn TaskStore>,
) -> Arc<InboundServer> {
    let registry = Arc::new(PerAgentRegistry {
        backends,
        default: AgentId::parse("codex").unwrap(),
    });
    Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            store as Arc<dyn SessionStore>,
            Arc::new(AutoApprove),
            Arc::new(AgentOrWorkflowRoute {
                default: AgentId::parse("codex").unwrap(),
            }),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_task_store(task_store),
    )
}

fn build_server(backends: HashMap<String, Arc<dyn AgentBackend>>) -> Arc<InboundServer> {
    build_server_ex(
        backends,
        Arc::new(FakeStore::default()),
        Arc::new(MemoryTaskStore::new()),
    )
}

fn one_backend(
    agent: &str,
    backend: Arc<dyn AgentBackend>,
) -> HashMap<String, Arc<dyn AgentBackend>> {
    HashMap::from([(agent.to_string(), backend)])
}

// ---- request/response plumbing ----

fn jsonrpc_body(method: &str, id: i64, params: Value) -> axum::body::Body {
    axum::body::Body::from(
        serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .unwrap(),
    )
}

fn post_request(method: &str, params: Value) -> axum::http::Request<axum::body::Body> {
    post_request_with_id(method, 1, params)
}

fn post_request_with_id(
    method: &str,
    id: i64,
    params: Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(SVC_PARAM_VERSION, VERSION)
        .body(jsonrpc_body(method, id, params))
        .unwrap()
}

fn post_request_with_cursor(
    method: &str,
    params: Value,
    cursor: i64,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(SVC_PARAM_VERSION, VERSION)
        .header("Last-Event-ID", cursor.to_string())
        .body(jsonrpc_body(method, 1, params))
        .unwrap()
}

/// POST an already-fully-formed JSON-RPC request body verbatim (used by the
/// corpus replay tests, which POST a captured/reconstructed request AS-IS).
fn post_raw(body: &Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(SVC_PARAM_VERSION, VERSION)
        .body(axum::body::Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn json_response(resp: axum::response::Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn sse_body(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Extract every `data:` payload from an SSE body, in wire order.
fn sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect()
}

/// Parse an SSE body into an ordered `Vec<(seq, kind)>` — mirrors the
/// `collect_sse_frames` helper in server.rs's own internal tests (Task 8),
/// which is `mod tests`-private and so not reachable from this external
/// integration-test crate.
fn sse_frames(body: &str) -> Vec<(i64, String)> {
    let mut result = Vec::new();
    for block in body.split("\n\n") {
        let mut seq: Option<i64> = None;
        let mut kind: Option<String> = None;
        for line in block.lines() {
            if let Some(id_str) = line.strip_prefix("id:") {
                seq = id_str.trim().parse().ok();
            } else if let Some(data_str) = line.strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data_str.trim()) {
                    if let Some(k) = v.get("kind").and_then(Value::as_str) {
                        kind = Some(k.to_string());
                    }
                }
            }
        }
        if let (Some(s), Some(k)) = (seq, kind) {
            result.push((s, k));
        }
    }
    result
}

fn task_record(id: &str, status: TaskRecordStatus, result: Option<&str>) -> TaskRecord {
    TaskRecord {
        id: TaskId::parse(id).unwrap(),
        workflow: "review".to_string(),
        status,
        result: result.map(|s| s.to_string()),
        error: None,
        created_ms: now_ms(),
        updated_ms: now_ms(),
        input: "input text".to_string(),
        workflow_spec_json: None,
        resume_attempts: 0,
        session_cwd: None,
        batch_id: None,
        item_id: None,
    }
}

async fn seed_working_record(store: &Arc<MemoryTaskStore>, id: &str) -> TaskId {
    let tid = TaskId::parse(id).unwrap();
    store
        .create(&task_record(id, TaskRecordStatus::Working, None))
        .await
        .unwrap();
    tid
}

fn operation_id_for(task: &TaskId) -> OperationId {
    OperationId::parse(format!("op-{}", task.as_str())).unwrap()
}

// ============================================================================
// 1. SendMessage success envelope — LEGACY hand-built family.
// ============================================================================

/// Freezes the LEGACY unary-local success envelope (server.rs `unary_message`,
/// ~L2610): `task.id`/`task.state` are FLAT (no nested `status`), `artifact.text`
/// carries the turn's output, and `status` is the array of coalesced text
/// chunks. This is the family the wave-1 live-gate corpus below also exercises.
#[tokio::test]
async fn send_message_success_envelope_is_legacy_flat_family() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "PONG".into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "ping",
                "metadata": { "a2a-bridge.agent": "codex" }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = json_response(resp).await;
    let task = &body["result"]["task"];

    // field presence + types (never assert the generated-id FORMAT itself).
    assert!(task["id"].is_string(), "task.id must be a string: {body}");
    assert_eq!(
        task["state"], "TASK_STATE_COMPLETED",
        "task.state must be the flat TASK_STATE_COMPLETED value: {body}"
    );
    // The defining shape difference vs the a2a::Task-typed family: NO nested
    // `status` object — `state` lives directly on `task`.
    assert!(
        task.get("status").is_none(),
        "legacy family must NOT nest state under task.status: {body}"
    );

    assert_eq!(
        body["result"]["artifact"]["text"], "PONG",
        "artifact.text must carry the turn output: {body}"
    );
    let status = body["result"]["status"]
        .as_array()
        .unwrap_or_else(|| panic!("status must be an array: {body}"));
    assert!(
        status.iter().all(Value::is_string),
        "every status chunk must be a string: {body}"
    );
    assert!(
        status.iter().any(|v| v == "PONG"),
        "status must include the coalesced chunk: {body}"
    );
}

// ============================================================================
// 2/3. GetTask envelope for a known task — BOTH families.
// ============================================================================

/// Freezes the a2a::Task-TYPED family for GetTask's DURABLE-row branch
/// (server.rs `get_task`, ~L3190): `task.contextId` is present, `state` is
/// nested under `task.status.state`, top-level `task.state` is ABSENT, and the
/// stored result surfaces as `task.artifacts[0].parts[0].text`.
#[tokio::test]
async fn get_task_durable_row_is_typed_nested_family() {
    let task_store = Arc::new(MemoryTaskStore::new());
    task_store
        .create(&task_record(
            "durable-1",
            TaskRecordStatus::Completed,
            Some("OUTPUT"),
        ))
        .await
        .unwrap();

    let srv = build_server_ex(HashMap::new(), Arc::new(FakeStore::default()), task_store);
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::GET_TASK,
            json!({ "id": "durable-1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = json_response(resp).await;
    let task = &body["result"]["task"];

    assert_eq!(task["id"], "durable-1", "{body}");
    assert!(
        task["contextId"].is_string(),
        "typed family carries contextId: {body}"
    );
    assert!(
        task.get("state").is_none(),
        "typed family must NOT carry a flat task.state: {body}"
    );
    assert_eq!(
        task["status"]["state"], "TASK_STATE_COMPLETED",
        "state must be nested under status: {body}"
    );
    assert_eq!(
        task["artifacts"][0]["parts"][0]["text"], "OUTPUT",
        "the stored result must surface as the first artifact's text part: {body}"
    );
}

/// Freezes the LEGACY flat family for GetTask's session-mapping HEURISTIC
/// fallback (server.rs `get_task`, ~L3212-3222): used when no durable task_store
/// row exists but the (ephemeral) SessionStore already has a mapping for the
/// task — `task.state` is flat, and neither `status` nor `contextId` appear.
#[tokio::test]
async fn get_task_session_heuristic_fallback_is_legacy_flat_family() {
    let store = Arc::new(FakeStore::default());
    // No task_store row: the durable branch falls through to the heuristic.
    let srv = build_server_ex(
        HashMap::new(),
        store.clone(),
        Arc::new(MemoryTaskStore::new()),
    );
    store
        .put(
            &TaskId::parse("legacy-1").unwrap(),
            &SessionId::parse("session-legacy-1").unwrap(),
        )
        .await
        .unwrap();

    let resp = srv
        .router()
        .oneshot(post_request(methods::GET_TASK, json!({ "id": "legacy-1" })))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = json_response(resp).await;
    let task = &body["result"]["task"];

    assert_eq!(task["id"], "legacy-1", "{body}");
    assert_eq!(
        task["state"], "TASK_STATE_WORKING",
        "a known (session-mapped) task with no durable row heuristically reports WORKING: {body}"
    );
    assert!(
        task.get("status").is_none(),
        "legacy family must not carry a nested status object: {body}"
    );
    assert!(
        task.get("contextId").is_none(),
        "legacy family must not carry contextId: {body}"
    );
}

// ============================================================================
// 4/5. SubscribeToTask snapshot frame shape — flattened `kind`, seq cursor.
// ============================================================================

/// Freezes the terminal-task SubscribeToTask SSE contract (server.rs
/// `terminal_sse_response`): frames are delivered in strict seq order — each
/// node's `node_finished`, then `snapshot_complete`, then `terminal` — and the
/// `kind` discriminator is FLATTENED onto the frame object (reattach's
/// `WorkflowProgressFrame` has `#[serde(flatten)] kind: FrameKind`), so sibling
/// fields like `node`/`ok`/`output` sit alongside `kind` at the top level
/// rather than nested under it.
#[tokio::test]
async fn subscribe_terminal_snapshot_orders_frames_and_flattens_kind() {
    let task_store = Arc::new(MemoryTaskStore::new());
    let task = seed_working_record(&task_store, "sub-term-1").await;
    let node_a = NodeId::parse("node-a").unwrap();
    let node_b = NodeId::parse("node-b").unwrap();
    let op = operation_id_for(&task);

    let s1 = task_store
        .put_node_checkpoint_sequenced(&task, &node_a, &op, "out-a", true, now_ms(), None)
        .await
        .unwrap();
    let s2 = task_store
        .put_node_checkpoint_sequenced(&task, &node_b, &op, "out-b", true, now_ms(), None)
        .await
        .unwrap();
    let s3 = task_store
        .set_terminal_sequenced(
            &task,
            &op,
            TaskRecordStatus::Completed,
            Some("done"),
            None,
            now_ms(),
        )
        .await
        .unwrap();
    assert_eq!((s1, s2, s3), (1, 2, 3));

    let srv = build_server_ex(HashMap::new(), Arc::new(FakeStore::default()), task_store);
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SUBSCRIBE_TO_TASK,
            json!({ "id": "sub-term-1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = sse_body(resp).await;

    let frames = sse_frames(&body);
    assert_eq!(
        frames,
        vec![
            (1, "node_finished".to_string()),
            (2, "node_finished".to_string()),
            (2, "snapshot_complete".to_string()),
            (3, "terminal".to_string()),
        ],
        "expected ordered snapshot+terminal frames: {body}"
    );

    // `kind` is flattened: a sibling field (`node`) sits at the SAME level, not
    // nested under a wrapper key.
    assert!(
        body.contains("\"kind\":\"node_finished\"") && body.contains("\"node\":\"node-a\""),
        "kind must be a flattened top-level discriminator, not a nested wrapper: {body}"
    );
}

/// Freezes the Last-Event-ID cursor semantics (server.rs I2): `Some(K)` filters
/// to only frames with `seq > K` — mirrors the existing internal cursor test
/// (server.rs `subscribe_terminal_task_cursor_1_filters_seq_gt_1`) rather than
/// over-freezing new behavior.
#[tokio::test]
async fn subscribe_terminal_cursor_filters_seq_gt_cursor() {
    let task_store = Arc::new(MemoryTaskStore::new());
    let task = seed_working_record(&task_store, "sub-cursor-1").await;
    let node_a = NodeId::parse("node-a").unwrap();
    let node_b = NodeId::parse("node-b").unwrap();
    let op = operation_id_for(&task);

    task_store
        .put_node_checkpoint_sequenced(&task, &node_a, &op, "out-a", true, now_ms(), None)
        .await
        .unwrap(); // seq=1
    task_store
        .put_node_checkpoint_sequenced(&task, &node_b, &op, "out-b", true, now_ms(), None)
        .await
        .unwrap(); // seq=2
    task_store
        .set_terminal_sequenced(
            &task,
            &op,
            TaskRecordStatus::Completed,
            Some("done"),
            None,
            now_ms(),
        )
        .await
        .unwrap(); // seq=3

    let srv = build_server_ex(HashMap::new(), Arc::new(FakeStore::default()), task_store);
    let resp = srv
        .router()
        .oneshot(post_request_with_cursor(
            methods::SUBSCRIBE_TO_TASK,
            json!({ "id": "sub-cursor-1" }),
            1, // Last-Event-ID: 1 -> only seq > 1
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = sse_body(resp).await;
    let frames = sse_frames(&body);
    assert_eq!(
        frames,
        vec![
            (2, "node_finished".to_string()),
            (2, "snapshot_complete".to_string()),
            (3, "terminal".to_string()),
        ],
        "cursor=1 must filter out the seq=1 frame: {body}"
    );
}

// ============================================================================
// 6-9. Error contracts — stable codes/categories.
// ============================================================================

/// Freezes the empty-message reject: a message with no extractable text content
/// is rejected BEFORE routing (H1), as a client-caused JSON-RPC INVALID_REQUEST.
#[tokio::test]
async fn send_message_empty_text_is_invalid_request() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "unused".into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": { "metadata": { "a2a-bridge.agent": "codex" } } }),
        ))
        .await
        .unwrap();
    let body = json_response(resp).await;
    assert_eq!(body["error"]["code"], -32600, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("no text content"),
        "message must explain the missing text: {body}"
    );
}

/// Freezes the unary unknown-agent contract: routing succeeds (the id parses),
/// but registry resolution fails — this is an INTERNAL disposition (-32603),
/// and — UNLIKE AgentCrashed/ConfigInvalid — `UnknownAgent`'s message is NOT
/// redacted (error.rs `client_message()`: only `AgentCrashed`/`ConfigInvalid`
/// collapse to a static category; `UnknownAgent` keeps its helpful Display).
#[tokio::test]
async fn send_message_unknown_agent_unary_is_internal_and_unredacted() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "unused".into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "hi",
                "metadata": { "a2a-bridge.agent": "nope-agent" }
            }}),
        ))
        .await
        .unwrap();
    let body = json_response(resp).await;
    assert_eq!(body["error"]["code"], -32603, "{body}");
    assert_eq!(
        body["error"]["message"], "unknown agent: nope-agent",
        "the agent id must reach the wire verbatim (this variant is NOT redacted): {body}"
    );
}

/// Freezes the STREAMING unknown-agent terminal-failure shape: the HTTP
/// response still commits to a 200 SSE stream (server.rs: "streaming has
/// already committed to an SSE response"), so the failure surfaces as an
/// inline `event: error` frame (`{"kind":"error","text":<client_message>}`)
/// followed by a terminal `statusUpdate` frame with state Failed — never a
/// top-level JSON-RPC error.
#[tokio::test]
async fn send_streaming_message_unknown_agent_is_terminal_failure_frame() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "unused".into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_STREAMING_MESSAGE,
            json!({ "message": {
                "text": "hi",
                "metadata": { "a2a-bridge.agent": "nope-agent" }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let body = sse_body(resp).await;
    let payloads = sse_data_payloads(&body);
    assert!(!payloads.is_empty(), "no SSE data payloads: {body}");

    let error_frame = payloads
        .iter()
        .find_map(|p| serde_json::from_str::<Value>(p).ok())
        .filter(|v| v.get("kind").and_then(Value::as_str) == Some("error"))
        .unwrap_or_else(|| panic!("expected an error-kind SSE frame: {body}"));
    assert_eq!(
        error_frame["text"], "unknown agent: nope-agent",
        "the error frame's text must carry the unredacted agent id: {body}"
    );

    let last: a2a::StreamResponse = serde_json::from_str(payloads.last().unwrap())
        .unwrap_or_else(|e| panic!("final payload must parse as StreamResponse: {e}: {body}"));
    assert!(
        matches!(last, a2a::StreamResponse::StatusUpdate(e) if e.status.state == a2a::TaskState::Failed),
        "the stream must end with a terminal Failed statusUpdate: {body}"
    );
}

/// Freezes the invalid-effort contract: an unrecognised `a2a-bridge.effort`
/// value is rejected during metadata parsing (before routing), as a
/// client-caused INVALID_REQUEST.
#[tokio::test]
async fn send_message_invalid_effort_is_invalid_request() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "unused".into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "hi",
                "metadata": { "a2a-bridge.agent": "codex", "a2a-bridge.effort": "bogus" }
            }}),
        ))
        .await
        .unwrap();
    let body = json_response(resp).await;
    assert_eq!(body["error"]["code"], -32600, "{body}");
    let msg = body["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("effort"),
        "message must point at the offending field: {body}"
    );
}

/// Freezes the `TaskSpecInvalid` contract: it REJECTS the request (-32600,
/// same client-caused category as the other InvalidRequest-shaped errors)
/// but — unlike AgentCrashed/ConfigInvalid — reaches the wire UNREDACTED. The
/// expected text is computed by calling the SAME production function the
/// server calls (`bridge_core::task_spec::validate_input`), so this asserts
/// exact passthrough without hand-duplicating (and thus overfitting on) the
/// hint wording.
#[tokio::test]
async fn send_message_task_spec_invalid_reaches_wire_unredacted() {
    let srv = build_server(one_backend(
        "codex",
        Arc::new(FakeBackend {
            reply: "unused".into(),
        }),
    ));
    let text = "no front matter here, just plain prose";
    let expected = match bridge_core::task_spec::validate_input(text).unwrap_err() {
        BridgeError::TaskSpecInvalid { message } => message,
        other => panic!("expected TaskSpecInvalid from the fixture text, got {other:?}"),
    };

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": text,
                "metadata": { "a2a-bridge.skill": "review" }
            }}),
        ))
        .await
        .unwrap();
    let body = json_response(resp).await;
    assert_eq!(body["error"]["code"], -32600, "{body}");
    assert_eq!(
        body["error"]["message"], expected,
        "TaskSpecInvalid must reach the wire verbatim (unredacted): {body}"
    );
    // Sanity: it is NOT one of the redacted static categories.
    assert_ne!(body["error"]["message"], "agent crashed");
    assert_ne!(body["error"]["message"], "invalid config");
}

/// Freezes the internal-failure redaction boundary for `AgentCrashed`: the
/// wire gets ONLY the static literal `"agent crashed"` at JSON-RPC -32603 —
/// never the underlying reason (which here embeds a URL with a token).
#[tokio::test]
async fn send_message_agent_crashed_is_redacted_to_static_string() {
    let leaky_reason = "spawn failed: https://internal.example/leak?token=SECRET-TOKEN-XYZ";
    let srv = build_server(one_backend(
        "codex",
        Arc::new(AgentCrashedBackend {
            reason: leaky_reason.into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "hi",
                "metadata": { "a2a-bridge.agent": "codex" }
            }}),
        ))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let body: Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(body["error"]["code"], -32603, "{body}");
    assert_eq!(
        body["error"]["message"], "agent crashed",
        "the wire must get ONLY the static category, never `{{e}}`: {body}"
    );
    assert!(
        !raw.contains("SECRET-TOKEN-XYZ") && !raw.contains("internal.example"),
        "the leaky reason must NEVER reach the wire: {raw}"
    );
}

/// Freezes the internal-failure redaction boundary for `ConfigInvalid`: same
/// contract as AgentCrashed, through the OTHER redacted variant — the wire
/// gets ONLY `"invalid config"`, never the reason (here a filesystem path).
#[tokio::test]
async fn send_message_config_invalid_is_redacted_to_static_string() {
    let leaky_reason = "model \"bogus\" not permitted per /Users/alice/.secret/allowlist.toml";
    let srv = build_server(one_backend(
        "codex",
        Arc::new(ConfigInvalidBackend {
            reason: leaky_reason.into(),
        }),
    ));
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "hi",
                "metadata": { "a2a-bridge.agent": "codex" }
            }}),
        ))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let raw = String::from_utf8(bytes.to_vec()).unwrap();
    let body: Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(body["error"]["code"], -32603, "{body}");
    assert_eq!(
        body["error"]["message"], "invalid config",
        "the wire must get ONLY the static category, never `{{e}}`: {body}"
    );
    assert!(
        !raw.contains("alice") && !raw.contains(".secret"),
        "the leaky reason must NEVER reach the wire: {raw}"
    );
}

// ============================================================================
// Captured corpus replay (wave-1 live gate).
// ============================================================================
//
// `tests/wire_corpus/*.json` holds request/response PAIRS. The `response` half
// is the REAL response bytes captured from a live `serve` driving a real
// `codex-acp` agent (wave-1 live gate, 2026-07-03); only the response was
// captured raw. The `request` half is a RECONSTRUCTION of the shape actually
// sent (`SendMessage` with `contextId` + `text` + `metadata["a2a-bridge.agent"]`
// — see the task description) — the exact prompt text sent during that live
// run was not itself captured, so the request `text` here is a plausible
// stand-in, not verbatim captured bytes. Both halves are scrubbed of
// machine/host detail; ids are replaced with obvious placeholders.
//
// The replay proves today's server still ACCEPTS the captured request shape
// and produces a response matching the captured SHAPE (field presence + types
// + the achieved state value) — never byte-equality against the captured
// response (a fresh run drives a fake backend, not the real codex-acp agent,
// so the artifact TEXT will legitimately differ).

const CORPUS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wire_corpus");

fn load_corpus_pair(name: &str) -> (Value, Value) {
    let path = format!("{CORPUS_DIR}/{name}");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path} must exist: {e}"));
    let v: Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} must be valid JSON: {e}"));
    assert!(
        v.get("_provenance").is_some(),
        "{path} must carry a `_provenance` header: {v}"
    );
    (
        v.get("request").cloned().expect("request half present"),
        v.get("response").cloned().expect("response half present"),
    )
}

/// Replay one captured request through today's server (a fresh fake backend
/// standing in for the real agent) and assert the response shape matches the
/// captured one: same JSON-RPC id echo, `task.id` a string, `task.state`
/// EQUAL to the captured value (both runs complete normally), `artifact.text`
/// a non-empty string, `status` an array of strings. Never byte-equality.
fn assert_replay_matches_shape(request: &Value, captured_response: &Value, live_response: &Value) {
    assert_eq!(
        live_response["jsonrpc"], "2.0",
        "must be a JSON-RPC 2.0 envelope: {live_response}"
    );
    assert_eq!(
        live_response["id"], request["id"],
        "the JSON-RPC id must be echoed back: {live_response}"
    );
    assert!(
        live_response.get("error").is_none(),
        "replay must not error: {live_response}"
    );

    let live_task = &live_response["result"]["task"];
    assert!(
        live_task["id"].is_string(),
        "task.id must be a string (never assert its FORMAT): {live_response}"
    );
    assert_eq!(
        live_task["state"], captured_response["result"]["task"]["state"],
        "the achieved TASK_STATE must match the captured value: {live_response}"
    );

    let live_artifact_text = live_response["result"]["artifact"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("artifact.text must be a string: {live_response}"));
    assert!(
        !live_artifact_text.is_empty(),
        "artifact.text must be non-empty: {live_response}"
    );

    let status = live_response["result"]["status"]
        .as_array()
        .unwrap_or_else(|| panic!("status must be an array: {live_response}"));
    assert!(
        status.iter().all(Value::is_string),
        "every status chunk must be a string: {live_response}"
    );
}

/// Build a one-agent server whose registered id matches the corpus request's
/// `a2a-bridge.agent` metadata, so the reconstructed request routes cleanly.
fn build_server_for_corpus_request(request: &Value) -> Arc<InboundServer> {
    let agent = request["params"]["message"]["metadata"]["a2a-bridge.agent"]
        .as_str()
        .expect("corpus request must carry a2a-bridge.agent metadata");
    build_server(one_backend(
        agent,
        Arc::new(FakeBackend {
            reply: "REPLAYED-OK".into(),
        }),
    ))
}

#[tokio::test]
async fn corpus_replay_send_message_warm_first_turn() {
    let (request, response) = load_corpus_pair("send_message_warm_first_turn.json");
    assert_eq!(request["method"], methods::SEND_MESSAGE);

    let srv = build_server_for_corpus_request(&request);
    let resp = srv.router().oneshot(post_raw(&request)).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let live = json_response(resp).await;
    assert_replay_matches_shape(&request, &response, &live);
}

#[tokio::test]
async fn corpus_replay_send_message_warm_second_turn() {
    let (request, response) = load_corpus_pair("send_message_warm_second_turn.json");
    assert_eq!(request["method"], methods::SEND_MESSAGE);

    let srv = build_server_for_corpus_request(&request);
    let resp = srv.router().oneshot(post_raw(&request)).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let live = json_response(resp).await;
    assert_replay_matches_shape(&request, &response, &live);
}

#[tokio::test]
async fn corpus_replay_send_message_cold_agent() {
    let (request, response) = load_corpus_pair("send_message_cold_agent.json");
    assert_eq!(request["method"], methods::SEND_MESSAGE);

    let srv = build_server_for_corpus_request(&request);
    let resp = srv.router().oneshot(post_raw(&request)).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let live = json_response(resp).await;
    assert_replay_matches_shape(&request, &response, &live);
}
