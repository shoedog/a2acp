//! Versioned actuals. Context occupancy and checkpoint projections are never charges.
use crate::{
    error::BridgeError, governance::*, ids::*, orch::UsageSnapshot, task_store::TaskStore,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BillingIdentityV1 {
    pub billing_mode: String,
    pub account_group: String,
}
/// Config owns exact mapping; accounting never interprets policy.
pub trait BillingIdentityProviderV1: Send + Sync {
    fn identity(&self, agent: &str, model: Option<&str>) -> Option<BillingIdentityV1>;
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageAttributionV1 {
    pub execution: Option<String>,
    pub run_attempt: Option<String>,
    pub node: Option<String>,
    pub agent: String,
    pub effective_model: Option<String>,
    #[serde(default)]
    pub effective_effort: Option<String>,
    #[serde(default)]
    pub effective_mode: Option<String>,
    #[serde(default)]
    pub turn: Option<String>,
    pub workflow: Option<String>,
    pub attempt_kind: String,
    pub billing: Option<BillingIdentityV1>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageAttemptV1 {
    pub id: String,
    pub attribution: UsageAttributionV1,
    pub started_ms: i64,
    /// Unix UTC day, frozen at start (not local time or completion day).
    pub utc_day: i64,
    pub dispatched: bool,
    pub finalized: bool,
    pub usage: UsageSnapshot,
    pub cost_invalid: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageQueryV1 {
    pub execution: Option<String>,
    pub run_attempt: Option<String>,
    pub node: Option<String>,
    pub agent: Option<String>,
    pub effective_model: Option<String>,
    pub workflow: Option<String>,
    pub utc_day_from: Option<i64>,
    pub utc_day_through: Option<i64>,
    pub group_by_agent: bool,
    pub group_by_model: bool,
    pub group_by_workflow: bool,
    pub group_by_utc_day: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UsageGroupV1 {
    pub agent: Option<String>,
    pub effective_model: Option<String>,
    pub workflow: Option<String>,
    pub utc_day: Option<i64>,
    pub billing: Option<BillingIdentityV1>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageComponentV1 {
    pub reported_sum: u64,
    pub missing_count: u64,
    pub partial: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageBucketV1 {
    pub keys: UsageGroupV1,
    pub attempted: u64,
    pub dispatched: u64,
    pub pending: u64,
    pub unmapped: u64,
    pub total_tokens: UsageComponentV1,
    pub input_tokens: UsageComponentV1,
    pub output_tokens: UsageComponentV1,
    pub thought_tokens: UsageComponentV1,
    pub cached_read_tokens: UsageComponentV1,
    pub cached_write_tokens: UsageComponentV1,
    pub cost_by_currency: BTreeMap<String, f64>,
    pub cost_known: u64,
    pub cost_missing: u64,
    pub cost_invalid: u64,
    pub cost_partial: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageSummaryV1 {
    pub version: u32,
    pub coverage_start_ms: i64,
    pub historical_incomplete: bool,
    pub buckets: Vec<UsageBucketV1>,
}
impl UsageAttemptV1 {
    pub fn observe(&mut self, mut usage: UsageSnapshot) {
        if let Some(cost) = &usage.cost {
            if !cost.amount.is_finite() || cost.amount < 0.0 || cost.currency.trim().is_empty() {
                self.cost_invalid = true;
                usage.cost = None; // retain quality even when JSON cannot encode NaN/inf
            }
        }
        usage.merge_missing_from(&self.usage);
        if let (Some(next), Some(previous)) = (&mut usage.terminal, &self.usage.terminal) {
            next.thought_tokens = next.thought_tokens.or(previous.thought_tokens);
            next.cached_read_tokens = next.cached_read_tokens.or(previous.cached_read_tokens);
            next.cached_write_tokens = next.cached_write_tokens.or(previous.cached_write_tokens);
        }
        self.usage = usage;
    }
}
pub fn summarize_v1(
    coverage_start_ms: i64,
    mut rows: Vec<UsageAttemptV1>,
    q: &UsageQueryV1,
) -> Result<UsageSummaryV1, BridgeError> {
    let mut buckets = BTreeMap::<UsageGroupV1, UsageBucketV1>::new();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    for row in rows {
        let a = &row.attribution;
        if [
            (&q.execution, &a.execution),
            (&q.run_attempt, &a.run_attempt),
            (&q.node, &a.node),
            (&q.effective_model, &a.effective_model),
            (&q.workflow, &a.workflow),
        ]
        .iter()
        .any(|(filter, value)| filter.is_some() && filter != value)
            || q.agent.as_ref().is_some_and(|v| v != &a.agent)
            || q.utc_day_from.is_some_and(|d| row.utc_day < d)
            || q.utc_day_through.is_some_and(|d| row.utc_day > d)
        {
            continue;
        }
        let keys = UsageGroupV1 {
            agent: q.group_by_agent.then(|| a.agent.clone()),
            effective_model: q
                .group_by_model
                .then(|| a.effective_model.clone())
                .flatten(),
            workflow: q.group_by_workflow.then(|| a.workflow.clone()).flatten(),
            utc_day: q.group_by_utc_day.then_some(row.utc_day),
            billing: a.billing.clone(),
        };
        let b = buckets
            .entry(keys.clone())
            .or_insert_with(|| UsageBucketV1 {
                keys,
                ..Default::default()
            });
        b.attempted += 1;
        b.dispatched += u64::from(row.dispatched);
        b.unmapped += u64::from(a.billing.is_none());
        b.pending += u64::from(!row.finalized);
        let t = row.usage.terminal.as_ref();
        for (component, value) in [
            (&mut b.total_tokens, t.map(|t| t.total_tokens)),
            (&mut b.input_tokens, t.map(|t| t.input_tokens)),
            (&mut b.output_tokens, t.map(|t| t.output_tokens)),
            (&mut b.thought_tokens, t.and_then(|t| t.thought_tokens)),
            (
                &mut b.cached_read_tokens,
                t.and_then(|t| t.cached_read_tokens),
            ),
            (
                &mut b.cached_write_tokens,
                t.and_then(|t| t.cached_write_tokens),
            ),
        ] {
            if let Some(value) = value {
                component.reported_sum = component
                    .reported_sum
                    .checked_add(value)
                    .ok_or(BridgeError::StoreFailure)?;
            } else if row.finalized {
                component.missing_count += 1;
            }
            component.partial |= !row.finalized || value.is_none();
        }
        if row.cost_invalid {
            b.cost_invalid += 1;
        }
        if row.finalized {
            if row.cost_invalid {
                b.cost_partial = true;
            } else if let Some(cost) = &row.usage.cost {
                let total = b.cost_by_currency.entry(cost.currency.clone()).or_default();
                *total += cost.amount;
                if !total.is_finite() {
                    return Err(BridgeError::StoreFailure);
                }
                b.cost_known += 1;
            } else {
                b.cost_missing += 1;
                b.cost_partial = true;
            }
        } else {
            b.cost_partial = true;
        }
    }
    Ok(UsageSummaryV1 {
        version: 1,
        coverage_start_ms,
        historical_incomplete: true,
        buckets: buckets.into_values().collect(),
    })
}
/// Explicit awaited lifecycle. Start before checkout/resolution/configuration; transfer the
/// returned single-use handle to the outer decorator, or settle preparation failure.
#[derive(Clone)]
pub struct AccountingV1 {
    pub store: Arc<dyn TaskStore>,
    pub billing: Option<Arc<dyn BillingIdentityProviderV1>>,
    pub clock: crate::task_store::PersistenceClock,
    /// Operator-owned policy, independent of billing and metrics presentation.
    pub budget_policy: Option<Arc<BudgetPolicyV1>>,
    pub budget_context: Option<BudgetContextV1>,
}
impl AccountingV1 {
    pub fn new(store: Arc<dyn TaskStore>) -> Self {
        Self {
            store,
            billing: None,
            budget_policy: None,
            budget_context: None,
            clock: Arc::new(crate::task_store::system_wall_now_ms),
        }
    }
    pub async fn start(
        &self,
        owner: &crate::ids::AttemptIdentity,
        concrete_turn: crate::ids::AttemptId,
        mut attribution: UsageAttributionV1,
    ) -> Result<PreparedUsageAttemptV1, BridgeError> {
        if concrete_turn == owner.attempt_id {
            return Err(BridgeError::StoreFailure);
        }
        attribution.execution = Some(owner.execution_id.as_str().to_owned());
        attribution.run_attempt = Some(owner.attempt_id.as_str().to_owned());
        // Operator configuration is the sole authority, including when no provider exists.
        attribution.billing = self.billing.as_ref().and_then(|provider| {
            provider.identity(&attribution.agent, attribution.effective_model.as_deref())
        });
        let started_ms = (self.clock)();
        let row = UsageAttemptV1 {
            id: concrete_turn.as_str().to_owned(),
            attribution,
            started_ms,
            utc_day: started_ms.div_euclid(86_400_000),
            dispatched: false,
            finalized: false,
            usage: UsageSnapshot::default(),
            cost_invalid: false,
        };
        self.store.start_usage_attempt_v1(&row).await?;
        Ok(PreparedUsageAttemptV1 {
            accounting: self.clone(),
            concrete: concrete_turn,
            context: self
                .budget_context
                .clone()
                .unwrap_or_else(|| BudgetContextV1 {
                    request: BudgetRequestIdV1::Execution(owner.execution_id.clone()),
                    workflow_run: None,
                }),
            id: row.id,
            attribution: row.attribution,
            admitted: None,
            prompt_gate: None,
            provider_poll: None,
        })
    }
}
/// An exact-operation gate polls the provider once while its session ownership lock
/// is held. Cancellation before that poll cannot latch cancellation on the backend.
pub type DirectPromptFutureV1<'a> = dyn std::future::Future<Output = Result<crate::ports::BackendStream, BridgeError>>
    + Send
    + Unpin
    + 'a;
#[async_trait::async_trait]
pub trait DirectPromptGateV1: Send + Sync {
    async fn first_poll(
        &self,
        session: &SessionId,
        backend: &Arc<dyn crate::ports::AgentBackend>,
        prompt: &mut DirectPromptFutureV1<'_>,
    ) -> Result<
        std::task::Poll<Result<crate::ports::BackendStream, BridgeError>>,
        crate::translator::AccountingFailureV1,
    >;
}

/// Not Clone: a completed/replayed result cannot reuse this dispatch authority.
/// Dropping this handle or its stream deliberately leaves durable pending accounting.
pub struct PreparedUsageAttemptV1 {
    pub(crate) provider_poll: Option<Arc<std::sync::atomic::AtomicBool>>,
    accounting: AccountingV1,
    concrete: AttemptId,
    context: BudgetContextV1,
    id: String,
    attribution: UsageAttributionV1,
    admitted: Option<BudgetPermitV1>,
    prompt_gate: Option<Arc<dyn DirectPromptGateV1>>,
}
impl PreparedUsageAttemptV1 {
    pub fn attribution(&self) -> &UsageAttributionV1 {
        &self.attribution
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Consume direct admission authority before applying any retained session changes.
    /// A dropped future leaves accounting pending; its session owner must retain the claim
    /// until this operation resolves (including cancellation).
    pub async fn admit_direct(mut self) -> Result<Self, crate::translator::AccountingFailureV1> {
        let outcome = if self.admitted.is_some() {
            return Err(crate::translator::AccountingFailureV1::from(
                BridgeError::InvalidStateTransition,
            ));
        } else {
            self.admit().await
        };
        let original = match outcome {
            Ok(BudgetAdmissionOutcomeV1::Admitted(permit)) => {
                self.admitted = Some(permit);
                return Ok(self);
            }
            Ok(BudgetAdmissionOutcomeV1::Refused(refusal)) => BridgeError::BudgetRefused {
                refusal: Box::new(refusal),
            },
            Ok(BudgetAdmissionOutcomeV1::Ungoverned) => BridgeError::InvalidStateTransition,
            Err(error) => return Err(error.into()), // unknown persistence outcome stays pending
        };
        let mut failure = crate::translator::AccountingFailureV1::from(original);
        failure.persistence = self.preparation_failed().await.err();
        Err(failure)
    }
    pub fn is_admitted(&self) -> bool {
        self.admitted.is_some()
    }
    pub(crate) fn set_prompt_gate(&mut self, gate: Arc<dyn DirectPromptGateV1>) {
        self.prompt_gate = Some(gate);
    }
    pub(crate) fn take_prompt_gate(&mut self) -> Option<Arc<dyn DirectPromptGateV1>> {
        self.prompt_gate.take()
    }
    pub(crate) fn has_prompt_gate(&self) -> bool {
        self.prompt_gate.is_some()
    }
    /// Settlement only, with no dispatch or attribution authority. This future
    /// is used only after an exact prompt gate proves the provider was not polled.
    pub(crate) fn preparation_settlement(
        &self,
    ) -> impl std::future::Future<Output = Result<(), BridgeError>> + Send + 'static {
        let store = self.accounting.store.clone();
        let id = self.id.clone();
        async move { store.update_usage_attempt_v1(&id, false, None, true).await }
    }
    pub(crate) fn take_permit(&mut self) -> Option<BudgetPermitV1> {
        self.admitted.take()
    }
    /// Called only by the capture boundary, immediately before the inner prompt.
    pub(crate) async fn admit(&self) -> Result<BudgetAdmissionOutcomeV1, BridgeError> {
        // Legacy accounting permits empty attribution only when the store returns Ungoverned.
        // This sentinel cannot match an empty persisted identity during governed admission.
        let agent = AgentId::parse(&self.attribution.agent)
            .unwrap_or_else(|_| AgentId::parse("legacy-unattributed").expect("static agent ID"));
        let outcome = self
            .accounting
            .store
            .admit_usage_attempt_v1(
                self.accounting.budget_policy.as_deref(),
                &self.concrete,
                &self.context,
                &BudgetCandidateV1 {
                    agent,
                    model: self.attribution.effective_model.clone(),
                },
            )
            .await;
        // Consult the authority even without invocation caps: stored-policy refusals still apply.
        // Infrastructure failure retains the legacy dispatch error and pending row, without settlement.
        if self.accounting.budget_policy.is_none()
            && matches!(&outcome, BudgetAdmissionOutcomeV1::Refused(refusal)
                if refusal.reason == BudgetRefusalReasonV1::AccountingUnavailable)
        {
            Err(BridgeError::StoreFailure)
        } else {
            Ok(outcome)
        }
    }
    pub async fn preparation_failed(self) -> Result<(), BridgeError> {
        self.update(false, None, true).await
    }
    pub(crate) async fn update(
        &self,
        dispatched: bool,
        usage: Option<UsageSnapshot>,
        finalized: bool,
    ) -> Result<(), BridgeError> {
        self.accounting
            .store
            .update_usage_attempt_v1(&self.id, dispatched, usage, finalized)
            .await
    }
}
