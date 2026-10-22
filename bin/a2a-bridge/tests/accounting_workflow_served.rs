//! Served workflow accounting with metrics/traces absent and independent prompt observations.
pub(crate) mod common;
#[allow(dead_code)]
#[path = "accounting/support.rs"]
pub(crate) mod support;
use bridge_coordinator::{params::OpParams, Coordinator};
use bridge_core::{ids::*, task_store::TaskStore};
use std::{sync::Arc, time::Duration};

struct BoundBackend(Arc<support::Recording>);
#[async_trait::async_trait]
impl bridge_core::ports::AgentBackend for BoundBackend {
    async fn configure_session(
        &self,
        id: &SessionId,
        spec: &bridge_core::domain::SessionSpec,
    ) -> Result<(), bridge_core::error::BridgeError> {
        self.0.configure_session(id, spec).await
    }
    async fn configure_bound_session(
        &self,
        id: &SessionId,
        spec: &bridge_core::execution_policy::BoundSessionSpecV1,
    ) -> Result<(), bridge_core::error::BridgeError> {
        self.0.configure_session(id, &spec.session).await
    }
    async fn prompt(
        &self,
        id: &SessionId,
        parts: Vec<bridge_core::domain::Part>,
    ) -> Result<bridge_core::ports::BackendStream, bridge_core::error::BridgeError> {
        self.0.prompt(id, parts).await
    }
    async fn cancel(&self, id: &SessionId) -> Result<(), bridge_core::error::BridgeError> {
        self.0.cancel(id).await
    }
}
pub fn coordinator(f: &support::Fixture, batch: bool, bound: bool) -> Arc<Coordinator> {
    let registry =
        common::single_agent_registry("agent", Arc::new(BoundBackend(f.backend.clone())));
    let graph = Arc::new(
        serde_json::from_value(serde_json::json!({"id":"flow", "nodes":[
            {"id":"draft", "agent":"agent", "model":"good", "prompt_template":"{{input}}", "inputs":[]},
            {"id":"review", "agent":"agent", "model":"good", "prompt_template":"{{draft}}", "inputs":["draft"]}
        ]}))
        .unwrap(),
    );
    let mut c = Coordinator::new(
        Arc::new(bridge_coordinator::session_manager::SessionManager::new(
            registry.clone(),
            Duration::from_secs(60),
        )),
        Some(Arc::new(bridge_workflow::executor::WorkflowExecutor::new(
            registry.clone(),
        ))),
        Arc::new([(WorkflowId::parse("flow").unwrap(), graph)].into()),
        f.store.clone(),
        f.store.clone(),
        Arc::new(bridge_policy::permission::AutoPolicy),
        registry.clone(),
        Arc::new(bridge_coordinator::clock::SystemClock),
        None,
        batch.then(|| bridge_coordinator::batch::BatchRuntime::new_noop(2, 2)),
        Arc::new(bridge_observ::NoopObserver),
        3,
    )
    .with_accounting(support::billing_accounting(f.store.clone()));
    // Deliberately no workflow history, metrics, or trace sink.
    if bound {
        c = c.with_workflow_admission(Arc::new(
            bridge_workflow::admission::WorkflowAdmissionV1::new(
                registry,
                Arc::new(bridge_workflow::admission::DirectWorkflowCheckoutPlannerV1),
                bridge_core::session_cwd::SessionCwd::parse(
                    std::env::current_dir().unwrap().to_str().unwrap(),
                )
                .unwrap(),
                None,
            ),
        ));
    }
    Arc::new(c)
}
pub fn op() -> OpParams {
    OpParams::from_mcp_args(
        &serde_json::json!({"input":"---\ntask-type: freeform\n---\nbrief", "workflow":"flow"}),
    )
    .unwrap()
}
pub async fn settled(
    f: &support::Fixture,
    c: &Coordinator,
    task: &TaskId,
) -> bridge_core::task_store::TaskRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = f.store.get(task).await.unwrap().unwrap();
            if row.status != bridge_core::task_store::TaskRecordStatus::Working
                && !c.workflow_cancels_ref().lock().await.contains_key(task)
            {
                return row;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn served_rpc_reopens_supplied_identity_without_optional_telemetry() {
    use bridge_core::{
        domain::{RouteTarget, TaskMeta},
        error::BridgeError,
        ports::RouteDecision,
    };
    struct Route;
    impl RouteDecision for Route {
        fn route(&self, _: &TaskMeta) -> Result<RouteTarget, BridgeError> {
            Ok(RouteTarget::Workflow(WorkflowId::parse("flow").unwrap()))
        }
    }
    for bound in [false, true] {
        let f = support::Fixture::new();
        let c = coordinator(&f, false, bound);
        let owner = AttemptIdentity::initial().unwrap();
        let router = Arc::new(bridge_a2a_inbound::server::InboundServer::from_coordinator(
            c.clone(),
            Arc::new(Route),
            Arc::new(bridge_policy::auth::AlwaysGrant),
            "http://localhost",
            Arc::new(bridge_a2a_outbound::StubDelegation),
            "agent",
        ))
        .router();
        use tower::ServiceExt;
        let response = router.oneshot(axum::http::Request::builder().method("POST").uri("/")
            .header("content-type", "application/json").header("A2A-Version", "1.0")
            .body(axum::body::Body::from(serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"SendStreamingMessage", "params":{
                "message":{"text":"---\ntask-type: freeform\n---\nbrief", "taskId":owner.execution_id.as_str(), "metadata":{
                    "a2a-bridge.execution_id":owner.execution_id.as_str(), "a2a-bridge.attempt_id":owner.attempt_id.as_str()}}
            }}).to_string())).unwrap()).await.unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains("answer"),
            "{body:?}"
        );
        let task = TaskId::parse(owner.execution_id.as_str()).unwrap();
        assert_eq!(
            settled(&f, &c, &task).await.status,
            bridge_core::task_store::TaskRecordStatus::Completed
        );
        assert_eq!(f.backend.prompts.lock().unwrap().len(), 2);
        support::assert_usage(&f.reopened(Some(&owner)).await, 2, 0);
        let rows = f.rows();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        for row in rows {
            support::assert_billing(&row);
            assert_eq!(row.attribution.turn.as_ref(), Some(&row.id));
            assert_eq!(row.attribution.workflow.as_deref(), Some("flow"));
            assert!(matches!(
                row.attribution.node.as_deref(),
                Some("draft" | "review")
            ));
        }
    }
}
#[tokio::test]
async fn detached_preparation_and_settlement_failures_keep_original_cause() {
    for failure in ["insert", "config", "settlement"] {
        let f = support::Fixture::new();
        f.store.usage_summary_v1(&Default::default()).await.unwrap();
        if failure != "config" {
            let conn =
                rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite"))
                    .unwrap();
            conn.execute_batch("PRAGMA journal_mode=PERSIST;").unwrap();
            let operation = if failure == "insert" {
                "INSERT"
            } else {
                "UPDATE"
            };
            conn.execute_batch(&format!("CREATE TRIGGER refuse BEFORE {operation} ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'refused'); END;")).unwrap();
        }
        if failure != "insert" {
            *f.backend.config_error.lock().unwrap() =
                Some(bridge_core::error::BridgeError::ConfigMismatch {
                    field: "original-selection",
                });
        }
        let c = coordinator(&f, false, false);
        let task = c.run_workflow(op()).await.unwrap();
        let result = settled(&f, &c, &task).await;
        assert_eq!(
            result.status,
            bridge_core::task_store::TaskRecordStatus::Failed
        );
        assert!(f.backend.prompts.lock().unwrap().is_empty());
        let error = result.error.unwrap();
        if failure == "settlement" {
            assert!(
                error.contains("original-selection") && error.contains("StoreFailure"),
                "{error}"
            );
        } else if failure == "config" {
            assert!(error.contains("original-selection"), "{error}");
        } else {
            assert!(error.contains("StoreFailure"), "{error}");
        }
        let rows = f.rows();
        assert_eq!(
            rows.len(),
            if failure == "config" {
                2
            } else {
                usize::from(failure != "insert")
            }
        );
        if let Some(row) = rows.first() {
            assert!(!row.dispatched);
            assert_eq!(row.finalized, failure == "config");
        }
    }
}

#[tokio::test]
async fn dispatch_update_refusal_after_successful_configuration_never_prompts() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let f = support::Fixture::new();
    f.store.usage_summary_v1(&Default::default()).await.unwrap();
    let conn =
        rusqlite::Connection::open(f.dir.path().join("tasks.sqlite.usage-v1.sqlite")).unwrap();
    conn.execute_batch("PRAGMA journal_mode=PERSIST; CREATE TRIGGER refuse_dispatch BEFORE UPDATE ON usage_attempts_v1 BEGIN SELECT RAISE(ABORT, 'dispatch refused'); END;").unwrap();
    let configured = Arc::new(AtomicBool::new(false));
    let observed = configured.clone();
    *f.backend.before_config.lock().unwrap() = Some(Box::new(move || {
        observed.store(true, Ordering::SeqCst);
    }));
    let c = coordinator(&f, false, false);
    let task = c.run_workflow(op()).await.unwrap();
    let result = settled(&f, &c, &task).await;
    assert!(configured.load(Ordering::SeqCst));
    assert!(f.backend.config_error.lock().unwrap().is_none());
    assert!(!f.backend.fail_config.load(Ordering::SeqCst));
    assert!(
        f.backend.prompts.lock().unwrap().is_empty(),
        "dispatch accounting refusal must prevent every provider prompt"
    );
    assert_eq!(
        result.status,
        bridge_core::task_store::TaskRecordStatus::Failed
    );
    assert!(result.error.unwrap().contains("StoreFailure"));
    let owner = f
        .store
        .get_attempt_locator(&task)
        .await
        .unwrap()
        .unwrap()
        .identity;
    let reopened = f.reopened(Some(&owner)).await;
    // Existing scheduling admits the successor after a failed node. Each admitted
    // node must remain undispatched/pending when its dispatch update is refused.
    assert_eq!(
        (reopened.attempted, reopened.dispatched, reopened.pending),
        (2, 0, 2)
    );
    let rows = f.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| !row.dispatched && !row.finalized));
}
