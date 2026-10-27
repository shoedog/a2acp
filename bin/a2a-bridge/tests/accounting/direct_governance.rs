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
        if method != "Mcp" {
            // Producer exit evicts asynchronously; test durable identity collision only
            // after the live-binding UnsupportedRoute guard no longer applies.
            wait_binding(&f, &identity, false).await;
        }
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

// A new attempt for the same cold task, with an independently admitted selection.
fn followup(first: &AttemptIdentity) -> AttemptIdentity {
    AttemptIdentity {
        execution_id: first.execution_id.clone(),
        ..owner()
    }
}
fn cold_params(identity: &AttemptIdentity, task: &AttemptIdentity, model: &str) -> Value {
    json!({"message":{"text":"go","taskId":task.execution_id.as_str(),"metadata":{
        "a2a-bridge.execution_id":identity.execution_id.as_str(),
        "a2a-bridge.attempt_id":identity.attempt_id.as_str(),"a2a-bridge.model":model}}})
}
async fn cold_response(
    router: axum::Router,
    method: &str,
    identity: &AttemptIdentity,
    task: &AttemptIdentity,
    model: &str,
) -> axum::response::Response {
    router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(
                    json!({"jsonrpc":"2.0","id":1,"method":method,
            "params":cold_params(identity, task, model)})
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn response_text(response: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn wait_binding(f: &Fixture, identity: &AttemptIdentity, present: bool) {
    let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.coordinator.bindings().lock().await.contains_key(&task) != present {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
async fn assert_cold_refusal(
    f: &Fixture,
    router: axum::Router,
    method: &str,
    identity: &AttemptIdentity,
) {
    let effects = f.backend.effects.lock().unwrap().clone();
    let forgets = f.backend.forgets.lock().unwrap().clone();
    let reserved = reservations(f);
    let usage = count(f, true, "usage_attempts_v1");
    let history = count(f, false, "workflow_attempt_summaries");
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        rpc_on(router, method, cold_params(identity, identity, "good")),
    )
    .await
    .expect("bound follow-up must refuse without waiting for its producer");
    // UnsupportedRoute is intentionally hidden on the wire. With spare cap headroom,
    // a budget refusal before any history/reservation can only be the route guard.
    safe(&body, "SendMessage");
    assert_eq!(reservations(f), reserved);
    assert_eq!(count(f, true, "usage_attempts_v1"), usage);
    assert_eq!(count(f, false, "workflow_attempt_summaries"), history);
    assert_eq!(*f.backend.effects.lock().unwrap(), effects);
    assert_eq!(*f.backend.forgets.lock().unwrap(), forgets);
}

#[tokio::test]
async fn r3_cold_bound_reuse_is_unsupported() {
    for capped in [true, false] {
        for deferred in [false, true] {
            let f = if capped { governed(10) } else { Fixture::new() };
            let router = fixed_router(
                &f,
                RouteTarget::Local(AgentId::parse("agent").unwrap()),
                Arc::new(Default::default()),
                deferred,
            );
            let first = owner();
            let task = owner();
            let (release, gate) = tokio::sync::oneshot::channel();
            *f.backend.prompt_gate.lock().unwrap() = Some(gate);
            let response = cold_response(
                router.clone(),
                "SendStreamingMessage",
                &first,
                &task,
                "model",
            )
            .await;
            wait_for(|| prompts(&f) == 1).await;
            wait_binding(&f, &task, true).await;
            for method in ["SendMessage", "SendStreamingMessage"] {
                let next = followup(&task);
                if capped {
                    assert_cold_refusal(&f, router.clone(), method, &next).await;
                } else {
                    let body =
                        rpc_on(router.clone(), method, cold_params(&next, &task, "good")).await;
                    assert!(body.contains("answer"), "{body}");
                    // The uncapped bound branch retains the first request's selection/session.
                    assert_eq!(
                        f.backend
                            .prompt_models
                            .lock()
                            .unwrap()
                            .last()
                            .unwrap()
                            .as_deref(),
                        Some("model")
                    );
                    assert!(f
                        .backend
                        .prompts
                        .lock()
                        .unwrap()
                        .windows(2)
                        .all(|p| p[0] == p[1]));
                }
            }
            if capped {
                assert_eq!(reservations(&f).len(), 1);
            }
            release.send(()).unwrap();
            assert!(response_text(response).await.contains("answer"));
            wait_binding(&f, &task, false).await;
            let body = rpc_on(
                router,
                "SendStreamingMessage",
                cold_params(&owner(), &task, "good"),
            )
            .await;
            assert!(body.contains("answer"), "{body}");
            assert_eq!(
                f.backend
                    .prompt_models
                    .lock()
                    .unwrap()
                    .last()
                    .unwrap()
                    .as_deref(),
                Some("good")
            );
            if capped {
                assert_eq!(reservations(&f).len(), 2);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r3_cold_successor_sequence_is_safe() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let f = governed(10);
        let first = owner();
        let task = owner();
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = entered.clone();
        let (release, gate) = std::sync::mpsc::channel();
        *f.backend.before_turn.lock().unwrap() = Some(Box::new(move || {
            observed.store(true, Ordering::SeqCst);
            let _ = gate.recv_timeout(std::time::Duration::from_secs(20));
        }));
        let response = cold_response(
            f.router(false),
            "SendStreamingMessage",
            &first,
            &task,
            "model",
        )
        .await;
        wait_for(|| entered.load(Ordering::SeqCst)).await;
        wait_binding(&f, &task, true).await;
        assert_cold_refusal(&f, f.router(false), method, &followup(&task)).await;
        assert_eq!(prompts(&f), 0);
        drop(response); // The first producer disconnects after binding but before prompt.
        release.send(()).unwrap();
        wait_binding(&f, &task, false).await;
        let next = followup(&task);
        let body = rpc_on(f.router(false), method, cold_params(&next, &task, "good")).await;
        assert!(body.contains("answer"), "{body}");
        assert_eq!(
            *f.backend.prompt_models.lock().unwrap(),
            [Some("good".into())]
        );
        assert_eq!(prompts(&f), 1);
        let reserved = reservations(&f);
        assert_eq!(reserved.len(), 2); // The disconnected admission is never refunded.
        assert_eq!(
            reserved
                .iter()
                .filter(|r| r.candidate.model.as_deref() == Some("good"))
                .count(),
            1
        );
        let rows = f.rows();
        let row = rows
            .iter()
            .find(|r| r.attribution.run_attempt.as_deref() == Some(next.attempt_id.as_str()))
            .unwrap();
        assert_eq!(row.attribution.effective_model.as_deref(), Some("good"));
    }
}

// Pause each fresh resolution after the request seam, before admission/apply.
struct ColdResolveGates {
    inner: Arc<dyn AgentRegistry>,
    gates: std::sync::Mutex<std::collections::VecDeque<tokio::sync::oneshot::Receiver<()>>>,
    entered: AtomicUsize,
}
#[async_trait::async_trait]
impl AgentRegistry for ColdResolveGates {
    async fn resolve(&self, id: &AgentId) -> Result<Resolved, BridgeError> {
        let gate = self.gates.lock().unwrap().pop_front();
        self.entered.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = gate {
            let _ = gate.await;
        }
        self.inner.resolve(id).await
    }
    fn default_id(&self) -> AgentId {
        self.inner.default_id()
    }
    fn entry_snapshot(&self, id: &AgentId) -> Option<Arc<AgentEntry>> {
        self.inner.entry_snapshot(id)
    }
    fn configured_effective(&self, id: &AgentId) -> Option<EffectiveConfig> {
        self.inner.configured_effective(id)
    }
    async fn apply(&self, snapshot: RegistrySnapshot) -> Result<(), BridgeError> {
        self.inner.apply(snapshot).await
    }
    fn list(&self) -> Vec<AgentId> {
        self.inner.list()
    }
}

#[tokio::test]
async fn r3_concurrent_fresh_same_task() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let backend = Arc::new(Recording::default());
        let (first_ready, first_gate) = tokio::sync::oneshot::channel();
        let (second_ready, second_gate) = tokio::sync::oneshot::channel();
        let registry = Arc::new(ColdResolveGates {
            inner: crate::common::single_agent_registry("agent", backend.clone()),
            gates: std::sync::Mutex::new([first_gate, second_gate].into()),
            entered: AtomicUsize::new(0),
        });
        let policy =
            Arc::new(BudgetPolicyV1::new(vec![rule(EachRequest, Some(10), None)], vec![]).unwrap());
        let f = Fixture::with_policy(backend, registry.clone(), Some(policy));
        // Pending cold incarnations belong to the served adapter, so every
        // competing request must use this same server instance.
        let router = f.router(false);
        let first = owner();
        let task = owner();
        let next = followup(&task);
        let (configured, configure_gate) = tokio::sync::oneshot::channel();
        *f.backend.configure_gate.lock().unwrap() = Some(configure_gate);
        let (finish, prompt_gate) = tokio::sync::oneshot::channel();
        *f.backend.prompt_gate.lock().unwrap() = Some(prompt_gate);
        let response = cold_response(
            router.clone(),
            "SendStreamingMessage",
            &first,
            &task,
            "model",
        )
        .await;
        wait_for(|| registry.entered.load(Ordering::SeqCst) == 1).await;
        let second_router = router.clone();
        let second_identity = next.clone();
        let second = tokio::spawn(async move {
            rpc_on(
                second_router,
                method,
                cold_params(&second_identity, &second_identity, "good"),
            )
            .await
        });
        wait_for(|| registry.entered.load(Ordering::SeqCst) == 2).await;
        first_ready.send(()).unwrap();
        wait_for(|| f.backend.effects.lock().unwrap().contains(&"configure")).await;
        // Both requests passed the no-binding seam. The second commits admission while
        // the first owns the pending incarnation and is still configuring.
        second_ready.send(()).unwrap();
        wait_for(|| reservations(&f).len() == 2).await;
        assert_eq!(*f.backend.effects.lock().unwrap(), ["configure"]);
        configured.send(()).unwrap();
        let body = second.await.unwrap();
        assert!(body.contains("config mismatch"), "{body}");
        wait_for(|| prompts(&f) == 1).await;
        assert_eq!(*f.backend.effects.lock().unwrap(), ["configure", "prompt"]);
        assert!(f.backend.forgets.lock().unwrap().is_empty());
        assert_eq!(f.backend.configs.lock().unwrap().len(), 1);
        assert_eq!(
            f.backend.configs.lock().unwrap()[0].config.model.as_deref(),
            Some("model")
        );
        let mut candidates: Vec<_> = reservations(&f)
            .into_iter()
            .map(|r| r.candidate.model.unwrap())
            .collect();
        candidates.sort();
        assert_eq!(candidates, ["good", "model"]);
        // A third arrival sees the published winner and must take the pre-admission refusal.
        assert_cold_refusal(&f, router, method, &followup(&task)).await;
        finish.send(()).unwrap();
        assert!(response_text(response).await.contains("answer"));
        wait_binding(&f, &task, false).await;
        assert_eq!(*f.backend.effects.lock().unwrap(), ["configure", "prompt"]);
        assert_eq!(f.backend.forgets.lock().unwrap().len(), 1);
        assert_eq!(
            *f.backend.prompt_models.lock().unwrap(),
            [Some("model".into())]
        );
        assert_eq!(reservations(&f).len(), 2);
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
                            abort: Default::default(),
                            session: SessionId::parse("control").unwrap(),
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
                let bound_refusal =
                    state == "bound" && matches!(method, "SendMessage" | "SendStreamingMessage");
                let admitted =
                    turns != 0 && !bound_refusal && (method != "Continue" || state == "warm");
                if bound_refusal {
                    safe(&body, "SendMessage");
                    assert_eq!(count(&f, false, "workflow_attempt_summaries"), 0);
                    assert!(f.backend.forgets.lock().unwrap().is_empty());
                }
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
    config_reload_cleanup(false).await;
}

#[tokio::test]
async fn failed_cleanup_retains_binding() {
    config_reload_cleanup(true).await;
}

async fn config_reload_cleanup(failed_only: bool) {
    use futures::FutureExt;
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        for (state, failed_cleanup) in [
            ("warm", false),
            ("warm", true),
            ("fresh", false),
            ("fresh", true),
        ] {
            if (method == "Mcp" && state != "warm") || (failed_only && !failed_cleanup) {
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
                if state == "fresh" {
                    wait_binding(&f, &identity, failed_cleanup && !during_apply).await;
                    if failed_cleanup && !during_apply {
                        let task = TaskId::parse(identity.execution_id.as_str()).unwrap();
                        assert!(f.coordinator.bindings().lock().await[&task]
                            .lease
                            .is_retired());
                        assert_cold_refusal(&f, f.router(false), method, &identity).await;
                    }
                }
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

async fn cold_acp(capped: bool, turns: u64) -> (Fixture, Arc<crate::cold_acp_fixture::ColdAcp>) {
    let (rec, backend) = crate::cold_acp_fixture::ColdAcp::connect().await;
    let registry = crate::common::single_agent_registry("agent", backend);
    let policy = capped.then(|| {
        Arc::new(
            BudgetPolicyV1::new(
                vec![rule(
                    if turns == 1 {
                        UtcDayAgent(AgentId::parse("agent").unwrap())
                    } else {
                        EachRequest
                    },
                    Some(turns),
                    None,
                )],
                vec![],
            )
            .unwrap(),
        )
    });
    let store = (capped && turns == 1).then(|| {
        Arc::new(
            bridge_store::sqlite::SqliteStore::open_in_memory_with_clock(Arc::new(|| 86_400_000))
                .unwrap(),
        )
    });
    (
        Fixture::with_policy_and_store(Arc::new(Recording::default()), registry, policy, store),
        rec,
    )
}
async fn cold_repeat(next_model: &str) {
    for capped in [false, true] {
        for first_method in ["SendMessage", "SendStreamingMessage"] {
            for method in ["SendMessage", "SendStreamingMessage"] {
                // Unary owns the task execution; its next independent attempt uses SSE.
                if first_method == "SendMessage" && method == "SendMessage" {
                    continue;
                }
                let (f, rec) = cold_acp(capped, 10).await;
                let task = owner();
                for (index, model) in ["model", next_model].into_iter().enumerate() {
                    let transport = if index == 0 { first_method } else { method };
                    let identity = if transport == "SendMessage" {
                        followup(&task)
                    } else {
                        owner()
                    };
                    let mut params = cold_params(&identity, &task, model);
                    params["message"]["metadata"]["a2a-bridge.effort"] = json!("low");
                    params["message"]["metadata"]["a2a-bridge.mode"] = json!("ask");
                    let body = rpc_on(f.router(false), transport, params).await;
                    assert!(body.contains("answer"), "{body}");
                    assert!(!body.contains("cold-turn_"), "backend key leaked: {body}");
                    wait_binding(&f, &task, false).await;
                }
                let prompts = rec.prompts.lock().unwrap().clone();
                assert_eq!(prompts.len(), 2);
                assert_ne!(prompts[0].0, prompts[1].0, "{method}, capped={capped}");
                assert_eq!(prompts[1].1.model.as_deref(), Some(next_model));
                assert_eq!(prompts[1].1.effort, Some(Effort::Low));
                assert_eq!(prompts[1].1.mode.as_deref(), Some("ask"));
                assert!(rec.teardown.lock().unwrap().is_empty());
                let rows = f.rows();
                assert!(rows
                    .iter()
                    .any(|r| r.attribution.effective_model.as_deref() == Some(next_model)));
                if capped {
                    let saved = reservations(&f);
                    assert_eq!(saved.len(), 2);
                    assert!(saved
                        .iter()
                        .any(|r| r.candidate.model.as_deref() == Some(next_model)));
                }
                let before = rec.sessions.lock().unwrap().len();
                let body = rpc(&f, "GetTask", json!({"id":task.execution_id.as_str()})).await;
                assert!(body.contains(task.execution_id.as_str()), "{body}");
                assert_eq!(rec.sessions.lock().unwrap().len(), before);
            }
        }
    }
}
#[tokio::test]
async fn cold_reuse_after_eviction() {
    cold_repeat("good").await;
}
#[tokio::test]
async fn cold_same_selection_repeat() {
    cold_repeat("model").await;
}

#[tokio::test]
async fn exhausted_budget_after_eviction() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let (f, rec) = cold_acp(true, 1).await;
        let task = owner();
        assert!(rpc_on(
            f.router(false),
            "SendStreamingMessage",
            cold_params(&owner(), &task, "model")
        )
        .await
        .contains("answer"));
        wait_binding(&f, &task, false).await;
        let before = rec.sessions.lock().unwrap().clone();
        safe(
            &rpc_on(
                f.router(false),
                method,
                cold_params(&followup(&task), &task, "good"),
            )
            .await,
            method,
        );
        assert_eq!(*rec.sessions.lock().unwrap(), before);
        assert_eq!(rec.prompts.lock().unwrap().len(), 1);
        assert!(rec.teardown.lock().unwrap().is_empty());
        assert_eq!(
            f.store
                .usage_summary_v1(&Default::default())
                .await
                .unwrap()
                .buckets
                .iter()
                .map(|b| b.dispatched)
                .sum::<u64>(),
            1
        );
    }
}

#[tokio::test]
async fn outliving_follow_up() {
    for first_method in ["SendMessage", "SendStreamingMessage"] {
        for park in ["in_prompt", "before_configure", "after_configure"] {
            let delayed = park != "in_prompt";
            let (f, rec) = cold_acp(false, 10).await;
            let task = owner();
            let task_id = TaskId::parse(task.execution_id.as_str()).unwrap();
            let (finish, gate) = tokio::sync::oneshot::channel();
            let (finish_f, gate_f) = tokio::sync::oneshot::channel();
            *rec.gates.lock().unwrap() = if delayed {
                vec![Some(gate), None, Some(gate_f)]
            } else {
                vec![Some(gate), Some(gate_f), None]
            };
            let cwd = f.dir.path().canonicalize().unwrap();
            let identity = if first_method == "SendMessage" {
                followup(&task)
            } else {
                owner()
            };
            let mut params = cold_params(&identity, &task, "model");
            params["message"]["metadata"]["a2a-bridge.cwd"] = json!(cwd);
            let router = f.router(false);
            let first = tokio::spawn(async move { rpc_on(router, first_method, params).await });
            wait_for(|| rec.prompts.lock().unwrap().len() == 1).await;
            let old_key = f.store.session_for(&task_id).await.unwrap().unwrap();
            let (backend, lease) = {
                let map = f.coordinator.bindings();
                let bindings = map.lock().await;
                let b = &bindings[&task_id];
                (b.backend.clone(), Arc::downgrade(&b.lease))
            };
            // Park on either side of the actual configure, always before the prompt poll.
            let (release, gate) = tokio::sync::oneshot::channel();
            let mut release = Some(release);
            let (entered, configured) = tokio::sync::oneshot::channel();
            let wrapper = Arc::new(BoundConfigureGate {
                backend: backend.clone(),
                after_configure: park == "after_configure",
                entered: std::sync::Mutex::new(Some(entered)),
                gate: std::sync::Mutex::new(Some(gate)),
            });
            f.coordinator
                .bindings()
                .lock()
                .await
                .get_mut(&task_id)
                .unwrap()
                .backend = wrapper;
            let router = f.router(false);
            let next = owner();
            let request_task = task.clone();
            let mut params = cold_params(&next, &request_task, "good");
            params["message"]["metadata"]["a2a-bridge.cwd"] = json!(cwd);
            let follow =
                tokio::spawn(async move { rpc_on(router, "SendStreamingMessage", params).await });
            configured.await.unwrap();
            if !delayed {
                release.take().unwrap().send(()).unwrap();
            }
            finish.send(()).unwrap();
            assert!(first.await.unwrap().contains("answer"));
            wait_binding(&f, &task, false).await;
            assert!(
                lease.strong_count() > 0,
                "F alone must retain the evicted lease"
            );
            let (finish_b, gate_b) = tokio::sync::oneshot::channel();
            rec.gates.lock().unwrap().resize_with(3, || None);
            let successor_index = if delayed { 1 } else { 2 };
            if !delayed {
                wait_for(|| rec.prompts.lock().unwrap().len() == 2).await;
            }
            rec.gates.lock().unwrap()[successor_index] = Some(gate_b);
            let b = cold_response(
                f.router(false),
                "SendStreamingMessage",
                &owner(),
                &task,
                "good",
            )
            .await;
            wait_for(|| rec.prompts.lock().unwrap().len() > successor_index).await;
            let new_key = f.store.session_for(&task_id).await.unwrap().unwrap();
            assert_ne!(old_key, new_key);
            let stale = bridge_coordinator::dispatch::BindingGuard {
                bindings: f.coordinator.bindings().clone(),
                task: task_id.clone(),
                backend: Arc::new(bridge_coordinator::dispatch::ColdBindingLease::new(
                    f.backend.clone(),
                    old_key.clone(),
                    Arc::new(UntrackedLease),
                )),
                session: old_key.clone(),
            };
            drop(stale);
            wait_for(|| f.backend.forgets.lock().unwrap().len() == 1).await;
            assert_eq!(f.backend.forgets.lock().unwrap()[0], old_key);
            if delayed {
                release.take().unwrap().send(()).unwrap();
            }
            wait_for(|| rec.prompts.lock().unwrap().len() == 3 || follow.is_finished()).await;
            finish_f.send(()).unwrap();
            let body = follow.await.unwrap();
            assert!(body.contains("answer"), "{park}, {first_method}: {body}");
            wait_for(|| lease.strong_count() == 0).await;
            assert_eq!(
                f.coordinator.bindings().lock().await[&task_id]
                    .eff
                    .model
                    .as_deref(),
                Some("good")
            );
            assert_eq!(f.store.session_for(&task_id).await.unwrap(), Some(new_key));
            let prompts = rec.prompts.lock().unwrap().clone();
            let follow_index = if delayed { 2 } else { 1 };
            assert_eq!(
                prompts[0].0, prompts[follow_index].0,
                "F must use A's exact ACP session"
            );
            assert_ne!(prompts[0].0, prompts[successor_index].0);
            assert_eq!(
                prompts
                    .iter()
                    .filter(|(_, cfg)| cfg.model.as_deref() == Some("model"))
                    .count(),
                2
            );
            assert_eq!(
                prompts
                    .iter()
                    .filter(|(_, cfg)| cfg.model.as_deref() == Some("good"))
                    .count(),
                1
            );
            assert!(rec.teardown.lock().unwrap().is_empty());
            finish_b.send(()).unwrap();
            assert!(response_text(b).await.contains("answer"));
            wait_binding(&f, &task, false).await;
        }
    }
}

struct UntrackedLease;
impl Lease for UntrackedLease {}

struct BoundConfigureGate {
    after_configure: bool,
    backend: Arc<dyn AgentBackend>,
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    gate: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
#[async_trait::async_trait]
impl AgentBackend for BoundConfigureGate {
    async fn configure_session(
        &self,
        id: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        if self.after_configure {
            self.backend.configure_session(id, spec).await?;
        }
        self.entered
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        let gate = self.gate.lock().unwrap().take().unwrap();
        gate.await.unwrap();
        if self.after_configure {
            Ok(())
        } else {
            self.backend.configure_session(id, spec).await
        }
    }
    async fn prompt(&self, id: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        self.backend.prompt(id, parts).await
    }
    async fn cancel(&self, id: &SessionId) -> Result<(), BridgeError> {
        self.backend.cancel(id).await
    }
}

#[tokio::test]
async fn cancel_after_rebinding() {
    for capped in [false, true] {
        let (f, rec) = cold_acp(capped, 10).await;
        let task = owner();
        let id = TaskId::parse(task.execution_id.as_str()).unwrap();
        assert!(rpc_on(
            f.router(false),
            "SendStreamingMessage",
            cold_params(&owner(), &task, "model")
        )
        .await
        .contains("answer"));
        wait_binding(&f, &task, false).await;
        let old_key = f.store.session_for(&id).await.unwrap().unwrap();
        let (finish, gate) = tokio::sync::oneshot::channel();
        *rec.gates.lock().unwrap() = vec![None, Some(gate)];
        let next = cold_response(
            f.router(false),
            "SendStreamingMessage",
            &owner(),
            &task,
            "good",
        )
        .await;
        wait_for(|| rec.prompts.lock().unwrap().len() == 2).await;
        let new_key = f.store.session_for(&id).await.unwrap().unwrap();
        // Model an older mapping read racing the successor's publication.
        f.store.put(&id, &old_key).await.unwrap();
        let body = rpc(&f, "CancelTask", json!({"id":id.as_str()})).await;
        assert!(body.contains("CANCELED"), "{body}");
        wait_for(|| !rec.teardown.lock().unwrap().is_empty()).await;
        let expected = format!("cancel:{}", rec.prompts.lock().unwrap()[1].0);
        assert_eq!(*rec.teardown.lock().unwrap(), [expected]);
        assert_ne!(old_key, new_key);
        finish.send(()).unwrap();
        response_text(next).await;
        wait_binding(&f, &task, false).await;
    }
}

#[tokio::test]
async fn concurrent_fresh_keys() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        let (rec, backend) = crate::cold_acp_fixture::ColdAcp::connect().await;
        let (ready_a, gate_a) = tokio::sync::oneshot::channel();
        let (ready_b, gate_b) = tokio::sync::oneshot::channel();
        let registry = Arc::new(ColdResolveGates {
            inner: crate::common::single_agent_registry("agent", backend),
            gates: Default::default(),
            entered: AtomicUsize::new(0),
        });
        let policy =
            Arc::new(BudgetPolicyV1::new(vec![rule(EachRequest, Some(10), None)], vec![]).unwrap());
        let f = Fixture::with_policy(
            Arc::new(Recording::default()),
            registry.clone(),
            Some(policy),
        );
        let task = owner();
        let id = TaskId::parse(task.execution_id.as_str()).unwrap();
        assert!(rpc_on(
            f.router(false),
            "SendStreamingMessage",
            cold_params(&owner(), &task, "model")
        )
        .await
        .contains("answer"));
        wait_binding(&f, &task, false).await;
        let previous = f.store.session_for(&id).await.unwrap().unwrap();
        registry.gates.lock().unwrap().extend([gate_a, gate_b]);
        registry.entered.store(0, Ordering::SeqCst);
        let (finish, gate) = tokio::sync::oneshot::channel();
        *rec.gates.lock().unwrap() = vec![None, Some(gate)];
        let a = cold_response(
            f.router(false),
            "SendStreamingMessage",
            &owner(),
            &task,
            "good",
        )
        .await;
        wait_for(|| registry.entered.load(Ordering::SeqCst) == 1).await;
        let router = f.router(false);
        let next = followup(&task);
        let request_task = task.clone();
        let b = tokio::spawn(async move {
            rpc_on(router, method, cold_params(&next, &request_task, "model")).await
        });
        wait_for(|| registry.entered.load(Ordering::SeqCst) == 2).await;
        ready_a.send(()).unwrap();
        wait_for(|| rec.prompts.lock().unwrap().len() == 2).await;
        let key = f.store.session_for(&id).await.unwrap().unwrap();
        ready_b.send(()).unwrap();
        assert!(b.await.unwrap().contains("config mismatch"));
        assert_eq!(reservations(&f).len(), 3);
        assert_eq!(f.store.session_for(&id).await.unwrap(), Some(key.clone()));
        assert_ne!(key, previous);
        assert_eq!(rec.sessions.lock().unwrap().len(), 2);
        assert_eq!(rec.prompts.lock().unwrap().len(), 2);
        assert_eq!(
            rec.prompts.lock().unwrap()[1].1.model.as_deref(),
            Some("good")
        );
        assert!(rec.teardown.lock().unwrap().is_empty());
        finish.send(()).unwrap();
        assert!(response_text(a).await.contains("answer"));
        wait_binding(&f, &task, false).await;
    }
}

async fn capped_configure_case(cancel: bool, fail_config: bool, persistence: Option<&str>) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        for method in ["SendMessage", "SendStreamingMessage"] {
            let f = governed(1);
            f.backend.fail_config.store(fail_config, Ordering::SeqCst);
            let router = f.router(false);
            let identity = owner();
            let (release, gate) = tokio::sync::oneshot::channel();
            *f.backend.configure_gate.lock().unwrap() = Some(gate);
            let params = cold_params(&identity, &identity, "model");
            let request = tokio::spawn(rpc_on(router.clone(), method, params));
            wait_for(|| !f.backend.configured_sessions.lock().unwrap().is_empty()).await;
            let session = f.backend.configured_sessions.lock().unwrap()[0].0.clone();
            assert_eq!(prompts(&f), 0);
            assert_eq!(reservations(&f).len(), 1, "admit precedes configure");
            if cancel {
                let body = rpc_on(
                    router.clone(),
                    "CancelTask",
                    json!({"id":identity.execution_id.as_str()}),
                )
                .await;
                assert!(body.contains("TASK_STATE_CANCELED"), "{body}");
                assert!(f.backend.effects.lock().unwrap().contains(&"cancel"));
            }
            if let Some(fault) = persistence {
                let table = if fault == "usage" { "usage_attempts_v1" } else { "workflow_attempt_summaries" };
                conn(&f, fault == "usage").execute_batch(&format!(
                    "CREATE TRIGGER refuse_settlement BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT,'settlement failed'); END;"
                )).unwrap();
            }
            release.send(()).unwrap();
            let body = request.await.unwrap();
            if persistence.is_some() || (!cancel && fail_config) {
                if method == "SendMessage" {
                    assert!(serde_json::from_str::<Value>(&body).unwrap()["error"].is_object(), "{body}");
                } else {
                    assert!(body.contains("event: error") && body.contains("TASK_STATE_FAILED"), "{body}");
                }
                assert!(!body.contains("TASK_STATE_CANCELED"), "{body}");
            } else {
                let state = if cancel { "CANCELED" } else { "COMPLETED" };
                assert!(body.contains(&format!("TASK_STATE_{state}")), "{body}");
            }
            assert_eq!(prompts(&f), usize::from(!cancel && !fail_config));
            wait_binding(&f, &identity, false).await;
            let rows = f.rows();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].finalized, persistence != Some("usage"));
            // Admission conservatively marks usage dispatched; pre-prompt
            // cancellation finalizes that reservation without provider actuals.
            assert!(rows[0].dispatched);
            assert_eq!(rows[0].usage.terminal.is_none(), cancel || fail_config);
            if cancel || fail_config {
                assert_eq!(*f.backend.releases.lock().unwrap(), [session]);
            }
            let saved = f.store.attempt(&identity.attempt_id).await.unwrap().unwrap();
            if persistence == Some("terminal") {
                assert!(saved.terminal.is_none());
                continue;
            }
            let terminal = saved.terminal.unwrap();
            assert_eq!(
                terminal.outcome,
                if cancel { "canceled" } else if fail_config { "failed" } else { "completed" }
            );
            if cancel || fail_config {
                assert_eq!(terminal.terminal_reason, "pre_prompt_failure");
                assert_eq!(terminal.prompt_acceptance, "not_dispatched");
                assert_eq!(terminal.cleanup_disposition, "complete");
            }
        }
    })
    .await
    .expect("capped configure/cancel must finish");
}

#[tokio::test]
async fn capped_cancel_during_configure() {
    for fail in [false, true] {
        capped_configure_case(true, fail, None).await;
    }
}

#[tokio::test]
async fn capped_no_cancel_control() {
    for fail in [false, true] {
        capped_configure_case(false, fail, None).await;
    }
}

#[tokio::test]
async fn capped_cancel_preserves_persistence_errors() {
    for fault in ["usage", "terminal"] {
        capped_configure_case(true, true, Some(fault)).await;
    }
}

struct CancellableRecording {
    inner: Arc<Recording>,
    aborts:
        std::sync::Mutex<std::collections::HashMap<SessionId, tokio_util::sync::CancellationToken>>,
}

#[async_trait::async_trait]
impl AgentBackend for CancellableRecording {
    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        self.inner.resource_flight_v1()
    }
    async fn configure_session(
        &self,
        s: &SessionId,
        spec: &SessionSpec,
    ) -> Result<(), BridgeError> {
        self.inner.configure_session(s, spec).await
    }
    async fn prompt(&self, s: &SessionId, parts: Vec<Part>) -> Result<BackendStream, BridgeError> {
        let abort = tokio_util::sync::CancellationToken::new();
        self.aborts.lock().unwrap().insert(s.clone(), abort.clone());
        tokio::select! {
            biased;
            _ = abort.cancelled() => Ok(Box::pin(futures::stream::iter([
                Ok(Update::done(STOP_REASON_CANCELLED)),
            ])) as BackendStream),
            result = self.inner.prompt(s, parts) => result,
        }
    }
    async fn cancel(&self, s: &SessionId) -> Result<(), BridgeError> {
        if let Some(abort) = self.aborts.lock().unwrap().get(s) {
            abort.cancel();
        }
        self.inner.cancel(s).await
    }
    async fn forget_session(&self, s: &SessionId) {
        self.aborts.lock().unwrap().remove(s);
        self.inner.forget_session(s).await;
    }
}

#[tokio::test]
async fn capped_configure_does_not_block_others() {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let recording = Arc::new(Recording::default());
        let backend = Arc::new(CancellableRecording {
            inner: recording.clone(),
            aborts: Default::default(),
        });
        let registry = crate::common::single_agent_registry("agent", backend);
        let policy = BudgetPolicyV1::new(vec![rule(EachRequest, Some(10), None)], vec![]).unwrap();
        let f = Fixture::with_policy(recording, registry, Some(Arc::new(policy)));
        let router = f.router(false);
        // B is running; C is another producer that will finish while A configures.
        let b = owner();
        let c = owner();
        let (_finish_b, gate) = tokio::sync::oneshot::channel();
        *f.backend.prompt_gate.lock().unwrap() = Some(gate);
        let response_b =
            cold_response(router.clone(), "SendStreamingMessage", &b, &b, "model").await;
        wait_for(|| prompts(&f) == 1).await;
        let (finish_c, gate) = tokio::sync::oneshot::channel();
        *f.backend.prompt_gate.lock().unwrap() = Some(gate);
        let response_c =
            cold_response(router.clone(), "SendStreamingMessage", &c, &c, "model").await;
        wait_for(|| prompts(&f) == 2).await;
        let session_c = f.backend.configured_sessions.lock().unwrap()[1].0.clone();
        let a = owner();
        let (release_a, gate) = tokio::sync::oneshot::channel();
        *f.backend.configure_gate.lock().unwrap() = Some(gate);
        let request_a = tokio::spawn(rpc_on(
            router.clone(),
            "SendStreamingMessage",
            cold_params(&a, &a, "model"),
        ));
        wait_for(|| f.backend.configured_sessions.lock().unwrap().len() == 3).await;

        let canceled = rpc_on(
            router.clone(),
            "CancelTask",
            json!({"id":b.execution_id.as_str()}),
        )
        .await;
        assert!(canceled.contains("TASK_STATE_CANCELED"), "{canceled}");
        assert!(response_text(response_b)
            .await
            .contains("TASK_STATE_CANCELED"));
        let d = owner();
        let independent =
            rpc_on(router, "SendStreamingMessage", cold_params(&d, &d, "model")).await;
        assert!(
            independent.contains("TASK_STATE_COMPLETED"),
            "{independent}"
        );
        finish_c.send(()).unwrap();
        assert!(response_text(response_c)
            .await
            .contains("TASK_STATE_COMPLETED"));
        wait_binding(&f, &c, false).await;
        wait_for(|| f.backend.forgets.lock().unwrap().contains(&session_c)).await;
        assert_eq!(prompts(&f), 3); // A is still configuring.

        release_a.send(()).unwrap();
        assert!(request_a.await.unwrap().contains("TASK_STATE_COMPLETED"));
        wait_binding(&f, &a, false).await;
        wait_binding(&f, &b, false).await;
    })
    .await
    .expect("unrelated cancel/bind/eviction must finish while capped configure is parked");
}
