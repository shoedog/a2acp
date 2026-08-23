use std::ffi::{OsStr, OsString};

use crate::custody::{
    CustodyReadRefusalV1, PreservationReasonV1, WorktreeCustodyStateKindV1, WorktreeCustodyStateV1,
};

use super::UnusedCandidateDecisionV1;

/// False in A1, A2, and slice B. Increment 2 may change this only in the same
/// change that installs its refusing population-admission rule.
const EXACT_ABSENCE_POLICY_READY_V1: bool = false;

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct ExactAbsenceSweepReportV1 {
    requested_root: String,
    canonical_root: Option<String>,
    scan: ExactAbsenceScanStatusV1,
    entries: Vec<ExactAbsenceSweepEntryV1>,
}

impl ExactAbsenceSweepReportV1 {
    pub(crate) fn new(
        requested_root: String,
        canonical_root: Option<String>,
        scan: ExactAbsenceScanStatusV1,
        entries: Vec<ExactAbsenceSweepEntryV1>,
    ) -> Self {
        Self {
            requested_root,
            canonical_root,
            scan,
            entries,
        }
    }

    #[must_use]
    pub fn requested_root(&self) -> &str {
        &self.requested_root
    }

    #[must_use]
    pub fn canonical_root(&self) -> Option<&str> {
        self.canonical_root.as_deref()
    }

    #[must_use]
    pub fn scan(&self) -> &ExactAbsenceScanStatusV1 {
        &self.scan
    }

    #[must_use]
    pub fn entries(&self) -> &[ExactAbsenceSweepEntryV1] {
        &self.entries
    }

    /// Reports only whether the completed scan observed a complete enumeration
    /// through matching pinned root evidence. The returned report retains no
    /// descriptor, lock, or action authority.
    #[must_use]
    pub fn has_authoritative_scan(&self) -> bool {
        matches!(self.scan.enumeration(), ExactAbsenceEnumerationV1::Complete)
            && self.scan.custody_root() == CustodyRootObservationV1::Pinned
    }

    /// Yields entries that satisfy the report's snapshot-eligibility filter.
    /// Refused and unproven entries are absent.
    ///
    /// Borrowing keeps the entry, its exact enumerated name, and its assessment
    /// ergonomically coupled for ordinary callers. It does not type-enforce
    /// inseparability: the entry is cloneable, its name can be copied, and its raw
    /// decision is copyable. This API deliberately adds no separate effective-
    /// decision scalar.
    ///
    /// The scan session has ended before this iterator can be consumed. A yielded
    /// entry is input to a future T3b action-time re-decision, not authority to act.
    /// Under its own lock, T3b must:
    ///
    /// 1. Re-open the selected sweep root and establish its current object identity.
    /// 2. Re-read the exact record named by enumerated_name() without reconstructing
    ///    the name from record_path().
    /// 3. Re-establish record/sibling placement and source, root, worktree,
    ///    common-directory, and source/common-directory binding evidence.
    /// 4. Apply the current population-admission rule.
    /// 5. Repeat the exact-absence observation against the current target and Git
    ///    registration.
    /// 6. Refuse if any root, record, object, policy, or absence observation changed.
    /// 7. Keep the T3b lock and its action-time authority alive through the authorized
    ///    effect.
    ///
    /// No T3b implementation may remove, prune, settle, transition, or publish solely
    /// because a report entry appeared in effective().
    pub fn effective(&self) -> impl Iterator<Item = &ExactAbsenceSweepEntryV1> {
        self.entries.iter().filter(move |entry| {
            self.entry_is_effectively_authorized_for_policy(entry, EXACT_ABSENCE_POLICY_READY_V1)
        })
    }

    /// Private deterministic seam. Production supplies the constant above; tests
    /// supply explicit readiness so both branches are exercised before increment 2.
    fn entry_is_effectively_authorized_for_policy(
        &self,
        entry: &ExactAbsenceSweepEntryV1,
        policy_ready: bool,
    ) -> bool {
        policy_ready
            && self.has_authoritative_scan()
            && entry.assessment().decision() == UnusedCandidateDecisionV1::Authorized
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactAbsenceScanStatusV1 {
    enumeration: ExactAbsenceEnumerationV1,
    custody_root: CustodyRootObservationV1,
}

impl ExactAbsenceScanStatusV1 {
    pub(crate) fn new(
        enumeration: ExactAbsenceEnumerationV1,
        custody_root: CustodyRootObservationV1,
    ) -> Self {
        Self {
            enumeration,
            custody_root,
        }
    }

    #[must_use]
    pub fn enumeration(&self) -> &ExactAbsenceEnumerationV1 {
        &self.enumeration
    }

    #[must_use]
    pub fn custody_root(&self) -> CustodyRootObservationV1 {
        self.custody_root
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactAbsenceEnumerationV1 {
    Complete,
    Incomplete { skipped_entries: usize },
    Refused(ExactAbsenceRootRefusalV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExactAbsenceRootRefusalV1 {
    CannotCanonicalize,
    CannotEnumerate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyRootObservationV1 {
    Pinned,
    IdentityChanged,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactAbsenceSweepEntryV1 {
    record_path: String,
    enumerated_name: OsString,
    assessment: ExactAbsenceRecordAssessmentV1,
    record_bytes: Option<Vec<u8>>,
}

impl ExactAbsenceSweepEntryV1 {
    pub(crate) fn new(
        record_path: String,
        enumerated_name: OsString,
        assessment: ExactAbsenceRecordAssessmentV1,
        record_bytes: Option<Vec<u8>>,
    ) -> Self {
        Self {
            record_path,
            enumerated_name,
            assessment,
            record_bytes,
        }
    }

    #[must_use]
    pub fn record_path(&self) -> &str {
        &self.record_path
    }

    #[must_use]
    pub fn enumerated_name(&self) -> &OsStr {
        &self.enumerated_name
    }

    #[must_use]
    pub fn assessment(&self) -> &ExactAbsenceRecordAssessmentV1 {
        &self.assessment
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExactAbsenceRecordAssessmentV1 {
    Legacy(UnusedCandidateDecisionV1),
    UnreadableCustody(CustodyReadRefusalV1),
    Custody(CustodyRecordAssessmentV1),
}

impl ExactAbsenceRecordAssessmentV1 {
    /// Raw, behavior-compatible reporting projection. This is neither snapshot
    /// eligibility nor action authority.
    #[must_use]
    pub fn decision(&self) -> UnusedCandidateDecisionV1 {
        match self {
            Self::Legacy(decision) => *decision,
            Self::UnreadableCustody(_) => UnusedCandidateDecisionV1::Refused,
            Self::Custody(custody) => match custody.assessment() {
                CustodyExactAbsenceAssessmentV1::IneligiblePopulation(_) => {
                    UnusedCandidateDecisionV1::Refused
                }
                CustodyExactAbsenceAssessmentV1::CannotConstructSubject(_) => {
                    UnusedCandidateDecisionV1::Refused
                }
                CustodyExactAbsenceAssessmentV1::Assessed(decision) => *decision,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyRecordAssessmentV1 {
    state: CustodyStateSnapshotV1,
    assessment: CustodyExactAbsenceAssessmentV1,
}

impl CustodyRecordAssessmentV1 {
    pub(crate) fn new(
        state: CustodyStateSnapshotV1,
        assessment: CustodyExactAbsenceAssessmentV1,
    ) -> Self {
        Self { state, assessment }
    }

    #[must_use]
    pub fn state(&self) -> &CustodyStateSnapshotV1 {
        &self.state
    }

    #[must_use]
    pub fn assessment(&self) -> &CustodyExactAbsenceAssessmentV1 {
        &self.assessment
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CustodyExactAbsenceAssessmentV1 {
    /// Increment 2 constructs this after population admission refuses.
    IneligiblePopulation(IneligiblePopulationV1),
    /// Increment 2 constructs this when a guard or authority construction refuses.
    CannotConstructSubject(CannotConstructSubjectV1),
    /// A2 constructs this to retain the current raw decision.
    Assessed(UnusedCandidateDecisionV1),
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IneligiblePopulationV1 {
    /// `ProtectionPrepared` without a claim. Its claim is schema-optional, so this
    /// is not a malformed or missing-required-claim result.
    BareProtectionPrepared,
    /// A canonically decoded state outside increment 2's candidate population.
    /// The enclosing `CustodyRecordAssessmentV1` carries the exact state snapshot.
    StateNotCandidate,
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CannotConstructSubjectV1 {
    RecordedWorktreePathNotAbsolute,
    OutsideSweepRoot,
    RecordFileNotExpectedSibling,
    ClaimAuthorityUnavailable(ClaimAuthorityUnavailableV1),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimAuthorityUnavailableV1 {
    object: ClaimAuthorityObjectV1,
    reason: ClaimAuthorityUnavailableReasonV1,
}

impl ClaimAuthorityUnavailableV1 {
    #[must_use]
    pub const fn new(
        object: ClaimAuthorityObjectV1,
        reason: ClaimAuthorityUnavailableReasonV1,
    ) -> Self {
        Self { object, reason }
    }

    #[must_use]
    pub const fn object(&self) -> ClaimAuthorityObjectV1 {
        self.object
    }

    #[must_use]
    pub const fn reason(&self) -> ClaimAuthorityUnavailableReasonV1 {
        self.reason
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimAuthorityObjectV1 {
    Source,
    Root,
    Worktree,
    CommonDirectory,
    SourceCommonDirectoryBinding,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimAuthorityUnavailableReasonV1 {
    PathMismatch,
    NotAbsolute,
    IdentityIncomplete,
    ObservationUnavailable,
    IdentityChanged,
    OwnershipUnproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustodyStateSnapshotV1 {
    kind: WorktreeCustodyStateKindV1,
    preservation_reason: Option<PreservationReasonV1>,
}

impl CustodyStateSnapshotV1 {
    #[must_use]
    pub const fn kind(&self) -> WorktreeCustodyStateKindV1 {
        self.kind
    }

    #[must_use]
    pub const fn preservation_reason(&self) -> Option<PreservationReasonV1> {
        self.preservation_reason
    }
}

impl From<&WorktreeCustodyStateV1> for CustodyStateSnapshotV1 {
    fn from(state: &WorktreeCustodyStateV1) -> Self {
        match state {
            WorktreeCustodyStateV1::ProtectionPrepared {} => Self {
                kind: WorktreeCustodyStateKindV1::ProtectionPrepared,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::UnusedSettled {} => Self {
                kind: WorktreeCustodyStateKindV1::UnusedSettled,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Materializing {} => Self {
                kind: WorktreeCustodyStateKindV1::Materializing,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::LiveProtected {} => Self {
                kind: WorktreeCustodyStateKindV1::LiveProtected,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::PreservationPrepared {} => Self {
                kind: WorktreeCustodyStateKindV1::PreservationPrepared,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Preserved {} => Self {
                kind: WorktreeCustodyStateKindV1::Preserved,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::DeleteAuthorized {} => Self {
                kind: WorktreeCustodyStateKindV1::DeleteAuthorized,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::Removed {} => Self {
                kind: WorktreeCustodyStateKindV1::Removed,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::RecoveredLive { .. } => Self {
                kind: WorktreeCustodyStateKindV1::RecoveredLive,
                preservation_reason: None,
            },
            WorktreeCustodyStateV1::PreservationUnknown { reason } => Self {
                kind: WorktreeCustodyStateKindV1::PreservationUnknown,
                preservation_reason: Some(*reason),
            },
        }
    }
}

impl From<WorktreeCustodyStateV1> for CustodyStateSnapshotV1 {
    fn from(state: WorktreeCustodyStateV1) -> Self {
        Self::from(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_core::execution_policy::Sha256HexV1;

    type State = WorktreeCustodyStateV1;
    type Reason = PreservationReasonV1;
    type Decision = UnusedCandidateDecisionV1;
    type Record = ExactAbsenceRecordAssessmentV1;
    type Assessment = CustodyExactAbsenceAssessmentV1;
    type Enumeration = ExactAbsenceEnumerationV1;
    type Root = CustodyRootObservationV1;

    fn state() -> CustodyStateSnapshotV1 {
        CustodyStateSnapshotV1 {
            kind: WorktreeCustodyStateKindV1::Preserved,
            preservation_reason: None,
        }
    }

    fn scan(enumeration: Enumeration, root: Root) -> ExactAbsenceScanStatusV1 {
        ExactAbsenceScanStatusV1::new(enumeration, root)
    }

    fn report(
        scan: ExactAbsenceScanStatusV1,
        entries: Vec<ExactAbsenceSweepEntryV1>,
    ) -> ExactAbsenceSweepReportV1 {
        ExactAbsenceSweepReportV1::new("requested".into(), Some("canonical".into()), scan, entries)
    }

    fn custody(assessment: Assessment) -> Record {
        Record::Custody(CustodyRecordAssessmentV1::new(state(), assessment))
    }

    fn entry(name: &str, assessment: Record) -> ExactAbsenceSweepEntryV1 {
        ExactAbsenceSweepEntryV1::new(name.into(), name.into(), assessment, None)
    }

    fn assert_snapshot(state: State, reason: Option<Reason>) {
        let kind = state.kind();
        let by_ref = CustodyStateSnapshotV1::from(&state);
        assert_eq!(by_ref, CustodyStateSnapshotV1::from(state));
        assert_eq!(
            (by_ref.kind(), by_ref.preservation_reason()),
            (kind, reason)
        );
    }

    #[test]
    fn accessors_and_snapshots() {
        let refusal = ClaimAuthorityUnavailableV1::new(
            ClaimAuthorityObjectV1::Source,
            ClaimAuthorityUnavailableReasonV1::OwnershipUnproven,
        );
        let custody = CustodyRecordAssessmentV1::new(
            state(),
            Assessment::CannotConstructSubject(
                CannotConstructSubjectV1::ClaimAuthorityUnavailable(refusal),
            ),
        );
        let entry = entry("entry", Record::Custody(custody.clone()));
        let scan = scan(Enumeration::Complete, Root::Pinned);
        let report = report(scan.clone(), vec![entry.clone()]);
        assert!(
            report.requested_root() == "requested"
                && report.canonical_root() == Some("canonical")
                && report.scan() == &scan
                && report.entries() == std::slice::from_ref(&entry)
                && scan.enumeration() == &Enumeration::Complete
                && scan.custody_root() == Root::Pinned
                && entry.record_path() == "entry"
                && entry.enumerated_name() == OsStr::new("entry")
                && entry.assessment() == &Record::Custody(custody.clone())
                && custody.state().kind() == WorktreeCustodyStateKindV1::Preserved
                && custody.state().preservation_reason().is_none()
                && matches!(custody.assessment(), Assessment::CannotConstructSubject(_))
                && refusal.object() == ClaimAuthorityObjectV1::Source
                && refusal.reason() == ClaimAuthorityUnavailableReasonV1::OwnershipUnproven
        );

        assert_snapshot(State::ProtectionPrepared {}, None);
        assert_snapshot(State::UnusedSettled {}, None);
        assert_snapshot(State::Materializing {}, None);
        assert_snapshot(State::LiveProtected {}, None);
        assert_snapshot(State::PreservationPrepared {}, None);
        assert_snapshot(State::Preserved {}, None);
        assert_snapshot(State::DeleteAuthorized {}, None);
        assert_snapshot(State::Removed {}, None);
        assert_snapshot(
            State::RecoveredLive {
                predecessor_claim_digest: Sha256HexV1::digest(b"discarded"),
            },
            None,
        );
        for reason in [
            Reason::NodeFailure,
            Reason::Cancellation,
            Reason::AmbiguousCleanup,
            Reason::MaterializationInFlight,
            Reason::PostConditionDisagreement,
            Reason::RemovalFailed,
        ] {
            assert_snapshot(State::PreservationUnknown { reason }, Some(reason));
        }
    }

    #[test]
    fn raw_decision_and_historical_eligibility_projections() {
        let authorized = entry(
            "authorized",
            custody(Assessment::Assessed(Decision::Authorized)),
        );
        let refused = entry("refused", custody(Assessment::Assessed(Decision::Refused)));
        let legacy = entry("legacy", Record::Legacy(Decision::Authorized));
        for (assessment, decision) in [
            (Record::Legacy(Decision::Authorized), Decision::Authorized),
            (Record::Legacy(Decision::Refused), Decision::Refused),
            (
                Record::UnreadableCustody(CustodyReadRefusalV1::Unreadable("no".into())),
                Decision::Refused,
            ),
            (
                custody(Assessment::IneligiblePopulation(
                    IneligiblePopulationV1::BareProtectionPrepared,
                )),
                Decision::Refused,
            ),
            (
                custody(Assessment::CannotConstructSubject(
                    CannotConstructSubjectV1::OutsideSweepRoot,
                )),
                Decision::Refused,
            ),
            (
                custody(Assessment::Assessed(Decision::Authorized)),
                Decision::Authorized,
            ),
            (
                custody(Assessment::Assessed(Decision::Refused)),
                Decision::Refused,
            ),
        ] {
            assert_eq!(assessment.decision(), decision);
        }
        for enumeration in [
            Enumeration::Complete,
            Enumeration::Incomplete { skipped_entries: 1 },
            Enumeration::Refused(ExactAbsenceRootRefusalV1::CannotCanonicalize),
            Enumeration::Refused(ExactAbsenceRootRefusalV1::CannotEnumerate),
        ] {
            for root in [Root::Pinned, Root::IdentityChanged, Root::Unavailable] {
                assert_eq!(
                    report(scan(enumeration.clone(), root), vec![]).has_authoritative_scan(),
                    matches!(&enumeration, Enumeration::Complete) && root == Root::Pinned
                );
            }
        }
        let ready = report(
            scan(Enumeration::Complete, Root::Pinned),
            vec![refused.clone(), authorized.clone(), legacy.clone()],
        );
        assert_eq!(ready.effective().count(), 0);
        assert!(!ready.entry_is_effectively_authorized_for_policy(&authorized, false));
        assert!(ready.entry_is_effectively_authorized_for_policy(&authorized, true));
        assert!(!ready.entry_is_effectively_authorized_for_policy(&refused, true));
        assert!(ready.entry_is_effectively_authorized_for_policy(&legacy, true));
        for scan in [
            scan(Enumeration::Incomplete { skipped_entries: 1 }, Root::Pinned),
            scan(
                Enumeration::Refused(ExactAbsenceRootRefusalV1::CannotEnumerate),
                Root::Pinned,
            ),
            scan(Enumeration::Complete, Root::IdentityChanged),
            scan(Enumeration::Complete, Root::Unavailable),
        ] {
            assert!(
                !report(scan, vec![]).entry_is_effectively_authorized_for_policy(&authorized, true)
            );
        }
        let found = ready
            .entries()
            .iter()
            .find(|entry| ready.entry_is_effectively_authorized_for_policy(entry, true))
            .expect("borrowed authorized entry");
        assert!(std::ptr::eq(found, &ready.entries()[1]));
        assert_eq!(found.enumerated_name(), OsStr::new("authorized"));
        assert_eq!(
            ready
                .entries()
                .iter()
                .filter(|entry| ready.entry_is_effectively_authorized_for_policy(entry, true))
                .map(|entry| entry.enumerated_name())
                .collect::<Vec<_>>(),
            vec![OsStr::new("authorized"), OsStr::new("legacy")]
        );
    }
}
