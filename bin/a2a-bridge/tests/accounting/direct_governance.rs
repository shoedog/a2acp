use crate::{
    direct::{op, request},
    support::*,
};
use bridge_core::{error::BridgeError, governance::*, ids::*, task_store::TaskStore};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

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
    fixture(
        vec![rule(BudgetSelectorV1::EachRequest, Some(turns), None)],
        None,
    )
}
async fn prompt(
    f: &Fixture,
    context: &str,
) -> Result<bridge_coordinator::coordinator::TurnOutput, BridgeError> {
    f.coordinator
        .prompt_with_identity(op(context), AttemptIdentity::initial().unwrap())
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
async fn mcp(f: &Fixture, tool: &str, owner: &AttemptIdentity, context: &str) -> String {
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
        "input":if tool == "run_workflow" { workflow_input() } else { "go" },"model":if tool == "run_workflow" { None } else { Some("model") },"workflow":(tool == "run_workflow").then_some("fixture-workflow"),"context":context,"execution_id":owner.execution_id.as_str(),"attempt_id":owner.attempt_id.as_str()}}});
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
    owner: &AttemptIdentity,
    context: Option<&str>,
) -> String {
    if method == "Mcp" {
        mcp(f, "run", owner, context.unwrap_or("mcp")).await
    } else {
        request(f.router(false), method, owner, context).await
    }
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
    ] {
        assert!(!body.contains(secret), "{body}");
    }
}

#[tokio::test]
async fn direct_zero_and_exhausted_positive_coverage_and_scope_identity() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(0);
        safe(
            &surface(&f, method, &AttemptIdentity::initial().unwrap(), None).await,
            method,
        );
        reason(
            prompt(&f, "typed").await.err().unwrap(),
            BudgetRefusalReasonV1::LimitExceeded,
        );
        assert!(f.backend.prompts.lock().unwrap().is_empty());
        assert!(reservations(&f).is_empty());
        let f = governed(1);
        assert!(
            surface(&f, method, &AttemptIdentity::initial().unwrap(), None)
                .await
                .contains("answer")
        );
        assert_eq!(reservations(&f).len(), 1);
        let f = fixture(
            vec![
                rule(BudgetSelectorV1::EachRequest, Some(1), None),
                rule(
                    BudgetSelectorV1::UtcDayAgent(AgentId::parse("agent").unwrap()),
                    Some(2),
                    None,
                ),
            ],
            None,
        );
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        conn(&f, true)
            .execute("UPDATE usage_coverage_v1 SET started_ms=0", [])
            .unwrap();
        let mut owners = vec![];
        for _ in 0..2 {
            let owner = AttemptIdentity::initial().unwrap();
            let body = surface(&f, method, &owner, Some("same")).await;
            assert!(body.contains("answer"), "{body}");
            owners.push(owner);
        }
        let saved = reservations(&f);
        assert_eq!(saved.len(), 2);
        assert_ne!(saved[0].context.request, saved[1].context.request);
        for r in &saved {
            assert!(r.context.workflow_run.is_none());
            assert!(owners.iter().any(|o| r.context.request
                == BudgetRequestIdV1::Execution(o.execution_id.clone())
                && r.attempt != o.attempt_id));
        }
        assert_eq!(saved[0].scopes[1].owner, saved[1].scopes[1].owner);
        safe(
            &surface(
                &f,
                method,
                &AttemptIdentity::initial().unwrap(),
                Some("same"),
            )
            .await,
            method,
        );
        let duplicate = surface(&f, method, &owners[0], Some("same")).await;
        assert!(!duplicate.contains("answer"));
        assert_eq!(f.backend.prompts.lock().unwrap().len(), 2);
        assert_eq!(reservations(&f).len(), 2);
        let active: i64 = conn(&f, false)
            .query_row(
                "SELECT count(*) FROM workflow_attempt_summaries WHERE status='active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }
}

#[tokio::test]
async fn actual_candidate_compaction_seed_and_inject() {
    for allowance in ["model", "other"] {
        let f = fixture(
            vec![rule(BudgetSelectorV1::EachRequest, None, Some(10))],
            Some(allowance),
        );
        let ctx = ContextId::parse("retained").unwrap();
        let sm = &f.coordinator.session_manager;
        let turn = sm
            .checkout_turn(
                &ctx,
                AgentId::parse("agent").unwrap(),
                Some(bridge_core::domain::AgentOverride {
                    model: Some("model".into()),
                    ..Default::default()
                }),
                None,
            )
            .await
            .unwrap();
        sm.finish_turn(&ctx, turn.generation, &turn.op).await;
        let owner = AttemptIdentity::initial().unwrap();
        let result = f
            .coordinator
            .continue_turn_with_identity(op("retained"), owner)
            .await;
        if allowance == "other" {
            reason(
                result.err().unwrap(),
                BudgetRefusalReasonV1::MissingAllowance,
            );
            assert!(f.backend.prompts.lock().unwrap().is_empty());
            continue;
        }
        result.unwrap();
        let body = rpc(&f, "SessionCompact", json!({"contextId":"retained"})).await;
        assert!(body.contains("\"compacted\":true"), "{body}");
        assert_eq!(reservations(&f).len(), 2);
        assert!(reservations(&f)
            .iter()
            .all(|r| r.candidate.model.as_deref() == Some("model") && r.terminal_tokens == 10));
        let injected = bridge_coordinator::params::InjectParams::from_mcp_args(
            &json!({"context":"retained","text":"injected marker"}),
        )
        .unwrap();
        f.coordinator.inject(injected.into_request()).await.unwrap();
        assert_eq!(reservations(&f).len(), 2);
        let body = mcp(
            &f,
            "continue",
            &AttemptIdentity::initial().unwrap(),
            "retained",
        )
        .await;
        assert!(body.contains("answer"), "{body}");
        assert_eq!(reservations(&f).len(), 3);
        let parts = f.backend.parts.lock().unwrap();
        let last = parts
            .last()
            .unwrap()
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            last.contains("answer") && last.contains("injected marker"),
            "{last}"
        );
        assert!(f.rows().iter().all(|r| r.finalized));
        let active: i64 = conn(&f, false)
            .query_row(
                "SELECT count(*) FROM workflow_attempt_summaries WHERE status='active'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }
}

#[tokio::test]
async fn token_edges_and_workflow_only_control() {
    for (cap, expected) in [(9, Some(BudgetRefusalReasonV1::LimitExceeded)), (10, None)] {
        let f = fixture(
            vec![rule(BudgetSelectorV1::EachRequest, None, Some(cap))],
            Some("model"),
        );
        let result = prompt(&f, "tokens").await;
        if let Some(expected) = expected {
            reason(result.err().unwrap(), expected);
        } else {
            result.unwrap();
        }
        assert_eq!(reservations(&f).len(), usize::from(cap == 10));
        assert_eq!(
            f.backend.prompts.lock().unwrap().len(),
            usize::from(cap == 10)
        );
    }
    let f = fixture(
        vec![rule(BudgetSelectorV1::EachWorkflowRun, Some(0), Some(0))],
        None,
    );
    prompt(&f, "no-workflow").await.unwrap();
    assert!(reservations(&f)[0].scopes.is_empty());
}

#[tokio::test]
async fn lifecycle_failures_preserve_budget_projection_and_liability() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(0);
        conn(&f, false).execute_batch("CREATE TRIGGER refuse_terminal BEFORE UPDATE OF terminal_json ON workflow_attempt_summaries BEGIN SELECT RAISE(ABORT,'terminal failed'); END;").unwrap();
        safe(
            &surface(&f, method, &AttemptIdentity::initial().unwrap(), None).await,
            method,
        );
        assert!(f.backend.prompts.lock().unwrap().is_empty());
        assert!(reservations(&f).is_empty());
    }
    let f = governed(1);
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    conn(&f, true).execute_batch("CREATE TRIGGER refuse_admission BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT,'admission failed'); END;").unwrap();
    reason(
        prompt(&f, "failed").await.err().unwrap(),
        BudgetRefusalReasonV1::AccountingUnavailable,
    );
    assert!(f.backend.prompts.lock().unwrap().is_empty());
    assert!(reservations(&f).is_empty());
}

#[tokio::test]
async fn unsupported_runtime_routes_and_uncapped_controls() {
    for capped in [false, true] {
        let f = if capped { governed(1) } else { Fixture::new() };
        for method in ["SendMessage", "SendStreamingMessage"] {
            let body = request(
                f.router(true),
                method,
                &AttemptIdentity::initial().unwrap(),
                None,
            )
            .await;
            if capped {
                safe(&body, method);
            } else {
                assert!(body.contains("answer"), "{body}");
            }
        }
        let mut p = op("workflow");
        p.agent = None;
        p.model = None;
        p.effort = None;
        p.mode = None;
        p.workflow = Some("missing".into());
        let workflow = f.coordinator.run_workflow(p).await.err().unwrap();
        let batch = f
            .coordinator
            .run_batch(bridge_coordinator::batch::BatchParams {
                workflow: "missing".into(),
                concurrency: None,
                items: vec![],
            })
            .await
            .err()
            .unwrap();
        let recovery = f.coordinator.resume_with_cap(3).await;
        if capped {
            reason(workflow, BudgetRefusalReasonV1::UnsupportedRoute);
            reason(batch, BudgetRefusalReasonV1::UnsupportedRoute);
            reason(
                recovery.err().unwrap(),
                BudgetRefusalReasonV1::UnsupportedRoute,
            );
            reason(
                f.coordinator
                    .prepare_usage_selection(
                        AttemptIdentity::initial().unwrap(),
                        "agent".into(),
                        None,
                        "fanout_local",
                        None,
                    )
                    .await
                    .err()
                    .unwrap(),
                BudgetRefusalReasonV1::UnsupportedRoute,
            );
            assert!(f.backend.prompts.lock().unwrap().is_empty());
        } else {
            assert!(!matches!(workflow, BridgeError::BudgetRefused { .. }));
            assert!(!matches!(batch, BridgeError::BudgetRefused { .. }));
            recovery.unwrap();
        }
    }
}

#[tokio::test]
async fn day_edges_and_unknown_measurement() {
    use bridge_core::{accounting::*, ports::AgentBackend, workflow_history::*};
    use std::sync::atomic::{AtomicI64, Ordering};
    for tokens in [false, true] {
        let clock = Arc::new(AtomicI64::new(86_400_000));
        let time = clock.clone();
        let store = Arc::new(
            bridge_store::sqlite::SqliteStore::open_in_memory_with_clock(Arc::new(move || {
                time.load(Ordering::SeqCst)
            }))
            .unwrap(),
        );
        store.usage_summary_v1(&Default::default()).await.unwrap();
        let target = BudgetCandidateV1 {
            agent: AgentId::parse("agent").unwrap(),
            model: None,
        };
        let policy = BudgetPolicyV1::new(
            vec![rule(
                BudgetSelectorV1::UtcDayAgent(target.agent.clone()),
                (!tokens).then_some(1),
                tokens.then_some(100),
            )],
            if tokens {
                vec![TurnAllowanceV1 {
                    target,
                    terminal_tokens: 10.try_into().unwrap(),
                }]
            } else {
                vec![]
            },
        )
        .unwrap();
        let accounting = AccountingV1 {
            budget_policy: Some(Arc::new(policy)),
            ..AccountingV1::new(store.clone())
        };
        for (index, expected) in [
            None,
            Some(if tokens {
                BudgetRefusalReasonV1::UnknownMeasurement
            } else {
                BudgetRefusalReasonV1::LimitExceeded
            }),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            if index == 2 {
                clock.store(172_800_000, Ordering::SeqCst);
            }
            let owner = AttemptIdentity::initial().unwrap();
            let mut history = bridge_coordinator::admit_direct_attempt_with_history(
                Ok(store.clone()),
                Arc::new(bridge_observ::NoopObserver),
                owner.clone(),
                ExecutionSurface::Mcp,
                "direct",
                "direct",
                fingerprint_workload_shape(b"day"),
                false,
                clock.load(Ordering::SeqCst),
                "dropped",
            )
            .await
            .unwrap();
            let attempt = bridge_core::translator::AccountingSelectionV1::new(
                owner,
                UsageAttributionV1 {
                    agent: "agent".into(),
                    ..Default::default()
                },
            )
            .unwrap()
            .start(&accounting)
            .await
            .unwrap();
            // A finalized turn with no measurement: the conservative liability stays unknown.
            struct MissingUsage;
            #[async_trait::async_trait]
            impl AgentBackend for MissingUsage {
                async fn prompt(
                    &self,
                    _: &SessionId,
                    _: Vec<bridge_core::domain::Part>,
                ) -> Result<bridge_core::ports::BackendStream, BridgeError> {
                    Ok(Box::pin(futures::stream::iter([
                        Ok(bridge_core::ports::Update::Text("summary".into())),
                        Ok(bridge_core::ports::Update::done("end_turn")),
                    ])))
                }
                async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
                    Ok(())
                }
            }
            history.mark_prompt_dispatch().await.unwrap();
            let result = bridge_coordinator::compact::summarize_collect_accounted(
                Arc::new(MissingUsage),
                SessionId::parse("day").unwrap(),
                Some(attempt),
            )
            .await;
            let refused = result.is_err();
            if let Some(expected) = expected {
                reason(result.err().unwrap(), expected);
            } else {
                result.unwrap();
            }
            history
                .finish(
                    if refused { "failed" } else { "completed" },
                    "fixture",
                    refused,
                    "not_needed",
                )
                .await
                .unwrap();
        }
    }
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
            BudgetSelectorV1::UtcDayAgent(AgentId::parse("agent").unwrap()),
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
async fn refused_payload_survives_until_admitted_on_every_surface() {
    use std::sync::atomic::Ordering;
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let (f, now) = timed_fixture(2);
        assert!(surface(
            &f,
            method,
            &AttemptIdentity::initial().unwrap(),
            Some("payload")
        )
        .await
        .contains("answer"));
        assert!(rpc(&f, "SessionCompact", json!({"contextId":"payload"}))
            .await
            .contains("\"compacted\":true"));
        f.coordinator
            .inject(
                bridge_coordinator::params::InjectParams::from_mcp_args(
                    &json!({"context":"payload", "text":"unique injected payload"}),
                )
                .unwrap()
                .into_request(),
            )
            .await
            .unwrap();
        safe(
            &surface(
                &f,
                method,
                &AttemptIdentity::initial().unwrap(),
                Some("payload"),
            )
            .await,
            method,
        );
        assert_eq!(f.backend.prompts.lock().unwrap().len(), 2);
        assert_eq!(
            f.coordinator
                .session_manager
                .status(&ContextId::parse("payload").unwrap())
                .await
                .unwrap()
                .state,
            "idle"
        );
        now.store(172_800_000, Ordering::SeqCst);
        assert!(surface(
            &f,
            method,
            &AttemptIdentity::initial().unwrap(),
            Some("payload")
        )
        .await
        .contains("answer"));
        assert!(surface(
            &f,
            method,
            &AttemptIdentity::initial().unwrap(),
            Some("payload")
        )
        .await
        .contains("answer"));
        let prompts = f.backend.parts.lock().unwrap();
        let text = |index: usize| {
            prompts[index]
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(text(2).contains("answer") && text(2).contains("unique injected payload"));
        assert!(!text(3).contains("answer") && !text(3).contains("unique injected payload"));
    }
}

#[tokio::test]
async fn refused_compaction_preserves_claim_but_provider_failure_expires() {
    use std::sync::atomic::Ordering;
    let (f, now) = timed_fixture(1);
    prompt(&f, "compact-denied").await.unwrap();
    let ctx = ContextId::parse("compact-denied").unwrap();
    let snapshot = f
        .coordinator
        .session_manager
        .compaction_snapshot(&ctx)
        .await;
    let releases = f.backend.releases.lock().unwrap().len();
    safe(
        &rpc(&f, "SessionCompact", json!({"contextId":"compact-denied"})).await,
        "SendMessage",
    );
    assert_eq!(
        f.coordinator
            .session_manager
            .compaction_snapshot(&ctx)
            .await,
        snapshot
    );
    assert_eq!(f.backend.releases.lock().unwrap().len(), releases);
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 1);
    now.store(172_800_000, Ordering::SeqCst);
    f.coordinator
        .continue_turn_with_identity(op("compact-denied"), AttemptIdentity::initial().unwrap())
        .await
        .unwrap();
    let f = governed(1);
    prompt(&f, "failed-summary").await.unwrap();
    f.backend.fail_prompt.store(true, Ordering::SeqCst);
    let body = rpc(&f, "SessionCompact", json!({"contextId":"failed-summary"})).await;
    assert!(!body.contains("\"compacted\":true"));
    assert!(f
        .coordinator
        .session_manager
        .compaction_snapshot(&ContextId::parse("failed-summary").unwrap())
        .await
        .is_none());
    assert_eq!(reservations(&f).len(), 2);
    assert!(f.backend.prompts.lock().unwrap().len() == 2);
    assert_eq!(conn(&f, false).query_row("SELECT count(*) FROM workflow_attempt_summaries WHERE task_class='compaction' AND status='terminal' AND outcome='failed'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

#[tokio::test]
async fn governed_warm_model_change_accounts_intended_candidate() {
    use bridge_core::domain::*;
    for method in ["SendMessage", "SendStreamingMessage"] {
        for mapped in ["model", "old"] {
            let f = fixture(
                vec![rule(BudgetSelectorV1::EachRequest, None, Some(10))],
                Some(mapped),
            );
            // Bypass prompting to establish the old conversation selection, not a budget liability.
            let ctx = ContextId::parse("change-model").unwrap();
            let sm = &f.coordinator.session_manager;
            let t = sm
                .checkout_turn(
                    &ctx,
                    AgentId::parse("agent").unwrap(),
                    Some(AgentOverride {
                        model: Some("old".into()),
                        ..Default::default()
                    }),
                    None,
                )
                .await
                .unwrap();
            sm.finish_turn(&ctx, t.generation, &t.op).await;
            let owner = AttemptIdentity::initial().unwrap();
            let body = request(f.router(false), method, &owner, Some("change-model")).await;
            if mapped == "old" {
                safe(&body, method);
                assert!(f.backend.prompts.lock().unwrap().is_empty());
                assert!(reservations(&f).is_empty());
            } else {
                assert!(body.contains("answer"), "{body}");
                assert_eq!(f.backend.prompts.lock().unwrap().len(), 1);
                assert_eq!(
                    reservations(&f)[0].candidate.model.as_deref(),
                    Some("model")
                );
            }
        }
    }
}

#[tokio::test]
async fn missing_allowance_compaction_and_midday_coverage_refuse_without_prompt() {
    let f = fixture(
        vec![rule(BudgetSelectorV1::EachRequest, None, Some(10))],
        Some("default-only"),
    );
    let ctx = ContextId::parse("unmapped-summary").unwrap();
    let sm = &f.coordinator.session_manager;
    let t = sm
        .checkout_turn(
            &ctx,
            AgentId::parse("agent").unwrap(),
            Some(bridge_core::domain::AgentOverride {
                model: Some("retained".into()),
                ..Default::default()
            }),
            None,
        )
        .await
        .unwrap();
    sm.finish_turn(&ctx, t.generation, &t.op).await;
    let snapshot = sm.compaction_snapshot(&ctx).await;
    safe(
        &rpc(
            &f,
            "SessionCompact",
            json!({"contextId":"unmapped-summary"}),
        )
        .await,
        "SendMessage",
    );
    assert_eq!(sm.compaction_snapshot(&ctx).await, snapshot);
    assert!(f.backend.prompts.lock().unwrap().is_empty());
    let (f, now) = timed_fixture(10);
    now.store(86_400_000 + 43_200_000, std::sync::atomic::Ordering::SeqCst);
    reason(
        prompt(&f, "midday").await.err().unwrap(),
        BudgetRefusalReasonV1::IncompleteCoverage,
    );
    assert!(f.backend.prompts.lock().unwrap().is_empty());
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

#[tokio::test]
async fn governed_disconnect_terminalizes_without_refunding_admission() {
    use std::sync::atomic::Ordering;
    let f = governed(1);
    f.backend.stall.store(true, Ordering::SeqCst);
    let router = f.router(false);
    let owner = AttemptIdentity::initial().unwrap();
    let task =
        tokio::spawn(async move { request(router, "SendStreamingMessage", &owner, None).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while f.backend.prompts.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let active: i64 = conn(&f, false)
                .query_row(
                    "SELECT count(*) FROM workflow_attempt_summaries WHERE status='active'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            if f.backend.stopped.load(Ordering::SeqCst) && active == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(reservations(&f).len(), 1);
    assert!(!f.rows()[0].finalized);
}

fn fixed_router(
    f: &Fixture,
    target: bridge_core::domain::RouteTarget,
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> axum::Router {
    struct Route(bridge_core::domain::RouteTarget);
    impl bridge_core::ports::RouteDecision for Route {
        fn route_before_default(
            &self,
            _: &bridge_core::domain::TaskMeta,
        ) -> Result<Option<bridge_core::domain::RouteTarget>, BridgeError> {
            Ok(Some(self.0.clone()))
        }
        fn route(
            &self,
            _: &bridge_core::domain::TaskMeta,
        ) -> Result<bridge_core::domain::RouteTarget, BridgeError> {
            Ok(self.0.clone())
        }
    }
    struct Peer(Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait::async_trait]
    impl bridge_core::ports::DelegationPort for Peer {
        async fn delegate(
            &self,
            _: &bridge_core::domain::AuthContext,
            _: &TaskId,
            _: Vec<bridge_core::domain::Part>,
        ) -> Result<bridge_core::ports::Delegation, BridgeError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(BridgeError::UpstreamA2aError)
        }
        async fn cancel(&self, _: &bridge_core::domain::PeerTaskId) -> Result<(), BridgeError> {
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
    use bridge_core::domain::RouteTarget;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for method in ["SendMessage", "SendStreamingMessage"] {
        for capped in [false, true] {
            let f = if capped { governed(0) } else { Fixture::new() };
            let calls = Arc::new(AtomicUsize::new(0));
            let body = request(
                fixed_router(&f, RouteTarget::Fanout, calls.clone()),
                method,
                &AttemptIdentity::initial().unwrap(),
                None,
            )
            .await;
            if capped {
                safe(&body, method);
            }
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(!capped));
            assert_eq!(
                f.backend.prompts.lock().unwrap().len(),
                usize::from(!capped)
            );
        }
        let f = governed(0);
        let calls = Arc::new(AtomicUsize::new(0));
        let body = request(
            fixed_router(&f, RouteTarget::Delegate, calls.clone()),
            method,
            &AttemptIdentity::initial().unwrap(),
            None,
        )
        .await;
        assert!(!body.contains("budget refused"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(f.backend.prompts.lock().unwrap().is_empty());
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
async fn duplicate_identity_refuses_before_cap_exhaustion() {
    for method in ["SendMessage", "SendStreamingMessage", "Mcp"] {
        let f = governed(2);
        let owner = AttemptIdentity::initial().unwrap();
        assert!(surface(&f, method, &owner, None).await.contains("answer"));
        let body = surface(&f, method, &owner, None).await;
        assert!(
            !body.contains("answer") && !body.contains("budget refused"),
            "{body}"
        );
        assert_eq!(f.backend.prompts.lock().unwrap().len(), 1);
        assert_eq!(reservations(&f).len(), 1);
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
    use bridge_core::domain::RouteTarget;
    for capped in [false, true] {
        let f = if capped { governed(1) } else { Fixture::new() };
        let p = bridge_coordinator::params::OpParams::from_mcp_args_for_workflow(
            &json!({"workflow":"fixture-workflow", "input":workflow_input()}),
        )
        .unwrap();
        let result = f.coordinator.run_workflow(p).await;
        if capped {
            reason(
                result.err().unwrap(),
                BudgetRefusalReasonV1::UnsupportedRoute,
            );
        } else {
            wait_task(&f, &result.unwrap()).await;
        }
        let batch = f
            .coordinator
            .run_batch(bridge_coordinator::batch::BatchParams {
                workflow: "fixture-workflow".into(),
                concurrency: None,
                items: vec![bridge_core::task_store::BatchItem {
                    item_id: "one".into(),
                    input: workflow_input().into(),
                    session_cwd: None,
                }],
            })
            .await;
        if capped {
            reason(
                batch.err().unwrap(),
                BudgetRefusalReasonV1::UnsupportedRoute,
            );
        } else {
            let id = batch.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if f.coordinator.batch_status(&id).await.unwrap().status
                        != bridge_core::task_store::BatchStatus::Working
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        for method in ["SendMessage", "SendStreamingMessage"] {
            let owner = AttemptIdentity::initial().unwrap();
            let body = rpc_on(fixed_router(&f, RouteTarget::Workflow(WorkflowId::parse("fixture-workflow").unwrap()), Arc::new(Default::default())), method,
                json!({"message":{"text":workflow_input(),"taskId":owner.execution_id.as_str(),"metadata":{"a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str()}}})).await;
            if capped {
                safe(&body, "SendMessage");
            } else {
                wait_task(&f, &TaskId::parse(owner.execution_id.as_str()).unwrap()).await;
            }
        }
        let owner = AttemptIdentity::initial().unwrap();
        let body = mcp(&f, "run_workflow", &owner, "workflow").await;
        if capped {
            safe(&body, "Mcp");
            assert!(f.backend.prompts.lock().unwrap().is_empty());
        } else {
            wait_task(&f, &TaskId::parse(owner.execution_id.as_str()).unwrap()).await;
            assert_eq!(f.backend.prompts.lock().unwrap().len(), 5);
        }
    }
}

#[tokio::test]
async fn governed_compaction_preparation_refuses_selection_drift() {
    let f = governed(1);
    prompt(&f, "drift").await.unwrap();
    let sm = &f.coordinator.session_manager;
    let ctx = ContextId::parse("drift").unwrap();
    let snapshot = sm.compaction_snapshot(&ctx).await;
    let selected = snapshot.as_ref().unwrap();
    let mut usage = f
        .coordinator
        .prepare_usage_selection(
            AttemptIdentity::initial().unwrap(),
            selected.agent.as_str().into(),
            Some(selected.config.clone()),
            "compaction",
            None,
        )
        .await
        .unwrap();
    let mut overrides = op("drift").agent_override();
    overrides.model = Some("changed".into());
    let turn = sm
        .checkout_turn(
            &ctx,
            AgentId::parse("agent").unwrap(),
            Some(overrides),
            None,
        )
        .await
        .unwrap();
    sm.finish_turn(&ctx, turn.generation, &turn.op).await;
    let result = sm
        .compact_session_prepared(&ctx, &snapshot, |backend, session| {
            bridge_coordinator::compact::summarize_collect_accounted(backend, session, usage.take())
        })
        .await;
    let failure =
        bridge_core::translator::settle_usage_preparation_v1(result.map_err(Into::into), usage)
            .await
            .err()
            .unwrap();
    assert_eq!(
        failure.original,
        BridgeError::ConfigMismatch {
            field: "accounting selection"
        }
    );
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 1);
    assert_eq!(reservations(&f).len(), 1);
    assert_eq!(sm.status(&ctx).await.unwrap().state, "idle");
    assert!(f.rows().iter().all(|r| r.finalized));
}
