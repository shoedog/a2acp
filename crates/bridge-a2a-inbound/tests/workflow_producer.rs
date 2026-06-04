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
    PermissionDecision, PermissionRequest, RegistrySnapshot, RouteTarget, SessionContext,
    SessionSpec, TaskMeta,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{AgentId, CallerId, NodeId, SessionId, TaskId, WorkflowId};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, AuthMiddleware, BackendStream, Delegation, DelegationPort, Lease,
    PolicyEngine, Resolved, RouteDecision, SessionStore, Update,
};
use bridge_core::task_store::MemoryTaskStore;
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
        session_cwd: None,
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
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await;
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
// Write-ahead barrier (W3b Task 1b, DoD-1)
// ============================================================================

/// A backend that records, into a SHARED set, every node-session it was prompted
/// for. The session id encodes the node id (`workflow-<wf>-<node>-<run>`), so the
/// test can tell which nodes have been prompted so far.
struct BarrierRecordingBackend {
    reply: String,
    /// Shared across all agents: the set of node ids prompted so far.
    prompted: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentBackend for BarrierRecordingBackend {
    async fn prompt(&self, s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        // session = "workflow-<wf>-<node>-<run>"; pull the node segment out.
        let sid = s.as_str();
        let node = sid
            .strip_prefix("workflow-")
            .and_then(|rest| rest.strip_prefix("pipe-"))
            .and_then(|rest| rest.split('-').next())
            .unwrap_or(sid)
            .to_string();
        self.prompted.lock().unwrap().push(node);
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

struct BarrierRegistry {
    prompted: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentRegistry for BarrierRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        Ok(Resolved {
            entry: Arc::new(minimal_entry(id)),
            backend: Arc::new(BarrierRecordingBackend {
                reply: format!("{}_OUT", id.as_str().to_uppercase()),
                prompted: self.prompted.clone(),
            }),
            lease: Box::new(NoopLease),
        })
    }
    fn default_id(&self) -> AgentId {
        AgentId::parse("a").unwrap()
    }
    async fn apply(&self, _snap: RegistrySnapshot) -> Result<(), BridgeError> {
        Ok(())
    }
    fn list(&self) -> Vec<AgentId> {
        vec![AgentId::parse("a").unwrap(), AgentId::parse("b").unwrap()]
    }
}

/// **DoD-1 — write-ahead barrier**: in a 2-node pipeline a→b (b depends on a), the
/// downstream node `b` must NOT be prompted before `a`'s `NodeFinished` has been
/// handled by the consumer. This mirrors `drain_workflow`'s contract: it awaits the
/// sink's `node_finished` BEFORE pulling the next stream item, and `async_stream`'s
/// `yield` suspends the executor until that next pull — so `b`'s future is only
/// pushed AFTER the consumer returns from handling `a`'s NodeFinished.
///
/// `drain_workflow`/`WorkflowSink` are `pub(crate)`, so this drives the public
/// executor stream directly and snapshots the prompted-set at the exact `NodeFinished{a}`
/// handling point — the same suspension boundary the real drain relies on.
#[tokio::test]
async fn write_ahead_barrier() {
    use bridge_workflow::executor::WorkflowEvent;
    use futures::StreamExt;
    use tokio_util::sync::CancellationToken;

    let prompted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let registry = Arc::new(BarrierRegistry {
        prompted: prompted.clone(),
    });
    let executor = WorkflowExecutor::new(registry as Arc<dyn AgentRegistry>);

    // Pipeline a -> b ; b depends on a (terminal). wf id "pipe" matches the backend's
    // session-prefix parse above.
    let graph = Arc::new(WorkflowGraph {
        id: WorkflowId::parse("pipe").unwrap(),
        nodes: vec![
            WorkflowNode {
                id: NodeId::parse("a").unwrap(),
                agent: AgentId::parse("a").unwrap(),
                prompt_template: "{{input}}".into(),
                inputs: vec![],
            },
            WorkflowNode {
                id: NodeId::parse("b").unwrap(),
                agent: AgentId::parse("b").unwrap(),
                prompt_template: "got {{a}}".into(),
                inputs: vec![NodeId::parse("a").unwrap()],
            },
        ],
    });

    let mut stream = executor.run(graph, "DIFF".into(), "r".into(), CancellationToken::new());
    let mut saw_a_finished = false;
    while let Some(item) = stream.next().await {
        if let Ok(WorkflowEvent::NodeFinished { node, .. }) = &item {
            if node.as_str() == "a" {
                saw_a_finished = true;
                // At the instant a's NodeFinished is handled, b must NOT have been
                // prompted yet — the executor is suspended at the yield and hasn't
                // scheduled b. (If the barrier were broken, b's prompt would already
                // be recorded here.)
                let snapshot = prompted.lock().unwrap().clone();
                assert!(
                    !snapshot.iter().any(|n| n == "b"),
                    "write-ahead barrier violated: b was prompted before a's NodeFinished was handled; prompted so far: {snapshot:?}"
                );
            }
        }
    }
    assert!(saw_a_finished, "a's NodeFinished was never observed");
    // Sanity: b WAS eventually prompted (the pipeline actually ran to completion).
    assert!(
        prompted.lock().unwrap().iter().any(|n| n == "b"),
        "b should have been prompted by the end of the run"
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

/// **unary detached submit**: a UNARY `skill="code-review"` now returns a
/// canonical `a2a::Task` with state `working` IMMEDIATELY and persists a Working
/// row — it no longer rejects with InvalidRequest.
#[tokio::test]
async fn unary_workflow_send_returns_working_task() {
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).expect("valid JSON");
    assert!(body.get("error").is_none(), "must not be an error: {body}");
    let task = &body["result"]["task"];
    let id = task["id"].as_str().expect("task id present");
    assert_ne!(id, "task-1", "detached submit must mint a unique id");
    let state = task["status"]["state"]
        .as_str()
        .or_else(|| task["state"].as_str());
    assert_eq!(
        state,
        Some("TASK_STATE_WORKING"),
        "state must be working: {body}"
    );
    let rec = store
        .get(&bridge_core::ids::TaskId::parse(id).unwrap())
        .await
        .unwrap()
        .expect("row created");
    assert_eq!(
        rec.status,
        bridge_core::task_store::TaskRecordStatus::Working
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

/// **tasks/get canonical**: `GET_TASK` on a completed durable task must return a
/// canonical `a2a::Task` with state `TASK_STATE_COMPLETED` and an artifact
/// carrying the result payload.
#[tokio::test]
async fn tasks_get_returns_completed_with_artifact() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("g1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    store
        .set_terminal(
            &id,
            TaskRecordStatus::Completed,
            Some("THE_RESULT"),
            None,
            2,
        )
        .await
        .unwrap();

    let resp = srv
        .router()
        .oneshot(post_request(methods::GET_TASK, json!({ "taskId": "g1" })))
        .await
        .unwrap();
    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();
    let task = &body["result"]["task"];
    let state = task["status"]["state"]
        .as_str()
        .or_else(|| task["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_COMPLETED"), "{body}");
    assert!(
        body.to_string().contains("THE_RESULT"),
        "completed task must carry the result: {body}"
    );
}

/// **cancel_terminal_detached**: cancelling a task whose TaskStore row is already
/// terminal must return its true state (e.g. Completed) and must NOT mutate the row
/// or re-cancel any backend.
#[tokio::test]
async fn cancel_terminal_detached_returns_true_state_not_recancel() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("c1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    store
        .set_terminal(&id, TaskRecordStatus::Completed, Some("DONE"), None, 2)
        .await
        .unwrap();

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::CANCEL_TASK,
            json!({ "taskId": "c1" }),
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let state = body["result"]["task"]["state"]
        .as_str()
        .or_else(|| body["result"]["task"]["status"]["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_COMPLETED"), "{body}");
    assert_eq!(
        store.get(&id).await.unwrap().unwrap().status,
        TaskRecordStatus::Completed
    );
}

/// A Working row with NO registered cancel token (a stuck task, or one whose
/// runner just removed its token) → cancel flips it to Canceled via the atomic
/// `cancel_if_working` guard (no unconditional clobber).
#[tokio::test]
async fn cancel_working_no_token_flips_to_canceled() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("cw1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    // No token registered in workflow_cancels.
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::CANCEL_TASK,
            json!({ "taskId": "cw1" }),
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let state = body["result"]["task"]["state"]
        .as_str()
        .or_else(|| body["result"]["task"]["status"]["state"].as_str());
    assert_eq!(state, Some("TASK_STATE_CANCELED"), "{body}");
    assert_eq!(
        store.get(&id).await.unwrap().unwrap().status,
        TaskRecordStatus::Canceled
    );
}

// ============================================================================
// Task 11: tasks/list + gated submit test
// ============================================================================

#[tokio::test]
async fn tasks_list_returns_recent_newest_first() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    for (id, ms) in [("l-old", 1i64), ("l-new", 5i64)] {
        store
            .create(&TaskRecord {
                id: TaskId::parse(id).unwrap(),
                workflow: "code-review".into(),
                status: TaskRecordStatus::Working,
                result: None,
                error: None,
                created_ms: ms,
                updated_ms: ms,
                input: String::new(),
                workflow_spec_json: None,
                resume_attempts: 0,
                session_cwd: None,
            })
            .await
            .unwrap();
    }
    let resp = srv
        .router()
        .oneshot(post_request(methods::LIST_TASKS, json!({ "limit": 10 })))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let tasks = body["result"]["tasks"].as_array().expect("tasks array");
    assert_eq!(
        tasks[0]["id"].as_str(),
        Some("l-new"),
        "newest-first: {body}"
    );
}

/// A backend that blocks on a gate (AtomicBool + Notify) before yielding its reply.
struct GatedBackend {
    reply: String,
    gate: Arc<(std::sync::atomic::AtomicBool, tokio::sync::Notify)>,
}

#[async_trait]
impl AgentBackend for GatedBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        while !self.gate.0.load(std::sync::atomic::Ordering::Acquire) {
            self.gate.1.notified().await;
        }
        let reply = self.reply.clone();
        let updates = vec![
            Ok(Update::Text(reply)),
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

/// Registry where every agent uses a GatedBackend with the shared gate.
struct GatedRegistry {
    gate: Arc<(std::sync::atomic::AtomicBool, tokio::sync::Notify)>,
}

#[async_trait]
impl AgentRegistry for GatedRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let reply = format!("{}_REPLY", id.as_str().to_uppercase());
        Ok(Resolved {
            entry: Arc::new(minimal_entry(id)),
            backend: Arc::new(GatedBackend {
                reply,
                gate: self.gate.clone(),
            }),
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
        vec![
            AgentId::parse("codex").unwrap(),
            AgentId::parse("claude").unwrap(),
            AgentId::parse("synth").unwrap(),
        ]
    }
}

fn build_gated_workflow_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    gate: Arc<(std::sync::atomic::AtomicBool, tokio::sync::Notify)>,
) -> Arc<InboundServer> {
    let registry = Arc::new(GatedRegistry { gate });
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
async fn submit_returns_working_before_completion_then_completes() {
    use bridge_core::task_store::{MemoryTaskStore, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let gate = Arc::new((
        std::sync::atomic::AtomicBool::new(false),
        tokio::sync::Notify::new(),
    ));
    let srv = build_gated_workflow_server(store.clone(), gate.clone());

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": { "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" } } }),
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let id = body["result"]["task"]["id"].as_str().unwrap().to_string();
    let tid = bridge_core::ids::TaskId::parse(&id).unwrap();
    // Still Working while gated.
    assert_eq!(
        store.get(&tid).await.unwrap().unwrap().status,
        TaskRecordStatus::Working
    );
    // Release the gate (set flag THEN wake) so late-parking nodes (synth) also proceed.
    gate.0.store(true, std::sync::atomic::Ordering::Release);
    gate.1.notify_waiters();
    for _ in 0..200 {
        if store.get(&tid).await.unwrap().unwrap().status.is_terminal() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        store.get(&tid).await.unwrap().unwrap().status,
        TaskRecordStatus::Completed
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

// ============================================================================
// Task 12: detached Failed / Canceled / swept-Interrupted / panic-finalizer
// ============================================================================

/// Build a server whose registry uses `ErrorBackend` for EVERY agent (including
/// the terminal `synth` node). When synth fails the executor yields
/// `WorkflowOutcome::Failed`, which the runner persists as `TaskRecordStatus::Failed`.
fn build_failing_synth_workflow_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> Arc<InboundServer> {
    let err: Arc<dyn AgentBackend> = Arc::new(ErrorBackend);
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), err.clone()),
        ("claude".to_string(), err.clone()),
        ("synth".to_string(), err.clone()),
    ]
    .into();
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
        .with_workflows(executor, map)
        .with_task_store(store),
    )
}

/// A backend whose `prompt()` panics. Because the executor wraps the `prompt()` call
/// in a `tokio::select!` inside the spawned task, the panic propagates through the
/// executor and unwinds the spawned task, triggering the `Finalizer` drop guard.
struct PanickingBackend;

#[async_trait]
impl AgentBackend for PanickingBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        panic!("boom: injected panic for finalizer test");
    }
    async fn cancel(&self, _s: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}

/// Build a server where every node uses a panicking backend.
fn build_panicking_workflow_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> Arc<InboundServer> {
    let panic_be: Arc<dyn AgentBackend> = Arc::new(PanickingBackend);
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), panic_be.clone()),
        ("claude".to_string(), panic_be.clone()),
        ("synth".to_string(), panic_be.clone()),
    ]
    .into();
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
        .with_workflows(executor, map)
        .with_task_store(store),
    )
}

/// **DoD-7 — panic finalizer**: when the spawned runner panics (backend panics
/// through the executor), the `Finalizer` drop-guard must write a terminal row so
/// the task is never left in `Working`.
///
/// NOTE: The `tokio::select! { biased; _ = cancel.cancelled() => … s = backend.prompt(…) => … }`
/// in `run_node` does NOT wrap the prompt future in a catch-panic boundary. A panic
/// inside `prompt()` propagates as an unwind through the spawned async block, which
/// triggers `Finalizer::drop`. The drop guard spawns a secondary task to call
/// `set_terminal(Failed, …)`. We wait up to 200 yields for that secondary task to
/// complete. The join handle returns `Err(JoinError{panic})` — we swallow it with
/// `let _ = handle.await`.
#[tokio::test]
async fn runner_panic_finalizes_failed_no_orphan() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_panicking_workflow_server(store.clone());
    let task = TaskId::parse("panic-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await;
    let _ = handle.await; // Err(JoinError{panic}) — swallow it
                          // The Finalizer spawns a secondary task; give it time to write the row.
    for _ in 0..200 {
        if store
            .get(&task)
            .await
            .unwrap()
            .unwrap()
            .status
            .is_terminal()
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    let rec = store.get(&task).await.unwrap().unwrap();
    assert!(
        rec.status.is_terminal(),
        "panic must finalize via drop-guard, not orphan Working; got: {:?}",
        rec.status
    );
}

/// **DoD-7 — detached Failed**: when the terminal `synth` node errors, the detached
/// runner must persist `TaskRecordStatus::Failed` with a non-empty error marker.
///
/// The executor yields `WorkflowOutcome::Failed` when `term_ok=false && !cancelled`.
/// All three nodes use `ErrorBackend` (prompt returns Err), so codex, claude, and
/// synth all fail. The terminal node is `synth`; its `ok=false` drives `Failed`.
#[tokio::test]
async fn detached_runner_persists_failed_on_node_failure() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_failing_synth_workflow_server(store.clone());
    let task = TaskId::parse("fail-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".into()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await
    .await
    .unwrap();
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Failed);
    assert!(
        rec.error.is_some(),
        "failed record must carry an error marker"
    );
}

/// **DoD-7 — detached Canceled**: firing the token while the gated backend is
/// parked causes the executor to take the `cancel.cancelled()` branch in
/// `run_node`'s `select!`, set `term_ok=false`, and since `cancel.is_cancelled()`
/// is true the outcome is `WorkflowOutcome::Canceled`. The runner persists
/// `TaskRecordStatus::Canceled`.
///
/// NOTE: `workflow_cancels` is a private field; the token is wired directly via
/// `spawn_detached_workflow_with_token_for_test` — no explicit map insert needed.
#[tokio::test]
async fn detached_runner_persists_canceled_on_token_fire() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let gate = Arc::new((
        std::sync::atomic::AtomicBool::new(false),
        tokio::sync::Notify::new(),
    ));
    let srv = build_gated_workflow_server(store.clone(), gate.clone());
    let task = TaskId::parse("cxl-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    // Pass the token directly to the seam; the executor observes it via select!.
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_with_token_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".into()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
        token.clone(),
    )
    .await;
    // Cancel while the gated backend is parked on notified().await.
    token.cancel();
    let _ = handle.await;
    assert_eq!(
        store.get(&task).await.unwrap().unwrap().status,
        TaskRecordStatus::Canceled,
        "detached runner must persist Canceled when token fires while gated"
    );
}

/// **DoD-10 — swept-Interrupted → tasks/get failed**: a `Working` row that was swept
/// to `Interrupted` (simulating an interrupted previous server run) must appear as
/// `TASK_STATE_FAILED` at the wire (A2A has no `Interrupted` state). The reason text
/// must be included so callers can distinguish it from a regular failure.
#[tokio::test]
async fn swept_interrupted_reports_failed_over_wire() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let id = TaskId::parse("swept-1").unwrap();
    store
        .create(&TaskRecord {
            id: id.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    // Sweep flips Working → Interrupted (simulates a prior crash / server restart).
    store.sweep_interrupted(9).await.unwrap();
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::GET_TASK,
            json!({ "taskId": "swept-1" }),
        ))
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let state = body["result"]["task"]["status"]["state"]
        .as_str()
        .or_else(|| body["result"]["task"]["state"].as_str());
    assert_eq!(
        state,
        Some("TASK_STATE_FAILED"),
        "interrupted → failed at the wire: {body}"
    );
    // DoD-10: the reason MUST reach the wire so callers can distinguish a
    // restart-interruption from a regular failure. `sweep_interrupted` sets
    // error="interrupted (serve restarted)", which `task_record_to_a2a` surfaces
    // as the artifact text.
    assert!(
        body.to_string().contains("interrupted"),
        "swept task must carry the interrupted reason: {body}"
    );
}

// ============================================================================
// Task 7: TaskStoreSink checkpoints each finished node (W3b)
// ============================================================================

/// **detached_runner_checkpoints_each_node**: drive a 3-node `code-review` workflow
/// through the detached runner backed by a real `MemoryTaskStore`. After the run
/// completes, assert `store.node_checkpoints(&task)` has one row per node with the
/// correct `(node_id, output, ok)`.  The `FakeBackend` replies per agent:
///   codex → "CODEX_REVIEW", claude → "CLAUDE_REVIEW", synth → "SYNTH_FINAL".
#[tokio::test]
async fn detached_runner_checkpoints_each_node() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let task = TaskId::parse("chk-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await;
    handle.await.unwrap();

    // Task must complete.
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Completed);

    // Every node must have a checkpoint row.
    let mut checkpoints = store.node_checkpoints(&task).await.unwrap();
    // stable debug output (lookup below is map-based, order doesn't affect correctness)
    checkpoints.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    assert_eq!(
        checkpoints.len(),
        3,
        "expected 3 node checkpoints (codex, claude, synth); got: {checkpoints:?}"
    );

    // Build an expected map.
    let expected: std::collections::HashMap<&str, (&str, bool)> = [
        ("codex", ("CODEX_REVIEW", true)),
        ("claude", ("CLAUDE_REVIEW", true)),
        ("synth", ("SYNTH_FINAL", true)),
    ]
    .into();

    for (node_id, output, ok) in &checkpoints {
        let (exp_output, exp_ok) = expected
            .get(node_id.as_str())
            .unwrap_or_else(|| panic!("unexpected checkpoint node: {}", node_id.as_str()));
        assert_eq!(
            output.as_str(),
            *exp_output,
            "node {} output mismatch",
            node_id.as_str()
        );
        assert_eq!(ok, exp_ok, "node {} ok mismatch", node_id.as_str());
    }
}

/// A `TaskStore` wrapper that delegates everything to an inner `MemoryTaskStore`
/// EXCEPT `put_node_checkpoint`, which always returns `Err(BridgeError::StoreFailure)`.
/// Used to verify that a checkpoint write failure aborts the drain and causes the
/// detached runner to mark the task `Failed`.
struct FailingCheckpointStore {
    inner: MemoryTaskStore,
}

impl FailingCheckpointStore {
    fn new() -> Self {
        Self {
            inner: MemoryTaskStore::new(),
        }
    }
}

#[async_trait]
impl bridge_core::task_store::TaskStore for FailingCheckpointStore {
    async fn create(&self, rec: &bridge_core::task_store::TaskRecord) -> Result<(), BridgeError> {
        self.inner.create(rec).await
    }
    async fn set_terminal(
        &self,
        id: &TaskId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        updated_ms: i64,
    ) -> Result<(), BridgeError> {
        self.inner
            .set_terminal(id, status, result, error, updated_ms)
            .await
    }
    async fn get(
        &self,
        id: &TaskId,
    ) -> Result<Option<bridge_core::task_store::TaskRecord>, BridgeError> {
        self.inner.get(id).await
    }
    async fn list(
        &self,
        limit: usize,
    ) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        self.inner.list(limit).await
    }
    async fn sweep_interrupted(&self, updated_ms: i64) -> Result<u64, BridgeError> {
        self.inner.sweep_interrupted(updated_ms).await
    }
    async fn cancel_if_working(&self, id: &TaskId, updated_ms: i64) -> Result<bool, BridgeError> {
        self.inner.cancel_if_working(id, updated_ms).await
    }
    async fn put_node_checkpoint(
        &self,
        _task: &TaskId,
        _node: &bridge_core::ids::NodeId,
        _output: &str,
        _ok: bool,
        _ts: i64,
    ) -> Result<(), BridgeError> {
        // Always fail — simulates a DB write error.
        Err(BridgeError::StoreFailure)
    }
    async fn node_checkpoints(
        &self,
        task: &TaskId,
    ) -> Result<Vec<(bridge_core::ids::NodeId, String, bool)>, BridgeError> {
        self.inner.node_checkpoints(task).await
    }
    async fn claim_resume_attempt(
        &self,
        task: &TaskId,
        cap: u32,
        now_ms: i64,
    ) -> Result<bridge_core::task_store::ResumeClaim, BridgeError> {
        self.inner.claim_resume_attempt(task, cap, now_ms).await
    }
    async fn working_tasks(&self) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        self.inner.working_tasks().await
    }

    async fn record_node_started(
        &self,
        task: &TaskId,
        node: &bridge_core::ids::NodeId,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        self.inner.record_node_started(task, node, ts).await
    }

    async fn put_node_checkpoint_sequenced(
        &self,
        _task: &TaskId,
        _node: &bridge_core::ids::NodeId,
        _output: &str,
        _ok: bool,
        _ts: i64,
    ) -> Result<i64, BridgeError> {
        // Always fail — simulates a DB write error (mirrors put_node_checkpoint failure).
        Err(BridgeError::StoreFailure)
    }

    async fn set_terminal_sequenced(
        &self,
        task: &TaskId,
        status: bridge_core::task_store::TaskRecordStatus,
        result: Option<&str>,
        error: Option<&str>,
        ts: i64,
    ) -> Result<i64, BridgeError> {
        self.inner.set_terminal_sequenced(task, status, result, error, ts).await
    }

    async fn progress_snapshot(
        &self,
        task: &TaskId,
    ) -> Result<bridge_core::task_store::TaskProgressSnapshot, BridgeError> {
        self.inner.progress_snapshot(task).await
    }
}

// ============================================================================
// Task 8: submit persists input + versioned workflow_spec_json
// ============================================================================

/// **detached_submit_persists_input_and_spec**: a UNARY `skill="code-review"` submit
/// must persist the submitted text as `record.input` AND a versioned JSON snapshot of
/// the resolved workflow graph as `record.workflow_spec_json`.
///
/// Assertions:
/// - `record.input` equals the submitted text `"DIFF"`.
/// - `record.workflow_spec_json` is `Some(s)` where `s` contains `"\"v\":1"` (the
///   version tag) AND the node ids "codex", "claude", "synth" (the resolved graph).
#[tokio::test]
async fn detached_submit_persists_input_and_spec() {
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            serde_json::json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).expect("valid JSON");
    assert!(body.get("error").is_none(), "must not be an error: {body}");

    let task_id_str = body["result"]["task"]["id"]
        .as_str()
        .expect("task id present")
        .to_string();
    let task_id = bridge_core::ids::TaskId::parse(&task_id_str).unwrap();

    let rec = store
        .get(&task_id)
        .await
        .unwrap()
        .expect("TaskRecord must exist");

    // The persisted input must equal the submitted text.
    assert_eq!(
        rec.input, "DIFF",
        "record.input must equal the submitted text"
    );

    // The workflow_spec_json must be Some and contain the version tag + node ids.
    let spec_json = rec
        .workflow_spec_json
        .as_deref()
        .expect("workflow_spec_json must be Some");
    assert!(
        spec_json.contains("\"v\":1"),
        "spec must contain version tag {{\"v\":1}}; got: {spec_json}"
    );
    assert!(
        spec_json.contains("codex"),
        "spec must contain node id 'codex'; got: {spec_json}"
    );
    assert!(
        spec_json.contains("claude"),
        "spec must contain node id 'claude'; got: {spec_json}"
    );
    assert!(
        spec_json.contains("synth"),
        "spec must contain node id 'synth'; got: {spec_json}"
    );
}

/// **detached_runner_checkpoint_write_failure_fails_task**: when `put_node_checkpoint`
/// returns `Err`, the fallible `drain_workflow` propagates the error, causing the
/// detached runner to mark the task `Failed` (via the `Err(e)` arm in
/// `spawn_detached_workflow`).
#[tokio::test]
async fn detached_runner_checkpoint_write_failure_fails_task() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(FailingCheckpointStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let task = TaskId::parse("chk-fail-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await;
    handle.await.unwrap();

    // The checkpoint write failure must have aborted the drain → task is Failed.
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        TaskRecordStatus::Failed,
        "checkpoint write failure must mark the task Failed; got: {:?}",
        rec.status
    );
}

// ============================================================================
// Task 5: DetachedProgressSink in the detached runner — sequenced terminal on
// ALL detached paths + hub-before-spawn + hub cleanup on terminal.
// ============================================================================

/// **detached_runner_sequenced_terminal_and_hub_cleanup**: a detached submit driven
/// to completion must (1) still satisfy the W3b contract (status Completed, a
/// checkpoint present), (2) have a NON-NULL `terminal_seq` (the sink wrote the
/// terminal via the sequenced method), and (3) have its progress hub REMOVED from
/// `progress_hubs` once the task reaches terminal.
#[tokio::test]
async fn detached_runner_sequenced_terminal_and_hub_cleanup() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());
    let task = TaskId::parse("detached-seq-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: String::new(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    )
    .await;
    // While the runner is in-flight the hub must be present (inserted before spawn).
    assert!(
        srv.has_progress_hub_for_test(&task).await,
        "hub must be inserted BEFORE the runner is spawned"
    );
    handle.await.unwrap();

    // W3b contract: Completed with the synth result.
    let got = store.get(&task).await.unwrap().unwrap();
    assert_eq!(got.status, TaskRecordStatus::Completed);
    assert_eq!(got.result.as_deref(), Some("SYNTH_FINAL"));

    // The terminal was written via the sequenced method → terminal_seq is non-NULL,
    // and checkpoints are present (W3b checkpointing intact).
    let snap = store.progress_snapshot(&task).await.unwrap();
    assert!(
        snap.terminal_seq.is_some(),
        "detached terminal must set terminal_seq (sequenced write)"
    );
    assert!(
        !snap.checkpoints.is_empty(),
        "W3b checkpoints must still be persisted"
    );

    // The hub is cleaned up once the task reaches terminal (no leak).
    assert!(
        !srv.has_progress_hub_for_test(&task).await,
        "progress hub must be REMOVED after the task reaches terminal"
    );
}

/// **detached_unknown_workflow_reject_sets_terminal_seq (I6)**: a detached submit whose
/// routed workflow id is NOT registered must finalize via the sequenced path — the
/// resulting terminal task has a NON-NULL `terminal_seq`, and no hub is leaked (the
/// reject happens pre-spawn, so no hub was ever inserted).
#[tokio::test]
async fn detached_unknown_workflow_reject_sets_terminal_seq() {
    use bridge_core::task_store::{MemoryTaskStore, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    /// Routes `skill="code-review"` to a workflow id that is NOT registered on the
    /// server (so the runner's graph lookup is `None` → the unknown-workflow reject).
    struct GhostWorkflowRoute;
    impl RouteDecision for GhostWorkflowRoute {
        fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            if meta.skill.as_deref() == Some("code-review") {
                Ok(RouteTarget::Workflow(WorkflowId::parse("ghost-workflow")?))
            } else {
                Ok(RouteTarget::Local(AgentId::parse("codex")?))
            }
        }
    }

    let registry = Arc::new(FakeRegistry {
        replies: [("synth".to_string(), "SYNTH_FINAL".to_string())].into(),
    });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    // Register ONLY `code-review`; the route returns `ghost-workflow`, which is absent.
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = Arc::new(
        InboundServer::new(
            registry as Arc<dyn AgentRegistry>,
            Arc::new(FakeStore::default()),
            Arc::new(AutoApprove),
            Arc::new(GhostWorkflowRoute),
            Arc::new(AlwaysGrant),
            "http://localhost:8080",
            Arc::new(NoDelegation),
            "codex",
        )
        .with_workflows(executor, map)
        .with_task_store(store.clone()),
    );

    let resp = srv
        .clone()
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": { "a2a-bridge.skill": "code-review" }
            }}),
        ))
        .await
        .unwrap();
    // Drain the response body so the handler fully runs.
    let _ = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();

    // Exactly one row was created (the unknown-workflow path `create`s before reject).
    let rows = store.list(10).await.unwrap();
    assert_eq!(rows.len(), 1, "one task row must have been created");
    let rec = &rows[0];
    assert_eq!(
        rec.status,
        TaskRecordStatus::Failed,
        "unknown workflow → Failed; got {:?}",
        rec.status
    );

    // I6: the terminal was written via the sequenced path → terminal_seq is non-NULL.
    let snap = store.progress_snapshot(&rec.id).await.unwrap();
    assert!(
        snap.terminal_seq.is_some(),
        "unknown-workflow terminal must set terminal_seq (sequenced write)"
    );
    // No hub was ever inserted for this pre-spawn reject → none leaked.
    assert!(
        !srv.has_progress_hub_for_test(&rec.id).await,
        "no hub may be registered for a pre-spawn reject"
    );
}

/// **I6 — resume short-circuit sets terminal_seq**: a resumed task whose terminal
/// node already has a checkpoint short-circuits to terminal; that transition must now
/// go through the sequenced path (non-NULL `terminal_seq`) and leave no hub.
#[tokio::test]
async fn resume_short_circuit_sets_terminal_seq() {
    use bridge_core::ids::{NodeId, TaskId};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, _prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-short-seq").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    for (node, out) in [("codex", "CODEX_DONE"), ("claude", "CLAUDE_DONE"), ("synth", "SYNTH_FINAL")] {
        store
            .put_node_checkpoint(&task, &NodeId::parse(node).unwrap(), out, true, 2)
            .await
            .unwrap();
    }

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Completed);
    let snap = store.progress_snapshot(&task).await.unwrap();
    assert!(
        snap.terminal_seq.is_some(),
        "resume short-circuit terminal must set terminal_seq (sequenced write)"
    );
    assert!(
        !srv.has_progress_hub_for_test(&task).await,
        "resume short-circuit must not leave a hub"
    );
}

/// **I6 — resume no-snapshot Interrupt sets terminal_seq**: a `Working` task with no
/// snapshot is Interrupted at resume; that transition must now be sequenced (non-NULL
/// `terminal_seq`).
#[tokio::test]
async fn resume_no_snapshot_interrupt_sets_terminal_seq() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, _prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-no-snap-seq").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Interrupted);
    let snap = store.progress_snapshot(&task).await.unwrap();
    assert!(
        snap.terminal_seq.is_some(),
        "resume Interrupt must set terminal_seq (sequenced write)"
    );
}

// ============================================================================
// Task 10a: resume_working_tasks boot routine
// ============================================================================

/// Backend that records (into a SHARED set) the node id it was prompted for, then
/// replies with a fixed text + Done. The recorded node id is the AGENT id carried at
/// construction — in `review_graph` node id == agent id (codex/claude/synth), so the
/// shared set is exactly "which nodes were prompted during the resume run".
struct ResumeRecordingBackend {
    node: String,
    reply: String,
    prompted: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentBackend for ResumeRecordingBackend {
    async fn prompt(&self, _s: &SessionId, _p: Vec<Part>) -> Result<BackendStream, BridgeError> {
        self.prompted.lock().unwrap().push(self.node.clone());
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

/// Registry resolving codex/claude/synth to a recording backend that pushes the
/// resolved node/agent id into a shared prompted-set when prompted.
struct ResumeRecordingRegistry {
    prompted: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentRegistry for ResumeRecordingRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let node = id.as_str().to_string();
        let reply = match node.as_str() {
            "codex" => "CODEX_REVIEW",
            "claude" => "CLAUDE_REVIEW",
            "synth" => "SYNTH_FINAL",
            _ => "OUT",
        }
        .to_string();
        Ok(Resolved {
            entry: Arc::new(minimal_entry(id)),
            backend: Arc::new(ResumeRecordingBackend {
                node,
                reply,
                prompted: self.prompted.clone(),
            }),
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
        vec![
            AgentId::parse("codex").unwrap(),
            AgentId::parse("claude").unwrap(),
            AgentId::parse("synth").unwrap(),
        ]
    }
}

/// Build a server over a recording registry + the given task store, with the review
/// graph registered. Returns the server and the shared prompted-set so the resume
/// happy-path test can assert which nodes were re-prompted.
fn build_recording_resume_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> (Arc<InboundServer>, Arc<std::sync::Mutex<Vec<String>>>) {
    let prompted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let registry = Arc::new(ResumeRecordingRegistry {
        prompted: prompted.clone(),
    });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    let srv = Arc::new(
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
    );
    (srv, prompted)
}

/// `{"v":<v>,"graph":<review_graph>}` snapshot string — the exact envelope shape the
/// detached-submit path persists (see the `RouteTarget::Workflow` arm of
/// `unary_message`). `v=1` is the live version; `v=2` exercises the forward-compat door.
fn review_snapshot(v: u32) -> String {
    serde_json::json!({ "v": v, "graph": &*review_graph() }).to_string()
}

/// Poll the store until the task reaches a terminal status (any non-Working), or panic
/// after a bounded number of cooperative yields. The boot resume runner is DETACHED (no
/// JoinHandle is returned), so the happy-path assertion must wait for the background run
/// to finalize rather than asserting immediately after `resume_working_tasks` returns.
async fn poll_to_terminal(
    store: &std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    task: &bridge_core::ids::TaskId,
) -> bridge_core::task_store::TaskRecord {
    use bridge_core::task_store::TaskRecordStatus;
    for _ in 0..2000 {
        if let Some(rec) = store.get(task).await.unwrap() {
            if rec.status != TaskRecordStatus::Working {
                return rec;
            }
        }
        tokio::task::yield_now().await;
    }
    panic!("task {} never reached a terminal status", task.as_str());
}

/// **resume_runs_only_pending_nodes**: a `Working` task whose snapshot is the review
/// graph (codex,claude→synth) with a checkpoint for `codex` ONLY. After resume, `codex`
/// must NOT be re-prompted (its output is seeded), `claude` AND `synth` ARE prompted,
/// and the task finalizes `Completed`.
#[tokio::test]
async fn resume_runs_only_pending_nodes() {
    use bridge_core::ids::{NodeId, TaskId};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-pending-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    // codex already finished before the crash → its checkpoint is the resume seed.
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("codex").unwrap(),
            "CODEX_DONE",
            true,
            2,
        )
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = poll_to_terminal(&store, &task).await;
    assert_eq!(
        rec.status,
        TaskRecordStatus::Completed,
        "resumed task must finalize Completed; got {:?}",
        rec.status
    );
    let nodes = prompted.lock().unwrap().clone();
    assert!(
        !nodes.iter().any(|n| n == "codex"),
        "codex was checkpointed (seeded) → must NOT be re-prompted; prompted: {nodes:?}"
    );
    assert!(
        nodes.iter().any(|n| n == "claude"),
        "claude was un-checkpointed → must be prompted; prompted: {nodes:?}"
    );
    assert!(
        nodes.iter().any(|n| n == "synth"),
        "synth was un-checkpointed → must be prompted; prompted: {nodes:?}"
    );
    // One resume attempt was consumed.
    assert_eq!(
        rec.resume_attempts, 1,
        "exactly one resume attempt consumed"
    );
}

/// **resume_no_snapshot_interrupts**: a `Working` task with no workflow snapshot cannot
/// be reconstructed → Interrupted.
#[tokio::test]
async fn resume_no_snapshot_interrupts() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, _prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-no-snap").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: None,
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Interrupted);
}

/// **resume_unparseable_snapshot_interrupts**: a snapshot that isn't valid JSON → Interrupted.
#[tokio::test]
async fn resume_unparseable_snapshot_interrupts() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, _prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-bad-json").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some("not json".into()),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Interrupted);
}

/// **resume_unknown_version_interrupts**: a structurally valid snapshot whose envelope
/// version is unknown (`v=2`) → Interrupted (the forward-compat door, NOT a panic).
#[tokio::test]
async fn resume_unknown_version_interrupts() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, _prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-v2").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            // Valid graph, but an unknown schema version.
            workflow_spec_json: Some(review_snapshot(2)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Interrupted);
}

/// **resume_cap_exhausted_interrupts**: a `Working` task whose `resume_attempts` is
/// already at `cap` → `claim_resume_attempt` returns Exhausted → Interrupted (the
/// poison-pill guard).
#[tokio::test]
async fn resume_cap_exhausted_interrupts() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let cap = 3u32;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-exhausted").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: cap, // already at the cap
            session_cwd: None,
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, cap).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Interrupted);
    // Cap was exhausted → no resume run was spawned → no node was prompted.
    assert!(
        prompted.lock().unwrap().is_empty(),
        "an exhausted-cap task must not run any node"
    );
}

/// **resume_poison_task_terminates_at_cap** (Task 11 — DoD-9): prove that the
/// `claim_resume_attempt` + `resume_working_tasks` pair forms a **terminating**
/// poison-pill guard. A "poison" task is one whose resumed run never reaches the
/// terminal node, so on every boot `resume_working_tasks` would try to resume it
/// again — the cap must stop that.
///
/// Strategy B (chosen over A because `MemoryTaskStore` has no "reset to Working"
/// helper, making Strategy A's simulated-crash loop awkward):
///
/// 1. Drive the counter by calling `claim_resume_attempt` directly in a **bounded**
///    loop (capped at `cap+5` to catch any infinite-loop regression fast), asserting
///    the exact `Resumable{1}`, `Resumable{2}`, ..., `Resumable{cap}` sequence and
///    then `Exhausted` — proving the counter is monotone and terminates after exactly
///    `cap` claims.
/// 2. With `resume_attempts == cap` and the row still `Working`, call
///    `resume_working_tasks(&srv, cap)` and assert the task is now `Interrupted` (the
///    Exhausted → Interrupted transition) and that `resume_attempts` stopped at `cap`
///    (no further increments).
///
/// This guarantees: (a) the loop TERMINATES (bounded-iteration guard panics if it
/// doesn't), and (b) `resume_working_tasks` converts a cap-exhausted poison task to
/// `Interrupted` rather than resuming it again.
#[tokio::test]
async fn resume_poison_task_terminates_at_cap() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{
        MemoryTaskStore, ResumeClaim, TaskRecord, TaskRecordStatus, TaskStore,
    };
    use std::sync::Arc;

    let cap = 3u32;
    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, prompted) = build_recording_resume_server(store.clone());

    let task = TaskId::parse("poison-cap-1").unwrap();
    // Seed: Working, valid snapshot (no terminal checkpoint) — a task that would
    // loop forever if the cap didn't exist.
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            // Valid snapshot with NO terminal-node checkpoint → the short-circuit
            // never fires, so every boot would try to run the workflow again.
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();

    // ── Step 1: drive the counter and assert the exact Resumable{1..cap} → Exhausted
    // sequence. The outer bound (`cap + 5`) means a broken cap that never exhausts
    // will panic here rather than loop forever.
    let mut resumable_count = 0u32;
    let mut saw_exhausted = false;
    for iteration in 0..(cap + 5) {
        let claim = store.claim_resume_attempt(&task, cap, 100).await.unwrap();
        match claim {
            ResumeClaim::Resumable { attempt } => {
                resumable_count += 1;
                assert_eq!(
                    attempt, resumable_count,
                    "Resumable attempt counter must increment monotonically: \
                     expected {resumable_count}, got {attempt}"
                );
                assert!(
                    resumable_count <= cap,
                    "received more than {cap} Resumable claims before Exhausted \
                     (iteration {iteration}): broken cap!"
                );
            }
            ResumeClaim::Exhausted => {
                saw_exhausted = true;
                // Confirm we got exactly `cap` Resumable claims before Exhausted.
                assert_eq!(
                    resumable_count, cap,
                    "expected exactly {cap} Resumable claims before Exhausted, \
                     got {resumable_count}"
                );
                break;
            }
        }
    }
    assert!(
        saw_exhausted,
        "cap was never exhausted after {} iterations — broken poison-cap guard! \
         The loop MUST terminate.",
        cap + 5
    );

    // The row must still be Working (claim_resume_attempt only increments the counter,
    // it does NOT change the status).
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        TaskRecordStatus::Working,
        "claim_resume_attempt must not change the status; expected Working"
    );
    assert_eq!(
        rec.resume_attempts, cap,
        "resume_attempts must equal cap after exhaustion"
    );

    // ── Step 2: call resume_working_tasks — it must detect Exhausted and flip to
    // Interrupted WITHOUT spawning a runner (no node is prompted).
    bridge_a2a_inbound::server::resume_working_tasks(&srv, cap).await;

    let final_rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        final_rec.status,
        TaskRecordStatus::Interrupted,
        "resume_working_tasks must mark a cap-exhausted poison task Interrupted; \
         got {:?}",
        final_rec.status
    );
    // The counter must NOT have incremented beyond cap.
    assert_eq!(
        final_rec.resume_attempts, cap,
        "resume_attempts must remain at cap after the Exhausted → Interrupted transition; \
         got {}",
        final_rec.resume_attempts
    );
    // No backend was prompted — the poison task was never resumed.
    assert!(
        prompted.lock().unwrap().is_empty(),
        "a cap-exhausted poison task must not prompt any backend; \
         prompted: {:?}",
        prompted.lock().unwrap()
    );
}

/// **resume_terminal_checkpoint_short_circuits**: a `Working` task whose snapshot is the
/// review graph and which HAS a checkpoint for the TERMINAL node `synth` → the workflow
/// had actually finished before the crash. Finalize DIRECTLY to Completed (result =
/// SYNTH_FINAL), with NO backend prompted and NO resume attempt consumed.
#[tokio::test]
async fn resume_terminal_checkpoint_short_circuits() {
    use bridge_core::ids::{NodeId, TaskId};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, prompted) = build_recording_resume_server(store.clone());
    let task = TaskId::parse("resume-short-circuit").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    // Pre-write checkpoints for all upstream nodes AND the terminal node — the terminal
    // output was produced but the row was never flipped (the W3a §8 write-failure gap).
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("codex").unwrap(),
            "CODEX_DONE",
            true,
            2,
        )
        .await
        .unwrap();
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("claude").unwrap(),
            "CLAUDE_DONE",
            true,
            2,
        )
        .await
        .unwrap();
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("synth").unwrap(),
            "SYNTH_FINAL",
            true,
            3,
        )
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        TaskRecordStatus::Completed,
        "terminal checkpoint present → finalize Completed without re-run"
    );
    assert_eq!(rec.result.as_deref(), Some("SYNTH_FINAL"));
    // No backend was prompted — the short-circuit re-runs nothing.
    assert!(
        prompted.lock().unwrap().is_empty(),
        "short-circuit must not prompt any backend"
    );
    // No resume attempt was consumed.
    assert_eq!(
        rec.resume_attempts, 0,
        "short-circuit must NOT consume a resume attempt"
    );
}

// ============================================================================
// W3b Task 10a follow-up: cancel during a RESUMED run
// ============================================================================

/// Registry where every agent resolves to a `PendingBackend`, EXCEPT `codex` which
/// resolves to a `RecordingFakeBackend`. Because the resume seed already has a
/// `codex` checkpoint, `run_from` skips codex entirely — the `RecordingFakeBackend`
/// for `codex` is never called — proving the seeded node is not re-prompted.
/// `claude` and `synth` receive `PendingBackend` so the resumed run stays in-flight
/// long enough for the cancel to fire.
struct PendingResumeRegistry {
    codex_prompted: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl AgentRegistry for PendingResumeRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let backend: Arc<dyn AgentBackend> = if id.as_str() == "codex" {
            // Seeded by the checkpoint → this backend must NEVER be called.
            let prompted = self.codex_prompted.clone();
            Arc::new(RecordingFakeBackend {
                reply: "CODEX_REVIEW".into(),
                received: prompted,
            })
        } else {
            // claude / synth: block forever so the resumed run stays in-flight.
            Arc::new(PendingBackend)
        };
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
        vec![
            AgentId::parse("codex").unwrap(),
            AgentId::parse("claude").unwrap(),
            AgentId::parse("synth").unwrap(),
        ]
    }
}

/// Build a server whose registry blocks on `claude` and `synth` (PendingBackend)
/// while `codex` uses a recording backend (fast, but will never be called because
/// it is seeded in the checkpoint). Returns the server and the shared prompted-set
/// so the test can assert codex was not re-prompted.
fn build_pending_resume_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
) -> (Arc<InboundServer>, Arc<std::sync::Mutex<Vec<String>>>) {
    let codex_prompted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let registry = Arc::new(PendingResumeRegistry {
        codex_prompted: codex_prompted.clone(),
    });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    let srv = Arc::new(
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
    );
    (srv, codex_prompted)
}

/// **resume_then_cancel_mid_run_finalizes_canceled**: prove that the
/// `CancellationToken` registered by `resume_working_tasks` in `workflow_cancels`
/// is the one that cancels the resumed run, and that the resumed run finalizes
/// cleanly as `Canceled` (no orphan, no double-finalize).
///
/// Shape:
/// 1. Seed a `Working` task with the review-graph snapshot + a `codex` checkpoint
///    only — so the resume will re-run `claude` + `synth`, both of which block
///    on a `PendingBackend`.
/// 2. Call `resume_working_tasks(&srv, 3)` — this registers the cancel token in
///    `workflow_cancels` and spawns the detached runner, which blocks on
///    `PendingBackend::prompt`.
/// 3. Fire the cancel via the real `tasks/cancel` JSON-RPC path — the handler
///    finds the `Working` row in `task_store`, looks up the token in
///    `workflow_cancels`, and fires it. The executor's `select!` in `run_node`
///    observes the cancellation and yields `WorkflowOutcome::Canceled`.
/// 4. `poll_to_terminal` → assert final status is `Canceled`.
/// 5. Assert `codex` was NOT re-prompted (its checkpoint was the seed; `run_from`
///    skips seeded nodes entirely).
#[tokio::test]
async fn resume_then_cancel_mid_run_finalizes_canceled() {
    use a2a::methods;
    use bridge_core::ids::{NodeId, TaskId};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let (srv, codex_prompted) = build_pending_resume_server(store.clone());

    // ── 1. Seed a Working task with the review snapshot + codex checkpoint ──
    let task = TaskId::parse("resume-cancel-mid-run").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: None,
        })
        .await
        .unwrap();
    // codex already finished before the crash → seeded; claude + synth are pending.
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("codex").unwrap(),
            "CODEX_DONE",
            true,
            2,
        )
        .await
        .unwrap();

    // ── 2. resume_working_tasks: registers token in workflow_cancels + spawns runner
    //    The runner's claude/synth backends block on PendingBackend::prompt. After
    //    resume_working_tasks returns, the token is already inserted (the insert
    //    happens synchronously before spawn_detached_workflow is called).
    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    // Yield a few times so the spawned runner can get scheduled and reach the
    // PendingBackend.prompt() await point — making the cancel observable in the
    // executor's select!. (Even if it hasn't reached the await yet, the token
    // fires first and the select! picks it up immediately when it does poll.)
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    // ── 3. Fire the cancel via the real tasks/cancel JSON-RPC path ──────────
    // The handler reads the Working row from task_store, finds the token in
    // workflow_cancels, and cancels it. This is identical to the pattern used
    // in cancel_task_fires_workflow_token_stream_ends_canceled.
    let cancel_resp = srv
        .clone()
        .router()
        .oneshot(post_request(
            methods::CANCEL_TASK,
            serde_json::json!({ "taskId": "resume-cancel-mid-run" }),
        ))
        .await
        .unwrap();
    let cancel_body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(cancel_resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    // The cancel_task handler must have found and fired the token.
    assert!(
        cancel_body.get("error").is_none(),
        "tasks/cancel must succeed: {cancel_body}"
    );

    // ── 4. poll_to_terminal: wait for the spawned runner to write Canceled ──
    let rec = poll_to_terminal(&store, &task).await;
    assert_eq!(
        rec.status,
        TaskRecordStatus::Canceled,
        "resumed run must finalize Canceled after token fire; got {:?}",
        rec.status
    );

    // ── 5. codex must NOT have been re-prompted (it was seeded) ──────────────
    assert!(
        codex_prompted.lock().unwrap().is_empty(),
        "codex was checkpointed (seeded) → run_from must not re-prompt it; \
         received prompts: {:?}",
        codex_prompted.lock().unwrap()
    );
}

// ============================================================================
// Task 7: WorkflowRunContext — per-request cwd threads to every node
// ============================================================================

/// Backend that captures the `SessionSpec.cwd` from `configure_session`.
struct CwdCapBackend {
    reply: String,
    cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>>,
}

#[async_trait]
impl AgentBackend for CwdCapBackend {
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
    async fn configure_session(
        &self,
        _s: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.cwds.lock().unwrap().push(spec.cwd.clone());
        Ok(())
    }
}

/// Build a server where every agent uses a `CwdCapBackend` sharing a single `cwds` vec.
fn build_cwd_cap_server(
    cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>>,
) -> Arc<InboundServer> {
    let mk = |reply: &str| -> Arc<dyn AgentBackend> {
        Arc::new(CwdCapBackend {
            reply: reply.to_string(),
            cwds: cwds.clone(),
        })
    };
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), mk("CODEX")),
        ("claude".to_string(), mk("CLAUDE")),
        ("synth".to_string(), mk("FINAL")),
    ]
    .into();
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

/// STREAMING path: `message/stream` with `a2a-bridge.cwd="/req"` must cause every
/// workflow node's `configure_session` to receive `spec.cwd == Some("/req")`.
/// This is the rev1 miss — `spawn_workflow_producer` was calling `executor.run`
/// (default ctx) instead of `run_with_context`. This test proves the fix.
#[tokio::test]
async fn streaming_workflow_threads_cwd_to_every_node() {
    let cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let srv = build_cwd_cap_server(cwds.clone());

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_STREAMING_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": {
                    "a2a-bridge.skill": "code-review",
                    "a2a-bridge.cwd": "/req"
                }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    // Drain the SSE stream to ensure the workflow completes.
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("FINAL"), "workflow must complete: {body}");

    let captured = cwds.lock().unwrap();
    assert_eq!(
        captured.len(),
        3,
        "all 3 nodes must call configure_session; got {:?}",
        &*captured
    );
    for cwd in captured.iter() {
        assert_eq!(
            cwd.as_ref().map(|c| c.as_str()),
            Some("/req"),
            "every node must receive cwd=/req (streaming path), got {:?}",
            cwd
        );
    }
}

/// DETACHED path: `message/send` with `a2a-bridge.cwd="/req"` must cause every
/// workflow node's `configure_session` to receive `spec.cwd == Some("/req")`.
#[tokio::test]
async fn detached_workflow_threads_cwd_to_every_node() {
    use bridge_core::task_store::{MemoryTaskStore, TaskRecordStatus, TaskStore};

    let cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());

    let mk = |reply: &str| -> Arc<dyn AgentBackend> {
        Arc::new(CwdCapBackend {
            reply: reply.to_string(),
            cwds: cwds.clone(),
        })
    };
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), mk("CODEX")),
        ("claude".to_string(), mk("CLAUDE")),
        ("synth".to_string(), mk("FINAL")),
    ]
    .into();
    let registry = Arc::new(PerAgentRegistry { backends });
    let executor = Arc::new(WorkflowExecutor::new(
        registry.clone() as Arc<dyn AgentRegistry>
    ));
    let mut map: HashMap<WorkflowId, Arc<WorkflowGraph>> = HashMap::new();
    map.insert(WorkflowId::parse("code-review").unwrap(), review_graph());
    let srv: Arc<InboundServer> = Arc::new(
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
        .with_task_store(store.clone()),
    );

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            json!({ "message": {
                "text": "DIFF",
                "metadata": {
                    "a2a-bridge.skill": "code-review",
                    "a2a-bridge.cwd": "/req"
                }
            }}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    // The detached submit returns immediately with a Working task; drain the body.
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let task_id = body["result"]["task"]["id"]
        .as_str()
        .expect("response must carry task.id")
        .to_string();
    let task = bridge_core::ids::TaskId::parse(task_id).unwrap();

    // Poll until terminal (mirrors poll_to_terminal in existing tests).
    let rec = poll_to_terminal(&store, &task).await;
    assert_eq!(
        rec.status,
        TaskRecordStatus::Completed,
        "detached workflow must complete; got {:?}",
        rec.status
    );

    let captured = cwds.lock().unwrap();
    assert_eq!(
        captured.len(),
        3,
        "all 3 nodes must call configure_session; got {:?}",
        &*captured
    );
    for cwd in captured.iter() {
        assert_eq!(
            cwd.as_ref().map(|c| c.as_str()),
            Some("/req"),
            "every node must receive cwd=/req (detached path), got {:?}",
            cwd
        );
    }
}

// ============================================================================
// Task 9: boot resume re-validates + restores session_cwd
// ============================================================================

/// Build a server with a `CwdCapBackend` for every agent AND a wired task store.
/// The `cwds` vec captures every `configure_session` call so the test can assert
/// the cwd threaded to the resumed nodes. The server also wires the recording
/// resume graph (review_graph) so it can be deserialized from the snapshot.
fn build_cwd_cap_resume_server(
    store: std::sync::Arc<dyn bridge_core::task_store::TaskStore>,
    cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>>,
) -> Arc<InboundServer> {
    let mk = |reply: &str| -> Arc<dyn AgentBackend> {
        Arc::new(CwdCapBackend {
            reply: reply.to_string(),
            cwds: cwds.clone(),
        })
    };
    let backends: HashMap<String, Arc<dyn AgentBackend>> = [
        ("codex".to_string(), mk("CODEX")),
        ("claude".to_string(), mk("CLAUDE")),
        ("synth".to_string(), mk("FINAL")),
    ]
    .into();
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
        .with_workflows(executor, map)
        .with_task_store(store),
    )
}

/// **resume_restores_session_cwd**: a `Working` task persisted with
/// `session_cwd = Some("/req")` + a valid review-graph snapshot + a `codex`-only
/// checkpoint. After `resume_working_tasks`, the resumed runner must dispatch all
/// un-checkpointed nodes with `SessionSpec.cwd == Some("/req")`.
#[tokio::test]
async fn resume_restores_session_cwd() {
    use bridge_core::ids::{NodeId, TaskId};
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let srv = build_cwd_cap_resume_server(store.clone(), cwds.clone());
    let task = TaskId::parse("resume-cwd-1").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            session_cwd: Some("/req".into()),
        })
        .await
        .unwrap();
    // codex already finished before the crash → its checkpoint is the resume seed.
    store
        .put_node_checkpoint(
            &task,
            &NodeId::parse("codex").unwrap(),
            "CODEX_DONE",
            true,
            2,
        )
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = poll_to_terminal(&store, &task).await;
    assert_eq!(
        rec.status,
        TaskRecordStatus::Completed,
        "resumed task must finalize Completed; got {:?}",
        rec.status
    );

    // The resumed nodes (claude + synth) must each receive cwd = Some("/req").
    let captured = cwds.lock().unwrap().clone();
    assert!(
        !captured.is_empty(),
        "at least one configure_session call expected; got none"
    );
    for cwd in &captured {
        assert_eq!(
            cwd.as_ref().map(|c| c.as_str()),
            Some("/req"),
            "every resumed node must receive cwd=/req; got {:?}",
            cwd
        );
    }
}

/// **resume_corrupt_session_cwd_interrupts**: a `Working` task whose persisted
/// `session_cwd` is a relative path (rejected by `SessionCwd::parse`) + a valid
/// workflow snapshot. `resume_working_tasks` must mark it `Interrupted` ("unreadable
/// session cwd") and must NOT spawn a runner (no configure_session / prompt calls).
#[tokio::test]
async fn resume_corrupt_session_cwd_interrupts() {
    use bridge_core::ids::TaskId;
    use bridge_core::task_store::{MemoryTaskStore, TaskRecord, TaskRecordStatus, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let cwds: Arc<std::sync::Mutex<Vec<Option<bridge_core::SessionCwd>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let srv = build_cwd_cap_resume_server(store.clone(), cwds.clone());
    let task = TaskId::parse("resume-cwd-corrupt").unwrap();
    store
        .create(&TaskRecord {
            id: task.clone(),
            workflow: "code-review".into(),
            status: TaskRecordStatus::Working,
            result: None,
            error: None,
            created_ms: 1,
            updated_ms: 1,
            input: "DIFF".into(),
            workflow_spec_json: Some(review_snapshot(1)),
            resume_attempts: 0,
            // Relative path — SessionCwd::parse rejects this.
            session_cwd: Some("relative-or-bad".into()),
        })
        .await
        .unwrap();

    bridge_a2a_inbound::server::resume_working_tasks(&srv, 3).await;

    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        TaskRecordStatus::Interrupted,
        "corrupt session_cwd must interrupt the task; got {:?}",
        rec.status
    );
    // No runner was spawned — no configure_session call was made.
    assert!(
        cwds.lock().unwrap().is_empty(),
        "no node must be prompted when session_cwd is corrupt; got: {:?}",
        cwds.lock().unwrap()
    );
}

/// **detached_submit_persists_session_cwd**: a `message/send` with
/// `a2a-bridge.cwd="/req"` must persist `session_cwd=Some("/req")` in the
/// `TaskRecord` (Task 8 of the session_cwd increment).
#[tokio::test]
async fn detached_submit_persists_session_cwd() {
    use bridge_core::task_store::{MemoryTaskStore, TaskStore};
    use std::sync::Arc;

    let store: Arc<dyn TaskStore> = Arc::new(MemoryTaskStore::new());
    let srv = build_workflow_server_with_task_store(store.clone());

    let resp = srv
        .router()
        .oneshot(post_request(
            methods::SEND_MESSAGE,
            serde_json::json!({ "message": {
                "text": "DIFF",
                "metadata": {
                    "a2a-bridge.skill": "code-review",
                    "a2a-bridge.cwd": "/req"
                }
            }}),
        ))
        .await
        .unwrap();

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).expect("valid JSON");
    assert!(body.get("error").is_none(), "must not be an error: {body}");

    let task_id_str = body["result"]["task"]["id"]
        .as_str()
        .expect("task id present")
        .to_string();
    let task_id = bridge_core::ids::TaskId::parse(&task_id_str).unwrap();

    let rec = store
        .get(&task_id)
        .await
        .unwrap()
        .expect("TaskRecord must exist");

    assert_eq!(
        rec.session_cwd.as_deref(),
        Some("/req"),
        "record.session_cwd must equal the submitted a2a-bridge.cwd"
    );
}
