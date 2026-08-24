use bridge_core::execution_policy::{
    settle_node_cleanup_v2, NodeCleanupObservationV1, NodeCleanupRecordV2, NodeCleanupV2,
    WorktreePreservationDispositionV1, WorktreePreservationResultV1, NODE_CLEANUP_RECORD_SCHEMA_V2,
};
use bridge_core::resource_flight::RecoveryOwnerV1;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct CleanupOwnershipAuditV1(Arc<AtomicU64>);

impl CleanupOwnershipAuditV1 {
    #[must_use]
    pub fn violation_count(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn record_violation(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
#[must_use = "the sole cleanup owner must be settled or transferred"]
pub struct SoleCleanupOwnerGuardV1 {
    recovery_owner: Option<RecoveryOwnerV1>,
    audit: CleanupOwnershipAuditV1,
}

impl SoleCleanupOwnerGuardV1 {
    pub fn new(recovery_owner: RecoveryOwnerV1, audit: CleanupOwnershipAuditV1) -> Self {
        Self {
            recovery_owner: Some(recovery_owner),
            audit,
        }
    }

    pub fn settle(mut self) {
        self.recovery_owner.take();
    }

    #[must_use]
    pub fn transfer(mut self) -> RecoveryOwnerV1 {
        self.recovery_owner
            .take()
            .expect("a live cleanup guard always carries its exact owner")
    }
}

impl Drop for SoleCleanupOwnerGuardV1 {
    fn drop(&mut self) {
        if self.recovery_owner.is_some() {
            self.audit.record_violation();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedWorktreePreservationV1(WorktreePreservationResultV1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreservationStillPendingV1;

impl TypedWorktreePreservationV1 {
    pub fn new(result: WorktreePreservationResultV1) -> Result<Self, PreservationStillPendingV1> {
        if result.disposition == WorktreePreservationDispositionV1::Pending {
            return Err(PreservationStillPendingV1);
        }
        Ok(Self(result))
    }
}

#[derive(Debug)]
pub struct PreservationRequiredV1;

#[derive(Debug)]
pub struct PreservationTypedV1 {
    preservation: TypedWorktreePreservationV1,
}

#[derive(Debug)]
#[must_use = "preservation-first settlement must be resolved"]
pub struct WorkflowNodeCancellationSettlementV1<Preservation> {
    observation: NodeCleanupObservationV1,
    elapsed_after_cancellation_ms: u64,
    cleanup_deadline_after_cancellation_ms: u64,
    guard: Option<SoleCleanupOwnerGuardV1>,
    preservation: Preservation,
}

impl WorkflowNodeCancellationSettlementV1<PreservationRequiredV1> {
    pub fn new(
        observation: NodeCleanupObservationV1,
        elapsed_after_cancellation_ms: u64,
        cleanup_deadline_after_cancellation_ms: u64,
        audit: CleanupOwnershipAuditV1,
    ) -> Self {
        let guard = observation
            .recovery_owner()
            .cloned()
            .map(|owner| SoleCleanupOwnerGuardV1::new(owner, audit));
        Self {
            observation,
            elapsed_after_cancellation_ms,
            cleanup_deadline_after_cancellation_ms,
            guard,
            preservation: PreservationRequiredV1,
        }
    }

    pub fn after_preservation(
        self,
        preservation: TypedWorktreePreservationV1,
    ) -> WorkflowNodeCancellationSettlementV1<PreservationTypedV1> {
        WorkflowNodeCancellationSettlementV1 {
            observation: self.observation,
            elapsed_after_cancellation_ms: self.elapsed_after_cancellation_ms,
            cleanup_deadline_after_cancellation_ms: self.cleanup_deadline_after_cancellation_ms,
            guard: self.guard,
            preservation: PreservationTypedV1 { preservation },
        }
    }
}

impl WorkflowNodeCancellationSettlementV1<PreservationTypedV1> {
    /// A pending result is a snapshot; reconstruct it with a fresh elapsed reading before polling.
    pub fn into_disposition(mut self) -> Result<NodeCleanupRecordV2, Box<Self>> {
        let settled_cleanup = settle_node_cleanup_v2(
            self.observation.clone(),
            self.elapsed_after_cancellation_ms,
            self.cleanup_deadline_after_cancellation_ms,
        );
        if settled_cleanup.is_pending() {
            return Err(Box::new(self));
        }
        let mut record = NodeCleanupRecordV2 {
            schema_version: NODE_CLEANUP_RECORD_SCHEMA_V2,
            cleanup: settled_cleanup,
            preservation: self.preservation.preservation.0.clone(),
            collateral: None,
        };
        if record.validate_coherence().is_err() {
            return Err(Box::new(self));
        }
        record.cleanup = match (record.cleanup, self.guard.take()) {
            (NodeCleanupV2::Partial { duration_ms, .. }, Some(guard)) => NodeCleanupV2::Partial {
                duration_ms,
                recovery_owner: guard.transfer(),
            },
            (
                NodeCleanupV2::Unknown {
                    duration_ms,
                    recovery_owner: Some(_),
                },
                Some(guard),
            ) => NodeCleanupV2::Unknown {
                duration_ms,
                recovery_owner: Some(guard.transfer()),
            },
            (cleanup, Some(guard)) => {
                guard.settle();
                cleanup
            }
            (cleanup, None) => cleanup,
        };
        Ok(record)
    }
}
