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
    let executor = Arc::new(WorkflowExecutor::new(registry.clone() as Arc<dyn AgentRegistry>));
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
