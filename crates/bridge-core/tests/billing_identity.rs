use bridge_core::{
    accounting::*, error::BridgeError, harvest::*, ids::*, task_store::*,
    translator::AccountingSelectionV1,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
struct Ledger(Mutex<Vec<UsageAttemptV1>>);
#[async_trait::async_trait]
impl HarvestAuditStore for Ledger {
    async fn commit_bundle(
        &self,
        _raw: HarvestRawRecordV1,
        _decision: HarvestSanitizationDecisionV1,
    ) -> Result<HarvestAuditCommit, HarvestAuditStoreError> {
        unreachable!()
    }
    async fn get_by_audit_id(
        &self,
        _audit_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        unreachable!()
    }
    async fn get_by_attempt_key(
        &self,
        _run_id: &str,
        _node_id: &str,
        _attempt_id: u32,
        _turn_id: &str,
    ) -> Result<Option<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        unreachable!()
    }
    async fn list_by_task_id(
        &self,
        _task_id: &str,
        _after_audit_id: Option<&str>,
        _limit: u32,
    ) -> Result<Vec<HarvestAuditBundleV1>, HarvestAuditStoreError> {
        unreachable!()
    }
}
#[async_trait::async_trait]
impl TaskStore for Ledger {
    async fn start_usage_attempt_v1(&self, row: &UsageAttemptV1) -> Result<(), BridgeError> {
        self.0.lock().unwrap().push(row.clone());
        Ok(())
    }
    async fn update_usage_attempt_v1(
        &self,
        id: &str,
        dispatched: bool,
        usage: Option<bridge_core::orch::UsageSnapshot>,
        finalized: bool,
    ) -> Result<(), BridgeError> {
        let mut rows = self.0.lock().unwrap();
        let row = rows.iter_mut().find(|r| r.id == id).unwrap();
        row.dispatched |= dispatched;
        row.finalized |= finalized;
        if let Some(usage) = usage {
            row.observe(usage);
        }
        Ok(())
    }
    async fn create(&self, _rec: &TaskRecord) -> Result<(), BridgeError> {
        unreachable!()
    }
    async fn set_terminal(
        &self,
        _id: &TaskId,
        _status: TaskRecordStatus,
        _result: Option<&str>,
        _error: Option<&str>,
        _updated_ms: i64,
    ) -> Result<(), BridgeError> {
        unreachable!()
    }
    async fn get(&self, _id: &TaskId) -> Result<Option<TaskRecord>, BridgeError> {
        unreachable!()
    }
    async fn list(&self, _limit: usize) -> Result<Vec<TaskRecord>, BridgeError> {
        unreachable!()
    }
    async fn sweep_interrupted(&self, _updated_ms: i64) -> Result<u64, BridgeError> {
        unreachable!()
    }
    async fn cancel_if_working(&self, _id: &TaskId, _updated_ms: i64) -> Result<bool, BridgeError> {
        unreachable!()
    }
    async fn put_node_checkpoint(
        &self,
        _task: &TaskId,
        _node: &NodeId,
        _output: &str,
        _ok: bool,
        _ts: i64,
    ) -> Result<(), BridgeError> {
        unreachable!()
    }
    async fn node_checkpoints(
        &self,
        _task: &TaskId,
    ) -> Result<
        Vec<(
            NodeId,
            String,
            bool,
            Option<bridge_core::orch::UsageSnapshot>,
        )>,
        BridgeError,
    > {
        unreachable!()
    }
    async fn claim_resume_attempt(
        &self,
        _task: &TaskId,
        _cap: u32,
        _now_ms: i64,
    ) -> Result<ResumeClaim, BridgeError> {
        unreachable!()
    }
    async fn working_tasks(&self) -> Result<Vec<TaskRecord>, BridgeError> {
        unreachable!()
    }
    async fn record_node_started(
        &self,
        _task: &TaskId,
        _node: &NodeId,
        _operation_id: &OperationId,
        _ts: i64,
    ) -> Result<i64, BridgeError> {
        unreachable!()
    }
    async fn put_node_checkpoint_sequenced(
        &self,
        _task: &TaskId,
        _node: &NodeId,
        _operation_id: &OperationId,
        _output: &str,
        _ok: bool,
        _ts: i64,
        _usage: Option<&bridge_core::orch::UsageSnapshot>,
    ) -> Result<i64, BridgeError> {
        unreachable!()
    }
    async fn set_terminal_sequenced(
        &self,
        _task: &TaskId,
        _operation_id: &OperationId,
        _status: TaskRecordStatus,
        _result: Option<&str>,
        _error: Option<&str>,
        _ts: i64,
    ) -> Result<i64, BridgeError> {
        unreachable!()
    }
    async fn journal_from(
        &self,
        _task: &TaskId,
        _after_seq: i64,
    ) -> Result<Vec<bridge_core::orch::OrchEvent>, BridgeError> {
        unreachable!()
    }
    async fn progress_snapshot(&self, _task: &TaskId) -> Result<TaskProgressSnapshot, BridgeError> {
        unreachable!()
    }
}
struct Provider {
    group: Mutex<String>,
    calls: AtomicUsize,
}
impl BillingIdentityProviderV1 for Provider {
    fn identity(&self, agent: &str, model: Option<&str>) -> Option<BillingIdentityV1> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match (agent, model) {
            ("first", Some("raw" | "override")) | ("second", Some("raw")) => {
                Some(BillingIdentityV1 {
                    account_group: self.group.lock().unwrap().clone(),
                    billing_mode: if agent == "second" {
                        "subscription"
                    } else {
                        "metered"
                    }
                    .into(),
                })
            }
            _ => None,
        }
    }
}
#[tokio::test]
async fn row_and_envelope_share_one_authoritative_frozen_identity() {
    let ledger = Arc::new(Ledger::default());
    let provider = Arc::new(Provider {
        group: Mutex::new("original".into()),
        calls: AtomicUsize::new(0),
    });
    let owner = AttemptIdentity::initial().unwrap();
    for (agent, model, mode) in [
        ("first", Some("raw"), Some("metered")),
        ("first", Some("override"), Some("metered")),
        ("second", Some("raw"), Some("subscription")),
        ("other", Some("raw"), None),
        ("first", Some("unknown"), None),
        ("first", None, None),
        ("no-provider", Some("raw"), None),
    ] {
        let mut accounting = AccountingV1::new(ledger.clone());
        if agent != "no-provider" {
            accounting.billing = Some(provider.clone());
        }
        let mut selection =
            AccountingSelectionV1::new(owner.clone(), UsageAttributionV1::default()).unwrap();
        selection.attribution.agent = agent.into();
        selection.attribution.effective_model = model.map(Into::into);
        selection.attribution.billing = Some(BillingIdentityV1::default()); // Caller-supplied, untrusted.
        let before = provider.calls.load(Ordering::SeqCst);
        let prepared = selection.start(&accounting).await.unwrap();
        let calls = before + usize::from(agent != "no-provider");
        assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
        let attribution = prepared.attribution().clone();
        assert_eq!(
            attribution
                .billing
                .as_ref()
                .map(|b| b.billing_mode.as_str()),
            mode
        );
        if let Some(billing) = &attribution.billing {
            assert_eq!(billing.account_group, *provider.group.lock().unwrap());
        }
        assert_eq!(
            ledger.0.lock().unwrap().last().unwrap().attribution,
            attribution
        );
        *provider.group.lock().unwrap() += "-changed";
        prepared.into_attempt().preparation_failed().await.unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), calls);
        assert_eq!(
            ledger.0.lock().unwrap().last().unwrap().attribution,
            attribution
        );
    }
}
