//! Provider-free, read-only custody inventory records.
//!
//! This module deliberately has no filesystem, Git, network, or mutation API. Later custody stages may
//! consume these canonical records, but an inventory record by itself grants no sealing, promotion, or reaping
//! authority.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::execution_policy::Sha256HexV1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LosslessPathV1(Vec<u8>);

impl LosslessPathV1 {
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyUnitKindV1 {
    StandaloneClone,
    LinkedWorktree,
    EvidenceDirectory,
    SourceLocalRefs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyStateClassV1 {
    Captured,
    Empty,
    ExcludedReproducible,
    Unresolved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyReasonCodeV1 {
    OwnershipUnknown,
    ActivityUnknown,
    OperationBusy,
    WriterUncontrolled,
    ConsumerProbeFailed,
    IdentityChanged,
    MountBoundary,
    ContentUnresolved,
    GitOperationInProgress,
    DependencyUnresolved,
    PrivacyHold,
    CustodyUnverified,
    RetentionUnmet,
    HistoryAmbiguous,
}

impl CustodyReasonCodeV1 {
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::OwnershipUnknown => "park.ownership_unknown",
            Self::ActivityUnknown => "park.activity_unknown",
            Self::OperationBusy => "park.operation_busy",
            Self::WriterUncontrolled => "park.writer_uncontrolled",
            Self::ConsumerProbeFailed => "park.consumer_probe_failed",
            Self::IdentityChanged => "park.identity_changed",
            Self::MountBoundary => "park.mount_boundary",
            Self::ContentUnresolved => "park.content_unresolved",
            Self::GitOperationInProgress => "park.git_operation_in_progress",
            Self::DependencyUnresolved => "park.dependency_unresolved",
            Self::PrivacyHold => "park.privacy_hold",
            Self::CustodyUnverified => "park.custody_unverified",
            Self::RetentionUnmet => "park.retention_unmet",
            Self::HistoryAmbiguous => "park.history_ambiguous",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyInventoryEntryV1 {
    pub unit_id: String,
    pub kind: CustodyUnitKindV1,
    pub path: LosslessPathV1,
    pub state: CustodyStateClassV1,
    pub reasons: Vec<CustodyReasonCodeV1>,
}

impl CustodyInventoryEntryV1 {
    pub fn new(
        unit_id: impl Into<String>,
        kind: CustodyUnitKindV1,
        path: LosslessPathV1,
        state: CustodyStateClassV1,
        reasons: Vec<CustodyReasonCodeV1>,
    ) -> Result<Self, CustodyInventoryError> {
        let unit_id = unit_id.into();
        if unit_id.is_empty() {
            return Err(CustodyInventoryError::EmptyUnitId);
        }
        let reasons: Vec<_> = BTreeSet::<_>::from_iter(reasons).into_iter().collect();
        match (state, reasons.is_empty()) {
            (CustodyStateClassV1::Unresolved, true) => {
                Err(CustodyInventoryError::UnresolvedWithoutReason)
            }
            (CustodyStateClassV1::Unresolved, false) => Ok(Self {
                unit_id,
                kind,
                path,
                state,
                reasons,
            }),
            (_, false) => Err(CustodyInventoryError::ResolvedWithBlockingReasons),
            (_, true) => Ok(Self {
                unit_id,
                kind,
                path,
                state,
                reasons,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustodyInventoryPlanV1 {
    pub schema: String,
    pub entries: Vec<CustodyInventoryEntryV1>,
}

impl CustodyInventoryPlanV1 {
    pub fn new(mut entries: Vec<CustodyInventoryEntryV1>) -> Result<Self, CustodyInventoryError> {
        entries.sort_by(|left, right| left.unit_id.cmp(&right.unit_id));
        if entries
            .windows(2)
            .any(|pair| pair[0].unit_id == pair[1].unit_id)
        {
            return Err(CustodyInventoryError::DuplicateUnitId);
        }
        Ok(Self {
            schema: "custody-plan.v1".to_owned(),
            entries,
        })
    }

    pub fn content_digest(&self) -> Result<Sha256HexV1, CustodyInventoryError> {
        let bytes =
            serde_json::to_vec(self).map_err(|_| CustodyInventoryError::CanonicalEncoding)?;
        Ok(Sha256HexV1::digest(&bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalReconciliationDispositionV1 {
    PresentWithResolvedIdentity,
    PresentButChanged,
    AmbiguousMatch,
    AbsentWithSubstantiatedHistoricalDeletionEvidence,
    AbsentWithoutSufficientDeletionOrCustodyEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoricalReconciliationRowV1 {
    pub historical_id: String,
    pub historical_path: LosslessPathV1,
    pub disposition: HistoricalReconciliationDispositionV1,
}

impl HistoricalReconciliationRowV1 {
    #[must_use]
    pub fn absent_without_sufficient_evidence(
        historical_id: impl Into<String>,
        historical_path: LosslessPathV1,
    ) -> Self {
        Self {
            historical_id: historical_id.into(),
            historical_path,
            disposition:
                HistoricalReconciliationDispositionV1::AbsentWithoutSufficientDeletionOrCustodyEvidence,
        }
    }

    #[must_use]
    pub const fn outcome_code(&self) -> &'static str {
        match self.disposition {
            HistoricalReconciliationDispositionV1::PresentWithResolvedIdentity => "outcome.present_resolved",
            HistoricalReconciliationDispositionV1::PresentButChanged => "park.identity_changed",
            HistoricalReconciliationDispositionV1::AmbiguousMatch => "park.history_ambiguous",
            HistoricalReconciliationDispositionV1::AbsentWithSubstantiatedHistoricalDeletionEvidence => {
                "outcome.absent_historical_deletion"
            }
            HistoricalReconciliationDispositionV1::AbsentWithoutSufficientDeletionOrCustodyEvidence => {
                "outcome.absent_unverified"
            }
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CustodyInventoryError {
    #[error("custody inventory unit id is empty")]
    EmptyUnitId,
    #[error("an unresolved custody inventory entry requires at least one blocking reason")]
    UnresolvedWithoutReason,
    #[error("a resolved custody inventory entry cannot carry blocking reasons")]
    ResolvedWithBlockingReasons,
    #[error("a custody inventory plan contains duplicate unit ids")]
    DuplicateUnitId,
    #[error("custody inventory plan canonical encoding failed")]
    CanonicalEncoding,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> CustodyInventoryEntryV1 {
        CustodyInventoryEntryV1::new(
            id,
            CustodyUnitKindV1::StandaloneClone,
            LosslessPathV1::from_bytes(id.as_bytes().to_vec()),
            CustodyStateClassV1::Captured,
            vec![],
        )
        .unwrap()
    }

    #[test]
    fn plan_order_and_digest_do_not_depend_on_input_order() {
        let left = CustodyInventoryPlanV1::new(vec![entry("b"), entry("a")]).unwrap();
        let right = CustodyInventoryPlanV1::new(vec![entry("a"), entry("b")]).unwrap();
        assert_eq!(left.entries[0].unit_id, "a");
        assert_eq!(
            left.content_digest().unwrap(),
            right.content_digest().unwrap()
        );
    }

    #[test]
    fn duplicate_reason_codes_are_canonicalized() {
        let entry = CustodyInventoryEntryV1::new(
            "unit",
            CustodyUnitKindV1::LinkedWorktree,
            LosslessPathV1::from_bytes(b"unit".to_vec()),
            CustodyStateClassV1::Unresolved,
            vec![
                CustodyReasonCodeV1::DependencyUnresolved,
                CustodyReasonCodeV1::DependencyUnresolved,
            ],
        )
        .unwrap();
        assert_eq!(
            entry.reasons,
            vec![CustodyReasonCodeV1::DependencyUnresolved]
        );
    }

    #[test]
    fn reason_codes_match_the_closed_adr_vocabulary() {
        let observed = [
            CustodyReasonCodeV1::OwnershipUnknown.as_code(),
            CustodyReasonCodeV1::ActivityUnknown.as_code(),
            CustodyReasonCodeV1::OperationBusy.as_code(),
            CustodyReasonCodeV1::WriterUncontrolled.as_code(),
            CustodyReasonCodeV1::ConsumerProbeFailed.as_code(),
            CustodyReasonCodeV1::IdentityChanged.as_code(),
            CustodyReasonCodeV1::MountBoundary.as_code(),
            CustodyReasonCodeV1::ContentUnresolved.as_code(),
            CustodyReasonCodeV1::GitOperationInProgress.as_code(),
            CustodyReasonCodeV1::DependencyUnresolved.as_code(),
            CustodyReasonCodeV1::PrivacyHold.as_code(),
            CustodyReasonCodeV1::CustodyUnverified.as_code(),
            CustodyReasonCodeV1::RetentionUnmet.as_code(),
            CustodyReasonCodeV1::HistoryAmbiguous.as_code(),
        ];
        assert_eq!(
            observed,
            [
                "park.ownership_unknown",
                "park.activity_unknown",
                "park.operation_busy",
                "park.writer_uncontrolled",
                "park.consumer_probe_failed",
                "park.identity_changed",
                "park.mount_boundary",
                "park.content_unresolved",
                "park.git_operation_in_progress",
                "park.dependency_unresolved",
                "park.privacy_hold",
                "park.custody_unverified",
                "park.retention_unmet",
                "park.history_ambiguous",
            ]
        );
    }
}
