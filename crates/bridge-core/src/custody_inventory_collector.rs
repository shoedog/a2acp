//! Fixed-population, read-only custody metadata collection.
//!
//! Paths are supplied explicitly. This module never discovers paths, reads contents, or grants
//! sealing, promotion, deletion, or operator authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::custody_inventory::{
    CustodyInventoryEntryV1, CustodyInventoryError, CustodyInventoryPlanV1, CustodyReasonCodeV1,
    CustodyStateClassV1, CustodyUnitKindV1, HistoricalReconciliationDispositionV1,
    HistoricalReconciliationRowV1, LosslessPathV1,
};
use crate::fs_custody::BirthTimeV1;

pub const SLICE_1B_MAX_CURRENT_UNITS_V1: usize = 8;
pub const SLICE_1B_MAX_HISTORICAL_ROWS_V1: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyCollectionTargetV1 {
    pub unit_id: String,
    pub kind: CustodyUnitKindV1,
    pub path: LosslessPathV1,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoricalCandidateV1 {
    pub historical_id: String,
    pub historical_path: LosslessPathV1,
    pub expected_identity: Option<ObservedPathIdentityV1>,
}

/// A validated population. The private fields prevent bypassing the eight-row caps.
///
/// ```compile_fail
/// use bridge_core::custody_inventory_collector::FixedCustodyPopulationV1;
/// let _ = FixedCustodyPopulationV1 { current: vec![], historical: vec![] };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedCustodyPopulationV1 {
    current: Vec<CustodyCollectionTargetV1>,
    historical: Vec<HistoricalCandidateV1>,
}

impl FixedCustodyPopulationV1 {
    pub fn new(
        mut current: Vec<CustodyCollectionTargetV1>,
        mut historical: Vec<HistoricalCandidateV1>,
    ) -> Result<Self, CustodyCollectorErrorV1> {
        if !(1..=SLICE_1B_MAX_CURRENT_UNITS_V1).contains(&current.len()) {
            return Err(CustodyCollectorErrorV1::CurrentCount);
        }
        if !(1..=SLICE_1B_MAX_HISTORICAL_ROWS_V1).contains(&historical.len()) {
            return Err(CustodyCollectorErrorV1::HistoricalCount);
        }
        let mut current_ids = BTreeSet::new();
        for target in &current {
            if target.unit_id.is_empty() {
                return Err(CustodyCollectorErrorV1::EmptyCurrentId);
            }
            if !current_ids.insert(target.unit_id.as_str()) {
                return Err(CustodyCollectorErrorV1::DuplicateCurrentId);
            }
            validate_path(&target.path)?;
        }
        let mut historical_ids = BTreeSet::new();
        for candidate in &historical {
            if candidate.historical_id.is_empty() {
                return Err(CustodyCollectorErrorV1::EmptyHistoricalId);
            }
            if !historical_ids.insert(candidate.historical_id.as_str()) {
                return Err(CustodyCollectorErrorV1::DuplicateHistoricalId);
            }
            validate_path(&candidate.historical_path)?;
        }
        current.sort_by(|a, b| a.unit_id.cmp(&b.unit_id));
        historical.sort_by(|a, b| a.historical_id.cmp(&b.historical_id));
        Ok(Self {
            current,
            historical,
        })
    }

    #[must_use]
    pub fn current(&self) -> &[CustodyCollectionTargetV1] {
        &self.current
    }

    #[must_use]
    pub fn historical(&self) -> &[HistoricalCandidateV1] {
        &self.historical
    }
}

fn validate_path(path: &LosslessPathV1) -> Result<(), CustodyCollectorErrorV1> {
    if path.as_bytes().is_empty() || path.as_bytes().contains(&0) {
        return Err(CustodyCollectorErrorV1::InvalidPath);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedPathKindV1 {
    Directory,
    RegularFile,
    Other,
    Symlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedPathIdentityV1 {
    pub kind: ObservedPathKindV1,
    pub dev: u64,
    pub ino: u64,
    pub birthtime: Option<BirthTimeV1>,
}

impl ObservedPathIdentityV1 {
    /// Birthtime strengthens kind/device/inode identity only when both sides carry it.
    #[must_use]
    pub fn matches(&self, observed: &Self) -> bool {
        self.kind == observed.kind
            && self.dev == observed.dev
            && self.ino == observed.ino
            && match (self.birthtime, observed.birthtime) {
                (Some(expected), Some(actual)) => expected == actual,
                _ => true,
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathObservationV1 {
    Present(ObservedPathIdentityV1),
    Missing,
    Unreadable,
}

pub trait ReadOnlyCustodyProbeV1 {
    fn observe(&self, path: &LosslessPathV1) -> PathObservationV1;
}

#[cfg(unix)]
pub struct UnixMetadataCustodyProbeV1;

#[cfg(unix)]
impl ReadOnlyCustodyProbeV1 for UnixMetadataCustodyProbeV1 {
    fn observe(&self, path: &LosslessPathV1) -> PathObservationV1 {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::MetadataExt;

        if validate_path(path).is_err() {
            return PathObservationV1::Unreadable;
        }
        let raw = std::ffi::OsString::from_vec(path.as_bytes().to_vec());
        let path = std::path::PathBuf::from(raw);
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return PathObservationV1::Missing;
            }
            Err(_) => return PathObservationV1::Unreadable,
        };
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            ObservedPathKindV1::Symlink
        } else if file_type.is_dir() {
            ObservedPathKindV1::Directory
        } else if file_type.is_file() {
            ObservedPathKindV1::RegularFile
        } else {
            ObservedPathKindV1::Other
        };
        PathObservationV1::Present(ObservedPathIdentityV1 {
            kind,
            dev: metadata.dev(),
            ino: metadata.ino(),
            birthtime: BirthTimeV1::from_metadata(&metadata),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyCollectionResultV1 {
    pub plan: CustodyInventoryPlanV1,
    pub historical_rows: Vec<HistoricalReconciliationRowV1>,
}

pub fn collect_fixed_population_v1(
    population: FixedCustodyPopulationV1,
    probe: &impl ReadOnlyCustodyProbeV1,
) -> Result<CustodyCollectionResultV1, CustodyCollectorErrorV1> {
    let paths: BTreeSet<Vec<u8>> = population
        .current
        .iter()
        .map(|target| target.path.as_bytes().to_vec())
        .chain(
            population
                .historical
                .iter()
                .map(|candidate| candidate.historical_path.as_bytes().to_vec()),
        )
        .collect();
    let observations: BTreeMap<Vec<u8>, PathObservationV1> = paths
        .into_iter()
        .map(|bytes| {
            let observation = probe.observe(&LosslessPathV1::from_bytes(bytes.clone()));
            (bytes, observation)
        })
        .collect();

    let entries = population
        .current
        .iter()
        .map(|target| {
            let observation = observations
                .get(target.path.as_bytes())
                .expect("current path was inserted into the exact union");
            CustodyInventoryEntryV1::new(
                &target.unit_id,
                target.kind,
                target.path.clone(),
                CustodyStateClassV1::Unresolved,
                current_reasons(*observation),
            )
            .map_err(CustodyCollectorErrorV1::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plan = CustodyInventoryPlanV1::new(entries)?;
    let historical_rows = population
        .historical
        .iter()
        .map(|candidate| {
            let observation = observations
                .get(candidate.historical_path.as_bytes())
                .expect("historical path was inserted into the exact union");
            let current_matches = population
                .current
                .iter()
                .filter(|target| target.path == candidate.historical_path)
                .count();
            HistoricalReconciliationRowV1 {
                historical_id: candidate.historical_id.clone(),
                historical_path: candidate.historical_path.clone(),
                disposition: historical_disposition(
                    *observation,
                    current_matches,
                    candidate.expected_identity,
                ),
            }
        })
        .collect();
    Ok(CustodyCollectionResultV1 {
        plan,
        historical_rows,
    })
}

fn current_reasons(observation: PathObservationV1) -> Vec<CustodyReasonCodeV1> {
    use CustodyReasonCodeV1::{
        ActivityUnknown, ConsumerProbeFailed, ContentUnresolved, CustodyUnverified, MountBoundary,
    };
    match observation {
        PathObservationV1::Present(ObservedPathIdentityV1 {
            kind: ObservedPathKindV1::Symlink,
            ..
        }) => vec![MountBoundary, ContentUnresolved, CustodyUnverified],
        PathObservationV1::Missing => {
            vec![ActivityUnknown, ContentUnresolved, CustodyUnverified]
        }
        PathObservationV1::Unreadable => {
            vec![ConsumerProbeFailed, ContentUnresolved, CustodyUnverified]
        }
        PathObservationV1::Present(_) => vec![ContentUnresolved, CustodyUnverified],
    }
}

fn historical_disposition(
    observation: PathObservationV1,
    current_matches: usize,
    expected: Option<ObservedPathIdentityV1>,
) -> HistoricalReconciliationDispositionV1 {
    use HistoricalReconciliationDispositionV1::{
        AbsentWithoutSufficientDeletionOrCustodyEvidence, AmbiguousMatch, PresentButChanged,
        PresentWithResolvedIdentity,
    };
    match observation {
        PathObservationV1::Missing => AbsentWithoutSufficientDeletionOrCustodyEvidence,
        PathObservationV1::Unreadable => AmbiguousMatch,
        PathObservationV1::Present(actual)
            if matches!(
                actual.kind,
                ObservedPathKindV1::Symlink | ObservedPathKindV1::Other
            ) =>
        {
            AmbiguousMatch
        }
        PathObservationV1::Present(_) if current_matches != 1 => AmbiguousMatch,
        PathObservationV1::Present(_) if expected.is_none() => AmbiguousMatch,
        PathObservationV1::Present(actual)
            if expected.is_some_and(|prior| prior.matches(&actual)) =>
        {
            PresentWithResolvedIdentity
        }
        PathObservationV1::Present(_) => PresentButChanged,
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CustodyCollectorErrorV1 {
    #[error("current custody population must have one to eight targets")]
    CurrentCount,
    #[error("historical custody population must have one to eight candidates")]
    HistoricalCount,
    #[error("current custody target id is empty")]
    EmptyCurrentId,
    #[error("current custody target id is duplicated")]
    DuplicateCurrentId,
    #[error("historical custody candidate id is empty")]
    EmptyHistoricalId,
    #[error("historical custody candidate id is duplicated")]
    DuplicateHistoricalId,
    #[error("custody population path is empty or contains NUL")]
    InvalidPath,
    #[error(transparent)]
    Inventory(#[from] CustodyInventoryError),
}
