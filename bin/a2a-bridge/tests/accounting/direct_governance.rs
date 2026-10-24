use crate::{
    direct::{op, request},
    support::*,
};
use bridge_coordinator::batch::BatchParams;
use bridge_coordinator::coordinator::TurnOutput;
use bridge_coordinator::params::OpParams;
use bridge_core::task_store::{BatchItem, BatchStatus};
use bridge_core::workflow_history::WorkflowHistoryStore;
use bridge_core::{
    domain::*, error::BridgeError, governance::*, ids::*, ports::*, task_store::TaskStore,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tower::ServiceExt;
use BudgetRefusalReasonV1::*;
use BudgetSelectorV1::*;

fn owner() -> AttemptIdentity {
    AttemptIdentity::initial().unwrap()
}

fn rule(selector: BudgetSelectorV1, turns: Option<u64>, tokens: Option<u64>) -> BudgetRuleV1 {
    BudgetRuleV1 {
        id: format!("{selector:?}"),
        selector,
        limits: BudgetLimitsV1 {
            turns,
            terminal_tokens: tokens,
        },
    }
}
fn fixture(rules: Vec<BudgetRuleV1>, model: Option<&str>) -> Fixture {
    let backend = Arc::new(Recording::default());
    let registry = crate::common::single_agent_registry("agent", backend.clone());
    let allowances = model
        .into_iter()
        .map(|model| TurnAllowanceV1 {
            target: BudgetCandidateV1 {
                agent: AgentId::parse("agent").unwrap(),
                model: Some(model.into()),
            },
            terminal_tokens: 10.try_into().unwrap(),
        })
        .collect();
    Fixture::with_policy(
        backend,
        registry,
        Some(Arc::new(BudgetPolicyV1::new(rules, allowances).unwrap())),
    )
}
fn governed(turns: u64) -> Fixture {
    fixture(vec![rule(EachRequest, Some(turns), None)], None)
}
async fn prompt(f: &Fixture, context: &str) -> Result<TurnOutput, BridgeError> {
    f.coordinator
        .prompt_with_identity(op(context), owner())
        .await
}
fn conn(f: &Fixture, usage: bool) -> rusqlite::Connection {
    let c = rusqlite::Connection::open(f.dir.path().join(if usage {
        "tasks.sqlite.usage-v1.sqlite"
    } else {
        "tasks.sqlite"
    }))
    .unwrap();
    if usage {
        c.execute_batch("PRAGMA journal_mode=PERSIST;").unwrap();
    }
    c
}
fn reservations(f: &Fixture) -> Vec<BudgetReservationV1> {
    let c = conn(f, true);
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='governance_reservations_v1')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !exists {
        return vec![];
    }
    let rows = c
        .prepare("SELECT record_json FROM governance_reservations_v1")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| {
            serde_json::from_value::<BudgetReservationV1>(
                serde_json::from_str::<Value>(&r.unwrap()).unwrap()["reservation"].clone(),
            )
            .unwrap()
        })
        .collect();
    rows
}
fn reason(e: BridgeError, expected: BudgetRefusalReasonV1) {
    assert!(
        matches!(&e, BridgeError::BudgetRefused { refusal } if refusal.reason == expected),
        "{e:?}"
    );
    assert_eq!(e.client_message(), "budget refused");
    assert!(!e.is_transient());
}
async fn rpc(f: &Fixture, method: &str, params: Value) -> String {
    rpc_on(f.router(false), method, params).await
}
async fn rpc_on(router: axum::Router, method: &str, params: Value) -> String {
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn mcp(f: &Fixture, tool: &str, identity: &AttemptIdentity, context: &str) -> String {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (client, server) = tokio::io::duplex(65536);
    let (read, write) = tokio::io::split(server);
    let coord = f.coordinator.clone();
    let task = tokio::spawn(async move {
        bridge_mcp::serve(read, write, coord).await.unwrap();
    });
    let (read, mut write) = tokio::io::split(client);
    let mut lines = tokio::io::BufReader::new(read).lines();
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    lines.next_line().await.unwrap();
    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":{
        "text":"injected marker","input":if tool == "run_workflow" { workflow_input() } else { "go" },"model":if tool == "run_workflow" { None } else { Some("model") },"workflow":(tool == "run_workflow").then_some("fixture-workflow"),"context":context,"execution_id":identity.execution_id.as_str(),"attempt_id":identity.attempt_id.as_str()}}});
    write
        .write_all(format!("{call}\n").as_bytes())
        .await
        .unwrap();
    let result = lines.next_line().await.unwrap().unwrap();
    task.abort(); // Keep retained sessions available to the next transport invocation.
    let _ = task.await;
    result
}
async fn surface(
    f: &Fixture,
    method: &str,
    identity: &AttemptIdentity,
    context: Option<&str>,
) -> String {
    if method == "Mcp" {
        mcp(f, "run", identity, context.unwrap_or("mcp")).await
    } else {
        request(f.router(false), method, identity, context).await
    }
}
async fn call(f: &Fixture, method: &str) -> String {
    surface(f, method, &owner(), Some("warm")).await
}
async fn continuation(f: &Fixture, method: &str) -> String {
    if method == "Mcp" {
        mcp(f, "continue", &owner(), "warm").await
    } else {
        call(f, method).await
    }
}

fn prompts(f: &Fixture) -> usize {
    f.backend.prompts.lock().unwrap().len()
}

fn safe(body: &str, method: &str) {
    if method == "SendStreamingMessage" {
        assert!(body.contains("event: error"), "{body}");
        assert_eq!(body.matches("TASK_STATE_FAILED").count(), 1, "{body}");
        assert!(!body.contains("artifact-update"));
    } else {
        let value: Value = serde_json::from_str(body).unwrap();
        if method == "Mcp" {
            assert_eq!(value["result"]["isError"], true);
            assert_eq!(value["result"]["content"][0]["text"], "budget refused");
        } else {
            assert_eq!(value["error"]["message"], "budget refused");
            assert!(value["error"].get("data").is_none());
        }
    }
    assert!(body.contains("budget refused"), "{body}");
    for secret in [
        "LimitExceeded",
        "allowance",
        "policy",
        "persistence",
        "IncompleteCoverage",
        "UnsupportedRoute",
    ] {
        assert!(!body.contains(secret), "{body}");
    }
}

#[tokio::test]
async fn direct_zero_and_positive_controls() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(0);
        safe(&call(&f, method).await, method);
        reason(prompt(&f, "typed").await.err().unwrap(), LimitExceeded);
        assert!(prompts(&f) == 0);
        assert!(reservations(&f).is_empty());
        let f = governed(1);
        assert!(call(&f, method).await.contains("answer"));
        assert_eq!(reservations(&f).len(), 1);
        assert_eq!(
            count(
                &f,
                false,
                "workflow_attempt_summaries WHERE outcome='completed'"
            ),
            1
        );
        assert!(call(&f, method).await.contains("answer"));
        let saved = reservations(&f);
        assert_eq!(saved.len(), 2);
        assert_ne!(saved[0].context.request, saved[1].context.request);
        assert!(saved.iter().all(|r| r.context.workflow_run.is_none()));
    }
}

#[tokio::test]
async fn token_edges_and_workflow_only_control() {
    for (cap, expected) in [(9, Some(LimitExceeded)), (10, None)] {
        let f = fixture(vec![rule(EachRequest, None, Some(cap))], Some("model"));
        let result = prompt(&f, "tokens").await;
        if let Some(expected) = expected {
            reason(result.err().unwrap(), expected);
        } else {
            result.unwrap();
        }
        assert_eq!(reservations(&f).len(), usize::from(cap == 10));
        assert_eq!(prompts(&f), usize::from(cap == 10));
    }
    let f = fixture(vec![rule(EachWorkflowRun, Some(0), Some(0))], None);
    prompt(&f, "no-workflow").await.unwrap();
    assert!(reservations(&f)[0].scopes.is_empty());
}

#[tokio::test]
async fn lifecycle_failures_preserve_budget_projection_and_liability() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(0);
        conn(&f, false).execute_batch("CREATE TRIGGER refuse_terminal BEFORE UPDATE OF terminal_json ON workflow_attempt_summaries BEGIN SELECT RAISE(ABORT,'terminal failed'); END;").unwrap();
        safe(&call(&f, method).await, method);
        assert!(prompts(&f) == 0);
        assert!(reservations(&f).is_empty());
    }
    let f = governed(1);
    f.backend.fail_prompt.store(true, Ordering::SeqCst);
    assert!(!call(&f, "SendStreamingMessage").await.contains("answer"));
    assert_eq!(prompts(&f), 1);
    assert_eq!(reservations(&f).len(), 1);
    assert_eq!(
        count(
            &f,
            false,
            "workflow_attempt_summaries WHERE outcome='failed'"
        ),
        1
    );
    let f = governed(1);
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    conn(&f, true).execute_batch("CREATE TRIGGER refuse_admission BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT,'admission failed'); END;").unwrap();
    reason(
        prompt(&f, "failed").await.err().unwrap(),
        AccountingUnavailable,
    );
    assert!(prompts(&f) == 0);
    assert!(reservations(&f).is_empty());
}

fn timed_fixture(turns: u64) -> (Fixture, Arc<std::sync::atomic::AtomicI64>) {
    use std::sync::atomic::{AtomicI64, Ordering};
    let now = Arc::new(AtomicI64::new(86_400_000));
    let time = now.clone();
    let store = Arc::new(
        bridge_store::sqlite::SqliteStore::open_in_memory_with_clock(Arc::new(move || {
            time.load(Ordering::SeqCst)
        }))
        .unwrap(),
    );
    let backend = Arc::new(Recording::default());
    let registry = crate::common::single_agent_registry("agent", backend.clone());
    let policy = BudgetPolicyV1::new(
        vec![rule(
            UtcDayAgent(AgentId::parse("agent").unwrap()),
            Some(turns),
            None,
        )],
        vec![],
    )
    .unwrap();
    (
        Fixture::with_policy_and_store(backend, registry, Some(Arc::new(policy)), Some(store)),
        now,
    )
}

#[tokio::test]
async fn governed_warm_model_change_accounts_intended_candidate() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        for mapped in ["model", "old"] {
            let f = fixture(vec![rule(EachRequest, None, Some(10))], Some(mapped));
            // Bypass prompting to establish the old conversation selection, not a budget liability.
            let ctx = ContextId::parse("change-model").unwrap();
            let sm = &f.coordinator.session_manager;
            let config = AgentOverride {
                model: Some("old".into()),
                ..Default::default()
            };
            let agent = AgentId::parse("agent").unwrap();
            let t = sm
                .checkout_turn(&ctx, agent, Some(config), None)
                .await
                .unwrap();
            sm.finish_turn(&ctx, t.generation, &t.op).await;
            let identity = owner();
            let body = request(f.router(false), method, &identity, Some("change-model")).await;
            if mapped == "old" {
                safe(&body, method);
                assert!(prompts(&f) == 0);
                assert!(reservations(&f).is_empty());
            } else {
                assert!(body.contains("answer"), "{body}");
                assert_eq!(prompts(&f), 1);
                assert_eq!(
                    reservations(&f)[0].candidate.model.as_deref(),
                    Some("model")
                );
            }
        }
    }
}

#[tokio::test]
async fn coverage_initialization_does_not_deserialize_unrelated_history() {
    let f = governed(1);
    f.store.ensure_usage_coverage_v1().await.unwrap();
    conn(&f, true)
        .execute(
            "INSERT INTO usage_attempts_v1 VALUES('unrelated','invalid-json')",
            [],
        )
        .unwrap();
    f.store.ensure_usage_coverage_v1().await.unwrap();
    assert!(f.store.usage_summary_v1(&Default::default()).await.is_err());
}

async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn count(f: &Fixture, usage: bool, table: &str) -> i64 {
    conn(f, usage)
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[tokio::test]
async fn sse_disconnect_lifecycle() {
    for after_admission in [false, true] {
        let f = governed(1);
        f.backend.stall.store(true, Ordering::SeqCst);
        if after_admission {
            let router = f.router(false);
            let identity = owner();
            let task = tokio::spawn(async move {
                request(router, "SendStreamingMessage", &identity, None).await
            });
            wait_for(|| prompts(&f) != 0).await;
            task.abort();
            let _ = task.await;
        } else {
            // Do not poll the SSE body: close the receiver before the spawned producer's first poll.
            let response = f.router(false).oneshot(axum::http::Request::builder()
                .method("POST").uri("/").header("content-type", "application/json").header("A2A-Version", "1.0")
                .body(axum::body::Body::from(json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"go"}}}).to_string())).unwrap()).await.unwrap();
            drop(response);
        }
        let terminal = "workflow_attempt_summaries WHERE status='terminal'";
        let success = "workflow_attempt_summaries WHERE outcome='completed'";
        wait_for(|| count(&f, false, terminal) == 1).await;
        assert_eq!(count(&f, false, success), 0);
        assert_eq!(prompts(&f), usize::from(after_admission));
        assert_eq!(reservations(&f).len(), usize::from(after_admission));
        if after_admission {
            assert!(f.backend.stopped.load(Ordering::SeqCst));
            assert!(!f.rows()[0].finalized);
        }
    }
}

#[tokio::test]
async fn compaction_and_inject_refused_under_caps() {
    for capped in [false, true] {
        for route in ["SessionCompact", "SessionInject", "Mcp"] {
            let f = if capped { governed(10) } else { Fixture::new() };
            f.store.usage_summary_v1(&Default::default()).await.unwrap();
            prompt(&f, "retained").await.unwrap();
            let ctx = ContextId::parse("retained").unwrap();
            let sm = &f.coordinator.session_manager;
            let before = sm.compaction_snapshot(&ctx).await;
            let usage = count(&f, true, "usage_attempts_v1");
            let history = count(&f, false, "workflow_attempt_summaries");
            let body = if route == "Mcp" {
                mcp(&f, "inject", &owner(), "retained").await
            } else {
                rpc(
                    &f,
                    route,
                    json!({"contextId":"retained", "text":"injected marker"}),
                )
                .await
            };
            if capped {
                safe(&body, route);
                assert_eq!(sm.compaction_snapshot(&ctx).await, before);
                assert_eq!(sm.status(&ctx).await.unwrap().state, "idle");
                assert_eq!(sm.pending_inject_count(&ctx).await, 0);
                assert!(f.backend.releases.lock().unwrap().is_empty());
                assert_eq!(prompts(&f), 1);
                assert_eq!(count(&f, true, "usage_attempts_v1"), usage);
                assert_eq!(count(&f, false, "workflow_attempt_summaries"), history);
                assert_eq!(reservations(&f).len(), 1);
            } else if route == "SessionCompact" {
                assert!(body.contains("\"compacted\":true"), "{body}");
                assert_eq!(prompts(&f), 2);
            } else {
                assert!(body.contains("queued"), "{body}");
                assert_eq!(sm.pending_inject_count(&ctx).await, 1);
            }
        }
    }
}

#[tokio::test]
async fn refused_continuation_preserves_session() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let (f, now) = timed_fixture(1);
        let identity = owner();
        assert!(surface(&f, method, &identity, Some("warm"))
            .await
            .contains("answer"));
        let sm = &f.coordinator.session_manager;
        let ctx = ContextId::parse("warm").unwrap();
        let before = sm.compaction_snapshot(&ctx).await;
        safe(&continuation(&f, method).await, method);
        assert_eq!(prompts(&f), 1);
        assert_eq!(sm.compaction_snapshot(&ctx).await, before);
        now.store(172_800_000, Ordering::SeqCst);
        assert!(continuation(&f, method).await.contains("answer"));
        // The original owner remains completed even across a refused continuation.
        let original = f
            .store
            .attempt(&identity.attempt_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(original.terminal.unwrap().outcome, "completed");
        let prompts = f.backend.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0], prompts[1]);
    }
}

fn fixed_router(f: &Fixture, target: RouteTarget, calls: Arc<AtomicUsize>) -> axum::Router {
    struct Route(RouteTarget);
    impl RouteDecision for Route {
        fn route_before_default(&self, _: &TaskMeta) -> Result<Option<RouteTarget>, BridgeError> {
            Ok(Some(self.0.clone()))
        }
        fn route(&self, _: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(self.0.clone())
        }
    }
    struct Peer(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl DelegationPort for Peer {
        async fn delegate(
            &self,
            _: &AuthContext,
            _: &TaskId,
            _: Vec<Part>,
        ) -> Result<Delegation, BridgeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::UpstreamA2aError)
        }
        async fn cancel(&self, _: &PeerTaskId) -> Result<(), BridgeError> {
            Ok(())
        }
    }
    Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
        f.coordinator.clone(),
        Arc::new(Route(target)),
        Arc::new(bridge_policy::auth::AlwaysGrant),
        "http://localhost",
        Arc::new(Peer(calls)),
        "agent",
    ))
    .router()
}

#[tokio::test]
async fn fanout_guard_precedes_both_legs_and_remote_only_defers_to_peer() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        for capped in [false, true] {
            let f = if capped { governed(0) } else { Fixture::new() };
            let calls = Arc::new(AtomicUsize::new(0));
            let body = request(
                fixed_router(&f, RouteTarget::Fanout, calls.clone()),
                method,
                &owner(),
                None,
            )
            .await;
            if capped {
                safe(&body, method);
            }
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(!capped));
            assert_eq!(prompts(&f), usize::from(!capped));
        }
        let f = governed(0);
        let calls = Arc::new(AtomicUsize::new(0));
        let body = request(
            fixed_router(&f, RouteTarget::Delegate, calls.clone()),
            method,
            &owner(),
            None,
        )
        .await;
        assert!(!body.contains("budget refused"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(prompts(&f) == 0);
        assert!(f
            .store
            .usage_summary_v1(&Default::default())
            .await
            .unwrap()
            .buckets
            .is_empty());
        assert!(reservations(&f).is_empty());
    }
}

#[tokio::test]
async fn duplicate_identity_is_distinct_from_cap_refusal() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(2);
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        let identity = owner();
        assert!(surface(&f, method, &identity, None)
            .await
            .contains("answer"));
        let body = surface(&f, method, &identity, None).await;
        assert!(
            !body.contains("answer") && !body.contains("budget refused"),
            "{body}"
        );
        assert_eq!(prompts(&f), 1);
        assert_eq!(reservations(&f).len(), 1);
        assert!(call(&f, method).await.contains("answer"));
    }
}

fn workflow_input() -> &'static str {
    "---\ntask-type: code-review\n---\n# Review\n\n## Description\nReview this change.\n\n## Acceptance Criteria\n- Report findings\n"
}

async fn wait_task(f: &Fixture, id: &TaskId) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !f.store.get(id).await.unwrap().unwrap().status.is_terminal() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn configured_workflow_and_batch_guards_have_executable_controls() {
    for capped in [false, true] {
        let f = if capped { governed(1) } else { Fixture::new() };
        let p = OpParams::from_mcp_args_for_workflow(
            &json!({"workflow":"fixture-workflow", "input":workflow_input()}),
        )
        .unwrap();
        let result = f.coordinator.run_workflow(p).await;
        if capped {
            reason(result.err().unwrap(), UnsupportedRoute);
        } else {
            wait_task(&f, &result.unwrap()).await;
        }
        let batch = f
            .coordinator
            .run_batch(BatchParams {
                workflow: "fixture-workflow".into(),
                concurrency: None,
                items: vec![BatchItem {
                    item_id: "one".into(),
                    input: workflow_input().into(),
                    session_cwd: None,
                }],
            })
            .await;
        if capped {
            reason(batch.err().unwrap(), UnsupportedRoute);
        } else {
            let id = batch.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while f.coordinator.batch_status(&id).await.unwrap().status == BatchStatus::Working
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        for method in ["SendMessage", "SendStreamingMessage"] {
            let identity = owner();
            let body = rpc_on(fixed_router(&f, RouteTarget::Workflow(WorkflowId::parse("fixture-workflow").unwrap()), Arc::new(Default::default())), method,
                json!({"message":{"text":workflow_input(),"taskId":identity.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":identity.execution_id.as_str(),"a2a-bridge.attempt_id":identity.attempt_id.as_str()}}})).await;
            if capped {
                safe(&body, "SendMessage");
            } else {
                wait_task(&f, &TaskId::parse(identity.execution_id.as_str()).unwrap()).await;
            }
        }
        let identity = owner();
        let body = mcp(&f, "run_workflow", &identity, "workflow").await;
        if capped {
            safe(&body, "Mcp");
            assert!(prompts(&f) == 0);
        } else {
            wait_task(&f, &TaskId::parse(identity.execution_id.as_str()).unwrap()).await;
            assert_eq!(prompts(&f), 5);
        }
    }
}
