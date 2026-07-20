//! Owner-private evidence index and retention primitives for R3d3.
//!
//! This module is deliberately effect-local: it persists only under an injected owner-lock
//! capability. R3d5 remains the sole production root initializer and activation owner.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Write as _;
use std::os::fd::AsRawFd as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::compatibility_schedule_retention::FullEvidenceUnlinkProofV1;
use crate::compatibility_schedule_schema::{
    parse_schedule_evidence_record, portable_evidence_path_key, relative_evidence_path,
    ColdStorageBindingV1, EvidenceClassV1, EvidenceIndexEntryV1, EvidenceIndexV1,
    OptionalRelativeEvidencePathV1, OptionalSha256V1, RelativeEvidencePathV1,
    ScheduleEvidenceRecordV1, ValidateRecord,
};
use crate::compatibility_schedule_state::{EvidenceStateCapability, StateQuota};
use crate::{local_file, BoxError};

#[cfg_attr(not(test), allow(dead_code))]
pub(super) const DAY_MS: i64 = 86_400_000;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_EVIDENCE_ITEMS: usize = 256;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_STATE_RECORD_BYTES: u64 = 16 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_STATE_GENERATIONS: usize = 10_000;
#[cfg_attr(not(test), allow(dead_code))]
const STATE_FILE_MODE: u32 = 0o600;
#[cfg_attr(not(test), allow(dead_code))]
const STATE_PREFIX: &str = "evidence-state.";
#[cfg_attr(not(test), allow(dead_code))]
const HOT_TOTAL_CAP_BYTES: u64 = 10 * 1024 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const HOT_STATE_CAP_BYTES: u64 = 1024 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const HOT_SCRATCH_CAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const HOT_SEALED_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;

#[cfg_attr(not(test), allow(dead_code))]
fn require_sha256(label: &str, value: &str) -> Result<(), BoxError> {
    if !local_file::valid_sha256(value) || value.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(format!("schedule evidence: {label} is not lowercase SHA-256").into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn stable_id(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b':' | b'/' | b'.')
        })
    {
        return Err(format!("schedule evidence: {label} is not a bounded stable id").into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn bounded_text(label: &str, value: &str) -> Result<(), BoxError> {
    if value.is_empty() || value.len() > 4096 || value.bytes().any(|byte| byte == 0) {
        return Err(
            format!("schedule evidence: {label} is empty, oversized, or contains NUL").into(),
        );
    }
    if crate::compatibility::looks_like_secret(value) {
        return Err(format!("schedule evidence: {label} contains secret-shaped material").into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn add_days(timestamp_ms: i64, days: u32) -> Result<i64, BoxError> {
    if timestamp_ms <= 0 {
        return Err("schedule evidence: terminal time must be positive".into());
    }
    let duration = i64::from(days)
        .checked_mul(DAY_MS)
        .ok_or("schedule evidence: retention duration overflow")?;
    timestamp_ms
        .checked_add(duration)
        .ok_or_else(|| "schedule evidence: retention deadline overflow".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceRetentionRequestV1 {
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) terminal_at_ms: i64,
    pub(super) case_minimum_days: u32,
    pub(super) release_retain_until_ms: Option<i64>,
    pub(super) pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct RetentionDecisionV1 {
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) hot_retain_until_ms: i64,
}

#[cfg_attr(not(test), allow(dead_code))]
fn class_retention_days(class: EvidenceClassV1) -> (u32, Option<u32>, u32) {
    match class {
        EvidenceClassV1::RoutineGreen => (30, Some(180), 14),
        EvidenceClassV1::PreflightBlocked => (90, Some(180), 30),
        EvidenceClassV1::FailedOrUnknown => (180, Some(365), 30),
        EvidenceClassV1::ManualCompatibility => (90, Some(365), 30),
        EvidenceClassV1::Incident => (180, None, 30),
        EvidenceClassV1::PromotionRelease => (0, None, 30),
        EvidenceClassV1::AuthorizationBudgetAudit => (0, Some(365), 0),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn decide_retention(
    request: &EvidenceRetentionRequestV1,
) -> Result<RetentionDecisionV1, BoxError> {
    let (class_full_days, compact_days, hot_days) = class_retention_days(request.evidence_class);
    if request.evidence_class == EvidenceClassV1::PromotionRelease
        && request.release_retain_until_ms.is_none()
        && !request.pinned
    {
        return Err(
            "schedule evidence: promotion/release evidence needs a release lifetime or pin".into(),
        );
    }
    if let Some(release) = request.release_retain_until_ms {
        if release < request.terminal_at_ms {
            return Err("schedule evidence: release lifetime predates terminal publication".into());
        }
    }
    if request.pinned {
        return Ok(RetentionDecisionV1 {
            full_retain_until_ms: i64::MAX,
            compact_retain_until_ms: i64::MAX,
            hot_retain_until_ms: i64::MAX,
        });
    }
    let class_full = add_days(request.terminal_at_ms, class_full_days)?;
    let case_full = add_days(request.terminal_at_ms, request.case_minimum_days)?;
    let release = request
        .release_retain_until_ms
        .unwrap_or(request.terminal_at_ms);
    let full = class_full.max(case_full).max(release);
    let compact = match compact_days {
        Some(days) => add_days(request.terminal_at_ms, days)?.max(full),
        None => i64::MAX,
    };
    let hot = add_days(request.terminal_at_ms, hot_days)?.min(full);
    Ok(RetentionDecisionV1 {
        full_retain_until_ms: full,
        compact_retain_until_ms: compact,
        hot_retain_until_ms: hot,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct IndexedEvidenceV1 {
    pub(super) evidence_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) full_evidence_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) compact_record_sha256: String,
    pub(super) archive_bytes: u64,
    pub(super) manifest_bytes: u64,
    pub(super) compact_record_bytes: u64,
    pub(super) compact_record: String,
    pub(super) hot_path: RelativeEvidencePathV1,
    pub(super) cold_path: OptionalRelativeEvidencePathV1,
    pub(super) terminal_at_ms: i64,
    pub(super) case_minimum_days: u32,
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) hot_retain_until_ms: i64,
    pub(super) hot_present: bool,
}

impl IndexedEvidenceV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn sealed_hot_bytes(&self) -> Result<u64, BoxError> {
        self.archive_bytes
            .checked_add(self.manifest_bytes)
            .ok_or_else(|| "schedule evidence: indexed sealed byte total overflow".into())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn total_indexed_bytes(&self) -> Result<u64, BoxError> {
        self.sealed_hot_bytes()?
            .checked_add(self.compact_record_bytes)
            .ok_or_else(|| "schedule evidence: indexed hot byte total overflow".into())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn immutable_eq(&self, other: &Self) -> bool {
        self.evidence_id == other.evidence_id
            && self.evidence_class == other.evidence_class
            && self.full_evidence_sha256 == other.full_evidence_sha256
            && self.manifest_sha256 == other.manifest_sha256
            && self.compact_record_sha256 == other.compact_record_sha256
            && self.archive_bytes == other.archive_bytes
            && self.manifest_bytes == other.manifest_bytes
            && self.compact_record_bytes == other.compact_record_bytes
            && self.compact_record == other.compact_record
            && self.hot_path == other.hot_path
            && self.terminal_at_ms == other.terminal_at_ms
            && self.case_minimum_days == other.case_minimum_days
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum PinLifecycleV1 {
    Active,
    Released { released_at_ms: i64, reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidencePinV1 {
    pub(super) pin_id: String,
    pub(super) evidence_id: String,
    pub(super) reason: String,
    pub(super) created_at_ms: i64,
    pub(super) lifecycle: PinLifecycleV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum TombstoneLifecycleV1 {
    Pending,
    FullEvidenceUnlinked { unlinked_at_ms: i64 },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceTombstoneV1 {
    pub(super) tombstone_id: String,
    pub(super) evidence_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) full_evidence_sha256: String,
    pub(super) manifest_sha256: String,
    pub(super) compact_record_sha256: String,
    pub(super) archive_bytes: u64,
    pub(super) manifest_bytes: u64,
    pub(super) compact_record_bytes: u64,
    pub(super) compact_record: String,
    pub(super) hot_path: RelativeEvidencePathV1,
    pub(super) cold_path: OptionalRelativeEvidencePathV1,
    pub(super) hot_was_present: bool,
    pub(super) terminal_at_ms: i64,
    pub(super) full_retain_until_ms: i64,
    pub(super) compact_retain_until_ms: i64,
    pub(super) reason_code: String,
    pub(super) created_at_ms: i64,
    pub(super) lifecycle: TombstoneLifecycleV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum FileProviderMaterializationV1 {
    Materialized,
    Offloaded,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum FileProviderSynchronizationV1 {
    NotUploaded,
    Uploading,
    Synchronized,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum FileProviderObjectStateV1 {
    Known {
        materialization: FileProviderMaterializationV1,
        synchronization: FileProviderSynchronizationV1,
    },
    Unavailable {
        reason_code: String,
    },
    Unknown {
        reason_code: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct FileProviderObservationV1 {
    pub(super) cold_root_sha256: String,
    pub(super) file_provider_domain_id: String,
    pub(super) object_path: OptionalRelativeEvidencePathV1,
    pub(super) state: FileProviderObjectStateV1,
    pub(super) observed_at_ms: i64,
}

impl FileProviderObservationV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn validate(&self) -> Result<(), BoxError> {
        require_sha256("FileProvider cold root", &self.cold_root_sha256)?;
        bounded_text(
            "FileProvider domain identity",
            &self.file_provider_domain_id,
        )?;
        if self.observed_at_ms <= 0 {
            return Err("schedule evidence: FileProvider observation time is invalid".into());
        }
        if let OptionalRelativeEvidencePathV1::RelativePath { value } = &self.object_path {
            relative_evidence_path("FileProvider object path", value)?;
        }
        match &self.state {
            FileProviderObjectStateV1::Known { .. } => {}
            FileProviderObjectStateV1::Unavailable { reason_code }
            | FileProviderObjectStateV1::Unknown { reason_code } => {
                stable_id("FileProvider state reason", reason_code)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
// The larger terminal variant keeps both exact file-provider observations inline in the immutable
// journal record; indirection would not reduce the serialized record or this default-off path's risk.
#[allow(clippy::large_enum_variant)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ColdCopyLifecycleV1 {
    Admitted,
    Abandoned {
        abandoned_at_ms: i64,
        reason_code: String,
    },
    Published {
        published_at_ms: i64,
        archive_observation: FileProviderObservationV1,
        manifest_observation: FileProviderObservationV1,
        last_content_verified_at_ms: i64,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ColdCopyRecordV1 {
    pub(super) copy_id: String,
    pub(super) evidence_id: String,
    pub(super) archive_sha256: String,
    pub(super) archive_bytes: u64,
    pub(super) manifest_sha256: String,
    pub(super) manifest_bytes: u64,
    pub(super) consent_id: String,
    pub(super) consent_sha256: String,
    pub(super) consent_revocation_generation: u64,
    pub(super) cold_root_sha256: String,
    pub(super) file_provider_domain_id: String,
    pub(super) archive_path: RelativeEvidencePathV1,
    pub(super) manifest_path: RelativeEvidencePathV1,
    pub(super) deadline_ms: i64,
    pub(super) admitted_at_ms: i64,
    pub(super) lifecycle: ColdCopyLifecycleV1,
}

impl ColdCopyRecordV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("cold-copy id", &self.copy_id)?;
        stable_id("cold-copy evidence id", &self.evidence_id)?;
        stable_id("cold-copy consent id", &self.consent_id)?;
        require_sha256("cold-copy archive", &self.archive_sha256)?;
        require_sha256("cold-copy manifest", &self.manifest_sha256)?;
        require_sha256("cold-copy consent", &self.consent_sha256)?;
        require_sha256("cold-copy root", &self.cold_root_sha256)?;
        bounded_text(
            "cold-copy FileProvider domain",
            &self.file_provider_domain_id,
        )?;
        relative_evidence_path("cold-copy archive path", &self.archive_path)?;
        relative_evidence_path("cold-copy manifest path", &self.manifest_path)?;
        if portable_evidence_path_key(&self.archive_path)
            == portable_evidence_path_key(&self.manifest_path)
            || self.archive_bytes == 0
            || self.manifest_bytes == 0
            || self.consent_revocation_generation == 0
            || self.admitted_at_ms <= 0
            || self.deadline_ms <= self.admitted_at_ms
        {
            return Err("schedule evidence: cold-copy identity or bound is invalid".into());
        }
        if let ColdCopyLifecycleV1::Published {
            published_at_ms,
            archive_observation,
            manifest_observation,
            last_content_verified_at_ms,
        } = &self.lifecycle
        {
            archive_observation.validate()?;
            manifest_observation.validate()?;
            if *published_at_ms < self.admitted_at_ms
                || *published_at_ms > self.deadline_ms
                || *last_content_verified_at_ms < *published_at_ms
                || *last_content_verified_at_ms > archive_observation.observed_at_ms
                || *last_content_verified_at_ms > manifest_observation.observed_at_ms
                || archive_observation.cold_root_sha256 != self.cold_root_sha256
                || manifest_observation.cold_root_sha256 != self.cold_root_sha256
                || archive_observation.file_provider_domain_id != self.file_provider_domain_id
                || manifest_observation.file_provider_domain_id != self.file_provider_domain_id
                || archive_observation.object_path
                    != (OptionalRelativeEvidencePathV1::RelativePath {
                        value: self.archive_path.clone(),
                    })
                || manifest_observation.object_path
                    != (OptionalRelativeEvidencePathV1::RelativePath {
                        value: self.manifest_path.clone(),
                    })
            {
                return Err("schedule evidence: published cold-copy state is unbound".into());
            }
        }
        if let ColdCopyLifecycleV1::Abandoned {
            abandoned_at_ms,
            reason_code,
        } = &self.lifecycle
        {
            stable_id("cold-copy abandonment reason", reason_code)?;
            if *abandoned_at_ms <= self.deadline_ms {
                return Err(
                    "schedule evidence: cold-copy abandonment does not follow its deadline".into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct StorageIntegrityHoldV1 {
    pub(super) hold_id: String,
    pub(super) evidence_id: String,
    pub(super) reason_code: String,
    pub(super) detected_at_ms: i64,
}

impl StorageIntegrityHoldV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("storage-integrity hold id", &self.hold_id)?;
        stable_id("storage-integrity evidence id", &self.evidence_id)?;
        stable_id("storage-integrity reason", &self.reason_code)?;
        if self.detected_at_ms <= 0 {
            return Err("schedule evidence: storage-integrity hold time is invalid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum BundleGcLifecycleV1 {
    Pending,
    Unlinked {
        unlinked_at_ms: i64,
    },
    SafeSkipped {
        skipped_at_ms: i64,
        reason_code: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct BundleGcActionV1 {
    pub(super) action_id: String,
    pub(super) bundle_id: String,
    pub(super) evidence_id: String,
    pub(super) provider_id: String,
    pub(super) case_id: String,
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) cache_root_sha256: String,
    pub(super) path: RelativeEvidencePathV1,
    pub(super) content_sha256: String,
    pub(super) length_bytes: u64,
    pub(super) preserved_in_full_evidence_sha256: String,
    pub(super) reason_code: String,
    pub(super) planned_at_ms: i64,
    pub(super) started_at_ms: i64,
    pub(super) lifecycle: BundleGcLifecycleV1,
}

impl BundleGcActionV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("bundle GC action id", &self.action_id)?;
        stable_id("bundle GC bundle id", &self.bundle_id)?;
        stable_id("bundle GC evidence id", &self.evidence_id)?;
        stable_id("bundle GC provider id", &self.provider_id)?;
        stable_id("bundle GC case id", &self.case_id)?;
        stable_id("bundle GC reason", &self.reason_code)?;
        require_sha256("bundle GC cache root", &self.cache_root_sha256)?;
        require_sha256("bundle GC content", &self.content_sha256)?;
        require_sha256(
            "bundle GC preserved full evidence",
            &self.preserved_in_full_evidence_sha256,
        )?;
        relative_evidence_path("bundle GC path", &self.path)?;
        if self.path.components.len() != 1
            || self.length_bytes == 0
            || self.planned_at_ms <= 0
            || self.started_at_ms < self.planned_at_ms
        {
            return Err("schedule evidence: bundle GC action identity is invalid".into());
        }
        match &self.lifecycle {
            BundleGcLifecycleV1::Pending => {}
            BundleGcLifecycleV1::Unlinked { unlinked_at_ms } => {
                if *unlinked_at_ms < self.started_at_ms {
                    return Err("schedule evidence: bundle GC unlink predates its intent".into());
                }
            }
            BundleGcLifecycleV1::SafeSkipped {
                skipped_at_ms,
                reason_code,
            } => {
                stable_id("bundle GC skip reason", reason_code)?;
                if *skipped_at_ms < self.started_at_ms {
                    return Err("schedule evidence: bundle GC skip predates its intent".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum ImageGcLifecycleV1 {
    Pending,
    Removed {
        removed_at_ms: i64,
    },
    SafeSkipped {
        skipped_at_ms: i64,
        reason_code: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct ImageGcActionV1 {
    pub(super) action_id: String,
    pub(super) digest: String,
    pub(super) planned_inventory_sha256: String,
    pub(super) planned_at_ms: i64,
    pub(super) started_at_ms: i64,
    pub(super) lifecycle: ImageGcLifecycleV1,
}

#[cfg_attr(not(test), allow(dead_code))]
fn require_image_digest(label: &str, value: &str) -> Result<(), BoxError> {
    let Some(sha256) = value.strip_prefix("sha256:") else {
        return Err(format!("schedule evidence: {label} is not an immutable digest").into());
    };
    require_sha256(label, sha256)
}

impl ImageGcActionV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("image GC action id", &self.action_id)?;
        require_image_digest("image GC digest", &self.digest)?;
        require_sha256("image GC planned inventory", &self.planned_inventory_sha256)?;
        if self.planned_at_ms <= 0 || self.started_at_ms < self.planned_at_ms {
            return Err("schedule evidence: image GC action time is invalid".into());
        }
        match &self.lifecycle {
            ImageGcLifecycleV1::Pending => {}
            ImageGcLifecycleV1::Removed { removed_at_ms } => {
                if *removed_at_ms < self.started_at_ms {
                    return Err("schedule evidence: image GC removal predates its intent".into());
                }
            }
            ImageGcLifecycleV1::SafeSkipped {
                skipped_at_ms,
                reason_code,
            } => {
                stable_id("image GC skip reason", reason_code)?;
                if *skipped_at_ms < self.started_at_ms {
                    return Err("schedule evidence: image GC skip predates its intent".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceStateModelV1 {
    pub(super) hot_root_sha256: String,
    pub(super) cold_storage: ColdStorageBindingV1,
    pub(super) entries: BTreeMap<String, IndexedEvidenceV1>,
    pub(super) pins: BTreeMap<String, EvidencePinV1>,
    pub(super) tombstones: BTreeMap<String, EvidenceTombstoneV1>,
    pub(super) retired_evidence_ids: BTreeSet<String>,
    #[serde(default)]
    pub(super) cold_copies: BTreeMap<String, ColdCopyRecordV1>,
    #[serde(default)]
    pub(super) storage_integrity_holds: BTreeMap<String, StorageIntegrityHoldV1>,
    #[serde(default)]
    pub(super) bundle_gc_actions: BTreeMap<String, BundleGcActionV1>,
    #[serde(default)]
    pub(super) image_gc_actions: BTreeMap<String, ImageGcActionV1>,
}

impl EvidenceStateModelV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn new(
        hot_root_sha256: String,
        cold_storage: ColdStorageBindingV1,
    ) -> Result<Self, BoxError> {
        let value = Self {
            hot_root_sha256,
            cold_storage,
            entries: BTreeMap::new(),
            pins: BTreeMap::new(),
            tombstones: BTreeMap::new(),
            retired_evidence_ids: BTreeSet::new(),
            cold_copies: BTreeMap::new(),
            storage_integrity_holds: BTreeMap::new(),
            bundle_gc_actions: BTreeMap::new(),
            image_gc_actions: BTreeMap::new(),
        };
        value.validate()?;
        Ok(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn validate(&self) -> Result<(), BoxError> {
        require_sha256("hot root", &self.hot_root_sha256)?;
        if self.entries.len() > MAX_EVIDENCE_ITEMS
            || self.pins.len() > MAX_EVIDENCE_ITEMS * 4
            || self.tombstones.len() > MAX_EVIDENCE_ITEMS * 4
            || self.retired_evidence_ids.len() > MAX_EVIDENCE_ITEMS * 4
            || self.cold_copies.len() > MAX_EVIDENCE_ITEMS * 4
            || self.storage_integrity_holds.len() > MAX_EVIDENCE_ITEMS * 4
            || self.bundle_gc_actions.len() > MAX_EVIDENCE_ITEMS * 4
            || self.image_gc_actions.len() > MAX_EVIDENCE_ITEMS * 4
        {
            return Err("schedule evidence: state collections exceed their bounds".into());
        }
        for (id, entry) in &self.entries {
            if id != &entry.evidence_id || self.retired_evidence_ids.contains(id) {
                return Err("schedule evidence: entry key is mismatched or retired".into());
            }
            stable_id("evidence id", id)?;
            require_sha256("full evidence", &entry.full_evidence_sha256)?;
            require_sha256("evidence manifest", &entry.manifest_sha256)?;
            require_sha256("compact record", &entry.compact_record_sha256)?;
            if entry.archive_bytes == 0
                || entry.manifest_bytes == 0
                || entry.compact_record_bytes == 0
                || entry.compact_record.len() as u64 != entry.compact_record_bytes
                || local_file::sha256_hex(entry.compact_record.as_bytes())
                    != entry.compact_record_sha256
                || !entry.compact_record.ends_with('\n')
                || entry.total_indexed_bytes().is_err()
                || entry.terminal_at_ms <= 0
                || entry.full_retain_until_ms < entry.terminal_at_ms
                || entry.compact_retain_until_ms < entry.full_retain_until_ms
                || entry.hot_retain_until_ms < entry.terminal_at_ms
                || entry.hot_retain_until_ms > entry.full_retain_until_ms
            {
                return Err(
                    "schedule evidence: indexed bytes or retention clocks are invalid".into(),
                );
            }
            validate_compact_record_material(
                &entry.compact_record,
                &entry.evidence_id,
                entry.evidence_class,
                entry.terminal_at_ms,
                &entry.full_evidence_sha256,
                &entry.manifest_sha256,
            )?;
            let minimum = decide_retention(&EvidenceRetentionRequestV1 {
                evidence_class: entry.evidence_class,
                terminal_at_ms: entry.terminal_at_ms,
                case_minimum_days: entry.case_minimum_days,
                release_retain_until_ms: if entry.evidence_class
                    == EvidenceClassV1::PromotionRelease
                {
                    Some(entry.full_retain_until_ms)
                } else {
                    None
                },
                pinned: false,
            })?;
            if entry.full_retain_until_ms < minimum.full_retain_until_ms
                || entry.compact_retain_until_ms < minimum.compact_retain_until_ms
                || entry.hot_retain_until_ms < minimum.hot_retain_until_ms
            {
                return Err("schedule evidence: indexed retention shortens policy".into());
            }
            if !entry.hot_present
                && !matches!(
                    entry.cold_path,
                    OptionalRelativeEvidencePathV1::RelativePath { .. }
                )
            {
                return Err("schedule evidence: absent hot bytes require a cold object".into());
            }
        }

        for (id, pin) in &self.pins {
            if id != &pin.pin_id || pin.created_at_ms <= 0 {
                return Err("schedule evidence: pin key/time is invalid".into());
            }
            stable_id("pin id", id)?;
            stable_id("pinned evidence id", &pin.evidence_id)?;
            bounded_text("pin reason", &pin.reason)?;
            match &pin.lifecycle {
                PinLifecycleV1::Active if !self.entries.contains_key(&pin.evidence_id) => {
                    return Err("schedule evidence: active pin has no live evidence".into())
                }
                PinLifecycleV1::Active
                    if self.tombstones.values().any(|tombstone| {
                        tombstone.evidence_id == pin.evidence_id
                            && tombstone.lifecycle == TombstoneLifecycleV1::Pending
                    }) =>
                {
                    return Err(
                        "schedule evidence: active pin conflicts with pending tombstone".into(),
                    )
                }
                PinLifecycleV1::Released {
                    released_at_ms,
                    reason,
                } => {
                    if *released_at_ms < pin.created_at_ms {
                        return Err("schedule evidence: pin release predates creation".into());
                    }
                    bounded_text("pin release reason", reason)?;
                }
                _ => {}
            }
        }

        for (id, tombstone) in &self.tombstones {
            if id != &tombstone.tombstone_id || tombstone.created_at_ms <= 0 {
                return Err("schedule evidence: tombstone key/time is invalid".into());
            }
            stable_id("tombstone id", id)?;
            stable_id("tombstoned evidence id", &tombstone.evidence_id)?;
            stable_id("tombstone reason", &tombstone.reason_code)?;
            require_sha256("tombstoned evidence", &tombstone.full_evidence_sha256)?;
            require_sha256("tombstoned manifest", &tombstone.manifest_sha256)?;
            require_sha256(
                "tombstoned compact record",
                &tombstone.compact_record_sha256,
            )?;
            relative_evidence_path("tombstoned hot evidence path", &tombstone.hot_path)?;
            match (&tombstone.cold_path, &self.cold_storage) {
                (OptionalRelativeEvidencePathV1::Absent, _) => {}
                (
                    OptionalRelativeEvidencePathV1::RelativePath { value },
                    ColdStorageBindingV1::OwnerIcloud { .. },
                ) => relative_evidence_path("tombstoned cold evidence path", value)?,
                (OptionalRelativeEvidencePathV1::RelativePath { .. }, _) => {
                    return Err(
                        "schedule evidence: tombstone cold path has no bound cold root".into(),
                    )
                }
            }
            if tombstone.archive_bytes == 0
                || tombstone.manifest_bytes == 0
                || tombstone.compact_record_bytes == 0
                || tombstone.compact_record.len() as u64 != tombstone.compact_record_bytes
                || local_file::sha256_hex(tombstone.compact_record.as_bytes())
                    != tombstone.compact_record_sha256
                || !tombstone.compact_record.ends_with('\n')
                || tombstone
                    .archive_bytes
                    .checked_add(tombstone.manifest_bytes)
                    .is_none()
                || tombstone.full_retain_until_ms <= 0
                || tombstone.compact_retain_until_ms < tombstone.full_retain_until_ms
                || tombstone.created_at_ms < tombstone.full_retain_until_ms
                || (!tombstone.hot_was_present
                    && !matches!(
                        tombstone.cold_path,
                        OptionalRelativeEvidencePathV1::RelativePath { .. }
                    ))
            {
                return Err("schedule evidence: tombstone deletion identity is invalid".into());
            }
            validate_compact_record_material(
                &tombstone.compact_record,
                &tombstone.evidence_id,
                tombstone.evidence_class,
                tombstone.terminal_at_ms,
                &tombstone.full_evidence_sha256,
                &tombstone.manifest_sha256,
            )?;
            match tombstone.lifecycle {
                TombstoneLifecycleV1::Pending => {
                    let entry = self
                        .entries
                        .get(&tombstone.evidence_id)
                        .ok_or("schedule evidence: pending tombstone has no indexed entry")?;
                    if entry.full_evidence_sha256 != tombstone.full_evidence_sha256
                        || entry.evidence_class != tombstone.evidence_class
                        || entry.manifest_sha256 != tombstone.manifest_sha256
                        || entry.compact_record_sha256 != tombstone.compact_record_sha256
                        || entry.archive_bytes != tombstone.archive_bytes
                        || entry.manifest_bytes != tombstone.manifest_bytes
                        || entry.compact_record_bytes != tombstone.compact_record_bytes
                        || entry.compact_record != tombstone.compact_record
                        || entry.hot_path != tombstone.hot_path
                        || entry.cold_path != tombstone.cold_path
                        || entry.hot_present != tombstone.hot_was_present
                        || entry.terminal_at_ms != tombstone.terminal_at_ms
                        || entry.full_retain_until_ms != tombstone.full_retain_until_ms
                        || entry.compact_retain_until_ms != tombstone.compact_retain_until_ms
                    {
                        return Err("schedule evidence: pending tombstone identity mismatch".into());
                    }
                }
                TombstoneLifecycleV1::FullEvidenceUnlinked { unlinked_at_ms } => {
                    if unlinked_at_ms < tombstone.created_at_ms
                        || self.entries.contains_key(&tombstone.evidence_id)
                        || !self.retired_evidence_ids.contains(&tombstone.evidence_id)
                    {
                        return Err(
                            "schedule evidence: completed tombstone state is inconsistent".into(),
                        );
                    }
                }
            }
        }

        let cold_root_and_domain = match &self.cold_storage {
            ColdStorageBindingV1::Absent => None,
            ColdStorageBindingV1::OwnerIcloud {
                root_sha256,
                file_provider_domain_id,
                ..
            } => Some((root_sha256, file_provider_domain_id)),
        };
        let mut active_copy_evidence = BTreeSet::new();
        let mut published_evidence = BTreeSet::new();
        for (id, copy) in &self.cold_copies {
            if id != &copy.copy_id
                || (!matches!(copy.lifecycle, ColdCopyLifecycleV1::Abandoned { .. })
                    && !active_copy_evidence.insert(copy.evidence_id.clone()))
            {
                return Err(
                    "schedule evidence: cold-copy key or evidence target is duplicated".into(),
                );
            }
            copy.validate()?;
            let Some((root_sha256, domain_id)) = cold_root_and_domain else {
                return Err("schedule evidence: cold-copy history has no bound cold root".into());
            };
            if &copy.cold_root_sha256 != root_sha256 || &copy.file_provider_domain_id != domain_id {
                return Err("schedule evidence: cold-copy root or domain diverges".into());
            }
            let (
                archive_sha256,
                archive_bytes,
                manifest_sha256,
                manifest_bytes,
                cold_path,
                terminal_at_ms,
            ) = if let Some(entry) = self.entries.get(&copy.evidence_id) {
                (
                    &entry.full_evidence_sha256,
                    entry.archive_bytes,
                    &entry.manifest_sha256,
                    entry.manifest_bytes,
                    &entry.cold_path,
                    entry.terminal_at_ms,
                )
            } else {
                let tombstone = self
                    .tombstones
                    .values()
                    .find(|value| {
                        value.evidence_id == copy.evidence_id
                            && matches!(
                                value.lifecycle,
                                TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
                            )
                    })
                    .ok_or("schedule evidence: cold-copy target disappeared")?;
                (
                    &tombstone.full_evidence_sha256,
                    tombstone.archive_bytes,
                    &tombstone.manifest_sha256,
                    tombstone.manifest_bytes,
                    &tombstone.cold_path,
                    tombstone.terminal_at_ms,
                )
            };
            if &copy.archive_sha256 != archive_sha256
                || copy.archive_bytes != archive_bytes
                || &copy.manifest_sha256 != manifest_sha256
                || copy.manifest_bytes != manifest_bytes
                || copy.admitted_at_ms < terminal_at_ms
            {
                return Err("schedule evidence: cold-copy content identity diverges".into());
            }
            match &copy.lifecycle {
                ColdCopyLifecycleV1::Admitted => {
                    if !self.entries.contains_key(&copy.evidence_id)
                        || !matches!(cold_path, OptionalRelativeEvidencePathV1::Absent)
                        || self.tombstones.values().any(|value| {
                            value.evidence_id == copy.evidence_id
                                && value.lifecycle == TombstoneLifecycleV1::Pending
                        })
                    {
                        return Err(
                            "schedule evidence: admitted cold copy is not exclusively pending"
                                .into(),
                        );
                    }
                }
                ColdCopyLifecycleV1::Abandoned { .. } => {}
                ColdCopyLifecycleV1::Published { .. } => {
                    if !published_evidence.insert(copy.evidence_id.clone())
                        || cold_path
                            != &(OptionalRelativeEvidencePathV1::RelativePath {
                                value: copy.archive_path.clone(),
                            })
                    {
                        return Err("schedule evidence: published cold copy is not indexed".into());
                    }
                }
            }
        }
        for entry in self.entries.values() {
            if matches!(
                entry.cold_path,
                OptionalRelativeEvidencePathV1::RelativePath { .. }
            ) && !published_evidence.contains(&entry.evidence_id)
            {
                return Err("schedule evidence: indexed cold path has no published copy".into());
            }
            if !entry.hot_present {
                let copy = self
                    .cold_copies
                    .values()
                    .find(|copy| {
                        copy.evidence_id == entry.evidence_id
                            && matches!(copy.lifecycle, ColdCopyLifecycleV1::Published { .. })
                    })
                    .ok_or("schedule evidence: cold-only entry has no copy record")?;
                let ColdCopyLifecycleV1::Published {
                    archive_observation,
                    manifest_observation,
                    ..
                } = &copy.lifecycle
                else {
                    return Err("schedule evidence: cold-only entry is not published".into());
                };
                for observation in [archive_observation, manifest_observation] {
                    if !matches!(
                        observation.state,
                        FileProviderObjectStateV1::Known {
                            synchronization: FileProviderSynchronizationV1::Synchronized,
                            ..
                        }
                    ) {
                        return Err(
                            "schedule evidence: cold-only entry is not known synchronized".into(),
                        );
                    }
                }
            }
        }

        for (id, hold) in &self.storage_integrity_holds {
            if id != &hold.hold_id
                || (!self.entries.contains_key(&hold.evidence_id)
                    && !self.retired_evidence_ids.contains(&hold.evidence_id))
            {
                return Err("schedule evidence: storage-integrity hold target is invalid".into());
            }
            hold.validate()?;
        }

        let mut pending_bundles = BTreeSet::new();
        for (id, action) in &self.bundle_gc_actions {
            if id != &action.action_id {
                return Err("schedule evidence: bundle GC action key is invalid".into());
            }
            action.validate()?;
            if action.lifecycle == BundleGcLifecycleV1::Pending
                && !pending_bundles.insert(action.bundle_id.clone())
            {
                return Err("schedule evidence: bundle has multiple pending GC actions".into());
            }
        }

        let mut pending_images = BTreeSet::new();
        for (id, action) in &self.image_gc_actions {
            if id != &action.action_id {
                return Err("schedule evidence: image GC action key is invalid".into());
            }
            action.validate()?;
            if action.lifecycle == ImageGcLifecycleV1::Pending
                && !pending_images.insert(action.digest.clone())
            {
                return Err("schedule evidence: image has multiple pending GC actions".into());
            }
        }

        for retired in &self.retired_evidence_ids {
            stable_id("retired evidence id", retired)?;
            if !self.tombstones.values().any(|tombstone| {
                tombstone.evidence_id == *retired
                    && matches!(
                        tombstone.lifecycle,
                        TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
                    )
            }) {
                return Err("schedule evidence: retired id has no completed tombstone".into());
            }
        }
        self.project_index_at(1)?.validate()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn project_index_at(&self, generation: u64) -> Result<EvidenceIndexV1, BoxError> {
        let entries = self
            .entries
            .values()
            .map(|entry| EvidenceIndexEntryV1 {
                evidence_id: entry.evidence_id.clone(),
                evidence_class: entry.evidence_class,
                full_evidence_sha256: entry.full_evidence_sha256.clone(),
                compact_record_sha256: entry.compact_record_sha256.clone(),
                hot_path: entry.hot_path.clone(),
                cold_path: entry.cold_path.clone(),
                full_retain_until_ms: entry.full_retain_until_ms,
                compact_retain_until_ms: entry.compact_retain_until_ms,
                pinned: self.pins.values().any(|pin| {
                    pin.evidence_id == entry.evidence_id && pin.lifecycle == PinLifecycleV1::Active
                }),
                // Cross-process flock leases are the deletion authority. This projection does not
                // claim a race-prone durable reader count.
                lease_count: 0,
            })
            .collect();
        let index = EvidenceIndexV1 {
            schema_version: 1,
            index_id: "owner-evidence-index".into(),
            generation,
            hot_root_sha256: self.hot_root_sha256.clone(),
            cold_storage: self.cold_storage.clone(),
            entries,
        };
        index.validate()?;
        Ok(index)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn insert_entry(&mut self, entry: IndexedEvidenceV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.entries.contains_key(&entry.evidence_id)
            || candidate.retired_evidence_ids.contains(&entry.evidence_id)
            || candidate
                .tombstones
                .values()
                .any(|value| value.evidence_id == entry.evidence_id)
        {
            return Err("schedule evidence: evidence id is already live or retired".into());
        }
        candidate.entries.insert(entry.evidence_id.clone(), entry);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn admit_cold_copy(
        &mut self,
        binding: ColdStorageBindingV1,
        copy: ColdCopyRecordV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        copy.validate()?;
        let ColdStorageBindingV1::OwnerIcloud {
            consent_id,
            consent_sha256,
            root_sha256,
            file_provider_domain_id,
        } = &binding
        else {
            return Err("schedule evidence: cold-copy admission needs owner-iCloud binding".into());
        };
        if copy.consent_id != *consent_id
            || copy.consent_sha256 != *consent_sha256
            || copy.cold_root_sha256 != *root_sha256
            || copy.file_provider_domain_id != *file_provider_domain_id
            || copy.lifecycle != ColdCopyLifecycleV1::Admitted
            || candidate.cold_copies.contains_key(&copy.copy_id)
            || candidate.cold_copies.values().any(|value| {
                value.evidence_id == copy.evidence_id
                    && !matches!(value.lifecycle, ColdCopyLifecycleV1::Abandoned { .. })
            })
            || candidate.tombstones.values().any(|value| {
                value.evidence_id == copy.evidence_id
                    && value.lifecycle == TombstoneLifecycleV1::Pending
            })
        {
            return Err(
                "schedule evidence: cold-copy admission identity is invalid or reused".into(),
            );
        }
        let entry = candidate
            .entries
            .get(&copy.evidence_id)
            .ok_or("schedule evidence: cold-copy target does not exist")?;
        if !entry.hot_present
            || !matches!(entry.cold_path, OptionalRelativeEvidencePathV1::Absent)
            || entry.full_evidence_sha256 != copy.archive_sha256
            || entry.archive_bytes != copy.archive_bytes
            || entry.manifest_sha256 != copy.manifest_sha256
            || entry.manifest_bytes != copy.manifest_bytes
        {
            return Err("schedule evidence: cold-copy target identity is invalid".into());
        }
        match &candidate.cold_storage {
            ColdStorageBindingV1::Absent => {}
            ColdStorageBindingV1::OwnerIcloud {
                root_sha256: current_root,
                file_provider_domain_id: current_domain,
                ..
            } if current_root == root_sha256 && current_domain == file_provider_domain_id => {}
            ColdStorageBindingV1::OwnerIcloud { .. } => {
                return Err("schedule evidence: cold-copy admission changes root or domain".into())
            }
        }
        candidate.cold_storage = binding;
        candidate.cold_copies.insert(copy.copy_id.clone(), copy);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn abandon_cold_copy(
        &mut self,
        copy_id: &str,
        reason_code: &str,
        abandoned_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let copy = candidate
            .cold_copies
            .get_mut(copy_id)
            .ok_or("schedule evidence: cold-copy admission does not exist")?;
        if copy.lifecycle != ColdCopyLifecycleV1::Admitted {
            return Err("schedule evidence: only an admitted cold copy can be abandoned".into());
        }
        if abandoned_at_ms <= copy.deadline_ms {
            return Err("schedule evidence: cold-copy admission has not expired".into());
        }
        copy.lifecycle = ColdCopyLifecycleV1::Abandoned {
            abandoned_at_ms,
            reason_code: reason_code.into(),
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn publish_cold_copy(
        &mut self,
        copy_id: &str,
        archive_observation: FileProviderObservationV1,
        manifest_observation: FileProviderObservationV1,
        published_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let (evidence_id, archive_path) = {
            let copy = candidate
                .cold_copies
                .get_mut(copy_id)
                .ok_or("schedule evidence: cold-copy admission does not exist")?;
            if copy.lifecycle != ColdCopyLifecycleV1::Admitted {
                return Err("schedule evidence: cold copy is already published".into());
            }
            let evidence_id = copy.evidence_id.clone();
            let archive_path = copy.archive_path.clone();
            copy.lifecycle = ColdCopyLifecycleV1::Published {
                published_at_ms,
                archive_observation,
                manifest_observation,
                last_content_verified_at_ms: published_at_ms,
            };
            (evidence_id, archive_path)
        };
        let entry = candidate
            .entries
            .get_mut(&evidence_id)
            .ok_or("schedule evidence: cold-copy target disappeared")?;
        if !matches!(entry.cold_path, OptionalRelativeEvidencePathV1::Absent) {
            return Err("schedule evidence: cold-copy target already has a cold path".into());
        }
        entry.cold_path = OptionalRelativeEvidencePathV1::RelativePath {
            value: archive_path,
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn update_published_cold_copy(
        &mut self,
        copy_id: &str,
        archive_observation: FileProviderObservationV1,
        manifest_observation: FileProviderObservationV1,
        content_verified_at_ms: Option<i64>,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let copy = candidate
            .cold_copies
            .get_mut(copy_id)
            .ok_or("schedule evidence: published cold copy does not exist")?;
        let ColdCopyLifecycleV1::Published {
            published_at_ms,
            last_content_verified_at_ms,
            ..
        } = &copy.lifecycle
        else {
            return Err("schedule evidence: cold copy is not published".into());
        };
        let published_at_ms = *published_at_ms;
        let last_content_verified_at_ms = content_verified_at_ms
            .unwrap_or(*last_content_verified_at_ms)
            .max(*last_content_verified_at_ms);
        copy.lifecycle = ColdCopyLifecycleV1::Published {
            published_at_ms,
            archive_observation,
            manifest_observation,
            last_content_verified_at_ms,
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn mark_hot_evidence_absent(&mut self, evidence_id: &str) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.has_active_pin(evidence_id) {
            return Err("schedule evidence: pinned hot evidence cannot be evicted".into());
        }
        let entry = candidate
            .entries
            .get_mut(evidence_id)
            .ok_or("schedule evidence: hot-eviction target does not exist")?;
        if !entry.hot_present
            || !matches!(
                entry.cold_path,
                OptionalRelativeEvidencePathV1::RelativePath { .. }
            )
        {
            return Err("schedule evidence: hot-eviction target is not eligible".into());
        }
        entry.hot_present = false;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn add_storage_integrity_hold(
        &mut self,
        hold: StorageIntegrityHoldV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        hold.validate()?;
        if candidate
            .storage_integrity_holds
            .contains_key(&hold.hold_id)
        {
            return Err("schedule evidence: storage-integrity hold already exists".into());
        }
        candidate
            .storage_integrity_holds
            .insert(hold.hold_id.clone(), hold);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn pin(&mut self, pin: EvidencePinV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.pins.contains_key(&pin.pin_id) || pin.lifecycle != PinLifecycleV1::Active {
            return Err("schedule evidence: pin must be a new active record".into());
        }
        if candidate.tombstones.values().any(|tombstone| {
            tombstone.evidence_id == pin.evidence_id
                && tombstone.lifecycle == TombstoneLifecycleV1::Pending
        }) {
            return Err("schedule evidence: pending tombstone blocks a new pin".into());
        }
        candidate.pins.insert(pin.pin_id.clone(), pin);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn unpin(
        &mut self,
        pin_id: &str,
        reason: &str,
        released_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let evidence_id = {
            let pin = candidate
                .pins
                .get(pin_id)
                .ok_or("schedule evidence: pin does not exist")?;
            if pin.lifecycle != PinLifecycleV1::Active {
                return Err("schedule evidence: pin is already released".into());
            }
            pin.evidence_id.clone()
        };
        let entry = candidate
            .entries
            .get_mut(&evidence_id)
            .ok_or("schedule evidence: active pin target disappeared")?;
        if entry.evidence_class == EvidenceClassV1::Incident {
            let release_lifetime = add_days(released_at_ms, 180)?;
            entry.full_retain_until_ms = entry.full_retain_until_ms.max(release_lifetime);
            entry.compact_retain_until_ms = i64::MAX;
        }
        let pin = candidate
            .pins
            .get_mut(pin_id)
            .ok_or("schedule evidence: pin disappeared during release")?;
        pin.lifecycle = PinLifecycleV1::Released {
            released_at_ms,
            reason: reason.into(),
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn has_active_pin(&self, evidence_id: &str) -> bool {
        self.pins
            .values()
            .any(|pin| pin.evidence_id == evidence_id && pin.lifecycle == PinLifecycleV1::Active)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn has_admitted_cold_copy(&self, evidence_id: &str) -> bool {
        self.cold_copies.values().any(|copy| {
            copy.evidence_id == evidence_id && copy.lifecycle == ColdCopyLifecycleV1::Admitted
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn begin_tombstone(
        &mut self,
        tombstone_id: &str,
        evidence_id: &str,
        reason_code: &str,
        created_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        if candidate.tombstones.contains_key(tombstone_id)
            || candidate.has_active_pin(evidence_id)
            || candidate.has_admitted_cold_copy(evidence_id)
        {
            return Err(
                "schedule evidence: tombstone id exists, evidence is pinned, or an admitted cold copy is unresolved"
                    .into(),
            );
        }
        let entry = candidate
            .entries
            .get(evidence_id)
            .ok_or("schedule evidence: tombstone target does not exist")?;
        if created_at_ms < entry.full_retain_until_ms {
            return Err("schedule evidence: full-evidence retention has not elapsed".into());
        }
        let tombstone = EvidenceTombstoneV1 {
            tombstone_id: tombstone_id.into(),
            evidence_id: evidence_id.into(),
            evidence_class: entry.evidence_class,
            full_evidence_sha256: entry.full_evidence_sha256.clone(),
            manifest_sha256: entry.manifest_sha256.clone(),
            compact_record_sha256: entry.compact_record_sha256.clone(),
            archive_bytes: entry.archive_bytes,
            manifest_bytes: entry.manifest_bytes,
            compact_record_bytes: entry.compact_record_bytes,
            compact_record: entry.compact_record.clone(),
            hot_path: entry.hot_path.clone(),
            cold_path: entry.cold_path.clone(),
            hot_was_present: entry.hot_present,
            terminal_at_ms: entry.terminal_at_ms,
            full_retain_until_ms: entry.full_retain_until_ms,
            compact_retain_until_ms: entry.compact_retain_until_ms,
            reason_code: reason_code.into(),
            created_at_ms,
            lifecycle: TombstoneLifecycleV1::Pending,
        };
        candidate.tombstones.insert(tombstone_id.into(), tombstone);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn complete_tombstone(
        &mut self,
        proof: FullEvidenceUnlinkProofV1,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let tombstone = candidate
            .tombstones
            .get_mut(proof.tombstone_id())
            .ok_or("schedule evidence: tombstone does not exist")?;
        if tombstone.lifecycle != TombstoneLifecycleV1::Pending {
            return Err("schedule evidence: tombstone is already complete".into());
        }
        if tombstone.evidence_id != proof.evidence_id() {
            return Err("schedule evidence: unlink proof targets different evidence".into());
        }
        let evidence_id = tombstone.evidence_id.clone();
        tombstone.lifecycle = TombstoneLifecycleV1::FullEvidenceUnlinked {
            unlinked_at_ms: proof.unlinked_at_ms(),
        };
        candidate
            .entries
            .remove(&evidence_id)
            .ok_or("schedule evidence: tombstone target disappeared")?;
        candidate.retired_evidence_ids.insert(evidence_id);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn begin_bundle_gc(&mut self, action: BundleGcActionV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        action.validate()?;
        if action.lifecycle != BundleGcLifecycleV1::Pending
            || candidate.bundle_gc_actions.contains_key(&action.action_id)
            || candidate.bundle_gc_actions.values().any(|existing| {
                existing.bundle_id == action.bundle_id
                    && existing.lifecycle == BundleGcLifecycleV1::Pending
            })
        {
            return Err("schedule evidence: bundle GC intent is duplicate or not pending".into());
        }
        if candidate.bundle_gc_actions.len() >= MAX_EVIDENCE_ITEMS * 4 {
            let oldest_terminal = candidate
                .bundle_gc_actions
                .values()
                .filter(|existing| existing.lifecycle != BundleGcLifecycleV1::Pending)
                .min_by(|left, right| {
                    left.started_at_ms
                        .cmp(&right.started_at_ms)
                        .then_with(|| left.action_id.cmp(&right.action_id))
                })
                .map(|existing| existing.action_id.clone())
                .ok_or("schedule evidence: pending bundle GC actions fill the bounded state")?;
            candidate.bundle_gc_actions.remove(&oldest_terminal);
        }
        candidate
            .bundle_gc_actions
            .insert(action.action_id.clone(), action);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn complete_bundle_gc(
        &mut self,
        action_id: &str,
        unlinked_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let action = candidate
            .bundle_gc_actions
            .get_mut(action_id)
            .ok_or("schedule evidence: bundle GC action does not exist")?;
        if action.lifecycle != BundleGcLifecycleV1::Pending {
            return Err("schedule evidence: bundle GC action is already terminal".into());
        }
        action.lifecycle = BundleGcLifecycleV1::Unlinked { unlinked_at_ms };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn safe_skip_bundle_gc(
        &mut self,
        action_id: &str,
        reason_code: &str,
        skipped_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let action = candidate
            .bundle_gc_actions
            .get_mut(action_id)
            .ok_or("schedule evidence: bundle GC action does not exist")?;
        if action.lifecycle != BundleGcLifecycleV1::Pending {
            return Err("schedule evidence: bundle GC action is already terminal".into());
        }
        action.lifecycle = BundleGcLifecycleV1::SafeSkipped {
            skipped_at_ms,
            reason_code: reason_code.into(),
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn begin_image_gc(&mut self, action: ImageGcActionV1) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        action.validate()?;
        if action.lifecycle != ImageGcLifecycleV1::Pending
            || candidate.image_gc_actions.contains_key(&action.action_id)
            || candidate.image_gc_actions.values().any(|existing| {
                existing.digest == action.digest
                    && existing.lifecycle == ImageGcLifecycleV1::Pending
            })
        {
            return Err("schedule evidence: image GC intent is duplicate or not pending".into());
        }
        if candidate.image_gc_actions.len() >= MAX_EVIDENCE_ITEMS * 4 {
            let oldest_terminal = candidate
                .image_gc_actions
                .values()
                .filter(|existing| existing.lifecycle != ImageGcLifecycleV1::Pending)
                .min_by(|left, right| {
                    left.started_at_ms
                        .cmp(&right.started_at_ms)
                        .then_with(|| left.action_id.cmp(&right.action_id))
                })
                .map(|existing| existing.action_id.clone())
                .ok_or("schedule evidence: pending image GC actions fill the bounded state")?;
            candidate.image_gc_actions.remove(&oldest_terminal);
        }
        candidate
            .image_gc_actions
            .insert(action.action_id.clone(), action);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn complete_image_gc(
        &mut self,
        action_id: &str,
        removed_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let action = candidate
            .image_gc_actions
            .get_mut(action_id)
            .ok_or("schedule evidence: image GC action does not exist")?;
        if action.lifecycle != ImageGcLifecycleV1::Pending {
            return Err("schedule evidence: image GC action is already terminal".into());
        }
        action.lifecycle = ImageGcLifecycleV1::Removed { removed_at_ms };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn safe_skip_image_gc(
        &mut self,
        action_id: &str,
        reason_code: &str,
        skipped_at_ms: i64,
    ) -> Result<(), BoxError> {
        let mut candidate = self.clone();
        let action = candidate
            .image_gc_actions
            .get_mut(action_id)
            .ok_or("schedule evidence: image GC action does not exist")?;
        if action.lifecycle != ImageGcLifecycleV1::Pending {
            return Err("schedule evidence: image GC action is already terminal".into());
        }
        action.lifecycle = ImageGcLifecycleV1::SafeSkipped {
            skipped_at_ms,
            reason_code: reason_code.into(),
        };
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn pin_transition_allowed(previous: &EvidencePinV1, next: &EvidencePinV1) -> bool {
    previous.pin_id == next.pin_id
        && previous.evidence_id == next.evidence_id
        && previous.reason == next.reason
        && previous.created_at_ms == next.created_at_ms
        && (previous.lifecycle == next.lifecycle
            || matches!(
                (&previous.lifecycle, &next.lifecycle),
                (PinLifecycleV1::Active, PinLifecycleV1::Released { .. })
            ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn tombstone_transition_allowed(
    previous: &EvidenceTombstoneV1,
    next: &EvidenceTombstoneV1,
) -> bool {
    previous.tombstone_id == next.tombstone_id
        && previous.evidence_id == next.evidence_id
        && previous.evidence_class == next.evidence_class
        && previous.full_evidence_sha256 == next.full_evidence_sha256
        && previous.manifest_sha256 == next.manifest_sha256
        && previous.compact_record_sha256 == next.compact_record_sha256
        && previous.archive_bytes == next.archive_bytes
        && previous.manifest_bytes == next.manifest_bytes
        && previous.compact_record_bytes == next.compact_record_bytes
        && previous.compact_record == next.compact_record
        && previous.hot_path == next.hot_path
        && previous.cold_path == next.cold_path
        && previous.hot_was_present == next.hot_was_present
        && previous.terminal_at_ms == next.terminal_at_ms
        && previous.full_retain_until_ms == next.full_retain_until_ms
        && previous.compact_retain_until_ms == next.compact_retain_until_ms
        && previous.reason_code == next.reason_code
        && previous.created_at_ms == next.created_at_ms
        && (previous.lifecycle == next.lifecycle
            || matches!(
                (&previous.lifecycle, &next.lifecycle),
                (
                    TombstoneLifecycleV1::Pending,
                    TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
                )
            ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn cold_storage_transition_allowed(
    previous: &ColdStorageBindingV1,
    next: &ColdStorageBindingV1,
) -> bool {
    previous == next
        || matches!(
            (previous, next),
            (
                ColdStorageBindingV1::Absent,
                ColdStorageBindingV1::OwnerIcloud { .. }
            )
        )
        || matches!(
            (previous, next),
            (
                ColdStorageBindingV1::OwnerIcloud {
                    root_sha256: previous_root,
                    file_provider_domain_id: previous_domain,
                    ..
                },
                ColdStorageBindingV1::OwnerIcloud {
                    root_sha256: next_root,
                    file_provider_domain_id: next_domain,
                    ..
                }
            ) if previous_root == next_root && previous_domain == next_domain
        )
}

#[cfg_attr(not(test), allow(dead_code))]
fn cold_copy_transition_allowed(previous: &ColdCopyRecordV1, next: &ColdCopyRecordV1) -> bool {
    let immutable = previous.copy_id == next.copy_id
        && previous.evidence_id == next.evidence_id
        && previous.archive_sha256 == next.archive_sha256
        && previous.archive_bytes == next.archive_bytes
        && previous.manifest_sha256 == next.manifest_sha256
        && previous.manifest_bytes == next.manifest_bytes
        && previous.consent_id == next.consent_id
        && previous.consent_sha256 == next.consent_sha256
        && previous.consent_revocation_generation == next.consent_revocation_generation
        && previous.cold_root_sha256 == next.cold_root_sha256
        && previous.file_provider_domain_id == next.file_provider_domain_id
        && previous.archive_path == next.archive_path
        && previous.manifest_path == next.manifest_path
        && previous.deadline_ms == next.deadline_ms
        && previous.admitted_at_ms == next.admitted_at_ms;
    if !immutable {
        return false;
    }
    match (&previous.lifecycle, &next.lifecycle) {
        (left, right) if left == right => true,
        (ColdCopyLifecycleV1::Admitted, ColdCopyLifecycleV1::Published { .. }) => true,
        (ColdCopyLifecycleV1::Admitted, ColdCopyLifecycleV1::Abandoned { .. }) => true,
        (
            ColdCopyLifecycleV1::Published {
                published_at_ms: previous_published,
                archive_observation: previous_archive,
                manifest_observation: previous_manifest,
                last_content_verified_at_ms: previous_verified,
            },
            ColdCopyLifecycleV1::Published {
                published_at_ms: next_published,
                archive_observation: next_archive,
                manifest_observation: next_manifest,
                last_content_verified_at_ms: next_verified,
            },
        ) => {
            previous_published == next_published
                && next_archive.observed_at_ms >= previous_archive.observed_at_ms
                && next_manifest.observed_at_ms >= previous_manifest.observed_at_ms
                && next_verified >= previous_verified
        }
        _ => false,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn bundle_gc_transition_allowed(previous: &BundleGcActionV1, next: &BundleGcActionV1) -> bool {
    previous.action_id == next.action_id
        && previous.bundle_id == next.bundle_id
        && previous.evidence_id == next.evidence_id
        && previous.provider_id == next.provider_id
        && previous.case_id == next.case_id
        && previous.evidence_class == next.evidence_class
        && previous.cache_root_sha256 == next.cache_root_sha256
        && previous.path == next.path
        && previous.content_sha256 == next.content_sha256
        && previous.length_bytes == next.length_bytes
        && previous.preserved_in_full_evidence_sha256 == next.preserved_in_full_evidence_sha256
        && previous.reason_code == next.reason_code
        && previous.planned_at_ms == next.planned_at_ms
        && previous.started_at_ms == next.started_at_ms
        && (previous.lifecycle == next.lifecycle
            || matches!(
                (&previous.lifecycle, &next.lifecycle),
                (
                    BundleGcLifecycleV1::Pending,
                    BundleGcLifecycleV1::Unlinked { .. } | BundleGcLifecycleV1::SafeSkipped { .. }
                )
            ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn image_gc_transition_allowed(previous: &ImageGcActionV1, next: &ImageGcActionV1) -> bool {
    previous.action_id == next.action_id
        && previous.digest == next.digest
        && previous.planned_inventory_sha256 == next.planned_inventory_sha256
        && previous.planned_at_ms == next.planned_at_ms
        && previous.started_at_ms == next.started_at_ms
        && (previous.lifecycle == next.lifecycle
            || matches!(
                (&previous.lifecycle, &next.lifecycle),
                (
                    ImageGcLifecycleV1::Pending,
                    ImageGcLifecycleV1::Removed { .. } | ImageGcLifecycleV1::SafeSkipped { .. }
                )
            ))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceStateSnapshotV1 {
    pub(super) schema_version: u16,
    pub(super) generation: u64,
    pub(super) previous_record: OptionalSha256V1,
    pub(super) recorded_at_ms: i64,
    pub(super) state: EvidenceStateModelV1,
}

impl EvidenceStateSnapshotV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn first(
        state: EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<Self, BoxError> {
        let value = Self {
            schema_version: 1,
            generation: 1,
            previous_record: OptionalSha256V1::Absent,
            recorded_at_ms,
            state,
        };
        value.validate()?;
        Ok(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn successor(
        &self,
        state: EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<Self, BoxError> {
        let value = Self {
            schema_version: 1,
            generation: self
                .generation
                .checked_add(1)
                .ok_or("schedule evidence: generation overflow")?,
            previous_record: OptionalSha256V1::Sha256 {
                value: evidence_state_snapshot_sha256(self)?,
            },
            recorded_at_ms,
            state,
        };
        value.validate()?;
        Ok(value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn project_index(&self) -> Result<EvidenceIndexV1, BoxError> {
        self.validate()?;
        self.state.project_index_at(self.generation)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generation == 0 || self.recorded_at_ms <= 0 {
            return Err("schedule evidence: snapshot header is invalid".into());
        }
        match (&self.previous_record, self.generation) {
            (OptionalSha256V1::Absent, 1) => {}
            (OptionalSha256V1::Sha256 { value }, generation) if generation > 1 => {
                require_sha256("previous snapshot", value)?;
            }
            _ => return Err("schedule evidence: snapshot predecessor shape is invalid".into()),
        }
        self.state.validate()?;
        if self
            .state
            .entries
            .values()
            .any(|entry| entry.terminal_at_ms > self.recorded_at_ms)
            || self.state.pins.values().any(|pin| {
                pin.created_at_ms > self.recorded_at_ms
                    || matches!(
                        &pin.lifecycle,
                        PinLifecycleV1::Released { released_at_ms, .. }
                            if *released_at_ms > self.recorded_at_ms
                    )
            })
            || self.state.tombstones.values().any(|value| {
                value.created_at_ms > self.recorded_at_ms
                    || matches!(
                        &value.lifecycle,
                            TombstoneLifecycleV1::FullEvidenceUnlinked { unlinked_at_ms }
                            if *unlinked_at_ms > self.recorded_at_ms
                    )
            })
            || self.state.cold_copies.values().any(|value| {
                value.admitted_at_ms > self.recorded_at_ms
                    || matches!(
                        &value.lifecycle,
                        ColdCopyLifecycleV1::Abandoned {
                            abandoned_at_ms,
                            ..
                        } if *abandoned_at_ms > self.recorded_at_ms
                    )
                    || matches!(
                        &value.lifecycle,
                        ColdCopyLifecycleV1::Published {
                            published_at_ms,
                            archive_observation,
                            manifest_observation,
                            last_content_verified_at_ms,
                        } if *published_at_ms > self.recorded_at_ms
                            || archive_observation.observed_at_ms > self.recorded_at_ms
                            || manifest_observation.observed_at_ms > self.recorded_at_ms
                            || *last_content_verified_at_ms > self.recorded_at_ms
                    )
            })
            || self
                .state
                .storage_integrity_holds
                .values()
                .any(|value| value.detected_at_ms > self.recorded_at_ms)
            || self.state.bundle_gc_actions.values().any(|value| {
                value.started_at_ms > self.recorded_at_ms
                    || matches!(
                        value.lifecycle,
                        BundleGcLifecycleV1::Unlinked { unlinked_at_ms }
                            if unlinked_at_ms > self.recorded_at_ms
                    )
                    || matches!(
                        value.lifecycle,
                        BundleGcLifecycleV1::SafeSkipped { skipped_at_ms, .. }
                            if skipped_at_ms > self.recorded_at_ms
                    )
            })
            || self.state.image_gc_actions.values().any(|value| {
                value.started_at_ms > self.recorded_at_ms
                    || matches!(
                        value.lifecycle,
                        ImageGcLifecycleV1::Removed { removed_at_ms }
                            if removed_at_ms > self.recorded_at_ms
                    )
                    || matches!(
                        value.lifecycle,
                        ImageGcLifecycleV1::SafeSkipped { skipped_at_ms, .. }
                            if skipped_at_ms > self.recorded_at_ms
                    )
            })
        {
            return Err("schedule evidence: state event postdates its snapshot".into());
        }
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn evidence_state_snapshot_bytes(value: &EvidenceStateSnapshotV1) -> Result<Vec<u8>, BoxError> {
    value.validate()?;
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn evidence_state_snapshot_sha256(
    value: &EvidenceStateSnapshotV1,
) -> Result<String, BoxError> {
    Ok(local_file::sha256_hex(&evidence_state_snapshot_bytes(
        value,
    )?))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn validate_evidence_state_transition(
    previous: &EvidenceStateSnapshotV1,
    next: &EvidenceStateSnapshotV1,
) -> Result<(), BoxError> {
    previous.validate()?;
    next.validate()?;
    if next.generation != previous.generation.saturating_add(1)
        || next.recorded_at_ms <= previous.recorded_at_ms
        || next.previous_record
            != (OptionalSha256V1::Sha256 {
                value: evidence_state_snapshot_sha256(previous)?,
            })
        || next.state.hot_root_sha256 != previous.state.hot_root_sha256
        || !cold_storage_transition_allowed(&previous.state.cold_storage, &next.state.cold_storage)
    {
        return Err("schedule evidence: snapshot chain/root transition is invalid".into());
    }
    if !previous
        .state
        .retired_evidence_ids
        .is_subset(&next.state.retired_evidence_ids)
    {
        return Err("schedule evidence: retired evidence history was removed".into());
    }
    if next.state.cold_storage != previous.state.cold_storage {
        let ColdStorageBindingV1::OwnerIcloud {
            consent_id,
            consent_sha256,
            root_sha256,
            file_provider_domain_id,
        } = &next.state.cold_storage
        else {
            return Err("schedule evidence: cold storage binding was removed".into());
        };
        if !next.state.cold_copies.values().any(|copy| {
            !previous.state.cold_copies.contains_key(&copy.copy_id)
                && copy.consent_id == *consent_id
                && copy.consent_sha256 == *consent_sha256
                && copy.cold_root_sha256 == *root_sha256
                && copy.file_provider_domain_id == *file_provider_domain_id
        }) {
            return Err("schedule evidence: cold binding changed without a new admission".into());
        }
    }
    for (id, prior) in &previous.state.pins {
        let current = next
            .state
            .pins
            .get(id)
            .ok_or("schedule evidence: pin history was removed")?;
        if !pin_transition_allowed(prior, current) {
            return Err("schedule evidence: pin changed nonmonotonically".into());
        }
        if matches!(
            (&prior.lifecycle, &current.lifecycle),
            (
                PinLifecycleV1::Active,
                PinLifecycleV1::Released { released_at_ms, .. }
            ) if *released_at_ms <= previous.recorded_at_ms
        ) {
            return Err("schedule evidence: pin release was backdated".into());
        }
    }
    for (id, current) in &next.state.pins {
        if !previous.state.pins.contains_key(id)
            && (current.created_at_ms <= previous.recorded_at_ms
                || current.lifecycle != PinLifecycleV1::Active)
        {
            return Err("schedule evidence: new pin is backdated or skips active state".into());
        }
    }
    for (id, prior) in &previous.state.tombstones {
        let current = next
            .state
            .tombstones
            .get(id)
            .ok_or("schedule evidence: tombstone history was removed")?;
        if !tombstone_transition_allowed(prior, current) {
            return Err("schedule evidence: tombstone changed nonmonotonically".into());
        }
        if matches!(
            (&prior.lifecycle, &current.lifecycle),
            (
                TombstoneLifecycleV1::Pending,
                TombstoneLifecycleV1::FullEvidenceUnlinked { unlinked_at_ms }
            ) if *unlinked_at_ms <= previous.recorded_at_ms
        ) {
            return Err("schedule evidence: tombstone completion was backdated".into());
        }
    }
    for (id, current) in &next.state.tombstones {
        if !previous.state.tombstones.contains_key(id)
            && (current.created_at_ms <= previous.recorded_at_ms
                || current.lifecycle != TombstoneLifecycleV1::Pending)
        {
            return Err(
                "schedule evidence: new tombstone is backdated or skips pending state".into(),
            );
        }
    }
    for (id, prior) in &previous.state.cold_copies {
        let current = next
            .state
            .cold_copies
            .get(id)
            .ok_or("schedule evidence: cold-copy history was removed")?;
        if !cold_copy_transition_allowed(prior, current) {
            return Err("schedule evidence: cold-copy state changed nonmonotonically".into());
        }
        match (&prior.lifecycle, &current.lifecycle) {
            (
                ColdCopyLifecycleV1::Admitted,
                ColdCopyLifecycleV1::Abandoned {
                    abandoned_at_ms, ..
                },
            ) if *abandoned_at_ms <= previous.recorded_at_ms => {
                return Err("schedule evidence: cold-copy abandonment was backdated".into())
            }
            (
                ColdCopyLifecycleV1::Admitted,
                ColdCopyLifecycleV1::Published {
                    published_at_ms,
                    archive_observation,
                    manifest_observation,
                    ..
                },
            ) if *published_at_ms <= previous.recorded_at_ms
                || archive_observation.observed_at_ms <= previous.recorded_at_ms
                || manifest_observation.observed_at_ms <= previous.recorded_at_ms =>
            {
                return Err("schedule evidence: cold publication was backdated".into())
            }
            (
                ColdCopyLifecycleV1::Published {
                    archive_observation: previous_archive,
                    manifest_observation: previous_manifest,
                    last_content_verified_at_ms: previous_verified,
                    ..
                },
                ColdCopyLifecycleV1::Published {
                    archive_observation: current_archive,
                    manifest_observation: current_manifest,
                    last_content_verified_at_ms: current_verified,
                    ..
                },
            ) if (current_archive != previous_archive
                && current_archive.observed_at_ms <= previous.recorded_at_ms)
                || (current_manifest != previous_manifest
                    && current_manifest.observed_at_ms <= previous.recorded_at_ms)
                || (current_verified != previous_verified
                    && *current_verified <= previous.recorded_at_ms) =>
            {
                return Err("schedule evidence: cold verification was backdated".into())
            }
            _ => {}
        }
    }
    for (id, current) in &next.state.cold_copies {
        if !previous.state.cold_copies.contains_key(id)
            && (current.admitted_at_ms <= previous.recorded_at_ms
                || current.lifecycle != ColdCopyLifecycleV1::Admitted)
        {
            return Err("schedule evidence: cold-copy admission was backdated".into());
        }
    }
    for (id, prior) in &previous.state.storage_integrity_holds {
        if next.state.storage_integrity_holds.get(id) != Some(prior) {
            return Err("schedule evidence: storage-integrity hold history changed".into());
        }
    }
    for (id, current) in &next.state.storage_integrity_holds {
        if !previous.state.storage_integrity_holds.contains_key(id)
            && current.detected_at_ms <= previous.recorded_at_ms
        {
            return Err("schedule evidence: storage-integrity hold was backdated".into());
        }
    }
    for (id, prior) in &previous.state.bundle_gc_actions {
        let Some(current) = next.state.bundle_gc_actions.get(id) else {
            if prior.lifecycle == BundleGcLifecycleV1::Pending {
                return Err("schedule evidence: pending bundle GC history was removed".into());
            }
            continue;
        };
        if !bundle_gc_transition_allowed(prior, current) {
            return Err("schedule evidence: bundle GC action changed nonmonotonically".into());
        }
        let transition_at_ms = match (&prior.lifecycle, &current.lifecycle) {
            (BundleGcLifecycleV1::Pending, BundleGcLifecycleV1::Unlinked { unlinked_at_ms }) => {
                Some(*unlinked_at_ms)
            }
            (
                BundleGcLifecycleV1::Pending,
                BundleGcLifecycleV1::SafeSkipped { skipped_at_ms, .. },
            ) => Some(*skipped_at_ms),
            _ => None,
        };
        if transition_at_ms.is_some_and(|value| value <= previous.recorded_at_ms) {
            return Err("schedule evidence: bundle GC completion was backdated".into());
        }
    }
    for (id, current) in &next.state.bundle_gc_actions {
        if !previous.state.bundle_gc_actions.contains_key(id)
            && (current.started_at_ms <= previous.recorded_at_ms
                || current.lifecycle != BundleGcLifecycleV1::Pending)
        {
            return Err("schedule evidence: bundle GC intent was backdated or terminal".into());
        }
    }
    for (id, prior) in &previous.state.image_gc_actions {
        let Some(current) = next.state.image_gc_actions.get(id) else {
            if prior.lifecycle == ImageGcLifecycleV1::Pending {
                return Err("schedule evidence: pending image GC history was removed".into());
            }
            continue;
        };
        if !image_gc_transition_allowed(prior, current) {
            return Err("schedule evidence: image GC action changed nonmonotonically".into());
        }
        let transition_at_ms = match (&prior.lifecycle, &current.lifecycle) {
            (ImageGcLifecycleV1::Pending, ImageGcLifecycleV1::Removed { removed_at_ms }) => {
                Some(*removed_at_ms)
            }
            (
                ImageGcLifecycleV1::Pending,
                ImageGcLifecycleV1::SafeSkipped { skipped_at_ms, .. },
            ) => Some(*skipped_at_ms),
            _ => None,
        };
        if transition_at_ms.is_some_and(|value| value <= previous.recorded_at_ms) {
            return Err("schedule evidence: image GC terminal state was backdated".into());
        }
    }
    for (id, current) in &next.state.image_gc_actions {
        if !previous.state.image_gc_actions.contains_key(id)
            && (current.started_at_ms <= previous.recorded_at_ms
                || current.lifecycle != ImageGcLifecycleV1::Pending)
        {
            return Err("schedule evidence: image GC intent was backdated or terminal".into());
        }
    }
    for (id, prior) in &previous.state.entries {
        if let Some(current) = next.state.entries.get(id) {
            if !prior.immutable_eq(current)
                || current.full_retain_until_ms < prior.full_retain_until_ms
                || current.compact_retain_until_ms < prior.compact_retain_until_ms
                || current.hot_retain_until_ms < prior.hot_retain_until_ms
                || (prior.cold_path != current.cold_path
                    && !matches!(
                        (&prior.cold_path, &current.cold_path),
                        (
                            OptionalRelativeEvidencePathV1::Absent,
                            OptionalRelativeEvidencePathV1::RelativePath { .. }
                        )
                    ))
                || (!prior.hot_present && current.hot_present)
            {
                return Err("schedule evidence: indexed evidence changed nonmonotonically".into());
            }
        } else {
            let completed = next.state.tombstones.values().any(|tombstone| {
                tombstone.evidence_id == *id
                    && tombstone.full_evidence_sha256 == prior.full_evidence_sha256
                    && matches!(
                        tombstone.lifecycle,
                        TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
                    )
            });
            if !completed || !next.state.retired_evidence_ids.contains(id) {
                return Err(
                    "schedule evidence: entry disappeared without a completed tombstone".into(),
                );
            }
        }
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct FileEvidenceJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    state_quota: StateQuota,
    next_generation: u64,
    previous_snapshot: EvidenceStateSnapshotV1,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceJournalOpen<'lock> {
    pub(super) journal: FileEvidenceJournal<'lock>,
    pub(super) snapshot: EvidenceStateSnapshotV1,
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) snapshot_sha256: String,
}

impl<'lock> FileEvidenceJournal<'lock> {
    #[cfg_attr(not(test), allow(dead_code))]
    fn generation_name(generation: u64) -> String {
        format!("{STATE_PREFIX}{generation:020}.json")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn generation_entries(
        directory: &local_file::PinnedDirectory,
    ) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule evidence: retained index directory path changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(STATE_PREFIX) {
                continue;
            }
            let Some(raw) = name
                .strip_prefix(STATE_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
            else {
                return Err("schedule evidence: malformed state generation name".into());
            };
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule evidence: malformed state generation number".into());
            }
            entries.push((raw.parse()?, name.into()));
        }
        if entries.len() > MAX_STATE_GENERATIONS || !directory.current_path_matches() {
            return Err("schedule evidence: state generation scan is unbounded or unstable".into());
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn read_generation(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<(EvidenceStateSnapshotV1, String), BoxError> {
        use std::os::unix::fs::MetadataExt as _;

        let file = directory.open_regular_file(OsStr::new(name), "evidence state generation")?;
        let metadata = file.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != STATE_FILE_MODE
            || metadata.len() > MAX_STATE_RECORD_BYTES
        {
            return Err(
                "schedule evidence: state generation is not a bounded owner-only mode-0600 file"
                    .into(),
            );
        }
        let snapshot = local_file::read_open_regular_file_bounded(
            &file,
            "evidence state generation",
            MAX_STATE_RECORD_BYTES,
        )?;
        let value: EvidenceStateSnapshotV1 = serde_json::from_slice(&snapshot.bytes)
            .map_err(|error| format!("schedule evidence: invalid state generation: {error}"))?;
        let mut canonical = serde_json::to_vec(&value)?;
        canonical.push(b'\n');
        if canonical != snapshot.bytes {
            return Err("schedule evidence: state generation is not canonical JSON".into());
        }
        value.validate()?;
        Ok((value, snapshot.sha256))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn initialize<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
        state: &EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<EvidenceJournalOpen<'lock>, BoxError> {
        let directory = capability.evidence_index_directory();
        if !Self::generation_entries(directory)?.is_empty() {
            return Err("schedule evidence: state journal already exists".into());
        }
        let first = EvidenceStateSnapshotV1::first(state.clone(), recorded_at_ms)?;
        let mut journal = Self {
            directory,
            state_quota: capability.state_quota(),
            next_generation: 1,
            previous_snapshot: first.clone(),
        };
        let (snapshot, snapshot_sha256) = journal.append_initial(first)?;
        Ok(EvidenceJournalOpen {
            journal,
            snapshot,
            snapshot_sha256,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn append_initial(
        &mut self,
        snapshot: EvidenceStateSnapshotV1,
    ) -> Result<(EvidenceStateSnapshotV1, String), BoxError> {
        self.persist(&snapshot)?;
        let sha256 = evidence_state_snapshot_sha256(&snapshot)?;
        self.next_generation = 2;
        self.previous_snapshot = snapshot.clone();
        Ok((snapshot, sha256))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn open_existing<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<EvidenceJournalOpen<'lock>, BoxError> {
        let directory = capability.evidence_index_directory();
        let entries = Self::generation_entries(directory)?;
        if entries.is_empty() {
            return Err("schedule evidence: state journal has no generations".into());
        }
        let mut previous: Option<EvidenceStateSnapshotV1> = None;
        let mut latest = None;
        for (index, (generation, name)) in entries.into_iter().enumerate() {
            let expected = u64::try_from(index + 1)?;
            if generation != expected {
                return Err("schedule evidence: state generations are not contiguous".into());
            }
            let (snapshot, sha256) = Self::read_generation(directory, &name)?;
            if snapshot.generation != generation {
                return Err("schedule evidence: filename/record generation mismatch".into());
            }
            if let Some(prior) = &previous {
                validate_evidence_state_transition(prior, &snapshot)?;
            }
            previous = Some(snapshot.clone());
            latest = Some((snapshot, sha256));
        }
        let (snapshot, snapshot_sha256) =
            latest.ok_or("schedule evidence: state journal has no readable generation")?;
        Ok(EvidenceJournalOpen {
            journal: Self {
                directory,
                state_quota: capability.state_quota(),
                next_generation: snapshot
                    .generation
                    .checked_add(1)
                    .ok_or("schedule evidence: generation overflow")?,
                previous_snapshot: snapshot.clone(),
            },
            snapshot,
            snapshot_sha256,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn persist(&self, snapshot: &EvidenceStateSnapshotV1) -> Result<(), BoxError> {
        if usize::try_from(snapshot.generation)? > MAX_STATE_GENERATIONS {
            return Err("schedule evidence: state generation bound reached".into());
        }
        let bytes = evidence_state_snapshot_bytes(snapshot)?;
        if bytes.len() as u64 > MAX_STATE_RECORD_BYTES {
            return Err("schedule evidence: state generation exceeds the byte bound".into());
        }
        self.directory
            .recover_journal_append_residue(STATE_FILE_MODE, "evidence state generation")?;
        self.state_quota.reserve(bytes.len() as u64)?;
        let name = Self::generation_name(snapshot.generation);
        self.directory.write_new_journal_record(
            OsStr::new(&name),
            &bytes,
            STATE_FILE_MODE,
            "evidence state generation",
        )?;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn validate_append_candidate(
        &self,
        state: &EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<EvidenceStateSnapshotV1, BoxError> {
        if self.next_generation != self.previous_snapshot.generation.saturating_add(1) {
            return Err("schedule evidence: in-memory journal generation diverged".into());
        }
        let entries = Self::generation_entries(self.directory)?;
        if entries.len() as u64 != self.previous_snapshot.generation
            || entries.last().map(|(generation, _)| *generation)
                != Some(self.previous_snapshot.generation)
        {
            return Err(
                "schedule evidence: next journal generation is not exclusively available".into(),
            );
        }
        let snapshot = self
            .previous_snapshot
            .successor(state.clone(), recorded_at_ms)?;
        validate_evidence_state_transition(&self.previous_snapshot, &snapshot)?;
        Ok(snapshot)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn append(
        &mut self,
        state: &EvidenceStateModelV1,
        recorded_at_ms: i64,
    ) -> Result<(EvidenceStateSnapshotV1, String), BoxError> {
        let snapshot = self.validate_append_candidate(state, recorded_at_ms)?;
        self.persist(&snapshot)?;
        let sha256 = evidence_state_snapshot_sha256(&snapshot)?;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or("schedule evidence: generation overflow")?;
        self.previous_snapshot = snapshot.clone();
        Ok((snapshot, sha256))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn evidence_status_source_sha256<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
) -> Result<Option<String>, BoxError> {
    if FileEvidenceJournal::generation_entries(capability.evidence_index_directory())?.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        FileEvidenceJournal::open_existing(capability)?.snapshot_sha256,
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn lease_name(evidence_id: &str) -> Result<String, BoxError> {
    stable_id("lease evidence id", evidence_id)?;
    Ok(format!(
        "evidence-lease.{}.lock",
        local_file::sha256_hex(evidence_id.as_bytes())
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn open_or_create_lease_file(
    directory: &local_file::PinnedDirectory,
    evidence_id: &str,
) -> Result<File, BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    let name = lease_name(evidence_id)?;
    let file = match directory.open_regular_file(OsStr::new(&name), "evidence lease") {
        Ok(file) => file,
        Err(_) => {
            match directory.create_new_file(OsStr::new(&name), STATE_FILE_MODE, "evidence lease") {
                Ok(file) => {
                    file.sync_all()?;
                    directory.sync()?;
                    file
                }
                Err(_) => directory.open_regular_file(OsStr::new(&name), "evidence lease")?,
            }
        }
    };
    let metadata = file.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != STATE_FILE_MODE
        || !metadata.is_file()
        || metadata.nlink() != 1
    {
        return Err(
            "schedule evidence: lease is not an owner-only single-link mode-0600 file".into(),
        );
    }
    Ok(file)
}

#[cfg_attr(not(test), allow(dead_code))]
fn acquire_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
    operation: libc::c_int,
) -> Result<File, BoxError> {
    let file = open_or_create_lease_file(capability.evidence_index_directory(), evidence_id)?;
    // SAFETY: the verified single-link regular file descriptor is live. LOCK_NB refuses rather
    // than queueing across scheduler processes.
    if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } == -1 {
        return Err(format!(
            "schedule evidence: lease is busy: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    Ok(file)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn acquire_evidence_read_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
) -> Result<File, BoxError> {
    acquire_lease(capability, evidence_id, libc::LOCK_SH)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn try_acquire_evidence_gc_lease<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
) -> Result<File, BoxError> {
    acquire_lease(capability, evidence_id, libc::LOCK_EX)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn try_acquire_evidence_gc_lease_optional<C: EvidenceStateCapability + ?Sized>(
    capability: &C,
    evidence_id: &str,
) -> Result<Option<File>, BoxError> {
    let file = open_or_create_lease_file(capability.evidence_index_directory(), evidence_id)?;
    // SAFETY: the verified single-link regular file descriptor is live. LOCK_NB refuses rather
    // than queueing across scheduler processes.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK)
            || error.raw_os_error() == Some(libc::EAGAIN)
        {
            return Ok(None);
        }
        return Err(format!("schedule evidence: cannot acquire GC lease: {error}").into());
    }
    Ok(Some(file))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum HotAllocationV1 {
    State,
    Scratch,
    Sealed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct HotStorageCapsV1 {
    total_bytes: u64,
    state_bytes: u64,
    scratch_bytes: u64,
    sealed_bytes: u64,
    #[cfg(test)]
    reduced_for_test: bool,
}

impl HotStorageCapsV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn approved() -> Self {
        Self {
            total_bytes: HOT_TOTAL_CAP_BYTES,
            state_bytes: HOT_STATE_CAP_BYTES,
            scratch_bytes: HOT_SCRATCH_CAP_BYTES,
            sealed_bytes: HOT_SEALED_CAP_BYTES,
            #[cfg(test)]
            reduced_for_test: false,
        }
    }

    #[cfg(test)]
    fn reduced_for_test(
        total_bytes: u64,
        state_bytes: u64,
        scratch_bytes: u64,
        sealed_bytes: u64,
    ) -> Self {
        Self {
            total_bytes,
            state_bytes,
            scratch_bytes,
            sealed_bytes,
            reduced_for_test: true,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        let approved = self.total_bytes == HOT_TOTAL_CAP_BYTES
            && self.state_bytes == HOT_STATE_CAP_BYTES
            && self.scratch_bytes == HOT_SCRATCH_CAP_BYTES
            && self.sealed_bytes == HOT_SEALED_CAP_BYTES;
        #[cfg(not(test))]
        if !approved {
            return Err(
                "schedule evidence: hot allocation caps diverge from approved policy".into(),
            );
        }
        #[cfg(test)]
        if !approved && !self.reduced_for_test {
            return Err(
                "schedule evidence: hot allocation caps diverge from approved policy".into(),
            );
        }
        #[cfg(test)]
        if self.reduced_for_test
            && (self.total_bytes > HOT_TOTAL_CAP_BYTES
                || self.state_bytes > HOT_STATE_CAP_BYTES
                || self.scratch_bytes > HOT_SCRATCH_CAP_BYTES
                || self.sealed_bytes > HOT_SEALED_CAP_BYTES)
        {
            return Err("schedule evidence: test caps may only reduce approved policy".into());
        }
        if self.state_bytes == 0
            || self.scratch_bytes == 0
            || self.sealed_bytes == 0
            || self
                .state_bytes
                .checked_add(self.scratch_bytes)
                .and_then(|value| value.checked_add(self.sealed_bytes))
                != Some(self.total_bytes)
        {
            return Err("schedule evidence: hot allocation caps are invalid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct HotStorageUsageV1 {
    pub(super) state_bytes: u64,
    pub(super) scratch_bytes: u64,
    pub(super) sealed_bytes: u64,
}

impl HotStorageUsageV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn total(self) -> Option<u64> {
        self.state_bytes
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(self.sealed_bytes))
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn reserve_hot_bytes(
    caps: &HotStorageCapsV1,
    usage: &HotStorageUsageV1,
    allocation: HotAllocationV1,
    bytes: u64,
) -> Result<HotStorageUsageV1, BoxError> {
    caps.validate()?;
    if bytes == 0 {
        return Err("schedule evidence: hot reservation must be positive".into());
    }
    if usage.state_bytes > caps.state_bytes
        || usage.scratch_bytes > caps.scratch_bytes
        || usage.sealed_bytes > caps.sealed_bytes
        || usage.total().is_none_or(|total| total > caps.total_bytes)
    {
        return Err("schedule evidence: existing hot storage usage exceeds quota".into());
    }
    let mut next = *usage;
    let (used, cap) = match allocation {
        HotAllocationV1::State => (&mut next.state_bytes, caps.state_bytes),
        HotAllocationV1::Scratch => (&mut next.scratch_bytes, caps.scratch_bytes),
        HotAllocationV1::Sealed => (&mut next.sealed_bytes, caps.sealed_bytes),
    };
    *used = used
        .checked_add(bytes)
        .ok_or("schedule evidence: hot allocation overflow")?;
    if *used > cap || next.total().is_none_or(|total| total > caps.total_bytes) {
        return Err("schedule evidence: hot storage quota pressure".into());
    }
    Ok(next)
}

fn reserve_optional_hot_bytes(
    caps: &HotStorageCapsV1,
    usage: &HotStorageUsageV1,
    allocation: HotAllocationV1,
    bytes: u64,
) -> Result<HotStorageUsageV1, BoxError> {
    if bytes > 0 {
        return reserve_hot_bytes(caps, usage, allocation, bytes);
    }
    caps.validate()?;
    if usage.state_bytes > caps.state_bytes
        || usage.scratch_bytes > caps.scratch_bytes
        || usage.sealed_bytes > caps.sealed_bytes
        || usage.total().is_none_or(|total| total > caps.total_bytes)
    {
        return Err("schedule evidence: existing hot storage usage exceeds quota".into());
    }
    Ok(*usage)
}

#[cfg_attr(not(test), allow(dead_code))]
fn reserve_state_journal_bytes(current_bytes: u64, new_bytes: u64) -> Result<u64, BoxError> {
    if new_bytes == 0 {
        return Err("schedule evidence: state journal reservation must be positive".into());
    }
    let next = current_bytes
        .checked_add(new_bytes)
        .ok_or("schedule evidence: state journal byte accounting overflow")?;
    if next > HOT_STATE_CAP_BYTES {
        return Err("schedule evidence: state journal exceeds the hot state quota".into());
    }
    Ok(next)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn plan_hot_evictions(
    state: &EvidenceStateModelV1,
    now_ms: i64,
    bytes_needed: u64,
) -> Result<Vec<String>, BoxError> {
    state.validate()?;
    if now_ms <= 0 || bytes_needed == 0 {
        return Err("schedule evidence: eviction request is invalid".into());
    }
    let mut candidates = state
        .entries
        .values()
        .filter(|entry| {
            entry.hot_present
                && entry.hot_retain_until_ms <= now_ms
                && !state.has_active_pin(&entry.evidence_id)
                && matches!(
                    entry.cold_path,
                    OptionalRelativeEvidencePathV1::RelativePath { .. }
                )
                && !state.tombstones.values().any(|tombstone| {
                    tombstone.evidence_id == entry.evidence_id
                        && tombstone.lifecycle == TombstoneLifecycleV1::Pending
                })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.terminal_at_ms
            .cmp(&right.terminal_at_ms)
            .then_with(|| left.evidence_id.cmp(&right.evidence_id))
    });
    let mut reclaimed = 0_u64;
    let mut selected = Vec::new();
    for entry in candidates {
        reclaimed = reclaimed
            .checked_add(entry.sealed_hot_bytes()?)
            .ok_or("schedule evidence: eviction byte total overflow")?;
        selected.push(entry.evidence_id.clone());
        if reclaimed >= bytes_needed {
            return Ok(selected);
        }
    }
    Err("schedule evidence: protected evidence prevents quota recovery".into())
}

#[cfg_attr(not(test), allow(dead_code))]
const SCHEDULE_SIDECAR_NAME: &str = "schedule-sidecar.json";
#[cfg_attr(not(test), allow(dead_code))]
const MAX_SEAL_ENTRIES: usize = 4_096;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_SEAL_FILE_BYTES: u64 = 64 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_SEAL_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_SEAL_PATH_BYTES: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct SealLimitsV1 {
    pub(super) max_entries: usize,
    pub(super) max_file_bytes: u64,
    pub(super) max_total_bytes: u64,
}

impl SealLimitsV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn approved() -> Self {
        Self {
            max_entries: MAX_SEAL_ENTRIES,
            max_file_bytes: MAX_SEAL_FILE_BYTES,
            max_total_bytes: MAX_SEAL_TOTAL_BYTES,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(self) -> Result<(), BoxError> {
        if self.max_entries == 0
            || self.max_entries > MAX_SEAL_ENTRIES
            || self.max_file_bytes == 0
            || self.max_file_bytes > MAX_SEAL_FILE_BYTES
            || self.max_total_bytes == 0
            || self.max_total_bytes > MAX_SEAL_TOTAL_BYTES
            || self.max_file_bytes > self.max_total_bytes
        {
            return Err("schedule evidence: seal limits exceed the approved bounds".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct SealEvidenceRequestV1 {
    pub(super) evidence_class: EvidenceClassV1,
    pub(super) terminal_at_ms: i64,
    pub(super) case_minimum_days: u32,
    pub(super) release_retain_until_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct SealedEvidenceFileV1 {
    path: String,
    length_bytes: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct SealedEvidenceManifestV1 {
    schema_version: u16,
    evidence_id: String,
    created_at_ms: i64,
    terminal_at_ms: i64,
    source_tree_sha256: String,
    directories: Vec<String>,
    files: Vec<SealedEvidenceFileV1>,
    sidecar_path: String,
    sidecar_sha256: String,
    aggregate_sha256: OptionalSha256V1,
    archive_sha256: String,
    archive_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct CompactEvidenceRecordV1 {
    schema_version: u16,
    evidence_id: String,
    evidence_class: EvidenceClassV1,
    terminal_at_ms: i64,
    affected_case_ids: Vec<String>,
    sidecar_sha256: String,
    aggregate_sha256: OptionalSha256V1,
    archive_sha256: String,
    manifest_sha256: String,
}

impl CompactEvidenceRecordV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1
            || self.terminal_at_ms <= 0
            || self.affected_case_ids.is_empty()
            || self.affected_case_ids.len() > MAX_EVIDENCE_ITEMS
        {
            return Err("schedule evidence: compact record shape is invalid".into());
        }
        stable_id("compact evidence id", &self.evidence_id)?;
        let mut case_ids = BTreeSet::new();
        for case_id in &self.affected_case_ids {
            stable_id("compact affected case id", case_id)?;
            if !case_ids.insert(case_id) {
                return Err("schedule evidence: compact affected case ids are not unique".into());
            }
        }
        require_sha256("compact sidecar", &self.sidecar_sha256)?;
        if let OptionalSha256V1::Sha256 { value } = &self.aggregate_sha256 {
            require_sha256("compact aggregate", value)?;
        }
        require_sha256("compact archive", &self.archive_sha256)?;
        require_sha256("compact manifest", &self.manifest_sha256)?;
        if json_value_contains_secret(&serde_json::to_value(self)?) {
            return Err("schedule evidence: compact record contains secret-shaped material".into());
        }
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_compact_record_material(
    raw: &str,
    expected_evidence_id: &str,
    expected_evidence_class: EvidenceClassV1,
    expected_terminal_at_ms: i64,
    expected_archive_sha256: &str,
    expected_manifest_sha256: &str,
) -> Result<(), BoxError> {
    let record: CompactEvidenceRecordV1 = serde_json::from_str(raw)?;
    record.validate()?;
    if canonical_json(&record)? != raw.as_bytes()
        || record.evidence_id != expected_evidence_id
        || record.evidence_class != expected_evidence_class
        || record.terminal_at_ms != expected_terminal_at_ms
        || record.archive_sha256 != expected_archive_sha256
        || record.manifest_sha256 != expected_manifest_sha256
    {
        return Err("schedule evidence: compact record identity binding is invalid".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct PreparedSealedEvidenceV1 {
    evidence_id: String,
    evidence_class: EvidenceClassV1,
    terminal_at_ms: i64,
    case_minimum_days: u32,
    release_retain_until_ms: Option<i64>,
    sidecar_sha256: String,
    aggregate_sha256: OptionalSha256V1,
    archive: Vec<u8>,
    archive_sha256: String,
    manifest: Vec<u8>,
    manifest_sha256: String,
    compact_record: Vec<u8>,
    compact_record_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
struct FilesystemIdentityV1 {
    device: u64,
    inode: u64,
    length: u64,
    uid: u32,
    mode: u32,
    links: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FilesystemIdentityV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt as _;

        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            uid: metadata.uid(),
            mode: metadata.mode(),
            links: metadata.nlink(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn matches(&self, metadata: &std::fs::Metadata) -> bool {
        self == &Self::from_metadata(metadata)
    }
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
struct PlannedSealDirectoryV1 {
    path: Vec<String>,
    directory: local_file::PinnedDirectory,
    identity: FilesystemIdentityV1,
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
struct PlannedSealFileV1 {
    path: Vec<String>,
    parent: local_file::PinnedDirectory,
    name: OsString,
    identity: FilesystemIdentityV1,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
struct SnapshottedSealFileV1 {
    path: Vec<String>,
    bytes: Vec<u8>,
    sha256: String,
}

#[cfg_attr(not(test), allow(dead_code))]
fn evidence_path_string(components: &[String]) -> String {
    components.join("/")
}

#[cfg_attr(not(test), allow(dead_code))]
fn evidence_path_buf(components: &[String]) -> PathBuf {
    components.iter().collect()
}

#[cfg_attr(not(test), allow(dead_code))]
fn admit_portable_source_path(
    portable_paths: &mut BTreeSet<String>,
    components: &[String],
) -> Result<(), BoxError> {
    let relative = RelativeEvidencePathV1 {
        components: components.to_vec(),
    };
    relative_evidence_path("source evidence path", &relative)?;
    if evidence_path_string(components).len() > MAX_SEAL_PATH_BYTES
        || !portable_paths.insert(portable_evidence_path_key(&relative))
    {
        return Err("schedule evidence: source paths collide or exceed the portable bound".into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_private_directory_metadata(
    metadata: &std::fs::Metadata,
    label: &str,
) -> Result<(), BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(format!(
            "schedule evidence: {label} is not an owner-owned mode-0700 directory"
        )
        .into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn validate_private_file_metadata(
    metadata: &std::fs::Metadata,
    label: &str,
) -> Result<(), BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(format!(
            "schedule evidence: {label} is not an owner-owned single-link mode-0600 file"
        )
        .into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn inventory_seal_directory(
    directory: &local_file::PinnedDirectory,
    path: &[String],
    limits: SealLimitsV1,
    portable_paths: &mut BTreeSet<String>,
    directories: &mut Vec<PlannedSealDirectoryV1>,
    files: &mut Vec<PlannedSealFileV1>,
    total_bytes: &mut u64,
) -> Result<(), BoxError> {
    let handle = directory.file_handle();
    let initial_metadata = handle.metadata()?;
    validate_private_directory_metadata(&initial_metadata, "source directory")?;
    let initial_identity = FilesystemIdentityV1::from_metadata(&initial_metadata);
    directories.push(PlannedSealDirectoryV1 {
        path: path.to_vec(),
        directory: directory.clone(),
        identity: initial_identity.clone(),
    });

    let mut entries =
        std::fs::read_dir(directory.acp_session_cwd())?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or("schedule evidence: source entry name is not UTF-8")?
            .to_owned();
        let mut child_path = path.to_vec();
        child_path.push(name.clone());
        admit_portable_source_path(portable_paths, &child_path)?;
        let source_entry_count = directories
            .len()
            .saturating_sub(1)
            .saturating_add(files.len());
        if source_entry_count >= limits.max_entries {
            return Err("schedule evidence: source entry count exceeds the seal limit".into());
        }
        let stable_path = directory.acp_session_cwd().join(&name);
        let metadata = std::fs::symlink_metadata(&stable_path)?;
        if metadata.file_type().is_symlink() {
            return Err("schedule evidence: source symbolic links are forbidden".into());
        }
        if metadata.is_dir() {
            validate_private_directory_metadata(&metadata, "source child directory")?;
            let child = directory.open_child_directory(
                OsStr::new(&name),
                "schedule evidence source child directory",
            )?;
            if !FilesystemIdentityV1::from_metadata(&metadata)
                .matches(&child.file_handle().metadata()?)
            {
                return Err("schedule evidence: source directory changed during inventory".into());
            }
            inventory_seal_directory(
                &child,
                &child_path,
                limits,
                portable_paths,
                directories,
                files,
                total_bytes,
            )?;
        } else if metadata.is_file() {
            validate_private_file_metadata(&metadata, "source file")?;
            if metadata.len() > limits.max_file_bytes {
                return Err("schedule evidence: source file exceeds the seal limit".into());
            }
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or("schedule evidence: source byte count overflow")?;
            if *total_bytes > limits.max_total_bytes {
                return Err("schedule evidence: source bytes exceed the seal limit".into());
            }
            files.push(PlannedSealFileV1 {
                path: child_path,
                parent: directory.clone(),
                name: OsString::from(name),
                identity: FilesystemIdentityV1::from_metadata(&metadata),
            });
        } else {
            return Err("schedule evidence: source contains a special file".into());
        }
    }
    if !initial_identity.matches(&handle.metadata()?) || !directory.current_path_matches() {
        return Err("schedule evidence: source directory changed during inventory".into());
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn json_value_contains_secret(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(value) => crate::compatibility::looks_like_secret(value),
        serde_json::Value::Array(values) => values.iter().any(json_value_contains_secret),
        serde_json::Value::Object(values) => values.iter().any(|(key, value)| {
            crate::compatibility::sensitive_json_key(key)
                || crate::compatibility::looks_like_secret(key)
                || json_value_contains_secret(value)
        }),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn scan_sealed_file(path: &[String], bytes: &[u8]) -> Result<(), BoxError> {
    if crate::compatibility::looks_like_secret(&String::from_utf8_lossy(bytes)) {
        return Err(format!(
            "schedule evidence: source file {} contains secret-shaped raw material",
            evidence_path_string(path)
        )
        .into());
    }
    if path
        .last()
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".json"))
    {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
            format!(
                "schedule evidence: source JSON {} is invalid: {error}",
                evidence_path_string(path)
            )
        })?;
        if json_value_contains_secret(&value) {
            return Err(format!(
                "schedule evidence: source JSON {} contains secret-shaped decoded material",
                evidence_path_string(path)
            )
            .into());
        }
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn snapshot_planned_file(
    planned: &PlannedSealFileV1,
    limits: SealLimitsV1,
) -> Result<SnapshottedSealFileV1, BoxError> {
    let stable_path = planned.parent.acp_session_cwd().join(&planned.name);
    let before_path = std::fs::symlink_metadata(&stable_path)?;
    if !planned.identity.matches(&before_path) {
        return Err("schedule evidence: source file was replaced after inventory".into());
    }
    let file = planned
        .parent
        .open_regular_file(&planned.name, "schedule evidence source file")?;
    if !planned.identity.matches(&file.metadata()?) {
        return Err("schedule evidence: opened source file differs from inventory".into());
    }
    let snapshot = local_file::read_open_regular_file_bounded(
        &file,
        "schedule evidence source file",
        limits.max_file_bytes,
    )?;
    let after_descriptor = file.metadata()?;
    let after_path = std::fs::symlink_metadata(&stable_path)?;
    if !planned.identity.matches(&after_descriptor)
        || !planned.identity.matches(&after_path)
        || snapshot.bytes.len() as u64 != planned.identity.length
    {
        return Err("schedule evidence: source file changed while it was read".into());
    }
    scan_sealed_file(&planned.path, &snapshot.bytes)?;
    Ok(SnapshottedSealFileV1 {
        path: planned.path.clone(),
        bytes: snapshot.bytes,
        sha256: snapshot.sha256,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn append_archive_directory<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    path: &[String],
) -> Result<(), BoxError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o700);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(0);
    header.set_cksum();
    archive.append_data(&mut header, evidence_path_buf(path), std::io::empty())?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn append_archive_file<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    file: &SnapshottedSealFileV1,
) -> Result<(), BoxError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(file.bytes.len() as u64);
    header.set_cksum();
    archive.append_data(
        &mut header,
        evidence_path_buf(&file.path),
        file.bytes.as_slice(),
    )?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn deterministic_archive(
    directories: &[PlannedSealDirectoryV1],
    files: &[SnapshottedSealFileV1],
) -> Result<Vec<u8>, BoxError> {
    let encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for directory in directories.iter().filter(|value| !value.path.is_empty()) {
        append_archive_directory(&mut archive, &directory.path)?;
    }
    for file in files {
        append_archive_file(&mut archive, file)?;
    }
    archive.finish()?;
    let encoder = archive.into_inner()?;
    Ok(encoder.finish()?)
}

#[cfg_attr(not(test), allow(dead_code))]
fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, BoxError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn prepare_sealed_evidence(
    source: &local_file::PinnedDirectory,
    request: &SealEvidenceRequestV1,
) -> Result<PreparedSealedEvidenceV1, BoxError> {
    prepare_sealed_evidence_with_hook(source, request, &SealLimitsV1::approved(), || {})
}

#[cfg_attr(not(test), allow(dead_code))]
fn prepare_sealed_evidence_with_hook<F>(
    source: &local_file::PinnedDirectory,
    request: &SealEvidenceRequestV1,
    limits: &SealLimitsV1,
    after_inventory: F,
) -> Result<PreparedSealedEvidenceV1, BoxError>
where
    F: FnOnce(),
{
    limits.validate()?;
    let mut portable_paths = BTreeSet::new();
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    inventory_seal_directory(
        source,
        &[],
        *limits,
        &mut portable_paths,
        &mut directories,
        &mut files,
        &mut total_bytes,
    )?;
    if files.is_empty() {
        return Err("schedule evidence: completed source contains no files".into());
    }
    directories.sort_by(|left, right| left.path.cmp(&right.path));
    files.sort_by(|left, right| left.path.cmp(&right.path));
    after_inventory();

    let mut snapshots = Vec::with_capacity(files.len());
    for file in &files {
        snapshots.push(snapshot_planned_file(file, *limits)?);
    }
    for directory in &directories {
        if !directory
            .identity
            .matches(&directory.directory.file_handle().metadata()?)
            || !directory.directory.current_path_matches()
        {
            return Err("schedule evidence: source directory changed during sealing".into());
        }
    }

    let sidecars = snapshots
        .iter()
        .filter(|file| {
            file.path
                .last()
                .is_some_and(|name| name == SCHEDULE_SIDECAR_NAME)
        })
        .collect::<Vec<_>>();
    if sidecars.len() != 1 {
        return Err("schedule evidence: source must contain exactly one schedule sidecar".into());
    }
    let sidecar_file = sidecars[0];
    let sidecar: ScheduleEvidenceRecordV1 = parse_schedule_evidence_record(&sidecar_file.bytes)?;
    if sidecar.evidence_index_id != "owner-evidence-index"
        || request.terminal_at_ms < sidecar.created_at_ms
    {
        return Err("schedule evidence: sidecar index or terminal binding is invalid".into());
    }

    let aggregates = snapshots
        .iter()
        .filter(|file| {
            file.path
                .last()
                .is_some_and(|name| name.ends_with("aggregate.json"))
        })
        .collect::<Vec<_>>();
    if aggregates.len() > 1 {
        return Err("schedule evidence: source contains multiple aggregate candidates".into());
    }
    if let Some(aggregate) = aggregates.first() {
        crate::compatibility::validate_child_aggregate_bytes(&aggregate.bytes)?;
    }
    let aggregate_sha256 = match (&sidecar.aggregate, aggregates.as_slice()) {
        (OptionalSha256V1::Absent, []) => OptionalSha256V1::Absent,
        (OptionalSha256V1::Sha256 { value }, [aggregate]) if value == &aggregate.sha256 => {
            OptionalSha256V1::Sha256 {
                value: value.clone(),
            }
        }
        (OptionalSha256V1::Absent, [_]) => {
            return Err("schedule evidence: sidecar omits the included aggregate".into())
        }
        (OptionalSha256V1::Sha256 { .. }, []) => {
            return Err("schedule evidence: sidecar names a missing aggregate".into())
        }
        (OptionalSha256V1::Sha256 { .. }, [_]) => {
            return Err("schedule evidence: aggregate byte hash does not match the sidecar".into())
        }
        _ => return Err("schedule evidence: aggregate cardinality is invalid".into()),
    };

    let archive = deterministic_archive(&directories, &snapshots)?;
    let archive_sha256 = local_file::sha256_hex(&archive);
    let manifest_files = snapshots
        .iter()
        .map(|file| SealedEvidenceFileV1 {
            path: evidence_path_string(&file.path),
            length_bytes: file.bytes.len() as u64,
            sha256: file.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let manifest_directories = directories
        .iter()
        .filter(|directory| !directory.path.is_empty())
        .map(|directory| evidence_path_string(&directory.path))
        .collect::<Vec<_>>();
    let source_tree_sha256 =
        local_file::sha256_hex(&canonical_json(&(&manifest_directories, &manifest_files))?);
    let manifest_value = SealedEvidenceManifestV1 {
        schema_version: 1,
        evidence_id: sidecar.schedule_record_id.clone(),
        created_at_ms: sidecar.created_at_ms,
        terminal_at_ms: request.terminal_at_ms,
        source_tree_sha256,
        directories: manifest_directories,
        files: manifest_files,
        sidecar_path: evidence_path_string(&sidecar_file.path),
        sidecar_sha256: sidecar_file.sha256.clone(),
        aggregate_sha256: aggregate_sha256.clone(),
        archive_sha256: archive_sha256.clone(),
        archive_bytes: archive.len() as u64,
    };
    let manifest = canonical_json(&manifest_value)?;
    let manifest_sha256 = local_file::sha256_hex(&manifest);
    let compact_value = CompactEvidenceRecordV1 {
        schema_version: 1,
        evidence_id: sidecar.schedule_record_id.clone(),
        evidence_class: request.evidence_class,
        terminal_at_ms: request.terminal_at_ms,
        affected_case_ids: sidecar.affected_case_ids.clone(),
        sidecar_sha256: sidecar_file.sha256.clone(),
        aggregate_sha256: aggregate_sha256.clone(),
        archive_sha256: archive_sha256.clone(),
        manifest_sha256: manifest_sha256.clone(),
    };
    compact_value.validate()?;
    let compact_record = canonical_json(&compact_value)?;
    let compact_record_sha256 = local_file::sha256_hex(&compact_record);
    Ok(PreparedSealedEvidenceV1 {
        evidence_id: sidecar.schedule_record_id,
        evidence_class: request.evidence_class,
        terminal_at_ms: request.terminal_at_ms,
        case_minimum_days: request.case_minimum_days,
        release_retain_until_ms: request.release_retain_until_ms,
        sidecar_sha256: sidecar_file.sha256.clone(),
        aggregate_sha256,
        archive,
        archive_sha256,
        manifest,
        manifest_sha256,
        compact_record,
        compact_record_sha256,
    })
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct EvidenceHotStoreV1 {
    root: local_file::PinnedDirectory,
    state: local_file::PinnedDirectory,
    scratch: local_file::PinnedDirectory,
    sealed: local_file::PinnedDirectory,
}

impl EvidenceHotStoreV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn open_existing(root: &local_file::PinnedDirectory) -> Result<Self, BoxError> {
        validate_private_directory_metadata(&root.file_handle().metadata()?, "hot evidence root")?;
        if !root.current_path_matches() {
            return Err("schedule evidence: hot evidence root path changed".into());
        }
        let state = root.open_child_directory(OsStr::new("state"), "hot evidence state")?;
        let scratch = root.open_child_directory(OsStr::new("scratch"), "hot evidence scratch")?;
        let sealed = root.open_child_directory(OsStr::new("sealed"), "hot sealed evidence")?;
        for (label, directory) in [
            ("hot evidence state", &state),
            ("hot evidence scratch", &scratch),
            ("hot sealed evidence", &sealed),
        ] {
            validate_private_directory_metadata(&directory.file_handle().metadata()?, label)?;
            if !directory.current_path_matches() {
                return Err(format!("schedule evidence: {label} path changed").into());
            }
        }
        Ok(Self {
            root: root.clone(),
            state,
            scratch,
            sealed,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn root_sha256(&self) -> &str {
        self.root.object_sha256()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn root_directory(&self) -> &local_file::PinnedDirectory {
        &self.root
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn sealed_directory(&self) -> &local_file::PinnedDirectory {
        &self.sealed
    }
}

impl PreparedSealedEvidenceV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("prepared evidence id", &self.evidence_id)?;
        if self.terminal_at_ms <= 0
            || self.archive.is_empty()
            || self.manifest.is_empty()
            || self.compact_record.is_empty()
            || local_file::sha256_hex(&self.archive) != self.archive_sha256
            || local_file::sha256_hex(&self.manifest) != self.manifest_sha256
            || local_file::sha256_hex(&self.compact_record) != self.compact_record_sha256
        {
            return Err("schedule evidence: prepared evidence hash or shape is invalid".into());
        }
        require_sha256("prepared sidecar", &self.sidecar_sha256)?;
        match &self.aggregate_sha256 {
            OptionalSha256V1::Absent => {}
            OptionalSha256V1::Sha256 { value } => require_sha256("prepared aggregate", value)?,
        }
        let manifest: SealedEvidenceManifestV1 = serde_json::from_slice(&self.manifest)?;
        if canonical_json(&manifest)? != self.manifest
            || manifest.schema_version != 1
            || manifest.evidence_id != self.evidence_id
            || manifest.terminal_at_ms != self.terminal_at_ms
            || manifest.sidecar_sha256 != self.sidecar_sha256
            || manifest.aggregate_sha256 != self.aggregate_sha256
            || manifest.archive_sha256 != self.archive_sha256
            || manifest.archive_bytes != self.archive.len() as u64
        {
            return Err("schedule evidence: prepared manifest binding is invalid".into());
        }
        let compact: CompactEvidenceRecordV1 = serde_json::from_slice(&self.compact_record)?;
        compact.validate()?;
        if canonical_json(&compact)? != self.compact_record
            || compact.schema_version != 1
            || compact.evidence_id != self.evidence_id
            || compact.evidence_class != self.evidence_class
            || compact.terminal_at_ms != self.terminal_at_ms
            || compact.sidecar_sha256 != self.sidecar_sha256
            || compact.aggregate_sha256 != self.aggregate_sha256
            || compact.archive_sha256 != self.archive_sha256
            || compact.manifest_sha256 != self.manifest_sha256
        {
            return Err("schedule evidence: prepared compact record binding is invalid".into());
        }
        decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: self.evidence_class,
            terminal_at_ms: self.terminal_at_ms,
            case_minimum_days: self.case_minimum_days,
            release_retain_until_ms: self.release_retain_until_ms,
            pinned: false,
        })?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum SealPublicationFailpointV1 {
    None,
    AfterScratchArchive,
    AfterSealedArchive,
    AfterSealed,
    AfterIndexPublication,
}

#[derive(Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct PublishedEvidenceV1 {
    pub(super) snapshot: EvidenceStateSnapshotV1,
    pub(super) snapshot_sha256: String,
    pub(super) usage: HotStorageUsageV1,
    pub(super) hot_path: RelativeEvidencePathV1,
    pub(super) scratch_cleanup_required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct UnindexedEvidenceV1 {
    pub(super) scratch: Vec<String>,
    pub(super) sealed: Vec<String>,
}

impl UnindexedEvidenceV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn is_empty(&self) -> bool {
        self.scratch.is_empty() && self.sealed.is_empty()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn write_verified_payload_file(
    directory: &local_file::PinnedDirectory,
    name: &str,
    bytes: &[u8],
    sha256: &str,
) -> Result<(), BoxError> {
    let mut file =
        directory.create_new_file(OsStr::new(name), STATE_FILE_MODE, "sealed evidence payload")?;
    file.write_all(bytes)?;
    file.sync_all()?;
    directory.sync()?;
    drop(file);
    let reopened = directory.open_regular_file(OsStr::new(name), "sealed evidence payload")?;
    validate_private_file_metadata(&reopened.metadata()?, "sealed evidence payload")?;
    let snapshot = local_file::read_open_regular_file_bounded(
        &reopened,
        "sealed evidence payload",
        MAX_SEAL_TOTAL_BYTES,
    )?;
    if snapshot.bytes != bytes || snapshot.sha256 != sha256 {
        return Err("schedule evidence: persisted payload verification failed".into());
    }
    Ok(())
}

fn expected_payload_files(prepared: &PreparedSealedEvidenceV1) -> [(&'static str, &[u8], &str); 2] {
    [
        (
            "evidence.tar.gz",
            prepared.archive.as_slice(),
            prepared.archive_sha256.as_str(),
        ),
        (
            "manifest.json",
            prepared.manifest.as_slice(),
            prepared.manifest_sha256.as_str(),
        ),
    ]
}

fn inspect_expected_payload_directory(
    parent: &local_file::PinnedDirectory,
    name: &str,
    expected: &[(&str, &[u8], &str)],
    label: &str,
) -> Result<(Option<local_file::PinnedDirectory>, u64), BoxError> {
    let Some(directory) = parent.open_child_directory_optional(OsStr::new(name), label)? else {
        return Ok((None, 0));
    };
    validate_private_directory_metadata(&directory.file_handle().metadata()?, label)?;
    let mut names = std::fs::read_dir(directory.acp_session_cwd())?
        .map(|entry| {
            entry?
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::other("payload child is not UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    if names.len() > expected.len()
        || names
            .iter()
            .any(|name| !expected.iter().any(|(expected, _, _)| name == expected))
    {
        return Err(format!("schedule evidence: {label} has an unexpected child").into());
    }
    let mut bytes = 0_u64;
    for (child_name, expected_bytes, expected_sha256) in expected {
        let Some(metadata) = directory.child_metadata_no_follow(OsStr::new(child_name), label)?
        else {
            continue;
        };
        if !metadata.is_regular()
            || metadata.link_count() != 1
            || metadata.owner_uid() != unsafe { libc::geteuid() }
            || metadata.permission_mode() != STATE_FILE_MODE
            || metadata.length() != expected_bytes.len() as u64
        {
            return Err(format!("schedule evidence: {label} child metadata changed").into());
        }
        let file = directory.open_regular_file(OsStr::new(child_name), label)?;
        validate_private_file_metadata(&file.metadata()?, label)?;
        let snapshot =
            local_file::read_open_regular_file_bounded(&file, label, expected_bytes.len() as u64)?;
        if snapshot.bytes != *expected_bytes || snapshot.sha256 != *expected_sha256 {
            return Err(format!("schedule evidence: {label} child bytes changed").into());
        }
        bytes = bytes
            .checked_add(snapshot.bytes.len() as u64)
            .ok_or("schedule evidence: payload residue byte count overflow")?;
    }
    if !directory.current_path_matches() || !parent.current_path_matches() {
        return Err(format!("schedule evidence: {label} path changed").into());
    }
    Ok((Some(directory), bytes))
}

fn ensure_expected_payload_file(
    directory: &local_file::PinnedDirectory,
    child_name: &str,
    bytes: &[u8],
    sha256: &str,
    label: &str,
) -> Result<(), BoxError> {
    if directory
        .child_metadata_no_follow(OsStr::new(child_name), label)?
        .is_none()
    {
        write_verified_payload_file(directory, child_name, bytes, sha256)?;
    }
    directory.sync()?;
    let file = directory.open_regular_file(OsStr::new(child_name), label)?;
    validate_private_file_metadata(&file.metadata()?, label)?;
    let snapshot = local_file::read_open_regular_file_bounded(&file, label, bytes.len() as u64)?;
    if snapshot.bytes != bytes || snapshot.sha256 != sha256 {
        return Err(format!("schedule evidence: {label} child bytes changed").into());
    }
    Ok(())
}

fn measure_flat_private_file_bytes(
    directory: &local_file::PinnedDirectory,
    label: &str,
) -> Result<u64, BoxError> {
    if !directory.current_path_matches() {
        return Err(format!("schedule evidence: {label} path changed").into());
    }
    let mut bytes = 0_u64;
    let mut entries = 0_usize;
    for entry in std::fs::read_dir(directory.acp_session_cwd())? {
        let name = entry?.file_name();
        let metadata = directory
            .child_metadata_no_follow(&name, label)?
            .ok_or_else(|| format!("schedule evidence: {label} entry disappeared"))?;
        if !metadata.is_regular()
            || metadata.link_count() != 1
            || metadata.owner_uid() != unsafe { libc::geteuid() }
            || metadata.permission_mode() != STATE_FILE_MODE
        {
            return Err(format!("schedule evidence: {label} contains an unsafe entry").into());
        }
        bytes = bytes
            .checked_add(metadata.length())
            .ok_or("schedule evidence: hot state byte count overflow")?;
        entries += 1;
        if entries > MAX_EVIDENCE_ITEMS * 4 {
            return Err(format!("schedule evidence: {label} inventory exceeds its bound").into());
        }
    }
    if !directory.current_path_matches() {
        return Err(format!("schedule evidence: {label} changed during measurement").into());
    }
    Ok(bytes)
}

fn measure_payload_root_bytes(
    directory: &local_file::PinnedDirectory,
    label: &str,
) -> Result<u64, BoxError> {
    let names = list_payload_children(directory, label)?;
    let mut bytes = 0_u64;
    for name in names {
        let payload = directory.open_child_directory(OsStr::new(&name), label)?;
        validate_private_directory_metadata(&payload.file_handle().metadata()?, label)?;
        bytes = bytes
            .checked_add(measure_flat_private_file_bytes(&payload, label)?)
            .ok_or("schedule evidence: hot payload byte count overflow")?;
    }
    Ok(bytes)
}

fn measure_hot_storage_usage(
    store: &EvidenceHotStoreV1,
    journal: &FileEvidenceJournal<'_>,
) -> Result<HotStorageUsageV1, BoxError> {
    let state_bytes = journal
        .state_quota
        .usage_bytes()?
        .checked_add(measure_flat_private_file_bytes(
            &store.state,
            "hot evidence state",
        )?)
        .ok_or("schedule evidence: hot state usage overflow")?;
    Ok(HotStorageUsageV1 {
        state_bytes,
        scratch_bytes: measure_payload_root_bytes(&store.scratch, "scratch payload")?,
        sealed_bytes: measure_payload_root_bytes(&store.sealed, "sealed payload")?,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
fn payload_object_name(evidence_id: &str) -> Result<String, BoxError> {
    stable_id("payload evidence id", evidence_id)?;
    Ok(local_file::sha256_hex(evidence_id.as_bytes()))
}

#[cfg_attr(not(test), allow(dead_code))]
fn incident_pin_id(evidence_id: &str) -> Result<String, BoxError> {
    stable_id("incident evidence id", evidence_id)?;
    Ok(format!(
        "incident-pin:{}",
        local_file::sha256_hex(evidence_id.as_bytes())
    ))
}

#[cfg_attr(not(test), allow(dead_code))]
fn cleanup_complete_scratch_payload(
    parent: &local_file::PinnedDirectory,
    name: &str,
    scratch: &local_file::PinnedDirectory,
) -> Result<(), BoxError> {
    for child in ["evidence.tar.gz", "manifest.json"] {
        scratch.remove_child(
            OsStr::new(child),
            false,
            "indexed evidence scratch file cleanup",
        )?;
    }
    scratch.sync()?;
    parent.remove_child(
        OsStr::new(name),
        true,
        "indexed evidence scratch directory cleanup",
    )?;
    parent.sync()?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn list_payload_children(
    directory: &local_file::PinnedDirectory,
    label: &str,
) -> Result<Vec<String>, BoxError> {
    if !directory.current_path_matches() {
        return Err(format!("schedule evidence: {label} directory path changed").into());
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(directory.acp_session_cwd())? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("schedule evidence: {label} entry is not UTF-8"))?;
        stable_id(label, name.trim_end_matches(".partial"))?;
        let metadata = std::fs::symlink_metadata(directory.acp_session_cwd().join(&name))?;
        validate_private_directory_metadata(&metadata, label)?;
        names.push(name);
        if names.len() > MAX_EVIDENCE_ITEMS * 4 {
            return Err(format!("schedule evidence: {label} inventory exceeds its bound").into());
        }
    }
    if !directory.current_path_matches() {
        return Err(format!("schedule evidence: {label} directory changed during scan").into());
    }
    names.sort();
    Ok(names)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn inspect_unindexed_evidence(
    store: &EvidenceHotStoreV1,
    state: &EvidenceStateModelV1,
) -> Result<UnindexedEvidenceV1, BoxError> {
    state.validate()?;
    if state.hot_root_sha256 != store.root_sha256() {
        return Err("schedule evidence: state/hot-root binding mismatch".into());
    }
    let referenced = state
        .entries
        .values()
        .filter(|entry| {
            entry.hot_present
                && entry.hot_path.components.len() == 2
                && entry
                    .hot_path
                    .components
                    .first()
                    .is_some_and(|value| value == "sealed")
        })
        .map(|entry| entry.hot_path.components[1].clone())
        .collect::<BTreeSet<_>>();
    let scratch = list_payload_children(&store.scratch, "scratch payload")?;
    let sealed = list_payload_children(&store.sealed, "sealed payload")?
        .into_iter()
        .filter(|name| !referenced.contains(name))
        .collect();
    Ok(UnindexedEvidenceV1 { scratch, sealed })
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn publish_prepared_evidence(
    store: &EvidenceHotStoreV1,
    journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    prepared: &PreparedSealedEvidenceV1,
    caps: &HotStorageCapsV1,
    recorded_at_ms: i64,
    failpoint: SealPublicationFailpointV1,
) -> Result<PublishedEvidenceV1, BoxError> {
    prepared.validate()?;
    state.validate()?;
    if state.hot_root_sha256 != store.root_sha256() {
        return Err("schedule evidence: publication state/hot-root binding mismatch".into());
    }
    let retention = decide_retention(&EvidenceRetentionRequestV1 {
        evidence_class: prepared.evidence_class,
        terminal_at_ms: prepared.terminal_at_ms,
        case_minimum_days: prepared.case_minimum_days,
        release_retain_until_ms: prepared.release_retain_until_ms,
        pinned: false,
    })?;
    let object_name = payload_object_name(&prepared.evidence_id)?;
    let hot_path = RelativeEvidencePathV1 {
        components: vec!["sealed".into(), object_name.clone()],
    };
    let indexed_entry = IndexedEvidenceV1 {
        evidence_id: prepared.evidence_id.clone(),
        evidence_class: prepared.evidence_class,
        full_evidence_sha256: prepared.archive_sha256.clone(),
        manifest_sha256: prepared.manifest_sha256.clone(),
        compact_record_sha256: prepared.compact_record_sha256.clone(),
        archive_bytes: prepared.archive.len() as u64,
        manifest_bytes: prepared.manifest.len() as u64,
        compact_record_bytes: prepared.compact_record.len() as u64,
        compact_record: String::from_utf8(prepared.compact_record.clone())?,
        hot_path: hot_path.clone(),
        cold_path: OptionalRelativeEvidencePathV1::Absent,
        terminal_at_ms: prepared.terminal_at_ms,
        case_minimum_days: prepared.case_minimum_days,
        full_retain_until_ms: retention.full_retain_until_ms,
        compact_retain_until_ms: retention.compact_retain_until_ms,
        hot_retain_until_ms: retention.hot_retain_until_ms,
        hot_present: true,
    };
    let expected_files = expected_payload_files(prepared);
    let sealed_bytes = (prepared.archive.len() as u64)
        .checked_add(prepared.manifest.len() as u64)
        .ok_or("schedule evidence: sealed payload byte overflow")?;
    let scratch_name = format!("{object_name}.partial");

    if let Some(existing) = state.entries.get(&prepared.evidence_id) {
        if existing != &indexed_entry || journal.previous_snapshot.state != *state {
            return Err("schedule evidence: existing publication identity diverged".into());
        }
        if prepared.evidence_class == EvidenceClassV1::Incident {
            let pin_id = incident_pin_id(&prepared.evidence_id)?;
            if !state.pins.get(&pin_id).is_some_and(|pin| {
                pin.evidence_id == prepared.evidence_id && pin.lifecycle == PinLifecycleV1::Active
            }) {
                return Err("schedule evidence: existing incident publication lost its pin".into());
            }
        }
        let (_, persisted_sealed_bytes) = inspect_expected_payload_directory(
            &store.sealed,
            &object_name,
            &expected_files,
            "sealed evidence recovery payload",
        )?;
        if persisted_sealed_bytes != sealed_bytes {
            return Err("schedule evidence: indexed sealed payload is incomplete".into());
        }
        let (scratch, persisted_scratch_bytes) = inspect_expected_payload_directory(
            &store.scratch,
            &scratch_name,
            &expected_files,
            "scratch evidence recovery payload",
        )?;
        if scratch.is_some() && persisted_scratch_bytes != sealed_bytes {
            return Err("schedule evidence: indexed scratch payload is incomplete".into());
        }
        let scratch_cleanup_required = scratch.is_some_and(|scratch| {
            cleanup_complete_scratch_payload(&store.scratch, &scratch_name, &scratch).is_err()
        });
        let usage = measure_hot_storage_usage(store, journal)?;
        reserve_optional_hot_bytes(caps, &usage, HotAllocationV1::State, 0)?;
        let snapshot = journal.previous_snapshot.clone();
        let snapshot_sha256 = evidence_state_snapshot_sha256(&snapshot)?;
        return Ok(PublishedEvidenceV1 {
            snapshot,
            snapshot_sha256,
            usage,
            hot_path,
            scratch_cleanup_required,
        });
    }

    let mut candidate = state.clone();
    candidate.insert_entry(indexed_entry)?;
    if prepared.evidence_class == EvidenceClassV1::Incident {
        candidate.pin(EvidencePinV1 {
            pin_id: incident_pin_id(&prepared.evidence_id)?,
            evidence_id: prepared.evidence_id.clone(),
            reason: "incident evidence pinned at publication".into(),
            created_at_ms: recorded_at_ms,
            lifecycle: PinLifecycleV1::Active,
        })?;
    }
    let next_snapshot = journal.validate_append_candidate(&candidate, recorded_at_ms)?;
    let (_, existing_scratch_bytes) = inspect_expected_payload_directory(
        &store.scratch,
        &scratch_name,
        &expected_files,
        "scratch evidence recovery payload",
    )?;
    let (_, existing_sealed_bytes) = inspect_expected_payload_directory(
        &store.sealed,
        &object_name,
        &expected_files,
        "sealed evidence recovery payload",
    )?;
    let missing_scratch_bytes = sealed_bytes
        .checked_sub(existing_scratch_bytes)
        .ok_or("schedule evidence: scratch residue exceeds expected payload")?;
    let missing_sealed_bytes = sealed_bytes
        .checked_sub(existing_sealed_bytes)
        .ok_or("schedule evidence: sealed residue exceeds expected payload")?;
    let usage = measure_hot_storage_usage(store, journal)?;
    let state_bytes = u64::try_from(evidence_state_snapshot_bytes(&next_snapshot)?.len())?;
    let after_state = reserve_hot_bytes(caps, &usage, HotAllocationV1::State, state_bytes)?;
    let peak_scratch = reserve_optional_hot_bytes(
        caps,
        &after_state,
        HotAllocationV1::Scratch,
        missing_scratch_bytes,
    )?;
    let _peak_both = reserve_optional_hot_bytes(
        caps,
        &peak_scratch,
        HotAllocationV1::Sealed,
        missing_sealed_bytes,
    )?;

    let scratch = store.scratch.open_or_create_child_directory(
        OsStr::new(&scratch_name),
        0o700,
        "evidence scratch payload",
    )?;
    validate_private_directory_metadata(
        &scratch.file_handle().metadata()?,
        "evidence scratch payload",
    )?;
    store.scratch.sync()?;
    inspect_expected_payload_directory(
        &store.scratch,
        &scratch_name,
        &expected_files,
        "evidence scratch payload",
    )?;
    ensure_expected_payload_file(
        &scratch,
        "evidence.tar.gz",
        &prepared.archive,
        &prepared.archive_sha256,
        "evidence scratch archive",
    )?;
    if failpoint == SealPublicationFailpointV1::AfterScratchArchive {
        return Err("schedule evidence: injected crash after scratch archive".into());
    }
    ensure_expected_payload_file(
        &scratch,
        "manifest.json",
        &prepared.manifest,
        &prepared.manifest_sha256,
        "evidence scratch manifest",
    )?;
    scratch.sync()?;
    store.scratch.sync()?;

    let sealed = store.sealed.open_or_create_child_directory(
        OsStr::new(&object_name),
        0o700,
        "sealed evidence payload",
    )?;
    validate_private_directory_metadata(
        &sealed.file_handle().metadata()?,
        "sealed evidence payload",
    )?;
    store.sealed.sync()?;
    inspect_expected_payload_directory(
        &store.sealed,
        &object_name,
        &expected_files,
        "sealed evidence payload",
    )?;
    ensure_expected_payload_file(
        &sealed,
        "evidence.tar.gz",
        &prepared.archive,
        &prepared.archive_sha256,
        "sealed evidence archive",
    )?;
    if failpoint == SealPublicationFailpointV1::AfterSealedArchive {
        return Err("schedule evidence: injected crash after sealed archive".into());
    }
    ensure_expected_payload_file(
        &sealed,
        "manifest.json",
        &prepared.manifest,
        &prepared.manifest_sha256,
        "sealed evidence manifest",
    )?;
    sealed.sync()?;
    store.sealed.sync()?;
    if failpoint == SealPublicationFailpointV1::AfterSealed {
        return Err("schedule evidence: injected crash after sealed payload".into());
    }

    let (snapshot, snapshot_sha256) = journal.append(&candidate, recorded_at_ms)?;
    if failpoint == SealPublicationFailpointV1::AfterIndexPublication {
        return Err("schedule evidence: injected crash after index publication".into());
    }
    *state = candidate;
    let scratch_cleanup_required =
        cleanup_complete_scratch_payload(&store.scratch, &scratch_name, &scratch).is_err();
    let usage = measure_hot_storage_usage(store, journal)?;
    Ok(PublishedEvidenceV1 {
        snapshot,
        snapshot_sha256,
        usage,
        hot_path,
        scratch_cleanup_required,
    })
}

#[cfg_attr(not(test), allow(dead_code))]
const INCIDENT_MIGRATION_PREFIX: &str = "incident-migration.";
#[cfg_attr(not(test), allow(dead_code))]
const MAX_INCIDENT_MIGRATIONS: usize = 1_024;
#[cfg_attr(not(test), allow(dead_code))]
const MAX_INCIDENT_MIGRATION_RECORD_BYTES: u64 = 64 * 1024;
#[cfg_attr(not(test), allow(dead_code))]
const R3B_INCIDENT_MIGRATION_MANIFEST: &[u8] =
    include_bytes!("../../../compatibility/r3b-incident-migration-v1.json");

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct IncidentMigrationManifestV1 {
    pub(super) schema_version: u16,
    pub(super) incidents: Vec<IncidentMigrationItemV1>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct IncidentMigrationItemV1 {
    pub(super) incident_id: String,
    pub(super) incident_reference: String,
    pub(super) source_path: String,
    pub(super) expected_mode: u32,
    pub(super) expected_length_bytes: u64,
    pub(super) expected_sha256: String,
    pub(super) affected_case_ids: Vec<String>,
}

impl IncidentMigrationItemV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        stable_id("incident migration id", &self.incident_id)?;
        require_sha256("incident migration source", &self.expected_sha256)?;
        if self.incident_reference.is_empty()
            || self.incident_reference.len() > 128
            || !self.incident_reference.bytes().all(|byte| {
                byte.is_ascii_uppercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
            || self.source_path.is_empty()
            || self.source_path.len() > 4_096
            || self.source_path.bytes().any(|byte| byte == 0)
            || !std::path::Path::new(&self.source_path).is_absolute()
            || self.expected_mode != STATE_FILE_MODE
            || self.expected_length_bytes == 0
            || self.expected_length_bytes > MAX_SEAL_FILE_BYTES
            || self.affected_case_ids.is_empty()
            || self.affected_case_ids.len() > MAX_EVIDENCE_ITEMS
        {
            return Err("schedule evidence: incident migration item shape is invalid".into());
        }
        let mut normalized_cases = self.affected_case_ids.clone();
        for case_id in &normalized_cases {
            stable_id("incident migration affected case", case_id)?;
        }
        normalized_cases.sort();
        normalized_cases.dedup();
        if normalized_cases != self.affected_case_ids {
            return Err(
                "schedule evidence: incident migration cases are not sorted and unique".into(),
            );
        }
        Ok(())
    }
}

impl IncidentMigrationManifestV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1
            || self.incidents.is_empty()
            || self.incidents.len() > MAX_EVIDENCE_ITEMS
        {
            return Err("schedule evidence: incident migration manifest shape is invalid".into());
        }
        let mut incident_ids = BTreeSet::new();
        let mut source_paths = BTreeSet::new();
        for item in &self.incidents {
            item.validate()?;
            if !incident_ids.insert(item.incident_id.clone())
                || !source_paths.insert(item.source_path.clone())
            {
                return Err(
                    "schedule evidence: incident migration manifest identity is duplicated".into(),
                );
            }
        }
        let mut sorted = self.incidents.clone();
        sorted.sort_by(|left, right| left.incident_id.cmp(&right.incident_id));
        if sorted != self.incidents {
            return Err("schedule evidence: incident migration manifest is not sorted".into());
        }
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn r3b_incident_migration_manifest() -> Result<IncidentMigrationManifestV1, BoxError> {
    let value: IncidentMigrationManifestV1 =
        serde_json::from_slice(R3B_INCIDENT_MIGRATION_MANIFEST)?;
    value.validate()?;
    if value.incidents.len() != 2
        || canonical_json(&value)?.as_slice() != R3B_INCIDENT_MIGRATION_MANIFEST
    {
        return Err(
            "schedule evidence: checked-in R3b incident migration manifest is not exact".into(),
        );
    }
    Ok(value)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "disposition", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum IncidentMigrationDispositionV1 {
    Missing,
    Mismatch {
        reason_code: String,
        observed_mode: u32,
        observed_length_bytes: u64,
        observed_sha256: OptionalSha256V1,
    },
    Migrated {
        archive_sha256: String,
        manifest_sha256: String,
        compact_record_sha256: String,
    },
}

impl IncidentMigrationDispositionV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        match self {
            Self::Missing => Ok(()),
            Self::Mismatch {
                reason_code,
                observed_mode,
                observed_sha256,
                ..
            } => {
                stable_id("incident migration mismatch reason", reason_code)?;
                if *observed_mode > 0o777 {
                    return Err(
                        "schedule evidence: incident migration observed mode is invalid".into(),
                    );
                }
                if let OptionalSha256V1::Sha256 { value } = observed_sha256 {
                    require_sha256("incident migration observed source", value)?;
                }
                Ok(())
            }
            Self::Migrated {
                archive_sha256,
                manifest_sha256,
                compact_record_sha256,
            } => {
                require_sha256("migrated incident archive", archive_sha256)?;
                require_sha256("migrated incident manifest", manifest_sha256)?;
                require_sha256("migrated incident compact record", compact_record_sha256)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct IncidentMigrationRecordV1 {
    schema_version: u16,
    generation: u64,
    previous_record: OptionalSha256V1,
    recorded_at_ms: i64,
    item: IncidentMigrationItemV1,
    disposition: IncidentMigrationDispositionV1,
}

impl IncidentMigrationRecordV1 {
    #[cfg_attr(not(test), allow(dead_code))]
    fn validate(&self) -> Result<(), BoxError> {
        if self.schema_version != 1 || self.generation == 0 || self.recorded_at_ms <= 0 {
            return Err("schedule evidence: incident migration record header is invalid".into());
        }
        match (&self.previous_record, self.generation) {
            (OptionalSha256V1::Absent, 1) => {}
            (OptionalSha256V1::Sha256 { value }, generation) if generation > 1 => {
                require_sha256("incident migration predecessor", value)?;
            }
            _ => {
                return Err(
                    "schedule evidence: incident migration predecessor shape is invalid".into(),
                )
            }
        }
        self.item.validate()?;
        self.disposition.validate()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn incident_migration_record_sha256(
    record: &IncidentMigrationRecordV1,
) -> Result<String, BoxError> {
    record.validate()?;
    Ok(local_file::sha256_hex(&canonical_json(record)?))
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct FileIncidentMigrationJournal<'lock> {
    directory: &'lock local_file::PinnedDirectory,
    state_quota: StateQuota,
    records: Vec<IncidentMigrationRecordV1>,
}

impl<'lock> FileIncidentMigrationJournal<'lock> {
    #[cfg_attr(not(test), allow(dead_code))]
    fn generation_name(generation: u64) -> String {
        format!("{INCIDENT_MIGRATION_PREFIX}{generation:020}.json")
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn generation_entries(
        directory: &local_file::PinnedDirectory,
    ) -> Result<Vec<(u64, String)>, BoxError> {
        if !directory.current_path_matches() {
            return Err("schedule evidence: incident migration directory path changed".into());
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(directory.canonical_path())? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.starts_with(INCIDENT_MIGRATION_PREFIX) {
                continue;
            }
            let Some(raw) = name
                .strip_prefix(INCIDENT_MIGRATION_PREFIX)
                .and_then(|value| value.strip_suffix(".json"))
            else {
                return Err("schedule evidence: malformed incident migration generation".into());
            };
            if raw.len() != 20 || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err("schedule evidence: malformed incident migration generation".into());
            }
            entries.push((raw.parse()?, name.into()));
        }
        if entries.len() > MAX_INCIDENT_MIGRATIONS || !directory.current_path_matches() {
            return Err(
                "schedule evidence: incident migration scan is unbounded or unstable".into(),
            );
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn read_generation(
        directory: &local_file::PinnedDirectory,
        name: &str,
    ) -> Result<IncidentMigrationRecordV1, BoxError> {
        let file = directory.open_regular_file(OsStr::new(name), "incident migration record")?;
        let metadata = file.metadata()?;
        validate_private_file_metadata(&metadata, "incident migration record")?;
        if metadata.len() > MAX_INCIDENT_MIGRATION_RECORD_BYTES {
            return Err("schedule evidence: incident migration record is oversized".into());
        }
        let snapshot = local_file::read_open_regular_file_bounded(
            &file,
            "incident migration record",
            MAX_INCIDENT_MIGRATION_RECORD_BYTES,
        )?;
        let record: IncidentMigrationRecordV1 = serde_json::from_slice(&snapshot.bytes)?;
        if canonical_json(&record)? != snapshot.bytes {
            return Err(
                "schedule evidence: incident migration record is not canonical JSON".into(),
            );
        }
        record.validate()?;
        Ok(record)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn validate_records(records: &[IncidentMigrationRecordV1]) -> Result<(), BoxError> {
        if records.len() > MAX_INCIDENT_MIGRATIONS {
            return Err("schedule evidence: incident migration history exceeds its bound".into());
        }
        let mut prior_global: Option<&IncidentMigrationRecordV1> = None;
        let mut latest_by_incident = BTreeMap::<String, &IncidentMigrationRecordV1>::new();
        for (index, record) in records.iter().enumerate() {
            record.validate()?;
            let expected_generation = u64::try_from(index + 1)?;
            if record.generation != expected_generation {
                return Err(
                    "schedule evidence: incident migration generations are not contiguous".into(),
                );
            }
            if let Some(previous) = prior_global {
                if record.recorded_at_ms <= previous.recorded_at_ms
                    || record.previous_record
                        != (OptionalSha256V1::Sha256 {
                            value: incident_migration_record_sha256(previous)?,
                        })
                {
                    return Err(
                        "schedule evidence: incident migration chain is not monotonic".into(),
                    );
                }
            }
            if let Some(previous) = latest_by_incident.get(&record.item.incident_id) {
                if previous.item != record.item
                    || matches!(
                        previous.disposition,
                        IncidentMigrationDispositionV1::Migrated { .. }
                    )
                {
                    return Err(
                        "schedule evidence: incident migration identity changed after recording"
                            .into(),
                    );
                }
            }
            latest_by_incident.insert(record.item.incident_id.clone(), record);
            prior_global = Some(record);
        }
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn open_existing_or_empty<C: EvidenceStateCapability + ?Sized>(
        capability: &'lock C,
    ) -> Result<Self, BoxError> {
        let directory = capability.migration_directory();
        let mut records = Vec::new();
        for (index, (generation, name)) in
            Self::generation_entries(directory)?.into_iter().enumerate()
        {
            if generation != u64::try_from(index + 1)? {
                return Err(
                    "schedule evidence: incident migration filenames are not contiguous".into(),
                );
            }
            let record = Self::read_generation(directory, &name)?;
            if record.generation != generation {
                return Err(
                    "schedule evidence: incident migration filename/record mismatch".into(),
                );
            }
            records.push(record);
        }
        Self::validate_records(&records)?;
        Ok(Self {
            directory,
            state_quota: capability.state_quota(),
            records,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn latest_for(&self, incident_id: &str) -> Option<&IncidentMigrationRecordV1> {
        self.records
            .iter()
            .rev()
            .find(|record| record.item.incident_id == incident_id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn append(
        &mut self,
        item: &IncidentMigrationItemV1,
        disposition: IncidentMigrationDispositionV1,
        recorded_at_ms: i64,
    ) -> Result<(), BoxError> {
        item.validate()?;
        disposition.validate()?;
        if self.records.len() >= MAX_INCIDENT_MIGRATIONS
            || self
                .records
                .last()
                .is_some_and(|record| recorded_at_ms <= record.recorded_at_ms)
        {
            return Err(
                "schedule evidence: incident migration append time/bound is invalid".into(),
            );
        }
        if let Some(previous) = self.latest_for(&item.incident_id) {
            if previous.item != *item
                || matches!(
                    previous.disposition,
                    IncidentMigrationDispositionV1::Migrated { .. }
                )
            {
                return Err(
                    "schedule evidence: incident migration cannot change or repin identity".into(),
                );
            }
        }
        let generation = u64::try_from(self.records.len() + 1)?;
        let previous_record = match self.records.last() {
            Some(previous) => OptionalSha256V1::Sha256 {
                value: incident_migration_record_sha256(previous)?,
            },
            None => OptionalSha256V1::Absent,
        };
        let record = IncidentMigrationRecordV1 {
            schema_version: 1,
            generation,
            previous_record,
            recorded_at_ms,
            item: item.clone(),
            disposition,
        };
        let mut candidate = self.records.clone();
        candidate.push(record.clone());
        Self::validate_records(&candidate)?;
        let bytes = canonical_json(&record)?;
        if bytes.len() as u64 > MAX_INCIDENT_MIGRATION_RECORD_BYTES {
            return Err("schedule evidence: incident migration record exceeds its bound".into());
        }
        self.directory
            .recover_journal_append_residue(STATE_FILE_MODE, "incident migration record")?;
        self.state_quota.reserve(bytes.len() as u64)?;
        let name = Self::generation_name(generation);
        self.directory.write_new_journal_record(
            OsStr::new(&name),
            &bytes,
            STATE_FILE_MODE,
            "incident migration record",
        )?;
        self.records = candidate;
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn records(&self) -> &[IncidentMigrationRecordV1] {
        &self.records
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
struct IncidentMigrationSidecarV1 {
    schema_version: u16,
    migration_kind: String,
    incident_id: String,
    incident_reference: String,
    manifest_item_sha256: String,
    source_path_sha256: String,
    source_mode: u32,
    source_length_bytes: u64,
    source_sha256: String,
    affected_case_ids: Vec<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
fn incident_migration_sidecar(
    item: &IncidentMigrationItemV1,
) -> Result<(Vec<u8>, String), BoxError> {
    item.validate()?;
    let value = IncidentMigrationSidecarV1 {
        schema_version: 1,
        migration_kind: "r3b_incident_aggregate".into(),
        incident_id: item.incident_id.clone(),
        incident_reference: item.incident_reference.clone(),
        manifest_item_sha256: local_file::sha256_hex(&canonical_json(item)?),
        source_path_sha256: local_file::sha256_hex(item.source_path.as_bytes()),
        source_mode: item.expected_mode,
        source_length_bytes: item.expected_length_bytes,
        source_sha256: item.expected_sha256.clone(),
        affected_case_ids: item.affected_case_ids.clone(),
    };
    let bytes = canonical_json(&value)?;
    let sha256 = local_file::sha256_hex(&bytes);
    scan_sealed_file(&["incident-migration-sidecar.json".into()], &bytes)?;
    Ok((bytes, sha256))
}

#[cfg_attr(not(test), allow(dead_code))]
enum IncidentSourceObservationV1 {
    Missing,
    Mismatch(IncidentMigrationDispositionV1),
    Exact(Vec<u8>),
}

#[cfg_attr(not(test), allow(dead_code))]
fn mismatch_disposition(
    reason_code: &str,
    observed_mode: u32,
    observed_length_bytes: u64,
    observed_sha256: OptionalSha256V1,
) -> IncidentMigrationDispositionV1 {
    IncidentMigrationDispositionV1::Mismatch {
        reason_code: reason_code.into(),
        observed_mode,
        observed_length_bytes,
        observed_sha256,
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn inspect_incident_source(
    item: &IncidentMigrationItemV1,
) -> Result<IncidentSourceObservationV1, BoxError> {
    use std::os::unix::fs::MetadataExt as _;

    item.validate()?;
    let path = std::path::Path::new(&item.source_path);
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IncidentSourceObservationV1::Missing)
        }
        Err(error) => {
            return Err(format!(
                "schedule evidence: cannot inspect incident source {}: {error}",
                path.display()
            )
            .into())
        }
    };
    let observed_mode = metadata.mode() & 0o777;
    let observed_length_bytes = metadata.len();
    let metadata_reason = if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
    {
        Some("unsafe_source_metadata")
    } else if observed_mode != item.expected_mode {
        Some("mode_mismatch")
    } else if observed_length_bytes != item.expected_length_bytes {
        Some("length_mismatch")
    } else {
        None
    };
    if let Some(reason) = metadata_reason {
        return Ok(IncidentSourceObservationV1::Mismatch(mismatch_disposition(
            reason,
            observed_mode,
            observed_length_bytes,
            OptionalSha256V1::Absent,
        )));
    }

    let snapshot = local_file::read_regular_file_bounded(
        path,
        "R3b incident migration source",
        item.expected_length_bytes,
    )?;
    let final_metadata = std::fs::symlink_metadata(path)?;
    if !final_metadata.is_file()
        || final_metadata.uid() != unsafe { libc::geteuid() }
        || final_metadata.nlink() != 1
        || final_metadata.mode() & 0o777 != observed_mode
        || final_metadata.len() != observed_length_bytes
    {
        return Err("schedule evidence: incident source metadata changed during inspection".into());
    }
    if snapshot.bytes.len() as u64 != item.expected_length_bytes
        || snapshot.sha256 != item.expected_sha256
    {
        return Ok(IncidentSourceObservationV1::Mismatch(mismatch_disposition(
            "hash_mismatch",
            observed_mode,
            observed_length_bytes,
            OptionalSha256V1::Sha256 {
                value: snapshot.sha256,
            },
        )));
    }
    crate::compatibility::validate_child_aggregate_bytes(&snapshot.bytes)?;
    Ok(IncidentSourceObservationV1::Exact(snapshot.bytes))
}

#[cfg_attr(not(test), allow(dead_code))]
fn prepare_incident_migration_evidence(
    item: &IncidentMigrationItemV1,
    aggregate: Vec<u8>,
    recorded_at_ms: i64,
) -> Result<PreparedSealedEvidenceV1, BoxError> {
    item.validate()?;
    if recorded_at_ms <= 0
        || aggregate.len() as u64 != item.expected_length_bytes
        || local_file::sha256_hex(&aggregate) != item.expected_sha256
    {
        return Err("schedule evidence: incident migration source identity diverged".into());
    }
    crate::compatibility::validate_child_aggregate_bytes(&aggregate)?;
    let (sidecar, sidecar_sha256) = incident_migration_sidecar(item)?;
    let files = vec![
        SnapshottedSealFileV1 {
            path: vec!["incident-migration-sidecar.json".into()],
            bytes: sidecar,
            sha256: sidecar_sha256.clone(),
        },
        SnapshottedSealFileV1 {
            path: vec!["pinned-aggregate.json".into()],
            bytes: aggregate,
            sha256: item.expected_sha256.clone(),
        },
    ];
    for file in &files {
        scan_sealed_file(&file.path, &file.bytes)?;
    }
    let archive = deterministic_archive(&[], &files)?;
    let archive_sha256 = local_file::sha256_hex(&archive);
    let manifest_files = files
        .iter()
        .map(|file| SealedEvidenceFileV1 {
            path: evidence_path_string(&file.path),
            length_bytes: file.bytes.len() as u64,
            sha256: file.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let manifest_directories = Vec::<String>::new();
    let source_tree_sha256 =
        local_file::sha256_hex(&canonical_json(&(&manifest_directories, &manifest_files))?);
    let manifest_value = SealedEvidenceManifestV1 {
        schema_version: 1,
        evidence_id: item.incident_id.clone(),
        created_at_ms: recorded_at_ms,
        terminal_at_ms: recorded_at_ms,
        source_tree_sha256,
        directories: manifest_directories,
        files: manifest_files,
        sidecar_path: "incident-migration-sidecar.json".into(),
        sidecar_sha256: sidecar_sha256.clone(),
        aggregate_sha256: OptionalSha256V1::Sha256 {
            value: item.expected_sha256.clone(),
        },
        archive_sha256: archive_sha256.clone(),
        archive_bytes: archive.len() as u64,
    };
    let manifest = canonical_json(&manifest_value)?;
    let manifest_sha256 = local_file::sha256_hex(&manifest);
    let compact_value = CompactEvidenceRecordV1 {
        schema_version: 1,
        evidence_id: item.incident_id.clone(),
        evidence_class: EvidenceClassV1::Incident,
        terminal_at_ms: recorded_at_ms,
        affected_case_ids: item.affected_case_ids.clone(),
        sidecar_sha256,
        aggregate_sha256: OptionalSha256V1::Sha256 {
            value: item.expected_sha256.clone(),
        },
        archive_sha256: archive_sha256.clone(),
        manifest_sha256: manifest_sha256.clone(),
    };
    compact_value.validate()?;
    let compact_record = canonical_json(&compact_value)?;
    let compact_record_sha256 = local_file::sha256_hex(&compact_record);
    let prepared = PreparedSealedEvidenceV1 {
        evidence_id: item.incident_id.clone(),
        evidence_class: EvidenceClassV1::Incident,
        terminal_at_ms: recorded_at_ms,
        case_minimum_days: 0,
        release_retain_until_ms: None,
        sidecar_sha256: manifest_value.sidecar_sha256,
        aggregate_sha256: manifest_value.aggregate_sha256,
        archive,
        archive_sha256,
        manifest,
        manifest_sha256,
        compact_record,
        compact_record_sha256,
    };
    prepared.validate()?;
    Ok(prepared)
}

#[cfg_attr(not(test), allow(dead_code))]
fn existing_incident_migration_disposition(
    store: &EvidenceHotStoreV1,
    state: &EvidenceStateModelV1,
    item: &IncidentMigrationItemV1,
) -> Result<Option<IncidentMigrationDispositionV1>, BoxError> {
    state.validate()?;
    let Some(entry) = state.entries.get(&item.incident_id) else {
        if state.retired_evidence_ids.contains(&item.incident_id)
            || state
                .tombstones
                .values()
                .any(|tombstone| tombstone.evidence_id == item.incident_id)
        {
            return Err(
                "schedule evidence: incident migration id has retired evidence history".into(),
            );
        }
        return Ok(None);
    };
    let compact: CompactEvidenceRecordV1 = serde_json::from_str(&entry.compact_record)?;
    let expected_path = RelativeEvidencePathV1 {
        components: vec!["sealed".into(), payload_object_name(&item.incident_id)?],
    };
    let pin_id = incident_pin_id(&item.incident_id)?;
    if entry.evidence_class != EvidenceClassV1::Incident
        || !entry.hot_present
        || entry.hot_path != expected_path
        || compact.aggregate_sha256
            != (OptionalSha256V1::Sha256 {
                value: item.expected_sha256.clone(),
            })
        || !matches!(
            state.pins.get(&pin_id).map(|pin| &pin.lifecycle),
            Some(PinLifecycleV1::Active)
        )
    {
        return Err(
            "schedule evidence: existing incident id does not match the migration identity".into(),
        );
    }

    let object_name = entry
        .hot_path
        .components
        .last()
        .ok_or("schedule evidence: migrated incident hot path is empty")?;
    let object = store
        .sealed
        .open_child_directory(OsStr::new(object_name), "migrated incident payload")?;
    let archive_file =
        object.open_regular_file(OsStr::new("evidence.tar.gz"), "migrated incident archive")?;
    validate_private_file_metadata(&archive_file.metadata()?, "migrated incident archive")?;
    let archive = local_file::read_open_regular_file_bounded(
        &archive_file,
        "migrated incident archive",
        MAX_SEAL_TOTAL_BYTES,
    )?;
    let manifest_file =
        object.open_regular_file(OsStr::new("manifest.json"), "migrated incident manifest")?;
    validate_private_file_metadata(&manifest_file.metadata()?, "migrated incident manifest")?;
    let manifest = local_file::read_open_regular_file_bounded(
        &manifest_file,
        "migrated incident manifest",
        MAX_SEAL_FILE_BYTES,
    )?;
    if archive.sha256 != entry.full_evidence_sha256
        || archive.bytes.len() as u64 != entry.archive_bytes
        || manifest.sha256 != entry.manifest_sha256
        || manifest.bytes.len() as u64 != entry.manifest_bytes
    {
        return Err("schedule evidence: migrated incident payload hash diverged".into());
    }
    let manifest_value: SealedEvidenceManifestV1 = serde_json::from_slice(&manifest.bytes)?;
    let (sidecar, sidecar_sha256) = incident_migration_sidecar(item)?;
    let expected_files = vec![
        SealedEvidenceFileV1 {
            path: "incident-migration-sidecar.json".into(),
            length_bytes: sidecar.len() as u64,
            sha256: sidecar_sha256.clone(),
        },
        SealedEvidenceFileV1 {
            path: "pinned-aggregate.json".into(),
            length_bytes: item.expected_length_bytes,
            sha256: item.expected_sha256.clone(),
        },
    ];
    let expected_directories = Vec::<String>::new();
    let expected_source_tree_sha256 =
        local_file::sha256_hex(&canonical_json(&(&expected_directories, &expected_files))?);
    if canonical_json(&manifest_value)? != manifest.bytes
        || manifest_value.schema_version != 1
        || manifest_value.evidence_id != item.incident_id
        || manifest_value.created_at_ms != entry.terminal_at_ms
        || manifest_value.terminal_at_ms != entry.terminal_at_ms
        || manifest_value.source_tree_sha256 != expected_source_tree_sha256
        || manifest_value.directories != expected_directories
        || manifest_value.files != expected_files
        || manifest_value.sidecar_path != "incident-migration-sidecar.json"
        || manifest_value.sidecar_sha256 != sidecar_sha256
        || manifest_value.aggregate_sha256
            != (OptionalSha256V1::Sha256 {
                value: item.expected_sha256.clone(),
            })
        || manifest_value.archive_sha256 != entry.full_evidence_sha256
        || manifest_value.archive_bytes != entry.archive_bytes
        || compact.affected_case_ids != item.affected_case_ids
        || compact.sidecar_sha256 != sidecar_sha256
    {
        return Err("schedule evidence: existing incident payload is not this migration".into());
    }
    Ok(Some(IncidentMigrationDispositionV1::Migrated {
        archive_sha256: entry.full_evidence_sha256.clone(),
        manifest_sha256: entry.manifest_sha256.clone(),
        compact_record_sha256: entry.compact_record_sha256.clone(),
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum IncidentMigrationFailpointV1 {
    None,
    AfterEvidencePublication,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct IncidentMigrationResultV1 {
    pub(super) disposition: IncidentMigrationDispositionV1,
    pub(super) evidence_published: bool,
    pub(super) journal_appended: bool,
}

#[cfg_attr(not(test), allow(dead_code))]
fn record_incident_migration_disposition(
    journal: &mut FileIncidentMigrationJournal<'_>,
    item: &IncidentMigrationItemV1,
    disposition: IncidentMigrationDispositionV1,
    recorded_at_ms: i64,
) -> Result<IncidentMigrationResultV1, BoxError> {
    if let Some(previous) = journal.latest_for(&item.incident_id) {
        if previous.item != *item {
            return Err("schedule evidence: incident migration identity changed".into());
        }
        if previous.disposition == disposition {
            return Ok(IncidentMigrationResultV1 {
                disposition,
                evidence_published: false,
                journal_appended: false,
            });
        }
    }
    journal.append(item, disposition.clone(), recorded_at_ms)?;
    Ok(IncidentMigrationResultV1 {
        disposition,
        evidence_published: false,
        journal_appended: true,
    })
}

// Migration receives each journal, retained state, quota view, and failpoint independently so
// recovery cannot smuggle an implicit effect bundle across this boundary.
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn migrate_incident_evidence(
    store: &EvidenceHotStoreV1,
    evidence_journal: &mut FileEvidenceJournal<'_>,
    state: &mut EvidenceStateModelV1,
    migration_journal: &mut FileIncidentMigrationJournal<'_>,
    item: &IncidentMigrationItemV1,
    caps: &HotStorageCapsV1,
    usage: &mut HotStorageUsageV1,
    recorded_at_ms: i64,
    failpoint: IncidentMigrationFailpointV1,
) -> Result<IncidentMigrationResultV1, BoxError> {
    item.validate()?;
    state.validate()?;
    if recorded_at_ms <= 0 || state.hot_root_sha256 != store.root_sha256() {
        return Err("schedule evidence: incident migration state/root/time is invalid".into());
    }
    if let Some(previous) = migration_journal.latest_for(&item.incident_id) {
        if previous.item != *item {
            return Err(
                "schedule evidence: incident migration cannot repin changed source bytes".into(),
            );
        }
    }
    if let Some(disposition) = existing_incident_migration_disposition(store, state, item)? {
        if let Some(previous) = migration_journal.latest_for(&item.incident_id) {
            if matches!(
                previous.disposition,
                IncidentMigrationDispositionV1::Migrated { .. }
            ) && previous.disposition != disposition
            {
                return Err(
                    "schedule evidence: migrated incident journal/payload identity diverged".into(),
                );
            }
        }
        return record_incident_migration_disposition(
            migration_journal,
            item,
            disposition,
            recorded_at_ms,
        );
    }
    if migration_journal
        .latest_for(&item.incident_id)
        .is_some_and(|record| {
            matches!(
                record.disposition,
                IncidentMigrationDispositionV1::Migrated { .. }
            )
        })
    {
        return Err("schedule evidence: migrated incident payload disappeared".into());
    }

    let aggregate = match inspect_incident_source(item)? {
        IncidentSourceObservationV1::Missing => {
            return record_incident_migration_disposition(
                migration_journal,
                item,
                IncidentMigrationDispositionV1::Missing,
                recorded_at_ms,
            )
        }
        IncidentSourceObservationV1::Mismatch(disposition) => {
            return record_incident_migration_disposition(
                migration_journal,
                item,
                disposition,
                recorded_at_ms,
            )
        }
        IncidentSourceObservationV1::Exact(bytes) => bytes,
    };
    let prepared = prepare_incident_migration_evidence(item, aggregate, recorded_at_ms)?;
    let published = publish_prepared_evidence(
        store,
        evidence_journal,
        state,
        &prepared,
        caps,
        recorded_at_ms,
        SealPublicationFailpointV1::None,
    )?;
    *usage = published.usage;
    if failpoint == IncidentMigrationFailpointV1::AfterEvidencePublication {
        return Err("schedule evidence: injected crash after incident evidence publication".into());
    }
    let disposition = existing_incident_migration_disposition(store, state, item)?
        .ok_or("schedule evidence: migrated incident was not indexed after publication")?;
    let mut result = record_incident_migration_disposition(
        migration_journal,
        item,
        disposition,
        recorded_at_ms,
    )?;
    result.evidence_published = true;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compatibility_schedule::TriggerKindV1;
    use crate::compatibility_schedule_schema::{
        AdmissionAuthorityV1, CheckBindingV1, ColdStorageBindingV1, EffectiveIdentityV1,
        EvidenceClassV1, FingerprintV1, OptionalEffectiveIdentityV1, OptionalRecordRefV1,
        OptionalRelativeEvidencePathV1, OptionalSha256V1, OptionalStableIdV1, OptionalTextV1,
        ProfileSourceKindV1, ProfileSourceRefV1, RelativeEvidencePathV1, ScheduleEvidenceRecordV1,
        StandingGrantAuthorityV1,
    };
    use crate::compatibility_schedule_state::SchedulerStateRoot;
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    fn digest(ch: char) -> String {
        ch.to_string().repeat(64)
    }

    fn root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn relative(name: &str) -> RelativeEvidencePathV1 {
        RelativeEvidencePathV1 {
            components: vec!["sealed".into(), name.into()],
        }
    }

    fn entry(id: &str, terminal_at_ms: i64, bytes: u64) -> IndexedEvidenceV1 {
        entry_for_class(id, terminal_at_ms, bytes, EvidenceClassV1::RoutineGreen, 30)
    }

    fn entry_for_class(
        id: &str,
        terminal_at_ms: i64,
        bytes: u64,
        evidence_class: EvidenceClassV1,
        case_minimum_days: u32,
    ) -> IndexedEvidenceV1 {
        let retention = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class,
            terminal_at_ms,
            case_minimum_days,
            release_retain_until_ms: None,
            pinned: false,
        })
        .unwrap();
        let compact_record = String::from_utf8(
            canonical_json(&CompactEvidenceRecordV1 {
                schema_version: 1,
                evidence_id: id.into(),
                evidence_class,
                terminal_at_ms,
                affected_case_ids: vec!["case-1".into()],
                sidecar_sha256: digest('c'),
                aggregate_sha256: OptionalSha256V1::Absent,
                archive_sha256: digest('a'),
                manifest_sha256: digest('b'),
            })
            .unwrap(),
        )
        .unwrap();
        IndexedEvidenceV1 {
            evidence_id: id.into(),
            evidence_class,
            full_evidence_sha256: digest('a'),
            manifest_sha256: digest('b'),
            compact_record_sha256: local_file::sha256_hex(compact_record.as_bytes()),
            archive_bytes: bytes,
            manifest_bytes: 128,
            compact_record_bytes: compact_record.len() as u64,
            compact_record,
            hot_path: relative(&format!("{id}.tar.gz")),
            cold_path: OptionalRelativeEvidencePathV1::Absent,
            terminal_at_ms,
            case_minimum_days,
            full_retain_until_ms: retention.full_retain_until_ms,
            compact_retain_until_ms: retention.compact_retain_until_ms,
            hot_retain_until_ms: retention.hot_retain_until_ms,
            hot_present: true,
        }
    }

    fn model() -> EvidenceStateModelV1 {
        EvidenceStateModelV1::new(digest('c'), ColdStorageBindingV1::Absent).unwrap()
    }

    fn bundle_gc_action(action_id: &str, bundle_id: &str, started_at_ms: i64) -> BundleGcActionV1 {
        BundleGcActionV1 {
            action_id: action_id.into(),
            bundle_id: bundle_id.into(),
            evidence_id: "evidence-1".into(),
            provider_id: "provider-1".into(),
            case_id: "case-1".into(),
            evidence_class: EvidenceClassV1::RoutineGreen,
            cache_root_sha256: digest('1'),
            path: RelativeEvidencePathV1 {
                components: vec![format!("{bundle_id}.json")],
            },
            content_sha256: digest('2'),
            length_bytes: 512,
            preserved_in_full_evidence_sha256: digest('3'),
            reason_code: "retention_expired".into(),
            planned_at_ms: started_at_ms - 1,
            started_at_ms,
            lifecycle: BundleGcLifecycleV1::Pending,
        }
    }

    fn image_gc_action(action_id: &str, digest_char: char, started_at_ms: i64) -> ImageGcActionV1 {
        ImageGcActionV1 {
            action_id: action_id.into(),
            digest: format!("sha256:{}", digest(digest_char)),
            planned_inventory_sha256: digest('4'),
            planned_at_ms: started_at_ms - 1,
            started_at_ms,
            lifecycle: ImageGcLifecycleV1::Pending,
        }
    }

    fn publish_test_cold_copy(
        state: &mut EvidenceStateModelV1,
        evidence_id: &str,
        admitted_at_ms: i64,
    ) {
        let binding = state.cold_storage.clone();
        let ColdStorageBindingV1::OwnerIcloud {
            consent_id,
            consent_sha256,
            root_sha256,
            file_provider_domain_id,
        } = &binding
        else {
            panic!("test cold copy needs a cold binding")
        };
        let entry = state.entries[evidence_id].clone();
        let archive_path = relative(&format!("{evidence_id}.tar.gz"));
        let manifest_path = relative(&format!("{evidence_id}.manifest.json"));
        let copy_id = format!("cold-copy-{evidence_id}");
        state
            .admit_cold_copy(
                binding.clone(),
                ColdCopyRecordV1 {
                    copy_id: copy_id.clone(),
                    evidence_id: evidence_id.into(),
                    archive_sha256: entry.full_evidence_sha256,
                    archive_bytes: entry.archive_bytes,
                    manifest_sha256: entry.manifest_sha256,
                    manifest_bytes: entry.manifest_bytes,
                    consent_id: consent_id.clone(),
                    consent_sha256: consent_sha256.clone(),
                    consent_revocation_generation: 1,
                    cold_root_sha256: root_sha256.clone(),
                    file_provider_domain_id: file_provider_domain_id.clone(),
                    archive_path: archive_path.clone(),
                    manifest_path: manifest_path.clone(),
                    deadline_ms: admitted_at_ms + 100,
                    admitted_at_ms,
                    lifecycle: ColdCopyLifecycleV1::Admitted,
                },
            )
            .unwrap();
        let observation = |path: RelativeEvidencePathV1| FileProviderObservationV1 {
            cold_root_sha256: root_sha256.clone(),
            file_provider_domain_id: file_provider_domain_id.clone(),
            object_path: OptionalRelativeEvidencePathV1::RelativePath { value: path },
            state: FileProviderObjectStateV1::Known {
                materialization: FileProviderMaterializationV1::Materialized,
                synchronization: FileProviderSynchronizationV1::Synchronized,
            },
            observed_at_ms: admitted_at_ms + 1,
        };
        state
            .publish_cold_copy(
                &copy_id,
                observation(archive_path),
                observation(manifest_path),
                admitted_at_ms + 1,
            )
            .unwrap();
    }

    fn fingerprint(ch: char) -> FingerprintV1 {
        FingerprintV1 {
            schema_version: 1,
            sha256: digest(ch),
        }
    }

    fn sidecar_bytes(aggregate_sha256: Option<String>, schema_version: u16) -> Vec<u8> {
        let value = ScheduleEvidenceRecordV1 {
            schema_version,
            schedule_record_id: "schedule-1".into(),
            trigger: TriggerKindV1::Daily,
            source: ProfileSourceRefV1 {
                kind: ProfileSourceKindV1::ScheduledAdvisory,
                schema_version: 1,
                source_sha256: digest('1'),
                row_id: "case-1".into(),
                row_sha256: digest('2'),
            },
            profile_policy_bundle_sha256: digest('3'),
            characterization_profile: fingerprint('4'),
            case_execution: fingerprint('5'),
            admission_attempt: fingerprint('6'),
            authority: AdmissionAuthorityV1::StandingGrant(StandingGrantAuthorityV1 {
                grant_id: "grant-1".into(),
                generation: 1,
                grant_sha256: digest('7'),
                characterization_id: "characterization-1".into(),
                characterization_sha256: digest('8'),
            }),
            aggregate: aggregate_sha256.map_or(OptionalSha256V1::Absent, |value| {
                OptionalSha256V1::Sha256 { value }
            }),
            evidence_index_id: "owner-evidence-index".into(),
            check: CheckBindingV1::Absent,
            storage_consent: OptionalRecordRefV1::Absent,
            quarantine: OptionalRecordRefV1::Absent,
            characterization: OptionalRecordRefV1::Absent,
            window_id: "window-1".into(),
            attempt_idempotency_key: digest('9'),
            equivalent_work_key: digest('a'),
            consumption: OptionalRecordRefV1::Absent,
            repeat_nonce: OptionalStableIdV1::Absent,
            ledger_reservation_id: "reservation-1".into(),
            budget_reservation_sha256: digest('b'),
            ledger_reconciliation: OptionalSha256V1::Absent,
            deadline_derivation_sha256: digest('c'),
            preflight_results_sha256: digest('d'),
            admission_lock_holder_sha256: digest('e'),
            supervisor_record_sha256: digest('f'),
            freshness_observation_sha256: digest('0'),
            requested_identity: EffectiveIdentityV1 {
                model: "gpt-5.6-luna".into(),
                effort: OptionalTextV1::Text {
                    value: "low".into(),
                },
                mode: OptionalTextV1::Absent,
            },
            expected_effective_identity: EffectiveIdentityV1 {
                model: "gpt-5.6-luna".into(),
                effort: OptionalTextV1::Text {
                    value: "low".into(),
                },
                mode: OptionalTextV1::Absent,
            },
            observed_effective_identity: OptionalEffectiveIdentityV1::Absent,
            publication_outbox: OptionalRecordRefV1::Absent,
            status_publication: OptionalSha256V1::Absent,
            affected_case_ids: vec!["case-1".into()],
            created_at_ms: 1_000_000,
        };
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn aggregate_bytes() -> Vec<u8> {
        crate::compatibility::child_terminal_aggregate_fixture(
            &crate::compatibility::ChildTerminalAggregateFixtureV1 {
                case_id: "case-1".into(),
                candidate_sha256: digest('1'),
                candidate_length_bytes: 1,
                manifest_sha256: digest('2'),
                requested_model: "gpt-5.6-luna".into(),
                requested_effort: Some("low".into()),
                requested_mode: None,
                observed_model: "gpt-5.6-luna".into(),
                observed_effort: Some("low".into()),
                observed_mode: None,
                tokens: Some(1),
                cost_usd: Some(0.000_001),
                duration_ms: 1,
            },
        )
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn pin_source(path: &Path) -> local_file::PinnedDirectory {
        let snapshot = local_file::snapshot_directory(path, "test completed evidence").unwrap();
        local_file::PinnedDirectory::open(
            path,
            &snapshot.canonical_cwd,
            &snapshot.identity,
            "test completed evidence",
        )
        .unwrap()
    }

    fn seal_request() -> SealEvidenceRequestV1 {
        SealEvidenceRequestV1 {
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms: 1_000_001,
            case_minimum_days: 30,
            release_retain_until_ms: None,
        }
    }

    fn prepared_evidence() -> PreparedSealedEvidenceV1 {
        prepared_evidence_for_class(EvidenceClassV1::RoutineGreen)
    }

    fn prepared_evidence_for_class(evidence_class: EvidenceClassV1) -> PreparedSealedEvidenceV1 {
        let source = root();
        write_private(
            source.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(source.path().join("result.txt").as_path(), b"clean\n");
        let mut request = seal_request();
        request.evidence_class = evidence_class;
        prepare_sealed_evidence(&pin_source(source.path()), &request).unwrap()
    }

    fn test_hot_store() -> (tempfile::TempDir, EvidenceHotStoreV1) {
        let root = root();
        for name in ["state", "scratch", "sealed"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
            std::fs::set_permissions(
                root.path().join(name),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        let pinned = pin_source(root.path());
        let store = EvidenceHotStoreV1::open_existing(&pinned).unwrap();
        (root, store)
    }

    fn empty_hot_usage() -> HotStorageUsageV1 {
        HotStorageUsageV1 {
            state_bytes: 0,
            scratch_bytes: 0,
            sealed_bytes: 0,
        }
    }

    fn migration_item(
        incident_id: &str,
        source_path: &Path,
        bytes: &[u8],
    ) -> IncidentMigrationItemV1 {
        IncidentMigrationItemV1 {
            incident_id: incident_id.into(),
            incident_reference: format!("TEST-{}", incident_id.to_ascii_uppercase()),
            source_path: source_path.to_string_lossy().into_owned(),
            expected_mode: 0o600,
            expected_length_bytes: bytes.len() as u64,
            expected_sha256: local_file::sha256_hex(bytes),
            affected_case_ids: vec!["case-1".into()],
        }
    }

    #[test]
    fn retention_uses_the_longest_case_class_pin_and_release_clock() {
        let terminal = 1_000_000;
        let ordinary = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms: terminal,
            case_minimum_days: 45,
            release_retain_until_ms: None,
            pinned: false,
        })
        .unwrap();
        assert_eq!(ordinary.full_retain_until_ms, terminal + 45 * DAY_MS);
        assert_eq!(ordinary.compact_retain_until_ms, terminal + 180 * DAY_MS);
        assert_eq!(ordinary.hot_retain_until_ms, terminal + 14 * DAY_MS);

        let promotion = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::PromotionRelease,
            terminal_at_ms: terminal,
            case_minimum_days: 1,
            release_retain_until_ms: Some(terminal + 500 * DAY_MS),
            pinned: false,
        })
        .unwrap();
        assert_eq!(promotion.full_retain_until_ms, terminal + 500 * DAY_MS);
        assert_eq!(promotion.compact_retain_until_ms, i64::MAX);

        let pinned = decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::Incident,
            terminal_at_ms: terminal,
            case_minimum_days: 1,
            release_retain_until_ms: None,
            pinned: true,
        })
        .unwrap();
        assert_eq!(pinned.full_retain_until_ms, i64::MAX);
        assert_eq!(pinned.compact_retain_until_ms, i64::MAX);
        assert_eq!(pinned.hot_retain_until_ms, i64::MAX);
    }

    #[test]
    fn retention_rejects_overflow_and_missing_release_lifetime() {
        assert!(decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::RoutineGreen,
            terminal_at_ms: i64::MAX - DAY_MS,
            case_minimum_days: 30,
            release_retain_until_ms: None,
            pinned: false,
        })
        .is_err());
        assert!(decide_retention(&EvidenceRetentionRequestV1 {
            evidence_class: EvidenceClassV1::PromotionRelease,
            terminal_at_ms: 1,
            case_minimum_days: 0,
            release_retain_until_ms: None,
            pinned: false,
        })
        .is_err());
    }

    #[test]
    fn incident_unpin_starts_a_180_day_release_lifetime_without_partial_mutation() {
        let terminal = 1_000_000;
        let incident = entry_for_class("incident-1", terminal, 512, EvidenceClassV1::Incident, 0);

        let mut state = model();
        state.insert_entry(incident).unwrap();
        state
            .pin(EvidencePinV1 {
                pin_id: "pin-incident-1".into(),
                evidence_id: "incident-1".into(),
                reason: "incident investigation".into(),
                created_at_ms: terminal + 1,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();

        let released_at_ms = terminal + 200 * DAY_MS;
        state
            .unpin("pin-incident-1", "resolved", released_at_ms)
            .unwrap();
        let retained = state.entries.get("incident-1").unwrap();
        assert_eq!(retained.full_retain_until_ms, released_at_ms + 180 * DAY_MS);
        assert_eq!(retained.compact_retain_until_ms, i64::MAX);

        let mut overflow = model();
        let incident = entry_for_class("incident-2", terminal, 512, EvidenceClassV1::Incident, 0);
        overflow.insert_entry(incident).unwrap();
        overflow
            .pin(EvidencePinV1 {
                pin_id: "pin-incident-2".into(),
                evidence_id: "incident-2".into(),
                reason: "incident investigation".into(),
                created_at_ms: terminal + 1,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();
        assert!(overflow
            .unpin("pin-incident-2", "resolved", i64::MAX - DAY_MS)
            .is_err());
        assert_eq!(
            overflow.pins["pin-incident-2"].lifecycle,
            PinLifecycleV1::Active
        );
    }

    #[test]
    fn evidence_model_projects_pins_and_never_shortens_clocks() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        state
            .pin(EvidencePinV1 {
                pin_id: "pin-1".into(),
                evidence_id: "evidence-1".into(),
                reason: "incident investigation".into(),
                created_at_ms: 1_000_001,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();
        let previous = EvidenceStateSnapshotV1::first(state.clone(), 1_000_002).unwrap();
        let projected = previous.project_index().unwrap();
        assert!(projected.entries[0].pinned);

        state.unpin("pin-1", "resolved", 1_000_003).unwrap();
        state
            .entries
            .get_mut("evidence-1")
            .unwrap()
            .full_retain_until_ms -= 1;
        assert!(previous.successor(state, 1_000_004).is_err());
    }

    #[test]
    fn snapshot_projects_the_actual_journal_generation() {
        let first = EvidenceStateSnapshotV1::first(model(), 1_000_000).unwrap();
        assert_eq!(first.project_index().unwrap().generation, 1);

        let second = first.successor(model(), 1_000_001).unwrap();
        assert_eq!(second.project_index().unwrap().generation, 2);
    }

    #[test]
    fn successor_rejects_backdated_pin_and_tombstone_events() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        let first = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();

        let mut backdated_pin = state.clone();
        backdated_pin
            .pin(EvidencePinV1 {
                pin_id: "pin-1".into(),
                evidence_id: "evidence-1".into(),
                reason: "incident investigation".into(),
                created_at_ms: first.recorded_at_ms - 1,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();
        let next = first
            .successor(backdated_pin, first.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&first, &next).is_err());

        state
            .pin(EvidencePinV1 {
                pin_id: "pin-1".into(),
                evidence_id: "evidence-1".into(),
                reason: "incident investigation".into(),
                created_at_ms: first.recorded_at_ms - 2,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();
        let pinned = EvidenceStateSnapshotV1::first(state.clone(), first.recorded_at_ms).unwrap();
        state
            .unpin("pin-1", "resolved", pinned.recorded_at_ms - 1)
            .unwrap();
        let next = pinned.successor(state, pinned.recorded_at_ms + 1).unwrap();
        assert!(validate_evidence_state_transition(&pinned, &next).is_err());

        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        let first = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                first.recorded_at_ms - 1,
            )
            .unwrap();
        let next = first.successor(state, first.recorded_at_ms + 1).unwrap();
        assert!(validate_evidence_state_transition(&first, &next).is_err());

        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                19_999_999_998,
            )
            .unwrap();
        let pending = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                pending.recorded_at_ms - 1,
            ))
            .unwrap();
        let next = pending
            .successor(state, pending.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&pending, &next).is_err());
    }

    #[test]
    fn model_rejects_an_active_pin_combined_with_a_pending_tombstone() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                20_000_000_000,
            )
            .unwrap();
        state.pins.insert(
            "pin-1".into(),
            EvidencePinV1 {
                pin_id: "pin-1".into(),
                evidence_id: "evidence-1".into(),
                reason: "invalid late pin".into(),
                created_at_ms: 20_000_000_001,
                lifecycle: PinLifecycleV1::Active,
            },
        );

        assert!(state
            .validate()
            .unwrap_err()
            .to_string()
            .contains("active pin conflicts with pending tombstone"));
    }

    #[test]
    fn snapshot_rejects_future_pin_release_and_tombstone_completion() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        state
            .pin(EvidencePinV1 {
                pin_id: "pin-1".into(),
                evidence_id: "evidence-1".into(),
                reason: "incident investigation".into(),
                created_at_ms: 19_999_999_998,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();
        let previous = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .unpin("pin-1", "resolved", previous.recorded_at_ms + 2)
            .unwrap();
        assert!(previous
            .successor(state, previous.recorded_at_ms + 1)
            .is_err());

        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                19_999_999_998,
            )
            .unwrap();
        let previous = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                previous.recorded_at_ms + 2,
            ))
            .unwrap();
        assert!(previous
            .successor(state, previous.recorded_at_ms + 1)
            .is_err());
    }

    #[test]
    fn gc_transitions_require_a_durable_pending_generation_and_immutable_identity() {
        let first = EvidenceStateSnapshotV1::first(model(), 20_000_000_000).unwrap();

        let mut bundle_terminal = model();
        bundle_terminal
            .begin_bundle_gc(bundle_gc_action(
                "bundle-action-1",
                "bundle-1",
                first.recorded_at_ms + 1,
            ))
            .unwrap();
        bundle_terminal
            .complete_bundle_gc("bundle-action-1", first.recorded_at_ms + 2)
            .unwrap();
        let skipped_pending = first
            .successor(bundle_terminal, first.recorded_at_ms + 3)
            .unwrap();
        assert!(validate_evidence_state_transition(&first, &skipped_pending).is_err());

        let mut image_terminal = model();
        image_terminal
            .begin_image_gc(image_gc_action(
                "image-action-1",
                '5',
                first.recorded_at_ms + 1,
            ))
            .unwrap();
        image_terminal
            .safe_skip_image_gc(
                "image-action-1",
                "runtime_inventory_changed",
                first.recorded_at_ms + 2,
            )
            .unwrap();
        let skipped_pending = first
            .successor(image_terminal, first.recorded_at_ms + 3)
            .unwrap();
        assert!(validate_evidence_state_transition(&first, &skipped_pending).is_err());

        let mut pending_state = model();
        pending_state
            .begin_bundle_gc(bundle_gc_action(
                "bundle-action-2",
                "bundle-2",
                first.recorded_at_ms + 1,
            ))
            .unwrap();
        let pending = first
            .successor(pending_state.clone(), first.recorded_at_ms + 2)
            .unwrap();
        validate_evidence_state_transition(&first, &pending).unwrap();

        pending_state
            .bundle_gc_actions
            .get_mut("bundle-action-2")
            .unwrap()
            .provider_id = "provider-2".into();
        let changed = pending
            .successor(pending_state, pending.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&pending, &changed).is_err());
    }

    #[test]
    fn gc_transitions_reject_backdated_intents_and_terminal_events() {
        let first = EvidenceStateSnapshotV1::first(model(), 20_000_000_000).unwrap();

        let mut backdated_bundle = model();
        backdated_bundle
            .begin_bundle_gc(bundle_gc_action(
                "bundle-action-1",
                "bundle-1",
                first.recorded_at_ms,
            ))
            .unwrap();
        let next = first
            .successor(backdated_bundle, first.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&first, &next).is_err());

        let mut pending_state = model();
        pending_state
            .begin_image_gc(image_gc_action(
                "image-action-1",
                '5',
                first.recorded_at_ms + 1,
            ))
            .unwrap();
        let pending = first
            .successor(pending_state.clone(), first.recorded_at_ms + 2)
            .unwrap();
        validate_evidence_state_transition(&first, &pending).unwrap();
        pending_state
            .complete_image_gc("image-action-1", pending.recorded_at_ms)
            .unwrap();
        let next = pending
            .successor(pending_state, pending.recorded_at_ms + 1)
            .unwrap();
        assert!(validate_evidence_state_transition(&pending, &next).is_err());
    }

    #[test]
    fn gc_state_rejects_duplicate_pending_targets_and_future_terminal_events() {
        let mut duplicate = model();
        duplicate
            .begin_bundle_gc(bundle_gc_action("bundle-action-1", "bundle-1", 1_000_001))
            .unwrap();
        duplicate.bundle_gc_actions.insert(
            "bundle-action-2".into(),
            bundle_gc_action("bundle-action-2", "bundle-1", 1_000_002),
        );
        assert!(duplicate.validate().is_err());

        let mut future_bundle = model();
        future_bundle
            .begin_bundle_gc(bundle_gc_action("bundle-action-1", "bundle-1", 1_000_001))
            .unwrap();
        future_bundle
            .complete_bundle_gc("bundle-action-1", 1_000_003)
            .unwrap();
        assert!(EvidenceStateSnapshotV1::first(future_bundle, 1_000_002).is_err());

        let mut future_image = model();
        future_image
            .begin_image_gc(image_gc_action("image-action-1", '5', 1_000_001))
            .unwrap();
        future_image
            .safe_skip_image_gc("image-action-1", "runtime_error", 1_000_003)
            .unwrap();
        assert!(EvidenceStateSnapshotV1::first(future_image, 1_000_002).is_err());
    }

    #[test]
    fn gc_state_compacts_only_terminal_actions_and_keeps_the_journal_transition_valid() {
        let mut state = model();
        for index in 0..MAX_EVIDENCE_ITEMS * 4 {
            let started_at_ms = 1_000_000 + index as i64 * 2;
            let mut bundle = bundle_gc_action(
                &format!("bundle-action-{index}"),
                &format!("bundle-{index}"),
                started_at_ms,
            );
            bundle.lifecycle = BundleGcLifecycleV1::Unlinked {
                unlinked_at_ms: started_at_ms + 1,
            };
            state
                .bundle_gc_actions
                .insert(bundle.action_id.clone(), bundle);

            let mut image = ImageGcActionV1 {
                action_id: format!("image-action-{index}"),
                digest: format!("sha256:{index:064x}"),
                planned_inventory_sha256: digest('4'),
                planned_at_ms: started_at_ms - 1,
                started_at_ms,
                lifecycle: ImageGcLifecycleV1::Pending,
            };
            image.lifecycle = ImageGcLifecycleV1::Removed {
                removed_at_ms: started_at_ms + 1,
            };
            state
                .image_gc_actions
                .insert(image.action_id.clone(), image);
        }
        state.validate().unwrap();
        let previous = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();

        state
            .begin_bundle_gc(bundle_gc_action(
                "bundle-action-new",
                "bundle-new",
                previous.recorded_at_ms + 1,
            ))
            .unwrap();
        state
            .begin_image_gc(image_gc_action(
                "image-action-new",
                'f',
                previous.recorded_at_ms + 1,
            ))
            .unwrap();
        state
            .begin_bundle_gc(bundle_gc_action(
                "bundle-action-newer",
                "bundle-newer",
                previous.recorded_at_ms + 2,
            ))
            .unwrap();
        state
            .begin_image_gc(image_gc_action(
                "image-action-newer",
                'e',
                previous.recorded_at_ms + 2,
            ))
            .unwrap();
        assert_eq!(state.bundle_gc_actions.len(), MAX_EVIDENCE_ITEMS * 4);
        assert_eq!(state.image_gc_actions.len(), MAX_EVIDENCE_ITEMS * 4);
        assert!(!state.bundle_gc_actions.contains_key("bundle-action-0"));
        assert!(!state.bundle_gc_actions.contains_key("bundle-action-1"));
        assert!(!state.image_gc_actions.contains_key("image-action-0"));
        assert!(!state.image_gc_actions.contains_key("image-action-1"));
        assert!(state.bundle_gc_actions.contains_key("bundle-action-new"));
        assert!(state.bundle_gc_actions.contains_key("bundle-action-newer"));
        assert!(state.image_gc_actions.contains_key("image-action-new"));
        assert!(state.image_gc_actions.contains_key("image-action-newer"));

        let next = previous
            .successor(state, previous.recorded_at_ms + 2)
            .unwrap();
        validate_evidence_state_transition(&previous, &next).unwrap();
    }

    #[test]
    fn transition_requires_a_durable_pending_tombstone_generation() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        let previous = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                previous.recorded_at_ms + 1,
            )
            .unwrap();
        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                previous.recorded_at_ms + 2,
            ))
            .unwrap();
        let next = previous
            .successor(state, previous.recorded_at_ms + 3)
            .unwrap();
        assert!(validate_evidence_state_transition(&previous, &next).is_err());
    }

    #[test]
    fn tombstone_is_durable_before_entry_removal_and_is_monotonic() {
        let mut state = model();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        let first = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        state
            .begin_tombstone(
                "tombstone-1",
                "evidence-1",
                "retention_expired",
                20_000_000_001,
            )
            .unwrap();
        let pending = first.successor(state.clone(), 20_000_000_002).unwrap();
        validate_evidence_state_transition(&first, &pending).unwrap();
        assert!(state.entries.contains_key("evidence-1"));

        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                20_000_000_003,
            ))
            .unwrap();
        let complete = pending.successor(state.clone(), 20_000_000_004).unwrap();
        validate_evidence_state_transition(&pending, &complete).unwrap();
        assert!(!state.entries.contains_key("evidence-1"));

        let mut resurrected = state;
        resurrected
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap_err();
    }

    #[test]
    fn cold_copy_does_not_permit_retirement_before_the_full_evidence_clock() {
        let terminal = 1_000_000;
        let mut state = EvidenceStateModelV1::new(
            digest('c'),
            ColdStorageBindingV1::OwnerIcloud {
                consent_id: "consent-1".into(),
                consent_sha256: digest('d'),
                root_sha256: digest('e'),
                file_provider_domain_id: "owner-icloud-domain".into(),
            },
        )
        .unwrap();
        let evidence = entry("evidence-1", terminal, 512);
        let full_retain_until_ms = evidence.full_retain_until_ms;
        state.insert_entry(evidence).unwrap();
        publish_test_cold_copy(&mut state, "evidence-1", terminal + 1);

        assert!(state
            .begin_tombstone(
                "tombstone-early",
                "evidence-1",
                "quota_gc",
                full_retain_until_ms - 1,
            )
            .is_err());
        assert!(state.entries.contains_key("evidence-1"));
        assert!(state.tombstones.is_empty());
    }

    #[test]
    fn evidence_journal_reopens_a_contiguous_owner_private_hash_chain() {
        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-journal")
            .unwrap();
        let mut state = model();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1).unwrap();
        state
            .insert_entry(entry("evidence-1", 1_000_000, 512))
            .unwrap();
        opened.journal.append(&state, 2_000_000).unwrap();
        drop(opened);

        let reopened = FileEvidenceJournal::open_existing(&lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 2);
        assert!(reopened.snapshot.state.entries.contains_key("evidence-1"));
        assert_eq!(
            std::fs::metadata(
                root.path()
                    .join("evidence-index/evidence-state.00000000000000000002.json")
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn evidence_interruption_before_atomic_publish_reopens_and_retries_same_generation() {
        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-interrupted-publication")
            .unwrap();
        let evidence_directory = lock.evidence_index_directory();
        let temporary = evidence_directory
            .canonical_path()
            .join(".a2a-journal-append-v1.tmp");
        let final_record = evidence_directory
            .canonical_path()
            .join(FileEvidenceJournal::generation_name(2));
        let mut state = model();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1).unwrap();
        state
            .insert_entry(entry("evidence-interrupted", 1_000_000, 512))
            .unwrap();

        evidence_directory.fail_journal_publish_on_nth_call_for_test(1);
        let error = opened.journal.append(&state, 2_000_000).unwrap_err();
        assert!(error
            .to_string()
            .contains("injected failure before journal publication"));
        assert!(temporary.is_file());
        assert!(!final_record.exists());
        drop(opened);

        let mut reopened = FileEvidenceJournal::open_existing(&lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 1);
        reopened.journal.append(&state, 2_000_000).unwrap();
        assert!(!temporary.exists());
        assert!(final_record.is_file());
        assert_eq!(
            FileEvidenceJournal::open_existing(&lock)
                .unwrap()
                .snapshot
                .generation,
            2
        );
    }

    #[test]
    fn evidence_journal_rejects_gap_corruption_and_same_path_replacement() {
        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-corrupt")
            .unwrap();
        let state = model();
        FileEvidenceJournal::initialize(&lock, &state, 1).unwrap();
        std::fs::write(
            root.path()
                .join("evidence-index/evidence-state.00000000000000000003.json"),
            b"{}\n",
        )
        .unwrap();
        std::fs::set_permissions(
            root.path()
                .join("evidence-index/evidence-state.00000000000000000003.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(FileEvidenceJournal::open_existing(&lock).is_err());

        let moved = root.path().with_extension("moved");
        std::fs::rename(root.path().join("evidence-index"), &moved).unwrap();
        std::fs::create_dir(root.path().join("evidence-index")).unwrap();
        std::fs::set_permissions(
            root.path().join("evidence-index"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        assert!(FileEvidenceJournal::open_existing(&lock).is_err());
    }

    #[test]
    fn shared_reader_blocks_exclusive_gc_lease() {
        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-lease")
            .unwrap();
        let reader = acquire_evidence_read_lease(&lock, "evidence-1").unwrap();
        assert!(try_acquire_evidence_gc_lease(&lock, "evidence-1").is_err());
        drop(reader);
        let exclusive = try_acquire_evidence_gc_lease(&lock, "evidence-1").unwrap();
        assert!(acquire_evidence_read_lease(&lock, "evidence-1").is_err());
        drop(exclusive);
    }

    #[test]
    fn quotas_enforce_each_allocation_and_the_total() {
        let caps = HotStorageCapsV1::approved();
        let usage = HotStorageUsageV1 {
            state_bytes: caps.state_bytes - 1,
            scratch_bytes: 0,
            sealed_bytes: 0,
        };
        assert!(reserve_hot_bytes(&caps, &usage, HotAllocationV1::State, 1).is_ok());
        assert!(reserve_hot_bytes(&caps, &usage, HotAllocationV1::State, 2).is_err());

        let total_pressure = HotStorageUsageV1 {
            state_bytes: caps.state_bytes,
            scratch_bytes: caps.scratch_bytes,
            sealed_bytes: caps.sealed_bytes,
        };
        assert!(reserve_hot_bytes(&caps, &total_pressure, HotAllocationV1::Sealed, 1).is_err());

        let already_over_cap = HotStorageUsageV1 {
            state_bytes: caps.state_bytes + 1,
            scratch_bytes: 0,
            sealed_bytes: 0,
        };
        assert!(reserve_hot_bytes(&caps, &already_over_cap, HotAllocationV1::Sealed, 1).is_err());

        assert_eq!(
            reserve_state_journal_bytes(HOT_STATE_CAP_BYTES - 1, 1).unwrap(),
            HOT_STATE_CAP_BYTES
        );
        assert!(reserve_state_journal_bytes(HOT_STATE_CAP_BYTES, 1).is_err());
        assert!(reserve_state_journal_bytes(u64::MAX, 1).is_err());
    }

    #[test]
    fn evidence_journal_enforces_the_aggregate_state_cap_before_create() {
        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-journal-cap")
            .unwrap();
        let state = model();
        let opened = FileEvidenceJournal::initialize(&lock, &state, 1).unwrap();

        for generation in 100..164 {
            let file = lock
                .evidence_index_directory()
                .create_new_file(
                    OsStr::new(&FileEvidenceJournal::generation_name(generation)),
                    STATE_FILE_MODE,
                    "test oversized evidence journal",
                )
                .unwrap();
            file.set_len(MAX_STATE_RECORD_BYTES).unwrap();
        }
        let successor = opened.snapshot.successor(state, 2).unwrap();
        assert!(opened.journal.persist(&successor).is_err());
        assert!(!root
            .path()
            .join("evidence-index/evidence-state.00000000000000000002.json")
            .exists());
    }

    #[test]
    fn evidence_journal_refuses_a_generation_beyond_its_reopen_bound() {
        let fixture = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(fixture.path()).unwrap();
        let owner = scheduler
            .try_owner_admission("test/evidence-generation-bound")
            .unwrap();
        let state = model();
        let opened = FileEvidenceJournal::initialize(&owner, &state, 1).unwrap();
        let over_bound = EvidenceStateSnapshotV1 {
            schema_version: 1,
            generation: u64::try_from(MAX_STATE_GENERATIONS).unwrap() + 1,
            previous_record: OptionalSha256V1::Sha256 { value: digest('f') },
            recorded_at_ms: 2,
            state,
        };

        assert!(opened.journal.persist(&over_bound).is_err());
        assert!(!owner
            .evidence_index_directory()
            .canonical_path()
            .join(FileEvidenceJournal::generation_name(over_bound.generation))
            .exists());
    }

    #[test]
    fn quota_gc_selects_only_eligible_unpinned_oldest_evidence() {
        let now = 90 * DAY_MS;
        let mut state = EvidenceStateModelV1::new(
            digest('c'),
            ColdStorageBindingV1::OwnerIcloud {
                consent_id: "consent-1".into(),
                consent_sha256: digest('d'),
                root_sha256: digest('e'),
                file_provider_domain_id: "owner-icloud-domain".into(),
            },
        )
        .unwrap();
        let oldest = entry("oldest", 1, 300);
        let pinned = entry("pinned", 2, 400);
        let fresh = entry("fresh", now, 1_000);
        state.insert_entry(oldest).unwrap();
        state.insert_entry(pinned).unwrap();
        state.insert_entry(fresh).unwrap();
        publish_test_cold_copy(&mut state, "oldest", now + 1);
        publish_test_cold_copy(&mut state, "pinned", now + 3);
        publish_test_cold_copy(&mut state, "fresh", now + 5);
        state
            .pin(EvidencePinV1 {
                pin_id: "pin-pinned".into(),
                evidence_id: "pinned".into(),
                reason: "active incident".into(),
                created_at_ms: 3,
                lifecycle: PinLifecycleV1::Active,
            })
            .unwrap();

        let selected = plan_hot_evictions(&state, now, 350).unwrap();
        assert_eq!(selected, vec!["oldest"]);
        assert!(plan_hot_evictions(&state, now, 450).is_err());
        assert!(plan_hot_evictions(&state, now, 500).is_err());
    }

    #[test]
    fn index_rejects_portable_path_collision() {
        let mut state = model();
        state.insert_entry(entry("evidence-1", 1, 100)).unwrap();
        let mut collision = entry("evidence-2", 2, 100);
        collision.hot_path = RelativeEvidencePathV1 {
            components: vec!["SEALED".into(), "EVIDENCE-1.TAR.GZ".into()],
        };
        assert!(state.insert_entry(collision).is_err());
    }

    #[test]
    fn completed_tombstone_keeps_historical_identity() {
        let mut state = model();
        state.insert_entry(entry("evidence-1", 1, 100)).unwrap();
        state
            .begin_tombstone("tombstone-1", "evidence-1", "quota_gc", 20_000_000_000)
            .unwrap();
        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                20_000_000_001,
            ))
            .unwrap();
        let tombstone = state.tombstones.get("tombstone-1").unwrap();
        assert_eq!(tombstone.evidence_id, "evidence-1");
        assert_eq!(tombstone.evidence_class, EvidenceClassV1::RoutineGreen);
        assert_eq!(tombstone.full_evidence_sha256, digest('a'));
        assert_eq!(tombstone.manifest_sha256, digest('b'));
        assert_eq!(
            tombstone.compact_record_sha256,
            local_file::sha256_hex(tombstone.compact_record.as_bytes())
        );
        assert_eq!(
            tombstone.compact_record_bytes,
            tombstone.compact_record.len() as u64
        );
        assert_eq!(tombstone.cold_path, OptionalRelativeEvidencePathV1::Absent);
        assert_eq!(tombstone.terminal_at_ms, 1);
        assert_eq!(tombstone.compact_retain_until_ms, 1 + 180 * DAY_MS);
        assert!(matches!(
            tombstone.lifecycle,
            TombstoneLifecycleV1::FullEvidenceUnlinked { .. }
        ));
        assert_eq!(
            state.retired_evidence_ids,
            BTreeSet::from(["evidence-1".into()])
        );
    }

    #[test]
    fn compact_record_identity_is_bound_to_live_and_tombstoned_evidence() {
        let mut live = entry("evidence-1", 1, 100);
        let mut compact: CompactEvidenceRecordV1 =
            serde_json::from_str(&live.compact_record).unwrap();
        compact.evidence_id = "different-evidence".into();
        live.compact_record = String::from_utf8(canonical_json(&compact).unwrap()).unwrap();
        live.compact_record_bytes = live.compact_record.len() as u64;
        live.compact_record_sha256 = local_file::sha256_hex(live.compact_record.as_bytes());
        assert!(model().insert_entry(live).is_err());

        let mut state = model();
        state.insert_entry(entry("evidence-1", 1, 100)).unwrap();
        state
            .begin_tombstone("tombstone-1", "evidence-1", "quota_gc", 20_000_000_000)
            .unwrap();
        state
            .complete_tombstone(FullEvidenceUnlinkProofV1::for_test(
                "tombstone-1",
                "evidence-1",
                20_000_000_001,
            ))
            .unwrap();
        let tombstone = state.tombstones.get_mut("tombstone-1").unwrap();
        let mut compact: CompactEvidenceRecordV1 =
            serde_json::from_str(&tombstone.compact_record).unwrap();
        compact.evidence_id = "different-evidence".into();
        tombstone.compact_record = String::from_utf8(canonical_json(&compact).unwrap()).unwrap();
        tombstone.compact_record_bytes = tombstone.compact_record.len() as u64;
        tombstone.compact_record_sha256 =
            local_file::sha256_hex(tombstone.compact_record.as_bytes());
        assert!(state.validate().is_err());
    }

    #[test]
    fn sealing_is_deterministic_and_preserves_the_exact_aggregate_bytes() {
        let root = root();
        let aggregate = aggregate_bytes();
        write_private(
            root.path().join("pinned-aggregate.json").as_path(),
            &aggregate,
        );
        write_private(
            root.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(Some(local_file::sha256_hex(&aggregate)), 1),
        );
        std::fs::create_dir(root.path().join("diagnostics")).unwrap();
        std::fs::set_permissions(
            root.path().join("diagnostics"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_private(
            root.path().join("diagnostics/result.txt").as_path(),
            b"bounded diagnostic\n",
        );
        let source = pin_source(root.path());

        let first = prepare_sealed_evidence(&source, &seal_request()).unwrap();
        let second = prepare_sealed_evidence(&source, &seal_request()).unwrap();
        assert_eq!(first.archive, second.archive);
        assert_eq!(first.archive_sha256, second.archive_sha256);
        assert_eq!(first.manifest, second.manifest);
        assert_eq!(first.compact_record, second.compact_record);
        assert_eq!(first.evidence_id, "schedule-1");
        assert_eq!(
            first.aggregate_sha256,
            OptionalSha256V1::Sha256 {
                value: local_file::sha256_hex(&aggregate)
            }
        );

        let decoder = flate2::read::GzDecoder::new(first.archive.as_slice());
        let mut archive = tar::Archive::new(decoder);
        let mut archived_aggregate = None;
        for item in archive.entries().unwrap() {
            let mut item = item.unwrap();
            if item.path().unwrap().as_ref() == Path::new("pinned-aggregate.json") {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut item, &mut bytes).unwrap();
                archived_aggregate = Some(bytes);
            }
        }
        assert_eq!(archived_aggregate.as_deref(), Some(aggregate.as_slice()));
    }

    #[test]
    fn sealing_requires_one_strict_sidecar_and_an_exact_optional_aggregate() {
        let missing = root();
        write_private(missing.path().join("result.txt").as_path(), b"clean\n");
        assert!(prepare_sealed_evidence(&pin_source(missing.path()), &seal_request()).is_err());

        let unknown = root();
        write_private(
            unknown.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 2),
        );
        assert!(prepare_sealed_evidence(&pin_source(unknown.path()), &seal_request()).is_err());

        let aggregate = aggregate_bytes();
        let mismatch = root();
        write_private(
            mismatch.path().join("pinned-aggregate.json").as_path(),
            &aggregate,
        );
        write_private(
            mismatch.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(Some(digest('f')), 1),
        );
        assert!(prepare_sealed_evidence(&pin_source(mismatch.path()), &seal_request()).is_err());

        let unexpected = root();
        write_private(
            unexpected.path().join("pinned-aggregate.json").as_path(),
            &aggregate,
        );
        write_private(
            unexpected.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        assert!(prepare_sealed_evidence(&pin_source(unexpected.path()), &seal_request()).is_err());
        crate::compatibility::validate_child_aggregate_bytes(&aggregate).unwrap();
    }

    #[test]
    fn sealing_rejects_secret_unsafe_or_nonportable_source_entries() {
        let secret = root();
        write_private(
            secret.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(
            secret.path().join("diagnostic.txt").as_path(),
            b"token=opaque-secret-value\n",
        );
        assert!(prepare_sealed_evidence(&pin_source(secret.path()), &seal_request()).is_err());

        let decoded = root();
        write_private(
            decoded.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(
            decoded.path().join("diagnostic.json").as_path(),
            br#"{"password":""}"#,
        );
        assert!(prepare_sealed_evidence(&pin_source(decoded.path()), &seal_request()).is_err());

        let named_secret = root();
        write_private(
            named_secret.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(
            named_secret
                .path()
                .join("token=opaque-secret-value.txt")
                .as_path(),
            b"clean\n",
        );
        assert!(
            prepare_sealed_evidence(&pin_source(named_secret.path()), &seal_request()).is_err()
        );

        let mut portable_paths = BTreeSet::new();
        admit_portable_source_path(&mut portable_paths, &["A.txt".into()]).unwrap();
        assert!(admit_portable_source_path(&mut portable_paths, &["a.txt".into()]).is_err());

        let broadened = root();
        write_private(
            broadened.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(broadened.path().join("wide.txt").as_path(), b"wide");
        std::fs::set_permissions(
            broadened.path().join("wide.txt"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(prepare_sealed_evidence(&pin_source(broadened.path()), &seal_request()).is_err());

        let linked = root();
        write_private(
            linked.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(linked.path().join("target.txt").as_path(), b"target");
        std::fs::hard_link(
            linked.path().join("target.txt"),
            linked.path().join("hard.txt"),
        )
        .unwrap();
        assert!(prepare_sealed_evidence(&pin_source(linked.path()), &seal_request()).is_err());
    }

    #[test]
    fn sealing_rejects_symlinks_special_files_broadened_directories_and_duplicate_sidecars() {
        let symlinked = root();
        write_private(
            symlinked.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        std::os::unix::fs::symlink(
            symlinked.path().join("schedule-sidecar.json"),
            symlinked.path().join("sidecar-link.json"),
        )
        .unwrap();
        assert!(prepare_sealed_evidence(&pin_source(symlinked.path()), &seal_request()).is_err());

        let special = root();
        write_private(
            special.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        let _socket =
            std::os::unix::net::UnixListener::bind(special.path().join("diagnostic.sock")).unwrap();
        assert!(prepare_sealed_evidence(&pin_source(special.path()), &seal_request()).is_err());

        let broadened = root();
        write_private(
            broadened.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        std::fs::create_dir(broadened.path().join("diagnostics")).unwrap();
        std::fs::set_permissions(
            broadened.path().join("diagnostics"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(prepare_sealed_evidence(&pin_source(broadened.path()), &seal_request()).is_err());

        let duplicate = root();
        write_private(
            duplicate.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        std::fs::create_dir(duplicate.path().join("nested")).unwrap();
        std::fs::set_permissions(
            duplicate.path().join("nested"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_private(
            duplicate
                .path()
                .join("nested/schedule-sidecar.json")
                .as_path(),
            &sidecar_bytes(None, 1),
        );
        assert!(prepare_sealed_evidence(&pin_source(duplicate.path()), &seal_request()).is_err());
    }

    #[test]
    fn sealing_rejects_replacement_during_walk_and_bounded_overflow() {
        let replaced = root();
        write_private(
            replaced.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(replaced.path().join("result.txt").as_path(), b"before");
        let source = pin_source(replaced.path());
        assert!(prepare_sealed_evidence_with_hook(
            &source,
            &seal_request(),
            &SealLimitsV1::approved(),
            || {
                std::fs::rename(
                    replaced.path().join("result.txt"),
                    replaced.path().join("result.old"),
                )
                .unwrap();
                write_private(replaced.path().join("result.txt").as_path(), b"after");
            },
        )
        .is_err());

        let bounded = root();
        write_private(
            bounded.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        write_private(bounded.path().join("one.txt").as_path(), b"1");
        let limits = SealLimitsV1 {
            max_entries: 1,
            max_file_bytes: 1024,
            max_total_bytes: 1024,
        };
        assert!(prepare_sealed_evidence_with_hook(
            &pin_source(bounded.path()),
            &seal_request(),
            &limits,
            || {},
        )
        .is_err());

        let exact = root();
        write_private(
            exact.path().join("schedule-sidecar.json").as_path(),
            &sidecar_bytes(None, 1),
        );
        let limits = SealLimitsV1 {
            max_entries: 1,
            max_file_bytes: MAX_SEAL_FILE_BYTES,
            max_total_bytes: MAX_SEAL_TOTAL_BYTES,
        };
        assert!(prepare_sealed_evidence_with_hook(
            &pin_source(exact.path()),
            &seal_request(),
            &limits,
            || {},
        )
        .is_ok());

        let excessive_metadata = root();
        let mut sidecar: ScheduleEvidenceRecordV1 =
            serde_json::from_slice(&sidecar_bytes(None, 1)).unwrap();
        sidecar.affected_case_ids = (0..=MAX_EVIDENCE_ITEMS)
            .map(|index| format!("case-{index}"))
            .collect();
        let mut sidecar = serde_json::to_vec(&sidecar).unwrap();
        sidecar.push(b'\n');
        write_private(
            excessive_metadata
                .path()
                .join("schedule-sidecar.json")
                .as_path(),
            &sidecar,
        );
        assert!(
            prepare_sealed_evidence(&pin_source(excessive_metadata.path()), &seal_request(),)
                .is_err()
        );
    }

    #[test]
    fn publication_makes_sealed_bytes_visible_only_with_the_index_generation() {
        let prepared = prepared_evidence();
        let (hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-publication")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();

        let published = publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(published.snapshot.generation, 2);
        assert!(!published.scratch_cleanup_required);
        assert_eq!(
            published.snapshot_sha256,
            evidence_state_snapshot_sha256(&published.snapshot).unwrap()
        );
        assert_eq!(published.usage.scratch_bytes, 0);
        assert_eq!(
            published.usage.sealed_bytes,
            (prepared.archive.len() + prepared.manifest.len()) as u64
        );
        assert_eq!(
            published.usage.state_bytes,
            std::fs::read_dir(state_root.path().join("evidence-index"))
                .unwrap()
                .filter_map(|entry| {
                    let entry = entry.unwrap();
                    let name = entry.file_name();
                    let name = name.to_str().unwrap();
                    (name.starts_with("evidence-state.") && name.ends_with(".json"))
                        .then(|| entry.metadata().unwrap().len())
                })
                .sum::<u64>()
        );
        let indexed = state.entries.get("schedule-1").unwrap();
        assert_eq!(indexed.full_evidence_sha256, prepared.archive_sha256);
        assert_eq!(indexed.manifest_sha256, prepared.manifest_sha256);
        assert_eq!(
            indexed.compact_record,
            String::from_utf8(prepared.compact_record.clone()).unwrap()
        );
        assert_eq!(indexed.hot_path, published.hot_path);

        let object = published.hot_path.components.last().unwrap();
        for (name, expected) in [
            ("evidence.tar.gz", prepared.archive.as_slice()),
            ("manifest.json", prepared.manifest.as_slice()),
        ] {
            let path = hot_root.path().join("sealed").join(object).join(name);
            assert_eq!(std::fs::read(&path).unwrap(), expected);
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(std::fs::read_dir(hot_root.path().join("scratch"))
            .unwrap()
            .next()
            .is_none());
        let reopened = FileEvidenceJournal::open_existing(&lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 2);
        assert!(reopened.snapshot.state.entries.contains_key("schedule-1"));
        let repeated = publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_003,
            SealPublicationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(repeated.snapshot.generation, 2);
        assert!(inspect_unindexed_evidence(&store, &state)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn publication_crash_points_leave_unindexed_recoverable_payloads() {
        let prepared = prepared_evidence();
        for failpoint in [
            SealPublicationFailpointV1::AfterScratchArchive,
            SealPublicationFailpointV1::AfterSealedArchive,
            SealPublicationFailpointV1::AfterSealed,
        ] {
            let (hot_root, store) = test_hot_store();
            let state_root = root();
            let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
            let lock = scheduler
                .try_owner_admission("test/evidence-publication-crash")
                .unwrap();
            let mut state = EvidenceStateModelV1::new(
                store.root_sha256().to_owned(),
                ColdStorageBindingV1::Absent,
            )
            .unwrap();
            let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
            assert!(publish_prepared_evidence(
                &store,
                &mut opened.journal,
                &mut state,
                &prepared,
                &HotStorageCapsV1::approved(),
                1_000_002,
                failpoint,
            )
            .is_err());
            assert!(state.entries.is_empty());
            assert_eq!(
                FileEvidenceJournal::open_existing(&lock)
                    .unwrap()
                    .snapshot
                    .generation,
                1
            );
            let residue = inspect_unindexed_evidence(&store, &state).unwrap();
            match failpoint {
                SealPublicationFailpointV1::AfterScratchArchive => {
                    assert_eq!(residue.scratch.len(), 1);
                    assert!(residue.sealed.is_empty());
                }
                SealPublicationFailpointV1::AfterSealedArchive => {
                    assert_eq!(residue.scratch.len(), 1);
                    assert_eq!(residue.sealed.len(), 1);
                    let sealed = hot_root.path().join("sealed").join(&residue.sealed[0]);
                    assert!(sealed.join("evidence.tar.gz").is_file());
                    assert!(!sealed.join("manifest.json").exists());
                }
                SealPublicationFailpointV1::AfterSealed => {
                    assert_eq!(residue.scratch.len(), 1);
                    assert_eq!(residue.sealed.len(), 1);
                    let sealed = hot_root.path().join("sealed").join(&residue.sealed[0]);
                    assert!(sealed.join("evidence.tar.gz").is_file());
                    assert!(sealed.join("manifest.json").is_file());
                }
                SealPublicationFailpointV1::None
                | SealPublicationFailpointV1::AfterIndexPublication => unreachable!(),
            }

            let recovered = publish_prepared_evidence(
                &store,
                &mut opened.journal,
                &mut state,
                &prepared,
                &HotStorageCapsV1::approved(),
                1_000_003,
                SealPublicationFailpointV1::None,
            )
            .unwrap();
            assert_eq!(recovered.snapshot.generation, 2);
            assert!(state.entries.contains_key("schedule-1"));
            assert!(inspect_unindexed_evidence(&store, &state)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn crash_after_index_publication_leaves_visible_sealed_evidence_and_scratch_residue() {
        let prepared = prepared_evidence();
        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-publication-post-index-crash")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();

        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::AfterIndexPublication,
        )
        .is_err());
        let reopened = FileEvidenceJournal::open_existing(&lock).unwrap();
        assert_eq!(reopened.snapshot.generation, 2);
        assert!(reopened.snapshot.state.entries.contains_key("schedule-1"));
        let residue = inspect_unindexed_evidence(&store, &reopened.snapshot.state).unwrap();
        assert_eq!(residue.scratch.len(), 1);
        assert!(residue.sealed.is_empty());

        let mut recovered_state = reopened.snapshot.state.clone();
        let mut recovered_journal = reopened.journal;
        let recovered = publish_prepared_evidence(
            &store,
            &mut recovered_journal,
            &mut recovered_state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_003,
            SealPublicationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(recovered.snapshot.generation, 2);
        assert!(inspect_unindexed_evidence(&store, &recovered_state)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn approved_hot_storage_caps_cannot_be_enlarged_by_a_caller() {
        let mut caps = HotStorageCapsV1::approved();
        caps.sealed_bytes += 1;
        caps.total_bytes += 1;

        assert!(caps.validate().is_err());
        assert!(HotStorageCapsV1::reduced_for_test(
            HOT_TOTAL_CAP_BYTES + 1,
            HOT_STATE_CAP_BYTES,
            HOT_SCRATCH_CAP_BYTES,
            HOT_SEALED_CAP_BYTES + 1,
        )
        .validate()
        .is_err());
    }

    #[test]
    fn publication_measures_unindexed_residue_before_enforcing_the_live_cap() {
        let prepared = prepared_evidence();
        let (hot_root, store) = test_hot_store();
        let residue = hot_root.path().join("sealed/crash-residue");
        std::fs::create_dir(&residue).unwrap();
        std::fs::set_permissions(&residue, std::fs::Permissions::from_mode(0o700)).unwrap();
        write_private(&residue.join("evidence.tar.gz"), b"residue\n");

        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-live-cap-residue")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let new_sealed_bytes = (prepared.archive.len() + prepared.manifest.len()) as u64;
        let residue_bytes = b"residue\n".len() as u64;
        let caps = HotStorageCapsV1::reduced_for_test(
            HOT_STATE_CAP_BYTES + new_sealed_bytes + new_sealed_bytes + residue_bytes - 1,
            HOT_STATE_CAP_BYTES,
            new_sealed_bytes,
            new_sealed_bytes + residue_bytes - 1,
        );

        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &caps,
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert!(state.entries.is_empty());
        assert!(residue.exists());
    }

    #[test]
    fn publication_refuses_tampering_quota_pressure_and_index_append_failure() {
        let prepared = prepared_evidence();
        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-publication-refusal")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();

        let mut tampered = prepared.clone();
        tampered.archive.push(0);
        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &tampered,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert!(inspect_unindexed_evidence(&store, &state)
            .unwrap()
            .is_empty());

        let caps = HotStorageCapsV1::reduced_for_test(
            HOT_STATE_CAP_BYTES + 1 + HOT_SEALED_CAP_BYTES,
            HOT_STATE_CAP_BYTES,
            1,
            HOT_SEALED_CAP_BYTES,
        );
        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &caps,
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert!(inspect_unindexed_evidence(&store, &state)
            .unwrap()
            .is_empty());

        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_001,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert!(inspect_unindexed_evidence(&store, &state)
            .unwrap()
            .is_empty());

        write_private(
            state_root
                .path()
                .join("evidence-index/evidence-state.00000000000000000002.json")
                .as_path(),
            b"{}\n",
        );
        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert!(state.entries.is_empty());
        let residue = inspect_unindexed_evidence(&store, &state).unwrap();
        assert!(residue.is_empty());
    }

    #[test]
    fn publication_accounts_the_full_appended_state_generation() {
        let prepared = prepared_evidence();
        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-publication-state-quota")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let existing_state_bytes = std::fs::read_dir(state_root.path().join("evidence-index"))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        let compact_bytes = prepared.compact_record.len() as u64;
        let sealed_bytes = (prepared.archive.len() + prepared.manifest.len()) as u64;
        let state_cap = existing_state_bytes + compact_bytes;
        let caps = HotStorageCapsV1::reduced_for_test(
            state_cap + sealed_bytes * 2,
            state_cap,
            sealed_bytes,
            sealed_bytes,
        );
        assert!(publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &caps,
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .is_err());
        assert_eq!(
            FileEvidenceJournal::open_existing(&lock)
                .unwrap()
                .snapshot
                .generation,
            1
        );
        assert!(inspect_unindexed_evidence(&store, &state)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn incident_publication_is_pinned_until_explicit_release() {
        let prepared = prepared_evidence_for_class(EvidenceClassV1::Incident);
        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-evidence-publication")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut opened = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();

        publish_prepared_evidence(
            &store,
            &mut opened.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .unwrap();
        let pin = state
            .pins
            .values()
            .find(|pin| pin.evidence_id == "schedule-1")
            .expect("incident publication must create its active pin");
        assert_eq!(
            pin.pin_id,
            format!("incident-pin:{}", local_file::sha256_hex(b"schedule-1"))
        );
        assert_eq!(pin.lifecycle, PinLifecycleV1::Active);
        let pin_id = pin.pin_id.clone();
        assert!(state
            .begin_tombstone(
                "tombstone-incident",
                "schedule-1",
                "retention_expired",
                i64::MAX,
            )
            .is_err());

        let released_at_ms = 1_000_003;
        state
            .unpin(&pin_id, "operator incident release", released_at_ms)
            .unwrap();
        let full_retain_until_ms = add_days(released_at_ms, 180).unwrap();
        assert_eq!(
            state.entries["schedule-1"].full_retain_until_ms,
            full_retain_until_ms
        );
        state
            .begin_tombstone(
                "tombstone-incident",
                "schedule-1",
                "retention_expired",
                full_retain_until_ms,
            )
            .unwrap();
    }

    #[test]
    fn cold_copy_transition_binds_consent_root_and_allows_one_way_cache_eviction() {
        let mut state = model();
        state.insert_entry(entry("evidence-1", 1, 512)).unwrap();
        let first = EvidenceStateSnapshotV1::first(state.clone(), 20_000_000_000).unwrap();
        let archive_path = RelativeEvidencePathV1 {
            components: vec!["evidence-1.tar.gz".into()],
        };
        let manifest_path = RelativeEvidencePathV1 {
            components: vec!["evidence-1.manifest.json".into()],
        };
        let binding = ColdStorageBindingV1::OwnerIcloud {
            consent_id: "consent-1".into(),
            consent_sha256: digest('d'),
            root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
        };
        let copy = ColdCopyRecordV1 {
            copy_id: "cold-copy-1".into(),
            evidence_id: "evidence-1".into(),
            archive_sha256: digest('a'),
            archive_bytes: 512,
            manifest_sha256: digest('b'),
            manifest_bytes: 128,
            consent_id: "consent-1".into(),
            consent_sha256: digest('d'),
            consent_revocation_generation: 1,
            cold_root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
            archive_path: archive_path.clone(),
            manifest_path: manifest_path.clone(),
            deadline_ms: 20_000_000_100,
            admitted_at_ms: 20_000_000_001,
            lifecycle: ColdCopyLifecycleV1::Admitted,
        };
        let mut deleting = state.clone();
        deleting
            .begin_tombstone(
                "tombstone-before-admission",
                "evidence-1",
                "retention_expired",
                i64::MAX,
            )
            .unwrap();
        assert!(deleting
            .admit_cold_copy(binding.clone(), copy.clone())
            .is_err());
        state.admit_cold_copy(binding, copy).unwrap();
        let admitted = first.successor(state.clone(), 20_000_000_002).unwrap();
        validate_evidence_state_transition(&first, &admitted).unwrap();

        assert!(state
            .abandon_cold_copy("cold-copy-1", "deadline_expired", 20_000_000_050,)
            .is_err());

        let observation = |path: RelativeEvidencePathV1| FileProviderObservationV1 {
            cold_root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
            object_path: OptionalRelativeEvidencePathV1::RelativePath { value: path },
            state: FileProviderObjectStateV1::Known {
                materialization: FileProviderMaterializationV1::Materialized,
                synchronization: FileProviderSynchronizationV1::Synchronized,
            },
            observed_at_ms: 20_000_000_003,
        };
        state
            .publish_cold_copy(
                "cold-copy-1",
                observation(archive_path.clone()),
                observation(manifest_path),
                20_000_000_003,
            )
            .unwrap();
        let published = admitted.successor(state.clone(), 20_000_000_004).unwrap();
        validate_evidence_state_transition(&admitted, &published).unwrap();
        assert_eq!(
            state.entries["evidence-1"].cold_path,
            OptionalRelativeEvidencePathV1::RelativePath {
                value: archive_path
            }
        );

        state.mark_hot_evidence_absent("evidence-1").unwrap();
        let evicted = published.successor(state.clone(), 20_000_000_005).unwrap();
        validate_evidence_state_transition(&published, &evicted).unwrap();
        assert!(!state.entries["evidence-1"].hot_present);

        let mut resurrected = state;
        resurrected
            .entries
            .get_mut("evidence-1")
            .unwrap()
            .hot_present = true;
        let resurrected = evicted.successor(resurrected, 20_000_000_006).unwrap();
        assert!(validate_evidence_state_transition(&evicted, &resurrected).is_err());
    }

    #[test]
    fn cold_only_validation_uses_the_published_replacement_after_abandonment() {
        let mut state = model();
        state.insert_entry(entry("evidence-1", 1, 512)).unwrap();
        let binding = ColdStorageBindingV1::OwnerIcloud {
            consent_id: "consent-1".into(),
            consent_sha256: digest('d'),
            root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
        };
        let copy = |copy_id: &str, admitted_at_ms: i64, deadline_ms: i64| ColdCopyRecordV1 {
            copy_id: copy_id.into(),
            evidence_id: "evidence-1".into(),
            archive_sha256: digest('a'),
            archive_bytes: 512,
            manifest_sha256: digest('b'),
            manifest_bytes: 128,
            consent_id: "consent-1".into(),
            consent_sha256: digest('d'),
            consent_revocation_generation: 1,
            cold_root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
            archive_path: RelativeEvidencePathV1 {
                components: vec![format!("{copy_id}.tar.gz")],
            },
            manifest_path: RelativeEvidencePathV1 {
                components: vec![format!("{copy_id}.manifest.json")],
            },
            deadline_ms,
            admitted_at_ms,
            lifecycle: ColdCopyLifecycleV1::Admitted,
        };
        state
            .admit_cold_copy(binding.clone(), copy("cold-copy-1", 10, 20))
            .unwrap();
        state
            .abandon_cold_copy("cold-copy-1", "deadline_expired", 21)
            .unwrap();
        let replacement = copy("cold-copy-2", 22, 30);
        let archive_path = replacement.archive_path.clone();
        let manifest_path = replacement.manifest_path.clone();
        state.admit_cold_copy(binding, replacement).unwrap();
        let observation = |path: RelativeEvidencePathV1| FileProviderObservationV1 {
            cold_root_sha256: digest('e'),
            file_provider_domain_id: "icloud-domain-1".into(),
            object_path: OptionalRelativeEvidencePathV1::RelativePath { value: path },
            state: FileProviderObjectStateV1::Known {
                materialization: FileProviderMaterializationV1::Materialized,
                synchronization: FileProviderSynchronizationV1::Synchronized,
            },
            observed_at_ms: 23,
        };
        state
            .publish_cold_copy(
                "cold-copy-2",
                observation(archive_path),
                observation(manifest_path),
                23,
            )
            .unwrap();
        state.mark_hot_evidence_absent("evidence-1").unwrap();
    }

    #[test]
    fn checked_in_r3b_migration_manifest_binds_both_exact_incidents() {
        let manifest = r3b_incident_migration_manifest().unwrap();
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.incidents.len(), 2);
        assert_eq!(
            manifest.incidents[0],
            IncidentMigrationItemV1 {
                incident_id: "inc-r3b-claude-oauth-expiry-2026-07-16".into(),
                incident_reference: "INC-R3B-CLAUDE-OAUTH-EXPIRY-2026-07-16".into(),
                source_path: "/private/tmp/a2a-bridge-r3b-live.EeBAyf/pinned-aggregate.json".into(),
                expected_mode: 0o600,
                expected_length_bytes: 25_128,
                expected_sha256: "7f718f32743170fd7ae73a3027c870f052a8fabbd282762554922abf5e1571c1"
                    .into(),
                affected_case_ids: vec![
                    "claude-host-acp-044-fable".into(),
                    "claude-reader-055-fable".into(),
                    "codex-host-bridge-gpt56-sol".into(),
                    "codex-reader-bridge-gpt56-sol".into(),
                ],
            }
        );
        assert_eq!(
            manifest.incidents[1],
            IncidentMigrationItemV1 {
                incident_id: "inc-r3b-container-start-stall-2026-07-16".into(),
                incident_reference: "INC-R3B-CONTAINER-START-STALL-2026-07-16".into(),
                source_path: "/private/tmp/a2a-bridge-r3b-live2.mbOljW/pinned-aggregate.json"
                    .into(),
                expected_mode: 0o600,
                expected_length_bytes: 19_894,
                expected_sha256: "319b3cf4b92a36b1f2e2cdd71b7a97fb6d5c4309c2f919a4e3bce39dd28a9b3e"
                    .into(),
                affected_case_ids: vec![
                    "claude-host-acp-044-fable".into(),
                    "claude-reader-055-fable".into(),
                    "codex-host-bridge-gpt56-sol".into(),
                    "codex-reader-bridge-gpt56-sol".into(),
                ],
            }
        );
    }

    #[test]
    fn incident_migration_journal_reopen_rejects_gaps_and_chain_tampering() {
        let source = root();
        let aggregate = aggregate_bytes();
        let first_item = migration_item(
            "incident-journal-first",
            &source.path().join("missing-first.json"),
            &aggregate,
        );
        let second_item = migration_item(
            "incident-journal-second",
            &source.path().join("missing-second.json"),
            &aggregate,
        );
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-migration-journal-reopen")
            .unwrap();
        let mut migration = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();
        migration
            .append(
                &first_item,
                IncidentMigrationDispositionV1::Missing,
                1_000_001,
            )
            .unwrap();
        migration
            .append(
                &second_item,
                IncidentMigrationDispositionV1::Missing,
                1_000_002,
            )
            .unwrap();
        drop(migration);

        let first_path = state_root
            .path()
            .join("migration/incident-migration.00000000000000000001.json");
        let second_path = state_root
            .path()
            .join("migration/incident-migration.00000000000000000002.json");
        let first_bytes = std::fs::read(&first_path).unwrap();
        std::fs::remove_file(&first_path).unwrap();
        let gap_error = FileIncidentMigrationJournal::open_existing_or_empty(&lock)
            .err()
            .unwrap();
        assert!(gap_error
            .to_string()
            .contains("incident migration filenames are not contiguous"));
        write_private(&first_path, &first_bytes);
        assert_eq!(
            FileIncidentMigrationJournal::open_existing_or_empty(&lock)
                .unwrap()
                .records()
                .len(),
            2
        );

        let mut second: IncidentMigrationRecordV1 =
            serde_json::from_slice(&std::fs::read(&second_path).unwrap()).unwrap();
        second.previous_record = OptionalSha256V1::Sha256 { value: digest('f') };
        write_private(&second_path, &canonical_json(&second).unwrap());
        let chain_error = FileIncidentMigrationJournal::open_existing_or_empty(&lock)
            .err()
            .unwrap();
        assert!(chain_error
            .to_string()
            .contains("incident migration chain is not monotonic"));
    }

    #[test]
    fn migration_interruption_before_atomic_publish_reopens_and_retries_same_generation() {
        let source = root();
        let aggregate = aggregate_bytes();
        let item = migration_item(
            "incident-interrupted-publication",
            &source.path().join("missing-interrupted.json"),
            &aggregate,
        );
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/migration-interrupted-publication")
            .unwrap();
        let migration_directory = lock.migration_directory();
        let temporary = migration_directory
            .canonical_path()
            .join(".a2a-journal-append-v1.tmp");
        let final_record = migration_directory
            .canonical_path()
            .join(FileIncidentMigrationJournal::generation_name(1));
        let mut journal = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();

        migration_directory.fail_journal_publish_on_nth_call_for_test(1);
        let error = journal
            .append(&item, IncidentMigrationDispositionV1::Missing, 1_000_001)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("injected failure before journal publication"));
        assert!(temporary.is_file());
        assert!(!final_record.exists());
        drop(journal);

        let mut reopened = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();
        assert!(reopened.records().is_empty());
        reopened
            .append(&item, IncidentMigrationDispositionV1::Missing, 1_000_001)
            .unwrap();
        assert!(!temporary.exists());
        assert!(final_record.is_file());
        assert_eq!(
            FileIncidentMigrationJournal::open_existing_or_empty(&lock)
                .unwrap()
                .records()
                .len(),
            1
        );
    }

    #[test]
    fn incident_migration_publishes_and_pins_exact_source_once() {
        let source = root();
        let aggregate = aggregate_bytes();
        let source_path = source.path().join("pinned-aggregate.json");
        write_private(&source_path, &aggregate);
        let item = migration_item("incident-migration-1", &source_path, &aggregate);

        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-migration")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut evidence = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let mut migration = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();
        let mut usage = empty_hot_usage();

        let first = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_002,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(first.evidence_published);
        assert!(first.journal_appended);
        assert!(matches!(
            first.disposition,
            IncidentMigrationDispositionV1::Migrated { .. }
        ));
        assert_eq!(
            state.entries[&item.incident_id].evidence_class,
            EvidenceClassV1::Incident
        );
        assert_eq!(
            serde_json::from_str::<CompactEvidenceRecordV1>(
                &state.entries[&item.incident_id].compact_record
            )
            .unwrap()
            .aggregate_sha256,
            OptionalSha256V1::Sha256 {
                value: item.expected_sha256.clone()
            }
        );
        assert_eq!(
            state
                .pins
                .values()
                .filter(|pin| pin.evidence_id == item.incident_id)
                .count(),
            1
        );
        assert_eq!(migration.records().len(), 1);

        let duplicate = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_003,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(!duplicate.evidence_published);
        assert!(!duplicate.journal_appended);
        assert_eq!(migration.records().len(), 1);
        assert_eq!(
            FileEvidenceJournal::open_existing(&lock)
                .unwrap()
                .snapshot
                .generation,
            2
        );
    }

    #[test]
    fn incident_migration_records_missing_and_mismatch_without_evidence() {
        let source = root();
        let aggregate = aggregate_bytes();
        let missing_path = source.path().join("missing-aggregate.json");
        let missing_item = migration_item("incident-missing", &missing_path, &aggregate);

        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-migration-missing")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut evidence = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let mut migration = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();
        let mut usage = empty_hot_usage();
        let missing = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &missing_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_002,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert_eq!(missing.disposition, IncidentMigrationDispositionV1::Missing);
        assert!(missing.journal_appended);
        assert!(!missing.evidence_published);
        assert!(state.entries.is_empty());

        let mismatch_path = source.path().join("mismatch-aggregate.json");
        let mut changed = aggregate.clone();
        let changed_index = changed.iter().position(|byte| *byte == b'1').unwrap();
        changed[changed_index] = b'2';
        write_private(&mismatch_path, &changed);
        let mismatch_item = migration_item("incident-mismatch", &mismatch_path, &aggregate);
        let mismatch = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &mismatch_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_003,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            mismatch.disposition,
            IncidentMigrationDispositionV1::Mismatch {
                ref reason_code,
                observed_sha256: OptionalSha256V1::Sha256 { ref value },
                ..
            } if reason_code == "hash_mismatch" && value == &local_file::sha256_hex(&changed)
        ));

        let mode_path = source.path().join("mode-mismatch-aggregate.json");
        write_private(&mode_path, &aggregate);
        std::fs::set_permissions(&mode_path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let mode_item = migration_item("incident-mode-mismatch", &mode_path, &aggregate);
        let mode_mismatch = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &mode_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_004,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            mode_mismatch.disposition,
            IncidentMigrationDispositionV1::Mismatch {
                ref reason_code,
                observed_mode: 0o640,
                observed_sha256: OptionalSha256V1::Absent,
                ..
            } if reason_code == "mode_mismatch"
        ));

        let length_path = source.path().join("length-mismatch-aggregate.json");
        let mut longer = aggregate.clone();
        longer.push(b'\n');
        write_private(&length_path, &longer);
        let length_item = migration_item("incident-length-mismatch", &length_path, &aggregate);
        let length_mismatch = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &length_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_005,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            length_mismatch.disposition,
            IncidentMigrationDispositionV1::Mismatch {
                ref reason_code,
                observed_length_bytes,
                ..
            } if reason_code == "length_mismatch"
                && observed_length_bytes == longer.len() as u64
        ));

        let linked_path = source.path().join("linked-aggregate.json");
        let linked_peer = source.path().join("linked-peer.json");
        write_private(&linked_path, &aggregate);
        std::fs::hard_link(&linked_path, &linked_peer).unwrap();
        let linked_item = migration_item("incident-linked", &linked_path, &aggregate);
        let linked = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &linked_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_006,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(matches!(
            linked.disposition,
            IncidentMigrationDispositionV1::Mismatch {
                ref reason_code,
                ..
            } if reason_code == "unsafe_source_metadata"
        ));

        let invalid_path = source.path().join("invalid-aggregate.json");
        let invalid = b"{\"not\":\"an aggregate\"}\n";
        write_private(&invalid_path, invalid);
        let invalid_item = migration_item("incident-invalid-aggregate", &invalid_path, invalid);
        assert!(migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &invalid_item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_007,
            IncidentMigrationFailpointV1::None,
        )
        .is_err());
        assert!(state.entries.is_empty());
        assert_eq!(migration.records().len(), 5);
        assert_eq!(
            std::fs::metadata(
                state_root
                    .path()
                    .join("migration/incident-migration.00000000000000000001.json")
            )
            .unwrap()
            .permissions()
            .mode()
                & 0o777,
            0o600
        );
        drop(migration);
        assert_eq!(
            FileIncidentMigrationJournal::open_existing_or_empty(&lock)
                .unwrap()
                .records()
                .len(),
            5
        );
    }

    #[test]
    fn incident_migration_recovers_post_publication_crash_without_repinning() {
        let source = root();
        let aggregate = aggregate_bytes();
        let source_path = source.path().join("pinned-aggregate.json");
        write_private(&source_path, &aggregate);
        let item = migration_item("incident-crash-recovery", &source_path, &aggregate);

        let (_hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-migration-crash")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut evidence = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let mut migration = FileIncidentMigrationJournal::open_existing_or_empty(&lock).unwrap();
        let mut usage = empty_hot_usage();

        assert!(migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_002,
            IncidentMigrationFailpointV1::AfterEvidencePublication,
        )
        .is_err());
        assert!(state.entries.contains_key(&item.incident_id));
        assert!(migration.records().is_empty());
        assert_eq!(state.pins.len(), 1);

        let recovered = migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &item,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_003,
            IncidentMigrationFailpointV1::None,
        )
        .unwrap();
        assert!(!recovered.evidence_published);
        assert!(recovered.journal_appended);
        assert_eq!(state.pins.len(), 1);
        assert_eq!(migration.records().len(), 1);

        let mut changed_identity = item.clone();
        changed_identity.expected_sha256 = digest('f');
        assert!(migrate_incident_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &mut migration,
            &changed_identity,
            &HotStorageCapsV1::approved(),
            &mut usage,
            1_000_004,
            IncidentMigrationFailpointV1::None,
        )
        .is_err());
        assert_eq!(state.pins.len(), 1);
        assert_eq!(migration.records().len(), 1);
    }

    #[test]
    fn incident_migration_recovery_rejects_coherent_noncanonical_payload_identity() {
        let source = root();
        let aggregate = aggregate_bytes();
        let source_path = source.path().join("pinned-aggregate.json");
        write_private(&source_path, &aggregate);
        let item = migration_item("incident-recovery-identity", &source_path, &aggregate);

        let (hot_root, store) = test_hot_store();
        let state_root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(state_root.path()).unwrap();
        let lock = scheduler
            .try_owner_admission("test/incident-migration-recovery-identity")
            .unwrap();
        let mut state =
            EvidenceStateModelV1::new(store.root_sha256().to_owned(), ColdStorageBindingV1::Absent)
                .unwrap();
        let mut evidence = FileEvidenceJournal::initialize(&lock, &state, 1_000_001).unwrap();
        let prepared = prepare_incident_migration_evidence(&item, aggregate, 1_000_002).unwrap();
        publish_prepared_evidence(
            &store,
            &mut evidence.journal,
            &mut state,
            &prepared,
            &HotStorageCapsV1::approved(),
            1_000_002,
            SealPublicationFailpointV1::None,
        )
        .unwrap();

        let object_name = payload_object_name(&item.incident_id).unwrap();
        let manifest_path = hot_root
            .path()
            .join("sealed")
            .join(object_name)
            .join("manifest.json");
        let mut manifest: SealedEvidenceManifestV1 =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest.created_at_ms += 1;
        manifest.source_tree_sha256 = digest('f');
        let manifest_bytes = canonical_json(&manifest).unwrap();
        let manifest_sha256 = local_file::sha256_hex(&manifest_bytes);
        write_private(&manifest_path, &manifest_bytes);

        let entry = state.entries.get_mut(&item.incident_id).unwrap();
        let mut compact: CompactEvidenceRecordV1 =
            serde_json::from_str(&entry.compact_record).unwrap();
        compact.affected_case_ids = vec!["case-2".into()];
        compact.manifest_sha256 = manifest_sha256.clone();
        let compact_bytes = canonical_json(&compact).unwrap();
        entry.manifest_sha256 = manifest_sha256;
        entry.manifest_bytes = manifest_bytes.len() as u64;
        entry.compact_record_sha256 = local_file::sha256_hex(&compact_bytes);
        entry.compact_record_bytes = compact_bytes.len() as u64;
        entry.compact_record = String::from_utf8(compact_bytes).unwrap();
        state.validate().unwrap();

        let error = existing_incident_migration_disposition(&store, &state, &item).unwrap_err();
        assert!(error
            .to_string()
            .contains("existing incident payload is not this migration"));
    }

    #[test]
    fn evidence_state_directory_is_not_exposed_without_owner_lock() {
        fn require_capability<C: crate::compatibility_schedule_state::EvidenceStateCapability>(
            _: &C,
        ) {
        }

        let root = root();
        let scheduler = SchedulerStateRoot::initialize_for_test(Path::new(root.path())).unwrap();
        let lock = scheduler
            .try_owner_admission("test/evidence-capability")
            .unwrap();
        require_capability(&lock);
    }
}
