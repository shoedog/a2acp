//! Explicit workflow accounting ownership, independent of observers and liveness.

use bridge_core::{
    accounting::{AccountingV1, UsageAttributionV1},
    domain::EffectiveConfig,
    error::BridgeError,
    governance::{BudgetContextV1, BudgetRequestIdV1},
    ids::{AgentId, AttemptIdentity, NodeId, WorkflowId},
    translator::{AccountingSelectionV1, PreparedAccountingTurnV1},
};

/// One workflow invocation's accounting service and run-attempt ownership.
/// Construction and selection perform no persistence or provider work. Clones
/// retain the same ownership; each explicit selection mints a new concrete ID.
#[derive(Clone)]
pub struct WorkflowAccountingInvocationV1 {
    accounting: AccountingV1,
    owner: AttemptIdentity,
    workflow: WorkflowId,
}

impl WorkflowAccountingInvocationV1 {
    pub fn new(mut accounting: AccountingV1, owner: AttemptIdentity, workflow: WorkflowId) -> Self {
        accounting.budget_context = Some(BudgetContextV1 {
            request: BudgetRequestIdV1::Execution(owner.execution_id.clone()),
            workflow_run: Some(owner.execution_id.clone()),
        });
        Self {
            accounting,
            owner,
            workflow,
        }
    }

    /// Attribute an already selected candidate; this grants no routing, retry,
    /// fallback, or dispatch authority. The caller supplies its effective config
    /// and attempt kind (for example, node execution or compaction).
    pub fn select(
        &self,
        node: &NodeId,
        agent: &AgentId,
        kind: &str,
        config: &EffectiveConfig,
    ) -> Result<WorkflowAccountingSelectionV1, BridgeError> {
        let selection = AccountingSelectionV1::new(
            self.owner.clone(),
            UsageAttributionV1 {
                workflow: Some(self.workflow.as_str().into()),
                node: Some(node.as_str().into()),
                agent: agent.as_str().into(),
                attempt_kind: kind.into(),
                effective_model: config.model.clone(),
                effective_effort: config.effort.map(|e| format!("{e:?}").to_ascii_lowercase()),
                effective_mode: config.mode.clone(),
                ..Default::default()
            },
        )?;
        Ok(WorkflowAccountingSelectionV1 {
            accounting: self.accounting.clone(),
            selection,
        })
    }
}

/// An inspectable candidate bound to its invocation's service. Not Clone: a
/// prepared dispatch authority must not be duplicated or moved to another owner.
pub struct WorkflowAccountingSelectionV1 {
    accounting: AccountingV1,
    selection: AccountingSelectionV1,
}

impl WorkflowAccountingSelectionV1 {
    pub fn selection(&self) -> &AccountingSelectionV1 {
        &self.selection
    }

    /// Start durable accounting before checkout, resolution, or configuration.
    /// Transfer the result to the existing capture boundary on dispatch, or use
    /// `PreparedAccountingTurnV1::reject_preparation` on an admitted rejection.
    /// Dropping a prepared turn deliberately leaves a pending row.
    pub async fn prepare(self) -> Result<PreparedAccountingTurnV1, BridgeError> {
        self.selection.start(&self.accounting).await
    }
}
