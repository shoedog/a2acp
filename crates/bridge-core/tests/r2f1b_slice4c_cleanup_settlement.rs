use bridge_core::error::BridgeError;
use bridge_core::execution_policy::{
    cleanup_deadline_after_cancellation_ms_v1, settle_node_cleanup_v2, NodeCauseV1,
    NodeCleanupObservationV1, NodeCleanupV2, CLEANUP_TAIL_MS, DEFAULT_WORK_CUTOFF_MS,
};
use bridge_core::ids::AttemptId;
use bridge_core::resource_flight::{BoundedRecoveryReasonV1, RecoveryOwnerV1, ResourceFlightIdV1};

fn flight_id(digit: char) -> ResourceFlightIdV1 {
    ResourceFlightIdV1::parse(format!("resource-flight-{}", digit.to_string().repeat(64))).unwrap()
}

fn owner(reason: &str) -> RecoveryOwnerV1 {
    RecoveryOwnerV1 {
        attempt_id: AttemptId::parse("attempt-11111111111111111111111111111111").unwrap(),
        resource_flight_id: flight_id('2'),
        reason: BoundedRecoveryReasonV1::new(reason).unwrap(),
    }
}

#[test]
fn settled_by_deadline_yields_terminal_and_late_settlement_transfers() {
    let cause = NodeCauseV1::from_bridge_error(&BridgeError::StoreFailure);
    for (observation, expected) in [
        (
            NodeCleanupObservationV1::Complete(17, owner("complete cleanup")),
            NodeCleanupV2::Complete { duration_ms: 17 },
        ),
        (
            NodeCleanupObservationV1::Failed(18, cause.clone(), owner("failed cleanup")),
            NodeCleanupV2::Failed {
                duration_ms: 18,
                cause,
            },
        ),
        (
            NodeCleanupObservationV1::NotNeeded,
            NodeCleanupV2::NotNeeded,
        ),
    ] {
        assert_eq!(
            settle_node_cleanup_v2(observation, 59_999, 60_000),
            expected
        );
    }
    let exact_owner = owner("late settlement");
    for observation in [
        NodeCleanupObservationV1::Complete(60_001, exact_owner.clone()),
        NodeCleanupObservationV1::Failed(
            60_001,
            NodeCauseV1::from_bridge_error(&BridgeError::StoreFailure),
            exact_owner.clone(),
        ),
    ] {
        assert_eq!(
            settle_node_cleanup_v2(observation, 60_001, 60_000),
            NodeCleanupV2::Partial {
                duration_ms: 60_001,
                recovery_owner: exact_owner.clone(),
            }
        );
    }
}

#[test]
fn unsettled_at_deadline_transfers_exact_owner_as_partial() {
    let exact_owner = owner("cleanup deadline");
    assert!(settle_node_cleanup_v2(
        NodeCleanupObservationV1::Complete(60_000, exact_owner.clone()),
        59_999,
        60_000
    )
    .is_pending());
    assert_eq!(
        settle_node_cleanup_v2(
            NodeCleanupObservationV1::Unsettled(60_000, exact_owner.clone()),
            60_000,
            60_000
        ),
        NodeCleanupV2::Partial {
            duration_ms: 60_000,
            recovery_owner: exact_owner
        }
    );
}

#[test]
fn unknown_cleanup_retains_identifiable_owner() {
    let exact_owner = owner("unknown cleanup");
    let owned = NodeCleanupObservationV1::UnsettledUnknownOwned(60_000, exact_owner.clone());
    assert_eq!(
        settle_node_cleanup_v2(owned, 60_000, 60_000),
        NodeCleanupV2::Unknown {
            duration_ms: 60_000,
            recovery_owner: Some(exact_owner),
        }
    );
}

#[test]
fn work_cutoff_plus_cleanup_tail_cap_binds() {
    let cancellation_anchor_ms = DEFAULT_WORK_CUTOFF_MS + 10_000;
    let deadline =
        cleanup_deadline_after_cancellation_ms_v1(cancellation_anchor_ms, DEFAULT_WORK_CUTOFF_MS);
    assert_eq!(deadline, CLEANUP_TAIL_MS - 10_000);
    let exact_owner = owner("capped deadline");
    let observation = NodeCleanupObservationV1::Unsettled(deadline, exact_owner.clone());
    assert_eq!(
        settle_node_cleanup_v2(observation, deadline, deadline),
        NodeCleanupV2::Partial {
            duration_ms: deadline,
            recovery_owner: exact_owner,
        }
    );
}

#[test]
fn complete_partial_and_unknown_wire_bytes_remain_literal() {
    let exact_owner = owner("cleanup deadline");
    const PARTIAL: &[u8] = br#"{"state":"partial","duration_ms":60000,"recovery_owner":{"attempt_id":"attempt-11111111111111111111111111111111","resource_flight_id":"resource-flight-2222222222222222222222222222222222222222222222222222222222222222","reason":"cleanup deadline"}}"#;
    const UNKNOWN: &[u8] = br#"{"state":"unknown","duration_ms":60001,"recovery_owner":{"attempt_id":"attempt-11111111111111111111111111111111","resource_flight_id":"resource-flight-2222222222222222222222222222222222222222222222222222222222222222","reason":"cleanup deadline"}}"#;
    for (cleanup, literal) in [
        (
            NodeCleanupV2::Complete { duration_ms: 17 },
            b"{\"state\":\"complete\",\"duration_ms\":17}".as_slice(),
        ),
        (
            NodeCleanupV2::Partial {
                duration_ms: 60_000,
                recovery_owner: exact_owner.clone(),
            },
            PARTIAL,
        ),
        (
            NodeCleanupV2::Unknown {
                duration_ms: 60_001,
                recovery_owner: Some(exact_owner),
            },
            UNKNOWN,
        ),
    ] {
        assert_eq!(serde_json::to_vec(&cleanup).unwrap(), literal);
    }
}
