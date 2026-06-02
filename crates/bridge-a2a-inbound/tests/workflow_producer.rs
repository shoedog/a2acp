// workflow_producer.rs — STREAMING e2e for the `RouteTarget::Workflow` producer.
//
// Builds a real `InboundServer` over fakes, wires a `code-review` workflow
// (codex/claude fan-in to synth) via the public `.with_workflows(executor, map)`
// builder, drives a STREAMING `skill="code-review"` task through the public router,
// and asserts the SSE frames: at least one node Status, a final synth Artifact, and
// a terminal Completed.

use std::collections::HashMap;
use std::sync::Arc;

use a2a::{methods, SVC_PARAM_VERSION};
use async_trait::async_trait;
use serde_json::{json, Value};
use tower::ServiceExt;

use bridge_a2a_inbound::server::InboundServer;
use bridge_core::domain::{
    AgentEntry, AgentKind, AuthContext, InboundRequest, Part, PeerTaskId, PendingRequest,
    PermissionDecision, PermissionRequest, RegistrySnapshot, RouteTarget, SessionContext, TaskMeta,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, CallerId, NodeId, SessionId, TaskId, WorkflowId};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, AuthMiddleware, BackendStream, Delegation, DelegationPort, Lease,
    PolicyEngine, Resolved, RouteDecision, SessionStore, Update,
};
use bridge_workflow::executor::WorkflowExecutor;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};

// ---- fakes ----

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
        auth_method: None,
        name: None,
        description: None,
        tags: vec![],
        version: None,
        extensions: Default::default(),
    }
}

/// Backend that replies with a fixed text then Done. Each registered agent gets a
/// distinct reply so the synth fan-in is observable.
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

/// Registry mapping agent id -> reply text.
struct FakeRegistry {
    replies: HashMap<String, String>,
}
#[async_trait]
impl AgentRegistry for FakeRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let reply = self
            .replies
            .get(id.as_str())
            .cloned()
            .ok_or(BridgeError::UnknownAgent {
                id: id.as_str().into(),
            })?;
        Ok(Resolved {
            entry: Arc::new(minimal_entry(id)),
            backend: Arc::new(FakeBackend { reply }),
            lease: Box::new(NoopLease),
        })
    }
    fn default_id(&self) -> AgentId {
        AgentId::parse("codex").unwrap()
    }
    async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
        Ok(())
    }
    fn list(&self) -> Vec<AgentId> {
        self.replies
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
    async fn put_pending(&self, _t: &TaskId, _r: &PendingRequest) -> Result<(), BridgeError> {
        Ok(())
    }
    async fn take_pending(&self, _t: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
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
        Ok(AuthContext::new(CallerId::parse("anon").unwrap()))
    }
}

/// Routes `skill="code-review"` to the workflow; everything else to local codex.
struct WorkflowRoute;
impl RouteDecision for WorkflowRoute {
    fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
        if meta.skill.as_deref() == Some("code-review") {
            Ok(RouteTarget::Workflow(WorkflowId::parse("code-review")?))
        } else {
            Ok(RouteTarget::Local(AgentId::parse("codex")?))
        }
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

/// codex/claude (no inputs) fan-in to synth (terminal).
fn review_graph() -> Arc<WorkflowGraph> {
    let n = |id: &str, ag: &str, ins: &[&str], tpl: &str| WorkflowNode {
        id: NodeId::parse(id).unwrap(),
        agent: AgentId::parse(ag).unwrap(),
        prompt_template: tpl.into(),
        inputs: ins.iter().map(|i| NodeId::parse(*i).unwrap()).collect(),
    };
    Arc::new(WorkflowGraph {
        id: WorkflowId::parse("code-review").unwrap(),
        nodes: vec![
            n("codex", "codex", &[], "review {{input}}"),
            n("claude", "claude", &[], "review {{input}}"),
            n(
                "synth",
                "synth",
                &["codex", "claude"],
                "merge {{codex}} + {{claude}} for {{input}}",
            ),
        ],
    })
}

fn build_workflow_server() -> Arc<InboundServer> {
    let registry = Arc::new(FakeRegistry {
        replies: [
            ("codex".to_string(), "CODEX_REVIEW".to_string()),
            ("claude".to_string(), "CLAUDE_REVIEW".to_string()),
            ("synth".to_string(), "SYNTH_FINAL".to_string()),
        ]
        .into(),
    });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());

    Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(WorkflowRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_workflows(executor, map),
    )
}

fn build_workflow_server_with_task_store(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> Arc<InboundServer> {
    let registry = Arc::new(FakeRegistry {
        replies: [
            ("codex".to_string(), "CODEX_REVIEW".to_string()),
            ("claude".to_string(), "CLAUDE_REVIEW".to_string()),
            ("synth".to_string(), "SYNTH_FINAL".to_string()),
        ]
        .into(),
    });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(WorkflowRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_workflows(executor, map)
        .with_task_store(store),
    )
}

#[tokio::test]
async fn detached_runner_persists_completed_result() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let task = TaskId::parse("detached-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    );
    handle.await.unwrap();
    let got = store.get(&task).await.unwrap().unwrap();
    assert_eq!(got.status, TaskRecordStatus::Completed);
    assert_eq!(got.result.as_deref(), Some("SYNTH_FINAL"));
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

fn post_request(method: &str, params: Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(SVC_PARAM_VERSION, "1.0")
        .body(jsonrpc_body(method, params))
        .unwrap()
}

fn sse_data_payloads(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| s.trim_end_matches('\r').to_owned())
        .collect()
}

#[tokio::test]
async fn streaming_workflow_emits_node_status_synth_artifact_and_completed() {
    let srv = build_workflow_server();
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_STREAMING_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let payloads = sse_data_payloads(&body);
    assert!(!payloads.is_empty(), "no SSE data payloads: {body}");

    let responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("data payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // >= 1 node Status frame (the producer emits "node <id> started"/"... ok").
    // Node-status frames carry TaskState::Working; the terminal frame is Completed.
    let status_count = responses
        .iter()
        .filter(|sr| {
            matches!(sr, a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Working)
        })
        .count();
    assert!(
        status_count >= 1,
        "expected at least one node Status frame: {body}"
    );

    // A synth Artifact carrying SYNTH_FINAL.
    assert!(
        body.contains("SYNTH_FINAL"),
        "expected the synth output as an artifact: {body}"
    );
    let has_artifact = responses
        .iter()
        .any(|sr| matches!(sr, a2a::StreamResponse::ArtifactUpdate(_)));
    assert!(has_artifact, "expected an ArtifactUpdate frame: {body}");

    // Terminal Completed is the final frame.
    let last = responses.last().unwrap();
    assert!(
        matches!(
            last,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Completed
        ),
        "final frame must be terminal statusUpdate(Completed): {}",
        payloads.last().unwrap()
    );
}

// ============================================================================
// Strengthened assertions (Task 10 Step 4)
// ============================================================================

/// A backend that records the full prompt text it receives AND replies with a fixed
/// string.  Used by `synth_got_both_reviews` to verify the fan-in wiring.
struct RecordingFakeBackend {
    reply: String,
    received: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentBackend for RecordingFakeBackend {
    async fn prompt(&self, _s: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        // Record the concatenated prompt text.
        let text: String = parts.iter().map(|p| p.text.as_str()).collect();
        self.received.lock().unwrap().push(text);
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

/// A backend that always returns an Err from `prompt()` (simulates a failed node).
struct ErrorBackend;

#[async_trait]
impl AgentBackend for ErrorBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        Err(BridgeError::UnknownAgent {
            id: "synth-injected-error".into(),
        })
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Per-agent backend dispatch: maps agent ids to pre-built `Arc<dyn AgentBackend>`.
struct PerAgentRegistry {
    backends: HashMap<String, Arc<dyn AgentBackend>>,
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
        AgentId::parse("codex").unwrap()
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

/// Build a server using a `PerAgentRegistry` with the supplied backends map.
fn build_server_per_agent(backends: HashMap<String, Arc<dyn AgentBackend>>) -> Arc<InboundServer> {
    let registry = Arc::new(PerAgentRegistry { backends });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());

    Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(WorkflowRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_workflows(executor, map),
    )
}

/// **synth_got_both_reviews**: the synth node's prompt must contain both the
/// codex-fake output ("CODEX_REVIEW") and the claude-fake output ("CLAUDE_REVIEW").
#[tokio::test]
async fn synth_receives_both_fan_out_reviews() {
    let synth_received = Arc::new(std::sync::Mutex::new(Vec::new()));

    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        (
            "codex".to_string(),
            Arc::new(RecordingFakeBackend {
                reply: "CODEX_REVIEW".into(),
                received: Arc::new(std::sync::Mutex::new(vec![])),
            }) as Arc<dyn AgentBackend>,
        ),
        (
            "claude".to_string(),
            Arc::new(RecordingFakeBackend {
                reply: "CLAUDE_REVIEW".into(),
                received: Arc::new(std::sync::Mutex::new(vec![])),
            }) as Arc<dyn AgentBackend>,
        ),
        (
            "synth".to_string(),
            Arc::new(RecordingFakeBackend {
                reply: "SYNTH_FINAL".into(),
                received: Arc::clone(&synth_received),
            }) as Arc<dyn AgentBackend>,
        ),
    ]
    .into();

    let srv = build_server_per_agent(backends);
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_STREAMING_MESSAGE,
            json!({ "message": {
                "text": "DIFF_CONTENT",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    // Drain the SSE stream (we only need the workflow to finish).
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        body.contains("SYNTH_FINAL"),
        "synth output must appear: {body}"
    );

    // The synth backend must have received a prompt containing BOTH fan-out outputs.
    let prompts = synth_received.lock().unwrap();
    assert_eq!(prompts.len(), 1, "synth must be prompted exactly once");
    let synth_prompt = &prompts[0];
    assert!(
        synth_prompt.contains("CODEX_REVIEW"),
        "synth prompt must contain codex output; prompt: {synth_prompt}"
    );
    assert!(
        synth_prompt.contains("CLAUDE_REVIEW"),
        "synth prompt must contain claude output; prompt: {synth_prompt}"
    );
}

/// **unary_reject**: a UNARY `skill="code-review"` must return a JSON-RPC
/// `InvalidRequest` error — NOT start a workflow run, NOT panic.
#[tokio::test]
async fn unary_workflow_send_returns_invalid_request_error() {
    let srv = build_workflow_server();
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE, // ← unary, not streaming
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();

    // Must NOT be 500/panic; the server should reply 4xx or 200 with a JSON-RPC error.
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).expect("response must be valid JSON");

    // A JSON-RPC error reply has `"error"` key with `"code"` == -32600.
    let error = body
        .get("error")
        .expect("unary workflow must return a JSON-RPC error object");
    let code = error
        .get("code")
        .and_then(|c| c.as_i64())
        .expect("error must have a numeric code");
    assert_eq!(
        code, -32600,
        "unary workflow send must return InvalidRequest (-32600), got: {body}"
    );
}

// ============================================================================
// A2A-layer cancel wiring (DoD-7)
// ============================================================================

/// Backend whose `prompt()` returns a stream that never resolves — the ONLY
/// terminator for any node running this backend is an external cancel.
struct PendingBackend;

#[async_trait]
impl AgentBackend for PendingBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        Ok(Box::pin(futures::stream::pending()))
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// **DoD-7 — A2A-layer cancel wiring**: an inbound `CancelTask` JSON-RPC call
/// must fire the workflow's `CancellationToken` (registered in
/// `InboundServer.workflow_cancels`) causing the pending executor to yield
/// `WorkflowOutcome::Canceled` and the SSE stream to terminate with a
/// `TaskState::Canceled` status update — within a 2-second timeout.
#[tokio::test]
async fn cancel_task_fires_workflow_token_stream_ends_canceled() {
    // ── build the server with all-pending backends ────────────────────────────
    let pending: Arc<dyn AgentBackend> = Arc::new(PendingBackend);
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), pending.clone()),
        ("claude".to_string(), pending.clone()),
        ("synth".to_string(), pending.clone()),
    ]
    .into();
    let srv = build_server_per_agent(backends);

    // ── start the streaming task with a known, fixed task id ─────────────────
    // `task_id_from_params` accepts a top-level `taskId` field — provide one so
    // we don't have to parse the first SSE frame to discover the id.
    const TASK_ID: &str = "task-wf-cancel-dod7";
    let stream_req = axum::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/json")
        .header(SVC_PARAM_VERSION, "1.0")
        .body(axum::body::Body::from(
            serde_json::to_vec(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": methods::SEND_STREAMING_MESSAGE,
                "params": {
                    "taskId": TASK_ID,
                    "message": {
                        "text": "DIFF",
                        "metadata": { "a2a-bridge.skill": "code-review" }
                    }
                }
            }))
            .unwrap(),
        ))
        .unwrap();

    // Call the handler: the Response is returned as soon as the SSE headers are
    // committed.  The body is a lazy stream that only ends when the workflow
    // terminates.  We collect it in a separate task so we can issue CancelTask
    // while the body is being drained.
    let resp = srv.clone().router().oneshot(stream_req).await.unwrap();
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "streaming request must succeed"
    );
    let body_handle = {
        let body = resp.into_body();
        tokio::spawn(async move { axum::body::to_bytes(body, usize::MAX).await.unwrap() })
    };

    // ── give the producer task time to register its cancel token ─────────────
    // `spawn_workflow_producer` inserts the token in `workflow_cancels` before
    // driving the stream.  A few cooperative yields are sufficient to let that
    // spawned task run past the mutex-insert point.
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    // ── issue CancelTask via the same shared InboundServer ───────────────────
    let cancel_req = post_request(methods::CANCEL_TASK, json!({ "taskId": TASK_ID }));
    let cancel_resp = srv.clone().router().oneshot(cancel_req).await.unwrap();
    assert_eq!(
        cancel_resp.status(),
        axum::http::StatusCode::OK,
        "CancelTask must return 200"
    );
    let cancel_body: Value = serde_json::from_slice(
        &axum::body::to_bytes(cancel_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        cancel_body["result"]["task"]["state"], "TASK_STATE_CANCELED",
        "CancelTask result must report TASK_STATE_CANCELED: {cancel_body}"
    );

    // ── collect the SSE stream; must end with Canceled within the timeout ─────
    let raw_bytes = tokio::time::timeout(std::time::Duration::from_secs(2), body_handle)
        .await
        .expect("SSE body must terminate within 2s after cancel (timeout = stream hung)")
        .expect("body collection task must not panic");

    let body = String::from_utf8(raw_bytes.to_vec()).unwrap();
    let payloads = sse_data_payloads(&body);
    assert!(
        !payloads.is_empty(),
        "SSE stream must emit at least one data frame before Canceled: {body}"
    );

    let responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("SSE payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // The final frame MUST be a terminal Canceled status — not Failed, not a hang.
    let last = responses.last().unwrap();
    assert!(
        matches!(
            last,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Canceled
        ),
        "final SSE frame must be terminal statusUpdate(Canceled) after CancelTask; \
         got: {}",
        payloads.last().unwrap()
    );
}

/// **terminal_failed**: when the synth node errors, the A2A streaming task must end
/// in a `Failed` terminal state (NOT a panic, NOT Completed).
#[tokio::test]
async fn workflow_with_failing_synth_ends_in_failed_state() {
    // synth → ErrorBackend (prompt returns Err) → the node fails → workflow Failed.
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        (
            "codex".to_string(),
            Arc::new(RecordingFakeBackend {
                reply: "CODEX_REVIEW".into(),
                received: Arc::new(std::sync::Mutex::new(vec![])),
            }) as Arc<dyn AgentBackend>,
        ),
        (
            "claude".to_string(),
            Arc::new(RecordingFakeBackend {
                reply: "CLAUDE_REVIEW".into(),
                received: Arc::new(std::sync::Mutex::new(vec![])),
            }) as Arc<dyn AgentBackend>,
        ),
        (
            "synth".to_string(),
            Arc::new(ErrorBackend) as Arc<dyn AgentBackend>,
        ),
    ]
    .into();

    let srv = build_server_per_agent(backends);
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_STREAMING_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    let payloads = sse_data_payloads(&body);
    assert!(!payloads.is_empty(), "no SSE payloads: {body}");

    let responses: Vec<a2a::StreamResponse> = payloads
        .iter()
        .map(|p| {
            serde_json::from_str(p)
                .unwrap_or_else(|e| panic!("payload must parse as StreamResponse: {e}: {p}"))
        })
        .collect();

    // The final frame must be a terminal Failed status.
    let last = responses.last().unwrap();
    assert!(
        matches!(
            last,
            a2a::StreamResponse::StatusUpdate(e)
                if e.status.state == a2a::TaskState::Failed
        ),
        "final frame must be terminal statusUpdate(Failed) when synth errors: {}",
        payloads.last().unwrap()
    );
}
