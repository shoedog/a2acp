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
        "text":"injected marker","input":if tool == "run_workflow" { workflow_input() } else { "go" },"model":if tool == "run_workflow" { None } else { Some("model") },"workflow":(tool == "run_workflow").then_some("fixture-workflow"),"context":(context != "mcp").then_some(context),"execution_id":identity.execution_id.as_str(),"attempt_id":identity.attempt_id.as_str()}}});
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
#[allow(non_snake_case)]
async fn r3_missing_allowance_keeps_A_and_governed_model_change_admits_B() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
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
            let before = sm.compaction_snapshot(&ctx).await;
            f.backend.effects.lock().unwrap().clear();
            let identity = owner();
            let body = surface(&f, method, &identity, Some("change-model")).await;
            if mapped == "old" {
                safe(&body, method);
                assert!(prompts(&f) == 0);
                assert!(reservations(&f).is_empty());
                assert_eq!(sm.compaction_snapshot(&ctx).await, before);
                assert!(f.backend.effects.lock().unwrap().is_empty());
                f.coordinator
                    .continue_turn_with_identity(op("change-model"), owner())
                    .await
                    .unwrap();
                assert_eq!(reservations(&f)[0].candidate.model.as_deref(), Some("old"));
            } else {
                assert!(body.contains("answer"), "{body}");
                assert_eq!(*f.backend.effects.lock().unwrap(), ["reconcile", "prompt"]);
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
async fn closed_before_admission_and_sse_disconnect_lifecycle() {
    for (after_admission, context) in [(false, None), (false, Some("closed")), (true, None)] {
        let f = governed(1);
        let ctx = ContextId::parse("closed").unwrap();
        if context.is_some() {
            let sm = &f.coordinator.session_manager;
            let t = sm
                .checkout_turn(&ctx, AgentId::parse("agent").unwrap(), None, None)
                .await
                .unwrap();
            sm.finish_turn(&ctx, t.generation, &t.op).await;
            f.backend.effects.lock().unwrap().clear();
        }
        let before = f
            .coordinator
            .session_manager
            .compaction_snapshot(&ctx)
            .await;
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
                .body(axum::body::Body::from(json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"go","contextId":context}}}).to_string())).unwrap()).await.unwrap();
            drop(response);
        }
        let terminal = "workflow_attempt_summaries WHERE status='terminal'";
        let success = "workflow_attempt_summaries WHERE outcome='completed'";
        wait_for(|| count(&f, false, terminal) == 1).await;
        assert_eq!(count(&f, false, success), 0);
        assert_eq!(prompts(&f), usize::from(after_admission));
        assert_eq!(reservations(&f).len(), usize::from(after_admission));
        if !after_admission {
            assert!(f.backend.effects.lock().unwrap().is_empty());
            assert_eq!(
                f.coordinator
                    .session_manager
                    .compaction_snapshot(&ctx)
                    .await,
                before
            );
        }
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
async fn refused_turn_keeps_session_exact() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let (f, now) = timed_fixture(1);
        let identity = owner();
        assert!(surface(&f, method, &identity, Some("warm"))
            .await
            .contains("answer"));
        let sm = &f.coordinator.session_manager;
        let ctx = ContextId::parse("warm").unwrap();
        let before = sm.compaction_snapshot(&ctx).await;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let age = sm.status(&ctx).await.unwrap().idle_age_ms;
        f.backend.effects.lock().unwrap().clear();
        let configs = format!("{:?}", f.backend.configs.lock().unwrap());
        safe(&continuation(&f, method).await, method);
        assert!(f.backend.effects.lock().unwrap().is_empty());
        assert_eq!(format!("{:?}", f.backend.configs.lock().unwrap()), configs);
        assert!(sm.status(&ctx).await.unwrap().idle_age_ms >= age);
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

fn fixed_router(
    f: &Fixture,
    target: RouteTarget,
    calls: Arc<AtomicUsize>,
    deferred: bool,
) -> axum::Router {
    struct Route(RouteTarget, bool);
    impl RouteDecision for Route {
        fn route_before_default(&self, _: &TaskMeta) -> Result<Option<RouteTarget>, BridgeError> {
            Ok((!self.1).then(|| self.0.clone()))
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
        Arc::new(Route(target, deferred)),
        Arc::new(bridge_policy::auth::AlwaysGrant),
        "http://localhost",
        Arc::new(Peer(calls)),
        "agent",
    ))
    .router()
}

#[tokio::test]
async fn r3_delegate_is_unsupported_and_fanout_is_guarded() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        for capped in [false, true] {
            let f = if capped { governed(0) } else { Fixture::new() };
            let calls = Arc::new(AtomicUsize::new(0));
            let body = request(
                fixed_router(&f, RouteTarget::Fanout, calls.clone(), false),
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
        for (capped, deferred) in [(false, false), (true, false), (true, true)] {
            let f = if capped { governed(0) } else { Fixture::new() };
            let calls = Arc::new(AtomicUsize::new(0));
            let body = request(
                fixed_router(&f, RouteTarget::Delegate, calls.clone(), deferred),
                method,
                &owner(),
                None,
            )
            .await;
            assert_eq!(body.contains("budget refused"), capped);
            if capped {
                assert_eq!(count(&f, false, "workflow_attempt_summaries"), 0);
            }
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(!capped));
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
            let body = rpc_on(fixed_router(&f, RouteTarget::Workflow(WorkflowId::parse("fixture-workflow").unwrap()), Arc::new(Default::default()), false), method,
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

#[tokio::test]
async fn route_matrix_exactly_once() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp", "Continue"] {
        for state in ["fresh", "warm", "bound", "omitted"] {
            for turns in [0, 1] {
                let f = governed(turns);
                let sm = &f.coordinator.session_manager;
                let ctx = ContextId::parse("matrix").unwrap();
                let identity = owner();
                if state == "warm" {
                    let turn = sm
                        .checkout_turn(&ctx, AgentId::parse("agent").unwrap(), None, None)
                        .await
                        .unwrap();
                    sm.finish_turn(&ctx, turn.generation, &turn.op).await;
                }
                if state == "bound" {
                    let resolved = f
                        .coordinator
                        .registry()
                        .resolve(&AgentId::parse("agent").unwrap())
                        .await
                        .unwrap();
                    f.coordinator.bindings().lock().await.insert(
                        TaskId::parse(identity.execution_id.as_str()).unwrap(),
                        bridge_coordinator::dispatch::TaskBinding {
                            agent: resolved.entry.id.clone(),
                            backend: resolved.backend,
                            eff: Default::default(),
                            lease: Arc::from(resolved.lease),
                        },
                    );
                }
                let before = sm.compaction_snapshot(&ctx).await;
                f.backend.effects.lock().unwrap().clear();
                let body = if method == "Continue" {
                    mcp(
                        &f,
                        "continue",
                        &identity,
                        if state == "omitted" { "mcp" } else { "matrix" },
                    )
                    .await
                } else {
                    surface(
                        &f,
                        method,
                        &identity,
                        (state != "bound" && state != "omitted").then_some("matrix"),
                    )
                    .await
                };
                let admitted = turns != 0 && (method != "Continue" || state == "warm");
                assert_eq!(
                    body.contains("answer"),
                    admitted,
                    "{method}/{state}: {body}"
                );
                assert_eq!(reservations(&f).len(), usize::from(admitted));
                assert_eq!(prompts(&f), usize::from(admitted));
                if !admitted {
                    assert!(
                        f.backend.effects.lock().unwrap().is_empty(),
                        "{method}/{state}"
                    );
                    assert_eq!(sm.compaction_snapshot(&ctx).await, before);
                }
            }
        }
    }
}

#[tokio::test]
async fn r3_policy_attach_rejects_live_context() {
    for state in [
        "empty",
        "idle",
        "running",
        "claim",
        "seed",
        "inject",
        "contended",
    ] {
        for capped in [false, true] {
            let f = if state == "contended" {
                governed(1)
            } else {
                Fixture::new()
            };
            let ctx = ContextId::parse("attach").unwrap();
            let sm = &f.coordinator.session_manager;
            if state != "empty" && state != "claim" {
                prompt(&f, "attach").await.unwrap();
            }
            if state == "seed" {
                rpc(&f, "SessionCompact", json!({"contextId":"attach"})).await;
            }
            if state == "inject" {
                rpc(
                    &f,
                    "SessionInject",
                    json!({"contextId":"attach","text":"queued"}),
                )
                .await;
            }
            let _running = if state == "running" {
                Some(sm.checkout_existing_turn(&ctx).await.unwrap())
            } else {
                None
            };
            let _claim = if state == "claim" {
                Some(
                    sm.claim_turn_observed(
                        &ctx,
                        AgentId::parse("agent").unwrap(),
                        None,
                        None,
                        Arc::new(bridge_core::diagnostics::NoopDiagnosticObserver::default()),
                    )
                    .await
                    .unwrap(),
                )
            } else {
                None
            };
            let mut accounting = billing_accounting(f.store.clone());
            accounting.budget_policy = capped.then(|| {
                Arc::new(
                    BudgetPolicyV1::new(vec![rule(EachRequest, Some(1), None)], vec![]).unwrap(),
                )
            });
            if state == "contended" {
                let candidate = bridge_coordinator::Coordinator::new(
                    sm.clone(),
                    None,
                    Arc::new(Default::default()),
                    f.store.clone(),
                    f.store.clone(),
                    Arc::new(bridge_policy::permission::AutoPolicy),
                    f.coordinator.registry(),
                    Arc::new(bridge_coordinator::clock::SystemClock),
                    None,
                    None,
                    Arc::new(bridge_observ::NoopObserver),
                    3,
                );
                let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let check = observed.clone();
                *f.backend.on_prompt_poll.lock().unwrap() = Some(Box::new(move || {
                    // The R1 first-poll gate holds the manager lock through this callback.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        candidate.with_accounting(accounting)
                    }));
                    assert_eq!(result.is_err(), capped);
                    check.store(true, Ordering::SeqCst);
                }));
                prompt(&f, "attach").await.unwrap();
                assert!(observed.load(Ordering::SeqCst));
                continue;
            }
            let coordinator = Arc::try_unwrap(f.coordinator).ok().unwrap();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                coordinator.with_accounting(accounting)
            }));
            assert_eq!(result.is_err(), capped && state != "empty", "{state}");
        }
    }
}

#[tokio::test]
async fn config_reload_during_apply() {
    use futures::FutureExt;
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        for (state, failed_cleanup) in [
            ("warm", false),
            ("warm", true),
            ("fresh", false),
            ("bound", false),
            ("bound", true),
        ] {
            if method == "Mcp" && state != "warm" {
                continue;
            }
            for during_apply in [false, true] {
                let f = governed(1);
                let sm = &f.coordinator.session_manager;
                let ctx = ContextId::parse("reload").unwrap();
                let identity = owner();
                f.backend
                    .cleanup_error
                    .store(failed_cleanup, Ordering::SeqCst);
                if state == "warm" {
                    let turn = sm
                        .checkout_turn(&ctx, AgentId::parse("agent").unwrap(), None, None)
                        .await
                        .unwrap();
                    sm.finish_turn(&ctx, turn.generation, &turn.op).await;
                }
                let registry = f.coordinator.registry();
                if state == "bound" {
                    let resolved = registry.resolve(&registry.default_id()).await.unwrap();
                    f.coordinator.bindings().lock().await.insert(
                        TaskId::parse(identity.execution_id.as_str()).unwrap(),
                        bridge_coordinator::dispatch::TaskBinding {
                            agent: resolved.entry.id.clone(),
                            backend: resolved.backend,
                            eff: Default::default(),
                            lease: Arc::from(resolved.lease),
                        },
                    );
                }
                f.backend.effects.lock().unwrap().clear();
                let mut entry = (*registry.entry_snapshot(&registry.default_id()).unwrap()).clone();
                entry.name = Some("replacement snapshot".into());
                let snapshot = RegistrySnapshot {
                    default: entry.id.clone(),
                    entries: vec![entry],
                    allowed_cmds: vec!["test-cmd".into()],
                    billing_identities: vec![],
                };
                let mut reload: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
                    registry.apply(snapshot).now_or_never().unwrap().unwrap();
                }));
                let (tx, rx) = tokio::sync::oneshot::channel();
                if during_apply {
                    if state == "warm" {
                        *f.backend.reconcile_gate.lock().unwrap() = Some(rx);
                    } else {
                        *f.backend.configure_gate.lock().unwrap() = Some(rx);
                    }
                } else {
                    *f.backend.before_turn.lock().unwrap() = reload.take();
                }
                let router = f.router(false);
                let coordinator = f.coordinator.clone();
                let request_identity = identity.clone();
                let request = tokio::spawn(async move {
                    if method == "Mcp" {
                        let mut p = op("reload");
                        p.mode = None;
                        coordinator
                            .prompt_with_identity_outcome(p, request_identity)
                            .await
                            .err()
                            .map(|f| f.original)
                            .unwrap()
                            .client_message()
                            .to_owned()
                    } else {
                        request(
                            router,
                            method,
                            &request_identity,
                            (state == "warm").then_some("reload"),
                        )
                        .await
                    }
                });
                if during_apply {
                    wait_for(|| {
                        f.backend
                            .effects
                            .lock()
                            .unwrap()
                            .contains(&if state == "warm" {
                                "reconcile"
                            } else {
                                "configure"
                            })
                    })
                    .await;
                    reload.take().unwrap()();
                    tx.send(()).unwrap();
                }
                let body = request.await.unwrap();
                assert!(body.contains("config mismatch"), "{method}/{state}: {body}");
                assert_eq!(reservations(&f).len(), 1);
                assert_eq!(prompts(&f), 0);
                assert_eq!(f.backend.releases.lock().unwrap().len(), 1);
                let expected = if failed_cleanup {
                    if state == "warm" && !during_apply {
                        "failed"
                    } else {
                        "unknown"
                    }
                } else {
                    "complete"
                };
                let saved = f
                    .store
                    .attempt(&identity.attempt_id)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    saved.terminal.unwrap().cleanup_disposition,
                    expected,
                    "{method}/{state}/{during_apply}"
                );
                let status = rpc(
                    &f,
                    "AttemptStatus",
                    json!({"attemptId":identity.attempt_id.as_str()}),
                )
                .await;
                assert!(
                    status.contains(&format!("\"cleanup_disposition\":\"{expected}\"")),
                    "{status}"
                );
                if state == "warm" {
                    assert_ne!(sm.status(&ctx).await.map(|s| s.state), Some("idle"));
                }
            }
        }
    }
}

#[tokio::test]
async fn r3_ready_failure_beats_disconnect() {
    for buffered in [false, true] {
        let f = governed(1);
        f.backend.stream_failure.store(buffered, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();
        *f.backend.prompt_gate.lock().unwrap() = Some(rx);
        *f.backend.prompt_error.lock().unwrap() = Some(BridgeError::agent_failure(serde_json::from_value(json!({
        "schema_version":1,"failed_phase":"prompt_start","class":"protocol","disposition":"fatal","code":"acp.protocol",
        "summary":"fatal ACP result","stderr_observed":false,"stderr_line_count":0,"prompt_may_have_been_accepted":true
    })).unwrap()));
        let response = f.router(false).oneshot(axum::http::Request::builder()
        .method("POST").uri("/").header("content-type", "application/json").header("A2A-Version", "1.0")
        .body(axum::body::Body::from(json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"go","contextId":"fatal"}}}).to_string())).unwrap()).await.unwrap();
        wait_for(|| {
            prompts(&f) == 1 && (!buffered || f.backend.stream_waiting.load(Ordering::SeqCst))
        })
        .await;
        drop(response);
        tx.send(()).unwrap();
        wait_for(|| !f.backend.releases.lock().unwrap().is_empty()).await;
        assert_eq!(reservations(&f).len(), 1);
        assert!(f
            .coordinator
            .session_manager
            .status(&ContextId::parse("fatal").unwrap())
            .await
            .is_none());
    }
}

#[tokio::test]
async fn preparation_terminal_persistence_is_preserved() {
    for fail in [false, true] {
        let f = governed(1);
        if fail {
            conn(&f, false).execute_batch("CREATE TRIGGER refuse_terminal BEFORE UPDATE OF terminal_json ON workflow_attempt_summaries BEGIN SELECT RAISE(ABORT,'terminal failed'); END;").unwrap();
        }
        let failure = f
            .coordinator
            .continue_turn_with_identity_outcome(op("absent"), owner())
            .await
            .err()
            .unwrap();
        assert!(matches!(failure.original, BridgeError::SessionNotFound));
        assert_eq!(failure.persistence.is_some(), fail);
        assert!(f.backend.effects.lock().unwrap().is_empty());
    }
}

// Wire controls retain frame boundaries and payloads, normalizing generated identities
// and JSON object order (HashMap iteration is deliberately process-randomized).
fn stable_wire(body: &str) -> String {
    fn canonical(mut value: Value) -> Value {
        match &mut value {
            Value::Object(map) => {
                for (key, value) in map.iter_mut() {
                    if matches!(key.as_str(), "messageId" | "artifactId")
                        || (key == "context"
                            && value.as_str().is_some_and(|s| s.starts_with("ctx-")))
                    {
                        *value = json!("generated");
                    } else {
                        *value = canonical(value.take());
                    }
                }
                map.sort_keys();
            }
            Value::Array(values) => {
                for value in values {
                    *value = canonical(value.take());
                }
            }
            Value::String(text) => {
                if let Ok(inner @ Value::Object(_)) = serde_json::from_str(text) {
                    *text = canonical(inner).to_string();
                }
            }
            _ => {}
        }
        value
    }
    if body.starts_with("event:") {
        body.split('\n')
            .map(|line| match line.strip_prefix("data: ") {
                Some(data) => format!("data: {}", canonical(serde_json::from_str(data).unwrap())),
                None => line.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        canonical(serde_json::from_str(body).unwrap()).to_string()
    }
}

#[tokio::test]
async fn uncapped_order_and_wire_control() {
    let mut receipts = vec![];
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        for warm in [false, true] {
            for failed in [false, true] {
                let f = Fixture::new();
                let identity = AttemptIdentity {
                    execution_id: ExecutionId::parse(format!("exec-{}", "1".repeat(32))).unwrap(),
                    attempt_id: AttemptId::parse(format!("attempt-{}", "2".repeat(32))).unwrap(),
                    ordinal: 0,
                    parent_attempt_id: None,
                };
                if warm {
                    let ctx = ContextId::parse("wire").unwrap();
                    let t = f
                        .coordinator
                        .session_manager
                        .checkout_turn(&ctx, AgentId::parse("agent").unwrap(), None, None)
                        .await
                        .unwrap();
                    f.coordinator
                        .session_manager
                        .finish_turn(&ctx, t.generation, &t.op)
                        .await;
                }
                f.backend.effects.lock().unwrap().clear();
                f.backend.fail_prompt.store(failed, Ordering::SeqCst);
                let body = surface(&f, method, &identity, warm.then_some("wire")).await;
                assert_eq!(body.contains("answer"), !failed, "{body}");
                receipts.push(json!({"method":method,"warm":warm,"failed":failed,"body":stable_wire(&body),
                    "effects":*f.backend.effects.lock().unwrap(),"parts":*f.backend.prompt_parts.lock().unwrap()}));
            }
        }
    }
    let receipt = serde_json::to_string_pretty(&receipts).unwrap();
    if let Ok(path) = std::env::var("A2A_GOVERNANCE_WIRE_BASE") {
        assert_eq!(std::fs::read_to_string(path).unwrap(), receipt);
    }
    if let Ok(path) = std::env::var("A2A_GOVERNANCE_WIRE_OUT") {
        std::fs::write(path, receipt).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disconnect_during_admission_keeps_committed_outcome() {
    for turns in [0, 1] {
        use std::sync::atomic::AtomicBool;
        let armed = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let (release, wait) = std::sync::mpsc::channel();
        let (gate, started) = (armed.clone(), entered.clone());
        let wait = std::sync::Mutex::new(wait);
        let store = Arc::new(
            bridge_store::sqlite::SqliteStore::open_in_memory_with_clock(Arc::new(move || {
                if gate.swap(false, Ordering::SeqCst) {
                    started.store(true, Ordering::SeqCst);
                    wait.lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(20))
                        .unwrap();
                }
                bridge_core::task_store::system_wall_now_ms()
            }))
            .unwrap(),
        );
        store.ensure_usage_coverage_v1().await.unwrap();
        let backend = Arc::new(Recording::default());
        let registry = crate::common::single_agent_registry("agent", backend.clone());
        let policy = Arc::new(
            BudgetPolicyV1::new(vec![rule(EachRequest, Some(turns), None)], vec![]).unwrap(),
        );
        let f = Fixture::with_policy_and_store(backend, registry, Some(policy), Some(store));
        let ctx = ContextId::parse("pending").unwrap();
        let sm = &f.coordinator.session_manager;
        let turn = sm
            .checkout_turn(&ctx, AgentId::parse("agent").unwrap(), None, None)
            .await
            .unwrap();
        sm.finish_turn(&ctx, turn.generation, &turn.op).await;
        let before = sm.compaction_snapshot(&ctx).await;
        f.backend.effects.lock().unwrap().clear();
        let identity = owner();
        armed.store(true, Ordering::SeqCst);
        let response = f.router(false).oneshot(axum::http::Request::builder()
            .method("POST").uri("/").header("content-type", "application/json").header("A2A-Version", "1.0")
            .body(axum::body::Body::from(json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"go","contextId":"pending","taskId":identity.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":identity.execution_id.as_str(),"a2a-bridge.attempt_id":identity.attempt_id.as_str()}}}}).to_string())).unwrap()).await.unwrap();
        wait_for(|| entered.load(Ordering::SeqCst)).await;
        drop(response);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if f.store
                    .attempt(&identity.attempt_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .terminal
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(f.backend.effects.lock().unwrap().is_empty());
        assert_eq!(sm.compaction_snapshot(&ctx).await, before);
        assert_eq!(sm.status(&ctx).await.unwrap().state, "idle");
        let usage = f.store.usage_summary_v1(&Default::default()).await.unwrap();
        assert_eq!(usage.buckets[0].dispatched, turns);
    }
}
