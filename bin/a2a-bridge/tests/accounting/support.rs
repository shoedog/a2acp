use bridge_core::{
    accounting::*, domain::*, error::BridgeError, ids::*, ports::*, task_store::TaskStore,
};
use bridge_store::sqlite::SqliteStore;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Default)]
pub struct Recording {
    pub prompts: Mutex<Vec<SessionId>>,
    pub parts: Mutex<Vec<Vec<Part>>>,
    pub fail_config: AtomicBool,
    pub config_error: Mutex<Option<BridgeError>>,
    pub releases: Mutex<Vec<SessionId>>,
    pub stall: AtomicBool,
    pub stopped: Arc<AtomicBool>,
    pub before_config: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
struct RecordingStream {
    inner: BackendStream,
    stopped: Arc<AtomicBool>,
}
impl futures::Stream for RecordingStream {
    type Item = Result<Update, BridgeError>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.get_mut().inner.as_mut().poll_next(cx)
    }
}
impl Drop for RecordingStream {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}
#[async_trait::async_trait]
impl AgentBackend for Recording {
    fn resource_flight_v1(&self) -> Result<BackendResourceFlightV1, BridgeError> {
        Ok(BackendResourceFlightV1::LegacyV2)
    }

    async fn prompt(
        &self,
        session: &SessionId,
        parts: Vec<Part>,
    ) -> Result<BackendStream, BridgeError> {
        self.prompts.lock().unwrap().push(session.clone());
        self.parts.lock().unwrap().push(parts.clone());
        let mut reported = usage();
        if parts.iter().any(|p| p.text.contains("double")) {
            reported.terminal.as_mut().unwrap().total_tokens = 30;
        }
        let text = if self.stall.load(Ordering::SeqCst) {
            "answer".repeat(300)
        } else {
            "answer".into()
        };
        let updates = vec![Ok(Update::Usage(reported)), Ok(Update::Text(text))];
        use futures::StreamExt;
        let tail: BackendStream = if self.stall.load(Ordering::SeqCst) {
            Box::pin(futures::stream::pending())
        } else {
            Box::pin(futures::stream::iter([Ok(Update::done("end_turn"))]))
        };
        Ok(Box::pin(RecordingStream {
            inner: Box::pin(futures::stream::iter(updates).chain(tail)),
            stopped: self.stopped.clone(),
        }))
    }
    async fn configure_session(&self, _: &SessionId, _: &SessionSpec) -> Result<(), BridgeError> {
        let hook = self.before_config.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
        if let Some(error) = self.config_error.lock().unwrap().clone() {
            return Err(error);
        }
        if self.fail_config.load(Ordering::SeqCst) {
            Err(BridgeError::ConfigMismatch { field: "test" })
        } else {
            Ok(())
        }
    }
    async fn release_session(&self, session: &SessionId) {
        self.releases.lock().unwrap().push(session.clone());
    }
    async fn reconcile_config(
        &self,
        _: &SessionId,
        _: &SessionSpec,
    ) -> Result<bridge_core::orch::ReconcileOutcome, BridgeError> {
        Ok(bridge_core::orch::ReconcileOutcome::Applied)
    }
    async fn cancel(&self, _: &SessionId) -> Result<(), BridgeError> {
        Ok(())
    }
}
pub fn usage() -> bridge_core::orch::UsageSnapshot {
    bridge_core::orch::UsageSnapshot {
        terminal: Some(bridge_core::orch::TerminalUsage {
            total_tokens: 15,
            input_tokens: 10,
            output_tokens: 5,
            thought_tokens: Some(2),
            cached_read_tokens: Some(3),
            cached_write_tokens: Some(4),
        }),
        cost: Some(bridge_core::orch::UsageCost {
            amount: 0.25,
            currency: "USD".into(),
        }),
        ..Default::default()
    }
}
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub store: Arc<SqliteStore>,
    pub backend: Arc<Recording>,
    pub coordinator: Arc<bridge_coordinator::Coordinator>,
}
impl Fixture {
    pub fn new() -> Self {
        let backend = Arc::new(Recording::default());
        let registry = crate::common::single_agent_registry("agent", backend.clone());
        Self::with_registry(backend, registry)
    }
    pub fn with_registry(backend: Arc<Recording>, registry: Arc<dyn AgentRegistry>) -> Self {
        Self::with_policy(backend, registry, None)
    }
    pub fn with_policy(
        backend: Arc<Recording>,
        registry: Arc<dyn AgentRegistry>,
        policy: Option<Arc<bridge_core::governance::BudgetPolicyV1>>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store =
            Arc::new(SqliteStore::open_shared_history(&dir.path().join("tasks.sqlite")).unwrap());
        let manager = Arc::new(bridge_coordinator::session_manager::SessionManager::new(
            registry.clone(),
            Duration::from_secs(3600),
        ));
        let coordinator = Arc::new(
            bridge_coordinator::Coordinator::new(
                manager,
                None,
                Arc::new(Default::default()),
                store.clone(),
                store.clone(),
                Arc::new(bridge_policy::permission::AutoPolicy),
                registry,
                Arc::new(bridge_coordinator::clock::SystemClock),
                None,
                None,
                Arc::new(bridge_observ::NoopObserver),
                3,
            )
            .with_workflow_history(Ok(store.clone()))
            .with_accounting(AccountingV1 {
                budget_policy: policy,
                ..billing_accounting(store.clone())
            }),
        );
        Self {
            dir,
            store,
            backend,
            coordinator,
        }
    }
    pub fn rows(&self) -> Vec<UsageAttemptV1> {
        let conn = rusqlite::Connection::open_with_flags(
            self.dir.path().join("tasks.sqlite.usage-v1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let rows = conn
            .prepare("SELECT record_json FROM usage_attempts_v1")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        rows
    }
    pub async fn reopened(&self, owner: Option<&AttemptIdentity>) -> UsageBucketV1 {
        let reopened =
            SqliteStore::open_history_read_only(&self.dir.path().join("tasks.sqlite")).unwrap();
        let summary = reopened
            .usage_summary_v1(&UsageQueryV1 {
                execution: owner.map(|o| o.execution_id.as_str().into()),
                run_attempt: owner.map(|o| o.attempt_id.as_str().into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(summary.buckets.len(), 1, "{summary:?}");
        summary.buckets.into_iter().next().unwrap()
    }
}
pub fn assert_usage(b: &UsageBucketV1, count: u64, pending: u64) {
    assert_eq!(
        (b.attempted, b.dispatched, b.pending),
        (count, count, pending)
    );
    assert_eq!(
        (
            b.total_tokens.reported_sum,
            b.input_tokens.reported_sum,
            b.output_tokens.reported_sum
        ),
        (15 * count, 10 * count, 5 * count)
    );
    assert_eq!(
        (
            b.thought_tokens.reported_sum,
            b.cached_read_tokens.reported_sum,
            b.cached_write_tokens.reported_sum
        ),
        (2 * count, 3 * count, 4 * count)
    );
    if pending == 0 {
        assert_eq!(b.cost_by_currency["USD"], 0.25 * count as f64);
    }
}

// Synthetic operator mapping: unresolved and unknown models deliberately stay unmapped.
pub fn billing_accounting(store: Arc<SqliteStore>) -> AccountingV1 {
    struct Billing;
    impl BillingIdentityProviderV1 for Billing {
        fn identity(&self, agent: &str, model: Option<&str>) -> Option<BillingIdentityV1> {
            (agent == "agent" && matches!(model, Some("model" | "good" | "reject"))).then(|| {
                BillingIdentityV1 {
                    account_group: "fixture".into(),
                    billing_mode: "metered".into(),
                }
            })
        }
    }
    AccountingV1 {
        billing: Some(Arc::new(Billing)),
        ..AccountingV1::new(store)
    }
}
pub fn assert_billing(row: &UsageAttemptV1) {
    assert_eq!(
        row.attribution.billing,
        Some(BillingIdentityV1 {
            account_group: "fixture".into(),
            billing_mode: "metered".into()
        })
    );
}
