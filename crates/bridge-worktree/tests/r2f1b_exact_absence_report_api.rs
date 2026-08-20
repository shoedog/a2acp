use bridge_worktree::custody::{PreservationReasonV1, WorktreeCustodyStateKindV1};
use bridge_worktree::sweep::{
    scan_worktree_records, sweep_orphans_with_exact_absence, CannotConstructSubjectV1,
    ClaimAuthorityObjectV1, ClaimAuthorityUnavailableReasonV1, ClaimAuthorityUnavailableV1,
    CustodyExactAbsenceAssessmentV1, CustodyRecordAssessmentV1, CustodyRootObservationV1,
    CustodyStateSnapshotV1, ExactAbsenceEnumerationV1, ExactAbsenceProbeV1,
    ExactAbsenceRecordAssessmentV1, ExactAbsenceRootRefusalV1, ExactAbsenceScanStatusV1,
    ExactAbsenceSweepEntryV1, ExactAbsenceSweepReportV1, IneligiblePopulationV1,
    ScannedWorktreeRecordV1, UnusedCandidateDecisionV1,
};

fn assert_public<T>() {}

fn assert_effective_item_type<'a>(_: impl Iterator<Item = &'a ExactAbsenceSweepEntryV1>) {}

#[allow(dead_code)]
fn assert_public_accessor_signatures(
    report: &ExactAbsenceSweepReportV1,
    scan: &ExactAbsenceScanStatusV1,
    entry: &ExactAbsenceSweepEntryV1,
    record_assessment: &ExactAbsenceRecordAssessmentV1,
    custody_assessment: &CustodyRecordAssessmentV1,
    authority_refusal: ClaimAuthorityUnavailableV1,
    snapshot: &CustodyStateSnapshotV1,
) {
    let _: &str = report.requested_root();
    let _: Option<&str> = report.canonical_root();
    let _: &ExactAbsenceScanStatusV1 = report.scan();
    let _: &[ExactAbsenceSweepEntryV1] = report.entries();
    let _: bool = report.has_authoritative_scan();
    assert_effective_item_type(report.effective());

    let _: &ExactAbsenceEnumerationV1 = scan.enumeration();
    let _: CustodyRootObservationV1 = scan.custody_root();

    let _: &str = entry.record_path();
    let _: &std::ffi::OsStr = entry.enumerated_name();
    let _: &ExactAbsenceRecordAssessmentV1 = entry.assessment();

    let _: UnusedCandidateDecisionV1 = record_assessment.decision();
    let _: &CustodyStateSnapshotV1 = custody_assessment.state();
    let _: &CustodyExactAbsenceAssessmentV1 = custody_assessment.assessment();

    let _: ClaimAuthorityObjectV1 = authority_refusal.object();
    let _: ClaimAuthorityUnavailableReasonV1 = authority_refusal.reason();
    let _: ClaimAuthorityUnavailableV1 =
        ClaimAuthorityUnavailableV1::new(authority_refusal.object(), authority_refusal.reason());

    let _: WorktreeCustodyStateKindV1 = snapshot.kind();
    let _: Option<PreservationReasonV1> = snapshot.preservation_reason();
}

#[test]
fn exact_absence_report_vocabulary_is_public() {
    assert_public::<CannotConstructSubjectV1>();
    assert_public::<ClaimAuthorityObjectV1>();
    assert_public::<ClaimAuthorityUnavailableReasonV1>();
    assert_public::<ClaimAuthorityUnavailableV1>();
    assert_public::<CustodyExactAbsenceAssessmentV1>();
    assert_public::<CustodyRecordAssessmentV1>();
    assert_public::<CustodyRootObservationV1>();
    assert_public::<CustodyStateSnapshotV1>();
    assert_public::<ExactAbsenceEnumerationV1>();
    assert_public::<ExactAbsenceRecordAssessmentV1>();
    assert_public::<ExactAbsenceRootRefusalV1>();
    assert_public::<ExactAbsenceScanStatusV1>();
    assert_public::<ExactAbsenceSweepEntryV1>();
    assert_public::<ExactAbsenceSweepReportV1>();
    assert_public::<IneligiblePopulationV1>();
}

#[test]
fn public_scan_functions_keep_visibility_and_exact_signatures() {
    let _: fn(&str) -> Vec<(String, ScannedWorktreeRecordV1)> = scan_worktree_records;
    let _: fn(&str, &dyn ExactAbsenceProbeV1) = sweep_orphans_with_exact_absence;
}
