use bridge_core::execution_policy::{
    deadline_activation_v2_for, scheduler_activation_readiness_v1, DeadlineActivationV2,
    NodeCleanupObservationV1, PolicyActivationV1, SchedulerActivationReadinessV1,
};
use bridge_core::retained_resource_flight::CleanupDeadlineTransferV1;

fn ownerless_observation_from_transfer(
    transfer: CleanupDeadlineTransferV1,
) -> Option<NodeCleanupObservationV1> {
    match transfer {
        CleanupDeadlineTransferV1::Unknown {
            result,
            proof: Some(proof),
            ..
        } => Some(NodeCleanupObservationV1::UnsettledUnknownOwnerless(
            result.duration_ms,
            proof,
        )),
        _ => None,
    }
}

#[test]
fn bridge_workflow_constructs_ownerless_observation_only_from_transfer_mint() {
    let _consume_minted_transfer: fn(
        CleanupDeadlineTransferV1,
    ) -> Option<NodeCleanupObservationV1> = ownerless_observation_from_transfer;
}

#[test]
fn ownerless_proof_wiring_keeps_production_disarmed() {
    let _proof_slot = |transfer: CleanupDeadlineTransferV1| {
        matches!(
            transfer,
            CleanupDeadlineTransferV1::Unknown { proof: Some(_), .. }
        )
    };
    let readiness = scheduler_activation_readiness_v1();
    assert_eq!(readiness, SchedulerActivationReadinessV1::Disarmed);
    assert_eq!(
        deadline_activation_v2_for(readiness, PolicyActivationV1::Production),
        DeadlineActivationV2::ManualOnlyR2f1a
    );
}
