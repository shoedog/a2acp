use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use bridge_coordinator::clock::{Clock, ManualClock};
use bridge_coordinator::session_manager::SessionManager;
use bridge_coordinator::Coordinator;
use bridge_core::domain::{
    AgentEntry, AgentKind, Effort, Part, PeerTaskId, PendingRequest, PermissionDecision,
    PermissionRequest, RegistrySnapshot, SessionContext,
};
use bridge_core::error::BridgeError;
use bridge_core::ids::{
    AgentId, AttemptIdentity, ContextId, NodeId, OperationId, SessionId, TaskId, WorkflowId,
};
use bridge_core::ports::{
    AgentBackend, AgentRegistry, BackendStream, Lease, PolicyEngine, Resolved, SessionStore, Update,
};
use bridge_core::session_cwd::SessionCwd;
use bridge_core::task_store::{MemoryTaskStore, TaskStore};
use bridge_core::workflow_history::{
    AttemptReservation, AttemptTerminal, CompletedAttempt, LedgerError, LedgerUnavailableReason,
    MemoryWorkflowHistoryStore, TerminalWrite, WorkflowHistoryStore,
};
use bridge_mcp::framing::FrameReader;
use bridge_workflow::executor::WorkflowExecutor;
use bridge_workflow::graph::{WorkflowGraph, WorkflowNode};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

const TEST_MAX_FRAME: usize = 16 * 1024 * 1024;

struct NoopLease;
impl Lease for NoopLease {}

struct FakeBackend {
    text: String,
    releases: AtomicUsize,
    prompts: AtomicUsize,
    fail_release: AtomicBool,
}

impl FakeBackend {
    fn new(text: &str) -> Self {
        Self {
            text: text.into(),
            releases: AtomicUsize::new(0),
            prompts: AtomicUsize::new(0),
            fail_release: AtomicBool::new(false),
        }
    }

    fn releases(&self) -> usize {
        self.releases.load(Ordering::SeqCst)
    }

    fn prompts(&self) -> usize {
        self.prompts.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentBackend for FakeBackend {
    async fn prompt(
        &self,
        _session: &SessionId,
        _parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        let updates = vec![
            Ok(Update::Text(self.text.clone())),
            Ok(Update::Done {
                stop_reason: "end_turn".into(),
                prefix_attestation: Default::default(),
            }),
        ];
        Ok(Box::pin(tokio_stream::iter(updates)))
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }

    async fn release_session(&self, _session: &SessionId) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }

    async fn release_session_checked(&self, _session: &SessionId) -> Result<(), BridgeError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if self.fail_release.load(Ordering::SeqCst) {
            Err(BridgeError::StoreFailure)
        } else {
            Ok(())
        }
    }
}

struct FakeRegistry {
    entry: AgentEntry,
    backend: Arc<FakeBackend>,
}

#[async_trait]
impl AgentRegistry for FakeRegistry {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        if *id != self.entry.id {
            return Err(BridgeError::UnknownAgent {
                id: id.as_str().into(),
            });
        }
        Ok(Resolved {
            entry: Arc::new(self.entry.clone()),
            backend: self.backend.clone(),
            lease: Box::new(NoopLease),
        })
    }

    fn default_id(&self) -> AgentId {
        self.entry.id.clone()
    }

    async fn apply(&self, _snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
        Ok(())
    }

    fn list(&self) -> Vec<AgentId> {
        vec![self.entry.id.clone()]
    }
}

#[derive(Default)]
struct FakeSessionStore {
    sessions: StdMutex<HashMap<String, SessionId>>,
    pending: StdMutex<HashMap<String, PendingRequest>>,
    peers: StdMutex<HashMap<String, PeerTaskId>>,
    cancels: StdMutex<std::collections::HashSet<String>>,
    fanouts: StdMutex<std::collections::HashSet<String>>,
}

#[async_trait]
impl SessionStore for FakeSessionStore {
    async fn put(&self, task: &TaskId, session: &SessionId) -> Result<(), BridgeError> {
        self.sessions
            .lock()
            .unwrap()
            .insert(task.as_str().into(), session.clone());
        Ok(())
    }

    async fn session_for(&self, task: &TaskId) -> Result<Option<SessionId>, BridgeError> {
        Ok(self.sessions.lock().unwrap().get(task.as_str()).cloned())
    }

    async fn put_pending(&self, task: &TaskId, req: &PendingRequest) -> Result<(), BridgeError> {
        self.pending
            .lock()
            .unwrap()
            .insert(task.as_str().into(), req.clone());
        Ok(())
    }

    async fn take_pending(&self, task: &TaskId) -> Result<Option<PendingRequest>, BridgeError> {
        Ok(self.pending.lock().unwrap().remove(task.as_str()))
    }

    async fn set_peer_task(&self, task: &TaskId, peer: &PeerTaskId) -> Result<(), BridgeError> {
        self.peers
            .lock()
            .unwrap()
            .insert(task.as_str().into(), peer.clone());
        Ok(())
    }

    async fn peer_task_for(&self, task: &TaskId) -> Result<Option<PeerTaskId>, BridgeError> {
        Ok(self.peers.lock().unwrap().get(task.as_str()).cloned())
    }

    async fn request_cancel(&self, task: &TaskId) -> Result<(), BridgeError> {
        self.cancels.lock().unwrap().insert(task.as_str().into());
        Ok(())
    }

    async fn cancel_requested(&self, task: &TaskId) -> Result<bool, BridgeError> {
        Ok(self.cancels.lock().unwrap().contains(task.as_str()))
    }

    async fn set_fanout(&self, task: &TaskId) -> Result<(), BridgeError> {
        self.fanouts.lock().unwrap().insert(task.as_str().into());
        Ok(())
    }

    async fn is_fanout(&self, task: &TaskId) -> Result<bool, BridgeError> {
        Ok(self.fanouts.lock().unwrap().contains(task.as_str()))
    }
}

struct AllowPolicy;

impl PolicyEngine for AllowPolicy {
    fn decide(
        &self,
        _req: &PermissionRequest,
        _ctx: &SessionContext,
    ) -> Result<PermissionDecision, BridgeError> {
        Ok(PermissionDecision::Approve)
    }
}

struct PromptBarrierHistory {
    inner: MemoryWorkflowHistoryStore,
    marks: AtomicUsize,
    terminals: AtomicUsize,
    backend: Arc<FakeBackend>,
    releases_at_terminal: AtomicUsize,
}

impl PromptBarrierHistory {
    fn new(backend: Arc<FakeBackend>) -> Self {
        Self {
            inner: MemoryWorkflowHistoryStore::default(),
            marks: AtomicUsize::default(),
            terminals: AtomicUsize::default(),
            backend,
            releases_at_terminal: AtomicUsize::default(),
        }
    }
}

#[async_trait]
impl WorkflowHistoryStore for PromptBarrierHistory {
    async fn reserve(&self, row: &AttemptReservation) -> Result<(), LedgerError> {
        self.inner.reserve(row).await
    }

    async fn mark_prompt_acceptance(
        &self,
        _id: &bridge_core::ids::AttemptId,
        _acceptance: &str,
    ) -> Result<(), LedgerError> {
        self.marks.fetch_add(1, Ordering::SeqCst);
        Err(LedgerError::new(LedgerUnavailableReason::Io))
    }

    async fn terminalize(
        &self,
        id: &bridge_core::ids::AttemptId,
        terminal: &AttemptTerminal,
    ) -> Result<TerminalWrite, LedgerError> {
        self.releases_at_terminal
            .store(self.backend.releases(), Ordering::SeqCst);
        self.terminals.fetch_add(1, Ordering::SeqCst);
        self.inner.terminalize(id, terminal).await
    }

    async fn set_pinned(
        &self,
        id: &bridge_core::ids::AttemptId,
        pinned: bool,
    ) -> Result<bool, LedgerError> {
        self.inner.set_pinned(id, pinned).await
    }

    async fn interrupt_active(&self, completed_ms: i64) -> Result<u64, LedgerError> {
        self.inner.interrupt_active(completed_ms).await
    }

    async fn latest_reservation_for_task(
        &self,
        task: &TaskId,
    ) -> Result<Option<AttemptReservation>, LedgerError> {
        self.inner.latest_reservation_for_task(task).await
    }

    async fn completed_between(
        &self,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<Vec<CompletedAttempt>, LedgerError> {
        self.inner.completed_between(start_ms, end_ms).await
    }
}

#[derive(Default)]
struct WorkflowLifecycleObserver {
    started: AtomicUsize,
    finished: AtomicUsize,
    stopped: AtomicUsize,
}

impl bridge_core::ports::Observer for WorkflowLifecycleObserver {
    fn record(&self, _event: &bridge_core::ports::ObsEvent<'_>) {}

    fn record_workflow(&self, event: &bridge_core::ports::WorkflowObsEvent<'_>) {
        match event {
            bridge_core::ports::WorkflowObsEvent::Started { .. } => {
                self.started.fetch_add(1, Ordering::SeqCst);
            }
            bridge_core::ports::WorkflowObsEvent::Finished { .. } => {
                self.finished.fetch_add(1, Ordering::SeqCst);
            }
            bridge_core::ports::WorkflowObsEvent::Stopped { .. } => {
                self.stopped.fetch_add(1, Ordering::SeqCst);
            }
            bridge_core::ports::WorkflowObsEvent::TelemetryUnavailable { .. } => {}
        }
    }
}

struct PromptBarrierFixture {
    coord: Arc<Coordinator>,
    backend: Arc<FakeBackend>,
    history: Arc<PromptBarrierHistory>,
    observer: Arc<WorkflowLifecycleObserver>,
}

struct Fixture {
    coord: Arc<Coordinator>,
    backend: Arc<FakeBackend>,
    perm_registry: Arc<bridge_core::permission::PermissionRegistry>,
}

fn agent_entry() -> AgentEntry {
    AgentEntry {
        id: AgentId::parse("codex").unwrap(),
        cmd: Some("codex".into()),
        base_url: None,
        api_key_env: None,
        args: Vec::new(),
        kind: AgentKind::Acp,
        model_provider: None,
        model: None,
        effort: Some(Effort::High),
        mode: None,
        preflight: false,
        fallback_models: vec![],
        cwd: None,
        session_cwd: None,
        sandbox: None,
        watchdog: None,
        mcp: Vec::new(),
        mcp_delivery: Default::default(),
        auth_method: None,
        pre_authenticated: false,
        host_fallback_eligible: false,
        name: None,
        description: None,
        tags: Vec::new(),
        version: None,
        extensions: Default::default(),
    }
}

fn workflow(id: &str) -> Arc<WorkflowGraph> {
    Arc::new(WorkflowGraph {
        id: WorkflowId::parse(id).unwrap(),
        nodes: vec![WorkflowNode {
            id: NodeId::parse("only").unwrap(),
            agent: AgentId::parse("codex").unwrap(),
            prompt_template: "{{input}}".into(),
            inputs: Vec::new(),
            retry: None,
            harvest_sanitization: None,
        }],
        panel: None,
    })
}

fn fixture() -> Fixture {
    fixture_with_split_storage(false)
}

fn split_storage_fixture() -> Fixture {
    fixture_with_split_storage(true)
}

fn prompt_barrier_fixture(fail_release: bool) -> PromptBarrierFixture {
    let backend = Arc::new(FakeBackend::new("must not be prompted"));
    backend.fail_release.store(fail_release, Ordering::SeqCst);
    let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
        entry: agent_entry(),
        backend: backend.clone(),
    });
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
    let session_manager = Arc::new(SessionManager::new_with_clock(
        registry.clone(),
        Duration::from_secs(60),
        clock.clone(),
    ));
    let history = Arc::new(PromptBarrierHistory::new(backend.clone()));
    let observer = Arc::new(WorkflowLifecycleObserver::default());
    let coord = Arc::new(
        Coordinator::new(
            session_manager,
            None,
            Arc::new(HashMap::new()),
            Arc::new(MemoryTaskStore::new()),
            Arc::new(FakeSessionStore::default()),
            Arc::new(AllowPolicy),
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            observer.clone(),
            3,
        )
        .with_workflow_history(Ok(history.clone())),
    );
    PromptBarrierFixture {
        coord,
        backend,
        history,
        observer,
    }
}

fn fixture_with_split_storage(split_storage: bool) -> Fixture {
    let backend = Arc::new(FakeBackend::new("backend text"));
    let registry: Arc<dyn AgentRegistry> = Arc::new(FakeRegistry {
        entry: agent_entry(),
        backend: backend.clone(),
    });
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(1_700_000_000_000));
    let perm_registry = bridge_core::permission::PermissionRegistry::new();
    let session_manager = Arc::new(
        SessionManager::new_with_clock(registry.clone(), Duration::from_secs(60), clock.clone())
            .with_permission_registry(perm_registry.clone()),
    );
    let (task_store, workflow_history): (Arc<dyn TaskStore>, Arc<dyn WorkflowHistoryStore>) =
        if split_storage {
            (
                Arc::new(MemoryTaskStore::new()),
                Arc::new(bridge_store::sqlite::SqliteStore::open_in_memory().unwrap()),
            )
        } else {
            let shared = Arc::new(bridge_store::sqlite::SqliteStore::open_in_memory().unwrap());
            (shared.clone(), shared)
        };
    let session_store: Arc<dyn SessionStore> = Arc::new(FakeSessionStore::default());
    let policy: Arc<dyn PolicyEngine> = Arc::new(AllowPolicy);
    let mut workflows = HashMap::new();
    workflows.insert(
        WorkflowId::parse("code-review").unwrap(),
        workflow("code-review"),
    );
    let executor = Arc::new(WorkflowExecutor::new(registry.clone()));
    let coord = Arc::new(
        Coordinator::new(
            session_manager,
            Some(executor),
            Arc::new(workflows),
            task_store,
            session_store,
            policy,
            registry,
            clock,
            Some(SessionCwd::parse("/tmp").unwrap()),
            None,
            Arc::new(bridge_observ::NoopObserver),
            3,
        )
        .with_workflow_history(Ok(workflow_history))
        .with_permission_registry(perm_registry.clone()),
    );
    Fixture {
        coord,
        backend,
        perm_registry,
    }
}

fn text_body(reply: &Value) -> Value {
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn req(id: i64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn with_attempt_identity(mut arguments: Value) -> Value {
    let identity = AttemptIdentity::initial().unwrap();
    let object = arguments.as_object_mut().unwrap();
    object.insert(
        "execution_id".into(),
        Value::String(identity.execution_id.as_str().to_owned()),
    );
    object.insert(
        "attempt_id".into(),
        Value::String(identity.attempt_id.as_str().to_owned()),
    );
    arguments
}

fn initialize_reqs() -> Vec<Value> {
    vec![
        req(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    ]
}

/// Run a full MCP session over an in-process duplex: write ALL request frames, signal EOF (drop the
/// writer), let `serve` drain them and write every reply (the 256 KiB duplex buffer holds them),
/// then `serve` hits EOF -> `shutdown()` -> returns; finally collect the buffered reply frames.
///
/// This deliberately does NOT interleave send/recv: a request-then-await-reply loop on a single
/// in-process duplex deadlocks the test runtime (the spawned `serve` and the awaiting reader never
/// hand off cleanly on the in-memory waker path). The real `a2a-bridge mcp` binary uses OS pipes +
/// a multi-thread runtime, where streaming interleaving works — that path is the T8 `mcp` live-gate.
async fn run_session(coord: Arc<Coordinator>, requests: Vec<Value>) -> Vec<Value> {
    // Two independent duplexes (NOT a split of one): requests client->serve, replies serve->client.
    // A single split duplex can't signal EOF by dropping just the write half — the read half keeps the
    // stream open — so serve would block forever on `reader.next()`. With two duplexes we can FULLY drop
    // the request writer (EOF on serve's reader) while keeping the reply channel open.
    let (mut req_w, req_r) = tokio::io::duplex(256 * 1024);
    let (resp_w, resp_r) = tokio::io::duplex(256 * 1024);

    for req in &requests {
        let mut buf = serde_json::to_vec(req).unwrap();
        buf.push(b'\n');
        req_w.write_all(&buf).await.unwrap();
    }
    req_w.flush().await.unwrap();
    drop(req_w); // full EOF on req_r -> serve drains, replies, shuts down, returns

    bridge_mcp::serve(req_r, resp_w, coord).await.unwrap();

    let mut reader = FrameReader::new(resp_r, TEST_MAX_FRAME);
    let mut replies = Vec::new();
    while let Some(item) = reader.next().await {
        replies.push(item.unwrap());
    }
    replies
}

#[tokio::test]
async fn initialize_echoes_version_and_lists_tools() {
    let replies = run_session(
        fixture().coord,
        vec![
            req(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        ],
    )
    .await;

    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "run",
            "continue",
            "inject",
            "permit",
            "run_workflow",
            "status",
            "clear",
            "cancel_task"
        ]
    );
}

#[tokio::test]
async fn framed_tools_call_prompt_barrier_cleans_up_before_one_terminal_write() {
    for (fail_release, expected_cleanup, expected_releases) in
        [(false, "complete", 1), (true, "failed", 2)]
    {
        let fixture = prompt_barrier_fixture(fail_release);
        let identity = AttemptIdentity::initial().unwrap();
        let mut requests = initialize_reqs();
        requests.push(req(
            2,
            "tools/call",
            json!({
                "name": "run",
                "arguments": {
                    "input": "must remain unpolled",
                    "agent": "codex",
                    "cwd": "/tmp/repo",
                    "execution_id": identity.execution_id.as_str(),
                    "attempt_id": identity.attempt_id.as_str(),
                }
            }),
        ));

        let replies = run_session(fixture.coord.clone(), requests).await;
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[1]["result"]["isError"], true);
        assert_eq!(
            replies[1]["result"]["content"][0]["text"],
            "durable evidence unavailable: io"
        );
        assert_eq!(fixture.backend.prompts(), 0);
        assert_eq!(
            fixture.history.releases_at_terminal.load(Ordering::SeqCst),
            1,
            "the prompt barrier must make exactly one release attempt before terminalization",
        );
        // The prompt barrier owns one generation-bound checked release. If it
        // fails, the cleanup-failed tombstone is retried exactly once when this
        // process-level harness sends EOF and runs shutdown.
        assert_eq!(fixture.backend.releases(), expected_releases);
        assert_eq!(fixture.history.marks.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.history.terminals.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.observer.started.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.observer.finished.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.observer.stopped.load(Ordering::SeqCst), 0);

        let rows = fixture
            .history
            .completed_between(0, i64::MAX)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].reservation.identity, identity);
        assert_eq!(
            rows[0].reservation.surface,
            bridge_core::workflow_history::ExecutionSurface::Mcp
        );
        assert_eq!(rows[0].terminal.terminal_reason, "prompt_barrier_failed");
        assert_eq!(rows[0].terminal.cleanup_disposition, expected_cleanup);
        assert_eq!(rows[0].terminal.prompt_acceptance, "unknown");
    }
}

#[tokio::test]
async fn tools_call_inject_queues_for_existing_context() {
    let fixture = fixture();
    let out = fixture
        .coord
        .prompt_with_identity(
            bridge_coordinator::params::OpParams {
                workflow: None,
                skill: None,
                input: "hello".into(),
                context: None,
                agent: Some(AgentId::parse("codex").unwrap()),
                model: None,
                effort: None,
                mode: None,
                cwd: Some("/tmp/repo".into()),
            },
            AttemptIdentity::initial().unwrap(),
        )
        .await
        .unwrap();
    let mut reqs = initialize_reqs();
    reqs.push(req(
        2,
        "tools/call",
        json!({
            "name": "inject",
            "arguments": {
                "context": out.context.as_str(),
                "text": "queued",
                "append": true
            }
        }),
    ));

    let replies = run_session(fixture.coord.clone(), reqs).await;

    assert_eq!(replies.len(), 2);
    let body = text_body(&replies[1]);
    assert_eq!(body["queued"], 1);
}

#[tokio::test]
async fn tools_call_permit_resolves_pending_permission() {
    let fixture = fixture();
    let ctx = ContextId::parse("ctx-mcp-permit").unwrap();
    let op = OperationId::parse("turn-1").unwrap();
    let (rx, _guard) = fixture.perm_registry.register(
        bridge_core::permission::PermKey {
            context_id: ctx.clone(),
            generation: 1,
            op: op.clone(),
            request_id: "req-mcp".into(),
        },
        bridge_core::permission::PendingPermissionView {
            request_id: "req-mcp".into(),
            tool_call_id: "tool-1".into(),
            generation: 1,
            op,
            title: "write file".into(),
            options: Vec::new(),
            raw_input: None,
            timeout_ms: 120_000,
        },
    );
    let mut reqs = initialize_reqs();
    reqs.push(req(
        2,
        "tools/call",
        json!({
            "name": "permit",
            "arguments": {
                "context": ctx.as_str(),
                "generation": 1,
                "op": "turn-1",
                "requestId": "req-mcp",
                "decision": { "decision": "approve" }
            }
        }),
    ));

    let replies = run_session(fixture.coord.clone(), reqs).await;

    assert_eq!(replies.len(), 2);
    let body = text_body(&replies[1]);
    assert_eq!(body["resolved"], true);
    assert!(matches!(
        rx.await.unwrap(),
        bridge_core::permission::PermissionResolution::Decided(
            bridge_core::domain::PermitDecision::Approve { .. }
        )
    ));
}

#[tokio::test]
async fn tools_call_run_workflow_returns_task_id() {
    let mut reqs = initialize_reqs();
    let identity = AttemptIdentity::initial().unwrap();
    reqs.push(req(
        2,
        "tools/call",
        json!({
            "name": "run_workflow",
            "arguments": {
                "workflow": "code-review",
                "input": "---\ntask-type: freeform\n---\nreview this",
                "cwd": "/tmp/repo",
                "execution_id": identity.execution_id.as_str(),
                "attempt_id": identity.attempt_id.as_str()
            }
        }),
    ));
    let replies = run_session(fixture().coord, reqs).await;

    // initialize + tools/call -> 2 replies (notifications/initialized has none).
    assert_eq!(replies.len(), 2);
    let body = text_body(&replies[1]);
    assert_eq!(body["task_id"], identity.execution_id.as_str());
    assert_eq!(body["execution_id"], identity.execution_id.as_str());
    assert_eq!(body["attempt_id"], identity.attempt_id.as_str());
    assert_eq!(body["attempt_ordinal"], identity.ordinal);
    assert_eq!(
        body["parent_attempt_id"],
        serde_json::to_value(identity.parent_attempt_id).unwrap()
    );
}

#[tokio::test]
async fn split_task_and_history_authority_refuses_workflow_before_prompt() {
    let fixture = split_storage_fixture();
    let mut reqs = initialize_reqs();
    reqs.push(req(
        2,
        "tools/call",
        json!({
            "name": "run_workflow",
            "arguments": with_attempt_identity(json!({
                "workflow": "code-review",
                "input": "---\ntask-type: freeform\n---\nreview this",
                "cwd": "/tmp/repo"
            }))
        }),
    ));

    let replies = run_session(fixture.coord.clone(), reqs).await;
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(
        fixture.backend.prompts(),
        0,
        "split authority must refuse before any provider prompt effect"
    );
}

#[tokio::test]
async fn tools_call_before_initialized_is_minus_32600() {
    let replies = run_session(
        fixture().coord,
        vec![req(
            1,
            "tools/call",
            json!({ "name": "run", "arguments": { "input": "hello" } }),
        )],
    )
    .await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], -32600);
}

#[tokio::test]
async fn unknown_method_is_minus_32601() {
    let replies = run_session(
        fixture().coord,
        vec![json!({ "jsonrpc": "2.0", "id": 1, "method": "unknown/method" })],
    )
    .await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["error"]["code"], -32601);
}

#[tokio::test]
async fn bad_args_is_iserror_not_jsonrpc_error() {
    let mut reqs = initialize_reqs();
    reqs.push(req(
        2,
        "tools/call",
        json!({ "name": "run", "arguments": {} }),
    ));
    let replies = run_session(fixture().coord, reqs).await;

    assert_eq!(replies.len(), 2);
    assert!(replies[1].get("error").is_none());
    assert_eq!(replies[1]["result"]["isError"], true);
}

#[tokio::test]
async fn clean_eof_triggers_shutdown() {
    let fixture = fixture();
    let coord = fixture.coord.clone();
    let backend = fixture.backend.clone();

    let mut reqs = initialize_reqs();
    reqs.push(req(
        2,
        "tools/call",
        json!({ "name": "run", "arguments": with_attempt_identity(json!({ "input": "hello", "cwd": "/tmp/repo" })) }),
    ));
    // After run_session returns, serve has already hit EOF -> shutdown() -> returned.
    let replies = run_session(coord.clone(), reqs).await;

    let body = text_body(&replies[1]);
    let ctx = ContextId::parse(body["context"].as_str().unwrap()).unwrap();
    // EOF -> Coordinator::shutdown -> release_all: the warm context is gone + the backend released.
    assert!(coord.session_manager.status(&ctx).await.is_none());
    assert_eq!(backend.releases(), 1);
}
