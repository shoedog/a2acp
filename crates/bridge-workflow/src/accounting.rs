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
    pub(crate) fn admits_fresh_fallback(&self) -> bool {
        self.accounting.budget_policy.is_none()
            && self.owner.ordinal == 0
            && self.owner.parent_attempt_id.is_none()
    }

    /// Resumed execution requires the already claimed durable owner. This check
    /// grants no preparation permission and never consumes a resume slot.
    pub(crate) async fn validate_resumed_owner(
        &self,
        store: &dyn bridge_core::task_store::TaskStore,
        task: &bridge_core::ids::TaskId,
        spec: &crate::run_spec::WorkflowRunSpecV1,
    ) -> Result<(), BridgeError> {
        if self.owner.ordinal == 0
            || self.owner.parent_attempt_id.is_none()
            || self.owner.attempt_id != spec.attempt_id
            || self.owner.execution_id.as_str() != task.as_str()
            || self.workflow != spec.graph.id
            || store
                .get_attempt_locator(task)
                .await?
                .is_none_or(|locator| locator.identity != self.owner)
        {
            return Err(BridgeError::InvalidStateTransition);
        }
        Ok(())
    }

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

    /// Reserve one cold visit before resolution or configuration can escape.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_candidate(
        self,
        store: &dyn bridge_core::task_store::TaskStore,
        task: &bridge_core::ids::TaskId,
        node: &NodeId,
        progress: &mut bridge_core::task_store::CandidateProgressV1,
        selected_chain: Option<usize>,
        position: usize,
        context: bridge_core::ids::ContextId,
    ) -> Result<PreparedAccountingTurnV1, bridge_core::translator::AccountingFailureV1> {
        use bridge_core::task_store::*;
        let persistence = |error: BridgeError| bridge_core::translator::AccountingFailureV1 {
            persistence: Some(error.clone()),
            ..error.into()
        };
        if progress.visits.is_empty()
            && store
                .put_candidate_progress_v1(task, node, None, progress)
                .await
                .map_err(persistence)?
                != CandidatePublicationV1::Applied
        {
            return Err(BridgeError::InvalidStateTransition.into());
        }
        let mut next = progress.clone();
        next.selected_chain = selected_chain;
        if next
            .visits
            .last()
            .is_some_and(|visit| visit.position == position)
        {
            next.retries_consumed += 1;
        }
        next.visits.push(CandidateVisitV1 {
            position,
            accounting_id: self.selection.concrete.as_str().into(),
            owner: self.selection.owner.clone(),
            cold_context: context,
            phase: CandidatePhaseV1::Preparing,
            rejection: None,
        });
        let prepared = self.prepare().await.map_err(persistence)?;
        match store
            .put_candidate_progress_v1(task, node, Some(progress), &next)
            .await
        {
            Ok(CandidatePublicationV1::Applied) => {
                *progress = next;
                Ok(prepared)
            }
            result => {
                let error = result.err().unwrap_or(BridgeError::InvalidStateTransition);
                let mut failure = prepared.reject_preparation(error.clone()).await;
                failure.persistence = Some(error);
                Err(failure)
            }
        }
    }

    /// Start durable accounting before checkout, resolution, or configuration.
    /// Transfer the result to the existing capture boundary on dispatch, or use
    /// `PreparedAccountingTurnV1::reject_preparation` on an admitted rejection.
    /// Dropping a prepared turn deliberately leaves a pending row.
    pub async fn prepare(self) -> Result<PreparedAccountingTurnV1, BridgeError> {
        self.selection.start(&self.accounting).await
    }
}
