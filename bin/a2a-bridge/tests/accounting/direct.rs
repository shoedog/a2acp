use crate::support::*;
use bridge_core::{accounting::*, ids::*, task_store::TaskStore};
use std::sync::atomic::Ordering;

use bridge_core::{
    domain::{RouteTarget, TaskMeta},
    error::BridgeError,
    ports::RouteDecision,
};
use std::sync::Arc;

impl Fixture {
    pub fn router(&self, fanout: bool) -> axum::Router {
        self.router_with_delegation(fanout, Arc::new(bridge_a2a_outbound::StubDelegation))
    }
    fn router_with_delegation(
        &self,
        fanout: bool,
        delegation: Arc<dyn bridge_core::ports::DelegationPort>,
    ) -> axum::Router {
        struct Route(bool);
        impl RouteDecision for Route {
            fn route_before_default(
                &self,
                meta: &TaskMeta,
            ) -> Result<Option<RouteTarget>, BridgeError> {
                self.route(meta).map(Some)
            }
            fn route(&self, _: &TaskMeta) -> Result<RouteTarget, BridgeError> {
                Ok(if self.0 {
                    RouteTarget::Fanout
                } else {
                    RouteTarget::Local(AgentId::parse("agent")?)
                })
            }
        }
        Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
            self.coordinator.clone(),
            Arc::new(Route(fanout)),
            Arc::new(bridge_policy::auth::AlwaysGrant),
            "http://localhost",
            delegation,
            "agent",
        ))
        .router()
    }
}

pub async fn request(
    router: axum::Router,
    method: &str,
    owner: &AttemptIdentity,
    context: Option<&str>,
) -> String {
    use tower::ServiceExt;
    let body = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":method, "params":{
        "message":{"text":"go", "taskId":owner.execution_id.as_str(), "contextId":context,
            "metadata":{"a2a-bridge.model":"model", "account_group":"forged", "billing_mode":"subscription", "a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}}
    }});
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(body.to_string()))
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
pub fn op(context: &str) -> bridge_coordinator::params::OpParams {
    bridge_coordinator::params::OpParams::from_mcp_args(
        &serde_json::json!({"input":"go", "context":context,
        "model":"model", "effort":"high", "mode":"mode"}),
    )
    .unwrap()
}

#[tokio::test]
async fn served_cold_warm_unary_streaming_and_fanout_reopen_exact_actuals() {
    for method in ["SendMessage", "SendStreamingMessage"] {
        for warm in [false, true] {
            let f = Fixture::new();
            let mut owners = Vec::new();
            for _ in 0..2 {
                let owner = AttemptIdentity::initial().unwrap();
                let body = request(f.router(false), method, &owner, warm.then_some("warm")).await;
                assert!(body.contains("answer"), "{body}");
                owners.push(owner);
            }
            {
                let prompts = f.backend.prompts.lock().unwrap();
                assert_eq!(prompts.len(), 2);
                assert_eq!(prompts[0] == prompts[1], warm);
            }
            for owner in owners {
                assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
            }
        }
        let f = Fixture::new();
        let owner = AttemptIdentity::initial().unwrap();
        let body = request(f.router(true), method, &owner, None).await;
        assert!(body.contains("answer"), "{body}");
        assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
    }
}

#[tokio::test]
async fn mcp_run_continue_busy_expired_and_retained_selection() {
    let f = Fixture::new();
    let first = AttemptIdentity::initial().unwrap();
    f.coordinator
        .prompt_with_identity(op("warm"), first.clone())
        .await
        .unwrap();
    let next = AttemptIdentity::initial().unwrap();
    f.coordinator
        .continue_turn_with_identity(op("warm"), next.clone())
        .await
        .unwrap();
    assert_usage(&f.reopened(Some(&first)).await, 1, 0);
    assert_usage(&f.reopened(Some(&next)).await, 1, 0);
    {
        let prompts = f.backend.prompts.lock().unwrap();
        assert_eq!(prompts[0], prompts[1]);
    }
    let ctx = ContextId::parse("warm").unwrap();
    let _busy = f
        .coordinator
        .session_manager
        .checkout_existing_turn(&ctx)
        .await
        .unwrap();
    for (context, error) in [
        ("warm", bridge_core::error::BridgeError::HandleBusy),
        ("expired", bridge_core::error::BridgeError::SessionNotFound),
    ] {
        let owner = AttemptIdentity::initial().unwrap();
        assert!(
            matches!(f.coordinator.continue_turn_with_identity(op(context), owner.clone()).await, Err(e) if e == error)
        );
        let b = f.reopened(Some(&owner)).await;
        assert_eq!((b.attempted, b.dispatched, b.pending), (1, 0, 0));
    }
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn admitted_configuration_failure_and_unavailable_writer_never_prompt() {
    let f = Fixture::new();
    f.backend.fail_config.store(true, Ordering::SeqCst);
    let owner = AttemptIdentity::initial().unwrap();
    assert!(f
        .coordinator
        .prompt_with_identity(op("bad"), owner.clone())
        .await
        .is_err());
    let b = f.reopened(Some(&owner)).await;
    assert_eq!((b.attempted, b.dispatched, b.pending), (1, 0, 0));
    assert!(f.backend.prompts.lock().unwrap().is_empty());
    let missing = AttemptIdentity::initial().unwrap();
    let mut params = op("missing");
    params.agent = Some(AgentId::parse("missing").unwrap());
    assert!(f
        .coordinator
        .prompt_with_identity(params, missing.clone())
        .await
        .is_err());
    let b = f.reopened(Some(&missing)).await;
    assert_eq!((b.attempted, b.dispatched, b.pending), (1, 0, 0));
    f.backend.fail_config.store(false, Ordering::SeqCst);
    // Poison the admitted sidecar after admission, before the next production entry.
    let path = f.dir.path().join("tasks.sqlite.usage-v1.sqlite");
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, "foreign").unwrap();
    let body = request(
        f.router(false),
        "SendStreamingMessage",
        &AttemptIdentity::initial().unwrap(),
        None,
    )
    .await;
    assert!(!body.contains("answer"), "{body}");
    assert!(f.backend.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compaction_usage_has_its_own_scope_and_retained_config() {
    let f = Fixture::new();
    f.coordinator
        .prompt_with_identity(op("warm"), AttemptIdentity::initial().unwrap())
        .await
        .unwrap();
    use tower::ServiceExt;
    let body = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"SessionCompact", "params":{"contextId":"warm"}});
    let response = f
        .router(false)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("compacted"));
    assert_usage(&f.reopened(None).await, 2, 0);
    let rows = f.rows();
    let compact = rows
        .iter()
        .find(|r| r.attribution.attempt_kind == "compaction")
        .unwrap();
    assert_eq!(
        compact.attribution.effective_model.as_deref(),
        Some("model")
    );
    assert_eq!(
        compact.attribution.effective_effort.as_deref(),
        Some("high")
    );
    assert_eq!(compact.attribution.effective_mode.as_deref(), Some("mode"));
    assert_ne!(rows[0].attribution.execution, rows[1].attribution.execution);
    for row in rows {
        assert_ne!(Some(row.id), row.attribution.run_attempt);
    }
}

#[tokio::test]
async fn stopped_consumption_retains_observed_usage_without_completion() {
    let f = Fixture::new();
    f.backend.stall.store(true, Ordering::SeqCst);
    let owner = AttemptIdentity::initial().unwrap();
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(100),
        f.coordinator
            .prompt_with_identity(op("interrupted"), owner.clone())
    )
    .await
    .is_err());
    assert_usage(&f.reopened(Some(&owner)).await, 1, 1);
}

#[tokio::test]
async fn shared_backend_concurrent_executions_keep_filters_isolated() {
    let f = Fixture::new();
    let a = AttemptIdentity::initial().unwrap();
    let b = AttemptIdentity::initial().unwrap();
    let (x, y) = tokio::join!(
        f.coordinator.prompt_with_identity(op("a"), a.clone()),
        f.coordinator.prompt_with_identity(op("b"), b.clone())
    );
    x.unwrap();
    y.unwrap();
    for owner in [a, b] {
        assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
    }
}

#[tokio::test]
async fn selection_hook_precedes_start_and_preserves_typed_failure() {
    use bridge_core::translator::{settle_usage_preparation_v1, AccountingSelectionV1};
    let f = Fixture::new();
    let owner = AttemptIdentity::initial().unwrap();
    let selection = AccountingSelectionV1::new(
        owner.clone(),
        UsageAttributionV1 {
            agent: "candidate".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(f
        .store
        .usage_summary_v1(&Default::default())
        .await
        .unwrap()
        .buckets
        .is_empty());
    let id = selection.concrete.clone();
    let prepared = selection
        .start(&AccountingV1::new(f.store.clone()))
        .await
        .unwrap();
    let original = bridge_core::error::BridgeError::ConfigMismatch { field: "test" };
    assert!(matches!(
        settle_usage_preparation_v1::<()>(Err(original.into()), Some(prepared)).await,
        Err(bridge_core::translator::AccountingFailureV1 {
            original: bridge_core::error::BridgeError::ConfigMismatch { field: "test" },
            ..
        })
    ));
    let subsequent = AccountingSelectionV1::new(owner.clone(), Default::default()).unwrap();
    assert_eq!(subsequent.owner, owner);
    assert_ne!(subsequent.concrete, id);
    assert!(f.backend.prompts.lock().unwrap().is_empty());
}

fn sql(f: &Fixture, usage: bool, statement: &str) {
    sql_at(f.dir.path(), usage, statement);
}
fn sql_at(root: &std::path::Path, usage: bool, statement: &str) {
    let file = if usage {
        "tasks.sqlite.usage-v1.sqlite"
    } else {
        "tasks.sqlite"
    };
    let conn = rusqlite::Connection::open_with_flags(
        root.join(file),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .unwrap();
    if usage {
        conn.execute_batch("PRAGMA journal_mode=PERSIST;").unwrap();
    }
    conn.execute_batch(statement).unwrap();
}

#[tokio::test]
async fn review_bound_followup_uses_the_retained_agent_when_routing_selects_default() {
    use bridge_core::{domain::RegistrySnapshot, ports::AgentRegistry};
    let a = Arc::new(Recording::default());
    let b = Arc::new(Recording::default());
    b.stall.store(true, Ordering::SeqCst);
    let seed = crate::common::single_agent_registry("a", a.clone());
    let entry_a = seed.entry_snapshot(&AgentId::parse("a").unwrap()).unwrap();
    let mut entry_b = (*entry_a).clone();
    entry_b.id = AgentId::parse("b").unwrap();
    let (spawn_a, spawn_b) = (a.clone(), b.clone());
    let registry: Arc<dyn AgentRegistry> = Arc::new(
        bridge_registry::registry::Registry::new(
            RegistrySnapshot {
                default: entry_a.id.clone(),
                entries: vec![(*entry_a).clone(), entry_b],
                allowed_cmds: vec!["test-cmd".into()],
            },
            Arc::new(move |entry| {
                let backend: Arc<dyn bridge_core::ports::AgentBackend> = if entry.id.as_str() == "b"
                {
                    spawn_b.clone()
                } else {
                    spawn_a.clone()
                };
                Box::pin(async move { Ok(backend) })
            }),
        )
        .unwrap(),
    );
    let f = Fixture::with_registry(b.clone(), registry.clone());
    struct Routed(Arc<dyn AgentRegistry>);
    impl RouteDecision for Routed {
        fn route_before_default(
            &self,
            meta: &TaskMeta,
        ) -> Result<Option<RouteTarget>, BridgeError> {
            self.route(meta).map(Some)
        }
        fn route(&self, meta: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Local(
                meta.agent.clone().unwrap_or_else(|| self.0.default_id()),
            ))
        }
    }
    let router = Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
        f.coordinator.clone(),
        Arc::new(Routed(registry)),
        Arc::new(bridge_policy::auth::AlwaysGrant),
        "http://localhost",
        Arc::new(bridge_a2a_outbound::StubDelegation),
        "local",
    ))
    .router();
    use tower::ServiceExt;
    let owner = AttemptIdentity::initial().unwrap();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{
        "text":"start", "taskId":owner.execution_id.as_str(), "metadata":{"a2a-bridge.agent":"b", "a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}
    }}});
    let held_response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/json")
                .header("A2A-Version", "1.0")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while b.prompts.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    b.stall.store(false, Ordering::SeqCst);
    let text = request(router, "SendStreamingMessage", &owner, None).await;
    assert!(text.contains("answer"), "{text}");
    let rows = f.rows();
    assert_eq!(b.prompts.lock().unwrap().len(), 2);
    assert!(a.prompts.lock().unwrap().is_empty());
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.attribution.agent == "b"));
    let reopened = bridge_store::sqlite::SqliteStore::open_history_read_only(
        &f.dir.path().join("tasks.sqlite"),
    )
    .unwrap();
    let query = |agent: &str| UsageQueryV1 {
        agent: Some(agent.into()),
        execution: Some(owner.execution_id.as_str().into()),
        ..Default::default()
    };
    assert!(reopened
        .usage_summary_v1(&query("a"))
        .await
        .unwrap()
        .buckets
        .is_empty());
    assert_eq!(
        reopened
            .usage_summary_v1(&query("b"))
            .await
            .unwrap()
            .buckets[0]
            .total_tokens
            .reported_sum,
        30
    );
    drop(held_response);
}

#[tokio::test]
async fn review_history_barrier_refusals_settle_or_surface_accounting_failure() {
    for surface in ["mcp", "SendMessage", "fanout"] {
        for fail_settlement in [false, true] {
            let f = Fixture::new();
            let root = f.dir.path().to_owned();
            *f.backend.before_config.lock().unwrap() = Some(Box::new(move || {
                sql_at(&root, false, "CREATE TRIGGER refuse_dispatch BEFORE UPDATE OF prompt_acceptance ON workflow_attempt_summaries WHEN NEW.prompt_acceptance='dispatch_uncertain' BEGIN SELECT RAISE(ABORT, 'dispatch refused'); END;");
                if fail_settlement {
                    sql_at(&root, true, "CREATE TRIGGER refuse_settlement BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'settlement refused'); END;");
                }
            }));
            let owner = AttemptIdentity::initial().unwrap();
            if surface == "mcp" {
                let error = f
                    .coordinator
                    .prompt_with_identity(op("barrier"), owner.clone())
                    .await
                    .err()
                    .unwrap();
                if fail_settlement {
                    assert_eq!(error, BridgeError::StoreFailure);
                } else {
                    assert!(matches!(
                        error,
                        BridgeError::DurableEvidenceUnavailable { .. }
                    ));
                }
            } else {
                let body =
                    request(f.router(surface == "fanout"), "SendMessage", &owner, None).await;
                let error = if fail_settlement {
                    "store failure"
                } else {
                    "durable evidence unavailable"
                };
                assert!(body.contains(error), "{surface}: {body}");
            }
            let bucket = f.reopened(Some(&owner)).await;
            assert_eq!(
                (bucket.attempted, bucket.dispatched, bucket.pending),
                (1, 0, u64::from(fail_settlement)),
                "{surface}"
            );
            assert!(f.backend.prompts.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn review_fanout_node_filter_retains_exact_local_usage_without_peer_charge() {
    struct Peer;
    #[async_trait::async_trait]
    impl bridge_core::ports::DelegationPort for Peer {
        async fn delegate(
            &self,
            _: &bridge_core::domain::AuthContext,
            _: &TaskId,
            _: Vec<bridge_core::domain::Part>,
        ) -> Result<bridge_core::ports::Delegation, BridgeError> {
            let mut remote = usage();
            remote.terminal.as_mut().unwrap().total_tokens = 900;
            Ok(bridge_core::ports::Delegation {
                events: Box::pin(futures::stream::iter([
                    Ok(bridge_core::translator::Event::usage(remote)),
                    Ok(bridge_core::translator::Event::artifact("remote answer")),
                ])),
                peer_task: tokio::sync::watch::channel(None).1,
            })
        }
        async fn cancel(&self, _: &bridge_core::domain::PeerTaskId) -> Result<(), BridgeError> {
            Ok(())
        }
    }
    let f = Fixture::new();
    let owner = AttemptIdentity::initial().unwrap();
    let body = request(
        f.router_with_delegation(true, Arc::new(Peer)),
        "SendMessage",
        &owner,
        None,
    )
    .await;
    assert!(body.contains("remote answer"), "{body}");
    let store = bridge_store::sqlite::SqliteStore::open_history_read_only(
        &f.dir.path().join("tasks.sqlite"),
    )
    .unwrap();
    for node in ["agent", "peer", "different"] {
        let buckets = store
            .usage_summary_v1(&UsageQueryV1 {
                execution: Some(owner.execution_id.as_str().into()),
                run_attempt: Some(owner.attempt_id.as_str().into()),
                node: Some(node.into()),
                ..Default::default()
            })
            .await
            .unwrap()
            .buckets;
        if node == "agent" {
            assert_eq!(buckets.len(), 1);
            assert_usage(&buckets[0], 1, 0);
        } else {
            assert!(buckets.is_empty());
        }
    }
    assert_eq!(f.rows().len(), 1);
}

#[tokio::test]
async fn review_missing_accounting_envelope_refuses_while_legacy_entries_work() {
    use bridge_core::{ports::AgentBackend, translator::Translator};
    use futures::StreamExt;
    let f = Fixture::new();
    let session = SessionId::parse("missing").unwrap();
    let task = TaskId::parse("missing").unwrap();
    let translator = Translator::for_turn(f.backend.clone(), None);
    assert!(matches!(
        translator
            .run(
                f.backend.as_ref(),
                f.store.as_ref(),
                &bridge_policy::permission::AutoPolicy,
                &task,
                &session,
                vec![]
            )
            .next()
            .await,
        Some(Err(bridge_core::translator::AccountingFailureV1 {
            original: BridgeError::StoreFailure,
            ..
        }))
    ));
    assert!(matches!(
        bridge_coordinator::compact::summarize_collect_accounted(
            f.backend.clone(),
            session.clone(),
            None
        )
        .await,
        Err(BridgeError::StoreFailure)
    ));
    assert!(f.backend.prompts.lock().unwrap().is_empty());
    assert!(
        bridge_coordinator::compact::summarize_collect(f.backend.clone(), session.clone())
            .await
            .unwrap()
            .contains("answer")
    );
    let mut legacy = f.backend.prompt(&session, vec![]).await.unwrap();
    while let Some(update) = legacy.next().await {
        update.unwrap();
    }
    assert_eq!(f.backend.prompts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn review_concurrent_payloads_and_independent_owner_filters_do_not_swap() {
    let f = Fixture::new();
    let a = AttemptIdentity::initial().unwrap();
    let b = AttemptIdentity::initial().unwrap();
    let mut params = op("double");
    params.input = "double".into();
    let (x, y) = tokio::join!(
        f.coordinator.prompt_with_identity(op("a"), a.clone()),
        f.coordinator.prompt_with_identity(params, b.clone())
    );
    x.unwrap();
    y.unwrap();
    let store = bridge_store::sqlite::SqliteStore::open_history_read_only(
        &f.dir.path().join("tasks.sqlite"),
    )
    .unwrap();
    for (owner, total) in [(&a, 15), (&b, 30)] {
        for query in [
            UsageQueryV1 {
                run_attempt: Some(owner.attempt_id.as_str().into()),
                ..Default::default()
            },
            UsageQueryV1 {
                execution: Some(owner.execution_id.as_str().into()),
                ..Default::default()
            },
        ] {
            let summary = store.usage_summary_v1(&query).await.unwrap();
            assert_eq!(summary.buckets[0].total_tokens.reported_sum, total);
            assert_eq!(summary.buckets[0].attempted, 1);
        }
    }
    let mismatched = store
        .usage_summary_v1(&UsageQueryV1 {
            execution: Some(b.execution_id.as_str().into()),
            run_attempt: Some(a.attempt_id.as_str().into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(mismatched.buckets.is_empty());
}

#[tokio::test]
async fn review_served_disconnect_drops_consumption_and_preserves_pending_actuals() {
    let f = Fixture::new();
    f.backend.stall.store(true, Ordering::SeqCst);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = f.router(false);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let owner = AttemptIdentity::initial().unwrap();
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let mut response = client.post(url).header("A2A-Version","1.0").json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"SendStreamingMessage","params":{"message":{"text":"go", "taskId":owner.execution_id.as_str(), "metadata":{"a2a-bridge.model":"model", "account_group":"forged", "billing_mode":"subscription", "a2a-bridge.execution_id":owner.execution_id.as_str(),"a2a-bridge.attempt_id":owner.attempt_id.as_str()}}}})).send().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_some()
    );
    drop(response);
    drop(client);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !f.backend.stopped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_usage(&f.reopened(Some(&owner)).await, 1, 1);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn review_mcp_transport_run_and_continue_share_session_not_accounting_owner() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let f = Fixture::new();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(server);
    let coord = f.coordinator.clone();
    let serving = tokio::spawn(async move {
        bridge_mcp::serve(read, write, coord).await.unwrap();
    });
    let (read, mut write) = tokio::io::split(client);
    let mut lines = tokio::io::BufReader::new(read).lines();
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    lines.next_line().await.unwrap().unwrap();
    for name in ["run", "continue"] {
        let owner = AttemptIdentity::initial().unwrap();
        let request = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":{"input":"go","context":"transport","execution_id":owner.execution_id.as_str(),"attempt_id":owner.attempt_id.as_str()}}});
        write
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["result"].get("isError").is_none(), "{line}");
        assert!(line.contains("answer"), "{line}");
        assert_usage(&f.reopened(Some(&owner)).await, 1, 0);
    }
    let prompts = f.backend.prompts.lock().unwrap().clone();
    assert_eq!(prompts[0], prompts[1]);
    write.shutdown().await.unwrap();
    drop(write);
    serving.await.unwrap();
}

#[tokio::test]
async fn review_c2_hook_and_typed_outcome_preserve_evidence_and_settlement_error() {
    use bridge_core::{
        diagnostics::*, ports::BackendCleanupDispositionV1, routing::*, translator::*,
    };
    for known in [false, true] {
        for fail_settlement in [false, true] {
            let f = Fixture::new();
            f.store.usage_summary_v1(&Default::default()).await.unwrap();
            let owner = AttemptIdentity::initial().unwrap();
            let selection = AccountingSelectionV1::new(
                owner.clone(),
                UsageAttributionV1 {
                    agent: "candidate".into(),
                    effective_model: Some("selected".into()),
                    ..Default::default()
                },
            )
            .unwrap();
            let first_id = selection.concrete.clone();
            let prepared = selection
                .start_with(&AccountingV1::new(f.store.clone()), |selection| {
                    assert!(
                        f.rows().is_empty(),
                        "selection hook must precede accounting start"
                    );
                    assert_eq!(
                        selection.attribution.effective_model.as_deref(),
                        Some("selected")
                    );
                    Ok(())
                })
                .await
                .unwrap();
            if fail_settlement {
                sql(&f,true,"CREATE TRIGGER refuse_settlement BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'settlement refused'); END;");
            }
            let original = BridgeError::agent_failure(
                FailureDiagnostic::build(
                    FailureDiagnosticInput {
                        failed_phase: DiagnosticPhase::PromptStart,
                        last_completed_phase: Some(DiagnosticPhase::ConfigApply),
                        class: DiagnosticFailureClass::RateLimited,
                        disposition: FailureDisposition::Fatal,
                        code: "provider.rate_limited".into(),
                        summary: "bounded provider failure".into(),
                        causes: vec![],
                        stderr_observed: false,
                        stderr_line_count: 0,
                        stderr_scope: None,
                        stderr_tail: None,
                        stderr_redaction: None,
                        retry_after_ms: Some(1000),
                        reset_at_ms: None,
                        prompt_may_have_been_accepted: !known,
                    },
                    &DiagnosticRedactor::default(),
                )
                .unwrap(),
            );
            let mut failure = AccountingFailureV1::from(original.clone());
            assert_eq!(failure.replay, ReplayEvidenceV1::default());
            assert_eq!(failure.cleanup, BackendCleanupDispositionV1::Unknown);
            if known {
                failure.replay = ReplayEvidenceV1 {
                    prompt_acceptance: ExclusionEvidenceV1::Excluded,
                    effects: ExclusionEvidenceV1::Excluded,
                    cleanup_completed: true,
                };
                failure.cleanup = BackendCleanupDispositionV1::Complete;
            }
            let expected = failure.clone();
            let failure = failure.settle(Some(prepared)).await;
            assert_eq!(failure.original, original);
            assert_eq!(failure.replay, expected.replay);
            assert_eq!(failure.cleanup, expected.cleanup);
            assert_eq!(
                failure.persistence,
                fail_settlement.then_some(BridgeError::StoreFailure)
            );
            assert_eq!(
                failure.surfaced_error(),
                if fail_settlement {
                    BridgeError::StoreFailure
                } else {
                    original
                }
            );
            assert_eq!(f.rows()[0].finalized, !fail_settlement);
            if fail_settlement {
                sql(&f, true, "DROP TRIGGER refuse_settlement;");
            }
            let next = AccountingSelectionV1::new(
                owner.clone(),
                UsageAttributionV1 {
                    agent: "explicit-subsequent".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(next.owner, owner);
            assert_ne!(next.concrete, first_id);
            let next = next
                .start(&AccountingV1::new(f.store.clone()))
                .await
                .unwrap();
            assert_eq!(
                next.attribution().execution.as_deref(),
                Some(owner.execution_id.as_str())
            );
            assert_eq!(
                next.attribution().run_attempt.as_deref(),
                Some(owner.attempt_id.as_str())
            );
            assert!(next
                .reject_preparation(BridgeError::StoreFailure)
                .await
                .persistence
                .is_none());
            assert_eq!(f.rows().len(), 2);
            assert!(f.backend.prompts.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn compaction_claim_rejects_stale_agent_handle_and_config_without_expiry() {
    use bridge_core::{domain::*, translator::*};
    for change in ["agent", "handle", "model", "unchanged", "busy"] {
        let backend = Arc::new(Recording::default());
        let registry = crate::common::single_agent_registry("agent", backend.clone());
        let a = AgentId::parse("agent").unwrap();
        let entry = (*registry.entry_snapshot(&a).unwrap()).clone();
        let mut other = entry.clone();
        other.id = AgentId::parse("b").unwrap();
        registry
            .apply(RegistrySnapshot {
                default: a.clone(),
                entries: vec![entry, other],
                allowed_cmds: vec!["test-cmd".into()],
            })
            .await
            .unwrap();
        let f = Fixture::with_registry(backend, registry);
        let sm = &f.coordinator.session_manager;
        let ctx = ContextId::parse("compact-claim").unwrap();
        let m1 = Some(AgentOverride {
            model: Some("M1".into()),
            ..Default::default()
        });
        let first = sm
            .checkout_turn(&ctx, a.clone(), m1.clone(), None)
            .await
            .unwrap();
        sm.finish_turn(&ctx, first.generation, &first.op).await;
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
        // Deterministic interleaving: preparation has persisted, but the claim has not run.
        match change {
            "agent" | "handle" => {
                sm.release(&ctx).await;
                let agent = if change == "agent" {
                    AgentId::parse("b").unwrap()
                } else {
                    a.clone()
                };
                let next = sm
                    .checkout_turn(&ctx, agent, m1.clone(), None)
                    .await
                    .unwrap();
                sm.finish_turn(&ctx, next.generation, &next.op).await;
            }
            "model" => {
                let next = sm
                    .checkout_turn(
                        &ctx,
                        a.clone(),
                        Some(AgentOverride {
                            model: Some("M2".into()),
                            ..Default::default()
                        }),
                        None,
                    )
                    .await
                    .unwrap();
                sm.finish_turn(&ctx, next.generation, &next.op).await;
            }
            "busy" => {
                sm.checkout_existing_turn(&ctx).await.unwrap();
            }
            _ => {}
        }
        let current = sm.compaction_snapshot(&ctx).await;
        let releases = f.backend.releases.lock().unwrap().len();
        let result = sm
            .compact_session_prepared(&ctx, &snapshot, |backend, session| {
                bridge_coordinator::compact::summarize_collect_accounted(
                    backend,
                    session,
                    usage.take(),
                )
            })
            .await;
        let result = settle_usage_preparation_v1(result.map_err(Into::into), usage).await;
        let row = &f.rows()[0];
        assert!(row.finalized);
        assert_eq!(row.dispatched, change == "unchanged");
        assert_eq!(
            f.backend.prompts.lock().unwrap().len(),
            usize::from(change == "unchanged")
        );
        if change == "unchanged" {
            assert!(result.is_ok());
        } else {
            assert_eq!(
                result.err().unwrap().original,
                if change == "busy" {
                    BridgeError::HandleBusy
                } else {
                    BridgeError::ConfigMismatch {
                        field: "accounting selection",
                    }
                }
            );
            assert_eq!(sm.compaction_snapshot(&ctx).await, current);
            assert_eq!(f.backend.releases.lock().unwrap().len(), releases);
            assert_eq!(
                sm.status(&ctx).await.unwrap().state,
                if change == "busy" { "running" } else { "idle" }
            );
        }
    }
}

#[tokio::test]
async fn production_preparation_outcomes_keep_typed_cause_and_persistence_separate() {
    use bridge_core::{ports::BackendCleanupDispositionV1, routing::*, translator::*};
    for mcp in [false, true] {
        for known in [false, true] {
            for rejected in [false, true] {
                let f = Fixture::new();
                f.store.usage_summary_v1(&Default::default()).await.unwrap();
                if rejected {
                    sql(&f, true, "CREATE TRIGGER refuse_settlement BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'settlement refused'); END;");
                }
                let original = BridgeError::ConfigMismatch {
                    field: "distinctive configuration failure",
                };
                let failure = if mcp {
                    *f.backend.config_error.lock().unwrap() = Some(original.clone());
                    f.coordinator
                        .prompt_with_identity_outcome(
                            op("failed-config"),
                            AttemptIdentity::initial().unwrap(),
                        )
                        .await
                        .err()
                        .unwrap()
                } else {
                    let prepared = f
                        .coordinator
                        .prepare_usage_selection(
                            AttemptIdentity::initial().unwrap(),
                            "candidate".into(),
                            None,
                            "direct",
                            None,
                        )
                        .await
                        .unwrap();
                    let mut failure = AccountingFailureV1::from(original.clone());
                    if known {
                        failure.replay = ReplayEvidenceV1 {
                            prompt_acceptance: ExclusionEvidenceV1::Excluded,
                            effects: ExclusionEvidenceV1::Excluded,
                            cleanup_completed: true,
                        };
                        failure.cleanup = BackendCleanupDispositionV1::Complete;
                    }
                    let expected = failure.clone();
                    let failure = settle_usage_preparation_v1::<()>(Err(failure), prepared)
                        .await
                        .err()
                        .unwrap();
                    assert_eq!(failure.replay, expected.replay);
                    assert_eq!(failure.cleanup, expected.cleanup);
                    failure
                };
                if mcp {
                    assert_eq!(failure.replay, ReplayEvidenceV1::default());
                    assert_eq!(failure.cleanup, BackendCleanupDispositionV1::Unknown);
                }
                assert_eq!(failure.original, original);
                assert_eq!(
                    failure.persistence,
                    rejected.then_some(BridgeError::StoreFailure)
                );
                assert_eq!(
                    failure.surfaced_error(),
                    if rejected {
                        BridgeError::StoreFailure
                    } else {
                        original
                    }
                );
                assert_eq!(f.rows()[0].finalized, !rejected);
                assert!(!f.rows()[0].dispatched);
                assert!(f.backend.prompts.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn billing_change_after_start_does_not_relabel_reopened_rows() {
    use bridge_core::translator::AccountingSelectionV1;
    let f = Fixture::new();
    let owner = AttemptIdentity::initial().unwrap();
    let mut accounting = billing_accounting(f.store.clone());
    let attribution = UsageAttributionV1 {
        agent: "agent".into(),
        effective_model: Some("model".into()),
        ..Default::default()
    };
    let old = AccountingSelectionV1::new(owner.clone(), attribution.clone())
        .unwrap()
        .start(&accounting)
        .await
        .unwrap();
    let expected = old.attribution().clone();
    accounting.billing = None;
    old.into_attempt().preparation_failed().await.unwrap();
    let next = AccountingSelectionV1::new(owner, attribution)
        .unwrap()
        .start(&accounting)
        .await
        .unwrap();
    assert!(next.attribution().billing.is_none());
    next.into_attempt().preparation_failed().await.unwrap();
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    let old = rows
        .iter()
        .find(|r| r.attribution.billing.is_some())
        .unwrap();
    assert_eq!(old.attribution, expected);
    assert_billing(old);
    assert!(rows.iter().all(|r| r.finalized && !r.dispatched));
}
