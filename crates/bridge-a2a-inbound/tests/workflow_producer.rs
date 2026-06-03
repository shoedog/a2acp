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
        })
        .await
        .unwrap();
    let handle = bridge_a2a_inbound::server::spawn_detached_workflow_for_test(
        &srv,
        task.clone(),
        vec!["DIFF".to_string()],
        bridge_core::ids::WorkflowId::parse("code-review").unwrap(),
    );
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
    );
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

    // Task must complete.
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(rec.status, TaskRecordStatus::Completed);

    // Every node must have a checkpoint row.
    let mut checkpoints = store.node_checkpoints(&task).await.unwrap();
    // Sort by node id for deterministic comparison.
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
        assert_eq!(
            ok, exp_ok,
            "node {} ok mismatch",
            node_id.as_str()
        );
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
    async fn create(
        &self,
        rec: &bridge_core::task_store::TaskRecord,
    ) -> Result<(), BridgeError> {
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
    async fn cancel_if_working(
        &self,
        id: &TaskId,
        updated_ms: i64,
    ) -> Result<bool, BridgeError> {
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
    async fn working_tasks(
        &self,
    ) -> Result<Vec<bridge_core::task_store::TaskRecord>, BridgeError> {
        self.inner.working_tasks().await
    }
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

    // The checkpoint write failure must have aborted the drain → task is Failed.
    let rec = store.get(&task).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        TaskRecordStatus::Failed,
        "checkpoint write failure must mark the task Failed; got: {:?}",
        rec.status
    );
}
