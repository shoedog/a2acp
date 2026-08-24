use bridge_core::execution_policy::{
    NodeCleanupObservationV1, NodeCleanupV2, WorktreePreservationDispositionV1,
    WorktreePreservationResultV1,
};
use bridge_core::ids::AttemptId;
use bridge_core::resource_flight::{BoundedRecoveryReasonV1, RecoveryOwnerV1, ResourceFlightIdV1};
use bridge_workflow::cancellation_settlement::{
    CleanupOwnershipAuditV1, PreservationRequiredV1, PreservationTypedV1, SoleCleanupOwnerGuardV1,
    TypedWorktreePreservationV1, WorkflowNodeCancellationSettlementV1,
};

fn owner() -> RecoveryOwnerV1 {
    RecoveryOwnerV1 {
        attempt_id: AttemptId::parse("attempt-11111111111111111111111111111111").unwrap(),
        resource_flight_id: ResourceFlightIdV1::parse(format!(
            "resource-flight-{}",
            "2".repeat(64)
        ))
        .unwrap(),
        reason: BoundedRecoveryReasonV1::new("deadline").unwrap(),
    }
}

fn preserved() -> TypedWorktreePreservationV1 {
    TypedWorktreePreservationV1::new(WorktreePreservationResultV1 {
        disposition: WorktreePreservationDispositionV1::Preserved,
        custody_id: None,
        claim_digest: None,
    })
    .unwrap()
}

#[test]
fn disposition_requires_typed_and_coherent_preservation() {
    let pending: WorkflowNodeCancellationSettlementV1<PreservationRequiredV1> =
        WorkflowNodeCancellationSettlementV1::new(
            NodeCleanupObservationV1::NotNeeded,
            1,
            60_000,
            CleanupOwnershipAuditV1::default(),
        );
    let ready: WorkflowNodeCancellationSettlementV1<PreservationTypedV1> =
        pending.after_preservation(preserved());
    assert!(ready.into_disposition().is_err());
    assert!(TypedWorktreePreservationV1::new(WorktreePreservationResultV1::pending()).is_err());
}

#[test]
fn dropping_the_sole_cleanup_owner_is_detected() {
    let audit = CleanupOwnershipAuditV1::default();
    drop(SoleCleanupOwnerGuardV1::new(owner(), audit.clone()));
    assert_eq!(audit.violation_count(), 1);
}

#[test]
fn settling_or_transferring_the_exact_owner_has_no_violation() {
    let audit = CleanupOwnershipAuditV1::default();
    SoleCleanupOwnerGuardV1::new(owner(), audit.clone()).settle();
    let exact_owner = owner();
    let transferred = SoleCleanupOwnerGuardV1::new(exact_owner.clone(), audit.clone()).transfer();
    assert_eq!(transferred, exact_owner);
    assert_eq!(audit.violation_count(), 0);
}

#[test]
fn preservation_first_disposition_transfers_the_guard_owner() {
    let audit = CleanupOwnershipAuditV1::default();
    let exact_owner = owner();
    let disposition = WorkflowNodeCancellationSettlementV1::new(
        NodeCleanupObservationV1::Unsettled(60_000, exact_owner.clone()),
        60_000,
        60_000,
        audit.clone(),
    )
    .after_preservation(preserved())
    .into_disposition()
    .unwrap();
    assert_eq!(
        disposition.cleanup,
        NodeCleanupV2::Partial {
            duration_ms: 60_000,
            recovery_owner: exact_owner,
        }
    );
    assert_eq!(audit.violation_count(), 0);
}
