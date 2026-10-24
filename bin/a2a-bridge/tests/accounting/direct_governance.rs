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
    let response = f
        .router(false)
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
        "input":"go","model":"model","context":context,"execution_id":owner.execution_id.as_str(),"attempt_id":owner.attempt_id.as_str()}}});
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
                Some(1),
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
            if let Some(expected) = expected {
                reason(result.err().unwrap(), expected);
            } else {
                result.unwrap();
            }
            history
                .finish("completed", "fixture", false, "not_needed")
                .await
                .unwrap();
        }
    }
}
