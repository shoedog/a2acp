use bridge_core::custody_inventory::{
    CustodyInventoryEntryV1, CustodyReasonCodeV1, CustodyStateClassV1, CustodyUnitKindV1,
    HistoricalReconciliationDispositionV1, HistoricalReconciliationRowV1, LosslessPathV1,
};

#[test]
fn unresolved_inventory_entries_are_the_only_entries_that_carry_blocking_reasons() {
    let path = LosslessPathV1::from_bytes(b"/private/tmp/a2a-run-17".to_vec());

    let resolved = CustodyInventoryEntryV1::new(
        "unit-17",
        CustodyUnitKindV1::StandaloneClone,
        path.clone(),
        CustodyStateClassV1::Captured,
        vec![],
    )
    .expect("a captured unit with no blocking reason is well-formed");
    assert_eq!(resolved.state, CustodyStateClassV1::Captured);
    assert!(resolved.reasons.is_empty());

    let unresolved = CustodyInventoryEntryV1::new(
        "unit-18",
        CustodyUnitKindV1::LinkedWorktree,
        path,
        CustodyStateClassV1::Unresolved,
        vec![CustodyReasonCodeV1::DependencyUnresolved],
    )
    .expect("an unresolved linked worktree retains its blocking reason");
    assert_eq!(
        unresolved.reasons,
        vec![CustodyReasonCodeV1::DependencyUnresolved]
    );

    assert!(CustodyInventoryEntryV1::new(
        "unit-19",
        CustodyUnitKindV1::EvidenceDirectory,
        LosslessPathV1::from_bytes(b"/private/tmp/evidence".to_vec()),
        CustodyStateClassV1::Captured,
        vec![CustodyReasonCodeV1::ContentUnresolved],
    )
    .is_err());
}

#[test]
fn historical_absence_stays_explicitly_unverified() {
    let row = HistoricalReconciliationRowV1::absent_without_sufficient_evidence(
        "historical-clone-8",
        LosslessPathV1::from_bytes(b"/private/tmp/a2a-clone-8".to_vec()),
    );

    assert_eq!(
        row.disposition,
        HistoricalReconciliationDispositionV1::AbsentWithoutSufficientDeletionOrCustodyEvidence
    );
    assert_eq!(row.outcome_code(), "outcome.absent_unverified");
    assert_ne!(row.outcome_code(), "verified");
}
